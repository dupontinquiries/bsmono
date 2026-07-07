//! GPU mesher driven by sparse surface records (v0.24.5 Layer A.2).
//!
//! Counterpart to `world::gpu_mesher_vk::VulkanMesher`, but consumes a
//! `SurfaceRecord[]` buffer instead of a dense chunk-pool slot. The CPU
//! produces the surface records via `world::surface::extract_surface`;
//! this module uploads them, dispatches the compute shader
//! `chunk_mesh_sparse.comp`, and reads back the produced vertex/index
//! buffers.
//!
//! # Architecture
//!
//! Single device, single queue (the renderer's graphics queue). The
//! compute pipeline runs on the same VkDevice as the renderer, so a
//! future zero-copy path can bind the mesher's output buffers directly
//! as the renderer's vertex source. For v1 we read back to CPU and
//! re-upload via the existing `upload_prebuilt_mesh_at_lod` path —
//! wasteful but small, and it isolates the new path from the renderer's
//! buffer-management code.
//!
//! # Per-chunk reservation
//!
//! Each `mesh_chunk` call:
//!   1. Uploads the chunk's surface records into the HOST_VISIBLE input
//!      buffer at offset 0 (we reuse the buffer for every chunk; no
//!      cross-chunk concurrency at this stage).
//!   2. Resets the counter buffer to (0, 0, 0, 0) via cmd_fill_buffer.
//!   3. Dispatches `ceil(record_count / 64)` workgroups.
//!   4. Waits on the fence.
//!   5. Reads back counters → `(vertex_count, index_count, overflow)`.
//!   6. If overflow: returns an error (caller falls back to CPU mesher).
//!   7. Otherwise: reads vertex and index data, packs into a ChunkMesh.
//!
//! Capacity is generous: 1MB vertex buffer (~43k verts) and 256KB index
//! buffer (~64k indices) per chunk. v2 may batch multiple chunks into
//! one dispatch with per-chunk output regions.

#![cfg(feature = "gpu")]

use crate::render::context::VulkanContext;
use crate::render::mesher::{ChunkMesh, Vertex};
use crate::world::chunk::ChunkCoord;
use crate::world::surface::SurfaceRecord;
use ash::{vk, Device};
use std::ffi::CStr;
use std::path::Path;
use std::sync::Mutex;

const ENTRY_NAME: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"main\0") };

/// Workgroup size. Must match `local_size_x` in the shader.
const WORKGROUP_SIZE: u32 = 64;

/// Per-chunk vertex capacity (in vertices, not bytes). 43_690 verts ≈
/// 1MB at 24 bytes/vertex. Typical chunk has 2-5K verts; this is ~10×
/// headroom.
pub const VERTEX_CAPACITY: u32 = 43_690;

/// Per-chunk index capacity (in u32 indices, not bytes). 65_535 indices
/// = 256KB. With 6 indices/quad that's ~10K quads, matching ~43K verts.
pub const INDEX_CAPACITY: u32 = 65_535;

/// Max records (= max surface blocks per chunk). Worst case is a fully-
/// alternating checkerboard pattern, but realistically capped well below
/// CHUNK_VOLUME. 16K × 16B = 256KB upload region per slot.
pub const MAX_RECORDS_PER_CHUNK: u32 = 16_384;

/// Maximum chunks meshed per batch. Buffers are sized for this many
/// slots; the actual chunks-per-submission can be set lower at runtime
/// via the `UNENDLICH_GPU_MESH_BATCH` env var (see `effective_batch_size`).
///
/// History:
///   v0.24.7: 16 (24 MB persistent HOST_VISIBLE buffers).
///   v0.24.11: 64 (96 MB). Lets us reach single-submission for typical
///     chunk-load bursts of 30-60 chunks, fully amortizing the ~550us
///     AGX dispatch floor.
///
/// On M2 unified memory, 96 MB is trivial. On discrete GPUs, it's
/// host-visible PCIe memory which is also fine.
pub const MAX_BATCH_CHUNKS: u32 = 64;

/// Read the runtime batch-size cap from `UNENDLICH_GPU_MESH_BATCH`.
/// Defaults to MAX_BATCH_CHUNKS. Clamped to [1, MAX_BATCH_CHUNKS].
/// Lets the test harness sweep batch sizes without rebuilding.
pub fn effective_batch_size() -> u32 {
    let max = MAX_BATCH_CHUNKS;
    match std::env::var("UNENDLICH_GPU_MESH_BATCH").ok().and_then(|s| s.parse::<u32>().ok()) {
        Some(n) if n >= 1 && n <= max => n,
        Some(n) if n > max => max,
        _ => max,
    }
}

/// One chunk's input for the batched mesh API.
pub struct ChunkBatchInput<'a> {
    pub coord: ChunkCoord,
    pub records: &'a [SurfaceRecord],
}

