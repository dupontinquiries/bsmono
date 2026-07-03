//! NEON (aarch64) BT.709 RGB ↔ YCbCr conversion.
//!
//! Processes 16 pixels per iteration via `vld3q_u8` (deinterleaving load) 
//! and `vst3q_u8` (interleaving store). On Apple Silicon and modern ARM 
//! cores this provides ~3-4× the throughput of the scalar implementation.
//!
//! These functions are wrapped with `#[target_feature(enable = "neon")]` 
//! so they may use NEON intrinsics regardless of the global build flags 
//! — but they must only be CALLED on systems where NEON is available 
//! (which is true for all aarch64 by spec).

#![cfg(target_arch = "aarch64")]

use std::arch::aarch64::*;

/// SAFETY: Caller must ensure NEON is available. On aarch64 this is 
/// always true (NEON is mandatory in the AArch64 spec).
#[target_feature(enable = "neon")]
pub unsafe fn rgb_to_planes(
    rgb: &[u8], _w: usize, _h: usize,
    y: &mut [f32], cb: &mut [f32], cr: &mut [f32],
) {
    let n = y.len();
    debug_assert!(rgb.len() >= 3 * n);
    debug_assert!(cb.len() >= n && cr.len() >= n);
    
    // BT.709 coefficients
    let yr = vdupq_n_f32(0.2126);
    let yg = vdupq_n_f32(0.7152);
    let yb = vdupq_n_f32(0.0722);
    let cb_r = vdupq_n_f32(-0.1146);
    let cb_g = vdupq_n_f32(-0.3854);
    let cb_b = vdupq_n_f32(0.5);
    let cr_r = vdupq_n_f32(0.5);
    let cr_g = vdupq_n_f32(-0.4542);
    let cr_b = vdupq_n_f32(-0.0458);
    
    let mut i = 0;
    while i + 16 <= n {
        // Load 16 pixels as deinterleaved R, G, B planes.
        // vld3q_u8 takes 48 contiguous bytes and produces three u8x16.
        let rgb_ptr = rgb.as_ptr().add(i * 3);
        let rgb16 = vld3q_u8(rgb_ptr);
        let r_u8 = rgb16.0;
        let g_u8 = rgb16.1;
        let b_u8 = rgb16.2;
        
        // Widen u8x16 → u16x8x2, then to u32x4x4, then to f32x4x4.
        // We need 4 separate f32x4 vectors per channel to cover 16 pixels.
        let r_lo = vmovl_u8(vget_low_u8(r_u8));
        let r_hi = vmovl_u8(vget_high_u8(r_u8));
        let g_lo = vmovl_u8(vget_low_u8(g_u8));
        let g_hi = vmovl_u8(vget_high_u8(g_u8));
        let b_lo = vmovl_u8(vget_low_u8(b_u8));
        let b_hi = vmovl_u8(vget_high_u8(b_u8));
        
        // Each *_lo/_hi is u16x8 covering 8 pixels. Split each further 
        // into two u32x4 then convert to f32x4.
        let r_q0 = vcvtq_f32_u32(vmovl_u16(vget_low_u16(r_lo)));
        let r_q1 = vcvtq_f32_u32(vmovl_u16(vget_high_u16(r_lo)));
        let r_q2 = vcvtq_f32_u32(vmovl_u16(vget_low_u16(r_hi)));
        let r_q3 = vcvtq_f32_u32(vmovl_u16(vget_high_u16(r_hi)));
        let g_q0 = vcvtq_f32_u32(vmovl_u16(vget_low_u16(g_lo)));
        let g_q1 = vcvtq_f32_u32(vmovl_u16(vget_high_u16(g_lo)));
        let g_q2 = vcvtq_f32_u32(vmovl_u16(vget_low_u16(g_hi)));
        let g_q3 = vcvtq_f32_u32(vmovl_u16(vget_high_u16(g_hi)));
        let b_q0 = vcvtq_f32_u32(vmovl_u16(vget_low_u16(b_lo)));
        let b_q1 = vcvtq_f32_u32(vmovl_u16(vget_high_u16(b_lo)));
        let b_q2 = vcvtq_f32_u32(vmovl_u16(vget_low_u16(b_hi)));
        let b_q3 = vcvtq_f32_u32(vmovl_u16(vget_high_u16(b_hi)));
        
        // Compute Y, Cb, Cr for each quartet via fused multiply-add.
        // y = yr*r + yg*g + yb*b
        let y0 = vfmaq_f32(vfmaq_f32(vmulq_f32(yr, r_q0), yg, g_q0), yb, b_q0);
        let y1 = vfmaq_f32(vfmaq_f32(vmulq_f32(yr, r_q1), yg, g_q1), yb, b_q1);
        let y2 = vfmaq_f32(vfmaq_f32(vmulq_f32(yr, r_q2), yg, g_q2), yb, b_q2);
        let y3 = vfmaq_f32(vfmaq_f32(vmulq_f32(yr, r_q3), yg, g_q3), yb, b_q3);
        
        let cb0 = vfmaq_f32(vfmaq_f32(vmulq_f32(cb_r, r_q0), cb_g, g_q0), cb_b, b_q0);
        let cb1 = vfmaq_f32(vfmaq_f32(vmulq_f32(cb_r, r_q1), cb_g, g_q1), cb_b, b_q1);
        let cb2 = vfmaq_f32(vfmaq_f32(vmulq_f32(cb_r, r_q2), cb_g, g_q2), cb_b, b_q2);
        let cb3 = vfmaq_f32(vfmaq_f32(vmulq_f32(cb_r, r_q3), cb_g, g_q3), cb_b, b_q3);
        
        let cr0 = vfmaq_f32(vfmaq_f32(vmulq_f32(cr_r, r_q0), cr_g, g_q0), cr_b, b_q0);
        let cr1 = vfmaq_f32(vfmaq_f32(vmulq_f32(cr_r, r_q1), cr_g, g_q1), cr_b, b_q1);
        let cr2 = vfmaq_f32(vfmaq_f32(vmulq_f32(cr_r, r_q2), cr_g, g_q2), cr_b, b_q2);
        let cr3 = vfmaq_f32(vfmaq_f32(vmulq_f32(cr_r, r_q3), cr_g, g_q3), cr_b, b_q3);
        
        // Store all 16 results back to the output planes.
        let yp = y.as_mut_ptr().add(i);
        vst1q_f32(yp.add(0), y0);
        vst1q_f32(yp.add(4), y1);
        vst1q_f32(yp.add(8), y2);
        vst1q_f32(yp.add(12), y3);
        let cbp = cb.as_mut_ptr().add(i);
        vst1q_f32(cbp.add(0), cb0);
        vst1q_f32(cbp.add(4), cb1);
        vst1q_f32(cbp.add(8), cb2);
        vst1q_f32(cbp.add(12), cb3);
        let crp = cr.as_mut_ptr().add(i);
        vst1q_f32(crp.add(0), cr0);
        vst1q_f32(crp.add(4), cr1);
        vst1q_f32(crp.add(8), cr2);
        vst1q_f32(crp.add(12), cr3);
        
        i += 16;
    }
    
    // Tail: process remaining pixels scalar.
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

/// SAFETY: Caller must ensure NEON is available.
#[target_feature(enable = "neon")]
pub unsafe fn planes_to_rgb(
    y: &[f32], cb: &[f32], cr: &[f32], out: &mut [u8], n: usize,
) {
    debug_assert!(y.len() >= n && cb.len() >= n && cr.len() >= n);
    debug_assert!(out.len() >= 3 * n);
    
    // Inverse BT.709 matrix coefficients
    let r_v = vdupq_n_f32(1.5748);
    let g_u = vdupq_n_f32(-0.1873);
    let g_v = vdupq_n_f32(-0.4681);
    let b_u = vdupq_n_f32(1.8556);
    let zero = vdupq_n_f32(0.0);
    let max = vdupq_n_f32(255.0);
    
    let mut i = 0;
    while i + 16 <= n {
        // Load 4 quartets each of Y, Cb, Cr.
        let yp = y.as_ptr().add(i);
        let cbp = cb.as_ptr().add(i);
        let crp = cr.as_ptr().add(i);
        let y0 = vld1q_f32(yp.add(0));
        let y1 = vld1q_f32(yp.add(4));
        let y2 = vld1q_f32(yp.add(8));
        let y3 = vld1q_f32(yp.add(12));
        let u0 = vld1q_f32(cbp.add(0));
        let u1 = vld1q_f32(cbp.add(4));
        let u2 = vld1q_f32(cbp.add(8));
        let u3 = vld1q_f32(cbp.add(12));
        let v0 = vld1q_f32(crp.add(0));
        let v1 = vld1q_f32(crp.add(4));
        let v2 = vld1q_f32(crp.add(8));
        let v3 = vld1q_f32(crp.add(12));
        
        // R = Y + 1.5748*V
        // G = Y - 0.1873*U - 0.4681*V
        // B = Y + 1.8556*U
        let r_f0 = vfmaq_f32(y0, r_v, v0);
        let r_f1 = vfmaq_f32(y1, r_v, v1);
        let r_f2 = vfmaq_f32(y2, r_v, v2);
        let r_f3 = vfmaq_f32(y3, r_v, v3);
        let g_f0 = vfmaq_f32(vfmaq_f32(y0, g_u, u0), g_v, v0);
        let g_f1 = vfmaq_f32(vfmaq_f32(y1, g_u, u1), g_v, v1);
        let g_f2 = vfmaq_f32(vfmaq_f32(y2, g_u, u2), g_v, v2);
        let g_f3 = vfmaq_f32(vfmaq_f32(y3, g_u, u3), g_v, v3);
        let b_f0 = vfmaq_f32(y0, b_u, u0);
        let b_f1 = vfmaq_f32(y1, b_u, u1);
        let b_f2 = vfmaq_f32(y2, b_u, u2);
        let b_f3 = vfmaq_f32(y3, b_u, u3);
        
        // Clamp to [0, 255] then convert to u8.
        // vcvtq_u32_f32 truncates toward zero (matches `as u8` behavior 
        // for non-negative finite values).
        let clamp = |x: float32x4_t| -> float32x4_t {
            vminq_f32(vmaxq_f32(x, zero), max)
        };
        let r_u32_0 = vcvtq_u32_f32(clamp(r_f0));
        let r_u32_1 = vcvtq_u32_f32(clamp(r_f1));
        let r_u32_2 = vcvtq_u32_f32(clamp(r_f2));
        let r_u32_3 = vcvtq_u32_f32(clamp(r_f3));
        let g_u32_0 = vcvtq_u32_f32(clamp(g_f0));
        let g_u32_1 = vcvtq_u32_f32(clamp(g_f1));
        let g_u32_2 = vcvtq_u32_f32(clamp(g_f2));
        let g_u32_3 = vcvtq_u32_f32(clamp(g_f3));
        let b_u32_0 = vcvtq_u32_f32(clamp(b_f0));
        let b_u32_1 = vcvtq_u32_f32(clamp(b_f1));
        let b_u32_2 = vcvtq_u32_f32(clamp(b_f2));
        let b_u32_3 = vcvtq_u32_f32(clamp(b_f3));
        
        // Narrow u32x4 → u16x4, combine pairs to u16x8, narrow to u8x8, 
        // combine pairs to u8x16.
        let r_u16_lo = vcombine_u16(vmovn_u32(r_u32_0), vmovn_u32(r_u32_1));
        let r_u16_hi = vcombine_u16(vmovn_u32(r_u32_2), vmovn_u32(r_u32_3));
        let g_u16_lo = vcombine_u16(vmovn_u32(g_u32_0), vmovn_u32(g_u32_1));
        let g_u16_hi = vcombine_u16(vmovn_u32(g_u32_2), vmovn_u32(g_u32_3));
        let b_u16_lo = vcombine_u16(vmovn_u32(b_u32_0), vmovn_u32(b_u32_1));
        let b_u16_hi = vcombine_u16(vmovn_u32(b_u32_2), vmovn_u32(b_u32_3));
        let r_u8 = vcombine_u8(vmovn_u16(r_u16_lo), vmovn_u16(r_u16_hi));
        let g_u8 = vcombine_u8(vmovn_u16(g_u16_lo), vmovn_u16(g_u16_hi));
        let b_u8 = vcombine_u8(vmovn_u16(b_u16_lo), vmovn_u16(b_u16_hi));
        
        // Interleaving store: vst3q_u8 takes 3 u8x16 vectors and writes 
        // 48 interleaved bytes (R0G0B0R1G1B1...).
        let triple = uint8x16x3_t(r_u8, g_u8, b_u8);
        vst3q_u8(out.as_mut_ptr().add(i * 3), triple);
        
        i += 16;
    }
    
    // Tail: scalar for the last <16 pixels.
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
