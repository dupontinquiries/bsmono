//! ssr_params.rs — pure screen-space-reflection march parameters.
//!
//! Derives the ray-march step configuration for SSR and the distance fade that
//! dims reflections as the march reaches its limit (hiding the hard cutoff).
//! Pure, deterministic, panic-free (R1/R11/R21).

/// Ray-march configuration for one SSR trace.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StepCfg {
    /// World/view distance advanced per step (`max_dist / steps`).
    pub step_size: f32,
    /// Number of march steps.
    pub steps: u32,
    /// Depth-acceptance thickness for a hit.
    pub thickness: f32,
}

/// Build the march config: `step_size = max_dist / steps`, with `thickness`
/// passed through (clamped non-negative). `steps == 0` is treated as a single
/// step so `step_size` stays finite.
pub fn march(steps: u32, thickness: f32, max_dist: f32) -> StepCfg {
    let md = max_dist.max(0.0);
    let denom = steps.max(1) as f32;
    StepCfg {
        step_size: md / denom,
        steps,
        thickness: thickness.max(0.0),
    }
}

/// Distance fade for a reflection that hit at `hit_dist`: `1` at the camera,
/// falling linearly to `0` at `max_dist`. Always in `[0,1]`, monotonically
/// non-increasing in `hit_dist`.
pub fn fade(hit_dist: f32, max_dist: f32) -> f32 {
    let md = max_dist.max(1e-4);
    let t = (hit_dist.max(0.0) / md).max(0.0).min(1.0);
    1.0 - t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_size_is_max_dist_over_steps() {
        for &steps in &[1u32, 8, 32, 64, 256] {
            let cfg = march(steps, 0.5, 100.0);
            assert!((cfg.step_size - 100.0 / steps as f32).abs() < 1e-4);
            assert_eq!(cfg.steps, steps);
        }
    }

    #[test]
    fn thickness_respected() {
        assert_eq!(march(32, 0.25, 50.0).thickness, 0.25);
        assert_eq!(march(32, -1.0, 50.0).thickness, 0.0, "negative thickness clamps to 0");
    }

    #[test]
    fn zero_steps_is_finite() {
        let cfg = march(0, 0.1, 80.0);
        assert!(cfg.step_size.is_finite(), "step size must stay finite");
        assert_eq!(cfg.step_size, 80.0, "0 steps treated as 1");
    }

    #[test]
    fn fade_in_unit_range() {
        let mut seed = 0x55A0_0001u32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / ((1u32 << 24) as f32)
        };
        for _ in 0..2000 {
            let hd = lcg() * 200.0 - 50.0;
            let md = lcg() * 200.0;
            let f = fade(hd, md);
            assert!((0.0..=1.0).contains(&f), "fade oob {}", f);
        }
    }

    #[test]
    fn fade_zero_at_max_and_one_at_zero() {
        assert!((fade(0.0, 100.0) - 1.0).abs() < 1e-6, "full at camera");
        assert!(fade(100.0, 100.0) < 1e-6, "zero at max_dist");
        assert!(fade(150.0, 100.0) < 1e-6, "zero beyond max_dist");
    }

    #[test]
    fn fade_monotone_non_increasing() {
        let mut prev = 2.0f32;
        let mut hd = 0.0f32;
        while hd <= 120.0 {
            let f = fade(hd, 100.0);
            assert!(f <= prev + 1e-6, "fade increased at {}", hd);
            prev = f;
            hd += 1.0;
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(march(48, 0.3, 75.0), march(48, 0.3, 75.0));
        assert_eq!(fade(33.0, 75.0), fade(33.0, 75.0));
    }
}
