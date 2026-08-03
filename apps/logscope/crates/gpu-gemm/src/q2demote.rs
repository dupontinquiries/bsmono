//! q2demote.rs — a 2-bit residency format for experts the router rarely picks.
//!
//! ## The format, and why it is ours rather than ggml's
//!
//! This is not for reading Q2_K files. It re-quantizes weights **already resident** in q4, so the
//! layout can be whatever serves that: **2 bits per weight plus one f16 scale per 32**, giving
//! 2.5 bpw against q4's 4.5 — a 44% cut in bytes moved per fetch.
//!
//! **That design choice was wrong, and the measurement in this file disproves it.** I argued Q2_K's
//! sub-block mins and second scale tier were complexity a demotion format could skip. The measured
//! relative L2 error says otherwise:
//!
//! ```text
//! uniform noise        0.316
//! gaussian-ish         0.444   <- closest to real weight distributions
//! sparse w/ outliers   1.659   <- error LARGER than the signal
//! ```
//!
//! 44% on a realistic distribution is destructive, not a trade. And the outlier case explains why:
//! one large weight sets the group's scale, the other 31 land on the two inner levels, and those
//! levels are now nowhere near where the mass actually sits.
//!
//! Q2_K's extra machinery is not gratuitous — **a per-sub-block minimum is exactly what stops an
//! outlier from destroying its neighbours**, and at four levels there is no headroom to absorb that
//! any other way. A viable demotion format needs that structure.
//!
//! ## Symmetric, not asymmetric
//!
//! Four levels is few enough that spending one on a zero offset costs 25% of the representable
//! range. Symmetric around zero keeps all four for magnitude, which matters more at 2 bits than at 4.
//!
//! ## What this module is FOR
//!
//! Two things, and the second is the one that decides whether demotion is a good idea at all:
//!
//! 1. the quantize/dot pair a GPU kernel must match, exactly as `dot_iq1_s` serves IQ1_S;
//! 2. **`relative_error`**, which measures what demoting a given expert actually costs.
//!
//! Usage frequency is a proxy for how OFTEN an expert matters, not how MUCH it matters when it does,
//! and in a sparse MoE those come apart: a rare expert is rare because it is a specialist, chosen
//! precisely when the input is unusual. Measuring the error directly is what replaces that proxy.

/// f32 to f16 bits. `gguf::f16_to_f32` is the inverse and already in this crate; the forward
/// direction lived only in main.rs, which a library module cannot reach.
fn to_f16(f: f32) -> u16 {
    // The mantissa is 23 bits and must be shifted by 13 to reach 10 — but `(b & 0x7f_ffff) as u16`
    // TRUNCATES to 16 bits FIRST, discarding the top 7 mantissa bits and shifting in the wrong ones.
    // 0.3333 came back as 0.2512 and every scale in the format was wrong, so `relative_error` was
    // measuring this converter rather than the quantization — the bad-fixture failure again, in code
    // I wrote in this same commit.
    let b = f.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32 - 127 + 15;
    let mant = (b & 0x7f_ffff) >> 13;                  // shift in 32-bit space, THEN narrow
    if exp <= 0 { return sign; }                       // flush subnormals to zero
    if exp >= 0x1f { return sign | 0x7c00; }           // saturate to inf
    sign | ((exp as u16) << 10) | (mant as u16)
}

fn from_f16(h: u16) -> f32 { crate::gguf::f16_to_f32(h) }

/// Weights per scale. Matches the q4 resident format's grouping so the two are interchangeable at
/// the block level.
pub const GROUP: usize = 32;

/// Bytes for `n` weights: 2 bits each plus one f16 scale per group.
pub fn packed_len(n: usize) -> usize {
    let groups = n.div_ceil(GROUP);
    groups * (GROUP / 4 + 2)      // 8 bytes of codes + 2 bytes of scale
}