/// One chunk's output. `mesh` is per-chunk Result so one chunk's
/// overflow doesn't sink the whole batch — the caller can fall back
/// to CPU mesher for that specific chunk.
pub struct ChunkBatchOutput {
    pub coord: ChunkCoord,
    pub mesh: Result<ChunkMesh>,
}

/// One chunk's input for the runs-batched mesh API (v0.24.10).
/// Same shape as `ChunkBatchInput` but carries `SurfaceRun` instead
/// of `SurfaceRecord`. Different kernel, different upload format.
pub struct ChunkBatchInputRuns<'a> {
    pub coord: ChunkCoord,
    pub runs: &'a [crate::world::surface::SurfaceRun],
}

#[derive(Debug)]
pub enum SparseMeshError {
    Vulkan(vk::Result),
    ShaderMissing(String),
    Io(std::io::Error),
    Overflow { vertex_count: u32, index_count: u32 },
    TooManyRecords(usize),
    NoSuitableMemoryType,
}

impl From<vk::Result> for SparseMeshError {
    fn from(r: vk::Result) -> Self { Self::Vulkan(r) }
}
impl From<std::io::Error> for SparseMeshError {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}
impl std::fmt::Display for SparseMeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vulkan(r) => write!(f, "Vulkan error: {:?}", r),
            Self::ShaderMissing(s) => write!(f, "shader not found at: {}", s),
            Self::Io(e) => write!(f, "I/O: {}", e),
            Self::Overflow { vertex_count, index_count } =>
                write!(f, "GPU mesh overflowed capacity (verts={}, idx={})",
                       vertex_count, index_count),
            Self::TooManyRecords(n) =>
                write!(f, "too many surface records: {} (max {})",
                       n, MAX_RECORDS_PER_CHUNK),
            Self::NoSuitableMemoryType => write!(f, "no host-visible memory type"),
        }
    }
}
impl std::error::Error for SparseMeshError {}

pub type Result<T> = std::result::Result<T, SparseMeshError>;

/// Sparse GPU mesher.
///
/// Owns its compute pipeline, descriptor set, and per-chunk buffers.
/// Shares the device + queue with the renderer's `VulkanContext`.
///
/// Not thread-safe for concurrent `mesh_chunk` calls — internal
/// `Mutex<()>` serializes submissions. A future version with per-frame
/// command buffers could mesh chunks in parallel.
pub struct SparseGpuMesher {
    // Per-chunk buffers, all HOST_VISIBLE persistent-mapped.
    records_buffer: vk::Buffer,
    records_memory: vk::DeviceMemory,
    records_ptr: *mut u8,

    vertex_buffer: vk::Buffer,
    vertex_memory: vk::DeviceMemory,
    vertex_ptr: *mut u8,

    index_buffer: vk::Buffer,
    index_memory: vk::DeviceMemory,
    index_ptr: *mut u8,

    counter_buffer: vk::Buffer,
    counter_memory: vk::DeviceMemory,
    counter_ptr: *mut u8,

    pipeline: vk::Pipeline,
    /// Second pipeline for the runs kernel (v0.24.10). Reads the same
    /// buffers but a different shader. Shares pipeline_layout because
    /// push constants and descriptor set layout are identical.
    runs_pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,

    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    fence: vk::Fence,
    /// Serializes inter-mesher access to the mesher's own buffers
    /// (records / vertex / index / counter) — only one batch in flight
    /// at a time. The buffers are reused across batches.
    submit_lock: Mutex<()>,
    /// v0.24.21-gpu-mesher-prod: shared with the renderer's
    /// `VulkanContext::queue_submit_lock`. Acquired briefly around
    /// `vkQueueSubmit` so the mesher's submits don't race with the
    /// renderer's. Not held during fence-wait or readback — those are
    /// CPU/GPU sync, not queue access.
    queue_submit_lock: std::sync::Arc<Mutex<()>>,

    queue: vk::Queue,
    device: Device,
}

// SAFETY: persistent maps are HOST_COHERENT, all access via submit_lock.
unsafe impl Send for SparseGpuMesher {}
unsafe impl Sync for SparseGpuMesher {}

#[repr(C)]
#[derive(Copy, Clone)]
struct PushConstants {
    record_count: u32,
    vertex_capacity: u32,
    index_capacity: u32,
    record_offset: u32,   // first record index for this chunk in records buffer
    vertex_offset: u32,   // first vertex slot for this chunk
    index_offset: u32,    // first index slot for this chunk
    counter_offset: u32,  // u32 offset of this chunk's 4-u32 counter group
    _pad: u32,
}

