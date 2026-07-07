//! High-level chunk storage. Owns loaded chunks, handles eviction and lookup.
//!
//! Chunks are 3D-coordinated. Loading happens in a 3D radius around the
//! player. Derived queries (biome, surface y) go to the generator since
//! we no longer store them on the chunk.

use crate::constants::{CHUNK_SIZE, GPU_MAX_QUEUED_CHUNKS, runtime_render_distance};
use crate::world::biomes::BiomeSample;
use crate::world::blocks::BlockId;
use crate::world::chunk::{Chunk, ChunkCoord};
use crate::world::chunk_loader::{block_to_chunk, ChunkLoader};
use crate::world::fields::{ChunkFields, FieldRegistry};
use crate::world::octree::OctreeArray;
use crate::world::terrain_generator::TerrainGenerator;
use dashmap::DashMap;
use parking_lot::RwLock;
use crate::world::ticks::{TickRegistry, TickScheduler};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

// =============================================================================
// mgr_update sub-zone timing (v0.23.4 diagnostic)
// =============================================================================
//
// Per-frame mgr_update was 400-600ms during the v0.23.3 RD=64 playtest.
// `update()` does three things — drain newly-generated chunks, evict
// distant ones (when player crossed a chunk boundary), and walk the RD
// sphere requesting missing chunks. We don't yet know which dominates.
// These atomics accumulate microseconds in each phase; the binary reads
// and resets them at the 1s log boundary.
//
// Atomics + Relaxed are sufficient: only one thread (the main loop) ever
// writes them, and the binary reads the snapshot once a second. We don't
// need ordering guarantees with other state.

/// Microseconds spent in `loader.drain_completed()` since last reset.
pub static MGR_DRAIN_US: AtomicU64 = AtomicU64::new(0);
/// Microseconds spent in `evict_distant()` since last reset (zero on
/// most frames — only runs on player-chunk boundary cross).
pub static MGR_EVICT_US: AtomicU64 = AtomicU64::new(0);
/// Microseconds spent in `request_visible()` since last reset (zero on
/// most frames — only runs on player-chunk-cross or chunk-completion).
pub static MGR_REQUEST_US: AtomicU64 = AtomicU64::new(0);

/// Result of `ChunkManager::raycast_octree`. World-space version of
/// the per-chunk `RaycastHit` from `world/octree_raycast.rs`.
///
/// Callers wanting compatibility with the legacy `raycast::RayHit`
/// can map this with one of the conversion helpers (TODO: add when
/// the first caller migrates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OctreeWorldRayHit {
    /// Distance along the ray (in blocks/world-units) at which the
    /// hit occurred.
    pub t: f32,
    /// Integer world coordinates of the hit cell.
    pub world_cell: (i32, i32, i32),
    /// Block ID at the hit cell.
    pub block: BlockId,
    /// Face of the cell the ray entered through, as a unit vector
    /// in {-1, 0, 1}^3. Used for placement (face-attached new block)
    /// and particle spawning.
    pub normal: (i32, i32, i32),
    /// World-space point where the ray crossed the entering face.
    pub point: [f32; 3],
}

/// Returns `t` at which the ray exits the axis-aligned chunk box.
/// Assumes the ray's current origin is inside the box; returns
/// f32::INFINITY for axis-aligned-parallel rays that don't constrain
/// that axis. Helper for the world-level chunk-walking raycast.
fn slab_exit_t(origin: [f32; 3], dir: [f32; 3], box_min: [f32; 3], box_max: [f32; 3]) -> f32 {
    let mut t_max = f32::INFINITY;
    for ax in 0..3 {
        if dir[ax].abs() < 1e-6 {
            continue;
        }
        let t = if dir[ax] > 0.0 {
            (box_max[ax] - origin[ax]) / dir[ax]
        } else {
            (box_min[ax] - origin[ax]) / dir[ax]
        };
        if t > 0.0 && t < t_max {
            t_max = t;
        }
    }
    t_max
}

/// Whether a transitioning chunk should be re-meshed on this tick under the
/// stride policy. With `stride == 1` this is always true (original every-tick
/// behavior). For `stride > 1`, each chunk re-meshes once per `stride` ticks,
/// phase-offset by a hash of its coord so the transition set's re-meshes are
/// spread evenly across ticks (≈1/stride per tick) rather than all firing on
/// the same tick. Pure + deterministic so it's unit-testable.
#[inline]
pub fn transition_remesh_due(coord: ChunkCoord, tick: u64, stride: u64) -> bool {
    if stride <= 1 {
        return true;
    }
    // Cheap spatial hash → per-coord phase in [0, stride).
    let h = (coord.x as i64).wrapping_mul(73_856_093)
        ^ (coord.y as i64).wrapping_mul(19_349_663)
        ^ (coord.z as i64).wrapping_mul(83_492_791);
    let phase = (h as u64) % stride;
    tick % stride == phase
}

/// Whether a chunk is entirely empty sky and can be skipped from the load set.
/// `surface_target` is the 2D-estimated terrain elevation (blocks) at the
/// chunk's column; `chunk_y` is the chunk coordinate's Y; `margin_chunks` is
/// how many chunks of sky above the surface to still load. A chunk is sky when
/// its BOTTOM (chunk_y * CHUNK_SIZE) sits above surface + margin. Pure +
/// unit-testable; the generous margin (plus the caller's surface estimate
/// being a smooth 2D field) keeps real terrain — which never exceeds
/// surface_target by more than a few blocks — safely loaded.
#[inline]
pub fn is_sky_chunk(surface_target: f64, chunk_y: i32, margin_chunks: i32) -> bool {
    let chunk_bottom = (chunk_y * CHUNK_SIZE) as f64;
    let cutoff = surface_target + (margin_chunks * CHUNK_SIZE) as f64;
    chunk_bottom > cutoff
}

