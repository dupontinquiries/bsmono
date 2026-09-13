//! The GAN itself, ported faithfully from the v6 (`gan-zai` r2) PyTorch model.
//!
//! Layers are built *by hand* out of `Var`s rather than with candle_nn's
//! helper constructors. The reason is checkpointing: we want full control over
//! exactly which tensors are saved/loaded — including BatchNorm's running
//! mean/var, which the GUI needs at eval time and which candle_nn's BatchNorm
//! keeps in a place that `VarMap::save` would silently drop.
//!
//! Shapes (B = batch, C = stems):
//!   Generator:     z (B,128) [+ context, + beat phase] -> mel (B,C,n_mels,time_frames) in [-1,1]
//!   Discriminator: mel (B,C,n_mels,time_frames)   -> logit (B,)
//!   SeqDiscriminator: mel (B,C,n_mels,W) -> logits (B, W/16), one per time column
//!
//! Generator stack (g_filters f = 256):
//!   [ctx encoder: (B,C,n_mels,ctx_frames) -> (B,ctx_dim), concatenated to z]
//!   fc: latent(+ctx_dim) -> (n_mels/16)*(time_frames/16)*f, reshape (B,f,h0,w0)
//!   4x ConvTranspose2d k4 s2 p1 (f->f/2), BatchNorm, ReLU      256->128->64->32
//!   [beat phase: sin/cos planes appended before the final deconv]
//!   final ConvTranspose2d -> C channels, Tanh                   -> (B,C,n_mels,time_frames)
//!
//! Every bracketed part is config-gated (`ctx_frames`, `beat_phase`, `stems`)
//! and OFF in `Config::default()`, under which this is byte-for-byte the v6
//! model and its checkpoints load. `Config::dynamic()` turns them all on.
//!
//! Discriminator stack (d_filters = 64), 4x SparseBlock stride 2:
//!   C->64->128->256->512, AdaptiveAvgPool -> Linear(512,1) -> (B,)
//! The sequence discriminator is the same stack with the pool over frequency
//! only, so a bad transition is scored where it happens rather than averaged
//! away.

use candle_core::{DType, Device, Result, Tensor, Var, D};
use std::collections::HashMap;

use crate::config::Config;

// ----------------------------------------------------------------------------
// small helpers
// ----------------------------------------------------------------------------

/// N(0, 0.02) init, matching the Python `weights_init` for conv layers.
fn randn_var(shape: &[usize], dev: &Device) -> Result<Var> {
    Var::randn(0f32, 0.02f32, shape, dev)
}

/// Collect a Var's underlying tensor into a flat name->tensor map for saving.
fn put(map: &mut HashMap<String, Tensor>, name: &str, v: &Tensor) {
    map.insert(name.to_string(), v.clone());
}

// ----------------------------------------------------------------------------
// Linear
// ----------------------------------------------------------------------------

pub struct Linear {
    w: Var, // (out, in)
    b: Var, // (out,)
}

impl Linear {
    fn new(in_f: usize, out_f: usize, dev: &Device) -> Result<Self> {
        Ok(Self {
            w: randn_var(&[out_f, in_f], dev)?,
            b: Var::zeros(&[out_f], DType::F32, dev)?,
        })
    }
    /// x: (B, in) -> (B, out)
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let y = x.matmul(&self.w.as_tensor().t()?)?;
        y.broadcast_add(self.b.as_tensor())
    }
    fn vars(&self, out: &mut Vec<Var>) {
        out.push(self.w.clone());
        out.push(self.b.clone());
    }
    fn collect(&self, p: &str, m: &mut HashMap<String, Tensor>) {
        put(m, &format!("{p}.w"), self.w.as_tensor());
        put(m, &format!("{p}.b"), self.b.as_tensor());
    }
    fn load(&mut self, p: &str, m: &HashMap<String, Tensor>) -> Result<()> {
        self.w.set(&m[&format!("{p}.w")])?;
        self.b.set(&m[&format!("{p}.b")])?;
        Ok(())
    }
}

// ----------------------------------------------------------------------------
// Conv2d  (weight (out, in/groups, k, k), no bias — matches bias=False)
// ----------------------------------------------------------------------------

pub struct Conv2d {
    w: Var,
    stride: usize,
    padding: usize,
    groups: usize,
}

impl Conv2d {
    fn new(
        cin: usize,
        cout: usize,
        k: usize,
        stride: usize,
        padding: usize,
        groups: usize,
        dev: &Device,
    ) -> Result<Self> {
        Ok(Self {
            w: randn_var(&[cout, cin / groups, k, k], dev)?,
            stride,
            padding,
            groups,
        })
    }
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        x.conv2d(self.w.as_tensor(), self.padding, self.stride, 1, self.groups)
    }
    fn vars(&self, out: &mut Vec<Var>) {
        out.push(self.w.clone());
    }
    fn collect(&self, p: &str, m: &mut HashMap<String, Tensor>) {
        put(m, &format!("{p}.w"), self.w.as_tensor());
    }
    fn load(&mut self, p: &str, m: &HashMap<String, Tensor>) -> Result<()> {
        self.w.set(&m[&format!("{p}.w")])?;
        Ok(())
    }
}

// ----------------------------------------------------------------------------
// ConvTranspose2d  (weight (in, out, k, k), no bias)
// ----------------------------------------------------------------------------

pub struct ConvT2d {
    w: Var,
    stride: usize,
    padding: usize,
    out_padding: usize,
}

impl ConvT2d {
    fn new(
        cin: usize,
        cout: usize,
        k: usize,
        stride: usize,
        padding: usize,
        out_padding: usize,
        dev: &Device,
    ) -> Result<Self> {
        Ok(Self {
            w: randn_var(&[cin, cout, k, k], dev)?,
            stride,
            padding,
            out_padding,
        })
    }
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        // BEATSPACE_DECONV=upsample routes through zero-insert + conv2d, which
        // is the same operation on a different candle kernel.
        //
        // conv_transpose2d is a naive six-deep loop parallel only over c_out;
        // conv2d is im2col + gemm parallel over BATCH. At the final layer
        // c_out is 1, so the transposed path runs on one thread while the
        // conv2d path would run on all of them.
        //
        // OFF BY DEFAULT and measured rather than assumed. The first phase
        // report showed G forward at 2.2s of 145 — so the forward deconv is
        // NOT the bottleneck and switching it blind would be a change with no
        // evidence. The flag exists so the same run can be timed both ways;
        // where it should pay is the BACKWARD, since the gradient of a
        // convolution is a transposed convolution and the G backward is 12x
        // its forward.
        if std::env::var("BEATSPACE_DECONV").as_deref() == Ok("upsample") {
            return self.forward_via_conv2d(x);
        }
        x.conv_transpose2d(
            self.w.as_tensor(),
            self.padding,
            self.out_padding,
            self.stride,
            1,
        )
    }

    /// A transposed convolution as zero-insert upsample + ordinary convolution.
    ///
    /// Identical arithmetic: inserting `stride-1` zeros between input samples
    /// and convolving with the kernel flipped, at padding `k - 1 - p`, is the
    /// textbook equivalence. The cost is the zeros — the intermediate is
    /// `stride^2` times larger and most of it is zero, so this trades memory
    /// bandwidth for parallelism and is not obviously a win at every shape.
    ///
    /// It should win exactly where c_out is small, which is the last layer of
    /// this generator and the first layer's gradient in the discriminator.
    fn forward_via_conv2d(&self, x: &Tensor) -> Result<Tensor> {
        let w = self.w.as_tensor();          // (c_in, c_out, kh, kw)
        let (_, _, kh, kw) = w.dims4()?;
        let s = self.stride;

        // Zero-insert. upsample_nearest2d would REPEAT samples, which is a
        // different operation and a silently wrong one — the zeros are what
        // make this equal to a transposed convolution.
        let x = if s > 1 {
            let (b, c, h, wd) = x.dims4()?;
            let z = Tensor::zeros((b, c, h, wd, s * s - 1), x.dtype(), x.device())?;
            let spread = Tensor::cat(&[x.reshape((b, c, h, wd, 1))?, z], 4)?
                .reshape((b, c, h, wd * s, s))?
                .transpose(3, 4)?
                .reshape((b, c, h * s, wd * s))?;
            // The trailing row/column of zeros is not part of the output.
            spread.narrow(2, 0, h * s - (s - 1))?.narrow(3, 0, wd * s - (s - 1))?
        } else {
            x.clone()
        };

        // The kernel: swap in/out channels and flip both spatial axes.
        // Contiguous before index_select, which candle requires — a transpose
        // leaves a strided view and the flip below reads through it.
        let mut k = w.transpose(0, 1)?.contiguous()?;
        for axis in [2usize, 3] {
            let n = if axis == 2 { kh } else { kw };
            let idx = Tensor::from_iter((0..n).rev().map(|i| i as u32), w.device())?;
            k = k.index_select(&idx, axis)?.contiguous()?;
        }
        let pad = (kh - 1).saturating_sub(self.padding);
        let y = x.conv2d(&k.contiguous()?, pad, 1, 1, 1)?;
        if self.out_padding > 0 {
            let (_, _, h, wd) = y.dims4()?;
            return y.narrow(2, 0, h + self.out_padding)?.narrow(3, 0, wd + self.out_padding);
        }
        Ok(y)
    }
    fn vars(&self, out: &mut Vec<Var>) {
        out.push(self.w.clone());
    }
    fn collect(&self, p: &str, m: &mut HashMap<String, Tensor>) {
        put(m, &format!("{p}.w"), self.w.as_tensor());
    }
    fn load(&mut self, p: &str, m: &HashMap<String, Tensor>) -> Result<()> {
        self.w.set(&m[&format!("{p}.w")])?;
        Ok(())
    }
}

