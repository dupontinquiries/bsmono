//! v0.1.381 (session arc, step 2): `web2qml://sessions` page
//! renderer. Reads ParsedEntry slices from audit_log, groups
//! by session_id, renders as markdown that passes through
//! `manpages::to_qml` for consistent look.
//!
//! Session-bounded view of recent activity: each session shows
//! start time, duration, command count, first/last command.
//! Tapping the session header expands into the command list.
//! For v1 we render everything inline (no on-demand expansion)
//! since session-grouped audit slices are usually small —
//! typical session is 20-50 commands across 30-60 minutes.

use crate::audit_log::{read_recent_entries, ParsedEntry};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// v0.1.383 (session arc, step 3 completion): in-memory cache
/// of LLM-generated session summaries. Keyed on session_id.
/// Each entry stores the summary text plus the
/// `last_entry_ts_ms` it was generated against — if a newer
/// entry has joined the session since, the cache misses and we
/// re-summarize. Survives across page reloads within one
/// process; not persisted to disk (sessions are short-lived
/// enough that re-summarizing on the next launch is fine).
///
/// One process-global; concurrent access through Mutex.
struct CachedSummary {
    text: String,
    last_entry_ts_ms: u64,
    generated_at_ms: u64,
}

static SUMMARY_CACHE: Mutex<Option<std::collections::HashMap<u64, CachedSummary>>>
    = Mutex::new(None);

/// Read the cache. Returns Some(text) if a fresh-enough
/// summary exists for the session_id; None otherwise.
fn cached_summary(session_id: u64, last_entry_ts_ms: u64) -> Option<String> {
    let guard = SUMMARY_CACHE.lock().ok()?;
    let map = guard.as_ref()?;
    let entry = map.get(&session_id)?;
    if entry.last_entry_ts_ms == last_entry_ts_ms {
        Some(entry.text.clone())
    } else {
        None
    }
}

/// Insert/update a cache entry. Idempotent; overwrites any
/// existing entry for the same session_id.
pub fn store_summary(session_id: u64, last_entry_ts_ms: u64, text: String) {
    let mut guard = match SUMMARY_CACHE.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if guard.is_none() {
        *guard = Some(std::collections::HashMap::new());
    }
    if let Some(map) = guard.as_mut() {
        let generated_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        map.insert(session_id, CachedSummary {
            text,
            last_entry_ts_ms,
            generated_at_ms,
        });
    }
}

/// Build the prompt for summarizing a session. Returns a
/// string suitable for `ollama_query` / `ollama_query_multi`.
///
/// Public for direct testing — the actual LLM call lives in
/// main.rs (so we can use Backend's existing model-routing
/// and timeout handling without duplicating it here).
pub fn build_session_prompt(entries: &[&ParsedEntry]) -> String {
    let mut s = String::with_capacity(2048);
    s.push_str("Summarize what a person was doing during this terminal session. \
        ONE OR TWO short sentences. No markdown, no code blocks, no quotes. \
        Focus on the WORK, not the mechanics — \"fixed a lazy-load bug and \
        committed\" not \"ran cargo, ran git status, ran git commit.\"\n\
        \n\
        CRITICAL: Output ONLY the summary. Do NOT preface with phrases like \
        'Sure,', 'The user', 'This session', 'Based on'. Start IMMEDIATELY \
        with the substance.\n\
        \n\
        Example session:\n\
          1. cd ~/projects/web2qml\n\
          2. cargo test --features voice\n\
          3. vim src/voice.rs\n\
          4. cargo test --features voice\n\
          5. git diff src/voice.rs\n\
          6. git add src/voice.rs\n\
          7. git commit -m 'fix silence threshold'\n\
        Summary: Fixed a voice silence-threshold bug in web2qml, tested, and committed.\n\
        \n\
        Example session:\n\
          1. ls ~/Downloads\n\
          2. unzip ~/Downloads/dataset.zip -d ~/data\n\
          3. python3 process.py ~/data/dataset.csv\n\
          4. python3 process.py --verify ~/data/output.json\n\
        Summary: Unzipped a dataset and ran a processing script with verification.\n\
        \n\
        Actual session (");
    s.push_str(&entries.len().to_string());
    s.push_str(" commands):\n");
    // List up to first 25 + last 5 commands — caps prompt
    // size while still giving the model the shape of the
    // session. Long sessions usually have repetitive middles
    // (cargo test, vim X, cargo test, vim X, ...); first +
    // last captures the arc better than a uniform sample.
    let n = entries.len();
    if n <= 30 {
        for (i, e) in entries.iter().enumerate() {
            let cmd = e.command.chars().take(120).collect::<String>();
            s.push_str(&format!("  {}. {}\n", i + 1, cmd));
        }
    } else {
        for (i, e) in entries.iter().take(25).enumerate() {
            let cmd = e.command.chars().take(120).collect::<String>();
            s.push_str(&format!("  {}. {}\n", i + 1, cmd));
        }
        s.push_str(&format!("  ... ({} commands omitted) ...\n", n - 30));
        for (i, e) in entries.iter().skip(n - 5).enumerate() {
            let cmd = e.command.chars().take(120).collect::<String>();
            s.push_str(&format!("  {}. {}\n", n - 5 + i + 1, cmd));
        }
    }
    s.push_str("Summary:");
    s
}

