// src/llama_backend.rs
//
// v0.1.462 (llama.cpp backend arc): in-process GGUF inference
// via the `llama-cpp-2` crate, GPU-accelerated through Vulkan
// (the universal target — works on Apple Silicon via Honeykrisp
// and Qualcomm Adreno via Mesa Turnip, plus AMD via RADV).
//
// ── FEATURE GATING ──
//
// The entire FFI surface is gated behind the `llamacpp` Cargo
// feature, which pulls in `llama-cpp-2/vulkan`. When the
// feature is OFF:
//   - This module compiles to a set of stub functions that
//     return "not built with llamacpp" errors.
//   - No llama-cpp-2 dependency is linked.
//   - The Provider::LlamaCpp dispatch arm falls through to the
//     network path (Claude/OpenAI) or errors cleanly.
// This mirrors the `claude` feature pattern in llm_provider.rs:
// the user can always drop the feature and rebuild.
//
// ── THREADING MODEL (important) ──
//
// The expensive resource is the model weights (multiple GB).
// Per the llama-cpp-2 usage pattern, we hold the model as
// `Arc<LlamaModel>` ONCE after loading, then create a cheap
// per-query `LlamaContext` for each generation. The Arc clones
// into the existing async summary worker thread the same way
// the API-key Arc does. A LlamaContext is created and consumed
// entirely within a single generation call on one thread, so
// we never share a context across threads.
//
// ── WHAT I COULD NOT VERIFY ──
//
// This module's FFI calls (the cfg(feature="llamacpp") body)
// CANNOT be compiled in the dev sandbox: llama-cpp-2 needs
// clang + a Vulkan SDK + the llama.cpp C++ sources, none of
// which are available here. The exact llama-cpp-2 API
// (LlamaBackend::init, load_from_file, new_context, batch
// decode, sampler loop) is written from the crate's published
// examples but has NOT been compiled. Treat the
// cfg(feature="llamacpp") body as DRAFT until `cargo build
// --features=llamacpp` confirms it on hardware. The stub path
// (feature off) and the pure helpers below ARE compile-checked.

#![allow(dead_code)]

#[cfg(feature = "llamacpp")]
use std::sync::Arc;

/// Handle to a loaded model. Behind the feature flag this wraps
/// the real Arc<LlamaModel> + backend; without it, it's a unit
/// placeholder so the Backend struct field type is stable
/// across feature configs (same trick as the Provider enum).
#[cfg(feature = "llamacpp")]
pub struct LoadedModel {
    pub model: Arc<llama_cpp_2::model::LlamaModel>,
    pub backend: Arc<llama_cpp_2::llama_backend::LlamaBackend>,
    /// Path the model was loaded from (for display / reload).
    pub path: String,
    /// Estimated parameter count in billions, for context-size
    /// and recommendation logic. 0.0 if unknown.
    pub params_billions: f32,
}

#[cfg(not(feature = "llamacpp"))]
pub struct LoadedModel {
    pub path: String,
    pub params_billions: f32,
}

/// Generation parameters. Kept feature-independent so callers
/// don't vary by config.
#[derive(Debug, Clone)]
pub struct GenParams {
    /// Maximum tokens to generate.
    pub max_tokens: usize,
    /// Context window size. Clamped to the model's trained max
    /// by llama.cpp internally.
    pub n_ctx: u32,
    /// Max tokens decodable in ONE decode() call. MUST be >= the
    /// prompt token count or llama.cpp aborts the process with
    /// GGML_ASSERT(n_tokens_all <= cparams.n_batch). We feed the
    /// whole prompt in one batch, so this is set equal to n_ctx.
    /// (v0.1.560: was unset → defaulted to 512 while n_ctx=4096, so
    /// any prompt of 512..4096 tokens crashed the whole app.)
    pub n_batch: u32,
    /// Sampling temperature. 0.0 = greedy.
    pub temperature: f32,
    /// Number of CPU threads for the non-offloaded parts.
    pub n_threads: i32,
    /// GPU layers to offload. -1 = offload all (the whole point
    /// of the Vulkan backend). 0 = CPU only.
    pub n_gpu_layers: i32,
}

impl Default for GenParams {
    fn default() -> Self {
        Self {
            max_tokens: 512,
            n_ctx: 4096,
            n_batch: 4096, // == n_ctx: feed the whole prompt in one batch
            temperature: 0.7,
            n_threads: num_cpus_fallback(),
            // Offload everything to the GPU by default — Vulkan
            // is the reason this backend exists. If the model
            // doesn't fit in VRAM, llama.cpp spills to CPU
            // automatically for the overflow layers.
            n_gpu_layers: -1,
        }
    }
}

/// Best-effort CPU thread count without pulling the num_cpus
/// crate. Falls back to 4 if the std API is unavailable.
fn num_cpus_fallback() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(4)
}

