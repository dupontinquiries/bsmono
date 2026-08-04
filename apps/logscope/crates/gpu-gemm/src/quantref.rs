//! quantref.rs — the CPU reference contract every quantizer must meet, at every width.
//!
//! Written before q1, q2 and q3 exist, so an implementation inherits what q4_K cost seven wrong
//! theories to learn instead of rediscovering it.
//!
//! ## The two rules, and why they are not obvious
//!
//! **Multiply by a reciprocal, never divide.** Metal's default fast-math lowers `/` to a multiply by
//! an f32-rounded reciprocal, allowed 2.5 ULP and not correctly rounded. A CPU that divides and a GPU
//! that does this disagree by an ULP — invisible until the value crosses a quantization boundary,
//! then a whole level.
//!
//! **Round ties to even, never away from zero.** WGSL's `round()` is half-to-even; Rust's
//! `f32::round()` is half-away-from-zero. They differ at exactly 0.5, 2.5, 4.5 — where the even
//! neighbour is lower.
//!
//! ## Tie density scales with level count, which decides how big a test must be
//!
//! A tie needs `w * inv_scale` to land exactly on `.5`. The finer the level spacing, the more values
//! do. q4_K at ±7 needed **70000 super-blocks** to expose 1104 mismatches while 112 blocks showed
//! zero — so a small gate reported a real bug as passing for weeks.
//!
//! **That is the trap this module exists to close.** Each width's test must be sized to its own tie
//! density, not to a convenient constant.

/// Symmetric level bound for a width: q2 -> ±1, q3 -> ±3, q4 -> ±7, q8 -> ±127.
///
/// Returns None for q1, which **cannot be symmetric-with-zero**: one bit gives two levels, and a
/// signed range needs three (-1, 0, +1). Real 1-bit formats like IQ1_S use a codebook of 8-weight
/// patterns instead, so a q1 written to this contract would be a different thing wearing the name.
pub fn level_bound(bits: u32) -> Option<f32> {
    match bits {
        0 | 1 => None,
        b if b <= 8 => Some(((1u32 << (b - 1)) - 1) as f32),
        _ => None,
    }
}

/// The reference quantizer. **Any GPU kernel must match this bit-for-bit.**
pub fn quantize_block(w: &[f32], bits: u32) -> Option<(f32, Vec<i32>)> {
    let bound = level_bound(bits)?;
    let amax = w.iter().fold(0.0f32, |a, x| a.max(x.abs()));
    // Reciprocal, not division — see the module docs.
    let scale = if amax > 0.0 { amax * (1.0f32 / bound) } else { 1.0 };
    let inv = if scale > 0.0 { 1.0f32 / scale } else { 0.0 };
    let q = w.iter()
        .map(|x| (x * inv).round_ties_even().clamp(-bound, bound) as i32)
        .collect();
    Some((scale, q))
}

