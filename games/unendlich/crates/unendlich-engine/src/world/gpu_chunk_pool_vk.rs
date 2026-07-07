//! Vulkan-backed `ChunkPool` implementation.
//!
//! This is a sibling of `gpu_generator::VulkanChunkGenerator`: same Vulkan
//! plumbing pattern (instance/device/queue cached, persistent buffer map,
//! HOST_VISIBLE | HOST_COHERENT memory, `Send + Sync` via internal
//! `submit_lock`), but the buffer is treated as a persistent indexed
//! storage rather than a per-call scratch.
//!
//! # Why this lives separate from `gpu_generator` for v0.18.0
//!
//! The existing generator owns its own buffer that it overwrites on each
//! `generate_batch` call. Migrating it to write into the pool's buffer is
//! a v0.18.0.x sub-step that requires both
//!   (a) the generator's descriptor set to point at the pool's buffer, and
//!   (b) the generator's push-constant `chunk_index` to be an absolute pool
//!       slot rather than a per-batch slot.
//! Both are mechanical, but doing them as a separate increment lets us
//! validate the pool itself in isolation — including on hardware where I
//! can't verify it (Honeykrisp on M2). If the generator integration breaks
//! on a particular driver, we know it's not the pool's fault.

#![cfg(feature = "gpu")]

use crate::constants::CHUNK_VOLUME;
use crate::world::blocks::BlockId;
use crate::world::chunk::ChunkCoord;
use crate::world::gpu_chunk_pool::{ChunkPool, CpuShadowPool, PoolError, PoolSlot, SLOT_BYTES};
use crate::world::gpu_generator::{create_storage_buffer, GpuGenError};
use ash::{vk, Device, Entry, Instance};
use std::convert::TryFrom;
use std::ffi::CStr;
use std::sync::Mutex;

impl From<GpuGenError> for PoolError {
    fn from(e: GpuGenError) -> Self {
        PoolError::Backend(format!("{}", e))
    }
}

impl From<vk::Result> for PoolError {
    fn from(r: vk::Result) -> Self {
        PoolError::Backend(format!("Vulkan error: {:?}", r))
    }
}

/// Vulkan-backed chunk pool. Owns its own SSBO sized to hold `capacity`
/// chunks worth of packed-byte data.
///
/// Slot lifecycle bookkeeping (alloc, free, version checks) is delegated
/// to an internal `CpuShadowPool` — same logic, exhaustively tested
/// without a GPU.
pub struct VulkanChunkPool {
    capacity: u32,

    // Slot bookkeeping reused from the CPU shadow pool. We do NOT use
    // its storage Vec for block data — that would defeat the purpose
    // of having a GPU pool. We only consult it for alloc / free /
    // validate logic. The CPU shadow's storage stays as `None` for
    // every slot.
    bookkeeping: CpuShadowPool,

    // Vulkan handles. Order matters for Drop — destroyed bottom-up.
    block_buffer: vk::Buffer,
    block_memory: vk::DeviceMemory,
    block_ptr: *mut u8,

    submit_lock: Mutex<()>,

    queue: vk::Queue,
    queue_family: u32,
    physical_device: vk::PhysicalDevice,
    device: Device,
    _instance: Instance,
    _entry: Entry,
}

// SAFETY: identical reasoning to VulkanChunkGenerator. `block_ptr` is a
// persistent map into HOST_VISIBLE+HOST_COHERENT memory; all access is
// gated behind `submit_lock` for any code path that could race with the
// GPU. Reads after fence-wait, writes before submit.
unsafe impl Send for VulkanChunkPool {}
unsafe impl Sync for VulkanChunkPool {}

