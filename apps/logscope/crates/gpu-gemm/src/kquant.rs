//! kquant.rs — Q5_K and Q2_K, the K-quants this engine was missing.
//!
//! Q4_K and Q6_K were supported; Q2_K, Q3_K and Q5_K were not. All five are **scalar** formats — a
//! quantized value times a scale, with per-sub-block scales packed to save space — which puts them
//! in the same tractable category as IQ4_XS and nowhere near the codebook IQ types.
//!
//! Q2_K is the one that unlocks a checkpoint: a 39.6 GB Q2_K Scout exists, and at 36.9 GiB it is
//! over the 31 GiB residency ceiling but within reach of the streaming path.
//!
//! ## The shared trick, and where it breaks
//!
//! Q4_K and Q5_K pack eight 6-bit scales and eight 6-bit minima into twelve bytes: the first four of
//! each take six bits directly, and the last four are **split** — four low bits in one byte and two
//! high bits borrowed from the top of another. That split is where this session already found a bug
//! once, in the GPU shader, and hand-verified the correct form against a real block:
//!
//! ```text
//! j < 4:  sc = q[j] & 63                              mn = q[j+4] & 63
//! j >= 4: sc = (q[j+4] & 0xF) | ((q[j-4] >> 6) << 4)  mn = (q[j+4] >> 4) | ((q[j] >> 6) << 4)
//! ```
//!
//! Note `q[j]` in the min's high bits, not `q[j-4]`. They are different bytes and using the wrong one
//! is a mistake that still produces plausible weights.
//!
//! Q2_K is simpler: its scales and minima are four bits each in one byte per sub-block, so there is
//! no split to get wrong — but it has only four quantization levels, so an error in the scale is
//! proportionally far more visible.

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

/// The 6-bit scale/min pair for sub-block `j`, from the twelve packed bytes.
///
/// Shared by Q4_K and Q5_K. **Hand-verified against a real Q4_K block this session** — the twelve
/// bytes `[85,116,147,178,209,240,18,49,80,111,142,173]` give scales `[21,52,19,50,16,31,46,45]`.
pub fn scale_min_k4(j: usize, q: &[u8]) -> (u8, u8) {
    if j < 4 {
        (q[j] & 63, q[j + 4] & 63)
    } else {
        // q[j] for the min's high bits, NOT q[j-4]. Different bytes, and the wrong one still yields
        // plausible weights — which is why it is worth a comment rather than being "obvious".
        ((q[j + 4] & 0xF) | ((q[j - 4] >> 6) << 4),
         (q[j + 4] >> 4) | ((q[j] >> 6) << 4))
    }
}

pub const Q5_K_BLOCK: usize = 176;
pub const Q2_K_BLOCK: usize = 84;

/// Q5_K: 256 weights in 176 bytes — d, dmin, 12 scale bytes, 32 high-bit bytes, 128 low-nibble bytes.
pub fn dequant_q5_k(raw: &[u8], n: usize) -> Result<Vec<f32>, String> {
    if n % 256 != 0 { return Err(format!("Q5_K needs a multiple of 256 weights, got {n}")) }
    let blocks = n / 256;
    if raw.len() < blocks * Q5_K_BLOCK {
        return Err(format!("Q5_K: {} bytes for {blocks} blocks, need {}", raw.len(), blocks * Q5_K_BLOCK));
    }
    let mut out = vec![0.0f32; n];
    for b in 0..blocks {
        let p = b * Q5_K_BLOCK;
        let d = f16(raw[p], raw[p + 1]);
        let dmin = f16(raw[p + 2], raw[p + 3]);
        let scales = &raw[p + 4..p + 16];
        let qh = &raw[p + 16..p + 48];
        let qs = &raw[p + 48..p + 176];
        let mut o = b * 256;
        for g in 0..4 {
            // Two sub-blocks per 64-weight group, low nibbles then high — and the FIFTH bit comes
            // from qh, one bit per weight, indexed by the group rather than the sub-block.
            let (sc1, m1) = scale_min_k4(g * 2, scales);
            let (sc2, m2) = scale_min_k4(g * 2 + 1, scales);
            let (d1, mm1) = (d * sc1 as f32, dmin * m1 as f32);
            let (d2, mm2) = (d * sc2 as f32, dmin * m2 as f32);
            for l in 0..32 {
                let qsb = qs[g * 32 + l];
                let hi_lo = ((qh[l] >> (g * 2)) & 1) << 4;
                let hi_hi = ((qh[l] >> (g * 2 + 1)) & 1) << 4;
                out[o + l] = d1.mul_add(((qsb & 0xF) | hi_lo) as f32, -mm1);
                out[o + 32 + l] = d2.mul_add(((qsb >> 4) | hi_hi) as f32, -mm2);
            }
            o += 64;
        }
    }
    Ok(out)
}