// ----------------------------------------------------------------------------
// BatchNorm2d  (manual; persists gamma/beta + running mean/var)
// ----------------------------------------------------------------------------

pub struct BatchNorm2d {
    gamma: Var, // (C,)
    beta: Var,  // (C,)
    run_mean: Var,
    run_var: Var,
    eps: f64,
    momentum: f64,
    c: usize,
}

impl BatchNorm2d {
    fn new(c: usize, dev: &Device) -> Result<Self> {
        Ok(Self {
            // Python weights_init: BN weight ~ N(1, 0.02), bias = 0
            gamma: Var::from_tensor(
                &(Tensor::randn(0f32, 0.02f32, &[c], dev)? + 1.0)?,
            )?,
            beta: Var::zeros(&[c], DType::F32, dev)?,
            run_mean: Var::zeros(&[c], DType::F32, dev)?,
            run_var: Var::from_tensor(&Tensor::ones(&[c], DType::F32, dev)?)?,
            eps: 1e-5,
            momentum: 0.1,
            c,
        })
    }

    /// x: (B, C, H, W). In train mode, normalize with batch stats and update the
    /// running estimates; in eval mode, normalize with the stored running stats.
    fn forward(&self, x: &Tensor, train: bool) -> Result<Tensor> {
        let c = self.c;
        // reduce over batch(0), height(2), width(3) -> per-channel (C,)
        let (mean, var, centered) = if train {
            let mean = x.mean((0usize, 2, 3))?; // (C,)
            let mean_b = mean.reshape((1, c, 1, 1))?;
            let centered = x.broadcast_sub(&mean_b)?;
            let var = centered.sqr()?.mean((0usize, 2, 3))?; // biased, like PyTorch fwd
            // update running stats (detached): run = (1-m)*run + m*batch
            let m = self.momentum;
            let new_mean =
                ((self.run_mean.as_tensor() * (1.0 - m))? + (&mean * m)?)?;
            let new_var = ((self.run_var.as_tensor() * (1.0 - m))? + (&var * m)?)?;
            self.run_mean.set(&new_mean.detach())?;
            self.run_var.set(&new_var.detach())?;
            // CENTERED IS REUSED, not recomputed. It was built here for the
            // variance, dropped, and then `x.broadcast_sub(&mean)` ran again
            // below — a second full-tensor pass over up to 8.4M elements, on a
            // backend whose elementwise kernels are single-threaded
            // (cpu_backend/utils.rs walks a plain .iter(), no rayon).
            (mean, var, Some(centered))
        } else {
            (
                self.run_mean.as_tensor().clone(),
                self.run_var.as_tensor().clone(),
                None,
            )
        };
        let var = var.reshape((1, c, 1, 1))?;
        let g = self.gamma.as_tensor().reshape((1, c, 1, 1))?;
        let b = self.beta.as_tensor().reshape((1, c, 1, 1))?;

        // SCALE AND SHIFT FOLDED. The tail was div -> mul -> add: three passes
        // where the arithmetic is one affine transform per channel. Since
        //
        //     y = (x - mu)/sqrt(v + eps) * gamma + beta
        //       = (x - mu) * (gamma / sqrt(v + eps)) + beta
        //
        // the divide and the gamma multiply collapse into ONE per-channel
        // factor, computed on a (1,C,1,1) tensor rather than on the full one.
        // Two full-tensor passes instead of three, and the saved one is a
        // divide.
        let scale = g.broadcast_div(&var.affine(1.0, self.eps)?.sqrt()?)?;
        // Named `cen` rather than `c`: `c` is the CHANNEL COUNT in this
        // scope, and shadowing it inside the match arm would have made the
        // reshape below read the tensor instead. It compiled either way.
        let centered = match centered {
            Some(cen) => cen,
            None => {
                let mean = mean.reshape((1, c, 1, 1))?;
                x.broadcast_sub(&mean)?
            }
        };
        centered.broadcast_mul(&scale)?.broadcast_add(&b)
    }

    fn vars(&self, out: &mut Vec<Var>) {
        // Only gamma/beta are trainable; running stats are buffers (no grad).
        out.push(self.gamma.clone());
        out.push(self.beta.clone());
    }
    fn collect(&self, p: &str, m: &mut HashMap<String, Tensor>) {
        put(m, &format!("{p}.gamma"), self.gamma.as_tensor());
        put(m, &format!("{p}.beta"), self.beta.as_tensor());
        put(m, &format!("{p}.run_mean"), self.run_mean.as_tensor());
        put(m, &format!("{p}.run_var"), self.run_var.as_tensor());
    }
    fn load(&mut self, p: &str, m: &HashMap<String, Tensor>) -> Result<()> {
        self.gamma.set(&m[&format!("{p}.gamma")])?;
        self.beta.set(&m[&format!("{p}.beta")])?;
        self.run_mean.set(&m[&format!("{p}.run_mean")])?;
        self.run_var.set(&m[&format!("{p}.run_var")])?;
        Ok(())
    }
}

// ----------------------------------------------------------------------------
// Generator
// ----------------------------------------------------------------------------

/// One transposed-conv stage, with its BatchNorm folded into a per-channel
/// affine (`scale`/`shift`) and the activation it should apply. Plain data, no
/// candle/wgpu types — this is what the GPU backend uploads.
#[cfg(feature = "gpu")]
pub struct GpuDeconv {
    pub cin: usize,
    pub cout: usize,
    pub k: usize,
    pub stride: usize,
    pub padding: usize,
    pub w: Vec<f32>,     // (cin, cout, k, k) row-major
    pub scale: Vec<f32>, // (cout,)  folded BN scale  (1.0 if no BN)
    pub shift: Vec<f32>, // (cout,)  folded BN shift  (0.0 if no BN)
    pub act: u32,        // 1 = ReLU, 2 = Tanh
}

/// The whole generator flattened for a non-candle (GPU) forward pass.
#[cfg(feature = "gpu")]
pub struct GenParams {
    pub latent: usize,
    pub f0: usize,
    pub h0: usize,
    pub w0: usize,
    pub n_mels: usize,
    pub time_frames: usize,
    pub fc_w: Vec<f32>, // (f0, latent) row-major  (out, in)
    pub fc_b: Vec<f32>, // (f0,)
    pub deconvs: Vec<GpuDeconv>,
}

/// What the conditional renderer is given besides `z`.
///
/// Both parts are optional individually, and a generator built with a part
/// disabled ignores it; a generator built with it ENABLED refuses to run
/// without it, because a silently-zeroed context would train a model that
/// looks conditional and is not.
#[derive(Default)]
pub struct Cond {
    /// `(B, stems, n_mels, ctx_frames)`: the tail of the previous window.
    pub ctx: Option<Tensor>,
    /// `(B, time_frames)`: beat phase of each output column, in [0, 1).
    pub phase: Option<Tensor>,
}

/// Reduces the previous window's tail to a vector the renderer conditions on.
///
/// Two strided 3x3 convolutions, mean-pooled, then a linear map with tanh.
/// Deliberately small: its job is "what was playing", not a reconstruction —
/// the discriminators judge whether the continuation fits, and gradient
/// reaches here through the renderer.
pub struct CtxEncoder {
    c0: Conv2d,
    c1: Conv2d,
    fc: Linear,
}

impl CtxEncoder {
    fn new(stems: usize, ctx_dim: usize, dev: &Device) -> Result<Self> {
        Ok(Self {
            c0: Conv2d::new(stems, 16, 3, 2, 1, 1, dev)?,
            c1: Conv2d::new(16, 32, 3, 2, 1, 1, dev)?,
            fc: Linear::new(32, ctx_dim, dev)?,
        })
    }
    /// `(B, stems, n_mels, ctx_frames)` -> `(B, ctx_dim)`.
    fn forward(&self, ctx: &Tensor) -> Result<Tensor> {
        let leaky = |x: Tensor| -> Result<Tensor> { x.maximum(&(&x * 0.2)?) };
        let x = leaky(self.c0.forward(ctx)?)?;
        let x = leaky(self.c1.forward(&x)?)?;
        let x = x.mean(D::Minus1)?.mean(D::Minus1)?; // (B, 32)
        self.fc.forward(&x)?.tanh()
    }
    fn vars(&self, out: &mut Vec<Var>) {
        self.c0.vars(out);
        self.c1.vars(out);
        self.fc.vars(out);
    }
    fn collect(&self, p: &str, m: &mut HashMap<String, Tensor>) {
        self.c0.collect(&format!("{p}.c0"), m);
        self.c1.collect(&format!("{p}.c1"), m);
        self.fc.collect(&format!("{p}.fc"), m);
    }
    fn load(&mut self, p: &str, m: &HashMap<String, Tensor>) -> Result<()> {
        self.c0.load(&format!("{p}.c0"), m)?;
        self.c1.load(&format!("{p}.c1"), m)?;
        self.fc.load(&format!("{p}.fc"), m)
    }
}

pub struct Generator {
    fc: Linear,
    deconvs: Vec<ConvT2d>,
    bns: Vec<BatchNorm2d>, // one fewer than deconvs (last deconv has no BN)
    f0: usize,             // channels after fc reshape (= g_filters)
    h0: usize,             // 8
    w0: usize,             // 16
    /// Present when `ctx_frames > 0`.
    ctx_enc: Option<CtxEncoder>,
    /// Two extra input planes on the last deconvolution when `beat_phase`.
    phase_planes: bool,
    stems: usize,
}

