//! `strata` — layered geology: rock tier by depth + noise.
//!
//! Worker-03 / WORLD pure core, round 16. Maps a depth (and a noise value that
//! waves the layer boundaries) onto a rock-tier index via ascending depth
//! `bands`. The noise perturbs the boundary so strata undulate rather than
//! sitting at dead-flat depths. Pairs with `ore_distribution` (R9): the stratum
//! tells you which rock, ore tells you what's hidden in it.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// How far (in depth units) the noise can wave a band boundary.
const BOUNDARY_JITTER: f32 = 2.0;

/// Stratum index for a cell at `depth`, with a per-cell `noise` value (≈[0,1])
/// that perturbs boundaries, against ascending depth `bands`. Returns the count
/// of bands the (perturbed) depth has passed, clamped to a `u8`. Empty bands
/// → 0.
pub fn stratum(depth: f32, noise: f32, bands: &[f32]) -> u8 {
    if bands.is_empty() {
        return 0;
    }
    let jitter = (clamp01(noise) * 2.0 - 1.0) * BOUNDARY_JITTER; // [-J, J]
    let d = depth + jitter;
    let mut idx: u32 = 0;
    for &b in bands {
        if d >= b {
            idx += 1;
        } else {
            break;
        }
    }
    idx.min(255) as u8
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    const BANDS: [f32; 4] = [8.0, 24.0, 56.0, 120.0];

    #[test]
    fn shallow_and_deep_differ() {
        let shallow = stratum(2.0, 0.5, &BANDS);
        let deep = stratum(100.0, 0.5, &BANDS);
        assert_ne!(shallow, deep);
        assert!(deep > shallow);
    }

    #[test]
    fn band_boundaries_map() {
        // noise 0.5 → zero jitter → clean boundaries.
        assert_eq!(stratum(0.0, 0.5, &BANDS), 0);
        assert_eq!(stratum(8.0, 0.5, &BANDS), 1);
        assert_eq!(stratum(24.0, 0.5, &BANDS), 2);
        assert_eq!(stratum(56.0, 0.5, &BANDS), 3);
        assert_eq!(stratum(120.0, 0.5, &BANDS), 4);
    }

    #[test]
    fn monotone_in_depth() {
        let mut prev = 0u8;
        for d in 0..200 {
            let s = stratum(d as f32, 0.5, &BANDS); // fixed noise
            assert!(s >= prev, "stratum decreased with depth");
            prev = s;
        }
    }

    #[test]
    fn index_bounded() {
        for d in 0..300 {
            let s = stratum(d as f32, 0.9, &BANDS);
            assert!(s as usize <= BANDS.len());
        }
    }

    #[test]
    fn empty_bands_safe() {
        assert_eq!(stratum(50.0, 0.3, &[]), 0);
        assert_eq!(stratum(-5.0, 0.0, &[]), 0);
    }

    #[test]
    fn deterministic() {
        assert_eq!(stratum(40.0, 0.2, &BANDS), stratum(40.0, 0.2, &BANDS));
    }
}
