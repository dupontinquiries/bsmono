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
    /// GELU-gated FFN instead of SiLU-gated (all Gemma).
    GeGluActivation,
    /// Gemma's RMS norm: `x/rms(x) · (1 + w)`, computed in f32 — the `1 +` is not a detail, it changes
    /// every normalized value.
    GemmaRmsNorm,
    /// tanh soft-capping on attention logits and final logits (Gemma 2; Gemma 3+ default it off).
    LogitSoftcap,
    /// The token embedding is multiplied by sqrt(d) before the first block (all Gemma).
    EmbeddingScale,
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
            Feature::GeGluActivation => "GELU-gated FFN",
            Feature::GemmaRmsNorm => "Gemma (1+w) RMS norm",
            Feature::LogitSoftcap => "tanh soft-cap on attention and final logits",
            Feature::EmbeddingScale => "token embedding scaled by sqrt(d)",
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
            Feature::GeGluActivation => true,    // selected per block from ArchConfig::act (--gate-gemma)
            Feature::GemmaRmsNorm => true,       // (1+w) folded into the weight at load (--gate-gemma)
            Feature::LogitSoftcap => false,      // not implemented anywhere
            Feature::EmbeddingScale => true,     // Model::embed_scale, set from the arch at load
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
                         Feature::GemmaRmsNorm, Feature::LogitSoftcap, Feature::EmbeddingScale],
        "gemma3" | "gemma3n" => vec![Feature::StandardSwa, Feature::PostNorm, Feature::QkNormPreRope,
                                     Feature::GeGluActivation, Feature::GemmaRmsNorm, Feature::EmbeddingScale],
        // Gemma 4 (src/models/gemma4.cpp): everything gemma3 needs, plus per-layer rope freqs, a
        // per-layer HEAD DIM (key_length_swa vs key_length) and a learned per-layer output scale.
        "gemma4" => vec![Feature::StandardSwa, Feature::PostNorm, Feature::QkNormPreRope,
                         Feature::GeGluActivation, Feature::GemmaRmsNorm,
                         Feature::PerLayerRopeFreqs, Feature::PerLayerHeadDim, Feature::LayerOutputScale,
                         Feature::EmbeddingScale],
        "glm4moe" | "glm-dsa" | "deepseek2" | "deepseek3" => vec![Feature::Mla, Feature::SharedExpert],
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
    fn scout_has_no_remaining_declared_gaps() {
        // Llama-4 needed NoPE, attention temperature, chunked SWA, a shared expert, interleaved MoE and
        // use_kq_norm. Every one is now implemented; the last two landed this session.
        let g = gaps("llama4", 16);
        assert!(g.is_empty(), "unexpected gaps: {g:?}");
        // Maverick (128 experts) differs only in skipping use_kq_norm, and is likewise clear
        assert!(gaps("llama4", 128).is_empty());
    }

    #[test]
    fn glm_no_longer_reports_mla_as_a_blocker() {
        // It DID, through most of this project: the kernels existed but nothing called them. Forward,
        // compressed cache, projections, decode block and per-layer routing are now all in place.
        assert!(Feature::Mla.implemented());
        assert!(gaps("glm4moe", 0).is_empty(), "{:?}", gaps("glm4moe", 0));
        assert!(gaps("deepseek3", 0).is_empty());
        // but the note must still say it has never run on a real checkpoint — validated against a CPU
        // reference is not the same as having loaded a 200 GB model, and pretending otherwise is how a
        // "supported" architecture surprises someone
        assert!(Feature::Mla.note().unwrap().contains("never yet run on a real"));
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
        // Gemma 3 needed five things: SWA, post-norms, learned Q/K norm before rope, GELU gating and the
        // (1+w) norm. All five are now in.
        assert!(gaps("gemma3", 0).is_empty(), "{:?}", gaps("gemma3", 0));
        // Gemma 2 shares those but also soft-caps its attention and final logits, which is unimplemented.
        assert_eq!(gaps("gemma2", 0), vec![Feature::LogitSoftcap]);
        // Gemma 4's remaining gaps are the structural ones, unchanged by this work.
        let g4 = gaps("gemma4", 0);
        assert!(g4.contains(&Feature::PerLayerHeadDim) && g4.contains(&Feature::LayerOutputScale));
        assert!(!g4.contains(&Feature::GeGluActivation), "the activation is no longer a gap for any Gemma");
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
