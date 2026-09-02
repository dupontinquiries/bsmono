//! The constant-quality gate.
//!
//! A block of the previous frame is allowed to stand in for a block of the
//! next frame when it is at least as close to the next frame as a JPEG of
//! the next frame at the user's quality setting is. "Quality 97" therefore
//! means what it says: reuse is never worse than JPEG q97 would be.
//!
//! Upstream recomputes the JPEG-side error inside every candidate test. Here
//! it is computed once per frame per block into a [`ThresholdMap`], which
//! makes each candidate test a single bounded SSE — and makes real motion
//! search affordable, which upstream never wired up.

use crate::frame::{block_sse, Frame};
use crate::plan::Grid;
use rayon::prelude::*;
use std::io::Cursor;

/// Produces the degraded reference frame the gate compares against.
pub trait ReferenceMaker: Send + Sync {
    fn degrade(&self, frame: &Frame) -> anyhow::Result<Frame>;
    fn describe(&self) -> String;
}

/// JPEG encode + decode at `quality` (1–100).
#[derive(Debug, Clone, Copy)]
pub struct JpegReference {
    pub quality: u8,
}

impl ReferenceMaker for JpegReference {
    fn degrade(&self, frame: &Frame) -> anyhow::Result<Frame> {
        use image::ImageEncoder;
        let mut buf = Vec::with_capacity(frame.data.len() / 4);
        {
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, self.quality.clamp(1, 100));
            enc.write_image(
                &frame.data,
                frame.width,
                frame.height,
                image::ExtendedColorType::Rgb8,
            )?;
        }
        let img = image::ImageReader::with_format(Cursor::new(&buf), image::ImageFormat::Jpeg)
            .decode()?
            .into_rgb8();
        anyhow::ensure!(
            img.width() == frame.width && img.height() == frame.height,
            "jpeg round trip changed size"
        );
        Ok(Frame::from_rgb(frame.width, frame.height, img.into_raw())?)
    }

    fn describe(&self) -> String {
        format!("jpeg q{}", self.quality)
    }
}

/// Per-block SSE between the next frame and its degraded reference: the
/// most error a stand-in block may have.
#[derive(Debug, Clone)]
pub struct ThresholdMap {
    pub grid: Grid,
    pub sse: Vec<u64>,
}

impl ThresholdMap {
    pub fn compute(next: &Frame, reference: &Frame, grid: Grid) -> Self {
        assert_eq!((next.width, next.height), (reference.width, reference.height));
        let bs = grid.block_size;
        let sse: Vec<u64> = (0..grid.len())
            .into_par_iter()
            .map(|i| {
                let (x, y) = grid.origin(i);
                block_sse(next, x, y, reference, x, y, bs)
            })
            .collect();
        ThresholdMap { grid, sse }
    }

    #[inline]
    pub fn get(&self, cell: usize) -> u64 {
        self.sse[cell]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn textured(w: u32, h: u32) -> Frame {
        let mut f = Frame::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = ((x * 13 + y * 7) % 97) as u8;
                f.set_px(x, y, [v, 255 - v, (x ^ y) as u8]);
            }
        }
        f
    }

    #[test]
    fn jpeg_reference_degrades_but_keeps_size() {
        let f = textured(64, 48);
        let r = JpegReference { quality: 50 }.degrade(&f).unwrap();
        assert_eq!((r.width, r.height), (64, 48));
        assert_ne!(r, f);
        let hi = JpegReference { quality: 100 }.degrade(&f).unwrap();
        assert!(f.psnr(&hi) > f.psnr(&r), "q100 should be closer than q50");
    }

    #[test]
    fn threshold_map_is_zero_against_self() {
        let f = textured(60, 60);
        let g = Grid::new(60, 60, 30);
        let t = ThresholdMap::compute(&f, &f, g);
        assert_eq!(t.sse, vec![0, 0, 0, 0]);
        let r = JpegReference { quality: 60 }.degrade(&f).unwrap();
        let t = ThresholdMap::compute(&f, &r, g);
        assert!(t.sse.iter().all(|&s| s > 0));
    }
}
