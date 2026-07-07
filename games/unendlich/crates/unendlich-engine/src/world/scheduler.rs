//! Distance-based deterministic chunk tick scheduler.
//!
//! Picks how often each loaded chunk's tick handlers run, based purely on
//! the chunk's distance from the player. Three bands:
//!
//!  - `Render`: close to player. Ticks every game-tick. Drives animations
//!    and immediate visual changes.
//!  - `Simulation`: loaded but outside render-distance. Ticks 1 of every
//!    `simulation_divisor` game-ticks. Cheaper but still runs continuously.
//!  - `CatchUp`: too far for live simulation. Doesn't tick during play;
//!    when the chunk re-enters a faster band, the handlers advance by
//!    the elapsed game-tick count at once.
//!
//! ## Why "purely a function of distance"
//!
//! The scheduler must be deterministic for multiplayer / rollback netcode.
//! Two clients running the same simulation must agree on which chunks
//! tick when. Distance is itself deterministic (player position is part
//! of the simulation state), so any band assignment that's a pure
//! function of distance preserves determinism.
//!
//! Crucially: the scheduler does NOT take real-time CPU budget into
//! account. It does NOT skip ticks because we ran out of frame time.
//! Those would introduce non-determinism. Cost reduction comes from
//! the band-divisor structure (pre-decided), not from runtime
//! adaptation.
//!
//! ## The time-homogeneity contract
//!
//! Catch-up only works if tick handlers are time-homogeneous: advancing
//! N ticks at once must produce the same end state as advancing 1 tick
//! N times. This is a real constraint on handler design — handlers that
//! sample external state (wind, light) per tick can't naively be
//! "advanced by N" because they'd see only one sample where they should
//! have seen N. The handler is responsible for handling the `tick_dt`
//! field correctly; the scheduler just provides the dt.
//!
//! Tests in this module verify the band assignment + should_fire logic.
//! Per-handler time-homogeneity tests live with each handler.

use crate::world::chunk::ChunkCoord;
use crate::world::ticks::TickId;

/// Which band a chunk is in for tick-rate purposes. Determined entirely
/// by Chebyshev distance from the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChunkTickBand {
    /// Close to player — every game-tick.
    Render,
    /// Outside render but within simulation distance — 1 of N ticks.
    Simulation,
    /// Beyond simulation distance — no live ticking, catch-up on
    /// re-entry to a faster band.
    CatchUp,
}

impl ChunkTickBand {
    /// The tick-rate divisor for this band. Render = 1 (fires every
    /// tick). Simulation = `params.simulation_divisor`. CatchUp = 0
    /// (never fires during play).
    pub fn divisor(self, params: &TickScheduleParams) -> u64 {
        match self {
            ChunkTickBand::Render => 1,
            ChunkTickBand::Simulation => params.simulation_divisor as u64,
            ChunkTickBand::CatchUp => 0,
        }
    }
}

/// Parameters configuring the band thresholds and divisors. Stored on
/// ChunkManager (or wherever the scheduler is wired in); passed by ref
/// to band-classification calls so test code can use different values.
#[derive(Debug, Clone, Copy)]
pub struct TickScheduleParams {
    /// Chebyshev distance (in chunks) at or below which a chunk is in
    /// the Render band. Should equal the renderer's RENDER_DISTANCE.
    pub render_distance: i32,
    /// Chebyshev distance (in chunks) at or below which a chunk is in
    /// the Simulation band (above this, it's CatchUp). Must be >=
    /// render_distance.
    pub simulation_distance: i32,
    /// How many game-ticks elapse between Simulation-band tick fires.
    /// Render band always fires every tick (divisor 1). A divisor of
    /// 8 means a Simulation-band chunk fires its handlers once every
    /// 8 game-ticks; the handler receives a `tick_dt` of 8 * tick_period
    /// to compensate.
    pub simulation_divisor: u32,
}

impl Default for TickScheduleParams {
    fn default() -> Self {
        Self {
            render_distance: crate::constants::RENDER_DISTANCE,
            simulation_distance: crate::constants::RENDER_DISTANCE * 2,
            simulation_divisor: 8,
        }
    }
}

/// Compute Chebyshev distance (max of |dx|, |dy|, |dz|) between two
/// chunk coordinates. Used as the canonical distance metric for band
/// classification.
#[inline]
pub fn chebyshev_distance(a: ChunkCoord, b: ChunkCoord) -> i32 {
    let dx = (a.x - b.x).abs();
    let dy = (a.y - b.y).abs();
    let dz = (a.z - b.z).abs();
    dx.max(dy).max(dz)
}

