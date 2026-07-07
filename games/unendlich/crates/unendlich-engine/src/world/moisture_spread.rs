//! `moisture_spread` — moisture falloff from a water source.
//!
//! Worker-03 / WORLD pure core, round 17. Distance from water (river/lake/sea)
//! into a [0,1] moisture value via an exponential falloff scaled by the source's
//! strength: highest at the water's edge, fading to zero far inland. Feeds biome
//! selection and `vegetation_scatter` (R12).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const FALLOFF: f32 = 16.0; // distance scale of the decay

/// Moisture in [0,1] at `dist` from a source of `source_strength` (0..1).
/// Maximal at the source, decaying with distance, scaling with strength.
pub fn moisture(dist: f32, source_strength: f32) -> f32 {
    let s = clamp01(source_strength);
    let d = dist.max(0.0);
    clamp01(s * (-d / FALLOFF).exp())
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximal_at_source() {
        assert!((moisture(0.0, 1.0) - 1.0).abs() < 1e-6);
        // At the source, moisture equals the strength.
        assert!((moisture(0.0, 0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn far_goes_to_zero() {
        assert!(moisture(1000.0, 1.0) < 1e-3);
    }

    #[test]
    fn monotone_decreasing() {
        let mut prev = f32::INFINITY;
        for d in 0..200 {
            let m = moisture(d as f32, 0.9);
            assert!(m <= prev + 1e-6);
            prev = m;
        }
    }

    #[test]
    fn strength_scales() {
        assert!(moisture(10.0, 0.8) > moisture(10.0, 0.2));
    }

    #[test]
    fn bounded() {
        for d in 0..100 {
            for si in 0..=10 {
                let m = moisture(d as f32, si as f32 / 10.0);
                assert!(m >= 0.0 && m <= 1.0);
            }
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(moisture(7.0, 0.6), moisture(7.0, 0.6));
    }
}
