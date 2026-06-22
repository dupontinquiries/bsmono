// src/combo_detect.rs
//
// v0.1.404 (session arc — command-combo detection): find
// command sequences (N-grams) that repeat across sessions and
// rank them as candidate workflows.
//
// Pure analysis over a slice of ParsedEntry. No I/O, no Qt —
// the caller (sessions.rs) reads the audit log and hands us
// the entries; we return ranked candidates. This keeps the
// detection logic fully unit-testable.
//
// The value proposition of a workflow is "save keystrokes you
// otherwise retype and misremember." The ranking heuristics
// (from the todo design) bias toward that:
//   - PREFER long keystroke sequences (more saved typing)
//   - PREFER shell-specific syntax (pipes, redirects, flags —
//     the stuff users misremember)
//   - DEFER short common sequences (`git status`→`git diff` —
//     correct positives but useless suggestions)
//   - DEFER sequences with destructive commands until they've
//     repeated more times (don't suggest `rm -rf` early)
//   - PREFER sequences spanning multiple days (a real workflow,
//     not one-off iteration)

#![allow(dead_code)]

use crate::audit_log::ParsedEntry;

/// One detected candidate sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct ComboCandidate {
    /// The command sequence, in order.
    pub commands: Vec<String>,
    /// How many times this exact sequence occurred across all
    /// scanned sessions.
    pub occurrences: usize,
    /// Number of DISTINCT calendar days the sequence appeared
    /// on (a sequence seen across 5 days is a stronger
    /// workflow signal than one seen 5 times in an hour).
    pub distinct_days: usize,
    /// Whether any command in the sequence looks destructive
    /// (rm, drop, delete, etc.). Affects the
    /// suggest-readiness gate, not just the score.
    pub has_destructive: bool,
    /// Final ranking score (higher = better workflow
    /// candidate). Computed by `score_candidate`.
    pub score: f64,
}

/// Tunables for combo detection. Defaults chosen from the
/// todo design; exposed so tests can pin behavior and a
/// future ship can make them configurable.
#[derive(Clone, Debug)]
pub struct ComboConfig {
    /// Minimum sequence length (commands) to consider.
    pub min_len: usize,
    /// Maximum sequence length to consider. Longer N-grams
    /// are rarer and the combinatorics grow; cap keeps the
    /// scan bounded.
    pub max_len: usize,
    /// A sequence must occur at least this many times to be a
    /// candidate at all.
    pub min_occurrences: usize,
    /// Destructive sequences need this many occurrences before
    /// we'll surface them (higher bar than min_occurrences).
    pub destructive_min_occurrences: usize,
    /// How many top candidates to return.
    pub top_n: usize,
}

impl Default for ComboConfig {
    fn default() -> Self {
        Self {
            min_len: 2,
            max_len: 5,
            min_occurrences: 2,
            destructive_min_occurrences: 4,
            top_n: 5,
        }
    }
}

/// Commands considered "trivial" — short muscle-memory
/// commands that are correct positives but useless to
/// suggest as workflows. A sequence made ENTIRELY of these
/// is penalized heavily.
fn is_trivial_command(cmd: &str) -> bool {
    let c = cmd.trim();
    // Strip a leading sudo for the triviality check.
    let c = c.strip_prefix("sudo ").unwrap_or(c).trim();
    matches!(c,
        "ls" | "ls -l" | "ls -la" | "ls -a" | "ll" | "la"
        | "cd" | "cd .." | "cd -" | "cd ~"
        | "pwd" | "clear" | "cls" | "exit" | "q"
        | "git status" | "git diff" | "git log"
        | "whoami" | "date" | "echo"
    )
}

/// Heuristic: does the command contain shell-specific syntax
/// that's worth saving (pipes, redirects, command
/// substitution, multiple chained commands, complex flags)?
/// These are what users misremember.
fn has_shell_syntax(cmd: &str) -> bool {
    cmd.contains('|')
        || cmd.contains('>')
        || cmd.contains('<')
        || cmd.contains("&&")
        || cmd.contains("||")
        || cmd.contains(';')
        || cmd.contains("$(")
        || cmd.contains('`')
        || cmd.contains("xargs")
        || cmd.contains("awk")
        || cmd.contains("sed")
        || cmd.contains("grep")
        || cmd.contains("find ")
}