impl Generator {
    pub fn new(cfg: &Config, dev: &Device) -> Result<Self> {
        let f = cfg.g_filters; // 256
        let h0 = cfg.n_mels / 16; // 128/16 = 8
        let w0 = cfg.time_frames / 16; // 256/16 = 16
        let ctx_enc = if cfg.ctx_frames > 0 {
            Some(CtxEncoder::new(cfg.stems, cfg.ctx_dim, dev)?)
        } else {
            None
        };
        let fc_in = cfg.latent + if ctx_enc.is_some() { cfg.ctx_dim } else { 0 };
        let fc = Linear::new(fc_in, f * h0 * w0, dev)?;

        // Deconv channel schedule: 256 -> 128 -> 64 -> 32 (BN+ReLU each),
        // then a final 32 -> stems layer with Tanh. The final layer also
        // reads the two beat-phase planes when they are on — at that point
        // the grid is n_mels/2 by time_frames/2, one column per two frames,
        // which is fine enough to place a kick on a beat.
        let stage_out = [f / 2, f / 4, f / 8, cfg.stems]; // 128,64,32,C
        let mut deconvs = Vec::new();
        let mut bns = Vec::new();
        let mut cin = f;
        for (i, &cout) in stage_out.iter().enumerate() {
            let last = i == stage_out.len() - 1;
            let extra = if last && cfg.beat_phase { 2 } else { 0 };
            deconvs.push(ConvT2d::new(cin + extra, cout, 4, 2, 1, 0, dev)?);
            if !last {
                bns.push(BatchNorm2d::new(cout, dev)?);
            }
            cin = cout;
        }
        Ok(Self {
            fc,
            deconvs,
            bns,
            f0: f,
            h0,
            w0,
            ctx_enc,
            phase_planes: cfg.beat_phase,
            stems: cfg.stems,
        })
    }

    /// Is this generator conditional at all? The legacy entry points refuse
    /// to run one that is.
    pub fn conditional(&self) -> bool {
        self.ctx_enc.is_some() || self.phase_planes
    }

    /// Beat phase `(B, T)` in [0, 1) -> sin/cos planes `(B, 2, h, w)` where
    /// `w = T/2`, each column the mean of two frames' sin and cos. Averaged
    /// on the circle rather than on the phase value: the mean of 0.99 and
    /// 0.01 is not 0.5.
    fn phase_planes(phase: &Tensor, h: usize, w: usize) -> Result<Tensor> {
        let (b, t) = phase.dims2()?;
        if t != 2 * w {
            candle_core::bail!("beat phase has {t} columns, the last stage wants {}", 2 * w);
        }
        let ang = (phase * (2.0 * std::f64::consts::PI))?;
        let mut planes = Vec::with_capacity(2);
        for p in [ang.sin()?, ang.cos()?] {
            let p = p.reshape((b, w, 2))?.mean(D::Minus1)?; // (B, w)
            planes.push(p.reshape((b, 1, 1, w))?.broadcast_as((b, 1, h, w))?.contiguous()?);
        }
        Tensor::cat(&planes, 1)
    }

    fn forward(&self, z: &Tensor, cond: Option<&Cond>, train: bool) -> Result<Tensor> {
        let b = z.dim(0)?;
        let zin = match &self.ctx_enc {
            Some(enc) => {
                let ctx = cond.and_then(|c| c.ctx.as_ref()).ok_or_else(|| {
                    candle_core::Error::Msg("this generator was built with ctx_frames > 0 and needs Cond::ctx".into())
                })?;
                Tensor::cat(&[z, &enc.forward(ctx)?], 1)?
            }
            None => z.clone(),
        };
        let mut x = self.fc.forward(&zin)?; // (B, f*h0*w0)
        x = x.reshape((b, self.f0, self.h0, self.w0))?;
        let n = self.deconvs.len();
        for i in 0..n {
            if i == n - 1 && self.phase_planes {
                let phase = cond.and_then(|c| c.phase.as_ref()).ok_or_else(|| {
                    candle_core::Error::Msg("this generator was built with beat_phase and needs Cond::phase".into())
                })?;
                let (_, _, h, w) = x.dims4()?;
                x = Tensor::cat(&[&x, &Self::phase_planes(phase, h, w)?], 1)?;
            }
            x = self.deconvs[i].forward(&x)?;
            if i < n - 1 {
                x = self.bns[i].forward(&x, train)?;
                x = x.relu()?;
            } else {
                x = x.tanh()?; // final layer -> [-1,1]
            }
        }
        Ok(x) // (B,stems,n_mels,time_frames)
    }

    /// The legacy unconditional entry points. They fail loudly on a
    /// conditional generator rather than fabricating a context.
    pub fn forward_train(&self, z: &Tensor) -> Result<Tensor> {
        self.forward(z, None, true)
    }
    pub fn forward_eval(&self, z: &Tensor) -> Result<Tensor> {
        self.forward(z, None, false)
    }
    pub fn forward_train_cond(&self, z: &Tensor, cond: &Cond) -> Result<Tensor> {
        self.forward(z, Some(cond), true)
    }
    pub fn forward_eval_cond(&self, z: &Tensor, cond: &Cond) -> Result<Tensor> {
        self.forward(z, Some(cond), false)
    }

    /// Convenience for the GUI: one latent row -> mel as a flat (n_mels*T) Vec.
    /// Unconditional, one stem — the loop player's contract.
    pub fn generate(&self, z: &[f32], dev: &Device) -> Result<(Vec<f32>, usize, usize)> {
        if self.conditional() {
            candle_core::bail!("generate() is the loop player's entry point; this generator is conditional and needs generate_next()");
        }
        let latent = z.len();
        let zt = Tensor::from_vec(z.to_vec(), (1, latent), dev)?;
        let mel = self.forward_eval(&zt)?; // (1,1,n_mels,time_frames)
        let mel = mel.squeeze(0)?.squeeze(0)?; // (n_mels,time_frames)
        let (n_mels, t) = mel.dims2()?;
        let flat = mel.flatten_all()?.to_vec1::<f32>()?;
        Ok((flat, n_mels, t))
    }

    /// One window of a stream: `z`, the previous window's planes (from which
    /// the last `ctx_frames` columns are taken) and the beat phase of each
    /// new column. Returns `stems` planes of `(n_mels x T)` laid end to end,
    /// plus `(n_mels, t)`.
    pub fn generate_next(
        &self,
        z: &[f32],
        prev: Option<&[f32]>,
        ctx_frames: usize,
        phase: Option<&[f32]>,
        n_mels: usize,
        t: usize,
        dev: &Device,
    ) -> Result<(Vec<f32>, usize, usize)> {
        let zt = Tensor::from_vec(z.to_vec(), (1, z.len()), dev)?;
        let mut cond = Cond::default();
        if self.ctx_enc.is_some() {
            let prev = prev.ok_or_else(|| candle_core::Error::Msg("generate_next needs the previous window".into()))?;
            let full = Tensor::from_vec(prev.to_vec(), (1, self.stems, n_mels, t), dev)?;
            cond.ctx = Some(full.narrow(3, t - ctx_frames, ctx_frames)?.contiguous()?);
        }
        if self.phase_planes {
            let ph = phase.ok_or_else(|| candle_core::Error::Msg("generate_next needs a beat phase".into()))?;
            cond.phase = Some(Tensor::from_vec(ph.to_vec(), (1, t), dev)?);
        }
        let mel = self.forward_eval_cond(&zt, &cond)?; // (1,stems,n_mels,t)
        let flat = mel.flatten_all()?.to_vec1::<f32>()?;
        Ok((flat, n_mels, t))
    }

    /// Flatten all weights (folding eval-time BatchNorm into per-channel
    /// scale/shift) for an external GPU forward pass. CPU-side; cheap.
    #[cfg(feature = "gpu")]
    pub fn export_params(&self) -> Result<GenParams> {
        // The wgpu path implements fc + four deconvolutions on one latent.
        // A context encoder, phase planes on the last stage and a multi-stem
        // output are three shader changes it does not have, and it cannot be
        // validated here (no GPU), so it says so rather than exporting a
        // model whose weights it would then read wrong.
        if self.conditional() || self.stems != 1 {
            candle_core::bail!(
                "the GPU generator path only supports the unconditional one-stem model; \
this one has ctx {} / beat_phase {} / stems {}",
                self.ctx_enc.is_some(), self.phase_planes, self.stems
            );
        }
        let (f0, latent) = self.fc.w.as_tensor().dims2()?;
        let fc_w = self.fc.w.as_tensor().flatten_all()?.to_vec1::<f32>()?;
        let fc_b = self.fc.b.as_tensor().flatten_all()?.to_vec1::<f32>()?;
        let eps = 1e-5f32;

        let mut deconvs = Vec::new();
        for (i, dc) in self.deconvs.iter().enumerate() {
            let (cin, cout, k, _k2) = dc.w.as_tensor().dims4()?;
            let w = dc.w.as_tensor().flatten_all()?.to_vec1::<f32>()?;
            let (scale, shift, act) = if i < self.bns.len() {
                // fold BatchNorm(eval): y = (x-mean)/sqrt(var+eps)*gamma + beta
                let bn = &self.bns[i];
                let gamma = bn.gamma.as_tensor().flatten_all()?.to_vec1::<f32>()?;
                let beta = bn.beta.as_tensor().flatten_all()?.to_vec1::<f32>()?;
                let rm = bn.run_mean.as_tensor().flatten_all()?.to_vec1::<f32>()?;
                let rv = bn.run_var.as_tensor().flatten_all()?.to_vec1::<f32>()?;
                let mut sc = vec![0f32; cout];
                let mut sh = vec![0f32; cout];
                for c in 0..cout {
                    let s = gamma[c] / (rv[c] + eps).sqrt();
                    sc[c] = s;
                    sh[c] = beta[c] - rm[c] * s;
                }
                (sc, sh, 1u32) // ReLU after BN
            } else {
                (vec![1.0f32; cout], vec![0.0f32; cout], 2u32) // final: Tanh, no BN
            };
            deconvs.push(GpuDeconv {
                cin,
                cout,
                k,
                stride: dc.stride,
                padding: dc.padding,
                w,
                scale,
                shift,
                act,
            });
        }

        Ok(GenParams {
            latent,
            f0,
            h0: self.h0,
            w0: self.w0,
            n_mels: self.h0 * 16,
            time_frames: self.w0 * 16,
            fc_w,
            fc_b,
            deconvs,
        })
    }

