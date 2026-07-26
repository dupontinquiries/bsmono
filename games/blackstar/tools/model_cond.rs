//! Conditional (blob -> detail) GAN for 3D shape upscaling, in the same Burn
//! idiom as `model.rs` so it drops into the existing trainer.
//!
//! Why this rather than the unconditional model: `Generator` maps a latent to an
//! image, so it can only invent a plausible sample, not upscale a GIVEN one. For
//! detail transfer the generator has to see the low-detail shape and add to it,
//! and the critic has to judge the PAIR -- otherwise it happily accepts detail
//! that is realistic but belongs to a different part.
//!
//! Shapes are unchanged from kBM3: single-channel 128x256 in [-1,1], which is how
//! a 3D component is encoded (six orthographic depth views at 64x64 tiled 2x4).
//! `MelCache` therefore loads this data as-is; only the batch needs to yield two
//! tensors (blob, detail) instead of one.
//!
//! Generator is a U-Net: four stride-2 encoder convs down to (B,f,8,16), four
//! stride-2 transposed convs back up, with skip connections concatenated at each
//! level. The skips matter -- without them the bottleneck discards the silhouette
//! and the output drifts off the input shape, which is exactly the "detail that
//! has nothing to do with the model" failure.

use burn::nn::conv::{Conv2d, Conv2dConfig, ConvTranspose2d, ConvTranspose2dConfig};
use burn::nn::pool::{AdaptiveAvgPool2d, AdaptiveAvgPool2dConfig};
use burn::nn::{BatchNorm, BatchNormConfig, Linear, LinearConfig, PaddingConfig2d};
use burn::prelude::*;
use burn::tensor::activation::{leaky_relu, relu, tanh};

#[derive(Config, Debug)]
pub struct CondGanConfig {
    #[config(default = 256)]
    pub g_filters: usize,
    #[config(default = 64)]
    pub d_filters: usize,
    #[config(default = 128)]
    pub height: usize,
    #[config(default = 256)]
    pub width: usize,
    /// weight on the L1 reconstruction term; pix2pix uses 100.0. Pure adversarial
    /// loss on a few thousand samples collapses -- L1 anchors the output to the
    /// target while the critic supplies sharpness.
    #[config(default = 100.0)]
    pub l1_weight: f32,
}

impl CondGanConfig {
    pub fn init_generator<B: Backend>(&self, device: &B::Device) -> CondGenerator<B> {
        let f = self.g_filters;
        CondGenerator {
            e1: conv(1, f / 8, device),
            e2: conv(f / 8, f / 4, device),
            eb2: BatchNormConfig::new(f / 4).init(device),
            e3: conv(f / 4, f / 2, device),
            eb3: BatchNormConfig::new(f / 2).init(device),
            e4: conv(f / 2, f, device),
            eb4: BatchNormConfig::new(f).init(device),

            d1: convt(f, f / 2, device),
            db1: BatchNormConfig::new(f / 2).init(device),
            d2: convt(f, f / 4, device), // f/2 + skip f/2 = f
            db2: BatchNormConfig::new(f / 4).init(device),
            d3: convt(f / 2, f / 8, device), // f/4 + skip f/4 = f/2
            db3: BatchNormConfig::new(f / 8).init(device),
            d4: convt(f / 4, 1, device), // f/8 + skip f/8 = f/4
        }
    }

