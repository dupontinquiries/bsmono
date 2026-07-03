//! 8x8 forward and inverse Discrete Cosine Transform on f32.
//!
//! Used by every CPU-side codec for I-frame and P-frame coefficient encoding.
//! The transform table is computed lazily once via `init()`.
//!
//! # Future direction
//! 
//! This is a clear candidate for SIMD and ultimately a WGSL compute shader. 
//! The current scalar implementation runs at roughly 200 MB/s of pixel 
//! throughput on a single core.
//! 
//! ## SIMD plan (follow-up session)
//! 
//! The natural decomposition is per-row dot-product against the DCT basis. 
//! For each output coefficient k:
//! 
//!     out[k] = sum_i input[i] * DCT[k][i]
//! 
//! With f32x8 (AVX) the row-DCT becomes a single FMA-chain: 8 input lanes, 
//! 8 basis lanes, horizontal reduction. NEON/SSE fall back to two f32x4 
//! halves with separate reductions. The column-DCT can either repeat the 
//! same pattern with a transpose, or use a transposed DCT matrix to avoid 
//! the transpose.
//! 
//! Estimated speedup: 3-4× over the current scalar implementation.
//! 
//! ## GPU plan
//! 
//! One workgroup per 8x8 block, 64 threads cooperating on the butterfly. 
//! Could exceed 10 GB/s for batch transforms. Useful when 1000s of blocks 
//! are queued per frame.
//!
//! # Numerics
//!
//! Pre-multiplied with the standard 1/sqrt(8) for k=0 and sqrt(2/8) for k≥1 
//! basis-function normalization, so dct_fwd produces JPEG-compatible 
//! coefficients with no extra scaling needed.

use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, Ordering};

static mut DCT: [[f32; 8]; 8] = [[0.0; 8]; 8];
static INIT: AtomicBool = AtomicBool::new(false);

/// Initialize the precomputed DCT basis. Idempotent (cheap to call repeatedly).
pub fn init() {
    if INIT.load(Ordering::Relaxed) { return; }
    unsafe {
        for k in 0..8 {
            for i in 0..8 {
                DCT[k][i] = ((2 * i + 1) as f32 * k as f32 * PI / 16.0).cos();
            }
            let s = if k == 0 { 1.0 / (8.0f32).sqrt() } else { (2.0 / 8.0f32).sqrt() };
            for i in 0..8 { DCT[k][i] *= s; }
        }
    }
    INIT.store(true, Ordering::Relaxed);
}

#[cfg(target_arch = "aarch64")]
#[path = "dct_neon.rs"]
mod dct_neon;

#[cfg(target_arch = "x86_64")]
#[path = "dct_sse.rs"]
mod dct_sse;

/// Forward 8x8 DCT. `b` is row-major input pixels, `o` receives coefficients.
#[inline(always)]
pub fn dct_fwd(b: &[f32; 64], o: &mut [f32; 64]) {
    unsafe {
        let m = &*(&raw const DCT);
        #[cfg(target_arch = "aarch64")]
        {
            // SAFETY: NEON is mandatory in aarch64; always available.
            dct_neon::dct_fwd(b, o, m);
            return;
        }
        #[cfg(target_arch = "x86_64")]
        {
            if std::is_x86_feature_detected!("sse4.1") {
                dct_sse::dct_fwd(b, o, m);
                return;
            }
        }
        #[allow(unreachable_code)]
        dct_fwd_scalar(b, o, m);
    }
}

#[inline(always)]
unsafe fn dct_fwd_scalar(b: &[f32; 64], o: &mut [f32; 64], m: &[[f32; 8]; 8]) {
    let mut t = [0.0f32; 64];
    for y in 0..8 {
        let r = y * 8;
        for k in 0..8 {
            t[y * 8 + k] = b[r] * m[k][0] + b[r + 1] * m[k][1]
                + b[r + 2] * m[k][2] + b[r + 3] * m[k][3]
                + b[r + 4] * m[k][4] + b[r + 5] * m[k][5]
                + b[r + 6] * m[k][6] + b[r + 7] * m[k][7];
        }
    }
    for x in 0..8 {
        for k in 0..8 {
            o[k * 8 + x] = t[x] * m[k][0] + t[8 + x] * m[k][1]
                + t[16 + x] * m[k][2] + t[24 + x] * m[k][3]
                + t[32 + x] * m[k][4] + t[40 + x] * m[k][5]
                + t[48 + x] * m[k][6] + t[56 + x] * m[k][7];
        }
    }
}