impl VulkanChunkPool {
    /// Create a new pool with the given slot capacity.
    pub fn new(capacity: u32) -> Result<Self, PoolError> {
        assert!(capacity > 0);

        let entry = unsafe { Entry::load() }
            .map_err(|e| PoolError::Backend(format!("Failed to load Vulkan: {}", e)))?;

        let instance = create_instance(&entry)?;
        let physical_device = pick_physical_device(&instance)?;
        let queue_family = find_compute_queue_family(&instance, physical_device)?;
        let device = create_device(&instance, physical_device, queue_family)?;
        let queue = unsafe { device.get_device_queue(queue_family, 0) };

        let block_size = SLOT_BYTES as u64 * capacity as u64;
        let (block_buffer, block_memory) =
            create_storage_buffer(&instance, &device, physical_device, block_size)?;

        let block_ptr = unsafe {
            device.map_memory(block_memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8
        };

        Ok(Self {
            capacity,
            bookkeeping: CpuShadowPool::new(capacity),
            block_buffer,
            block_memory,
            block_ptr,
            submit_lock: Mutex::new(()),
            queue,
            queue_family,
            physical_device,
            device,
            _instance: instance,
            _entry: entry,
        })
    }

    /// Expose the underlying buffer handle so downstream consumers (the
    /// GPU mesher in v0.18.1, the GPU generator after rewiring) can
    /// bind it as their input SSBO. Callers MUST treat slots they do
    /// not own as opaque — concurrent reads are fine, writes require
    /// the slot to be owned by the writer.
    pub fn buffer(&self) -> vk::Buffer { self.block_buffer }

    /// Slot stride in bytes — the offset between consecutive slots in
    /// the underlying buffer. Equal to `SLOT_BYTES`.
    pub fn slot_stride_bytes(&self) -> u64 { SLOT_BYTES as u64 }

    /// Vulkan device the pool was created against. Exposed so consumers
    /// who want to share device/queue with the pool can do so without
    /// creating a second Vulkan device. (Multiple devices on one GPU
    /// is allowed but wasteful.)
    pub fn device(&self) -> &Device { &self.device }
    pub fn queue(&self) -> vk::Queue { self.queue }
    pub fn queue_family(&self) -> u32 { self.queue_family }

    /// Byte offset of a slot within the pool buffer. Validates the slot
    /// handle. Useful for the mesher / generator descriptor updates that
    /// need to address a specific slot.
    pub fn slot_offset_bytes(&self, slot: PoolSlot) -> Result<u64, PoolError> {
        let _ = self.bookkeeping.coord_of(slot)?;
        Ok(slot.index as u64 * SLOT_BYTES as u64)
    }

    /// Find a memory type index suitable for HOST_VISIBLE | HOST_COHERENT
    /// allocations satisfying `type_filter`. Public so that downstream
    /// consumers (e.g. the v0.18.1 mesher) can allocate buffers on the
    /// same device + memory regime as the pool.
    pub fn find_host_visible_memory_type(&self, type_filter: u32) -> Result<u32, PoolError> {
        let props = unsafe {
            self._instance.get_physical_device_memory_properties(self.physical_device)
        };
        for i in 0..props.memory_type_count {
            if type_filter & (1 << i) != 0
                && props.memory_types[i as usize].property_flags.contains(
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )
            {
                return Ok(i);
            }
        }
        Err(PoolError::Backend("No HOST_VISIBLE memory type available".into()))
    }
}

impl ChunkPool for VulkanChunkPool {
    fn capacity(&self) -> u32 { self.capacity }
    fn len(&self) -> u32 { self.bookkeeping.len() }

    fn alloc(&mut self, coord: ChunkCoord, version: u64) -> Result<PoolSlot, PoolError> {
        // Bookkeeping decides which index we get; storage is a fixed
        // offset from that index.
        self.bookkeeping.alloc(coord, version)
    }

    fn upload(&mut self, slot: PoolSlot, blocks: &[BlockId]) -> Result<(), PoolError> {
        if blocks.len() != CHUNK_VOLUME {
            return Err(PoolError::Backend(format!(
                "upload size {} != CHUNK_VOLUME {}", blocks.len(), CHUNK_VOLUME
            )));
        }
        // Validate the handle without touching the bookkeeping's
        // storage Vec (which we keep at None always).
        let _ = self.bookkeeping.coord_of(slot)?;

        // Pack BlockId bytes into u32 words at the slot's offset.
        // SAFETY: the slot's range is bounded by SLOT_BYTES, the
        // backing memory is HOST_VISIBLE | HOST_COHERENT and persistently
        // mapped, and we hold the submit_lock so no GPU command using
        // this slot is in flight.
        let _guard = self.submit_lock.lock().unwrap();
        let offset = slot.index as usize * SLOT_BYTES;
        unsafe {
            let dst = self.block_ptr.add(offset);
            // Safe pack: same layout the shader writes — 4 bytes per u32,
            // little-endian on Vulkan-supported platforms (all of them).
            for (i, &b) in blocks.iter().enumerate() {
                *dst.add(i) = b as u8;
            }
        }
        // HOST_COHERENT memory means no flush call is required; the GPU
        // will see the writes on the next submit.
        Ok(())
    }

