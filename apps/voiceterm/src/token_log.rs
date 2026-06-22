// v0.1.352: token accounting for LLM calls.
//
// What this module does:
//   - Provides a TokenEvent record that captures (ts, op, model,
//     input_tokens, output_tokens, cached_input_tokens) per call
//   - Appends to ~/.config/web2qml/token_log.jsonl (one JSON
//     object per line, easy to grep / tail / rm)
//   - Reads back the log on demand and produces summaries
//     (per-op totals, percentiles, top-5 spikes) suitable for
//     rendering in the w2q://stats/tokens man page
//
// What this module does NOT do:
//   - Doesn't know pricing. The pricing fields live on Backend
//     and are user-entered text. Cost calculation happens at
//     render time, not here.
//   - Doesn't enforce limits. No "you've spent $X, are you
//     sure?" warnings in v1 — explicitly deferred per the
//     conversation that produced this ship.
//   - Doesn't aggregate across sessions in memory. Each query
//     re-reads the JSONL. For typical usage (hundreds of
//     events) this is fine; if it ever becomes a problem we
//     can add an in-memory cache.
//
// Failures are best-effort: if disk write fails, log to stderr
// and continue. We don't want token accounting to block actual
// LLM responses from reaching the user.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

/// v0.1.364: time-window filter for the token-stats man page.
///
/// "Session" requires the caller to supply the unix timestamp of
/// the current web2qml session start — we don't track it inside
/// this module (process lifetime is owned by main.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeWindow {
    /// Events emitted since the current web2qml session started.
    Session,
    /// Events with a timestamp on the current local-day date.
    /// Uses the system timezone for "midnight."
    Today,
    /// Events emitted in the last 168 hours (rolling 7d, NOT
    /// calendar-week — power users care about "how much did I
    /// spend this week's-worth-of-work").
    Last7d,
    /// All events in the log. Default; matches v0.1.352 behavior.
    AllTime,
}

impl TimeWindow {
    /// Parse a `?window=<x>` URL query value. Tolerates case +
    /// alternate spellings; falls back to AllTime when unknown so
    /// stale/typo URLs don't break the page.
    pub fn parse(s: &str) -> TimeWindow {
        match s.trim().to_ascii_lowercase().as_str() {
            "session" => TimeWindow::Session,
            "today" => TimeWindow::Today,
            "7d" | "week" | "7days" | "last7d" => TimeWindow::Last7d,
            _ => TimeWindow::AllTime,
        }
    }

    /// URL slug for tab links — the inverse of `parse`.
    pub fn slug(self) -> &'static str {
        match self {
            TimeWindow::Session => "session",
            TimeWindow::Today => "today",
            TimeWindow::Last7d => "7d",
            TimeWindow::AllTime => "all",
        }
    }

    /// Human-readable label rendered in the tab nav and the
    /// "Totals (X)" section header.
    pub fn label(self) -> &'static str {
        match self {
            TimeWindow::Session => "Session",
            TimeWindow::Today => "Today",
            TimeWindow::Last7d => "Last 7 days",
            TimeWindow::AllTime => "All-time",
        }
    }
}

/// One LLM call's accounting. Serialized as JSON-per-line.
#[derive(Clone, Debug, PartialEq)]
pub struct TokenEvent {
    /// ISO 8601 timestamp at the moment of the call.
    pub ts: String,
    /// Operation tag — e.g. "block_summary", "chat".
    /// Caller chooses; we don't enumerate.
    pub op: String,
    /// Model string the API was called with, e.g. "claude-haiku-4-5".
    pub model: String,
    /// Input tokens reported by the API's usage block.
    pub input_tokens: u32,
    /// Output tokens reported by the API.
    pub output_tokens: u32,
    /// Cached input tokens (Anthropic's prompt caching reports
    /// these separately). 0 when no caching was used.
    pub cached_input_tokens: u32,
}

/// Default log path. Caller-overridable for tests.
pub fn default_log_path() -> PathBuf {
    let mut p = match dirs_home() {
        Some(h) => h,
        None => return PathBuf::from("/tmp/web2qml_token_log.jsonl"),
    };
    p.push(".config");
    p.push("web2qml");
    let _ = std::fs::create_dir_all(&p);
    p.push("token_log.jsonl");
    p
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// Append an event to the log. Best-effort: errors log to
/// stderr and are swallowed. Callers should not gate the LLM
/// response on this succeeding.
pub fn append_event(event: TokenEvent) {
    let path = default_log_path();
    append_event_to(event, &path);
}

/// Test seam: write to an explicit path.
pub fn append_event_to(event: TokenEvent, path: &PathBuf) {
    let line = serialize_event(&event);
    let f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path);
    let mut f = match f {
        Ok(f) => f,
        Err(e) => {
            crate::clog!("token-log", "open {} failed: {}", path.display(), e);
            return;
        }
    };
    if let Err(e) = writeln!(f, "{}", line) {
        crate::clog!("token-log", "write failed: {}", e);
    }
}

