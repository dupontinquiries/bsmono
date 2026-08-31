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
    /// IMPLEMENTED since _289 — the loader reads both and concatenates them PER HEAD. This doc said
    /// "blocks GLM-4.7-Flash and GLM-5.2 at least" for several bundles after the code landed, while
    /// `implemented()` below correctly said true: prose and table disagreeing about the same fact,
    /// which is how a fixed gap keeps being reported as a gap by anyone who reads the enum first.
    MlaSplit3dProjections,
    /// GELU-gated FFN instead of SiLU-gated (all Gemma).
    GeGluActivation,
    /// Gemma's RMS norm: `x/rms(x) · (1 + w)`, computed in f32 — the `1 +` is not a detail, it changes
    /// every normalized value.
    GemmaRmsNorm,
    /// tanh soft-capping on the FINAL logits (Gemma 2; Gemma 3+ default it off).
    ///
    /// Split from the attention cap because the two have completely different costs: this one is an
    /// elementwise pass over the output vector, the other lives inside the softmax. One feature
    /// covering both meant the cheap half could not be claimed without claiming the expensive one,
    /// which is how a registry entry ends up lying in whichever direction someone needed.
    FinalLogitSoftcap,
    /// tanh soft-capping on ATTENTION logits, before the softmax (Gemma 2).
    ///
    /// Cannot be folded into q the way the attention SCALE was — tanh is not linear — so it needs
    /// the cap inside every attention kernel rather than one dispatch in front of them.
    AttnLogitSoftcap,
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
    /// V is taken FROM THE K PROJECTION on layers that ship no `attn_v` (Gemma 4 full-attention
    /// layers), with a WEIGHTLESS rms on V. Not "V equals cached K": K then gets its learned norm
    /// and rope, V does not — same projection, different post-processing (gemma4.cpp).
    AltAttnValueFromKey,
    /// Gated DeltaNet recurrent layers (Qwen3.5/3.6 hybrids, Qwen3-Next): a per-head decaying
    /// delta-rule state instead of a KV cache, fed by a causal depthwise conv over fused qkv.
    GatedDeltaNet,
    GatedAttentionOutput,
    MRopeSections,
    GatedSharedExpert,
    /// Dense (shared) FFN and the routed MoE run as PARALLEL branches off attn_out — each with its
    /// own pre/post norms — summed, post-normed once, then added to the residual (Gemma 4 MoE).
    /// Sequential wiring runs and produces plausible text, which is the failure mode worth a flag.
    TwinFfnParallelResidual,
    /// Router logits are computed on a WEIGHTLESS rms of attn_out, scaled by 1/sqrt(d) and a
    /// learned per-dimension vector (`ffn_gate_inp.scale`) — not on the MoE branch's normed input
    /// (Gemma 4's "custom MoE logits calculation").
    RouterInputNormScale,
    /// Expert outputs are multiplied by a LEARNED per-expert scale vector
    /// (`ffn_down_exps.scale`, one value per expert). Distinct from `ExpertWeightsScale`, which is
    /// a metadata CONSTANT — conflating a trained vector with a constant is the Llama-4-norms
    /// class of mistake.
    PerExpertOutputScale,
    /// A self-conditioning FFN at the decoder top (DiffusionGemma `self_cond.*`) feeding the
    /// previous denoising step's state back into the current one.
    SelfConditioning,
    /// A SECOND set of per-layer output scalars for the causal ENCODE pass
    /// (`layer_output_scale_enc`) beside the denoise-pass set — one stack, two modes
    /// (DiffusionGemma). Depends on `LayerOutputScale` being read at all.
    EncDecLayerScale,
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
            Feature::FinalLogitSoftcap => "tanh soft-cap on the final logits",
            Feature::AttnLogitSoftcap => "tanh soft-cap on attention logits, inside the softmax",
            Feature::EmbeddingScale => "token embedding scaled by sqrt(d)",
            Feature::PerLayerRopeBase => "separate rope base for windowed layers",
            Feature::AttentionScaleHparam => "attention scale from hparams, not 1/sqrt(head_dim)",
            Feature::ExpertWeightsScale => "routed-expert weights scaled by a metadata constant",
            Feature::ExpertWeightsNorm => "routed-expert weights renormalized to sum to 1",
            Feature::SparseAttentionIndexer => "sparse attention: an indexer selects top-k keys",
            Feature::AltAttnValueFromKey => "V sourced from the K projection (attn_v absent; weightless V norm)",
            Feature::GatedDeltaNet => "Gated DeltaNet recurrent layers (delta-rule state + conv, no KV cache)",
            Feature::GatedAttentionOutput => "gated attention (per-head q‖gate interleaved in a double-width q projection; attention output × sigmoid(gate))",
            Feature::MRopeSections => "multi-section rope (IMRoPE: rope.dimension_sections split across position channels)",
            Feature::GatedSharedExpert => "shared expert with its OWN sigmoid gate (ffn_gate_inp_shexp scales the shared expert's output per token)",
            Feature::TwinFfnParallelResidual => "dense + MoE FFN as parallel branches with one joining post-norm",
            Feature::RouterInputNormScale => "router input: weightless rms of attn_out times a learned scale vector",
            Feature::PerExpertOutputScale => "learned per-expert output scale vector",
            Feature::SelfConditioning => "self-conditioning FFN at the decoder top",
            Feature::EncDecLayerScale => "separate encoder-pass per-layer output scalars",
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
            // Implemented: Model::logit_softcap, applied by logits_out — one funnel in front of
            // eighteen call sites, so a model that states a cap cannot get uncapped logits from
            // some paths and capped ones from others.
            Feature::FinalLogitSoftcap => true,
            Feature::AttnLogitSoftcap => false,  // needs the cap inside the attention kernels
            Feature::EmbeddingScale => true,     // Model::embed_scale, set from the arch at load
            Feature::PerLayerRopeBase => true,   // BlockWeights::rope_base, from rope.freq_base_swa
            // IMPLEMENTED, and this table did not learn — the SIXTH stale entry, after
            // ExpertWeightsScale, ExpertWeightsNorm, PerLayerRopeFreqs and MlaSplit3dProjections.
            // The loader reads {arch}.attention.scale and the value is applied by PRE-SCALING q by
            // scale*sqrt(head_dim) before attention, so the kernels' own 1/sqrt(head_dim) cancels to
            // exactly the stated scale — one dispatch, no change to seven attention kernels. The
            // comment this replaces ("hardcoded in the kernels") described the reason it could not
            // be done, which stopped being true when the pre-scale landed.
            //
            // A stale FALSE here is not cosmetic: is_supported vetoes on any gap, so this entry was
            // refusing Gemma 3 27B and Gemma 4 for a feature the engine has.
            Feature::AttentionScaleHparam => true,
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
            Feature::SparseAttentionIndexer => false,
            // Gemma 4 MoE / DiffusionGemma (gemmadiff.rs is the CPU reference each will be gated
            // against; none has a GPU caller yet — the reference existing is NOT the feature
            // existing, which is exactly the distinction this table exists to hold).
            Feature::AltAttnValueFromKey => false,     // the loader requires attn_v today
            // CPU-ROUTED and wired: the loader builds each recurrent block's DeltaNetLayerHost,
            // new_cache_for allocates per-layer states, and decode routes those layers through
            // block_decode_deltanet — which calls the SAME deltanet::step the invariant tests
            // bind. --gate-deltanet-load proves load + decode + the seek refusal on a synthetic
            // hybrid. Slow by design (one download+upload per recurrent layer per token); the GPU
            // kernel is the speed work and checks against this path.
            Feature::GatedDeltaNet => true,
            // The two things a REAL Qwen3.5/3.6 full-attention layer still needs. Observed in
            // qwen35moe.cpp: create_tensor_qkv with n_embd_head*n_head*2 (q and its output gate
            // interleaved per head), attention output multiplied by sigmoid(gate); and
            // ggml_rope_multi with sections. Neither is read or applied here yet.
            // Implemented shape-driven: a double-width attn_q is de-interleaved per head at load
            // (qkvsplit::deinterleave_q_gate, unit-tested), the gate half becomes attn_out_gate,
            // and block_decode + block_decode_fused apply attn ∘ sigmoid(gate) before wo — proven
            // EQUAL to each other at 3.6e-7 by --gate-deltanet-load's fused-vs-unfused cross-check
            // (which also caught the post_attention_norm double-load on its first run). Every
            // other block fn refuses a gated layer by name (refuse_attn_gate_where_unapplied).
            Feature::GatedAttentionOutput => true,
            Feature::MRopeSections => false,
            // The loader reads ffn_{gate,up,down}_shexp but NOT ffn_gate_inp_shexp — the shared
            // expert's own gate. qwen35moe.cpp multiplies the shared expert output by
            // sigmoid(ffn_gate_inp_shexp(x)); adding it ungated is quietly wrong output.
            Feature::GatedSharedExpert => false,
            Feature::TwinFfnParallelResidual => false, // block forward runs ONE ffn branch
            Feature::RouterInputNormScale => false,    // router reads the branch's normed input
            Feature::PerExpertOutputScale => false,    // ffn_down_exps.scale is not read
            Feature::SelfConditioning => false,        // self_cond.* tensors are not read
            Feature::EncDecLayerScale => false,        // no pass-dependent scalar plumbing
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
            Feature::FinalLogitSoftcap => Some("Gemma 2 only; implemented — logits_out applies it"),
            Feature::AttnLogitSoftcap => Some("Gemma 2 only; Gemma 3 and 4 default it off"),
            Feature::AttentionScaleHparam => Some("matches the 1/sqrt(head_dim) default on Gemma 3 12B \
and below; differs only on the 27B, which uses 1/sqrt(n_embd/n_head)"),
            Feature::ExpertWeightsScale => Some("GLM-5.2 sets 2.5; ignoring it scales every MoE layer's \
output down by that factor — a quietly worse model, not a broken one"),
            Feature::MlaSplit3dProjections => Some("a LOADING gap, not a maths one — the MLA forward is \
implemented and validated; these checkpoints just store k_b and v_b separately and in 3-D"),
            Feature::SparseAttentionIndexer => Some("glm-dsa needs sparse attention: an indexer selects top-k keys. NOT a correctness blocker: \
dense attention is EXACT at or below index_topk, because selecting the top k of k-or-fewer keys \
is all of them. Above that bound it is an approximation, and at a 1M context it is unaffordable. \
The dense-fallback bargain was llama.cpp's too until mid-2026; its master now SHIPS DSA \
(src/models/glm-dsa.cpp + llama-kv-cache-dsa.h: 32-head lightning indexer, per-layer \
indexer_types, indexer tensors TENSOR_NOT_REQUIRED) — the reference to transcribe when \
long-context efficiency matters here. IndexShare (index_topk_freq 4 → only ~1 layer in 4 \
carries indexer tensors) is handled on our side: unread tensors are simply not fetched"),
            Feature::Mla => Some("wired and validated against the CPU reference, but never yet run on a \
real DeepSeek/GLM checkpoint — the first such load is the test"),
            Feature::GatedDeltaNet => Some("recurrent layers run through the CPU-routed \
block_decode_deltanet (correct-first, like the MoE and MLA bootstraps); deltanet.rs is the \
tested core. Sequential positions only — recurrent state cannot seek, so trie/fleet prefix \
reuse is refused for hybrids"),
            Feature::GatedAttentionOutput => Some("wired: the double-width q is split per head at \
load and decode applies attn * sigmoid(gate) before wo (block_decode and _fused, cross-checked \
against each other; other paths refuse gated layers by name). Never run on a real checkpoint"),
            Feature::MRopeSections => Some("IMRoPE: position is split across rope sections \
(rope.dimension_sections — REQUIRED metadata for qwen35moe, so every converted file carries it). \
For pure-text input with all position components equal, M-RoPE is claimed equivalent to plain 1D \
rope (Qwen2-VL paper) — but the ggml kernel is not vendored here, so the per-dim frequency \
mapping of GGML_ROPE_TYPE_IMROPE is UNVERIFIED; check ggml-cpu's rope against ours on a shared \
input before treating NEOX as sufficient, and do not flip this flag on the equivalence argument \
alone"),
            Feature::GatedSharedExpert => Some("the shared expert path adds its output UNGATED; \
qwen35moe scales it by sigmoid(ffn_gate_inp_shexp(x)) first. One small projection + the existing \
sigmoid_mul_t — unbuilt, and unexercised by the dense-FFN fixture, so it stays a named gap"),
            Feature::AltAttnValueFromKey => Some("STRUCTURAL for the affected layers: without it the \
loader refuses on the missing attn_v; faking V=K without the weightless norm runs and is wrong"),
            Feature::TwinFfnParallelResidual => Some("sequential wiring produces plausible text — the \
CPU reference in gemmadiff pins the parallel join order the gate must match"),
            Feature::RouterInputNormScale => Some("routing on the branch input instead of attn_out \
mis-routes every token quietly; gemmadiff::router_logits_reference is the oracle"),
            Feature::PerExpertOutputScale => Some("ignoring the vector is the GLM-5.2 \
expert_weights_scale bug again, per expert this time — quietly worse, never wrong-looking"),
            Feature::SelfConditioning => Some("diffusion-only; skipping it changes denoise trajectories, \
not shapes — the kind of gap only an end-to-end oracle catches"),
            Feature::EncDecLayerScale => Some("gemmadiff::layer_out_scale REFUSES an encode pass without \
the enc scalar rather than borrowing the denoise one"),
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
                         Feature::GemmaRmsNorm, Feature::AttnLogitSoftcap, Feature::FinalLogitSoftcap, Feature::EmbeddingScale, Feature::PerLayerRopeBase, Feature::AttentionScaleHparam],
        "gemma3" | "gemma3n" => vec![Feature::StandardSwa, Feature::PostNorm, Feature::QkNormPreRope,
                                     Feature::GeGluActivation, Feature::GemmaRmsNorm, Feature::EmbeddingScale, Feature::PerLayerRopeBase, Feature::AttentionScaleHparam],
        // Gemma 4 (src/models/gemma4.cpp): everything gemma3 needs, plus per-layer rope freqs, a
        // per-layer HEAD DIM (key_length_swa vs key_length) and a learned per-layer output scale.
        // AltAttnValueFromKey is BASE gemma4, not just the diffusion variant: wv is
        // TENSOR_NOT_REQUIRED and `Vcur = wv ? mm(wv,cur) : Kcur` — V is the raw k_proj output
        // (then a WEIGHTLESS rms norm) while K separately gets the learned k_norm + rope.
        // Not yet in this list because no registry Feature exists for it: gemma4.cpp's
        // hparams.has_kv(il) — some layers REUSE an earlier layer's KV cache entirely
        // (cross-layer KV sharing). First seen in the Aug 2026 snapshot; add the Feature the
        // moment a gemma4 checkpoint is actually attempted.
        "gemma4" => vec![Feature::StandardSwa, Feature::PostNorm, Feature::QkNormPreRope,
                         Feature::GeGluActivation, Feature::GemmaRmsNorm,
                         Feature::PerLayerRopeFreqs, Feature::PerLayerHeadDim, Feature::LayerOutputScale,
                         Feature::EmbeddingScale, Feature::PerLayerRopeBase, Feature::AttentionScaleHparam,
                         Feature::AltAttnValueFromKey],
        // DiffusionGemma = the Gemma 4 26B-A4B MoE base plus the diffusion pieces. Transcribed
        // from gemma4.cpp (base) + the HF drop's configs and weight census (diffusion). The MoE
        // routing/gather machinery itself is exercised daily (qwen3moe, llada2) so it is not a
        // feature here; what IS listed is every way this stack differs from what block_decode runs.
        "diffusion_gemma" | "diffusiongemma" => vec![
            Feature::StandardSwa, Feature::PostNorm, Feature::QkNormPreRope,
            Feature::GeGluActivation, Feature::GemmaRmsNorm, Feature::FinalLogitSoftcap,
            Feature::PerLayerRopeFreqs, Feature::PerLayerHeadDim, Feature::LayerOutputScale,
            Feature::EmbeddingScale, Feature::PerLayerRopeBase, Feature::AttentionScaleHparam,
            Feature::SharedExpert,
            Feature::AltAttnValueFromKey, Feature::TwinFfnParallelResidual,
            Feature::RouterInputNormScale, Feature::PerExpertOutputScale,
            Feature::SelfConditioning, Feature::EncDecLayerScale],
        // Qwen3.5/3.6 hybrids (qwen35moe.cpp / qwen3next.cpp): 3-in-4 layers are Gated DeltaNet.
        // QkNormPreRope covers the 1-in-4 gated-attention blocks; the deltanet gap is the load
        // blocker and its report points at the CPU oracle.
        "qwen35moe" | "qwen36moe" | "qwen3_5moe" | "qwen3next" =>
            vec![Feature::QkNormPreRope, Feature::SharedExpert, Feature::GatedDeltaNet,
                 Feature::GatedAttentionOutput, Feature::MRopeSections, Feature::GatedSharedExpert],
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
    fn diffusion_gemma_gaps_are_the_eight_structural_ones_not_the_gemma3_base() {
        // The Gemma 3 base (SWA, post-norms, QK-norm, GeGLU, (1+w) norm, embedding scale,
        // per-layer rope base, softcap) is implemented and must NOT appear as a gap — a stale
        // FALSE there would refuse work the engine can do (the PerLayerRopeFreqs lesson). What
        // remains is the per-layer geometry pair, the output scalars (both sets), and the four
        // Gemma-4-MoE/diffusion mechanisms.
        let g = gaps("diffusion_gemma", 128);
        let expect = [Feature::PerLayerHeadDim, Feature::LayerOutputScale,
                      Feature::AltAttnValueFromKey, Feature::TwinFfnParallelResidual,
                      Feature::RouterInputNormScale, Feature::PerExpertOutputScale,
                      Feature::SelfConditioning, Feature::EncDecLayerScale];
        for f in expect { assert!(g.contains(&f), "{f:?} must be a named gap: {g:?}"); }
        assert_eq!(g.len(), expect.len(), "and nothing implemented leaks into the gap list: {g:?}");
        // the report names the arch and every gap, so the loader's refusal is actionable
        let report = gap_report("diffusion_gemma", 128);
        assert_eq!(report.len(), expect.len());
        assert!(report.iter().all(|l| l.contains("diffusion_gemma")));
    }

    #[test]
    fn per_expert_scale_is_not_the_metadata_constant() {
        // ExpertWeightsScale (a constant, implemented) and PerExpertOutputScale (a trained
        // vector, not) collapsing into one entry is how the registry would end up claiming the
        // vector is handled because the constant is — the exact class of the Llama-4 norm bug.
        assert_ne!(Feature::ExpertWeightsScale, Feature::PerExpertOutputScale);
        assert!(Feature::ExpertWeightsScale.implemented());
        assert!(!Feature::PerExpertOutputScale.implemented());
    }

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
        assert!(!Feature::AttnLogitSoftcap.implemented(), "the ATTENTION cap is still not implemented");
        assert!(Feature::FinalLogitSoftcap.implemented(), "the FINAL cap is, via logits_out");
    }

    #[test]
    fn gemma4_needs_four_things_gemma3_does_not() {
        let g3 = required("gemma3", 0);
        let g4 = required("gemma4", 0);
        assert!(g3.contains(&Feature::QkNormPreRope) && g4.contains(&Feature::QkNormPreRope));
        // The fourth is AltAttnValueFromKey, and it is BASE gemma4 per gemma4.cpp (wv is
        // TENSOR_NOT_REQUIRED there) — previously listed only under diffusion_gemma, which
        // would have let a base-gemma4 load fail on "missing attn_v" with no gap named.
        for f in [Feature::PerLayerRopeFreqs, Feature::PerLayerHeadDim, Feature::LayerOutputScale,
                  Feature::AltAttnValueFromKey] {
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
        // the sqrt(d) embedding scale and a separate rope base for windowed layers. The third, the
        // hparam attention scale, matched our 1/sqrt(head_dim) default on every Gemma 3 EXCEPT the
        // 27B — which is why it was listed as a requirement and left unimplemented, so a 27B could
        // not silently load wrong.
        //
        // All three are implemented now, so the test's name is finally literal: gemma3 has NO gaps.
        // The 27B loads with its stated scale, applied by pre-scaling q so the kernels' own
        // 1/sqrt(head_dim) cancels.
        assert!(gaps("gemma3", 0).is_empty(), "gemma3 gaps: {:?}", gaps("gemma3", 0));
        assert!(Feature::PerLayerRopeBase.implemented() && Feature::EmbeddingScale.implemented());
        // Gemma 2 shares those but also soft-caps its attention and final logits, which is unimplemented.
        // Gemma 2 shares those but also soft-caps its attention and final logits, which is
        // unimplemented — and is now the ONLY thing standing between this engine and Gemma 2.
        // Gemma 2 needs BOTH caps and now has one: the final cap is implemented, the attention
        // cap is the single remaining gap for the architecture.
        assert_eq!(gaps("gemma2", 0), vec![Feature::AttnLogitSoftcap]);
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
        // The gap text used to be asserted on the word STRUCTURAL, which encoded a diagnosis that
        // turned out to be wrong (c4-785/787): llama.cpp does not implement DSA either and runs this
        // model dense, and dense attention is EXACT at or below index_topk. What the report must now
        // carry is the true blocker — IndexShare's per-layer asymmetry — and the honest scope of the
        // dense fallback. A guard that pins a superseded word keeps the superseded diagnosis alive.
        let rep = gap_report("glm-dsa", 0).join(" ");
        assert!(rep.contains("IndexShare"), "the gap must name the loader trap: {rep}");
        assert!(rep.to_ascii_lowercase().contains("dense"), "and the dense fallback: {rep}");
        assert!(rep.contains("index_topk"), "and the bound below which dense is exact: {rep}");
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
        assert!(!Feature::AttnLogitSoftcap.implemented(),
                "no attention kernel caps its logits — that is the half still missing");
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
