//! zerocopy.rs — SCOPED EXPERIMENT: import mmap'd GGUF pages as GPU memory instead of uploading them.
//!
//! The upload path allocates GPU buffers and copies 40–70 GiB into them, which overruns the unified-memory
//! driver's buffer-object budget (the "Failed to allocate BO VMA" crash on the 70B). On Apple silicon
//! (Asahi/Honeykrisp) host and device memory are the SAME physical RAM, so the copy — and the separate
//! allocation — are pure waste. This module imports the mmap'd file's pages directly as `VkDeviceMemory`
//! via `VK_EXT_external_memory_host` and hands the resulting `VkBuffer` back up to wgpu, so a normal
//! `wgpu::Buffer` ends up being a *view into the file's page cache*: no upload, no per-weight allocation.
//!
//! The wgpu→ash→wgpu swing: `Device::as_hal::<Vulkan>()` drops to the raw `ash::Device` to do the one thing
//! wgpu won't (import a host pointer as device memory), then `Buffer::from_raw` + `create_buffer_from_hal`
//! lift the buffer back so the rest of the engine binds it like any other.
//!
//! DELIBERATELY NOT wired into the live loader — the original upload path is untouched. This is a
//! self-contained probe + import you build with `--features zerocopy` and run on the M2 to answer the one
//! question the whole approach hinges on: does Honeykrisp actually support external-memory-host import?
//!
//! HARD REQUIREMENT: the wgpu device must have been created with `VK_EXT_external_memory_host` ENABLED.
//! wgpu's `request_device` does NOT enable it, so create_device_with_import() shows the hal path that
//! does. Draft until run on hardware — the ash/wgpu-hal calls are structurally correct but unverified on a
//! real Vulkan device here.

// ── pure planner (always compiled, tested) — decides WHEN import is worth it ──────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportDecision {
    /// Upload normally — the weights fit the GPU buffer-upload budget (the original path handles it).
    Upload,
    /// Import the mmap'd file in place — too big to upload as buffers, but fits in RAM (UMA reads it there).
    Import,
    /// Won't fit even mapped into RAM.
    TooBig,
}

/// Choose the residency path. `file_bytes` is the mmap'd GGUF size (imported in place), `gpu_upload_budget`
/// the buffer-allocation ceiling the upload path can safely hit, `ram_available` free system RAM. Import
/// wins exactly in the gap the upload path can't serve but UMA can: bigger than the GPU budget, ≤ RAM.
pub fn should_import(file_bytes: u64, gpu_upload_budget: u64, ram_available: u64) -> ImportDecision {
    if file_bytes <= gpu_upload_budget { ImportDecision::Upload }
    else if file_bytes <= ram_available { ImportDecision::Import }
    else { ImportDecision::TooBig }
}

/// Round `n` up to a multiple of `align` — the imported size must be a whole number of the driver's
/// `minImportedHostPointerAlignment`, and the mmap base must already be aligned to it (page-aligned, so it is).
pub fn aligned_import_size(file_bytes: u64, min_align: u64) -> u64 {
    let a = min_align.max(1);
    n_div_ceil(file_bytes, a) * a
}
fn n_div_ceil(n: u64, d: u64) -> u64 { (n + d - 1) / d }

/// Whether a host pointer is legal to import: aligned to `min_align` and the region a whole multiple of it.
pub fn import_pointer_ok(ptr_addr: usize, size: u64, min_align: u64) -> bool {
    min_align != 0 && (ptr_addr as u64) % min_align == 0 && size % min_align == 0
}

// ── unsafe import (gated) — the actual wgpu→ash→wgpu swing ─────────────────────────────────────────────────
#[cfg(feature = "zerocopy")]
pub mod imp {
    use ash::vk;

    /// What the probe learned about the device's import support.
    #[derive(Clone, Copy, Debug)]
    pub struct ImportSupport {
        pub available: bool,          // VK_EXT_external_memory_host usable on this device
        pub min_align: u64,           // minImportedHostPointerAlignment
    }

    /// Owns the imported `VkDeviceMemory` and frees it on drop; the buffers viewing it must be dropped first,
    /// and the mmap kept alive for its whole life (the GPU reads the file's pages in place).
    pub struct ImportGuard {
        device: ash::Device, // a clone of the raw handle (ash::Device is Clone + refcounts nothing — handle only)
        memory: vk::DeviceMemory,
    }
    impl Drop for ImportGuard {
        fn drop(&mut self) {
            unsafe { self.device.free_memory(self.memory, None); }
        }
    }