/// Serialize a TokenEvent to a JSON line (no trailing newline).
/// Hand-rolled because the codebase doesn't pull in serde for
/// most paths. Format is deliberately stable:
///
/// `{"ts":"...","op":"...","model":"...","in":N,"out":N,"cached_in":N}`
pub fn serialize_event(e: &TokenEvent) -> String {
    let mut out = String::with_capacity(160);
    out.push_str("{\"ts\":");
    json_str(&mut out, &e.ts);
    out.push_str(",\"op\":");
    json_str(&mut out, &e.op);
    out.push_str(",\"model\":");
    json_str(&mut out, &e.model);
    out.push_str(",\"in\":");
    out.push_str(&e.input_tokens.to_string());
    out.push_str(",\"out\":");
    out.push_str(&e.output_tokens.to_string());
    out.push_str(",\"cached_in\":");
    out.push_str(&e.cached_input_tokens.to_string());
    out.push('}');
    out
}

fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parse a single JSONL line back into a TokenEvent. Tolerant
/// of unknown fields and field order, but expects the exact
/// keys above. Returns None on any parse failure (corrupt line,
/// missing required field, etc.).
pub fn parse_event(line: &str) -> Option<TokenEvent> {
    let line = line.trim();
    if line.is_empty() || !line.starts_with('{') { return None; }
    Some(TokenEvent {
        ts: extract_str(line, "\"ts\"")?,
        op: extract_str(line, "\"op\"")?,
        model: extract_str(line, "\"model\"")?,
        input_tokens: extract_num(line, "\"in\"")?,
        output_tokens: extract_num(line, "\"out\"")?,
        cached_input_tokens: extract_num(line, "\"cached_in\"").unwrap_or(0),
    })
}

fn extract_str(line: &str, key: &str) -> Option<String> {
    let idx = line.find(key)?;
    let after = &line[idx + key.len()..];
    let colon = after.find(':')?;
    let after = &after[colon + 1..];
    let quote = after.find('"')?;
    let body = &after[quote + 1..];
    let mut out = String::new();
    let mut esc = false;
    for c in body.chars() {
        if esc {
            match c {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                _ => out.push(c),
            }
            esc = false;
            continue;
        }
        if c == '\\' { esc = true; continue; }
        if c == '"' { break; }
        out.push(c);
    }
    Some(out)
}

fn extract_num(line: &str, key: &str) -> Option<u32> {
    let idx = line.find(key)?;
    let after = &line[idx + key.len()..];
    let colon = after.find(':')?;
    let after = &after[colon + 1..];
    // Skip whitespace, then read digits.
    let trimmed = after.trim_start();
    let mut end = 0;
    for c in trimmed.chars() {
        if c.is_ascii_digit() { end += 1; } else { break; }
    }
    if end == 0 { return None; }
    trimmed[..end].parse().ok()
}

/// Load all events. Best-effort: corrupt lines are skipped with
/// a stderr warning.
pub fn load_events() -> Vec<TokenEvent> {
    load_events_from(&default_log_path())
}

pub fn load_events_from(path: &PathBuf) -> Vec<TokenEvent> {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut events = Vec::new();
    for (i, line) in content.lines().enumerate() {
        if line.trim().is_empty() { continue; }
        match parse_event(line) {
            Some(e) => events.push(e),
            None => crate::clog!("token-log", "skipping corrupt line {}: {}",
                i + 1, line.chars().take(60).collect::<String>()),
        }
    }
    events
}

/// v0.1.365: delete the entire token log. Best-effort: any
/// filesystem error logs to stderr and returns false. Used by
/// the reset link on the token-stats man page.
///
/// Backs up the log to a sibling `.bak` file first so the user
/// can recover if they reset by accident. Old `.bak` from a
/// prior reset is overwritten. We DON'T provide a UI to
/// recover — the backup is for users who notice immediately
/// and run `mv token_log.jsonl.bak token_log.jsonl`.
pub fn reset_log() -> bool {
    reset_log_at(&default_log_path())
}

pub fn reset_log_at(path: &PathBuf) -> bool {
    let bak_path = {
        let mut p = path.clone();
        p.set_extension("jsonl.bak");
        p
    };
    // Try to rename current to .bak. If the log doesn't exist,
    // there's nothing to do — count that as success.
    match std::fs::rename(path, &bak_path) {
        Ok(_) => {
            eprintln!(
                "[token-log] reset: moved {} → {}",
                path.display(), bak_path.display()
            );
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            crate::clog!("token-log", "reset: no log to reset (already empty)");
            true
        }
        Err(e) => {
            crate::clog!("token-log", "reset failed: {}", e);
            false
        }
    }
}

/// Summary statistics for one operation type.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct OpStats {
    pub op: String,
    pub count: u32,
    pub total_in: u64,
    pub total_out: u64,
    pub total_cached_in: u64,
    pub min_in: u32,
    pub median_in: u32,
    pub max_in: u32,
    pub p99_in: u32,
    pub min_out: u32,
    pub median_out: u32,
    pub max_out: u32,
    pub p99_out: u32,
}

