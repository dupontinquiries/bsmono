//! backend.rs — pluggable inference backends behind one `Backend` trait, driven
//! by a worker thread that owns the (often !Send) model. The UI talks to it over
//! channels and never blocks the frame. GenParams (context window, reply budget,
//! temperature) are app-controlled and pushed to the worker live.
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use logscope::llm_backend::{self, GenParams, LoadedModel};

// ── the abstraction ──────────────────────────────────────────────────────────
/// A loadable, streaming text-generation backend. Not `Send`: the worker thread
/// constructs and owns it locally (llama.cpp's handle is !Send), so it never
/// crosses a thread boundary.
pub trait Backend {
    fn name(&self) -> &'static str;
    fn loaded(&self) -> bool;
    fn load(&mut self, path: &str, params: &GenParams) -> Result<(), String>;
    fn generate(&mut self, prompt: &str, params: &GenParams, cancel: &dyn Fn() -> bool, on_token: &mut dyn FnMut(&str) -> bool) -> Result<usize, String>;
    /// Generate replies for several prompts at once. Default loops `generate` (so every backend works);
    /// the wgpu engine OVERRIDES this to decode them in one batched pass — the fan-out payoff, since a
    /// batch-1 decode barely uses the GPU. Used by the runner's `:fa`/`:ca`. No streaming (replies whole).
    fn generate_batched(&mut self, prompts: &[String], params: &GenParams, cancel: &dyn Fn() -> bool) -> Vec<String> {
        prompts.iter().map(|p| {
            let mut out = String::new();
            let _ = self.generate(p, params, cancel, &mut |s| { out.push_str(s); true });
            out.trim().to_string()
        }).collect()
    }
    /// Resume the last generation from warm KV state (no re-prefill), for auto-continue. None if
    /// this backend can't resume in place — the worker then falls back to a prompt-based generate.
    fn continue_last(&mut self, _params: &GenParams, _cancel: &dyn Fn() -> bool, _on: &mut dyn FnMut(&str) -> bool) -> Option<Result<usize, String>> { None }
    /// Chat-template family the loaded model expects, so the app builds a matching prompt.
    /// Defaults to Plain; the wgpu engine detects the real family from the tokenizer.
    fn template(&self) -> TemplateKind { TemplateKind::Plain }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BackendKind { LlamaCpp, Wgpu }
impl BackendKind {
    pub const ALL: [BackendKind; 2] = [BackendKind::LlamaCpp, BackendKind::Wgpu];
    pub fn label(self) -> &'static str {
        match self { BackendKind::LlamaCpp => "llama.cpp · Vulkan", BackendKind::Wgpu => "wgpu/Burn (experimental)" }
    }
    /// The backend to use by default: llama.cpp when its FFI is compiled in (mature,
    /// fast), otherwise the clean-room wgpu engine. Loading and chatting must agree on
    /// this so the chat talks to the backend that actually holds the loaded model.
    pub fn preferred() -> Self {
        if llm_backend::is_available() { BackendKind::LlamaCpp } else { BackendKind::Wgpu }
    }
}

/// llama.cpp / GGML backend (Vulkan when built `--features llamacpp`).
struct LlamaCpp { model: Option<LoadedModel> }
impl Backend for LlamaCpp {
    fn name(&self) -> &'static str { "llama.cpp" }
    fn loaded(&self) -> bool { self.model.is_some() }
    fn load(&mut self, path: &str, params: &GenParams) -> Result<(), String> {
        self.model = Some(llm_backend::load_model(path, params)?); Ok(())
    }
    fn generate(&mut self, prompt: &str, params: &GenParams, cancel: &dyn Fn() -> bool, on: &mut dyn FnMut(&str) -> bool) -> Result<usize, String> {
        // Cancellation now threads through: the decode loop checks `cancel()` each token and stops when `on`
        // returns false, so the Stop button and voice barge-in halt llama.cpp generation.
        match &self.model {
            Some(m) => llm_backend::generate_stream(m, prompt, params, || cancel(), |s| on(s)),
            None => Err("no model loaded".into()),
        }
    }
}