    fn free(&mut self, slot: PoolSlot) -> Result<(), PoolError> {
        // Note: callers are responsible for ensuring no in-flight GPU
        // work references this slot. The pool does not track command
        // buffer lifetimes itself.
        //
        // The CPU bookkeeping handles the index-recycling logic.
        self.bookkeeping.free(slot)
    }

    fn read(&self, slot: PoolSlot) -> Result<Vec<BlockId>, PoolError> {
        // Validate the handle.
        let _ = self.bookkeeping.coord_of(slot)?;

        let _guard = self.submit_lock.lock().unwrap();
        let offset = slot.index as usize * SLOT_BYTES;
        let mut out = Vec::with_capacity(CHUNK_VOLUME);
        // SAFETY: persistent map; submit_lock held; offset bounded by slot index.
        unsafe {
            let src = self.block_ptr.add(offset);
            for i in 0..CHUNK_VOLUME {
                let byte = *src.add(i);
                out.push(BlockId::try_from(byte).unwrap_or(BlockId::Air));
            }
        }
        Ok(out)
    }

    fn coord_of(&self, slot: PoolSlot) -> Result<ChunkCoord, PoolError> {
        self.bookkeeping.coord_of(slot)
    }
}

impl Drop for VulkanChunkPool {
    fn drop(&mut self) {
        unsafe {
            // Wait for any in-flight work using our buffer to complete
            // before unmapping. Other consumers (mesher, generator) hold
            // weak references via descriptor sets — they should also be
            // torn down before the pool, but device_wait_idle is the
            // belt-and-suspenders.
            let _ = self.device.device_wait_idle();
            self.device.unmap_memory(self.block_memory);
            self.device.destroy_buffer(self.block_buffer, None);
            self.device.free_memory(self.block_memory, None);
            self.device.destroy_device(None);
            self._instance.destroy_instance(None);
        }
    }
}

// ===== Vulkan setup helpers =============================================
//
// Duplicated from gpu_generator because Vulkan's instance/device/queue
// trio have to be created once per logical Vulkan context, and the pool
// is its own context. These COULD be hoisted into a shared module if
// we end up with three or more Vulkan-using subsystems; for now,
// duplication is cheaper than the refactor.

fn create_instance(entry: &Entry) -> Result<Instance, PoolError> {
    let app_info = vk::ApplicationInfo::builder()
        .application_name(unsafe { CStr::from_bytes_with_nul_unchecked(b"unendlich\0") })
        .application_version(vk::make_api_version(0, 0, 18, 0))
        .engine_name(unsafe { CStr::from_bytes_with_nul_unchecked(b"unendlich\0") })
        .engine_version(vk::make_api_version(0, 0, 18, 0))
        .api_version(vk::make_api_version(0, 1, 2, 0));
    let info = vk::InstanceCreateInfo::builder().application_info(&app_info);
    Ok(unsafe { entry.create_instance(&info, None)? })
}

fn pick_physical_device(instance: &Instance) -> Result<vk::PhysicalDevice, PoolError> {
    let devs = unsafe { instance.enumerate_physical_devices()? };
    if devs.is_empty() {
        return Err(PoolError::Backend("No Vulkan physical devices found".into()));
    }
    let mut best = devs[0];
    let mut best_score = 0u32;
    for &dev in &devs {
        let props = unsafe { instance.get_physical_device_properties(dev) };
        let score = match props.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => 1000,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 500,
            vk::PhysicalDeviceType::VIRTUAL_GPU => 100,
            vk::PhysicalDeviceType::CPU => 10,
            _ => 1,
        };
        if score > best_score {
            best_score = score;
            best = dev;
        }
    }
    Ok(best)
}

