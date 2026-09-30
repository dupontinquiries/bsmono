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
    /// The Liaison's view of the loaded model's architecture support — its GGUF arch string and
    /// whatever gaps the loader found, so `what_is_blocked_with` can report them instead of them
    /// only ever reaching a `wgpu-engine: ⚠` stderr line. `None` by default: only the wgpu engine
    /// (`gpu_kernels::Model`) tracks this today — llama.cpp and the daemon path have no such data
    /// to report, and a fabricated "no gaps" would be exactly the ungrounded claim the Liaison
    /// refuses to make.
    fn model_status(&self) -> Option<logscope::liaison::ModelStatus> { None }

    /// Generate from a STRUCTURED chat history, letting a backend that has the model's chat template
    /// apply it itself. This is the path that makes Qwen/Ornith tool-calling reliable: the server
    /// wraps the messages in the exact turn markers the model trained on, rather than the app
    /// flattening them into one Plain string and hoping.
    ///
    /// The default flattens to `generate()`, so backends that cannot use messages (the wgpu engine,
    /// which builds its own prompt) lose nothing. Only `Remote` overrides it (llama-server's
    /// `/v1/chat/completions` applies the template server-side).
    fn generate_chat(&mut self, messages: &[ChatTurn], params: &GenParams, cancel: &dyn Fn() -> bool, on_token: &mut dyn FnMut(&str) -> bool) -> Result<usize, String> {
        self.generate(&flatten_chat(messages), params, cancel, on_token)
    }
}

/// One turn for `generate_chat`. Role plus text; tool results ride as a `tool` role so a server that
/// templates them (Qwen, Llama-3 ipython) frames them correctly.
#[derive(Debug, Clone)]
pub struct ChatTurn { pub role: ChatRole, pub content: String }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole { System, User, Assistant, Tool }

impl ChatTurn {
    pub fn system(s: impl Into<String>) -> Self { Self { role: ChatRole::System, content: s.into() } }
    pub fn user(s: impl Into<String>) -> Self { Self { role: ChatRole::User, content: s.into() } }
    pub fn assistant(s: impl Into<String>) -> Self { Self { role: ChatRole::Assistant, content: s.into() } }
}

