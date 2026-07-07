//! memplan.rs — pure GPU memory upload-strategy planning for the Vulkan backend on unified-memory hardware.
//!
//! On a DISCRETE GPU you stage every upload: allocate a HOST_VISIBLE scratch buffer, `memcpy` into it,
//! `vkCmdCopyBuffer` into a DEVICE_LOCAL buffer, then barrier — two allocations and a queue transfer per
//! resource. On Apple-silicon UMA (Asahi / Honeykrisp) host and device memory are the SAME physical RAM, so
//! that copy moves bytes from a place to itself. The win isn't faster copying — it's NOT copying, by
//! *declaring* the right memory type (or importing the host pointer you already have) instead of writing the
//! staging machinery.
//!
//! Two zero-copy routes, in order of preference:
//!   1. **Import the host pointer** (`VK_EXT_external_memory_host`): chain `VkImportMemoryHostPointerInfoEXT`
//!      into `vkAllocateMemory` and the GPU reads your existing allocation — an mmap'd GGUF's pages, a Rust
//!      `Vec` — with *zero* allocations and *zero* copies. Needs the extension, a pointer aligned to
//!      `minImportedHostPointerAlignment`, a size that's a multiple of it, and a memory type returned by
//!      `vkGetMemoryHostPointerPropertiesEXT`.
//!   2. **Map direct** (plain UMA): allocate one buffer in a `DEVICE_LOCAL | HOST_VISIBLE` type, map it once,
//!      write in place. No staging buffer, no copy command, no transfer queue.
//!
//! This module is the pure DECISION — given the device's advertised memory types, whether it's UMA, and the
//! host allocation's alignment, it picks the route and the memory-type index. The `vkAllocateMemory` /
//! `vkMapMemory` / import calls are the unsafe FFI seam that executes what this plans (behind `llamacpp`).
#![allow(dead_code)]

/// `VkMemoryPropertyFlagBits` values (stable ABI constants).
pub mod prop {
    pub const DEVICE_LOCAL: u32 = 0x1;
    pub const HOST_VISIBLE: u32 = 0x2;
    pub const HOST_COHERENT: u32 = 0x4;
    pub const HOST_CACHED: u32 = 0x8;
}

/// One entry of `VkPhysicalDeviceMemoryProperties::memoryTypes`: its property flags + heap index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemType {
    pub flags: u32,
    pub heap: u32,
}

impl MemType {
    pub fn has(&self, bits: u32) -> bool { self.flags & bits == bits }
}

/// What the host intends to do with a mapped buffer, which decides *which* host-visible type is best:
/// coherent-uncached streams CPU→GPU writes efficiently; cached is far faster for GPU→CPU READBACK (logits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    HostWrite, // CPU writes, GPU reads (weights, activations in) — want HOST_COHERENT, not cached
    HostRead,  // GPU writes, CPU reads (logits/readback) — want HOST_CACHED
}

/// The import parameters queried from the device + host allocation for route 1.
#[derive(Clone, Copy, Debug)]
pub struct HostImport {
    pub ptr_align: usize,           // alignment of the host pointer we want to import
    pub size: usize,                // its size in bytes
    pub min_align: usize,           // VkPhysicalDeviceExternalMemoryHostPropertiesEXT::minImportedHostPointerAlignment
    pub importable_types: u32,      // bitmask of memory-type indices from vkGetMemoryHostPointerPropertiesEXT
}

impl HostImport {
    /// Whether this host pointer is legal to import: aligned to `min_align` and a whole multiple of it.
    pub fn is_valid(&self) -> bool {
        self.min_align != 0 && self.ptr_align % self.min_align == 0 && self.size % self.min_align == 0
    }
    fn type_ok(&self, i: usize) -> bool { i < 32 && self.importable_types & (1 << i) != 0 }
}

/// The planned upload route. Each carries the memory-type index(es) the FFI should allocate from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upload {
    /// Zero copy, zero alloc: `vkAllocateMemory` importing the host pointer into `mem_type`.
    ImportHostPointer { mem_type: u32 },
    /// UMA: one allocation in a device-local + host-visible `mem_type`, mapped and written in place.
    MapDirect { mem_type: u32 },
    /// Discrete fallback: `memcpy` into `staging_type`, copy-buffer into `device_type`.
    Stage { staging_type: u32, device_type: u32 },
}