/// Aggregate stats by operation. Returns one OpStats per
/// distinct op tag. Per-op vectors of input/output token counts
/// are sorted to compute percentiles.
pub fn stats_by_op(events: &[TokenEvent]) -> Vec<OpStats> {
    use std::collections::BTreeMap;
    let mut buckets: BTreeMap<String, Vec<&TokenEvent>> = BTreeMap::new();
    for e in events {
        buckets.entry(e.op.clone()).or_default().push(e);
    }
    buckets.into_iter().map(|(op, evs)| {
        let count = evs.len() as u32;
        let mut ins: Vec<u32> = evs.iter().map(|e| e.input_tokens).collect();
        let mut outs: Vec<u32> = evs.iter().map(|e| e.output_tokens).collect();
        ins.sort_unstable();
        outs.sort_unstable();
        let total_in: u64 = ins.iter().map(|n| *n as u64).sum();
        let total_out: u64 = outs.iter().map(|n| *n as u64).sum();
        let total_cached_in: u64 = evs.iter().map(|e| e.cached_input_tokens as u64).sum();
        OpStats {
            op,
            count,
            total_in,
            total_out,
            total_cached_in,
            min_in: *ins.first().unwrap_or(&0),
            median_in: pct(&ins, 50),
            max_in: *ins.last().unwrap_or(&0),
            p99_in: pct(&ins, 99),
            min_out: *outs.first().unwrap_or(&0),
            median_out: pct(&outs, 50),
            max_out: *outs.last().unwrap_or(&0),
            p99_out: pct(&outs, 99),
        }
    }).collect()
}

/// Percentile of a sorted slice. p=50 → median, p=99 → top 1%.
/// Uses nearest-rank method; for empty slices returns 0.
pub fn pct(sorted: &[u32], p: u32) -> u32 {
    if sorted.is_empty() { return 0; }
    let p = p.min(100);
    let idx = ((p as f64 / 100.0) * sorted.len() as f64).ceil() as usize;
    let idx = idx.saturating_sub(1).min(sorted.len() - 1);
    sorted[idx]
}

/// Top N events ranked by input_tokens (descending). These are
/// the spike events — what answers "don't test with this
/// command" by showing which prior calls were unusually large.
pub fn top_spikes(events: &[TokenEvent], n: usize) -> Vec<TokenEvent> {
    let mut copy: Vec<TokenEvent> = events.to_vec();
    copy.sort_by(|a, b| b.input_tokens.cmp(&a.input_tokens));
    copy.truncate(n);
    copy
}

/// Estimated cost in USD given pricing in dollars per million
/// tokens (separate for input and output). Cached input typically
/// costs less (Anthropic: 10% of normal input), but the user
/// enters a single number for simplicity — we model cached input
/// as 10% of the input rate by default.
pub fn estimate_cost(
    events: &[TokenEvent],
    input_dollars_per_million: f64,
    output_dollars_per_million: f64,
) -> f64 {
    if input_dollars_per_million <= 0.0 && output_dollars_per_million <= 0.0 {
        return 0.0;
    }
    let mut cost = 0.0;
    for e in events {
        let in_uncached = e.input_tokens.saturating_sub(e.cached_input_tokens) as f64;
        let in_cached = e.cached_input_tokens as f64;
        let out = e.output_tokens as f64;
        cost += in_uncached * input_dollars_per_million / 1_000_000.0;
        cost += in_cached * input_dollars_per_million * 0.1 / 1_000_000.0;
        cost += out * output_dollars_per_million / 1_000_000.0;
    }
    cost
}

/// Build an ISO 8601 timestamp for "now". Uses chrono if
/// available; otherwise a basic SystemTime-derived stamp.
pub fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Crude but stable: just emit Unix-seconds as a string. The
    // man page can render it more nicely. ISO can come later.
    format!("{}", secs)
}

/// v0.1.364: parse a TokenEvent.ts string back to unix seconds.
/// The current format is unix-seconds-as-string (see now_iso);
/// this helper handles both that and a real ISO 8601 string in
/// case we switch later. Returns None on parse failure so
/// malformed lines don't crash the time-window filter — they
/// just get dropped.
pub fn parse_event_ts(ts: &str) -> Option<u64> {
    // Fast path: pure-digit unix seconds.
    if !ts.is_empty() && ts.chars().all(|c| c.is_ascii_digit()) {
        return ts.parse::<u64>().ok();
    }
    // Slow path: ISO 8601 with a 'T' separator like
    // "2026-05-17T15:30:00Z". We don't pull in chrono here;
    // crude hand-parse to (year, month, day, hh, mm, ss).
    let (date_part, time_part) = match ts.split_once('T') {
        Some(x) => x,
        None => return None,
    };
    let d: Vec<&str> = date_part.split('-').collect();
    if d.len() != 3 { return None; }
    let yr: i64 = d[0].parse().ok()?;
    let mo: i64 = d[1].parse().ok()?;
    let da: i64 = d[2].parse().ok()?;
    // Time portion: trim trailing Z / timezone suffix.
    let time_clean = time_part.trim_end_matches('Z')
        .split_once('+').map(|(a, _)| a)
        .or_else(|| time_part.trim_end_matches('Z').split_once('-').map(|(a, _)| a))
        .unwrap_or(time_part.trim_end_matches('Z'));
    let t: Vec<&str> = time_clean.split(':').collect();
    if t.len() < 2 { return None; }
    let hh: i64 = t[0].parse().ok()?;
    let mm: i64 = t[1].parse().ok()?;
    let ss: i64 = if t.len() >= 3 {
        // Strip fractional seconds.
        t[2].split('.').next()?.parse().ok()?
    } else { 0 };
    // Days from civil epoch (1970-01-01) using Howard Hinnant's
    // algorithm. Cheap, no external dep.
    let y = yr - if mo <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64;
    let m = mo as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + da - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let secs = days * 86400 + hh * 3600 + mm * 60 + ss;
    if secs < 0 { return None; }
    Some(secs as u64)
}

