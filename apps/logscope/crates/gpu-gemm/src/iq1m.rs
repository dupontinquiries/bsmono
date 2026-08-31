//! iq1m.rs — IQ1_M, which shares IQ1_S's codebook and differs in how it stores scales.
//!
//! The five remaining unsupported types in a UD-IQ1_S checkpoint are all codebook formats, and four
//! of them need a 2048-entry grid transcribed and every entry right. **IQ1_M does not: it uses the
//! same `iq1s_grid` that IQ1_S already has here.**
//!
//! That makes it the one tractable member of the group — 9% of the file, and the difference is
//! entirely in the scale layout:
//!
//! ```text
//! IQ1_S   50 bytes/256   one f16 per block, per-32 sub-scales in qh's top bits
//! IQ1_M   56 bytes/256   NO block-wide f16; four 4-bit scales in `scales`, and a
//!                        per-16 delta sign carried in qh
//! ```
//!
//! Six extra bytes buy finer scale granularity: IQ1_S scales per 32 weights, IQ1_M per 16. That is
//! the whole reason the format exists and the only thing that has to be got right.
//!
//! ## The delta sign
//!
//! Both formats add a small signed offset (`IQ1S_DELTA`, 0.125) to every weight, and its sign comes
//! from a bit in `qh`. **In IQ1_M that bit is per 16 weights and in IQ1_S per 32.** Applying the
//! IQ1_S spacing decodes half the block with the wrong offset sign — a small error on every weight
//! rather than a large one on a few, which is the shape that reads as a slightly worse model.

use crate::iq1s_grid::{entry, IQ1S_DELTA};

pub const IQ1_M_BLOCK: usize = 56;

fn f16(lo: u8, hi: u8) -> f32 {
    let h = u16::from_le_bytes([lo, hi]) as u32;
    let (sign, exp, mant) = ((h >> 15) & 1, (h >> 10) & 0x1F, h & 0x3FF);
    let bits = if exp == 0 {
        if mant == 0 { sign << 31 } else {
            let (mut e, mut m) = (-14i32, mant);
            while m & 0x400 == 0 { m <<= 1; e -= 1; }
            (sign << 31) | (((e + 127) as u32) << 23) | ((m & 0x3FF) << 13)
        }
    } else if exp == 0x1F { (sign << 31) | 0x7F80_0000 | (mant << 13) }
    else { (sign << 31) | ((exp + 112) << 23) | (mant << 13) };
    f32::from_bits(bits)
}

