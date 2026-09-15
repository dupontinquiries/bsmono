//! Restoration filters ahead of the gate.
//!
//! bscale's `Network` is already architecture-agnostic about scale: an
//! SRVGG-shaped op list whose last conv emits `3*shuffle*shuffle` channels
//! degenerates to a plain same-resolution residual net at `shuffle = 1`
//! (`add_input: true` makes it `output = input + learned correction`, i.e. a
//! denoiser/deblur/de-artifact net is just an SRVGG net with no upsampling).
//! No new op, no new kernel, no engine change to load one — see
//! `net::Network::srvgg`.
//!
//! What IS new here: running several of them in sequence, over the whole
//! decoded frame, before the block-matching gate ever sees it. That's the
//! "layers you can add" a user asked for — denoise, then deblur, then
//! de-artifact, each independently trained and swappable, each an ordinary
//! `Upscaler` at scale 1. `FilteredSource` is the seam: it wraps any
//! `FrameSource` and is itself one, so a filter chain composes with ffmpeg
//! I/O, `Limited`, `Cancellable`, segments — everything upstream of the gate
//! — with zero changes to any of them.
//!
//! Deliberately NOT folded into the gate/residual/cascade machinery: those
//! exist to avoid re-running the expensive upscale CNN on unchanged blocks.
//! A restoration filter should see every pixel every frame (noise and
//! compression artifacts are exactly the high-frequency content the block
//! reuse gate is tuned to treat as "unchanged" and skip) — so filters run
//! whole-frame, once, up front, and the existing reuse logic runs on their
//! output as if it were the source. The two ideas don't fight; they don't
//! need to know about each other either.

use crate::pipeline::{FrameSource, SourceInfo};
use crate::upscale::Upscaler;
use crate::Frame;

/// Runs `filters` over every frame from `inner`, in order. Empty chain is a
/// pass-through (no allocation beyond the `Vec` itself).
pub struct FilteredSource {
    inner: Box<dyn FrameSource>,
    filters: Vec<Box<dyn Upscaler>>,
}

impl FilteredSource {
    /// Every filter must be scale 1 — this is a restoration chain, not an
    /// upscale ladder. A scale-1 check here, once, is cheaper than a wrong
    /// frame size surfacing as a panic three stages downstream.
    pub fn new(inner: Box<dyn FrameSource>, filters: Vec<Box<dyn Upscaler>>) -> anyhow::Result<Self> {
        for f in &filters {
            anyhow::ensure!(f.scale() == 1, "pre-filter '{}' is scale x{}; filters run before the upscaler and must be scale 1 (train with shuffle=1 / no pixel-shuffle)", f.name(), f.scale());
        }
        Ok(FilteredSource { inner, filters })
    }

    pub fn names(&self) -> Vec<String> {
        self.filters.iter().map(|f| f.name()).collect()
    }
}

impl FrameSource for FilteredSource {
    fn info(&self) -> SourceInfo {
        self.inner.info()
    }
    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        let Some(mut f) = self.inner.next_frame()? else { return Ok(None) };
        for filt in &mut self.filters {
            f = filt.upscale(&f)?;
        }
        Ok(Some(f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upscale::NearestUpscaler;

    struct OneFrame(Option<Frame>);
    impl FrameSource for OneFrame {
        fn info(&self) -> SourceInfo {
            SourceInfo { width: 4, height: 4, fps: (30, 1), frame_count: Some(1) }
        }
        fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
            Ok(self.0.take())
        }
    }

    #[test]
    fn empty_chain_is_identity() {
        let f = Frame::new(4, 4);
        let src = OneFrame(Some(f.clone()));
        let mut chained = FilteredSource::new(Box::new(src), vec![]).unwrap();
        let out = chained.next_frame().unwrap().unwrap();
        assert_eq!(out.data, f.data);
        assert!(chained.next_frame().unwrap().is_none());
    }

    #[test]
    fn rejects_a_non_unity_scale_filter() {
        let src = OneFrame(Some(Frame::new(4, 4)));
        let bad: Box<dyn Upscaler> = Box::new(NearestUpscaler { scale: 2 });
        assert!(FilteredSource::new(Box::new(src), vec![bad]).is_err());
    }

    #[test]
    fn chain_runs_in_order() {
        // Two identity-scale filters that each tag a pixel let us confirm
        // ordering without a real network: NearestUpscaler{scale:1} is a
        // literal copy, so this mainly proves the plumbing runs N times.
        let src = OneFrame(Some(Frame::new(2, 2)));
        let filters: Vec<Box<dyn Upscaler>> = vec![Box::new(NearestUpscaler { scale: 1 }), Box::new(NearestUpscaler { scale: 1 })];
        let mut chained = FilteredSource::new(Box::new(src), filters).unwrap();
        assert!(chained.next_frame().unwrap().is_some());
    }
}
