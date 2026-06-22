// src/log_summary.rs
//
// AI-generated summaries for command output blocks. The goal: make a
// terminal that's friendly to non-power users by injecting plain-
// language explanations above the raw output of commands like
// `ls -la`, `dust`, `ps`, etc.
//
// Architecture:
//
//   1. Each completed command block has its (command, output) handed to
//      request_summary(). The request is debounced (500ms) so rapid-fire
//      commands don't spam the LLM, and queued at the back of
//      SUMMARY_QUEUE.
//
//   2. A single background worker thread (spawned at startup) drains
//      the queue. Before invoking ollama, it cooperates with voice via
//      heavy_inference::try_acquire — if voice is recording or
//      transcribing, the worker defers and re-checks every 200ms.
//      Voice always wins ties.
//
//   3. The summary prompt is small but deliberate: ask for ONE plain
//      sentence (or two short ones), no markdown, no jargon, biased
//      toward "what would help a non-technical user understand this
//      output." The output is then trimmed and stored in
//      SUMMARIES, keyed by block id.
//
//   4. Future: streaming/partial updates for live commands (top, htop)
//      with the 750-1500ms cadence the user requested. v1 only handles
//      one-shot blocks. The infrastructure is here; the live-update
//      polling loop will be a separate worker.
//
//   5. QML reads results via main.rs qt_methods. The completion of a
//      summary increments SUMMARY_REVISION so QML can refresh.
//
// Coordination with voice:
//
//   The "heavy" lock isn't a strict mutex — voice uses whisper-rs (own
//   thread, own process address space), summary uses ollama
//   (subprocess). They CAN technically run concurrently. The lock is a
//   COURTESY mechanism: on phone-class hardware, both running at once
//   results in slow whisper + slow ollama + bad UX. So we serialize.
//
//   Implementation: a single Mutex<InferenceOwner> that summary worker
//   tries to acquire before each ollama invocation. Voice does not
//   need to actively grab the lock — it just sets VOICE_ACTIVE which
//   summary checks. This keeps voice's hot path lock-free.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, Duration};

/// Whether log summarization is enabled. Off by default — power users
/// hate inline AI in their terminal. Setting flipped on/off from QML
/// via main.rs::log_summary_set_enabled.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Set true by voice subsystem when whisper is running (capture or
/// transcribe). Summary worker checks this and defers if true.
/// Cleared when voice goes idle.
pub static VOICE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Pending summary requests. Worker drains this; QML doesn't see it.
static SUMMARY_QUEUE: OnceLock<Mutex<Vec<SummaryRequest>>> = OnceLock::new();

/// Completed summaries. Keyed by block id. Worker writes; QML reads
/// via get_summary().
static SUMMARIES: OnceLock<Mutex<HashMap<u32, SummaryEntry>>> = OnceLock::new();

/// Per-command output history for diff summarization (v0.1.106).
///
/// Keyed by NORMALIZED command (whitespace squashed, leading sudo
/// stripped — see normalize_command). Value is the most recent output
/// we summarized for that command. When the same command runs again,
/// we diff the new output against this stored one. If the diff is
/// small, we summarize ONLY the diff: faster (smaller prompt → smaller
/// inference time) AND more relevant (the user already knows what the
/// command "usually" does; they want to know what changed).
///
/// Capped at 32 entries (LRU-ish — we evict oldest when full).
/// Per-entry size is capped at 4KB to bound memory.
static OUTPUT_HISTORY: OnceLock<Mutex<HashMap<String, OutputHistoryEntry>>> = OnceLock::new();

#[derive(Clone, Debug)]
struct OutputHistoryEntry {
    /// Truncated output (≤4KB) for diffing
    output: String,
    /// When stored — used for LRU eviction.
    stored_at: Instant,
}

/// Monotonic counter incremented each time a new summary lands.
/// QML polls this to detect "anything new" without iterating the map.
static SUMMARY_REVISION: AtomicU64 = AtomicU64::new(0);

/// One pending summary request. The output is captured at queue time
/// — the block's actual output may grow after submission, but we only
/// summarize what we saw.
#[derive(Clone, Debug)]
struct SummaryRequest {
    block_id: u32,
    command: String,
    output: String,
    queued_at: Instant,
    /// How many times we've tried and failed. Capped — see MAX_ATTEMPTS.
    /// On each failure (ollama not running, all models timed out, etc.)
    /// we increment and re-queue with a backoff; once we hit the cap,
    /// the request is dropped permanently.
    attempts: u32,
    /// Earliest moment we should retry. On first queue this is `queued_at`;
    /// after each failure we set it to `Instant::now() + backoff`.
    not_before: Instant,
    /// v0.1.106: when true, `output` field contains a unified-diff
    /// snippet instead of the raw command output. The prompt builder
    /// uses a different template that frames it as "what changed
    /// since last time you ran this." Result: shorter, more relevant
    /// summaries for repeat commands.
    is_diff: bool,
    /// v0.1.378 (email arc): when true, the block looks like an
    /// email (detected via `term_blocks::CommandBlock::detected_kind`
    /// at submit time). Prompt is reframed as "what's this email
    /// about and what does it want from me" rather than the generic
    /// "what's this output telling me" template. Adds ~50 tokens
    /// per call but vastly improves output quality for the email
    /// case — the generic prompt produces "lists From, To, Subject
    /// headers and the message body" type tautologies that aren't
    /// useful.
    is_email: bool,
}

/// Maximum retry attempts per request. Set deliberately high (12) so
/// that "ollama wasn't running when I started the app, started it
/// later, where are my summaries?" works. With exponential backoff
/// capped at 60s, 12 attempts spans ~5-6 minutes of retries —
/// long enough to bridge most setup gaps, short enough that the
/// queue doesn't grow unbounded if ollama is permanently broken.
const MAX_ATTEMPTS: u32 = 12;

/// Backoff schedule (seconds): 2, 4, 8, 16, 32, 60, 60, 60, 60, 60, 60, 60.
/// Capped at 60 so the user only waits up to a minute after starting
/// ollama before seeing summaries land.
fn retry_backoff(attempts: u32) -> Duration {
    let secs = match attempts {
        0 => 0,
        1 => 2,
        2 => 4,
        3 => 8,
        4 => 16,
        5 => 32,
        _ => 60,
    };
    Duration::from_secs(secs)
}

