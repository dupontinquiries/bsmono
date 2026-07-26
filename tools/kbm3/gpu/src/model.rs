//! The kBM3 GAN in Burn, architecturally identical to the PyTorch v5 model so
//! that shapes and behaviour match: Generator projects a latent to (B,f,8,16)
//! and upsamples with four stride-2 ConvTranspose2d blocks to (B,1,128,256);
//! Discriminator is a strided-conv DCGAN critic ending in AdaptiveAvgPool -> Linear.
//!
//! Being generic over `B: Backend`, the SAME code runs on the Autodiff<Wgpu>
//! (Vulkan) backend for GPU training and on plain Wgpu for inference.


use burn::module::Param;
use burn::nn::conv::{Conv2d, Conv2dConfig, ConvTranspose2d, ConvTranspose2dConfig};
use burn::nn::pool::{AdaptiveAvgPool2d, AdaptiveAvgPool2dConfig};
use burn::nn::{BatchNorm, BatchNormConfig, Linear, LinearConfig, PaddingConfig2d};
use burn::prelude::*;
use burn::tensor::activation::{leaky_relu, relu, sigmoid, tanh};

#[derive(Config, Debug)]
pub struct GanConfig {
    #[config(default = 256)]
    pub latent: usize,
    #[config(default = 256)]
    pub g_filters: usize,
    #[config(default = 64)]
    pub d_filters: usize,
    #[config(default = 256)]
    pub n_mels: usize,
    #[config(default = 560)]
    pub time_frames: usize,
}

impl GanConfig {
    pub fn min_h(&self) -> usize {
        self.n_mels / 16
    }
    pub fn min_w(&self) -> usize {
        self.time_frames / 16
    }

    pub fn init_generator<B: Backend>(&self, device: &B::Device) -> Generator<B> {
        let f = self.g_filters;
        // The FC projects the latent to the seed feature map. Projecting to the
        // full `f` channels makes this weight matrix latent*(f*min_h*min_w) --
        // at the 256x560 config that's 140 MB in a SINGLE tensor, which
        // exceeds the GPU's max buffer-binding size (128 MB on lavapipe; even
        // where it fits, a 140 MB dense weight is a real bottleneck). Project
        // to f/2 seed channels instead (70 MB), then let ct1 widen back to the
        // pyramid. Same output shape, ~same capacity, half the FC.
        let fc_ch = (f / 2).max(1);
        Generator {
            fc: LinearConfig::new(self.latent, fc_ch * self.min_h() * self.min_w()).init(device),
            ct1: convt(fc_ch, f / 2, device),
            bn1: BatchNormConfig::new(f / 2).init(device),
            ct2: convt(f / 2, f / 4, device),
            bn2: BatchNormConfig::new(f / 4).init(device),
            ct3: convt(f / 4, f / 8, device),
            bn3: BatchNormConfig::new(f / 8).init(device),
            ct4: convt(f / 8, 1, device),
            f: fc_ch,
            min_h: self.min_h(),
            min_w: self.min_w(),
        }
    }

    pub fn init_discriminator<B: Backend>(&self, device: &B::Device) -> Discriminator<B> {
        let f = self.d_filters;
        Discriminator {
            b1: SparseBlock::new(1, f, 2, device),
            b2: SparseBlock::new(f, f * 2, 2, device),
            b3: SparseBlock::new(f * 2, f * 4, 2, device),
            b4: SparseBlock::new(f * 4, f * 8, 2, device),
            pool: AdaptiveAvgPool2dConfig::new([1, 1]).init(),
            fc: LinearConfig::new(f * 8, 1).init(device),
        }
    }
}

fn convt<B: Backend>(cin: usize, cout: usize, device: &B::Device) -> ConvTranspose2d<B> {
    ConvTranspose2dConfig::new([cin, cout], [4, 4])
        .with_stride([2, 2])
        .with_padding([1, 1])
        .with_bias(false)
        .init(device)
}

