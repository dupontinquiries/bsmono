//! Plant lifecycle: stages, transitions, and visual modulation.
//!
//! Plants progress through a small state machine driven by tick aging
//! (modulated by the local moisture field). The stage determines:
//!   - Growth rate (faster when wet)
//!   - Color modulation at mesh time (greens → yellows → browns)
//!   - Eventual death (block becomes Air)
//!
//! Stages are stored in the lower 3 bits of `BlockInstanceState::flags`.
//! `age_ticks` measures progress within the current stage; transitions
//! happen at thresholds modulated by moisture.
//!
//! Tunables (constants) are deliberately fast for visible MVP iteration —
//! a "real" game would want plants taking minutes-to-hours to mature, not
//! seconds. Tweak `STAGE_*_DURATION_TICKS` to slow things down later.

use crate::world::chunk::BlockInstanceState;

/// Bit mask carving out the stage portion of the flags byte.
pub const STAGE_MASK: u8 = 0b0000_0111;
/// Bits in `flags` reserved for future use (decaying, withered-from-fire,
/// etc.). Currently `FLAG_WITHERED` and `FLAG_DECAYING` from
/// BlockInstanceState live in bits 4+ to avoid colliding with stage bits.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Stage {
    Seedling = 0,
    Growing = 1,
    Mature = 2,
    Drying = 3,
    Withered = 4,
}

impl Stage {
    pub fn from_flags(flags: u8) -> Self {
        match flags & STAGE_MASK {
            0 => Stage::Seedling,
            1 => Stage::Growing,
            2 => Stage::Mature,
            3 => Stage::Drying,
            _ => Stage::Withered,
        }
    }

    pub fn into_flags(self, prev_flags: u8) -> u8 {
        (prev_flags & !STAGE_MASK) | (self as u8 & STAGE_MASK)
    }
}

/// Tick durations spent in each stage before transitioning. Total
/// lifetime ≈ 5 + 15 + 45 = 65 ticks ≈ 32s (at 0.5s tick period). Plus
/// optional drying → withered → death tail if conditions are bad.
///
/// These are real-time-scale for iteration. Adjust later for gameplay.
pub const STAGE_SEEDLING_DURATION_TICKS: u8 = 10;   // ~5s
pub const STAGE_GROWING_DURATION_TICKS:  u8 = 30;   // ~15s
pub const STAGE_MATURE_DURATION_TICKS:   u8 = 60;   // ~30s if dry, else stay mature
pub const STAGE_DRYING_DURATION_TICKS:   u8 = 30;   // ~15s

/// Moisture threshold below which plants start drying. Above this, mature
/// plants stay mature indefinitely.
pub const DRY_MOISTURE_THRESHOLD: f32 = 0.40;

/// Maximum tree cluster diameter before growth events stop firing.
/// Diameter = largest Chebyshev distance between any two wood blocks
/// in the same connected cluster. A cap of 100 means trees can grow
/// quite large — taller than a chunk (16) and wider than typical
/// canopies. Worldgen-placed trees start at diameter 8-12, so this
/// gives ~10× headroom for organic growth.
///
/// At 100, individual trees can span chunk boundaries. The
/// flood-fill in `TreeRule` is configured with a cap of 128 to
/// give comfortable margin (clusters at the cap shouldn't be
/// rejected as overflow).
pub const TREE_DIAMETER_CAP: i32 = 100;

/// Soft cap on tree cluster size (count of wood blocks) used to
/// taper growth probability. Above this, growth probability scales
/// linearly toward zero — a tree at the cap effectively stops growing.
/// Below it, growth fires at full rate.
///
/// Tuned for "small Minecraft-y" trees: a healthy mature tree settles
/// around 25-35 wood blocks. The hard TreeRule flood-fill cap is 128;
/// the soft cap leaves comfortable margin so we never collide with it.
///
/// Without this gate, the growth feedback loop runs away: each growth
/// event spawns 5 new leaves around the new wood, which become 5 new
/// growth candidates. At fixed humidity, the eligible-leaf surface
/// grows ~quadratically with tree size while the per-chunk sample
/// budget is fixed — so trees fan out into 10×10×10 blobs instead of
/// stabilizing. The cluster-size gate is what gives organic-looking
/// terminal size.
pub const TREE_CLUSTER_SOFT_CAP: usize = 30;

/// Maximum number of TreeGrowth events that can fire on one chunk in
/// one tick. Without this cap, the per-leaf growth probability fires
/// independently for every sample in `SAMPLES_PER_CHUNK_PER_TICK`,
/// which means a chunk full of leaves can grow ~8 wood blocks per
/// tick during heated periods. With the cap, growth is bounded to a
/// gentle organic creep regardless of how many leaves are in the
/// chunk.
///
/// Combined with the cluster-size soft cap, this produces visible
/// growth (one block per chunk every few ticks under good conditions)
/// without the runaway fan-out behavior. Trees grow in slow steady
/// motion rather than fast pixelated explosion.
pub const TREE_GROWTH_EVENTS_PER_CHUNK_PER_TICK: usize = 2;

/// Maximum number of TreeShrink events that can fire on one chunk
/// in one tick. Mirror of `TREE_GROWTH_EVENTS_PER_CHUNK_PER_TICK`.
/// Same cap value: at neutral moisture (0.4) growth and shrink fire
/// at equal rates, the tree's edge breathes; in wet conditions
/// growth wins; in dry conditions shrink wins.
///
/// If shrink fired faster than growth at neutral moisture, trees
/// would slowly evaporate even in stable forest conditions. Keeping
/// the rate caps equal makes net growth/shrink a clean function
/// of moisture alone.
pub const TREE_SHRINK_EVENTS_PER_CHUNK_PER_TICK: usize = 2;

/// Color modulation applied to sub-voxel cells based on lifecycle stage.
/// Multiplied component-wise with each cell's base color before shading.
/// Designed to read at a glance: green plant → yellow drying → brown
/// withered.
pub fn stage_color_modulation(stage: Stage) -> [f32; 3] {
    match stage {
        // Seedlings: distinctly pale / yellow-green ("baby leaf" vibe).
        // Boost red and green strongly, cut blue. Greens look chartreuse-y.
        Stage::Seedling => [1.30, 1.20, 0.70],
        // Growing: closer to neutral but slightly brightened — a healthy
        // young plant that's starting to fill out. NOT identical to
        // Mature so users see a transition.
        Stage::Growing  => [1.05, 1.10, 0.90],
        // Mature: deep saturated green. Distinctly darker than Growing.
        // Greens punch, reds and blues drop.
        Stage::Mature   => [0.75, 1.00, 0.65],
        // Drying: straw yellow — reds boosted, greens reduced, blues crushed.
        Stage::Drying   => [1.50, 1.10, 0.40],
        // Withered: deep brown — green crushed further, red high, blue near 0.
        Stage::Withered => [1.20, 0.50, 0.20],
    }
}

/// Advance one plant tick. Mutates the input state in place. Returns the
/// new stage so the caller can decide if the block should die (returns
/// Withered + age_ticks > death threshold → caller replaces block with Air).
///
/// `moisture` is the local moisture field value, [0, 1].
pub fn advance_one_tick(state: &mut BlockInstanceState, moisture: f32) -> Stage {
    advance_n_ticks(state, moisture, 1)
}

/// Per-tick age increment given moisture. Wet (> 0.6) adds 2 per tick;
/// otherwise 1. This matches the wet_bonus pattern in the original
/// `advance_one_tick` and is shared between the per-tick fast path
/// and the catch-up loop in `advance_n_ticks`.
#[inline]
fn age_increment_per_tick(moisture: f32) -> u8 {
    if moisture > 0.6 { 2 } else { 1 }
}

/// Ticks-until-stage-transition for the current state. Returns None
/// if the current stage doesn't have a deterministic age-based
/// transition (Mature with high moisture, Withered, Drying with
/// moisture > 0.7 — handled via the per-iteration recovery branch).
///
/// This is the core helper for catch-up: it lets us advance directly
/// to the next transition boundary without simulating every tick in
/// between.
fn ticks_to_transition(stage: Stage, age: u8, moisture: f32) -> Option<u8> {
    let per_tick = age_increment_per_tick(moisture);
    let threshold = match stage {
        Stage::Seedling => Some(STAGE_SEEDLING_DURATION_TICKS),
        Stage::Growing  => Some(STAGE_GROWING_DURATION_TICKS),
        Stage::Mature => {
            // Only transitions to Drying when moisture is low enough.
            if moisture < DRY_MOISTURE_THRESHOLD {
                Some(STAGE_MATURE_DURATION_TICKS)
            } else {
                None
            }
        }
        Stage::Drying => Some(STAGE_DRYING_DURATION_TICKS),
        Stage::Withered => None, // terminal — no further transition
    }?;
    if age >= threshold {
        // Already at/past threshold — would transition next tick.
        Some(0)
    } else {
        // ceil((threshold - age) / per_tick)
        let remaining = threshold - age;
        Some((remaining + per_tick - 1) / per_tick)
    }
}

/// Per-tick probability that a tree-root marker walks 1 block to a
/// neighboring anchored wood block when its current position has lost
/// anchorage. Slow on purpose: the marker should be "sticky," only
/// migrating after sustained adverse conditions.
///
/// At default 0.5s tick rate, 0.05 means ~20 ticks (~10 real seconds)
/// average before a walk fires. Together with the "walk by exactly 1
/// block" rule, this produces the curved-base aesthetic the user
/// requested: erosion on one side gradually pulls the marker laterally
/// without snapping the geometry above.
pub const STICKY_WALK_PROBABILITY: f32 = 0.05;

/// Should we rebuild this chunk's POI cache this tick?
///
/// **Current policy (v0.24.70)**: rebuild iff
/// `chunk.poi_cache.is_none() || chunk.structural_dirty`. An idle
/// chunk with a built cache and no wood transitions never rebuilds —
/// paying the POI cost once per chunk lifetime instead of every
/// `POI_REFRESH_INTERVAL_TICKS` ticks (the previous placeholder).
///
/// The dirty flag is set by `Chunk::set()` on any wood transition
/// (add/remove/species swap of `OakLog`/`BirchLog`). It's cleared
/// when InstallPoi applies a fresh cache (see
/// `chunk_manager.rs` InstallPoi op).
///
/// The `tick_id` parameter is unused; kept for API stability.
#[inline]
fn should_refresh_poi(
    chunk: &crate::world::chunk::Chunk,
    _tick_id: crate::world::ticks::TickId,
) -> bool {
    // v0.24.70-structural-dirty: replaces the placeholder
    // POI_REFRESH_INTERVAL_TICKS cadence with explicit dirty
    // marking. Rebuild iff:
    //   - No cache exists yet (first encounter), OR
    //   - The chunk's structural_dirty flag is set (a wood
    //     transition happened since the last rebuild).
    //
    // An idle chunk with a built cache and no edits never
    // rebuilds — paying the cost once per chunk lifetime instead
    // of every POI_REFRESH_INTERVAL_TICKS ticks.
    //
    // The tick_id parameter is unused; kept for API stability.
    chunk.poi_cache.is_none() || chunk.structural_dirty
}

/// Build a fresh POI cache for the given chunk. Walks the chunk's
/// wood blocks and groups them into clusters (via cross-chunk-aware
/// `MultiChunkSource`); for each cluster whose anchor lives in this
/// chunk, emits a `TreePoi` entry.
///
/// **"Anchor lives in this chunk"** means: the canonical anchor
/// candidate (lowest-y member touching `is_tree_anchorage()` ground;
/// tiebroken by canonical (y, z, x) order) lies inside this chunk.
/// A tree spanning chunks A and B with anchor in A appears in A's
/// cache and NOT in B's. This avoids double-counting at the cost of
/// occasionally building a cache for a chunk whose visible wood is
/// "owned" by another chunk's cache.
///
/// **Cost**: one MultiChunkSource flood-fill per distinct wood
/// cluster touching this chunk. Trees are bounded by TREE_CLUSTER_MAX
/// (~128) so each fill is small. Total chunk cost = O(distinct trees
/// × tree size). For a forested chunk with ~10 trees of ~50 wood
/// blocks each = ~500 source lookups, ~30 µs typical.
/// Result of a POI build: the cache itself, plus a list of world
/// positions where a tree-root marker should be set because the
/// corresponding tree currently has no marker on any member.
///
/// The caller (lifecycle handler) emits `SetMarker` ops for these
/// positions and an `InstallPoiCache` op for the cache.
struct PoiBuildResult {
    cache: crate::world::chunk::ChunkPoiCache,
    /// Anchor positions where a SetMarker op should be enqueued.
    /// Each entry: an anchored cluster with no existing marker on
    /// any of its members. The position is the canonical anchor
    /// (lowest-y wood touching anchorage).
    markers_to_set: Vec<[i32; 3]>,
}

fn build_poi_cache(
    chunk: &crate::world::chunk::Chunk,
    coord: crate::world::chunk::ChunkCoord,
    chunks: &dashmap::DashMap<crate::world::chunk::ChunkCoord, std::sync::Arc<crate::world::chunk::Chunk>>,
    tick_id: crate::world::ticks::TickId,
) -> PoiBuildResult {
    use crate::world::chunk::{Chunk, ChunkPoiCache, TreePoi};
    use crate::world::cluster::{cluster_at, MultiChunkSource, TreeRule};
    use crate::constants::CHUNK_SIZE;

    let (origin_x, origin_y, origin_z) = coord.block_origin();
    let _ = (origin_y, origin_z); // origins consumed below via chunk-local conversion

    // Use the chunk's wood_indices inverted index. Maintained
    // incrementally by Chunk::set() and seeded once at chunk load by
    // from_blocks(). For chunks with no wood (~70% of loaded chunks
    // — underground stone, ocean, desert, etc.) this is an empty
    // BTreeSet check, not a 32K linear scan.
    if chunk.wood_indices.is_empty() {
        return PoiBuildResult {
            cache: ChunkPoiCache::empty(tick_id),
            markers_to_set: Vec::new(),
        };
    }

    let source = MultiChunkSource {
        get_chunk: |c| chunks.get(&c).map(|r| r.value().clone()),
    };

    let mut visited: std::collections::HashSet<[i32; 3]> = std::collections::HashSet::new();
    let mut tree_pois: Vec<TreePoi> = Vec::new();
    let mut markers_to_set: Vec<[i32; 3]> = Vec::new();

    for &idx in chunk.wood_indices.iter() {
        let (lx, ly, lz) = Chunk::index_to_xyz(idx);
        let wx = origin_x + lx;
        let wy = chunk.coord.block_origin().1 + ly;
        let wz = chunk.coord.block_origin().2 + lz;
        let wpos = [wx, wy, wz];
        if visited.contains(&wpos) { continue; }

        let cluster = cluster_at(wx, wy, wz, &TreeRule, &source);
        if cluster.id.is_anchored() {
            visited.insert(wpos);
            continue;
        }

        for &m in &cluster.members {
            visited.insert(m);
        }

        let anchor_candidate = cluster.members.iter().copied().find(|&[mx, my, mz]| {
            let neighbors = [
                (mx - 1, my,     mz),
                (mx + 1, my,     mz),
                (mx,     my - 1, mz),
                (mx,     my + 1, mz),
                (mx,     my,     mz - 1),
                (mx,     my,     mz + 1),
            ];
            neighbors.iter().any(|&(nx, ny, nz)| {
                read_block_world(chunks, nx, ny, nz)
                    .map(|b| b.is_tree_anchorage())
                    .unwrap_or(false)
            })
        });

        let canonical_pos = anchor_candidate.unwrap_or(cluster.members[0]);
        let canonical_chunk = {
            let cx = canonical_pos[0].div_euclid(CHUNK_SIZE);
            let cy = canonical_pos[1].div_euclid(CHUNK_SIZE);
            let cz = canonical_pos[2].div_euclid(CHUNK_SIZE);
            crate::world::chunk::ChunkCoord::new(cx, cy, cz)
        };
        if canonical_chunk != coord {
            continue;
        }

        // Marker discovery: if the cluster has an anchor candidate
        // AND no current member has FLAG_TREE_ROOT set, emit a
        // SetMarker for the canonical anchor. This is how trees
        // first acquire their root marker — on the POI build that
        // first sees them with anchorage.
        if let Some(anchor) = anchor_candidate {
            let any_marker = cluster.members.iter().any(|&[mx, my, mz]| {
                let cc_x = mx.div_euclid(CHUNK_SIZE);
                let cc_y = my.div_euclid(CHUNK_SIZE);
                let cc_z = mz.div_euclid(CHUNK_SIZE);
                let llx = mx.rem_euclid(CHUNK_SIZE);
                let lly = my.rem_euclid(CHUNK_SIZE);
                let llz = mz.rem_euclid(CHUNK_SIZE);
                let cc = crate::world::chunk::ChunkCoord::new(cc_x, cc_y, cc_z);
                chunks.get(&cc).map(|r| {
                    let mi = Chunk::block_index(llx, lly, llz) as u16;
                    r.value().tree_root_indices.contains(&mi)
                }).unwrap_or(false)
            });
            if !any_marker {
                markers_to_set.push(anchor);
            }
        }

        tree_pois.push(TreePoi {
            anchor_candidate,
            cluster_bottom: cluster.members[0],
            cluster_size: cluster.members.len(),
            diameter: cluster.diameter(),
        });
    }

    PoiBuildResult {
        cache: ChunkPoiCache {
            computed_at_tick: tick_id,
            tree_pois,
        },
        markers_to_set,
    }
}

