//! dandere2x — block-matched residual super-resolution for video.
//!
//! `dandere2x upscale -i in.mkv -o out.mkv --model RealESRGAN_x2.safetensors`
//! `dandere2x analyze -i in.mkv`      what block reuse buys on this clip
//! `dandere2x selftest [--model ..]`  GPU kernels vs CPU reference
//! `dandere2x probe`                  adapters wgpu can see
//! `dandere2x app`                    the egui shell (feature `app`)

#[cfg(feature = "app")]
mod app;

use clap::{Parser, Subcommand};
use d2x_core::pipeline::{Event, FrameSource, MatcherKind};
use d2x_job::job::{self, Backend, Job};
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
    /// Context ring around each residual block, in input pixels.
    ///
    /// This is what the CNN sees AROUND a block it is re-upscaling. Upstream's
    /// 1 was tuned for waifu2x, whose receptive field is a handful of pixels;
    /// Real-ESRGAN's RRDB body sees hundreds, so one pixel of context is
    /// effectively none and the block's own edges come back wrong.
    ///
    /// That is the flicker: a seam is DRAWN where a block was re-upscaled and
    /// ABSENT where the frame was copied from the previous one, so fixed grid
    /// lines blink on and off as the residual planner changes its mind frame to
    /// frame. A purely spatial artefact would be steady; a purely motion-driven
    /// one would slide. Blinking at fixed positions is what points at the
    /// residual path rather than the CNN tile grid.
    ///
    /// 8 costs (block+2*8)^2 / block^2 per residual — at the default 30px block
    /// that is ~2.4x the pixels through the network per block, which is real but
    /// bounded, and only for blocks that actually changed.
    #[arg(long, default_value_t = 8)]
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
    /// Recent outputs kept as extra references (occlusions, flicker). Default: from --ram-budget.
    #[arg(long)]
    refs: Option<usize>,
    /// Shot-start outputs retained as references (cut-backs). Default: from --ram-budget.
    #[arg(long)]
    keyframe_refs: Option<usize>,
    /// Host memory the job may hold (e.g. 96G). Default: half of MemAvailable.
    #[arg(long)]
    ram_budget: Option<String>,
    /// GPU memory for activation sets (e.g. 8G). Default: a quarter of the RAM budget.
    #[arg(long)]
    vram_budget: Option<String>,
    /// Content-keyed block cache size (e.g. 4G; 0 = off). Default: 20% of the RAM budget.
    #[arg(long)]
    block_cache: Option<String>,
    /// Activation sets in flight per GPU upscaler. On unified memory (Apple/Asahi)
    /// there is no transfer to hide and each set steals VRAM from tile size:
    /// measured 27% slower at 4 sets than at 1. Raise only on discrete GPUs.
    #[arg(long, default_value_t = 1)]
    inflight: usize,
    /// Conv kernel: rt2 | rt1 | rt4 | wino (Winograd F(2,3), 2.25x fewer multiplies).
    /// All bit-exact; `bench` compares. rt2 is fastest on AGX among the direct forms.
    #[arg(long, default_value = "rt2f")]
    kernel: String,
    /// Frames whose residuals share one GPU call.
    #[arg(long, default_value_t = 4)]
    batch: usize,
    /// Difficulty cascade: smooth missing blocks go to a cheaper model. Give a
    /// model path, or "default" for the vendored realesr-animevideov3.
    #[arg(long)]
    cascade: Option<String>,
    /// Flat missing blocks (luma std below --flat-std) skip the network: bicubic.
    #[arg(long)]
    flat_bypass: bool,
    /// Luma std-dev below which a block is "flat".
    #[arg(long, default_value_t = 2.0)]
    flat_std: f64,
    /// Luma std-dev below which a block is "smooth" (cascade tier).
    #[arg(long, default_value_t = 8.0)]
    smooth_std: f64,
    /// Gate hysteresis for long-reused blocks: threshold × (1 + bonus·min(streak,8)/8). 0 = off.
    #[arg(long, default_value_t = 0.0)]
    streak_bonus: f64,
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
        /// Reuse finished segment parts from an earlier run with identical settings.
        #[arg(long)]
        resume: bool,
        /// Keep segment parts after concat, so a later --resume can reuse them.
        #[arg(long)]
        keep_parts: bool,
        /// Start time in seconds (snaps to the previous keyframe).
        #[arg(long, default_value_t = 0.0)]
        start: f64,
        /// End time in seconds.
        #[arg(long)]
        end: Option<f64>,
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
        /// rt2f (default) | rt2h | rt2c8h | rt2c8 | rt2 | rt1 | rt4 | wino
        #[arg(long, default_value = "rt2f")]
        kernel: String,
    },
    /// Compare two renders of the same clip: PSNR and SSIM per frame, plus the
    /// worst frame. This is the other half of every speed number — run the
    /// slow settings once as a reference, then price a fast setting against it.
    Compare {
        /// Reference render (the slow settings).
        #[arg(short, long)]
        reference: PathBuf,
        /// Render under test.
        #[arg(short, long)]
        test: PathBuf,
        /// Stop after N frames (0 = all).
        #[arg(long, default_value_t = 0)]
        frames: u64,
        /// Print a line per frame, not just the summary.
        #[arg(long)]
        per_frame: bool,
    },
    /// List GPU adapters.
    Probe,
    /// Time the network on full-size tiles on this GPU, both kernels.
    Bench {
        #[arg(long)]
        model: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        tile: u32,
        #[arg(long, default_value_t = 1)]
        inflight: usize,
        #[arg(long, default_value_t = 6)]
        iters: u32,
        /// GPU memory budget (e.g. 8G); with it the tile grows up to 1024.
        #[arg(long)]
        vram_budget: Option<String>,
    },
    /// Print a model's architecture and cost.
    Model {
        path: PathBuf,
    },
    /// Scan a models folder: architecture, cost, and whether each one loads
    /// and produces sane output. `--gmac` turns cost into seconds per frame
    /// using a figure from `bench` on this machine.
    Models {
        /// Folder to walk (default: the app's `models/` directory).
        dir: Option<PathBuf>,
        /// Skip the verification forward pass (classification only).
        #[arg(long)]
        quick: bool,
        /// GMAC/s from `dandere2x bench`, to report seconds per 1080p frame.
        #[arg(long)]
        gmac: Option<f64>,
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
    j.refs_override = match (common.refs, common.keyframe_refs) {
        (None, None) => None,
        (r, k) => Some((r.unwrap_or(3), k.unwrap_or(4))),
    };
    j.ram_budget = common.ram_budget.as_deref().map(d2x_core::budget::parse_bytes).transpose()?;
    j.vram_budget = common.vram_budget.as_deref().map(d2x_core::budget::parse_bytes).transpose()?;
    j.block_cache_override = common.block_cache.as_deref().map(d2x_core::budget::parse_bytes).transpose()?;
    j.inflight = common.inflight;
    j.kernel = common.kernel.clone();
    j.pipeline.batch = common.batch;
    j.pipeline.queue_depth = common.batch.max(2) * 2;
    j.pipeline.analyze.tier_std = [common.flat_std, common.smooth_std];
    j.pipeline.analyze.streak_bonus = common.streak_bonus;
    j.cascade_model = common.cascade.as_ref().map(|c| if c == "default" { None } else { Some(PathBuf::from(c)) });
    j.flat_bypass = common.flat_bypass;
    j.pipeline.tiers = 1 + j.cascade_model.is_some() as usize + j.flat_bypass as usize;
    Ok(j)
}

