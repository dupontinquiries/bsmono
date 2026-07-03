// src/system_info.rs
// Lightweight system queries for voice commands.
// All implementations use only std + shelling to common Unix tools.

use std::fs;
use std::process::Command;

/// Compute days until a target date description like "christmas", "may 15", "next friday"
pub fn days_until(target: &str) -> String {
    match resolve_date(target) {
        Some(target_days) => {
            let now = days_since_epoch();
            let diff = target_days - now;
            if diff < 0 {
                format!("{} was {} days ago", target, -diff)
            } else if diff == 0 {
                format!("{} is today!", target)
            } else if diff == 1 {
                format!("{} is tomorrow", target)
            } else {
                format!("{} days until {}", diff, target)
            }
        }
        None => format!("Couldn't parse \"{}\"", target),
    }
}

pub fn days_since(target: &str) -> String {
    match resolve_date(target) {
        Some(target_days) => {
            let now = days_since_epoch();
            let diff = now - target_days;
            if diff < 0 {
                format!("{} is {} days from now", target, -diff)
            } else if diff == 0 {
                format!("{} is today", target)
            } else {
                format!("{} days since {}", diff, target)
            }
        }
        None => format!("Couldn't parse \"{}\"", target),
    }
}

pub fn current_time() -> String {
    // Use `date` command for proper local timezone
    if let Ok(out) = Command::new("date").arg("+%I:%M %p").output() {
        if out.status.success() {
            return String::from_utf8_lossy(&out.stdout).trim().to_string();
        }
    }
    // Fallback: UTC HH:MM
    let secs = epoch_secs();
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    format!("{:02}:{:02} UTC", h, m)
}

pub fn current_date() -> String {
    if let Ok(out) = Command::new("date").arg("+%A, %B %d, %Y").output() {
        if out.status.success() {
            return String::from_utf8_lossy(&out.stdout).trim().to_string();
        }
    }
    let days = days_since_epoch();
    format!("{} days since Jan 1 1970", days)
}

pub fn battery() -> String {
    // /sys/class/power_supply on Linux
    for ps in &["BAT0", "BAT1"] {
        let cap_path = format!("/sys/class/power_supply/{}/capacity", ps);
        let stat_path = format!("/sys/class/power_supply/{}/status", ps);
        if let Ok(cap) = fs::read_to_string(&cap_path) {
            let status = fs::read_to_string(&stat_path).unwrap_or("Unknown".into());
            return format!("{}% ({})", cap.trim(), status.trim());
        }
    }
    // macOS fallback
    if let Ok(out) = Command::new("pmset").args(&["-g", "batt"]).output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = s.lines().nth(1) {
                return line.trim().to_string();
            }
        }
    }
    "Battery info unavailable".into()
}

pub fn disk_space() -> String {
    if let Ok(out) = Command::new("df").args(&["-h", "/"]).output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = s.lines().nth(1) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 5 {
                    return format!("{} free of {} ({} used)", parts[3], parts[1], parts[4]);
                }
            }
        }
    }
    "Disk info unavailable".into()
}

pub fn memory_usage() -> String {
    if let Ok(content) = fs::read_to_string("/proc/meminfo") {
        // v0.1.602: parsing extracted to the pure, tested sysmon module.
        if let Some((used_kb, total_kb)) = crate::sysmon::parse_meminfo_used_total(&content) {
            return crate::sysmon::format_mem_usage(used_kb, total_kb);
        }
    }
    "Memory info unavailable".into()
}

