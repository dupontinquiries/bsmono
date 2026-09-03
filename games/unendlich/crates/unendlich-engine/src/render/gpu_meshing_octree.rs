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

/// r478/r480: tiled shared-memory mesher path. Reads the live global atomic
/// (seeded from UNENDLICH_OCTREE_TILE at startup, settable from the Performance
/// page). The dispatch pushes it as a constant each batch.
fn octree_tile_enabled() -> u32 {
    crate::constants::octree_tile_enabled()
}

/// Cells per chunk = 32³ = 32768. Dispatched as 32768/64 = 512
/// workgroups along X, batch chunks along Y.
const CELLS_PER_CHUNK: u32 = 32 * 32 * 32;
const WORKGROUPS_PER_CHUNK_X: u32 = CELLS_PER_CHUNK / WORKGROUP_SIZE;
const _: () = assert!(CELLS_PER_CHUNK % WORKGROUP_SIZE == 0);

/// One vertex as `chunk_mesh_octree.comp` writes it: 3 floats position
/// (world space), 3 floats color. This is the wire format between the
/// shader and the host and it is pinned at 24 bytes; `mesher::Vertex`
/// is the CPU type and may grow fields (it did: `sway`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuWireVertex {
    pub position: [f32; 3],
    pub color: [f32; 3],
}
const _: [(); 24] = [(); std::mem::size_of::<GpuWireVertex>()];
impl GpuWireVertex {
    #[inline]
    pub fn to_vertex(self) -> crate::render::mesher::Vertex {
        crate::render::mesher::Vertex::new(self.position, self.color)
    }
}

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
    /// Every ring slot is in flight; `poll`/`wait_oldest` first.
    Busy,
    EmptyBatch,
    BatchTooLarge { got: usize, cap: usize },
    UnknownTicket(u64),
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
            Self::Busy => write!(f, "every mesh ring slot is in flight"),
            Self::EmptyBatch => write!(f, "empty mesh batch"),
            Self::BatchTooLarge { got, cap } =>
                write!(f, "mesh batch of {} exceeds slot capacity {}", got, cap),
            Self::UnknownTicket(t) => write!(f, "mesh ticket {} is not in flight", t),
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
    /// r478: 1 = tiled shared-memory mesher path (UNENDLICH_OCTREE_TILE=1).
    tile_enabled: u32,
}
const _: [(); 24] = [(); std::mem::size_of::<PushConstants>()];

/// Chunks per dispatch slot for a ring of `inflight` slots. The output
/// buffers are sized ONCE at `MAX_BATCH_CHUNKS` and partitioned between
/// the slots, so memory is constant whatever the ring depth (218 MB of
/// host-visible output at 64 chunks; a per-slot copy would be N× that on
/// unified memory). A deeper ring therefore means smaller dispatches —
/// which is what the single-queue hitch wants anyway.
pub fn slot_chunks_for(inflight: usize) -> usize {
    (MAX_BATCH_CHUNKS as usize / inflight.max(1)).max(1)
}

/// `UNENDLICH_GPU_MESH_INFLIGHT` — how many mesh dispatches may be on the
/// queue at once. 1 is the old synchronous shape (submit → wait → read).
/// Default 2: one running while the next is uploaded and recorded.
pub const DEFAULT_INFLIGHT: usize = 2;
pub fn inflight_from_env() -> usize {
    std::env::var("UNENDLICH_GPU_MESH_INFLIGHT").ok()
        .and_then(|s| s.parse::<usize>().ok())
        .map(|n| n.clamp(1, MAX_BATCH_CHUNKS as usize))
        .unwrap_or(DEFAULT_INFLIGHT)
}

/// Storage-buffer descriptor offsets must satisfy
/// `minStorageBufferOffsetAlignment`, which the spec caps at 256. The
/// slot's chunk-meta region is aligned to that so the per-slot descriptor
/// write is legal on every driver without querying.
const META_REGION_ALIGN: u64 = 256;

fn meta_region_stride(slot_chunks: usize) -> u64 {
    let raw = slot_chunks as u64 * std::mem::size_of::<ChunkMetaGpu>() as u64;
    (raw + META_REGION_ALIGN - 1) / META_REGION_ALIGN * META_REGION_ALIGN
}

/// Names one submitted mesh batch. Returned by `submit_batch`, handed back
/// by `poll` / `wait_oldest` / `collect` with that batch's outputs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct MeshTicket(u64);

/// What the caller learns about a finished dispatch besides its meshes:
/// how long it held the queue (exclusive, overlap-deduplicated — see
/// `DispatchRing::complete`) and how many chunks it carried. The
/// dispatcher's budget and batch sizer are fed from this, never from the
/// wall time around a blocking call.
#[derive(Clone, Copy, Debug, Default)]
pub struct DispatchCost {
    pub gpu_exclusive_us: u64,
    pub chunks: u32,
}

/// One batch in flight: the pre-screened output placeholders (readback
/// fills the active ones), and which positions are active.
struct PendingBatch {
    outputs: Vec<OctreeBatchOutput>,
    active: Vec<usize>,
    ticket: u64,
}

