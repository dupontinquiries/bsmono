//! Simulation tick system.
//!
//! Distinct from render frames: ticks advance the *world*, not the screen.
//! Plant growth, leaf decay, fluid flow, mob AI — anything that changes the
//! world over time runs as a registered tick handler at a fixed rate.
//!
//! Why fixed-rate (vs per-frame): simulation determinism. Same wall-clock
//! time should produce the same number of growth steps regardless of FPS,
//! so a slow frame doesn't visibly stall world evolution and a fast frame
//! doesn't accelerate it. Also makes lifecycle balancing easier — we set
//! "plant grows in N ticks", not "plant grows in M frames at 60 Hz."
//!
//! Tick rate: 2 Hz (one tick every 0.5 real seconds). Slow enough that
//! plants visibly progress through stages while not dominating CPU. Should
//! become configurable per-handler if some systems want faster (fluids?)
//! or slower (climate change?).
//!
//! Concurrency: today the system is single-threaded — handlers run
//! sequentially in registration order. Future work is to parallelize
//! handlers that don't conflict (per-chunk plant ticks across chunks
//! could run concurrently). The deferred-edit pattern (handlers append
//! to `block_edits`, applied after all handlers run) keeps things
//! conflict-free even when sequential.

use crate::world::blocks::BlockId;
use crate::world::chunk::{Chunk, ChunkCoord};
use dashmap::DashMap;
use std::sync::Arc;

/// Default tick period in real seconds.
pub const DEFAULT_TICK_PERIOD: f32 = 0.5;

/// Monotonic tick counter — the **master clock** of the simulation.
///
/// **Why a typed integer instead of f64 wall time:** rollback netcode
/// requires that "the same tick" be unambiguous across machines.
/// Wall-clock time can never agree (different drift, different frame
/// rates, different epochs). A typed `TickId` is the canonical
/// reference everywhere a system needs "what time is it for the
/// simulation."
///
/// All time-dependent simulation logic — wind direction, plant
/// lifecycle aging, sky phase, fluid flow — should derive its
/// effective time from `tick_id × tick_period_secs`, NOT from real
/// wall time. That way the same `TickId` always produces the same
/// effects regardless of when (or even whether) wall-clock time has
/// elapsed since startup. Replay = re-run from a saved state with
/// the same TickIds = same outputs.
///
/// Wraps a `u64`. At 2 Hz, u64 doesn't wrap for ~2.9e11 years. Even
/// at 10kHz it's safe for billions of years. Don't worry about
/// wraparound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TickId(pub u64);

impl TickId {
    pub const ZERO: TickId = TickId(0);
    pub fn next(self) -> TickId { TickId(self.0 + 1) }
    pub fn raw(self) -> u64 { self.0 }
    /// Convert to "simulation seconds" using the standard tick period.
    /// Useful for handlers that want a real-seconds-equivalent value
    /// (e.g. wind direction phase computed in seconds).
    pub fn to_sim_seconds(self, tick_period: f32) -> f64 {
        (self.0 as f64) * (tick_period as f64)
    }
}

impl std::fmt::Display for TickId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "T{}", self.0)
    }
}

/// A pending block edit produced by a tick handler. Applied to the world
/// after all handlers have run for this tick.
#[derive(Debug, Clone, Copy)]
pub struct DeferredEdit {
    pub world_x: i32,
    pub world_y: i32,
    pub world_z: i32,
    pub new_block: BlockId,
}

/// A pending instance-state change produced by a tick handler. Distinct
/// from `DeferredEdit` because instance state is per-block-instance
/// metadata (lifecycle stage, decay age, etc.), not a block-id swap.
/// Applied at end of tick like edits.
#[derive(Debug, Clone, Copy)]
pub struct DeferredStateChange {
    pub world_x: i32,
    pub world_y: i32,
    pub world_z: i32,
    pub new_state: crate::world::chunk::BlockInstanceState,
}

