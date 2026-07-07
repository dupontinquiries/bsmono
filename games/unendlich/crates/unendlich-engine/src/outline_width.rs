//! outline_width.rs — pure selection/hover outline thickness by distance.
//!
//! Outline pixel width shrinks with distance toward a floor (so distant
//! selections stay visible but thin). Pure, deterministic, panic-free
//! (R1/R11/R21).

/// Outline thickness in pixels at `dist` for a `base` width. Near ≈ `base`,
/// falling off with distance but never below 25% of `base`. Always `>= 0`,
/// monotonically non-increasing in `dist`, and linear in `base`.
pub fn outline_px(dist: f32, base: f32) -> f32 {
    let b = base.max(0.0);
    let d = dist.max(0.0);
    let floor = b * 0.25;
    (b / (1.0 + d / 16.0)).max(floor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_thicker_than_far() {
        assert!(outline_px(0.0, 4.0) > outline_px(100.0, 4.0));
    }

    #[test]
    fn far_hits_floor() {
        let floor = 4.0 * 0.25;
        assert!((outline_px(1e6, 4.0) - floor).abs() < 1e-4, "far should hit the floor");
    }

    #[test]
    fn non_negative() {
        let mut seed = 0x0117_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            assert!(outline_px(lcg() * 2000.0 - 100.0, lcg() * 8.0) >= 0.0);
        }
    }

    #[test]
    fn monotone_non_increasing() {
        let mut prev = f32::INFINITY;
        let mut d = 0.0f32;
        while d <= 500.0 {
            let w = outline_px(d, 4.0);
            assert!(w <= prev + 1e-6, "width grew at d={}", d);
            prev = w;
            d += 1.0;
        }
    }

    #[test]
    fn base_scales_linearly() {
        for &d in &[0.0f32, 10.0, 50.0, 300.0] {
            assert!((outline_px(d, 8.0) - 2.0 * outline_px(d, 4.0)).abs() < 1e-4, "not linear in base at d={}", d);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(outline_px(33.0, 4.0), outline_px(33.0, 4.0));
    }
}
