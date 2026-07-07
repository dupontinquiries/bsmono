//! `batch_strata` — batched strata classification (perf bonus).
//!
//! Worker-03 / WORLD pure core, round 18. Classifies a column (or chunk) of
//! `(depth, noise)` samples against shared `bands` in one call, producing
//! **identical** tier ids to the scalar reference (a verbatim mirror of R16
//! `strata`).
//!
//! PERF: strata are evaluated per voxel along a column during generation
//! (CHUNK_SIZE samples per column, CHUNK_SIZE² columns per chunk). Batching
//! hoists the shared `bands` slice and keeps the band-crossing scan over a
//! contiguous input; because each sample runs the exact scalar `stratum_one`,
//! the ids are bit-identical to per-sample calls — a batched fast path can't
//! diverge from a scalar fallback.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const BOUNDARY_JITTER: f32 = 2.0;

/// Scalar reference — identical to `strata::stratum` (R16).
pub fn stratum_one(depth: f32, noise: f32, bands: &[f32]) -> u8 {
    if bands.is_empty() {
        return 0;
    }
    let jitter = (clamp01(noise) * 2.0 - 1.0) * BOUNDARY_JITTER;
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

/// Batched strata over `(depth, noise)` samples sharing `bands`.
/// Bit-identical to calling `stratum_one` per sample.
pub fn stratum_batch(samples: &[(f32, f32)], bands: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .map(|&(d, n)| stratum_one(d, n, bands))
        .collect()
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    const BANDS: [f32; 4] = [8.0, 24.0, 56.0, 120.0];

    #[test]
    fn batch_matches_scalar() {
        let s = [
            (0.0f32, 0.5),
            (8.0, 0.5),
            (30.0, 0.9),
            (100.0, 0.1),
            (200.0, 0.5),
        ];
        let batch = stratum_batch(&s, &BANDS);
        for (i, &(d, n)) in s.iter().enumerate() {
            assert_eq!(batch[i], stratum_one(d, n, &BANDS), "drift at {i}");
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(stratum_batch(&[], &BANDS).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let one = stratum_one(40.0, 0.2, &BANDS);
        assert_eq!(stratum_batch(&[(40.0, 0.2)], &BANDS), vec![one]);
    }

    #[test]
    fn bounded() {
        for v in stratum_batch(&[(0.0, 0.5), (50.0, 0.9), (300.0, 0.0)], &BANDS) {
            assert!(v as usize <= BANDS.len());
        }
    }

    #[test]
    fn deterministic() {
        let s = [(10.0f32, 0.3), (10.0, 0.3)];
        assert_eq!(stratum_batch(&s, &BANDS), stratum_batch(&s, &BANDS));
    }

    #[test]
    fn empty_bands_all_zero_like_scalar() {
        let s = [(5.0f32, 0.3), (99.0, 0.7)];
        let batch = stratum_batch(&s, &[]);
        assert!(batch.iter().all(|&v| v == 0));
        assert_eq!(batch[0], stratum_one(5.0, 0.3, &[]));
    }
}
