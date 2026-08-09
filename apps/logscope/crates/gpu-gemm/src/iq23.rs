//! iq23.rs — IQ3_XXS and IQ2_S, decoded against the fetched grids.
//!
//! Both follow IQ2_XXS's shape and differ in where the bits come from. Writing them together makes
//! the differences visible rather than buried in two files that look alike.
//!
//! ```text
//! IQ2_XXS  66 B   one 8-byte grid entry per 8 weights; signs and scale packed in a u32
//! IQ3_XXS  98 B   TWO 4-byte entries per 8 weights, from a 256-entry u32 grid
//! IQ2_S    82 B   grid index is 8 bits PLUS 2 from qh; signs are their own byte array
//! ```
//!
//! The scale multipliers differ and are not interchangeable: **IQ2_XXS and IQ2_S use `* 0.25`,
//! IQ3_XXS uses `* 0.5`.** Copying the wrong one halves or doubles every weight in the tensor, which
//! a model survives — it produces coherent, worse output, which is the failure this family keeps
//! presenting.

use crate::iqgrids::{IQ3XXS_GRID, IQ2S_GRID, KSIGNS_IQ2XS, KMASK_IQ2XS};

pub const IQ3_XXS_BLOCK: usize = 98;
pub const IQ2_S_BLOCK: usize = 82;

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

