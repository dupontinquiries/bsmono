//! The frame loop, as two stages joined by a bounded channel:
//!
//! ```text
//!   source ──▶ [A: pad, noise, analyze, residual] ──chan──▶ [B: upscale, merge] ──▶ sink
//! ```
//!
//! A is CPU (rayon inside `analyze`); B is wherever the [`Upscaler`] runs.
//! A only depends on its own previous output (the low-res reconstruction),
//! so it never waits for the GPU, and B never waits for analysis unless A is
//! actually the slower stage. That is the whole reason for the split, and
//! the channel depth is small on purpose: a deep queue would hide which
//! stage is the bottleneck, and [`Event::FrameDone`] reports both timings
//! so the loop is legible from outside.

use crate::frame::{Frame, Pad};
use crate::gate::{JpegReference, ReferenceMaker};
use crate::matcher::{BlockMatcher, Diamond, Exhaustive, Stationary};
use crate::plan::{FramePlan, FrameStats};
use crate::predict::{analyze, AnalyzeConfig};
use crate::residual::{make_residual, merge, Residual};
use crate::upscale::Upscaler;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceInfo {
    pub width: u32,
    pub height: u32,
    /// Frames per second as a rational, e.g. `(24000, 1001)`.
    pub fps: (u32, u32),
    pub frame_count: Option<u64>,
}

pub trait FrameSource: Send {
    fn info(&self) -> SourceInfo;
    /// `Ok(None)` at end of stream.
    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>>;
}

