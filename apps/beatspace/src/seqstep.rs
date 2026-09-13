//! One training step of the dynamic-track model, with no I/O in it.
//!
//! `train.rs` samples a `SeqBatch` from the dataset and checkpoints; the
//! smoke test builds one from synthetic audio. Both call `step` here, so
//! the thing being smoke-tested is the thing that trains.
//!
//! ## What a step does
//!
//! A batch is `B` runs of `K` consecutive windows, `window_stride` frames
//! apart, so adjacent windows share `overlap_frames`. From it:
//!
//! 1. **Teacher-forced continuations.** For every position `w >= 1` the
//!    renderer is given the tail of the REAL window `w-1`, the beat phase of
//!    window `w`, and fresh noise, and asked for window `w`. All `B*(K-1)`
//!    positions go through the generator as one batch.
//! 2. **A free-running rollout** (when `cfg.rollout`). Starting from the real
//!    window 0, the renderer continues ITSELF `K-1` times. Playback only ever
//!    does this, and a model trained on real context alone drifts within a
//!    few windows when fed its own output — this is the standard exposure
//!    bias fix. Gradient flows back through the chain, so an early window is
//!    also trained to be a good context for the next.
//! 3. **Two discriminators.** The window discriminator judges each window as
//!    before, now over all stems at once — so "the drums fit these
//!    instruments" is part of what real means. The sequence discriminator
//!    sees `[prev ‖ next[overlap..]]`, the two windows joined at their seam
//!    into one `2T - overlap` strip, and scores every time column, so a bad
//!    join is penalised where it is rather than averaged into the texture.
//!
//! The generator's loss is the sum of both discriminators' verdicts on its
//! teacher-forced output, plus the sequence verdict on the rollout.

