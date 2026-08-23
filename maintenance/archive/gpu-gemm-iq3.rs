// ARCHIVED: IQ3_XXS / IQ3_S / IQ2_S dequantisation, and twelve tests pinning the
//           IQ3_XXS block format that its successor never restated.
// ORIGIN: apps/logscope/crates/gpu-gemm/src/iq3.rs
// REMOVED: 0c599e55 2026-08-22
// SUPERSEDED-BY: apps/logscope/crates/gpu-gemm/src/iq23.rs — same six public items,
//           covering IQ3_XXS *and* IQ2_S. gguf.rs dispatches ggml types 18, 21 and 22
//           to iq23; nothing ever called this file.
// LAST-KNOWN-GOOD: its own 14 tests passed at 9d00d500 (2026-08-22), the last suite
//           run before removal. NEVER CONFIRMED against a real IQ3_S or IQ2_S
//           checkpoint — no such file exists in this tree, and the tests are
//           synthetic blocks. Treat "tests passed" and "decodes a real model"
//           as the different claims they are.
//
// WHY IT IS WORTH KEEPING, given the successor is strictly wider:
//
// `dequant_iq3_xxs` was byte-identical between the two, but `dequant_iq3_s` and
// `dequant_iq2_s` had DRIFTED — this file's versions are the older answer, and
// nothing had run them in a long time. So the CODE here is not the thing to
// revive.
//
// The TESTS are. Four of them asserted properties iq23 did not, and those were
// moved across before deletion (the 8-byte IQ2_XXS grid entry against IQ3_XXS's
// 4, the shared 128-entry sign table, the 1024-entry IQ2_S grid against
// IQ2_XXS's 256, and IQ2_S packing two scales per group low-nibble-first). The
// remaining eight overlap iq23's own twelve but say it differently — in terms
// of the bit layout rather than the decoded values — and that phrasing is the
// clearest description of the IQ3_XXS format in this repository.
//
// If a future IQ3 bug needs the format spelled out, read from here. Do not
// copy the implementation back without diffing it against iq23 first.
//
// ─────────────────────────────────────────────────────────────────────────────

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

/// IQ3_S — the last unsupported type in a UD-IQ1_S Scout, at 24% of its bytes.
///
/// ## Its scale is unlike every other IQ format here
///
/// `1 + 2*nibble`, with **no half-offset and no 0.25 or 0.5 multiplier**. Every other format in this
/// file is `(0.5 + nibble) * k`. Carrying the familiar form over gives a scale that is wrong by a
/// factor between 2 and 4 depending on the nibble — non-uniform, so it distorts the weight
/// distribution rather than merely shrinking it, which is worse than IQ3_XXS's uniform halving.
///
/// ## The ninth index bit
///
/// The grid has 512 entries, so an index is nine bits: eight from `qs`, one from `qh`. That bit is
/// extracted by a SHIFT THAT VARIES WITH POSITION — `(qh[b] << (8 - 2*l)) & 256` for the even index
/// and `(7 - 2*l)` for the odd — which packs eight index bits into one byte at alternating offsets.
///
/// A fixed shift reads the same bit for every `l`, so a quarter of the entries index into the wrong
/// half of the grid. In range, and wrong.
pub const IQ3_S_BLOCK: usize = 110;

