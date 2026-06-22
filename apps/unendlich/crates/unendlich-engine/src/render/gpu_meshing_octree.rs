//! Octree-driven GPU mesher (v0.24.27-octree-mesher).
//!
//! Sibling of `gpu_meshing_sparse` but reads world data from per-
//! chunk `OctreeArray` bytes instead of CPU-extracted
//! `SurfaceRecord[]`. Same dispatcher / batching / readback shape;
//! the input wire format changes and the compute shader changes,
//! everything else parallels the sparse mesher.
//!
//! ## Why this exists
//!
//! Architecture-v1 §3.1 calls for one substrate (the octree) read
//! by all consumers. Today's `chunk_mesh_sparse.comp` reads
//! `SurfaceRecord[]` which is CPU-extracted from the dense Chunk
//! every meshing call. That's an extra pipeline stage on the CPU
//! per chunk, exactly the kind of derived-state duplication the
//! architecture wants to delete. v0.24.27 ports the meshing path
//! to consume octrees directly.
//!
//! ## Scope of this ship
//!
//! Foundation. Strict Camp 1 quads, no bumpiness. Open-boundary
//! neighbor lookups (out-of-chunk neighbors treated as air).
//! Visual output will *not* match the CPU mesher (which has
//! weathering-modulated corner displacement); it *will* match the
//! existing `chunk_mesh_sparse.comp` GPU mesher modulo chunk-
//! boundary double-emission.
//!
//! Activated via `UNENDLICH_GPU_MESHER=2`. The opt-in coexists
//! with `=1` (sparse mesher) and `=0` (CPU only). No production
//! user gets surprised by this ship.
//!
//! ## What follows (filed for next ships)
//!
//! - **Cross-chunk neighbor lookups.** Bind 7 octrees per dispatch
//!   (self + 6 neighbors). Eliminates the boundary double-emission.
//! - **Shared buffer with renderer.** Read directly from
//!   `OctreeBufferPool::buffer` instead of maintaining our own
//!   copy. Cuts memory in half. Requires changing constructor
//!   order in the bin so the renderer is built before the mesher.
//! - **4×4 sub-vertex grid (Ship B).** Add weathering instance
//!   buffer; emit 4×4 grids on faces where `weathering > 0`,
//!   displaced by `bumpy_height(world_x, world_z)`. 16× vertex
//!   inflation per emitted face.

#![cfg(feature = "render-core")]

use ash::{vk, Device, Instance};
use std::ffi::CStr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::render::context::VulkanContext;
use crate::render::mesher::ChunkMesh;
use crate::world::chunk::ChunkCoord;
use crate::world::octree::OctreeArray;

const ENTRY_NAME: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"main\0") };

/// Workgroup size in `chunk_mesh_octree.comp`. One thread per cell.
const WORKGROUP_SIZE: u32 = 64;

/// Cells per chunk = 32³ = 32768. Dispatched as 32768/64 = 512
/// workgroups along X, batch chunks along Y.
const CELLS_PER_CHUNK: u32 = 32 * 32 * 32;
const WORKGROUPS_PER_CHUNK_X: u32 = CELLS_PER_CHUNK / WORKGROUP_SIZE;
const _: () = assert!(CELLS_PER_CHUNK % WORKGROUP_SIZE == 0);

/// Per-chunk vertex slot capacity. Matches sparse mesher; large
/// enough that fully-dense surface chunks fit comfortably (typical
/// surface: 15-30K vertices; pathological worst case observed at
/// ~43K).
///
/// v0.24.NEXT+11-mesher-headroom: raised from 65_535 to 98_304 (1.5×).
/// 65_535 was a u16 ceiling from before indices became u32. With the
/// ceiling now arbitrary, sizing for ~98K verts gives generous headroom
/// over the observed 43K worst case. Cost at MAX_BATCH_CHUNKS=64:
/// +50 MB vertex buffer. Trivial on systems with multi-GB mesh pools.
pub const VERTEX_CAPACITY: u32 = 98_304;

/// Per-chunk index slot capacity.
///
/// v0.24.NEXT+9-overflow-fix: was 65_535 — set equal to VERTEX_CAPACITY,
/// which is wrong: indexed meshing emits ~1.5-3× as many indices as
/// vertices, so a chunk index-overflows long before it vertex-overflows.
/// Dense foliage chunks were observed needing i=117924 (with only
/// v=42112), tripping `Err(Overflow)` → an empty mesh → a visible hole.
/// Raised to 131_072 (covers the observed counts with headroom; indices
/// are read back as u32 so there is no 16-bit ceiling).
///
/// v0.24.NEXT+11-mesher-headroom: raised again from 131_072 to 262_144
/// after observing i=131856 in foliage-dense biomes (the 2026-05-28 run)
/// — pathological dense chunks can hit 3.08× index:vertex, so the
/// previous 131_072 (≈3× of old VERT_CAP=65_535) is now insufficient.
/// 262_144 = 2.67× of the new VERT_CAP=98_304; covers v=98K with ~3.08×
/// index ratio. Cost at MAX_BATCH_CHUNKS=64: +33 MB index buffer.
pub const INDEX_CAPACITY: u32 = 262_144;

/// Maximum chunks meshed per batch. Same as sparse for buffer
/// sizing consistency. The bin's existing
/// `UNENDLICH_GPU_MESH_BATCH` env var clamps the runtime value.
pub const MAX_BATCH_CHUNKS: u32 = 64;

