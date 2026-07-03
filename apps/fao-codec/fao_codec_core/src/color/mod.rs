//! BT.709 limited-range color space conversions.
//!
//! Used by every encode/decode path. This is one of the busiest hot loops 
//! in the codec — for a 1920x1080 frame, it processes 2M pixels per call, 
//! and there are typically 2 calls per frame (encode + decode).
//!
//! # Implementation strategy
//!
//! The public functions `rgb_to_planes` and `planes_to_rgb` dispatch at 
//! call time to the best available backend:
//!
//! - **NEON** (aarch64 only) — processes 16 pixels per iteration via 
//!   deinterleaving load (`vld3q_u8`). Typically 3-4× scalar throughput 
//!   on Apple Silicon and modern ARM cores.
//! - **AVX2** (x86_64, Haswell+ ~2013) — processes 8 pixels per iteration. 
//!   ~2× the SSE4.1 path, ~4-6× scalar. Two 128-bit loads combined with 
//!   `_mm256_set_m128i` so each 128-bit lane has its own 4 pixels and the 
//!   per-lane PSHUFB shuffle masks work the same in both lanes.
//! - **SSE4.1** (x86_64) — processes 4 pixels per iteration via byte-level 
//!   shuffles (`PSHUFB`/SSSE3). Roughly 2-3× scalar throughput on modern 
//!   x86 cores. Used as fallback when AVX2 isn't detected.
//! - **Scalar** (always available) — reference implementation. Used as 
//!   fallback on x86_64 hardware older than 2008 and on any target 
//!   without a vector backend.
//!
//! On aarch64, NEON is mandatory in the spec — no runtime detection 
//! needed; the dispatch is a compile-time `cfg` choice.
//!
//! On x86_64, runtime CPUID feature detection picks AVX2 when available 
//! (Haswell+, ~2013), then SSE4.1 (universal since 2008), then scalar. 
//! No nightly Rust required.
//!
//! # Numerical equivalence
//!
//! The SIMD paths are validated against the scalar reference in unit 
//! tests. Output may differ from scalar by ±1 LSB on the converted u8 
//! values due to FMA vs separate mul-add ordering; this is well within 
//! the noise floor of any downstream lossy codec.

mod scalar;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(target_arch = "x86_64")]
mod sse2;

#[cfg(target_arch = "x86_64")]
mod avx2;

/// Convert packed RGB888 to BT.709 YCbCr planar f32.
/// 
/// Outputs match the standard ITU-R BT.709 matrix. No range remap (full 
/// 0..255 in, full 0..255 out). Widths must satisfy `y.len() == w*h` and 
/// `cb.len() == cr.len() == y.len()` for non-subsampled output.
pub fn rgb_to_planes(rgb: &[u8], w: usize, h: usize, y: &mut [f32], cb: &mut [f32], cr: &mut [f32]) {
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is mandatory in aarch64; always available.
        unsafe { neon::rgb_to_planes(rgb, w, h, y, cb, cr) };
        return;
    }
    
    #[cfg(target_arch = "x86_64")]
    {
        // AVX2 (Haswell ~2013 and later) processes 8 pixels per iter — 
        // ~2× the throughput of the SSE4.1 path. Try it first.
        if std::is_x86_feature_detected!("avx2") {
            unsafe { avx2::rgb_to_planes(rgb, w, h, y, cb, cr) };
            return;
        }
        // SSSE3+SSE4.1 are required (universal on x86_64 ≥ 2008). 
        // Standard library detects them via cpuid.
        if std::is_x86_feature_detected!("sse4.1") && std::is_x86_feature_detected!("ssse3") {
            unsafe { sse2::rgb_to_planes(rgb, w, h, y, cb, cr) };
            return;
        }
    }
    
    #[allow(unreachable_code)]
    scalar::rgb_to_planes(rgb, w, h, y, cb, cr);
}