/// wgpu clean-room backend — drives the pure-Rust GPU engine in `gpu-gemm`
/// (GGUF load → resident weights → multi-head forward → KV-cache decode →
/// tokenizer/sampler). Built only under `--features wgpu-engine`; otherwise it's
/// a clear stub so the default build stays light. Burn lives in `burn-probe/` as
/// a Vulkan correctness oracle, not a shipped backend.
#[cfg(feature = "wgpu-engine")]
pub(crate) mod engine {
    use super::TemplateKind;
    use gpu_kernels::tokenizer::Tokenizer;
    use gpu_kernels::{Gpu, Model, SamplerParams, Session};
    use logscope::llm_backend::GenParams;
    use std::time::Instant;
    /// Prefill block size. Attention scratch peaks at O(PREFILL_CHUNK · prompt_len), so this bounds
    /// the memory a long context can demand (vs O(prompt_len²) for a single batched prefill).
    const PREFILL_CHUNK: usize = 256;
    /// Hard ceiling on the context window regardless of the n_ctx slider — the KV cache is O(window)
    /// memory (~hundreds of KB per token), so this is the backstop against OOMing the device.
    const MAX_CONTEXT: usize = 32768;
    pub struct Engine {
        // gpu / model / tok are shared, read-only-after-load state (RULESET K9: weights never mutate
        // during inference) — behind `Arc` so `lane()` can derive sibling engines that reuse the SAME
        // resident weights, device, and pipeline cache. Per-lane state is everything below: the KV
        // anchors, the rng, the tuner. This is PREWARM.md's "an instance is a session, not a second
        // copy of the weights", applied to the in-app engine.
        gpu: std::sync::Arc<Gpu>, model: std::sync::Arc<Model>, tok: std::sync::Arc<Tokenizer>,
        eos: Option<u32>, rng: u64,
        /// Every token that ends a turn for this model — the GGUF-declared eos plus all stop
        /// specials. The model may emit ANY of them (llama.cpp's EOG set for DeepSeek-R1-Distill is
        /// {128001,128008,128009}); watching only one (the old <|eot_id|> bug) misses the real stop
        /// and loops. The decode callbacks halt on this whole set.
        stop_ids: Vec<u32>,
        /// Chat-template family, reported to the UI so it frames the prompt to match the model.
        template: TemplateKind,
        /// KV-cache PREFIX TREE: a small pool of `(full token context, its prefilled session, batch-
        /// prefilled length)` — one per recent branch tip, not just the single last turn. On a new prompt
        /// the engine reuses the entry sharing the longest usable prefix (see `kvtree::pick_anchor`), so a
        /// sibling branch — or N `:continue-all` continuations fanned off one leaf — reuses that branch's
        /// KV instead of re-prefilling from scratch (1 prefill + N decodes for a fan-out). The third field
        /// is how many leading positions were written by a *batched* prefill (vs extended by per-token
        /// decode_step); we only ever seed from that pure-prefill region — seeding decode-written rows is a
        /// path kernel-validate doesn't guard and can drift at scale, so capping reuse to the prefill
        /// boundary keeps every reuse bit-exact. `kv_cap` bounds the pool (each entry holds a full KV
        /// cache, so the pool trades memory for fewer prefills); default 3 keeps the last few branch
        /// tips warm (the common fan-out/sibling-switch win), tuned by `LOGSCOPE_KV_TREE` (=1 restores
        /// the original single-anchor behaviour; newest evicted last / oldest first).
        anchors: Vec<(Vec<u32>, Session, usize)>,
        kv_cap: usize,
        warm: usize,
        /// The loaded model's file name (basename) — printed on each generation log line so a log
        /// reader can tell which model produced a reply (e.g. after switching models mid-session).
        name: String,
        /// Adapts passes-per-submit across replies (sweeps SUBMIT_PROFILES, settles on the fastest by
        /// per-token decode time, caches it). None when env-pinned, cached, or autotune is disabled.
        submit_tuner: Option<gpu_kernels::SubmitTuner>,
        /// Int8-resident embedding table, built lazily on the first windowed sampled decode
        /// (`LOGSCOPE_SAMPLE_WINDOW`, default 4): the GPU-side row gather that keeps a whole sample→
        /// embed→decode window on-device. ~350 MB for a 151k×2048 vocab (vs 1.2 GB f32); skipped
        /// entirely when the window is disabled or the sampler is ineligible.
        embed_q8: Option<gpu_kernels::QWeight>,
        /// Optional small DRAFT model for speculative decoding (opt-in via `LOGSCOPE_SPEC_DRAFT`).
        /// When present, `generate` routes through the engine's `speculative_generate_stats`: the
        /// draft proposes a block of tokens that the target verifies in ONE forward — bit-exact to
        /// greedy decode, but more tokens per expensive target pass. Must share the target's vocab.
        draft: Option<Model>,
    }
    impl Engine {
        pub fn load(path: &str) -> Result<Self, String> {
            // mmap the GGUF (don't RAM-load it): a 40 GB file would OOM on the read alone, and even a
            // cache HIT shouldn't pull GB into RAM just to compute a content key. The OS pages bytes
            // on demand; `bytes` derefs to &[u8] exactly like the old fs::read Vec, and the mapping
            // is dropped at the end of load once tensors are resident.
            let mm = gpu_kernels::file_bytes(path).map_err(|e| format!("open {path}: {e}"))?;
            let bytes: &[u8] = &mm;
            let gpu = Gpu::new();
            eprintln!("wgpu-engine: adapter = {}", gpu.adapter_name);
            let g = gpu_kernels::gguf::parse(&bytes)?;
            // Pre-flight: the loader auto-selects q4-resident (≈half the int8 footprint) when int8 won't
            // fit, and falls back to q4 on a GPU OOM. So only refuse when even q4 can't fit the budget;
            // otherwise let the loader pick. LOGSCOPE_FORCE_LOAD=1 bypasses; LOGSCOPE_MAX_RESIDENT_GB sets
            // the budget; LOGSCOPE_Q4_RESIDENT=1 forces q4 up front.
            if std::env::var("LOGSCOPE_FORCE_LOAD").is_err() {
                if let (Some(est), Some(budget)) = (gpu_kernels::store::estimate_int8_resident(&g), gpu_kernels::store::resident_budget_bytes()) {
                    let q4_est = est / 2; // q4-resident is ≈half of int8
                    if q4_est > budget {
                        let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
                        return Err(format!(
                            "won't fit: ~{:.1} GiB needed even at q4-resident, ~{:.1} GiB available. Try a \
                             smaller model, free memory, or LOGSCOPE_FORCE_LOAD=1 to try anyway (likely OOM).",
                            gib(q4_est), gib(budget)));
                    }
                }
            }
            let tok = Tokenizer::from_gguf(&g).map_err(|e| format!("tokenizer: {e}"))?;
            // Resolve the stop the way the harness does: the GGUF-declared eos FIRST (DeepSeek-R1-
            // Distill declares 128001 <｜end▁of▁sentence｜>), THEN fall back to a stop special. The
            // old order preferred <|eot_id|>(128009) and so missed DeepSeek's real stop → the
            // whole-answer loop. The primary `eos` drives the engine's internal stop; `stop_ids` is
            // the full set every decode callback also checks (covers eot/eom/end_of_text/im_end).
            let declared = g.get_u32("tokenizer.ggml.eos_token_id");
            let eos = declared.or_else(|| tok.special_id("<|eot_id|>"));
            let stop_ids: Vec<u32> = crate::genguard::stop_token_set(declared, &[
                tok.special_id("<|eot_id|>"),
                tok.special_id("<|eom_id|>"),
                tok.special_id("<|end_of_text|>"),
                tok.special_id("<|im_end|>"),
                tok.special_id("<｜end▁of▁sentence｜>"),
                tok.special_id("<|endoftext|>"),
                // Turn-START markers are also stops: in a single assistant turn the model must never open a
                // new turn — emitting one means it ran past its real stop and is hallucinating the next turn
                // (the "untamed stopping" runaway). These now tokenize atomically, so the id-based decode
                // stop check halts cleanly instead of leaking "<|im_start|>user…" into the reply.
                tok.special_id("<|im_start|>"),
                tok.special_id("<|start_header_id|>"),
                // Llama4 (Scout/Maverick): <|eot|>/<|eom|> end a turn; <|header_start|> opens one.
                tok.special_id("<|eot|>"),
                tok.special_id("<|eom|>"),
                tok.special_id("<|header_start|>"),
            ]).into_iter().collect();
            // Template family: a ChatML model carries <|im_end|> but none of the Llama-3 header
            // markers; a Llama-3 model carries <|eot_id|>/<|start_header_id|>. Anything else → Plain.
            let template = if tok.special_id("<|header_start|>").is_some() {
                TemplateKind::Llama4 // Llama4: <|header_start|>role<|header_end|> … <|eot|>
            } else if tok.special_id("<|im_end|>").is_some()
                && tok.special_id("<|start_header_id|>").is_none() {
                TemplateKind::ChatML
            } else if tok.special_id("<|eot_id|>").is_some() || tok.special_id("<|start_header_id|>").is_some() {
                TemplateKind::Llama3
            } else if tok.special_id("<|turn>").is_some() || tok.special_id("<turn|>").is_some() {
                TemplateKind::Gemma // Gemma 4 turn markers (changed from Gemma 3's <start_of_turn>/<end_of_turn>)
            } else {
                TemplateKind::Plain
            };
            let t0 = Instant::now();
            // Content-addressed central cache (Batch 1): key the derived int8 blob by the model's
            // CONTENT and keep it in one shared dir, so the same GGUF across projects dedups to one
            // entry and the blobs stop piling up next to every model. fp's 2nd field is the content
            // key (not mtime), so a reload matches regardless of path/mtime; a changed model →
            // different key → automatic miss. LOGSCOPE_CACHE_DIR / LOGSCOPE_CACHE_MAX_GB tune it.
            let key = gpu_kernels::store::content_key(&bytes);
            let fp = (bytes.len() as u64, key);
            let _ = gpu_kernels::store::ensure_cache_dir();
            let cache = gpu_kernels::store::cache_path(key);
            // Fast path: load the ready int8 weights from the central blob and skip dequant entirely.
            let model = match gpu.load_int8_cache(&cache, fp) {
                Some(m) => { eprintln!("wgpu-engine: loaded int8 cache (skipped dequant) in {:.1}s [{}]", t0.elapsed().as_secs_f32(), cache.display()); m }
                None => {
                    eprintln!("wgpu-engine: loading {path} ({:.1} MiB) — quantizing to resident weights (int8, or q4 if it won't fit)…", bytes.len() as f32 / (1 << 20) as f32);
                    let m = gpu.load_gguf_quantized(&bytes)?;
                    // Write the central blob so the NEXT load (any project, incl. crash-recovery) skips
                    // this. Make room BEFORE writing (evict to cap − incoming) so the dir never briefly
                    // holds old+new; the post-write evict is then just an exact final guard.
                    let cap = gpu_kernels::store::cache_cap_bytes();
                    let incoming = gpu_kernels::store::estimate_int8_resident(&g).unwrap_or(0);
                    gpu_kernels::store::make_room_for(cap, incoming);
                    match gpu.save_int8_cache(&m, &cache, fp) {
                        Ok(true) => {
                            let (n, freed) = gpu_kernels::store::evict_to_cap(cap);
                            let evicted = if n > 0 { format!("; evicted {n}, freed {:.1} GiB", freed as f64 / (1u64 << 30) as f64) } else { String::new() };
                            eprintln!("wgpu-engine: wrote int8 cache [{}] (central, content-addressed){evicted}", cache.display());
                        }
                        Ok(false) => eprintln!("wgpu-engine: int8 cache skipped (bias/QK-Norm/MoE/q4 layouts aren't cacheable yet — reloads from the GGUF)"),
                        Err(e) => eprintln!("wgpu-engine: int8 cache write skipped: {e}"),
                    }
                    m
                }
            };
            // Batch 2: report the resident footprint (the int8 blob size ≈ GPU/UMA memory the model
            // occupies) so memory pressure is visible before loading a bigger model. The q4-resident
            // + mmap path for models that don't fit as int8 is the TODO that lets 30B run in 32 GB.
            if let Ok(md) = std::fs::metadata(&cache) {
                eprintln!("wgpu-engine: int8 resident footprint ≈ {:.2} GiB", md.len() as f64 / (1u64 << 30) as f64);
            }
            eprintln!("wgpu-engine: loaded in {:.1}s — d={} layers={} heads={}/{} head_dim={} rope_base={} kv ff={} vocab={} | output int8={} | eos={:?} stop={:?} template={:?} | tokenizer byte_level={} vocab={}",
                t0.elapsed().as_secs_f32(), model.d, model.n_layers, model.n_heads, model.n_kv_heads, model.head_dim, model.base, model.h, model.vocab,
                model.output_is_quantized(), eos, stop_ids, template, tok.is_byte_level(), tok.vocab_size());
            if std::env::var("LOGSCOPE_DISPATCH_BENCH").is_ok() { gpu.dispatch_bench(); }
            if std::env::var("LOGSCOPE_WEIGHT_CHECK").is_ok() { eprintln!("wgpu-engine: {}", model.weight_probe(&gpu)); }
            // Prewarm: compile the prefill + decode pipelines now (in the load window) so the
            // first user token runs at steady speed instead of crawling up from ~0.1 tok/s
            // while shaders compile. The returned count is the "warm" signal.
            let warm = gpu.prewarm(&model);
            eprintln!("wgpu-engine: prewarmed {warm} pipelines (prefill+decode kernels compiled at load)");
            // System-optimization: pick the fastest decode mat-vec tile for THIS GPU by a quick
            // benchmark on the model's dominant (FFN up) shape — runs only on first load per
            // (adapter, shape); the result is cached to disk so later loads stay snappy. Skipped when
            // LOGSCOPE_MATVEC_TILE pins it, or LOGSCOPE_NO_AUTOTUNE is set. Pure speed choice — the
            // output is bit-identical for any tile (--gate-matvec), so this can never change results.
            if !gpu.profile_forced() && std::env::var_os("LOGSCOPE_NO_AUTOTUNE").is_none() {
                let (k, n) = (model.d, 2 * model.h); // wgate_up: the largest decode mat-vec
                if let Some(t) = gpu_kernels::store::autotune_get(&gpu.adapter_name, k, n) {
                    gpu.set_matvec_tile(t);
                    eprintln!("wgpu-engine: decode tile = {t} (cached autotune for this GPU)");
                } else {
                    let t0 = std::time::Instant::now();
                    let best = gpu.autotune_matvec(k, n);
                    gpu.set_matvec_tile(best);
                    gpu_kernels::store::autotune_put(&gpu.adapter_name, k, n, best);
                    eprintln!("wgpu-engine: autotuned decode tile = {best} in {:.0}ms (cached; set LOGSCOPE_MATVEC_TILE to override, LOGSCOPE_NO_AUTOTUNE to skip)", t0.elapsed().as_secs_f32() * 1000.0);
                }
            } else if gpu.profile_forced() {
                eprintln!("wgpu-engine: decode tile = {} (LOGSCOPE_MATVEC_TILE override)", gpu.matvec_tile());
            }
            // Per-shape autotuning, beyond the single global tile above: the model's distinct mat-vec
            // shapes (attention QKVO, fused FFN gate/up, FFN-down, output) have different optimal tiles
            // because the activation-reuse payoff scales with k. Probe each DISTINCT shape once — decode
            // tile + prefill GEMM tile — and cache to disk keyed by this adapter, reused across launches
            // and across models that share a shape. Bit-exact across tiles (--gate-gemm-spec / -matvec),
            // so this is purely a speed choice. Logged so it's observable on real hardware.
            if std::env::var_os("LOGSCOPE_NO_AUTOTUNE").is_none() {
                let shapes = gpu.model_decode_shapes(&model);
                let safe: String = gpu.adapter_name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
                let cache = gpu_kernels::store::cache_root().join(format!("autotune-shapes-{safe}.bin"));
                let (dl, gl) = gpu.load_autotune(&cache).unwrap_or((0, 0));
                if dl > 0 || gl > 0 { eprintln!("wgpu-engine: per-shape autotune — loaded {dl} decode + {gl} GEMM tiles from cache"); }
                // Time-box the whole probe so a slow/large model can never stall the load (capped-width
                // probes keep each shape cheap; this is the hard backstop).
                let t0 = std::time::Instant::now();
                let deadline = t0 + std::time::Duration::from_secs(20);
                let dec = gpu.autotune_shapes(&shapes, Some(deadline));            // decode tiles (uncached)
                let m_probe = model.d.clamp(64, 512);                             // representative prefill width
                // The GEMM tile only matters when the tiled prefill is enabled, so only probe it then.
                let gem = if std::env::var_os("LOGSCOPE_PREFILL_TILED").is_some() { gpu.autotune_gemm(&shapes, m_probe, Some(deadline)) } else { Vec::new() };
                for (n, k, t, ms) in &dec { eprintln!("wgpu-engine:   probed decode n={n} k={k} → tile {t} in {ms:.0}ms"); }
                for (n, k, t, ms) in &gem { eprintln!("wgpu-engine:   probed GEMM   n={n} k={k} → tile {t} in {ms:.0}ms"); }
                if !dec.is_empty() || !gem.is_empty() {
                    let _ = gpu.save_autotune(&cache);
                    let total: f32 = dec.iter().chain(gem.iter()).map(|x| x.3).sum();
                    eprintln!("wgpu-engine: per-shape autotune probed {} decode + {} GEMM shapes ({:.0}ms work, {:.1}s wall; cached to {})",
                        dec.len(), gem.len(), total, t0.elapsed().as_secs_f32(), cache.display());
                    if std::time::Instant::now() >= deadline { eprintln!("wgpu-engine: NOTE — autotune hit its 20s time-box; remaining shapes use defaults (re-run to finish, or pin LOGSCOPE_*_TILE)"); }
                }
                // Print the active per-shape table (covers the cached case too) so the chosen tiles are visible.
                let tiled = std::env::var_os("LOGSCOPE_PREFILL_TILED").is_some();
                let mut seen = std::collections::HashSet::new();
                for (n, k) in shapes.iter().copied() {
                    if seen.insert((n, k)) {
                        let gemm = if tiled { format!(" · GEMM tile {}", gpu.gemm_tile_for(n, k)) } else { String::new() };
                        eprintln!("wgpu-engine:   shape n={n} k={k} → decode tile {}{gemm}", gpu.decode_tile_for(n, k));
                    }
                }
                eprintln!("wgpu-engine: tile overrides — LOGSCOPE_DECODE_TILE / LOGSCOPE_GEMM_TILE pin, LOGSCOPE_NO_AUTOTUNE skips, LOGSCOPE_PREFILL_TILED=1 enables the tiled prefill GEMM");
            }
            // Submit batching: a 14B decode is ~24 submits/token and the profile is sync-dominated, so the
            // passes-per-submit knob matters. Pinned (env) → fixed; cached → use it; else adapt across the
            // next replies (SubmitTuner sweeps SUBMIT_PROFILES by per-token decode time). Bit-exact across
            // flush values (--gate-submit), so adapting between replies never changes output.
            let submit_tuner = if gpu.flush_every_pinned() {
                eprintln!("wgpu-engine: submit batch = {} passes/submit (LOGSCOPE_FLUSH_EVERY override)", gpu.flush_every());
                None
            } else if let Some(v) = gpu_kernels::store::flush_get(&gpu.adapter_name) {
                gpu.set_flush_every(v);
                eprintln!("wgpu-engine: submit batch = {v} passes/submit (cached autotune for this GPU)");
                None
            } else if std::env::var_os("LOGSCOPE_NO_AUTOTUNE").is_some() {
                None
            } else {
                // Startup stride probe: measure the whole ladder synthetically (~1 s worst-case, once per
                // GPU until the live tuner's settle is cached), start at the winner, and hand the LIVE
                // tuner only the winner's neighborhood — so real replies begin near-optimal and settling
                // takes ~3 candidates instead of the full ladder. LOGSCOPE_FLUSH_EVERY still pins.
                let probe = gpu.submit_stride_probe(gpu_kernels::SUBMIT_PROFILES);
                let table: Vec<String> = probe.iter().map(|(c, ms)| format!("{c}:{ms:.0}ms")).collect();
                let winner = probe.first().map(|&(c, _)| c).unwrap_or(64);
                // Winner's neighborhood, plus winner/4: the probe ranks by raw speed on synthetic tiny
                // passes, but the LIVE tuner settles under the per-submit smoothness budget — it needs a
                // clearly-smoother candidate in its set to fall to when the raw winner's submits run long.
                let neighborhood: Vec<u32> = gpu_kernels::SUBMIT_PROFILES.iter().cloned()
                    .filter(|&c| c == winner || c == winner.saturating_mul(2) || c == winner / 2 || c == winner / 4)
                    .collect();
                let t = gpu_kernels::SubmitTuner::new(
                    Box::leak(neighborhood.clone().into_boxed_slice()), 2);
                gpu.set_flush_every(winner);
                eprintln!("wgpu-engine: submit batch = {winner} passes/submit (startup stride probe [{}]); live tuner refines {:?} over next replies; pin with LOGSCOPE_FLUSH_EVERY", table.join(" "), neighborhood);
                Some(t)
            };
            let name = std::path::Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path).to_string();
            // KV prefix-tree pool size. Unset → 3: a small pool keeps the last few branch tips warm so
            // sibling switches and `:continue-all` fan-outs reuse their lineage's KV (1 prefill + N
            // decodes) instead of re-prefilling from scratch every time — the common branchy-chat win,
            // on by default. Each entry holds a full KV cache, so this trades a few hundred MB–GB of
            // memory per anchor for fewer prefills; LOGSCOPE_KV_TREE=N tunes the pool (=1 restores the
            // original single-anchor behaviour) and LOGSCOPE_KV_OFF disables reuse entirely. Reuse stays
            // bit-exact regardless of pool size: only the pure-prefill region of an anchor is ever seeded.
            let kv_cap = std::env::var("LOGSCOPE_KV_TREE").ok()
                .map(|v| v.trim().parse::<usize>().unwrap_or(6).max(1)).unwrap_or(3);
            if std::env::var("LOGSCOPE_KV_OFF").is_ok() { eprintln!("wgpu-engine: KV reuse DISABLED (LOGSCOPE_KV_OFF) — full prefill every turn, graph speedups off"); }
            else if kv_cap > 1 { eprintln!("wgpu-engine: KV prefix tree ON — pool cap {kv_cap} (each entry holds a KV cache; trades memory for fewer prefills on branchy/fan-out chats)"); }
            // Speculative decoding (opt-in): if LOGSCOPE_SPEC_DRAFT points at a GGUF, load it as a
            // small DRAFT model. It must share the target's vocab/tokenizer (same family, smaller
            // size — e.g. a 1B draft for an 8B target). `generate` then runs the engine's
            // speculative_generate_stats (draft proposes, target verifies a block per forward).
            let draft = match std::env::var("LOGSCOPE_SPEC_DRAFT") {
                Ok(dpath) if !dpath.trim().is_empty() => {
                    let load_draft = || -> Result<Model, String> {
                        let dmm = gpu_kernels::mmap_ro(&dpath).map_err(|e| format!("spec draft mmap {dpath}: {e}"))?;
                        let dm = gpu.load_gguf_quantized(&dmm[..]).map_err(|e| format!("spec draft load {dpath}: {e}"))?;
                        Ok(dm)
                    };
                    match load_draft() {
                        Ok(dm) if dm.vocab == model.vocab => {
                            let base = std::path::Path::new(&dpath).file_name().and_then(|s| s.to_str()).unwrap_or(&dpath);
                            gpu.prewarm(&dm);
                            eprintln!("wgpu-engine: spec-decode ENABLED — draft {base} (d={} L={} vocab={}) proposes, target verifies; tune the block with LOGSCOPE_SPEC_K (default 4)", dm.d, dm.n_layers, dm.vocab);
                            Some(dm)
                        }
                        Ok(dm) => { eprintln!("wgpu-engine: spec draft vocab {} != target {} — spec-decode OFF (draft and target must share a tokenizer)", dm.vocab, model.vocab); None }
                        Err(e) => { eprintln!("wgpu-engine: spec draft load failed — spec-decode OFF: {e}"); None }
                    }
                }
                _ => None,
            };
            Ok(Self { gpu: std::sync::Arc::new(gpu), model: std::sync::Arc::new(model), tok: std::sync::Arc::new(tok), eos, stop_ids, template, rng: 0x2545_F491_4F6C_DD1D, anchors: Vec::new(), kv_cap, warm, name, submit_tuner, embed_q8: None, draft })
        }
        pub fn template(&self) -> TemplateKind { self.template }
        /// The loaded model's file basename (log/UI tag).
        pub fn model_name(&self) -> &str { &self.name }
        /// Derive a sibling engine that SHARES this one's resident weights, device, and pipeline cache
        /// (all read-only after load — RULESET K9), with its own KV anchors, rng stream, and log tag.
        /// This is what makes N concurrent lanes cost N KV caches instead of N model copies: spinning a
        /// lane is O(ms) (no GGUF read, no dequant, no pipeline compile), and every lane's submits
        /// interleave on the one shared queue — the submit governor keeps any single lane from
        /// monopolizing it. Lanes carry NO submit tuner (the primary tunes the shared flush_every, and
        /// concurrent sweeps would fight) and NO draft model (speculative decode stays a primary-lane
        /// feature); kv_cap is 1 because a lane runs one linear task, so one warm resume anchor is the
        /// whole win and the pool's memory stays bounded at ~one KV cache per lane.
        pub fn lane(&self, n: usize) -> Engine {
            Engine {
                gpu: self.gpu.clone(), model: self.model.clone(), tok: self.tok.clone(),
                eos: self.eos, stop_ids: self.stop_ids.clone(), template: self.template,
                rng: self.rng ^ 0x9E37_79B9_7F4A_7C15u64.wrapping_mul(n as u64 + 1),
                anchors: Vec::new(), kv_cap: 1, warm: self.warm,
                name: format!("{}#L{n}", self.name),
                submit_tuner: None, embed_q8: None, draft: None,
            }
        }

