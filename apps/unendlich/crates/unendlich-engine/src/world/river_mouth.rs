//! `river_mouth` — river widens toward the sea (delta flare).
//!
//! Worker-03 / WORLD pure core, round 17. As a river approaches the sea its
//! channel flares into a delta; far upstream it relaxes to its base width. Width
//! is the base plus an extra that decays with distance to the sea, so it's
//! widest at the mouth, monotonically narrowing inland, and never below base.
//! Pairs with `river_carve` (R9) / `flow_accumulation` (R8).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const FLARE: f32 = 3.0; // mouth is up to (1+FLARE)× base, plus a floor
const FLARE_FLOOR: f32 = 2.0;
const DECAY_SCALE: f32 = 20.0;

/// Channel width at `dist_to_sea` for a river of `base_w`. Widest at the mouth
/// (`dist 0`), decaying to `base_w` far upstream, always `≥ base_w`.
pub fn width_at(dist_to_sea: f32, base_w: f32) -> f32 {
    let b = base_w.max(0.0);
    let d = dist_to_sea.max(0.0);
    let delta_max = b * FLARE + FLARE_FLOOR;
    let extra = delta_max / (1.0 + d / DECAY_SCALE);
    b + extra
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widest_at_the_sea() {
        let at_sea = width_at(0.0, 4.0);
        let upstream = width_at(100.0, 4.0);
        assert!(at_sea > upstream);
        // mouth width == base + full flare
        assert!((at_sea - (4.0 + (4.0 * FLARE + FLARE_FLOOR))).abs() < 1e-4);
    }

    #[test]
    fn approaches_base_upstream() {
        let far = width_at(100_000.0, 4.0);
        assert!((far - 4.0).abs() < 0.1, "did not relax to base");
    }

    #[test]
    fn monotone_decreasing_in_distance() {
        let mut prev = f32::INFINITY;
        for d in 0..200 {
            let w = width_at(d as f32, 5.0);
            assert!(w <= prev + 1e-6, "width rose going upstream");
            prev = w;
        }
    }

    #[test]
    fn never_below_base() {
        for d in 0..500 {
            assert!(width_at(d as f32, 6.0) >= 6.0);
        }
    }

    #[test]
    fn bounded_by_mouth_width() {
        let b = 5.0;
        let max = b + (b * FLARE + FLARE_FLOOR);
        for d in 0..300 {
            let w = width_at(d as f32, b);
            assert!(w <= max + 1e-4);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(width_at(12.0, 4.0), width_at(12.0, 4.0));
    }
}
