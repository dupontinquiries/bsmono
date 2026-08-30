//! aperture-engine — logscope's wgpu inference engine, linked IN, not called out to.
//!
//! `LogscopeProvider` (aperture-complete::providers) talks HTTP to a
//! `logscope-serve` running somewhere else. That is the right shape for a
//! model on another machine and the wrong shape for a model on this one:
//! a localhost round-trip per keystroke-idle, a second process to start and
//! babysit, and a copy of the weights resident in it rather than in the
//! editor asking for them.
//!
//! This crate is the other shape. `gpu-gemm` — the same crate
//! `logscope-serve` is built from — is a dependency, the model loads into
//! the editor's own address space, and completion is a function call.
//!
//! ## The seam, and why it is shaped like logscope-serve's
//!
//! `logscope-serve`'s main() already reduces its whole engine to one value:
//!
//! ```text
//! let gen: Arc<dyn Fn(&str, usize) -> String + Send + Sync> = ...;
//! netserve::serve(listener, keyring, gen);
//! ```
//!
//! Everything above that line is loading and warming; everything below is
//! HTTP. [`Backend`] is deliberately that same interface — "text and a
//! token budget in, text out" — because it is the seam a shared daemon
//! needs later:
//!
//! * TODAY, [`LocalEngine`] implements it by owning the model.
//! * The EXISTING `LogscopeProvider` implements the same idea over HTTP.
//! * A future daemon (see `daemon` below) implements it over a unix socket,
//!   holding ONE resident model that aperture and logscope both borrow —
//!   which is the actual goal, since two processes each loading a 15 GB
//!   model on one machine is the thing worth avoiding.
//!
//! Because all three are the same shape, adding the daemon later changes
//! which `Backend` is constructed and nothing else. That is the whole
//! reason this crate does not just inline `gpu_kernels` calls into a
//! provider.
//!
//! ## What is verified here
//!
//! The provider logic, the prompt shaping and the trait wiring are unit
//! tested against a fake `Backend`. The GPU path is NOT — loading a real
//! GGUF needs a model file and a working adapter, neither of which the
//! authoring sandbox has (lavapipe is a CPU rasteriser; a 15 GB checkpoint
//! is not in the tree). [`LocalEngine::load`] is therefore compile-checked
//! and reasoned about, not run. Say so out loud rather than implying a
//! green test covered it.

use aperture_complete::{CompletionRequest, Provider};

/// Text in, text out — the one thing every inference backend does, whether
/// it owns the model, speaks HTTP to a server, or speaks to a daemon that
/// owns it for everybody. Deliberately the same signature
/// `logscope-serve` already collapses its engine into.
///
/// `Send + Sync` because a loaded model is SHARED, not rebuilt: the editor
/// holds one `Arc<dyn Backend>` and every provider the completion worker
/// constructs clones that handle. A 15 GB checkpoint cannot be re-loaded
/// per request, and `build_provider` is called freely — so the sharing has
/// to be in the type, not in a convention someone has to remember.
pub trait Backend: Send + Sync {
    /// A human name for the loop pane and logs.
    fn describe(&self) -> String;
    /// Generate at most `max_tokens` continuing `prompt`.
    fn generate(&self, prompt: &str, max_tokens: usize) -> Result<String, String>;

    /// Generate, delivering text AS IT ARRIVES.
    ///
    /// `on_piece` returns false to stop — a cancelled completion, a
    /// closed pane. Implementations that cannot stream should leave this
    /// default, which calls [`Backend::generate`] and hands the whole
    /// reply over in one piece: correct, and honest about being
    /// non-streaming rather than pretending by chopping a finished
    /// string into fake tokens.
    ///
    /// A DEFAULT rather than a required method, so adding this does not
    /// break every existing Backend — and because most of them genuinely
    /// cannot stream. The one that can is the daemon client, where the
    /// difference is a forty-second stare at a still UI versus text
    /// appearing as the model produces it.
    ///
    /// Returns the full text as well, so a caller that also wants the
    /// finished string does not have to accumulate it itself.
    fn generate_streaming(
        &self,
        prompt: &str,
        max_tokens: usize,
        on_piece: &mut dyn FnMut(&str) -> bool,
    ) -> Result<String, String> {
        let out = self.generate(prompt, max_tokens)?;
        let _ = on_piece(&out);
        Ok(out)
    }
}

/// How the in-process engine samples. Mirrors `logscope-serve`'s own
/// choice (greedy with a repetition penalty and an eos stop, which is what
/// produces coherent terminating completions rather than a wandering
/// sample) but as VISIBLE FIELDS — the same visible-loop discipline the
/// ollama and logscope provider configs already follow, so a person can
/// see and change what the editor asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineConfig {
    /// Path to the GGUF. Loaded once, at [`LocalEngine::load`].
    pub model_path: String,
    /// Tokens to generate per completion.
    pub max_tokens: usize,
    /// 0.0 = greedy. logscope-serve uses greedy for completions.
    pub temperature: f32,
    /// `usize`, matching `gpu_kernels::SamplerParams` exactly — the field
    /// types are the engine's, not a parallel set that needs casting at
    /// the boundary (the compiler caught this one on the first `--features
    /// gpu` check).
    pub top_k: usize,
    pub top_p: f32,
    /// Discourage the model from repeating the buffer back at you — the
    /// failure mode a code completion hits most.
    pub repeat_penalty: f32,
    pub repeat_last_n: usize,
    /// Prepended to the prefix, same role as `LogscopeConfig::instruction`:
    /// these are base-ish models, so a completion rides as continuation
    /// under an instruction rather than as a FIM infill.
    pub instruction: String,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            model_path: String::new(),
            max_tokens: 96,
            temperature: 0.0,
            top_k: 1,
            top_p: 1.0,
            repeat_penalty: 1.15,
            repeat_last_n: 64,
            instruction: "Continue the code exactly from where it stops. Output only the continuation, no prose, no backticks.".into(),
        }
    }
}

