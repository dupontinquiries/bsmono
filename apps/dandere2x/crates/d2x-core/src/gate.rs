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

/// Rows per strip when the round trip runs in parallel. JPEG's 8×8 blocks
/// (16×16 MCUs with 4:2:0 chroma) reconstruct independently — DC prediction
/// only affects entropy coding — so encoding 16-row-aligned strips gives the
/// same pixels as the whole image, on every core.
const STRIP_ALIGN: u32 = 16;

impl JpegReference {
    fn round_trip(&self, frame: &Frame) -> anyhow::Result<Frame> {
        use image::ImageEncoder;
        let mut buf = Vec::with_capacity(frame.data.len() / 4);
        {
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, self.quality.clamp(1, 100));
            enc.write_image(&frame.data, frame.width, frame.height, image::ExtendedColorType::Rgb8)?;
        }
        let img = image::ImageReader::with_format(Cursor::new(&buf), image::ImageFormat::Jpeg).decode()?.into_rgb8();
        anyhow::ensure!(img.width() == frame.width && img.height() == frame.height, "jpeg round trip changed size");
        Ok(Frame::from_rgb(frame.width, frame.height, img.into_raw())?)
    }
}

impl ReferenceMaker for JpegReference {
    fn degrade(&self, frame: &Frame) -> anyhow::Result<Frame> {
        let threads = rayon::current_num_threads().max(1) as u32;
        let strip = (frame.height.div_ceil(threads)).div_ceil(STRIP_ALIGN) * STRIP_ALIGN;
        if threads == 1 || strip >= frame.height {
            return self.round_trip(frame);
        }
        let starts: Vec<u32> = (0..frame.height).step_by(strip as usize).collect();
        let parts: Vec<Frame> = starts
            .par_iter()
            .map(|&y0| {
                let h = strip.min(frame.height - y0);
                self.round_trip(&frame.crop(0, y0, frame.width, h))
            })
            .collect::<anyhow::Result<_>>()?;
        let mut out = Frame::new(frame.width, frame.height);
        let mut y = 0usize;
        for part in parts {
            let n = part.data.len();
            let off = y * frame.width as usize * 3;
            out.data[off..off + n].copy_from_slice(&part.data);
            y += part.height as usize;
        }
        Ok(out)
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

    /// Strip-parallel encoding must reproduce the whole-image round trip
    /// exactly — the gate's numbers cannot depend on the core count.
    #[test]
    fn strip_round_trip_equals_whole_image() {
        let mut f = Frame::new(96, 200);
        for (i, b) in f.data.iter_mut().enumerate() {
            *b = ((i * 37 + (i >> 9) * 91) % 256) as u8;
        }
        let r = JpegReference { quality: 90 };
        let whole = r.round_trip(&f).unwrap();
        for threads in [2usize, 3, 5, 8] {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            let strips = pool.install(|| r.degrade(&f).unwrap());
            assert_eq!(strips, whole, "{threads} threads");
        }
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
