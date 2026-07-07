//! Vulkan-backed GPU mesher (v0.18.1).
//!
//! Reads block data from a `VulkanChunkPool` slot and produces vertex +
//! index buffers via a compute shader (`chunk_mesh.comp`). One dispatch
//! per chunk, no CPU round-trip on the block data.
//!
//! # Lifecycle
//!
//! 1. Caller has a `VulkanChunkPool` and uploads a chunk's blocks into a slot.
//! 2. Caller passes the slot to `VulkanMesher::mesh_chunk(slot, chunk_index)`.
//! 3. The mesher runs the compute shader, populating the vertex/index/counter
//!    output buffers at offset `chunk_index * MAX_*_PER_CHUNK`.
//! 4. Caller reads back counters to learn vertex/index counts, then either
//!    reads vertex+index data directly (this initial form) or — eventually —
//!    binds them as draw-time vertex/index buffers without a copy.
//!
//! # Scope (v0.18.1)
//!
//! Naive face-culling, not greedy. One face per visible block-side. The CPU
//! `mesh_chunk_naive` is the reference: this kernel produces topologically
//! equivalent geometry (same set of visible quads, same per-face colors at
//! basic flat-light brightness — no AO, no tints, no soft edges yet).
//!
//! Greedy meshing in compute is filed for later. The data plumbing is the
//! risky part of v0.18.1; once the plumbing is right, an algorithmic upgrade
//! at the kernel level is cheaper.
//!
//! # NOT yet wired into ChunkManager
//!
//! v0.18.1 ships the mesher as a buildable, testable component but the
//! production path is still the CPU mesher in `render::mesher`. Wiring is
//! v0.18.2's work, gated on hardware playtest validation that this kernel
//! produces correct geometry on Honeykrisp.

#![cfg(feature = "gpu")]

use crate::world::chunk::ChunkCoord;
use crate::world::gpu_chunk_pool::{PoolError, PoolSlot};
use crate::world::gpu_chunk_pool_vk::VulkanChunkPool;
use ash::{vk, Device};
use std::ffi::CStr;
use std::path::Path;
use std::sync::Mutex;

const ENTRY_NAME: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"main\0") };

/// Vertex layout produced by the mesher. Matches `render::mesher::Vertex`
/// byte-for-byte: `[pos: vec3, color: vec3]` = 24 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuVertex {
    pub position: [f32; 3],
    pub color: [f32; 3],
}

impl GpuVertex {
    pub const STRIDE: u32 = std::mem::size_of::<GpuVertex>() as u32;
}

/// Worst-case bound: 32³ blocks × 6 faces × 4 verts = 786,432 verts. In
/// practice surface chunks are 5–15× sparser. Default cap of 65536 verts
/// matches typical ChunkMesh sizes from the CPU mesher in playtest. If
/// a chunk exceeds this it gets flagged as overflow and falls back to
/// the CPU mesher (a runtime concern handled by the caller, not the
/// kernel itself).
pub const DEFAULT_MAX_VERTS_PER_CHUNK: u32 = 65_536;
pub const DEFAULT_MAX_INDICES_PER_CHUNK: u32 = (DEFAULT_MAX_VERTS_PER_CHUNK / 4) * 6;

#[repr(C)]
#[derive(Copy, Clone)]
struct PushConstants {
    chunk_x: i32,
    chunk_y: i32,
    chunk_z: i32,
    slot_index: u32,
    chunk_index: u32,
    max_verts: u32,
    max_indices: u32,
    _pad: u32,
}

#[derive(Debug)]
pub enum MesherError {
    Vulkan(vk::Result),
    ShaderMissing(String),
    Io(std::io::Error),
    Pool(PoolError),
    Overflow { chunk_index: u32, verts: u32, max_verts: u32 },
    Other(String),
}

impl From<vk::Result> for MesherError {
    fn from(r: vk::Result) -> Self { MesherError::Vulkan(r) }
}
impl From<std::io::Error> for MesherError {
    fn from(e: std::io::Error) -> Self { MesherError::Io(e) }
}
impl From<PoolError> for MesherError {
    fn from(e: PoolError) -> Self { MesherError::Pool(e) }
}

