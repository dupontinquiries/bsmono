//! model_kind.rs — classify a gguf as LLM / diffusion / vision-projector BEFORE the LLM loader tries it.
//!
//! The LLM loader assumes every `.gguf` is a language model and, on a diffusion checkpoint (e.g.
//! `z-image-turbo-Q8_0.gguf`), fell through to the llama path and died deep inside tokenizer loading with the
//! cryptic `missing tokenizer.ggml.tokens`. A diffusion model simply has no token vocab — it isn't an LLM at
//! all. This inspects gguf metadata + tensor NAMES (the honest signal, since converters often drop
//! `general.architecture`) and reports what the file actually is, so the loader can route a diffusion model
//! to the image pipeline (sd-cli / mediagen) and refuse with a useful message instead of a tokenizer crash.

use crate::gguf::Gguf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelKind {
    /// A language model — safe for the wgpu LLM loader. `arch` is the reported/derived architecture.
    Llm { arch: String },
    /// A diffusion image model (stable-diffusion / flux / z-image / DiT). Needs the image subsystem
    /// (sd-cli), not the LLM loader. `family` is a best-effort label.
    Diffusion { family: String },
    /// A CLIP vision projector (`mmproj`) — pairs with a VLM as its `--mmproj`, not loaded standalone.
    VisionProjector,
    /// Couldn't positively identify it, but it lacks LLM markers — definitely not for the LLM loader.
    Unknown,
}

impl ModelKind {
    pub fn is_llm(&self) -> bool { matches!(self, ModelKind::Llm { .. }) }
    /// A user-facing explanation + where to take this file, for the non-LLM cases.
    pub fn guidance(&self) -> String {
        match self {
            ModelKind::Llm { arch } => format!("language model ({arch})"),
            ModelKind::Diffusion { family } => format!(
                "this is a {family} diffusion image model, not a language model — it has no tokenizer. \
                 Load it through the image pipeline (:image / sd-cli), not the LLM loader."),
            ModelKind::VisionProjector =>
                "this is a CLIP vision projector (mmproj) — it pairs with a vision-language model as its \
                 --mmproj companion and isn't loaded on its own. Load the VLM and point --mmproj at this file.".into(),
            ModelKind::Unknown =>
                "this gguf has no language-model tensors (no token embedding / tokenizer), so the LLM loader \
                 can't use it. It may be a diffusion, vision, or audio model for a different subsystem.".into(),
        }
    }
}

fn any_tensor(g: &Gguf, needles: &[&str]) -> bool {
    g.tensors.iter().any(|t| { let n = t.name.as_str(); needles.iter().any(|s| n.contains(s)) })
}

/// Architectures that are diffusion image models (from `general.architecture` when present).
const DIFFUSION_ARCHS: &[&str] = &[
    "flux", "sd", "sd1", "sd2", "sdxl", "sd3", "stable-diffusion", "stable_diffusion",
    "z-image", "zimage", "z_image", "pixart", "pixart-alpha", "dit", "mmdit", "wan", "hunyuan-dit", "kolors",
];

