//! Octree probe pipeline (v0.24.26-octree-on-gpu).
//!
//! A compute pipeline that reads cells from the GPU-resident octree
//! and writes the results to a host-visible buffer so the CPU can
//! verify the upload format end-to-end.
//!
//! ## Why this exists
//!
//! `octree_buffer.rs` puts the octree on the GPU but no production
//! consumer reads it yet — the mesher still eats `SurfaceRecord[]`
//! and the renderer reads vertex meshes. Without a consumer, "did
//! the upload work" is unanswerable: the bytes might be there, or
//! corrupt, or laid out wrong, and nothing would catch it.
//!
//! The probe is that missing read path. It:
//! 1. Takes a list of `(base_node, x, y, z)` queries (CPU-supplied).
//! 2. Dispatches `octree_probe.comp` which calls `octree_sample` from
//!    `octree_sample.glsl` to walk the octree on the GPU.
//! 3. Writes each query's resulting `block_id` to a host-visible
//!    result buffer.
//! 4. Returns the results to the caller, which compares against the
//!    CPU `OctreeArray::sample` to assert the GPU and CPU readers
//!    agree.
//!
//! It is the falsification gate for `OctreeBufferPool`. If a future
//! shader change to `octree_sample.glsl` drifts from the Rust
//! `OctreeNode` layout, this probe will catch it the first time
//! `UNENDLICH_OCTREE_VERIFY=1` runs.
//!
//! ## Lifecycle
//!
//! Constructed once, alongside `OctreeBufferPool`. `run_probe()` is
//! invoked synchronously by the binary on shutdown (when verification
//! is enabled). Each call:
//! 1. Writes queries to the host-visible query buffer.
//! 2. Records a one-shot command buffer with a clean barrier.
//! 3. Submits, waits on a fence (blocks the CPU thread).
//! 4. Reads results from the host-visible result buffer.
//!
//! The probe never runs inside the renderer's per-frame command stream,
//! so it doesn't need to participate in MAX_FRAMES_IN_FLIGHT pipelining
//! or descriptor versioning.
//!
//! ## Capacity
//!
//! MAX_PROBE_QUERIES = 16384. One chunk has 32768 cells (32³); 16K
//! queries is enough to sample dozens of chunks heavily, or one chunk
//! sparsely-but-completely-by-random-sampling. Caller chunks larger
//! workloads into multiple `run_probe` calls.

#![cfg(feature = "render-core")]

use ash::{vk, Device, Instance};
use std::ffi::CStr;
use std::path::Path;
use std::sync::Mutex;

const ENTRY_NAME: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"main\0") };

/// Workgroup size in `octree_probe.comp`. One invocation per query.
/// Dispatch `ceil(query_count / 64)` workgroups.
const WORKGROUP_SIZE: u32 = 64;

/// Maximum queries per `run_probe` call. See module docs for sizing.
pub const MAX_PROBE_QUERIES: u32 = 16_384;

/// One probe query. std430-friendly: 16 bytes, naturally aligned.
///
/// `base_node` is the chunk's base index in the global octree node
/// table (from [`super::octree_buffer::OctreeSlot::base_node_index`]).
/// `x`, `y`, `z` are chunk-local coordinates in `[0, 32)`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct ProbeQuery {
    pub base_node: u32,
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

const _: [(); 16] = [(); std::mem::size_of::<ProbeQuery>()];

/// Push constants for `octree_probe.comp`.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct ProbePushConstants {
    query_count: u32,
}

const _: [(); 4] = [(); std::mem::size_of::<ProbePushConstants>()];

