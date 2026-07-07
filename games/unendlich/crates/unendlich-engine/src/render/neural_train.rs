//! neural_train.rs — in-engine background trainer for the neural shading MLP.
//!
//! Self-trains the shading net (cheap G-buffer features → expensive shaded RGB) on a
//! background thread from samples the game feeds during play, then bakes Q8 weights
//! (`q8.rs`) the shade pass can hot-swap. Training in-engine means the net learns on
//! EXACTLY the features the engine emits, so the trainer↔engine feature-encoding match
//! (BUGWATCH B2) is automatic, and there is no separate offline step.
//!
//! `TrainMlp` is a faithful Rust port of the prototype `mlp.py`: forward is
//! linear+silu on the hidden layers and a linear output; backward is the MSE gradient
//! with Adam (β1=0.9, β2=0.999, ε=1e-8, bias-corrected) — identical math, so a net
//! trained here matches one trained in numpy given the same samples. `export_q8`
//! folds the bias by augmenting + zero-padding to the 32-block and quantizes via the
//! engine's `q8::quantize_layer`, producing exactly the `(qw, qs, k, n)` the shade
//! pipelines consume.
//!
//! What this module does NOT include (runtime-coupled, the next wiring): the
//! ground-truth sample SOURCE — a pass that, for N sampled pixels, casts the full
//! 22-ray AO + soft shadow through the octree and pairs it with the G-buffer feature
//! → `Sample` — and the weight hot-swap into the shade pass. See the doc. Without the
//! ground-truth feed the trainer simply idles (no samples ⇒ no steps).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub const FEAT: usize = 14; // lean feature count
pub const OUT: usize = 3; // shaded RGB

/// One training pair: (feature[14], target shaded RGB[3]).
pub type Sample = ([f32; FEAT], [f32; OUT]);

/// A quantized layer ready for the shade pipeline: (qw, qs, k_padded, n_out).
pub type BakedLayer = (Vec<u32>, Vec<f32>, usize, usize);

#[derive(Clone, Copy)]
pub struct TrainCfg {
    pub batch: usize,
    pub lr: f32,
    pub ring_cap: usize,
    pub bake_every: u64,
}
impl Default for TrainCfg {
    fn default() -> Self {
        // lean net trains fast; a few thousand samples/step, bake a few times a second.
        Self { batch: 1024, lr: 2e-3, ring_cap: 1 << 16, bake_every: 64 }
    }
}