    /// Critic over the PAIR: input is (blob, detail) stacked on the channel axis.
    pub fn init_discriminator<B: Backend>(&self, device: &B::Device) -> CondDiscriminator<B> {
        let f = self.d_filters;
        CondDiscriminator {
            c1: conv(2, f, device), // 2 channels: condition + candidate
            c2: conv(f, f * 2, device),
            bn2: BatchNormConfig::new(f * 2).init(device),
            c3: conv(f * 2, f * 4, device),
            bn3: BatchNormConfig::new(f * 4).init(device),
            c4: conv(f * 4, f * 8, device),
            bn4: BatchNormConfig::new(f * 8).init(device),
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

fn conv<B: Backend>(cin: usize, cout: usize, device: &B::Device) -> Conv2d<B> {
    Conv2dConfig::new([cin, cout], [4, 4])
        .with_stride([2, 2])
        .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
        .init(device)
}

#[derive(Module, Debug)]
pub struct CondGenerator<B: Backend> {
    e1: Conv2d<B>,
    e2: Conv2d<B>,
    eb2: BatchNorm<B>,
    e3: Conv2d<B>,
    eb3: BatchNorm<B>,
    e4: Conv2d<B>,
    eb4: BatchNorm<B>,
    d1: ConvTranspose2d<B>,
    db1: BatchNorm<B>,
    d2: ConvTranspose2d<B>,
    db2: BatchNorm<B>,
    d3: ConvTranspose2d<B>,
    db3: BatchNorm<B>,
    d4: ConvTranspose2d<B>,
}

impl<B: Backend> CondGenerator<B> {
    /// blob: (B,1,128,256) in [-1,1] -> detail: (B,1,128,256) in [-1,1]
    pub fn forward(&self, blob: Tensor<B, 4>) -> Tensor<B, 4> {
        // encoder
        let s1 = leaky_relu(self.e1.forward(blob), 0.2); // (f/8, 64,128)
        let s2 = leaky_relu(self.eb2.forward(self.e2.forward(s1.clone())), 0.2); // (f/4, 32,64)
        let s3 = leaky_relu(self.eb3.forward(self.e3.forward(s2.clone())), 0.2); // (f/2, 16,32)
        let x = leaky_relu(self.eb4.forward(self.e4.forward(s3.clone())), 0.2); // (f,   8,16)

        // decoder, concatenating the matching encoder level at each step so the
        // input silhouette survives the bottleneck
        let x = relu(self.db1.forward(self.d1.forward(x))); // (f/2, 16,32)
        let x = Tensor::cat(vec![x, s3], 1); // (f,    16,32)
        let x = relu(self.db2.forward(self.d2.forward(x))); // (f/4, 32,64)
        let x = Tensor::cat(vec![x, s2], 1); // (f/2,  32,64)
        let x = relu(self.db3.forward(self.d3.forward(x))); // (f/8, 64,128)
        let x = Tensor::cat(vec![x, s1], 1); // (f/4,  64,128)
        tanh(self.d4.forward(x)) // (1,   128,256)
    }
}

#[derive(Module, Debug)]
pub struct CondDiscriminator<B: Backend> {
    c1: Conv2d<B>,
    c2: Conv2d<B>,
    bn2: BatchNorm<B>,
    c3: Conv2d<B>,
    bn3: BatchNorm<B>,
    c4: Conv2d<B>,
    bn4: BatchNorm<B>,
    pool: AdaptiveAvgPool2d,
    fc: Linear<B>,
}

impl<B: Backend> CondDiscriminator<B> {
    /// (blob, candidate) -> logit: (B,)
    pub fn forward(&self, blob: Tensor<B, 4>, cand: Tensor<B, 4>) -> Tensor<B, 1> {
        let x = Tensor::cat(vec![blob, cand], 1); // (B,2,128,256)
        let x = leaky_relu(self.c1.forward(x), 0.2);
        let x = leaky_relu(self.bn2.forward(self.c2.forward(x)), 0.2);
        let x = leaky_relu(self.bn3.forward(self.c3.forward(x)), 0.2);
        let x = leaky_relu(self.bn4.forward(self.c4.forward(x)), 0.2);
        let x = self.pool.forward(x);
        let b = x.dims()[0];
        let x = x.flatten::<2>(1, 3);
        self.fc.forward(x).reshape([b])
    }
}

/// Mean absolute error, the pix2pix reconstruction term.
pub fn l1_loss<B: Backend>(pred: Tensor<B, 4>, target: Tensor<B, 4>) -> Tensor<B, 1> {
    (pred - target).abs().mean()
}

/// Same numerically stable BCE-with-logits as the unconditional model.
pub fn bce_with_logits<B: Backend>(logits: Tensor<B, 1>, target: f32) -> Tensor<B, 1> {
    let max = logits.clone().clamp_min(0.0);
    let softplus = logits.clone().abs().neg().exp().add_scalar(1.0).log();
    (max - logits.mul_scalar(target) + softplus).mean()
}

// ---------------------------------------------------------------------------
// Training step, for reference. Mirrors the unconditional loop in main.rs:
//
//   let fake = g.forward(blob.clone());
//   // D: real pair -> ~0.9, fake pair -> 0
//   let d_loss = bce_with_logits(d.forward(blob.clone(), detail.clone()), 0.9)
//              + bce_with_logits(d.forward(blob.clone(), fake.clone().detach()), 0.0);
//   // G: fool D, and stay close to the target
//   let g_loss = bce_with_logits(d.forward(blob.clone(), fake.clone()), 1.0)
//              + l1_loss(fake, detail).mul_scalar(cfg.l1_weight);
//
// The L1 term is what makes this trainable on a few thousand samples: pure
// adversarial loss at that scale collapses onto whichever donor dominates the
// set, which is the same failure as pasting one model's parts onto everything.
// ---------------------------------------------------------------------------