/// Deferred operation on the per-block transitions map. Like
/// `DeferredStateChange` but for `Chunk::transitions` (visual
/// animations in flight). Two variants: Start a new transition,
/// Finish/remove an existing one. Applied at end of tick like
/// other deferred operations.
///
/// The handler also has the option of NOT enqueueing a Finish but
/// instead enqueueing a final `DeferredStateChange` that commits
/// the to_stage to instance_state — the next mesh produces the
/// committed visual. We provide explicit Finish so the handler can
/// also clear the transition entry without bumping every block in
/// the chunk.
#[derive(Debug, Clone, Copy)]
pub enum DeferredTransitionOp {
    Start {
        world_x: i32, world_y: i32, world_z: i32,
        transition: crate::world::chunk::BlockTransition,
    },
    Finish {
        world_x: i32, world_y: i32, world_z: i32,
    },
}

/// Deferred operation on a chunk's tree-root marker index OR its
/// POI cache. Tree-root markers are per-block metadata stored in
/// `Chunk::tree_root_indices` + the corresponding
/// `BlockInstanceState::FLAG_TREE_ROOT` flag. The POI cache holds
/// pre-computed structural summaries (anchors, cluster sizes, etc.)
/// consumed by the marker stickiness and growth/shrink passes. The
/// lifecycle handler enqueues these ops; the tick driver applies them
/// in the same apply phase that commits block edits and state changes.
///
/// Variants:
/// - `SetMarker`: place a root marker at this position. Block at that
///   position MUST be wood (OakLog/BirchLog) at apply time.
/// - `ClearMarker`: remove a root marker (also clears the unanchored
///   flag). No-op if no marker existed.
/// - `SetUnanchored`: toggle the unanchored flag on an existing marker.
///   No-op if no marker exists at this position.
/// - `InstallPoiCache`: replace the chunk's POI cache with the given
///   freshly-computed cache. Built by the lifecycle handler's POI pass.
#[derive(Debug, Clone)]
pub enum DeferredTreeRootOp {
    SetMarker { world_x: i32, world_y: i32, world_z: i32 },
    ClearMarker { world_x: i32, world_y: i32, world_z: i32 },
    SetUnanchored {
        world_x: i32, world_y: i32, world_z: i32, unanchored: bool,
    },
    InstallPoiCache {
        chunk: ChunkCoord,
        cache: crate::world::chunk::ChunkPoiCache,
    },
}