    pub fn trainable_vars(&self) -> Vec<Var> {
        let mut v = Vec::new();
        self.fc.vars(&mut v);
        for d in &self.deconvs {
            d.vars(&mut v);
        }
        for b in &self.bns {
            b.vars(&mut v);
        }
        if let Some(e) = &self.ctx_enc {
            e.vars(&mut v);
        }
        v
    }

    pub fn state_dict(&self) -> HashMap<String, Tensor> {
        let mut m = HashMap::new();
        self.fc.collect("g.fc", &mut m);
        for (i, d) in self.deconvs.iter().enumerate() {
            d.collect(&format!("g.deconv{i}"), &mut m);
        }
        for (i, b) in self.bns.iter().enumerate() {
            b.collect(&format!("g.bn{i}"), &mut m);
        }
        if let Some(e) = &self.ctx_enc {
            e.collect("g.ctx", &mut m);
        }
        m
    }

    pub fn load_state(&mut self, m: &HashMap<String, Tensor>) -> Result<()> {
        self.fc.load("g.fc", m)?;
        for (i, d) in self.deconvs.iter_mut().enumerate() {
            d.load(&format!("g.deconv{i}"), m)?;
        }
        for (i, b) in self.bns.iter_mut().enumerate() {
            b.load(&format!("g.bn{i}"), m)?;
        }
        if let Some(e) = &mut self.ctx_enc {
            if !m.contains_key("g.ctx.fc.w") {
                candle_core::bail!("checkpoint has no context encoder (g.ctx.*) but this run has ctx_frames > 0");
            }
            e.load("g.ctx", m)?;
        }
        Ok(())
    }
}

// ----------------------------------------------------------------------------
// SparseBlock (discriminator building block)
//   depthwise conv -> activation mask (|x|>thr) -> pointwise conv -> BN -> ReLU
// ----------------------------------------------------------------------------

pub struct SparseBlock {
    depthwise: Conv2d,
    pointwise: Conv2d,
    bn: BatchNorm2d,
    threshold: f32,
    /// Gate steepness. 40.0 in the reference pipeline.
    sharpness: f32,
}

/// DEPTHWISE CONVOLUTION IS A PESSIMISATION ON THIS BACKEND, and it is where
/// the discriminator's time goes.
///
/// candle implements a grouped convolution by SPLITTING it (conv.rs:193):
///
/// ```text
/// let blocks = self.chunk(groups, 1)?;
/// let kernel = kernel.chunk(groups, 0)?;
/// blocks.iter().zip(&kernel).map(|(b, k)| b.conv2d_single_group(k, &params))
/// ```
///
/// `.map()`, not `par_iter().map()` — so `groups == c_in` becomes c_in
/// SEQUENTIAL conv2d calls, each on a single-channel tensor. Per discriminator
/// forward, across channels 1 -> 64 -> 128 -> 256, that is
/// 1 + 64 + 128 + 256 = 449 separate convolutions. Three D forwards per step,
/// 64 steps: ~86,000 convolution invocations an epoch, and as many again in
/// the backward.
///
/// Each one is tiny, so im2col setup dominates its own gemm and the batch-level
/// parallelism inside it has almost nothing to chew on. That is why the
/// measured D forward is 208 ms while the generator's forward — which does MORE
/// multiply-accumulates, through the supposedly naive transposed-convolution
/// kernel — is 34 ms.
///
/// Depthwise separable convolutions save FLOPs and are a win on hardware with a
/// fused depthwise kernel: cuDNN, Metal Performance Shaders, mobile NPUs. On a
/// backend that emulates groups by splitting, they trade one large matmul for
/// hundreds of small serial ones, and FLOPs saved is the wrong thing to have
/// optimised.
///
/// BEATSPACE_DENSE_D=1 builds a single dense convolution instead. It is NOT
/// checkpoint-compatible — different parameter shapes, so a run started one way
/// cannot resume the other — which is why it is a flag and not a change.
/// `x * sigmoid(sharpness * (|x| - t))`, in one traversal.
///
/// A SHARP SIGMOID, NOT A HARD STEP, and this is the bug that killed the
/// 269-epoch run. The port implemented v5.2's gate:
///
///     x * (x.abs() > threshold).float()
///
/// and v5.3 of the reference pipeline replaced it precisely because that
/// collapses the discriminator. Its own comment, worth quoting in full because
/// it describes the failure that was observed here exactly:
///
///   "the gate used to be a hard step, (x.abs() > threshold).float(), which has
///    EXACTLY ZERO gradient for every unit below threshold. Verified that even
///    at random init 80-99.9% of units in a block fall below threshold, and
///    that fraction only grows deeper in the network as training progresses --
///    units below threshold can never receive a gradient to recover, so the
///    whole discriminator can (and did) collapse into a permanently frozen,
///    input-independent fixed point: bit-identical D(real)/D(fake)/losses for
///    tens of thousands of iterations straight."
///
/// That is the observed signature: d_loss pinned at 2ln2, g_loss at ln2, flat
/// for forty points, after a vertical fall. A frozen input-independent D scores
/// exactly there.
///
/// The sigmoid is near-zero below threshold and near-one above, so the forward
/// behaviour is nearly the same — but its gradient is never exactly zero, so a
/// dead unit can still receive a small gradient and recover.
///
/// One CustomOp1 rather than a chain, for the reason the hard version was: the
/// chain is four passes and four allocations over a tensor that reaches 8.4M
/// elements, and candle's elementwise kernels are single-threaded.
struct SparseGate {
    thr: f32,
    /// 40.0 in the reference. High enough that the gate is effectively binary
    /// in the forward pass, low enough that the derivative below threshold is
    /// small rather than zero.
    sharpness: f32,
}

impl candle_core::CustomOp1 for SparseGate {
    fn name(&self) -> &'static str { "sparse-gate" }

    fn cpu_fwd(&self, s: &candle_core::CpuStorage, l: &candle_core::Layout)
        -> Result<(candle_core::CpuStorage, candle_core::Shape)> {
        let thr = self.thr;
        let out = match s {
            candle_core::CpuStorage::F32(v) => {
                // start_offset honoured, because a narrowed or transposed
                // tensor arrives here as a view and reading from index 0 would
                // gate the wrong elements.
                let src = match l.contiguous_offsets() {
                    Some((a, b)) => &v[a..b],
                    None => candle_core::bail!("sparse-gate wants a contiguous input"),
                };
                let s = self.sharpness;
                candle_core::CpuStorage::F32(
                    src.iter().map(|&x| x / (1.0 + (-s * (x.abs() - thr)).exp())).collect())
            }
            _ => candle_core::bail!("sparse-gate is f32 only"),
        };
        Ok((out, l.shape().clone()))
    }

    /// The derivative of `x * sigma(s(|x| - t))`.
    ///
    /// By the product rule, with `u = s(|x| - t)` and `g = sigma(u)`:
    ///
    ///     dy/dx = g + x * g(1-g) * s * sign(x)
    ///
    /// The second term is what the hard step threw away, and it is the whole
    /// point: it is non-zero for every unit, so one that has fallen below
    /// threshold still receives gradient and can climb back. A step function's
    /// derivative is zero there, which is why those units died permanently.
    ///
    /// Reads the ARGUMENT rather than the result: at x = 0 both are zero and
    /// the result cannot say what the input was.
    fn bwd(&self, arg: &Tensor, _res: &Tensor, grad: &Tensor) -> Result<Option<Tensor>> {
        let s = self.sharpness as f64;
        let u = arg.abs()?.affine(s, -(self.thr as f64) * s)?;   // s(|x| - t)
        let g = candle_nn::ops::sigmoid(&u)?;
        let one_minus = g.affine(-1.0, 1.0)?;
        // sign(x), and 0 at exactly 0 — where the term vanishes anyway.
        let sign = arg.sign()?;
        let second = arg.mul(&g)?.mul(&one_minus)?.affine(s, 0.0)?.mul(&sign)?;
        Ok(Some(grad.mul(&g.add(&second)?)?))
    }
}

impl SparseBlock {
    fn new(cin: usize, cout: usize, stride: usize, threshold: f32, dev: &Device) -> Result<Self> {
        // One dense conv rather than depthwise + pointwise. More parameters and
        // more FLOPs, on a path that is one gemm instead of `cin` of them.
        if std::env::var("BEATSPACE_DENSE_D").as_deref() == Ok("1") {
            return Ok(Self {
                // groups = 1, so this reaches conv2d_im2col_gemm directly.
                depthwise: Conv2d::new(cin, cout, 3, stride, 1, 1, dev)?,
                // Identity-shaped 1x1: kept so the struct, the var list and the
                // checkpoint keys stay one shape, rather than forking every
                // function that walks a block.
                pointwise: Conv2d::new(cout, cout, 1, 1, 0, 1, dev)?,
                bn: BatchNorm2d::new(cout, dev)?,
                threshold,
                sharpness: 40.0,
            });
        }
        Ok(Self {
            // depthwise: groups == cin, k3
            depthwise: Conv2d::new(cin, cin, 3, stride, 1, cin, dev)?,
            // pointwise: 1x1
            pointwise: Conv2d::new(cin, cout, 1, 1, 0, 1, dev)?,
            bn: BatchNorm2d::new(cout, dev)?,
            threshold,
            sharpness: 40.0,
        })
    }

