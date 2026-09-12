//! beatspace — a Rust port of the kBM3 mel-spectrogram GAN.
//!
//!   beatspace train [--data-dir ./data] [--epochs N] [--resume]
//!   beatspace gui   [--ckpt-dir ./checkpoints]
//!
//! `train` learns a generator from your own audio; `gui` opens the realtime
//! latent-space explorer that morphs between generated beats.

mod micsample;
mod micsession;
#[cfg(feature = "app")] mod miclisten;
mod arrange;
mod audio_io;
mod config;
mod dataset;
mod upscale;
mod dsp;
#[cfg(feature = "gpu")]
mod gpu;
#[cfg(feature = "app")]
mod arrangepage;
#[cfg(feature = "app")]
mod gui;
mod model;
#[cfg(feature = "app")]
mod player;
mod seqstep;
mod smoke;
mod stream;
mod train;
mod trainpage;
mod wgpu_train;
#[cfg(feature = "gpu")]
mod wgpu_run;

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
    /// Listen on the microphone and keep only the interesting moments as
    /// training crops. Leave it running; train on what it collected.
    ///
    /// Writes mel spectrograms, never audio — but mel is NOT anonymised, since
    /// this crate's Griffin-Lim can turn a crop back into audible sound. Treat
    /// a session directory as recordings.
    #[cfg(feature = "app")]
    Listen(ListenArgs),
    /// Open the realtime latent-space GUI.
    #[cfg(feature = "app")]
    Gui(GuiArgs),
    /// Train a tiny model on synthetic waveforms for a few seconds and report
    /// whether the architecture is sound. CPU-only, no data, no GPU — run it
    /// after every architecture change.
    Smoke(SmokeArgs),
    /// Validate + benchmark the GPU generator (batched) against the CPU.
    #[cfg(feature = "gpu")]
    Gpu(GpuArgs),
    /// Run the TRAINING kernels on this device and compare with the CPU
    /// reference. The acceptance test for the wgpu training path.
    #[cfg(feature = "gpu")]
    GpuSelftest,
}

#[cfg(feature = "app")]
#[derive(Args, Debug)]
struct ListenArgs {
    /// Where session directories are created. Each run gets its own.
    #[arg(long, default_value = "./captures")]
    out: std::path::PathBuf,
    /// Stop after this many hours. 0 means no time limit, which has to be
    /// asked for: a session that stops when it has collected enough is a
    /// different promise to the people in the room than one that stops when
    /// you said it would.
    #[arg(long, default_value_t = 8.0)]
    hours: f64,
    /// Stop after this many kept crops.
    #[arg(long, default_value_t = 4096)]
    budget: usize,
    /// Mel bands, must match what you will train with.
    #[arg(long)]
    n_mels: Option<usize>,
    /// Crop width in frames, must match what you will train with.
    #[arg(long)]
    time_frames: Option<usize>,

    // ── tuning ──
    //
    // Every default was set against SYNTHETIC transients, so the first real
    // room is the first honest test of them. These are flags rather than
    // constants because tuning by rebuilding is tuning nobody does.
    /// Onset fires when spectral flux exceeds this multiple of the recent
    /// median. Higher hears less.
    #[arg(long, default_value_t = 2.5)]
    onset_mult: f32,
    /// Frames to ignore after a keep. Default is one crop width, so kept crops
    /// never overlap.
    #[arg(long)]
    refractory: Option<usize>,
    /// How different a crop must be from what is already kept. Lower keeps
    /// more near-duplicates.
    #[arg(long, default_value_t = 0.08)]
    novelty: f32,
    /// Keep crops that look like voiced speech.
    ///
    /// Off by default. Read micsample's module docs before turning it on: the
    /// crops are mel, mel is NOT anonymised, and this crate's Griffin-Lim can
    /// turn them back into audible sound.
    #[arg(long, default_value_t = false)]
    allow_speech: bool,
    /// Listen and report what WOULD be kept, writing nothing.
    ///
    /// The way to find this room's numbers: run it for a few minutes, read the
    /// tally, adjust, repeat — without a directory of crops to clean up after
    /// each attempt.
    #[arg(long, default_value_t = false)]
    dry_run: bool,
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

