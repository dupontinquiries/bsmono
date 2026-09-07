//! d2x-core — the bscale engine, headless.
//!
//! bscale makes video super-resolution cheap by not upscaling what has
//! not changed: each frame is compared block-by-block against the previous
//! output, blocks that pass a JPEG-quality gate are reused, and only the
//! rest is packed into a small "residual" image for the CNN. This crate is
//! that idea with nothing attached:
//!
//! - [`frame`]    — RGB8 frames and the block primitives.
//! - [`gate`]     — the quality gate (JPEG round trip → per-block threshold).
//! - [`matcher`]  — stationary / diamond / exhaustive block search.
//! - [`predict`]  — the analysis of one frame transition into a [`plan::FramePlan`].
//! - [`plate`]    — the per-window background mosaic (fao-codec's v11 plate) used as a reference.
//! - [`residual`] — residual packing and the merge that inverts it.
//! - [`upscale`]  — the `Upscaler` trait the GPU implements.
//! - [`waifu2x`]  — the `upconv_7` model, its loader, tiling, and CPU reference.
//! - [`pipeline`] — the two-stage frame loop over `FrameSource`/`FrameSink`.
//!
//! Video I/O (ffmpeg), the wgpu backend, and any UI live in other crates and
//! only ever see these traits.

pub mod courtesy;
pub mod dropped;
pub mod jobmetrics;
pub mod batch;
pub mod blockcache;
/// Covering a missed cell from cached quadrants, so a block that straddles an edge does not
/// re-do the three quarters that did not change.
pub mod subtile;
pub mod budget;
pub mod frame;
pub mod gate;
pub mod matcher;
pub mod net;
pub mod phase_corr;
pub mod pipeline;
pub mod plan;
pub mod plate;
pub mod predict;
pub mod quality;
/// Robust Contrast-Adaptive Sharpening, ported from AMD's FSR SDK (see `rcas`).
pub mod rcas;
/// Cross-worker near-match reuse: one segment reusing a block another already upscaled.
pub mod xref;
pub mod registry;
pub mod residual;
pub mod st;
pub mod upscale;
pub mod waifu2x;

/// Deterministic synthetic images for tests: a smooth field with mild
/// deterministic grain, so block-matching error surfaces look like real
/// footage (one basin per true match) rather than like a hash function.
#[cfg(test)]
pub(crate) mod testimg {
    use crate::frame::Frame;

    pub fn smooth(w: u32, h: u32, seed: u32) -> Frame {
        let mut f = Frame::new(w, h);
        let s = seed as f64 * 0.37;
        for y in 0..h {
            for x in 0..w {
                let (xf, yf) = (x as f64, y as f64);
                let base = |k: f64| 128.0 + 60.0 * ((xf / 5.3 + k + s).sin() * (yf / 7.1 - k).cos()) + 30.0 * ((xf + yf) / 11.0 + k).sin();
                let grain = |c: u32| (((x * 7 + y * 13 + c * 29 + seed) % 7) as f64) - 3.0;
                let px = |k: f64, c: u32| (base(k) + grain(c)).round().clamp(0.0, 255.0) as u8;
                f.set_px(x, y, [px(0.0, 0), px(1.7, 1), px(3.1, 2)]);
            }
        }
        f
    }

    /// `next[x, y] = prev[x - dx, y - dy]` (toroidal wrap).
    pub fn shifted(prev: &Frame, dx: i64, dy: i64) -> Frame {
        let mut n = Frame::new(prev.width, prev.height);
        for y in 0..prev.height {
            for x in 0..prev.width {
                let sx = (x as i64 - dx).rem_euclid(prev.width as i64) as u32;
                let sy = (y as i64 - dy).rem_euclid(prev.height as i64) as u32;
                n.set_px(x, y, prev.px(sx, sy));
            }
        }
        n
    }
}

pub use frame::Frame;
pub use pipeline::{run, Event, FrameSink, FrameSource, PipelineConfig, SourceInfo, Summary};
pub use plan::{FramePlan, FrameStats};
pub use predict::{AnalyzeConfig, SubpelFilter, TierMetric};
pub use upscale::Upscaler;