/// Compact resource summary suitable for a small floating overlay near
/// the terminal cursor.
///
/// Event-driven: returns an empty string MOST of the time. Only emits a
/// non-empty summary when something notable just happened — a memory
/// jump, CPU spike, or sustained pressure crossing a threshold. This
/// keeps the UI quiet during normal use; the overlay flashes in only
/// when the user might actually want to know.
///
/// State is tracked in a process-local Mutex<LastSample>. Caller polls
/// this on a 2s timer; most calls return empty (no UI update). The
/// returned string is short — fits in a 140px wide overlay.
///
/// Conditions that produce a non-empty result:
///   - Memory used % changed by ≥ 5 percentage points since last call
///   - Memory used % crossed 80% (entering or leaving pressure zone)
///   - 1-min load crossed 1.0 from below (CPU getting busy)
///   - 1-min load fell below 0.5 from above 1.0 (CPU calmed down)
///   - Memory > 90% (always shown — high alert)
///   - Load > 4.0 (always shown — heavy CPU)
///
/// The string is prefixed with a sigil indicating direction:
///   ↑  rising / new pressure
///   ↓  falling / pressure released
///   !  high alert (sustained)
pub fn resource_overlay() -> String {
    use std::sync::OnceLock;
    use std::sync::Mutex;

    #[derive(Default, Clone, Copy)]
    struct LastSample {
        load_1min: f32,
        mem_pct: u32,
        had_sample: bool,
    }
    static LAST: OnceLock<Mutex<LastSample>> = OnceLock::new();

    // Read /proc/loadavg + /proc/meminfo; parsing is in the pure,
    // tested sysmon module (v0.1.602).
    let load_1min: f32 = fs::read_to_string("/proc/loadavg")
        .ok()
        .map(|s| crate::sysmon::parse_loadavg_1min(&s))
        .unwrap_or(0.0);
    let mem_pct: u32 = fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|content| crate::sysmon::parse_meminfo_pct(&content))
        .unwrap_or(0);

    let mutex = LAST.get_or_init(|| Mutex::new(LastSample::default()));
    let mut last = match mutex.lock() {
        Ok(g) => g,
        Err(_) => return String::new(),
    };

    let prev = *last;
    *last = LastSample { load_1min, mem_pct, had_sample: true };

    // The event-driven decision is the pure, tested sysmon::overlay_decision.
    crate::sysmon::overlay_decision(prev.had_sample, prev.load_1min, prev.mem_pct, load_1min, mem_pct)
        .unwrap_or_default()
}

/// Structured form of resource_overlay for clients that want to render
/// their own visual (e.g. bars / dots / no labels). Returns the same
/// event-driven decision logic but as a tuple instead of formatted text:
///   (load_1min, mem_pct, kind)
/// where `kind` is:
///   1 = rising (memory or CPU just went up notably)
///   2 = falling (memory or CPU just calmed down)
///   3 = alert (sustained high memory > 90% or load > 4)
///
/// Returns None when nothing notable just happened.
///
/// IMPORTANT: this internally calls `resource_overlay()` which updates
/// the shared LastSample state. Callers should pick ONE entry point —
/// either text (`resource_overlay`) or raw (`resource_overlay_raw`) —
/// and call it consistently. Calling both alternately means the first
/// consumes the event and the second sees nothing.
pub fn resource_overlay_raw() -> Option<(f32, u32, u8)> {
    // Reuse the formatted version's logic by inspecting its output.
    // This is mildly inefficient (formatting the string we then throw
    // away) but keeps the threshold logic single-sourced. Performance
    // is fine because /proc reads are sub-100µs.
    let formatted = resource_overlay();
    if formatted.is_empty() { return None; }

    // Re-read /proc to get the current values. We could parse them out
    // of `formatted` but re-reading is simpler and the cost is trivial.
    let load_1min: f32 = fs::read_to_string("/proc/loadavg").ok()
        .and_then(|s| s.split_whitespace().next().and_then(|s| s.parse().ok()))
        .unwrap_or(0.0);
    let mem_pct: u32 = fs::read_to_string("/proc/meminfo").ok().map(|content| {
        let mut total_kb = 0u64;
        let mut avail_kb = 0u64;
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                total_kb = rest.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                avail_kb = rest.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
            }
        }
        if total_kb > 0 {
            // v0.1.468: saturating — avoid underflow if avail>total.
            (total_kb.saturating_sub(avail_kb.min(total_kb)) * 100 / total_kb) as u32
        } else { 0 }
    }).unwrap_or(0);

    // Map first character of formatted string to kind code.
    let kind = if formatted.starts_with('!') { 3u8 }
        else if formatted.starts_with('\u{2191}') { 1u8 }   // ↑
        else if formatted.starts_with('\u{2193}') { 2u8 }   // ↓
        else { 1u8 };  // default to "rising" as a safe fallback

    Some((load_1min, mem_pct, kind))
}