/// Per-chunk octree slot capacity in nodes. The theoretical worst
/// case for a 32³ chunk at depth 5 is 4681 internal nodes + 32768
/// leaves = 37 449 nodes (when every cell is a unique block type,
/// preventing any homogeneous-subtree collapse). 40 960 covers
/// that with 8.4% margin.
///
/// History:
///   v0.24.27 initial: 8192 (guess based on "max observed ~5000").
///     Too low — real surface chunks regularly hit 8200-10800 nodes
///     and failed with `TooManyNodes`, producing empty meshes (and
///     thus terrain holes) in the world.
///   v0.24.27-fix1: 40960. Covers the theoretical worst case.
///   v0.24.30-shared-octree: the mesher no longer keeps its own
///     buffer; this constant is now ONLY a defensive pre-screen
///     ("reject pathologically large octrees before they reach the
///     pool"). The pool itself enforces its own capacity. The
///     constant stays at 40960 because that's the theoretical
///     worst case for a 32³ chunk at depth 5 (4681 internal + 32768
///     leaves = 37449, with margin).
pub const MAX_NODES_PER_CHUNK: u32 = 40_960;

/// Effective batch size from env var, clamped to MAX_BATCH_CHUNKS.
pub fn effective_batch_size() -> u32 {
    let max = MAX_BATCH_CHUNKS;
    match std::env::var("UNENDLICH_GPU_MESH_BATCH").ok().and_then(|s| s.parse::<u32>().ok()) {
        Some(n) if n >= 1 && n <= max => n,
        Some(n) if n > max => max,
        _ => max,
    }
}

/// One chunk's input. Takes an `Arc<OctreeArray>` for cheap cloning
/// (the same Arc is held by ChunkManager). The mesher only reads
/// `.nodes` from it.
pub struct OctreeBatchInput {
    pub coord: ChunkCoord,
    pub octree: Arc<OctreeArray>,
    /// v0.24.31-version-tracked: content version (from the streaming
    /// layer). Used by the pool's `upload_if_changed` to short-circuit
    /// redundant uploads when the same chunk has already been
    /// dispatched at this version. Defaults to 0 for callers that
    /// don't track versioning; in that case every dispatch will
    /// re-upload (matches v0.24.30 behavior).
    pub version: u64,
    /// v0.24.33-climate-tint: per-chunk climate values for shader-side
    /// color modulation. Both in [0, 1]. Defaults to (0.5, 0.5) when
    /// callers don't provide them — yields uniform unmodulated color
    /// (visual parity with v0.24.32).
    pub temperature: f32,
    pub humidity: f32,
}

/// One chunk's output. `mesh` is per-chunk Result so one chunk's
/// overflow doesn't sink the whole batch.
pub struct OctreeBatchOutput {
    pub coord: ChunkCoord,
    pub mesh: Result<ChunkMesh>,
}

#[derive(Debug)]
pub enum OctreeMeshError {
    Vulkan(vk::Result),
    ShaderMissing(String),
    Io(std::io::Error),
    Overflow { vertex_count: u32, index_count: u32 },
    TooManyNodes(usize),
    NoSuitableMemoryType,
}
impl From<vk::Result> for OctreeMeshError { fn from(r: vk::Result) -> Self { Self::Vulkan(r) } }
impl From<std::io::Error> for OctreeMeshError { fn from(e: std::io::Error) -> Self { Self::Io(e) } }
impl std::fmt::Display for OctreeMeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vulkan(r) => write!(f, "Vulkan error: {:?}", r),
            Self::ShaderMissing(p) => write!(f, "Shader missing: {}", p),
            Self::Io(e) => write!(f, "I/O error: {}", e),
            Self::Overflow { vertex_count, index_count } =>
                write!(f, "octree mesher overflow: v={}, i={}", vertex_count, index_count),
            Self::TooManyNodes(n) =>
                write!(f, "octree has too many nodes: {} (max {})", n, MAX_NODES_PER_CHUNK),
            Self::NoSuitableMemoryType => write!(f, "no host-visible memory type"),
        }
    }
}
impl std::error::Error for OctreeMeshError {}

pub type Result<T> = std::result::Result<T, OctreeMeshError>;

/// Per-chunk metadata SSBO entry. Must match the GLSL `ChunkMeta`
/// struct in `chunk_mesh_octree.comp` exactly.
#[repr(C)]
#[derive(Copy, Clone, Default)]
struct ChunkMetaGpu {
    octree_base: u32,
    vertex_base: u32,
    index_base: u32,
    counter_base: u32,
    chunk_origin_x: i32,
    chunk_origin_y: i32,
    chunk_origin_z: i32,
    _pad: u32,
    /// v0.24.30-cross-chunk: absolute node base index of the +X
    /// neighbor's octree in the shared pool, or `u32::MAX` if the
    /// neighbor isn't resident (boundary chunk; treat that face as
    /// open). Same pattern for the other five neighbors.
    ///
    /// With these in place, the mesher emits a face at a chunk
    /// boundary only when the cell on the other side of that
    /// boundary is genuinely non-solid — not just "we don't know
    /// because the neighbor isn't in this batch." Previously
    /// (open-boundary v0.24.27..v0.24.29) every chunk emitted
    /// faces at its outer surfaces regardless of whether the
    /// neighbor occluded them, producing visible z-fighting and
    /// doubled normals at seams.
    neighbor_octree_base_px: u32,
    neighbor_octree_base_nx: u32,
    neighbor_octree_base_py: u32,
    neighbor_octree_base_ny: u32,
    neighbor_octree_base_pz: u32,
    neighbor_octree_base_nz: u32,
    /// v0.24.33-climate-tint: per-chunk temperature and humidity in
    /// [0, 1]. Shader multiplies/mixes base block colors against
    /// these to give visible chunk-level climate variation (lush
    /// vs. dry grass, etc.). Computed once on the worker thread
    /// at the chunk's center via WorldModel.
    temperature: f32,
    humidity: f32,
}
// 8 (base meta) + 6 (neighbors) + 2 (climate floats) = 16 u32 = 64 bytes.
// std430 alignment: 4 bytes (largest scalar). 64 % 4 == 0. ✓
const _: [(); 64] = [(); std::mem::size_of::<ChunkMetaGpu>()];

