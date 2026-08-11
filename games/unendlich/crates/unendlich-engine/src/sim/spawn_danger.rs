//! Spawn danger scaling — RLCraft's defining idea: the world gets more
//! dangerous the deeper and farther out you go, not just "the same three
//! mobs forever." Nothing in this engine currently varies a creature's
//! combat stats by *where* it spawned (checked: no `mob_level`, no
//! `difficulty_scal*`, no structure/dungeon system exists yet either —
//! this is the smaller, purely-additive piece of that gap that's safe to
//! land without a worldgen-scale feature).
//!
//! Two independent axes, both monotonic and both capped (never unbounded —
//! an infinitely-scaling mob is a design bug, not difficulty):
//!   - **Depth**: below `SEA_LEVEL`, danger rises with distance underground.
//!     Caves already vary dragon *flavor* by depth (`sim::cave_dragon`,
//!     r692) — this adds *toughness* on the same axis, composing with
//!     that rather than replacing it.
//!   - **Distance from origin**: farther from `[0,0,0]` (the conventional
//!     "home"/spawn point), things get tougher — the classic "don't
//!     wander too far too early" RPG-survival shape.
//!
//! Deliberately NOT: altitude-based scaling (flying doesn't dodge
//! difficulty — RLCraft's "just build a sky base" escape hatch is
//! exactly the kind of thing worth NOT reproducing), or per-biome
//! multipliers (biome already carries its own flavor via `cave_dragon`
//! and surface biome tables; stacking a second orthogonal danger source
//! on top of biome specifically would double-count "dangerous-feeling"
//! places rather than reward exploration generally).

/// Below this depth, danger scaling saturates at its maximum — matches
/// roughly "deep cave" territory relative to `SEA_LEVEL` (64).
const MAX_DEPTH_BELOW_SEA_LEVEL: f32 = 96.0;
/// Beyond this distance from the origin, danger scaling saturates.
const MAX_DISTANCE_FROM_ORIGIN: f32 = 4000.0;
/// Multiplier contributed at full depth saturation (additive with distance).
const MAX_DEPTH_BONUS: f32 = 1.5;
/// Multiplier contributed at full distance saturation (additive with depth).
const MAX_DISTANCE_BONUS: f32 = 1.5;
/// Hard ceiling on the combined multiplier, regardless of how the two axes
/// combine — a single, auditable number for "how strong can this ever get."
const MAX_TOTAL_MULTIPLIER: f32 = 4.0;

/// Danger multiplier for a creature spawning at `position`. `1.0` at or
/// above sea level near the origin (baseline, unchanged from today).
/// Monotonically non-decreasing in both depth and distance; bounded above
/// by [`MAX_TOTAL_MULTIPLIER`]; never below `1.0`.
///
/// Intended use: scale a freshly-spawned creature's part HP (and, if
/// wired later, damage output) by this factor. Deliberately NOT wired
/// into anything yet in this patch beyond `spawn()`'s HP pass — see the
/// integration for exactly what that does and doesn't touch.
pub fn danger_multiplier(position: [f32; 3]) -> f32 {
    let depth_below_sea = (crate::constants::SEA_LEVEL as f32 - position[1]).max(0.0);
    let depth_frac = (depth_below_sea / MAX_DEPTH_BELOW_SEA_LEVEL).clamp(0.0, 1.0);
    let depth_bonus = depth_frac * MAX_DEPTH_BONUS;

    let dist = (position[0] * position[0] + position[2] * position[2]).sqrt();
    let dist_frac = (dist / MAX_DISTANCE_FROM_ORIGIN).clamp(0.0, 1.0);
    let dist_bonus = dist_frac * MAX_DISTANCE_BONUS;

    (1.0 + depth_bonus + dist_bonus).min(MAX_TOTAL_MULTIPLIER)
}

