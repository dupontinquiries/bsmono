//! archfeat.rs — what each architecture NEEDS versus what this engine IMPLEMENTS.
//!
//! Llama-4 cost three debugging rounds because the engine had *partial* support and no way to say so.
//! NoPE and attention temperature were implemented, `use_kq_norm` was not, and a missing normalization
//! does not crash — it produces fluent nonsense that looks like a quantization bug, a streaming bug, or a
//! conversion bug. Every one of those was suspected and cleared before the real cause was found by reading
//! llama.cpp.
//!
//! This is the liaison rule applied to model support: **declare the gap rather than generate garbage**.
//!
//! ## What this is and is not
//!
//! A hand-maintained checklist transcribed from llama.cpp's per-architecture files
//! (`src/models/<arch>.cpp`), which are the closest thing to a specification that exists. It is NOT
//! derived from our code, so it can drift. Overstating support is the dangerous direction, so entries
//! default to "not implemented" and are only promoted when a gate proves the behaviour.

use std::fmt;

/// A capability an architecture may require beyond a plain Llama block.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Feature {
    /// Every Nth layer skips rope entirely (Llama-4 iRoPE).
    NoPe,
    /// Query scaled by a position-dependent temperature on NoPE layers (Llama-4 long context).
    AttnTemp,
    /// Weightless RMS norm on Q and K, AFTER rope, rope layers only (Llama-4 Llama4TextL2Norm).
    KqNormPostRope,
    /// Learned per-head norm on Q and K BEFORE rope (Qwen3). Different weight AND ordering from
    /// `KqNormPostRope` — conflating the two is precisely the Llama-4 mistake.
    QkNormPreRope,
    /// Chunked sliding-window attention with a layer pattern (Llama-4: 8192 window, 3 chunked / 1 full).
    ChunkedSwa,
    /// Multi-head Latent Attention: KV cached as a compressed latent (DeepSeek-V3, GLM-MoE).
    Mla,
    /// A dense FFN added on top of the routed experts (GLM / DeepSeek / Llama-4).
    SharedExpert,
    /// Expert tensors present on only some blocks (interleave_moe_layer_step).
    InterleavedMoe,
    /// Standard sliding-window attention on a repeating layer pattern, with a SEPARATE rope frequency
    /// base on the windowed layers (Gemma 2/3/4). Distinct from Llama-4's `ChunkedSwa`.
    StandardSwa,
    /// Post-normalization after attention and after the FFN, on top of the usual pre-norms (Gemma family).
    PostNorm,
    /// Per-layer rope frequency tensors, so full-attention layers scale rope differently from windowed
    /// ones (Gemma 4 `rope_freqs`).
    PerLayerRopeFreqs,
    /// A per-layer HEAD DIMENSION: windowed and full-attention layers use different head sizes
    /// (Gemma 4 `attention.key_length_swa` vs `key_length`). Everything here assumes one head_dim for the
    /// whole model, so this is structural rather than a knob.
    PerLayerHeadDim,
    /// A learned scalar multiplying each layer's output (Gemma 4 `layer_output_scale`).
    LayerOutputScale,
    /// MLA reconstruction matrices shipped as SEPARATE, THREE-dimensional `attn_k_b` / `attn_v_b`
    /// (`[head_dim x lora x n_head]`) rather than one fused 2-D `attn_kv_b`.
    ///
    /// Purely a loading concern — the MLA forward itself is implemented and validated — but it blocks
    /// every checkpoint that uses this layout, which is GLM-4.7-Flash and GLM-5.2 at least.
    MlaSplit3dProjections,
    /// GELU-gated FFN instead of SiLU-gated (all Gemma).
    GeGluActivation,
    /// Gemma's RMS norm: `x/rms(x) · (1 + w)`, computed in f32 — the `1 +` is not a detail, it changes
    /// every normalized value.
    GemmaRmsNorm,
    /// tanh soft-capping on attention logits and final logits (Gemma 2; Gemma 3+ default it off).
    LogitSoftcap,
    /// The token embedding is multiplied by sqrt(d) before the first block (all Gemma).
    EmbeddingScale,
    /// Windowed layers rotate at a DIFFERENT rope frequency base than full-attention ones
    /// (Gemma `rope.freq_base_swa`). Distinct from `PerLayerRopeFreqs`, which is per-layer rope
    /// frequency TENSORS in Gemma 4.
    PerLayerRopeBase,
    /// Routed-expert weights are multiplied by a constant from metadata (`expert_weights_scale`).
    ///
    /// Llama-4 and Qwen3 leave it unset, so it defaulted harmlessly to 1. GLM-5.2 sets **2.5** — ignoring
    /// it makes every MoE layer's contribution 2.5x too small, which does not error and does not produce
    /// noise: it produces a model that is quietly worse than it should be.
    ExpertWeightsScale,
    /// Top-k routed weights are renormalized to sum to 1 (`expert_weights_norm`).
    ExpertWeightsNorm,
    /// DeepSeek-style SPARSE ATTENTION: a separate small indexer scores keys, and full attention runs
    /// only over its top-k. GLM-5.2 uses 32 indexer heads of key_length 128 selecting top_k 2048 out of a
    /// 1M context. Structural — there is no dense fallback that is merely slower, because attending to
    /// everything at 1M context is not a thing the engine can do.
    SparseAttentionIndexer,
    /// The attention scale is a hyperparameter rather than 1/sqrt(head_dim). Equal to the default on
    /// most Gemma 3, but NOT on the 27B, which uses 1/sqrt(n_embd/n_head).
    AttentionScaleHparam,
}