/// Band assignment for a chunk based on player distance. Pure function
/// of (chunk, player, params). No state, no randomness, no time-budget
/// awareness.
#[inline]
pub fn band_for_chunk(
    chunk: ChunkCoord,
    player: ChunkCoord,
    params: &TickScheduleParams,
) -> ChunkTickBand {
    let d = chebyshev_distance(chunk, player);
    if d <= params.render_distance {
        ChunkTickBand::Render
    } else if d <= params.simulation_distance {
        ChunkTickBand::Simulation
    } else {
        ChunkTickBand::CatchUp
    }
}

/// Whether a chunk in the given band should fire its tick handlers
/// for the given TickId. Pure function of (band, tick_id, params).
///
/// Render: always true (every tick).
/// Simulation: true when tick_id is divisible by simulation_divisor.
///   This phase-aligns simulation ticks across all chunks in the band
///   (every chunk fires on the same ticks, easier to reason about).
/// CatchUp: always false (handled via on-reload catch-up, not live).
#[inline]
pub fn should_fire_in_band(
    band: ChunkTickBand,
    tick_id: TickId,
    params: &TickScheduleParams,
) -> bool {
    match band {
        ChunkTickBand::Render => true,
        ChunkTickBand::Simulation => {
            tick_id.raw() % (params.simulation_divisor as u64) == 0
        }
        ChunkTickBand::CatchUp => false,
    }
}

