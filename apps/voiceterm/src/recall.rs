//! v0.1.640: recall ranker — "what did I do last time" as a query. The
//! terminal remembers WITH you: "how did I fix this", "what was that docker
//! command", "the command I ran in the nginx dir last week". Pure ranker
//! over the command-block history (term_blocks::CommandBlock) you already
//! persist — blends keyword relevance with recency, dedups repeated
//! commands to their most recent + most successful instance.
//!
//! PURE + testable: it operates on a lightweight RecallItem view of the
//! block fields (command, output snippet, cwd, timestamp, exit code) so it
//! has no dependency on BlockTracker/FFI. The caller maps its CommandBlocks
//! into RecallItems and renders the ranked result.
//!
//! Why this is the thinking-surface vision realized: a generic shell has
//! Ctrl-R (exact substring, no ranking, no output/cwd/success awareness).
//! This ranks by what you MEANT — matching the command, its output, and
//! the directory — and prefers commands that actually SUCCEEDED, because
//! "how did I fix this" wants the fix, not the failed attempts.

/// A lightweight, rankable view of one past command. The caller fills
/// this from a term_blocks::CommandBlock.
#[derive(Debug, Clone)]
pub struct RecallItem {
    pub command: String,
    /// a short snippet of the command's output (caller truncates).
    pub output_snippet: String,
    /// working directory the command ran in, if known.
    pub cwd: Option<String>,
    /// unix seconds when it started (for recency).
    pub started_at: u64,
    /// exit code if known; None = unknown/still-running.
    pub exit_code: Option<i32>,
}

/// A ranked recall result.
#[derive(Debug, Clone, PartialEq)]
pub struct RecallHit {
    pub command: String,
    pub cwd: Option<String>,
    pub started_at: u64,
    pub exit_code: Option<i32>,
    pub score: f32,
}

/// Rank `items` against `query`, newest-first as a tiebreak, deduped by
/// command (keeping the best-scored instance). `now` is the current unix
/// time (for recency); `top_n` caps the result. An empty query returns the
/// most RECENT items (the "what did I just do" case), deduped.
pub fn recall(query: &str, items: &[RecallItem], now: u64, top_n: usize) -> Vec<RecallHit> {
    let terms = tokenize(query);

    // Score every item, then dedup by command keeping the highest score.
    use std::collections::HashMap;
    let mut best: HashMap<&str, RecallHit> = HashMap::new();
    for it in items {
        let score = score_item(it, &terms, now);
        if score <= 0.0 && !terms.is_empty() {
            continue; // a real query that matched nothing → drop
        }
        let entry = RecallHit {
            command: it.command.clone(),
            cwd: it.cwd.clone(),
            started_at: it.started_at,
            exit_code: it.exit_code,
            score,
        };
        match best.get(it.command.as_str()) {
            Some(prev) if prev.score >= score && prev.started_at >= it.started_at => {}
            _ => { best.insert(it.command.as_str(), entry); }
        }
    }

    let mut hits: Vec<RecallHit> = best.into_values().collect();
    // primary: score desc; tiebreak: recency desc.
    hits.sort_by(|a, b| {
        b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
            .then(b.started_at.cmp(&a.started_at))
    });
    hits.truncate(top_n);
    hits
}

fn tokenize(q: &str) -> Vec<String> {
    q.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_' && c != '.')
        .filter(|t| t.len() >= 2) // 1-char terms are noise
        .map(|t| t.to_string())
        .collect()
}

