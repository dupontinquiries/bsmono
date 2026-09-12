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
    bce_with_logits, load_discriminator, load_generator, load_seq_discriminator,
    save_discriminator, save_generator, save_seq_discriminator, Discriminator, Generator,
    SeqDiscriminator,
};
use crate::seqstep::{self, SeqModels};

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
    let mut sd = if cfg.sequential() { Some(SeqDiscriminator::new(cfg, &dev)?) } else { None };
    if cfg.sequential() {
        println!(
            "[train] sequence path: {} windows per sample at stride {} ({} stems, ctx {} frames, \
beat phase {}, rollout {})",
            cfg.seq_windows, cfg.window_stride(), cfg.stems, cfg.ctx_frames, cfg.beat_phase, cfg.rollout
        );
    } else if cfg.conditional() {
        anyhow::bail!(
            "ctx_frames / beat_phase are set but seq_windows is {}: a conditional renderer \
needs consecutive windows to learn from. Set --seq-windows 2 or more (or --dynamic).",
            cfg.seq_windows
        );
    }

    let mut start_epoch = 0usize;
    if resume {
        let gp = cfg.ckpt_dir.join("gen_latest.safetensors");
        let dp = cfg.ckpt_dir.join("disc_latest.safetensors");
        if gp.exists() && dp.exists() {
            load_generator(&mut g, &gp, &dev)?;
            load_discriminator(&mut d, &dp, &dev)?;
            if let Some(sd) = &mut sd {
                let sp = cfg.ckpt_dir.join("seqd_latest.safetensors");
                if sp.exists() {
                    load_seq_discriminator(sd, &sp, &dev)?;
                } else {
                    println!("[train] no seqd_latest.safetensors — the sequence discriminator \
starts fresh against a trained generator; expect its loss to fall quickly");
                }
            }
            match read_resume_epoch(cfg) {
                Some(e) => {
                    start_epoch = e;
                    println!("[train] resumed from {gp:?} at epoch {e}");
                }
                None => println!(
                    "[train] resumed from {gp:?}, but {} is missing or unreadable — \
the epoch counter restarts at 0. Weights are fine; only the numbering is lost.",
                    resume_state_path(cfg).display()),
            }
            // WHAT A RESUME DOES NOT RESTORE, said out loud because the run
            // will look slightly worse for a few hundred steps and the operator
            // should know why rather than suspecting the checkpoint.
            //
            // Adam's first and second moment estimates are gone. candle's
            // `AdamW` keeps them in private fields with no accessor
            // (candle-nn/src/optim.rs: `first_moment` / `second_moment` on a
            // private per-var struct; only `params()`/`set_params()` are
            // public), so there is nothing to serialise without forking candle.
            //
            // The cost is bounded and worth stating precisely: beta2 = 0.999
            // gives the second moment a memory of roughly 1/(1-beta2) = 1000
            // steps, so the optimiser is re-warmed within about that many —
            // under a fifth of one epoch at the default 64 steps. It is a
            // transient, not a reset.
            println!("[train] NOTE: optimizer state is NOT restored (candle's AdamW keeps its \
moments private). Expect ~1000 steps of re-warming; the weights themselves are exact.");
        } else {
            println!("[train] --resume set but no latest checkpoint in {}; starting fresh",
                     cfg.ckpt_dir.display());
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
    let opt_g = adam(g.trainable_vars())?;
    let opt_d = adam(d.trainable_vars())?;
    // The networks and their optimisers, owned by whichever path runs them.
    let mut nets = match sd.take() {
        Some(sd) => {
            let opt_sd = adam(sd.trainable_vars())?;
            Nets::Seq(SeqModels { g, d, sd, opt_g, opt_d, opt_sd })
        }
        None => Nets::Legacy { g, d, opt_g, opt_d },
    };

    let b = cfg.batch_size;
    let latent = cfg.latent;
    let smooth = cfg.label_smooth as f64;

    // fixed targets reused each step
    let ones = Tensor::ones(b, DType::F32, &dev)?;
    let zeros = Tensor::zeros(b, DType::F32, &dev)?;
    let real_target = ones.affine(smooth, 0.0)?; // 0.9
    let fake_target = zeros.clone();
    let gen_target = ones.clone();

    // FROM start_epoch, not 0: `cfg.epochs` is the target the operator asked to
    // reach, not a count of additional epochs. Resuming a 20000-epoch run at
    // 4000 should do 16000 more, and `--epochs 4000` on a run already at 4000
    // should do nothing and say so.
    if start_epoch >= cfg.epochs {
        println!("[train] already at epoch {start_epoch} of a {}-epoch target — nothing to do. \
Raise --epochs to continue.", cfg.epochs);
        return Ok(());
    }
    for epoch in start_epoch..cfg.epochs {
        let mut d_running = 0f32;
        let mut g_running = 0f32;

        // Per-phase timing, because "97 s/epoch" is not an answer to WHY.
        // Nothing here was attributed, so the choice between a smaller batch, a
        // smaller model and a different conv path was a guess. Accumulated over
        // the epoch and printed with it — four Instant reads per step against a
        // step that takes over a second.
        let (mut t_sample, mut t_gfwd, mut t_dstep, mut t_gstep) = (0f64, 0f64, 0f64, 0f64);
        let mut t_dfwd = 0f64;
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
                checkpoint(&nets, cfg, epoch)?;
                let _ = fs::remove_file(stop_path(cfg));
                return Ok(());
            }

            let (g, d, opt_g, opt_d) = match &mut nets {
                Nets::Seq(m) => {
                    // ---- the sequence path: one call, three networks ----
                    let t0 = std::time::Instant::now();
                    let batch = ds.sample_sequence_batch(b, cfg.seq_windows, &dev)?;
                    t_sample += t0.elapsed().as_secs_f64();
                    let t1 = std::time::Instant::now();
                    let l = seqstep::step(cfg, &dev, &batch, m, |n| {
                        Ok(Tensor::randn(0f32, 1f32, (n, latent), &dev)?)
                    })?;
                    // Attributed to G bwd on the phase line: the step is not
                    // split into phases yet, and that is the column an
                    // operator reading the legacy line takes as "the model".
                    t_gstep += t1.elapsed().as_secs_f64();
                    if !l.finite() {
                        anyhow::bail!("epoch {epoch} step {step}: a loss went non-finite ({l:?})");
                    }
                    d_running += l.d + l.sd;
                    g_running += l.g;
                    continue;
                }
                Nets::Legacy { g, d, opt_g, opt_d } => (g, d, opt_g, opt_d),
            };

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
            // TIMED, because these three D forwards were the missing 27%. The
            // first phase report accounted for 73% of the epoch and the gap was
            // exactly here: two forwards in the D step and one more in the G
            // step, none of them measured.
            let tdf = std::time::Instant::now();
            let real_logits = d.forward(&real, true)?;
            let fake_logits_d = d.forward(&fake.detach(), true)?;
            let d_loss = (bce_with_logits(&real_logits, &real_target)?
                + bce_with_logits(&fake_logits_d, &fake_target)?)?;
            t_dfwd += tdf.elapsed().as_secs_f64();
            let t2 = std::time::Instant::now();
            opt_d.backward_step(&d_loss)?;
            t_dstep += t2.elapsed().as_secs_f64();

            // ---- G step ----
            // The THIRD D forward. Not removable: opt_d has just updated the
            // discriminator's weights, so the generator must be scored against
            // the new D — reusing fake_logits_d would train G against a
            // discriminator that no longer exists. Standard DCGAN ordering, and
            // it costs a full D forward per step.
            let tdf2 = std::time::Instant::now();
            let fake_logits_g = d.forward(&fake, true)?;
            let g_loss = bce_with_logits(&fake_logits_g, &gen_target)?;
            t_dfwd += tdf2.elapsed().as_secs_f64();
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
        let acc = t_sample + t_gfwd + t_dfwd + t_dstep + t_gstep;
        println!(
            "[train]   phases: sample {:.1}s · G fwd {:.1}s · D fwd x3 {:.1}s · D bwd {:.1}s · \
G bwd {:.1}s ({:.0}% accounted). G bwd runs through the WHOLE discriminator before it \
reaches G, which is why it is many times the G forward.",
            t_sample, t_gfwd, t_dfwd, t_dstep, t_gstep,
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
            checkpoint(&nets, cfg, epoch + 1)?;
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

/// The networks a run owns. The legacy path is the v6 DCGAN pair; the
/// sequence path adds the sequence discriminator and moves the step into
/// `seqstep`, which the smoke test drives with the same struct.
enum Nets {
    Legacy { g: Generator, d: Discriminator, opt_g: AdamW, opt_d: AdamW },
    Seq(SeqModels),
}

fn checkpoint(nets: &Nets, cfg: &Config, epoch: usize) -> Result<()> {
    let (g, d, sd): (&Generator, &Discriminator, Option<&SeqDiscriminator>) = match nets {
        Nets::Legacy { g, d, .. } => (g, d, None),
        Nets::Seq(m) => (&m.g, &m.d, Some(&m.sd)),
    };
    let dir: &Path = &cfg.ckpt_dir;
    let gp = dir.join(format!("gen_{epoch:06}.safetensors"));
    let dp = dir.join(format!("disc_{epoch:06}.safetensors"));
    save_generator(g, &gp)?;
    save_discriminator(d, &dp)?;
    // rolling "latest" so the GUI can always find the newest weights
    save_generator(g, &dir.join("gen_latest.safetensors"))?;
    save_discriminator(d, &dir.join("disc_latest.safetensors"))?;
    if let Some(sd) = sd {
        save_seq_discriminator(sd, &dir.join(format!("seqd_{epoch:06}.safetensors")))?;
        save_seq_discriminator(sd, &dir.join("seqd_latest.safetensors"))?;
    }
    // WHICH EPOCH THOSE WEIGHTS ARE. Without it `--resume` reloaded the weights
    // and restarted the counter at zero, so gen_000000 was overwritten on every
    // resume, `ckpt_every` re-triggered from the beginning, and the log claimed
    // epoch 3 of a run that had already done 4000.
    write_resume_state(cfg, epoch)?;
    println!("[train] checkpoint @ epoch {epoch} -> {gp:?}");
    Ok(())
}

/// Where the epoch counter lives between runs.
///
/// A sidecar rather than a safetensors field: it is one integer, it must be
/// readable without loading 4 GB of weights (the GUI wants it, and so does a
/// person with `cat`), and a corrupt or absent one has to degrade to "start
/// from zero" rather than making the checkpoint unloadable.
pub fn resume_state_path(cfg: &Config) -> std::path::PathBuf {
    cfg.ckpt_dir.join("resume.json")
}

fn write_resume_state(cfg: &Config, epoch: usize) -> Result<()> {
    let body = format!(
        "{{\n  \"epoch\": {epoch},\n  \"time_frames\": {},\n  \"n_mels\": {},\n  \"latent\": {}\n}}\n",
        cfg.time_frames, cfg.n_mels, cfg.latent);
    fs::write(resume_state_path(cfg), body)
        .with_context(|| format!("writing {}", resume_state_path(cfg).display()))
}

/// The epoch a `--resume` should continue FROM, or 0.
///
/// Deliberately tolerant: a missing, unreadable or unparsable file means start
/// at zero and say so. The alternative — refusing to train because a
/// bookkeeping sidecar is malformed — turns a cosmetic problem into a stopped
/// run, and the weights are still perfectly loadable.
pub fn read_resume_epoch(cfg: &Config) -> Option<usize> {
    read_resume_epoch_in(&cfg.ckpt_dir)
}

/// The same, for any checkpoint directory — the GUI loads weights from
/// paths that are not its own `ckpt_dir` and wants the epoch beside them.
pub fn read_resume_epoch_in(dir: &Path) -> Option<usize> {
    let raw = fs::read_to_string(dir.join("resume.json")).ok()?;
    let after = raw.split(r#""epoch""#).nth(1)?;
    let digits: String = after.trim_start_matches([':', ' '])
        .chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
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

#[cfg(test)]
mod resume_state {
    use super::*;
    use crate::config::Config;

    fn tmp(name: &str) -> Config {
        let dir = std::env::temp_dir().join(format!("beatspace-resume-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Config { ckpt_dir: dir, ..Config::default() }
    }

    #[test]
    fn an_epoch_written_is_an_epoch_read() {
        let cfg = tmp("roundtrip");
        write_resume_state(&cfg, 4137).unwrap();
        assert_eq!(read_resume_epoch(&cfg), Some(4137));
        fs::remove_dir_all(&cfg.ckpt_dir).ok();
    }

    #[test]
    fn epoch_zero_round_trips_as_zero_not_as_absent() {
        // The distinction matters: Some(0) means "checkpointed at the very
        // start", None means "no record". Conflating them would make a fresh
        // checkpoint look like a missing one.
        let cfg = tmp("zero");
        write_resume_state(&cfg, 0).unwrap();
        assert_eq!(read_resume_epoch(&cfg), Some(0));
        fs::remove_dir_all(&cfg.ckpt_dir).ok();
    }

    #[test]
    fn a_missing_file_is_none_rather_than_an_error() {
        let cfg = tmp("missing");
        assert_eq!(read_resume_epoch(&cfg), None);
        fs::remove_dir_all(&cfg.ckpt_dir).ok();
    }

    #[test]
    fn garbage_degrades_to_none_instead_of_stopping_the_run() {
        // A malformed bookkeeping sidecar must not be able to prevent training.
        // The weights are still loadable; only the numbering is lost.
        let cfg = tmp("garbage");
        for body in ["", "{", "not json at all", r#"{"epoch": }"#, r#"{"epochs": 12}"#] {
            fs::write(resume_state_path(&cfg), body).unwrap();
            assert_eq!(read_resume_epoch(&cfg), None, "body {body:?} should not parse");
        }
        fs::remove_dir_all(&cfg.ckpt_dir).ok();
    }

    #[test]
    fn the_record_carries_the_shape_so_a_reader_can_see_it_without_the_weights() {
        // The point of a sidecar over a safetensors field: readable with `cat`,
        // and by the GUI, without loading gigabytes.
        let cfg = Config { time_frames: 128, n_mels: 64, latent: 32, ..tmp("shape") };
        write_resume_state(&cfg, 7).unwrap();
        let raw = fs::read_to_string(resume_state_path(&cfg)).unwrap();
        for needle in ["\"epoch\": 7", "\"time_frames\": 128", "\"n_mels\": 64", "\"latent\": 32"] {
            assert!(raw.contains(needle), "record is missing {needle}:\n{raw}");
        }
        fs::remove_dir_all(&cfg.ckpt_dir).ok();
    }

    #[test]
    fn a_later_write_replaces_the_earlier_epoch() {
        // checkpoint() calls this every time; a stale value would send a resume
        // backwards and silently redo work.
        let cfg = tmp("overwrite");
        write_resume_state(&cfg, 10).unwrap();
        write_resume_state(&cfg, 200).unwrap();
        assert_eq!(read_resume_epoch(&cfg), Some(200));
        fs::remove_dir_all(&cfg.ckpt_dir).ok();
    }
}