/// Global counter of consecutive summary failures across ALL blocks.
/// Reset on any successful summary. Used by the lockup watchdog —
/// if ollama is overloaded (every model timeout), we back off the
/// entire queue rather than thrashing each block through 4 models.
///
/// Failure mode this prevents (observed in v0.1.91 logs):
///   block 3 fails on gemma:2b (12s timeout)
///   block 3 fails on phi3:mini (12s timeout)
///   block 3 fails on llama3.2:1b (12s timeout)
///   block 3 fails on tinyllama (12s timeout)  -> 48s gone
///   [log-summary] block 3 retry 1 in 2s
///   block 4 starts, same path, 48s thrash
///   block 5 starts, same path, 48s thrash
///   ...
///
/// With watchdog: after ~3 consecutive global failures, pause the
/// queue for 90s. New blocks queue up but don't run. After the pause,
/// try once. If that fails, pause 180s. Doubling cap at 600s (10 min).
/// First success resets to normal.
static CONSECUTIVE_FAILURES: AtomicU64 = AtomicU64::new(0);
static QUEUE_PAUSED_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

/// Reset failure tracking on success.
fn note_summary_success() {
    CONSECUTIVE_FAILURES.store(0, Ordering::SeqCst);
    if let Ok(mut g) = QUEUE_PAUSED_UNTIL.lock() {
        *g = None;
    }
}

/// Tally a failure. Returns the duration to wait before the next
/// queue scan (i.e. how long the queue is paused for).
fn note_summary_failure() -> Duration {
    let count = CONSECUTIVE_FAILURES.fetch_add(1, Ordering::SeqCst) + 1;
    // First 2 failures: no global pause, individual block backoff
    // handles it. Beyond 2: pause the whole queue.
    if count <= 2 {
        return Duration::from_secs(0);
    }
    // 3rd+ failure: pause grows. count=3 -> 90s, 4 -> 180s, 5 -> 360s, cap 600s.
    let pause_secs = match count {
        3 => 90,
        4 => 180,
        5 => 360,
        _ => 600,
    };
    let until = Instant::now() + Duration::from_secs(pause_secs);
    if let Ok(mut g) = QUEUE_PAUSED_UNTIL.lock() {
        *g = Some(until);
    }
    crate::clog!("log-summary", "watchdog: {} consecutive failures, pausing queue for {}s",
              count, pause_secs);
    Duration::from_secs(pause_secs)
}

/// True when the queue is in a watchdog-paused state.
fn queue_paused() -> bool {
    if let Ok(g) = QUEUE_PAUSED_UNTIL.lock() {
        if let Some(until) = *g {
            return Instant::now() < until;
        }
    }
    false
}

/// One completed summary record.
#[derive(Clone, Debug)]
pub struct SummaryEntry {
    pub block_id: u32,
    pub text: String,
    /// Model name (so QML can show "via gemma:2b" etc.). For dumb
    /// summaries this is "dumb".
    pub model: String,
    /// Wall-clock ms the inference took (0 for dumb summaries — they
    /// run synchronously and are too fast to bother measuring).
    pub duration_ms: u32,
    /// epoch seconds when summary was produced
    pub completed_at: u64,
    /// True when this is a dumb (rule-based) fallback. AI summaries
    /// when they land OVERWRITE dumb entries; QML uses this flag to
    /// decide whether to render the model attribution byline.
    pub is_dumb: bool,
    /// Extracted keywords from the output. Used by score_relevance
    /// when comparing voice utterances against past blocks. Filled
    /// for both dumb and AI summaries (the dumb extractor runs
    /// regardless and its keywords are stored on the entry).
    pub keywords: Vec<String>,
}

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    crate::clog!("log-summary", "enabled={}", enabled);
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Get the current summary revision — increments on each new summary
/// that lands. QML calls this on a poll timer to detect updates.
pub fn revision() -> u64 {
    SUMMARY_REVISION.load(Ordering::Relaxed)
}

/// Look up the summary text for a block. Returns None if no summary
/// has been produced (or feature disabled, or worker hasn't gotten to it).
pub fn get_summary(block_id: u32) -> Option<SummaryEntry> {
    let map = SUMMARIES.get()?.lock().ok()?;
    map.get(&block_id).cloned()
}

/// Drain all current summary entries — useful for QML to rebuild a
/// "summaries by block" model in one call. Cheap (just clones the
/// HashMap).
pub fn list_all() -> Vec<SummaryEntry> {
    let q = match SUMMARIES.get() { Some(q) => q, None => return Vec::new() };
    let g = match q.lock() { Ok(g) => g, Err(_) => return Vec::new() };
    g.values().cloned().collect()
}