/// Score one item. Relevance dominates; recency and success are modifiers.
fn score_item(it: &RecallItem, terms: &[String], now: u64) -> f32 {
    let recency = recency_boost(it.started_at, now);
    if terms.is_empty() {
        // no query → pure recency (the "what did I just do" listing).
        return recency;
    }
    let cmd_l = it.command.to_lowercase();
    let out_l = it.output_snippet.to_lowercase();
    let cwd_l = it.cwd.as_deref().unwrap_or("").to_lowercase();

    let mut rel = 0.0f32;
    let mut matched_any = false;
    for t in terms {
        // command match is the strongest signal.
        if cmd_l.contains(t.as_str()) {
            rel += if word_boundary(&cmd_l, t) { 1.0 } else { 0.6 };
            matched_any = true;
        } else if cwd_l.contains(t.as_str()) {
            // "in the nginx dir" — directory context.
            rel += 0.5;
            matched_any = true;
        } else if out_l.contains(t.as_str()) {
            // matched in the output ("the command that printed X").
            rel += 0.4;
            matched_any = true;
        }
    }
    if !matched_any {
        return 0.0;
    }
    // require coverage: matching MORE of the query terms ranks higher.
    let coverage = matched_terms(it, terms) as f32 / terms.len() as f32;
    rel *= 0.5 + 0.5 * coverage;

    // success modifier: a succeeded command (exit 0) is usually what
    // "how did I do X" wants; a failed one is a lesser match but not zero
    // (you might be recalling the error).
    let success = match it.exit_code {
        Some(0) => 1.15,
        Some(_) => 0.9,
        None => 1.0,
    };

    rel * success + 0.25 * recency // relevance dominates; recency nudges
}

fn matched_terms(it: &RecallItem, terms: &[String]) -> usize {
    let hay = format!(
        "{} {} {}",
        it.command.to_lowercase(),
        it.output_snippet.to_lowercase(),
        it.cwd.as_deref().unwrap_or("").to_lowercase()
    );
    terms.iter().filter(|t| hay.contains(t.as_str())).count()
}

/// 0..1 recency score: ~1.0 within the last hour, decaying over ~30 days.
fn recency_boost(started_at: u64, now: u64) -> f32 {
    let age = now.saturating_sub(started_at) as f32;
    let day = 86_400.0;
    // exponential-ish decay: 1.0 now, ~0.5 at ~3 days, ~0.1 at ~30 days.
    let half_life = 3.0 * day;
    0.5f32.powf(age / half_life)
}

fn word_boundary(haystack: &str, term: &str) -> bool {
    // true if `term` appears as a whole token in `haystack`.
    haystack.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .any(|w| w == term)
}

// ── v0.1.660: cross-session persistence ──────────────────────────────
// Recall blocks are in-memory, so "what did I do last time" dies with the
// session. The kernel-dev's #1 pain is post-reboot context loss, so recall
// must survive a reboot. These pure fns serialize a capped, deduped history
// to a flat line format (one record per line, tab-delimited, escaped) and
// parse it back. The wiring layer reads/writes the file and merges the
// persisted history with the live in-memory blocks before ranking.

/// Max persisted history entries (ring buffer; oldest dropped).
pub const MAX_PERSISTED: usize = 2000;

fn esc(s: &str) -> String { s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n") }
fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(other) => { out.push(other); }
                None => out.push('\\'),
            }
        } else { out.push(c); }
    }
    out
}

/// Serialize items to the persisted line format:
/// `started_at \t exit_code \t cwd \t command \t output_snippet`
/// (exit_code "" = None; cwd "" = None). Newest order preserved.
pub fn serialize_history(items: &[RecallItem]) -> String {
    items.iter().map(|it| {
        let exit = it.exit_code.map(|e| e.to_string()).unwrap_or_default();
        let cwd = it.cwd.as_deref().unwrap_or("");
        format!("{}\t{}\t{}\t{}\t{}",
            it.started_at, exit, esc(cwd), esc(&it.command), esc(&it.output_snippet))
    }).collect::<Vec<_>>().join("\n")
}

