//! One place that turns settings into a running pipeline, shared by the CLI
//! subcommands and the egui shell so they cannot drift.

use crate::ffmpeg::{self, EncodeOptions, FfmpegSink, FfmpegSource, PngDirSink, PngDirSource, Segment};
use anyhow::{anyhow, Context};
use d2x_core::net::{CpuNet, Network};
use d2x_core::pipeline::{Event as PEvent, FrameSink, FrameSource, MatcherKind, PipelineConfig, Summary};

/// What a job reports. `Stage` covers the work before the first frame,
/// which on a long input used to be a silent minute.
#[derive(Debug, Clone)]
pub enum Event {
    Stage(String),
    Pipeline(PEvent),
    /// Segment-parallel runs: which worker the pipeline event belongs to.
    Segment(usize, PEvent),
}
use d2x_core::predict::AnalyzeConfig;
use d2x_core::blockcache::BlockCache;
use d2x_core::budget::{plan_ring, MemoryBudget, RingPlan};
use d2x_core::upscale::{BicubicUpscaler, NearestUpscaler, Resampled, Upscaler};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Wgpu,
    Cpu,
    Bicubic,
    Nearest,
}

impl std::str::FromStr for Backend {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "wgpu" | "gpu" => Backend::Wgpu,
            "cpu" => Backend::Cpu,
            "bicubic" => Backend::Bicubic,
            "nearest" => Backend::Nearest,
            _ => return Err(anyhow!("backend must be wgpu|cpu|bicubic|nearest")),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub input: PathBuf,
    pub output: PathBuf,
    pub model: Option<PathBuf>,
    pub backend: Backend,
    pub scale: u32,
    pub tile: u32,
    pub require_hw: bool,
    pub pipeline: PipelineConfig,
    pub encode: EncodeOptions,
    /// Stop after this many frames (0 = all). For trying settings on a clip.
    pub max_frames: u64,
    /// Segment workers (1 = a single pipeline). Segments are cut on source
    /// keyframes, share one upscaler, encode to part files, and are
    /// concatenated by stream copy at the end.
    pub jobs: usize,
    /// Only this range of the input, in seconds (a clip's in/out points).
    /// `start` is snapped to the previous keyframe by the decoder; `end` is exact.
    pub start: f64,
    pub end: Option<f64>,
    /// Set from another thread to stop after the current frame; the encoder
    /// then finalises a playable partial file.
    pub cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Difficulty cascade: route smooth missing blocks (luma std below
    /// `pipeline.analyze.tier_std[1]`) to this cheaper model instead of the
    /// main one. `Some(None)` = the vendored compact model.
    pub cascade_model: Option<Option<PathBuf>>,
    /// Route flat missing blocks (luma std below `tier_std[0]`) to bicubic.
    pub flat_bypass: bool,
    /// Memory budgets; `None` = detect (half of MemAvailable for RAM, a
    /// quarter of that for the GPU). The RAM budget sizes the reference
    /// rings and the block cache unless those are set explicitly.
    pub ram_budget: Option<u64>,
    pub vram_budget: Option<u64>,
    /// Explicit ring depths override the budget-derived ones.
    pub refs_override: Option<(usize, usize)>,
    /// Explicit block-cache bytes override the budget-derived figure.
    pub block_cache_override: Option<u64>,
    /// Activation sets in flight per GPU upscaler.
    pub inflight: usize,
}

impl Default for Job {
    fn default() -> Self {
        Job {
            input: PathBuf::new(),
            output: PathBuf::new(),
            model: None,
            backend: Backend::Wgpu,
            scale: 2,
            tile: 0,
            require_hw: false,
            // Noise 0: upstream's 8 raised the gate floor for waifu2x; Real-ESRGAN
            // is degradation-trained and would just denoise it away.
            pipeline: PipelineConfig { analyze: AnalyzeConfig::default(), quality: 97, noise: 0, matcher: MatcherKind::Diamond, ..PipelineConfig::default() },
            encode: EncodeOptions::default(),
            max_frames: 0,
            jobs: 1,
            start: 0.0,
            end: None,
            cancel: None,
            cascade_model: None,
            flat_bypass: false,
            ram_budget: None,
            vram_budget: None,
            refs_override: None,
            block_cache_override: None,
            inflight: 2,
        }
    }
}

/// The vendored fast model, relative to the app's `models/` directory, tried
/// when no `--model` is given.
// ── model resolution ────────────────────────────────────────────────────────
//
// PORTED BACK from maintenance/archive/dandere2x-inline-job.rs. What this
// crate had was `default_model_paths` walking the executable's ancestors and
// the compile-time manifest dir for ONE hard-coded filename — so `--model
// realesrgan-x4` could not be written by name, `~/models` was never looked at,
// and `env!("CARGO_MANIFEST_DIR")` is a path baked at build time that resolves
// only on the machine that built the binary.
//
// That last one is the same defect as unendlich's shader paths
// (games/unendlich/crates/unendlich-engine/src/assets.rs): a pinned or copied
// binary fails on a file that ships with the repo. It is kept here as the LAST
// candidate, where it costs one probe and can still help a `cargo run`, rather
// than as the answer.
//
// Order matters and is stated in the original: an operator's own directory
// beats a checkout's vendored copy, always. A tree that shadows the model
// someone deliberately installed is a tree that silently upscales with the
// wrong network.

/// Where a model may live, in resolution order.
///
/// The operator's own model directory comes FIRST, before anything vendored.
/// This monorepo already has a `~/models` tree (GGUFs for logscope, whisper
/// weights, and so on) and the big super-resolution checkpoints belong beside
/// them rather than in git: `RealESRGAN_x2` is 67 MB, and waifu2x's
/// `upconv_7` ships as a 10.6 MB JSON — four times the size of the
/// safetensors model this repo does vendor, for a format that compresses
/// worse and diffs worse.
///
/// So: only the 2.5 MB `realesr-animevideov3` is carried in-tree, as the
/// working default. Everything else is found, not shipped.
///
/// `DANDERE2X_MODELS` overrides, `~/models` and `~/models/upscale` are the
/// conventional homes, then XDG data, then the vendored fallback.
pub fn model_search_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(d) = std::env::var("DANDERE2X_MODELS") {
        v.push(PathBuf::from(d));
    }
    if let Ok(home) = std::env::var("HOME") {
        let h = PathBuf::from(home);
        v.push(h.join("models/upscale"));
        v.push(h.join("models"));
    }
    if let Ok(x) = std::env::var("XDG_DATA_HOME") {
        v.push(PathBuf::from(x).join("dandere2x/models"));
    } else if let Ok(home) = std::env::var("HOME") {
        v.push(PathBuf::from(home).join(".local/share/dandere2x/models"));
    }
    // Vendored, last: a checkout's copy should never shadow one the operator
    // deliberately put in their own directory.
    if let Ok(exe) = std::env::current_exe() {
        for anc in exe.ancestors().take(5) {
            v.push(anc.join("models"));
        }
    }
    v.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models"));
    v
}