pub fn dequant_iq3_s(raw: &[u8], n: usize) -> Result<Vec<f32>, String> {
    if n % 256 != 0 { return Err(format!("IQ3_S needs a multiple of 256 weights, got {n}")) }
    let blocks = n / 256;
    if raw.len() < blocks * IQ3_S_BLOCK {
        return Err(format!("IQ3_S: {} bytes for {blocks} blocks, need {}",
            raw.len(), blocks * IQ3_S_BLOCK));
    }
    let grid = &crate::iqgrids::IQ3S_GRID;
    let mut out = vec![0.0f32; n];
    for b in 0..blocks {
        let p = b * IQ3_S_BLOCK;
        let d = f16(raw[p], raw[p + 1]);
        let qs = &raw[p + 2..p + 66];        // 64 index bytes
        let qh = &raw[p + 66..p + 74];       // 8 bytes, one ninth-bit each per 32-weight group
        let signs = &raw[p + 74..p + 106];   // 32 explicit sign bytes
        let scales = &raw[p + 106..p + 110]; // 4 bytes, two nibbles each
        let mut o = b * 256;
        for pair in 0..4 {
            // 1 + 2*nibble. NOT (0.5 + nibble) * k — the form every other IQ format here uses.
            let db = [d * (1.0 + 2.0 * (scales[pair] & 0xF) as f32),
                      d * (1.0 + 2.0 * (scales[pair] >> 4) as f32)];
            for half in 0..2 {
                let qo = pair * 16 + half * 8;
                let so = pair * 8 + half * 4;
                let hb = qh[pair * 2 + half] as usize;
                for l in 0..4 {
                    // Position-varying shift: the ninth bit for index 2l comes from a different
                    // offset than for 2l+1. A fixed shift reads one bit for every l and sends a
                    // quarter of the entries into the wrong half of a 512-entry grid.
                    let i1 = qs[qo + 2 * l] as usize | ((hb << (8 - 2 * l)) & 256);
                    let i2 = qs[qo + 2 * l + 1] as usize | ((hb << (7 - 2 * l)) & 256);
                    let (g1, g2) = (grid[i1].to_le_bytes(), grid[i2].to_le_bytes());
                    let sb = signs[so + l];
                    for j in 0..4 {
                        let s1 = if sb & KMASK_IQ2XS[j] != 0 { -1.0 } else { 1.0 };
                        let s2 = if sb & KMASK_IQ2XS[j + 4] != 0 { -1.0 } else { 1.0 };
                        out[o + j] = db[half] * g1[j] as f32 * s1;
                        out[o + 4 + j] = db[half] * g2[j] as f32 * s2;
                    }
                    o += 8;
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod iq3s_tests {
    use super::*;

    #[test]
    fn the_SCALE_form_is_1_PLUS_2N_unlike_every_other_IQ_format_here() {
        // Others are (0.5 + nibble) * k. Carrying that form over gives a scale wrong by a factor
        // between 2 and 4 depending on the nibble — NON-UNIFORM, so it distorts the weight
        // distribution rather than merely shrinking it.
        let mut raw = vec![0u8; IQ3_S_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C;              // d = 1.0
        raw[106] = 0x00;                            // nibble 0 -> db = 1 + 0 = 1.0
        let v = dequant_iq3_s(&raw, 256).unwrap();
        let g = crate::iqgrids::IQ3S_GRID[0].to_le_bytes();
        assert_eq!(v[0], 1.0 * g[0] as f32, "a zero nibble scales by 1, not by 0.5*k");
        raw[106] = 0x03;                            // nibble 3 -> 1 + 6 = 7.0
        let v = dequant_iq3_s(&raw, 256).unwrap();
        assert_eq!(v[0], 7.0 * g[0] as f32);
    }

    #[test]
    fn the_NINTH_index_bit_uses_a_POSITION_VARYING_shift() {
        // A fixed shift reads the same bit for every l, sending a quarter of the entries into the
        // wrong half of a 512-entry grid — in range, and wrong.
        assert_eq!(crate::iqgrids::IQ3S_GRID.len(), 512);
        // l=0 takes bit 0 via <<8, l=1 takes bit 2 via <<6, and so on for the even indices.
        for l in 0..4usize {
            let only_bit = 1usize << (2 * l);
            assert_eq!((only_bit << (8 - 2 * l)) & 256, 256, "bit {} must reach the 9th place", 2 * l);
        }
    }

    #[test]
    fn the_ODD_index_takes_a_DIFFERENT_bit_from_the_even_one() {
        // (8 - 2l) against (7 - 2l). Using one shift for both halves duplicates the ninth bit and
        // pairs of weights index the same half of the grid.
        for l in 0..4usize {
            let odd_bit = 1usize << (2 * l + 1);
            assert_eq!((odd_bit << (7 - 2 * l)) & 256, 256);
            assert_eq!((odd_bit << (8 - 2 * l)) & 256, 0, "the even shift must NOT catch it");
        }
    }

    #[test]
    fn a_zero_block_yields_n_values() {
        assert_eq!(dequant_iq3_s(&vec![0u8; IQ3_S_BLOCK], 256).unwrap().len(), 256);
    }

    #[test]
    fn WRONG_lengths_are_refused() {
        assert!(dequant_iq3_s(&[0u8; 4], 256).is_err());
        assert!(dequant_iq3_s(&vec![0u8; IQ3_S_BLOCK], 300).is_err());
    }
}