/// Queue a block for summarization. Caller passes (block_id, command,
/// output). Returns immediately. Worker picks up async.
///
/// Skipped silently when disabled, when output is too short to be
/// worth summarizing (< 3 lines or < 80 chars), or when this block
/// already has a summary recorded (so re-queueing is a no-op).
///
/// Side effect: ALSO produces an immediate dumb-mode summary (rule-
/// based, no LLM) and stores it so users see something instantly. The
/// AI summary, when it lands, OVERWRITES the dumb one. If AI is
/// disabled or never lands (ollama not running, retries exhausted),
/// the dumb summary persists.
pub fn request_summary(block_id: u32, command: &str, output: &str) {
    // Skip while a password prompt is active. Even though terminal
    // echo-off prevents the password from appearing in `output`, the
    // command itself ("sudo something") may include sensitive context,
    // and the AI summary path would ship that to Ollama. Defense in
    // depth: while in password mode, drop the entire request.
    if crate::security::should_suppress_capture() {
        return;
    }
    // Generate the dumb summary first — it's cheap and runs even when
    // AI summarization is disabled, so users always have SOMETHING.
    // We store it under a separate "is_dumb" flag so the AI worker
    // can distinguish "no summary yet" from "we have a dumb fallback,
    // overwrite when better arrives."
    let dumb = crate::dumb_summary::summarize(command, output);
    if !dumb.text.is_empty() {
        SUMMARIES.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(map) = SUMMARIES.get() {
            if let Ok(mut g) = map.lock() {
                // Only insert dumb if NO entry exists. If an AI entry
                // already lives here (from a previous run / cache),
                // don't downgrade.
                let should_insert = match g.get(&block_id) {
                    None => true,
                    Some(e) => e.is_dumb,  // overwrite stale dumb with fresh dumb
                };
                if should_insert {
                    g.insert(block_id, SummaryEntry {
                        block_id,
                        text: dumb.text.clone(),
                        model: "dumb".to_string(),
                        duration_ms: 0,
                        completed_at: epoch_secs(),
                        is_dumb: true,
                        keywords: dumb.keywords.clone(),
                    });
                }
            }
        }
        SUMMARY_REVISION.fetch_add(1, Ordering::Relaxed);
    }
    // If AI summarization is disabled, stop here — dumb is the final
    // answer for this block.
    if !is_enabled() { return; }
    // Only summarize substantial output. Single-line responses (e.g.
    // `pwd` printing one path) don't need explanation; the user can
    // read them.
    let line_count = output.lines().count();
    if line_count < 3 && output.len() < 80 {
        return;
    }
    // Don't re-summarize a block that already has a NON-DUMB summary.
    if let Some(map) = SUMMARIES.get() {
        if let Ok(g) = map.lock() {
            if let Some(e) = g.get(&block_id) {
                if !e.is_dumb { return; }
            }
        }
    }
    // Cap output size sent to LLM. Summarizing a 50KB log line by
    // line is expensive AND most models have 4-8k context. Take
    // first 100 lines + last 30 lines if it's longer — typical
    // command output's interesting parts cluster at the ends.
    let truncated = truncate_for_prompt(output, 100, 30);

    // v0.1.378 (email arc, ship 2 of 3): detect email shape
    // from the ORIGINAL output (before truncation, so the
    // From:/Subject:/etc. anchor headers at the top survive
    // even when output is large). When detected, the worker
    // uses a reframed prompt that asks "what's this email
    // about and what does it want" rather than the generic
    // template that produces "lists From, To, Subject headers
    // and the message body" tautologies for email-shaped
    // input.
    //
    // Pure boolean — we don't change WHICH text gets sent (the
    // truncate / diff path still applies), only the PROMPT
    // template selected for it.
    let is_email = crate::term_blocks::CommandBlock::looks_like_email(output);

    // v0.1.106: diff-summary path. If we've seen this command before,
    // diff the new output against stored. Three outcomes:
    //   1. "(output unchanged)" — store immediate "Same as last run"
    //      summary, skip AI entirely. Saves the LLM round-trip for
    //      idempotent commands.
    //   2. Small diff — queue with is_diff=true, prompt builder uses
    //      a "what changed" template. Faster + more relevant.
    //   3. Big diff (>200 lines changed) — fall through to full
    //      summary (more useful than wading through diff).
    let prev_output = lookup_previous_output(command);
    let mut prompt_input = truncated.clone();
    let mut is_diff = false;
    if let Some(prev) = prev_output.as_ref() {
        match compute_diff_summary_input(prev, &truncated) {
            Some(diff_text) if diff_text == "(output unchanged)" => {
                // Short-circuit. Store a synthetic summary that says
                // "same output as before". User sees this immediately
                // without an LLM call. No queue insert.
                let map = SUMMARIES.get_or_init(|| Mutex::new(HashMap::new()));
                if let Ok(mut g) = map.lock() {
                    let same_text = "Same output as the last run of this command.".to_string();
                    g.insert(block_id, SummaryEntry {
                        block_id,
                        text: same_text,
                        model: "diff-noop".to_string(),
                        duration_ms: 0,
                        completed_at: epoch_secs(),
                        is_dumb: false,
                        keywords: dumb.keywords.clone(),
                    });
                }
                SUMMARY_REVISION.fetch_add(1, Ordering::Relaxed);
                // Update history with current (same as prev, so a no-op
                // but cheap and keeps stored_at fresh for LRU).
                store_output_history(command, &truncated);
                crate::clog!("log-summary", "block {}: output unchanged from previous run, skipping AI",
                    block_id);
                return;
            }
            Some(diff_text) => {
                // Useful diff — queue this instead of the full output.
                prompt_input = diff_text;
                is_diff = true;
                crate::clog!("log-summary", "block {}: using diff against previous run ({} chars)",
                    block_id, prompt_input.len());
            }
            None => {
                // Diff was too big — fall through to full summary.
                crate::clog!("log-summary", "block {}: diff too large, doing full summary",
                    block_id);
            }
        }
    }
    // Regardless of diff path, store this output for next-time diffing.
    store_output_history(command, &truncated);

    let req = SummaryRequest {
        block_id,
        command: command.to_string(),
        output: prompt_input,
        queued_at: Instant::now(),
        attempts: 0,
        not_before: Instant::now(),
        is_diff,
        is_email,
    };
    let q = SUMMARY_QUEUE.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut g) = q.lock() {
        // De-dupe: if this block_id is already in the queue, replace
        // the existing entry rather than adding a second copy.
        g.retain(|r| r.block_id != block_id);
        g.push(req);
        // Cap queue at 8 — older entries get dropped if user is
        // running commands faster than the LLM can keep up. Newest
        // commands are usually most relevant to summarize.
        while g.len() > 8 {
            g.remove(0);
        }
    }
    crate::clog!("log-summary", "queued block {} ({}b) for AI summarization",
        block_id, output.len());
}

/// Normalize a command for the diff-summary key.
///
/// Strips leading `sudo`/`doas`, squashes whitespace, removes trailing
/// shell separators. Result: `sudo apt-get update` and `apt-get  update`
/// hash to the same key, so we can diff their outputs even though the
/// raw command lines differed.
///
/// Conservative — if the command has redirects or pipes, we still
/// normalize it but a true semantic match would need shell parsing.
/// In practice that's fine: same pipeline run again yields same key.
pub fn normalize_command(cmd: &str) -> String {
    let mut s = cmd.trim().to_string();
    for pre in &["sudo ", "doas ", "time ", "command "] {
        if let Some(rest) = s.strip_prefix(pre) {
            s = rest.trim_start().to_string();
        }
    }
    // Squash internal whitespace runs into single space.
    let parts: Vec<&str> = s.split_whitespace().collect();
    parts.join(" ")
}

/// Compute a unified-diff-style snippet between previous and current
/// outputs. Returns Some(diff_text) if the diff is small (<200 lines
/// changed — worth summarizing as diff instead of full output) or
/// None if the diff is too big (full summarization is more useful).
///
/// We use line-level naive diff: lines unique to `prev` get `-`,
/// lines unique to `curr` get `+`, lines in both get omitted (with a
/// header `@@ ... unchanged @@`). Not a Myers diff — for command
/// output, the simple set-difference is usually informative enough,
/// and we save a dependency.
fn compute_diff_summary_input(prev: &str, curr: &str) -> Option<String> {
    use std::collections::HashSet;
    let prev_lines: Vec<&str> = prev.lines().collect();
    let curr_lines: Vec<&str> = curr.lines().collect();
    let prev_set: HashSet<&str> = prev_lines.iter().copied().collect();
    let curr_set: HashSet<&str> = curr_lines.iter().copied().collect();

    let added: Vec<&str> = curr_lines.iter()
        .filter(|l| !prev_set.contains(*l))
        .copied().collect();
    let removed: Vec<&str> = prev_lines.iter()
        .filter(|l| !curr_set.contains(*l))
        .copied().collect();

    let total_change = added.len() + removed.len();
    if total_change == 0 {
        // Output unchanged — special signal back to caller. We return
        // a one-line marker; caller can use this to short-circuit
        // entirely without calling LLM.
        return Some(String::from("(output unchanged)"));
    }
    // Cap at 200 changed lines — beyond this, full summary is likely
    // more valuable than wading through the diff.
    if total_change > 200 {
        return None;
    }
    let mut out = String::with_capacity(prev.len().min(2048) + curr.len().min(2048));
    let unchanged = curr_lines.len() - added.len();
    out.push_str(&format!("@@ {} unchanged lines, {} added, {} removed @@\n",
        unchanged, added.len(), removed.len()));
    for l in &removed {
        out.push('-');
        // Cap each line length to keep prompt tight.
        let trimmed: String = l.chars().take(200).collect();
        out.push_str(&trimmed);
        out.push('\n');
    }
    for l in &added {
        out.push('+');
        let trimmed: String = l.chars().take(200).collect();
        out.push_str(&trimmed);
        out.push('\n');
    }
    Some(out)
}

