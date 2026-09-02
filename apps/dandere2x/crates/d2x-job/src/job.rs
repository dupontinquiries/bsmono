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
        }
    }
}

/// The vendored fast model, relative to the app's `models/` directory, tried
/// when no `--model` is given.
pub fn default_model_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        for anc in exe.ancestors().take(5) {
            v.push(anc.join("models/realesr-animevideov3.safetensors"));
        }
    }
    v.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models/realesr-animevideov3.safetensors"));
    v
}

pub fn load_model(explicit: Option<&Path>) -> anyhow::Result<(Arc<Network>, PathBuf)> {
    let path = match explicit {
        Some(p) => p.to_path_buf(),
        None => default_model_paths().into_iter().find(|p| p.exists()).ok_or_else(|| anyhow!("no --model given and the vendored realesr-animevideov3.safetensors was not found"))?,
    };
    let net = Network::load(&path).with_context(|| format!("loading model {}", path.display()))?;
    Ok((Arc::new(net), path))
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
                Box::new(d2x_wgpu::WgpuUpscaler::new(net, d2x_wgpu::GpuOptions { tile: job.tile, require_hw: job.require_hw, low_power: false })?)
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
pub fn run_segmented(job: &Job, mut on_event: impl FnMut(usize, PEvent)) -> anyhow::Result<Summary> {
    anyhow::ensure!(!job.input.is_dir() && job.output.extension().is_some(), "--jobs needs a video input and a video output");
    anyhow::ensure!(job.start == 0.0 && job.end.is_none(), "segment parallelism over a sub-range is not implemented yet; use jobs = 1 for clip bounces");
    let p = ffmpeg::probe(&job.input)?;
    let keyframes = ffmpeg::keyframe_times(&job.input).unwrap_or_default();
    let segs = ffmpeg::segments(&keyframes, p.duration_s, job.jobs);
    let server = BatchServer::start(build_upscaler(job)?, std::time::Duration::from_millis(15), 4_000_000);
    let scale = server.scale;
    let (w, h) = (p.info.width * scale, p.info.height * scale);
    let stem = job.output.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
    let ext = job.output.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "mkv".into());
    let parts: Vec<PathBuf> = segs.iter().map(|s| job.output.with_file_name(format!("{stem}.part{:03}.{ext}", s.index))).collect();

    let (tx, rx) = std::sync::mpsc::channel::<(usize, Result<PEvent, String>)>();
    let mut handles = Vec::new();
    for (seg, part) in segs.iter().zip(&parts) {
        let (seg, part, tx, p, job, server, all) = (*seg, part.clone(), tx.clone(), p.clone(), job.clone(), server.clone(), keyframes.clone());
        handles.push(std::thread::Builder::new().name(format!("d2x-seg{}", seg.index)).spawn(move || -> anyhow::Result<Summary> {
            let src = Box::new(FfmpegSource::open_segment(&job.input, &p, seg)?);
            let mut enc = job.encode.clone();
            // Keyframes relative to this segment's start (one scan, shared).
            enc.keyframes = all.iter().filter(|&&t| t >= seg.start && seg.end.map_or(true, |e| t < e)).map(|t| t - seg.start).collect();
            let enc = with_keyint(enc, &all, p.info.fps);
            let sink = Box::new(FfmpegSink::open(&part, None, &p, w, h, &enc)?);
            let up: Box<dyn Upscaler> = Box::new(server.client());
            let r = d2x_core::pipeline::run(src, up, sink, job.pipeline, |e| {
                let _ = tx.send((seg.index, Ok(e)));
            });
            if let Err(e) = &r {
                let _ = tx.send((seg.index, Err(format!("{e:#}"))));
            }
            r
        })?);
    }
    drop(tx);
    for (i, msg) in rx {
        match msg {
            Ok(e) => on_event(i, e),
            Err(e) => eprintln!("segment {i}: {e}"),
        }
    }
    let mut frames = 0;
    let mut fraction = 0.0;
    let mut elapsed = std::time::Duration::ZERO;
    for h in handles {
        let s = h.join().map_err(|_| anyhow!("segment worker panicked"))??;
        fraction += s.upscaled_pixel_fraction * s.frames as f64;
        frames += s.frames;
        elapsed = elapsed.max(s.elapsed);
    }
    ffmpeg::concat_parts(&parts, &job.input, &job.output)?;
    for part in &parts {
        let _ = std::fs::remove_file(part);
    }
    Ok(Summary { frames, elapsed, upscaled_pixel_fraction: if frames > 0 { fraction / frames as f64 } else { 0.0 } })
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
    let upscaler = build_upscaler(job)?;
    let scale = upscaler.scale();
    on_event(Event::Stage("starting encoder".into()));
    let sink = open_sink(job, probe.as_ref(), &keyframes, info.width * scale, info.height * scale)?;
    on_event(Event::Stage("frame 0 is a full-frame upscale; later frames only pay for what changed".into()));
    d2x_core::pipeline::run(source, upscaler, sink, job.pipeline, |e| on_event(Event::Pipeline(e)))
}