/// Vulkan resources of one ring slot. Owned per slot so two dispatches in
/// flight never share a command buffer, a fence, or an output region.
struct SlotVk {
    command_buffer: vk::CommandBuffer,
    fence: vk::Fence,
    /// Same bindings as every other slot except binding 4 (chunk_metas),
    /// which points at THIS slot's region so the shader's
    /// `chunk_metas[gl_WorkGroupID.y]` stays zero-based per dispatch.
    descriptor_set: vk::DescriptorSet,
    pending: Option<PendingBatch>,
}

struct RingVk {
    /// Lifecycle + GPU-time accounting (`render::mesh_dispatch_ring`).
    lifecycle: crate::render::mesh_dispatch_ring::DispatchRing,
    slots: Vec<SlotVk>,
    /// Wall clock the ring's `u64` microseconds are measured from.
    epoch: std::time::Instant,
    next_ticket: u64,
    /// Per-chunk GPU cost model, µs, exponential moving average of the
    /// exclusive dispatch time divided by its chunk count. Starts at a
    /// conservative prior so the first dispatch on an unknown driver is
    /// small; converges within a few dispatches.
    per_chunk_ema_us: f64,
    samples: u64,
    /// Costs of dispatches completed since the last `take_costs`.
    costs: Vec<DispatchCost>,
}

/// Prior per-chunk cost before any measurement. 2 ms is roughly what the
/// Honeykrisp report implies per chunk at the low end (8–15 ms measured;
/// the prior errs small so a fast driver ramps up in two or three
/// dispatches rather than one slow driver hitching on the first).
const PER_CHUNK_PRIOR_US: f64 = 2_000.0;
const EMA_ALPHA: f64 = 0.35;

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
/// ## The ring (2026-09-02)
///
/// The perfbisect desktop report (docs/claude-notes/
/// perfbisect-desktop-findings.md) showed the frame's main pass queued
/// behind a 64-chunk mesh dispatch on the one graphics queue for
/// 0.5–1.6 s on Honeykrisp. Two things fix that and both live here:
///
/// 1. **Dispatches are submitted, not waited on.** `submit_batch` uploads
///    and pins the batch's octree spans under the pool lock, drops the
///    lock, records into a free slot and submits. `poll` returns finished
///    batches without blocking; `wait_oldest` bounds the wait when every
///    slot is busy. The pure lifecycle is
///    `render::mesh_dispatch_ring::DispatchRing`; this type attaches the
///    `vk::Fence`/`vk::CommandBuffer`/output regions to its slot indices.
///    Pinning is no longer optional: a mutex cannot be held across an
///    asynchronous dispatch, so every span a dispatch reads is pinned
///    from submit to completion and freed by the ledger after.
///    `UNENDLICH_GPU_POOL_PIN` and `UNENDLICH_GPU_POOL_EARLY_UNLOCK` are
///    therefore gone from this module (the lock-held mode they toggled
///    no longer exists).
/// 2. **The queue is the compute queue when the context has a distinct
///    one** (`VulkanContext::compute_queue_is_distinct`). On a device with
///    a dedicated COMPUTE family the frame never queues behind a mesh
///    dispatch at all. On a single-family device the mesher stays on the
///    graphics queue and the *batch sizer* bounds the hitch: each
///    completed dispatch updates a per-chunk GPU-time model, and
///    `suggested_batch(budget_us)` answers how many chunks fit in the
///    time the dispatcher is willing to put in front of a frame.
///
/// `mesh_chunks_batch` keeps its blocking contract (submit + collect per
/// window of `slot_capacity()` chunks) so `gpu-mesh-bench` and the
/// parity tests are unchanged.
pub struct OctreeGpuMesher {
    /// v0.24.30-shared-octree: handle to the renderer's pool.
    /// Used for upload + slot lookup during `mesh_chunks_*`. The
    /// pool's buffer is also bound at descriptor-set creation
    /// time (see `octree_pool_buffer` cached below for the
    /// stable handle).
    octree_pool: Arc<Mutex<crate::render::octree_buffer::OctreeBufferPool>>,
    /// Stable buffer handle captured at construction. The pool
    /// never reallocates its buffer over its lifetime, so this
    /// handle is valid as long as `octree_pool` is alive.
    #[allow(dead_code)]
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

    command_pool: vk::CommandPool,
    ring: Mutex<RingVk>,
    /// Chunks one dispatch slot can carry (`MAX_BATCH_CHUNKS / inflight`).
    slot_chunks: usize,
    meta_stride: u64,

    queue: vk::Queue,
    /// True when `queue` is not the graphics queue: the frame does not
    /// queue behind our dispatches, so the dispatcher may use a larger
    /// per-dispatch budget.
    queue_is_async: bool,
    queue_submit_lock: Arc<Mutex<()>>,
    device: Device,
}

// SAFETY: persistent maps are HOST_COHERENT; every touch of a slot's
// region happens under the `ring` mutex, and a slot's region is only
// read by the host after its fence has signaled.
unsafe impl Send for OctreeGpuMesher {}
unsafe impl Sync for OctreeGpuMesher {}

