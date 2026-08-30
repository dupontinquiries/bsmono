//! kernel.rs — the f64 vector kernels, written so the compiler can vectorise them and so the
//! *ordering* is ours rather than the optimiser's.
//!
//! Two things worth stating plainly, because they are routinely confused:
//!
//! **SIMD does not reduce precision.** AArch64 NEON `float64x2_t` is IEEE double, two lanes wide.
//! Elementwise operations are bit-identical to scalar. Only *reductions* differ, and only because
//! splitting into lanes changes the summation order — a different rounding, not a worse one. What
//! would genuinely cost precision is `-ffast-math`-style reassociation, which this workspace never
//! enables (see the release profile).
//!
//! **We do not hand-write intrinsics here.** The kernels are shaped so LLVM's auto-vectoriser
//! produces the NEON we want (fixed lane count, no aliasing, no early exits), and the lane count
//! is expressed structurally through `LANES` so the reduction tree is a property of the source
//! rather than of the target. That keeps one implementation correct everywhere — including
//! lavapipe-style CI machines and x86 — instead of an aarch64 path that only some of us can test.
//! If profiling later shows LLVM missing something, intrinsics go behind `cfg(target_arch)` with
//! this scalar version as the differential oracle.

use crate::sum::{sum_neumaier, sum_pairwise, two_product, two_sum};

/// f64 lanes in a 128-bit NEON register. The reduction tree is written around this constant so
/// the arithmetic ordering is fixed by the source, not discovered by the backend.
pub const LANES: usize = 2;

/// Dot product, fast path. Lane-parallel accumulation then a fixed-order combine.
///
/// Error grows as O(n·ε) within a lane, which is fine for well-conditioned data. When the dot
/// product decides convergence — residual norms, CG's alpha and beta — use `dot_compensated`.
pub fn dot(a: &[f64], b: &[f64]) -> f64 {
    assert_eq!(a.len(), b.len(), "dot: length mismatch");
    let n = a.len();
    let mut acc = [0.0f64; LANES];
    let chunks = n / LANES;
    for c in 0..chunks {
        let base = c * LANES;
        // Independent accumulators: no loop-carried dependency between lanes, which is what
        // lets this become a pair of NEON FMAs.
        for l in 0..LANES {
            acc[l] = f64::mul_add(a[base + l], b[base + l], acc[l]);
        }
    }
    let mut tail = 0.0;
    for i in chunks * LANES..n {
        tail = f64::mul_add(a[i], b[i], tail);
    }
    // Fixed combine order: lane 0, lane 1, ..., then the tail.
    sum_pairwise(&acc) + tail
}

/// Dot product with compensated accumulation: error independent of `n`, and exact when the
/// products happen to be representable.
///
/// This is the one to use for residual norms. A Newton step is only as trustworthy as the number
/// it compares against the tolerance, and a residual near convergence is precisely where terms
/// cancel and naive accumulation reports noise.
pub fn dot_compensated(a: &[f64], b: &[f64]) -> f64 {
    assert_eq!(a.len(), b.len(), "dot_compensated: length mismatch");
    let mut sum = 0.0f64;
    let mut comp = 0.0f64;
    for i in 0..a.len() {
        // Each product is split into its exact high and low parts, so the multiplication's own
        // rounding error is carried rather than discarded.
        let (p, pe) = two_product(a[i], b[i]);
        let (s, se) = two_sum(sum, p);
        sum = s;
        comp += se + pe;
    }
    sum + comp
}

/// y += alpha * x, elementwise. Bit-identical to the scalar loop; FMA makes it *more* accurate
/// than separate multiply-then-add, not less.
pub fn axpy(alpha: f64, x: &[f64], y: &mut [f64]) {
    assert_eq!(x.len(), y.len(), "axpy: length mismatch");
    for i in 0..x.len() {
        y[i] = f64::mul_add(alpha, x[i], y[i]);
    }
}

pub fn scale(alpha: f64, x: &mut [f64]) {
    for v in x.iter_mut() {
        *v *= alpha;
    }
}

