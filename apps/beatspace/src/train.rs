//! Training loop — a faithful DCGAN schedule matching the v6 Python:
//!   Adam(betas=(0.5, 0.999)), lr 2e-4, BCEWithLogits, one-sided label
//!   smoothing (real target = 0.9). D step on real + fake.detach(); G step on
//!   fake. Checkpoints (generator + discriminator + config.json) are written to
//!   `cfg.ckpt_dir` every `cfg.ckpt_every` epochs, with a rolling `*_latest`.

use anyhow::{Context, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::{AdamW, Optimizer, ParamsAdamW};
use std::fs;
use std::path::Path;

use crate::config::Config;
use crate::dataset;
use crate::model::{
    bce_with_logits, load_discriminator, load_generator, save_discriminator, save_generator,
    Discriminator, Generator,
};

pub fn run(cfg: &Config, resume: bool) -> Result<()> {
    let dev = Device::cuda_if_available(0).unwrap_or(Device::Cpu);
    println!("[train] device: {:?}", dev);

    fs::create_dir_all(&cfg.ckpt_dir)?;
    cfg.save_json(&cfg.ckpt_dir.join("config.json"))
        .context("save config.json")?;

    // --- data ---
    let ds = dataset::build_or_load(cfg)?;
    println!("[train] dataset ready: {} clips", ds.len());

    // --- models ---
    let mut g = Generator::new(cfg, &dev)?;
    let mut d = Discriminator::new(cfg, &dev)?;

    if resume {
        let gp = cfg.ckpt_dir.join("gen_latest.safetensors");
        let dp = cfg.ckpt_dir.join("disc_latest.safetensors");
        if gp.exists() && dp.exists() {
            load_generator(&mut g, &gp, &dev)?;
            load_discriminator(&mut d, &dp, &dev)?;
            println!("[train] resumed from {gp:?}");
        } else {
            println!("[train] --resume set but no latest checkpoint; starting fresh");
        }
    }

    // --- optimizers (two, one per network) ---
    let adam = |vars| -> Result<AdamW> {
        Ok(AdamW::new(
            vars,
            ParamsAdamW {
                lr: cfg.lr,
                beta1: cfg.beta1,
                beta2: 0.999,
                eps: 1e-8,
                weight_decay: 0.0,
            },
        )?)
    };
    let mut opt_g = adam(g.trainable_vars())?;
    let mut opt_d = adam(d.trainable_vars())?;

    let b = cfg.batch_size;
    let latent = cfg.latent;
    let smooth = cfg.label_smooth as f64;

    // fixed targets reused each step
    let ones = Tensor::ones(b, DType::F32, &dev)?;
    let zeros = Tensor::zeros(b, DType::F32, &dev)?;
    let real_target = ones.affine(smooth, 0.0)?; // 0.9
    let fake_target = zeros.clone();
    let gen_target = ones.clone();

    for epoch in 0..cfg.epochs {
        let mut d_running = 0f32;
        let mut g_running = 0f32;

        for _ in 0..cfg.steps_per_epoch {
            // real batch
            let real = ds.sample_batch(b, &dev)?; // (B,1,128,256)

            // latent noise
            let z = Tensor::randn(0f32, 1f32, (b, latent), &dev)?;
            let fake = g.forward_train(&z)?; // (B,1,128,256)

            // ---- D step ----
            let real_logits = d.forward(&real, true)?;
            let fake_logits_d = d.forward(&fake.detach(), true)?;
            let d_loss = (bce_with_logits(&real_logits, &real_target)?
                + bce_with_logits(&fake_logits_d, &fake_target)?)?;
            opt_d.backward_step(&d_loss)?;

            // ---- G step ----
            let fake_logits_g = d.forward(&fake, true)?;
            let g_loss = bce_with_logits(&fake_logits_g, &gen_target)?;
            opt_g.backward_step(&g_loss)?;

            d_running += d_loss.to_scalar::<f32>()?;
            g_running += g_loss.to_scalar::<f32>()?;
        }

        let n = cfg.steps_per_epoch as f32;
        println!(
            "[train] epoch {:>5}/{}  d_loss {:.4}  g_loss {:.4}",
            epoch + 1,
            cfg.epochs,
            d_running / n,
            g_running / n
        );

        let last = epoch + 1 == cfg.epochs;
        if (epoch + 1) % cfg.ckpt_every == 0 || last {
            checkpoint(&g, &d, cfg, epoch + 1)?;
        }
    }

    println!("[train] done.");
    Ok(())
}

fn checkpoint(g: &Generator, d: &Discriminator, cfg: &Config, epoch: usize) -> Result<()> {
    let dir: &Path = &cfg.ckpt_dir;
    let gp = dir.join(format!("gen_{epoch:06}.safetensors"));
    let dp = dir.join(format!("disc_{epoch:06}.safetensors"));
    save_generator(g, &gp)?;
    save_discriminator(d, &dp)?;
    // rolling "latest" so the GUI can always find the newest weights
    save_generator(g, &dir.join("gen_latest.safetensors"))?;
    save_discriminator(d, &dir.join("disc_latest.safetensors"))?;
    println!("[train] checkpoint @ epoch {epoch} -> {gp:?}");
    Ok(())
}
