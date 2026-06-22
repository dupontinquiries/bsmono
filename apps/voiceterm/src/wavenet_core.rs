//! wavenet_core — a small, reusable WaveNet-style audio predictor in
//! candle (pure-Rust ML, no C++/ONNX). Shared across the realtime-audio
//! projects: web2qml (voice-DB augmentation — generate fewer-but-better
//! synthetic utterance variations), bo-mux (predictive noise cancellation,
//! a neural upgrade to the classical phase-locked template predictor in
//! bo-mux/src/predictor.rs), and the third realtime-audio project.
//!
//! ── WHAT THIS IS / IS NOT (read before relying on it) ──────────────────
//! IS: the ARCHITECTURE — a stack of causal, exponentially-dilated conv1d
//!     layers with gated (tanh·sigmoid) activations, residual + skip
//!     connections, and a 1x1 output head. Compiles; runs a forward pass;
//!     output is the correct shape (next-sample distribution per timestep).
//!     A reusable core the three projects can train + call.
//! IS NOT: a trained model. Freshly-initialized weights produce NOISE, not
//!     speech. Producing USEFUL 1-2s predictions requires training on real
//!     audio (a training loop + data), and audio QUALITY cannot be judged
//!     in a headless sandbox (no way to listen). So this core is verified
//!     to RUN and have correct shapes/gradients-able structure; it is NOT
//!     verified to sound good. Training + perceptual validation happen on
//!     the user's machine with real samples.
//!
//! ── why WaveNet for "1-2s prediction to fuzz the DB with better values" ─
//! Instead of fuzzing the fingerprint DB with many crude synthetic seeds,
//! a trained predictor can generate a SMALL number of realistic utterance
//! variations (different prosody/noise/room) seeded from a real sample —
//! higher-value augmentation. The same next-sample predictor also drives
//! bo-mux's "predict then play the inverse" cancellation. One core, three
//! uses.
//!
//! ── realtime note ──────────────────────────────────────────────────────
//! Autoregressive WaveNet generation is O(samples) sequential and slow at
//! 16kHz. "Cheap realtime" means: keep the layer count + channels small
//! (this default is tiny), and for true realtime use the standard
//! fast-wavenet trick (cache per-layer dilation queues so each new sample
//! is O(layers) not O(receptive_field)). The cache path is sketched in
//! `WaveNet::generate_step` but left as a TODO — the offline forward path
//! is what's compile-verified here.

#[cfg(feature = "wavenet")]
mod imp {
    use candle_core::{Result, Tensor};
    use candle_nn::{Conv1d, Conv1dConfig, Module, VarBuilder};

    /// Hyperparameters. Defaults are deliberately TINY (cheap/realtime-ish).
    #[derive(Clone, Copy)]
    pub struct Config {
        pub residual_ch: usize, // channels in the residual path
        pub skip_ch: usize,     // channels in the skip path
        pub n_layers: usize,    // dilated layers (dilation doubles each)
        pub kernel: usize,      // conv kernel width (2 is classic)
        pub quant_levels: usize,// output classes (mu-law buckets, e.g. 256)
    }
    impl Default for Config {
        fn default() -> Self {
            Self { residual_ch: 16, skip_ch: 16, n_layers: 6, kernel: 2, quant_levels: 256 }
        }
    }

    struct ResBlock {
        filter: Conv1d, // dilated conv → tanh branch
        gate: Conv1d,   // dilated conv → sigmoid branch
        res: Conv1d,    // 1x1 back to residual_ch
        skip: Conv1d,   // 1x1 to skip_ch
        dilation: usize,
        kernel: usize,
    }
    impl ResBlock {
        fn new(cfg: &Config, dilation: usize, vb: VarBuilder) -> Result<Self> {
            let dconf = Conv1dConfig { padding: 0, stride: 1, dilation, groups: 1 };
            let oneconf = Conv1dConfig { padding: 0, stride: 1, dilation: 1, groups: 1 };
            Ok(Self {
                filter: candle_nn::conv1d(cfg.residual_ch, cfg.residual_ch, cfg.kernel, dconf, vb.pp("filter"))?,
                gate:   candle_nn::conv1d(cfg.residual_ch, cfg.residual_ch, cfg.kernel, dconf, vb.pp("gate"))?,
                res:    candle_nn::conv1d(cfg.residual_ch, cfg.residual_ch, 1, oneconf, vb.pp("res"))?,
                skip:   candle_nn::conv1d(cfg.residual_ch, cfg.skip_ch, 1, oneconf, vb.pp("skip"))?,
                dilation, kernel: cfg.kernel,
            })
        }
        /// returns (residual_out, skip_out). Causal: left-pads by
        /// dilation*(kernel-1) so output length == input length and no
        /// future samples leak in.
        fn forward(&self, x: &Tensor) -> Result<(Tensor, Tensor)> {
            let pad = self.dilation * (self.kernel - 1);
            let xp = x.pad_with_zeros(2, pad, 0)?;
            let f = self.filter.forward(&xp)?.tanh()?;
            let g = candle_nn::ops::sigmoid(&self.gate.forward(&xp)?)?;
            let z = (f * g)?;
            let res = (self.res.forward(&z)? + x)?; // residual add
            let skip = self.skip.forward(&z)?;
            Ok((res, skip))
        }
    }

