// src/llama_backend.rs
//
// In-process GGUF inference via the `llama-cpp-2` crate, GPU-accelerated through
// Vulkan. Adapted from the user's uploaded llama_backend.rs to this crate: the
// cross-project `hf_hub` call is replaced with a local filename-based estimator,
// and the QML hook notes are dropped. The threading model and feature gating are
// kept as the user designed them.
//
// ── FEATURE GATING ──
// The FFI surface is gated behind the `llamacpp` Cargo feature. With it OFF
// (the default, and what builds in this sandbox) this module is a set of stubs
// returning "not built with llamacpp" and no llama-cpp-2 dependency is linked.
// To enable: add `llama-cpp-2 = { version = "0.1", features = ["vulkan"] }` to
// [dependencies], make `llamacpp` pull it, and build `--features llamacpp` on a
// machine with clang + a Vulkan SDK + the llama.cpp sources.
//
// ── THREADING ──
// Hold the model as Arc<LlamaModel> once; create a cheap per-query LlamaContext
// per generation on a single thread. The Arc clones into worker threads.
//
// ── WHAT IS NOT VERIFIED ──
// The cfg(feature="llamacpp") body cannot be compiled in this sandbox (no Vulkan
// SDK / clang / llama.cpp sources). It is written from llama-cpp-2's published
// examples and is DRAFT until `cargo build --features llamacpp` confirms it on
// hardware. The stub path (feature off) and the pure helpers below ARE compiled
// and tested here.

#![allow(dead_code)]

#[cfg(feature = "llamacpp")]
use std::sync::Arc;

#[cfg(feature = "llamacpp")]
pub struct LoadedModel {
    pub model: Arc<llama_cpp_2::model::LlamaModel>,
    pub backend: Arc<llama_cpp_2::llama_backend::LlamaBackend>,
    pub path: String,
    pub params_billions: f32,
}

#[cfg(not(feature = "llamacpp"))]
pub struct LoadedModel {
    pub path: String,
    pub params_billions: f32,
}

#[derive(Debug, Clone)]
pub struct GenParams {
    pub max_tokens: usize,
    pub n_ctx: u32,
    pub temperature: f32,
    pub n_threads: i32,
    pub n_gpu_layers: i32,
}

impl Default for GenParams {
    fn default() -> Self {
        Self {
            max_tokens: 512,
            n_ctx: 4096,
            temperature: 0.7,
            n_threads: num_cpus_fallback(),
            n_gpu_layers: -1, // offload everything to the GPU; Vulkan is the point
        }
    }
}

fn num_cpus_fallback() -> i32 {
    std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(4)
}

/// Best-effort parameter-count estimate from a GGUF filename (e.g. "...-7b-...",
/// "Qwen2.5-0.5B"). Replaces the cross-project hf_hub lookup. Pure + testable.
pub fn estimate_params_billions(path: &str) -> Option<f32> {
    let name = std::path::Path::new(path).file_name()?.to_str()?.to_lowercase();
    let b = name.as_bytes();
    for i in 1..b.len() {
        if b[i] == b'b' {
            let mut j = i;
            while j > 0 && (b[j - 1].is_ascii_digit() || b[j - 1] == b'.') {
                j -= 1;
            }
            if j < i {
                if let Ok(v) = name[j..i].parse::<f32>() {
                    if v > 0.0 && v < 2000.0 {
                        return Some(v);
                    }
                }
            }
        }
    }
    None
}

// ── Active-model registry ──
#[cfg(feature = "llamacpp")]
mod active_registry {
    use super::{Arc, LoadedModel};
    use std::sync::{Mutex, OnceLock};
    static ACTIVE: OnceLock<Mutex<Option<Arc<LoadedModel>>>> = OnceLock::new();
    fn cell() -> &'static Mutex<Option<Arc<LoadedModel>>> {
        ACTIVE.get_or_init(|| Mutex::new(None))
    }
    pub fn set(model: Arc<LoadedModel>) {
        if let Ok(mut g) = cell().lock() { *g = Some(model); }
    }
    pub fn get() -> Option<Arc<LoadedModel>> {
        cell().lock().ok().and_then(|g| g.clone())
    }
    pub fn is_set() -> bool {
        cell().lock().map(|g| g.is_some()).unwrap_or(false)
    }
}

#[cfg(feature = "llamacpp")]
pub fn set_active_model(model: Arc<LoadedModel>) { active_registry::set(model); }
#[cfg(feature = "llamacpp")]
pub fn active_model() -> Option<Arc<LoadedModel>> { active_registry::get() }
#[cfg(feature = "llamacpp")]
pub fn has_active_model() -> bool { active_registry::is_set() }

#[cfg(not(feature = "llamacpp"))]
pub fn set_active_model(_model: std::sync::Arc<LoadedModel>) {}
#[cfg(not(feature = "llamacpp"))]
pub fn active_model() -> Option<std::sync::Arc<LoadedModel>> { None }
#[cfg(not(feature = "llamacpp"))]
pub fn has_active_model() -> bool { false }