/// Context passed to each tick handler. Read-only access to world state;
/// edits are queued via `block_edits` / `state_changes` and applied after
/// this tick's handlers finish.
pub struct TickContext<'a> {
    /// All currently-loaded chunks. Handlers may iterate but should not
    /// mutate directly — append to `block_edits` / `state_changes` instead.
    pub chunks: &'a DashMap<ChunkCoord, Arc<Chunk>>,
    /// Optional field reader for handlers that depend on world fields
    /// (moisture, wind, light). `None` if fields aren't enabled. Keyed
    /// by field name; returns the sampled value at world position or
    /// None if the field isn't registered or the chunk isn't loaded.
    ///
    /// The reader is a closure to keep TickContext free of structural
    /// dependencies on FieldRegistry's concrete type — the bin can wire
    /// any field source it likes (real fields, mock fields for testing,
    /// etc.).
    pub field_reader: Option<&'a dyn Fn(&str, i32, i32, i32) -> Option<f32>>,
    /// Optional **per-chunk** field reader that returns a stack-allocated
    /// snapshot of one chunk's field grid. Handlers that loop over many
    /// plants in a single chunk should call this ONCE at the top of the
    /// chunk loop and sample from the snapshot per-plant — much cheaper
    /// than calling `field_reader` per plant (each `field_reader` call
    /// pays a string-hash + 2 DashMap reads + an RwLock write acquire).
    ///
    /// `None` in test contexts that don't wire it up. Handlers should
    /// fall back to `field_reader` when this is None.
    pub chunk_field_reader: Option<&'a dyn Fn(&str, ChunkCoord)
        -> Option<crate::world::fields::ChunkFieldSnapshot>>,
    /// Player's current chunk coord, for distance-based tick culling. If
    /// None, handlers should tick everything (test contexts, etc.).
    pub player_chunk: Option<ChunkCoord>,
    /// **Canonical simulation clock** — the integer tick number this
    /// handler is running for. Use this (not `world_time`) for any
    /// time-dependent decision so the same tick always produces the
    /// same result regardless of wall-clock differences across
    /// machines. Required for rollback netcode.
    pub tick_id: TickId,
    /// Tick period in real seconds — the constant used to convert
    /// between TickId and simulation seconds. Stored here so handlers
    /// can do `tick_id.to_sim_seconds(tick_period)` if they want a
    /// seconds-valued time argument (wind direction, etc.).
    pub tick_period: f32,
    /// Real seconds since last tick. Equal to `tick_period` in the
    /// normal case but a tick-driver may pass a different value for
    /// catch-up ticks. Handlers using this for time integration should
    /// switch to `tick_id` if rollback determinism matters.
    pub tick_dt: f32,
    /// Simulation seconds since the world's tick 0. Equal to
    /// `tick_id.to_sim_seconds(tick_period)`. Provided for legacy
    /// handlers that want an f64 seconds value; PREFER `tick_id` for
    /// new code.
    pub world_time: f64,
    /// Distance-based scheduler parameters. Handlers consult these
    /// (along with `player_chunk` and `tick_id`) to decide which chunks
    /// to tick this frame and at what effective dt. None means the
    /// scheduler isn't configured (test contexts) and handlers should
    /// fall back to their default behavior.
    pub schedule: Option<crate::world::scheduler::TickScheduleParams>,
    /// Per-(handler, chunk) last-fire tracker for catch-up. When a
    /// handler decides "yes this chunk fires this tick," it queries
    /// the tracker for the elapsed-tick gap since its last fire on
    /// this chunk, applies that many ticks of evolution, then marks
    /// the fire. None means no tracker (test contexts) — handler
    /// should fall back to the band's default n.
    pub fire_tracker: Option<&'a crate::world::scheduler::LastFireTracker>,
    /// Pending block-id edits — appended by handlers, applied by tick driver.
    pub block_edits: Vec<DeferredEdit>,
    /// Pending instance-state changes — same pattern as block_edits but
    /// for finer-grained per-block metadata.
    pub state_changes: Vec<DeferredStateChange>,
    /// Pending transitions-map operations (Start / Finish) — same
    /// deferred pattern. Applied at end of tick alongside the others.
    pub transition_ops: Vec<DeferredTransitionOp>,
    /// Pending tree-root marker operations. Applied at end of tick
    /// alongside the others. Used by the lifecycle handler's tree
    /// discovery + stickiness passes (Phase A of the tree-topology
    /// rework). Empty for handlers that don't touch trees.
    pub tree_root_ops: Vec<DeferredTreeRootOp>,
}

impl<'a> TickContext<'a> {
    /// Should this chunk tick this frame? Returns true if `player_chunk`
    /// is None (no culling) or if the chunk is within `radius` chunks
    /// (Chebyshev distance) of the player.
    pub fn chunk_in_tick_range(&self, coord: ChunkCoord, radius: i32) -> bool {
        match self.player_chunk {
            None => true,
            Some(p) => {
                let dx = (coord.x - p.x).abs();
                let dy = (coord.y - p.y).abs();
                let dz = (coord.z - p.z).abs();
                dx.max(dy).max(dz) <= radius
            }
        }
    }

