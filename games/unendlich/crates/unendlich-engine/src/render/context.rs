//! Vulkan context for graphics rendering — instance, device, queues, surface.
//!
//! Separate from `world::gpu_generator::VulkanChunkGenerator`, which is a
//! compute-only context. The triangle renderer needs graphics + present
//! capability and a window surface, so this is its own type. A future
//! integration could share an instance and a device, but that's an
//! optimization — for now keeping them separate keeps the failure modes
//! cleanly partitioned.

use ash::extensions::khr;
use ash::{vk, Device, Entry, Instance};
use std::ffi::{CStr, CString};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub enum ContextError {
    Vulkan(vk::Result),
    NoSuitableDevice,
    NoGraphicsQueue,
    NoPresentQueue,
    Other(String),
}

impl From<vk::Result> for ContextError {
    fn from(r: vk::Result) -> Self { ContextError::Vulkan(r) }
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContextError::Vulkan(r) => write!(f, "Vulkan error: {:?}", r),
            ContextError::NoSuitableDevice => write!(f, "No Vulkan device with graphics + present + swapchain support"),
            ContextError::NoGraphicsQueue => write!(f, "No graphics-capable queue family"),
            ContextError::NoPresentQueue => write!(f, "No present-capable queue family"),
            ContextError::Other(s) => write!(f, "{}", s),
        }
    }
}
impl std::error::Error for ContextError {}

pub type Result<T> = std::result::Result<T, ContextError>;

pub struct VulkanContext {
    pub entry: Entry,
    pub instance: Instance,
    pub surface_loader: khr::Surface,
    pub surface: vk::SurfaceKHR,
    pub physical_device: vk::PhysicalDevice,
    pub device: Device,
    pub graphics_queue_family: u32,
    pub present_queue_family: u32,
    pub graphics_queue: vk::Queue,
    pub present_queue: vk::Queue,
    /// v0.24.3 Ship A: queue family chosen for async compute work
    /// (mesher dispatches, world generation). May be the same as
    /// `graphics_queue_family` if no dedicated compute family exists
    /// on this device.
    pub compute_queue_family: u32,
    /// v0.24.3 Ship A: queue handle for async compute work. Three
    /// possible states (see context.rs::new for the full taxonomy):
    ///   - Different family than graphics: true async lane.
    ///   - Same family, second queue index: separate handle, may
    ///     serialize on hardware but the OS scheduler can arbitrate.
    ///   - Same family, only one queue available: equal to
    ///     `graphics_queue`. No benefit; callers should ignore.
    /// Use `compute_queue_is_distinct` to disambiguate.
    pub compute_queue: vk::Queue,
    /// v0.24.3 Ship A: true if `compute_queue` provides any
    /// independence from `graphics_queue` (different handle, possibly
    /// different hardware lane). If false, using compute_queue is
    /// equivalent to using graphics_queue and Ship B onward should
    /// short-circuit to the graphics path.
    pub compute_queue_is_distinct: bool,
    pub swapchain_loader: khr::Swapchain,
    /// v0.24.21-gpu-mesher-prod: serializes `vkQueueSubmit` calls on
    /// `graphics_queue` across threads. The Vulkan spec requires
    /// external synchronization for queue submissions to the same
    /// `VkQueue` handle. Pre-v0.24.21 the renderer was the only thread
    /// submitting, so no lock was needed. v0.24.21 wires
    /// `SparseGpuMesher` into the meshing pool — that adds a second
    /// thread (the GPU dispatcher worker) that submits compute work to
    /// the same queue. Both call sites acquire this mutex briefly
    /// around the submit (NOT around the fence-wait), so per-frame
    /// contention is in the microseconds.
    ///
    /// If `compute_queue_is_distinct` is true, the mesher can in
    /// principle use `compute_queue` without this lock. For
    /// v0.24.21 we keep things simple and always acquire — the cost
    /// is negligible. A later ship can split.
    pub queue_submit_lock: Arc<Mutex<()>>,
}

