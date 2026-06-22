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
pub enum Arch { Llama, Qwen2, Gemma2, Phi3, Unknown }

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
    pub norm: Norm,
    pub act: Act,
    pub rope_interleaved: bool,       // llama-style interleaved-pair RoPE
    pub logit_softcap: Option<f32>,   // Gemma2 final-logit soft cap
    pub attn_softcap: Option<f32>,    // Gemma2 attention-logit soft cap
    pub tied_embed_default: bool,     // output usually tied to embedding
    pub supported: bool,              // kernels wired in the engine TODAY
    pub note: &'static str,           // what's required to support it (or "" when supported)
}

/// Map a `general.architecture` string (None/"" → llama, since some converters omit it) to its config.
pub fn for_arch(arch: &str) -> ArchConfig {
    match arch {
        "" | "llama" => ArchConfig {
            arch: Arch::Llama, prefix: "llama", attn_qkv_bias: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true, note: "",
        },
        "qwen2" => ArchConfig {
            arch: Arch::Qwen2, prefix: "qwen2", attn_qkv_bias: true, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: true,
            note: "Q/K/V bias add (blk.N.attn_{q,k,v}.bias) loaded + applied in Lin::apply; validated by --gate-qwen2 (correctness). Bit-exact-vs-real-Qwen2 still M2-gated.",
        },
        "gemma" | "gemma2" => ArchConfig {
            arch: Arch::Gemma2, prefix: "gemma2", attn_qkv_bias: false, norm: Norm::RmsGemma, act: Act::GeGlu,
            rope_interleaved: true, logit_softcap: Some(30.0), attn_softcap: Some(50.0), tied_embed_default: true,
            supported: false,
            note: "needs geglu activation, (1+w) rms norm, and tanh soft-cap on attn + final logits",
        },
        "phi3" => ArchConfig {
            arch: Arch::Phi3, prefix: "phi3", attn_qkv_bias: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, logit_softcap: None, attn_softcap: None, tied_embed_default: false,
            supported: false,
            note: "fused qkv tensor + partial-rotary RoPE; needs a split + partial-dim rope kernel",
        },
        _ => ArchConfig {
            arch: Arch::Unknown, prefix: "", attn_qkv_bias: false, norm: Norm::Rms, act: Act::SwiGlu,
            rope_interleaved: true, logit_softcap: None, attn_softcap: None, tied_embed_default: true,
            supported: false, note: "unknown architecture — use the llama.cpp backend",
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
    }
}

/// Per-block bias tensor names — present only when `attn_qkv_bias` (Qwen2). Empty otherwise.
pub fn qkv_bias_tensors(cfg: &ArchConfig, layer: usize) -> Vec<String> {
    if cfg.attn_qkv_bias {
        vec![format!("blk.{layer}.attn_q.bias"), format!("blk.{layer}.attn_k.bias"), format!("blk.{layer}.attn_v.bias")]
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
