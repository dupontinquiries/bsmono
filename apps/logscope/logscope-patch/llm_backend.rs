//! Local GGML / GGUF inference backend — the "ollama -> ggml" path.
//!
//! Lets logscope run a local GGUF model in-process as an alternative to the
//! Ollama HTTP providers. The model layer treats a local GGUF as just another
//! pickable model; Chat routes to whichever backend the picked model belongs to.
//!
//! BUILD BOUNDARY (same class as the render/glfw wall): the FFI body — the
//! `cfg(feature = "llamacpp")` sections — needs `clang` + a Vulkan SDK + the
//! llama.cpp C++ sources, none of which exist in the dev sandbox. Treat that
//! body as DRAFT until `cargo build -p logscope --features llamacpp` confirms it
//! on hardware (Asahi/Vulkan). Everything else here — GGUF discovery, the
//! parameter estimator, GenParams, the active-model registry, `is_available()`,
//! and the stub load/generate — compiles and is unit-tested everywhere.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

// ── Loaded-model handle (field-stable across feature configs) ────────────────
#[cfg(feature = "llama-ffi")]
pub struct LoadedModel {
    pub model: Arc<llama_cpp_2::model::LlamaModel>,
    pub backend: Arc<llama_cpp_2::llama_backend::LlamaBackend>,
    pub path: String,
    pub params_billions: f32,
}
#[cfg(not(feature = "llama-ffi"))]
pub struct LoadedModel {
    pub path: String,
    pub params_billions: f32,
}

/// Generation parameters (feature-independent so callers don't vary by config).
#[derive(Clone, Debug)]
pub struct GenParams {
    pub max_tokens: usize,
    pub n_ctx: u32,
    /// MUST equal n_ctx: we feed the whole prompt in one batch to avoid the
    /// `GGML_ASSERT(n_tokens_all <= n_batch)` process-abort in llama.cpp.
    pub n_batch: u32,
    pub temperature: f32,
    pub n_threads: i32,
    /// -1 = offload all layers to the GPU.
    pub n_gpu_layers: i32,
}
impl Default for GenParams {
    fn default() -> Self {
        Self { max_tokens: 512, n_ctx: 4096, n_batch: 4096, temperature: 0.7, n_threads: 4, n_gpu_layers: -1 }
    }
}

/// Compile-time capability flag the UI reads to show/hide the local path.
pub const fn is_available() -> bool { cfg!(feature = "llama-ffi") }

// ── Process-global active-model registry (feature-independent) ───────────────
static ACTIVE: OnceLock<Mutex<Option<Arc<LoadedModel>>>> = OnceLock::new();
fn cell() -> &'static Mutex<Option<Arc<LoadedModel>>> { ACTIVE.get_or_init(|| Mutex::new(None)) }
pub fn set_active_model(m: Arc<LoadedModel>) { *cell().lock().unwrap() = Some(m); }
pub fn active_model() -> Option<Arc<LoadedModel>> { cell().lock().unwrap().clone() }
pub fn has_active_model() -> bool { cell().lock().unwrap().is_some() }
pub fn clear_active_model() { *cell().lock().unwrap() = None; }

// ── Pure: local GGUF discovery (works in any config) ─────────────────────────
#[derive(Clone, Debug)]
pub struct LocalModel {
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub params_billions: f32,
}

/// Default directory logscope looks in for local `.gguf` files.
pub fn default_models_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("logscope").join("models")
}

/// Estimate parameter count (billions) from a filename, e.g. "qwen2.5-7b" -> 7.0,
/// "mixtral-8x7b" -> 56.0. Returns None if no `<n>b` marker is present.
pub fn estimate_params_billions(path: &str) -> Option<f32> {
    let name = Path::new(path).file_name()?.to_string_lossy().to_lowercase();
    let re = regex::Regex::new(r"(\d+)x(\d+(?:\.\d+)?)b|(\d+(?:\.\d+)?)b").ok()?;
    let c = re.captures(&name)?;
    if let (Some(a), Some(b)) = (c.get(1), c.get(2)) {
        Some(a.as_str().parse::<f32>().ok()? * b.as_str().parse::<f32>().ok()?)
    } else {
        c.get(3).and_then(|m| m.as_str().parse::<f32>().ok())
    }
}

