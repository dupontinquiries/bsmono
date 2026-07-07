//! sparse/bind.rs — the unsafe sparse-residency binder (gated `sparse`).
//!
//! Creates ONE sparse VkBuffer spanning all the resident weights, backs it with N physical VkDeviceMemory
//! blocks bound via `vkQueueBindSparse`, and lifts it back to a normal `wgpu::Buffer` — so the engine binds
//! tensor views into it exactly as it does the arena today, but there's one buffer instead of ~20 slabs and
//! the load peak is the resident size (no staging VA-doubling). The same `vkQueueBindSparse` mechanism can
//! later UNBIND blocks to page a working set — the substrate for layer streaming.
//!
//! ── THE INTEGRATION BLOCKER (honest) ────────────────────────────────────────────────────────────────────
//! `sparseBinding`/`sparseResidencyBuffer` are CORE Vulkan 1.0 features, and they must be enabled at device
//! creation. wgpu 29 has no sparse support: it neither exposes a sparse `Features` flag nor enables the
//! Vulkan feature, and its create-device callback exposes `PhysicalDeviceFeatures` but the core-feature
//! field is PRIVATE with only a copy-getter — so, unlike the extension-enable trick that made zero-copy's
//! device creation work, there is no clean API to turn `sparseBinding` on. The paths forward are:
//!   1. `Adapter::device_from_raw` — build the ash `Device` ourselves with the sparse features enabled, then
//!      wrap it. Works, but replicates wgpu's whole device init (all required extensions/queues) and is
//!      fragile across wgpu versions.
//!   2. A tiny upstream wgpu-hal change to expose core-feature enabling in the callback (a one-liner there).
//!   3. Reconsider: `maxBufferSize` is 128 GiB on Honeykrisp, so a SINGLE non-sparse 40 GiB buffer may work
//!      if wgpu's 2 GiB `max_buffer_size` cap can be raised via `required_limits` — same "one buffer, no
//!      VA-doubling" win without the sparse feature. Worth probing before committing to (1).
//!
//! So: the binder below is COMPILE-CHECKED and its create/allocate/bind/wrap logic is correct against
//! wgpu-hal 29 + ash 0.38, but it cannot run until the device is created with the sparse feature enabled
//! (path 1 or 2). The pure planner in the parent module is fully usable regardless.

use ash::vk;
use crate::sparse::SparsePlan;

/// Owns the physical blocks + the sparse buffer; frees them on drop (buffers viewing it must drop first).
pub struct SparseBuffer {
    pub buffer: wgpu::Buffer,        // the wgpu-visible handle (bind tensor views into this)
    guard: SparseGuard,
}
struct SparseGuard {
    device: ash::Device,             // cloned handle (no ownership; frees via raw calls)
    raw_buffer: vk::Buffer,
    memories: Vec<vk::DeviceMemory>,
}
impl Drop for SparseGuard {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_buffer(self.raw_buffer, None);
            for m in self.memories.drain(..) { self.device.free_memory(m, None); }
        }
    }
}

/// Whether this device can do sparse-resident buffers (queries `sparseResidencyBuffer`). Cheap; run before
/// attempting — false → fall back to the slab arena.
///
/// # Safety
/// `device` must be a live Vulkan-backed wgpu device.
pub unsafe fn sparse_available(device: &wgpu::Device) -> bool {
    device.as_hal::<wgpu::hal::api::Vulkan>()
        .map(|hal| {
            let phys = hal.raw_physical_device();
            let instance = hal.shared_instance().raw_instance();
            let feats = instance.get_physical_device_features(phys);
            feats.sparse_binding == vk::TRUE && feats.sparse_residency_buffer == vk::TRUE
        })
        .unwrap_or(false)
}