/// How many recent audit-log entries to scan when building the
/// sessions page. 2000 is roughly several days of moderate use
/// for one person; covers the "what did I do this week" range
/// without becoming a memory pig. Sessions older than this
/// fall off the page (intentional — for older work the user
/// can grep the raw audit log).
const SESSIONS_SCAN_LIMIT: usize = 2000;

/// Maximum number of sessions to render. Beyond this we'd
/// produce a wall of headers no one scrolls through.
const MAX_SESSIONS_RENDERED: usize = 20;

/// Build the sessions-page markdown. Pure function; safe to
/// call repeatedly. main.rs wires this to `web2qml://sessions`.
pub fn render_sessions() -> String {
    let entries = read_recent_entries(SESSIONS_SCAN_LIMIT);
    render_sessions_from(&entries)
}

/// v0.1.391: pre-warm the sessions page by reading the
/// audit log once at startup so the first user-triggered
/// render doesn't pay cold-cache fs I/O. Mirror of
/// `bookmarks::prewarm_bookmarks()` — same mechanism (warm
/// the OS page cache, don't memoize since content changes
/// per command).
///
/// Different from `recipes::prewarm_recipes` (which
/// memoizes since recipes is compile-time data). Three
/// page renders, two strategies:
///
///   - Recipes: OnceLock-memoize (compile-time data)
///   - Bookmarks / Sessions: warm OS page cache
///     (content changes per save/command)
///
/// Best-effort: read failures are silently ignored. The
/// audit log might not exist yet on first launch; that's
/// fine, the synchronous render path handles that.
pub fn prewarm_sessions() {
    let _ = read_recent_entries(SESSIONS_SCAN_LIMIT);
}

/// v0.1.383: returns (session_id, last_entry_ts_ms,
/// command_count, cloned_entries) for the most recent session
/// — needed by the Backend's refresh path to know what to
/// summarize. Reads from the same audit-log scan; clones the
/// matching entries so the caller can build the prompt without
/// holding the audit-log lock.
///
/// Returns None when there are no entries OR the most recent
/// session is the pre-tracking pseudo-session (id=0).
pub fn most_recent_session_for_summary()
    -> Option<(u64, u64, Vec<ParsedEntry>)>
{
    let entries = read_recent_entries(SESSIONS_SCAN_LIMIT);
    if entries.is_empty() { return None; }

    // Find the highest session_id (None entries are
    // pseudo-session 0, which we don't summarize).
    let mut highest: Option<u64> = None;
    for e in &entries {
        if let Some(sid) = e.session_id {
            if sid != 0 && highest.map_or(true, |h| sid > h) {
                highest = Some(sid);
            }
        }
    }
    let target_sid = highest?;

    // Collect entries belonging to that session.
    let mut session_entries: Vec<ParsedEntry> = entries
        .into_iter()
        .filter(|e| e.session_id == Some(target_sid))
        .collect();
    // audit_log returns oldest-first; this preserves that.

    let last_ts = session_entries.last().map(|e| e.ts_ms).unwrap_or(0);
    if session_entries.is_empty() { return None; }

    // Cap entries we pass back at 200 — same prompt-cap
    // intent. Take the last 200 (most recent are most
    // relevant for "what did I just finish").
    if session_entries.len() > 200 {
        let drop = session_entries.len() - 200;
        session_entries.drain(..drop);
    }

    Some((target_sid, last_ts, session_entries))
}