/// Store an entry in the output history for future diff-summarization.
fn store_output_history(command: &str, output: &str) {
    let key = normalize_command(command);
    if key.is_empty() { return; }
    let map = OUTPUT_HISTORY.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut g) = map.lock() {
        // Cap entry to 4KB. For very long output, we keep the head+tail
        // (same shape as truncate_for_prompt) so the diff is still
        // meaningful but memory bounded.
        let stored = if output.len() <= 4096 {
            output.to_string()
        } else {
            let lines: Vec<&str> = output.lines().collect();
            let head_lines = 60.min(lines.len());
            let tail_lines = 20.min(lines.len().saturating_sub(head_lines));
            let mut s = String::with_capacity(4096);
            for l in lines.iter().take(head_lines) {
                s.push_str(l); s.push('\n');
            }
            if lines.len() > head_lines + tail_lines {
                s.push_str("... (truncated for diff history) ...\n");
            }
            for l in lines.iter().skip(lines.len() - tail_lines) {
                s.push_str(l); s.push('\n');
            }
            s
        };
        g.insert(key, OutputHistoryEntry {
            output: stored,
            stored_at: Instant::now(),
        });
        // LRU eviction at cap 32.
        while g.len() > 32 {
            if let Some(oldest_key) = g.iter()
                .min_by_key(|(_, e)| e.stored_at)
                .map(|(k, _)| k.clone())
            {
                g.remove(&oldest_key);
            } else { break; }
        }
    }
}

/// Look up a previous output for diff summarization. Returns the
/// stored output if this command was seen before.
fn lookup_previous_output(command: &str) -> Option<String> {
    let key = normalize_command(command);
    if key.is_empty() { return None; }
    let map = OUTPUT_HISTORY.get()?;
    let g = map.lock().ok()?;
    g.get(&key).map(|e| e.output.clone())
}

