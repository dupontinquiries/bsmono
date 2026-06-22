//! lod_crossfade.rs — pure LOD crossfade weight across a transition band.
//!
//! A smoothstep blend from LOD A (0) to LOD B (1) across `[band_lo, band_hi]`,
//! so swapping LODs dithers/cross-dissolves instead of popping. Pure,
//! deterministic, panic-free (R1/R11/R21).

/// Crossfade weight at `dist`: `0` (full LOD A) below `band_lo`, `1` (full LOD B)
/// above `band_hi`, smoothstep between. `band_hi` is guarded to exceed
/// `band_lo`. Result in `[0,1]`.
pub fn fade(dist: f32, band_lo: f32, band_hi: f32) -> f32 {
    let lo = band_lo;
    let hi = band_hi.max(lo + 1e-4); // guard hi > lo
    if dist <= lo {
        0.0
    } else if dist >= hi {
        1.0
    } else {
        let t = (dist - lo) / (hi - lo); // [0,1]
        (t * t * (3.0 - 2.0 * t)).max(0.0).min(1.0) // smoothstep
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_lo_is_zero() {
        assert_eq!(fade(5.0, 10.0, 20.0), 0.0);
        assert_eq!(fade(10.0, 10.0, 20.0), 0.0);
    }

    #[test]
    fn above_hi_is_one() {
        assert_eq!(fade(20.0, 10.0, 20.0), 1.0);
        assert_eq!(fade(100.0, 10.0, 20.0), 1.0);
    }

    #[test]
    fn smooth_between() {
        let mid = fade(15.0, 10.0, 20.0);
        assert!((mid - 0.5).abs() < 1e-5, "midpoint not 0.5: {}", mid);
        // monotone increasing across the band
        let mut prev = -1.0f32;
        let mut d = 10.0f32;
        while d <= 20.0 {
            let f = fade(d, 10.0, 20.0);
            assert!(f >= prev - 1e-6, "not monotone at {}", d);
            prev = f;
            d += 0.1;
        }
    }

    #[test]
    fn clamped_unit_range() {
        let mut seed = 0x1817_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let f = fade(lcg() * 100.0, lcg() * 50.0, lcg() * 50.0 + 25.0);
            assert!((0.0..=1.0).contains(&f), "fade oob {}", f);
        }
    }

    #[test]
    fn hi_greater_than_lo_guard() {
        // hi <= lo must not divide by zero
        let f = fade(15.0, 20.0, 10.0);
        assert!(f.is_finite() && (0.0..=1.0).contains(&f));
    }

    #[test]
    fn deterministic() {
        assert_eq!(fade(13.0, 10.0, 20.0), fade(13.0, 10.0, 20.0));
    }
}
