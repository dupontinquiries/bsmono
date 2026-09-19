//! blockquant — the 32-weight symmetric block quantizer, written so BOTH of its loops vectorize on
//! baseline targets (x86-64 SSE2, AArch64 NEON) while producing bit-identical output.
//!
//! Every weight that reaches the GPU as int8 or q4 from an f32 / f16 / Q6_K / Q5_K source passes
//! through this at LOAD — for a Q4_K_M file that is ffn_down and the output matrix, a third of
//! the model. It does not affect tok/s. It is minutes of wall time on a 35B.
//!
//! The block was:
//!
//! ```text
//! let mut amax = 0f32;
//! for j in 0..32 { amax = amax.max(w[j].abs()); }              // (1)
//! …
//! for j in 0..32 { q[j] = (w[j] * inv).round().clamp(-L, L) as i32 …; }   // (2)
//! ```
//!
//! **(1) is a float max REDUCTION.** `f32::max` lowers to `llvm.maxnum`, and without fast-math
//! flags LLVM will not reassociate it — NaN semantics — so it stays a 32-step scalar dependency
//! chain on every architecture. [`amax32`] keeps eight independent running maxima (a `select` per
//! lane, nothing to prove) and folds them at the end. `max` is exact, so lane order cannot change
//! the result: bit-identical, not approximately equal.
//!
//! **(2) calls `round()`**, which is round-half-AWAY-from-zero. (That rule is deliberate and
//! measured: the target driver's `round()` is ties-away whatever the WGSL spec says — see the note
//! in `quantize_q4_resident`.) Baseline SSE2 has no rounding instruction at all and SSE4.1's
//! `roundps` has no ties-away mode, so on x86 this is a scalar `roundf` libm CALL per weight; on
//! AArch64 it is `frinta`, which does vectorize. [`round_away_i32`] is
//! `(v + copysign(PRED_HALF, v)) as i32` — an add and a truncating convert, vector instructions
//! everywhere. It is EXACT, and that is not argued: `the_rounding_identity_exhaustively` enumerates
//! every f32 with |v| < 2^24, both signs (2,533,359,616 values, 8 s optimized) and finds no
//! disagreement. The constant must be the PREDECESSOR of one half: with 0.5 itself,
//! 0.49999997 + 0.5 rounds up to 1.0 and truncates to 1 where `round()` gives 0.
//!
//! Measured here at -O on baseline x86-64, per block: 220 ns → 124 ns from (1) alone. ARM was not
//! compiled or measured (no AArch64 std in this sandbox). On ARM (2) already vectorizes, so (1) is
//! expected to be the larger share there — expected, not shown.

/// The largest f32 below 0.5.
pub const PRED_HALF: f32 = 0.49999997;

/// `v.round() as i32` — round half away from zero, saturating, NaN → 0 — without `round()`.
#[inline(always)]
pub fn round_away_i32(v: f32) -> i32 { (v + PRED_HALF.copysign(v)) as i32 }

/// max |w[j]| over a block, as eight independent lanes. Ignores NaN exactly as `f32::max` does
/// (a comparison with NaN is false, so the lane keeps its value).
#[inline(always)]
pub fn amax32(w: &[f32; 32]) -> f32 {
    let mut lanes = [0f32; 8];
    for c in w.chunks_exact(8) {
        for (a, &x) in lanes.iter_mut().zip(c) { let v = x.abs(); *a = if v > *a { v } else { *a }; }
    }
    let mut m = 0f32;
    for &a in &lanes { m = if a > m { a } else { m }; }
    m
}

/// (scale, 1/scale) for a block whose largest magnitude is `amax`, quantizing to `[-levels, levels]`.
/// The reciprocal-of-levels multiply and the separate `1.0 / scale` are the callers' exact
/// arithmetic — the GPU lowers `/` to a multiply by an f32-rounded reciprocal and this matches it.
#[inline(always)]
pub fn block_scale(amax: f32, inv_levels: f32) -> (f32, f32) {
    let scale = if amax > 0.0 { amax * inv_levels } else { 1.0 };
    (scale, if scale > 0.0 { 1.0f32 / scale } else { 0.0 })
}