/// v0.1.364: filter a slice of events down to those within the
/// given time window. `session_start_unix` is the unix-seconds
/// timestamp of when the current web2qml process started — used
/// only when `window == Session`. `now_unix` is the current
/// time — passed in so tests can produce deterministic results.
///
/// Returns owned Vec rather than a Vec of references so callers
/// can pass it into stats functions that take slices.
pub fn filter_by_window(
    events: &[TokenEvent],
    window: TimeWindow,
    session_start_unix: u64,
    now_unix: u64,
) -> Vec<TokenEvent> {
    if matches!(window, TimeWindow::AllTime) {
        return events.to_vec();
    }
    let cutoff: u64 = match window {
        TimeWindow::Session => session_start_unix,
        TimeWindow::Today => {
            // Midnight today in local time. We don't have a tz
            // library; assume UTC for simplicity. Power users can
            // refine if local-tz drift becomes an issue (filed
            // follow-up).
            let day = now_unix / 86400;
            day * 86400
        }
        TimeWindow::Last7d => now_unix.saturating_sub(7 * 86400),
        TimeWindow::AllTime => 0, // unreachable above
    };
    events.iter()
        .filter(|e| {
            parse_event_ts(&e.ts).map(|t| t >= cutoff).unwrap_or(false)
        })
        .cloned()
        .collect()
}

/// Parse usage tokens out of an Anthropic Messages API response.
/// Returns (input_tokens, output_tokens, cache_read_input_tokens).
/// All zero on parse failure (best-effort — we don't want
/// instrumentation breakage to break the actual response).
///
/// Anthropic response shape (relevant fields only):
///   {
///     "content": [...],
///     "usage": {
///       "input_tokens": 1842,
///       "output_tokens": 156,
///       "cache_read_input_tokens": 0,    // optional
///       "cache_creation_input_tokens": 0 // optional
///     }
///   }
pub fn parse_anthropic_usage(json_text: &str) -> (u32, u32, u32) {
    let usage_idx = match json_text.find("\"usage\"") {
        Some(i) => i,
        None => return (0, 0, 0),
    };
    let after = &json_text[usage_idx..];
    // Take the next ~300 chars to scope our search. The usage
    // object should be tiny.
    let scope_end = after.len().min(400);
    let scope = &after[..scope_end];
    let input_tokens = extract_num(scope, "\"input_tokens\"").unwrap_or(0);
    let output_tokens = extract_num(scope, "\"output_tokens\"").unwrap_or(0);
    let cache_read = extract_num(scope, "\"cache_read_input_tokens\"").unwrap_or(0);
    (input_tokens, output_tokens, cache_read)
}

