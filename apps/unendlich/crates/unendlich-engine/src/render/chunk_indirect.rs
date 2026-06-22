//! Indirect chunk drawing (v0.20).
//!
//! Replaces the per-chunk draw loop in `WorldRenderer` with a pipeline that
//! issues one compute dispatch (frustum cull → emit indirect commands) and
//! one `vkCmdDrawIndexedIndirectCount` call per pass. CPU-side per-chunk
//! work goes from ~6 Vulkan calls × N chunks per frame to ~3 calls total.
//!
//! # Pipeline shape
//!
//! Per frame, per pass (main + shadow):
//!
//!   1. CPU: update chunk-metadata SSBO for any chunks that uploaded or
//!      evicted this frame. Incremental — touches only changed entries.
//!   2. CPU: write frustum + chunk_count into push constants, dispatch
//!      `chunk_cull.comp`. Each invocation reads one chunk's metadata,
//!      tests AABB against frustum, and on visibility appends a
//!      VkDrawIndexedIndirectCommand into the output buffer via atomic
//!      counter.
//!   3. GPU: barrier from compute writes to indirect-command reads.
//!   4. CPU: issue `vkCmdDrawIndexedIndirectCount` with the count buffer
//!      and command buffer. GPU draws all visible chunks in one call.
//!
//! # Capacity
//!
//! `MAX_CHUNK_DRAWS` = 4096. Sized to comfortably exceed the 925-chunks-
//! at-render-distance-6 working set. The metadata SSBO is sparse — slots
//! marked with `index_count = 0` are skipped by the cull shader.
//!
//! # Slot management
//!
//! This module owns its own slot allocator for chunk metadata (separate
//! from the `mesh_buffer_pool`'s span allocator, because metadata slots
//! are fixed-size and the index serves as a stable ID for the chunk's
//! draw entry). When a chunk is uploaded, the renderer:
//!   - allocs a metadata slot (or reuses the chunk's existing slot)
//!   - writes the slot's `ChunkDraw` entry from the mesh's offsets/counts
//! When evicted:
//!   - sets `index_count = 0` (cull shader skips it)
//!   - returns the slot to the free list
//!
//! # NOT in this module
//!
//! - The actual draw call. The renderer makes that call directly using
//!   `cmd_draw_indexed_indirect_count` against this module's buffers.
//! - The mesh data. That stays in `mesh_buffer_pool::VulkanMeshPool`.
//! - The graphics pipeline / vertex shader. Unchanged — works as-is
//!   because vertex/index buffers are bound once per pass with the pool's
//!   buffers and `firstIndex` / `vertexOffset` in each indirect command
//!   handle the per-chunk offsets.

#![cfg(feature = "render-core")]

use ash::{vk, Device, Instance};
use std::collections::VecDeque;
use std::ffi::CStr;
use std::path::Path;

const ENTRY_NAME: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"main\0") };

/// Maximum number of simultaneously-active chunk draws. Sized for
/// render-distance-6 (~925 chunks) with comfortable headroom.
pub const MAX_CHUNK_DRAWS: u32 = 4096;

/// Render-distance-relative default for the draw-slot pool. The fixed 4096 was
/// sized for RD=6 (~925 chunks) and silently overflows at higher RD: an RD=12
/// run had up to ~5416 rendering chunks and logged 3315 "slot pool exhausted"
/// failures, which manifest as chunks that never receive a draw slot and so go
/// unrendered ("incomplete mesh" holes). The full RD sphere holds ~(4/3)π·rd³
/// chunks; we provision that plus 1.5× headroom for LOD-transition churn (a
/// chunk can briefly hold both its old and new slot), clamped to [4096, 65536].
/// Slots are tiny (tens of bytes each), so over-provisioning costs a few
/// hundred KB even at the ceiling — cheap on any device including 6-8GB SoCs.
pub fn default_max_draws(render_distance: i32) -> u32 {
    let rd = render_distance.max(1) as f64;
    let sphere = (4.0 / 3.0) * std::f64::consts::PI * rd * rd * rd;
    let with_headroom = (sphere * 1.5).ceil() as u32;
    with_headroom.clamp(MAX_CHUNK_DRAWS, 65536)
}