/// v0.1.533: Ollama is now an OPT-IN fallback, OFF by default. The
/// default LLM path is the in-process backends (llama.cpp / Claude);
/// Ollama runs only if the user enables "Ollama fallback" in settings.
/// This stops the Ollama path from consuming processing space by
/// default — no `which ollama` / `ollama list` spawns, no escalation
/// cascade — unless the user explicitly wants it. Mirrors the
/// is_app_backgrounded atomic pattern; set from the persisted
/// `ollama_fallback_enabled=` setting at startup.
static OLLAMA_FALLBACK_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Enable/disable the Ollama fallback at runtime (driven by the
/// settings toggle + the persisted value loaded at startup).
pub fn set_ollama_fallback_enabled(on: bool) {
    OLLAMA_FALLBACK_ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the user has opted into the Ollama fallback.
pub fn ollama_fallback_enabled() -> bool {
    OLLAMA_FALLBACK_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Check if `ollama` binary is on PATH for LLM fallback.
///
/// v0.1.533: returns false unless the user opted into the Ollama
/// fallback (see OLLAMA_FALLBACK_ENABLED). When disabled, we don't even
/// spawn `which` — the whole Ollama path is inert by default, which is
/// the processing-cleanup win. When enabled, the original PATH check
/// applies.
pub fn has_ollama() -> bool {
    if !ollama_fallback_enabled() {
        return false;
    }
    Command::new("which").arg("ollama").output()
        .map(|o| o.status.success()).unwrap_or(false)
}

/// List installed Ollama models by parsing `ollama list` output.
///
/// Returns model names (e.g. "gemma:2b", "QyrouNnet/summarizer:400m-q8_0")
/// or empty Vec on failure. Cached for 60 seconds to avoid spawning
/// `ollama list` repeatedly.
///
/// Used by the summarizer to discover purpose-built summarizer models
/// the user installs (e.g. QyrouNnet/summarizer:400m-q8_0). Without
/// this, our preferred-model list was hardcoded to gemma:2b/phi3:mini
/// /llama3.2:1b/tinyllama and never tried newly-pulled summarizers.
pub fn list_installed_models() -> Vec<String> {
    use std::sync::Mutex;
    use std::time::{Instant, Duration};
    static CACHE: Mutex<Option<(Instant, Vec<String>)>> = Mutex::new(None);

    if let Ok(g) = CACHE.lock() {
        if let Some((when, ref models)) = *g {
            if when.elapsed() < Duration::from_secs(60) {
                return models.clone();
            }
        }
    }

    let mut out = Vec::new();
    if !has_ollama() {
        return out;
    }
    let result = Command::new("ollama").arg("list").output();
    if let Ok(o) = result {
        if o.status.success() {
            let txt = String::from_utf8_lossy(&o.stdout);
            // Skip header line ("NAME ID SIZE MODIFIED") and parse
            // each subsequent line's first whitespace-separated token.
            for (i, line) in txt.lines().enumerate() {
                if i == 0 { continue; }  // header
                let trimmed = line.trim();
                if trimmed.is_empty() { continue; }
                if let Some(name) = trimmed.split_whitespace().next() {
                    out.push(name.to_string());
                }
            }
        }
    }
    if let Ok(mut g) = CACHE.lock() {
        *g = Some((Instant::now(), out.clone()));
    }
    out
}

/// Find the best summarizer model among installed ones.
///
/// Priority:
///   1. Models with "summariz" in the name (purpose-built summarizers
///      like QyrouNnet/summarizer:400m-q8_0). User explicitly pulled
///      these for summarization, treat as the strongest signal.
///   2. The hardcoded preference order (gemma:2b → phi3:mini →
///      llama3.2:1b → tinyllama) as fallback for users who haven't
///      pulled a dedicated summarizer.
///
/// Returns the model list in priority order. Caller iterates trying
/// each until one succeeds.
pub fn summarizer_priority_list() -> Vec<String> {
    let installed = list_installed_models();
    let mut out: Vec<String> = Vec::new();

    // 1. Purpose-built summarizers: any installed model matching
    //    "summariz" (case-insensitive). Multiple variants get sorted
    //    smaller-to-larger by quant suffix when possible — q4 is
    //    faster, q8 is more accurate; for our short summaries q4 is
    //    usually sufficient. We just take whatever the user pulled
    //    in installation order; if they have both q4 and q8, both
    //    get tried. Better than nothing.
    for m in &installed {
        let lc = m.to_ascii_lowercase();
        if lc.contains("summariz") {
            out.push(m.clone());
        }
    }

    // 2. Hardcoded fallbacks (must be installed to add — unlike before
    //    where we'd try them all and let ollama report "model not
    //    pulled", which adds 12s of timeout per missing one).
    let fallback_pref = ["gemma:2b", "phi3:mini", "llama3.2:1b", "tinyllama"];
    for &m in &fallback_pref {
        if installed.iter().any(|im| im == m || im.starts_with(&format!("{}:", m.split(':').next().unwrap_or("")))) {
            if !out.contains(&m.to_string()) {
                out.push(m.to_string());
            }
        }
    }

    // If we've discovered no models at all (ollama listing failed?),
    // fall back to the original hardcoded list and let it discover
    // failures live. Better to try and fail than to give up.
    if out.is_empty() {
        for &m in &fallback_pref {
            out.push(m.to_string());
        }
    }

    out
}

/// Detect total system RAM in GB (Linux). Returns None if unknown.
pub fn ram_gb() -> Option<u32> {
    if let Ok(content) = fs::read_to_string("/proc/meminfo") {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                if let Some(kb_str) = rest.split_whitespace().next() {
                    if let Ok(kb) = kb_str.parse::<u64>() {
                        return Some((kb / 1_048_576) as u32);
                    }
                }
            }
        }
    }
    None
}

/// Choose how many parallel LLM queries to run based on system RAM.
/// Phone (4-8GB):  1 model
/// Laptop (8-16): 1-3 models
/// Desktop (16-32): 3-5 models
/// Workstation (>32GB): 5-10 models
pub fn parallel_llm_count() -> usize {
    let ram = ram_gb().unwrap_or(8);
    match ram {
        0..=8 => 1,
        9..=16 => 2,
        17..=32 => 3,
        33..=64 => 5,
        _ => 8,
    }
}

/// List models that are likely available locally (must be pulled in ollama).
/// Order by speed. tinyllama is ~640MB, gemma:2b is ~1.4GB, phi3:mini ~2.3GB
pub fn preferred_models() -> Vec<&'static str> {
    let ram = ram_gb().unwrap_or(8);
    if ram <= 8 {
        vec!["tinyllama"]
    } else if ram <= 16 {
        vec!["tinyllama", "gemma:2b"]
    } else {
        vec!["tinyllama", "gemma:2b", "phi3:mini"]
    }
}