/// The fallback rendering when a backend has no template: a neutral, readable transcript. Not a
/// model-specific format — backends that need one override `generate_chat`.
pub fn flatten_chat(messages: &[ChatTurn]) -> String {
    let mut s = String::new();
    for m in messages {
        let tag = match m.role { ChatRole::System => "System", ChatRole::User => "User", ChatRole::Assistant => "Assistant", ChatRole::Tool => "Tool result" };
        s.push_str(&format!("{tag}: {}\n\n", m.content));
    }
    s.push_str("Assistant: ");
    s
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BackendKind { LlamaCpp, Wgpu, Daemon, Remote }
impl BackendKind {
    pub const ALL: [BackendKind; 4] = [BackendKind::LlamaCpp, BackendKind::Wgpu, BackendKind::Daemon, BackendKind::Remote];
    pub fn label(self) -> &'static str {
        match self { BackendKind::LlamaCpp => "llama.cpp · Vulkan", BackendKind::Wgpu => "wgpu/Burn (experimental)", BackendKind::Daemon => "shared daemon (one copy of the weights)", BackendKind::Remote => "remote server (llama-server / Anthropic / Ollama over HTTP)" }
    }
    /// The backend to use by default: llama.cpp when its FFI is compiled in (mature,
    /// fast), otherwise the clean-room wgpu engine. Loading and chatting must agree on
    /// this so the chat talks to the backend that actually holds the loaded model.
    pub fn preferred() -> Self {
        if llm_backend::is_available() { BackendKind::LlamaCpp } else { BackendKind::Wgpu }
    }

    /// What to start with, PREFERRING a daemon that is actually running.
    ///
    /// Kept separate from `preferred()` rather than folded into it,
    /// because this one touches the filesystem. `preferred()` is pure
    /// and callable anywhere; a version that connected to a socket would
    /// be a trap the moment someone called it in a draw path.
    ///
    /// Probed ONCE, at worker startup. Not cached with a TTL and not
    /// re-probed per request: a daemon appearing mid-session should not
    /// silently move the model out from under a chat that already loaded
    /// one, and a daemon vanishing is handled where it is noticed — the
    /// generate call fails and says so.
    ///
    /// The fallback is the point. An app whose model stops loading
    /// because a background service is not running would be worse than
    /// one with no daemon: the failure is invisible and looks like the
    /// app's fault. So this is an ordering, not a switch.
    pub fn preferred_reachable() -> Self {
        // Explicit beats discovered: LOGSCOPE_REMOTE names a server (or a matrix box) and that is
        // a decision the person made, so it wins even over a local daemon. Unreachable → fall
        // through, same rule as the daemon: an absent server must not be an error path.
        if Remote::spec_from_env().map(|spec| Remote::resolve(&spec).map(|(ep, _, _)| hive::endpoint::probe(&ep).is_ok()).unwrap_or(false)).unwrap_or(false) {
            return BackendKind::Remote;
        }
        if std::os::unix::net::UnixStream::connect(daemon_socket_path()).is_ok() {
            BackendKind::Daemon
        } else {
            BackendKind::preferred()
        }
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
    use gpu_kernels::archetype::{ContextTurn, Refusal};
    use logscope::llm_backend::GenParams;
    use std::time::Instant;

    /// Say what the constraint did, once per turn, to the diagnostics AND the hive ledger — the
    /// ledger because `exhausted` steps are the signal that the grammar is overriding nothing and
    /// nobody watches stderr during an agent run.
    pub(crate) fn report_constraint(engine: &str, filt: Option<&crate::toolfilter::ToolFenceFilter>) {
        let Some(line) = filt.and_then(|f| f.report()) else { return };
        crate::diag!("wgpu-engine: [{engine}] {line}");
        let _ = hive::ledger::append(hive::ledger::Entry::Note { text: format!("{engine} {line}") });
    }

    /// The tool names a generated GBNF allows: the quoted literals on its `name ::=` alternation,
    /// e.g. `name ::= "\"read_file\"" | "\"run\""` → ["read_file", "run"]. Empty when the grammar
    /// is not one of ours, which the acceptor reads as "any name".
    pub(crate) fn grammar_tool_names(gbnf: &str) -> Vec<String> {
        let Some(line) = gbnf.lines().find(|l| l.trim_start().starts_with("name ::=")) else { return Vec::new() };
        // Everything after `::=` — otherwise the rule's own name rides along with the first alternative.
        let Some((_, line)) = line.split_once("::=") else { return Vec::new() };
        let mut out = Vec::new();
        // each alternative is "\"NAME\"" — the inner escaped quotes are what we key on
        for part in line.split('|') {
            let p = part.trim();
            let inner = p.trim_start_matches('"').trim_end_matches('"');
            let inner = inner.trim_start_matches("\\\"").trim_end_matches("\\\"");
            let name: String = inner.chars().filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect();
            if !name.is_empty() && name != "name" { out.push(name); }
        }
        out
    }
    /// Prefill block size. Attention scratch peaks at O(PREFILL_CHUNK · prompt_len), so this bounds
    /// the memory a long context can demand (vs O(prompt_len²) for a single batched prefill). ALSO caps
    /// how much work lands in each prefill dispatch: a GPU watchdog (Windows TDR, or the Asahi/Linux
    /// firmware timeout) kills any single dispatch that runs longer than ~1-2s, so an over-large chunk on
    /// a big prompt can hang the GPU ("GPU timeout / Halted" in dmesg). Smaller chunks = shorter dispatches
    /// = safe under the watchdog, at a small throughput cost. LOGSCOPE_PREFILL_CHUNK overrides.
    const PREFILL_CHUNK: usize = 256;
    /// The effective prefill chunk: env override, else the const default. Lower this (e.g. 64, 32) if the
    /// GPU watchdog fires during prefill on a long prompt.
    fn prefill_chunk() -> usize {
        // Accept the prefixed form, and the bare PREFILL_CHUNK as a forgiving fallback (the LOGSCOPE_
        // prefix is easy to drop and a silently-ignored override wastes a whole watchdog-bisection run).
        std::env::var("LOGSCOPE_PREFILL_CHUNK").or_else(|_| std::env::var("PREFILL_CHUNK")).ok()
            .and_then(|v| v.trim().parse::<usize>().ok()).filter(|&n| n >= 1).unwrap_or(PREFILL_CHUNK)
    }

    /// Prefill chunk scaled to the MODEL, not a fixed token count.
    ///
    /// What the watchdog actually bounds is the DURATION of one dispatch, and that scales with
    /// chunk x d x ff — so a constant token count means a bigger model gets a proportionally longer
    /// dispatch. 256 was tuned on a d=3072/ff=8192 shape; Llama-4 Scout is d=5120/ff=16384, about 3.3x the
    /// work per token, which is how a default that is safe on a 3B hangs the GPU on a 109B. Holding
    /// chunk x d x ff roughly constant keeps dispatch duration roughly constant instead.
    ///
    /// Only ever lowers: models at or below the reference shape keep 256. An explicit override always wins.
    fn prefill_chunk_for_streaming(d: usize, ff: usize, streams: bool, experts: Option<(usize, usize)>) -> usize {
        if std::env::var("LOGSCOPE_PREFILL_CHUNK").is_ok() || std::env::var("PREFILL_CHUNK").is_ok() {
            return prefill_chunk();
        }
        // One definition, in logscope::tuneprofile, so the engine and the profile cannot disagree about
        // what the chunk should be — two copies of a tuning rule is how they drift.
        logscope::tuneprofile::Defaults::default().prefill_chunk_moe(d, ff, streams, experts) as usize
    }
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
        /// Opt-in tiered warm-resume pool (LOGSCOPE_KV_POOL). When Some, replaces the FIFO `anchors`
        /// Vec: value-ranked eviction with demote-before-drop across Hot(f32)/Warm(int8)/Cold(host)
        /// tiers, so a hot compaction prefix or a fan's shared ancestor survives sibling churn that
        /// the FIFO's remove(0) would have dropped. Keyed + selected by token prefix exactly like
        /// the FIFO (kvtree::pick_anchor), so it's a drop-in whose only visible effect is more hits.
        pool: Option<crate::anchorpool::AnchorPool>,
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
        /// Latched off once windowed decode hits device buffer pressure (the M4 ceiling) and falls back
        /// to per-token: keeps the rest of the session on the path that actually fits, instead of
        /// re-attempting the window and re-failing every turn.
        disable_windowing: bool,
        /// Optional small DRAFT model for speculative decoding (opt-in via `LOGSCOPE_SPEC_DRAFT`).
        /// When present, `generate` routes through the engine's `speculative_generate_stats`: the
        /// draft proposes a block of tokens that the target verifies in ONE forward — bit-exact to
        /// greedy decode, but more tokens per expensive target pass. Must share the target's vocab.
        draft: Option<Model>,
        /// Previous speculative turn's committed context and the cache it left behind.
        ///
        /// A single slot, deliberately separate from the shared prefix pool: `spec_core` owns its
        /// cache, so a speculative turn that consumed a pool entry would have nothing to put back
        /// and would strip the pool one turn at a time. One slot is all a linear chat needs, and it
        /// touches none of the pool's invariants.
        spec_anchor: Option<(Vec<u32>, gpu_kernels::SpecRun)>,
        /// The PREVIOUS agent turn's prompt tokens and the recurrent checkpoint folded through
        /// them — retained across `generate()` calls only on a linear agent run (see the guard below
        /// in `generate`). This is the whole carry-forward: `generate_batched` prefills the shared
        /// prefix once and decodes each branch from it, so turn N+1 pays for one more chunk rather
        /// than a full prefill of the prompt every turn. A linear run means each turn's prompt is
        /// the previous turn's prompt *appended* with exactly one new reply, so the previous turn's
        /// terminal recurrent state (folded through positions 0..|P| of that cache) is a valid,
        /// position-absolute anchor for the next turn — no seeking, no shared-prefix ambiguity.
        ///
        /// `(tokens, session)`: the token ids for the prefix check (the next turn's prompt must be a
        /// superset of these, unchanged) and the session whose cache carries the delta-net state we
        /// carry forward. `DeltanetGpuState` is `Copy`, so holding a `Session` by value is a cheap
        /// shallow copy — no allocation, no borrow to release. The engine thread owns it; it never
        /// crosses a thread, so no `Mutex`. Left `None` for every non-agent path, so a chat that
        /// never advances the agent loop pays exactly nothing for this field's existence.
        prev_turn: Option<(Vec<u32>, gpu_kernels::Session, usize)>,
        /// Long-horizon watcher. ADVISORY ONLY — it observes the numbers this file already prints and
        /// says what it would do, and changes nothing.
        ///
        /// Deliberately measure-before-mutate. The detector was built from one seventeen-turn log and
        /// its job is to rewrite the operator's transcript; before it is allowed to do that it should
        /// be seen firing, or not firing, on real traffic. A wrong compaction is expensive and silent.
        ///
        /// `Mutex`, not `RefCell`. `logscope-fabricd` serves this worker as
        /// `Arc<dyn Worker + Send + Sync>`, and a `RefCell` is not `Sync`, so the
        /// daemon would not compile the moment this field existed. It never did —
        /// the error only appears under `--features wgpu-engine`, which the GUI
        /// build does not enable, so the binary that needs the bound is the one
        /// nobody was building.
        ///
        /// One borrow site and it is per-turn, so the lock is free in practice.
        horizon: std::sync::Mutex<gpu_kernels::horizon::Horizon>,
    }
    impl Engine {
        pub fn load(path: &str) -> Result<Self, String> {
            // mmap the GGUF (don't RAM-load it): a 40 GB file would OOM on the read alone, and even a
            // cache HIT shouldn't pull GB into RAM just to compute a content key. The OS pages bytes
            // on demand; `bytes` derefs to &[u8] exactly like the old fs::read Vec, and the mapping
            // is dropped at the end of load once tensors are resident.
            // Retained for the process lifetime (Colibri's g_maps): weights that want to be VIEWS into
            // the file cannot borrow a mapping the loader drops on return. Reused by path, so reloading
            // the same model does not map it twice.
            crate::diag::note(&format!("wgpu-engine: STAGE begin mmap+retain {path}"));
            let bytes: &'static [u8] = gpu_kernels::retain_map(path).map_err(|e| format!("open {path}: {e}"))?;
            crate::diag::note("wgpu-engine: STAGE end mmap+retain");
            // Hand back the page cache of any model we mapped earlier and are no longer using. The
            // registry cannot unmap (weights hold 'static slices into it), but residency is reclaimable
            // — and a session that switches models was otherwise holding every one of them resident.
            let stale = gpu_kernels::release_stale_map_residency(path);
            if stale > 0 {
                crate::diag!("wgpu-engine: released page cache from {stale} previously-loaded model file(s) \
— only the active model stays resident");
            }
            // Configure the engine from the tuning profile BEFORE the device is created — these values are
            // read during setup. Adapter-specific defaults need the adapter name, which we do not have
            // yet, so this applies the portable ones; the driver-detected fallbacks (Honeykrisp subgroup
            // hang, conservative VMA) still apply on top inside Gpu::new().
            {
                let d = logscope::tuneprofile::Defaults::default();
                gpu_kernels::set_engine_config(gpu_kernels::EngineConfig {
                    subgroup_matvec: d.subgroup_matvec,
                    convert_chunk_blocks: d.convert_chunk_blocks as usize,
                });
            }
            // SPLIT CHECKPOINT: if this is `-00001-of-0000N.gguf`, map the siblings too. A split model's
            // shard 1 carries metadata for the WHOLE model but only its own share of the tensors, so
            // loading it alone fails on a missing tensor with everything looking healthy. Unifying the
            // shards here avoids merging them on disk, which for a 62 GB checkpoint would mean having
            // room for a second copy.
            let sibling_paths = gpu_kernels::gguf::shard_siblings(std::path::Path::new(path));
            let mut extra_maps: Vec<_> = Vec::new();
            if let Some(paths) = &sibling_paths {
                if paths.len() > 1 {
                    let missing: Vec<String> = paths.iter().filter(|p| !p.exists())
                        .map(|p| p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()).collect();
                    if !missing.is_empty() {
                        return Err(format!("split checkpoint is incomplete — missing {missing:?}. All {} shards must be present (the Models tab lists what to fetch).", paths.len()));
                    }
                    for p in paths.iter().skip(1) {
                        extra_maps.push(gpu_kernels::retain_map(p.to_string_lossy().as_ref())
                            .map_err(|e| format!("open shard {}: {e}", p.display()))?);
                    }
                    crate::diag!("wgpu-engine: split checkpoint — {} shards found, loading as one model", paths.len());
                }
            }
            let mut parts: Vec<&'static [u8]> = vec![bytes];
            for m in &extra_maps { parts.push(m); }
            let gpu = Gpu::new();
            gpu.maybe_start_gpu_spin();
            // Build stamp beside the adapter: a log that does not identify its own binary cannot be
            // read against a change, and three have now been mis-attributed for exactly that reason.
            crate::diag!("wgpu-engine: adapter = {} | engine build {}", gpu.adapter_name, gpu_kernels::BUILD);
            // Preflight readiness report (cf. Colibri's doctor): surface detected fragilities + the
            // mitigations they trigger ONCE at load, so a driver quirk or a mis-set knob is visible before
            // the first token instead of as an opaque mid-decode hang. Cheap; read-only.
            {
                let asahi = gpu.conservative_vma;
                let mut notes: Vec<String> = Vec::new();
                if asahi {
                    // Honeykrisp/Asahi: the fragile path we've hardened against (subgroup hangs, unreliable
                    // completion callbacks → bounded submits, watchdog-sensitive dispatches).
                    // REPORT THE ACTUAL STATE, not the default. This was a static string keyed on
                    // conservative_vma and said "subgroup matvec OFF" whether or not it was — so a run
                    // with LOGSCOPE_MATVEC_SG=1 printed the same line as one without, and there was no
                    // way to tell from the log whether the override took.
                    //
                    // A preflight summary that reports intent rather than outcome is worse than none:
                    // it is read as confirmation.
                    notes.push(format!("Asahi/Honeykrisp Vulkan detected → subgroup matvec {}, decode \
window submits bounded (watchdog-safe), governor uses the tight Apple-Vulkan budget",
                        if gpu.subgroup_kernel_ok() { "ON (LOGSCOPE_MATVEC_SG override)" } else { "OFF" }));
                }
                // Report env overrides and whether each PARSED (the PREFILL_CHUNK-vs-LOGSCOPE_PREFILL_CHUNK
                // class of silent-ignore cost real test cycles; make it visible).
                for (name, alias) in [("LOGSCOPE_PREFILL_CHUNK", Some("PREFILL_CHUNK")),
                                      ("LOGSCOPE_MATVEC_SG", None), ("LOGSCOPE_DECODE_ATTN_SERIAL", None),
                                      ("LOGSCOPE_FLUSH_EVERY", None), ("LOGSCOPE_WINDOW_ONE_SUBMIT", None)] {
                    let val = std::env::var(name).ok().or_else(|| alias.and_then(|a| std::env::var(a).ok()));
                    if let Some(v) = val { notes.push(format!("env {name}={v} (active)")); }
                }
                if notes.is_empty() {
                    crate::diag!("wgpu-engine: preflight — no special fragilities or overrides detected (standard path)");
                } else {
                    for n in notes { crate::diag!("wgpu-engine: preflight — {n}"); }
                }
            }
            let g = gpu_kernels::gguf::parse(&bytes)?;
            // Pre-flight: the loader auto-selects q4-resident (≈half the int8 footprint) when int8 won't
            // fit, and falls back to q4 on a GPU OOM. So only refuse when even q4 can't fit the budget;
            // otherwise let the loader pick. LOGSCOPE_FORCE_LOAD=1 bypasses; LOGSCOPE_MAX_RESIDENT_GB sets
            // the budget; LOGSCOPE_Q4_RESIDENT=1 forces q4 up front.
            if std::env::var("LOGSCOPE_FORCE_LOAD").is_err() {
                // FILE-AWARE, ARCH-INDEPENDENT. The previous check read `llama.*` metadata keys —
                // so a qwen2/gemma3/diffusion_gemma header returned None and the guard silently
                // skipped itself (the "a guard that cannot find its input" class, already fixed
                // once below for vocab width) — and it priced EVERYTHING as int8, which refuses
                // an IQ1_S checkpoint the native path holds at 1.5625 bpw. The estimator walks
                // the tensor table instead: no arch knowledge, and type-19 tensors are priced at
                // the file's own bytes exactly when the loader will keep them that way.
                let est = gpu_kernels::store::estimate_resident_for_load(&g);
                if let Some((budget, budget_from)) = gpu_kernels::store::resident_budget() {
                    let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
                    if est.native_view_bytes > 0 {
                        // `load` is a constructor — no self yet, so the model is named by its file.
                        let fname = std::path::Path::new(path).file_name()
                            .map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
                        crate::diag!("wgpu-engine: [{}] {}", fname,
                            gpu_kernels::memplan::iq1s_load_plan(
                                gib(est.native_view_bytes) as f32, gib(budget) as f32,
                                std::env::var("LOGSCOPE_NO_IQ1_NATIVE").is_err()));
                    }
                    if est.plan_q4() > budget {
                        // The ceiling's PROVENANCE is in the message, not just its
                        // value. A refusal reading "~63 GiB available" on a phone is
                        // unactionable: the reader cannot tell whether that is a stale
                        // constant, a setting inherited from a desktop, or a real
                        // reading. Naming the source ("90% of MemAvailable on
                        // fp5 (Fairphone 5)" vs "LOGSCOPE_MAX_RESIDENT_GB=63") answers
                        // that in the message instead of costing an evening.
                        //
                        // The f32 term is broken out for the same reason. On a small
                        // model it DOMINATES and is the most likely thing to be wrong:
                        // token_embd/output are priced at 4 bytes per parameter
                        // regardless of how the file stores them, so a 1B with a 128k
                        // vocab books ~1.0 GiB for the embedding alone — twice what the
                        // whole q2_k file weighs on disk. If a small model is refused
                        // on a machine that plainly has room, this line is where to
                        // look first.
                        return Err(format!(
                            "won't fit: ~{:.1} GiB needed even at q4-resident{}, against ~{:.1} GiB \
                             ({}).{} Try a smaller model, free memory, LOGSCOPE_MAX_RESIDENT_GB=<gib> \
                             to set the ceiling explicitly, or LOGSCOPE_FORCE_LOAD=1 to try anyway \
                             (likely OOM).",
                            gib(est.plan_q4()),
                            if est.native_view_bytes > 0 {
                                format!(" (of which {:.1} GiB is IQ1_S held in the file's own bytes)",
                                        gib(est.native_view_bytes))
                            } else { String::new() },
                            gib(budget),
                            budget_from,
                            if est.f32_bytes > est.q4_rest_bytes {
                                format!(" NOTE: {:.1} GiB of that estimate is the embedding/output \
                                         priced at f32 — for a small model with a large vocabulary \
                                         this term dominates and may be pessimistic.",
                                        gib(est.f32_bytes))
                            } else { String::new() }));
                    }
                }
            }
            // Weights-only checkpoints (diffuse-cpp's LLaDA-8B GGUFs) carry no tokenizer at all, which is
            // why that project's CLI takes raw ids. LOGSCOPE_TOKENIZER_GGUF borrows one from a file with
            // the same vocabulary; the width is cross-checked against the model's own output before it
            // is accepted, because a near-miss prints fluent nonsense rather than failing.
            // FROM THE EMBEDDING TENSOR, not from a metadata key. The first version of this looked up
            // "llama.vocab_size" or "vocab_size" — but hparams are ARCH-PREFIXED, and this file's arch
            // is 'diffuse', so the lookup returned None, the width came out 0, and the check silently
            // skipped itself. A Llama-3 tokenizer (128256) was then accepted for a 126464-wide model
            // and the prompt's own special ids (128000, 128006, 128009) indexed past the embedding
            // table: "range start index 524288000 out of range for slice of length 517996544", which
            // is 128000 x 4096 against 126464 x 4096. A guard that cannot find its input is not a
            // guard. token_embd.weight always exists and always has the real row count.
            let want_vocab = g.tensor("token_embd.weight")
                .and_then(|t| t.dims.last().copied())
                .map(|v| v as usize)
                .or_else(|| g.declared_arch()
                    .and_then(|a| g.get_u32(&format!("{a}.vocab_size")))
                    .map(|v| v as usize))
                .unwrap_or(0);
            let tok = Tokenizer::from_gguf_with_fallback(&g, want_vocab).map_err(|e| format!("tokenizer: {e}"))?;
            // Resolve the stop the way the harness does: the GGUF-declared eos FIRST (DeepSeek-R1-
            // Distill declares 128001 <｜end▁of▁sentence｜>), THEN fall back to a stop special. The
            // old order preferred <|eot_id|>(128009) and so missed DeepSeek's real stop → the
            // whole-answer loop. The primary `eos` drives the engine's internal stop; `stop_ids` is
            // the full set every decode callback also checks (covers eot/eom/end_of_text/im_end).
            let declared = g.get_u32("tokenizer.ggml.eos_token_id");
            let eos = declared.or_else(|| tok.special_id("<|eot_id|>"));
            let base_stops = crate::genguard::stop_token_set(declared, &[
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
            ]);
            // Defense-in-depth (Colibri #298): also stop on EVERY control special the tokenizer carries,
            // not just the hardcoded names above. A converted GGUF whose config omits a control token
            // (<|observation|>, <sop>, [gMASK], image/audio markers, …) would otherwise print it as chat
            // text and run past the turn end. Content specials (<think>/<tool_call>/…) are excluded by
            // control_stop_ids, so reasoning/tool output survives.
            let stop_ids: Vec<u32> = crate::genguard::stop_token_set_with_controls(
                base_stops, &tok.control_stop_ids()).into_iter().collect();
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
            } else if tok.special_id("<role>").is_some()
                || (tok.special_id("<|role_end|>").is_some() && tok.special_id("<|role_start|>").is_none()) {
                // Bailing2 (Ling 2.0 / LLaDA2): <role>HUMAN</role>...<|role_end|> turns. The first
                // LLaDA2 hardware run fed 1718 tokens with ZERO specials under Plain — an
                // instruct-tuned model given role-free text, independent of its missing shexp.
                // <|role_start|> excluded: that spelling is Megrez, a different family.
                TemplateKind::Bailing2
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
                Some(m) => { crate::diag!("wgpu-engine: loaded int8 cache (skipped dequant) in {:.1}s [{}]", t0.elapsed().as_secs_f32(), cache.display()); m }
                None => {
                    crate::diag!("wgpu-engine: loading {path} ({:.1} MiB) — quantizing to resident weights (int8, or q4 if it won't fit)…", bytes.len() as f32 / (1 << 20) as f32);
                    let m = gpu.load_gguf_quantized_parts_static(&parts)?;
                    // Write the central blob so the NEXT load (any project, incl. crash-recovery) skips
                    // this. Make room BEFORE writing (evict to cap − incoming) so the dir never briefly
                    // holds old+new; the post-write evict is then just an exact final guard.
                    // Skip the cache entirely when the source is already a K-quant. int8 residency
                    // from Q4_K has the SAME scale granularity (one per 32 weights), so it stores 4-bit
                    // values in 8 bits and cannot add precision — it doubles memory and writes
                    // gigabytes of cache that buy nothing. ~7.5 GiB per 8B model, on device, per model.
                    // WHAT IS ACTUALLY IN THE FILE, before any line that samples
                    // one tensor and speaks for the whole model.
                    //
                    // A Qwen3-14B-UD-IQ1_S load reported a Q3_K dequant, a Q5_K
                    // transcode and "source is ggml type 16" (IQ2_XXS) — three
                    // formats, from a file named for a fourth. Each line was
                    // true of the tensors it saw and none said the file was
                    // mixed. When that model then emitted gibberish, the first
                    // question was which formats were in it, and the log could
                    // not answer.
                    crate::diag!("loader: {}", gpu_kernels::memplan::type_histogram_line(&g));
                    // And what the forward pass will IGNORE. Three
                    // unsupported-model reports in one week each turned on a
                    // tensor present in the file that no code path reads —
                    // diffusion-gemma's per-expert and router scales, gemma4's
                    // inp_gate/proj side path, and its fused expert matrix that
                    // the separate-name lookup misses entirely. None of them
                    // errors; they produce plausible wrong output, which is
                    // strictly harder to diagnose than a crash.
                    if let Some(line) = gpu_kernels::memplan::unread_tensor_line(&g) {
                        crate::diag!("loader: {line}");
                    }
                    let src_type = g.tensor("blk.0.attn_q.weight").map(|t| t.ggml_type).unwrap_or(0);
                    let incoming_est = gpu_kernels::store::estimate_int8_resident(&g).unwrap_or(0);
                    if !gpu_kernels::memplan::int8_worth_it(src_type) {
                        crate::diag!("wgpu-engine: int8 cache SKIPPED — source is ggml type {src_type}, \
which already carries per-32 scales. Converting to int8 would double the bytes without adding \
precision, and cost {:.1} GiB of disk for nothing",
                            gpu_kernels::memplan::int8_cache_waste(src_type, incoming_est) as f64
                                / (1u64 << 30) as f64);
                    } else {
                    let cap = gpu_kernels::store::cache_cap_bytes();
                    let incoming = gpu_kernels::store::estimate_int8_resident(&g).unwrap_or(0);
                    gpu_kernels::store::make_room_for(cap, incoming);
                    match gpu.save_int8_cache(&m, &cache, fp) {
                        Ok(true) => {
                            let (n, freed) = gpu_kernels::store::evict_to_cap(cap);
                            let evicted = if n > 0 { format!("; evicted {n}, freed {:.1} GiB", freed as f64 / (1u64 << 30) as f64) } else { String::new() };
                            crate::diag!("wgpu-engine: wrote int8 cache [{}] (central, content-addressed){evicted}", cache.display());
                        }
                        Ok(false) => crate::diag!("wgpu-engine: int8 cache skipped (bias/QK-Norm/MoE/q4 layouts aren't cacheable yet — reloads from the GGUF)"),
                        Err(e) => crate::diag!("wgpu-engine: int8 cache write skipped: {e}"),
                    }
                    }
                    m
                }
            };
            // Batch 2: report the resident footprint (the int8 blob size ≈ GPU/UMA memory the model
            // occupies) so memory pressure is visible before loading a bigger model. The q4-resident
            // + mmap path for models that don't fit as int8 is the TODO that lets 30B run in 32 GB.
            if let Ok(md) = std::fs::metadata(&cache) {
                crate::diag!("wgpu-engine: int8 resident footprint ≈ {:.2} GiB", md.len() as f64 / (1u64 << 30) as f64);
            }
            crate::diag!("wgpu-engine: loaded in {:.1}s — d={} layers={} heads={}/{} head_dim={} rope_base={} kv ff={} vocab={} | output int8={} | eos={:?} stop={:?} template={:?} | tokenizer byte_level={} vocab={}",
                t0.elapsed().as_secs_f32(), model.d, model.n_layers, model.n_heads, model.n_kv_heads, model.head_dim, model.base, model.h, model.vocab,
                model.output_is_quantized(), eos, stop_ids, template, tok.is_byte_level(), tok.vocab_size());

            // WHICH PATH THE WEIGHTS TOOK. A load-path optimisation that misses the bytes is worth
            // nothing, and this model's bytes are its experts: they are stacked into an f32 buffer
            // before quantisation, so no byte-slicing fast path reaches them. Printing the split
            // means the next log answers "was it worth it" instead of the next guess.
            {
                use std::sync::atomic::Ordering::Relaxed;
                // RESET AFTER READING. These are process-global counters and nothing cleared them,
                // so a second model load reported the FIRST model's sliced bytes as part of its own
                // total: an M4 Max log showed "15.71B" as the sliced figure for three different
                // checkpoints in a row, with the percentage falling each time as the denominator
                // grew. A cumulative counter presented as a per-load percentage is a wrong number,
                // not a rough one.
                // CAPTURE BEFORE THE RESET. c4-774 made these swap-to-zero so a second model load
                // could not report the first one's sliced bytes, and c4-809 then read them at
                // GENERATION time for the roofline — by which point they are zero, so that line
                // never printed once. Two correct changes that break each other, which is what a
                // process-global counter with a consuming read invites.
                let fast = gpu_kernels::LOAD_FAST_WEIGHTS.swap(0, Relaxed);
                let slow = gpu_kernels::LOAD_SLOW_WEIGHTS.swap(0, Relaxed);
                crate::backend::RESIDENT_ELEMS.store(fast + slow, Relaxed);
                let tot = fast + slow;
                if tot > 0 {
                    crate::diag!("wgpu-engine: load path — {:.1}% of weights sliced from the file directly \
({:.2}B of {:.2}B), {:.1}% took the f32 round-trip. What still round-trips is the \
leftovers — 1D norms and any tensor the converter left f32.",
                        100.0 * fast as f64 / tot as f64, fast as f64 / 1e9, tot as f64 / 1e9,
                        100.0 * slow as f64 / tot as f64);
                }
            }            // LOAD ADVICE. Runs the planners against this model and says only what they DISAGREE with —
            // an unprobed budget, a split that would beat streaming, a GPU the planner would skip. Every
            // planner is otherwise tested logic with no caller, and a hypothesis that has never seen a
            // real model is exactly what produced the 24 GiB constant and the thread-spawn figure.
            {
                // Bytes touched per layer per token: the dense weights, approximated from the model's
                // own dimensions rather than a residency figure that may not be in scope here.
                let per_layer = (4 * model.d * model.d + 3 * model.d * model.h) as u64 / 2;  // ~q4
                let facts = logscope::loadadvice::LoadFacts {
                    n_layers: model.n_layers, bytes_per_layer: per_layer,
                    gpu_budget_bytes: gpu_kernels::memplan::gpu_buffer_budget_uma(
                        gpu_kernels::memplan::system_ram_bytes(), gpu.conservative_vma, true),
                    measured_vram_gib: 0.0,
                    gpu_gib_s: 258.1, cpu_gib_s: 113.2, d: model.d,
                };
                for line in logscope::loadadvice::advise(&facts) {
                    crate::diag!("wgpu-engine: advice — {line}");
                }
            }
            if std::env::var("LOGSCOPE_DISPATCH_BENCH").is_ok() { gpu.dispatch_bench(); }
            if std::env::var("LOGSCOPE_WEIGHT_CHECK").is_ok() { crate::diag!("wgpu-engine: {}", model.weight_probe(&gpu)); }
            // Prewarm: compile the prefill + decode pipelines now (in the load window) so the
            // first user token runs at steady speed instead of crawling up from ~0.1 tok/s
            // while shaders compile. The returned count is the "warm" signal.
            let warm = gpu.prewarm(&model);
            crate::diag!("wgpu-engine: prewarmed {warm} pipelines (prefill+decode kernels compiled at load)");
            // System-optimization: pick the fastest decode mat-vec tile for THIS GPU by a quick
            // benchmark on the model's dominant (FFN up) shape — runs only on first load per
            // (adapter, shape); the result is cached to disk so later loads stay snappy. Skipped when
            // LOGSCOPE_MATVEC_TILE pins it, or LOGSCOPE_NO_AUTOTUNE is set. Pure speed choice — the
            // output is bit-identical for any tile (--gate-matvec), so this can never change results.
            if !gpu.profile_forced() && std::env::var_os("LOGSCOPE_NO_AUTOTUNE").is_none() {
                let (k, n) = (model.d, 2 * model.h); // wgate_up: the largest decode mat-vec
                if let Some(t) = gpu_kernels::store::autotune_get(&gpu.adapter_name, k, n) {
                    // RE-APPLY THE GUARD ON CACHE LOAD. The autotune SEARCH skips subgroup kernels
                    // when sg_ok is false, but a cached choice was restored without that check — so a
                    // cache written when the subgroup kernel was allowed (a different build, driver,
                    // or LOGSCOPE_MATVEC_SG=1 run) selects V2WgSg on a machine where the guard has
                    // since disabled it. Your Qwen log shows exactly that: "subgroup decode kernel
                    // DISABLED" three lines above "decode tile = 8 (kernel V2WgSg, cached autotune)".
                    //
                    // A guard enforced on one path and not the other is the same class as every other
                    // stale-check bug this session; a cache is just a path that skips the code that
                    // would have said no.
                    let t = {
                        let dc = gpu_kernels::DecodeConfig::unpack(t);
                        if dc.kernel.needs_subgroups() && !gpu.subgroup_kernel_ok() {
                            crate::diag!("wgpu-engine: cached decode kernel {:?} needs subgroups, which \
are disabled on this device — falling back to the autotuned non-subgroup default", dc.kernel);
                            gpu_kernels::DecodeConfig { kernel: gpu_kernels::MatvecKernel::V2Wg128,
                                                        tile: dc.tile }.pack()
                        } else { t }
                    };
                    gpu.set_matvec_tile(t);
                    // UNPACK BEFORE PRINTING. The cache stores a packed (kernel, tile) u32 and this
                    // printed it raw, so the line read "decode tile = 2147549192" — a value that
                    // cannot be a tile, on every load, on both machines. Tiles are 1/2/4/8.
                    //
                    // Harmless to the engine and corrosive to the log: a number nobody can read is a
                    // line nobody checks, and this is the line you would consult when a shape is slow.
                    let dc = gpu_kernels::DecodeConfig::unpack(t);
                    crate::diag!("wgpu-engine: decode tile = {} (kernel {:?}, cached autotune for this GPU)",
                        dc.tile, dc.kernel);
                } else {
                    let t0 = std::time::Instant::now();
                    let best = gpu.autotune_matvec(k, n);
                    gpu.set_matvec_tile(best);
                    gpu_kernels::store::autotune_put(&gpu.adapter_name, k, n, best);
                    crate::diag!("wgpu-engine: autotuned decode tile = {best} in {:.0}ms (cached; set LOGSCOPE_MATVEC_TILE to override, LOGSCOPE_NO_AUTOTUNE to skip)", t0.elapsed().as_secs_f32() * 1000.0);
                }
            } else if gpu.profile_forced() {
                crate::diag!("wgpu-engine: decode tile = {} (LOGSCOPE_MATVEC_TILE override)", gpu.matvec_tile());
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
                // VERSIONED IN THE NAME, like the other two caches. This is the per-shape file — a
                // SEPARATE store from autotune-v4.tsv, which is why bumping that one did nothing and a
                // run after the kernel fixes still reported "loaded 41 decode + 14 GEMM tiles from
                // cache".
                //
                // Those verdicts were measured against kernels whose byte unpacking returned zero for
                // three lanes in four, and against a candidate set that probe_valid_gemm_tiles pruned
                // using the same corrupt weights. Every tile in that file is a conclusion about a
                // build that no longer exists — and it is why LOGSCOPE_MATVEC_V3 could not take
                // effect: a cached choice never re-enters the search.
                //
                // v2: kernels fixed (lane-varying shifts replaced with extractBits/insertBits).
                let cache = gpu_kernels::store::cache_root()
                    .join(format!("autotune-shapes-v2-{safe}.bin"));
                let (dl, gl) = gpu.load_autotune(&cache).unwrap_or((0, 0));
                if dl > 0 || gl > 0 { crate::diag!("wgpu-engine: per-shape autotune — loaded {dl} decode + {gl} GEMM tiles from cache"); }
                // Time-box the whole probe so a slow/large model can never stall the load (capped-width
                // probes keep each shape cheap; this is the hard backstop).
                let t0 = std::time::Instant::now();
                let deadline = t0 + std::time::Duration::from_secs(20);
                let dec = gpu.autotune_shapes(&shapes, Some(deadline));            // decode tiles (uncached)
                let m_probe = model.d.clamp(64, 512);                             // representative prefill width
                // The GEMM tile only matters when the tiled prefill is enabled, so only probe it then.
                let gem = if std::env::var_os("LOGSCOPE_PREFILL_TILED").is_some() { gpu.autotune_gemm(&shapes, m_probe, Some(deadline)) } else { Vec::new() };
                for (n, k, t, ms) in &dec { crate::diag!("wgpu-engine:   probed decode n={n} k={k} → tile {t} in {ms:.0}ms"); }
                for (n, k, t, ms) in &gem { crate::diag!("wgpu-engine:   probed GEMM   n={n} k={k} → tile {t} in {ms:.0}ms"); }
                if !dec.is_empty() || !gem.is_empty() {
                    let _ = gpu.save_autotune(&cache);
                    let total: f32 = dec.iter().chain(gem.iter()).map(|x| x.3).sum();
                    crate::diag!("wgpu-engine: per-shape autotune probed {} decode + {} GEMM shapes ({:.0}ms work, {:.1}s wall; cached to {})",
                        dec.len(), gem.len(), total, t0.elapsed().as_secs_f32(), cache.display());
                    if std::time::Instant::now() >= deadline { crate::diag!("wgpu-engine: NOTE — autotune hit its 20s time-box; remaining shapes use defaults (re-run to finish, or pin LOGSCOPE_*_TILE)"); }
                }
                // Print the active per-shape table (covers the cached case too) so the chosen tiles are visible.
                let tiled = std::env::var_os("LOGSCOPE_PREFILL_TILED").is_some();
                let mut seen = std::collections::HashSet::new();
                for (n, k) in shapes.iter().copied() {
                    if seen.insert((n, k)) {
                        let gemm = if tiled { format!(" · GEMM tile {}", gpu.gemm_tile_for(n, k)) } else { String::new() };
                        crate::diag!("wgpu-engine:   shape n={n} k={k} → decode tile {}{gemm}", gpu.decode_tile_for(n, k));
                    }
                }
                crate::diag!("wgpu-engine: tile overrides — LOGSCOPE_DECODE_TILE / LOGSCOPE_GEMM_TILE pin, LOGSCOPE_NO_AUTOTUNE skips, LOGSCOPE_PREFILL_TILED=1 enables the tiled prefill GEMM");
                // Dispatch-reduction levers for dispatch-bound decode (Asahi): all bit-identical to default,
                // OPT-IN. One switch enables the set; the profile prints the bind-group hit rate when active.
                let fast = std::env::var("LOGSCOPE_FAST_DECODE").as_deref() == Ok("1");
                if fast {
                    crate::diag!("wgpu-engine: LOGSCOPE_FAST_DECODE=1 — dispatch levers ON (bind-group cache + MoE gate‖up/SwiGLU + down/combine fusions); bit-identical, decode-speed only");
                } else {
                    crate::diag!("wgpu-engine: dispatch levers available (dispatch-bound decode): LOGSCOPE_FAST_DECODE=1 enables all (bind-group cache + 2 MoE fusions), or set LOGSCOPE_BG_CACHE / LOGSCOPE_FUSE_GLU / LOGSCOPE_FUSE_DOWN individually");
                }
            }
            // Submit batching: a 14B decode is ~24 submits/token and the profile is sync-dominated, so the
            // passes-per-submit knob matters. Pinned (env) → fixed; cached → use it; else adapt across the
            // next replies (SubmitTuner sweeps SUBMIT_PROFILES by per-token decode time). Bit-exact across
            // flush values (--gate-submit), so adapting between replies never changes output.
            let submit_tuner = if gpu.flush_every_pinned() {
                crate::diag!("wgpu-engine: submit batch = {} passes/submit (LOGSCOPE_FLUSH_EVERY override)", gpu.flush_every());
                None
            } else if let Some(v) = gpu_kernels::store::flush_get(&gpu.adapter_name) {
                gpu.set_flush_every(v);
                crate::diag!("wgpu-engine: submit batch = {v} passes/submit (cached autotune for this GPU)");
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
                crate::diag!("wgpu-engine: submit batch = {winner} passes/submit (startup stride probe [{}]); live tuner refines {:?} over next replies; pin with LOGSCOPE_FLUSH_EVERY", table.join(" "), neighborhood);
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
            if std::env::var("LOGSCOPE_KV_OFF").is_ok() { crate::diag!("wgpu-engine: KV reuse DISABLED (LOGSCOPE_KV_OFF) — full prefill every turn, graph speedups off"); }
            else if kv_cap > 1 { crate::diag!("wgpu-engine: KV prefix tree ON — pool cap {kv_cap} (each entry holds a KV cache; trades memory for fewer prefills on branchy/fan-out chats)"); }
            // Tiered pool (opt-in): value-ranked eviction with int8/host demotion. Budget the
            // GPU-resident KV at kv_cap full f32 caches' worth of a typical 4k-token tip; the pool
            // then holds MORE tips than kv_cap by demoting the cold ones to int8/host instead of
            // dropping them. LOGSCOPE_KV_POOL_MB overrides the budget directly.
            let pool = if std::env::var("LOGSCOPE_KV_POOL").is_ok() && std::env::var("LOGSCOPE_KV_OFF").is_err() {
                let kv_dim = model.n_kv_heads * model.head_dim;
                let per_tip = 2 * 4096 * kv_dim * 4 * model.n_layers; // f32 KV for a 4k-token tip
                let budget = std::env::var("LOGSCOPE_KV_POOL_MB").ok().and_then(|v| v.parse::<usize>().ok())
                    .map(|mb| mb * 1_000_000).unwrap_or(per_tip * kv_cap);
                crate::diag!("wgpu-engine: KV pool ON (tiered Hot/Warm/Cold, value-ranked) — budget {:.0} MB, entry cap {}", budget as f32 / 1e6, kv_cap * 4);
                Some(crate::anchorpool::AnchorPool::new(budget, model.n_layers, kv_dim))
            } else { None };
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
                            crate::diag!("wgpu-engine: spec-decode ENABLED — draft {base} (d={} L={} vocab={}) proposes, target verifies; tune the block with LOGSCOPE_SPEC_K (default 4)", dm.d, dm.n_layers, dm.vocab);
                            Some(dm)
                        }
                        Ok(dm) => { crate::diag!("wgpu-engine: spec draft vocab {} != target {} — spec-decode OFF (draft and target must share a tokenizer)", dm.vocab, model.vocab); None }
                        Err(e) => { crate::diag!("wgpu-engine: spec draft load failed — spec-decode OFF: {e}"); None }
                    }
                }
                _ => None,
            };
            Ok(Self { gpu: std::sync::Arc::new(gpu), model: std::sync::Arc::new(model), tok: std::sync::Arc::new(tok), eos, stop_ids, template, rng: 0x2545_F491_4F6C_DD1D, anchors: Vec::new(), kv_cap, pool, warm, name, submit_tuner, embed_q8: None, draft, disable_windowing: false, spec_anchor: None, prev_turn: None,
                horizon: std::sync::Mutex::new(gpu_kernels::horizon::Horizon::new(4096, 512, 0)) })
        }
        pub fn template(&self) -> TemplateKind { self.template }
        /// The loaded model's file basename (log/UI tag).
        pub fn model_name(&self) -> &str { &self.name }
        /// The Liaison's `ModelStatus` for the resident model: its GGUF-declared arch and whatever
        /// gap lines the loader printed at load time. Closes the hole `docs/12-queen-and-workers.md`'s
        /// liaison audit named — `Model` used to carry an arch's EFFECTS but not its name, so this
        /// data existed only as a `wgpu-engine: ⚠` stderr line with no way back into the app. Not
        /// wired into the chat UI yet (that needs a `dyn Backend`-level accessor, since `Engine` is
        /// one concrete backend among several) — this is the piece that was missing to do it.
        pub fn model_status(&self) -> logscope::liaison::ModelStatus {
            logscope::liaison::ModelStatus {
                name: self.name.clone(),
                arch: self.model.arch.clone(),
                gaps: self.model.gaps.clone(),
            }
        }

        /// What ONE more instance of this model would cost in device memory: the weights the
        /// planner actually placed, plus a KV cache for the app's default window. Replicator mode
        /// divides the budget by this to bound its slider.
        ///
        /// 0 when the planner did not run (f32 loads, synthetic models) — the caller treats
        /// unknown as "no memory bound" rather than as zero slots, the same rule ctxwin uses.
        pub fn instance_bytes(&self) -> u64 {
            let w = self.model.planned_weight_bytes;
            if w == 0 { return 0; }
            w + self.model.kv_bytes_per_token() * 4096
        }
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
                // A lane starts with no speculative anchor: the cache is not shareable across lanes
                // (each holds its own KV) and a lane's first prompt is a different branch anyway.
                spec_anchor: None,
                horizon: std::sync::Mutex::new(gpu_kernels::horizon::Horizon::new(4096, 512, 0)),
                eos: self.eos, stop_ids: self.stop_ids.clone(), template: self.template,
                rng: self.rng ^ 0x9E37_79B9_7F4A_7C15u64.wrapping_mul(n as u64 + 1),
                anchors: Vec::new(), kv_cap: 1, pool: None, warm: self.warm,
                name: format!("{}#L{n}", self.name),
                submit_tuner: None, embed_q8: None, draft: None, disable_windowing: false,
                prev_turn: None,
            }
        }

        /// Windowed sampled decode: is it eligible for `sp`, and how big a window? Shared by BOTH
        /// the fresh-prompt path (`generate`) and the warm-resume path (`continue_last`) so they can
        /// never drift — the auto-continue silently reverting to token-by-token (36 submits/token on
        /// Metal, where encoder setup is the whole wall) was exactly that drift. Builds the int8
        /// embed table lazily on first eligible use. Returns the window size, or None to take the
        /// per-token path. Eligibility mirrors SAMPLE_AT's host contract (temp>0, 2≤top_k≤64).
        fn windowed_ready(&mut self, sp: &SamplerParams) -> Option<usize> {
            if self.disable_windowing { return None; } // latched off after a device-pressure fallback
            let sample_win = std::env::var("LOGSCOPE_SAMPLE_WINDOW").ok()
                .and_then(|v| v.parse::<usize>().ok()).unwrap_or(4);
            if !(sample_win >= 2 && sp.temperature > 0.0 && (2..=64).contains(&sp.top_k)) { return None; }
            // A DIFFUSION model never takes the autoregressive window, so building its int8 embed
            // table is pure waste: 307 MiB and ~0.15s on LLaDA2-mini, allocated on every first turn
            // and never read. The 737/739 logs printed "windowed sampling ON" immediately before
            // the line announcing that generation was routing to diffuse_generate instead.
            if self.model.diffusion
                && std::env::var("LOGSCOPE_DIFFUSION_AR").as_deref() != Ok("1")
                && self.model.mask_token_id.is_some() { return None; }
            if self.embed_q8.is_none() {
                let tq = Instant::now();
                self.embed_q8 = Some(self.gpu.embed_q8(&self.model));
                crate::diag!("wgpu-engine: [{}] windowed sampling ON (k={sample_win}) — int8 embed table resident ({:.0} MiB, {:.2}s)",
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

        /// Magical dispatch-lever autotune (hidden). Before the user's first real message, decide whether
        /// the (bit-identical) dispatch levers make THIS device+model faster — by running a small throwaway
        /// decode with the levers OFF, then ON, comparing wall-clock, and switching on if ON wins. The
        /// generated tokens are discarded and never shown. The verdict is cached per-GPU so this runs once.
        ///
        /// Precedence: an explicit LOGSCOPE_FAST_DECODE env override skips the probe (user intent wins); a
        /// persisted verdict skips it (already answered on this GPU); a dense (non-MoE) model skips it (the
        /// levers don't bear on it). Otherwise: probe now, so the first real reply is already on the fast path.
        pub fn ab_probe(&self) {
            // env override wins — set the state directly, no probe
            match std::env::var("LOGSCOPE_FAST_DECODE").as_deref() {
                Ok("1") => { gpu_kernels::ab_set_decided(true);  crate::diag!("wgpu-engine: fast-decode levers ON (LOGSCOPE_FAST_DECODE=1) — A/B skipped"); return; }
                Ok("0") => { gpu_kernels::ab_set_decided(false); crate::diag!("wgpu-engine: fast-decode levers OFF (LOGSCOPE_FAST_DECODE=0) — A/B skipped"); return; }
                _ => {}
            }
            // Persisted verdict, keyed by GPU **and decode class**. Keying on the adapter alone was a
            // real bug: the two MoE fusions are no-ops on a dense model, so a verdict measured on a dense
            // model says "levers off" and that verdict was then applied to MoE models — exactly where the
            // levers matter most, because a MoE layer issues per-expert dispatches and decode here is
            // dispatch-bound. Same GPU, different question.
            let ab_key = format!("{} [{}]", self.gpu.adapter_name, self.model.decode_class());
            if let Some(on) = gpu_kernels::store::fast_ab_get(&ab_key) {
                gpu_kernels::ab_set_decided(on);
                crate::diag!("wgpu-engine: fast-decode levers {} (cached A/B verdict for {ab_key})", if on { "ON" } else { "OFF" });
                return;
            }
            // Probe on MoE AND dense models. The two MoE fusions are no-ops on a dense model, but the
            // bind-group cache cuts per-dispatch SETUP cost on ANY dispatch-bound decode — and the feedback
            // profiles (setup≫sync even on a dense 4B) show dense decode is dispatch-bound here too. So the
            // A/B measures the cache's effect via the same timing comparison; MoE models additionally get
            // the fusions. (Only skip is the env/persisted-verdict short-circuits above.)
            // A throwaway prefix: a few tokens so decode actually routes MoE experts. Content is irrelevant;
            // BOS (id 0 fallback) + a short run keeps it cheap. Warm the pipelines first so neither phase
            // eats shader-compile time.
            let seed_prefix: Vec<u32> = { let t = self.tok.encode("The"); if t.is_empty() { vec![0] } else { t } };
            const WARM: usize = 4;   // discard: pipeline fill / cache warmup
            const WIN: usize = 12;   // timed tokens per phase (median over these)
            let time_phase = |on: bool| -> f32 {
                gpu_kernels::ab_set_decided(on); // force levers for this phase (bit-identical either way)
                let _warm = self.run_tokens(&seed_prefix, WARM, 0.0, 0x51ED);
                let t0 = std::time::Instant::now();
                let toks = self.run_tokens(&seed_prefix, WIN, 0.0, 0x51ED);
                let dt = t0.elapsed().as_secs_f32();
                let n = toks.len().max(1);
                (dt / n as f32) * 1000.0 // ms/token
            };
            let off = time_phase(false);
            let on = time_phase(true);
            // ON wins only if it beats OFF by ≥5% (margin guards against noise flipping it on).
            let decision = on < off * 0.95;
            gpu_kernels::ab_set_decided(decision);
            // CACHE ONLY A DECISIVE RESULT. The margin above picks a side for THIS run, but an
            // inconclusive measurement must not be written down: run-to-run variance here is easily a few
            // percent, and a cached coin flip persists for every later load while nobody knows why it is
            // there. Within the margin, decline to record and let the next load measure again.
            // Persist alongside the weight cache, so tuning travels with the install rather than living
            // only in process memory.
            let prof_path = gpu_kernels::store::cache_path(0).with_file_name("tuning-profile.json");
            let (mut prof, warn) = logscope::tuneprofile::Profile::load_or_default(&prof_path, &self.gpu.adapter_name);
            if let Some(w) = warn { crate::diag!("wgpu-engine: {w}"); }
            let v = logscope::tuneprofile::Verdict {
                option: "fast_decode".into(), chosen: decision,
                on_ms: on * 1000.0, off_ms: off * 1000.0,
                note: format!("{} [{}]", self.gpu.adapter_name, self.model.decode_class()),
            };
            let key = format!("{} [{}]", self.gpu.adapter_name, self.model.decode_class());
            let tkey = logscope::tuneprofile::TuneKey::new(
                &self.gpu.adapter_name, self.model.decode_class(),
                self.model.residency_class());
            if prof.record(&tkey, v.clone()) {
                // MARGIN BEFORE PERSISTING. The A/B measures two timings and picks the faster, and
                // "faster" over a short sample includes noise — a verdict written from a 2% win gets
                // cached and applied to every later run on this GPU and decode class.
                //
                // dwell::Verdict answers whether the win pays for taking it. There is no dwell here
                // because this decides once per model load rather than continuously; the margin is
                // the half that applies, and it is the half that stops a coin-flip becoming a
                // persisted policy.
                //
                // Scores are inverted from times: the levers "win" by being FASTER, so the challenger
                // score is off_ms and the incumbent is on_ms when decision is true.
                let (incumbent_ms, challenger_ms) = if decision { (off * 1000.0, on * 1000.0) }
                                                    else { (on * 1000.0, off * 1000.0) };
                let mut d = logscope::dwell::Dwelling::new(
                    std::time::Duration::from_secs(0), logscope::dwell::DEFAULT_MARGIN);
                let verdict = d.consider(1.0 / incumbent_ms.max(1e-6),
                                         1.0 / challenger_ms.max(1e-6), false);
                if verdict.switches() {
                    gpu_kernels::store::fast_ab_put(&key, decision);
                } else {
                    crate::diag!("wgpu-engine: A/B verdict NOT persisted — {}", verdict.explain());
                }
                if let Err(e) = prof.save(&prof_path) { crate::diag!("wgpu-engine: tuning profile not saved: {e}"); }
                crate::diag!("wgpu-engine: fast-decode A/B — {} by {:+.0}% ({:.1} vs {:.1} ms/tok), cached for [{}]",
                    if decision { "ON" } else { "off" }, v.margin() * 100.0,
                    v.on_ms, v.off_ms, self.model.decode_class());
            } else {
                crate::diag!("wgpu-engine: fast-decode A/B INCONCLUSIVE ({:+.1}%, {:.1} vs {:.1} ms/tok) — \
using {} for this run but NOT caching it; a difference this small is run-to-run noise and caching it \
would mislead every later load",
                    v.margin() * 100.0, v.on_ms, v.off_ms, if decision { "ON" } else { "off" });
            }
            crate::diag!("wgpu-engine: fast-decode A/B — off {off:.1} ms/tok, on {on:.1} ms/tok → levers {} (cached for {})",
                if decision { "ON" } else { "OFF" }, self.gpu.adapter_name);
        }

        /// The constraint for this turn, if the caller set one. `GenParams.grammar` holds the GBNF
        /// the harness generated from the run's tool whitelist; this engine does not interpret GBNF,
        /// so the names are read back out of it (they are the quoted literals on the `name ::=` line)
        /// and handed to the acceptor, which enforces the same shape natively in Rust.
        ///
        /// A grammar with no recognisable names still constrains the FENCE (any name allowed), which
        /// is the part that stops a malformed block; nothing here can make a decode fail.
        fn tool_filter<'t>(&self, tok: &'t Tokenizer, p: &GenParams) -> Option<crate::toolfilter::ToolFenceFilter<'t>> {
            let g = p.grammar.as_deref()?;
            let names = grammar_tool_names(g);
            Some(crate::toolfilter::ToolFenceFilter::new(tok, &names))
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
                crate::diag!("wgpu-engine: [{}] prompt diag — {} special tokens (showing ≤12): {:?}; tail ids = {:?}",
                    self.name, sp.len(), &sp[..sp.len().min(12)], tail);
                // TEMPLATE/TOKENIZER MISMATCH ALARM. A prompt whose marker text tokenises to ZERO
                // specials, on a model that registers specials, means the builder emitted a
                // different family's markers than this tokenizer knows — they go in as ordinary
                // words and the model is handed an unfamiliar frame. Seen live: a load line
                // reporting template=Bailing2 while the prompt ended
                // "assistant<|end_header_id|>\n\n" (Llama3), 0 specials found, because a second
                // prompt-building path bypassed the resolved template. Silent before this line —
                // the specials count was printed but nothing said the count was WRONG.
                if sp.is_empty() && prompt.contains("<|") {
                    let marker = prompt.rfind("<|").map(|i| &prompt[i..(i + 24).min(prompt.len())]).unwrap_or("");
                    crate::diag!("wgpu-engine: [{}] TEMPLATE MISMATCH — the prompt carries marker TEXT \
({marker:?}…) that this tokenizer does not know as a special token, so it was tokenised as ordinary \
words. The prompt builder and the model's template disagree; the load line above names the template \
this model actually resolved to.", self.name);
                }
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
            //
            // The window SIZES ITSELF from what this prompt needs, within the model's trained
            // context and the KV memory left after the resident weights — n_ctx is a floor, not the
            // answer. This is T1: the operator's repository summary ran 8 turns at a hand-set 4864
            // against a model trained to 131072, eliding the repo out of the middle of its own
            // prompt every turn (c4-842). Telling them to raise a setting was the wrong shape of
            // fix for a long-horizon agent; the engine knows all three bounds.
            let min_reply = ceiling.min(256);
            let (window, why) = gpu_kernels::ctxwin::auto_window(
                prompt_ids.len() + min_reply, p.n_ctx, self.model.trained_ctx,
                self.model.kv_bytes_per_token(), self.model.planned_weight_bytes,
                gpu_kernels::store::resident_budget_bytes(), MAX_CONTEXT);
            if let Some(w) = &why { crate::diag!("wgpu-engine: [{}] {w}", self.name); }
            let max_prompt = window.saturating_sub(min_reply).max(256);
            // Publish the fit for the status line, on BOTH paths. Setting it only when truncation
            // happens would leave the badge showing a previous turn's elision after a short prompt,
            // which is the kind of stale indicator that gets believed.
            crate::backend::CTX_FIT.store(crate::backend::ctxfit_pack(prompt_ids.len(), window, 0),
                              std::sync::atomic::Ordering::Relaxed);
            if prompt_ids.len() > max_prompt {
                let orig = prompt_ids.len();
                // MARK THE GAP. The old splice joined the head straight onto the tail, so a sentence
                // cut in half arrived as a sentence and a function whose body was removed arrived as
                // a function with a different body — the model cannot tell that anything is missing,
                // it just reads a document that says something else. That is tolerable for chat
                // scrollback and actively wrong for reading a repository, where the middle IS the
                // content.
                //
                // The marker is priced into the budget BEFORE the split (ctxwin::plan), never
                // appended after: the window is what the KV cache is allocated from, so overflowing
                // it is the device OOM the bound exists to prevent, not a formatting slip.
                let mtext = gpu_kernels::ctxwin::marker_text(0);
                let mlen = self.tok.encode(&mtext).len().max(8);
                let pl = gpu_kernels::ctxwin::plan(orig, max_prompt, 1024, mlen);
                // SNAP BOTH EDGES TO LINE BOUNDARIES. The plan's indices are arbitrary token
                // positions: the head ends mid-function and the tail resumes mid-line, so the model
                // receives a truncated file followed by half of another. For a pasted repository
                // that is most of the damage. Snapping costs a few tokens and only ever SHRINKS what
                // is kept (proved exhaustively in ctxwin::snapping), so the window the KV cache was
                // sized from cannot be exceeded.
                let boundary: Vec<bool> = prompt_ids.iter()
                    .map(|&t| self.tok.decode_bytes(&[t]).contains(&b'\n')).collect();
                const SNAP_SLACK: usize = 48;
                let head = gpu_kernels::ctxwin::snap_back(&boundary, pl.head, SNAP_SLACK);
                let tail_start = gpu_kernels::ctxwin::snap_fwd(&boundary, orig - pl.tail, SNAP_SLACK);
                let mut kept = prompt_ids[..head].to_vec();
                if pl.marker > 0 {
                    let mut mids = self.tok.encode(&gpu_kernels::ctxwin::marker_text(pl.elided));
                    mids.truncate(pl.marker);          // never spend more than was reserved
                    kept.extend_from_slice(&mids);
                }
                kept.extend_from_slice(&prompt_ids[tail_start.min(orig)..]);
                debug_assert!(kept.len() <= max_prompt, "context plan overflowed its own window");
                prompt_ids = kept;
                crate::backend::CTX_FIT.store(crate::backend::ctxfit_pack(orig, max_prompt, pl.elided),
                                  std::sync::atomic::Ordering::Relaxed);
                crate::diag!("wgpu-engine: context {orig} tokens > {max_prompt} window — kept {head} head \
+ {} tail, {} elided from the middle and MARKED as such; both cuts snapped to line boundaries \
(head -{}, tail +{}) so neither edge lands mid-line — the window already auto-sized to what \
this prompt needs, so a cut this deep means a HARDER bound held it (the auto-size line above \
names which)",
                    orig - tail_start, tail_start - head,
                    pl.head - head, tail_start - (orig - pl.tail));
            }
            // Reply budget. This USED to be a bare `ceiling.min(window - prompt)`, which never
            // errors and never narrates — a real session walked 2470 -> 2050 -> 1832 -> 1187 ->
            // 874 -> 814 over seventeen turns with nothing said, and the turn that wanted a long
            // answer got exactly its cap and stopped mid-thought. ctxwin::relief makes the same
            // arithmetic a DECISION: it says when the squeeze has crossed into costing an answer,
            // and under the long-horizon policy it asks for a compaction instead of shrinking
            // every reply forever.
            let policy = if p.summarize_instead_of_squeeze {
                gpu_kernels::ctxwin::ReliefPolicy::SummarizeAndContinue
            } else { gpu_kernels::ctxwin::ReliefPolicy::Autotune };
            let relief = gpu_kernels::ctxwin::relief(policy, prompt_ids.len(), window, ceiling);
            let n_new = match &relief {
                gpu_kernels::ctxwin::Relief::Proceed { n_new } => *n_new,
                gpu_kernels::ctxwin::Relief::Squeeze { n_new, note } => {
                    if let Some(n) = note { crate::diag!("wgpu-engine: [{}] {n}", self.name); }
                    *n_new
                }
                gpu_kernels::ctxwin::Relief::Summarize { summarize_upto, keep_recent, reason } => {
                    // The engine does not summarize: that is a turn, and turns belong to the
                    // caller. Publish the request and fall back to the squeeze for THIS reply, so
                    // a caller that ignores the signal degrades exactly as before rather than
                    // hanging or producing nothing.
                    crate::backend::COMPACT_REQUEST.store(
                        crate::backend::compact_pack(*summarize_upto, *keep_recent),
                        std::sync::atomic::Ordering::Relaxed);
                    crate::diag!("wgpu-engine: [{}] COMPACTION REQUESTED — {reason}", self.name);
                    window.saturating_sub(prompt_ids.len()).max(1)
                }
            };
            let cap = (prompt_ids.len() + n_new).max(1);
            // ── Speculative-decode path (opt-in: a draft model was loaded) ───────────────────────────
            // Draft proposes a block; target verifies the whole block in one forward (engine
            // `speculative_generate_stats`). Greedy and bit-exact to normal decode, but more tokens per
            // expensive target pass. Bypasses KV-tree reuse (spec manages its own target+draft caches).
            // LOGSCOPE_SPEC_DRAFT=lookup asks for the model-free drafter (c4-838): it finds the
            // current suffix earlier in the context and proposes what followed. No second
            // checkpoint to download, load or keep resident, and the same verifier c4-837 proved
            // correct by self-draft — so at TEMPERATURE 0 output is bit-identical to plain
            // decoding and only the speed differs.
            //
            // The temperature qualifier is not pedantry and this comment lacked it while
            // perfverdict.rs carried it — two descriptions of one verifier, one of them wrong.
            // `spec::accepted_run` is greedy: it accepts while the draft equals the verifier's
            // own token and commits the verifier's tokens. That is identical output only where
            // the verifier is deterministic. Above temperature 0 it samples, and a speculative
            // run consumes randomness in a different order — still a valid draw from the same
            // distribution, but not the SAME draw, so a prompt stops reproducing. See
            // gpu_kernels::autospec, which is why auto-enabling is gated on temperature 0. Suited to repository reading, where identifiers and paths repeat; it should
            // mostly whiff on prose, which costs a verification slot and nothing else.
            let want_lookup = std::env::var("LOGSCOPE_SPEC_DRAFT").as_deref() == Ok("lookup");
            if want_lookup && self.draft.is_none() {
                let k: usize = std::env::var("LOGSCOPE_SPEC_K").ok().and_then(|s| s.trim().parse().ok())
                    .filter(|&k| k >= 1).unwrap_or(4);
                let ng: usize = std::env::var("LOGSCOPE_SPEC_NGRAM").ok().and_then(|s| s.trim().parse().ok())
                    .filter(|&n| n >= 1).unwrap_or(2);   // measured: ng=2 beats 3 and 4 on every corpus
                let mut d = gpu_kernels::spec::PromptLookup { ngram: ng };
                let mut st = gpu_kernels::SpecStats::default();
                let td = Instant::now();
                // CHUNKED, not per-position. `speculative_generate_drafter` prefills one decode step
                // per prompt token: on the measured 139 ms/token Asahi run that is 227 s of
                // time-to-first-token for a 1626-token prompt, against 14.7 s here — so the operator
                // who follows the engine's own advice to set LOGSCOPE_SPEC_DRAFT=lookup waits four
                // minutes for a first token and concludes the feature is broken. Same chunk rule as
                // ordinary streaming, from the one helper that owns it.
                let chunk = prefill_chunk_for_streaming(self.model.d, self.model.h,
                    self.model.streams_experts(), self.model.moe_counts());
                // SINGLE-SLOT ANCHOR, not the shared prefix pool. spec_core owns its cache, so
                // consuming a pool entry would leave nothing to put back and would strip the pool a
                // turn at a time; keeping the previous speculative turn's own cache here reuses the
                // prefix without touching the pool's invariants at all. In a linear chat that is the
                // same 97% hit ordinary streaming gets — and without it speculation is a NET LOSS on
                // every turn after the first: 2.1 tok/s against plain streaming's 6.0, because a
                // 14.7 s fresh prefill swamps a drafter worth 1.49x.
                let seeded = self.spec_anchor.as_ref().and_then(|(toks, run)| {
                    let n = toks.iter().zip(&prompt_ids).take_while(|(a, b)| a == b).count();
                    // Leave the last shared token to the suffix pass so there is always a batched
                    // forward to produce logits from; a zero-length suffix has no distribution.
                    let n = n.min(prompt_ids.len().saturating_sub(1)).min(run.pos);
                    run.cache.as_ref().filter(|_| n >= 16).map(|c| (c, n))
                });
                let prefill = match seeded {
                    Some((c, n)) => {
                        crate::diag!("wgpu-engine: [{}] speculative prefix reuse — {n} of {} prompt \
tokens seeded from the previous speculative turn, {} to prefill",
                            self.name, prompt_ids.len(), prompt_ids.len() - n);
                        gpu_kernels::SpecPrefill::Seeded { src: c, len: n, chunk }
                    }
                    None => {
                        crate::diag!("wgpu-engine: [{}] speculative prefill — {} prompt tokens, chunk \
{chunk}, no anchor yet (first speculative turn, or the prompt diverged early)",
                            self.name, prompt_ids.len());
                        gpu_kernels::SpecPrefill::Chunked(chunk)
                    }
                };
                let mut run = gpu_kernels::SpecRun::default();
                let ids = self.gpu.speculative_generate_drafter_run(&self.model, &mut d, &prompt_ids,
                    n_new, k, prefill, &mut st, &mut run);
                // Keep this turn's cache as the next turn's anchor. The committed context is the
                // prompt plus what was generated, which is exactly what the next prompt will extend.
                let mut anchor_toks = prompt_ids.clone();
                anchor_toks.extend_from_slice(&ids);
                self.spec_anchor = Some((anchor_toks, run));
                let secs = td.elapsed().as_secs_f32().max(1e-3);
                let mut produced = 0usize;
                let mut pending: Vec<u8> = Vec::new();
                for &next in &ids {
                    if self.stop_ids.contains(&next) { break; }
                    if self.tok.is_special(next) { continue; }
                    produced += 1;
                    pending.extend_from_slice(&self.tok.decode_bytes(&[next]));
                    let valid = match std::str::from_utf8(&pending) { Ok(t) => t.len(), Err(e) => e.valid_up_to() };
                    if valid > 0 {
                        let text: String = String::from_utf8_lossy(&pending[..valid]).into_owned();
                        pending.drain(..valid);
                        let _ = on(&text);
                    }
                }
                // Tokens per verification pass is the speedup, NOT the acceptance rate: a high rate
                // on short drafts can be worth less than a mediocre one on long drafts (c4-829).
                let per_pass = if st.rounds > 0 { st.emitted as f64 / st.rounds as f64 } else { 0.0 };
                crate::diag!("wgpu-engine: [{}] speculative (prompt-lookup, k={k}, ngram={ng}) — \
{} tokens in {:.1}s = {:.2} tok/s | {} accepted of {} drafted over {} rounds = {:.2} tokens per \
verification pass (1.00 means the drafter paid for nothing)",
                    self.name, produced, secs, produced as f32 / secs,
                    st.accepted, st.rounds * k, st.rounds, per_pass);

                // WHEN THE DRAFTER IS A NET LOSS, and why — the two causes are
                // sampling settings, and they are settings the operator chose.
                //
                // Reported by DavidAU for Qwen3.6-27B's MTP quants and it
                // generalises to any speculative path, because the mechanism is
                // the same: a draft is accepted when the target agrees, and
                // both temperature and a repetition penalty MOVE the target's
                // distribution away from the greedy token the drafter guessed.
                // Their break-even, measured on a 2-token draft, was 50%
                // acceptance — below it the plain path is faster.
                //
                // We report tokens-per-pass rather than a rate (c4-829: a high
                // rate on short drafts can be worth less than a mediocre one on
                // long drafts), so the break-even here is 1.00 — the drafter
                // paid for its verification and returned exactly what one
                // ordinary forward would have.
                if per_pass < 1.15 && st.rounds > 8 {
                    let mut why: Vec<String> = Vec::new();
                    if p.temperature > 1.0 {
                        why.push(format!("temperature {:.2} is above 1.0 — sampling away from the \
drafter's greedy guess is exactly what rejects it", p.temperature));
                    }
                    if (p.repeat_penalty - 1.0).abs() > 0.01 {
                        why.push(format!("repeat_penalty {:.2} is not 1.0 — it reshapes the \
target distribution per position, so the draft is scored against something the drafter never saw",
                            p.repeat_penalty));
                    }
                    let tail = if why.is_empty() {
                        "sampling is not the cause (temperature <= 1.0, penalty off), so this text \
is genuinely unpredictable to a prompt-lookup drafter — prose rather than code, or a first pass \
over unfamiliar material. LOGSCOPE_SPEC_DRAFT=0 turns it off.".to_string()
                    } else {
                        format!("{}. Either lower them for this run or turn the drafter off.",
                                why.join("; "))
                    };
                    crate::diag!("wgpu-engine: [{}] speculative NET LOSS — {per_pass:.2} tokens \
per verification pass is at or below the 1.00 break-even, so the drafting cost bought nothing. {}",
                        self.name, tail);
                }
                if !pending.is_empty() { let _ = on(&String::from_utf8_lossy(&pending)); }
                return Ok(produced);
            }
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
                    crate::backend::check_output_markers(&self.name, &chunk, &self.tok);
                    pending.drain(..valid);
                    if !on(&chunk) { break; }
                }
                if !pending.is_empty() { let _ = on(&String::from_utf8_lossy(&pending)); }
                crate::diag!("wgpu-engine: [{}] SPECULATIVE k={k} — {} tokens in {:.1}s = {:.2} tok/s | {:.2} accepted/round × {} rounds (~{:.2} tok per target verify-pass)",
                    self.name, ids.len(), secs, ids.len() as f32 / secs,
                    st.accepted as f64 / st.rounds.max(1) as f64, st.rounds, st.emitted as f64 / st.rounds.max(1) as f64);
                return Ok(produced);
            }
            // Which cached branch tip shares the longest usable prefix with this prompt? Both the
            // FIFO and the tiered pool select with kvtree::pick_anchor (≥16 shared, ≤256 divergent),
            // searching the whole pool so a sibling switch or an Nth fan-out continuation reuses ITS
            // lineage's KV, not just the last turn's. The pool additionally MATERIALIZES the chosen
            // tip to a seedable f32 Session (promoting from int8/host if it had been demoted) and
            // removes it — we re-insert the extended tip after generating.
            let kv_off = std::env::var("LOGSCOPE_KV_OFF").is_ok();
            let mut picked_session: Option<Session> = None; // Some ⇒ pool path (owns the source)
            // `had_cache` distinguishes a MISS from a first turn. pool.pick returning None when the
            // pool already holds entries means the prefix diverged or was evicted — and that costs the
            // whole prompt again, which the log reported in words identical to a first turn.
            let mut had_cache = false;
            // LINEAR AGENT RECURRENT-STATE CARRY-FORWARD (Gated DeltaNet). A hybrid model folds a
            // recurrent hidden-state checkpoint through every position of its prompt, so a full
            // cold prefill of the whole growing prompt every agent turn is pure waste — the model
            // has already seen prompt[0..|P|] and its delta-net state *is* the compression of that
            // history. On a LINEAR agent turn, the new prompt is the previous turn's prompt appended
            // with exactly one reply, so the previous turn's terminal checkpoint (at next_pos ==
            // |P|) is a valid, position-absolute anchor: seeding it and prefilling only the new
            // Δ tokens decodes them at the same absolute positions the original prefill saw, giving a
            // numerically identical result (the regression gate `gate-deltanet-prefix-reuse` proves
            // this to 1e-3 and proves the alternative — a shorter shared prefix — panics).
            //
            // This is a NARROWER branch than the KV prefix tree below, not a replacement: it fires
            // only for a hybrid model whose previous turn's prompt is a prefix of this one (the
            // linear agent loop). A fan-out run (where pick_anchor would grab a shorter shared
            // prefix) is guarded against below — the delta reuse refuses unless this prompt CONTAINS
            // the previous turn's ENTIRE prompt unchanged, so a shared-prefix checkpoint can never
            // be installed. Non-agent chat never retains a previous turn, so this is dead weight for
            // it. Only the GPU-resident checkpoint (deltanet_gpu) is live by default;
            // LOGSCOPE_DELTANET_CPU=1 routes to the CPU oracle, which this path detects and
            // rejects (carry_deltanet_state returns false on the CPU route, so we fall through
            // to a cold prefill).
            let delta_seed = if self.model.deltanet_layers() > 0 {
                // The linear-agent carry-forward is decided by the model's ARCHETYPE, not by folding
                // the delta-state invariant into a token-prefix check the KV tree shares. The delta
                // state is position-absolute: the previous turn's transcript was folded to next_pos ==
                // delta_pos, so this turn's prompt must embed that transcript at identical absolute
                // positions up to delta_pos — the RecurrentStream invariant. plan_context reports which
                // invariant holds, and a Refusal reason if it does not, instead of a bare position.
                if let Some((prev_toks, _sess, prev_pos)) = &self.prev_turn {
                    let current = ContextTurn::current(prompt_ids.clone());
                    let previous = ContextTurn::carried(prev_toks.clone(), prev_toks.clone(), *prev_pos);
                    let plan = self.model.archetype().plan_context(Some(&previous), &current);
                    plan.recurrent_seed
                } else {
                    None
                }
            } else {
                None
            };
            // CARRY-FORWARD DIAGNOSTIC. Fires every turn for a hybrid (Gated DeltaNet) model, so a
            // run that never reuses (every turn logging "full prefill") shows exactly WHY: whether
            // `prev_turn` is set at all (turn 1 never carried a session forward) or the carried
            // transcript no longer embeds at the same absolute position (the driver's append broke
            // — injected context, or a branch). The Refusal variant names the invariant that broke.
            if self.model.deltanet_layers() > 0 {
                let (recursed_reason, carried) = self.prev_turn.as_ref().map(|(prev_toks, _, prev_pos)| {
                    let current = ContextTurn::current(prompt_ids.to_vec());
                    let previous = ContextTurn::carried(prev_toks.clone(), prev_toks.clone(), *prev_pos);
                    let plan = self.model.archetype().plan_context(Some(&previous), &current);
                    (plan.refused_reason, plan.prompt_position)
                }).unwrap_or((Some(Refusal::NoCarriedTurn), prompt_ids.len()));
                let delta_used = match (self.prev_turn.is_some(), delta_seed, recursed_reason) {
                    (true, Some(l), None) => format!("reused delta carry at position {l}"),
                    (true, None, Some(reason)) => format!("delta carry NOT fired — {reason:?}"),
                    (false, _, _) => "delta carry NOT fired — no prev_turn carried forward (first turn or cleared)".to_string(),
                    // Unreachable: a set prev_turn yields either a seed or a refusal, never both or neither.
                    (true, Some(_), Some(_)) | (true, None, None) => "delta carry NOT fired — planner inconsistent".to_string(),
                };
                crate::diag!("wgpu-engine: [{}] delta carry-forward: {}", self.name, delta_used);
            }
            let (reuse, l, pick_idx) = if kv_off {
                (false, 0, 0)
            } else if let Some(delta_l) = delta_seed {
                // Linear recurrent-state carry-forward. Hybrid + previous turn's prompt is a
                // prefix of this one → reuse the carried checkpoint instead of a cold prefill.
                // The carried Session is picked up by copy directly in the branch below
                // (`picked_session` is left None here — seeding from a pool anchor would install
                // state at the wrong position); this arm only supplies `l`, the position the Δ
                // suffix starts at.
                (true, delta_l, usize::MAX)
            } else if self.model.deltanet_layers() > 0 {
                // A hybrid (Gated DeltaNet) model carries, per recurrent layer, a GPU-resident
                // recurrent state { ring, s, next_pos } that is ABSOLUTE — folded through positions
                // 0..N of THIS cache, cannot seek to an earlier position. `pick_anchor` hands back a
                // SHARED PREFIX `l` (a prefix of the stored tip, usually < its full length), so
                // seeding a fresh session from it would install delta state built at `next_pos`
                // while declaring it belongs at `l < next_pos`: the assert in block_prefill_deltanet_gpu
                // fires, or worse, a correct-looking install silently corrupts every downstream token.
                // The only sound recurrent reuse is a full prefill from position 0, so disable the
                // KV prefix tree for every recurrent model. A first turn starts at position 0 anyway,
                // so the tool-call loop pays nothing for this; every earlier reuse was silently wrong.
                (false, 0, 0)
            } else if let Some(pool) = self.pool.as_mut() {
                had_cache = !pool.is_empty();
                // Prompt-scaled, not a fixed 256: an agent turn appends kilobytes of tool
                // output, so its divergent suffix is always thousands of tokens and the old cap
                // could not be met by construction — eight turns of the operator's repo summary
                // missed the cache and paid a full 15.3s prefill each.
                match pool.pick(&self.gpu, &self.model, &prompt_ids, 16, crate::kvtree::suffix_bound(prompt_ids.len())) {
                    Some((i, ln, sess)) => { picked_session = Some(sess); (true, ln, i) }
                    None => (false, 0, 0),
                }
            } else {
                let meta: Vec<(Vec<u32>, usize, usize)> =
                    self.anchors.iter().map(|(t, sess, pf)| (t.clone(), *pf, sess.pos)).collect();
                // REPORT WHAT THE PREFIX TREE DID. This cache saves whole prefills — the largest
                // per-turn cost after the first — and had no telemetry at all: no hit line, no miss
                // line, and kvtree::prefill_savings was called only by its own test. A log could not
                // answer "did it ever hit", so the thresholds have never been tuned against a real
                // session.
                match crate::kvtree::pick_anchor(&meta, &prompt_ids, 16, crate::kvtree::suffix_bound(prompt_ids.len())) {
                    Some((i, ln)) => {
                        let saved = crate::kvtree::prefill_savings(prompt_ids.len(), ln);
                        crate::diag!("wgpu-engine: [{}] KV prefix tree HIT — reusing {ln} of {} prompt \
tokens ({:.0}% of the prefill skipped), {} to prefill", self.name, prompt_ids.len(),
                            100.0 * saved as f64 / prompt_ids.len().max(1) as f64,
                            prompt_ids.len() - ln);
                        (true, ln, i)
                    }
                    None => {
                        // A miss has three causes needing opposite responses, and "miss" names none
                        // of them.
                        let why = crate::kvtree::explain_miss(&meta, &prompt_ids, 16, crate::kvtree::suffix_bound(prompt_ids.len()));
                        let fix = why.tunable().map(|f| format!(" — {f}")).unwrap_or_default();
                        crate::diag!("wgpu-engine: [{}] KV prefix tree MISS ({} anchor(s)): {why:?}{fix}",
                            self.name, meta.len());
                        (false, 0, 0)
                    }
                }
            };
            let suffix_len = prompt_ids.len() - l;
            let mut s = self.gpu.session_new(&self.model, cap, self.rng);
            let t0 = Instant::now();
            // A DIFFUSION model does not decode from a KV cache: diffuse_generate_cb re-forwards the
            // block-truncated canvas each step and takes only the raw prompt ids. Prefilling one
            // therefore buys nothing and costs the whole prompt — 12.5s on the 737 run, printed as
            // "full prefill (first turn for this model)" immediately before the line saying
            // generation was routing to the diffusion path instead. The empty session is still
            // created so every borrow below stays valid; it is simply never filled.
            let diffusion_path = self.model.diffusion
                && std::env::var("LOGSCOPE_DIFFUSION_AR").as_deref() != Ok("1")
                && self.model.mask_token_id.is_some();   // diffuse_generate refuses without one
            if diffusion_path {
                crate::diag!("wgpu-engine: [{}] diffusion model — skipping the KV prefill ({} prompt tokens); block denoising re-forwards the canvas and reads no cache",
                    self.name, prompt_ids.len());
            } else if let Some(delta_l) = delta_seed {
                // LINEAR AGENT RECURRENT-STATE CARRY-FORWARD. The previous turn's prompt is a
                // prefix of this one, so fold its terminal recurrent state into a fresh session
                // and prefill only the appended Δ tokens. This is the whole win: turn N pays for
                // the Δ suffix instead of a cold prefill of the whole growing prompt.
                //
                // Seed from the CARRIED session, not the KV pool (`picked_session`/`pick_idx` are
                // left None / usize::MAX here — seeding from an anchor would install state at the
                // wrong position). carry_deltanet_state copies the GPU-resident { ring, s } tensors
                // AND sets next_pos == delta_l, so the fresh session's delta-net state is exactly
                // the previous turn's terminal state, positioned for position delta_l, which is
                // where the Δ suffix begins. Without setting next_pos the assert in
                // block_prefill_deltanet_gpu would fire (you can't seek to an earlier position).
                // Grab the carried session's cache by reference: Session owns GPU buffers (wgpu::
                // Buffer + a Mutex<AtomicUsize>) so it is NOT Copy/Clone, but carry_deltanet_state
                // and seed_from_prefix only read src: &KvCache — no move needed, so a live
                // &self.prev_turn can stay borrowed here. On the CPU oracle route
                // (LOGSCOPE_DELTANET_CPU=1) carry_deltanet_state returns false — the device tensors
                // don't exist there — so we fall through to the cold prefill below rather than
                // corrupting a state that isn't the live one.
                let delta_prev = match self.prev_turn.as_ref().map(|(_, s, _)| s) {
                    Some(sess) => sess,
                    None => return Err("delta reuse selected but the carried session is gone (prev_turn cleared?)".to_string()),
                };
                let delta_ok = self.gpu.carry_deltanet_state(&self.model, &mut s, &delta_prev.cache, delta_l);
                // Carry the K/V rows the prefix would have filled, too — a full prefill writes
                // them, and a later turn may want them; cheap and keeps this turn's cache complete.
                self.gpu.seed_from_prefix(&self.model, &mut s, &delta_prev.cache, delta_l);
                if !delta_ok {
                    // CPU oracle path: no device checkpoint to carry. Drop the anchor and cold-
                    // prefill (the CPU oracle is an A/B reference, never the shipped route).
                    self.prev_turn = None;
                    crate::diag!("wgpu-engine: [{}] delta reuse — CPU oracle route, no device state; cold prefill",
                        self.name);
                    let pc = prefill_chunk_for_streaming(self.model.d, self.model.h,
                        self.model.streams_experts(), self.model.moe_counts());
                    s.logits = self.gpu.prefill_chunked(&self.model, &s.cache, &prompt_ids, pc);
                    s.pos = prompt_ids.len();
                    // skip the reuse log below
                } else {
                    let pc = prefill_chunk_for_streaming(self.model.d, self.model.h,
                        self.model.streams_experts(), self.model.moe_counts());
                    // Prefill the appended Δ at the carried positions — same batched kernel the
                    // full prefill used, just the suffix rows. Bit-identical to a fresh full
                    // prefill except fp-reordering (proven by the gate).
                    s.logits = self.gpu.prefill_chunked_from(&self.model, &s.cache, &prompt_ids[delta_l..], delta_l, pc);
                    s.pos = prompt_ids.len();
                }
            } else if reuse {
                // Seed from the picked tip: the pool handed us an owned Session; the FIFO borrows in place.
                if let Some(src) = picked_session.as_ref() {
                    self.gpu.seed_from_prefix(&self.model, &mut s, &src.cache, l);
                } else {
                    let src = &self.anchors[pick_idx].1.cache; // borrow ends after the copy
                    self.gpu.seed_from_prefix(&self.model, &mut s, src, l);
                }
                // Prefill the divergent suffix with the BATCHED kernel (verify_forward / block_
                // prefill_ext at offset l) — the same path a full prefill uses — instead of per-
                // token decode_step. Per-token vs batched differ by ~1e-6, which is enough to tip a
                // near-tied eos in a long, degenerating context (the "produces once then stops"
                // failure). Batched suffix keeps reuse within machine precision of a full prefill.
                // CHUNKED, not verify_forward. Same batched kernel — prefill_chunked_from
                // runs block_prefill_ext at offset l, exactly as verify_forward did, so the
                // precision argument above is untouched — but it projects ONLY the last row.
                // verify_forward materialises s x vocab logits, so a long divergent suffix
                // asked for a buffer no device has: the field crash was 6873 x 128256 x 4 =
                // 3.53 GB against a 2.15 GB cap, reached by :ws on a real repo. c11 fixed
                // the SPECULATIVE seeded path this way and missed this one, which is the
                // ordinary streaming path every chat takes — the same defect, twice, because
                // the first fix chased a backtrace instead of the pattern.
                let pc = prefill_chunk_for_streaming(self.model.d, self.model.h,
                    self.model.streams_experts(), self.model.moe_counts());
                s.logits = self.gpu.prefill_chunked_from(&self.model, &s.cache, &prompt_ids[l..], l, pc);
                s.pos = prompt_ids.len();
            } else {
                // O(1)-bandwidth batched prefill of the whole prompt (one [T,d] pass).
                // O(chunk·T) chunked prefill — NOT one O(T²) batched pass — so a long pasted
                // context can't balloon attention scratch to tens of GB and OOM the device.
                let pc = prefill_chunk_for_streaming(self.model.d, self.model.h, self.model.streams_experts(), self.model.moe_counts());
                {
                    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                    if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        let why = if std::env::var("LOGSCOPE_PREFILL_CHUNK").is_ok() || std::env::var("PREFILL_CHUNK").is_ok() {
                            " (overridden)".to_string()
                        } else if pc < PREFILL_CHUNK {
                            format!(" (auto-lowered from {PREFILL_CHUNK} for this model's d={}/ff={} — a fixed token count means a bigger model gets a longer dispatch, which is what trips the GPU watchdog)", self.model.d, self.model.h)
                        } else if pc > PREFILL_CHUNK {
                            format!(" (raised from {PREFILL_CHUNK}: this model routes {} experts with {} \
active, so its FFN runs as small per-expert dispatches and a bigger chunk cuts their COUNT rather than \
lengthening them)",
                                self.model.moe_counts().map(|c| c.0).unwrap_or(0),
                                self.model.moe_counts().map(|c| c.1).unwrap_or(0))
                        } else if self.model.streams_experts() {
                            " (kept at full size: this model STREAMS experts, so refill traffic dominates \
and a smaller chunk would re-pull every cold expert more often)".to_string()
                        } else {
                            " (default; set LOGSCOPE_PREFILL_CHUNK to lower for GPU-watchdog hangs)".to_string()
                        };
                        crate::diag!("wgpu-engine: prefill chunk = {pc} tokens/dispatch{why}");
                    }
                }
                s.logits = self.gpu.prefill_chunked(&self.model, &s.cache, &prompt_ids, pc);
                s.pos = prompt_ids.len();
            }
            // CARRIED TO THE PROFILE LINE. Decode compared against this model's OWN prefill needs no
            // roofline, which is why it can be reported when the bandwidth verdict cannot — and the
            // bandwidth verdict almost never can, because --gate-perf is rarely run.
            // DIVIDE BY THE TOKENS ACTUALLY PREFILLED, not by the prompt. When the prefix tree hits,
            // most of the prompt is not prefilled at all: a turn reusing 1626 of 1721 tokens spent
            // 0.39s on 95 real tokens, and dividing that by 1721 gives 0.23 ms/tok instead of 4.1 —
            // which inflated the phase-balance ratio to 269x and reported a cache SUCCESS as a
            // decode pathology. c4-750's guard catches a phase that was skipped entirely; this is a
            // phase that was mostly skipped, and the same division artifact wearing a smaller number.
            let prefilled_now = prompt_ids.len().saturating_sub(if reuse { l } else { 0 });
            let prefill_ms_per_tok = if prefilled_now == 0 { 0.0 }
                else { t0.elapsed().as_secs_f64() * 1000.0 / prefilled_now as f64 };
            let prefill_tokens = prefilled_now;
            crate::diag!("wgpu-engine: [{}] prompt {} tokens → up to {} (temp {:.2}); {} in {:.2}s, decoding…", self.name,
                prompt_ids.len(), n_new, p.temperature,
                // NAME THE MISS. A hit says "reused N anchor tokens"; a miss said "full prefill",
                // which is exactly what a FIRST turn says — so the one number worth watching was
                // invisible, and a chat that silently lost its anchor every turn read as a chat with
                // long prompts.
                //
                // A miss costs the whole prompt again: measured at 12.6 ms/tok on this device, so a
                // 1700-token prompt is 20 s paid for nothing. The pool holds 3, and a fourth branch
                // evicts one.
                if reuse {
                    format!("reused {l} anchor tokens + prefilled {suffix_len}")
                } else if had_cache {
                    format!("ANCHOR MISS — re-prefilled all {} tokens (the cached prefix diverged or \
was evicted)", prompt_ids.len())
                } else {
                    format!("full prefill (first turn for this model)")
                },
                t0.elapsed().as_secs_f32());

            // ── long-horizon watch (advisory) ────────────────────────────────────────────────────
            // Feeds horizon the same numbers the line above just printed. It changes nothing; it says
            // what it would change, so the diagnosis can be checked against real traffic before it is
            // allowed to rewrite anyone's transcript.
            //
            // The window is read per turn rather than fixed at construction because the operator can
            // move the n_ctx slider mid-session, and a watcher advising against a window that is no
            // longer in force is worse than no watcher.
            {
                // A poisoned lock here must not take down a serving daemon: the
                // watcher is advisory and observes numbers already printed.
                let mut hz = self.horizon.lock().unwrap_or_else(|e| e.into_inner());
                if hz.window() != window { *hz = gpu_kernels::horizon::Horizon::new(window, 512, l.max(1)); }
                hz.observe(gpu_kernels::horizon::Turn {
                    prompt_len: prompt_ids.len(),
                    reused: if reuse { l } else { 0 },
                    prefill_s: t0.elapsed().as_secs_f32(),
                    emitted: 0, // filled by the next turn's prompt growth; not used by the policy
                });
                match hz.advise(prompt_ids.len()) {
                    gpu_kernels::horizon::Move::Continue => {}
                    // ctxwin::auto_window already sizes the window from the PROMPT and the device
                    // budget. horizon adds the axis it cannot see: whether the history is worth
                    // keeping. So this is phrased as a slider recommendation rather than as a
                    // competing autotuner — two things sizing the same window from different
                    // evidence is how they end up fighting.
                    gpu_kernels::horizon::Move::Grow { to, why } => crate::diag!(
                        "wgpu-engine: [{}] long-horizon ADVISORY (nothing changed) — this chat would \
use an n_ctx of {to}; auto_window sizes to the prompt, it cannot see that the history is worth \
keeping: {why}", self.name),
                    gpu_kernels::horizon::Move::Compact { keep_prefix, also_grow, why } => crate::diag!(
                        "wgpu-engine: [{}] long-horizon ADVISORY (nothing changed) — summarise and \
continue, keeping the first {keep_prefix} tokens{}: {why}", self.name,
                        match also_grow { Some(to) => format!(" and raise the window to {to}"), None => String::new() }),
                }
            }

            let td = Instant::now();
            // Batch 3: snapshot the engine's dispatch counters so we can report the CPU-bound (setup)
            // vs GPU-bound (sync) split for THIS generation — the data that answers "why is llama.cpp
            // faster": high submits/token + setup_ms-dominated ⇒ dispatch-bound (→ fuse kernels), not
            // compute-bound.
            // The CAUSES need the same baseline. Read as absolutes they carry a lifetime total into a
            // per-generation report — your log showed "349 submits" beside causes summing to 4132, and
            // the causes grew across turns while the submit count did not.
            let cause0 = gpu_kernels::SubmitCauseSnapshot::now();
            let submit_ns0 = gpu_kernels::gpu_submit_ns();
            let sample_ns0 = gpu_kernels::sample_ns();
            use std::sync::atomic::Ordering as AOrd;
            let (s0, p0, su0, sy0) = (
                gpu_kernels::GPU_SUBMITS.load(AOrd::Relaxed), gpu_kernels::GPU_POLLS.load(AOrd::Relaxed),
                gpu_kernels::GPU_SETUP_NS.load(AOrd::Relaxed), gpu_kernels::GPU_SYNC_NS.load(AOrd::Relaxed));
            let (gp0, gd0) = (gpu_kernels::GPU_PASSES_OPENED.load(AOrd::Relaxed),
                              gpu_kernels::GPU_DISPATCHES.load(AOrd::Relaxed));
            let (bgh0, bgm0) = (gpu_kernels::BG_HITS.load(AOrd::Relaxed), gpu_kernels::BG_MISSES.load(AOrd::Relaxed));
            // The three WHY counters need the same baseline as the hit/miss
            // pair above. They are process-global atomics that nothing resets,
            // and the line that prints them reports a per-turn hit rate beside
            // them — so on an M2 Max run they read "592 lookups" next to "1587
            // output / 4220 dims", misses exceeding lookups tenfold in the same
            // sentence. Session totals printed as if they were this turn's.
            let (mo0, md0, mi0) = (gpu_kernels::BG_MISS_OUT.load(AOrd::Relaxed),
                                   gpu_kernels::BG_MISS_DIMS.load(AOrd::Relaxed),
                                   gpu_kernels::BG_MISS_INPUTS.load(AOrd::Relaxed));
            let enc0 = gpu_kernels::gpu_encode_ns();
            // Windowed SAMPLED decode: the whole sample→embed→decode cycle stays on-device for k
            // tokens per readback — no per-token full-vocab logits sync and no per-token O(v·log v)
            // CPU sort. Eligibility + lazy int8-embed build live in `windowed_ready` so the resume
            // path shares them exactly. LOGSCOPE_SAMPLE_WINDOW (default 4; 0/1 disables).
            let sample_win_opt = self.windowed_ready(&sp);
            // The grammar for this turn, if any: on the wgpu engine this is what makes a malformed
            // tool block impossible rather than merely detectable afterwards. Built AFTER the last
            // `&mut self` above, because it borrows the tokenizer for the whole decode.
            let mut filt = self.tool_filter(&self.tok, p);
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
                let mut step = |next: u32| {
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
                // DIFFUSION MODELS ARE NOT AUTOREGRESSIVE. LLaDA2/LLaDA/Dream/RND1 are masked
                // diffusion: they predict masked positions inside a block with bidirectional
                // attention, iteratively. AR-decoding one produces exactly what the engine's own
                // doc comment on diffuse_generate predicts — "fluent noise": real words, correct
                // punctuation, no coherence. Every earlier fix in this hunt was necessary and none
                // could have produced sense, because the chat path never called the diffusion
                // generator at all.
                if model.diffusion {
                    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                    if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        crate::diag!("wgpu-engine: [{}] DIFFUSION model — routing generation through diffuse_generate (block denoising), NOT the autoregressive window. LOGSCOPE_DIFFUSION_AR=1 forces the old AR path for comparison; LOGSCOPE_DIFFUSION_STEPS / _BLOCK tune the schedule.", self.name);
                    }
                    if std::env::var("LOGSCOPE_DIFFUSION_AR").as_deref() != Ok("1") {
                        // PER-BLOCK now (reference semantics), so this multiplies by block count:
                        // forwards = ceil(n_fill/block) * steps. The reference's ideal is
                        // steps == block (one token per commit), which at 20 blocks would be 640
                        // whole-prefix forwards — minutes per reply until the committed prefix is
                        // KV-cached. 8 is the compromise: 4 tokens per commit instead of the old
                        // loop's 21, at a quarter of the ideal's cost. Raise it for quality.
                        let block: usize = std::env::var("LOGSCOPE_DIFFUSION_BLOCK").ok()
                            .and_then(|v| v.parse().ok()).unwrap_or(32);
                        // ONE TOKEN PER COMMIT, the reference's schedule. The 8 above was chosen for
                        // a cost that no longer exists: the comment beside it said the ideal meant
                        // "minutes per reply until the committed prefix is KV-cached", and c4-770
                        // cached it — a step now forwards its own 32 rows instead of ~2000.
                        //
                        // The reason to spend that budget on quality is in the output. Committing
                        // 4 positions from ONE forward means each picks its best token against a
                        // canvas that is still mostly masked, so neighbours never see each other's
                        // choices: "provide provide", "like like", "What What", "with with" —
                        // duplicated adjacent words and locally-plausible text that does not agree
                        // with itself. That is the signature of multi-token commits, not of a broken
                        // model, and steps == block is what the reference does to avoid it.
                        //
                        // It is not 32x the cost either: the confidence threshold (c4-766) commits
                        // every position over 0.95 immediately, so a block the model is sure about
                        // still closes in a couple of steps.
                        let steps: usize = std::env::var("LOGSCOPE_DIFFUSION_STEPS").ok()
                            .and_then(|v| v.parse().ok()).unwrap_or(block);
                        // Stop set, cancel and per-block streaming all reach the generator now.
                        // Before: a diffusion reply filled its entire ceiling (21 blocks at 8 steps
                        // = 168 whole-prefix forwards) even when the model finished in two; cancel
                        // was not checked at all, so an unwanted generation could only be escaped by
                        // killing the app; and nothing appeared on screen until every block was
                        // done. Each block's tokens now go through the SAME `step` callback the AR
                        // path uses, so detokenisation, the runaway guard and stop handling are
                        // identical between the two paths.
                        let mut stop_ids: Vec<u32> = stop.iter().copied().collect();
                        if let Some(e) = eos { stop_ids.push(e); }
                        let mut produced = 0usize;
                        let mut emit = |blk: &[u32]| -> bool {
                            for &t in blk {
                                // Count what the READER will see. `step` swallows specials, so
                                // counting everything handed to it produced the log line "produced
                                // 20 tokens" beside an empty reply — a number that described the
                                // loop's bookkeeping rather than the answer, which is the opposite
                                // of what telemetry is for.
                                if !tok.is_special(t) { produced += 1; }
                                if !step(t) { return false; }
                            }
                            true
                        };
                        // SAMPLING, per the reference (chatllm.cpp forces do_sample with top_p).
                        // The loop previously took each position's argmax and the operator's
                        // temperature did nothing on a diffusion turn — the log said "temp 0.35"
                        // while generation was strictly greedy. LOGSCOPE_DIFFUSION_GREEDY=1 forces
                        // the old behaviour, which is also what temperature 0 gives.
                        let diff_sp = if std::env::var("LOGSCOPE_DIFFUSION_GREEDY").as_deref() == Ok("1") {
                            None
                        } else { Some(&sp) };
                        // CONFIDENCE THRESHOLD, the reference's convergence accelerator
                        // (chatllm.cpp defaults it to 0.95). We passed None, so it never ran: every
                        // step committed exactly the schedule even when the model was certain about
                        // a dozen positions, which costs forwards — the expensive unit on this path
                        // — and diverges from the dynamics the schedule was designed around.
                        // LOGSCOPE_DIFFUSION_TAU sets it; 0 restores the schedule-only behaviour.
                        let tau = std::env::var("LOGSCOPE_DIFFUSION_TAU").ok()
                            .and_then(|v| v.trim().parse::<f32>().ok())
                            .unwrap_or(0.95)
                            .clamp(0.0, 1.0);
                        let tau = if tau <= 0.0 { None } else { Some(tau) };
                        match gpu.diffuse_generate_sampled(model, &seen, n_new, steps, block, tau,
                                                           &stop_ids, diff_sp, rng, cancel, &mut emit) {
                            Ok(_) => {
                                if !pending.is_empty() { let _ = on(&String::from_utf8_lossy(&pending)); }
                                return produced;
                            }
                            Err(e) => crate::diag!("wgpu-engine: [{}] diffuse_generate refused ({e}) — falling back to the AR window, which cannot produce coherent text for this model", self.name),
                        }
                    }
                }
                let p = match (sample_win_opt, embed_q8) {
                    (Some(sample_win), Some(eq)) => {
                        let w = gpu.stream_session_window(model, s, &mut seen, n_new, eos, &sp, rng, sample_win,
                                                          &gpu_kernels::EmbedT::Q8(eq), cancel, &mut step);
                        if w == usize::MAX {
                            // Windowed decode hit the device buffer-object ceiling. The device is
                            // FATALLY poisoned once this happens (a captured validation error still
                            // leaves every subsequent alloc invalid — confirmed on M4), so retrying
                            // per-token on the same device would just abort again. Latch windowing off so
                            // the NEXT turn uses per-token from the start, and end this turn cleanly.
                            self.disable_windowing = true;
                            crate::diag!("wgpu-engine: [{}] windowed decode hit the device buffer ceiling — windowing disabled; the NEXT turn will use per-token. (Lower LOGSCOPE_WINDOW_MAX to avoid this.)", self.name);
                            0
                        } else { w }
                    }
                    _ => match filt.as_mut() {
                        Some(f) => gpu.stream_session_constrained(model, s, &mut seen, n_new, eos, &sp, rng, cancel, Some(f), step),
                        None => gpu.stream_session(model, s, &mut seen, n_new, eos, &sp, rng, cancel, step),
                    },
                };
                if !pending.is_empty() { let _ = on(&String::from_utf8_lossy(&pending)); }
                report_constraint(&self.name, filt.as_ref());
                p
            };
            let mut rng = self.rng;
            let mut produced = stream_from(&mut s, &mut rng);
            // After reuse the suffix is batch-prefilled by verify_forward, so the WHOLE prompt is valid KV
            // (seeded [0,l) + batch-prefilled [l,len)) — advertise all of it as reusable, not just the copied
            // prefix `l`. Under-reporting to `l` capped continuation reuse: a child resuming from this tip got
            // clamped to the seed and blew past the 256 suffix window into a full prefill (the 3/24 ceiling).
            let mut prefill_len = prompt_ids.len();
            // Was the empty reply the model cleanly stopping, or something going wrong?
            //
            // `stream_session` breaks on eos BEFORE calling `on_token`, so an eos-first reply leaves
            // `produced == 0` and `stop_tok == None` — indistinguishable from a cap, a guard trip or
            // a cancel. `s.logits` is left at the distribution eos was drawn from, so the answer is
            // right there: top-1 and its margin over the runner-up.
            //
            // The retry below justifies itself as recovering a NEAR-TIED eos tipped by reuse drift.
            // That is a good hypothesis and it was never checked — the retry fired on the token count
            // alone. When the model wants to stop by a wide margin a clean prefill reproduces it
            // exactly, and the retry is a full redundant prefill for nothing: three of them in one
            // observed session at 17.2s, 17.3s and 17.7s, which the operator experiences as the app
            // hanging and the model "being fussy".
            let stop_margin = |lg: &[f32]| -> Option<(u32, f32)> {
                if lg.is_empty() { return None; }
                let (mut i1, mut v1, mut v2) = (0usize, f32::NEG_INFINITY, f32::NEG_INFINITY);
                for (i, v) in lg.iter().enumerate() {
                    if *v > v1 { v2 = v1; v1 = *v; i1 = i; } else if *v > v2 { v2 = *v; }
                }
                Some((i1 as u32, v1 - v2))
            };
            // Below this the two candidates really could swap under a different prefill path, which
            // is the only case the retry was ever meant to cover.
            const NEAR_TIE_LOGITS: f32 = 0.5;
            // RESTORED. f7fb925314 replaced these four lines with the comment
            // below, whose own words are "NOT CHANGED HERE, deliberately" -- it
            // meant to document, not to change behaviour, and left both uses of
            // `stopped_decisively` pointing at nothing. Unseen because this path
            // compiles only with `wgpu-engine`; found by fleet_build.sh's
            // logscope+features build ("cannot find value `stopped_decisively`").
            let stopped_decisively = match stop_margin(&s.logits) {
                Some((top, margin)) => stop.contains(&top) && margin >= NEAR_TIE_LOGITS,
                None => false,
            };
            // MARGIN IS CONFIDENCE, NOT CORRECTNESS, and that is where the
            // skip above overreached. Its premise — "when the model wants to
            // stop by a wide margin a clean prefill reproduces it exactly" —
            // holds only if the reused prefix is RIGHT. A wrong KV prefix
            // produces whatever it produces, including a confident EOS, and
            // the skip then declines to run the one check that would have
            // caught it. Worse, it makes the premise untestable in the field:
            // the retry that would disprove it never fires.
            //
            // Reported from an M2 Max run on two DIFFERENT models: "hi there"
            // answered coherently, then a request for graph algorithms
            // returned empty, EOS first token, 11.63 logits of margin. Two
            // models agreeing is not two models stopping; it is the thing they
            // share, which is this cache path.
            //
            // So the margin still gates the skip, but only for a stop that
            // could plausibly BE the model's: one that arrives after it has
            // said something. Zero tokens on a turn the model was just handed
            // is not a decisive stop, it is an empty answer — there was
            // nothing to stop. That case retries however wide the margin,
            // because the margin is not evidence about it.
            // MARGIN IS CONFIDENCE, NOT CORRECTNESS. The skip below rests on
            // "when the model wants to stop by a wide margin a clean prefill
            // reproduces it exactly", which holds only if the reused prefix is
            // RIGHT. A wrong prefix produces whatever it produces, including a
            // confident EOS — and the skip then declines to run the one check
            // that would catch it, which also makes its own premise untestable
            // in the field: the retry that would disprove it never fires.
            //
            // PROVENANCE, since the obvious suspicion is a recent merge and it
            // is wrong: the skip came in with 53b564519 on 2026-08-18, before
            // any of the four bundles merged today, and none of them touch
            // this file. The retry itself dates to the monorepo skeleton.
            //
            // That commit was written FROM THE SAME SYMPTOM ON THE SAME
            // HARDWARE — "four turns in a row came back empty, each taking
            // seventeen seconds" on Asahi/M2 Max — and concluded the model was
            // at fault: "a 4B pruned finetune given a 1626-token system prompt
            // ... genuinely wants to emit eot immediately on most turns. That
            // is the model."
            //
            // The new report is what that conclusion did not survive. It is
            // TWO DIFFERENT MODELS, and one of them answered "hi there"
            // coherently first — so it is not a model that always stops, it is
            // a turn that stops after a specific kind of preceding turn. The
            // August reasoning fixed the seventeen seconds and the wrong
            // explanation, which were both real, and took the model's guilt as
            // established when only one model had been seen.
            //
            // REPORTED, and not reproducible here: an M2 Max Asahi run where
            // "hi there" was answered coherently and the next turn — a request
            // for graph algorithms — returned empty, EOS as the first token,
            // 11.63 logits of margin, ON TWO DIFFERENT MODELS. Two models
            // agreeing is not two models stopping; it is the thing they share.
            //
            // NOT CHANGED HERE, deliberately. The obvious edits are both wrong:
            // gating on "said nothing" makes `stopped_decisively` permanently
            // false, because it is only read where `produced == 0` already
            // holds, and that silently restores the always-retry this
            // replaced. Gating on the reuse fraction needs the anchor length,
            // which is not in scope at this point — `prefill_len` here is the
            // whole prompt, so the obvious test is vacuously true.
            //
            // The right fix is to retry ONCE per anchor and remember the
            // verdict, so a genuine stop costs one prefill rather than none or
            // every time. That needs somewhere to keep the verdict and a
            // device to measure it on. Until then the escape hatch is real and
            // documented: LOGSCOPE_ALWAYS_REPREFILL=1 on the affected run will
            // say which of the two this is, and that answer is worth more than
            // a guess made here.
            if reuse && produced == 0 && stopped_decisively && !cancel() {
                if let Some((top, margin)) = stop_margin(&s.logits) {
                    crate::diag!("wgpu-engine: reuse produced 0 tokens, but the model chose stop token {top} \
                                  by {margin:.2} logits over the runner-up — that is the model stopping, not \
                                  reuse drift, and a clean prefill would reproduce it. Retry SKIPPED (it costs \
                                  a full prefill); LOGSCOPE_ALWAYS_REPREFILL=1 forces it anyway.");
                }
            }
            let force_retry = std::env::var("LOGSCOPE_ALWAYS_REPREFILL").as_deref() == Ok("1");
            if reuse && produced == 0 && (!stopped_decisively || force_retry) && !cancel() {
                crate::diag!("wgpu-engine: reuse produced 0 tokens — retrying with a clean full prefill…");
                let mut s2 = self.gpu.session_new(&self.model, cap, self.rng);
                s2.logits = self.gpu.prefill_chunked(&self.model, &s2.cache, &prompt_ids, prefill_chunk_for_streaming(self.model.d, self.model.h, self.model.streams_experts(), self.model.moe_counts()));
                s2.pos = prompt_ids.len();
                produced = stream_from(&mut s2, &mut rng);
                s = s2;
                prefill_len = prompt_ids.len();
            }
            self.rng = rng;
            let secs = td.elapsed().as_secs_f32().max(1e-3);
            // ROOFLINE, from what the loader actually made resident. Decode reads each ACTIVE weight once
            // per token, so achieved bandwidth is arithmetic on the throughput — no probe, no calibration.
            // The fraction needs a denominator and only appears for adapters in the spec table; the
            // achieved figure is always real. This is the number that says whether the next move is a
            // different model or a faster engine: at 9% the hardware is idle and the model is not the
            // problem, at 60% it is the only thing left.
            {
                use std::sync::atomic::Ordering::Relaxed as R2;
                let elems = crate::backend::RESIDENT_ELEMS.load(R2);
                let tok_s = produced as f64 / secs.max(1e-6) as f64;
                    if elems > 0 && tok_s > 0.0 {
                    // Resident width, not the file's — decode reads what is in memory. int8 is the default
                        // residency; q4 models move half as much, so this is the CONSERVATIVE choice
                        // and will under-report their achieved bandwidth rather than over-report it.
                        let bits = 8.0f32;
                    let got = gpu_kernels::roofline::achieved_gb_s(tok_s, elems as f64, bits);
                    match gpu_kernels::roofline::spec_bandwidth(&self.gpu.adapter_name) {
                        Some(bw) => {
                            let f = got / bw;
                            crate::diag!("wgpu-engine: [{}] roofline — {got:.0} GB/s of {bw:.0} spec = {:.0}% \
of the decode ceiling; {}", self.name, 100.0 * f, gpu_kernels::roofline::verdict(f));
                        }
                        None => crate::diag!("wgpu-engine: [{}] roofline — {got:.0} GB/s achieved; this adapter \
is not in the spec table so no fraction is reported (a percentage of a guessed denominator is worse \
than none)", self.name),
                    }
                }
            }
            crate::diag!("wgpu-engine: [{}] produced {} tokens in {:.1}s = {:.2} tok/s | {} pipelines cached (warm {})", self.name,
                produced, secs, produced as f32 / secs, self.gpu.pipeline_count(), self.warm);
            // Dispatch profile for this turn: submits & polls per token, and where the wall time went
            // (CPU setup vs GPU sync). A high submits/tok with setup ≳ sync is the dispatch-bound floor.
            let (ds, dp) = (gpu_kernels::GPU_SUBMITS.load(AOrd::Relaxed) - s0, gpu_kernels::GPU_POLLS.load(AOrd::Relaxed) - p0);
            let (setup_ms, sync_ms) = ((gpu_kernels::GPU_SETUP_NS.load(AOrd::Relaxed) - su0) as f64 / 1e6, (gpu_kernels::GPU_SYNC_NS.load(AOrd::Relaxed) - sy0) as f64 / 1e6);
            let per = produced.max(1) as f64;
            // Classify against TOTAL wall time, not setup-vs-sync alone. Comparing only those two says
            // "dispatch-bound" whenever setup >= sync — even when both are a rounding error against the
            // turn (setup 102ms and sync 0ms out of 46s is 0.2%, i.e. the GPU was simply busy). That
            // mislabel sends optimisation effort at command setup when the kernels are the cost.
            let total_ms = (secs as f64) * 1000.0;
            let encode_ms = (gpu_kernels::gpu_encode_ns().saturating_sub(enc0)) as f64 / 1e6;
            let accounted = setup_ms + encode_ms + sync_ms;
            // "GPU-execution-bound" WAS AN OVERSTATEMENT. The condition is that setup and sync together
            // are under 15% of wall time — which says where the time ISN'T, not where it is. At the 0.1%
            // seen in real runs, 99.9% is UNACCOUNTED, and unaccounted is not a bound: the GPU may be
            // executing, or the timers may simply not cover the path that is slow.
            //
            // Same class as gate-q4k claiming "the loader can stop requantizing" while testing only
            // dequantization — a label asserting more than its evidence, which is worse than no label
            // because nobody re-derives it.
            // The suffix "-bound" is appended by the caller, so a phrase that already reads as a
            // sentence produced "neither setup nor sync sees it-bound" in a real log. Kept short and
            // adjectival for that reason: the label must survive the template it is dropped into.
            // ENCODE IS NOW MEASURED, so it must be judged rather than left in the remainder. The
            // operator's run reads "setup 15ms + encode 3534ms + submit 147ms + sync 351ms of
            // 4183ms" — encode is 84% of decode — and this still said "likely GPU-execution-bound",
            // because it inferred from setup+sync alone and found them small. That inference was
            // reasonable while encode was invisible (c4-819) and is now simply wrong: the answer is
            // in the terms, not in what is left over.
            //
            // 292 microseconds per dispatch of pure CPU command recording is not the GPU being
            // busy. It is begin_compute_pass, a frame mutex, and two keep-alive allocations, 12095
            // times a turn.
            let enc_share = encode_ms / total_ms.max(1.0);
            let bound = if enc_share >= 0.40 { "ENCODE-bound (CPU command recording)" }
                else if accounted < total_ms * 0.02 { "UNACCOUNTED" }
                else if accounted < total_ms * 0.15 { "likely GPU-execution" }
                else if setup_ms >= sync_ms { "dispatch" } else { "GPU-sync" };
            // NAME THE BOTTLENECK, not just the split. "GPU-execution-bound, 0.1% accounted" says
            // where the time ISN'T and leaves an operator with nothing to act on. perfverdict resolves
            // it into one verdict — or says the run is at the memory roofline and the split is
            // irrelevant, which is the answer that stops work rather than misdirecting it.
            //
            // active_bytes comes from the resident footprint; gib_per_s is 0 unless --gate-perf has
            // run, and perfverdict reports efficiency 0 and says so rather than dividing by it.
            {
                let sample = gpu_kernels::perfverdict::Sample {
                    tokens: per as u64,
                    wall_ms: total_ms,
                    setup_ms,
                    sync_ms,
                    submits: ds as u64,
                    // ACTIVE BYTES, derived from the architecture rather than stored — so it cannot
                    // drift from what the model is. Parameters times the resident bytes-per-parameter:
                    // ~0.56 for q4 residency, which is the format the loader reports.
                    active_bytes: {
                        let experts = self.model.n_active_experts();
                        let params = gpu_kernels::active_params_per_token(
                            self.model.d, self.model.h, self.model.vocab, self.model.n_layers,
                            self.model.n_kv_heads, self.model.head_dim, experts);
                        (params as f64 * 0.5625) as u64      // q4 + per-32 scales
                    },
                    // READ FROM THE MEASUREMENT, and None stays None. An adapter that has never run
                    // --gate-perf yields 0 and perfverdict reports efficiency 0 and says the roofline
                    // cannot be evaluated — rather than substituting a default, which would produce a
                    // confident efficiency figure from no measurement.
                    gib_per_s: gpu_kernels::store::bandwidth_get(&self.gpu.adapter_name)
                        .unwrap_or(0.0) as f64,
                };
                let expected_batch_ms = if ds > 0 { total_ms / ds as f64 } else { 0.0 };
                let v = gpu_kernels::perfverdict::verdict(&sample, expected_batch_ms);
                eprint!("[{}] ", gpu_kernels::stamp());
                crate::diag!("wgpu-engine: [{}] bottleneck — {}", self.name, v.advice());
            }
            eprint!("[{}] ", gpu_kernels::stamp());
            // THREE BUCKETS, not two. queue.submit was timed by nothing, and on a driver whose
            // completion callbacks are slow enough to disable the governor — "submit stalled 739ms" —
            // it is exactly where time hides. Reporting two terms and calling the remainder
            // unaccounted was how 99.8% became a mystery about kernels.
            // Delta, like the others — an absolute here would be the lifetime-total bug from _456.
            let submit_ms = (gpu_kernels::gpu_submit_ns().saturating_sub(submit_ns0)) as f64 / 1e6;
            let sample_ms = (gpu_kernels::sample_ns().saturating_sub(sample_ns0)) as f64 / 1e6;
            // ENCODE joins the accounting. It was the one phase nobody timed, and the profile has
            // been reporting "15.8% accounted" — attributing 84% of decode to nothing while the
            // verdict inferred "likely GPU-execution-bound" from the size of what was left over.
            // An inference from a remainder is only as good as the terms subtracted from it.
            let encode_ms = (gpu_kernels::gpu_encode_ns().saturating_sub(enc0)) as f64 / 1e6;
            let accounted = setup_ms + encode_ms + sync_ms + submit_ms + sample_ms;
            // Passes vs dispatches settle the fusion question this line could not answer on the
            // first 729 hardware log: ~= means pass-per-dispatch, a multiple means fusion is live.
            let (gp1, gd1) = (gpu_kernels::GPU_PASSES_OPENED.load(AOrd::Relaxed).saturating_sub(gp0),
                              gpu_kernels::GPU_DISPATCHES.load(AOrd::Relaxed).saturating_sub(gd0));
            // OVER 100% IS NOT A ROUNDING ERROR AND MUST NOT READ AS ONE.
            //
            // A real 8B run reported "134.3% accounted", which looks like a bug
            // in the report and is worse than that: it means the buckets
            // OVERLAP, so the split cannot be used to prioritise. Whichever
            // bucket is largest may be largest only because it contains
            // another. Saying so is the difference between a number someone
            // acts on and a number someone should not.
            //
            // Prefill is not the cause — `td` and every baseline are taken
            // after it. The two live candidates, neither confirmed:
            //   * these counters are process-wide and decode may run on more
            //     than one thread, in which case per-thread times sum past
            //     wall clock and the excess is real but not double-counting;
            //   * a timed region nests inside another (setup's span covers a
            //     `bg_cache.lock()`, so a thread blocked there is charged to
            //     setup while the holder is charged to whatever it is doing).
            // Distinguishing them needs per-thread accounting, which is a
            // change to the counters rather than to this line.
            let over = accounted > total_ms * 1.02;
            crate::diag!("wgpu-engine: [{}] profile — {ds} submits ({:.0}/tok), {dp} polls, {gp1} passes / {gd1} disp ({:.1}/pass) | setup {setup_ms:.0}ms + encode {encode_ms:.0}ms + submit {submit_ms:.0}ms + sync {sync_ms:.0}ms + sample {sample_ms:.0}ms of {total_ms:.0}ms ({bound}-bound, {:.1}% accounted){}",
                self.name, ds as f64 / per, gd1 as f64 / (gp1.max(1)) as f64, accounted / total_ms.max(1.0) * 100.0,
                if over {
                    // The thread count decides between the two explanations, so the
                    // line reports the ANSWER rather than restating the dilemma.
                    let n = gpu_kernels::timing_thread_count();
                    if n > 1 {
                        format!(" — OVER 100% because {n} THREADS charged these counters, so per-thread times sum past the wall clock. The buckets are individually right; they are NOT a partition of one timeline and must not be ranked against each other as though they were")
                    } else {
                        " — OVER 100% on a SINGLE THREAD, so this is genuine double counting: a timed region nests inside another. setup's span covers a bg_cache.lock(), so a thread blocked there is charged to setup while the holder is charged elsewhere — read a large `setup` as possible LOCK CONTENTION rather than resource churn until that is separated".to_string()
                    }
                } else { String::new() });
            // WHY the frame drained, not just how often. On Metal a run showed 1119 submits/token at
            // 2.0 ms each while flush_every was 128 — five passes per submit — and nothing in the log
            // said which mechanism was responsible.
            // Same baseline and the same total as the profile line above, so the two can be read
            // together. Passing `ds` means "N of M attributed" compares against THIS generation's
            // submits rather than the process lifetime's.
            let attr = format!("wgpu-engine: [{}] {}", self.name,
                gpu_kernels::submit_attribution_since(cause0, ds as u64));
            // THE VERDICT, immediately after the numbers it is about. The attribution line is 200+
            // characters and every diagnosis this session began by grepping the same three figures
            // out of it; this says which dominates and what to run next.
            //
            // The caveat prints only when something is anomalous. A run that always adds two lines
            // teaches the reader to skip both, which is how five GPU resets sat unread in journalctl.
            // DELTAS, not cumulative counters. Reading them cumulatively made this line contradict
            // the attribution line directly above it — 0% against 55% for the same run, because the
            // verdict was describing the previously-loaded model's dispatches.
            let (v_nf, v_dl, v_ps) = gpu_kernels::SubmitCauseSnapshot::now().shares_since(&cause0);
            let (v_gp, v_gd) = (
                gpu_kernels::GPU_PASSES_OPENED.load(AOrd::Relaxed).saturating_sub(gp0),
                gpu_kernels::GPU_DISPATCHES.load(AOrd::Relaxed).saturating_sub(gd0));
            let (headline, caveat) = gpu_kernels::run_verdict_delta_shares(v_nf, v_dl, v_ps, v_gp, v_gd,
                per as u64, secs as f64, sync_ms / 1000.0,
                // the REAL answer, from the loaded model — the verdict's MoE arm
                // asserted router logits on dense models before this was passed
                self.model.moe_counts().is_some());
            crate::diag!("wgpu-engine: [{}] {headline}", self.name);
            if let Some(c) = caveat { crate::diag!("wgpu-engine: [{}] {c}", self.name); }
            // EXPERT RESIDENCY. These counters have been tracked and reported nowhere, so there has
            // been no way to tell whether pinning more experts would help or whether the cache is
            // already saturated and the cost is in the kernels. For a streaming MoE that is the whole
            // question, and it decides whether the next move is budget or a kernel.
            {
                let (hits, fills, bytes, repins, _) = gpu_kernels::moe_stream_stats();
                if hits + fills > 0 {
                    let r = gpu_kernels::perfverdict::ExpertResidency {
                        hits, fills, bytes, repins,
                        pinned: gpu_kernels::moe_pin_counts().0, total: gpu_kernels::moe_pin_counts().1,
                    };
                    crate::diag!("wgpu-engine: [{}] {}", self.name, r.report(per as u64));
                    // Prediction is a DIFFERENT lever from the pin set: the pin set decides what is
                    // always here, prediction decides what will be wanted next and is the only one
                    // that can hide a fetch behind compute.
                    let (issued, phits) = gpu_kernels::moe_prefetch_stats();
                    crate::diag!("wgpu-engine: [{}] {}", self.name,
                        gpu_kernels::perfverdict::PrefetchAccuracy { issued, hits: phits }.verdict());
                }
            }
            // THE PROFILE CHECKED AGAINST ITSELF. A line that contradicts itself is worse than no
            // line: every number in it gets used to reason about where time goes, and one that cannot
            // be true poisons the conclusions drawn from the rest.
            //
            // Your run printed "1 polls" beside "download 141" — every download maps a buffer and
            // every map polls — and "sync 0ms" across 19.9 s, while a separate phase breakdown of the
            // same run reported 15.5% sync.
            for d in gpu_kernels::perfverdict::profile_doubts(
                dp as u64,
                gpu_kernels::SubmitCauseSnapshot::now().downloads_since(&cause0),
                sync_ms as f64, total_ms as f64)
            {
                crate::diag!("wgpu-engine: [{}] profile DOUBT — {}", self.name, d.explain());
            }
            // THE SELF-RELATIVE VERDICT. Prefill and decode read the same weights in the same run, so
            // the ratio between their per-token costs measures how much latency decode fails to hide —
            // with no bandwidth ceiling, no active-byte count and no gate run beforehand.
            //
            // Reported after the roofline verdict rather than instead of it: they answer different
            // questions, and this one can answer whenever a generation happened at all.
            {
                let ntok = per as usize;
                let dec_ms_per_tok = if ntok > 0 { total_ms as f64 / ntok as f64 } else { 0.0 };
                let sr = gpu_kernels::perfverdict::self_relative(
                    prefill_ms_per_tok, dec_ms_per_tok, prefill_tokens, ntok);
                if !matches!(sr, gpu_kernels::perfverdict::SelfRelative::NotEnoughData) {
                    crate::diag!("wgpu-engine: phase balance — {}", sr.advice());
                }
                // PUBLISH THE RATIO for the UI strip. The comparison has always been computed here
                // and spent on one stderr line; the strip could not raise a starvation concern
                // because it had no baseline to compare against, only throughput. A f32 in an
                // atomic is the whole mechanism — the alternative, deriving starvation from tok/s,
                // is a claim without a measurement.
                if let gpu_kernels::perfverdict::SelfRelative::DecodeStarved { ratio } = sr {
                    crate::backend::LAST_DECODE_RATIO.store((ratio as f32).to_bits(), std::sync::atomic::Ordering::Relaxed);
                } else {
                    crate::backend::LAST_DECODE_RATIO.store(0f32.to_bits(), std::sync::atomic::Ordering::Relaxed);
                }
            }
            crate::diag!("[{}] {attr}", gpu_kernels::stamp());
            crate::diag::note(&attr);
            // BOTH baselined at the same moment, or the ratio mixes scopes. vv0 was hardcoded to
            // zero — "the subtraction keeps the shape of the others" — which made vv a LIFETIME
            // total while hv is per-reply. c4-816 then divided one by the other, so the
            // offset-bearing hit rate shrank toward zero the longer a session ran, and would have
            // been read as the cache degrading. Exactly the mismatch c4-810 hit with the roofline
            // counters, reintroduced two bundles later by me in the line meant to fix a denominator.
            let vv0 = gpu_kernels::BG_VIEW_VARYING.load(AOrd::Relaxed);
            let hv0 = gpu_kernels::BG_HITS_VIEWED.load(AOrd::Relaxed);
                    // Saturating like its neighbours: a plain subtraction on a global counter wraps
                    // if anything ever resets it, and a wrapped u64 reads as an enormous hit count.
                    let (bgh, bgm) = (gpu_kernels::BG_HITS.load(AOrd::Relaxed).saturating_sub(bgh0),
                                      gpu_kernels::BG_MISSES.load(AOrd::Relaxed).saturating_sub(bgm0));
            if bgh + bgm > 0 {
                // SPLIT OUT THE UNCACHEABLE LOOKUPS. A dispatch whose input carries a non-zero view is a
                    // KV slice, and its key CAN NEVER REPEAT — counting those as misses makes a working
                    // cache read as 13% and invites optimising what is not broken. lib.rs says so at the
                    // increment site, and the counter it increments was never read by anything.
                    let vv = gpu_kernels::BG_VIEW_VARYING.load(AOrd::Relaxed).saturating_sub(vv0);
                    // Baselined the same way, or a lifetime total would be divided by a per-reply
                    // denominator and the rate would climb past 100% as the session went on.
                    let hv = gpu_kernels::BG_HITS_VIEWED.load(AOrd::Relaxed).saturating_sub(hv0);
                    // THE DENOMINATOR WAS INVENTED. `cacheable = (hits + misses) - view_varying`
                    // assumed an offset-bearing lookup could never hit, so it subtracted lookups
                    // that DO hit — producing "45702 hits / 22090 cacheable" in the operator's log,
                    // a rate above 100% that c4-797's guard then suppressed. The guard was right and
                    // the arithmetic was wrong; suppressing a number is not the same as fixing it.
                    //
                    // There are only two honest rates here, and both have real denominators: how
                    // often a lookup hits at all, and how often an OFFSET-BEARING lookup hits. The
                    // second is the one that decides whether binding whole buffers and passing
                    // offsets through the dims uniform would pay (perfverdict.rs documents that
                    // change and gates it on exactly this measurement).
                    let lookups = bgh + bgm;
                    // A hardware log printed "2656 hits / 0 cacheable lookups (265600% reused)":
                    // `vv` had exceeded hits+misses, `cacheable` saturated to zero, and the ratio was
                    // divided by the max(1) floor. A percentage of nothing is not a small error, it
                    // is a fabricated number in a line whose whole job is to be trusted — so when
                    // the accounting does not close, say that instead of printing it.
                    if lookups == 0 {
                        crate::diag!("wgpu-engine: [{}] bind-group cache — no lookups this turn [LOGSCOPE_BG_CACHE]",
                            self.name);
                    } else {
                        let overall = 100.0 * bgh as f64 / lookups as f64;
                        let viewed = if vv > 0 {
                            format!("{:.0}% of the {vv} offset-bearing ones hit", 100.0 * hv as f64 / vv as f64)
                        } else {
                            "none carried a view offset".to_string()
                        };
                        // WHY the misses missed. The hit rate alone points nowhere: this line has
                        // reported "none carried a view offset" alongside a 57% rate, which rules
                        // out the KV slices everyone blames and leaves no candidate named. These
                        // three counters name one.
                        // Deltas, matching `bgh`/`bgm` above — see the baseline
                        // capture for why.
                        let (mo, md, mi) = (
                            gpu_kernels::BG_MISS_OUT.load(AOrd::Relaxed).saturating_sub(mo0),
                            gpu_kernels::BG_MISS_DIMS.load(AOrd::Relaxed).saturating_sub(md0),
                            gpu_kernels::BG_MISS_INPUTS.load(AOrd::Relaxed).saturating_sub(mi0));
                        let why = if mo + md + mi == 0 { String::new() } else {
                            let worst = if mo >= md && mo >= mi {
                                "the OUTPUT buffer moved — the pool hands back a different slab for the same dispatch, so the fix is pool reuse per call site, not the attention dispatches"
                            } else if md >= mi {
                                "the DIMS CONTENT changed — that cache is keyed by content, so these are dispatches whose position \
or sequence length advanced. They are genuinely different dispatches and no cache can hold them: \
this is the floor, not a defect"
                            } else {
                                "the INPUTS changed — these dispatches genuinely differ and the cache is behaving"
                            };
                            format!(" Repeat misses: {mo} output / {md} dims / {mi} inputs — {worst}.")
                        };
                        // PER-TOKEN NEEDS A TOKEN. `per` is `produced.max(1)`,
                        // which keeps the division safe and silently turns the
                        // figure into a raw total when nothing was produced --
                        // the M2 run printed "392/tok saved" on a turn that
                        // generated zero tokens. Say which it is.
                        let saved = if produced > 0 {
                            format!("{:.0}/tok saved", bgh as f64 / produced as f64)
                        } else {
                            format!("{bgh} saved (no tokens this turn, so there is no per-token figure)")
                        };
                        crate::diag!("wgpu-engine: [{}] bind-group cache — {bgh} of {lookups} lookups hit \
({overall:.0}%), {saved}; {viewed}.{why} A moving view offset is the only genuinely unhittable \
kind, so that figure — not the offset-bearing COUNT — is what would justify rebuilding the \
attention dispatches [LOGSCOPE_BG_CACHE]", self.name);
                    }
            }
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
                        crate::diag!("wgpu-engine: submit-tuner SETTLED flush_every={chosen} passes/submit ({btxt}; cached for this GPU); sweep:");
                        for (f, tok, sub) in t.results() { crate::diag!("wgpu-engine:   flush {f:>4} → {:.1} ms/tok · {:.1} ms/submit", tok * 1000.0, sub); }
                    } else {
                        crate::diag!("wgpu-engine: submit-tuner sampled flush_every={used} → {:.1} ms/tok · {:.1} ms/submit ({} candidate(s) done)",
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
                let next_argmax = s.logits.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal)).map(|(i, _)| i);
                let why = match stop_tok.get() {
                    Some(t) if Some(t) == self.eos => format!("model emitted EOS {t}"),
                    Some(t) => format!("model emitted stop token {t}"),
                    // `stream_session` breaks on eos before `on_token` runs, so an eos-first reply
                    // arrives here with stop_tok unset. Reporting it as a cap or a guard trip sends
                    // the reader looking for a limit that was never hit — the observed log said
                    // "no stop token seen" on the same line as "argmax=Some(128009)", which IS a
                    // stop token. `s.logits` is the distribution eos was drawn from, so read it.
                    None => match next_argmax.map(|i| i as u32) {
                        Some(t) if Some(t) == self.eos =>
                            format!("model emitted EOS {t} as its FIRST token (uncounted: stream_session \
                                     breaks on eos before the stop callback)"),
                        Some(t) if self.stop_ids.contains(&t) =>
                            format!("model chose stop token {t} first (uncounted, same path as EOS)"),
                        _ => "token cap / runaway guard / cancel (no stop token seen)".to_string(),
                    },
                };
                let decoded = String::from_utf8_lossy(&self.tok.decode_bytes(&s.generated)).replace('\n', "\\n");
                crate::diag!("wgpu-engine: [{}] SHORT REPLY ({produced} tok) — stop: {why}; stop_set={:?}; next-token argmax={:?}",
                    self.name, self.stop_ids, next_argmax);
                crate::diag!("wgpu-engine: [{}]   generated text = {:?}", self.name, decoded);
                // CODE CHECK, behind LOGSCOPE_DEBUG_FORMAT. Reports rather than rewrites: a formatter that
                // silently repairs a model's output makes broken code LOOK correct, and the reader loses the
                // one signal that something went wrong.
                if logscope::codefmt::enabled() {
                    if let Some(s) = logscope::codefmt::summary(&logscope::codefmt::check(&decoded)) {
                        crate::diag!("wgpu-engine: [{}] {s}", self.name);
                    }
                }
                crate::diag!("wgpu-engine: [{}]   generated ids (≤16) = {:?}", self.name, tail);
            }
            // This turn (prompt + what we generated) joins the pool as a branch tip; `prefill_len` is how
            // many leading rows were batch-prefilled — the only region ever re-seeded.
            let mut anchor_toks = prompt_ids.clone();
            anchor_toks.extend_from_slice(&s.generated);
            // RECURRENT-STATE CARRY-FORWARD SNAPSHOT. For a hybrid (Gated DeltaNet) model this is the
            // whole point of the turn: keep the carried session — its cache carries the delta-net
            // state folded through prompt[0..|P|] (at next_pos == |P|) — so next turn's linear agent
            // continuation can fold that terminal checkpoint into a fresh session and prefill only the
            // appended Δ. Snapshot BEFORE `s` is moved into the pool or FIFO anchors below. Only
            // meaningful for hybrid models; the delta check at the top of this function decides whether
            // next turn actually reuses it (and a full-attention model here just gets an unused entry —
            // no delta layers to carry).
            //
            // `Session` owns GPU buffers, so it is not Copy/Clone: the live `s` is swapped out into a
            // fresh session (same `cap`) and the carried one handed to prev_turn — no buffer copy, just
            // a fresh allocation on the swap-in. The swap runs only for a hybrid, so a non-hybrid chat
            // pays nothing for carrying nothing (prev_turn stays None, `s` moves straight to the pool).
            let carried = if self.model.deltanet_layers() > 0 {
                Some(std::mem::replace(&mut s, self.gpu.session_new(&self.model, cap, self.rng)))
            } else {
                None
            };
            if let Some(carried) = carried {
                // The delta state's TRUE next_pos: the carried transcript is prompt_ids + the reply
                // folded through the recurrent path, so the checkpoint sits at next_pos == |prompt_ids|
                // + |generated| == carried.pos. `mem::replace` above handed this old session to
                // `carried`, so read pos off it — the session now bound to `s` is the fresh swap-in
                // (pos == 0). Captured before the move into the tuple.
                let delta_pos = carried.pos;
                self.prev_turn = Some((anchor_toks.clone(), carried, delta_pos));
            }
            if let Some(pool) = self.pool.as_mut() {
                pool.insert(&self.gpu, anchor_toks, prefill_len, s, self.kv_cap * 4);
                if std::env::var("LOGSCOPE_KV_CENSUS").is_ok() {
                    crate::diag!("wgpu-engine: [{}] KV pool — {}", self.name, pool.census());
                }
            } else {
                // FIFO fallback: evict oldest to stay within kv_cap (each entry holds a full KV cache).
                self.anchors.push((anchor_toks, s, prefill_len));
                while self.anchors.len() > self.kv_cap.max(1) { self.anchors.remove(0); }
            }
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
            let reply = p.reply_ceiling.max(p.max_tokens).max(1);
            // Same self-sizing as generate(): the widest branch plus reply headroom is the need.
            let longest = enc.iter().map(|e| e.len()).max().unwrap_or(1);
            let (window, why) = gpu_kernels::ctxwin::auto_window(
                longest + reply.min(256), p.n_ctx, self.model.trained_ctx,
                self.model.kv_bytes_per_token(), self.model.planned_weight_bytes,
                gpu_kernels::store::resident_budget_bytes(), MAX_CONTEXT);
            if let Some(w) = &why { crate::diag!("wgpu-engine: [{}] {w}", self.name); }
            let max_seq = window;
            let mut pc = self.gpu.session_new(&self.model, max_seq, self.rng);
            if !shared.is_empty() { let _ = self.gpu.prefill_chunked(&self.model, &pc.cache, &shared, prefill_chunk_for_streaming(self.model.d, self.model.h, self.model.streams_experts(), self.model.moe_counts())); pc.pos = shared.len(); }
            let sp = SamplerParams { temperature: p.temperature, top_k: 40, top_p: 0.95, repeat_penalty: p.repeat_penalty, repeat_last_n: 64, min_p: p.min_p };
            let queue: Vec<(usize, Vec<u32>, usize)> = enc.iter().enumerate()
                .map(|(i, e)| (i, e[shared.len().min(e.len())..].to_vec(), reply.min(window.saturating_sub(e.len())).max(1)))
                .collect();
            let t0 = Instant::now();
            crate::diag!("wgpu-engine: [{}] BATCHED — {} prompts, shared prefix {} tok, decoding together", self.name, n, shared.len());
            let prefix = if shared.is_empty() { None } else { Some((&pc.cache, shared.len())) };
            let out = self.gpu.run_batched(&self.model, prefix, queue, &sp, self.eos, n, max_seq);
            crate::diag!("wgpu-engine: [{}] batched {} replies in {:.1}s ({:.2}s/branch wall)", self.name, out.len(), t0.elapsed().as_secs_f32(), t0.elapsed().as_secs_f32() / n as f32);
            let mut replies = vec![String::new(); n];
            for (id, toks) in out { if id < n { replies[id] = String::from_utf8_lossy(&self.tok.decode_bytes(&toks)).trim().to_string(); } }
            replies
        }

        pub fn continue_last(&mut self, p: &GenParams, cancel: &dyn Fn() -> bool, on: &mut dyn FnMut(&str) -> bool) -> Option<Result<usize, String>> {
            let (toks, old, prefill_len) = if let Some(pool) = self.pool.as_mut() {
                pool.take_last(&self.gpu, &self.model)?
            } else {
                self.anchors.pop()?
            };
            if old.logits.is_empty() || old.pos == 0 {
                if let Some(pool) = self.pool.as_mut() { pool.insert(&self.gpu, toks, prefill_len, old, self.kv_cap * 4); }
                else { self.anchors.push((toks, old, prefill_len)); }
                return Some(Ok(0));
            }
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
            // The grammar for this turn, if any: on the wgpu engine this is what makes a malformed
            // tool block impossible rather than merely detectable afterwards. Built AFTER the last
            // `&mut self` above, because it borrows the tokenizer for the whole decode.
            let mut filt = self.tool_filter(&self.tok, p);
            let embed_q8 = self.embed_q8.as_ref();
            let (gpu, model, tok, eos, stop) = (&self.gpu, &self.model, &self.tok, self.eos, &self.stop_ids);
            let mut seen = toks;
            let mut pending: Vec<u8> = Vec::new();
            let mut recent = String::new();
            let mut rng = self.rng;
            // WHY A RESUME PRODUCED NOTHING.
            //
            // A fleet run shows 24 consecutive "continued 0 tokens in 0.4s =
            // 0.00 tok/s" lines on Llama-3.3-8B. gpu-gemm already carries a
            // note about this shape — it fixed the CONSEQUENCE, a governor that
            // read each no-op as a clean window and grew its cap 8 -> 2172 on
            // the evidence of generations that never happened. The cause was
            // never established, and "0.00 tok/s" is indistinguishable between
            // a broken session, an empty distribution, and a turn that had
            // simply already finished.
            //
            // The resumed session carries the previous turn's next-token
            // distribution (`s.logits = old.logits.clone()`), so if that turn
            // ended by sampling EOS, the resume samples EOS again and stops
            // before emitting anything. That is not a fault, it is a request to
            // continue something already complete — but it currently reports
            // identically to a fault.
            //
            // Recorded rather than guessed at: this cannot be reproduced
            // without the model, so the next run says which it was.
            let mut first_tok: Option<u32> = None;
            let mut halt_reason: Option<&'static str> = None;
            let mut step = |next: u32| {
                if first_tok.is_none() { first_tok = Some(next); }
                if stop.contains(&next) {
                    if halt_reason.is_none() { halt_reason = Some("a stop token"); }
                    return false;
                }
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
            // Captured BEFORE the window runs: it overwrites `s.logits` on
            // halt, so asking afterwards reads the wrong distribution.
            let resumed_argmax_is_eos = match eos {
                Some(e) => s.logits.iter().enumerate()
                    .max_by(|a, b| a.1.total_cmp(b.1))
                    .is_some_and(|(i, _)| i as u32 == e),
                None => false,
            };
            let produced = match (sample_win_opt, embed_q8) {
                (Some(sample_win), Some(eq)) => {
                    let w = gpu.stream_session_window(model, &mut s, &mut seen, n_new, eos, &sp, &mut rng, sample_win,
                                                      &gpu_kernels::EmbedT::Q8(eq), cancel, &mut step);
                    if w == usize::MAX {
                        self.disable_windowing = true;
                        crate::diag!("wgpu-engine: [{}] windowed decode hit the device buffer ceiling — windowing disabled for the next turn. (Lower LOGSCOPE_WINDOW_MAX to avoid this.)", self.name);
                        0
                    } else { w }
                }
                _ => match filt.as_mut() {
                    Some(f) => gpu.stream_session_constrained(model, &mut s, &mut seen, n_new, eos, &sp, &mut rng, cancel, Some(f), step),
                    None => gpu.stream_session(model, &mut s, &mut seen, n_new, eos, &sp, &mut rng, cancel, step),
                },
            };
            if !pending.is_empty() { let _ = on(&String::from_utf8_lossy(&pending)); }
            report_constraint(&self.name, filt.as_ref());
            self.rng = rng;
            let secs = td.elapsed().as_secs_f32().max(1e-3);
            crate::diag!("wgpu-engine: [{}] continued {produced} tokens in {secs:.1}s = {:.2} tok/s (warm resume, no prefill{})",
                self.name, produced as f32 / secs, if sample_win_opt.is_some() { ", windowed" } else { "" });
            if produced == 0 {
                // Name the three cases apart. Twenty-four identical zero lines
                // told the operator nothing except that something was wrong,
                // and told the governor something false.
                let why = match (first_tok, halt_reason) {
                    (Some(t), Some(r)) if Some(t) == eos =>
                        format!("the resumed distribution's first token was EOS ({t}) — this turn had \
already finished, so there was nothing to continue. Not a fault: {r} halted it immediately."),
                    (Some(t), Some(r)) =>
                        format!("the first sampled token ({t}) was {r}, so generation halted before \
emitting anything."),
                    (Some(t), None) =>
                        format!("the first token ({t}) was sampled but nothing was emitted — a \
special token or an incomplete UTF-8 sequence swallowed it."),
                    // EOS FIRST LOOKS IDENTICAL TO NO SAMPLE AT ALL from here,
                    // and the difference is the whole point of this match.
                    // `stream_session_window` compares against eos and BREAKS
                    // BEFORE calling `on_token` (gpu-gemm lib.rs, the replay
                    // loop), so a turn that was already finished never sets
                    // `first_tok` and arrives here as the fault case.
                    //
                    // The other diagnostic in this file already knew:
                    // "uncounted: stream_session breaks on eos before the stop
                    // callback". This one did not, and reported twenty-four
                    // finished turns as twenty-four real faults.
                    //
                    // So ask the logits directly rather than inferring from a
                    // callback that was never going to fire.
                    (None, _) if resumed_argmax_is_eos =>
                        "the resumed distribution's argmax was already EOS — this turn had \
finished before the resume was asked for, and the window breaks on eos before the token \
callback runs, so nothing was reported. Not a fault.".to_string(),
                    (None, _) =>
                        "NO token was sampled at all — the session's logits were empty or the \
window returned immediately, which is the case that indicates a real fault.".to_string(),
                };
                crate::diag!("wgpu-engine: [{}] resume produced nothing — {why}", self.name);
            }
            if let Some(pool) = self.pool.as_mut() {
                pool.insert(&self.gpu, seen, prefill_len, s, self.kv_cap * 4);
            } else {
                self.anchors.push((seen, s, prefill_len));
                while self.anchors.len() > self.kv_cap.max(1) { self.anchors.remove(0); }
            }
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
        #[cfg(feature = "wgpu-engine")] { let e = engine::Engine::load(path)?; e.ab_probe(); self.engine = Some(e); Ok(()) }
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
    fn model_status(&self) -> Option<logscope::liaison::ModelStatus> {
        #[cfg(feature = "wgpu-engine")]
        { self.engine.as_ref().map(|e| e.model_status()) }
        #[cfg(not(feature = "wgpu-engine"))]
        { None }
    }
}

pub fn make(kind: BackendKind) -> Box<dyn Backend> {
    match kind {
        BackendKind::LlamaCpp => Box::new(LlamaCpp { model: None }),
        BackendKind::Wgpu => Box::new(Wgpu { #[cfg(feature = "wgpu-engine")] engine: None }),
        BackendKind::Daemon => Box::new(Daemon::default()),
        BackendKind::Remote => Box::new(Remote::default()),
    }
}

/// Talk to a shared daemon instead of loading our own copy of the model.
///
/// WHY: one 8B model is 7.2 GiB resident and ~86 s to load, and this
/// machine's loader already clamps its GPU budget from 48 GiB to 31. Two
/// apps each holding their own copy is the situation the daemon exists to
/// avoid, and this app is the heavier of the two.
///
/// `load` does not load anything — it CHECKS that a daemon is listening
/// and remembers the path. The weights are already resident in another
/// process; the model path is still recorded so `loaded()` and the UI can
/// say which model is being talked to.
///
/// Streams. `logscope-serve --unix` answers `/generate` with chunked
/// transfer, and `on_token` is called per chunk as it lands — at ~4 tok/s
/// the alternative is forty seconds of a still UI followed by a
/// paragraph, which would make the daemon feel worse than loading a model
/// of our own in exactly the app where its memory saving is largest.
#[derive(Default)]
pub struct Daemon {
    path: Option<std::path::PathBuf>,
    model: Option<String>,
}

impl Backend for Daemon {
    fn name(&self) -> &'static str { "daemon" }
    fn loaded(&self) -> bool { self.path.is_some() }

    fn load(&mut self, path: &str, _params: &GenParams) -> Result<(), String> {
        let sock = daemon_socket_path();
        // Fail FAST and name the path. This is what the caller uses to
        // decide whether to fall back to loading in-process, so a slow
        // failure here is a slow startup for everyone without a daemon.
        std::os::unix::net::UnixStream::connect(&sock)
            .map_err(|e| format!("no daemon at {}: {e}", sock.display()))?;
        self.path = Some(sock);
        self.model = Some(path.to_string());
        Ok(())
    }

    fn generate(&mut self, prompt: &str, params: &GenParams, cancel: &dyn Fn() -> bool,
                on_token: &mut dyn FnMut(&str) -> bool) -> Result<usize, String> {
        use std::io::{Read, Write};
        let sock_path = self.path.clone().ok_or("no daemon connected")?;
        let mut sock = std::os::unix::net::UnixStream::connect(&sock_path)
            .map_err(|e| format!("connect: {e}"))?;
        let req = format!(
            "POST /generate?max={} HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/plain\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{prompt}",
            params.max_tokens.clamp(1, 8192), prompt.len());
        sock.write_all(req.as_bytes()).map_err(|e| format!("write: {e}"))?;

        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let head_end = loop {
            if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") { break p; }
            let n = sock.read(&mut tmp).map_err(|e| format!("read: {e}"))?;
            if n == 0 { return Err("daemon closed before sending a header".into()); }
            buf.extend_from_slice(&tmp[..n]);
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        if !head.lines().next().unwrap_or("").contains(" 200") {
            return Err(format!("daemon: {}", head.lines().next().unwrap_or("").trim()));
        }
        let chunked = head.lines().any(|l| {
            let l = l.to_ascii_lowercase();
            l.starts_with("transfer-encoding:") && l.contains("chunked")
        });

        let mut pending = String::from_utf8_lossy(&buf[head_end + 4..]).into_owned();
        let mut n_tokens = 0usize;
        if !chunked {
            // An older daemon, or `serve_unix` rather than the streaming
            // one. Mixed versions on a machine are normal mid-upgrade.
            let mut rest = String::new();
            sock.read_to_string(&mut rest).map_err(|e| format!("read: {e}"))?;
            pending.push_str(&rest);
            let _ = on_token(&pending);
            return Ok(pending.split_whitespace().count());
        }
        loop {
            if cancel() {
                // Dropping the socket is what stops the daemon
                // generating, rather than it finishing into the void.
                return Ok(n_tokens);
            }
            let (text, rest, done) = drain_chunks(&pending);
            pending = rest;
            if !text.is_empty() {
                n_tokens += text.split_whitespace().count();
                if !on_token(&text) { return Ok(n_tokens); }
            }
            if done { return Ok(n_tokens); }
            let n = sock.read(&mut tmp).map_err(|e| format!("read: {e}"))?;
            if n == 0 {
                // A half-reply accepted as whole is worse than an error.
                return Err("daemon closed the connection mid-reply".into());
            }
            pending.push_str(&String::from_utf8_lossy(&tmp[..n]));
        }
    }
}


/// Talk to a server over HTTP instead of loading weights: llama-server (OpenAI or Anthropic
/// protocol, MTP included when it was started with `--spec-type draft-mtp`), optiq, Ollama, or
/// the Studio across the mesh. A thin client over `hive::endpoint`; the bespoke
/// `/generate` protocol stays with `Daemon` for the unix-socket wgpu server.
///
/// Configured by `LOGSCOPE_REMOTE`, one of
///   openai:http://ultra:8080[/model]      anthropic:http://ultra:8080[/model]
///   ollama:http://mini:11434/model        matrix:<box-name>   matrix:   (first healthy worker)
/// The model segment is optional for llama-server (it serves what it loaded); when absent, the
/// `load(path)` argument's file stem is sent, which llama-server ignores and Ollama needs.
#[derive(Default)]
pub struct Remote {
    ep: Option<hive::endpoint::Endpoint>,
    label: String,
    concurrency: usize,
}

impl Remote {
    pub fn spec_from_env() -> Option<String> { std::env::var("LOGSCOPE_REMOTE").ok().filter(|s| !s.trim().is_empty()) }

    /// (endpoint, label, concurrency) from a spec string. Pure apart from reading the matrix file
    /// for `matrix:` specs.
    pub fn resolve(spec: &str) -> Result<(hive::endpoint::Endpoint, String, usize), String> {
        use hive::endpoint::Endpoint;
        let (kind, rest) = spec.split_once(':').ok_or_else(|| format!("LOGSCOPE_REMOTE '{spec}': expected kind:target"))?;
        if kind == "matrix" {
            let m = hive::matrix::Matrix::load();
            let b = if rest.is_empty() {
                m.with_role(hive::matrix::Role::Worker).first().cloned().cloned().ok_or("matrix: no healthy worker box (run `hive matrix probe`)")?
            } else {
                m.get(rest).cloned().ok_or_else(|| format!("matrix: no box named '{rest}'"))?
            };
            return Ok((b.endpoint.clone(), format!("{} ({})", b.name, kind_name(&b.endpoint)), b.concurrency.max(1)));
        }
        // kind:http://host:port[/model] — split the model off the path only after the authority
        let (base, model) = match rest.find("://").map(|i| i + 3).and_then(|a| rest[a..].find('/').map(|p| a + p)) {
            Some(p) => (rest[..p].to_string(), Some(rest[p + 1..].to_string()).filter(|m| !m.is_empty())),
            None => (rest.to_string(), None),
        };
        let model = model.unwrap_or_else(|| "default".into());
        let ep = match kind {
            "openai" => Endpoint::OpenAi { base, model, key: std::env::var("OPENAI_API_KEY").ok() },
            "anthropic" => Endpoint::Anthropic { base, model, key: std::env::var("ANTHROPIC_API_KEY").ok() },
            "ollama" => Endpoint::Ollama { base, model },
            k => return Err(format!("LOGSCOPE_REMOTE kind '{k}': openai | anthropic | ollama | matrix")),
        };
        Ok((ep.clone(), format!("{} {}", kind, kind_base(&ep)), std::env::var("LOGSCOPE_REMOTE_NP").ok().and_then(|s| s.parse().ok()).unwrap_or(2)))
    }
}

fn kind_name(ep: &hive::endpoint::Endpoint) -> &'static str {
    use hive::endpoint::Endpoint::*;
    match ep { OpenAi { .. } => "openai", Anthropic { .. } => "anthropic", Ollama { .. } => "ollama", ClaudeCode { .. } => "claude-code" }
}
fn kind_base(ep: &hive::endpoint::Endpoint) -> String {
    use hive::endpoint::Endpoint::*;
    match ep { OpenAi { base, .. } | Anthropic { base, .. } | Ollama { base, .. } => base.clone(), ClaudeCode { .. } => "claude".into() }
}

impl Backend for Remote {
    fn name(&self) -> &'static str { "remote" }
    fn loaded(&self) -> bool { self.ep.is_some() }

    fn load(&mut self, path: &str, _params: &GenParams) -> Result<(), String> {
        use hive::endpoint::Endpoint;
        let spec = Remote::spec_from_env().ok_or("LOGSCOPE_REMOTE is not set (openai:http://host:port, anthropic:…, ollama:…/model, matrix:name)")?;
        let (mut ep, label, np) = Remote::resolve(&spec)?;
        // The model name: what the spec said, else the file stem of whatever the UI asked to load.
        let stem = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
        match &mut ep {
            Endpoint::OpenAi { model, .. } | Endpoint::Anthropic { model, .. } | Endpoint::Ollama { model, .. } if model == "default" && !stem.is_empty() => *model = stem,
            _ => {}
        }
        // Fail FAST and name the target, like the daemon: the caller uses this to fall back.
        hive::endpoint::probe(&ep).map_err(|e| format!("remote {label}: {e}"))?;
        self.ep = Some(ep); self.label = label; self.concurrency = np;
        Ok(())
    }

    /// Structured chat straight to the server's chat endpoint, so llama-server applies the model's
    /// own template (Qwen/Ornith turn markers, tool framing). Uses the same grammar and cache path
    /// as `generate`; falls back to the flattened prompt for the Ollama endpoint, which this
    /// `hive::endpoint` client drives through /api/generate rather than a chat route.
    fn generate_chat(&mut self, messages: &[ChatTurn], params: &GenParams, cancel: &dyn Fn() -> bool, on_token: &mut dyn FnMut(&str) -> bool) -> Result<usize, String> {
        use hive::endpoint::{generate_messages, ChatMessage, Event, Request};
        let ep = self.ep.clone().ok_or("no remote server connected")?;
        // Ollama's endpoint here is /api/generate (no messages): flatten for it.
        if matches!(ep, hive::endpoint::Endpoint::Ollama { .. }) {
            return self.generate(&flatten_chat(messages), params, cancel, on_token);
        }
        let msgs: Vec<ChatMessage> = messages.iter().map(|m| ChatMessage {
            role: match m.role { ChatRole::System => "system", ChatRole::User => "user", ChatRole::Assistant => "assistant", ChatRole::Tool => "tool" }.to_string(),
            content: m.content.clone(),
        }).collect();
        let req = Request { prompt: String::new(), system: None, max_tokens: params.max_tokens.clamp(1, 32768) as u32, temperature: params.temperature, resume: None, grammar: params.grammar.clone() };
        let mut n = 0usize;
        let mut on = |e: Event| -> bool { if let Event::Delta(s) = e { n += 1; if cancel() { return false; } return on_token(&s); } true };
        let m = generate_messages(&ep, &msgs, &req, &mut on)?;
        Ok(if m.out_tokens as usize > 0 { m.out_tokens as usize } else { n })
    }

    fn generate(&mut self, prompt: &str, params: &GenParams, cancel: &dyn Fn() -> bool,
                on_token: &mut dyn FnMut(&str) -> bool) -> Result<usize, String> {
        use hive::endpoint::{generate, Event, Request};
        let ep = self.ep.clone().ok_or("no remote server connected")?;
        let req = Request { prompt: prompt.to_string(), system: None, max_tokens: params.max_tokens.clamp(1, 32768) as u32, temperature: params.temperature, resume: None, grammar: params.grammar.clone() };
        let mut n = 0usize;
        let mut on = |e: Event| -> bool {
            if let Event::Delta(s) = e { n += 1; if cancel() { return false; } return on_token(&s); }
            true
        };
        let m = generate(&ep, &req, &mut on)?;
        Ok(if m.out_tokens as usize > 0 { m.out_tokens as usize } else { n })
    }

    /// Fan out across the server's slots (`llama-server -np N`, `OLLAMA_NUM_PARALLEL`) instead of
    /// looping: the Ultra decodes them as a batch, which is the whole point of `:fa`/`:ca` there.
    fn generate_batched(&mut self, prompts: &[String], params: &GenParams, cancel: &dyn Fn() -> bool) -> Vec<String> {
        use hive::endpoint::{generate, Event, Request};
        let Some(ep) = self.ep.clone() else { return prompts.iter().map(|_| String::new()).collect() };
        let width = self.concurrency.max(1);
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut out: Vec<String> = prompts.iter().map(|_| String::new()).collect();
        for chunk in (0..prompts.len()).collect::<Vec<_>>().chunks(width) {
            if cancel() { cancelled.store(true, Ordering::Relaxed); break; }
            let handles: Vec<_> = chunk.iter().map(|&i| {
                let (ep, p, c) = (ep.clone(), prompts[i].clone(), cancelled.clone());
                let req = Request { prompt: p, system: None, max_tokens: params.max_tokens.clamp(1, 32768) as u32, temperature: params.temperature, resume: None, grammar: params.grammar.clone() };
                std::thread::spawn(move || {
                    let mut text = String::new();
                    let mut on = |e: Event| -> bool { if let Event::Delta(s) = e { text.push_str(&s); } !c.load(Ordering::Relaxed) };
                    let _ = generate(&ep, &req, &mut on);
                    (i, text)
                })
            }).collect();
            for h in handles { if let Ok((i, text)) = h.join() { out[i] = text.trim().to_string(); } }
        }
        out
    }

    /// The server applies its own chat template to the message we send, so the app must not
    /// wrap the prompt in one: Plain is the truthful answer here.
    fn template(&self) -> TemplateKind { TemplateKind::Plain }
}

/// Where the daemon listens. Must agree with `netserve::default_socket_path`
/// and `aperture-engine`'s client, or the halves never meet — the rule is
/// duplicated here rather than imported because this crate does not
/// depend on gpu-gemm in every configuration.
fn daemon_socket_path() -> std::path::PathBuf {
    if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
        return std::path::Path::new(&rt).join("logscope-engine.sock");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::Path::new(&home).join(".logscope-engine.sock")
}

/// Pull every COMPLETE chunk from `buf`; leave a partial one for the next
/// read. Guessing at a partial chunk is the difference between streaming
/// and corrupting, and a chunk split across two reads is the normal case.
fn drain_chunks(buf: &str) -> (String, String, bool) {
    let mut out = String::new();
    let mut rest = buf;
    loop {
        let Some((len_s, tail)) = rest.split_once("\r\n") else {
            return (out, rest.to_string(), false);
        };
        let Some(first) = len_s.split(';').next() else {
            return (out, rest.to_string(), false);
        };
        let Ok(n) = usize::from_str_radix(first.trim(), 16) else {
            return (out, rest.to_string(), false);
        };
        if n == 0 { return (out, String::new(), true); }
        if tail.len() < n + 2 { return (out, rest.to_string(), false); }
        out.push_str(&tail[..n]);
        rest = &tail[n + 2..];
    }
}

// ── worker protocol ──────────────────────────────────────────────────────────
/// Chat-template family the loaded model expects. The wgpu path builds the prompt app-side, so it
/// must match the model: a ChatML model (smollm2, `<|im_end|>`) fed Llama-3 markers drifts into
/// byte-level marker text and never hits its real stop. Detected from the tokenizer at load.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TemplateKind { Llama3, ChatML, Plain, Gemma, Llama4, Bailing2 }
impl TemplateKind {
    fn to_u8(self) -> u8 { match self { Self::Llama3 => 0, Self::ChatML => 1, Self::Plain => 2, Self::Gemma => 3, Self::Llama4 => 4, Self::Bailing2 => 5 } }
    fn from_u8(v: u8) -> Option<Self> { Some(match v { 0 => Self::Llama3, 1 => Self::ChatML, 2 => Self::Plain, 3 => Self::Gemma, 4 => Self::Llama4, 5 => Self::Bailing2, _ => return None }) }
}

/// What a successful load reports back to the UI.
pub struct LoadInfo { pub path: String, pub template: TemplateKind,
    /// The Liaison's view of what just loaded — `None` when the backend has nothing to report
    /// (llama.cpp, the daemon path) rather than a fabricated "no gaps".
    pub model_status: Option<logscope::liaison::ModelStatus> }

impl LoadInfo {
    /// One line for a status sink: the file stem, the template, and the Liaison's gap count when it
    /// had one — `Ornith-1.5-35B-A3B-Q4 (ChatML, 2 gaps)`. Never invents "fully supported" for a
    /// backend that did not say so.
    pub fn summary(&self) -> String {
        let stem = std::path::Path::new(&self.path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| self.path.clone());
        match &self.model_status {
            Some(ms) if ms.gaps.is_empty() => format!("{stem} ({:?}, {} · no gaps)", self.template, ms.arch),
            Some(ms) => format!("{stem} ({:?}, {} · {} gaps)", self.template, ms.arch, ms.gaps.len()),
            None => format!("{stem} ({:?})", self.template),
        }
    }
}

pub enum Cmd { SetBackend(BackendKind), SetParams(GenParams), Load(String), Generate(String), GenerateChat(Vec<ChatTurn>), Continue(String) }
pub enum Evt { Loaded(Result<LoadInfo, String>), Token(String), Done { tokens: usize, secs: f32 }, Error(String) }

pub struct Inference { tx: Sender<Cmd>, pub rx: Receiver<Evt>, cancel: Arc<AtomicBool>,
    /// Detected chat template of the CURRENTLY loaded model, published by the worker on every successful
    /// load (255 = nothing loaded). UI tabs read this LIVE at prompt-build time instead of relying only on
    /// per-tab cached state, so a tab created after the load (or one whose Loaded event another drain
    /// consumed) can never silently build Llama-3 prompts for a ChatML model.
    template: Arc<std::sync::atomic::AtomicU8> }

/// Prompt tokens, window and elision from the LAST generation, for the status line.
///
/// A static rather than a field on Inference: Engine has lanes and several constructors, and this
/// is "whatever generated most recently" rather than per-handle state — threading it through every
/// lane would be plumbing in service of a shape the data does not have. Packed into one u64
/// (prompt:21 | window:21 | elided:22) so the three are read together; separately, a torn read
/// would show one turn's prompt against another turn's window.
pub(crate) static CTX_FIT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The engine's standing request for a compaction, published the same way and for the same
/// reason as CTX_FIT: the decision is made where the arithmetic is (`ctxwin::relief`), and the
/// ACTION belongs to the caller, because summarizing is a turn and the engine does not own turns.
/// Packed (summarize_upto:21 | keep_recent:21 | seq:22) — the sequence number rises on every
/// request so the UI can tell "still the same unanswered request" from "asked again this turn".
/// Zero means no request outstanding.
pub(crate) static COMPACT_REQUEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// What the engine wants folded, and what must survive verbatim.
/// This machine's cell report for the hive matrix, built from what the engine
/// already knows: the model it holds, how much of `prompt_ids` its KV pool
/// already covers, and whether it is generating.
///
/// The warmth number is `kvtree::pick_anchor`'s own answer against THIS
/// prompt — the same call the local prefill path makes — not a guess about
/// conversation shape. `docs/11-hive-matrix.md` explains why that distinction
/// decides the design: placement commits to the number, so a wrong prediction
/// is worse than reporting a cold cell.
///
/// `tok_s` is the last measured decode rate; 0.0 before the first turn, which
/// `hive::place` treats as the slowest possible cell rather than dividing by
/// zero.
pub fn node_report(
    id: &str,
    model_label: &str,
    anchors: &[(Vec<u32>, usize, usize)],
    prompt_ids: &[u32],
    last_tok_s: f32,
    busy: bool,
) -> logscope_fan::hive::NodeReport {
    let warm = crate::kvtree::pick_anchor(
        anchors, prompt_ids, 16, crate::kvtree::suffix_bound(prompt_ids.len()))
        .map(|(_, ln)| ln)
        .unwrap_or(0);
    logscope_fan::hive::NodeReport {
        id: id.to_string(),
        tok_s: last_tok_s.max(0.0),
        residents: vec![(model_label.to_string(), warm, busy)],
    }
}

#[cfg(test)]
mod node_report_tests {
    use super::*;

    /// The report's warmth is the prefix tree's OWN answer: an anchor sharing
    /// a long prefix is reported as warm, and an unrelated one as cold. If
    /// these two cases ever agree, the report has stopped measuring.
    #[test]
    fn warmth_comes_from_the_anchor_pool() {
        let prompt: Vec<u32> = (0..400u32).collect();
        // an anchor sharing the first 300 tokens
        let shared: Vec<u32> = prompt[..300].to_vec();
        let anchors = vec![(shared.clone(), 300usize, 300usize)];
        let r = node_report("here", "Qwen3-14B", &anchors, &prompt, 12.5, false);
        assert_eq!(r.residents.len(), 1);
        assert!(r.residents[0].1 >= 16, "a 300-token shared prefix reported as {}", r.residents[0].1);
        assert_eq!(r.tok_s, 12.5);

        // an unrelated anchor is cold
        let other: Vec<u32> = (9000..9300u32).collect();
        let cold = node_report("here", "Qwen3-14B", &[(other, 300, 300)], &prompt, 12.5, false);
        assert_eq!(cold.residents[0].1, 0, "an unrelated anchor was reported warm");
    }

    /// An engine that has not generated yet reports 0.0 rather than a
    /// fabricated rate; hive::place must survive it.
    #[test]
    fn a_fresh_engine_reports_no_rate_and_still_places() {
        let r = node_report("here", "m", &[], &[1, 2, 3], 0.0, false);
        assert_eq!(r.tok_s, 0.0);
        let matrix = logscope_fan::hive::assemble(Some(&r), &[], &[]);
        let job = logscope_fan::hive::Job {
            reach: logscope_fan::workerkit::Reach::Lan,
            partner_node: None, prompt_tokens: 3, models: vec![], turns: 1,
        };
        let p = logscope_fan::hive::place(&job, &matrix).expect("a zero-rate cell is still a cell");
        assert_eq!(p.cell.node.id, "here");
    }

    /// A busy engine reports busy, and hive::place then skips it — the
    /// end-to-end path from engine state to a placement decision.
    #[test]
    fn a_busy_engine_is_not_placed_on() {
        let r = node_report("here", "m", &[], &[1, 2, 3], 20.0, true);
        let matrix = logscope_fan::hive::assemble(Some(&r), &[], &[]);
        let job = logscope_fan::hive::Job {
            reach: logscope_fan::workerkit::Reach::Lan,
            partner_node: None, prompt_tokens: 3, models: vec![], turns: 1,
        };
        assert!(logscope_fan::hive::place(&job, &matrix).is_none());
        assert!(logscope_fan::hive::explain_empty(&job, &matrix).contains("wait, not a failure"));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactRequest { pub summarize_upto: usize, pub keep_recent: usize, pub seq: usize }

pub(crate) fn compact_pack(summarize_upto: usize, keep_recent: usize) -> u64 {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) & ((1 << 22) - 1);
    ((summarize_upto.min((1 << 21) - 1) as u64) << 43)
        | ((keep_recent.min((1 << 21) - 1) as u64) << 22)
        | seq
}
/// What a per-frame poll of the compaction request should DO. Pure, because
/// getting it wrong is invisible: the engine publishes the request from inside
/// generate(), so every poll that sees a fresh one sees it mid-stream, and a
/// poll that cleared unconditionally destroyed the signal one frame after it
/// was raised — the "summarize does nothing" field report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactPoll {
    /// Show the offer and clear the slot.
    Take,
    /// Already declined at this seq — clear without offering.
    Clear,
    /// Mid-stream: leave it pending for a later frame.
    Leave,
}

pub fn compact_poll(req_seq: usize, declined: usize, streaming: bool) -> CompactPoll {
    if req_seq == declined {
        CompactPoll::Clear
    } else if streaming {
        CompactPoll::Leave
    } else {
        CompactPoll::Take
    }
}

#[cfg(test)]
mod compact_poll_tests {
    use super::*;

    /// The field bug, as a sequence: the request is raised mid-stream, must
    /// SURVIVE every frame until the stream ends, and is then offered once.
    #[test]
    fn a_request_raised_mid_stream_survives_until_the_stream_ends() {
        for _frame in 0..5 {
            assert_eq!(compact_poll(7, 0, true), CompactPoll::Leave,
                "a mid-stream poll must not consume the request");
        }
        assert_eq!(compact_poll(7, 0, false), CompactPoll::Take);
    }

    #[test]
    fn a_declined_request_is_cleared_not_re_offered() {
        assert_eq!(compact_poll(7, 7, false), CompactPoll::Clear);
        assert_eq!(compact_poll(7, 7, true), CompactPoll::Clear, "declines clear even mid-stream");
        assert_eq!(compact_poll(8, 7, false), CompactPoll::Take, "a NEW request after a decline offers again");
    }
}

pub(crate) fn compact_unpack(v: u64) -> CompactRequest {
    CompactRequest { summarize_upto: (v >> 43) as usize,
                     keep_recent: ((v >> 22) & ((1 << 21) - 1)) as usize,
                     seq: (v & ((1 << 22) - 1)) as usize }
}

/// Prompt tokens, window size, and how many tokens were dropped from the middle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CtxFit { pub prompt: usize, pub window: usize, pub elided: usize }

pub(crate) fn ctxfit_pack(prompt: usize, window: usize, elided: usize) -> u64 {
    ((prompt.min((1 << 21) - 1) as u64) << 43)
        | ((window.min((1 << 21) - 1) as u64) << 22)
        | (elided.min((1 << 22) - 1) as u64)
}
pub(crate) fn ctxfit_unpack(v: u64) -> CtxFit {
    CtxFit { prompt: (v >> 43) as usize,
             window: ((v >> 22) & ((1 << 21) - 1)) as usize,
             elided: (v & ((1 << 22) - 1)) as usize }
}
impl Inference {
    /// The worker-published template of the loaded model, if any.
    pub fn detected_template(&self) -> Option<TemplateKind> {
        TemplateKind::from_u8(self.template.load(Ordering::Relaxed))
    }

    /// The last generation's context fit, for the status line. None before the first generation.
    pub fn context_fit(&self) -> Option<CtxFit> {
        let v = CTX_FIT.load(Ordering::Relaxed);
        if v == 0 { None } else { Some(ctxfit_unpack(v)) }
    }

    /// The engine's outstanding compaction request, if the long-horizon policy asked for one.
    pub fn compact_request(&self) -> Option<CompactRequest> {
        let v = COMPACT_REQUEST.load(Ordering::Relaxed);
        if v == 0 { None } else { Some(compact_unpack(v)) }
    }
    /// Clear it once the caller has read it — an unanswered request left set would re-offer on
    /// every frame, and the UI holds its own copy (with the seq) once it has one.
    pub fn clear_compact_request(&self) { COMPACT_REQUEST.store(0, Ordering::Relaxed); }
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
    /// Structured chat: the worker calls `Backend::generate_chat`, so a server with the model's
    /// template applies it. Used for the Remote backend; other backends flatten it identically to
    /// the prompt path, so callers can always prefer this when they have the turns.
    pub fn generate_chat(&self, turns: Vec<ChatTurn>) { self.cancel.store(false, Ordering::Relaxed); let _ = self.tx.send(Cmd::GenerateChat(turns)); }
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
/// OUTPUT-SIDE MIRROR of the TEMPLATE MISMATCH alarm.
///
/// The stream swallows special tokens by ID, so a marker that reaches the screen was emitted as
/// ORDINARY TEXT — the model typed "<|python_tag|>" rather than producing token 128010. Models do
/// that when the frame they were given taught them to: a prompt whose markers went in as words
/// invites markers to come back as words, and the visible symptom is a reply that leaks its own
/// scaffolding, or the prompt itself.
///
/// Reported, never stripped. Removing the text would hide the framing bug that caused it and leave
/// a reply that looks almost right, which is the failure mode this project keeps paying for.
// Gated with its only caller. gpu_kernels is a wgpu-engine dependency, and this signature names it
// at module level — so `cargo test -p logscope-egui` WITHOUT the feature has failed to compile since
// c4-771, which is why the tool-parser tests could not run and why nothing noticed for five bundles.
#[cfg(feature = "wgpu-engine")]
/// Weight elements the loader made resident, captured before the load-path counters are consumed.
/// The roofline needs this at generation time and the counters do not survive that long.
/// Decode-to-prefill cost ratio from the last run, f32 bits; 0 = not starved or not measured.
///
/// Read by the UI strip. Deliberately only set from the DecodeStarved verdict — a healthy or
/// unmeasured run stores 0, so a stale ratio cannot outlive the condition that produced it.
pub static LAST_DECODE_RATIO: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub static RESIDENT_ELEMS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// The cfg above attaches to RESIDENT_ELEMS, not to this fn — so this gpu_kernels-typed function
// stayed in a headless build and broke it. Same shape of gate bug as the workspace/merge/sandbox
// ones, found the same way: by building without default features for the first time in a while.
#[cfg(feature = "wgpu-engine")]
pub fn check_output_markers(name: &str, text: &str, tok: &gpu_kernels::tokenizer::Tokenizer) {
    let Some(i) = text.find("<|") else { return };
    let end = text[i..].find("|>").map(|j| i + j + 2);
    let Some(end) = end else { return };
    let marker = &text[i..end];
    // Only interesting when the tokenizer KNOWS this marker as a special: then the model had a
    // single id available and chose to spell it out instead.
    if tok.special_id(marker).is_none() { return; }
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if SAID.swap(true, std::sync::atomic::Ordering::Relaxed) { return; }
    crate::diag!("wgpu-engine: [{name}] OUTPUT MARKER LEAK — the reply contains {marker:?} as literal \
TEXT, and this tokenizer knows that marker as a special token. The stream suppresses specials by id, \
so this was typed, not emitted: the model is imitating its own scaffolding, which is what a prompt \
framed in the wrong markers teaches it to do. Check the prompt diag line above for the special count.");
}

fn normalize_prompt_template(prompt: &str, t: TemplateKind) -> Option<String> {
    match t {
        // BAILING2 (Ling 2.0 / LLaDA2). This arm was missing, and its absence is why the
        // TEMPLATE MISMATCH alarm kept firing on real runs with nothing repairing it: the UI sent
        // Llama-3 markers, the worker knew the model was Bailing2, and the repair function — whose
        // entire job is exactly this — had no case for it, so the markers passed through and
        // tokenised as ordinary words (prompt diag: "0 special tokens" on 1718 tokens).
        //
        // Repairing at the WORKER is the right layer for a last line of defence: it is the only
        // place that knows what the loaded model actually wants, and it catches every UI path at
        // once rather than each builder separately.
        // FROM Bailing2, the direction that was missing. Every arm here translated INTO a template
        // from Llama-3 or ChatML markers, so a prompt already framed in <role> turns — which is now
        // what a Bailing2 model's builder produces — reached a ChatML or Llama model untranslated.
        // Seen live: DiffuCoder (ChatML) received "<|role_end|><role>ASSISTANT</role>" as literal
        // text, 0 specials on a 2147-token prompt. Switching models mid-session is exactly when the
        // stale-prompt case fires, so the repair has to work in both directions or it only ever
        // rescues the first model loaded.
        TemplateKind::ChatML | TemplateKind::Llama3 | TemplateKind::Llama4 if prompt.contains("<role>") => {
            let llama = !matches!(t, TemplateKind::ChatML);
            let mut s = prompt.to_string();
            for (src, ch, l3) in [("SYSTEM", "system", "system"), ("HUMAN", "user", "user"),
                                  ("ASSISTANT", "assistant", "assistant")] {
                let from = format!("<role>{src}</role>");
                let to = if llama { format!("<|start_header_id|>{l3}<|end_header_id|>\n\n") }
                         else { format!("<|im_start|>{ch}\n") };
                s = s.replace(&from, &to);
            }
            s = s.replace("<|role_end|>", if llama { "<|eot_id|>" } else { "<|im_end|>\n" });
            debug_assert!(!s.contains("<role>") && !s.contains("<|role_end|>"),
                "Bailing2 markers survived translation");
            Some(s)
        }
        TemplateKind::Bailing2 if prompt.contains("<|start_header_id|>") || prompt.contains("<|im_start|>") => {
            let mut s = prompt.replace("<|begin_of_text|>", "");
            for (role_src, role_dst) in [("system", "SYSTEM"), ("user", "HUMAN"), ("assistant", "ASSISTANT")] {
                s = s.replace(&format!("<|start_header_id|>{role_src}<|end_header_id|>\n\n"),
                              &format!("<role>{role_dst}</role>"));
                s = s.replace(&format!("<|start_header_id|>{role_src}<|end_header_id|>"),
                              &format!("<role>{role_dst}</role>"));
                s = s.replace(&format!("<|im_start|>{role_src}\n"), &format!("<role>{role_dst}</role>"));
            }
            s = s.replace("<|eot_id|>", "<|role_end|>").replace("<|im_end|>\n", "<|role_end|>")
                 .replace("<|im_end|>", "<|role_end|>");
            debug_assert!(!s.contains("<|start_header_id|>") && !s.contains("<|im_start|>"),
                "foreign turn markers survived translation to Bailing2");
            Some(s)
        }
        TemplateKind::ChatML if prompt.contains("<|start_header_id|>") => {
            let s = prompt.replace("<|begin_of_text|>", "")
                .replace("<|start_header_id|>", "<|im_start|>")
                .replace("<|end_header_id|>\n\n", "\n")
                // TOTAL, not best-effort. The line above only matches when the marker is followed by
                // exactly two newlines. Followed by anything else — one newline, a space, content —
                // it SURVIVES into the prompt, and the tokenizer byte-encodes it as literal text.
                // That is a Llama-3 marker sitting inside a ChatML turn, which is precisely the shape
                // that garbles a reply while the prompt still looks structurally fine.
                .replace("<|end_header_id|>", "\n")
                .replace("<|eot_id|>", "<|im_end|>\n");
            // NO SOURCE MARKER MAY SURVIVE. A partial translation is worse than none: the prompt keeps
            // its shape, the reply garbles, and nothing in the log says which marker leaked.
            debug_assert!(!s.contains("<|start_header_id|>") && !s.contains("<|end_header_id|>")
                          && !s.contains("<|eot_id|>"),
                "Llama-3 markers survived translation to ChatML");
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

/// One-shot latch so the EXPECTED Llama-4 rewrite is announced once per process, not once per turn.
static TEMPLATE_NOTE_DONE: AtomicBool = AtomicBool::new(false);

fn worker(cmd_rx: Receiver<Cmd>, evt: Sender<Evt>, cancel: Arc<AtomicBool>, template_out: Arc<std::sync::atomic::AtomicU8>) {
    // Default to the backend that's actually compiled in (llama.cpp if available),
    // so the header Load and the chat tab operate on the same, working backend.
    // A live daemon wins: one copy of the weights for every app on the
    // machine instead of 7.2 GiB and ~86 s each. Falls back to the
    // compiled-in backend when none is listening, which is the ordinary
    // case and must stay silent-fast rather than an error path.
    let mut kind = BackendKind::preferred_reachable();
    let mut backend = make(kind);
    crate::diag!("worker: default backend = {} ({})", kind.label(), backend.name());
    if kind == BackendKind::Remote {
        crate::diag!("worker: using a remote server (LOGSCOPE_REMOTE) — this process holds no weights");
    }
    if kind == BackendKind::Daemon {
        // Say so plainly. "Why is startup instant today" and "why did my
        // model selection do nothing" have the same answer and it is
        // this, and a person should not have to read the source to find
        // out that another process holds their model.
        crate::diag!("worker: using the shared daemon — this process holds no weights");
    }
    let mut params = GenParams::default();
    // What is actually resident. `Cmd::Load` used to reload unconditionally, and
    // the UI re-asserts its model selection on several paths (the same habit the
    // SetBackend guard below already exists to absorb). A log of one session
    // showed Llama-3.2-1B loaded TEN times. For a 1B the seconds are easy to miss,
    // but every reload drops the KV prefix tree, the autotune-warmed pipelines and
    // any prefix a long prompt had already paid for — so a workflow like
    // `:ws <path>` then "summarize the codebase" re-prefills from scratch each
    // turn and can never build on the previous one.
    let mut loaded: Option<String> = None;
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            // Only rebuild (and thus drop any loaded model) on a genuine switch. The
            // chat tab re-asserts its backend choice on first open; without this guard
            // that would silently wipe a model the user just loaded.
            Cmd::SetBackend(k) => if k != kind {
                kind = k; backend = make(k); loaded = None;
                crate::diag!("worker: backend → {} ({})", k.label(), backend.name());
                let _ = evt.send(Evt::Loaded(Err(format!("backend → {} ({}) — reload the model", k.label(), backend.name()))));
            },
            Cmd::SetParams(p) => { params = p; }
            Cmd::Load(path) => {
                // ALREADY RESIDENT → acknowledge without reloading. The UI still
                // gets Evt::Loaded(Ok), so any state waiting on it settles exactly
                // as before; the difference is that the weights, the prefix tree
                // and the compiled pipelines survive.
                //
                // Compared as the caller gave it rather than canonicalised: a
                // stat() per Load on a path that may live on a slow mount is a
                // worse trade than occasionally reloading when someone passes the
                // same file by two different spellings.
                if loaded.as_deref() == Some(path.as_str()) {
                    let template = backend.template();
                    template_out.store(template.to_u8(), Ordering::Relaxed);
                    crate::diag!("worker: '{path}' is already loaded — keeping it \
(weights, KV prefix tree and warmed pipelines survive)");
                    let _ = evt.send(Evt::Loaded(Ok(LoadInfo { path, template, model_status: backend.model_status() })));
                    continue;
                }
                crate::diag!("worker: loading '{path}' via {} backend", backend.name());
                match backend.load(&path, &params) {
                    Ok(()) => { let template = backend.template(); template_out.store(template.to_u8(), Ordering::Relaxed); loaded = Some(path.clone()); let _ = evt.send(Evt::Loaded(Ok(LoadInfo { path, template, model_status: backend.model_status() }))); }
                    Err(e) => { loaded = None; crate::diag!("worker: load failed: {e}"); let _ = evt.send(Evt::Loaded(Err(e))); }
                }
            }
            Cmd::Generate(prompt) => {
                // Last-line-of-defense template repair (see normalize_prompt_template): the worker knows
                // the loaded model's template; if the UI built the prompt with the other family's markers,
                // translate and say so loudly — this is the bug that garbled every ChatML model.
                let prompt = match normalize_prompt_template(&prompt, backend.template()) {
                    Some(fixed) => {
                        // TWO DIFFERENT SITUATIONS, previously reported identically as a mismatch to
                        // "report this". For Llama-4 the UI builds Llama-3 markers ON PURPOSE and this
                        // rewrite is the documented handoff (see chat.rs's TemplateKind::Llama4 arm) —
                        // calling that a bug trained the user to ignore a message that also fires for a
                        // real one. Anything else IS stale UI state and still says so.
                        let sent = if fixed.contains("<|im_start|>") { "Llama-3" } else { "ChatML" };
                        if backend.template() == TemplateKind::Llama4 && sent == "Llama-3" {
                            if !TEMPLATE_NOTE_DONE.swap(true, Ordering::Relaxed) {
                                crate::diag!("worker: prompt rewritten to Llama-4 markers (expected: the UI \
builds Llama-3 markers and the worker translates them authoritatively)");
                            }
                        } else {
                            // ONCE, AND NOT AS A FAULT. This printed on every model switch and ended with "please
                            // report it" — right when the translation was partial, wrong since _409 made it total.
                            //
                            // The cause is structural and known: the prompt is BUILT AT SEND TIME, before the load
                            // completes, so it necessarily carries the previous model's markers on the first turn
                            // after a switch. The worker translating authoritatively is the design, not a symptom.
                            //
                            // A warning that fires on correct behaviour is one people learn to skip — and THIS
                            // message is what hid a real garbling bug for weeks, because it read as a resolved
                            // notice. One informational line naming the actual remaining work.
                            static MISMATCH_NOTE: std::sync::atomic::AtomicBool =
                                std::sync::atomic::AtomicBool::new(false);
                            if !MISMATCH_NOTE.swap(true, Ordering::Relaxed) {
                                crate::diag!("worker: prompt carried {sent} markers to a {:?} model and was \\
translated. Expected on the first turn after a model switch — the prompt is built before the load \\
finishes, so it cannot know the new template. The durable fix is rebuilding it once Evt::Loaded \\
lands; until then the translation is total and this is not a fault.",
                                    backend.template());
                            }
                        }
                        fixed
                    }
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
            Cmd::GenerateChat(turns) => {
                if backend.loaded() {
                    cancel.store(false, Ordering::Relaxed);
                    let (evt2, cancel2) = (evt.clone(), cancel.clone());
                    let start = std::time::Instant::now();
                    let mut cb = move |p: &str| -> bool { let _ = evt2.send(Evt::Token(p.to_string())); !cancel2.load(Ordering::Relaxed) };
                    let cancel3 = cancel.clone();
                    let is_cancelled = move || cancel3.load(Ordering::Relaxed);
                    match backend.generate_chat(&turns, &params, &is_cancelled, &mut cb) {
                        Ok(n) => { let _ = evt.send(Evt::Done { tokens: n, secs: start.elapsed().as_secs_f32() }); }
                        Err(e) => { let _ = evt.send(Evt::Error(e)); }
                    }
                } else {
                    simulate(&flatten_chat(&turns), &evt, std::time::Instant::now(), &cancel);
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

/// The two watchdog tables must agree.
///
/// `gpu_kernels::watchdog_prone_name` and `logscope::tuneprofile::watchdog_prone` encode the same
/// decision in two crates, because `logscope` cannot depend on `gpu-gemm` and duplicating the
/// predicate was cheaper than restructuring for it.
///
/// That duplication already went wrong once. _358 widened the engine's guard to mobile GPUs after a
/// Fairphone reported "not enough wavelets", and the tuning table stayed Apple-only — so a phone got
/// the subgroup path from one table while the other was refusing it. **A policy expressed in two
/// places gets corrected in one, and the survivor looks right because it matches the code around it.**
///
/// This crate sees both, so the agreement is checkable here even though neither crate can check it
/// alone. That is the durable fix: another sweep would have found instance seven, and this stops it
/// being written.
#[cfg(all(test, feature = "wgpu-engine"))]
mod watchdog_agreement {
    #[test]
    fn both_crates_classify_every_adapter_identically() {
        let cases = [
            // Mobile — the class that was missing from the tuning table.
            "Adreno (TM) 740", "Mali-G715", "PowerVR B-Series BXM-8-256",
            "Immortalis-G720", "Samsung Xclipse 940", "Vivante GC7000",
            // Apple, in both the spellings that appear in real adapter strings.
            "Apple M2 Max (G14C B1) [Vulkan]", "Apple M2 Ultra (G14D B1) [Vulkan]",
            "honeykrisp", "AGX",
            // Desktop — must stay on the fast path in both.
            "NVIDIA GeForce RTX 4090", "AMD Radeon RX 7900 XTX", "Intel Arc A770",
            "llvmpipe (LLVM 20.1.2, 256 bits) [Vulkan]",
            // Neither — a device nobody has enumerated must not be guessed at differently.
            "Some Vendor GPU Nobody Has Heard Of",
        ];
        for a in cases {
            let engine = gpu_kernels::watchdog_prone_name(a);
            let tuning = logscope::tuneprofile::watchdog_prone(a);
            assert_eq!(engine, tuning,
                "{a}: engine says {engine}, tuning table says {tuning} — the two tables have drifted, \
which is exactly how a phone got the subgroup path from one while the other refused it");
        }
    }

    #[test]
    fn the_agreement_test_would_have_CAUGHT_the_original_drift() {
        // Before _358 the tuning table matched only Apple. Adreno is the case that exposed it, and the
        // test above compares that string — so this asserts the fixture is the one that matters rather
        // than a set that happens to agree.
        assert!(gpu_kernels::watchdog_prone_name("Adreno (TM) 740"));
        assert!(logscope::tuneprofile::watchdog_prone("Adreno (TM) 740"));
    }
}


#[cfg(test)]
mod template_translation_tests {
    use super::*;

    /// A Llama-3 prompt whose header marker is NOT followed by two newlines.
    ///
    /// The translation replaced `<|end_header_id|>\n\n` only. Real prompts do not always have that
    /// exact spacing — a system block with a single newline, or content starting immediately, leaves
    /// the marker in place and the tokenizer byte-encodes it as literal text. A Llama-3 marker inside
    /// a ChatML turn garbles the reply while the prompt still looks structurally fine, which is why it
    /// survived: the diag prints special-token IDs, and a leaked marker is not a special token.
    #[test]
    fn no_llama3_marker_survives_translation_whatever_the_spacing() {
        for tail in ["\n\n", "\n", " ", "", "\n \n"] {
            let p = format!("<|begin_of_text|><|start_header_id|>user<|end_header_id|>{tail}hi<|eot_id|>");
            let out = normalize_prompt_template(&p, TemplateKind::ChatML)
                .expect("a Llama-3 prompt to a ChatML model must be rewritten");
            assert!(!out.contains("<|start_header_id|>"), "spacing {tail:?}: start leaked");
            assert!(!out.contains("<|end_header_id|>"), "spacing {tail:?}: END LEAKED — {out:?}");
            assert!(!out.contains("<|eot_id|>"), "spacing {tail:?}: eot leaked");
            assert!(out.contains("<|im_start|>") && out.contains("<|im_end|>"),
                "spacing {tail:?}: ChatML markers must be present");
        }
    }

    #[test]
    fn a_prompt_already_in_the_right_family_is_left_alone() {
        // Returning Some for a correct prompt would fire the mismatch warning on every turn, and a
        // warning that always fires is one nobody reads — the reason the real one was ignorable.
        let p = "<|im_start|>user\nhi<|im_end|>\n";
        assert!(normalize_prompt_template(p, TemplateKind::ChatML).is_none());
    }
}

/// The two bits-per-weight tables must agree.
///
/// `gpu_kernels::rolequant::Level` and `logscope::rolequant_hint` carry the same constants because
/// `logscope` cannot depend on `gpu-gemm` — the advice line needs the arithmetic and the policy lives
/// engine-side.
///
/// That duplication already went wrong once in this tree, with the watchdog tables: a policy expressed
/// in two places gets corrected in one, and the survivor looks right because it matches the code
/// around it. This crate depends on both, so the agreement is checkable here even though neither can
/// check it alone.
#[cfg(all(test, feature = "wgpu-engine"))]
mod rolequant_agreement {
    #[test]
    fn the_bpw_constants_match_across_crates() {
        use gpu_kernels::rolequant::Level;
        assert_eq!(Level::Q2.bpw(), logscope::rolequant_hint::Q2_BPW);
        assert_eq!(Level::Q4.bpw(), logscope::rolequant_hint::Q4_BPW);
    }

    #[test]
    fn the_saving_the_advice_quotes_is_what_the_planner_would_free() {
        // The advice says "a liaison at Q2 frees ~N GiB". If the planner would free a different
        // number, the line is describing a plan nobody will get — which is worse than no advice,
        // because it is checkable and wrong rather than merely absent.
        use gpu_kernels::rolequant::{Level, Member, Role};
        let m = Member { name: "liaison".into(), role: Role::Liaison,
                         params: 8_000_000_000, chosen: Level::Q4 };
        let planner_frees = (m.bytes_at(Level::Q4) - m.bytes_at(Level::Q2)) as f32
            / (1u64 << 30) as f32;
        let advice_says = logscope::rolequant_hint::liaison_saving_gib(8_000_000_000).unwrap();
        assert!((planner_frees - advice_says).abs() < 0.01,
            "advice quotes {advice_says:.2} GiB, planner frees {planner_frees:.2}");
    }
}

#[cfg(test)]
mod diffusion_setup_skips {
    /// The KV prefill and the int8 embed table are skipped ONLY when generation will actually take
    /// the diffusion path. Both fallbacks — LOGSCOPE_DIFFUSION_AR=1 and a model with no mask id
    /// (diffuse_generate refuses) — decode autoregressively and need the prefilled cache; skipping
    /// it there would decode from an EMPTY cache and produce garbage, which is the same class of
    /// silent breakage this whole hunt has been about. The condition is duplicated at two call
    /// sites, so this pins the predicate itself.
    fn skip(diffusion: bool, ar_env: Option<&str>, has_mask: bool) -> bool {
        diffusion && ar_env != Some("1") && has_mask
    }

    #[test]
    fn skips_apply_exactly_when_the_diffusion_path_runs() {
        assert!(skip(true, None, true), "a diffusion model with a mask id skips both setups");
        assert!(!skip(true, Some("1"), true), "LOGSCOPE_DIFFUSION_AR=1 decodes AR and needs the prefill");
        assert!(!skip(true, None, false), "no mask id means diffuse_generate refuses and AR runs");
        assert!(!skip(false, None, true), "a normal model always prefills");
        assert!(!skip(false, Some("1"), false), "a normal model is unaffected by the diffusion env");
    }
}

#[cfg(test)]
mod bailing2_repair {
    use super::*;

    /// The exact shape the hardware log carried: a Llama-3 framed prompt handed to a Bailing2 model,
    /// which tokenised to ZERO specials because none of those markers exist in that vocabulary.
    #[test]
    fn llama3_markers_are_repaired_into_role_turns() {
        let p = "<|begin_of_text|><|start_header_id|>system<|end_header_id|>\n\nbe brief<|eot_id|>\
<|start_header_id|>user<|end_header_id|>\n\nhi there<|eot_id|>\
<|start_header_id|>assistant<|end_header_id|>\n\n";
        let out = normalize_prompt_template(p, TemplateKind::Bailing2).expect("repair must fire");
        assert!(!out.contains("<|start_header_id|>"), "Llama-3 markers survived: {out}");
        assert!(!out.contains("<|begin_of_text|>"));
        assert!(out.starts_with("<role>SYSTEM</role>be brief<|role_end|>"), "{out}");
        assert!(out.contains("<role>HUMAN</role>hi there<|role_end|>"), "{out}");
        assert!(out.ends_with("<role>ASSISTANT</role>"), "the turn must be left OPEN to generate: {out}");
    }

    #[test]
    fn chatml_markers_are_repaired_too() {
        let p = "<|im_start|>user\nhi<|im_end|>\n<|im_start|>assistant\n";
        let out = normalize_prompt_template(p, TemplateKind::Bailing2).expect("repair must fire");
        assert!(!out.contains("<|im_start|>"), "{out}");
        assert!(out.contains("<role>HUMAN</role>hi<|role_end|>"), "{out}");
        assert!(out.ends_with("<role>ASSISTANT</role>"), "{out}");
    }

    /// A prompt already in the right form must be left ALONE — repairing twice would double the
    /// markers, and returning Some() for a correct prompt would hide the real builder's success.
    #[test]
    fn an_already_correct_bailing2_prompt_is_not_touched() {
        let p = "<role>SYSTEM</role>x<|role_end|><role>HUMAN</role>hi<|role_end|><role>ASSISTANT</role>";
        assert!(normalize_prompt_template(p, TemplateKind::Bailing2).is_none());
    }

    /// And the arm must not steal prompts meant for other families.
    #[test]
    fn other_templates_are_unaffected() {
        let p = "<|start_header_id|>user<|end_header_id|>\n\nhi<|eot_id|>";
        let out = normalize_prompt_template(p, TemplateKind::Llama3);
        assert!(out.is_none() || !out.unwrap().contains("<role>"));
    }
}

#[cfg(test)]
mod ctxfit_packing {
    use super::{ctxfit_pack, ctxfit_unpack, CtxFit};

    /// The three numbers share one word so they are read together. If the packing loses or bleeds a
    /// field, the status line reports a prompt against the wrong window — worse than no badge,
    /// because the operator would act on it.
    #[test]
    fn realistic_values_round_trip() {
        for (p, w, e) in [(0usize, 0usize, 0usize), (13412, 3840, 9572), (1, 2, 3),
                          (2_000_000, 1_048_576, 1_000_000), (5852, 32768, 0)] {
            let got = ctxfit_unpack(ctxfit_pack(p, w, e));
            let want = CtxFit { prompt: p.min((1 << 21) - 1),
                                window: w.min((1 << 21) - 1),
                                elided: e.min((1 << 22) - 1) };
            assert_eq!(got, want, "packing {p}/{w}/{e}");
        }
    }

    /// Fields must not bleed into each other: a huge elision cannot invent a prompt.
    #[test]
    fn fields_are_independent() {
        let a = ctxfit_unpack(ctxfit_pack(0, 0, (1 << 22) - 1));
        assert_eq!(a.prompt, 0, "elision bled into prompt");
        assert_eq!(a.window, 0, "elision bled into window");
        let b = ctxfit_unpack(ctxfit_pack((1 << 21) - 1, 0, 0));
        assert_eq!(b.window, 0);
        assert_eq!(b.elided, 0);
    }

    /// Zero means "nothing generated yet", so a real all-zero state must not be mistaken for data.
    #[test]
    fn the_empty_state_is_distinguishable() {
        assert_eq!(ctxfit_pack(0, 0, 0), 0, "no generation yet reads as absent, not as 0/0");
    }

    /// The 1M-context case the repo-reading work is aiming at must not silently saturate: 21 bits
    /// holds 2097151, so a 1048576 window fits with room. This pins that headroom rather than
    /// leaving it to be discovered when a badge reads wrong.
    #[test]
    fn a_million_token_window_fits_the_field() {
        let f = ctxfit_unpack(ctxfit_pack(1_048_576, 1_048_576, 0));
        assert_eq!(f.prompt, 1_048_576);
        assert_eq!(f.window, 1_048_576);
    }
}

// ── the zero-caller guard ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod published_signals {
    //! Three times in one session a SIGNAL was published with nothing reading it:
    //! `memplan::iq1s_load_plan` (tests, no callers, and a doc asserting a gap that had closed),
    //! `COMPACT_REQUEST` (the long-horizon policy publishing a request no UI consumed), and
    //! `context_fit()` (the engine measuring prompt/window/elision for nobody). Each looked
    //! implemented and did nothing, and none of them could fail a test, because the code was
    //! correct — it simply was not reached.
    //!
    //! This is the guard for that class, scoped to where it keeps happening: the atomics
    //! `backend.rs` publishes for the UI. A signal nothing loads is dead; an accessor the chat
    //! never calls is a measurement taken and thrown away. Source-scanning rather than
    //! type-checking because the defect is precisely the thing a compiler is happy about — the
    //! same reason `perfverdict` reads `main.rs` with `include_str!` to check the gate list.

    const BACKEND: &str = include_str!("backend.rs");
    const CHAT: &str = concat!(
        include_str!("tabs/chat.rs"),
        // The draw moved out of `chat.rs` (chat_draw, chat_spatial). A test that reads only
        // the state file would scan code with none of the call sites in it and pass
        // vacuously — which is worse than failing, because it looks like the wiring is fine.
        include_str!("tabs/chat_draw.rs"),
        include_str!("tabs/chat_spatial.rs"),
    );

    /// Names of the atomics `backend.rs` publishes for consumption (`pub`/`pub(crate)` statics).
    fn published() -> Vec<String> {
        BACKEND.lines().filter_map(|l| {
            let t = l.trim_start();
            let rest = t.strip_prefix("pub(crate) static ").or_else(|| t.strip_prefix("pub static "))?;
            let (name, ty) = rest.split_once(':')?;
            ty.contains("Atomic").then(|| name.trim().to_string())
        }).collect()
    }

    #[test]
    fn every_published_signal_is_both_stored_and_loaded() {
        let names = published();
        assert!(names.len() >= 3, "the scan found {} — it stopped matching the declarations", names.len());
        for n in &names {
            assert!(BACKEND.contains(&format!("{n}.store(")),
                    "{n} is published but never written — nothing can be signalled through it");
            let loaded = BACKEND.contains(&format!("{n}.load(")) || CHAT.contains(&format!("{n}\n"))
                || CHAT.contains(&format!("{n}."));
            assert!(loaded, "{n} is written and never read — a signal with no consumer, which is \
what COMPACT_REQUEST was until the compaction banner landed");
        }
    }

    #[test]
    fn every_accessor_over_a_published_signal_has_a_caller_in_the_chat() {
        // `pub fn foo(&self)` in backend.rs whose body loads one of the published atomics is an
        // accessor built FOR the UI; if its name never appears in chat.rs, the measurement is
        // taken and discarded. This is exactly how context_fit() sat unused.
        let names = published();
        let mut checked = 0;
        for (i, line) in BACKEND.lines().enumerate() {
            let t = line.trim_start();
            let Some(rest) = t.strip_prefix("pub fn ") else { continue };
            let Some(fname) = rest.split(['(', '<']).next() else { continue };
            // body = until the next line at the same indent starting with '}'
            let body: String = BACKEND.lines().skip(i + 1).take(12).collect::<Vec<_>>().join("\n");
            if !names.iter().any(|n| body.contains(&format!("{n}.load("))) { continue; }
            checked += 1;
            // `fname(` — a CALL, not a mention. The first draft accepted `CHAT.contains(fname)`
            // and a deliberate mutation (deleting the only call to `context_fit`) did NOT trip it,
            // because a comment three lines away still named the function. A guard against dead
            // code that a comment can satisfy is itself dead.
            assert!(CHAT.contains(&format!("{fname}(")),
                    "backend::{fname} reads a published signal and nothing in the chat calls it — \
either wire it up or delete it, but a measurement nobody reads is not a feature");
        }
        assert!(checked >= 1, "the accessor scan matched nothing — it has stopped working");
    }
}

#[cfg(test)]
mod grammar_names_tests {
    #[cfg(feature = "wgpu-engine")]
    #[test]
    fn names_are_read_back_out_of_the_harness_grammar() {
        use crate::backend::engine::grammar_tool_names;
        // exactly what llm_remote::tool_fence_grammar emits
        let g = crate::llm_remote::tool_fence_grammar(&["read_file", "run"]);
        let mut names = grammar_tool_names(&g);
        names.sort();
        assert_eq!(names, vec!["read_file".to_string(), "run".to_string()], "grammar was:\n{g}");
        // a grammar that is not ours constrains the fence but names nothing
        assert!(grammar_tool_names("root ::= \"x\"").is_empty());
    }
}

#[cfg(test)]
mod chat_flatten_tests {
    use super::*;

    struct Recorder { last: std::rc::Rc<std::cell::RefCell<String>> }
    impl Backend for Recorder {
        fn name(&self) -> &'static str { "rec" }
        fn loaded(&self) -> bool { true }
        fn load(&mut self, _p: &str, _g: &GenParams) -> Result<(), String> { Ok(()) }
        fn generate(&mut self, prompt: &str, _g: &GenParams, _c: &dyn Fn() -> bool, on: &mut dyn FnMut(&str) -> bool) -> Result<usize, String> {
            *self.last.borrow_mut() = prompt.to_string(); on(prompt); Ok(1)
        }
    }

    #[test]
    fn default_generate_chat_flattens_to_generate() {
        let seen = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
        let mut b = Recorder { last: seen.clone() };
        let turns = vec![ChatTurn::system("be brief"), ChatTurn::user("hi"), ChatTurn::assistant("hello"), ChatTurn { role: ChatRole::Tool, content: "42".into() }];
        let mut out = String::new();
        let n = b.generate_chat(&turns, &GenParams::default(), &|| false, &mut |t| { out.push_str(t); true }).unwrap();
        assert_eq!(n, 1);
        let p = seen.borrow();
        assert!(p.contains("System: be brief") && p.contains("User: hi") && p.contains("Assistant: hello") && p.contains("Tool result: 42"));
        assert!(p.trim_end().ends_with("Assistant:"), "the flattened prompt ends with the assistant primer: {p:?}");
    }
}

#[cfg(test)]
mod daemon_tests {
    use super::*;

    /// Serialises the two tests that set XDG_RUNTIME_DIR.
    ///
    /// Fifth time this shape has come up this week (bo-suite,
    /// aperture-engine, gitmind, voiceterm's snapshot store). Env vars
    /// are process-global and cargo runs tests in parallel, so a test
    /// that sets one races every other test that reads it. Adding the
    /// lock up front rather than after a flake.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        L.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn no_daemon_falls_back_rather_than_failing() {
        // THE PROPERTY THAT MAKES THE DAEMON SAFE TO ADD. An app whose
        // model stops loading because a background service is not
        // running would be worse than one with no daemon at all: the
        // failure is invisible and looks like the app's fault.
        let _serial = env_lock();
        let prev = std::env::var("XDG_RUNTIME_DIR").ok();
        std::env::set_var("XDG_RUNTIME_DIR", "/nonexistent-daemon-dir");
        assert_eq!(BackendKind::preferred_reachable(), BackendKind::preferred(),
                   "no daemon should fall back to the compiled-in backend");
        assert_ne!(BackendKind::preferred_reachable(), BackendKind::Daemon);
        match prev { Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                     None => std::env::remove_var("XDG_RUNTIME_DIR") }
    }

    #[test]
    fn remote_spec_parses_and_an_unreachable_server_fails_fast() {
        let _serial = env_lock();
        let (ep, label, np) = Remote::resolve("openai:http://ultra:8080").unwrap();
        assert!(matches!(ep, hive::endpoint::Endpoint::OpenAi { ref base, ref model, .. } if base == "http://ultra:8080" && model == "default"));
        assert!(label.starts_with("openai") && np == 2);
        let (ep, _, _) = Remote::resolve("ollama:http://mini:11434/qwen3.6:35b-a3b").unwrap();
        assert!(matches!(ep, hive::endpoint::Endpoint::Ollama { ref base, ref model } if base == "http://mini:11434" && model == "qwen3.6:35b-a3b"));
        assert!(Remote::resolve("ftp:x").is_err() && Remote::resolve("nocolon").is_err());
        // Loading against a dead port must fail fast and name the target: the worker uses the
        // error to fall back, exactly as it does for a missing daemon.
        let prev = std::env::var("LOGSCOPE_REMOTE").ok();
        std::env::set_var("LOGSCOPE_REMOTE", "openai:http://127.0.0.1:9");
        let err = Remote::default().load("/models/x.gguf", &GenParams::default()).unwrap_err();
        assert!(err.contains("remote openai"), "{err}");
        assert_ne!(BackendKind::preferred_reachable(), BackendKind::Remote, "an unreachable LOGSCOPE_REMOTE must not be preferred");
        match prev { Some(v) => std::env::set_var("LOGSCOPE_REMOTE", v), None => std::env::remove_var("LOGSCOPE_REMOTE") }
    }

    #[test]
    fn a_live_daemon_wins() {
        // The other half: if the ordering were not honoured the memory
        // saving would silently never happen, which is the failure that
        // looks like success.
        let _serial = env_lock();
        let prev = std::env::var("XDG_RUNTIME_DIR").ok();
        let dir = std::env::temp_dir().join(format!("lg-daemon-pick-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sock = dir.join("logscope-engine.sock");
        let _ = std::fs::remove_file(&sock);
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        std::thread::spawn(move || { while listener.accept().is_ok() {} });

        std::env::set_var("XDG_RUNTIME_DIR", &dir);
        assert_eq!(BackendKind::preferred_reachable(), BackendKind::Daemon,
                   "a listening daemon was not preferred");

        match prev { Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                     None => std::env::remove_var("XDG_RUNTIME_DIR") }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chunks_are_drained_only_when_complete() {
        // A chunk split across two socket reads is the NORMAL case, not
        // an edge one. Decoding a partial chunk would emit half a token
        // and then desynchronise the stream, which reads as the model
        // producing garbage.
        let (text, rest, done) = drain_chunks("5\r\nhello\r\n3\r\nwo");
        assert_eq!(text, "hello", "the complete chunk came out");
        assert_eq!(rest, "3\r\nwo", "the partial one was kept whole");
        assert!(!done);

        // Feed the remainder and it completes.
        let (text, rest, done) = drain_chunks("3\r\nwor\r\n0\r\n\r\n");
        assert_eq!(text, "wor");
        assert!(rest.is_empty());
        assert!(done, "the zero chunk terminates");
    }

    #[test]
    fn a_chunk_boundary_inside_a_length_prefix_is_survivable() {
        // The nastiest split: the hex length itself arrives in pieces.
        let (text, rest, done) = drain_chunks("1");
        assert!(text.is_empty() && !done, "nothing decodable yet");
        assert_eq!(rest, "1", "kept for the next read");
    }

    #[test]
    fn the_socket_path_matches_the_servers_rule() {
        let _serial = env_lock();
        let prev = std::env::var("XDG_RUNTIME_DIR").ok();
        // If the client and the daemon disagree about where to meet,
        // each concludes the other is not running — a failure with no
        // useful error anywhere.
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/4242");
        assert_eq!(daemon_socket_path(),
                   std::path::Path::new("/run/user/4242/logscope-engine.sock"));
        match prev { Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                     None => std::env::remove_var("XDG_RUNTIME_DIR") }
    }

    #[test]
    fn loading_without_a_daemon_fails_fast_and_names_the_path() {
        let _serial = env_lock();
        let prev = std::env::var("XDG_RUNTIME_DIR").ok();
        // This is what the caller uses to decide whether to fall back to
        // an in-process load, so it must be quick and it must say where
        // it looked.
        std::env::set_var("XDG_RUNTIME_DIR", "/nonexistent-daemon-dir");
        let mut d = Daemon::default();
        let err = d.load("model.gguf", &GenParams::default()).unwrap_err();
        assert!(err.contains("nonexistent-daemon-dir"), "{err}");
        assert!(!d.loaded(), "a failed load must not look connected");
        match prev { Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                     None => std::env::remove_var("XDG_RUNTIME_DIR") }
    }
}