/// Runtime override for the draw-slot pool capacity via `UNENDLICH_MAX_DRAWS`
/// (clamped to [256, 65536]); defaults to a render-distance-relative size (see
/// [`default_max_draws`]). Read once at pool creation. Watch the dashboard
/// Memory panel's OOM-fallback chip + octree utilization to tune it (see
/// docs/MEMORY-PRESSURE.md).
pub fn runtime_max_draws() -> u32 {
    let default = default_max_draws(crate::constants::runtime_render_distance());
    match std::env::var("UNENDLICH_MAX_DRAWS") {
        Ok(v) => match v.parse::<u32>() {
            Ok(n) if (256..=65536).contains(&n) => n,
            Ok(n) => {
                log::warn!("UNENDLICH_MAX_DRAWS={} out of [256,65536]; using {}", n, default);
                default
            }
            Err(_) => {
                log::warn!("UNENDLICH_MAX_DRAWS={} not a valid u32; using {}", v, default);
                default
            }
        },
        Err(_) => default,
    }
}

/// Workgroup size in `chunk_cull.comp`. Dispatch ceil(active / WG).
pub const CULL_WORKGROUP_SIZE: u32 = 64;

/// Flag bits in `ChunkDraw::flags`.
pub const FLAG_DEBUG_OVERLAY: u32 = 1;

/// Mirrors `chunk_cull.comp`'s `ChunkDraw`. std430 layout.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct ChunkDraw {
    pub index_count: u32,
    pub first_index: u32,
    /// Per-chunk vertex base in the shared mesh buffer pool, in
    /// vertex units (not bytes). The cull shader propagates this to
    /// the indirect command's `vertexOffset` so the GPU adds it to
    /// each index during fetch. v0.24.20-vertex-pack: was always 0
    /// before; the CPU pre-shifted every index instead.
    pub vertex_offset: i32,
    pub flags: u32,
    /// `xyz` = chunk's world AABB min corner (xyz used by cull
    /// shader for frustum testing).
    /// `w`   = chunk's world-space origin Y, read by the vertex
    /// shader to reconstruct world position from the chunk-local
    /// packed vertex (v0.24.20-vertex-pack). The cull shader does
    /// not read `.w`. `aabb_min.x` and `aabb_min.z` already coincide
    /// with the chunk's XZ origin since the renderer builds them
    /// from `coord.block_origin()`.
    pub aabb_min: [f32; 4],
    /// `xyz` = chunk's world AABB max corner. `w` reserved.
    pub aabb_max: [f32; 4],
}

impl ChunkDraw {
    pub const fn empty() -> Self {
        Self {
            index_count: 0,
            first_index: 0,
            vertex_offset: 0,
            flags: 0,
            aabb_min: [0.0; 4],
            aabb_max: [0.0; 4],
        }
    }
}

/// Mirrors `VkDrawIndexedIndirectCommand`. Used only for size assertion;
/// the GPU shader writes these directly.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct DrawIndexedIndirect {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    vertex_offset: i32,
    first_instance: u32,
}

const _: [(); std::mem::size_of::<DrawIndexedIndirect>()] =
    [(); std::mem::size_of::<vk::DrawIndexedIndirectCommand>()];

/// Push constants for the cull shader. Layout matches `chunk_cull.comp`.
///
/// v0.24.0a: shrunk from 112 bytes (6 planes inline + count + pad) to
/// 8 bytes (view_index + chunk_count). Frustum planes now live in the
/// ViewParams SSBO at binding 3; this push constant just selects which
/// view slot to read.
///
/// v0.24.4 (shadow-stride): extended to 16 bytes with `stride` and
/// `stride_phase`. When stride > 1, the cull shader admits only chunks
/// where `(slot_index % stride) == stride_phase`. Main cull always
/// passes `stride=1, phase=0` (= admit all visible chunks).
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct CullPushConstants {
    /// Which slot in the ViewParams SSBO to read frustum planes from.
    /// v0.24.0a: caller writes the slot's planes before dispatching,
    /// then sets view_index to match. v0.24.0b: GPU `view_compute.comp`
    /// derives the planes; CPU just sets view_index.
    pub view_index: u32,
    pub chunk_count: u32,
    /// Temporal stride: 1 = admit all visible chunks (default), N > 1
    /// = admit only one in N chunks per frame (the chunks where
    /// `slot_index % stride == stride_phase`). Used by shadow cull to
    /// amortize render cost over N frames.
    pub stride: u32,
    /// Which residue class to admit this frame: `0..stride`. Caller
    /// rotates this each frame so all chunks get rendered over N frames.
    pub stride_phase: u32,
}

