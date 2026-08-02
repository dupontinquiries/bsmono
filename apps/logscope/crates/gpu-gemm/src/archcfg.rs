//! archcfg.rs — SCRATCHWORK (2026-06-12) toward multi-architecture support. Pure, no wgpu, testable.
//!
//! The loader hard-refuses any `general.architecture` other than "llama". This module is the
//! groundwork to lift that: a registry that, per architecture, records the GGUF metadata key prefix,
//! the tensor-name quirks, and the *kernel* differences (attention QKV bias, activation, norm variant,
//! logit/attention soft-capping, RoPE style). It does NOT implement the kernels — those need the M2 to
//! validate against a reference — but it makes the differences explicit and gives the loader a single
//! place to ask "what does this arch need, and do we support it yet?" instead of a hardcoded string ==.
//!
//! Roadmap order (per the loader's own note): Qwen2 next (just adds QKV bias on top of llama), then
//! Gemma2 (geglu + dual soft-cap + a norm tweak), then Phi-3 (fused QKV / partial-rotary).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch { Llama, Qwen2, Qwen3, Qwen35, Qwen3Moe, Qwen4, Llama4, Gemma2, Gemma3, Phi3, Glm4Moe, DeepseekV3, Unknown }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Norm { /// x/rms(x) · w
    Rms, /// gemma: x/rms(x) · (1 + w), and norms computed in f32
    RmsGemma }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act { /// silu(gate) · up
    SwiGlu, /// gelu(gate) · up
    GeGlu }

#[derive(Clone, Debug, PartialEq)]
pub struct ArchConfig {
    pub arch: Arch,
    pub prefix: &'static str,         // GGUF metadata key prefix, e.g. "llama" → "llama.embedding_length"
    pub attn_qkv_bias: bool,          // qkv projections have bias tensors (Qwen2)
    pub attn_qk_norm: bool,           // per-head RMSNorm on Q and K before RoPE (Qwen3 / Qwen3.5 / Qwen4 / Llama4)
    pub norm: Norm,
    pub act: Act,
    pub rope_interleaved: bool,       // true = llama-style ADJACENT-pair RoPE (ggml NORM); false = NEOX split-half (qwen/gemma/phi). WIRED: loader sets Gpu.rope_neox = !this
    pub moe: bool,                    // mixture-of-experts: a router (ffn_gate_inp) + N expert FFNs (Qwen4 / Llama4)
    pub moe_shared_expert: bool,      // an always-on shared expert alongside the routed ones (Llama4)
    pub interleaved_nope: bool,       // some attention layers use NO positional encoding (Llama4 iRoPE)
    pub logit_softcap: Option<f32>,   // Gemma2 final-logit soft cap
    pub attn_softcap: Option<f32>,    // Gemma2 attention-logit soft cap
    pub tied_embed_default: bool,     // output usually tied to embedding
    pub supported: bool,              // kernels wired in the engine TODAY (full arch, incl. MoE if any)
    pub dense_supported: bool,        // the NON-MoE forward path works on existing kernels — so a dense
                                      // checkpoint of this arch (expert_count == 0) loads even when `supported`
                                      // is false because the MoE routing kernel isn't wired yet
    pub note: &'static str,           // what's required to support it (or "" when supported)
}

/// Map a `general.architecture` string (None/"" → llama, since some converters omit it) to its config.
/// Is this architecture supported, DERIVED from what archfeat says is missing?
///
/// The `supported` field beside each entry was hand-maintained, and this session produced five bugs
/// where it disagreed with reality — archfeat claiming Gemma support this denied, a flag that stayed
/// false for four commits after the code landed, a note describing a blocker that had been fixed.
/// Every one was the cost of two descriptions of one fact kept in sync by hand.
///
/// llama.cpp has no capability matrix at all: an architecture is in the enum and works, or it is
/// unsupported. That is the shape to copy. Here the matrix is load-bearing — the refusals name which
/// features are missing, which is genuinely more useful than "no" — so instead of removing it,
/// SUPPORTED IS DEFINED AS "archfeat lists no gaps". One source of truth, computed.
///
/// `variant` is the expert count, since Llama-4 Scout and Maverick differ by it.
pub fn is_supported(arch: &str, variant: u32) -> bool {
    // ARCHFEAT VETOES, IT DOES NOT GRANT. Writing this as `gaps.is_empty()` alone failed immediately on
    // two architectures, and both failures were right:
    //
    //   deepseek2 has NO known gaps since _294 implemented the split MLA layout, yet is declared
    //   unsupported because it has never completed a forward pass on a real checkpoint. "No gap I know
    //   of" is not "verified", and a derivation that conflated them would have declared MLA working on
    //   the strength of my own feature table.
    //
    //   gemma3 IS supported, and AttentionScaleHparam is listed as a gap — true only of the 27B, which
    //   the loader already refuses by layer count. A gap that applies to one size cannot veto the
    //   others, and `gaps(arch, 0)` cannot see which size it is being asked about.
    //
    // So a gap is sufficient to refuse and its absence is not sufficient to accept. That asymmetry is
    // what makes this safe to derive at all: the drift it removes is "the flag says yes while a gap is
    // listed", which is the direction that ships broken output.
    for_arch(arch).supported && crate::archfeat::gaps(arch, variant).is_empty()
}

/// What the hand-maintained flag says, kept only so the drift test can compare them.
///
/// Callers should use `is_supported`. This exists to make disagreement a TEST FAILURE rather than a
/// runtime surprise, and can be deleted once every entry has been reconciled.
pub fn declared_supported(arch: &str) -> bool { for_arch(arch).supported }