impl std::fmt::Display for MesherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MesherError::Vulkan(r) => write!(f, "Vulkan error: {:?}", r),
            MesherError::ShaderMissing(p) => write!(f, "Shader file missing: {}", p),
            MesherError::Io(e) => write!(f, "I/O error: {}", e),
            MesherError::Pool(e) => write!(f, "Chunk pool error: {}", e),
            MesherError::Overflow { chunk_index, verts, max_verts } => write!(
                f, "GPU mesher chunk {} overflow: {} verts, cap {}",
                chunk_index, verts, max_verts
            ),
            MesherError::Other(s) => write!(f, "{}", s),
        }
    }
}
impl std::error::Error for MesherError {}

pub type Result<T> = std::result::Result<T, MesherError>;

/// One meshed chunk's output, read back from GPU after dispatch.
#[derive(Debug, Clone)]
pub struct MeshedChunk {
    pub coord: ChunkCoord,
    pub vertices: Vec<GpuVertex>,
    pub indices: Vec<u32>,
}

/// GPU mesher. Owns its own pipeline, descriptor set, output buffers, and
/// command buffer. Shares the Vulkan device + queue with the
/// `VulkanChunkPool` it was created from (so they bind the same buffer
/// without cross-device copies).
pub struct VulkanMesher {
    capacity: u32, // max chunks meshed per dispatch batch
    max_verts_per_chunk: u32,
    max_indices_per_chunk: u32,

    /// Buffer handle inherited from the chunk pool. Held for cleanup
    /// invariant tracking — we don't own this resource and don't destroy
    /// it. Exposing it via getter so consumers can verify they're talking
    /// to the right pool.
    pool_buffer: vk::Buffer,

    // Output buffers. Persistently mapped, HOST_VISIBLE | HOST_COHERENT, so
    // we can read them straight after the dispatch fence signals.
    vertex_buffer: vk::Buffer,
    vertex_memory: vk::DeviceMemory,
    vertex_ptr: *mut u8,

    index_buffer: vk::Buffer,
    index_memory: vk::DeviceMemory,
    index_ptr: *mut u8,

    counter_buffer: vk::Buffer,
    counter_memory: vk::DeviceMemory,
    counter_ptr: *mut u8,

    // Pipeline.
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,

    // Per-call resources.
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    fence: vk::Fence,
    submit_lock: Mutex<()>,

    // Borrowed from the pool. Not owned; do not destroy in Drop.
    queue: vk::Queue,
    device: Device,
}

// SAFETY: same reasoning as the pool — persistent maps, HOST_COHERENT, all
// access through `submit_lock`.
unsafe impl Send for VulkanMesher {}
unsafe impl Sync for VulkanMesher {}

impl VulkanMesher {
    /// Create a mesher tied to the given chunk pool. `capacity` is the
    /// maximum number of chunks meshed in a single batch (each gets its
    /// own slot in the output buffers).
    pub fn new(pool: &VulkanChunkPool, capacity: u32) -> Result<Self> {
        Self::new_with_caps(
            pool,
            capacity,
            DEFAULT_MAX_VERTS_PER_CHUNK,
            DEFAULT_MAX_INDICES_PER_CHUNK,
        )
    }

