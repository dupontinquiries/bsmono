// beatspace-vocoder -- HiFi-GAN-style neural vocoder trained on the GPU (Vulkan).
//
// Companion to the Rust GAN trainer: mel -> waveform, so BOTH models train on
// the Apple GPU via Mesa's Vulkan driver. CubeCL has no FFT, so the STFT loss is
// a differentiable framing + windowing + DFT-matmul (GPUs love matmuls).
//
//   WGPU_BACKEND=vulkan cargo run --release --bin vocoder -- train --iters 200000
//   WGPU_BACKEND=vulkan cargo run --release --bin vocoder -- smoke
//
#![recursion_limit = "256"]

#[path = "../audio.rs"]
mod audio;

use std::time::Instant;

use burn::backend::wgpu::WgpuDevice;
use burn::backend::{Autodiff, Wgpu};
use burn::module::Module;
use burn::nn::conv::{
    Conv1d, Conv1dConfig, Conv2d, Conv2dConfig, ConvTranspose1d, ConvTranspose1dConfig,
};
use burn::nn::{PaddingConfig1d, PaddingConfig2d};
use burn::optim::{AdamConfig, GradientsParams, Optimizer};
use burn::prelude::*;
use burn::tensor::activation::{leaky_relu, tanh};
use burn::tensor::{Distribution, TensorData};

type BE = Wgpu<f32, i32>;
type AB = Autodiff<BE>;

const HOP: usize = 512; // fixed: upsample rates below multiply to this
const UPS: [usize; 4] = [8, 8, 4, 2];
const UPK: [usize; 4] = [16, 16, 8, 4];
const RES_K: [usize; 3] = [3, 7, 11];
const RES_D: [usize; 3] = [1, 3, 5]; // one dilation set per resblock kernel

fn envu(k: &str, d: usize) -> usize {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

// --------------------------------------------------------------------------- //
// STFT magnitude via DFT matmul (differentiable, no FFT primitive needed)
// --------------------------------------------------------------------------- //
struct Dft<B: Backend> {
    cos: Tensor<B, 2>, // [n_fft, n_freq]
    sin: Tensor<B, 2>, // [n_fft, n_freq]
    win: Tensor<B, 1>, // [n_fft]
    n_fft: usize,
    hop: usize,
}
fn make_dft<B: Backend>(n_fft: usize, hop: usize, device: &B::Device) -> Dft<B> {
    let n_freq = n_fft / 2 + 1;
    let mut cos = vec![0f32; n_fft * n_freq];
    let mut sin = vec![0f32; n_fft * n_freq];
    for n in 0..n_fft {
        for k in 0..n_freq {
            let a = 2.0 * std::f32::consts::PI * (k as f32) * (n as f32) / (n_fft as f32);
            cos[n * n_freq + k] = a.cos();
            sin[n * n_freq + k] = -a.sin();
        }
    }
    let win: Vec<f32> = (0..n_fft)
        .map(|n| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * n as f32 / n_fft as f32).cos())
        .collect();
    Dft {
        cos: Tensor::from_data(TensorData::new(cos, [n_fft, n_freq]), device),
        sin: Tensor::from_data(TensorData::new(sin, [n_fft, n_freq]), device),
        win: Tensor::from_data(TensorData::new(win, [n_fft]), device),
        n_fft,
        hop,
    }
}
impl<B: Backend> Dft<B> {
    /// wav [Bt, L] -> magnitude [Bt, n_frames, n_freq]
    fn mag(&self, wav: Tensor<B, 2>) -> Tensor<B, 3> {
        let [bt, l] = wav.dims();
        let n_frames = (l - self.n_fft) / self.hop + 1;
        let mut frames = Vec::with_capacity(n_frames);
        for i in 0..n_frames {
            let s = i * self.hop;
            frames.push(wav.clone().slice([0..bt, s..s + self.n_fft])); // [bt, n_fft]
        }
        let x = Tensor::stack::<3>(frames, 1); // [bt, n_frames, n_fft]
        let x = x.mul(self.win.clone().reshape([1, 1, self.n_fft]));
        let x = x.reshape([bt * n_frames, self.n_fft]);
        let re = x.clone().matmul(self.cos.clone()); // [bt*nf, n_freq]
        let im = x.matmul(self.sin.clone());
        let mag = (re.clone() * re + im.clone() * im).clamp_min(1e-8).sqrt();
        let n_freq = self.n_fft / 2 + 1;
        mag.reshape([bt, n_frames, n_freq])
    }
}

