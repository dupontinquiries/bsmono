// beatspace-gpu -- end-to-end GPU training of the kBM3 GAN on Apple Silicon via
// Vulkan (Mesa "Honeykrisp"), reading the Python pipeline's SQLite mel cache and
// writing Burn checkpoints. No CUDA; PyTorch has no backend for this GPU.
//
//   WGPU_BACKEND=vulkan cargo run --release -- train --cache cache --iters 20000
//   WGPU_BACKEND=vulkan cargo run --release -- train --resume 6000
//   WGPU_BACKEND=vulkan cargo run --release -- generate --gen checkpoints_rs/gen_6000 --n 4
//
#![recursion_limit = "256"]

mod data;
mod model;
mod audio;

use std::io::Write;
use std::time::Instant;

use burn::backend::wgpu::WgpuDevice;
use burn::backend::{Autodiff, Wgpu};
use burn::module::{AutodiffModule, Module};
use burn::optim::{AdamConfig, GradientsParams, Optimizer};
use burn::record::{FullPrecisionSettings, NamedMpkFileRecorder};
use burn::tensor::activation::sigmoid;
use burn::tensor::{Distribution, Tensor};

use data::MelCache;
use model::{bce_with_logits, GanConfig, Generator};

type BE = Wgpu<f32, i32>;
type AB = Autodiff<BE>;

fn recorder() -> NamedMpkFileRecorder<FullPrecisionSettings> {
    NamedMpkFileRecorder::<FullPrecisionSettings>::new()
}
fn save_module<B: burn::tensor::backend::Backend, M: Module<B>>(m: &M, path: &str) {
    m.clone().save_file(path, &recorder()).expect("checkpoint save failed");
}
fn load_module<B: burn::tensor::backend::Backend, M: Module<B>>(m: M, path: &str, dev: &B::Device) -> M {
    m.load_file(path, &recorder(), dev).expect("checkpoint load failed")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("smoke");

    let device = WgpuDevice::default();
    println!("Backend: Wgpu (Vulkan on Asahi). Device: {device:?}");
    let env = |k: &str, d: usize| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let cfg = GanConfig::new()
        .with_n_mels(env("BS_NMELS", 256))
        .with_time_frames(env("BS_TF", 560))
        .with_g_filters(env("BS_GF", 256))
        .with_d_filters(env("BS_DF", 64));
    println!(
        "Config: {}x{} mels, g_filters={}, d_filters={}",
        cfg.n_mels, cfg.time_frames, cfg.g_filters, cfg.d_filters
    );

    match cmd {
        "smoke" => smoke(&cfg, &device),
        "train" => train(&cfg, &device, &args),
        "generate" => generate(&cfg, &device, &args),
        "build-cache" => build_cache_cmd(&cfg, &args),
        "bench" => bench(&cfg, &device, &args),
        other => {
            eprintln!("unknown command '{other}'. use: smoke | train | generate | build-cache | bench");
            std::process::exit(2);
        }
    }
}

/// Build the sharded SQLite mel cache from a folder of audio, in the exact
/// format both this Rust trainer and the Python pipeline read. Pure CPU work
/// (decode + STFT + mel), no GPU needed.
fn build_cache_cmd(cfg: &GanConfig, args: &[String]) {
    let data_dir: String = arg(args, "--data").unwrap_or_else(|| {
        eprintln!("build-cache requires --data <folder of .opus/.wav>");
        std::process::exit(2);
    });
    let cache_dir: String = arg(args, "--cache").unwrap_or_else(|| "cache".into());
    let num_shards: usize = arg(args, "--num-shards").unwrap_or(32);
    // These MUST match the Python audio config (gan-zai-v5.3.py / vocoder.py):
    // sr=48000, n_fft=4096, hop=512, db range [-80, 40]. n_mels/time_frames
    // come from the model config so a built cache matches what training reads.
    let sr: u32 = arg(args, "--sr").unwrap_or(48000);
    let n_fft: usize = arg(args, "--n-fft").unwrap_or(4096);
    let hop: usize = arg(args, "--hop").unwrap_or(512);
    // Default to int8 (~4.6x smaller cache, negligible quality loss for GAN
    // training). --f32-cache writes raw f32 blobs, byte-compatible with the
    // Python pipeline's caches.
    let int8 = !args.iter().any(|a| a == "--f32-cache");
    data::build_cache(&data_dir, &cache_dir, num_shards, sr, n_fft, hop, cfg.n_mels, cfg.time_frames, -80.0, 40.0, int8);
}