use anyhow::{bail, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::{AdamW, Optimizer};

use crate::config::Config;
use crate::dataset::SeqBatch;
use crate::model::{bce_with_logits, Cond, Discriminator, Generator, SeqDiscriminator};

/// The three networks and their optimisers.
pub struct SeqModels {
    pub g: Generator,
    pub d: Discriminator,
    pub sd: SeqDiscriminator,
    pub opt_g: AdamW,
    pub opt_d: AdamW,
    pub opt_sd: AdamW,
}

/// What one step measured, for the log and for assertions.
#[derive(Debug, Clone, Copy)]
pub struct SeqLosses {
    /// Window discriminator, real + fake.
    pub d: f32,
    /// Sequence discriminator, real + fakes.
    pub sd: f32,
    /// Generator total.
    pub g: f32,
    /// Its window part.
    pub g_win: f32,
    /// Its sequence part (teacher-forced and rollout averaged).
    pub g_seq: f32,
}

impl SeqLosses {
    pub fn finite(&self) -> bool {
        [self.d, self.sd, self.g, self.g_win, self.g_seq].iter().all(|x| x.is_finite())
    }
}

/// Join two adjacent windows at their overlap into one strip.
///
/// The first `overlap` columns of `next` are the same instant in time as the
/// last `overlap` of `prev`, so they are dropped from `next` rather than
/// played twice: a real pair is then literally the clip's own `2T - overlap`
/// consecutive frames, and a fake pair has the same geometry.
///
/// THE STRIP IS THEN TRIMMED TO A MULTIPLE OF 16 COLUMNS, symmetrically, so
/// the seam stays in the middle. candle 0.11's `Conv2D` backward computes
/// the transposed convolution's `out_padding` from the HEIGHT only
/// (backprop.rs: `arg.dim(2)? - out_size`) and applies it to both axes, so
/// a stride-2 stack over an input whose width parity ever differs from its
/// height parity fails in the backward pass with "shape mismatch in add".
/// `n_mels` is a multiple of 16 and stays even through all four blocks, so
/// the width must too. Six columns off each end of a 124-wide strip is what
/// this costs at the default geometry; the join is untouched.
pub fn join_pair(prev: &Tensor, next: &Tensor, overlap: usize) -> Result<Tensor> {
    let t = next.dim(3)?;
    let tail = next.narrow(3, overlap, t - overlap)?;
    let strip = Tensor::cat(&[prev, &tail], 3)?;
    let w = strip.dim(3)?;
    let keep = w / 16 * 16;
    if keep == 0 {
        bail!("a joined pair is only {w} columns wide; the sequence discriminator needs 16");
    }
    let lead = (w - keep) / 2;
    Ok(strip.narrow(3, lead, keep)?)
}

/// The conditioning the renderer gets for the window after `prev`.
fn cond_for(cfg: &Config, prev: &Tensor, phase: &Tensor) -> Result<Cond> {
    let t = prev.dim(3)?;
    Ok(Cond {
        ctx: if cfg.ctx_frames > 0 {
            Some(prev.narrow(3, t - cfg.ctx_frames, cfg.ctx_frames)?.contiguous()?)
        } else {
            None
        },
        phase: if cfg.beat_phase { Some(phase.clone()) } else { None },
    })
}

/// Concatenate a list of equally-shaped tensors along the batch axis.
fn stack_batch(parts: &[Tensor]) -> Result<Tensor> {
    Ok(Tensor::cat(parts, 0)?)
}

/// Run one step and update all three networks. `z_for` supplies the latent
/// noise for a given batch size, so a caller can seed it.
pub fn step(
    cfg: &Config,
    dev: &Device,
    batch: &SeqBatch,
    m: &mut SeqModels,
    mut z_for: impl FnMut(usize) -> Result<Tensor>,
) -> Result<SeqLosses> {
    let (b, k, _c, _m, _t) = batch.mel.dims5()?;
    if k < 2 {
        bail!("a sequence step needs at least two windows per sample, got {k}");
    }
    let ov = cfg.overlap_frames;
    let smooth = cfg.label_smooth as f64;

    // Per-position views. narrow along K then drop the axis: (B, C, M, T)
    // and (B, T).
    let real: Vec<Tensor> = (0..k).map(|w| Ok(batch.mel.narrow(1, w, 1)?.squeeze(1)?)).collect::<Result<_>>()?;
    let phase: Vec<Tensor> = (0..k).map(|w| Ok(batch.phase.narrow(1, w, 1)?.squeeze(1)?)).collect::<Result<_>>()?;

    // ---- teacher-forced continuations, one generator batch ----
    let prevs: Vec<Tensor> = (1..k).map(|w| real[w - 1].clone()).collect();
    let nexts: Vec<Tensor> = (1..k).map(|w| real[w].clone()).collect();
    let real_prev = stack_batch(&prevs)?; // (B(K-1), C, M, T)
    let real_next = stack_batch(&nexts)?;
    let phase_next = stack_batch(&(1..k).map(|w| phase[w].clone()).collect::<Vec<_>>())?;
    let cond_tf = cond_for(cfg, &real_prev, &phase_next)?;
    let n_tf = b * (k - 1);
    let fake_tf = m.g.forward_train_cond(&z_for(n_tf)?, &cond_tf)?;

    let real_pair = join_pair(&real_prev, &real_next, ov)?;
    let fake_pair_tf = join_pair(&real_prev, &fake_tf, ov)?;

    // ---- free-running rollout ----
    let fake_pair_roll = if cfg.rollout {
        let mut prev = real[0].clone();
        let mut pairs = Vec::with_capacity(k - 1);
        for w in 1..k {
            let cond = cond_for(cfg, &prev, &phase[w])?;
            let g_w = m.g.forward_train_cond(&z_for(b)?, &cond)?;
            pairs.push(join_pair(&prev, &g_w, ov)?);
            prev = g_w;
        }
        Some(stack_batch(&pairs)?)
    } else {
        None
    };

    let targets = |like: &Tensor, v: f64| -> Result<Tensor> {
        Ok((Tensor::ones(like.shape(), DType::F32, dev)? * v)?)
    };

    // ---- window discriminator ----
    let d_real = m.d.forward(&real_next, true)?;
    let d_fake = m.d.forward(&fake_tf.detach(), true)?;
    let d_loss = (bce_with_logits(&d_real, &targets(&d_real, smooth)?)?
        + bce_with_logits(&d_fake, &targets(&d_fake, 0.0)?)?)?;
    m.opt_d.backward_step(&d_loss)?;

    // ---- sequence discriminator ----
    // The two fake sources share one weight so real and fake stay balanced.
    let sd_real = m.sd.forward(&real_pair, true)?;
    let sd_fake_tf = m.sd.forward(&fake_pair_tf.detach(), true)?;
    let mut sd_fake = bce_with_logits(&sd_fake_tf, &targets(&sd_fake_tf, 0.0)?)?;
    if let Some(roll) = &fake_pair_roll {
        let sd_fake_roll = m.sd.forward(&roll.detach(), true)?;
        sd_fake = ((sd_fake + bce_with_logits(&sd_fake_roll, &targets(&sd_fake_roll, 0.0)?)?)? * 0.5)?;
    }
    let sd_loss = (bce_with_logits(&sd_real, &targets(&sd_real, smooth)?)? + sd_fake)?;
    m.opt_sd.backward_step(&sd_loss)?;

    // ---- generator, against the updated discriminators ----
    let g_d = m.d.forward(&fake_tf, true)?;
    let g_win = bce_with_logits(&g_d, &targets(&g_d, 1.0)?)?;
    let g_sd_tf = m.sd.forward(&fake_pair_tf, true)?;
    let mut g_seq = bce_with_logits(&g_sd_tf, &targets(&g_sd_tf, 1.0)?)?;
    if let Some(roll) = &fake_pair_roll {
        let g_sd_roll = m.sd.forward(roll, true)?;
        g_seq = ((g_seq + bce_with_logits(&g_sd_roll, &targets(&g_sd_roll, 1.0)?)?)? * 0.5)?;
    }
    let g_loss = (&g_win + &g_seq)?;
    m.opt_g.backward_step(&g_loss)?;

    Ok(SeqLosses {
        d: d_loss.to_scalar::<f32>()?,
        sd: sd_loss.to_scalar::<f32>()?,
        g: g_loss.to_scalar::<f32>()?,
        g_win: g_win.to_scalar::<f32>()?,
        g_seq: g_seq.to_scalar::<f32>()?,
    })
}

/// Continue from `prev` for `n` windows in eval mode — what playback does —
/// returning each window. For the smoke test's rollout check.
pub fn rollout_eval(
    cfg: &Config,
    g: &Generator,
    prev: &Tensor,
    phases: &[Tensor],
    mut z_for: impl FnMut(usize) -> Result<Tensor>,
) -> Result<Vec<Tensor>> {
    let b = prev.dim(0)?;
    let mut cur = prev.clone();
    let mut out = Vec::with_capacity(phases.len());
    for ph in phases {
        let cond = cond_for(cfg, &cur, ph)?;
        let next = g.forward_eval_cond(&z_for(b)?, &cond)?;
        out.push(next.clone());
        cur = next;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_pair_drops_the_overlap_and_trims_to_sixteen_columns_around_the_seam() {
        let dev = Device::Cpu;
        // Two 11-wide windows overlapping by 2: 20 columns, trimmed to 16 by
        // dropping two from each end. Columns are numbered so the seam is
        // visible: prev is 0..11, next continues 9..20 in the same timeline.
        let a = Tensor::arange(0f32, 11.0, &dev).unwrap().reshape((1, 1, 1, 11)).unwrap();
        let b = Tensor::arange(9f32, 20.0, &dev).unwrap().reshape((1, 1, 1, 11)).unwrap();
        let j = join_pair(&a, &b, 2).unwrap();
        assert_eq!(j.dims(), &[1, 1, 1, 16]);
        let v: Vec<f32> = j.flatten_all().unwrap().to_vec1().unwrap();
        let want: Vec<f32> = (2..18).map(|x| x as f32).collect();
        assert_eq!(v, want, "one contiguous timeline, seam at column 9");
    }

    #[test]
    fn a_sixteen_multiple_strip_survives_the_sequence_discriminator_backward() {
        // The candle limitation join_pair works around, pinned: an untrimmed
        // 60-wide strip fails in the backward pass, a 48-wide one does not.
        use crate::model::SeqDiscriminator;
        let dev = Device::Cpu;
        let cfg = Config { n_mels: 32, time_frames: 32, d_filters: 32, latent: 16, g_filters: 64, ..Config::dynamic() };
        let sd = SeqDiscriminator::new(&cfg, &dev).unwrap();
        let mut opt = AdamW::new(sd.trainable_vars(), Default::default()).unwrap();
        let good = Tensor::randn(0f32, 1f32, (2, 2, 32, 48), &dev).unwrap();
        let loss = sd.forward(&good, true).unwrap().sqr().unwrap().mean_all().unwrap();
        opt.backward_step(&loss).expect("a 48-wide strip must train");
        let bad = Tensor::randn(0f32, 1f32, (2, 2, 32, 60), &dev).unwrap();
        let loss = sd.forward(&bad, true).unwrap().sqr().unwrap().mean_all().unwrap();
        let err = opt.backward_step(&loss).expect_err("candle 0.11 cannot backprop a stride-2 conv over a 60-wide, 32-high input; if this passes, drop the trim in join_pair");
        assert!(err.to_string().contains("shape mismatch"), "{err}");
    }
}
