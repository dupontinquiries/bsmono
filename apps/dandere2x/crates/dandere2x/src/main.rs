//! dandere2x — block-matched residual super-resolution for video.
//!
//! `dandere2x upscale -i in.mkv -o out.mkv --model RealESRGAN_x2.safetensors`
//! `dandere2x analyze -i in.mkv`      what block reuse buys on this clip
//! `dandere2x selftest [--model ..]`  GPU kernels vs CPU reference
//! `dandere2x probe`                  adapters wgpu can see
//! `dandere2x app`                    the egui shell (feature `app`)

mod ffmpeg;
mod job;
#[cfg(feature = "app")]
mod app;

use clap::{Parser, Subcommand};
use d2x_core::pipeline::{Event, MatcherKind};
use job::{Backend, Job};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "dandere2x", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Args, Clone)]
struct Common {
    /// Model file: .safetensors (Real-ESRGAN RRDB/SRVGG via tools/pth2st.py) or waifu2x .json. Default: vendored realesr-animevideov3.
    #[arg(long)]
    model: Option<PathBuf>,
    /// wgpu | cpu | bicubic | nearest
    #[arg(long, default_value = "wgpu")]
    backend: String,
    /// Output scale (a x4 model serving x2 is resampled with Lanczos).
    #[arg(long, default_value_t = 2)]
    scale: u32,
    /// GPU tile side in input pixels (0 = per-model default).
    #[arg(long, default_value_t = 0)]
    tile: u32,
    /// Refuse software adapters.
    #[arg(long)]
    require_hw: bool,
    #[arg(long, default_value_t = 30)]
    block_size: u32,
    /// JPEG quality the reuse gate is measured against (higher = stricter = less reuse).
    #[arg(long, short = 'q', default_value_t = 97)]
    quality: u8,
    #[arg(long, default_value_t = 1)]
    bleed: u32,
    /// Uniform noise on inputs (upstream 8 for waifu2x; 0 for Real-ESRGAN).
    #[arg(long, default_value_t = 0)]
    noise: u8,
    /// stationary | diamond | exhaustive:<radius>
    #[arg(long, default_value = "diamond")]
    matcher: String,
    /// Stop after N frames (settings trials).
    #[arg(long, default_value_t = 0)]
    frames: u64,
}

#[derive(Subcommand)]
enum Cmd {
    /// Upscale a video (or a directory of PNGs) to AV1 (or a PNG directory).
    Upscale {
        #[arg(short, long)]
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[command(flatten)]
        common: Common,
        /// av1 | x264 | x265
        #[arg(long, default_value = "av1")]
        codec: String,
        #[arg(long, default_value_t = 24)]
        crf: u32,
        /// SVT-AV1 preset 0..13 (6 ≈ 1 fps at 1080p on an M2 for the encoder alone).
        #[arg(long, default_value_t = 6)]
        preset: u32,
        /// 8-bit output instead of 10-bit.
        #[arg(long)]
        eight_bit: bool,
        /// Segment workers: cut on source keyframes, one shared GPU, N encoders, parts concatenated.
        #[arg(long, short = 'j', default_value_t = 1)]
        jobs: usize,
    },
    /// Stage A only: per-frame reuse statistics, no upscaling.
    Analyze {
        #[arg(short, long)]
        input: PathBuf,
        #[command(flatten)]
        common: Common,
    },
    /// GPU kernels against the CPU reference.
    Selftest {
        #[arg(long)]
        model: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        tile: u32,
    },
    /// List GPU adapters.
    Probe,
    /// Print a model's architecture and cost.
    Model {
        path: PathBuf,
    },
    /// The GUI.
    #[cfg(feature = "app")]
    App,
}

fn parse_matcher(s: &str) -> anyhow::Result<MatcherKind> {
    Ok(match s {
        "stationary" => MatcherKind::Stationary,
        "diamond" => MatcherKind::Diamond,
        s if s.starts_with("exhaustive") => MatcherKind::Exhaustive { radius: s.split(':').nth(1).and_then(|r| r.parse().ok()).unwrap_or(4) },
        _ => anyhow::bail!("matcher must be stationary|diamond|exhaustive[:radius]"),
    })
}

fn job_from(common: &Common, input: PathBuf, output: PathBuf) -> anyhow::Result<Job> {
    let mut j = Job { input, output, model: common.model.clone(), backend: common.backend.parse::<Backend>()?, scale: common.scale, tile: common.tile, require_hw: common.require_hw, max_frames: common.frames, ..Default::default() };
    j.pipeline.analyze.block_size = common.block_size;
    j.pipeline.analyze.bleed = common.bleed;
    j.pipeline.quality = common.quality;
    j.pipeline.noise = common.noise;
    j.pipeline.matcher = parse_matcher(&common.matcher)?;
    Ok(j)
}