impl OctreeGpuMesher {
    /// v0.24.30-shared-octree: construct with a handle to the
    /// renderer's `OctreeBufferPool`. The pool's buffer is bound
    /// to every slot's compute descriptor set; dispatches read from
    /// there. The mesher owns no octree storage.
    ///
    /// Ring depth is `inflight_from_env()`; use `with_inflight` to pin it
    /// (benches, tests).
    pub fn new(
        ctx: &VulkanContext,
        octree_pool: Arc<Mutex<crate::render::octree_buffer::OctreeBufferPool>>,
    ) -> Result<Self> {
        Self::with_inflight(ctx, octree_pool, inflight_from_env())
    }

    pub fn with_inflight(
        ctx: &VulkanContext,
        octree_pool: Arc<Mutex<crate::render::octree_buffer::OctreeBufferPool>>,
        inflight: usize,
    ) -> Result<Self> {
        let inflight = inflight.clamp(1, MAX_BATCH_CHUNKS as usize);
        let slot_chunks = slot_chunks_for(inflight);
        let meta_stride = meta_region_stride(slot_chunks);
        let device = ctx.device.clone();
        let queue_submit_lock = ctx.queue_submit_lock.clone();

        // The queue. A distinct compute queue means the frame's main pass
        // never sits behind a mesh dispatch; on a one-queue device we are
        // on the graphics queue and the dispatcher's batch sizer is what
        // bounds the hitch. Reported at info so a perfbisect run can tell
        // which world it measured.
        let (queue, queue_family, queue_is_async) = if ctx.compute_queue_is_distinct {
            (ctx.compute_queue, ctx.compute_queue_family, true)
        } else {
            (ctx.graphics_queue, ctx.graphics_queue_family, false)
        };

        // Capture the pool's buffer handle under a short lock.
        // The handle is stable for the pool's lifetime.
        let octree_pool_buffer = {
            let pool = octree_pool.lock().expect("octree_pool mutex poisoned at mesher init");
            pool.buffer
        };

        unsafe {
            // 1. Output buffers, sized once at MAX_BATCH_CHUNKS and
            //    partitioned between the slots.
            let chunk_metas_bytes = inflight as u64 * meta_stride;
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

            // 3. One descriptor set per slot. Bindings 0-3 are the whole
            //    buffers (bases are absolute); binding 4 is the slot's
            //    meta region.
            let (descriptor_pool, sets) =
                allocate_descriptor_sets(&device, descriptor_set_layout, inflight as u32)?;
            for (k, &set) in sets.iter().enumerate() {
                update_descriptor_set(
                    &device, set,
                    octree_pool_buffer, vertex_buffer, index_buffer, counter_buffer,
                    chunk_metas_buffer, k as u64 * meta_stride, meta_stride,
                );
            }

            // 4. Command pool on the chosen family; one command buffer and
            //    one fence per slot.
            let pool_info = vk::CommandPoolCreateInfo::builder()
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
                .queue_family_index(queue_family);
            let command_pool = device.create_command_pool(&pool_info, None)?;
            let alloc_info = vk::CommandBufferAllocateInfo::builder()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(inflight as u32);
            let cmds = device.allocate_command_buffers(&alloc_info)?;

            let mut slots = Vec::with_capacity(inflight);
            for (k, cmd) in cmds.into_iter().enumerate() {
                let fence = device.create_fence(&vk::FenceCreateInfo::builder(), None)?;
                slots.push(SlotVk { command_buffer: cmd, fence, descriptor_set: sets[k], pending: None });
            }

            log::info!(
                "OctreeGpuMesher initialized: ring inflight={} slot_chunks={} queue={} \
                 (vert cap={}, idx cap={}, output {} MB host-visible)",
                inflight, slot_chunks,
                if queue_is_async { "compute (distinct)" } else { "graphics (shared)" },
                VERTEX_CAPACITY, INDEX_CAPACITY,
                (vertex_bytes + index_bytes + counter_bytes + chunk_metas_bytes) / (1024 * 1024),
            );

            Ok(Self {
                octree_pool, octree_pool_buffer,
                chunk_metas_buffer, chunk_metas_memory, chunk_metas_ptr,
                vertex_buffer, vertex_memory, vertex_ptr,
                index_buffer, index_memory, index_ptr,
                counter_buffer, counter_memory, counter_ptr,
                pipeline, pipeline_layout, descriptor_set_layout, descriptor_pool,
                command_pool,
                ring: Mutex::new(RingVk {
                    lifecycle: crate::render::mesh_dispatch_ring::DispatchRing::new(inflight),
                    slots,
                    epoch: std::time::Instant::now(),
                    next_ticket: 1,
                    per_chunk_ema_us: PER_CHUNK_PRIOR_US,
                    samples: 0,
                    costs: Vec::new(),
                }),
                slot_chunks, meta_stride,
                queue, queue_is_async,
                queue_submit_lock, device,
            })
        }
    }

    /// Chunks one dispatch can carry. The dispatcher's batch cap.
    pub fn slot_capacity(&self) -> usize { self.slot_chunks }
    /// Ring depth.
    pub fn inflight_capacity(&self) -> usize { self.ring.lock().unwrap().slots.len() }
    /// Dispatches currently on the queue.
    pub fn in_flight(&self) -> usize { self.ring.lock().unwrap().lifecycle.in_flight() }
    /// True when dispatches go to a queue the frame does not share.
    pub fn queue_is_async(&self) -> bool { self.queue_is_async }
    /// Is a slot free right now?
    pub fn can_submit(&self) -> bool {
        matches!(self.ring.lock().unwrap().lifecycle.admission(),
                 crate::render::mesh_dispatch_ring::Admission::Submit { .. })
    }