impl VulkanContext {
    /// Build a graphics-capable Vulkan context for the given GLFW window.
    ///
    /// `window_extensions` is the list returned by
    /// `glfw.get_required_instance_extensions()`. We add `VK_KHR_surface` if
    /// it isn't already there (it always is on supported platforms, but
    /// belt-and-suspenders).
    #[cfg(feature = "window")]
    pub fn new(
        glfw: &glfw::Glfw,
        window: &glfw::Window,
    ) -> Result<Self> {
        let entry = unsafe { Entry::load() }
            .map_err(|e| ContextError::Other(format!("Failed to load Vulkan: {}", e)))?;

        if !glfw.vulkan_supported() {
            return Err(ContextError::Other("GLFW reports Vulkan is not supported".into()));
        }

        let instance = create_instance(&entry, glfw)?;

        let surface_loader = khr::Surface::new(&entry, &instance);
        let surface = create_surface(window, &instance)?;

        let picked = pick_physical_device(&instance, &surface_loader, surface)?;
        let physical_device       = picked.device;
        let graphics_queue_family = picked.graphics_qf;
        let present_queue_family  = picked.present_qf;
        let compute_queue_family  = picked.compute_qf;
        let compute_dedicated     = picked.compute_dedicated;

        log::info!(
            "Picked physical device {} (graphics qf={}, present qf={}, compute qf={} dedicated={})",
            unsafe {
                CStr::from_ptr(instance.get_physical_device_properties(physical_device).device_name.as_ptr())
                    .to_string_lossy()
            },
            graphics_queue_family,
            present_queue_family,
            compute_queue_family,
            compute_dedicated,
        );

        let device = create_device(
            &instance, physical_device,
            graphics_queue_family, present_queue_family,
            compute_queue_family, compute_dedicated,
        )?;
        let graphics_queue = unsafe { device.get_device_queue(graphics_queue_family, 0) };
        let present_queue  = unsafe { device.get_device_queue(present_queue_family, 0) };

        // v0.24.3 Ship A: fetch the compute queue.
        //
        // Three cases handled:
        //   (a) compute_queue_family != graphics_queue_family
        //       Dedicated compute family. Queue index 0 within that
        //       family. True async-compute capability — separate
        //       hardware lane on most desktop GPUs.
        //   (b) compute_queue_family == graphics_queue_family AND
        //       create_device requested a SECOND queue from this family
        //       Queue index 1. Same hardware lane but separate queue
        //       handle; the OS scheduler / driver can arbitrate.
        //   (c) compute_queue_family == graphics_queue_family AND
        //       only one queue was available
        //       Falls back to queue index 0 (same as graphics_queue).
        //       No async benefit; mesher should keep submitting via
        //       graphics_queue. This is the "Ship A discovers we only
        //       have one queue" case.
        //
        // The `compute_queue_is_distinct` flag tells callers (the
        // mesher in Ship B+) whether using compute_queue gives them
        // anything beyond a duplicate handle.
        let qf_props = unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
        let compute_family_queue_count = qf_props
            .get(compute_queue_family as usize)
            .map(|p| p.queue_count)
            .unwrap_or(1);

        let (compute_queue, compute_queue_is_distinct) =
            if compute_queue_family != graphics_queue_family {
                // Case (a): dedicated family.
                (unsafe { device.get_device_queue(compute_queue_family, 0) }, true)
            } else if compute_family_queue_count >= 2 {
                // Case (b): same family, second queue.
                (unsafe { device.get_device_queue(compute_queue_family, 1) }, true)
            } else {
                // Case (c): same family, single queue. Reuse graphics queue.
                log::warn!(
                    "Compute queue family has only 1 queue and matches graphics — \
                     compute_queue will be identical to graphics_queue. Async \
                     compute will not provide queue-level parallelism on this device."
                );
                (graphics_queue, false)
            };

        log::info!(
            "Compute queue acquired: family={} distinct={} (graphics handle reused: {})",
            compute_queue_family,
            compute_queue_is_distinct,
            !compute_queue_is_distinct,
        );

        let swapchain_loader = khr::Swapchain::new(&instance, &device);

        Ok(Self {
            entry, instance, surface_loader, surface,
            physical_device, device,
            graphics_queue_family, present_queue_family,
            graphics_queue, present_queue,
            compute_queue_family, compute_queue, compute_queue_is_distinct,
            swapchain_loader,
            queue_submit_lock: Arc::new(Mutex::new(())),
        })
    }