/// IQ3_XXS: 256 weights in 98 bytes — f16 d, 64 bytes qs, 32 bytes of packed scales and signs.
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
        let sas = &raw[p + 66..p + 98];   // scales and signs share these 32 bytes
        for ib32 in 0..8 {
            let a = u32::from_le_bytes([sas[ib32 * 4], sas[ib32 * 4 + 1],
                                        sas[ib32 * 4 + 2], sas[ib32 * 4 + 3]]);
            // * 0.5 here, against IQ2_XXS's * 0.25. Copying the wrong constant doubles every weight
            // in the tensor and the model still produces coherent, worse output.
            let db = d * (0.5 + (a >> 28) as f32) * 0.5;
            for l in 0..4 {
                let signs = KSIGNS_IQ2XS[((a >> (7 * l)) & 127) as usize];
                // TWO grid entries of four bytes each, not one of eight.
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

/// IQ2_S: 256 weights in 82 bytes — 32 qs, 8 qh, 32 signs, 8 scales, f16 d.
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
        let qs = &raw[p + 2..p + 34];
        let qh = &raw[p + 34..p + 42];
        let signs = &raw[p + 42..p + 74];
        let scales = &raw[p + 74..p + 82];
        for ib32 in 0..8 {
            // TWO scales per 32 weights, low nibble then high — where IQ2_XXS has one per group.
            let db = [d * (0.5 + (scales[ib32] & 0xF) as f32) * 0.25,
                      d * (0.5 + (scales[ib32] >> 4) as f32) * 0.25];
            for l in 0..4 {
                let dl = db[l / 2];
                // The grid index is 8 bits from qs PLUS 2 from qh — a 1024-entry grid, so dropping
                // the qh bits silently confines every lookup to the first quarter of it.
                let idx = qs[ib32 * 4 + l] as usize
                        | ((((qh[ib32] >> (2 * l)) & 3) as usize) << 8);
                let g = IQ2S_GRID[idx].to_le_bytes();
                let sg = signs[ib32 * 4 + l];
                let base = b * 256 + ib32 * 32 + l * 8;
                for j in 0..8 {
                    let s = if sg & KMASK_IQ2XS[j] != 0 { -1.0 } else { 1.0 };
                    out[base + j] = dl * g[j] as f32 * s;
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
    fn the_SCALE_MULTIPLIERS_differ_between_the_two() {
        // IQ3_XXS uses 0.5 and IQ2_S uses 0.25. Copying the wrong one halves or doubles every weight
        // in the tensor — which a model survives, producing coherent worse output.
        let mut x = vec![0u8; IQ3_XXS_BLOCK];
        x[0] = 0x00; x[1] = 0x3C;                       // d = 1.0
        let vx = dequant_iq3_xxs(&x, 256).unwrap();
        let mut s = vec![0u8; IQ2_S_BLOCK];
        s[0] = 0x00; s[1] = 0x3C;
        let vs = dequant_iq2_s(&s, 256).unwrap();
        let g3 = IQ3XXS_GRID[0].to_le_bytes();
        let g2 = IQ2S_GRID[0].to_le_bytes();
        assert_eq!(vx[0], 0.25 * g3[0] as f32, "IQ3_XXS: 0.5 * 0.5");
        assert_eq!(vs[0], 0.125 * g2[0] as f32, "IQ2_S: 0.5 * 0.25");
    }

    #[test]
    fn IQ2_S_uses_qh_to_reach_the_WHOLE_1024_entry_grid() {
        // Dropping the two qh bits confines every lookup to the first quarter — a working model that
        // has quietly lost three quarters of its codebook.
        assert_eq!(IQ2S_GRID.len(), 1024);
        let mut raw = vec![0u8; IQ2_S_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C;
        raw[34] = 0b11;                                  // qh sub-block 0, l=0 -> +3<<8
        let v = dequant_iq2_s(&raw, 256).unwrap();
        let expect = IQ2S_GRID[3 << 8].to_le_bytes();
        assert_eq!(v[0], 0.125 * expect[0] as f32, "the qh bits must reach index 768");
    }

    #[test]
    fn IQ3_XXS_reads_TWO_grid_entries_per_eight_weights() {
        // Four bytes each from a u32 grid, where IQ2_XXS takes one eight-byte entry from a u64 grid.
        assert_eq!(IQ3XXS_GRID.len(), 256);
        assert_eq!(std::mem::size_of_val(&IQ3XXS_GRID[0]), 4);
    }

    #[test]
    fn both_yield_n_values_and_refuse_wrong_lengths() {
        assert_eq!(dequant_iq3_xxs(&vec![0u8; IQ3_XXS_BLOCK], 256).unwrap().len(), 256);
        assert_eq!(dequant_iq2_s(&vec![0u8; IQ2_S_BLOCK], 256).unwrap().len(), 256);
        assert!(dequant_iq3_xxs(&[0u8; 4], 256).is_err());
        assert!(dequant_iq2_s(&[0u8; 4], 256).is_err());
        assert!(dequant_iq2_s(&vec![0u8; IQ2_S_BLOCK], 300).is_err());
    }

    #[test]
    fn the_BLOCK_SIZES_match_what_type_nbytes_reports() {
        assert_eq!(crate::gguf::type_nbytes(18, 256), Some(IQ3_XXS_BLOCK));
        assert_eq!(crate::gguf::type_nbytes(22, 256), Some(IQ2_S_BLOCK));
    }
}

pub const IQ3_S_BLOCK: usize = 110;

/// IQ3_S: 256 weights in 110 bytes — f16 d, 64 qs, 8 qh, 32 signs, 4 scales.
///
/// **The last type standing between a UD-IQ1_S Scout and loading**, at 24% of that file's
/// previously-unsupported bytes.
///
/// Two things set it apart from its siblings:
///
/// * **The scale has no half-offset.** It is `d * (1 + 2*nibble)`, not `d * (0.5 + nibble) * k`.
///   Applying IQ2_S's form here scales every weight by roughly a quarter and the model still speaks.
/// * **The ninth grid bit comes from a SHIFTED mask, not a shift-and-mask.** `(qh << (8-2l)) & 256`
///   moves a different bit into place for each `l`, and the two grid lookups in a pair use shifts
///   one apart — 8-2l and 7-2l. Writing the natural `(qh >> l) & 1` gets the right shape and the
///   wrong bit.
pub fn dequant_iq3_s(raw: &[u8], n: usize) -> Result<Vec<f32>, String> {
    use crate::iqgrids::IQ3S_GRID;
    if n % 256 != 0 { return Err(format!("IQ3_S needs a multiple of 256 weights, got {n}")) }
    let blocks = n / 256;
    if raw.len() < blocks * IQ3_S_BLOCK {
        return Err(format!("IQ3_S: {} bytes for {blocks} blocks, need {}",
            raw.len(), blocks * IQ3_S_BLOCK));
    }
    let mut out = vec![0.0f32; n];
    for b in 0..blocks {
        let p = b * IQ3_S_BLOCK;
        let d = f16(raw[p], raw[p + 1]);
        let qs = &raw[p + 2..p + 66];
        let qh = &raw[p + 66..p + 74];
        let signs = &raw[p + 74..p + 106];
        let scales = &raw[p + 106..p + 110];
        let mut o = b * 256;
        for pair in 0..4 {
            // No half-offset: 1 + 2*nibble. IQ2_S's (0.5 + n) * 0.25 here scales everything by about
            // a quarter and the model still speaks.
            let db1 = d * (1.0 + 2.0 * (scales[pair] & 0xF) as f32);
            let db2 = d * (1.0 + 2.0 * (scales[pair] >> 4) as f32);
            for half in 0..2 {
                let db = if half == 0 { db1 } else { db2 };
                let qh_b = qh[pair * 2 + half] as usize;
                // EIGHT qs bytes per group, not sixteen. 256 weights / 8 per grid entry = 32
                // entries, two per l, four l per group = 8 groups x 8 bytes = 64, which is exactly
                // the qs length. My first version stepped by 16 and ran off the end at group 4 —
                // caught by an index panic rather than by producing wrong weights, which is the
                // lucky version of this mistake.
                let qbase = (pair * 2 + half) * 8;
                let sbase = (pair * 2 + half) * 4;
                for l in 0..4 {
                    // Shifted mask, not shift-and-mask, and the pair's two lookups differ by one.
                    let i1 = qs[qbase + 2 * l] as usize | ((qh_b << (8 - 2 * l)) & 256);
                    let i2 = qs[qbase + 2 * l + 1] as usize | ((qh_b << (7 - 2 * l)) & 256);
                    let g1 = IQ3S_GRID[i1].to_le_bytes();
                    let g2 = IQ3S_GRID[i2].to_le_bytes();
                    let sg = signs[sbase + l];
                    for j in 0..4 {
                        let s1 = if sg & KMASK_IQ2XS[j] != 0 { -1.0 } else { 1.0 };
                        let s2 = if sg & KMASK_IQ2XS[j + 4] != 0 { -1.0 } else { 1.0 };
                        out[o + l * 8 + j] = db * g1[j] as f32 * s1;
                        out[o + l * 8 + 4 + j] = db * g2[j] as f32 * s2;
                    }
                }
                o += 32;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod iq3s_tests {
    use super::*;
    use crate::iqgrids::IQ3S_GRID;

    #[test]
    fn IQ3_S_has_NO_half_offset_in_its_scale() {
        // 1 + 2*nibble, not (0.5 + nibble) * k. Applying a sibling's form scales every weight by
        // about a quarter and the model still speaks — coherent and wrong.
        let mut raw = vec![0u8; IQ3_S_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C;                    // d = 1.0, scale nibble 0
        let v = dequant_iq3_s(&raw, 256).unwrap();
        let g = IQ3S_GRID[0].to_le_bytes();
        assert_eq!(v[0], 1.0 * g[0] as f32, "a zero nibble gives scale 1, not 0.5 or 0.125");
    }

    #[test]
    fn the_NINTH_GRID_BIT_uses_a_shifted_MASK() {
        // (qh << (8-2l)) & 256, and the pair's second lookup shifts one less. The natural
        // (qh >> l) & 1 gets the right shape and the wrong bit.
        let mut raw = vec![0u8; IQ3_S_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C;
        raw[106] = 0x00;
        raw[66] = 0b0000_0001;                            // qh bit 0 -> l=0's FIRST lookup only
        let v = dequant_iq3_s(&raw, 256).unwrap();
        // 1 << 8 = 256, so grid1 index becomes 256 while grid2 (shift 7) stays 0.
        assert_eq!(v[0], IQ3S_GRID[256].to_le_bytes()[0] as f32);
        assert_eq!(v[4], IQ3S_GRID[0].to_le_bytes()[0] as f32, "the pair's second lookup differs");
    }

    #[test]
    fn IQ3_S_yields_n_and_refuses_wrong_lengths() {
        assert_eq!(dequant_iq3_s(&vec![0u8; IQ3_S_BLOCK], 256).unwrap().len(), 256);
        assert!(dequant_iq3_s(&[0u8; 4], 256).is_err());
        assert!(dequant_iq3_s(&vec![0u8; IQ3_S_BLOCK], 300).is_err());
    }

    #[test]
    fn the_block_size_matches_type_nbytes() {
        assert_eq!(crate::gguf::type_nbytes(21, 256), Some(IQ3_S_BLOCK));
    }
}