/// Pick the memory type best matching `required` bits, preferring those that also have all `preferred` bits;
/// among equals, the lowest index (Vulkan orders "more performant" types first). Returns the type index.
pub fn pick_memtype(types: &[MemType], required: u32, preferred: u32) -> Option<u32> {
    let mut best: Option<(u32, bool)> = None; // (index, has_preferred)
    for (i, t) in types.iter().enumerate() {
        if !t.has(required) { continue; }
        let pref = t.has(preferred);
        match best {
            Some((_, true)) => {} // already have a preferred match at a lower index
            _ if pref => best = Some((i as u32, true)),
            None => best = Some((i as u32, false)),
            _ => {}
        }
    }
    best.map(|(i, _)| i)
}

/// The host-visible type to map for a given access pattern: cached for readback, coherent-uncached for writes.
pub fn host_visible_type(types: &[MemType], access: Access) -> Option<u32> {
    match access {
        Access::HostRead => pick_memtype(types, prop::HOST_VISIBLE, prop::HOST_CACHED),
        Access::HostWrite => {
            // want HOST_VISIBLE|HOST_COHERENT but NOT cached; fall back to any host-visible
            for (i, t) in types.iter().enumerate() {
                if t.has(prop::HOST_VISIBLE | prop::HOST_COHERENT) && !t.has(prop::HOST_CACHED) {
                    return Some(i as u32);
                }
            }
            pick_memtype(types, prop::HOST_VISIBLE, prop::HOST_COHERENT)
        }
    }
}

/// Plan how to get `access` data onto the GPU with the least copying the hardware allows.
/// `uma` = the device advertises a single device-local heap that's also host-visible (Apple silicon).
/// `import` = Some when `VK_EXT_external_memory_host` is available and we have a host pointer to try.
pub fn plan_upload(types: &[MemType], uma: bool, access: Access, import: Option<HostImport>) -> Upload {
    // Route 1 — import the host pointer (zero copy, zero alloc), if the extension + alignment + a valid type
    // line up. Best when the source already lives in host memory (mmap'd weights, a Rust buffer).
    if let Some(imp) = import {
        if imp.is_valid() {
            if let Some(mt) = host_visible_type(types, access) {
                if imp.type_ok(mt as usize) {
                    return Upload::ImportHostPointer { mem_type: mt };
                }
                // else: find ANY importable host-visible type
                for (i, t) in types.iter().enumerate() {
                    if t.has(prop::HOST_VISIBLE) && imp.type_ok(i) {
                        return Upload::ImportHostPointer { mem_type: i as u32 };
                    }
                }
            }
        }
    }
    // Route 2 — plain UMA direct map: a device-local + host-visible type means no staging is ever needed.
    if uma {
        if let Some(mt) = pick_memtype(types, prop::DEVICE_LOCAL | prop::HOST_VISIBLE, prop::HOST_COHERENT) {
            return Upload::MapDirect { mem_type: mt };
        }
    }
    // Route 3 — discrete fallback: stage through host-visible, copy into device-local.
    let staging = host_visible_type(types, Access::HostWrite).unwrap_or(0);
    let device = pick_memtype(types, prop::DEVICE_LOCAL, 0).unwrap_or(0);
    Upload::Stage { staging_type: staging, device_type: device }
}

/// Round `n` up to a multiple of `align` (align 0 treated as 1).
pub fn round_up(n: u64, align: u64) -> u64 {
    let a = align.max(1);
    n.div_ceil(a) * a
}

/// A tensor's location inside the (mmap'd) GGUF file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TensorSpan {
    pub offset: u64,
    pub size: u64,
}