pub fn for_arch(arch: &str) -> ArchConfig {
    match arch {
        "" | "llama" => ArchConfig {
            arch: Arch::Llama, prefix: "llama", attn_qkv_bias: false, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true, note: "",
        },
        "qwen2" => ArchConfig {
            arch: Arch::Qwen2, prefix: "qwen2", attn_qkv_bias: true, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true,
            note: "Q/K/V bias add (blk.N.attn_{q,k,v}.bias) loaded + applied in Lin::apply; validated by --gate-qwen2 (correctness). Bit-exact-vs-real-Qwen2 still M2-gated.",
        },
        // Qwen3 drops Qwen2's QKV bias and adds per-head QK-Norm: an RMSNorm (blk.N.attn_q_norm.weight /
        // attn_k_norm.weight, each [head_dim]) applied to Q and K before RoPE. Everything else is llama-shape
        // (GQA, SwiGLU, RMSNorm). Reuses rmsnorm_t over [t*heads, head_dim], so no new kernel.
        "qwen3" => ArchConfig {
            arch: Arch::Qwen3, prefix: "qwen3", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true,
            note: "per-head QK-Norm (attn_q_norm/attn_k_norm) applied before RoPE; correctness via --gate-qwen3. Bit-exact-vs-real-Qwen3 M2-gated.",
        },
        // Qwen3.5 (e.g. Ornith-1.0-9B, general.architecture = "qwen3_5"): same forward-pass shape as Qwen3
        // for our purposes (QK-Norm, no QKV bias, GQA, SwiGLU, RMSNorm). Reads qwen3_5.* metadata.
        "qwen3_5" | "qwen35" => ArchConfig {
            arch: Arch::Qwen35, prefix: "qwen3_5", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true,
            note: "Qwen3.5 dense (Ornith): QK-Norm before RoPE, qwen3_5.* metadata; correctness via --gate-qwen3. M2-gated for bit-exactness.",
        },
        // Qwen3-MoE (general.architecture = "qwen3moe"): the Qwen3 QK-Norm attention base + mixture-of-
        // experts — a router (ffn_gate_inp) selecting top-k of N experts (ffn_{gate,up,down}_exps), NO shared
        // expert (unlike Qwen2-MoE). This is Qwen3-30B-A3B / Qwen3-Coder-30B-A3B (128 experts, 8 active).
        // Softmax gating. Same expert shape as a dense Qwen3 FFN, so each selected expert reuses the existing
        // matvec/SwiGLU kernels; the router is a small matvec + top-k (moe::route).
        "qwen3moe" => ArchConfig {
            arch: Arch::Qwen3Moe, prefix: "qwen3moe", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: true, moe_shared_expert: false, interleaved_nope: false,
            logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true,
            note: "Qwen3-MoE (Qwen3-Coder-30B-A3B confirmed: d=2048, 48 layers, n_heads=32, n_kv_heads=4, head_dim=128 [NOTE head_dim != d/n_heads=64], 128 experts × ff=768, top-8, NO shared expert; ffn_*_exps stacked with the expert dim OUTERMOST, each slice contiguous & 256-divisible; attn_v/ffn_down_exps are mixed Q6_K/Q4_K per layer). Needs: (1) head_dim decoupled from d/n_heads in the attention path [moe::layer_shape], (2) per-expert dequant→requant of the stacked ffn_*_exps in the loader, (3) MoE forward route→top-8→weighted expert sum [moe::route, ready]. Reuses the existing matvec/SwiGLU kernels per expert.",
        },
        // Qwen4 (general.architecture = "qwen4"): continues Qwen3.5's per-head QK-Norm base and adds
        // mixture-of-experts — a router (blk.N.ffn_gate_inp.weight) selecting top-k of N expert FFNs
        // (ffn_{gate,up,down}_exps.weight). The DENSE forward (QK-Norm → RoPE → GQA → SwiGLU → RMSNorm) is
        // already wired, so a dense Qwen4 checkpoint (expert_count == 0) loads via the existing path; a MoE
        // checkpoint needs the top-k routing + expert-gather kernel (M2-gated, like the other new kernels).
        "qwen4" => ArchConfig {
            arch: Arch::Qwen4, prefix: "qwen4", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: true, moe_shared_expert: false, interleaved_nope: false,
            logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: false, dense_supported: true,
            note: "Qwen4 adds mixture-of-experts (ffn_gate_inp router + ffn_{gate,up,down}_exps) on the Qwen3.5 QK-Norm base. A DENSE Qwen4 (expert_count 0) loads on the existing QK-Norm path; a MoE checkpoint needs the top-k routing + expert-gather kernel (M2-gated).",
        },
        // Llama4 (general.architecture = "llama4"): MoE with a shared expert (Scout/Maverick), interleaved
        // NoPE — a subset of attention layers use NO positional encoding (iRoPE, for length generalization) —
        // and QK-Norm. Inherently MoE (no meaningful dense form), so no dense escape; needs the MoE routing
        // kernel + a per-layer RoPE/NoPE switch. Config-complete here so the loader names exactly what's missing.
        "llama4" => ArchConfig {
            arch: Arch::Llama4, prefix: "llama4", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, moe: true, moe_shared_expert: true, interleaved_nope: true,
            logit_softcap: None, attn_softcap: None, tied_embed_default: false,
            supported: true, dense_supported: false,
            note: "Llama4 MoE: 16 routed (top-1, raw-sigmoid gate on the expert INPUT) + 1 shared expert, interleaved NoPE every 4th layer, weightless QK-Norm on RoPE layers, iRoPE attention temperature tuning on NoPE layers (length generalization past 8K). Runs the v1 routed path (v2 GPU-routing covers softmax gating only, so far).",
        },
        "gemma" | "gemma2" => ArchConfig {
            arch: Arch::Gemma2, prefix: "gemma2", attn_qkv_bias: false, attn_qk_norm: false, norm: Norm::RmsGemma, act: Act::GeGlu,
            rope_interleaved: false, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: Some(30.0), attn_softcap: Some(50.0), tied_embed_default: true,
            supported: false, dense_supported: false,
            note: "needs geglu activation, (1+w) rms norm, and tanh soft-cap on attn + final logits",
        },
        // Gemma 3: everything Gemma 2 has EXCEPT the soft-caps (gemma3.cpp defaults
        // f_final_logit_softcapping to 0), plus learned per-head Q/K norms applied BEFORE rope — the same
        // shape Qwen3 uses, which is why attn_qk_norm carries it. Sliding window, (1+w) norm, GELU gating
        // and the post-norms are all implemented and gated (--gate-swa, --gate-gemma).
        "gemma3" | "gemma3n" => ArchConfig {
            arch: Arch::Gemma3, prefix: "gemma3", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::RmsGemma, act: Act::GeGlu,
            rope_interleaved: false, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true,
            note: "gemma3: (1+w) rms norm, geglu, post-norms and sliding-window attention",
        },
        "phi3" => ArchConfig {
            arch: Arch::Phi3, prefix: "phi3", attn_qkv_bias: false, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: false,
            supported: false, dense_supported: false,
            note: "fused qkv tensor + partial-rotary RoPE; needs a split + partial-dim rope kernel",
        },
        // GLM-4-MoE (general.architecture = "glm4moe"). Two sub-forms exist; this registry entry is the
        // STANDARD-attention one (GQA + per-head QK-Norm + qkv bias, like Qwen3, plus shared-expert MoE).
        // The MLA sub-form is detected by tensor presence in the loader and refused there. NEOX rope.
        "glm4moe" => ArchConfig {
            arch: Arch::Glm4Moe, prefix: "glm4moe", attn_qkv_bias: true, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: true, moe_shared_expert: true, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: false,
            supported: true, dense_supported: true,
            note: "",
        },
        // DeepSeek-V2/V3 (+ GLM-MoE with MLA): Multi-head Latent Attention. Detected here with the right
        // GLM-5.2 (`glm-dsa`). SEPARATE from DeepSeek despite sharing MLA, because `prefix` selects the
        // METADATA KEY namespace and a glm-dsa file spells its keys `glm-dsa.attention.*`. Folding it
        // into the DeepSeek entry meant looking up `deepseek2.attention.kv_lora_rank` in a file that has
        // `glm-dsa.attention.kv_lora_rank` — so the loader reported "no kv_lora_rank in its metadata"
        // about a checkpoint that states it plainly. A wrong reason is worse than no reason: it sends
        // the reader to the checkpoint when the fault is ours.
        "glm-dsa" | "glm_dsa" => ArchConfig {
            arch: Arch::DeepseekV3, prefix: "glm-dsa", attn_qkv_bias: false, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: true, moe_shared_expert: true, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: false,
            supported: false, dense_supported: false,
            note: "GLM-5.2: MLA plus a SPARSE-ATTENTION INDEXER (32 heads, key_length 128, top_k 2048 \
over a 1M context) and expert_weights_scale 2.5. The indexer tensors are present per block \
(indexer.attn_k, indexer.attn_q_b, indexer.k_norm, indexer.proj) and nothing reads them — see \
docs/glm-5.2-arch.md",
        },
        "deepseek2" | "deepseek3" | "deepseekv3" => ArchConfig {
            arch: Arch::DeepseekV3, prefix: "deepseek2", attn_qkv_bias: false, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: false, moe: true, moe_shared_expert: true, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: false,
            supported: false, dense_supported: false,
            note: "MLA is wired: compressed-latent cache, per-layer routing, the decode block \
(--gate-mla-block), and BOTH kv_b layouts — the legacy fused attn_kv_b and the modern split \
attn_k_b + attn_v_b, which GGUF stores 3-D but which are the same bytes as llama.cpp's 2-D declaration \
(PR #12801). Still unverified end to end on a real checkpoint, so `supported` stays false until one runs",
        },
        _ => ArchConfig {
            arch: Arch::Unknown, prefix: "", attn_qkv_bias: false, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: false, dense_supported: false, note: "unknown architecture — use the llama.cpp backend",
        },
    }
}