/// Heuristic: is the command destructive enough that we
/// shouldn't proactively suggest saving it until the user
/// has clearly normalized the behavior?
fn is_destructive(cmd: &str) -> bool {
    let c = cmd.trim().to_lowercase();
    // Whole-word-ish checks — avoid matching e.g. "format" for
    // "rm". Keep it simple but cover the common dangerous ops.
    c.starts_with("rm ")
        || c == "rm"
        || c.starts_with("rmdir ")
        || c.contains("rm -rf")
        || c.contains("rm -fr")
        || c.starts_with("dd ")
        || c.contains("mkfs")
        || c.contains("drop table")
        || c.contains("drop database")
        || c.contains("truncate ")
        || c.starts_with("git push --force")
        || c.contains("git push -f")
        || c.starts_with("git reset --hard")
        || c.starts_with("kill ")
        || c.starts_with("killall ")
        || c.starts_with("shutdown")
        || c.starts_with("reboot")
        || c.contains("> /dev/")
        || c.starts_with("chmod -r")
        || c.starts_with("chown -r")
}

/// Convert a unix-ms timestamp to a calendar day number
/// (days since epoch). Used to count distinct days a sequence
/// appears on. Uses simple integer division — DST/timezone
/// boundaries can shift a sequence by a day in edge cases,
/// which is fine for a "spans multiple days" heuristic.
fn day_number(ts_ms: u64) -> i64 {
    (ts_ms / 86_400_000) as i64
}

/// Normalize a command for sequence matching. We match on the
/// command text as typed, trimmed. We deliberately do NOT
/// normalize away arguments — `grep foo` and `grep bar` are
/// different workflows. But we collapse internal whitespace
/// runs so `ls   -l` and `ls -l` match.
fn normalize_command(cmd: &str) -> String {
    cmd.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Detect frequent command-combos in a slice of audit-log
/// entries. Entries are expected oldest-first (as
/// `read_recent_entries` returns them). Sequences are only
/// considered WITHIN a session — a sequence spanning a
/// session boundary isn't a workflow, it's two unrelated
/// bursts of work.
///
/// Returns up to `cfg.top_n` candidates, highest score first.
pub fn detect_combos(entries: &[ParsedEntry], cfg: &ComboConfig) -> Vec<ComboCandidate> {
    if entries.len() < cfg.min_len {
        return Vec::new();
    }

    // Group consecutive entries by session_id so we only form
    // N-grams within a session. None (pre-tracking) entries
    // all bucket together under pseudo-session 0 — they came
    // from older builds and we treat them as one stream.
    //
    // Build a map: normalized-sequence -> occurrence info.
    use std::collections::HashMap;
    struct SeqInfo {
        count: usize,
        days: std::collections::HashSet<i64>,
        // First occurrence's commands (already normalized) for
        // display. All occurrences share the same normalized
        // text by construction.
        commands: Vec<String>,
    }
    let mut seqs: HashMap<String, SeqInfo> = HashMap::new();

    // Walk session runs. We find maximal runs of equal
    // session_id and slide N-gram windows inside each run.
    let mut i = 0;
    while i < entries.len() {
        let sid = entries[i].session_id;
        let mut j = i;
        while j < entries.len() && entries[j].session_id == sid {
            j += 1;
        }
        // entries[i..j] is one session run.
        let run = &entries[i..j];
        // Pre-normalize once.
        let norm: Vec<String> = run.iter()
            .map(|e| normalize_command(&e.command))
            .collect();

        for n in cfg.min_len..=cfg.max_len {
            if run.len() < n { break; }
            for start in 0..=(run.len() - n) {
                let window = &norm[start..start + n];
                // Skip windows containing an empty command.
                if window.iter().any(|c| c.is_empty()) {
                    continue;
                }
                // Key on the joined sequence (using a separator
                // that can't appear in a normalized command run
                // — the unit separator).
                let key = window.join("\x1F");
                let day = day_number(run[start].ts_ms);
                let info = seqs.entry(key).or_insert_with(|| SeqInfo {
                    count: 0,
                    days: std::collections::HashSet::new(),
                    commands: window.to_vec(),
                });
                info.count += 1;
                info.days.insert(day);
            }
        }
        i = j;
    }

    // Build candidates from sequences meeting the occurrence
    // floor, then score + filter + sort.
    let mut candidates: Vec<ComboCandidate> = Vec::new();
    for info in seqs.values() {
        if info.count < cfg.min_occurrences {
            continue;
        }
        let has_destructive = info.commands.iter().any(|c| is_destructive(c));
        // Destructive sequences need a higher occurrence bar.
        if has_destructive && info.count < cfg.destructive_min_occurrences {
            continue;
        }
        let mut cand = ComboCandidate {
            commands: info.commands.clone(),
            occurrences: info.count,
            distinct_days: info.days.len(),
            has_destructive,
            score: 0.0,
        };
        cand.score = score_candidate(&cand);
        candidates.push(cand);
    }

    // Sort by score descending; tie-break by occurrences then
    // by total keystrokes (longer = more valuable).
    candidates.sort_by(|a, b| {
        b.score.partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.occurrences.cmp(&a.occurrences))
            .then(total_keystrokes(b).cmp(&total_keystrokes(a)))
    });

    candidates.truncate(cfg.top_n);
    candidates
}

