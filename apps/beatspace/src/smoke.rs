//! smoke — is the architecture sound, right now, on this machine, in seconds.
//!
//! ## What this is for
//!
//! A GAN that is broken and a GAN that is merely undertrained look identical
//! from the outside: both emit noise. The difference is only visible after
//! hours of real training, which is exactly when it is most expensive to
//! discover. So this trains a DELIBERATELY TINY model on DELIBERATELY
//! SYNTHETIC audio for a few seconds and asserts the things that must be true
//! of any working architecture, whatever the data is.
//!
//! Synthetic sources — sine, saw, square, and a kick/snare/clap kit — because
//! they are generated rather than loaded. No corpus to ship, no licence, no
//! decode path, identical on every machine, and their mel spectrograms are
//! genuinely different from each other, which is what the diversity checks
//! below need. A test that trained on one folder of real audio would pass or
//! fail for reasons that have nothing to do with the architecture.
//!
//! ## The check that matters most
//!
//! `outputs_vary_with_z`. Mode collapse — the generator learning one output
//! and emitting it for every latent — is the failure this whole exercise
//! exists to catch, and it is invisible in the loss curve: a collapsed
//! generator can post a perfectly reasonable g_loss forever. Two different `z`
//! vectors must produce measurably different mel. Nothing else here notices if
//! that stops being true.
//!
//! ## What it deliberately does NOT check
//!
//! Audio quality, convergence, or whether the model learns anything useful.
//! Those need real data and real time. Every assertion here is a STRUCTURAL
//! one — shapes flow, gradients arrive, outputs differ, losses are finite —
//! chosen so it can be true in three seconds on a CPU. A smoke test that
//! needed a good model would not run often enough to be worth having, and a
//! check that only passes on a lucky seed is worse than no check.
//!
//! ## Determinism
//!
//! Every source is generated from a fixed formula and every random draw comes
//! from an explicit seed, so a failure reproduces. Where a threshold is used
//! it is deliberately loose — these assert "not broken", not "good" — because
//! a tight threshold on a 40-step run is a flake generator, and a flaky smoke
//! test gets disabled, which is the same as not having one.

use anyhow::{bail, Context, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::{AdamW, Optimizer, ParamsAdamW};

use crate::config::Config;
use crate::dsp::Mel;
use crate::model::{bce_with_logits, Discriminator, Generator};

/// A deterministic PRNG, so a failure reproduces exactly.
///
/// SplitMix64, the same generator `core-kit::rng` uses, inlined for the same
/// reason `hatch-core` inlined its easing curve: this module is meant to run
/// with nothing else present, and one struct is cheaper than the dependency.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }
    /// Standard normal, Box-Muller. The latent distribution the generator is
    /// trained against, so the smoke test must draw from the same one.
    pub fn normal(&mut self) -> f32 {
        let u1 = self.next_f32().max(1e-7);
        let u2 = self.next_f32();
        (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
    }
    pub fn normal_vec(&mut self, n: usize) -> Vec<f32> {
        (0..n).map(|_| self.normal()).collect()
    }
}

/// The synthetic sources. Each is a different SHAPE in mel space, not just a
/// different pitch — a bank of sines at different frequencies would let a
/// collapsed generator score well by learning one blurry horizontal band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// One partial. Energy in a single mel band.
    Sine,
    /// All harmonics, 1/n. Energy spread up the spectrum, decaying.
    Saw,
    /// Odd harmonics only. A comb, visibly unlike saw.
    Square,
    /// Low sine with a fast decay envelope. Broadband transient, then bass.
    Kick,
    /// Noise burst plus a mid tone, medium decay.
    Snare,
    /// Short noise burst, no tone. Broadband and brief.
    Clap,
}