#[derive(Debug)]
pub enum ProbeError {
    Vulkan(vk::Result),
    ShaderMissing(String),
    Io(std::io::Error),
    Backend(String),
    TooManyQueries { wanted: usize, max: u32 },
}
impl From<vk::Result> for ProbeError { fn from(r: vk::Result) -> Self { Self::Vulkan(r) } }
impl From<std::io::Error> for ProbeError { fn from(e: std::io::Error) -> Self { Self::Io(e) } }
impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vulkan(r) => write!(f, "Vulkan error: {:?}", r),
            Self::ShaderMissing(p) => write!(f, "Shader missing: {}", p),
            Self::Io(e) => write!(f, "I/O error: {}", e),
            Self::Backend(s) => write!(f, "{}", s),
            Self::TooManyQueries { wanted, max } =>
                write!(f, "probe: {} queries exceeds MAX_PROBE_QUERIES={}", wanted, max),
        }
    }
}
impl std::error::Error for ProbeError {}

/// Compute pipeline + descriptor wiring + host-visible IO buffers for
/// the octree probe.
pub struct OctreeProbePipeline {
    pub descriptor_set_layout: vk::DescriptorSetLayout,
    pub pipeline_layout: vk::PipelineLayout,
    pub pipeline: vk::Pipeline,
    pub descriptor_pool: vk::DescriptorPool,
    pub descriptor_set: vk::DescriptorSet,

    query_buffer: vk::Buffer,
    query_memory: vk::DeviceMemory,
    query_ptr: *mut u8,

    result_buffer: vk::Buffer,
    result_memory: vk::DeviceMemory,
    result_ptr: *mut u8,

    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    fence: vk::Fence,

    queue: vk::Queue,
    /// Held over `queue_submit` to serialize with the renderer's
    /// per-frame submits on the same queue. Pattern stolen from
    /// `gpu_meshing_sparse::SparseGpuMesher::queue_submit_lock` —
    /// same reason: the M2 Honeykrisp driver has a single queue and
    /// concurrent submits from different threads have produced
    /// surprising failures historically.
    queue_submit_lock: Mutex<()>,

    device: Device,
}

// SAFETY: same justification as ViewComputePipeline. Vulkan handles
// are u64 IDs; the raw pointers index host-coherent memory we own.
unsafe impl Send for OctreeProbePipeline {}

impl OctreeProbePipeline {
    /// Build the pipeline and bind it to the octree storage buffer.
    /// `queue_family` is the family for the `queue` — typically the
    /// engine's graphics queue family (compute is the same family on
    /// M2 / Honeykrisp).
    pub unsafe fn new(
        device: &Device,
        instance: &Instance,
        physical_device: vk::PhysicalDevice,
        octree_buffer: vk::Buffer,
        queue: vk::Queue,
        queue_family: u32,
    ) -> Result<Self, ProbeError> {
        let descriptor_set_layout = create_descriptor_set_layout(device)?;
        let pipeline_layout = create_pipeline_layout(device, descriptor_set_layout)?;
        let pipeline = create_compute_pipeline(device, pipeline_layout)?;
        let (descriptor_pool, descriptor_set) =
            allocate_descriptor_set(device, descriptor_set_layout)?;

        // IO buffers. Both host-visible|coherent so CPU writes/reads
        // are zero-copy.
        let query_bytes = (MAX_PROBE_QUERIES as u64) * std::mem::size_of::<ProbeQuery>() as u64;
        let (query_buffer, query_memory, query_ptr) = create_host_visible_storage(
            device, instance, physical_device, query_bytes,
        )?;
        let result_bytes = (MAX_PROBE_QUERIES as u64) * std::mem::size_of::<u32>() as u64;
        let (result_buffer, result_memory, result_ptr) = create_host_visible_storage(
            device, instance, physical_device, result_bytes,
        )?;

        update_descriptor_set(
            device, descriptor_set,
            octree_buffer, query_buffer, result_buffer,
        );

        // One-shot command pool + a primary command buffer reused
        // across probe calls. `RESET_COMMAND_BUFFER` lets us reset
        // without recreating the pool.
        let pool_info = vk::CommandPoolCreateInfo::builder()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(queue_family);
        let command_pool = device.create_command_pool(&pool_info, None)?;
        let alloc_info = vk::CommandBufferAllocateInfo::builder()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let command_buffer = device.allocate_command_buffers(&alloc_info)?[0];

        let fence_info = vk::FenceCreateInfo::builder();
        let fence = device.create_fence(&fence_info, None)?;

        Ok(Self {
            descriptor_set_layout,
            pipeline_layout,
            pipeline,
            descriptor_pool,
            descriptor_set,
            query_buffer, query_memory, query_ptr,
            result_buffer, result_memory, result_ptr,
            command_pool,
            command_buffer,
            fence,
            queue,
            queue_submit_lock: Mutex::new(()),
            device: device.clone(),
        })
    }