fn find_compute_queue_family(instance: &Instance, dev: vk::PhysicalDevice) -> Result<u32, PoolError> {
    let families = unsafe { instance.get_physical_device_queue_family_properties(dev) };
    families.iter().enumerate()
        .find(|(_, p)| p.queue_flags.contains(vk::QueueFlags::COMPUTE))
        .map(|(i, _)| i as u32)
        .ok_or_else(|| PoolError::Backend("No compute queue family found".into()))
}

fn create_device(instance: &Instance, dev: vk::PhysicalDevice, queue_family: u32) -> Result<Device, PoolError> {
    let priorities = [1.0f32];
    let queue_info = vk::DeviceQueueCreateInfo::builder()
        .queue_family_index(queue_family)
        .queue_priorities(&priorities);

    // No special device features required (FP32-only pipeline, see
    // gpu_generator.rs for the rationale).
    let features = vk::PhysicalDeviceFeatures::default();

    let info = vk::DeviceCreateInfo::builder()
        .queue_create_infos(std::slice::from_ref(&queue_info))
        .enabled_features(&features);

    Ok(unsafe { instance.create_device(dev, &info, None)? })
}

#[cfg(test)]
mod tests {
    // The Vulkan-backed pool can only be exercised on a host with a
    // working Vulkan loader. The headless CI sandbox has none, so
    // these tests are gated by an env-var opt-in.
    //
    // To run on hardware:
    //   UNENDLICH_RUN_VK_TESTS=1 cargo test --features gpu vulkan_chunk_pool
    //
    // The CpuShadowPool tests in `gpu_chunk_pool` cover the lifecycle
    // logic; this gate just verifies the Vulkan layer is wired up.

    use super::*;

    fn vk_tests_enabled() -> bool {
        std::env::var("UNENDLICH_RUN_VK_TESTS").ok().as_deref() == Some("1")
    }

    fn cc(x: i32, y: i32, z: i32) -> ChunkCoord { ChunkCoord { x, y, z } }

    #[test]
    fn vulkan_pool_alloc_upload_read_roundtrip() {
        if !vk_tests_enabled() {
            eprintln!("skipping vulkan_pool_alloc_upload_read_roundtrip — set UNENDLICH_RUN_VK_TESTS=1");
            return;
        }
        let mut pool = VulkanChunkPool::new(4)
            .expect("VulkanChunkPool::new failed — is a Vulkan ICD installed?");
        assert_eq!(pool.capacity(), 4);

        let s = pool.alloc(cc(1, 2, 3), 42).unwrap();
        let mut data = vec![BlockId::Air; CHUNK_VOLUME];
        data[0] = BlockId::Stone;
        data[100] = BlockId::Dirt;
        data[CHUNK_VOLUME - 1] = BlockId::Stone;

        pool.upload(s, &data).unwrap();
        let got = pool.read(s).unwrap();
        assert_eq!(got, data);
        assert_eq!(pool.coord_of(s).unwrap(), cc(1, 2, 3));
        assert_eq!(pool.slot_offset_bytes(s).unwrap(), 0);

        pool.free(s).unwrap();
        assert!(matches!(pool.coord_of(s), Err(PoolError::InvalidSlot { .. })));
    }

    #[test]
    fn vulkan_pool_offsets_are_distinct() {
        if !vk_tests_enabled() {
            eprintln!("skipping vulkan_pool_offsets_are_distinct — set UNENDLICH_RUN_VK_TESTS=1");
            return;
        }
        let mut pool = VulkanChunkPool::new(4).unwrap();
        let a = pool.alloc(cc(0, 0, 0), 0).unwrap();
        let b = pool.alloc(cc(1, 0, 0), 0).unwrap();
        let off_a = pool.slot_offset_bytes(a).unwrap();
        let off_b = pool.slot_offset_bytes(b).unwrap();
        assert_ne!(off_a, off_b);
        assert_eq!(off_b - off_a, SLOT_BYTES as u64);
    }
}
