//! sum.rs — reductions whose answer does not depend on how the work was split.
//!
//! Floating-point addition is not associative, so a sum has as many answers as it has
//! orderings. That is normally a rounding curiosity; here it is a correctness problem, because
//! this workspace makes two promises that both die if reductions drift:
//!
//!   * **the same input gives the same output**, run to run, thread count to thread count — the
//!     bit-parity discipline the rest of this monorepo already tests for; and
//!   * **the residual is trustworthy**, because Newton's convergence test compares it against a
//!     tolerance, and a residual that is mostly cancellation noise stops the iteration early or
//!     never.
//!
//! So every reduction here fixes its ordering by construction rather than inheriting whatever
//! the optimiser or the scheduler chose. Three tiers, in increasing cost:
//!
//! | tier | error growth | cost | use |
//! |---|---|---|---|
//! | `sum_pairwise` | O(log n · ε) | ~1× | default; vectorises cleanly |
//! | `sum_neumaier` | O(ε), independent of n | ~4× | residuals, dot products that decide convergence |
//! | `sum_exact` | correctly rounded | ~10× | verification only, never in a hot loop |
//!
//! Naive left-to-right accumulation is deliberately *not* offered. It grows error as O(n·ε) and
//! it is the thing everyone reaches for by accident.

/// Block size for pairwise summation. Chosen so a block fits comfortably in registers and is a
/// multiple of the NEON f64 lane count (2), which lets the block loop vectorise while the
/// *ordering* stays fixed by the block structure rather than by the vectoriser.
pub const BLOCK: usize = 64;

/// Pairwise (cascade) summation. Error grows as O(log n · ε) instead of O(n · ε), the ordering is
/// determined entirely by the input length, and it is identical on any machine.
pub fn sum_pairwise(xs: &[f64]) -> f64 {
    if xs.len() <= BLOCK {
        // Straight-line within a block: short enough that O(n·ε) over 64 terms is irrelevant, and
        // this is the shape LLVM will happily turn into NEON f64x2 accumulation.
        let mut acc = 0.0;
        for &x in xs {
            acc += x;
        }
        return acc;
    }
    // Split at a BLOCK multiple so the recursion tree — and therefore the rounding — depends only
    // on the length, never on how a caller chose to chunk the data.
    let half = (xs.len() / 2 + BLOCK - 1) / BLOCK * BLOCK;
    sum_pairwise(&xs[..half]) + sum_pairwise(&xs[half..])
}

/// Neumaier's compensated sum (Kahan–Babuška–Neumaier): tracks the rounding error in a second
/// accumulator and adds it back at the end.
///
/// Neumaier rather than plain Kahan because Kahan loses the compensation when the running total is
/// *smaller* than the incoming term — exactly what happens when a residual is converging toward
/// zero and each new contribution is comparatively large. That is precisely our case.
pub fn sum_neumaier(xs: &[f64]) -> f64 {
    let mut sum = 0.0f64;
    let mut c = 0.0f64;
    for &x in xs {
        let t = sum + x;
        // Whichever operand is larger determines where the lost low-order bits went.
        c += if sum.abs() >= x.abs() { (sum - t) + x } else { (x - t) + sum };
        sum = t;
    }
    sum + c
}

/// Error-free transformation: returns `(s, e)` with `s = fl(a+b)` and `a + b = s + e` exactly.
/// Knuth's two-sum — no branch, valid for any finite inputs.
#[inline]
pub fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let s = a + b;
    let bv = s - a;
    let e = (a - (s - bv)) + (b - bv);
    (s, e)
}

/// Error-free product via FMA: `a * b = p + e` exactly.
#[inline]
pub fn two_product(a: f64, b: f64) -> (f64, f64) {
    let p = a * b;
    let e = f64::mul_add(a, b, -p);
    (p, e)
}