/// Classify a parsed gguf. Order matters: positive LLM markers win first (a real LLM always has them), then
/// vision projector, then diffusion, else Unknown. Detection leans on tensor NAMES because diffusion/vision
/// converters frequently omit or reuse `general.architecture`.
pub fn classify(g: &Gguf) -> ModelKind {
    let arch = g.get_str("general.architecture").unwrap_or("").to_ascii_lowercase();

    // --- LLM: the defining markers are a tokenizer vocab or the token-embedding + transformer-block tensors.
    let has_tokenizer = g.metadata.contains_key("tokenizer.ggml.tokens");
    let has_llm_tensors = g.tensors.iter().any(|t| {
        let n = t.name.as_str();
        // LLM transformer blocks are TOP-LEVEL "blk.N.*" (vision projector blocks are "v.blk.N.*", so we
        // must anchor at the start, not substring-match, or an mmproj reads as an LLM).
        n == "token_embd.weight" || n == "output_norm.weight" || n.starts_with("blk.")
    });
    if has_tokenizer || has_llm_tensors {
        let a = if arch.is_empty() { "llama".to_string() } else { arch };
        return ModelKind::Llm { arch: a };
    }

    // --- Vision projector (mmproj): CLIP tower tensors / the clip.* metadata flag.
    let has_clip_meta = g.metadata.keys().any(|k| k.starts_with("clip."));
    let has_vision_tensors = any_tensor(g, &["mm.", "mm_input", "v.blk", "v.patch_embd", "vision_model", "mmproj"]);
    if arch == "clip" || has_clip_meta || has_vision_tensors {
        return ModelKind::VisionProjector;
    }

    // --- Diffusion: architecture tag, or the characteristic UNet / DiT / flux tensor names.
    if DIFFUSION_ARCHS.contains(&arch.as_str()) {
        return ModelKind::Diffusion { family: if arch.is_empty() { "diffusion".into() } else { arch } };
    }
    let unet = any_tensor(g, &["input_blocks", "output_blocks", "middle_block", "time_embed", "model.diffusion_model"]);
    let flux = any_tensor(g, &["double_blocks", "single_blocks", "img_in", "txt_in", "img_mlp", "txt_mlp"]);
    let dit = any_tensor(g, &["x_embedder", "t_embedder", "y_embedder", "final_layer", "pos_embed", "patch_embed"]);
    let vae = any_tensor(g, &["first_stage_model", "encoder.down", "decoder.up", "quant_conv"]);
    if unet || flux || dit || vae {
        let family = if flux { "flux" } else if unet { "stable-diffusion" } else if dit { "DiT" } else { "diffusion" };
        return ModelKind::Diffusion { family: family.into() };
    }

    ModelKind::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gguf::{Gguf, Value, TensorInfo};
    use std::collections::HashMap;

    fn g(meta: &[(&str, Value)], tensors: &[&str]) -> Gguf {
        Gguf {
            version: 3,
            metadata: meta.iter().map(|(k, v)| (k.to_string(), v.clone())).collect::<HashMap<_, _>>(),
            tensors: tensors.iter().map(|n| TensorInfo { name: n.to_string(), dims: vec![1], ggml_type: 0, offset: 0 }).collect(),
            data_offset: 0,
        }
    }

    #[test]
    fn llm_by_tokenizer_or_tensors() {
        let by_tok = g(&[("general.architecture", Value::Str("qwen2".into())), ("tokenizer.ggml.tokens", Value::Arr(vec![]))], &[]);
        assert_eq!(classify(&by_tok), ModelKind::Llm { arch: "qwen2".into() });
        // no arch, no tokenizer, but LLM tensors → still an LLM (converters drop arch)
        let by_tensor = g(&[], &["token_embd.weight", "blk.0.attn_q.weight", "output_norm.weight"]);
        assert_eq!(classify(&by_tensor), ModelKind::Llm { arch: "llama".into() });
        assert!(classify(&by_tensor).is_llm());
    }

    #[test]
    fn z_image_diffusion_is_not_an_llm() {
        // the real failing case: no tokenizer, no LLM tensors, DiT-style tensor names → Diffusion, not a crash
        let z = g(&[], &["x_embedder.weight", "t_embedder.mlp.0.weight", "final_layer.linear.weight", "pos_embed"]);
        let k = classify(&z);
        assert!(matches!(k, ModelKind::Diffusion { .. }), "got {k:?}");
        assert!(!k.is_llm());
        assert!(k.guidance().contains("image pipeline"));
    }

    #[test]
    fn flux_and_unet_and_arch_tag() {
        let flux = g(&[], &["double_blocks.0.img_attn.qkv.weight", "single_blocks.0.linear1.weight"]);
        assert_eq!(classify(&flux), ModelKind::Diffusion { family: "flux".into() });
        let unet = g(&[], &["model.diffusion_model.input_blocks.0.0.weight", "time_embed.0.weight"]);
        assert_eq!(classify(&unet), ModelKind::Diffusion { family: "stable-diffusion".into() });
        let tagged = g(&[("general.architecture", Value::Str("sdxl".into()))], &[]);
        assert_eq!(classify(&tagged), ModelKind::Diffusion { family: "sdxl".into() });
    }

    #[test]
    fn mmproj_is_a_vision_projector() {
        let mm = g(&[("clip.has_vision_encoder", Value::Bool(true))], &["mm.0.weight", "v.blk.0.attn_q.weight"]);
        assert_eq!(classify(&mm), ModelKind::VisionProjector);
        assert!(classify(&mm).guidance().contains("mmproj"));
    }

    #[test]
    fn truly_unknown_is_flagged_not_misread_as_llm() {
        let k = classify(&g(&[("general.name", Value::Str("mystery".into()))], &["some.random.tensor"]));
        assert_eq!(k, ModelKind::Unknown);
        assert!(!k.is_llm());
    }
}