    /// **Deterministic** iteration order over loaded chunks. Handlers
    /// MUST iterate via this method (not `chunks.iter()` directly) for
    /// rollback determinism: DashMap's native iteration order varies
    /// between runs and across machines, which would make replay
    /// produce different edit orderings.
    ///
    /// Returns coords sorted lexicographically by (x, y, z). Cost is
    /// O(N log N) per tick but N is bounded by render-distance² which
    /// is small (hundreds, not thousands).
    pub fn sorted_chunk_coords(&self) -> Vec<ChunkCoord> {
        let mut coords: Vec<ChunkCoord> = self.chunks.iter()
            .map(|entry| *entry.key())
            .collect();
        coords.sort_by(|a, b|
            a.x.cmp(&b.x)
                .then_with(|| a.y.cmp(&b.y))
                .then_with(|| a.z.cmp(&b.z))
        );
        coords
    }

    pub fn enqueue_edit(&mut self, x: i32, y: i32, z: i32, b: BlockId) {
        self.block_edits.push(DeferredEdit {
            world_x: x, world_y: y, world_z: z, new_block: b,
        });
    }

    pub fn enqueue_state(&mut self, x: i32, y: i32, z: i32,
        s: crate::world::chunk::BlockInstanceState) {
        self.state_changes.push(DeferredStateChange {
            world_x: x, world_y: y, world_z: z, new_state: s,
        });
    }

    /// Queue the start of a visual transition for the block at world
    /// coords (x, y, z). The mesher will interpolate visual properties
    /// between `from_stage` and `to_stage` once the chunk re-meshes.
    /// The handler is also responsible for enqueueing a final
    /// `DeferredStateChange` (committing the to_stage to instance_state)
    /// + a `Finish` op to clear the transition entry — typically when
    /// `current_tick - started_tick >= TRANSITION_ANIM_TICKS`.
    pub fn enqueue_transition_start(&mut self, x: i32, y: i32, z: i32,
        transition: crate::world::chunk::BlockTransition) {
        self.transition_ops.push(DeferredTransitionOp::Start {
            world_x: x, world_y: y, world_z: z, transition,
        });
    }

    /// Queue removal of the transition entry at the given world coords.
    /// Typically paired with a `state_changes` write that commits the
    /// to_stage to instance_state.
    pub fn enqueue_transition_finish(&mut self, x: i32, y: i32, z: i32) {
        self.transition_ops.push(DeferredTransitionOp::Finish {
            world_x: x, world_y: y, world_z: z,
        });
    }

    /// Queue placement of a tree-root marker at world coords. The block
    /// at that position MUST be wood (OakLog/BirchLog) when this op
    /// is applied — caller is responsible for verifying.
    pub fn enqueue_tree_root_set(&mut self, x: i32, y: i32, z: i32) {
        self.tree_root_ops.push(DeferredTreeRootOp::SetMarker {
            world_x: x, world_y: y, world_z: z,
        });
    }

    /// Queue removal of a tree-root marker. No-op if no marker exists
    /// at that position when applied.
    pub fn enqueue_tree_root_clear(&mut self, x: i32, y: i32, z: i32) {
        self.tree_root_ops.push(DeferredTreeRootOp::ClearMarker {
            world_x: x, world_y: y, world_z: z,
        });
    }

    /// Queue toggling of a tree-root marker's "unanchored" state.
    pub fn enqueue_tree_root_unanchored(&mut self, x: i32, y: i32, z: i32, on: bool) {
        self.tree_root_ops.push(DeferredTreeRootOp::SetUnanchored {
            world_x: x, world_y: y, world_z: z, unanchored: on,
        });
    }

    /// Queue installation of a freshly-computed POI cache on a chunk.
    /// Replaces any existing cache. The lifecycle handler's POI pass
    /// builds the cache from a snapshot of cluster information; this
    /// op installs it for subsequent passes (this same tick's
    /// stickiness pass, and future ticks until the next refresh).
    pub fn enqueue_install_poi_cache(
        &mut self,
        chunk: ChunkCoord,
        cache: crate::world::chunk::ChunkPoiCache,
    ) {
        self.tree_root_ops.push(DeferredTreeRootOp::InstallPoiCache { chunk, cache });
    }
}

