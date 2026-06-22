//! Block weathering — environmental aging that drives bumpy/eroded
//! visuals.
//!
//! Each block has a per-instance weathering value (stored in
//! `BlockInstanceState.age_ticks` interpreted as 0-255 = 0.0-1.0 of
//! "fully weathered"). This value:
//!
//!   - Is set at world-generation time from environment fields (a
//!     stone block in a humid cave gets high weathering; a stone
//!     block in dry desert gets low).
//!   - Defaults to 0 (fresh, unweathered, perfectly cubic) when a
//!     player places a block.
//!   - Is consumed by the mesher to drive bump amplitude — high
//!     weathering = more pronounced bumps; zero = perfectly flat top.
//!   - Slowly evolves toward an environment-driven equilibrium (TODO,
//!     phase 6+ when caves and water flow are wired).
//!
//! ## Why repurpose `age_ticks` instead of adding a new field
//!
//! `BlockInstanceState` is `(u8 age_ticks, u8 flags)` — 16 bits total,
//! shared across all block types. Plants already use age_ticks for
//! lifecycle progression (which IS effectively weathering — old
//! grass has more of an aged look). For terrain blocks (dirt, sand,
//! stone) `age_ticks` is currently unused, so we can repurpose it
//! as weathering without adding a new field. The two interpretations
//! don't conflict because they apply to disjoint block types
//! (plants vs. terrain).
//!
//! ## Initial weathering values
//!
//! Player-placed: `age_ticks = 0` (fresh, no bumps).
//! World-generated soft block (grass, dirt, sand, snow): high
//! weathering (~200 of 255) — these blocks are visually weathered by
//! default to look natural at gen time.
//! World-generated hard block (stone, log): low (0).
//!
//! ## What "weathered" means visually
//!
//! At weathering = 0: block tops are perfectly flat at integer Y.
//! At weathering = 1: full bump amplitude (top dips down by up to
//! ~0.18 block at "potholes"; surface is bumpy but never above the
//! cube outline).
//!
//! ## Future
//!
//! - Tick handler that slowly pushes weathering toward environment
//!   equilibrium based on humidity, temperature, wind, water
//!   proximity.
//! - Cave-specific equilibrium (humid cave floor → high weathering
//!   stone; dry desert stone → low).
//! - Animal/player trample as fast-acting weathering input.

use crate::world::blocks::BlockId;
use crate::world::chunk::BlockInstanceState;

/// Map age_ticks (0-255) to a normalized weathering value (0.0-1.0).
#[inline]
pub fn weathering_value(state: BlockInstanceState) -> f32 {
    state.age_ticks as f32 / 255.0
}

/// Default weathering for a world-generated block. Soft surface
/// blocks come out highly weathered (visible bumps); hard blocks come
/// out fresh (no bumps).
///
/// Future: read humidity / temperature / sky-exposure fields to
/// modulate. For now uses fixed per-block-type defaults.
pub fn worldgen_default_weathering(block: BlockId) -> u8 {
    if block.is_subvoxel_only() { return 0; }
    let softness = block.softness();
    if softness < 0.1 {
        // Hard blocks (stone, logs): no weathering at gen.
        0
    } else if softness < 0.55 {
        // Medium blocks (dirt, grass): medium weathering ~150.
        150
    } else {
        // Very soft blocks (sand, snow): high weathering ~210.
        210
    }
}

/// Depth-aware weathering for a world-generated block. Adds a cave-
/// flavored hook to the basic per-type defaults: hard blocks (stone)
/// gain weathering when deep underground (humidity / water-flow
/// proxy). Sky-exposed surface stone stays fresh; deep cave stone
/// becomes bumpy and eroded.
///
/// Approximation (no neighbor queries needed):
///   - Sea level (~64): stone gets near-zero weathering.
///   - 32 blocks below sea: stone gets medium weathering (~80).
///   - 64+ blocks below: stone gets high weathering (~150).
///
/// Soft blocks unchanged from the per-type default — sand/grass don't
/// weather extra just for being deep.
///
/// This is a worldgen approximation. Future: a real cave-weathering
/// pass that uses humidity / temperature / wind / water-flow fields
/// to drive per-cell weathering equilibrium.
pub fn worldgen_default_weathering_at_depth(block: BlockId, world_y: i32) -> u8 {
    let base = worldgen_default_weathering(block);
    // Only modify hard blocks (stone-like).
    if block.is_subvoxel_only() { return 0; }
    if block.softness() >= 0.1 { return base; }
    // Hard block depth ramp.
    let sea_level = crate::constants::SEA_LEVEL as i32;
    let depth_below = (sea_level - world_y).max(0);
    // 0 below sea level → 0; 32 below → ~80; 64+ below → ~150.
    let weathering = ((depth_below as f32 / 64.0).min(1.0) * 150.0) as u8;
    weathering
}

