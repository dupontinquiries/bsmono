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
        .with_n_mels(env("BS_NMELS", 128))
        .with_time_frames(env("BS_TF", 256))
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
        other => {
            eprintln!("unknown command '{other}'. use: smoke | train | generate");
            std::process::exit(2);
        }
    }
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

fn train(cfg: &GanConfig, device: &WgpuDevice, args: &[String]) {
    let iters: usize = arg(args, "--iters").unwrap_or(20000);
    let batch: usize = arg(args, "--batch").unwrap_or(32);
    let ckpt_every: usize = arg(args, "--ckpt-every").unwrap_or(2000);
    let ckpt_dir: String = arg(args, "--ckpt-dir").unwrap_or_else(|| "checkpoints_rs".into());
    let cache_dir: String = arg(args, "--cache").unwrap_or_else(|| "cache".into());
    let resume: Option<usize> = arg(args, "--resume");
    let synthetic = args.iter().any(|a| a == "--synthetic");
    std::fs::create_dir_all(&ckpt_dir).ok();

    // Real data unless --synthetic or the cache is absent.
    let mut cache: Option<MelCache> = if synthetic {
        None
    } else if std::path::Path::new(&cache_dir).join("manifest.json").exists() {
        let c = MelCache::open(&cache_dir, 250, cfg.n_mels, cfg.time_frames);
        println!("Loaded cache '{}': {} valid tracks.", cache_dir, c.len());
        assert!(c.len() >= batch, "cache has fewer tracks than batch size");
        Some(c)
    } else {
        println!("No cache at '{cache_dir}'; using synthetic data.");
        None
    };

    let mut g = cfg.init_generator::<AB>(device);
    let mut d = cfg.init_discriminator::<AB>(device);
    let mut opt_g = AdamConfig::new().with_beta_1(0.5).init();
    let mut opt_d = AdamConfig::new().with_beta_1(0.5).init();
    let lr = 2e-4;

    let mut start = 0;
    if let Some(step) = resume {
        g = load_module(g, &format!("{ckpt_dir}/gen_{step}"), device);
        d = load_module(d, &format!("{ckpt_dir}/disc_{step}"), device);
        start = step + 1;
        println!("Resumed generator+discriminator from step {step}.");
    }

    println!("Training {iters} iters, batch {batch}, on GPU (Vulkan). Ctrl-C safe.");
    let t0 = Instant::now();

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

        // Discriminator
        let fake = g.forward(z);
        let d_real_logits = d.forward(real);
        let d_fake_logits = d.forward(fake.clone().detach());
        let loss_d = bce_with_logits(d_real_logits.clone(), 0.9)
            + bce_with_logits(d_fake_logits.clone(), 0.0);
        let grads = loss_d.backward();
        let grads = GradientsParams::from_grads(grads, &d);
        d = opt_d.step(lr, d, grads);

        // Generator
        let g_logits = d.forward(fake);
        let loss_g = bce_with_logits(g_logits, 0.9);
        let grads = loss_g.backward();
        let grads = GradientsParams::from_grads(grads, &g);
        g = opt_g.step(lr, g, grads);

        if step % 20 == 0 || step + 1 == iters {
            let d_real = sigmoid(d_real_logits).mean().into_scalar();
            let d_fake = sigmoid(d_fake_logits).mean().into_scalar();
            let ms = t0.elapsed().as_millis() as f64 / (step - start + 1) as f64;
            println!(
                "[{step}/{iters}] lossD: {:.4} lossG: {:.4} | D(real): {:.2} D(fake): {:.2} ({ms:.0} ms/it)",
                loss_d.into_scalar(), loss_g.into_scalar(), d_real, d_fake,
            );
        }

        if step > 0 && step % ckpt_every == 0 {
            save_module(&g, &format!("{ckpt_dir}/gen_{step}"));
            save_module(&d, &format!("{ckpt_dir}/disc_{step}"));
            dump_mel(&g.valid(), cfg, device, &format!("{ckpt_dir}/sample_{step}"));
            println!("  checkpoint @ {step}: gen_{step}.mpk, disc_{step}.mpk, sample_{step}_0.bin");
        }
    }
    save_module(&g, &format!("{ckpt_dir}/gen_{iters}"));
    save_module(&d, &format!("{ckpt_dir}/disc_{iters}"));
    println!("Done in {:.1}s. Final: {ckpt_dir}/gen_{iters}.mpk", t0.elapsed().as_secs_f64());
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