/// Push constants for `chunk_mesh_octree.comp`. Must match the
/// GLSL `PC` block exactly.
#[repr(C)]
#[derive(Copy, Clone)]
struct PushConstants {
    vertex_capacity: u32,
    index_capacity: u32,
    batch_count: u32,
    /// v0.24.39-mid-lod: Mid LOD support. For Near, `lod_cell_count=32`
    /// and `lod_cell_stride=1` (per-block iteration). For Mid,
    /// `lod_cell_count=16` and `lod_cell_stride=2` (each cell
    /// represents a 2³ source-block group). Far/VeryFar (factor 4
    /// and 8) are not yet wired up but the shader handles arbitrary
    /// power-of-two combinations satisfying
    /// `lod_cell_count * lod_cell_stride == 32`.
    lod_cell_count: u32,
    lod_cell_stride: u32,
    _pad: u32,
}
const _: [(); 24] = [(); std::mem::size_of::<PushConstants>()];

/// Octree-driven GPU mesher. Mirrors `SparseGpuMesher` shape but
/// for octree input. Both mesher kinds can coexist on the same
/// VulkanContext; each owns its own buffers + pipeline.
///
/// v0.24.30-shared-octree: this mesher now reads octree data from
/// the renderer's `OctreeBufferPool` instead of maintaining its
/// own internal buffer. Saves ~100 MB; brings the mesher's octree
/// reads under the v0.24.26 probe's coverage; eliminates the
/// "unverified consumer buffer" class of bug that v0.24.27 spent
/// three sub-ships chasing.
///
/// The mesher's `mesh_chunks_one_submission` locks the pool for
/// the duration of upload + dispatch + readback (~1 ms per batch).
/// Main-thread per-chunk uploads (~10 µs each) contend on this
/// lock; expected wait is ~30 µs per upload at steady state.
/// Acceptable for foundation. A deferred-free pattern that lets
/// dispatch happen lock-free is filed as a follow-up.
pub struct OctreeGpuMesher {
    /// v0.24.30-shared-octree: handle to the renderer's pool.
    /// Used for upload + slot lookup during `mesh_chunks_*`. The
    /// pool's buffer is also bound at descriptor-set creation
    /// time (see `octree_pool_buffer` cached below for the
    /// stable handle).
    octree_pool: Arc<Mutex<crate::render::octree_buffer::OctreeBufferPool>>,
    /// Stable buffer handle captured at construction. The pool
    /// never reallocates its buffer over its lifetime, so this
    /// handle is valid as long as `octree_pool` is alive. We
    /// cache it here so the descriptor set wire-up at construction
    /// (and any future descriptor updates) can access it without
    /// locking.
    octree_pool_buffer: vk::Buffer,

    chunk_metas_buffer: vk::Buffer,
    chunk_metas_memory: vk::DeviceMemory,
    chunk_metas_ptr: *mut u8,

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
    pipeline_layout: vk::PipelineLayout,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,

    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    fence: vk::Fence,

    submit_lock: Mutex<()>,
    queue_submit_lock: Arc<Mutex<()>>,

    queue: vk::Queue,
    device: Device,
}

// SAFETY: persistent maps are HOST_COHERENT; all access via submit_lock.
unsafe impl Send for OctreeGpuMesher {}
unsafe impl Sync for OctreeGpuMesher {}