/// Quantize f32 weights to the 2-bit demotion format.
///
/// Levels are -1.5, -0.5, +0.5, +1.5 times the scale — symmetric, and evenly spaced so the decode is
/// a multiply rather than a table lookup.
pub fn quantize(w: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(packed_len(w.len()));
    for g in w.chunks(GROUP) {
        let amax = g.iter().fold(0.0f32, |a, x| a.max(x.abs()));
        // 1.5 is the largest level, so the scale that puts amax at full scale is amax/1.5.
        let scale = if amax > 0.0 { amax / 1.5 } else { 0.0 };
        let inv = if scale > 0.0 { 1.0 / scale } else { 0.0 };
        let mut codes = [0u8; GROUP / 4];
        for (i, x) in g.iter().enumerate() {
            // -1.5,-0.5,0.5,1.5 -> 0,1,2,3
            // +1.5, matching the decode's -1.5. My first version added 2.0 while `level` subtracted
            // 1.5, so -1.0 and +1.0 encoded to codes 1 and 3 and decoded to -0.25 and +0.75 — an
            // asymmetric ladder from a format documented as symmetric. The symmetry test caught it,
            // which is the one property a 2-bit format cannot afford to get wrong: at four levels a
            // half-step bias is an eighth of the whole range.
            let q = (((x * inv) + 1.5).round().clamp(0.0, 3.0)) as u8;
            codes[i / 4] |= q << ((i % 4) * 2);
        }
        // A group shorter than GROUP still writes a full group; the caller's length governs reads.
        out.extend_from_slice(&codes);
        out.extend_from_slice(&to_f16(scale).to_le_bytes());
    }
    out
}

/// Dequantize one weight, for reference and for the error measurement.
fn level(code: u8, scale: f32) -> f32 { (code as f32 - 1.5) * scale }

/// Dot product against the packed form, reading the source bytes directly.
///
/// No dequantize pass: the same shape as `dot_iq1_s`, and the reference a GPU kernel must match
/// bit-for-bit before it is allowed anywhere near a model.
pub fn dot(packed: &[u8], act: &[f32], n: usize) -> f32 {
    let mut acc = 0.0f32;
    for (gi, chunk) in act.chunks(GROUP).enumerate() {
        let base = gi * (GROUP / 4 + 2);
        if base + GROUP / 4 + 2 > packed.len() { break; }
        let scale = from_f16(u16::from_le_bytes([packed[base + 8], packed[base + 9]]));
        for (i, a) in chunk.iter().enumerate() {
            if gi * GROUP + i >= n { break; }
            let code = (packed[base + i / 4] >> ((i % 4) * 2)) & 0x3;
            acc += a * level(code, scale);
        }
    }
    acc
}