/// Scan a directory for `*.gguf` files (non-recursive), sorted by name.
pub fn list_local_gguf(dir: &Path) -> Vec<LocalModel> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("gguf") {
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let pb = estimate_params_billions(&p.to_string_lossy()).unwrap_or(0.0);
                out.push(LocalModel { name, path: p, size_bytes: size, params_billions: pb });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

// ── load / generate ──────────────────────────────────────────────────────────
#[cfg(feature = "llama-ffi")]
pub fn load_model(path: &str, params: &GenParams) -> Result<LoadedModel, String> {
    use llama_cpp_2::llama_backend::LlamaBackend;
    use llama_cpp_2::model::params::LlamaModelParams;
    use llama_cpp_2::model::LlamaModel;

    if !std::path::Path::new(path).exists() {
        return Err(format!("model file not found: {path}"));
    }
    let backend = LlamaBackend::init().map_err(|e| format!("llama backend init failed: {e}"))?;
    let model_params = LlamaModelParams::default().with_n_gpu_layers(params.n_gpu_layers as u32);
    let model = LlamaModel::load_from_file(&backend, path, &model_params)
        .map_err(|e| format!("failed to load GGUF '{path}': {e}"))?;
    let params_billions = estimate_params_billions(path).unwrap_or(0.0);
    Ok(LoadedModel { model: Arc::new(model), backend: Arc::new(backend), path: path.to_string(), params_billions })
}
#[cfg(not(feature = "llama-ffi"))]
pub fn load_model(_path: &str, _params: &GenParams) -> Result<LoadedModel, String> {
    Err("logscope was built without the `llamacpp` feature; rebuild with `--features llamacpp`".into())
}

/// Streaming completion: invokes `on_token` for each decoded piece and returns
/// the number of tokens generated. `generate` is the buffered wrapper.
#[cfg(feature = "llama-ffi")]
pub fn generate_stream(loaded: &LoadedModel, prompt: &str, params: &GenParams, mut on_token: impl FnMut(&str)) -> Result<usize, String> {
    use std::num::NonZeroU32;
    use llama_cpp_2::context::params::LlamaContextParams;
    use llama_cpp_2::llama_batch::LlamaBatch;
    use llama_cpp_2::model::AddBos;
    use llama_cpp_2::sampling::LlamaSampler;

    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(params.n_ctx))
        .with_n_batch(params.n_batch)
        .with_n_threads(params.n_threads);
    let mut ctx = loaded.model.new_context(&loaded.backend, ctx_params)
        .map_err(|e| format!("context creation failed: {e}"))?;

    let tokens = loaded.model.str_to_token(prompt, AddBos::Always)
        .map_err(|e| format!("tokenize failed: {e}"))?;

    // Keep prompt within one batch (n_batch == n_ctx) and reserve room for the
    // reply; truncate to the most-recent window (keeping BOS) rather than abort.
    let usable = (params.n_ctx as usize).saturating_sub(params.max_tokens).min(params.n_batch as usize);
    let cap = usable.max(64);
    let tokens: Vec<_> = if tokens.len() > cap {
        let mut kept = Vec::with_capacity(cap);
        kept.push(tokens[0]);
        kept.extend_from_slice(&tokens[tokens.len() - (cap - 1)..]);
        kept
    } else { tokens };

    let n_prompt = tokens.len();
    if n_prompt == 0 { return Err("empty prompt after tokenization".into()); }

    let mut batch = LlamaBatch::new(params.n_ctx as usize, 1);
    let last = n_prompt - 1;
    for (i, tok) in tokens.iter().enumerate() {
        batch.add(*tok, i as i32, &[0], i == last).map_err(|e| format!("batch add failed: {e}"))?;
    }
    ctx.decode(&mut batch).map_err(|e| format!("prompt decode failed: {e}"))?;

    let mut sampler = if params.temperature <= 0.01 {
        LlamaSampler::greedy()
    } else {
        LlamaSampler::chain_simple([LlamaSampler::temp(params.temperature), LlamaSampler::dist(1234)])
    };

    let mut n_cur = n_prompt as i32;
    let mut n_decoded = 0usize;
    while n_decoded < params.max_tokens {
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(token);
        if loaded.model.is_eog_token(token) { break; }
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let piece = loaded.model.token_to_piece(token, &mut decoder, false, None).unwrap_or_default();
        on_token(&piece);
        batch.clear();
        batch.add(token, n_cur, &[0], true).map_err(|e| format!("batch add (gen) failed: {e}"))?;
        n_cur += 1;
        n_decoded += 1;
        ctx.decode(&mut batch).map_err(|e| format!("gen decode failed: {e}"))?;
    }
    Ok(n_decoded)
}

/// Buffered convenience wrapper over [`generate_stream`].
#[cfg(feature = "llama-ffi")]
pub fn generate(loaded: &LoadedModel, prompt: &str, params: &GenParams) -> Result<String, String> {
    let mut out = String::new();
    generate_stream(loaded, prompt, params, |p| out.push_str(p))?;
    Ok(out)
}

#[cfg(not(feature = "llama-ffi"))]
pub fn generate_stream(_loaded: &LoadedModel, _prompt: &str, _params: &GenParams, _on_token: impl FnMut(&str)) -> Result<usize, String> {
    Err("not built with the `llamacpp` feature".into())
}
#[cfg(not(feature = "llama-ffi"))]
pub fn generate(_loaded: &LoadedModel, _prompt: &str, _params: &GenParams) -> Result<String, String> {
    Err("not built with the `llamacpp` feature".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn params_estimate() {
        assert_eq!(estimate_params_billions("qwen2.5-7b-instruct.gguf"), Some(7.0));
        assert_eq!(estimate_params_billions("Meta-Llama-3.1-8B.gguf"), Some(8.0));
        assert_eq!(estimate_params_billions("mixtral-8x7b.gguf"), Some(56.0));
        assert_eq!(estimate_params_billions("model-1.5b.gguf"), Some(1.5));
        assert_eq!(estimate_params_billions("whisper.bin"), None);
    }
    #[test]
    fn availability_matches_feature() {
        assert_eq!(is_available(), cfg!(feature = "llama-ffi"));
    }
    #[cfg(feature = "llama-ffi")]
    #[test]
    fn real_backend_inits_and_missing_model_errs() {
        // Exercises the REAL FFI at runtime: LlamaBackend::init() must succeed
        // and loading a nonexistent GGUF must return Err (not panic/abort).
        let r = load_model("/definitely/not/a/real/model.gguf", &GenParams::default());
        assert!(r.is_err(), "expected load error for missing file, got Ok");
    }
    #[test]
    fn scan_finds_gguf() {
        let dir = std::env::temp_dir().join(format!("ls_gguf_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("foo-7b.gguf"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let v = list_local_gguf(&dir);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "foo-7b");
        assert_eq!(v[0].params_billions, 7.0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
