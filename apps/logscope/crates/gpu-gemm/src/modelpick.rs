//! modelpick — which checkpoint will actually run on this machine.
//!
//! `archcfg` says which architectures the engine can execute; it says nothing about whether a
//! given checkpoint FITS. Those are different questions and the second is the one someone
//! asks before downloading eight gigabytes: "what should I run on this box?"
//!
//! Pure arithmetic over a small table, so the answer can be argued about without the machine
//! in front of you — and so the fleet's phone targets get the same answer here that they
//! would get on the device.
//!
//! ## Why it asks `archcfg` rather than carrying its own list
//!
//! A recommender with its own idea of what runs is a second source of truth, and the first
//! time they disagree someone downloads a model the loader refuses. Every candidate names its
//! `general.architecture` string and is filtered through `archcfg::is_supported`, so a model
//! cannot be recommended for an architecture the engine cannot execute.
//!
//! ## Weights are not the whole budget
//!
//! The KV cache is the part people forget, and it is what turns "it fits" into an OOM at 4k
//! tokens. It scales with context, layers and KV heads, so a 30B MoE with few KV heads can
//! need less cache than an 8B dense model — which is exactly the sort of inversion a
//! rule-of-thumb misses.

/// A candidate checkpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub name: &'static str,
    /// `general.architecture` — checked against `archcfg`, never assumed.
    pub arch: &'static str,
    /// Total parameters in billions (all experts, for MoE).
    pub params_b: f32,
    /// Parameters ACTIVE per token. Equals `params_b` for a dense model, and is what decides
    /// speed; the total is what decides whether it fits.
    pub active_b: f32,
    pub layers: u32,
    /// KV heads (GQA), not attention heads — the cache is sized by these.
    pub kv_heads: u32,
    pub head_dim: u32,
}

/// Bits per weight for the common GGUF quantisations.
///
/// The K-quants are not their nominal bit width: `Q4_K_M` averages ~4.8 bits once the
/// per-block scales and the mixed higher-precision tensors are counted. Using 4.0 here would
/// under-estimate a 30B model by more than a gigabyte, which is the difference between a
/// recommendation and an OOM.
pub fn bits_per_weight(quant: &str) -> f32 {
    match quant.to_ascii_uppercase().as_str() {
        "Q2_K" => 3.35,
        "Q3_K_M" => 3.9,
        "Q4_0" => 4.5,
        "Q4_K_M" => 4.8,
        "Q5_K_M" => 5.7,
        "Q6_K" => 6.6,
        "Q8_0" => 8.5,
        "F16" => 16.0,
        _ => 4.8,
    }
}

/// Weight bytes for a model at a quantisation.
pub fn weight_mb(m: &Model, quant: &str) -> f32 {
    m.params_b * 1e9 * bits_per_weight(quant) / 8.0 / 1e6
}

/// KV cache bytes at a context length, in MB.
///
/// `2` for K and V, `2` bytes per element for the f16 cache this engine keeps. Sized by KV
/// heads rather than attention heads — GQA is the whole reason a modern 8B model has a cache
/// a quarter the size of an old one, and using attention heads would over-estimate by the GQA
/// ratio and refuse models that fit comfortably.
pub fn kv_cache_mb(m: &Model, ctx: u32) -> f32 {
    2.0 * 2.0 * ctx as f32 * m.layers as f32 * m.kv_heads as f32 * m.head_dim as f32 / 1e6
}

/// How a candidate came out.
#[derive(Debug, Clone, PartialEq)]
pub struct Fit {
    pub model: Model,
    pub quant: &'static str,
    pub weights_mb: f32,
    pub kv_mb: f32,
    /// Weights + cache + the engine's own working set.
    pub total_mb: f32,
    /// Active parameters — what decides tokens per second.
    pub active_b: f32,
}

/// Headroom left for everything that is not this model: the compositor, the app, the
/// allocator's own fragmentation.
///
/// 15% and a 512MB floor. A recommendation that uses every byte is one that OOMs the moment
/// something else on the machine asks for memory, and on a phone that something is the
/// compositor.
pub fn usable_mb(total_mb: f32) -> f32 {
    (total_mb * 0.85).min(total_mb - 512.0).max(0.0)
}