/// Relative L2 error from demoting these weights — **the number that should decide demotion**.
///
/// A rare expert is rare because it is a specialist: the router picked it when the input was unusual,
/// so its tokens are exactly the ones the common experts would have handled badly. Frequency says how
/// often an expert is used; this says what is lost when it is degraded, and only the second is the
/// question being asked.
pub fn relative_error(w: &[f32]) -> f32 {
    let packed = quantize(w);
    let (mut num, mut den) = (0.0f64, 0.0f64);
    for (gi, chunk) in w.chunks(GROUP).enumerate() {
        let base = gi * (GROUP / 4 + 2);
        let scale = from_f16(u16::from_le_bytes([packed[base + 8], packed[base + 9]]));
        for (i, x) in chunk.iter().enumerate() {
            let code = (packed[base + i / 4] >> ((i % 4) * 2)) & 0x3;
            let d = (level(code, scale) - x) as f64;
            num += d * d;
            den += (*x as f64) * (*x as f64);
        }
    }
    if den == 0.0 { 0.0 } else { (num / den).sqrt() as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weights(n: usize, seed: u32) -> Vec<f32> {
        let mut r = seed;
        (0..n).map(|_| { r = r.wrapping_mul(1664525).wrapping_add(1013904223);
            ((r >> 8) as f32 / 8_388_608.0) - 1.0 }).collect()
    }

    #[test]
    fn the_format_is_2_point_5_bits_per_weight() {
        // 44% smaller than q4's 4.5 bpw, which is the entire speed argument for demotion.
        let bpw = packed_len(1024) as f32 * 8.0 / 1024.0;
        assert!((bpw - 2.5).abs() < 0.01, "got {bpw}");
        assert!(bpw < 4.5 * 0.57, "must be well under q4 to be worth a second format");
    }

    #[test]
    fn a_CONSTANT_group_round_trips_almost_exactly() {
        // All weights equal means one level carries them, and the error is only the level offset.
        let w = vec![0.5f32; GROUP];
        assert!(relative_error(&w) < 0.35, "got {}", relative_error(&w));
    }

    #[test]
    fn the_error_on_random_weights_is_LARGE_and_that_is_the_point() {
        // Two bits is four levels. This is not a free lunch and the module must not imply it is —
        // ~20-30% relative L2 is what demotion actually costs, and it is why the decision has to be
        // per-expert rather than blanket.
        let e = relative_error(&weights(4096, 7));
        assert!(e > 0.10, "2-bit error should be substantial, got {e}");
        assert!(e < 0.60, "but not destructive, got {e}");
    }

    #[test]
    fn dot_matches_a_dequantize_then_dot_reference() {
        // The property a GPU kernel must reproduce. Reading packed bytes directly must equal
        // expanding them first, or the fast path and the reference disagree about the same weights.
        let w = weights(256, 3);
        let a = weights(256, 11);
        let packed = quantize(&w);
        let direct = dot(&packed, &a, 256);
        let mut expanded = 0.0f32;
        for (gi, chunk) in a.chunks(GROUP).enumerate() {
            let base = gi * (GROUP / 4 + 2);
            let s = from_f16(u16::from_le_bytes([packed[base + 8], packed[base + 9]]));
            for (i, x) in chunk.iter().enumerate() {
                let c = (packed[base + i / 4] >> ((i % 4) * 2)) & 0x3;
                expanded += x * level(c, s);
            }
        }
        assert!((direct - expanded).abs() < 1e-3, "{direct} vs {expanded}");
    }

    #[test]
    fn a_ZERO_group_does_not_divide_by_zero() {
        let w = vec![0.0f32; GROUP];
        assert_eq!(relative_error(&w), 0.0);
        assert_eq!(dot(&quantize(&w), &vec![1.0; GROUP], GROUP), 0.0);
    }

    #[test]
    fn error_VARIES_between_weight_distributions() {
        // The whole basis for choosing per expert. If every expert quantized alike there would be
        // nothing to choose and frequency would be as good a signal as any.
        let flat = relative_error(&vec![0.5f32; 1024]);
        let spread = relative_error(&weights(1024, 5));
        assert!(spread > flat * 1.5, "flat {flat} vs spread {spread}");
    }

    #[test]
    fn the_levels_are_SYMMETRIC_around_zero() {
        // Four levels is few enough that spending one on a zero offset costs 25% of the range.
        let w: Vec<f32> = vec![-1.0, 1.0].into_iter().cycle().take(GROUP).collect();
        let packed = quantize(&w);
        let s = from_f16(u16::from_le_bytes([packed[8], packed[9]]));
        let (a, b) = (level(packed[0] & 3, s), level((packed[0] >> 2) & 3, s));
        assert!((a + b).abs() < 1e-3, "mirrored inputs must give mirrored levels: {a} {b}");
    }
}

#[cfg(test)]
mod f16_tests {
    use super::*;

    #[test]
    fn to_f16_round_trips_the_scales_this_format_uses() {
        // Written by hand because the forward conversion lived only in main.rs. If it is wrong, every
        // scale is wrong and every error measurement is measuring the converter rather than the
        // format — which is exactly the bad-fixture failure this codebase has paid for twice.
        for v in [0.3333f32, 0.5, 1.0, 0.0625, 2.5, 0.001] {
            let back = from_f16(to_f16(v));
            assert!((back - v).abs() / v.max(1e-6) < 0.01, "{v} -> {back}");
        }
        assert_eq!(from_f16(to_f16(0.0)), 0.0);
    }
}

#[cfg(test)]
mod measured {
    use super::*;

    /// The actual cost of demotion, printed rather than asserted — this is the number that decides
    /// whether the idea is worth the kernel, and a threshold baked into an assertion would hide it.
    #[test]
    fn print_the_relative_error_across_distributions() {
        let mut r = 12345u32;
        let mut rnd = || { r = r.wrapping_mul(1664525).wrapping_add(1013904223);
            ((r >> 8) as f32 / 8_388_608.0) - 1.0 };
        let uniform: Vec<f32> = (0..4096).map(|_| rnd()).collect();
        // Weights in a trained model are far from uniform — mostly small, with rare large outliers.
        // A format judged on uniform noise is judged on the wrong distribution.
        let gaussian: Vec<f32> = (0..4096).map(|_| {
            let s: f32 = (0..6).map(|_| rnd()).sum();
            s / 6.0
        }).collect();
        let peaked: Vec<f32> = (0..4096).map(|i| if i % 97 == 0 { 1.0 } else { rnd() * 0.05 }).collect();
        println!("q2 demotion relative L2 error:");
        println!("  uniform            {:.3}", relative_error(&uniform));
        println!("  gaussian-ish       {:.3}  <- closest to real weights", relative_error(&gaussian));
        println!("  sparse w/ outliers {:.3}  <- worst case: one big value sets the scale",
            relative_error(&peaked));
    }
}