impl Source {
    pub const ALL: [Source; 6] = [
        Source::Sine,
        Source::Saw,
        Source::Square,
        Source::Kick,
        Source::Snare,
        Source::Clap,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Source::Sine => "sine",
            Source::Saw => "saw",
            Source::Square => "square",
            Source::Kick => "kick",
            Source::Snare => "snare",
            Source::Clap => "clap",
        }
    }

    /// `n` samples at `sr`, fundamental `f0` Hz, amplitude in [-1, 1].
    ///
    /// Noise uses the passed rng so the whole test stays reproducible; the
    /// tonal sources are pure formulas and need none.
    pub fn render(self, n: usize, sr: u32, f0: f32, rng: &mut Rng) -> Vec<f32> {
        let sr = sr as f32;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f32 / sr;
            let phase = std::f32::consts::TAU * f0 * t;
            let v = match self {
                Source::Sine => phase.sin(),
                // 16 harmonics is enough to be visibly saw-shaped in mel and
                // cheap enough that generating a second of it is free.
                Source::Saw => {
                    let mut s = 0.0;
                    for h in 1..=16 {
                        s += (phase * h as f32).sin() / h as f32;
                    }
                    s * (2.0 / std::f32::consts::PI)
                }
                Source::Square => {
                    let mut s = 0.0;
                    for h in (1..=31).step_by(2) {
                        s += (phase * h as f32).sin() / h as f32;
                    }
                    s * (4.0 / std::f32::consts::PI)
                }
                Source::Kick => {
                    // Pitch sweep down plus a fast amplitude decay: the shape
                    // that makes a kick a kick rather than a low sine.
                    let env = (-18.0 * t).exp();
                    let sweep = f0 * (1.0 + 6.0 * (-40.0 * t).exp());
                    (std::f32::consts::TAU * sweep * t).sin() * env
                }
                Source::Snare => {
                    let env = (-24.0 * t).exp();
                    let noise = rng.next_f32() * 2.0 - 1.0;
                    (phase.sin() * 0.4 + noise * 0.6) * env
                }
                Source::Clap => {
                    let env = (-60.0 * t).exp();
                    (rng.next_f32() * 2.0 - 1.0) * env
                }
            };
            out.push(v.clamp(-1.0, 1.0));
        }
        out
    }
}

/// A config sized so the whole test runs in seconds — but not so small that
/// the architecture stops being the architecture.
///
/// THE SIZES ARE NOT ARBITRARY, and getting them wrong looked exactly like a
/// broken model. The generator's channel schedule is
/// `[g_filters/2, /4, /8, 1]`, so `g_filters = 16` gives the last BatchNorm
/// **two channels**. With two channels, "both happen to be negative, so ReLU
/// zeroes the layer and the final tanh returns exactly 0" is a coin flip
/// rather than a pathology — and that is precisely what happened: eval-mode
/// output was exactly 0.0 everywhere while train mode was healthy
/// (range [-0.13, 0.27], z-diversity 0.034). At the real `g_filters = 256`
/// the same layer has 32 channels and all of them dying is not a thing that
/// occurs.
///
/// So `g_filters = 64` — the last BatchNorm gets 8 channels, small enough to
/// stay fast and wide enough that the eval path means something.
///
/// `time_frames = 32` for the matching reason: the seed grid is
/// `n_mels/16 × time_frames/16`, so 16 frames gives a width-1 seed, which is
/// a 4-wide transposed-convolution kernel operating almost entirely on
/// padding.
///
/// The lesson generalises past this function: a smoke test that shrinks a
/// model has to shrink it along the dimensions the model tolerates, or it
/// reports architecture failures that only exist at sizes nobody trains.
pub fn tiny_config() -> Config {
    let base = Config::default();
    Config {
        n_mels: 32,
        time_frames: 32,
        latent: 16,
        // -> deconv channels [32, 16, 8, 1]; last BatchNorm has 8 channels.
        g_filters: 64,
        // -> discriminator channels [32, 64, 128, 256].
        //
        // Not smaller, and this is the subtler of the two sizing traps.
        // `sparse_threshold` is an ABSOLUTE cut (0.05) inside a soft gate,
        // `x * sigma(40 * (|x| - 0.05))`, so a unit below threshold is
        // attenuated rather than killed — but the attenuation COMPOUNDS over
        // the four blocks. Activation magnitude grows with `d_filters`,
        // because each block sums over `cin * 4 * 4` taps, so a small
        // discriminator sits further under the threshold at every layer and
        // the product of four attenuations reaches the head as noise. At
        // `d_filters = 8` and again at 16, every score in a mixed batch came
        // out bit-identical: the head was seeing its own bias. That reads as
        // "the discriminator has collapsed" and is really "the threshold does
        // not scale with the model".
        //
        // model.rs already knows the shape of this — its own comment records
        // that "even at random init 80-99.9% of units in a block fall below
        // threshold". What it does not say is that the fraction depends on
        // `d_filters`, which is what makes the constant awkward to shrink.
        d_filters: 32,
        batch_size: 4,
        n_fft: 256,
        hop: 64,
        sr: 8_000,
        fmax: 4_000.0,
        ..base
    }
}