impl SparseGpuMesher {
    /// Construct using the renderer's existing Vulkan device + queue.
    /// The mesher submits to the same graphics queue as the renderer;
    /// the internal mutex serializes against rendering on the bin's
    /// side (caller's responsibility to not call this during a frame
    /// where the queue is being submitted to).
    pub fn new(ctx: &VulkanContext) -> Result<Self> {
        let device = ctx.device.clone();
        let queue = ctx.graphics_queue;
        let queue_family = ctx.graphics_queue_family;

        // Allocate the four storage buffers, sized for MAX_BATCH_CHUNKS slots.
        // Each chunk gets its own slot within each buffer; slots are contiguous.
        //
        //   records:  MAX_BATCH_CHUNKS × MAX_RECORDS_PER_CHUNK × 16 B
        //   vertices: MAX_BATCH_CHUNKS × VERTEX_CAPACITY × 24 B
        //   indices:  MAX_BATCH_CHUNKS × INDEX_CAPACITY × 4 B
        //   counters: MAX_BATCH_CHUNKS × 4 × 4 B (vert, idx, overflow, reserved per chunk)
        let records_size = (MAX_BATCH_CHUNKS as u64)
            * (MAX_RECORDS_PER_CHUNK as u64) * (SurfaceRecord::SIZE as u64);
        let (records_buffer, records_memory, records_ptr) =
            create_host_visible_buffer(&device, ctx, records_size)?;

        let vertex_size = (MAX_BATCH_CHUNKS as u64) * (VERTEX_CAPACITY as u64) * 24;
        let (vertex_buffer, vertex_memory, vertex_ptr) =
            create_host_visible_buffer(&device, ctx, vertex_size)?;

        let index_size = (MAX_BATCH_CHUNKS as u64) * (INDEX_CAPACITY as u64) * 4;
        let (index_buffer, index_memory, index_ptr) =
            create_host_visible_buffer(&device, ctx, index_size)?;

        let counter_size = (MAX_BATCH_CHUNKS as u64) * 16; // 4 u32 per chunk
        let (counter_buffer, counter_memory, counter_ptr) =
            create_host_visible_buffer(&device, ctx, counter_size)?;

        // Pipeline, descriptors.
        let descriptor_set_layout = unsafe { create_descriptor_set_layout(&device)? };
        let pipeline_layout = unsafe { create_pipeline_layout(&device, descriptor_set_layout)? };
        let pipeline = unsafe { create_compute_pipeline_named(&device, pipeline_layout, "chunk_mesh_sparse")? };
        let runs_pipeline = unsafe { create_compute_pipeline_named(&device, pipeline_layout, "chunk_mesh_runs")? };
        let descriptor_pool = unsafe { create_descriptor_pool(&device)? };
        let descriptor_set = unsafe {
            allocate_descriptor_set(&device, descriptor_pool, descriptor_set_layout)?
        };
        unsafe {
            update_descriptor_set(
                &device, descriptor_set,
                records_buffer, vertex_buffer, index_buffer, counter_buffer,
            );
        }

        // Command pool, command buffer, fence.
        let command_pool = unsafe {
            let info = vk::CommandPoolCreateInfo::builder()
                .queue_family_index(queue_family)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
            device.create_command_pool(&info, None)?
        };
        let command_buffer = unsafe {
            let info = vk::CommandBufferAllocateInfo::builder()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1);
            device.allocate_command_buffers(&info)?[0]
        };
        let fence = unsafe {
            let info = vk::FenceCreateInfo::builder();
            device.create_fence(&info, None)?
        };

        log::info!(
            "SparseGpuMesher initialized (record cap={}, vert cap={}, idx cap={})",
            MAX_RECORDS_PER_CHUNK, VERTEX_CAPACITY, INDEX_CAPACITY,
        );