impl OctreeGpuMesher {
    /// v0.24.30-shared-octree: construct with a handle to the
    /// renderer's `OctreeBufferPool`. The pool's buffer is bound
    /// to the mesher's compute descriptor set; subsequent
    /// dispatches read from there. The mesher no longer owns
    /// its own octree storage.
    pub fn new(
        ctx: &VulkanContext,
        octree_pool: Arc<Mutex<crate::render::octree_buffer::OctreeBufferPool>>,
    ) -> Result<Self> {
        let device = ctx.device.clone();
        let queue = ctx.graphics_queue;
        let queue_submit_lock = ctx.queue_submit_lock.clone();

        // Capture the pool's buffer handle under a short lock.
        // The handle is stable for the pool's lifetime.
        let octree_pool_buffer = {
            let pool = octree_pool.lock().expect("octree_pool mutex poisoned at mesher init");
            pool.buffer
        };

        unsafe {
            // 1. Allocate per-batch buffers (no longer including
            //    the octree buffer — that comes from the shared
            //    pool now).

            let chunk_metas_bytes = (MAX_BATCH_CHUNKS as u64)
                * (std::mem::size_of::<ChunkMetaGpu>() as u64);
            let (chunk_metas_buffer, chunk_metas_memory, chunk_metas_ptr) =
                create_host_visible_storage(&device, &ctx.instance, ctx.physical_device, chunk_metas_bytes)?;

            let vertex_bytes = (MAX_BATCH_CHUNKS as u64) * (VERTEX_CAPACITY as u64) * 24;
            let (vertex_buffer, vertex_memory, vertex_ptr) =
                create_host_visible_storage(&device, &ctx.instance, ctx.physical_device, vertex_bytes)?;

            let index_bytes = (MAX_BATCH_CHUNKS as u64) * (INDEX_CAPACITY as u64) * 4;
            let (index_buffer, index_memory, index_ptr) =
                create_host_visible_storage(&device, &ctx.instance, ctx.physical_device, index_bytes)?;

            // 4 u32 per chunk slot.
            let counter_bytes = (MAX_BATCH_CHUNKS as u64) * 16;
            let (counter_buffer, counter_memory, counter_ptr) =
                create_host_visible_storage(&device, &ctx.instance, ctx.physical_device, counter_bytes)?;

            // 2. Pipeline.
            let descriptor_set_layout = create_descriptor_set_layout(&device)?;
            let pipeline_layout = create_pipeline_layout(&device, descriptor_set_layout)?;
            let pipeline = create_compute_pipeline(&device, pipeline_layout)?;

            // 3. Descriptor set. Octree binding points at the
            //    shared pool's buffer.
            let (descriptor_pool, descriptor_set) =
                allocate_descriptor_set(&device, descriptor_set_layout)?;
            update_descriptor_set(
                &device, descriptor_set,
                octree_pool_buffer, vertex_buffer, index_buffer, counter_buffer, chunk_metas_buffer,
            );

            // 4. Command pool + buffer + fence.
            let pool_info = vk::CommandPoolCreateInfo::builder()
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
                .queue_family_index(ctx.graphics_queue_family);
            let command_pool = device.create_command_pool(&pool_info, None)?;
            let alloc_info = vk::CommandBufferAllocateInfo::builder()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1);
            let command_buffer = device.allocate_command_buffers(&alloc_info)?[0];

            let fence_info = vk::FenceCreateInfo::builder();
            let fence = device.create_fence(&fence_info, None)?;

            log::info!(
                "OctreeGpuMesher initialized (shared-pool octree buffer, vert cap={}, idx cap={})",
                VERTEX_CAPACITY, INDEX_CAPACITY,
            );

            Ok(Self {
                octree_pool, octree_pool_buffer,
                chunk_metas_buffer, chunk_metas_memory, chunk_metas_ptr,
                vertex_buffer, vertex_memory, vertex_ptr,
                index_buffer, index_memory, index_ptr,
                counter_buffer, counter_memory, counter_ptr,
                pipeline, pipeline_layout, descriptor_set_layout,
                descriptor_pool, descriptor_set,
                command_pool, command_buffer, fence,
                submit_lock: Mutex::new(()),
                queue_submit_lock,
                queue, device,
            })
        }
    }

    /// Mesh up to `effective_batch_size()` chunks per submission.
    /// Auto-chunks if `chunks.len()` exceeds the cap.
    ///
    /// `lod_stride` is the LOD's source-block stride: 1 for Near
    /// (per-block iteration), 2 for Mid (each mesher cell represents
    /// a 2³ source-block group). All chunks in the batch must be at
    /// the same LOD. v0.24.39-mid-lod added the parameter; existing
    /// Near callers pass 1.
    pub fn mesh_chunks_batch(&self, chunks: &[OctreeBatchInput], lod_stride: u32) -> Result<Vec<OctreeBatchOutput>> {
        if chunks.is_empty() {
            return Ok(Vec::new());
        }
        debug_assert!(
            lod_stride == 1 || lod_stride == 2 || lod_stride == 4 || lod_stride == 8,
            "lod_stride must divide 32 and be a power of two; got {}", lod_stride
        );
        let mut all_outputs = Vec::with_capacity(chunks.len());
        for window in chunks.chunks(effective_batch_size() as usize) {
            let mut outs = self.mesh_chunks_one_submission(window, lod_stride)?;
            all_outputs.append(&mut outs);
        }
        Ok(all_outputs)
    }

    fn mesh_chunks_one_submission(
        &self, chunks: &[OctreeBatchInput], lod_stride: u32,
    ) -> Result<Vec<OctreeBatchOutput>> {
        debug_assert!(chunks.len() <= MAX_BATCH_CHUNKS as usize);

        // Pre-screen.
        let mut active_slots: Vec<usize> = Vec::with_capacity(chunks.len());
        let mut outputs: Vec<OctreeBatchOutput> = chunks.iter().map(|c| {
            let n = c.octree.nodes.len();
            if n > MAX_NODES_PER_CHUNK as usize {
                OctreeBatchOutput {
                    coord: c.coord,
                    mesh: Err(OctreeMeshError::TooManyNodes(n)),
                }
            } else if n == 0 {
                // Empty octree (shouldn't happen but handle anyway).
                OctreeBatchOutput {
                    coord: c.coord,
                    mesh: Ok(ChunkMesh::empty()),
                }
            } else {
                OctreeBatchOutput {
                    coord: c.coord,
                    mesh: Err(OctreeMeshError::Vulkan(vk::Result::NOT_READY)),
                }
            }
        }).collect();

        for (idx, c) in chunks.iter().enumerate() {
            let n = c.octree.nodes.len();
            if n > 0 && n <= MAX_NODES_PER_CHUNK as usize {
                active_slots.push(idx);
            }
        }
        if active_slots.is_empty() {
            return Ok(outputs);
        }

        let _guard = self.submit_lock.lock().unwrap();

        // v0.24.30-shared-octree: upload via the shared pool. The
        // pool will allocate (or reuse) a span per chunk and return
        // the absolute node base index. We capture indices into a
        // local vec, then carry them through the meta-write step.
        // The pool lock is HELD for the entire dispatch+readback
        // window so main-thread evictions can't reuse a span while
        // the GPU is mid-read.
        //
        // Lock duration: ~1 ms per batch. Main-thread upload waits
        // ~30 µs on average (see DESIGN_NOTES Increment 74 for the
        // contention analysis).
        let pool_guard = self.octree_pool.lock()
            .map_err(|_| OctreeMeshError::Vulkan(vk::Result::ERROR_UNKNOWN))?;

        // Per-active-slot: pool's base_node_index (u32::MAX = upload failed).
        let mut node_bases: Vec<u32> = Vec::with_capacity(active_slots.len());
        // v0.24.30-cross-chunk: per-active-slot, the 6 neighbor
        // octree bases in the pool (or u32::MAX if not resident).
        // Order: [+X, -X, +Y, -Y, +Z, -Z].
        let mut neighbor_bases: Vec<[u32; 6]> = Vec::with_capacity(active_slots.len());
        {
            let mut pool = pool_guard;
            for &slot_idx in &active_slots {
                let c = &chunks[slot_idx];
                // Reuse pool slot if present. Replacing on every
                // dispatch would waste allocator churn — the pool's
                // upload() handles replacement implicitly, but we
                // can avoid it when the chunk is already resident.
                //
                // Note: this is correct only because chunk octrees
                // are immutable for a given (coord, version). The
                // streaming layer re-uploads under a new version when
                // the chunk changes; v0.24.30-foundation does NOT
                // track version yet, so for now we upload every time
                // to guarantee freshness. The TODO is filed.
                match pool.upload_if_changed(c.coord, &c.octree, c.version) {
                    Ok(slot) => node_bases.push(slot.base_node_index()),
                    Err(e) => {
                        log::warn!(
                            "OctreeGpuMesher: pool upload failed for {:?}: {}; \
                             skipping this chunk in batch",
                            c.coord, e
                        );
                        // Mark this slot's output as failed and use a
                        // sentinel so positional indices line up.
                        outputs[slot_idx].mesh =
                            Err(OctreeMeshError::Vulkan(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY));
                        node_bases.push(u32::MAX);
                    }
                }

                // v0.24.30-cross-chunk: resolve the 6 neighbor
                // octree bases. A neighbor is "resident" if it has
                // an active slot in the pool — typically true for
                // RD-1 chunks (the chunks around our active set
                // are usually also loaded). When not resident, we
                // pass u32::MAX and the shader falls back to
                // open-boundary semantics for that face.
                //
                // The cost is 6 HashMap lookups per chunk. At 64
                // chunks per batch that's ~384 lookups — bounded
                // and cheap relative to the GPU work.
                let lookup = |dc: (i32, i32, i32)| -> u32 {
                    let nc = crate::world::chunk::ChunkCoord::new(
                        c.coord.x + dc.0, c.coord.y + dc.1, c.coord.z + dc.2,
                    );
                    pool.slot(nc).map(|s| s.base_node_index()).unwrap_or(u32::MAX)
                };
                neighbor_bases.push([
                    lookup(( 1,  0,  0)),  // +X
                    lookup((-1,  0,  0)),  // -X
                    lookup(( 0,  1,  0)),  // +Y
                    lookup(( 0, -1,  0)),  // -Y
                    lookup(( 0,  0,  1)),  // +Z
                    lookup(( 0,  0, -1)),  // -Z
                ]);
            }

            // Pool guard `pool` stays live through the dispatch call
            // below so the GPU's reads of the buffer aren't
            // invalidated by a concurrent eviction.
            self.dispatch_and_wait(
                &active_slots, chunks, &node_bases, &neighbor_bases,
                lod_stride, &mut outputs,
            )?;

            // v0.24.NEXT+3-pool-early-unlock: at this point the fence has
            // signalled, so the GPU has finished reading the octree pool
            // for this batch (the submitted command buffer contained only
            // this dispatch). The readback below reads ONLY the mesher's
            // own output buffers (counter/vertex/index), never the pool.
            // So it is safe to release the pool lock now, before the
            // readback memcpy — which lets main-thread octree uploads
            // (~10 µs each) proceed during the readback instead of
            // queueing behind it. Shortens the contended window by the
            // readback duration; biggest effect during chunk-storm bursts
            // where many uploads stack up.
            //
            // Gated (default off) so the default pipeline is byte-for-byte
            // unchanged until A/B'd on hardware: the safety argument is
            // sound (and matches the long-standing comment that eviction
            // is safe "after this point"), but releasing a lock earlier
            // around GPU memory is exactly the kind of change that wants
            // a hardware confirmation before becoming default.
            let early_unlock = std::env::var("UNENDLICH_GPU_POOL_EARLY_UNLOCK")
                .map(|v| v != "0" && !v.is_empty()).unwrap_or(false);
            if early_unlock {
                drop(pool);
            }
            self.readback_outputs(&active_slots, &mut outputs)?;
            // If !early_unlock, `pool` drops here at end-of-scope (after
            // the readback) — the original, more conservative behaviour.
        }

        Ok(outputs)
    }

    /// v0.24.30-shared-octree: factored out so the pool lock holds
    /// over upload + dispatch as a single critical section. Operates on
    /// caller-provided `node_bases` (one per active slot). Skips chunks
    /// whose `node_bases[i] == u32::MAX` (pool-upload failures recorded
    /// into `outputs[i]` already).
    ///
    /// v0.24.NEXT+3: renamed from `dispatch_and_readback` — this now
    /// ends at the fence wait (`record_submit_wait`); readback is a
    /// separate `readback_outputs` call so the caller can drop the pool
    /// lock before reading back (the GPU is done with the octree pool
    /// at fence-signal).
    ///
    /// v0.24.30-cross-chunk: `neighbor_bases[i]` carries the 6
    /// neighbor octree bases for active slot `i`, in order
    /// `[+X, -X, +Y, -Y, +Z, -Z]`. `u32::MAX` means "neighbor not
    /// resident in pool — shader falls back to open-boundary."
    fn dispatch_and_wait(
        &self,
        active_slots: &[usize],
        chunks: &[OctreeBatchInput],
        node_bases: &[u32],
        neighbor_bases: &[[u32; 6]],
        lod_stride: u32,
        outputs: &mut [OctreeBatchOutput],
    ) -> Result<()> {
        debug_assert_eq!(active_slots.len(), node_bases.len());
        debug_assert_eq!(active_slots.len(), neighbor_bases.len());

        unsafe {
            for (pos, &slot_idx) in active_slots.iter().enumerate() {
                let octree_base = node_bases[pos];
                if octree_base == u32::MAX {
                    // Pool upload failed for this slot — skip the
                    // meta write so the GPU effectively does nothing
                    // for it. The matching `outputs[slot_idx]` is
                    // already marked Err.
                    continue;
                }
                let c = &chunks[slot_idx];
                let nb = &neighbor_bases[pos];

                // ChunkMeta → chunk_metas_buffer. octree_base is the
                // pool's absolute node index for this chunk; the
                // shader walks the octree starting there. Neighbor
                // bases let the shader sample across chunk boundaries
                // instead of treating them as open.
                let origin = c.coord.block_origin();
                let meta = ChunkMetaGpu {
                    octree_base,
                    vertex_base: (slot_idx as u32) * VERTEX_CAPACITY,
                    index_base:  (slot_idx as u32) * INDEX_CAPACITY,
                    counter_base: (slot_idx as u32) * 4,
                    chunk_origin_x: origin.0,
                    chunk_origin_y: origin.1,
                    chunk_origin_z: origin.2,
                    _pad: 0,
                    neighbor_octree_base_px: nb[0],
                    neighbor_octree_base_nx: nb[1],
                    neighbor_octree_base_py: nb[2],
                    neighbor_octree_base_ny: nb[3],
                    neighbor_octree_base_pz: nb[4],
                    neighbor_octree_base_nz: nb[5],
                    temperature: c.temperature,
                    humidity: c.humidity,
                };
                let meta_dst_offset = (slot_idx as u64)
                    * (std::mem::size_of::<ChunkMetaGpu>() as u64);
                let meta_dst = self.chunk_metas_ptr.add(meta_dst_offset as usize);
                std::ptr::copy_nonoverlapping(
                    &meta as *const _ as *const u8,
                    meta_dst,
                    std::mem::size_of::<ChunkMetaGpu>(),
                );
            }
        }
        self.record_submit_wait(active_slots, chunks, lod_stride, outputs)?;
        Ok(())
    }

    /// Records the compute dispatch + submits + waits on the fence.
    /// Caller must hold the pool lock for the duration of this call (so
    /// the GPU's reads of the pool buffer aren't invalidated by a
    /// concurrent eviction). Readback is **not** done here — see
    /// `readback_outputs`.
    fn record_submit_wait(
        &self,
        active_slots: &[usize],
        chunks: &[OctreeBatchInput],
        lod_stride: u32,
        outputs: &mut [OctreeBatchOutput],
    ) -> Result<()> {
        // 2. Record command buffer.
        let counter_bytes_total = (MAX_BATCH_CHUNKS as u64) * 16;
        unsafe {
            self.device.reset_command_buffer(
                self.command_buffer,
                vk::CommandBufferResetFlags::empty(),
            )?;
            let begin = vk::CommandBufferBeginInfo::builder()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            self.device.begin_command_buffer(self.command_buffer, &begin)?;

            // Zero all counters for the active slots (cheap — 256 B total).
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
                &[], &[counter_barrier], &[],
            );

            // Bind pipeline + descriptor set.
            self.device.cmd_bind_pipeline(
                self.command_buffer,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline,
            );
            self.device.cmd_bind_descriptor_sets(
                self.command_buffer,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_layout, 0,
                std::slice::from_ref(&self.descriptor_set), &[],
            );

            // ONE dispatch covering the whole batch. The shader's
            // chunk_idx = gl_WorkGroupID.y selects the slot from
            // chunk_metas; active_slots are the slots we filled.
            // Inactive slots will have stale counters (we zeroed
            // them) and zero output — they're harmless because we
            // only read back the active ones.
            // v0.24.39-mid-lod: derive cell count from stride. For
            // Near stride=1, cell_count=32, ³ = 32768 cells, workgroups
            // = 32768/64 = 512. For Mid stride=2, cell_count=16, ³
            // = 4096 cells, workgroups = 64.
            let lod_cell_count = 32 / lod_stride;
            let cells_per_chunk = lod_cell_count * lod_cell_count * lod_cell_count;
            let workgroups_x = (cells_per_chunk + WORKGROUP_SIZE - 1) / WORKGROUP_SIZE;
            let push = PushConstants {
                vertex_capacity: VERTEX_CAPACITY,
                index_capacity: INDEX_CAPACITY,
                batch_count: chunks.len() as u32,
                lod_cell_count,
                lod_cell_stride: lod_stride,
                _pad: 0,
            };
            let push_bytes = std::slice::from_raw_parts(
                &push as *const _ as *const u8,
                std::mem::size_of::<PushConstants>(),
            );
            self.device.cmd_push_constants(
                self.command_buffer, self.pipeline_layout,
                vk::ShaderStageFlags::COMPUTE, 0, push_bytes,
            );
            self.device.cmd_dispatch(
                self.command_buffer,
                workgroups_x, chunks.len() as u32, 1,
            );

            // Barrier: shader writes → host reads on the output
            // buffers (single barrier per buffer for clarity).
            let buffer_barriers = [
                buf_barrier(self.vertex_buffer),
                buf_barrier(self.index_buffer),
                buf_barrier(self.counter_buffer),
            ];
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[], &buffer_barriers, &[],
            );

            self.device.end_command_buffer(self.command_buffer)?;

            // Submit + wait.
            self.device.reset_fences(&[self.fence])?;
            let submit = vk::SubmitInfo::builder()
                .command_buffers(std::slice::from_ref(&self.command_buffer))
                .build();
            {
                let _qg = self.queue_submit_lock.lock()
                    .expect("octree mesher queue submit lock poisoned");
                self.device.queue_submit(self.queue, &[submit], self.fence)?;
            }
            self.device.wait_for_fences(&[self.fence], true, u64::MAX)?;
        }
        Ok(())
    }

    /// Reads the GPU mesh output (counters + vertex/index data) for each
    /// active slot into `outputs`. Reads **only** the mesher's own
    /// HOST_COHERENT output buffers (counter/vertex/index) — never the
    /// octree pool — so the caller does NOT need to hold the octree-pool
    /// lock for this. The GPU finished reading the octree pool at
    /// fence-signal inside `record_submit_wait`, so by the time we get
    /// here the pool can already have been released (see the
    /// `UNENDLICH_GPU_POOL_EARLY_UNLOCK` path in `mesh_chunks_batch`).
    /// Split out of the former `record_and_submit` in v0.24.NEXT+3 so
    /// that early-unlock is possible.
    fn readback_outputs(
        &self,
        active_slots: &[usize],
        outputs: &mut [OctreeBatchOutput],
    ) -> Result<()> {
        // 3. Readback per active slot.
        //
        // v0.24.30-shared-octree: skip slots whose pool upload
        // failed earlier. Those slots have a specific upload-failure
        // error (`Vulkan(ERROR_OUT_OF_DEVICE_MEMORY)`) set in
        // outputs, and no valid meta was written to GPU, so the
        // counter buffer contains stale data. Reading it would
        // overwrite the Err with a meaningless Ok.
        //
        // Active slots (the normal case) start with the placeholder
        // `Err(NOT_READY)` which means "needs readback"; that gets
        // overwritten by the successful `Ok(ChunkMesh {...})` below.
        for &slot_idx in active_slots {
            if matches!(
                outputs[slot_idx].mesh,
                Err(OctreeMeshError::Vulkan(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY))
            ) {
                continue;
            }
            let counter_offset = (slot_idx as u32) * 4;
            let (vc, ic, overflow) = unsafe {
                let p = self.counter_ptr as *const u32;
                (
                    *p.add(counter_offset as usize + 0),
                    *p.add(counter_offset as usize + 1),
                    *p.add(counter_offset as usize + 2),
                )
            };

            if overflow != 0 || vc > VERTEX_CAPACITY || ic > INDEX_CAPACITY {
                outputs[slot_idx].mesh = Err(OctreeMeshError::Overflow {
                    vertex_count: vc, index_count: ic,
                });
                continue;
            }

            // v0.24.44-readback-bulk: bulk memcpy via slice::to_vec
            // instead of per-vertex push. `Vertex` is `repr(C)` of
            // `[f32; 6]` so the layout matches the shader's emit
            // (3 floats pos, 3 floats color, no padding). The previous
            // per-element push loop did 1 array construction + 1
            // Vertex::new call + 1 vec push per vertex; for the
            // worst-case 44K-vertex chunk that's 132K function
            // invocations, even though the data is already in the
            // right shape. Single `to_vec` is a memcpy + alloc.
            //
            // Same treatment for indices below.
            let v_base_floats = (slot_idx as usize) * (VERTEX_CAPACITY as usize) * 6;
            let vertex_src: &[crate::render::mesher::Vertex] = unsafe {
                std::slice::from_raw_parts(
                    (self.vertex_ptr as *const f32).add(v_base_floats)
                        as *const crate::render::mesher::Vertex,
                    vc as usize,
                )
            };
            let vertices = vertex_src.to_vec();

            let i_base = (slot_idx as usize) * (INDEX_CAPACITY as usize);
            let index_src: &[u32] = unsafe {
                std::slice::from_raw_parts(
                    (self.index_ptr as *const u32).add(i_base),
                    ic as usize,
                )
            };
            let indices = index_src.to_vec();

            outputs[slot_idx].mesh = Ok(ChunkMesh { vertices, indices });
        }

        Ok(())
    }
}