/// Resolve a model by BARE NAME across the search dirs, so `--model
/// RealESRGAN_x2` works without the operator typing a path. A name containing
/// a separator is taken as a path and used as given.
pub fn resolve_model(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.is_absolute() || name.contains('/') {
        return p.exists().then(|| p.to_path_buf());
    }
    for d in model_search_dirs() {
        for ext in ["", ".safetensors", ".json"] {
            let c = d.join(format!("{name}{ext}"));
            if c.is_file() { return Some(c); }
        }
    }
    None
}

/// The default model, tried when no `--model` is given.
pub fn default_model_paths() -> Vec<PathBuf> {
    model_search_dirs().into_iter()
        .map(|d| d.join("realesr-animevideov3.safetensors"))
        .collect()
}


#[cfg(test)]
mod model_search_tests {
    use super::*;

    #[test]
    fn an_operators_directory_beats_a_vendored_copy() {
        // The ORDER is the property. A checkout's `models/` shadowing one the
        // operator deliberately installed means silently upscaling with the
        // wrong network, and the output looks fine — just not as good as they
        // asked for.
        //
        // SAFETY: single-threaded test; both variables are restored.
        let prev = (std::env::var_os("DANDERE2X_MODELS"), std::env::var_os("HOME"));
        unsafe {
            std::env::set_var("DANDERE2X_MODELS", "/opt/chosen");
            std::env::set_var("HOME", "/home/u");
        }
        let dirs = model_search_dirs();
        let pos = |s: &str| dirs.iter().position(|d| d.to_string_lossy() == s);
        assert_eq!(pos("/opt/chosen"), Some(0), "the explicit override is not first");
        let home_models = dirs.iter().position(|d| d.ends_with("models") && d.starts_with("/home/u"));
        let vendored = dirs.iter().rposition(|d| d.to_string_lossy().contains(".."));
        if let (Some(h), Some(v)) = (home_models, vendored) {
            assert!(h < v, "a vendored copy outranks the operator's own directory");
        }
        // And the compile-time path is LAST, for the same reason it is last in
        // unendlich: it is right only on the machine that built the binary.
        assert!(
            dirs.last().is_some_and(|d| d.to_string_lossy().contains("..")),
            "the build-time path is not the fallback: {:?}", dirs.last()
        );
        unsafe {
            match prev.0 { Some(v) => std::env::set_var("DANDERE2X_MODELS", v), None => std::env::remove_var("DANDERE2X_MODELS") }
            match prev.1 { Some(v) => std::env::set_var("HOME", v), None => std::env::remove_var("HOME") }
        }
    }

