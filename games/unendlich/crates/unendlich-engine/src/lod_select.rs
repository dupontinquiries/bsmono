//! lod_select.rs — pure LOD index from distance + fidelity.
//!
//! Counts how many ascending distance bands a *fidelity-scaled* distance clears,
//! clamped to the last band. Higher fidelity divides the distance, so detail is
//! kept farther out. Pairs with `lod_band` (R9). Pure, deterministic, panic-free
//! (R1/R11/R21).

/// LOD index for `dist` against ascending `bands`, scaled by `fidelity` (higher
/// = lower index at a given distance). Clamped to `bands.len()-1`; empty `bands`
/// yields `0`.
pub fn lod_index(dist: f32, bands: &[f32], fidelity: f32) -> u32 {
    if bands.is_empty() {
        return 0;
    }
    let eff = dist.max(0.0) / fidelity.max(1e-3);
    let mut b = 0u32;
    for &t in bands {
        if eff >= t {
            b += 1;
        }
    }
    b.min(bands.len() as u32 - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    const BANDS: &[f32] = &[10.0, 40.0, 120.0, 300.0];

    #[test]
    fn near_is_zero() {
        assert_eq!(lod_index(0.0, BANDS, 1.0), 0);
        assert_eq!(lod_index(5.0, BANDS, 1.0), 0);
    }

    #[test]
    fn crosses_bands() {
        assert_eq!(lod_index(15.0, BANDS, 1.0), 1);
        assert_eq!(lod_index(50.0, BANDS, 1.0), 2);
        assert_eq!(lod_index(150.0, BANDS, 1.0), 3);
    }

    #[test]
    fn higher_fidelity_pushes_bands_out() {
        for &d in &[15.0f32, 50.0, 150.0, 400.0] {
            assert!(lod_index(d, BANDS, 2.0) <= lod_index(d, BANDS, 1.0), "fidelity didn't lower index at {}", d);
        }
    }

    #[test]
    fn clamps_to_last() {
        assert_eq!(lod_index(1e9, BANDS, 1.0), BANDS.len() as u32 - 1);
        assert_eq!(lod_index(50.0, &[], 1.0), 0);
    }

    #[test]
    fn monotone_in_distance() {
        let mut prev = 0u32;
        let mut d = 0.0f32;
        while d <= 1000.0 {
            let i = lod_index(d, BANDS, 1.0);
            assert!(i >= prev, "index dropped at {}", d);
            prev = i;
            d += 2.0;
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(lod_index(77.0, BANDS, 1.5), lod_index(77.0, BANDS, 1.5));
    }
}