#[derive(Debug)]
pub enum IndirectError {
    Vulkan(vk::Result),
    ShaderMissing(String),
    Io(std::io::Error),
    Exhausted,
    Other(String),
}
impl From<vk::Result> for IndirectError { fn from(r: vk::Result) -> Self { Self::Vulkan(r) } }
impl From<std::io::Error> for IndirectError { fn from(e: std::io::Error) -> Self { Self::Io(e) } }
impl std::fmt::Display for IndirectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vulkan(r) => write!(f, "Vulkan error: {:?}", r),
            Self::ShaderMissing(p) => write!(f, "Shader missing: {}", p),
            Self::Io(e) => write!(f, "I/O error: {}", e),
            Self::Exhausted => write!(f, "Indirect draw slot pool exhausted"),
            Self::Other(s) => write!(f, "{}", s),
        }
    }
}
impl std::error::Error for IndirectError {}

/// Slot handle into the chunk metadata array. Stable across upload/eviction
/// cycles for the same chunk if the renderer reuses it; the indirect module
/// itself doesn't care, just allocates fresh on demand.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DrawSlot(pub u32);

/// CPU shadow of the GPU metadata array. Owns the SSBO and tracks which
/// slots are in use. Updates flow CPU → GPU via direct memcpy into the
/// persistent map; the cull shader reads what's there at dispatch time.
pub struct IndirectDrawSet {
    // Slots
    capacity: u32,
    cpu_shadow: Vec<ChunkDraw>,
    free_list: VecDeque<u32>,
    /// Highest in-use slot index, +1. Cull shader iterates 0..high_water
    /// to bound work; lowering this when many slots free at once is a
    /// nice-to-have optimization filed for later (currently it never
    /// shrinks during the session).
    high_water: u32,

    // GPU buffers
    metadata_buffer: vk::Buffer,
    metadata_memory: vk::DeviceMemory,
    metadata_ptr: *mut u8,

    pub commands_buffer: vk::Buffer,
    commands_memory: vk::DeviceMemory,

    pub count_buffer: vk::Buffer,
    count_memory: vk::DeviceMemory,

    // Cull pipeline
    pub descriptor_set: vk::DescriptorSet,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set_layout: vk::DescriptorSetLayout,
    pub pipeline_layout: vk::PipelineLayout,
    pub pipeline: vk::Pipeline,

    device: Device,
}

// SAFETY: same reasoning as the mesh pool — persistent map, HOST_COHERENT,
// access serialized through the renderer's frame loop (single-threaded).
unsafe impl Send for IndirectDrawSet {}

