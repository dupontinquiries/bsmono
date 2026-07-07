//! tonemap_curve.rs — pure ACES-ish luminance tonemap.
//!
//! The Narkowicz ACES filmic approximation with an exposure multiplier, mapping
//! `[0,∞)` HDR luminance into `[0,1]`. Pairs with `tonemap.glsl`. Pure,
//! deterministic, panic-free (R1/R11/R21).

/// Tonemap `x` (linear HDR luminance) at `exposure`. `0 → 0`, large `→ 1`,
/// monotonically increasing, clamped to `[0,1]`.
pub fn tonemap(x: f32, exposure: f32) -> f32 {
    let v = x.max(0.0) * exposure.max(0.0);
    // Narkowicz ACES filmic approximation
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    let tm = (v * (a * v + b)) / (v * (c * v + d) + e);
    crate::util::mathx::saturate(tm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_zero() {
        assert_eq!(tonemap(0.0, 1.0), 0.0);
    }

    #[test]
    fn large_approaches_one() {
        assert!((tonemap(1e6, 1.0) - 1.0).abs() < 1e-3, "large should saturate to 1");
    }

    #[test]
    fn monotone_in_x() {
        let mut prev = -1.0f32;
        let mut x = 0.0f32;
        while x <= 20.0 {
            let t = tonemap(x, 1.0);
            assert!(t >= prev - 1e-6, "not monotone at x={}", x);
            assert!((0.0..=1.0).contains(&t));
            prev = t;
            x += 0.02;
        }
    }

    #[test]
    fn exposure_scales() {
        // more exposure brightens a mid value (monotone in exposure)
        assert!(tonemap(0.5, 2.0) > tonemap(0.5, 0.5));
    }

    #[test]
    fn clamped_unit_range() {
        let mut seed = 0x2317_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let t = tonemap(lcg() * 100.0 - 10.0, lcg() * 8.0);
            assert!((0.0..=1.0).contains(&t), "tonemap oob {}", t);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(tonemap(0.7, 1.5), tonemap(0.7, 1.5));
    }
}
