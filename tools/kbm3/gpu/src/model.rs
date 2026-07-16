//! The kBM3 GAN in Burn, architecturally identical to the PyTorch v5 model so
//! that shapes and behaviour match: Generator projects a latent to (B,f,8,16)
//! and upsamples with four stride-2 ConvTranspose2d blocks to (B,1,128,256);
//! Discriminator is a strided-conv DCGAN critic ending in AdaptiveAvgPool -> Linear.
//!
//! Being generic over `B: Backend`, the SAME code runs on the Autodiff<Wgpu>
//! (Vulkan) backend for GPU training and on plain Wgpu for inference.


use burn::nn::conv::{Conv2d, Conv2dConfig, ConvTranspose2d, ConvTranspose2dConfig};
use burn::nn::pool::{AdaptiveAvgPool2d, AdaptiveAvgPool2dConfig};
use burn::nn::{BatchNorm, BatchNormConfig, Linear, LinearConfig, PaddingConfig2d};
use burn::prelude::*;
use burn::tensor::activation::{leaky_relu, relu, tanh};

#[derive(Config, Debug)]
pub struct GanConfig {
    #[config(default = 128)]
    pub latent: usize,
    #[config(default = 256)]
    pub g_filters: usize,
    #[config(default = 64)]
    pub d_filters: usize,
    #[config(default = 128)]
    pub n_mels: usize,
    #[config(default = 256)]
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
        Generator {
            fc: LinearConfig::new(self.latent, f * self.min_h() * self.min_w()).init(device),
            ct1: convt(f, f / 2, device),
            bn1: BatchNormConfig::new(f / 2).init(device),
            ct2: convt(f / 2, f / 4, device),
            bn2: BatchNormConfig::new(f / 4).init(device),
            ct3: convt(f / 4, f / 8, device),
            bn3: BatchNormConfig::new(f / 8).init(device),
            ct4: convt(f / 8, 1, device),
            f,
            min_h: self.min_h(),
            min_w: self.min_w(),
        }
    }

    pub fn init_discriminator<B: Backend>(&self, device: &B::Device) -> Discriminator<B> {
        let f = self.d_filters;
        Discriminator {
            c1: conv(1, f, device),
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
}

#[derive(Module, Debug)]
pub struct Discriminator<B: Backend> {
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

impl<B: Backend> Discriminator<B> {
    /// mel: (B, 1, n_mels, time_frames) -> logit: (B,)
    pub fn forward(&self, x: Tensor<B, 4>) -> Tensor<B, 1> {
        let x = leaky_relu(self.c1.forward(x), 0.2);
        let x = leaky_relu(self.bn2.forward(self.c2.forward(x)), 0.2);
        let x = leaky_relu(self.bn3.forward(self.c3.forward(x)), 0.2);
        let x = leaky_relu(self.bn4.forward(self.c4.forward(x)), 0.2);
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