/// Deterministic [0, 1) random for the sticky-walk gate. Hashes
/// (marker world position, tick_id) into a u32 then normalizes.
/// Same input always yields the same output — replay determinism is
/// preserved (a player who replays the same world state from the same
/// tick sees the same walks fire).
#[inline]
fn sticky_walk_random(wx: i32, wy: i32, wz: i32, tick_id: crate::world::ticks::TickId) -> f32 {
    let mut h: u64 = 0x9E3779B97F4A7C15;
    h ^= (wx as i64 as u64).wrapping_mul(0xBF58476D1CE4E5B9);
    h ^= (wy as i64 as u64).wrapping_mul(0x94D049BB133111EB);
    h ^= (wz as i64 as u64).wrapping_mul(0x2545F4914F6CDD1D);
    h ^= tick_id.0.wrapping_mul(0xC2B2AE3D27D4EB4F);
    // Mix.
    let h = h ^ (h >> 33);
    let h = h.wrapping_mul(0xFF51AFD7ED558CCD);
    let h = h ^ (h >> 33);
    let h = h.wrapping_mul(0xC4CEB9FE1A85EC53);
    let h = h ^ (h >> 33);
    // Top 24 bits → [0, 1).
    ((h >> 40) as f32) / (1u32 << 24) as f32
}

/// Read a single block at world coordinates from the loaded chunk map.
/// Returns `None` if the chunk containing that position isn't loaded
/// (treat unloaded space as "unknown" — most callers either skip the
/// edit or treat it as a hard boundary).
///
/// Used by tree growth/discovery passes that need to look at neighbors
/// that may be in adjacent chunks. Roughly equivalent to
/// `MultiChunkSource::block_at` but avoids constructing a source object
/// for one-off lookups.
#[inline]
fn read_block_world(
    chunks: &dashmap::DashMap<crate::world::chunk::ChunkCoord, std::sync::Arc<crate::world::chunk::Chunk>>,
    wx: i32, wy: i32, wz: i32,
) -> Option<crate::world::blocks::BlockId> {
    use crate::constants::CHUNK_SIZE;
    use crate::world::chunk::{Chunk, ChunkCoord};
    let cx = wx.div_euclid(CHUNK_SIZE);
    let cy = wy.div_euclid(CHUNK_SIZE);
    let cz = wz.div_euclid(CHUNK_SIZE);
    let lx = wx.rem_euclid(CHUNK_SIZE);
    let ly = wy.rem_euclid(CHUNK_SIZE);
    let lz = wz.rem_euclid(CHUNK_SIZE);
    let chunk = chunks.get(&ChunkCoord::new(cx, cy, cz))?;
    let idx = Chunk::block_index(lx, ly, lz);
    Some(chunk.value().blocks[idx])
}

/// Read moisture at a chunk-local position, preferring a pre-built
/// snapshot when available and falling back to a per-block field
/// reader otherwise.
///
/// The hot production path takes the snapshot branch — one cache-hot
/// array index. The fallback exists for test contexts that wire up
/// only `field_reader` (no `chunk_field_reader`).
///
/// Defaults to 0.5 (the "neutral, neither dry nor wet" assumption)
/// when no source is available; this matches the prior inline
/// `unwrap_or(0.5)` behavior at every call site.
#[inline]
fn moisture_at(
    snapshot: Option<&crate::world::fields::ChunkFieldSnapshot>,
    field_reader: Option<&dyn Fn(&str, i32, i32, i32) -> Option<f32>>,
    lx: i32, ly: i32, lz: i32,
    wx: i32, wy: i32, wz: i32,
) -> f32 {
    if let Some(snap) = snapshot {
        snap.sample_at_block(lx, ly, lz)
    } else {
        field_reader
            .and_then(|r| r("moisture", wx, wy, wz))
            .unwrap_or(0.5)
    }
}

/// Advance the plant's lifecycle by `n` simulated ticks at once. Used
/// by catch-up when a chunk re-enters tick range after being away.
///
/// **Time-homogeneity property** (verified by tests): for any starting
/// state and constant `moisture`, calling `advance_n_ticks(state, m, N)`
/// produces the same final state and stage as `N` consecutive
/// `advance_one_tick(state, m)` calls. This is the property that lets
/// catch-up be applied in one call without divergence from "ticked
/// normally."
///
/// **Caveat — moisture history**: catch-up gets a single moisture
/// sample. If moisture varied during the catch-up window, the
/// approximation isn't perfect. A Mature plant that was wet for half
/// the window then dried might enter Drying earlier than this catch-up
/// computes. The acceptable approximation: use the current sample;
/// wet plants won't dry, dry plants will dry. Future improvement could
/// integrate moisture from a stored time-series, but that's bigger.
///
/// **Drying recovery** (moisture > 0.7): if catch-up starts in Drying
/// with high moisture, the plant recovers to Mature on the first
/// simulated tick (matching `advance_one_tick`'s Drying-recovery
/// branch). Then the catch-up loop continues with the remaining ticks
/// in the Mature stage.
pub fn advance_n_ticks(state: &mut BlockInstanceState, moisture: f32, n: u32) -> Stage {
    let mut remaining = n;
    let mut stage = Stage::from_flags(state.flags);

    // Iteration cap: each iteration either consumes >=1 tick OR does a
    // zero-cost transition (e.g. Drying recovery). We bound iterations
    // at 2 × max_stages to ensure we don't infinite-loop in any
    // pathological case (Withered + age past threshold should just sit).
    // 5 stages × 2 = 10 is a safe loose bound.
    let mut iterations = 0u32;
    let max_iterations = remaining.saturating_add(16);

    while remaining > 0 && iterations < max_iterations {
        iterations += 1;

        // Special case: Drying with moisture > 0.7 → instant recovery
        // to Mature, BUT only if age hasn't already crossed the death
        // threshold. The original `advance_one_tick` advances age first
        // (by per-tick increment), then checks transitions in match
        // order: death-via-age comes before recovery, so a wet plant
        // at death age withers rather than recovering. Preserve that.
        if stage == Stage::Drying && moisture > 0.7 {
            // Check what the age would be after a 1-tick advance.
            let per_tick = age_increment_per_tick(moisture);
            let projected_age = state.age_ticks.saturating_add(per_tick);
            if projected_age >= STAGE_DRYING_DURATION_TICKS {
                // Death wins over recovery (matches original match order).
                state.age_ticks = 0;
                stage = Stage::Withered;
                state.flags = stage.into_flags(state.flags);
                remaining -= 1;
                continue;
            }
            // Recovery: stage→Mature, age→0, consume 1 tick.
            state.age_ticks = 0;
            stage = Stage::Mature;
            state.flags = stage.into_flags(state.flags);
            remaining -= 1;
            continue;
        }

        // Withered is terminal — accumulate age until cap (death age
        // detection lives in `should_die`, not here). No further stage
        // transition. We can fast-forward all remaining ticks at once.
        if stage == Stage::Withered {
            let per_tick = age_increment_per_tick(moisture) as u32;
            let total_age = (per_tick.saturating_mul(remaining))
                .min(u8::MAX as u32) as u8;
            state.age(total_age);
            break;
        }

        // Mature with high moisture stays Mature; no transition. Just
        // accumulate age (capped at u8::MAX). Same fast-forward as
        // Withered, but stage stays Mature.
        if stage == Stage::Mature && moisture >= DRY_MOISTURE_THRESHOLD {
            let per_tick = age_increment_per_tick(moisture) as u32;
            let total_age = (per_tick.saturating_mul(remaining))
                .min(u8::MAX as u32) as u8;
            state.age(total_age);
            break;
        }

        // General case: there's a stage transition coming up. Compute
        // how many ticks until we reach it; advance min(remaining, that)
        // ticks; do the transition if reached.
        let until = match ticks_to_transition(stage, state.age_ticks, moisture) {
            Some(t) => t,
            None => {
                // No transition possible from this stage in this moisture
                // — accumulate all remaining ticks as age, then break.
                let per_tick = age_increment_per_tick(moisture) as u32;
                let total_age = (per_tick.saturating_mul(remaining))
                    .min(u8::MAX as u32) as u8;
                state.age(total_age);
                break;
            }
        };

        let consume = (until as u32).max(1).min(remaining);
        let per_tick = age_increment_per_tick(moisture);
        // Advance age by consume × per_tick ticks worth.
        let age_add = (consume as u32 * per_tick as u32).min(u8::MAX as u32) as u8;
        state.age(age_add);

        // Check whether we hit the threshold and transition.
        let threshold_reached = state.age_ticks >= match stage {
            Stage::Seedling => STAGE_SEEDLING_DURATION_TICKS,
            Stage::Growing  => STAGE_GROWING_DURATION_TICKS,
            Stage::Mature   => STAGE_MATURE_DURATION_TICKS,
            Stage::Drying   => STAGE_DRYING_DURATION_TICKS,
            Stage::Withered => u8::MAX, // unreachable per the early-return above
        };

        if threshold_reached {
            state.age_ticks = 0;
            stage = match stage {
                Stage::Seedling => Stage::Growing,
                Stage::Growing  => Stage::Mature,
                Stage::Mature   => Stage::Drying,
                Stage::Drying   => Stage::Withered,
                Stage::Withered => Stage::Withered,
            };
            state.flags = stage.into_flags(state.flags);
        }

        remaining = remaining.saturating_sub(consume);
    }

    state.flags = stage.into_flags(state.flags);
    stage
}

/// Tick threshold beyond which a withered plant should die (block →
/// Air). Triggers cleanup of dead plants from the world.
pub const WITHERED_DEATH_AGE_TICKS: u8 = 60;

/// Has this withered plant been dead long enough to be removed?
pub fn should_die(state: &BlockInstanceState) -> bool {
    Stage::from_flags(state.flags) == Stage::Withered
        && state.age_ticks >= WITHERED_DEATH_AGE_TICKS
}

/// Build the plant lifecycle tick handler. Iterates the per-chunk plant
/// inverted index, advances each plant's lifecycle state, and queues
/// state changes (or block-id edits for dying plants) on the tick context.
///
/// Reads moisture from the field framework (if present) for each plant
/// position. Plants in low-moisture environments dry and wither; plants
/// in wet environments stay healthy indefinitely.
///
/// **Plant index iteration is the "group ticking" pattern:** we don't
/// scan all 32k cells of every chunk — we visit only the indexed plant
/// blocks. With ~200 plants per surface chunk and ~20 visible chunks,
/// that's 4000 lookups per tick, easily under 1ms even with the field
/// reads.
///
/// **Tick culling** (skip far-away chunks) is a future addition — would
/// hide the cost of distant simulation by lazy-catchup when the player
/// approaches.
pub fn lifecycle_handler() -> std::sync::Arc<
    dyn Fn(&mut crate::world::ticks::TickContext) + Send + Sync
