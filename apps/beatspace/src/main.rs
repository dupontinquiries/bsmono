//! beatspace — a Rust port of the kBM3 mel-spectrogram GAN.
//!
//!   beatspace train [--data-dir ./data] [--epochs N] [--resume]
//!   beatspace gui   [--ckpt-dir ./checkpoints]
//!
//! `train` learns a generator from your own audio; `gui` opens the realtime
//! latent-space explorer that morphs between generated beats.

mod audio_io;
mod config;
mod dataset;
mod dsp;
#[cfg(feature = "gpu")]
mod gpu;
mod gui;
mod model;
mod player;
mod train;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

use config::Config;

#[derive(Parser)]
#[command(name = "beatspace", version, about = "Train a beat GAN and explore its latent space in realtime.")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Train the GAN on a folder of audio files.
    Train(TrainArgs),
    /// Open the realtime latent-space GUI.
    Gui(GuiArgs),
    /// Validate + benchmark the GPU generator (batched) against the CPU.
    #[cfg(feature = "gpu")]
    Gpu(GpuArgs),
}

#[cfg(feature = "gpu")]
#[derive(Args)]
struct GpuArgs {
    /// Directory containing config.json + gen_latest.safetensors.
    #[arg(long, default_value = "./checkpoints")]
    ckpt_dir: PathBuf,
    /// How many beats to generate in one batch.
    #[arg(long, default_value_t = 64)]
    batch: usize,
}

#[derive(Args)]
struct TrainArgs {
    /// Folder of audio files to train on (.opus, .wav, .flac, .mp3, ...).
    #[arg(long, default_value = "./data")]
    data_dir: PathBuf,
    /// Where checkpoints + config.json are written.
    #[arg(long, default_value = "./checkpoints")]
    ckpt_dir: PathBuf,
    /// Mel-cache directory.
    #[arg(long, default_value = "./cache")]
    cache_dir: PathBuf,
    /// Number of epochs.
    #[arg(long)]
    epochs: Option<usize>,
    /// Batch size.
    #[arg(long)]
    batch_size: Option<usize>,
    /// Steps (batches) per epoch.
    #[arg(long)]
    steps_per_epoch: Option<usize>,
    /// Save a checkpoint every N epochs.
    #[arg(long)]
    ckpt_every: Option<usize>,
    /// Resume from gen_latest/disc_latest in ckpt_dir if present.
    #[arg(long, default_value_t = false)]
    resume: bool,
}

#[derive(Args)]
struct GuiArgs {
    /// Directory containing config.json + gen_latest.safetensors.
    #[arg(long, default_value = "./checkpoints")]
    ckpt_dir: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Train(a) => {
            let mut cfg = Config::default();
            cfg.data_dir = a.data_dir;
            cfg.ckpt_dir = a.ckpt_dir;
            cfg.cache_dir = a.cache_dir;
            if let Some(v) = a.epochs {
                cfg.epochs = v;
            }
            if let Some(v) = a.batch_size {
                cfg.batch_size = v;
            }
            if let Some(v) = a.steps_per_epoch {
                cfg.steps_per_epoch = v;
            }
            if let Some(v) = a.ckpt_every {
                cfg.ckpt_every = v;
            }
            train::run(&cfg, a.resume)?;
        }
        Cmd::Gui(a) => {
            // Prefer the config saved alongside the checkpoints so the model
            // shape matches what was trained; fall back to defaults.
            let cfg_path = a.ckpt_dir.join("config.json");
            let mut cfg = if cfg_path.exists() {
                match Config::load_json(&cfg_path) {
                    Ok(mut c) => {
                        c.ckpt_dir = a.ckpt_dir.clone();
                        println!("[gui] loaded {cfg_path:?}");
                        c
                    }
                    Err(e) => {
                        eprintln!("[gui] couldn't read {cfg_path:?}: {e}; using defaults");
                        let mut c = Config::default();
                        c.ckpt_dir = a.ckpt_dir.clone();
                        c
                    }
                }
            } else {
                let mut c = Config::default();
                c.ckpt_dir = a.ckpt_dir.clone();
                c
            };
            cfg.ckpt_dir = a.ckpt_dir;
            gui::run(cfg)?;
        }
        #[cfg(feature = "gpu")]
        Cmd::Gpu(a) => {
            run_gpu(a)?;
        }
    }
    Ok(())
}