    #[test]
    fn a_path_is_used_as_given_and_a_bare_name_is_searched() {
        // The distinction the old `default_model_paths` could not make: one
        // hard-coded filename meant `--model realesrgan-x4` was not expressible
        // at all.
        assert_eq!(resolve_model("/definitely/not/here.safetensors"), None,
                   "a missing absolute path was invented");
        assert_eq!(resolve_model("also/not/here"), None);
        // A bare name that exists nowhere resolves to nothing rather than to a
        // plausible-looking path that fails later at load.
        assert_eq!(resolve_model("no-such-model-anywhere"), None);
    }
}

pub fn load_model(explicit: Option<&Path>) -> anyhow::Result<(Arc<Network>, PathBuf)> {
    let path = match explicit {
        Some(p) => p.to_path_buf(),
        None => default_model_paths().into_iter().find(|p| p.exists()).ok_or_else(|| anyhow!("no --model given and the vendored realesr-animevideov3.safetensors was not found"))?,
    };
    let net = Network::load(&path).with_context(|| format!("loading model {}", path.display()))?;
    Ok((Arc::new(net), path))
}

/// Resolved memory plan for a job: budgets, ring depths, block cache.
#[derive(Debug, Clone, Copy)]
pub struct MemPlan {
    pub budget: MemoryBudget,
    pub ring: RingPlan,
    pub block_cache_bytes: u64,
    pub recent_refs: usize,
    pub keyframe_refs: usize,
}

impl std::fmt::Display for MemPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "budget RAM {} MiB / GPU {} MiB → {} recent + {} shot-start references ({:.1} MiB each), block cache {} MiB",
            self.budget.ram >> 20,
            self.budget.vram >> 20,
            self.recent_refs,
            self.keyframe_refs,
            self.ring.bytes_per_ref as f64 / (1u64 << 20) as f64,
            self.block_cache_bytes >> 20
        )
    }
}

/// Turn the job's budgets into ring depths and cache size for a source of
/// `in_w`×`in_h` at `scale`.
pub fn mem_plan(job: &Job, in_w: u32, in_h: u32, scale: u32) -> MemPlan {
    let mut budget = MemoryBudget::detect();
    if let Some(r) = job.ram_budget {
        budget.ram = r;
    }
    if let Some(v) = job.vram_budget {
        budget.vram = v;
    }
    let ring = plan_ring(&budget, in_w, in_h, scale, job.jobs.max(1), 32, 4096);
    let (recent_refs, keyframe_refs) = job.refs_override.unwrap_or((ring.recent_refs, ring.keyframe_refs));
    let block_cache_bytes = job.block_cache_override.unwrap_or(ring.block_cache_bytes);
    MemPlan { budget, ring, block_cache_bytes, recent_refs, keyframe_refs }
}

/// Apply a memory plan to the pipeline config.
pub fn apply_plan(cfg: &mut PipelineConfig, plan: &MemPlan) {
    cfg.recent_refs = plan.recent_refs;
    cfg.keyframe_refs = plan.keyframe_refs;
    cfg.block_cache_bytes = plan.block_cache_bytes;
}

