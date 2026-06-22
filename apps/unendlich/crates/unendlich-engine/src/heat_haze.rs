//! heat_haze.rs — pure refraction-distortion strength near a hot source.
//!
//! Shimmering distortion strength that's strong near a hot source, fades with
//! distance, and flickers over time, bounded to `[0,1]`. Pure, deterministic,
//! panic-free (R1/R11/R21).

/// Heat-haze strength at `dist` from the source, with `intensity` scale,
/// animated by `t`. Near + hot ⇒ strong, far ⇒ 0, shimmering with `t`. In
/// `[0,1]`.
pub fn haze(dist: f32, intensity: f32, t: f32) -> f32 {
    let i = intensity.max(0.0);
    let d = dist.max(0.0) / 8.0;
    let falloff = 1.0 / (1.0 + d * d); // 1 near, ->0 far
    // two-frequency flicker mapped into [0,1]
    let osc = (t * 7.0).sin() * 0.6 + (t * 13.0).sin() * 0.4; // [-1,1]
    let shimmer = 0.5 + 0.5 * osc; // [0,1]
    (i * falloff * shimmer).max(0.0).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_hot_stronger_than_far() {
        for &t in &[0.3f32, 1.0, 2.5, 4.0] {
            assert!(haze(1.0, 0.8, t) >= haze(60.0, 0.8, t), "near not >= far at t={}", t);
        }
    }

    #[test]
    fn far_is_zero() {
        assert!(haze(1e6, 1.0, 1.0) < 1e-3);
    }

    #[test]
    fn shimmers_bounded_and_varies() {
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        let mut t = 0.0f32;
        while t <= 10.0 {
            let h = haze(2.0, 0.9, t);
            assert!((0.0..=1.0).contains(&h), "haze oob {}", h);
            min = min.min(h);
            max = max.max(h);
            t += 0.05;
        }
        assert!(max - min > 0.05, "haze does not shimmer");
    }

    #[test]
    fn intensity_scales() {
        // pick dist/t avoiding the [0,1] clamp
        let t = 0.4f32;
        assert!(haze(10.0, 0.4, t) >= haze(10.0, 0.1, t), "intensity didn't scale");
    }

    #[test]
    fn continuous_in_time() {
        let step = 0.005f32;
        let mut prev = haze(3.0, 0.8, 0.0);
        let mut t = 0.0f32;
        while t <= 6.0 {
            let cur = haze(3.0, 0.8, t);
            assert!((cur - prev).abs() < 0.05, "jump at t={}", t);
            prev = cur;
            t += step;
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(haze(5.0, 0.7, 1.234), haze(5.0, 0.7, 1.234));
    }
}