    fn forward(&self, x: &Tensor, train: bool) -> Result<Tensor> {
        let x = self.depthwise.forward(x)?;
        // THE SPARSITY MASK, in one pass instead of four.
        //
        // It was abs() -> gt() -> to_dtype() -> mul(): four full-tensor
        // traversals, each allocating a whole new tensor, and candle's
        // elementwise ops are SINGLE-THREADED — cpu_backend/utils.rs uses a
        // plain `.iter()`, with no rayon anywhere in unary_map or binary_map.
        //
        // On the discriminator's first block that tensor is
        // (16, 64, 64, 128) = 8.4M elements, so the four-pass version moved
        // 134 MB through one core per block per forward, three forwards a
        // step. The threshold test is the same arithmetic either way; what is
        // saved is three traversals and three allocations.
        //
        // Numerically identical: |x| > t keeps x, else 0. Written with a
        // single custom unary so the comparison and the select happen in the
        // same pass over the data.
        let thr = self.threshold;
        let x = x.apply_op1(SparseGate { thr, sharpness: self.sharpness })?;
        let x = self.pointwise.forward(&x)?;
        let x = self.bn.forward(&x, train)?;
        // LEAKY relu at 0.2, matching the reference. A plain relu is the same
        // dead-unit trap as the hard gate one line up: a channel driven
        // negative has zero gradient forever, and this block already had one
        // way to freeze.
        x.maximum(&(&x * 0.2)?)
    }

    fn vars(&self, out: &mut Vec<Var>) {
        self.depthwise.vars(out);
        self.pointwise.vars(out);
        self.bn.vars(out);
    }
    fn collect(&self, p: &str, m: &mut HashMap<String, Tensor>) {
        self.depthwise.collect(&format!("{p}.dw"), m);
        self.pointwise.collect(&format!("{p}.pw"), m);
        self.bn.collect(&format!("{p}.bn"), m);
    }
    fn load(&mut self, p: &str, m: &HashMap<String, Tensor>) -> Result<()> {
        self.depthwise.load(&format!("{p}.dw"), m)?;
        self.pointwise.load(&format!("{p}.pw"), m)?;
        self.bn.load(&format!("{p}.bn"), m)?;
        Ok(())
    }
}

// ----------------------------------------------------------------------------
// Discriminator
// ----------------------------------------------------------------------------

pub struct Discriminator {
    blocks: Vec<SparseBlock>,
    head: Linear, // (512 -> 1)
}

/// The four stride-2 SparseBlocks both discriminators share.
fn sparse_stack(cfg: &Config, dev: &Device) -> Result<Vec<SparseBlock>> {
    let d = cfg.d_filters; // 64
    // stems -> 64 -> 128 -> 256 -> 512
    let chans = [(cfg.stems, d), (d, d * 2), (d * 2, d * 4), (d * 4, d * 8)];
    let mut blocks = Vec::new();
    for &(cin, cout) in chans.iter() {
        blocks.push(SparseBlock::new(cin, cout, 2, cfg.sparse_threshold, dev)?);
    }
    Ok(blocks)
}

impl Discriminator {
    pub fn new(cfg: &Config, dev: &Device) -> Result<Self> {
        let blocks = sparse_stack(cfg, dev)?;
        let head = Linear::new(cfg.d_filters * 8, 1, dev)?;
        Ok(Self { blocks, head })
    }

    /// mel: (B,1,n_mels,time_frames) -> logits (B,)
    pub fn forward(&self, x: &Tensor, train: bool) -> Result<Tensor> {
        let mut x = x.clone();
        for b in &self.blocks {
            x = b.forward(&x, train)?;
        }
        // adaptive avg pool to 1x1: mean over H and W
        let x = x.mean(D::Minus1)?.mean(D::Minus1)?; // (B, 512)
        let x = self.head.forward(&x)?; // (B,1)
        x.squeeze(1) // (B,)
    }

    pub fn trainable_vars(&self) -> Vec<Var> {
        let mut v = Vec::new();
        for b in &self.blocks {
            b.vars(&mut v);
        }
        self.head.vars(&mut v);
        v
    }

    pub fn state_dict(&self) -> HashMap<String, Tensor> {
        let mut m = HashMap::new();
        for (i, b) in self.blocks.iter().enumerate() {
            b.collect(&format!("d.block{i}"), &mut m);
        }
        self.head.collect("d.head", &mut m);
        m
    }

    pub fn load_state(&mut self, m: &HashMap<String, Tensor>) -> Result<()> {
        for (i, b) in self.blocks.iter_mut().enumerate() {
            b.load(&format!("d.block{i}"), m)?;
        }
        self.head.load("d.head", m)?;
        Ok(())
    }
}

// ----------------------------------------------------------------------------
// Sequence discriminator
// ----------------------------------------------------------------------------

/// Scores a run of adjacent windows, one logit per time column.
///
/// The window discriminator ends in a global average pool, so it sees a
/// window as texture and cannot tell a seam from a bar of music: a bad
/// transition is a few columns' worth of evidence averaged into nothing.
/// This one pools over FREQUENCY only and puts a linear head on every
/// remaining column, so it answers "is the audio around here real" at each
/// point along the join — PatchGAN along time. Same SparseBlocks, so what it
/// has learned about texture is not learned twice.
pub struct SeqDiscriminator {
    blocks: Vec<SparseBlock>,
    head: Linear, // (512 -> 1), applied per column
}

impl SeqDiscriminator {
    pub fn new(cfg: &Config, dev: &Device) -> Result<Self> {
        let blocks = sparse_stack(cfg, dev)?;
        let head = Linear::new(cfg.d_filters * 8, 1, dev)?;
        Ok(Self { blocks, head })
    }

    /// mel `(B, stems, n_mels, W)` -> logits `(B, W/16)`.
    pub fn forward(&self, x: &Tensor, train: bool) -> Result<Tensor> {
        let mut x = x.clone();
        for b in &self.blocks {
            x = b.forward(&x, train)?;
        }
        let x = x.mean(2)?; // (B, 512, W')
        let (b, c, w) = x.dims3()?;
        // One matmul for every column of every sample rather than a batched
        // broadcast: (B*W', 512) @ (512, 1).
        let cols = x.transpose(1, 2)?.contiguous()?.reshape((b * w, c))?;
        self.head.forward(&cols)?.reshape((b, w))
    }

    pub fn trainable_vars(&self) -> Vec<Var> {
        let mut v = Vec::new();
        for b in &self.blocks {
            b.vars(&mut v);
        }
        self.head.vars(&mut v);
        v
    }

    pub fn state_dict(&self) -> HashMap<String, Tensor> {
        let mut m = HashMap::new();
        for (i, b) in self.blocks.iter().enumerate() {
            b.collect(&format!("sd.block{i}"), &mut m);
        }
        self.head.collect("sd.head", &mut m);
        m
    }

    pub fn load_state(&mut self, m: &HashMap<String, Tensor>) -> Result<()> {
        for (i, b) in self.blocks.iter_mut().enumerate() {
            b.load(&format!("sd.block{i}"), m)?;
        }
        self.head.load("sd.head", m)
    }
}

// ----------------------------------------------------------------------------
// Numerically-stable BCE with logits
//   loss = mean( max(x,0) - x*z + log(1 + exp(-|x|)) )
// candle's built-in does sigmoid-then-log which can NaN at saturation.
// ----------------------------------------------------------------------------

pub fn bce_with_logits(logits: &Tensor, target: &Tensor) -> Result<Tensor> {
    let max0 = logits.relu()?; // max(x,0)
    let xz = logits.mul(target)?; // x*z
    let absx = logits.abs()?;
    // log(1+exp(-|x|)) == softplus(-|x|)
    let softplus = absx.neg()?.exp()?.affine(1.0, 1.0)?.log()?;
    let loss = ((max0 - xz)? + softplus)?;
    loss.mean_all()
}

// ----------------------------------------------------------------------------
// checkpoint save/load helpers
// ----------------------------------------------------------------------------

pub fn save_generator(g: &Generator, path: &std::path::Path) -> Result<()> {
    let m = g.state_dict();
    candle_core::safetensors::save(&m, path)
}

pub fn save_discriminator(d: &Discriminator, path: &std::path::Path) -> Result<()> {
    let m = d.state_dict();
    candle_core::safetensors::save(&m, path)
}

pub fn save_seq_discriminator(d: &SeqDiscriminator, path: &std::path::Path) -> Result<()> {
    let m = d.state_dict();
    candle_core::safetensors::save(&m, path)
}

pub fn load_seq_discriminator(d: &mut SeqDiscriminator, path: &std::path::Path, dev: &Device) -> Result<()> {
    let m = candle_core::safetensors::load(path, dev)?;
    d.load_state(&m)
}

/// What geometry a checkpoint was trained with, read from the checkpoint.
///
/// `g.fc.w` is `[g_filters * h0 * w0, latent]`, and `h0`/`w0` are
/// `n_mels/16` and `time_frames/16`. So the file states its own shape and
/// nothing has to be guessed or configured — which is why this reads the
/// checkpoint rather than adding a GUI field for a hint. A hint is a
/// second place to be wrong, and the answer is already in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CkptShape {
    pub latent: usize,
    pub time_frames: usize,
}

