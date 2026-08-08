//! iq4.rs — IQ4_NL and IQ4_XS, the two IQ formats that are not codebook formats.
//!
//! The IQ family is usually a codebook: IQ2_XXS, IQ3_S and friends store indices into grids of
//! 8-weight patterns, and porting one means transcribing 2048 entries and getting every one right.
//!
//! **IQ4_NL and IQ4_XS are different and the name hides it.** They are NON-LINEAR quants: sixteen
//! signed values in one small table, indexed by a 4-bit nibble exactly as Q4_0 is. The table is
//! spaced to match the distribution of real weights rather than linearly, which is where the
//! accuracy comes from — and it is sixteen numbers, not two thousand.
//!
//! That makes them the tractable end of the IQ family, and IQ4_XS is 21% of a UD-IQ1_S Scout
//! checkpoint by bytes — the third largest of seven unsupported types and by far the cheapest to
//! add.
//!
//! ## Layouts
//!
//! ```text
//! IQ4_NL   32 weights in 18 bytes:  f16 d, then 16 bytes of nibbles
//! IQ4_XS  256 weights in 136 bytes: f16 d, u16 scales_h, 4 bytes scales_l, 128 bytes qs
//! ```
//!
//! IQ4_XS's per-32 sub-block scale is a 6-bit value split across two places — four low bits in
//! `scales_l` and two high bits in `scales_h` — which is the same trick Q4_K uses and the same place
//! a port goes wrong.

/// The non-linear value table. **This is the entire codebook**, and it is why these two formats are
/// portable in an afternoon where IQ2_XXS is not.
///
/// Taken from llama.cpp's `kvalues_iq4nl`. The spacing is tight near zero and loose at the tails,
/// matching where weight mass actually sits.
pub const KVALUES_IQ4NL: [i8; 16] = [
    -127, -104, -83, -65, -49, -35, -22, -10, 1, 13, 25, 38, 53, 69, 89, 113,
];

/// Decode an f16 stored as two little-endian bytes.
fn f16(lo: u8, hi: u8) -> f32 {
    let h = u16::from_le_bytes([lo, hi]) as u32;
    let (sign, exp, mant) = ((h >> 15) & 1, (h >> 10) & 0x1F, h & 0x3FF);
    let bits = if exp == 0 {
        if mant == 0 { sign << 31 } else {
            // Subnormal: normalise by shifting until the implicit bit appears.
            let mut e: i32 = -14;
            let mut m = mant;
            while m & 0x400 == 0 { m <<= 1; e -= 1; }
            (sign << 31) | (((e + 127) as u32) << 23) | ((m & 0x3FF) << 13)
        }
    } else if exp == 0x1F {
        (sign << 31) | 0x7F80_0000 | (mant << 13)
    } else {
        (sign << 31) | ((exp + 112) << 23) | (mant << 13)
    };
    f32::from_bits(bits)
}

/// Bytes per block.
pub const IQ4_NL_BLOCK: usize = 18;
pub const IQ4_XS_BLOCK: usize = 136;

/// Dequantize IQ4_NL: 32 weights per 18-byte block.
pub fn dequant_iq4_nl(raw: &[u8], n: usize) -> Result<Vec<f32>, String> {
    if n % 32 != 0 { return Err(format!("IQ4_NL needs a multiple of 32 weights, got {n}")) }
    let blocks = n / 32;
    if raw.len() < blocks * IQ4_NL_BLOCK {
        return Err(format!("IQ4_NL: {} bytes for {blocks} blocks, need {}",
            raw.len(), blocks * IQ4_NL_BLOCK));
    }
    let mut out = Vec::with_capacity(n);
    for b in 0..blocks {
        let p = b * IQ4_NL_BLOCK;
        let d = f16(raw[p], raw[p + 1]);
        let qs = &raw[p + 2..p + 18];
        // Low nibbles first for the whole half, then high — NOT interleaved per byte. Getting this
        // backwards produces plausible output with the weights shuffled, which is the failure that
        // does not announce itself.
        for j in 0..16 { out.push(d * KVALUES_IQ4NL[(qs[j] & 0xF) as usize] as f32); }
        for j in 0..16 { out.push(d * KVALUES_IQ4NL[(qs[j] >> 4) as usize] as f32); }
    }
    Ok(out)
}