/// int8: 32 weights → 32 levels in [-127, 127]. Returns the block's scale.
#[inline(always)]
pub fn q8_block(w: &[f32; 32], q: &mut [i8; 32]) -> f32 {
    let (scale, inv) = block_scale(amax32(w), 1.0f32 / 127.0);
    for (o, &x) in q.iter_mut().zip(w) { *o = round_away_i32(x * inv).clamp(-127, 127) as i8; }
    scale
}

/// q4: 32 weights → 32 nibbles in 1..=15 (level + 8, symmetric around 8). Returns the scale.
#[inline(always)]
pub fn q4_block(w: &[f32; 32], nib: &mut [u8; 32]) -> f32 {
    let (scale, inv) = block_scale(amax32(w), 1.0f32 / 7.0);
    for (o, &x) in nib.iter_mut().zip(w) { *o = (round_away_i32(x * inv).clamp(-7, 7) + 8) as u8; }
    scale
}

/// The int8 block exactly as `quantize_q8_resident` had it. The oracle.
pub fn q8_block_reference(w: &[f32; 32], q: &mut [i8; 32]) -> f32 {
    let mut amax = 0f32;
    for j in 0..32 { amax = amax.max(w[j].abs()); }
    let scale = if amax > 0.0 { amax * (1.0f32 / 127.0) } else { 1.0 };
    let inv_scale = if scale > 0.0 { 1.0f32 / scale } else { 0.0 };
    for j in 0..32 { q[j] = (w[j] * inv_scale).round().clamp(-127.0, 127.0) as i32 as i8; }
    scale
}