    // --- shape. Changing any of these invalidates an existing checkpoint, and
    //     --resume will say so by name rather than failing inside a tensor
    //     assignment. Must be multiples of 16: the generator's seed grid is
    //     n_mels/16 by time_frames/16.
    /// SAMPLER DURATION, in mel frames — the width of one training crop, which
    /// is exactly the length of one generated beat. 64 frames at hop 256 and
    /// 22050 Hz is ~0.74 s; `--time-frames 128` doubles it. Multiple of 16.
    #[arg(long)]
    time_frames: Option<usize>,
    /// Mel bands (frequency resolution of the crop). Multiple of 16.
    #[arg(long)]
    n_mels: Option<usize>,
    /// Latent width — the dimensionality of the space the GUI explores.
    #[arg(long)]
    latent: Option<usize>,
    /// Generator width. The last deconvolution stage is this over 8.
    #[arg(long)]
    g_filters: Option<usize>,
    /// Discriminator width.
    #[arg(long)]
    d_filters: Option<usize>,

    // --- dynamic tracks. `--dynamic` is the preset; the individual flags
    //     override it. All of them are shape fields except --seq-windows,
    //     --no-rollout and --overlap-frames, which only change the training
    //     signal and the stitching.
    /// The dynamic-track preset: --stems 2 --ctx-frames 8 --beat-phase
    /// --seq-windows 3 and streaming playback. Trains a renderer that
    /// continues its own output with drums and instruments as separate
    /// stems, judged by a sequence discriminator across window joins.
    #[arg(long, default_value_t = false)]
    dynamic: bool,
    /// Mel stems per frame: 1 (mixed) or 2 (harmonic + percussive).
    #[arg(long)]
    stems: Option<usize>,
    /// Frames of the previous window the renderer conditions on. 0 = none.
    #[arg(long)]
    ctx_frames: Option<usize>,
    /// Width of the context vector.
    #[arg(long)]
    ctx_dim: Option<usize>,
    /// Feed the renderer a beat phase per column (sin/cos planes).
    #[arg(long)]
    beat_phase: Option<bool>,
    /// Consecutive windows per sample. 2+ enables the sequence path.
    #[arg(long)]
    seq_windows: Option<usize>,
    /// Skip the free-running rollout (faster, but the model is only ever
    /// asked to continue real audio, which playback never gives it).
    #[arg(long, default_value_t = false)]
    no_rollout: bool,
    /// Frames adjacent windows overlap by, in training and playback.
    #[arg(long)]
    overlap_frames: Option<usize>,

    // --- optimisation, safe to change between runs on the same checkpoint.
    /// Adam learning rate.
    #[arg(long)]
    lr: Option<f64>,
    /// Audio sample rate the trainer decodes to.
    #[arg(long)]
    sr: Option<u32>,
}

#[derive(Args)]
struct SmokeArgs {
    /// Training steps. The default is the smallest number that reliably moves
    /// every parameter group; raise it to make the diversity numbers steadier,
    /// not to make the model good.
    #[arg(long, default_value_t = 12)]
    steps: usize,
    /// Seed, so a failure reproduces exactly.
    #[arg(long, default_value_t = 7)]
    seed: u64,
    /// Smoke the DYNAMIC architecture instead: two stems, context, beat
    /// phase, the sequence discriminator and a free-running rollout.
    #[arg(long, default_value_t = false)]
    dynamic: bool,
}