/// Inverse 8x8 DCT. Reverses `dct_fwd`. `c` is row-major coefficients, `o` 
/// receives reconstructed pixels.
#[inline(always)]
pub fn dct_inv(c: &[f32; 64], o: &mut [f32; 64]) {
    unsafe {
        let m = &*(&raw const DCT);
        #[cfg(target_arch = "aarch64")]
        {
            dct_neon::dct_inv(c, o, m);
            return;
        }
        #[cfg(target_arch = "x86_64")]
        {
            if std::is_x86_feature_detected!("sse4.1") {
                dct_sse::dct_inv(c, o, m);
                return;
            }
        }
        #[allow(unreachable_code)]
        dct_inv_scalar(c, o, m);
    }
}

#[inline(always)]
unsafe fn dct_inv_scalar(c: &[f32; 64], o: &mut [f32; 64], m: &[[f32; 8]; 8]) {
    let mut t = [0.0f32; 64];
    for x in 0..8 {
        for n in 0..8 {
            t[n * 8 + x] = c[x] * m[0][n] + c[8 + x] * m[1][n]
                + c[16 + x] * m[2][n] + c[24 + x] * m[3][n]
                + c[32 + x] * m[4][n] + c[40 + x] * m[5][n]
                + c[48 + x] * m[6][n] + c[56 + x] * m[7][n];
        }
    }
    for y in 0..8 {
        let r = y * 8;
        for n in 0..8 {
            o[y * 8 + n] = t[r] * m[0][n] + t[r + 1] * m[1][n]
                + t[r + 2] * m[2][n] + t[r + 3] * m[3][n]
                + t[r + 4] * m[4][n] + t[r + 5] * m[5][n]
                + t[r + 6] * m[6][n] + t[r + 7] * m[7][n];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn dct_roundtrip() {
        init();
        let input: [f32; 64] = std::array::from_fn(|i| (i as f32) * 0.1 - 3.0);
        let mut coef = [0.0f32; 64];
        let mut back = [0.0f32; 64];
        dct_fwd(&input, &mut coef);
        dct_inv(&coef, &mut back);
        for i in 0..64 {
            assert!((input[i] - back[i]).abs() < 1e-3,
                "round-trip mismatch at {}: in={}, back={}", i, input[i], back[i]);
        }
    }
    
    #[test]
    fn dct_dc_is_mean() {
        init();
        // Constant input → all energy in DC coefficient (index 0)
        let input = [4.0f32; 64];
        let mut coef = [0.0f32; 64];
        dct_fwd(&input, &mut coef);
        // DC = sum / sqrt(64) = 4*64 / 8 = 32 (orthonormal scaling)
        assert!((coef[0] - 32.0).abs() < 1e-3);
        // All AC coefficients should be ~0
        for i in 1..64 {
            assert!(coef[i].abs() < 1e-3, "AC[{}] = {}", i, coef[i]);
        }
    }
    
    #[cfg(target_arch = "aarch64")]
    #[test]
    fn dct_simd_matches_scalar() {
        // Cross-check NEON against the scalar implementation on varied input.
        init();
        let mut input = [0.0f32; 64];
        let mut seed: u32 = 0x1234_5678;
        for v in input.iter_mut() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *v = (seed as i32 as f32) * 1e-7;  // values in roughly [-300, 300]
        }
        let mut simd_out = [0.0f32; 64];
        let mut scalar_out = [0.0f32; 64];
        dct_fwd(&input, &mut simd_out);
        unsafe {
            let m = &*(&raw const DCT);
            dct_fwd_scalar(&input, &mut scalar_out, m);
        }
        for i in 0..64 {
            // FMA can shift results by tiny amounts. Allow 0.01 tolerance.
            assert!((simd_out[i] - scalar_out[i]).abs() < 0.01,
                "DCT[{}] simd={} scalar={}", i, simd_out[i], scalar_out[i]);
        }
        // Round trip via inverse.
        let mut back_simd = [0.0f32; 64];
        let mut back_scalar = [0.0f32; 64];
        dct_inv(&simd_out, &mut back_simd);
        unsafe {
            let m = &*(&raw const DCT);
            dct_inv_scalar(&scalar_out, &mut back_scalar, m);
        }
        for i in 0..64 {
            assert!((back_simd[i] - back_scalar[i]).abs() < 0.01,
                "IDCT[{}] simd={} scalar={}", i, back_simd[i], back_scalar[i]);
        }
        // Round-trip should also recover original input.
        for i in 0..64 {
            assert!((back_simd[i] - input[i]).abs() < 0.01,
                "round-trip[{}] back={} input={}", i, back_simd[i], input[i]);
        }
    }
}