impl Feature {
    pub fn label(self) -> &'static str {
        match self {
            Feature::NoPe => "NoPE layers",
            Feature::AttnTemp => "attention temperature",
            Feature::KqNormPostRope => "use_kq_norm (weightless Q/K norm after rope)",
            Feature::QkNormPreRope => "learned Q/K norm before rope",
            Feature::ChunkedSwa => "chunked sliding-window attention",
            Feature::Mla => "multi-head latent attention",
            Feature::SharedExpert => "shared expert",
            Feature::InterleavedMoe => "interleaved MoE layers",
            Feature::StandardSwa => "sliding-window attention (with a separate rope base)",
            Feature::PostNorm => "post-attention / post-FFN normalization",
            Feature::PerLayerRopeFreqs => "per-layer rope frequency tensors",
            Feature::PerLayerHeadDim => "per-layer head dimension (SWA layers differ from full-attention ones)",
            Feature::LayerOutputScale => "per-layer learned output scale",
            Feature::MlaSplit3dProjections => "split 3-D attn_k_b / attn_v_b instead of fused attn_kv_b",
            Feature::GeGluActivation => "GELU-gated FFN",
            Feature::GemmaRmsNorm => "Gemma (1+w) RMS norm",
            Feature::LogitSoftcap => "tanh soft-cap on attention and final logits",
            Feature::EmbeddingScale => "token embedding scaled by sqrt(d)",
            Feature::PerLayerRopeBase => "separate rope base for windowed layers",
            Feature::AttentionScaleHparam => "attention scale from hparams, not 1/sqrt(head_dim)",
            Feature::ExpertWeightsScale => "routed-expert weights scaled by a metadata constant",
            Feature::ExpertWeightsNorm => "routed-expert weights renormalized to sum to 1",
            Feature::SparseAttentionIndexer => "sparse attention: an indexer selects top-k keys",
        }
    }

    /// Does this engine implement it? Promoted to true only when a gate proves the behaviour.
    pub fn implemented(self) -> bool {
        match self {
            Feature::NoPe => true,           // BlockWeights::nope
            Feature::AttnTemp => true,       // llama4::AttnTemp
            Feature::KqNormPostRope => true, // kq_post + --gate-llama4-kqnorm
            Feature::QkNormPreRope => true,  // qk_norm with learned weights (Qwen3)
            Feature::SharedExpert => true,   // MoeLayer::shared
            Feature::InterleavedMoe => true, // per-block expert-set detection in the loader
            // CORRECTED. This was claimed as implemented on the assumption that "we support Gemma2"
            // implied it. The loader only accepts `post_attention_norm` as an alternate NAME for
            // ffn_norm — a pre-FFN norm — which is a different operation from Gemma's post-attention and
            // post-FFN norms. Neither is applied.
            // Now applied: post_attn after attention and post_ffn after the FFN, both before the
            // residual add, loaded through the same (1+w) fold as the pre-norms.
            Feature::PostNorm => true,
            Feature::ChunkedSwa => true,     // same paths as StandardSwa; the rule differs, the wiring does not
            // Forward, compressed cache, per-layer projections, decode block and per-layer routing are
            // all in place and checked (--gate-mla-block + the kernel-validation suite). Not yet exercised
            // on a real GLM/DeepSeek checkpoint — see the note.
            Feature::Mla => true,
            // Wired end to end: f32 decode (windowed KV view), q8 decode (kernel start bound) and
            // prefill (per-row window in the parallel kernel), each checked against an independent
            // reference by --gate-swa. The one hole is the serial prefill fallback used when
            // head_dim > 192, which stays causal-only and SAYS SO at dispatch rather than silently
            // ignoring the window.
            Feature::StandardSwa => true,
            Feature::PerLayerRopeFreqs => false, // one rope base is used for all layers
            Feature::PerLayerHeadDim => false,   // head_dim is a single model-wide value throughout
            Feature::LayerOutputScale => false,  // the tensor is not read
            // Implemented in _289: the loader now reads attn_k_b + attn_v_b and concatenates them, having
            // established from llama.cpp PR #12801 that the 3-D GGUF form is byte-identical to the 2-D
            // declaration. This flag stayed false for four commits after the code landed — the fifth
            // instance of a registry disagreeing with the loader, and the one that made a fixed gap keep
            // reporting itself as a gap.
            Feature::MlaSplit3dProjections => true,
            Feature::GeGluActivation => true,    // selected per block from ArchConfig::act (--gate-gemma)
            Feature::GemmaRmsNorm => true,       // (1+w) folded into the weight at load (--gate-gemma)
            Feature::LogitSoftcap => false,      // not implemented anywhere
            Feature::EmbeddingScale => true,     // Model::embed_scale, set from the arch at load
            Feature::PerLayerRopeBase => true,   // BlockWeights::rope_base, from rope.freq_base_swa
            Feature::AttentionScaleHparam => false, // 1/sqrt(head_dim) is hardcoded in the kernels
            Feature::ExpertWeightsScale => false,   // the key is never read; effectively hardcoded to 1
            Feature::ExpertWeightsNorm => false,    // moe::route normalizes for softmax only
            Feature::SparseAttentionIndexer => false, // no indexer exists
        }
    }

    /// What a user can do about a missing feature, when there is something.
    pub fn note(self) -> Option<&'static str> {
        match self {

            Feature::PerLayerRopeFreqs => Some("full-attention layers get the windowed layers' rope base; \
matters most on long contexts"),
            Feature::PerLayerHeadDim => Some("STRUCTURAL: head_dim is model-wide here, so a checkpoint \
whose windowed and full layers differ cannot be laid out correctly at all"),
            Feature::LayerOutputScale => Some("layer outputs are unscaled; the tensor is present in the \
file and ignored"),
            Feature::LogitSoftcap => Some("Gemma 2 only; Gemma 3 and 4 default it off"),
            Feature::AttentionScaleHparam => Some("matches the 1/sqrt(head_dim) default on Gemma 3 12B \
and below; differs only on the 27B, which uses 1/sqrt(n_embd/n_head)"),
            Feature::ExpertWeightsScale => Some("GLM-5.2 sets 2.5; ignoring it scales every MoE layer's \
output down by that factor — a quietly worse model, not a broken one"),
            Feature::MlaSplit3dProjections => Some("a LOADING gap, not a maths one — the MLA forward is \
implemented and validated; these checkpoints just store k_b and v_b separately and in 3-D"),
            Feature::SparseAttentionIndexer => Some("STRUCTURAL: without the indexer there is no dense \
fallback, because attending to a 1M context in full is not something the engine can do"),
            Feature::Mla => Some("wired and validated against the CPU reference, but never yet run on a \
real DeepSeek/GLM checkpoint — the first such load is the test"),
            _ => None,
        }
    }
}

