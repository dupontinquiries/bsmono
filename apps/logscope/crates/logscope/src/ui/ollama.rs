/// ollama.rs — local Ollama API client with streaming + cancel support.

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

pub const DEFAULT_HOST:  &str = "http://localhost:11434";
#[allow(dead_code)]
pub const DEFAULT_MODEL: &str = ""; // empty = auto-detect

/// Pick the first available model, falling back to "tinyllama".
pub fn resolve_model(host: &str) -> String {
    list_models(host)
        .ok()
        .and_then(|mut m: Vec<String>| m.drain(..).next())
        .unwrap_or_else(|| "tinyllama".into())
}

/// List models available in this Ollama instance.
pub fn list_models(host: &str) -> Result<Vec<String>, String> {
    let url = format!("{host}/api/tags");
    let resp = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .call()
        .map_err(|e| format!("Ollama not reachable at {host}: {e}"))?;
    let body: serde_json::Value = resp.into_json()
        .map_err(|e| e.to_string())?;
    Ok(body["models"]
        .as_array()
        .map(|arr| arr.iter()
            .filter_map(|m| m["name"].as_str().map(str::to_owned))
            .collect::<Vec<_>>())
        .unwrap_or_default())
}

// ── result types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct OllamaResult {
    pub text:            String,
    pub model:           String,
    pub prompt_tokens:   u64,
    pub response_tokens: u64,
}

#[derive(Debug, Clone)]
pub enum StreamChunk {
    Token(String),
    Done { model: String },
    Cancelled,
    Error(String),
}

// ── public streaming API ──────────────────────────────────────────────────────

/// Streaming explain — checks both cancel flags between every chunk.
/// `global_cancel`: set by window close or tab reset (kills all calls).
/// `local_cancel`:  set by per-card/per-selection cancel button.
#[allow(dead_code)]
pub fn explain_streaming(
    lines:         &[&str],
    host:          &str,
    model:         &str,
    tx:            &crossbeam_channel::Sender<StreamChunk>,
    global_cancel: &Arc<AtomicBool>,
    local_cancel:  &Arc<AtomicBool>,
) {
    let block  = lines.join("\n");
    let prompt = format!(
        "You are a log analysis assistant. Explain what is happening in these \
         log lines. Be concise (2-4 sentences). Identify: what component is \
         involved, what event occurred, whether it looks like an error or normal \
         operation, and any recommended action.\n\nLog lines:\n{block}"
    );
    let model = resolve_if_empty(model, host);
    stream_ollama(&prompt, host, &model, tx, global_cancel, local_cancel);
}

#[allow(dead_code)]
pub fn summarize_streaming(
    text:          &str,
    host:          &str,
    model:         &str,
    tx:            &crossbeam_channel::Sender<StreamChunk>,
    global_cancel: &Arc<AtomicBool>,
    local_cancel:  &Arc<AtomicBool>,
) {
    let prompt = format!(
        "You are a log analysis assistant. Summarize the following log excerpt \
         concisely. Focus on: errors, warnings, unusual patterns, key events. \
         Use plain prose, no bullet points, 3-5 sentences maximum.\n\nLog excerpt:\n{text}"
    );
    let model = resolve_if_empty(model, host);
    stream_ollama(&prompt, host, &model, tx, global_cancel, local_cancel);
}

/// Fire a raw prompt — used by dropdown items that compose their own prompt.
pub fn prompt_streaming(
    prompt:        &str,
    host:          &str,
    model:         &str,
    tx:            &crossbeam_channel::Sender<StreamChunk>,
    global_cancel: &Arc<AtomicBool>,
    local_cancel:  &Arc<AtomicBool>,
) {
    let model = resolve_if_empty(model, host);
    stream_ollama(prompt, host, &model, tx, global_cancel, local_cancel);
}

// ── non-streaming wrappers (for pipeline use) ─────────────────────────────────

#[allow(dead_code)]
pub fn explain(lines: &[&str], host: &str, model: &str) -> Result<OllamaResult, String> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let no_cancel = Arc::new(AtomicBool::new(false));
    explain_streaming(lines, host, model, &tx, &no_cancel, &no_cancel);
    collect_stream(rx)
}

#[allow(dead_code)]
pub fn summarize(text: &str, host: &str, model: &str) -> Result<OllamaResult, String> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let no_cancel = Arc::new(AtomicBool::new(false));
    summarize_streaming(text, host, model, &tx, &no_cancel, &no_cancel);
    collect_stream(rx)
}