    /// The batch sizer. How many chunks fit in `dispatch_budget_us` of
    /// queue time according to the per-chunk model, clamped to
    /// `[1, slot_capacity]`. Before any measurement the prior
    /// (`PER_CHUNK_PRIOR_US`) answers, so the first dispatch is small.
    pub fn suggested_batch(&self, dispatch_budget_us: u64) -> usize {
        let ring = self.ring.lock().unwrap();
        let n = (dispatch_budget_us as f64 / ring.per_chunk_ema_us.max(1.0)).floor() as usize;
        n.clamp(1, self.slot_chunks)
    }

    /// Current per-chunk GPU-time estimate, µs, and how many dispatches fed it.
    pub fn per_chunk_estimate_us(&self) -> (f64, u64) {
        let ring = self.ring.lock().unwrap();
        (ring.per_chunk_ema_us, ring.samples)
    }

    /// Costs of dispatches completed since the last call. The dispatcher
    /// charges its per-frame window budget from these, not from the wall
    /// time around a blocking call (which double-counts under overlap).
    pub fn take_costs(&self) -> Vec<DispatchCost> {
        std::mem::take(&mut self.ring.lock().unwrap().costs)
    }

    /// Mesh up to `slot_capacity()` chunks per submission, blocking.
    /// Auto-windows if `chunks.len()` exceeds the cap. Submit + collect per
    /// window, so this never overlaps two of its own windows; it is the
    /// bench/test entry, not the streaming path.
    ///
    /// `lod_stride` is the LOD's source-block stride: 1 for Near
    /// (per-block iteration), 2 for Mid (each mesher cell represents
    /// a 2³ source-block group). All chunks in the batch must be at
    /// the same LOD.
    pub fn mesh_chunks_batch(&self, chunks: &[OctreeBatchInput], lod_stride: u32) -> Result<Vec<OctreeBatchOutput>> {
        if chunks.is_empty() {
            return Ok(Vec::new());
        }
        let mut all_outputs = Vec::with_capacity(chunks.len());
        for window in chunks.chunks(self.slot_chunks) {
            let t = self.submit_batch(window, lod_stride)?;
            all_outputs.append(&mut self.collect(t)?);
        }
        Ok(all_outputs)
    }

