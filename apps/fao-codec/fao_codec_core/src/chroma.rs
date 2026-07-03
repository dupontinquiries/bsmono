//! 4:2:0 chroma subsampling helpers.
//!
//! The codec stores chroma at half resolution (one Cb/Cr sample per 2x2 luma 
//! block). These functions implement the down/up resampling.

/// Downsample a chroma plane by 2× in each dimension via box filter.
/// 
/// `plane` is full-res `w × h`. `out` must hold `(w/2) × (h/2)` samples.
pub fn chroma_downsample_420(plane: &[f32], w: usize, h: usize, out: &mut [f32]) {
    let hw = w / 2;
    let hh = h / 2;
    for y in 0..hh {
        for x in 0..hw {
            let y0 = y * 2;
            let x0 = x * 2;
            out[y * hw + x] = (
                plane[y0 * w + x0]
                + plane[y0 * w + x0 + 1]
                + plane[(y0 + 1) * w + x0]
                + plane[(y0 + 1) * w + x0 + 1]
            ) * 0.25;
        }
    }
}

/// Upsample a chroma plane by 2× via bilinear interpolation.
/// 
/// `plane` is half-res `hw × hh`. `out` must hold `w × (hh*2)` samples 
/// (typically full luma width).
pub fn chroma_upsample_420(plane: &[f32], hw: usize, hh: usize, out: &mut [f32], w: usize) {
    for y in 0..hh * 2 {
        for x in 0..hw * 2 {
            let sy = (y as f32 / 2.0).min((hh - 1) as f32);
            let sx = (x as f32 / 2.0).min((hw - 1) as f32);
            let iy = sy as usize;
            let ix = sx as usize;
            let fy = sy - iy as f32;
            let fx = sx - ix as f32;
            let iy1 = (iy + 1).min(hh - 1);
            let ix1 = (ix + 1).min(hw - 1);
            out[y * w + x] = plane[iy * hw + ix] * (1.0 - fx) * (1.0 - fy)
                + plane[iy * hw + ix1] * fx * (1.0 - fy)
                + plane[iy1 * hw + ix] * (1.0 - fx) * fy
                + plane[iy1 * hw + ix1] * fx * fy;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn down_then_up_constant_preserves() {
        // Constant 100.0 plane → downsample → upsample → still 100.0
        let plane = vec![100.0f32; 16];
        let mut down = vec![0.0f32; 4];
        chroma_downsample_420(&plane, 4, 4, &mut down);
        for v in &down { assert!((*v - 100.0).abs() < 1e-3); }
        let mut up = vec![0.0f32; 16];
        chroma_upsample_420(&down, 2, 2, &mut up, 4);
        for v in &up { assert!((*v - 100.0).abs() < 1e-3); }
    }
    
    #[test]
    fn downsample_averages_2x2() {
        // Each 2x2 block averages to its center.
        let plane = vec![
            1.0, 2.0, 5.0, 6.0,
            3.0, 4.0, 7.0, 8.0,
            9.0, 10.0, 13.0, 14.0,
            11.0, 12.0, 15.0, 16.0,
        ];
        let mut down = vec![0.0f32; 4];
        chroma_downsample_420(&plane, 4, 4, &mut down);
        assert!((down[0] - 2.5).abs() < 1e-3);   // (1+2+3+4)/4
        assert!((down[1] - 6.5).abs() < 1e-3);   // (5+6+7+8)/4
        assert!((down[2] - 10.5).abs() < 1e-3);  // (9+10+11+12)/4
        assert!((down[3] - 14.5).abs() < 1e-3);  // (13+14+15+16)/4
    }
}
