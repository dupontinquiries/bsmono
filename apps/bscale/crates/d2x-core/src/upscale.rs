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
    /// Side of the canvas that several small inputs should be packed into
    /// before one call (0 = don't pack). A GPU runner returns its tile size.
    fn canvas(&self) -> u32 {
        0
    }
    /// Upscale several frames; order preserved. Packs into canvases when
    /// [`Upscaler::canvas`] says so, so small residuals from several frames
    /// share one dispatch.
    fn upscale_many(&mut self, frames: &[&Frame]) -> anyhow::Result<Vec<Frame>> {
        crate::batch::upscale_many_dyn(self.as_dyn(), frames)
    }
    /// Object-safety shim for the default `upscale_many`; every impl returns `self`.
    #[doc(hidden)]
    fn as_dyn(&mut self) -> &mut dyn Upscaler;
}

/// Pixel replication. Exact, so tests can compare bit for bit.
#[derive(Debug, Clone, Copy)]
pub struct NearestUpscaler {
    pub scale: u32,
}

impl Upscaler for NearestUpscaler {
    fn as_dyn(&mut self) -> &mut dyn Upscaler {
        self
    }
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
    fn as_dyn(&mut self) -> &mut dyn Upscaler {
        self
    }
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

/// Runs an upscaler at its native factor and resamples the result to
/// `target` (Lanczos3). Lets a x4 network serve a x2 job — the supersampled
/// route Real-ESRGAN's own `--outscale` takes — and a x2 network serve x1
/// (a pure restoration pass).
pub struct Resampled {
    pub inner: Box<dyn Upscaler>,
    pub target: u32,
}

impl Upscaler for Resampled {
    fn as_dyn(&mut self) -> &mut dyn Upscaler {
        self
    }
    fn scale(&self) -> u32 {
        self.target
    }
    fn canvas(&self) -> u32 {
        self.inner.canvas()
    }
    fn upscale(&mut self, frame: &Frame) -> anyhow::Result<Frame> {
        let native = self.inner.upscale(frame)?;
        if self.inner.scale() == self.target {
            return Ok(native);
        }
        let img = image::RgbImage::from_raw(native.width, native.height, native.data).ok_or_else(|| anyhow::anyhow!("bad frame buffer"))?;
        let out = image::imageops::resize(&img, frame.width * self.target, frame.height * self.target, image::imageops::FilterType::Lanczos3);
        Ok(Frame::from_rgb(out.width(), out.height(), out.into_raw())?)
    }
    fn name(&self) -> String {
        format!("{} -> x{} (lanczos)", self.inner.name(), self.target)
    }
}

/// Wraps an upscaler and RCAS-sharpens its output (`crate::rcas`). Belongs
/// here rather than in the filter chain (`crate::filters`): a restoration
/// filter runs BEFORE the upscaler, on the source; sharpening compensates
/// for softness the upscaler itself introduces (regression-to-the-mean CNN
/// output, bicubic, or a reused block inheriting an earlier upscale's
/// softness — see rcas.rs's own module doc), so it has to run AFTER,
/// wrapping the upscaler's output rather than the source.
pub struct Sharpened {
    pub inner: Box<dyn Upscaler>,
    pub sharpness: crate::rcas::Sharpness,
}

impl Upscaler for Sharpened {
    fn as_dyn(&mut self) -> &mut dyn Upscaler {
        self
    }
    fn scale(&self) -> u32 {
        self.inner.scale()
    }
    fn canvas(&self) -> u32 {
        self.inner.canvas()
    }
    fn upscale(&mut self, frame: &Frame) -> anyhow::Result<Frame> {
        let mut out = self.inner.upscale(frame)?;
        crate::rcas::sharpen(&mut out, self.sharpness);
        Ok(out)
    }
    fn name(&self) -> String {
        format!("{} + rcas({:.2} stops)", self.inner.name(), self.sharpness.0)
    }
}

#[cfg(test)]
mod sharpened_tests {
    use super::*;
    use crate::rcas::Sharpness;

    #[test]
    fn wrapping_preserves_scale_and_actually_changes_pixels() {
        let mut f = Frame::new(6, 6);
        // A hard step edge down the middle -- something RCAS's cross-kernel
        // has real gradient information to act on (a flat frame is exactly
        // the case `sharpen` should leave alone, so it's not the case to
        // prove wiring with).
        for y in 0..6 {
            for x in 0..6 {
                let v = if x < 3 { 60 } else { 200 };
                f.set_px(x, y, [v, v, v]);
            }
        }
        let mut sharpened = Sharpened { inner: Box::new(NearestUpscaler { scale: 2 }), sharpness: Sharpness(0.5) };
        let base = NearestUpscaler { scale: 2 }.upscale(&f).unwrap();
        let out = sharpened.upscale(&f).unwrap();
        assert_eq!(sharpened.scale(), 2, "wrapping must not change the reported scale");
        assert_eq!((out.width, out.height), (base.width, base.height));
        assert_ne!(out.data, base.data, "RCAS on a hard edge must change at least one pixel");
    }
}
