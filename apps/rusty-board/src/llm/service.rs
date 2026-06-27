//! `LlmService` — the LLM equivalent of `VoiceService`. Owns a
//! cloned config snapshot, spawns a worker thread per request,
//! returns mpsc::Receiver for the bridge to poll.

use crate::config::LlmConfig;
use crate::llm::client::{ClientConfig, LlmClient};
use crate::llm::LlmTask;
use anyhow::{Context, Result};
use std::sync::mpsc;
use tracing::{debug, info};

/// A single request issued to the service. The task is what the
/// user asked for; `committed_on_success` controls whether the
/// service's caller wants the bridge to auto-commit the resulting
/// text to the focused app (the common case) or just display it
/// in the microterm for the user to confirm.
#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub task: LlmTask,
    pub commit_on_success: bool,
}

/// The reply type returned via mpsc to the bridge.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub text: String,
    pub commit: bool,
    pub task_label: &'static str,
}

pub struct LlmService {
    client_cfg: ClientConfig,
}

impl LlmService {
    pub fn new(cfg: LlmConfig) -> Result<Self> {
        if !cfg.enabled {
            anyhow::bail!("LLM disabled in config");
        }
        let client_cfg = ClientConfig {
            endpoint: cfg.endpoint,
            model: cfg.model,
            api_key: cfg.api_key,
            system_prompt: cfg.system_prompt,
            timeout_ms: cfg.timeout_ms,
            max_tokens: cfg.max_tokens,
            temperature: cfg.temperature,
        };
        // Try once at startup to surface obvious errors (e.g. bad
        // endpoint URL) before the user actually invokes the
        // feature. A successful new() means the URL parses; we
        // don't make a real HTTP call here to avoid blocking
        // startup.
        let _ = LlmClient::new(client_cfg.clone()).context("constructing LlmClient")?;
        info!(
            "LLM service ready (endpoint={}, model={})",
            client_cfg.endpoint, client_cfg.model
        );
        Ok(Self { client_cfg })
    }

    /// Run a request on a background thread; the returned Receiver
    /// yields exactly one LlmResponse (or an Err if the request
    /// failed). The bridge polls it from the GTK main loop.
    pub fn submit(
        &self,
        req: LlmRequest,
    ) -> Result<mpsc::Receiver<Result<LlmResponse>>> {
        let (tx, rx) = mpsc::channel();
        let cfg = self.client_cfg.clone();
        std::thread::Builder::new()
            .name("llm-worker".to_string())
            .spawn(move || {
                let result = match LlmClient::new(cfg) {
                    Ok(client) => {
                        let prompt = req.task.user_prompt();
                        debug!("LLM prompt: {}", prompt);
                        client.complete(&prompt).map(|text| LlmResponse {
                            text,
                            commit: req.commit_on_success,
                            task_label: req.task.label(),
                        })
                    }
                    Err(e) => Err(e),
                };
                let _ = tx.send(result);
            })
            .context("spawning LLM worker thread")?;
        Ok(rx)
    }

    /// Like submit() but uses SSE streaming. As tokens arrive, the
    /// worker writes the accumulated-so-far string into
    /// `partials_slot` (a shared Mutex<String> that the bridge's
    /// timer reads at ~30Hz to update the microterm). The final
    /// result still arrives via the returned Receiver.
    pub fn submit_stream(
        &self,
        req: LlmRequest,
        partials_slot: std::sync::Arc<parking_lot::Mutex<String>>,
    ) -> Result<mpsc::Receiver<Result<LlmResponse>>> {
        let (tx, rx) = mpsc::channel();
        let cfg = self.client_cfg.clone();
        std::thread::Builder::new()
            .name("llm-stream-worker".to_string())
            .spawn(move || {
                // Clear any prior partial before starting.
                partials_slot.lock().clear();
                let result = match LlmClient::new(cfg) {
                    Ok(client) => {
                        let prompt = req.task.user_prompt();
                        debug!("LLM stream prompt: {}", prompt);
                        let label = req.task.label();
                        let commit = req.commit_on_success;
                        let slot = partials_slot.clone();
                        client
                            .complete_stream(&prompt, move |acc| {
                                *slot.lock() = acc.to_string();
                            })
                            .map(|text| LlmResponse {
                                text,
                                commit,
                                task_label: label,
                            })
                    }
                    Err(e) => Err(e),
                };
                let _ = tx.send(result);
            })
            .context("spawning LLM stream-worker thread")?;
        Ok(rx)
    }
}
