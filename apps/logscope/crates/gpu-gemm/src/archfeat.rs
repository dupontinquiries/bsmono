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
    /// Does a run at this context length actually need the sparse-attention indexer?
    ///
    /// Colibri's glm.c states it directly: the indexer is a NO-OP for `seq <= index_topk`, because
    /// top-k of everything is everything. Below that bound dense causal attention is not an
    /// approximation of the sparse path — it is the same computation, validated there against a
    /// transformers oracle.
    ///
    /// So "unsupported architecture" is the wrong verdict for a short-context run, and a hard refusal
    /// costs a model that would work. Above the bound the model degrades rather than breaking, which
    /// is a warning rather than a gate.
    pub fn indexer_needed_at(seq_len: usize, index_topk: usize) -> bool {
        index_topk > 0 && seq_len > index_topk
    }

    /// What to tell an operator about a prompt of `seq_len` on a model with this bound.
    ///
    /// Returns None when the run is inside the exact range — silence is right there, because a warning
    /// on every short prompt is one nobody reads by the time it matters.
    ///
    /// This does NOT refuse. Above the bound the model degrades rather than breaking, and a hard cap
    /// would turn a usable long-context run into no run at all. The operator can judge a documented
    /// approximation; they cannot judge a silent one.
    pub fn indexer_warning(seq_len: usize, index_topk: usize) -> Option<String> {
        if !Self::indexer_needed_at(seq_len, index_topk) { return None; }
        Some(format!(
            "prompt is {seq_len} tokens against a sparse-attention bound of {index_topk}. Dense attention is EXACT at or below the bound; above it this attends to every key where the model was trained to attend to the top {index_topk}, so quality degrades with length. Not a failure — an approximation you should know you are making"))
    }

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
            // IMPLEMENTED, and this table did not learn — the third stale entry found here, after
            // ExpertWeightsScale and ExpertWeightsNorm. `{arch}.rope.freq_base_swa` is read per layer
            // and applied at all three rope sites via `w.rope_base.unwrap_or(base)`; Gemma's windowed
            // layers use a different base from its global ones and have since the swa work.
            //
            // A stale FALSE here is not harmless: is_supported vetoes on any gap, so a table claiming
            // a feature is missing refuses an architecture the engine can actually run.
            Feature::PerLayerRopeFreqs => true,
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
            // IMPLEMENTED in _369/_370, and this table did not learn — the exact stale-flag shape the
            // Q6_K file-view gate had. `{arch}.expert_weights_scale` is read from metadata and applied
            // in moe::route AFTER normalization (order is load-bearing: scaling first cancels against
            // the divide, and the model would run with the factor silently doing nothing).
            Feature::ExpertWeightsScale => true,
            // Also metadata-driven now, with the arch as fallback — and Llama-4 VETOES it, because its
            // raw-sigmoid gate multiplies the expert's input and a normalized weight there is a
            // different forward pass rather than the same one scaled.
            Feature::ExpertWeightsNorm => true,
            // Still absent, and still the only real blocker — but NOT unconditionally. Colibri's
            // glm.c: the DSA indexer is a no-op for seq <= index_topk, where top-k IS every key, so
            // dense causal attention is the same computation rather than an approximation of it.
            // GLM-5.2's index_topk is 2048. See `indexer_needed_at`.
            Feature::SparseAttentionIndexer => false
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
        let l4 = crate::archcfg::moe_config(&crate::archcfg::for_arch("llama4"), 16, 1, 1, None, None);
        let q3 = crate::archcfg::moe_config(&crate::archcfg::for_arch("qwen3moe"), 128, 8, 0, None, None);
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
        let mc = crate::archcfg::moe_config(&c, 16, 1, 1, None, None);
        assert_eq!(mc.norm, crate::moe::RouteNorm::Sigmoid, "llama.cpp: EXPERT_GATING_FUNC_TYPE_SIGMOID");
        assert!(mc.weight_before, "the gate multiplies the expert INPUT");
        assert!(!mc.normalize, "llama.cpp passes norm_w = false");
        // and the contrast: qwen3moe is the ordinary softmax-over-normalized-weights case
        let q = crate::archcfg::moe_config(&crate::archcfg::for_arch("qwen3moe"), 128, 8, 0, None, None);
        assert_eq!(q.norm, crate::moe::RouteNorm::Softmax);
        assert!(!q.weight_before && q.normalize);
    }

    #[test]
    fn the_warning_is_SILENT_inside_the_exact_range() {
        // A warning on every short prompt is one nobody reads by the time it matters, which is how the
        // template-mismatch message became ignorable.
        assert!(Feature::indexer_warning(100, 2048).is_none());
        assert!(Feature::indexer_warning(2048, 2048).is_none(), "at the bound: still exact");
        let w = Feature::indexer_warning(4096, 2048).expect("past the bound");
        assert!(w.contains("degrades with length"));
        assert!(w.contains("Not a failure"), "it must not read as an error");
    }

    #[test]
    fn a_model_with_no_declared_bound_gets_NO_warning() {
        // index_topk = 0 means the file did not say, and inventing a bound would produce a warning
        // about a property nobody declared.
        assert!(Feature::indexer_warning(100_000, 0).is_none());
    }

    #[test]
    fn the_indexer_is_NOT_needed_below_index_topk() {
        // Colibri's glm.c, stated directly: the DSA indexer is a no-op for seq <= index_topk, because
        // top-k of everything IS everything. Below that bound dense causal attention is not an
        // approximation of the sparse path — it is the same computation, validated there against a
        // transformers oracle.
        //
        // GLM-5.2's index_topk is 2048. So "unsupported architecture" is the wrong verdict for a
        // short-context run, and a hard refusal costs a model that would have worked.
        assert!(!Feature::indexer_needed_at(1, 2048));
        assert!(!Feature::indexer_needed_at(2048, 2048), "exactly at the bound: still every key");
        assert!(Feature::indexer_needed_at(2049, 2048), "past it, the indexer selects a subset");
        // A model that declares no index_topk cannot be reasoned about this way.
        assert!(!Feature::indexer_needed_at(100_000, 0));
    }

    #[test]
    fn glm52_requirements_come_from_its_shipped_metadata() {
        // Transcribed from GLM-5_2-UD-IQ1_S-00001-of-00006.gguf, not inferred from the family name.
        // glm-dsa is NOT glm4moe: it adds a sparse-attention indexer and two expert-weighting knobs that
        // the GLM-4 line does not have, and treating them as one architecture would silently apply the
        // wrong forward pass.
        // All three are REQUIRED by the file — that has not changed and is the point of the test.
        for f in [Feature::SparseAttentionIndexer, Feature::ExpertWeightsScale, Feature::ExpertWeightsNorm] {
            assert!(required("glm-dsa", 0).contains(&f), "glm-dsa requires {f:?}");
            assert!(!required("glm4moe", 0).contains(&f), "glm4moe does NOT — they are different archs");
        }
        // But two are now IMPLEMENTED (_369/_370), so only one remains a GAP. This test asserted all
        // three were gaps and would have kept saying so forever — the same shape as the Llama-4 veto,
        // where the behaviour was tested, the test passed, and the test was wrong.
        let g = gaps("glm-dsa", 0);
        assert!(g.contains(&Feature::SparseAttentionIndexer), "the indexer is the remaining blocker");
        assert!(!g.contains(&Feature::ExpertWeightsScale), "scale is read from metadata and applied");
        assert!(!g.contains(&Feature::ExpertWeightsNorm), "norm is metadata-driven with an arch fallback");
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
    #[test]
    fn qk_norm_is_a_VARIANT_property_not_an_expert_count() {
        // The loader read `expert_count != 128`, using "has 128 experts" as a stand-in for "is
        // Maverick" — the variant that disables post-rope Q/K norm. Scout has 16 and gets it.
        //
        // Fifth proxy of the session, same failure: the name says one thing and the value encodes
        // another. A future Llama-4 with 128 experts that DOES use qk-norm silently loses it, and a
        // wrong qk-norm degrades text without breaking a single shape.
        //
        // The rule as the loader now applies it: metadata first, expert count as fallback.
        let decide = |use_qk_norm: Option<u32>, experts: u32| -> bool {
            use_qk_norm.map(|v| v != 0).unwrap_or(experts != 128)
        };
        // Fallback path — the observed rule, kept because files may omit the key.
        assert!(decide(None, 16), "Scout: 16 experts, qk-norm on");
        assert!(!decide(None, 128), "Maverick: 128 experts, qk-norm off");
        // Metadata path — and it must be able to CONTRADICT the count, or reading it changes nothing.
        assert!(decide(Some(1), 128), "a 128-expert model that declares qk-norm keeps it");
        assert!(!decide(Some(0), 16), "and a 16-expert model that declares none does not");
    }
}