/// Apply a danger multiplier to a part HP value, rounding to the nearest
/// whole point and never rounding a positive HP down to 0 (a "buffed"
/// creature with 0 HP is a contradiction — floor at 1 for any part that
/// started with HP > 0).
pub fn scale_hp(base_hp: u16, multiplier: f32) -> u16 {
    if base_hp == 0 {
        return 0;
    }
    let scaled = (base_hp as f32 * multiplier).round();
    scaled.max(1.0) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_at_surface_near_origin_is_unchanged() {
        assert_eq!(danger_multiplier([0.0, 64.0, 0.0]), 1.0);
        assert_eq!(danger_multiplier([0.0, 70.0, 0.0]), 1.0, "above sea level, at the origin: still exactly baseline");
        // Genuinely "near" (not exactly at) the origin picks up a small,
        // correctly nonzero distance contribution — this is intentional
        // continuous scaling, not a step function, so "near" isn't quite
        // "exactly zero."
        assert!(danger_multiplier([10.0, 70.0, 10.0]) > 1.0);
        assert!(danger_multiplier([10.0, 70.0, 10.0]) < 1.01, "but the contribution at 14m out should be tiny");
    }

    #[test]
    fn deeper_than_sea_level_increases_danger() {
        let shallow = danger_multiplier([0.0, 60.0, 0.0]); // 4 below sea level
        let deep = danger_multiplier([0.0, 0.0, 0.0]); // 64 below sea level
        assert!(shallow > 1.0);
        assert!(deep > shallow, "deeper should be more dangerous than shallow");
    }

    #[test]
    fn depth_bonus_saturates_and_never_exceeds_its_cap() {
        let very_deep = danger_multiplier([0.0, -10_000.0, 0.0]);
        let past_saturation = danger_multiplier([0.0, 64.0 - MAX_DEPTH_BELOW_SEA_LEVEL - 500.0, 0.0]);
        // Both are past the saturation depth; must be equal (clamped), not
        // still climbing, and must not exceed 1.0 + MAX_DEPTH_BONUS (no
        // distance contribution here, both at x=z=0).
        assert!((very_deep - (1.0 + MAX_DEPTH_BONUS)).abs() < 1e-4);
        assert!((very_deep - past_saturation).abs() < 1e-4, "must saturate, not keep climbing");
    }

    #[test]
    fn farther_from_origin_increases_danger() {
        let near = danger_multiplier([100.0, 64.0, 0.0]);
        let far = danger_multiplier([3000.0, 64.0, 0.0]);
        assert!(near > 1.0);
        assert!(far > near, "farther from origin should be more dangerous");
    }

    #[test]
    fn distance_bonus_saturates() {
        let far = danger_multiplier([MAX_DISTANCE_FROM_ORIGIN, 64.0, 0.0]);
        let farther = danger_multiplier([MAX_DISTANCE_FROM_ORIGIN * 10.0, 64.0, 0.0]);
        assert!((far - farther).abs() < 1e-4, "must saturate, not keep climbing");
        assert!((far - (1.0 + MAX_DISTANCE_BONUS)).abs() < 1e-4);
    }

    #[test]
    fn depth_and_distance_compose_additively_up_to_the_hard_ceiling() {
        // Both axes fully saturated: 1.0 + 1.5 + 1.5 = 4.0, capped at
        // MAX_TOTAL_MULTIPLIER (also 4.0 here, so this exercises both the
        // arithmetic AND the cap agreeing — deliberately chosen so a
        // future constant change that makes them disagree gets caught).
        let both = danger_multiplier([MAX_DISTANCE_FROM_ORIGIN * 2.0, -1000.0, 0.0]);
        assert!((both - MAX_TOTAL_MULTIPLIER).abs() < 1e-4);
    }

    #[test]
    fn never_below_one_never_above_ceiling_across_a_wide_sweep() {
        let mut seed = 0xD15Cu64;
        let mut next = |lo: f32, hi: f32| {
            seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17;
            let unit = (seed >> 40) as f32 / (1u64 << 24) as f32;
            lo + unit * (hi - lo)
        };
        for _ in 0..5000 {
            let p = [next(-50_000.0, 50_000.0), next(-2000.0, 2000.0), next(-50_000.0, 50_000.0)];
            let m = danger_multiplier(p);
            assert!(m >= 1.0, "multiplier below 1.0 at {p:?}: {m}");
            assert!(m <= MAX_TOTAL_MULTIPLIER + 1e-4, "multiplier above ceiling at {p:?}: {m}");
            assert!(m.is_finite());
        }
    }

    #[test]
    fn scale_hp_rounds_and_floors_at_one_for_nonzero_input() {
        assert_eq!(scale_hp(10, 1.0), 10);
        assert_eq!(scale_hp(10, 2.0), 20);
        assert_eq!(scale_hp(10, 1.5), 15);
        assert_eq!(scale_hp(0, 3.0), 0, "an absent part (0 HP) stays absent, scaling doesn't invent one");
        assert_eq!(scale_hp(1, 0.1), 1, "a positive-HP part must never round down to 0 from scaling alone");
    }

    #[test]
    fn scale_hp_matches_danger_multiplier_at_realistic_values() {
        // A dragon eye (10 HP) deep in a cave far from origin.
        let mult = danger_multiplier([3500.0, -200.0, 3500.0]);
        assert_eq!(mult, MAX_TOTAL_MULTIPLIER, "this position should be fully saturated on both axes");
        assert_eq!(scale_hp(10, mult), 40, "10 HP at 4x = 40");
    }
}