/// Recover it, given the `g_filters` and `n_mels` the caller will build
/// with. Returns `None` when `g.fc.w` is absent or does not decompose —
/// in which case candle's own error is the honest thing to show, because
/// the file is not the shape this function assumes.
///
/// A conditional checkpoint's `g.fc.w` is `[.., latent + ctx_dim]`, and the
/// file carries `g.ctx.fc.w` of `[ctx_dim, 32]` to say so; the ctx width is
/// read from there and subtracted. The `config.json` next to a checkpoint is
/// still the authority on everything else (stems, beat_phase), which is why
/// the GUI prefers it when present.
pub fn checkpoint_shape(
    m: &std::collections::HashMap<String, candle_core::Tensor>,
    g_filters: usize,
    n_mels: usize,
) -> Option<CkptShape> {
    let t = m.get("g.fc.w")?;
    let dims = t.dims();
    let (rows, fc_in) = (*dims.first()?, *dims.get(1)?);
    let ctx_dim = m.get("g.ctx.fc.w").and_then(|w| w.dims().first().copied()).unwrap_or(0);
    let latent = fc_in.checked_sub(ctx_dim)?;
    let h0 = n_mels / 16;
    let per_frame_block = g_filters.checked_mul(h0)?;
    if per_frame_block == 0 || rows % per_frame_block != 0 {
        return None;
    }
    let w0 = rows / per_frame_block;
    Some(CkptShape { latent, time_frames: w0 * 16 })
}

pub fn load_generator(g: &mut Generator, path: &std::path::Path, dev: &Device) -> Result<()> {
    let m = candle_core::safetensors::load(path, dev)?;
    g.load_state(&m)
}


pub fn load_discriminator(d: &mut Discriminator, path: &std::path::Path, dev: &Device) -> Result<()> {
    let m = candle_core::safetensors::load(path, dev)?;
    d.load_state(&m)
}

// ── why training is slow on this hardware ───────────────────────────────────
//
// An educated diagnosis, from reading candle 0.11's CPU backend rather than
// from profiling — the phase timing added alongside this will confirm or refute
// it, and the numbers below are the prediction it should be judged against.
//
// THE DEVICE LINE IS CORRECT AND THE LOW CPU IS THE SYMPTOM. "device: Cpu" is
// accurate; candle has no Vulkan backend and Asahi has no CUDA or Metal. But a
// busy CPU-bound trainer should peg every core, and it does not — which means
// the work is running on ONE core most of the time.
//
// WHERE. candle's two convolutions are implemented completely differently:
//
//   * conv2d  -> src/cpu_backend/conv2d.rs, an im2col + gemm lowering, tiled,
//     `(0..p.b_size).into_par_iter()` — parallel over BATCH, and gemm itself
//     takes Parallelism::Rayon(num_threads). This one uses the machine.
//
//   * conv_transpose2d -> src/cpu_backend/mod.rs:1243, a six-deep naive loop
//     (b x i_h x i_w x c_in x k_h x k_w) whose only parallelism is
//     `(0..p.c_out).into_par_iter()` — parallel over OUTPUT CHANNELS.
//
// The generator is four ConvTranspose2d plus a final one, and the final one
// outputs ONE channel:
//
//   256 -> 128 -> 64 -> 32 -> 1
//
// So `par_iter` over c_out = 1 is a single thread, and it is the layer with the
// LARGEST output tensor in the network — (B, 1, n_mels, time_frames), 16 x 128
// x 256. Its inner work is b x i_h x i_w x c_in x k_h x k_w = 16 x 64 x 128 x
// 32 x 4 x 4, which is ~67 million multiply-accumulates on one core, per step,
// twice (forward, and again inside the backward). On a 24-core M2 Ultra that
// reads as almost no CPU activity while being the bottleneck.
//
// The backward pass compounds it: the gradient of a transposed convolution
// w.r.t. its input is a forward convolution — which is the fast path — but the
// gradient w.r.t. the WEIGHTS goes back through the same naive kernel.
//
// WHAT WOULD FIX IT, in increasing order of work:
//
//   1. A wider final layer. If the last deconv produced, say, 16 channels and a
//      1x1 conv2d reduced them to 1, the deconv parallelises 16 ways and the
//      reduction runs on the gemm path. This CHANGES THE MODEL — more
//      parameters, and a checkpoint from before it will not load — so it is a
//      decision about the architecture rather than an optimisation, and it is
//      not being made here.
//
//   2. Express the deconv as zero-insert upsample + conv2d. Mathematically the
//      same operation, and it routes onto the im2col+gemm path that
//      parallelises over batch. The insert wastes memory bandwidth on the
//      zeros, so it is not obviously a win at every shape — but at c_out=1 it
//      trades a 1-thread kernel for a 16-thread one, which is not close.
//
//   3. The wgpu path in wgpu_train.rs, which is why it exists. Its GEMM is
//      verified on this hardware, and the whole point of lowering every
//      convolution — forward and transposed — to one matmul is that there is no
//      naive kernel left to fall onto.
//
// NOT the explanation, checked and ruled out: candle's thread count comes from
// rayon's, which defaults to every core, so nothing is capping it; and matmul
// does take Parallelism::Rayon. The fc layer and the discriminator (whose convs
// are all conv2d) are on the fast paths already.

#[cfg(test)]
mod ckpt_shape_tests {
    use super::*;
    use candle_core::{DType, Device, Tensor};

