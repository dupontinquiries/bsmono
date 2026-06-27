//! HTTP client for an OpenAI-compatible chat completions endpoint.
//!
//! Uses reqwest's blocking API on purpose: this client runs INSIDE a
//! worker thread spawned by `LlmService`, so non-blocking would mean
//! pulling in a whole tokio runtime for one HTTP call. Blocking +
//! mpsc channel back to the UI thread is the simpler model.
//!
//! Two completion modes:
//!   - `complete()` — single-shot, returns the full response text
//!   - `complete_stream(on_partial)` — Server-Sent Events; the
//!     callback fires with the accumulated text-so-far on each
//!     delta token. Final return is the same accumulated string.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::BufRead;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    max_tokens: u32,
    temperature: f32,
    stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String, // "system" | "user" | "assistant"
    pub content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

/// SSE delta event payload.
#[derive(Debug, Deserialize)]
struct StreamResponse {
    choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
}

#[derive(Debug, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
}

/// Configuration snapshot the client takes at construction.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
    pub system_prompt: String,
    pub timeout_ms: u32,
    pub max_tokens: u32,
    pub temperature: f32,
}

pub struct LlmClient {
    cfg: ClientConfig,
    http: reqwest::blocking::Client,
}

impl LlmClient {
    pub fn new(cfg: ClientConfig) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(cfg.timeout_ms as u64))
            .build()
            .context("building reqwest client")?;
        Ok(Self { cfg, http })
    }

    /// Run a one-shot chat completion. Returns the assistant's reply
    /// text (whitespace-trimmed).
    pub fn complete(&self, user_prompt: &str) -> Result<String> {
        let body = ChatRequest {
            model: self.cfg.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: self.cfg.system_prompt.clone(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: user_prompt.to_string(),
                },
            ],
            max_tokens: self.cfg.max_tokens,
            temperature: self.cfg.temperature,
            stream: false,
        };

        let mut req = self.http.post(&self.cfg.endpoint).json(&body);
        if !self.cfg.api_key.is_empty() {
            req = req.bearer_auth(&self.cfg.api_key);
        }

        let resp = req.send().context("sending LLM request")?;
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().unwrap_or_default();
            return Err(anyhow!(
                "LLM endpoint returned HTTP {}: {}",
                status,
                body_text
            ));
        }

        let parsed: ChatResponse = resp.json().context("parsing LLM response")?;
        let text = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();
        Ok(text.trim().to_string())
    }

    /// Streaming completion. Sets `stream: true` on the request and
    /// reads Server-Sent Events line-by-line, parsing each
    /// `data: {...}` payload as a delta. The callback fires with
    /// the accumulated-so-far text after each delta; the final
    /// return is the same string with whitespace trimmed.
    ///
    /// The callback runs ON THE WORKER THREAD that called
    /// complete_stream — it's not main-thread safe. Use it to
    /// write to a shared atomic/mutex that the UI thread polls.
    pub fn complete_stream(
        &self,
        user_prompt: &str,
        on_partial: impl Fn(&str),
    ) -> Result<String> {
        let body = ChatRequest {
            model: self.cfg.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: self.cfg.system_prompt.clone(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: user_prompt.to_string(),
                },
            ],
            max_tokens: self.cfg.max_tokens,
            temperature: self.cfg.temperature,
            stream: true,
        };

        let mut req = self.http.post(&self.cfg.endpoint).json(&body);
        if !self.cfg.api_key.is_empty() {
            req = req.bearer_auth(&self.cfg.api_key);
        }

        let resp = req.send().context("sending streaming LLM request")?;
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().unwrap_or_default();
            return Err(anyhow!(
                "LLM endpoint returned HTTP {}: {}",
                status,
                body_text
            ));
        }

        // SSE: lines are "data: <json>" followed by a blank line.
        // The terminator is "data: [DONE]". We use a BufReader on
        // the response body to read line-by-line.
        let reader = std::io::BufReader::new(resp);
        let mut accumulated = String::new();
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    return Err(anyhow!("error reading SSE stream: {}", e));
                }
            };
            // Skip blanks and comments.
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            // We only handle `data: ...` lines; other SSE field
            // types (event:, id:, retry:) are ignored.
            let payload = match line.strip_prefix("data: ") {
                Some(p) => p,
                None => continue,
            };
            if payload.trim() == "[DONE]" {
                break;
            }
            // Some servers omit the space after "data:".
            let payload = payload.trim();
            if payload == "[DONE]" {
                break;
            }
            // Parse the JSON. Ignore parse errors silently — the
            // server may emit non-delta keepalives or differently-
            // shaped envelopes that we don't care about.
            let parsed: Result<StreamResponse, _> = serde_json::from_str(payload);
            if let Ok(envelope) = parsed {
                if let Some(choice) = envelope.choices.into_iter().next() {
                    if let Some(chunk) = choice.delta.content {
                        accumulated.push_str(&chunk);
                        on_partial(&accumulated);
                    }
                }
            }
        }

        Ok(accumulated.trim().to_string())
    }
}