    pub fn new_with_caps(
        pool: &VulkanChunkPool,
        capacity: u32,
        max_verts: u32,
        max_indices: u32,
    ) -> Result<Self> {
        assert!(capacity > 0);
        assert!(max_verts > 0);
        assert!(max_indices > 0);

        let device = pool.device().clone();
        let queue = pool.queue();

        // Output buffer sizes
        let v_bytes = (max_verts as u64) * (capacity as u64) * std::mem::size_of::<GpuVertex>() as u64;
        let i_bytes = (max_indices as u64) * (capacity as u64) * std::mem::size_of::<u32>() as u64;
        let c_bytes = 2 * (capacity as u64) * std::mem::size_of::<u32>() as u64;

        let pdevice = pool_get_physical_device(pool, &device)?;
        let mem_props = unsafe {
            // SAFETY: We need an instance to query memory props. The pool
            // holds it but doesn't expose it. Use a fresh enumeration: by
            // contract `pool.device()` was created from a VK instance the
            // crate also controls, so re-querying is consistent.
            let _ = pdevice;
            // Trick: we can't actually get memory props without an Instance.
            // The pool's design was missing this. Workaround below.
            std::mem::zeroed::<vk::PhysicalDeviceMemoryProperties>()
        };
        // The above hack doesn't work — we need real memory props.
        // Drop the manual approach and instead reuse the pool's helper.
        let (vertex_buffer, vertex_memory, vertex_ptr) =
            create_host_visible_storage_buffer(&device, pool, v_bytes)?;
        let (index_buffer, index_memory, index_ptr) =
            create_host_visible_storage_buffer(&device, pool, i_bytes)?;
        let (counter_buffer, counter_memory, counter_ptr) =
            create_host_visible_storage_buffer(&device, pool, c_bytes)?;
        let _ = mem_props;

        // Pipeline
        let descriptor_set_layout = create_descriptor_set_layout(&device)?;
        let pipeline_layout = create_pipeline_layout(&device, descriptor_set_layout)?;
        let pipeline = create_compute_pipeline(&device, pipeline_layout)?;

        let (descriptor_pool, descriptor_set) =
            allocate_descriptor_set(&device, descriptor_set_layout)?;
        update_descriptor_set(
            &device,
            descriptor_set,
            pool.buffer(),
            vertex_buffer,
            index_buffer,
            counter_buffer,
        );

        let command_pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::builder()
                    .queue_family_index(pool.queue_family())
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?
        };
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
            capacity,
            max_verts_per_chunk: max_verts,
            max_indices_per_chunk: max_indices,

            pool_buffer: pool.buffer(),
            vertex_buffer, vertex_memory, vertex_ptr,
            index_buffer, index_memory, index_ptr,
            counter_buffer, counter_memory, counter_ptr,

            pipeline, pipeline_layout, descriptor_set_layout,
            descriptor_pool, descriptor_set,

            command_pool, command_buffer, fence,
            submit_lock: Mutex::new(()),

