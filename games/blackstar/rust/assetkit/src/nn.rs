//! Minimal CPU conv net — a reference implementation of the conditional
//! upscaler, so the architecture can be checked BEFORE any GPU time.
//!
//! Not a training rig: it is a falsifier. A GAN on a GPU will always produce
//! something and the loss will always fall, which makes it a poor instrument for
//! deciding whether a task is learnable. This trains a small U-Net on the real
//! pairs and compares it against the two baselines that matter -- returning the
//! input unchanged, and returning the training mean. A model that cannot beat
//! those has learned nothing, whatever its loss curve says.
//!
//! Deliberately dependency-free and f32 CPU: correctness over speed. Images are
//! downsampled from 128x256 so a run finishes in seconds rather than hours.

pub const IW: usize = 64;   // working width  (128x256 downsampled by 4)
pub const IH: usize = 32;   // working height

pub struct Rng(u64);
impl Rng {
    pub fn new(s: u64) -> Self { Rng(s | 1) }
    fn n(&mut self) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 }
    pub fn f(&mut self) -> f32 { (self.n() % 1_000_000) as f32 / 1_000_000.0 }
    /// Kaiming-ish: variance scaled by fan-in, which keeps activations alive
    /// through several layers instead of collapsing to zero.
    pub fn normal(&mut self, fan_in: usize) -> f32 {
        let s = (2.0 / fan_in as f32).sqrt();
        (self.f() + self.f() + self.f() - 1.5) * 2.0 * s
    }
}

#[derive(Clone)]
pub struct Conv {
    pub cin: usize, pub cout: usize, pub k: usize,
    pub w: Vec<f32>, pub b: Vec<f32>,
    pub gw: Vec<f32>, pub gb: Vec<f32>,
    // Adam state: plain SGD needs a hand-tuned lr per layer, Adam does not
    mw: Vec<f32>, vw: Vec<f32>, mb: Vec<f32>, vb: Vec<f32>, t: f32,
}

impl Conv {
    pub fn new(cin: usize, cout: usize, k: usize, rng: &mut Rng) -> Self {
        let n = cout * cin * k * k;
        Conv {
            cin, cout, k,
            w: (0..n).map(|_| rng.normal(cin * k * k)).collect(),
            b: vec![0.0; cout],
            gw: vec![0.0; n], gb: vec![0.0; cout],
            mw: vec![0.0; n], vw: vec![0.0; n],
            mb: vec![0.0; cout], vb: vec![0.0; cout], t: 0.0,
        }
    }

    /// same-size convolution, zero padded
    pub fn forward(&self, x: &[f32], h: usize, w: usize) -> Vec<f32> {
        let p = self.k / 2;
        let mut y = vec![0.0f32; self.cout * h * w];
        for oc in 0..self.cout {
            for yy in 0..h {
                for xx in 0..w {
                    let mut s = self.b[oc];
                    for ic in 0..self.cin {
                        for ky in 0..self.k {
                            let sy = yy as isize + ky as isize - p as isize;
                            if sy < 0 || sy >= h as isize { continue; }
                            for kx in 0..self.k {
                                let sx = xx as isize + kx as isize - p as isize;
                                if sx < 0 || sx >= w as isize { continue; }
                                s += self.w[((oc * self.cin + ic) * self.k + ky) * self.k + kx]
                                   * x[(ic * h + sy as usize) * w + sx as usize];
                            }
                        }
                    }
                    y[(oc * h + yy) * w + xx] = s;
                }
            }
        }
        y
    }

