//! # Phase correlation — FFT-based global motion estimation
//!
//! Cross-power spectrum on two consecutive frames yields a sub-pixel 
//! translation estimate (dx, dy) plus a confidence value. Used by v8 
//! reconstruction (camera-motion seed for SfM) and by v10 motion-comp 
//! (prediction shift).
//!
//! This module was extracted from the legacy v7_perception_codec.rs 
//! during the v10 cleanup. The phase correlation algorithm itself is 
//! generic computer vision, not specific to v7 — moving it out makes 
//! the dependency clean.

use num_complex::Complex32;
use rustfft::FftPlanner;
use std::sync::Arc;

// ═══════════════════════════════════════════════════════════
// CameraMotion — per-frame global motion estimation
// ═══════════════════════════════════════════════════════════

#[derive(Clone, Copy, Debug, Default)]
pub struct CameraMotion {
    /// Estimated translation in pixels (sub-pixel precision)
    pub dx: f32,
    pub dy: f32,
    /// Confidence in the estimate [0.0, 1.0] — based on peak sharpness
    /// in the phase correlation surface. Low confidence means scene-cut
    /// or content too uniform to detect motion.
    pub confidence: f32,
    /// Estimated zoom factor (1.0 = no zoom). Currently unused; reserved
    /// for future log-polar phase correlation.
    pub zoom: f32,
}

impl CameraMotion {
    pub fn none() -> Self {
        Self { dx: 0.0, dy: 0.0, confidence: 0.0, zoom: 1.0 }
    }
    
    pub fn is_static(&self) -> bool {
        self.confidence > 0.5 && self.dx.abs() < 0.5 && self.dy.abs() < 0.5
    }
}

// ═══════════════════════════════════════════════════════════
// PhaseCorrelationPass — FFT-based global motion detection
// ═══════════════════════════════════════════════════════════
//
// Algorithm:
//   1. Take a center crop (power-of-2 sized for FFT efficiency)
//      from current and previous frame
//   2. Apply Hann window to reduce edge artifacts
//   3. Forward FFT both → A, B (complex)
//   4. Cross-power spectrum: C = (A · conj(B)) / |A · conj(B)|
//      This normalizes amplitude away, leaving only phase information
//   5. Inverse FFT C → real-valued correlation surface
//   6. Find peak coordinates (px, py)
//   7. dx, dy = px, py (with wrap correction for negative shifts)
//   8. Confidence = peak height / second-highest peak height
//
// Sub-pixel refinement uses parabolic interpolation around the peak
// (~0.1 pixel accuracy on real video).

pub struct PhaseCorrelationPass {
    /// Window size for FFT (power of 2). 256 gives good speed/accuracy
    /// trade-off at 1080p+. Computational cost is O(N² log N).
    pub window_size: usize,
    /// Precomputed forward + inverse FFT plans. rustfft's Fft trait
    /// objects are Send+Sync, so multiple workers can use the same
    /// plan concurrently without locking.
    /// 
    /// Previously this struct held `Mutex<FftPlanner>` and called
    /// `plan_fft_forward()` per estimate() — every worker serialized
    /// through that mutex even though plan lookup is fast. With 6
    /// V8 worker shards the contention was visible. Now: zero locking
    /// in the hot path.
    /// 
    /// Also precomputes a Hann window of length `window_size` for the 
    /// 2D windowing step — same Hann is used by every estimate() call.
    fft_forward: std::sync::Arc<dyn rustfft::Fft<f32>>,
    fft_inverse: std::sync::Arc<dyn rustfft::Fft<f32>>,
    hann: Vec<f32>,
}

impl Default for PhaseCorrelationPass {
    fn default() -> Self {
        Self::new(256)
    }
}

impl PhaseCorrelationPass {
    pub fn new(window_size: usize) -> Self {
        // Force window_size to be a power of 2 for FFT efficiency
        let ws = window_size.next_power_of_two().min(1024).max(32);
        let mut planner = FftPlanner::<f32>::new();
        let fft_forward = planner.plan_fft_forward(ws);
        let fft_inverse = planner.plan_fft_inverse(ws);
        let hann: Vec<f32> = (0..ws).map(|i| {
            let t = i as f32 / (ws - 1) as f32;
            0.5 * (1.0 - (2.0 * std::f32::consts::PI * t).cos())
        }).collect();
        Self {
            window_size: ws,
            fft_forward,
            fft_inverse,
            hann,
        }
    }
    
