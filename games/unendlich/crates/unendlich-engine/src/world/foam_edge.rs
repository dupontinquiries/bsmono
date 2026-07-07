//! `foam_edge` — shoreline foam intensity near the water line.
//!
//! Worker-03 / WORLD-FX pure core, round 20. Returns foam strength in [0,1]
//! based on how far the seabed sits below the water line: strongest right at the
//! shore (shallow water churns), fading to nothing in deeper water, and zero
//! above the line (dry land). Pairs with `sea_level` (R3) / `beach_band` (R18).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Foam in [0,1] for a submerged cell `depth_below_water` below the water line,
/// over a `foam_width` shallow band. Max at the shore (depth 0), 0 below water
/// surface line is impossible (negative depth = above water → 0), 0 past the
/// band.
pub fn foam(depth_below_water: f32, foam_width: f32) -> f32 {
    let w = foam_width.max(0.0);
    if w <= 0.0 {
        return if depth_below_water == 0.0 { 1.0 } else { 0.0 };
    }
    let d = depth_below_water;
    if d < 0.0 || d > w {
        return 0.0;
    }
    clamp01(1.0 - d / w)
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_at_the_edge() {
        assert!((foam(0.0, 3.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn deep_water_has_no_foam() {
        assert_eq!(foam(10.0, 3.0), 0.0);
    }

    #[test]
    fn above_water_has_no_foam() {
        assert_eq!(foam(-1.0, 3.0), 0.0);
    }

    #[test]
    fn width_scales_band() {
        assert!(foam(2.0, 8.0) > 0.0);
        assert_eq!(foam(2.0, 1.0), 0.0); // depth 2 beyond a 1-wide band
    }

    #[test]
    fn bounded() {
        for di in -20..40 {
            let v = foam(di as f32 * 0.5, 4.0);
            assert!(v >= 0.0 && v <= 1.0);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(foam(1.5, 4.0), foam(1.5, 4.0));
    }
}