impl IndirectDrawSet {
    /// Build the indirect-draw infrastructure. Allocates GPU buffers,
    /// creates the cull pipeline. Caller must call `destroy` before the
    /// device is destroyed.
    ///
    /// `view_params_buffer` is the ViewParams SSBO (binding 3 in the
    /// cull shader). Caller owns its lifetime — must outlive this
    /// IndirectDrawSet. v0.24.0a: caller writes frustum planes into
    /// the buffer before each cull dispatch and sets push.view_index.
    pub unsafe fn new(
        device: &Device,
        instance: &Instance,
        physical_device: vk::PhysicalDevice,
        view_params_buffer: vk::Buffer,
    ) -> Result<Self, IndirectError> {
        let capacity = runtime_max_draws();
        if capacity != MAX_CHUNK_DRAWS {
            log::info!("indirect draw slot pool: capacity {} (UNENDLICH_MAX_DRAWS override)", capacity);
        }

        // Metadata SSBO — HOST_VISIBLE so CPU can poke per-chunk updates.
        let metadata_size =
            (capacity as u64) * std::mem::size_of::<ChunkDraw>() as u64;
        let (metadata_buffer, metadata_memory, metadata_ptr) = create_buffer(
            device, instance, physical_device,
            metadata_size,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true, // host-visible
        )?;

        // Indirect commands — GPU-only (compute writes, draw reads).
        let commands_size =
            (capacity as u64) * std::mem::size_of::<DrawIndexedIndirect>() as u64;
        let (commands_buffer, commands_memory, _) = create_buffer(
            device, instance, physical_device,
            commands_size,
            vk::BufferUsageFlags::STORAGE_BUFFER
                | vk::BufferUsageFlags::INDIRECT_BUFFER
                | vk::BufferUsageFlags::TRANSFER_DST,
            false,
        )?;

        // Count buffer — single u32, GPU writes via atomic, draw reads via
        // indirectCount, CPU clears via cmd_fill_buffer each frame.
        // TRANSFER_DST is required for the per-frame zero-fill.
        let (count_buffer, count_memory, _) = create_buffer(
            device, instance, physical_device,
            std::mem::size_of::<u32>() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER
                | vk::BufferUsageFlags::INDIRECT_BUFFER
                | vk::BufferUsageFlags::TRANSFER_DST,
            false,
        )?;

        // Cull pipeline.
        let descriptor_set_layout = create_descriptor_set_layout(device)?;
        let pipeline_layout = create_pipeline_layout(device, descriptor_set_layout)?;
        let pipeline = create_compute_pipeline(device, pipeline_layout)?;

        let (descriptor_pool, descriptor_set) =
            allocate_descriptor_set(device, descriptor_set_layout)?;
        update_descriptor_set(
            device, descriptor_set,
            metadata_buffer, commands_buffer, count_buffer,
            view_params_buffer,
        );

        // Slot bookkeeping.
        let cpu_shadow = vec![ChunkDraw::empty(); capacity as usize];
        let mut free_list = VecDeque::with_capacity(capacity as usize);
        for i in 0..capacity { free_list.push_back(i); }

        // Initialize GPU metadata buffer to all zero (= all slots inactive).
        std::ptr::write_bytes(metadata_ptr, 0, metadata_size as usize);

        Ok(Self {
            capacity,
            cpu_shadow,
            free_list,
            high_water: 0,
            metadata_buffer, metadata_memory, metadata_ptr,
            commands_buffer, commands_memory,
            count_buffer, count_memory,
            descriptor_set, descriptor_pool, descriptor_set_layout,
            pipeline_layout, pipeline,
            device: device.clone(),
        })
    }

    pub fn capacity(&self) -> u32 { self.capacity }
    pub fn high_water(&self) -> u32 { self.high_water }
    pub fn in_use(&self) -> u32 { self.capacity - self.free_list.len() as u32 }

    /// The chunk-metadata SSBO. Exposed for binding into the chunk and
    /// shadow graphics pipelines at descriptor binding 3 — the vertex
    /// shader reads `chunks[gl_InstanceIndex].aabb_min` to recover each
    /// chunk's world origin for unpacking the chunk-local packed vertex
    /// position (v0.24.20-vertex-pack).
    pub fn metadata_buffer(&self) -> vk::Buffer { self.metadata_buffer }
    pub fn metadata_buffer_size(&self) -> u64 {
        (self.capacity as u64) * std::mem::size_of::<ChunkDraw>() as u64
    }

    /// Allocate a slot. Caller must `set` it before the next dispatch or
    /// the entry stays zeroed (which the cull shader correctly skips).
    pub fn alloc(&mut self) -> Result<DrawSlot, IndirectError> {
        let idx = self.free_list.pop_front()
            .ok_or(IndirectError::Exhausted)?;
        if idx + 1 > self.high_water { self.high_water = idx + 1; }
        Ok(DrawSlot(idx))
    }

    /// Update a slot's metadata. Writes through to the GPU's persistent
    /// map immediately — the next compute dispatch sees the new values.
    pub fn set(&mut self, slot: DrawSlot, draw: ChunkDraw) {
        let i = slot.0 as usize;
        debug_assert!(i < self.cpu_shadow.len());
        self.cpu_shadow[i] = draw;
        // SAFETY: persistent map, HOST_COHERENT.
        unsafe {
            let dst = (self.metadata_ptr as *mut ChunkDraw).add(i);
            std::ptr::write(dst, draw);
        }
    }

    /// Free a slot. Marks the GPU entry as inactive (cull shader skips
    /// `index_count == 0`) and recycles the slot index for reuse.
    pub fn free(&mut self, slot: DrawSlot) {
        // Zero out the entry so the next dispatch ignores it.
        self.set(slot, ChunkDraw::empty());
        self.free_list.push_back(slot.0);
        // Don't shrink high_water; the next dispatch wastes some
        // invocations on inactive slots but each is a single load + early
        // return. Lowering high_water on free is filed for later.
    }

