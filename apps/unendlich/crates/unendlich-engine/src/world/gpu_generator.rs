//! GPU-accelerated chunk generation, 3D-density / 32³ / WorldModel-aligned.
//!
//! This is the GPU mirror of `cpu_generator::WorldGenerator::generate_chunk`.
//! The compute shader at `assets/shaders/chunk_generation.comp.spv` evaluates
//! density + caves + surface decoration on the GPU; this Rust glue handles
//! Vulkan plumbing (instance/device/pipeline/buffers) and read-back.
//!
//! Key invariants:
//!   * Per-batch dispatch: one workgroup per chunk; up to `batch_size` chunks
//!     processed in a single submit. Submit overhead is amortized across
//!     the batch.
//!   * Persistent memory map: the output buffer is mapped once and held for
//!     the generator's lifetime. This avoids the `ERROR_MEMORY_MAP_FAILED`
//!     pattern we hit on Asahi/M2 with per-call mapping.
//!   * Reusable command buffer + fence: no per-batch allocation churn.
//!   * `Send + Sync` via `submit_lock: Mutex<()>` because Vulkan queue
//!     submission is not thread-safe.

use crate::constants::CHUNK_VOLUME;
use crate::world::blocks::BlockId;
use crate::world::chunk::{Chunk, ChunkCoord};
use ash::{vk, Device, Entry, Instance};
use std::convert::TryFrom;
use std::ffi::CStr;
use std::sync::Mutex;

const ENTRY_NAME: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"main\0") };

/// Push constant struct — must match the GLSL `PushConstants` block exactly.
/// 3 ints (chunk coord) + 1 uint (seed) + 1 uint (chunk index in batch) = 20 bytes.
/// Vulkan push constants are 4-byte-aligned, so pad to 24 = 6×u32.
#[repr(C)]
#[derive(Copy, Clone)]
struct PushConstants {
    chunk_x: i32,
    chunk_y: i32,
    chunk_z: i32,
    seed: u32,
    chunk_index: u32,
    _pad: u32,
}

#[derive(Debug)]
pub enum GpuGenError {
    Vulkan(vk::Result),
    NoDevice,
    NoComputeQueue,
    NoMemoryType,
    ShaderMissing(String),
    Io(std::io::Error),
    Other(String),
}

impl From<vk::Result> for GpuGenError {
    fn from(r: vk::Result) -> Self { GpuGenError::Vulkan(r) }
}

impl From<std::io::Error> for GpuGenError {
    fn from(e: std::io::Error) -> Self { GpuGenError::Io(e) }
}

impl std::fmt::Display for GpuGenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuGenError::Vulkan(r) => write!(f, "Vulkan error: {:?}", r),
            GpuGenError::NoDevice => write!(f, "No suitable Vulkan device found"),
            GpuGenError::NoComputeQueue => write!(f, "No compute queue family found"),
            GpuGenError::NoMemoryType => write!(f, "Suitable memory type not found"),
            GpuGenError::ShaderMissing(p) => write!(f, "Shader file missing: {}", p),
            GpuGenError::Io(e) => write!(f, "I/O error: {}", e),
            GpuGenError::Other(s) => write!(f, "{}", s),
        }
    }
}
impl std::error::Error for GpuGenError {}

pub type Result<T> = std::result::Result<T, GpuGenError>;

#[derive(Debug, Clone)]
pub struct GpuGeneratedChunk {
    pub coord: ChunkCoord,
    pub blocks: Vec<BlockId>,
}

pub struct VulkanChunkGenerator {
    // Order matters for Drop — destroyed bottom-up.
    block_buffer: vk::Buffer,
    block_memory: vk::DeviceMemory,
    block_ptr: *mut u8,

    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    descriptor_set_layout: vk::DescriptorSetLayout,
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,

    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    fence: vk::Fence,
    submit_lock: Mutex<()>,

