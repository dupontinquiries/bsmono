//! `world::gen_falloff` — smooth falloff curves for chunk generation, so a burst of
//! hundreds of pending chunks becomes a gentle ramp instead of a spike.
//!
//! Two curves, both C1-continuous (no rate steps, which is what makes loading feel
//! smooth):
//!
//! 1. **Distance falloff** — near the player generation runs at full rate; toward
//!    the load edge it eases (smootherstep) down to a floor, so far rings trickle in
//!    rather than all being requested at once. This spreads the "hundreds pending
//!    during a climb" burst across more frames.
//! 2. **Settle falloff** — as the loaded set approaches its target, the total budget
//!    tapers toward zero, so a settled world stops hammering the generator ("falloff
//!    after a while").
//!
//! Note: these smooth *generation load*; they do not address GPU draw cost. When the
//! frame is pinned on the main pass (overdraw), occlusion culling is the lever, not
//! this. Pure & deterministic.

/// Smootherstep `6t^5 - 15t^4 + 10t^3` on `[0,1]` — zero first derivative at both
/// ends, so curves built on it have no slope discontinuity.
#[inline]
pub fn smootherstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Distance falloff weight in `[floor, 1]`: `1.0` at the player (`dist_ratio == 0`),
/// easing smoothly to `floor` at the load edge (`dist_ratio == 1`). `dist_ratio` is
/// the chunk's distance as a fraction of the load radius.
pub fn falloff(dist_ratio: f32, floor: f32) -> f32 {
    let floor = floor.clamp(0.0, 1.0);
    let d = dist_ratio.clamp(0.0, 1.0);
    floor + (1.0 - floor) * (1.0 - smootherstep(d))
}

/// Per-ring generation budget: `base_budget` scaled by the distance falloff (min 1
/// so distant rings still creep forward). Near rings fill fast, far rings trickle.
pub fn ring_budget(dist_ratio: f32, base_budget: u32, floor: f32) -> u32 {
    if base_budget == 0 {
        return 0;
    }
    let w = falloff(dist_ratio, floor);
    ((base_budget as f32) * w).round().max(1.0) as u32
}

/// Settle scale in `[0,1]`: full budget while the world is far from loaded, tapering
/// smoothly to `0` as `loaded` reaches `target`. Lets generation quiet down once a
/// region is essentially complete instead of spinning on the last few chunks.
pub fn settle_scale(loaded: u32, target: u32) -> f32 {
    if target == 0 {
        return 0.0;
    }
    let frac = (loaded as f32 / target as f32).clamp(0.0, 1.0);
    1.0 - smootherstep(frac)
}

/// Convenience: the effective per-tick budget combining a base rate, the settle
/// taper, and (optionally) a distance-ring falloff. `dist_ratio` may be `0.0` for a
/// single global budget.
pub fn effective_budget(base: u32, loaded: u32, target: u32, dist_ratio: f32, floor: f32) -> u32 {
    if base == 0 {
        return 0;
    }
    let s = settle_scale(loaded, target);
    let d = falloff(dist_ratio, floor);
    ((base as f32) * s * d).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falloff_endpoints_and_monotonic() {
        assert!((falloff(0.0, 0.1) - 1.0).abs() < 1e-6, "full rate at the player");
        assert!((falloff(1.0, 0.1) - 0.1).abs() < 1e-6, "floor at the edge");
        // Monotonically non-increasing with distance.
        let mut prev = f32::INFINITY;
        for i in 0..=100 {
            let w = falloff(i as f32 / 100.0, 0.1);
            assert!(w <= prev + 1e-6, "falloff must not rise: {w} > {prev}");
            prev = w;
        }
    }

    #[test]
    fn falloff_is_smooth_at_the_ends() {
        // Smootherstep has ~zero slope at 0 and 1, so the falloff plateaus gently
        // rather than kinking — the property that makes loading feel smooth.
        let near_slope = (falloff(0.02, 0.1) - falloff(0.0, 0.1)).abs();
        let mid_slope = (falloff(0.52, 0.1) - falloff(0.5, 0.1)).abs();
        assert!(near_slope < mid_slope, "should be flatter near the player than mid");
    }

    #[test]
    fn ring_budget_near_beats_far() {
        let near = ring_budget(0.1, 100, 0.1);
        let far = ring_budget(0.9, 100, 0.1);
        assert!(near > far, "near rings get more budget: {near} vs {far}");
        assert!(far >= 1, "far rings still creep forward");
    }

    #[test]
    fn settle_tapers_to_zero_when_loaded() {
        assert!((settle_scale(0, 1000) - 1.0).abs() < 1e-6, "empty world → full budget");
        assert!(settle_scale(1000, 1000).abs() < 1e-6, "loaded world → no budget");
        // Halfway is somewhere in between and strictly decreasing.
        assert!(settle_scale(250, 1000) > settle_scale(750, 1000));
    }

    #[test]
    fn effective_budget_combines_both() {
        // Far from loaded, near ring → close to base.
        let hot = effective_budget(64, 100, 1000, 0.0, 0.1);
        assert!(hot > 50, "hot region gets most of the base: {hot}");
        // Nearly loaded → tapered down regardless of ring.
        let cold = effective_budget(64, 980, 1000, 0.0, 0.1);
        assert!(cold < hot, "settled region gets less: {cold} vs {hot}");
    }

    #[test]
    fn degenerate_inputs_are_safe() {
        assert_eq!(ring_budget(0.5, 0, 0.1), 0);
        assert_eq!(settle_scale(5, 0), 0.0);
        assert_eq!(effective_budget(0, 0, 0, 0.0, 0.0), 0);
    }
}
