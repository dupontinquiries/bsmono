//! Scalar reference implementation of BT.709 RGB ↔ YCbCr conversion.
//!
//! Always available regardless of CPU features. SIMD paths are validated 
//! against this implementation in tests.

/// Convert packed RGB888 to BT.709 YCbCr planar f32 — scalar reference.
pub fn rgb_to_planes(rgb: &[u8], _w: usize, _h: usize, y: &mut [f32], cb: &mut [f32], cr: &mut [f32]) {
    let n = y.len();
    let mut i = 0;
    while i + 4 <= n {
        for k in 0..4 {
            let j = (i + k) * 3;
            let r = rgb[j] as f32;
            let g = rgb[j + 1] as f32;
            let b = rgb[j + 2] as f32;
            y[i + k] = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            cb[i + k] = -0.1146 * r - 0.3854 * g + 0.5 * b;
            cr[i + k] = 0.5 * r - 0.4542 * g - 0.0458 * b;
        }
        i += 4;
    }
    while i < n {
        let j = i * 3;
        let r = rgb[j] as f32;
        let g = rgb[j + 1] as f32;
        let b = rgb[j + 2] as f32;
        y[i] = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        cb[i] = -0.1146 * r - 0.3854 * g + 0.5 * b;
        cr[i] = 0.5 * r - 0.4542 * g - 0.0458 * b;
        i += 1;
    }
}

/// Inverse of `rgb_to_planes` — scalar reference.
pub fn planes_to_rgb(y: &[f32], cb: &[f32], cr: &[f32], out: &mut [u8], n: usize) {
    let mut i = 0;
    while i + 4 <= n {
        for k in 0..4 {
            let j = (i + k) * 3;
            let (luma, u, v) = (y[i + k], cb[i + k], cr[i + k]);
            out[j]     = (luma + 1.5748 * v).clamp(0.0, 255.0) as u8;
            out[j + 1] = (luma - 0.1873 * u - 0.4681 * v).clamp(0.0, 255.0) as u8;
            out[j + 2] = (luma + 1.8556 * u).clamp(0.0, 255.0) as u8;
        }
        i += 4;
    }
    while i < n {
        let j = i * 3;
        let (luma, u, v) = (y[i], cb[i], cr[i]);
        out[j]     = (luma + 1.5748 * v).clamp(0.0, 255.0) as u8;
        out[j + 1] = (luma - 0.1873 * u - 0.4681 * v).clamp(0.0, 255.0) as u8;
        out[j + 2] = (luma + 1.8556 * u).clamp(0.0, 255.0) as u8;
        i += 1;
    }
}