/// Build the text sent to the model. Pure, and public for the same reason
/// `providers::assemble_prompt` is: a preview and the real request must be
/// the same function, or they drift.
pub fn assemble_prompt(cfg: &EngineConfig, req: &CompletionRequest) -> String {
    let mut out = String::new();
    if !cfg.instruction.trim().is_empty() {
        out.push_str(cfg.instruction.trim());
        out.push_str("\n\n");
    }
    out.push_str(&req.prefix);
    out
}

/// A GGUF found on disk, for the model selector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundModel {
    pub path: std::path::PathBuf,
    /// File name without the extension — what the picker shows.
    pub label: String,
    pub bytes: u64,
}

impl FoundModel {
    /// "7.2 GB" — the number that decides whether this one fits, so the
    /// picker shows it rather than making someone guess from the name.
    pub fn size_label(&self) -> String {
        let gb = self.bytes as f64 / 1_073_741_824.0;
        if gb >= 1.0 {
            format!("{gb:.1} GB")
        } else {
            format!("{:.0} MB", self.bytes as f64 / 1_048_576.0)
        }
    }
}

/// Where to look for GGUFs when nobody has said. `APERTURE_MODELS` first
/// (explicit beats guessed), then the conventional spots — the same
/// `~/models` logscope-serve is usually pointed at, plus the caches
/// llama.cpp and HF tooling actually write to.
pub fn default_model_dirs() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(v) = std::env::var("APERTURE_MODELS") {
        for part in v.split(':').filter(|p| !p.is_empty()) {
            out.push(std::path::PathBuf::from(part));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let h = std::path::Path::new(&home);
        out.push(h.join("models"));
        out.push(h.join(".cache/huggingface/hub"));
        out.push(h.join(".cache/llama.cpp"));
    }
    out
}

/// Every `.gguf` under these directories, one level deep plus one nested
/// level (HF's hub layout puts the file inside a per-repo folder), sorted
/// by label and de-duplicated by path.
///
/// Deliberately NOT a deep recursive walk: a home directory scan that
/// wanders into a source tree costs seconds and surprises people. Two
/// levels covers the real layouts; anything else is what `APERTURE_MODELS`
/// is for.
pub fn discover_models(dirs: &[std::path::PathBuf]) -> Vec<FoundModel> {
    fn push_gguf(out: &mut Vec<FoundModel>, p: &std::path::Path) {
        if p.extension().and_then(|e| e.to_str()) != Some("gguf") {
            return;
        }
        let Ok(md) = std::fs::metadata(p) else { return };
        if !md.is_file() {
            return;
        }
        out.push(FoundModel {
            label: p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            path: p.to_path_buf(),
            bytes: md.len(),
        });
    }
    let mut out: Vec<FoundModel> = Vec::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if let Ok(inner) = std::fs::read_dir(&p) {
                    for e2 in inner.flatten() {
                        push_gguf(&mut out, &e2.path());
                    }
                }
            } else {
                push_gguf(&mut out, &p);
            }
        }
    }
    out.sort_by(|a, b| a.label.cmp(&b.label).then(a.path.cmp(&b.path)));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

/// A [`Backend`] that owns the model in this process.
///
/// Construction is split from use on purpose: [`load`](Self::load) does the
/// expensive, fallible work (read the GGUF, parse it, build the tokenizer,
/// create the adapter, upload weights, warm pipelines) and reports what
/// happened, so a UI can show "loading…" and then a real error instead of
/// freezing and then failing.
pub struct LocalEngine {
    inner: std::sync::Arc<dyn Backend>,
}

impl LocalEngine {
    /// Wrap any backend — the constructor tests use, and the one a daemon
    /// client will use later without touching this crate's other half.
    pub fn with_backend(inner: Box<dyn Backend>) -> Self {
        LocalEngine { inner: std::sync::Arc::from(inner) }
    }

    /// Share an already-loaded engine. This is the one a UI uses: load
    /// once on a worker thread, keep the handle, hand a clone to every
    /// provider the completion worker builds.
    pub fn shared(inner: std::sync::Arc<dyn Backend>) -> Self {
        LocalEngine { inner }
    }

    /// The handle, for a caller that wants to keep it across rebuilds.
    pub fn handle(&self) -> std::sync::Arc<dyn Backend> {
        self.inner.clone()
    }

    /// Load a GGUF into this process and warm the pipelines.
    ///
    /// The body mirrors `logscope-serve`'s main() step for step, because
    /// it is the same engine: parse, tokenizer, `Gpu::new`, `load_gguf`,
    /// `warm`, then generation via `generate_kv_stream`. Divergence there
    /// would mean the editor and the server disagree about what the same
    /// model does.
    ///
    /// NOT EXERCISED in the authoring sandbox: no adapter worth the name
    /// and no checkpoint in the tree. Compile-checked only.
    #[cfg(feature = "gpu")]
    pub fn load(cfg: EngineConfig) -> Result<Self, String> {
        Ok(LocalEngine { inner: std::sync::Arc::new(gpu::GpuBackend::load(cfg)?) })
    }

    pub fn describe(&self) -> String {
        self.inner.describe()
    }
}

/// How to reach a model, in the order to try.
///
/// The daemon (see the `daemon` module below) is the right answer when it
/// is there: one resident copy of the weights instead of one per app, on
/// a machine where the loader already clamps its own GPU budget from 48
/// GiB to 31. But an editor whose completions stop working because a
/// background service is not running is worse than an editor with no
/// daemon at all — the failure is invisible, intermittent, and looks like
/// the editor's fault.
///
/// So the fallback is not an error path. It is the ordinary path with a
/// preferred first entry, and `resolve` returns the first backend that
/// actually constructs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// A shared daemon holding the weights for every app on the machine.
    Daemon,
    /// This process loads its own copy.
    InProcess,
}