/// Render given a slice — extracted for testability.
fn render_sessions_from(entries: &[ParsedEntry]) -> String {
    let mut s = String::with_capacity(8 * 1024);
    s.push_str("# Recent sessions\n\n");
    s.push_str("Each session is a bounded period of work — \
        commands that happened in close temporal succession \
        with the same person at the keyboard. Sessions split \
        on 15 minutes of inactivity, speaker change, or \
        process restart.\n\n");
    s.push_str("Newer sessions appear first.\n\n");

    if entries.is_empty() {
        s.push_str("*No audit-log entries yet. Run a few commands and check back.*\n");
        return s;
    }

    // Group entries by session_id. Entries with no session_id
    // (written by pre-v0.1.379 builds) all bucket together as
    // pseudo-session 0.
    //
    // BTreeMap sorts session ids ascending; we walk in reverse
    // so newer sessions appear first.
    let mut by_session: BTreeMap<u64, Vec<&ParsedEntry>> = BTreeMap::new();
    for e in entries {
        let sid = e.session_id.unwrap_or(0);
        by_session.entry(sid).or_default().push(e);
    }

    // Reverse the sort: newer (higher sid) first.
    let session_ids: Vec<u64> = by_session.keys().rev().copied().collect();

    s.push_str(&format!("**{} session(s) found in last {} entries.**\n\n",
        session_ids.len(), entries.len()));

    // v0.1.404 (session arc — command-combo detection):
    // surface frequent command sequences as candidate
    // workflows. Runs over the same entry slice. Only shown
    // when at least one candidate clears the ranking bar, so
    // the section is silent for users without repeated
    // workflows yet.
    render_frequent_sequences(&mut s, entries);

    let mut rendered = 0;
    for sid in session_ids {
        if rendered >= MAX_SESSIONS_RENDERED {
            let remaining = by_session.len() - rendered;
            s.push_str(&format!(
                "\n*({} older session(s) not shown — grep the \
                audit log for full history.)*\n",
                remaining));
            break;
        }
        let session_entries = &by_session[&sid];
        render_one_session(&mut s, sid, session_entries);
        rendered += 1;
    }

    s
}

/// v0.1.404: render the "Frequent sequences" section. Calls
/// combo_detect over the entry slice and renders up to top_n
/// candidates as a markdown list. Each candidate shows the
/// command sequence, occurrence count, day-span, and a
/// `web2qml://save-workflow?...` link that hands off to the
/// workflow-recording path (link target filed; for v1 it
/// routes to the recipe/workflow surface).
///
/// Silent (emits nothing) when no candidate clears the bar —
/// the common case for a fresh user with no repeated work.
fn render_frequent_sequences(s: &mut String, entries: &[ParsedEntry]) {
    let cfg = crate::combo_detect::ComboConfig::default();
    let combos = crate::combo_detect::detect_combos(entries, &cfg);
    if combos.is_empty() {
        return;
    }
    s.push_str("## Frequent sequences\n\n");
    s.push_str("Command sequences you've repeated across \
        sessions — candidates to save as workflows so you \
        don't retype them.\n\n");

    for (idx, c) in combos.iter().enumerate() {
        // Sequence as an inline-code arrow chain.
        let chain = c.commands.iter()
            .map(|cmd| format!("`{}`", cmd))
            .collect::<Vec<_>>()
            .join(" → ");
        // Frequency descriptor.
        let freq = if c.distinct_days >= 2 {
            format!("{}× across {} days", c.occurrences, c.distinct_days)
        } else {
            format!("{}×", c.occurrences)
        };
        let warn = if c.has_destructive {
            " ⚠️ (contains a destructive command — review before saving)"
        } else {
            ""
        };
        s.push_str(&format!("{}. {} — *{}*{}\n",
            idx + 1, chain, freq, warn));
        // Save-as-workflow handoff link. Encode the sequence
        // as a newline-joined, percent-encoded payload so the
        // workflow surface can prefill. (Link target wiring
        // filed as the proactive-suggestion follow-up; the
        // link is rendered now so the affordance exists.)
        let payload = c.commands.join("\n");
        let encoded = percent_encode_workflow(&payload);
        s.push_str(&format!(
            "   - [Save as workflow](web2qml://save-workflow?cmds={})\n",
            encoded));
    }
    s.push('\n');
}