fn smoke(cfg: &GanConfig, device: &WgpuDevice) {
    let g = cfg.init_generator::<BE>(device);
    let d = cfg.init_discriminator::<BE>(device);
    let z = Tensor::<BE, 2>::random([4, cfg.latent], Distribution::Normal(0.0, 1.0), device);
    let t0 = Instant::now();
    let fake = g.forward(z);
    let logits = d.forward(fake.clone());
    let _ = logits.to_data();
    println!("G(z) -> {:?}", fake.dims());
    println!("D(G(z)) -> {:?}", logits.dims());
    println!("first forward (incl. kernel compile): {} ms", t0.elapsed().as_millis());
}

/// Pure throughput benchmark: runs the REAL training step (D fwd+bwd+step,
/// then G fwd+bwd+step) on synthetic data, so numbers reflect actual
/// training compute -- not a toy microbenchmark. Does explicit warmup
/// iterations to absorb one-time SPIR-V kernel compilation, then times a
/// measurement window and reports min / median / p95 / mean ms/it. This is
/// the honest way to compare GPUs (e.g. M4 Max vs M2 Ultra): a single
/// running-average number hides the warmup and the tail.
///
///   WGPU_BACKEND=vulkan cargo run --release --bin beatspace-gpu -- \
///       bench --batch 8 --warmup 10 --measure 50
///
/// Sweep batch sizes to find where throughput saturates (the memory-bandwidth
/// knee -- expected to differ between M4 Max and the higher-bandwidth M2 Ultra):
///   for b in 1 2 4 8 16 32; do ... bench --batch $b; done
fn bench(cfg: &GanConfig, device: &WgpuDevice, args: &[String]) {
    let batch: usize = arg(args, "--batch").unwrap_or(8);
    let warmup: usize = arg(args, "--warmup").unwrap_or(10);
    let measure: usize = arg(args, "--measure").unwrap_or(50);

    println!(
        "bench: {}x{} mels, latent {}, g_filters {}, d_filters {}, batch {batch}",
        cfg.n_mels, cfg.time_frames, cfg.latent, cfg.g_filters, cfg.d_filters
    );
    println!("warmup {warmup} iters (absorbs kernel compilation), then timing {measure} iters...");

    let mut g = cfg.init_generator::<AB>(device);
    let mut d = cfg.init_discriminator::<AB>(device);
    let mut opt_g = AdamConfig::new().with_beta_1(0.5).init();
    let mut opt_d = AdamConfig::new().with_beta_1(0.5).init();
    let lr = 2e-4;

    // One real training step, as a macro so we don't have to name the
    // optimizer's (complex, generic) type in a closure signature.
    macro_rules! step {
        () => {{
            let real = Tensor::<AB, 4>::random(
                [batch, 1, cfg.n_mels, cfg.time_frames],
                Distribution::Uniform(-1.0, 1.0),
                device,
            );
            let z = Tensor::<AB, 2>::random([batch, cfg.latent], Distribution::Normal(0.0, 1.0), device);

            let fake = g.forward(z);
            let d_real = d.forward(real);
            let d_fake = d.forward(fake.clone().detach());
            let loss_d = bce_with_logits(d_real, 0.9) + bce_with_logits(d_fake.clone(), 0.0);
            let grads = GradientsParams::from_grads(loss_d.backward(), &d);
            d = opt_d.step(lr, d.clone(), grads);

            let g_logits = d.forward(fake);
            let loss_g = bce_with_logits(g_logits, 0.9);
            let grads = GradientsParams::from_grads(loss_g.backward(), &g);
            g = opt_g.step(lr, g.clone(), grads);
            // force a real sync so we measure completed work
            let _ = loss_g.into_scalar();
        }};
    }

    for _ in 0..warmup {
        step!();
    }

    // Component profiling: time each phase separately (with a forced sync
    // after each) to see whether the generator, the discriminator, or the
    // backward/optimizer step dominates. `--profile` runs this instead of the
    // aggregate timing.
    if args.iter().any(|a| a == "--profile") {
        let sync = |t: &Tensor<AB, 1>| { let _ = t.clone().mean().into_scalar(); };
        let mut acc = [0f64; 5];
        let reps = measure.min(10).max(3);
        for _ in 0..reps {
            let real = Tensor::<AB, 4>::random([batch, 1, cfg.n_mels, cfg.time_frames], Distribution::Uniform(-1.0, 1.0), device);
            let z = Tensor::<AB, 2>::random([batch, cfg.latent], Distribution::Normal(0.0, 1.0), device);

            let t = Instant::now();
            let fake = g.forward(z);
            { let _ = fake.clone().flatten::<1>(0, 3).mean().into_scalar(); }
            acc[0] += t.elapsed().as_secs_f64() * 1000.0;

            let t = Instant::now();
            let d_real = d.forward(real);
            let d_fake = d.forward(fake.clone().detach());
            sync(&d_real); sync(&d_fake);
            acc[1] += t.elapsed().as_secs_f64() * 1000.0;

            let t = Instant::now();
            let loss_d = bce_with_logits(d_real, 0.9) + bce_with_logits(d_fake, 0.0);
            let grads = GradientsParams::from_grads(loss_d.backward(), &d);
            d = opt_d.step(lr, d.clone(), grads);
            acc[2] += t.elapsed().as_secs_f64() * 1000.0;

            let t = Instant::now();
            let fake2 = g.forward(Tensor::<AB, 2>::random([batch, cfg.latent], Distribution::Normal(0.0, 1.0), device));
            let g_logits = d.forward(fake2);
            let loss_g = bce_with_logits(g_logits, 0.9);
            acc[3] += t.elapsed().as_secs_f64() * 1000.0;

            let t = Instant::now();
            let grads = GradientsParams::from_grads(loss_g.backward(), &g);
            g = opt_g.step(lr, g.clone(), grads);
            let _ = loss_g.into_scalar();
            acc[4] += t.elapsed().as_secs_f64() * 1000.0;
        }
        let r = reps as f64;
        println!("\ncomponent profile (ms/it, avg over {reps}):");
        println!("  G forward:            {:.1}", acc[0]/r);
        println!("  D forward (real+fake):{:.1}", acc[1]/r);
        println!("  D backward + step:    {:.1}", acc[2]/r);
        println!("  G fwd + D fwd (for G):{:.1}", acc[3]/r);
        println!("  G backward + step:    {:.1}", acc[4]/r);
        println!("  (sum):                {:.1}", acc.iter().sum::<f64>()/r);
        return;
    }

    let mut times = Vec::with_capacity(measure);
    for _ in 0..measure {
        let t = Instant::now();
        step!();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let min = times[0];
    let median = times[times.len() / 2];
    let p95 = times[(times.len() as f64 * 0.95) as usize];
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    let ips = 1000.0 / median;

    println!("\n  ms/it   min {min:.1}   median {median:.1}   p95 {p95:.1}   mean {mean:.1}");
    println!("  throughput (median): {ips:.2} it/s   ({:.1} samples/s at batch {batch})", ips * batch as f64);
    println!("  {}", if median < 1000.0 { "-> sub-1000ms/it" } else { "-> above 1000ms/it" });
}

fn train(cfg: &GanConfig, device: &WgpuDevice, args: &[String]) {
    let iters: usize = arg(args, "--iters").unwrap_or(20000);
    let batch: usize = arg(args, "--batch").unwrap_or(32);
    let ckpt_every: usize = arg(args, "--ckpt-every").unwrap_or(2000);
    let log_every: usize = arg(args, "--log-every").unwrap_or(50);
    let sample_every: usize = arg(args, "--sample-every").unwrap_or(ckpt_every);
    let ckpt_dir: String = arg(args, "--ckpt-dir").unwrap_or_else(|| "checkpoints_rs".into());
    let cache_dir: String = arg(args, "--cache").unwrap_or_else(|| "cache".into());
    let num_shards: usize = arg(args, "--num-shards").unwrap_or(32);
    let lr: f64 = arg(args, "--lr").unwrap_or(2e-4);
    let beta1: f32 = arg(args, "--beta1").unwrap_or(0.5);
    let label_smooth: f32 = arg(args, "--label-smooth").unwrap_or(0.9);
    let resume: Option<usize> = arg(args, "--resume");
    let synthetic = args.iter().any(|a| a == "--synthetic");
    let no_ema = args.iter().any(|a| a == "--no-ema");
    let ema_decay: f32 = arg(args, "--ema-decay").unwrap_or(0.999);
    // How often (in iters) to run the EMA blend. The blend touches ~8 tensors
    // (a handful of kernel dispatches) every time; on backends with high
    // per-dispatch overhead (e.g. Asahi/Mesa) doing it every step adds fixed
    // cost. Running it every N steps with a compounded decay (decay^N) is
    // numerically almost identical for the usual decay~0.999 and cuts that
    // overhead ~N-fold. Default 1 (every step) preserves exact old behaviour.
    let ema_every: usize = arg(args, "--ema-every").unwrap_or(1).max(1);
    let noise_sigma0: f32 = arg(args, "--noise-sigma").unwrap_or(0.1);
    let noise_iters: usize = arg(args, "--noise-iters").unwrap_or(iters / 5).max(1);
    std::fs::create_dir_all(&ckpt_dir).ok();

    // Real data unless --synthetic or the cache is absent.
    let mut cache: Option<MelCache> = if synthetic {
        None
    } else if std::path::Path::new(&cache_dir).join("manifest.json").exists() {
        let c = MelCache::open(&cache_dir, num_shards, cfg.n_mels, cfg.time_frames);
        println!("Loaded cache '{}': {} valid tracks.", cache_dir, c.len());
        if c.len() == 0 {
            eprintln!(
                "ERROR: the cache has 0 valid tracks -- nothing to train on. If the shards \
                 clearly contain data (check `du -sh {cache_dir}`), see the orphaned-rows \
                 warning above; otherwise (re)build the cache with \
                 `build-cache --data <dir> --cache {cache_dir}`."
            );
            std::process::exit(1);
        }
        if c.len() < batch {
            // next_batch samples WITH replacement (random track + random crop
            // per draw), so batch > track count is not an error -- it just
            // means more repetition. Warn, since very few tracks makes for a
            // degenerate GAN, but let it run.
            eprintln!(
                "NOTE: only {} valid track(s), fewer than batch size {}. Training will \
                 sample with replacement (heavy repetition -- fine for a quick smoke test, \
                 but a real run wants many more tracks).",
                c.len(), batch
            );
        }
        Some(c)
    } else {
        println!("No cache at '{cache_dir}'; using synthetic data.");
        None
    };

    let mut g = cfg.init_generator::<AB>(device);
    let mut d = cfg.init_discriminator::<AB>(device);
    let mut opt_g = AdamConfig::new().with_beta_1(beta1).init();
    let mut opt_d = AdamConfig::new().with_beta_1(beta1).init();

    // Generator EMA: a shadow copy sampled/checkpointed instead of the raw
    // (noisier, step-to-step-jittery) generator -- matches gan-zai-v5.3.py.
    let mut g_ema = if no_ema { None } else { Some(cfg.init_generator::<AB>(device)) };
    if let Some(ref mut ema) = g_ema {
        *ema = ema.ema_blend(&g, 0.0); // seed EMA at the same init as the live generator
    }

    let mut start = 0;
    if let Some(step) = resume {
        g = load_module(g, &format!("{ckpt_dir}/gen_{step}"), device);
        d = load_module(d, &format!("{ckpt_dir}/disc_{step}"), device);
        if let Some(ref mut ema) = g_ema {
            let path = format!("{ckpt_dir}/gen_ema_{step}");
            if std::path::Path::new(&format!("{path}.mpk")).exists() {
                *ema = load_module(ema.clone(), &path, device);
            } else {
                *ema = ema.ema_blend(&g, 0.0); // no EMA checkpoint found; reseed from live
            }
        }
        start = step + 1;
        println!("Resumed generator+discriminator from step {step}.");
    }

    let csv_path = format!("{ckpt_dir}/train_log.csv");
    if !std::path::Path::new(&csv_path).exists() {
        std::fs::write(&csv_path, "step,lossD,lossG,D_real,D_fake,noise_sigma,ms_per_it\n").ok();
    }

    println!(
        "Training {iters} iters, batch {batch}, on GPU (Vulkan). EMA: {}. Ctrl-C safe.",
        if g_ema.is_some() { format!("on (decay {ema_decay})") } else { "off".into() }
    );
    let mut t_window = Instant::now();
    let t0_total = Instant::now();
    let mut its_since_log = 0usize;

    for step in start..iters {
        let real = match cache.as_mut() {
            Some(c) => c.next_batch::<AB>(batch, device),
            None => Tensor::<AB, 4>::random(
                [batch, 1, cfg.n_mels, cfg.time_frames],
                Distribution::Uniform(-1.0, 1.0),
                device,
            ),
        };
        let z = Tensor::<AB, 2>::random([batch, cfg.latent], Distribution::Normal(0.0, 1.0), device);

        // Instance noise: linearly decays to 0 over noise_iters, added to
        // both real and fake batches before they reach the discriminator.
        // Widens the support overlap early in training -- matches
        // gan-zai-v5.3.py's _instance_noise_sigma.
        let sigma = if step < noise_iters {
            noise_sigma0 * (1.0 - step as f32 / noise_iters as f32)
        } else {
            0.0
        };
        let noised = |x: Tensor<AB, 4>| -> Tensor<AB, 4> {
            if sigma > 0.0 {
                x.clone() + Tensor::<AB, 4>::random(x.shape(), Distribution::Normal(0.0, sigma as f64), device)
            } else {
                x
            }
        };

        // Discriminator
        let fake = g.forward(z);
        let d_real_logits = d.forward(noised(real));
        let d_fake_logits = d.forward(noised(fake.clone().detach()));
        let loss_d = bce_with_logits(d_real_logits.clone(), label_smooth)
            + bce_with_logits(d_fake_logits.clone(), 0.0);
        let grads = loss_d.backward();
        let grads = GradientsParams::from_grads(grads, &d);
        d = opt_d.step(lr, d, grads);

        // Generator
        let g_logits = d.forward(noised(fake));
        let loss_g = bce_with_logits(g_logits, label_smooth);
        let grads = loss_g.backward();
        let grads = GradientsParams::from_grads(grads, &g);
        g = opt_g.step(lr, g, grads);

        // EMA update, with an early-training warmup (tracks fast while
        // weights are moving fast), matching gan-zai-v5.3.py. Optionally run
        // only every `ema_every` steps (compounding the decay) to cut
        // per-step dispatch overhead on high-overhead backends.
        if let Some(ref mut ema) = g_ema {
            if step % ema_every == 0 {
                let base = ema_decay.min((1.0 + step as f32) / (10.0 + step as f32));
                // compound so N skipped steps ~= N single steps at `base`
                let decay = base.powi(ema_every as i32);
                *ema = ema.ema_blend(&g, decay);
            }
        }

        its_since_log += 1;
        if step % log_every == 0 || step + 1 == iters {
            let d_real = sigmoid(d_real_logits).mean().into_scalar();
            let d_fake = sigmoid(d_fake_logits).mean().into_scalar();
            let ms = t_window.elapsed().as_millis() as f64 / its_since_log as f64;
            t_window = Instant::now();
            its_since_log = 0;
            let (lossd, lossg) = (loss_d.into_scalar(), loss_g.into_scalar());
            println!(
                "[{step}/{iters}] lossD: {lossd:.4} lossG: {lossg:.4} | D(real): {d_real:.2} D(fake): {d_fake:.2} | noise: {sigma:.3} ({ms:.0} ms/it)",
            );
            use std::io::Write as _;
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&csv_path) {
                let _ = writeln!(f, "{step},{lossd:.4},{lossg:.4},{d_real:.4},{d_fake:.4},{sigma:.4},{ms:.1}");
            }
        }

        if step > 0 && step % ckpt_every == 0 {
            save_module(&g, &format!("{ckpt_dir}/gen_{step}"));
            save_module(&d, &format!("{ckpt_dir}/disc_{step}"));
            if let Some(ref ema) = g_ema {
                save_module(ema, &format!("{ckpt_dir}/gen_ema_{step}"));
            }
            println!("  checkpoint @ {step}: gen_{step}.mpk, disc_{step}.mpk");
        }
        if step > 0 && step % sample_every == 0 {
            let sample_g = g_ema.as_ref().unwrap_or(&g);
            dump_mel(&sample_g.valid(), cfg, device, &format!("{ckpt_dir}/sample_{step}"));
            println!("  sample @ {step}: sample_{step}_0.bin");
        }
    }
    save_module(&g, &format!("{ckpt_dir}/gen_{iters}"));
    save_module(&d, &format!("{ckpt_dir}/disc_{iters}"));
    if let Some(ref ema) = g_ema {
        save_module(ema, &format!("{ckpt_dir}/gen_ema_{iters}"));
    }
    println!("Done in {:.1}s. Final: {ckpt_dir}/gen_{iters}.mpk", t0_total.elapsed().as_secs_f64());
}