/// Correctly rounded sum by distillation: repeatedly apply two-sum until the residuals are
/// exhausted. Expensive; this exists so tests can state what the right answer *is* rather than
/// comparing two approximations to each other.
pub fn sum_exact(xs: &[f64]) -> f64 {
    let mut partials: Vec<f64> = Vec::with_capacity(8);
    for &x in xs {
        let mut x = x;
        let mut i = 0;
        for j in 0..partials.len() {
            let (s, e) = two_sum(partials[j], x);
            if e != 0.0 {
                partials[i] = e;
                i += 1;
            }
            x = s;
        }
        partials.truncate(i);
        partials.push(x);
    }
    partials.iter().fold(0.0, |a, b| a + b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sum where naive accumulation is catastrophically wrong: one huge term, many tiny ones.
    /// The tiny terms vanish into the rounding of the big one unless the error is tracked.
    fn ill_conditioned() -> Vec<f64> {
        let mut v = vec![1.0e17];
        v.extend(std::iter::repeat(1.0).take(10_000));
        v.push(-1.0e17);
        v
    }

    #[test]
    fn compensation_recovers_what_naive_summation_destroys() {
        let xs = ill_conditioned();
        let truth = 10_000.0;

        let naive = xs.iter().fold(0.0f64, |a, b| a + b);
        let neumaier = sum_neumaier(&xs);
        let exact = sum_exact(&xs);

        // Naive loses everything: the 10,000 ones disappear into 1e17's last bits.
        assert!(
            (naive - truth).abs() > 1000.0,
            "naive was supposed to fail here, got {naive}"
        );
        assert_eq!(neumaier, truth, "Neumaier must recover the exact value");
        assert_eq!(exact, truth);
    }

    #[test]
    fn pairwise_beats_naive_on_a_long_sum() {
        // 10^7 terms of 0.1 — long enough that O(n·ε) drift is measurable.
        let xs = vec![0.1f64; 10_000_000];
        let truth = sum_exact(&[1_000_000.0]); // 0.1 * 1e7, exactly representable as 1e6
        let naive = xs.iter().fold(0.0f64, |a, b| a + b);
        let pair = sum_pairwise(&xs);
        assert!(
            (pair - truth).abs() <= (naive - truth).abs(),
            "pairwise {pair} should be no worse than naive {naive}"
        );
        assert!((pair - truth).abs() < 1e-6, "pairwise drifted: {}", pair - truth);
    }

    #[test]
    fn the_same_call_always_gives_the_same_bits() {
        // The reproducibility promise, stated exactly: identical input, identical output bits.
        // This is what lets the rest of the workspace test with `to_bits()` rather than epsilons.
        let xs: Vec<f64> = (0..5_000).map(|i| ((i as f64) * 0.7).sin() * 1e6).collect();
        let a = sum_pairwise(&xs);
        for _ in 0..8 {
            assert_eq!(sum_pairwise(&xs).to_bits(), a.to_bits());
        }
        assert_eq!(sum_neumaier(&xs).to_bits(), sum_neumaier(&xs).to_bits());
    }

    #[test]
    fn chunked_reductions_stay_inside_the_pairwise_error_bound() {
        // Re-chunking changes the summation tree, so the answer changes in the last bits. What
        // must hold is the textbook bound: pairwise error is at most log2(n)·eps·SUM|x_i| —
        // note the bound scales with the sum of MAGNITUDES, not with the result. For a
        // cancelling sum those differ enormously (here the condition number is ~3.6e4), which
        // is exactly why an earlier version of this test asserting a flat relative tolerance on
        // the *result* was wrong. It failed, and the test was at fault, not the code.
        let xs: Vec<f64> = (0..5_000).map(|i| ((i as f64) * 0.7).sin() * 1e6).collect();
        let truth = sum_exact(&xs);
        let magnitude: f64 = sum_exact(&xs.iter().map(|x| x.abs()).collect::<Vec<_>>());
        let n = xs.len() as f64;
        let bound = n.log2() * f64::EPSILON * magnitude;
        // Condition number of this sum, reported so the bound's size is legible.
        let kappa = magnitude / truth.abs();
        assert!(kappa > 1e4, "this test is only meaningful on a cancelling sum, kappa={kappa}");

        for chunk in [1usize, 7, 64, 128, 999, 4096] {
            let parts: Vec<f64> = xs.chunks(chunk).map(sum_pairwise).collect();
            let got = sum_pairwise(&parts);
            assert!(
                (got - truth).abs() <= bound,
                "chunk {chunk}: error {:.3e} exceeds pairwise bound {:.3e}",
                (got - truth).abs(),
                bound
            );
        }
        // And the compensated sum is bound-free: its error does not grow with n at all.
        assert!(
            (sum_neumaier(&xs) - truth).abs() <= truth.abs() * 4.0 * f64::EPSILON,
            "Neumaier should be near-exact even here"
        );
    }

    #[test]
    fn two_sum_and_two_product_are_error_free() {
        let cases = [
            (1.0e17, 1.0),
            (0.1, 0.2),
            (1.0, f64::MIN_POSITIVE),
            (3.0e300, -3.0e300),
        ];
        for (a, b) in cases {
            let (s, e) = two_sum(a, b);
            // s + e reproduces a + b exactly, which we can check by re-summing exactly.
            assert_eq!(sum_exact(&[s, e]), sum_exact(&[a, b]), "two_sum({a}, {b})");
        }
        let (p, e) = two_product(0.1, 0.2);
        assert_eq!(p + e, sum_exact(&[p, e]));
        assert!(e != 0.0, "0.1*0.2 is inexact, so the error term must be nonzero");
    }

    #[test]
    fn empty_and_single_element_reductions_are_sane() {
        for f in [sum_pairwise as fn(&[f64]) -> f64, sum_neumaier, sum_exact] {
            assert_eq!(f(&[]), 0.0);
            assert_eq!(f(&[42.5]), 42.5);
        }
    }
}