/// Truncate output to (head_lines, tail_lines) joined with a marker
/// when content is dropped. Preserves the head and tail so the LLM
/// sees both the start of the output (often the heading) and the end
/// (often the result/exit status).
fn truncate_for_prompt(output: &str, head_lines: usize, tail_lines: usize) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= head_lines + tail_lines {
        return output.to_string();
    }
    let mut out = String::with_capacity(output.len().min(8192));
    for line in lines.iter().take(head_lines) {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&format!("\n... [{} more lines elided] ...\n\n",
        lines.len() - head_lines - tail_lines));
    for line in lines.iter().skip(lines.len() - tail_lines) {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Build the summarization prompt. Goal: ONE short sentence (or two
/// short ones) that explains the output to a non-technical user.
/// Avoid jargon, avoid restating the command, avoid markdown.
///
/// The prompt is structured to encourage helpful pattern recognition:
///   - file listings → file types and counts
///   - dust output → top space consumers in plain words
///   - ps/top → what programs are using resources
///   - errors → what went wrong in friendly terms
///   - configs/JSON → what the data is roughly about
///
/// Build the diff-summarization prompt. Frames the output as a diff
/// against a previous run of the same command. Tells the model to
/// describe WHAT CHANGED, not what the command does — the user already
/// knows that, they want the delta.
///
/// Smaller prompt than the full version because (a) the diff is much
/// shorter than full output and (b) we don't need few-shot examples
/// for "describe a diff" — small models grok that pattern with just
/// instruction.
fn build_diff_prompt(command: &str, diff_text: &str) -> String {
    let cmd_short: String = command.chars().take(120).collect();
    let diff_short: String = diff_text.chars().take(3000).collect();
    format!(
        "You are a friendly assistant. The user just ran a command \
         they've run before. Below is a unified diff showing how the \
         output CHANGED since last time. Describe ONLY what changed in \
         ONE OR TWO short sentences. No markdown, no code blocks, no quotes.\n\
         \n\
         CRITICAL: Output ONLY the summary itself. Do NOT preface with \
         phrases like 'Sure,', 'Here is', 'Here's a brief explanation', \
         'Based on the diff', 'The diff shows', 'This shows', 'I see', \
         'Let me explain', or any similar preamble. Start IMMEDIATELY \
         with the substance — what changed.\n\
         \n\
         Lines starting with `+` are NEW (in current run, not previous).\n\
         Lines starting with `-` are GONE (were in previous, not current).\n\
         The `@@ ... unchanged @@` marker shows context.\n\
         \n\
         Examples:\n\
         \n\
         Command: ls\n\
         Diff:\n\
         @@ 4 unchanged lines, 1 added, 0 removed @@\n\
         +new_report.pdf\n\
         What changed: A new file 'new_report.pdf' appeared.\n\
         \n\
         Command: docker ps\n\
         Diff:\n\
         @@ 1 unchanged line, 1 added, 1 removed @@\n\
         -nginx-old   Up 2 hours\n\
         +nginx       Up 2 minutes\n\
         What changed: The nginx container was just restarted (was 'nginx-old', running 2 hours; now 'nginx', running 2 minutes).\n\
         \n\
         Command: {}\n\
         Diff:\n\
         {}\n\
         What changed:",
        cmd_short.replace('\\', " ").replace('\n', " "),
        diff_short
    )
}

/// Build the AI summarization prompt for a regular (non-diff) summary.
///
/// We give a few examples (few-shot) because small local models do
/// much better with concrete demos than abstract instruction.
fn build_prompt(command: &str, output: &str) -> String {
    let cmd_short: String = command.chars().take(120).collect();
    // Output cap is enforced earlier in truncate_for_prompt; we cap
    // here as a final safety net for the prompt-template itself.
    let out_short: String = output.chars().take(4000).collect();
    format!(
        "You are a friendly assistant explaining terminal output to a \
         person who is NOT a developer. Be brief and helpful. ONE OR TWO \
         short sentences. No markdown, no code blocks, no quotes.\n\
         \n\
         CRITICAL: Output ONLY the summary itself. Do NOT preface with \
         phrases like 'Sure,', 'Here is', 'Here's a brief explanation', \
         'Based on the output', 'The output shows', 'This shows', 'I see', \
         'Let me explain', or any similar preamble. Start IMMEDIATELY \
         with the substance. Do NOT introduce yourself or restate the \
         question. The user will read your text right next to the command, \
         so context is already obvious.\n\
         \n\
         Examples:\n\
         \n\
         Command: ls -la\n\
         Output:\n\
         total 24\n\
         drwxr-xr-x  4 alice alice 4096 Nov  3 10:12 .\n\
         drwxr-xr-x 24 alice alice 4096 Nov  2 16:00 ..\n\
         -rw-r--r--  1 alice alice 1234 Nov  3 10:12 README.md\n\
         -rw-r--r--  1 alice alice 5678 Nov  2 16:00 main.py\n\
         drwxr-xr-x  2 alice alice 4096 Nov  3 10:12 src\n\
         Summary: This folder has a README, a Python file, and a 'src' subfolder. Mostly Python code in progress.\n\
         \n\
         Command: dust\n\
         Output:\n\
          1.2G  ./node_modules\n\
          800M  ./.git\n\
           45M  ./build\n\
           12M  ./src\n\
         Summary: Most of the space is from JavaScript dependencies (node_modules) and git history. Source code is small.\n\
         \n\
         Command: ps aux | head\n\
         Output:\n\
         USER       PID %CPU %MEM  COMMAND\n\
         alice     2341  4.2  1.1  firefox\n\
         alice     5678  0.5  0.3  bash\n\
         Summary: Firefox is running and using a bit of CPU. Otherwise the system is quiet.\n\
         \n\
         Command: {}\n\
         Output:\n\
         {}\n\
         Summary:",
        cmd_short.replace('\\', " ").replace('\n', " "),
        out_short
    )
}

/// v0.1.378 (email arc, ship 2 of 3): email-specific prompt.
///
/// The generic `build_prompt` template is tuned for terminal
/// command output — short, factual descriptions of what `ls`
/// or `dust` showed. Applied to an email it produces tautological
/// summaries like "This output shows an email with a From header,
/// a Subject header, and a body." Useless.
///
/// The email prompt reframes the question: "what's this email
/// about" + "what does it want from the reader." Worth ~50
/// extra tokens of prompt for vastly more useful output.
///
/// We keep the same anti-preamble discipline and the same
/// brevity bound (1-2 sentences) so the surrounding UI doesn't
/// need an email-specific layout.
fn build_email_prompt(command: &str, output: &str) -> String {
    let cmd_short: String = command.chars().take(120).collect();
    // Emails can be long. Cap higher than the generic prompt
    // (4000) since header context + body content together
    // benefit from being seen as one unit. 6000 chars
    // ≈ ~1500 tokens of input + ~50 tokens output fits in
    // every model we use comfortably.
    let out_short: String = output.chars().take(6000).collect();
    format!(
        "You are a friendly assistant explaining an email to its \
         recipient. Be brief and helpful. ONE OR TWO short sentences. \
         No markdown, no code blocks, no quotes.\n\
         \n\
         Tell the reader (a) WHAT the email is about in plain language, \
         and (b) WHAT IT ASKS THEM TO DO if anything (reply, click a \
         link, attend something, sign something). If it's purely \
         informational, say so. If it looks like spam or a phishing \
         attempt, flag it.\n\
         \n\
         CRITICAL: Output ONLY the summary itself. Do NOT preface with \
         phrases like 'Sure,', 'Here is', 'This email', 'The sender', \
         or any similar preamble. Start IMMEDIATELY with the substance. \
         Do not restate the subject line verbatim — the user can read \
         that themselves.\n\
         \n\
         Examples:\n\
         \n\
         Command: cat 0001.eml\n\
         Output:\n\
         From: alice@acme.com\n\
         To: bob@example.com\n\
         Subject: Lunch Friday?\n\
         Date: Mon, 1 Jan 2024 10:00:00 +0000\n\
         \n\
         Hey Bob, are you free for lunch this Friday around noon? \
         Thinking we could try that new ramen place near the office. \
         Let me know!\n\
         Summary: Alice wants to know if Friday noon works for lunch at a new ramen place. Reply with a yes or no.\n\
         \n\
         Command: mail -e\n\
         Output:\n\
         From: noreply@github.com\n\
         To: dev@example.com\n\
         Subject: [repo] Build #4032 failed on main\n\
         \n\
         The CI build on main has failed. View logs at https://...\n\
         Summary: A CI build failed on the main branch — needs investigation. No reply expected; click through for logs.\n\
         \n\
         Command: cat phish.eml\n\
         Output:\n\
         From: security@paypa1.com\n\
         To: you@example.com\n\
         Subject: URGENT: verify your account\n\
         \n\
         Your account has been suspended. Click here within 24 hours to verify.\n\
         Summary: Likely phishing — sender domain is misspelled (paypa1 vs paypal) and pressures urgency. Do NOT click.\n\
         \n\
         Command: {}\n\
         Output:\n\
         {}\n\
         Summary:",
        cmd_short.replace('\\', " ").replace('\n', " "),
        out_short
    )
}

/// Strip common LLM preface phrases from a generated summary.
///
/// Even with strong "no preamble" instructions in the prompt, smaller
/// models (gemma:2b, llama3.2:1b, tinyllama) frequently produce
/// preambles like "Sure, here's a brief explanation:" or "The output
/// shows that...". This post-processor removes them so the summary
/// reads as the substance the user actually wants.
///
/// Strategy:
///   1. Lowercase-prefix-match against a list of known preamble phrases
///   2. If matched, advance past the preamble (and any colon/comma/dash
///      that follows)
///   3. If the remaining text starts with a lowercase letter (because
///      we lopped off "Sure," to leave "here's the..."), capitalize.
///   4. Repeat once in case the model stacked two prefaces.
///
/// Conservative: we only strip recognized phrases, never substantive
/// content. False negatives (some preamble slips through) are better
/// than false positives (real content stripped).
pub fn strip_preamble(s: &str) -> String {
    let mut text = s.trim().to_string();

    // Phrases to detect (lowercase-compared). Order roughly by length —
    // longer phrases first so we don't half-match (e.g. "here's" before
    // "here's a brief explanation").
    const PREAMBLES: &[&str] = &[
        "sure, here's a brief explanation of the output you provided:",
        "sure, here is a brief explanation of the output you provided:",
        "sure, here's a brief explanation of the output:",
        "sure, here's a quick summary of the output:",
        "here's a brief explanation of the output you provided:",
        "here is a brief explanation of the output you provided:",
        "here's a brief explanation of the output:",
        "based on the output you provided,",
        "based on the output provided,",
        "based on the command output,",
        "based on the output,",
        "looking at the output,",
        "looking at the command output,",
        "from what i can see,",
        "from the output,",
        "let me explain.",
        "let me explain:",
        "let me break this down.",
        "let me break this down:",
        "the output shows that",
        "the output shows",
        "the command output shows",
        "the command shows",
        "this output shows",
        "this shows that",
        "this shows",
        "this command shows",
        "this command",
        "in summary,",
        "to summarize,",
        "summary:",
        "here's a quick summary:",
        "here is a quick summary:",
        "here's a summary:",
        "here is a summary:",
        "here's the summary:",
        "here is the summary:",
        "here's a brief summary:",
        "here's a brief breakdown:",
        "here's a breakdown:",
        "here is a breakdown:",
        "here is what's happening:",
        "here's what's happening:",
        "here is what happened:",
        "here's what happened:",
        "here is what is happening:",
        "here's what is happening:",
        "sure, here's",
        "sure! here's",
        "sure, here is",
        "sure! here is",
        "certainly, here's",
        "certainly! here's",
        "certainly, here is",
        "of course, here's",
        "of course! here's",
        "absolutely, here's",
        "okay, here's",
        "okay! here's",
        "ok, here's",
        "alright, here's",
        "great question!",
        "good question!",
        "i see.",
        "i see!",
        "got it.",
        "got it!",
        "understood.",
        "understood!",
        "sure.",
        "sure!",
        "certainly.",
        "certainly!",
        "absolutely.",
        "absolutely!",
        "of course.",
        "of course!",
    ];

    // Up to 2 passes: strip a preamble, look at remainder, strip again
    // if it ALSO starts with one. (Some models stack: "Sure! Looking
    // at the output, ...".)
    for _ in 0..2 {
        let lc = text.to_lowercase();
        let lc_trimmed = lc.trim_start();
        // Track how much leading whitespace the trim ate so we can
        // advance the original (case-preserved) text by the right amount.
        let leading_ws = lc.len() - lc_trimmed.len();

        let mut stripped = false;
        for &pat in PREAMBLES {
            if lc_trimmed.starts_with(pat) {
                // Advance past pat in the original string.
                let advance = leading_ws + pat.len();
                if advance >= text.len() {
                    text.clear();
                } else {
                    text = text[advance..].to_string();
                }
                // Eat any leading punctuation ("," "." ":" "-" "—") and
                // whitespace that the preamble left dangling.
                text = text.trim_start_matches(|c: char|
                    c.is_whitespace() || c == ',' || c == '.' || c == ':'
                        || c == '-' || c == '—' || c == ';'
                ).to_string();
                stripped = true;
                break;
            }
        }
        if !stripped { break; }
    }

    // Capitalize first letter if it's lowercase (we lopped off
    // "Sure," and revealed "here's the..." → "Here's the...").
    if let Some(c) = text.chars().next() {
        if c.is_lowercase() {
            let mut chars = text.chars();
            let first = chars.next().unwrap();
            text = first.to_uppercase().collect::<String>() + chars.as_str();
        }
    }

    text
}

/// Start the background summary worker. Idempotent.
pub fn start_worker() {
    use std::sync::atomic::AtomicBool;
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    SUMMARY_QUEUE.get_or_init(|| Mutex::new(Vec::new()));
    SUMMARIES.get_or_init(|| Mutex::new(HashMap::new()));

    std::thread::spawn(|| {
        crate::clog!("log-summary", "worker started");
        loop {
            std::thread::sleep(Duration::from_millis(250));
            if !is_enabled() {
                continue;
            }
            // Defer to voice — if whisper is recording or transcribing,
            // we don't want to compete for CPU. Voice always wins.
            if VOICE_ACTIVE.load(Ordering::Relaxed) {
                continue;
            }
            // Watchdog: if too many consecutive ollama failures,
            // the queue is paused. New blocks queue up but don't
            // run. After the pause expires, we try once; if that
            // succeeds, normal flow resumes; if it fails, pause
            // doubles (capped at 600s).
            if queue_paused() {
                continue;
            }
            // Pop the FIRST request whose `not_before` time has passed.
            // Requests still in backoff stay in the queue. This way
            // failed requests can sit waiting for ollama to come up
            // without blocking newer requests behind them.
            //
            // Previous behavior dropped any request older than 30s as
            // "stale" — that meant if ollama wasn't running when the
            // user opened the app, summaries were silently abandoned
            // even after they fixed the setup. Now we keep retrying
            // up to MAX_ATTEMPTS with backoff; user can start ollama
            // mid-session and watch summaries land.
            let now = Instant::now();
            let req = match SUMMARY_QUEUE.get() {
                Some(q) => match q.lock() {
                    Ok(mut g) => {
                        let pos = g.iter().position(|r| r.not_before <= now);
                        match pos {
                            Some(p) => Some(g.remove(p)),
                            None => None,
                        }
                    }
                    Err(_) => None,
                },
                None => None,
            };
            let req = match req { Some(r) => r, None => continue };
            // Race check: voice may have started between our top-of-loop
            // check and now. If so, re-queue at the front and back off.
            if VOICE_ACTIVE.load(Ordering::Relaxed) {
                if let Some(q) = SUMMARY_QUEUE.get() {
                    if let Ok(mut g) = q.lock() {
                        g.insert(0, req);
                    }
                }
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            // Run the LLM. We pick a single model deliberately — for
            // summary, gemma:2b is a good balance of quality and speed.
            // Falls back to ollama_query_multi for safety if gemma:2b
            // isn't available.
            let prompt = if req.is_email {
                build_email_prompt(&req.command, &req.output)
            } else if req.is_diff {
                build_diff_prompt(&req.command, &req.output)
            } else {
                build_prompt(&req.command, &req.output)
            };
            let started = Instant::now();
            // Re-check voice activity just before we spawn ollama
            // — minimizes the window where voice could start AFTER
            // our outer check but BEFORE we burn ~2s of CPU.
            if VOICE_ACTIVE.load(Ordering::Relaxed) {
                if let Some(q) = SUMMARY_QUEUE.get() {
                    if let Ok(mut g) = q.lock() { g.insert(0, req); }
                }
                continue;
            }
            // Use the dedicated summary model (gemma:2b) when
            // available, otherwise fall through to whatever the
            // standard escalation order picks.
            //
            // For retry attempts (after a previous failure), clear
            // the model blacklist first. The blacklist exists to
            // avoid spamming a permanently-broken model in the same
            // session, but in our case the most common failure mode
            // is "ollama wasn't running" — once user starts it, all
            // models work again. Clearing on retry lets that recover.
            if req.attempts > 0 {
                crate::system_info::clear_model_blacklist();
            }
            let result = run_summary_inference(&prompt);
            match result {
                Ok((text, model)) => {
                    let trimmed = clean_summary_output(&text);
                    if trimmed.is_empty() {
                        crate::clog!("log-summary", "empty summary for block {} — re-queueing for retry",
                            req.block_id);
                        // Empty result — retry, treat as soft failure.
                        requeue_after_failure(req);
                        continue;
                    }
                    let elapsed_ms = started.elapsed().as_millis() as u32;
                    crate::clog!("log-summary", "block {} via {}: {} ({}ms)",
                        req.block_id, model, trimmed, elapsed_ms);
                    let entry = SummaryEntry {
                        block_id: req.block_id,
                        text: trimmed,
                        model,
                        duration_ms: elapsed_ms,
                        completed_at: epoch_secs(),
                        is_dumb: false,
                        // Preserve keywords from the dumb entry (if any)
                        // — they were computed from the same output so
                        // they're equally valid here. Saves recomputing
                        // and keeps the relevance-score path stable
                        // through the dumb→AI overwrite.
                        keywords: SUMMARIES.get()
                            .and_then(|m| m.lock().ok())
                            .and_then(|g| g.get(&req.block_id).map(|e| e.keywords.clone()))
                            .unwrap_or_default(),
                    };
                    if let Some(map) = SUMMARIES.get() {
                        if let Ok(mut g) = map.lock() {
                            g.insert(req.block_id, entry);
                            // Cap memory: drop oldest entries if we
                            // accumulate more than 100 summaries (a
                            // long terminal session shouldn't keep
                            // all of them in RAM).
                            if g.len() > 100 {
                                let oldest_id = g.values()
                                    .min_by_key(|e| e.completed_at)
                                    .map(|e| e.block_id);
                                if let Some(id) = oldest_id {
                                    g.remove(&id);
                                }
                            }
                        }
                    }
                    SUMMARY_REVISION.fetch_add(1, Ordering::Relaxed);
                    // Reset the consecutive-failure watchdog. If we'd been
                    // backing off a paused queue, success means ollama is
                    // healthy again and we can resume normal cadence.
                    note_summary_success();
                }
                Err(e) => {
                    crate::clog!("log-summary", "block {} failed (attempt {}): {}",
                        req.block_id, req.attempts + 1, e);
                    // Tally for the global watchdog. After 3 consecutive
                    // failures across all blocks, the queue gets paused
                    // for 90s+ to give ollama time to recover or for
                    // the user to start ollama if it's not running.
                    note_summary_failure();
                    requeue_after_failure(req);
                }
            }
        }
    });
}

/// Re-queue a failed request with backoff. Caps at MAX_ATTEMPTS — if
/// already at the cap, drops the request and logs.
fn requeue_after_failure(mut req: SummaryRequest) {
    req.attempts += 1;
    if req.attempts >= MAX_ATTEMPTS {
        crate::clog!("log-summary", "giving up on block {} after {} attempts",
            req.block_id, req.attempts);
        return;
    }
    let backoff = retry_backoff(req.attempts);
    req.not_before = Instant::now() + backoff;
    crate::clog!("log-summary", "block {} retry {} in {}s",
        req.block_id, req.attempts, backoff.as_secs());
    if let Some(q) = SUMMARY_QUEUE.get() {
        if let Ok(mut g) = q.lock() {
            g.push(req);
            // Cap queue size so failed requests don't accumulate
            // forever if ollama is permanently broken.
            while g.len() > 32 {
                g.remove(0);
            }
        }
    }
}

/// Try summary inference with a preferred model first, falling back
/// through the standard escalation if the preferred model is missing
/// or blacklisted. Returns (response_text, model_used).
///
/// v0.1.101: bails early after 2 timeouts. If the FIRST model took
/// 8+ seconds and failed, ollama is overloaded/down — running through
/// 3 more 12s timeouts buys us nothing but 36 more seconds of CPU
/// thrash. Caller's retry path will come back to us soon enough; the
/// global watchdog will pause if we keep failing.
///
/// Net change vs v0.1.100: single failed attempt = 12s (was 12s).
/// Total failure path = 24s (was 48s) before the watchdog kicks in.
fn run_summary_inference(prompt: &str) -> Result<(String, String), String> {
    // v0.1.105: dynamic priority list. Discovers installed Ollama
    // models and prioritizes any with "summariz" in the name (e.g.
    // QyrouNnet/summarizer:400m-q8_0) over the hardcoded gemma/phi3/
    // llama/tinyllama fallback. User can install a dedicated
    // summarizer with `ollama pull QyrouNnet/summarizer:400m-q8_0`
    // and we'll use it automatically without code changes.
    //
    // Also skips uninstalled fallback models (was: tried all 4 even
    // when only 1 was pulled, eating 12s/missing × 3 = 36s timeout
    // per failed inference).
    //
    // v0.1.115: summarizer-named models get a 30s timeout in
    // system_info::ollama_query_model_raw. To avoid total-time
    // blow-out (2 × 30s = 60s before bailing), we also enforce a
    // wall-clock budget of 45s across ALL attempts.
    let preferred: Vec<String> = crate::system_info::summarizer_priority_list();
    if preferred.is_empty() {
        return Err("no summarizer model available (ollama list returned nothing)".into());
    }
    const TOTAL_BUDGET_SECS: u64 = 45;
    let overall_start = Instant::now();
    let mut slow_failures = 0u32;
    for (idx, model) in preferred.iter().enumerate() {
        // Bail if we've already burned the total budget.
        if overall_start.elapsed().as_secs() >= TOTAL_BUDGET_SECS {
            crate::clog!("log-summary", "total budget {}s exhausted, bailing", TOTAL_BUDGET_SECS);
            return Err("ollama too slow (total budget exhausted)".into());
        }
        let start = Instant::now();
        match crate::system_info::ollama_query_model_raw(model, prompt) {
            Ok(text) if !text.trim().is_empty() => {
                return Ok((text, model.to_string()));
            }
            Ok(_) => continue,
            Err(e) => {
                let took = start.elapsed();
                crate::clog!("log-summary", "{} failed in {}ms: {}",
                    model, took.as_millis(), e);
                // A "slow failure" = took 8+ seconds (close to timeout).
                // Almost always means ollama is busy or process spawn
                // is contended. Cheap failures (e.g. <500ms = "binary
                // not found", "model not pulled") are fine to retry on
                // because the next model might be installed.
                if took.as_secs() >= 8 {
                    slow_failures += 1;
                }
                // Bail early once we've seen 2 slow failures in a
                // single inference call. Saves the user 24+ seconds
                // of pointless waiting.
                if slow_failures >= 2 && idx + 1 < preferred.len() {
                    eprintln!(
                        "[log-summary] {} consecutive slow failures, \
                         bailing early — global watchdog will handle backoff",
                        slow_failures
                    );
                    return Err("ollama is overloaded (early bail after slow failures)".into());
                }
                continue;
            }
        }
    }
    Err("no summarizer model available".into())
}

/// Trim and de-jargon the summary output. The model often:
///   - prepends "Summary:" because it imitates the few-shot
///   - wraps in quotes
///   - emits markdown despite being told not to
///   - emits multiple paragraphs when one line was asked
///
/// We trim those mechanically.
fn clean_summary_output(text: &str) -> String {
    let mut s = text.trim().to_string();
    // Strip leading "Summary:" if present (model echoed the label).
    for prefix in &["Summary:", "summary:", "SUMMARY:"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim().to_string();
        }
    }
    // v0.1.105: strip LLM preamble phrases ("Sure, here's a brief
    // explanation of the output you provided:" etc). Even with strong
    // "no preamble" instructions in the prompt, smaller models leak
    // these ~30% of the time. The post-processor catches the common
    // patterns. See strip_preamble docs for full strategy.
    s = strip_preamble(&s);
    // Strip surrounding quotes.
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s = s[1..s.len() - 1].to_string();
    }
    // Strip leading list-marker if present.
    for prefix in &["- ", "* ", "• "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim().to_string();
        }
    }
    // Take only the first paragraph (split on \n\n) — model occasionally
    // generates multiple even when told to be brief.
    if let Some(idx) = s.find("\n\n") {
        s = s[..idx].to_string();
    }
    // And cap raw length so a runaway model doesn't spam the UI.
    if s.len() > 400 {
        // Truncate at the last sentence boundary before 400 chars.
        let cut = s[..400].rfind(|c: char| c == '.' || c == '!' || c == '?')
            .unwrap_or(400);
        s = format!("{}...", &s[..cut]);
    }
    s.trim().to_string()
}