/// Euclidean norm, computed to avoid the overflow and underflow that a naive sqrt(sum of squares)
/// suffers when components are near the exponent limits.
pub fn norm2(x: &[f64]) -> f64 {
    let scale = x.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    if scale == 0.0 || !scale.is_finite() {
        return if scale.is_finite() { 0.0 } else { f64::INFINITY };
    }
    let inv = 1.0 / scale;
    let squares: Vec<f64> = x.iter().map(|v| (v * inv) * (v * inv)).collect();
    scale * sum_neumaier(&squares).sqrt()
}

/// Maximum absolute value — the infinity norm. This is the mismatch measure power flow converges
/// on, because a per-bus tolerance is physically meaningful in a way an aggregate norm is not:
/// one bad bus in a thousand should fail the test, and in a 2-norm it would average away.
pub fn norm_inf(x: &[f64]) -> f64 {
    x.iter().fold(0.0f64, |m, v| m.max(v.abs()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sum::sum_exact;

    #[test]
    fn dot_matches_an_exactly_computed_reference() {
        let a: Vec<f64> = (0..1000).map(|i| (i as f64 * 0.37).cos()).collect();
        let b: Vec<f64> = (0..1000).map(|i| (i as f64 * 0.11).sin()).collect();
        let products: Vec<f64> = a.iter().zip(&b).map(|(x, y)| x * y).collect();
        let truth = sum_exact(&products);
        assert!((dot(&a, &b) - truth).abs() < 1e-12);
        assert!((dot_compensated(&a, &b) - truth).abs() < 1e-14);
    }

    #[test]
    fn compensation_wins_where_the_dot_product_cancels() {
        // Nearly orthogonal vectors with large components: the result is tiny, the terms are not.
        let n = 4096;
        let mut a = vec![0.0; n];
        let mut b = vec![0.0; n];
        for i in 0..n {
            let s = if i % 2 == 0 { 1.0 } else { -1.0 };
            a[i] = 1.0e8 * s + (i as f64);
            b[i] = 1.0e8 * s - (i as f64);
        }
        let products: Vec<f64> = a.iter().zip(&b).map(|(x, y)| x * y).collect();
        let truth = sum_exact(&products);
        let plain = dot(&a, &b);
        let comp = dot_compensated(&a, &b);
        assert!(
            (comp - truth).abs() <= (plain - truth).abs(),
            "compensated {comp} should be no worse than plain {plain}, truth {truth}"
        );
    }

    #[test]
    fn dot_is_reproducible_bit_for_bit() {
        let a: Vec<f64> = (0..3333).map(|i| (i as f64).sqrt()).collect();
        let b: Vec<f64> = (0..3333).map(|i| 1.0 / (1.0 + i as f64)).collect();
        let first = dot(&a, &b).to_bits();
        for _ in 0..16 {
            assert_eq!(dot(&a, &b).to_bits(), first);
        }
    }

    #[test]
    fn dot_handles_lengths_that_are_not_a_multiple_of_the_lane_count() {
        // The tail loop is where lane-blocked kernels break. Check every residue.
        for n in 0..=17 {
            let a: Vec<f64> = (0..n).map(|i| 1.0 + i as f64).collect();
            let b: Vec<f64> = (0..n).map(|i| 2.0 + i as f64).collect();
            let truth: f64 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
            assert!((dot(&a, &b) - truth).abs() < 1e-9, "n={n}");
        }
    }

    #[test]
    fn norm2_survives_components_that_would_overflow_when_squared() {
        // 1e200 squared is inf. A naive norm returns inf; this one must not.
        let x = vec![1.0e200, 1.0e200];
        let got = norm2(&x);
        assert!(got.is_finite(), "norm2 overflowed: {got}");
        assert!((got / (1.0e200 * std::f64::consts::SQRT_2) - 1.0).abs() < 1e-12);
        // And the same at the other end: 1e-200 squared is zero.
        let y = vec![1.0e-200, 1.0e-200];
        assert!(norm2(&y) > 0.0, "norm2 underflowed to zero");
    }

    #[test]
    fn axpy_and_norms_agree_with_the_obvious_definitions() {
        let x = vec![1.0, -2.0, 3.0];
        let mut y = vec![10.0, 10.0, 10.0];
        axpy(2.0, &x, &mut y);
        assert_eq!(y, vec![12.0, 6.0, 16.0]);
        assert_eq!(norm_inf(&x), 3.0);
        assert!((norm2(&x) - 14.0f64.sqrt()).abs() < 1e-15);
        assert_eq!(norm_inf(&[]), 0.0);
    }
}