/// All upscaler tiers for a job: main, then the cascade model, then bicubic.
/// GPU tiers share one device; `vram` is split across them by their cost.
pub fn build_tiers(job: &Job) -> anyhow::Result<Vec<Box<dyn Upscaler>>> {
    #[cfg(feature = "gpu")]
    if job.backend == Backend::Wgpu {
        let budget = job.vram_budget.unwrap_or_else(|| MemoryBudget::detect().vram);
        let ctx = d2x_wgpu::GpuContext::new(&d2x_wgpu::GpuOptions { require_hw: job.require_hw, ..Default::default() })?;
        let mut nets: Vec<(Arc<Network>, u32)> = Vec::new();
        let (net, _) = load_model(job.model.as_deref())?;
        nets.push((net, job.tile));
        if let Some(cheap) = &job.cascade_model {
            let (n, _) = load_model(cheap.as_deref())?;
            nets.push((n, 0));
        }
        // Split the GPU budget by each net's per-pixel cost so the expensive
        // one gets the big tiles.
        let total_cost: f64 = nets.iter().map(|(n, _)| n.macs_per_input_pixel()).sum();
        let mut tiers: Vec<Box<dyn Upscaler>> = Vec::new();
        for (n, tile) in nets {
            let share = (budget as f64 * n.macs_per_input_pixel() / total_cost.max(1.0)) as u64;
            let up = d2x_wgpu::WgpuUpscaler::with_context(ctx.clone(), n, d2x_wgpu::GpuOptions { tile, require_hw: job.require_hw, low_power: false, inflight: job.inflight.max(1), vram_budget: share })?;
            let native = up.scale();
            let boxed: Box<dyn Upscaler> = Box::new(up);
            tiers.push(if native == job.scale { boxed } else { Box::new(Resampled { inner: boxed, target: job.scale }) });
        }
        if job.flat_bypass {
            if tiers.len() == 1 {
                tiers.push(Box::new(BicubicUpscaler { scale: job.scale }));
                // tier 1 = bicubic too: without a cascade, "smooth" is as good as "flat".
            }
            tiers.push(Box::new(BicubicUpscaler { scale: job.scale }));
        }
        return Ok(tiers);
    }
    let mut tiers = vec![build_upscaler(job)?];
    if let Some(cheap) = &job.cascade_model {
        let mut j = job.clone();
        j.model = cheap.clone();
        j.cascade_model = None;
        tiers.push(build_upscaler(&j)?);
    }
    if job.flat_bypass {
        if tiers.len() == 1 {
            // Tier index 2 is bicubic by definition; fill tier 1 with the
            // main upscaler so routing stays meaningful without a cascade.
            tiers.push(build_upscaler(job)?);
        }
        tiers.push(Box::new(BicubicUpscaler { scale: job.scale }));
    }
    Ok(tiers)
}

pub fn build_upscaler(job: &Job) -> anyhow::Result<Box<dyn Upscaler>> {
    let native: Box<dyn Upscaler> = match job.backend {
        Backend::Nearest => return Ok(Box::new(NearestUpscaler { scale: job.scale })),
        Backend::Bicubic => return Ok(Box::new(BicubicUpscaler { scale: job.scale })),
        Backend::Cpu => {
            let (net, _) = load_model(job.model.as_deref())?;
            let tile = if job.tile == 0 { 128 } else { job.tile };
            Box::new(CpuNet { net, tile })
        }
        Backend::Wgpu => {
            #[cfg(feature = "gpu")]
            {
                let (net, _) = load_model(job.model.as_deref())?;
                Box::new(d2x_wgpu::WgpuUpscaler::new(net, d2x_wgpu::GpuOptions { tile: job.tile, require_hw: job.require_hw, low_power: false, inflight: job.inflight.max(1), vram_budget: job.vram_budget.unwrap_or(0) })?)
            }
            #[cfg(not(feature = "gpu"))]
            {
                anyhow::bail!("built without the `gpu` feature; use --backend cpu")
            }
        }
    };
    if native.scale() == job.scale {
        Ok(native)
    } else {
        Ok(Box::new(Resampled { inner: native, target: job.scale }))
    }
}

/// A source that stops after `n` frames.
struct Limited {
    inner: Box<dyn FrameSource>,
    left: u64,
}

