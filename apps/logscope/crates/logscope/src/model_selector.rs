//! model_selector.rs — memory-pressure-aware model selection and handoff
//!
//! Design
//! ──────
//! Ollama reports model parameter counts via /api/tags (size_vram field).
//! We also read /proc/meminfo on Linux for available RAM + VRAM proxy.
//!
//! Tiers:
//!   Large  (>= 13B params or > 8 GB VRAM)  — full reasoning, slow
//!   Medium (3B–13B)                          — balanced
//!   Small  (< 3B)                            — fast, low memory, summarization
//!
//! Handoff strategy
//! ────────────────
//!   1. Main chat always uses the "primary" model (user's explicit choice or
//!      largest available that fits in free RAM).
//!   2. Sub-sessions (snippet explanations from peek/cargo) use the "delegate"
//!      model — the smallest model that can handle the task.
//!   3. When memory pressure is HIGH (free < 20% of total), we switch the
//!      primary model down to the next tier and generate a summary of the
//!      conversation using the smaller model to preserve context.
//!   4. The original large model is re-selected when pressure drops.
//!
//! Context compression
//! ───────────────────
//!   When handing off to a smaller model, we compress the conversation history
//!   into a summary prompt injected as a System message. The full history stays
//!   on disk (conversation_db); only the summary is in the active context window.


// ── model metadata ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelTier { Small, Medium, Large }

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub name:         String,
    pub tier:         ModelTier,
    /// Disk footprint in bytes (from /api/tags `size`)
    pub size_bytes:   u64,
    /// VRAM footprint when loaded (from /api/tags `size_vram`, 0 if not loaded)
    pub size_vram:    u64,
    /// Parameter count (millions, e.g. 7000 = 7B)
    pub params_m:     u64,
    /// The host this model was discovered on ("" = local)
    pub host:         String,
    /// If true this is the "autohost" virtual entry — pick best host at runtime
    pub autohost:     bool,
    /// Quantization label from /api/tags details (e.g. "Q4_K_M")
    #[allow(dead_code)]
    pub quant:        String,
}

impl ModelInfo {
    pub fn from_ollama_tag(name: &str, size_bytes: u64) -> Self {
        Self::from_ollama_entry(name, size_bytes, 0, "", "")
    }

    /// Build from a full /api/tags model JSON entry.
    pub fn from_ollama_entry(
        name: &str, size_bytes: u64, size_vram: u64, quant: &str, host: &str,
    ) -> Self {
        let params_m = estimate_params_from_name(name);
        let tier = if params_m >= 13_000 || size_bytes >= 8 * 1024 * 1024 * 1024 {
            ModelTier::Large
        } else if params_m >= 3_000 || size_bytes >= 2 * 1024 * 1024 * 1024 {
            ModelTier::Medium
        } else {
            ModelTier::Small
        };
        ModelInfo {
            name: name.to_owned(), tier, size_bytes, size_vram,
            params_m, host: host.to_owned(), autohost: false,
            quant: quant.to_owned(),
        }
    }

    /// The internal key stored in the StringList: "name" for local autohost,
    /// "name@http://host" for pinned-host, "name@auto" for auto-select.
    pub fn store_key(&self) -> String {
        if self.autohost {
            format!("{}@auto", self.name)
        } else if self.host.is_empty() {
            self.name.clone()
        } else {
            format!("{}@{}", self.name, self.host)
        }
    }
}

fn estimate_params_from_name(name: &str) -> u64 {
    // Extract numbers like "7b", "13b", "70b", "1.5b", "0.5b" from model names
    let lower = name.to_lowercase();
    // Common patterns: llama3:7b, mistral:7b, phi3:mini, tinyllama:1.1b
    for part in lower.split(|c: char| !c.is_alphanumeric() && c != '.') {
        if let Some(stripped) = part.strip_suffix('b') {
            if let Ok(n) = stripped.parse::<f64>() {
                return (n * 1000.0) as u64;
            }
        }
    }
    // Known model families
    if lower.contains("tiny") || lower.contains("0.5") { return 500; }
    if lower.contains("mini") || lower.contains("1.1") { return 1100; }
    if lower.contains("phi3")  { return 3800; }
    if lower.contains("gemma") { return 7000; }
    // Unknown — assume medium
    7000
}