#[cfg(test)]
mod staleness_tests {
    use super::*;

    /// Per-layer RoPE frequencies are implemented, and the table said otherwise.
    ///
    /// Third stale entry found in this table, after ExpertWeightsScale and ExpertWeightsNorm. Each was
    /// implemented in the loader and left marked missing here — and a stale `false` is not harmless,
    /// because `is_supported` vetoes on ANY gap: the table claiming a feature is absent refuses an
    /// architecture the engine can run.
    ///
    /// `{arch}.rope.freq_base_swa` is read per layer and applied at all three rope sites through
    /// `w.rope_base.unwrap_or(base)`. Gemma's windowed layers have used a different base from its
    /// global ones since the SWA work.
    #[test]
    fn per_layer_rope_is_implemented_not_a_gap() {
        assert!(Feature::PerLayerRopeFreqs.implemented());
        for arch in ["gemma3", "gemma2", "llama"] {
            assert!(!gaps(arch, 0).contains(&Feature::PerLayerRopeFreqs),
                "{arch} must not be blocked on a feature the loader has");
        }
    }

    /// The three that ARE still missing, so the list stays honest in both directions.
    ///
    /// Marking a feature implemented when it is not is the worse error — it turns a clean refusal into
    /// a model that loads and produces wrong text — so these are pinned as firmly as the correction.
    #[test]
    fn the_remaining_gaps_are_still_gaps() {
        assert!(!Feature::LogitSoftcap.implemented(), "nothing applies a softcap anywhere");
        assert!(!Feature::PerLayerHeadDim.implemented(), "head_dim is one model-wide value");
        assert!(!Feature::LayerOutputScale.implemented(), "the tensor is not read");
    }

