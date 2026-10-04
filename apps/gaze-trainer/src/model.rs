//! The model: what turns a feature vector into a point on the screen, small enough to train on the phone in seconds.
//!
//! Pipeline: standardise every feature, project onto the leading principal components (a few hundred pixels of eye patch
//! carry a handful of directions that matter), scale them all by ONE common factor, then either a ridge regression (closed form, the baseline every
//! network has to beat) or a one-hidden-layer tanh network trained with Adam. `Kind::Mean` predicts the centre of the
//! training targets, the number to beat before anything counts as learning.

use crate::dataset::Sample;
use crate::rng::Rng;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind { Mean, Ridge, Mlp }

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub kind: Kind,
    /// Principal components kept.
    pub pca_k: usize,
    pub lambda: f32,
    pub hidden: usize,
    pub epochs: usize,
    pub lr: f32,
    pub seed: u64,
}

impl Default for Config {
    fn default() -> Self { Config { kind: Kind::Mlp, pca_k: 48, lambda: 1.0, hidden: 32, epochs: 150, lr: 0.01, seed: 1 } }
}

#[derive(Debug, Clone, PartialEq)]
struct Standardizer { mean: Vec<f32>, inv_sd: Vec<f32> }

impl Standardizer {
    fn fit(x: &[&[f32]]) -> Standardizer {
        let (n, d) = (x.len() as f32, x[0].len());
        let mut mean = vec![0f32; d];
        for r in x { for (m, v) in mean.iter_mut().zip(*r) { *m += v / n; } }
        let mut var = vec![0f32; d];
        for r in x { for k in 0..d { var[k] += (r[k] - mean[k]).powi(2) / n; } }
        Standardizer { mean, inv_sd: var.iter().map(|v| 1.0 / v.sqrt().max(1e-3)).collect() }
    }
    fn apply(&self, v: &[f32]) -> Vec<f32> { v.iter().zip(&self.mean).zip(&self.inv_sd).map(|((x, m), s)| (x - m) * s).collect() }
}