impl fmt::Display for Feature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.label()) }
}

/// Features an architecture requires, transcribed from llama.cpp's `src/models/<arch>.cpp`.
///
/// `variant` disambiguates where llama.cpp does — for Llama-4 the expert count selects `use_kq_norm`
/// (`use_kq_norm = type != LLM_TYPE_17B_128E`), so Scout needs it and Maverick does not. `0` means
/// "unknown variant" and returns the conservative union.
pub fn required(arch: &str, variant: u32) -> Vec<Feature> {
    match arch {
        "llama4" => {
            let mut v = vec![Feature::NoPe, Feature::AttnTemp, Feature::ChunkedSwa,
                             Feature::SharedExpert, Feature::InterleavedMoe];
            if variant != 128 { v.push(Feature::KqNormPostRope); }
            v
        }
        "qwen3moe" | "qwen3" | "qwen35" => vec![Feature::QkNormPreRope],
        // Gemma 2/3/4 all use standard SWA on a layer pattern with a separate rope base; 3 and 4 add
        // learned Q/K norms before rope, and 4 adds per-layer rope frequency tensors.
        "gemma2" => vec![Feature::StandardSwa, Feature::PostNorm, Feature::GeGluActivation,
                         Feature::GemmaRmsNorm, Feature::LogitSoftcap, Feature::EmbeddingScale, Feature::PerLayerRopeBase, Feature::AttentionScaleHparam],
        "gemma3" | "gemma3n" => vec![Feature::StandardSwa, Feature::PostNorm, Feature::QkNormPreRope,
                                     Feature::GeGluActivation, Feature::GemmaRmsNorm, Feature::EmbeddingScale, Feature::PerLayerRopeBase, Feature::AttentionScaleHparam],
        // Gemma 4 (src/models/gemma4.cpp): everything gemma3 needs, plus per-layer rope freqs, a
        // per-layer HEAD DIM (key_length_swa vs key_length) and a learned per-layer output scale.
        "gemma4" => vec![Feature::StandardSwa, Feature::PostNorm, Feature::QkNormPreRope,
                         Feature::GeGluActivation, Feature::GemmaRmsNorm,
                         Feature::PerLayerRopeFreqs, Feature::PerLayerHeadDim, Feature::LayerOutputScale,
                         Feature::EmbeddingScale, Feature::PerLayerRopeBase, Feature::AttentionScaleHparam],
        // deepseek2/3 covers GLM-4.7-Flash, which ships the split 3-D projections. Listed so the
        // refusal names the real blocker rather than blaming the MLA forward, which is implemented.
        "deepseek2" | "deepseek3" => vec![Feature::Mla, Feature::SharedExpert,
                                          Feature::MlaSplit3dProjections],
        "glm4moe" => vec![Feature::Mla, Feature::SharedExpert],
        // GLM-5.2. Transcribed from the shipped first shard's metadata rather than guessed:
        //   expert_gating_func 2 (sigmoid), expert_weights_norm true, expert_weights_scale 2.5,
        //   kv_lora_rank 512 / q_lora_rank 2048 (MLA), expert_shared_count 1,
        //   attention.indexer.{head_count 32, key_length 128, top_k 2048} over a 1048576 context.
        "glm-dsa" => vec![Feature::Mla, Feature::SharedExpert, Feature::ExpertWeightsScale,
                          Feature::ExpertWeightsNorm, Feature::SparseAttentionIndexer,
                          Feature::MlaSplit3dProjections],
        _ => Vec::new(),
    }
}

