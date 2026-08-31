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
        q5k_block(&mut out[b * 256..b * 256 + 256], &raw[p..p + Q5_K_BLOCK]);
    }
    Ok(out)
}

/// One Q5_K super-block. Split out for the same reason as Q4_K's: `mul_add` is a call into
/// compiler_builtins' `fmaf` on baseline x86-64, and `#[target_feature]` only reaches a body it can
/// inline. Measured 12x slower than llama.cpp's scalar C before this — the worst of the three.
#[inline(always)]
fn q5k_block_core(out: &mut [f32], blk: &[u8]) {
    let d = f16(blk[0], blk[1]);
    let dmin = f16(blk[2], blk[3]);
    let scales = &blk[4..16];
    let qh = &blk[16..48];
    let qs = &blk[48..176];
    for g in 0..4 {
        // Two sub-blocks per 64-weight group, low nibbles then high — and the FIFTH bit comes
        // from qh, one bit per weight, indexed by the group rather than the sub-block.
        let (sc1, m1) = scale_min_k4(g * 2, scales);
        let (sc2, m2) = scale_min_k4(g * 2 + 1, scales);
        let (d1, mm1) = (d * sc1 as f32, dmin * m1 as f32);
        let (d2, mm2) = (d * sc2 as f32, dmin * m2 as f32);
        let q = &qs[g * 32..g * 32 + 32];
        let (lo, hi) = out[g * 64..g * 64 + 64].split_at_mut(32);
        for (((o1, o2), &qsb), &qhb) in lo.iter_mut().zip(hi.iter_mut()).zip(q.iter()).zip(qh.iter()) {
            let hi_lo = ((qhb >> (g * 2)) & 1) << 4;
            let hi_hi = ((qhb >> (g * 2 + 1)) & 1) << 4;
            *o1 = d1.mul_add(((qsb & 0xF) | hi_lo) as f32, -mm1);
            *o2 = d2.mul_add(((qsb >> 4) | hi_hi) as f32, -mm2);
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "fma")]
unsafe fn q5k_block_fma(out: &mut [f32], blk: &[u8]) { q5k_block_core(out, blk) }

fn q5k_block(out: &mut [f32], blk: &[u8]) {
    #[cfg(target_arch = "x86_64")]
    if crate::fmapath::has_fma() {
        // SAFETY: guarded by the runtime detection this line performs.
        return unsafe { q5k_block_fma(out, blk) };
    }
    q5k_block_core(out, blk)
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
        q2k_block(&mut out[b * 256..b * 256 + 256], &raw[p..p + Q2_K_BLOCK]);
    }
    Ok(out)
}