/// Try each reach in order, returning the first that works, plus which
/// one it was so a UI can say where the answer is coming from.
///
/// Takes CONSTRUCTORS rather than doing the work, so this is testable
/// without a daemon, a model or a GPU — the decision is what is worth
/// pinning, and the decision is "try, and carry on if it fails".
///
/// Returns the LAST error when everything fails, not the first: the last
/// attempt is the in-process load, and "no such model file" is a more
/// useful thing to show than "connection refused" from a daemon the
/// person may not know they were supposed to have.
pub fn resolve(
    order: &[Reach],
    mut daemon: impl FnMut() -> Result<std::sync::Arc<dyn Backend>, String>,
    mut in_process: impl FnMut() -> Result<std::sync::Arc<dyn Backend>, String>,
) -> Result<(LocalEngine, Reach), String> {
    let mut last = Err("no reach was attempted".to_string());
    for r in order {
        let attempt = match r {
            Reach::Daemon => daemon(),
            Reach::InProcess => in_process(),
        };
        match attempt {
            Ok(b) => return Ok((LocalEngine::shared(b), *r)),
            Err(e) => last = Err(e),
        }
    }
    last.map(|_: std::sync::Arc<dyn Backend>| unreachable!())
}

/// The [`Provider`] adapter: this is what the completion worker holds.
pub struct EngineProvider {
    pub cfg: EngineConfig,
    engine: LocalEngine,
}

impl EngineProvider {
    pub fn new(cfg: EngineConfig, engine: LocalEngine) -> Self {
        EngineProvider { cfg, engine }
    }
}

impl Provider for EngineProvider {
    fn name(&self) -> &str {
        "engine (in-process)"
    }

    fn complete(&mut self, req: &CompletionRequest) -> Result<String, String> {
        let prompt = assemble_prompt(&self.cfg, req);
        self.engine.inner.generate(&prompt, self.cfg.max_tokens)
    }
}

// ── the GPU half, behind a feature ──────────────────────────────────────────
//
// Feature-gated so `cargo test -p aperture-engine` (and anything that only
// wants the trait and the prompt shaping) does not build wgpu. The default
// is OFF for exactly the reason aperture-complete has no dependencies at
// all: the fast loop stays fast.
#[cfg(feature = "gpu")]
mod gpu {
    use super::{Backend, EngineConfig};
    use gpu_kernels::tokenizer::Tokenizer;
    use gpu_kernels::{gguf, Gpu};
    use std::sync::Arc;

    pub struct GpuBackend {
        gpu: Arc<Gpu>,
        model: Arc<gpu_kernels::Model>,
        tok: Arc<Tokenizer>,
        eos: Option<u32>,
        adapter: String,
        sampler: gpu_kernels::SamplerParams,
    }

    impl GpuBackend {
        pub fn load(cfg: EngineConfig) -> Result<Self, String> {
            let bytes = std::fs::read(&cfg.model_path)
                .map_err(|e| format!("read {}: {e}", cfg.model_path))?;
            let g = gguf::parse(&bytes).map_err(|e| format!("parse gguf: {e}"))?;
            let tok = Arc::new(Tokenizer::from_gguf(&g).map_err(|e| format!("tokenizer: {e}"))?);
            let gpu = Arc::new(Gpu::new());
            let model = Arc::new(gpu.load_gguf(&bytes).map_err(|e| format!("load model: {e}"))?);
            gpu.warm(&model);
            // Same eos resolution as logscope-serve: the GGUF's own id,
            // falling back to the llama3 end-of-turn token.
            let eos = g
                .get_u32("tokenizer.ggml.eos_token_id")
                .or_else(|| tok.special_id("<|eot_id|>"));
            let adapter = gpu.adapter_name.clone();
            let sampler = gpu_kernels::SamplerParams {
                temperature: cfg.temperature,
                top_k: cfg.top_k,
                top_p: cfg.top_p,
                repeat_penalty: cfg.repeat_penalty,
                repeat_last_n: cfg.repeat_last_n,
                min_p: 0.0,
            };
            Ok(GpuBackend { gpu, model, tok, eos, adapter, sampler })
        }
    }

    impl Backend for GpuBackend {
        fn describe(&self) -> String {
            format!("in-process wgpu ({})", self.adapter)
        }

        fn generate(&self, prompt: &str, max_tokens: usize) -> Result<String, String> {
            let ids = self.tok.encode(prompt);
            let mut rng = 0xC0FFEE_u64;
            let mut out: Vec<u32> = Vec::new();
            self.gpu.generate_kv_stream(
                &self.model,
                &ids,
                max_tokens,
                self.eos,
                &self.sampler,
                &mut rng,
                &|| false,
                |t| {
                    out.push(t);
                    true
                },
            );
            Ok(self.tok.decode(&out))
        }
    }
}

// ── the daemon seam ─────────────────────────────────────────────────────────