    /// Estimate global motion between two frames (Y plane only).
    /// Frames must have same width and height.
    pub fn estimate(
        &self,
        cur_yuv: &[u8],
        prev_yuv: &[u8],
        width: usize,
        height: usize,
    ) -> CameraMotion {
        let ws = self.window_size;
        if width < ws || height < ws { return CameraMotion::none(); }
        if cur_yuv.len() < width * height || prev_yuv.len() < width * height {
            return CameraMotion::none();
        }
        
        // Extract centered Y-plane window from each frame
        let cx = (width - ws) / 2;
        let cy = (height - ws) / 2;
        let mut cur_window = vec![Complex32::new(0.0, 0.0); ws * ws];
        let mut prev_window = vec![Complex32::new(0.0, 0.0); ws * ws];
        
        // Hann window — precomputed once at construction; same for every call.
        let hann = &self.hann;
        
        // Compute mean for DC removal (improves correlation peak)
        let mut sum_cur = 0.0f64;
        let mut sum_prev = 0.0f64;
        for y in 0..ws {
            for x in 0..ws {
                let src_idx = (cy + y) * width + (cx + x);
                sum_cur += cur_yuv[src_idx] as f64;
                sum_prev += prev_yuv[src_idx] as f64;
            }
        }
        let mean_cur = (sum_cur / (ws * ws) as f64) as f32;
        let mean_prev = (sum_prev / (ws * ws) as f64) as f32;
        
        // Fill complex windows with windowed (mean-removed) pixels
        for y in 0..ws {
            for x in 0..ws {
                let src_idx = (cy + y) * width + (cx + x);
                let dst_idx = y * ws + x;
                let w = hann[x] * hann[y];
                cur_window[dst_idx] = Complex32::new(
                    (cur_yuv[src_idx] as f32 - mean_cur) * w, 0.0
                );
                prev_window[dst_idx] = Complex32::new(
                    (prev_yuv[src_idx] as f32 - mean_prev) * w, 0.0
                );
            }
        }
        
        // 2D FFT: rows then columns
        self.fft2d_forward(&mut cur_window, ws);
        self.fft2d_forward(&mut prev_window, ws);
        
        // Cross-power spectrum: A · conj(B) / |A · conj(B)|
        for i in 0..ws * ws {
            let a = cur_window[i];
            let b = prev_window[i].conj();
            let prod = a * b;
            let mag = prod.norm();
            if mag > 1e-9 {
                cur_window[i] = prod / mag;
            } else {
                cur_window[i] = Complex32::new(0.0, 0.0);
            }
        }
        
        // Inverse FFT → correlation surface
        self.fft2d_inverse(&mut cur_window, ws);
        
        // Find peak
        let mut peak_val = -f32::INFINITY;
        let mut peak_x = 0usize;
        let mut peak_y = 0usize;
        let mut second_peak = -f32::INFINITY;
        for y in 0..ws {
            for x in 0..ws {
                let v = cur_window[y * ws + x].re;
                if v > peak_val {
                    second_peak = peak_val;
                    peak_val = v;
                    peak_x = x;
                    peak_y = y;
                } else if v > second_peak {
                    second_peak = v;
                }
            }
        }
        
        // Convert to signed displacement (FFT wraps shifts past N/2)
        let dx_raw = if peak_x > ws / 2 { peak_x as i32 - ws as i32 } else { peak_x as i32 };
        let dy_raw = if peak_y > ws / 2 { peak_y as i32 - ws as i32 } else { peak_y as i32 };
        
        // Sub-pixel refinement: parabolic fit on peak neighbors
        let (dx_sub, dy_sub) = self.subpixel_peak(&cur_window, ws, peak_x, peak_y);
        
        let dx = dx_raw as f32 + dx_sub;
        let dy = dy_raw as f32 + dy_sub;
        
        // Confidence: ratio of peak to second-highest, clamped to [0,1]
        let confidence = if second_peak > 0.0 && peak_val > 0.0 {
            ((peak_val - second_peak) / peak_val).clamp(0.0, 1.0)
        } else if peak_val > 0.0 {
            1.0
        } else {
            0.0
        };
        
        CameraMotion {
            dx,
            dy,
            confidence,
            zoom: 1.0,
        }
    }
    
    /// 2D forward FFT in-place
    fn fft2d_forward(&self, data: &mut [Complex32], n: usize) {
        // Use the precomputed plan — no mutex contention with other workers.
        debug_assert_eq!(n, self.window_size);
        let fft = &self.fft_forward;
        // Row pass
        for row in 0..n {
            let start = row * n;
            fft.process(&mut data[start..start + n]);
        }
        // Column pass: transpose, FFT, transpose back
        let mut col_buf = vec![Complex32::new(0.0, 0.0); n];
        for col in 0..n {
            for row in 0..n {
                col_buf[row] = data[row * n + col];
            }
            fft.process(&mut col_buf);
            for row in 0..n {
                data[row * n + col] = col_buf[row];
            }
        }
    }
    
    /// 2D inverse FFT in-place (no normalization — peak position is what matters)
    fn fft2d_inverse(&self, data: &mut [Complex32], n: usize) {
        debug_assert_eq!(n, self.window_size);
        let fft = &self.fft_inverse;
        for row in 0..n {
            let start = row * n;
            fft.process(&mut data[start..start + n]);
        }
        let mut col_buf = vec![Complex32::new(0.0, 0.0); n];
        for col in 0..n {
            for row in 0..n {
                col_buf[row] = data[row * n + col];
            }
            fft.process(&mut col_buf);
            for row in 0..n {
                data[row * n + col] = col_buf[row];
            }
        }
    }
    
    /// Parabolic interpolation around peak for sub-pixel accuracy
    fn subpixel_peak(&self, surface: &[Complex32], n: usize, px: usize, py: usize) -> (f32, f32) {
        if px == 0 || py == 0 || px >= n - 1 || py >= n - 1 {
            return (0.0, 0.0);
        }
        let at = |x: usize, y: usize| surface[y * n + x].re;
        let center = at(px, py);
        let left   = at(px - 1, py);
        let right  = at(px + 1, py);
        let up     = at(px, py - 1);
        let down   = at(px, py + 1);
        
        // Parabolic fit: x_offset = (left - right) / (2 * (left + right - 2*center))
        let dx_denom = left + right - 2.0 * center;
        let dy_denom = up + down - 2.0 * center;
        let dx_off = if dx_denom.abs() > 1e-9 { 0.5 * (left - right) / dx_denom } else { 0.0 };
        let dy_off = if dy_denom.abs() > 1e-9 { 0.5 * (up - down) / dy_denom } else { 0.0 };
        
        // Clamp — parabolic fit only valid within ±0.5 pixel
        (dx_off.clamp(-0.5, 0.5), dy_off.clamp(-0.5, 0.5))
    }
}