#[derive(Module, Debug)]
pub struct Generator<B: Backend> {
    fc: Linear<B>,
    ct1: ConvTranspose2d<B>,
    bn1: BatchNorm<B>,
    ct2: ConvTranspose2d<B>,
    bn2: BatchNorm<B>,
    ct3: ConvTranspose2d<B>,
    bn3: BatchNorm<B>,
    ct4: ConvTranspose2d<B>,
    #[module(skip)]
    f: usize,
    #[module(skip)]
    min_h: usize,
    #[module(skip)]
    min_w: usize,
}

impl<B: Backend> Generator<B> {
    /// z: (B, latent) -> mel: (B, 1, n_mels, time_frames) in [-1, 1]
    pub fn forward(&self, z: Tensor<B, 2>) -> Tensor<B, 4> {
        let b = z.dims()[0];
        let x = self
            .fc
            .forward(z)
            .reshape([b, self.f, self.min_h, self.min_w]);
        let x = relu(self.bn1.forward(self.ct1.forward(x)));
        let x = relu(self.bn2.forward(self.ct2.forward(x)));
        let x = relu(self.bn3.forward(self.ct3.forward(x)));
        tanh(self.ct4.forward(x))
    }

    /// Exponential moving average: `self` is the EMA shadow copy, `live` is
    /// the model currently being trained. Returns the new EMA state.
    /// Matches gan-zai-v5.3.py's ema_update: learnable parameters are
    /// blended `decay*ema + (1-decay)*live`; BatchNorm running stats
    /// (buffers, not learned) are copied straight from `live` rather than
    /// blended, exactly as the Python side does.
    pub fn ema_blend(&self, live: &Self, decay: f32) -> Self {
        Generator {
            fc: blend_linear(&self.fc, &live.fc, decay),
            ct1: blend_convt(&self.ct1, &live.ct1, decay),
            bn1: blend_bn(&self.bn1, &live.bn1, decay),
            ct2: blend_convt(&self.ct2, &live.ct2, decay),
            bn2: blend_bn(&self.bn2, &live.bn2, decay),
            ct3: blend_convt(&self.ct3, &live.ct3, decay),
            bn3: blend_bn(&self.bn3, &live.bn3, decay),
            ct4: blend_convt(&self.ct4, &live.ct4, decay),
            f: live.f,
            min_h: live.min_h,
            min_w: live.min_w,
        }
    }
}

fn blend_tensor<B: Backend, const D: usize>(ema: &Tensor<B, D>, live: &Tensor<B, D>, decay: f32) -> Tensor<B, D> {
    (ema.clone() * decay + live.clone() * (1.0 - decay)).detach()
}

fn blend_linear<B: Backend>(ema: &Linear<B>, live: &Linear<B>, decay: f32) -> Linear<B> {
    Linear {
        weight: Param::from_tensor(blend_tensor(&ema.weight.val(), &live.weight.val(), decay)),
        bias: match (&ema.bias, &live.bias) {
            (Some(e), Some(l)) => Some(Param::from_tensor(blend_tensor(&e.val(), &l.val(), decay))),
            _ => None,
        },
    }
}

fn blend_convt<B: Backend>(ema: &ConvTranspose2d<B>, live: &ConvTranspose2d<B>, decay: f32) -> ConvTranspose2d<B> {
    let mut out = live.clone();
    out.weight = Param::from_tensor(blend_tensor(&ema.weight.val(), &live.weight.val(), decay));
    out.bias = match (&ema.bias, &live.bias) {
        (Some(e), Some(l)) => Some(Param::from_tensor(blend_tensor(&e.val(), &l.val(), decay))),
        _ => None,
    };
    out
}

fn blend_bn<B: Backend>(ema: &BatchNorm<B>, live: &BatchNorm<B>, decay: f32) -> BatchNorm<B> {
    let mut out = live.clone();
    out.gamma = Param::from_tensor(blend_tensor(&ema.gamma.val(), &live.gamma.val(), decay));
    out.beta = Param::from_tensor(blend_tensor(&ema.beta.val(), &live.beta.val(), decay));
    // Running stats are buffers, not learned parameters -- copied straight
    // from the live model rather than blended (matches the Python side).
    out.running_mean.update(live.running_mean.value());
    out.running_var.update(live.running_var.value());
    out
}