/// A registered tick handler. Boxed so we can hold a heterogeneous mix in
/// a Vec. `Send + Sync` so the registry itself is thread-safe (handlers
/// will run on background threads in the future).
pub type BoxedHandler = Arc<dyn Fn(&mut TickContext) + Send + Sync>;

/// r183: cadence combinator — runs `inner` every `stride`-th invocation
/// with `dt` scaled by the stride, so growth/decay RATES are unchanged
/// while per-tick cost drops to 1/stride (operator perf: plants 14.8ms +
/// leaves 9.0ms per tick were the stutter teeth).
pub fn strided(stride: u64, inner: BoxedHandler) -> BoxedHandler {
    let shared = Arc::new(std::sync::atomic::AtomicU64::new(stride.max(1)));
    strided_dyn(shared, inner)
}

/// r184: live-tunable stride — the settings UI / autotuner writes the
/// shared atomic; the handler reads it per invocation. Same dt-scaling
/// contract as `strided` (rates unchanged).
pub fn strided_dyn(
    stride: Arc<std::sync::atomic::AtomicU64>,
    inner: BoxedHandler,
) -> BoxedHandler {
    let count = std::sync::atomic::AtomicU64::new(0);
    Arc::new(move |ctx: &mut TickContext| {
        let n = stride.load(std::sync::atomic::Ordering::Relaxed).max(1);
        let c = count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if c % n != 0 {
            return;
        }
        let saved_dt = ctx.tick_dt;
        ctx.tick_dt = saved_dt * n as f32;
        inner(ctx);
        ctx.tick_dt = saved_dt;
    })
}

pub struct TickHandler {
    pub name: &'static str,
    pub handler: BoxedHandler,
}

/// Registry of all tick handlers. Built once at startup; handlers run in
/// registration order each tick.
pub struct TickRegistry {
    handlers: Vec<TickHandler>,
}

impl TickRegistry {
    pub fn new() -> Self { Self { handlers: Vec::new() } }

    pub fn register(&mut self, name: &'static str, handler: BoxedHandler) {
        self.handlers.push(TickHandler { name, handler });
    }

    pub fn len(&self) -> usize { self.handlers.len() }
    pub fn is_empty(&self) -> bool { self.handlers.is_empty() }

    /// Names of registered handlers in registration order. Used for
    /// per-handler diagnostic accounting.
    pub fn handler_names(&self) -> Vec<&'static str> {
        self.handlers.iter().map(|h| h.name).collect()
    }

    /// Run one tick: invoke each handler in turn with the same context,
    /// returning the accumulated edits + state changes for the caller
    /// (ChunkManager) to apply.
    ///
    /// `field_reader` is an optional closure that handlers can call to
    /// sample world fields (moisture, wind, etc.) at world positions.
    /// `player_chunk` is the player's current chunk for distance-based
    /// tick culling — handlers can use `ctx.chunk_in_tick_range(...)` to
    /// skip distant chunks.
    pub fn run_tick(
        &self,
        chunks: &DashMap<ChunkCoord, Arc<Chunk>>,
        field_reader: Option<&dyn Fn(&str, i32, i32, i32) -> Option<f32>>,
        chunk_field_reader: Option<&dyn Fn(&str, ChunkCoord)
            -> Option<crate::world::fields::ChunkFieldSnapshot>>,
        player_chunk: Option<ChunkCoord>,
        tick_id: TickId,
        tick_period: f32,
        tick_dt: f32,
        schedule: Option<crate::world::scheduler::TickScheduleParams>,
        fire_tracker: Option<&crate::world::scheduler::LastFireTracker>,
    ) -> TickEdits {
        // Derive the legacy f64 world_time from tick_id so handlers
        // using it stay consistent. NEW handlers should prefer tick_id.
        let world_time = tick_id.to_sim_seconds(tick_period);
        let mut ctx = TickContext {
            chunks, field_reader, chunk_field_reader, player_chunk,
            tick_id, tick_period, tick_dt, world_time,
            schedule, fire_tracker,
            block_edits: Vec::new(),
            state_changes: Vec::new(),
            transition_ops: Vec::new(),
            tree_root_ops: Vec::new(),
        };
        // Per-handler timing. Records wall-clock microseconds spent in
        // each handler's closure for this tick. Aggregated across all
        // ticks of a tick batch by the caller (ChunkManager). Lets the
        // bin's diagnostic logging attribute tick_sim time per handler.
        let mut handler_timings: Vec<(&'static str, u64)> =
            Vec::with_capacity(self.handlers.len());
        for h in &self.handlers {
            let t0 = std::time::Instant::now();
            (h.handler)(&mut ctx);
            let elapsed = t0.elapsed().as_micros() as u64;
            handler_timings.push((h.name, elapsed));
        }
        TickEdits {
            block_edits: ctx.block_edits,
            state_changes: ctx.state_changes,
            transition_ops: ctx.transition_ops,
            tree_root_ops: ctx.tree_root_ops,
            handler_timings,
        }
    }
}