/// Query ollama with a model. Default to tinyllama (fast, ~640MB)
pub fn ollama_query(prompt: &str) -> Result<String, String> {
    let model = std::env::var("WEB2QML_OLLAMA_MODEL").unwrap_or_else(|_| "tinyllama".into());
    ollama_query_model(&model, prompt)
}

/// Per-model timeout in seconds. Prevents one slow model from blocking everything.
/// Bumped from 4s to 8s in v0.1.73 after testing showed several real-world
/// prompts (esp. multi-line questions from "ask ai") hitting the 4s mark
/// just before completion. Tinyllama on a phone is slower than on desktop;
/// 8s tolerance keeps the asynchronous path responsive while letting
/// borderline-fast inferences complete.
const OLLAMA_TIMEOUT_SECS: u64 = 8;
/// Blacklist threshold — after N timeouts, stop trying this model for the session
const BLACKLIST_AFTER: u32 = 5;

/// Tracks model timeouts within a session. Persistent across calls (not disk).
static MODEL_FAILURES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, u32>>>
    = std::sync::OnceLock::new();
fn model_failures() -> &'static std::sync::Mutex<std::collections::HashMap<String, u32>> {
    MODEL_FAILURES.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}
fn is_blacklisted(model: &str) -> bool {
    model_failures().lock().ok()
        .and_then(|g| g.get(model).copied())
        .map(|n| n >= BLACKLIST_AFTER).unwrap_or(false)
}
fn record_failure(model: &str) {
    if let Ok(mut g) = model_failures().lock() {
        let n = g.entry(model.to_string()).or_insert(0);
        *n += 1;
        if *n == BLACKLIST_AFTER {
            eprintln!("LLM: blacklisted {} after {} failures this session", model, n);
        }
    }
}
fn record_success(model: &str) {
    // Reset failure count on success — model might be slow only sometimes
    if let Ok(mut g) = model_failures().lock() {
        if let Some(n) = g.get_mut(model) { *n = 0; }
    }
}

/// Public: clear all model failure counts. Used by the log_summary
/// retry path so that "ollama wasn't running, started it later" works
/// even after the blacklist would otherwise lock everything out.
/// Called periodically while the summary worker is in retry mode.
pub fn clear_model_blacklist() {
    if let Ok(mut g) = model_failures().lock() {
        if !g.is_empty() {
            eprintln!("LLM: clearing model blacklist ({} entries)", g.len());
            g.clear();
        }
    }
}