/// Depthwise-separable conv with a smooth activation-sparsity gate.
/// ~8-10x fewer multiply-adds than a dense Conv2d of the same channel count,
/// since the spatial (depthwise) conv operates one-channel-at-a-time and only
/// the pointwise (1x1) conv mixes channels -- most of a dense conv's cost is
/// the cross-channel mixing at every spatial position, which this defers to a
/// cheap 1x1.
///
/// The gate matches gan-zai-v5.3.py's SparseBlock (post-fix): a sharp sigmoid
/// approximating a hard magnitude threshold, but -- unlike a true hard
/// threshold -- never exactly zero-gradient anywhere. The original (pre-fix)
/// hard-threshold version caused a real, observed training failure: it
/// collapsed the whole discriminator into a permanently frozen,
/// input-independent state after ~150 iterations in a live run (see
/// pipeline/gan-zai-v5.3.py's changelog / commit 8dfe364). This port only
/// ever implements the fixed (sigmoid-gated) version.
///
/// PERFORMANCE NOTE (Asahi/Mesa): the depthwise conv uses `with_groups(cin)`,
/// which creates up to 1024 groups at the deepest block. Grouped/depthwise
/// convolutions are a known weak spot on some wgpu/CubeCL + Mesa compute
/// paths -- they can be much slower than a single dense conv despite doing
/// fewer FLOPs, because the backend may not fuse them well. Set the env var
/// `BS_DENSE_DISC=1` to build the discriminator with plain DENSE 3x3 convs
/// (same sigmoid gate, same shapes) instead of depthwise-separable, so you
/// can measure which is actually faster on your GPU. On Apple Metal (macOS)
/// depthwise is typically fine; on Asahi it may not be.
#[derive(Module, Debug)]
pub struct SparseBlock<B: Backend> {
    /// In depthwise mode: the depthwise (grouped) conv, then `pointwise` mixes
    /// channels. In dense mode: a single dense conv cin->cout and `pointwise`
    /// is an identity 1x1 (kept for module-shape stability across modes).
    depthwise: Conv2d<B>,
    pointwise: Conv2d<B>,
    bn: BatchNorm<B>,
    #[module(skip)]
    threshold: f32,
    #[module(skip)]
    sharpness: f32,
    #[module(skip)]
    dense: bool,
}

impl<B: Backend> SparseBlock<B> {
    fn new(cin: usize, cout: usize, stride: usize, device: &B::Device) -> Self {
        let dense = std::env::var("BS_DENSE_DISC").ok().as_deref() == Some("1");
        if dense {
            // Single dense 3x3 conv cin->cout (more FLOPs, but ONE dispatch and
            // a well-optimized kernel on most backends). pointwise becomes a
            // cout->cout 1x1 that we still run (cheap) so the gate sits between
            // spatial mixing and channel refinement, matching the two-stage
            // structure.
            let conv = Conv2dConfig::new([cin, cout], [3, 3])
                .with_stride([stride, stride])
                .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
                .with_bias(false)
                .init(device);
            let pointwise = Conv2dConfig::new([cout, cout], [1, 1]).with_bias(false).init(device);
            SparseBlock { depthwise: conv, pointwise, bn: BatchNormConfig::new(cout).init(device), threshold: 0.05, sharpness: 40.0, dense: true }
        } else {
            let depthwise = Conv2dConfig::new([cin, cin], [3, 3])
                .with_stride([stride, stride])
                .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
                .with_groups(cin)
                .with_bias(false)
                .init(device);
            let pointwise = Conv2dConfig::new([cin, cout], [1, 1]).with_bias(false).init(device);
            SparseBlock { depthwise, pointwise, bn: BatchNormConfig::new(cout).init(device), threshold: 0.05, sharpness: 40.0, dense: false }
        }
    }

    fn forward(&self, x: Tensor<B, 4>) -> Tensor<B, 4> {
        // spatial conv (depthwise grouped, or dense cin->cout)
        let x = self.depthwise.forward(x);
        // sigmoid activation-sparsity gate (never exactly zero-gradient)
        let gate = sigmoid((x.clone().abs().sub_scalar(self.threshold)).mul_scalar(self.sharpness));
        let x = x * gate;
        // channel mix (pointwise 1x1)
        let x = self.pointwise.forward(x);
        leaky_relu(self.bn.forward(x), 0.2)
    }
}