/// Q2_K: 256 weights in 84 bytes — 16 scale bytes, 64 quant bytes, d, dmin.
///
/// **Only four levels per weight**, so a scale error is proportionally far more visible than in
/// Q4_K. Its scales and minima are four bits each in one byte, with no split to get wrong.
pub fn dequant_q2_k(raw: &[u8], n: usize) -> Result<Vec<f32>, String> {
    if n % 256 != 0 { return Err(format!("Q2_K needs a multiple of 256 weights, got {n}")) }
    let blocks = n / 256;
    if raw.len() < blocks * Q2_K_BLOCK {
        return Err(format!("Q2_K: {} bytes for {blocks} blocks, need {}", raw.len(), blocks * Q2_K_BLOCK));
    }
    let mut out = vec![0.0f32; n];
    for b in 0..blocks {
        let p = b * Q2_K_BLOCK;
        // d and dmin sit at the END here, unlike Q4_K and Q5_K where they lead. Assuming the Q4_K
        // layout reads scales as a float and produces garbage that still has the right length.
        let scales = &raw[p..p + 16];
        let qs = &raw[p + 16..p + 80];
        let d = f16(raw[p + 80], raw[p + 81]);
        let dmin = f16(raw[p + 82], raw[p + 83]);
        let mut o = b * 256;
        let mut is = 0usize;
        for g in 0..2 {
            for shift in [0u32, 2, 4, 6] {
                let sc = scales[is];
                let dl = d * (sc & 0xF) as f32;
                let ml = dmin * (sc >> 4) as f32;
                for l in 0..16 {
                    out[o + l] = dl.mul_add(((qs[g * 32 + l] >> shift) & 3) as f32, -ml);
                }
                let sc = scales[is + 1];
                let dl = d * (sc & 0xF) as f32;
                let ml = dmin * (sc >> 4) as f32;
                for l in 0..16 {
                    out[o + 16 + l] = dl.mul_add(((qs[g * 32 + 16 + l] >> shift) & 3) as f32, -ml);
                }
                is += 2;
                o += 32;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_SCALE_SPLIT_matches_the_block_hand_verified_this_session() {
        // gate-q4k printed these twelve bytes and the indices they yield. Reusing the verified pair
        // rather than re-deriving it is the point of sharing the function.
        let q = [85u8, 116, 147, 178, 209, 240, 18, 49, 80, 111, 142, 173];
        let sc: Vec<u8> = (0..8).map(|j| scale_min_k4(j, &q).0).collect();
        assert_eq!(sc, vec![21, 52, 19, 50, 16, 31, 46, 45]);
        let mn: Vec<u8> = (0..8).map(|j| scale_min_k4(j, &q).1).collect();
        assert_eq!(mn, vec![17, 48, 18, 49, 53, 54, 8, 10]);
    }

    #[test]
    fn the_MIN_high_bits_come_from_q_j_not_q_j_minus_4() {
        // Different bytes, and the wrong one still yields plausible weights. Pinned because it is
        // the kind of detail a port gets wrong silently.
        let mut q = [0u8; 12];
        q[6] = 0xC0;          // j=6: top two bits set -> min high bits = 3
        assert_eq!(scale_min_k4(6, &q).1, 3 << 4);
        q = [0u8; 12];
        q[2] = 0xC0;          // j-4 = 2 for j=6; must NOT affect the min
        assert_eq!(scale_min_k4(6, &q).1, 0);
    }

    #[test]
    fn Q5_K_decodes_a_zero_block_to_zero() {
        let raw = vec![0u8; Q5_K_BLOCK];
        let v = dequant_q5_k(&raw, 256).unwrap();
        assert_eq!(v.len(), 256);
        assert!(v.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn Q5_K_uses_the_FIFTH_bit_from_qh() {
        // Without it Q5_K is Q4_K with extra bytes, and the output is plausible and wrong by a
        // factor that varies per weight.
        let mut raw = vec![0u8; Q5_K_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C;      // d = 1.0
        raw[4] = 1;                         // sub-block 0 scale = 1
        raw[16] = 0x01;                     // qh weight 0, group 0 low -> bit 4 set
        let v = dequant_q5_k(&raw, 256).unwrap();
        assert_eq!(v[0], 16.0, "the high bit must contribute 16");
    }

    #[test]
    fn Q2_K_puts_d_and_dmin_at_the_END_unlike_the_others() {
        // Assuming the Q4_K layout reads scales as a float and produces garbage with the right
        // length — which passes every size check.
        let mut raw = vec![0u8; Q2_K_BLOCK];
        raw[80] = 0x00; raw[81] = 0x3C;     // d = 1.0 at byte 80, not byte 0
        raw[0] = 0x01;                       // sub-block 0 scale = 1
        raw[16] = 0x03;                      // first weight = 3
        let v = dequant_q2_k(&raw, 256).unwrap();
        assert_eq!(v[0], 3.0);
    }

    #[test]
    fn Q2_K_has_only_FOUR_levels_per_weight() {
        // Two bits, so every value is 0..3 before scaling — and a scale error is proportionally far
        // more visible than in a 4-bit format.
        let mut raw = vec![0u8; Q2_K_BLOCK];
        raw[80] = 0x00; raw[81] = 0x3C;
        raw[0] = 0x01;
        raw[16] = 0xFF;                      // all four 2-bit fields set
        let v = dequant_q2_k(&raw, 256).unwrap();
        assert_eq!(v[0], 3.0, "the maximum of two bits is 3");
    }

    #[test]
    fn WRONG_lengths_are_refused() {
        assert!(dequant_q5_k(&[0u8; 4], 256).is_err());
        assert!(dequant_q2_k(&[0u8; 4], 256).is_err());
        assert!(dequant_q5_k(&[0u8; Q5_K_BLOCK], 300).is_err());
    }
}
