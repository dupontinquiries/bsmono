//! The seam the GPU sits behind.
//!
//! The engine never knows what upscales a frame; it only needs the factor
//! and a `Frame -> Frame`. `d2x-wgpu` implements this with the waifu2x CNN
//! on Vulkan; the CPU implementations here are the test oracle (nearest is
//! an exact identity for the residual/merge round trip) and the fallback
//! when no adapter exists.

use crate::frame::Frame;

pub trait Upscaler: Send {
    /// Integer scale factor this upscaler produces.
    fn scale(&self) -> u32;
    fn upscale(&mut self, frame: &Frame) -> anyhow::Result<Frame>;
    fn name(&self) -> String;
}

/// Pixel replication. Exact, so tests can compare bit for bit.
#[derive(Debug, Clone, Copy)]
pub struct NearestUpscaler {
    pub scale: u32,
}

impl Upscaler for NearestUpscaler {
    fn scale(&self) -> u32 {
        self.scale
    }
    fn upscale(&mut self, frame: &Frame) -> anyhow::Result<Frame> {
        let s = self.scale;
        let mut out = Frame::new(frame.width * s, frame.height * s);
        for y in 0..frame.height {
            // Build one output row, then copy it s-1 more times.
            let oy = y * s;
            let mut row = Vec::with_capacity((frame.width * s * 3) as usize);
            for x in 0..frame.width {
                let p = frame.px(x, y);
                for _ in 0..s {
                    row.extend_from_slice(&p);
                }
            }
            for j in 0..s {
                let d = out.idx(0, oy + j);
                out.data[d..d + row.len()].copy_from_slice(&row);
            }
        }
        Ok(out)
    }
    fn name(&self) -> String {
        format!("nearest x{}", self.scale)
    }
}

/// Catmull-Rom (bicubic) via the `image` crate. Not a CNN, but a usable
/// fallback and a sanity baseline for "is the GPU output at least better
/// than this".
#[derive(Debug, Clone, Copy)]
pub struct BicubicUpscaler {
    pub scale: u32,
}

impl Upscaler for BicubicUpscaler {
    fn scale(&self) -> u32 {
        self.scale
    }
    fn upscale(&mut self, frame: &Frame) -> anyhow::Result<Frame> {
        let img = image::RgbImage::from_raw(frame.width, frame.height, frame.data.clone())
            .ok_or_else(|| anyhow::anyhow!("bad frame buffer"))?;
        let out = image::imageops::resize(
            &img,
            frame.width * self.scale,
            frame.height * self.scale,
            image::imageops::FilterType::CatmullRom,
        );
        Ok(Frame::from_rgb(out.width(), out.height(), out.into_raw())?)
    }
    fn name(&self) -> String {
        format!("bicubic x{}", self.scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_replicates() {
        let mut f = Frame::new(2, 1);
        f.set_px(0, 0, [1, 2, 3]);
        f.set_px(1, 0, [4, 5, 6]);
        let up = NearestUpscaler { scale: 2 }.upscale(&f).unwrap();
        assert_eq!((up.width, up.height), (4, 2));
        assert_eq!(up.px(0, 0), [1, 2, 3]);
        assert_eq!(up.px(1, 1), [1, 2, 3]);
        assert_eq!(up.px(2, 0), [4, 5, 6]);
        assert_eq!(up.px(3, 1), [4, 5, 6]);
    }

    #[test]
    fn bicubic_has_right_size() {
        let f = Frame::new(7, 5);
        let up = BicubicUpscaler { scale: 2 }.upscale(&f).unwrap();
        assert_eq!((up.width, up.height), (14, 10));
    }
}
