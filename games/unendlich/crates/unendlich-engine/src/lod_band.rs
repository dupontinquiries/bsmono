//! lod_band.rs — pure entity LOD band selector (the entity LOD pass).
//!
//! Picks which LOD band a distant/large entity falls into from its distance,
//! world size and a quality multiplier, against a set of distance thresholds;
//! and reports the minimum part half-size kept at a band (coarser parts farther
//! out). Pure, deterministic, panic-free (R1/R11/R21).

/// Choose a LOD band for an entity. `bands` are ascending distance thresholds.
/// Larger `size` and higher `quality` both let an entity keep detail longer
/// (they shrink the effective distance). The result is clamped to
/// `bands.len()-1`; an empty `bands` slice yields band 0.
pub fn band(dist: f32, size: f32, quality: f32, bands: &[f32]) -> usize {
    if bands.is_empty() {
        return 0;
    }
    let eff = dist.max(0.0) / (size.max(1e-3) * quality.max(1e-3));
    let mut b = 0usize;
    for &t in bands {
        if eff >= t {
            b += 1;
        }
    }
    b.min(bands.len() - 1)
}

/// Minimum part half-extent kept at `band`: parts smaller than this are dropped.
/// Grows with band (coarser at far bands): `base * (band + 1)`.
pub fn min_part_half(band: usize, base: f32) -> f32 {
    base.max(0.0) * (band as f32 + 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BANDS: &[f32] = &[10.0, 40.0, 120.0, 300.0];

    #[test]
    fn nearer_is_lower_or_equal_band() {
        let mut prev = 0usize;
        let mut d = 0.0f32;
        while d <= 2000.0 {
            let b = band(d, 1.0, 1.0, BANDS);
            assert!(b >= prev, "band decreased with distance at d={}", d);
            assert!(b <= BANDS.len() - 1);
            prev = b;
            d += 5.0;
        }
    }

    #[test]
    fn bigger_size_sees_detail_longer() {
        // at the same distance, a larger entity is in a lower (more detailed) band
        for &d in &[20.0f32, 80.0, 200.0, 500.0] {
            let small = band(d, 1.0, 1.0, BANDS);
            let big = band(d, 5.0, 1.0, BANDS);
            assert!(big <= small, "bigger size not <= at d={} ({} > {})", d, big, small);
        }
    }

    #[test]
    fn higher_quality_sees_detail_longer() {
        for &d in &[20.0f32, 80.0, 200.0, 500.0] {
            let lo = band(d, 1.0, 0.5, BANDS);
            let hi = band(d, 1.0, 2.0, BANDS);
            assert!(hi <= lo, "higher quality not <= at d={}", d);
        }
    }

    #[test]
    fn clamps_to_last_band() {
        assert_eq!(band(1e9, 1.0, 1.0, BANDS), BANDS.len() - 1);
        assert_eq!(band(50.0, 1.0, 1.0, &[]), 0, "empty bands -> 0");
    }

    #[test]
    fn min_part_half_grows_with_band() {
        let mut prev = -1.0f32;
        for b in 0..8usize {
            let m = min_part_half(b, 0.05);
            assert!(m > prev, "min_part_half not growing at band {}", b);
            prev = m;
        }
        assert_eq!(min_part_half(0, 0.05), 0.05);
    }

    #[test]
    fn deterministic() {
        assert_eq!(band(123.0, 2.0, 1.5, BANDS), band(123.0, 2.0, 1.5, BANDS));
        assert_eq!(min_part_half(3, 0.1), min_part_half(3, 0.1));
    }
}