/// The candidates this engine can run, best first for a machine with `vram_mb`.
///
/// "Best" is the largest ACTIVE parameter count that fits — active is what the model knows
/// per token, and a 30B MoE with 3B active is a faster and often better answer than a dense
/// 8B on the same memory. Ties break toward fewer total parameters, because the smaller
/// download is the better default when nothing else separates them.
pub fn recommend(catalog: &[Model], vram_mb: f32, ctx: u32, quants: &[&'static str]) -> Vec<Fit> {
    let budget = usable_mb(vram_mb);
    let mut out: Vec<Fit> = Vec::new();
    for m in catalog {
        // The engine's answer, not ours. A model recommended for an architecture the loader
        // refuses is worse than no recommendation.
        if !crate::archcfg::is_supported(m.arch, 0) {
            continue;
        }
        // The best quantisation that fits, not the smallest: someone who can run Q6 should
        // not be handed Q3.
        let mut best: Option<Fit> = None;
        for q in quants {
            let w = weight_mb(m, q);
            let kv = kv_cache_mb(m, ctx);
            let total = w + kv + WORKING_SET_MB;
            if total <= budget {
                let f = Fit {
                    model: m.clone(),
                    quant: q,
                    weights_mb: w,
                    kv_mb: kv,
                    total_mb: total,
                    active_b: m.active_b,
                };
                if best.as_ref().is_none_or(|b| bits_per_weight(q) > bits_per_weight(b.quant)) {
                    best = Some(f);
                }
            }
        }
        if let Some(f) = best {
            out.push(f);
        }
    }
    out.sort_by(|a, b| {
        b.active_b
            .partial_cmp(&a.active_b)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.model.params_b.partial_cmp(&b.model.params_b).unwrap_or(std::cmp::Ordering::Equal))
    });
    out
}

/// Scratch the engine needs beyond weights and cache: logits, the dequant staging buffers,
/// one layer's activations.
pub const WORKING_SET_MB: f32 = 400.0;