/// Fraction of values in `w` that land within `eps` of a tie at this width.
///
/// **The number that sizes a test.** A gate too small to contain a tie reports the tie rule as
/// correct, which is exactly what happened to q4_K.
pub fn tie_fraction(w: &[f32], bits: u32, eps: f32) -> f32 {
    let Some(bound) = level_bound(bits) else { return 0.0 };
    let amax = w.iter().fold(0.0f32, |a, x| a.max(x.abs()));
    if amax <= 0.0 { return 0.0 }
    let inv = 1.0f32 / (amax * (1.0f32 / bound));
    let n = w.iter().filter(|x| {
        let v = (*x * inv).abs();
        (v - v.floor() - 0.5).abs() < eps
    }).count();
    n as f32 / w.len().max(1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut r = seed;
        (0..n).map(|_| { r = r.wrapping_mul(1664525).wrapping_add(1013904223);
            ((r >> 8) as f32 / 8_388_608.0) - 1.0 }).collect()
    }

    #[test]
    fn the_level_bounds_follow_the_width() {
        assert_eq!(level_bound(2), Some(1.0));
        assert_eq!(level_bound(3), Some(3.0));
        assert_eq!(level_bound(4), Some(7.0));
        assert_eq!(level_bound(8), Some(127.0));
    }

    #[test]
    fn q1_is_REFUSED_rather_than_given_a_degenerate_bound() {
        // One bit gives two levels and a signed range needs three (-1, 0, +1). Real 1-bit formats
        // use a codebook of 8-weight patterns — IQ1_S does — so a "q1" built to this contract would
        // be a different format wearing the name, and returning Some(0.0) would let someone build it.
        assert_eq!(level_bound(1), None);
        assert!(quantize_block(&noise(32, 1), 1).is_none());
    }

    #[test]
    fn TIES_round_to_even_at_every_width() {
        // The rule WGSL uses and Rust does not. Constructed so the quotient is exactly 2.5: the even
        // neighbour is 2, and f32::round() would give 3.
        // THE REACHABLE TIES DEPEND ON THE WIDTH, which my first fixture got wrong. A quotient of
        // 2.5 needs the bound to be at least 3, so at 2 bits (bound 1) the ONLY tie is 0.5 — there
        // is no room for another below the clamp. Testing 2.5 at 2 bits tested the clamp instead.
        //
        // That is itself worth pinning: a narrow format has fewer distinct ties, which is the other
        // half of why tie density rises with width.
        for bits in [2u32, 3, 4, 8] {
            let bound = level_bound(bits).unwrap();
            // amax IS the bound, so scale is exactly 1.0 and w values are their own quotients.
            let mut w = vec![bound, 0.5, -0.5];
            if bound >= 3.0 { w.push(2.5); w.push(-2.5); }
            let (scale, q) = quantize_block(&w, bits).unwrap();
            assert_eq!(scale, 1.0, "bits {bits}: amax is the bound so the scale is unity");
            assert_eq!(q[1], 0, "0.5 must round to 0 at {bits} bits, got {}", q[1]);
            assert_eq!(q[2], 0, "-0.5 must round to 0");
            if bound >= 3.0 {
                assert_eq!(q[3], 2, "2.5 must round to 2 at {bits} bits, got {}", q[3]);
                assert_eq!(q[4], -2, "-2.5 must round to -2");
            }
        }
    }

    #[test]
    fn the_scale_uses_a_RECIPROCAL_not_a_division() {
        // Asserted by result rather than by reading the source: amax * (1/bound) and amax / bound
        // differ by an ULP for some inputs, and the GPU computes the first whichever the source says.
        for bits in [2u32, 3, 4, 8] {
            let bound = level_bound(bits).unwrap();
            let w = vec![2.2067833f32, 0.5, -1.0];
            let (scale, _) = quantize_block(&w, bits).unwrap();
            let recip = 2.2067833f32 * (1.0f32 / bound);
            assert_eq!(scale.to_bits(), recip.to_bits(),
                "bits {bits}: scale must be the reciprocal product, not a quotient");
        }
    }

    #[test]
    fn TIE_DENSITY_RISES_WITH_WIDTH_which_is_how_big_a_test_must_be() {
        // The trap that cost q4_K weeks: 112 blocks showed zero mismatches and 70000 showed 1104.
        // Finer levels mean more values land on a tie, so each width needs its own test size — a
        // constant would be too small somewhere.
        let w = noise(8192, 7);
        // Measured over a window proportional to the level spacing, because that is what "near a
        // tie" means: at 8 bits the levels are 16x closer than at 4, so a FIXED eps counts a far
        // wider slice of each interval and the comparison says nothing about density.
        let frac = |bits: u32| {
            let bound = level_bound(bits).unwrap();
            tie_fraction(&w, bits, 0.02)         // 2% of an interval, at every width
                * bound                          // intervals per unit range
        };
        let (t2, t4, t8) = (frac(2), frac(4), frac(8));
        assert!(t8 > t4 && t4 > t2,
            "ties per unit range must rise with width: {t2:.3} {t4:.3} {t8:.3}");
    }

    #[test]
    fn a_test_that_contains_NO_TIES_proves_nothing_about_the_tie_rule() {
        // Stated as an assertion so nobody sizes a future gate by convenience. If the fraction is
        // zero, a wrong tie rule passes.
        let tiny = vec![0.1f32, 0.2, 0.3];
        assert_eq!(tie_fraction(&tiny, 4, 1e-6), 0.0);
    }

    #[test]
    fn clamping_holds_at_every_width() {
        // A value beyond amax cannot occur from the block's own max, but a shared scale across a
        // super-block can produce one — Q4_K's sub-blocks do exactly that.
        for bits in [2u32, 3, 4, 8] {
            let bound = level_bound(bits).unwrap();
            let (_, q) = quantize_block(&[1.0, -1.0], bits).unwrap();
            assert!(q.iter().all(|v| (*v as f32).abs() <= bound), "bits {bits}: {q:?}");
        }
    }

    #[test]
    fn an_all_zero_block_does_not_divide_by_zero() {
        for bits in [2u32, 3, 4, 8] {
            let (scale, q) = quantize_block(&vec![0.0f32; 32], bits).unwrap();
            assert_eq!(scale, 1.0);
            assert!(q.iter().all(|v| *v == 0));
        }
    }
}