    queue: vk::Queue,
    device: Device,
    _instance: Instance,
    _entry: Entry,

    batch_size: u32,
}

// SAFETY: `block_ptr` is a persistent map into HOST_VISIBLE+HOST_COHERENT
// memory. Reads happen after fence-wait under `submit_lock`, so no aliasing.
unsafe impl Send for VulkanChunkGenerator {}
unsafe impl Sync for VulkanChunkGenerator {}

impl VulkanChunkGenerator {
    pub fn new(batch_size: u32) -> Result<Self> {
        assert!(batch_size > 0);

        let entry = unsafe { Entry::load() }
            .map_err(|e| GpuGenError::Other(format!("Failed to load Vulkan: {}", e)))?;

        let instance = create_instance(&entry)?;
        let physical_device = pick_physical_device(&instance)?;
        let queue_family = find_compute_queue_family(&instance, physical_device)?;
        let device = create_device(&instance, physical_device, queue_family)?;
        let queue = unsafe { device.get_device_queue(queue_family, 0) };

        let command_pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::builder()
                    .queue_family_index(queue_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?
        };

        let descriptor_set_layout = create_descriptor_set_layout(&device)?;
        let pipeline_layout = create_pipeline_layout(&device, descriptor_set_layout)?;
        let pipeline = create_compute_pipeline(&device, pipeline_layout)?;

        // Block buffer holds packed bytes: 32^3 = 32,768 bytes per chunk,
        // packed into u32 words (8,192 words per chunk).
        let blocks_per_chunk_packed = CHUNK_VOLUME / 4; // 8,192 u32s
        let block_words_total = blocks_per_chunk_packed as u64 * batch_size as u64;
        let block_size = block_words_total * std::mem::size_of::<u32>() as u64;

        let (block_buffer, block_memory) =
            create_storage_buffer(&instance, &device, physical_device, block_size)?;

        let block_ptr = unsafe {
            device.map_memory(block_memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8
        };

        let (descriptor_pool, descriptor_set) =
            allocate_descriptor_set(&device, descriptor_set_layout)?;
        update_descriptor_set(&device, descriptor_set, block_buffer);

        let command_buffer = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::builder()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0]
        };

        let fence = unsafe {
            device.create_fence(&vk::FenceCreateInfo::builder(), None)?
        };