fn buf_barrier(buf: vk::Buffer) -> vk::BufferMemoryBarrier {
    vk::BufferMemoryBarrier::builder()
        .src_access_mask(vk::AccessFlags::SHADER_WRITE)
        .dst_access_mask(vk::AccessFlags::HOST_READ)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .buffer(buf)
        .offset(0).size(vk::WHOLE_SIZE)
        .build()
}

impl Drop for OctreeGpuMesher {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_fence(self.fence, None);
            self.device.free_command_buffers(self.command_pool, &[self.command_buffer]);
            self.device.destroy_command_pool(self.command_pool, None);

            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.pipeline_layout, None);
            self.device.destroy_descriptor_pool(self.descriptor_pool, None);
            self.device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);

            // v0.24.30-shared-octree: the octree buffer is owned by
            // the renderer's `OctreeBufferPool` (via Arc<Mutex>).
            // The Arc clone we hold is dropped automatically when
            // `OctreeGpuMesher` is dropped; if it's the last clone,
            // the pool's own Drop runs. We must NOT destroy the
            // pool's buffer here — that's the pool's responsibility.

            self.device.unmap_memory(self.chunk_metas_memory);
            self.device.destroy_buffer(self.chunk_metas_buffer, None);
            self.device.free_memory(self.chunk_metas_memory, None);

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