/// Orthonormal columns by Gram-Schmidt (rows here: a Vec of k vectors of length d).
fn orthonormalise(q: &mut [Vec<f32>]) {
    for i in 0..q.len() {
        for j in 0..i {
            let dot: f32 = q[i].iter().zip(&q[j]).map(|(a, b)| a * b).sum();
            let qj = q[j].clone();
            for (a, b) in q[i].iter_mut().zip(&qj) { *a -= dot * b; }
        }
        let n = q[i].iter().map(|a| a * a).sum::<f32>().sqrt().max(1e-12);
        for a in q[i].iter_mut() { *a /= n; }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Pca { comps: Vec<Vec<f32>> }

impl Pca {
    /// Subspace (power) iteration on the covariance of the standardised rows: k directions in a few passes over the data.
    fn fit(z: &[Vec<f32>], k: usize, iters: usize, rng: &mut Rng) -> Pca {
        let (n, d) = (z.len(), z[0].len());
        let k = k.min(d).min(n.saturating_sub(1)).max(1);
        let mut q: Vec<Vec<f32>> = (0..k).map(|_| (0..d).map(|_| rng.normal() as f32).collect()).collect();
        orthonormalise(&mut q);
        for _ in 0..iters {
            // y = X^T (X q_i) for each component
            let mut next: Vec<Vec<f32>> = Vec::with_capacity(k);
            for qi in &q {
                let proj: Vec<f32> = z.iter().map(|r| r.iter().zip(qi).map(|(a, b)| a * b).sum()).collect();
                let mut y = vec![0f32; d];
                for (r, p) in z.iter().zip(&proj) { for (yv, rv) in y.iter_mut().zip(r) { *yv += rv * p; } }
                next.push(y);
            }
            orthonormalise(&mut next);
            q = next;
        }
        Pca { comps: q }
    }
    fn project(&self, v: &[f32]) -> Vec<f32> { self.comps.iter().map(|c| c.iter().zip(v).map(|(a, b)| a * b).sum()).collect() }
}

#[derive(Debug, Clone, PartialEq)]
struct Mlp { w1: Vec<Vec<f32>>, b1: Vec<f32>, w2: Vec<[f32; 2]>, b2: [f32; 2] }

impl Mlp {
    fn forward(&self, x: &[f32]) -> (Vec<f32>, [f32; 2]) {
        let h: Vec<f32> = self.w1.iter().zip(&self.b1).map(|(w, b)| (w.iter().zip(x).map(|(a, c)| a * c).sum::<f32>() + b).tanh()).collect();
        let mut o = self.b2;
        for (hv, w) in h.iter().zip(&self.w2) { o[0] += hv * w[0]; o[1] += hv * w[1]; }
        (h, o)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub kind: Kind,
    std: Standardizer,
    pca: Pca,
    /// The spread of the strongest component: every projection is divided by this one number. NOT per component: whitening
    /// gave the dozens of pure-noise directions the same weight as the one that carries the signal, and a network trained on
    /// that overfits the noise (it did worse than predicting the centre on a signal it should have found).
    zsd: Vec<f32>,
    centre: [f32; 2],
    /// Ridge weights: k rows then the bias row, two outputs.
    ridge: Vec<[f32; 2]>,
    mlp: Option<Mlp>,
}

fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<[f64; 2]>) -> Option<Vec<[f64; 2]>> {
    let n = a.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-12 { return None; }
        a.swap(col, piv);
        b.swap(col, piv);
        for r in 0..n {
            if r != col {
                let f = a[r][col] / a[col][col];
                for k in col..n { a[r][k] -= f * a[col][k]; }
                for o in 0..2 { b[r][o] -= f * b[col][o]; }
            }
        }
    }
    Some((0..n).map(|i| [b[i][0] / a[i][i], b[i][1] / a[i][i]]).collect())
}

impl Model {
    pub fn fit(samples: &[Sample], cfg: &Config) -> Option<Model> {
        if samples.len() < 4 { return None; }
        let rows: Vec<&[f32]> = samples.iter().map(|s| s.f.as_slice()).collect();
        let mut rng = Rng::new(cfg.seed);
        let std = Standardizer::fit(&rows);
        let z: Vec<Vec<f32>> = rows.iter().map(|r| std.apply(r)).collect();
        let n = samples.len() as f32;
        let centre = [samples.iter().map(|s| s.target[0]).sum::<f32>() / n, samples.iter().map(|s| s.target[1]).sum::<f32>() / n];
        let pca = Pca::fit(&z, cfg.pca_k, 6, &mut rng);
        let p: Vec<Vec<f32>> = z.iter().map(|r| pca.project(r)).collect();
        let k = pca.comps.len();
        let lead = (0..k).map(|i| (p.iter().map(|r| r[i] * r[i]).sum::<f32>() / n).sqrt()).fold(0.0f32, f32::max).max(1e-6);
        let zsd: Vec<f32> = vec![lead; k];
        let w: Vec<Vec<f32>> = p.iter().map(|r| r.iter().zip(&zsd).map(|(a, s)| a / s).collect()).collect();
        let mut m = Model { kind: cfg.kind, std, pca, zsd, centre, ridge: vec![[0.0; 2]; k + 1], mlp: None };
        if cfg.kind == Kind::Mean { return Some(m); }
        // ridge: (A^T A + lambda I) W = A^T Y, A = [w, 1], the bias row unpenalised
        let mut ata = vec![vec![0f64; k + 1]; k + 1];
        let mut aty = vec![[0f64; 2]; k + 1];
        for (r, s) in w.iter().zip(samples) {
            for i in 0..=k {
                let ai = if i < k { r[i] as f64 } else { 1.0 };
                for j in 0..=k { ata[i][j] += ai * if j < k { r[j] as f64 } else { 1.0 }; }
                aty[i][0] += ai * s.target[0] as f64;
                aty[i][1] += ai * s.target[1] as f64;
            }
        }
        for i in 0..k { ata[i][i] += cfg.lambda as f64 * n as f64 / 100.0; }
        m.ridge = solve(ata, aty)?.into_iter().map(|v| [v[0] as f32, v[1] as f32]).collect();
        if cfg.kind == Kind::Ridge { return Some(m); }
        // the network, on the whitened projections, starting from the ridge solution's neighbourhood
        let h = cfg.hidden.max(2);
        let sc = (1.0 / k as f32).sqrt();
        let mut net = Mlp {
            w1: (0..h).map(|_| (0..k).map(|_| rng.normal() as f32 * sc).collect()).collect(),
            b1: vec![0.0; h],
            w2: (0..h).map(|_| [rng.normal() as f32 * 0.1, rng.normal() as f32 * 0.1]).collect(),
            b2: centre,
        };
        // Adam state, flat: w1 (h*k), b1 (h), w2 (h*2), b2 (2)
        let np = h * k + h + h * 2 + 2;
        let (mut mom, mut vel) = (vec![0f32; np], vec![0f32; np]);
        let (b1c, b2c, eps, decay) = (0.9f32, 0.999f32, 1e-8f32, 1e-4f32);
        let mut order: Vec<usize> = (0..samples.len()).collect();
        let batch = 32usize;
        let mut step = 0i32;
        for _ in 0..cfg.epochs {
            rng.shuffle(&mut order);
            for chunk in order.chunks(batch) {
                let mut grad = vec![0f32; np];
                for &i in chunk {
                    let (hid, out) = net.forward(&w[i]);
                    let err = [out[0] - samples[i].target[0], out[1] - samples[i].target[1]];
                    let mut dh = vec![0f32; h];
                    for j in 0..h {
                        grad[h * k + h + j * 2] += hid[j] * err[0];
                        grad[h * k + h + j * 2 + 1] += hid[j] * err[1];
                        dh[j] = (err[0] * net.w2[j][0] + err[1] * net.w2[j][1]) * (1.0 - hid[j] * hid[j]);
                    }
                    grad[np - 2] += err[0];
                    grad[np - 1] += err[1];
                    for j in 0..h {
                        for (c, xv) in w[i].iter().enumerate() { grad[j * k + c] += dh[j] * xv; }
                        grad[h * k + j] += dh[j];
                    }
                }
                step += 1;
                let bs = chunk.len() as f32;
                let (c1, c2) = (1.0 - b1c.powi(step), 1.0 - b2c.powi(step));
                let mut params: Vec<&mut f32> = Vec::with_capacity(np);
                for row in net.w1.iter_mut() { for v in row.iter_mut() { params.push(v); } }
                for v in net.b1.iter_mut() { params.push(v); }
                for row in net.w2.iter_mut() { for v in row.iter_mut() { params.push(v); } }
                for v in net.b2.iter_mut() { params.push(v); }
                for (idx, prm) in params.into_iter().enumerate() {
                    let g = grad[idx] / bs + decay * *prm;
                    mom[idx] = b1c * mom[idx] + (1.0 - b1c) * g;
                    vel[idx] = b2c * vel[idx] + (1.0 - b2c) * g * g;
                    *prm -= cfg.lr * (mom[idx] / c1) / ((vel[idx] / c2).sqrt() + eps);
                }
            }
        }
        m.mlp = Some(net);
        Some(m)
    }

    /// The screen point (fractions, clamped to the screen) for a feature vector. A vector of the wrong length is None.
    pub fn predict(&self, f: &[f32]) -> Option<[f32; 2]> {
        if f.len() != self.std.mean.len() { return None; }
        let w: Vec<f32> = self.pca.project(&self.std.apply(f)).iter().zip(&self.zsd).map(|(a, s)| a / s).collect();
        let raw = match (self.kind, &self.mlp) {
            (Kind::Mean, _) => self.centre,
            (Kind::Mlp, Some(net)) => net.forward(&w).1,
            _ => {
                let k = w.len();
                let mut o = self.ridge[k];
                for i in 0..k { o[0] += w[i] * self.ridge[i][0]; o[1] += w[i] * self.ridge[i][1]; }
                o
            }
        };
        Some([raw[0].clamp(0.0, 1.0), raw[1].clamp(0.0, 1.0)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples whose features are a hidden low-dimensional signal spread across `dim` noisy pixels: x and y are what to learn.
    fn world(n: usize, dim: usize, seed: u64, nonlinear: bool) -> Vec<Sample> {
        // ONE mixing of the hidden signal into pixels for every call (a fixed seed), so a training set and a test set are the
        // same world; only the draws differ. (The first version re-drew the mixing per call, which made the test set a
        // different world, and the network test passed hollowly.)
        let mut mixer = Rng::new(99);
        let mix: Vec<[f32; 2]> = (0..dim).map(|_| [mixer.normal() as f32, mixer.normal() as f32]).collect();
        let mut rng = Rng::new(seed);
        (0..n).map(|i| {
            let (x, y) = (rng.unit() as f32, rng.unit() as f32);
            // nonlinear: the screen point reaches the pixels through a saturating, coupled map (as an eye turning does: the
            // iris moves less per degree toward the corners). Locally invertible, so it IS learnable, but a straight line misses it.
            let (a, b) = if nonlinear { ((2.5 * (x - 0.5) + 0.8 * (y - 0.5)).sin() * 0.6, (2.5 * (y - 0.5) - 0.8 * (x - 0.5)).sin() * 0.6) } else { (x - 0.5, y - 0.5) };
            let f: Vec<f32> = mix.iter().map(|m| m[0] * a * 3.0 + m[1] * b * 3.0 + rng.normal() as f32 * 0.3).collect();
            Sample { f, target: [x, y], dot: (i % 17) as u32, session: 1 }
        }).collect()
    }

    fn rmse(m: &Model, test: &[Sample]) -> f32 {
        let s: f32 = test.iter().map(|t| { let p = m.predict(&t.f).unwrap(); (p[0] - t.target[0]).powi(2) + (p[1] - t.target[1]).powi(2) }).sum();
        (s / test.len() as f32).sqrt()
    }

    #[test]
    fn ridge_recovers_a_linear_signal_buried_in_pixels_and_the_mean_does_not() {
        let (train, test) = (world(300, 80, 1, false), world(100, 80, 2, false));
        let mean = Model::fit(&train, &Config { kind: Kind::Mean, ..Config::default() }).unwrap();
        let ridge = Model::fit(&train, &Config { kind: Kind::Ridge, ..Config::default() }).unwrap();
        let (m, r) = (rmse(&mean, &test), rmse(&ridge, &test));
        assert!(m > 0.35, "predicting the centre is bad by construction: {m}");
        assert!(r < 0.12, "ridge should find it: {r}");
    }

    /// A world a straight line cannot read: the pixels carry a signed quantity `a`, the screen point is x = |a|, y = a^2.
    /// Symmetric in `a`, so no linear function of the features correlates with either target.
    fn folded(n: usize, seed: u64) -> Vec<Sample> {
        let mut mixer = Rng::new(99);
        let mix: Vec<[f32; 2]> = (0..40).map(|_| [mixer.normal() as f32, mixer.normal() as f32]).collect();
        let mut rng = Rng::new(seed);
        (0..n).map(|i| {
            let a = rng.unit() as f32 * 2.0 - 1.0;
            Sample { f: mix.iter().map(|m| m[0] * a * 3.0 + m[1] * 0.0 + rng.normal() as f32 * 0.2).collect(), target: [a.abs(), a * a], dot: i as u32, session: 1 }
        }).collect()
    }

    #[test]
    fn the_network_learns_what_a_straight_line_provably_cannot() {
        let (train, test) = (folded(500, 3), folded(150, 4));
        let mean = rmse(&Model::fit(&train, &Config { kind: Kind::Mean, ..Config::default() }).unwrap(), &test);
        let ridge = rmse(&Model::fit(&train, &Config { kind: Kind::Ridge, ..Config::default() }).unwrap(), &test);
        let mlp = rmse(&Model::fit(&train, &Config { kind: Kind::Mlp, epochs: 200, lr: 0.003, hidden: 32, ..Config::default() }).unwrap(), &test);
        assert!(ridge > mean * 0.9, "ridge cannot read |a|: ridge {ridge}, mean {mean}");
        assert!(mlp < mean * 0.5, "the network can: mlp {mlp}, mean {mean}");
    }

    #[test]
    fn training_is_deterministic_and_predictions_stay_on_the_screen() {
        let train = world(120, 30, 5, false);
        let cfg = Config { epochs: 20, ..Config::default() };
        let (a, b) = (Model::fit(&train, &cfg).unwrap(), Model::fit(&train, &cfg).unwrap());
        assert_eq!(a, b, "the same seed gives the same model");
        let c = Model::fit(&train, &Config { seed: 2, ..cfg.clone() }).unwrap();
        assert_ne!(a, c, "a different seed gives a different one");
        let wild = vec![1.0e6f32; 30];
        let p = a.predict(&wild).unwrap();
        assert!((0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1]), "{p:?}");
    }

    #[test]
    fn too_little_data_and_the_wrong_feature_length_are_none_not_a_guess() {
        assert!(Model::fit(&world(3, 10, 6, false), &Config::default()).is_none());
        let m = Model::fit(&world(60, 10, 7, false), &Config { epochs: 5, ..Config::default() }).unwrap();
        assert!(m.predict(&[0.0; 9]).is_none());
        assert!(m.predict(&[0.0; 10]).is_some());
    }
}