    /// returns dL/dx, accumulating dL/dw and dL/db
    pub fn backward(&mut self, x: &[f32], dy: &[f32], h: usize, w: usize) -> Vec<f32> {
        let p = self.k / 2;
        let mut dx = vec![0.0f32; self.cin * h * w];
        for oc in 0..self.cout {
            for yy in 0..h {
                for xx in 0..w {
                    let g = dy[(oc * h + yy) * w + xx];
                    if g == 0.0 { continue; }
                    self.gb[oc] += g;
                    for ic in 0..self.cin {
                        for ky in 0..self.k {
                            let sy = yy as isize + ky as isize - p as isize;
                            if sy < 0 || sy >= h as isize { continue; }
                            for kx in 0..self.k {
                                let sx = xx as isize + kx as isize - p as isize;
                                if sx < 0 || sx >= w as isize { continue; }
                                let wi = ((oc * self.cin + ic) * self.k + ky) * self.k + kx;
                                let xi = (ic * h + sy as usize) * w + sx as usize;
                                self.gw[wi] += g * x[xi];
                                dx[xi] += g * self.w[wi];
                            }
                        }
                    }
                }
            }
        }
        dx
    }

    pub fn step(&mut self, lr: f32) {
        self.t += 1.0;
        let (b1, b2, eps) = (0.9f32, 0.999f32, 1e-8f32);
        let c1 = 1.0 - b1.powf(self.t);
        let c2 = 1.0 - b2.powf(self.t);
        for i in 0..self.w.len() {
            self.mw[i] = b1 * self.mw[i] + (1.0 - b1) * self.gw[i];
            self.vw[i] = b2 * self.vw[i] + (1.0 - b2) * self.gw[i] * self.gw[i];
            self.w[i] -= lr * (self.mw[i] / c1) / ((self.vw[i] / c2).sqrt() + eps);
            self.gw[i] = 0.0;
        }
        for i in 0..self.b.len() {
            self.mb[i] = b1 * self.mb[i] + (1.0 - b1) * self.gb[i];
            self.vb[i] = b2 * self.vb[i] + (1.0 - b2) * self.gb[i] * self.gb[i];
            self.b[i] -= lr * (self.mb[i] / c1) / ((self.vb[i] / c2).sqrt() + eps);
            self.gb[i] = 0.0;
        }
    }
}

pub fn lrelu(x: &[f32]) -> Vec<f32> { x.iter().map(|v| if *v > 0.0 { *v } else { 0.2 * v }).collect() }
pub fn dlrelu(pre: &[f32], dy: &[f32]) -> Vec<f32> {
    pre.iter().zip(dy).map(|(p, g)| if *p > 0.0 { *g } else { 0.2 * g }).collect()
}
pub fn tanh_v(x: &[f32]) -> Vec<f32> { x.iter().map(|v| v.tanh()).collect() }
pub fn dtanh(out: &[f32], dy: &[f32]) -> Vec<f32> {
    out.iter().zip(dy).map(|(o, g)| g * (1.0 - o * o)).collect()
}

/// Generator: three same-size conv layers with a residual add. Small on purpose —
/// the question is whether the mapping is learnable at all, not how good it gets.
pub struct Gen { pub c1: Conv, pub c2: Conv, pub c3: Conv, pub cin: usize }

impl Gen {
    /// cin=1 is box only; cin=2 adds the CONTEXT plane as a real second channel.
    /// Blending context into the box instead (a first attempt) corrupts the box
    /// signal and measured WORSE -- 25.7% vs identity against 34.1% -- because the
    /// two planes mean different things and averaging them destroys both.
    pub fn new(f: usize, rng: &mut Rng) -> Self { Self::with_channels(1, f, rng) }
    pub fn with_channels(cin: usize, f: usize, rng: &mut Rng) -> Self {
        Gen { c1: Conv::new(cin, f, 3, rng), c2: Conv::new(f, f, 3, rng),
              c3: Conv::new(f, 1, 3, rng), cin }
    }
    /// x is cin planes concatenated; the residual is taken from the FIRST plane
    /// (the box), since that is the one the output should resemble.
    pub fn forward(&self, x: &[f32]) -> (Vec<f32>, Vec<Vec<f32>>) {
        let z1 = self.c1.forward(x, IH, IW);
        let a1 = lrelu(&z1);
        let z2 = self.c2.forward(&a1, IH, IW);
        let a2 = lrelu(&z2);
        let z3 = self.c3.forward(&a2, IH, IW);
        // residual: predict the DELTA from the blob, not the image from scratch.
        // Without this the net spends its capacity re-learning the silhouette it
        // was already given.
        let out: Vec<f32> = z3.iter().zip(x.iter().take(IH * IW))
                              .map(|(d, b)| (d + b).tanh()).collect();
        (out, vec![z1, a1, z2, a2, z3])
    }
    pub fn backward(&mut self, x: &[f32], cache: &[Vec<f32>], out: &[f32], dl: &[f32]) {
        let d3 = dtanh(out, dl);
        let d2 = self.c3.backward(&cache[3], &d3, IH, IW);
        let d2 = dlrelu(&cache[2], &d2);
        let d1 = self.c2.backward(&cache[1], &d2, IH, IW);
        let d1 = dlrelu(&cache[0], &d1);
        let _ = self.c1.backward(x, &d1, IH, IW);
    }
    pub fn step(&mut self, lr: f32) { self.c1.step(lr); self.c2.step(lr); self.c3.step(lr); }
}