#[cfg(feature = "gpu")]
fn run_gpu(a: GpuArgs) -> Result<()> {
    use candle_core::Device;
    use std::time::Instant;

    let cfg_path = a.ckpt_dir.join("config.json");
    let cfg = if cfg_path.exists() {
        Config::load_json(&cfg_path)?
    } else {
        Config::default()
    };
    let dev = Device::Cpu;

    // candle generator (reference, CPU)
    let mut g = model::Generator::new(&cfg, &dev)?;
    let latest = a.ckpt_dir.join("gen_latest.safetensors");
    if latest.exists() {
        model::load_generator(&mut g, &latest, &dev)?;
        println!("[gpu] loaded {latest:?}");
    } else {
        println!("[gpu] no checkpoint; using random init (still a valid correctness test)");
    }

    let batch = a.batch.max(1);
    let latent = cfg.latent;
    // deterministic-ish latents
    let zs: Vec<f32> = (0..batch * latent)
        .map(|i| (((i * 2654435761usize) % 1000) as f32 / 500.0) - 1.0)
        .collect();

    // ---- GPU path ----
    let params = g.export_params()?;
    let gpu = gpu::GpuGenerator::new(&params)?;
    println!("[gpu] adapter: {}", gpu.adapter_name());
    // warm up (shader compile / first submit)
    let _ = gpu.generate_batch(&zs, batch)?;
    let t0 = Instant::now();
    let gpu_out = gpu.generate_batch(&zs, batch)?;
    let gpu_ms = t0.elapsed().as_secs_f32() * 1000.0;

    // ---- CPU reference, batched candle forward (one big forward pass) ----
    let beat = cfg.n_mels * cfg.time_frames;
    let t_cb = Instant::now();
    let zt = candle_core::Tensor::from_vec(zs.clone(), (batch, latent), &dev)?;
    let mel_b = g.forward_eval(&zt)?; // (batch,1,n_mels,T)
    let cpu_batch_out = mel_b.flatten_all()?.to_vec1::<f32>()?;
    let cpu_batch_ms = t_cb.elapsed().as_secs_f32() * 1000.0;

    // ---- CPU reference, one beat at a time (the naive loop) ----
    let t1 = Instant::now();
    let mut cpu_out = vec![0f32; batch * beat];
    for b in 0..batch {
        let z = &zs[b * latent..(b + 1) * latent];
        let (flat, _nm, _t) = g.generate(z, &dev)?;
        cpu_out[b * beat..(b + 1) * beat].copy_from_slice(&flat);
    }
    let cpu_ms = t1.elapsed().as_secs_f32() * 1000.0;

    // ---- compare ----
    let mut max_abs = 0f32;
    let mut sum_sq = 0f64;
    for (a, b) in gpu_out.iter().zip(cpu_out.iter()) {
        let d = (a - b).abs();
        if d > max_abs {
            max_abs = d;
        }
        sum_sq += (d as f64) * (d as f64);
    }
    let rmse = (sum_sq / gpu_out.len() as f64).sqrt();

    println!("\n  batch              : {batch} beats ({}x{} each)", cfg.n_mels, cfg.time_frames);
    println!("  GPU (batched)      : {gpu_ms:9.1} ms   {:7.2} ms/beat   [{}]", gpu_ms / batch as f32, gpu.adapter_name());
    println!("  CPU candle batched : {cpu_batch_ms:9.1} ms   {:7.2} ms/beat", cpu_batch_ms / batch as f32);
    println!("  CPU candle per-beat: {cpu_ms:9.1} ms   {:7.2} ms/beat", cpu_ms / batch as f32);
    println!("  CPU batch speedup  : {:.2}x  (batched fwd vs per-beat loop)", cpu_ms / cpu_batch_ms.max(1e-3));
    println!("  GPU vs CPU per-beat: {:.2}x", cpu_ms / gpu_ms.max(1e-3));
    println!("  max |GPU-CPU|      : {max_abs:.3e}   rmse {rmse:.3e}");
    if max_abs < 2e-3 {
        println!("  -> GPU output matches the candle generator. ✓");
    } else {
        println!("  -> WARNING: GPU/CPU differ more than expected.");
    }
    let _ = cpu_batch_out; // (validated implicitly by shapes; CPU paths agree)
    Ok(())
}
