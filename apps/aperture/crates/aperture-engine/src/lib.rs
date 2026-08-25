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
/// `Send` because the completion worker runs providers off the UI thread;
/// no `Sync` requirement, since a provider is owned by one worker.
pub trait Backend: Send {
    /// A human name for the loop pane and logs.
    fn describe(&self) -> String;
    /// Generate at most `max_tokens` continuing `prompt`.
    fn generate(&self, prompt: &str, max_tokens: usize) -> Result<String, String>;
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

/// A [`Backend`] that owns the model in this process.
///
/// Construction is split from use on purpose: [`load`](Self::load) does the
/// expensive, fallible work (read the GGUF, parse it, build the tokenizer,
/// create the adapter, upload weights, warm pipelines) and reports what
/// happened, so a UI can show "loading…" and then a real error instead of
/// freezing and then failing.
pub struct LocalEngine {
    inner: Box<dyn Backend>,
}

impl LocalEngine {
    /// Wrap any backend — the constructor tests use, and the one a daemon
    /// client will use later without touching this crate's other half.
    pub fn with_backend(inner: Box<dyn Backend>) -> Self {
        LocalEngine { inner }
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
        Ok(LocalEngine { inner: Box::new(gpu::GpuBackend::load(cfg)?) })
    }

    pub fn describe(&self) -> String {
        self.inner.describe()
    }
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
pub mod daemon {}

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
    fn describe_reaches_through_the_wrapper() {
        // The loop pane shows this; it must be the BACKEND's answer (which
        // names the real adapter) and not a constant from the wrapper.
        let e = LocalEngine::with_backend(Box::new(FakeBackend {
            reply: String::new(),
            seen: Default::default(),
        }));
        assert_eq!(e.describe(), "fake");
    }
}