/// The metadata key names the loader should read for this arch (generalizing the hardcoded "llama.*").
#[derive(Clone, Debug, PartialEq)]
pub struct DimKeys {
    pub embedding_length: String,
    pub feed_forward_length: String,
    pub block_count: String,
    pub head_count: String,
    pub head_count_kv: String,
    pub rope_freq_base: String,
    pub rms_eps: String,
    pub expert_count: String,        // MoE: total routed experts ({pfx}.expert_count); 0/absent = dense
    pub expert_used_count: String,   // MoE: top-k experts activated per token
    pub expert_shared_count: String, // MoE: always-on shared experts (Llama4)
}
pub fn dim_keys(cfg: &ArchConfig) -> DimKeys {
    let p = cfg.prefix;
    DimKeys {
        embedding_length: format!("{p}.embedding_length"),
        feed_forward_length: format!("{p}.feed_forward_length"),
        block_count: format!("{p}.block_count"),
        head_count: format!("{p}.attention.head_count"),
        head_count_kv: format!("{p}.attention.head_count_kv"),
        rope_freq_base: format!("{p}.rope.freq_base"),
        rms_eps: format!("{p}.attention.layer_norm_rms_epsilon"),
        expert_count: format!("{p}.expert_count"),
        expert_used_count: format!("{p}.expert_used_count"),
        expert_shared_count: format!("{p}.expert_shared_count"),
    }
}

