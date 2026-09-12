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
use crate::phase_corr::PhaseCorrelation;
use crate::plate::Plate;
use crate::plan::{FramePlan, FrameStats, MAX_TIERS};
use crate::blockcache::{BlockCache, Entry};
use crate::predict::{analyze_tiered, AnalyzeConfig, AnalyzeState, CachedBlocks};
use std::sync::{Arc, Mutex};
use crate::residual::{make_residual, merge_tiered, Residual};
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
    /// Frames queued between the two stages. Also bounds how many frames'
    /// residuals stage B can batch into one GPU call.
    pub queue_depth: usize,
    /// Recent outputs kept as extra references (occlusions, flicker).
    pub recent_refs: usize,
    /// Shot-start (redraw) outputs retained as references (cut-backs).
    pub keyframe_refs: usize,
    /// Max frames whose residuals share one upscale call.
    pub batch: usize,
    /// For `analyze_only`: how many upscaler tiers to assume when routing
    /// (the real run uses the number of upscalers it is given).
    pub tiers: usize,
    /// Byte budget of the content-keyed block cache (0 = off). Shared
    /// across segment workers when `run_tiered` is given one.
    pub block_cache_bytes: u64,
    /// Byte budget of the CROSS-WORKER near-match store (0 = off), shared with the block
    /// cache across segment workers.
    ///
    /// The block cache above is exact-match: a cell whose bytes were upscaled before. This
    /// one is keyed by a coarse fingerprint and decided by the SAME SSE budget the reference
    /// ring uses — so a cut back to a shot another segment already rendered can be reused
    /// even though nothing matches byte for byte. See `crate::xref` for why the fingerprint
    /// is never allowed to be the authority.
    pub xref_bytes: u64,
    /// Frames per plate window (0 = no plate). Stage A holds a whole window
    /// before analysing its first frame, so this costs `window · frame`
    /// bytes of RAM and one window of latency; a window also ends early at
    /// a scene cut (raw-frame PSNR below `analyze.min_psnr`). Each window's
    /// plate is upscaled once by the main tier and serves `Plated` blocks.
    pub plate_window: usize,
    /// Pairwise phase-correlation confidence below which a frame's motion
    /// is held rather than accumulated into the plate placement.
    pub plate_min_confidence: f32,
    /// Seed the block motion search with each frame's global motion (phase
    /// correlation, one estimate per frame pair). Lets the diamond search
    /// reach pans beyond its radius; costs one phase correlation per frame.
    pub mv_seed: bool,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        PipelineConfig {
            analyze: AnalyzeConfig::default(),
            quality: 97,
            noise: 8,
            matcher: MatcherKind::Stationary,
            queue_depth: 8,
            recent_refs: 3,
            keyframe_refs: 4,
            batch: 4,
            tiers: 1,
            block_cache_bytes: 0,
            xref_bytes: 0,
            plate_window: 0,
            plate_min_confidence: 0.2,
            mv_seed: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Started { info: SourceInfo, padded: (u32, u32), upscaler: String, matcher: &'static str, gate: String },
    /// Emitted by stage B after each output frame is written.
    FrameDone { index: u64, stats: FrameStats, analyze_time: Duration, upscale_time: Duration, merge_time: Duration },
    /// A window's plate went through the main tier: `frames` is how many
    /// frames it will serve, `area` its size in frame units, so its cost per
    /// served frame is `area / frames`.
    PlateUpscaled { index: u64, size: (u32, u32), frames: u64, area: f64, time: Duration },
    Finished { frames: u64, elapsed: Duration, upscaled_pixel_fraction: f64, heavy_pixel_fraction: f64, plate_pixel_fraction: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub frames: u64,
    pub elapsed: Duration,
    /// Mean over frames of the fraction of pixels that went through any
    /// network tier. 1.0 means bscale saved nothing.
    pub upscaled_pixel_fraction: f64,
    /// Mean fraction that went through the *main* network (tier 0) — what the
    /// expensive model's time follows.
    pub heavy_pixel_fraction: f64,
    /// Plate pixels upscaled by the main tier, in frame units, per frame:
    /// the amortised cost the two fractions above do not include. Total
    /// main-network work is `heavy_pixel_fraction + plate_pixel_fraction`.
    pub plate_pixel_fraction: f64,
    /// Block cache at the end of the run: hits, misses, inserts, live bytes.
    pub cache: (u64, u64, u64, u64),
}

/// What stage A sends to stage B per frame.
struct Work {
    index: u64,
    plan: Option<FramePlan>, // None for the first frame
    residual: Residual,
    cached: CachedBlocks,
    analyze_time: Duration,
    /// Set on the first frame of a plate window: the plate stage B must
    /// upscale before merging this and the window's following frames.
    plate: Option<(Frame, u64)>,
}

/// The reference bookkeeping both stages mirror, so `Recalled { r }` means
/// the same frame on the low-res and the upscaled side.
struct Ring<T> {
    recent: std::collections::VecDeque<T>,
    keyframes: std::collections::VecDeque<T>,
    recent_cap: usize,
    keyframe_cap: usize,
}

impl<T: Clone> Ring<T> {
    fn new(recent_cap: usize, keyframe_cap: usize) -> Self {
        Ring { recent: Default::default(), keyframes: Default::default(), recent_cap, keyframe_cap }
    }
    /// References in `Recalled` index order: recent (newest first), then
    /// retained shot starts (newest first).
    fn refs(&self) -> Vec<&T> {
        self.recent.iter().chain(self.keyframes.iter()).collect()
    }
    /// Index of the newest anchor in `refs()`, if any.
    fn anchor(&self) -> Option<u8> {
        if self.keyframes.is_empty() || self.recent.len() >= 255 { None } else { Some(self.recent.len() as u8) }
    }
    /// After a frame is finished: `prev` becomes a recent reference; a redraw
    /// frame is also retained as a shot start.
    fn advance(&mut self, prev: T, this: &T, this_is_redraw: bool) {
        if self.recent_cap > 0 {
            self.recent.push_front(prev);
            self.recent.truncate(self.recent_cap);
        }
        if this_is_redraw && self.keyframe_cap > 0 {
            self.keyframes.push_front(this.clone());
            self.keyframes.truncate(self.keyframe_cap);
        }
    }
}

/// Pad up to a block-size multiple (replicate) and add noise.
fn prepare(frame: Frame, bs: u32, noise: u8, seed: u64) -> Frame {
    let pw = frame.width.div_ceil(bs) * bs;
    let ph = frame.height.div_ceil(bs) * bs;
    let mut f = if (pw, ph) == (frame.width, frame.height) { frame } else { frame.padded_to(pw, ph, 0, 0, Pad::Replicate) };
    f.add_uniform_noise(noise, seed);
    f
}

/// Reads a source in plate windows: up to `window` prepared frames, ended
/// early at a scene cut so a plate never straddles two shots. A cut is a
/// frame pair whose raw PSNR is below `min_psnr` OR whose phase-correlation
/// confidence is below `min_confidence` — the second catches what the
/// first misses (a cut between two busy frames of similar statistics), and
/// the motion it measures is what the plate is placed by, so nothing is
/// estimated twice. `window <= 1` degenerates to one frame at a time.
struct WindowReader {
    source: Box<dyn FrameSource>,
    window: usize,
    min_psnr: f64,
    min_confidence: f32,
    bs: u32,
    noise: u8,
    index: u64,
    /// First frame of the next window, already prepared, with its motion
    /// relative to the frame before it (None across a cut).
    pending: Option<(Frame, Option<(f32, f32)>)>,
    done: bool,
    pc: Option<PhaseCorrelation>,
    /// `D2X_PLATE_DEBUG=1`: print every pair's motion and every window.
    debug: bool,
}

/// One window: its first frame's index, its frames, and their frame-0-
/// relative displacements and pairwise confidences (see `Plate::build_placed`).
struct Window {
    start: u64,
    frames: Vec<Frame>,
    disp: Vec<(f32, f32)>,
    confidence: Vec<f32>,
    /// Each frame's motion relative to its predecessor (`cur[x] ≈
    /// prev[x − dx]`); `None` for the first frame after a cut or at the
    /// start of the source.
    pair: Vec<Option<(f32, f32)>>,
}

impl Window {
    /// The block-search hint for frame `i`: where its content sits in the
    /// previous frame, as a source offset.
    fn hint(&self, i: usize) -> Option<(i64, i64)> {
        self.pair[i].map(|(dx, dy)| (-(dx.round() as i64), -(dy.round() as i64)))
    }
    fn plate(&self, min_confidence: f32) -> Plate {
        let refs: Vec<&Frame> = self.frames.iter().collect();
        let _ = min_confidence; // motion below it already ended the window
        Plate::build_placed(&refs, &self.disp, self.confidence.clone())
    }
}

impl WindowReader {
    fn new(source: Box<dyn FrameSource>, window: usize, min_psnr: f64, min_confidence: f32, bs: u32, noise: u8) -> Self {
        let debug = std::env::var("D2X_PLATE_DEBUG").map(|v| v != "0").unwrap_or(false);
        WindowReader { source, window: window.max(1), min_psnr, min_confidence, bs, noise, index: 0, pending: None, done: false, pc: None, debug }
    }
    fn read_one(&mut self) -> anyhow::Result<Option<Frame>> {
        if self.done {
            return Ok(None);
        }
        match self.source.next_frame()? {
            Some(raw) => {
                let f = prepare(raw, self.bs, self.noise, self.index);
                self.index += 1;
                Ok(Some(f))
            }
            None => {
                self.done = true;
                Ok(None)
            }
        }
    }
    fn next_window(&mut self) -> anyhow::Result<Option<Window>> {
        let (first, first_pair) = match self.pending.take() {
            Some(f) => f,
            None => match self.read_one()? {
                Some(f) => (f, None),
                None => return Ok(None),
            },
        };
        let start = self.index - 1;
        if self.window > 1 && self.pc.is_none() {
            self.pc = Some(PhaseCorrelation::for_frame(first.width, first.height));
        }
        let mut win = Window { start, frames: vec![first], disp: vec![(0.0, 0.0)], confidence: vec![1.0], pair: vec![first_pair] };
        let (mut ax, mut ay) = (0.0f32, 0.0f32);
        // Read one frame past the window so the next window's first frame
        // also gets its motion hint; that frame waits in `pending`.
        while self.window > 1 {
            match self.read_one()? {
                Some(f) => {
                    let last = win.frames.last().unwrap();
                    let m = self.pc.as_ref().unwrap().estimate(&f, last);
                    let cut = m.confidence < self.min_confidence || last.psnr(&f) < self.min_psnr;
                    if self.debug {
                        eprintln!("plate: frame {:6} motion ({:+7.2}, {:+7.2}) conf {:.2}{}", self.index - 1, m.dx, m.dy, m.confidence, if cut { "  CUT" } else { "" });
                    }
                    if cut {
                        self.pending = Some((f, None));
                        break;
                    }
                    if win.frames.len() >= self.window {
                        self.pending = Some((f, Some((m.dx, m.dy))));
                        break;
                    }
                    ax += m.dx;
                    ay += m.dy;
                    win.frames.push(f);
                    win.disp.push((ax, ay));
                    win.confidence.push(m.confidence);
                    win.pair.push(Some((m.dx, m.dy)));
                }
                None => break,
            }
        }
        if self.debug && self.window > 1 {
            let (mx, my) = win.disp.iter().fold((0.0f32, 0.0f32), |a, d| (a.0.max(d.0.abs()), a.1.max(d.1.abs())));
            eprintln!("plate: window {}..{} ({} frames), max |displacement| ({:.1}, {:.1}) px", win.start, win.start + win.frames.len() as u64, win.frames.len(), mx, my);
        }
        Ok(Some(win))
    }
}

/// Run the whole pipeline with one upscaler. `on_event` is called from the
/// calling thread.
pub fn run(
    source: Box<dyn FrameSource>,
    upscaler: Box<dyn Upscaler>,
    sink: Box<dyn FrameSink>,
    cfg: PipelineConfig,
    on_event: impl FnMut(Event),
) -> anyhow::Result<Summary> {
    run_tiered(source, vec![upscaler], sink, cfg, on_event)
}

/// Run with up to [`MAX_TIERS`] upscalers: tier 0 is the main network and
/// takes redraws and textured blocks; tier 1 (cheap network) and tier 2
/// (bicubic) take smooth and flat blocks per `AnalyzeConfig::tier_std`.
/// All tiers must produce the same scale. A private block cache is created
/// per `cfg.block_cache_bytes`; use [`run_tiered_with_cache`] to share one.
pub fn run_tiered(
    source: Box<dyn FrameSource>,
    tiers: Vec<Box<dyn Upscaler>>,
    sink: Box<dyn FrameSink>,
    cfg: PipelineConfig,
    on_event: impl FnMut(Event),
) -> anyhow::Result<Summary> {
    let cache = (cfg.block_cache_bytes > 0).then(|| Arc::new(Mutex::new(BlockCache::new(cfg.block_cache_bytes))));
    run_tiered_with_cache(source, tiers, sink, cfg, cache, on_event)
}

/// [`run_tiered`] with an explicit (possibly shared) block cache.
pub fn run_tiered_with_cache(
    source: Box<dyn FrameSource>,
    mut tiers: Vec<Box<dyn Upscaler>>,
    mut sink: Box<dyn FrameSink>,
    cfg: PipelineConfig,
    cache: Option<Arc<Mutex<BlockCache>>>,
    mut on_event: impl FnMut(Event),
) -> anyhow::Result<Summary> {
    anyhow::ensure!(!tiers.is_empty() && tiers.len() <= MAX_TIERS, "1 to {MAX_TIERS} upscaler tiers");
    let info = source.info();
    let bs = cfg.analyze.block_size;
    let bleed = cfg.analyze.bleed;
    let scale = tiers[0].scale();
    for (t, u) in tiers.iter().enumerate() {
        anyhow::ensure!(u.scale() == scale, "tier {t} ({}) is x{} but tier 0 is x{scale}", u.name(), u.scale());
    }
    let n_tiers = tiers.len();
    anyhow::ensure!(bs >= 4, "block size {bs} is too small");
    anyhow::ensure!(info.width >= 1 && info.height >= 1, "empty source");
    let padded = (info.width.div_ceil(bs) * bs, info.height.div_ceil(bs) * bs);
    let matcher = cfg.matcher.build();
    let reference = JpegReference { quality: cfg.quality };
    on_event(Event::Started {
        info,
        padded,
        upscaler: tiers.iter().map(|u| u.name()).collect::<Vec<_>>().join(" | "),
        matcher: matcher.name(),
        gate: reference.describe(),
    });

    let started = Instant::now();
    let (tx, rx) = mpsc::sync_channel::<anyhow::Result<Work>>(cfg.queue_depth.max(1));
    let acfg = cfg.analyze;
    let noise = cfg.noise;
    let (recent_refs, keyframe_refs, batch) = (cfg.recent_refs, cfg.keyframe_refs, cfg.batch.max(1));
    let cache_a = cache.clone();
    let cache_b = cache;
    let (plate_window, plate_conf) = (cfg.plate_window, cfg.plate_min_confidence);
    let (gated, bucket) = (cfg.analyze.cache_quant, cfg.analyze.cache_bucket);
    // The reader needs pairwise motion for hints even without plates.
    let reader_window = if plate_window > 1 { plate_window } else if cfg.mv_seed { 2 } else { 1 };
    let mv_seed = cfg.mv_seed;

    // Stage A.
    let analyzer = std::thread::Builder::new().name("d2x-analyze".into()).spawn(move || {
        let reference: &dyn ReferenceMaker = &reference;
        let mut prev_working: Option<Frame> = None;
        let mut ring: Ring<Frame> = Ring::new(recent_refs, keyframe_refs);
        let mut state = AnalyzeState { streak: Vec::new(), cache: cache_a, anchor: None, since_anchor: 0, force_cut: false };
        let mut reader = WindowReader::new(source, reader_window, acfg.min_psnr, plate_conf, bs, noise);
        'windows: loop {
            let t_win = Instant::now();
            let win = match reader.next_window() {
                Ok(Some(w)) => w,
                Ok(None) => break,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    break;
                }
            };
            let plate = if plate_window > 1 && win.frames.len() > 1 { Some(win.plate(plate_conf)) } else { None };
            let (start, n_win) = (win.start, win.frames.len() as u64);
            let hints: Vec<Option<(i64, i64)>> = (0..win.frames.len()).map(|i| if mv_seed { win.hint(i) } else { None }).collect();
            for (i, frame) in win.frames.into_iter().enumerate() {
                let t0 = Instant::now();
                let index = start + i as u64;
                let hint = hints[i];
                let plate_msg = if i == 0 { plate.as_ref().map(|p| (p.lo.clone(), n_win)) } else { None };
                let view = plate.as_ref().map(|p| p.view(i));
                let mut work = match prev_working.take() {
                    None => {
                        ring.keyframes.push_front(frame.clone());
                        ring.keyframes.truncate(ring.keyframe_cap);
                        prev_working = Some(frame.clone());
                        Work { index, plan: None, residual: Residual::Whole(frame), cached: CachedBlocks::new(), analyze_time: t0.elapsed(), plate: plate_msg }
                    }
                    Some(mut prev) => {
                        let older = ring.refs();
                        state.anchor = ring.anchor();
                        let r = analyze_tiered(&mut prev, &frame, &older, view, hint, reference, matcher.as_ref(), &acfg, n_tiers, &mut state);
                        match r {
                            Ok((plan, working_next, cached)) => {
                                let residual = make_residual(&frame, &plan, bleed);
                                ring.advance(prev, &working_next, plan.stats.is_anchor(&plan.grid));
                                prev_working = Some(working_next);
                                Work { index, plan: Some(plan), residual, cached, analyze_time: t0.elapsed(), plate: plate_msg }
                            }
                            Err(e) => {
                                let _ = tx.send(Err(e));
                                break 'windows;
                            }
                        }
                    }
                };
                if i == 0 {
                    // The window's read and plate build are this frame's cost.
                    work.analyze_time = t_win.elapsed();
                }
                if tx.send(Ok(work)).is_err() {
                    break 'windows; // stage B went away
                }
            }
        }
    })?;

    // Stage B, on this thread. Frames are taken from the channel in groups
    // so their residuals share one upscale call; merges stay sequential.
    let crop_w = info.width * scale;
    let crop_h = info.height * scale;
    let mut prev_up: Option<Frame> = None;
    let mut ring: Ring<Frame> = Ring::new(recent_refs, keyframe_refs);
    let mut frames: u64 = 0;
    let mut fraction_sum = 0.0;
    let mut heavy_sum = 0.0;
    let mut plate_sum = 0.0;
    let mut plate_up: Option<Frame> = None;
    let frame_area = (padded.0 as f64) * (padded.1 as f64);
    let mut result: anyhow::Result<()> = Ok(());

    'outer: loop {
        let mut group: Vec<Work> = Vec::with_capacity(batch);
        match rx.recv() {
            Ok(Ok(w)) => group.push(w),
            Ok(Err(e)) => {
                result = Err(e);
                break;
            }
            Err(_) => break,
        }
        while group.len() < batch {
            match rx.try_recv() {
                Ok(Ok(w)) => group.push(w),
                Ok(Err(e)) => {
                    result = Err(e);
                    break 'outer;
                }
                Err(_) => break,
            }
        }

        let t0 = Instant::now();
        // One packed call per tier over the whole group.
        let mut ups: [std::collections::VecDeque<Frame>; MAX_TIERS] = Default::default();
        for (t, up) in tiers.iter_mut().enumerate() {
            let inputs: Vec<&Frame> = group.iter().filter_map(|w| w.residual.tier(t)).collect();
            if inputs.is_empty() {
                continue;
            }
            match up.upscale_many(&inputs) {
                Ok(v) => ups[t] = v.into(),
                Err(e) => {
                    result = Err(e.context(format!("tier {t} upscaling frames {}..", group[0].index)));
                    break 'outer;
                }
            }
        }
        let upscale_time = t0.elapsed() / (group.len() as u32);

        for mut work in group {
            if let Some((p, n_win)) = work.plate.take() {
                let t = Instant::now();
                match tiers[0].upscale(&p) {
                    Ok(up) => {
                        let area = (p.width as f64) * (p.height as f64) / frame_area;
                        plate_sum += area;
                        on_event(Event::PlateUpscaled { index: work.index, size: (p.width, p.height), frames: n_win, area, time: t.elapsed() });
                        plate_up = Some(up);
                    }
                    Err(e) => {
                        result = Err(e.context(format!("upscaling the plate for frames {}..", work.index)));
                        break 'outer;
                    }
                }
            }
            let mut tier_up: [Option<Frame>; MAX_TIERS] = Default::default();
            for t in 0..MAX_TIERS {
                if work.residual.tier(t).is_some() {
                    tier_up[t] = ups[t].pop_front();
                }
            }
            let residual_up = tier_up[0].take();
            let t1 = Instant::now();
            let (out, stats) = match (&work.plan, prev_up.as_ref()) {
                (None, _) => {
                    let f = residual_up.expect("first frame is always upscaled whole");
                    ring.keyframes.push_front(f.clone());
                    ring.keyframes.truncate(ring.keyframe_cap);
                    (f, FrameStats { redraw: true, psnr: f64::NAN, ..Default::default() })
                }
                (Some(plan), Some(prev)) => {
                    // Feed the block cache with what the networks just produced.
                    if let (Some(c), false) = (&cache_b, plan.redraw) {
                        let cell = (bs + 2 * bleed) * scale;
                        let side = bs * scale;
                        let mut c = c.lock().unwrap();
                        for rv in plan.residuals.iter().filter(|rv| rv.tier < 2) {
                            let img = if rv.tier == 0 { residual_up.as_ref() } else { tier_up[rv.tier as usize].as_ref() };
                            if let Some(img) = img {
                                let blk = img.crop(rv.cell_x * cell + bleed * scale, rv.cell_y * cell + bleed * scale, side, side);
                                c.insert(rv.key, Arc::new(blk.data));
                            }
                        }
                    }
                    let older = ring.refs();
                    let refs: [Option<&Frame>; MAX_TIERS] = [residual_up.as_ref(), tier_up[1].as_ref(), tier_up[2].as_ref()];
                    match merge_tiered(prev, &older, plate_up.as_ref(), &refs, &work.cached, plan, scale, bleed) {
                        Ok(f) => (f, plan.stats),
                        Err(e) => {
                            result = Err(e.context(format!("merging frame {}", work.index)));
                            break 'outer;
                        }
                    }
                }
                (Some(_), None) => {
                    result = Err(anyhow::anyhow!("frame {} has a plan but no previous output", work.index));
                    break 'outer;
                }
            };
            let merge_time = t1.elapsed();

            // Harvest the subtiles of a big upscale. `out` is the finished
            // frame, so the block at (x·s, y·s) is exactly what the network
            // drew: for a whole redraw the upscaled frame, for a region the
            // rectangle's interior (full weight — only its bleed ring is
            // feathered, and no harvested cell touches the ring).
            if let (Some(c), Some(plan)) = (&cache_b, &work.plan) {
                if !plan.harvest.is_empty() {
                    let side = bs * scale;
                    let mut c = c.lock().unwrap();
                    for hc in &plan.harvest {
                        if !out.block_in_bounds(hc.x * scale, hc.y * scale, side) {
                            continue;
                        }
                        let up = Arc::new(out.crop(hc.x * scale, hc.y * scale, side, side).data);
                        c.insert(hc.key, up.clone());
                        if gated > 0 {
                            c.insert_gated(hc.coarse, hc.key, Entry { up, lo: hc.lo.clone() }, bucket);
                        }
                    }
                }
            }

            let visible = if (out.width, out.height) == (crop_w, crop_h) { out.clone() } else { out.crop(0, 0, crop_w, crop_h) };
            if let Err(e) = sink.write(&visible) {
                result = Err(e.context(format!("writing frame {}", work.index)));
                break 'outer;
            }
            let (fraction, heavy) = match &work.plan {
                None => (1.0, 1.0),
                Some(p) => (p.stats.upscaled_fraction(&p.grid, bleed), p.stats.heavy_fraction(&p.grid, bleed)),
            };
            fraction_sum += fraction;
            heavy_sum += heavy;
            frames += 1;
            if let Some(p) = prev_up.take() {
                let anchor = work.plan.as_ref().map_or(true, |pl| pl.stats.is_anchor(&pl.grid));
                ring.advance(p, &out, anchor);
            }
            prev_up = Some(out);
            on_event(Event::FrameDone { index: work.index, stats, analyze_time: work.analyze_time, upscale_time, merge_time });
        }
    }
    drop(rx);
    let _ = analyzer.join();
    result?;
    sink.finish()?;
    let elapsed = started.elapsed();
    let upscaled_pixel_fraction = if frames > 0 { fraction_sum / frames as f64 } else { 0.0 };
    let heavy_pixel_fraction = if frames > 0 { heavy_sum / frames as f64 } else { 0.0 };
    let plate_pixel_fraction = if frames > 0 { plate_sum / frames as f64 } else { 0.0 };
    let cache_stats = cache_b.map_or((0, 0, 0, 0), |c| {
        let c = c.lock().unwrap();
        (c.hits, c.misses, c.inserts, c.bytes())
    });
    on_event(Event::Finished { frames, elapsed, upscaled_pixel_fraction, heavy_pixel_fraction, plate_pixel_fraction });
    Ok(Summary { frames, elapsed, upscaled_pixel_fraction, heavy_pixel_fraction, plate_pixel_fraction, cache: cache_stats })
}