/// Multi-resolution STFT loss: spectral convergence + log-magnitude L1.
fn mrstft_loss<B: Backend>(pred: Tensor<B, 2>, target: Tensor<B, 2>, dfts: &[Dft<B>]) -> Tensor<B, 1> {
    let mut total = Tensor::<B, 1>::zeros([1], &pred.device());
    for d in dfts {
        let p = d.mag(pred.clone());
        let t = d.mag(target.clone());
        let diff = t.clone() - p.clone();
        let sc = fro(diff) / (fro(t.clone()) + 1e-7);
        let mag = (p.clamp_min(1e-7).log() - t.clamp_min(1e-7).log()).abs().mean();
        total = total + sc + mag;
    }
    total / (dfts.len() as f32)
}
fn fro<B: Backend>(x: Tensor<B, 3>) -> Tensor<B, 1> {
    x.powf_scalar(2.0).sum().sqrt()
}

// --------------------------------------------------------------------------- //
// Generator
// --------------------------------------------------------------------------- //
#[derive(Module, Debug)]
struct ResBlock<B: Backend> {
    c1: Conv1d<B>,
    c2: Conv1d<B>,
}
impl<B: Backend> ResBlock<B> {
    fn new(ch: usize, k: usize, d: usize, device: &B::Device) -> Self {
        ResBlock {
            c1: Conv1dConfig::new(ch, ch, k)
                .with_dilation(d)
                .with_padding(PaddingConfig1d::Explicit(d * (k - 1) / 2, d * (k - 1) / 2))
                .init(device),
            c2: Conv1dConfig::new(ch, ch, k)
                .with_padding(PaddingConfig1d::Explicit((k - 1) / 2, (k - 1) / 2))
                .init(device),
        }
    }
    fn forward(&self, x: Tensor<B, 3>) -> Tensor<B, 3> {
        let xt = self.c2.forward(leaky_relu(self.c1.forward(leaky_relu(x.clone(), 0.1)), 0.1));
        x + xt
    }
}

#[derive(Module, Debug)]
struct VocGen<B: Backend> {
    conv_pre: Conv1d<B>,
    ups: Vec<ConvTranspose1d<B>>,
    res: Vec<ResBlock<B>>,
    conv_post: Conv1d<B>,
    #[module(skip)]
    num_up: usize,
    #[module(skip)]
    num_res: usize,
}
impl<B: Backend> VocGen<B> {
    fn new(n_mels: usize, ch: usize, device: &B::Device) -> Self {
        let mut ups = Vec::new();
        let mut res = Vec::new();
        for i in 0..UPS.len() {
            let cin = ch >> i;
            let cout = ch >> (i + 1);
            ups.push(
                ConvTranspose1dConfig::new([cin, cout], UPK[i])
                    .with_stride(UPS[i])
                    .with_padding((UPK[i] - UPS[i]) / 2)
                    .init(device),
            );
            for j in 0..RES_K.len() {
                res.push(ResBlock::new(cout, RES_K[j], RES_D[j], device));
            }
        }
        let last = ch >> UPS.len();
        VocGen {
            conv_pre: Conv1dConfig::new(n_mels, ch, 7)
                .with_padding(PaddingConfig1d::Explicit(3, 3))
                .init(device),
            ups,
            res,
            conv_post: Conv1dConfig::new(last, 1, 7)
                .with_padding(PaddingConfig1d::Explicit(3, 3))
                .init(device),
            num_up: UPS.len(),
            num_res: RES_K.len(),
        }
    }
    /// mel [B, n_mels, T] -> wav [B, 1, T*HOP]
    fn forward(&self, mel: Tensor<B, 3>) -> Tensor<B, 3> {
        let mut x = self.conv_pre.forward(mel);
        for i in 0..self.num_up {
            x = self.ups[i].forward(leaky_relu(x, 0.1));
            let mut xs = self.res[i * self.num_res].forward(x.clone());
            for j in 1..self.num_res {
                xs = xs + self.res[i * self.num_res + j].forward(x.clone());
            }
            x = xs / (self.num_res as f32);
        }
        tanh(self.conv_post.forward(leaky_relu(x, 0.1)))
    }
}