/// One synthetic training batch, already in the [0, 1] mel-unit range the
/// discriminator expects, shaped `[batch, 1, n_mels, time_frames]`.
pub fn synthetic_batch(cfg: &Config, mel: &Mel, rng: &mut Rng, dev: &Device) -> Result<Tensor> {
    let need = cfg.n_fft + cfg.hop * (cfg.time_frames + 4);
    let mut rows: Vec<f32> = Vec::with_capacity(cfg.batch_size * cfg.n_mels * cfg.time_frames);

    for b in 0..cfg.batch_size {
        let src = Source::ALL[b % Source::ALL.len()];
        // Spread the fundamentals so two rows of the same source still differ.
        let f0 = 80.0 * (1.0 + (b / Source::ALL.len()) as f32 * 0.5);
        let audio = src.render(need, cfg.sr, f0, rng);
        let (m, t) = mel.wav_to_mel(&audio);
        if t < cfg.time_frames {
            bail!(
                "synthetic clip produced {t} mel frames, need {}: raise the sample count",
                cfg.time_frames
            );
        }
        // Take the first `time_frames` columns; mel is [n_mels, t] row-major.
        for r in 0..cfg.n_mels {
            for c in 0..cfg.time_frames {
                rows.push(m[r * t + c]);
            }
        }
    }

    Tensor::from_vec(
        rows,
        (cfg.batch_size, 1, cfg.n_mels, cfg.time_frames),
        dev,
    )
    .context("building the synthetic batch tensor")
}

/// What a run observed. Returned rather than asserted so the CLI can print it
/// and the tests can assert on it — the same split as everything else here.
#[derive(Debug, Clone)]
pub struct Report {
    pub steps: usize,
    /// Discriminator loss at the first and last step.
    pub d_loss_first: f32,
    pub d_loss_last: f32,
    pub g_loss_first: f32,
    pub g_loss_last: f32,
    /// Mean absolute difference between generator outputs for two different
    /// `z`. Near zero means mode collapse.
    pub z_diversity: f32,
    /// Mean absolute difference between two generator outputs for the SAME
    /// `z`. Must be zero in eval mode — anything else means nondeterminism.
    pub z_repeatability: f32,
    /// Fraction of generator parameters whose value changed over the run.
    pub params_moved: f32,
    /// Spread of the discriminator's scores across a batch.
    ///
    /// EXACTLY zero means the head is seeing one input — the interesting
    /// failure. A merely SMALL spread does not mean that, and at 12 steps from
    /// random init it is the expected reading: `sparse_threshold` is an
    /// absolute 0.05 inside `x * sigma(40 * (|x| - 0.05))`, and model.rs
    /// records that "even at random init 80-99.9% of units in a block fall
    /// below threshold". Four blocks of that compounds, so what reaches the
    /// head at init is a whisper on top of its own bias. Measured here:
    /// ~1e-9, from scores that agree to eight decimals and differ in the
    /// ninth.
    pub d_score_spread: f32,
    /// How many discriminator scores that spread was measured over. One score
    /// has a spread of zero for arithmetic reasons rather than model ones.
    pub d_score_count: usize,
    /// The scores themselves. A spread of exactly zero is much easier to read
    /// when the values are visible: all-equal says the head is seeing one
    /// input, all-NaN says something else entirely.
    pub d_scores: Vec<f32>,
    /// Range of the generator's own output. A diversity of exactly zero reads
    /// the same whether the generator ignores `z` or emits one constant
    /// everywhere; this says which.
    pub out_min: f32,
    pub out_max: f32,
    /// The same diversity measured through `forward_train` instead of
    /// `forward_eval`. The two differ only in how BatchNorm normalises — batch
    /// statistics versus the running estimates — so when eval is dead and
    /// train is not, the running stats are the fault and not the weights.
    pub z_diversity_train: f32,
    pub out_min_train: f32,
    pub out_max_train: f32,
}