fn tiers_str(s: &d2x_core::plan::FrameStats) -> String {
    let names = ["main", "cheap", "bicubic"];
    let parts: Vec<String> = s.residual_tiers.iter().enumerate().filter(|(_, &(c, _))| c > 0).map(|(t, &(c, r))| format!("{}:{c}x{r}", names[t])).collect();
    if parts.is_empty() { "-".into() } else { parts.join(" ") }
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
                "frame {index:6}  psnr {:5.1}  stat {:4} mov {:4} rec {:3} cache {:3} miss {:4} fade {:3}  residual {} {kind}  A {:5.1}ms U {:7.1}ms M {:5.1}ms",
                stats.psnr,
                stats.stationary,
                stats.moved,
                stats.recalled,
                stats.cached,
                stats.missing,
                stats.fades,
                tiers_str(stats),
                analyze_time.as_secs_f64() * 1e3,
                upscale_time.as_secs_f64() * 1e3,
                merge_time.as_secs_f64() * 1e3
            );
        }
        Event::Finished { frames, elapsed, upscaled_pixel_fraction, .. } => {
            eprintln!("done: {frames} frames in {:.1}s ({:.2} fps); {:.1}% of pixels went through the network", elapsed.as_secs_f64(), *frames as f64 / elapsed.as_secs_f64().max(1e-9), upscaled_pixel_fraction * 100.0);
        }
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Upscale { input, output, common, codec, crf, preset, eight_bit, jobs, start, end, resume, keep_parts } => {
            let mut job = job_from(&common, input, output)?;
            job.jobs = jobs;
            job.resume = resume;
            job.keep_parts = keep_parts;
            job.start = start;
            job.end = end;
            job.encode.codec = codec;
            job.encode.crf = crf;
            job.encode.preset = preset;
            job.encode.ten_bit = !eight_bit;
            let s = job::run(&job, |e| match e {
                job::Event::Stage(m) => eprintln!("… {m}"),
                job::Event::Pipeline(e) => print_event(&e),
                job::Event::Segment(seg, e) => {
                    if !matches!(e, Event::Finished { .. }) {
                        eprint!("[seg {seg}] ");
                        print_event(&e);
                    }
                }
            })?;
            if job.jobs > 1 {
                eprintln!("done: {} frames in {:.1}s ({:.2} fps); {:.1}% of pixels went through the network", s.frames, s.elapsed.as_secs_f64(), s.frames as f64 / s.elapsed.as_secs_f64().max(1e-9), s.upscaled_pixel_fraction * 100.0);
            }
        }
        Cmd::Analyze { input, common } => {
            let job = job_from(&common, input, PathBuf::new())?;
            let (source, _) = job::open_source(&job)?;
            let bleed = job.pipeline.analyze.bleed;
            let summary = d2x_core::pipeline::analyze_only(source, job.pipeline, |i, s, frac, t| {
                eprintln!("frame {i:6}  psnr {:5.1}  stat {:4} mov {:4} rec {:3} miss {:4} fade {:3}  upscaled {:5.1}%{}  {:.1}ms", s.psnr, s.stationary, s.moved, s.recalled, s.missing, s.fades, frac * 100.0, if s.redraw { " REDRAW" } else { "" }, t.as_secs_f64() * 1e3);
            })?;
            let _ = bleed;
            eprintln!("{} frames: {:.1}% of pixels through the main network ({:.1}x fewer than full upscaling), {:.1}% through any network tier", summary.frames, summary.heavy_pixel_fraction * 100.0, 1.0 / summary.heavy_pixel_fraction.max(1e-9), summary.upscaled_pixel_fraction * 100.0);
        }
        Cmd::Selftest { model, tile, kernel } => {
            #[cfg(feature = "gpu")]
            {
                let (net, path) = job::load_model(model.as_deref())?;
                eprintln!("model: {} ({:.2} MMAC per input pixel, x{})", path.display(), net.macs_per_input_pixel() / 1e6, net.scale);
                let r = d2x_wgpu::selftest(net, d2x_wgpu::GpuOptions { tile, kernel: kernel.parse()?, ..Default::default() })?;
                println!("{r}");
                if !r.pass {
                    std::process::exit(1);
                }
            }
            #[cfg(not(feature = "gpu"))]
            {
                let _ = (model, tile, kernel);
                anyhow::bail!("built without the `gpu` feature");
            }
        }
        Cmd::Bench { model, tile, inflight, iters, vram_budget } => {
            #[cfg(feature = "gpu")]
            {
                let (net, path) = job::load_model(model.as_deref())?;
                let budget = vram_budget.as_deref().map(d2x_core::budget::parse_bytes).transpose()?.unwrap_or(0);
                eprintln!("model: {} ({:.2} MMAC per input pixel, x{})", path.display(), net.macs_per_input_pixel() / 1e6, net.scale);
                for kernel in ["rt2f", "rt2h", "rt2c8h", "rt2c8", "rt2c8h"] {
                    let opts = d2x_wgpu::GpuOptions { tile, inflight, vram_budget: budget, kernel: kernel.parse()?, ..Default::default() };
                    match d2x_wgpu::bench(net.clone(), opts, iters) {
                        Ok(r) => println!("{r}"),
                        Err(e) => println!("kernel {kernel}: {e:#}"),
                    }
                }
            }
            #[cfg(not(feature = "gpu"))]
            {
                let _ = (model, tile, inflight, iters, vram_budget);
                anyhow::bail!("built without the `gpu` feature");
            }
        }
        Cmd::Models { dir, quick, gmac } => {
            let dir = dir.unwrap_or_else(|| {
                job::default_model_paths()
                    .into_iter()
                    .find(|p| p.exists())
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                    .unwrap_or_else(|| PathBuf::from("models"))
            });
            // `..` segments from the default-path search make the printed
            // paths unusable for copy-paste; resolve them.
            let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
            let found = d2x_core::registry::scan(&dir, !quick);
            if found.is_empty() {
                println!("no .safetensors or .json models under {}", dir.display());
            }
            for m in &found {
                let (name, sub, badge) = m.row();
                let cost = gmac.map(|g| format!(" · {:.1} s/1080p frame", m.secs_per_1080p(g))).unwrap_or_default();
                println!("{badge:>6}  {name:<34} {sub}{cost}");
                if let Some(d) = m.status.detail() {
                    println!("        {d}");
                }
            }
            // The cascade needs two usable models at the same output scale;
            // say which pair the folder offers rather than making the user work
            // it out from the list.
            for scale in [2u32, 3, 4] {
                if let Some((cheap, dear)) = d2x_core::registry::ladder(&found, scale) {
                    println!(
                        "\nx{scale} ladder: --model {} --cascade {}  ({:.2} vs {:.2} MMAC/px, {:.0}x)",
                        dear.path.display(),
                        cheap.path.display(),
                        dear.macs_per_px / 1e6,
                        cheap.macs_per_px / 1e6,
                        dear.macs_per_px / cheap.macs_per_px.max(1.0)
                    );
                }
            }
        }
        Cmd::Compare { reference, test, frames, per_frame } => {
            let (pa, pb) = (d2x_job::ffmpeg::probe(&reference)?, d2x_job::ffmpeg::probe(&test)?);
            anyhow::ensure!(
                (pa.info.width, pa.info.height) == (pb.info.width, pb.info.height),
                "different sizes: {}x{} vs {}x{} — compare renders of the same clip at the same scale",
                pa.info.width,
                pa.info.height,
                pb.info.width,
                pb.info.height
            );
            let mut a = d2x_job::ffmpeg::FfmpegSource::open(&reference, &pa)?;
            let mut b = d2x_job::ffmpeg::FfmpegSource::open(&test, &pb)?;
            let mut rep = d2x_core::quality::QualityReport::new();
            let mut i = 0u64;
            loop {
                if frames > 0 && i >= frames {
                    break;
                }
                let (fa, fb) = (a.next_frame()?, b.next_frame()?);
                let (fa, fb) = match (fa, fb) {
                    (Some(x), Some(y)) => (x, y),
                    (None, None) => break,
                    // A length mismatch is worth failing on: comparing a
                    // truncated render against a full one would report a
                    // flattering number for the frames that exist.
                    (x, y) => anyhow::bail!("clips differ in length: reference {} test {} after {i} frames", if x.is_some() { "has more" } else { "ended" }, if y.is_some() { "has more" } else { "ended" }),
                };
                rep.push(i, &fa, &fb);
                if per_frame {
                    let p = fa.psnr(&fb);
                    println!("frame {i:6}  psnr {:7.2} dB  ssim {:.4}", p, d2x_core::quality::ssim(&fa, &fb));
                }
                i += 1;
            }
            println!("{rep}");
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