/// v0.1.406 (proactive suggestion): find the single best
/// candidate worth proactively suggesting RIGHT NOW, given
/// the recent entries. Used by the toast trigger.
///
/// "Worth suggesting now" is stricter than "appears on the
/// sessions page": we only fire when a strong candidate has
/// occurred EXACTLY `suggest_at` times (default 3) — the
/// crossing point — so the toast appears once when a pattern
/// becomes established, not on every later repeat. The
/// caller additionally dedups by pattern key so a pattern
/// that re-crosses (e.g. after a restart resets the count)
/// isn't nagged repeatedly within a session.
///
/// We also require the candidate to clear a positive score
/// (so pure-trivial sequences never trigger a toast) and to
/// be non-destructive (never proactively suggest saving a
/// destructive sequence — the user can still save it via the
/// sessions page, which has the explicit ⚠️ affordance).
///
/// Returns the candidate plus a stable pattern key (the
/// newline-joined command sequence) for dedup.
pub fn proactive_suggestion(
    entries: &[ParsedEntry],
    suggest_at: usize,
) -> Option<(ComboCandidate, String)> {
    let cfg = ComboConfig {
        // Consider all candidates, not just the top 5 — the
        // one at the threshold might not be the global top.
        top_n: usize::MAX,
        ..Default::default()
    };
    let all = detect_combos(entries, &cfg);
    // Among candidates at exactly the suggestion threshold,
    // non-destructive, positive score, pick the highest score.
    let mut best: Option<&ComboCandidate> = None;
    for c in &all {
        if c.occurrences != suggest_at { continue; }
        if c.has_destructive { continue; }
        if c.score <= 0.0 { continue; }
        match best {
            Some(b) if b.score >= c.score => {}
            _ => best = Some(c),
        }
    }
    best.map(|c| {
        let key = c.commands.join("\n");
        (c.clone(), key)
    })
}

/// Total characters across the sequence's commands — proxy
/// for "keystrokes saved" if this becomes a workflow.
fn total_keystrokes(c: &ComboCandidate) -> usize {
    c.commands.iter().map(|s| s.len()).sum::<usize>()
        + c.commands.len().saturating_sub(1) // newlines between
}