// --------------------------------------------------------------------------- //
// Multi-Period Discriminator
// --------------------------------------------------------------------------- //
#[derive(Module, Debug)]
struct PeriodDisc<B: Backend> {
    convs: Vec<Conv2d<B>>,
    post: Conv2d<B>,
    #[module(skip)]
    period: usize,
}
impl<B: Backend> PeriodDisc<B> {
    fn new(period: usize, device: &B::Device) -> Self {
        let ch = [1usize, 32, 128, 256, 512, 512];
        let mut convs = Vec::new();
        for i in 0..5 {
            let stride = if i < 4 { 3 } else { 1 };
            convs.push(
                Conv2dConfig::new([ch[i], ch[i + 1]], [5, 1])
                    .with_stride([stride, 1])
                    .with_padding(PaddingConfig2d::Explicit(2, 0, 2, 0))
                    .init(device),
            );
        }
        PeriodDisc {
            convs,
            post: Conv2dConfig::new([512, 1], [3, 1])
                .with_padding(PaddingConfig2d::Explicit(1, 0, 1, 0))
                .init(device),
            period,
        }
    }
    /// wav [B,1,L] -> (logits [B,N], feature maps)
    fn forward(&self, wav: Tensor<B, 3>) -> (Tensor<B, 2>, Vec<Tensor<B, 4>>) {
        let [b, c, l] = wav.dims();
        let pad = (self.period - (l % self.period)) % self.period;
        let wav = if pad > 0 {
            Tensor::cat(vec![wav.clone(), wav.slice([0..b, 0..c, 0..pad])], 2)
        } else {
            wav
        };
        let l2 = l + pad;
        let mut x = wav.reshape([b, c, l2 / self.period, self.period]);
        let mut feats = Vec::new();
        for conv in &self.convs {
            x = leaky_relu(conv.forward(x), 0.1);
            feats.push(x.clone());
        }
        x = self.post.forward(x);
        feats.push(x.clone());
        let [bb, cc, hh, ww] = x.dims();
        (x.reshape([bb, cc * hh * ww]), feats)
    }
}

// --------------------------------------------------------------------------- //
// Real data: discover audio files once, decode + extract a matching (mel, wav)
// segment per draw. Mirrors vocoder.py's VocoderDataset (which also decodes a
// random file per __getitem__ -- this isn't a regression vs. the Python side).
// --------------------------------------------------------------------------- //
struct RealVocoderData {
    files: Vec<std::path::PathBuf>,
    sr: u32,
    n_fft: usize,
    hop: usize,
    n_mels: usize,
    db_floor: f64,
    db_ceil: f64,
}
impl RealVocoderData {
    fn discover(data_dir: &str, sr: u32, n_fft: usize, hop: usize, n_mels: usize) -> Self {
        let mut files = Vec::new();
        for entry in walk(std::path::Path::new(data_dir)) {
            if let Some(ext) = entry.extension().and_then(|e| e.to_str()) {
                if ext.eq_ignore_ascii_case("opus") || ext.eq_ignore_ascii_case("wav") {
                    files.push(entry);
                }
            }
        }
        if files.is_empty() {
            eprintln!("warning: no .opus/.wav files found under {data_dir}");
        } else {
            println!("RealVocoderData: {} audio files under {data_dir}", files.len());
        }
        Self { files, sr, n_fft, hop, n_mels, db_floor: -80.0, db_ceil: 40.0 }
    }