impl FrameSource for Limited {
    fn info(&self) -> d2x_core::pipeline::SourceInfo {
        self.inner.info()
    }
    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        self.inner.next_frame()
    }
}
use d2x_core::frame::Frame;

struct Cancellable {
    inner: Box<dyn FrameSource>,
    flag: Arc<std::sync::atomic::AtomicBool>,
}

impl FrameSource for Cancellable {
    fn info(&self) -> d2x_core::pipeline::SourceInfo {
        self.inner.info()
    }
    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        if self.flag.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(None);
        }
        self.inner.next_frame()
    }
}

pub fn open_source(job: &Job) -> anyhow::Result<(Box<dyn FrameSource>, Option<ffmpeg::Probe>)> {
    let (src, probe): (Box<dyn FrameSource>, Option<ffmpeg::Probe>) = if job.input.is_dir() {
        (Box::new(PngDirSource::open(&job.input, (30, 1))?), None)
    } else {
        let p = ffmpeg::probe(&job.input)?;
        let seg = Segment { index: 0, start: job.start, end: job.end };
        (Box::new(FfmpegSource::open_segment(&job.input, &p, seg)?), Some(p))
    };
    let src: Box<dyn FrameSource> = if job.max_frames > 0 { Box::new(Limited { inner: src, left: job.max_frames }) } else { src };
    let src: Box<dyn FrameSource> = match &job.cancel {
        Some(flag) => Box::new(Cancellable { inner: src, flag: flag.clone() }),
        None => src,
    };
    Ok((src, probe))
}

/// Median keyframe interval of the source, in frames, so the output GOP
/// cadence matches it even where `-force_key_frames` is ignored.
pub fn keyint_for(keyframes: &[f64], fps: (u32, u32)) -> Option<u32> {
    if keyframes.len() < 2 {
        return None;
    }
    let mut gaps: Vec<f64> = keyframes.windows(2).map(|w| w[1] - w[0]).filter(|g| *g > 0.0).collect();
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = gaps[gaps.len() / 2];
    Some(((median * fps.0 as f64 / fps.1 as f64).round() as u32).clamp(1, 600))
}

fn with_keyint(mut enc: EncodeOptions, keyframes: &[f64], fps: (u32, u32)) -> EncodeOptions {
    if enc.codec == "av1" && !enc.svt_params.iter().any(|p| p.starts_with("keyint=")) {
        if let Some(k) = keyint_for(keyframes, fps) {
            enc.svt_params.push(format!("keyint={k}"));
        }
    }
    enc
}

pub fn open_sink(job: &Job, probe: Option<&ffmpeg::Probe>, keyframes: &[f64], width: u32, height: u32) -> anyhow::Result<Box<dyn FrameSink>> {
    if job.output.extension().is_none() || job.output.is_dir() {
        return Ok(Box::new(PngDirSink::open(&job.output)?));
    }
    let p = probe.ok_or_else(|| anyhow!("encoding to a video needs a video input (PNG-dir input can only write a PNG dir)"))?;
    let mut enc = job.encode.clone();
    if enc.keyframes.is_empty() {
        enc.keyframes = keyframes.iter().filter(|&&t| t >= job.start && job.end.map_or(true, |e| t < e)).map(|t| t - job.start).collect();
    }
    let enc = with_keyint(enc, keyframes, p.info.fps);
    // A range that is only audio-muxable if it starts at 0: ffmpeg would
    // otherwise pair frame 0 with the source's first audio sample. Cut the
    // audio to the same range.
    Ok(Box::new(FfmpegSink::open_ranged(&job.output, Some(&job.input), p, width, height, &enc, job.start, job.end)?))
}

/// One upscaler shared by several pipelines through a server thread that
/// batches concurrent requests into one packed call. The GPU is serial
/// anyway; what this buys is that (a) every worker's decode, analysis,
/// merge and encode overlap the others' upscales, and (b) several segments'
/// small residuals share one dispatch instead of each occupying a sliver of
/// the device.
struct BatchRequest {
    frames: Vec<Frame>,
    reply: std::sync::mpsc::Sender<anyhow::Result<Vec<Frame>>>,
}