/// Minimal percent-encoder for the save-workflow payload.
/// Encodes everything that isn't an unreserved URL char so
/// shell metacharacters in commands survive the round-trip.
/// Mirrors the encoding the mermaid/niftyplot links use.
fn percent_encode_workflow(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn render_one_session(s: &mut String, sid: u64, entries: &[&ParsedEntry]) {
    // Stats. Entries are stored oldest-first by audit_log; we
    // take first/last from the slice.
    let count = entries.len();
    let first_ts = entries.first().map(|e| e.ts_ms).unwrap_or(0);
    let last_ts = entries.last().map(|e| e.ts_ms).unwrap_or(0);
    let duration_ms = last_ts.saturating_sub(first_ts);

    // Header: format the session-id as a date when it's a
    // real unix-seconds value (v0.1.379+), or "pre-tracking"
    // when 0.
    let header = if sid == 0 {
        "## Pre-session-tracking entries".to_string()
    } else {
        format!("## Session {}", format_unix_seconds(sid))
    };
    s.push_str(&header);
    s.push_str("\n\n");

    // v0.1.383: LLM summary banner when a cached summary
    // exists for this session. The cache is keyed on
    // session_id + last_entry_ts_ms — when a new entry
    // joins the session, the next render misses and the
    // banner reads "(updating...)"; QML triggers a refresh
    // out of band. If there's no cached summary at all
    // (first visit), the banner reads "(generating...)" —
    // Backend.refresh_session_summary(sid) fires a one-shot
    // worker that produces it.
    if let Some(text) = cached_summary(sid, last_ts) {
        // Italics + quote-like indent for visual prominence
        // without being a heading.
        s.push_str(&format!("> ✨ *{}*\n\n", text.trim()));
    } else if sid != 0 {
        // Don't offer summary for the pre-tracking pseudo-
        // session — its boundaries are arbitrary.
        s.push_str("> ✨ *(session summary will appear here)*\n\n");
    }

    // Compact stats line. Duration is human-friendly.
    s.push_str(&format!(
        "*{} command(s) over {}*\n\n",
        count,
        format_duration_ms(duration_ms),
    ));

    // First and last command summary.
    if let Some(first) = entries.first() {
        let cmd = truncate_command(&first.command);
        s.push_str(&format!("- **First:** `{}`\n", cmd));
    }
    if entries.len() > 1 {
        if let Some(last) = entries.last() {
            let cmd = truncate_command(&last.command);
            s.push_str(&format!("- **Last:** `{}`\n", cmd));
        }
    }

    // Speaker attribution summary. If all commands by same
    // speaker, show once; if mixed, note that.
    let unique_speakers: std::collections::HashSet<&str> = entries.iter()
        .filter_map(|e| e.speaker_name.as_deref())
        .collect();
    if unique_speakers.len() == 1 {
        if let Some(name) = unique_speakers.iter().next() {
            s.push_str(&format!("- **Speaker:** {}\n", name));
        }
    } else if unique_speakers.len() > 1 {
        let names: Vec<&str> = unique_speakers.into_iter().collect();
        s.push_str(&format!("- **Speakers:** {}\n", names.join(", ")));
    }

    // Inline command list. Compact — just command + exit code
    // marker. Tappable detail comes in a future ship.
    if count <= 30 {
        // Full inline listing for short sessions.
        s.push_str("\n");
        for e in entries {
            let exit_marker = match e.exit_code {
                Some(0) | None => "",
                Some(_) => " ❌",
            };
            let cmd = truncate_command(&e.command);
            s.push_str(&format!("- `{}`{}\n", cmd, exit_marker));
        }
    } else {
        // Long session — show first 10 + last 5 so the user
        // sees how it started and how it ended.
        s.push_str(&format!(
            "\n*(showing first 10 and last 5 of {} commands)*\n",
            count));
        for e in entries.iter().take(10) {
            let exit_marker = match e.exit_code {
                Some(0) | None => "",
                Some(_) => " ❌",
            };
            s.push_str(&format!("- `{}`{}\n",
                truncate_command(&e.command), exit_marker));
        }
        s.push_str("\n...\n\n");
        // Last 5: take from the tail. Skip if it would
        // duplicate any of the first-10 entries (small
        // session edge case — count > 30 here, so safe).
        let tail_start = count.saturating_sub(5);
        for e in &entries[tail_start..] {
            let exit_marker = match e.exit_code {
                Some(0) | None => "",
                Some(_) => " ❌",
            };
            s.push_str(&format!("- `{}`{}\n",
                truncate_command(&e.command), exit_marker));
        }
    }

    s.push_str("\n---\n\n");
}

/// Truncate a command for the listing — long pipelines or
/// inline scripts read as `...rest` rather than blowing out
/// the layout.
fn truncate_command(cmd: &str) -> String {
    const MAX: usize = 80;
    if cmd.len() <= MAX {
        return cmd.to_string();
    }
    let mut cut = MAX;
    while cut > 0 && !cmd.is_char_boundary(cut) { cut -= 1; }
    format!("{}…", &cmd[..cut])
}

/// Format unix epoch seconds as a human-readable date.
/// Manual implementation (no chrono) — we just need
/// YYYY-MM-DD HH:MM. Same approach as audit_log's
/// chrono_stamp_or_epoch helper.
fn format_unix_seconds(secs: u64) -> String {
    // Days-from-epoch math, then back out year/month/day.
    // This is the standard "civil from days" algorithm —
    // works for any sensible date, no calendar dep.
    let days = (secs / 86400) as i64;
    let secs_today = secs % 86400;
    let hours = (secs_today / 3600) as u32;
    let minutes = ((secs_today % 3600) / 60) as u32;

    // Convert days-since-1970-01-01 → Y/M/D.
    // Algorithm from "chrono" (Howard Hinnant's date library)
    // — handles leap years correctly.
    let days = days + 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = (days - era * 146097) as u64;
    let yoe = (doe - doe/1460 + doe/36524 - doe/146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365*yoe + yoe/4 - yoe/100);
    let mp = (5*doy + 2) / 153;
    let d = doy - (153*mp + 2)/5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02} {:02}:{:02}", year, m, d, hours, minutes)
}