    /// One (mel, wav) pair: mel [n_mels, seg_frames], wav [seg_frames*hop] f32.
    /// Retries a few times on decode failure / too-short files, then falls
    /// back to silence (matches vocoder.py's fallback behavior).
    fn next_pair(&self, seg_frames: usize) -> (Vec<f32>, Vec<f32>) {
        let seg_len = seg_frames * self.hop;
        for _ in 0..4 {
            if self.files.is_empty() {
                break;
            }
            let path = &self.files[fastrand::usize(0..self.files.len())];
            let y = match audio::decode_audio(path, self.sr) {
                Ok(y) => y,
                Err(_) => continue,
            };
            if y.len() < seg_len + self.hop {
                continue;
            }
            let start = fastrand::usize(0..(y.len() - seg_len - self.hop));
            let wav_seg = y[start..start + seg_len].to_vec();
            // mel spans the segment + one hop, matching vocoder.py's alignment
            let mel_input = &y[start..start + seg_len + self.hop];
            let (mel, n_frames) = audio::wav_to_unit_mel(mel_input, self.sr as f64, self.n_fft, self.hop, self.n_mels, self.db_floor, self.db_ceil);
            if n_frames < seg_frames {
                continue;
            }
            // crop to exactly seg_frames columns (row-major [n_mels, n_frames])
            let mut cropped = vec![0f32; self.n_mels * seg_frames];
            for m in 0..self.n_mels {
                cropped[m * seg_frames..(m + 1) * seg_frames].copy_from_slice(&mel[m * n_frames..m * n_frames + seg_frames]);
            }
            return (cropped, wav_seg);
        }
        (vec![-1.0; self.n_mels * seg_frames], vec![0.0; seg_len])
    }
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

// --------------------------------------------------------------------------- //
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("smoke");
    let iters = args
        .iter()
        .position(|a| a == "--iters")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(200000usize);
    let batch = envu("BS_BATCH", 8);
    let n_mels = envu("BS_NMELS", 256);
    let ch = envu("BS_CH", 128);
    let seg = envu("BS_SEG", 32); // mel frames per segment
    let data_dir = arg::<String>(&args, "--data");
    let synthetic = args.iter().any(|a| a == "--synthetic") || data_dir.is_none();

    let device = WgpuDevice::default();
    println!("Backend: Wgpu (Vulkan). Device: {device:?}");
    println!("vocoder: n_mels={n_mels} ch={ch} seg_frames={seg} wav_len={}", seg * HOP);

    let periods = [2usize, 3, 5, 7, 11];
    let stft_cfgs = [(512usize, 128usize), (1024, 256), (256, 64)];

    match cmd {
        "smoke" => {
            let g = VocGen::<BE>::new(n_mels, ch, &device);
            let mel = Tensor::<BE, 3>::random(
                [2, n_mels, seg],
                Distribution::Uniform(-1.0, 1.0),
                &device,
            );
            let t0 = Instant::now();
            let wav = g.forward(mel);
            let _ = wav.to_data();
            println!("G: mel[2,{n_mels},{seg}] -> wav{:?} (expected [...,{}])", wav.dims(), seg * HOP);
            let d = PeriodDisc::<BE>::new(3, &device);
            let (logit, feats) = d.forward(wav);
            println!("MPD(period 3): logits{:?}, {} feature maps", logit.dims(), feats.len());
            println!("first forward (incl. kernel compile): {} ms", t0.elapsed().as_millis());
        }
        "train" => train(n_mels, ch, seg, batch, iters, &periods, &stft_cfgs, &device, data_dir.as_deref(), synthetic),
        other => {
            eprintln!("unknown '{other}'. use: smoke | train");
            std::process::exit(2);
        }
    }
}