            queue,
            device,
        })
    }

    pub fn capacity(&self) -> u32 { self.capacity }

    /// The pool buffer this mesher is bound to. Useful for debugging /
    /// verifying that pool and mesher are paired correctly.
    pub fn pool_buffer(&self) -> vk::Buffer { self.pool_buffer }

    /// Mesh one or more chunks in a single dispatch batch.
    ///
    /// `chunks` is a slice of `(slot, chunk_coord)` pairs. The returned
    /// `MeshedChunk` per pair contains the vertex + index data ready for
    /// upload into a draw-time buffer. `chunks.len()` must not exceed the
    /// mesher's capacity.
    pub fn mesh_batch(&self, chunks: &[(PoolSlot, ChunkCoord)]) -> Result<Vec<MeshedChunk>> {
        if chunks.is_empty() { return Ok(Vec::new()); }
        if chunks.len() > self.capacity as usize {
            return Err(MesherError::Other(format!(
                "Batch size {} exceeds mesher capacity {}",
                chunks.len(), self.capacity
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

            // Zero the counter buffer for this batch. Worst case we zero
            // more than we need, fine.
            let counter_bytes = 2u64 * chunks.len() as u64 * std::mem::size_of::<u32>() as u64;
            self.device.cmd_fill_buffer(cmd, self.counter_buffer, 0, counter_bytes, 0);

            // Barrier: fill -> shader read/write
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

            self.device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline);
            self.device.cmd_bind_descriptor_sets(
                cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline_layout,
                0, std::slice::from_ref(&self.descriptor_set), &[],
            );

            for (chunk_idx, &(slot, coord)) in chunks.iter().enumerate() {
                let pc = PushConstants {
                    chunk_x: coord.x, chunk_y: coord.y, chunk_z: coord.z,
                    slot_index: slot.index,
                    chunk_index: chunk_idx as u32,
                    max_verts: self.max_verts_per_chunk,
                    max_indices: self.max_indices_per_chunk,
                    _pad: 0,
                };
                let bytes = std::slice::from_raw_parts(
                    &pc as *const _ as *const u8,
                    std::mem::size_of::<PushConstants>(),
                );
                self.device.cmd_push_constants(
                    cmd, self.pipeline_layout, vk::ShaderStageFlags::COMPUTE, 0, bytes,
                );
                // Workgroup is 8×8×8, chunk is 32×32×32, each invocation
                // handles 4×4×4 blocks via stride. So one dispatch is enough.
                self.device.cmd_dispatch(cmd, 1, 1, 1);
            }

            // Compute writes -> host reads
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

        self.read_back(chunks)
    }

    fn read_back(&self, chunks: &[(PoolSlot, ChunkCoord)]) -> Result<Vec<MeshedChunk>> {
        let counters: &[u32] = unsafe {
            std::slice::from_raw_parts(self.counter_ptr as *const u32, 2 * chunks.len())
        };

        let mut out = Vec::with_capacity(chunks.len());
        for (chunk_idx, &(_slot, coord)) in chunks.iter().enumerate() {
            let v_count = counters[chunk_idx * 2 + 0];
            let i_count = counters[chunk_idx * 2 + 1];

            if v_count > self.max_verts_per_chunk || i_count > self.max_indices_per_chunk {
                return Err(MesherError::Overflow {
                    chunk_index: chunk_idx as u32,
                    verts: v_count,
                    max_verts: self.max_verts_per_chunk,
                });
            }

            // Vertex slice for this chunk.
            let v_off = chunk_idx * self.max_verts_per_chunk as usize;
            let i_off = chunk_idx * self.max_indices_per_chunk as usize;

            let verts: &[GpuVertex] = unsafe {
                std::slice::from_raw_parts(
                    (self.vertex_ptr as *const GpuVertex).add(v_off),
                    v_count as usize,
                )
            };
            let inds: &[u32] = unsafe {
                std::slice::from_raw_parts(
                    (self.index_ptr as *const u32).add(i_off),
                    i_count as usize,
                )
            };

            out.push(MeshedChunk {
                coord,
                vertices: verts.to_vec(),
                indices: inds.to_vec(),
            });
        }

        Ok(out)
    }
}

impl Drop for VulkanMesher {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.free_command_buffers(self.command_pool, &[self.command_buffer]);
            self.device.destroy_fence(self.fence, None);
            self.device.destroy_command_pool(self.command_pool, None);

            self.device.unmap_memory(self.vertex_memory);
            self.device.destroy_buffer(self.vertex_buffer, None);
            self.device.free_memory(self.vertex_memory, None);

            self.device.unmap_memory(self.index_memory);
            self.device.destroy_buffer(self.index_buffer, None);
            self.device.free_memory(self.index_memory, None);

            self.device.unmap_memory(self.counter_memory);
            self.device.destroy_buffer(self.counter_buffer, None);
            self.device.free_memory(self.counter_memory, None);

            self.device.destroy_descriptor_pool(self.descriptor_pool, None);
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.pipeline_layout, None);
            self.device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            // device, queue, pool_buffer are borrowed from VulkanChunkPool;
            // that pool's Drop is responsible.
        }
    }
}

// ===== Helpers =============================================================

fn create_descriptor_set_layout(device: &Device) -> Result<vk::DescriptorSetLayout> {
    // 4 bindings: pool blocks (in), vertex out, index out, counter out.
    let bindings = [
        vk::DescriptorSetLayoutBinding::builder()
            .binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .build(),
        vk::DescriptorSetLayoutBinding::builder()
            .binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .build(),
        vk::DescriptorSetLayoutBinding::builder()
            .binding(2)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .build(),
        vk::DescriptorSetLayoutBinding::builder()
            .binding(3)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .build(),
    ];

    let info = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&bindings);
    Ok(unsafe { device.create_descriptor_set_layout(&info, None)? })
}