> {
    use crate::world::blocks::BlockId;
    use crate::world::chunk::Chunk;

    std::sync::Arc::new(move |ctx: &mut crate::world::ticks::TickContext| {
        use crate::world::scheduler::{band_for_chunk, should_fire_in_band, ChunkTickBand};

        let schedule = ctx.schedule;
        let player = ctx.player_chunk;
        let tick_id = ctx.tick_id;
        let tracker = ctx.fire_tracker;
        const HANDLER_NAME: &str = "plant_lifecycle";
        // r282: stagger Simulation-band firing per chunk so the band's work
        // spreads across the divisor window instead of bunching on every Nth
        // tick (the periodic plant_lifecycle spike). Same per-chunk cadence +
        // catch-up, just de-synchronised. UNENDLICH_PLANT_STAGGER=0 disables.
        let stagger = std::env::var("UNENDLICH_PLANT_STAGGER")
            .map(|v| v != "0").unwrap_or(true);
        // r295: window (frames) over which to spread first-encounter POI builds so
        // a freshly-streamed batch of chunks doesn't all full-scan on one frame.
        // 0/1 = disabled (build immediately). Default 8.
        let poi_build_spread: u64 = std::env::var("UNENDLICH_PLANT_BUILD_SPREAD")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(8);

        // Sorted iteration for rollback determinism — same input state
        // must produce the same edit order across runs and machines.
        // DashMap's native iter would silently break replay.
        for coord in ctx.sorted_chunk_coords() {
            // Distance-based band classification. Three modes:
            //   - With scheduler + player: full band logic. Render fires
            //     every tick; Simulation fires once per N ticks (catch-up
            //     applied via tracker); CatchUp doesn't fire.
            //   - Without scheduler (test contexts): legacy radius cull.
            let (should_run, default_n) = match (schedule.as_ref(), player) {
                (Some(params), Some(p)) => {
                    let band = band_for_chunk(coord, p, params);
                    let fire = if stagger {
                        crate::world::scheduler::should_fire_in_band_staggered(band, coord, tick_id, params)
                    } else {
                        should_fire_in_band(band, tick_id, params)
                    };
                    let n: u32 = match band {
                        ChunkTickBand::Render => 1,
                        ChunkTickBand::Simulation => params.simulation_divisor,
                        ChunkTickBand::CatchUp => 0,
                    };
                    (fire, n)
                }
                _ => {
                    const TICK_CULL_RADIUS: i32 = crate::constants::RENDER_DISTANCE + 1;
                    let in_range = ctx.chunk_in_tick_range(coord, TICK_CULL_RADIUS);
                    (in_range, 1u32)
                }
            };
            if !should_run { continue; }

            // Compute n: gap since last fire OR default if no tracker.
            // For first-fire (no tracker entry yet), use band default.
            let ticks_to_advance = match tracker {
                Some(t) => t.elapsed_or_default(HANDLER_NAME, coord, tick_id, default_n),
                None => default_n,
            };

            let Some(entry) = ctx.chunks.get(&coord) else { continue };
            let chunk = entry.value();
            let (origin_x, origin_y, origin_z) = coord.block_origin();

            // Fast path early-out: if the chunk has nothing for this
            // handler to act on, skip. The "nothing" definition has to
            // cover both plant blocks (grass, leaves, flowers — all in
            // `plant_indices`) AND wood/tree blocks (`tree_root_indices`
            // once discovered). For a fresh chunk we don't know about
            // wood yet, so we always run the POI pass below at least
            // once (it builds the cache, which then enables the fast
            // path on subsequent ticks).
            //
            // Three cases:
            //   - Has plants → run normally.
            //   - Has known tree roots → run normally.
            //   - Has built POI cache that found 0 trees AND no plants
            //     → skip (this chunk truly has nothing for us).
            //   - Otherwise (no cache yet) → run POI pass below to find
            //     out, then take the appropriate path.
            //
            // Why this is safe: every code path below either iterates
            // a known index OR consults the cache. A chunk with empty
            // plant_indices + empty tree_root_indices + a built-empty
            // POI cache has nothing any path can fire on.
            let has_plants = !chunk.plant_indices.is_empty();
            let has_known_trees = !chunk.tree_root_indices.is_empty();
            let cache_says_no_trees = chunk.poi_cache
                .as_ref()
                .map(|c| c.tree_pois.is_empty())
                .unwrap_or(false);
            if !has_plants && !has_known_trees && cache_says_no_trees {
                if let Some(t) = tracker {
                    t.mark_fired(HANDLER_NAME, coord, tick_id);
                }
                continue;
            }

            // Per-chunk moisture snapshot. Built ONCE here and sampled
            // cheaply per-plant below — replacing what was previously
            // dozens-to-hundreds of `field_at("moisture", ...)` calls
            // per chunk per tick (each paying string-hash + 2 DashMap
            // reads + an RwLock write acquire).
            //
            // For 2167 chunks × ~100 plants each × per-call overhead,
            // the old path was roughly 700K field_at calls per tick
            // adding up to most of tick_sim. The snapshot reduces this
            // to 2167 snapshots per tick (one per chunk), each a
            // 2KB memcpy after the lock acquire.
            //
            // None when chunk_field_reader isn't wired (test contexts);
            // the per-call sites below fall back to ctx.field_reader.
            let moisture_snapshot = ctx.chunk_field_reader
                .and_then(|reader| reader("moisture", coord));

            // ─── Pass 0a: POI cache refresh ────────────────────────────
            //
            // Walk the chunk's wood blocks (sparse via tree_root_indices
            // when discovered, full scan on first encounter) and build
            // a fresh ChunkPoiCache. The cache holds: per-tree anchor
            // candidate, cluster size, diameter, bottom-most member.
            // Subsequent passes consume the cache instead of re-walking.
            //
            // **Refresh policy** (v0.24.70): rebuild when the cache is
            // None OR `chunk.structural_dirty` is set. An idle chunk
            // with a built cache and no wood edits since the last
            // rebuild NEVER refreshes — paying the cost once per chunk
            // lifetime instead of every POI_REFRESH_INTERVAL_TICKS
            // ticks (the previous placeholder cadence).
            //
            // structural_dirty is set on wood transitions in
            // `Chunk::set()` and cleared when InstallPoi applies a
            // fresh cache (chunk_manager.rs). See `should_refresh_poi`
            // below for the explicit check.
            //
            // The build itself uses MultiChunkSource so trees crossing
            // chunk boundaries are walked fully. Members are filtered
            // to "anchor lives in THIS chunk" so a cluster spanning
            // two chunks doesn't appear in both caches — it appears
            // only in the cache of the chunk that owns its anchor.
            if should_refresh_poi(chunk.as_ref(), tick_id) {
                // r295: PHASE THE FIRST-ENCOUNTER BUILD. The first POI build of a
                // chunk does a full wood scan (build_poi_cache); when a batch of
                // fresh chunks streams in together they all hit this on the same
                // frame → the plant_lifecycle spike (operator M2: 16 chunks = 81ms,
                // worst 193ms). Spread the FIRST build (cache None) across a window
                // by stable per-chunk phase so a batch fans out over POI_BUILD_SPREAD
                // frames instead of bunching. A structural-dirty refresh (an edit)
                // is NOT delayed — those must apply promptly. Deferring a fresh
                // chunk's BUILD for a few frames is invisible: the tree passes below
                // consume a None cache and no-op (same as any not-yet-built chunk),
                // while plant aging still runs. UNENDLICH_PLANT_BUILD_SPREAD=0/1 off.
                let first_build = chunk.poi_cache.is_none();
                let build_now = if first_build && poi_build_spread > 1 {
                    tick_id.raw() % poi_build_spread
                        == crate::world::scheduler::chunk_phase(coord, poi_build_spread as u32)
                } else {
                    true
                };
                if build_now {
                    let result = build_poi_cache(
                        chunk.as_ref(), coord, ctx.chunks, tick_id,
                    );
                    ctx.enqueue_install_poi_cache(coord, result.cache);
                    for anchor in result.markers_to_set {
                        ctx.enqueue_tree_root_set(anchor[0], anchor[1], anchor[2]);
                    }
                }
                // else: not this chunk's build-phase — skip ONLY the build this
                // frame; the plant passes below still run (they consume the cache,
                // which stays None on a fresh chunk, so tree work no-ops while plant
                // aging proceeds normally). The build lands within poi_build_spread
                // frames, fanning a streamed batch out instead of bunching.
            }

            // ─── Pass 0b: tree-root stickiness ─────────────────────────
            //
            // For each tree-root marker in this chunk, verify it still
            // touches anchorage. If yes, clear unanchored flag if set.
            // If no but an adjacent wood block IS anchored, gate by
            // a slow probability and (if fired) walk the marker by 1
            // to that adjacent block. If no adjacent wood is anchored,
            // set unanchored flag.
            //
            // The "walk by 1, only every so often" rule produces the
            // curved-base tree aesthetic: as soil erodes from under a
            // tree, the marker walks one block laterally to the new
            // contact point, but the geometry above stays put. Over
            // time the tree appears to lean away from the eroded side.
            //
            // Walk gating: 0.05 per-tick probability per marker. At
            // default 0.5s tick rate, average 20 ticks (~10s) before
            // a walk fires. Marker stays sticky on its current spot
            // unless conditions stay bad for a while.
            //
            // For Phase A we DON'T act on the unanchored flag yet —
            // Phase B will tie it to bottom-up degrowth. Today setting
            // the flag is observable but produces no behavior change.
            for &marker_idx in chunk.tree_root_indices.iter() {
                let (mlx, mly, mlz) = Chunk::index_to_xyz(marker_idx);
                let mwx = origin_x + mlx;
                let mwy = origin_y + mly;
                let mwz = origin_z + mlz;

                // Sanity: the block at the marker should still be wood.
                // If it's not (race with concurrent edit, or marker
                // never got cleaned up), enqueue a clear and move on.
                let here = chunk.blocks[marker_idx as usize];
                if !matches!(here, BlockId::OakLog | BlockId::BirchLog) {
                    ctx.enqueue_tree_root_clear(mwx, mwy, mwz);
                    continue;
                }

                // Direct anchorage check: any of the 6 face-adjacent
                // blocks `is_tree_anchorage()`? If yes, this marker is
                // happily anchored; clear unanchored flag if set.
                let face_neighbors = [
                    (mwx - 1, mwy,     mwz),
                    (mwx + 1, mwy,     mwz),
                    (mwx,     mwy - 1, mwz),
                    (mwx,     mwy + 1, mwz),
                    (mwx,     mwy,     mwz - 1),
                    (mwx,     mwy,     mwz + 1),
                ];
                let directly_anchored = face_neighbors.iter().any(|&(nx, ny, nz)| {
                    read_block_world(ctx.chunks, nx, ny, nz)
                        .map(|b| b.is_tree_anchorage())
                        .unwrap_or(false)
                });
                if directly_anchored {
                    // Clear the unanchored flag if it was set.
                    if let Some(state) = chunk.instance_state.get(&marker_idx) {
                        if state.has_flag(BlockInstanceState::FLAG_TREE_UNANCHORED) {
                            ctx.enqueue_tree_root_unanchored(mwx, mwy, mwz, false);
                        }
                    }
                    continue;
                }

                // Not directly anchored. Look for a face-adjacent wood
                // block that IS anchored — a candidate to walk to.
                // Same species only (don't merge oak into birch).
                let same_species = here;
                let walk_candidate: Option<(i32, i32, i32)> = face_neighbors
                    .iter()
                    .copied()
                    .find(|&(nx, ny, nz)| {
                        if read_block_world(ctx.chunks, nx, ny, nz) != Some(same_species) {
                            return false;
                        }
                        // Does THAT wood block have anchorage?
                        let n_neighbors = [
                            (nx - 1, ny,     nz),
                            (nx + 1, ny,     nz),
                            (nx,     ny - 1, nz),
                            (nx,     ny + 1, nz),
                            (nx,     ny,     nz - 1),
                            (nx,     ny,     nz + 1),
                        ];
                        n_neighbors.iter().any(|&(nx2, ny2, nz2)| {
                            read_block_world(ctx.chunks, nx2, ny2, nz2)
                                .map(|b| b.is_tree_anchorage())
                                .unwrap_or(false)
                        })
                    });

                match walk_candidate {
                    Some((nx, ny, nz)) => {
                        // Slow walk gating: deterministic per-marker
                        // per-tick decision driven by hashed (marker_pos,
                        // tick_id). Threshold 0.05 → ~20 ticks average
                        // before a walk fires. Same input → same fire
                        // (replay determinism preserved).
                        let r = sticky_walk_random(mwx, mwy, mwz, tick_id);
                        if r < STICKY_WALK_PROBABILITY {
                            // Fire the walk: clear here, set there.
                            // Also clear unanchored on the OLD position
                            // explicitly (the clear-marker path already
                            // does this, but be defensive).
                            ctx.enqueue_tree_root_clear(mwx, mwy, mwz);
                            ctx.enqueue_tree_root_set(nx, ny, nz);
                        } else {
                            // Walk gated this tick; mark unanchored so
                            // Phase B knows the tree is in a bad spot.
                            ctx.enqueue_tree_root_unanchored(mwx, mwy, mwz, true);
                        }
                    }
                    None => {
                        // No anchored wood adjacent. Tree is fully
                        // disconnected from ground. Mark unanchored.
                        // Phase B will then drive bottom-up degrowth.
                        ctx.enqueue_tree_root_unanchored(mwx, mwy, mwz, true);
                    }
                }
            }

            // ─── Pass 1: noise-driven pressure sampling ────────────────
            //
            // Draw N MoistureStress + N Growth samples for this chunk
            // this tick. For each sample:
            //   - Find which (if any) plant is at the sampled position.
            //   - If it's an eligible plant for the dimension, evaluate
            //     the probability function at the local moisture, and
            //     fire the transition iff noise_value < p(moisture).
            //
            // Cost is O(N samples × 1 lookup each), independent of plant
            // count. Eligibility is per-stage: MoistureStress fires only
            // on Mature plants, Growth fires only on Seedling/Growing.
            //
            // **Catch-up handling**: if ticks_to_advance > 1, we'd ideally
            // draw samples for each of those ticks (using their distinct
            // tick_ids as the noise stream advances). For N > 1 we draw
            // samples for `min(ticks_to_advance, ANIM_TICKS)` ticks —
            // any more is wasted (the plant's already in the new state
            // after the first fire). This is an approximation; acceptable
            // because catch-up scenarios are visually invisible (player
            // wasn't there) and the population-level average still
            // matches the individual-tick behavior.
            //
            // We accumulate all sample-driven transitions, then in Pass 2
            // skip plants that already had a transition started this
            // tick (so the deterministic logic doesn't double-fire).
            use crate::world::pressure_sampler::{
                pressure_samples_iter, PressureDim, p_death, p_growth, p_tree_growth,
                p_tree_shrink,
                SAMPLES_PER_CHUNK_PER_TICK,
            };

            // Track which plant indices got a sample-driven transition
            // started this tick. The deterministic per-plant pass below
            // skips these to avoid double-firing.
            let mut sampler_started: std::collections::HashSet<u16> =
                std::collections::HashSet::new();

            // How many ticks of sampling to run. For a Render-band chunk
            // with no catch-up, this is 1. For a Simulation-band chunk
            // catching up its 8-tick band, this is 8. For a long catch-up
            // (re-entered from CatchUp band), capped at ANIM_TICKS so
            // we don't waste cycles — even in 100-tick catch-ups, the
            // plant only needs ONE sample-fire to transition.
            //
            // We use ticks_to_advance directly because it already
            // captures the band cadence + catch-up gap.
            let sample_ticks = ticks_to_advance
                .min(crate::world::chunk::TRANSITION_ANIM_TICKS as u32 + 1);

            for tick_offset in 0..sample_ticks {
                let sample_tick = crate::world::ticks::TickId(
                    tick_id.raw().saturating_sub(
                        (sample_ticks - 1 - tick_offset) as u64));

                // Process MoistureStress samples — fire Mature→Drying.
                for s in pressure_samples_iter(
                    coord, sample_tick, PressureDim::MoistureStress,
                    SAMPLES_PER_CHUNK_PER_TICK,
                ) {
                    let plant_idx = Chunk::block_index(s.local_x, s.local_y, s.local_z) as u16;
                    if !chunk.plant_indices.contains(&plant_idx) {
                        // Sample landed on non-plant — most common case.
                        // Plant indices is a sparse set, so most samples
                        // miss. That's fine: cost is bounded by samples,
                        // not population.
                        continue;
                    }
                    let block_id = chunk.blocks[plant_idx as usize];
                    if !block_id.has_subvoxels() { continue; }
                    let inst = chunk.instance_state.get(&plant_idx).copied()
                        .unwrap_or_default();
                    let stage = Stage::from_flags(inst.flags);
                    if stage != Stage::Mature { continue; }
                    // Skip if already in animation (don't double-fire).
                    if chunk.transitions.contains_key(&plant_idx) { continue; }
                    // Skip if already sampler-fired this tick.
                    if sampler_started.contains(&plant_idx) { continue; }

                    let wx = origin_x + s.local_x;
                    let wy = origin_y + s.local_y;
                    let wz = origin_z + s.local_z;
                    let moisture = moisture_at(
                        moisture_snapshot.as_ref(), ctx.field_reader,
                        s.local_x, s.local_y, s.local_z, wx, wy, wz,
                    );

                    let p = p_death(moisture);
                    if s.noise_value < p {
                        // Fire the transition. For Render-band cadence
                        // (sample_ticks == 1, ticks_to_advance == 1),
                        // start the visual animation. For longer catch-up,
                        // commit directly — player wasn't there to see.
                        if ticks_to_advance <= crate::world::chunk::TRANSITION_ANIM_TICKS as u32 {
                            ctx.enqueue_transition_start(wx, wy, wz,
                                crate::world::chunk::BlockTransition {
                                    from_stage: Stage::Mature as u8,
                                    to_stage: Stage::Drying as u8,
                                    started_tick: tick_id.raw(),
                                });
                        } else {
                            // Catch-up commit: write Drying directly.
                            let mut s = inst;
                            s.flags = Stage::Drying.into_flags(s.flags);
                            s.age_ticks = 0;
                            ctx.enqueue_state(wx, wy, wz, s);
                        }
                        sampler_started.insert(plant_idx);
                    }
                }

                // Process Growth samples — promote existing plants OR
                // spawn new ones into eligible empty cells.
                //
                // The same noise pattern that picks growth locations
                // ALSO picks spawn locations. A wet biome with empty
                // patches sees growth samples land on those patches
                // and fire spawning, gradually filling them in. No
                // separate spawn-rate tuning — emerges naturally from
                // the unified sample budget.
                for s in pressure_samples_iter(
                    coord, sample_tick, PressureDim::Growth,
                    SAMPLES_PER_CHUNK_PER_TICK,
                ) {
                    let plant_idx = Chunk::block_index(s.local_x, s.local_y, s.local_z) as u16;
                    let block_at_sample = chunk.blocks[plant_idx as usize];

                    let wx = origin_x + s.local_x;
                    let wy = origin_y + s.local_y;
                    let wz = origin_z + s.local_z;
                    let moisture = moisture_at(
                        moisture_snapshot.as_ref(), ctx.field_reader,
                        s.local_x, s.local_y, s.local_z, wx, wy, wz,
                    );
                    let p = p_growth(moisture);

                    // Branch by what's at the sample position.
                    if block_at_sample.has_subvoxels()
                        && chunk.plant_indices.contains(&plant_idx)
                    {
                        // ── Existing-plant promotion path ──────────────
                        let inst = chunk.instance_state.get(&plant_idx).copied()
                            .unwrap_or_default();
                        let stage = Stage::from_flags(inst.flags);
                        if !matches!(stage, Stage::Seedling | Stage::Growing) {
                            continue;
                        }
                        if chunk.transitions.contains_key(&plant_idx) { continue; }
                        if sampler_started.contains(&plant_idx) { continue; }

                        if s.noise_value < p {
                            // Stage promotion: skip animation for now —
                            // Seedling/Growing → next stage doesn't have
                            // a strong visual difference yet.
                            let new_stage = match stage {
                                Stage::Seedling => Stage::Growing,
                                Stage::Growing  => Stage::Mature,
                                _ => stage, // unreachable per filter above
                            };
                            let mut new_state = inst;
                            new_state.flags = new_stage.into_flags(new_state.flags);
                            new_state.age_ticks = 0;
                            ctx.enqueue_state(wx, wy, wz, new_state);
                            sampler_started.insert(plant_idx);
                        }
                    } else if matches!(block_at_sample, BlockId::Air) {
                        // ── Spawn-into-empty-cell path ─────────────────
                        //
                        // Eligibility: cell is Air, the cell directly
                        // below is a plant-bearing surface (Grass), and
                        // we're not at y=0 of the chunk (avoiding the
                        // cross-chunk substrate read for now — most
                        // surfaces aren't at chunk boundaries, so we
                        // catch >99% of spawn opportunities anyway).
                        //
                        // Sample-driven spawning means in a wet biome
                        // with empty patches, growth samples land in
                        // those patches and fire spawning, gradually
                        // filling them in. No separate spawn-rate
                        // tuning — emerges from the unified Growth
                        // sample budget.
                        if s.local_y == 0 {
                            // Skip y=0 to avoid cross-chunk substrate
                            // read. Future: read the chunk below.
                            continue;
                        }
                        let below_idx = Chunk::block_index(
                            s.local_x, s.local_y - 1, s.local_z) as u16;
                        let below_block = chunk.blocks[below_idx as usize];
                        // Substrate check: only spawn on Grass for now.
                        // Future: per-biome eligible substrates (Moss
                        // for tundra, etc.).
                        if below_block != BlockId::Grass {
                            continue;
                        }
                        // Probability gate.
                        if s.noise_value >= p {
                            continue;
                        }
                        // Spawn! Default is TallGrass at Seedling stage
                        // (default state). Future: pick from biome's
                        // plant set; for now everything's TallGrass.
                        // The chunk's `plant_indices` updates
                        // automatically via the COW set() in apply
                        // phase. Default state = Seedling, age 0 = no
                        // explicit instance_state needed.
                        ctx.enqueue_edit(wx, wy, wz, BlockId::TallGrass);
                        sampler_started.insert(plant_idx);
                    }
                    // Else: sample landed on solid non-plant (stone,
                    // dirt, etc.) — nothing to promote, can't spawn,
                    // skip.
                }

                // Process TreeGrowth samples — extend trees by replacing
                // a leaf adjacent to wood with new wood, and adding new
                // leaves around the new wood block.
                //
                // Eligibility:
                //   - Sample lands on an OakLeaves or BirchLeaves block.
                //   - At least one of the 6 face-adjacent cells is a
                //     matching wood block (OakLog or BirchLog).
                //   - The tree cluster (flood-filled wood-only) has
                //     diameter < TREE_DIAMETER_CAP.
                //
                // Process:
                //   - Replace the leaf with the matching wood type.
                //   - Add new leaves at the cell above and the four
                //     horizontally-adjacent cells if they're currently
                //     Air. Don't overwrite anything else.
                //
                // Cost-bounded: the cluster diameter check runs flood-
                // fill bounded by TREE_CLUSTER_MAX. For typical trees
                // (50-200 wood blocks) this is fast (<1ms). For
                // pathologically merged tree forests it falls back to
                // anchored — fine, that means we don't grow there.
                //
                // **Per-chunk per-tick growth budget**: at most
                // TREE_GROWTH_EVENTS_PER_CHUNK_PER_TICK events fire
                // here. Without this cap, every sample that lands on
                // an eligible leaf can fire growth, and a forested
                // chunk can grow ~SAMPLES_PER_CHUNK_PER_TICK = 8
                // blocks per tick — making trees fan out into solid
                // blobs instead of stabilizing. The cap turns growth
                // into a slow organic creep visible at human time scale.
                let mut growth_events_fired: usize = 0;
                for s in pressure_samples_iter(
                    coord, sample_tick, PressureDim::TreeGrowth,
                    SAMPLES_PER_CHUNK_PER_TICK,
                ) {
                    if growth_events_fired >= TREE_GROWTH_EVENTS_PER_CHUNK_PER_TICK {
                        break;
                    }
                    let plant_idx = Chunk::block_index(s.local_x, s.local_y, s.local_z) as u16;
                    let block_at_sample = chunk.blocks[plant_idx as usize];

                    // Filter: only leaves are eligible for tree growth.
                    let leaf_kind = match block_at_sample {
                        BlockId::OakLeaves   => Some(BlockId::OakLog),
                        BlockId::BirchLeaves => Some(BlockId::BirchLog),
                        _ => None,
                    };
                    let Some(matching_log) = leaf_kind else { continue; };

                    // Find an adjacent matching wood block (6-direction).
                    // World-coord lookups via the world reader so trees
                    // crossing chunk boundaries get their wood-neighbor
                    // detected correctly. The previous SingleChunkSource
                    // path bailed on cross-chunk neighbors, which silently
                    // froze growth on any tree whose trunk happened to
                    // straddle a chunk seam.
                    let wx_leaf = origin_x + s.local_x;
                    let wy_leaf = origin_y + s.local_y;
                    let wz_leaf = origin_z + s.local_z;
                    let world_neighbors: [(i32, i32, i32); 6] = [
                        (wx_leaf - 1, wy_leaf, wz_leaf),
                        (wx_leaf + 1, wy_leaf, wz_leaf),
                        (wx_leaf, wy_leaf - 1, wz_leaf),
                        (wx_leaf, wy_leaf + 1, wz_leaf),
                        (wx_leaf, wy_leaf, wz_leaf - 1),
                        (wx_leaf, wy_leaf, wz_leaf + 1),
                    ];
                    let mut adjacent_log_world: Option<(i32, i32, i32)> = None;
                    for &(nwx, nwy, nwz) in &world_neighbors {
                        if read_block_world(ctx.chunks, nwx, nwy, nwz)
                            == Some(matching_log)
                        {
                            adjacent_log_world = Some((nwx, nwy, nwz));
                            break;
                        }
                    }
                    let Some((logwx, logwy, logwz)) = adjacent_log_world else {
                        continue; // No adjacent wood — skip.
                    };

                    // Probability gate. Sample local moisture at the
                    // leaf's position; trees grow more in wet conditions.
                    let wx = wx_leaf;
                    let wy = wy_leaf;
                    let wz = wz_leaf;
                    let moisture = moisture_at(
                        moisture_snapshot.as_ref(), ctx.field_reader,
                        s.local_x, s.local_y, s.local_z, wx, wy, wz,
                    );
                    if s.noise_value >= p_tree_growth(moisture) {
                        continue; // Probability didn't fire.
                    }

                    // Diameter cap check. Compute the tree's current
                    // cluster (wood-only flood-fill) starting from the
                    // adjacent log via a multi-chunk source so trees
                    // spanning chunk boundaries are measured fully.
                    use crate::world::cluster::{
                        cluster_at, MultiChunkSource, TreeRule,
                    };
                    let chunks_ref = ctx.chunks;
                    let source = MultiChunkSource {
                        get_chunk: |c| chunks_ref.get(&c).map(|r| r.value().clone()),
                    };
                    let cluster = cluster_at(logwx, logwy, logwz, &TreeRule, &source);
                    if cluster.id.is_anchored() {
                        // Cluster was too large for the rule's cap and
                        // got returned as the anchored sentinel — treat
                        // as full, don't grow further.
                        continue;
                    }
                    if cluster.diameter() >= TREE_DIAMETER_CAP {
                        continue;
                    }

                    // Cluster-size soft gate. As the tree approaches
                    // TREE_CLUSTER_SOFT_CAP wood blocks, growth
                    // probability tapers linearly toward zero. This
                    // gives natural-looking terminal size without a
                    // hard wall — small trees grow freely, big trees
                    // saturate gradually. Without this, growth is a
                    // pure runaway feedback loop until TREE_DIAMETER_CAP.
                    let size = cluster.members.len();
                    let size_factor = if size >= TREE_CLUSTER_SOFT_CAP {
                        0.0
                    } else {
                        1.0 - (size as f32 / TREE_CLUSTER_SOFT_CAP as f32)
                    };
                    // Re-test against the size-tapered probability.
                    // s.noise_value already passed the base gate
                    // (>= p_tree_growth(moisture)) above; we only
                    // need to tighten it further as the tree fills out.
                    if s.noise_value >= p_tree_growth(moisture) * size_factor {
                        continue;
                    }

                    // All checks passed — fire the growth event.
                    // 1. Replace the leaf with matching wood.
                    ctx.enqueue_edit(wx, wy, wz, matching_log);
                    sampler_started.insert(plant_idx);
                    growth_events_fired += 1;

                    // 2. Add new leaves at the air cells above and
                    //    horizontally around the new wood. World-coord
                    //    lookups + edits — `enqueue_edit` already routes
                    //    cross-chunk edits to the right chunk in the
                    //    apply phase, so we don't need any per-chunk
                    //    bounds check here.
                    let new_leaf = match matching_log {
                        BlockId::OakLog => BlockId::OakLeaves,
                        BlockId::BirchLog => BlockId::BirchLeaves,
                        _ => continue, // unreachable per filter above
                    };
                    let leaf_world_targets: [(i32, i32, i32); 5] = [
                        (wx,     wy + 1, wz),     // above
                        (wx - 1, wy,     wz),
                        (wx + 1, wy,     wz),
                        (wx,     wy,     wz - 1),
                        (wx,     wy,     wz + 1),
                    ];
                    for &(lwx, lwy, lwz) in &leaf_world_targets {
                        if read_block_world(ctx.chunks, lwx, lwy, lwz)
                            == Some(BlockId::Air)
                        {
                            ctx.enqueue_edit(lwx, lwy, lwz, new_leaf);
                        }
                        // If the cell isn't air (leaf, wood, anything),
                        // leave it alone — don't double up. If the chunk
                        // isn't loaded (`None`), also skip — we'll
                        // grow into it next time it's loaded.
                    }
                }

                // Process TreeShrink samples — retract trees by replacing
                // a wood block at the tree's "leaf edge" with leaf or air.
                //
                // Eligibility:
                //   - Sample lands on an OakLog or BirchLog block.
                //   - At least one of the 6 face-adjacent cells is air
                //     or matching leaves (i.e. the wood is at the tree
                //     surface — interior trunk wood with all-wood
                //     neighbors is protected).
                //
                // Process:
                //   - If any face-adjacent cell is a matching leaf,
                //     replace the wood with a matching leaf (the wood
                //     "demotes" to leaf as the branch retracts).
                //   - Else (all non-leaf neighbors include air), replace
                //     with air (the tip just disappears).
                //
                // Per-chunk per-tick budget: same as growth. At neutral
                // moisture (0.4), growth and shrink fire at equal rates
                // and the tree's edge breathes. In wet conditions
                // growth dominates. In dry conditions shrink dominates.
                let mut shrink_events_fired: usize = 0;
                for s in pressure_samples_iter(
                    coord, sample_tick, PressureDim::TreeShrink,
                    SAMPLES_PER_CHUNK_PER_TICK,
                ) {
                    if shrink_events_fired >= TREE_SHRINK_EVENTS_PER_CHUNK_PER_TICK {
                        break;
                    }
                    let plant_idx = Chunk::block_index(s.local_x, s.local_y, s.local_z) as u16;
                    let block_at_sample = chunk.blocks[plant_idx as usize];

                    // Filter: only wood is eligible for tree shrink.
                    let matching_leaf = match block_at_sample {
                        BlockId::OakLog   => BlockId::OakLeaves,
                        BlockId::BirchLog => BlockId::BirchLeaves,
                        _ => continue,
                    };

                    // Find what kind of neighbors this wood has. We
                    // need at least one air-or-leaf neighbor for the
                    // wood to be at the tree's "leaf edge". Interior
                    // trunk wood (6 wood neighbors) is protected.
                    let wx = origin_x + s.local_x;
                    let wy = origin_y + s.local_y;
                    let wz = origin_z + s.local_z;
                    let world_neighbors: [(i32, i32, i32); 6] = [
                        (wx - 1, wy, wz),
                        (wx + 1, wy, wz),
                        (wx, wy - 1, wz),
                        (wx, wy + 1, wz),
                        (wx, wy, wz - 1),
                        (wx, wy, wz + 1),
                    ];
                    let mut has_leaf_neighbor = false;
                    let mut has_air_neighbor = false;
                    for &(nwx, nwy, nwz) in &world_neighbors {
                        match read_block_world(ctx.chunks, nwx, nwy, nwz) {
                            Some(BlockId::OakLeaves) | Some(BlockId::BirchLeaves) => {
                                has_leaf_neighbor = true;
                            }
                            Some(BlockId::Air) => {
                                has_air_neighbor = true;
                            }
                            _ => {}
                        }
                    }
                    if !has_leaf_neighbor && !has_air_neighbor {
                        // Interior wood — protected from shrink.
                        continue;
                    }

                    // Probability gate: mirror of tree-growth.
                    // High when dry, low when wet.
                    let moisture = moisture_at(
                        moisture_snapshot.as_ref(), ctx.field_reader,
                        s.local_x, s.local_y, s.local_z, wx, wy, wz,
                    );
                    if s.noise_value >= p_tree_shrink(moisture) {
                        continue; // Probability didn't fire.
                    }

                    // Don't shrink the tree's anchor (root marker). If
                    // we shrunk roots, the cluster would lose its
                    // anchorage and the FLAG_TREE_UNANCHORED bit would
                    // start cascading — which is interesting behavior,
                    // but for now we keep roots intact and only let
                    // shrink act on the canopy.
                    let is_root = chunk.instance_state.get(&plant_idx)
                        .map(|s| s.has_flag(BlockInstanceState::FLAG_TREE_ROOT))
                        .unwrap_or(false);
                    if is_root {
                        continue;
                    }

                    // Fire the shrink event.
                    let replacement = if has_leaf_neighbor {
                        matching_leaf
                    } else {
                        BlockId::Air
                    };
                    ctx.enqueue_edit(wx, wy, wz, replacement);
                    sampler_started.insert(plant_idx);
                    shrink_events_fired += 1;
                }
            }

            // ─── Pass 2: deterministic per-plant lifecycle ─────────────
            //
            // This pass runs `advance_n_ticks` for each plant, handling:
            //   - Age accumulation
            //   - Seedling → Growing → Mature deterministic age-based
            //     promotion (acts as a fallback / floor when the noise
            //     sampler doesn't fire growth often enough)
            //   - Drying → Withered death (samples don't fire this; once
            //     a plant is Drying, age determines when it withers)
            //   - Drying → Mature recovery via moisture > 0.7
            //   - Withered terminal accumulation
            //
            // The Mature → Drying transition is suppressed: the noise
            // sampler is the only path. We achieve this by clamping the
            // moisture passed to `advance_n_ticks` for Mature plants to
            // be no lower than DRY_MOISTURE_THRESHOLD, so the function's
            // internal "is moisture dry enough to start drying" check
            // never fires.
            //
            // Plants that the sampler ALREADY started a transition for
            // are skipped entirely — no further state change this tick.

            // Group ticking: iterate the inverted index, not all cells.
            // plant_indices is a BTreeSet — already sorted, deterministic.
            for &plant_idx in &chunk.plant_indices {
                if sampler_started.contains(&plant_idx) {
                    // Sampler already wrote this plant this tick.
                    continue;
                }

                let block = chunk.blocks[plant_idx as usize];
                if !block.has_subvoxels() { continue; }

                let (lx, ly, lz) = Chunk::index_to_xyz(plant_idx);
                let wx = origin_x + lx;
                let wy = origin_y + ly;
                let wz = origin_z + lz;

                // Sample moisture at the plant's position (snapshot when
                // available; falls back to field_reader for tests).
                let moisture = moisture_at(
                    moisture_snapshot.as_ref(), ctx.field_reader,
                    lx, ly, lz, wx, wy, wz,
                );

                let old_state = chunk.instance_state.get(&plant_idx).copied()
                    .unwrap_or_default();
                let old_stage = Stage::from_flags(old_state.flags);

                // Suppress the deterministic Mature→Drying path. The
                // sampler is now the only way for a Mature plant to
                // start drying. We achieve suppression by passing a
                // moisture value clamped to be at least
                // DRY_MOISTURE_THRESHOLD when calling advance_n_ticks
                // for Mature plants. Other stages get the real moisture
                // (Drying still uses real moisture for the recovery
                // branch and the per-tick wet-bonus).
                let advance_moisture = if old_stage == Stage::Mature {
                    moisture.max(DRY_MOISTURE_THRESHOLD)
                } else {
                    moisture
                };

                // Fast path: a fully-aged Mature plant in a wet env stays
                // Mature forever and doesn't need a write. Skip the tick
                // entirely. Without this, even a stable forest would dirty
                // every chunk every 0.5s. With this, the cost drops to
                // O(plants-currently-changing-state).
                //
                // We also fast-path Mature plants that are already
                // age-saturated (255). They can't progress to Drying
                // without low moisture, and they can't accumulate more age.
                if old_stage == Stage::Mature
                    && moisture >= DRY_MOISTURE_THRESHOLD
                    && old_state.age_ticks >= STAGE_MATURE_DURATION_TICKS
                {
                    continue;
                }
                // Same for Withered plants that haven't reached death age:
                // they can't progress further without aging, and aging is
                // saturating — once it hits 255 they'll be killed below.
                if old_stage == Stage::Withered
                    && old_state.age_ticks >= WITHERED_DEATH_AGE_TICKS
                {
                    // Force death below.
                    if matches!(block, BlockId::Grass) {
                        let mut s = old_state;
                        s.flags = Stage::Mature.into_flags(s.flags);
                        s.age_ticks = 0;
                        if s != old_state {
                            ctx.enqueue_state(wx, wy, wz, s);
                        }
                    } else {
                        ctx.enqueue_edit(wx, wy, wz, BlockId::Air);
                    }
                    continue;
                }

                let mut state = old_state;
                // Animation handoff: if a transition is currently in
                // flight for this plant, two cases:
                //   - Past anim duration: commit the to_stage now,
                //     enqueue a Finish to clear the transition entry,
                //     and continue normal handler flow from the new
                //     committed stage.
                //   - Still animating: skip this plant — let the
                //     mesher continue interpolating, don't disturb the
                //     state mid-animation.
                if let Some(t) = chunk.transitions.get(&plant_idx).copied() {
                    let elapsed = tick_id.raw().saturating_sub(t.started_tick);
                    if elapsed >= crate::world::chunk::TRANSITION_ANIM_TICKS {
                        // Commit: stage = to_stage, age = 0 (matches
                        // what advance_one_tick does on stage transition).
                        let to_stage_typed = Stage::from_flags(t.to_stage);
                        state.flags = to_stage_typed.into_flags(state.flags);
                        state.age_ticks = 0;
                        ctx.enqueue_state(wx, wy, wz, state);
                        ctx.enqueue_transition_finish(wx, wy, wz);
                        // Don't run advance_n_ticks this tick — the
                        // commit IS this tick's advancement. Next tick
                        // will pick up from the committed state.
                        continue;
                    } else {
                        // Mid-animation. Don't touch the state.
                        continue;
                    }
                }

                // Catch-up-aware advance: run the right number of ticks
                // at once. Time-homogeneity tests (search for
                // time_homogeneity_*) prove that this matches running
                // ticks_to_advance individual ticks.
                // r190: PER-PLANT STAGGER — the harness measured 41,606
                // state writes/tick from this loop (each one clones the
                // chunk + bumps version + remeshes: the plant stutter).
                // Each plant now advances on 1/8 of ticks with an 8x
                // window — IDENTICAL evolution by the time-homogeneity
                // property above, 1/8 the writes and remeshes. Stage
                // commits delay <=8 ticks (~4s), within the existing
                // "catch-up commit for unseen ticks" philosophy.
                const PLANT_STAGGER: u64 = 8;
                if (tick_id.raw().wrapping_add(plant_idx as u64)) % PLANT_STAGGER != 0 {
                    continue;
                }
                let eff_ticks = ticks_to_advance.saturating_mul(PLANT_STAGGER as u32);
                // r191: MEMOIZED advance — advance_n_ticks is pure in
                // (state, moisture, n); whole fields of grass share the
                // exact same key (smooth moisture fields -> identical
                // f32 bits), so one computation serves the class. This
                // is the "graph of before->after tick states" cache;
                // keys are EXACT (no quantization -> zero semantic
                // change). Capped; cleared when oversized.
                thread_local! {
                    static ADV_MEMO: std::cell::RefCell<
                        std::collections::HashMap<(u8, u8, u32, u32), (crate::world::chunk::BlockInstanceState, Stage)>,
                    > = std::cell::RefCell::new(std::collections::HashMap::new());
                }
                let memo_key = (state.age_ticks, state.flags, advance_moisture.to_bits(), eff_ticks);
                let new_stage = ADV_MEMO.with(|m| {
                    let mut m = m.borrow_mut();
                    if let Some(&(s_after, stg)) = m.get(&memo_key) {
                        state = s_after;
                        stg
                    } else {
                        let stg = advance_n_ticks(&mut state, advance_moisture, eff_ticks);
                        if m.len() > 65536 { m.clear(); }
                        m.insert(memo_key, (state, stg));
                        stg
                    }
                });

                // Skip writes that don't actually change anything.
                if state == old_state {
                    continue;
                }

                // Animation gate: when the moisture-driven Mature→Drying
                // transition fires, we want a smooth visual fade from
                // green to straw-yellow over TRANSITION_ANIM_TICKS ticks
                // (~3 real seconds). Instead of immediately committing
                // the stage change, START a transition entry; the next
                // few handler firings will see it via the check above
                // and animate / commit at the right time.
                //
                // Only fires when:
                //   - old_stage was Mature
                //   - new_stage is Drying (the only deterministic-
                //     promote-with-animation case for now)
                //   - We're in render distance (band check would have
                //     already gated us; we're effectively here only
                //     for chunks that fired)
                //
                // For catch-up scenarios (large `ticks_to_advance`),
                // skip the animation and commit the new stage directly:
                // distant chunks shouldn't show animations the player
                // wasn't there to see. Threshold for "catch-up enough
                // to skip animation" = more than the animation duration
                // itself; if N <= ANIM_TICKS the player is essentially
                // at the chunk (Render band, every tick).
                let animate = old_stage == Stage::Mature
                    && new_stage == Stage::Drying
                    && ticks_to_advance <= crate::world::chunk::TRANSITION_ANIM_TICKS as u32;

                if animate {
                    let transition = crate::world::chunk::BlockTransition {
                        from_stage: Stage::Mature as u8,
                        to_stage: Stage::Drying as u8,
                        started_tick: tick_id.raw(),
                    };
                    ctx.enqueue_transition_start(wx, wy, wz, transition);
                    // DON'T enqueue a state change — the visible stage
                    // remains Mature in instance_state until the
                    // transition commits. The mesher reads the
                    // transition entry to interpolate.
                    continue;
                }

                if new_stage == Stage::Withered && should_die(&state) {
                    if matches!(block, BlockId::Grass) {
                        // Grass is a surface bearer — don't kill it; reset
                        // its lifecycle to Mature so it stays.
                        state.flags = Stage::Mature.into_flags(state.flags);
                        state.age_ticks = 0;
                        if state != old_state {
                            ctx.enqueue_state(wx, wy, wz, state);
                        }
                    } else {
                        ctx.enqueue_edit(wx, wy, wz, BlockId::Air);
                    }
                } else {
                    ctx.enqueue_state(wx, wy, wz, state);
                }
            }

            // Mark this chunk as fired for this handler. After the per-
            // block loop so the next fire computes its gap correctly.
            // Drop the entry borrow first to avoid double-borrow if some
            // tracker impl wants to look at chunks.
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

    #[test]
    fn fresh_state_is_seedling() {
        let s = BlockInstanceState::default();
        assert_eq!(Stage::from_flags(s.flags), Stage::Seedling);
    }

    #[test]
    fn dry_plants_progress_through_full_lifecycle() {
        let mut s = BlockInstanceState::default();
        // Dry environment: moisture = 0.2.
        let m = 0.2;
        // Seedling → Growing
        for _ in 0..STAGE_SEEDLING_DURATION_TICKS {
            advance_one_tick(&mut s, m);
        }
        assert_eq!(Stage::from_flags(s.flags), Stage::Growing);
        // Growing → Mature
        for _ in 0..STAGE_GROWING_DURATION_TICKS {
            advance_one_tick(&mut s, m);
        }
        assert_eq!(Stage::from_flags(s.flags), Stage::Mature);
        // Mature → Drying (because moisture < threshold)
        for _ in 0..STAGE_MATURE_DURATION_TICKS {
            advance_one_tick(&mut s, m);
        }
        assert_eq!(Stage::from_flags(s.flags), Stage::Drying);
        // Drying → Withered
        for _ in 0..STAGE_DRYING_DURATION_TICKS {
            advance_one_tick(&mut s, m);
        }
        assert_eq!(Stage::from_flags(s.flags), Stage::Withered);
    }

    #[test]
    fn wet_plants_dont_dry() {
        let mut s = BlockInstanceState::default();
        s.flags = Stage::Mature.into_flags(0);
        s.age_ticks = STAGE_MATURE_DURATION_TICKS;
        // Wet environment: moisture = 0.9.
        // Should NOT transition to Drying.
        let stage = advance_one_tick(&mut s, 0.9);
        assert_eq!(stage, Stage::Mature);
    }

    #[test]
    fn drying_plants_recover_when_rehydrated() {
        let mut s = BlockInstanceState::default();
        s.flags = Stage::Drying.into_flags(0);
        s.age_ticks = 5;
        // Sudden rain: moisture > 0.7. Plant recovers to Mature.
        let stage = advance_one_tick(&mut s, 0.85);
        assert_eq!(stage, Stage::Mature);
    }

    #[test]
    fn withered_plants_eventually_die() {
        let mut s = BlockInstanceState::default();
        s.flags = Stage::Withered.into_flags(0);
        s.age_ticks = WITHERED_DEATH_AGE_TICKS - 1;
        assert!(!should_die(&s));
        s.age_ticks = WITHERED_DEATH_AGE_TICKS;
        assert!(should_die(&s));
    }

    #[test]
    fn color_modulation_differs_by_stage() {
        let g = stage_color_modulation(Stage::Growing);
        let d = stage_color_modulation(Stage::Drying);
        let w = stage_color_modulation(Stage::Withered);
        // Drying should boost red, drop blue compared to growing.
        assert!(d[0] > g[0]);
        assert!(d[2] < g[2]);
        // Withered drops green further.
        assert!(w[1] < d[1]);
    }

    #[test]
    fn lifecycle_handler_ages_plants() {
        // Build a chunk with a tall-grass plant in it; run the handler
        // once; check that instance state was updated.
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::blocks::BlockId;
        use crate::world::ticks::{TickContext, DeferredEdit, DeferredStateChange};
        use dashmap::DashMap;

        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(5, 5, 5, BlockId::TallGrass);
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(chunk));

        // Wet environment so the plant doesn't dry.
        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "moisture" { Some(0.8) } else { None }
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

        let handler = lifecycle_handler();
        // r191: per-plant STAGGER — drive the full 8-tick window; the
        // single aging write lands on the plant's slot.
        for tk in 0..8u64 {
            ctx.tick_id = crate::world::ticks::TickId(tk);
            handler(&mut ctx);
            if !ctx.state_changes.is_empty() { break; }
        }

        // Should have produced one state change.
        assert_eq!(ctx.state_changes.len(), 1);
        assert_eq!(ctx.block_edits.len(), 0);
        let s = ctx.state_changes[0];
        assert_eq!((s.world_x, s.world_y, s.world_z), (5, 5, 5));
        // Age advanced (1 tick + 1 wet bonus = 2)
        assert!(s.new_state.age_ticks > 0);
        // Suppress unused warning on DeferredEdit.
        let _ = std::marker::PhantomData::<DeferredEdit>;
        let _ = std::marker::PhantomData::<DeferredStateChange>;
    }

    /// Wet-to-dry gradient: simulate a row of plants where moisture
    /// decays linearly with distance from a water source. After many
    /// ticks, plants closer to the water should be at earlier
    /// lifecycle stages (still Mature) than plants far from water
    /// (Drying or Withered).
    ///
    /// This is the "showcase" test of the lifecycle + moisture
    /// coupling. Watching it run validates that the simulation
    /// responds to environmental gradients in a believable way.
    #[test]
    fn moisture_gradient_produces_distance_dependent_lifecycle() {
        const ROW_LENGTH: usize = 10;
        const TICKS: usize = 200;

        // 10 plants in a row. Plant[0] is right next to the water
        // source (moisture 1.0); Plant[9] is far away (moisture 0.1).
        // Linear gradient between.
        let mut plants: Vec<BlockInstanceState> = (0..ROW_LENGTH)
            .map(|_| BlockInstanceState::default())
            .collect();

        let moisture_at = |i: usize| -> f32 {
            // 1.0 at i=0, 0.1 at i=9.
            1.0 - (i as f32 / (ROW_LENGTH - 1) as f32) * 0.9
        };

        for _tick in 0..TICKS {
            for (i, p) in plants.iter_mut().enumerate() {
                advance_one_tick(p, moisture_at(i));
            }
        }

        // After 200 ticks (~100s real-time at 0.5s tick), the dry end
        // should be visibly later in the lifecycle than the wet end.
        // Plant[0] (m=1.0): wet → progresses Seedling→Growing→Mature
        //                   and STAYS at Mature (saturating age).
        // Plant[9] (m=0.1): dry → progresses fully through cycle to
        //                   Drying or Withered.
        let wet_stage = Stage::from_flags(plants[0].flags);
        let dry_stage = Stage::from_flags(plants[9].flags);

        // Wet end should be Mature (or earlier if not enough ticks).
        assert_eq!(wet_stage, Stage::Mature,
            "wet end (m=1.0) should rest at Mature; got {:?}", wet_stage);

        // Dry end should be PAST Mature in the lifecycle progression.
        // Withered or Drying both qualify.
        assert!(matches!(dry_stage, Stage::Drying | Stage::Withered),
            "dry end (m=0.1) should reach Drying or Withered; got {:?}", dry_stage);

        // Sanity: monotonic-ish progression along the row. The dry-end
        // stage's "ordinal" should be >= the wet-end's. We map stages
        // to integers for comparison.
        fn stage_ordinal(s: Stage) -> u8 {
            match s {
                Stage::Seedling => 0,
                Stage::Growing  => 1,
                Stage::Mature   => 2,
                Stage::Drying   => 3,
                Stage::Withered => 4,
            }
        }
        let wet_ord = stage_ordinal(wet_stage);
        let dry_ord = stage_ordinal(dry_stage);
        assert!(dry_ord > wet_ord,
            "lifecycle should advance further at dry end than wet end (wet={}, dry={})",
            wet_ord, dry_ord);
    }

    /// Variant of the gradient test with a MOVING water source —
    /// moisture peak shifts along the row over time. Plants briefly
    /// in the wet zone should temporarily slow progression.
    ///
    /// Tests the rehydration recovery path: a Drying plant that gets
    /// rewatered transitions back to Mature.
    #[test]
    fn moving_water_source_rehydrates_drying_plants() {
        // Single plant. Spend half the time dry, then half wet.
        let mut s = BlockInstanceState::default();

        // Phase 1: dry → grow up and start drying. With STAGE durations
        // 10/30/60, we need at least 100 ticks dry to reach Drying.
        for _ in 0..120 {
            advance_one_tick(&mut s, 0.2);
        }
        // Should be in Drying or Withered at this point.
        let mid_stage = Stage::from_flags(s.flags);
        assert!(matches!(mid_stage, Stage::Drying | Stage::Withered),
            "after 120 dry ticks plant should be Drying/Withered; got {:?}", mid_stage);

        // Phase 2: water source moves over → plant rehydrates. If it
        // was Drying (not yet Withered), it should revert to Mature.
        if mid_stage == Stage::Drying {
            // Even a few wet ticks at high moisture (>0.7) trigger
            // recovery in advance_one_tick's recovery path.
            for _ in 0..5 {
                advance_one_tick(&mut s, 0.85);
            }
            let final_stage = Stage::from_flags(s.flags);
            assert_eq!(final_stage, Stage::Mature,
                "drying plant should recover to Mature when rewatered; got {:?}",
                final_stage);
        }
    }

    // ─── Time-homogeneity tests ────────────────────────────────────────
    //
    // For any starting state and constant moisture, advance_n_ticks(N)
    // must produce identical (state, stage) to N successive
    // advance_one_tick calls. This is the property that makes catch-up
    // safe: a chunk that was "away" for N ticks and now catches up
    // with one call ends up in the same place it would have if it
    // had ticked normally the whole time.
    //
    // Each test exercises a different region of the state machine.

    /// Helper: run advance_one_tick N times, return final state + stage.
    fn step_by_step(initial: BlockInstanceState, moisture: f32, n: u32)
        -> (BlockInstanceState, Stage)
    {
        let mut s = initial;
        let mut stage = Stage::from_flags(s.flags);
        for _ in 0..n {
            stage = advance_one_tick(&mut s, moisture);
        }
        (s, stage)
    }

    /// Helper: run advance_n_ticks(N) once, return final state + stage.
    fn all_at_once(initial: BlockInstanceState, moisture: f32, n: u32)
        -> (BlockInstanceState, Stage)
    {
        let mut s = initial;
        let stage = advance_n_ticks(&mut s, moisture, n);
        (s, stage)
    }

    fn assert_homogeneity(initial: BlockInstanceState, moisture: f32, n: u32, label: &str) {
        let (step_state, step_stage) = step_by_step(initial, moisture, n);
        let (atomic_state, atomic_stage) = all_at_once(initial, moisture, n);
        assert_eq!(step_stage, atomic_stage,
            "{}: stage diverged — step={:?} atomic={:?} after {} ticks moisture={}",
            label, step_stage, atomic_stage, n, moisture);
        assert_eq!(step_state.flags, atomic_state.flags,
            "{}: flags diverged — step={:08b} atomic={:08b}",
            label, step_state.flags, atomic_state.flags);
        assert_eq!(step_state.age_ticks, atomic_state.age_ticks,
            "{}: age diverged — step={} atomic={} after {} ticks moisture={}",
            label, step_state.age_ticks, atomic_state.age_ticks, n, moisture);
    }

    #[test]
    fn time_homogeneity_seedling_dry() {
        // Seedling, dry, advance through to Mature and beyond.
        let initial = BlockInstanceState::default();
        for n in [1u32, 5, 10, 11, 30, 41, 50, 100] {
            assert_homogeneity(initial, 0.3, n, &format!("seedling-dry-n={}", n));
        }
    }

    #[test]
    fn time_homogeneity_seedling_wet() {
        // Seedling, wet (per-tick = 2 + recovery rules), advance widely.
        let initial = BlockInstanceState::default();
        for n in [1u32, 3, 5, 6, 16, 21, 50, 100] {
            assert_homogeneity(initial, 0.8, n, &format!("seedling-wet-n={}", n));
        }
    }

    #[test]
    fn time_homogeneity_growing_marginal_moisture() {
        // Growing stage, moisture in the [0.4, 0.6] band — no wet bonus,
        // doesn't trigger drying. Should age linearly to Mature.
        let mut initial = BlockInstanceState::default();
        initial.flags = Stage::Growing.into_flags(initial.flags);
        initial.age_ticks = 5;
        for n in [1u32, 10, 20, 25, 26, 50, 100] {
            assert_homogeneity(initial, 0.5, n, &format!("growing-marginal-n={}", n));
        }
    }

    #[test]
    fn time_homogeneity_mature_dry_drying_to_withered() {
        // Mature with dry moisture: should drift through Drying into Withered.
        let mut initial = BlockInstanceState::default();
        initial.flags = Stage::Mature.into_flags(initial.flags);
        initial.age_ticks = 10;
        for n in [1u32, 50, 51, 81, 100, 200] {
            assert_homogeneity(initial, 0.2, n, &format!("mature-dry-n={}", n));
        }
    }

    #[test]
    fn time_homogeneity_mature_wet_stays_mature() {
        // Mature wet: no transition, just age accumulation (capped).
        let mut initial = BlockInstanceState::default();
        initial.flags = Stage::Mature.into_flags(initial.flags);
        initial.age_ticks = 30;
        for n in [1u32, 10, 100, 1000] {
            assert_homogeneity(initial, 0.8, n, &format!("mature-wet-n={}", n));
        }
    }

    #[test]
    fn time_homogeneity_drying_dry_to_withered() {
        // Drying with continued dry moisture: should hit Withered.
        let mut initial = BlockInstanceState::default();
        initial.flags = Stage::Drying.into_flags(initial.flags);
        initial.age_ticks = 5;
        for n in [1u32, 25, 26, 50, 100] {
            assert_homogeneity(initial, 0.2, n, &format!("drying-dry-n={}", n));
        }
    }

    #[test]
    fn time_homogeneity_drying_recovery_low_age() {
        // Drying with high moisture and low age — should recover next tick.
        let mut initial = BlockInstanceState::default();
        initial.flags = Stage::Drying.into_flags(initial.flags);
        initial.age_ticks = 5;
        for n in [1u32, 2, 10, 100] {
            assert_homogeneity(initial, 0.8, n, &format!("drying-recover-n={}", n));
        }
    }

    /// Edge case: Drying near death threshold + high moisture. The
    /// match order in `advance_one_tick` puts death-via-age before
    /// recovery, so a wet plant at the death age withers rather than
    /// recovering. `advance_n_ticks` must preserve this quirk.
    #[test]
    fn time_homogeneity_drying_at_death_threshold_with_wet() {
        let mut initial = BlockInstanceState::default();
        initial.flags = Stage::Drying.into_flags(initial.flags);
        // age_ticks = 28 + per_tick wet (2) = 30 → exactly at death threshold.
        initial.age_ticks = 28;
        for n in [1u32, 2, 10] {
            assert_homogeneity(initial, 0.8, n, &format!("drying-deathline-wet-n={}", n));
        }
    }

    #[test]
    fn time_homogeneity_withered_terminal() {
        // Withered should sit and accumulate age (capped at u8::MAX).
        let mut initial = BlockInstanceState::default();
        initial.flags = Stage::Withered.into_flags(initial.flags);
        initial.age_ticks = 0;
        for n in [1u32, 50, 60, 100, 1000, 100_000] {
            assert_homogeneity(initial, 0.5, n, &format!("withered-n={}", n));
        }
    }

    /// Catch-up scenario: a plant that was "away" for a long time
    /// (CatchUp band) re-enters tick range. Apply elapsed-tick gap
    /// in one call. Should match having been ticked all along under
    /// the same constant moisture.
    #[test]
    fn time_homogeneity_long_catchup_gap() {
        // Fresh seedling, dry conditions, catch up 500 ticks at once.
        // Expected: should reach Mature stage and stay there (since dry
        // takes 60 ticks to start drying, then 30 to wither — total
        // ~110 ticks under dry from Seedling → Mature → Drying → Withered).
        let initial = BlockInstanceState::default();
        assert_homogeneity(initial, 0.3, 500, "catchup-500-dry");
        assert_homogeneity(initial, 0.8, 500, "catchup-500-wet");
        assert_homogeneity(initial, 0.5, 500, "catchup-500-marginal");
    }

    #[test]
    fn time_homogeneity_zero_ticks_no_op() {
        // n=0 should leave state untouched and return current stage.
        let mut s = BlockInstanceState::default();
        s.age_ticks = 7;
        s.flags = Stage::Growing.into_flags(s.flags);
        let stage = advance_n_ticks(&mut s, 0.5, 0);
        assert_eq!(stage, Stage::Growing);
        assert_eq!(s.age_ticks, 7);
    }

    /// Integration test for the Mature→Drying transition animation:
    /// the handler should NOT immediately commit the stage change;
    /// instead it should enqueue a transition_start. After the
    /// animation duration elapses, on the next handler firing, the
    /// transition should commit (write the new stage) and finish
    /// (clear the entry).
    #[test]
    fn handler_starts_transition_on_mature_to_drying() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId, DeferredTransitionOp};
        use dashmap::DashMap;

        // Set up a Mature plant in dry conditions. The Mature→Drying
        // transition is now noise-sampler-driven: not every tick will
        // fire on a given plant, but over MANY ticks under dry
        // conditions, a sample SHOULD eventually land on the plant
        // and the probability function (p_death(0.2) ≈ 0.88) means
        // most landings fire the transition.
        //
        // We run up to 5000 ticks and verify a transition eventually
        // fires. With 8 samples per tick over 4096 chunk cells,
        // probability of hitting (5,5,5) is ~0.195% per tick, and
        // p_death(0.2) ≈ 0.88, so expected fire is ~0.17% per tick =
        // 1 in 580 ticks. 5000 ticks is plenty.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(5, 5, 5, BlockId::TallGrass);
        let plant_idx = Chunk::block_index(5, 5, 5) as u16;
        let mut s = BlockInstanceState::default();
        s.flags = Stage::Mature.into_flags(s.flags);
        s.age_ticks = STAGE_MATURE_DURATION_TICKS;
        chunk.instance_state.insert(plant_idx, s);
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(chunk));

        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "moisture" { Some(0.2) } else { None }
        };

        let handler = lifecycle_handler();

        let mut fired = false;
        let mut fire_tick: Option<u64> = None;
        for raw in 100u64..5100 {
            let mut ctx = TickContext {
                chunks: &chunks,
                field_reader: Some(&reader),
                chunk_field_reader: None,
                player_chunk: None,
                tick_id: TickId(raw),
                tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
                schedule: None, fire_tracker: None,
                block_edits: Vec::new(),
                state_changes: Vec::new(),
                transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);

            // Look for a Start op targeting (5, 5, 5).
            for op in &ctx.transition_ops {
                if let DeferredTransitionOp::Start {
                    world_x, world_y, world_z, transition,
                } = op {
                    if (*world_x, *world_y, *world_z) == (5, 5, 5) {
                        assert_eq!(transition.from_stage, Stage::Mature as u8);
                        assert_eq!(transition.to_stage, Stage::Drying as u8);
                        assert_eq!(transition.started_tick, raw);
                        fired = true;
                        fire_tick = Some(raw);
                        break;
                    }
                }
            }
            if fired { break; }
        }
        assert!(fired,
            "Mature→Drying sampler should have fired within 5000 ticks under dry conditions; \
             fire_tick={:?}", fire_tick);
    }

    /// Companion test: when the animation duration has elapsed, the
    /// next handler firing should commit the transition (write the
    /// to_stage to instance_state) and enqueue a Finish.
    #[test]
    fn handler_commits_transition_after_anim_duration() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord, BlockTransition,
            TRANSITION_ANIM_TICKS};
        use crate::world::ticks::{TickContext, TickId, DeferredTransitionOp};
        use dashmap::DashMap;

        // Plant in Mature stage (still — the transition hasn't committed
        // yet) with an active Mature→Drying transition started at tick
        // 100. We tick at tick 100 + ANIM_TICKS so the handler should
        // commit + finish.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(5, 5, 5, BlockId::TallGrass);
        let plant_idx = Chunk::block_index(5, 5, 5) as u16;
        let mut s = BlockInstanceState::default();
        s.flags = Stage::Mature.into_flags(s.flags);
        s.age_ticks = STAGE_MATURE_DURATION_TICKS;
        chunk.instance_state.insert(plant_idx, s);
        chunk.transitions.insert(plant_idx, BlockTransition {
            from_stage: Stage::Mature as u8,
            to_stage: Stage::Drying as u8,
            started_tick: 100,
        });
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(chunk));

        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "moisture" { Some(0.2) } else { None }
        };

        let handler = lifecycle_handler();
        let mut ctx = TickContext {
            chunks: &chunks,
            field_reader: Some(&reader),
                chunk_field_reader: None,
            player_chunk: None,
            // Tick at exactly the animation deadline.
            tick_id: TickId(100 + TRANSITION_ANIM_TICKS),
            tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
            schedule: None, fire_tracker: None,
            block_edits: Vec::new(),
            state_changes: Vec::new(),
            transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
        };
        handler(&mut ctx);

        // Should have committed: ONE state change (writing Drying) AND
        // ONE Finish op.
        assert_eq!(ctx.state_changes.len(), 1, "expected one state change for commit");
        assert_eq!(ctx.transition_ops.len(), 1, "expected one Finish op");

        let s = ctx.state_changes[0].new_state;
        assert_eq!(Stage::from_flags(s.flags), Stage::Drying,
            "committed stage should be Drying");
        assert_eq!(s.age_ticks, 0,
            "age_ticks should reset to 0 on commit (matching advance_one_tick)");

        match ctx.transition_ops[0] {
            DeferredTransitionOp::Finish { world_x, world_y, world_z } => {
                assert_eq!((world_x, world_y, world_z), (5, 5, 5));
            }
            _ => panic!("expected Finish op"),
        }
    }

    /// Mid-animation tick: the handler should NOT touch a plant whose
    /// transition is still in progress. The mesher continues
    /// interpolating; the handler waits.
    #[test]
    fn handler_skips_plants_mid_animation() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord, BlockTransition};
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(5, 5, 5, BlockId::TallGrass);
        let plant_idx = Chunk::block_index(5, 5, 5) as u16;
        let mut s = BlockInstanceState::default();
        s.flags = Stage::Mature.into_flags(s.flags);
        chunk.instance_state.insert(plant_idx, s);
        chunk.transitions.insert(plant_idx, BlockTransition {
            from_stage: Stage::Mature as u8,
            to_stage: Stage::Drying as u8,
            started_tick: 100,
        });
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(chunk));

        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "moisture" { Some(0.2) } else { None }
        };

        let handler = lifecycle_handler();
        // Tick mid-animation (3 ticks in, animation is 6 ticks).
        let mut ctx = TickContext {
            chunks: &chunks,
            field_reader: Some(&reader),
                chunk_field_reader: None,
            player_chunk: None,
            tick_id: TickId(103),
            tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
            schedule: None, fire_tracker: None,
            block_edits: Vec::new(),
            state_changes: Vec::new(),
            transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
        };
        handler(&mut ctx);

        // No edits, no state changes, no transition ops — handler
        // should be a no-op for this plant during mid-animation.
        assert!(ctx.state_changes.is_empty(),
            "mid-animation should produce no state changes; got {}",
            ctx.state_changes.len());
        assert!(ctx.transition_ops.is_empty(),
            "mid-animation should produce no transition ops; got {}",
            ctx.transition_ops.len());
        assert!(ctx.block_edits.is_empty(),
            "mid-animation should produce no block edits");
    }

    /// Catch-up scenario: when ticks_to_advance is large (chunk was
    /// in CatchUp band for a long time then re-entered), the handler
    /// should NOT start an animation — it should commit the stage
    /// directly. The player wasn't there to see the animation.
    #[test]
    fn handler_skips_animation_during_catchup() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId};
        use crate::world::scheduler::{LastFireTracker, TickScheduleParams};
        use dashmap::DashMap;

        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(5, 5, 5, BlockId::TallGrass);
        let plant_idx = Chunk::block_index(5, 5, 5) as u16;
        let mut s = BlockInstanceState::default();
        s.flags = Stage::Mature.into_flags(s.flags);
        s.age_ticks = STAGE_MATURE_DURATION_TICKS;
        chunk.instance_state.insert(plant_idx, s);
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(chunk));

        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "moisture" { Some(0.2) } else { None }
        };

        // Set up tracker so the handler's elapsed_or_default returns
        // a large catch-up gap (much bigger than animation duration).
        let tracker = LastFireTracker::new();
        tracker.mark_fired("plant_lifecycle", ChunkCoord::new(0, 0, 0), TickId(100));
        let params = TickScheduleParams::default();

        let handler = lifecycle_handler();
        // Tick 100 + 500 = catch-up gap of 500 (> ANIM_TICKS).
        let mut ctx = TickContext {
            chunks: &chunks,
            field_reader: Some(&reader),
                chunk_field_reader: None,
            player_chunk: Some(ChunkCoord::new(0, 0, 0)),
            tick_id: TickId(600),
            tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
            schedule: Some(params),
            fire_tracker: Some(&tracker),
            block_edits: Vec::new(),
            state_changes: Vec::new(),
            transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
        };
        // r191: stagger-aware — run the plant's full 8-tick window.
        for tk in 600..608u64 {
            ctx.tick_id = TickId(tk);
            handler(&mut ctx);
            if !ctx.state_changes.is_empty() || !ctx.block_edits.is_empty() { break; }
        }

        // Catch-up: should commit directly (state change OR block edit
        // for death), not start an animation (no transition op).
        // With 500 catch-up ticks and dry conditions starting from
        // Mature(age=60), the plant cycles through Drying → Withered
        // and likely dies — that's a block edit (TallGrass → Air),
        // not a state change. Either output is fine; the assertion
        // is just "something was done, but no animation."
        assert!(ctx.transition_ops.is_empty(),
            "catch-up should NOT start animation; got {} ops", ctx.transition_ops.len());
        assert!(!ctx.state_changes.is_empty() || !ctx.block_edits.is_empty(),
            "catch-up should produce direct state change(s) or block edit(s)");
    }

    /// Growth-as-spawning: an empty cell above grass in wet conditions
    /// should eventually have a Growth sample fire on it and spawn a
    /// new plant block. Verifies the spawning path through the handler.
    #[test]
    fn handler_spawns_plant_in_eligible_empty_cell() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId, DeferredEdit};
        use dashmap::DashMap;

        // Set up a chunk with grass at (5, 5, 5) and air at (5, 6, 5).
        // The eligible spawn cell is (5, 6, 5) — air with grass below,
        // y > 0 so no cross-chunk concerns.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(5, 5, 5, BlockId::Grass);
        // (5, 6, 5) stays Air by default.
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(chunk));

        // Wet conditions so p_growth is high.
        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "moisture" { Some(0.8) } else { None }
        };

        let handler = lifecycle_handler();

        // Run up to 5000 ticks looking for a spawn. Same probability
        // math as the death-fire test: ~0.2% per tick chance of a
        // sample landing on (5, 6, 5), then p_growth(0.8) ≈ 0.88.
        // Expected fire ~0.18% per tick = 1 in ~570 ticks. 5000 is
        // plenty.
        let mut spawned = false;
        let mut spawn_tick: Option<u64> = None;
        for raw in 0u64..5000 {
            let mut ctx = TickContext {
                chunks: &chunks,
                field_reader: Some(&reader),
                chunk_field_reader: None,
                player_chunk: None,
                tick_id: TickId(raw),
                tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
                schedule: None, fire_tracker: None,
                block_edits: Vec::new(),
                state_changes: Vec::new(),
                transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);
            for e in &ctx.block_edits {
                if (e.world_x, e.world_y, e.world_z) == (5, 6, 5)
                    && matches!(e.new_block, BlockId::TallGrass)
                {
                    spawned = true;
                    spawn_tick = Some(raw);
                    break;
                }
            }
            if spawned { break; }
            // Suppress unused warning.
            let _ = std::marker::PhantomData::<DeferredEdit>;
        }
        assert!(spawned,
            "Growth sampler should have spawned a plant within 5000 ticks; \
             spawn_tick={:?}", spawn_tick);
    }

    /// Spawn ineligibility: a cell with no grass below (just air or
    /// stone) should NOT spawn even if Growth samples land on it.
    #[test]
    fn handler_does_not_spawn_without_grass_substrate() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        // (5, 6, 5) is air, (5, 5, 5) is STONE (not grass). No spawn
        // should happen here.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(5, 5, 5, BlockId::Stone);
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(chunk));

        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "moisture" { Some(0.8) } else { None }
        };

        let handler = lifecycle_handler();

        // Run 2000 ticks. Even with high p_growth, no spawns should
        // happen because substrate check fails. (Other random samples
        // might land elsewhere, but those cells have no grass below
        // either — chunk is empty except for one stone.)
        for raw in 0u64..2000 {
            let mut ctx = TickContext {
                chunks: &chunks,
                field_reader: Some(&reader),
                chunk_field_reader: None,
                player_chunk: None,
                tick_id: TickId(raw),
                tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
                schedule: None, fire_tracker: None,
                block_edits: Vec::new(),
                state_changes: Vec::new(),
                transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);
            assert!(ctx.block_edits.is_empty(),
                "no spawns should occur without grass substrate; got {} edits at tick {}",
                ctx.block_edits.len(), raw);
        }
    }

    /// Spawn doesn't happen at chunk y=0 (cross-chunk substrate read
    /// is deferred). Even with grass at (lx, 0, lz), spawning at
    /// (lx, 1, lz) is fine, but spawning at (lx, 0, lz) when there
    /// would be grass in the chunk below is NOT attempted.
    ///
    /// We test this by setting up an air column at (5, 0, 5) and
    /// verifying that even in wet conditions the handler doesn't
    /// emit a spawn at y=0. (We can't set up grass "below" at
    /// y=-1 of the chunk since that's outside.)
    #[test]
    fn handler_does_not_spawn_at_chunk_y_zero() {
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        let chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        // Whole chunk is air. (5, 0, 5) is air with no substrate
        // detectable in this chunk. Even in wet conditions, no spawn.
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(chunk));

        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "moisture" { Some(0.9) } else { None }
        };

        let handler = lifecycle_handler();

        // Many ticks — each Growth sample landing at any y=0 cell
        // should NOT spawn (because we explicitly skip y=0). Other
        // y values are also air so they wouldn't spawn either.
        for raw in 0u64..1000 {
            let mut ctx = TickContext {
                chunks: &chunks,
                field_reader: Some(&reader),
                chunk_field_reader: None,
                player_chunk: None,
                tick_id: TickId(raw),
                tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
                schedule: None, fire_tracker: None,
                block_edits: Vec::new(),
                state_changes: Vec::new(),
                transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);
            assert!(ctx.block_edits.is_empty(),
                "no spawns should occur at y=0 (cross-chunk read deferred)");
        }
    }

    /// Tree growth: a small tree (3-block trunk + leaf cluster) should
    /// have its leaves replaced with wood over time, with new leaves
    /// added around the new wood. We verify by:
    ///   1. Setting up a 3-block trunk with leaves around the top.
    ///   2. Running the handler many ticks under wet conditions.
    ///   3. Expecting at least one leaf-to-log conversion.
    #[test]
    fn handler_grows_trees_via_leaf_to_wood() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        // Build a tree: trunk at x=8, z=8, y=4..7; leaves at the top
        // and around y=6.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for y in 4..7 {
            chunk.set(8, y, 8, BlockId::OakLog);
        }
        // Leaf canopy around the top of the trunk.
        for &(dx, dy, dz) in &[
            (0, 1, 0),  // directly above trunk top
            (1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1),  // sides of top
            (1, 1, 0), (-1, 1, 0), (0, 1, 1), (0, 1, -1),  // sides of above
        ] {
            chunk.set(8 + dx, 6 + dy, 8 + dz, BlockId::OakLeaves);
        }
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(chunk));

        // Wet conditions for high p_tree_growth.
        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "moisture" { Some(0.85) } else { None }
        };

        let handler = lifecycle_handler();

        // Run up to 20000 ticks and look for a leaf-to-log conversion.
        // Expected fire rate: 8 samples × ~9/4096 chance of hitting a
        // canopy leaf × p_tree_growth(0.85) ≈ 0.27 = ~0.5% per tick.
        // So expected first fire ~200 ticks; 20000 is huge margin.
        let mut log_grew = false;
        let mut grew_at: Option<(i32, i32, i32)> = None;
        for raw in 0u64..20000 {
            let mut ctx = TickContext {
                chunks: &chunks,
                field_reader: Some(&reader),
                chunk_field_reader: None,
                player_chunk: None,
                tick_id: TickId(raw),
                tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
                schedule: None, fire_tracker: None,
                block_edits: Vec::new(),
                state_changes: Vec::new(),
                transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);
            for e in &ctx.block_edits {
                if matches!(e.new_block, BlockId::OakLog) {
                    log_grew = true;
                    grew_at = Some((e.world_x, e.world_y, e.world_z));
                    break;
                }
            }
            if log_grew { break; }
        }
        assert!(log_grew,
            "tree should have grown a new log within 20000 ticks; grew_at={:?}",
            grew_at);
    }

    /// Tree growth diameter cap: a tree that's already at the cap
    /// should NOT grow further. We test by constructing a cluster
    /// explicitly at the cap and verifying no log conversions fire.
    ///
    /// Note: TREE_DIAMETER_CAP is 100, so this test would need to
    /// build a 100-block-spanning tree which is bigger than a single
    /// chunk. Instead we test the cap's MECHANISM: lower the cap
    /// would reject growth, but since the constant is fixed, we test
    /// that the diameter check is wired up by checking the
    /// `cluster.diameter()` of a known tree matches expectation.
    /// Real cap testing happens at the cluster module level.
    #[test]
    fn tree_growth_respects_cluster_diameter() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::cluster::{cluster_at, SingleChunkSource, TreeRule};

        // Build a 5-tall trunk.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for y in 4..9 {
            chunk.set(8, y, 8, BlockId::OakLog);
        }
        let source = SingleChunkSource { chunk: &chunk };
        let cluster = cluster_at(8, 4, 8, &TreeRule, &source);
        assert_eq!(cluster.size(), 5);
        // Diameter = 4 (Chebyshev between y=4 and y=8).
        assert_eq!(cluster.diameter(), 4);
        // Far below the 100-block cap, so growth would be allowed.
        assert!(cluster.diameter() < TREE_DIAMETER_CAP);
    }

    /// A leaf with no adjacent wood should NOT trigger tree growth,
    /// even under wet conditions and many ticks. (Loose leaves left
    /// over from worldgen variations or after death should stay
    /// loose, not become a tree on their own.)
    #[test]
    fn handler_does_not_grow_isolated_leaves() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        // Lone OakLeaves with no adjacent wood.
        chunk.set(8, 5, 8, BlockId::OakLeaves);
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(chunk));

        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "moisture" { Some(0.9) } else { None }
        };

        let handler = lifecycle_handler();

        // Run 2000 ticks. No log conversions should occur.
        for raw in 0u64..2000 {
            let mut ctx = TickContext {
                chunks: &chunks,
                field_reader: Some(&reader),
                chunk_field_reader: None,
                player_chunk: None,
                tick_id: TickId(raw),
                tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
                schedule: None, fire_tracker: None,
                block_edits: Vec::new(),
                state_changes: Vec::new(),
                transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);
            for e in &ctx.block_edits {
                assert!(!matches!(e.new_block, BlockId::OakLog | BlockId::BirchLog),
                    "isolated leaf should not become a log; got {:?} at ({}, {}, {}) tick {}",
                    e.new_block, e.world_x, e.world_y, e.world_z, raw);
            }
        }
    }

    /// Per-chunk-per-tick growth event budget. Even with many
    /// eligible leaves and saturating moisture, at most
    /// TREE_GROWTH_EVENTS_PER_CHUNK_PER_TICK leaf→log conversions
    /// should fire on a single tick. Without this budget, a forested
    /// chunk grows up to SAMPLES_PER_CHUNK_PER_TICK = 8 blocks per tick.
    #[test]
    fn tree_growth_capped_per_chunk_per_tick() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        // Build a forest: 4 separate small trees in the chunk so we
        // have plenty of eligible leaves for samples to land on.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for &(tx, tz) in &[(4, 4), (4, 20), (20, 4), (20, 20)] {
            // Trunk: 3 wood blocks tall.
            for y in 4..7 {
                chunk.set(tx, y, tz, BlockId::OakLog);
            }
            // Leaf canopy.
            for &(dx, dy, dz) in &[
                (0, 1, 0), (1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1),
                (1, 1, 0), (-1, 1, 0), (0, 1, 1), (0, 1, -1),
            ] {
                chunk.set(tx + dx, 6 + dy, tz + dz, BlockId::OakLeaves);
            }
        }
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(chunk));

        // Saturate moisture so probability fires every sample.
        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "moisture" { Some(0.99) } else { None }
        };

        let handler = lifecycle_handler();

        // Walk many ticks; on every tick the growth events count
        // should never exceed the budget.
        let mut max_seen = 0;
        for raw in 0u64..200 {
            let mut ctx = TickContext {
                chunks: &chunks,
                field_reader: Some(&reader),
                chunk_field_reader: None,
                player_chunk: None,
                tick_id: TickId(raw),
                tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
                schedule: None, fire_tracker: None,
                block_edits: Vec::new(),
                state_changes: Vec::new(),
                transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);
            // Count leaf→log conversions (the growth events).
            let conversions = ctx.block_edits.iter().filter(|e| {
                matches!(e.new_block, BlockId::OakLog | BlockId::BirchLog)
            }).count();
            max_seen = max_seen.max(conversions);
            assert!(
                conversions <= super::TREE_GROWTH_EVENTS_PER_CHUNK_PER_TICK,
                "tick {}: {} growth events fired, exceeds budget {}",
                raw, conversions, super::TREE_GROWTH_EVENTS_PER_CHUNK_PER_TICK,
            );
        }
        // Sanity: we should have observed at least one tick with
        // events fired (otherwise we're not actually testing the cap).
        assert!(max_seen > 0,
            "no growth events fired in 200 ticks — test isn't actually exercising the path");
    }

    /// Cluster-size soft cap: a tree at or above
    /// TREE_CLUSTER_SOFT_CAP wood blocks should stop growing even
    /// under saturating moisture. Without this gate, growth runs
    /// away into 10×10×10 wood blobs.
    #[test]
    fn tree_growth_taper_at_cluster_soft_cap() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        // Build a tree at exactly TREE_CLUSTER_SOFT_CAP wood blocks.
        // Linear column trunk, easy to count. Leaves all around to
        // maximize sample-hit rate.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let trunk_x = 16;
        let trunk_z = 16;
        let trunk_height = super::TREE_CLUSTER_SOFT_CAP as i32;
        for y in 0..trunk_height {
            chunk.set(trunk_x, 1 + y, trunk_z, BlockId::OakLog);
        }
        // Leaves in a thick ring at the top so plenty of growth
        // candidates exist.
        let top = 1 + trunk_height - 1;
        for &(dx, dy, dz) in &[
            (1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1),
            (1, 1, 0), (-1, 1, 0), (0, 1, 1), (0, 1, -1),
            (0, 1, 0),
            (2, 0, 0), (-2, 0, 0), (0, 0, 2), (0, 0, -2),
        ] {
            chunk.set(trunk_x + dx, top + dy, trunk_z + dz, BlockId::OakLeaves);
        }
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(chunk));

        // Saturate moisture.
        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "moisture" { Some(0.99) } else { None }
        };
        let handler = lifecycle_handler();

        // Walk many ticks. Count any leaf→log conversion and verify
        // at most a handful (the soft cap goes to zero exactly at
        // SOFT_CAP, but wood-add updates the cluster size on the
        // next sample, so a couple events past the cap are tolerable
        // since the cluster_at compute uses the pre-edit state).
        // We tolerate up to TREE_GROWTH_EVENTS_PER_CHUNK_PER_TICK
        // events as a single tick may still have fired since events
        // fire before we re-check size next tick.
        let mut total_growth = 0;
        for raw in 0u64..400 {
            let mut ctx = TickContext {
                chunks: &chunks,
                field_reader: Some(&reader),
                chunk_field_reader: None,
                player_chunk: None,
                tick_id: TickId(raw),
                tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
                schedule: None, fire_tracker: None,
                block_edits: Vec::new(),
                state_changes: Vec::new(),
                transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);
            for e in &ctx.block_edits {
                if matches!(e.new_block, BlockId::OakLog | BlockId::BirchLog) {
                    total_growth += 1;
                }
            }
        }
        // Without the soft cap, a tree at SOFT_CAP=30 starting from
        // ~30 wood blocks would grow ~30+ more blocks over 400 ticks
        // (high moisture, lots of leaves). With the cap, the
        // size_factor is 0 so the secondary probability gate fires
        // never. Allow a small tolerance for any race in the test
        // setup: the cluster compute uses the chunk state at sample
        // time, which is pre-apply.
        assert!(total_growth <= 2,
            "tree at TREE_CLUSTER_SOFT_CAP grew {} blocks in 400 ticks — soft cap is broken (expected ≤ 2)",
            total_growth);
    }

    /// Tree shrink fires under dry conditions: a tree with leaf-edge
    /// wood and very low moisture should lose at least one wood block
    /// over time.
    #[test]
    fn tree_shrink_fires_in_dry_conditions() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        // Build a tree: 4-block trunk + a small canopy. The top-of-trunk
        // wood and the wood with leaves around it are leaf-edge, so
        // they're shrink-eligible.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for y in 4..8 {
            chunk.set(8, y, 8, BlockId::OakLog);
        }
        // Leaf canopy around y=7 (top of trunk).
        for &(dx, dy, dz) in &[
            (0, 1, 0),
            (1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1),
            (1, 1, 0), (-1, 1, 0), (0, 1, 1), (0, 1, -1),
        ] {
            chunk.set(8 + dx, 7 + dy, 8 + dz, BlockId::OakLeaves);
        }
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(chunk));

        // Very dry conditions for high p_tree_shrink.
        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "moisture" { Some(0.10) } else { None }
        };

        let handler = lifecycle_handler();

        // Within 5000 ticks the tree should lose at least one wood block.
        let mut shrunk = false;
        for raw in 0u64..5000 {
            let mut ctx = TickContext {
                chunks: &chunks,
                field_reader: Some(&reader),
                chunk_field_reader: None,
                player_chunk: None,
                tick_id: TickId(raw),
                tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
                schedule: None, fire_tracker: None,
                block_edits: Vec::new(),
                state_changes: Vec::new(),
                transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);
            for e in &ctx.block_edits {
                // A shrink event replaces wood with leaf or air. We're
                // looking for the wood being replaced by something
                // non-wood (and non-Stone, since we built the trunk
                // out of OakLog only).
                if matches!(e.new_block,
                    BlockId::OakLeaves | BlockId::BirchLeaves | BlockId::Air)
                {
                    // Could be growth turning a leaf into something —
                    // but in dry conditions growth is rare and the
                    // event count we want to see is wood→leaf/air
                    // i.e. shrink. Distinguish by looking up what
                    // was at that position before.
                    let chunk = chunks.get(&ChunkCoord::new(0, 0, 0)).unwrap();
                    let lx = e.world_x - 0;
                    let ly = e.world_y - 0;
                    let lz = e.world_z - 0;
                    if (0..32).contains(&lx) && (0..32).contains(&ly) && (0..32).contains(&lz) {
                        let idx = Chunk::block_index(lx, ly, lz);
                        let was = chunk.blocks[idx as usize];
                        if matches!(was, BlockId::OakLog | BlockId::BirchLog) {
                            shrunk = true;
                            break;
                        }
                    }
                }
            }
            if shrunk { break; }
        }
        assert!(shrunk,
            "tree should have lost at least one wood block under dry moisture in 5000 ticks");
    }

    /// Tree shrink does NOT fire on interior trunk wood (all-wood
    /// neighbors). Only edge wood with at least one air or leaf
    /// neighbor is eligible.
    #[test]
    fn tree_shrink_protects_interior_wood() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        // Build a 3×3×3 wood cube. The center block has all 6 wood
        // neighbors — interior. The faces are edge.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for dx in 0..3 {
            for dy in 0..3 {
                for dz in 0..3 {
                    chunk.set(8 + dx, 8 + dy, 8 + dz, BlockId::OakLog);
                }
            }
        }
        let center = (9, 9, 9);
        let center_idx = Chunk::block_index(center.0, center.1, center.2);
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(ChunkCoord::new(0, 0, 0), std::sync::Arc::new(chunk));

        let reader = |name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> {
            if name == "moisture" { Some(0.05) } else { None } // very dry
        };

        let handler = lifecycle_handler();

        // Walk many ticks. The center block should NEVER be replaced.
        for raw in 0u64..1000 {
            let mut ctx = TickContext {
                chunks: &chunks,
                field_reader: Some(&reader),
                chunk_field_reader: None,
                player_chunk: None,
                tick_id: TickId(raw),
                tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
                schedule: None, fire_tracker: None,
                block_edits: Vec::new(),
                state_changes: Vec::new(),
                transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);
            for e in &ctx.block_edits {
                let lx = e.world_x;
                let ly = e.world_y;
                let lz = e.world_z;
                if (0..32).contains(&lx) && (0..32).contains(&ly) && (0..32).contains(&lz) {
                    let idx = Chunk::block_index(lx, ly, lz);
                    assert!(idx != center_idx,
                        "interior wood at {:?} was edited at tick {}; only edge wood should be shrink-eligible",
                        center, raw);
                }
            }
        }
    }

    // ─── Phase A: tree-root markers + POI cache ────────────────────────

    /// Helper: apply tree_root_ops from a TickContext to the chunks
    /// map. Mirrors what ChunkManager::apply does in production but
    /// tightly scoped for tests.
    #[cfg(test)]
    fn apply_tree_root_ops_for_test(
        chunks: &dashmap::DashMap<crate::world::chunk::ChunkCoord, std::sync::Arc<crate::world::chunk::Chunk>>,
        ops: Vec<crate::world::ticks::DeferredTreeRootOp>,
    ) {
        use crate::constants::CHUNK_SIZE;
        use crate::world::chunk::ChunkCoord;
        use crate::world::ticks::DeferredTreeRootOp;
        for op in ops {
            match op {
                DeferredTreeRootOp::SetMarker { world_x, world_y, world_z } => {
                    let cx = world_x.div_euclid(CHUNK_SIZE);
                    let cy = world_y.div_euclid(CHUNK_SIZE);
                    let cz = world_z.div_euclid(CHUNK_SIZE);
                    let lx = world_x.rem_euclid(CHUNK_SIZE);
                    let ly = world_y.rem_euclid(CHUNK_SIZE);
                    let lz = world_z.rem_euclid(CHUNK_SIZE);
                    if let Some(mut e) = chunks.get_mut(&ChunkCoord::new(cx, cy, cz)) {
                        let mut nc = e.value().as_ref().clone();
                        nc.set_tree_root_marker(lx, ly, lz, true);
                        *e = std::sync::Arc::new(nc);
                    }
                }
                DeferredTreeRootOp::ClearMarker { world_x, world_y, world_z } => {
                    let cx = world_x.div_euclid(CHUNK_SIZE);
                    let cy = world_y.div_euclid(CHUNK_SIZE);
                    let cz = world_z.div_euclid(CHUNK_SIZE);
                    let lx = world_x.rem_euclid(CHUNK_SIZE);
                    let ly = world_y.rem_euclid(CHUNK_SIZE);
                    let lz = world_z.rem_euclid(CHUNK_SIZE);
                    if let Some(mut e) = chunks.get_mut(&ChunkCoord::new(cx, cy, cz)) {
                        let mut nc = e.value().as_ref().clone();
                        nc.set_tree_root_marker(lx, ly, lz, false);
                        *e = std::sync::Arc::new(nc);
                    }
                }
                DeferredTreeRootOp::SetUnanchored { world_x, world_y, world_z, unanchored } => {
                    let cx = world_x.div_euclid(CHUNK_SIZE);
                    let cy = world_y.div_euclid(CHUNK_SIZE);
                    let cz = world_z.div_euclid(CHUNK_SIZE);
                    let lx = world_x.rem_euclid(CHUNK_SIZE);
                    let ly = world_y.rem_euclid(CHUNK_SIZE);
                    let lz = world_z.rem_euclid(CHUNK_SIZE);
                    if let Some(mut e) = chunks.get_mut(&ChunkCoord::new(cx, cy, cz)) {
                        let mut nc = e.value().as_ref().clone();
                        nc.set_tree_unanchored(lx, ly, lz, unanchored);
                        *e = std::sync::Arc::new(nc);
                    }
                }
                DeferredTreeRootOp::InstallPoiCache { chunk, cache } => {
                    if let Some(mut e) = chunks.get_mut(&chunk) {
                        let mut nc = e.value().as_ref().clone();
                        nc.poi_cache = Some(cache);
                        // v0.24.70: mirror the real chunk_manager
                        // InstallPoi path — fresh cache install clears
                        // the dirty flag.
                        nc.structural_dirty = false;
                        *e = std::sync::Arc::new(nc);
                    }
                }
            }
        }
    }

    /// Apply both block_edits and tree_root_ops from a ctx. The block
    /// path uses set() (which maintains tree_root_indices on
    /// wood-removal automatically).
    #[cfg(test)]
    fn apply_all_ops_for_test(
        chunks: &dashmap::DashMap<crate::world::chunk::ChunkCoord, std::sync::Arc<crate::world::chunk::Chunk>>,
        block_edits: Vec<crate::world::ticks::DeferredEdit>,
        tree_root_ops: Vec<crate::world::ticks::DeferredTreeRootOp>,
    ) {
        use crate::constants::CHUNK_SIZE;
        use crate::world::chunk::ChunkCoord;
        for e in block_edits {
            let cx = e.world_x.div_euclid(CHUNK_SIZE);
            let cy = e.world_y.div_euclid(CHUNK_SIZE);
            let cz = e.world_z.div_euclid(CHUNK_SIZE);
            let lx = e.world_x.rem_euclid(CHUNK_SIZE);
            let ly = e.world_y.rem_euclid(CHUNK_SIZE);
            let lz = e.world_z.rem_euclid(CHUNK_SIZE);
            if let Some(mut entry) = chunks.get_mut(&ChunkCoord::new(cx, cy, cz)) {
                let mut nc = entry.value().as_ref().clone();
                nc.set(lx, ly, lz, e.new_block);
                *entry = std::sync::Arc::new(nc);
            }
        }
        apply_tree_root_ops_for_test(chunks, tree_root_ops);
    }

    /// A tree planted on dirt should get a root marker after one
    /// lifecycle tick. The marker should be on the lowest wood block
    /// (the one touching dirt), not somewhere else.
    #[test]
    fn poi_pass_discovers_root_marker_on_anchored_tree() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        let coord = ChunkCoord::new(0, 0, 0);
        let mut chunk = Chunk::empty(coord);
        // Dirt floor at y=3, oak trunk y=4..8.
        chunk.set(8, 3, 8, BlockId::Dirt);
        for y in 4..9 { chunk.set(8, y, 8, BlockId::OakLog); }

        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(coord, std::sync::Arc::new(chunk));

        let reader = |_name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> { Some(0.5) };

        let handler = lifecycle_handler();
        let mut ctx = TickContext {
            chunks: &chunks,
            field_reader: Some(&reader),
            chunk_field_reader: None,
            player_chunk: None,
            tick_id: TickId(1),
            tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
            schedule: None, fire_tracker: None,
            block_edits: Vec::new(),
            state_changes: Vec::new(),
            transition_ops: Vec::new(),
            tree_root_ops: Vec::new(),
        };
        handler(&mut ctx);
        let edits = std::mem::take(&mut ctx.block_edits);
        let tops  = std::mem::take(&mut ctx.tree_root_ops);
        apply_all_ops_for_test(&chunks, edits, tops);

        let chunk_after = chunks.get(&coord).unwrap().clone();
        // The lowest-y wood block (y=4) touches dirt at y=3 → it's the
        // canonical anchor → should have FLAG_TREE_ROOT.
        let marker_idx = Chunk::block_index(8, 4, 8) as u16;
        assert!(chunk_after.tree_root_indices.contains(&marker_idx),
            "expected root marker at (8,4,8), got indices {:?}",
            chunk_after.tree_root_indices);
        let state = chunk_after.instance_state.get(&marker_idx).expect("instance state");
        assert!(state.has_flag(crate::world::chunk::BlockInstanceState::FLAG_TREE_ROOT));
        // POI cache populated with one tree.
        let cache = chunk_after.poi_cache.as_ref().expect("poi cache");
        assert_eq!(cache.tree_pois.len(), 1);
        assert_eq!(cache.tree_pois[0].anchor_candidate, Some([8, 4, 8]));
        assert_eq!(cache.tree_pois[0].cluster_size, 5);
    }

    /// A tree floating in air (no anchorage) should NOT get an anchor
    /// candidate in the POI cache. (The marker discovery is gated on
    /// stickiness in this test scenario; the POI alone won't set the
    /// marker without anchorage.)
    #[test]
    fn poi_pass_records_unanchored_floating_tree() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        let coord = ChunkCoord::new(0, 0, 0);
        let mut chunk = Chunk::empty(coord);
        // Floating trunk: no dirt below, all air around.
        for y in 5..10 { chunk.set(8, y, 8, BlockId::OakLog); }

        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(coord, std::sync::Arc::new(chunk));

        let reader = |_name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> { Some(0.5) };

        let handler = lifecycle_handler();
        let mut ctx = TickContext {
            chunks: &chunks,
            field_reader: Some(&reader),
            chunk_field_reader: None,
            player_chunk: None,
            tick_id: TickId(1),
            tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
            schedule: None, fire_tracker: None,
            block_edits: Vec::new(),
            state_changes: Vec::new(),
            transition_ops: Vec::new(),
            tree_root_ops: Vec::new(),
        };
        handler(&mut ctx);
        let tops  = std::mem::take(&mut ctx.tree_root_ops);
        apply_tree_root_ops_for_test(&chunks, tops);

        let chunk_after = chunks.get(&coord).unwrap().clone();
        let cache = chunk_after.poi_cache.as_ref().expect("poi cache");
        assert_eq!(cache.tree_pois.len(), 1, "tree should appear in cache");
        assert!(cache.tree_pois[0].anchor_candidate.is_none(),
            "floating tree should have no anchor candidate");
    }

    /// Once a marker is set on an anchored tree, subsequent ticks
    /// should keep it stable (no walks fire because anchorage is fine).
    #[test]
    fn marker_stays_put_when_anchorage_intact() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        let coord = ChunkCoord::new(0, 0, 0);
        let mut chunk = Chunk::empty(coord);
        chunk.set(8, 3, 8, BlockId::Dirt);
        for y in 4..9 { chunk.set(8, y, 8, BlockId::OakLog); }
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(coord, std::sync::Arc::new(chunk));
        let reader = |_name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> { Some(0.3) }; // dryish, no growth

        let handler = lifecycle_handler();
        for tick in 0u64..50 {
            let mut ctx = TickContext {
                chunks: &chunks,
                field_reader: Some(&reader),
                chunk_field_reader: None,
                player_chunk: None,
                tick_id: TickId(tick),
                tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
                schedule: None, fire_tracker: None,
                block_edits: Vec::new(),
                state_changes: Vec::new(),
                transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);
            let edits = std::mem::take(&mut ctx.block_edits);
            let tops  = std::mem::take(&mut ctx.tree_root_ops);
            apply_all_ops_for_test(&chunks, edits, tops);
        }

        let chunk_after = chunks.get(&coord).unwrap().clone();
        // Marker should still be at (8, 4, 8) — same place 50 ticks later.
        let marker_idx = Chunk::block_index(8, 4, 8) as u16;
        assert!(chunk_after.tree_root_indices.contains(&marker_idx),
            "marker should still be at (8,4,8) after 50 ticks; indices: {:?}",
            chunk_after.tree_root_indices);
        // Should NOT be flagged unanchored.
        let state = chunk_after.instance_state.get(&marker_idx).expect("state");
        assert!(!state.has_flag(crate::world::chunk::BlockInstanceState::FLAG_TREE_UNANCHORED),
            "anchored tree should not be flagged unanchored");
    }

    /// Removing the soil under an anchored tree should eventually
    /// flag it unanchored (after stickiness pass detects no anchorage).
    /// The marker may also walk if there's an adjacent wood with
    /// anchorage — but in this configuration there isn't one, so the
    /// outcome is "unanchored flag set."
    #[test]
    fn marker_becomes_unanchored_when_soil_removed() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        let coord = ChunkCoord::new(0, 0, 0);
        let mut chunk = Chunk::empty(coord);
        chunk.set(8, 3, 8, BlockId::Dirt);
        for y in 4..9 { chunk.set(8, y, 8, BlockId::OakLog); }
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(coord, std::sync::Arc::new(chunk));
        let reader = |_name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> { Some(0.3) };

        let handler = lifecycle_handler();

        // Tick 1: discover marker.
        {
            let mut ctx = TickContext {
                chunks: &chunks,
                field_reader: Some(&reader),
                chunk_field_reader: None,
                player_chunk: None,
                tick_id: TickId(1),
                tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
                schedule: None, fire_tracker: None,
                block_edits: Vec::new(),
                state_changes: Vec::new(),
                transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);
            let edits = std::mem::take(&mut ctx.block_edits);
            let tops  = std::mem::take(&mut ctx.tree_root_ops);
            apply_all_ops_for_test(&chunks, edits, tops);
        }

        // Now remove the dirt under the tree.
        {
            let mut entry = chunks.get_mut(&coord).unwrap();
            let mut nc = entry.value().as_ref().clone();
            nc.set(8, 3, 8, BlockId::Air);
            *entry = std::sync::Arc::new(nc);
        }

        // Run several ticks — eventually the cache rebuilds (every
        // 10 ticks) and the stickiness pass detects no anchorage.
        let handler = lifecycle_handler();
        let mut saw_unanchored = false;
        for tick in 2u64..50 {
            let mut ctx = TickContext {
                chunks: &chunks,
                field_reader: Some(&reader),
                chunk_field_reader: None,
                player_chunk: None,
                tick_id: TickId(tick),
                tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
                schedule: None, fire_tracker: None,
                block_edits: Vec::new(),
                state_changes: Vec::new(),
                transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);
            let edits = std::mem::take(&mut ctx.block_edits);
            let tops  = std::mem::take(&mut ctx.tree_root_ops);
            apply_all_ops_for_test(&chunks, edits, tops);
            let chunk_now = chunks.get(&coord).unwrap().clone();
            let marker_idx = Chunk::block_index(8, 4, 8) as u16;
            if let Some(state) = chunk_now.instance_state.get(&marker_idx) {
                if state.has_flag(crate::world::chunk::BlockInstanceState::FLAG_TREE_UNANCHORED) {
                    saw_unanchored = true;
                    break;
                }
            }
        }
        assert!(saw_unanchored, "marker should be flagged unanchored after soil is removed");
    }

    /// Removing a wood block that carries a marker should
    /// automatically clear the marker (via Chunk::set()'s
    /// tree_root_indices maintenance).
    #[test]
    fn marker_cleared_when_wood_removed() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};

        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(8, 3, 8, BlockId::Dirt);
        chunk.set(8, 4, 8, BlockId::OakLog);
        // Set the marker manually.
        chunk.set_tree_root_marker(8, 4, 8, true);
        let marker_idx = Chunk::block_index(8, 4, 8) as u16;
        assert!(chunk.tree_root_indices.contains(&marker_idx));

        // Replace the wood with air. Marker maintenance should fire.
        chunk.set(8, 4, 8, BlockId::Air);
        assert!(!chunk.tree_root_indices.contains(&marker_idx),
            "removing wood should clear its tree-root marker");
        assert!(chunk.instance_state.get(&marker_idx).is_none()
            || !chunk.instance_state[&marker_idx].has_flag(
                crate::world::chunk::BlockInstanceState::FLAG_TREE_ROOT,
            ),
            "removing wood should clear the FLAG_TREE_ROOT bit");
    }

    /// v0.24.70-structural-dirty: with explicit dirty marking, an
    /// idle chunk should rebuild its POI cache exactly ONCE (the
    /// initial build at tick 0 when chunk is fresh-dirty), then
    /// never again as long as no wood changes happen. The old
    /// cadence-based refresh would have produced multiple rebuilds
    /// over 30 ticks; the new behavior produces exactly one.
    #[test]
    fn poi_refresh_only_fires_when_dirty() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        let coord = ChunkCoord::new(0, 0, 0);
        let mut chunk = Chunk::empty(coord);
        chunk.set(8, 3, 8, BlockId::Dirt);
        for y in 4..9 { chunk.set(8, y, 8, BlockId::OakLog); }
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(coord, std::sync::Arc::new(chunk));
        let reader = |_name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> { Some(0.3) };

        let handler = lifecycle_handler();
        let mut tick_id_seen_in_cache: Vec<u64> = Vec::new();
        for tick in 0u64..30 {
            let mut ctx = TickContext {
                chunks: &chunks,
                field_reader: Some(&reader),
                chunk_field_reader: None,
                player_chunk: None,
                tick_id: TickId(tick),
                tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
                schedule: None, fire_tracker: None,
                block_edits: Vec::new(),
                state_changes: Vec::new(),
                transition_ops: Vec::new(),
                tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);
            let edits = std::mem::take(&mut ctx.block_edits);
            let tops  = std::mem::take(&mut ctx.tree_root_ops);
            apply_all_ops_for_test(&chunks, edits, tops);
            let chunk_now = chunks.get(&coord).unwrap().clone();
            if let Some(cache) = &chunk_now.poi_cache {
                let last = tick_id_seen_in_cache.last().copied().unwrap_or(u64::MAX);
                if cache.computed_at_tick.0 != last {
                    tick_id_seen_in_cache.push(cache.computed_at_tick.0);
                }
            }
        }
        // Old behavior: ≥3 rebuilds across 30 ticks (cadence).
        // New behavior: exactly 1 rebuild at tick 0 (initial dirty,
        // then InstallPoi clears the dirty flag; no further rebuilds
        // because nothing dirties the chunk again).
        assert_eq!(tick_id_seen_in_cache.len(), 1,
            "expected exactly 1 POI rebuild (initial), got {:?}",
            tick_id_seen_in_cache);
        assert_eq!(tick_id_seen_in_cache[0], 0,
            "the single rebuild should be at tick 0");
    }

    /// v0.24.70-structural-dirty: setting/removing wood after the
    /// initial build should trigger a fresh rebuild. Verifies the
    /// dirty flag flow: edit → dirty=true → next-tick rebuild →
    /// InstallPoi clears dirty=false.
    #[test]
    fn poi_refresh_fires_after_wood_edit() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::ticks::{TickContext, TickId};
        use dashmap::DashMap;

        let coord = ChunkCoord::new(0, 0, 0);
        let mut chunk = Chunk::empty(coord);
        chunk.set(8, 3, 8, BlockId::Dirt);
        for y in 4..9 { chunk.set(8, y, 8, BlockId::OakLog); }
        let chunks: DashMap<ChunkCoord, std::sync::Arc<Chunk>> = DashMap::new();
        chunks.insert(coord, std::sync::Arc::new(chunk));
        let reader = |_name: &str, _x: i32, _y: i32, _z: i32| -> Option<f32> { Some(0.3) };

        let handler = lifecycle_handler();

        // Tick 0: initial build.
        {
            let mut ctx = TickContext {
                chunks: &chunks, field_reader: Some(&reader),
                chunk_field_reader: None, player_chunk: None,
                tick_id: TickId(0),
                tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
                schedule: None, fire_tracker: None,
                block_edits: Vec::new(), state_changes: Vec::new(),
                transition_ops: Vec::new(), tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);
            let edits = std::mem::take(&mut ctx.block_edits);
            let tops  = std::mem::take(&mut ctx.tree_root_ops);
            apply_all_ops_for_test(&chunks, edits, tops);
        }
        let initial_tick = chunks.get(&coord).unwrap().poi_cache.as_ref()
            .expect("initial cache").computed_at_tick.0;
        assert_eq!(initial_tick, 0);

        // Manually dirty the chunk by adding a new wood block (which
        // sets structural_dirty=true).
        {
            let mut e = chunks.get_mut(&coord).unwrap();
            let mut nc = e.value().as_ref().clone();
            nc.set(10, 4, 10, BlockId::OakLog);
            assert!(nc.structural_dirty, "wood add should set dirty");
            *e = std::sync::Arc::new(nc);
        }

        // Tick 5: handler should see dirty=true and rebuild.
        {
            let mut ctx = TickContext {
                chunks: &chunks, field_reader: Some(&reader),
                chunk_field_reader: None, player_chunk: None,
                tick_id: TickId(5),
                tick_period: 0.5, tick_dt: 0.5, world_time: 0.0,
                schedule: None, fire_tracker: None,
                block_edits: Vec::new(), state_changes: Vec::new(),
                transition_ops: Vec::new(), tree_root_ops: Vec::new(),
            };
            handler(&mut ctx);
            let edits = std::mem::take(&mut ctx.block_edits);
            let tops  = std::mem::take(&mut ctx.tree_root_ops);
            apply_all_ops_for_test(&chunks, edits, tops);
        }
        let final_chunk = chunks.get(&coord).unwrap();
        let final_tick = final_chunk.poi_cache.as_ref()
            .expect("rebuilt cache").computed_at_tick.0;
        assert_eq!(final_tick, 5,
            "wood edit at tick ~5 should trigger rebuild at tick 5");
        assert!(!final_chunk.structural_dirty,
            "InstallPoi should have cleared the dirty flag");
    }
}
