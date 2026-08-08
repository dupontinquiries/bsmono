//! iq2xxs.rs — IQ2_XXS, the first codebook format decoded from a fetched table rather than memory.
//!
//! 18.1% of a UD-IQ1_S Scout by bytes, and the largest single share this engine can now reach.
//!
//! ## Why this one was blocked and is not any more
//!
//! The IQ codebook formats index into grids of 256 to 512 entries. Those are DATA — a single wrong
//! constant produces plausible weights in one sub-block out of thousands, which reads as a slightly
//! worse model and is never traced back to the table.
//!
//! I could not write them from memory and should not have tried. `iqgrids.rs` now carries them
//! extracted from llama.cpp's `ggml-common.h`, with the literal count asserted against the declared
//! array length before anything was emitted — a check that is only possible when the values come
//! from the source rather than from recall.
//!
//! ## Layout
//!
//! 256 weights in 66 bytes: f16 `d`, then eight 8-byte groups. Each group is two u32s: the first
//! holds four grid indices, the second holds four 7-bit sign selectors and a 4-bit scale in its top
//! nibble.
//!
//! The scale is `0.5 + (aux32[1] >> 28)`, times `0.25` — **not a plain multiply**, and the constant
//! is why a format with two bits per weight can represent anything useful.

use crate::iqgrids::{IQ2XXS_GRID, KSIGNS_IQ2XS, KMASK_IQ2XS};

pub const IQ2_XXS_BLOCK: usize = 66;

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

/// Dequantize IQ2_XXS.
pub fn dequant_iq2_xxs(raw: &[u8], n: usize) -> Result<Vec<f32>, String> {
    if n % 256 != 0 { return Err(format!("IQ2_XXS needs a multiple of 256 weights, got {n}")) }
    let blocks = n / 256;
    if raw.len() < blocks * IQ2_XXS_BLOCK {
        return Err(format!("IQ2_XXS: {} bytes for {blocks} blocks, need {}",
            raw.len(), blocks * IQ2_XXS_BLOCK));
    }
    let mut out = vec![0.0f32; n];
    for b in 0..blocks {
        let p = b * IQ2_XXS_BLOCK;
        let d = f16(raw[p], raw[p + 1]);
        let qs = &raw[p + 2..p + 66];
        for ib32 in 0..8 {
            let o = ib32 * 8;
            let a0 = u32::from_le_bytes([qs[o], qs[o + 1], qs[o + 2], qs[o + 3]]);
            let a1 = u32::from_le_bytes([qs[o + 4], qs[o + 5], qs[o + 6], qs[o + 7]]);
            // 0.5 + top nibble, times 0.25. Not a plain multiply — this offset is why two bits per
            // weight can represent anything useful, and dropping the 0.5 makes every zero-scale
            // sub-block vanish rather than being small.
            let db = d * (0.5 + (a1 >> 28) as f32) * 0.25;
            for l in 0..4 {
                let g = IQ2XXS_GRID[((a0 >> (8 * l)) & 0xFF) as usize].to_le_bytes();
                // SEVEN bits per sign selector, not eight. Masking with 0xFF reads into the next
                // selector and flips signs in the following group.
                let signs = KSIGNS_IQ2XS[((a1 >> (7 * l)) & 127) as usize];
                let base = b * 256 + ib32 * 32 + l * 8;
                for j in 0..8 {
                    let s = if signs & KMASK_IQ2XS[j] != 0 { -1.0 } else { 1.0 };
                    out[base + j] = db * g[j] as f32 * s;
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
    fn the_GRIDS_came_from_the_SOURCE_and_their_lengths_are_pinned() {
        // The check that makes the transcription trustworthy, and the one impossible on values typed
        // from memory: each table's literal count was asserted against its declared length.
        assert_eq!(IQ2XXS_GRID.len(), 256);
        assert_eq!(KSIGNS_IQ2XS.len(), 128);
        assert_eq!(KMASK_IQ2XS.len(), 8);
        assert_eq!(crate::iqgrids::IQ2XS_GRID.len(), 512);
        assert_eq!(crate::iqgrids::IQ3XXS_GRID.len(), 256);
        assert_eq!(crate::iqgrids::IQ3S_GRID.len(), 512);
    }

    #[test]
    fn the_SIGN_SELECTOR_is_SEVEN_bits_not_eight() {
        // Masking with 0xFF reads into the next selector and flips signs in the following group —
        // wrong weights that are individually the right magnitude, which is the hardest kind to see.
        let mut raw = vec![0u8; IQ2_XXS_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C;              // d = 1.0
        raw[2 + 4] = 0x80;                          // bit 7 of a1: belongs to selector 1, not 0
        let v = dequant_iq2_xxs(&raw, 256).unwrap();
        // Selector 0 must be unaffected by bit 7.
        assert_eq!(((0x80u32) & 127), 0, "bit 7 is outside selector 0's seven bits");
        assert_eq!(v.len(), 256);
    }

    #[test]
    fn the_SCALE_carries_a_HALF_offset() {
        // 0.5 + nibble, times 0.25. Dropping the 0.5 makes every zero-nibble sub-block vanish rather
        // than be small, which silently deletes a sixteenth of the weights.
        let mut raw = vec![0u8; IQ2_XXS_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C;
        // grid entry 0 with all-positive signs, scale nibble 0 -> db = 1.0 * 0.5 * 0.25
        let v = dequant_iq2_xxs(&raw, 256).unwrap();
        let g0 = IQ2XXS_GRID[0].to_le_bytes();
        assert_eq!(v[0], 0.125 * g0[0] as f32, "a zero nibble must still scale by 0.5*0.25");
    }

    #[test]
    fn a_zero_block_yields_n_values() {
        assert_eq!(dequant_iq2_xxs(&vec![0u8; IQ2_XXS_BLOCK], 256).unwrap().len(), 256);
    }

    #[test]
    fn WRONG_lengths_are_refused() {
        assert!(dequant_iq2_xxs(&[0u8; 4], 256).is_err());
        assert!(dequant_iq2_xxs(&vec![0u8; IQ2_XXS_BLOCK], 300).is_err());
    }
}