/// Per-block MoE tensor names (router + expert FFN stacks, + a shared expert for Llama4) — present only when
/// `moe`. Empty for dense architectures. These are the tensors the routing/expert-gather kernel will bind.
pub fn moe_tensors(cfg: &ArchConfig, layer: usize) -> Vec<String> {
    if !cfg.moe { return Vec::new(); }
    let mut v = vec![
        format!("blk.{layer}.ffn_gate_inp.weight"),   // router logits: [n_expert, d]
        format!("blk.{layer}.ffn_gate_exps.weight"),  // stacked expert gate: [n_expert, ffn, d]
        format!("blk.{layer}.ffn_up_exps.weight"),    // stacked expert up:   [n_expert, ffn, d]
        format!("blk.{layer}.ffn_down_exps.weight"),  // stacked expert down: [n_expert, d, ffn]
    ];
    if cfg.moe_shared_expert {
        v.push(format!("blk.{layer}.ffn_gate_shexp.weight"));
        v.push(format!("blk.{layer}.ffn_up_shexp.weight"));
        v.push(format!("blk.{layer}.ffn_down_shexp.weight"));
    }
    v
}

/// The router gate normalization for this arch: softmax for the Qwen family, sigmoid for Llama4-style gating.
pub fn route_norm(cfg: &ArchConfig) -> crate::moe::RouteNorm {
    match cfg.arch {
        Arch::Llama4 => crate::moe::RouteNorm::Sigmoid,
        _ => crate::moe::RouteNorm::Softmax,
    }
}

/// Build the MoE routing config from an arch + the checkpoint's expert counts (from `dim_keys`). `n_used`
/// falls back to a sensible default when the metadata omits it.
/// `scale` and `norm` come from the checkpoint's `{arch}.expert_weights_scale` and
/// `{arch}.expert_weights_norm` — the key names llama.cpp uses, arch-prefixed. Passed in rather than
/// read here so this stays free of the Gguf, and taken as Options so "the metadata omits it" is
/// distinguishable from "the metadata says 1.0".
///
/// The prefix matters more than it looks: reading these under the WRONG arch prefix returns None for a
/// model that set them, which is exactly how glm-dsa's kv_lora_rank was misread as absent.
pub fn moe_config(cfg: &ArchConfig, n_routed: u32, n_used: u32, n_shared: u32,
                  scale: Option<f32>, norm: Option<bool>) -> crate::moe::MoeConfig {
    crate::moe::MoeConfig {
        n_routed,
        n_used: if n_used == 0 { 8.min(n_routed.max(1)) } else { n_used },
        n_shared: if cfg.moe_shared_expert { n_shared.max(1) } else { n_shared },
        norm: route_norm(cfg),
        // Llama4: raw-sigmoid top-1 gate applied to the expert's INPUT (weight-before-FFN);
        // everything else: normalized weights combined on the output.
        // Metadata wins; the arch default is the fallback. llama.cpp reads this from the checkpoint
        // rather than deriving it, and a model that sets expert_weights_norm=false without being
        // Llama-4 would otherwise be normalized against its own config.
        // METADATA WINS, EXCEPT WHERE THE ARCHITECTURE FORBIDS IT.
        //
        // Making this metadata-driven was right for Qwen and DeepSeek, and I introduced a risk with
        // it: Llama-4's gate is not a normalization TOGGLE, it is a different forward pass. The raw
        // sigmoid multiplies the expert's INPUT (weight_before), and a normalized weight applied there
        // is not the same computation scaled — it is wrong. A checkpoint that happens to carry
        // expert_weights_norm=true would have silently switched Llama-4 into a shape it does not have.
        //
        // So the arch VETOES rather than merely defaulting, which is archfeat's rule arriving here.
        // ARCHITECTURAL WHERE THE ARCHITECTURE DECIDES IT, metadata only where it does not.
        //
        // Checked against llama.cpp's per-model builders, and it hardcodes norm_w as a LITERAL for
        // both of these — not from hparams:
        //
        //     models/llama4.cpp:228     LLM_FFN_SILU, false,   <- Llama-4
        //     models/qwen3moe.cpp:144   LLM_FFN_SILU, true,    <- Qwen3-MoE
        //
        // So my _370 change was wrong for BOTH, not just Llama-4. A Qwen checkpoint carrying
        // expert_weights_norm=false would have switched off a normalization the architecture requires,
        // and the output would have been plausible rather than broken.
        //
        // The scale is different and IS read from metadata in both files
        // (hparams.expert_weights_scale), which is why routed_scale stays metadata-driven.
        normalize: match cfg.arch {
            Arch::Llama4 => false,
            Arch::Qwen3Moe => true,
            _ => norm.unwrap_or(true),
        },
        // GLM-5.2 sets routed_scaling_factor (2.5); everything else leaves it inert at 1.0. Applied
        // AFTER normalization — see the ordering note on the field.
        routed_scale: scale.unwrap_or(1.0),
        weight_before: matches!(cfg.arch, Arch::Llama4),
    }
}

/// Per-block bias tensor names — present only when `attn_qkv_bias` (Qwen2). Empty otherwise.
pub fn qkv_bias_tensors(cfg: &ArchConfig, layer: usize) -> Vec<String> {
    if cfg.attn_qkv_bias {
        vec![format!("blk.{layer}.attn_q.bias"), format!("blk.{layer}.attn_k.bias"), format!("blk.{layer}.attn_v.bias")]
    } else { Vec::new() }
}