    /// The result of an import: wgpu buffers (one per requested span) viewing the imported file memory.
    pub struct Imported {
        pub buffers: Vec<wgpu::Buffer>,
        pub _guard: ImportGuard, // frees the VkDeviceMemory after the buffers drop
    }

    /// Whether this adapter's device ADVERTISES `VK_EXT_external_memory_host`. Cheap, safe, no device
    /// created — call this FIRST. On Honeykrisp/Mesa this returns false (the extension isn't there), which
    /// is the clean answer that says "pivot to streaming" instead of panicking on a request for a missing
    /// extension.
    ///
    /// # Safety
    /// `adapter` must be a Vulkan-backed wgpu adapter.
    pub unsafe fn import_available(adapter: &wgpu::Adapter) -> bool {
        adapter.as_hal::<wgpu::hal::api::Vulkan>()
            .map(|hal| hal.physical_device_capabilities().supports_extension(ash::ext::external_memory_host::NAME))
            .unwrap_or(false)
    }

    /// Create a wgpu Device+Queue with `VK_EXT_external_memory_host` ENABLED — which `request_device` won't
    /// do, and without which [`import_region`] faults. Drops to the hal adapter, opens the device with the
    /// extension appended via the create-device callback, and lifts the result back to wgpu. Use this in place
    /// of `adapter.request_device(...)` when you intend to import weights.
    ///
    /// Returns a clean error (not a panic) when the extension isn't advertised — check [`import_available`]
    /// first for the fast path.
    ///
    /// # Safety
    /// `adapter` must be a Vulkan-backed wgpu adapter.
    pub unsafe fn create_device_with_import(
        adapter: &wgpu::Adapter,
        desc: &wgpu::DeviceDescriptor<'_>,
    ) -> Result<(wgpu::Device, wgpu::Queue), String> {
        if !import_available(adapter) {
            return Err("VK_EXT_external_memory_host is not supported on this device (Honeykrisp/Mesa does not advertise it) — zero-copy import is unavailable; use q4-resident or layer streaming".into());
        }
        // scope the hal adapter borrow so it's dropped before create_device_from_hal reborrows `adapter`
        let open = {
            let hal_adapter = adapter
                .as_hal::<wgpu::hal::api::Vulkan>()
                .ok_or("not a Vulkan adapter")?;
            hal_adapter
                .open_with_callback(
                    desc.required_features,
                    &desc.required_limits,
                    &desc.memory_hints,
                    Some(Box::new(|args: wgpu::hal::vulkan::CreateDeviceCallbackArgs| {
                        // append the extension; the framework keeps its own required set and merges
                        let name = ash::ext::external_memory_host::NAME;
                        if !args.extensions.contains(&name) {
                            args.extensions.push(name);
                        }
                    })),
                )
                .map_err(|e| format!("hal open_with_callback failed: {e:?}"))?
        };
        adapter
            .create_device_from_hal::<wgpu::hal::api::Vulkan>(open, desc)
            .map_err(|e| format!("create_device_from_hal failed: {e:?}"))
    }
    ///
    /// # Safety
    /// `device` must be a live Vulkan-backed wgpu device.
    pub unsafe fn probe(device: &wgpu::Device) -> Option<ImportSupport> {
        let hal = device.as_hal::<wgpu::hal::api::Vulkan>()?;
        let phys = hal.raw_physical_device();
        let instance = hal.shared_instance().raw_instance();
        // query minImportedHostPointerAlignment via VkPhysicalDeviceProperties2
        let mut host_props = vk::PhysicalDeviceExternalMemoryHostPropertiesEXT::default();
        let mut props2 = vk::PhysicalDeviceProperties2::default().push_next(&mut host_props);
        instance.get_physical_device_properties2(phys, &mut props2);
        let min_align = host_props.min_imported_host_pointer_alignment;
        Some(ImportSupport { available: min_align > 0, min_align })
    }

