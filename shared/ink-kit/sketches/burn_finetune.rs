//! ════════════════════════════════════════════════════════════════════════════
//!  ILLUSTRATIVE SKETCH — NOT COMPILED here (needs burn 0.21 + wgpu, i.e. GPU
//!  hardware). Shape verified only against burn 0.21 docs; API names below are
//!  the ones to re-check when this is wired on hardware.
//! ════════════════════════════════════════════════════════════════════════════
//!
//! Handwriting finetuning over an ink-kit [`Corpus`] — the "rust toolset for
//! finetuning models" piece, aligned with what the tree already uses:
//! logscope/burn-probe pins **burn 0.21 (wgpu backend)**, so this stays on the
//! same generation rather than introducing candle as a second ML stack.
//!
//! Two model tiers, both fed by the same corpus (vector-first pays off here):
//!
//! 1. **Image tier** (this sketch): rasterized PGM → small CNN classifier
//!    (chars / symbols / shape classes). This is the bot-arena
//!    "handwriting → image or text" challenge target: cheap to train, easy to
//!    score, runs on the FP5's GPU via wgpu.
//! 2. **Sequence tier** (next): raw stroke points → 1D conv / GRU CTC over
//!    point deltas (dx, dy, pressure, pen-up). The `.ink` format keeps order
//!    and timing precisely so this tier needs no new capture work.
//!
//! Data flow (all real, tested ink-kit API):
//! ```text
//! Corpus::open(root)             manifest.tsv + ink/*.ink
//!   .split(100)                  deterministic 10% val (FNV of id — stable)
//!   .load_strokes(id)            Vec<Stroke>
//! rasterize(&strokes, 64, …)     Gray8, MNIST-convention (ink=255 on 0)
//! ```
//!
//! burn 0.21 gotchas to re-check on hardware:
//! * `burn::backend::wgpu::Wgpu` device selection on Mesa/Panfrost (FP5) and
//!   Asahi: request the low-power adapter explicitly or the phone may pick a
//!   software fallback.
//! * `Learner` API churn: 0.21 renamed several builder methods from 0.13-era
//!   examples floating around; trust the 0.21 book, not blog posts.
//! * Autodiff backend wrapper is `burn::backend::Autodiff<Wgpu>`.

use burn::backend::{wgpu::Wgpu, Autodiff};
use burn::config::Config;
use burn::data::dataloader::batcher::Batcher;
use burn::module::Module;
use burn::nn::{conv::Conv2d, conv::Conv2dConfig, Linear, LinearConfig, Relu};
use burn::tensor::{backend::Backend, Tensor};

use ink_kit::dataset::{Corpus, SampleMeta};
use ink_kit::raster::rasterize;

const IMG: usize = 64;

/// One corpus sample turned into tensors.
#[derive(Clone)]
pub struct InkItem {
    pub pixels: Vec<f32>, // IMG*IMG, 0..1
    pub class: usize,
}

pub fn load_items(corpus: &Corpus, metas: &[SampleMeta], classes: &[String]) -> Vec<InkItem> {
    metas
        .iter()
        .filter_map(|m| {
            let strokes = corpus.load_strokes(&m.id).ok()?;
            let img = rasterize(&strokes, IMG, 0.08, IMG as f64 / 96.0);
            let class = classes.iter().position(|c| *c == m.label)?;
            Some(InkItem {
                pixels: img.data.iter().map(|&v| v as f32 / 255.0).collect(),
                class,
            })
        })
        .collect()
}

#[derive(Module, Debug)]
pub struct InkNet<B: Backend> {
    conv1: Conv2d<B>,
    conv2: Conv2d<B>,
    fc1: Linear<B>,
    fc2: Linear<B>,
    act: Relu,
}

#[derive(Config)]
pub struct InkNetConfig {
    pub num_classes: usize,
}

impl InkNetConfig {
    pub fn init<B: Backend>(&self, device: &B::Device) -> InkNet<B> {
        InkNet {
            conv1: Conv2dConfig::new([1, 16], [3, 3]).init(device),
            conv2: Conv2dConfig::new([16, 32], [3, 3]).init(device),
            // 64→(pool)…: exact flatten dim computed once on hardware; the
            // point of the sketch is the wiring, not this constant.
            fc1: LinearConfig::new(32 * 14 * 14, 128).init(device),
            fc2: LinearConfig::new(128, self.num_classes).init(device),
            act: Relu::new(),
        }
    }
}

impl<B: Backend> InkNet<B> {
    pub fn forward(&self, x: Tensor<B, 4>) -> Tensor<B, 2> {
        // conv → relu → maxpool ×2 → flatten → fc → logits (standard shape;
        // pooling via burn::nn::pool::MaxPool2d, elided for brevity)
        unimplemented!("filled in on hardware; see burn 0.21 mnist example")
    }
}

pub fn train(corpus_root: &std::path::Path) {
    type B = Autodiff<Wgpu>;
    let device = Default::default();

    let corpus = Corpus::open(corpus_root);
    let (train_meta, val_meta) = corpus.split(100).expect("manifest");
    // class vocabulary = distinct labels, sorted for determinism
    let mut classes: Vec<String> = train_meta.iter().map(|m| m.label.clone()).collect();
    classes.sort();
    classes.dedup();

    let _train = load_items(&corpus, &train_meta, &classes);
    let _val = load_items(&corpus, &val_meta, &classes);
    let _model = InkNetConfig { num_classes: classes.len() }.init::<B>(&device);

    // burn LearnerBuilder + Adam + CrossEntropyLoss; checkpoint into
    // models-kit's canonical home so every app can find it:
    let out = models_kit::resolve("ink", "inknet-v1.mpk");
    let _ = out; // learner.save_file(out, …)
}

// ── bot-arena challenge hook ────────────────────────────────────────────────
// A `challenges/handwriting/` entry scores a worker by:
//   1. corpus_status(root) must report 0 missing ink files (well-formed data),
//   2. train/val split accuracy of its trained checkpoint on the held-out val
//      set (deterministic split ⇒ comparable across the whole population),
//   3. optional: sequence-tier CER for handwriting→text once tier 2 exists.
// The fitness function is exactly `val accuracy` — no bespoke scoring code.
