//! d2x-core — the dandere2x engine, headless.
//!
//! dandere2x makes video super-resolution cheap by not upscaling what has
//! not changed: each frame is compared block-by-block against the previous
//! output, blocks that pass a JPEG-quality gate are reused, and only the
//! rest is packed into a small "residual" image for the CNN. This crate is
//! that idea with nothing attached:
//!
//! - [`frame`]    — RGB8 frames and the block primitives.
//! - [`gate`]     — the quality gate (JPEG round trip → per-block threshold).
//! - [`matcher`]  — stationary / diamond / exhaustive block search.
//! - [`predict`]  — the analysis of one frame transition into a [`plan::FramePlan`].
//! - [`residual`] — residual packing and the merge that inverts it.
//! - [`upscale`]  — the `Upscaler` trait the GPU implements.
//! - [`waifu2x`]  — the `upconv_7` model, its loader, tiling, and CPU reference.
//! - [`pipeline`] — the two-stage frame loop over `FrameSource`/`FrameSink`.
//!
//! Video I/O (ffmpeg), the wgpu backend, and any UI live in other crates and
//! only ever see these traits.

pub mod frame;
pub mod gate;
pub mod matcher;
pub mod net;
pub mod pipeline;
pub mod plan;
pub mod predict;
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
pub use predict::AnalyzeConfig;
pub use upscale::Upscaler;

// ── model location ───────────────────────────────────────────────────────────
//
// Lives in the CORE, not the CLI, because there are now two consumers — the
// dandere2x binary and beatspace's `upscale` feature — and a second copy of a
// search path is how two callers end up disagreeing about where models are.
// The CLI's `job::model_search_dirs` should delegate here.

/// Directories searched for a model, in order. The operator's own directory
/// comes first: the useful checkpoints are 67 MB and belong in `~/models`
/// beside this monorepo's other weights, not in git.
pub fn model_search_dirs() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut v = Vec::new();
    if let Ok(d) = std::env::var("DANDERE2X_MODELS") { v.push(PathBuf::from(d)); }
    if let Ok(home) = std::env::var("HOME") {
        let h = PathBuf::from(home);
        v.push(h.join("models/upscale"));
        v.push(h.join("models"));
        v.push(h.join(".local/share/dandere2x/models"));
    }
    if let Ok(exe) = std::env::current_exe() {
        for anc in exe.ancestors().take(5) { v.push(anc.join("models")); }
    }
    v.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models"));
    v
}

/// The same list as strings, for an error message that says where it looked.
pub fn model_search_dirs_display() -> Vec<String> {
    model_search_dirs().iter().map(|p| p.display().to_string()).collect()
}

/// The default model and whether it was the vendored copy.
///
/// `bool` is `true` when the hit came from the in-tree `models/` directory, so
/// a caller can say "using the 2.5 MB default" rather than implying the
/// operator chose it.
pub fn resolve_default_model() -> Option<(std::path::PathBuf, bool)> {
    let vendored_tail = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models");
    for d in model_search_dirs() {
        let c = d.join("realesr-animevideov3.safetensors");
        if c.is_file() {
            let is_vendored = d == vendored_tail
                || std::env::current_exe().ok()
                    .map(|e| e.ancestors().take(5).any(|a| a.join("models") == d))
                    .unwrap_or(false);
            return Some((c, is_vendored));
        }
    }
    None
}

#[cfg(test)]
mod model_dirs_tests {
    use super::*;

    #[test]
    fn the_operators_directory_is_searched_before_anything_vendored() {
        // The whole point of the reshape: a checkout must not shadow a model
        // the operator deliberately placed in ~/models.
        let dirs = model_search_dirs();
        let home_at = dirs.iter().position(|d| d.ends_with("models/upscale"));
        let vendored_at = dirs.iter().rposition(|d| d.ends_with("models"));
        if let (Some(h), Some(v)) = (home_at, vendored_at) {
            assert!(h < v, "vendored dir is searched before ~/models: {dirs:?}");
        }
    }

    #[test]
    fn the_env_override_wins_outright() {
        // Serialised via a fixed name; the VALUE is what is asserted, not order
        // against other tests.
        unsafe { std::env::set_var("DANDERE2X_MODELS", "/tmp/d2x-override-test") };
        let first = model_search_dirs().first().cloned();
        unsafe { std::env::remove_var("DANDERE2X_MODELS") };
        assert_eq!(first, Some(std::path::PathBuf::from("/tmp/d2x-override-test")));
    }

    #[test]
    fn the_display_list_is_never_empty_so_an_error_can_always_say_where() {
        assert!(!model_search_dirs_display().is_empty());
    }
}
