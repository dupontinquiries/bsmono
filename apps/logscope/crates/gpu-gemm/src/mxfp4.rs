//! mxfp4.rs — MXFP4 (ggml type 39), the quant the current Qwen MoE releases actually ship in.
//!
//! "The mainline standard is to use MXFP4 for the MoE tensors, and Q8 for the rest" is how the
//! community repackages Qwen3.5-35B-A3B and Qwen3.6-35B-A3B, so without this the most-downloaded
//! GGUF of the model that succeeds Qwen3-30B-A3B fails with "unsupported ggml type 39" — a number,
//! not an answer.
//!
//! Every constant below is copied from the reference rather than recalled, because all three are
//! easy to get subtly wrong and none of them would announce it:
//!
//!   * The code table is {0,1,2,3,4,6,8,12} and its negatives — INTEGERS, twice the nominal E2M1
//!     values (0.5, 1, 1.5, ...). ggml compensates by halving the scale, and a table of the nominal
//!     values with an unhalved scale would agree on nothing.
//!   * The scale is e8m0_to_fp32_HALF: 2^(e-128), assembled as a bit pattern, with x < 2 handled by
//!     denormal patterns (0x00200000 << x) because the normalized formula underflows there.
//!   * The packing is Q4_0's, not Q4_K's: the low nibble of byte j is element j and the HIGH nibble
//!     is element j + 16. Reading them as adjacent would interleave the block and still produce
//!     plausible-looking floats.
//!
//! e = 0 and e = 0x7F are NaN/zero sentinels in the reference's companion function; the HALF variant
//! does not special-case them, and neither does this — matching is the point.

/// 32 weights per block: one E8M0 scale byte, then 16 bytes of packed nibbles.
pub const MXFP4_BLOCK: usize = 17;

/// ggml's kvalues_mxfp4 (= kvalues_fp4). Integers, twice the nominal E2M1 magnitudes.
const KVALUES: [i8; 16] = [0, 1, 2, 3, 4, 6, 8, 12, 0, -1, -2, -3, -4, -6, -8, -12];

/// ggml_e8m0_to_fp32_half: 2^(x-128), built as a bit pattern exactly as the reference does.
fn e8m0_to_fp32_half(x: u8) -> f32 {
    let bits: u32 = if x < 2 {
        // 0x00200000 = 2^-128, 0x00400000 = 2^-127 — denormals, where the normalized form breaks.
        0x0020_0000u32 << x
    } else {
        // 0.5 * 2^(x-127) = 2^(x-128): a normalized float with exponent field (x-1).
        (x as u32 - 1) << 23
    };
    f32::from_bits(bits)
}