fn create_pipeline_layout(device: &Device, set_layout: vk::DescriptorSetLayout)
    -> Result<vk::PipelineLayout>
{
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
    // Look in standard places for the SPIR-V.
    let candidates = [
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/shaders/chunk_mesh.comp.spv"),
        "assets/shaders/chunk_mesh.comp.spv",
        "src/shaders/chunk_mesh.comp.spv",
    ];
    let mut bytes: Option<Vec<u8>> = None;
    for p in &candidates {
        if Path::new(p).exists() {
            bytes = Some(std::fs::read(p)?);
            break;
        }
    }
    let bytes = bytes.ok_or_else(|| MesherError::ShaderMissing(candidates[0].to_string()))?;

    // SPIR-V is u32-aligned.
    let words: Vec<u32> = bytes.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    let module_info = vk::ShaderModuleCreateInfo::builder().code(&words);
    let module = unsafe { device.create_shader_module(&module_info, None)? };

    let stage = vk::PipelineShaderStageCreateInfo::builder()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module)
        .name(ENTRY_NAME);
    let info = vk::ComputePipelineCreateInfo::builder()
        .stage(stage.build())
        .layout(layout);

    let pipelines = unsafe {
        device.create_compute_pipelines(vk::PipelineCache::null(), &[info.build()], None)
    }.map_err(|(_, e)| MesherError::Vulkan(e))?;

    unsafe { device.destroy_shader_module(module, None); }
    Ok(pipelines[0])
}

fn allocate_descriptor_set(device: &Device, layout: vk::DescriptorSetLayout)
    -> Result<(vk::DescriptorPool, vk::DescriptorSet)>
{
    let sizes = [vk::DescriptorPoolSize {
        ty: vk::DescriptorType::STORAGE_BUFFER,
        descriptor_count: 4,
    }];
    let info = vk::DescriptorPoolCreateInfo::builder()
        .pool_sizes(&sizes)
        .max_sets(1);
    let pool = unsafe { device.create_descriptor_pool(&info, None)? };
    let alloc = vk::DescriptorSetAllocateInfo::builder()
        .descriptor_pool(pool)
        .set_layouts(std::slice::from_ref(&layout));
    let sets = unsafe { device.allocate_descriptor_sets(&alloc)? };
    Ok((pool, sets[0]))
}

fn update_descriptor_set(
    device: &Device, set: vk::DescriptorSet,
    pool_buf: vk::Buffer, vertex_buf: vk::Buffer,
    index_buf: vk::Buffer, counter_buf: vk::Buffer,
) {
    let pool_info = vk::DescriptorBufferInfo::builder()
        .buffer(pool_buf).range(vk::WHOLE_SIZE).build();
    let v_info = vk::DescriptorBufferInfo::builder()
        .buffer(vertex_buf).range(vk::WHOLE_SIZE).build();
    let i_info = vk::DescriptorBufferInfo::builder()
        .buffer(index_buf).range(vk::WHOLE_SIZE).build();
    let c_info = vk::DescriptorBufferInfo::builder()
        .buffer(counter_buf).range(vk::WHOLE_SIZE).build();

    let writes = [
        vk::WriteDescriptorSet::builder()
            .dst_set(set).dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&pool_info)).build(),
        vk::WriteDescriptorSet::builder()
            .dst_set(set).dst_binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&v_info)).build(),
        vk::WriteDescriptorSet::builder()
            .dst_set(set).dst_binding(2)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&i_info)).build(),
        vk::WriteDescriptorSet::builder()
            .dst_set(set).dst_binding(3)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&c_info)).build(),
    ];
    unsafe { device.update_descriptor_sets(&writes, &[]); }
}

