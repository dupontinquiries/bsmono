//! `batch_erosion_advect` — batched sediment-advection amounts (perf bonus).
//!
//! Worker-03 / WORLD pure core, round 24. Computes the advected-out amount for
//! many cells in one call, producing **bit-identical** values to the scalar
//! reference (a verbatim mirror of R23 `erosion_advect`'s per-cell rule: a cell
//! moves a `clamp01(rate)` fraction of its suspended sediment downstream).
//!
//! PERF: the advection step touches every cell of a region's sediment field
//! each tick. The amount-leaving computation (`suspended · clamp01(rate)`) is
//! the hot inner term; batching it over a contiguous slice with one shared,
//! pre-clamped rate keeps it branch-light. Because each cell runs the exact
//! scalar `advect_one`, the moved amounts match the grid stepper bit-for-bit —
//! mass routed by a batched path equals a scalar fallback.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Scalar reference — the per-cell amount that leaves a cell in R23
/// `erosion_advect` (`amount = suspended · clamp01(rate)`; non-positive ⇒ 0).
pub fn advect_one(suspended: f32, rate: f32) -> f32 {
    let r = clamp01(rate);
    let amount = suspended * r;
    if amount <= 0.0 {
        0.0
    } else {
        amount
    }
}

/// Batched advected-out amounts over a `suspended` slice sharing `rate`.
/// Bit-identical to calling `advect_one` per cell.
pub fn advect_batch(suspended: &[f32], rate: f32) -> Vec<f32> {
    suspended.iter().map(|&s| advect_one(s, rate)).collect()
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_is_bit_parity_with_scalar() {
        let s = [0.0f32, 10.0, 2.5, 100.0, -3.0];
        let batch = advect_batch(&s, 0.4);
        for (i, &v) in s.iter().enumerate() {
            assert_eq!(batch[i].to_bits(), advect_one(v, 0.4).to_bits(), "bit drift at {i}");
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(advect_batch(&[], 0.5).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let one = advect_one(10.0, 0.3);
        assert_eq!(advect_batch(&[10.0], 0.3), vec![one]);
    }

    #[test]
    fn mass_consistent() {
        // The amount leaving never exceeds what's suspended (rate clamped ≤ 1).
        for &s in &[0.0f32, 1.0, 50.0, 1000.0] {
            assert!(advect_one(s, 2.0) <= s + 1e-6); // rate>1 clamps to 1
            assert!(advect_one(s, 0.5) <= s + 1e-6);
        }
    }

    #[test]
    fn deterministic() {
        let s = [1.0f32, 2.0, 3.0];
        assert_eq!(advect_batch(&s, 0.5), advect_batch(&s, 0.5));
    }

    #[test]
    fn nonpositive_moves_nothing_parity() {
        let s = [0.0f32, -5.0];
        let b = advect_batch(&s, 0.5);
        assert!(b.iter().all(|&v| v == 0.0));
        assert_eq!(b[0], advect_one(0.0, 0.5));
    }
}