        Ok(Self {
            block_buffer, block_memory, block_ptr,
            descriptor_pool, descriptor_set, descriptor_set_layout,
            pipeline, pipeline_layout,
            command_pool, command_buffer, fence,
            submit_lock: Mutex::new(()),
            queue,
            device,
            _instance: instance,
            _entry: entry,
            batch_size,
        })
    }

    pub fn batch_size(&self) -> u32 { self.batch_size }

    pub fn generate_batch(&self, coords: &[ChunkCoord], seed: u32) -> Result<Vec<GpuGeneratedChunk>> {
        if coords.is_empty() { return Ok(Vec::new()); }
        if coords.len() > self.batch_size as usize {
            return Err(GpuGenError::Other(format!(
                "Batch size {} exceeds generator capacity {}",
                coords.len(), self.batch_size
            )));
        }

        let _guard = self.submit_lock.lock().unwrap();
        let cmd = self.command_buffer;

        unsafe {
            self.device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;

            self.device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::builder()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;

            self.device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline);
            self.device.cmd_bind_descriptor_sets(
                cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline_layout,
                0, std::slice::from_ref(&self.descriptor_set), &[],
            );

            // Zero the output region for this batch first, since the shader
            // uses atomicAnd/atomicOr to write bytes within u32 words. Stale
            // bits would otherwise leak from previous batches.
            //
            // We do this via vkCmdFillBuffer (size in bytes), which zeros
            // the storage buffer atomically before we dispatch.
            let blocks_per_chunk_packed = (CHUNK_VOLUME / 4) as u64;
            let used_words = blocks_per_chunk_packed * coords.len() as u64;
            let used_bytes = used_words * std::mem::size_of::<u32>() as u64;
            self.device.cmd_fill_buffer(cmd, self.block_buffer, 0, used_bytes, 0);

            // Barrier: the fill must complete before the shader reads the buffer
            // (well, the shader writes to it — but the writes might be
            // RMW via atomicAnd, so we still need fill -> compute ordering).
            let fill_barrier = vk::MemoryBarrier::builder()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE);
            self.device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                std::slice::from_ref(&fill_barrier), &[], &[],
            );

            for (slot, &c) in coords.iter().enumerate() {
                let pc = PushConstants {
                    chunk_x: c.x, chunk_y: c.y, chunk_z: c.z,
                    seed,
                    chunk_index: slot as u32,
                    _pad: 0,
                };
                let bytes = std::slice::from_raw_parts(
                    &pc as *const _ as *const u8,
                    std::mem::size_of::<PushConstants>(),
                );
                self.device.cmd_push_constants(
                    cmd, self.pipeline_layout, vk::ShaderStageFlags::COMPUTE, 0, bytes,
                );
                // Workgroup is 8x8x8, chunk is 32x32x32 -> dispatch (1,1,1) since
                // each invocation handles 4x4x4 sub-blocks internally.
                self.device.cmd_dispatch(cmd, 1, 1, 1);
            }

            // Compute writes -> host reads barrier.
            let read_barrier = vk::MemoryBarrier::builder()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                .dst_access_mask(vk::AccessFlags::HOST_READ);
            self.device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                std::slice::from_ref(&read_barrier), &[], &[],
            );

            self.device.end_command_buffer(cmd)?;

            self.device.reset_fences(std::slice::from_ref(&self.fence))?;
            let submit = vk::SubmitInfo::builder().command_buffers(std::slice::from_ref(&cmd));
            self.device.queue_submit(self.queue, std::slice::from_ref(&submit), self.fence)?;
            self.device.wait_for_fences(std::slice::from_ref(&self.fence), true, u64::MAX)?;
        }

        self.read_back(coords)
    }

    /// Convenience: generate a batch and convert to `Chunk` values directly.
    pub fn generate_chunks(&self, coords: &[ChunkCoord], seed: u32) -> Result<Vec<Chunk>> {
        let raw = self.generate_batch(coords, seed)?;
        // Terrain only — structures are stamped by TerrainGenerator as a CPU post-pass.
        Ok(raw.into_iter().map(|g| Chunk::from_blocks(g.coord, g.blocks)).collect())
    }

    fn read_back(&self, coords: &[ChunkCoord]) -> Result<Vec<GpuGeneratedChunk>> {
        let blocks_per_chunk_packed = CHUNK_VOLUME / 4;
        let total_packed_words = blocks_per_chunk_packed * coords.len();

        // SAFETY: persistent map. The fence wait above ensures the GPU has
        // finished. HOST_COHERENT memory means no explicit invalidate call.
        let block_data: &[u32] = unsafe {
            std::slice::from_raw_parts(self.block_ptr as *const u32, total_packed_words)
        };

        let mut out = Vec::with_capacity(coords.len());

        for (slot, &c) in coords.iter().enumerate() {
            let word_start = slot * blocks_per_chunk_packed;
            let chunk_words = &block_data[word_start..word_start + blocks_per_chunk_packed];

            let mut blocks = Vec::with_capacity(CHUNK_VOLUME);
            for &word in chunk_words {
                for i in 0..4 {
                    let byte = ((word >> (i * 8)) & 0xFF) as u8;
                    blocks.push(BlockId::try_from(byte).unwrap_or(BlockId::Air));
                }
            }
            debug_assert_eq!(blocks.len(), CHUNK_VOLUME);

            out.push(GpuGeneratedChunk { coord: c, blocks });
        }

        Ok(out)
    }
}