/// Render the man-page markdown from current log + user
/// pricing. Caller invokes when the user opens
/// `web2qml://stats/tokens`.
///
/// v0.1.364: takes a TimeWindow plus the session-start and
/// current unix timestamps. Caller (main.rs) extracts the
/// window from the URL query string and passes the timestamps
/// from its own bookkeeping. Pure function; no clock reads.
pub fn render_manpage(
    events: &[TokenEvent],
    input_dollars_per_million: f64,
    output_dollars_per_million: f64,
    window: TimeWindow,
    session_start_unix: u64,
    now_unix: u64,
) -> String {
    let mut out = String::with_capacity(2048);
    out.push_str("# Token usage\n\n");
    out.push_str("Live counts of LLM tokens consumed by web2qml ");
    out.push_str("operations. Updated every time a model call returns ");
    out.push_str("with a usage block.\n\n");

    // v0.1.364: time-window tab nav. Each label links to the
    // same URL with a different `?window=` slug. The currently-
    // selected tab is bolded; others are normal links so the
    // user can see which view they're on at a glance.
    out.push_str("**Window**: ");
    let windows = [
        TimeWindow::Session, TimeWindow::Today,
        TimeWindow::Last7d, TimeWindow::AllTime,
    ];
    for (i, w) in windows.iter().enumerate() {
        if i > 0 { out.push_str(" · "); }
        if *w == window {
            // Active tab: bolded plain text (NOT a link, so the
            // user can see "you are here").
            out.push_str(&format!("**{}**", w.label()));
        } else {
            out.push_str(&format!(
                "[{}](web2qml://stats/tokens?window={})",
                w.label(), w.slug(),
            ));
        }
    }
    out.push_str("\n\n");

    // v0.1.364: filter events to the selected window before
    // anything stats-related runs.
    let filtered = filter_by_window(events, window, session_start_unix, now_unix);
    let events = &filtered[..];

    if events.is_empty() {
        out.push_str(&format!(
            "_No events recorded in window **{}**. Run some commands that ",
            window.label()
        ));
        out.push_str("invoke Claude (e.g. enable log_summary and let a ");
        out.push_str("block summarize) to populate this page._\n");
        return out;
    }

    let total_events = events.len();
    let total_in: u64 = events.iter().map(|e| e.input_tokens as u64).sum();
    let total_out: u64 = events.iter().map(|e| e.output_tokens as u64).sum();
    let total_cached: u64 = events.iter().map(|e| e.cached_input_tokens as u64).sum();

    out.push_str(&format!("## Totals ({})\n\n", window.label().to_lowercase()));
    out.push_str(&format!("- Events: **{}**\n", total_events));
    out.push_str(&format!("- Input tokens: **{}**\n", total_in));
    out.push_str(&format!("- Output tokens: **{}**\n", total_out));
    if total_cached > 0 {
        out.push_str(&format!("- Cached input: **{}**\n", total_cached));
    }

    let pricing_set = input_dollars_per_million > 0.0 || output_dollars_per_million > 0.0;
    if pricing_set {
        let cost = estimate_cost(events, input_dollars_per_million, output_dollars_per_million);
        out.push_str(&format!(
            "- **Estimated cost: ${:.4}** (at ${:.2}/M input, ${:.2}/M output, cached input at 10%)\n",
            cost, input_dollars_per_million, output_dollars_per_million,
        ));
    } else {
        out.push_str("- _Cost estimate: set $/1M token prices in Settings to see this._\n");
    }

    out.push_str("\n## Per-operation breakdown\n\n");
    let by_op = stats_by_op(events);
    for s in &by_op {
        out.push_str(&format!("### `{}`\n\n", s.op));
        out.push_str(&format!("- Count: **{}**\n", s.count));
        out.push_str(&format!(
            "- Input tokens: total {}, min {}, median {}, max {}, p99 {}\n",
            s.total_in, s.min_in, s.median_in, s.max_in, s.p99_in
        ));
        out.push_str(&format!(
            "- Output tokens: total {}, min {}, median {}, max {}, p99 {}\n",
            s.total_out, s.min_out, s.median_out, s.max_out, s.p99_out
        ));
        if s.total_cached_in > 0 {
            out.push_str(&format!("- Cached input: {}\n", s.total_cached_in));
        }
        if pricing_set {
            // Per-op cost: we recompute on the subset to be exact.
            let subset: Vec<TokenEvent> = events.iter()
                .filter(|e| e.op == s.op).cloned().collect();
            let c = estimate_cost(&subset, input_dollars_per_million, output_dollars_per_million);
            out.push_str(&format!("- Estimated cost: ${:.4}\n", c));
        }
        out.push('\n');
    }

    out.push_str("## Spikes — top 5 most token-heavy events\n\n");
    out.push_str("If a single command unexpectedly spent a lot of ");
    out.push_str("tokens, it shows up here. Useful for spotting which ");
    out.push_str("commands to AVOID feeding to summaries (or which ");
    out.push_str("ones to set log_summary off for) when you're trying ");
    out.push_str("to keep spend predictable.\n\n");
    let spikes = top_spikes(events, 5);
    for (i, e) in spikes.iter().enumerate() {
        out.push_str(&format!(
            "{}. **{} in / {} out** — `{}` (model: `{}`, ts: {})\n",
            i + 1, e.input_tokens, e.output_tokens, e.op, e.model, e.ts,
        ));
    }

    out.push_str("\n## How this is recorded\n\n");
    out.push_str("Every LLM call captures `(timestamp, operation, ");
    out.push_str("model, input_tokens, output_tokens, cached_input_tokens)` ");
    out.push_str("and appends one JSON line to:\n\n");
    out.push_str("```\n~/.config/web2qml/token_log.jsonl\n```\n\n");
    out.push_str("Pricing fields live in `settings.conf` (also under ");
    out.push_str("`~/.config/web2qml/`) and only affect what's shown here ");
    out.push_str("— they are not sent anywhere.\n\n");
    // v0.1.365: reset link. Goes to a confirmation step rather
    // than wiping directly — accidental clicks would otherwise
    // destroy the user's accumulated history. The Yes link on
    // the confirm page is what actually triggers the rename.
    out.push_str("---\n\n");
    out.push_str("[Reset log…](web2qml://stats/tokens?action=reset_prompt) — ");
    out.push_str("moves the JSONL to `token_log.jsonl.bak` and starts fresh.\n");

    out
}

/// v0.1.365: confirmation page rendered when the user clicks
/// "Reset log…" on the main token-stats page. Separate render
/// fn so we don't pollute render_manpage with branching for
/// every URL variant.
pub fn render_reset_confirm() -> String {
    let mut out = String::with_capacity(512);
    out.push_str("# Reset token log?\n\n");
    out.push_str("This will move `~/.config/web2qml/token_log.jsonl` ");
    out.push_str("to `~/.config/web2qml/token_log.jsonl.bak` and start ");
    out.push_str("with an empty log.\n\n");
    out.push_str("If you reset by accident, you can recover by running:\n\n");
    out.push_str("```\nmv ~/.config/web2qml/token_log.jsonl.bak \\\n");
    out.push_str("   ~/.config/web2qml/token_log.jsonl\n```\n\n");
    out.push_str("(Any prior `.bak` from an earlier reset gets overwritten.)\n\n");
    out.push_str("---\n\n");
    out.push_str("**[Yes, reset the log](web2qml://stats/tokens?action=reset_confirm)** ");
    out.push_str("· [Cancel](web2qml://stats/tokens)\n");
    out
}

