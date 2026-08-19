//! The GAN itself, ported faithfully from the v6 (`gan-zai` r2) PyTorch model.
//!
//! Layers are built *by hand* out of `Var`s rather than with candle_nn's
//! helper constructors. The reason is checkpointing: we want full control over
//! exactly which tensors are saved/loaded — including BatchNorm's running
//! mean/var, which the GUI needs at eval time and which candle_nn's BatchNorm
//! keeps in a place that `VarMap::save` would silently drop.
//!
//! Shapes (B = batch):
//!   Generator:     z (B,128)             -> mel (B,1,n_mels,time_frames) in [-1,1]
//!   Discriminator: mel (B,1,n_mels,time_frames)   -> logit (B,)
//!
//! Generator stack (g_filters f = 256):
//!   fc: latent -> (n_mels/16)*(time_frames/16)*f, reshape (B,f,h0,w0)
//!   4x ConvTranspose2d k4 s2 p1 (f->f/2), BatchNorm, ReLU      256->128->64->32
//!   final ConvTranspose2d -> 1 channel, Tanh                   -> (B,1,n_mels,time_frames)
//!
//! Discriminator stack (d_filters = 64), 4x SparseBlock stride 2:
//!   1->64->128->256->512, AdaptiveAvgPool -> Linear(512,1) -> (B,)

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
        let (mean, var) = if train {
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
            (mean, var)
        } else {
            (
                self.run_mean.as_tensor().clone(),
                self.run_var.as_tensor().clone(),
            )
        };
        let mean = mean.reshape((1, c, 1, 1))?;
        let var = var.reshape((1, c, 1, 1))?;
        let g = self.gamma.as_tensor().reshape((1, c, 1, 1))?;
        let b = self.beta.as_tensor().reshape((1, c, 1, 1))?;
        let xn = x
            .broadcast_sub(&mean)?
            .broadcast_div(&var.affine(1.0, self.eps)?.sqrt()?)?;
        xn.broadcast_mul(&g)?.broadcast_add(&b)
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

pub struct Generator {
    fc: Linear,
    deconvs: Vec<ConvT2d>,
    bns: Vec<BatchNorm2d>, // one fewer than deconvs (last deconv has no BN)
    f0: usize,             // channels after fc reshape (= g_filters)
    h0: usize,             // 8
    w0: usize,             // 16
}

impl Generator {
    pub fn new(cfg: &Config, dev: &Device) -> Result<Self> {
        let f = cfg.g_filters; // 256
        let h0 = cfg.n_mels / 16; // 128/16 = 8
        let w0 = cfg.time_frames / 16; // 256/16 = 16
        let fc = Linear::new(cfg.latent, f * h0 * w0, dev)?;

        // Deconv channel schedule: 256 -> 128 -> 64 -> 32 (BN+ReLU each),
        // then a final 32 -> 1 layer with Tanh.
        let stage_out = [f / 2, f / 4, f / 8, 1]; // 128,64,32,1
        let mut deconvs = Vec::new();
        let mut bns = Vec::new();
        let mut cin = f;
        for (i, &cout) in stage_out.iter().enumerate() {
            deconvs.push(ConvT2d::new(cin, cout, 4, 2, 1, 0, dev)?);
            if i < stage_out.len() - 1 {
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
        })
    }

    fn forward(&self, z: &Tensor, train: bool) -> Result<Tensor> {
        let b = z.dim(0)?;
        let mut x = self.fc.forward(z)?; // (B, f*h0*w0)
        x = x.reshape((b, self.f0, self.h0, self.w0))?;
        let n = self.deconvs.len();
        for i in 0..n {
            x = self.deconvs[i].forward(&x)?;
            if i < n - 1 {
                x = self.bns[i].forward(&x, train)?;
                x = x.relu()?;
            } else {
                x = x.tanh()?; // final layer -> [-1,1]
            }
        }
        Ok(x) // (B,1,n_mels,time_frames)
    }

    pub fn forward_train(&self, z: &Tensor) -> Result<Tensor> {
        self.forward(z, true)
    }
    pub fn forward_eval(&self, z: &Tensor) -> Result<Tensor> {
        self.forward(z, false)
    }

    /// Convenience for the GUI: one latent row -> mel as a flat (n_mels*T) Vec.
    pub fn generate(&self, z: &[f32], dev: &Device) -> Result<(Vec<f32>, usize, usize)> {
        let latent = z.len();
        let zt = Tensor::from_vec(z.to_vec(), (1, latent), dev)?;
        let mel = self.forward_eval(&zt)?; // (1,1,n_mels,time_frames)
        let mel = mel.squeeze(0)?.squeeze(0)?; // (n_mels,time_frames)
        let (n_mels, t) = mel.dims2()?;
        let flat = mel.flatten_all()?.to_vec1::<f32>()?;
        Ok((flat, n_mels, t))
    }