        Ok(Self {
            records_buffer, records_memory, records_ptr,
            vertex_buffer, vertex_memory, vertex_ptr,
            index_buffer, index_memory, index_ptr,
            counter_buffer, counter_memory, counter_ptr,
            pipeline, runs_pipeline, pipeline_layout, descriptor_set_layout,
            descriptor_pool, descriptor_set,
            command_pool, command_buffer, fence,
            submit_lock: Mutex::new(()),
            queue_submit_lock: ctx.queue_submit_lock.clone(),
            queue, device,
        })
    }

    /// Mesh one chunk's surface records into a `ChunkMesh`. Returns
    /// `Err(Overflow)` if the GPU output exceeded capacity (caller
    /// should fall back to CPU mesher for that chunk).
    ///
    /// Implemented as a single-element batch dispatch — same code path
    /// as `mesh_chunks_batch`, batch size 1. Existing callers (including
    /// `gpu-mesh-bench`) work unchanged.
    pub fn mesh_chunk(&self, coord: ChunkCoord, records: &[SurfaceRecord]) -> Result<ChunkMesh> {
        let input = [ChunkBatchInput { coord, records }];
        let mut out = self.mesh_chunks_batch(&input)?;
        out.pop().unwrap().mesh
    }

    /// Mesh a batch of chunks in a single GPU submission. The dispatch
    /// floor on AGX (~550us per submit) is paid once for the entire batch,
    /// not per chunk — this is the primary perf win over `mesh_chunk`.
    ///
    /// Each chunk gets its own slot in the input and output buffers.
    /// Returns one output per input, in the same order. Per-chunk errors
    /// (overflow, too-many-records) are localized — one chunk's failure
    /// doesn't sink the batch.
    ///
    /// Batch size must be ≤ MAX_BATCH_CHUNKS (currently 16). Larger
    /// inputs are split internally into multiple submissions.
    pub fn mesh_chunks_batch(&self, chunks: &[ChunkBatchInput]) -> Result<Vec<ChunkBatchOutput>> {
        if chunks.is_empty() {
            return Ok(Vec::new());
        }

        // Split oversized batches into MAX_BATCH_CHUNKS-sized submissions.
        // Recursion would work but explicit chunking is clearer.
        let mut all_outputs = Vec::with_capacity(chunks.len());
        for window in chunks.chunks(effective_batch_size() as usize) {
            let mut outs = self.mesh_chunks_one_submission(window)?;
            all_outputs.append(&mut outs);
        }
        Ok(all_outputs)
    }

    /// Mesh up to MAX_BATCH_CHUNKS chunks in exactly one submission.
    /// Internal helper for `mesh_chunks_batch`.
    fn mesh_chunks_one_submission(
        &self, chunks: &[ChunkBatchInput],
    ) -> Result<Vec<ChunkBatchOutput>> {
        debug_assert!(chunks.len() <= MAX_BATCH_CHUNKS as usize);

        // Pre-screen for too-many-records errors and identify chunks that
        // need GPU work vs chunks that are empty (skip GPU entirely).
        let mut active_slots: Vec<usize> = Vec::with_capacity(chunks.len());
        let mut outputs: Vec<ChunkBatchOutput> = chunks.iter().map(|c| {
            if c.records.len() > MAX_RECORDS_PER_CHUNK as usize {
                ChunkBatchOutput {
                    coord: c.coord,
                    mesh: Err(SparseMeshError::TooManyRecords(c.records.len())),
                }
            } else if c.records.is_empty() {
                ChunkBatchOutput {
                    coord: c.coord,
                    mesh: Ok(ChunkMesh { vertices: Vec::new(), indices: Vec::new() }),
                }
            } else {
                // Placeholder; filled after GPU dispatch.
                ChunkBatchOutput {
                    coord: c.coord,
                    mesh: Err(SparseMeshError::Vulkan(vk::Result::NOT_READY)),
                }
            }
        }).collect();

        for (idx, c) in chunks.iter().enumerate() {
            if !c.records.is_empty()
                && c.records.len() <= MAX_RECORDS_PER_CHUNK as usize
            {
                active_slots.push(idx);
            }
        }

        if active_slots.is_empty() {
            // Whole batch is empties / too-many. No GPU work needed.
            return Ok(outputs);
        }

        // Serialize against concurrent submissions on the shared queue.
        let _guard = self.submit_lock.lock().unwrap();

        // 1. Upload records for each active slot.
        unsafe {
            for &slot_idx in &active_slots {
                let c = &chunks[slot_idx];
                let dst_offset_bytes =
                    slot_idx * (MAX_RECORDS_PER_CHUNK as usize) * SurfaceRecord::SIZE;
                let dst = self.records_ptr.add(dst_offset_bytes);
                let bytes = c.records.len() * SurfaceRecord::SIZE;
                std::ptr::copy_nonoverlapping(
                    c.records.as_ptr() as *const u8,
                    dst,
                    bytes,
                );
            }
        }

        // 2. Record command buffer: reset all active counter groups,
        //    one dispatch per active slot.
        let counter_bytes_total = (MAX_BATCH_CHUNKS as u64) * 16;
        unsafe {
            self.device.reset_command_buffer(
                self.command_buffer,
                vk::CommandBufferResetFlags::empty(),
            )?;
            let begin = vk::CommandBufferBeginInfo::builder()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            self.device.begin_command_buffer(self.command_buffer, &begin)?;

            // Reset all counters (cheap — total 256 bytes).
            self.device.cmd_fill_buffer(
                self.command_buffer,
                self.counter_buffer, 0, counter_bytes_total, 0,
            );
            // Barrier: transfer writes → shader reads/writes.
            let counter_barrier = vk::BufferMemoryBarrier::builder()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.counter_buffer)
                .offset(0)
                .size(counter_bytes_total)
                .build();
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[counter_barrier],
                &[],
            );

            // Bind pipeline + descriptor set ONCE for the whole batch.
            self.device.cmd_bind_pipeline(
                self.command_buffer,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline,
            );
            self.device.cmd_bind_descriptor_sets(
                self.command_buffer,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_layout,
                0,
                std::slice::from_ref(&self.descriptor_set),
                &[],
            );

            // One dispatch per active slot. Each gets its own push
            // constants identifying which slot to read/write.
            for &slot_idx in &active_slots {
                let c = &chunks[slot_idx];
                let record_offset = (slot_idx as u32) * MAX_RECORDS_PER_CHUNK;
                let vertex_offset = (slot_idx as u32) * VERTEX_CAPACITY;
                let index_offset = (slot_idx as u32) * INDEX_CAPACITY;
                let counter_offset = (slot_idx as u32) * 4; // 4 u32 per slot

                let push = PushConstants {
                    record_count: c.records.len() as u32,
                    vertex_capacity: VERTEX_CAPACITY,
                    index_capacity: INDEX_CAPACITY,
                    record_offset,
                    vertex_offset,
                    index_offset,
                    counter_offset,
                    _pad: 0,
                };
                let push_bytes = std::slice::from_raw_parts(
                    &push as *const _ as *const u8,
                    std::mem::size_of::<PushConstants>(),
                );
                self.device.cmd_push_constants(
                    self.command_buffer,
                    self.pipeline_layout,
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    push_bytes,
                );

                let workgroups = (c.records.len() as u32 + WORKGROUP_SIZE - 1) / WORKGROUP_SIZE;
                self.device.cmd_dispatch(self.command_buffer, workgroups, 1, 1);
            }

            // Barrier: shader writes → host reads (single barrier covering
            // all output buffers; per-slot regions are written and read
            // independently but all need to be visible before readback).
            let out_barrier_v = vk::BufferMemoryBarrier::builder()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                .dst_access_mask(vk::AccessFlags::HOST_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.vertex_buffer)
                .offset(0).size(vk::WHOLE_SIZE)
                .build();
            let out_barrier_i = vk::BufferMemoryBarrier::builder()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                .dst_access_mask(vk::AccessFlags::HOST_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.index_buffer)
                .offset(0).size(vk::WHOLE_SIZE)
                .build();
            let out_barrier_c = vk::BufferMemoryBarrier::builder()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                .dst_access_mask(vk::AccessFlags::HOST_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.counter_buffer)
                .offset(0).size(counter_bytes_total)
                .build();
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[],
                &[out_barrier_v, out_barrier_i, out_barrier_c],
                &[],
            );

            self.device.end_command_buffer(self.command_buffer)?;

            // Submit + wait. ONE submit covers the whole batch — this
            // is where the dispatch-floor amortization happens.
            self.device.reset_fences(&[self.fence])?;
            let submit = vk::SubmitInfo::builder()
                .command_buffers(std::slice::from_ref(&self.command_buffer))
                .build();
            // v0.24.21-gpu-mesher-prod: serialize against renderer's
            // submits on the same queue. Released immediately so the
            // fence-wait below doesn't block the renderer.
            {
                let _qg = self.queue_submit_lock.lock()
                    .expect("queue submit lock poisoned");
                self.device.queue_submit(self.queue, &[submit], self.fence)?;
            }
            self.device.wait_for_fences(&[self.fence], true, u64::MAX)?;
        }

        // 3. Per-slot readback. Counter groups are 4 u32 each; vertex
        //    and index data live at slot_idx × CAPACITY.
        for &slot_idx in &active_slots {
            let (vert_count, idx_count, overflow) = unsafe {
                let counters_ptr = (self.counter_ptr as *const u32).add(slot_idx * 4);
                (*counters_ptr.add(0), *counters_ptr.add(1), *counters_ptr.add(2))
            };

            if overflow > 0 {
                outputs[slot_idx].mesh = Err(SparseMeshError::Overflow {
                    vertex_count: vert_count,
                    index_count: idx_count,
                });
                continue;
            }

            let vertices = unsafe {
                let slot_start_floats = slot_idx * (VERTEX_CAPACITY as usize) * 6;
                let floats = std::slice::from_raw_parts(
                    (self.vertex_ptr as *const f32).add(slot_start_floats),
                    (vert_count as usize) * 6,
                );
                let mut v = Vec::with_capacity(vert_count as usize);
                for chunk in floats.chunks_exact(6) {
                    v.push(Vertex::new(
                        [chunk[0], chunk[1], chunk[2]],
                        [chunk[3], chunk[4], chunk[5]],
                    ));
                }
                v
            };
            let indices = unsafe {
                let slot_start = slot_idx * (INDEX_CAPACITY as usize);
                let raw = std::slice::from_raw_parts(
                    (self.index_ptr as *const u32).add(slot_start),
                    idx_count as usize,
                );
                raw.to_vec()
            };

            outputs[slot_idx].mesh = Ok(ChunkMesh { vertices, indices });
        }

        Ok(outputs)
    }

    // ─────────────────────────────────────────────────────────────────
    // Runs API (v0.24.10) — consumes SurfaceRun[] instead of SurfaceRecord[].
    //
    // Same buffer layout, same descriptor set, different pipeline. The
    // runs kernel emits exactly one quad per run; the records kernel
    // emits up to 6 per record.
    // ─────────────────────────────────────────────────────────────────

    /// Mesh a batch of chunks using the runs kernel. Each chunk's input
    /// is a slice of `SurfaceRun`s from `extract_surface_runs`. Output is
    /// a `ChunkMesh` per chunk, same as `mesh_chunks_batch`.
    pub fn mesh_chunks_batch_runs(
        &self, chunks: &[ChunkBatchInputRuns],
    ) -> Result<Vec<ChunkBatchOutput>> {
        if chunks.is_empty() {
            return Ok(Vec::new());
        }
        let mut all_outputs = Vec::with_capacity(chunks.len());
        for window in chunks.chunks(effective_batch_size() as usize) {
            let mut outs = self.mesh_chunks_one_submission_runs(window)?;
            all_outputs.append(&mut outs);
        }
        Ok(all_outputs)
    }

    fn mesh_chunks_one_submission_runs(
        &self, chunks: &[ChunkBatchInputRuns],
    ) -> Result<Vec<ChunkBatchOutput>> {
        use crate::world::surface::SurfaceRun;
        debug_assert!(chunks.len() <= MAX_BATCH_CHUNKS as usize);

        let mut active_slots: Vec<usize> = Vec::with_capacity(chunks.len());
        let mut outputs: Vec<ChunkBatchOutput> = chunks.iter().map(|c| {
            if c.runs.len() > MAX_RECORDS_PER_CHUNK as usize {
                ChunkBatchOutput {
                    coord: c.coord,
                    mesh: Err(SparseMeshError::TooManyRecords(c.runs.len())),
                }
            } else if c.runs.is_empty() {
                ChunkBatchOutput {
                    coord: c.coord,
                    mesh: Ok(ChunkMesh { vertices: Vec::new(), indices: Vec::new() }),
                }
            } else {
                ChunkBatchOutput {
                    coord: c.coord,
                    mesh: Err(SparseMeshError::Vulkan(vk::Result::NOT_READY)),
                }
            }
        }).collect();

        for (idx, c) in chunks.iter().enumerate() {
            if !c.runs.is_empty()
                && c.runs.len() <= MAX_RECORDS_PER_CHUNK as usize
            {
                active_slots.push(idx);
            }
        }

        if active_slots.is_empty() {
            return Ok(outputs);
        }

        let _guard = self.submit_lock.lock().unwrap();

        // Upload runs to per-slot regions. SurfaceRun::SIZE == 16, same
        // as SurfaceRecord, so the same MAX_RECORDS_PER_CHUNK slot
        // stride applies.
        unsafe {
            for &slot_idx in &active_slots {
                let c = &chunks[slot_idx];
                let dst_offset_bytes =
                    slot_idx * (MAX_RECORDS_PER_CHUNK as usize) * SurfaceRun::SIZE;
                let dst = self.records_ptr.add(dst_offset_bytes);
                let bytes = c.runs.len() * SurfaceRun::SIZE;
                std::ptr::copy_nonoverlapping(
                    c.runs.as_ptr() as *const u8,
                    dst,
                    bytes,
                );
            }
        }

        let counter_bytes_total = (MAX_BATCH_CHUNKS as u64) * 16;
        unsafe {
            self.device.reset_command_buffer(
                self.command_buffer,
                vk::CommandBufferResetFlags::empty(),
            )?;
            let begin = vk::CommandBufferBeginInfo::builder()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            self.device.begin_command_buffer(self.command_buffer, &begin)?;

            self.device.cmd_fill_buffer(
                self.command_buffer,
                self.counter_buffer, 0, counter_bytes_total, 0,
            );
            let counter_barrier = vk::BufferMemoryBarrier::builder()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.counter_buffer)
                .offset(0).size(counter_bytes_total)
                .build();
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[counter_barrier],
                &[],
            );

            // Bind the RUNS pipeline (not the records one).
            self.device.cmd_bind_pipeline(
                self.command_buffer,
                vk::PipelineBindPoint::COMPUTE,
                self.runs_pipeline,
            );
            self.device.cmd_bind_descriptor_sets(
                self.command_buffer,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_layout,
                0,
                std::slice::from_ref(&self.descriptor_set),
                &[],
            );

            for &slot_idx in &active_slots {
                let c = &chunks[slot_idx];
                let record_offset = (slot_idx as u32) * MAX_RECORDS_PER_CHUNK;
                let vertex_offset = (slot_idx as u32) * VERTEX_CAPACITY;
                let index_offset = (slot_idx as u32) * INDEX_CAPACITY;
                let counter_offset = (slot_idx as u32) * 4;
                let push = PushConstants {
                    record_count: c.runs.len() as u32,
                    vertex_capacity: VERTEX_CAPACITY,
                    index_capacity: INDEX_CAPACITY,
                    record_offset,
                    vertex_offset,
                    index_offset,
                    counter_offset,
                    _pad: 0,
                };
                let push_bytes = std::slice::from_raw_parts(
                    &push as *const _ as *const u8,
                    std::mem::size_of::<PushConstants>(),
                );
                self.device.cmd_push_constants(
                    self.command_buffer,
                    self.pipeline_layout,
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    push_bytes,
                );

                let workgroups = (c.runs.len() as u32 + WORKGROUP_SIZE - 1) / WORKGROUP_SIZE;
                self.device.cmd_dispatch(self.command_buffer, workgroups, 1, 1);
            }

            let out_barrier_v = vk::BufferMemoryBarrier::builder()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                .dst_access_mask(vk::AccessFlags::HOST_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.vertex_buffer)
                .offset(0).size(vk::WHOLE_SIZE)
                .build();
            let out_barrier_i = vk::BufferMemoryBarrier::builder()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                .dst_access_mask(vk::AccessFlags::HOST_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.index_buffer)
                .offset(0).size(vk::WHOLE_SIZE)
                .build();
            let out_barrier_c = vk::BufferMemoryBarrier::builder()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                .dst_access_mask(vk::AccessFlags::HOST_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.counter_buffer)
                .offset(0).size(counter_bytes_total)
                .build();
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[],
                &[out_barrier_v, out_barrier_i, out_barrier_c],
                &[],
            );

            self.device.end_command_buffer(self.command_buffer)?;

            self.device.reset_fences(&[self.fence])?;
            let submit = vk::SubmitInfo::builder()
                .command_buffers(std::slice::from_ref(&self.command_buffer))
                .build();
            // v0.24.21-gpu-mesher-prod: same queue lock as the records
            // path. See the matching comment above.
            {
                let _qg = self.queue_submit_lock.lock()
                    .expect("queue submit lock poisoned");
                self.device.queue_submit(self.queue, &[submit], self.fence)?;
            }
            self.device.wait_for_fences(&[self.fence], true, u64::MAX)?;
        }

        // Per-slot readback (identical to the records path).
        for &slot_idx in &active_slots {
            let (vert_count, idx_count, overflow) = unsafe {
                let counters_ptr = (self.counter_ptr as *const u32).add(slot_idx * 4);
                (*counters_ptr.add(0), *counters_ptr.add(1), *counters_ptr.add(2))
            };
            if overflow > 0 {
                outputs[slot_idx].mesh = Err(SparseMeshError::Overflow {
                    vertex_count: vert_count,
                    index_count: idx_count,
                });
                continue;
            }
            let vertices = unsafe {
                let slot_start_floats = slot_idx * (VERTEX_CAPACITY as usize) * 6;
                let floats = std::slice::from_raw_parts(
                    (self.vertex_ptr as *const f32).add(slot_start_floats),
                    (vert_count as usize) * 6,
                );
                let mut v = Vec::with_capacity(vert_count as usize);
                for chunk in floats.chunks_exact(6) {
                    v.push(Vertex::new(
                        [chunk[0], chunk[1], chunk[2]],
                        [chunk[3], chunk[4], chunk[5]],
                    ));
                }
                v
            };
            let indices = unsafe {
                let slot_start = slot_idx * (INDEX_CAPACITY as usize);
                let raw = std::slice::from_raw_parts(
                    (self.index_ptr as *const u32).add(slot_start),
                    idx_count as usize,
                );
                raw.to_vec()
            };
            outputs[slot_idx].mesh = Ok(ChunkMesh { vertices, indices });
        }

        Ok(outputs)
    }
}