    /// Run a synchronous probe: write `queries` to GPU, dispatch the
    /// shader, wait for completion, read back the block_id results.
    ///
    /// Caller must ensure that the octree buffer's relevant slots are
    /// not being written by another path between the call and return.
    /// In v0.24.26 the renderer doesn't upload octrees from threads
    /// other than the main one, and the binary calls this from the
    /// main thread on shutdown, so the constraint is trivially met.
    ///
    /// On success, returns a `Vec<u32>` of the same length as
    /// `queries`, each entry being the `block_id` (raw u8 widened to
    /// u32) read from the octree at the queried position.
    pub unsafe fn run_probe(
        &mut self,
        queries: &[ProbeQuery],
    ) -> Result<Vec<u32>, ProbeError> {
        if queries.len() > MAX_PROBE_QUERIES as usize {
            return Err(ProbeError::TooManyQueries {
                wanted: queries.len(), max: MAX_PROBE_QUERIES,
            });
        }
        if queries.is_empty() {
            return Ok(Vec::new());
        }

        // 1. Write queries into the host-visible query buffer.
        std::ptr::copy_nonoverlapping(
            queries.as_ptr() as *const u8,
            self.query_ptr,
            queries.len() * std::mem::size_of::<ProbeQuery>(),
        );

        // 2. Record one-shot command buffer.
        self.device.reset_command_buffer(
            self.command_buffer,
            vk::CommandBufferResetFlags::empty(),
        )?;
        let begin = vk::CommandBufferBeginInfo::builder()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        self.device.begin_command_buffer(self.command_buffer, &begin)?;

        // Barrier: host writes to query buffer → shader reads.
        let in_barrier = vk::BufferMemoryBarrier::builder()
            .src_access_mask(vk::AccessFlags::HOST_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(self.query_buffer)
            .offset(0).size(vk::WHOLE_SIZE)
            .build();
        self.device.cmd_pipeline_barrier(
            self.command_buffer,
            vk::PipelineStageFlags::HOST,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::DependencyFlags::empty(),
            &[], &[in_barrier], &[],
        );

        self.device.cmd_bind_pipeline(
            self.command_buffer, vk::PipelineBindPoint::COMPUTE, self.pipeline,
        );
        self.device.cmd_bind_descriptor_sets(
            self.command_buffer, vk::PipelineBindPoint::COMPUTE,
            self.pipeline_layout, 0,
            std::slice::from_ref(&self.descriptor_set), &[],
        );
        let push = ProbePushConstants { query_count: queries.len() as u32 };
        let push_bytes = std::slice::from_raw_parts(
            &push as *const _ as *const u8,
            std::mem::size_of::<ProbePushConstants>(),
        );
        self.device.cmd_push_constants(
            self.command_buffer, self.pipeline_layout,
            vk::ShaderStageFlags::COMPUTE, 0, push_bytes,
        );
        let groups = (queries.len() as u32 + WORKGROUP_SIZE - 1) / WORKGROUP_SIZE;
        self.device.cmd_dispatch(self.command_buffer, groups, 1, 1);

        // Barrier: shader writes → host reads on result buffer.
        let out_barrier = vk::BufferMemoryBarrier::builder()
            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
            .dst_access_mask(vk::AccessFlags::HOST_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(self.result_buffer)
            .offset(0).size(vk::WHOLE_SIZE)
            .build();
        self.device.cmd_pipeline_barrier(
            self.command_buffer,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::PipelineStageFlags::HOST,
            vk::DependencyFlags::empty(),
            &[], &[out_barrier], &[],
        );

        self.device.end_command_buffer(self.command_buffer)?;

        // 3. Submit + wait.
        self.device.reset_fences(&[self.fence])?;
        let submit = vk::SubmitInfo::builder()
            .command_buffers(std::slice::from_ref(&self.command_buffer))
            .build();
        {
            let _qg = self.queue_submit_lock.lock()
                .expect("octree probe queue submit lock poisoned");
            self.device.queue_submit(self.queue, &[submit], self.fence)?;
        }
        self.device.wait_for_fences(&[self.fence], true, u64::MAX)?;

        // 4. Readback.
        let mut out = vec![0u32; queries.len()];
        std::ptr::copy_nonoverlapping(
            self.result_ptr as *const u32,
            out.as_mut_ptr(),
            queries.len(),
        );
        Ok(out)
    }

    /// Free Vulkan resources. Caller must ensure no GPU work
    /// references this pipeline's buffers.
    pub unsafe fn destroy(&mut self) {
        let d = &self.device;
        d.destroy_fence(self.fence, None);
        d.free_command_buffers(self.command_pool, &[self.command_buffer]);
        d.destroy_command_pool(self.command_pool, None);

        d.unmap_memory(self.query_memory);
        d.destroy_buffer(self.query_buffer, None);
        d.free_memory(self.query_memory, None);

        d.unmap_memory(self.result_memory);
        d.destroy_buffer(self.result_buffer, None);
        d.free_memory(self.result_memory, None);

        d.destroy_pipeline(self.pipeline, None);
        d.destroy_pipeline_layout(self.pipeline_layout, None);
        d.destroy_descriptor_pool(self.descriptor_pool, None);
        d.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
    }
}

unsafe fn create_descriptor_set_layout(
    device: &Device,
) -> Result<vk::DescriptorSetLayout, ProbeError> {
    let bindings = [
        vk::DescriptorSetLayoutBinding::builder()
            .binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .build(),  // octree nodes
        vk::DescriptorSetLayoutBinding::builder()
            .binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .build(),  // queries
        vk::DescriptorSetLayoutBinding::builder()
            .binding(2)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .build(),  // results
    ];
    let info = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&bindings);
    Ok(device.create_descriptor_set_layout(&info, None)?)
}

unsafe fn create_pipeline_layout(
    device: &Device, set_layout: vk::DescriptorSetLayout,
) -> Result<vk::PipelineLayout, ProbeError> {
    let pcr = vk::PushConstantRange::builder()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0)
        .size(std::mem::size_of::<ProbePushConstants>() as u32);
    let info = vk::PipelineLayoutCreateInfo::builder()
        .set_layouts(std::slice::from_ref(&set_layout))
        .push_constant_ranges(std::slice::from_ref(&pcr));
    Ok(device.create_pipeline_layout(&info, None)?)
}