    fn fc(rows: usize, latent: usize) -> std::collections::HashMap<String, Tensor> {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "g.fc.w".to_string(),
            Tensor::zeros((rows, latent), DType::F32, &Device::Cpu).unwrap(),
        );
        m
    }

    #[test]
    fn the_real_640_frame_checkpoint_reports_640_frames() {
        // The actual numbers from checkpoints-epoch337-640frames:
        // g.fc.w is [81920, 128], trained at g_filters 256, n_mels 128.
        // 81920 = 256 * (128/16) * (640/16) = 256 * 8 * 40.
        let got = checkpoint_shape(&fc(81920, 128), 256, 128);
        assert_eq!(got, Some(CkptShape { latent: 128, time_frames: 640 }));
    }

    #[test]
    fn the_config_default_reports_its_own_64_frames() {
        // The other half of the mismatch the GUI kept printing: 8192 is
        // 64 frames, which is what the default config builds.
        let got = checkpoint_shape(&fc(8192, 128), 256, 128);
        assert_eq!(got, Some(CkptShape { latent: 128, time_frames: 64 }));
    }

    #[test]
    fn a_file_that_does_not_decompose_returns_none() {
        // Better to hand back None and let candle report the real shape
        // error than to invent a frame count from a row count that is not
        // a whole number of frames — a confidently wrong rebuild is
        // harder to debug than an honest mismatch.
        assert_eq!(checkpoint_shape(&fc(81921, 128), 256, 128), None);
        assert!(checkpoint_shape(&std::collections::HashMap::new(), 256, 128).is_none());
    }

    #[test]
    fn zero_filters_or_mels_is_not_a_divide_by_zero() {
        assert_eq!(checkpoint_shape(&fc(81920, 128), 0, 128), None);
        assert_eq!(checkpoint_shape(&fc(81920, 128), 256, 0), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unit_below_threshold_still_receives_gradient() {
        // THE WHOLE FIX. The hard step `x * (|x| > t)` has EXACTLY ZERO
        // gradient below threshold, and the reference pipeline's v5.3 comment
        // records what that does: "even at random init 80-99.9% of units in a
        // block fall below threshold ... units below threshold can never
        // receive a gradient to recover, so the whole discriminator can (and
        // did) collapse into a permanently frozen, input-independent fixed
        // point".
        //
        // That is the signature the 269-epoch run produced — d_loss pinned at
        // 2ln2, flat for forty points. So the property to assert is not that
        // the gate matches the old one; it is that it DIFFERS from it in
        // exactly the place the old one was fatal.
        use candle_core::{Device, Tensor, Var};
        let dev = Device::Cpu;
        let thr = 0.05f32;
        // Values well below threshold: the dead zone.
        let below: Vec<f32> = vec![0.001, 0.005, 0.01, 0.02, -0.001, -0.02];
        let v = Var::from_tensor(&Tensor::from_vec(below.clone(), (1, 1, 2, 3), &dev).unwrap())
            .unwrap();
        let y = v.as_tensor().apply_op1(SparseGate { thr, sharpness: 40.0 }).unwrap();
        let g = y.sum_all().unwrap().backward().unwrap();
        let d = g.get(&v).expect("no gradient at all")
            .flatten_all().unwrap().to_vec1::<f32>().unwrap();

        for (x, dx) in below.iter().zip(&d) {
            assert!(dx.abs() > 1e-6,
                    "unit at {x} has gradient {dx} — it can never recover, which is the \
v5.2 collapse");
            assert!(dx.is_finite());
        }
    }

    #[test]
    fn the_gate_still_suppresses_below_threshold_and_passes_above() {
        // The forward behaviour must stay near the hard gate's, or this is a
        // different model rather than a fixed one. sharpness 40 makes the
        // sigmoid effectively binary a little way either side of t.
        use candle_core::{Device, Tensor};
        let dev = Device::Cpu;
        let thr = 0.05f32;
        let xs = vec![0.001f32, 0.5, -0.5, 0.0];
        let x = Tensor::from_vec(xs.clone(), (1, 1, 1, 4), &dev).unwrap();
        let y = x.apply_op1(SparseGate { thr, sharpness: 40.0 }).unwrap()
            .flatten_all().unwrap().to_vec1::<f32>().unwrap();
        assert!(y[0].abs() < 0.001, "0.001 should be suppressed, got {}", y[0]);
        assert!((y[1] - 0.5).abs() < 0.01, "0.5 should pass, got {}", y[1]);
        assert!((y[2] + 0.5).abs() < 0.01, "-0.5 should pass, got {}", y[2]);
        assert_eq!(y[3], 0.0, "zero stays zero");
    }

    #[test]
    fn the_block_activation_is_leaky() {
        // A plain relu is the same dead-unit trap one line down from the gate:
        // a channel driven negative gets zero gradient forever. The reference
        // uses leaky_relu(0.2) and so must this.
        use candle_core::{Device, Tensor};
        let dev = Device::Cpu;
        let blk = SparseBlock::new(1, 1, 1, 0.0, &dev).unwrap();
        // Drive the output negative by construction is awkward; assert on the
        // source instead, which is what actually has to not regress.
        let src = include_str!("model.rs");
        assert!(src.contains("x.maximum(&(&x * 0.2)?)"), "the block is not leaky");
        assert!(!src.contains("        x.relu()\n    }"), "a plain relu came back");
        let _ = blk;
        let _ = Tensor::zeros((1, 1, 4, 4), candle_core::DType::F32, &dev).unwrap();
    }

    #[test]
    fn the_folded_batchnorm_matches_the_unfolded_formula() {
        // Compared against the ARITHMETIC IT REPLACED, not against a property.
        // My first attempt asserted "variance is 1" and got 1.012 — which sent
        // me looking for a bug in the refactor when the assertion was simply
        // the wrong claim: y is normalised by the BIASED batch variance and
        // then scaled by gamma, so E[y^2] is only 1 when gamma is exactly 1 and
        // eps is exactly 0. Testing the replacement against the original is the
        // check that actually says "I did not change the maths".
        use candle_core::{Device, Tensor};
        let dev = Device::Cpu;
        let bn = BatchNorm2d::new(3, &dev).unwrap();
        let xs: Vec<f32> = (0..2 * 3 * 4 * 5)
            .map(|i| (i as f32 * 0.41).sin() * 3.0 + 1.0).collect();
        let x = Tensor::from_vec(xs, (2, 3, 4, 5), &dev).unwrap();

        let got = bn.forward(&x, true).unwrap();

        // The original, written out: mean, centered, biased var, then
        // sub -> div -> mul -> add as four separate broadcasts.
        let mean = x.mean((0usize, 2, 3)).unwrap();
        let mean_b = mean.reshape((1, 3, 1, 1)).unwrap();
        let centered = x.broadcast_sub(&mean_b).unwrap();
        let var = centered.sqr().unwrap().mean((0usize, 2, 3)).unwrap()
            .reshape((1, 3, 1, 1)).unwrap();
        let g = bn.gamma.as_tensor().reshape((1, 3, 1, 1)).unwrap();
        let b = bn.beta.as_tensor().reshape((1, 3, 1, 1)).unwrap();
        let want = x.broadcast_sub(&mean_b).unwrap()
            .broadcast_div(&var.affine(1.0, bn.eps).unwrap().sqrt().unwrap()).unwrap()
            .broadcast_mul(&g).unwrap()
            .broadcast_add(&b).unwrap();

        let a = got.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        let e = want.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        let worst = a.iter().zip(&e).map(|(x, y)| (x - y).abs()).fold(0f32, f32::max);
        assert!(worst < 1e-5, "the fold changed the arithmetic: worst {worst}");
        // And it must not be trivially equal because everything is zero.
        assert!(a.iter().any(|v| v.abs() > 0.1), "the test data was degenerate");
    }

    #[test]
    fn eval_mode_uses_the_running_stats_and_not_the_batch() {
        // The eval arm has no `centered` to reuse, so it takes the fallback
        // path — which must subtract the RUNNING mean rather than recomputing
        // the batch's. Getting that wrong makes inference depend on batch
        // composition and nothing fails loudly.
        //
        // My first version asserted eval was the identity at init. It is not:
        // gamma is randn(0, 0.02) + 1.0, the DCGAN convention, so a channel
        // scaled by 0.9785 is a perfectly ordinary draw. The code was right and
        // the assertion was wrong — checked before changing anything.
        use candle_core::{Device, Tensor};
        let dev = Device::Cpu;
        let bn = BatchNorm2d::new(2, &dev).unwrap();
        let xs: Vec<f32> = (0..2 * 2 * 3 * 3).map(|i| i as f32 * 0.7 - 3.0).collect();
        let x = Tensor::from_vec(xs, (2, 2, 3, 3), &dev).unwrap();

        let got = bn.forward(&x, false).unwrap();
        let mean = bn.run_mean.as_tensor().reshape((1, 2, 1, 1)).unwrap();
        let var = bn.run_var.as_tensor().reshape((1, 2, 1, 1)).unwrap();
        let g = bn.gamma.as_tensor().reshape((1, 2, 1, 1)).unwrap();
        let b = bn.beta.as_tensor().reshape((1, 2, 1, 1)).unwrap();
        let want = x.broadcast_sub(&mean).unwrap()
            .broadcast_div(&var.affine(1.0, bn.eps).unwrap().sqrt().unwrap()).unwrap()
            .broadcast_mul(&g).unwrap()
            .broadcast_add(&b).unwrap();

        let a = got.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        let e = want.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        let worst = a.iter().zip(&e).map(|(p, q)| (p - q).abs()).fold(0f32, f32::max);
        assert!(worst < 1e-5, "eval does not follow the running stats: worst {worst}");

        // And the batch's own statistics must NOT have leaked in: this input
        // has a large mean, so a train-mode result would be centred near zero.
        assert!(a.iter().map(|v| v.abs()).fold(0f32, f32::max) > 1.0,
                "eval output looks batch-normalised, so the running stats were ignored");
    }

    // the_fused_gate_matches_the_four_op_chain_and_its_gradient REMOVED.
    //
    // It asserted the fused gate matched `x * (|x| > t)` exactly, forward and
    // backward — which was right when the gate was a hard step and is now the
    // wrong thing to want. The reference pipeline replaced that step precisely
    // because its gradient is zero below threshold, and a test demanding the
    // old gradient would have blocked the fix.
    //
    // a_unit_below_threshold_still_receives_gradient replaces it, and asserts
    // the property the old one forbade.

    #[test]
    fn the_dense_discriminator_block_has_the_same_shape_contract() {
        // The flag swaps depthwise+pointwise for dense+1x1, which changes the
        // parameter shapes. What must NOT change is what a block does to a
        // tensor: same output channels, same stride behaviour. If it did, the
        // flag would not be an A/B of speed, it would be a different model
        // that also happens to run.
        use candle_core::{Device, Tensor};
        let dev = Device::Cpu;
        let x = Tensor::zeros((2, 8, 16, 32), candle_core::DType::F32, &dev).unwrap();

        std::env::set_var("BEATSPACE_DENSE_D", "1");
        let dense = SparseBlock::new(8, 16, 2, 0.0, &dev).unwrap();
        std::env::remove_var("BEATSPACE_DENSE_D");
        let depth = SparseBlock::new(8, 16, 2, 0.0, &dev).unwrap();

        let a = dense.forward(&x, false).unwrap();
        let b = depth.forward(&x, false).unwrap();
        assert_eq!(a.dims(), b.dims(), "the two paths disagree on shape");
        assert_eq!(a.dims(), &[2, 16, 8, 16], "stride 2 on 16x32 should give 8x16");
    }

    #[test]
    fn a_grouped_conv_really_is_split_per_group() {
        // The claim the whole finding rests on, checked against candle rather
        // than read from its source: a depthwise conv over N channels costs
        // roughly N times a single-channel one, because it IS N of them run in
        // sequence. Timed loosely — this asserts an order of magnitude, not a
        // number, so it cannot fail on a busy machine.
        use candle_core::{DType, Device, Tensor};
        let dev = Device::Cpu;
        let c = 64usize;
        let x = Tensor::ones((1, c, 32, 32), DType::F32, &dev).unwrap();
        let kd = Tensor::ones((c, 1, 3, 3), DType::F32, &dev).unwrap();   // depthwise
        let k1 = Tensor::ones((c, c, 1, 1), DType::F32, &dev).unwrap();   // dense 1x1

        let t0 = std::time::Instant::now();
        let _ = x.conv2d(&kd, 1, 1, 1, c).unwrap();
        let depthwise = t0.elapsed().as_secs_f64();
        let t1 = std::time::Instant::now();
        let _ = x.conv2d(&k1, 0, 1, 1, 1).unwrap();
        let dense = t1.elapsed().as_secs_f64();

        // The depthwise does ~9x fewer multiply-accumulates than the dense 1x1
        // over 64 channels, and should still not be dramatically FASTER — if it
        // were, the split would not be costing what this finding says. Loose
        // bound so a noisy machine cannot fail it spuriously.
        assert!(depthwise > dense / 20.0,
                "depthwise {depthwise:.6}s vs dense 1x1 {dense:.6}s — the group split may have \
been fixed upstream, which would retire the BEATSPACE_DENSE_D flag");
    }

    /// The two deconv paths must agree to floating-point noise.
    ///
    /// "Mathematically identical" is a claim, and an unchecked one here would
    /// be the worst kind: the upsample path would train, produce plausible
    /// output, and be subtly wrong. A flipped kernel axis or an off-by-one in
    /// the padding does not crash — it shifts the image by a pixel.
    #[test]
    fn upsample_conv2d_matches_conv_transpose2d() {
        use candle_core::{Device, Tensor};
        let dev = Device::Cpu;
        for (c_in, c_out, stride, pad, h, w) in
            [(3usize, 2usize, 2usize, 1usize, 5usize, 6usize),
             (4, 1, 2, 1, 4, 4),      // c_out = 1, the case that motivates this
             (2, 3, 1, 0, 5, 5),      // stride 1, the no-insert path
             (2, 2, 2, 0, 3, 4)] {
            let kh = 4;
            let xs: Vec<f32> = (0..c_in * h * w).map(|i| (i as f32 * 0.37).sin()).collect();
            let x = Tensor::from_vec(xs, (1, c_in, h, w), &dev).unwrap();
            let ws: Vec<f32> = (0..c_in * c_out * kh * kh)
                .map(|i| (i as f32 * 0.11).cos()).collect();
            let wt = Tensor::from_vec(ws, (c_in, c_out, kh, kh), &dev).unwrap();

            let layer = ConvT2d {
                w: Var::from_tensor(&wt).unwrap(),
                stride, padding: pad, out_padding: 0,
            };
            let want = x.conv_transpose2d(layer.w.as_tensor(), pad, 0, stride, 1).unwrap();
            let got = layer.forward_via_conv2d(&x).unwrap();
            assert_eq!(want.dims(), got.dims(),
                       "shape differs at c_in{c_in} c_out{c_out} s{stride} p{pad}");
            let a = want.flatten_all().unwrap().to_vec1::<f32>().unwrap();
            let b = got.flatten_all().unwrap().to_vec1::<f32>().unwrap();
            let worst = a.iter().zip(&b).map(|(x, y)| (x - y).abs())
                .fold(0f32, f32::max);
            assert!(worst < 1e-4,
                    "c_in{c_in} c_out{c_out} s{stride} p{pad}: worst {worst}");
        }
    }
}

#[cfg(test)]
mod conditional_tests {
    use super::*;
    use candle_core::Device;

    /// A tiny dynamic config: the smoke test's sizes with every conditional
    /// part on.
    fn tiny_dynamic() -> Config {
        Config {
            n_mels: 32, time_frames: 32, latent: 16, g_filters: 64, d_filters: 32,
            batch_size: 4, n_fft: 256, hop: 64, sr: 8_000, fmax: 4_000.0,
            ..Config::dynamic()
        }
    }

    fn cond(cfg: &Config, b: usize, seed: f32) -> Cond {
        let dev = Device::Cpu;
        let ctx = Tensor::randn(seed, 0.5f32, (b, cfg.stems, cfg.n_mels, cfg.ctx_frames), &dev).unwrap();
        let phase: Vec<f32> = (0..b * cfg.time_frames).map(|i| ((i as f32 * 0.07 + seed) % 1.0).abs()).collect();
        Cond {
            ctx: Some(ctx),
            phase: Some(Tensor::from_vec(phase, (b, cfg.time_frames), &dev).unwrap()),
        }
    }

    #[test]
    fn the_default_config_builds_the_legacy_generator_with_the_legacy_keys() {
        let dev = Device::Cpu;
        let cfg = Config { n_mels: 32, time_frames: 32, latent: 16, g_filters: 64, ..Config::default() };
        let g = Generator::new(&cfg, &dev).unwrap();
        assert!(!g.conditional());
        let keys: Vec<String> = g.state_dict().keys().cloned().collect();
        assert!(keys.iter().all(|k| !k.starts_with("g.ctx")), "{keys:?}");
        let sd = g.state_dict();
        assert_eq!(sd["g.fc.w"].dims(), &[64 * 2 * 2, 16]);
        assert_eq!(sd["g.deconv3.w"].dims(), &[8, 1, 4, 4], "last deconv is 8 -> 1, no phase planes");
        // And the legacy entry points run.
        let z = Tensor::zeros((2, 16), candle_core::DType::F32, &dev).unwrap();
        assert_eq!(g.forward_eval(&z).unwrap().dims(), &[2, 1, 32, 32]);
    }

    #[test]
    fn the_dynamic_generator_has_the_shapes_the_design_says() {
        let dev = Device::Cpu;
        let cfg = tiny_dynamic();
        let g = Generator::new(&cfg, &dev).unwrap();
        assert!(g.conditional());
        let sd = g.state_dict();
        assert_eq!(sd["g.fc.w"].dims(), &[64 * 2 * 2, 16 + cfg.ctx_dim], "fc reads z and the context");
        assert_eq!(sd["g.deconv3.w"].dims(), &[8 + 2, 2, 4, 4], "last deconv reads two phase planes, emits two stems");
        assert_eq!(sd["g.ctx.c0.w"].dims(), &[16, 2, 3, 3]);
        assert_eq!(sd["g.ctx.fc.w"].dims(), &[cfg.ctx_dim, 32]);
        let z = Tensor::zeros((3, 16), candle_core::DType::F32, &dev).unwrap();
        let out = g.forward_eval_cond(&z, &cond(&cfg, 3, 0.1)).unwrap();
        assert_eq!(out.dims(), &[3, 2, 32, 32]);
        // The unconditional entry point refuses rather than zero-filling.
        let err = g.forward_eval(&z).unwrap_err().to_string();
        assert!(err.contains("Cond::ctx"), "{err}");
    }

    #[test]
    fn the_context_and_the_phase_each_change_the_output() {
        // Structural, not learned: at random init a different input on
        // either path must reach the output, or training could never make
        // the model use it.
        let dev = Device::Cpu;
        let cfg = tiny_dynamic();
        let g = Generator::new(&cfg, &dev).unwrap();
        let z = Tensor::randn(0f32, 1f32, (2, 16), &dev).unwrap();
        let a = cond(&cfg, 2, 0.1);
        let base = g.forward_eval_cond(&z, &a).unwrap();
        let mut other_ctx = cond(&cfg, 2, 0.9);
        other_ctx.phase = a.phase.clone();
        let mut other_phase = cond(&cfg, 2, 0.1);
        other_phase.phase = Some((a.phase.as_ref().unwrap() + 0.5).unwrap());
        let diff = |t: &Tensor| (t - &base).unwrap().abs().unwrap().mean_all().unwrap().to_scalar::<f32>().unwrap();
        // Strictly positive rather than a threshold: at N(0, 0.02) init the
        // context reaches the output through five small layers and lands
        // around 1e-6, but a DISCONNECTED path gives exactly zero, and that
        // is the failure this catches.
        assert!(diff(&g.forward_eval_cond(&z, &other_ctx).unwrap()) > 0.0, "context is ignored");
        assert!(diff(&g.forward_eval_cond(&z, &other_phase).unwrap()) > 0.0, "phase is ignored");
        // And gradient reaches the context encoder: every one of its
        // parameters must appear in the trainable set.
        let n_ctx = g.trainable_vars().len() - Generator::new(&Config { ctx_frames: 0, ..cfg.clone() }, &dev).unwrap().trainable_vars().len();
        assert_eq!(n_ctx, 4, "c0.w, c1.w, fc.w, fc.b");
    }

    #[test]
    fn phase_planes_average_on_the_circle() {
        // Two adjacent columns at 0.99 and 0.01 straddle the wrap; their
        // sin mean is ~0 and cos mean is ~1, not the sin/cos of 0.5.
        let dev = Device::Cpu;
        let phase = Tensor::from_vec(vec![0.99f32, 0.01, 0.25, 0.25], (1, 4), &dev).unwrap();
        let planes = Generator::phase_planes(&phase, 3, 2).unwrap();
        assert_eq!(planes.dims(), &[1, 2, 3, 2]);
        let v: Vec<f32> = planes.flatten_all().unwrap().to_vec1().unwrap();
        // layout: [sin plane (3x2)] [cos plane (3x2)]
        assert!(v[0].abs() < 0.1, "sin across the wrap: {}", v[0]);
        assert!(v[6] > 0.99, "cos across the wrap: {}", v[6]);
        assert!((v[1] - 1.0).abs() < 1e-5 && v[7].abs() < 1e-5, "sin/cos of 0.25");
        // Every row is the same column vector: the planes are broadcast over
        // frequency.
        assert_eq!(v[0], v[2]);
        assert_eq!(v[1], v[5]);
    }

    #[test]
    fn the_sequence_discriminator_scores_one_logit_per_time_column() {
        let dev = Device::Cpu;
        let cfg = tiny_dynamic();
        let sd = SeqDiscriminator::new(&cfg, &dev).unwrap();
        // A pair of windows joined at the overlap: 2T - overlap wide.
        let w = 2 * cfg.time_frames - cfg.overlap_frames;
        let x = Tensor::randn(0f32, 0.5f32, (3, cfg.stems, cfg.n_mels, w), &dev).unwrap();
        let out = sd.forward(&x, true).unwrap();
        // Four stride-2 blocks with padding 1: ceil(w/2) four times.
        let mut cols = w;
        for _ in 0..4 { cols = (cols + 1) / 2; }
        assert_eq!(out.dims(), &[3, cols]);
        // The window discriminator reads the same stems and pools to one.
        let d = Discriminator::new(&cfg, &dev).unwrap();
        let win = Tensor::randn(0f32, 0.5f32, (3, cfg.stems, cfg.n_mels, cfg.time_frames), &dev).unwrap();
        assert_eq!(d.forward(&win, true).unwrap().dims(), &[3]);
        // Round trip through its own state dict.
        let mut sd2 = SeqDiscriminator::new(&cfg, &dev).unwrap();
        sd2.load_state(&sd.state_dict()).unwrap();
        let again = sd2.forward(&x, false).unwrap();
        let _ = again;
    }

    #[test]
    fn a_conditional_checkpoint_reports_its_latent_without_the_context_width() {
        let dev = Device::Cpu;
        let cfg = tiny_dynamic();
        let g = Generator::new(&cfg, &dev).unwrap();
        let sh = checkpoint_shape(&g.state_dict(), cfg.g_filters, cfg.n_mels).unwrap();
        assert_eq!(sh, CkptShape { latent: 16, time_frames: 32 });
    }

    #[test]
    fn a_legacy_checkpoint_does_not_load_into_a_conditional_generator_silently() {
        let dev = Device::Cpu;
        let cfg = tiny_dynamic();
        let legacy = Generator::new(&Config { ctx_frames: 0, beat_phase: false, stems: 1, ..cfg.clone() }, &dev).unwrap();
        let mut g = Generator::new(&cfg, &dev).unwrap();
        let err = g.load_state(&legacy.state_dict()).unwrap_err().to_string();
        assert!(err.contains("shape") || err.contains("g.ctx"), "{err}");
    }
}