    /// Reset the visibility counter for a new frame. Caller must record
    /// this *before* the cull dispatch, with appropriate barriers.
    pub fn record_reset_count(&self, cmd: vk::CommandBuffer) {
        unsafe {
            self.device.cmd_fill_buffer(
                cmd, self.count_buffer, 0,
                std::mem::size_of::<u32>() as u64, 0,
            );
        }
    }

    /// Issue the cull dispatch. Caller is responsible for barriers
    /// before (transfer→shader for the count buffer) and after (shader
    /// write → indirect read) this call.
    pub fn record_cull_dispatch(
        &self,
        cmd: vk::CommandBuffer,
        push: &CullPushConstants,
    ) {
        unsafe {
            self.device.cmd_bind_pipeline(
                cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline);
            self.device.cmd_bind_descriptor_sets(
                cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline_layout,
                0, std::slice::from_ref(&self.descriptor_set), &[],
            );
            let bytes = std::slice::from_raw_parts(
                push as *const _ as *const u8,
                std::mem::size_of::<CullPushConstants>(),
            );
            self.device.cmd_push_constants(
                cmd, self.pipeline_layout,
                vk::ShaderStageFlags::COMPUTE, 0, bytes,
            );
            // Use high_water as the upper bound — the shader checks
            // index_count==0 and early-exits, but bounding the dispatch
            // saves invocations on a fresh world where most slots are
            // still unused.
            let groups = if self.high_water == 0 { 0 }
                else { (self.high_water + CULL_WORKGROUP_SIZE - 1) / CULL_WORKGROUP_SIZE };
            if groups > 0 {
                self.device.cmd_dispatch(cmd, groups, 1, 1);
            }
        }
    }

    /// Tear down. Must be called before the device is destroyed.
    pub unsafe fn destroy(&mut self) {
        let d = &self.device;
        d.destroy_pipeline(self.pipeline, None);
        d.destroy_pipeline_layout(self.pipeline_layout, None);
        d.destroy_descriptor_pool(self.descriptor_pool, None);
        d.destroy_descriptor_set_layout(self.descriptor_set_layout, None);

        d.unmap_memory(self.metadata_memory);
        d.destroy_buffer(self.metadata_buffer, None);
        d.free_memory(self.metadata_memory, None);

        d.destroy_buffer(self.commands_buffer, None);
        d.free_memory(self.commands_memory, None);

        d.destroy_buffer(self.count_buffer, None);
        d.free_memory(self.count_memory, None);

        self.metadata_ptr = std::ptr::null_mut();
    }
}

// ----- Helpers ------------------------------------------------------------

unsafe fn create_buffer(
    device: &Device,
    instance: &Instance,
    physical_device: vk::PhysicalDevice,
    size: u64,
    usage: vk::BufferUsageFlags,
    host_visible: bool,
) -> Result<(vk::Buffer, vk::DeviceMemory, *mut u8), IndirectError> {
    let info = vk::BufferCreateInfo::builder()
        .size(size)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = device.create_buffer(&info, None)?;
    let req = device.get_buffer_memory_requirements(buffer);
    let props = instance.get_physical_device_memory_properties(physical_device);

    let want = if host_visible {
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT
    } else {
        vk::MemoryPropertyFlags::DEVICE_LOCAL
    };
    let mut chosen = None;
    for i in 0..props.memory_type_count {
        if req.memory_type_bits & (1 << i) != 0
            && props.memory_types[i as usize].property_flags.contains(want)
        {
            chosen = Some(i);
            break;
        }
    }
    // Fallback: if DEVICE_LOCAL alone isn't found (unlikely on consumer GPUs
    // but possible on integrated/Apple where unified memory lacks the bit),
    // accept HOST_VISIBLE for the GPU-side buffers too. They're written by
    // the GPU and read by the GPU; HOST_VISIBLE doesn't hurt correctness,
    // only theoretical perf.
    if chosen.is_none() && !host_visible {
        for i in 0..props.memory_type_count {
            if req.memory_type_bits & (1 << i) != 0
                && props.memory_types[i as usize].property_flags.contains(
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )
            {
                chosen = Some(i);
                break;
            }
        }
    }
    let mt = chosen.ok_or_else(|| IndirectError::Other(
        format!("No suitable memory type (host_visible={})", host_visible)))?;

    let alloc = vk::MemoryAllocateInfo::builder()
        .allocation_size(req.size)
        .memory_type_index(mt);
    let memory = device.allocate_memory(&alloc, None)?;
    device.bind_buffer_memory(buffer, memory, 0)?;

    let ptr = if host_visible {
        device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8
    } else {
        std::ptr::null_mut()
    };
    Ok((buffer, memory, ptr))
}