/// Score a candidate. Higher = better workflow suggestion.
/// The weighting encodes the todo's preferences.
fn score_candidate(c: &ComboCandidate) -> f64 {
    let mut score = 0.0f64;

    // Base: frequency matters but with diminishing returns
    // (log-ish) so a sequence seen 50 times doesn't bury
    // everything. occurrences >= min so >= 2 here.
    score += (c.occurrences as f64).ln() * 10.0;

    // Keystrokes saved — the core value. Each char of saved
    // typing is worth a little; this is what separates a
    // 50-char pipeline from `ls; cd`.
    let ks = c.commands.iter().map(|s| s.len()).sum::<usize>();
    score += ks as f64 * 0.5;

    // Shell-syntax bonus: any command with pipes/redirects/etc
    // is the kind users misremember. Strong bonus.
    let shelly = c.commands.iter().filter(|cmd| has_shell_syntax(cmd)).count();
    score += shelly as f64 * 15.0;

    // Multi-day bonus: a sequence spanning multiple distinct
    // days is a real workflow, not one-off iteration. Strong
    // signal.
    if c.distinct_days >= 2 {
        score += 25.0;
        // Extra for each additional day, capped.
        score += ((c.distinct_days - 1).min(5) as f64) * 8.0;
    }

    // Triviality penalty: a sequence made ENTIRELY of trivial
    // muscle-memory commands is useless to suggest. Heavy
    // penalty (can go negative → filtered effectively to the
    // bottom).
    let all_trivial = c.commands.iter().all(|cmd| is_trivial_command(cmd));
    if all_trivial {
        score -= 60.0;
    } else {
        // Partial triviality: each trivial command slightly
        // dampens (a workflow that's mostly `cd`s with one
        // real command is weaker than an all-substance one).
        let trivial_count = c.commands.iter()
            .filter(|cmd| is_trivial_command(cmd)).count();
        score -= trivial_count as f64 * 4.0;
    }

    // Length sweet-spot: very short (2) sequences are weaker
    // workflow candidates than 3-4 command ones. A tiny nudge.
    if c.commands.len() >= 3 {
        score += 5.0;
    }

    score
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: build a ParsedEntry with just the fields combo
    // detection reads (command, ts_ms, session_id). Other
    // fields get defaults.
    fn entry(cmd: &str, ts_ms: u64, sid: Option<u64>) -> ParsedEntry {
        ParsedEntry {
            ts_ms,
            block_id: None,
            command: cmd.to_string(),
            cwd: None,
            exit_code: Some(0),
            duration_ms: None,
            speaker_name: None,
            speaker_confidence: None,
            attestation: String::new(),
            tree_hash: None,
            parent_block_id: None,
            output_truncated: false,
            output_first_line: None,
            session_id: sid,
        }
    }

    const DAY: u64 = 86_400_000;

    #[test]
    fn empty_input_no_candidates() {
        let cfg = ComboConfig::default();
        assert!(detect_combos(&[], &cfg).is_empty());
    }

    #[test]
    fn single_occurrence_not_surfaced() {
        // A sequence seen once doesn't meet min_occurrences.
        let entries = vec![
            entry("cargo build", 1000, Some(1)),
            entry("cargo test", 2000, Some(1)),
        ];
        let cfg = ComboConfig::default();
        assert!(detect_combos(&entries, &cfg).is_empty());
    }

    #[test]
    fn repeated_sequence_detected() {
        // Same 2-command sequence twice in one session.
        let entries = vec![
            entry("cargo build --release", 1000, Some(1)),
            entry("cargo test --all", 2000, Some(1)),
            entry("git status", 3000, Some(1)),
            entry("cargo build --release", 4000, Some(1)),
            entry("cargo test --all", 5000, Some(1)),
        ];
        let cfg = ComboConfig::default();
        let combos = detect_combos(&entries, &cfg);
        assert!(!combos.is_empty(), "should detect the repeated build+test");
        let top = &combos[0];
        assert_eq!(top.commands,
            vec!["cargo build --release".to_string(),
                 "cargo test --all".to_string()]);
        assert_eq!(top.occurrences, 2);
    }

    #[test]
    fn sequence_not_detected_across_session_boundary() {
        // The same two commands, but in different sessions —
        // NOT a workflow.
        let entries = vec![
            entry("make", 1000, Some(1)),
            entry("make install", 2000, Some(2)), // different session
            entry("make", 3000, Some(3)),
            entry("make install", 4000, Some(4)),
        ];
        let cfg = ComboConfig::default();
        let combos = detect_combos(&entries, &cfg);
        // No 2-gram forms within a single session.
        assert!(combos.is_empty(),
            "cross-session pairs shouldn't count: {:?}", combos);
    }

    #[test]
    fn trivial_only_sequence_penalized_below_substantive() {
        // A trivial sequence (ls; cd) repeated AND a
        // substantive one repeated. The substantive one must
        // rank higher.
        let entries = vec![
            // trivial pair x3
            entry("ls", 1000, Some(1)),
            entry("cd ..", 1100, Some(1)),
            entry("ls", 1200, Some(1)),
            entry("cd ..", 1300, Some(1)),
            entry("ls", 1400, Some(1)),
            entry("cd ..", 1500, Some(1)),
            // substantive pair x2
            entry("docker compose up -d", 2000, Some(1)),
            entry("docker compose logs -f web", 2100, Some(1)),
            entry("docker compose up -d", 3000, Some(1)),
            entry("docker compose logs -f web", 3100, Some(1)),
        ];
        let cfg = ComboConfig::default();
        let combos = detect_combos(&entries, &cfg);
        assert!(!combos.is_empty());
        // The substantive docker sequence should outrank the
        // trivial ls/cd one even though ls/cd occurred more.
        let top = &combos[0];
        assert!(top.commands[0].contains("docker"),
            "substantive sequence should rank first, got: {:?}", combos);
    }

    #[test]
    fn destructive_sequence_needs_higher_bar() {
        // rm sequence seen only twice (below
        // destructive_min_occurrences=4) → not surfaced.
        let entries = vec![
            entry("rm -rf build", 1000, Some(1)),
            entry("cargo build", 2000, Some(1)),
            entry("rm -rf build", 3000, Some(1)),
            entry("cargo build", 4000, Some(1)),
        ];
        let cfg = ComboConfig::default();
        let combos = detect_combos(&entries, &cfg);
        // The rm-containing 2-gram should be filtered out.
        assert!(!combos.iter().any(|c|
            c.commands.iter().any(|cmd| cmd.contains("rm -rf"))),
            "destructive seq below bar should not surface: {:?}", combos);
    }

    #[test]
    fn destructive_sequence_surfaces_after_enough_repeats() {
        // Same rm sequence, now 4 times → meets the bar.
        let mut entries = Vec::new();
        for k in 0..4 {
            entries.push(entry("rm -rf build", 1000 + k * 1000, Some(1)));
            entries.push(entry("make clean install", 1500 + k * 1000, Some(1)));
        }
        let cfg = ComboConfig::default();
        let combos = detect_combos(&entries, &cfg);
        assert!(combos.iter().any(|c| c.has_destructive),
            "destructive seq at the bar should surface: {:?}", combos);
    }

    #[test]
    fn multi_day_sequence_scores_higher() {
        // Two equivalent-frequency sequences; one spans 3 days,
        // the other is same-day. Multi-day should win.
        let entries = vec![
            // multi-day sequence: appears day 0, day 1, day 2
            entry("terraform plan", 0 * DAY + 1000, Some(1)),
            entry("terraform apply", 0 * DAY + 2000, Some(1)),
            entry("terraform plan", 1 * DAY + 1000, Some(2)),
            entry("terraform apply", 1 * DAY + 2000, Some(2)),
            entry("terraform plan", 2 * DAY + 1000, Some(3)),
            entry("terraform apply", 2 * DAY + 2000, Some(3)),
            // same-day sequence: 3x in session 4, same day
            entry("npm run lint", 3 * DAY + 1000, Some(4)),
            entry("npm run testx", 3 * DAY + 1100, Some(4)),
            entry("npm run lint", 3 * DAY + 1200, Some(4)),
            entry("npm run testx", 3 * DAY + 1300, Some(4)),
            entry("npm run lint", 3 * DAY + 1400, Some(4)),
            entry("npm run testx", 3 * DAY + 1500, Some(4)),
        ];
        let cfg = ComboConfig::default();
        let combos = detect_combos(&entries, &cfg);
        assert!(!combos.is_empty());
        let top = &combos[0];
        assert!(top.commands[0].contains("terraform"),
            "multi-day sequence should rank first: {:?}", combos);
        assert!(top.distinct_days >= 3);
    }

    #[test]
    fn normalize_collapses_whitespace() {
        assert_eq!(normalize_command("ls   -l"), "ls -l");
        assert_eq!(normalize_command("  git   commit  -m  x "), "git commit -m x");
    }

    #[test]
    fn shell_syntax_detection() {
        assert!(has_shell_syntax("cat foo | grep bar"));
        assert!(has_shell_syntax("echo hi > file.txt"));
        assert!(has_shell_syntax("make && make install"));
        assert!(has_shell_syntax("find . -name '*.rs'"));
        assert!(!has_shell_syntax("cargo build"));
    }

    #[test]
    fn destructive_detection() {
        assert!(is_destructive("rm -rf node_modules"));
        assert!(is_destructive("git push --force origin main"));
        assert!(is_destructive("dd if=/dev/zero of=/dev/sda"));
        assert!(is_destructive("DROP TABLE users"));
        assert!(!is_destructive("cargo build"));
        assert!(!is_destructive("ls -la"));
    }

    #[test]
    fn trivial_detection() {
        assert!(is_trivial_command("ls"));
        assert!(is_trivial_command("cd .."));
        assert!(is_trivial_command("git status"));
        assert!(is_trivial_command("sudo ls"));
        assert!(!is_trivial_command("docker compose up -d"));
        assert!(!is_trivial_command("cargo build --release"));
    }

    #[test]
    fn top_n_respected() {
        // Generate many distinct repeated sequences; ensure we
        // cap at top_n.
        let mut entries = Vec::new();
        let mut ts = 1000u64;
        for k in 0..10 {
            let a = format!("command-alpha-{}", k);
            let b = format!("command-beta-{}", k);
            for _ in 0..2 {
                entries.push(entry(&a, ts, Some(1))); ts += 100;
                entries.push(entry(&b, ts, Some(1))); ts += 100;
            }
        }
        let cfg = ComboConfig { top_n: 3, ..Default::default() };
        let combos = detect_combos(&entries, &cfg);
        assert!(combos.len() <= 3, "should cap at top_n=3, got {}", combos.len());
    }

    #[test]
    fn longer_ngram_detected() {
        // A 3-command sequence repeated.
        let entries = vec![
            entry("git add -A", 1000, Some(1)),
            entry("git commit -m wip", 2000, Some(1)),
            entry("git push origin feature", 3000, Some(1)),
            entry("git add -A", 4000, Some(1)),
            entry("git commit -m wip", 5000, Some(1)),
            entry("git push origin feature", 6000, Some(1)),
        ];
        let cfg = ComboConfig::default();
        let combos = detect_combos(&entries, &cfg);
        // The 3-gram should be among candidates (and rank well
        // — it's long and substantive).
        assert!(combos.iter().any(|c| c.commands.len() == 3),
            "should detect the 3-command git sequence: {:?}", combos);
    }

    // v0.1.406: proactive_suggestion tests — the threshold-
    // crossing trigger for the toast.

    #[test]
    fn proactive_fires_at_exact_threshold() {
        // A substantive 2-gram seen exactly 3 times → suggest.
        let mut entries = Vec::new();
        let mut ts = 1000u64;
        for _ in 0..3 {
            entries.push(entry("docker compose up -d", ts, Some(1))); ts += 100;
            entries.push(entry("docker compose logs -f web", ts, Some(1))); ts += 100;
        }
        let got = proactive_suggestion(&entries, 3);
        assert!(got.is_some(), "should suggest at the 3rd occurrence");
        let (cand, key) = got.unwrap();
        assert_eq!(cand.occurrences, 3);
        assert!(key.contains("docker compose up -d"));
    }

    #[test]
    fn proactive_silent_below_threshold() {
        // Same sequence only twice → not yet.
        let mut entries = Vec::new();
        let mut ts = 1000u64;
        for _ in 0..2 {
            entries.push(entry("docker compose up -d", ts, Some(1))); ts += 100;
            entries.push(entry("docker compose logs -f web", ts, Some(1))); ts += 100;
        }
        assert!(proactive_suggestion(&entries, 3).is_none(),
            "two occurrences shouldn't trigger");
    }

    #[test]
    fn proactive_silent_above_threshold() {
        // Seen 4 times — past the crossing. The toast fires
        // only AT the crossing (==3), so 4 is silent. (The
        // sessions page still lists it; this is just the
        // proactive nudge being one-shot.)
        let mut entries = Vec::new();
        let mut ts = 1000u64;
        for _ in 0..4 {
            entries.push(entry("docker compose up -d", ts, Some(1))); ts += 100;
            entries.push(entry("docker compose logs -f web", ts, Some(1))); ts += 100;
        }
        assert!(proactive_suggestion(&entries, 3).is_none(),
            "four occurrences is past the one-shot crossing");
    }

    #[test]
    fn proactive_never_suggests_destructive() {
        // A destructive sequence at the threshold must NOT
        // trigger a proactive toast (user can still save via
        // the sessions page's explicit affordance).
        let mut entries = Vec::new();
        let mut ts = 1000u64;
        for _ in 0..3 {
            entries.push(entry("rm -rf build", ts, Some(1))); ts += 100;
            entries.push(entry("make install", ts, Some(1))); ts += 100;
        }
        // Note: at 3 occurrences the destructive seq is also
        // below destructive_min_occurrences(4), so it wouldn't
        // be a candidate at all — but this test pins the intent
        // regardless of that interaction.
        assert!(proactive_suggestion(&entries, 3).is_none(),
            "destructive sequence shouldn't be proactively suggested");
    }

    #[test]
    fn proactive_never_suggests_trivial() {
        // ls/cd seen 3 times → all-trivial penalty drives score
        // negative → no suggestion.
        let mut entries = Vec::new();
        let mut ts = 1000u64;
        for _ in 0..3 {
            entries.push(entry("ls", ts, Some(1))); ts += 100;
            entries.push(entry("cd ..", ts, Some(1))); ts += 100;
        }
        assert!(proactive_suggestion(&entries, 3).is_none(),
            "trivial muscle-memory sequence shouldn't trigger");
    }
}
