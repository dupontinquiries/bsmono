//! light_falloff.rs — pure point-light attenuation.
//!
//! Inverse-square-ish attenuation with a smooth window that reaches exactly 0 at
//! `radius`, so a point light has bounded influence with no hard seam. Pure,
//! deterministic, panic-free (R1/R11/R21). Result in `[0,1]`.

/// Attenuation at `dist` for a light of `radius`: `1` at the source, falling
/// (inverse-square-ish) to `0` at and beyond `radius`. `radius` guarded `> 0`.
pub fn atten(dist: f32, radius: f32) -> f32 {
    let r = radius.max(1e-4);
    let d = dist.max(0.0);
    if d >= r {
        return 0.0;
    }
    let inv = 1.0 / (1.0 + d * d); // inverse-square-ish core
    let w = (1.0 - (d / r).powi(4)).max(0.0); // smooth window -> 0 at r
    (inv * w * w).max(0.0).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_source_is_one() {
        assert!((atten(0.0, 10.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn past_radius_is_zero() {
        assert_eq!(atten(10.0, 10.0), 0.0);
        assert_eq!(atten(50.0, 10.0), 0.0);
    }

    #[test]
    fn monotone_decreasing() {
        let mut prev = 2.0f32;
        let mut d = 0.0f32;
        while d <= 10.0 {
            let a = atten(d, 10.0);
            assert!(a <= prev + 1e-6, "attenuation rose at d={}", d);
            prev = a;
            d += 0.05;
        }
    }

    #[test]
    fn clamped_unit_range() {
        let mut seed = 0x2217_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let a = atten(lcg() * 50.0 - 5.0, lcg() * 20.0);
            assert!((0.0..=1.0).contains(&a), "atten oob {}", a);
        }
    }

    #[test]
    fn radius_guard() {
        let a = atten(1.0, 0.0);
        assert!(a.is_finite() && (0.0..=1.0).contains(&a));
        let b = atten(0.0, -5.0);
        assert!(b.is_finite() && (0.0..=1.0).contains(&b));
    }

    #[test]
    fn deterministic() {
        assert_eq!(atten(3.0, 10.0), atten(3.0, 10.0));
    }
}
