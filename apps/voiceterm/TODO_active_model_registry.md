# Active local-model registry (v0.1.554)

## Done — backend primitive (some testable, NOT run in full crate)
`llama_backend` now has a process-global registry of the active loaded
model, so off-thread consumers can use the local model without threading
an Arc through every call chain:

- `set_active_model(Arc<LoadedModel>)` — publish (called at all 4 model-
  store sites in main.rs, right after `*mguard = Some(Arc::new(m))`).
- `active_model() -> Option<Arc<LoadedModel>>` — snapshot (cheap clone).
- `has_active_model() -> bool`.
- Feature-gated: real impl under `llamacpp`, no-op/None/false stubs when
  off (call sites are runtime-guarded by is_available() but compiled
  unconditionally, so the stubs are REQUIRED for the no-feature build —
  don't remove them).

### First consumer wired
`chat_send` (sync path): when the llama_model_locked try_lock is
contended (a worker holds it), it now falls back to `active_model()`
instead of giving up to cloud. Low-risk, immediately useful.

## NOT done — the higher-value, higher-risk consumer (do when testable)
**log_summary should use the local model instead of only Ollama.**
Right now src/log_summary.rs's summarize fn ONLY calls
`system_info::ollama_query_model_raw` in a priority loop. With Ollama
not installed (the user's setup), it churns retries + watchdog-pauses
forever, even though a working Qwen model is loaded. The registry now
makes the model reachable from the summary worker thread.

### Plan when testable
In log_summary.rs's `try_summarize_*` (around line 1180, the
`summarizer_priority_list()` loop):
1. BEFORE the Ollama loop, try the local model:
   `if let Some(m) = crate::llama_backend::active_model() { ... generate
   ... return Ok((text, "local")); }`
   Use llama_backend::generate(&m, prompt, &GenParams{max_tokens:..,
   ..default()}) — check generate's exact signature first (it's the
   feature-gated fn ~line 164).
2. Only fall to the Ollama loop if active_model() is None.
3. This also fixes the retry churn: when a local model exists, summary
   never touches the (absent) Ollama path.

RISK: log_summary has voice-coordination + watchdog + backoff logic on
its worker thread. Adding the local branch must not disturb the
VOICE_ACTIVE deferral or the failure-count/backoff bookkeeping. Test
that summaries actually land (web-summary block in the log) and that
voice transcription isn't starved while a summary generates.

## Verification (this version)
Static: brace-clean (llama_backend + main), qt_struct 441 clean,
qml_lint 7 clean, backend_check clean, pure suite 144/144. The registry
logic itself isn't in the pure set (llama_backend needs FFI). NOT run.