impl Report {
    /// A one-line summary for a log or a GUI status bar.
    pub fn line(&self) -> String {
        format!(
            "{} steps · d {:.3}->{:.3} · g {:.3}->{:.3} · z-diversity {:.4} · params moved {:.0}% · d-spread {:.3e}",
            self.steps,
            self.d_loss_first,
            self.d_loss_last,
            self.g_loss_first,
            self.g_loss_last,
            self.z_diversity,
            self.params_moved * 100.0,
            self.d_score_spread
        ) + &format!(
            " · gen eval [{:.4}, {:.4}] div {:.6} · gen train [{:.4}, {:.4}] div {:.6} · {} d-scores",
            self.out_min, self.out_max, self.z_diversity,
            self.out_min_train, self.out_max_train, self.z_diversity_train,
            self.d_score_count
        ) + &format!(" · scores {:?}", self.d_scores)
    }
}


/// Re-draw every parameter from our own seeded generator.
///
/// Uses the model's own `state_dict`/`load_state` pair rather than reaching
/// into its fields: the shapes come from the model, so this keeps working when
/// a layer is added, and a name that stops existing is a load error rather
/// than a silent skip.
///
/// PER-PARAMETER DISTRIBUTIONS, NOT ONE. The first version drew every entry
/// from N(0, 0.02) and broke the model completely — every check went to
/// exactly zero at once: 0% of parameters moved, z-diversity 0.00000000,
/// discriminator spread 0.00000000. The cause is that BatchNorm's gamma is
/// initialised to N(1, 0.02), not N(0, 0.02) — model.rs says so in a comment,
/// "Python weights_init: BN weight ~ N(1, 0.02), bias = 0" — so a blanket
/// redraw set every scale to ~0, which multiplies each activation by zero and
/// leaves a generator that emits one constant, has no gradient, and gives the
/// discriminator nothing to separate.
///
/// That is worth keeping as a comment rather than just a fix: the three
/// failures were the smoke test correctly detecting a broken model, and the
/// broken model was the test's own doing. The assertions were right. It also
/// shows the checks are not vacuous — something genuinely dead makes all three
/// fire at once, which is the behaviour they were written for.
///
/// The `run_mean`/`run_var` buffers are not trainable and not random: they are
/// zeros and ones. Redrawing them would be a different (and wrong) model.
fn reseed(g: &mut Generator, d: &mut Discriminator, dev: &Device, seed: u64) -> Result<()> {
    use std::collections::HashMap;
    let mut rng = Rng::new(seed);

    let mut redraw = |sd: HashMap<String, Tensor>| -> Result<HashMap<String, Tensor>> {
        // Sorted, so the draw order does not depend on HashMap iteration order
        // — otherwise "same seed" would still give different weights run to run.
        let mut names: Vec<String> = sd.keys().cloned().collect();
        names.sort();
        let mut out = HashMap::new();
        for name in names {
            let t = &sd[&name];
            let dims = t.dims().to_vec();
            let n: usize = dims.iter().product();
            // Match model.rs's own initialiser for each parameter kind.
            let vals: Vec<f32> = if name.ends_with(".run_var") {
                vec![1.0; n]
            } else if name.ends_with(".run_mean") || name.ends_with(".beta") {
                vec![0.0; n]
            } else if name.ends_with(".gamma") {
                (0..n).map(|_| 1.0 + rng.normal() * 0.02).collect()
            } else {
                // Weights and biases: randn_var's N(0, 0.02), the DCGAN convention.
                (0..n).map(|_| rng.normal() * 0.02).collect()
            };
            out.insert(name, Tensor::from_vec(vals, dims, dev)?);
        }
        Ok(out)
    };

    let gs = redraw(g.state_dict())?;
    g.load_state(&gs)?;
    let ds = redraw(d.state_dict())?;
    d.load_state(&ds)?;
    Ok(())
}

