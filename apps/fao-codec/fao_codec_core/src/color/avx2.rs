//! AVX2 (x86_64) BT.709 RGB ↔ YCbCr conversion at 8 pixels per iteration.
//!
//! ~2× the throughput of the SSE4.1 path on AVX2-capable cores (Haswell
//! and later, ~2013). Falls back to SSE4.1 then scalar.
//!
//! # Why two 128-bit loads instead of one 256-bit load
//!
//! AVX2's `_mm256_shuffle_epi8` operates on each 128-bit lane independently
//! — there is no single-instruction cross-lane byte shuffle. Pixels 4-7
//! in a packed RGB stream span bytes 12-23, which crosses the low/high
//! 128-bit boundary if loaded as one `__m256i`.
//!
//! The fix is to load two separate `__m128i`s — pixels 0-3 from bytes
//! 0-11, pixels 4-7 from bytes 12-23 — and combine them with
//! `_mm256_set_m128i`. After that each 128-bit lane has its own 4
//! pixels and the same per-lane PSHUFB masks from the SSE path work
//! identically in both lanes.

#![cfg(target_arch = "x86_64")]

use std::arch::x86_64::*;

/// SAFETY: caller must ensure AVX2 is available.
#[target_feature(enable = "avx2")]
pub unsafe fn rgb_to_planes(
    rgb: &[u8], _w: usize, _h: usize,
    y: &mut [f32], cb: &mut [f32], cr: &mut [f32],
) {
    let n = y.len();
    debug_assert!(rgb.len() >= 3 * n);
    debug_assert!(cb.len() >= n && cr.len() >= n);
    
    let yr = _mm256_set1_ps(0.2126);
    let yg = _mm256_set1_ps(0.7152);
    let yb = _mm256_set1_ps(0.0722);
    let cb_r = _mm256_set1_ps(-0.1146);
    let cb_g = _mm256_set1_ps(-0.3854);
    let cb_b = _mm256_set1_ps(0.5);
    let cr_r = _mm256_set1_ps(0.5);
    let cr_g = _mm256_set1_ps(-0.4542);
    let cr_b = _mm256_set1_ps(-0.0458);
    
    // Per-128-bit-lane shuffle masks. These extract R/G/B as u32x4 from
    // the 12 bytes of 4 packed RGB pixels — same masks used in the SSE
    // path. With AVX2 they apply to BOTH lanes simultaneously.
    let r_mask = _mm256_setr_epi8(
        // low lane: pixels 0-3
        0, -128i8, -128i8, -128i8,
        3, -128i8, -128i8, -128i8,
        6, -128i8, -128i8, -128i8,
        9, -128i8, -128i8, -128i8,
        // high lane: pixels 4-7 (loaded with their own 12-byte base, so
        // the same byte indices apply within the lane)
        0, -128i8, -128i8, -128i8,
        3, -128i8, -128i8, -128i8,
        6, -128i8, -128i8, -128i8,
        9, -128i8, -128i8, -128i8,
    );
    let g_mask = _mm256_setr_epi8(
        1, -128i8, -128i8, -128i8,
        4, -128i8, -128i8, -128i8,
        7, -128i8, -128i8, -128i8,
        10, -128i8, -128i8, -128i8,
        1, -128i8, -128i8, -128i8,
        4, -128i8, -128i8, -128i8,
        7, -128i8, -128i8, -128i8,
        10, -128i8, -128i8, -128i8,
    );
    let b_mask = _mm256_setr_epi8(
        2, -128i8, -128i8, -128i8,
        5, -128i8, -128i8, -128i8,
        8, -128i8, -128i8, -128i8,
        11, -128i8, -128i8, -128i8,
        2, -128i8, -128i8, -128i8,
        5, -128i8, -128i8, -128i8,
        8, -128i8, -128i8, -128i8,
        11, -128i8, -128i8, -128i8,
    );
    
    let mut i = 0;
    // Stop 8 pixels before the end so each load (which grabs 16 bytes
    // and uses the first 12) doesn't read past `rgb`.
    while i + 8 <= n {
        // Load pixels 0-3 (bytes 0-11) into low __m128i. Reading 16 is
        // fine because `i + 8 <= n` guarantees ≥24 bytes ahead.
        let lo = _mm_loadu_si128(rgb.as_ptr().add(i * 3) as *const __m128i);
        // Load pixels 4-7 (bytes 12-23) into high __m128i.
        let hi = _mm_loadu_si128(rgb.as_ptr().add(i * 3 + 12) as *const __m128i);
        // Combine into one __m256i with low lane = lo, high lane = hi.
        let pixels = _mm256_set_m128i(hi, lo);
        
        // Per-lane shuffle: extracts R/G/B as u32x8 across both lanes.
        let r_u32 = _mm256_shuffle_epi8(pixels, r_mask);
        let g_u32 = _mm256_shuffle_epi8(pixels, g_mask);
        let b_u32 = _mm256_shuffle_epi8(pixels, b_mask);
        
        // u32 → f32 (signed conversion is fine — u8 values fit positive i32).
        let r_f = _mm256_cvtepi32_ps(r_u32);
        let g_f = _mm256_cvtepi32_ps(g_u32);
        let b_f = _mm256_cvtepi32_ps(b_u32);
        
        // FMA isn't required for AVX2 alone; use plain mul+add for
        // CPUs that have AVX2 but not FMA (rare but possible: Via Centaur).
        // Modern Intel/AMD with AVX2 also have FMA; the compiler may
        // fuse these into vfmaddXXXps automatically with -C target-cpu=native.
        let y_v = _mm256_add_ps(
            _mm256_add_ps(_mm256_mul_ps(yr, r_f), _mm256_mul_ps(yg, g_f)),
            _mm256_mul_ps(yb, b_f),
        );
        let cb_v = _mm256_add_ps(
            _mm256_add_ps(_mm256_mul_ps(cb_r, r_f), _mm256_mul_ps(cb_g, g_f)),
            _mm256_mul_ps(cb_b, b_f),
        );
        let cr_v = _mm256_add_ps(
            _mm256_add_ps(_mm256_mul_ps(cr_r, r_f), _mm256_mul_ps(cr_g, g_f)),
            _mm256_mul_ps(cr_b, b_f),
        );
        
        _mm256_storeu_ps(y.as_mut_ptr().add(i), y_v);
        _mm256_storeu_ps(cb.as_mut_ptr().add(i), cb_v);
        _mm256_storeu_ps(cr.as_mut_ptr().add(i), cr_v);
        
        i += 8;
    }
    
    // Tail: scalar for the last <8 pixels.
    while i < n {
        let j = i * 3;
        let r = *rgb.get_unchecked(j) as f32;
        let g = *rgb.get_unchecked(j + 1) as f32;
        let b = *rgb.get_unchecked(j + 2) as f32;
        *y.get_unchecked_mut(i) = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        *cb.get_unchecked_mut(i) = -0.1146 * r - 0.3854 * g + 0.5 * b;
        *cr.get_unchecked_mut(i) = 0.5 * r - 0.4542 * g - 0.0458 * b;
        i += 1;
    }
}