/// Where a SHARED daemon plugs in, and why nothing here has to change when
/// it arrives.
///
/// The problem it solves is resident weight, not latency: on one machine,
/// aperture holding its own copy of a 15 GB model while logscope holds
/// another is the thing that does not fit. A daemon owns the model once;
/// both editors borrow it.
///
/// The design that follows from what already exists:
///
/// * The daemon is `logscope-serve` with a unix-socket listener alongside
///   its TCP one — not a new server. `netserve::serve` already takes
///   `(listener, keyring, gen)`, and `gen` is already the
///   `Fn(&str, usize) -> String` shape [`Backend`] mirrors, so the engine
///   half is done; what is missing is the socket and a lifecycle (start on
///   demand, idle-unload, one instance per model path).
/// * Aperture's side is a `DaemonBackend` implementing [`Backend`] over
///   that socket. It goes HERE, next to `GpuBackend`, and
///   `LocalEngine::with_backend` already accepts it — the provider, the
///   prompt assembly and the loop pane do not learn about it.
/// * Unix socket rather than the existing TCP+Bearer path because
///   same-machine sharing should not be authenticated by a key on a
///   loopback port when filesystem permissions already say who may
///   connect. The TCP path stays exactly as it is for the real
///   across-the-LAN case `LogscopeProvider` serves.
///
/// Not built. Recorded here rather than in a planning document because
/// this is the file whose shape has to accommodate it.
pub mod daemon {
    use super::Backend;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    /// Where the daemon listens. One socket per user, under the runtime
    /// dir when there is one — a socket in /tmp is world-visible and this
    /// carries prompts.
    /// Delegates to the daemon's own crate under the `gpu` feature, so
    /// the two halves cannot disagree about where to meet — two copies
    /// of this rule is how a client and a server each conclude the other
    /// is not running. Without the feature (the default, dependency-free
    /// build) it reimplements the same rule, and a test asserts the two
    /// agree.
    pub fn socket_path() -> std::path::PathBuf {
        #[cfg(feature = "gpu")]
        {
            gpu_kernels::netserve::default_socket_path()
        }
        #[cfg(not(feature = "gpu"))]
        {
            if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
                return std::path::Path::new(&rt).join("logscope-engine.sock");
            }
            // No runtime dir (a bare container, some BSDs): fall back to
            // the home directory rather than /tmp, so the permissions are
            // the user's own rather than a sticky shared directory's.
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            std::path::Path::new(&home).join(".logscope-engine.sock")
        }
    }

    fn find_seq(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    /// The request line both paths send, so they cannot drift.
    fn request(prompt: &str, max_tokens: usize) -> String {
        format!(
            "POST /generate?max={} HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/plain\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{prompt}",
            max_tokens.clamp(1, 8192),
            prompt.len(),
        )
    }

    /// Pull every COMPLETE chunk out of `buf`.
    ///
    /// Returns the decoded text, the undecodable remainder, and whether
    /// the terminating zero-chunk was seen. A partial chunk at the end is
    /// LEFT in the remainder rather than guessed at — that is the whole
    /// difference between streaming and corrupting.
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
            if n == 0 {
                return (out, String::new(), true);
            }
            if tail.len() < n + 2 {
                return (out, rest.to_string(), false); // incomplete: wait
            }
            out.push_str(&tail[..n]);
            rest = &tail[n + 2..];
        }
    }

    /// Decode an HTTP/1.1 chunked body.
    ///
    /// Mirrors `gpu_gemm::netserve::chunked::decode`, and is duplicated
    /// on purpose: the default build of this crate does not depend on
    /// gpu-gemm at all (that is the whole point of the `gpu` feature), so
    /// linking the daemon crate to parse eight lines of framing would
    /// drag a GPU engine into an editor that only wanted to talk to a
    /// socket. The two are pinned equal by a test.
    ///
    /// `None` on a malformed length or a truncated chunk — a connection
    /// cut mid-reply is an error, not a short answer.
    pub fn decode_chunked(body: &str) -> Option<String> {
        let mut out = String::new();
        let mut rest = body;
        loop {
            let (len_s, tail) = rest.split_once("\r\n")?;
            let len_s = len_s.split(';').next()?.trim(); // chunk extensions
            let n = usize::from_str_radix(len_s, 16).ok()?;
            if n == 0 {
                return Some(out);
            }
            if tail.len() < n {
                return None;
            }
            out.push_str(&tail[..n]);
            rest = tail.get(n + 2..)?;
        }
    }

    /// A [`Backend`] that asks a daemon holding the weights.
    ///
    /// Speaks the SAME request shape as `logscope-serve`'s HTTP endpoint
    /// (POST /generate, prompt as the raw body, `?max=N`) over a unix
    /// socket instead of TCP. Same shape on purpose: the daemon is that
    /// server with a second listener, not a second protocol, so one
    /// handler serves both and they cannot drift.
    ///
    /// No Bearer key here. On a unix socket the filesystem already
    /// answers "who may connect", and a key on a same-machine socket is
    /// ceremony that also has to be stored somewhere.
    pub struct DaemonBackend {
        pub path: std::path::PathBuf,
        pub timeout: std::time::Duration,
    }

    impl DaemonBackend {
        pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
            DaemonBackend { path: path.into(), timeout: std::time::Duration::from_secs(120) }
        }

        /// Connect, or fail fast. Called by `resolve`'s daemon
        /// constructor, so "the daemon is not running" costs one failed
        /// connect rather than a timeout — the fallback should be quick
        /// enough that nobody notices it happened.
        pub fn probe(path: impl Into<std::path::PathBuf>) -> Result<Self, String> {
            let me = DaemonBackend::new(path);
            UnixStream::connect(&me.path).map_err(|e| format!("daemon at {}: {e}", me.path.display()))?;
            Ok(me)
        }
    }

    impl Backend for DaemonBackend {
        fn describe(&self) -> String {
            format!("daemon ({})", self.path.display())
        }

        /// Reads the socket INCREMENTALLY, decoding each chunk as it
        /// lands, so text appears while the model is still producing it.
        /// That is the whole reason the chunked path exists: at ~4 tok/s
        /// a 170-token reply is otherwise forty seconds of a still UI.
        ///
        /// Falls back to the whole-body path when the daemon answers
        /// without `Transfer-Encoding: chunked` — an older daemon, or
        /// `serve_unix` rather than `serve_unix_streaming`. Mixed
        /// versions on one machine are normal during an upgrade.
        fn generate_streaming(
            &self,
            prompt: &str,
            max_tokens: usize,
            on_piece: &mut dyn FnMut(&str) -> bool,
        ) -> Result<String, String> {
            use std::io::Read;
            let mut sock = UnixStream::connect(&self.path).map_err(|e| format!("connect: {e}"))?;
            sock.set_read_timeout(Some(self.timeout)).ok();
            sock.set_write_timeout(Some(self.timeout)).ok();
            sock.write_all(request(prompt, max_tokens).as_bytes())
                .map_err(|e| format!("write: {e}"))?;

            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            let head_end = loop {
                if let Some(p) = find_seq(&buf, b"\r\n\r\n") { break p; }
                let n = sock.read(&mut tmp).map_err(|e| format!("read: {e}"))?;
                if n == 0 { return Err("daemon closed before sending a header".into()); }
                buf.extend_from_slice(&tmp[..n]);
            };
            let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
            let status = head.lines().next().unwrap_or("").to_string();
            if !status.contains(" 200") {
                let body = String::from_utf8_lossy(&buf[head_end + 4..]).into_owned();
                return Err(format!("{}: {}", status.trim(),
                                   body.trim().lines().next().unwrap_or("")));
            }
            let chunked = head.lines().any(|l| {
                let l = l.to_ascii_lowercase();
                l.starts_with("transfer-encoding:") && l.contains("chunked")
            });

            let mut pending = String::from_utf8_lossy(&buf[head_end + 4..]).into_owned();
            if !chunked {
                let mut rest = String::new();
                sock.read_to_string(&mut rest).map_err(|e| format!("read: {e}"))?;
                pending.push_str(&rest);
                let _ = on_piece(&pending);
                return Ok(pending);
            }

            let mut out = String::new();
            loop {
                let (text, rest, done) = drain_chunks(&pending);
                pending = rest;
                if !text.is_empty() {
                    out.push_str(&text);
                    // The caller stopped caring; dropping the socket is
                    // what tells the daemon to stop generating.
                    if !on_piece(&text) { return Ok(out); }
                }
                if done { return Ok(out); }
                let n = sock.read(&mut tmp).map_err(|e| format!("read: {e}"))?;
                if n == 0 { return Err("daemon closed the connection mid-reply".into()); }
                pending.push_str(&String::from_utf8_lossy(&tmp[..n]));
            }
        }
        fn generate(&self, prompt: &str, max_tokens: usize) -> Result<String, String> {
            let mut sock = UnixStream::connect(&self.path).map_err(|e| format!("connect: {e}"))?;
            sock.set_read_timeout(Some(self.timeout)).ok();
            sock.set_write_timeout(Some(self.timeout)).ok();
            let req = format!(
                "POST /generate?max={} HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/plain\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{prompt}",
                max_tokens.clamp(1, 8192),
                prompt.len(),
            );
            sock.write_all(req.as_bytes()).map_err(|e| format!("write: {e}"))?;
            let mut raw = String::new();
            sock.read_to_string(&mut raw).map_err(|e| format!("read: {e}"))?;
            let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
            let status = head.lines().next().unwrap_or("");
            if status.contains(" 200") {
                // CHUNKED OR NOT. The daemon grew a streaming endpoint
                // that answers with `Transfer-Encoding: chunked`, and
                // this client only ever read whole bodies — so turning
                // streaming on server-side would have delivered hex
                // length prefixes INSIDE the completion text. Not a
                // crash: a completion containing "1a" and stray CRLFs,
                // which is the kind of corruption that gets blamed on
                // the model.
                //
                // Decided by the header rather than by sniffing the
                // body, because a completion can legitimately begin with
                // a hex-looking line.
                if head
                    .lines()
                    .any(|l| l.to_ascii_lowercase().starts_with("transfer-encoding:")
                        && l.to_ascii_lowercase().contains("chunked"))
                {
                    decode_chunked(body).ok_or_else(|| {
                        // A truncated chunked body must not read as a
                        // short answer: the daemon died mid-generation
                        // and a half-completion accepted as whole is
                        // worse than a visible error.
                        "daemon closed the connection mid-reply".to_string()
                    })
                } else {
                    Ok(body.to_string())
                }
            } else {
                // Carry the daemon's own first line rather than a generic
                // failure: it is the only place that knows whether the
                // model is missing, still loading, or out of memory.
                Err(format!("{}: {}", status.trim(), body.trim().lines().next().unwrap_or("")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backend that records what it was asked and replies from a script —
    /// the fake that lets the provider logic be tested with no GPU.
    struct FakeBackend {
        reply: String,
        seen: std::sync::Arc<std::sync::Mutex<Vec<(String, usize)>>>,
    }

    impl Backend for FakeBackend {
        fn describe(&self) -> String {
            "fake".into()
        }
        fn generate(&self, prompt: &str, max_tokens: usize) -> Result<String, String> {
            self.seen.lock().unwrap().push((prompt.to_string(), max_tokens));
            Ok(self.reply.clone())
        }
    }

    fn req(prefix: &str) -> CompletionRequest {
        CompletionRequest {
            generation: 0,
            prefix: prefix.into(),
            suffix: String::new(),
            lang: "rust".into(),
            path: None,
        }
    }

    fn ok_backend() -> std::sync::Arc<dyn Backend> {
        std::sync::Arc::new(FakeBackend { reply: "x".into(), seen: Default::default() })
    }

    /// Serialises the tests that mutate PROCESS-GLOBAL env vars.
    ///
    /// `socket_path` reads XDG_RUNTIME_DIR/HOME and `default_model_dirs`
    /// reads APERTURE_MODELS, and cargo runs tests on parallel threads —
    /// so one test's `set_var` lands mid-run in another. They happen to
    /// pass today, which is luck rather than isolation: this is the exact
    /// shape of the bo-suite failure diagnosed earlier this session,
    /// where a per-test tempdir looked like isolation while the shared
    /// thing was the variable naming the directory. Serialising now is
    /// cheaper than diagnosing it later from a flake.
    ///
    /// Poison ignored so one panicking test does not mask the rest.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        L.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// A one-shot fake daemon on a unix socket: answers the next
    /// connection with `body`, records the raw request.
    fn fake_daemon(body: &'static str, status: &'static str)
        -> (std::path::PathBuf, std::sync::mpsc::Receiver<String>)
    {
        use std::io::{Read, Write};
        let dir = std::env::temp_dir().join(format!("ap-daemon-{}-{:?}",
            std::process::id(), std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("engine.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            while let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 8192];
                let n = s.read(&mut buf).unwrap_or(0);
                let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
                let _ = s.write_all(format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()).as_bytes());
            }
        });
        (path, rx)
    }

    #[test]
    fn the_daemon_client_speaks_the_same_request_shape_as_logscope_serve() {
        // Same shape on purpose: the daemon is logscope-serve with a
        // second listener, not a second protocol. If this drifts, one
        // handler stops serving both.
        let (path, rx) = fake_daemon("let x = 1;", "200 OK");
        let b = daemon::DaemonBackend::new(&path);
        let out = b.generate("fn f() { ", 64).expect("generated");
        assert_eq!(out, "let x = 1;");

        let req = rx.recv().unwrap();
        assert!(req.starts_with("POST /generate?max=64 HTTP/1.1\r\n"), "{req}");
        assert!(req.contains("fn f() {"), "the prompt is the body: {req}");
        // No Bearer key: on a unix socket the filesystem answers who may
        // connect, and a key here would be ceremony with a storage
        // problem attached.
        assert!(!req.contains("Authorization"), "{req}");
    }

    /// A fake daemon that answers with chunked transfer, like the
    /// streaming endpoint does.
    fn chunked_daemon(pieces: &'static [&'static str]) -> std::path::PathBuf {
        use std::io::{Read, Write};
        let dir = std::env::temp_dir().join(format!(
            "ap-chunk-{}-{:?}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("engine.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            while let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let mut body = String::new();
                for p in pieces {
                    body.push_str(&format!("{:x}\r\n{p}\r\n", p.len()));
                }
                body.push_str("0\r\n\r\n");
                let _ = s.write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
                     Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n{body}"
                ).as_bytes());
            }
        });
        path
    }

    #[test]
    fn a_chunked_reply_arrives_as_text_not_as_framing() {
        // THE BREAK THIS PREVENTS: the daemon grew a streaming endpoint
        // and this client only read whole bodies, so turning streaming on
        // would have delivered hex length prefixes inside the completion.
        // Not a crash — a completion containing "b" and stray CRLFs,
        // which gets blamed on the model.
        let path = chunked_daemon(&["let x", " = 1", ";"]);
        let b = daemon::DaemonBackend::new(&path);
        let out = b.generate("fn f() {", 32).expect("generated");
        assert_eq!(out, "let x = 1;", "framing leaked into the text: {out:?}");
    }

    /// The duplicated decoder must match the server's, or the two halves
    /// disagree about what a reply says.
    ///
    /// Only runs with the `gpu` feature, since that is the only build
    /// that links the daemon crate — which is the point of the
    /// duplication: an editor that only wants to talk to a socket should
    /// not drag in a GPU engine to parse eight lines of framing.
    #[cfg(feature = "gpu")]
    #[test]
    fn the_two_chunked_decoders_agree() {
        let cases = [
            "5\r\nhello\r\n0\r\n\r\n",
            "2;x=1\r\nhi\r\n0\r\n\r\n",
            "0\r\n\r\n",
            "5\r\nab\r\n",          // truncated
            "zz\r\nhi\r\n0\r\n\r\n", // bad length
            "",
        ];
        for c in cases {
            assert_eq!(
                daemon::decode_chunked(c),
                gpu_kernels::netserve::chunked::decode(c),
                "decoders disagree on {c:?}"
            );
        }
    }

    #[test]
    fn streaming_delivers_pieces_before_the_reply_is_finished() {
        // The property the whole chunked path exists for. A test that
        // only checked the final string would pass against the
        // non-streaming client too, so this asserts the FIRST piece
        // arrives while the daemon is still generating.
        use std::io::{Read, Write};
        let dir = std::env::temp_dir().join(format!("ap-stream-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("s.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut b = [0u8; 4096];
                let _ = s.read(&mut b);
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
                let _ = s.write_all(b"6\r\nfirst \r\n");
                let _ = s.flush();
                std::thread::sleep(std::time::Duration::from_millis(250));
                let _ = s.write_all(b"6\r\nsecond\r\n0\r\n\r\n");
                let _ = s.flush();
            }
        });

        let b = daemon::DaemonBackend::new(&path);
        let start = std::time::Instant::now();
        let mut first_at = None;
        let mut pieces = Vec::new();
        let out = b
            .generate_streaming("q", 16, &mut |p: &str| {
                if first_at.is_none() {
                    first_at = Some(start.elapsed());
                }
                pieces.push(p.to_string());
                true
            })
            .expect("streamed");

        assert_eq!(out, "first second");
        assert_eq!(pieces.first().map(String::as_str), Some("first "));
        // The daemon sleeps 250ms before the second piece. If the first
        // only arrived after that, nothing streamed.
        let t = first_at.expect("a piece arrived");
        assert!(t.as_millis() < 200, "first piece waited {t:?} — not streaming");
        assert!(pieces.len() >= 2, "arrived as one lump: {pieces:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_non_streaming_daemon_still_works_through_the_same_call() {
        // Mixed versions on one machine are normal during an upgrade, so
        // generate_streaming must handle a Content-Length daemon too.
        let (path, _rx) = fake_daemon("whole reply", "200 OK");
        let b = daemon::DaemonBackend::new(&path);
        let mut got = String::new();
        let out = b
            .generate_streaming("q", 8, &mut |p: &str| { got.push_str(p); true })
            .expect("worked");
        assert_eq!(out, "whole reply");
        assert_eq!(got, "whole reply", "delivered once, in full");
    }

    #[test]
    fn a_truncated_chunked_reply_is_an_error_not_a_short_answer() {
        // A daemon killed mid-generation must not look like it finished.
        // `decode_chunked` returns None and the caller must surface that.
        assert_eq!(daemon::decode_chunked("5\r\nab\r\n"), None, "truncated read as success");
        assert_eq!(daemon::decode_chunked("zz\r\nhi\r\n0\r\n\r\n"), None, "bad length");
        // And a well-formed one still works, including chunk extensions.
        assert_eq!(daemon::decode_chunked("2;x=1\r\nhi\r\n0\r\n\r\n").as_deref(), Some("hi"));
    }

    #[test]
    fn a_plain_reply_still_works_unchanged() {
        // The non-streaming endpoint is still what `serve_unix` uses, so
        // the Content-Length path must not regress.
        let (path, _rx) = fake_daemon("plain body", "200 OK");
        let b = daemon::DaemonBackend::new(&path);
        assert_eq!(b.generate("x", 8).expect("generated"), "plain body");
    }

    #[test]
    fn a_daemon_error_carries_its_own_words() {
        // The daemon is the only party that knows whether the model is
        // missing, still loading, or out of memory. Replacing that with a
        // generic failure throws away the only useful diagnosis.
        let (path, _rx) = fake_daemon("model still loading", "503 Service Unavailable");
        let b = daemon::DaemonBackend::new(&path);
        let err = b.generate("x", 8).err().expect("a 503 is an error");
        assert!(err.contains("503"), "{err}");
        assert!(err.contains("still loading"), "{err}");
    }

    #[test]
    fn probing_an_absent_daemon_fails_fast_and_names_the_path() {
        // This is what `resolve` calls, so it must be quick and it must
        // say WHERE it looked — "connection refused" with no path is the
        // least actionable error a person can be handed.
        let missing = std::env::temp_dir().join("definitely-not-a-daemon.sock");
        let _ = std::fs::remove_file(&missing);
        let err = daemon::DaemonBackend::probe(&missing).err().expect("no daemon there");
        assert!(err.contains("definitely-not-a-daemon.sock"), "{err}");
    }

    #[test]
    fn the_socket_path_is_per_user_not_shared() {
        // A socket in a sticky shared directory is world-visible, and
        // this one carries prompts.
        let _serial = env_lock();
        let restore_rt = std::env::var("XDG_RUNTIME_DIR").ok();
        let restore_home = std::env::var("HOME").ok();
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        assert_eq!(daemon::socket_path(),
                   std::path::Path::new("/run/user/1000/logscope-engine.sock"));
        std::env::remove_var("XDG_RUNTIME_DIR");
        // No runtime dir: the home directory, not /tmp.
        std::env::set_var("HOME", "/home/someone");
        assert_eq!(daemon::socket_path(),
                   std::path::Path::new("/home/someone/.logscope-engine.sock"));
        // Put the environment back: a test that leaves HOME pointing at a
        // fiction breaks whatever runs next in this process.
        match restore_rt { Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v), None => std::env::remove_var("XDG_RUNTIME_DIR") }
        match restore_home { Some(v) => std::env::set_var("HOME", v), None => std::env::remove_var("HOME") }
    }

    #[test]
    fn a_missing_daemon_falls_through_to_the_local_load() {
        // The property that decides whether a daemon is safe to add at
        // all: an editor whose completions stop because a background
        // service is not running is worse than one with no daemon. The
        // failure would be invisible, intermittent, and look like the
        // editor's fault.
        let order = [Reach::Daemon, Reach::InProcess];
        // Destructured with a match rather than `.expect`, because
        // LocalEngine holds a trait object and deriving Debug on it just
        // to satisfy unwrap would be the tail wagging the dog.
        let (engine, used) = match resolve(
            &order,
            || Err("connection refused".into()),
            || Ok(ok_backend()),
        ) {
            Ok(v) => v,
            Err(e) => panic!("did not fall through to local: {e}"),
        };
        assert_eq!(used, Reach::InProcess);
        assert_eq!(engine.describe(), "fake");
    }

    #[test]
    fn the_daemon_wins_when_it_is_there() {
        // The whole point: one resident copy of the weights instead of
        // one per app. If the order were not honoured the daemon would
        // never be used and the memory saving would silently not happen.
        let (_e, used) = match resolve(
            &[Reach::Daemon, Reach::InProcess],
            || Ok(ok_backend()),
            || panic!("local load ran while the daemon was available"),
        ) {
            Ok(v) => v,
            Err(e) => panic!("daemon was available but resolve failed: {e}"),
        };
        assert_eq!(used, Reach::Daemon);
    }

    #[test]
    fn total_failure_reports_the_LAST_error_not_the_first() {
        // The last attempt is the in-process load, and "no such model
        // file" is more useful to show than "connection refused" from a
        // daemon the person may not know they were meant to have.
        let err = resolve(
            &[Reach::Daemon, Reach::InProcess],
            || Err("connection refused".into()),
            || Err("no such model file".into()),
        )
        .err()
        .expect("both failed, so this must be an error");
        assert_eq!(err, "no such model file");
    }

    #[test]
    fn an_empty_order_is_an_error_rather_than_a_panic() {
        let err = resolve(&[], || Ok(ok_backend()), || Ok(ok_backend()))
            .err()
            .expect("an empty order cannot succeed");
        assert!(err.contains("no reach"), "{err}");
    }

    #[test]
    fn the_provider_sends_the_assembled_prompt_and_honors_the_budget() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let backend = FakeBackend { reply: "let x = 1;".into(), seen: seen.clone() };
        let cfg = EngineConfig {
            instruction: "Continue.".into(),
            max_tokens: 32,
            ..EngineConfig::default()
        };
        let mut p = EngineProvider::new(cfg, LocalEngine::with_backend(Box::new(backend)));
        let out = p.complete(&req("fn f() { ")).unwrap();
        assert_eq!(out, "let x = 1;");

        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), 1, "one completion, one generate");
        let (prompt, max) = &calls[0];
        assert_eq!(*max, 32, "the config's budget reaches the backend");
        assert!(prompt.starts_with("Continue.\n\n"), "instruction first: {prompt:?}");
        assert!(prompt.ends_with("fn f() { "), "the prompt ends where the model continues: {prompt:?}");
    }

    #[test]
    fn an_empty_instruction_sends_the_bare_prefix() {
        // The instruction is a knob, not a requirement — blanking it must
        // give the model exactly the buffer, with no invisible preamble
        // left behind.
        let cfg = EngineConfig { instruction: "  ".into(), ..EngineConfig::default() };
        let p = assemble_prompt(&cfg, &req("fn f() { "));
        assert_eq!(p, "fn f() { ");
    }

    #[test]
    fn a_backend_error_reaches_the_caller_rather_than_becoming_an_empty_ghost() {
        // A failed generate must surface as Err. Silently returning "" would
        // render as a ghost suggestion of nothing, which reads as "the model
        // had no idea" instead of "the model never ran".
        struct Failing;
        impl Backend for Failing {
            fn describe(&self) -> String {
                "failing".into()
            }
            fn generate(&self, _p: &str, _m: usize) -> Result<String, String> {
                Err("adapter lost".into())
            }
        }
        let mut p = EngineProvider::new(
            EngineConfig::default(),
            LocalEngine::with_backend(Box::new(Failing)),
        );
        assert_eq!(p.complete(&req("x")), Err("adapter lost".into()));
    }

    #[test]
    fn discovery_finds_ggufs_one_and_two_levels_deep_and_ignores_the_rest() {
        let dir = std::env::temp_dir().join(format!("aperture-eng-disc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repo-a")).unwrap();
        std::fs::create_dir_all(dir.join("deep/deeper")).unwrap();
        std::fs::write(dir.join("zeta.gguf"), b"x").unwrap();          // top level
        std::fs::write(dir.join("repo-a/alpha.gguf"), b"xx").unwrap(); // HF-style nesting
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();          // wrong extension
        std::fs::write(dir.join("deep/deeper/buried.gguf"), b"x").unwrap(); // too deep

        let found = discover_models(&[dir.clone()]);
        let labels: Vec<&str> = found.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, vec!["alpha", "zeta"], "sorted by label, both depths, {found:?}");
        assert!(!labels.contains(&"notes"), "only .gguf files are models");
        assert!(!labels.contains(&"buried"),
                "three levels deep is not scanned — a home-dir walk that wanders is the bug this avoids");

        // A directory that does not exist is silently skipped, not an error:
        // default_model_dirs() lists conventional paths that mostly will not
        // exist on any given machine.
        let missing = dir.join("nope");
        assert_eq!(discover_models(&[missing]).len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn size_label_switches_units_where_a_person_would() {
        let m = |bytes| FoundModel { path: "x".into(), label: "x".into(), bytes };
        assert_eq!(m(7_730_941_132).size_label(), "7.2 GB");
        assert_eq!(m(400 * 1_048_576).size_label(), "400 MB");
        // Exactly 1 GiB reads as GB, not "1024 MB".
        assert_eq!(m(1_073_741_824).size_label(), "1.0 GB");
    }

    #[test]
    fn explicit_model_dirs_come_before_the_guessed_ones() {
        // APERTURE_MODELS is the escape hatch for a machine that keeps its
        // models somewhere unconventional; it must WIN, not merely be
        // included, or the picker leads with whatever ~/models happens to
        // hold.
        let _serial = env_lock();
        std::env::set_var("APERTURE_MODELS", "/tmp/first:/tmp/second");
        let dirs = default_model_dirs();
        std::env::remove_var("APERTURE_MODELS");
        assert_eq!(dirs[0], std::path::PathBuf::from("/tmp/first"));
        assert_eq!(dirs[1], std::path::PathBuf::from("/tmp/second"));
    }

    #[test]
    fn a_loaded_engine_is_shared_not_rebuilt_per_provider() {
        // The 15 GB fact, pinned: two providers built from one handle must
        // point at the SAME backend, or every build_provider call would
        // re-load a checkpoint.
        let backend: std::sync::Arc<dyn Backend> = std::sync::Arc::new(FakeBackend {
            reply: "ok".into(),
            seen: Default::default(),
        });
        let a = LocalEngine::shared(backend.clone());
        let b = LocalEngine::shared(backend.clone());
        assert!(std::sync::Arc::ptr_eq(&a.handle(), &b.handle()),
                "both engines share one resident model");
    }

    #[test]
    fn describe_reaches_through_the_wrapper() {        // The loop pane shows this; it must be the BACKEND's answer (which
        // names the real adapter) and not a constant from the wrapper.
        let e = LocalEngine::with_backend(Box::new(FakeBackend {
            reply: String::new(),
            seen: Default::default(),
        }));
        assert_eq!(e.describe(), "fake");
    }
}