// ── Active-model registry (v0.1.554) ──
//
// A process-global handle to the currently-active loaded model, so
// off-thread consumers (the log-summary worker, future agents) can use
// the local model WITHOUT threading an Arc through every call site. The
// Backend populates this on model activation (set_llama_model_path) and
// on async load. Mirrors the OLLAMA_FALLBACK_ENABLED global-state
// pattern in system_info. Safe because LoadedModel is already shared as
// Arc across thread boundaries by the async query worker.
#[cfg(feature = "llamacpp")]
mod active_registry {
    use super::{Arc, LoadedModel};
    use std::sync::{Mutex, OnceLock};

    static ACTIVE: OnceLock<Mutex<Option<Arc<LoadedModel>>>> = OnceLock::new();

    fn cell() -> &'static Mutex<Option<Arc<LoadedModel>>> {
        ACTIVE.get_or_init(|| Mutex::new(None))
    }

    /// Publish the active model (called on load/activation).
    pub fn set(model: Arc<LoadedModel>) {
        if let Ok(mut g) = cell().lock() { *g = Some(model); }
    }

    /// Snapshot the active model, if any. Cheap Arc clone.
    pub fn get() -> Option<Arc<LoadedModel>> {
        cell().lock().ok().and_then(|g| g.clone())
    }

    /// True when a model is registered.
    pub fn is_set() -> bool {
        cell().lock().map(|g| g.is_some()).unwrap_or(false)
    }
}

/// Publish the active model so off-thread consumers can use it.
/// No-op when built without llamacpp.
#[cfg(feature = "llamacpp")]
pub fn set_active_model(model: Arc<LoadedModel>) { active_registry::set(model); }

/// Snapshot the active model, if any. None when built without llamacpp.
#[cfg(feature = "llamacpp")]
pub fn active_model() -> Option<Arc<LoadedModel>> { active_registry::get() }

/// Whether an active local model is registered.
#[cfg(feature = "llamacpp")]
pub fn has_active_model() -> bool { active_registry::is_set() }

#[cfg(not(feature = "llamacpp"))]
pub fn has_active_model() -> bool { false }

/// Feature-off stubs so the (runtime-guarded, compile-unconditional)
/// call sites in main.rs build without llamacpp. LoadedModel exists in
/// both builds, so the signatures line up; these just no-op / None.
#[cfg(not(feature = "llamacpp"))]
pub fn set_active_model(_model: std::sync::Arc<LoadedModel>) {}

#[cfg(not(feature = "llamacpp"))]
pub fn active_model() -> Option<std::sync::Arc<LoadedModel>> { None }


/// Load a GGUF model from disk. Returns a LoadedModel handle on
/// success.
///
/// Feature ON: actually loads via llama-cpp-2 with Vulkan.
/// Feature OFF: returns an error so the caller can fall back.
#[cfg(feature = "llamacpp")]
pub fn load_model(path: &str, params: &GenParams)
    -> Result<LoadedModel, String>
{
    use llama_cpp_2::llama_backend::LlamaBackend;
    use llama_cpp_2::model::LlamaModel;
    use llama_cpp_2::model::params::LlamaModelParams;

    // Backend init is process-global in llama.cpp; calling it
    // more than once is benign but we hold one in the handle.
    let backend = LlamaBackend::init()
        .map_err(|e| format!("llama backend init failed: {e}"))?;

    // n_gpu_layers on the MODEL params controls offload.
    let model_params = LlamaModelParams::default()
        .with_n_gpu_layers(params.n_gpu_layers as u32);

    let model = LlamaModel::load_from_file(&backend, path, &model_params)
        .map_err(|e| format!("failed to load GGUF '{path}': {e}"))?;

    let params_billions = crate::hf_hub::estimate_params_billions(path)
        .unwrap_or(0.0);

    Ok(LoadedModel {
        model: Arc::new(model),
        backend: Arc::new(backend),
        path: path.to_string(),
        params_billions,
    })
}

#[cfg(not(feature = "llamacpp"))]
pub fn load_model(_path: &str, _params: &GenParams)
    -> Result<LoadedModel, String>
{
    Err("not built with `llamacpp` feature".to_string())
}