    /// Flatten all weights (folding eval-time BatchNorm into per-channel
    /// scale/shift) for an external GPU forward pass. CPU-side; cheap.
    #[cfg(feature = "gpu")]
    pub fn export_params(&self) -> Result<GenParams> {
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
/// `|x| > t ? x : 0`, in one traversal.
///
/// A CustomOp1 rather than a chain of tensor ops because the chain is four
/// passes and four allocations over a tensor that can be 8.4M elements, and
/// candle's elementwise kernels are single-threaded.
///
/// IT IMPLEMENTS `bwd`, and the first version did not.
///
/// I reached for `apply_op1_no_bwd` and wrote a comment rationalising it — that
/// the gate is "a straight-through path where it passes and a stop where it
/// does not". That is true of the VALUE and false of the GRAPH: no_bwd cuts the
/// backward edge, so no gradient would have reached the depthwise convolution
/// beneath it and the discriminator would have quietly stopped learning half of
/// itself. The loss would still move, because the pointwise and later layers
/// still train.
///
/// The original chain was differentiable: `mul` by a constant mask passes
/// `grad * mask` through. This reproduces exactly that, in one pass.
struct SparseGate {
    thr: f32,
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
                candle_core::CpuStorage::F32(
                    src.iter().map(|&x| if x.abs() > thr { x } else { 0.0 }).collect())
            }
            _ => candle_core::bail!("sparse-gate is f32 only"),
        };
        Ok((out, l.shape().clone()))
    }

    /// `grad * (|x| > t)`, matching what `mul` by the constant mask did.
    ///
    /// Reads the ARGUMENT rather than the result: at exactly zero the two are
    /// indistinguishable in the output — a gated-off element and a genuine zero
    /// both read 0 — and deriving the mask from the result would pass gradient
    /// through elements the forward had blocked.
    fn bwd(&self, arg: &Tensor, _res: &Tensor, grad: &Tensor) -> Result<Option<Tensor>> {
        let mask = arg.abs()?.gt(self.thr)?.to_dtype(DType::F32)?;
        Ok(Some(grad.mul(&mask)?))
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
            });
        }
        Ok(Self {
            // depthwise: groups == cin, k3
            depthwise: Conv2d::new(cin, cin, 3, stride, 1, cin, dev)?,
            // pointwise: 1x1
            pointwise: Conv2d::new(cin, cout, 1, 1, 0, 1, dev)?,
            bn: BatchNorm2d::new(cout, dev)?,
            threshold,
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
        let x = x.apply_op1(SparseGate { thr })?;
        let x = self.pointwise.forward(&x)?;
        let x = self.bn.forward(&x, train)?;
        x.relu()
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

impl Discriminator {
    pub fn new(cfg: &Config, dev: &Device) -> Result<Self> {
        let d = cfg.d_filters; // 64
        // 1 -> 64 -> 128 -> 256 -> 512
        let chans = [(1usize, d), (d, d * 2), (d * 2, d * 4), (d * 4, d * 8)];
        let mut blocks = Vec::new();
        for &(cin, cout) in chans.iter() {
            blocks.push(SparseBlock::new(cin, cout, 2, cfg.sparse_threshold, dev)?);
        }
        let head = Linear::new(d * 8, 1, dev)?;
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
mod tests {
    use super::*;

    #[test]
    fn the_fused_gate_matches_the_four_op_chain_and_its_gradient() {
        // The forward is the easy half. The GRADIENT is the half I nearly lost:
        // the first version used apply_op1_no_bwd, which cuts the backward edge,
        // so nothing would have reached the depthwise conv and the
        // discriminator would have quietly stopped training half of itself
        // while the loss kept moving.
        use candle_core::{DType, Device, Tensor, Var};
        let dev = Device::Cpu;
        let thr = 0.25f32;
        let vals: Vec<f32> = (0..64).map(|i| (i as f32 * 0.31).sin()).collect();

        // Fused.
        let a = Var::from_tensor(&Tensor::from_vec(vals.clone(), (1, 1, 8, 8), &dev).unwrap()).unwrap();
        let ya = a.as_tensor().apply_op1(SparseGate { thr }).unwrap();
        let ga = ya.sum_all().unwrap().backward().unwrap();

        // The chain it replaces.
        let b = Var::from_tensor(&Tensor::from_vec(vals.clone(), (1, 1, 8, 8), &dev).unwrap()).unwrap();
        let mask = b.as_tensor().abs().unwrap().gt(thr).unwrap().to_dtype(DType::F32).unwrap();
        let yb = b.as_tensor().mul(&mask).unwrap();
        let gb = yb.sum_all().unwrap().backward().unwrap();

        let fa = ya.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        let fb = yb.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        assert_eq!(fa, fb, "the forward differs");

        let da = ga.get(&a).expect("the fused gate produced NO gradient")
            .flatten_all().unwrap().to_vec1::<f32>().unwrap();
        let db = gb.get(&b).expect("the chain produced no gradient")
            .flatten_all().unwrap().to_vec1::<f32>().unwrap();
        assert_eq!(da, db, "the gradient differs");
        // And it is a real mask, not all-ones — otherwise the test would pass
        // on a gate that does nothing.
        assert!(da.iter().any(|&v| v == 0.0) && da.iter().any(|&v| v != 0.0),
                "the threshold gated nothing, so this proves nothing");
    }

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
