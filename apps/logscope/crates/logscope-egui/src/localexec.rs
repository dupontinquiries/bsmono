//! localexec.rs — the logscope-native `dispatch::Executor`: run an assigned slice on a local engine
//! `Backend` (the wgpu/Burn engine or llama.cpp) rather than shelling to an external server. This is the
//! on-box counterpart to `ollama::OllamaExecutor`, for fleet members running the engine directly.
//!
//! It's a thin adapter over the crate's existing `backend::Backend` trait — which already owns the loaded
//! model, tokenizer, and sampler and streams tokens — so it carries no engine wiring of its own and is
//! unit-testable with a mock backend (no GPU, no features). A real member builds one from its loaded
//! `WgpuEngine`/`LlamaCpp` backend; the GPU decode path is exercised on a box. No kernels here.
#![allow(dead_code)]

use crate::backend::Backend;
use crate::dispatch::Executor;
use logscope::llm_backend::GenParams;

/// Wraps a loaded engine `Backend` plus a `GenParams` template. Each assigned slice's `payload` is the
/// prompt; `run` generates the full reply (accumulating the streamed tokens) and returns it trimmed.
pub struct BackendExecutor<B: Backend> {
    pub backend: B,
    pub params: GenParams,
}

impl<B: Backend> BackendExecutor<B> {
    pub fn new(backend: B, params: GenParams) -> Self { Self { backend, params } }
}

impl<B: Backend> Executor for BackendExecutor<B> {
    fn run(&mut self, _task: u64, payload: &str) -> Result<String, String> {
        if !self.backend.loaded() { return Err("logscope backend has no model loaded".into()); }
        let mut out = String::new();
        let never_cancel = || false;
        // collect the streamed tokens; never cancel (dispatch handles timeouts at the scheduler layer)
        self.backend.generate(payload, &self.params, &never_cancel, &mut |tok| { out.push_str(tok); true })?;
        Ok(out.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CPU mock of the `Backend` trait — streams "ran:<prompt>" in two chunks, no model or GPU needed.
    struct EchoBackend { loaded: bool, fail: bool }
    impl Backend for EchoBackend {
        fn name(&self) -> &'static str { "echo" }
        fn loaded(&self) -> bool { self.loaded }
        fn load(&mut self, _path: &str, _p: &GenParams) -> Result<(), String> { self.loaded = true; Ok(()) }
        fn generate(&mut self, prompt: &str, _p: &GenParams, _cancel: &dyn Fn() -> bool, on: &mut dyn FnMut(&str) -> bool) -> Result<usize, String> {
            if self.fail { return Err("decode failed".into()); }
            on("ran:");      // two chunks → exercises token accumulation
            on(prompt);
            Ok(2)
        }
    }

    #[test]
    fn forwards_prompt_and_collects_streamed_reply() {
        let mut ex = BackendExecutor::new(EchoBackend { loaded: true, fail: false }, GenParams::default());
        assert_eq!(ex.run(7, "hello").unwrap(), "ran:hello");
    }

    #[test]
    fn errors_when_no_model_loaded() {
        let mut ex = BackendExecutor::new(EchoBackend { loaded: false, fail: false }, GenParams::default());
        assert!(ex.run(0, "x").unwrap_err().contains("no model loaded"));
    }

    #[test]
    fn propagates_backend_error() {
        let mut ex = BackendExecutor::new(EchoBackend { loaded: true, fail: true }, GenParams::default());
        assert!(ex.run(0, "x").unwrap_err().contains("decode failed"));
    }
}