        /// Windowed sampled decode: is it eligible for `sp`, and how big a window? Shared by BOTH
        /// the fresh-prompt path (`generate`) and the warm-resume path (`continue_last`) so they can
        /// never drift — the auto-continue silently reverting to token-by-token (36 submits/token on
        /// Metal, where encoder setup is the whole wall) was exactly that drift. Builds the int8
        /// embed table lazily on first eligible use. Returns the window size, or None to take the
        /// per-token path. Eligibility mirrors SAMPLE_AT's host contract (temp>0, 2≤top_k≤64).
        fn windowed_ready(&mut self, sp: &SamplerParams) -> Option<usize> {
            let sample_win = std::env::var("LOGSCOPE_SAMPLE_WINDOW").ok()
                .and_then(|v| v.parse::<usize>().ok()).unwrap_or(4);
            if !(sample_win >= 2 && sp.temperature > 0.0 && (2..=64).contains(&sp.top_k)) { return None; }
            if self.embed_q8.is_none() {
                let tq = Instant::now();
                self.embed_q8 = Some(self.gpu.embed_q8(&self.model));
                eprintln!("wgpu-engine: [{}] windowed sampling ON (k={sample_win}) — int8 embed table resident ({:.0} MiB, {:.2}s)",
                    self.name, (self.model.vocab * self.model.d) as f32 / (1024.0 * 1024.0), tq.elapsed().as_secs_f32());
            }
            Some(sample_win)
        }