impl Drop for SparseGpuMesher {
    fn drop(&mut self) {
        unsafe {
            // Best-effort cleanup; ignore errors (we're dropping anyway).
            let _ = self.device.device_wait_idle();

            self.device.destroy_fence(self.fence, None);
            self.device.destroy_command_pool(self.command_pool, None);

            self.device.destroy_descriptor_pool(self.descriptor_pool, None);
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline(self.runs_pipeline, None);
            self.device.destroy_pipeline_layout(self.pipeline_layout, None);
            self.device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);

            self.device.unmap_memory(self.records_memory);
            self.device.destroy_buffer(self.records_buffer, None);
            self.device.free_memory(self.records_memory, None);

            self.device.unmap_memory(self.vertex_memory);
            self.device.destroy_buffer(self.vertex_buffer, None);
            self.device.free_memory(self.vertex_memory, None);

            self.device.unmap_memory(self.index_memory);
            self.device.destroy_buffer(self.index_buffer, None);
            self.device.free_memory(self.index_memory, None);

            self.device.unmap_memory(self.counter_memory);
            self.device.destroy_buffer(self.counter_buffer, None);
            self.device.free_memory(self.counter_memory, None);
        }
    }
}

// ───── Vulkan helpers ──────────────────────────────────────────────────

fn create_host_visible_buffer(
    device: &Device,
    ctx: &VulkanContext,
    size: u64,
) -> Result<(vk::Buffer, vk::DeviceMemory, *mut u8)> {
    let info = vk::BufferCreateInfo::builder()
        .size(size)
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = unsafe { device.create_buffer(&info, None)? };
    let req = unsafe { device.get_buffer_memory_requirements(buffer) };

    let mem_props = unsafe {
        ctx.instance.get_physical_device_memory_properties(ctx.physical_device)
    };
    let memory_type_index = find_host_visible_memory_type(&mem_props, req.memory_type_bits)
        .ok_or(SparseMeshError::NoSuitableMemoryType)?;

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

fn find_host_visible_memory_type(
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    type_filter: u32,
) -> Option<u32> {
    let want = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
    for i in 0..mem_props.memory_type_count {
        if type_filter & (1 << i) != 0 {
            let props = mem_props.memory_types[i as usize].property_flags;
            if props.contains(want) {
                return Some(i);
            }
        }
    }
    None
}

unsafe fn create_descriptor_set_layout(device: &Device) -> Result<vk::DescriptorSetLayout> {
    let bindings = [
        // 0: records (read-only storage)
        vk::DescriptorSetLayoutBinding::builder()
            .binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .build(),
        // 1: vertices (write)
        vk::DescriptorSetLayoutBinding::builder()
            .binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .build(),
        // 2: indices (write)
        vk::DescriptorSetLayoutBinding::builder()
            .binding(2)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .build(),
        // 3: counters (read-write)
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
) -> Result<vk::PipelineLayout> {
    let push_range = vk::PushConstantRange::builder()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0)
        .size(std::mem::size_of::<PushConstants>() as u32)
        .build();
    let info = vk::PipelineLayoutCreateInfo::builder()
        .set_layouts(std::slice::from_ref(&set_layout))
        .push_constant_ranges(std::slice::from_ref(&push_range));
    Ok(device.create_pipeline_layout(&info, None)?)
}

unsafe fn create_compute_pipeline(
    device: &Device, layout: vk::PipelineLayout,
) -> Result<vk::Pipeline> {
    create_compute_pipeline_named(device, layout, "chunk_mesh_sparse")
}

/// Load and create a compute pipeline from a SPIR-V file. The shader
/// is found under `assets/shaders/{name}.comp.spv` or
/// `src/shaders/{name}.comp.spv` (build-tree fallback).
unsafe fn create_compute_pipeline_named(
    device: &Device, layout: vk::PipelineLayout, name: &str,
) -> Result<vk::Pipeline> {
    let candidates = [
        format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/shaders/{}.comp.spv"), name),
        format!("assets/shaders/{}.comp.spv", name),
        format!("src/shaders/{}.comp.spv", name),
    ];
    let mut bytes: Option<Vec<u8>> = None;
    for p in &candidates {
        if Path::new(p).exists() {
            bytes = Some(std::fs::read(p)?);
            break;
        }
    }
    let bytes = bytes.ok_or_else(||
        SparseMeshError::ShaderMissing(candidates[0].clone()))?;
    let words: Vec<u32> = bytes.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();

    let module_info = vk::ShaderModuleCreateInfo::builder().code(&words);
    let module = device.create_shader_module(&module_info, None)?;
    let stage = vk::PipelineShaderStageCreateInfo::builder()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module)
        .name(ENTRY_NAME)
        .build();
    let info = vk::ComputePipelineCreateInfo::builder()
        .stage(stage)
        .layout(layout)
        .build();
    let pipelines = device.create_compute_pipelines(
        vk::PipelineCache::null(),
        std::slice::from_ref(&info),
        None,
    ).map_err(|(_, r)| SparseMeshError::Vulkan(r))?;
    device.destroy_shader_module(module, None);
    Ok(pipelines[0])
}