#[derive(Module, Debug)]
pub struct Discriminator<B: Backend> {
    b1: SparseBlock<B>,
    b2: SparseBlock<B>,
    b3: SparseBlock<B>,
    b4: SparseBlock<B>,
    pool: AdaptiveAvgPool2d,
    fc: Linear<B>,
}

impl<B: Backend> Discriminator<B> {
    /// mel: (B, 1, n_mels, time_frames) -> logit: (B,)
    pub fn forward(&self, x: Tensor<B, 4>) -> Tensor<B, 1> {
        let x = self.b1.forward(x);
        let x = self.b2.forward(x);
        let x = self.b3.forward(x);
        let x = self.b4.forward(x);
        let x = self.pool.forward(x); // (B, f*8, 1, 1)
        let b = x.dims()[0];
        let x = x.flatten::<2>(1, 3); // (B, f*8)
        self.fc.forward(x).reshape([b]) // (B,)
    }
}

/// Numerically stable BCE-with-logits against a constant target (0.0 or ~0.9),
/// returned as a scalar mean. Matches nn.BCEWithLogitsLoss with a filled target.
pub fn bce_with_logits<B: Backend>(logits: Tensor<B, 1>, target: f32) -> Tensor<B, 1> {
    // mean( max(x,0) - x*z + log(1 + exp(-|x|)) )
    let max = logits.clone().clamp_min(0.0);
    let softplus = logits.clone().abs().neg().exp().add_scalar(1.0).log();
    (max - logits.mul_scalar(target) + softplus).mean()
}

/// Numerically stable `softplus(x) = log(1 + exp(x))`, via the identity
/// `softplus(x) = max(x,0) + log(1 + exp(-|x|))` (never overflows).
fn softplus<B: Backend>(x: Tensor<B, 1>) -> Tensor<B, 1> {
    let max = x.clone().clamp_min(0.0);
    max + x.abs().neg().exp().add_scalar(1.0).log()
}

/// **RpGAN** — the relativistic pairing loss from "The GAN is dead; long live
/// the GAN! A Modern Baseline GAN" (Huang et al., NeurIPS 2024 — R3GAN).
///
/// Where the classic GAN asks D "is this sample real?" (an absolute decision
/// boundary), RpGAN asks "is this real sample better than that fake one?" —
/// the critic scores a *pair*. That single change is what fixes mode dropping:
/// on the StackedMNIST 1000-mode benchmark the classic loss drops modes while
/// RpGAN reaches full coverage, cutting reverse-KL from 0.9270 to 0.0781.
///
///   L_D = mean( softplus( -(D(real) - D(fake)) ) )
///   L_G = mean( softplus(  (D(real) - D(fake)) ) )
///
/// The pairing is elementwise over the batch (sample i of real vs sample i of
/// fake), which is the standard cheap approximation to the full pairing
/// expectation. Note both losses need BOTH logit sets, so unlike the BCE path
/// the generator step must see D(real) too.
///
/// CAVEAT worth knowing: the R3GAN paper's full recipe is RpGAN + R1 + R2
/// (zero-centered gradient penalties on real AND fake). R1/R2 require
/// second-order gradients, which Burn's autodiff does not provide -- `grad()`
/// returns an InnerBackend tensor with no autodiff tracking, so there's
/// nothing to backprop through. The tractable stand-in is instance noise,
/// which Roth et al. (2017) showed is theoretically equivalent in effect to
/// the R1/R2 penalties -- and which this trainer already applies (decaying
/// sigma on D's inputs). The Python pipeline, which has PyTorch's double
/// backward, gets the exact R1/R2 with lazy regularization.
pub fn rp_gan_d<B: Backend>(d_real: Tensor<B, 1>, d_fake: Tensor<B, 1>) -> Tensor<B, 1> {
    softplus((d_real - d_fake).neg()).mean()
}

/// Generator side of the RpGAN objective (see [`rp_gan_d`]).
pub fn rp_gan_g<B: Backend>(d_real: Tensor<B, 1>, d_fake: Tensor<B, 1>) -> Tensor<B, 1> {
    softplus(d_real - d_fake).mean()
}