unsafe fn create_compute_pipeline(
    device: &Device, layout: vk::PipelineLayout,
) -> Result<vk::Pipeline, ProbeError> {
    let candidates = [
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/shaders/octree_probe.comp.spv"),
        "assets/shaders/octree_probe.comp.spv",
        "src/shaders/octree_probe.comp.spv",
    ];
    let mut bytes: Option<Vec<u8>> = None;
    for p in &candidates {
        if Path::new(p).exists() {
            bytes = Some(std::fs::read(p)?);
            break;
        }
    }
    let bytes = bytes.ok_or_else(|| ProbeError::ShaderMissing(candidates[0].to_string()))?;
    let words: Vec<u32> = bytes.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    let module_info = vk::ShaderModuleCreateInfo::builder().code(&words);
    let module = device.create_shader_module(&module_info, None)?;

    let stage = vk::PipelineShaderStageCreateInfo::builder()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module)
        .name(ENTRY_NAME);
    let info = vk::ComputePipelineCreateInfo::builder()
        .stage(stage.build())
        .layout(layout);
    let pipelines = device.create_compute_pipelines(
        vk::PipelineCache::null(), &[info.build()], None,
    ).map_err(|(_, r)| ProbeError::Vulkan(r))?;
    device.destroy_shader_module(module, None);
    Ok(pipelines[0])
}