/// Parse the persisted format back into RecallItems. Skips malformed lines.
pub fn parse_history(text: &str) -> Vec<RecallItem> {
    text.lines().filter_map(|line| {
        if line.trim().is_empty() { return None; }
        let f: Vec<&str> = line.splitn(5, '\t').collect();
        if f.len() < 5 { return None; }
        let started_at = f[0].parse::<u64>().ok()?;
        let exit_code = if f[1].is_empty() { None } else { f[1].parse::<i32>().ok() };
        let cwd = if f[2].is_empty() { None } else { Some(unesc(f[2])) };
        Some(RecallItem {
            started_at,
            exit_code,
            cwd,
            command: unesc(f[3]),
            output_snippet: unesc(f[4]),
        })
    }).collect()
}

/// Merge persisted history with live in-memory blocks: dedup by
/// (command, started_at), keep newest, sort by started_at ascending, and
/// cap to MAX_PERSISTED (drop oldest). Returns the set to persist + rank.
pub fn merge_history(persisted: Vec<RecallItem>, live: &[RecallItem]) -> Vec<RecallItem> {
    use std::collections::HashMap;
    let mut by_key: HashMap<(String, u64), RecallItem> = HashMap::new();
    for it in persisted.into_iter().chain(live.iter().cloned()) {
        by_key.insert((it.command.clone(), it.started_at), it);
    }
    let mut out: Vec<RecallItem> = by_key.into_values().collect();
    out.sort_by_key(|it| it.started_at);
    if out.len() > MAX_PERSISTED {
        let drop = out.len() - MAX_PERSISTED;
        out.drain(0..drop); // oldest first
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(cmd: &str, out: &str, cwd: Option<&str>, ts: u64, exit: Option<i32>) -> RecallItem {
        RecallItem {
            command: cmd.into(),
            output_snippet: out.into(),
            cwd: cwd.map(|s| s.to_string()),
            started_at: ts,
            exit_code: exit,
        }
    }

    const NOW: u64 = 1_000_000;

    #[test]
    fn matches_command_text() {
        let items = vec![
            item("docker compose up -d", "", None, NOW - 100, Some(0)),
            item("ls -la", "", None, NOW - 50, Some(0)),
        ];
        let hits = recall("docker", &items, NOW, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].command, "docker compose up -d");
    }

    #[test]
    fn empty_query_returns_recent() {
        let items = vec![
            item("old cmd", "", None, NOW - 100_000, Some(0)),
            item("recent cmd", "", None, NOW - 10, Some(0)),
        ];
        let hits = recall("", &items, NOW, 5);
        assert_eq!(hits[0].command, "recent cmd"); // newest first
    }

    #[test]
    fn dedups_repeated_command_keeps_recent() {
        let items = vec![
            item("git status", "", None, NOW - 100_000, Some(0)),
            item("git status", "", None, NOW - 100, Some(0)),
        ];
        let hits = recall("git status", &items, NOW, 5);
        assert_eq!(hits.len(), 1, "repeated command deduped");
        assert_eq!(hits[0].started_at, NOW - 100, "kept the most recent");
    }

    #[test]
    fn matches_cwd_context() {
        let items = vec![
            item("vim conf", "", Some("/etc/nginx"), NOW - 100, Some(0)),
            item("vim conf", "", Some("/home/me"), NOW - 200, Some(0)),
        ];
        // "nginx" only matches via cwd — but cmd is identical, so dedup
        // keeps the nginx one (higher score from the cwd match).
        let hits = recall("nginx", &items, NOW, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].cwd.as_deref(), Some("/etc/nginx"));
    }

    #[test]
    fn succeeded_outranks_failed_same_relevance() {
        let items = vec![
            item("make build", "error", None, NOW - 100, Some(2)),
            item("make install", "ok", None, NOW - 100, Some(0)),
        ];
        // both match "make" equally on relevance; the exit-0 one wins.
        let hits = recall("make", &items, NOW, 5);
        assert_eq!(hits[0].command, "make install");
    }

    #[test]
    fn coverage_more_terms_ranks_higher() {
        let items = vec![
            item("docker run", "", None, NOW - 100, Some(0)),
            item("docker compose up nginx", "", None, NOW - 100, Some(0)),
        ];
        // query has 2 terms; the item matching BOTH should win.
        let hits = recall("docker nginx", &items, NOW, 5);
        assert_eq!(hits[0].command, "docker compose up nginx");
    }

    #[test]
    fn matched_in_output() {
        let items = vec![
            item("cargo test", "FAILED: parse_url panicked", None, NOW - 100, Some(101)),
            item("ls", "", None, NOW - 50, Some(0)),
        ];
        let hits = recall("parse_url", &items, NOW, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].command, "cargo test");
    }

    #[test]
    fn no_match_returns_empty() {
        let items = vec![item("ls -la", "", None, NOW - 100, Some(0))];
        assert!(recall("kubernetes", &items, NOW, 5).is_empty());
    }

    #[test]
    fn recency_breaks_score_ties() {
        let items = vec![
            item("git pull", "", None, NOW - 10_000, Some(0)),
            item("git push", "", None, NOW - 100, Some(0)),
        ];
        // both match "git" equally; the more recent (push) wins the tie.
        let hits = recall("git", &items, NOW, 5);
        assert_eq!(hits[0].command, "git push");
    }

    #[test]
    fn respects_top_n() {
        let items = vec![
            item("git a", "", None, NOW - 10, Some(0)),
            item("git b", "", None, NOW - 20, Some(0)),
            item("git c", "", None, NOW - 30, Some(0)),
        ];
        assert_eq!(recall("git", &items, NOW, 2).len(), 2);
    }

    #[test]
    fn persist_round_trip() {
        let items = vec![
            item("python tracer.py", "ok\nrunning", Some("/home/h/asahi"), 1000, Some(0)),
            item("make", "", None, 1010, Some(2)),
        ];
        let ser = serialize_history(&items);
        let back = parse_history(&ser);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].command, "python tracer.py");
        assert_eq!(back[0].cwd.as_deref(), Some("/home/h/asahi"));
        assert_eq!(back[0].exit_code, Some(0));
        assert_eq!(back[1].exit_code, Some(2));
        assert_eq!(back[1].cwd, None);
    }

    #[test]
    fn persist_escapes_tabs_and_newlines() {
        // a command/output with tabs + newlines must survive the line format
        let items = vec![item("echo\tx", "line1\nline2", None, 5, None)];
        let back = parse_history(&serialize_history(&items));
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].command, "echo\tx");
        assert_eq!(back[0].output_snippet, "line1\nline2");
        assert_eq!(back[0].exit_code, None);
    }

    #[test]
    fn parse_skips_malformed() {
        let txt = "not enough fields\n5\t0\t\tgood cmd\toutput\n";
        let back = parse_history(txt);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].command, "good cmd");
    }

    #[test]
    fn merge_dedups_and_sorts() {
        let persisted = vec![
            item("make", "", None, 100, Some(0)),
            item("python a.py", "", None, 90, Some(0)),
        ];
        let live = vec![
            item("make", "", None, 100, Some(0)), // dup of persisted
            item("python b.py", "", None, 110, Some(0)),
        ];
        let merged = merge_history(persisted, &live);
        assert_eq!(merged.len(), 3, "dup (make@100) collapsed");
        // sorted ascending by started_at
        assert_eq!(merged[0].started_at, 90);
        assert_eq!(merged[2].started_at, 110);
    }

    #[test]
    fn merge_caps_to_max() {
        let persisted: Vec<RecallItem> = (0..(MAX_PERSISTED as u64 + 50))
            .map(|i| item("c", "", None, i, Some(0)))
            .collect();
        // distinct started_at so none dedup
        let persisted: Vec<RecallItem> = persisted.into_iter().enumerate()
            .map(|(i, mut it)| { it.command = format!("c{}", i); it })
            .collect();
        let merged = merge_history(persisted, &[]);
        assert_eq!(merged.len(), MAX_PERSISTED, "capped to MAX_PERSISTED");
        // oldest dropped → smallest started_at remaining is 50
        assert_eq!(merged[0].started_at, 50);
    }
}