/// The q4 block exactly as `quantize_q4_resident` / `_cpu` had it. The oracle.
pub fn q4_block_reference(w: &[f32; 32], nib: &mut [u8; 32]) -> f32 {
    let mut amax = 0f32;
    for j in 0..32 { amax = amax.max(w[j].abs()); }
    let scale = if amax > 0.0 { amax * (1.0f32 / 7.0) } else { 1.0 };
    let inv_scale = if scale > 0.0 { 1.0f32 / scale } else { 0.0 };
    for j in 0..32 { let q = (w[j] * inv_scale).round().clamp(-7.0, 7.0) as i32; nib[j] = (q + 8) as u8; }
    scale
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng(s: &mut u64) -> u64 { *s ^= *s << 13; *s ^= *s >> 7; *s ^= *s << 17; *s }

    /// Blocks chosen to hit what could differ: exact .5 ties at every level (the whole reason the
    /// rounding rule matters), a block of zeros (scale falls back to 1.0), negatives, denormals,
    /// one huge outlier, and NaN / ±Inf in every position class.
    fn blocks() -> Vec<[f32; 32]> {
        let mut out = Vec::new();
        let mut s = 0x9E3779B97F4A7C15u64;
        for amp in [1.0e-3f32, 0.07, 1.0, 40.0, 3.0e4] {
            for _ in 0..400 {
                let mut b = [0f32; 32];
                for x in b.iter_mut() { *x = ((rng(&mut s) >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0) * amp; }
                out.push(b);
            }
        }
        // ties: weights at (k + 0.5) * scale for both level counts, with the block max pinned so
        // the scale is exactly what makes them ties
        for levels in [7.0f32, 127.0] {
            let mut b = [0f32; 32];
            b[0] = levels; // amax = levels → scale = 1.0 (to within the reciprocal's rounding)
            for (j, x) in b.iter_mut().enumerate().skip(1) { let k = (j as f32 * 0.37 * levels / 12.0).floor(); *x = (k + 0.5) * if j % 2 == 0 { 1.0 } else { -1.0 }; }
            out.push(b);
        }
        out.push([0f32; 32]);
        out.push([-0.0f32; 32]);
        out.push([f32::MIN_POSITIVE / 4.0; 32]);
        let mut o = [0.01f32; 32]; o[19] = -9.0e8; out.push(o);
        for (at, bad) in [(0usize, f32::NAN), (7, f32::NAN), (31, f32::INFINITY), (8, f32::NEG_INFINITY)] {
            let mut b = [0.25f32; 32]; b[3] = -1.5; b[at] = bad; out.push(b);
        }
        out
    }

    #[test]
    fn both_block_quantizers_are_bit_identical_to_the_code_they_replace() {
        let bs = blocks();
        assert!(bs.len() > 2000);
        let (mut levels8, mut levels4) = (std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
        for (n, w) in bs.iter().enumerate() {
            let (mut a, mut b) = ([0i8; 32], [0i8; 32]);
            let (sa, sb) = (q8_block_reference(w, &mut a), q8_block(w, &mut b));
            assert!(sa.to_bits() == sb.to_bits() && a == b, "int8 block {n}: scale {sa} vs {sb}\n{a:?}\n{b:?}");
            let (mut c, mut d) = ([0u8; 32], [0u8; 32]);
            let (sc, sd) = (q4_block_reference(w, &mut c), q4_block(w, &mut d));
            assert!(sc.to_bits() == sd.to_bits() && c == d, "q4 block {n}: scale {sc} vs {sd}\n{c:?}\n{d:?}");
            levels8.extend(a); levels4.extend(c);
        }
        // not vacuous: the fixtures must actually exercise the whole output range
        assert!(levels8.len() > 200 && levels8.contains(&127) && levels8.contains(&-127), "{} int8 levels seen", levels8.len());
        assert_eq!(levels4.len(), 15, "every nibble 1..=15 appears: {levels4:?}");
    }

    /// `amax32` against the scalar reduction, including NaN (ignored by both) and where the
    /// maximum sits in each lane.
    #[test]
    fn the_lane_max_is_the_scalar_max() {
        for w in blocks() {
            let mut want = 0f32; for x in w { want = want.max(x.abs()); }
            assert_eq!(amax32(&w).to_bits(), want.to_bits(), "{w:?}");
        }
        for at in 0..32 { let mut w = [1.0f32; 32]; w[at] = -77.5; assert_eq!(amax32(&w), 77.5, "max at {at}"); }
    }

    /// A strided walk over the whole range plus every neighbourhood where rounding changes its
    /// answer. Runs in the ordinary (unoptimized) suite; the exhaustive version is below.
    #[test]
    fn the_rounding_identity_on_a_stride_and_at_every_boundary() {
        assert_eq!(PRED_HALF.to_bits(), 0.5f32.to_bits() - 1, "the constant is the predecessor of one half");
        let top = ((1u32 << 24) as f32).to_bits();
        let mut bits = 0u32;
        while bits < top {
            for sign in [0u32, 0x8000_0000] { let v = f32::from_bits(bits | sign); assert_eq!(round_away_i32(v), v.round() as i32, "{v:e}"); }
            bits += 9973;
        }
        for k in 0..300i32 {
            let half = k as f32 + 0.5;
            for d in -3i32..=3 {
                for sign in [1.0f32, -1.0] {
                    let v = f32::from_bits((half.to_bits() as i32 + d) as u32) * sign;
                    assert_eq!(round_away_i32(v), v.round() as i32, "near {half}: {v:e}");
                }
            }
        }
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.7e7, -1.7e7, 3.0e9, -3.0e9, 0.0, -0.0] {
            assert_eq!(round_away_i32(v), v.round() as i32, "{v:e}");
        }
        assert_eq!(((PRED_HALF + 0.5) as i32, PRED_HALF.round() as i32), (1, 0), "why the constant cannot be 0.5");
    }

    /// EVERY f32 with |v| < 2^24, both signs: 2.5 billion values. 8 s optimized, far too slow in a
    /// debug suite, so it is ignored by default:
    ///   cargo test --release -p gpu-gemm --lib the_rounding_identity_exhaustively -- --ignored
    /// Run once (rustc 1.96, x86-64) when this module was written: 0 disagreements.
    #[test]
    #[ignore]
    fn the_rounding_identity_exhaustively() {
        let top = ((1u32 << 24) as f32).to_bits();
        let mut bad = 0u64;
        for bits in 0..top {
            for sign in [0u32, 0x8000_0000] {
                let v = f32::from_bits(bits | sign);
                if round_away_i32(v) != v.round() as i32 { bad += 1; }
            }
        }
        assert_eq!(bad, 0);
    }
}