/// Public: returns true if any models are currently blacklisted.
/// log_summary uses this to know whether to also call clear_model_blacklist
/// before its retry attempts.
pub fn any_blacklisted() -> bool {
    model_failures().lock().ok()
        .map(|g| g.values().any(|&n| n >= BLACKLIST_AFTER))
        .unwrap_or(false)
}

/// Query a specific model with a hard timeout. Spawns ollama as a child process
/// and kills it if it doesn't return within OLLAMA_TIMEOUT_SECS seconds.
pub fn ollama_query_model(model: &str, prompt: &str) -> Result<String, String> {
    if !has_ollama() {
        return Err("ollama not installed".into());
    }
    if is_blacklisted(model) {
        return Err(format!("{} blacklisted (too many timeouts)", model));
    }
    let full_prompt = build_intent_prompt(prompt);
    ollama_run_raw(model, &full_prompt, OLLAMA_TIMEOUT_SECS)
}

/// Same as ollama_query_model but sends the prompt VERBATIM (no
/// build_intent_prompt wrapping). Used by callers like log_summary
/// that have their own prompt template — wrapping their prompt in
/// the intent-classifier shell would produce nonsense.
///
/// Uses a longer timeout (12s vs the standard 8s) because summary
/// inference is naturally slower than intent classification. Worth
/// the wait when the answer arrives, and the caller is on a worker
/// thread anyway.
pub fn ollama_query_model_raw(model: &str, prompt: &str) -> Result<String, String> {
    if !has_ollama() {
        return Err("ollama not installed".into());
    }
    if is_blacklisted(model) {
        return Err(format!("{} blacklisted (too many timeouts)", model));
    }
    // v0.1.115: purpose-built summarizer models (any name containing
    // "summariz") get a 30s timeout instead of the standard 12s.
    // Background:
    //   - User pulled QyrouNnet/summarizer:400m-q8_0 and q4_k_m;
    //     they're 270-386MB models specialized for summarization
    //   - First-call latency on these was hitting 12s repeatedly,
    //     forcing fallback to gemma/phi3 (which weren't installed)
    //   - 30s gives the dedicated summarizer enough room while
    //     still capping total wait — 30s on a single 400M model
    //     is reasonable for one-shot summarization
    //   - Other models (gemma:2b, phi3:mini, tinyllama) keep 12s;
    //     they're general-purpose and the 12s budget is fine
    let timeout_secs = if model.to_ascii_lowercase().contains("summariz") {
        30
    } else {
        12
    };
    ollama_run_raw(model, prompt, timeout_secs)
}

/// v0.1.453: ask the ollama daemon to unload a model after we
/// time out and kill the `ollama run` CLI client. Killing the
/// client does NOT stop the daemon's inference — the model keeps
/// generating inside `ollama serve`, which is the root cause of
/// the "ollama is overloaded" cascade the user saw (every timeout
/// left the previous request still running while a new one piled
/// on). `ollama stop <model>` unloads it from the daemon.
///
/// Best-effort + fire-and-forget: spawned detached with a short
/// self-timeout so it never blocks the caller. We don't read its
/// result — if the daemon is wedged, there's nothing useful to do
/// anyway. Uses `ollama stop` (available in modern ollama; a
/// no-op error on old versions, harmless).
fn stop_ollama_model(model: &str) {
    let model = model.to_string();
    std::thread::spawn(move || {
        if let Ok(mut child) = std::process::Command::new("ollama")
            .args(&["stop", &model])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            // Don't wait forever for stop itself — bound it.
            let start = std::time::Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {
                        if start.elapsed().as_secs() >= 3 {
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(_) => break,
                }
            }
        }
    });
}

