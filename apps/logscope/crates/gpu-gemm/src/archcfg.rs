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
pub enum Arch { Llama, Qwen2, Qwen3, Qwen35, Qwen4, Llama4, Gemma2, Phi3, Unknown }

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
    pub rope_interleaved: bool,       // llama-style interleaved-pair RoPE
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
pub fn for_arch(arch: &str) -> ArchConfig {
    match arch {
        "" | "llama" => ArchConfig {
            arch: Arch::Llama, prefix: "llama", attn_qkv_bias: false, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true, note: "",
        },
        "qwen2" => ArchConfig {
            arch: Arch::Qwen2, prefix: "qwen2", attn_qkv_bias: true, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true,
            note: "Q/K/V bias add (blk.N.attn_{q,k,v}.bias) loaded + applied in Lin::apply; validated by --gate-qwen2 (correctness). Bit-exact-vs-real-Qwen2 still M2-gated.",
        },
        // Qwen3 drops Qwen2's QKV bias and adds per-head QK-Norm: an RMSNorm (blk.N.attn_q_norm.weight /
        // attn_k_norm.weight, each [head_dim]) applied to Q and K before RoPE. Everything else is llama-shape
        // (GQA, SwiGLU, RMSNorm). Reuses rmsnorm_t over [t*heads, head_dim], so no new kernel.
        "qwen3" => ArchConfig {
            arch: Arch::Qwen3, prefix: "qwen3", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true,
            note: "per-head QK-Norm (attn_q_norm/attn_k_norm) applied before RoPE; correctness via --gate-qwen3. Bit-exact-vs-real-Qwen3 M2-gated.",
        },
        // Qwen3.5 (e.g. Ornith-1.0-9B, general.architecture = "qwen3_5"): same forward-pass shape as Qwen3
        // for our purposes (QK-Norm, no QKV bias, GQA, SwiGLU, RMSNorm). Reads qwen3_5.* metadata.
        "qwen3_5" | "qwen35" => ArchConfig {
            arch: Arch::Qwen35, prefix: "qwen3_5", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, dense_supported: true,
            note: "Qwen3.5 dense (Ornith): QK-Norm before RoPE, qwen3_5.* metadata; correctness via --gate-qwen3. M2-gated for bit-exactness.",
        },
        // Qwen4 (general.architecture = "qwen4"): continues Qwen3.5's per-head QK-Norm base and adds
        // mixture-of-experts — a router (blk.N.ffn_gate_inp.weight) selecting top-k of N expert FFNs
        // (ffn_{gate,up,down}_exps.weight). The DENSE forward (QK-Norm → RoPE → GQA → SwiGLU → RMSNorm) is
        // already wired, so a dense Qwen4 checkpoint (expert_count == 0) loads via the existing path; a MoE
        // checkpoint needs the top-k routing + expert-gather kernel (M2-gated, like the other new kernels).
        "qwen4" => ArchConfig {
            arch: Arch::Qwen4, prefix: "qwen4", attn_qkv_bias: false, attn_qk_norm: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, moe: true, moe_shared_expert: false, interleaved_nope: false,
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
            supported: false, dense_supported: false,
            note: "Llama4 is mixture-of-experts (routed + one shared expert) with interleaved NoPE layers (some attention layers use no positional encoding) and QK-Norm. Needs the MoE routing/expert-gather kernel + a per-layer RoPE/NoPE switch (M2-gated).",
        },
        "gemma" | "gemma2" => ArchConfig {
            arch: Arch::Gemma2, prefix: "gemma2", attn_qkv_bias: false, attn_qk_norm: false, norm: Norm::RmsGemma, act: Act::GeGlu,
            rope_interleaved: true, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: Some(30.0), attn_softcap: Some(50.0), tied_embed_default: true,
            supported: false, dense_supported: false,
            note: "needs geglu activation, (1+w) rms norm, and tanh soft-cap on attn + final logits",
        },
        "phi3" => ArchConfig {
            arch: Arch::Phi3, prefix: "phi3", attn_qkv_bias: false, attn_qk_norm: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, moe: false, moe_shared_expert: false, interleaved_nope: false, logit_softcap: None, attn_softcap: None, tied_embed_default: false,
            supported: false, dense_supported: false,
            note: "fused qkv tensor + partial-rotary RoPE; needs a split + partial-dim rope kernel",
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
    fn llama_is_supported_today() {
        let c = for_arch("llama");
        assert!(c.supported && c.note.is_empty());
        assert_eq!(for_arch(""), c, "missing arch string falls back to llama");
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
        assert!(!c.supported && !c.dense_supported, "inherently MoE + NoPE — no dense forward to fall back to");
        assert!(!c.tied_embed_default);
        // shared-expert tensors present alongside the routed stacks
        let mt = moe_tensors(&c, 0);
        assert!(mt.iter().any(|t| t == "blk.0.ffn_gate_shexp.weight"));
        assert!(mt.iter().any(|t| t == "blk.0.ffn_gate_exps.weight"));
        assert!(c.note.contains("NoPE") && c.note.contains("shared expert"));
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