#[cfg(feature = "llamacpp")]
pub fn load_model(path: &str, params: &GenParams) -> Result<LoadedModel, String> {
    use llama_cpp_2::llama_backend::LlamaBackend;
    use llama_cpp_2::model::params::LlamaModelParams;
    use llama_cpp_2::model::LlamaModel;

    let backend = LlamaBackend::init().map_err(|e| format!("llama backend init failed: {e}"))?;
    let model_params = LlamaModelParams::default().with_n_gpu_layers(params.n_gpu_layers as u32);
    let model = LlamaModel::load_from_file(&backend, path, &model_params)
        .map_err(|e| format!("failed to load GGUF '{path}': {e}"))?;
    let params_billions = estimate_params_billions(path).unwrap_or(0.0);
    Ok(LoadedModel {
        model: Arc::new(model),
        backend: Arc::new(backend),
        path: path.to_string(),
        params_billions,
    })
}

#[cfg(not(feature = "llamacpp"))]
pub fn load_model(_path: &str, _params: &GenParams) -> Result<LoadedModel, String> {
    Err("not built with `llamacpp` feature".to_string())
}

#[cfg(feature = "llamacpp")]
pub fn generate(loaded: &LoadedModel, prompt: &str, params: &GenParams) -> Result<String, String> {
    use llama_cpp_2::context::params::LlamaContextParams;
    use llama_cpp_2::llama_batch::LlamaBatch;
    use llama_cpp_2::model::{AddBos, Special};
    use llama_cpp_2::sampling::LlamaSampler;
    use std::num::NonZeroU32;

    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(params.n_ctx))
        .with_n_threads(params.n_threads);
    let mut ctx = loaded.model.new_context(&loaded.backend, ctx_params)
        .map_err(|e| format!("context creation failed: {e}"))?;

    let tokens = loaded.model.str_to_token(prompt, AddBos::Always)
        .map_err(|e| format!("tokenize failed: {e}"))?;
    let n_prompt = tokens.len();
    if n_prompt as u32 >= params.n_ctx {
        return Err(format!("prompt ({n_prompt} tokens) exceeds context window ({})", params.n_ctx));
    }

    let mut batch = LlamaBatch::new(params.n_ctx as usize, 1);
    let last_idx = n_prompt - 1;
    for (i, tok) in tokens.iter().enumerate() {
        batch.add(*tok, i as i32, &[0], i == last_idx)
            .map_err(|e| format!("batch add failed: {e}"))?;
    }
    ctx.decode(&mut batch).map_err(|e| format!("prompt decode failed: {e}"))?;

    let mut sampler = if params.temperature <= 0.01 {
        LlamaSampler::greedy()
    } else {
        LlamaSampler::chain_simple([
            LlamaSampler::temp(params.temperature),
            LlamaSampler::dist(1234),
        ])
    };

    let mut out = String::new();
    let mut n_cur = n_prompt as i32;
    let mut n_decoded = 0usize;
    while n_decoded < params.max_tokens {
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(token);
        if loaded.model.is_eog_token(token) { break; }
        let piece = loaded.model.token_to_str(token, Special::Plaintext).unwrap_or_default();
        out.push_str(&piece);
        batch.clear();
        batch.add(token, n_cur, &[0], true).map_err(|e| format!("batch add (gen) failed: {e}"))?;
        n_cur += 1;
        n_decoded += 1;
        ctx.decode(&mut batch).map_err(|e| format!("gen decode failed: {e}"))?;
    }
    Ok(out)
}

#[cfg(not(feature = "llamacpp"))]
pub fn generate(_loaded: &LoadedModel, _prompt: &str, _params: &GenParams) -> Result<String, String> {
    Err("not built with `llamacpp` feature".to_string())
}

/// Whether this build supports the in-process llama.cpp backend.
pub const fn is_available() -> bool {
    cfg!(feature = "llamacpp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gen_params_default_offloads_to_gpu() {
        let p = GenParams::default();
        assert_eq!(p.n_gpu_layers, -1);
        assert!(p.n_threads >= 1);
        assert_eq!(p.max_tokens, 512);
    }

    #[test]
    fn num_cpus_fallback_positive() {
        assert!(num_cpus_fallback() >= 1);
    }

    #[test]
    fn is_available_matches_feature() {
        assert_eq!(is_available(), cfg!(feature = "llamacpp"));
    }

    #[test]
    fn estimate_params_from_name() {
        assert_eq!(estimate_params_billions("Meta-Llama-3-8B-Instruct.Q4.gguf"), Some(8.0));
        assert_eq!(estimate_params_billions("qwen2.5-0.5b-instruct.gguf"), Some(0.5));
        assert_eq!(estimate_params_billions("model-no-size.gguf"), None);
    }

    #[test]
    fn load_without_feature_errors() {
        #[cfg(not(feature = "llamacpp"))]
        {
            match load_model("/nonexistent.gguf", &GenParams::default()) {
                Ok(_) => panic!("stub path should not load a model"),
                Err(e) => assert!(e.contains("llamacpp")),
            }
        }
    }
}