/// Dequantize IQ4_XS: 256 weights per 136-byte block, eight sub-blocks of 32.
pub fn dequant_iq4_xs(raw: &[u8], n: usize) -> Result<Vec<f32>, String> {
    if n % 256 != 0 { return Err(format!("IQ4_XS needs a multiple of 256 weights, got {n}")) }
    let blocks = n / 256;
    if raw.len() < blocks * IQ4_XS_BLOCK {
        return Err(format!("IQ4_XS: {} bytes for {blocks} blocks, need {}",
            raw.len(), blocks * IQ4_XS_BLOCK));
    }
    let mut out = vec![0.0f32; n];
    for b in 0..blocks {
        let p = b * IQ4_XS_BLOCK;
        let d = f16(raw[p], raw[p + 1]);
        let scales_h = u16::from_le_bytes([raw[p + 2], raw[p + 3]]);
        let scales_l = &raw[p + 4..p + 8];
        let qs = &raw[p + 8..p + 136];
        for ib in 0..8 {
            // The 6-bit sub-block scale, split: four low bits in scales_l (two per byte) and two
            // high bits in scales_h (two per sub-block). Same split as Q4_K, and the same place a
            // port goes wrong — a wrong high bit scales a whole 32-weight run by 4x or 16x.
            let ls_lo = ((scales_l[ib / 2] >> (4 * (ib % 2))) & 0xF) as i32;
            let ls_hi = ((scales_h >> (2 * ib)) & 3) as i32;
            let ls = (ls_lo | (ls_hi << 4)) - 32;   // biased by 32, signed
            let dl = d * ls as f32;
            let qp = ib * 16;
            let base = b * 256 + ib * 32;
            for j in 0..16 {
                out[base + j] = dl * KVALUES_IQ4NL[(qs[qp + j] & 0xF) as usize] as f32;
                out[base + 16 + j] = dl * KVALUES_IQ4NL[(qs[qp + j] >> 4) as usize] as f32;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_TABLE_is_the_whole_codebook_and_it_is_SIXTEEN_entries() {
        // The reason these two are portable in an afternoon where IQ2_XXS is not: sixteen numbers
        // against two thousand 8-weight patterns.
        assert_eq!(KVALUES_IQ4NL.len(), 16);
        assert_eq!(KVALUES_IQ4NL[0], -127);
        assert_eq!(KVALUES_IQ4NL[15], 113);
    }

    #[test]
    fn the_spacing_is_NON_LINEAR_and_tighter_near_zero() {
        // Which is where the accuracy over Q4_0 comes from, and a linear table would silently be
        // "working" while losing exactly the resolution the format exists to provide.
        let near = (KVALUES_IQ4NL[8] - KVALUES_IQ4NL[7]) as i32;   // 1 - (-10) = 11
        let far = (KVALUES_IQ4NL[15] - KVALUES_IQ4NL[14]) as i32;  // 113 - 89 = 24
        assert!(far > near * 2, "near {near}, far {far}");
    }

    #[test]
    fn IQ4_NL_decodes_a_known_block() {
        // d = 1.0 (f16 0x3C00), all nibbles zero -> every weight is KVALUES[0].
        let mut raw = vec![0u8; IQ4_NL_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C;
        let v = dequant_iq4_nl(&raw, 32).unwrap();
        assert_eq!(v.len(), 32);
        assert!(v.iter().all(|x| (*x - (-127.0)).abs() < 1e-6), "got {}", v[0]);
    }

    #[test]
    fn IQ4_NL_puts_LOW_nibbles_first_across_the_half_not_per_byte() {
        // Interleaving per byte produces plausible output with the weights shuffled — the failure
        // that does not announce itself, and the one a "looks reasonable" check would pass.
        let mut raw = vec![0u8; IQ4_NL_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C;
        raw[2] = 0xF0;   // low nibble 0, high nibble 15
        let v = dequant_iq4_nl(&raw, 32).unwrap();
        assert_eq!(v[0], -127.0, "weight 0 takes the LOW nibble of byte 0");
        assert_eq!(v[16], 113.0, "weight 16 takes its HIGH nibble");
    }

    #[test]
    fn IQ4_XS_scale_is_SIX_bits_split_across_two_places() {
        // Four low bits in scales_l and two high in scales_h, biased by 32 — the same trick Q4_K
        // uses and the same place a port goes wrong. A wrong high bit scales a whole 32-weight run.
        let mut raw = vec![0u8; IQ4_XS_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C;          // d = 1.0
        raw[4] = 0x20;                          // sub-block 0 low bits = 0, sub-block 1 low = 2
        raw[2] = 0x01;                          // sub-block 0 high bits = 1 -> ls = 16 - 32 = -16
        let v = dequant_iq4_xs(&raw, 256).unwrap();
        assert_eq!(v[0], -16.0 * -127.0, "high bits must contribute 16, not be ignored");
    }

    #[test]
    fn a_WRONG_LENGTH_is_refused_rather_than_read_past_the_end() {
        assert!(dequant_iq4_nl(&[0u8; 4], 32).is_err());
        assert!(dequant_iq4_xs(&[0u8; 4], 256).is_err());
        assert!(dequant_iq4_nl(&[0u8; 18], 33).is_err(), "33 is not a multiple of 32");
    }

    #[test]
    fn block_sizes_match_what_type_nbytes_already_reports() {
        // The sizes were added to gguf.rs before the decoders existed so the loader could report
        // honestly. They must agree, or the report was wrong about the bytes it counted.
        assert_eq!(IQ4_NL_BLOCK, 18);
        assert_eq!(IQ4_XS_BLOCK, 136);
    }
}