/// Aggregated output of one tick — block-id edits, instance-state
/// changes, and transition ops. All applied by the tick driver in
/// ChunkManager after all handlers have run.
pub struct TickEdits {
    pub block_edits: Vec<DeferredEdit>,
    pub state_changes: Vec<DeferredStateChange>,
    pub transition_ops: Vec<DeferredTransitionOp>,
    pub tree_root_ops: Vec<DeferredTreeRootOp>,
    /// Per-handler microseconds spent during this tick. The vec is
    /// in registration order; one entry per registered handler.
    /// Used by the chunk manager to attribute tick_sim time and
    /// expose per-handler stats to the bin's diagnostic log.
    pub handler_timings: Vec<(&'static str, u64)>,
}

impl Default for TickRegistry {
    fn default() -> Self { Self::new() }
}

/// Tick scheduler — accumulates real time and decides when to fire ticks.
/// Owned by ChunkManager; advanced once per frame.
///
/// Holds the canonical `TickId` counter that increments each time a tick
/// fires. Consumers (handlers, physics, wind) read this to know "what
/// simulation time is it." The legacy `world_time_secs` accumulator
/// stays for backward compat but is now derived from tick_id × period
/// when it matters for determinism.
pub struct TickScheduler {
    accumulator: f32,
    period: f32,
    /// Master clock for the simulation. Increments by 1 each time a
    /// tick fires. NEVER mutated by anything else — this is the
    /// authoritative tick number.
    current_tick: TickId,
    /// Real seconds since world start. NOT used for any deterministic
    /// decision — keep around only for UI / debug display. Anywhere
    /// else, derive seconds from `current_tick.to_sim_seconds(period)`.
    pub world_time_secs: f64,
}

impl TickScheduler {
    pub fn new() -> Self {
        Self {
            accumulator: 0.0,
            period: DEFAULT_TICK_PERIOD,
            current_tick: TickId::ZERO,
            world_time_secs: 0.0,
        }
    }

    pub fn set_period(&mut self, period: f32) { self.period = period.max(1e-3); }

    /// Add elapsed real time. Returns the number of ticks that should fire
    /// this frame (0 most frames, 1 every period, possibly more if we're
    /// catching up after a slow frame).
    ///
    /// Caller is expected to call `advance_tick()` exactly that many
    /// times to bump the canonical TickId. (Two-phase API so the caller
    /// can observe the in-progress tick number for logging / netcode
    /// without a borrow conflict.)
    pub fn advance(&mut self, real_dt: f32) -> u32 {
        // Cap real_dt to avoid runaway tick spirals after a long pause.
        let dt = real_dt.min(self.period * 4.0);
        self.accumulator += dt;
        self.world_time_secs += dt as f64;
        let mut count = 0u32;
        while self.accumulator >= self.period {
            self.accumulator -= self.period;
            count += 1;
        }
        count
    }