        /// Token-level generation for a fabric Worker: prefill `prefix`, decode `max_new` tokens with
        /// `temperature`/`seed`, halting on the model's stop set. Tokens in → tokens out, `&self` so
        /// one resident model can serve many prefixes. The portable unit is tokens (KV stays local).
        pub fn run_tokens(&self, prefix: &[u32], max_new: usize, temperature: f32, seed: u64) -> Vec<u32> {
            let sp = SamplerParams { temperature, top_k: 40, top_p: 0.95, repeat_penalty: 1.15, repeat_last_n: 64, min_p: 0.05 };
            self.gpu.run_tokens(&self.model, prefix, max_new, &sp, &self.stop_ids, seed)
        }
        pub fn generate(&mut self, prompt: &str, p: &GenParams, cancel: &dyn Fn() -> bool, on: &mut dyn FnMut(&str) -> bool) -> Result<usize, String> {
            let toks = self.tok.encode(prompt);
            let mut prompt_ids = if toks.is_empty() { vec![0] } else { toks };
            // Diagnostic: scan the ENCODED prompt for special tokens + show its tail. A correct ChatML
            // prompt has exactly 2 specials per turn + 1 (the trailing <|im_start|>), and the tail must
            // end on the assistant primer. EXTRA specials = stray markers from injected content (a log
            // line containing "<|im_end|>" etc.) silently turned into control tokens, which corrupts the
            // turn structure and makes the model emit its own <|im_start|> — exactly the early-stop seen.
            {
                let sp: Vec<(usize, u32)> = prompt_ids.iter().enumerate()
                    .filter(|(_, &t)| self.tok.is_special(t)).map(|(i, &t)| (i, t)).collect();
                let tail: Vec<u32> = prompt_ids.iter().rev().take(12).rev().copied().collect();
                eprintln!("wgpu-engine: [{}] prompt diag — {} special tokens (showing ≤12): {:?}; tail ids = {:?}",
                    self.name, sp.len(), &sp[..sp.len().min(12)], tail);
            }
            let sp = SamplerParams { temperature: p.temperature, top_k: 40, top_p: 0.95, repeat_penalty: p.repeat_penalty, repeat_last_n: 64, min_p: p.min_p };
            // Decode budget for this reply. With auto-continue on, `reply_ceiling` is the per-reply
            // ceiling and we decode it in ONE continuous session (no re-prefill segmentation).
            let ceiling = p.reply_ceiling.max(p.max_tokens).max(1);
            // Bound the prompt to the context WINDOW. The KV cache is O(prompt) memory (~hundreds of
            // KB/token), allocated up front in session_new — so an unbounded prompt (a huge paste)
            // OOMs the device before any compute. Reserve only a MINIMUM reply slice when fitting the
            // prompt, so a long prompt isn't over-truncated to satisfy a large continuous ceiling; the
            // reply budget then yields to the prompt and fills whatever room remains. On overflow keep
            // the head (system prompt) + most recent tail, dropping the middle — standard window shift.
            let window = (p.n_ctx as usize).clamp(1024, MAX_CONTEXT);
            let min_reply = ceiling.min(256);
            let max_prompt = window.saturating_sub(min_reply).max(256);
            if prompt_ids.len() > max_prompt {
                let head = (max_prompt / 3).min(1024);            // keep the system prompt
                let tail = max_prompt - head;                     // keep the most recent context
                let orig = prompt_ids.len();
                let mut kept = prompt_ids[..head].to_vec();
                kept.extend_from_slice(&prompt_ids[orig - tail..]);
                prompt_ids = kept;
                eprintln!("wgpu-engine: context {orig} tokens > {max_prompt} window — kept {head} head + {tail} tail (raise n_ctx for more)");
            }
            // Reply budget = ceiling, capped to the room left after the prompt (continuous decode fits
            // the window instead of forcing prompt truncation).
            let n_new = ceiling.min(window.saturating_sub(prompt_ids.len()).max(1));
            let cap = (prompt_ids.len() + n_new).max(1);
            // ── Speculative-decode path (opt-in: a draft model was loaded) ───────────────────────────
            // Draft proposes a block; target verifies the whole block in one forward (engine
            // `speculative_generate_stats`). Greedy and bit-exact to normal decode, but more tokens per
            // expensive target pass. Bypasses KV-tree reuse (spec manages its own target+draft caches).
            if let Some(draft) = self.draft.as_ref() {
                let k: usize = std::env::var("LOGSCOPE_SPEC_K").ok().and_then(|s| s.trim().parse().ok()).filter(|&k| k >= 1).unwrap_or(4);
                let mut st = gpu_kernels::SpecStats::default();
                let td = Instant::now();
                let ids = self.gpu.speculative_generate_stats(&self.model, draft, &prompt_ids, n_new, k, &mut st);
                let secs = td.elapsed().as_secs_f32().max(1e-3);
                // Emit through the same UTF-8 byte-buffering + special-swallow + stop logic as streaming.
                let mut pending: Vec<u8> = Vec::new();
                let mut produced = 0usize;
                for &next in &ids {
                    if self.stop_ids.contains(&next) { break; }
                    if self.tok.is_special(next) { continue; }
                    produced += 1;
                    pending.extend_from_slice(&self.tok.decode_bytes(&[next]));
                    let valid = match std::str::from_utf8(&pending) { Ok(s) => s.len(), Err(e) => e.valid_up_to() };
                    if valid == 0 { continue; }
                    let chunk = String::from_utf8_lossy(&pending[..valid]).into_owned();
                    pending.drain(..valid);
                    if !on(&chunk) { break; }
                }
                if !pending.is_empty() { let _ = on(&String::from_utf8_lossy(&pending)); }
                eprintln!("wgpu-engine: [{}] SPECULATIVE k={k} — {} tokens in {:.1}s = {:.2} tok/s | {:.2} accepted/round × {} rounds (~{:.2} tok per target verify-pass)",
                    self.name, ids.len(), secs, ids.len() as f32 / secs,
                    st.accepted as f64 / st.rounds.max(1) as f64, st.rounds, st.emitted as f64 / st.rounds.max(1) as f64);
                return Ok(produced);
            }
            // Which cached branch tip shares the longest usable prefix with this prompt? `pick_anchor`
            // applies the same thresholds the single-anchor path used (≥16 shared, ≤256 divergent), but
            // searches the whole pool — so a sibling switch or an Nth fan-out continuation reuses ITS
            // lineage's KV, not just the last turn's. Build the lightweight (toks, prefill_len, pos) view;
            // cloning a few hundred token ids per entry is negligible next to a prefill.
            let meta: Vec<(Vec<u32>, usize, usize)> =
                self.anchors.iter().map(|(t, sess, pf)| (t.clone(), *pf, sess.pos)).collect();
            let kv_off = std::env::var("LOGSCOPE_KV_OFF").is_ok();
            let pick = if kv_off { None } else { crate::kvtree::pick_anchor(&meta, &prompt_ids, 16, 256) };
            let (reuse, l, pick_idx) = match pick { Some((i, ln)) => (true, ln, i), None => (false, 0, 0) };
            let suffix_len = prompt_ids.len() - l;
            let mut s = self.gpu.session_new(&self.model, cap, self.rng);
            let t0 = Instant::now();
            if reuse {
                let src = &self.anchors[pick_idx].1.cache; // borrow ends after the copy
                self.gpu.seed_from_prefix(&self.model, &mut s, src, l);
                // Prefill the divergent suffix with the BATCHED kernel (verify_forward / block_
                // prefill_ext at offset l) — the same path a full prefill uses — instead of per-
                // token decode_step. Per-token vs batched differ by ~1e-6, which is enough to tip a
                // near-tied eos in a long, degenerating context (the "produces once then stops"
                // failure). Batched suffix keeps reuse within machine precision of a full prefill.
                let all = self.gpu.verify_forward(&self.model, &s.cache, &prompt_ids[l..], l);
                let v = self.model.vocab;
                s.logits = all[all.len() - v..].to_vec();
                s.pos = prompt_ids.len();
            } else {
                // O(1)-bandwidth batched prefill of the whole prompt (one [T,d] pass).
                // O(chunk·T) chunked prefill — NOT one O(T²) batched pass — so a long pasted
                // context can't balloon attention scratch to tens of GB and OOM the device.
                s.logits = self.gpu.prefill_chunked(&self.model, &s.cache, &prompt_ids, PREFILL_CHUNK);
                s.pos = prompt_ids.len();
            }
            eprintln!("wgpu-engine: [{}] prompt {} tokens → up to {} (temp {:.2}); {} in {:.2}s, decoding…", self.name,
                prompt_ids.len(), n_new, p.temperature,
                if reuse { format!("reused {l} anchor tokens + prefilled {suffix_len}") } else { format!("full prefill") },
                t0.elapsed().as_secs_f32());
            let td = Instant::now();
            // Batch 3: snapshot the engine's dispatch counters so we can report the CPU-bound (setup)
            // vs GPU-bound (sync) split for THIS generation — the data that answers "why is llama.cpp
            // faster": high submits/token + setup_ms-dominated ⇒ dispatch-bound (→ fuse kernels), not
            // compute-bound.
            use std::sync::atomic::Ordering as AOrd;
            let (s0, p0, su0, sy0) = (
                gpu_kernels::GPU_SUBMITS.load(AOrd::Relaxed), gpu_kernels::GPU_POLLS.load(AOrd::Relaxed),
                gpu_kernels::GPU_SETUP_NS.load(AOrd::Relaxed), gpu_kernels::GPU_SYNC_NS.load(AOrd::Relaxed));
            // Windowed SAMPLED decode: the whole sample→embed→decode cycle stays on-device for k
            // tokens per readback — no per-token full-vocab logits sync and no per-token O(v·log v)
            // CPU sort. Eligibility + lazy int8-embed build live in `windowed_ready` so the resume
            // path shares them exactly. LOGSCOPE_SAMPLE_WINDOW (default 4; 0/1 disables).
            let sample_win_opt = self.windowed_ready(&sp);
            let embed_q8 = self.embed_q8.as_ref();
            let (gpu, model, tok, eos, stop) = (&self.gpu, &self.model, &self.tok, self.eos, &self.stop_ids);
            // Diagnostic: which stop token (if any) ended the stream, captured from the decode callback —
            // distinguishes "model emitted im_end/eot early" from "hit the cap / runaway guard / cancel".
            let stop_tok = std::cell::Cell::new(None::<u32>);
            // Stream up to `n_new` tokens from a prefilled session into `on` (UTF-8 buffered;
            // specials swallowed). Factored into a closure so the turn can be retried with a clean
            // full prefill if anchor reuse stops at zero tokens.
            let mut stream_from = |s: &mut Session, rng: &mut u64| -> usize {
                let mut seen = prompt_ids.clone();
                let mut pending: Vec<u8> = Vec::new();
                let mut recent = String::new(); // tail of generated text, for the runaway guard
                let step = |next: u32| {
                    if stop.contains(&next) { stop_tok.set(Some(next)); return false; }   // halt on ANY stop token, not just primary eos
                    if tok.is_special(next) { return true; }     // other specials: swallow, keep going
                    pending.extend_from_slice(&tok.decode_bytes(&[next]));
                    let valid = match std::str::from_utf8(&pending) { Ok(s) => s.len(), Err(e) => e.valid_up_to() };
                    if valid == 0 { return true; }
                    let chunk = String::from_utf8_lossy(&pending[..valid]).into_owned();
                    pending.drain(..valid);
                    let cont = on(&chunk);
                    // Backstop: if a degenerate model loops past its stop, cut it (4-gram repeat or a
                    // leaked pseudo-template marker). Check only the tail to stay O(1) per token.
                    recent.push_str(&chunk);
                    // Keep ~1200 bytes of tail, but cut on a CHAR BOUNDARY — `recent` may hold
                    // multi-byte UTF-8 (accented output), and String::drain panics on a mid-char index.
                    if recent.len() > 1200 {
                        let mut cut = recent.len() - 1200;
                        while cut < recent.len() && !recent.is_char_boundary(cut) { cut += 1; }
                        recent.drain(..cut);
                    }
                    if crate::genguard::runaway_stop(&recent, 0.5) != crate::genguard::StopReason::None { return false; }
                    cont
                };
                let p = match (sample_win_opt, embed_q8) {
                    (Some(sample_win), Some(eq)) => gpu.stream_session_window(model, s, &mut seen, n_new, eos, &sp, rng, sample_win,
                                                                  &gpu_kernels::EmbedT::Q8(eq), cancel, step),
                    _ => gpu.stream_session(model, s, &mut seen, n_new, eos, &sp, rng, cancel, step),
                };
                if !pending.is_empty() { let _ = on(&String::from_utf8_lossy(&pending)); }
                p
            };
            let mut rng = self.rng;
            let mut produced = stream_from(&mut s, &mut rng);
            // After reuse the suffix is batch-prefilled by verify_forward, so the WHOLE prompt is valid KV
            // (seeded [0,l) + batch-prefilled [l,len)) — advertise all of it as reusable, not just the copied
            // prefix `l`. Under-reporting to `l` capped continuation reuse: a child resuming from this tip got
            // clamped to the seed and blew past the 256 suffix window into a full prefill (the 3/24 ceiling).
            let mut prefill_len = prompt_ids.len();
            // Self-heal: anchor reuse that stops at 0 tokens (with no cancel) may have hit residual
            // reuse drift tipping a near-tied eos — retry once with a clean full prefill. If that
            // also yields nothing, the model genuinely stopped (nothing more to be done here).
            if reuse && produced == 0 && !cancel() {
                eprintln!("wgpu-engine: reuse produced 0 tokens — retrying with a clean full prefill…");
                let mut s2 = self.gpu.session_new(&self.model, cap, self.rng);
                s2.logits = self.gpu.prefill_chunked(&self.model, &s2.cache, &prompt_ids, PREFILL_CHUNK);
                s2.pos = prompt_ids.len();
                produced = stream_from(&mut s2, &mut rng);
                s = s2;
                prefill_len = prompt_ids.len();
            }
            self.rng = rng;
            let secs = td.elapsed().as_secs_f32().max(1e-3);
            eprintln!("wgpu-engine: [{}] produced {} tokens in {:.1}s = {:.2} tok/s | {} pipelines cached (warm {})", self.name,
                produced, secs, produced as f32 / secs, self.gpu.pipeline_count(), self.warm);
            // Dispatch profile for this turn: submits & polls per token, and where the wall time went
            // (CPU setup vs GPU sync). A high submits/tok with setup ≳ sync is the dispatch-bound floor.
            let (ds, dp) = (gpu_kernels::GPU_SUBMITS.load(AOrd::Relaxed) - s0, gpu_kernels::GPU_POLLS.load(AOrd::Relaxed) - p0);
            let (setup_ms, sync_ms) = ((gpu_kernels::GPU_SETUP_NS.load(AOrd::Relaxed) - su0) as f64 / 1e6, (gpu_kernels::GPU_SYNC_NS.load(AOrd::Relaxed) - sy0) as f64 / 1e6);
            let per = produced.max(1) as f64;
            eprintln!("wgpu-engine: [{}] profile — {ds} submits ({:.0}/tok), {dp} polls | setup {setup_ms:.0}ms vs sync {sync_ms:.0}ms ({}-bound)",
                self.name, ds as f64 / per, if setup_ms >= sync_ms { "dispatch" } else { "GPU" });
            // Adapt passes-per-submit: feed this turn's per-token decode time to the tuner, which sweeps
            // SUBMIT_PROFILES across replies and settles on the fastest, then caches it. Bit-exact across
            // flush (--gate-submit), so adapting between replies never changes output. Needs enough tokens
            // for a stable per-token time; very short replies are skipped.
            if let Some(t) = self.submit_tuner.as_mut() {
                if produced >= 8 {
                    let spt = secs as f64 / per; // mean seconds per decoded token this turn
                    let subs_per_tok = (ds as f64 / per).max(1.0).round() as u64; // this turn's submits/token
                    let used = self.gpu.flush_every();
                    if let Some(chosen) = t.record(spt, subs_per_tok) {
                        gpu_kernels::store::flush_put(&self.gpu.adapter_name, chosen);
                        let b = t.budget_ms();
                        let btxt = if b.is_finite() { format!("{b:.0} ms/submit budget") } else { "no submit budget".into() };
                        eprintln!("wgpu-engine: submit-tuner SETTLED flush_every={chosen} passes/submit ({btxt}; cached for this GPU); sweep:");
                        for (f, tok, sub) in t.results() { eprintln!("wgpu-engine:   flush {f:>4} → {:.1} ms/tok · {:.1} ms/submit", tok * 1000.0, sub); }
                    } else {
                        eprintln!("wgpu-engine: submit-tuner sampled flush_every={used} → {:.1} ms/tok · {:.1} ms/submit ({} candidate(s) done)",
                            spt * 1000.0, spt * 1000.0 / subs_per_tok as f64, t.results().len());
                    }
                    self.gpu.set_flush_every(t.current()); // value for the next reply
                }
            }
            // Diagnostic for the early-stop investigation: a short reply is the symptom (Qwen produced
            // 9 then 2 then 2 tokens). Dump WHY it stopped and the last token IDs so the cause is
            // visible — model emitting a stop token (template/context) vs the guard vs the cap. The
            // argmax of the final logits is what the model wanted next (its stop token, if it stopped).
            if produced < 24 {
                let tail: Vec<u32> = s.generated.iter().rev().take(16).rev().copied().collect();
                let why = match stop_tok.get() {
                    Some(t) if Some(t) == self.eos => format!("model emitted EOS {t}"),
                    Some(t) => format!("model emitted stop token {t}"),
                    None => "token cap / runaway guard / cancel (no stop token seen)".to_string(),
                };
                let next_argmax = s.logits.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal)).map(|(i, _)| i);
                let decoded = String::from_utf8_lossy(&self.tok.decode_bytes(&s.generated)).replace('\n', "\\n");
                eprintln!("wgpu-engine: [{}] SHORT REPLY ({produced} tok) — stop: {why}; stop_set={:?}; next-token argmax={:?}",
                    self.name, self.stop_ids, next_argmax);
                eprintln!("wgpu-engine: [{}]   generated text = {:?}", self.name, decoded);
                eprintln!("wgpu-engine: [{}]   generated ids (≤16) = {:?}", self.name, tail);
            }
            // This turn (prompt + what we generated) joins the pool as a branch tip; `prefill_len` is how
            // many leading rows were batch-prefilled — the only region ever re-seeded. Evict oldest first
            // so the pool stays within `kv_cap` (each entry holds a full KV cache).
            let mut anchor_toks = prompt_ids.clone();
            anchor_toks.extend_from_slice(&s.generated);
            self.anchors.push((anchor_toks, s, prefill_len));
            while self.anchors.len() > self.kv_cap.max(1) { self.anchors.remove(0); }
            Ok(produced)
        }

        /// Resume the LAST generation from warm KV state — no re-prefill. The anchor session
        /// already holds the full context in its cache; we copy it into a session with headroom
        /// (a GPU buffer copy, not a forward pass) and keep decoding from the live next-token
        /// logits. This turns auto-continue from "re-prefill thousands of tokens" into an instant
        /// resume. `None` when there's no resumable anchor (caller falls back to a prompt generate).
        /// Decode several prompts in ONE batched pass — the fan-out payoff. Finds their longest common
        /// token prefix, prefills it once, then admits each prompt's divergent suffix into `run_batched`,
        /// which steps all sessions together. A batch-1 decode barely uses the GPU, so each extra branch is
        /// close to free. Equivalent to N separate greedy generates (the engine proves batched==per-request
        /// to 0.0); at temperature>0 the per-session RNG differs so tokens differ but the distribution is the
        /// same. No streaming, and a single stop token (eos) rather than the full stop set — fine for a fan.
        pub fn generate_batched(&mut self, prompts: &[String], p: &GenParams) -> Vec<String> {
            let n = prompts.len();
            if n == 0 { return Vec::new(); }
            if n == 1 {
                let mut o = String::new();
                let _ = self.generate(&prompts[0], p, &|| false, &mut |s| { o.push_str(s); true });
                return vec![o];
            }
            let enc: Vec<Vec<u32>> = prompts.iter().map(|pr| { let t = self.tok.encode(pr); if t.is_empty() { vec![0] } else { t } }).collect();
            // longest common token prefix across every branch — the shared ancestor, prefilled once
            let mut shared = enc[0].clone();
            for e in &enc[1..] { let c = shared.iter().zip(e).take_while(|(a, b)| a == b).count(); shared.truncate(c); }
            let window = (p.n_ctx as usize).clamp(1024, MAX_CONTEXT);
            let reply = p.reply_ceiling.max(p.max_tokens).max(1);
            let max_seq = window;
            let mut pc = self.gpu.session_new(&self.model, max_seq, self.rng);
            if !shared.is_empty() { let _ = self.gpu.prefill_chunked(&self.model, &pc.cache, &shared, PREFILL_CHUNK); pc.pos = shared.len(); }
            let sp = SamplerParams { temperature: p.temperature, top_k: 40, top_p: 0.95, repeat_penalty: p.repeat_penalty, repeat_last_n: 64, min_p: p.min_p };
            let queue: Vec<(usize, Vec<u32>, usize)> = enc.iter().enumerate()
                .map(|(i, e)| (i, e[shared.len().min(e.len())..].to_vec(), reply.min(window.saturating_sub(e.len())).max(1)))
                .collect();
            let t0 = Instant::now();
            eprintln!("wgpu-engine: [{}] BATCHED — {} prompts, shared prefix {} tok, decoding together", self.name, n, shared.len());
            let prefix = if shared.is_empty() { None } else { Some((&pc.cache, shared.len())) };
            let out = self.gpu.run_batched(&self.model, prefix, queue, &sp, self.eos, n, max_seq);
            eprintln!("wgpu-engine: [{}] batched {} replies in {:.1}s ({:.2}s/branch wall)", self.name, out.len(), t0.elapsed().as_secs_f32(), t0.elapsed().as_secs_f32() / n as f32);
            let mut replies = vec![String::new(); n];
            for (id, toks) in out { if id < n { replies[id] = String::from_utf8_lossy(&self.tok.decode_bytes(&toks)).trim().to_string(); } }
            replies
        }

        pub fn continue_last(&mut self, p: &GenParams, cancel: &dyn Fn() -> bool, on: &mut dyn FnMut(&str) -> bool) -> Option<Result<usize, String>> {
            let (toks, old, prefill_len) = self.anchors.pop()?;
            if old.logits.is_empty() || old.pos == 0 { self.anchors.push((toks, old, prefill_len)); return Some(Ok(0)); }
            let n_new = (p.max_tokens).max(1);
            let sp = SamplerParams { temperature: p.temperature, top_k: 40, top_p: 0.95, repeat_penalty: p.repeat_penalty, repeat_last_n: 64, min_p: p.min_p };
            // Copy the live KV into a session with room for the continuation (buffer copy, no forward).
            let cap = (old.pos + n_new).max(1);
            let mut s = self.gpu.session_new(&self.model, cap, self.rng);
            self.gpu.seed_from_prefix(&self.model, &mut s, &old.cache, old.pos);
            s.pos = old.pos;
            s.logits = old.logits.clone(); // resume from the live next-token distribution
            drop(old);
            let td = Instant::now();
            // Same windowed-decode eligibility as the fresh path (this is the fix for auto-continue
            // reverting to token-by-token: it used to call stream_session unconditionally).
            let sample_win_opt = self.windowed_ready(&sp);
            let embed_q8 = self.embed_q8.as_ref();
            let (gpu, model, tok, eos, stop) = (&self.gpu, &self.model, &self.tok, self.eos, &self.stop_ids);
            let mut seen = toks;
            let mut pending: Vec<u8> = Vec::new();
            let mut recent = String::new();
            let mut rng = self.rng;
            let step = |next: u32| {
                if stop.contains(&next) { return false; }
                if tok.is_special(next) { return true; }
                pending.extend_from_slice(&tok.decode_bytes(&[next]));
                let valid = match std::str::from_utf8(&pending) { Ok(s) => s.len(), Err(e) => e.valid_up_to() };
                if valid == 0 { return true; }
                let chunk = String::from_utf8_lossy(&pending[..valid]).into_owned();
                pending.drain(..valid);
                let cont = on(&chunk);
                recent.push_str(&chunk);
                if recent.len() > 1200 {
                    let mut cut = recent.len() - 1200;
                    while cut < recent.len() && !recent.is_char_boundary(cut) { cut += 1; }
                    recent.drain(..cut);
                }
                if crate::genguard::runaway_stop(&recent, 0.5) != crate::genguard::StopReason::None { return false; }
                cont
            };
            let produced = match (sample_win_opt, embed_q8) {
                (Some(sample_win), Some(eq)) => gpu.stream_session_window(model, &mut s, &mut seen, n_new, eos, &sp, &mut rng, sample_win,
                                                              &gpu_kernels::EmbedT::Q8(eq), cancel, step),
                _ => gpu.stream_session(model, &mut s, &mut seen, n_new, eos, &sp, &mut rng, cancel, step),
            };
            if !pending.is_empty() { let _ = on(&String::from_utf8_lossy(&pending)); }
            self.rng = rng;
            let secs = td.elapsed().as_secs_f32().max(1e-3);
            eprintln!("wgpu-engine: [{}] continued {produced} tokens in {secs:.1}s = {:.2} tok/s (warm resume, no prefill{})",
                self.name, produced as f32 / secs, if sample_win_opt.is_some() { ", windowed" } else { "" });
            self.anchors.push((seen, s, prefill_len));
            while self.anchors.len() > self.kv_cap.max(1) { self.anchors.remove(0); }
            Some(Ok(produced))
        }
    }
}