/// Dequantize IQ1_M: 256 weights in 56 bytes.
///
/// Layout: 32 bytes `qs` (grid index low bits), 16 bytes `qh` (index high bits plus delta signs),
/// 8 bytes `scales` (four 4-bit scale nibbles plus the block scale's bits).
pub fn dequant_iq1_m(raw: &[u8], n: usize) -> Result<Vec<f32>, String> {
    if n % 256 != 0 { return Err(format!("IQ1_M needs a multiple of 256 weights, got {n}")) }
    let blocks = n / 256;
    if raw.len() < blocks * IQ1_M_BLOCK {
        return Err(format!("IQ1_M: {} bytes for {blocks} blocks, need {}",
            raw.len(), blocks * IQ1_M_BLOCK));
    }
    let mut out = vec![0.0f32; n];
    for b in 0..blocks {
        let p = b * IQ1_M_BLOCK;
        let qs = &raw[p..p + 32];
        let qh = &raw[p + 32..p + 48];
        let sc = &raw[p + 48..p + 56];
        // THE SCALE BYTES ARE READ AS FOUR u16s, and both scales come out of them. Getting this
        // from the field names alone is not possible; it is
        //     scale.u16 = (sc[0] >> 12) | ((sc[1] >> 8) & 0x00f0) | ((sc[2] >> 4) & 0x0f00) | (sc[3] & 0xf000)
        // in ggml-quants.c, over `const uint16_t * sc = (const uint16_t *)x[i].scales`.
        let sc16 = |j: usize| u16::from_le_bytes([sc[2 * j], sc[2 * j + 1]]);
        // BLOCK SCALE: the top nibble of each of the four u16s — i.e. the top nibbles of scale
        // BYTES 1, 3, 5, 7, the ODD ones.
        //
        // This module used bytes 0, 2, 4, 6. Every IQ1_M tensor therefore decoded with an f16 built
        // from unrelated bits: not zero, not noise, just a plausible wrong number scaling the whole
        // block. Found by differential against llama.cpp's dequantizer on identical bytes.
        let hi = (sc16(0) >> 12)
               | ((sc16(1) >> 8) & 0x00F0)
               | ((sc16(2) >> 4) & 0x0F00)
               | (sc16(3) & 0xF000);
        let d = f16((hi & 0xFF) as u8, (hi >> 8) as u8);
        for ib in 0..16 {
            // SUB-BLOCK SCALES are 3-BIT, not 4-bit, and there are TWO per 32 weights: within u16
            // `sc16(ib32/2)`, at bit `6*(ib32%2)` for the first 16 weights and `+3` for the second
            // 16. This loop walks 16-weight groups, so `ib32 = ib/2` and `half = ib%2` picks which
            // of the pair applies.
            //
            // Reading them as 4-bit nibbles (what this did) takes three real bits plus one bit
            // belonging to the neighbouring scale, so roughly half the sub-blocks came out at
            // double their intended scale — an error that leaves the model running and merely bad.
            let (ib32, half) = (ib / 2, ib % 2);
            let s = ((sc16(ib32 / 2) >> (6 * (ib32 % 2) + 3 * half)) & 0x7) as f32;
            let dl = d * (2.0 * s + 1.0);
            for k in 0..2 {
                let idx_lo = qs[ib * 2 + k] as usize;
                let idx_hi = ((qh[ib] >> (4 * k)) & 7) as usize;
                let g = entry(idx_lo | (idx_hi << 8));
                // PER-16 delta sign, against IQ1_S's per-32. Using the IQ1_S spacing gives half the
                // block the wrong offset sign — a small error on every weight, which reads as a
                // slightly worse model rather than as a bug.
                let delta = if (qh[ib] >> (4 * k + 3)) & 1 != 0 { -IQ1S_DELTA } else { IQ1S_DELTA };
                let base = b * 256 + ib * 16 + k * 8;
                for j in 0..8 {
                    out[base + j] = dl * (g[j] as f32 + delta);
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn IQ1_M_REUSES_the_grid_IQ1_S_already_has() {
        // The reason this one is tractable and the other four are not: no 2048-entry transcription.
        assert_eq!(crate::iq1s_grid::IQ1S_GRID.len(), 2048);
        let e = entry(0);
        assert_eq!(e.len(), 8, "eight weights per grid entry, shared by both formats");
    }

    #[test]
    fn the_BLOCK_SIZE_is_six_bytes_larger_than_IQ1_S() {
        // 56 against 50, and those six bytes buy per-16 scale granularity instead of per-32. That is
        // the entire difference between the formats.
        assert_eq!(IQ1_M_BLOCK, 56);
        assert_eq!(crate::gguf::type_nbytes(19, 256), Some(50), "IQ1_S for comparison");
    }

    #[test]
    fn a_zero_block_decodes_without_panicking_and_yields_n() {
        let raw = vec![0u8; IQ1_M_BLOCK];
        let v = dequant_iq1_m(&raw, 256).unwrap();
        assert_eq!(v.len(), 256);
    }

    #[test]
    fn the_DELTA_SIGN_is_per_16_weights_not_per_32() {
        // Using IQ1_S's spacing gives half the block the wrong offset sign — a small error on every
        // weight, which reads as a slightly worse model rather than as a bug.
        let mut raw = vec![0u8; IQ1_M_BLOCK];
        // A NON-ZERO BLOCK SCALE now needs the ODD scale bytes: d is assembled from the top nibbles
        // of scales[1], [3], [5], [7] (= raw 49/51/53/55). Setting raw[48] — what this test did —
        // contributes nothing to d, so with the layout corrected the whole block decoded to zero
        // and the assertion became vacuously false. 0xC0 at 53 and 0x30 at 55 give d = 1.0.
        raw[53] = 0xC0; raw[55] = 0x30;       // d = f16(0x3C00) = 1.0
        raw[48] = 0x01;                       // sub-block 0 first-half 3-bit scale = 1 -> dl = 3
        raw[32] = 0b1000_0000;                // sub-block 0, k=1 delta sign SET
        let v = dequant_iq1_m(&raw, 256).unwrap();
        // Weights 0..8 (k=0) and 8..16 (k=1) must differ in sign of their offset.
        assert_ne!(v[0], v[8], "the two halves of sub-block 0 take different delta signs");
    }

    #[test]
    fn WRONG_lengths_are_refused() {
        assert!(dequant_iq1_m(&[0u8; 4], 256).is_err());
        assert!(dequant_iq1_m(&[0u8; IQ1_M_BLOCK], 300).is_err());
    }

    /// REFERENCE VECTOR from llama.cpp's own `dequantize_row_iq1_m`, on these exact bytes.
    ///
    /// Two scale bugs lived here until a differential run found them. The 16-bit block scale is
    /// built from the top nibbles of the ODD scale bytes (1,3,5,7) - llama.cpp reads `scales` as
    /// four u16s and takes the top nibble of each - and this file used the even ones. And the
    /// sub-block scales are 3-BIT fields, two per 32 weights at bit offsets 6*(i%2) and +3,
    /// where this file read 4-bit nibbles and so took one bit belonging to the neighbouring scale.
    ///
    /// Neither produces a crash or a zero: both give plausible wrong numbers, which is why an
    /// IQ1_M model would have run and merely been bad.
    #[test]
    fn iq1_m_matches_the_llama_cpp_reference_vector() {
        const RAW: [u8; IQ1_M_BLOCK] = [
        225, 59, 3, 46, 17, 42, 50, 181, 121, 8, 15, 8, 177, 247, 237, 76,
        46, 93, 58, 7, 249, 127, 33, 238, 35, 45, 23, 138, 32, 154, 246, 181,
        136, 127, 102, 232, 9, 36, 2, 170, 73, 242, 193, 85, 27, 39, 254, 83,
        38, 110, 73, 13, 177, 56, 72, 156,
        ];
        const EXPECT: [f32; 16] = [
            0.0125387907, -0.00975239277, 0.00139319897, 0.0125387907,
            0.00139319897, 0.0125387907, 0.00139319897, 0.0125387907,
            0.00139319897, -0.00975239277, 0.00139319897, 0.00139319897,
            -0.00975239277, 0.0125387907, 0.0125387907, 0.0125387907,
        ];
        let got = dequant_iq1_m(&RAW, 256).unwrap();
        for (i, (&e, &g)) in EXPECT.iter().zip(got.iter()).enumerate() {
            assert!((e - g).abs() <= 1e-6 * e.abs().max(1e-3),
                    "weight {i}: reference {e}, got {g} - a scale field has drifted again");
        }
    }
}
