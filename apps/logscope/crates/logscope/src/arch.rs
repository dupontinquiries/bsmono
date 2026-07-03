//! arch.rs — model architecture detection & capability model.
//!
//! Reads a GGUF model's `general.architecture` plus its hyperparameters and produces a normalized
//! [`ArchInfo`] the engine can act on — including the structural features that separate Qwen and DeepSeek
//! from plain llama: grouped-query attention (GQA), mixture-of-experts FFN (MoE), DeepSeek's multi-head
//! latent attention (MLA), QK-norm (Qwen3), and RoPE scaling. It then gives an honest verdict, via
//! [`EngineCaps`], on whether the current engine has the kernels to run the model or which features are
//! still missing — so "we support Qwen/DeepSeek" means something checkable rather than aspirational.
//!
//! The GGUF read is the seam (the loader fills [`Hparams`]); the classification and the support check are
//! pure and unit-tested. This complements the engine's per-arch kernel wiring (gpu-gemm `archcfg`).
#![allow(dead_code)]

/// Model family, from `general.architecture`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family { Llama, Qwen2, Qwen3, Gemma2, Phi3, DeepSeek2, Mixtral, Unknown }
impl Family {
    pub fn from_arch(s: &str) -> Family {
        match s.trim().to_lowercase().as_str() {
            "" | "llama" => Family::Llama,
            "qwen2" => Family::Qwen2,
            "qwen3" => Family::Qwen3,
            "gemma" | "gemma2" => Family::Gemma2,
            "phi3" => Family::Phi3,
            "deepseek2" | "deepseek-v2" | "deepseek_v2" | "deepseek3" | "deepseek-v3" => Family::DeepSeek2,
            "mixtral" => Family::Mixtral,
            _ => Family::Unknown,
        }
    }
}