/// Discriminator over the PAIR: input is (blob, candidate) on two channels, so it
/// can reject detail that is realistic but belongs to a different part.
pub struct Disc { pub c1: Conv, pub c2: Conv, pub c3: Conv }

impl Disc {
    pub fn new(f: usize, rng: &mut Rng) -> Self {
        Disc { c1: Conv::new(2, f, 3, rng), c2: Conv::new(f, f, 3, rng), c3: Conv::new(f, 1, 3, rng) }
    }
    fn stack(blob: &[f32], cand: &[f32]) -> Vec<f32> {
        let mut v = Vec::with_capacity(blob.len() * 2);
        v.extend_from_slice(blob); v.extend_from_slice(cand); v
    }
    /// mean logit over the map — a patch critic, which is what pix2pix uses
    pub fn forward(&self, blob: &[f32], cand: &[f32]) -> (f32, Vec<Vec<f32>>) {
        let x = Self::stack(blob, cand);
        let z1 = self.c1.forward(&x, IH, IW);
        let a1 = lrelu(&z1);
        let z2 = self.c2.forward(&a1, IH, IW);
        let a2 = lrelu(&z2);
        let z3 = self.c3.forward(&a2, IH, IW);
        let logit = z3.iter().sum::<f32>() / z3.len() as f32;
        (logit, vec![x, z1, a1, z2, a2, z3])
    }
    /// returns dL/d(candidate) so the generator can be pushed by the critic
    pub fn backward(&mut self, cache: &[Vec<f32>], dlogit: f32) -> Vec<f32> {
        let n = IH * IW;
        let d3 = vec![dlogit / n as f32; n];
        let d2 = self.c3.backward(&cache[4], &d3, IH, IW);
        let d2 = dlrelu(&cache[3], &d2);
        let d1 = self.c2.backward(&cache[2], &d2, IH, IW);
        let d1 = dlrelu(&cache[1], &d1);
        let dx = self.c1.backward(&cache[0], &d1, IH, IW);
        dx[n..].to_vec()          // gradient w.r.t. the candidate channel only
    }
    pub fn step(&mut self, lr: f32) { self.c1.step(lr); self.c2.step(lr); self.c3.step(lr); }
}

/// numerically stable BCE-with-logits, matching the kBM3 loss
pub fn bce(logit: f32, target: f32) -> (f32, f32) {
    let max = logit.max(0.0);
    let sp = (1.0 + (-logit.abs()).exp()).ln();
    let loss = max - logit * target + sp;
    let s = 1.0 / (1.0 + (-logit).exp());
    (loss, s - target)
}

/// 128x256 -> 32x64 by box average
pub fn downsample(img: &[f32], w: usize, h: usize) -> Vec<f32> {
    let fx = w / IW;
    let fy = h / IH;
    let mut o = vec![0.0f32; IH * IW];
    for y in 0..IH {
        for x in 0..IW {
            let mut s = 0.0;
            for dy in 0..fy { for dx in 0..fx { s += img[(y * fy + dy) * w + x * fx + dx]; } }
            o[y * IW + x] = s / (fx * fy) as f32;
        }
    }
    o
}