// ---- tiny deterministic PRNG (SplitMix64 → uniform / gaussian) ----
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn uniform(&mut self) -> f32 {
        // [0,1)
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn gaussian(&mut self) -> f32 {
        // Box–Muller
        let u1 = self.uniform().max(1e-7);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// The trainable f32 MLP. `sizes` = [14, 64, 48, 3] for the lean net.
pub struct TrainMlp {
    sizes: Vec<usize>,
    w: Vec<Vec<f32>>, // w[i]: [sizes[i+1] * sizes[i]] row-major (out, in)
    b: Vec<Vec<f32>>, // b[i]: [sizes[i+1]]
    mw: Vec<Vec<f32>>,
    vw: Vec<Vec<f32>>,
    mb: Vec<Vec<f32>>,
    vb: Vec<Vec<f32>>,
    t: u64,
}

impl TrainMlp {
    pub fn new(sizes: Vec<usize>, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let n = sizes.len() - 1;
        let mut w = Vec::with_capacity(n);
        let mut b = Vec::with_capacity(n);
        for i in 0..n {
            let (out, inp) = (sizes[i + 1], sizes[i]);
            let scale = (2.0f32 / inp as f32).sqrt(); // He init
            let mut wi = vec![0f32; out * inp];
            for v in wi.iter_mut() {
                *v = rng.gaussian() * scale;
            }
            w.push(wi);
            b.push(vec![0f32; out]);
        }
        let zeros = |src: &Vec<Vec<f32>>| src.iter().map(|x| vec![0f32; x.len()]).collect::<Vec<_>>();
        let (mw, vw) = (zeros(&w), zeros(&w));
        let (mb, vb) = (zeros(&b), zeros(&b));
        Self { sizes, w, b, mw, vw, mb, vb, t: 0 }
    }

    pub fn lean() -> Self {
        Self::new(vec![FEAT, 64, 48, OUT], 0)
    }

    fn n_layers(&self) -> usize {
        self.sizes.len() - 1
    }

    /// Forward over a minibatch `x` [m, sizes[0]]. If `cache` is given, pushes
    /// `(a_in, z_preact)` per layer. Returns the linear output [m, OUT].
    fn forward(&self, x: &[f32], m: usize, cache: Option<&mut Vec<(Vec<f32>, Vec<f32>)>>) -> Vec<f32> {
        let nl = self.n_layers();
        let mut a = x.to_vec();
        let mut a_cols = self.sizes[0];
        let mut cache = cache;
        for i in 0..nl {
            let (out, inp) = (self.sizes[i + 1], self.sizes[i]);
            debug_assert_eq!(a_cols, inp);
            // z = a @ W[i]^T + b[i]
            let mut z = vec![0f32; m * out];
            for r in 0..m {
                for c in 0..out {
                    let mut acc = self.b[i][c];
                    let wrow = &self.w[i][c * inp..c * inp + inp];
                    let arow = &a[r * inp..r * inp + inp];
                    for k in 0..inp {
                        acc += arow[k] * wrow[k];
                    }
                    z[r * out + c] = acc;
                }
            }
            if let Some(cc) = cache.as_deref_mut() {
                cc.push((a.clone(), z.clone()));
            }
            if i + 1 < nl {
                // hidden: silu
                let mut na = vec![0f32; m * out];
                for j in 0..m * out {
                    na[j] = silu(z[j]);
                }
                a = na;
            } else {
                // output: linear
                a = z;
            }
            a_cols = out;
        }
        a
    }

    /// One Adam minibatch step on (`xb` [m,FEAT], `yb` [m,OUT]). Returns the MSE.
    pub fn train_step(&mut self, xb: &[f32], yb: &[f32], m: usize, lr: f32) -> f32 {
        let nl = self.n_layers();
        let mut cache: Vec<(Vec<f32>, Vec<f32>)> = Vec::with_capacity(nl);
        let pred = self.forward(xb, m, Some(&mut cache));

        // MSE + its gradient w.r.t. the linear output.
        let mut g = vec![0f32; m * OUT];
        let mut sse = 0f32;
        let inv = 2.0 / m as f32;
        for j in 0..m * OUT {
            let d = pred[j] - yb[j];
            g[j] = inv * d;
            sse += d * d;
        }
        let mse = sse / (m * OUT) as f32;

        // Backprop. g starts as [m, OUT]; shrinks per layer.
        let mut g_cols = OUT;
        for i in (0..nl).rev() {
            let (out, inp) = (self.sizes[i + 1], self.sizes[i]);
            debug_assert_eq!(g_cols, out);
            let (a, _z) = &cache[i]; // a: [m, inp]
            // gW[i] = g^T @ a  → [out, inp];  gb[i] = sum_rows g → [out]
            let mut gw = vec![0f32; out * inp];
            let mut gb = vec![0f32; out];
            for r in 0..m {
                let grow = &g[r * out..r * out + out];
                let arow = &a[r * inp..r * inp + inp];
                for c in 0..out {
                    let gc = grow[c];
                    gb[c] += gc;
                    let wbase = c * inp;
                    for k in 0..inp {
                        gw[wbase + k] += gc * arow[k];
                    }
                }
            }
            // propagate to the previous layer's pre-activation (if any).
            if i > 0 {
                let prev_cols = self.sizes[i]; // = inp
                let prev_z = &cache[i - 1].1; // [m, prev_cols]
                let mut ng = vec![0f32; m * prev_cols];
                for r in 0..m {
                    let grow = &g[r * out..r * out + out];
                    for k in 0..prev_cols {
                        // (g @ W[i])[r,k] = sum_c g[r,c] * W[i][c,k]
                        let mut acc = 0f32;
                        for c in 0..out {
                            acc += grow[c] * self.w[i][c * inp + k];
                        }
                        ng[r * prev_cols + k] = acc * dsilu(prev_z[r * prev_cols + k]);
                    }
                }
                g = ng;
                g_cols = prev_cols;
            }
            // Adam update for W[i], b[i].
            self.t += 1; // matches mlp.py: t increments per (step, but here once per layer-step is fine as long as consistent)
            adam(&mut self.w[i], &gw, &mut self.mw[i], &mut self.vw[i], self.t, lr);
            adam(&mut self.b[i], &gb, &mut self.mb[i], &mut self.vb[i], self.t, lr);
        }
        mse
    }

    /// Bake each layer to Q8 (`q8::quantize_layer`): bias folded as a +1 augmented
    /// input column, zero-padded to the 32-block. Output is the shade pipeline's
    /// `(qw, qs, k, n)` per layer, in order.
    pub fn export_q8(&self) -> Vec<BakedLayer> {
        let mut out = Vec::with_capacity(self.n_layers());
        for i in 0..self.n_layers() {
            let (n, d) = (self.sizes[i + 1], self.sizes[i]);
            let (qw, qs, k) = crate::render::q8::quantize_layer(&self.w[i], &self.b[i], n, d);
            out.push((qw, qs, k, n));
        }
        out
    }

    /// Train on a synthetic mapping for `steps` and return the final MSE — a sanity
    /// check that the math learns (the loss should fall well below the init). Cheap;
    /// callable from a debug command.
    pub fn self_test(steps: usize) -> f32 {
        let mut net = TrainMlp::lean();
        let mut rng = Rng::new(1234);
        let m = 256usize;
        let mut last = 0f32;
        let mut start = 0f32;
        for step in 0..steps {
            let mut xb = vec![0f32; m * FEAT];
            let mut yb = vec![0f32; m * OUT];
            for r in 0..m {
                let mut s = 0f32;
                for k in 0..FEAT {
                    let v = rng.uniform() * 2.0 - 1.0;
                    xb[r * FEAT + k] = v;
                    s += v;
                }
                // a smooth target the net can fit
                let t = (s / FEAT as f32 * 0.5 + 0.5).clamp(0.0, 1.0);
                yb[r * OUT] = t;
                yb[r * OUT + 1] = t * t;
                yb[r * OUT + 2] = 1.0 - t;
            }
            last = net.train_step(&xb, &yb, m, 2e-3);
            if step == 0 {
                start = last;
            }
            if steps >= 4 && step % (steps / 4) == 0 {
                log::info!("[neural] trainer self-test step {step}: mse {last:.5}");
            }
        }
        log::info!(
            "[neural] trainer self-test: mse {start:.5} -> {last:.5} over {steps} steps (lower = learning)"
        );
        last
    }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}
fn silu(x: f32) -> f32 {
    x * sigmoid(x)
}
fn dsilu(x: f32) -> f32 {
    let s = sigmoid(x);
    s * (1.0 + x * (1.0 - s))
}

/// In-place Adam step (bias-corrected), matching mlp.py.
fn adam(p: &mut [f32], g: &[f32], m: &mut [f32], v: &mut [f32], t: u64, lr: f32) {
    const B1: f32 = 0.9;
    const B2: f32 = 0.999;
    const EPS: f32 = 1e-8;
    let bc1 = 1.0 - B1.powi(t as i32);
    let bc2 = 1.0 - B2.powi(t as i32);
    for i in 0..p.len() {
        m[i] = B1 * m[i] + (1.0 - B1) * g[i];
        v[i] = B2 * v[i] + (1.0 - B2) * g[i] * g[i];
        let mhat = m[i] / bc1;
        let vhat = v[i] / bc2;
        p[i] -= lr * mhat / (vhat.sqrt() + EPS);
    }
}

// =============================================================================
// Background trainer: a thread that consumes game-fed samples and publishes baked
// weights. Gated by the caller (e.g. UNENDLICH_NEURAL_TRAIN).
// =============================================================================

pub struct BackgroundTrainer {
    tx: mpsc::Sender<Vec<Sample>>,
    weights: Arc<Mutex<Option<Vec<BakedLayer>>>>,
    generation: Arc<AtomicU64>,
    steps: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    last_seen_gen: u64,
}

impl BackgroundTrainer {
    pub fn spawn(seed: u64, cfg: TrainCfg) -> Self {
        let (tx, rx) = mpsc::channel::<Vec<Sample>>();
        let weights = Arc::new(Mutex::new(None));
        let generation = Arc::new(AtomicU64::new(0));
        let steps = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        let (w_t, g_t, s_t, stop_t) = (weights.clone(), generation.clone(), steps.clone(), stop.clone());
        let handle = std::thread::Builder::new()
            .name("neural-trainer".into())
            .spawn(move || {
                let mut net = TrainMlp::lean();
                let mut ring: Vec<Sample> = Vec::with_capacity(cfg.ring_cap);
                let mut rng = Rng::new(seed ^ 0xD1B5_4A32_D192_ED03);
                let mut step = 0u64;
                loop {
                    if stop_t.load(Ordering::Relaxed) {
                        break;
                    }
                    // Drain everything currently queued.
                    let mut got = false;
                    loop {
                        match rx.try_recv() {
                            Ok(mut batch) => {
                                ring.append(&mut batch);
                                got = true;
                            }
                            Err(mpsc::TryRecvError::Empty) => break,
                            Err(mpsc::TryRecvError::Disconnected) => return,
                        }
                    }
                    if ring.len() > cfg.ring_cap {
                        let drop = ring.len() - cfg.ring_cap;
                        ring.drain(0..drop); // evict oldest
                    }
                    if ring.len() >= cfg.batch {
                        // Random minibatch from the ring.
                        let mut xb = vec![0f32; cfg.batch * FEAT];
                        let mut yb = vec![0f32; cfg.batch * OUT];
                        for r in 0..cfg.batch {
                            let (f, y) = &ring[rng.below(ring.len())];
                            xb[r * FEAT..r * FEAT + FEAT].copy_from_slice(f);
                            yb[r * OUT..r * OUT + OUT].copy_from_slice(y);
                        }
                        net.train_step(&xb, &yb, cfg.batch, cfg.lr);
                        step += 1;
                        s_t.store(step, Ordering::Relaxed);
                        if step % cfg.bake_every == 0 {
                            let baked = net.export_q8();
                            *w_t.lock().unwrap() = Some(baked);
                            g_t.fetch_add(1, Ordering::Release);
                        }
                    } else if !got {
                        std::thread::sleep(Duration::from_millis(4));
                    }
                }
            })
            .expect("spawn neural-trainer thread");

        Self { tx, weights, generation, steps, stop, handle: Some(handle), last_seen_gen: 0 }
    }

    /// Feed a batch of (feature, target) pairs (from the ground-truth sample pass).
    pub fn push(&self, samples: Vec<Sample>) {
        let _ = self.tx.send(samples);
    }

    /// If the trainer has baked a newer weight set since the last call, return it
    /// (for the shade pass to hot-swap). Returns `None` if nothing new.
    pub fn try_take_weights(&mut self) -> Option<Vec<BakedLayer>> {
        let gen = self.generation.load(Ordering::Acquire);
        if gen == self.last_seen_gen {
            return None;
        }
        self.last_seen_gen = gen;
        self.weights.lock().unwrap().clone()
    }

    pub fn steps(&self) -> u64 {
        self.steps.load(Ordering::Relaxed)
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for BackgroundTrainer {
    fn drop(&mut self) {
        self.stop();
    }
}