/// Inner ollama-spawn-and-poll. Common code factored out of
/// ollama_query_model and ollama_query_model_raw. Caller passes
/// the timeout in seconds.
fn ollama_run_raw(model: &str, full_prompt: &str, timeout_secs: u64) -> Result<String, String> {
    let mut child = std::process::Command::new("ollama")
        .args(&["run", model, full_prompt])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("ollama spawn: {}", e))?;

    let pid = child.id();
    let start = std::time::Instant::now();

    // Poll for completion with timeout
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    record_failure(model);
                    return Err("ollama exited non-zero".into());
                }
                break;
            }
            Ok(None) => {
                if start.elapsed().as_secs() >= timeout_secs {
                    eprintln!("LLM: {} timed out after {}s, killing pid {}",
                        model, timeout_secs, pid);
                    let _ = child.kill();
                    let _ = child.wait();
                    // v0.1.453: killing the `ollama run` CLI client
                    // does NOT stop the daemon's inference — the
                    // model keeps generating inside `ollama serve`,
                    // which is why repeated timeouts cascade into
                    // "ollama is overloaded". Ask the daemon to
                    // unload the model so it actually frees up.
                    stop_ollama_model(model);
                    record_failure(model);
                    return Err(format!("{} timeout", model));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                record_failure(model);
                return Err(format!("ollama wait: {}", e));
            }
        }
    }

    // Read stdout
    let mut output = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        use std::io::Read;
        let _ = stdout.read_to_end(&mut output);
    }
    let text = String::from_utf8_lossy(&output).trim().to_string();
    let elapsed = start.elapsed();
    eprintln!("LLM: {} returned in {:.2}s", model, elapsed.as_secs_f32());
    record_success(model);
    Ok(text)
}

/// Cancellable variant of ollama_query that accepts a JobHandle from
/// async_jobs. Differences from ollama_query_model:
///   - prompt is sent VERBATIM (no build_intent_prompt wrapping) so
///     callers can use this for non-intent prompts like fragment checks
///   - registers child PID with the handle so async_jobs::cancel_job
///     can SIGKILL it externally
///   - polls handle.is_cancelled() between waits and exits early if set
///   - shorter default timeout (3s) — fragment checks are binary so
///     anything taking longer than that should give up and let local
///     heuristic decide
///
/// Picks the first model in the standard escalation list that's neither
/// blacklisted nor missing. We don't fan out to multiple models here
/// because the binary classification is too simple to need multiple
/// opinions, and parallel inference would burn battery.
pub fn ollama_query_with_handle(
    prompt: &str,
    handle: &crate::async_jobs::JobHandle,
) -> Result<String, String> {
    // Cache check FIRST — same logic as ollama_query_multi. The
    // cancellable path is used for fragment-classification and
    // similar voice-driven prompts that hit the same shapes
    // repeatedly. A cache hit returns instantly, no subprocess spawn.
    if let Some(cached) = crate::llm_cache::lookup(prompt) {
        eprintln!("LLM (handle): cache hit ({}b)", cached.len());
        return Ok(cached);
    }

    if !has_ollama() {
        return Err("ollama not installed".into());
    }
    // Pick the smallest available non-blacklisted model.
    let model = ["tinyllama", "gemma:2b", "phi3:mini", "llama3.2:1b", "llama3.1:8b"]
        .iter()
        .find(|m| !is_blacklisted(m))
        .copied()
        .unwrap_or("tinyllama");

    // Doubled in v0.1.66 from 3s to 6s. With the single-tier default,
    // CPU has more headroom so the LLM can take longer without the user
    // experience suffering. Also the heuristic is conservative: the
    // ambiguous branch already arrives slowly (multi-word, multi-clause
    // utterance), so an extra 3s of think time isn't perceived as lag.
    const TIMEOUT_SECS: u64 = 6;

    let mut child = std::process::Command::new("ollama")
        .args(&["run", model, prompt])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("ollama spawn: {}", e))?;

    let pid = child.id();
    handle.set_child_pid(Some(pid));
    let start = std::time::Instant::now();

    loop {
        // Check cancellation FIRST. If async_job_cancel was called,
        // it has already SIGKILLed the child; we just need to reap and
        // exit cleanly.
        if handle.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            handle.set_child_pid(None);
            return Err("cancelled by user".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                handle.set_child_pid(None);
                if !status.success() {
                    record_failure(model);
                    return Err(format!("{} exited non-zero", model));
                }
                break;
            }
            Ok(None) => {
                if start.elapsed().as_secs() >= TIMEOUT_SECS {
                    eprintln!("LLM async: {} timed out after {}s, killing pid {}",
                        model, TIMEOUT_SECS, pid);
                    let _ = child.kill();
                    let _ = child.wait();
                    handle.set_child_pid(None);
                    // v0.1.453: also unload the model from the daemon
                    // (killing the CLI client alone leaves inference
                    // running in `ollama serve`).
                    stop_ollama_model(model);
                    record_failure(model);
                    return Err(format!("{} timeout", model));
                }
                // Poll every 50ms — same as the synchronous variant.
                // Cancellation latency therefore averages 25ms, max 50ms.
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                handle.set_child_pid(None);
                record_failure(model);
                return Err(format!("ollama wait: {}", e));
            }
        }
    }

    let mut output = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        use std::io::Read;
        let _ = stdout.read_to_end(&mut output);
    }
    let text = String::from_utf8_lossy(&output).trim().to_string();
    let elapsed = start.elapsed();
    eprintln!("LLM async: {} returned in {:.2}s: {:?}", model, elapsed.as_secs_f32(), text);
    record_success(model);
    // Cache the response so identical prompts in this session (and
    // future sessions) skip the subprocess spawn. Empty / error
    // responses are not cached (llm_cache::store filters them).
    crate::llm_cache::store(prompt, &text);
    Ok(text)
}