unsafe fn create_descriptor_set_layout(
    device: &Device,
) -> Result<vk::DescriptorSetLayout> {
    let bindings = [
        // 0: octree nodes
        vk::DescriptorSetLayoutBinding::builder()
            .binding(0).descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1).stage_flags(vk::ShaderStageFlags::COMPUTE).build(),
        // 1: vertices
        vk::DescriptorSetLayoutBinding::builder()
            .binding(1).descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1).stage_flags(vk::ShaderStageFlags::COMPUTE).build(),
        // 2: indices
        vk::DescriptorSetLayoutBinding::builder()
            .binding(2).descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1).stage_flags(vk::ShaderStageFlags::COMPUTE).build(),
        // 3: counters
        vk::DescriptorSetLayoutBinding::builder()
            .binding(3).descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1).stage_flags(vk::ShaderStageFlags::COMPUTE).build(),
        // 4: chunk_metas
        vk::DescriptorSetLayoutBinding::builder()
            .binding(4).descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1).stage_flags(vk::ShaderStageFlags::COMPUTE).build(),
    ];
    let info = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&bindings);
    Ok(device.create_descriptor_set_layout(&info, None)?)
}

unsafe fn create_pipeline_layout(
    device: &Device, set_layout: vk::DescriptorSetLayout,
) -> Result<vk::PipelineLayout> {
    let pcr = vk::PushConstantRange::builder()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0).size(std::mem::size_of::<PushConstants>() as u32);
    let info = vk::PipelineLayoutCreateInfo::builder()
        .set_layouts(std::slice::from_ref(&set_layout))
        .push_constant_ranges(std::slice::from_ref(&pcr));
    Ok(device.create_pipeline_layout(&info, None)?)
}