unsafe fn allocate_descriptor_set(
    device: &Device, layout: vk::DescriptorSetLayout,
) -> Result<(vk::DescriptorPool, vk::DescriptorSet), ProbeError> {
    let sizes = [vk::DescriptorPoolSize {
        ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 3,
    }];
    let info = vk::DescriptorPoolCreateInfo::builder()
        .pool_sizes(&sizes).max_sets(1);
    let pool = device.create_descriptor_pool(&info, None)?;
    let alloc = vk::DescriptorSetAllocateInfo::builder()
        .descriptor_pool(pool)
        .set_layouts(std::slice::from_ref(&layout));
    let sets = device.allocate_descriptor_sets(&alloc)?;
    Ok((pool, sets[0]))
}

unsafe fn update_descriptor_set(
    device: &Device, set: vk::DescriptorSet,
    octree: vk::Buffer, queries: vk::Buffer, results: vk::Buffer,
) {
    let o_info = vk::DescriptorBufferInfo::builder()
        .buffer(octree).range(vk::WHOLE_SIZE).build();
    let q_info = vk::DescriptorBufferInfo::builder()
        .buffer(queries).range(vk::WHOLE_SIZE).build();
    let r_info = vk::DescriptorBufferInfo::builder()
        .buffer(results).range(vk::WHOLE_SIZE).build();
    let writes = [
        vk::WriteDescriptorSet::builder()
            .dst_set(set).dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&o_info)).build(),
        vk::WriteDescriptorSet::builder()
            .dst_set(set).dst_binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&q_info)).build(),
        vk::WriteDescriptorSet::builder()
            .dst_set(set).dst_binding(2)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&r_info)).build(),
    ];
    device.update_descriptor_sets(&writes, &[]);
}

/// Create a HOST_VISIBLE | HOST_COHERENT storage buffer for IO.
/// Mirrors the pattern in `mesh_buffer_pool::create_persistent_buffer`
/// and `octree_buffer::create_octree_buffer`.
unsafe fn create_host_visible_storage(
    device: &Device,
    instance: &Instance,
    physical_device: vk::PhysicalDevice,
    size: u64,
) -> Result<(vk::Buffer, vk::DeviceMemory, *mut u8), ProbeError> {
    let info = vk::BufferCreateInfo::builder()
        .size(size)
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = device.create_buffer(&info, None)?;

    let req = device.get_buffer_memory_requirements(buffer);
    let props = instance.get_physical_device_memory_properties(physical_device);

    let mut chosen = None;
    for i in 0..props.memory_type_count {
        let mt = props.memory_types[i as usize];
        if req.memory_type_bits & (1 << i) != 0
            && mt.property_flags.contains(
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )
        {
            chosen = Some(i);
            break;
        }
    }
    let mt = chosen.ok_or_else(||
        ProbeError::Backend("No HOST_VISIBLE | HOST_COHERENT memory type".into()))?;

    let alloc = vk::MemoryAllocateInfo::builder()
        .allocation_size(req.size).memory_type_index(mt);
    let memory = device.allocate_memory(&alloc, None)?;
    device.bind_buffer_memory(buffer, memory, 0)?;
    let ptr = device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8;
    Ok((buffer, memory, ptr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_query_is_16_bytes() {
        // GLSL std430 sees this as a uvec4. The size pin keeps the
        // CPU and GPU sides in sync.
        assert_eq!(std::mem::size_of::<ProbeQuery>(), 16);
    }

    #[test]
    fn push_constants_are_4_bytes() {
        assert_eq!(std::mem::size_of::<ProbePushConstants>(), 4);
    }
}