/// Inverse of `rgb_to_planes`. Reconstructs RGB888 from BT.709 YCbCr planes.
/// Output is clamped to [0, 255].
pub fn planes_to_rgb(y: &[f32], cb: &[f32], cr: &[f32], out: &mut [u8], n: usize) {
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is mandatory in aarch64; always available.
        unsafe { neon::planes_to_rgb(y, cb, cr, out, n) };
        return;
    }
    
    #[cfg(target_arch = "x86_64")]
    {
        // AVX2 first (Haswell+) — 8 pixels per iter, ~2× SSE.
        if std::is_x86_feature_detected!("avx2") {
            unsafe { avx2::planes_to_rgb(y, cb, cr, out, n) };
            return;
        }
        if std::is_x86_feature_detected!("sse4.1") && std::is_x86_feature_detected!("ssse3") {
            unsafe { sse2::planes_to_rgb(y, cb, cr, out, n) };
            return;
        }
    }
    
    #[allow(unreachable_code)]
    scalar::planes_to_rgb(y, cb, cr, out, n);
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn color_roundtrip_neutral_gray() {
        // Gray (128, 128, 128) → Y=128, Cb=Cr=0 → back to (128, 128, 128).
        let rgb = vec![128u8; 12]; // 4 pixels of gray
        let mut y = vec![0.0f32; 4];
        let mut cb = vec![0.0f32; 4];
        let mut cr = vec![0.0f32; 4];
        rgb_to_planes(&rgb, 4, 1, &mut y, &mut cb, &mut cr);
        for i in 0..4 {
            assert!((y[i] - 128.0).abs() < 0.5);
            assert!(cb[i].abs() < 0.5);
            assert!(cr[i].abs() < 0.5);
        }
        let mut back = vec![0u8; 12];
        planes_to_rgb(&y, &cb, &cr, &mut back, 4);
        for k in 0..12 {
            assert!((back[k] as i32 - rgb[k] as i32).abs() <= 1,
                "channel {}: {} != {}", k, back[k], rgb[k]);
        }
    }
    
    #[test]
    fn pure_red_roundtrip() {
        let rgb = vec![255u8, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0];
        let mut y = vec![0.0f32; 4];
        let mut cb = vec![0.0f32; 4];
        let mut cr = vec![0.0f32; 4];
        rgb_to_planes(&rgb, 4, 1, &mut y, &mut cb, &mut cr);
        let mut back = vec![0u8; 12];
        planes_to_rgb(&y, &cb, &cr, &mut back, 4);
        for k in (0..12).step_by(3) {
            assert!((back[k] as i32 - 255).abs() <= 1);
            assert!(back[k + 1] <= 1);
            assert!(back[k + 2] <= 1);
        }
    }
    
    /// Validate SIMD vs scalar agreement on a meaningful frame size.
    /// 
    /// Differences of ±1 LSB on the output are tolerated (FMA vs separate 
    /// mul-add ordering can shift the rounding boundary). Any larger 
    /// divergence indicates a bug in the SIMD math.
    #[test]
    fn simd_matches_scalar_on_random_input() {
        // 256 pixels — exercises both the 16-wide main loop and the 
        // scalar tail (256 % 16 = 0, so add a few odd pixels too).
        for n in [256, 257, 263, 1024] {
            let mut rgb = vec![0u8; n * 3];
            // Pseudo-random fill (not crypto, just deterministic varied data).
            let mut seed: u32 = 0xdead_beef;
            for b in rgb.iter_mut() {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                *b = (seed >> 16) as u8;
            }
            
            let mut y_simd = vec![0.0f32; n];
            let mut cb_simd = vec![0.0f32; n];
            let mut cr_simd = vec![0.0f32; n];
            rgb_to_planes(&rgb, n, 1, &mut y_simd, &mut cb_simd, &mut cr_simd);
            
            let mut y_ref = vec![0.0f32; n];
            let mut cb_ref = vec![0.0f32; n];
            let mut cr_ref = vec![0.0f32; n];
            scalar::rgb_to_planes(&rgb, n, 1, &mut y_ref, &mut cb_ref, &mut cr_ref);
            
            // Allow tiny f32 difference from FMA.
            for i in 0..n {
                assert!((y_simd[i] - y_ref[i]).abs() < 0.01,
                    "Y[{}] diverges: simd={}, ref={}", i, y_simd[i], y_ref[i]);
                assert!((cb_simd[i] - cb_ref[i]).abs() < 0.01,
                    "Cb[{}] diverges: simd={}, ref={}", i, cb_simd[i], cb_ref[i]);
                assert!((cr_simd[i] - cr_ref[i]).abs() < 0.01,
                    "Cr[{}] diverges: simd={}, ref={}", i, cr_simd[i], cr_ref[i]);
            }
            
            // Round trip.
            let mut back_simd = vec![0u8; n * 3];
            planes_to_rgb(&y_simd, &cb_simd, &cr_simd, &mut back_simd, n);
            let mut back_ref = vec![0u8; n * 3];
            scalar::planes_to_rgb(&y_ref, &cb_ref, &cr_ref, &mut back_ref, n);
            
            for i in 0..n * 3 {
                let d = (back_simd[i] as i32 - back_ref[i] as i32).abs();
                assert!(d <= 1, "out[{}] diverges: simd={}, ref={}", i, back_simd[i], back_ref[i]);
            }
        }
    }
}