// ── memory pressure ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MemPressure {
    Low,    // > 40% free
    Medium, // 20–40% free
    High,   // < 20% free
}

pub fn read_mem_pressure() -> MemPressure {
    // Linux: read /proc/meminfo
    if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
        let mut total: u64 = 0;
        let mut available: u64 = 0;
        for line in content.lines() {
            if line.starts_with("MemTotal:") {
                total = parse_kb_line(line);
            } else if line.starts_with("MemAvailable:") {
                available = parse_kb_line(line);
            }
        }
        if total > 0 {
            let free_pct = available as f64 / total as f64;
            return if free_pct > 0.40 { MemPressure::Low }
                   else if free_pct > 0.20 { MemPressure::Medium }
                   else { MemPressure::High };
        }
    }
    MemPressure::Low // fallback
}

fn parse_kb_line(line: &str) -> u64 {
    line.split_whitespace().nth(1)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
        * 1024
}

pub fn read_free_bytes() -> u64 {
    if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
        for line in content.lines() {
            if line.starts_with("MemAvailable:") {
                return parse_kb_line(line);
            }
        }
    }
    u64::MAX
}

// ── selector ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ModelSelector {
    pub models:       Vec<ModelInfo>,
    pub primary_idx:  usize,
    pub delegate_idx: usize,
}

impl ModelSelector {
    pub fn new(models: Vec<ModelInfo>) -> Self {
        if models.is_empty() {
            return ModelSelector { models: vec![
                ModelInfo { name: "tinyllama".into(), tier: ModelTier::Small, size_bytes: 0, size_vram: 0, params_m: 1100, host: String::new(), autohost: false, quant: String::new() }
            ], primary_idx: 0, delegate_idx: 0 };
        }

        // Auto-select: largest model that fits in 60% of free RAM
        let free = read_free_bytes();
        let budget = (free as f64 * 0.6) as u64;

        let primary_idx = models.iter().enumerate()
            .filter(|(_, m)| m.size_bytes <= budget || m.size_bytes == 0)
            .max_by_key(|(_, m)| m.params_m)
            .map(|(i, _)| i)
            .unwrap_or(0);

        // Delegate: smallest available (for summarization, sub-tasks)
        let delegate_idx = models.iter().enumerate()
            .min_by_key(|(_, m)| m.params_m)
            .map(|(i, _)| i)
            .unwrap_or(0);

        ModelSelector { models, primary_idx, delegate_idx }
    }

    pub fn primary(&self) -> &ModelInfo { &self.models[self.primary_idx] }
    pub fn delegate(&self) -> &ModelInfo { &self.models[self.delegate_idx] }