unsafe fn create_descriptor_set_layout(device: &Device) -> Result<vk::DescriptorSetLayout, IndirectError> {
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
        // v0.24.0a: binding 3 = ViewParams SSBO. Holds per-view frustum
        // planes (and, for v0.24.0b+, matrices). The shader reads from
        // view_params[push.view_index].
        vk::DescriptorSetLayoutBinding::builder()
            .binding(3)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .build(),
    ];
    let info = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&bindings);
    Ok(device.create_descriptor_set_layout(&info, None)?)
}

unsafe fn create_pipeline_layout(
    device: &Device, set_layout: vk::DescriptorSetLayout,
) -> Result<vk::PipelineLayout, IndirectError> {
    let pcr = vk::PushConstantRange::builder()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0)
        .size(std::mem::size_of::<CullPushConstants>() as u32);
    let info = vk::PipelineLayoutCreateInfo::builder()
        .set_layouts(std::slice::from_ref(&set_layout))
        .push_constant_ranges(std::slice::from_ref(&pcr));
    Ok(device.create_pipeline_layout(&info, None)?)
}

unsafe fn create_compute_pipeline(
    device: &Device, layout: vk::PipelineLayout,
) -> Result<vk::Pipeline, IndirectError> {
    let candidates = [
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/shaders/chunk_cull.comp.spv"),
        "assets/shaders/chunk_cull.comp.spv",
        "src/shaders/chunk_cull.comp.spv",
    ];
    let mut bytes: Option<Vec<u8>> = None;
    for p in &candidates {
        if Path::new(p).exists() {
            bytes = Some(std::fs::read(p)?);
            break;
        }
    }
    let bytes = bytes.ok_or_else(||
        IndirectError::ShaderMissing(candidates[0].to_string()))?;
    let words: Vec<u32> = bytes.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();

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
    ).map_err(|(_, e)| IndirectError::Vulkan(e))?;
    device.destroy_shader_module(module, None);
    Ok(pipelines[0])
}