    /// Return the next TickId that will fire (= current+1) and advance
    /// the canonical counter. Call once per tick that the caller is
    /// about to execute.
    pub fn advance_tick(&mut self) -> TickId {
        self.current_tick = self.current_tick.next();
        self.current_tick
    }

    /// Current tick number. Equal to the most recent TickId returned by
    /// `advance_tick`, or ZERO if no tick has fired yet.
    pub fn current_tick(&self) -> TickId { self.current_tick }

    pub fn period(&self) -> f32 { self.period }
}

impl Default for TickScheduler {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_fires_ticks_at_period() {
        let mut s = TickScheduler::new();
        s.set_period(1.0);
        assert_eq!(s.advance(0.5), 0);
        assert_eq!(s.advance(0.6), 1); // total 1.1, 1 tick fired, 0.1 left
        assert_eq!(s.advance(2.5), 2); // total 2.6, 2 more ticks
    }

    #[test]
    fn scheduler_accumulates_world_time() {
        let mut s = TickScheduler::new();
        s.advance(1.0);
        s.advance(0.5);
        assert!((s.world_time_secs - 1.5).abs() < 1e-3);
    }

    #[test]
    fn registry_runs_handlers_in_order() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let counter = Arc::new(AtomicUsize::new(0));
        let mut r = TickRegistry::new();

        let c1 = counter.clone();
        r.register("a", Arc::new(move |_ctx| {
            c1.fetch_add(1, Ordering::SeqCst);
        }));
        let c2 = counter.clone();
        r.register("b", Arc::new(move |_ctx| {
            c2.fetch_add(10, Ordering::SeqCst);
        }));

        let chunks: DashMap<ChunkCoord, Arc<Chunk>> = DashMap::new();
        let edits = r.run_tick(&chunks, None, None, None, TickId::ZERO, 0.5, 0.5, None, None);
        assert_eq!(counter.load(Ordering::SeqCst), 11);
        assert_eq!(edits.block_edits.len(), 0);
        assert_eq!(edits.state_changes.len(), 0);
    }

    #[test]
    fn deferred_edits_accumulate() {
        let mut r = TickRegistry::new();
        r.register("edit", Arc::new(|ctx| {
            ctx.enqueue_edit(0, 0, 0, BlockId::Stone);
            ctx.enqueue_edit(1, 0, 0, BlockId::Air);
        }));
        let chunks: DashMap<ChunkCoord, Arc<Chunk>> = DashMap::new();
        let edits = r.run_tick(&chunks, None, None, None, TickId::ZERO, 0.5, 0.5, None, None);
        assert_eq!(edits.block_edits.len(), 2);
        assert_eq!(edits.block_edits[0].new_block, BlockId::Stone);
        assert_eq!(edits.block_edits[1].new_block, BlockId::Air);
    }

    #[test]
    fn deferred_state_changes_accumulate() {
        use crate::world::chunk::BlockInstanceState;
        let mut r = TickRegistry::new();
        r.register("statechange", Arc::new(|ctx| {
            ctx.enqueue_state(5, 6, 7,
                BlockInstanceState { age_ticks: 42, flags: 0b101 });
        }));
        let chunks: DashMap<ChunkCoord, Arc<Chunk>> = DashMap::new();
        let edits = r.run_tick(&chunks, None, None, None, TickId::ZERO, 0.5, 0.5, None, None);
        assert_eq!(edits.state_changes.len(), 1);
        assert_eq!(edits.state_changes[0].new_state.age_ticks, 42);
    }
}