unsafe fn create_descriptor_pool(device: &Device) -> Result<vk::DescriptorPool> {
    let pool_size = vk::DescriptorPoolSize::builder()
        .ty(vk::DescriptorType::STORAGE_BUFFER)
        .descriptor_count(4)
        .build();
    let info = vk::DescriptorPoolCreateInfo::builder()
        .pool_sizes(std::slice::from_ref(&pool_size))
        .max_sets(1);
    Ok(device.create_descriptor_pool(&info, None)?)
}

unsafe fn allocate_descriptor_set(
    device: &Device,
    pool: vk::DescriptorPool,
    layout: vk::DescriptorSetLayout,
) -> Result<vk::DescriptorSet> {
    let info = vk::DescriptorSetAllocateInfo::builder()
        .descriptor_pool(pool)
        .set_layouts(std::slice::from_ref(&layout));
    Ok(device.allocate_descriptor_sets(&info)?[0])
}

unsafe fn update_descriptor_set(
    device: &Device,
    set: vk::DescriptorSet,
    records: vk::Buffer,
    vertices: vk::Buffer,
    indices: vk::Buffer,
    counters: vk::Buffer,
) {
    let bindings = [
        (0u32, records),
        (1, vertices),
        (2, indices),
        (3, counters),
    ];
    let infos: Vec<vk::DescriptorBufferInfo> = bindings.iter().map(|(_, b)|
        vk::DescriptorBufferInfo::builder()
            .buffer(*b)
            .offset(0)
            .range(vk::WHOLE_SIZE)
            .build()
    ).collect();
    let writes: Vec<vk::WriteDescriptorSet> = bindings.iter().enumerate().map(|(i, (b, _))|
        vk::WriteDescriptorSet::builder()
            .dst_set(set)
            .dst_binding(*b)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&infos[i]))
            .build()
    ).collect();
    device.update_descriptor_sets(&writes, &[]);
}