fn epoch_secs() -> u64 {
    crate::util::epoch_secs()
}

#[cfg(test)]
mod email_prompt_tests {
    //! v0.1.378 (email arc): tests for the email-specific
    //! prompt path. These exercise the prompt builder
    //! directly; the end-to-end behavior (queue → worker →
    //! prompt selection) is integration-level and lives
    //! under tests/.

    use super::*;

    #[test]
    fn build_email_prompt_includes_command_and_output() {
        let p = build_email_prompt("cat msg.eml",
            "From: alice@example.com\nSubject: hi\n\nhello");
        assert!(p.contains("cat msg.eml"));
        assert!(p.contains("From: alice@example.com"));
        assert!(p.contains("hello"));
    }

    #[test]
    fn build_email_prompt_asks_about_purpose_and_action() {
        // The reframing is the whole point — verify the
        // prompt explicitly asks for purpose + action.
        let p = build_email_prompt("mail", "From: a\nSubject: x\n\nbody");
        let lower = p.to_lowercase();
        assert!(lower.contains("what") && lower.contains("about"),
            "prompt should ask what the email is ABOUT");
        assert!(lower.contains("ask") || lower.contains("do"),
            "prompt should ask what the email asks the reader to DO");
    }

    #[test]
    fn build_email_prompt_includes_phishing_example() {
        // The phishing-flag example is load-bearing — small
        // models pattern-match against in-context examples
        // strongly. Removing this example would silently
        // regress phishing detection quality.
        let p = build_email_prompt("cat suspicious.eml", "From: noreply\n\nclick");
        assert!(p.to_lowercase().contains("phish"),
            "prompt should include the phishing example to anchor that case");
    }