unsafe fn create_compute_pipeline(
    device: &Device, layout: vk::PipelineLayout,
) -> Result<vk::Pipeline> {
    let candidates = [
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/shaders/chunk_mesh_octree.comp.spv"),
        "assets/shaders/chunk_mesh_octree.comp.spv",
        "src/shaders/chunk_mesh_octree.comp.spv",
    ];
    let mut bytes: Option<Vec<u8>> = None;
    for p in &candidates {
        if Path::new(p).exists() {
            bytes = Some(std::fs::read(p)?);
            break;
        }
    }
    let bytes = bytes.ok_or_else(|| OctreeMeshError::ShaderMissing(candidates[0].to_string()))?;
    let words: Vec<u32> = bytes.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    let module_info = vk::ShaderModuleCreateInfo::builder().code(&words);
    let module = device.create_shader_module(&module_info, None)?;

    let stage = vk::PipelineShaderStageCreateInfo::builder()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module).name(ENTRY_NAME);
    let info = vk::ComputePipelineCreateInfo::builder()
        .stage(stage.build()).layout(layout);
    let pipelines = device.create_compute_pipelines(
        vk::PipelineCache::null(), &[info.build()], None,
    ).map_err(|(_, r)| OctreeMeshError::Vulkan(r))?;
    device.destroy_shader_module(module, None);
    Ok(pipelines[0])
}

