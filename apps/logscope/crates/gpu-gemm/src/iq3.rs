//! iq3.rs — IQ3_XXS, decoded against the fetched grids.
//!
//! Structurally IQ2_XXS's sibling: same seven-bit sign selectors, same `ksigns_iq2xs` table, same
//! half-offset scale. Three differences, and each is a place a copy of the IQ2_XXS decoder would be
//! wrong while producing plausible output.
//!
//! ```text
//!                      IQ2_XXS                    IQ3_XXS
//! grid entry           8 bytes (u64)              4 bytes (u32)
//! indices per group    4, from one u32            8, read directly from qs
//! scale multiplier     0.25                       0.5
//! ```
//!
//! **The grid entry is half the width**, so one index covers four weights rather than eight and a
//! group needs two lookups where IQ2_XXS needs one. Reading it as a u64 walks off the end of the
//! table for high indices and reads the next entry's bytes for low ones.

use crate::iqgrids::{IQ3XXS_GRID, KSIGNS_IQ2XS, KMASK_IQ2XS};

pub const IQ3_XXS_BLOCK: usize = 98;

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

/// Dequantize IQ3_XXS: 256 weights in 98 bytes — f16 d, 64 index bytes, 32 scale-and-sign bytes.
pub fn dequant_iq3_xxs(raw: &[u8], n: usize) -> Result<Vec<f32>, String> {
    if n % 256 != 0 { return Err(format!("IQ3_XXS needs a multiple of 256 weights, got {n}")) }
    let blocks = n / 256;
    if raw.len() < blocks * IQ3_XXS_BLOCK {
        return Err(format!("IQ3_XXS: {} bytes for {blocks} blocks, need {}",
            raw.len(), blocks * IQ3_XXS_BLOCK));
    }
    let mut out = vec![0.0f32; n];
    for b in 0..blocks {
        let p = b * IQ3_XXS_BLOCK;
        let d = f16(raw[p], raw[p + 1]);
        let qs = &raw[p + 2..p + 66];
        let sas = &raw[p + 66..p + 98];
        for ib32 in 0..8 {
            let a = u32::from_le_bytes([sas[ib32 * 4], sas[ib32 * 4 + 1],
                                        sas[ib32 * 4 + 2], sas[ib32 * 4 + 3]]);
            // 0.5 multiplier, against IQ2_XXS's 0.25. Copying the sibling's constant halves every
            // weight in the file — uniformly, so the model still generates and is quietly wrong.
            let db = d * (0.5 + (a >> 28) as f32) * 0.5;
            for l in 0..4 {
                let signs = KSIGNS_IQ2XS[((a >> (7 * l)) & 127) as usize];
                // TWO lookups per group: the entry is 4 bytes, so one index covers four weights.
                let g1 = IQ3XXS_GRID[qs[ib32 * 8 + 2 * l] as usize].to_le_bytes();
                let g2 = IQ3XXS_GRID[qs[ib32 * 8 + 2 * l + 1] as usize].to_le_bytes();
                let base = b * 256 + ib32 * 32 + l * 8;
                for j in 0..4 {
                    let s1 = if signs & KMASK_IQ2XS[j] != 0 { -1.0 } else { 1.0 };
                    let s2 = if signs & KMASK_IQ2XS[j + 4] != 0 { -1.0 } else { 1.0 };
                    out[base + j] = db * g1[j] as f32 * s1;
                    out[base + 4 + j] = db * g2[j] as f32 * s2;
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
    fn the_GRID_ENTRY_is_FOUR_bytes_not_eight() {
        // Reading it as a u64 walks off the table for high indices and reads the next entry's bytes
        // for low ones. The type is the difference between the two formats.
        assert_eq!(IQ3XXS_GRID.len(), 256);
        assert_eq!(std::mem::size_of_val(&IQ3XXS_GRID[0]), 4);
        assert_eq!(std::mem::size_of_val(&crate::iqgrids::IQ2XXS_GRID[0]), 8, "the sibling's is 8");
    }

    #[test]
    fn the_SCALE_MULTIPLIER_is_0_5_not_IQ2_XXS_s_0_25() {
        // Copying the sibling's constant halves every weight in the file — uniformly, so the model
        // still generates and is quietly wrong, which is the worst available failure.
        let mut raw = vec![0u8; IQ3_XXS_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C;                 // d = 1.0
        let v = dequant_iq3_xxs(&raw, 256).unwrap();
        let g0 = IQ3XXS_GRID[0].to_le_bytes();
        // scale nibble 0 -> db = 1.0 * 0.5 * 0.5 = 0.25
        assert_eq!(v[0], 0.25 * g0[0] as f32);
    }

    #[test]
    fn EIGHT_indices_per_group_not_four() {
        // A group covers 32 weights and each index covers 4, so eight lookups. Four would decode
        // half the block and leave the rest at zero.
        let v = dequant_iq3_xxs(&vec![0u8; IQ3_XXS_BLOCK], 256).unwrap();
        assert_eq!(v.len(), 256);
    }

    #[test]
    fn the_SIGN_table_is_shared_with_IQ2_XXS() {
        // Same seven-bit selectors and the same 128-entry table — one of the few things that IS
        // safe to copy between the two.
        assert_eq!(KSIGNS_IQ2XS.len(), 128);
    }

    #[test]
    fn WRONG_lengths_are_refused() {
        assert!(dequant_iq3_xxs(&[0u8; 4], 256).is_err());
        assert!(dequant_iq3_xxs(&vec![0u8; IQ3_XXS_BLOCK], 300).is_err());
    }
}

/// IQ2_S — 23.4% of a UD-IQ1_S Scout, and the last large piece.
///
/// Unlike the XXS formats it carries **explicit sign bytes** rather than seven-bit selectors into
/// `ksigns_iq2xs`: one byte per eight weights, read directly. That removes the seven-bit trap and
/// adds a different one — the sign bytes live AFTER the indices in the same array, at `qs + 32`,
/// so a decoder that treats `qs` as 64 index bytes reads sign data as indices.
///
/// Two scales per 32-weight group, low nibble then high, each with the familiar half-offset. The
/// grid index is ten bits: eight from `qs` and two from `qh`, which is why `iq2s_grid` has 1024
/// entries where `iq2xxs_grid` has 256.
pub const IQ2_S_BLOCK: usize = 82;

pub fn dequant_iq2_s(raw: &[u8], n: usize) -> Result<Vec<f32>, String> {
    if n % 256 != 0 { return Err(format!("IQ2_S needs a multiple of 256 weights, got {n}")) }
    let blocks = n / 256;
    if raw.len() < blocks * IQ2_S_BLOCK {
        return Err(format!("IQ2_S: {} bytes for {blocks} blocks, need {}",
            raw.len(), blocks * IQ2_S_BLOCK));
    }
    let mut out = vec![0.0f32; n];
    for b in 0..blocks {
        let p = b * IQ2_S_BLOCK;
        let d = f16(raw[p], raw[p + 1]);
        let qs = &raw[p + 2..p + 34];        // 32 index bytes
        let signs = &raw[p + 34..p + 66];    // 32 sign bytes — AFTER the indices, not interleaved
        let qh = &raw[p + 66..p + 74];       // 8 bytes of high index bits
        let scales = &raw[p + 74..p + 82];   // 8 bytes, two 4-bit scales each
        for ib32 in 0..8 {
            let db = [
                d * (0.5 + (scales[ib32] & 0xF) as f32) * 0.25,
                d * (0.5 + (scales[ib32] >> 4) as f32) * 0.25,
            ];
            for l in 0..4 {
                // TEN bits: eight from qs, two from qh. Using eight indexes only the first quarter
                // of a 1024-entry grid, so three quarters of the codebook is never reached and the
                // weights are plausible and wrong.
                let idx = qs[ib32 * 4 + l] as usize
                        | ((((qh[ib32] >> (2 * l)) & 3) as usize) << 8);
                let g = crate::iqgrids::IQ2S_GRID[idx].to_le_bytes();
                let sb = signs[ib32 * 4 + l];
                let base = b * 256 + ib32 * 32 + l * 8;
                for j in 0..8 {
                    let s = if sb & KMASK_IQ2XS[j] != 0 { -1.0 } else { 1.0 };
                    out[base + j] = db[l / 2] * g[j] as f32 * s;
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod iq2s_tests {
    use super::*;

    #[test]
    fn the_GRID_INDEX_is_TEN_bits_and_the_grid_has_1024_entries() {
        // Using eight bits indexes only the first quarter, so three quarters of the codebook is
        // never reached — plausible weights drawn from a codebook a quarter the intended size.
        assert_eq!(crate::iqgrids::IQ2S_GRID.len(), 1024);
        assert_eq!(crate::iqgrids::IQ2XXS_GRID.len(), 256, "the XXS grid for comparison");
    }

    #[test]
    fn SIGN_BYTES_are_explicit_and_come_AFTER_the_indices() {
        // Not seven-bit selectors into ksigns_iq2xs. A decoder that treats qs as 64 index bytes
        // reads sign data as indices, which stays in range and produces garbage.
        let mut raw = vec![0u8; IQ2_S_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C;         // d = 1.0
        raw[74] = 0x01;                        // scale 0 low nibble = 1
        raw[34] = 0xFF;                        // all eight signs NEGATIVE for group 0
        let v = dequant_iq2_s(&raw, 256).unwrap();
        let g = crate::iqgrids::IQ2S_GRID[0].to_le_bytes();
        assert_eq!(v[0], -(1.0 * 1.5 * 0.25) * g[0] as f32);
    }

    #[test]
    fn TWO_scales_per_group_low_nibble_then_high() {
        let mut raw = vec![0u8; IQ2_S_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C;
        raw[74] = 0x30;                        // low nibble 0, high nibble 3
        let v = dequant_iq2_s(&raw, 256).unwrap();
        let g = crate::iqgrids::IQ2S_GRID[0].to_le_bytes();
        assert_eq!(v[0], 0.5 * 0.25 * g[0] as f32, "first half takes the LOW nibble");
        assert_eq!(v[16], 3.5 * 0.25 * g[0] as f32, "second half takes the HIGH nibble");
    }

    #[test]
    fn WRONG_lengths_are_refused() {
        assert!(dequant_iq2_s(&[0u8; 4], 256).is_err());
        assert!(dequant_iq2_s(&vec![0u8; IQ2_S_BLOCK], 300).is_err());
    }
}