    /// Import a whole mmap'd region as one device memory, then bind each `(offset, size, label)` span as a
    /// wgpu STORAGE buffer viewing it — zero copy, zero per-span allocation. `import_size` must be
    /// `aligned_import_size(region_len, support.min_align)` and the mmap must cover it (page-rounded mmap does).
    ///
    /// # Safety
    /// `host_ptr..+import_size` must stay mapped and readable for as long as any returned buffer lives; the
    /// device must have `VK_EXT_external_memory_host` enabled (see create_device_with_import).
    pub unsafe fn import_region(
        device: &wgpu::Device,
        host_ptr: *const u8,
        import_size: u64,
        spans: &[(u64, u64, String)],
    ) -> Result<Imported, String> {
        let hal = device.as_hal::<wgpu::hal::api::Vulkan>().ok_or("not a Vulkan device")?;
        let raw: &ash::Device = hal.raw_device();
        let phys = hal.raw_physical_device();
        let instance = hal.shared_instance().raw_instance();
        let ext = ash::ext::external_memory_host::Device::new(instance, raw);

        // which memory types accept THIS host pointer? (ash 0.38 exposes only the raw fn pointer)
        let mut host_props = vk::MemoryHostPointerPropertiesEXT::default();
        (ext.fp().get_memory_host_pointer_properties_ext)(
            raw.handle(),
            vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT,
            host_ptr as *const _,
            &mut host_props,
        ).result().map_err(|e| format!("vkGetMemoryHostPointerProperties failed: {e:?}"))?;
        let mem_props = instance.get_physical_device_memory_properties(phys);
        let mem_type = (0..mem_props.memory_type_count)
            .find(|&i| host_props.memory_type_bits & (1 << i) != 0)
            .ok_or("no importable memory type for this host pointer")?;

        // ONE import of the whole region → one VkDeviceMemory the GPU reads in place
        let mut import = vk::ImportMemoryHostPointerInfoEXT::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT)
            .host_pointer(host_ptr as *mut _);
        let alloc = vk::MemoryAllocateInfo::default().allocation_size(import_size).memory_type_index(mem_type).push_next(&mut import);
        let memory = raw.allocate_memory(&alloc, None).map_err(|e| format!("import allocate_memory failed: {e:?} (is VK_EXT_external_memory_host enabled on the device?)"))?;
        let guard = ImportGuard { device: raw.clone(), memory };

        // bind each span as a buffer VIEW into the imported memory, then lift back to wgpu
        let mut buffers = Vec::with_capacity(spans.len());
        for (offset, size, label) in spans {
            let mut ext_ci = vk::ExternalMemoryBufferCreateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT);
            let ci = vk::BufferCreateInfo::default().size(*size)
                .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
                .sharing_mode(vk::SharingMode::EXCLUSIVE)
                .push_next(&mut ext_ci);
            let vkbuf = raw.create_buffer(&ci, None).map_err(|e| format!("create_buffer({label}) failed: {e:?}"))?;
            raw.bind_buffer_memory(vkbuf, memory, *offset).map_err(|e| format!("bind_buffer_memory({label}) failed: {e:?}"))?;
            let hal_buf = wgpu::hal::vulkan::Buffer::from_raw(vkbuf);
            let buf = device.create_buffer_from_hal::<wgpu::hal::api::Vulkan>(hal_buf, &wgpu::BufferDescriptor {
                label: Some(label),
                size: *size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            buffers.push(buf);
        }
        Ok(Imported { buffers, _guard: guard })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_import_only_in_the_gap() {
        let gib = |n: u64| n * (1 << 30);
        // 40 GiB file, 48 GiB upload budget → upload (original path handles it)
        assert_eq!(should_import(gib(40), gib(48), gib(170)), ImportDecision::Upload);
        // 70 GiB file, 48 GiB budget, 170 GiB RAM → import (too big to upload, fits to map)
        assert_eq!(should_import(gib(70), gib(48), gib(170)), ImportDecision::Import);
        // 200 GiB file, 170 GiB RAM → too big even to map
        assert_eq!(should_import(gib(200), gib(48), gib(170)), ImportDecision::TooBig);
        // boundary: exactly the budget → still upload
        assert_eq!(should_import(gib(48), gib(48), gib(170)), ImportDecision::Upload);
    }

    #[test]
    fn import_alignment_rules() {
        assert_eq!(aligned_import_size(4097, 4096), 8192);
        assert_eq!(aligned_import_size(8192, 4096), 8192);
        assert_eq!(aligned_import_size(0, 4096), 0);
        // a page-aligned mmap base + page-rounded size is importable
        assert!(import_pointer_ok(0x1_0000_0000, 8192, 4096));
        // a misaligned base is not
        assert!(!import_pointer_ok(0x1_0000_0040, 8192, 4096));
        // a size that isn't a whole multiple is not
        assert!(!import_pointer_ok(0x1_0000_0000, 5000, 4096));
    }
}