unsafe fn allocate_descriptor_set(
    device: &Device, layout: vk::DescriptorSetLayout,
) -> Result<(vk::DescriptorPool, vk::DescriptorSet)> {
    let sizes = [vk::DescriptorPoolSize {
        ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 5,
    }];
    let info = vk::DescriptorPoolCreateInfo::builder()
        .pool_sizes(&sizes).max_sets(1);
    let pool = device.create_descriptor_pool(&info, None)?;
    let alloc = vk::DescriptorSetAllocateInfo::builder()
        .descriptor_pool(pool).set_layouts(std::slice::from_ref(&layout));
    let sets = device.allocate_descriptor_sets(&alloc)?;
    Ok((pool, sets[0]))
}

unsafe fn update_descriptor_set(
    device: &Device, set: vk::DescriptorSet,
    octree: vk::Buffer, vertex: vk::Buffer, index: vk::Buffer,
    counter: vk::Buffer, metas: vk::Buffer,
) {
    let o = vk::DescriptorBufferInfo::builder().buffer(octree).range(vk::WHOLE_SIZE).build();
    let v = vk::DescriptorBufferInfo::builder().buffer(vertex).range(vk::WHOLE_SIZE).build();
    let i = vk::DescriptorBufferInfo::builder().buffer(index).range(vk::WHOLE_SIZE).build();
    let c = vk::DescriptorBufferInfo::builder().buffer(counter).range(vk::WHOLE_SIZE).build();
    let m = vk::DescriptorBufferInfo::builder().buffer(metas).range(vk::WHOLE_SIZE).build();
    let writes = [
        vk::WriteDescriptorSet::builder().dst_set(set).dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&o)).build(),
        vk::WriteDescriptorSet::builder().dst_set(set).dst_binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&v)).build(),
        vk::WriteDescriptorSet::builder().dst_set(set).dst_binding(2)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&i)).build(),
        vk::WriteDescriptorSet::builder().dst_set(set).dst_binding(3)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&c)).build(),
        vk::WriteDescriptorSet::builder().dst_set(set).dst_binding(4)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&m)).build(),
    ];
    device.update_descriptor_sets(&writes, &[]);
}

unsafe fn create_host_visible_storage(
    device: &Device,
    instance: &Instance,
    physical_device: vk::PhysicalDevice,
    size: u64,
) -> Result<(vk::Buffer, vk::DeviceMemory, *mut u8)> {
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
    let mt = chosen.ok_or(OctreeMeshError::NoSuitableMemoryType)?;

    let alloc = vk::MemoryAllocateInfo::builder()
        .allocation_size(req.size).memory_type_index(mt);
    let memory = device.allocate_memory(&alloc, None)?;
    device.bind_buffer_memory(buffer, memory, 0)?;
    let ptr = device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8;
    Ok((buffer, memory, ptr))
}