/// Required features this engine does NOT implement — the things that silently degrade output.
pub fn gaps(arch: &str, variant: u32) -> Vec<Feature> {
    required(arch, variant).into_iter().filter(|f| !f.implemented()).collect()
}

/// A loader-facing report. Empty when nothing is missing, so the good case prints nothing.
pub fn gap_report(arch: &str, variant: u32) -> Vec<String> {
    gaps(arch, variant).into_iter().map(|f| match f.note() {
        Some(n) => format!("{arch} needs {f} — NOT implemented ({n})"),
        None => format!("{arch} needs {f} — NOT implemented; output will be wrong"),
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scout_needs_the_post_rope_norm_and_maverick_does_not() {
        assert!(required("llama4", 16).contains(&Feature::KqNormPostRope), "Scout (16E) needs it");
        assert!(!required("llama4", 128).contains(&Feature::KqNormPostRope), "Maverick (128E) does not");
        // an unknown variant takes the conservative union rather than assuming the lenient case
        assert!(required("llama4", 0).contains(&Feature::KqNormPostRope));
    }

    #[test]
    fn the_two_qk_norms_are_distinct_features() {
        // Conflating these IS the Llama-4 bug: different weight (learned vs unit), different ordering
        // (before vs after rope). If they ever collapse into one entry, that bug becomes possible again.
        assert_ne!(Feature::KqNormPostRope, Feature::QkNormPreRope);
        assert!(required("qwen3moe", 0).contains(&Feature::QkNormPreRope));
        assert!(!required("qwen3moe", 0).contains(&Feature::KqNormPostRope));
        assert!(!required("llama4", 16).contains(&Feature::QkNormPreRope));
    }

    #[test]
    fn maverick_differs_from_scout_only_where_llamacpp_says() {
        // Llama-4 Maverick: 17B active, 400B total, 128 experts, 1M context — versus Scout's 16 experts
        // and 10M. The ONLY architectural difference llama4.cpp encodes is use_kq_norm, which is
        // `type != LLM_TYPE_17B_128E` — so Maverick skips the post-rope norm Scout needs.
        let scout = required("llama4", 16);
        let mav = required("llama4", 128);
        assert!(scout.contains(&Feature::KqNormPostRope), "Scout applies it");
        assert!(!mav.contains(&Feature::KqNormPostRope), "Maverick does NOT — the 128E branch");
        // everything else is shared, and both are gap-free
        for f in [Feature::NoPe, Feature::AttnTemp, Feature::ChunkedSwa, Feature::SharedExpert,
                  Feature::InterleavedMoe] {
            assert!(scout.contains(&f) && mav.contains(&f), "{f:?} is shared");
        }
        assert!(gaps("llama4", 128).is_empty(), "{:?}", gaps("llama4", 128));
    }

    #[test]
    fn scout_has_no_remaining_declared_gaps() {
        // Llama-4 needed NoPE, attention temperature, chunked SWA, a shared expert, interleaved MoE and
        // use_kq_norm. Every one is now implemented; the last two landed this session.
        let g = gaps("llama4", 16);
        assert!(g.is_empty(), "unexpected gaps: {g:?}");
        // Maverick (128 experts) differs only in skipping use_kq_norm, and is likewise clear
        assert!(gaps("llama4", 128).is_empty());
    }

    #[test]
    fn mla_is_not_the_blocker_but_the_tensor_LAYOUT_is() {
        // This test has now said three things across the project, and the progression is the point:
        // first "MLA blocks GLM" (true — the kernels existed but nothing called them), then "MLA no
        // longer blocks it" (true — _211 wired it), and now the accurate version. GLM-4.7-Flash still
        // fails to load, but NOT because of MLA maths.
        assert!(Feature::Mla.implemented(), "the forward is wired and validated");
        assert!(!gaps("deepseek2", 0).contains(&Feature::Mla), "MLA is not what blocks it");

        // FOURTH thing this test has said, and the reason the running commentary is kept: the layout gap
        // is now CLOSED too (_289 reads attn_k_b + attn_v_b and concatenates). The flag stayed false for
        // four commits after the code landed, so a fixed gap kept reporting itself as a gap in the
        // loader's own warning — the fifth instance of a registry disagreeing with what the code does.
        assert!(Feature::MlaSplit3dProjections.implemented(), "_289 reads the split form");
        assert!(!gaps("deepseek2", 0).contains(&Feature::MlaSplit3dProjections));
        assert!(!gaps("glm-dsa", 0).contains(&Feature::MlaSplit3dProjections));

        // glm4moe uses the fused form and is genuinely clear.
        assert!(gaps("glm4moe", 0).is_empty(), "{:?}", gaps("glm4moe", 0));
    }

    #[test]
    fn every_gemma_feature_promoted_here_has_a_caller_not_just_a_kernel() {
        // This test has now said three different things, and the history is the point. It first claimed
        // Gemma2 was fully supported (wrong — the table was overstating). It was then corrected to assert
        // nothing was implemented. Each of these is now promoted because something CALLS it, not because
        // a kernel exists:
        //   PostNorm        block_decode applies post_attn; ffn_apply applies post_ffn
        //   GeGluActivation ffn_apply selects it from BlockWeights::geglu, set from ArchConfig::act
        //   GemmaRmsNorm    gemma_norm() folds (1+w) into every norm weight at load
        // all three checked by --gate-gemma.
        for f in [Feature::PostNorm, Feature::GeGluActivation, Feature::GemmaRmsNorm, Feature::StandardSwa] {
            assert!(f.implemented(), "{f:?} should be implemented");
        }
        // and the one thing still genuinely missing is not quietly promoted with them
        assert!(!Feature::LogitSoftcap.implemented(), "soft-capping is still not implemented");
    }

    #[test]
    fn gemma4_needs_three_things_gemma3_does_not() {
        let g3 = required("gemma3", 0);
        let g4 = required("gemma4", 0);
        assert!(g3.contains(&Feature::QkNormPreRope) && g4.contains(&Feature::QkNormPreRope));
        for f in [Feature::PerLayerRopeFreqs, Feature::PerLayerHeadDim, Feature::LayerOutputScale] {
            assert!(!g3.contains(&f), "gemma3 does not need {f:?}");
            assert!(g4.contains(&f), "gemma4 does need {f:?}");
        }
    }

    #[test]
    fn gemma4_names_a_structural_blocker_not_a_missing_knob() {
        // Per-layer head_dim is not something to bolt on: head_dim is a single model-wide value
        // everywhere here. The report must distinguish that from a merely unimplemented option, because
        // the two imply completely different amounts of work.
        let r = gap_report("gemma4", 0);
        assert!(r.iter().any(|s| s.contains("STRUCTURAL")), "{r:?}");
        assert!(r.iter().any(|s| s.contains("present in the file and ignored")),
                "an ignored tensor should say so: {r:?}");
    }

    #[test]
    fn the_two_sliding_windows_are_distinct_features() {
        // Llama-4 uses CHUNKED SWA; Gemma uses STANDARD SWA with a separate rope base. Implementing one
        // does not give the other, so they must not collapse — the same trap as the two Q/K norms.
        assert_ne!(Feature::ChunkedSwa, Feature::StandardSwa);
        assert!(required("llama4", 16).contains(&Feature::ChunkedSwa));
        assert!(!required("llama4", 16).contains(&Feature::StandardSwa));
        assert!(required("gemma3", 0).contains(&Feature::StandardSwa));
        assert!(!required("gemma3", 0).contains(&Feature::ChunkedSwa));
    }

    #[test]
    fn every_gemma_needs_the_embedding_scale_and_it_is_implemented() {
        // Found by reading the FORWARD PASS rather than the tensor list: every gemma*.cpp starts with
        // `inpL = ggml_scale(inpL, sqrtf(n_embd))`. It is not a tensor, so nothing about the checkpoint
        // hints at it — and at d=3840 the factor is ~62, so a model missing it produces nonsense rather
        // than slightly-off output. Gemma 3 was declared supported WITHOUT it, which is the bug this
        // entry exists to stop recurring.
        for a in ["gemma2", "gemma3", "gemma4"] {
            assert!(required(a, 0).contains(&Feature::EmbeddingScale), "{a} needs it");
        }
        assert!(Feature::EmbeddingScale.implemented());
        assert!(!required("llama", 0).contains(&Feature::EmbeddingScale), "llama does not scale");
    }

    #[test]
    fn gemma3_is_down_to_no_gaps_and_gemma2_only_needs_the_softcap() {
        // A LINE-BY-LINE read of gemma3.cpp's graph found two more things after this test first passed:
        // the sqrt(d) embedding scale and a separate rope base for windowed layers. Both are now
        // implemented. The third finding, the hparam attention scale, matches our 1/sqrt(head_dim)
        // default on every Gemma 3 except the 27B — so it is listed as a requirement and NOT implemented,
        // which is what keeps a 27B from silently loading wrong.
        assert_eq!(gaps("gemma3", 0), vec![Feature::AttentionScaleHparam]);
        assert!(Feature::PerLayerRopeBase.implemented() && Feature::EmbeddingScale.implemented());
        // Gemma 2 shares those but also soft-caps its attention and final logits, which is unimplemented.
        assert_eq!(gaps("gemma2", 0), vec![Feature::LogitSoftcap, Feature::AttentionScaleHparam]);
        // Gemma 4's remaining gaps are the structural ones, unchanged by this work.
        let g4 = gaps("gemma4", 0);
        assert!(g4.contains(&Feature::PerLayerHeadDim) && g4.contains(&Feature::LayerOutputScale));
        assert!(!g4.contains(&Feature::GeGluActivation), "the activation is no longer a gap for any Gemma");
    }

    #[test]
    fn the_three_moe_gating_shapes_stay_distinct() {
        // Graph-level reads confirmed two DIFFERENT MoE conventions, and a third architecture family
        // uses a third. Collapsing them is the same class of mistake as the two Q/K norms.
        let l4 = crate::archcfg::moe_config(&crate::archcfg::for_arch("llama4"), 16, 1, 1);
        let q3 = crate::archcfg::moe_config(&crate::archcfg::for_arch("qwen3moe"), 128, 8, 0);
        // llama4: sigmoid, weight on the expert INPUT, no normalization
        assert_eq!(l4.norm, crate::moe::RouteNorm::Sigmoid);
        assert!(l4.weight_before && !l4.normalize);
        // qwen3moe: softmax over NORMALIZED weights, applied on the output
        assert_eq!(q3.norm, crate::moe::RouteNorm::Softmax);
        assert!(!q3.weight_before && q3.normalize);
        // and they must not be the same config by accident
        assert_ne!((l4.norm, l4.weight_before, l4.normalize), (q3.norm, q3.weight_before, q3.normalize));
    }

    #[test]
    fn llama4_was_verified_at_graph_level_and_its_routing_is_sigmoid() {
        // The graph-level read of llama4.cpp found no gaps. The single most load-bearing thing it
        // confirmed is the expert gating: Llama-4 uses SIGMOID with the weight applied to the expert's
        // INPUT and no normalization, where nearly every other MoE uses softmax over normalized weights.
        // Getting that wrong would degrade output without any error, so it is worth a standing assertion
        // rather than a note in a commit.
        let c = crate::archcfg::for_arch("llama4");
        let mc = crate::archcfg::moe_config(&c, 16, 1, 1);
        assert_eq!(mc.norm, crate::moe::RouteNorm::Sigmoid, "llama.cpp: EXPERT_GATING_FUNC_TYPE_SIGMOID");
        assert!(mc.weight_before, "the gate multiplies the expert INPUT");
        assert!(!mc.normalize, "llama.cpp passes norm_w = false");
        // and the contrast: qwen3moe is the ordinary softmax-over-normalized-weights case
        let q = crate::archcfg::moe_config(&crate::archcfg::for_arch("qwen3moe"), 128, 8, 0);
        assert_eq!(q.norm, crate::moe::RouteNorm::Softmax);
        assert!(!q.weight_before && q.normalize);
    }

    #[test]
    fn glm52_requirements_come_from_its_shipped_metadata() {
        // Transcribed from GLM-5_2-UD-IQ1_S-00001-of-00006.gguf, not inferred from the family name.
        // glm-dsa is NOT glm4moe: it adds a sparse-attention indexer and two expert-weighting knobs that
        // the GLM-4 line does not have, and treating them as one architecture would silently apply the
        // wrong forward pass.
        let g = gaps("glm-dsa", 0);
        for f in [Feature::SparseAttentionIndexer, Feature::ExpertWeightsScale, Feature::ExpertWeightsNorm] {
            assert!(g.contains(&f), "glm-dsa needs {f:?}");
            assert!(!required("glm4moe", 0).contains(&f), "glm4moe does NOT — they are different archs");
        }
        // MLA is shared and already implemented, so it must not appear as a gap for either
        assert!(!g.contains(&Feature::Mla));

        // The scale is the dangerous one: it fails QUIETLY. 2.5 ignored means every MoE layer contributes
        // 40% of what it should, which reads as a mediocre model rather than a broken one.
        assert!(Feature::ExpertWeightsScale.note().unwrap().contains("quietly worse"));
        // The indexer is the blocking one, and the report must say why there is no degraded mode.
        assert!(gap_report("glm-dsa", 0).iter().any(|s| s.contains("STRUCTURAL")));
    }

    #[test]
    fn an_unknown_architecture_claims_nothing() {
        // Silence is honest: we have not transcribed requirements, which differs from knowing it needs
        // none. The loader's tensor-level wiring check is the backstop.
        assert!(required("some-new-arch", 0).is_empty());
        assert!(gap_report("some-new-arch", 0).is_empty());
    }

    #[test]
    fn a_fully_supported_architecture_reports_no_gaps() {
        assert!(gaps("qwen3moe", 0).is_empty(), "qwen3moe should be fully covered");
    }
}
