//! A wgpu `Surface` (Vulkan backend) bound directly to a
//! `systemcompkit::wayland_io::LayerSurface`'s raw `WlSurface`, with no
//! winit involved. See this crate's own Cargo.toml description for why
//! Vulkan-via-`raw-window-handle` sidesteps the wayland-egl question
//! entirely rather than answering it.

use std::ptr::NonNull;

use raw_window_handle::{RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle};
use systemcompkit::wayland_io::LayerSurface;
use wayland_client::Proxy;

pub use egui_wgpu::wgpu;

#[derive(Debug)]
pub enum RenderError {
    NoAdapter,
    Device(String),
    Surface(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::NoAdapter => write!(f, "no Vulkan-capable adapter for this Wayland surface"),
            RenderError::Device(e) => write!(f, "wgpu device request failed: {e}"),
            RenderError::Surface(e) => write!(f, "wgpu surface configure failed: {e}"),
        }
    }
}

/// Builds the two raw-window-handle handles `wgpu` needs from a live
/// `LayerSurface` — the `wl_display` pointer from the connection, the
/// `wl_surface` pointer from the surface itself. Both are borrows into
/// `LayerSurface`'s own state; the handles are only valid as long as it is.
fn raw_handles(layer: &LayerSurface) -> (RawDisplayHandle, RawWindowHandle) {
    let display_ptr = layer.connection().backend().display_ptr();
    let surface_ptr = layer.wl_surface().id().as_ptr();

    let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
        NonNull::new(display_ptr as *mut _).expect("a live Connection always has a non-null wl_display"),
    ));
    let window = RawWindowHandle::Wayland(WaylandWindowHandle::new(
        NonNull::new(surface_ptr as *mut _).expect("a live WlSurface always has a non-null object pointer"),
    ));
    (display, window)
}

pub struct VulkanSurface {
    pub instance: wgpu::Instance,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    tier: topbar_core::RenderTier,
}

impl VulkanSurface {
    /// `size` should match the `LayerSurfaceIntent`/`Placement` the caller
    /// already connected `layer` with — this does not read `layer`'s
    /// configured size itself since that may not have arrived yet (a
    /// `configure` event is asynchronous; see `LayerSurface::connect`'s
    /// own doc). A caller resizes via `reconfigure` once the real
    /// `configured_size()` shows up.
    pub fn new(layer: &LayerSurface, size: (u32, u32)) -> Result<VulkanSurface, RenderError> {
        let (display_handle, window_handle) = raw_handles(layer);

        // Explicit VULKAN, not `Backends::all()` picking whatever's
        // available — the whole point of this crate is answering "does
        // Vulkan work here" rather than silently falling back to GL/EGL
        // and never actually exercising the path being asked about.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        // Safety: `display_handle`/`window_handle` point at objects owned
        // by `layer`, which outlives this call; `layer` isn't consumed or
        // moved out from under them for the surface's lifetime because the
        // surface only borrows raw pointers, not `layer` itself — the
        // caller is responsible for keeping `layer` alive at least as long
        // as this `VulkanSurface`, same responsibility any raw-window-handle
        // consumer has.
        let surface = unsafe {
            instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle { raw_display_handle: Some(display_handle), raw_window_handle: window_handle })
                .map_err(|e| RenderError::Surface(e.to_string()))?
        };

