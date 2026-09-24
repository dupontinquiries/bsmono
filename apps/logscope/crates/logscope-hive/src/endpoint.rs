//! endpoint — one place that generates text, whatever protocol it speaks.
//! OpenAI SSE (llama-server, optiq, the Studio), Anthropic Messages SSE (llama-server's /v1/messages,
//! optiq, api.anthropic.com), Ollama NDJSON, and Claude Code in headless `-p --output-format stream-json`
//! mode (the only way a Max plan is allowed to be used from another program: drive the real binary).
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Endpoint {
    OpenAi { base: String, model: String, #[serde(default)] key: Option<String> },
    Anthropic { base: String, model: String, #[serde(default)] key: Option<String> },
    Ollama { base: String, model: String },
    /// `claude -p` on this machine; `extra` are CLI flags (e.g. --allowedTools Read,Grep).
    ClaudeCode { #[serde(default)] extra: Vec<String>, #[serde(default)] cwd: Option<String> },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metrics {
    pub ttft_ms: f64,
    pub total_ms: f64,
    pub out_tokens: u32,
    pub prompt_tokens: Option<u32>,
    pub gen_tok_s: f64,
    pub server_gen_tok_s: Option<f64>,
    pub prompt_tok_s: Option<f64>,
    pub draft_accept: Option<f64>,
    pub cost_usd: Option<f64>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Event {
    Started,
    Delta(String),
    Reasoning(String),
    /// A tool call the SERVER parsed out of the model's reply — llama-server with `--jinja` applies
    /// the model's own template on the way in and its matching parser on the way out, so the caller
    /// gets `{name, arguments}` instead of having to recognise `<tool_call>` itself. Streamed calls
    /// arrive in fragments; this fires once per completed call.
    ToolCall { id: Option<String>, name: String, arguments: serde_json::Value },
    Done(Metrics),
    Error(String),
}

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub prompt: String,
    pub system: Option<String>,
    pub max_tokens: u32,
    pub temperature: f32,
    /// Claude Code only: continue a previous headless session.
    pub resume: Option<String>,
    /// A GBNF grammar for servers that take one (llama-server does, on its OpenAI endpoint too);
    /// others ignore the field.
    pub grammar: Option<String>,
}

/// Blocking. Emits Started, Delta…, then Done or Error, and returns the final metrics.
/// The callback returns `true` to keep streaming; `false` cancels: the reader is dropped, the
/// connection closes, and every server here stops generating on a closed socket.
pub fn generate(ep: &Endpoint, req: &Request, on: &mut dyn FnMut(Event) -> bool) -> Result<Metrics, String> {
    let _ = on(Event::Started);
    let r = match ep {
        Endpoint::OpenAi { base, model, key } => openai(base, model, key.as_deref(), req, on),
        Endpoint::Anthropic { base, model, key } => anthropic(base, model, key.as_deref(), req, on),
        Endpoint::Ollama { base, model } => ollama(base, model, req, on),
        Endpoint::ClaudeCode { extra, cwd } => claude_code(extra, cwd.as_deref(), req, on),
    };
    match r { Ok(m) => { let _ = on(Event::Done(m.clone())); Ok(m) } Err(e) => { let _ = on(Event::Error(e.clone())); Err(e) } }
}

/// Health / model listing without generating. Returns a short description or the error.
pub fn probe(ep: &Endpoint) -> Result<String, String> {
    let t = Duration::from_millis(1500);
    match ep {
        Endpoint::OpenAi { base, .. } => {
            let j: serde_json::Value = ureq::get(&format!("{base}/v1/models")).timeout(t).call().map_err(|e| e.to_string())?.into_json().map_err(|e| e.to_string())?;
            let names: Vec<String> = j.get("data").and_then(|d| d.as_array()).map(|a| a.iter().filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(|s| s.to_string())).collect()).unwrap_or_default();
            Ok(format!("openai-compatible, models: {}", if names.is_empty() { "(unnamed)".into() } else { names.join(", ") }))
        }
        Endpoint::Anthropic { base, .. } => {
            // no standard list endpoint on every server; a 1-token message is the probe
            let body = serde_json::json!({"model": "probe", "max_tokens": 1, "messages": [{"role": "user", "content": "hi"}]});
            let mut r = ureq::post(&format!("{base}/v1/messages")).set("anthropic-version", "2023-06-01").timeout(Duration::from_secs(20));
            if let Endpoint::Anthropic { key: Some(k), .. } = ep { r = r.set("x-api-key", k); }
            r.send_json(body).map(|_| "anthropic-compatible".to_string()).map_err(|e| e.to_string())
        }
        Endpoint::Ollama { base, .. } => {
            let j: serde_json::Value = ureq::get(&format!("{base}/api/tags")).timeout(t).call().map_err(|e| e.to_string())?.into_json().map_err(|e| e.to_string())?;
            let names: Vec<String> = j.get("models").and_then(|d| d.as_array()).map(|a| a.iter().filter_map(|m| m.get("name").and_then(|i| i.as_str()).map(|s| s.to_string())).collect()).unwrap_or_default();
            Ok(format!("ollama, models: {}", names.join(", ")))
        }
        Endpoint::ClaudeCode { .. } => {
            let out = Command::new("claude").arg("--version").output().map_err(|e| format!("claude not on PATH: {e}"))?;
            Ok(format!("claude code {}", String::from_utf8_lossy(&out.stdout).trim()))
        }
    }
}

fn finish(m: &mut Metrics, t0: Instant, first: Option<Instant>) {
    m.total_ms = t0.elapsed().as_secs_f64() * 1000.0;
    if let Some(f) = first { let s = f.elapsed().as_secs_f64(); if s > 0.0 && m.out_tokens > 1 { m.gen_tok_s = (m.out_tokens as f64 - 1.0) / s; } }
}

/// A role+content pair for the chat endpoint. `content` is plain text; tool results ride as role
/// "tool" so a templating server frames them.
#[derive(Debug, Clone)]
pub struct ChatMessage { pub role: String, pub content: String }

/// Stream an OpenAI-compatible chat completion from an explicit message list, so llama-server applies
/// the model's own template. `req.prompt` is ignored; everything else (grammar, temperature, tokens)
/// applies. This is what `generate_chat` on the Remote backend calls.
pub fn generate_messages(ep: &Endpoint, messages: &[ChatMessage], req: &Request, on: &mut dyn FnMut(Event) -> bool) -> Result<Metrics, String> {
    let _ = on(Event::Started);
    let (base, model, key) = match ep {
        Endpoint::OpenAi { base, model, key } => (base.as_str(), model.as_str(), key.as_deref()),
        Endpoint::Anthropic { base, model, key } => (base.as_str(), model.as_str(), key.as_deref()),
        other => return Err(format!("generate_messages: {other:?} has no chat-completions endpoint")),
    };
    let json: Vec<serde_json::Value> = messages.iter().map(|m| serde_json::json!({"role": m.role, "content": m.content})).collect();
    let r = openai_stream(base, model, key, json, req, on);
    match &r { Ok(m) => { let _ = on(Event::Done(m.clone())); } Err(e) => { let _ = on(Event::Error(e.clone())); } }
    r
}

fn openai(base: &str, model: &str, key: Option<&str>, req: &Request, on: &mut dyn FnMut(Event) -> bool) -> Result<Metrics, String> {
    let mut messages = Vec::new();
    if let Some(s) = &req.system { messages.push(serde_json::json!({"role": "system", "content": s})); }
    messages.push(serde_json::json!({"role": "user", "content": req.prompt}));
    openai_stream(base, model, key, messages, req, on)
}

fn openai_stream(base: &str, model: &str, key: Option<&str>, messages: Vec<serde_json::Value>, req: &Request, on: &mut dyn FnMut(Event) -> bool) -> Result<Metrics, String> {
    let mut body = serde_json::json!({"model": model, "messages": messages, "stream": true, "max_tokens": req.max_tokens,
        "temperature": req.temperature, "stream_options": {"include_usage": true}, "cache_prompt": true});
    if let Some(g) = &req.grammar { body["grammar"] = serde_json::Value::String(g.clone()); }
    let t0 = Instant::now();
    let mut r = ureq::post(&format!("{base}/v1/chat/completions")).timeout(Duration::from_secs(900));
    if let Some(k) = key { r = r.set("Authorization", &format!("Bearer {k}")); }
    let resp = r.send_json(body).map_err(|e| e.to_string())?;
    let (mut m, mut first) = (Metrics::default(), None);
    // index → (id, name, argument text so far). OpenAI streams a call's arguments as JSON fragments
    // across deltas, so they are joined here and parsed once at the end.
    let mut pending: std::collections::BTreeMap<u64, (Option<String>, String, String)> = std::collections::BTreeMap::new();
    for line in BufReader::new(resp.into_reader()).lines() {
        let line = line.map_err(|e| e.to_string())?;
        let Some(data) = line.strip_prefix("data: ") else { continue };
        if data.trim() == "[DONE]" { break; }
        let Ok(j) = serde_json::from_str::<serde_json::Value>(data) else { continue };
        if let Some(tcs) = j.pointer("/choices/0/delta/tool_calls").and_then(|t| t.as_array()) {
            for tc in tcs {
                let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                let e = pending.entry(idx).or_default();
                if let Some(id) = tc.get("id").and_then(|i| i.as_str()) { e.0 = Some(id.to_string()); }
                if let Some(n) = tc.pointer("/function/name").and_then(|n| n.as_str()) { e.1.push_str(n); }
                if let Some(a) = tc.pointer("/function/arguments").and_then(|a| a.as_str()) { e.2.push_str(a); }
            }
        }
        if let Some(rc) = j.pointer("/choices/0/delta/reasoning_content").and_then(|c| c.as_str()) {
            if !rc.is_empty() { if first.is_none() { first = Some(Instant::now()); m.ttft_ms = t0.elapsed().as_secs_f64() * 1000.0; } if !on(Event::Reasoning(rc.to_string())) { break; } }
        }
        if let Some(c) = j.pointer("/choices/0/delta/content").and_then(|c| c.as_str()) {
            if !c.is_empty() { if first.is_none() { first = Some(Instant::now()); m.ttft_ms = t0.elapsed().as_secs_f64() * 1000.0; } m.out_tokens += 1; if !on(Event::Delta(c.to_string())) { break; } }
        }
        if let Some(t) = j.get("timings") {
            m.prompt_tokens = t.get("prompt_n").and_then(|v| v.as_u64()).map(|v| v as u32);
            m.prompt_tok_s = t.get("prompt_per_second").and_then(|v| v.as_f64());
            m.server_gen_tok_s = t.get("predicted_per_second").and_then(|v| v.as_f64());
            if let Some(n) = t.get("predicted_n").and_then(|v| v.as_u64()) { m.out_tokens = n as u32; }
            if let (Some(a), Some(d)) = (t.get("draft_n_accepted").and_then(|v| v.as_f64()), t.get("draft_n").and_then(|v| v.as_f64())) { if d > 0.0 { m.draft_accept = Some(a / d); } }
        }
        if let Some(u) = j.get("usage") {
            if m.prompt_tokens.is_none() { m.prompt_tokens = u.get("prompt_tokens").and_then(|v| v.as_u64()).map(|v| v as u32); }
            if let Some(n) = u.get("completion_tokens").and_then(|v| v.as_u64()) { m.out_tokens = n as u32; }
        }
    }
    for (_, (id, name, args)) in pending {
        if name.is_empty() { continue; }
        let arguments = serde_json::from_str(&args).unwrap_or(serde_json::json!({}));
        if !on(Event::ToolCall { id, name, arguments }) { break; }
    }
    finish(&mut m, t0, first);
    Ok(m)
}

fn anthropic(base: &str, model: &str, key: Option<&str>, req: &Request, on: &mut dyn FnMut(Event) -> bool) -> Result<Metrics, String> {
    let mut body = serde_json::json!({"model": model, "max_tokens": req.max_tokens, "stream": true,
        "messages": [{"role": "user", "content": req.prompt}]});
    if let Some(s) = &req.system { body["system"] = serde_json::Value::String(s.clone()); }
    if req.temperature > 0.0 { body["temperature"] = serde_json::json!(req.temperature); }
    let t0 = Instant::now();
    let mut r = ureq::post(&format!("{base}/v1/messages")).set("anthropic-version", "2023-06-01").timeout(Duration::from_secs(900));
    if let Some(k) = key { r = r.set("x-api-key", k); }
    let resp = r.send_json(body).map_err(|e| e.to_string())?;
    let (mut m, mut first) = (Metrics::default(), None);
    for line in BufReader::new(resp.into_reader()).lines() {
        let line = line.map_err(|e| e.to_string())?;
        let Some(data) = line.strip_prefix("data: ") else { continue };   // `event:` lines carry no payload we need
        let Ok(j) = serde_json::from_str::<serde_json::Value>(data) else { continue };
        match j.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "message_start" => { m.prompt_tokens = j.pointer("/message/usage/input_tokens").and_then(|v| v.as_u64()).map(|v| v as u32); }
            "content_block_delta" => {
                if let Some(th) = j.pointer("/delta/thinking").and_then(|t| t.as_str()) {
                    if first.is_none() { first = Some(Instant::now()); m.ttft_ms = t0.elapsed().as_secs_f64() * 1000.0; }
                    if !on(Event::Reasoning(th.to_string())) { break; }
                } else if let Some(text) = j.pointer("/delta/text").and_then(|t| t.as_str()) {
                    if first.is_none() { first = Some(Instant::now()); m.ttft_ms = t0.elapsed().as_secs_f64() * 1000.0; }
                    m.out_tokens += 1; if !on(Event::Delta(text.to_string())) { break; }
                }
            }
            "message_delta" => { if let Some(n) = j.pointer("/usage/output_tokens").and_then(|v| v.as_u64()) { m.out_tokens = n as u32; } }
            "message_stop" => break,
            "error" => return Err(j.pointer("/error/message").and_then(|s| s.as_str()).unwrap_or("stream error").to_string()),
            _ => {}
        }
    }
    finish(&mut m, t0, first);
    Ok(m)
}

fn ollama(base: &str, model: &str, req: &Request, on: &mut dyn FnMut(Event) -> bool) -> Result<Metrics, String> {
    let mut body = serde_json::json!({"model": model, "prompt": req.prompt, "stream": true, "options": {"num_predict": req.max_tokens, "temperature": req.temperature}});
    if let Some(s) = &req.system { body["system"] = serde_json::Value::String(s.clone()); }
    let t0 = Instant::now();
    let resp = ureq::post(&format!("{base}/api/generate")).timeout(Duration::from_secs(900)).send_json(body).map_err(|e| e.to_string())?;
    let (mut m, mut first) = (Metrics::default(), None);
    for line in BufReader::new(resp.into_reader()).lines() {
        let line = line.map_err(|e| e.to_string())?;
        let Ok(j) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if let Some(p) = j.get("response").and_then(|c| c.as_str()) { if !p.is_empty() { if first.is_none() { first = Some(Instant::now()); m.ttft_ms = t0.elapsed().as_secs_f64() * 1000.0; } m.out_tokens += 1; if !on(Event::Delta(p.to_string())) { break; } } }
        if j.get("done").and_then(|d| d.as_bool()) == Some(true) {
            let ns = |k: &str| j.get(k).and_then(|v| v.as_f64());
            m.prompt_tokens = j.get("prompt_eval_count").and_then(|v| v.as_u64()).map(|v| v as u32);
            if let (Some(n), Some(d)) = (m.prompt_tokens, ns("prompt_eval_duration")) { if d > 0.0 { m.prompt_tok_s = Some(n as f64 / (d / 1e9)); } }
            if let (Some(n), Some(d)) = (ns("eval_count"), ns("eval_duration")) { if d > 0.0 { m.server_gen_tok_s = Some(n / (d / 1e9)); m.out_tokens = n as u32; } }
            break;
        }
    }
    finish(&mut m, t0, first);
    Ok(m)
}

/// Drive the real `claude` binary. Never reads its credentials; only pipes text in and JSON out.
fn claude_code(extra: &[String], cwd: Option<&str>, req: &Request, on: &mut dyn FnMut(Event) -> bool) -> Result<Metrics, String> {
    let mut cmd = Command::new("claude");
    cmd.arg("-p").arg(&req.prompt).args(["--output-format", "stream-json", "--verbose", "--include-partial-messages"]);
    if let Some(s) = &req.system { cmd.arg("--append-system-prompt").arg(s); }
    if let Some(id) = &req.resume { cmd.arg("--resume").arg(id); }
    cmd.args(extra);
    if let Some(d) = cwd { cmd.current_dir(d); }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let t0 = Instant::now();
    let mut child = cmd.spawn().map_err(|e| format!("spawn claude: {e} (Claude Code installed and logged in?)"))?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let (mut m, mut first) = (Metrics::default(), None);
    let mut saw_partial = false;
    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|e| e.to_string())?;
        if let Some(ev) = parse_claude_line(&line, &mut m, &mut saw_partial) {
            if first.is_none() { first = Some(Instant::now()); m.ttft_ms = t0.elapsed().as_secs_f64() * 1000.0; }
            if !on(Event::Delta(ev)) { let _ = child.kill(); break; }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() && m.out_tokens == 0 {
        let mut err = String::new(); if let Some(mut e) = child.stderr.take() { use std::io::Read; let _ = e.read_to_string(&mut err); }
        return Err(format!("claude exited {status}: {}", err.trim()));
    }
    finish(&mut m, t0, first);
    Ok(m)
}

/// One stream-json line → text to show, updating metrics. Tolerant of the format's variants:
/// `stream_event` (partial deltas), `assistant` (whole content blocks), `result` (totals).
pub fn parse_claude_line(line: &str, m: &mut Metrics, saw_partial: &mut bool) -> Option<String> {
    let j: serde_json::Value = serde_json::from_str(line).ok()?;
    match j.get("type").and_then(|t| t.as_str())? {
        "system" => { m.session_id = j.get("session_id").and_then(|s| s.as_str()).map(|s| s.to_string()); None }
        "stream_event" => {
            // reasoning first: Claude Code carries it as a thinking_delta; it is not answer text.
            if let Some(th) = j.pointer("/event/delta/thinking").and_then(|t| t.as_str()) { return Some(format!("\u{1}think\u{1}{th}")); }
            let text = j.pointer("/event/delta/text").and_then(|t| t.as_str())?.to_string();
            *saw_partial = true; m.out_tokens += 1; Some(text)
        }
        "assistant" => {
            if *saw_partial { return None; }   // partials already carried this text
            let blocks = j.pointer("/message/content").and_then(|c| c.as_array())?;
            let text: String = blocks.iter().filter_map(|b| match b.get("type").and_then(|t| t.as_str()) {
                Some("text") => b.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()),
                Some("tool_use") => Some(format!("\n[tool: {}]\n", b.get("name").and_then(|n| n.as_str()).unwrap_or("?"))),
                _ => None }).collect();
            if text.is_empty() { None } else { m.out_tokens += (text.len() / 4).max(1) as u32; Some(text) }
        }
        "result" => {
            m.session_id = j.get("session_id").and_then(|s| s.as_str()).map(|s| s.to_string()).or(m.session_id.take());
            m.cost_usd = j.get("total_cost_usd").and_then(|v| v.as_f64());
            if let Some(n) = j.pointer("/usage/output_tokens").and_then(|v| v.as_u64()) { m.out_tokens = n as u32; }
            m.prompt_tokens = j.pointer("/usage/input_tokens").and_then(|v| v.as_u64()).map(|v| v as u32);
            None
        }
        _ => None,
    }
}