fn generate(cfg: &GanConfig, device: &WgpuDevice, args: &[String]) {
    let gen_path: String = arg(args, "--gen").expect("--gen <path/to/gen_STEP> required");
    let n: usize = arg(args, "--n").unwrap_or(4);
    let out: String = arg(args, "--out").unwrap_or_else(|| "samples_rs".into());
    std::fs::create_dir_all(&out).ok();
    let g = load_module(cfg.init_generator::<BE>(device), &gen_path, device);
    dump_mel(&g, cfg, device, &format!("{out}/mel"));
    let _ = n; // dump_mel emits a small batch; see below
    println!("Wrote mel tensors to {out}/. Convert to audio with tools/mel2audio.py.");
}

/// Generate a small batch and write each mel as a raw little-endian f32 file
/// `<'{prefix}_{i}_{H}x{W}.bin'>`. The Python side (mel_unit_to_audio) turns
/// these into wav/opus, so audio synthesis stays with the existing pipeline.
fn dump_mel(g: &Generator<BE>, cfg: &GanConfig, device: &WgpuDevice, prefix: &str) {
    let n = 2usize;
    let z = Tensor::<BE, 2>::random([n, cfg.latent], Distribution::Normal(0.0, 1.0), device);
    let mel = g.forward(z);
    let (h, w) = (cfg.n_mels, cfg.time_frames);
    let data: Vec<f32> = mel.into_data().to_vec().expect("tensor to vec");
    for i in 0..n {
        let slice = &data[i * h * w..(i + 1) * h * w];
        let path = format!("{prefix}_{i}_{h}x{w}.bin");
        let mut f = std::fs::File::create(&path).expect("create bin");
        f.write_all(bytemuck::cast_slice(slice)).expect("write bin");
    }
}

fn arg<T: std::str::FromStr>(args: &[String], key: &str) -> Option<T> {
    let i = args.iter().position(|a| a == key)?;
    args.get(i + 1)?.parse().ok()
}
