//! bloom_math.rs — pure emissive → tonemap → bloom interaction.
//!
//! Makes glow predictable: a soft-knee prefilter decides how much an emissive
//! value contributes to bloom, and a combined tonemap+bloom keeps the result in
//! `[0,1]` (no blow-out). Pairs with `tonemap_lut` (R2) and the emissive core.
//! Pure, deterministic, panic-free (R1/R11/R21). All functions are finite for
//! any finite input (and NaN-safe, since `f32::min/max` drop NaN).

/// Soft-knee bloom prefilter: how much of `luma` survives a threshold with a
/// `knee`-wide quadratic ramp. Returns 0 below `threshold - knee`, the excess
/// `luma - threshold` above `threshold + knee`, and a C1-continuous quadratic
/// blend between. Monotonically non-decreasing in `luma`.
pub fn soft_threshold(luma: f32, threshold: f32, knee: f32) -> f32 {
    let l = luma.max(0.0).min(f32::MAX); // clamp + NaN-sanitize
    let t = threshold.max(0.0);
    let k = knee.max(0.0);
    if k <= 0.0 {
        return (l - t).max(0.0); // hard knee
    }
    let lower = t - k;
    if l <= lower {
        0.0
    } else if l >= t + k {
        l - t
    } else {
        let x = l - lower; // in [0, 2k]
        (x * x) / (4.0 * k) // 0 at x=0, == k at x=2k (continuous with l-t)
    }
}

/// Bloom contribution of an emissive value at a given exposure, after the
/// soft-knee threshold. Monotone in both `emissive_intensity` and `exposure`.
pub fn bloom_contrib(emissive_intensity: f32, exposure: f32, threshold: f32, knee: f32) -> f32 {
    let luma = (emissive_intensity.max(0.0) * exposure.max(0.0)).min(f32::MAX);
    soft_threshold(luma, threshold, knee)
}

/// Tonemap a colour (exposure + Reinhard, per channel) and add a tonemapped
/// bloom glow from `emissive`. Output channels are always in `[0,1]` — the glow
/// can brighten but never blow out. Uses fixed prefilter `threshold=1.0`,
/// `knee=0.5`.
pub fn tonemap_then_bloom(color: [f32; 3], emissive: f32, exposure: f32) -> [f32; 3] {
    let exp = exposure.max(0.0);
    let tm = |c: f32| {
        let x = (c.max(0.0) * exp).min(f32::MAX);
        x / (1.0 + x) // Reinhard -> [0,1)
    };
    let base = [tm(color[0]), tm(color[1]), tm(color[2])];
    let b = bloom_contrib(emissive, exp, 1.0, 0.5);
    let bt = b / (1.0 + b); // tonemap the bloom too -> [0,1)
    [
        (base[0] + bt).min(1.0),
        (base[1] + bt).min(1.0),
        (base[2] + bt).min(1.0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_threshold_is_zero() {
        // hard knee: anything <= threshold contributes nothing
        assert_eq!(soft_threshold(0.5, 1.0, 0.0), 0.0);
        assert_eq!(soft_threshold(1.0, 1.0, 0.0), 0.0);
        // soft knee: below the knee floor (threshold - knee) is still zero
        assert_eq!(soft_threshold(0.4, 1.0, 0.5), 0.0);
        assert_eq!(bloom_contrib(0.1, 1.0, 1.0, 0.5), 0.0);
    }

    #[test]
    fn prop_monotonic_in_intensity() {
        let mut prev = -1.0f32;
        let mut e = 0.0f32;
        while e <= 50.0 {
            let c = bloom_contrib(e, 1.0, 1.0, 0.5);
            assert!(c >= prev - 1e-6, "not monotone at e={} ({} < {})", e, c, prev);
            prev = c;
            e += 0.1;
        }
    }

    #[test]
    fn prop_no_nan_inf_for_finite() {
        let vals = [0.0f32, 0.5, 1.0, 2.0, 1e3, 1e20, 3.4e38, -5.0];
        for &l in &vals {
            for &t in &[0.0f32, 1.0, 2.0] {
                for &k in &[0.0f32, 0.5, 2.0] {
                    assert!(soft_threshold(l, t, k).is_finite(), "st {} {} {}", l, t, k);
                }
            }
            for &exp in &vals {
                assert!(bloom_contrib(l, exp, 1.0, 0.5).is_finite(), "bc {} {}", l, exp);
                let o = tonemap_then_bloom([l, l * 0.5, l + 1.0], l, exp);
                for c in o {
                    assert!(c.is_finite(), "tmb {} {}", l, exp);
                }
            }
        }
    }

    #[test]
    fn knee_is_continuous_at_threshold() {
        // No jump as we sweep across the knee region.
        let (t, k) = (1.0f32, 0.5f32);
        let step = 0.005f32;
        let mut l = t - 2.0 * k;
        let mut prev = soft_threshold(l, t, k);
        while l <= t + 2.0 * k {
            let cur = soft_threshold(l, t, k);
            assert!((cur - prev).abs() <= step * 1.5 + 1e-5, "jump at l={}", l);
            prev = cur;
            l += step;
        }
    }

    #[test]
    fn exposure_scales_contribution() {
        // For an emissive above threshold, more exposure => more bloom.
        let lo = bloom_contrib(2.0, 0.5, 1.0, 0.5);
        let hi = bloom_contrib(2.0, 2.0, 1.0, 0.5);
        assert!(hi > lo, "exposure did not scale contribution ({} !> {})", hi, lo);
    }

    #[test]
    fn tonemap_no_blowout() {
        let mut seed = 0xB100_0001u32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / ((1u32 << 24) as f32)
        };
        for _ in 0..500 {
            let color = [lcg() * 100.0, lcg() * 100.0, lcg() * 100.0];
            let o = tonemap_then_bloom(color, lcg() * 1000.0, lcg() * 8.0);
            for c in o {
                assert!((0.0..=1.0).contains(&c), "blow-out: {}", c);
            }
        }
    }

    #[test]
    fn prop_deterministic() {
        assert_eq!(soft_threshold(1.3, 1.0, 0.5), soft_threshold(1.3, 1.0, 0.5));
        assert_eq!(
            tonemap_then_bloom([0.4, 0.6, 0.8], 3.0, 1.5),
            tonemap_then_bloom([0.4, 0.6, 0.8], 3.0, 1.5)
        );
    }
}
