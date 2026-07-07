//! lens_dirt.rs — pure bloom-modulated lens-dirt intensity.
//!
//! Lens-dirt brightness = dirt mask × bloom × gain, so the dirt texture only
//! shows where bright bloom hits it. Pure, deterministic, panic-free
//! (R1/R11/R21). Result in `[0,1]`.

/// Lens-dirt intensity for dirt `mask`, incident `bloom`, and `gain`. `0` with no
/// bloom or no mask; rises with all three; clamped to `[0,1]`.
pub fn dirt(mask: f32, bloom: f32, gain: f32) -> f32 {
    crate::util::mathx::saturate(mask.max(0.0) * bloom.max(0.0) * gain.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_bloom_is_zero() {
        assert_eq!(dirt(1.0, 0.0, 2.0), 0.0);
    }

    #[test]
    fn no_mask_is_zero() {
        assert_eq!(dirt(0.0, 1.0, 2.0), 0.0);
    }

    #[test]
    fn scales_with_inputs() {
        assert!(dirt(0.5, 0.5, 2.0) > dirt(0.5, 0.5, 1.0), "gain should scale");
        assert!(dirt(0.5, 0.8, 1.0) > dirt(0.5, 0.4, 1.0), "bloom should scale");
    }

    #[test]
    fn clamped_unit_range() {
        let mut seed = 0x2427_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let d = dirt(lcg() * 2.0 - 0.5, lcg() * 3.0, lcg() * 4.0);
            assert!((0.0..=1.0).contains(&d), "dirt oob {}", d);
        }
    }

    #[test]
    fn monotone_in_bloom() {
        let mut prev = -1.0f32;
        let mut b = 0.0f32;
        while b <= 2.0 {
            let d = dirt(0.5, b, 1.0);
            assert!(d >= prev - 1e-6, "not monotone at bloom={}", b);
            prev = d;
            b += 0.02;
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(dirt(0.5, 0.6, 1.2), dirt(0.5, 0.6, 1.2));
    }
}