/// Dequantize `n` MXFP4 weights.
pub fn dequant_mxfp4(raw: &[u8], n: usize) -> Result<Vec<f32>, String> {
    const QK: usize = 32;
    if n % QK != 0 { return Err(format!("MXFP4 needs a multiple of {QK} weights, got {n}")); }
    let blocks = n / QK;
    if raw.len() < blocks * MXFP4_BLOCK {
        return Err(format!("MXFP4: {} bytes for {blocks} blocks, need {}",
            raw.len(), blocks * MXFP4_BLOCK));
    }
    let mut out = vec![0.0f32; n];
    for b in 0..blocks {
        let blk = &raw[b * MXFP4_BLOCK..(b + 1) * MXFP4_BLOCK];
        let d = e8m0_to_fp32_half(blk[0]);
        let qs = &blk[1..];
        for j in 0..QK / 2 {
            // Q4_0's split, not Q4_K's adjacency: low nibble is j, high nibble is j + 16.
            out[b * QK + j] = KVALUES[(qs[j] & 0x0F) as usize] as f32 * d;
            out[b * QK + j + QK / 2] = KVALUES[(qs[j] >> 4) as usize] as f32 * d;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(e: u8, nibbles: [u8; 32]) -> Vec<u8> {
        let mut v = vec![e];
        for j in 0..16 { v.push(nibbles[j] | (nibbles[j + 16] << 4)); }
        v
    }

    /// The scale is 2^(x-128), NOT 2^(x-127). One exponent out doubles every weight in the file and
    /// the model still produces fluent text, merely worse — the failure mode this project keeps
    /// paying for.
    #[test]
    fn the_scale_is_the_half_variant() {
        assert_eq!(e8m0_to_fp32_half(128), 1.0, "2^(128-128) must be 1.0");
        assert_eq!(e8m0_to_fp32_half(129), 2.0);
        assert_eq!(e8m0_to_fp32_half(127), 0.5);
        // the denormal branch
        assert_eq!(e8m0_to_fp32_half(0), f32::from_bits(0x0020_0000));
        assert_eq!(e8m0_to_fp32_half(1), f32::from_bits(0x0040_0000));
    }

    /// The code table is integers 0,1,2,3,4,6,8,12 — not the nominal 0,0.5,1,1.5,2,3,4,6.
    #[test]
    fn the_code_table_is_the_doubled_integer_form() {
        let mut nib = [0u8; 32];
        for (i, n) in nib.iter_mut().enumerate().take(16) { *n = i as u8; }
        let v = dequant_mxfp4(&block(128, nib), 32).unwrap();   // scale 1.0
        assert_eq!(&v[..8], &[0.0, 1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0]);
        assert_eq!(&v[8..16], &[0.0, -1.0, -2.0, -3.0, -4.0, -6.0, -8.0, -12.0]);
    }

    /// The HIGH nibble of byte j is element j+16, not j+1. Reading them adjacent interleaves the
    /// block and still yields plausible floats, which is why this is pinned rather than assumed.
    #[test]
    fn the_high_nibble_lands_sixteen_elements_later() {
        let mut nib = [0u8; 32];
        nib[0] = 2;        // low nibble of byte 0  -> element 0  -> 2.0
        nib[16] = 7;       // high nibble of byte 0 -> element 16 -> 12.0
        let v = dequant_mxfp4(&block(128, nib), 32).unwrap();
        assert_eq!(v[0], 2.0);
        assert_eq!(v[16], 12.0, "high nibble must land at j + 16");
        assert_eq!(v[1], 0.0, "and NOT at j + 1");
    }

    #[test]
    fn the_scale_multiplies_every_code() {
        let mut nib = [0u8; 32];
        nib[3] = 5;                                   // value 6
        let v = dequant_mxfp4(&block(130, nib), 32).unwrap();   // scale 4.0
        assert_eq!(v[3], 24.0);
    }

    #[test]
    fn wrong_lengths_are_refused() {
        assert!(dequant_mxfp4(&[0u8; MXFP4_BLOCK], 31).is_err(), "not a multiple of 32");
        assert!(dequant_mxfp4(&[0u8; 4], 32).is_err(), "short buffer");
        assert_eq!(dequant_mxfp4(&[0u8; MXFP4_BLOCK * 2], 64).unwrap().len(), 64);
    }
}

#[cfg(test)]
mod size_agreement {
    /// type_nbytes and the dequantiser must agree, or a sliced read hands the wrong bytes to a
    /// decoder that will happily produce plausible floats from them.
    #[test]
    fn the_declared_size_is_what_the_dequantiser_consumes() {
        for n in [32usize, 64, 256, 4096] {
            let want = crate::gguf::type_nbytes(39, n).expect("MXFP4 size must be known");
            assert_eq!(want, n / 32 * super::MXFP4_BLOCK);
            let raw = vec![128u8; want];
            assert_eq!(super::dequant_mxfp4(&raw, n).unwrap().len(), n);
            // one byte short must be refused, not read
            assert!(super::dequant_mxfp4(&raw[..want - 1], n).is_err(), "n={n}: short buffer accepted");
        }
        assert!(crate::gguf::type_nbytes(39, 33).is_none(), "not a multiple of 32");
    }
}

#[cfg(test)]
mod slice_path {
    use crate::Gpu;

    fn blocks(n_blocks: usize, seed: u64) -> Vec<u8> {
        let mut x = seed;
        let mut next = move || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x };
        let mut v = Vec::with_capacity(n_blocks * super::MXFP4_BLOCK);
        for i in 0..n_blocks {
            // Exercise the denormal scale branch (e < 2) as well as normal exponents.
            v.push(match i % 4 { 0 => 128u8, 1 => 130, 2 => 1, _ => 126 });
            for _ in 0..16 { v.push((next() >> 24) as u8); }
        }
        v
    }

    /// The slice path must land where the f32 round-trip lands. MXFP4 was the last common source
    /// without one, so every weight of a Qwen3.5/3.6 MXFP4_MOE repackaging took the slow load —
    /// which is a platform-independent cost, not a macOS one.
    #[test]
    fn the_transcode_matches_the_f32_round_trip() {
        let gpu = Gpu::new();
        for (n, k) in [(2usize, 32usize), (3, 64), (8, 256)] {
            let raw = blocks(n * (k / 32), 11 + n as u64);
            let f32s = super::dequant_mxfp4(&raw, n * k).expect("dequant");
            let want = gpu.download(&gpu.quantize_q8_resident(&f32s, n, k).qs);
            let fast = gpu.mxfp4_to_int8_resident(&raw, n, k).expect("transcode");
            let have = gpu.download(&fast.qs);
            assert_eq!(want.len(), have.len(), "n={n} k={k}: scale count");
            for (i, (a, b)) in want.iter().zip(&have).enumerate() {
                assert!((a - b).abs() <= 1e-9 * a.abs().max(1.0),
                    "n={n} k={k} block {i}: sliced scale {b} != round-tripped {a}");
            }
        }
    }

    #[test]
    fn shapes_it_cannot_honour_are_refused() {
        let gpu = Gpu::new();
        let raw = blocks(2, 5);
        assert!(gpu.mxfp4_to_int8_resident(&raw, 1, 31).is_none(), "k must be a multiple of 32");
        assert!(gpu.mxfp4_to_int8_resident(&raw[..8], 1, 64).is_none(), "short buffer must refuse");
    }
}