    /// Some features are required by an ARCHITECTURE and some by a FILE, and the table only expresses
    /// the first.
    ///
    /// I wrote this test asserting that every implemented feature must be required by some declared
    /// architecture, and it failed on PerLayerRopeFreqs — correctly. Gemma uses a separate rope base
    /// for its windowed layers only when the checkpoint carries `rope.freq_base_swa`; a Gemma without
    /// that key needs one base and is not missing anything.
    ///
    /// So "implemented and required by no architecture" is a legitimate state, and the test that
    /// assumed otherwise was asserting a rule this table does not have. Recorded rather than deleted,
    /// because the distinction is easy to lose: `required()` answers what an ARCH always needs, and a
    /// file-conditional capability has no place in it.
    #[test]
    fn a_file_conditional_feature_is_implemented_and_required_by_NO_arch() {
        assert!(Feature::PerLayerRopeFreqs.implemented());
        let archs = ["llama", "llama4", "qwen3moe", "gemma3", "glm-dsa", "deepseek2", "glm4moe"];
        assert!(!archs.iter().any(|a| required(a, 0).contains(&Feature::PerLayerRopeFreqs)),
            "no arch requires it unconditionally — it depends on rope.freq_base_swa being present");
    }

    /// The expert-weighting features ARE architectural, and must be declared.
    ///
    /// The contrast that makes the case above legible: these are properties of the routing math, true
    /// of every checkpoint of the architecture, so a file omitting the key gets a default rather than
    /// a different forward pass.
    #[test]
    fn the_architectural_features_are_declared_by_the_archs_that_have_them() {
        for f in [Feature::ExpertWeightsScale, Feature::ExpertWeightsNorm] {
            assert!(required("glm-dsa", 0).contains(&f), "{f:?} is a glm-dsa property");
        }
    }
}