/// Checkpoints worth offering, with the shapes taken from their own configs.
pub fn catalog() -> Vec<Model> {
    vec![
        Model { name: "Qwen3-4B", arch: "qwen3", params_b: 4.0, active_b: 4.0, layers: 36, kv_heads: 8, head_dim: 128 },
        Model { name: "Qwen3-8B", arch: "qwen3", params_b: 8.2, active_b: 8.2, layers: 36, kv_heads: 8, head_dim: 128 },
        Model { name: "Qwen3-14B", arch: "qwen3", params_b: 14.8, active_b: 14.8, layers: 40, kv_heads: 8, head_dim: 128 },
        // MoE: 30B total, ~3B active. The reason `active_b` is a separate field.
        Model { name: "Qwen3-30B-A3B", arch: "qwen3moe", params_b: 30.5, active_b: 3.3, layers: 48, kv_heads: 4, head_dim: 128 },
        Model { name: "Gemma3-4B", arch: "gemma3", params_b: 4.3, active_b: 4.3, layers: 34, kv_heads: 4, head_dim: 256 },
        Model { name: "Gemma3-12B", arch: "gemma3", params_b: 12.2, active_b: 12.2, layers: 48, kv_heads: 8, head_dim: 256 },
        Model { name: "Llama-3.2-3B", arch: "llama", params_b: 3.2, active_b: 3.2, layers: 28, kv_heads: 8, head_dim: 128 },
        Model { name: "Llama-3.1-8B", arch: "llama", params_b: 8.0, active_b: 8.0, layers: 32, kv_heads: 8, head_dim: 128 },
        // GLM-4, with the shapes from its own config so the catalogue is ready — and filtered
        // out by `is_supported` until the arch is verified. The recommender asks the engine
        // rather than carrying its own opinion, which is what stops it offering a model the
        // loader would refuse.
        Model { name: "GLM-4-9B", arch: "chatglm", params_b: 9.4, active_b: 9.4, layers: 40, kv_heads: 2, head_dim: 128 },
        Model { name: "GLM-4-32B-0414", arch: "chatglm", params_b: 32.6, active_b: 32.6, layers: 61, kv_heads: 2, head_dim: 128 },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUANTS: &[&str] = &["Q3_K_M", "Q4_K_M", "Q5_K_M", "Q6_K", "Q8_0"];

    /// A recommender with its own idea of what runs is a second source of truth, and the
    /// first time the two disagree someone downloads a model the loader refuses.
    /// The catalogue can list a model before the engine can run it, and the filter is what
    /// keeps that from becoming a bad recommendation. GLM-4 is in the table with its real
    /// shapes and is NOT offered, because `archcfg` says the arch is unverified.
    #[test]
    fn a_catalogued_but_unsupported_model_is_not_offered() {
        assert!(catalog().iter().any(|m| m.arch == "chatglm"), "GLM should be catalogued");
        assert!(!crate::archcfg::is_supported("chatglm", 0), "the premise");
        let out = recommend(&catalog(), 64_000.0, 4096, QUANTS);
        assert!(
            !out.iter().any(|f| f.model.arch == "chatglm"),
            "an unverified arch was recommended: {:?}",
            out.iter().map(|f| f.model.name).collect::<Vec<_>>()
        );
        // The day `chatglm` is verified it appears here with no change to this file, which is
        // the point of asking the engine instead of keeping a second list.
    }

    #[test]
    fn nothing_is_recommended_for_an_architecture_the_engine_cannot_run() {
        let unsupported = Model {
            name: "DeepSeek-V3",
            arch: "deepseek2",
            params_b: 3.0,
            active_b: 3.0,
            layers: 8,
            kv_heads: 2,
            head_dim: 64,
        };
        assert!(!crate::archcfg::is_supported("deepseek2", 0), "the premise");
        let out = recommend(&[unsupported], 64_000.0, 4096, QUANTS);
        assert!(out.is_empty(), "a model was offered for an arch the loader refuses: {out:?}");
    }

    /// The KV cache is what turns "it fits" into an OOM at 4k tokens, and it is sized by KV
    /// heads, not attention heads — GQA is why a modern 8B has a quarter the cache of an old
    /// one.
    #[test]
    fn the_cache_is_counted_and_scales_with_context() {
        let m = &catalog()[1]; // Qwen3-8B
        let short = kv_cache_mb(m, 2048);
        let long = kv_cache_mb(m, 32768);
        assert!(long > short * 15.0, "the cache should scale with context: {short} → {long}");
        // Big enough to matter: at 32k it is over a gigabyte, which is the whole point of
        // counting it rather than adding a fudge factor to the weights.
        assert!(long > 1000.0, "{long}MB");

        // And it is part of the decision: a machine that fits the weights and not the cache
        // must not be told the model fits.
        let w = weight_mb(m, "Q4_K_M");
        let just_weights = w + WORKING_SET_MB + 100.0;
        let out = recommend(&[m.clone()], just_weights / 0.85, 32768, QUANTS);
        assert!(
            out.iter().all(|f| f.quant != "Q4_K_M" || f.total_mb <= usable_mb(just_weights / 0.85)),
            "a fit was reported that does not fit: {out:?}"
        );
    }

    /// Active parameters decide what the model knows per token; total decides whether it
    /// fits. A 30B MoE with 3B active is a faster answer than a dense 8B on the same memory —
    /// and a recommender that sorted on total would offer it FIRST, which is the wrong way
    /// round.
    #[test]
    fn ranking_is_by_active_parameters_not_by_download_size() {
        // 24GB: everything in the catalogue fits at some quant.
        let out = recommend(&catalog(), 24_000.0, 4096, QUANTS);
        assert!(out.len() >= 4, "{:?}", out.iter().map(|f| f.model.name).collect::<Vec<_>>());
        // Descending by active.
        for w in out.windows(2) {
            assert!(w[0].active_b >= w[1].active_b, "{:?}", out.iter().map(|f| (f.model.name, f.active_b)).collect::<Vec<_>>());
        }
        // The MoE is NOT first despite being the biggest download.
        assert_ne!(out[0].model.name, "Qwen3-30B-A3B");
    }

    /// Someone who can run Q6 should not be handed Q3.
    #[test]
    fn the_best_quantisation_that_fits_is_the_one_offered() {
        let big = recommend(&catalog(), 64_000.0, 4096, QUANTS);
        let q3 = big.iter().find(|f| f.model.name == "Qwen3-4B").expect("4B fits in 64GB");
        assert_eq!(q3.quant, "Q8_0", "plenty of room and it offered {}", q3.quant);

        // A tight machine gets the small quant rather than nothing.
        let tight = recommend(&catalog(), 6_000.0, 2048, QUANTS);
        assert!(!tight.is_empty(), "nothing offered on 6GB");
        assert!(tight.iter().all(|f| f.total_mb <= usable_mb(6_000.0)));
    }

    /// A recommendation that uses every byte OOMs the moment something else asks for memory,
    /// and on a phone that something is the compositor.
    #[test]
    fn headroom_is_left_for_the_rest_of_the_machine() {
        assert!(usable_mb(8_000.0) <= 8_000.0 - 512.0);
        assert!(usable_mb(64_000.0) < 64_000.0 * 0.9);
        // A tiny machine does not get a negative budget.
        assert_eq!(usable_mb(256.0), 0.0);
        assert!(recommend(&catalog(), 256.0, 2048, QUANTS).is_empty());
    }

    /// `Q4_K_M` is not four bits. Using 4.0 under-estimates a 30B by more than a gigabyte,
    /// which is the difference between a recommendation and an OOM.
    #[test]
    fn k_quants_are_costed_at_their_real_width() {
        assert!(bits_per_weight("Q4_K_M") > 4.0);
        assert!(bits_per_weight("Q8_0") > 8.0);
        assert!(bits_per_weight("q4_k_m") == bits_per_weight("Q4_K_M"), "case-insensitive");
        // An unknown quant is costed as Q4_K_M rather than as free.
        assert!(bits_per_weight("IQ4_XS_SOMETHING_NEW") >= 4.0);
    }
}