    #[test]
    fn build_email_prompt_caps_output_length() {
        // 50k-byte input gets truncated to ≤6000 chars. The
        // resulting prompt should NOT contain the full input.
        let huge = "x".repeat(50_000);
        let p = build_email_prompt("mail", &huge);
        // v0.1.772: measure the LONGEST RUN of x's, not the total. The prompt
        // template itself contains a few isolated 'x' letters (in the example
        // text), so a raw count over-reports; the truncated input is the only
        // long run, so its length is what the cap actually governs.
        let mut longest = 0usize;
        let mut run = 0usize;
        for c in p.chars() {
            if c == 'x' { run += 1; longest = longest.max(run); } else { run = 0; }
        }
        assert!(longest <= 6000,
            "expected the truncated input run to be ≤6000 x's, got {}",
            longest);
    }

    #[test]
    fn build_email_prompt_anti_preamble_clause() {
        // Like build_prompt, build_email_prompt must instruct
        // the model not to start with "Sure," / "Here is" /
        // "This email" etc.
        let p = build_email_prompt("mail", "From: a\n\nb");
        let lower = p.to_lowercase();
        assert!(lower.contains("do not preface")
                || lower.contains("do not start"));
        assert!(lower.contains("immediately"));
    }

    #[test]
    fn build_prompt_unchanged_by_email_addition() {
        // Sanity: the non-email path should still produce a
        // prompt with the original ls -la example (regression
        // check that we didn't accidentally edit build_prompt).
        let p = build_prompt("ls", "total 0\n");
        assert!(p.contains("ls -la"),
            "build_prompt's ls -la example should still be present");
        assert!(p.contains("README.md"),
            "build_prompt's README example should still be present");
    }

