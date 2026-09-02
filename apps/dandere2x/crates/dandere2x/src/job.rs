//! One place that turns settings into a running pipeline, shared by the CLI
//! subcommands and the egui shell so they cannot drift.

use crate::ffmpeg::{self, EncodeOptions, FfmpegSink, FfmpegSource, PngDirSink, PngDirSource};
use anyhow::{anyhow, Context};
use d2x_core::net::{CpuNet, Network};
use d2x_core::pipeline::{Event, FrameSink, FrameSource, MatcherKind, PipelineConfig, Summary};
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
            pipeline: PipelineConfig { analyze: AnalyzeConfig::default(), quality: 97, noise: 0, matcher: MatcherKind::Diamond, queue_depth: 2 },
            encode: EncodeOptions::default(),
            max_frames: 0,
            jobs: 1,
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

pub fn open_source(job: &Job) -> anyhow::Result<(Box<dyn FrameSource>, Option<ffmpeg::Probe>)> {
    let (src, probe): (Box<dyn FrameSource>, Option<ffmpeg::Probe>) = if job.input.is_dir() {
        (Box::new(PngDirSource::open(&job.input, (30, 1))?), None)
    } else {
        let p = ffmpeg::probe(&job.input)?;
        (Box::new(FfmpegSource::open(&job.input, &p)?), Some(p))
    };
    if job.max_frames > 0 {
        Ok((Box::new(Limited { inner: src, left: job.max_frames }), probe))
    } else {
        Ok((src, probe))
    }
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

pub fn open_sink(job: &Job, probe: Option<&ffmpeg::Probe>, width: u32, height: u32) -> anyhow::Result<Box<dyn FrameSink>> {
    if job.output.extension().is_none() || job.output.is_dir() {
        return Ok(Box::new(PngDirSink::open(&job.output)?));
    }
    let p = probe.ok_or_else(|| anyhow!("encoding to a video needs a video input (PNG-dir input can only write a PNG dir)"))?;
    let mut enc = job.encode.clone();
    if enc.keyframes.is_empty() {
        match ffmpeg::keyframe_times(&job.input) {
            Ok(k) => enc.keyframes = k,
            Err(e) => eprintln!("note: could not read source keyframes ({e}); encoder picks its own"),
        }
    }
    let kf = enc.keyframes.clone();
    let enc = with_keyint(enc, &kf, p.info.fps);
    Ok(Box::new(FfmpegSink::open(&job.output, Some(&job.input), p, width, height, &enc)?))
}

/// One upscaler shared by several pipelines: the GPU is serial anyway; what
/// sharing buys is that every worker's decode, analysis, merge and encode
/// overlap with the others' upscales, so the GPU queue is rarely empty.
pub struct SharedUpscaler(pub Arc<std::sync::Mutex<Box<dyn Upscaler>>>);

impl Upscaler for SharedUpscaler {
    fn scale(&self) -> u32 {
        self.0.lock().unwrap().scale()
    }
    fn upscale(&mut self, frame: &Frame) -> anyhow::Result<Frame> {
        self.0.lock().unwrap().upscale(frame)
    }
    fn name(&self) -> String {
        format!("shared {}", self.0.lock().unwrap().name())
    }
}

/// Segment-parallel run: `jobs` workers over keyframe-aligned segments of a
/// video input, one shared upscaler, part files concatenated at the end.
/// Events carry the segment index as `Event::Started`'s upscaler name prefix
/// and are delivered on the calling thread.
pub fn run_segmented(job: &Job, mut on_event: impl FnMut(usize, Event)) -> anyhow::Result<Summary> {
    anyhow::ensure!(!job.input.is_dir() && job.output.extension().is_some(), "--jobs needs a video input and a video output");
    let p = ffmpeg::probe(&job.input)?;
    let keyframes = ffmpeg::keyframe_times(&job.input).unwrap_or_default();
    let segs = ffmpeg::segments(&keyframes, p.duration_s, job.jobs);
    let upscaler = Arc::new(std::sync::Mutex::new(build_upscaler(job)?));
    let scale = upscaler.lock().unwrap().scale();
    let (w, h) = (p.info.width * scale, p.info.height * scale);
    let stem = job.output.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
    let ext = job.output.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "mkv".into());
    let parts: Vec<PathBuf> = segs.iter().map(|s| job.output.with_file_name(format!("{stem}.part{:03}.{ext}", s.index))).collect();

    let (tx, rx) = std::sync::mpsc::channel::<(usize, Result<Event, String>)>();
    let mut handles = Vec::new();
    for (seg, part) in segs.iter().zip(&parts) {
        let (seg, part, tx, p, job, upscaler) = (*seg, part.clone(), tx.clone(), p.clone(), job.clone(), upscaler.clone());
        handles.push(std::thread::Builder::new().name(format!("d2x-seg{}", seg.index)).spawn(move || -> anyhow::Result<Summary> {
            let src = Box::new(FfmpegSource::open_segment(&job.input, &p, seg)?);
            let mut enc = job.encode.clone();
            // Keyframes relative to this segment's start.
            let all = ffmpeg::keyframe_times(&job.input).unwrap_or_default();
            enc.keyframes = all.iter().filter(|&&t| t >= seg.start && seg.end.map_or(true, |e| t < e)).map(|t| t - seg.start).collect();
            let enc = with_keyint(enc, &all, p.info.fps);
            let sink = Box::new(FfmpegSink::open(&part, None, &p, w, h, &enc)?);
            let up: Box<dyn Upscaler> = Box::new(SharedUpscaler(upscaler));
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
        return run_segmented(job, |_, e| on_event(e));
    }
    let (source, probe) = open_source(job)?;
    let info = source.info();
    let upscaler = build_upscaler(job)?;
    let scale = upscaler.scale();
    let sink = open_sink(job, probe.as_ref(), info.width * scale, info.height * scale)?;
    d2x_core::pipeline::run(source, upscaler, sink, job.pipeline, on_event)
}
