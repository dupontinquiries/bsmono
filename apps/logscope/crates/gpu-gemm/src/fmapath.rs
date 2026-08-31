//! fmapath — make `f32::mul_add` cost one instruction instead of one function call.
//!
//! ## The measurement
//!
//! The K-quant dequantizers use `mul_add` deliberately: the WGSL shader contracts `dsc * nib - dm`
//! into an FMA and cannot be stopped from doing so, and `gate-q4k` showed that a CPU reference
//! rounding twice disagrees with the GPU by one ULP on half the block scales — which then crosses
//! quantization boundaries in the re-quantizer and produced 297430 differing words on one tensor.
//! One rounding is the correct choice and it must stay.
//!
//! What was not measured is what it costs. On baseline `x86_64-unknown-linux-gnu` there is no FMA
//! in the target feature set, so `f32::mul_add` does not lower to an instruction — it lowers to an
//! indirect call into `compiler_builtins`' `fmaf`, per element. Isolated on this machine:
//!
//! ```text
//!   mul_add : 25.38 ms      plain d*q - m : 3.13 ms      (5.24M elements)
//! ```
//!
//! 8.1x, and it is unvectorisable because a call sits in the loop body. Against llama.cpp's scalar C
//! dequantizer on identical bytes, Q4_K measured 9x slower, Q5_K 12x, Q2_K 2.7x — and those are
//! exactly the three formats that use `mul_add`. Q6_K, which does more work per element with plain
//! arithmetic, was already at parity.
//!
//! ## Why this is not "stop using mul_add"
//!
//! Dropping to `d * q - m` would be 8x faster and would silently reintroduce the two-rounding
//! divergence the FMA was chosen to remove. The semantics are load-bearing; only the lowering is
//! wrong.
//!
//! So: detect FMA once, and run the loop inside a `#[target_feature(enable = "fma")]` function
//! where `mul_add` lowers to `vfmadd`. Identical arithmetic, identical results, one instruction.
//! A CPU without FMA keeps the libm call and stays bit-identical — slow, but no machine gets a
//! different answer than it does today.
//!
//! **aarch64 needs none of this.** FMA is baseline in ARMv8, so `mul_add` already lowers to `fmla`;
//! the Asahi and phone targets were never paying this. It is a desktop-x86 tax exclusively, which
//! is probably why it went unnoticed on a project that benchmarks on ARM.

/// Does this CPU have hardware FMA? Detected once; the answer cannot change during a run.
#[cfg(target_arch = "x86_64")]
pub fn has_fma() -> bool {
    use std::sync::OnceLock;
    static YES: OnceLock<bool> = OnceLock::new();
    *YES.get_or_init(|| std::arch::is_x86_feature_detected!("fma"))
}

/// Every other architecture this ships to (aarch64 desktop and phone) has FMA in its baseline, so
/// `mul_add` is already an instruction and the dispatch would be pure overhead.
#[cfg(not(target_arch = "x86_64"))]
pub fn has_fma() -> bool { true }

/// Run `f` with FMA enabled when the hardware has it.
///
/// The dispatch happens ONCE PER CALL, not per element — a branch per element would give back a
/// good part of what the instruction saves. `f` should therefore be a whole loop, not one lane.
///
/// Safety: the `#[target_feature]` function is only reachable through the `has_fma()` check, which
/// is the exact precondition the attribute requires.
#[inline]
pub fn with_fma<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(target_arch = "x86_64")]
    {
        if has_fma() {
            #[target_feature(enable = "fma")]
            unsafe fn go<R>(f: impl FnOnce() -> R) -> R { f() }
            // SAFETY: guarded by the runtime detection immediately above.
            return unsafe { go(f) };
        }
    }
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property that makes this safe to apply to a numerically load-bearing path: enabling the
    /// feature changes the INSTRUCTION, never the VALUE. `mul_add` is a single-rounding fused
    /// multiply-add either way — an `fmaf` call and a `vfmadd` instruction compute the same thing.
    #[test]
    fn the_fma_path_gives_bit_identical_results() {
        // Values chosen so the two-rounding form differs: a*b needs more than 24 bits of mantissa.
        let cases: [(f32, f32, f32); 5] = [
            (1.0000001, 1.0000001, -1.0),
            (0.31525478, 3.0000002, -0.5),
            (16777217.0, 1.0000001, -16777216.0),
            (1e-20, 1e-20, 1e-30),
            (-7.3, 1.0 / 3.0, 2.4333334),
        ];
        for (a, b, c) in cases {
            let plain = a.mul_add(b, c);
            let fast = with_fma(|| a.mul_add(b, c));
            assert_eq!(plain.to_bits(), fast.to_bits(),
                       "with_fma changed a value: {a}*{b}+{c} -> {plain} vs {fast}");
        }
    }

    /// And it is genuinely a fused op, not two roundings — otherwise the whole reason the K-quant
    /// path uses `mul_add` would be gone and this module would be papering over that.
    #[test]
    fn mul_add_really_is_one_rounding() {
        // A CASE THAT ACTUALLY DISTINGUISHES THEM. My first attempt used (1.0000001, 1.0000001,
        // -1.0) on the assumption that any >24-bit product would do; the two agreed and the test
        // failed on its own "too weak to prove anything" message. Catastrophic cancellation is what
        // exposes the difference: c chosen as -(a*b) rounded, so the unfused form cancels to exactly
        // zero while the fused one keeps the bits the rounding threw away.
        let (a, b) = (1.0000002384f32, 2.0018880367f32);
        let c = -(a * b);
        let fused = with_fma(|| a.mul_add(b, c));
        let unfused = a * b + c;
        assert_eq!(unfused, 0.0, "the unfused form cancels");
        assert_ne!(fused, 0.0, "the fused form keeps the low bits — that is the whole point");
        assert_ne!(fused.to_bits(), unfused.to_bits());
    }

    #[test]
    fn a_loop_under_with_fma_matches_the_same_loop_without() {
        let q: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
        let (d, m) = (0.0123456f32, 0.4567891f32);
        let mut a = vec![0f32; q.len()];
        let mut b = vec![0f32; q.len()];
        for (o, &x) in a.iter_mut().zip(q.iter()) { *o = d.mul_add(x as f32, -m); }
        with_fma(|| { for (o, &x) in b.iter_mut().zip(q.iter()) { *o = d.mul_add(x as f32, -m); } });
        assert_eq!(a.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                   b.iter().map(|v| v.to_bits()).collect::<Vec<_>>());
    }
}