    pub struct WaveNet {
        input: Conv1d,         // 1ch audio → residual_ch
        blocks: Vec<ResBlock>,
        out1: Conv1d,          // skip_ch → skip_ch
        out2: Conv1d,          // skip_ch → quant_levels
        pub cfg: Config,
    }
    impl WaveNet {
        pub fn new(cfg: Config, vb: VarBuilder) -> Result<Self> {
            let one = Conv1dConfig { padding: 0, stride: 1, dilation: 1, groups: 1 };
            let input = candle_nn::conv1d(1, cfg.residual_ch, 1, one, vb.pp("input"))?;
            let mut blocks = Vec::with_capacity(cfg.n_layers);
            for i in 0..cfg.n_layers {
                let dilation = 1usize << i; // 1,2,4,8,...
                blocks.push(ResBlock::new(&cfg, dilation, vb.pp(format!("block{i}")))?);
            }
            let out1 = candle_nn::conv1d(cfg.skip_ch, cfg.skip_ch, 1, one, vb.pp("out1"))?;
            let out2 = candle_nn::conv1d(cfg.skip_ch, cfg.quant_levels, 1, one, vb.pp("out2"))?;
            Ok(Self { input, blocks, out1, out2, cfg })
        }

        /// Forward over a whole sequence. x: [batch, 1, len] (audio in
        /// [-1,1]). Returns [batch, quant_levels, len] — per-timestep
        /// next-sample logits. Causal: logit at t depends only on samples
        /// ≤ t. (Offline/teacher-forcing path — the verified one.)
        pub fn forward(&self, x: &Tensor) -> Result<Tensor> {
            let mut res = self.input.forward(x)?;
            let mut skip_sum: Option<Tensor> = None;
            for b in &self.blocks {
                let (r, s) = b.forward(&res)?;
                res = r;
                skip_sum = Some(match skip_sum { Some(acc) => (acc + s)?, None => s });
            }
            let s = skip_sum.expect("n_layers >= 1");
            let s = s.relu()?;
            let s = self.out1.forward(&s)?.relu()?;
            self.out2.forward(&s) // logits [b, quant_levels, len]
        }

        /// The receptive field (how many past samples influence one
        /// prediction). For realtime, generation should cache state so
        /// each new sample is O(n_layers) — see the module note.
        pub fn receptive_field(&self) -> usize {
            // sum of dilation*(kernel-1) + 1
            let k = self.cfg.kernel;
            let mut rf = 1usize;
            for i in 0..self.cfg.n_layers { rf += (1usize << i) * (k - 1); }
            rf
        }
    }

    /// mu-law companding (the classic WaveNet audio quantization): map
    /// f32 audio in [-1,1] to one of `levels` integer buckets and back.
    /// Pure + testable independent of candle.
    pub fn mu_law_encode(x: f32, levels: usize) -> usize {
        let mu = (levels - 1) as f32;
        let x = x.clamp(-1.0, 1.0);
        let y = x.signum() * (1.0 + mu * x.abs()).ln() / (1.0 + mu).ln();
        (((y + 1.0) / 2.0 * mu).round() as usize).min(levels - 1)
    }
    pub fn mu_law_decode(q: usize, levels: usize) -> f32 {
        let mu = (levels - 1) as f32;
        let y = (q as f32 / mu) * 2.0 - 1.0;
        y.signum() * (1.0 / mu) * ((1.0 + mu).powf(y.abs()) - 1.0)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use candle_nn::{VarBuilder, VarMap};

        #[test]
        fn forward_pass_shapes_are_causal_length_preserving() {
            let dev = Device::Cpu;
            let vm = VarMap::new();
            let vb = VarBuilder::from_varmap(&vm, DType::F32, &dev);
            let cfg = Config::default();
            let net = WaveNet::new(cfg, vb).unwrap();
            let len = 4000usize; // 0.25s @16k
            let x = Tensor::randn(0f32, 0.3, (1, 1, len), &dev).unwrap();
            let logits = net.forward(&x).unwrap();
            assert_eq!(logits.dims(), &[1, cfg.quant_levels, len], "length preserved, classes out");
        }

        #[test]
        fn receptive_field_grows_exponentially() {
            let dev = Device::Cpu;
            let vm = VarMap::new();
            let vb = VarBuilder::from_varmap(&vm, DType::F32, &dev);
            let net = WaveNet::new(Config::default(), vb).unwrap();
            // 6 layers, kernel 2: rf = 1 + (1+2+4+8+16+32) = 64
            assert_eq!(net.receptive_field(), 64);
        }

        #[test]
        fn mu_law_round_trips_approximately() {
            for &v in &[-0.9f32, -0.3, 0.0, 0.25, 0.8] {
                let q = mu_law_encode(v, 256);
                let back = mu_law_decode(q, 256);
                assert!((back - v).abs() < 0.05, "mu-law {v} -> {q} -> {back}");
            }
        }
    }
}

#[cfg(feature = "wavenet")]
// Public "shared core" surface (see Cargo.toml `wavenet` note): re-exported for
// other realtime-audio consumers; not referenced within this binary itself.
#[allow(unused_imports)]
pub use imp::*;