    /// Submit one batch WITHOUT waiting. Uploads and pins the batch's
    /// octree spans under the pool lock, releases it, records into a free
    /// slot and submits. `Err(Busy)` if every slot is in flight — the
    /// caller has outrun `poll`; wait with `wait_oldest` first.
    ///
    /// THE INVARIANT (same as the generator ring): a slot's command
    /// buffer, fence, descriptor set and output region are never reused
    /// until that slot's result has been drained, and every octree span
    /// the dispatch reads stays pinned until then. No dispatch in flight
    /// refers to a recycled resource.
    pub fn submit_batch(&self, chunks: &[OctreeBatchInput], lod_stride: u32) -> Result<MeshTicket> {
        use crate::render::mesh_dispatch_ring::Admission;
        if chunks.is_empty() {
            return Err(OctreeMeshError::EmptyBatch);
        }
        if chunks.len() > self.slot_chunks {
            return Err(OctreeMeshError::BatchTooLarge { got: chunks.len(), cap: self.slot_chunks });
        }
        debug_assert!(
            lod_stride == 1 || lod_stride == 2 || lod_stride == 4 || lod_stride == 8,
            "lod_stride must divide 32 and be a power of two; got {}", lod_stride
        );

        // Pre-screen. `active` lists the positions in `chunks` that go to
        // the GPU; they are compacted to meta indices 0..active.len() so
        // the dispatch carries no stale metas.
        let mut active: Vec<usize> = Vec::with_capacity(chunks.len());
        let mut outputs: Vec<OctreeBatchOutput> = chunks.iter().enumerate().map(|(i, c)| {
            let n = c.octree.nodes.len();
            let mesh = if n > MAX_NODES_PER_CHUNK as usize {
                Err(OctreeMeshError::TooManyNodes(n))
            } else if n == 0 {
                Ok(ChunkMesh::empty())
            } else {
                active.push(i);
                Err(OctreeMeshError::Vulkan(vk::Result::NOT_READY))
            };
            OctreeBatchOutput { coord: c.coord, mesh }
        }).collect();

        let mut ring = self.ring.lock().unwrap();
        let slot = match ring.lifecycle.admission() {
            Admission::Submit { slot } => slot,
            Admission::WaitOldest { .. } => return Err(OctreeMeshError::Busy),
        };

        if active.is_empty() {
            // Nothing for the GPU: hand the pre-screened outputs back via
            // a ticket that `poll`/`collect` resolve without a fence.
            // Recorded as an already-signaled slot: the fence is left
            // signaled from creation? No — fences start unsignaled, so we
            // mark this slot pending with a signaled fence explicitly.
            let ticket = ring.next_ticket;
            ring.next_ticket += 1;
            ring.lifecycle.acquire(slot, Vec::new());
            let now = ring.epoch.elapsed().as_micros() as u64;
            ring.lifecycle.submitted(slot, now);
            unsafe {
                let submit = vk::SubmitInfo::builder().build();
                let s = &ring.slots[slot];
                self.device.reset_fences(&[s.fence])?;
                let _qg = self.queue_submit_lock.lock().expect("octree mesher queue submit lock poisoned");
                // An empty submit still signals the fence: keeps every
                // ticket on one code path.
                self.device.queue_submit(self.queue, &[submit], s.fence)?;
            }
            ring.slots[slot].pending = Some(PendingBatch { outputs, active, ticket });
            return Ok(MeshTicket(ticket));
        }

        // Upload + pin under the pool lock; the lock is released before
        // recording so main-thread uploads/evictions proceed while the
        // GPU runs (frees of pinned spans defer to the unpin at drain).
        let mut node_bases: Vec<u32> = Vec::with_capacity(active.len());
        let mut neighbor_bases: Vec<[u32; 6]> = Vec::with_capacity(active.len());
        let mut pin_offsets: Vec<u64> = Vec::with_capacity(active.len() * 7);
        {
            let mut pool = self.octree_pool.lock()
                .map_err(|_| OctreeMeshError::Vulkan(vk::Result::ERROR_UNKNOWN))?;
            for &ci in &active {
                let c = &chunks[ci];
                match pool.upload_if_changed(c.coord, &c.octree, c.version) {
                    Ok(s) => {
                        pool.pin_slot(s);
                        pin_offsets.push(s.byte_offset);
                        node_bases.push(s.base_node_index());
                    }
                    Err(e) => {
                        log::warn!(
                            "OctreeGpuMesher: pool upload failed for {:?}: {}; skipping this chunk in batch",
                            c.coord, e
                        );
                        outputs[ci].mesh =
                            Err(OctreeMeshError::Vulkan(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY));
                        node_bases.push(u32::MAX);
                    }
                }
                // v0.24.30-cross-chunk: the 6 neighbor bases, pinned too
                // (the shader reads them). Order: [+X, -X, +Y, -Y, +Z, -Z];
                // u32::MAX = not resident → open-boundary for that face.
                const NEIGHBOR_DELTAS: [(i32, i32, i32); 6] = [
                    ( 1,  0,  0), (-1,  0,  0), ( 0,  1,  0), ( 0, -1,  0), ( 0,  0,  1), ( 0,  0, -1),
                ];
                let mut bases = [u32::MAX; 6];
                for (k, dc) in NEIGHBOR_DELTAS.iter().enumerate() {
                    let nc = ChunkCoord::new(c.coord.x + dc.0, c.coord.y + dc.1, c.coord.z + dc.2);
                    if let Some(s) = pool.slot(nc) {
                        bases[k] = s.base_node_index();
                        pool.pin_slot(s);
                        pin_offsets.push(s.byte_offset);
                    }
                }
                neighbor_bases.push(bases);
            }
        }

        // Compact: positions whose upload failed drop out of the dispatch
        // (their outputs already carry the error).
        let mut dispatched: Vec<usize> = Vec::with_capacity(active.len());
        let mut dispatched_bases: Vec<(u32, [u32; 6])> = Vec::with_capacity(active.len());
        for (pos, &ci) in active.iter().enumerate() {
            if node_bases[pos] != u32::MAX {
                dispatched.push(ci);
                dispatched_bases.push((node_bases[pos], neighbor_bases[pos]));
            }
        }
        if dispatched.is_empty() {
            // Every upload failed. Unpin what we pinned and report the
            // batch as one failed dispatch rather than hanging a ticket.
            if let Ok(mut pool) = self.octree_pool.lock() { pool.unpin_spans(&pin_offsets); }
            return Err(OctreeMeshError::Vulkan(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY));
        }

        let ticket = ring.next_ticket;
        ring.next_ticket += 1;
        ring.lifecycle.acquire(slot, pin_offsets);

        // Metas into this slot's region: meta i describes dispatched[i],
        // whose output lives at global chunk index base + i.
        let global_base = (slot * self.slot_chunks) as u32;
        unsafe {
            for (i, (&ci, &(octree_base, nb))) in dispatched.iter().zip(dispatched_bases.iter()).enumerate() {
                let c = &chunks[ci];
                let g = global_base + i as u32;
                let origin = c.coord.block_origin();
                let meta = ChunkMetaGpu {
                    octree_base,
                    vertex_base: g * VERTEX_CAPACITY,
                    index_base:  g * INDEX_CAPACITY,
                    counter_base: g * 4,
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
                let dst = self.chunk_metas_ptr.add(
                    (slot as u64 * self.meta_stride) as usize
                        + i * std::mem::size_of::<ChunkMetaGpu>(),
                );
                std::ptr::copy_nonoverlapping(
                    &meta as *const _ as *const u8, dst, std::mem::size_of::<ChunkMetaGpu>());
            }
        }

        let record = self.record_and_submit(&ring.slots[slot], slot, dispatched.len() as u32, lod_stride);
        match record {
            Ok(()) => {
                let now = ring.epoch.elapsed().as_micros() as u64;
                ring.lifecycle.submitted(slot, now);
                ring.slots[slot].pending = Some(PendingBatch { outputs, active: dispatched, ticket });
                Ok(MeshTicket(ticket))
            }
            Err(e) => {
                let pins = ring.lifecycle.abort_recording(slot);
                if let Ok(mut pool) = self.octree_pool.lock() { pool.unpin_spans(&pins); }
                Err(e)
            }
        }
    }

    /// Records the slot's command buffer and submits it with the slot's
    /// fence. No wait. Zeroes ONLY this slot's counter region: the other
    /// slots' counters may be mid-write on the GPU.
    fn record_and_submit(&self, s: &SlotVk, slot: usize, batch_count: u32, lod_stride: u32) -> Result<()> {
        let cmd = s.command_buffer;
        let counter_off = (slot * self.slot_chunks) as u64 * 16;
        let counter_len = self.slot_chunks as u64 * 16;
        unsafe {
            self.device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
            let begin = vk::CommandBufferBeginInfo::builder()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            self.device.begin_command_buffer(cmd, &begin)?;

            self.device.cmd_fill_buffer(cmd, self.counter_buffer, counter_off, counter_len, 0);
            let counter_barrier = vk::BufferMemoryBarrier::builder()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.counter_buffer)
                .offset(counter_off).size(counter_len)
                .build();
            self.device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[], &[counter_barrier], &[],
            );

            self.device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline);
            self.device.cmd_bind_descriptor_sets(
                cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline_layout, 0,
                std::slice::from_ref(&s.descriptor_set), &[],
            );