pub struct BatchServer {
    tx: std::sync::mpsc::Sender<BatchRequest>,
    pub scale: u32,
    name: String,
    canvas: u32,
}

impl BatchServer {
    /// Owns `up` on its own thread. `wait` is how long to linger for more
    /// requests after the first arrives; `max_pixels` caps one packed call.
    pub fn start(mut up: Box<dyn Upscaler>, wait: std::time::Duration, max_pixels: u64) -> Arc<BatchServer> {
        let (tx, rx) = std::sync::mpsc::channel::<BatchRequest>();
        let (scale, name, canvas) = (up.scale(), up.name(), up.canvas());
        std::thread::Builder::new()
            .name("d2x-gpu".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    let mut reqs = vec![first];
                    let mut px: u64 = reqs[0].frames.iter().map(|f| (f.width * f.height) as u64).sum();
                    let deadline = std::time::Instant::now() + wait;
                    while px < max_pixels {
                        let now = std::time::Instant::now();
                        if now >= deadline {
                            break;
                        }
                        match rx.recv_timeout(deadline - now) {
                            Ok(r) => {
                                px += r.frames.iter().map(|f| (f.width * f.height) as u64).sum::<u64>();
                                reqs.push(r);
                            }
                            Err(_) => break,
                        }
                    }
                    let all: Vec<&Frame> = reqs.iter().flat_map(|r| r.frames.iter()).collect();
                    match up.upscale_many(&all) {
                        Ok(mut outs) => {
                            for r in reqs {
                                let n = r.frames.len();
                                let mine: Vec<Frame> = outs.drain(..n).collect();
                                let _ = r.reply.send(Ok(mine));
                            }
                        }
                        Err(e) => {
                            let msg = format!("{e:#}");
                            for r in reqs {
                                let _ = r.reply.send(Err(anyhow!("{msg}")));
                            }
                        }
                    }
                }
            })
            .expect("spawn gpu server");
        Arc::new(BatchServer { tx, scale, name, canvas })
    }

    pub fn client(self: &Arc<Self>) -> SharedUpscaler {
        SharedUpscaler(self.clone())
    }
}

pub struct SharedUpscaler(Arc<BatchServer>);

impl Upscaler for SharedUpscaler {
    fn as_dyn(&mut self) -> &mut dyn Upscaler {
        self
    }
    fn scale(&self) -> u32 {
        self.0.scale
    }
    fn canvas(&self) -> u32 {
        0 // packing happens in the server
    }
    fn upscale(&mut self, frame: &Frame) -> anyhow::Result<Frame> {
        Ok(self.upscale_many(&[frame])?.remove(0))
    }
    fn upscale_many(&mut self, frames: &[&Frame]) -> anyhow::Result<Vec<Frame>> {
        let (reply, rx) = std::sync::mpsc::channel();
        self.0.tx.send(BatchRequest { frames: frames.iter().map(|f| (*f).clone()).collect(), reply }).map_err(|_| anyhow!("gpu server gone"))?;
        rx.recv().map_err(|_| anyhow!("gpu server dropped the request"))?
    }
    fn name(&self) -> String {
        format!("shared {} (batched, canvas {})", self.0.name, self.0.canvas)
    }
}

/// Segment-parallel run: `jobs` workers over keyframe-aligned segments of a
/// video input, one shared upscaler, part files concatenated at the end.
/// Events carry the segment index as `Event::Started`'s upscaler name prefix
/// and are delivered on the calling thread.
/// What a segment worker reports back.
///
/// `Progress` was the only case; `Done` is the addition resume needed. The
/// coordinator learns of success from `join()` too, but only after every
/// worker has finished — too late to record anything, and precisely the window
/// a kill lands in.
enum SegMsg {
    Progress(PEvent),
    Done,
    Failed(String),
}