fn train(
    n_mels: usize,
    ch: usize,
    seg: usize,
    batch: usize,
    iters: usize,
    periods: &[usize],
    stft_cfgs: &[(usize, usize)],
    device: &WgpuDevice,
    data_dir: Option<&str>,
    synthetic: bool,
) {
    let wav_len = seg * HOP;
    const SR: u32 = 48000;
    const N_FFT: usize = 4096;
    let real_data = if synthetic {
        None
    } else {
        Some(RealVocoderData::discover(data_dir.unwrap(), SR, N_FFT, HOP, n_mels))
    };
    let mut g = VocGen::<AB>::new(n_mels, ch, device);
    let mut discs: Vec<PeriodDisc<AB>> = periods.iter().map(|&p| PeriodDisc::new(p, device)).collect();
    let mut opt_g = AdamConfig::new().with_beta_1(0.8).init();
    let mut opt_d: Vec<_> = periods.iter().map(|_| AdamConfig::new().with_beta_1(0.8).init()).collect();
    let dfts: Vec<Dft<AB>> = stft_cfgs.iter().map(|&(n, h)| make_dft(n, h, device)).collect();
    let lr = 2e-4;
    let (lambda_stft, lambda_fm) = (2.5f32, 2.0f32);

    println!("Training {iters} iters, batch {batch}, on GPU (Vulkan). {}",
             if real_data.is_some() { "Real audio data." } else { "Synthetic pairs." });
    let t0 = Instant::now();
    for step in 0..iters {
        let (mel, wav) = if let Some(ref rd) = real_data {
            let mut mel_buf = Vec::with_capacity(batch * n_mels * seg);
            let mut wav_buf = Vec::with_capacity(batch * wav_len);
            for _ in 0..batch {
                let (m, w) = rd.next_pair(seg);
                mel_buf.extend_from_slice(&m);
                wav_buf.extend_from_slice(&w);
            }
            let mel = Tensor::<AB, 1>::from_floats(mel_buf.as_slice(), device).reshape([batch, n_mels, seg]);
            let wav = Tensor::<AB, 1>::from_floats(wav_buf.as_slice(), device).reshape([batch, 1, wav_len]);
            (mel, wav)
        } else {
            let mel = Tensor::<AB, 3>::random([batch, n_mels, seg], Distribution::Uniform(-1.0, 1.0), device);
            let wav = Tensor::<AB, 3>::random([batch, 1, wav_len], Distribution::Uniform(-1.0, 1.0), device);
            (mel, wav)
        };

        let fake = g.forward(mel);
        let fake = fake.clone().slice([0..batch, 0..1, 0..wav_len]);

        // ---- Discriminators (LS-GAN) ----
        let mut loss_d_val = 0f32;
        for (di, d) in discs.iter_mut().enumerate() {
            let (dr, _) = d.forward(wav.clone());
            let (df, _) = d.forward(fake.clone().detach());
            let loss = (dr.clone() - 1.0).powf_scalar(2.0).mean() + df.clone().powf_scalar(2.0).mean();
            loss_d_val += loss.clone().into_scalar();
            let grads = loss.backward();
            let gp = GradientsParams::from_grads(grads, d);
            *d = opt_d[di].step(lr, d.clone(), gp);
        }

        // ---- Generator: adversarial + feature matching + multi-res STFT ----
        let mut loss_adv = Tensor::<AB, 1>::zeros([1], device);
        let mut loss_fm = Tensor::<AB, 1>::zeros([1], device);
        for d in &discs {
            let (df, ff) = d.forward(fake.clone());
            let (_, fr) = d.forward(wav.clone());
            loss_adv = loss_adv + (df - 1.0).powf_scalar(2.0).mean();
            for (a, b) in fr.iter().zip(ff.iter()) {
                loss_fm = loss_fm + (b.clone() - a.clone().detach()).abs().mean();
            }
        }
        let loss_stft = mrstft_loss(
            fake.clone().reshape([batch, wav_len]),
            wav.clone().reshape([batch, wav_len]),
            &dfts,
        );
        let loss_g = loss_adv.clone() + loss_fm.clone() * lambda_fm + loss_stft.clone() * lambda_stft;
        let grads = loss_g.backward();
        let gp = GradientsParams::from_grads(grads, &g);
        g = opt_g.step(lr, g, gp);

        if step % 10 == 0 || step + 1 == iters {
            let ms = t0.elapsed().as_millis() as f64 / (step + 1) as f64;
            println!(
                "[{step}/{iters}] D:{:.3} G:{:.3} stft:{:.3} adv:{:.3} fm:{:.3} ({ms:.0}ms/it)",
                loss_d_val,
                loss_g.into_scalar(),
                loss_stft.into_scalar(),
                loss_adv.into_scalar(),
                loss_fm.into_scalar(),
            );
        }
    }
    println!("Done in {:.1}s.", t0.elapsed().as_secs_f64());
    let _ = Module::num_params(&g);
}

fn arg<T: std::str::FromStr>(args: &[String], key: &str) -> Option<T> {
    let i = args.iter().position(|a| a == key)?;
    args.get(i + 1)?.parse().ok()
}
