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
    // DEVICE, AND WHY.
    //
    // `--features gpu` does not affect training and never did: that feature
    // builds `src/gpu.rs`, a wgpu path for GENERATOR INFERENCE. Training runs
    // on candle, whose backends are CUDA and Metal — neither of which exists
    // on Asahi, where the Apple GPU is reached through Vulkan.
    //
    // So "device: Cpu" was accurate and read as a bug, because the flag that
    // was passed implied it should have been something else. Saying which
    // backend was looked for, and that the flag is inference-only, is the
    // difference between a limitation and a fault.
    let t_start = std::time::Instant::now();
    let _awake = StayAwake::new("beatspace is training");
    let dev = Device::cuda_if_available(0).unwrap_or(Device::Cpu);
    match &dev {
        Device::Cpu => {
            println!("[train] device: Cpu");
            #[cfg(feature = "gpu")]
            println!(
                "[train] note: --features gpu builds the wgpu GENERATOR path, not training. \
                 candle trains on CUDA or Metal; on Asahi the Apple GPU is Vulkan-only, so \
                 there is no candle backend for it and CPU is the whole menu."
            );
            #[cfg(not(feature = "gpu"))]
            println!("[train] note: no CUDA device found; candle has no Vulkan backend");
        }
        other => println!("[train] device: {other:?}"),
    }

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

        // Per-phase timing, because "97 s/epoch" is not an answer to WHY.
        // Nothing here was attributed, so the choice between a smaller batch, a
        // smaller model and a different conv path was a guess. Accumulated over
        // the epoch and printed with it — four Instant reads per step against a
        // step that takes over a second.
        let (mut t_sample, mut t_gfwd, mut t_dstep, mut t_gstep) = (0f64, 0f64, 0f64, 0f64);
        // This epoch's own clock. The run total cannot give a percentage: by
        // epoch 100 the accounted share of it would read as 1%.
        let t_epoch = std::time::Instant::now();

        for step in 0..cfg.steps_per_epoch {
            // STOP, CHECKED PER STEP rather than per epoch.
            //
            // The mechanism was complete on both sides and still read as
            // broken: the trainer polled only at epoch boundaries, so at 97
            // s/epoch the button did nothing for up to a minute and a half. A
            // stat every step costs nothing against a step that takes 1.5 s,
            // and the checkpoint below still runs — stopping mid-epoch loses
            // only the steps since the last one, not the epoch.
            if step > 0 && stop_requested(cfg) {
                println!("[train] stop requested mid-epoch at step {step}/{} — checkpointing",
                         cfg.steps_per_epoch);
                checkpoint(&g, &d, cfg, epoch)?;
                let _ = fs::remove_file(stop_path(cfg));
                return Ok(());
            }

            // real batch
            let t0 = std::time::Instant::now();
            let real = ds.sample_batch(b, &dev)?; // (B,1,128,256)
            t_sample += t0.elapsed().as_secs_f64();

            // latent noise
            let z = Tensor::randn(0f32, 1f32, (b, latent), &dev)?;
            let t1 = std::time::Instant::now();
            let fake = g.forward_train(&z)?; // (B,1,128,256)
            t_gfwd += t1.elapsed().as_secs_f64();

            // ---- D step ----
            let real_logits = d.forward(&real, true)?;
            let fake_logits_d = d.forward(&fake.detach(), true)?;
            let d_loss = (bce_with_logits(&real_logits, &real_target)?
                + bce_with_logits(&fake_logits_d, &fake_target)?)?;
            let t2 = std::time::Instant::now();
            opt_d.backward_step(&d_loss)?;
            t_dstep += t2.elapsed().as_secs_f64();

            // ---- G step ----
            let fake_logits_g = d.forward(&fake, true)?;
            let g_loss = bce_with_logits(&fake_logits_g, &gen_target)?;
            let t3 = std::time::Instant::now();
            opt_g.backward_step(&g_loss)?;
            t_gstep += t3.elapsed().as_secs_f64();

            d_running += d_loss.to_scalar::<f32>()?;
            g_running += g_loss.to_scalar::<f32>()?;
        }

        let n = cfg.steps_per_epoch as f32;
        // The ELAPSED SECONDS are on the line because everything downstream
        // needs them and nothing else has them: a rate, an ETA, and the answer
        // to "when should I come back". Without a clock in the output the
        // dashboard can plot a curve and cannot say how long it will take, and
        // a 20,000-epoch run with no ETA is one nobody can plan around.
        println!(
            "[train] epoch {:>5}/{}  d_loss {:.4}  g_loss {:.4}  ({:.1}s)",
            epoch + 1,
            cfg.epochs,
            d_running / n,
            g_running / n,
            t_start.elapsed().as_secs_f64(),
        );
        // WHERE THE EPOCH WENT. Printed once per epoch, so the operator can see
        // which phase to attack rather than guessing between batch size, model
        // size and the conv path. On this hardware candle has no BLAS backend —
        // mkl is x86 and accelerate is macOS — so the conv/matmul phases are
        // running on its portable path and are the expected suspects.
        let epoch_secs = t_epoch.elapsed().as_secs_f64();
        let acc = t_sample + t_gfwd + t_dstep + t_gstep;
        println!(
            "[train]   phases: sample {:.1}s · G fwd {:.1}s · D step {:.1}s · G step {:.1}s \
({:.0}% of the epoch accounted)",
            t_sample, t_gfwd, t_dstep, t_gstep,
            if epoch_secs > 0.0 { 100.0 * acc / epoch_secs } else { 0.0 },
        );

        // The existing check, not a second one. stop_requested() only honours
        // a STOP written AFTER this process started, which my first version
        // handled by deleting the file eagerly — worse, because a save that
        // panics then leaves no record that a stop was ever asked for.
        let asked_to_stop = stop_requested(cfg);

        let last = epoch + 1 == cfg.epochs || asked_to_stop;
        // STOP REQUESTED? Checked at the epoch boundary, where a checkpoint can
        // be written, so stopping costs at most one epoch rather than
        // everything since the last save.
        //
        // Through a FILE because the GUI runs this as a subprocess: it sets a
        // flag in its own memory and the trainer is a different address space,
        // so the button was inert. A signal would kill mid-epoch and lose the
        // work; a file the loop polls between epochs is what lets it exit
        // having saved. The check costs one stat per epoch.
        let asked_to_stop = stop_requested(cfg);
        if (epoch + 1) % cfg.ckpt_every == 0 || last || asked_to_stop {
            checkpoint(&g, &d, cfg, epoch + 1)?;
        }
        if asked_to_stop {
            // Removed on the way out, or the next run stops at epoch one and
            // looks broken.
            let _ = fs::remove_file(stop_path(cfg));
            println!("[train] stopped by request after epoch {} — checkpoint written", epoch + 1);
            return Ok(());
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


/// Where the GUI writes its stop request.
///
/// In the checkpoint directory rather than /tmp: it belongs to this run, two
/// trainers on one machine must not stop each other, and a stale file in a
/// checkpoint directory is discoverable by the person wondering why their run
/// keeps ending early.
pub fn stop_path(cfg: &Config) -> std::path::PathBuf {
    cfg.ckpt_dir.join("STOP")
}

/// Has a stop been asked for?
///
/// Only true for a file written AFTER this process started. A leftover STOP
/// from a previous run would otherwise end the next one at its first epoch,
/// and the operator would see a trainer that refuses to train.
pub fn stop_requested(cfg: &Config) -> bool {
    let p = stop_path(cfg);
    let Ok(md) = fs::metadata(&p) else { return false };
    let Ok(written) = md.modified() else { return true };
    match started_at().duration_since(std::time::UNIX_EPOCH) {
        Ok(start) => written
            .duration_since(std::time::UNIX_EPOCH)
            .map(|w| w >= start)
            .unwrap_or(true),
        Err(_) => true,
    }
}

/// When this process began, captured once.
fn started_at() -> std::time::SystemTime {
    use std::sync::OnceLock;
    static T: OnceLock<std::time::SystemTime> = OnceLock::new();
    *T.get_or_init(std::time::SystemTime::now)
}

/// Keeps the machine awake for as long as it is held.
///
/// A 20,000-epoch CPU run is hours, and the default on every desktop this
/// targets is to suspend on idle — training is not idle by any measure the
/// user cares about and is exactly idle by the one the session manager uses,
/// because nobody is touching the keyboard.
///
/// SPAWNS `systemd-inhibit` RATHER THAN TALKING TO D-BUS. logind owns the
/// inhibitor locks and KDE's powerdevil honours them, so this reaches
/// powerdevil without a bus library. That matters here specifically: the last
/// D-Bus crate added to this app pulled zbus with the tokio feature, which
/// panicked the file dialog AND took eframe's accessibility thread down with
/// it. A child process cannot do that.
///
/// The lock lives exactly as long as the child, and the child is killed on
/// drop — including on a panic, which is when leaving a machine permanently
/// awake would be least noticed.
pub struct StayAwake(Option<std::process::Child>);

impl StayAwake {
    /// Take the lock, or report why not and carry on.
    ///
    /// Never fatal. A machine with no logind — a container, a BSD, a phone
    /// running something else — should still train; it just may sleep. Failing
    /// the run over a power hint would be a worse trade than the hint.
    pub fn new(why: &str) -> StayAwake {
        let child = std::process::Command::new("systemd-inhibit")
            .arg("--what=idle:sleep")
            .arg("--who=beatspace")
            .arg(format!("--why={why}"))
            .arg("--mode=block")
            // A command that blocks forever and costs nothing: the inhibitor is
            // held for the lifetime of whatever systemd-inhibit runs, so the
            // command's only job is to not exit.
            .arg("sleep")
            .arg("infinity")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match child {
            Ok(c) => {
                println!("[train] holding an idle/sleep inhibitor — the machine will stay awake");
                StayAwake(Some(c))
            }
            Err(e) => {
                println!("[train] no sleep inhibitor ({e}) — the machine may suspend mid-run");
                StayAwake(None)
            }
        }
    }
}

impl Drop for StayAwake {
    fn drop(&mut self) {
        if let Some(c) = self.0.as_mut() {
            let _ = c.kill();
            // Reaped, or it becomes a zombie holding the lock for the lifetime
            // of the parent — which for the GUI is the whole session.
            let _ = c.wait();
        }
    }
}