impl Drop for VulkanChunkGenerator {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.free_command_buffers(self.command_pool, &[self.command_buffer]);
            self.device.destroy_fence(self.fence, None);

            self.device.unmap_memory(self.block_memory);
            self.device.destroy_buffer(self.block_buffer, None);
            self.device.free_memory(self.block_memory, None);

            self.device.destroy_descriptor_pool(self.descriptor_pool, None);
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.pipeline_layout, None);
            self.device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            self._instance.destroy_instance(None);
        }
    }
}

// ===== Vulkan setup helpers ============================================

fn create_instance(entry: &Entry) -> Result<Instance> {
    let app_info = vk::ApplicationInfo::builder()
        .application_name(unsafe { CStr::from_bytes_with_nul_unchecked(b"Meinkraft\0") })
        .application_version(vk::make_api_version(0, 0, 2, 0))
        .engine_name(unsafe { CStr::from_bytes_with_nul_unchecked(b"Meinkraft\0") })
        .engine_version(vk::make_api_version(0, 0, 2, 0))
        .api_version(vk::make_api_version(0, 1, 2, 0));
    let info = vk::InstanceCreateInfo::builder().application_info(&app_info);
    Ok(unsafe { entry.create_instance(&info, None)? })
}

fn pick_physical_device(instance: &Instance) -> Result<vk::PhysicalDevice> {
    let devs = unsafe { instance.enumerate_physical_devices()? };
    if devs.is_empty() { return Err(GpuGenError::NoDevice); }
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

fn find_compute_queue_family(instance: &Instance, dev: vk::PhysicalDevice) -> Result<u32> {
    let families = unsafe { instance.get_physical_device_queue_family_properties(dev) };
    families.iter().enumerate()
        .find(|(_, p)| p.queue_flags.contains(vk::QueueFlags::COMPUTE))
        .map(|(i, _)| i as u32)
        .ok_or(GpuGenError::NoComputeQueue)
}

fn create_device(instance: &Instance, dev: vk::PhysicalDevice, queue_family: u32) -> Result<Device> {
    let priorities = [1.0f32];
    let queue_info = vk::DeviceQueueCreateInfo::builder()
        .queue_family_index(queue_family)
        .queue_priorities(&priorities);

    // No special device features required. The compute shader is FP32-only
    // (was FP64 pre-v0.18.0.1, ported to FP32 to support drivers without
    // shaderFloat64 — notably Honeykrisp on Apple GPUs and most mobile
    // drivers, which is the entire target hardware list except discrete
    // workstation GPUs).
    let features = vk::PhysicalDeviceFeatures::default();

    let info = vk::DeviceCreateInfo::builder()
        .queue_create_infos(std::slice::from_ref(&queue_info))
        .enabled_features(&features);

    Ok(unsafe { instance.create_device(dev, &info, None)? })
}

fn create_descriptor_set_layout(device: &Device) -> Result<vk::DescriptorSetLayout> {
    // Only one binding now: the block buffer.
    let bindings = [vk::DescriptorSetLayoutBinding::builder()
        .binding(0)
        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .build()];

    let info = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&bindings);
    Ok(unsafe { device.create_descriptor_set_layout(&info, None)? })
}

fn create_pipeline_layout(device: &Device, set_layout: vk::DescriptorSetLayout) -> Result<vk::PipelineLayout> {
    let pcr = vk::PushConstantRange::builder()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0)
        .size(std::mem::size_of::<PushConstants>() as u32);

    let info = vk::PipelineLayoutCreateInfo::builder()
        .set_layouts(std::slice::from_ref(&set_layout))
        .push_constant_ranges(std::slice::from_ref(&pcr));

    Ok(unsafe { device.create_pipeline_layout(&info, None)? })
}