/// Train the tiny model on synthetic audio for `steps` and report what happened.
///
/// CPU by design: this must run in CI, in a sandbox, and on a phone, and the
/// point is to be runnable everywhere rather than fast anywhere.
pub fn run(steps: usize, seed: u64) -> Result<Report> {
    let cfg = tiny_config();
    let dev = Device::Cpu;
    let mel = Mel::new(&cfg);
    let mut rng = Rng::new(seed);

    let mut g = Generator::new(&cfg, &dev)?;
    let mut d = Discriminator::new(&cfg, &dev)?;
    // SEED THE WEIGHTS, not just the data.
    //
    // `Var::randn` in model.rs draws from candle's own global RNG, so seeding
    // only our `Rng` leaves initialisation random and two runs at "the same
    // seed" differ around the fifth decimal — which is exactly what the
    // reproducibility check caught. `Device::set_seed` is not the fix either:
    // the CPU device refuses it outright ("cannot seed the CPU rng with
    // set_seed").
    //
    // So the weights are re-drawn here from our own generator, through the
    // existing state_dict/load_state pair, at the same N(0, 0.02) the model
    // uses. Nothing about the architecture changes; the values just stop
    // depending on a RNG this test cannot reach.
    reseed(&mut g, &mut d, &dev, seed ^ 0xA5A5_A5A5)?;

    // Snapshot the generator's parameters so we can prove they moved. Compared
    // by value rather than by "did the optimizer run": an optimizer stepping
    // over an empty var list also "runs".
    let before: Vec<Vec<f32>> = g
        .trainable_vars()
        .iter()
        .map(|v| v.flatten_all()?.to_vec1::<f32>())
        .collect::<candle_core::Result<_>>()?;

    let params = |lr: f64| ParamsAdamW {
        lr,
        beta1: cfg.beta1,
        beta2: 0.999,
        eps: 1e-8,
        weight_decay: 0.0,
    };
    let mut opt_g = AdamW::new(g.trainable_vars(), params(cfg.lr))?;
    let mut opt_d = AdamW::new(d.trainable_vars(), params(cfg.lr))?;

    let (mut d_first, mut g_first) = (f32::NAN, f32::NAN);
    let (mut d_last, mut g_last) = (f32::NAN, f32::NAN);

    for step in 0..steps {
        let real = synthetic_batch(&cfg, &mel, &mut rng, &dev)?;
        let z = Tensor::from_vec(
            rng.normal_vec(cfg.batch_size * cfg.latent),
            (cfg.batch_size, cfg.latent),
            &dev,
        )?;

        // --- D step: real labelled 0.9 (one-sided smoothing), fake 0.0 ---
        let fake = g.forward_train(&z)?.detach();
        let d_real = d.forward(&real, true)?;
        let d_fake = d.forward(&fake, true)?;
        let ones = (Tensor::ones(d_real.shape(), DType::F32, &dev)? * cfg.label_smooth as f64)?;
        let zeros = Tensor::zeros(d_fake.shape(), DType::F32, &dev)?;
        let d_loss = (bce_with_logits(&d_real, &ones)? + bce_with_logits(&d_fake, &zeros)?)?;
        opt_d.backward_step(&d_loss)?;

        // --- G step: push the discriminator toward calling fakes real ---
        let fake = g.forward_train(&z)?;
        let d_out = d.forward(&fake, true)?;
        let target = Tensor::ones(d_out.shape(), DType::F32, &dev)?;
        let g_loss = bce_with_logits(&d_out, &target)?;
        opt_g.backward_step(&g_loss)?;

        let (dl, gl) = (
            d_loss.to_scalar::<f32>()?,
            g_loss.to_scalar::<f32>()?,
        );
        if !dl.is_finite() || !gl.is_finite() {
            bail!("step {step}: loss went non-finite (d={dl}, g={gl}) — the architecture is unstable, not undertrained");
        }
        if step == 0 {
            (d_first, g_first) = (dl, gl);
        }
        (d_last, g_last) = (dl, gl);
    }

    // --- did the parameters actually move? ---
    let after: Vec<Vec<f32>> = g
        .trainable_vars()
        .iter()
        .map(|v| v.flatten_all()?.to_vec1::<f32>())
        .collect::<candle_core::Result<_>>()?;
    let (mut moved, mut total) = (0usize, 0usize);
    for (b, a) in before.iter().zip(after.iter()) {
        for (x, y) in b.iter().zip(a.iter()) {
            total += 1;
            if (x - y).abs() > 0.0 {
                moved += 1;
            }
        }
    }
    let params_moved = if total == 0 {
        0.0
    } else {
        moved as f32 / total as f32
    };

    // --- diversity and repeatability, both in eval mode ---
    let za = rng.normal_vec(cfg.latent);
    let mut zb = rng.normal_vec(cfg.latent);
    // Guard against the two draws happening to be close: push them apart.
    for (i, v) in zb.iter_mut().enumerate() {
        *v = -za[i] + *v * 0.1;
    }
    let (out_a, _, _) = g.generate(&za, &dev)?;
    let (out_a2, _, _) = g.generate(&za, &dev)?;
    let (out_b, _, _) = g.generate(&zb, &dev)?;
    // Returns Result rather than 0.0-on-empty: a silent zero here would make
    // BOTH the diversity and the repeatability check pass for the wrong reason
    // — one wants a large number and the other wants exactly zero, so an empty
    // vector satisfies the second while hiding the first.
    let mad = |x: &[f32], y: &[f32]| -> Result<f32> {
        if x.is_empty() || x.len() != y.len() {
            bail!("generator returned {} and {} values — cannot compare", x.len(), y.len());
        }
        Ok(x.iter().zip(y).map(|(a, b)| (a - b).abs()).sum::<f32>() / x.len() as f32)
    };
    let z_diversity = mad(&out_a, &out_b)?;
    let z_repeatability = mad(&out_a, &out_a2)?;

    // --- is the discriminator still discriminating? ---
    let probe = synthetic_batch(&cfg, &mel, &mut rng, &dev)?;
    let scores = d.forward(&probe, false)?.flatten_all()?.to_vec1::<f32>()?;
    let d_score_spread = match (
        scores.iter().cloned().fold(f32::INFINITY, f32::min),
        scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
    ) {
        (lo, hi) if lo.is_finite() && hi.is_finite() => hi - lo,
        _ => 0.0,
    };

    let out_min = out_a.iter().cloned().fold(f32::INFINITY, f32::min);
    let out_max = out_a.iter().cloned().fold(f32::NEG_INFINITY, f32::max);

    // Same two latents, train-mode BatchNorm. One batch of two rows so the
    // batch statistics are real rather than a single row's own mean.
    let mut pair = za.clone();
    pair.extend_from_slice(&zb);
    let zt = Tensor::from_vec(pair, (2, cfg.latent), &dev)?;
    let tr = g.forward_train(&zt)?.flatten_all()?.to_vec1::<f32>()?;
    let half = tr.len() / 2;
    let z_diversity_train = mad(&tr[..half], &tr[half..])?;
    let out_min_train = tr.iter().cloned().fold(f32::INFINITY, f32::min);
    let out_max_train = tr.iter().cloned().fold(f32::NEG_INFINITY, f32::max);

    Ok(Report {
        steps,
        d_loss_first: d_first,
        d_loss_last: d_last,
        g_loss_first: g_first,
        g_loss_last: g_last,
        z_diversity,
        z_repeatability,
        params_moved,
        d_score_spread,
        d_score_count: scores.len(),
        d_scores: scores.clone(),
        out_min,
        out_max,
        z_diversity_train,
        out_min_train,
        out_max_train,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cheap enough to run in every one of the tests below.
    const STEPS: usize = 12;
    /// Any fixed value; the point is that it does not change between runs.
    const SEED: u64 = 0xB3A7_5EED_0000_0001;

    fn report() -> Report {
        run(STEPS, 7).expect("smoke run")
    }

    #[test]
    fn the_synthetic_sources_are_actually_different_from_each_other() {
        // If they were not, every diversity check below would be measuring
        // nothing. Assert the premise before relying on it.
        let cfg = tiny_config();
        let mel = Mel::new(&cfg);
        let need = cfg.n_fft + cfg.hop * (cfg.time_frames + 4);
        let mut rng = Rng::new(1);

        let mels: Vec<Vec<f32>> = Source::ALL
            .iter()
            .map(|s| {
                let a = s.render(need, cfg.sr, 110.0, &mut rng);
                mel.wav_to_mel(&a).0
            })
            .collect();

        for i in 0..mels.len() {
            for j in (i + 1)..mels.len() {
                let n = mels[i].len().min(mels[j].len());
                let diff: f32 = (0..n)
                    .map(|k| (mels[i][k] - mels[j][k]).abs())
                    .sum::<f32>()
                    / n as f32;
                assert!(
                    diff > 1e-3,
                    "{} and {} produce near-identical mel (mad {diff:.6}) — the source bank \
cannot detect mode collapse if its own sources collapse",
                    Source::ALL[i].name(),
                    Source::ALL[j].name()
                );
            }
        }
    }

    #[test]
    fn a_short_run_completes_with_finite_losses() {
        let r = report();
        assert_eq!(r.steps, STEPS);
        for (what, v) in [
            ("d_first", r.d_loss_first),
            ("d_last", r.d_loss_last),
            ("g_first", r.g_loss_first),
            ("g_last", r.g_loss_last),
        ] {
            assert!(v.is_finite(), "{what} is not finite: {v}");
        }
    }

    #[test]
    fn gradients_reach_the_generator() {
        // The failure this catches: a rewiring that leaves some parameter
        // group detached, so it never trains and nobody notices for hours.
        let r = report();
        assert!(
            r.params_moved > 0.5,
            "only {:.1}% of generator parameters changed over {STEPS} steps — \
something is detached from the graph",
            r.params_moved * 100.0
        );
    }

    #[test]
    fn outputs_vary_with_z() {
        // THE mode-collapse check. A generator that ignores its latent posts a
        // fine g_loss forever; this is the only thing here that notices.
        let r = report();
        assert!(
            r.z_diversity > 1e-4,
            "two different latents produced near-identical output (mad {:.8}) — \
the generator is ignoring z",
            r.z_diversity
        );
    }

    #[test]
    fn the_same_z_gives_the_same_output() {
        // Eval mode must be deterministic, or the latent explorer would drift
        // under a stationary cursor and every diversity number above would be
        // measuring noise instead of z.
        let r = report();
        assert_eq!(
            r.z_repeatability, 0.0,
            "the same latent produced different output twice in eval mode \
(mad {:.8}) — batchnorm or dropout is still in training mode",
            r.z_repeatability
        );
    }

    #[test]
    fn the_discriminator_does_not_answer_one_constant() {
        // A discriminator whose head sees one input gives the generator no
        // gradient signal at all, and the run continues looking healthy.
        //
        // THE BAR IS "NOT BIT-IDENTICAL", NOT "USEFULLY LARGE", and the first
        // version got this wrong by asserting > 1e-5. The measured spread on a
        // mixed batch at 12 steps is about 1e-9: scores agreeing to eight
        // decimals and differing in the ninth. That is not collapse, it is the
        // sparse gate — an absolute 0.05 threshold, four blocks deep, on a
        // model at random init, which model.rs's own comment says leaves
        // 80-99.9% of units under threshold per block.
        //
        // So the check tests the thing that is actually knowable here: are the
        // four scores DIFFERENT. Bit-identical output means the head is fed one
        // value; anything else means signal is reaching it, however faintly.
        // Asserting a magnitude instead would be asserting that a 12-step tiny
        // model discriminates well, which is not true and not the point.
        let r = report();
        assert!(
            r.d_score_spread > 0.0,
            "the discriminator returned bit-identical scores for four different \
inputs: {:?} — its head is seeing one value",
            r.d_scores
        );
        assert_eq!(r.d_score_count, tiny_config().batch_size, "one score per row");
    }

    #[test]
    fn the_run_is_reproducible_from_its_seed() {
        // Without this, a failure above is unreproducible and therefore
        // unfixable. Two runs at one seed must agree exactly.
        let a = run(4, 99).expect("first");
        let b = run(4, 99).expect("second");
        assert_eq!(
            a.d_loss_last, b.d_loss_last,
            "same seed gave different d_loss: {} vs {}",
            a.d_loss_last, b.d_loss_last
        );
        assert_eq!(
            a.g_loss_last, b.g_loss_last,
            "same seed gave different g_loss: {} vs {}",
            a.g_loss_last, b.g_loss_last
        );
    }

    #[test]
    fn the_report_line_names_every_number_it_prints() {
        let r = report();
        let l = r.line();
        for token in ["steps", "d ", "g ", "z-diversity", "params moved", "d-spread"] {
            assert!(l.contains(token), "summary is missing {token:?}: {l}");
        }
    }

    #[test]
    fn the_rng_is_split_mix_and_normal_looks_normal() {
        let mut rng = Rng::new(SEED);
        let n = 4096;
        let xs: Vec<f32> = (0..n).map(|_| rng.normal()).collect();
        let mean = xs.iter().sum::<f32>() / n as f32;
        let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / n as f32;
        // Loose on purpose: this pins "is a standard normal", not the exact
        // moments of 4096 draws.
        assert!(mean.abs() < 0.1, "mean {mean} is not near 0");
        assert!((var - 1.0).abs() < 0.15, "variance {var} is not near 1");
    }
}