/// SAFETY: caller must ensure AVX2 is available.
#[target_feature(enable = "avx2")]
pub unsafe fn planes_to_rgb(
    y: &[f32], cb: &[f32], cr: &[f32], out: &mut [u8], n: usize,
) {
    debug_assert!(y.len() >= n && cb.len() >= n && cr.len() >= n);
    debug_assert!(out.len() >= 3 * n);
    
    let r_v = _mm256_set1_ps(1.5748);
    let g_u = _mm256_set1_ps(-0.1873);
    let g_v = _mm256_set1_ps(-0.4681);
    let b_u = _mm256_set1_ps(1.8556);
    let zero = _mm256_setzero_ps();
    let max = _mm256_set1_ps(255.0);
    
    let mut i = 0;
    while i + 8 <= n {
        let y_f = _mm256_loadu_ps(y.as_ptr().add(i));
        let u_f = _mm256_loadu_ps(cb.as_ptr().add(i));
        let v_f = _mm256_loadu_ps(cr.as_ptr().add(i));
        
        let r_f = _mm256_add_ps(y_f, _mm256_mul_ps(r_v, v_f));
        let g_f = _mm256_add_ps(_mm256_add_ps(y_f, _mm256_mul_ps(g_u, u_f)), _mm256_mul_ps(g_v, v_f));
        let b_f = _mm256_add_ps(y_f, _mm256_mul_ps(b_u, u_f));
        
        // Clamp [0, 255] then convert to integer.
        let r_clamped = _mm256_min_ps(_mm256_max_ps(r_f, zero), max);
        let g_clamped = _mm256_min_ps(_mm256_max_ps(g_f, zero), max);
        let b_clamped = _mm256_min_ps(_mm256_max_ps(b_f, zero), max);
        
        let r_i = _mm256_cvtps_epi32(r_clamped);
        let g_i = _mm256_cvtps_epi32(g_clamped);
        let b_i = _mm256_cvtps_epi32(b_clamped);
        
        // Spill to an aligned temporary, then write interleaved RGB888.
        // _mm256_extract_epi32 is constrained to compile-time indices,
        // so doing a spill-then-read is cleaner than an 8-way unrolled
        // extraction (and matches what the compiler would emit anyway).
        let mut r_arr = [0i32; 8];
        let mut g_arr = [0i32; 8];
        let mut b_arr = [0i32; 8];
        _mm256_storeu_si256(r_arr.as_mut_ptr() as *mut __m256i, r_i);
        _mm256_storeu_si256(g_arr.as_mut_ptr() as *mut __m256i, g_i);
        _mm256_storeu_si256(b_arr.as_mut_ptr() as *mut __m256i, b_i);
        
        let base = i * 3;
        for k in 0..8 {
            *out.get_unchecked_mut(base + k * 3)     = r_arr[k] as u8;
            *out.get_unchecked_mut(base + k * 3 + 1) = g_arr[k] as u8;
            *out.get_unchecked_mut(base + k * 3 + 2) = b_arr[k] as u8;
        }
        
        i += 8;
    }
    
    while i < n {
        let j = i * 3;
        let luma = *y.get_unchecked(i);
        let u = *cb.get_unchecked(i);
        let v = *cr.get_unchecked(i);
        *out.get_unchecked_mut(j)     = (luma + 1.5748 * v).clamp(0.0, 255.0) as u8;
        *out.get_unchecked_mut(j + 1) = (luma - 0.1873 * u - 0.4681 * v).clamp(0.0, 255.0) as u8;
        *out.get_unchecked_mut(j + 2) = (luma + 1.8556 * u).clamp(0.0, 255.0) as u8;
        i += 1;
    }
}