/// Default weathering for a player-placed block: zero. Fresh stone
/// from the chisel has no bumps; weathers in over time as physics
/// time progresses (future tick handler).
pub fn player_placed_weathering() -> u8 { 0 }

/// Bump amplitude multiplier for a given weathering value.
/// Returns 0.0 when weathering is 0 (block is fresh — no bumps),
/// up to 1.0 when fully weathered.
///
/// Currently a linear pass-through, but this is the hook where any
/// non-linear weathering→amplitude curve goes (e.g. early plateau
/// + late ramp for "no visible change until really old").
#[inline]
pub fn bump_amplitude_factor(weathering: f32) -> f32 {
    weathering.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_state_has_zero_weathering() {
        let s = BlockInstanceState::default();
        assert_eq!(weathering_value(s), 0.0);
    }

    #[test]
    fn full_age_is_full_weathering() {
        let s = BlockInstanceState { age_ticks: 255, flags: 0 };
        assert!((weathering_value(s) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn mid_age_is_mid_weathering() {
        let s = BlockInstanceState { age_ticks: 128, flags: 0 };
        let v = weathering_value(s);
        assert!(v > 0.4 && v < 0.6, "mid age should give mid weathering, got {}", v);
    }

    #[test]
    fn worldgen_defaults_track_softness() {
        let stone   = worldgen_default_weathering(BlockId::Stone);
        let dirt    = worldgen_default_weathering(BlockId::Dirt);
        let sand    = worldgen_default_weathering(BlockId::Sand);
        // Stone < dirt < sand by softness, so weathering should follow.
        assert!(stone < dirt, "stone({}) should weather less than dirt({})", stone, dirt);
        assert!(dirt < sand, "dirt({}) should weather less than sand({})", dirt, sand);
    }

    #[test]
    fn player_placed_starts_fresh() {
        assert_eq!(player_placed_weathering(), 0);
    }

    #[test]
    fn bump_amplitude_factor_clamps() {
        assert_eq!(bump_amplitude_factor(0.0), 0.0);
        assert_eq!(bump_amplitude_factor(1.0), 1.0);
        assert_eq!(bump_amplitude_factor(2.0), 1.0); // clamps
        assert_eq!(bump_amplitude_factor(-0.5), 0.0); // clamps
    }

    /// Depth-aware: stone at sea level has zero weathering (surface
    /// stone is fresh), stone deep underground has high weathering
    /// (cave humidity / water-flow proxy).
    #[test]
    fn depth_weathering_increases_with_depth_for_stone() {
        let sea = crate::constants::SEA_LEVEL as i32;
        // At sea level: stone is fresh.
        assert_eq!(worldgen_default_weathering_at_depth(BlockId::Stone, sea), 0,
            "stone at sea level should be fresh");
        // 32 below: medium weathering.
        let mid = worldgen_default_weathering_at_depth(BlockId::Stone, sea - 32);
        assert!(mid > 0 && mid < 150,
            "stone 32 below sea should have medium weathering, got {}", mid);
        // 64 below: max depth weathering (~150).
        let deep = worldgen_default_weathering_at_depth(BlockId::Stone, sea - 64);
        assert!(deep >= 100,
            "stone 64 below sea should have high weathering, got {}", deep);
        // 200 below: still capped at max.
        let abyss = worldgen_default_weathering_at_depth(BlockId::Stone, sea - 200);
        assert_eq!(abyss, deep,
            "weathering should plateau at deep depths, got {} vs {}", abyss, deep);
    }

    /// Depth-aware: soft blocks unchanged by depth. Sand at sea level
    /// has same weathering as sand 100 below (no humidity dependence
    /// for sand — it's already weathered by definition).
    #[test]
    fn depth_weathering_does_not_affect_soft_blocks() {
        let sea = crate::constants::SEA_LEVEL as i32;
        for y in [sea + 50, sea, sea - 32, sea - 100] {
            assert_eq!(
                worldgen_default_weathering_at_depth(BlockId::Sand, y),
                worldgen_default_weathering(BlockId::Sand),
                "sand weathering should not depend on Y");
            assert_eq!(
                worldgen_default_weathering_at_depth(BlockId::Dirt, y),
                worldgen_default_weathering(BlockId::Dirt),
                "dirt weathering should not depend on Y");
            assert_eq!(
                worldgen_default_weathering_at_depth(BlockId::Grass, y),
                worldgen_default_weathering(BlockId::Grass),
                "grass weathering should not depend on Y");
        }
    }
}
