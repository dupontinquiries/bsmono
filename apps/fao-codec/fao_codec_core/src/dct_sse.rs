//! SSE4.1 (x86_64) accelerated 8x8 DCT and IDCT.
//!
//! Each 8-wide dot product is computed as 2× f32x4 multiplies + 
//! horizontal-add via _mm_hadd_ps. The hadd path is slightly slower than 
//! NEON's vaddvq_f32 (no single-instruction horizontal sum on SSE), but 
//! still 1.8-2.5× the scalar throughput on modern x86 cores.

#![cfg(target_arch = "x86_64")]

use std::arch::x86_64::*;

/// Horizontal sum of all 4 lanes of a __m128.
#[inline(always)]
unsafe fn hadd4(v: __m128) -> f32 {
    // hadd: (v0+v1, v2+v3, v0+v1, v2+v3)
    let h1 = _mm_hadd_ps(v, v);
    // hadd again: (v0+v1+v2+v3, ...)
    let h2 = _mm_hadd_ps(h1, h1);
    _mm_cvtss_f32(h2)
}

/// SAFETY: caller must ensure SSE4.1 is available.
#[target_feature(enable = "sse4.1")]
pub unsafe fn dct_fwd(b: &[f32; 64], o: &mut [f32; 64], dct: &[[f32; 8]; 8]) {
    // Stage 1: row-wise.
    let mut t = [0.0f32; 64];
    for y in 0..8 {
        let r = y * 8;
        let b_lo = _mm_loadu_ps(b.as_ptr().add(r));
        let b_hi = _mm_loadu_ps(b.as_ptr().add(r + 4));
        for k in 0..8 {
            let m_lo = _mm_loadu_ps(dct[k].as_ptr());
            let m_hi = _mm_loadu_ps(dct[k].as_ptr().add(4));
            let p_lo = _mm_mul_ps(b_lo, m_lo);
            let p_hi = _mm_mul_ps(b_hi, m_hi);
            let sum = _mm_add_ps(p_lo, p_hi);
            t[y * 8 + k] = hadd4(sum);
        }
    }
    // Stage 2: column-wise.
    for x in 0..8 {
        let col = [
            t[x], t[8 + x], t[16 + x], t[24 + x],
            t[32 + x], t[40 + x], t[48 + x], t[56 + x],
        ];
        let c_lo = _mm_loadu_ps(col.as_ptr());
        let c_hi = _mm_loadu_ps(col.as_ptr().add(4));
        for k in 0..8 {
            let m_lo = _mm_loadu_ps(dct[k].as_ptr());
            let m_hi = _mm_loadu_ps(dct[k].as_ptr().add(4));
            let p_lo = _mm_mul_ps(c_lo, m_lo);
            let p_hi = _mm_mul_ps(c_hi, m_hi);
            let sum = _mm_add_ps(p_lo, p_hi);
            o[k * 8 + x] = hadd4(sum);
        }
    }
}

/// SAFETY: caller must ensure SSE4.1 is available.
#[target_feature(enable = "sse4.1")]
pub unsafe fn dct_inv(c: &[f32; 64], o: &mut [f32; 64], dct: &[[f32; 8]; 8]) {
    // Stage 1: column-wise IDCT.
    let mut t = [0.0f32; 64];
    for x in 0..8 {
        let col = [
            c[x], c[8 + x], c[16 + x], c[24 + x],
            c[32 + x], c[40 + x], c[48 + x], c[56 + x],
        ];
        let c_lo = _mm_loadu_ps(col.as_ptr());
        let c_hi = _mm_loadu_ps(col.as_ptr().add(4));
        for n in 0..8 {
            let m_col = [
                dct[0][n], dct[1][n], dct[2][n], dct[3][n],
                dct[4][n], dct[5][n], dct[6][n], dct[7][n],
            ];
            let m_lo = _mm_loadu_ps(m_col.as_ptr());
            let m_hi = _mm_loadu_ps(m_col.as_ptr().add(4));
            let p_lo = _mm_mul_ps(c_lo, m_lo);
            let p_hi = _mm_mul_ps(c_hi, m_hi);
            let sum = _mm_add_ps(p_lo, p_hi);
            t[n * 8 + x] = hadd4(sum);
        }
    }
    // Stage 2: row-wise IDCT.
    for y in 0..8 {
        let r = y * 8;
        let t_lo = _mm_loadu_ps(t.as_ptr().add(r));
        let t_hi = _mm_loadu_ps(t.as_ptr().add(r + 4));
        for n in 0..8 {
            let m_col = [
                dct[0][n], dct[1][n], dct[2][n], dct[3][n],
                dct[4][n], dct[5][n], dct[6][n], dct[7][n],
            ];
            let m_lo = _mm_loadu_ps(m_col.as_ptr());
            let m_hi = _mm_loadu_ps(m_col.as_ptr().add(4));
            let p_lo = _mm_mul_ps(t_lo, m_lo);
            let p_hi = _mm_mul_ps(t_hi, m_hi);
            let sum = _mm_add_ps(p_lo, p_hi);
            o[y * 8 + n] = hadd4(sum);
        }
    }
}
