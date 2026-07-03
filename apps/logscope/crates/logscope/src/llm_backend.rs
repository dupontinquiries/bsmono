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
    /// Continuous-decode budget for a fresh reply: when auto-continue is on the engine decodes up to
    /// this many tokens in ONE session (no re-prefill segmentation), stopping early on eos. Equals
    /// `max_tokens` when auto-continue is off. `max_tokens` remains the per-chunk / manual-continue size.
    pub reply_ceiling: usize,
    pub n_ctx: u32,
    /// MUST equal n_ctx: we feed the whole prompt in one batch to avoid the
    /// `GGML_ASSERT(n_tokens_all <= n_batch)` process-abort in llama.cpp.
    pub n_batch: u32,
    pub temperature: f32,
    /// Repetition penalty (1.0 = off) and min-p nucleus floor (0.0 = off) — small-model
    /// coherence levers consumed by the wgpu engine's sampler.
    pub repeat_penalty: f32,
    pub min_p: f32,
    pub n_threads: i32,
    /// -1 = offload all layers to the GPU.
    pub n_gpu_layers: i32,
}
impl Default for GenParams {
    fn default() -> Self {
        Self { max_tokens: 512, reply_ceiling: 512, n_ctx: 4096, n_batch: 4096, temperature: 0.7, repeat_penalty: 1.15, min_p: 0.05, n_threads: 4, n_gpu_layers: -1 }
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

/// Recursively walk a directory for `*.gguf` files (bounded depth), sorted by name.
/// Handles nested model collections and the HuggingFace cache layout (where the file is
/// a symlink named `model.gguf` under `models--org--Name/snapshots/<hash>/`).
pub fn list_local_gguf(dir: &Path) -> Vec<LocalModel> {
    let mut out = Vec::new();
    walk_gguf(dir, 0, &mut out);
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

fn walk_gguf(dir: &Path, depth: usize, out: &mut Vec<LocalModel>) {
    if depth > 6 { return; } // bound recursion against pathological trees / symlink cycles
    let rd = match std::fs::read_dir(dir) { Ok(r) => r, Err(_) => return };
    for e in rd.flatten() {
        let p = e.path();
        // Recurse into real subdirectories only (not symlinked dirs — avoids cycles);
        // skip hidden dirs. Symlinked *files* (HF cache) fall through to the gguf check.
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            let hidden = p.file_name().and_then(|s| s.to_str()).map(|s| s.starts_with('.')).unwrap_or(false);
            if !hidden { walk_gguf(&p, depth + 1, out); }
        } else if p.extension().and_then(|x| x.to_str()) == Some("gguf") {
            // follow symlinks for the real size (DirEntry::metadata does not)
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            let name = smart_model_name(&p);
            let pb = estimate_params_billions(&p.to_string_lossy()).unwrap_or(0.0);
            out.push(LocalModel { name, path: p, size_bytes: size, params_billions: pb });
        }
    }
}

/// A readable label for a discovered model. Uses the file stem when it's descriptive;
/// when the stem is generic (HF cache stores files as `model.gguf`), climbs to the first
/// meaningful ancestor directory — skipping `snapshots`/`blobs`/hashes and unwrapping the
/// `models--org--Name` cache naming into `org/Name`.
fn smart_model_name(p: &Path) -> String {
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("model").to_string();
    let generic = stem.len() < 4
        || matches!(stem.as_str(), "model" | "ggml-model" | "ggml-model-f16" | "ggml-model-q4_0" | "pytorch_model" | "consolidated");
    if !generic { return stem; }
    for anc in p.ancestors().skip(1) {
        if let Some(name) = anc.file_name().and_then(|s| s.to_str()) {
            let lower = name.to_ascii_lowercase();
            let is_hash = name.len() >= 12 && name.chars().all(|c| c.is_ascii_hexdigit());
            if is_hash || matches!(lower.as_str(), "snapshots" | "blobs" | "refs") { continue; }
            let cleaned = name.strip_prefix("models--").unwrap_or(name).replace("--", "/");
            return format!("{cleaned} ({stem})");
        }
    }
    stem
}

// ── load / generate ──────────────────────────────────────────────────────────
#[cfg(feature = "llama-ffi")]
fn shared_backend() -> Result<Arc<llama_cpp_2::llama_backend::LlamaBackend>, String> {
    use llama_cpp_2::llama_backend::LlamaBackend;
    thread_local! {
        static BACKEND: std::cell::RefCell<Option<Arc<LlamaBackend>>> = const { std::cell::RefCell::new(None) };
    }
    BACKEND.with(|b| {
        if let Some(existing) = b.borrow().as_ref() { return Ok(existing.clone()); }
        let arc = Arc::new(LlamaBackend::init().map_err(|e| format!("llama backend init failed: {e}"))?);
        *b.borrow_mut() = Some(arc.clone());
        Ok(arc)
    })
}
#[cfg(feature = "llama-ffi")]
pub fn load_model(path: &str, params: &GenParams) -> Result<LoadedModel, String> {
    use llama_cpp_2::model::params::LlamaModelParams;
    use llama_cpp_2::model::LlamaModel;

    if !std::path::Path::new(path).exists() {
        return Err(format!("model file not found: {path}"));
    }
    // LlamaBackend::init() touches process-global ggml state and errors with
    // BackendAlreadyInitialized if called a second time — so loading a 2nd model used to fail.
    // Init it once and hand out clones of the same Arc. All llama.cpp model ops run on the single
    // worker thread, so a thread-local cell is sufficient (and sidesteps LlamaBackend: !Sync).
    let backend = shared_backend()?;
    let model_params = LlamaModelParams::default().with_n_gpu_layers(params.n_gpu_layers as u32);
    let model = LlamaModel::load_from_file(&backend, path, &model_params)
        .map_err(|e| format!("failed to load GGUF '{path}': {e}"))?;
    let params_billions = estimate_params_billions(path).unwrap_or(0.0);
    Ok(LoadedModel { model: Arc::new(model), backend, path: path.to_string(), params_billions })
}
#[cfg(not(feature = "llama-ffi"))]
pub fn load_model(_path: &str, _params: &GenParams) -> Result<LoadedModel, String> {
    Err("logscope was built without the `llamacpp` feature; rebuild with `--features llamacpp`".into())
}

/// Streaming completion: invokes `on_token` for each decoded piece and returns
/// the number of tokens generated. `generate` is the buffered wrapper.
#[cfg(feature = "llama-ffi")]
pub fn generate_stream(loaded: &LoadedModel, prompt: &str, params: &GenParams, cancel: impl Fn() -> bool, mut on_token: impl FnMut(&str) -> bool) -> Result<usize, String> {
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

    // Repetition penalty (params.repeat_penalty, default 1.15) FIRST in the chain — without it small/quant
    // models (e.g. Gemma) run away into repeated loops. penalty_last_n=64; freq/present penalties off.
    let penalties = LlamaSampler::penalties(64, params.repeat_penalty, 0.0, 0.0);
    let mut sampler = if params.temperature <= 0.01 {
        LlamaSampler::chain_simple([penalties, LlamaSampler::greedy()])
    } else {
        LlamaSampler::chain_simple([penalties, LlamaSampler::temp(params.temperature), LlamaSampler::dist(1234)])
    };

    let mut n_cur = n_prompt as i32;
    let mut n_decoded = 0usize;
    // Decode continuously up to the reply ceiling (auto-continue budget), but never past the KV
    // window: cap to the room left after the prompt so llama.cpp can't overrun n_ctx.
    let ceiling = params.reply_ceiling.max(params.max_tokens)
        .min((params.n_ctx as usize).saturating_sub(n_prompt).max(1));
    while n_decoded < ceiling {
        if cancel() { break; } // stop button / barge-in flipped the flag → halt promptly
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(token);
        if loaded.model.is_eog_token(token) { break; }
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let piece = loaded.model.token_to_piece(token, &mut decoder, false, None).unwrap_or_default();
        if !on_token(&piece) { break; } // consumer asked to stop
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
    generate_stream(loaded, prompt, params, || false, |p| { out.push_str(p); true })?;
    Ok(out)
}

#[cfg(not(feature = "llama-ffi"))]
pub fn generate_stream(_loaded: &LoadedModel, _prompt: &str, _params: &GenParams, _cancel: impl Fn() -> bool, _on_token: impl FnMut(&str) -> bool) -> Result<usize, String> {
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