fn print_event(e: &Event) {
    match e {
        Event::Started { info, padded, upscaler, matcher, gate } => {
            eprintln!("source {}x{} @ {}/{} fps, {} frames; analysis grid {}x{}", info.width, info.height, info.fps.0, info.fps.1, info.frame_count.map_or("?".into(), |n| n.to_string()), padded.0, padded.1);
            eprintln!("upscaler: {upscaler}\nmatcher: {matcher}, gate: {gate}");
        }
        Event::FrameDone { index, stats, analyze_time, upscale_time, merge_time } => {
            let kind = if stats.redraw { "REDRAW" } else { "" };
            eprintln!(
                "frame {index:6}  psnr {:5.1}  stat {:4} mov {:4} miss {:4} fade {:3}  residual {}x{} {kind}  A {:5.1}ms U {:7.1}ms M {:5.1}ms",
                stats.psnr,
                stats.stationary,
                stats.moved,
                stats.missing,
                stats.fades,
                stats.residual_dim,
                stats.residual_dim,
                analyze_time.as_secs_f64() * 1e3,
                upscale_time.as_secs_f64() * 1e3,
                merge_time.as_secs_f64() * 1e3
            );
        }
        Event::Finished { frames, elapsed, upscaled_pixel_fraction } => {
            eprintln!("done: {frames} frames in {:.1}s ({:.2} fps); {:.1}% of pixels went through the network", elapsed.as_secs_f64(), *frames as f64 / elapsed.as_secs_f64().max(1e-9), upscaled_pixel_fraction * 100.0);
        }
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Upscale { input, output, common, codec, crf, preset, eight_bit, jobs } => {
            let mut job = job_from(&common, input, output)?;
            job.jobs = jobs;
            job.encode.codec = codec;
            job.encode.crf = crf;
            job.encode.preset = preset;
            job.encode.ten_bit = !eight_bit;
            // PREFLIGHT BEFORE ANY WORK. The encoder is the last step of the
            // pipeline, so a missing libsvtav1 surfaces after every frame has
            // been through the CNN — the whole job wasted to learn something
            // one `ffmpeg -encoders` call knows up front. Only the encoder
            // actually selected is required; asking for AV1 support on an
            // x264 run would be a refusal the operator cannot act on.
            {
                let want_enc: &[&str] = match job.encode.codec.as_str() {
                    "av1" => &["libsvtav1"],
                    "x265" => &["libx265"],
                    _ => &["libx264"],
                };
                let pf = ffmpeg::preflight(want_enc, &[]);
                if let Some(msg) = pf.problem() {
                    anyhow::bail!("{msg}");
                }
            }
            if job.jobs > 1 {
                let s = job::run_segmented(&job, |seg, e| {
                    if !matches!(e, Event::Finished { .. }) {
                        eprint!("[seg {seg}] ");
                        print_event(&e);
                    }
                })?;
                eprintln!("done: {} frames in {:.1}s ({:.2} fps); {:.1}% of pixels went through the network", s.frames, s.elapsed.as_secs_f64(), s.frames as f64 / s.elapsed.as_secs_f64().max(1e-9), s.upscaled_pixel_fraction * 100.0);
            } else {
                job::run(&job, |e| print_event(&e))?;
            }
        }
        Cmd::Analyze { input, common } => {
            let job = job_from(&common, input, PathBuf::new())?;
            let (source, _) = job::open_source(&job)?;
            let bleed = job.pipeline.analyze.bleed;
            let summary = d2x_core::pipeline::analyze_only(source, job.pipeline, |i, s, frac, t| {
                eprintln!("frame {i:6}  psnr {:5.1}  stat {:4} mov {:4} miss {:4} fade {:3}  upscaled {:5.1}%{}  {:.1}ms", s.psnr, s.stationary, s.moved, s.missing, s.fades, frac * 100.0, if s.redraw { " REDRAW" } else { "" }, t.as_secs_f64() * 1e3);
            })?;
            let _ = bleed;
            eprintln!("{} frames, {:.1}% of pixels would go through the network ({:.1}x fewer than full upscaling)", summary.frames, summary.upscaled_pixel_fraction * 100.0, 1.0 / summary.upscaled_pixel_fraction.max(1e-9));
        }
        Cmd::Selftest { model, tile } => {
            #[cfg(feature = "gpu")]
            {
                let (net, path) = job::load_model(model.as_deref())?;
                eprintln!("model: {} ({:.2} MMAC per input pixel, x{})", path.display(), net.macs_per_input_pixel() / 1e6, net.scale);
                let r = d2x_wgpu::selftest(net, d2x_wgpu::GpuOptions { tile, require_hw: false, low_power: false })?;
                println!("{r}");
                if !r.pass {
                    std::process::exit(1);
                }
            }
            #[cfg(not(feature = "gpu"))]
            {
                let _ = (model, tile);
                anyhow::bail!("built without the `gpu` feature");
            }
        }
        Cmd::Probe => {
            #[cfg(feature = "gpu")]
            {
                let a = d2x_wgpu::probe();
                if a.is_empty() {
                    println!("no adapters (no Vulkan ICD? try `vulkaninfo --summary`)");
                }
                for r in a {
                    println!("{r}");
                }
            }
            #[cfg(not(feature = "gpu"))]
            println!("built without the `gpu` feature");
        }
        Cmd::Model { path } => {
            let net = d2x_core::net::Network::load(&path)?;
            println!("{}: x{} ({}), {} ops, {:.2} MMAC per input pixel, pre-pad {}, unshuffle {}, shuffle {}", net.name, net.scale, if net.exact { "valid" } else { "same-pad" }, net.ops.len(), net.macs_per_input_pixel() / 1e6, net.pre_pad, net.unshuffle, net.shuffle);
            let (w, h) = (1920u64, 1080u64);
            println!("1080p frame: {:.2} TMAC full-frame", net.macs_per_input_pixel() * (w * h) as f64 / 1e12);
        }
        #[cfg(feature = "app")]
        Cmd::App => app::run()?,
    }
    Ok(())
}