/// Bridges the in-process wgpu engine to the fabric `Worker` trait, so the local box is just another
/// node the Registry can dispatch to — same interface as a Remote box over the wire. It owns its
/// Engine (one resident model) and answers token-in/token-out requests for that model. The highest-
/// leverage wiring: once the engine is a Worker, fan-out / best-of-K / branch dispatch all work
/// against real generation, not a mock.
#[cfg(feature = "wgpu-engine")]
pub struct LocalWorker {
    id: crate::fabric::WorkerId,
    models: Vec<crate::fabric::ModelInfo>,
    model_name: String,
    engine: engine::Engine,
}
#[cfg(feature = "wgpu-engine")]
impl LocalWorker {
    /// Load `path` on the local device and expose it under `model_name` as a fabric worker.
    pub fn load(device: &str, author: &str, model_name: &str, public: bool, params_b: f32, path: &str) -> Result<Self, String> {
        let engine = engine::Engine::load(path)?;
        Ok(Self {
            id: crate::fabric::WorkerId::new(device, author),
            models: vec![crate::fabric::ModelInfo { name: model_name.to_string(), public, params_b }],
            model_name: model_name.to_string(),
            engine,
        })
    }
}
#[cfg(feature = "wgpu-engine")]
impl crate::fabric::Worker for LocalWorker {
    fn id(&self) -> &crate::fabric::WorkerId { &self.id }
    fn models(&self) -> &[crate::fabric::ModelInfo] { &self.models }
    fn generate(&self, req: &crate::fabric::GenRequest) -> Option<crate::fabric::GenResult> {
        if req.model != self.model_name { return None; } // this box doesn't serve that model
        let tokens = self.engine.run_tokens(&req.prefix, req.max_new, req.temperature, req.seed);
        Some(crate::fabric::GenResult { tokens, worker: self.id.clone(), model: self.model_name.clone() })
    }
}