/// Whether `request_visible` must re-walk the render sphere this frame.
///
/// The walk is idempotent, so re-running it without a change reproduces the
/// same request set. We re-walk when:
///   - the player crossed a chunk boundary (the desired set shifted), or
///   - it's the first walk (initial sphere must be requested), or
///   - a chunk just completed AND the previous walk was saturated — only then
///     does freed queue capacity let a budget-capped request through.
///
/// The last clause is the fix for the settling case: once every desired chunk
/// is enqueued (no tier saturated), completions no longer open new work, so the
/// full RD-sphere traversal is skipped instead of paying ~31ms/frame at RD=12.
/// Pure so the decision is unit-testable without a live manager.
/// v0.25-prefetch flag gates. Default OFF so behavior is unchanged until the
/// player opts in (`UNENDLICH_PREFETCH=1`).
fn prefetch_enabled() -> bool {
    std::env::var("UNENDLICH_PREFETCH")
        .map(|v| v != "0" && !v.is_empty())
        .unwrap_or(true) // R27: default ON (kill-switch: =0 disables)
}
/// Max chunks to request ahead per update (`UNENDLICH_PREFETCH_COUNT`, default 16).
fn prefetch_count() -> usize {
    std::env::var("UNENDLICH_PREFETCH_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16)
}
/// Reach scalar (frames-ahead) for the predictor (`UNENDLICH_PREFETCH_LOOKAHEAD`,
/// default 16.0). Reach in chunks = lookahead × speed(chunks/frame).
fn prefetch_lookahead() -> f32 {
    std::env::var("UNENDLICH_PREFETCH_LOOKAHEAD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16.0)
}

fn should_rewalk(
    player_moved: bool,
    any_completed: bool,
    first_request_pending: bool,
    last_walk_saturated: bool,
) -> bool {
    player_moved || first_request_pending || (any_completed && last_walk_saturated)
}

/// Choose which out-of-range chunks to evict this call. `candidates` is every
/// chunk currently beyond the eviction radius paired with its squared distance
/// to the player. With `budget == 0` all are evicted (original behavior); with
/// `budget > 0` only the farthest `budget` are, so the synchronous-save cost of
/// a big eviction wave is spread across moves. Pure + unit-testable.
fn select_evictions(mut candidates: Vec<(ChunkCoord, i32)>, budget: usize) -> Vec<ChunkCoord> {
    if budget > 0 && candidates.len() > budget {
        // Farthest first.
        candidates.sort_unstable_by(|a, b| b.1.cmp(&a.1));
        candidates.truncate(budget);
    }
    candidates.into_iter().map(|(c, _)| c).collect()
}

pub struct ChunkManager {
    chunks: Arc<DashMap<ChunkCoord, Arc<Chunk>>>,
    /// v0.24.23 propagation: parallel map of GPU-resident-style sparse
    /// octrees, one per loaded chunk. Lifecycle is identical to
    /// `chunks` — built on chunk-loader worker, inserted/replaced on
    /// every chunk replacement, removed on eviction. Consumers
    /// (raycast, region reads, future GPU mesher, defer-aware
    /// scheduler) query via `octree(coord)`.
    ///
    /// The CPU mesher still operates on `Chunk` for now; the octree
    /// is available alongside for forthcoming consumer integrations.
    /// Edits trigger a full octree rebuild on the editing thread
    /// (~sub-ms per chunk) plus a `mark_path_modified` stamp at the
    /// current `world_edit_tick` — that stamp is what the cache-defer
    /// scheme keys off.
    octrees: Arc<DashMap<ChunkCoord, Arc<OctreeArray>>>,
    /// Monotonic counter incremented on every block edit. Used as
    /// the `tick` argument for octree `mark_path_modified`, so
    /// consumers can ask `region_stable_since(my_last_read_tick + 1)`
    /// and get a meaningful answer.
    ///
    /// AtomicU64 so callers can read without locks. Stored as u64 but
    /// truncated to u32 when stamping the octree (the octree's field
    /// is u32; with 1M edits/sec the u32 wraps every ~71 minutes,
    /// which is fine — consumers that hold a read across a wrap will
    /// false-invalidate once and rebuild, which is benign).
    world_edit_tick: Arc<AtomicU64>,
    /// B1 (r102): monotonic world generation/epoch counter. Bumped on every
    /// `reset_with_generator`/`rebuild_for_seed` (a full world swap). Mesh jobs
    /// issued before a reset belong to the OLD world; tagging each job with the
    /// epoch at issue time and dropping results whose epoch != `world_epoch()`
    /// is what stops in-flight mesher output from uploading stale geometry into
    /// the freshly-regenerated world. See `world::mesh_epoch`.
    world_epoch: u64,
    loader: ChunkLoader,
    generator: Arc<TerrainGenerator>,
    /// Worker count the loader was built with, retained so the loader can be
    /// rebuilt against a fresh generator on a seed change (F4 lifecycle).
    num_workers: usize,
    player_chunk: ChunkCoord,
    /// r239 (mesh-vision-sched): the player's horizontal look direction (yaw,
    /// radians, world XZ), set each frame from the camera. Used to build the
    /// player view cone for vision-driven mesh scheduling. 0 until first set.
    #[cfg(feature = "mesh-vision-sched")]
    player_view_yaw: f32,
    /// v0.25-prefetch: previous player world position, for finite-difference
    /// velocity used by motion-predictive load-ahead. `None` until first update.
    last_player_world: Option<(i32, i32, i32)>,
    /// v0.25-prefetch: last per-frame velocity in CHUNK units, for the
    /// mesh-ahead predictor (`predicted_mesh_ahead`). Updated each `update`.
    last_vel_chunks: [f32; 3],
    eviction_distance: i32,
    /// Root path for persistent storage. `None` = no persistence (temp/test
    /// world). `Some(path)` = load explicit edits on chunk-insert and save
    /// on chunk-evict.
    world_root: Option<PathBuf>,
    /// Field overlay cache. `None` = no fields registered (legacy behavior).
    /// `Some` holds the registry + a per-chunk grid cache. Lazy: grids are
    /// only allocated when a chunk is loaded.
    fields: Option<FieldsState>,
    /// Tick simulation state. `None` = no simulation; `Some` runs registered
    /// tick handlers at the scheduler's period (default 0.5s).
    ticks: Option<TickState>,
    /// v0.24.NEXT+15-apply-budget: max chunks whose edits are applied in one
    /// tick. 0 = unlimited (default). When >0, overflow ops carry into
    /// following ticks via TickState's carry_* buffers, spreading a loading
    /// wave's apply cost over several frames instead of one 100+ms hitch.
    apply_budget: usize,
    /// Multiplier on incoming `real_dt` for tick advancement. 1.0 = real
    /// time. 10.0 = 10x faster (10x as many ticks fire per real second).
    /// 0.1 = 10x slower (1 tick every 5 real seconds at default period).
    /// Used as a SHOWCASE / TESTING knob — visible adaptations like plant
    /// lifecycle and weathering are slow at 1x but fast-forwarded at 100x+.
    /// NOT rollback-safe to change mid-game in multiplayer; for solo only.
    pub tick_speed_multiplier: f32,
    /// True if `request_visible` has not yet been called. Set false after
    /// the first call. Ensures the initial chunk-loading sphere gets
    /// requested even on a frame where the player hasn't moved (common
    /// at startup: player_chunk is still the initial value).
    first_request_pending: bool,
    /// v0.24.64: whether the most recent `request_visible` walk saturated any
    /// tier (hit PER_TIER_BUDGET with more candidates still wanted). Gates the
    /// per-completion re-walk: a freshly-completed chunk only opens new request
    /// capacity if a previous walk was budget-capped. When the last walk
    /// requested everything it wanted (no saturation), a completion changes
    /// nothing requestable, so the full RD-sphere re-walk is skipped. In the
    /// settling case (most chunks loaded, no tier saturates) this turns a
    /// per-frame ~15k-coord traversal — logged at ~31ms/frame at RD=12 — into
    /// nothing once the desired set is fully enqueued.
    last_walk_saturated: bool,
    /// Coords of chunks that currently have at least one active block
    /// transition (visual lerp in progress). Maintained by the apply
    /// phase: a chunk is added when it gains its first transition and
    /// removed when its last transition finishes. Lets the per-tick
    /// re-mesh sweep iterate only the few transitioning chunks
    /// (typically 0-10) instead of scanning all 2000+ loaded chunks.
    ///
    /// Single-threaded write access (only `tick_simulation` mutates).
    /// HashSet is sufficient — no concurrent access needed.
    chunks_with_transitions: std::collections::HashSet<ChunkCoord>,
    /// Stride for the per-tick transition re-mesh sweep. A transitioning
    /// chunk is re-meshed every `transition_remesh_stride` ticks instead of
    /// every tick, phase-offset by chunk coord so the load stays smooth across
    /// ticks rather than bursting. 1 = every tick (original behavior). Larger
    /// values trade animation step-granularity for fewer re-meshes — the
    /// remeshing storm (run data: ~35 re-meshes per chunk over a session,
    /// dominated by large cherry-blossom transition fronts) shrinks ~linearly.
    /// `UNENDLICH_TRANSITION_REMESH_STRIDE`, default 1.
    transition_remesh_stride: u64,
    /// Monotonic counter incremented once per apply pass; combined with chunk
    /// coord to decide which transitioning chunks re-mesh this tick.
    transition_remesh_tick: u64,
    /// `UNENDLICH_REMESH_GATE`: when true, an edited chunk whose tick edits are
    /// all micro-detail (grass/flowers, see `remesh_policy`) AND whose current
    /// LOD is too coarse to draw them updates its data silently — no re-mesh is
    /// triggered. Cuts the plant-lifecycle far-mesh churn. Default false.
    remesh_gate: bool,
    /// Count of remeshes skipped by the gate (for the run summary).
    remesh_skipped: u64,
    /// Optional sky-chunk cull margin, in chunks. `None` (default) loads the
    /// full RD ball — every empty sky chunk above terrain and every deep
    /// chunk below. `Some(m)` skips requesting any chunk whose bottom sits
    /// more than `m` chunks above the (2D-estimated) surface at its column —
    /// i.e. chunks that are entirely empty sky. The run data showed ~57% of
    /// loaded chunks are empty sky; culling them cuts generation, meshing,
    /// memory, and the empty-mesh count. Opt-in because it will not load
    /// far-above-terrain player builds (their chunks are never requested).
    /// `UNENDLICH_SKY_CULL` (chunks of margin; unset/0 = off).
    sky_cull_margin: Option<i32>,
    /// Max chunks evicted per `evict_distant` call. 0 (default) = unlimited
    /// (evict every out-of-range chunk at once). The expensive part of
    /// eviction is the SYNCHRONOUS disk save of any edited chunk; a player
    /// move that strands hundreds of edited chunks at once blocks the frame on
    /// those writes (run data: evict=98.5ms in a LONG FRAME). When >0, only the
    /// farthest `evict_budget` chunks are evicted per call and the rest drain
    /// on subsequent moves — bounding the per-frame save cost. Edits are never
    /// lost (still flushed on shutdown / as they drain).
    /// `UNENDLICH_EVICT_BUDGET`, default 0.
    evict_budget: usize,
}

/// Bundled field state: the shared registry + per-chunk grid storage.
/// Wrapped in RwLock so multiple threads can read field grids concurrently
/// while one thread is invalidating.
struct FieldsState {
    registry: Arc<FieldRegistry>,
    chunk_grids: Arc<DashMap<ChunkCoord, RwLock<ChunkFields>>>,
}

/// Bundled tick state: registry of handlers + scheduler tracking time
/// + per-(handler, chunk) last-fire tracker for catch-up.
/// Per-chunk tree-root op, grouped from the world-coord `DeferredTreeRootOp`
/// stream in `tick_simulation`. Hoisted to module scope (was a fn-local enum)
/// so the apply-budget carry-over buffer can hold deferred ops across ticks.
#[derive(Debug, Clone)]
enum ChunkTreeOp {
    SetMarker { lx: i32, ly: i32, lz: i32 },
    ClearMarker { lx: i32, ly: i32, lz: i32 },
    SetUnanchored { lx: i32, ly: i32, lz: i32, on: bool },
    InstallPoi { cache: crate::world::chunk::ChunkPoiCache },
}

/// Edits for a single chunk, grouped by kind. Used both for the live apply
/// maps and for the apply-budget carry-over buffer.
type BlockEditVec = Vec<(i32, i32, i32, BlockId)>;
type StateChangeVec = Vec<(i32, i32, i32, crate::world::chunk::BlockInstanceState)>;
type TransitionOpVec = Vec<(i32, i32, i32, Option<crate::world::chunk::BlockTransition>)>;

struct TickState {
    registry: Arc<TickRegistry>,
    scheduler: TickScheduler,
    /// Catch-up support: when a chunk transitions from a slower band
    /// to a faster band (player approach), handlers consult this to
    /// learn how many ticks have elapsed since their last fire on
    /// that chunk, and apply that many ticks of evolution at once.
    fire_tracker: Arc<crate::world::scheduler::LastFireTracker>,
    /// Per-handler microseconds spent during the most recent
    /// `tick_simulation` call. Sum across all sub-ticks fired this
    /// frame. Empty until the first tick fires. Exposed via
    /// `ChunkManager::last_tick_stats()` so the bin's diagnostic
    /// logging can attribute tick_sim time per handler.
    last_handler_us: Vec<(&'static str, u64)>,
    /// Microseconds spent in Phase 3 (apply edits to chunks) of the
    /// most recent `tick_simulation`. Includes COW clones, block
    /// edits, state changes, transition ops, and the per-tick
    /// re-mesh trigger for transitioning chunks.
    last_apply_us: u64,
    /// Number of distinct chunks that received any edit this tick
    /// batch (i.e. how many COW clones happened in Phase 3).
    last_apply_chunks: usize,
    /// v0.24.NEXT+15-apply-budget: when `UNENDLICH_APPLY_BUDGET` caps the
    /// chunks applied per tick, the edits for chunks that didn't fit are
    /// stashed here and merged into next tick's maps — spreading a loading
    /// wave over several ticks instead of one 100+ms hitch. No edit is
    /// dropped. Empty (and unused) when the budget is 0 (the default).
    carry_block_edits: std::collections::HashMap<ChunkCoord, BlockEditVec>,
    carry_state_changes: std::collections::HashMap<ChunkCoord, StateChangeVec>,
    carry_transition_ops: std::collections::HashMap<ChunkCoord, TransitionOpVec>,
    carry_tree_root_ops: std::collections::HashMap<ChunkCoord, Vec<ChunkTreeOp>>,
}

/// Snapshot of how much memory the loaded chunks would consume in SurfaceRun
/// encoding vs the live `Vec<BlockId>` storage. Reported in the run-summary
/// `compression:` log line; the ratio guides whether live-storage swap is
/// worth the architectural lift.
#[derive(Clone, Copy, Debug, Default)]
pub struct CompressionStats {
    /// Number of loaded chunks sampled.
    pub chunks: u64,
    /// Total bytes the live storage occupies (chunks × CHUNK_VOLUME × sizeof(BlockId)).
    pub raw_bytes: u64,
    /// Total bytes the SurfaceRun encoding would occupy across all chunks.
    pub surface_run_bytes: u64,
    /// Largest per-column run count seen (1..=CHUNK_SIZE; higher = less
    /// compressible column, e.g. ore-veined or cave-riddled stone).
    pub worst_col_runs: u32,
}

impl CompressionStats {
    /// `raw_bytes / surface_run_bytes` — values > 1 mean smaller. Returns 0
    /// if no chunks were sampled.
    pub fn ratio(&self) -> f32 {
        if self.surface_run_bytes == 0 { 0.0 }
        else { self.raw_bytes as f32 / self.surface_run_bytes as f32 }
    }
}

impl ChunkManager {
    pub fn new(generator: Arc<TerrainGenerator>, num_workers: usize) -> Self {
        let loader = ChunkLoader::new(Arc::clone(&generator), num_workers);
        Self {
            chunks: Arc::new(DashMap::with_capacity(1024)),
            octrees: Arc::new(DashMap::with_capacity(1024)),
            world_edit_tick: Arc::new(AtomicU64::new(0)),
            world_epoch: 0,
            loader,
            generator,
            num_workers,
            player_chunk: ChunkCoord::new(0, 0, 0),
            #[cfg(feature = "mesh-vision-sched")]
            player_view_yaw: 0.0,
            last_player_world: None,
            last_vel_chunks: [0.0; 3],
            // Hysteresis: don't evict until a chunk is somewhat outside the
            // load radius. Chebyshev radius +2 in each axis.
            // Resolved once at construction so all call sites stay in sync
            // with whatever UNENDLICH_RENDER_DISTANCE was set to at startup.
            eviction_distance: runtime_render_distance() + 2,
            world_root: None,
            fields: None,
            ticks: None,
            apply_budget: std::env::var("UNENDLICH_APPLY_BUDGET")
                .ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(0),
            tick_speed_multiplier: 1.0,
            first_request_pending: true,
            // Start true so the very first completion-driven re-walk isn't
            // suppressed before we've ever measured saturation.
            last_walk_saturated: true,
            chunks_with_transitions: std::collections::HashSet::new(),
            transition_remesh_stride: std::env::var("UNENDLICH_TRANSITION_REMESH_STRIDE")
                .ok().and_then(|v| v.parse::<u64>().ok()).filter(|&n| n >= 1).unwrap_or(1),
            transition_remesh_tick: 0,
            remesh_gate: std::env::var("UNENDLICH_REMESH_GATE")
                .ok().map(|v| v != "0").unwrap_or(true), // R27: default ON
            remesh_skipped: 0,
            sky_cull_margin: match std::env::var("UNENDLICH_SKY_CULL") {
                Err(_) => Some(1), // R27: default ON, margin 1
                Ok(v) => v.parse::<i32>().ok().filter(|&m| m > 0), // explicit; 0/neg = off
            },
            evict_budget: std::env::var("UNENDLICH_EVICT_BUDGET")
                .ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(0),
        }
    }

    /// Read-only access to a chunk's sparse-octree encoding. None if
    /// the chunk isn't loaded. Consumers (raycast, region read,
    /// future GPU mesher, defer-aware scheduler) call this rather
    /// than rebuilding the octree from `Chunk` every query.
    pub fn octree(&self, coord: ChunkCoord) -> Option<Arc<OctreeArray>> {
        self.octrees.get(&coord).map(|e| Arc::clone(e.value()))
    }

    /// Cast a world-space ray against the loaded chunks and return the
    /// first blocking hit. The picking primitive for interaction: pass the
    /// camera eye, look direction, and reach; the hit gives the targeted
    /// block (`cell`), the face to place against (`place_cell()`), and what
    /// was struck (`block`). Uses the default solid-block filter; for
    /// specialty rays (e.g. passing through glass/water) call
    /// `octree_raycast::raycast_world` directly with a custom predicate.
    pub fn raycast(
        &self,
        origin: [f32; 3],
        dir: [f32; 3],
        max_t: f32,
    ) -> Option<crate::world::octree_raycast::WorldRaycastHit> {
        crate::world::octree_raycast::raycast_world(
            |c| self.octree(c),
            origin,
            dir,
            max_t,
            crate::world::octree_raycast::block_is_blocking,
        )
    }

    /// v0.24.24: hand out an `OctreeBlockSampler` view of the
    /// manager's per-chunk octrees. The sampler holds its own
    /// `Arc` to the map so it can be moved across threads or into
    /// closures independently of the manager's lifetime.
    ///
    /// Use this when wiring a new consumer that prefers octree-
    /// canonical sampling — physics, raycasts, fluids, item region
    /// reads. Existing callers using `&ChunkManager` as a
    /// `BlockSampler` continue to hit the dense path, which is the
    /// right default for scattered single-point queries.
    pub fn octree_sampler(&self) -> OctreeBlockSampler {
        OctreeBlockSampler { octrees: Arc::clone(&self.octrees) }
    }

    /// Current world edit tick. Increments monotonically on every
    /// block edit. Consumers that want to defer work on stable
    /// regions snapshot this value when they last produced output
    /// and check `region_stable_since(_, snapshot + 1)` next time
    /// around.
    pub fn world_edit_tick(&self) -> u64 {
        self.world_edit_tick.load(Ordering::Relaxed)
    }

    /// B1 (r102): the current world epoch. Incremented on every world reset
    /// (`reset_with_generator`/`rebuild_for_seed`). The renderer/mesher should
    /// read this when a mesh job is issued and again when its result returns,
    /// discarding the result if the epoch changed in between (the world was
    /// reset under it). Pairs with `world::mesh_epoch::mesh_result_is_current`.
    pub fn world_epoch(&self) -> u64 {
        self.world_epoch
    }

    /// Internal helper: install a chunk + freshly-built octree as
    /// a unit. Used at every chunk-replace site so the two maps
    /// stay in sync.
    fn install_chunk(&self, chunk: Chunk, octree: OctreeArray) {
        let coord = chunk.coord;
        self.chunks.insert(coord, Arc::new(chunk));
        self.octrees.insert(coord, Arc::new(octree));
    }

    /// Internal helper: replace a chunk that's been edited. v0.24.23
    /// follow-up: uses `OctreeArray::set_block` for incremental edit
    /// instead of rebuilding the octree from scratch. The cost
    /// difference is ~1ms (rebuild) → ~10µs (incremental). The first
    /// edit through a previously-collapsed subtree pays a bit more
    /// (split-on-write expands the path); subsequent edits along the
    /// same path are even cheaper.
    ///
    /// Falls back to full rebuild if no existing octree is found
    /// (defensive — shouldn't happen since chunks are always
    /// installed with their octrees).
    fn replace_edited_chunk(
        &self,
        new_chunk: Chunk,
        edit_local: (i32, i32, i32),
    ) {
        let coord = new_chunk.coord;
        let tick = (self.world_edit_tick.fetch_add(1, Ordering::Relaxed) + 1) as u32;
        let (lx, ly, lz) = edit_local;
        let new_block = new_chunk.get(lx, ly, lz);

        let new_octree = match self.octrees.get(&coord) {
            Some(existing_arc) => {
                // Clone the underlying octree, mutate the clone.
                // The Arc in the map still points at the old
                // (immutable) octree until we replace it below.
                let mut cloned = (**existing_arc.value()).clone();
                drop(existing_arc);
                cloned.set_block(lx, ly, lz, new_block, tick);
                cloned
            }
            None => {
                // Fallback: no prior octree. Build from scratch and
                // stamp the edit path so consumers see the new tick.
                let mut fresh = OctreeArray::from_chunk(&new_chunk);
                fresh.mark_path_modified(lx, ly, lz, tick);
                fresh
            }
        };

        self.chunks.insert(coord, Arc::new(new_chunk));
        self.octrees.insert(coord, Arc::new(new_octree));
    }

    /// Internal helper: replace a chunk that's been edited in many
    /// places (e.g., tick-simulation batch apply). Rebuilds the
    /// octree and stamps the AABB of all edits with the current
    /// tick (one bulk stamp rather than per-edit).
    fn replace_batch_edited_chunk(
        &self,
        new_chunk: Chunk,
        edit_aabb: Option<crate::world::octree_region::Aabb3>,
        suppress_remesh: bool,
        block_edits: Option<&Vec<(i32, i32, i32, BlockId)>>,
    ) {
        let coord = new_chunk.coord;
        let tick = (self.world_edit_tick.fetch_add(1, Ordering::Relaxed) + 1) as u32;

        // Lever #1: incremental octree edit instead of a full
        // `from_chunk` rebuild. The octree stores block ids ONLY, so:
        //
        //  * BLOCK edits are the only thing that changes its data. We
        //    clone the chunk's EXISTING octree and apply each edit with
        //    `set_block` (visible) / `edit_block_unstamped` (gated) —
        //    O(depth) per edit — rather than rebuilding all 32,768 cells.
        //
        //  * state / transition / tree-root edits change no block, so the
        //    octree data is identical. We still clone + stamp it (when
        //    visible) so the mesher re-meshes to pick up the new per-cell
        //    tint/state, but we skip the rebuild.
        //
        // The only path that still calls `from_chunk` is the defensive
        // "no prior octree" fallback (shouldn't happen — chunks install
        // with their octree). Split-on-write means an edited octree may
        // carry extra nodes vs a fresh `from_chunk`; a periodic
        // recompaction (rebuild when node_count drifts past ~1.5× the
        // post-load baseline) is filed as follow-up — see TICK_APPLY_FIXES.md.
        let new_octree: OctreeArray = match self.octrees.get(&coord) {
            Some(existing) => {
                let mut oct = (**existing.value()).clone();
                drop(existing);
                match block_edits {
                    Some(edits) if !edits.is_empty() => {
                        for &(lx, ly, lz, b) in edits {
                            if suppress_remesh {
                                oct.edit_block_unstamped(lx, ly, lz, b);
                            } else {
                                oct.set_block(lx, ly, lz, b, tick);
                            }
                        }
                    }
                    _ => {
                        // No block edits: data unchanged. Stamp (when
                        // visible) so a re-mesh picks up the new tint/state;
                        // suppressed edits leave the region stable.
                        if !suppress_remesh {
                            match edit_aabb {
                                Some(aabb) => oct.mark_region_modified(aabb.min, aabb.max, tick),
                                None => oct.mark_region_modified(
                                    (0, 0, 0),
                                    (CHUNK_SIZE, CHUNK_SIZE, CHUNK_SIZE),
                                    tick,
                                ),
                            }
                        }
                    }
                }
                oct
            }
            None => {
                // Defensive fallback: no prior octree. Build from scratch
                // and stamp the edit AABB so consumers see the new tick.
                let mut fresh = OctreeArray::from_chunk(&new_chunk);
                if !suppress_remesh {
                    match edit_aabb {
                        Some(aabb) => fresh.mark_region_modified(aabb.min, aabb.max, tick),
                        None => fresh.mark_region_modified(
                            (0, 0, 0),
                            (CHUNK_SIZE, CHUNK_SIZE, CHUNK_SIZE),
                            tick,
                        ),
                    }
                }
                fresh
            }
        };

        self.chunks.insert(coord, Arc::new(new_chunk));
        self.octrees.insert(coord, Arc::new(new_octree));
    }

    /// Enable per-chunk field overlays (wind, moisture, etc.) using the
    /// given registry. Each loaded chunk gets a `ChunkFields` grid cache;
    /// fields are derived lazily on first read.
    pub fn with_fields(mut self, registry: Arc<FieldRegistry>) -> Self {
        log::info!("Field overlays enabled ({} fields)", registry.len());
        self.fields = Some(FieldsState {
            registry,
            chunk_grids: Arc::new(DashMap::with_capacity(1024)),
        });
        self
    }

    /// Runtime replacement of the field registry. A world launch changes the
    /// seed and rebuilds the terrain generator (`rebuild_for_seed`), but the
    /// field overlays (moisture/biome/wind) sample their OWN `WorldModel`; if
    /// that model is not rebuilt for the new seed, the overlays stay on the old
    /// world while the terrain uses the new one — two seeds in one world. Call
    /// this on launch with a registry built from a model at the new seed.
    /// Clears the cached per-chunk field grids so they recompute.
    pub fn set_fields(&mut self, registry: Arc<FieldRegistry>) {
        log::info!("Field overlays rebuilt ({} fields)", registry.len());
        self.fields = Some(FieldsState {
            registry,
            chunk_grids: Arc::new(DashMap::with_capacity(1024)),
        });
    }

    /// Enable simulation ticks driven by the registered handlers. Tick
    /// period defaults to `DEFAULT_TICK_PERIOD` (0.5s); override with
    /// `set_tick_period`.
    pub fn with_ticks(mut self, registry: Arc<TickRegistry>) -> Self {
        log::info!("Simulation ticks enabled ({} handlers)", registry.len());
        self.ticks = Some(TickState {
            registry,
            scheduler: TickScheduler::new(),
            fire_tracker: Arc::new(crate::world::scheduler::LastFireTracker::new()),
            last_handler_us: Vec::new(),
            last_apply_us: 0,
            last_apply_chunks: 0,
            carry_block_edits: std::collections::HashMap::new(),
            carry_state_changes: std::collections::HashMap::new(),
            carry_transition_ops: std::collections::HashMap::new(),
            carry_tree_root_ops: std::collections::HashMap::new(),
        });
        self
    }

    pub fn set_tick_period(&mut self, period: f32) {
        if let Some(t) = &mut self.ticks {
            t.scheduler.set_period(period);
        }
    }

    /// Attach a persistent storage location. After this, chunks load their
    /// explicit edits from disk on insertion, and save on eviction. Pristine
    /// chunks (no edits) write nothing — they're regenerable.
    /// r163: switch the persistent root LIVE (world switch). The caller
    /// must `save_all()` the outgoing world first; chunks inserted after
    /// this load/save under the new root.
    /// r184: live apply-budget override (settings Performance tab /
    /// autotuner). 0 = unlimited, matching the env default semantics.
    pub fn set_apply_budget(&mut self, n: usize) {
        self.apply_budget = n;
    }

    pub fn set_world_root(&mut self, world_root: PathBuf) {
        log::info!("Persistent storage re-rooted at {}", world_root.display());
        if let Err(e) = std::fs::create_dir_all(&world_root) {
            log::warn!("Failed to create world root {}: {}", world_root.display(), e);
        }
        self.world_root = Some(world_root);
    }

    pub fn with_storage(mut self, world_root: PathBuf) -> Self {
        log::info!("Persistent storage enabled at {}", world_root.display());
        if let Err(e) = std::fs::create_dir_all(&world_root) {
            log::warn!("Failed to create world root {}: {}", world_root.display(), e);
        }
        self.world_root = Some(world_root);
        self
    }

    /// Per-frame tick: receive completed chunks, request new ones around the
    /// player, and evict ones that are now out of range.
    ///
    /// `update` takes the player's full 3D world position now since chunks
    /// are 3D-streamed. Single-interest-point convenience wrapper around
    /// the multi-interest path; equivalent to calling
    /// `update_with_interest_points` with a single point at this position.
    pub fn update(&mut self, player_world_x: i32, player_world_y: i32, player_world_z: i32) {
        let new_player_chunk = block_to_chunk(player_world_x, player_world_y, player_world_z);

        // SUB-ZONE TIMING (v0.23.4 diagnostic): mgr_update was logged
        // at 400-600ms during RD=64 playtest. The work splits into
        // three phases — drain, evict, request_visible — and we want
        // to know which dominates before optimizing. Updates `MGR_*`
        // atomic accumulators (read by the binary at the 1s log
        // boundary; reset there too).
        let t_drain_start = std::time::Instant::now();

        // Drain completed chunks; track if any arrived (drives whether we
        // need to re-walk request_visible — newly-completed chunks could
        // mean the heap now has room; only worth re-walking if so).
        let mut any_completed = false;
        for loaded in self.loader.drain_completed_iter() {
            any_completed = true;
            let crate::world::chunk_loader::LoadedChunk { mut chunk, octree } = loaded;
            // Overlay any persisted edits on top of the freshly-generated
            // procedural chunk. Edits "stick" through worldgen changes.
            if let Some(root) = &self.world_root {
                match crate::storage::load_chunk_edits(root, &mut chunk) {
                    Ok(0) => {}
                    Ok(n) => log::debug!("Loaded {} edits for chunk {:?}", n, chunk.coord),
                    Err(e) => log::warn!("Failed to load edits for {:?}: {}", chunk.coord, e),
                }
            }
            // Allocate field grids for this chunk if fields are enabled.
            // Grids start invalid; first read derives them.
            if let Some(state) = &self.fields {
                let grid = ChunkFields::new(chunk.coord, state.registry.len());
                state.chunk_grids.insert(chunk.coord, RwLock::new(grid));
            }
            // v0.24.23: if persisted edits replaced any blocks, the
            // worker-built octree is stale relative to the chunk. Rebuild
            // here so the maps stay in sync. The common case (no
            // persisted edits) uses the worker-built octree directly.
            let octree = if chunk.dirty {
                OctreeArray::from_chunk(&chunk)
            } else {
                octree
            };
            self.install_chunk(chunk, octree);
        }
        MGR_DRAIN_US.fetch_add(
            t_drain_start.elapsed().as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        let t_evict_start = std::time::Instant::now();
        let player_moved = new_player_chunk != self.player_chunk;
        if player_moved {
            self.player_chunk = new_player_chunk;
            self.evict_distant();
        }
        MGR_EVICT_US.fetch_add(
            t_evict_start.elapsed().as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        // PERF: request_visible walks chunks_in_radius (~925 coords) and
        // does a DashMap read per coord. It's idempotent — calling it a
        // second time without something changing produces the same
        // result. So we ONLY call it when something has changed:
        //   - Player moved chunks → desired set has new entries.
        //   - A chunk just completed → maybe the in_flight set has room
        //     to take a previously-deduped request (priorities can shift).
        // On steady-state frames where nothing changed, this skips ~925
        // DashMap reads per frame. At 120fps that's a major reduction in
        // mgr_update cost, which user logs showed at 29-79ms/sec for
        // pure-overhead frames during initial chunk loading.
        let t_req_start = std::time::Instant::now();
        if should_rewalk(
            player_moved,
            any_completed,
            self.first_request_pending,
            self.last_walk_saturated,
        ) {
            self.first_request_pending = false;
            self.last_walk_saturated = self.request_visible();
        }
        MGR_REQUEST_US.fetch_add(
            t_req_start.elapsed().as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        // v0.25-prefetch: motion-predictive load-ahead. Finite-difference the
        // player world position to get a velocity, then request chunks along
        // the predicted path so generation (and the meshing it triggers)
        // starts BEFORE the camera arrives — the fix for the on-demand
        // near-mesh frame-head stall. Opt-in via UNENDLICH_PREFETCH=1; deduped
        // by the loader and ranked by the same distance² priority as
        // request_visible, so visible chunks always win contention.
        if let Some((px, py, pz)) = self.last_player_world {
            let bvel = [
                (player_world_x - px) as f32,
                (player_world_y - py) as f32,
                (player_world_z - pz) as f32,
            ];
            // Cache velocity in chunk units for the mesh-ahead predictor
            // (stored regardless of the prefetch flag; cheap).
            let cs = CHUNK_SIZE as f32;
            self.last_vel_chunks = [bvel[0] / cs, bvel[1] / cs, bvel[2] / cs];
            if prefetch_enabled() && bvel != [0.0, 0.0, 0.0] {
                self.request_prefetch(bvel);
            }
        }
        self.last_player_world = Some((player_world_x, player_world_y, player_world_z));
    }

    /// v0.25-prefetch (mesh-ahead): the motion-predicted chunks that are
    /// already LOADED (data resident) and should be MESHED ahead of entering
    /// the normal render-distance shell — so the expensive near-mesh happens
    /// before the chunk is visible instead of at the moment of need. The bin
    /// folds these into its mesh-candidate set; its own in-flight /
    /// already-meshed filters still apply, and overlaps with the normal shell
    /// dedup there. Returns `[]` when prefetch is off or the camera is still.
    pub fn predicted_mesh_ahead(&self) -> Vec<ChunkCoord> {
        if !prefetch_enabled() {
            return Vec::new();
        }
        let n = prefetch_count();
        if n == 0 || self.last_vel_chunks == [0.0, 0.0, 0.0] {
            return Vec::new();
        }
        let player = [self.player_chunk.x, self.player_chunk.y, self.player_chunk.z];
        crate::world::prefetch_predict::prefetch(player, self.last_vel_chunks, prefetch_lookahead(), n)
            .into_iter()
            .map(|c| ChunkCoord::new(c[0], c[1], c[2]))
            .filter(|c| self.chunks.contains_key(c)) // loaded → data ready to mesh
            .collect()
    }

    /// v0.25-prefetch: request chunks ahead of the player along the motion
    /// vector, using the (formerly dormant) `world::prefetch_predict` core.
    /// `block_vel` is the per-frame world-space displacement in blocks; it is
    /// converted to chunk units before prediction. Already-loaded chunks are
    /// skipped (matching `request_visible`); the loader dedups in-flight ones.
    fn request_prefetch(&self, block_vel: [f32; 3]) {
        let n = prefetch_count();
        if n == 0 {
            return;
        }
        let cs = CHUNK_SIZE as f32;
        let vel = [block_vel[0] / cs, block_vel[1] / cs, block_vel[2] / cs];
        let player = [self.player_chunk.x, self.player_chunk.y, self.player_chunk.z];
        let ahead = crate::world::prefetch_predict::prefetch(player, vel, prefetch_lookahead(), n);

        let mut reqs: Vec<(ChunkCoord, i32)> = Vec::with_capacity(ahead.len());
        for c in ahead {
            let coord = ChunkCoord::new(c[0], c[1], c[2]);
            if self.chunks.contains_key(&coord) {
                continue; // already resident — nothing to prefetch
            }
            let (dx, dy, dz) = (c[0] - player[0], c[1] - player[1], c[2] - player[2]);
            reqs.push((coord, dx * dx + dy * dy + dz * dz));
        }
        if !reqs.is_empty() {
            self.loader.request_many(&reqs);
        }
    }

    /// Multi-interest-point streaming.
    ///
    /// Multiplayer prep: in a multi-player game, chunks should be
    /// loaded around EVERY active player (and possibly other interest
    /// points like recently-active areas, server-pinned regions, etc.)
    /// — not just one central player.
    ///
    /// Today this delegates to the single-point path using the FIRST
    /// interest point. The signature is in place so call sites can
    /// migrate to it now, and when the underlying loader gains
    /// multi-point support the upgrade is internal.
    ///
    /// `points` is a slice of `(world_x, world_y, world_z)`. An empty
    /// slice is a no-op (useful for dedicated-server ticks where
    /// nobody local is generating interest).
    ///
    /// Future: the loader should compute a UNION of disks around each
    /// point, with eviction triggered by a chunk being outside ALL
    /// disks. For now we use the first point as the canonical
    /// "primary" so single-player behavior is preserved.
    #[cfg(feature = "mesh-vision-sched")]
    pub fn set_player_view_yaw(&mut self, yaw: f32) {
        // r239: player look yaw for vision-driven mesh scheduling. Call once
        // per frame before update_with_interest_points. No-op without feature.
        self.player_view_yaw = yaw;
    }

    pub fn update_with_interest_points(&mut self, points: &[(i32, i32, i32)]) {
        if let Some(&(x, y, z)) = points.first() {
            self.update(x, y, z);
        } else {
            // No interest points — drain completed chunks anyway so they
            // don't pile up indefinitely on a dedicated server tick.
            for loaded in self.loader.drain_completed_iter() {
                let crate::world::chunk_loader::LoadedChunk { chunk, octree } = loaded;
                if let Some(state) = &self.fields {
                    let grid = ChunkFields::new(chunk.coord, state.registry.len());
                    state.chunk_grids.insert(chunk.coord, RwLock::new(grid));
                }
                self.install_chunk(chunk, octree);
            }
        }
    }

    /// Advance simulation by `real_dt` real seconds. If a tick is due
    /// (per the scheduler's period), runs all registered handlers and
    /// applies their deferred edits. Multiple ticks may fire in one call
    /// if real_dt is large (e.g. after a slow frame).
    ///
    /// Call once per frame from the game loop; no-op if ticks are not
    /// enabled (`with_ticks` was never called).
    pub fn tick_simulation(&mut self, real_dt: f32) {
        // Apply the tick speed multiplier — the SHOWCASE knob that lets
        // the player accelerate slow simulation features (plant lifecycle,
        // weathering) without changing tick period (which would skip tick
        // handlers entirely on slow systems).
        let scaled_dt = real_dt * self.tick_speed_multiplier.max(0.0);
        // Phase 1: figure out how many ticks to run, capture period, AND
        // pre-compute the TickIds. Doing the advance_tick calls upfront
        // avoids a borrow conflict with the field_reader closure that
        // captures &self below.
        let (n_ticks, period, tick_ids) = {
            let Some(state) = &mut self.ticks else { return };
            let n = state.scheduler.advance(scaled_dt);
            if n == 0 { return; }
            let p = state.scheduler.period();
            let mut ids = Vec::with_capacity(n as usize);
            for _ in 0..n {
                ids.push(state.scheduler.advance_tick());
            }
            (n, p, ids)
        };
        let _ = n_ticks; // documented; tick_ids.len() is the same

        // Phase 2: run handlers. Build the field_reader closure here so
        // it can borrow `&self.fields` and `&self.chunks` (immutably);
        // the registry itself we access through a separate Arc clone so
        // we don't hold a borrow on `self.ticks` during handler execution.
        let (registry, fire_tracker) = match &self.ticks {
            Some(state) => (Arc::clone(&state.registry), Arc::clone(&state.fire_tracker)),
            None => return,
        };

        let mut all_block_edits = Vec::new();
        let mut all_state_changes = Vec::new();
        let mut all_transition_ops = Vec::new();
        let mut all_tree_root_ops = Vec::new();

        // Build a field reader that handlers can use. Captures by reference
        // since the closure lifetime is bound to this function.
        let field_reader_owned: Option<Box<dyn Fn(&str, i32, i32, i32) -> Option<f32> + '_>> =
            if self.fields.is_some() {
                Some(Box::new(|name: &str, x: i32, y: i32, z: i32| -> Option<f32> {
                    self.field_at(name, x, y, z)
                }))
            } else {
                None
            };

        // Build a per-chunk field reader. Returns a stack-allocated
        // ChunkFieldSnapshot — meant to be called ONCE per chunk per
        // tick, with the snapshot then sampled per-plant cheaply.
        // This is the optimization that gets tick_sim out of the
        // string-hash + DashMap + RwLock-write path on every plant.
        let chunk_field_reader_owned: Option<Box<dyn Fn(&str, ChunkCoord)
            -> Option<crate::world::fields::ChunkFieldSnapshot> + '_>> =
            if self.fields.is_some() {
                Some(Box::new(|name: &str, coord: ChunkCoord| {
                    self.chunk_field_snapshot(name, coord)
                }))
            } else {
                None
            };

        // Per-handler timing accumulator. Sum microseconds per handler
        // across all sub-ticks fired this frame. Stable handler names
        // (registered as &'static str) let us key by them safely.
        let mut handler_us_accum: std::collections::HashMap<&'static str, u64> =
            std::collections::HashMap::new();

        for tick_id in tick_ids {
            let edits = registry.run_tick(
                &self.chunks,
                field_reader_owned.as_ref().map(|f| f.as_ref() as &dyn Fn(&str, i32, i32, i32) -> Option<f32>),
                chunk_field_reader_owned.as_ref().map(|f|
                    f.as_ref() as &dyn Fn(&str, ChunkCoord)
                        -> Option<crate::world::fields::ChunkFieldSnapshot>),
                Some(self.player_chunk),
                tick_id,
                period,
                period,  // tick_dt = period (one full tick worth)
                Some(crate::world::scheduler::TickScheduleParams::default()),
                Some(&fire_tracker),
            );
            for (name, us) in &edits.handler_timings {
                *handler_us_accum.entry(*name).or_insert(0) += us;
            }
            all_block_edits.extend(edits.block_edits);
            all_state_changes.extend(edits.state_changes);
            all_transition_ops.extend(edits.transition_ops);
            all_tree_root_ops.extend(edits.tree_root_ops);
        }
        // Drop the closures before mutable borrows below.
        drop(field_reader_owned);
        drop(chunk_field_reader_owned);

        // Phase 3 timing.
        let t_apply = std::time::Instant::now();

        // Phase 3: apply edits via the normal `set_block` /
        // `set_instance_state` paths.
        //
        // Critical perf fix: BATCH per-chunk. The naive
        // "for each edit, call set_block" path COW-clones the chunk
        // separately for every edit — N edits to one chunk = N full
        // clones (~128KB block array each). Lifecycle ticking 50 plants
        // in a chunk = 50 clones per tick.
        //
        // Instead: group edits by chunk, COW-clone each chunk ONCE,
        // apply all its edits, then store the result. Edits to the same
        // chunk now cost one clone regardless of count.
        //
        // For block edits we still need to invalidate field overlays;
        // we collect a unique set of (chunk, local-coord) pairs while
        // applying, then run invalidation at the end.
        use std::collections::HashMap;
        // Group block_edits by chunk coord.
        let mut block_edits_by_chunk: HashMap<ChunkCoord, Vec<(i32, i32, i32, BlockId)>> = HashMap::new();
        for e in all_block_edits {
            let coord = block_to_chunk(e.world_x, e.world_y, e.world_z);
            let lx = e.world_x.rem_euclid(CHUNK_SIZE);
            let ly = e.world_y.rem_euclid(CHUNK_SIZE);
            let lz = e.world_z.rem_euclid(CHUNK_SIZE);
            block_edits_by_chunk.entry(coord).or_default().push((lx, ly, lz, e.new_block));
        }
        // Group state_changes by chunk coord.
        let mut state_changes_by_chunk:
            HashMap<ChunkCoord, Vec<(i32, i32, i32, crate::world::chunk::BlockInstanceState)>> = HashMap::new();
        for s in all_state_changes {
            let coord = block_to_chunk(s.world_x, s.world_y, s.world_z);
            let lx = s.world_x.rem_euclid(CHUNK_SIZE);
            let ly = s.world_y.rem_euclid(CHUNK_SIZE);
            let lz = s.world_z.rem_euclid(CHUNK_SIZE);
            state_changes_by_chunk.entry(coord).or_default().push((lx, ly, lz, s.new_state));
        }
        // Group transition_ops by chunk coord. Each entry holds the
        // local (x, y, z) plus an Option: Some(transition) = Start,
        // None = Finish. Newer ops in the same chunk override earlier
        // ones at the same index (Start then Finish in the same tick
        // = no entry; Finish then Start = entry created).
        let mut transition_ops_by_chunk:
            HashMap<ChunkCoord, Vec<(i32, i32, i32, Option<crate::world::chunk::BlockTransition>)>> = HashMap::new();
        for op in all_transition_ops {
            let (wx, wy, wz, payload) = match op {
                crate::world::ticks::DeferredTransitionOp::Start { world_x, world_y, world_z, transition } =>
                    (world_x, world_y, world_z, Some(transition)),
                crate::world::ticks::DeferredTransitionOp::Finish { world_x, world_y, world_z } =>
                    (world_x, world_y, world_z, None),
            };
            let coord = block_to_chunk(wx, wy, wz);
            let lx = wx.rem_euclid(CHUNK_SIZE);
            let ly = wy.rem_euclid(CHUNK_SIZE);
            let lz = wz.rem_euclid(CHUNK_SIZE);
            transition_ops_by_chunk.entry(coord).or_default().push((lx, ly, lz, payload));
        }

        // Group tree_root_ops by chunk coord. Two op flavors:
        //   - Per-block (SetMarker/ClearMarker/SetUnanchored): convert
        //     world coords → (chunk, local) and bin under the chunk.
        //   - Per-chunk (InstallPoiCache): bin directly under chunk.
        // Both flavors stored in the same per-chunk vec; the apply loop
        // matches on the variant. Order is preserved within a chunk so
        // that, e.g., a SetMarker before an InstallPoiCache in the same
        // tick applies in that order.
        // (ChunkTreeOp is now a module-level enum — see top of file.)
        let mut tree_root_ops_by_chunk: HashMap<ChunkCoord, Vec<ChunkTreeOp>> =
            HashMap::new();
        for op in all_tree_root_ops {
            use crate::world::ticks::DeferredTreeRootOp;
            match op {
                DeferredTreeRootOp::SetMarker { world_x, world_y, world_z } => {
                    let coord = block_to_chunk(world_x, world_y, world_z);
                    let lx = world_x.rem_euclid(CHUNK_SIZE);
                    let ly = world_y.rem_euclid(CHUNK_SIZE);
                    let lz = world_z.rem_euclid(CHUNK_SIZE);
                    tree_root_ops_by_chunk.entry(coord).or_default()
                        .push(ChunkTreeOp::SetMarker { lx, ly, lz });
                }
                DeferredTreeRootOp::ClearMarker { world_x, world_y, world_z } => {
                    let coord = block_to_chunk(world_x, world_y, world_z);
                    let lx = world_x.rem_euclid(CHUNK_SIZE);
                    let ly = world_y.rem_euclid(CHUNK_SIZE);
                    let lz = world_z.rem_euclid(CHUNK_SIZE);
                    tree_root_ops_by_chunk.entry(coord).or_default()
                        .push(ChunkTreeOp::ClearMarker { lx, ly, lz });
                }
                DeferredTreeRootOp::SetUnanchored { world_x, world_y, world_z, unanchored } => {
                    let coord = block_to_chunk(world_x, world_y, world_z);
                    let lx = world_x.rem_euclid(CHUNK_SIZE);
                    let ly = world_y.rem_euclid(CHUNK_SIZE);
                    let lz = world_z.rem_euclid(CHUNK_SIZE);
                    tree_root_ops_by_chunk.entry(coord).or_default()
                        .push(ChunkTreeOp::SetUnanchored { lx, ly, lz, on: unanchored });
                }
                DeferredTreeRootOp::InstallPoiCache { chunk, cache } => {
                    tree_root_ops_by_chunk.entry(chunk).or_default()
                        .push(ChunkTreeOp::InstallPoi { cache });
                }
            }
        }

        // v0.24.NEXT+15-apply-budget: merge any ops carried over from a
        // previous tick (deferred because that tick hit the per-tick apply
        // budget). Carried ops are OLDER, so they go BEFORE this tick's fresh
        // ops for the same chunk to preserve apply order. We splice the fresh
        // vec onto the end of the carried vec. When the budget is 0 these maps
        // are always empty, so this is a no-op in the default config.
        if let Some(state) = &mut self.ticks {
            for (coord, mut carried) in state.carry_block_edits.drain() {
                if let Some(fresh) = block_edits_by_chunk.remove(&coord) { carried.extend(fresh); }
                block_edits_by_chunk.insert(coord, carried);
            }
            for (coord, mut carried) in state.carry_state_changes.drain() {
                if let Some(fresh) = state_changes_by_chunk.remove(&coord) { carried.extend(fresh); }
                state_changes_by_chunk.insert(coord, carried);
            }
            for (coord, mut carried) in state.carry_transition_ops.drain() {
                if let Some(fresh) = transition_ops_by_chunk.remove(&coord) { carried.extend(fresh); }
                transition_ops_by_chunk.insert(coord, carried);
            }
            for (coord, mut carried) in state.carry_tree_root_ops.drain() {
                if let Some(fresh) = tree_root_ops_by_chunk.remove(&coord) { carried.extend(fresh); }
                tree_root_ops_by_chunk.insert(coord, carried);
            }
        }

        // Apply per-chunk: one COW clone, all edits, swap Arc once.
        // We process chunks that need EITHER kind of edit; the inner
        // logic applies whichever exist for that chunk.
        //
        // Sorted iteration for rollback determinism — chunk apply order
        // affects neighbor-chunk version bumps which affect re-mesh
        // ordering. Same input → same output.
        let mut chunks_needing_apply: Vec<ChunkCoord> = block_edits_by_chunk.keys()
            .chain(state_changes_by_chunk.keys())
            .chain(transition_ops_by_chunk.keys())
            .chain(tree_root_ops_by_chunk.keys())
            .copied()
            .collect();
        chunks_needing_apply.sort_by(|a, b|
            a.x.cmp(&b.x)
                .then_with(|| a.y.cmp(&b.y))
                .then_with(|| a.z.cmp(&b.z))
        );
        chunks_needing_apply.dedup();

        // v0.24.NEXT+15-apply-budget: cap chunks applied per tick. The loading
        // wave that lands hundreds of fresh-chunk decoration ops in one tick is
        // the dominant frame-time spike (observed apply=107.9ms for 489 chunks).
        // When UNENDLICH_APPLY_BUDGET=N>0, apply the first N (sorted, so
        // deterministic) and stash the rest's ops to carry into following ticks.
        // Default 0 = unlimited = byte-identical to the prior behaviour.
        let apply_budget = self.apply_budget;
        if apply_budget > 0 && chunks_needing_apply.len() > apply_budget {
            let deferred: Vec<ChunkCoord> = chunks_needing_apply.split_off(apply_budget);
            if let Some(state) = &mut self.ticks {
                for coord in &deferred {
                    if let Some(v) = block_edits_by_chunk.remove(coord) { state.carry_block_edits.insert(*coord, v); }
                    if let Some(v) = state_changes_by_chunk.remove(coord) { state.carry_state_changes.insert(*coord, v); }
                    if let Some(v) = transition_ops_by_chunk.remove(coord) { state.carry_transition_ops.insert(*coord, v); }
                    if let Some(v) = tree_root_ops_by_chunk.remove(coord) { state.carry_tree_root_ops.insert(*coord, v); }
                }
            }
        }
        let apply_chunk_count = chunks_needing_apply.len();

        for coord in chunks_needing_apply {
            // Get the current chunk and clone for edit. Skip if not loaded.
            let Some(entry) = self.chunks.get_mut(&coord) else { continue };
            let mut new_chunk = entry.value().clone_for_edit();
            // Apply all block edits for this chunk.
            if let Some(edits) = block_edits_by_chunk.get(&coord) {
                for &(lx, ly, lz, b) in edits {
                    new_chunk.set(lx, ly, lz, b);
                }
            }
            // Apply all state changes for this chunk.
            // r191: track whether any change touches more than age —
            // pure-aging batches skip the remesh entirely (the tick-
            // state / render-state DECOUPLING: aging is bookkeeping,
            // meshes only care about stage flags).
            let mut non_age_state_change = false;
            if let Some(changes) = state_changes_by_chunk.get(&coord) {
                for &(lx, ly, lz, s) in changes {
                    let before = new_chunk.get_instance_state_or_default(lx, ly, lz);
                    if before.flags != s.flags {
                        non_age_state_change = true;
                    }
                    new_chunk.set_instance_state(lx, ly, lz, s);
                }
            }
            // Apply all transition ops for this chunk. Start adds an
            // entry; Finish removes one. Order is preserved so that
            // Start-then-Finish in one tick is a no-op (creates the
            // entry, then removes it).
            //
            // After applying ops, sync the global
            // `chunks_with_transitions` set so the per-tick re-mesh
            // sweep below can iterate only currently-transitioning
            // chunks instead of scanning all loaded chunks. This is
            // the gate that turns the sweep from O(2000+) to O(few).
            let touched_transitions = transition_ops_by_chunk
                .get(&coord).is_some();
            if let Some(ops) = transition_ops_by_chunk.get(&coord) {
                for &(lx, ly, lz, payload) in ops {
                    let idx = crate::world::chunk::Chunk::block_index(lx, ly, lz) as u16;
                    match payload {
                        Some(t) => new_chunk.start_transition(idx, t),
                        None => new_chunk.finish_transition(idx),
                    }
                }
            }
            if touched_transitions {
                if new_chunk.transitions.is_empty() {
                    self.chunks_with_transitions.remove(&coord);
                } else {
                    self.chunks_with_transitions.insert(coord);
                }
            }

            // Apply tree_root_ops for this chunk. Order is preserved
            // so a SetMarker followed by an InstallPoiCache in the
            // same tick applies in that order; the cache reflects the
            // marker we just placed.
            //
            // Each op writes through the chunk's existing helper
            // methods (set_tree_root_marker / set_tree_unanchored)
            // which know how to keep tree_root_indices and the flag
            // bits in instance_state in sync.
            if let Some(ops) = tree_root_ops_by_chunk.get(&coord) {
                for op in ops {
                    match op {
                        ChunkTreeOp::SetMarker { lx, ly, lz } => {
                            new_chunk.set_tree_root_marker(*lx, *ly, *lz, true);
                        }
                        ChunkTreeOp::ClearMarker { lx, ly, lz } => {
                            new_chunk.set_tree_root_marker(*lx, *ly, *lz, false);
                        }
                        ChunkTreeOp::SetUnanchored { lx, ly, lz, on } => {
                            new_chunk.set_tree_unanchored(*lx, *ly, *lz, *on);
                        }
                        ChunkTreeOp::InstallPoi { cache } => {
                            new_chunk.poi_cache = Some(cache.clone());
                            // v0.24.70-structural-dirty: a fresh POI
                            // cache install means we just rebuilt; any
                            // pending dirty flag is now cleared.
                            // Subsequent rebuilds only fire if a wood
                            // transition re-dirties the chunk.
                            new_chunk.structural_dirty = false;
                        }
                    }
                }
            }

            // v0.24.23 propagation: replace chunk AND rebuild its
            // octree. Compute the AABB enclosing all edits for this
            // chunk so the dirty stamp is tight (instead of stamping
            // the whole chunk root, we stamp only the region that
            // actually changed). Consumers' `region_stable_since`
            // queries can then resolve "stable" for chunk regions
            // outside the edit AABB.
            //
            // Edit sources contributing to the AABB:
            //   - Block edits (set blocks)
            //   - State changes (per-cell animation/decay state)
            //   - Transition ops (visual lerps)
            //   - Tree-root marker ops (per-cell)
            // The `InstallPoiCache` op affects no specific cell, so
            // it contributes nothing to the AABB; the version bump
            // alone is enough to invalidate mesh.
            let mut edit_aabb: Option<crate::world::octree_region::Aabb3> = None;
            {
                let mut acc = |lx: i32, ly: i32, lz: i32| {
                    let cell_min = (lx, ly, lz);
                    let cell_max = (lx + 1, ly + 1, lz + 1);
                    edit_aabb = Some(match edit_aabb.take() {
                        None => crate::world::octree_region::Aabb3::new(cell_min, cell_max),
                        Some(a) => crate::world::octree_region::Aabb3::new(
                            (a.min.0.min(cell_min.0), a.min.1.min(cell_min.1), a.min.2.min(cell_min.2)),
                            (a.max.0.max(cell_max.0), a.max.1.max(cell_max.1), a.max.2.max(cell_max.2)),
                        ),
                    });
                };
                if let Some(edits) = block_edits_by_chunk.get(&coord) {
                    for &(lx, ly, lz, _) in edits { acc(lx, ly, lz); }
                }
                if let Some(changes) = state_changes_by_chunk.get(&coord) {
                    for &(lx, ly, lz, _) in changes { acc(lx, ly, lz); }
                }
                if let Some(ops) = transition_ops_by_chunk.get(&coord) {
                    for &(lx, ly, lz, _) in ops { acc(lx, ly, lz); }
                }
                if let Some(ops) = tree_root_ops_by_chunk.get(&coord) {
                    for op in ops {
                        match op {
                            ChunkTreeOp::SetMarker { lx, ly, lz }
                          | ChunkTreeOp::ClearMarker { lx, ly, lz }
                          | ChunkTreeOp::SetUnanchored { lx, ly, lz, .. } => {
                                acc(*lx, *ly, *lz);
                            }
                            ChunkTreeOp::InstallPoi { .. } => {} // per-chunk, no cell
                        }
                    }
                }
            }
            drop(entry);
            // Remesh gate (UNENDLICH_REMESH_GATE): if this chunk's only edits
            // this tick are micro-detail block growth (grass/flowers) and its
            // current LOD is too coarse to draw them, update the data silently —
            // no GPU re-mesh. Conservative: any state-change / transition /
            // tree-root op on the chunk, or any non-micro block, forces a normal
            // remesh. The data still updates, so growth appears when the chunk
            // later resolves to a finer LOD.
            let suppress_remesh = if self.remesh_gate {
                let has_other_ops = state_changes_by_chunk.contains_key(&coord)
                    || transition_ops_by_chunk.contains_key(&coord)
                    || tree_root_ops_by_chunk.contains_key(&coord);
                if !has_other_ops {
                    match block_edits_by_chunk.get(&coord) {
                        Some(edits) if !edits.is_empty() => {
                            let lod = crate::world::lod::desired_lod_for(
                                coord,
                                self.player_chunk,
                                crate::constants::runtime_render_distance(),
                            );
                            let blocks: Vec<BlockId> =
                                edits.iter().map(|&(_, _, _, b)| b).collect();
                            !crate::world::remesh_policy::chunk_requires_remesh(&blocks, lod)
                        }
                        _ => false,
                    }
                } else {
                    false
                }
            } else {
                false
            };
            // r191: age-only suppression — independent of the LOD gate,
            // always safe: nothing a mesher reads changed.
            let age_only_batch = !non_age_state_change
                && state_changes_by_chunk.contains_key(&coord)
                && !block_edits_by_chunk.contains_key(&coord)
                && !transition_ops_by_chunk.contains_key(&coord)
                && !tree_root_ops_by_chunk.contains_key(&coord);
            let suppress_remesh = suppress_remesh || age_only_batch;
            if suppress_remesh {
                self.remesh_skipped += 1;
            }
            self.replace_batch_edited_chunk(
                new_chunk,
                edit_aabb,
                suppress_remesh,
                block_edits_by_chunk.get(&coord),
            );

            // Field invalidation: any block edit in this chunk requires
            // checking if fields care. Run once per affected (block-edit)
            // local-coord.
            if let Some(state) = &self.fields {
                if let Some(edits) = block_edits_by_chunk.get(&coord) {
                    if let Some(grid_entry) = state.chunk_grids.get(&coord) {
                        let mut grid_w = grid_entry.write();
                        for &(lx, ly, lz, _) in edits {
                            grid_w.invalidate_for_edit(&state.registry, lx, ly, lz);
                        }
                    }
                }
            }
        }

        // Per-tick re-mesh trigger for chunks with active transitions.
        // For a 6-tick color animation to be visible, the chunk must be
        // re-meshed each tick the animation is in flight. We do this by
        // bumping the chunk version once per tick if any transitions
        // are active; the streamer picks up the version bump and queues
        // a re-mesh.
        //
        // Indexed: we maintain `chunks_with_transitions` (a HashSet of
        // ChunkCoord) in the apply phase above, so this sweep iterates
        // only the few chunks with active animations (typically 0-10)
        // instead of scanning all 2000+ loaded chunks. When the set is
        // empty (the common case), this is essentially free.
        //
        // We accept this cost knowing shader-side animation is the
        // long-term answer (no re-mesh, animate at fragment time).
        // For 2a, per-tick re-mesh is the simplest path that gives
        // visible smooth transitions without shader work.
        if !self.chunks_with_transitions.is_empty() {
            self.transition_remesh_tick = self.transition_remesh_tick.wrapping_add(1);
            let tick = self.transition_remesh_tick;
            let stride = self.transition_remesh_stride;
            // Clone-collect to avoid borrowing self twice (the get_mut
            // below also borrows self via self.chunks).
            let coords: Vec<ChunkCoord> =
                self.chunks_with_transitions.iter().copied().collect();
            for coord in coords {
                // Stride gate: re-mesh each transitioning chunk every `stride`
                // ticks, phase-offset by coord so the per-tick remesh count is
                // ~1/stride of the transition set spread evenly (no bursts).
                // stride==1 → always true → original every-tick behavior.
                if !transition_remesh_due(coord, tick, stride) {
                    continue;
                }
                if let Some(mut entry) = self.chunks.get_mut(&coord) {
                    // clone_for_edit already bumps version + sets dirty.
                    // We don't need to call bump_version_only afterwards.
                    let new_chunk = entry.value().clone_for_edit();
                    *entry = Arc::new(new_chunk);
                } else {
                    // Chunk was evicted while transitioning. Remove
                    // from the index so we don't keep trying.
                    self.chunks_with_transitions.remove(&coord);
                }
            }
        }

        // Stash the per-handler timing for the diagnostic log to read.
        // Convert HashMap to Vec preserving handler registration order.
        // We get registration order from the registry's handler list.
        let apply_us = t_apply.elapsed().as_micros() as u64;
        if let Some(state) = &mut self.ticks {
            state.last_handler_us = state.registry.handler_names()
                .into_iter()
                .map(|name| (name, handler_us_accum.get(name).copied().unwrap_or(0)))
                .collect();
            state.last_apply_us = apply_us;
            state.last_apply_chunks = apply_chunk_count;
        }
    }

    fn request_visible(&self) -> bool {
        // PERF (v0.23.6): the previous implementation called
        // `chunks_in_radius(player, rd)` which built and *sorted* a
        // Vec of every coord in the RD sphere — at RD=64 that's a
        // ~1.15M-entry allocate + 1.15M priority computations + a
        // sort of 1.15M items. Then we did 1.15M DashMap reads in
        // request_visible, then handed the survivors to
        // `loader.request_many` which took a mutex and did 2 HashSet
        // contains-checks per coord while holding it.
        //
        // The v0.23.5 RD=64 playtest showed mgr_update at 590ms/frame
        // with the breakdown attributing essentially all of it to
        // request_visible (drain=0.3ms, evict=0.0ms).
        //
        // v0.23.6 fix shape: walk Chebyshev shells outward from the
        // player, stop once we've accumulated enough requests.
        //
        // BUG (v0.23.8 playtest): the original budget was a single
        // global counter — once we'd accumulated 256 requests across
        // any shells, the walk broke. Near shells (0..=8) ALONE
        // exceed 256 coords (shell 4 = 386 by itself). With a moving
        // player, Near coords kept entering the request walk first
        // and the outer shells never got a turn — Mid/Far/VeryFar
        // were starved of generation indefinitely. v0.23.8 fix:
        // per-tier budgets.
        //
        // BUG (v0.23.9 playtest): per-tier classification per-coord
        // (`desired_lod_for` on every shell coord) was too eager.
        // request_visible took 40-66ms/frame because even saturated
        // tiers were iterating their full shells. v0.23.10 fix:
        // tier is purely a function of Chebyshev distance, which IS
        // the shell number. Classify the whole shell once, and skip
        // shells whose tier is already saturated. This drops the
        // Near hot path from O(shell_volume × per-coord lookup) to
        // O(64 × DashMap-read) per frame.
        const PER_TIER_BUDGET: usize = GPU_MAX_QUEUED_CHUNKS;

        let rd = runtime_render_distance();
        let rd_sq = rd * rd;
        let player = self.player_chunk;

        // Tier boundaries (Chebyshev distance). Clamped to rd so RD<32
        // degrades gracefully — outer bands become empty rather than
        // crossing the render distance.
        //
        // v0.24.41: same bands source as `desired_lod_for`. Reading
        // from `LodBands::current()` keeps this hot path identical to
        // the per-chunk classifier — without that, request_visible
        // would request chunks under one band scheme while the worker
        // classifies them under another, producing the wrong LOD.
        let bands = crate::world::lod::LodBands::current();
        let near_outer = bands.near.min(rd);
        let mid_outer  = bands.mid.min(rd);
        let far_outer  = bands.far.min(rd);

        let mut to_request: Vec<(ChunkCoord, i32)> = Vec::with_capacity(PER_TIER_BUDGET * 4);
        let mut near_count: usize = 0;
        let mut mid_count:  usize = 0;
        let mut far_count:  usize = 0;
        let mut vfar_count: usize = 0;

        // Sky cull (opt-in): resolve the model once so we can cheaply estimate
        // the surface elevation per candidate column and skip chunks that are
        // entirely empty sky above the terrain.
        let sky_cull_model = self.sky_cull_margin.map(|m| (self.generator.model_arc(), m));

        for shell in 0..=rd {
            // Classify the entire shell by its Chebyshev distance.
            // Mirrors `lod::desired_lod_for` exactly — but at shell
            // granularity, not coord granularity. One branch per
            // shell vs one per coord.
            //
            // If this shell's tier is already saturated, skip the
            // whole shell. This is the critical path improvement:
            // once Near is full at shell 4 (after ~64 fresh coords),
            // shells 5..=8 are skipped entirely.
            let tier_saturated = if shell <= near_outer {
                near_count >= PER_TIER_BUDGET
            } else if shell <= mid_outer {
                mid_count >= PER_TIER_BUDGET
            } else if shell <= far_outer {
                far_count >= PER_TIER_BUDGET
            } else {
                vfar_count >= PER_TIER_BUDGET
            };
            if tier_saturated { continue; }

            // Pick which counter we'll be incrementing — same tier
            // for the entire shell.
            let counter: &mut usize = if shell <= near_outer {
                &mut near_count
            } else if shell <= mid_outer {
                &mut mid_count
            } else if shell <= far_outer {
                &mut far_count
            } else {
                &mut vfar_count
            };

            // Shell-surface enumeration via face-round-robin ShellWalker
            // (v0.24.0a.2). Previously this was three nested loops
            // `dz: -s..=s`, `dy: -s..=s`, `dx: -s..=s` with a surface
            // filter — that walked the -z face first, which under
            // pipeline saturation caused chunks to load in a directional
            // slice rather than a sphere. See `lod::ShellWalker` for
            // the design.
            use crate::world::lod::ShellWalker;
            'shell_walk: for (dx, dy, dz) in ShellWalker::new(shell) {
                if dx*dx + dy*dy + dz*dz > rd_sq { continue; }

                let coord = ChunkCoord::new(
                    player.x + dx,
                    player.y + dy,
                    player.z + dz,
                );
                if self.chunks.contains_key(&coord) { continue; }

                // Sky cull (opt-in): skip chunks that are entirely empty sky
                // above the terrain at this column. The surface estimate is the
                // cheap 2D field; with the configured margin, real terrain
                // (which never exceeds surface_target by more than a few blocks)
                // is always kept.
                if let Some((model, margin)) = &sky_cull_model {
                    let cx = (coord.x * CHUNK_SIZE + CHUNK_SIZE / 2) as f64;
                    let cz = (coord.z * CHUNK_SIZE + CHUNK_SIZE / 2) as f64;
                    let surface = model.surface_target(cx, cz, 0.0);
                    if is_sky_chunk(surface, coord.y, *margin) {
                        continue;
                    }
                }

                if *counter >= PER_TIER_BUDGET {
                    // This shell's tier just filled mid-shell.
                    // Stop walking the rest of this shell; the
                    // outer-loop check will skip remaining shells
                    // of this tier next iteration.
                    break 'shell_walk;
                }
                *counter += 1;

                let prio = dx*dx + dy*dy + dz*dz;

                // r239 (mesh-vision-sched): vision-driven mesh gate. Build the
                // player view cone and DEFER chunks no cone needs (behind the
                // player / out of range). CONSERVATIVE first pass: occlusion is
                // OFF (always false), so we only skip clearly-unseen chunks —
                // never an in-view one — which can't produce an unmeshed hole
                // in front of the player. (The occlusion-backed cave win is a
                // follow-up once a line-of-sight source is wired; see
                // docs/design/mesh_vision_scheduling.md.) A deferred chunk's
                // voxels still load — only its MESH is skipped — and it
                // re-enters this walk (and meshes) once the player turns toward
                // it, since this runs every update.
                #[cfg(feature = "mesh-vision-sched")]
                {
                    use crate::sim::mesh_vision::{urgency_for_cones, MeshUrgency, ViewCone};
                    // chunk center in world blocks
                    let cc = [
                        (coord.x * CHUNK_SIZE + CHUNK_SIZE / 2) as f32,
                        (coord.y * CHUNK_SIZE + CHUNK_SIZE / 2) as f32,
                        (coord.z * CHUNK_SIZE + CHUNK_SIZE / 2) as f32,
                    ];
                    // player eye at the player chunk center
                    let eye = [
                        (self.player_chunk.x * CHUNK_SIZE + CHUNK_SIZE / 2) as f32,
                        (self.player_chunk.y * CHUNK_SIZE + CHUNK_SIZE / 2) as f32,
                        (self.player_chunk.z * CHUNK_SIZE + CHUNK_SIZE / 2) as f32,
                    ];
                    // range in blocks ≈ render distance (shells) × chunk size.
                    let range = (rd.max(1) * CHUNK_SIZE) as f32;
                    let cone = ViewCone::player(eye, self.player_view_yaw, range);
                    // occlusion OFF for now (conservative): never defer an
                    // in-view chunk.
                    let urgency = urgency_for_cones(&[cone], cc, |_, _| false);
                    if urgency == MeshUrgency::Defer {
                        // not needed by the cone — skip meshing this pass.
                        *counter -= 1; // don't spend budget on a deferred chunk
                        continue;
                    }
                }

                to_request.push((coord, prio));
            }

            // Early exit when every tier is saturated.
            if near_count >= PER_TIER_BUDGET
                && mid_count  >= PER_TIER_BUDGET
                && far_count  >= PER_TIER_BUDGET
                && vfar_count >= PER_TIER_BUDGET
            {
                break;
            }
        }

        if !to_request.is_empty() {
            self.loader.request_many(&to_request);
        }

        // Saturated ⇔ some tier hit its budget while more candidates were still
        // wanted. That's the only condition under which a later completion
        // (freeing queue capacity) opens new requestable work and should
        // trigger another walk; the caller uses this to gate the per-completion
        // re-walk via `should_rewalk`.
        near_count >= PER_TIER_BUDGET
            || mid_count >= PER_TIER_BUDGET
            || far_count >= PER_TIER_BUDGET
            || vfar_count >= PER_TIER_BUDGET
    }

    fn evict_distant(&self) {
        let max_dist_sq = self.eviction_distance * self.eviction_distance;
        let player = self.player_chunk;
        let world_root = self.world_root.clone();
        // Gather every out-of-range chunk with its distance.
        let candidates: Vec<(ChunkCoord, i32)> = self.chunks.iter()
            .filter_map(|e| {
                let d = e.key().distance_sq(player);
                if d > max_dist_sq { Some((*e.key(), d)) } else { None }
            })
            .collect();
        // Budget: cap evictions per call so a move that strands hundreds of
        // edited chunks doesn't block the frame on that many synchronous disk
        // saves. Evict the FARTHEST first (least likely to be re-needed); the
        // rest drain on subsequent moves. 0 = unlimited (original behavior).
        let evicted_coords = select_evictions(candidates, self.evict_budget);
        if evicted_coords.is_empty() {
            return;
        }
        let evict_set: std::collections::HashSet<ChunkCoord> = evicted_coords.iter().copied().collect();
        self.chunks.retain(|coord, chunk| {
            let keep = !evict_set.contains(coord);
            if !keep {
                // About to evict — save explicit edits if any.
                if let Some(root) = &world_root {
                    if !chunk.explicit_indices.is_empty() {
                        if let Err(e) = crate::storage::save_chunk_sync(root, chunk) {
                            log::warn!("Failed to save chunk {:?} on eviction: {}", coord, e);
                        }
                    }
                }
            }
            keep
        });
        // v0.24.23: drop octrees for evicted chunks. Their lifecycle
        // mirrors the chunks they came in with.
        for c in &evicted_coords {
            self.octrees.remove(c);
        }
        // Drop field grids for evicted chunks. They re-derive on reload.
        if let Some(state) = &self.fields {
            for c in &evicted_coords {
                state.chunk_grids.remove(c);
            }
        }
        // Drop tracker entries for evicted chunks so the tracker map
        // doesn't grow unbounded as the player explores. On reload,
        // a chunk's first-fire after re-entry uses the band's default
        // gap, which means it loses any catch-up history from before
        // eviction. That's fine: persistent catch-up across an evict-
        // reload boundary would require persisting tracker state to
        // disk alongside the chunk, which we explicitly defer for now.
        if let Some(state) = &self.ticks {
            for c in &evicted_coords {
                state.fire_tracker.evict_chunk(*c);
            }
        }
    }

    /// Save all currently-loaded dirty chunks. Call on world shutdown to
    /// ensure no edits are lost. Synchronous — blocks until all chunks are
    /// written.
    pub fn save_all(&self) -> usize {
        let Some(root) = &self.world_root else { return 0; };
        let mut saved = 0usize;
        for entry in self.chunks.iter() {
            let chunk = entry.value();
            if !chunk.explicit_indices.is_empty() {
                match crate::storage::save_chunk_sync(root, chunk) {
                    Ok(true) => saved += 1,
                    Ok(false) => {}
                    Err(e) => log::warn!("Failed to save chunk {:?}: {}", entry.key(), e),
                }
            }
        }
        saved
    }

    /// Read a field's value at a world position. Returns `None` if:
    ///   - Field overlays are not enabled on this manager
    ///   - The chunk containing this position is not loaded
    ///   - The named field is not registered
    ///
    /// Fields derive lazily — first read of a stale field triggers the
    /// derive function. Reads are cheap (DashMap lookup + grid sample) for
    /// already-derived fields.
    pub fn field_at(&self, field_name: &str, world_x: i32, world_y: i32, world_z: i32) -> Option<f32> {
        let state = self.fields.as_ref()?;
        let (idx, _def) = state.registry.get(field_name)?;
        let coord = block_to_chunk(world_x, world_y, world_z);
        let chunk_arc = self.chunks.get(&coord)?;
        let chunk = chunk_arc.value();
        let lx = world_x.rem_euclid(CHUNK_SIZE);
        let ly = world_y.rem_euclid(CHUNK_SIZE);
        let lz = world_z.rem_euclid(CHUNK_SIZE);

        let grid_lock = state.chunk_grids.get(&coord)?;
        let mut grid = grid_lock.write();
        let field_grid = grid.get_or_derive(idx, &state.registry, chunk);
        Some(field_grid.sample_at_block(lx, ly, lz))
    }

    /// Produce a stack-allocated snapshot of one chunk's scalar field.
    /// Cheap to call once per chunk and re-sample many times — the
    /// snapshot holds no lock, so per-plant samples avoid the
    /// `field_at` cost of (string hash + 2 DashMap reads + RwLock
    /// write acquire) per call.
    ///
    /// Tick handlers that iterate plants per chunk should use this:
    /// snapshot once at the top of the chunk loop, then sample
    /// per-plant.
    ///
    /// Returns None if fields aren't enabled, the named field isn't
    /// registered, or the chunk isn't loaded / has no field grid yet.
    pub fn chunk_field_snapshot(
        &self, field_name: &str, coord: ChunkCoord,
    ) -> Option<crate::world::fields::ChunkFieldSnapshot> {
        let state = self.fields.as_ref()?;
        let (idx, _def) = state.registry.get(field_name)?;
        let chunk_arc = self.chunks.get(&coord)?;
        let chunk = chunk_arc.value();
        let grid_lock = state.chunk_grids.get(&coord)?;
        let mut grid = grid_lock.write();
        let field_grid = grid.get_or_derive(idx, &state.registry, chunk);
        Some(field_grid.snapshot())
    }


    pub fn loaded_count(&self) -> usize { self.chunks.len() }

    /// r476: is the chunk containing this world cell loaded (data present)?
    /// Used by the "detached terrain" physics path to decide whether the
    /// ground under the player is real yet or still streaming in.
    pub fn is_loaded_at(&self, world_x: i32, world_y: i32, world_z: i32) -> bool {
        self.chunks.contains_key(&block_to_chunk(world_x, world_y, world_z))
    }

    /// r476: first AIR cell at or above `world_y` in this column, scanning up to
    /// `max_up` blocks. Used to settle a suspended player onto the real surface
    /// once terrain spawns under them (so they're not left embedded in rock).
    /// Returns None if the whole scanned span is solid or the column is unloaded.
    pub fn surface_above(&self, world_x: i32, world_y: i32, world_z: i32, max_up: i32) -> Option<i32> {
        if !self.is_loaded_at(world_x, world_y, world_z) {
            return None;
        }
        for dy in 0..max_up {
            let y = world_y + dy;
            let b = self.block_at(world_x, y, world_z);
            // Standable: this cell is air/non-solid AND the cell below is solid.
            if !b.is_solid() {
                return Some(y);
            }
        }
        None
    }

    // ─── F4: generator lifecycle (seed → fresh world) ───────────────────────
    //
    // The generator is no longer a startup-once singleton. On `Confirm ->
    // Launch(StartLocal{seed})` the bin calls `rebuild_for_seed` (compile-on-end)
    // to stand up a fresh world from the launch seed; the streamed chunks then
    // regenerate from it. The CPU surface is the verifiable ground truth (see
    // the headless test + examples/heightmap_collage.rs).

    /// The current generator's (mixed 32-bit) seed.
    pub fn generator_seed(&self) -> u32 {
        self.generator.seed()
    }

    /// Surface height at a world column. When the column's voxels are RESIDENT this
    /// reads the actual generated terrain (ground truth); it falls back to the CPU world
    /// model's analytic prediction only for not-yet-loaded columns.
    ///
    /// r583 — AUTHORITATIVE SURFACE (fixes the FP32/FP64 "float on air" bug,
    /// NEXT_SESSION_GUIDE §4a). The GPU fills voxels in FP32 (Asahi/Honeykrisp) while the
    /// CPU world model predicts in FP64, so the analytic surface drifts a block or two off
    /// the terrain that was actually generated — and flora/mobs/spawn placed at the
    /// prediction end up hovering or buried. Because `gpu_generator::read_back` hands us
    /// every generated voxel (they live in `self.chunks`), we prefer the TRUTH: take the
    /// prediction as an anchor, then scan the real blocks in a tight window for the topmost
    /// GROUND block (solid, excluding logs/leaves so we land on terrain, not a canopy) that
    /// has non-ground directly above. Non-resident columns read as all-Air here, so we
    /// return the prediction unchanged — distant spawn picks behave exactly as before.
    pub fn surface_y_at(&self, x: i32, z: i32, y_min: i32, y_max: i32) -> Option<i32> {
        let pred = self.generator.cpu().surface_y_at(x, z, y_min, y_max);
        if let Some(p) = pred {
            use crate::world::blocks::BlockId;
            let is_ground = |b: BlockId| {
                b.is_solid()
                    && !matches!(
                        b,
                        BlockId::OakLog
                            | BlockId::BirchLog
                            | BlockId::OakLeaves
                            | BlockId::BirchLeaves
                    )
            };
            // The FP gap is only a few blocks; scan a generous window around the anchor.
            let hi = (p + 8).min(y_max);
            let lo = (p - 24).max(y_min);
            let mut resident = false;
            let mut top: Option<i32> = None;
            let mut y = hi;
            while y >= lo {
                let b = self.block_at(x, y, z);
                if b != BlockId::Air {
                    resident = true;
                }
                if top.is_none() && is_ground(b) {
                    top = Some(y); // topmost ground cell (air/veg above, since top-down)
                    break;
                }
                y -= 1;
            }
            // Trust the voxels only if the column is actually loaded; else the prediction.
            if resident {
                return top.or(pred);
            }
        }
        pred
    }

    /// Choose a spawn position on the CURRENT surface: returns
    /// `(x, surface_y + 1, z)`, or `y_min + 1` if the column can't be sampled.
    pub fn pick_spawn(&self, x: i32, z: i32, y_min: i32, y_max: i32) -> (i32, i32, i32) {
        let y = self.surface_y_at(x, z, y_min, y_max).unwrap_or(y_min);
        (x, y + 1, z)
    }

    /// Swap in a freshly-built generator and drop all loaded/meshed chunk state
    /// so streaming regenerates the world from it. The loader is rebuilt so its
    /// worker threads pull from the new generator. After this the next `update`
    /// re-requests the shell around the player (`first_request_pending`).
    pub fn reset_with_generator(&mut self, generator: Arc<TerrainGenerator>) {
        // Repoint async loading at the new generator (drops old workers).
        self.loader = ChunkLoader::new(Arc::clone(&generator), self.num_workers);
        self.generator = generator;

        // Drop everything derived from the old seed. Clear in place so any Arc
        // clones held elsewhere observe the empty world rather than a stale one.
        self.chunks.clear();
        self.octrees.clear();
        self.chunks_with_transitions.clear();

        // Force a fresh interest walk + re-request around spawn.
        self.first_request_pending = true;
        self.last_player_world = None;
        self.last_walk_saturated = true;
        self.world_edit_tick.store(0, Ordering::Relaxed);
        // B1 (r102): a new world. Bump the epoch so any mesh result still in
        // flight from the previous world is identifiable as stale and dropped
        // by the renderer (see `world::mesh_epoch::mesh_result_is_current`).
        self.world_epoch = self.world_epoch.wrapping_add(1);
    }

    /// Rebuild the world generator from `seed` (the F4 launch path) and reset
    /// chunk state. Returns the generator-construction error if any.
    pub fn rebuild_for_seed(
        &mut self,
        seed: u64,
        backend: crate::world::terrain_generator::Backend,
        batch_size: u32,
    ) -> Result<(), crate::world::terrain_generator::TerrainGenError> {
        let generator = Arc::new(TerrainGenerator::new(seed, backend, batch_size)?);
        self.reset_with_generator(generator);
        Ok(())
    }
    pub fn pending_count(&self) -> usize { self.loader.pending_count() }
    pub fn generator(&self) -> &Arc<TerrainGenerator> { &self.generator }
    pub fn player_chunk(&self) -> ChunkCoord { self.player_chunk }

    /// Total remeshes skipped by the remesh gate this session (0 if the gate is
    /// off). Surfaced in the run summary so the saving is measurable.
    pub fn remesh_skipped(&self) -> u64 { self.remesh_skipped }

    /// c.13-instrumentation: cheap world-state snapshot for cross-
    /// referencing perf data with workload size.
    ///
    /// Returns (loaded_chunks, non_air_blocks). Iterates every block
    /// in every loaded chunk to count non-air blocks. Cost: O(chunks ×
    /// CHUNK_VOLUME). For rd=8 with ~33 chunks × 32768 blocks =
    /// ~1M block reads — sub-millisecond on modern hardware. Cheap
    /// enough to call at start and end of a run without perturbing
    /// measurements.
    ///
    /// Future extensions filed in DESIGN_NOTES — when we want plant
    /// counts, creature counts, etc., add them here as additional
    /// return values or as a richer snapshot struct.
    pub fn world_state_snapshot(&self) -> (usize, u64) {
        let loaded = self.chunks.len();
        let mut non_air_blocks: u64 = 0;
        for entry in self.chunks.iter() {
            let chunk = entry.value();
            for block in &chunk.blocks {
                if !block.is_air() {
                    non_air_blocks += 1;
                }
            }
        }
        (loaded, non_air_blocks)
    }

    /// Get an `Arc` clone of the world's field model. Used by the bin
    /// to populate `MeshRequest::world_model` so the mesher can sample
    /// climate fields per face for material-response tinting.
    pub fn model_arc(&self) -> Arc<crate::world::world_model::WorldModel> {
        self.generator.model_arc()
    }

    /// Current tick number — the canonical simulation clock. Returns
    /// `TickId::ZERO` if ticks aren't enabled. Anything wanting
    /// rollback-deterministic time should read THIS, not wall clock.
    pub fn current_tick(&self) -> crate::world::ticks::TickId {
        self.ticks.as_ref()
            .map(|s| s.scheduler.current_tick())
            .unwrap_or(crate::world::ticks::TickId::ZERO)
    }

    /// Per-handler microseconds spent during the most recent
    /// `tick_simulation` call. Empty until the first tick fires.
    /// Sum across all sub-ticks of the call (so at high tick speeds
    /// it scales up). Used by the bin's diagnostic log.
    pub fn last_handler_us(&self) -> &[(&'static str, u64)] {
        self.ticks.as_ref()
            .map(|s| s.last_handler_us.as_slice())
            .unwrap_or(&[])
    }

    /// Microseconds spent in tick-apply phase (Phase 3) of the most
    /// recent `tick_simulation`. Includes COW chunk clones and edit
    /// application.
    pub fn last_apply_us(&self) -> u64 {
        self.ticks.as_ref().map(|s| s.last_apply_us).unwrap_or(0)
    }

    /// How many distinct chunks were COW-cloned in the apply phase
    /// of the most recent `tick_simulation`. Useful with
    /// `last_apply_us` to reason about per-chunk apply cost.
    pub fn last_apply_chunks(&self) -> usize {
        self.ticks.as_ref().map(|s| s.last_apply_chunks).unwrap_or(0)
    }

    /// Direct read access to the chunks DashMap. Use sparingly — most
    /// callers want `block_at` or similar instead. Useful for debug
    /// overlays or systems that need to iterate loaded chunks.
    pub fn chunks_arc(&self) -> &dashmap::DashMap<ChunkCoord, Arc<Chunk>> {
        &self.chunks
    }

    /// Walk every loaded chunk and tally how many bytes it would take in
    /// SurfaceRun encoding vs the live `Vec<BlockId>` storage. Foundation
    /// telemetry for the chunk-compression track — the ratio tells us how
    /// much memory we'd recover by swapping the storage layer. Read once at
    /// run-summary time; the iteration is O(N_chunks × CHUNK_VOLUME) which
    /// is fine for that cadence but too expensive per-frame.
    pub fn compression_stats(&self) -> CompressionStats {
        use crate::world::surface_run::SurfaceRunChunk;
        use crate::constants::CHUNK_VOLUME;
        let raw_per_chunk =
            CHUNK_VOLUME * std::mem::size_of::<crate::world::blocks::BlockId>();
        let mut chunks = 0u64;
        let mut surface_run_bytes = 0u64;
        let mut worst_col_runs = 0u32;
        // Reuse one scratch across all chunks: clear+refill per chunk instead
        // of allocating a fresh SurfaceRunChunk (two Vecs) for each.
        let mut scratch = SurfaceRunChunk::empty();
        for entry in self.chunks.iter() {
            scratch.from_chunk_into(entry.value());
            let s = &scratch;
            chunks += 1;
            surface_run_bytes += s.byte_size() as u64;
            if let Some(&w) = s.col_runs.iter().max() {
                if w as u32 > worst_col_runs { worst_col_runs = w as u32; }
            }
        }
        let raw_bytes = chunks * raw_per_chunk as u64;
        CompressionStats {
            chunks,
            raw_bytes,
            surface_run_bytes,
            worst_col_runs,
        }
    }

    /// Current tick period in seconds. Returns the default if ticks
    /// aren't enabled. Combined with `current_tick`, these define
    /// simulation time: `sim_seconds = tick_id × tick_period`.
    pub fn tick_period(&self) -> f32 {
        self.ticks.as_ref()
            .map(|s| s.scheduler.period())
            .unwrap_or(crate::world::ticks::DEFAULT_TICK_PERIOD)
    }

    /// Iterate all currently-loaded chunk coordinates. Snapshots the keys
    /// to avoid holding a DashMap iterator across other operations.
    pub fn loaded_coords(&self) -> Vec<ChunkCoord> {
        self.chunks.iter().map(|r| *r.key()).collect()
    }

    pub fn get_chunk(&self, coord: ChunkCoord) -> Option<Arc<Chunk>> {
        self.chunks.get(&coord).map(|r| Arc::clone(r.value()))
    }

    /// Read the block at a world position. Returns Air if the chunk
    /// containing it isn't loaded.
    pub fn block_at(&self, world_x: i32, world_y: i32, world_z: i32) -> BlockId {
        let coord = block_to_chunk(world_x, world_y, world_z);
        let lx = world_x.rem_euclid(CHUNK_SIZE);
        let ly = world_y.rem_euclid(CHUNK_SIZE);
        let lz = world_z.rem_euclid(CHUNK_SIZE);
        match self.chunks.get(&coord) {
            Some(chunk) => chunk.get(lx, ly, lz),
            None => BlockId::Air,
        }
    }

    /// World-level hierarchical raycast. Walks the chunk grid the
    /// ray passes through, dispatches into each chunk's sparse
    /// octree for hierarchical-skip traversal.
    ///
    /// v0.24.23 propagation: this is the API the player block-
    /// selection raycast (and any other gameplay raycast that wants
    /// the skip-pass property) should call. Each chunk's octree
    /// answers in O(traversed nodes), not O(traversed cells), so a
    /// ray through homogeneous regions is near-free.
    ///
    /// Returns the first hit in front-to-back order, or `None` if
    /// the ray reaches `max_t` without striking any blocking block.
    /// `predicate` decides what counts as blocking (typical:
    /// `octree_raycast::block_is_blocking`, or `|b| b.is_solid() &&
    /// !b.is_transparent()` for opaque-only).
    ///
    /// Unloaded chunks are treated as air (the ray passes through
    /// them); the alternative — stopping at the boundary of unloaded
    /// terrain — would freeze player interaction when chunks stream
    /// in. Once the chunk loads, subsequent raycasts will see it.
    pub fn raycast_octree(
        &self,
        origin: [f32; 3],
        dir: [f32; 3],
        max_t: f32,
        predicate: impl Fn(BlockId) -> bool + Copy,
    ) -> Option<OctreeWorldRayHit> {
        // Normalize direction. Avoid division-by-zero downstream.
        let len2 = dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2];
        if len2 < 1e-12 {
            return None;
        }
        let inv_len = 1.0 / len2.sqrt();
        let dir = [dir[0] * inv_len, dir[1] * inv_len, dir[2] * inv_len];

        let cs = CHUNK_SIZE as f32;
        let mut t = 0.0f32;
        let mut current_origin = origin;
        // Hard cap on chunks visited per ray: 32 is enough for any
        // reasonable player-vision or mob-vision ray. Prevents
        // runaway iteration if a numeric degenerate edge case ever
        // failed to advance `t`.
        let mut hops = 0usize;
        while t < max_t && hops < 32 {
            hops += 1;
            // Which chunk are we currently in?
            let chunk_x = (current_origin[0] / cs).floor() as i32;
            let chunk_y = (current_origin[1] / cs).floor() as i32;
            let chunk_z = (current_origin[2] / cs).floor() as i32;
            let chunk_coord = ChunkCoord::new(chunk_x, chunk_y, chunk_z);
            let chunk_world_min = [
                chunk_x as f32 * cs,
                chunk_y as f32 * cs,
                chunk_z as f32 * cs,
            ];
            let chunk_world_max = [
                chunk_world_min[0] + cs,
                chunk_world_min[1] + cs,
                chunk_world_min[2] + cs,
            ];

            // Find when the ray exits this chunk.
            let t_chunk_exit = slab_exit_t(current_origin, dir, chunk_world_min, chunk_world_max);
            let remaining = max_t - t;
            let segment_t = t_chunk_exit.min(remaining);

            if let Some(octree_arc) = self.octrees.get(&chunk_coord) {
                let octree = octree_arc.value();
                let local_origin = [
                    current_origin[0] - chunk_world_min[0],
                    current_origin[1] - chunk_world_min[1],
                    current_origin[2] - chunk_world_min[2],
                ];
                if let Some(local_hit) = crate::world::octree_raycast::raycast_chunk(
                    octree,
                    local_origin,
                    dir,
                    segment_t,
                    predicate,
                ) {
                    // Convert chunk-local hit to world coordinates.
                    let world_cell = (
                        local_hit.cell.0 + chunk_x * CHUNK_SIZE,
                        local_hit.cell.1 + chunk_y * CHUNK_SIZE,
                        local_hit.cell.2 + chunk_z * CHUNK_SIZE,
                    );
                    let hit_t = t + local_hit.t;
                    let hit_point = [
                        origin[0] + dir[0] * hit_t,
                        origin[1] + dir[1] * hit_t,
                        origin[2] + dir[2] * hit_t,
                    ];
                    return Some(OctreeWorldRayHit {
                        t: hit_t,
                        world_cell,
                        block: local_hit.block,
                        normal: local_hit.normal,
                        point: hit_point,
                    });
                }
            }
            // No hit in this chunk (or chunk not loaded). Advance to
            // the next chunk along the ray. Step by `segment_t + ε`
            // so we cross the boundary cleanly.
            let step = segment_t + 1e-3;
            current_origin = [
                current_origin[0] + dir[0] * step,
                current_origin[1] + dir[1] * step,
                current_origin[2] + dir[2] * step,
            ];
            t += step;
        }
        None
    }

    /// Sample the biome at a world position. Goes to the generator since
    /// we don't store biome data on chunks. Doesn't require the chunk to
    /// be loaded.
    pub fn biome_at(&self, world_x: i32, world_y: i32, world_z: i32) -> BiomeSample {
        self.generator.cpu().biome_at_3d(world_x, world_y, world_z)
    }

    /// Set a block via copy-on-write. Returns true if the chunk was loaded.
    pub fn set_block(&self, world_x: i32, world_y: i32, world_z: i32, block: BlockId) -> bool {
        let coord = block_to_chunk(world_x, world_y, world_z);
        let lx = world_x.rem_euclid(CHUNK_SIZE);
        let ly = world_y.rem_euclid(CHUNK_SIZE);
        let lz = world_z.rem_euclid(CHUNK_SIZE);

        if let Some(entry) = self.chunks.get(&coord) {
            let mut new_chunk = entry.value().clone_for_edit();
            new_chunk.set(lx, ly, lz, block);
            drop(entry); // release the dashmap read lock before re-inserting
            // v0.24.23 propagation: rebuild octree + stamp dirty path so
            // consumers' `region_stable_since` queries see the edit.
            self.replace_edited_chunk(new_chunk, (lx, ly, lz));

            // Invalidate any field overlays that care about this edit.
            // Cheap: each field's predicate is one function call.
            if let Some(state) = &self.fields {
                if let Some(grid_entry) = state.chunk_grids.get(&coord) {
                    grid_entry.write().invalidate_for_edit(&state.registry, lx, ly, lz);
                }
            }

            // ---- Boundary neighbor invalidation ----
            //
            // If the edit is on a chunk-edge, the neighbor chunk's mesh
            // is now stale (its boundary faces depend on THIS chunk's
            // blocks via the neighbor-aware face-culling). Bump the
            // neighbor's version so the streaming loop detects it as
            // stale and resubmits to the meshing pool.
            //
            // Up to 3 neighbors get bumped (one per axis at the edge).
            // Neighbors outside [0, CHUNK_SIZE) are skipped (chunk not
            // loaded — nothing to invalidate).
            self.bump_neighbor_if_edge(coord, 0, lx);
            self.bump_neighbor_if_edge(coord, 1, ly);
            self.bump_neighbor_if_edge(coord, 2, lz);
            true
        } else {
            false
        }
    }

    /// Bump the version of a chunk without changing its blocks.
    /// Used for time-driven mesh refreshes (e.g. cloud animation):
    /// the streaming loop will detect the version mismatch with the
    /// renderer and resubmit the chunk for remeshing. Old mesh stays
    /// visible until the new one arrives — no flash.
    ///
    /// No-op if the chunk isn't loaded.
    pub fn bump_chunk_version(&self, coord: ChunkCoord) {
        if let Some(mut entry) = self.chunks.get_mut(&coord) {
            let mut new_chunk = (**entry.value()).clone();
            new_chunk.bump_version_only();
            *entry = Arc::new(new_chunk);
        }
    }

    /// If `local_coord` is at axis 0 or CHUNK_SIZE-1, bump the version
    /// of the neighbor chunk on that axis. Used by `set_block` to
    /// invalidate boundary-affected neighbor meshes.
    fn bump_neighbor_if_edge(&self, coord: ChunkCoord, axis: usize, local_coord: i32) {
        let delta = if local_coord == 0 {
            -1
        } else if local_coord == CHUNK_SIZE - 1 {
            1
        } else {
            return;
        };
        let mut nbr = coord;
        match axis {
            0 => nbr.x += delta,
            1 => nbr.y += delta,
            2 => nbr.z += delta,
            _ => return,
        }
        if let Some(mut entry) = self.chunks.get_mut(&nbr) {
            // Clone the chunk to bump its version (Arc is shared, so we
            // need a new Arc). Cheaper than rewriting; only the version
            // field changes.
            let mut new_nbr = (**entry.value()).clone();
            new_nbr.bump_version_only();
            *entry = Arc::new(new_nbr);
        }
    }

    /// Write per-block instance state at a world position. Same COW path
    /// as `set_block` — we clone the chunk, mutate the clone, swap the
    /// Arc atomically. Used by tick handlers to record lifecycle progress
    /// (age, decay, etc.) without changing the block id.
    ///
    /// Bumps chunk version, so the meshing pool re-meshes the chunk on
    /// next request — picks up the new color/state in the mesher's
    /// decoration pass.
    pub fn set_instance_state(&self, world_x: i32, world_y: i32, world_z: i32,
        state: crate::world::chunk::BlockInstanceState) -> bool {
        let coord = block_to_chunk(world_x, world_y, world_z);
        let lx = world_x.rem_euclid(CHUNK_SIZE);
        let ly = world_y.rem_euclid(CHUNK_SIZE);
        let lz = world_z.rem_euclid(CHUNK_SIZE);

        if let Some(mut entry) = self.chunks.get_mut(&coord) {
            let mut new_chunk = entry.value().clone_for_edit();
            new_chunk.set_instance_state(lx, ly, lz, state);
            *entry = Arc::new(new_chunk);
            true
        } else {
            false
        }
    }
}

/// Plug `ChunkManager` into the physics system as a block sampler. Returns
/// `Some(Air)` for unloaded chunks rather than `None` so falling-through-
/// unloaded-terrain feels like falling through space, not freezing in place.
impl crate::physics::BlockSampler for ChunkManager {
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        Some(self.block_at(x, y, z))
    }
}

/// Expose the live chunk world to the sim's perception/navigation layer: this is
/// the seam the cognition stack (vision line-of-sight, flow-field paths) reads,
/// so creatures perceive and route around *real* terrain instead of open air.
/// Unloaded chunks read as air (don't occlude / block paths phantomly).
impl crate::sim::physics::WorldQuery for ChunkManager {
    fn physics_at(&self, x: i32, y: i32, z: i32) -> crate::sim::physics::BlockPhysics {
        crate::sim::physics::physics_of_block(self.block_at(x, y, z))
    }
}

/// v0.24.24 propagation: octree-backed `BlockSampler` adapter.
///
/// Implements the same `BlockSampler` trait as `ChunkManager` itself,
/// but routes lookups through the per-chunk sparse octrees rather
/// than the dense `Chunk::blocks` array. Holds an `Arc` of the
/// octree map directly — no reference to the manager itself — so
/// callers can pass it across threads or move it into closures
/// without coupling to manager lifetime.
///
/// ## When to use which sampler
///
/// - **`&ChunkManager` (dense path):** single-point queries.
///   `block_at` is one memory access into `chunk.blocks[idx]`.
///   Wins when callers do scattered point queries.
///
/// - **`OctreeBlockSampler` (sparse path):** queries that touch
///   nearby points repeatedly OR that key off the cache-defer
///   pattern. Each `block_at` walks the octree O(depth) — slower
///   per-point than dense — but consumers using the sampler should
///   prefer hierarchical queries (`region_stable_since`, batch
///   `RegionRead`) which are unique to the octree path.
///
/// ## Why an adapter rather than swapping `ChunkManager`'s impl
///
/// Replacing `ChunkManager::block_at` to route through the octree
/// would silently slow down every existing `BlockSampler` consumer
/// (physics steps, fluid flow, mob vision DDA raycasts) for no
/// reason — the dense path serves them well. The adapter pattern
/// lets new code opt into octree-canonical sampling explicitly.
///
/// The opt-in is `ChunkManager::octree_sampler()`.
#[derive(Clone)]
pub struct OctreeBlockSampler {
    octrees: Arc<DashMap<ChunkCoord, Arc<OctreeArray>>>,
}

impl OctreeBlockSampler {
    /// Number of chunks currently in the sampler's view. Useful for
    /// tests and for logging when a sampler is detached from its
    /// originating manager.
    pub fn len(&self) -> usize {
        self.octrees.len()
    }

    pub fn is_empty(&self) -> bool {
        self.octrees.is_empty()
    }

    /// Direct access for callers that want to do their own
    /// hierarchical traversal (raycast, region read, defer-stable
    /// check). Returns the per-chunk octree if loaded.
    pub fn octree(&self, coord: ChunkCoord) -> Option<Arc<OctreeArray>> {
        self.octrees.get(&coord).map(|e| Arc::clone(e.value()))
    }
}

impl crate::physics::BlockSampler for OctreeBlockSampler {
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        let coord = block_to_chunk(x, y, z);
        let lx = x.rem_euclid(CHUNK_SIZE);
        let ly = y.rem_euclid(CHUNK_SIZE);
        let lz = z.rem_euclid(CHUNK_SIZE);
        match self.octrees.get(&coord) {
            Some(octree) => Some(octree.sample(lx, ly, lz)),
            // Unloaded → Air, matching ChunkManager's BlockSampler
            // policy so swapping samplers doesn't change "freezes
            // in unloaded chunks" behaviour.
            None => Some(BlockId::Air),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::chunk::Chunk;
    use crate::world::biomes::Biome;
    use crate::world::terrain_generator::TerrainGenerator;

    #[test]
    fn transition_stride_one_always_remeshes() {
        let c = ChunkCoord::new(3, -1, 7);
        for tick in 0..50 {
            assert!(transition_remesh_due(c, tick, 1), "stride 1 must remesh every tick");
        }
    }

    #[test]
    fn eviction_budget_zero_evicts_all() {
        let cands: Vec<(ChunkCoord, i32)> = (0..100)
            .map(|i| (ChunkCoord::new(i, 0, 0), i * i)).collect();
        assert_eq!(select_evictions(cands, 0).len(), 100);
    }

    #[test]
    fn eviction_budget_takes_farthest_first() {
        // Distances 0,1,4,9,...,81 for x=0..10. Budget 3 → the 3 farthest
        // (x=9,8,7 → dist 81,64,49).
        let cands: Vec<(ChunkCoord, i32)> = (0..10)
            .map(|i| (ChunkCoord::new(i, 0, 0), i * i)).collect();
        let picked = select_evictions(cands, 3);
        assert_eq!(picked.len(), 3);
        let xs: std::collections::HashSet<i32> = picked.iter().map(|c| c.x).collect();
        assert!(xs.contains(&9) && xs.contains(&8) && xs.contains(&7), "should evict farthest: got {:?}", xs);
        assert!(!xs.contains(&0), "should not evict nearest");
    }

    #[test]
    fn eviction_budget_above_count_evicts_all() {
        let cands: Vec<(ChunkCoord, i32)> = (0..5)
            .map(|i| (ChunkCoord::new(i, 0, 0), i * i)).collect();
        assert_eq!(select_evictions(cands, 50).len(), 5);
    }

    #[test]
    fn sky_chunk_above_surface_is_culled() {
        // Surface at y=70, margin 2 chunks (64 blocks) -> cutoff y=134.
        // A chunk whose bottom is at y=160 (chunk_y=5, 5*32=160) is sky.
        assert!(is_sky_chunk(70.0, 5, 2), "chunk bottom 160 > cutoff 134 -> sky");
    }

    #[test]
    fn chunk_straddling_surface_is_kept() {
        // Surface at y=70: the chunk containing it (chunk_y=2 -> 64..96) and
        // the margin band above must be kept (not sky).
        assert!(!is_sky_chunk(70.0, 2, 2), "surface chunk must load");
        assert!(!is_sky_chunk(70.0, 4, 2), "within-margin chunk (128) must load");
        // Deep chunks (below surface) are never sky.
        assert!(!is_sky_chunk(70.0, -3, 2), "underground chunk must load");
    }

    #[test]
    fn higher_surface_keeps_higher_chunks() {
        // chunk_y=7 -> bottom y=224. Plains (surface 70, cutoff 102): sky.
        // Mountain (surface 200, cutoff 232): 224 < 232 -> kept. Culling is
        // surface-relative, so raised biomes don't lose their geometry.
        assert!(is_sky_chunk(70.0, 7, 1), "plains: chunk bottom 224 is sky");
        assert!(!is_sky_chunk(200.0, 7, 1), "mountain: chunk bottom 224 still near surface");
    }

    #[test]
    fn transition_stride_remeshes_each_chunk_once_per_period() {
        // Over any window of `stride` consecutive ticks, a given chunk is due
        // exactly once — so it still animates, just at 1/stride the rate.
        for stride in [2u64, 3, 4, 6] {
            for (cx, cy, cz) in [(0, 0, 0), (5, 2, -9), (-12, 30, 4), (100, -7, 63)] {
                let c = ChunkCoord::new(cx, cy, cz);
                let due = (0..stride).filter(|&t| transition_remesh_due(c, t, stride)).count();
                assert_eq!(due, 1, "coord {:?} stride {stride}: due {due} times in a period", c);
            }
        }
    }

    #[test]
    fn transition_stride_spreads_load_across_ticks() {
        // With many chunks, each tick should fire roughly 1/stride of them —
        // the point of the phase offset (no single-tick burst of the whole set).
        let stride = 4u64;
        let coords: Vec<ChunkCoord> = (0..400)
            .map(|i| ChunkCoord::new(i % 20, (i / 20) % 5, i / 100))
            .collect();
        for tick in 0..stride {
            let firing = coords.iter().filter(|&&c| transition_remesh_due(c, tick, stride)).count();
            // Expect ~100 (400/4); allow generous spread for hash imbalance.
            assert!((40..=160).contains(&firing), "tick {tick}: {firing} fired (want ~100)");
        }
        // And the total over a full period equals the whole set (each fires once).
        let total: usize = (0..stride)
            .map(|t| coords.iter().filter(|&&c| transition_remesh_due(c, t, stride)).count())
            .sum();
        assert_eq!(total, coords.len());
    }

    fn make_manager() -> ChunkManager {
        // Test generator — minimal config; we won't actually load through it.
        let gen = Arc::new(TerrainGenerator::new(0xCAFE, crate::world::terrain_generator::Backend::Cpu, 1).unwrap());
        ChunkManager::new(gen, 1)
    }

    fn insert_chunk_at(mgr: &ChunkManager, coord: ChunkCoord) {
        // Insert a fresh empty chunk directly into the dashmap so we
        // don't have to wait for async loading. v0.24.23: also build
        // the octree so consumers that depend on `octree(coord)`
        // don't see None.
        let chunk = Chunk::empty(coord);
        let octree = OctreeArray::from_chunk(&chunk);
        mgr.install_chunk(chunk, octree);
    }

    // F4: the generator lifecycle. Proves the manager is NOT a startup-once
    // singleton — `rebuild_for_seed` swaps the generator and drops the old
    // world's chunks so streaming regenerates from the new seed. CPU backend,
    // fully headless (no bin).
    #[test]
    fn rebuild_for_seed_reseeds_generator_and_drops_world() {
        use crate::world::terrain_generator::Backend;
        let gen_a = Arc::new(TerrainGenerator::new(1337, Backend::Cpu, 1).unwrap());
        let mut mgr = ChunkManager::new(gen_a, 1);

        // Populate some loaded chunks + octrees (as if the old world streamed in).
        for x in 0..6 {
            insert_chunk_at(&mgr, ChunkCoord::new(x, 0, 0));
        }
        assert!(mgr.loaded_count() >= 6, "old world populated");
        let seed_before = mgr.generator_seed();
        let surf_before = mgr.surface_y_at(48, 48, 0, 200);

        // Relaunch with a different seed.
        mgr.rebuild_for_seed(42, Backend::Cpu, 1).unwrap();

        // Old world dropped; generator reseeded; will re-request around spawn.
        assert_eq!(mgr.loaded_count(), 0, "loaded chunks dropped on relaunch");
        assert!(mgr.octrees.is_empty(), "octrees dropped on relaunch");
        assert!(mgr.first_request_pending, "re-requests the shell after relaunch");
        assert_ne!(mgr.generator_seed(), seed_before, "generator reseeded");

        // The new seed yields a different surface (1337 vs 42 differ — see
        // heightmap_collage), so the spawn pick lands on the NEW world.
        let surf_after = mgr.surface_y_at(48, 48, 0, 200);
        assert!(surf_after.is_some());
        assert_ne!(surf_after, surf_before, "relaunch changed the surface");

        // pick_spawn sits one block above the new surface.
        let (sx, sy, sz) = mgr.pick_spawn(48, 48, 0, 200);
        assert_eq!((sx, sz), (48, 48));
        assert_eq!(sy, surf_after.unwrap() + 1);
    }

    #[test]
    fn reset_with_generator_can_round_trip_seeds() {
        use crate::world::terrain_generator::Backend;
        let mut mgr = ChunkManager::new(
            Arc::new(TerrainGenerator::new(7, Backend::Cpu, 1).unwrap()),
            1,
        );
        let s7 = mgr.generator_seed();
        mgr.reset_with_generator(Arc::new(TerrainGenerator::new(2024, Backend::Cpu, 1).unwrap()));
        assert_ne!(mgr.generator_seed(), s7);
        mgr.reset_with_generator(Arc::new(TerrainGenerator::new(7, Backend::Cpu, 1).unwrap()));
        assert_eq!(mgr.generator_seed(), s7, "same seed → same mixed generator seed (deterministic)");
    }

    // B1 (r102) regression: a world reset must advance the world epoch so that
    // mesh results still in flight from the previous world are identifiable as
    // stale (pre-fix there was no epoch, so a late result was indistinguishable
    // from a fresh one and could upload geometry for the old world).
    #[test]
    fn reset_bumps_world_epoch() {
        use crate::world::terrain_generator::Backend;
        let mut mgr = ChunkManager::new(
            Arc::new(TerrainGenerator::new(7, Backend::Cpu, 1).unwrap()),
            1,
        );
        let e0 = mgr.world_epoch();
        assert_eq!(e0, 0, "a fresh manager starts at epoch 0");

        mgr.reset_with_generator(Arc::new(TerrainGenerator::new(2024, Backend::Cpu, 1).unwrap()));
        let e1 = mgr.world_epoch();
        assert!(e1 > e0, "reset_with_generator must advance the epoch: {e1} > {e0}");

        mgr.rebuild_for_seed(99, Backend::Cpu, 1).unwrap();
        let e2 = mgr.world_epoch();
        assert!(e2 > e1, "rebuild_for_seed (F4 launch path) must advance the epoch: {e2} > {e1}");

        // The gate the renderer would use: a result tagged with a pre-reset
        // epoch is rejected; a current-epoch result is accepted.
        let now = mgr.world_epoch();
        assert!(
            !crate::world::mesh_epoch::mesh_result_is_current(e0, now),
            "a mesh result from epoch {e0} is stale at epoch {now}"
        );
        assert!(
            crate::world::mesh_epoch::mesh_result_is_current(now, now),
            "a mesh result from the current epoch is accepted"
        );
    }

    #[test]
    fn predicted_mesh_ahead_returns_loaded_chunks_along_motion() {
        std::env::set_var("UNENDLICH_PREFETCH", "1");
        let mut mgr = make_manager();
        mgr.player_chunk = ChunkCoord::new(0, 0, 0);

        // Stationary -> nothing to mesh ahead.
        mgr.last_vel_chunks = [0.0, 0.0, 0.0];
        assert!(mgr.predicted_mesh_ahead().is_empty(), "stationary -> none");

        // Moving +X but nothing loaded yet -> nothing to mesh (no data).
        mgr.last_vel_chunks = [1.0, 0.0, 0.0];
        assert!(mgr.predicted_mesh_ahead().is_empty(), "moving but unloaded -> none");

        // Load chunks ahead along +X; they should surface as mesh-ahead
        // candidates, and every returned coord must actually be loaded.
        for x in 1..=3 {
            insert_chunk_at(&mgr, ChunkCoord::new(x, 0, 0));
        }
        let got = mgr.predicted_mesh_ahead();
        assert!(!got.is_empty(), "loaded ahead chunks should be mesh-ahead candidates");
        assert!(got.iter().all(|c| mgr.chunks.contains_key(c)),
            "every mesh-ahead candidate must be a loaded chunk");
        assert!(got.iter().all(|c| c.x >= 0),
            "candidates should be ahead (+X), not behind");

        // Disabled -> none, even when moving with loaded chunks.
        std::env::set_var("UNENDLICH_PREFETCH", "0");
        assert!(mgr.predicted_mesh_ahead().is_empty(), "disabled -> none");
        std::env::remove_var("UNENDLICH_PREFETCH");
    }

    /// v0.24.23 propagation: world-level octree raycast finds a
    /// stone block placed in one chunk and hit-tests it correctly.
    /// Validates the chunk-walking dispatcher + local-to-world
    /// coordinate conversion.
    #[test]
    fn raycast_octree_finds_block() {
        let mgr = make_manager();
        let coord = ChunkCoord::new(0, 0, 0);
        insert_chunk_at(&mgr, coord);
        // Place a stone block at world position (15, 16, 16) — same
        // axis-aligned target as the per-chunk raycast unit test, but
        // routed through ChunkManager's world-level dispatcher.
        assert!(mgr.set_block(15, 16, 16, BlockId::Stone));

        let hit = mgr.raycast_octree(
            [0.5, 16.5, 16.5],
            [1.0, 0.0, 0.0],
            32.0,
            crate::world::octree_raycast::block_is_blocking,
        );
        let hit = hit.expect("ray must find the stone block");
        assert_eq!(hit.world_cell, (15, 16, 16));
        assert_eq!(hit.block, BlockId::Stone);
        assert_eq!(hit.normal, (-1, 0, 0));
        // t ≈ 14.5 (distance from x=0.5 to x=15.0).
        assert!((hit.t - 14.5).abs() < 0.01, "t={}", hit.t);
    }

    /// v0.24.23 propagation: the chunk-walking dispatcher correctly
    /// crosses chunk boundaries. Ray starts in chunk (0,0,0), exits,
    /// re-enters chunk (1,0,0), and finds a block placed there.
    #[test]
    fn raycast_octree_crosses_chunk_boundary() {
        let mgr = make_manager();
        // Load two adjacent chunks along +x.
        insert_chunk_at(&mgr, ChunkCoord::new(0, 0, 0));
        insert_chunk_at(&mgr, ChunkCoord::new(1, 0, 0));
        // Place a stone block at world x=40 — inside chunk (1,0,0) at
        // local x=8 — which is 39.5 units from the player at x=0.5.
        assert!(mgr.set_block(40, 16, 16, BlockId::Stone));

        let hit = mgr.raycast_octree(
            [0.5, 16.5, 16.5],
            [1.0, 0.0, 0.0],
            64.0,
            crate::world::octree_raycast::block_is_blocking,
        );
        let hit = hit.expect("ray must cross chunks and find the stone");
        assert_eq!(hit.world_cell, (40, 16, 16));
        assert_eq!(hit.block, BlockId::Stone);
        assert_eq!(hit.normal, (-1, 0, 0));
        // t ≈ 39.5 (distance from x=0.5 to x=40.0).
        assert!((hit.t - 39.5).abs() < 0.05, "t={}", hit.t);
    }

    /// v0.24.23 propagation: a ray that exits the loaded chunk
    /// without hitting anything returns None (the unloaded
    /// neighbor chunk is treated as air, not as opaque, so player
    /// raycasts don't false-hit at the loading frontier).
    #[test]
    fn raycast_octree_misses_through_empty() {
        let mgr = make_manager();
        let coord = ChunkCoord::new(0, 0, 0);
        insert_chunk_at(&mgr, coord);
        // No blocks set. Ray exits without hitting.
        let hit = mgr.raycast_octree(
            [16.0, 16.0, 16.0],
            [1.0, 0.0, 0.0],
            64.0,
            crate::world::octree_raycast::block_is_blocking,
        );
        assert!(hit.is_none());
    }

    /// v0.24.23 propagation: world_edit_tick advances on each
    /// set_block call. Consumers using the tick for cache-defer
    /// will see fresh values.
    #[test]
    fn world_edit_tick_advances_on_edits() {
        let mgr = make_manager();
        let coord = ChunkCoord::new(0, 0, 0);
        insert_chunk_at(&mgr, coord);
        let t0 = mgr.world_edit_tick();
        mgr.set_block(5, 5, 5, BlockId::Stone);
        let t1 = mgr.world_edit_tick();
        assert!(t1 > t0, "edit must advance the tick (was {} → {})", t0, t1);
        mgr.set_block(5, 5, 6, BlockId::Dirt);
        let t2 = mgr.world_edit_tick();
        assert!(t2 > t1);
    }

    /// v0.24.23 propagation: an edit stamps the octree's dirty
    /// path with the new tick, so a region read captured BEFORE
    /// the edit reports "not stable" after.
    #[test]
    fn octree_dirty_propagation_visible_via_manager() {
        let mgr = make_manager();
        let coord = ChunkCoord::new(0, 0, 0);
        insert_chunk_at(&mgr, coord);
        let snapshot_tick = mgr.world_edit_tick();
        let octree_before = mgr.octree(coord).expect("octree must exist");
        // Stable (no edits since snapshot).
        assert!(octree_before.region_stable_since(
            (4, 4, 4), (8, 8, 8), snapshot_tick as u32 + 1,
        ));
        mgr.set_block(5, 5, 5, BlockId::Stone);
        let octree_after = mgr.octree(coord).expect("octree must exist");
        // Now dirty.
        assert!(!octree_after.region_stable_since(
            (4, 4, 4), (8, 8, 8), snapshot_tick as u32 + 1,
        ), "octree must report dirty after set_block in region");
    }

    /// v0.24.24: `OctreeBlockSampler` returns the same blocks as the
    /// dense `ChunkManager::block_at` for the chunk that's loaded.
    /// Parity is the correctness guarantee that lets new code adopt
    /// the octree sampler without changing observable behaviour.
    #[test]
    fn octree_sampler_parity_with_dense_path() {
        use crate::physics::BlockSampler;
        let mgr = make_manager();
        let coord = ChunkCoord::new(0, 0, 0);
        insert_chunk_at(&mgr, coord);
        // Place a few blocks so the octree's not all-air.
        mgr.set_block(5, 5, 5, BlockId::Stone);
        mgr.set_block(10, 5, 5, BlockId::Dirt);
        mgr.set_block(0, 0, 0, BlockId::Stone);
        mgr.set_block(31, 31, 31, BlockId::Dirt);

        let sampler = mgr.octree_sampler();
        // Sampler should be detachable; we're holding mgr too, but
        // the Arc cloning means the sampler keeps its own view.
        assert!(!sampler.is_empty());

        // Check the set blocks and a few air positions.
        for &(x, y, z, expected) in &[
            (5, 5, 5, BlockId::Stone),
            (10, 5, 5, BlockId::Dirt),
            (0, 0, 0, BlockId::Stone),
            (31, 31, 31, BlockId::Dirt),
            (16, 16, 16, BlockId::Air),
            (7, 7, 7, BlockId::Air),
        ] {
            let dense = mgr.block_at(x, y, z);
            let sparse = sampler.block_at(x, y, z).unwrap();
            assert_eq!(dense, expected, "dense path mismatch at ({}, {}, {})", x, y, z);
            assert_eq!(sparse, expected, "octree sampler mismatch at ({}, {}, {})", x, y, z);
            assert_eq!(dense, sparse, "parity broken at ({}, {}, {})", x, y, z);
        }
    }

    /// v0.24.24: unloaded chunks sample as Air (not None or panic).
    /// Matches `ChunkManager::block_at` behaviour so swapping
    /// samplers in physics doesn't surprise the caller with sudden
    /// "freeze in unloaded chunks" semantics.
    #[test]
    fn octree_sampler_returns_air_for_unloaded() {
        use crate::physics::BlockSampler;
        let mgr = make_manager();
        let sampler = mgr.octree_sampler();
        // No chunks loaded → empty.
        assert!(sampler.is_empty());
        // Should return Some(Air) for any position.
        assert_eq!(sampler.block_at(0, 0, 0), Some(BlockId::Air));
        assert_eq!(sampler.block_at(-100, 50, 1000), Some(BlockId::Air));
    }

    /// Editing a block in the INTERIOR of a chunk bumps that chunk's
    /// version. This is the basic "mesh now stale" signal that the
    /// streaming loop watches for.
    #[test]
    fn set_block_bumps_chunk_version() {
        let mgr = make_manager();
        let coord = ChunkCoord::new(0, 0, 0);
        insert_chunk_at(&mgr, coord);
        let v0 = mgr.get_chunk(coord).unwrap().version;
        // Edit a block in the middle of the chunk.
        let world_x = CHUNK_SIZE / 2;
        let world_y = CHUNK_SIZE / 2;
        let world_z = CHUNK_SIZE / 2;
        let ok = mgr.set_block(world_x, world_y, world_z, BlockId::Stone);
        assert!(ok, "set_block should succeed for loaded chunk");
        let v1 = mgr.get_chunk(coord).unwrap().version;
        assert!(v1 > v0,
            "chunk version should bump after edit: v0={}, v1={}", v0, v1);
    }

    /// REGRESSION for the chunk-flash fix: editing a block on a chunk
    /// boundary must bump the NEIGHBOR chunk's version too. Otherwise
    /// the neighbor's mesh shows stale boundary faces (wall to a
    /// just-broken hole, or no wall to a just-placed block) and the
    /// version-aware streaming loop won't resubmit it.
    #[test]
    fn set_block_at_chunk_boundary_bumps_neighbor_version() {
        let mgr = make_manager();
        let coord_a = ChunkCoord::new(0, 0, 0);
        let coord_b_pos_x = ChunkCoord::new(1, 0, 0);  // neighbor to +X
        let coord_b_neg_x = ChunkCoord::new(-1, 0, 0); // neighbor to -X
        insert_chunk_at(&mgr, coord_a);
        insert_chunk_at(&mgr, coord_b_pos_x);
        insert_chunk_at(&mgr, coord_b_neg_x);
        let v_a_0 = mgr.get_chunk(coord_a).unwrap().version;
        let v_pos_0 = mgr.get_chunk(coord_b_pos_x).unwrap().version;
        let v_neg_0 = mgr.get_chunk(coord_b_neg_x).unwrap().version;

        // Edit at the +X edge of chunk A (local x = CHUNK_SIZE-1).
        let world_x = CHUNK_SIZE - 1;
        let world_y = 5;
        let world_z = 5;
        mgr.set_block(world_x, world_y, world_z, BlockId::Stone);

        let v_a_1 = mgr.get_chunk(coord_a).unwrap().version;
        let v_pos_1 = mgr.get_chunk(coord_b_pos_x).unwrap().version;
        let v_neg_1 = mgr.get_chunk(coord_b_neg_x).unwrap().version;

        assert!(v_a_1 > v_a_0, "edit chunk's version must bump");
        assert!(v_pos_1 > v_pos_0,
            "+X neighbor's version must bump after edit on +X edge: was {}, now {}",
            v_pos_0, v_pos_1);
        // -X neighbor was NOT on the edited edge; should NOT bump.
        assert_eq!(v_neg_1, v_neg_0,
            "-X neighbor's version must NOT bump when edit is on +X edge");
    }

    /// Edit in chunk INTERIOR should not bump any neighbors. Saves
    /// unnecessary mesh resubmissions.
    #[test]
    fn set_block_interior_does_not_bump_neighbors() {
        let mgr = make_manager();
        let coord_a = ChunkCoord::new(0, 0, 0);
        let coord_b = ChunkCoord::new(1, 0, 0);
        insert_chunk_at(&mgr, coord_a);
        insert_chunk_at(&mgr, coord_b);
        let v_b_0 = mgr.get_chunk(coord_b).unwrap().version;

        // Edit far from any boundary.
        mgr.set_block(CHUNK_SIZE / 2, CHUNK_SIZE / 2, CHUNK_SIZE / 2, BlockId::Stone);

        let v_b_1 = mgr.get_chunk(coord_b).unwrap().version;
        assert_eq!(v_b_1, v_b_0,
            "interior edit must not bump neighbors: was {}, now {}", v_b_0, v_b_1);
    }

    /// Edit at the +Y top edge bumps the +Y neighbor (vertical seams matter too).
    #[test]
    fn set_block_at_y_edge_bumps_y_neighbor() {
        let mgr = make_manager();
        let coord_a = ChunkCoord::new(0, 0, 0);
        let coord_above = ChunkCoord::new(0, 1, 0);
        insert_chunk_at(&mgr, coord_a);
        insert_chunk_at(&mgr, coord_above);
        let v_above_0 = mgr.get_chunk(coord_above).unwrap().version;

        mgr.set_block(5, CHUNK_SIZE - 1, 5, BlockId::Stone);

        let v_above_1 = mgr.get_chunk(coord_above).unwrap().version;
        assert!(v_above_1 > v_above_0,
            "above-Y neighbor's version must bump on top-edge edit");
    }

    /// Edit at a CORNER (3-axis edge) bumps all 3 neighbors (1 per axis).
    /// Diagonal neighbors aren't bumped — the mesh seam logic only needs
    /// face-adjacent neighbors.
    #[test]
    fn set_block_at_corner_bumps_three_neighbors() {
        let mgr = make_manager();
        let coord_a = ChunkCoord::new(0, 0, 0);
        let n_x = ChunkCoord::new(1, 0, 0);
        let n_y = ChunkCoord::new(0, 1, 0);
        let n_z = ChunkCoord::new(0, 0, 1);
        insert_chunk_at(&mgr, coord_a);
        insert_chunk_at(&mgr, n_x);
        insert_chunk_at(&mgr, n_y);
        insert_chunk_at(&mgr, n_z);
        let v_x_0 = mgr.get_chunk(n_x).unwrap().version;
        let v_y_0 = mgr.get_chunk(n_y).unwrap().version;
        let v_z_0 = mgr.get_chunk(n_z).unwrap().version;

        // Edit at the +X+Y+Z corner of chunk A.
        let edge = CHUNK_SIZE - 1;
        mgr.set_block(edge, edge, edge, BlockId::Stone);

        assert!(mgr.get_chunk(n_x).unwrap().version > v_x_0, "+X neighbor not bumped");
        assert!(mgr.get_chunk(n_y).unwrap().version > v_y_0, "+Y neighbor not bumped");
        assert!(mgr.get_chunk(n_z).unwrap().version > v_z_0, "+Z neighbor not bumped");
    }

    /// Multi-interest streaming: passing a single point should match
    /// the legacy single-point `update`. This is the contract that
    /// lets call sites migrate from `update` to
    /// `update_with_interest_points` without behavior change.
    #[test]
    fn single_interest_matches_legacy_update() {
        let mut a = make_manager();
        let mut b = make_manager();
        // Same operations through different APIs.
        a.update(100, 70, 200);
        b.update_with_interest_points(&[(100, 70, 200)]);
        assert_eq!(a.player_chunk(), b.player_chunk(),
            "primary interest point should set player_chunk identically");
    }

    /// Empty interest points (dedicated-server case): the call must
    /// not panic and must not advance player_chunk. This is critical
    /// because a server tick with no local players SHOULD still
    /// drain the loader's completed chunks (otherwise they pile up).
    #[test]
    fn empty_interest_points_does_not_crash() {
        let mut mgr = make_manager();
        let before = mgr.player_chunk();
        mgr.update_with_interest_points(&[]);
        let after = mgr.player_chunk();
        assert_eq!(before, after,
            "no interest points should leave player_chunk untouched");
    }

    /// Two interest points: today the implementation uses the FIRST
    /// as canonical. Test pins this behavior so future improvements
    /// (e.g. taking the centroid, taking a union of disks) require
    /// updating the test deliberately rather than silently drifting.
    #[test]
    fn first_interest_point_is_canonical_for_now() {
        let mut mgr = make_manager();
        // Two distant interest points — first wins.
        mgr.update_with_interest_points(&[
            (0, 64, 0),
            (10000, 64, 10000),
        ]);
        let pc = mgr.player_chunk();
        assert_eq!(pc, block_to_chunk(0, 64, 0),
            "first interest point should set player_chunk");
    }

    #[test]
    fn rewalk_gate_skips_steady_state_completions() {
        // Player still, walk fully enqueued (not saturated): a stream of
        // completions must NOT re-trigger the full-sphere walk. This is the
        // ~31ms/frame settling case.
        assert!(!should_rewalk(false, true, false, false),
            "completion with an unsaturated last walk should not re-walk");
        // ...but if the last walk WAS saturated, a completion frees capacity →
        // re-walk to enqueue the chunks we couldn't fit.
        assert!(should_rewalk(false, true, false, true),
            "completion after a saturated walk should re-walk");
        // Player crossing a chunk boundary always re-walks (new desired set),
        // regardless of saturation or completions.
        assert!(should_rewalk(true, false, false, false));
        // The very first walk must happen even when nothing else changed.
        assert!(should_rewalk(false, false, true, false));
        // Truly idle frame: no move, no completion, already walked once.
        assert!(!should_rewalk(false, false, false, true),
            "idle frame should not re-walk");
    }

    // Use the unused-import suppression for Biome (might or might not be needed).
    #[allow(dead_code)]
    fn _unused_biome_marker() -> Biome { Biome::Plains }
}