pub fn run_segmented(job: &Job, mut on_event: impl FnMut(usize, PEvent)) -> anyhow::Result<Summary> {
    anyhow::ensure!(!job.input.is_dir() && job.output.extension().is_some(), "--jobs needs a video input and a video output");
    anyhow::ensure!(job.start == 0.0 && job.end.is_none(), "segment parallelism over a sub-range is not implemented yet; use jobs = 1 for clip bounces");
    let p = ffmpeg::probe(&job.input)?;
    let keyframes = ffmpeg::keyframe_times(&job.input).unwrap_or_default();
    let segs = ffmpeg::segments(&keyframes, p.duration_s, job.jobs);
    let servers: Vec<Arc<BatchServer>> = build_tiers(job)?.into_iter().map(|u| BatchServer::start(u, std::time::Duration::from_millis(15), 4_000_000)).collect();
    let scale = servers[0].scale;
    let plan = mem_plan(job, p.info.width, p.info.height, scale);
    let mut cfg = job.pipeline;
    apply_plan(&mut cfg, &plan);
    eprintln!("memory: {plan} (rings per worker, cache shared)");
    let shared_cache = (cfg.block_cache_bytes > 0).then(|| Arc::new(std::sync::Mutex::new(BlockCache::new(cfg.block_cache_bytes))));
    let (w, h) = (p.info.width * scale, p.info.height * scale);
    let stem = job.output.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
    let ext = job.output.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "mkv".into());
    let parts: Vec<PathBuf> = segs.iter().map(|s| job.output.with_file_name(format!("{stem}.part{:03}.{ext}", s.index))).collect();

    // ── resume ───────────────────────────────────────────────────────────
    //
    // A segment already encoded on a previous run is skipped entirely: no
    // worker, no decode, no upscale. See `crate::checkpoint` for why a part
    // file on disk is not enough on its own to believe that.
    let ck_path = crate::checkpoint::path_for(&job.output);
    let fp = {
        let (size, mtime) = std::fs::metadata(&job.input)
            .map(|m| (m.len(), m.modified().ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64).unwrap_or(0)))
            .unwrap_or((0, 0));
        crate::checkpoint::fingerprint(
            &job.input, size, mtime,
            &job.model.as_ref().map(|m| m.display().to_string()).unwrap_or_else(|| "default".into()),
            job.scale,
            &format!("{:?}", job.backend),
        )
    };
    let mut ck = std::fs::read_to_string(&ck_path)
        .map(|t| crate::checkpoint::Checkpoint::parse(&t))
        .unwrap_or_default();
    if !ck.matches(&fp, segs.len()) {
        // A checkpoint for another job, another split, or no checkpoint at
        // all. Starting fresh is the safe answer; resuming into a different
        // job produces a full-length output that is half wrong.
        if !ck.fingerprint.is_empty() {
            eprintln!("resume: checkpoint does not match this job — starting over");
        }
        ck = crate::checkpoint::Checkpoint { fingerprint: fp.clone(), segments: segs.len(), ..Default::default() };
    }
    let done = ck.resumable(&parts, &|p: &Path| p.is_file());
    // Anything the record claimed but cannot back with a file is dropped, so
    // the checkpoint on disk never grows stale claims.
    ck.done = done.clone();
    if !done.is_empty() {
        eprintln!("resume: {} of {} segments already done — skipping them", done.len(), segs.len());
    }
    let write_ck = |c: &crate::checkpoint::Checkpoint| {
        if let Err(e) = std::fs::write(&ck_path, c.render()) {
            // Not fatal: the run still produces the right video, it just will
            // not be resumable. Saying so beats a silent loss of the feature.
            eprintln!("resume: could not write {}: {e}", ck_path.display());
        }
    };
    write_ck(&ck);

    let (tx, rx) = std::sync::mpsc::channel::<(usize, SegMsg)>();
    let mut handles = Vec::new();
    for (seg, part) in segs.iter().zip(&parts) {
        if done.contains(&seg.index) {
            continue;
        }
        let (seg, part, tx, p, job, servers, all, cache, cfg) = (*seg, part.clone(), tx.clone(), p.clone(), job.clone(), servers.clone(), keyframes.clone(), shared_cache.clone(), cfg);
        handles.push(std::thread::Builder::new().name(format!("d2x-seg{}", seg.index)).spawn(move || -> anyhow::Result<Summary> {
            let src = Box::new(FfmpegSource::open_segment(&job.input, &p, seg)?);
            let mut enc = job.encode.clone();
            // Keyframes relative to this segment's start (one scan, shared).
            enc.keyframes = all.iter().filter(|&&t| t >= seg.start && seg.end.map_or(true, |e| t < e)).map(|t| t - seg.start).collect();
            let enc = with_keyint(enc, &all, p.info.fps);
            let sink = Box::new(FfmpegSink::open(&part, None, &p, w, h, &enc)?);
            let ups: Vec<Box<dyn Upscaler>> = servers.iter().map(|s| Box::new(s.client()) as Box<dyn Upscaler>).collect();
            let r = d2x_core::pipeline::run_tiered_with_cache(src, ups, sink, cfg, cache, |e| {
                let _ = tx.send((seg.index, SegMsg::Progress(e)));
            });
            match &r {
                // The completion signal the checkpoint needs. `join()` also
                // reports success, but only after EVERY worker has finished —
                // too late to record anything, which is exactly the window a
                // kill lands in.
                Ok(_) => { let _ = tx.send((seg.index, SegMsg::Done)); }
                Err(e) => { let _ = tx.send((seg.index, SegMsg::Failed(format!("{e:#}")))); }
            }
            r
        })?);
    }
    drop(tx);
    for (i, msg) in rx {
        match msg {
            SegMsg::Progress(e) => on_event(i, e),
            SegMsg::Done => {
                // Written on EVERY completion rather than once at the end: the
                // whole point is to survive a kill, and a checkpoint flushed
                // at exit is one that never exists when it is needed.
                ck.done.insert(i);
                write_ck(&ck);
            }
            SegMsg::Failed(e) => eprintln!("segment {i}: {e}"),
        }
    }
    let mut frames = 0;
    let mut fraction = 0.0;
    let mut heavy = 0.0;
    let mut elapsed = std::time::Duration::ZERO;
    for h in handles {
        let s = h.join().map_err(|_| anyhow!("segment worker panicked"))??;
        fraction += s.upscaled_pixel_fraction * s.frames as f64;
        heavy += s.heavy_pixel_fraction * s.frames as f64;
        frames += s.frames;
        elapsed = elapsed.max(s.elapsed);
    }
    // Concat FIRST, then drop the parts and the checkpoint. If the concat
    // fails the parts and the record both survive, so the next run resumes
    // instead of re-upscaling everything to hit the same muxer error.
    ffmpeg::concat_parts(&parts, &job.input, &job.output)?;
    for part in &parts {
        let _ = std::fs::remove_file(part);
    }
    let _ = std::fs::remove_file(&ck_path);
    // NOTE: these totals cover the segments this RUN processed. A resumed run
    // reports less work than the video contains, because the skipped segments
    // were measured by the run that did them. Reporting a guess for them would
    // be worse than reporting the truth about what just happened.
    Ok(Summary { frames, elapsed, upscaled_pixel_fraction: if frames > 0 { fraction / frames as f64 } else { 0.0 }, heavy_pixel_fraction: if frames > 0 { heavy / frames as f64 } else { 0.0 } })
}

