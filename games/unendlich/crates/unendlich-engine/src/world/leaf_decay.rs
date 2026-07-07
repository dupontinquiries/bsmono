//! Leaf decay: leaves accumulate damage from wind exposure and eventually
//! fall (block becomes Air).
//!
//! Same shape as plant lifecycle — registered tick handler, per-block
//! instance state, mesher reads state for color modulation. Differs in
//! that:
//!
//!   - **Driver is wind**, not moisture. Stronger wind = faster decay.
//!     (Real rationale: leaves are pulled off branches by wind. Slow
//!     decay even at zero wind models age + cumulative weather.)
//!
//!   - **Single-stage progression**, not a multi-stage state machine.
//!     Leaves age linearly from healthy → tattered → fallen. Color
//!     shifts from leaf-green → autumn-yellow → autumn-brown → gone.
//!
//!   - **Tied to specific block types** (OakLeaves, BirchLeaves), not
//!     all has_subvoxels blocks.
//!
//! Group ticking: we can't reuse `plant_indices` directly because that
//! includes grass/ferns/flowers that don't decay this way. A future
//! `leaf_indices` would be ideal; for now the handler iterates
//! `plant_indices` and filters for leaves at handler time. Cheap because
//! plant_indices is already small and the filter is one match expression.

use crate::world::blocks::BlockId;
use crate::world::chunk::BlockInstanceState;

/// Tick threshold beyond which a leaf falls (block → Air). Tuned for
/// VISIBLE adaptation but not aggressive turnover: 200 ticks at 0.5s
/// per tick = ~100s real time at calm, or ~50s at strong wind. The
/// previous 60-tick value was too aggressive — leaves cycled through
/// the world every 30s, which the user (correctly) flagged as
/// "destroying leaves" when they broke nearby blocks (the breaks
/// just happened to coincide with already-late-stage leaves dying).
///
/// Capped at u8 so 255 is the absolute ceiling. 200 leaves headroom
/// for wind-acceleration to push specific leaves over the edge in
/// gusty zones without dropping all leaves at once.
pub const LEAF_DEATH_AGE_TICKS: u8 = 200;

/// Wind threshold above which leaf decay accumulates faster. At zero or
/// very low wind, leaves age slowly (1 tick per real tick). Above this
/// threshold, they age extra ticks per real tick (gusts pull them off).
pub const WIND_FAST_THRESHOLD: f32 = 0.5;

/// Color modulation curve for leaves over their decay lifetime.
/// `progress` is age_ticks / LEAF_DEATH_AGE_TICKS, clamped [0, 1].
/// Returns a multiplier applied per cell of the leaf sub-voxel grid.
///
/// Curve: starts at [1.0, 1.0, 1.0] (no change), shifts toward
/// [1.4, 0.9, 0.4] (yellow-orange) at midlife, ends near [1.5, 0.5, 0.2]
/// (deep autumn brown) just before falling.
pub fn decay_color_modulation(age_ticks: u8) -> [f32; 3] {
    let p = (age_ticks as f32 / LEAF_DEATH_AGE_TICKS as f32).clamp(0.0, 1.0);
    [
        1.0 + 0.5 * p,           // R rising
        1.0 - 0.5 * p,           // G falling
        1.0 - 0.8 * p,           // B falling faster
    ]
}

/// Advance one tick of leaf decay. Returns true if the leaf should fall
/// (be removed from the world).
pub fn advance_one_tick(state: &mut BlockInstanceState, wind_magnitude: f32) -> bool {
    advance_n_ticks(state, wind_magnitude, 1)
}

/// Advance leaf decay by `n` simulated ticks at once. Used by:
///  - The Render band (n=1, identical to advance_one_tick)
///  - The Simulation band (n=simulation_divisor, compensates for skipped ticks)
///  - Catch-up on chunk reload (n=elapsed_ticks_since_last_handler_run)
///
/// **Time-homogeneity property**: `advance_n_ticks(s, w, N)` produces
/// the same final state as N consecutive `advance_n_ticks(s, w, 1)`
/// calls *with the same wind value*. This is true for leaf decay because
/// the only per-tick state change is age accumulation, which is linear.
///
/// Note that if wind itself varies between the N ticks (which it
/// doesn't yet — wind is a static stub — but will once wind is
/// directional and time-varying), the homogeneity guarantee weakens.
/// Then "N at once" uses one wind sample where "N individually" would
/// use N. This is acceptable for slow-changing fields (the average
/// is what matters for accumulated decay) but worth noting.
pub fn advance_n_ticks(state: &mut BlockInstanceState, wind_magnitude: f32, n: u32) -> bool {
    if n == 0 { return state.age_ticks >= LEAF_DEATH_AGE_TICKS; }
    // Per-tick age increment: 1 base + 1 if gusty.
    let per_tick_age = if wind_magnitude > WIND_FAST_THRESHOLD { 2 } else { 1 };
    // Saturating multiplication then saturating add — avoid overflow
    // even for very large n (catch-up after long absence).
    let total = (per_tick_age as u32).saturating_mul(n);
    let total_clamped: u8 = total.min(u8::MAX as u32) as u8;
    state.age(total_clamped);
    state.set_flag(BlockInstanceState::FLAG_DECAYING, true);
    state.age_ticks >= LEAF_DEATH_AGE_TICKS
}

