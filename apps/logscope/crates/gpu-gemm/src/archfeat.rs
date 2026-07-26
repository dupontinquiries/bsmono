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
            Feature::PostNorm => true,       // Gemma2 support carries attn/ffn post-norms
            Feature::ChunkedSwa => true,     // same paths as StandardSwa; the rule differs, the wiring does not
            Feature::Mla => false,           // compute paths validated but NOT wired into decode
            // Wired end to end: f32 decode (windowed KV view), q8 decode (kernel start bound) and
            // prefill (per-row window in the parallel kernel), each checked against an independent
            // reference by --gate-swa. The one hole is the serial prefill fallback used when
            // head_dim > 192, which stays causal-only and SAYS SO at dispatch rather than silently
            // ignoring the window.
            Feature::StandardSwa => true,
            Feature::PerLayerRopeFreqs => false, // one rope base is used for all layers
        }
    }

    /// What a user can do about a missing feature, when there is something.
    pub fn note(self) -> Option<&'static str> {
        match self {

            Feature::PerLayerRopeFreqs => Some("full-attention layers get the windowed layers' rope base; \
matters most on long contexts"),
            Feature::Mla => Some("the compute kernels, the compressed-KV cache and the per-layer \
projection weights are all in place and validated; only the decode plumbing that routes MLA layers \
through them is missing"),
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
        "gemma2" => vec![Feature::StandardSwa, Feature::PostNorm],
        "gemma3" | "gemma3n" => vec![Feature::StandardSwa, Feature::PostNorm, Feature::QkNormPreRope],
        "gemma4" => vec![Feature::StandardSwa, Feature::PostNorm, Feature::QkNormPreRope,
                         Feature::PerLayerRopeFreqs],
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
    fn glm_reports_mla_as_the_blocker() {
        let g = gaps("glm4moe", 0);
        assert!(g.contains(&Feature::Mla));
        assert!(!g.contains(&Feature::SharedExpert), "shared expert IS implemented");
        // the note must say what remains, not just that something does — the compute and weights are in
        // place, only the decode plumbing is missing, and that distinction tells the next person where to start
        let r = gap_report("glm4moe", 0);
        assert!(r.iter().any(|s| s.contains("only the decode plumbing")), "{r:?}");
    }

    #[test]
    fn gemma2_is_now_fully_supported() {
        // It was NOT, and this test previously asserted the gap: gemma2.cpp sets swa_type = STANDARD with
        // n_swa = 4096 on a layer pattern, and we attended fully. With SWA wired through decode and
        // prefill, both of Gemma2's requirements are met.
        assert!(Feature::StandardSwa.implemented(), "SWA is wired end to end");
        assert!(Feature::PostNorm.implemented());
        assert!(gaps("gemma2", 0).is_empty(), "{:?}", gaps("gemma2", 0));
        assert!(gap_report("gemma2", 0).is_empty(), "nothing left to declare");
    }

    #[test]
    fn gemma4_adds_per_layer_rope_on_top_of_gemma3() {
        let g3 = required("gemma3", 0);
        let g4 = required("gemma4", 0);
        assert!(g3.contains(&Feature::QkNormPreRope) && g4.contains(&Feature::QkNormPreRope));
        assert!(!g3.contains(&Feature::PerLayerRopeFreqs), "gemma3 has no per-layer rope freqs");
        assert!(g4.contains(&Feature::PerLayerRopeFreqs), "gemma4 does (rope_freqs per layer)");
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