/// Analysis only: run stage A over the source and report per-frame stats,
/// never touching an upscaler. This is the `analyze` subcommand and the
/// cheapest way to see what a block size / quality setting buys on a clip.
pub fn analyze_only(
    source: Box<dyn FrameSource>,
    cfg: PipelineConfig,
    mut on_frame: impl FnMut(u64, &FrameStats, f64, Duration),
) -> anyhow::Result<Summary> {
    let bs = cfg.analyze.block_size;
    let bleed = cfg.analyze.bleed;
    let matcher = cfg.matcher.build();
    let reference = JpegReference { quality: cfg.quality };
    let started = Instant::now();
    let mut prev: Option<Frame> = None;
    let mut ring: Ring<Frame> = Ring::new(cfg.recent_refs, cfg.keyframe_refs);
    let mut state = AnalyzeState::default();
    // analyze_only never produces upscaled pixels, so the cache runs DRY:
    // keys and byte costs only. That is enough to measure the hit rate and
    // the eviction behaviour of a real run, harvested subtiles included.
    // Block bytes are charged at scale 2 (the usual factor); the shape of
    // the answer does not depend on it, the budget does.
    let dry = (cfg.block_cache_bytes > 0).then(|| Arc::new(Mutex::new(BlockCache::new_dry(cfg.block_cache_bytes))));
    state.cache = dry.clone();
    let up_block_bytes = (bs * 2) as u64 * (bs * 2) as u64 * 3;
    let mut index = 0u64;
    let mut fraction_sum = 0.0;
    let mut heavy_sum = 0.0;
    let mut plate_sum = 0.0;
    let mut frame_area = 0.0;
    let reader_window = if cfg.plate_window > 1 { cfg.plate_window } else if cfg.mv_seed { 2 } else { 1 };
    let mut reader = WindowReader::new(source, reader_window, cfg.analyze.min_psnr, cfg.plate_min_confidence, bs, cfg.noise);
    while let Some(win) = reader.next_window()? {
        let t_win = Instant::now();
        if frame_area == 0.0 {
            frame_area = (win.frames[0].width as f64) * (win.frames[0].height as f64);
        }
        let plate = if cfg.plate_window > 1 && win.frames.len() > 1 {
            let p = win.plate(cfg.plate_min_confidence);
            plate_sum += (p.lo.width as f64) * (p.lo.height as f64) / frame_area;
            Some(p)
        } else {
            None
        };
        let start = win.start;
        let hints: Vec<Option<(i64, i64)>> = (0..win.frames.len()).map(|i| if cfg.mv_seed { win.hint(i) } else { None }).collect();
        for (i, frame) in win.frames.into_iter().enumerate() {
            let t0 = if i == 0 { t_win } else { Instant::now() };
            index = start + i as u64;
            let hint = hints[i];
            match prev.take() {
                None => {
                    fraction_sum += 1.0;
                    heavy_sum += 1.0;
                    on_frame(index, &FrameStats { redraw: true, psnr: f64::NAN, ..Default::default() }, 1.0, t0.elapsed());
                    ring.keyframes.push_front(frame.clone());
                    prev = Some(frame);
                }
                Some(mut p) => {
                    let older = ring.refs();
                    state.anchor = ring.anchor();
                    let view = plate.as_ref().map(|pl| pl.view(i));
                    let (plan, next_w, _) = analyze_tiered(&mut p, &frame, &older, view, hint, &reference, matcher.as_ref(), &cfg.analyze, cfg.tiers.max(1), &mut state)?;
                    let fr = plan.stats.upscaled_fraction(&plan.grid, bleed);
                    fraction_sum += fr;
                    heavy_sum += plan.stats.heavy_fraction(&plan.grid, bleed);
                    on_frame(index, &plan.stats, fr, t0.elapsed());
                    if let Some(c) = &dry {
                        let mut c = c.lock().unwrap();
                        for hc in &plan.harvest {
                            c.insert_sized(hc.key, up_block_bytes);
                            if cfg.analyze.cache_quant > 0 {
                                c.insert_gated(hc.coarse, hc.key, Entry { up: Arc::new(Vec::new()), lo: hc.lo.clone() }, cfg.analyze.cache_bucket);
                            }
                        }
                        // What stage B would insert from the packed residual.
                        for rv in plan.residuals.iter().filter(|rv| rv.tier < 2) {
                            c.insert_sized(rv.key, up_block_bytes);
                        }
                    }
                    ring.advance(p, &next_w, plan.stats.is_anchor(&plan.grid));
                    prev = Some(next_w);
                }
            }
        }
        index += 1;
    }
    let n = index as f64;
    let cache = dry.map_or((0, 0, 0, 0), |c| {
        let c = c.lock().unwrap();
        (c.hits, c.misses, c.inserts, c.bytes())
    });
    Ok(Summary {
        frames: index,
        elapsed: started.elapsed(),
        upscaled_pixel_fraction: if index > 0 { fraction_sum / n } else { 0.0 },
        heavy_pixel_fraction: if index > 0 { heavy_sum / n } else { 0.0 },
        plate_pixel_fraction: if index > 0 { plate_sum / n } else { 0.0 },
        cache,
    })
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

    /// The two-stage pipeline with plate windows: a 20-frame pan over a
    /// world, a cut, then 6 more frames. Windows must break at the cut, each
    /// window's plate must be upscaled exactly once, the pan's frames must
    /// be served from the plate (not re-upscaled), and with nearest
    /// upscaling every output must equal the upscale of its input frame.
    /// This is the same code path the GPU backend runs: it only implements
    /// `Upscaler`, and the plate goes through `upscale` like a redraw frame.
    #[test]
    fn plate_windows_are_exact_and_break_at_cuts() {
        let world = smooth(320, 96, 11);
        let (fw, fh) = (128u32, 96u32);
        let mut frames: Vec<Frame> = (0..20u32).map(|i| world.crop(6 * i, 0, fw, fh)).collect();
        let other = smooth(200, 96, 40);
        let mut cut = Frame::new(fw, fh);
        for (i, b) in cut.data.iter_mut().enumerate() {
            *b = (i * 37 % 253) as u8;
        }
        frames.push(cut); // scene cut: unrelated content
        frames.extend((0..5u32).map(|i| other.crop(4 * i, 0, fw, fh)));
        let inputs = frames.clone();
        let out = Arc::new(Mutex::new(Vec::new()));
        let cfg = PipelineConfig {
            analyze: AnalyzeConfig { block_size: 16, bleed: 1, ..Default::default() },
            noise: 0,
            plate_window: 8,
            recent_refs: 0,
            keyframe_refs: 0,
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
        assert_eq!(summary.frames, 26);
        let out = out.lock().unwrap();
        let mut up = NearestUpscaler { scale: 2 };
        for (i, f) in inputs.iter().enumerate() {
            assert_eq!(out[i], up.upscale(f).unwrap(), "frame {i}");
        }
        // Windows: [0..8) [8..16) [16..20), then the cut frame alone (it is
        // unrelated to both neighbours, so it gets no plate), then [21..26).
        let plates: Vec<(u64, u64)> = events
            .iter()
            .filter_map(|e| if let Event::PlateUpscaled { index, frames, .. } = e { Some((*index, *frames)) } else { None })
            .collect();
        assert_eq!(plates, vec![(0, 8), (8, 8), (16, 4), (21, 5)]);
        assert!(summary.plate_pixel_fraction > 0.0);
        // Inside a pan window every frame after the first is fully plated.
        let plated: Vec<(u64, u32, u32)> = events
            .iter()
            .filter_map(|e| if let Event::FrameDone { index, stats, .. } = e { Some((*index, stats.plated, stats.missing)) } else { None })
            .collect();
        for &(i, plated, missing) in &plated {
            if (1..8).contains(&i) || (9..16).contains(&i) || (17..20).contains(&i) {
                assert_eq!((plated, missing), (48, 0), "frame {i}");
            }
        }
        // The cut frame is a redraw; the frames after it are plated too.
        assert!(plated.iter().any(|&(i, p, _)| i == 20 && p == 0));
        assert!(plated.iter().all(|&(i, p, m)| !(22..26).contains(&i) || (p == 48 && m == 0)), "{plated:?}");
    }

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
        // f4 = f2 again: its scribbled block is recalled from the ring, not re-upscaled.
        let f4 = f2.clone();
        let frames = vec![f0.clone(), f1.clone(), f2.clone(), f3.clone(), f4.clone()];
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
        assert_eq!(summary.frames, 5);
        let out = out.lock().unwrap();
        assert_eq!(out.len(), 5);
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
        assert_eq!(out[4], up.upscale(&f4).unwrap());
        let done = events.iter().filter(|e| matches!(e, Event::FrameDone { .. })).count();
        assert_eq!(done, 5);
        // Frame 4 after the scene cut: everything comes back from the ring.
        let last = events.iter().rev().find_map(|e| if let Event::FrameDone { index: 4, stats, .. } = e { Some(*stats) } else { None }).unwrap();
        assert_eq!(last.missing, 0, "{last:?}");
        assert!(last.recalled > 0, "{last:?}");
        assert!(summary.upscaled_pixel_fraction < 1.0);
    }

    /// Three nearest tiers: still an exact identity end to end.
    #[test]
    fn tiered_pipeline_is_exact() {
        let f0 = smooth(64, 64, 3);
        let mut f1 = f0.clone();
        for y in 0..16 {
            for x in 0..16 {
                f1.set_px(x, y, [200, 200, 200]);
                f1.set_px(16 + x, y, [(100 + x + y) as u8; 3]);
                let c = if (x + y) % 2 == 0 { 30 } else { 220 };
                f1.set_px(32 + x, y, [c, c, c]);
            }
        }
        let out = Arc::new(Mutex::new(Vec::new()));
        // Pinned to the std metric: this fixture's three blocks (flat, gentle
        // ramp, checker) were chosen to separate under luma std-dev. The
        // default metric is high-frequency energy, under which the ramp is as
        // blur-free as the flat block and both route to bicubic — correct
        // behaviour, different fixture.
        let cfg = PipelineConfig { analyze: AnalyzeConfig { block_size: 16, bleed: 1, tier_std: [2.0, 12.0], tier_metric: crate::predict::TierMetric::Std, ..Default::default() }, noise: 0, ..Default::default() };
        let tiers: Vec<Box<dyn Upscaler>> = (0..3).map(|_| Box::new(NearestUpscaler { scale: 2 }) as Box<dyn Upscaler>).collect();
        let mut stats = Vec::new();
        run_tiered(Box::new(VecSource::new(vec![f0.clone(), f1.clone()], (30, 1))), tiers, Box::new(VecSink { frames: out.clone() }), cfg, |e| {
            if let Event::FrameDone { stats: s, .. } = e {
                stats.push(s);
            }
        })
        .unwrap();
        let out = out.lock().unwrap();
        assert_eq!(out[1], NearestUpscaler { scale: 2 }.upscale(&f1).unwrap());
        assert_eq!(stats[1].residual_tiers, [(1, 1), (1, 1), (1, 1)], "{:?}", stats[1]);
    }

    /// A block whose exact cell content reappears at a new position is
    /// served from the block cache, and the output is still exact. Hits lag
    /// the pipeline depth: stage A analyses ahead of stage B's inserts, so
    /// the repeat sits several frames after the original.
    #[test]
    fn block_cache_serves_repeated_content() {
        let mut uniform = Frame::new(64, 64);
        uniform.data.iter_mut().for_each(|b| *b = 90);
        let stamp = |f: &mut Frame, x0: u32, y0: u32| {
            for y in 0..16 {
                for x in 0..16 {
                    f.set_px(x0 + x, y0 + y, [((x * 13 + y * 7) % 200) as u8, 40, ((x ^ y) * 9) as u8]);
                }
            }
        };
        let mut f1 = uniform.clone();
        stamp(&mut f1, 16, 16);
        let mut f2 = uniform.clone();
        stamp(&mut f2, 32, 32);
        let mut f3 = uniform.clone();
        stamp(&mut f3, 0, 32);
        let frames = vec![uniform.clone(), f1.clone(), uniform.clone(), uniform.clone(), uniform.clone(), uniform.clone(), f2.clone(), f3.clone()];
        let out = Arc::new(Mutex::new(Vec::new()));
        let cfg = PipelineConfig { analyze: AnalyzeConfig { block_size: 16, bleed: 1, ..Default::default() }, noise: 0, block_cache_bytes: 1 << 20, recent_refs: 0, keyframe_refs: 0, queue_depth: 1, batch: 1, ..Default::default() };
        let mut stats = Vec::new();
        run_tiered(Box::new(VecSource::new(frames, (30, 1))), vec![Box::new(NearestUpscaler { scale: 2 })], Box::new(VecSink { frames: out.clone() }), cfg, |e| {
            if let Event::FrameDone { stats: s, .. } = e {
                stats.push(s);
            }
        })
        .unwrap();
        let out = out.lock().unwrap();
        let mut up = NearestUpscaler { scale: 2 };
        assert_eq!(out[6], up.upscale(&f2).unwrap());
        assert_eq!(out[7], up.upscale(&f3).unwrap());
        assert_eq!(stats[1].cached, 0);
        assert_eq!(stats[6].cached, 1, "{:?}", stats[6]);
        assert_eq!(stats[7].cached, 1, "{:?}", stats[7]);
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
