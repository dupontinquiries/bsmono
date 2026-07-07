//! `batch_cliff_detect` — batched cliff classification (perf bonus).
//!
//! Worker-03 / WORLD pure core, round 20. Classifies many cells' gradients in
//! one call against a shared threshold, producing **identical** results to the
//! scalar reference (a verbatim mirror of R18 `cliff_detect`).
//!
//! PERF: cliff/material classification runs per surface cell during meshing
//! (CHUNK_SIZE² per chunk). Batching shares the threshold and walks a contiguous
//! gradient slice; because each cell runs the exact scalar `cliff_one`, the
//! boolean results are identical to per-cell calls — a batched fast path can't
//! disagree with a scalar fallback on which cells are bare rock.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Steepness = gradient magnitude (mirror of R18).
pub fn steepness(dh_dx: f32, dh_dz: f32) -> f32 {
    (dh_dx * dh_dx + dh_dz * dh_dz).sqrt()
}

/// Scalar reference — identical to `cliff_detect::is_cliff` (R18).
pub fn cliff_one(dh_dx: f32, dh_dz: f32, thresh: f32) -> bool {
    steepness(dh_dx, dh_dz) > thresh.max(0.0)
}

/// Batched cliff test over `(dh_dx, dh_dz)` gradients sharing `thresh`.
/// Identical to calling `cliff_one` per gradient.
pub fn cliff_batch(grads: &[(f32, f32)], thresh: f32) -> Vec<bool> {
    grads
        .iter()
        .map(|&(dx, dz)| cliff_one(dx, dz, thresh))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_matches_scalar() {
        let g = [
            (0.0f32, 0.0),
            (5.0, 0.0),
            (3.0, 4.0),
            (-2.0, 1.0),
            (0.1, 0.1),
        ];
        let batch = cliff_batch(&g, 2.0);
        for (i, &(dx, dz)) in g.iter().enumerate() {
            assert_eq!(batch[i], cliff_one(dx, dz, 2.0), "drift at {i}");
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(cliff_batch(&[], 1.0).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let one = cliff_one(3.0, 4.0, 2.0);
        assert_eq!(cliff_batch(&[(3.0, 4.0)], 2.0), vec![one]);
    }

    #[test]
    fn bounded_is_boolean() {
        // (trivially bounded — bool; check both outcomes occur)
        let b = cliff_batch(&[(0.0, 0.0), (9.0, 9.0)], 1.0);
        assert_eq!(b, vec![false, true]);
    }

    #[test]
    fn deterministic() {
        let g = [(2.0f32, 2.0), (2.0, 2.0)];
        assert_eq!(cliff_batch(&g, 1.0), cliff_batch(&g, 1.0));
    }

    #[test]
    fn threshold_shared_across_batch() {
        let g = [(3.0f32, 0.0), (3.0, 0.0)];
        assert_eq!(cliff_batch(&g, 2.0), vec![true, true]);
        assert_eq!(cliff_batch(&g, 3.0), vec![false, false]); // 3 > 3 is false
    }
}
