//! Phase correlation: one global translation per frame pair, sub-pixel,
//! with a confidence.
//!
//! This is `fao_codec_lib::phase_correlation` (v11.6 state, including the
//! second-peak exclusion that fixed confidence on static scenes) brought
//! over verbatim in algorithm and adapted at the edges: it takes an RGB
//! [`Frame`] and computes luma itself, and the parabolic sub-pixel fit
//! wraps at the window edge instead of giving up (the original returns 0
//! when the peak sits at index 0 — which is exactly the static-camera case,
//! whose sub-pixel weave is the fence-slit signal the SR plate needs).
//!
//! Two consumers now (fao-codec, dandere2x); the house pattern is to lift
//! this into a `shared/` kit and point both at it. Not done here so the
//! fao side stays untouched by this change — that lift is a separate,
//! mechanical commit.
//!
//! Convention, pinned by `plate::tests`: `estimate(cur, prev)` returns the
//! displacement of content from `prev` to `cur`, i.e. `cur[x] ≈ prev[x - dx]`.

use crate::frame::Frame;
use num_complex::Complex32;
use rustfft::FftPlanner;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GlobalMotion {
    /// Displacement in pixels, sub-pixel precision.
    pub dx: f32,
    pub dy: f32,
    /// `(peak − rival) / peak` in `[0, 1]`: 1 is an unambiguous peak, 0 a
    /// scene cut or content too uniform to register.
    pub confidence: f32,
}

impl GlobalMotion {
    pub fn none() -> Self {
        GlobalMotion { dx: 0.0, dy: 0.0, confidence: 0.0 }
    }
}

pub struct PhaseCorrelation {
    /// Power-of-two side of the centred window. 512 at 1080p; the estimate
    /// is of the window's content only, so a moving actor filling the
    /// centre of the frame is measured as "the camera".
    pub window: usize,
    fft_forward: Arc<dyn rustfft::Fft<f32>>,
    fft_inverse: Arc<dyn rustfft::Fft<f32>>,
    hann: Vec<f32>,
}