/// The device capabilities the weight planner needs — all queried once at init from the physical device.
#[derive(Clone, Debug)]
pub struct DeviceCaps {
    pub uma: bool,
    pub ext_external_memory_host: bool, // VK_EXT_external_memory_host present?
    pub min_import_align: u64,          // VkPhysicalDeviceExternalMemoryHostPropertiesEXT::minImportedHostPointerAlignment
    pub buffer_align: u64,              // alignment a buffer's memory binding needs (from vkGetBufferMemoryRequirements)
    pub mem_types: Vec<MemType>,
    pub importable_types: u32,          // from vkGetMemoryHostPointerPropertiesEXT for the mmap base
}

/// The plan for getting a whole GGUF's weights onto the GPU with the least copying.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeightMapping {
    /// `Some` → import the ENTIRE mmap'd file as one device memory (zero copy); each tensor is then a buffer
    /// bound at its own offset into it. `None` → import unavailable; the caller uploads per-tensor via
    /// [`plan_upload`] (MapDirect on UMA, Stage on discrete).
    pub import: Option<Upload>,
    /// Bytes to import (the file size rounded up to `min_import_align`, which the page-rounded mmap covers).
    pub import_size: u64,
    /// Indices of tensors whose offset isn't a multiple of `buffer_align` and so can't be bound as a view —
    /// those few fall back to a per-tensor copy even under whole-file import. GGUF aligns tensors (default
    /// 32B) so this is normally empty; it's here so a pathological file degrades instead of failing.
    pub unbindable: Vec<usize>,
}

/// Decide how to map a whole mmap'd GGUF's weights. `base_addr` is the mmap base pointer's address, `file_size`
/// the file length, `tensors` their spans. Prefers a single whole-file host-pointer import (zero copy for
/// every weight); otherwise leaves `import: None` for the caller to upload per-tensor.
pub fn plan_weight_mapping(base_addr: usize, file_size: u64, tensors: &[TensorSpan], caps: &DeviceCaps) -> WeightMapping {
    let import_size = round_up(file_size, caps.min_import_align);
    // Whole-file import is possible when the extension is present and the mmap base meets the import
    // alignment (an mmap base is page-aligned, and min_import_align is ≤ a page, so this holds in practice).
    let import = if caps.ext_external_memory_host {
        let hi = HostImport {
            ptr_align: base_addr,
            size: import_size as usize,
            min_align: caps.min_import_align as usize,
            importable_types: caps.importable_types,
        };
        match plan_upload(&caps.mem_types, caps.uma, Access::HostWrite, Some(hi)) {
            u @ Upload::ImportHostPointer { .. } => Some(u),
            _ => None, // planner declined the import (alignment / no importable type) → per-tensor fallback
        }
    } else {
        None
    };
    // Under whole-file import, a tensor is bound as a view at its offset, which must satisfy buffer alignment.
    let unbindable = if import.is_some() {
        let a = caps.buffer_align.max(1);
        tensors.iter().enumerate().filter(|(_, t)| t.offset % a != 0).map(|(i, _)| i).collect()
    } else {
        Vec::new()
    };
    WeightMapping { import, import_size, unbindable }
}


// raw Vulkan FFI that only compiles with `--features vk-upload` on a host with a
// Vulkan SDK/loader (like the target Mac). It EXECUTES an `Upload` the pure
// planner already chose — it makes no policy decisions of its own. Draft until
// built + run on hardware, same class as the llama.cpp FFI body.
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(feature = "vk-upload")]
pub mod exec {
    use super::Upload;
    use ash::vk;
    use std::os::raw::c_void;

    /// A GPU buffer + its backing allocation, the product of executing an [`Upload`].
    pub struct GpuBuffer {
        pub buffer: vk::Buffer,
        pub memory: vk::DeviceMemory,
        pub size: vk::DeviceSize,
        /// Persistent mapping (MapDirect) or the imported host pointer (ImportHostPointer); null for the
        /// device-local target of a staged upload.
        pub mapped: *mut u8,
        pub imported: bool, // true → the host allocation behind `mapped` MUST outlive this buffer
    }