/// v0.1.365: confirmation banner rendered after a successful
/// reset. Includes a link back to the main page so the user
/// has a clear exit. The page is also implicitly empty (no
/// events → "no events recorded" placeholder fires too).
pub fn render_reset_done() -> String {
    let mut out = String::with_capacity(512);
    out.push_str("# Token log reset\n\n");
    out.push_str("Done. The previous log was moved to ");
    out.push_str("`~/.config/web2qml/token_log.jsonl.bak`. A fresh log will ");
    out.push_str("start being written on the next Claude call.\n\n");
    out.push_str("[Back to token usage page](web2qml://stats/tokens)\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(op: &str, in_t: u32, out_t: u32) -> TokenEvent {
        TokenEvent {
            ts: "1000".to_string(),
            op: op.to_string(),
            model: "claude-haiku-4-5".to_string(),
            input_tokens: in_t,
            output_tokens: out_t,
            cached_input_tokens: 0,
        }
    }

    #[test]
    fn roundtrip_serialize_parse() {
        let e = TokenEvent {
            ts: "1740000000".to_string(),
            op: "block_summary".to_string(),
            model: "claude-haiku-4-5".to_string(),
            input_tokens: 1842,
            output_tokens: 156,
            cached_input_tokens: 0,
        };
        let line = serialize_event(&e);
        let parsed = parse_event(&line).expect("must parse");
        assert_eq!(parsed, e);
    }

    #[test]
    fn parse_handles_unknown_extra_fields() {
        // Future-proofing — if we add fields later, old parsers
        // should still extract the ones they know.
        let line = "{\"ts\":\"1000\",\"op\":\"x\",\"model\":\"m\",\"in\":10,\"out\":5,\"cached_in\":0,\"future\":\"unknown\"}";
        let p = parse_event(line).expect("parse");
        assert_eq!(p.input_tokens, 10);
        assert_eq!(p.output_tokens, 5);
    }

    #[test]
    fn percentile_basic() {
        let v = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        assert_eq!(pct(&v, 50), 5);
        assert_eq!(pct(&v, 99), 10);
        assert_eq!(pct(&v, 0), 1);
    }

    #[test]
    fn percentile_empty_zero() {
        assert_eq!(pct(&[], 50), 0);
    }

    #[test]
    fn top_spikes_descending_by_input() {
        let events = vec![
            ev("a", 100, 10),
            ev("b", 5000, 20),
            ev("c", 50, 5),
            ev("d", 1200, 80),
            ev("e", 999, 99),
        ];
        let top = top_spikes(&events, 3);
        assert_eq!(top.len(), 3);
        assert_eq!(top[0].op, "b"); // 5000
        assert_eq!(top[1].op, "d"); // 1200
        assert_eq!(top[2].op, "e"); // 999
    }

    #[test]
    fn stats_by_op_groups_correctly() {
        let events = vec![
            ev("block_summary", 100, 20),
            ev("block_summary", 200, 30),
            ev("block_summary", 300, 40),
            ev("chat", 1000, 200),
        ];
        let stats = stats_by_op(&events);
        assert_eq!(stats.len(), 2);
        let bs = stats.iter().find(|s| s.op == "block_summary").unwrap();
        assert_eq!(bs.count, 3);
        assert_eq!(bs.total_in, 600);
        assert_eq!(bs.median_in, 200);
        assert_eq!(bs.max_in, 300);
        let chat = stats.iter().find(|s| s.op == "chat").unwrap();
        assert_eq!(chat.count, 1);
        assert_eq!(chat.total_in, 1000);
    }

    #[test]
    fn cost_zero_when_pricing_unset() {
        let events = vec![ev("x", 1000, 100)];
        assert_eq!(estimate_cost(&events, 0.0, 0.0), 0.0);
    }

    #[test]
    fn cost_simple_math() {
        // 1M input @ $1/M = $1
        let events = vec![TokenEvent {
            ts: "0".to_string(),
            op: "x".to_string(),
            model: "m".to_string(),
            input_tokens: 1_000_000,
            output_tokens: 0,
            cached_input_tokens: 0,
        }];
        let c = estimate_cost(&events, 1.0, 0.0);
        assert!((c - 1.0).abs() < 0.0001, "expected $1, got {}", c);
    }

    #[test]
    fn cost_cached_input_at_ten_percent() {
        // 1M input, ALL cached, $1/M → should be $0.10
        let events = vec![TokenEvent {
            ts: "0".to_string(),
            op: "x".to_string(),
            model: "m".to_string(),
            input_tokens: 1_000_000,
            output_tokens: 0,
            cached_input_tokens: 1_000_000,
        }];
        let c = estimate_cost(&events, 1.0, 0.0);
        assert!((c - 0.1).abs() < 0.0001, "expected $0.10, got {}", c);
    }

    #[test]
    fn parse_anthropic_usage_extracts_tokens() {
        let json = r#"{"content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":1842,"output_tokens":156}}"#;
        let (i, o, c) = parse_anthropic_usage(json);
        assert_eq!(i, 1842);
        assert_eq!(o, 156);
        assert_eq!(c, 0);
    }

    #[test]
    fn parse_anthropic_usage_with_cache_field() {
        let json = r#"{"usage":{"input_tokens":2000,"output_tokens":100,"cache_read_input_tokens":1500}}"#;
        let (i, o, c) = parse_anthropic_usage(json);
        assert_eq!(i, 2000);
        assert_eq!(o, 100);
        assert_eq!(c, 1500);
    }

    #[test]
    fn parse_anthropic_usage_missing_returns_zeros() {
        let json = r#"{"content":[]}"#;
        assert_eq!(parse_anthropic_usage(json), (0, 0, 0));
    }

    #[test]
    fn render_manpage_no_events_shows_placeholder() {
        let md = render_manpage(&[], 0.0, 0.0, TimeWindow::AllTime, 0, 0);
        // v0.1.364: the "no events" message now mentions the
        // selected window so the user understands a session
        // with 0 events vs all-time with 0 events. The empty-
        // events placeholder fires BEFORE filtering when there
        // are literally no events.
        assert!(md.contains("No events recorded"));
    }

    #[test]
    fn render_manpage_pricing_unset_shows_hint() {
        let events = vec![ev("block_summary", 100, 20)];
        let md = render_manpage(&events, 0.0, 0.0, TimeWindow::AllTime, 0, 0);
        assert!(md.contains("set $/1M token prices"));
    }

    #[test]
    fn render_manpage_pricing_set_shows_cost() {
        let events = vec![TokenEvent {
            ts: "0".to_string(),
            op: "x".to_string(),
            model: "m".to_string(),
            input_tokens: 1_000_000,
            output_tokens: 0,
            cached_input_tokens: 0,
        }];
        let md = render_manpage(&events, 1.0, 0.0, TimeWindow::AllTime, 0, 100);
        assert!(md.contains("$1.0000"), "missing cost in:\n{}", md);
    }

    // v0.1.364: TimeWindow tests.

    #[test]
    fn time_window_parse_basic() {
        assert_eq!(TimeWindow::parse("session"), TimeWindow::Session);
        assert_eq!(TimeWindow::parse("today"), TimeWindow::Today);
        assert_eq!(TimeWindow::parse("7d"), TimeWindow::Last7d);
        assert_eq!(TimeWindow::parse("week"), TimeWindow::Last7d);
        assert_eq!(TimeWindow::parse("all"), TimeWindow::AllTime);
        // Case-insensitive
        assert_eq!(TimeWindow::parse("SESSION"), TimeWindow::Session);
        assert_eq!(TimeWindow::parse("Today"), TimeWindow::Today);
        // Unknown falls back to AllTime
        assert_eq!(TimeWindow::parse("xyzzy"), TimeWindow::AllTime);
        assert_eq!(TimeWindow::parse(""), TimeWindow::AllTime);
    }

    #[test]
    fn time_window_slug_inverse_of_parse() {
        for w in [TimeWindow::Session, TimeWindow::Today,
                  TimeWindow::Last7d, TimeWindow::AllTime] {
            assert_eq!(TimeWindow::parse(w.slug()), w);
        }
    }

    #[test]
    fn parse_event_ts_unix_seconds() {
        assert_eq!(parse_event_ts("1700000000"), Some(1700000000));
        assert_eq!(parse_event_ts("0"), Some(0));
    }

    #[test]
    fn parse_event_ts_iso_8601() {
        // 2026-05-17T15:30:00Z = 1779031800.
        // v0.1.466: the prior expected value (1779456600) was wrong
        // — it's actually 2026-05-22T13:30:00Z, a different date
        // (the "verified manually" constant was mis-entered). The
        // parser was correct; verified the right epoch independently
        // (date -u -d "2026-05-17T15:30:00Z" +%s → 1779031800).
        let t = parse_event_ts("2026-05-17T15:30:00Z");
        assert_eq!(t, Some(1779031800));
    }

    #[test]
    fn parse_event_ts_iso_with_fraction() {
        // Fractional seconds should be stripped.
        let a = parse_event_ts("2026-05-17T15:30:00.123Z");
        let b = parse_event_ts("2026-05-17T15:30:00Z");
        assert_eq!(a, b);
    }

    #[test]
    fn parse_event_ts_rejects_garbage() {
        assert_eq!(parse_event_ts(""), None);
        assert_eq!(parse_event_ts("not-a-ts"), None);
        assert_eq!(parse_event_ts("2026/05/17"), None);
    }

    #[test]
    fn filter_by_window_alltime_returns_everything() {
        let events = vec![
            ev_at("x", 100, 20, "1000"),
            ev_at("y", 50, 10, "2000"),
            ev_at("z", 30, 5, "3000"),
        ];
        let r = filter_by_window(&events, TimeWindow::AllTime, 2000, 4000);
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn filter_by_window_session_uses_session_start() {
        let events = vec![
            ev_at("x", 100, 20, "1000"),  // before session
            ev_at("y", 50, 10, "2500"),   // during session
            ev_at("z", 30, 5, "3000"),    // during session
        ];
        let r = filter_by_window(&events, TimeWindow::Session, 2000, 4000);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].op, "y");
        assert_eq!(r[1].op, "z");
    }

    #[test]
    fn filter_by_window_last7d() {
        let now = 1_000_000_000u64;
        let seven_d = 7 * 86400;
        let events = vec![
            ev_at("old", 100, 20, &(now - seven_d - 100).to_string()),  // outside
            ev_at("recent", 50, 10, &(now - 1000).to_string()),         // inside
            ev_at("just_now", 30, 5, &now.to_string()),                 // inside
        ];
        let r = filter_by_window(&events, TimeWindow::Last7d, 0, now);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].op, "recent");
        assert_eq!(r[1].op, "just_now");
    }

    #[test]
    fn filter_by_window_today_uses_midnight() {
        // Pick a clear day boundary. 1700000000 = Tue Nov 14 22:13:20 UTC 2023
        // Midnight of that day in UTC is 1699920000.
        let now = 1700000000u64;
        let events = vec![
            ev_at("yday", 100, 20, "1699900000"),     // before midnight
            ev_at("morning", 50, 10, "1699930000"),   // after midnight, same day
            ev_at("now", 30, 5, &now.to_string()),
        ];
        let r = filter_by_window(&events, TimeWindow::Today, 0, now);
        assert_eq!(r.len(), 2);
        assert!(r.iter().any(|e| e.op == "morning"));
        assert!(r.iter().any(|e| e.op == "now"));
    }

    #[test]
    fn filter_by_window_drops_unparseable_ts() {
        let events = vec![
            ev_at("good", 100, 20, "1000"),
            ev_at("bad", 50, 10, "garbage-ts"),
        ];
        let r = filter_by_window(&events, TimeWindow::AllTime, 0, 9999);
        // AllTime bypasses the filter entirely — both included.
        assert_eq!(r.len(), 2);
        let r2 = filter_by_window(&events, TimeWindow::Session, 0, 9999);
        // Session filters by ts >= 0, so "good" (1000 >= 0) included
        // and "bad" (unparseable) DROPPED.
        assert_eq!(r2.len(), 1);
        assert_eq!(r2[0].op, "good");
    }

    #[test]
    fn render_manpage_shows_tab_nav() {
        let events = vec![ev("x", 100, 20)];
        let md = render_manpage(&events, 0.0, 0.0, TimeWindow::AllTime, 0, 100);
        // All four tabs should be present.
        assert!(md.contains("Session"));
        assert!(md.contains("Today"));
        assert!(md.contains("Last 7 days"));
        assert!(md.contains("All-time"));
        // The active tab is bolded as plain text, not a link.
        assert!(md.contains("**All-time**"));
        // The OTHER tabs ARE links.
        assert!(md.contains("[Session](web2qml://stats/tokens?window=session)"));
    }

    fn ev_at(op: &str, in_tok: u32, out_tok: u32, ts: &str) -> TokenEvent {
        TokenEvent {
            ts: ts.to_string(),
            op: op.to_string(),
            model: "m".to_string(),
            input_tokens: in_tok,
            output_tokens: out_tok,
            cached_input_tokens: 0,
        }
    }

    // v0.1.365: reset_log tests.

    #[test]
    fn reset_log_nonexistent_returns_true() {
        let p = PathBuf::from("/tmp/__test_token_log_does_not_exist.jsonl");
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file({
            let mut q = p.clone();
            q.set_extension("jsonl.bak");
            q
        });
        // Reset on a path that doesn't exist returns true (no-op success).
        assert!(reset_log_at(&p));
    }

    #[test]
    fn reset_log_moves_to_bak() {
        let p = PathBuf::from("/tmp/__test_token_log_reset.jsonl");
        let bak = {
            let mut q = p.clone();
            q.set_extension("jsonl.bak");
            q
        };
        // Cleanup before test
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(&bak);
        // Seed a log with some content
        std::fs::write(&p, "hello\n").unwrap();
        assert!(p.exists());
        assert!(reset_log_at(&p));
        // Original gone, .bak present with the seeded content
        assert!(!p.exists());
        assert!(bak.exists());
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "hello\n");
        // Cleanup
        let _ = std::fs::remove_file(&bak);
    }

    #[test]
    fn reset_log_overwrites_previous_bak() {
        let p = PathBuf::from("/tmp/__test_token_log_reset_overwrite.jsonl");
        let bak = {
            let mut q = p.clone();
            q.set_extension("jsonl.bak");
            q
        };
        // Cleanup
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(&bak);
        // First reset
        std::fs::write(&p, "first\n").unwrap();
        assert!(reset_log_at(&p));
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "first\n");
        // Second reset — bak should now contain "second"
        std::fs::write(&p, "second\n").unwrap();
        assert!(reset_log_at(&p));
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "second\n");
        // Cleanup
        let _ = std::fs::remove_file(&bak);
    }
}