    /// T06-R27 (GLFW_DECOUPLING Approach A): build a graphics-capable Vulkan
    /// context with **no window and no glfw** — for offscreen rendering on
    /// lavapipe / CI. Instance is created without surface extensions; the
    /// device is picked by graphics+compute capability only (no present
    /// query); `surface` is left null and the present path is a no-op. Render
    /// into an offscreen target via `create_offscreen_target` (+ `depth.rs`
    /// for depth); read it back for screenshots/parity if needed.
    ///
    /// The `surface_loader` / `swapchain_loader` fields are still constructed
    /// (cheap function-pointer loaders) so the struct layout matches the
    /// windowed build byte-for-byte; they are never *called* headless (and
    /// `Drop` skips surface teardown when `surface` is null).
    #[cfg(feature = "headless")]
    pub fn new_headless() -> Result<Self> {
        let entry = unsafe { Entry::load() }
            .map_err(|e| ContextError::Other(format!("Failed to load Vulkan: {}", e)))?;

        let instance = create_instance_headless(&entry)?;

        // Loaders are safe to construct even though no surface extension is
        // enabled — ash only resolves the function pointers here; we never
        // invoke surface entry points on the headless path.
        let surface_loader = khr::Surface::new(&entry, &instance);
        let surface = vk::SurfaceKHR::null();

        let picked = pick_physical_device_headless(&instance)?;
        let physical_device       = picked.device;
        let graphics_queue_family = picked.graphics_qf;
        let present_queue_family  = picked.present_qf; // == graphics_qf headless
        let compute_queue_family  = picked.compute_qf;
        let compute_dedicated     = picked.compute_dedicated;

        log::info!(
            "Headless context: picked device {} (graphics qf={}, compute qf={} dedicated={}) — no surface",
            unsafe {
                CStr::from_ptr(instance.get_physical_device_properties(physical_device).device_name.as_ptr())
                    .to_string_lossy()
            },
            graphics_queue_family, compute_queue_family, compute_dedicated,
        );

        let device = create_device(
            &instance, physical_device,
            graphics_queue_family, present_queue_family,
            compute_queue_family, compute_dedicated,
        )?;
        let graphics_queue = unsafe { device.get_device_queue(graphics_queue_family, 0) };
        let present_queue  = graphics_queue; // no present queue headless

        let qf_props = unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
        let compute_family_queue_count = qf_props
            .get(compute_queue_family as usize)
            .map(|p| p.queue_count)
            .unwrap_or(1);
        let (compute_queue, compute_queue_is_distinct) =
            if compute_queue_family != graphics_queue_family {
                (unsafe { device.get_device_queue(compute_queue_family, 0) }, true)
            } else if compute_family_queue_count >= 2 {
                (unsafe { device.get_device_queue(compute_queue_family, 1) }, true)
            } else {
                (graphics_queue, false)
            };

        let swapchain_loader = khr::Swapchain::new(&instance, &device);

        Ok(Self {
            entry, instance, surface_loader, surface,
            physical_device, device,
            graphics_queue_family, present_queue_family,
            graphics_queue, present_queue,
            compute_queue_family, compute_queue, compute_queue_is_distinct,
            swapchain_loader,
            queue_submit_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Allocate an offscreen color render target (COLOR_ATTACHMENT | SAMPLED).
    /// The headless analogue of the swapchain image — render the scene here,
    /// then sample/read it back. Pair with `depth.rs` for a depth attachment.
    /// Available on any render-core build (windowed builds can use it for the
    /// post-FX scene target too).
    pub fn create_offscreen_target(
        &self,
        extent: vk::Extent2D,
        format: vk::Format,
    ) -> Result<crate::render::offscreen::OffscreenTarget> {
        crate::render::offscreen::OffscreenTarget::new(
            &self.instance, &self.device, self.physical_device, extent, format,
        ).map_err(ContextError::Vulkan)
    }

    /// Capture GPU + driver identity at the current moment.
    ///
    /// Reads `VkPhysicalDeviceProperties` (always available) and tries
    /// to read `VK_KHR_driver_properties` (Vulkan 1.2+ core, also available
    /// as extension on most 1.0/1.1 drivers). If the driver properties
    /// can't be queried, returns a `GpuInfo` with empty `driver_name`
    /// and `driver_info` — still enough to identify the device by
    /// vendor/device ID.
    ///
    /// c.13-scout: also queries multiview support and attaches it to the
    /// returned GpuInfo. Multiview is not part of the driver_hash (so it
    /// doesn't perturb cross-version comparisons), just an informational
    /// side-channel for the launcher to surface.
    ///
    /// This is intentionally side-effect-free and read-only: it queries
    /// data the driver already exposes. No allocations on the GPU, no
    /// state changes. Safe to call at any point after construction.
    pub fn gpu_info(&self) -> unendlich_shared::test_spec::GpuInfo {
        // Core properties — always available.
        let base_props = unsafe {
            self.instance.get_physical_device_properties(self.physical_device)
        };
        let vendor_id = base_props.vendor_id;
        let device_id = base_props.device_id;
        let driver_version_raw = base_props.driver_version;

        // Driver properties via pNext chain. Available in Vulkan 1.2 core,
        // or via VK_KHR_driver_properties extension. We attempt and
        // gracefully degrade if the chain isn't supported.
        //
        // The PhysicalDeviceDriverProperties struct has fixed-size
        // `driver_name` and `driver_info` arrays (256 chars each); we
        // copy them into Rust strings up to the first null byte.
        let mut driver_props = vk::PhysicalDeviceDriverProperties::default();
        let mut props2 = vk::PhysicalDeviceProperties2::builder()
            .push_next(&mut driver_props)
            .build();
        unsafe {
            self.instance.get_physical_device_properties2(
                self.physical_device,
                &mut props2,
            );
        }

        // Convert fixed-size C-style arrays to owned Rust strings.
        let driver_name = c_array_to_string(&driver_props.driver_name);
        let driver_info = c_array_to_string(&driver_props.driver_info);

        let multiview = self.multiview_support();

        unendlich_shared::test_spec::GpuInfo::new(
            vendor_id,
            device_id,
            driver_version_raw,
            driver_name,
            driver_info,
        ).with_multiview(multiview)
    }

    /// c.13-scout: probe `VK_KHR_multiview` (Vulkan 1.1 core) support.
    ///
    /// Multiview lets a single render pass write to multiple layers of a
    /// layered framebuffer, with `gl_ViewIndex` available in shaders.
    /// This is the foundation for one-pass cascaded shadow maps in
    /// c.17 — we need to know up front whether the current driver
    /// supports it. Asahi's Honeykrisp + Mesa state-of-the-art as of
    /// the 25.x series is not documented to claim multiview, but the
    /// query is cheap and the answer determines whether c.17's
    /// multiview-CSM is feasible or whether we need to pivot to
    /// per-cascade culling + frustum tightening.
    ///
    /// Reads both feature and property tables:
    ///   - `PhysicalDeviceMultiviewFeatures::multiview` — is it on?
    ///   - `PhysicalDeviceMultiviewProperties::max_multiview_view_count` —
    ///     how many layers can one pass touch?
    ///   - `PhysicalDeviceMultiviewProperties::max_multiview_instance_index`
    ///     — limit on instanced draws within multiview (not relevant for
    ///     CSM, recorded for completeness).
    ///
    /// Returns `MultiviewSupport::default()` (all-false) if the query
    /// chain isn't supported by the driver — equivalent to "no multiview."
    pub fn multiview_support(&self) -> unendlich_shared::test_spec::MultiviewSupport {
        // Query features.
        let mut features = vk::PhysicalDeviceMultiviewFeatures::default();
        let mut features2 = vk::PhysicalDeviceFeatures2::builder()
            .push_next(&mut features)
            .build();
        unsafe {
            self.instance.get_physical_device_features2(
                self.physical_device,
                &mut features2,
            );
        }

        // Query properties.
        let mut props = vk::PhysicalDeviceMultiviewProperties::default();
        let mut props2 = vk::PhysicalDeviceProperties2::builder()
            .push_next(&mut props)
            .build();
        unsafe {
            self.instance.get_physical_device_properties2(
                self.physical_device,
                &mut props2,
            );
        }

        unendlich_shared::test_spec::MultiviewSupport {
            multiview: features.multiview != 0,
            max_view_count: props.max_multiview_view_count,
            max_instance_index: props.max_multiview_instance_index,
            geometry_shader: features.multiview_geometry_shader != 0,
            tessellation_shader: features.multiview_tessellation_shader != 0,
        }
    }
}

/// Convert a fixed-size C array of `c_char` (typically 256) into an
/// owned Rust String, stopping at the first null byte. Lossy on
/// invalid UTF-8 (driver strings are conventionally ASCII).
fn c_array_to_string(arr: &[std::os::raw::c_char]) -> String {
    let end = arr.iter().position(|&c| c == 0).unwrap_or(arr.len());
    let bytes: Vec<u8> = arr[..end].iter().map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_device(None);
            // Headless contexts carry a null surface (and may not have the
            // surface extension loaded) — only tear down a real one.
            if self.surface != vk::SurfaceKHR::null() {
                self.surface_loader.destroy_surface(self.surface, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

#[cfg(feature = "window")]
fn create_instance(entry: &Entry, glfw: &glfw::Glfw) -> Result<Instance> {
    let app_name = CString::new("unendlich-triangle").unwrap();
    let engine_name = CString::new("unendlich").unwrap();

    let app_info = vk::ApplicationInfo::builder()
        .application_name(&app_name)
        .application_version(vk::make_api_version(0, 0, 2, 0))
        .engine_name(&engine_name)
        .engine_version(vk::make_api_version(0, 0, 2, 0))
        .api_version(vk::make_api_version(0, 1, 2, 0));

    // Get the surface extensions GLFW says we need (e.g. VK_KHR_xcb_surface).
    //
    // NB: `c_char` is `i8` on x86_64 but `u8` on aarch64 (and all ARM/AArch64
    // platforms). `CStr::as_ptr()` returns `*const c_char`, and ash expects
    // `&[*const c_char]`. Using a hardcoded `*const i8` here breaks on M-series
    // Macs / Asahi / any other AArch64 target.
    let glfw_exts = glfw.get_required_instance_extensions().unwrap_or_default();
    let glfw_ext_cstrings: Vec<CString> = glfw_exts.into_iter()
        .map(|s| CString::new(s).unwrap())
        .collect();
    #[allow(unused_mut)]
    let mut ext_ptrs: Vec<*const std::os::raw::c_char> = glfw_ext_cstrings.iter()
        .map(|c| c.as_ptr())
        .collect();

    log::info!("Instance extensions requested: {:?}",
        glfw_ext_cstrings.iter().map(|c| c.to_string_lossy()).collect::<Vec<_>>());

    // r174: macOS/MoltenVK — Vulkan is layered over Metal there, and the
    // loader only exposes the device if we opt into portability:
    // VK_KHR_portability_enumeration + the ENUMERATE_PORTABILITY flag
    // (operator's vulkaninfo shows both VK_MVK_macos_surface and the
    // portability extension present). cfg-guarded: Linux path unchanged.
    #[cfg(target_os = "macos")]
    let info = {
        ext_ptrs.push(vk::KhrPortabilityEnumerationFn::name().as_ptr());
        vk::InstanceCreateInfo::builder()
            .application_info(&app_info)
            .enabled_extension_names(&ext_ptrs)
            .flags(vk::InstanceCreateFlags::ENUMERATE_PORTABILITY_KHR)
    };
    #[cfg(not(target_os = "macos"))]
    let info = vk::InstanceCreateInfo::builder()
        .application_info(&app_info)
        .enabled_extension_names(&ext_ptrs);

    Ok(unsafe { entry.create_instance(&info, None)? })
}

#[cfg(feature = "window")]
fn create_surface(
    window: &glfw::Window,
    instance: &Instance,
) -> Result<vk::SurfaceKHR> {
    use ash::vk::Handle;

    // glfw 0.45's `create_window_surface` is typed against `vk_sys`, not `ash::vk`.
    // The handles are layout-compatible at the FFI level (VkInstance is an opaque
    // dispatchable handle = pointer; VkSurfaceKHR is a non-dispatchable handle =
    // u64). We transmute between the two newtypes at this boundary only.
    //
    // SAFETY: this is the same layout-compatibility trick that `ash-window`
    // uses to bridge to winit's loaders.

    // ash::vk::Instance -> *const c_void (pointer) -> vk_sys::Instance
    // (which is also a pointer-sized opaque type)
    let ash_instance_handle: ash::vk::Instance = instance.handle();
    let glfw_instance: vk_sys::Instance = unsafe {
        std::mem::transmute_copy(&ash_instance_handle)
    };

    let mut surface_raw: vk_sys::SurfaceKHR = 0;
    let result = window.create_window_surface(
        glfw_instance,
        std::ptr::null(),
        &mut surface_raw,
    );

    if result != vk_sys::SUCCESS {
        return Err(ContextError::Other(format!(
            "glfwCreateWindowSurface failed: VkResult={}", result
        )));
    }

    // vk_sys::SurfaceKHR is a u64 — same layout as ash::vk::SurfaceKHR.
    Ok(vk::SurfaceKHR::from_raw(surface_raw))
}

/// T06-R27: headless instance — no glfw, no surface extensions. ash `dlopen`s
/// libvulkan at runtime, so this needs no display toolchain (works on lavapipe
/// and in CI). API 1.2 to match the windowed instance.
#[cfg(feature = "headless")]
fn create_instance_headless(entry: &Entry) -> Result<Instance> {
    let app_name = CString::new("unendlich-headless").unwrap();
    let engine_name = CString::new("unendlich").unwrap();
    let app_info = vk::ApplicationInfo::builder()
        .application_name(&app_name)
        .application_version(vk::make_api_version(0, 0, 2, 0))
        .engine_name(&engine_name)
        .engine_version(vk::make_api_version(0, 0, 2, 0))
        .api_version(vk::make_api_version(0, 1, 2, 0));
    // No enabled_extension_names: an offscreen target needs neither
    // VK_KHR_surface nor any platform surface extension.
    let info = vk::InstanceCreateInfo::builder().application_info(&app_info);
    Ok(unsafe { entry.create_instance(&info, None)? })
}

/// T06-R27: headless device pick — graphics + compute capability only, no
/// surface/present query and no `VK_KHR_swapchain` requirement. Mirrors
/// `pick_physical_device`'s scoring + compute-family tiering. `present_qf` is
/// set equal to `graphics_qf` (there is no present on the headless path).
#[cfg(feature = "headless")]
fn pick_physical_device_headless(instance: &Instance) -> Result<PickedDevice> {
    let devices = unsafe { instance.enumerate_physical_devices()? };
    if devices.is_empty() {
        return Err(ContextError::NoSuitableDevice);
    }
    // (device, graphics_qf, compute_qf, compute_dedicated, score)
    let mut best: Option<(vk::PhysicalDevice, u32, u32, bool, u32)> = None;

    for &dev in &devices {
        let qf_props = unsafe { instance.get_physical_device_queue_family_properties(dev) };
        let mut graphics_qf: Option<u32> = None;
        let mut compute_qf: Option<u32> = None;
        let mut compute_is_dedicated = false;

        for (i, props) in qf_props.iter().enumerate() {
            let i = i as u32;
            if props.queue_flags.contains(vk::QueueFlags::GRAPHICS) && graphics_qf.is_none() {
                graphics_qf = Some(i);
            }
            let has_compute  = props.queue_flags.contains(vk::QueueFlags::COMPUTE);
            let has_graphics = props.queue_flags.contains(vk::QueueFlags::GRAPHICS);
            if has_compute && !has_graphics {
                compute_qf = Some(i);
                compute_is_dedicated = true;
            } else if has_compute && compute_qf.is_none() {
                compute_qf = Some(i);
                compute_is_dedicated = false;
            }
        }

        let gqf = match graphics_qf { Some(g) => g, None => continue };
        let cqf = compute_qf.unwrap_or(gqf);
        let c_dedicated = compute_is_dedicated && cqf != gqf;

        let props = unsafe { instance.get_physical_device_properties(dev) };
        let score = match props.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU   => 1000,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 500,
            vk::PhysicalDeviceType::VIRTUAL_GPU    => 100,
            vk::PhysicalDeviceType::CPU            => 10,
            _ => 1,
        };
        match best {
            None => best = Some((dev, gqf, cqf, c_dedicated, score)),
            Some((_, _, _, _, prev)) if score > prev =>
                best = Some((dev, gqf, cqf, c_dedicated, score)),
            _ => {}
        }
    }

    match best {
        Some((dev, gqf, cqf, c_dedicated, _)) => Ok(PickedDevice {
            device: dev,
            graphics_qf: gqf,
            present_qf: gqf,
            compute_qf: cqf,
            compute_dedicated: c_dedicated,
        }),
        None => Err(ContextError::NoSuitableDevice),
    }
}

/// v0.24.3 Ship A (multi-queue prep): result of `pick_physical_device`.
/// Replaces the prior `(PhysicalDevice, u32, u32)` tuple to make room
/// for the compute queue family info.
///
/// `compute_qf` is always populated (every Vulkan-capable GPU has at
/// least one queue family with COMPUTE_BIT). It MAY be the same as
/// `graphics_qf`. The `compute_dedicated` flag indicates whether the
/// compute queue family is genuinely separate from the graphics one.
pub struct PickedDevice {
    pub device: vk::PhysicalDevice,
    pub graphics_qf: u32,
    pub present_qf: u32,
    pub compute_qf: u32,
    pub compute_dedicated: bool,
}

#[cfg(feature = "window")]
fn pick_physical_device(
    instance: &Instance,
    surface_loader: &khr::Surface,
    surface: vk::SurfaceKHR,
) -> Result<PickedDevice> {
    let devices = unsafe { instance.enumerate_physical_devices()? };
    if devices.is_empty() {
        return Err(ContextError::NoSuitableDevice);
    }

    let required_ext = unsafe { CStr::from_bytes_with_nul_unchecked(b"VK_KHR_swapchain\0") };

    // (device, graphics_qf, present_qf, compute_qf, compute_dedicated, score)
    let mut best: Option<(vk::PhysicalDevice, u32, u32, u32, bool, u32)> = None;

    for &dev in &devices {
        // Need swapchain extension.
        let exts = match unsafe { instance.enumerate_device_extension_properties(dev) } {
            Ok(v) => v,
            Err(_) => continue,
        };
        let has_swapchain = exts.iter().any(|e| {
            let name = unsafe { CStr::from_ptr(e.extension_name.as_ptr()) };
            name == required_ext
        });
        if !has_swapchain { continue; }

        // Find a queue family for graphics, and one that supports present.
        // v0.24.3 Ship A (multi-queue prep): also find the best queue family
        // for COMPUTE work that we can use for async mesher dispatches.
        //
        // The async-compute hierarchy from best to worst:
        //   1. Dedicated compute queue family (COMPUTE_BIT but NOT
        //      GRAPHICS_BIT). True separate hardware lane on most
        //      desktop GPUs. Best parallelism.
        //   2. Same family as graphics but a second queue index.
        //      Lets the driver/OS scheduler arbitrate; may serialize on
        //      hardware but still helps for queue submission ordering.
        //   3. Same queue as graphics. No async benefit; fall back to
        //      current behavior. This is fine; mesher submits go on
        //      graphics queue as before.
        //
        // We pick the BEST tier available and let the caller use it.
        let qf_props = unsafe { instance.get_physical_device_queue_family_properties(dev) };
        let mut graphics_qf: Option<u32> = None;
        let mut present_qf: Option<u32> = None;
        let mut compute_qf: Option<u32> = None;
        let mut compute_is_dedicated = false;

        for (i, props) in qf_props.iter().enumerate() {
            let i = i as u32;
            if props.queue_flags.contains(vk::QueueFlags::GRAPHICS) && graphics_qf.is_none() {
                graphics_qf = Some(i);
            }
            let supports_present = unsafe {
                surface_loader.get_physical_device_surface_support(dev, i, surface)
            }.unwrap_or(false);
            if supports_present && present_qf.is_none() {
                present_qf = Some(i);
            }
            // Prefer a queue family that does both graphics + present.
            if props.queue_flags.contains(vk::QueueFlags::GRAPHICS) && supports_present {
                graphics_qf = Some(i);
                present_qf = Some(i);
                // Don't break: we still want to scan the rest for a
                // dedicated compute family.
            }

            // Compute queue family selection. Prefer DEDICATED (compute
            // without graphics) over shared.
            let has_compute  = props.queue_flags.contains(vk::QueueFlags::COMPUTE);
            let has_graphics = props.queue_flags.contains(vk::QueueFlags::GRAPHICS);
            if has_compute && !has_graphics {
                // Dedicated compute family — always prefer this.
                compute_qf = Some(i);
                compute_is_dedicated = true;
            } else if has_compute && compute_qf.is_none() {
                // Shared graphics+compute family. Take it as fallback.
                compute_qf = Some(i);
                compute_is_dedicated = false;
            }
        }

        let (gqf, pqf) = match (graphics_qf, present_qf) {
            (Some(g), Some(p)) => (g, p),
            _ => continue,
        };

        // Compute queue family: must exist. Fall back to graphics_qf
        // if no compute-capable family was identified (every Vulkan
        // device has at least one such family, but defensive).
        let cqf = compute_qf.unwrap_or(gqf);
        let c_dedicated = compute_is_dedicated && cqf != gqf;

        // Score: prefer discrete > integrated > other.
        let props = unsafe { instance.get_physical_device_properties(dev) };
        let score = match props.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU   => 1000,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 500,
            vk::PhysicalDeviceType::VIRTUAL_GPU    => 100,
            vk::PhysicalDeviceType::CPU            => 10,
            _ => 1,
        };

        match best {
            None => best = Some((dev, gqf, pqf, cqf, c_dedicated, score)),
            Some((_, _, _, _, _, prev)) if score > prev =>
                best = Some((dev, gqf, pqf, cqf, c_dedicated, score)),
            _ => {}
        }
    }

    match best {
        Some((dev, gqf, pqf, cqf, c_dedicated, _)) => {
            // Log the compute-queue tier we picked so chug-investigation
            // post-mortems have visibility.
            log::info!(
                "Compute queue family: index={} dedicated={} (graphics_qf={})",
                cqf, c_dedicated, gqf,
            );
            Ok(PickedDevice {
                device: dev,
                graphics_qf: gqf,
                present_qf: pqf,
                compute_qf: cqf,
                compute_dedicated: c_dedicated,
            })
        }
        None => Err(ContextError::NoSuitableDevice),
    }
}

fn create_device(
    instance: &Instance,
    pdev: vk::PhysicalDevice,
    graphics_qf: u32,
    present_qf: u32,
    compute_qf: u32,
    compute_dedicated: bool,
) -> Result<Device> {
    let priorities_one = [1.0f32];
    // For the graphics family, if compute shares it and we need a
    // second queue, request 2 queues with priorities [graphics=1.0,
    // compute=0.5]. The lower compute priority signals the driver that
    // graphics work should preempt compute when both are ready —
    // matches our "render never waits behind mesher" design.
    let priorities_two = [1.0f32, 0.5f32];

    // Track which families we've already requested so we don't
    // duplicate (Vulkan forbids two DeviceQueueCreateInfo entries with
    // the same family_index).
    let mut queue_infos = Vec::with_capacity(3);
    let mut requested_families: Vec<u32> = Vec::new();

    // Determine queue count for the graphics family. If compute shares
    // it AND is "distinct" (non-dedicated but a second queue is
    // available), request 2 queues here. We need to check the device's
    // actual queue_count for this family to know if 2 is supportable.
    let qf_props = unsafe { instance.get_physical_device_queue_family_properties(pdev) };
    let graphics_family_queue_count = qf_props
        .get(graphics_qf as usize)
        .map(|p| p.queue_count)
        .unwrap_or(1);

    let graphics_needs_two_queues =
        compute_qf == graphics_qf
        && !compute_dedicated
        && graphics_family_queue_count >= 2;

    queue_infos.push(
        vk::DeviceQueueCreateInfo::builder()
            .queue_family_index(graphics_qf)
            .queue_priorities(if graphics_needs_two_queues {
                &priorities_two[..]
            } else {
                &priorities_one[..]
            })
            .build()
    );
    requested_families.push(graphics_qf);

    // Add present family if distinct from graphics.
    if present_qf != graphics_qf && !requested_families.contains(&present_qf) {
        queue_infos.push(
            vk::DeviceQueueCreateInfo::builder()
                .queue_family_index(present_qf)
                .queue_priorities(&priorities_one)
                .build()
        );
        requested_families.push(present_qf);
    }

    // Add compute family if distinct from graphics AND present.
    // (Already handled above if compute shares with graphics via 2 queues.)
    if compute_qf != graphics_qf && !requested_families.contains(&compute_qf) {
        queue_infos.push(
            vk::DeviceQueueCreateInfo::builder()
                .queue_family_index(compute_qf)
                .queue_priorities(&priorities_one)
                .build()
        );
        requested_families.push(compute_qf);
    }

    let swapchain_ext = CString::new("VK_KHR_swapchain").unwrap();
    #[cfg(not(target_os = "macos"))]
    let device_exts = vec![swapchain_ext.as_ptr()];
    // r174: MoltenVK devices advertise VK_KHR_portability_subset and the
    // spec REQUIRES enabling it when present.
    #[cfg(target_os = "macos")]
    let device_exts = {
        let mut v = vec![swapchain_ext.as_ptr()];
        v.push(b"VK_KHR_portability_subset\0".as_ptr() as *const std::os::raw::c_char);
        v
    };

    let features = vk::PhysicalDeviceFeatures::default();

    // r687: NEGOTIATE Vulkan 1.1/1.2 feature support instead of blindly
    // enabling. Blindly requesting a feature the device lacks makes
    // `create_device` FAIL — the common macOS/MoltenVK failure, since MoltenVK
    // (Vulkan-over-Metal) doesn't advertise every feature a desktop GPU does.
    // We query what THIS device supports and only flip on the intersection with
    // what we need; anything missing is logged so the operator knows the GPU
    // path may be degraded rather than getting a cryptic device-create error.
    // On Linux/desktop GPUs every feature below is supported, so this is a
    // no-op there.
    let mut sup12 = vk::PhysicalDeviceVulkan12Features::default();
    let mut sup11 = vk::PhysicalDeviceVulkan11Features::default();
    let mut sup2 = vk::PhysicalDeviceFeatures2::builder()
        .push_next(&mut sup12)
        .push_next(&mut sup11)
        .build();
    unsafe { instance.get_physical_device_features2(pdev, &mut sup2) };

    // drawIndirectCount: the core of GPU-driven culling (vkCmdDrawIndexedIndirectCount,
    // promoted KHR→core in 1.2). Recent MoltenVK supports it; older builds don't.
    let want_draw_indirect_count = sup12.draw_indirect_count == vk::TRUE;
    // shaderDrawParameters (1.1): reliable per-draw gl_InstanceIndex in the chunk
    // vertex shader.
    let want_shader_draw_parameters = sup11.shader_draw_parameters == vk::TRUE;
    if !want_draw_indirect_count {
        log::warn!(
            "[gpu] device lacks drawIndirectCount (older MoltenVK / non-1.2 driver?) — \
             GPU-driven indirect culling will be unavailable; update MoltenVK if on macOS"
        );
    }
    if !want_shader_draw_parameters {
        log::warn!(
            "[gpu] device lacks shaderDrawParameters — per-draw instance index may be \
             unreliable on this driver"
        );
    }

    let mut vulkan12 = vk::PhysicalDeviceVulkan12Features::builder()
        .draw_indirect_count(want_draw_indirect_count)
        .build();

    let mut vulkan11 = vk::PhysicalDeviceVulkan11Features::builder()
        .shader_draw_parameters(want_shader_draw_parameters)
        .build();

    let info = vk::DeviceCreateInfo::builder()
        .queue_create_infos(&queue_infos)
        .enabled_extension_names(&device_exts)
        .enabled_features(&features)
        .push_next(&mut vulkan11)
        .push_next(&mut vulkan12);

    Ok(unsafe { instance.create_device(pdev, &info, None)? })
}

#[cfg(all(test, feature = "headless"))]
mod headless_smoke {
    use super::*;
    /// Proves the glfw-free headless Vulkan path actually creates a device.
    /// Run on lavapipe: VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json
    #[test]
    fn creates_headless_device() {
        let ctx = VulkanContext::new_headless()
            .expect("headless Vulkan context should construct (lavapipe/llvmpipe)");
        // Device + queues exist; drop cleans up. Touch a field to be sure.
        let _ = &ctx.device;
    }
}