    #[test]
    fn email_detection_recognizes_typical_message() {
        // Cross-module: looks_like_email is in term_blocks. We
        // use it from request_summary. Sanity-check we still
        // detect the canonical case so a refactor of either
        // side doesn't silently break the integration.
        use crate::term_blocks::CommandBlock;
        let typical = "From: alice@example.com\n\
                       To: bob@example.com\n\
                       Subject: meeting tomorrow\n\
                       Date: Mon, 1 Jan 2024 10:00:00 +0000\n\
                       \n\
                       Hey Bob, can we move to 3pm?";
        assert!(CommandBlock::looks_like_email(typical),
            "canonical email format should be detected — \
             changes to either side break the email-summary \
             path that depends on this integration");
    }

    #[test]
    fn email_detection_rejects_command_output() {
        // The detection MUST NOT fire on typical command
        // output. False positives here mean we use the email
        // prompt for shell output and confuse the model.
        use crate::term_blocks::CommandBlock;
        let cargo_output = "    Compiling web2qml v0.1.378\n\
                            warning: unused variable: `foo`\n\
                              --> src/main.rs:123:5\n";
        assert!(!CommandBlock::looks_like_email(cargo_output),
            "cargo output must NOT be flagged as email");

        let git_output = "On branch main\n\
                          Your branch is up to date with origin/main.\n\
                          \n\
                          nothing to commit, working tree clean";
        assert!(!CommandBlock::looks_like_email(git_output),
            "git status output must NOT be flagged as email");
    }
}