    /// React to pressure change. Returns true if model changed.
    pub fn adapt_to_pressure(&mut self, pressure: MemPressure) -> bool {
        match pressure {
            MemPressure::Low | MemPressure::Medium => false,
            MemPressure::High => {
                // Downgrade primary to next smaller tier
                let current_tier = &self.models[self.primary_idx].tier;
                let better = self.models.iter().enumerate()
                    .filter(|(i, m)| *i != self.primary_idx && m.tier < *current_tier)
                    .max_by_key(|(_, m)| m.params_m)
                    .map(|(i, _)| i);
                if let Some(idx) = better {
                    self.primary_idx = idx;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Set primary explicitly (user picked from dropdown).
    pub fn set_primary_by_name(&mut self, name: &str) {
        if let Some(idx) = self.models.iter().position(|m| m.name == name) {
            self.primary_idx = idx;
        }
    }

    pub fn set_delegate_by_name(&mut self, name: &str) {
        if let Some(idx) = self.models.iter().position(|m| m.name == name) {
            self.delegate_idx = idx;
        }
    }

    /// Build a summary prompt for context compression when handing off.
    #[allow(dead_code)]
    pub fn compression_system_prompt(history_summary: &str, current_model: &str) -> String {
        format!(
            "You are continuing a conversation that was started with model '{current_model}'. \
             Here is a compressed summary of what was discussed:\n\n{history_summary}\n\n\
             Continue from this context, maintaining consistency with the previous answers."
        )
    }
}

// ── file tokenization ─────────────────────────────────────────────────────────

/// Rough token count estimate: GPT-family models use ~4 chars/token on average
/// for English; code is closer to 3.5. We use 4 as a conservative estimate.
pub fn estimate_tokens(text: &str) -> usize {
    (text.len() + 3) / 4
}

/// Compress a file or large text into a context-friendly representation.
/// For files > `max_tokens`, extracts: first N lines, last N lines, and
/// a frequency-based middle sample. Returns the compressed text + token estimate.
pub fn compress_for_context(text: &str, max_tokens: usize) -> (String, usize) {
    let tokens = estimate_tokens(text);
    if tokens <= max_tokens {
        return (text.to_owned(), tokens);
    }

    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len();

    // Budget: 30% head, 30% tail, 40% sampled middle
    let head_lines  = ((max_tokens as f64 * 0.30) / 40.0) as usize; // ~40 chars/line avg
    let tail_lines  = ((max_tokens as f64 * 0.30) / 40.0) as usize;
    let mid_lines   = ((max_tokens as f64 * 0.40) / 40.0) as usize;

    let head_lines  = head_lines.min(n / 3);
    let tail_lines  = tail_lines.min(n / 3);

    let mut out = String::new();

    // Head
    if head_lines > 0 {
        out.push_str(&format!("── first {} lines ──\n", head_lines));
        for l in &lines[..head_lines] { out.push_str(l); out.push('\n'); }
    }

    // Sampled middle (every Nth line)
    let mid_start = head_lines;
    let mid_end   = n.saturating_sub(tail_lines);
    if mid_end > mid_start && mid_lines > 0 {
        let step = ((mid_end - mid_start) / mid_lines).max(1);
        out.push_str(&format!("\n── sampled middle ({} of {} lines, every {}) ──\n",
            mid_lines.min((mid_end-mid_start)/step), mid_end-mid_start, step));
        let mut i = mid_start;
        let mut shown = 0;
        while i < mid_end && shown < mid_lines {
            out.push_str(lines[i]); out.push('\n');
            i += step; shown += 1;
        }
    }

    // Tail
    if tail_lines > 0 && n > tail_lines {
        out.push_str(&format!("\n── last {} lines ──\n", tail_lines));
        for l in &lines[n - tail_lines..] { out.push_str(l); out.push('\n'); }
    }

    let compressed_tokens = estimate_tokens(&out);
    out.push_str(&format!("\n[Compressed from {} to ~{} tokens]\n",
        tokens, compressed_tokens));

    (out, compressed_tokens)
}

/// Generate a conversation summary using the current history.
/// Returns a compact prose summary for context compression.
pub fn summarize_history(messages: &[(&str, &str)]) -> String {
    // Simple extractive summary: collect assistant turns, truncate each
    let mut summary = String::from("Conversation summary:\n");
    for (role, content) in messages {
        if *role == "assistant" {
            let snippet: String = content.chars().take(200).collect();
            summary.push_str("• ");
            summary.push_str(snippet.trim());
            if content.len() > 200 { summary.push_str("…"); }
            summary.push('\n');
        }
    }
    summary
}

// ── fake MoE summarization ────────────────────────────────────────────────────
//
// Strategy: large model does focused analysis (minimal tokens), its output is
// passed to N small models as independent "experts", then a final small-model
// pass consolidates. This avoids copying large model state — each call is
// stateless from Ollama's perspective.
//
// The large model is prompted for SHORT structured output (JSON-ish key points)
// so its slow token generation doesn't dominate. Small models then elaborate
// quickly from these hints.

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

/// A single expert result from a small model.
#[derive(Debug, Clone)]
pub struct ExpertOpinion {
    pub model:   String,
    pub content: String,
}

/// Run the fake MoE pipeline:
///   1. Large model → short key-point analysis
///   2. Each small model → expand on those key points
///   3. Consolidator (smallest model) → final summary
///
/// Returns the final consolidated summary.
/// All calls are blocking — run on a rayon thread.
pub fn moe_summarize(
    host:         &str,
    large_model:  &str,
    small_models: &[&str], // usually 2-3 small models
    topic:        &str,    // the text/conversation to summarize
    cancel:       &Arc<AtomicBool>,
) -> Result<String, String> {
    if cancel.load(Ordering::Relaxed) { return Err("cancelled".into()); }

    // Step 1: Large model extracts key points (capped at ~200 tokens)
    let large_prompt = format!(
        "Extract the 5 most important technical points from the following as a \
         numbered list. Be extremely concise — one short sentence per point. \
         No preamble.\n\n{topic}"
    );
    let key_points = blocking_generate(host, large_model, &large_prompt, cancel)?;
    if cancel.load(Ordering::Relaxed) { return Err("cancelled".into()); }

    if small_models.is_empty() {
        return Ok(key_points);
    }

    // Step 2: Each small model elaborates from key points in parallel
    // We use crossbeam channels to collect results without blocking serially
    let (tx, rx) = crossbeam_channel::unbounded::<ExpertOpinion>();
    let key_points_arc = Arc::new(key_points.clone());
    let topic_arc = Arc::new(topic.to_owned());

    for &sm in small_models {
        if cancel.load(Ordering::Relaxed) { break; }
        let tx2 = tx.clone();
        let h   = host.to_owned();
        let m   = sm.to_owned();
        let kp  = Arc::clone(&key_points_arc);
        let tp  = Arc::clone(&topic_arc);
        let c   = Arc::clone(cancel);
        rayon::spawn(move || {
            let prompt = format!(
                "Given these key points from a technical analysis:\n{kp}\n\n\
                 And the original context:\n{tp}\n\n\
                 Add any important nuance or corrections in 2-3 sentences. \
                 Focus only on what the key points missed."
            );
            if let Ok(opinion) = blocking_generate(&h, &m, &prompt, &c) {
                let _ = tx2.send(ExpertOpinion { model: m, content: opinion });
            }
        });
    }
    drop(tx); // close the sender side

    // Collect expert opinions (with timeout)
    let mut opinions: Vec<ExpertOpinion> = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(op) => { opinions.push(op); }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if cancel.load(Ordering::Relaxed) || std::time::Instant::now() > deadline { break; }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
    if cancel.load(Ordering::Relaxed) { return Err("cancelled".into()); }

    if opinions.is_empty() {
        return Ok(key_points); // fallback to just the large model output
    }

    // Step 3: Consolidator (use the smallest / first small model)
    let consolidator = small_models.last().copied().unwrap_or(small_models[0]);
    let expert_text: String = opinions.iter()
        .map(|op| format!("[{}]: {}", op.model, op.content))
        .collect::<Vec<_>>()
        .join("\n\n");

    let final_prompt = format!(
        "Synthesize the following expert opinions into a single coherent summary \
         of 3-5 sentences. Resolve contradictions by preferring the most specific claim.\n\n\
         Key points:\n{key_points}\n\nExpert opinions:\n{expert_text}"
    );

    blocking_generate(host, consolidator, &final_prompt, cancel)
}

/// Blocking Ollama /api/generate call — returns the response text.
/// Call only from rayon threads, never from the GTK main thread.
#[allow(dead_code)]
fn blocking_generate(
    host:   &str,
    model:  &str,
    prompt: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<String, String> {
    if cancel.load(Ordering::Relaxed) { return Err("cancelled".into()); }

    let model = if model.is_empty() { resolve_model_local(host) } else { model.to_owned() };
    let body = serde_json::json!({
        "model":  model,
        "prompt": prompt,
        "stream": false,
        "options": { "num_predict": 300 } // cap output tokens for speed
    });

    let resp = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .build()
        .post(&format!("{host}/api/generate"))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| e.to_string())?;

    let json: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    Ok(json["response"].as_str().unwrap_or("").trim().to_owned())
}

// ── model resolution ──────────────────────────────────────────────────────────

/// Pick the first available model from the Ollama host, falling back to tinyllama.
/// Mirrors ui::ollama::resolve_model but lives here so model_selector.rs doesn't
/// depend on the UI crate tree.
#[allow(dead_code)]
fn resolve_model_local(host: &str) -> String {
    let url = format!("{host}/api/tags");
    if let Ok(resp) = ureq::get(&url).timeout(std::time::Duration::from_secs(5)).call() {
        if let Ok(body) = resp.into_json::<serde_json::Value>() {
            if let Some(name) = body["models"].as_array()
                .and_then(|a| a.first())
                .and_then(|m| m["name"].as_str())
            {
                return name.to_owned();
            }
        }
    }
    "tinyllama".into()
}

// ── tiered summarization (large→small handoff + fake MoE) ────────────────────
//
// Strategy:
//   1. Large model produces a minimal "planning note" (bullet points only,
//      capped at ~200 tokens) describing key facts and next reasoning steps.
//   2. Two or more small models each receive: (system context) + (planning note)
//      + (original question) and produce independent short answers.
//   3. A final pass by the *delegate* model reads all small-model outputs and
//      synthesises a single coherent answer.
//
// This way the large model never outputs prose (fast), the small models do the
// verbose work in parallel, and a tiny final pass unifies them.
// The large model's context window is never polluted by the verbose output.


/// One planning note from the large model, passed to small models.
#[derive(Debug, Clone)]
pub struct PlanningNote {
    pub model:   String,
    pub content: String,
    pub tokens:  usize,
}

/// Result from one small-model "expert" pass.
#[derive(Debug, Clone)]
pub struct ExpertResult {
    pub model:   String,
    pub content: String,
}

/// Build the minimal planning prompt for the large model.
/// Instructs it to output ONLY bullet points, no prose.
pub fn planning_prompt(user_question: &str, context_summary: &str) -> String {
    format!(
        "You are a planning assistant. Output ONLY a numbered bullet list (maximum 8 items, \
         50 words each). No prose. No explanations beyond the bullets. \
         List key facts, constraints, and reasoning steps needed to answer:\n\n\
         Context: {context_summary}\n\nQuestion: {user_question}"
    )
}

/// Build the expert prompt for a small model given the planning note.
pub fn expert_prompt(user_question: &str, planning_note: &str, expert_index: usize) -> String {
    format!(
        "You are expert #{expert_index}. Using the planning note below, answer the question \
         concisely (3-5 sentences). Focus on a different angle from other experts.\n\n\
         Planning note:\n{planning_note}\n\nQuestion: {user_question}"
    )
}

/// Build the synthesis prompt — merges expert outputs into a final answer.
pub fn synthesis_prompt(user_question: &str, expert_outputs: &[ExpertResult]) -> String {
    let mut experts = String::new();
    for (i, e) in expert_outputs.iter().enumerate() {
        experts.push_str(&format!("Expert {} ({}):\n{}\n\n", i+1, e.model, e.content));
    }
    format!(
        "Synthesise the following expert answers into one clear, concise response. \
         Remove redundancy, resolve contradictions by preferring consensus, keep it \
         under 200 words.\n\nQuestion: {user_question}\n\n{experts}"
    )
}

/// Decide how many expert passes to run based on model count and memory.
pub fn expert_count(selector: &ModelSelector) -> usize {
    let small_count = selector.models.iter()
        .filter(|m| m.tier == ModelTier::Small)
        .count();
    small_count.clamp(1, 3)
}

// ── realtime speed tracking ───────────────────────────────────────────────────
//
// We measure first-token latency (ms) and tokens/sec for every model+host
// combination and store a rolling average.  This gives a real empirical
// speed score independent of parameter count or VRAM size.
//
// The score is used by ModelSelector to prefer the fastest available host
// when the same model is available on multiple machines.

// SpeedMap is always HashMap<String, SpeedSample>

/// Rolling speed stats for a single (model, host) pair.
#[derive(Debug, Clone, Default)]
pub struct SpeedSample {
    /// First-token latency in ms (lower = better)
    pub ttft_ms:    f64,
    /// Tokens per second (higher = better)
    pub tps:        f64,
    /// Number of samples averaged
    pub n:          u32,
    /// VRAM currently in use on this host (from /api/ps), 0 = not loaded
    pub vram_bytes: u64,
    /// Whether the model is currently loaded in memory
    pub loaded:     bool,
}

impl SpeedSample {
    /// Update with a new measurement using exponential moving average (alpha=0.3).
    pub fn update(&mut self, ttft_ms: f64, tps: f64) {
        if self.n == 0 {
            self.ttft_ms = ttft_ms;
            self.tps     = tps;
        } else {
            let alpha = 0.3f64;
            self.ttft_ms = alpha * ttft_ms + (1.0 - alpha) * self.ttft_ms;
            self.tps     = alpha * tps     + (1.0 - alpha) * self.tps;
        }
        self.n += 1;
    }

    /// Composite score: higher is better.
    /// Weighted: 60% tokens/sec, 40% inverse of first-token latency.
    /// A loaded model gets a 2x bonus (no cold-start cost).
    pub fn score(&self) -> f64 {
        if self.n == 0 { return 0.0; }
        let tps_norm  = self.tps.min(200.0) / 200.0;       // normalised 0-1
        let ttft_norm = 1.0 - (self.ttft_ms.min(30_000.0) / 30_000.0);
        let base = 0.6 * tps_norm + 0.4 * ttft_norm;
        if self.loaded { base * 2.0 } else { base }
    }
}

/// Global speed table, keyed by "model@http://host:port".
/// Shared via Arc<Mutex<>> so background threads can write measurements.
pub type SpeedTable = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, SpeedSample>>>;

pub fn new_speed_table() -> SpeedTable {
    std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Record a completed streaming call into the speed table.
pub fn record_speed(
    table:    &SpeedTable,
    model:    &str,
    host:     &str,
    ttft_ms:  f64,
    tps:      f64,
) {
    let key = format!("{model}@{host}");
    if let Ok(mut t) = table.lock() {
        let e: &mut SpeedSample = t.entry(key).or_default();
        e.update(ttft_ms, tps);
    }
}

/// Update loaded/VRAM status from /api/ps response.
pub fn update_ps_status(table: &SpeedTable, host: &str, ps_body: &serde_json::Value) {
    if let Some(models) = ps_body["models"].as_array() {
        if let Ok(mut t) = table.lock() {
            let t: &mut std::collections::HashMap<String, SpeedSample> = &mut *t;
            for m in models {
                let name  = m["name"].as_str().unwrap_or("");
                let vram  = m["size_vram"].as_u64().unwrap_or(0);
                let key   = format!("{name}@{host}");
                let entry: &mut SpeedSample = t.entry(key).or_default();
                entry.vram_bytes = vram;
                entry.loaded     = vram > 0;
            }
        }
    }
}

/// Given a model name (without @host), return the best host to use
/// based on speed scores. Falls back to `default_host` if no data.
pub fn best_host_for_model(
    table:        &SpeedTable,
    model:        &str,
    candidates:   &[String],   // list of known hosts
    default_host: &str,
) -> String {
    let t: std::sync::MutexGuard<std::collections::HashMap<String, SpeedSample>> = match table.lock() { Ok(t) => t, Err(_) => return default_host.to_owned() };
    candidates.iter()
        .max_by(|a, b| {
            let ka = format!("{model}@{a}");
            let kb = format!("{model}@{b}");
            let sa = t.get(&ka).map(|s: &SpeedSample| s.score()).unwrap_or(0.0);
            let sb = t.get(&kb).map(|s: &SpeedSample| s.score()).unwrap_or(0.0);
            sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned()
        .unwrap_or_else(|| default_host.to_owned())
}