unsafe fn allocate_descriptor_set(
    device: &Device, layout: vk::DescriptorSetLayout,
) -> Result<(vk::DescriptorPool, vk::DescriptorSet), IndirectError> {
    // v0.24.0a: 4 descriptors (metadata, commands, count, view_params).
    let sizes = [vk::DescriptorPoolSize {
        ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 4,
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
    metadata: vk::Buffer, commands: vk::Buffer, count: vk::Buffer,
    view_params: vk::Buffer,
) {
    let m_info = vk::DescriptorBufferInfo::builder()
        .buffer(metadata).range(vk::WHOLE_SIZE).build();
    let c_info = vk::DescriptorBufferInfo::builder()
        .buffer(commands).range(vk::WHOLE_SIZE).build();
    let n_info = vk::DescriptorBufferInfo::builder()
        .buffer(count).range(vk::WHOLE_SIZE).build();
    let v_info = vk::DescriptorBufferInfo::builder()
        .buffer(view_params).range(vk::WHOLE_SIZE).build();
    let writes = [
        vk::WriteDescriptorSet::builder()
            .dst_set(set).dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&m_info)).build(),
        vk::WriteDescriptorSet::builder()
            .dst_set(set).dst_binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&c_info)).build(),
        vk::WriteDescriptorSet::builder()
            .dst_set(set).dst_binding(2)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&n_info)).build(),
        // v0.24.0a: ViewParams SSBO at binding 3.
        vk::WriteDescriptorSet::builder()
            .dst_set(set).dst_binding(3)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&v_info)).build(),
    ];
    device.update_descriptor_sets(&writes, &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_max_draws_scales_with_rd_and_clamps() {
        // Small RD floors at MAX_CHUNK_DRAWS.
        assert_eq!(default_max_draws(6), MAX_CHUNK_DRAWS);
        assert_eq!(default_max_draws(1), MAX_CHUNK_DRAWS);
        // RD=12: must comfortably exceed the ~5416 rendering chunks the run
        // hit (the value that exhausted the old fixed 4096 pool).
        assert!(default_max_draws(12) > 5416, "RD=12 default {} must exceed observed peak 5416", default_max_draws(12));
        // Monotonic non-decreasing in RD.
        let mut prev = 0;
        for rd in 1..=40 {
            let d = default_max_draws(rd);
            assert!(d >= prev, "RD {rd}: {d} < prev {prev}");
            assert!((MAX_CHUNK_DRAWS..=65536).contains(&d), "RD {rd}: {d} out of clamp range");
            prev = d;
        }
        // Ceiling holds at very large RD.
        assert_eq!(default_max_draws(64), 65536);
    }

    #[test]
    fn chunk_draw_size_matches_shader() {
        // ChunkDraw on the Rust side must equal the std430 layout in
        // chunk_cull.comp. std430 packs uint/int as 4 bytes, vec4 as 16.
        // 4 × u32 = 16 bytes, then 2 × vec4 = 32 bytes → total 48.
        assert_eq!(std::mem::size_of::<ChunkDraw>(), 48);
    }

    #[test]
    fn indirect_command_size_matches_vulkan() {
        assert_eq!(
            std::mem::size_of::<DrawIndexedIndirect>(),
            std::mem::size_of::<vk::DrawIndexedIndirectCommand>(),
        );
        // 5 × u32 = 20 bytes per the Vulkan spec.
        assert_eq!(std::mem::size_of::<DrawIndexedIndirect>(), 20);
    }

    #[test]
    fn cull_push_is_exactly_16_bytes() {
        // v0.24.0a: shrunk to (view_index, chunk_count). Two u32s = 8.
        // v0.24.4 (shadow-stride): extended with (stride, stride_phase)
        // for temporal shadow amortization. Four u32s = 16 bytes. Far
        // under the 128-byte Vulkan minimum, but pinning it ensures
        // the cull shader's matching push layout stays in sync.
        assert_eq!(std::mem::size_of::<CullPushConstants>(), 16,
            "CullPushConstants size drifted: {}",
            std::mem::size_of::<CullPushConstants>());
    }

    /// v0.24.20-vertex-pack: indices are uploaded UNSHIFTED to the
    /// shared vertex buffer. The per-chunk vertex base is communicated
    /// via the indirect-draw command's `vertex_offset` field (an i32
    /// the GPU adds to each index during fetch). This eliminates the
    /// per-upload CPU pass over all indices.
    ///
    /// This test exercises the conceptual contract:
    ///   - Original mesh indices reference vertices `0..vert_count`
    ///     within the chunk's mesh.
    ///   - On upload, vertex bytes go to byte offset
    ///     `vert_base_aligned`, which corresponds to vertex
    ///     `vert_base_aligned / sizeof(PackedVertex)` in the shared
    ///     buffer.
    ///   - With `vertex_offset = vert_base_count` in the indirect
    ///     command, the GPU fetches vertex `vert_base_count + i` for
    ///     every original index `i` — the right vertex.
    #[test]
    fn vertex_offset_addresses_correct_vertex() {
        // Pool gives us vert_base_aligned = 8 * 1000 (i.e. base
        // index = 1000 vertices in, since sizeof(PackedVertex)=8).
        const VERTEX_SIZE: u32 = 8;
        let vert_base_aligned: u32 = VERTEX_SIZE * 1000;
        let vert_base_count: i32 = (vert_base_aligned / VERTEX_SIZE) as i32;
        assert_eq!(vert_base_count, 1000);

        // Mesh indices are unshifted (reference vertices 0..4 within
        // the chunk's own mesh).
        let unshifted = [0u32, 1, 2, 0, 2, 3];

        // The GPU computes: fetched_vertex_index = vertex_offset + idx.
        // For each original index, the effective vertex fetched.
        let effective: Vec<u32> = unshifted.iter()
            .map(|i| (vert_base_count as u32).wrapping_add(*i))
            .collect();
        assert_eq!(effective, vec![1000, 1001, 1002, 1000, 1002, 1003]);
    }
}