        // Hardware first, `PowerPreference::LowPower` (the bar is idle
        // most of the time — no reason to wake a discrete GPU for it).
        // Fall back to `force_fallback_adapter: true` if nothing hardware
        // answers — this is what actually reaches lavapipe on a system
        // that has it, rather than requiring the caller to construct a
        // second `VulkanSurface` attempt by hand.
        let hardware_request = wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        };
        let fallback_request = wgpu::RequestAdapterOptions { force_fallback_adapter: true, ..hardware_request };
        let adapter = match pollster::block_on(instance.request_adapter(&hardware_request)) {
            Ok(adapter) => adapter,
            Err(_) => pollster::block_on(instance.request_adapter(&fallback_request)).map_err(|_| RenderError::NoAdapter)?,
        };
        let tier = classify_adapter(&adapter.get_info());

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .map_err(|e| RenderError::Device(e.to_string()))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.first().copied().ok_or_else(|| RenderError::Surface("adapter reports no surface formats".into()))?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.0.max(1),
            height: size.1.max(1),
            present_mode: caps.present_modes.first().copied().unwrap_or(wgpu::PresentMode::Fifo),
            alpha_mode: caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Opaque),
            view_formats: Vec::new(),
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        Ok(VulkanSurface { instance, device, queue, surface, config, tier })
    }

    /// Which [`topbar_core::RenderTier`] this surface actually landed on —
    /// classified from `wgpu::AdapterInfo::device_type` at `new()` time,
    /// not assumed from having requested `Backends::VULKAN`: a fallback
    /// adapter (lavapipe) is still a Vulkan adapter, just a software one,
    /// which is exactly the distinction `RenderTier` exists to carry
    /// through to `Runtime::quality()`.
    pub fn tier(&self) -> topbar_core::RenderTier {
        self.tier
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// Re-applies `size` — the call site for `Runtime::toggle_expanded`'s
    /// resize once `LayerSurface::configured_size()` actually reports the
    /// new dimensions (see `new`'s own doc on why size isn't read from
    /// `layer` automatically).
    pub fn reconfigure(&mut self, size: (u32, u32)) {
        self.config.width = size.0.max(1);
        self.config.height = size.1.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// Acquires the next frame's texture. wgpu 29 replaced the older
    /// `Result<SurfaceTexture, SurfaceError>` shape with this enum — kept
    /// as-is (not collapsed into a `Result` here) since `Suboptimal` still
    /// carries a texture worth presenting even though it's a variant a
    /// caller might otherwise expect grouped with the outright failures.
    pub fn acquire(&self) -> wgpu::CurrentSurfaceTexture {
        self.surface.get_current_texture()
    }
}

/// Maps wgpu's own hardware/software distinction onto the shared
/// `topbar_core::RenderTier` — `ShmOnly` is never produced here; that tier
/// only applies when there's no wgpu adapter answering at all, which is a
/// decision made one level up (the caller falling back to
/// `systemcompkit::wayland_io::paint_argb8888` entirely), not something
/// this function can detect from an `AdapterInfo` it was only called with
/// because SOME adapter did answer.
fn classify_adapter(info: &wgpu::AdapterInfo) -> topbar_core::RenderTier {
    match info.device_type {
        wgpu::DeviceType::Cpu => topbar_core::RenderTier::SoftwareEmulated,
        _ => topbar_core::RenderTier::Hardware,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter_info(device_type: wgpu::DeviceType) -> wgpu::AdapterInfo {
        wgpu::AdapterInfo {
            name: "test".into(),
            vendor: 0,
            device: 0,
            device_type,
            device_pci_bus_id: String::new(),
            driver: String::new(),
            driver_info: String::new(),
            backend: wgpu::Backend::Vulkan,
            subgroup_min_size: 0,
            subgroup_max_size: 0,
            transient_saves_memory: false,
        }
    }

    #[test]
    fn cpu_device_type_classifies_as_software_emulated() {
        assert_eq!(classify_adapter(&adapter_info(wgpu::DeviceType::Cpu)), topbar_core::RenderTier::SoftwareEmulated);
    }

    #[test]
    fn every_real_gpu_device_type_classifies_as_hardware() {
        for dt in [wgpu::DeviceType::DiscreteGpu, wgpu::DeviceType::IntegratedGpu, wgpu::DeviceType::VirtualGpu, wgpu::DeviceType::Other] {
            assert_eq!(classify_adapter(&adapter_info(dt)), topbar_core::RenderTier::Hardware);
        }
    }
}