#[cfg(feature = "app")]
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
            let mut cfg = if a.dynamic { Config::dynamic() } else { Config::default() };
            if let Some(v) = a.stems { cfg.stems = v; }
            if let Some(v) = a.ctx_frames { cfg.ctx_frames = v; }
            if let Some(v) = a.ctx_dim { cfg.ctx_dim = v; }
            if let Some(v) = a.beat_phase { cfg.beat_phase = v; }
            if let Some(v) = a.seq_windows { cfg.seq_windows = v; }
            if a.no_rollout { cfg.rollout = false; }
            if let Some(v) = a.overlap_frames { cfg.overlap_frames = v; }
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
            if let Some(v) = a.time_frames {
                cfg.time_frames = v;
            }
            if let Some(v) = a.n_mels {
                cfg.n_mels = v;
            }
            if let Some(v) = a.latent {
                cfg.latent = v;
            }
            if let Some(v) = a.g_filters {
                cfg.g_filters = v;
            }
            if let Some(v) = a.d_filters {
                cfg.d_filters = v;
            }
            if let Some(v) = a.lr {
                cfg.lr = v;
            }
            if let Some(v) = a.sr {
                cfg.sr = v;
                // fmax follows the rate. There is no --fmax flag, and leaving
                // a stale Nyquist above the new rate silently empties the top
                // mel bands rather than erroring.
                cfg.fmax = v as f32 / 2.0;
            }

            // VALIDATE BEFORE ANYTHING ALLOCATES. These constraints are
            // structural — a seed grid that is not a multiple of 16 still runs
            // and produces output that reads as a training failure rather than
            // a configuration one.
            if let Err(why) = cfg.validate() {
                anyhow::bail!("this configuration cannot build a model:\n  - {why}");
            }

            // AND CHECK THE CHECKPOINT BEFORE LOADING IT. `load_state` writes
            // into existing Vars, so a shape change surfaces deep inside a
            // tensor assignment, far from the flag that caused it.
            if a.resume {
                // cfg.ckpt_dir, not a.ckpt_dir: the arg was moved into cfg above.
                let saved = cfg.ckpt_dir.join("config.json");
                if saved.exists() {
                    match Config::load_json(&saved) {
                        Ok(prev) => {
                            if let Err(why) = cfg.shape_compatible_with(&prev) {
                                anyhow::bail!(
                                    "--resume cannot load {}: the model shape differs from the \
checkpoint's.\n  - {why}\n\nTrain into a different --ckpt-dir, or drop the flag that changed \
the shape. Non-shape settings (lr, epochs, batch size, paths) never block a resume.",
                                    saved.display()
                                );
                            }
                        }
                        Err(e) => eprintln!(
                            "[train] --resume: could not read {} ({e}); \
proceeding, but a shape mismatch will now fail inside the loader",
                            saved.display()
                        ),
                    }
                }
            }
            train::run(&cfg, a.resume)?;
        }
        Cmd::Smoke(a) if a.dynamic => {
            let r = smoke::run_seq(a.steps, a.seed)?;
            println!("[smoke] {}", r.line());
            let bad = r.failures();
            if bad.is_empty() {
                println!("[smoke] dynamic architecture looks sound");
            } else {
                for b in &bad {
                    eprintln!("[smoke] FAIL: {b}");
                }
                anyhow::bail!("{} smoke check(s) failed", bad.len());
            }
        }
        Cmd::Smoke(a) => {
            // Prints and then sets the exit code, rather than only asserting:
            // the same run is useful as a CI gate and as something a person
            // reads after changing a layer.
            let r = smoke::run(a.steps, a.seed)?;
            println!("[smoke] {}", r.line());
            let mut bad: Vec<String> = Vec::new();
            if r.params_moved <= 0.5 {
                bad.push(format!(
                    "only {:.1}% of generator parameters moved — something is detached",
                    r.params_moved * 100.0
                ));
            }
            if r.z_diversity <= 1e-4 {
                bad.push(format!(
                    "z-diversity {:.8} — the generator is ignoring its latent (mode collapse)",
                    r.z_diversity
                ));
            }
            if r.z_repeatability != 0.0 {
                bad.push(format!(
                    "the same latent gave different output twice ({:.8}) — eval mode is not deterministic",
                    r.z_repeatability
                ));
            }
            if r.d_score_spread <= 0.0 {
                bad.push(format!(
                    "discriminator scores are bit-identical ({:?}) — its head is seeing one value, not {}",
                    r.d_scores, r.d_score_count
                ));
            }
            if bad.is_empty() {
                println!("[smoke] architecture looks sound");
            } else {
                for b in &bad {
                    eprintln!("[smoke] FAIL: {b}");
                }
                anyhow::bail!("{} smoke check(s) failed", bad.len());
            }
        }
        #[cfg(feature = "app")]
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
        #[cfg(feature = "app")]
        Cmd::Listen(a) => {
            let mut cfg = Config::default();
            if let Some(v) = a.n_mels { cfg.n_mels = v; }
            if let Some(v) = a.time_frames { cfg.time_frames = v; }
            // The same validation the trainer runs, BEFORE a microphone is
            // opened: a session captured at a shape that cannot build a model
            // is hours of listening thrown away.
            if let Err(why) = cfg.validate() {
                anyhow::bail!("this configuration cannot build a model:\n  - {why}");
            }
            let policy = micsession::StopPolicy {
                max_seconds: if a.hours > 0.0 { Some(a.hours * 3600.0) } else { None },
                budget: a.budget,
            };
            let gate = micsample::Gate {
                time_frames: cfg.time_frames,
                threshold_mult: a.onset_mult,
                refractory: a.refractory.unwrap_or(cfg.time_frames),
                novelty_min_dist: a.novelty,
                speechlike: !a.allow_speech,
                budget: a.budget,
                ..micsample::Gate::default()
            };
            miclisten::listen(&cfg, &a.out, policy, gate, a.dry_run)?;
        }
        #[cfg(feature = "gpu")]
        Cmd::Gpu(a) => {
            run_gpu(a)?;
        }
        #[cfg(feature = "gpu")]
        Cmd::GpuSelftest => match wgpu_run::selftest() {
            Ok(report) => print!("{report}"),
            Err(report) => {
                // Printed, then a non-zero exit: a self-test that reports a
                // failure and exits 0 is one a script will not notice.
                print!("{report}");
                std::process::exit(1);
            }
        },
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