pub trait FrameSink: Send {
    fn write(&mut self, frame: &Frame) -> anyhow::Result<()>;
    fn finish(&mut self) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatcherKind {
    Stationary,
    Diamond,
    Exhaustive { radius: u32 },
}

impl MatcherKind {
    pub fn build(self) -> Box<dyn BlockMatcher> {
        match self {
            MatcherKind::Stationary => Box::new(Stationary),
            MatcherKind::Diamond => Box::new(Diamond::default()),
            MatcherKind::Exhaustive { radius } => Box::new(Exhaustive { radius }),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PipelineConfig {
    pub analyze: AnalyzeConfig,
    /// JPEG quality for the gate.
    pub quality: u8,
    /// Uniform noise strength added to every input frame (0 = off).
    pub noise: u8,
    pub matcher: MatcherKind,
    /// Frames queued between the two stages.
    pub queue_depth: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        PipelineConfig {
            analyze: AnalyzeConfig::default(),
            quality: 97,
            noise: 8,
            matcher: MatcherKind::Stationary,
            queue_depth: 2,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Started { info: SourceInfo, padded: (u32, u32), upscaler: String, matcher: &'static str, gate: String },
    /// Emitted by stage B after each output frame is written.
    FrameDone { index: u64, stats: FrameStats, analyze_time: Duration, upscale_time: Duration, merge_time: Duration },
    Finished { frames: u64, elapsed: Duration, upscaled_pixel_fraction: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub frames: u64,
    pub elapsed: Duration,
    /// Mean over frames of the fraction of pixels that went through the
    /// upscaler. 1.0 means dandere2x saved nothing.
    pub upscaled_pixel_fraction: f64,
}

/// What stage A sends to stage B per frame.
struct Work {
    index: u64,
    plan: Option<FramePlan>, // None for the first frame
    residual: Residual,
    analyze_time: Duration,
}

/// Pad up to a block-size multiple (replicate) and add noise.
fn prepare(frame: Frame, bs: u32, noise: u8, seed: u64) -> Frame {
    let pw = frame.width.div_ceil(bs) * bs;
    let ph = frame.height.div_ceil(bs) * bs;
    let mut f = if (pw, ph) == (frame.width, frame.height) { frame } else { frame.padded_to(pw, ph, 0, 0, Pad::Replicate) };
    f.add_uniform_noise(noise, seed);
    f
}

/// Run the whole pipeline. `on_event` is called from the calling thread.
pub fn run(
    mut source: Box<dyn FrameSource>,
    mut upscaler: Box<dyn Upscaler>,
    mut sink: Box<dyn FrameSink>,
    cfg: PipelineConfig,
    mut on_event: impl FnMut(Event),
) -> anyhow::Result<Summary> {
    let info = source.info();
    let bs = cfg.analyze.block_size;
    let bleed = cfg.analyze.bleed;
    let scale = upscaler.scale();
    anyhow::ensure!(bs >= 4, "block size {bs} is too small");
    anyhow::ensure!(info.width >= 1 && info.height >= 1, "empty source");
    let padded = (info.width.div_ceil(bs) * bs, info.height.div_ceil(bs) * bs);
    let matcher = cfg.matcher.build();
    let reference = JpegReference { quality: cfg.quality };
    on_event(Event::Started {
        info,
        padded,
        upscaler: upscaler.name(),
        matcher: matcher.name(),
        gate: reference.describe(),
    });

    let started = Instant::now();
    let (tx, rx) = mpsc::sync_channel::<anyhow::Result<Work>>(cfg.queue_depth.max(1));
    let acfg = cfg.analyze;
    let noise = cfg.noise;

    // Stage A.
    let analyzer = std::thread::Builder::new().name("d2x-analyze".into()).spawn(move || {
        let reference: &dyn ReferenceMaker = &reference;
        let mut index: u64 = 0;
        let mut prev_working: Option<Frame> = None;
        loop {
            let raw = match source.next_frame() {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    break;
                }
            };
            let t0 = Instant::now();
            let frame = prepare(raw, bs, noise, index);
            let work = match prev_working.take() {
                None => {
                    prev_working = Some(frame.clone());
                    Work { index, plan: None, residual: Residual::Whole(frame), analyze_time: t0.elapsed() }
                }
                Some(mut prev) => {
                    let r = analyze(&mut prev, &frame, reference, matcher.as_ref(), &acfg);
                    match r {
                        Ok((plan, working_next)) => {
                            let residual = make_residual(&frame, &plan, bleed);
                            prev_working = Some(working_next);
                            Work { index, plan: Some(plan), residual, analyze_time: t0.elapsed() }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e));
                            break;
                        }
                    }
                }
            };
            if tx.send(Ok(work)).is_err() {
                break; // stage B went away
            }
            index += 1;
        }
    })?;

    // Stage B, on this thread.
    let crop_w = info.width * scale;
    let crop_h = info.height * scale;
    let mut prev_up: Option<Frame> = None;
    let mut frames: u64 = 0;
    let mut fraction_sum = 0.0;
    let mut result: anyhow::Result<()> = Ok(());

    for msg in rx.iter() {
        let work = match msg {
            Ok(w) => w,
            Err(e) => {
                result = Err(e);
                break;
            }
        };
        let t0 = Instant::now();
        let residual_up = match work.residual.frame() {
            Some(f) => match upscaler.upscale(f) {
                Ok(u) => Some(u),
                Err(e) => {
                    result = Err(e.context(format!("upscaling frame {}", work.index)));
                    break;
                }
            },
            None => None,
        };
        let upscale_time = t0.elapsed();

        let t1 = Instant::now();
        let (out, stats) = match (&work.plan, prev_up.as_ref()) {
            (None, _) => {
                let f = residual_up.expect("first frame is always upscaled whole");
                (f, FrameStats { redraw: true, psnr: f64::NAN, ..Default::default() })
            }
            (Some(plan), Some(prev)) => match merge(prev, residual_up.as_ref(), plan, scale, bleed) {
                Ok(f) => (f, plan.stats),
                Err(e) => {
                    result = Err(e.context(format!("merging frame {}", work.index)));
                    break;
                }
            },
            (Some(_), None) => {
                result = Err(anyhow::anyhow!("frame {} has a plan but no previous output", work.index));
                break;
            }
        };
        let merge_time = t1.elapsed();

        let visible = if (out.width, out.height) == (crop_w, crop_h) { out.clone() } else { out.crop(0, 0, crop_w, crop_h) };
        if let Err(e) = sink.write(&visible) {
            result = Err(e.context(format!("writing frame {}", work.index)));
            break;
        }
        let fraction = match &work.plan {
            None => 1.0,
            Some(p) => p.stats.upscaled_fraction(&p.grid, bleed),
        };
        fraction_sum += fraction;
        frames += 1;
        prev_up = Some(out);
        on_event(Event::FrameDone {
            index: work.index,
            stats,
            analyze_time: work.analyze_time,
            upscale_time,
            merge_time,
        });
    }
    drop(rx);
    let _ = analyzer.join();
    result?;
    sink.finish()?;
    let elapsed = started.elapsed();
    let upscaled_pixel_fraction = if frames > 0 { fraction_sum / frames as f64 } else { 0.0 };
    on_event(Event::Finished { frames, elapsed, upscaled_pixel_fraction });
    Ok(Summary { frames, elapsed, upscaled_pixel_fraction })
}

/// Analysis only: run stage A over the source and report per-frame stats,
/// never touching an upscaler. This is the `analyze` subcommand and the
/// cheapest way to see what a block size / quality setting buys on a clip.
pub fn analyze_only(
    mut source: Box<dyn FrameSource>,
    cfg: PipelineConfig,
    mut on_frame: impl FnMut(u64, &FrameStats, f64, Duration),
) -> anyhow::Result<Summary> {
    let bs = cfg.analyze.block_size;
    let bleed = cfg.analyze.bleed;
    let matcher = cfg.matcher.build();
    let reference = JpegReference { quality: cfg.quality };
    let started = Instant::now();
    let mut prev: Option<Frame> = None;
    let mut index = 0u64;
    let mut fraction_sum = 0.0;
    while let Some(raw) = source.next_frame()? {
        let t0 = Instant::now();
        let frame = prepare(raw, bs, cfg.noise, index);
        match prev.take() {
            None => {
                fraction_sum += 1.0;
                on_frame(index, &FrameStats { redraw: true, psnr: f64::NAN, ..Default::default() }, 1.0, t0.elapsed());
                prev = Some(frame);
            }
            Some(mut p) => {
                let (plan, next_w) = analyze(&mut p, &frame, &reference, matcher.as_ref(), &cfg.analyze)?;
                let fr = plan.stats.upscaled_fraction(&plan.grid, bleed);
                fraction_sum += fr;
                on_frame(index, &plan.stats, fr, t0.elapsed());
                prev = Some(next_w);
            }
        }
        index += 1;
    }
    Ok(Summary { frames: index, elapsed: started.elapsed(), upscaled_pixel_fraction: if index > 0 { fraction_sum / index as f64 } else { 0.0 } })
}

// ---------------------------------------------------------------------------
// In-memory source/sink, for tests and for embedding.

pub struct VecSource {
    pub info: SourceInfo,
    pub frames: std::collections::VecDeque<Frame>,
}

impl VecSource {
    pub fn new(frames: Vec<Frame>, fps: (u32, u32)) -> Self {
        let (w, h) = frames.first().map(|f| (f.width, f.height)).unwrap_or((0, 0));
        let n = frames.len() as u64;
        VecSource { info: SourceInfo { width: w, height: h, fps, frame_count: Some(n) }, frames: frames.into() }
    }
}

impl FrameSource for VecSource {
    fn info(&self) -> SourceInfo {
        self.info
    }
    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        Ok(self.frames.pop_front())
    }
}

pub struct VecSink {
    pub frames: std::sync::Arc<std::sync::Mutex<Vec<Frame>>>,
}

impl FrameSink for VecSink {
    fn write(&mut self, frame: &Frame) -> anyhow::Result<()> {
        self.frames.lock().unwrap().push(frame.clone());
        Ok(())
    }
    fn finish(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testimg::smooth;
    use crate::upscale::NearestUpscaler;
    use std::sync::{Arc, Mutex};

    /// End to end with nearest upscaling and no noise: the output must be
    /// exactly the nearest-upscale of each frame's low-res reconstruction,
    /// which for an unchanged frame is the frame itself. Also exercises a
    /// non-block-multiple size (pad and crop).
    #[test]
    fn pipeline_is_exact_with_nearest_and_no_noise() {
        let f0 = smooth(70, 50, 0);
        let f1 = f0.clone(); // identical
        let mut f2 = f0.clone();
        for y in 10..26 {
            for x in 20..36 {
                f2.set_px(x, y, [200, 10, 10]);
            }
        }
        let mut f3 = Frame::new(70, 50); // scene cut
        for (i, b) in f3.data.iter_mut().enumerate() {
            *b = (i * 31 % 251) as u8;
        }
        let frames = vec![f0.clone(), f1.clone(), f2.clone(), f3.clone()];
        let out = Arc::new(Mutex::new(Vec::new()));
        let cfg = PipelineConfig {
            analyze: AnalyzeConfig { block_size: 16, bleed: 1, ..Default::default() },
            noise: 0,
            ..Default::default()
        };
        let mut events = Vec::new();
        let summary = run(
            Box::new(VecSource::new(frames, (30, 1))),
            Box::new(NearestUpscaler { scale: 2 }),
            Box::new(VecSink { frames: out.clone() }),
            cfg,
            |e| events.push(e),
        )
        .unwrap();
        assert_eq!(summary.frames, 4);
        let out = out.lock().unwrap();
        assert_eq!(out.len(), 4);
        for f in out.iter() {
            assert_eq!((f.width, f.height), (140, 100));
        }
        let mut up = NearestUpscaler { scale: 2 };
        assert_eq!(out[0], up.upscale(&f0).unwrap());
        assert_eq!(out[1], up.upscale(&f1).unwrap());
        // Frame 2: the changed block is exact; the rest came from frame 0
        // through the gate, which with bit-identical frames is also exact.
        assert_eq!(out[2], up.upscale(&f2).unwrap());
        assert_eq!(out[3], up.upscale(&f3).unwrap());
        let done = events.iter().filter(|e| matches!(e, Event::FrameDone { .. })).count();
        assert_eq!(done, 4);
        assert!(summary.upscaled_pixel_fraction < 1.0);
    }

    #[test]
    fn analyze_only_reports_savings() {
        let f0 = smooth(64, 64, 0);
        let frames = vec![f0.clone(), f0.clone(), f0.clone()];
        let cfg = PipelineConfig { noise: 0, analyze: AnalyzeConfig { block_size: 16, ..Default::default() }, ..Default::default() };
        let mut n = 0;
        let s = analyze_only(Box::new(VecSource::new(frames, (30, 1))), cfg, |_, _, _, _| n += 1).unwrap();
        assert_eq!(n, 3);
        assert_eq!(s.frames, 3);
        // first frame whole, two identical frames free
        assert!((s.upscaled_pixel_fraction - 1.0 / 3.0).abs() < 1e-9);
    }
}