// ── r330-bughunt: floating-trunk support predicate ──────────────────────────
// The r320 "floating-trunk decay" pass left logs hanging in three cases (§4e):
//   1. logs were never even visited — the handler iterates `plant_indices`,
//      but logs live in `wood_indices` (has_subvoxels() is false for logs), so
//      the whole log branch was dead code;
//   2. logs at the chunk floor (ly==0) were skipped because the below-block
//      lives in the neighbouring chunk ("can't see below");
//   3. only a bare `Air` block below counted as "unsupported".
// These predicates express the support rule purely so it can be tested in
// isolation; the handler does the (impure) neighbour lookup and feeds the
// resolved below-block in.

/// Is `below` valid structural support for a trunk log resting directly on it?
///
/// A trunk is held up by either another log (the trunk continuing downward) or
/// by real ground (`BlockId::is_tree_anchorage` — the same "feels like ground"
/// rule a tree uses to anchor its root marker). Everything else — air, fluids,
/// leaves, plants — is NOT structural support.
pub fn block_supports_trunk(below: BlockId) -> bool {
    matches!(below, BlockId::OakLog | BlockId::BirchLog) || below.is_tree_anchorage()
}

/// Should `block` be flagged for decay because it is an unsupported (floating)
/// trunk log?
///
/// `below` is the block directly beneath it, or `None` when that block lies
/// outside the loaded world (the chunk below is not resident). Non-logs are
/// never flagged by this rule. When the supporting block is unknown we
/// conservatively return `false`: we don't decay a trunk merely because its
/// support chunk hasn't streamed in yet (mirrors `is_tree_anchorage`'s own
/// "when in doubt, false" stance — a false decay deletes a legitimate tree,
/// which is worse than a one-chunk-load delay before an orphan starts falling).
pub fn log_is_unsupported(block: BlockId, below: Option<BlockId>) -> bool {
    let is_log = matches!(block, BlockId::OakLog | BlockId::BirchLog);
    if !is_log {
        return false;
    }
    match below {
        Some(b) => !block_supports_trunk(b),
        None => false,
    }
}

/// Build the leaf decay tick handler.
pub fn decay_handler() -> std::sync::Arc<
    dyn Fn(&mut crate::world::ticks::TickContext) + Send + Sync