fn create_compute_pipeline(device: &Device, layout: vk::PipelineLayout) -> Result<vk::Pipeline> {
    let candidates = [
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/shaders/chunk_generation.comp.spv"),
        "assets/shaders/chunk_generation.comp.spv",
        "src/shaders/chunk_generation.comp.spv",
    ];
    let bytes = candidates
        .iter()
        .find_map(|p| std::fs::read(p).ok())
        .ok_or_else(|| GpuGenError::ShaderMissing(candidates[1].to_string()))?;

    let words: Vec<u32> = bytes.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    let module_info = vk::ShaderModuleCreateInfo::builder().code(&words);
    let module = unsafe { device.create_shader_module(&module_info, None)? };

    let stage = vk::PipelineShaderStageCreateInfo::builder()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module)
        .name(ENTRY_NAME);

    let info = vk::ComputePipelineCreateInfo::builder().stage(*stage).layout(layout);
    let pipelines = unsafe {
        device.create_compute_pipelines(vk::PipelineCache::null(), std::slice::from_ref(&info), None)
            .map_err(|(_, e)| e)?
    };
    unsafe { device.destroy_shader_module(module, None); }
    Ok(pipelines[0])
}

fn allocate_descriptor_set(device: &Device, layout: vk::DescriptorSetLayout)
    -> Result<(vk::DescriptorPool, vk::DescriptorSet)>
{
    let sizes = [vk::DescriptorPoolSize {
        ty: vk::DescriptorType::STORAGE_BUFFER,
        descriptor_count: 1,
    }];
    let pool_info = vk::DescriptorPoolCreateInfo::builder()
        .pool_sizes(&sizes)
        .max_sets(1);
    let pool = unsafe { device.create_descriptor_pool(&pool_info, None)? };
    let alloc_info = vk::DescriptorSetAllocateInfo::builder()
        .descriptor_pool(pool)
        .set_layouts(std::slice::from_ref(&layout));
    let sets = unsafe { device.allocate_descriptor_sets(&alloc_info)? };
    Ok((pool, sets[0]))
}

fn update_descriptor_set(device: &Device, set: vk::DescriptorSet, block_buf: vk::Buffer) {
    let info = vk::DescriptorBufferInfo::builder()
        .buffer(block_buf)
        .range(vk::WHOLE_SIZE)
        .build();

    let write = vk::WriteDescriptorSet::builder()
        .dst_set(set)
        .dst_binding(0)
        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
        .buffer_info(std::slice::from_ref(&info))
        .build();

    unsafe { device.update_descriptor_sets(std::slice::from_ref(&write), &[]); }
}

pub(crate) fn create_storage_buffer(
    instance: &Instance,
    device: &Device,
    physical_device: vk::PhysicalDevice,
    size: vk::DeviceSize,
) -> Result<(vk::Buffer, vk::DeviceMemory)> {
    let info = vk::BufferCreateInfo::builder()
        .size(size)
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);

    let buffer = unsafe { device.create_buffer(&info, None)? };
    let req = unsafe { device.get_buffer_memory_requirements(buffer) };
    let props = unsafe { instance.get_physical_device_memory_properties(physical_device) };

    let mt = find_memory_type(
        req.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        &props,
    )?;

    let alloc = vk::MemoryAllocateInfo::builder()
        .allocation_size(req.size)
        .memory_type_index(mt);

    let memory = unsafe { device.allocate_memory(&alloc, None)? };
    unsafe { device.bind_buffer_memory(buffer, memory, 0)?; }
    Ok((buffer, memory))
}

pub(crate) fn find_memory_type(
    type_filter: u32,
    flags: vk::MemoryPropertyFlags,
    props: &vk::PhysicalDeviceMemoryProperties,
) -> Result<u32> {
    for i in 0..props.memory_type_count {
        if type_filter & (1 << i) != 0
            && props.memory_types[i as usize].property_flags.contains(flags)
        {
            return Ok(i);
        }
    }
    Err(GpuGenError::NoMemoryType)
}