/// Raw hyperparameters read from the GGUF metadata (the loader fills these; defaults mean "absent").
#[derive(Clone, Debug, Default)]
pub struct Hparams {
    pub n_layer: u32,
    pub n_head: u32,
    pub n_head_kv: u32,       // < n_head ⇒ grouped-query attention
    pub n_embd: u32,
    pub n_ff: u32,
    pub n_expert: u32,        // > 0 ⇒ mixture of experts
    pub n_expert_used: u32,   // active experts per token
    pub rope_freq_base: f32,
    pub rope_scale: f32,      // > 1.0 (or != 1.0) ⇒ context-extension scaling
    pub kv_lora_rank: u32,    // > 0 ⇒ DeepSeek multi-head latent attention (compressed KV)
    pub q_lora_rank: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttnKind {
    Mha,                                   // full multi-head
    Gqa { groups: u32 },                   // grouped-query (n_head / n_head_kv heads share each KV)
    Mla { kv_lora_rank: u32, q_lora_rank: u32 }, // DeepSeek latent attention
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FfnKind { Dense, Moe { experts: u32, used: u32 } }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RopeKind { None, Standard, Scaled }

/// A non-llama structural feature a model needs the engine to implement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature { Gqa, Moe, Mla, QkNorm, RopeScaling, AttnQkvBias }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Support { Full, Partial(Vec<Feature>), Unsupported(Vec<Feature>) }
impl Support { pub fn is_runnable(&self) -> bool { matches!(self, Support::Full) } }

/// The normalized view of a model's architecture.
#[derive(Clone, Debug)]
pub struct ArchInfo {
    pub family: Family,
    pub attention: AttnKind,
    pub ffn: FfnKind,
    pub qk_norm: bool,
    pub rope: RopeKind,
    pub features: Vec<Feature>, // everything beyond plain llama this model uses
}

/// Classify a model from its architecture string and hyperparameters.
pub fn detect(arch: &str, hp: &Hparams) -> ArchInfo {
    let family = Family::from_arch(arch);

    let attention = if hp.kv_lora_rank > 0 {
        AttnKind::Mla { kv_lora_rank: hp.kv_lora_rank, q_lora_rank: hp.q_lora_rank }
    } else if hp.n_head_kv > 0 && hp.n_head_kv < hp.n_head {
        AttnKind::Gqa { groups: hp.n_head / hp.n_head_kv.max(1) }
    } else {
        AttnKind::Mha
    };

    let ffn = if hp.n_expert > 0 {
        FfnKind::Moe { experts: hp.n_expert, used: hp.n_expert_used.max(1) }
    } else { FfnKind::Dense };

    let qk_norm = matches!(family, Family::Qwen3);
    let rope = if hp.rope_freq_base <= 0.0 { RopeKind::None }
               else if hp.rope_scale != 0.0 && hp.rope_scale != 1.0 { RopeKind::Scaled }
               else { RopeKind::Standard };

    let mut features = Vec::new();
    if matches!(attention, AttnKind::Gqa { .. }) { features.push(Feature::Gqa); }
    if matches!(attention, AttnKind::Mla { .. }) { features.push(Feature::Mla); }
    if matches!(ffn, FfnKind::Moe { .. }) { features.push(Feature::Moe); }
    if qk_norm { features.push(Feature::QkNorm); }
    if rope == RopeKind::Scaled { features.push(Feature::RopeScaling); }
    if matches!(family, Family::Qwen2 | Family::Qwen3) { features.push(Feature::AttnQkvBias); }

    ArchInfo { family, attention, ffn, qk_norm, rope, features }
}

/// What kernels the engine currently has. Flip a flag on once the corresponding kernel lands.
#[derive(Clone, Copy, Debug)]
pub struct EngineCaps {
    pub gqa: bool,
    pub attn_qkv_bias: bool,
    pub rope_scaling: bool,
    pub qk_norm: bool,
    pub moe: bool,
    pub mla: bool,
}
impl EngineCaps {
    /// The current wgpu engine: llama-class (RMSNorm, SwiGLU, GQA, RoPE) plus Qwen2's QKV bias and basic
    /// RoPE scaling. MoE, MLA, and Qwen3 QK-norm still need kernels.
    pub fn current() -> Self {
        Self { gqa: true, attn_qkv_bias: true, rope_scaling: true, qk_norm: false, moe: false, mla: false }
    }
    fn has(&self, f: Feature) -> bool {
        match f {
            Feature::Gqa => self.gqa,
            Feature::AttnQkvBias => self.attn_qkv_bias,
            Feature::RopeScaling => self.rope_scaling,
            Feature::QkNorm => self.qk_norm,
            Feature::Moe => self.moe,
            Feature::Mla => self.mla,
        }
    }
}

/// Whether `caps` can run `info`. `Full` if every feature is covered; otherwise the missing features are
/// reported. Missing a *core* feature (MoE or MLA — there's no fallback) is `Unsupported`; missing only
/// refinements (e.g. QK-norm, RoPE scaling) is `Partial` (it will load and run, but not bit-exactly).
pub fn support(info: &ArchInfo, caps: &EngineCaps) -> Support {
    let missing: Vec<Feature> = info.features.iter().copied().filter(|f| !caps.has(*f)).collect();
    if missing.is_empty() { return Support::Full; }
    let core_missing = missing.iter().any(|f| matches!(f, Feature::Moe | Feature::Mla));
    if core_missing { Support::Unsupported(missing) } else { Support::Partial(missing) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn llama_7b() -> Hparams {
        Hparams { n_layer: 32, n_head: 32, n_head_kv: 32, n_embd: 4096, n_ff: 11008,
                  rope_freq_base: 10000.0, rope_scale: 1.0, ..Default::default() }
    }

    #[test]
    fn plain_llama_is_mha_dense_and_fully_supported() {
        let info = detect("llama", &llama_7b());
        assert_eq!(info.attention, AttnKind::Mha);
        assert_eq!(info.ffn, FfnKind::Dense);
        assert!(info.features.is_empty());
        assert_eq!(support(&info, &EngineCaps::current()), Support::Full);
    }

    #[test]
    fn qwen2_gqa_is_detected_and_runnable() {
        // Qwen2-7B: 28 heads, 4 KV heads ⇒ GQA groups 7, plus QKV bias
        let hp = Hparams { n_layer: 28, n_head: 28, n_head_kv: 4, n_embd: 3584, n_ff: 18944,
                           rope_freq_base: 1_000_000.0, rope_scale: 1.0, ..Default::default() };
        let info = detect("qwen2", &hp);
        assert_eq!(info.attention, AttnKind::Gqa { groups: 7 });
        assert!(info.features.contains(&Feature::Gqa) && info.features.contains(&Feature::AttnQkvBias));
        assert_eq!(support(&info, &EngineCaps::current()), Support::Full);
    }

    #[test]
    fn qwen3_adds_qk_norm_as_a_partial_gap() {
        let hp = Hparams { n_layer: 36, n_head: 32, n_head_kv: 8, n_embd: 4096, n_ff: 12288,
                           rope_freq_base: 1_000_000.0, rope_scale: 1.0, ..Default::default() };
        let info = detect("qwen3", &hp);
        assert!(info.qk_norm);
        // GQA + QKV bias are covered; only QK-norm is missing ⇒ Partial (runs, not bit-exact)
        assert_eq!(support(&info, &EngineCaps::current()), Support::Partial(vec![Feature::QkNorm]));
    }

    #[test]
    fn mixtral_moe_needs_the_expert_kernel() {
        let hp = Hparams { n_layer: 32, n_head: 32, n_head_kv: 8, n_embd: 4096, n_ff: 14336,
                           n_expert: 8, n_expert_used: 2, rope_freq_base: 1_000_000.0, rope_scale: 1.0, ..Default::default() };
        let info = detect("mixtral", &hp);
        assert_eq!(info.ffn, FfnKind::Moe { experts: 8, used: 2 });
        match support(&info, &EngineCaps::current()) {
            Support::Unsupported(m) => assert!(m.contains(&Feature::Moe)),
            other => panic!("MoE without the kernel must be Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn deepseek_v2_is_mla_plus_moe() {
        // DeepSeek-V2: latent attention (kv_lora_rank) + a large MoE
        let hp = Hparams { n_layer: 60, n_head: 128, n_head_kv: 128, n_embd: 5120, n_ff: 12288,
                           n_expert: 160, n_expert_used: 6, kv_lora_rank: 512, q_lora_rank: 1536,
                           rope_freq_base: 10000.0, rope_scale: 40.0, ..Default::default() };
        let info = detect("deepseek2", &hp);
        assert_eq!(info.attention, AttnKind::Mla { kv_lora_rank: 512, q_lora_rank: 1536 });
        assert!(info.features.contains(&Feature::Mla) && info.features.contains(&Feature::Moe));
        assert_eq!(info.rope, RopeKind::Scaled); // YaRN-style long context
        // two core features missing ⇒ Unsupported, both reported
        match support(&info, &EngineCaps::current()) {
            Support::Unsupported(m) => { assert!(m.contains(&Feature::Mla) && m.contains(&Feature::Moe)); }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn enabling_a_kernel_flips_support() {
        let hp = Hparams { n_layer: 32, n_head: 32, n_head_kv: 8, n_embd: 4096, n_ff: 14336,
                           n_expert: 8, n_expert_used: 2, rope_freq_base: 1e6, rope_scale: 1.0, ..Default::default() };
        let info = detect("mixtral", &hp);
        let caps = EngineCaps { moe: true, ..EngineCaps::current() }; // MoE kernel just landed
        assert_eq!(support(&info, &caps), Support::Full);
    }

    #[test]
    fn unknown_architecture_classifies_structurally() {
        let info = detect("some_new_llm", &llama_7b());
        assert_eq!(info.family, Family::Unknown);
        assert_eq!(info.attention, AttnKind::Mha); // still readable from hparams
    }
}