/// One Q2_K super-block, split out for the FMA dispatch (see `q4k_block_core`).
#[inline(always)]
fn q2k_block_core(out: &mut [f32], blk: &[u8]) {
    // d and dmin sit at the END here, unlike Q4_K and Q5_K where they lead. Assuming the Q4_K
    // layout reads scales as a float and produces garbage that still has the right length.
    let scales = &blk[0..16];
    let qs = &blk[16..80];
    let d = f16(blk[80], blk[81]);
    let dmin = f16(blk[82], blk[83]);
    let mut o = 0usize;
    let mut is = 0usize;
    for g in 0..2 {
        for shift in [0u32, 2, 4, 6] {
            for half in 0..2 {
                let sc = scales[is + half];
                let dl = d * (sc & 0xF) as f32;
                let ml = dmin * (sc >> 4) as f32;
                let q = &qs[g * 32 + half * 16..g * 32 + half * 16 + 16];
                let y = &mut out[o + half * 16..o + half * 16 + 16];
                for (dst, &qb) in y.iter_mut().zip(q.iter()) {
                    *dst = dl.mul_add(((qb >> shift) & 3) as f32, -ml);
                }
            }
            is += 2;
            o += 32;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "fma")]
unsafe fn q2k_block_fma(out: &mut [f32], blk: &[u8]) { q2k_block_core(out, blk) }

fn q2k_block(out: &mut [f32], blk: &[u8]) {
    #[cfg(target_arch = "x86_64")]
    if crate::fmapath::has_fma() {
        // SAFETY: guarded by the runtime detection this line performs.
        return unsafe { q2k_block_fma(out, blk) };
    }
    q2k_block_core(out, blk)
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
    fn Q5_K_nibble_LAYOUT_is_low32_then_high32_not_interleaved() {
        // The bug this pins: a second Q5_K decoder (in gguf.rs, now deleted) read the nibbles as
        // interleaved pairs — weight j from qs[j/2], low or high by parity — which is Q4_0's
        // layout. ggml's K-quants put the LOW nibbles of a group's 32 bytes first and the HIGH
        // nibbles of the SAME bytes second. Both decoders produce 256 finite weights; only a
        // NON-uniform fixture separates them, which is why every uniform-fixture test passed over
        // the wrong one while kernel-validate reported max error 18.9 on two machines.
        let mut raw = vec![0u8; Q5_K_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C;      // d = 1.0 (dmin stays 0 so weights are exactly nibbles)
        raw[4] = 1; raw[5] = 1;             // sub-blocks 0 and 1: scale = 1
        raw[48] = 0x52;                     // group 0, byte 0: low nibble 2, high nibble 5
        raw[49] = 0xB7;                     // group 0, byte 1: low nibble 7, high nibble 11
        let v = dequant_q5_k(&raw, 256).unwrap();
        assert_eq!(v[0], 2.0,  "weight 0 = LOW nibble of byte 0");
        assert_eq!(v[1], 7.0,  "weight 1 = LOW nibble of byte 1 — interleaved would say 5");
        assert_eq!(v[32], 5.0, "weight 32 = HIGH nibble of byte 0, the same byte as weight 0");
        assert_eq!(v[33], 11.0, "weight 33 = HIGH nibble of byte 1");
    }

    #[test]
    fn Q5_K_matches_ggml_on_a_dense_reference_vector() {
        // The kernel-validate fixture, in miniature: non-uniform scales, mins, nibbles AND high
        // bits, expected values computed independently from ggml's dequantize_row_q5_K definition.
        // A decoder wrong in any of the four parts lands off this vector.
        let sc6: [u8; 8] = [5, 30, 63, 12, 48, 21, 39, 7];
        let m6: [u8; 8] = [2, 18, 50, 33, 9, 61, 24, 41];
        // inverse of scale_min_k4, byte-for-byte as gate-q4k established it
        let mut scales = [0u8; 12];
        for j in 0..4 { scales[j] = sc6[j] & 63; scales[j + 4] = m6[j] & 63; }
        for j in 4..8 {
            scales[j + 4] = (sc6[j] & 0x0F) | ((m6[j] & 0x0F) << 4);
            scales[j - 4] |= ((sc6[j] >> 4) & 3) << 6;
            scales[j] |= ((m6[j] >> 4) & 3) << 6;
        }
        let qs: Vec<u8> = (0..128).map(|i| (((i * 7 + 3) % 16) | (((i * 2 + 5) % 16) << 4)) as u8).collect();
        let hb = |g: usize, l: usize, half: usize| ((g + l + half) % 2) as u8;
        let mut qh = [0u8; 32];
        for g in 0..4 { for l in 0..32 {
            qh[l] |= hb(g, l, 0) << (2 * g);
            qh[l] |= hb(g, l, 1) << (2 * g + 1);
        }}
        let mut raw = vec![0u8; Q5_K_BLOCK];
        raw[0..2].copy_from_slice(&0x3C00u16.to_le_bytes());   // d = 1.0
        raw[2..4].copy_from_slice(&0x3800u16.to_le_bytes());   // dmin = 0.5
        raw[4..16].copy_from_slice(&scales);
        raw[16..48].copy_from_slice(&qh);
        raw[48..176].copy_from_slice(&qs);
        let v = dequant_q5_k(&raw, 256).unwrap();
        for g in 0..4 { for l in 0..32 {
            let b = qs[g * 32 + l];
            let lo = ((b & 0xF) + 16 * hb(g, l, 0)) as f32;
            let hi = ((b >> 4) + 16 * hb(g, l, 1)) as f32;
            let want_lo = sc6[2 * g] as f32 * lo - 0.5 * m6[2 * g] as f32;
            let want_hi = sc6[2 * g + 1] as f32 * hi - 0.5 * m6[2 * g + 1] as f32;
            assert!((v[64 * g + l] - want_lo).abs() < 1e-5,
                "g={g} l={l} lo: got {} want {want_lo}", v[64 * g + l]);
            assert!((v[64 * g + 32 + l] - want_hi).abs() < 1e-5,
                "g={g} l={l} hi: got {} want {want_hi}", v[64 * g + 32 + l]);
        }}
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

pub const Q3_K_BLOCK: usize = 110;

/// Q3_K's sixteen 6-bit scales, which are packed DIFFERENTLY from Q4_K's.
///
/// Q4_K splits bytes pairwise. Q3_K treats the twelve scale bytes as three u32s and shuffles: the
/// low four bits of each scale live in the first eight bytes, and the high two bits are gathered in
/// the last four, two bits per scale. **Assuming the Q4_K arrangement here produces sixteen
/// plausible scales that are all wrong**, which is the failure mode this whole family shares.
///
/// Transcribed from llama.cpp's `dequantize_row_q3_K`. The masks are `0x03030303` for the high pairs
/// and `0x0f0f0f0f` for the low nibbles, applied across all four lanes of each u32 at once.
///
/// Scales are SIGNED after unpacking: biased by 32, so a raw 0 means -32.
pub fn scales_q3_k(raw: &[u8]) -> [i8; 16] {
    let mut aux = [0u32; 4];
    for i in 0..3 {
        aux[i] = u32::from_le_bytes([raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2], raw[i * 4 + 3]]);
    }
    const KMASK1: u32 = 0x0303_0303;
    const KMASK2: u32 = 0x0f0f_0f0f;
    let tmp = aux[2];
    aux[2] = ((aux[0] >> 4) & KMASK2) | (((tmp >> 4) & KMASK1) << 4);
    aux[3] = ((aux[1] >> 4) & KMASK2) | (((tmp >> 6) & KMASK1) << 4);
    aux[0] = (aux[0] & KMASK2) | (((tmp) & KMASK1) << 4);
    aux[1] = (aux[1] & KMASK2) | (((tmp >> 2) & KMASK1) << 4);
    let mut out = [0i8; 16];
    for i in 0..4 {
        for b in 0..4 {
            // Biased by 32. A raw 0 is -32, not 0, and forgetting the bias flips the sign of every
            // weight in the sub-block rather than merely scaling it.
            out[i * 4 + b] = (((aux[i] >> (8 * b)) & 0xFF) as i32 - 32) as i8;
        }
    }
    out
}

/// Q3_K: 256 weights in 110 bytes — 32 hmask, 64 qs, 12 scales, f16 d.
///
/// **The high bit is INVERTED.** `hmask` set means "do not subtract 4", so a weight's value is
/// `(q & 3) - 4` when the bit is CLEAR. Reading it the natural way round gives every weight the
/// wrong sign in half the block, which looks like a badly-trained model rather than a decode bug.
pub fn dequant_q3_k(raw: &[u8], n: usize) -> Result<Vec<f32>, String> {
    if n % 256 != 0 { return Err(format!("Q3_K needs a multiple of 256 weights, got {n}")) }
    let blocks = n / 256;
    if raw.len() < blocks * Q3_K_BLOCK {
        return Err(format!("Q3_K: {} bytes for {blocks} blocks, need {}", raw.len(), blocks * Q3_K_BLOCK));
    }
    let mut out = vec![0.0f32; n];
    for b in 0..blocks {
        let p = b * Q3_K_BLOCK;
        let hmask = &raw[p..p + 32];
        let qs = &raw[p + 32..p + 96];
        let scales = scales_q3_k(&raw[p + 96..p + 108]);
        let d = f16(raw[p + 108], raw[p + 109]);
        let mut m: u8 = 1;
        let mut is = 0usize;
        let mut o = b * 256;
        for g in 0..2 {
            for shift in [0u32, 2, 4, 6] {
                for half in 0..2 {
                    let dl = d * scales[is] as f32;
                    for l in 0..16 {
                        let idx = g * 32 + half * 16 + l;
                        let q = ((qs[idx] >> shift) & 3) as i32;
                        // Inverted: the bit being SET means no subtraction.
                        let adj = if hmask[idx % 32] & m != 0 { 0 } else { 4 };
                        out[o + half * 16 + l] = dl * (q - adj) as f32;
                    }
                    is += 1;
                }
                o += 32;
                m <<= 1;
                if m == 0 { m = 1; }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod q3k_tests {
    use super::*;

    #[test]
    fn Q3_K_scales_are_packed_DIFFERENTLY_from_Q4_K() {
        // Assuming the Q4_K arrangement produces sixteen plausible scales that are all wrong, which
        // is the failure this family shares and the reason the two functions are separate.
        let raw = [0u8; 12];
        let s = scales_q3_k(&raw);
        assert_eq!(s.len(), 16, "Q3_K has SIXTEEN sub-blocks; Q4_K has eight");
        assert!(s.iter().all(|x| *x == -32), "raw 0 means -32 after the bias");
    }

    #[test]
    fn the_BIAS_of_32_is_applied_and_a_raw_zero_is_NEGATIVE() {
        // Forgetting it flips the sign of every weight in the sub-block rather than merely scaling
        // it — a much louder failure than a wrong magnitude, and still not an error.
        let mut raw = [0u8; 12];
        raw[0] = 32;                    // low nibble 0, and the value should come out at 32-32 = 0
        assert_eq!(scales_q3_k(&raw)[0], (32 & 0x0F) - 32);
    }

    #[test]
    fn the_HIGH_BIT_is_INVERTED() {
        // hmask SET means do not subtract 4. Read the natural way round, half the block gets the
        // wrong sign — which looks like a badly-trained model rather than a decode bug.
        let mut raw = vec![0u8; Q3_K_BLOCK];
        raw[108] = 0x00; raw[109] = 0x3C;      // d = 1.0
        // Scale 0's SIX bits are not one byte: four low bits here and two high bits from the last
        // four scale bytes. My first version set 33 expecting scale 1 and got -31, because the high
        // pair lives elsewhere — the exact confusion this function exists to encapsulate, and worth
        // leaving in the test rather than tidying into a magic number.
        raw[96 + 0] = 1;                        // low nibble 1
        raw[96 + 8] = 0b10;                     // scale 0's high pair = 2 -> (2 << 4) | 1 = 33 - 32 = 1
        raw[32] = 0x01;                         // weight 0 quant = 1
        // hmask bit 0 CLEAR for weight 0 -> subtract 4 -> 1 - 4 = -3
        let v = dequant_q3_k(&raw, 256).unwrap();
        assert_eq!(v[0], -3.0, "a clear mask bit must subtract 4");
        raw[0] = 0x01;                          // now SET hmask bit 0
        let v = dequant_q3_k(&raw, 256).unwrap();
        assert_eq!(v[0], 1.0, "a set bit must NOT subtract");
    }

    #[test]
    fn Q3_K_has_SIXTEEN_sub_blocks_of_sixteen() {
        // Q4_K and Q5_K have eight of thirty-two. A loop written for those overruns or underfills.
        let raw = vec![0u8; Q3_K_BLOCK];
        assert_eq!(dequant_q3_k(&raw, 256).unwrap().len(), 256);
    }

    #[test]
    fn WRONG_lengths_are_refused() {
        assert!(dequant_q3_k(&[0u8; 4], 256).is_err());
        assert!(dequant_q3_k(&[0u8; Q3_K_BLOCK], 300).is_err());
    }
}