struct Wgpu {
    #[cfg(feature = "wgpu-engine")]
    engine: Option<engine::Engine>,
}
impl Backend for Wgpu {
    fn name(&self) -> &'static str { "wgpu" }
    fn loaded(&self) -> bool {
        #[cfg(feature = "wgpu-engine")] { self.engine.is_some() }
        #[cfg(not(feature = "wgpu-engine"))] { false }
    }
    fn load(&mut self, path: &str, _p: &GenParams) -> Result<(), String> {
        #[cfg(feature = "wgpu-engine")] { self.engine = Some(engine::Engine::load(path)?); Ok(()) }
        #[cfg(not(feature = "wgpu-engine"))]
        { let _ = path; Err("wgpu engine: rebuild with --features wgpu-engine for clean-room GGUF inference (kernels validated in gpu-gemm)".into()) }
    }
    fn generate(&mut self, prompt: &str, p: &GenParams, cancel: &dyn Fn() -> bool, on: &mut dyn FnMut(&str) -> bool) -> Result<usize, String> {
        #[cfg(feature = "wgpu-engine")]
        { match &mut self.engine { Some(e) => e.generate(prompt, p, cancel, on), None => Err("no model loaded".into()) } }
        #[cfg(not(feature = "wgpu-engine"))]
        { let _ = (prompt, p, cancel, on); Err("wgpu engine: not built (enable --features wgpu-engine)".into()) }
    }
    fn continue_last(&mut self, p: &GenParams, cancel: &dyn Fn() -> bool, on: &mut dyn FnMut(&str) -> bool) -> Option<Result<usize, String>> {
        #[cfg(feature = "wgpu-engine")]
        { match &mut self.engine { Some(e) => e.continue_last(p, cancel, on), None => None } }
        #[cfg(not(feature = "wgpu-engine"))]
        { let _ = (p, cancel, on); None }
    }
    fn generate_batched(&mut self, prompts: &[String], p: &GenParams, cancel: &dyn Fn() -> bool) -> Vec<String> {
        #[cfg(feature = "wgpu-engine")]
        { if let Some(e) = &mut self.engine { let _ = cancel; return e.generate_batched(prompts, p); } }
        // no engine (or not built): fall back to sequential generate, same as the trait default
        prompts.iter().map(|pr| { let mut o = String::new(); let _ = self.generate(pr, p, cancel, &mut |s| { o.push_str(s); true }); o.trim().to_string() }).collect()
    }
    fn template(&self) -> TemplateKind {
        #[cfg(feature = "wgpu-engine")]
        { self.engine.as_ref().map(|e| e.template()).unwrap_or(TemplateKind::Llama3) }
        #[cfg(not(feature = "wgpu-engine"))]
        { TemplateKind::Llama3 }
    }
}