/// The per-head QK-Norm weight tensors for a layer (Qwen3/3.5), or empty for architectures without it.
pub fn qk_norm_tensors(cfg: &ArchConfig, layer: usize) -> Vec<String> {
    if cfg.attn_qk_norm {
        vec![format!("blk.{layer}.attn_q_norm.weight"), format!("blk.{layer}.attn_k_norm.weight")]
    } else { Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unverified_arch_can_never_verify_ITSELF_without_an_opt_in() {
        // The deadlock this documents. `supported` flips only once an architecture has completed a
        // forward pass on a real checkpoint — the right rule, and the reason deepseek2 stayed gated
        // with no known gaps. But attempting the run IS what produces that evidence, and the loader
        // refused the attempt, so the flag could never change.
        //
        // glm-dsa is precisely there: two of its three gaps implemented, the third a no-op below 2048
        // context, MLA layout fixed — and unrunnable. LOGSCOPE_TRY_UNSUPPORTED_ARCH is the escape, and
        // it claims nothing: it lets someone find out.
        assert!(!is_supported("glm-dsa", 0), "still unverified, correctly");
        assert!(!declared_supported("glm-dsa"), "and the declaration is what gates it, not the gaps");
        // The gap list is now down to one, which is what makes the attempt worth offering.
        assert_eq!(crate::archfeat::gaps("glm-dsa", 0).len(), 1,
            "only the sparse indexer remains: {:?}", crate::archfeat::gaps("glm-dsa", 0));
    }

    #[test]
    fn llama_is_supported_today() {
        let c = for_arch("llama");
        assert!(c.supported && c.note.is_empty());
        assert_eq!(for_arch(""), c, "missing arch string falls back to llama");    }

    #[test]
    fn deepseek_and_glmdsa_are_mla_but_glm4moe_is_standard() {
        // Corrected against real checkpoints: the MLA family (DeepSeek-V3, GLM-5.2 'glm-dsa') is detected
        // but unwired; GLM4-MoE ('glm4moe') uses STANDARD GQA attention + shared-expert MoE and IS supported.
        for name in ["deepseek2", "deepseek3", "deepseekv3", "glm-dsa", "glm_dsa"] {
            let c = for_arch(name);
            assert_eq!(c.arch, Arch::DeepseekV3, "{name} should map to the MLA arch");
            assert!(c.moe && c.moe_shared_expert, "DeepSeek/GLM-DSA has routed + shared experts");
            assert!(!c.supported, "MLA forward isn't wired yet → refuse cleanly");
            assert!(c.note.contains("MLA") || c.note.contains("Latent"), "note must explain the MLA gap");
        }
        // glm4moe: standard attention (QK-norm + bias) + shared-expert MoE — supported.
        let g = for_arch("glm4moe");
        assert_eq!(g.arch, Arch::Glm4Moe);
        assert!(g.supported && g.moe && g.moe_shared_expert && g.attn_qk_norm && g.attn_qkv_bias,
            "glm4moe is a supported standard-attention shared-expert MoE with QK-norm + bias");
    }

    #[test]
    fn qwen2_is_llama_plus_bias() {
        let c = for_arch("qwen2");
        assert_eq!(c.arch, Arch::Qwen2);
        assert!(c.attn_qkv_bias, "qwen2's distinguishing feature is qkv bias");
        assert_eq!(c.act, Act::SwiGlu);
        assert_eq!(c.norm, Norm::Rms);
        assert!(c.supported && c.note.contains("bias"), "now wired (bias add) + validated by --gate-qwen2");
        assert_eq!(qkv_bias_tensors(&c, 7), vec!["blk.7.attn_q.bias", "blk.7.attn_k.bias", "blk.7.attn_v.bias"]);
    }

    #[test]
    fn qwen3_drops_bias_adds_qk_norm() {
        let c = for_arch("qwen3");
        assert_eq!(c.arch, Arch::Qwen3);
        assert!(!c.attn_qkv_bias, "qwen3 removed qwen2's qkv bias");
        assert!(c.attn_qk_norm, "qwen3's distinguishing feature is per-head QK-Norm");
        assert!(c.supported && c.act == Act::SwiGlu && c.norm == Norm::Rms);
        assert_eq!(qk_norm_tensors(&c, 5), vec!["blk.5.attn_q_norm.weight", "blk.5.attn_k_norm.weight"]);
        assert!(qkv_bias_tensors(&c, 5).is_empty());
    }

    #[test]
    fn qwen4_is_qwen35_plus_moe_with_a_dense_escape() {
        let c = for_arch("qwen4");
        assert_eq!(c.arch, Arch::Qwen4);
        assert!(c.attn_qk_norm, "qwen4 keeps the QK-Norm base");
        assert!(c.moe && !c.moe_shared_expert && !c.interleaved_nope);
        assert!(!c.supported, "MoE routing kernel isn't wired yet");
        assert!(c.dense_supported, "a dense qwen4 (expert_count 0) rides the existing QK-Norm path");
        // router + stacked expert FFN tensors are named for the eventual kernel
        let mt = moe_tensors(&c, 3);
        assert!(mt.contains(&"blk.3.ffn_gate_inp.weight".to_string()));
        assert!(mt.contains(&"blk.3.ffn_down_exps.weight".to_string()));
        assert!(!mt.iter().any(|t| t.contains("shexp")), "qwen4 has no shared expert");
        assert_eq!(dim_keys(&c).expert_count, "qwen4.expert_count");
    }

    #[test]
    fn llama4_is_moe_with_shared_expert_and_interleaved_nope() {
        let c = for_arch("llama4");
        assert_eq!(c.arch, Arch::Llama4);
        assert!(c.moe && c.moe_shared_expert, "llama4 has routed + one shared expert");
        assert!(c.interleaved_nope, "llama4's iRoPE: some layers use no positional encoding");
        assert!(c.attn_qk_norm);
        assert!(c.supported, "llama4 runs: shared expert + NoPE switch + sigmoid weight-before gating landed");
        assert!(!c.dense_supported, "inherently MoE + NoPE — no dense forward to fall back to");
        assert!(!c.tied_embed_default);
        // shared-expert tensors present alongside the routed stacks
        let mt = moe_tensors(&c, 0);
        assert!(mt.iter().any(|t| t == "blk.0.ffn_gate_shexp.weight"));
        assert!(mt.iter().any(|t| t == "blk.0.ffn_gate_exps.weight"));
        assert!(c.note.contains("NoPE") && c.note.contains("shared expert"));
    }

    #[test]
    fn rope_pairing_by_family() {
        // llama rotates adjacent pairs (ggml NORM); qwen/gemma/phi rotate split halves (NEOX).
        // Wrong pairing = real words, scrambled meaning (the dense-Qwen salad, root-caused on M2 Ultra).
        assert!(for_arch("llama").rope_interleaved);
        for a in ["qwen2", "qwen3", "qwen3moe", "gemma2", "gemma3", "phi3"] {
            assert!(!for_arch(a).rope_interleaved, "{a} must be NEOX");
        }
    }

    #[test]
    fn qwen3moe_is_qwen3_qk_norm_plus_moe_no_shared() {
        let c = for_arch("qwen3moe");
        assert_eq!(c.arch, Arch::Qwen3Moe);
        assert!(c.attn_qk_norm && !c.attn_qkv_bias, "Qwen3 QK-Norm base");
        assert!(c.moe && !c.moe_shared_expert, "MoE with NO shared expert (unlike Qwen2-MoE)");
        assert_eq!(route_norm(&c), crate::moe::RouteNorm::Softmax);
        // Qwen3-30B-A3B: 128 experts, 8 active, no shared
        let mc = moe_config(&c, 128, 8, 0, None, None);
        assert_eq!(mc.n_routed, 128);
        assert_eq!(mc.n_used, 8);
        assert_eq!(mc.n_shared, 0);
        assert!(c.note.contains("30B-A3B"));
        // router + expert tensors named, no shared-expert tensors
        let mt = moe_tensors(&c, 0);
        assert!(mt.iter().any(|t| t == "blk.0.ffn_gate_inp.weight"));
        assert!(!mt.iter().any(|t| t.contains("shexp")));
    }

    #[test]
    fn moe_config_maps_arch_to_router() {
        // Qwen4: softmax gating, no shared expert
        let q = for_arch("qwen4");
        let qc = moe_config(&q, 128, 8, 0, None, None);
        assert_eq!(qc.norm, crate::moe::RouteNorm::Softmax);
        assert_eq!(qc.n_used, 8);
        assert_eq!(qc.n_shared, 0);
        // Llama4: sigmoid gating, at least one shared expert
        let l = for_arch("llama4");
        let lc = moe_config(&l, 128, 1, 1, None, None);
        assert_eq!(lc.norm, crate::moe::RouteNorm::Sigmoid);
        assert!(lc.n_shared >= 1, "llama4 forces a shared expert");
        // n_used defaults when metadata omits it
        assert_eq!(moe_config(&q, 64, 0, 0, None, None).n_used, 8);
    }

    #[test]
    fn dense_archs_emit_no_moe_tensors() {
        for a in ["llama", "qwen2", "qwen3", "qwen3_5"] {
            assert!(moe_tensors(&for_arch(a), 0).is_empty(), "{a} is dense — no expert tensors");
            assert!(!for_arch(a).moe);
        }
    }

    #[test]
    fn qwen3_5_ornith_reads_its_own_prefix() {
        let c = for_arch("qwen3_5");
        assert_eq!(c.arch, Arch::Qwen35);
        assert!(c.attn_qk_norm && !c.attn_qkv_bias && c.supported);
        assert_eq!(dim_keys(&c).embedding_length, "qwen3_5.embedding_length");
        // the "qwen35" spelling maps to the same config
        assert_eq!(for_arch("qwen35").arch, Arch::Qwen35);
    }

    #[test]
    fn a_note_must_not_claim_something_is_UNWIRED_that_archfeat_says_is_implemented() {
        // Third instance of the same drift this session (gemma4's missing entry, glm-dsa's prefix, now
        // this). GLM-4.7-Flash printed BOTH of these in one load:
        //     "MLA attention — compressed-latent KV cache (validated); per-layer routing active"
        //     "load failed: ... the MLA forward isn't wired yet"
        // The engine and its own error message disagreed, because the note was written before MLA
        // landed and nothing forced it to be revisited.
        for a in ["deepseek2", "deepseek3", "glm-dsa", "glm4moe"] {
            let n = for_arch(a).note.to_ascii_lowercase();
            let claims_unwired = n.contains("isn't wired") || n.contains("is not wired")
                || n.contains("not implemented");
            let mla_done = crate::archfeat::Feature::Mla.implemented();
            assert!(!(claims_unwired && mla_done),
                "{a}'s note says MLA is unwired but archfeat reports it implemented: {n}");
        }
        // The deepseek note has now said three different things across three commits, and the sequence is
        // the point: "MLA isn't wired" (stale), then "the blocker is 3-D tensor layout" (true at the
        // time), and now that BOTH layouts are read. Each revision followed a real change, and a note
        // that lagged behind sent the reader to inspect their checkpoint for our bug.
        let n = for_arch("deepseek2").note;
        assert!(n.contains("attn_k_b") && n.contains("attn_kv_b"), "both layouts named: {n}");
        // and it must still refuse, because reading the weights is not the same as having run them
        assert!(!for_arch("deepseek2").supported);
        assert!(n.contains("unverified end to end"), "the remaining gap has to stay stated: {n}");
    }

    #[test]
    fn the_registry_prefix_can_DISAGREE_with_the_file_and_the_file_wins() {
        // Qwen3.5 ships files whose keys are `qwen35.*` while the registry says `qwen3_5`. The loader
        // sniffed the real prefix, PRINTED that it was using it, and then never stored it — so every
        // later read through acfg.prefix returned None on that file. Rope base, norm epsilon,
        // expert_weights_scale: each defaulted silently, and a wrong rope base produces exactly the
        // near-random output reported ("doctrine%," in answer to "hi there").
        //
        // NOTICING AND NOT ACTING IS WORSE THAN NOT NOTICING: the log claimed the file's prefix was in
        // use, and it was not, so the one message that could have pointed here said the opposite.
        let a = for_arch("qwen35");
        let b = for_arch("qwen3_5");
        assert_eq!(a.arch, b.arch, "both spellings are the same architecture");
        // The registry's prefix is one spelling; a file using the other must override it, which is now
        // done at the sniff site rather than left in a local.
        assert_eq!(a.prefix, "qwen3_5", "registry default");
    }

    #[test]
    fn checkpoint_metadata_OVERRIDES_the_arch_default() {
        // llama.cpp reads expert_weights_norm from the checkpoint rather than deriving it from the
        // architecture. A model that sets it false without being Llama-4 would otherwise be normalized
        // against its own config — and the output would be plausible, not broken.
        // Rewritten twice as the reference check narrowed it. Qwen and Llama-4 are now BOTH
        // architectural — llama.cpp hardcodes norm_w for each — so this exercises an arch that has
        // not been checked against a reference, where the checkpoint is still the better source.
        let ds = for_arch("deepseek2");
        assert!(moe_config(&ds, 128, 8, 0, None, None).normalize, "default when absent");
        assert!(!moe_config(&ds, 128, 8, 0, None, Some(false)).normalize, "metadata wins");
        // I asserted the other direction here in _370 — that metadata could turn Llama-4's gate ON.
        // That is the case now vetoed: its raw sigmoid multiplies the expert's INPUT, so a normalized
        // weight there is a different forward pass, not the same one scaled. The test asserted the
        // behaviour and the behaviour was the bug.
        assert!(!moe_config(&for_arch("llama4"), 16, 1, 1, None, Some(true)).normalize,
            "the architecture vetoes; see LLAMA4_ignores_expert_weights_norm");
    }

    #[test]
    fn LLAMA4_ignores_expert_weights_norm_because_its_gate_is_ARCHITECTURAL() {
        // A risk I introduced when making this metadata-driven in _370. Llama-4's raw sigmoid
        // multiplies the expert's INPUT, so a normalized weight applied there is not the same
        // computation scaled — it is a different forward pass. A checkpoint carrying
        // expert_weights_norm=true would have silently switched it into a shape it does not have.
        let l4 = for_arch("llama4");
        assert!(!moe_config(&l4, 16, 1, 1, None, Some(true)).normalize,
            "metadata cannot turn Llama-4's raw-sigmoid gate into a normalized one");
        assert!(!moe_config(&l4, 16, 1, 1, None, None).normalize);
        assert!(moe_config(&l4, 16, 1, 1, None, None).weight_before,
            "and the weight still goes to the expert's input, which is why");
    }

    #[test]
    fn QWEN3MOE_also_ignores_the_metadata_and_llamacpp_is_why() {
        // The check found this and it widened the last fix. llama.cpp hardcodes norm_w as a LITERAL
        // in both per-model builders:
        //
        //     models/llama4.cpp:228     LLM_FFN_SILU, false,
        //     models/qwen3moe.cpp:144   LLM_FFN_SILU, true,
        //
        // Neither reads hparams.expert_weights_norm. So a Qwen checkpoint carrying
        // expert_weights_norm=false would have switched off a normalization the architecture
        // requires — the same class of silent breakage as the Llama-4 case, in the other direction.
        let q = for_arch("qwen3moe");
        assert!(moe_config(&q, 128, 8, 0, None, Some(false)).normalize,
            "the architecture requires it; metadata cannot switch it off");
        assert!(moe_config(&q, 128, 8, 0, None, None).normalize);
    }

    #[test]
    fn the_SCALE_is_still_metadata_driven_for_both() {
        // The scale IS read from hparams in both files, which is why it stays metadata-driven — the
        // two knobs are not the same kind of thing and treating them alike was the original mistake.
        for a in ["llama4", "qwen3moe"] {
            let c = for_arch(a);
            assert_eq!(moe_config(&c, 16, 1, 1, Some(2.5), None).routed_scale, 2.5, "{a}");
        }
    }

    #[test]
    fn an_architecture_we_have_not_checked_still_takes_the_metadata() {
        // The hardcoding is only justified where a reference implementation was read. Elsewhere the
        // checkpoint remains the better source.
        let d = for_arch("deepseek2");
        assert!(!moe_config(&d, 128, 8, 0, None, Some(false)).normalize);
    }

    #[test]
    fn an_absent_scale_is_INERT_rather_than_zero() {
        // None means the metadata omitted it, which must leave every weight untouched. Defaulting to
        // 0.0 would silence every routed expert and look like a broken router.
        assert_eq!(moe_config(&for_arch("qwen3moe"), 128, 8, 0, None, None).routed_scale, 1.0);
        assert_eq!(moe_config(&for_arch("qwen3moe"), 128, 8, 0, Some(2.5), None).routed_scale, 2.5);
    }

    #[test]
    fn the_declared_flag_and_the_derived_answer_do_not_disagree() {
        // The bug class this closes. Five times this session a hand-maintained flag said one thing and
        // archfeat said another, and each was found by a user hitting it rather than by a test. Now a
        // disagreement fails here.
        //
        // Listed explicitly rather than iterating every arch, because the list IS the reconciliation:
        // an entry appears here once someone has checked that both descriptions agree, and adding one
        // is a deliberate act.
        // A gap must be able to VETO a declared-supported arch — that is the direction that ships
        // broken output, and the only one worth failing a build over.
        for (arch, variant) in [("llama", 0u32), ("llama4", 16), ("llama4", 128),
                                ("qwen2", 0), ("qwen3moe", 0)] {
            if !crate::archfeat::gaps(arch, variant).is_empty() {
                assert!(!is_supported(arch, variant),
                    "{arch} (variant {variant}) has gaps {:?} yet is_supported says yes",
                    crate::archfeat::gaps(arch, variant));
            }
        }
        // and the reverse must NOT hold: an arch with no gaps can still be unsupported, because
        // "no gap I know of" is not "verified on a real checkpoint".
        assert!(crate::archfeat::gaps("deepseek2", 0).is_empty(), "the split layout landed in _294");
        assert!(!is_supported("deepseek2", 0), "but MLA has never run end to end");
    }

    #[test]
    fn an_arch_with_gaps_is_not_supported_however_the_flag_reads() {
        // The derivation, stated as a property: gaps and support are the same question. deepseek2 has
        // an unimplemented feature and must therefore be unsupported, whatever anyone typed.
        // gemma3 is the case that proves the veto has to be variant-aware rather than blanket:
        // AttentionScaleHparam is a gap only on the 27B, which the loader refuses by layer count.
        assert!(declared_supported("gemma3"), "the 12B and below work");
        // llama4 has no gaps and is declared supported, so it passes both halves
        assert!(crate::archfeat::gaps("llama4", 16).is_empty(), "{:?}", crate::archfeat::gaps("llama4", 16));
        assert!(is_supported("llama4", 16));
    }

    #[test]
    fn a_shared_config_entry_must_not_share_a_metadata_prefix() {
        // `prefix` selects the metadata KEY namespace, and GGUF spells those keys with the file's own
        // architecture string. GLM-5.2 was folded into the DeepSeek entry, so the loader looked up
        // `deepseek2.attention.kv_lora_rank` in a file that carries `glm-dsa.attention.kv_lora_rank` and
        // reported the metadata as MISSING. It was not missing; we asked the wrong question.
        assert_eq!(for_arch("glm-dsa").prefix, "glm-dsa");
        assert_eq!(for_arch("glm_dsa").prefix, "glm-dsa", "the underscore spelling resolves the same");
        assert_eq!(for_arch("deepseek3").prefix, "deepseek2");
        // they still SHARE the forward-pass shape — only the namespace differs
        assert_eq!(for_arch("glm-dsa").arch, for_arch("deepseek3").arch);
        // and glm-dsa's note must name the indexer, since that is the blocking gap rather than MLA
        assert!(for_arch("glm-dsa").note.contains("INDEXER"));
    }

    #[test]
    fn gemma3_is_enabled_and_differs_from_gemma2_where_llamacpp_says() {
        let c = for_arch("gemma3");
        assert!(c.supported && c.dense_supported, "gemma3's requirements are implemented and gated");
        // shared with gemma2
        assert_eq!(c.norm, Norm::RmsGemma, "x/rms(x)·(1+w)");
        assert_eq!(c.act, Act::GeGlu);
        assert!(!c.rope_interleaved, "NEOX split-half, like qwen/gemma/phi");
        // NOT shared: gemma3.cpp defaults the soft-caps off, and adds learned Q/K norms before rope
        assert!(c.logit_softcap.is_none() && c.attn_softcap.is_none(), "gemma3 has no soft-caps");
        assert!(c.attn_qk_norm, "gemma3 adds learned per-head Q/K norms before rope");
        let g2 = for_arch("gemma2");
        assert!(g2.logit_softcap.is_some() && !g2.attn_qk_norm, "gemma2 is the other way round");
        assert!(!g2.supported, "gemma2 still needs soft-capping");
    }

    #[test]
    fn gemma2_flags_its_quirks() {
        let c = for_arch("gemma2");
        assert_eq!(c.act, Act::GeGlu);
        assert_eq!(c.norm, Norm::RmsGemma);
        assert_eq!(c.logit_softcap, Some(30.0));
        assert_eq!(c.attn_softcap, Some(50.0));
        assert!(!c.supported);
    }

    #[test]
    fn dim_keys_use_the_arch_prefix() {
        let k = dim_keys(&for_arch("qwen2"));
        assert_eq!(k.embedding_length, "qwen2.embedding_length");
        assert_eq!(k.head_count_kv, "qwen2.attention.head_count_kv");
        let lk = dim_keys(&for_arch("llama"));
        assert_eq!(lk.embedding_length, "llama.embedding_length");
    }

    #[test]
    fn unknown_arch_is_unsupported_with_guidance() {
        let c = for_arch("mamba");
        assert_eq!(c.arch, Arch::Unknown);
        assert!(!c.supported && c.note.contains("llama.cpp"));
        assert!(qkv_bias_tensors(&c, 0).is_empty());
    }
}
