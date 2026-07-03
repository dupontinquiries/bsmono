//! `sim::stomp_erosion` — gentler, accumulation-gated footfall terrain damage.
//!
//! The original (r571/r573) stomp erosion crushed a full circular footprint to
//! bedrock-limited depth on EVERY qualifying footstep — too destructive per step,
//! and a perfect disc reads as artificial. This module reworks it around three
//! ideas the operator asked for:
//!
//!  * **Partial mining over a wider area.** Most footsteps only WEATHER the
//!    surface (tier it down a notch — Stone→Cobblestone, say) rather than
//!    removing it outright; the footprint can be wider because each hit is gentler.
//!  * **Repeated-spot breakthrough.** A block only actually breaks (crushes to
//!    air) once the SAME cell has been stomped on several times in a row — one
//!    stray step dents the ground, a few in the exact same spot break through it.
//!    [`update_streak`] tracks that per mover.
//!  * **Irregular footprint + varied result.** [`in_footprint`] ditches the
//!    perfect circle for a hash-jagged boundary, and [`weathered_variant`] picks
//!    among a small palette of plausible worn results (Cobblestone / Gravel /
//!    MossyCobblestone for Stone, etc.) instead of always the same one — using
//!    only block types that already exist and already render.
//!
//! Pure, deterministic (seeded), no allocation, no world access — the caller
//! walks the footprint and applies whatever this decides.

use crate::util::hash_util::mix64_unit;
use crate::world::blocks::BlockId;

/// Tunables for one stomp.
#[derive(Clone, Copy, Debug)]
pub struct StompParams {
    /// Max block hardness a FULL crush (once the streak threshold is met) can
    /// pulverise to air. Below this a block can still be weathered, just not
    /// broken outright.
    pub crush_cap: f32,
    /// Footprint outer radius in blocks (before jagged trimming).
    pub radius: i32,
    /// How many layers deep the foot searches for solid ground.
    pub depth: i32,
    /// Consecutive same-cell (x,z) stomps required before a block actually
    /// crushes to air; below this, qualifying hits only weather.
    pub streak_to_break: u32,
}

/// What happens to one solid cell this stomp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellAction {
    /// Tier the block down a notch (if it has a worn variant); otherwise no-op.
    Weather,
    /// Remove the block entirely (air).
    Crush,
}