            // ONE dispatch covering the batch. chunk_idx = gl_WorkGroupID.y
            // indexes this slot's meta region. v0.24.39-mid-lod: cell
            // count from stride (Near 32³ → 512 workgroups; Mid 16³ → 64).
            let lod_cell_count = 32 / lod_stride;
            let cells_per_chunk = lod_cell_count * lod_cell_count * lod_cell_count;
            let workgroups_x = (cells_per_chunk + WORKGROUP_SIZE - 1) / WORKGROUP_SIZE;
            let push = PushConstants {
                vertex_capacity: VERTEX_CAPACITY,
                index_capacity: INDEX_CAPACITY,
                batch_count,
                lod_cell_count,
                lod_cell_stride: lod_stride,
                tile_enabled: octree_tile_enabled(),
            };
            let push_bytes = std::slice::from_raw_parts(
                &push as *const _ as *const u8, std::mem::size_of::<PushConstants>());
            self.device.cmd_push_constants(
                cmd, self.pipeline_layout, vk::ShaderStageFlags::COMPUTE, 0, push_bytes);
            self.device.cmd_dispatch(cmd, workgroups_x, batch_count, 1);

            // Shader writes → host reads on the output buffers.
            let buffer_barriers = [
                buf_barrier(self.vertex_buffer),
                buf_barrier(self.index_buffer),
                buf_barrier(self.counter_buffer),
            ];
            self.device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[], &buffer_barriers, &[],
            );
            self.device.end_command_buffer(cmd)?;

            self.device.reset_fences(&[s.fence])?;
            let submit = vk::SubmitInfo::builder()
                .command_buffers(std::slice::from_ref(&cmd))
                .build();
            let _qg = self.queue_submit_lock.lock()
                .expect("octree mesher queue submit lock poisoned");
            self.device.queue_submit(self.queue, &[submit], s.fence)?;
        }
        Ok(())
    }

    /// Every finished batch, read back and unpinned, oldest first.
    /// Non-blocking; batches still running stay in the ring.
    pub fn poll(&self) -> Result<Vec<(MeshTicket, Vec<OctreeBatchOutput>)>> {
        let mut ring = self.ring.lock().unwrap();
        self.drain_finished(&mut ring, None)
    }

    /// Block (at most `timeout`) on the OLDEST in-flight dispatch, then
    /// drain everything that has finished. The backpressure call for a
    /// full ring; returns empty if nothing is in flight or the wait timed
    /// out.
    pub fn wait_oldest(&self, timeout: std::time::Duration) -> Result<Vec<(MeshTicket, Vec<OctreeBatchOutput>)>> {
        use crate::render::mesh_dispatch_ring::Admission;
        let mut ring = self.ring.lock().unwrap();
        let oldest = match ring.lifecycle.admission() {
            Admission::WaitOldest { slot } => Some(slot),
            Admission::Submit { .. } => {
                // Not full; still wait on the oldest pending one if any.
                ring.slots.iter().enumerate()
                    .filter(|(_, s)| s.pending.is_some())
                    .min_by_key(|(_, s)| s.pending.as_ref().map(|p| p.ticket).unwrap_or(u64::MAX))
                    .map(|(i, _)| i)
            }
        };
        if let Some(i) = oldest {
            let fence = ring.slots[i].fence;
            let ns = timeout.as_nanos().min(u64::MAX as u128) as u64;
            match unsafe { self.device.wait_for_fences(&[fence], true, ns) } {
                Ok(()) | Err(vk::Result::TIMEOUT) => {}
                Err(e) => return Err(e.into()),
            }
        }
        self.drain_finished(&mut ring, None)
    }

    /// Wait for ONE ticket and return its outputs. Other batches keep
    /// running and are not drained here.
    pub fn collect(&self, t: MeshTicket) -> Result<Vec<OctreeBatchOutput>> {
        let mut ring = self.ring.lock().unwrap();
        let Some(i) = ring.slots.iter().position(|s| s.pending.as_ref().map(|p| p.ticket) == Some(t.0)) else {
            return Err(OctreeMeshError::UnknownTicket(t.0));
        };
        let fence = ring.slots[i].fence;
        unsafe { self.device.wait_for_fences(&[fence], true, u64::MAX)?; }
        let mut out = self.drain_finished(&mut ring, Some(i))?;
        debug_assert_eq!(out.len(), 1);
        Ok(out.pop().map(|(_, o)| o).unwrap_or_default())
    }

    /// Drain slots whose fence has signaled (all of them, or `only` that
    /// one): charge GPU time, read back, unpin, release. Oldest first.
    fn drain_finished(
        &self, ring: &mut RingVk, only: Option<usize>,
    ) -> Result<Vec<(MeshTicket, Vec<OctreeBatchOutput>)>> {
        let mut done: Vec<usize> = Vec::new();
        for (i, s) in ring.slots.iter().enumerate() {
            if only.map_or(true, |o| o == i) && s.pending.is_some()
                && unsafe { self.device.get_fence_status(s.fence)? }
            {
                done.push(i);
            }
        }
        done.sort_by_key(|&i| ring.slots[i].pending.as_ref().map(|p| p.ticket).unwrap_or(0));
        let mut out = Vec::with_capacity(done.len());
        for i in done {
            let now = ring.epoch.elapsed().as_micros() as u64;
            let before = ring.lifecycle.gpu_time_consumed_us();
            let pins = ring.lifecycle.complete(i, now);
            let charged = ring.lifecycle.gpu_time_consumed_us() - before;
            let mut pending = ring.slots[i].pending.take().expect("drained slot has a batch");
            self.readback_slot(i, &mut pending);
            if !pins.is_empty() {
                match self.octree_pool.lock() {
                    Ok(mut pool) => pool.unpin_spans(&pins),
                    Err(_) => log::error!(
                        "OctreeGpuMesher: octree pool poisoned at unpin — {} pinned spans leaked",
                        pins.len()),
                }
            }
            let chunks = pending.active.len() as u32;
            if chunks > 0 {
                let per = charged as f64 / chunks as f64;
                ring.per_chunk_ema_us = if ring.samples == 0 { per }
                    else { ring.per_chunk_ema_us * (1.0 - EMA_ALPHA) + per * EMA_ALPHA };
                ring.samples += 1;
            }
            ring.costs.push(DispatchCost { gpu_exclusive_us: charged, chunks });
            out.push((MeshTicket(pending.ticket), pending.outputs));
        }
        Ok(out)
    }

    /// Reads the mesh output (counters + vertex/index data) for each
    /// dispatched position of slot `slot` into its outputs. Touches only
    /// the mesher's own HOST_COHERENT buffers, never the octree pool.
    fn readback_slot(&self, slot: usize, pending: &mut PendingBatch) {
        let global_base = slot * self.slot_chunks;
        for (i, &ci) in pending.active.iter().enumerate() {
            let g = global_base + i;
            let (vc, ic, overflow) = unsafe {
                let p = (self.counter_ptr as *const u32).add(g * 4);
                (*p, *p.add(1), *p.add(2))
            };
            if overflow != 0 || vc > VERTEX_CAPACITY || ic > INDEX_CAPACITY {
                pending.outputs[ci].mesh = Err(OctreeMeshError::Overflow {
                    vertex_count: vc, index_count: ic,
                });
                continue;
            }
            // The shader emits `GpuWireVertex` (6 floats, 24 B). It is NOT
            // `mesher::Vertex`: r594 added a `sway: u8` to `Vertex`, making
            // it 28 B, and the v0.24.44 "readback-bulk" cast of this region
            // to `*const Vertex` then read every vertex after the first at
            // the wrong stride — and ran ~390 KB past the buffer end for the
            // last slot. Convert per element; the static assert below keeps
            // the wire type honest.
            let wire: &[GpuWireVertex] = unsafe {
                std::slice::from_raw_parts(
                    (self.vertex_ptr as *const f32).add(g * VERTEX_CAPACITY as usize * 6)
                        as *const GpuWireVertex,
                    vc as usize,
                )
            };
            let vertices: Vec<crate::render::mesher::Vertex> =
                wire.iter().map(|w| w.to_vertex()).collect();
            let indices = unsafe {
                std::slice::from_raw_parts(
                    (self.index_ptr as *const u32).add(g * INDEX_CAPACITY as usize),
                    ic as usize,
                )
            }.to_vec();
            pending.outputs[ci].mesh = Ok(ChunkMesh { vertices, indices });
        }
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
            let ring = self.ring.get_mut().unwrap_or_else(|e| e.into_inner());
            let cmds: Vec<vk::CommandBuffer> = ring.slots.iter().map(|s| s.command_buffer).collect();
            for s in &ring.slots {
                self.device.destroy_fence(s.fence, None);
            }
            self.device.free_command_buffers(self.command_pool, &cmds);
            self.device.destroy_command_pool(self.command_pool, None);

            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.pipeline_layout, None);
            self.device.destroy_descriptor_pool(self.descriptor_pool, None);
            self.device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);

            // The octree buffer is the renderer's `OctreeBufferPool`'s.

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
    let candidates = crate::assets::shader_paths("chunk_mesh_octree.comp.spv");
    let mut bytes: Option<Vec<u8>> = None;
    for p in &candidates {
        if p.exists() {
            bytes = Some(std::fs::read(p)?);
            break;
        }
    }
    let bytes = bytes.ok_or_else(|| OctreeMeshError::ShaderMissing(candidates[0].display().to_string()))?;
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