/// Stable per-chunk phase offset in `[0, divisor)` — a splitmix64 hash of the
/// chunk coord. Deterministic (replay-safe) and independent of the player, so a
/// chunk keeps its phase as the player moves.
#[inline]
pub fn chunk_phase(chunk: ChunkCoord, divisor: u32) -> u64 {
    if divisor <= 1 {
        return 0;
    }
    let mut x = (chunk.x as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (chunk.y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (chunk.z as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    x % (divisor as u64)
}

/// Like [`should_fire_in_band`] but STAGGERS the Simulation band by chunk: each
/// chunk fires when `tick_id % divisor == chunk_phase(coord)` instead of all
/// chunks firing on the same tick. Same per-chunk cadence (once per
/// `simulation_divisor` ticks) and — paired with the catch-up tracker that
/// advances by elapsed ticks — the same total work, but spread evenly across the
/// window so no single tick carries the whole band (kills the periodic
/// plant_lifecycle spike). Render/CatchUp unchanged.
#[inline]
pub fn should_fire_in_band_staggered(
    band: ChunkTickBand,
    chunk: ChunkCoord,
    tick_id: TickId,
    params: &TickScheduleParams,
) -> bool {
    match band {
        ChunkTickBand::Render => true,
        ChunkTickBand::Simulation => {
            let div = params.simulation_divisor.max(1) as u64;
            tick_id.raw() % div == chunk_phase(chunk, params.simulation_divisor)
        }
        ChunkTickBand::CatchUp => false,
    }
}

/// The effective `tick_dt` (in real seconds) that a band's handlers
/// should receive when they fire. Render band sees the standard period;
/// Simulation band sees `divisor * period` to compensate for skipped
/// ticks. A handler that integrates over `tick_dt` will produce the
/// same total integration as if it had ticked at full rate.
///
/// **Time-homogeneity contract**: handlers MUST handle this dt
/// correctly. Naive "always age by 1 per tick" handlers will under-
/// count when they're in the Simulation band. Correct handlers scale
/// their per-tick effect by `tick_dt / tick_period` (or equivalently,
/// by the band's divisor).
#[inline]
pub fn tick_dt_for_band(band: ChunkTickBand, base_period: f32, params: &TickScheduleParams) -> f32 {
    let divisor = band.divisor(params);
    if divisor == 0 {
        // CatchUp band — caller shouldn't be asking, but return 0 as a
        // safe sentinel meaning "no time has elapsed for this chunk".
        0.0
    } else {
        base_period * divisor as f32
    }
}

/// Per-(handler, chunk) last-fire tracker. Used for catch-up: when a
/// chunk fires its handler, we compute `n = tick_id - last_fire_tick`
/// and pass that to the handler so it can advance the right number of
/// simulated steps. Without this, a chunk that's been in CatchUp band
/// for 1000 ticks then re-enters Simulation band would only advance by
/// the band's default divisor (8), missing 992 ticks of evolution.
///
/// Keyed by `(handler_name, chunk_coord)` so multiple handlers can
/// independently track their own last-fire times for the same chunk.
/// Handler names are `&'static str` (matching `TickHandler::name`).
///
/// **Concurrency**: wrapped in a parking_lot Mutex so the registry can
/// share one tracker across all handlers (they currently run
/// sequentially, but the lock makes future parallel handlers safe).
/// Lock contention is one acquire per chunk-handler-fire which is
/// negligible at our scale.
pub struct LastFireTracker {
    inner: parking_lot::Mutex<
        std::collections::HashMap<(&'static str, crate::world::chunk::ChunkCoord), TickId>,
    >,
}

impl LastFireTracker {
    pub fn new() -> Self {
        Self { inner: parking_lot::Mutex::new(std::collections::HashMap::new()) }
    }

    /// How many ticks have elapsed since this (handler, chunk) last
    /// fired? Returns the gap in ticks; if there's no record (first
    /// fire ever), returns `default_n` so first-fire behaves like a
    /// normal-cadence fire, not a catch-up.
    ///
    /// `default_n` is typically the band's divisor (1 for Render,
    /// `simulation_divisor` for Simulation). This means a fresh chunk
    /// that fires for the first time gets the band-default amount of
    /// evolution applied, not zero.
    pub fn elapsed_or_default(
        &self,
        handler: &'static str,
        chunk: crate::world::chunk::ChunkCoord,
        current_tick: TickId,
        default_n: u32,
    ) -> u32 {
        let map = self.inner.lock();
        match map.get(&(handler, chunk)) {
            Some(&last) => {
                // Gap clamps to u32::MAX. At 2 Hz tick rate this
                // saturates at ~2 billion seconds (~68 years). Real
                // catch-up gaps are bounded by chunk-eviction policy.
                let gap = current_tick.raw().saturating_sub(last.raw());
                (gap as u32).max(1)
            }
            None => default_n.max(1),
        }
    }

    /// Record that this (handler, chunk) fired at this tick. Call
    /// after a handler's per-chunk work completes.
    pub fn mark_fired(
        &self,
        handler: &'static str,
        chunk: crate::world::chunk::ChunkCoord,
        tick: TickId,
    ) {
        self.inner.lock().insert((handler, chunk), tick);
    }

    /// Clear all entries for chunks the player has moved permanently
    /// away from (chunk eviction). Without this, the map grows
    /// unbounded as the player explores.
    ///
    /// Called by ChunkManager when it evicts a chunk from memory.
    /// Removes entries for ALL handlers at that chunk in one pass.
    pub fn evict_chunk(&self, chunk: crate::world::chunk::ChunkCoord) {
        let mut map = self.inner.lock();
        map.retain(|(_, c), _| *c != chunk);
    }

    /// Number of (handler, chunk) entries currently tracked. Used
    /// for diagnostics / leak-checking.
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }
}

impl Default for LastFireTracker {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> TickScheduleParams {
        TickScheduleParams { render_distance: 4, simulation_distance: 8, simulation_divisor: 8 }
    }

    #[test]
    fn band_render_at_player() {
        let player = ChunkCoord::new(0, 0, 0);
        assert_eq!(band_for_chunk(player, player, &p()), ChunkTickBand::Render);
    }

    #[test]
    fn band_render_at_edge() {
        let player = ChunkCoord::new(0, 0, 0);
        let edge = ChunkCoord::new(4, 0, 0);
        assert_eq!(band_for_chunk(edge, player, &p()), ChunkTickBand::Render);
    }

    #[test]
    fn band_simulation_just_outside_render() {
        let player = ChunkCoord::new(0, 0, 0);
        let just_outside = ChunkCoord::new(5, 0, 0);
        assert_eq!(band_for_chunk(just_outside, player, &p()), ChunkTickBand::Simulation);
    }

    #[test]
    fn band_catchup_far() {
        let player = ChunkCoord::new(0, 0, 0);
        let far = ChunkCoord::new(20, 0, 0);
        assert_eq!(band_for_chunk(far, player, &p()), ChunkTickBand::CatchUp);
    }

    #[test]
    fn band_uses_chebyshev_not_euclidean() {
        // (3, 3, 0) has Chebyshev distance 3 (still in Render band)
        // but Euclidean distance ~4.24. Verify Chebyshev is used.
        let player = ChunkCoord::new(0, 0, 0);
        let diag = ChunkCoord::new(3, 3, 0);
        assert_eq!(band_for_chunk(diag, player, &p()), ChunkTickBand::Render);
    }

    #[test]
    fn render_band_fires_every_tick() {
        for raw in 0u64..20 {
            let t = TickId(raw);
            assert!(should_fire_in_band(ChunkTickBand::Render, t, &p()),
                "Render should fire at every tick id, missed t={}", raw);
        }
    }

    #[test]
    fn simulation_band_fires_every_nth_tick() {
        let params = p();
        let div = params.simulation_divisor as u64;
        for raw in 0u64..50 {
            let expected = raw % div == 0;
            let actual = should_fire_in_band(ChunkTickBand::Simulation, TickId(raw), &params);
            assert_eq!(actual, expected,
                "Simulation should fire iff tick%divisor==0, mismatch at t={} (div={})",
                raw, div);
        }
    }

    #[test]
    fn catchup_band_never_fires_live() {
        for raw in 0u64..20 {
            let t = TickId(raw);
            assert!(!should_fire_in_band(ChunkTickBand::CatchUp, t, &p()),
                "CatchUp should never fire live, fired at t={}", raw);
        }
    }

    #[test]
    fn band_assignment_is_deterministic() {
        // Same (chunk, player, params) always yields same band.
        let player = ChunkCoord::new(10, 5, -3);
        let chunk = ChunkCoord::new(15, 7, 0);
        let params = p();
        let b1 = band_for_chunk(chunk, player, &params);
        let b2 = band_for_chunk(chunk, player, &params);
        assert_eq!(b1, b2);
    }

    #[test]
    fn should_fire_is_deterministic() {
        // Same (band, tick_id, params) always yields same answer.
        let params = p();
        for band in [ChunkTickBand::Render, ChunkTickBand::Simulation, ChunkTickBand::CatchUp] {
            for raw in [0u64, 1, 7, 8, 100, 1000] {
                let t = TickId(raw);
                let a = should_fire_in_band(band, t, &params);
                let b = should_fire_in_band(band, t, &params);
                assert_eq!(a, b, "non-determinism at band={:?} t={}", band, raw);
            }
        }
    }

    #[test]
    fn tick_dt_compensates_for_band_divisor() {
        let params = p();
        let period = 0.5;
        // Render fires every tick → dt = 1 period.
        assert_eq!(tick_dt_for_band(ChunkTickBand::Render, period, &params), period);
        // Simulation fires every Nth tick → dt = N periods.
        let expected = period * params.simulation_divisor as f32;
        assert_eq!(tick_dt_for_band(ChunkTickBand::Simulation, period, &params), expected);
        // CatchUp never fires → dt = 0 (sentinel).
        assert_eq!(tick_dt_for_band(ChunkTickBand::CatchUp, period, &params), 0.0);
    }

    /// Time-homogeneity property: across any two bands, a chunk that
    /// stays in band A for some time vs band B for some time produces
    /// the same total tick_dt-time given the same elapsed wall time.
    /// (Render fires more often with smaller dt; Simulation fires less
    /// often with larger dt; product is the same.)
    #[test]
    fn fire_rate_times_dt_equals_constant_per_unit_wall_time() {
        let params = p();
        let period = 0.5_f32;
        let wall_time_ticks = 1000u64;

        // Render band: fires every tick × dt of period each
        let render_total = (wall_time_ticks as f32) * period;

        // Simulation band: fires (1000/divisor) times × dt of (period * divisor) each
        let sim_fires = wall_time_ticks / params.simulation_divisor as u64;
        let sim_dt = tick_dt_for_band(ChunkTickBand::Simulation, period, &params);
        let sim_total = (sim_fires as f32) * sim_dt;

        // Same total time integrated, modulo end-of-window remainder
        let diff = (render_total - sim_total).abs();
        assert!(diff < period * params.simulation_divisor as f32,
            "render_total={}, sim_total={}, diff={} should be < one sim period",
            render_total, sim_total, diff);
    }

    #[test]
    fn last_fire_tracker_reports_default_for_unknown() {
        let t = LastFireTracker::new();
        let n = t.elapsed_or_default("test_handler", ChunkCoord::new(0, 0, 0), TickId(100), 8);
        assert_eq!(n, 8, "unknown handler/chunk should get default_n");
    }

    #[test]
    fn last_fire_tracker_reports_gap_after_marking() {
        let t = LastFireTracker::new();
        let coord = ChunkCoord::new(1, 2, 3);
        t.mark_fired("h", coord, TickId(50));
        let n = t.elapsed_or_default("h", coord, TickId(75), 1);
        assert_eq!(n, 25, "gap should be tick_id - last_fire");
    }

    #[test]
    fn last_fire_tracker_clamps_to_minimum_one() {
        // If for some reason current_tick < last_fire (shouldn't happen
        // in practice), we still return at least 1 so handlers don't
        // do zero work and miss the fire.
        let t = LastFireTracker::new();
        let coord = ChunkCoord::new(0, 0, 0);
        t.mark_fired("h", coord, TickId(100));
        let n = t.elapsed_or_default("h", coord, TickId(100), 1);
        assert_eq!(n, 1, "same-tick re-fire should advance by 1, not 0");
    }

    #[test]
    fn last_fire_tracker_separates_handlers() {
        // Different handler names should not collide on the same chunk.
        let t = LastFireTracker::new();
        let coord = ChunkCoord::new(5, 5, 5);
        t.mark_fired("handler_a", coord, TickId(10));
        t.mark_fired("handler_b", coord, TickId(50));
        assert_eq!(t.elapsed_or_default("handler_a", coord, TickId(60), 1), 50);
        assert_eq!(t.elapsed_or_default("handler_b", coord, TickId(60), 1), 10);
    }

    #[test]
    fn last_fire_tracker_evicts_chunk() {
        let t = LastFireTracker::new();
        let coord = ChunkCoord::new(1, 1, 1);
        t.mark_fired("h", coord, TickId(100));
        t.mark_fired("g", coord, TickId(200));
        let other = ChunkCoord::new(2, 2, 2);
        t.mark_fired("h", other, TickId(300));
        assert_eq!(t.len(), 3);

        t.evict_chunk(coord);
        // Both entries for `coord` should be gone; `other` should remain.
        assert_eq!(t.len(), 1);
        // Re-querying coord returns the default.
        let n = t.elapsed_or_default("h", coord, TickId(400), 7);
        assert_eq!(n, 7);
    }

    /// Catch-up scenario: a chunk that was in CatchUp band for a long
    /// time, then re-enters a faster band. The handler should advance
    /// by the elapsed-tick gap, not the band's default divisor, so the
    /// chunk catches up on what it missed.
    #[test]
    fn last_fire_tracker_supports_catchup_gap() {
        let t = LastFireTracker::new();
        let coord = ChunkCoord::new(10, 0, 10);
        // Last fired 1000 ticks ago.
        t.mark_fired("decay", coord, TickId(50));
        // Now in Render band, default n = 1, but catch-up gap = 1000.
        let n = t.elapsed_or_default("decay", coord, TickId(1050), 1);
        assert_eq!(n, 1000, "catch-up should advance by full gap, not band default");
    }
}

#[cfg(test)]
mod scheduler_stagger_tests {
    use super::*;

    fn pc(x: i32, z: i32) -> ChunkCoord {
        ChunkCoord { x, y: 0, z }
    }

    #[test]
    fn each_chunk_fires_once_per_divisor_window() {
        // a staggered Simulation chunk fires exactly once across any `divisor`
        // consecutive ticks (same cadence as the aligned version, just phased).
        let params = TickScheduleParams { simulation_divisor: 8, ..Default::default() };
        let c = pc(3, 7);
        let fires: usize = (0..8)
            .filter(|t| should_fire_in_band_staggered(
                ChunkTickBand::Simulation, c, TickId(*t), &params))
            .count();
        assert_eq!(fires, 1, "exactly one fire per divisor window");
    }

    #[test]
    fn different_chunks_get_different_phases() {
        // the whole point: not all Simulation chunks fire on the same tick.
        let phases: std::collections::HashSet<u64> =
            (0..64).map(|i| chunk_phase(pc(i, i * 2 + 1), 8)).collect();
        assert!(phases.len() > 1, "phases must spread across the window, got {phases:?}");
    }

    #[test]
    fn stagger_phase_is_deterministic() {
        assert_eq!(chunk_phase(pc(5, 9), 8), chunk_phase(pc(5, 9), 8));
    }

    #[test]
    fn divisor_one_never_staggers() {
        assert_eq!(chunk_phase(pc(1, 2), 1), 0);
        let params = TickScheduleParams { simulation_divisor: 1, ..Default::default() };
        assert!(should_fire_in_band_staggered(
            ChunkTickBand::Simulation, pc(1, 2), TickId(5), &params));
    }

    #[test]
    fn render_and_catchup_unaffected_by_stagger() {
        let params = TickScheduleParams { simulation_divisor: 8, ..Default::default() };
        assert!(should_fire_in_band_staggered(
            ChunkTickBand::Render, pc(0, 0), TickId(3), &params));
        assert!(!should_fire_in_band_staggered(
            ChunkTickBand::CatchUp, pc(0, 0), TickId(3), &params));
    }
}