impl PhaseCorrelation {
    pub fn new(window: usize) -> Self {
        let ws = window.next_power_of_two().clamp(32, 1024);
        let mut planner = FftPlanner::<f32>::new();
        let fft_forward = planner.plan_fft_forward(ws);
        let fft_inverse = planner.plan_fft_inverse(ws);
        let hann: Vec<f32> = (0..ws)
            .map(|i| {
                let t = i as f32 / (ws - 1) as f32;
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * t).cos())
            })
            .collect();
        PhaseCorrelation { window: ws, fft_forward, fft_inverse, hann }
    }

    /// Largest window that fits a frame, capped at 512.
    pub fn for_frame(width: u32, height: u32) -> Self {
        let side = (width.min(height) as usize).min(512);
        // next_power_of_two rounds UP; we need the largest power of two ≤ side.
        let ws = if side.is_power_of_two() { side } else { side.next_power_of_two() / 2 };
        Self::new(ws)
    }

    /// Integer luma of the `ws`×`ws` window at `(cx, cy)`.
    fn window_luma(&self, f: &Frame, cx: usize, cy: usize) -> Vec<f32> {
        let ws = self.window;
        let mut out = Vec::with_capacity(ws * ws);
        for y in 0..ws {
            let row = f.row((cy + y) as u32, cx as u32, (cx + ws) as u32);
            for px in row.chunks_exact(3) {
                out.push(((77 * px[0] as u32 + 150 * px[1] as u32 + 29 * px[2] as u32) >> 8) as f32);
            }
        }
        out
    }

    /// Global motion of `cur` relative to `prev`: the componentwise median
    /// of the centre window and, when the frame is at least twice the
    /// window in both directions, the four quadrant-centred windows. One
    /// window measures whatever fills it — an actor in the middle of the
    /// frame reads as the camera — and a median of five is robust to one
    /// such window. Confidence is the median of the five.
    pub fn estimate(&self, cur: &Frame, prev: &Frame) -> GlobalMotion {
        let ws = self.window;
        let (w, h) = (cur.width as usize, cur.height as usize);
        if w < ws || h < ws || (cur.width, cur.height) != (prev.width, prev.height) {
            return GlobalMotion::none();
        }
        let mut origins = vec![((w - ws) / 2, (h - ws) / 2)];
        if w >= 2 * ws && h >= 2 * ws {
            for &qx in &[w / 4, 3 * w / 4] {
                for &qy in &[h / 4, 3 * h / 4] {
                    origins.push((qx - ws / 2, qy - ws / 2));
                }
            }
        }
        let mut ests: Vec<GlobalMotion> = origins.iter().map(|&(cx, cy)| self.estimate_at(cur, prev, cx, cy)).collect();
        if ests.len() == 1 {
            return ests[0];
        }
        let mid = ests.len() / 2;
        let median = |ests: &mut Vec<GlobalMotion>, key: fn(&GlobalMotion) -> f32| {
            ests.sort_by(|a, b| key(a).partial_cmp(&key(b)).unwrap());
            key(&ests[mid])
        };
        GlobalMotion { dx: median(&mut ests, |m| m.dx), dy: median(&mut ests, |m| m.dy), confidence: median(&mut ests, |m| m.confidence) }
    }

    /// One window's estimate, window origin at `(cx, cy)`.
    pub fn estimate_at(&self, cur: &Frame, prev: &Frame, cx: usize, cy: usize) -> GlobalMotion {
        let ws = self.window;
        let lc = self.window_luma(cur, cx, cy);
        let lp = self.window_luma(prev, cx, cy);
        let mean = |v: &[f32]| (v.iter().map(|&x| x as f64).sum::<f64>() / v.len() as f64) as f32;
        let (mc, mp) = (mean(&lc), mean(&lp));

        let mut a = vec![Complex32::new(0.0, 0.0); ws * ws];
        let mut b = vec![Complex32::new(0.0, 0.0); ws * ws];
        for y in 0..ws {
            for x in 0..ws {
                let i = y * ws + x;
                let w = self.hann[x] * self.hann[y];
                a[i] = Complex32::new((lc[i] - mc) * w, 0.0);
                b[i] = Complex32::new((lp[i] - mp) * w, 0.0);
            }
        }
        self.fft2d(&mut a, true);
        self.fft2d(&mut b, true);
        for i in 0..ws * ws {
            let prod = a[i] * b[i].conj();
            let mag = prod.norm();
            a[i] = if mag > 1e-9 { prod / mag } else { Complex32::new(0.0, 0.0) };
        }
        self.fft2d(&mut a, false);

        let mut peak = f32::NEG_INFINITY;
        let (mut px, mut py) = (0usize, 0usize);
        for y in 0..ws {
            for x in 0..ws {
                let v = a[y * ws + x].re;
                if v > peak {
                    peak = v;
                    px = x;
                    py = y;
                }
            }
        }
        // Rival = tallest value outside the peak's own 5×5 footprint
        // (wrapped). Shoulders of a sharp peak are not rival candidates.
        let mut rival = f32::NEG_INFINITY;
        for y in 0..ws {
            for x in 0..ws {
                let ddx = (x as i32 - px as i32).rem_euclid(ws as i32);
                let ddy = (y as i32 - py as i32).rem_euclid(ws as i32);
                let ddx = ddx.min(ws as i32 - ddx);
                let ddy = ddy.min(ws as i32 - ddy);
                if ddx <= 2 && ddy <= 2 {
                    continue;
                }
                rival = rival.max(a[y * ws + x].re);
            }
        }
        let signed = |p: usize| if p > ws / 2 { p as i32 - ws as i32 } else { p as i32 };
        let (sx, sy) = self.subpixel(&a, px, py);
        let confidence = if rival > 0.0 && peak > 0.0 {
            ((peak - rival) / peak).clamp(0.0, 1.0)
        } else if peak > 0.0 {
            1.0
        } else {
            0.0
        };
        GlobalMotion { dx: signed(px) as f32 + sx, dy: signed(py) as f32 + sy, confidence }
    }

    fn fft2d(&self, data: &mut [Complex32], forward: bool) {
        let n = self.window;
        let fft = if forward { &self.fft_forward } else { &self.fft_inverse };
        for row in 0..n {
            fft.process(&mut data[row * n..(row + 1) * n]);
        }
        let mut col = vec![Complex32::new(0.0, 0.0); n];
        for c in 0..n {
            for r in 0..n {
                col[r] = data[r * n + c];
            }
            fft.process(&mut col);
            for r in 0..n {
                data[r * n + c] = col[r];
            }
        }
    }

    /// Parabolic fit on the peak's four wrapped neighbours; valid within ±0.5.
    fn subpixel(&self, s: &[Complex32], px: usize, py: usize) -> (f32, f32) {
        let n = self.window;
        let at = |x: usize, y: usize| s[(y % n) * n + (x % n)].re;
        let c = at(px, py);
        let (l, r) = (at(px + n - 1, py), at(px + 1, py));
        let (u, d) = (at(px, py + n - 1), at(px, py + 1));
        let fit = |m: f32, p: f32| {
            let den = m + p - 2.0 * c;
            if den.abs() > 1e-9 { (0.5 * (m - p) / den).clamp(-0.5, 0.5) } else { 0.0 }
        };
        (fit(l, r), fit(u, d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testimg::{shifted, smooth};

    #[test]
    fn recovers_integer_shift_with_sign() {
        let prev = smooth(160, 128, 3);
        let pc = PhaseCorrelation::new(64);
        for &(dx, dy) in &[(0i64, 0i64), (5, 0), (0, -3), (-4, 6), (11, 9)] {
            let cur = shifted(&prev, dx, dy);
            let m = pc.estimate(&cur, &prev);
            assert!((m.dx - dx as f32).abs() < 0.5 && (m.dy - dy as f32).abs() < 0.5, "shift {dx},{dy}: got {m:?}");
            assert!(m.confidence > 0.3, "shift {dx},{dy}: confidence {}", m.confidence);
        }
    }

    #[test]
    fn unrelated_content_has_low_confidence() {
        let a = smooth(128, 128, 1);
        let b = smooth(128, 128, 77);
        let m = PhaseCorrelation::new(64).estimate(&b, &a);
        let same = PhaseCorrelation::new(64).estimate(&a, &a);
        assert!(m.confidence < same.confidence, "cut {m:?} vs static {same:?}");
    }

    #[test]
    fn for_frame_picks_largest_power_of_two() {
        assert_eq!(PhaseCorrelation::for_frame(1920, 1080).window, 512);
        assert_eq!(PhaseCorrelation::for_frame(640, 360).window, 256);
        assert_eq!(PhaseCorrelation::for_frame(256, 256).window, 256);
    }
}