/// Format a duration in ms as "<X>m" or "<H>h <M>m" — for
/// the per-session summary line. Short and approximate;
/// users want a sense of scale, not millisecond precision.
fn format_duration_ms(ms: u64) -> String {
    let minutes = ms / 60_000;
    if minutes == 0 {
        let seconds = ms / 1000;
        if seconds == 0 {
            return "<1s".to_string();
        }
        return format!("{}s", seconds);
    }
    if minutes < 60 {
        return format!("{}m", minutes);
    }
    let hours = minutes / 60;
    let mins = minutes % 60;
    if mins == 0 {
        format!("{}h", hours)
    } else {
        format!("{}h {}m", hours, mins)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(ts_ms: u64, cmd: &str, session_id: Option<u64>) -> ParsedEntry {
        ParsedEntry {
            ts_ms,
            block_id: Some(1),
            command: cmd.to_string(),
            cwd: None,
            exit_code: Some(0),
            duration_ms: Some(100),
            speaker_name: None,
            speaker_confidence: None,
            attestation: "typed".to_string(),
            tree_hash: None,
            parent_block_id: None,
            output_truncated: false,
            output_first_line: None,
            session_id,
        }
    }

    #[test]
    fn render_empty_shows_message() {
        let out = render_sessions_from(&[]);
        assert!(out.contains("No audit-log entries"));
    }

    // v0.1.404: the Frequent Sequences section renders when a
    // repeated substantive sequence exists, and stays silent
    // otherwise.
    #[test]
    fn render_frequent_sequences_appears_for_repeated_combo() {
        let sid = Some(1_700_000_000u64);
        let entries = vec![
            make_entry(1_700_000_000_000, "docker compose up -d", sid),
            make_entry(1_700_000_001_000, "docker compose logs -f", sid),
            make_entry(1_700_000_002_000, "docker compose up -d", sid),
            make_entry(1_700_000_003_000, "docker compose logs -f", sid),
        ];
        let out = render_sessions_from(&entries);
        assert!(out.contains("Frequent sequences"),
            "should show the section: {}", out);
        assert!(out.contains("docker compose up -d"));
        assert!(out.contains("save-workflow"),
            "should render the save handoff link");
    }

    #[test]
    fn render_frequent_sequences_silent_without_repeats() {
        let sid = Some(1_700_000_000u64);
        let entries = vec![
            make_entry(1_700_000_000_000, "cargo build", sid),
            make_entry(1_700_000_001_000, "cargo test", sid),
        ];
        let out = render_sessions_from(&entries);
        // No repeated sequence → no section.
        assert!(!out.contains("Frequent sequences"),
            "should be silent without repeats: {}", out);
    }

    #[test]
    fn percent_encode_workflow_handles_shell_metachars() {
        let enc = percent_encode_workflow("cat foo | grep bar");
        // Spaces and pipe must be encoded.
        assert!(enc.contains("%20"), "space should encode: {}", enc);
        assert!(enc.contains("%7C"), "pipe should encode: {}", enc);
        // Alphanumerics survive.
        assert!(enc.contains("cat"));
    }

    #[test]
    fn render_single_session() {
        let entries = vec![
            make_entry(1_700_000_000_000, "ls", Some(1_700_000_000)),
            make_entry(1_700_000_005_000, "cd src", Some(1_700_000_000)),
            make_entry(1_700_000_010_000, "cargo build", Some(1_700_000_000)),
        ];
        let out = render_sessions_from(&entries);
        assert!(out.contains("1 session(s)"));
        assert!(out.contains("3 command(s)"));
        assert!(out.contains("`ls`"));
        assert!(out.contains("`cd src`"));
        assert!(out.contains("`cargo build`"));
    }

    #[test]
    fn render_multiple_sessions_newest_first() {
        let entries = vec![
            // Older session
            make_entry(1_700_000_000_000, "morning task", Some(1_700_000_000)),
            // Newer session
            make_entry(1_700_001_000_000, "afternoon task", Some(1_700_001_000)),
        ];
        let out = render_sessions_from(&entries);
        let morning_pos = out.find("morning task").expect("morning should appear");
        let afternoon_pos = out.find("afternoon task").expect("afternoon should appear");
        assert!(afternoon_pos < morning_pos,
            "newer session should render first");
    }

    #[test]
    fn render_groups_pre_tracking_entries_separately() {
        let entries = vec![
            make_entry(1_699_000_000_000, "old typed", None),
            make_entry(1_700_000_000_000, "new typed", Some(1_700_000_000)),
        ];
        let out = render_sessions_from(&entries);
        assert!(out.contains("Pre-session-tracking"));
        // Both sessions should be present.
        assert!(out.contains("old typed"));
        assert!(out.contains("new typed"));
    }

    #[test]
    fn render_shows_speaker_attribution() {
        let mut e1 = make_entry(1_700_000_000_000, "ls", Some(1_700_000_000));
        e1.speaker_name = Some("Alice".to_string());
        let mut e2 = make_entry(1_700_000_005_000, "pwd", Some(1_700_000_000));
        e2.speaker_name = Some("Alice".to_string());
        let entries = vec![e1, e2];
        let out = render_sessions_from(&entries);
        assert!(out.contains("**Speaker:** Alice"));
    }

    #[test]
    fn render_shows_mixed_speakers() {
        let mut e1 = make_entry(1_700_000_000_000, "ls", Some(1_700_000_000));
        e1.speaker_name = Some("Alice".to_string());
        let mut e2 = make_entry(1_700_000_005_000, "pwd", Some(1_700_000_000));
        e2.speaker_name = Some("Bob".to_string());
        let entries = vec![e1, e2];
        let out = render_sessions_from(&entries);
        assert!(out.contains("**Speakers:**"));
        assert!(out.contains("Alice"));
        assert!(out.contains("Bob"));
    }

    #[test]
    fn render_marks_failed_commands() {
        let mut entries = vec![
            make_entry(1_700_000_000_000, "ls /nope", Some(1_700_000_000)),
        ];
        entries[0].exit_code = Some(2);
        let out = render_sessions_from(&entries);
        assert!(out.contains("❌"),
            "failed command should be marked: {}", out);
    }

    #[test]
    fn truncate_command_handles_long_input() {
        let long = "x".repeat(200);
        let t = truncate_command(&long);
        assert!(t.ends_with("…"));
        assert!(t.chars().count() <= 81); // 80 + ellipsis
    }

    #[test]
    fn truncate_command_unicode_safe() {
        // 80+ bytes of multibyte chars must not panic.
        let s = "日本語".repeat(50);
        let t = truncate_command(&s);
        // Must be valid UTF-8 (already guaranteed by Rust
        // String type, but the slice cut must respect char
        // boundaries).
        assert!(t.ends_with("…"));
    }

    #[test]
    fn format_duration_short() {
        assert_eq!(format_duration_ms(500), "<1s");
        assert_eq!(format_duration_ms(5_000), "5s");
        assert_eq!(format_duration_ms(60_000), "1m");
        assert_eq!(format_duration_ms(3_600_000), "1h");
        assert_eq!(format_duration_ms(3_900_000), "1h 5m");
    }

    #[test]
    fn format_unix_seconds_known_date() {
        // 2024-01-01 00:00:00 UTC = 1704067200
        assert_eq!(format_unix_seconds(1704067200), "2024-01-01 00:00");
        // 2025-06-15 14:30 UTC ≈ 1749998000
        let s = format_unix_seconds(1749998000);
        assert!(s.starts_with("2025-06-15"));
    }

    // v0.1.383: LLM summary banner + cache + prompt.

    #[test]
    fn build_session_prompt_short_session_inlines_all_commands() {
        let e1 = make_entry(1_700_000_000_000, "ls", Some(1_700_000_000));
        let e2 = make_entry(1_700_000_005_000, "cd src", Some(1_700_000_000));
        let e3 = make_entry(1_700_000_010_000, "cargo build", Some(1_700_000_000));
        let refs: Vec<&ParsedEntry> = vec![&e1, &e2, &e3];
        let prompt = build_session_prompt(&refs);
        // Prompt mentions all three commands.
        assert!(prompt.contains("ls"));
        assert!(prompt.contains("cd src"));
        assert!(prompt.contains("cargo build"));
        // Asks for ONE OR TWO short sentences.
        assert!(prompt.contains("ONE OR TWO short sentences"));
        // Includes the no-preamble instruction.
        assert!(prompt.contains("CRITICAL: Output ONLY"));
        // Ends with "Summary:" prompt marker.
        assert!(prompt.trim_end().ends_with("Summary:"));
    }

    #[test]
    fn build_session_prompt_long_session_uses_first_25_plus_last_5() {
        let mut entries: Vec<ParsedEntry> = Vec::new();
        for i in 0..50 {
            entries.push(make_entry(
                1_700_000_000_000 + (i * 1000) as u64,
                &format!("cmd_{}", i),
                Some(1_700_000_000),
            ));
        }
        let refs: Vec<&ParsedEntry> = entries.iter().collect();
        let prompt = build_session_prompt(&refs);
        // Should mention first command + last command + the
        // omitted-count marker.
        assert!(prompt.contains("cmd_0"),
            "first command must appear in prompt");
        assert!(prompt.contains("cmd_49"),
            "last command must appear in prompt");
        assert!(prompt.contains("commands omitted"),
            "long sessions must show the omitted marker");
        // Middle command (cmd_30) should NOT appear since
        // it's outside the first-25/last-5 windows.
        assert!(!prompt.contains("cmd_30"),
            "middle commands should be omitted in long sessions");
    }

    #[test]
    fn build_session_prompt_includes_worked_example() {
        let e1 = make_entry(1_700_000_000_000, "ls", Some(1_700_000_000));
        let refs: Vec<&ParsedEntry> = vec![&e1];
        let prompt = build_session_prompt(&refs);
        // The prompt teaches the model the desired format
        // via examples — at least one example must be
        // present so the model learns the "Fixed X, tested,
        // committed" style.
        assert!(prompt.contains("Example session:"),
            "prompt must include at least one example: {}", prompt);
    }

    #[test]
    fn cache_round_trip() {
        // store_summary + cached_summary should round-trip
        // when the last_entry_ts_ms matches.
        let sid = 1_700_999_000;
        let last_ts = 1_700_999_120_000;
        store_summary(sid, last_ts, "Test summary text".into());
        let got = cached_summary(sid, last_ts);
        assert_eq!(got.as_deref(), Some("Test summary text"));
    }

    #[test]
    fn cache_miss_when_last_ts_changes() {
        // If a new entry joined the session (last_ts moves
        // forward), the cache should miss so the page
        // re-summarizes.
        let sid = 1_700_999_500;
        store_summary(sid, 1000, "old".into());
        // Query with a different last_ts.
        let got = cached_summary(sid, 2000);
        assert_eq!(got, None);
    }

    #[test]
    fn cache_overwrite_keeps_latest() {
        let sid = 1_700_999_700;
        store_summary(sid, 1000, "first".into());
        store_summary(sid, 2000, "second".into());
        // First-ts query misses (newer entry replaced it).
        assert_eq!(cached_summary(sid, 1000), None);
        // Latest-ts query hits.
        assert_eq!(cached_summary(sid, 2000).as_deref(),
            Some("second"));
    }

    #[test]
    fn cache_miss_when_no_entry() {
        // Fresh session id, never stored.
        let sid = 9_999_999_999;
        assert_eq!(cached_summary(sid, 1234), None);
    }

    #[test]
    fn render_uses_cached_summary() {
        let sid = 1_700_888_000;
        let last_ts = 1_700_888_005_000;
        store_summary(sid, last_ts,
            "Test session was a smoke test.".into());
        let entries = vec![
            make_entry(1_700_888_000_000, "cmd1", Some(sid)),
            make_entry(last_ts, "cmd2", Some(sid)),
        ];
        let out = render_sessions_from(&entries);
        // The banner should appear, italicized, with the
        // sparkle prefix.
        assert!(out.contains("✨"),
            "expected sparkle prefix on summary banner: {}", out);
        assert!(out.contains("Test session was a smoke test."),
            "expected summary text in render");
    }

    #[test]
    fn render_shows_placeholder_when_no_summary_cached() {
        // Fresh sid that hasn't been stored.
        let sid = 1_700_777_000;
        let entries = vec![
            make_entry(1_700_777_000_000, "cmd1", Some(sid)),
        ];
        let out = render_sessions_from(&entries);
        // Placeholder banner should appear for real sessions
        // (not for pseudo-session 0).
        assert!(out.contains("session summary will appear"),
            "expected placeholder for un-cached session: {}", out);
    }

    #[test]
    fn render_no_placeholder_for_pre_tracking_pseudo_session() {
        let entries = vec![
            make_entry(1_700_777_000_000, "cmd1", None),
        ];
        let out = render_sessions_from(&entries);
        // No banner (placeholder OR cached) for sid=0 —
        // it's not a real session.
        assert!(!out.contains("✨"),
            "pre-tracking pseudo-session should have no summary banner: {}",
            out);
    }
}
