//! SSE4.1 BT.709 RGB ↔ YCbCr conversion (x86_64).
//!
//! Processes 4 pixels per iteration via byte-level shuffles (PSHUFB/SSSE3) 
//! to deinterleave the packed RGB stream. SSSE3 and SSE4.1 are universal 
//! on x86_64 (all CPUs after 2007/2008). On modern x86 cores this provides 
//! ~2-3× the throughput of the scalar implementation.

#![cfg(target_arch = "x86_64")]

use std::arch::x86_64::*;

/// SAFETY: caller must ensure SSE4.1 is available. Universal on x86_64.
#[target_feature(enable = "sse4.1")]
pub unsafe fn rgb_to_planes(
    rgb: &[u8], _w: usize, _h: usize,
    y: &mut [f32], cb: &mut [f32], cr: &mut [f32],
) {
    let n = y.len();
    debug_assert!(rgb.len() >= 3 * n);
    debug_assert!(cb.len() >= n && cr.len() >= n);
    
    let yr = _mm_set1_ps(0.2126);
    let yg = _mm_set1_ps(0.7152);
    let yb = _mm_set1_ps(0.0722);
    let cb_r = _mm_set1_ps(-0.1146);
    let cb_g = _mm_set1_ps(-0.3854);
    let cb_b = _mm_set1_ps(0.5);
    let cr_r = _mm_set1_ps(0.5);
    let cr_g = _mm_set1_ps(-0.4542);
    let cr_b = _mm_set1_ps(-0.0458);
    
    // Process 4 pixels at a time. Loop bound: while i + 4 <= n AND we have 
    // 16 bytes available to load (4 pixels × 3 = 12 bytes; the load grabs 
    // 16 but we only consume 12, so we need 4 pixels' worth of trailing 
    // pixels OR a safe overshoot region). To avoid reading past end, stop 
    // 4 pixels before n.
    let mut i = 0;
    while i + 8 <= n {
        // Load 16 bytes — covers 5⅓ pixels; we use the first 12 bytes.
        let pixels = _mm_loadu_si128(rgb.as_ptr().add(i * 3) as *const __m128i);
        
        // Shuffle masks to extract R, G, B as u32x4.
        // Source bytes (from low):
        //   B0=R0 B1=G0 B2=B0 B3=R1 B4=G1 B5=B1 B6=R2 B7=G2 B8=B2 B9=R3 B10=G3 B11=B3
        // Want R as 4 u32 lanes: (R0, R1, R2, R3) in lanes 0,1,2,3.
        // In bytes: lane 0 = byte 0; lane 1 = byte 3; lane 2 = byte 6; lane 3 = byte 9
        // So mask for R: [0, _, _, _, 3, _, _, _, 6, _, _, _, 9, _, _, _]
        // (_ means "zero out") → use 0x80 in shuffle mask.
        let r_mask = _mm_setr_epi8(
            0, -128i8, -128i8, -128i8,
            3, -128i8, -128i8, -128i8,
            6, -128i8, -128i8, -128i8,
            9, -128i8, -128i8, -128i8,
        );
        let g_mask = _mm_setr_epi8(
            1, -128i8, -128i8, -128i8,
            4, -128i8, -128i8, -128i8,
            7, -128i8, -128i8, -128i8,
            10, -128i8, -128i8, -128i8,
        );
        let b_mask = _mm_setr_epi8(
            2, -128i8, -128i8, -128i8,
            5, -128i8, -128i8, -128i8,
            8, -128i8, -128i8, -128i8,
            11, -128i8, -128i8, -128i8,
        );
        
        let r_u32 = _mm_shuffle_epi8(pixels, r_mask);
        let g_u32 = _mm_shuffle_epi8(pixels, g_mask);
        let b_u32 = _mm_shuffle_epi8(pixels, b_mask);
        
        // u32 → f32 via _mm_cvtepi32_ps (signed conversion is fine: u8 
        // values 0..255 fit in positive i32 range).
        let r_f = _mm_cvtepi32_ps(r_u32);
        let g_f = _mm_cvtepi32_ps(g_u32);
        let b_f = _mm_cvtepi32_ps(b_u32);
        
        // y = yr*r + yg*g + yb*b (3 muls + 2 adds — no FMA in plain SSE)
        let y_v = _mm_add_ps(
            _mm_add_ps(_mm_mul_ps(yr, r_f), _mm_mul_ps(yg, g_f)),
            _mm_mul_ps(yb, b_f),
        );
        let cb_v = _mm_add_ps(
            _mm_add_ps(_mm_mul_ps(cb_r, r_f), _mm_mul_ps(cb_g, g_f)),
            _mm_mul_ps(cb_b, b_f),
        );
        let cr_v = _mm_add_ps(
            _mm_add_ps(_mm_mul_ps(cr_r, r_f), _mm_mul_ps(cr_g, g_f)),
            _mm_mul_ps(cr_b, b_f),
        );
        
        _mm_storeu_ps(y.as_mut_ptr().add(i), y_v);
        _mm_storeu_ps(cb.as_mut_ptr().add(i), cb_v);
        _mm_storeu_ps(cr.as_mut_ptr().add(i), cr_v);
        
        i += 4;
    }
    
    // Tail: scalar for the remaining (up to 7) pixels. We bail at 
    // n-7 instead of n-3 to avoid reading 16 bytes past the end on the 
    // last possible SIMD iteration.
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

/// SAFETY: caller must ensure SSE4.1 is available.
#[target_feature(enable = "sse4.1")]
pub unsafe fn planes_to_rgb(
    y: &[f32], cb: &[f32], cr: &[f32], out: &mut [u8], n: usize,
) {
    debug_assert!(y.len() >= n && cb.len() >= n && cr.len() >= n);
    debug_assert!(out.len() >= 3 * n);
    
    let r_v = _mm_set1_ps(1.5748);
    let g_u = _mm_set1_ps(-0.1873);
    let g_v = _mm_set1_ps(-0.4681);
    let b_u = _mm_set1_ps(1.8556);
    let zero = _mm_setzero_ps();
    let max = _mm_set1_ps(255.0);
    
    let mut i = 0;
    while i + 4 <= n {
        let y_f = _mm_loadu_ps(y.as_ptr().add(i));
        let u_f = _mm_loadu_ps(cb.as_ptr().add(i));
        let v_f = _mm_loadu_ps(cr.as_ptr().add(i));
        
        // R = Y + 1.5748*V
        // G = Y - 0.1873*U - 0.4681*V
        // B = Y + 1.8556*U
        let r_f = _mm_add_ps(y_f, _mm_mul_ps(r_v, v_f));
        let g_f = _mm_add_ps(_mm_add_ps(y_f, _mm_mul_ps(g_u, u_f)), _mm_mul_ps(g_v, v_f));
        let b_f = _mm_add_ps(y_f, _mm_mul_ps(b_u, u_f));
        
        // Clamp [0, 255] then convert to integer.
        let clamp = |x: __m128| _mm_min_ps(_mm_max_ps(x, zero), max);
        let r_i = _mm_cvtps_epi32(clamp(r_f));
        let g_i = _mm_cvtps_epi32(clamp(g_f));
        let b_i = _mm_cvtps_epi32(clamp(b_f));
        
        // Extract individual u8 values and write interleaved RGB888.
        // (No native u32x4 → u8x4 narrow on SSE without packus; for our 
        // 4-pixel chunks scalar extraction via _mm_extract_epi32 is 
        // straightforward and avoids transmute concerns.)
        let r0 = _mm_extract_epi32(r_i, 0) as u8;
        let r1 = _mm_extract_epi32(r_i, 1) as u8;
        let r2 = _mm_extract_epi32(r_i, 2) as u8;
        let r3 = _mm_extract_epi32(r_i, 3) as u8;
        let g0 = _mm_extract_epi32(g_i, 0) as u8;
        let g1 = _mm_extract_epi32(g_i, 1) as u8;
        let g2 = _mm_extract_epi32(g_i, 2) as u8;
        let g3 = _mm_extract_epi32(g_i, 3) as u8;
        let b0 = _mm_extract_epi32(b_i, 0) as u8;
        let b1 = _mm_extract_epi32(b_i, 1) as u8;
        let b2 = _mm_extract_epi32(b_i, 2) as u8;
        let b3 = _mm_extract_epi32(b_i, 3) as u8;
        
        let base = i * 3;
        *out.get_unchecked_mut(base)      = r0;
        *out.get_unchecked_mut(base + 1)  = g0;
        *out.get_unchecked_mut(base + 2)  = b0;
        *out.get_unchecked_mut(base + 3)  = r1;
        *out.get_unchecked_mut(base + 4)  = g1;
        *out.get_unchecked_mut(base + 5)  = b1;
        *out.get_unchecked_mut(base + 6)  = r2;
        *out.get_unchecked_mut(base + 7)  = g2;
        *out.get_unchecked_mut(base + 8)  = b2;
        *out.get_unchecked_mut(base + 9)  = r3;
        *out.get_unchecked_mut(base + 10) = g3;
        *out.get_unchecked_mut(base + 11) = b3;
        
        i += 4;
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