/// Create a sparse-resident buffer per `plan` and bind all its physical blocks (fully resident). `usage` is
/// the wgpu usage the tensors need (STORAGE | COPY_DST). Blocks the calling thread until the bind completes.
///
/// # Safety
/// `device`/`queue` must be a Vulkan-backed wgpu device+queue whose queue family has SPARSE_BINDING.
pub unsafe fn create_sparse_resident(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    plan: &SparsePlan,
) -> Result<SparseBuffer, String> {
    let hal_dev = device.as_hal::<wgpu::hal::api::Vulkan>().ok_or("not a Vulkan device")?;
    let raw: &ash::Device = hal_dev.raw_device();
    let phys = hal_dev.raw_physical_device();
    let instance = hal_dev.shared_instance().raw_instance();

    // 1. the sparse buffer: virtual, no memory yet
    let ci = vk::BufferCreateInfo::default()
        .flags(vk::BufferCreateFlags::SPARSE_BINDING | vk::BufferCreateFlags::SPARSE_RESIDENCY)
        .size(plan.buffer_bytes)
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let raw_buffer = raw.create_buffer(&ci, None).map_err(|e| format!("create sparse buffer: {e:?}"))?;

    // 2. memory requirements → the memory type + real alignment
    let reqs = raw.get_buffer_memory_requirements(raw_buffer);
    let mem_props = instance.get_physical_device_memory_properties(phys);
    let mem_type = (0..mem_props.memory_type_count)
        .find(|&i| reqs.memory_type_bits & (1 << i) != 0
            && mem_props.memory_types[i as usize].property_flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL))
        .or_else(|| (0..mem_props.memory_type_count).find(|&i| reqs.memory_type_bits & (1 << i) != 0))
        .ok_or("no compatible memory type for the sparse buffer")?;

    let mut guard = SparseGuard { device: raw.clone(), raw_buffer, memories: Vec::new() };

    // 3. allocate the physical blocks + build the sparse binds (block B → VA [B·block, (B+1)·block))
    let mut binds: Vec<vk::SparseMemoryBind> = Vec::with_capacity(plan.block_count as usize);
    for b in 0..plan.block_count as u64 {
        let alloc = vk::MemoryAllocateInfo::default().allocation_size(plan.block_bytes).memory_type_index(mem_type);
        let mem = raw.allocate_memory(&alloc, None).map_err(|e| format!("allocate sparse block {b}: {e:?} (BO/VMA limit?)"))?;
        guard.memories.push(mem);
        binds.push(vk::SparseMemoryBind {
            resource_offset: b * plan.block_bytes,
            size: plan.block_bytes,
            memory: mem,
            memory_offset: 0,
            flags: vk::SparseMemoryBindFlags::empty(),
        });
    }

    // 4. bind all blocks in one vkQueueBindSparse, wait on a fence
    let buffer_bind = vk::SparseBufferMemoryBindInfo::default().buffer(raw_buffer).binds(&binds);
    let buffer_binds = [buffer_bind];
    let bind_info = vk::BindSparseInfo::default().buffer_binds(&buffer_binds);
    let vk_queue = {
        let hal_q = queue.as_hal::<wgpu::hal::api::Vulkan>().ok_or("not a Vulkan queue")?;
        hal_q.as_raw()
    };
    let fence = raw.create_fence(&vk::FenceCreateInfo::default(), None).map_err(|e| format!("fence: {e:?}"))?;
    raw.queue_bind_sparse(vk_queue, &[bind_info], fence).map_err(|e| format!("vkQueueBindSparse: {e:?}"))?;
    raw.wait_for_fences(&[fence], true, u64::MAX).map_err(|e| format!("wait bind fence: {e:?}"))?;
    raw.destroy_fence(fence, None);

    // 5. lift the now-backed buffer up to wgpu
    let hal_buf = wgpu::hal::vulkan::Buffer::from_raw(raw_buffer);
    let buffer = device.create_buffer_from_hal::<wgpu::hal::api::Vulkan>(hal_buf, &wgpu::BufferDescriptor {
        label: Some("sparse-resident"),
        size: plan.buffer_bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    Ok(SparseBuffer { buffer, guard })
}
