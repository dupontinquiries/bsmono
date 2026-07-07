//! `biome_transition` — blend-zone weights between two biomes.
//!
//! Worker-03 / WORLD-PERF pure core, round 12. Near a biome boundary, returns
//! how much of biome A vs biome B applies, using a smoothstep so the seam has a
//! gentle S-curve rather than a linear ramp (no visible crease). `edge_dist` is
//! the signed distance from the boundary (negative = inside A); `width` is the
//! total blend-zone thickness. Feeds the weights into `biome_blend`.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Blend weights `[w_a, w_b]` (summing to 1) at signed distance `edge_dist`
/// from a boundary, over a blend zone of total `width`. Deep in A → `[1,0]`,
/// deep in B → `[0,1]`, on the boundary → `[0.5,0.5]`. Smoothstep transition.
pub fn weights(edge_dist: f32, width: f32) -> [f32; 2] {
    if !(width > 0.0) {
        // Hard edge at 0.
        return if edge_dist < 0.0 { [1.0, 0.0] } else { [0.0, 1.0] };
    }
    let t = clamp01((edge_dist + width * 0.5) / width);
    let s = t * t * (3.0 - 2.0 * t); // smoothstep
    [1.0 - s, s]
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum(w: [f32; 2]) -> f32 {
        w[0] + w[1]
    }

    #[test]
    fn weights_sum_to_one() {
        for e in -10..=10 {
            let w = weights(e as f32, 6.0);
            assert!((sum(w) - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn deep_in_a_is_pure_a() {
        let w = weights(-10.0, 4.0);
        assert!((w[0] - 1.0).abs() < 1e-6 && w[1].abs() < 1e-6);
    }

    #[test]
    fn deep_in_b_is_pure_b() {
        let w = weights(10.0, 4.0);
        assert!(w[0].abs() < 1e-6 && (w[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn midpoint_is_even() {
        let w = weights(0.0, 4.0);
        assert!((w[0] - 0.5).abs() < 1e-6 && (w[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn width_scales_the_zone() {
        // At the same edge_dist, a wider zone is less far along the transition.
        let narrow = weights(1.0, 2.0);
        let wide = weights(1.0, 10.0);
        assert!(narrow[1] > wide[1], "wider zone should blend less at fixed dist");
    }

    #[test]
    fn clamped_in_range() {
        for e in -20..=20 {
            let w = weights(e as f32, 5.0);
            assert!(w[0] >= 0.0 && w[0] <= 1.0);
            assert!(w[1] >= 0.0 && w[1] <= 1.0);
        }
    }

    #[test]
    fn deterministic_and_hard_edge() {
        assert_eq!(weights(1.5, 4.0), weights(1.5, 4.0));
        // width 0 → hard edge.
        assert_eq!(weights(-0.1, 0.0), [1.0, 0.0]);
        assert_eq!(weights(0.1, 0.0), [0.0, 1.0]);
    }
}