pub fn make(kind: BackendKind) -> Box<dyn Backend> {
    match kind {
        BackendKind::LlamaCpp => Box::new(LlamaCpp { model: None }),
        BackendKind::Wgpu => Box::new(Wgpu { #[cfg(feature = "wgpu-engine")] engine: None }),
    }
}

// ── worker protocol ──────────────────────────────────────────────────────────
/// Chat-template family the loaded model expects. The wgpu path builds the prompt app-side, so it
/// must match the model: a ChatML model (smollm2, `<|im_end|>`) fed Llama-3 markers drifts into
/// byte-level marker text and never hits its real stop. Detected from the tokenizer at load.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TemplateKind { Llama3, ChatML, Plain, Gemma, Llama4 }
impl TemplateKind {
    fn to_u8(self) -> u8 { match self { Self::Llama3 => 0, Self::ChatML => 1, Self::Plain => 2, Self::Gemma => 3, Self::Llama4 => 4 } }
    fn from_u8(v: u8) -> Option<Self> { Some(match v { 0 => Self::Llama3, 1 => Self::ChatML, 2 => Self::Plain, 3 => Self::Gemma, 4 => Self::Llama4, _ => return None }) }
}

/// What a successful load reports back to the UI.
pub struct LoadInfo { pub path: String, pub template: TemplateKind }

pub enum Cmd { SetBackend(BackendKind), SetParams(GenParams), Load(String), Generate(String), Continue(String) }
pub enum Evt { Loaded(Result<LoadInfo, String>), Token(String), Done { tokens: usize, secs: f32 }, Error(String) }

pub struct Inference { tx: Sender<Cmd>, pub rx: Receiver<Evt>, cancel: Arc<AtomicBool>,
    /// Detected chat template of the CURRENTLY loaded model, published by the worker on every successful
    /// load (255 = nothing loaded). UI tabs read this LIVE at prompt-build time instead of relying only on
    /// per-tab cached state, so a tab created after the load (or one whose Loaded event another drain
    /// consumed) can never silently build Llama-3 prompts for a ChatML model.
    template: Arc<std::sync::atomic::AtomicU8> }
impl Inference {
    /// The worker-published template of the loaded model, if any.
    pub fn detected_template(&self) -> Option<TemplateKind> {
        TemplateKind::from_u8(self.template.load(Ordering::Relaxed))
    }
}
impl Inference {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = channel::<Cmd>();
        let (evt_tx, evt_rx) = channel::<Evt>();
        let cancel = Arc::new(AtomicBool::new(false));
        let template = Arc::new(std::sync::atomic::AtomicU8::new(255));
        let cw = cancel.clone();
        let tw = template.clone();
        std::thread::spawn(move || worker(cmd_rx, evt_tx, cw, tw));
        Self { tx: cmd_tx, rx: evt_rx, cancel, template }
    }
    pub fn load(&self, path: String) { let _ = self.tx.send(Cmd::Load(path)); }
    pub fn generate(&self, prompt: String) { self.cancel.store(false, Ordering::Relaxed); let _ = self.tx.send(Cmd::Generate(prompt)); }
    /// Resume the last reply in place (warm KV); `fallback` is used only if the backend can't resume.
    pub fn continue_stream(&self, fallback: String) { self.cancel.store(false, Ordering::Relaxed); let _ = self.tx.send(Cmd::Continue(fallback)); }
    pub fn set_params(&self, p: GenParams) { let _ = self.tx.send(Cmd::SetParams(p)); }
    pub fn set_backend(&self, k: BackendKind) { let _ = self.tx.send(Cmd::SetBackend(k)); }
    /// Request cancellation of the in-flight generation. The worker is blocked
    /// inside `generate()` and can't read the command channel, so this is a shared
    /// flag the per-token callback polls — it stops at the next token boundary.
    pub fn cancel(&self) { self.cancel.store(true, Ordering::Relaxed); }
}

/// Translate a prompt built with the WRONG family's markers to the loaded model's template. The UI caches
/// template state per tab and has been observed going stale (a ChatML model receiving Llama-3 headers —
/// byte-split into garbage since they're not in its vocab → "0 special tokens" → word salad). The worker
/// knows the loaded template authoritatively, so it repairs here as the last line of defense and logs it.
fn normalize_prompt_template(prompt: &str, t: TemplateKind) -> Option<String> {
    match t {
        TemplateKind::ChatML if prompt.contains("<|start_header_id|>") => {
            let s = prompt.replace("<|begin_of_text|>", "")
                .replace("<|start_header_id|>", "<|im_start|>")
                .replace("<|end_header_id|>\n\n", "\n")
                .replace("<|eot_id|>", "<|im_end|>\n");
            Some(s)
        }
        TemplateKind::Llama4 if prompt.contains("<|start_header_id|>") || prompt.contains("<|im_start|>") => {
            // Same header discipline as Llama-3, different marker spellings (+ <|eot|> for <|eot_id|>).
            let mut b = prompt.replace("<|begin_of_text|>", "")
                .replace("<|start_header_id|>", "<|header_start|>")
                .replace("<|end_header_id|>", "<|header_end|>")
                .replace("<|eot_id|>", "<|eot|>")
                .replace("<|im_start|>", "<|header_start|>");
            for r in ["system", "user", "assistant"] {
                b = b.replace(&format!("<|header_start|>{r}\n"), &format!("<|header_start|>{r}<|header_end|>\n\n"));
            }
            b = b.replace("<|im_end|>\n", "<|eot|>");
            Some(format!("<|begin_of_text|>{b}"))
        }
        TemplateKind::Llama3 if prompt.contains("<|im_start|>") => {
            let mut b = prompt.replace("<|im_start|>", "<|start_header_id|>");
            for r in ["system", "user", "assistant"] {
                b = b.replace(&format!("<|start_header_id|>{r}\n"), &format!("<|start_header_id|>{r}<|end_header_id|>\n\n"));
            }
            b = b.replace("<|im_end|>\n", "<|eot_id|>");
            Some(format!("<|begin_of_text|>{b}"))
        }
        _ => None,
    }
}

fn worker(cmd_rx: Receiver<Cmd>, evt: Sender<Evt>, cancel: Arc<AtomicBool>, template_out: Arc<std::sync::atomic::AtomicU8>) {
    // Default to the backend that's actually compiled in (llama.cpp if available),
    // so the header Load and the chat tab operate on the same, working backend.
    let mut kind = BackendKind::preferred();
    let mut backend = make(kind);
    eprintln!("worker: default backend = {} ({})", kind.label(), backend.name());
    let mut params = GenParams::default();
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            // Only rebuild (and thus drop any loaded model) on a genuine switch. The
            // chat tab re-asserts its backend choice on first open; without this guard
            // that would silently wipe a model the user just loaded.
            Cmd::SetBackend(k) => if k != kind {
                kind = k; backend = make(k);
                eprintln!("worker: backend → {} ({})", k.label(), backend.name());
                let _ = evt.send(Evt::Loaded(Err(format!("backend → {} ({}) — reload the model", k.label(), backend.name()))));
            },
            Cmd::SetParams(p) => { params = p; }
            Cmd::Load(path) => {
                eprintln!("worker: loading '{path}' via {} backend", backend.name());
                match backend.load(&path, &params) {
                    Ok(()) => { let template = backend.template(); template_out.store(template.to_u8(), Ordering::Relaxed); let _ = evt.send(Evt::Loaded(Ok(LoadInfo { path, template }))); }
                    Err(e) => { eprintln!("worker: load failed: {e}"); let _ = evt.send(Evt::Loaded(Err(e))); }
                }
            }
            Cmd::Generate(prompt) => {
                // Last-line-of-defense template repair (see normalize_prompt_template): the worker knows
                // the loaded model's template; if the UI built the prompt with the other family's markers,
                // translate and say so loudly — this is the bug that garbled every ChatML model.
                let prompt = match normalize_prompt_template(&prompt, backend.template()) {
                    Some(fixed) => { eprintln!("worker: PROMPT TEMPLATE MISMATCH — UI sent {} markers to a {:?} model; translated (UI template state is stale — report this)", if fixed.contains("<|im_start|>") { "Llama-3" } else { "ChatML" }, backend.template()); fixed }
                    None => prompt,
                };
                // NOTE: cancel is cleared by the SENDER (Inference::generate) on the UI thread, ordered
                // before any Stop the user then clicks — resetting it here (on dequeue) could clobber a
                // Stop clicked in the window between send and dequeue, which is exactly the "subsequent
                // turns don't abort" bug (the worker is busy finishing the prior turn when Stop is hit).
                let start = Instant::now();
                if backend.loaded() {
                    let evt2 = evt.clone();
                    let cancel2 = cancel.clone();
                    // emit each token; the bool also early-stops the decode loop
                    let mut cb = move |p: &str| -> bool {
                        let _ = evt2.send(Evt::Token(p.to_string()));
                        !cancel2.load(Ordering::Relaxed)
                    };
                    // checked between prefill tokens too, so Stop interrupts long prompts
                    let cancel3 = cancel.clone();
                    let is_cancelled = move || cancel3.load(Ordering::Relaxed);
                    match backend.generate(&prompt, &params, &is_cancelled, &mut cb) {
                        Ok(n) => { let _ = evt.send(Evt::Done { tokens: n, secs: start.elapsed().as_secs_f32() }); }
                        Err(e) => { let _ = evt.send(Evt::Error(e)); }
                    }
                } else {
                    simulate(&prompt, &evt, start, &cancel);
                }
            }
            Cmd::Continue(prompt) => {
                // cancel cleared by the sender (Inference::continue_stream) — see Cmd::Generate.
                let prompt = normalize_prompt_template(&prompt, backend.template()).unwrap_or(prompt);
                let start = Instant::now();
                if backend.loaded() {
                    let evt2 = evt.clone();
                    let cancel2 = cancel.clone();
                    let mut cb = move |p: &str| -> bool { let _ = evt2.send(Evt::Token(p.to_string())); !cancel2.load(Ordering::Relaxed) };
                    let cancel3 = cancel.clone();
                    let is_cancelled = move || cancel3.load(Ordering::Relaxed);
                    // Warm-resume from the live KV if the backend can; else fall back to re-prefilling
                    // the continuation prompt.
                    let res = match backend.continue_last(&params, &is_cancelled, &mut cb) {
                        Some(r) => r,
                        None => backend.generate(&prompt, &params, &is_cancelled, &mut cb),
                    };
                    match res {
                        Ok(n) => { let _ = evt.send(Evt::Done { tokens: n, secs: start.elapsed().as_secs_f32() }); }
                        Err(e) => { let _ = evt.send(Evt::Error(e)); }
                    }
                } else {
                    simulate(&prompt, &evt, start, &cancel);
                }
            }
        }
    }
}

fn simulate(prompt: &str, evt: &Sender<Evt>, start: Instant, cancel: &Arc<AtomicBool>) {
    // No model is loaded. Emit an unmistakable notice — never fake-looking content. (This
    // previously streamed a canned demo sentence at 24ms/word ≈ 40 "tok/s", which read as a
    // real but nonsensical answer and was deeply confusing.) Real wgpu decode is a few tok/s.
    let _ = prompt;
    let notice = "⚠ No model loaded — this is a placeholder, not real inference. \
        Load a GGUF first (Load model → choose a .gguf), then resend your message. \
        Note: real decode runs at a few tok/s; a fast ~40 tok/s reply means you are seeing this placeholder, not the model.";
    let mut n = 0usize;
    for w in notice.split_inclusive(' ') {
        if cancel.load(Ordering::Relaxed) { break; }
        if evt.send(Evt::Token(w.to_string())).is_err() { return; }
        n += 1;
        std::thread::sleep(Duration::from_millis(8));
    }
    let _ = evt.send(Evt::Done { tokens: n, secs: start.elapsed().as_secs_f32() });
}