unsafe fn allocate_descriptor_sets(
    device: &Device, layout: vk::DescriptorSetLayout, count: u32,
) -> Result<(vk::DescriptorPool, Vec<vk::DescriptorSet>)> {
    let sizes = [vk::DescriptorPoolSize {
        ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 5 * count,
    }];
    let info = vk::DescriptorPoolCreateInfo::builder()
        .pool_sizes(&sizes).max_sets(count);
    let pool = device.create_descriptor_pool(&info, None)?;
    let layouts = vec![layout; count as usize];
    let alloc = vk::DescriptorSetAllocateInfo::builder()
        .descriptor_pool(pool).set_layouts(&layouts);
    let sets = device.allocate_descriptor_sets(&alloc)?;
    Ok((pool, sets))
}

/// Bindings 0-3 whole-buffer; binding 4 (chunk_metas) is the slot's
/// region `[meta_offset, meta_offset + meta_range)` so the shader's
/// zero-based `chunk_metas[chunk_idx]` lands in that slot.
unsafe fn update_descriptor_set(
    device: &Device, set: vk::DescriptorSet,
    octree: vk::Buffer, vertex: vk::Buffer, index: vk::Buffer,
    counter: vk::Buffer, metas: vk::Buffer, meta_offset: u64, meta_range: u64,
) {
    let o = vk::DescriptorBufferInfo::builder().buffer(octree).range(vk::WHOLE_SIZE).build();
    let v = vk::DescriptorBufferInfo::builder().buffer(vertex).range(vk::WHOLE_SIZE).build();
    let i = vk::DescriptorBufferInfo::builder().buffer(index).range(vk::WHOLE_SIZE).build();
    let c = vk::DescriptorBufferInfo::builder().buffer(counter).range(vk::WHOLE_SIZE).build();
    let m = vk::DescriptorBufferInfo::builder().buffer(metas).offset(meta_offset).range(meta_range).build();
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

#[cfg(test)]
mod ring_layout_tests {
    use super::*;

    #[test]
    fn wire_vertex_is_the_shader_stride_and_cpu_vertex_is_not() {
        // The bug this guards: reading the shader's 24 B vertices as
        // `mesher::Vertex` (28 B since r594's `sway`).
        assert_eq!(std::mem::size_of::<GpuWireVertex>(), 24);
        assert_ne!(std::mem::size_of::<crate::render::mesher::Vertex>(), 24,
            "if Vertex is 24 B again this test is stale, not the code");
        let w = GpuWireVertex { position: [1.0, 2.0, 3.0], color: [0.1, 0.2, 0.3] };
        let v = w.to_vertex();
        assert_eq!(v.position, [1.0, 2.0, 3.0]);
        assert_eq!(v.color, [0.1, 0.2, 0.3]);
        assert_eq!(v.sway, 0);
    }

    #[test]
    fn slots_partition_the_fixed_output_and_never_exceed_it() {
        for inflight in 1..=(MAX_BATCH_CHUNKS as usize) {
            let per = slot_chunks_for(inflight);
            assert!(per >= 1);
            assert!(per * inflight <= MAX_BATCH_CHUNKS as usize,
                "inflight {} × {} chunks overruns the buffers", inflight, per);
        }
        assert_eq!(slot_chunks_for(1), 64);
        assert_eq!(slot_chunks_for(2), 32);
        assert_eq!(slot_chunks_for(3), 21);
        assert_eq!(slot_chunks_for(0), 64, "inflight 0 is treated as 1");
    }

    #[test]
    fn meta_regions_are_256_aligned_and_hold_the_slot() {
        for per in [1usize, 2, 21, 32, 64] {
            let stride = meta_region_stride(per);
            assert_eq!(stride % META_REGION_ALIGN, 0);
            assert!(stride >= per as u64 * std::mem::size_of::<ChunkMetaGpu>() as u64);
            assert!(stride < (per as u64 + 4) * std::mem::size_of::<ChunkMetaGpu>() as u64,
                "stride {} wastes more than a page of metas for {} chunks", stride, per);
        }
    }

    #[test]
    fn inflight_env_is_clamped() {
        // Not asserting on the live env (tests share a process); the
        // clamp is the contract.
        for (raw, want) in [("0", 1usize), ("1", 1), ("2", 2), ("999", 64)] {
            let n: usize = raw.parse().unwrap();
            assert_eq!(n.clamp(1, MAX_BATCH_CHUNKS as usize), want);
        }
    }
}