> {
    use crate::world::chunk::Chunk;

    std::sync::Arc::new(move |ctx: &mut crate::world::ticks::TickContext| {
        use crate::world::scheduler::{band_for_chunk, should_fire_in_band, ChunkTickBand};

        // Snapshot scheduler params + player position once per tick so
        // we don't pay the borrow / Option-unwrap per chunk.
        let schedule = ctx.schedule;
        let player = ctx.player_chunk;
        let tick_id = ctx.tick_id;
        let tracker = ctx.fire_tracker;
        const HANDLER_NAME: &str = "leaf_decay";

        // Sorted iteration for rollback determinism (see plant_lifecycle).
        for coord in ctx.sorted_chunk_coords() {
            // Distance-based band classification. Three modes:
            //  - With scheduler + player: full band logic. Render fires every
            //    tick; Simulation fires once per N ticks; CatchUp doesn't fire.
            //  - Without scheduler (test contexts, etc.): fall back to the
            //    legacy "tick everything within radius" behaviour.
            //
            // The `n` (ticks-of-evolution to apply) is determined by the
            // last-fire tracker if available: it returns the gap since
            // this chunk last fired this handler, defaulting to the
            // band's natural cadence (1 for Render, divisor for Simulation)
            // for first fires. This is what makes catch-up correct: a
            // chunk that just transitioned from CatchUp to Render after
            // being away for 500 ticks gets `n=500`, not `n=1`.
            let (should_run, default_n) = match (schedule.as_ref(), player) {
                (Some(params), Some(p)) => {
                    let band = band_for_chunk(coord, p, params);
                    let fire = should_fire_in_band(band, tick_id, params);
                    let n: u32 = match band {
                        ChunkTickBand::Render => 1,
                        ChunkTickBand::Simulation => params.simulation_divisor,
                        ChunkTickBand::CatchUp => 0,
                    };
                    (fire, n)
                }
                _ => {
                    // Legacy path: cull by render-distance+1 radius, age 1 per tick.
                    const TICK_CULL_RADIUS: i32 = crate::constants::RENDER_DISTANCE + 1;
                    let in_range = ctx.chunk_in_tick_range(coord, TICK_CULL_RADIUS);
                    (in_range, 1u32)
                }
            };
            if !should_run { continue; }

            // Compute n: gap since last fire, OR default if no tracker / first fire.
            let age_per_block = match tracker {
                Some(t) => t.elapsed_or_default(HANDLER_NAME, coord, tick_id, default_n),
                None => default_n,
            };

            let Some(entry) = ctx.chunks.get(&coord) else { continue };
            let chunk = entry.value();
            let (origin_x, origin_y, origin_z) = coord.block_origin();

            // Visit leaves (plant_indices) AND logs (wood_indices). r330-bughunt:
            // the previous code iterated only plant_indices, but logs are NOT in
            // it — has_subvoxels() is false for logs, so they live in the
            // separately-maintained wood_indices. The old log branch was therefore
            // dead code and floating trunks never decayed. Chaining both indices
            // makes the log-support check actually reachable. The two sets are
            // disjoint (a block is subvoxel-foliage XOR wood), so no double-visit.
            for &idx in chunk.plant_indices.iter().chain(chunk.wood_indices.iter()) {
                let block = chunk.blocks[idx as usize];
                let is_leaf = matches!(block, BlockId::OakLeaves | BlockId::BirchLeaves);
                let is_log = matches!(block, BlockId::OakLog | BlockId::BirchLog);
                if !is_leaf && !is_log {
                    continue;
                }
                let (lx, ly, lz) = Chunk::index_to_xyz(idx);
                let wx = origin_x + lx;
                let wy = origin_y + ly;
                let wz = origin_z + lz;

                // Sample wind magnitude. The current wind field is a
                // scalar 0.3-everywhere stub; a future directional wind
                // field will replace this with a magnitude derived from
                // the field vector's length.
                let wind = ctx.field_reader
                    .and_then(|r| r("wind", wx, wy, wz))
                    .unwrap_or(0.3);

                let mut old_state = chunk.instance_state.get(&idx).copied()
                    .unwrap_or_default();
                // r330-bughunt: floating-trunk support check. A log is flagged
                // for decay when the block directly below it is not structural
                // support (another log, or real ground). This cascades upward:
                // each log beneath turning to Air re-flags the one above next
                // tick. Index layout is x + z*CS + y*CS*CS, so the block directly
                // below is idx - CS*CS *within the same chunk*; at the chunk floor
                // (ly==0) it lives in the chunk directly beneath, which we look up
                // (None if that chunk isn't resident — see log_is_unsupported).
                if is_log {
                    let cs = crate::constants::CHUNK_SIZE as usize;
                    let below: Option<BlockId> = if ly > 0 {
                        // Same chunk, one layer down. ly>0 guarantees the
                        // subtraction can't underflow; .get keeps it panic-free.
                        chunk.blocks.get(idx as usize - cs * cs).copied()
                    } else {
                        // ly==0: the supporting block is the top layer of the
                        // chunk directly below. None when that chunk isn't loaded.
                        let below_coord = crate::world::chunk::ChunkCoord::new(
                            coord.x, coord.y - 1, coord.z);
                        let top_idx = lx as usize
                            + lz as usize * cs
                            + (cs - 1) * cs * cs;
                        ctx.chunks
                            .get(&below_coord)
                            .and_then(|e| e.value().blocks.get(top_idx).copied())
                    };
                    if log_is_unsupported(block, below) {
                        old_state.set_flag(BlockInstanceState::FLAG_DECAYING, true);
                    }
                }
                // r160: only ORPHANED / unsupported blocks decay; untouched
                // canopy is permanent.
                if !old_state.has_flag(BlockInstanceState::FLAG_DECAYING) {
                    continue;
                }
                let mut state = old_state;
                // age_per_block compensates for the band's tick divisor
                // AND any catch-up gap. See leaf_decay::tests::time_homogeneity_*
                // for the property that makes this safe.
                let died = advance_n_ticks(&mut state, wind, age_per_block);
                if died {
                    ctx.enqueue_edit(wx, wy, wz, BlockId::Air);
                } else if state != old_state {
                    ctx.enqueue_state(wx, wy, wz, state);
                }
            }

            // Mark this chunk as fired for this handler. Done after the
            // per-block loop so the next fire computes its gap against
            // this tick_id, not whatever was previous. Drop the borrow
            // first to avoid double-borrow on the chunks map (some
            // tracker impls might want to look at chunks).
            drop(entry);
            if let Some(t) = tracker {
                t.mark_fired(HANDLER_NAME, coord, tick_id);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── r330-bughunt: floating-trunk support — pure predicate tests ─────────

    #[test]
    fn trunk_on_air_is_unsupported() {
        assert!(log_is_unsupported(BlockId::OakLog, Some(BlockId::Air)));
        assert!(log_is_unsupported(BlockId::BirchLog, Some(BlockId::Air)));
    }

    #[test]
    fn trunk_on_ground_is_supported() {
        // Real ground (is_tree_anchorage) holds a trunk up.
        for ground in [BlockId::Stone, BlockId::Dirt, BlockId::Grass, BlockId::CoalOre] {
            assert!(block_supports_trunk(ground), "{ground:?} should be support");
            assert!(!log_is_unsupported(BlockId::OakLog, Some(ground)),
                "trunk on {ground:?} must not decay");
        }
    }

    #[test]
    fn trunk_on_log_is_supported() {
        // A log resting on the log below it (the trunk continuing down) is held.
        assert!(block_supports_trunk(BlockId::OakLog));
        assert!(block_supports_trunk(BlockId::BirchLog));
        assert!(!log_is_unsupported(BlockId::OakLog, Some(BlockId::OakLog)));
        assert!(!log_is_unsupported(BlockId::OakLog, Some(BlockId::BirchLog)));
    }

    #[test]
    fn trunk_on_non_structural_block_is_unsupported() {
        // Broadened from the r320 air-only check: leaves and fluids are not
        // structural support, so a trunk resting only on those is floating.
        for weak in [BlockId::OakLeaves, BlockId::BirchLeaves, BlockId::Water, BlockId::Flower] {
            assert!(!block_supports_trunk(weak), "{weak:?} is not support");
            assert!(log_is_unsupported(BlockId::OakLog, Some(weak)),
                "trunk resting only on {weak:?} must decay");
        }
    }

    #[test]
    fn non_log_is_never_flagged_by_support_rule() {
        // Leaves use the wind-age path, not the support rule — the predicate
        // must never flag a non-log regardless of what's below it.
        assert!(!log_is_unsupported(BlockId::OakLeaves, Some(BlockId::Air)));
        assert!(!log_is_unsupported(BlockId::Grass, Some(BlockId::Air)));
        assert!(!log_is_unsupported(BlockId::Air, Some(BlockId::Air)));
    }

    #[test]
    fn unknown_below_is_conservatively_supported() {
        // Below-chunk not resident → don't decay a trunk just because its
        // support hasn't streamed in. Avoids deleting trees at load seams.
        assert!(!log_is_unsupported(BlockId::OakLog, None));
        assert!(!log_is_unsupported(BlockId::BirchLog, None));
    }

    // ── r330-bughunt: handler integration — the §4e cross-seam + reachability ──

    /// A log on the floor of its chunk (ly==0) whose support lives in the chunk
    /// directly below: when that chunk is all-air, the trunk is floating and
    /// must be flagged + start aging. This is the exact case the r320 code
    /// skipped ("logs at the chunk floor are skipped — can't see below") AND
    /// never reached (logs aren't in plant_indices). Both are now fixed.
    #[test]
    fn floating_trunk_at_chunk_floor_decays() {
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::TickContext;
        use dashmap::DashMap;

        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        // Trunk segment at the chunk floor.
        let mut top = Chunk::empty(ChunkCoord::new(0, 0, 0));
        top.set(5, 0, 5, BlockId::OakLog);
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(top));
        // Chunk below is all air → no support beneath the trunk.
        chunks.insert(ChunkCoord::new(0, -1, 0),
            std::sync::Arc::new(Chunk::empty(ChunkCoord::new(0, -1, 0))));

        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "wind" { Some(0.3) } else { None }
        };
        let mut ctx = TickContext {
            chunks: &chunks,
            field_reader: Some(&reader),
            chunk_field_reader: None,
            player_chunk: None,
            tick_id: crate::world::ticks::TickId::ZERO,
            tick_period: 0.5,
            tick_dt: 0.5,
            world_time: 0.0,
            schedule: None,
            fire_tracker: None,
            block_edits: Vec::new(),
            state_changes: Vec::new(),
            transition_ops: Vec::new(),
            tree_root_ops: Vec::new(),
        };
        let handler = decay_handler();
        handler(&mut ctx);

        // The floating trunk should have been flagged decaying and aged one
        // tick (a state change, not yet an edit — it's freshly orphaned).
        let sc = ctx.state_changes.iter()
            .find(|s| (s.world_x, s.world_y, s.world_z) == (5, 0, 5))
            .expect("floating trunk at chunk floor must be flagged for decay");
        assert!(sc.new_state.has_flag(BlockInstanceState::FLAG_DECAYING),
            "floating trunk must be marked decaying");
        assert_eq!(sc.new_state.age_ticks, 1, "should have aged exactly one tick");
    }

    /// Same chunk-floor trunk, but the chunk below has solid ground at its top
    /// layer: the trunk is supported and must NOT decay. Guards against the
    /// broadened predicate over-decaying grounded trees across the seam.
    #[test]
    fn grounded_trunk_at_chunk_floor_survives() {
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::TickContext;
        use dashmap::DashMap;

        let cs = crate::constants::CHUNK_SIZE;
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        let mut top = Chunk::empty(ChunkCoord::new(0, 0, 0));
        top.set(5, 0, 5, BlockId::OakLog);
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(top));
        // Chunk below has Stone directly under the trunk (top layer, ly = CS-1).
        let mut below = Chunk::empty(ChunkCoord::new(0, -1, 0));
        below.set(5, cs - 1, 5, BlockId::Stone);
        chunks.insert(ChunkCoord::new(0, -1, 0), std::sync::Arc::new(below));

        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "wind" { Some(0.3) } else { None }
        };
        let mut ctx = TickContext {
            chunks: &chunks,
            field_reader: Some(&reader),
            chunk_field_reader: None,
            player_chunk: None,
            tick_id: crate::world::ticks::TickId::ZERO,
            tick_period: 0.5,
            tick_dt: 0.5,
            world_time: 0.0,
            schedule: None,
            fire_tracker: None,
            block_edits: Vec::new(),
            state_changes: Vec::new(),
            transition_ops: Vec::new(),
            tree_root_ops: Vec::new(),
        };
        let handler = decay_handler();
        handler(&mut ctx);

        assert!(ctx.state_changes.iter()
            .all(|s| (s.world_x, s.world_y, s.world_z) != (5, 0, 5)),
            "grounded trunk must not be flagged for decay");
        assert!(ctx.block_edits.iter()
            .all(|e| (e.world_x, e.world_y, e.world_z) != (5, 0, 5)),
            "grounded trunk must not be removed");
    }

    /// Mid-chunk floating trunk (ly>0, support in the same chunk): proves the
    /// reachability fix on its own — before r330 the log was never visited at
    /// all because logs aren't in plant_indices.
    #[test]
    fn mid_chunk_floating_trunk_is_now_scanned_and_decays() {
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::TickContext;
        use dashmap::DashMap;

        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        // Log at y=5 with nothing below it (air) — mid-chunk, same-chunk support.
        chunk.set(5, 5, 5, BlockId::OakLog);
        // Sanity: the log really is in wood_indices and NOT in plant_indices,
        // which is exactly why the old plant_indices-only loop missed it.
        let idx = Chunk::block_index(5, 5, 5) as u16;
        assert!(chunk.wood_indices.contains(&idx), "log must be in wood_indices");
        assert!(!chunk.plant_indices.contains(&idx), "log must NOT be in plant_indices");
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(chunk));

        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "wind" { Some(0.3) } else { None }
        };
        let mut ctx = TickContext {
            chunks: &chunks,
            field_reader: Some(&reader),
            chunk_field_reader: None,
            player_chunk: None,
            tick_id: crate::world::ticks::TickId::ZERO,
            tick_period: 0.5,
            tick_dt: 0.5,
            world_time: 0.0,
            schedule: None,
            fire_tracker: None,
            block_edits: Vec::new(),
            state_changes: Vec::new(),
            transition_ops: Vec::new(),
            tree_root_ops: Vec::new(),
        };
        let handler = decay_handler();
        handler(&mut ctx);

        let sc = ctx.state_changes.iter()
            .find(|s| (s.world_x, s.world_y, s.world_z) == (5, 5, 5))
            .expect("mid-chunk floating trunk must now be scanned and flagged");
        assert!(sc.new_state.has_flag(BlockInstanceState::FLAG_DECAYING));
    }

    #[test]
    fn leaf_decay_progresses_under_calm_wind() {
        let mut s = BlockInstanceState::default();
        let died = advance_one_tick(&mut s, 0.1);
        assert!(!died);
        assert_eq!(s.age_ticks, 1);
        assert!(s.has_flag(BlockInstanceState::FLAG_DECAYING));
    }

    #[test]
    fn leaf_decay_accelerates_under_gusty_wind() {
        let mut calm = BlockInstanceState::default();
        let mut gusty = BlockInstanceState::default();
        for _ in 0..20 {
            advance_one_tick(&mut calm, 0.1);
            advance_one_tick(&mut gusty, 0.9);
        }
        assert!(gusty.age_ticks > calm.age_ticks,
            "gusty leaves should decay faster: calm={} gusty={}",
            calm.age_ticks, gusty.age_ticks);
    }

    #[test]
    fn leaf_eventually_dies() {
        let mut s = BlockInstanceState::default();
        let mut died = false;
        for _ in 0..(LEAF_DEATH_AGE_TICKS as usize + 5) {
            if advance_one_tick(&mut s, 0.1) { died = true; break; }
        }
        assert!(died);
    }

    #[test]
    fn color_modulation_progresses() {
        let early = decay_color_modulation(5);
        let mid = decay_color_modulation(LEAF_DEATH_AGE_TICKS / 2);
        let late = decay_color_modulation(LEAF_DEATH_AGE_TICKS - 1);
        // Red rises, blue falls.
        assert!(mid[0] > early[0]);
        assert!(late[0] > mid[0]);
        assert!(mid[2] < early[2]);
        assert!(late[2] < mid[2]);
    }

    #[test]
    fn decay_handler_kills_leaves_when_old() {
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::blocks::BlockId;
        use crate::world::ticks::TickContext;
        use dashmap::DashMap;

        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(5, 5, 5, BlockId::OakLeaves);
        // Pre-age the leaf to just below the death threshold so one tick
        // pushes it over.
        let pre_state = BlockInstanceState {
            age_ticks: LEAF_DEATH_AGE_TICKS - 1,
            flags: BlockInstanceState::FLAG_DECAYING,
        };
        chunk.set_instance_state(5, 5, 5, pre_state);

        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(chunk));

        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "wind" { Some(0.3) } else { None }
        };
        let mut ctx = TickContext {
            chunks: &chunks,
            field_reader: Some(&reader),
                chunk_field_reader: None,
            player_chunk: None,
            tick_id: crate::world::ticks::TickId::ZERO,
            tick_period: 0.5,
            tick_dt: 0.5,
            world_time: 0.0,
            schedule: None,
            fire_tracker: None,
            block_edits: Vec::new(),
            state_changes: Vec::new(),
            transition_ops: Vec::new(),
            tree_root_ops: Vec::new(),
        };

        let handler = decay_handler();
        handler(&mut ctx);

        // Leaf should be queued for removal.
        assert_eq!(ctx.block_edits.len(), 1);
        assert_eq!(ctx.block_edits[0].new_block, BlockId::Air);
        assert_eq!((ctx.block_edits[0].world_x,
                    ctx.block_edits[0].world_y,
                    ctx.block_edits[0].world_z), (5, 5, 5));
    }

    /// Time-homogeneity: advancing N ticks at once must produce the same
    /// final state as N consecutive single-tick advances. This is the
    /// property that makes catch-up safe — a chunk that re-enters tick
    /// range after being away can apply N ticks of accumulated decay
    /// without diverging from a chunk that ticked normally for N ticks.
    ///
    /// Verified for both calm and gusty wind, since the per-tick effect
    /// differs by wind regime.
    #[test]
    fn time_homogeneity_calm_wind() {
        let wind = 0.1;
        let n = 50u32;

        let mut all_at_once = BlockInstanceState::default();
        let died_atomic = advance_n_ticks(&mut all_at_once, wind, n);

        let mut step_by_step = BlockInstanceState::default();
        let mut died_step = false;
        for _ in 0..n {
            died_step = advance_one_tick(&mut step_by_step, wind);
        }

        assert_eq!(all_at_once.age_ticks, step_by_step.age_ticks,
            "calm-wind: N-at-once age={} but step-by-step age={}",
            all_at_once.age_ticks, step_by_step.age_ticks);
        assert_eq!(all_at_once.flags, step_by_step.flags,
            "calm-wind: flag bits diverged");
        assert_eq!(died_atomic, died_step,
            "calm-wind: death status diverged");
    }

    #[test]
    fn time_homogeneity_gusty_wind() {
        let wind = 0.9;
        let n = 50u32;

        let mut all_at_once = BlockInstanceState::default();
        let died_atomic = advance_n_ticks(&mut all_at_once, wind, n);

        let mut step_by_step = BlockInstanceState::default();
        let mut died_step = false;
        for _ in 0..n {
            died_step = advance_one_tick(&mut step_by_step, wind);
        }

        assert_eq!(all_at_once.age_ticks, step_by_step.age_ticks,
            "gusty-wind: N-at-once age={} but step-by-step age={}",
            all_at_once.age_ticks, step_by_step.age_ticks);
        assert_eq!(died_atomic, died_step,
            "gusty-wind: death status diverged");
    }

    /// Catch-up edge case: a leaf that's been "away" long enough that
    /// N ticks would push it past the death threshold should die in
    /// exactly the same way as if it had ticked normally and crossed
    /// the threshold mid-stride. The "died" return must be true and
    /// the age must be saturated correctly.
    #[test]
    fn time_homogeneity_crosses_death_threshold() {
        let wind = 0.1;
        let huge_n = 10_000u32; // Way past LEAF_DEATH_AGE_TICKS

        let mut all_at_once = BlockInstanceState::default();
        let died_atomic = advance_n_ticks(&mut all_at_once, wind, huge_n);

        assert!(died_atomic, "leaf should have died after {} ticks", huge_n);
        // age_ticks is u8 — saturates at 255.
        assert_eq!(all_at_once.age_ticks, u8::MAX);
        assert!(all_at_once.has_flag(BlockInstanceState::FLAG_DECAYING));
    }

    /// Sanity: advance_n_ticks(_, _, 0) is a no-op for a fresh leaf
    /// (returns false because not yet aged enough to die) and a yes-op
    /// for an already-dead leaf (returns true because age >= threshold).
    #[test]
    fn advance_zero_ticks_is_query() {
        let mut fresh = BlockInstanceState::default();
        assert!(!advance_n_ticks(&mut fresh, 0.5, 0));
        assert_eq!(fresh.age_ticks, 0);

        let mut dead = BlockInstanceState {
            age_ticks: LEAF_DEATH_AGE_TICKS,
            flags: 0,
        };
        assert!(advance_n_ticks(&mut dead, 0.5, 0));
        assert_eq!(dead.age_ticks, LEAF_DEATH_AGE_TICKS);
    }

    /// Catch-up integration: a leaf that's been "away" (no fires)
    /// for many ticks should, on its next fire, age by the elapsed-tick
    /// gap so it ends up in the same state as if it had been ticking
    /// normally the whole time. This exercises the LastFireTracker
    /// path inside the handler.
    ///
    /// Scenario:
    ///   - Tick 0: Two chunks created with fresh leaves. Both fire.
    ///   - Ticks 1..N-1: Only chunk A fires (chunk B is "in CatchUp band").
    ///     We simulate B being in CatchUp by simply not having the player
    ///     in range — but the trick is: how do we make A fire while B
    ///     doesn't? Easiest: put A in Render band (player nearby) and
    ///     B in CatchUp band (player far). Then at tick N, "teleport"
    ///     the player so B is now in Render. Tracker says elapsed=N-1
    ///     since B's last fire, handler advances B by N-1 → matches A.
    #[test]
    fn catchup_via_tracker_matches_continuous() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::scheduler::{LastFireTracker, TickScheduleParams};
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        let total_ticks = 50u64;
        let params = TickScheduleParams {
            render_distance: 4,
            simulation_distance: 8,
            simulation_divisor: 8,
        };

        // Two chunks at different fixed positions.
        let chunk_a_coord = ChunkCoord::new(0, 0, 0);
        let chunk_b_coord = ChunkCoord::new(20, 0, 0); // far — CatchUp by default

        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        for &c in &[chunk_a_coord, chunk_b_coord] {
            let mut chunk = Chunk::empty(c);
            chunk.set(5, 5, 5, BlockId::OakLeaves);
            chunks.insert(c, std::sync::Arc::new(chunk));
        }

        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "wind" { Some(0.3) } else { None }
        };
        let handler = decay_handler();
        let tracker = LastFireTracker::new();

        // For ticks 0..total_ticks-1, player is at chunk_a (so A in
        // Render, B in CatchUp). Only A fires. B should not.
        let player_near_a = chunk_a_coord;
        for raw in 0..(total_ticks - 1) {
            let mut ctx = TickContext {
                chunks: &chunks,
                field_reader: Some(&reader),
                chunk_field_reader: None,
                player_chunk: Some(player_near_a),
                tick_id: TickId(raw),
                tick_period: 0.5,
                tick_dt: 0.5,
                world_time: 0.0,
                schedule: Some(params),
                fire_tracker: Some(&tracker),
                block_edits: Vec::new(),
                state_changes: Vec::new(),
                transition_ops: Vec::new(),
            tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);
            // Apply state changes back to chunks.
            for sc in &ctx.state_changes {
                let cc = ChunkCoord::new(
                    sc.world_x.div_euclid(crate::constants::CHUNK_SIZE),
                    sc.world_y.div_euclid(crate::constants::CHUNK_SIZE),
                    sc.world_z.div_euclid(crate::constants::CHUNK_SIZE),
                );
                if let Some(entry) = chunks.get(&cc) {
                    let mut new = (**entry.value()).clone();
                    let lx = sc.world_x.rem_euclid(crate::constants::CHUNK_SIZE);
                    let ly = sc.world_y.rem_euclid(crate::constants::CHUNK_SIZE);
                    let lz = sc.world_z.rem_euclid(crate::constants::CHUNK_SIZE);
                    new.set_instance_state(lx, ly, lz, sc.new_state);
                    drop(entry);
                    chunks.insert(cc, std::sync::Arc::new(new));
                }
            }
        }

        // After the loop, chunk A should be at age = total_ticks - 1
        // (fired every tick from 0 to total_ticks-2 inclusive).
        let render_idx = Chunk::block_index(5, 5, 5) as u16;
        let a_state = chunks.get(&chunk_a_coord).unwrap()
            .instance_state.get(&render_idx).copied().unwrap_or_default();
        let b_state_before = chunks.get(&chunk_b_coord).unwrap()
            .instance_state.get(&render_idx).copied().unwrap_or_default();
        assert_eq!(a_state.age_ticks as u64, total_ticks - 1,
            "chunk A should have aged total_ticks-1 times under continuous fire");
        assert_eq!(b_state_before.age_ticks, 0,
            "chunk B should not have aged at all — was in CatchUp band the whole time");

        // Now teleport player next to chunk B. On the next tick, B
        // enters Render band. The tracker says "B has never fired, so
        // gap = default_n = 1" — wait no, that's wrong for catch-up
        // semantics. Let me re-think...
        //
        // Hmm, the tracker only tracks chunks that have ACTUALLY FIRED
        // before. Chunk B has never fired (was always in CatchUp). On
        // its first-ever fire, the tracker returns the default which
        // for Render band is 1, not the catch-up gap.
        //
        // This is a subtlety in the catch-up design: a chunk that's
        // been away gets caught up only if it fired BEFORE going away.
        // First-ever fires don't get a catch-up bonus. That's actually
        // correct for most cases — the handler shouldn't pretend a
        // freshly-loaded chunk has been "evolving offscreen" if it
        // wasn't part of the simulation yet. But for our test scenario
        // (B was in CatchUp from tick 0), we'd want catch-up if B was
        // notionally "alive" the whole time.
        //
        // We can model this by manually marking B's "first fire" at
        // tick 0 in the tracker, simulating "B was alive but not in a
        // tick band." Then when the player teleports, B's gap becomes
        // total_ticks-1 and catch-up kicks in.
        tracker.mark_fired("leaf_decay", chunk_b_coord, TickId(0));
        // Re-simulate: at tick total_ticks - 1, the elapsed gap for B
        // since its (manually-marked) tick-0 fire should be total_ticks-1.
        let elapsed = tracker.elapsed_or_default(
            "leaf_decay", chunk_b_coord, TickId(total_ticks - 1), 1);
        assert_eq!(elapsed as u64, total_ticks - 1,
            "tracker should report full elapsed gap for catch-up");

        // Now actually fire chunk B once with the catch-up gap. Player
        // moves to chunk B's vicinity.
        let player_near_b = chunk_b_coord;
        let mut ctx = TickContext {
            chunks: &chunks,
            field_reader: Some(&reader),
                chunk_field_reader: None,
            player_chunk: Some(player_near_b),
            tick_id: TickId(total_ticks - 1),
            tick_period: 0.5,
            tick_dt: 0.5,
            world_time: 0.0,
            schedule: Some(params),
            fire_tracker: Some(&tracker),
            block_edits: Vec::new(),
            state_changes: Vec::new(),
            transition_ops: Vec::new(),
            tree_root_ops: Vec::new(),
        };
        handler(&mut ctx);
        // Apply.
        for sc in &ctx.state_changes {
            let cc = ChunkCoord::new(
                sc.world_x.div_euclid(crate::constants::CHUNK_SIZE),
                sc.world_y.div_euclid(crate::constants::CHUNK_SIZE),
                sc.world_z.div_euclid(crate::constants::CHUNK_SIZE),
            );
            if let Some(entry) = chunks.get(&cc) {
                let mut new = (**entry.value()).clone();
                let lx = sc.world_x.rem_euclid(crate::constants::CHUNK_SIZE);
                let ly = sc.world_y.rem_euclid(crate::constants::CHUNK_SIZE);
                let lz = sc.world_z.rem_euclid(crate::constants::CHUNK_SIZE);
                new.set_instance_state(lx, ly, lz, sc.new_state);
                drop(entry);
                chunks.insert(cc, std::sync::Arc::new(new));
            }
        }

        // After catch-up, B's age should equal A's age (= total_ticks - 1).
        let b_state_after = chunks.get(&chunk_b_coord).unwrap()
            .instance_state.get(&render_idx).copied().unwrap_or_default();
        assert_eq!(b_state_after.age_ticks, a_state.age_ticks,
            "catch-up failed: A age={} B age={} after catch-up fire",
            a_state.age_ticks, b_state_after.age_ticks);
    }

    /// Band-aware integration test: a leaf in the Render band ticked over
    /// N game-ticks must reach the same final state as a leaf in the
    /// Simulation band ticked over N game-ticks. The Render leaf fires
    /// every tick at age=1; the Simulation leaf fires every Nth tick at
    /// age=N. End state must match.
    ///
    /// This is the property that makes the distance-based scheduler safe:
    /// the same leaf in different bands ages identically per unit of
    /// game-tick wall time. A player can walk away from a tree (band
    /// drops to Simulation) and back (band rises to Render) without the
    /// tree's leaves diverging from "what they would have been if they'd
    /// stayed in Render the whole time."
    #[test]
    fn band_aware_leaves_match_render_after_n_ticks() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::scheduler::TickScheduleParams;
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        let total_ticks = 64u64; // multiple of simulation_divisor (8)
        let params = TickScheduleParams {
            render_distance: 4,
            simulation_distance: 12,
            simulation_divisor: 8,
        };

        // Build two chunks each with a fresh OakLeaves block, in two
        // different positions: one inside render distance, one inside
        // simulation distance. Same player position.
        let player_chunk = ChunkCoord::new(0, 0, 0);
        let render_chunk_coord = ChunkCoord::new(2, 0, 0);   // distance 2 → Render
        let sim_chunk_coord = ChunkCoord::new(8, 0, 0);      // distance 8 → Simulation

        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        for &c in &[render_chunk_coord, sim_chunk_coord] {
            let mut chunk = Chunk::empty(c);
            chunk.set(5, 5, 5, BlockId::OakLeaves);
            chunks.insert(c, std::sync::Arc::new(chunk));
        }

        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "wind" { Some(0.3) } else { None }
        };
        let handler = decay_handler();

        // Run total_ticks ticks; each tick the handler decides per-chunk
        // whether to fire (Render: every tick. Simulation: every 8th).
        for raw in 0..total_ticks {
            let mut ctx = TickContext {
                chunks: &chunks,
                field_reader: Some(&reader),
                chunk_field_reader: None,
                player_chunk: Some(player_chunk),
                tick_id: TickId(raw),
                tick_period: 0.5,
                tick_dt: 0.5,
                world_time: 0.0,
                schedule: Some(params),
                fire_tracker: None,
                block_edits: Vec::new(),
                state_changes: Vec::new(),
                transition_ops: Vec::new(),
            tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);

            // Apply state changes back to chunks (the real driver does this
            // via ChunkManager; here we apply them inline for the test).
            for sc in &ctx.state_changes {
                let cc = ChunkCoord::new(
                    sc.world_x.div_euclid(crate::constants::CHUNK_SIZE),
                    sc.world_y.div_euclid(crate::constants::CHUNK_SIZE),
                    sc.world_z.div_euclid(crate::constants::CHUNK_SIZE),
                );
                if let Some(entry) = chunks.get(&cc) {
                    let mut new = (**entry.value()).clone();
                    let lx = sc.world_x.rem_euclid(crate::constants::CHUNK_SIZE);
                    let ly = sc.world_y.rem_euclid(crate::constants::CHUNK_SIZE);
                    let lz = sc.world_z.rem_euclid(crate::constants::CHUNK_SIZE);
                    new.set_instance_state(lx, ly, lz, sc.new_state);
                    drop(entry);
                    chunks.insert(cc, std::sync::Arc::new(new));
                }
            }
        }

        // Inspect final ages.
        let render_idx = Chunk::block_index(5, 5, 5) as u16;
        let render_state = chunks.get(&render_chunk_coord).unwrap()
            .instance_state.get(&render_idx).copied().unwrap_or_default();
        let sim_state = chunks.get(&sim_chunk_coord).unwrap()
            .instance_state.get(&render_idx).copied().unwrap_or_default();

        assert_eq!(render_state.age_ticks, sim_state.age_ticks,
            "render leaf age={} but simulation leaf age={} after {} game-ticks — bands diverged",
            render_state.age_ticks, sim_state.age_ticks, total_ticks);
        // Sanity: both should have aged to ~total_ticks (calm wind = 1 per tick).
        assert_eq!(render_state.age_ticks as u64, total_ticks,
            "render leaf should have aged exactly total_ticks under calm wind");
    }
}
