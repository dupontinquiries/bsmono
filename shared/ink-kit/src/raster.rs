//! Stroke rasterizer: strokes → Gray8 bitmap → binary PGM (P5).
//!
//! Purpose-built for the training pipeline (handwriting → image models) and
//! thumbnails, *not* for on-screen rendering — the apps render vector strokes
//! through their own painters (egui `Painter::add(Shape::line)`, GTK snapshot).
//!
//! Convention: **ink = 255 on background 0** (MNIST-style), so exported
//! samples drop straight into standard vision preprocessing. Stamped filled
//! discs along resampled segments; pressure scales the disc radius when the
//! stroke carries it. Deterministic — same input, same bytes — which the
//! dataset dedupe/versioning relies on.

use crate::filter::resample;
use crate::stroke::Stroke;
use crate::{RectD, Vec2d};

#[derive(Clone, Debug, PartialEq)]
pub struct Gray8 {
    pub w: usize,
    pub h: usize,
    /// Row-major, `w * h` bytes.
    pub data: Vec<u8>,
}

impl Gray8 {
    pub fn new(w: usize, h: usize) -> Self {
        Gray8 {
            w,
            h,
            data: vec![0; w * h],
        }
    }

    #[inline]
    fn stamp_disc(&mut self, cx: f64, cy: f64, r: f64) {
        let r = r.max(0.5);
        let x0 = ((cx - r).floor().max(0.0)) as usize;
        let y0 = ((cy - r).floor().max(0.0)) as usize;
        let x1 = ((cx + r).ceil().min(self.w as f64 - 1.0)).max(0.0) as usize;
        let y1 = ((cy + r).ceil().min(self.h as f64 - 1.0)).max(0.0) as usize;
        if self.w == 0 || self.h == 0 {
            return;
        }
        for y in y0..=y1 {
            for x in x0..=x1 {
                let dx = x as f64 + 0.5 - cx;
                let dy = y as f64 + 0.5 - cy;
                let d = (dx * dx + dy * dy).sqrt();
                // 1px soft edge for cheap anti-aliasing
                let cov = (r + 0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    let v = (cov * 255.0) as u8;
                    let px = &mut self.data[y * self.w + x];
                    *px = (*px).max(v);
                }
            }
        }
    }

    /// Binary PGM (P5) bytes.
    pub fn to_pgm(&self) -> Vec<u8> {
        let mut out = format!("P5\n{} {}\n255\n", self.w, self.h).into_bytes();
        out.extend_from_slice(&self.data);
        out
    }
}

/// Rasterize strokes into a `size`×`size` image, fitting the strokes' joint
/// bbox with `margin_frac` border, preserving aspect (centered). `base_px`
/// is the stroke radius in output pixels at pressure 0.5.
pub fn rasterize(strokes: &[Stroke], size: usize, margin_frac: f64, base_px: f64) -> Gray8 {
    let mut img = Gray8::new(size, size);
    let mut bounds = RectD::EMPTY;
    for s in strokes {
        let b = s.bbox();
        if !b.is_empty() {
            bounds.include(b.min);
            bounds.include(b.max);
        }
    }
    if bounds.is_empty() || size == 0 {
        return img;
    }
    let span = bounds.width().max(bounds.height()).max(1e-9);
    let avail = size as f64 * (1.0 - 2.0 * margin_frac);
    let scale = avail / span;
    let cx = (bounds.min.x + bounds.max.x) * 0.5;
    let cy = (bounds.min.y + bounds.max.y) * 0.5;
    let half = size as f64 * 0.5;
    let map = |p: Vec2d| -> (f64, f64) { ((p.x - cx) * scale + half, (p.y - cy) * scale + half) };

    for s in strokes {
        if s.points.is_empty() {
            continue;
        }
        // resample in world units so stamp spacing ≈ half a pixel in output
        let step_world = (0.5 / scale).max(1e-9);
        let pts = resample(&s.points, step_world);
        for p in &pts {
            let (x, y) = map(p.pos);
            let r = base_px * (0.4 + 1.2 * p.pressure as f64);
            img.stamp_disc(x, y, r);
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stroke::{InkPoint, StrokeId, StrokeStyle};

    fn line_stroke() -> Stroke {
        let mut s = Stroke::new(StrokeId(1), StrokeStyle::default());
        s.points.push(InkPoint::xy(0.0, 0.0));
        s.points.push(InkPoint::xy(100.0, 100.0));
        s
    }

    #[test]
    fn draws_ink_and_is_deterministic() {
        let s = line_stroke();
        let a = rasterize(&[s.clone()], 64, 0.1, 1.5);
        let b = rasterize(&[s], 64, 0.1, 1.5);
        assert_eq!(a, b);
        let inked = a.data.iter().filter(|&&v| v > 128).count();
        assert!(inked > 50, "diagonal should mark pixels, got {}", inked);
        // corners of the diagonal land inside the margin
        assert_eq!(a.data[0], 0, "margin corner stays background");
    }

    #[test]
    fn pgm_header() {
        let img = rasterize(&[line_stroke()], 32, 0.1, 1.0);
        let pgm = img.to_pgm();
        assert!(pgm.starts_with(b"P5\n32 32\n255\n"));
        assert_eq!(pgm.len(), 13 + 32 * 32);
    }

    #[test]
    fn empty_input_is_blank() {
        let img = rasterize(&[], 16, 0.1, 1.0);
        assert!(img.data.iter().all(|&v| v == 0));
    }
}