#[allow(dead_code)]
fn collect_stream(rx: crossbeam_channel::Receiver<StreamChunk>) -> Result<OllamaResult, String> {
    let mut text  = String::new();
    let mut model = String::new();
    for chunk in rx {
        match chunk {
            StreamChunk::Token(t)        => text.push_str(&t),
            StreamChunk::Done { model: m } => { model = m; break; }
            StreamChunk::Cancelled       => break,
            StreamChunk::Error(e)        => return Err(e),
        }
    }
    Ok(OllamaResult { text: text.trim().to_owned(), model, prompt_tokens: 0, response_tokens: 0 })
}

// ── internals ─────────────────────────────────────────────────────────────────

fn resolve_if_empty(model: &str, host: &str) -> String {
    if model.is_empty() { resolve_model(host) } else { model.to_owned() }
}

fn stream_ollama(
    prompt:        &str,
    host:          &str,
    model:         &str,
    tx:            &crossbeam_channel::Sender<StreamChunk>,
    global_cancel: &Arc<AtomicBool>,
    local_cancel:  &Arc<AtomicBool>,
) {
    use std::io::BufRead;

    // Check cancel before even connecting
    if is_cancelled(global_cancel, local_cancel) {
        let _ = tx.send(StreamChunk::Cancelled);
        return;
    }

    let url  = format!("{host}/api/generate");
    let body = serde_json::json!({
        "model":  model,
        "prompt": prompt,
        "stream": true
    });

    let resp = match ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        // No read timeout — streaming can take minutes on slow hardware
        .build()
        .post(&url)
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
    {
        Ok(r) => r,
        Err(ureq::Error::Status(405, _)) => {
            let _ = tx.send(StreamChunk::Error(
                "Ollama 405 — set OLLAMA_ORIGINS='*' and restart: \
                 OLLAMA_ORIGINS='*' ollama serve".into()
            ));
            return;
        }
        Err(e) => {
            let _ = tx.send(StreamChunk::Error(format!("Ollama request failed: {e}")));
            return;
        }
    };

    let reader = std::io::BufReader::new(resp.into_reader());
    let mut final_model = model.to_owned();

    for line in reader.lines() {
        // Check cancel between every chunk
        if is_cancelled(global_cancel, local_cancel) {
            let _ = tx.send(StreamChunk::Cancelled);
            return;
        }

        let line = match line {
            Ok(l)  => l,
            Err(e) => { let _ = tx.send(StreamChunk::Error(e.to_string())); return; }
        };
        if line.is_empty() { continue; }

        let json: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v)  => v,
            Err(_) => continue,
        };

        if let Some(tok) = json["response"].as_str() {
            if !tok.is_empty() {
                let _ = tx.send(StreamChunk::Token(tok.to_owned()));
            }
        }
        if let Some(m) = json["model"].as_str() {
            final_model = m.to_owned();
        }
        if json["done"].as_bool().unwrap_or(false) {
            let _ = tx.send(StreamChunk::Done { model: final_model });
            return;
        }
    }

    let _ = tx.send(StreamChunk::Done { model: final_model });
}

#[inline]
fn is_cancelled(global: &Arc<AtomicBool>, local: &Arc<AtomicBool>) -> bool {
    global.load(Ordering::Relaxed) || local.load(Ordering::Relaxed)
}

// ── model unloading ───────────────────────────────────────────────────────────

/// Ask Ollama to immediately unload a model from VRAM/RAM by sending
/// a generate request with keep_alive=0.  This is a best-effort call —
/// we don't block the UI on it.  Runs on a rayon thread.
pub fn unload_model(host: &str, model: &str) {
    let body = serde_json::json!({
        "model":      model,
        "prompt":     "",
        "keep_alive": 0,
    });
    let _ = ureq::post(&format!("{host}/api/generate"))
        .timeout(std::time::Duration::from_secs(8))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string());
}

/// Unload every model currently loaded according to `ollama ps` (/api/ps).
/// Blocks briefly per model — call from a rayon thread.
pub fn unload_all_models(host: &str) {
    let url = format!("{host}/api/ps");
    if let Ok(resp) = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .call()
    {
        if let Ok(body) = resp.into_json::<serde_json::Value>() {
            if let Some(models) = body["models"].as_array() {
                for m in models {
                    if let Some(name) = m["name"].as_str() {
                        unload_model(host, name);
                    }
                }
            }
        }
    }
}