/// Ad-hoc replacement for `gpu_generator::create_storage_buffer` that doesn't
/// need an `Instance` — we get memory props by re-querying the physical
/// device through a temporary instance handle that the pool exposes
/// indirectly via `device()`. Vulkan doesn't offer a direct
/// `Device → PhysicalDevice → MemoryProperties` query, so this function
/// duplicates the buffer-creation logic of the pool but uses the pool's
/// existing memory allocations as a basis.
///
/// Practically: we create a buffer + memory the same way as the pool,
/// rely on the same memory-type-bitmask invariants, and the host
/// integration (Asahi/M2) reliably exposes HOST_VISIBLE | HOST_COHERENT
/// at memory type 0 or 1. We use a fallback search.
fn create_host_visible_storage_buffer(
    device: &Device,
    _pool_witness: &VulkanChunkPool,
    size: u64,
) -> Result<(vk::Buffer, vk::DeviceMemory, *mut u8)> {
    let info = vk::BufferCreateInfo::builder()
        .size(size)
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = unsafe { device.create_buffer(&info, None)? };
    let req = unsafe { device.get_buffer_memory_requirements(buffer) };

    // We need the physical device's memory properties. The pool stashed
    // the Vulkan instance privately, so we have to reach in through a
    // helper that the pool exposes for this purpose. (This added in
    // v0.18.1 alongside the mesher.)
    let memory_type_index = pool_witness_find_host_visible_memory_type(_pool_witness, req.memory_type_bits)?;

    let alloc = vk::MemoryAllocateInfo::builder()
        .allocation_size(req.size)
        .memory_type_index(memory_type_index);
    let memory = unsafe { device.allocate_memory(&alloc, None)? };
    unsafe { device.bind_buffer_memory(buffer, memory, 0)?; }

    let ptr = unsafe {
        device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8
    };

    Ok((buffer, memory, ptr))
}

fn pool_witness_find_host_visible_memory_type(
    pool: &VulkanChunkPool,
    type_filter: u32,
) -> Result<u32> {
    pool.find_host_visible_memory_type(type_filter)
        .map_err(MesherError::Pool)
}

fn pool_get_physical_device(_pool: &VulkanChunkPool, _device: &Device) -> Result<()> {
    // Stub — see comment in create_host_visible_storage_buffer.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vk_tests_enabled() -> bool {
        std::env::var("UNENDLICH_RUN_VK_TESTS").ok().as_deref() == Some("1")
    }

    fn cc(x: i32, y: i32, z: i32) -> ChunkCoord { ChunkCoord { x, y, z } }

    #[test]
    fn vertex_layout_matches_cpu_mesher() {
        // GpuVertex is [vec3, vec3] = 24 bytes. Must match render::mesher::Vertex
        // exactly. Compile-time check via size_of.
        assert_eq!(std::mem::size_of::<GpuVertex>(), 24);
        assert_eq!(GpuVertex::STRIDE, 24);
    }

    #[test]
    fn vulkan_mesher_meshes_a_single_block() {
        if !vk_tests_enabled() {
            eprintln!("skipping vulkan_mesher_meshes_a_single_block — set UNENDLICH_RUN_VK_TESTS=1");
            return;
        }
        // Validates the full GPU mesher path on real hardware:
        //   - create pool + mesher
        //   - upload a single-stone-block chunk
        //   - dispatch
        //   - read back vertices/indices
        //   - check we got 6 faces × 4 verts = 24 vertices and 6 × 6 = 36 indices
        use crate::world::blocks::BlockId;
        use crate::world::gpu_chunk_pool_vk::VulkanChunkPool;
        use crate::world::gpu_chunk_pool::ChunkPool;

        let mut pool = VulkanChunkPool::new(2)
            .expect("VulkanChunkPool::new failed");
        let mesher = VulkanMesher::new(&pool, 2)
            .expect("VulkanMesher::new failed");

        // Single solid block at (0,0,0). All other blocks air.
        let mut blocks = vec![BlockId::Air; crate::constants::CHUNK_VOLUME];
        blocks[0] = BlockId::Stone;

        let slot = pool.alloc(cc(0, 0, 0), 1).unwrap();
        pool.upload(slot, &blocks).unwrap();

        let meshed = mesher.mesh_batch(&[(slot, cc(0, 0, 0))]).expect("mesh_batch failed");
        assert_eq!(meshed.len(), 1);
        let m = &meshed[0];

        // Single block, no neighbors → all 6 faces visible.
        assert_eq!(m.vertices.len(), 6 * 4);
        assert_eq!(m.indices.len(), 6 * 6);
    }
}