/// Query LLM serially, smallest model first. If the result has a confident
/// `action` field with a known value, return immediately. Only escalate to
/// the next model if response was malformed or "none".
///
/// Rationale: parallel queries make ollama swap contexts on RAM-constrained
/// systems, tanking latency. Serial-smallest-first uses minimum memory and
/// usually returns in 0.5-2s for tinyllama. We only wake bigger models when
/// the small one truly fails.
pub fn ollama_query_multi(prompt: &str) -> Result<String, String> {
    // Cache check FIRST: many voice-related prompts are highly repetitive
    // (fragment-classification of common phrases, intent-disambiguation
    // for known utterances). A cache hit returns in microseconds vs the
    // ~1-4s an Ollama call takes. Cache hit rate is the main win here;
    // miss path is unchanged so we don't degrade novel-prompt latency.
    if let Some(cached) = crate::llm_cache::lookup(prompt) {
        eprintln!("LLM: cache hit ({}b)", cached.len());
        return Ok(cached);
    }

    if !has_ollama() {
        return Err("ollama not installed (apt install ollama or curl https://ollama.com/install.sh | sh)".into());
    }
    let models = preferred_models();
    if models.is_empty() { return Err("No models available".into()); }

    // Try each model in order until one gives a confident response
    let known_actions = ["navigate", "search", "terminal", "email", "file_append"];
    let mut last_err = String::from("no models tried");

    for model in &models {
        if is_blacklisted(model) { continue; }
        match ollama_query_model(model, prompt) {
            Ok(resp) => {
                // Check if the response is confident: extract action and verify it's a known one
                if let Some(action) = extract_action(&resp) {
                    if known_actions.contains(&action.as_str()) {
                        eprintln!("LLM: {} succeeded with action='{}', stopping early", model, action);
                        // Cache the confident response for future identical prompts.
                        crate::llm_cache::store(prompt, &resp);
                        return Ok(resp);
                    }
                    // "none" or unknown action — try next model
                    eprintln!("LLM: {} returned action='{}' (not confident), escalating", model, action);
                    last_err = format!("{} gave low-confidence result", model);
                } else {
                    eprintln!("LLM: {} returned no parseable action, escalating", model);
                    last_err = format!("{} response unparseable", model);
                }
            }
            Err(e) => {
                last_err = e.clone();
                // Continue to next model unless ollama itself is missing
                if e.contains("not installed") { return Err(e); }
            }
        }
    }
    Err(last_err)
}

/// Build a prompt that elicits structured JSON from the LLM
fn build_intent_prompt(user_text: &str) -> String {
    format!(
        "BE BRIEF. Reply with ONE LINE of JSON only. No markdown. No prose.\n\
         Schema: {{\"action\":<a>,\"arg\":<a>}} where action is one of: navigate, search, terminal, none.\n\
         User: open hacker news\n\
         Reply: {{\"action\":\"navigate\",\"arg\":\"https://news.ycombinator.com\"}}\n\
         User: find rust tutorials\n\
         Reply: {{\"action\":\"search\",\"arg\":\"rust tutorials\"}}\n\
         User: git status\n\
         Reply: {{\"action\":\"terminal\",\"arg\":\"git status\"}}\n\
         User: hello\n\
         Reply: {{\"action\":\"none\",\"arg\":\"\"}}\n\
         User: {}\n\
         Reply:",
        user_text.replace('"', "'")
    )
}