/// Deterministic small-integer hash, used both for footprint jaggedness and
/// weathering variety — same seeding convention as `sim::plant_survival`.
fn cell_hash(seed: u64, dx: i32, dz: i32) -> f32 {
    let k = (dx as i64 as u64) ^ ((dz as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    mix64_unit(seed ^ k)
}

/// Is offset `(dx, dz)` from the foot center inside this stomp's footprint?
/// A hard outer bound at `radius`, but the boundary is JAGGED, not a perfect
/// circle: cells near the edge randomly drop out (deterministically, from
/// `seed`), so repeated stomps at different `seed`s (e.g. per-tick) carve an
/// organic, uneven patch rather than a stack of identical discs.
pub fn in_footprint(dx: i32, dz: i32, radius: i32, seed: u64) -> bool {
    let r = radius.max(0) as f32;
    if r <= 0.0 {
        return dx == 0 && dz == 0;
    }
    let d2 = (dx * dx + dz * dz) as f32;
    if d2 > r * r {
        return false; // never exceed the outer bound
    }
    let jag = cell_hash(seed, dx, dz); // 0..1
    // Effective radius per-cell ranges ~55%..100% of the outer bound, so the
    // core always hits but the rim is ragged.
    let r_eff = r * (0.55 + 0.45 * jag);
    d2 <= r_eff * r_eff
}

/// Update a mover's same-spot stomp streak: `cell` matching `last_cell`
/// increments it, anything else resets to `1` (this stomp starts a new streak).
pub fn update_streak(last_cell: Option<(i32, i32)>, cell: (i32, i32), prev_streak: u32) -> u32 {
    if last_cell == Some(cell) {
        prev_streak.saturating_add(1)
    } else {
        1
    }
}

/// Whether a block of `hardness` can be touched by a stomp at all. `false` =
/// bedrock (or otherwise untouchable) — the caller should stop drilling deeper
/// through this column. A `true` result still needs [`streak_permits_crush`] to
/// decide whether this particular hit weathers or actually crushes.
pub fn cell_actionable(hardness: f32) -> bool {
    hardness >= 0.0
}

/// Combine [`cell_actionable`] and [`streak_permits_crush`] into one decision
/// for a solid cell this stomp reaches: `None` = bedrock, stop drilling;
/// otherwise `Weather` (dent it) or `Crush` (streak + softness both qualify).
pub fn decide(hardness: f32, streak: u32, params: &StompParams) -> Option<CellAction> {
    if !cell_actionable(hardness) {
        return None;
    }
    if streak_permits_crush(hardness, streak, params) {
        Some(CellAction::Crush)
    } else {
        Some(CellAction::Weather)
    }
}

/// Whether the accumulated streak is enough to actually crush (vs merely
/// weather) a block this hard.
pub fn streak_permits_crush(hardness: f32, streak: u32, params: &StompParams) -> bool {
    hardness >= 0.0 && hardness <= params.crush_cap && streak >= params.streak_to_break
}

/// A varied, plausible weathered result for `block` — a small deterministic
/// palette instead of always the same swap, using only block types that
/// already exist (Gravel, MossyCobblestone alongside the original Cobblestone/
/// Dirt/Snow). `None` = this block doesn't weather (nothing changes).
pub fn weathered_variant(block: BlockId, seed: u64) -> Option<BlockId> {
    use BlockId::*;
    let r = mix64_unit(seed);
    match block {
        Grass => Some(Dirt),
        SnowBlock => Some(Snow),
        Stone => {
            if r < 0.55 {
                Some(Cobblestone)
            } else if r < 0.8 {
                Some(Gravel)
            } else {
                Some(MossyCobblestone)
            }
        }
        Cobblestone => {
            if r < 0.6 {
                Some(MossyCobblestone)
            } else {
                Some(Gravel)
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> StompParams {
        StompParams { crush_cap: 2.0, radius: 3, depth: 2, streak_to_break: 3 }
    }

    #[test]
    fn footprint_center_always_included_and_outer_bound_respected() {
        for seed in 0..50u64 {
            assert!(in_footprint(0, 0, 3, seed), "center should always be in-footprint");
            assert!(!in_footprint(10, 0, 3, seed), "far outside the radius must never qualify");
        }
    }

    #[test]
    fn footprint_is_jagged_not_a_perfect_disc() {
        // Pick a point strictly BETWEEN the guaranteed-in inner bound
        // (~0.55·radius) and the hard outer bound (radius) — a perfect circle
        // would agree on every seed there; the jagged version should disagree.
        let mut saw_in = false;
        let mut saw_out = false;
        for seed in 0..200u64 {
            if in_footprint(2, 0, 3, seed) {
                saw_in = true;
            } else {
                saw_out = true;
            }
        }
        assert!(saw_in && saw_out, "mid-rim cells should vary by seed (jagged), not be uniform");
    }

    #[test]
    fn streak_resets_on_new_cell_and_grows_on_repeat() {
        let mut streak = update_streak(None, (5, 5), 0);
        assert_eq!(streak, 1);
        streak = update_streak(Some((5, 5)), (5, 5), streak);
        assert_eq!(streak, 2);
        streak = update_streak(Some((5, 5)), (5, 5), streak);
        assert_eq!(streak, 3);
        // moved to a different cell -> resets
        streak = update_streak(Some((5, 5)), (6, 5), streak);
        assert_eq!(streak, 1);
    }

    #[test]
    fn crush_requires_both_soft_enough_and_streak_met() {
        let p = params();
        assert!(!streak_permits_crush(1.0, 1, &p), "streak too low");
        assert!(!streak_permits_crush(5.0, 5, &p), "too hard regardless of streak");
        assert!(streak_permits_crush(1.0, 3, &p), "soft enough + streak met");
        assert!(streak_permits_crush(1.0, 10, &p), "streak past threshold still fine");
    }

    #[test]
    fn bedrock_is_never_actionable() {
        assert!(!cell_actionable(-1.0));
        assert!(cell_actionable(0.0));
    }

    #[test]
    fn decide_combines_bedrock_and_streak() {
        let p = params();
        assert_eq!(decide(-1.0, 99, &p), None, "bedrock always stops");
        assert_eq!(decide(1.0, 1, &p), Some(CellAction::Weather), "streak too low -> weather only");
        assert_eq!(decide(1.0, 3, &p), Some(CellAction::Crush), "soft + streak met -> crush");
        assert_eq!(decide(5.0, 99, &p), Some(CellAction::Weather), "too hard ever -> weather only");
    }

    #[test]
    fn weathering_is_varied_but_deterministic() {
        // same seed -> same result (determinism)
        for seed in 0..20u64 {
            assert_eq!(weathered_variant(BlockId::Stone, seed), weathered_variant(BlockId::Stone, seed));
        }
        // across many seeds, Stone should hit more than one outcome (variety)
        let mut outcomes = std::collections::HashSet::new();
        for seed in 0..200u64 {
            outcomes.insert(weathered_variant(BlockId::Stone, seed));
        }
        assert!(outcomes.len() > 1, "stone weathering should vary across seeds, not always be the same block");
        // untracked blocks don't weather
        assert_eq!(weathered_variant(BlockId::Bedrock, 42), None);
    }
}