/// Like list_models but returns full (name, size, size_vram, quant) tuples
/// for use by the provider registry.
#[allow(dead_code)]
pub fn list_models_with_info(host: &str)
    -> Result<Vec<(String, u64, u64, String)>, String>
{
    let url = format!("{host}/api/tags");
    let resp = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .call()
        .map_err(|e| e.to_string())?;
    let body: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    let arr = body["models"].as_array()
        .ok_or_else(|| "no models array".to_string())?;
    Ok(arr.iter().filter_map(|m| {
        let name  = m["name"].as_str()?.to_owned();
        let size  = m["size"].as_u64().unwrap_or(0);
        let vram  = m["size_vram"].as_u64().unwrap_or(0);
        let quant = m["details"]["quantization_level"].as_str().unwrap_or("").to_owned();
        Some((name, size, vram, quant))
    }).collect())
}

// ── Vision / multimodal ───────────────────────────────────────────────────────

/// Returns true if the model name suggests vision/multimodal capability.
pub fn is_vision_model(model: &str) -> bool {
    let m = model.to_lowercase();
    m.contains("llava") || m.contains("vision") || m.contains("moondream")
        || m.contains("bakllava") || m.contains("cogvlm") || m.contains("phi3-vision")
        || m.contains("minicpm") || m.contains("internvl") || m.contains("qwen-vl")
        || m.contains("llama3.2") // llama3.2 11b/90b are vision models
}

/// Returns true if the model is primarily an image-generation model.
/// (Most Ollama models are text; image gen via Ollama is experimental.)
pub fn is_image_gen_model(model: &str) -> bool {
    let m = model.to_lowercase();
    m.contains("sdxl") || m.contains("stable-diffusion") || m.contains("dall-e")
        || m.contains("flux") || m.contains("imagen")
}

/// Encode image bytes as base64 string (no padding header, just the data).
pub fn image_to_base64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Load an image file and base64-encode it for Ollama multimodal messages.
pub fn load_image_base64(path: &std::path::Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(image_to_base64(&bytes))
}

/// Take a screenshot using platform tools and return the path to the saved file.
/// Uses gnome-screenshot → scrot → grim (Wayland) in order of availability.
pub fn take_screenshot(output_path: &std::path::Path) -> Result<(), String> {
    // Try gnome-screenshot (GNOME)
    if let Ok(status) = std::process::Command::new("gnome-screenshot")
        .args(["-f", &output_path.to_string_lossy()])
        .status()
    {
        if status.success() { return Ok(()); }
    }
    // Try grim (Wayland compositors)
    if let Ok(status) = std::process::Command::new("grim")
        .arg(output_path.to_string_lossy().as_ref())
        .status()
    {
        if status.success() { return Ok(()); }
    }
    // Try scrot (X11)
    if let Ok(status) = std::process::Command::new("scrot")
        .arg(output_path.to_string_lossy().as_ref())
        .status()
    {
        if status.success() { return Ok(()); }
    }
    // Try import (ImageMagick)
    if let Ok(status) = std::process::Command::new("import")
        .args(["-window", "root", &output_path.to_string_lossy()])
        .status()
    {
        if status.success() { return Ok(()); }
    }
    Err("No screenshot tool found (tried gnome-screenshot, grim, scrot, import)".to_string())
}

/// Take an interactive area-select screenshot.
pub fn take_screenshot_area(output_path: &std::path::Path) -> Result<(), String> {
    // gnome-screenshot area
    if let Ok(s) = std::process::Command::new("gnome-screenshot")
        .args(["-a", "-f", &output_path.to_string_lossy()])
        .status() { if s.success() { return Ok(()); } }
    // grim + slurp (Wayland)
    if let Ok(region) = std::process::Command::new("slurp").output() {
        if region.status.success() {
            let r = String::from_utf8_lossy(&region.stdout).trim().to_string();
            if let Ok(s) = std::process::Command::new("grim")
                .args(["-g", &r, &output_path.to_string_lossy()])
                .status() { if s.success() { return Ok(()); } }
        }
    }
    // scrot -s (X11 interactive)
    if let Ok(s) = std::process::Command::new("scrot")
        .args(["-s", &output_path.to_string_lossy()])
        .status() { if s.success() { return Ok(()); } }
    Err("No area-screenshot tool found (tried gnome-screenshot -a, grim+slurp, scrot -s)".to_string())
}

/// Build a multimodal Ollama /api/chat message with attached images.
/// `images` is a list of base64-encoded image strings.
pub fn build_multimodal_message(content: &str, images: &[String]) -> serde_json::Value {
    if images.is_empty() {
        serde_json::json!({ "role": "user", "content": content })
    } else {
        serde_json::json!({
            "role": "user",
            "content": content,
            "images": images
        })
    }
}

