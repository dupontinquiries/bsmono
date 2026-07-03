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
        x.conv_transpose2d(
            self.w.as_tensor(),
            self.padding,
            self.out_padding,
            self.stride,
            1,
        )
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

impl SparseBlock {
    fn new(cin: usize, cout: usize, stride: usize, threshold: f32, dev: &Device) -> Result<Self> {
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
        // sparsity mask: zero out activations with |x| <= threshold
        let absx = x.abs()?;
        let mask = absx.gt(self.threshold)?.to_dtype(DType::F32)?;
        let x = x.mul(&mask)?;
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