/// Run a job end to end. `on_event` receives pipeline events on the calling thread.
pub fn run(job: &Job, mut on_event: impl FnMut(Event)) -> anyhow::Result<Summary> {
    if job.jobs > 1 {
        return run_segmented(job, |i, e| on_event(Event::Segment(i, e)));
    }
    on_event(Event::Stage(format!("probing {}", job.input.display())));
    let (source, probe) = open_source(job)?;
    let info = source.info();
    let keyframes = if probe.is_some() && job.output.extension().is_some() {
        on_event(Event::Stage("scanning keyframes (packet headers only; cached in a sidecar)".into()));
        ffmpeg::keyframe_times(&job.input).unwrap_or_default()
    } else {
        Vec::new()
    };
    on_event(Event::Stage(format!("loading model and initialising the {:?} backend", job.backend)));
    let tiers = build_tiers(job)?;
    let scale = tiers[0].scale();
    let plan = mem_plan(job, info.width, info.height, scale);
    let mut cfg = job.pipeline;
    apply_plan(&mut cfg, &plan);
    on_event(Event::Stage(format!("memory: {plan}")));
    on_event(Event::Stage("starting encoder".into()));
    let sink = open_sink(job, probe.as_ref(), &keyframes, info.width * scale, info.height * scale)?;
    on_event(Event::Stage("frame 0 is a full-frame upscale; later frames only pay for what changed".into()));
    d2x_core::pipeline::run_tiered(source, tiers, sink, cfg, |e| on_event(Event::Pipeline(e)))
}
