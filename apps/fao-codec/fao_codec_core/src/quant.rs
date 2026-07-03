//! Quantization tables and helpers used by all DCT-based codecs (v2..v6).
//!
//! The luma and chroma matrices are the standard JPEG defaults; users tune 
//! quality via the `qf` parameter to `build_qtable` (1=worst, 100=best).
//!
//! `dzq_fast` is the inner loop of CPU coefficient quantization — multiply 
//! by the inverse rather than divide. With 64 coefficients × thousands of 
//! blocks per frame, this matters.

/// Standard JPEG luma quantization table (8x8, row-major).
pub const LUMA_Q: [f32; 64] = [
    16.0, 11.0, 10.0, 16.0, 24.0, 40.0, 51.0, 61.0,
    12.0, 12.0, 14.0, 19.0, 26.0, 58.0, 60.0, 55.0,
    14.0, 13.0, 16.0, 24.0, 40.0, 57.0, 69.0, 56.0,
    14.0, 17.0, 22.0, 29.0, 51.0, 87.0, 80.0, 62.0,
    18.0, 22.0, 37.0, 56.0, 68.0, 109.0, 103.0, 77.0,
    24.0, 35.0, 55.0, 64.0, 81.0, 104.0, 113.0, 92.0,
    49.0, 64.0, 78.0, 87.0, 103.0, 121.0, 120.0, 101.0,
    72.0, 92.0, 95.0, 98.0, 112.0, 100.0, 103.0, 99.0,
];

/// Standard JPEG chroma quantization table (8x8, row-major).
pub const CHROMA_Q: [f32; 64] = [
    16.0, 17.0, 20.0, 24.0, 28.0, 34.0, 42.0, 50.0,
    17.0, 18.0, 22.0, 26.0, 32.0, 40.0, 48.0, 56.0,
    20.0, 22.0, 26.0, 32.0, 40.0, 48.0, 56.0, 62.0,
    24.0, 26.0, 32.0, 40.0, 48.0, 56.0, 62.0, 68.0,
    28.0, 32.0, 40.0, 48.0, 56.0, 62.0, 68.0, 72.0,
    34.0, 40.0, 48.0, 56.0, 62.0, 68.0, 72.0, 76.0,
    42.0, 48.0, 56.0, 62.0, 68.0, 72.0, 76.0, 80.0,
    50.0, 56.0, 62.0, 68.0, 72.0, 76.0, 80.0, 80.0,
];

/// JPEG zigzag scan order. After quantization, scan in this order to 
/// concentrate non-zero coefficients at the start (improves RLE compression).
pub const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5,
    12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51,
    58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Build a quality-scaled quantization table from a base (typically 
/// LUMA_Q or CHROMA_Q). `qf` ∈ [1, 100], where 100 is highest quality.
pub fn build_qtable(base: &[f32; 64], qf: i32) -> [f32; 64] {
    let q = qf.max(1).min(100);
    let s = if q < 50 { 5000.0 / q as f32 } else { 200.0 - 2.0 * q as f32 };
    let mut o = [0.0f32; 64];
    for i in 0..64 {
        o[i] = ((base[i] * s + 50.0) / 100.0).floor().max(1.0);
    }
    o
}

/// Pre-compute reciprocal Q table — multiply faster than divide.
pub fn build_qtable_inv(qt: &[f32; 64]) -> [f32; 64] {
    let mut o = [0.0f32; 64];
    for i in 0..64 { o[i] = 1.0 / qt[i]; }
    o
}

/// Dead-zone quantizer (slow path: division).
#[inline(always)]
pub fn dzq(v: f32, qs: f32, dz: f32) -> i32 {
    if v.abs() < qs * dz * 0.5 { 0 } else { (v / qs).round() as i32 }
}

/// Dead-zone quantizer (fast path: multiplication via pre-inverted scale).
#[inline(always)]
pub fn dzq_fast(v: f32, _qs: f32, inv_qs: f32, dz_half_qs: f32) -> i32 {
    if v.abs() < dz_half_qs { 0 } else { (v * inv_qs).round() as i32 }
}

#[inline]
fn cat(v: i32) -> u32 {
    if v == 0 { 0 } else { ((v.unsigned_abs() as f32 + 1.0).log2().ceil()) as u32 }
}

/// Estimate bit cost of encoding a zigzag-ordered coefficient block via 
/// DPCM-on-DC + RLE-on-AC. Returns (bits, this_dc) so the caller can 
/// thread `prev_dc` between blocks.
#[inline(always)]
pub fn est_bits_dpcm(zig: &[i32; 64], prev_dc: i32) -> (u32, i32) {
    let dd = zig[0] - prev_dc;
    let dc = cat(dd);
    let mut b = dc + dc.min(11) + 1;
    let mut ln: i32 = -1;
    for i in (1..64).rev() {
        if zig[i] != 0 { ln = i as i32; break; }
    }
    if ln > 0 {
        let mut zr = 0u32;
        for i in 1..=ln as usize {
            if zig[i] == 0 {
                zr += 1;
                if zr == 16 { b += 11; zr = 0; }
            } else {
                let c = cat(zig[i]);
                b += (2 + zr + c).min(16) + c;
                zr = 0;
            }
        }
    }
    b += 4;
    (b, zig[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn build_qtable_clamps_qf() {
        let q_low = build_qtable(&LUMA_Q, -10);
        let q_high = build_qtable(&LUMA_Q, 200);
        // Clamps to 1 / 100; both should yield valid (no NaN, no zeros).
        for i in 0..64 {
            assert!(q_low[i] >= 1.0);
            assert!(q_high[i] >= 1.0);
        }
    }
    
    #[test]
    fn dzq_fast_matches_dzq() {
        let qt = build_qtable(&LUMA_Q, 50);
        for i in 0..64 {
            let qs = qt[i];
            let inv_qs = 1.0 / qs;
            let dz_half = qs * 0.5 * 0.5;
            for v in [-100.0, -10.0, -1.0, 0.0, 1.0, 10.0, 100.0] {
                let slow = dzq(v, qs, 0.5);
                let fast = dzq_fast(v, qs, inv_qs, dz_half);
                assert_eq!(slow, fast, "i={} v={} qs={}", i, v, qs);
            }
        }
    }
}