    /// What the executor needs from the initialized device to run any route.
    pub struct UploadCtx<'a> {
        pub device: &'a ash::Device,
        pub queue: vk::Queue,      // a transfer/graphics queue (only used by the staging route)
        pub cmd_pool: vk::CommandPool,
    }

    /// Execute the `plan` the pure planner produced, moving `size` bytes from `host_ptr` into a GPU buffer of
    /// `usage`. For `ImportHostPointer` the caller guarantees `host_ptr` stays valid + mapped for the
    /// returned buffer's whole life (the GPU reads it in place).
    ///
    /// # Safety
    /// `ctx` handles must be valid; `host_ptr` must point to `size` readable bytes; for the import route it
    /// must additionally meet the alignment the planner already validated and remain live until the buffer
    /// is destroyed.
    pub unsafe fn execute_upload(
        ctx: &UploadCtx,
        plan: Upload,
        host_ptr: *const u8,
        size: vk::DeviceSize,
        usage: vk::BufferUsageFlags,
    ) -> GpuBuffer {
        match plan {
            Upload::ImportHostPointer { mem_type } => import_host_pointer(ctx.device, host_ptr, size, usage, mem_type),
            Upload::MapDirect { mem_type } => map_direct(ctx.device, host_ptr, size, usage, mem_type),
            Upload::Stage { staging_type, device_type } => stage(ctx, host_ptr, size, usage, staging_type, device_type),
        }
    }

    /// ROUTE 1 — zero copy, zero alloc: import the host allocation AS device memory. The GPU reads the
    /// mmap'd / Rust-owned bytes directly; there is no upload.
    unsafe fn import_host_pointer(device: &ash::Device, host_ptr: *const u8, size: vk::DeviceSize, usage: vk::BufferUsageFlags, mem_type: u32) -> GpuBuffer {
        // buffer must declare it may be backed by an imported host allocation
        let mut ext = vk::ExternalMemoryBufferCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT);
        let info = vk::BufferCreateInfo::default().size(size).usage(usage).sharing_mode(vk::SharingMode::EXCLUSIVE).push_next(&mut ext);
        let buffer = device.create_buffer(&info, None).expect("create_buffer");
        let req = device.get_buffer_memory_requirements(buffer);
        // chain the host pointer into the allocation — THIS is the "declare instead of copy"
        let mut import = vk::ImportMemoryHostPointerInfoEXT::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT)
            .host_pointer(host_ptr as *mut c_void);
        let alloc = vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(mem_type).push_next(&mut import);
        let memory = device.allocate_memory(&alloc, None).expect("import allocate_memory");
        device.bind_buffer_memory(buffer, memory, 0).expect("bind");
        GpuBuffer { buffer, memory, size, mapped: host_ptr as *mut u8, imported: true }
    }

    /// ROUTE 2 — UMA direct map: one device-local + host-visible allocation, mapped once, written in place.
    /// No staging buffer, no copy command. (Coherent memory needs no flush; a non-coherent type would need a
    /// `flush_mapped_memory_ranges` after the write.)
    unsafe fn map_direct(device: &ash::Device, host_ptr: *const u8, size: vk::DeviceSize, usage: vk::BufferUsageFlags, mem_type: u32) -> GpuBuffer {
        let info = vk::BufferCreateInfo::default().size(size).usage(usage).sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = device.create_buffer(&info, None).expect("create_buffer");
        let req = device.get_buffer_memory_requirements(buffer);
        let alloc = vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(mem_type);
        let memory = device.allocate_memory(&alloc, None).expect("allocate_memory");
        device.bind_buffer_memory(buffer, memory, 0).expect("bind");
        let mapped = device.map_memory(memory, 0, size, vk::MemoryMapFlags::empty()).expect("map") as *mut u8;
        std::ptr::copy_nonoverlapping(host_ptr, mapped, size as usize); // write straight into shared RAM
        GpuBuffer { buffer, memory, size, mapped, imported: false }
    }

    /// ROUTE 3 — discrete fallback: fill a host-visible staging buffer, then `vkCmdCopyBuffer` into a
    /// device-local buffer and wait. Sketched for completeness — on UMA the planner never selects this.
    unsafe fn stage(ctx: &UploadCtx, host_ptr: *const u8, size: vk::DeviceSize, usage: vk::BufferUsageFlags, staging_type: u32, device_type: u32) -> GpuBuffer {
        let dev = ctx.device;
        let mk = |usage: vk::BufferUsageFlags, mt: u32| -> (vk::Buffer, vk::DeviceMemory) {
            let b = dev.create_buffer(&vk::BufferCreateInfo::default().size(size).usage(usage).sharing_mode(vk::SharingMode::EXCLUSIVE), None).unwrap();
            let req = dev.get_buffer_memory_requirements(b);
            let m = dev.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(mt), None).unwrap();
            dev.bind_buffer_memory(b, m, 0).unwrap();
            (b, m)
        };
        let (staging, staging_mem) = mk(vk::BufferUsageFlags::TRANSFER_SRC, staging_type);
        let (buffer, memory) = mk(usage | vk::BufferUsageFlags::TRANSFER_DST, device_type);
        // fill staging
        let p = dev.map_memory(staging_mem, 0, size, vk::MemoryMapFlags::empty()).unwrap() as *mut u8;
        std::ptr::copy_nonoverlapping(host_ptr, p, size as usize);
        dev.unmap_memory(staging_mem);
        // record + submit the copy
        let cb = dev.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(ctx.cmd_pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1)).unwrap()[0];
        dev.begin_command_buffer(cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).unwrap();
        dev.cmd_copy_buffer(cb, staging, buffer, &[vk::BufferCopy::default().size(size)]);
        dev.end_command_buffer(cb).unwrap();
        let fence = dev.create_fence(&vk::FenceCreateInfo::default(), None).unwrap();
        dev.queue_submit(ctx.queue, &[vk::SubmitInfo::default().command_buffers(&[cb])], fence).unwrap();
        dev.wait_for_fences(&[fence], true, u64::MAX).unwrap();
        // cleanup staging (the device-local buffer is what we return)
        dev.destroy_fence(fence, None);
        dev.free_command_buffers(ctx.cmd_pool, &[cb]);
        dev.destroy_buffer(staging, None);
        dev.free_memory(staging_mem, None);
        GpuBuffer { buffer, memory, size, mapped: std::ptr::null_mut(), imported: false }
    }
    /// Import a WHOLE mmap'd GGUF as one device memory and bind each tensor as a buffer VIEW at its offset —
    /// zero copy for every weight from a single allocation. `mmap_ptr` is the file's mmap base (page-aligned;
    /// the caller keeps the mapping alive for the lifetime of every returned buffer). `import_size` is
    /// [`super::WeightMapping::import_size`]. Tensors listed in `unbindable` (offset not buffer-aligned) get a
    /// per-tensor copy instead. Returns one [`GpuBuffer`] per tensor, in order.
    ///
    /// # Safety
    /// `mmap_ptr..+import_size` must stay mapped and readable for as long as any returned buffer lives; the
    /// planner must have produced `Upload::ImportHostPointer` (checked) so the import is known legal.
    pub unsafe fn import_weight_file(
        ctx: &UploadCtx,
        mmap_ptr: *const u8,
        import_size: vk::DeviceSize,
        import_mem_type: u32,
        tensors: &[super::TensorSpan],
        unbindable: &[usize],
        usage: vk::BufferUsageFlags,
    ) -> Vec<GpuBuffer> {
        let dev = ctx.device;
        // ONE import of the whole file → one VkDeviceMemory the GPU reads in place.
        let mut import = vk::ImportMemoryHostPointerInfoEXT::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT)
            .host_pointer(mmap_ptr as *mut c_void);
        let alloc = vk::MemoryAllocateInfo::default().allocation_size(import_size).memory_type_index(import_mem_type).push_next(&mut import);
        let file_mem = dev.allocate_memory(&alloc, None).expect("import gguf mmap");

        tensors.iter().enumerate().map(|(i, t)| {
            if unbindable.contains(&i) {
                // rare: offset not buffer-aligned → copy this one tensor the plain way
                map_direct(dev, mmap_ptr.add(t.offset as usize), t.size, usage, import_mem_type)
            } else {
                // bind a buffer VIEW straight into the imported file memory at the tensor's offset — no copy
                let buffer = dev.create_buffer(&vk::BufferCreateInfo::default().size(t.size).usage(usage).sharing_mode(vk::SharingMode::EXCLUSIVE), None).expect("create tensor buffer");
                dev.bind_buffer_memory(buffer, file_mem, t.offset).expect("bind tensor view");
                GpuBuffer { buffer, memory: file_mem, size: t.size, mapped: mmap_ptr.add(t.offset as usize) as *mut u8, imported: true }
            }
        }).collect()
        // NOTE: all views share `file_mem`; free it ONCE after destroying the buffers (not per-buffer).
    }

    /// The one call a custom backend makes to load a GGUF's weights with the least copying the hardware
    /// allows: run the pure planner, then either import the whole mmap once (zero copy) or upload per-tensor.
    /// `mmap_ptr`/`file_size` describe the mmap'd file (kept alive by the caller); `tensors` its spans;
    /// `caps` the device. Returns a buffer per tensor plus whether the zero-copy import path was taken.
    ///
    /// # Safety
    /// `mmap_ptr..+file_size` must stay mapped for the lifetime of every returned buffer.
    pub unsafe fn load_gguf_weights(
        ctx: &UploadCtx,
        mmap_ptr: *const u8,
        file_size: u64,
        tensors: &[super::TensorSpan],
        caps: &super::DeviceCaps,
        usage: vk::BufferUsageFlags,
    ) -> (Vec<GpuBuffer>, bool) {
        let mapping = super::plan_weight_mapping(mmap_ptr as usize, file_size, tensors, caps);
        match mapping.import {
            Some(Upload::ImportHostPointer { mem_type }) => (
                import_weight_file(ctx, mmap_ptr, mapping.import_size, mem_type, tensors, &mapping.unbindable, usage),
                true, // whole-file zero-copy import
            ),
            _ => {
                // no import: upload each tensor by its own best route (MapDirect on UMA, Stage on discrete)
                let bufs = tensors.iter().map(|t| {
                    let plan = super::plan_upload(&caps.mem_types, caps.uma, super::Access::HostWrite, None);
                    execute_upload(ctx, plan, mmap_ptr.add(t.offset as usize), t.size, usage)
                }).collect();
                (bufs, false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prop::*;

    // Apple-silicon / Asahi UMA: one device-local heap, host-visible types (coherent + a cached variant).
    fn uma_types() -> Vec<MemType> {
        vec![
            MemType { flags: DEVICE_LOCAL | HOST_VISIBLE | HOST_COHERENT, heap: 0 },
            MemType { flags: DEVICE_LOCAL | HOST_VISIBLE | HOST_COHERENT | HOST_CACHED, heap: 0 },
        ]
    }
    // Discrete (NVIDIA-like): disjoint device-local vs host-visible heaps.
    fn discrete_types() -> Vec<MemType> {
        vec![
            MemType { flags: DEVICE_LOCAL, heap: 0 },
            MemType { flags: HOST_VISIBLE | HOST_COHERENT, heap: 1 },
            MemType { flags: HOST_VISIBLE | HOST_COHERENT | HOST_CACHED, heap: 1 },
        ]
    }

    #[test]
    fn uma_maps_direct_no_staging() {
        let plan = plan_upload(&uma_types(), true, Access::HostWrite, None);
        assert_eq!(plan, Upload::MapDirect { mem_type: 0 }, "UMA writes go to the coherent device-local+host-visible type, no copy");
    }

    #[test]
    fn discrete_falls_back_to_staging() {
        let plan = plan_upload(&discrete_types(), false, Access::HostWrite, None);
        assert_eq!(plan, Upload::Stage { staging_type: 1, device_type: 0 });
    }

    #[test]
    fn import_host_pointer_wins_when_valid() {
        // mmap'd GGUF: 4KB-aligned pointer, page-multiple size, extension present, type 0 importable
        let imp = HostImport { ptr_align: 4096, size: 1 << 20, min_align: 4096, importable_types: 0b01 };
        let plan = plan_upload(&uma_types(), true, Access::HostWrite, Some(imp));
        assert_eq!(plan, Upload::ImportHostPointer { mem_type: 0 }, "zero-copy import beats even a direct map");
    }

    #[test]
    fn misaligned_import_is_rejected_and_falls_through() {
        // a pointer not aligned to min_align can't be imported → fall back to UMA direct map
        let imp = HostImport { ptr_align: 64, size: 1000, min_align: 4096, importable_types: 0b11 };
        assert!(!imp.is_valid());
        let plan = plan_upload(&uma_types(), true, Access::HostWrite, Some(imp));
        assert_eq!(plan, Upload::MapDirect { mem_type: 0 }, "invalid import degrades gracefully, still no staging on UMA");
    }

    #[test]
    fn readback_prefers_cached_memory() {
        // logits read back by the CPU want the HOST_CACHED type (index 1), not the uncached one
        assert_eq!(host_visible_type(&uma_types(), Access::HostRead), Some(1));
        // writes want coherent-uncached (index 0)
        assert_eq!(host_visible_type(&uma_types(), Access::HostWrite), Some(0));
    }

    #[test]
    fn import_validity_checks_alignment_and_size() {
        assert!(HostImport { ptr_align: 8192, size: 8192, min_align: 4096, importable_types: 1 }.is_valid());
        assert!(!HostImport { ptr_align: 4096, size: 5000, min_align: 4096, importable_types: 1 }.is_valid()); // size not a multiple
        assert!(!HostImport { ptr_align: 4096, size: 4096, min_align: 0, importable_types: 1 }.is_valid());    // no import support
    }

    fn caps(uma: bool, ext: bool) -> DeviceCaps {
        DeviceCaps {
            uma,
            ext_external_memory_host: ext,
            min_import_align: 4096,
            buffer_align: 64,
            mem_types: if uma { uma_types() } else { discrete_types() },
            importable_types: 0b11,
        }
    }

    #[test]
    fn whole_gguf_imports_once_zero_copy() {
        // a page-aligned mmap base, three 64B-aligned tensors → one import, every tensor bindable
        let tensors = [
            TensorSpan { offset: 0, size: 4096 },
            TensorSpan { offset: 4096, size: 8192 },
            TensorSpan { offset: 12288, size: 2048 },
        ];
        let m = plan_weight_mapping(0x1_0000_0000, 14336, &tensors, &caps(true, true));
        assert!(matches!(m.import, Some(Upload::ImportHostPointer { .. })), "whole file imported zero-copy");
        assert_eq!(m.import_size, 16384, "file size rounded up to the 4096 import alignment");
        assert!(m.unbindable.is_empty(), "all 64B-aligned tensors bind as views");
    }

    #[test]
    fn misaligned_tensor_offset_falls_back_per_tensor() {
        let tensors = [
            TensorSpan { offset: 0, size: 4096 },
            TensorSpan { offset: 4100, size: 100 }, // not a multiple of buffer_align (64) → can't be a view
        ];
        let m = plan_weight_mapping(0x1_0000_0000, 4200, &tensors, &caps(true, true));
        assert!(m.import.is_some());
        assert_eq!(m.unbindable, vec![1], "the misaligned tensor is flagged for a per-tensor copy");
    }

    #[test]
    fn no_extension_means_no_whole_file_import() {
        let tensors = [TensorSpan { offset: 0, size: 4096 }];
        // UMA but the import extension is absent → import None; caller uploads per-tensor (still map-direct)
        let m = plan_weight_mapping(0x1_0000_0000, 4096, &tensors, &caps(true, false));
        assert_eq!(m.import, None);
        assert!(m.unbindable.is_empty());
    }

    #[test]
    fn round_up_rounds_to_alignment() {
        assert_eq!(round_up(0, 4096), 0);
        assert_eq!(round_up(1, 4096), 4096);
        assert_eq!(round_up(4096, 4096), 4096);
        assert_eq!(round_up(4097, 4096), 8192);
    }
}