/// Run a single completion against a loaded model. Creates a
/// fresh context, tokenizes the prompt, decodes, and samples
/// until max_tokens or EOS. Returns the generated text.
///
/// This is the per-query hot path. The model Arc is shared; the
/// context is created and dropped within this call (cheap
/// relative to the weights).
#[cfg(feature = "llamacpp")]
pub fn generate(
    loaded: &LoadedModel,
    prompt: &str,
    params: &GenParams,
) -> Result<String, String> {
    use std::num::NonZeroU32;
    use llama_cpp_2::context::params::LlamaContextParams;
    use llama_cpp_2::llama_batch::LlamaBatch;
    use llama_cpp_2::model::AddBos;
    use llama_cpp_2::sampling::LlamaSampler;

    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(params.n_ctx))
        .with_n_batch(params.n_batch)
        .with_n_threads(params.n_threads);

    let mut ctx = loaded.model
        .new_context(&loaded.backend, ctx_params)
        .map_err(|e| format!("context creation failed: {e}"))?;

    // Tokenize the prompt (with BOS as the model expects).
    let tokens = loaded.model
        .str_to_token(prompt, AddBos::Always)
        .map_err(|e| format!("tokenize failed: {e}"))?;

    // v0.1.560: the GGML_ASSERT(n_tokens_all <= cparams.n_batch) that
    // crashed the whole process fires inside decode() when the prompt
    // is longer than n_batch. The OLD guard checked against n_ctx, not
    // n_batch — so with the default n_batch=512 and n_ctx=4096, a
    // 512..4096-token prompt slipped past the guard and ABORTED. We now
    // (a) feed in one batch sized n_batch==n_ctx, and (b) TRUNCATE to a
    // safe cap (keeping the most recent tokens) rather than erroring —
    // a chat conversation grows unboundedly and must degrade, never
    // crash. We reserve room for the generated reply too (max_tokens).
    let usable = (params.n_ctx as usize)
        .saturating_sub(params.max_tokens)
        .min(params.n_batch as usize);
    // keep at least a minimal prompt window even if params are odd
    let cap = usable.max(64);
    let tokens: Vec<_> = if tokens.len() > cap {
        crate::clog!("llm",
            "prompt {} tokens > cap {} — truncating to most recent (keeps BOS)",
            tokens.len(), cap);
        // keep the BOS (first) token + the most recent (cap-1) tokens
        let mut kept = Vec::with_capacity(cap);
        kept.push(tokens[0]);
        let tail_start = tokens.len() - (cap - 1);
        kept.extend_from_slice(&tokens[tail_start..]);
        kept
    } else {
        tokens
    };

    let n_prompt = tokens.len();
    if n_prompt == 0 {
        return Err("empty prompt after tokenization".into());
    }

    // Feed the prompt in one batch.
    let mut batch = LlamaBatch::new(params.n_ctx as usize, 1);
    let last_idx = n_prompt - 1;
    for (i, tok) in tokens.iter().enumerate() {
        let is_last = i == last_idx;
        batch.add(*tok, i as i32, &[0], is_last)
            .map_err(|e| format!("batch add failed: {e}"))?;
    }
    ctx.decode(&mut batch)
        .map_err(|e| format!("prompt decode failed: {e}"))?;

    // Sampler: greedy if temperature ~0, else temperature
    // sampling. Kept simple for v1 — top-p/top-k tuning is a
    // follow-up.
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

        // EOS check.
        if loaded.model.is_eog_token(token) {
            break;
        }

        // Convert token → text piece and append.
        // NOTE: token_to_str(_, Special::Plaintext) is deprecated in
        // llama-cpp-2: migrated off the deprecated token_to_str/Special
        // (v0.1.606). token_to_str(tok, Special::Plaintext) is EXACTLY
        // token_to_piece(tok, &mut utf8_decoder, false, None) — verified
        // against the crate source (0.1.146): token_to_str builds a
        // UTF-8 decoder and calls token_to_piece with special =
        // matches!(Plaintext, Tokenize) = false, lstrip = None. So this
        // is behavior-preserving, not a guess.
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let piece = loaded.model
            .token_to_piece(token, &mut decoder, false, None)
            .unwrap_or_default();
        out.push_str(&piece);

        // Prepare next batch with just the sampled token.
        batch.clear();
        batch.add(token, n_cur, &[0], true)
            .map_err(|e| format!("batch add (gen) failed: {e}"))?;
        n_cur += 1;
        n_decoded += 1;

        ctx.decode(&mut batch)
            .map_err(|e| format!("gen decode failed: {e}"))?;
    }

    Ok(out)
}

#[cfg(not(feature = "llamacpp"))]
pub fn generate(
    _loaded: &LoadedModel,
    _prompt: &str,
    _params: &GenParams,
) -> Result<String, String> {
    Err("not built with `llamacpp` feature".to_string())
}

/// Whether this build supports the in-process llama.cpp
/// backend. QML can read this (via a Backend qt_method) to
/// show/hide the model picker.
pub const fn is_available() -> bool {
    cfg!(feature = "llamacpp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gen_params_default_offloads_to_gpu() {
        let p = GenParams::default();
        // Vulkan-everywhere: default must offload all layers.
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
        // In the dev sandbox the feature is off, so this is
        // false. On a `--features=llamacpp` build it's true.
        assert_eq!(is_available(), cfg!(feature = "llamacpp"));
    }

    #[test]
    fn load_without_feature_errors() {
        // The stub path must return an error, not panic.
        #[cfg(not(feature = "llamacpp"))]
        {
            let r = load_model("/nonexistent.gguf", &GenParams::default());
            // Avoid unwrap_err (would require LoadedModel: Debug);
            // match the Result instead.
            match r {
                Ok(_) => panic!("stub path should not load a model"),
                Err(e) => assert!(e.contains("llamacpp")),
            }
        }
    }
}