/// Extract the "action" field from a JSON response (best-effort)
fn extract_action(text: &str) -> Option<String> {
    let t = text.trim();
    let start = t.find('{')?;
    let end = t.rfind('}')?;
    if end <= start { return None; }
    let json = &t[start..=end];
    // Tiny JSON parse: find "action" key
    let key = "\"action\"";
    let pos = json.find(key)?;
    let rest = &json[pos + key.len()..];
    let colon = rest.find(':')?;
    let after = &rest[colon + 1..];
    // Find next quoted string
    let q1 = after.find('"')?;
    let after2 = &after[q1 + 1..];
    let q2 = after2.find('"')?;
    Some(after2[..q2].to_lowercase())
}

// ─── Date parsing ──────────────────────────────────────────────────

fn epoch_secs() -> i64 {
    crate::util::epoch_secs_i64()
}

fn days_since_epoch() -> i64 {
    epoch_secs() / 86400
}

/// Resolve a natural-language date to days-since-epoch
fn resolve_date(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches('?').trim_end_matches('.').to_lowercase();

    // Holidays (this year)
    let (y, _, _) = current_ymd();
    let mut try_year = y;
    let now_days = days_since_epoch();

    let holiday_md: Option<(u32, u32)> = match s.as_str() {
        "christmas" | "xmas" | "christmas day" => Some((12, 25)),
        "new year" | "new year's" | "new years" | "new year's day" => Some((1, 1)),
        "halloween" => Some((10, 31)),
        "valentine's day" | "valentines day" | "valentine" => Some((2, 14)),
        "thanksgiving" => Some((11, 25)),  // approximate
        "independence day" | "fourth of july" | "4th of july" => Some((7, 4)),
        "earth day" => Some((4, 22)),
        _ => None,
    };
    if let Some((m, d)) = holiday_md {
        let candidate = ymd_to_days(try_year, m, d);
        // If passed already this year, use next year
        if candidate < now_days { try_year += 1; }
        return Some(ymd_to_days(try_year, m, d));
    }

    // "tomorrow" / "today" / "yesterday"
    match s.as_str() {
        "today" => return Some(now_days),
        "tomorrow" => return Some(now_days + 1),
        "yesterday" => return Some(now_days - 1),
        _ => {}
    }

    // "next monday" / "monday"
    let weekdays = ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"];
    let s_clean = s.strip_prefix("next ").unwrap_or(&s);
    if let Some(idx) = weekdays.iter().position(|&w| w == s_clean) {
        // current weekday: jan 1 1970 was thursday (idx 4)
        let now_dow = ((now_days + 4) % 7) as usize;
        let mut delta = (idx as i64 - now_dow as i64).rem_euclid(7);
        if delta == 0 { delta = 7; }
        return Some(now_days + delta);
    }

    // "may 15" / "december 25"
    let months = [
        ("january", 1), ("february", 2), ("march", 3), ("april", 4),
        ("may", 5), ("june", 6), ("july", 7), ("august", 8),
        ("september", 9), ("october", 10), ("november", 11), ("december", 12),
    ];
    for (name, m) in &months {
        if let Some(rest) = s.strip_prefix(name) {
            let day_str = rest.trim().trim_end_matches("st").trim_end_matches("nd")
                .trim_end_matches("rd").trim_end_matches("th").trim();
            if let Ok(d) = day_str.parse::<u32>() {
                let candidate = ymd_to_days(try_year, *m, d);
                if candidate < now_days { try_year += 1; }
                return Some(ymd_to_days(try_year, *m, d));
            }
        }
    }

    None
}

/// Get current (year, month, day) using `date` command
fn current_ymd() -> (i32, u32, u32) {
    if let Ok(out) = Command::new("date").arg("+%Y-%m-%d").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            let parts: Vec<&str> = s.trim().split('-').collect();
            if parts.len() == 3 {
                return (
                    parts[0].parse().unwrap_or(2025),
                    parts[1].parse().unwrap_or(1),
                    parts[2].parse().unwrap_or(1),
                );
            }
        }
    }
    (2025, 1, 1)
}

/// Convert (year, month, day) to days since epoch (1970-01-01)
fn ymd_to_days(year: i32, month: u32, day: u32) -> i64 {
    // Gregorian date to Julian Day Number, then subtract JDN of 1970-01-01 (= 2440588)
    let a = ((14 - month as i32) / 12) as i32;
    let y = year + 4800 - a;
    let m = month as i32 + 12 * a - 3;
    let jdn = day as i32 + (153 * m + 2) / 5 + 365 * y + y / 4 - y / 100 + y / 400 - 32045;
    (jdn - 2440588) as i64
}
