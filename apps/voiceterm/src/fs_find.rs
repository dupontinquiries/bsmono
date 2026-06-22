//! v0.1.361: filesystem find mode.
//!
//! Powers the find bar's "files" mode, which runs ripgrep (or
//! a `grep -rIn` fallback) over the terminal's current working
//! directory and surfaces line-level matches.
//!
//! Design notes:
//!
//! - Runs in a worker thread spawned from `start_search`. The
//!   foreground (UI) thread polls via `poll_results` rather than
//!   blocking on a join. A single `IN_FLIGHT` atomic prevents
//!   queueing — a new search aborts any pending one by dropping
//!   its output (we don't try to kill the subprocess; ripgrep is
//!   fast enough that even an abandoned run finishes within a
//!   couple of seconds).
//!
//! - Result cap: 100 total matches across all files. Past that
//!   we stop reading the subprocess output. Ripgrep gets
//!   `--max-columns 200` so absurdly long lines don't bloat the
//!   payload.
//!
//! - File-line-match wire format: each result is
//!   `path\x1Fline_num\x1Fmatched_text` joined by `\n`. Empty
//!   result string means "no matches yet" OR "no matches found"
//!   — the caller distinguishes by checking `is_running()`.
//!
//! - Cancellation: setting CANCEL_FLAG = true makes the worker
//!   stop reading output and clear the result slot. The
//!   subprocess may still be running; we let it die on its own.
//!
//! - Backend choice: at module-init we probe for `rg`. If found,
//!   `BACKEND` holds "rg"; otherwise "grep". Tests aren't gated
//!   on a particular backend — just on the parsing/result-cap
//!   logic.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::io::{BufRead, BufReader};

static IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static CANCEL_FLAG: AtomicBool = AtomicBool::new(false);

/// Hard cap on results returned to the UI. Past this we stop
/// reading. 100 is generous — most useful queries match <20.
const RESULT_CAP: usize = 100;

/// Max line length we'll surface. Lines longer than this get
/// truncated to keep the wire payload sane.
const MAX_LINE_LEN: usize = 200;

/// One filesystem match. The wire format is
/// `path\x1Fline\x1Fmatched_text` per row.
#[derive(Clone, Debug)]
pub struct FsMatch {
    pub path: String,
    pub line: u32,
    pub matched: String,
}

/// Result slot read by `poll_results`. Replaced atomically when
/// the worker finishes. `None` = no search has completed since
/// the last `start_search`. `Some(vec)` = matches available
/// (may be empty if the search found nothing).
fn result_slot() -> &'static Mutex<Option<Vec<FsMatch>>> {
    static SLOT: std::sync::OnceLock<Mutex<Option<Vec<FsMatch>>>> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Cached probe result: true if `rg` is on PATH.
fn has_rg() -> bool {
    static FLAG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FLAG.get_or_init(|| {
        Command::new("rg")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// True while a search is being run by the worker thread.
pub fn is_running() -> bool {
    IN_FLIGHT.load(Ordering::Acquire)
}

/// Start a new search. Returns true if a search was scheduled,
/// false if one was already in flight (caller should cancel
/// first if they want to replace it).
///
/// `query` must not be empty; an empty query returns false
/// without starting work.
///
/// The worker writes its result into `result_slot` when done.
/// Caller polls via `poll_results`.
pub fn start_search(query: String, cwd: PathBuf) -> bool {
    if query.trim().is_empty() {
        return false;
    }
    // Quick claim. If a search is already running, refuse.
    if IN_FLIGHT.swap(true, Ordering::AcqRel) {
        return false;
    }
    // Reset the cancel flag and clear the slot atomically before
    // spawning so old results don't leak into the new search.
    CANCEL_FLAG.store(false, Ordering::Release);
    if let Ok(mut g) = result_slot().lock() {
        *g = None;
    }

    std::thread::Builder::new()
        .name("fs-find".to_string())
        .spawn(move || {
            let matches = run_search(&query, &cwd);
            // If cancelled mid-flight, drop the result.
            if !CANCEL_FLAG.load(Ordering::Acquire) {
                if let Ok(mut g) = result_slot().lock() {
                    *g = Some(matches);
                }
            }
            IN_FLIGHT.store(false, Ordering::Release);
        })
        .ok();
    true
}

/// Abort any in-flight search. The subprocess may continue
/// running but its output will be discarded. Safe to call when
/// nothing is running (no-op).
pub fn cancel() {
    CANCEL_FLAG.store(true, Ordering::Release);
}

/// Drain the result slot. Returns Some(...) exactly once after
/// each completed search. After the take, subsequent polls
/// return None until the next search completes.
pub fn poll_results() -> Option<Vec<FsMatch>> {
    if let Ok(mut g) = result_slot().lock() {
        g.take()
    } else {
        None
    }
}

/// v0.1.373: parse an optional glob filter from the query
/// string. Supports two forms:
///
///   "*.rs:fn main"   → glob="*.rs", query="fn main"
///   "glob:src/**:fn" → glob="src/**", query="fn"
///   "fn main"        → glob=None,   query="fn main"
///
/// Returns (Option<glob>, query). The glob form takes effect
/// only if `query` is non-empty after split — `"*.rs:"` is
/// treated as a normal query because there's nothing to
/// search FOR.
///
/// The shorthand `*.ext:` form only matches when the part
/// before `:` looks unambiguously like a glob (contains `*`,
/// `?`, or `**`). Plain `foo:bar` stays a literal query so
/// users searching for things like URLs or namespace
/// references don't accidentally trip the glob mode.
pub fn parse_glob_query(input: &str) -> (Option<String>, String) {
    let trimmed = input.trim();
    if trimmed.is_empty() { return (None, String::new()); }

    // Explicit form: "glob:PATTERN:QUERY"
    if let Some(rest) = trimmed.strip_prefix("glob:") {
        if let Some(idx) = rest.find(':') {
            let g = rest[..idx].trim();
            let q = rest[idx+1..].trim();
            if !g.is_empty() && !q.is_empty() {
                return (Some(g.to_string()), q.to_string());
            }
        }
        // Fall through to literal if format incomplete.
    }

    // Shorthand: "PATTERN:QUERY" where PATTERN contains glob chars.
    if let Some(idx) = trimmed.find(':') {
        let candidate_glob = &trimmed[..idx];
        let candidate_query = trimmed[idx+1..].trim();
        let looks_glob = candidate_glob.contains('*')
            || candidate_glob.contains('?')
            || candidate_glob.contains('[');
        if looks_glob && !candidate_query.is_empty() {
            return (Some(candidate_glob.to_string()), candidate_query.to_string());
        }
    }

    (None, trimmed.to_string())
}


/// Run the actual search subprocess and parse output into
/// FsMatch entries. Respects the result cap and the cancel flag.
///
/// v0.1.373: also handles glob filter from `parse_glob_query`.
/// When a glob is present, restricts rg's file scan via
/// `--glob PATTERN` (or grep's `--include=PATTERN`). Empty
/// glob is treated as None.
fn run_search(query: &str, cwd: &PathBuf) -> Vec<FsMatch> {
    let (glob_opt, real_query) = parse_glob_query(query);
    let query = real_query.as_str();
    // Bail early if the parsed query is empty (e.g. user typed
    // a bare glob with no search term).
    if query.is_empty() {
        return Vec::new();
    }
    let mut cmd = if has_rg() {
        let mut c = Command::new("rg");
        if let Some(g) = &glob_opt {
            c.arg("--glob").arg(g);
        }
        c.arg("--max-columns").arg(MAX_LINE_LEN.to_string())
            .arg("--line-number")
            .arg("--no-heading")
            .arg("--color=never")
            .arg("--max-count").arg("3")   // per-file cap
            .arg("--")
            .arg(query)
            .arg(".");
        c
    } else {
        // grep fallback. -I skips binary files, -n shows line
        // numbers, -r recurses, -F fixed-string (avoids regex
        // surprises in user queries).
        //
        // v0.1.373: --include=PATTERN narrows by filename when
        // a glob filter is present. GNU grep --include accepts
        // glob syntax that overlaps with rg's (`*.rs`, `src/*`,
        // brackets) but NOT `**` recursion — for that case we
        // fall back to no filter and let the `-r` recurse cover
        // it (the glob still constrains rg's path; grep just
        // does less aggressive filename narrowing).
        let mut c = Command::new("grep");
        if let Some(g) = &glob_opt {
            // grep --include treats `**` as literal — strip it
            // if present at start so `**/*.rs` becomes `*.rs`
            // (which is the practical equivalent for `-r`).
            let g_eff = g.trim_start_matches("**/");
            if !g_eff.is_empty() {
                c.arg(format!("--include={}", g_eff));
            }
        }
        c.arg("-rIn")
            .arg("--color=never")
            .arg("-F")
            .arg(query)
            .arg(".");
        c
    };

    cmd.current_dir(cwd);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            crate::clog!("fs-find", "spawn failed: {}", e);
            return Vec::new();
        }
    };

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => return Vec::new(),
    };

    let mut results = Vec::with_capacity(32);
    let reader = BufReader::new(stdout);
    for line_result in reader.lines() {
        if CANCEL_FLAG.load(Ordering::Acquire) { break; }
        if results.len() >= RESULT_CAP { break; }
        let line = match line_result {
            Ok(l) => l,
            Err(_) => break,
        };
        if let Some(m) = parse_match_line(&line) {
            results.push(m);
        }
    }

    // We deliberately don't wait()/kill() — the subprocess will
    // close on its own when its stdout pipe is dropped. Calling
    // wait() here would block until done; we want to return as
    // soon as we've hit the cap or read all available output.
    let _ = child.try_wait();

    results
}

/// Parse a single output line from rg/grep into an FsMatch.
///
/// Format is `path:line:content`. Tricky cases:
///   - Path may contain `:` (rare but legal on Linux). We split
///     on the FIRST `:` then split the remainder on the FIRST
///     `:` again — line number is always purely digits, so if
///     the second segment doesn't parse, fall back to "no match
///     parsed."
///   - Empty content is OK (matched line was empty after the
///     line-number prefix).
///
/// Returns None for lines that don't fit (binary-file warnings
/// from grep, "(standard input)" stubs, etc.).
pub fn parse_match_line(line: &str) -> Option<FsMatch> {
    let first_colon = line.find(':')?;
    let (path_raw, rest) = line.split_at(first_colon);
    let rest = &rest[1..]; // skip the ':'
    let second_colon = rest.find(':')?;
    let (line_str, content) = rest.split_at(second_colon);
    let content = &content[1..];
    let line_num: u32 = line_str.parse().ok()?;
    // Normalize the path: rg/grep prepend `./` because we asked
    // them to search `.`. Strip it for friendlier display.
    let path = path_raw.strip_prefix("./").unwrap_or(path_raw).to_string();
    // Truncate matched content to MAX_LINE_LEN so the wire
    // payload stays bounded.
    let matched = if content.len() > MAX_LINE_LEN {
        let mut s = content[..MAX_LINE_LEN].to_string();
        s.push_str("…");
        s
    } else {
        content.to_string()
    };
    Some(FsMatch { path, line: line_num, matched })
}

/// Render a Vec<FsMatch> into the wire format the QML side
/// expects: `path\x1Fline\x1Fmatched\n` per row.
pub fn render_wire(matches: &[FsMatch]) -> String {
    let mut out = String::with_capacity(matches.len() * 64);
    for (i, m) in matches.iter().enumerate() {
        if i > 0 { out.push('\n'); }
        out.push_str(&m.path);
        out.push('\x1F');
        out.push_str(&m.line.to_string());
        out.push('\x1F');
        out.push_str(&m.matched);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_match_line() {
        let m = parse_match_line("src/main.rs:42:    let foo = bar;").unwrap();
        assert_eq!(m.path, "src/main.rs");
        assert_eq!(m.line, 42);
        assert_eq!(m.matched, "    let foo = bar;");
    }

    #[test]
    fn parse_strips_dot_slash_prefix() {
        let m = parse_match_line("./src/main.rs:1:hello").unwrap();
        assert_eq!(m.path, "src/main.rs");
    }

    #[test]
    fn parse_rejects_non_numeric_line() {
        // grep emits "Binary file foo matches" style lines we
        // don't want to surface.
        assert!(parse_match_line("Binary file foo matches").is_none());
        assert!(parse_match_line("src/main.rs:NOTANUM:hello").is_none());
    }

    #[test]
    fn parse_handles_empty_content() {
        let m = parse_match_line("foo.txt:5:").unwrap();
        assert_eq!(m.matched, "");
    }

    #[test]
    fn parse_truncates_long_content() {
        let long = "x".repeat(MAX_LINE_LEN + 50);
        let line = format!("foo.txt:1:{}", long);
        let m = parse_match_line(&line).unwrap();
        // Truncated to MAX_LINE_LEN chars + ellipsis
        assert!(m.matched.ends_with("…"));
        assert_eq!(m.matched.chars().filter(|c| *c == 'x').count(), MAX_LINE_LEN);
    }

    #[test]
    fn parse_handles_colon_in_content() {
        // Match content can contain colons; we split on first
        // two only, rest is content.
        let m = parse_match_line("a.rs:7:fn foo(x: i32) -> bool {").unwrap();
        assert_eq!(m.path, "a.rs");
        assert_eq!(m.line, 7);
        assert_eq!(m.matched, "fn foo(x: i32) -> bool {");
    }

    #[test]
    fn render_wire_joins_with_unit_separator() {
        let matches = vec![
            FsMatch { path: "a.rs".into(), line: 1, matched: "foo".into() },
            FsMatch { path: "b.rs".into(), line: 2, matched: "bar".into() },
        ];
        let wire = render_wire(&matches);
        assert_eq!(wire, "a.rs\x1F1\x1Ffoo\nb.rs\x1F2\x1Fbar");
    }

    #[test]
    fn render_wire_empty() {
        assert_eq!(render_wire(&[]), "");
    }

    // v0.1.373: glob filter parser tests.

    #[test]
    fn parse_glob_literal_query_no_split() {
        // Plain query, no glob chars, stays literal.
        let (g, q) = parse_glob_query("fn main");
        assert_eq!(g, None);
        assert_eq!(q, "fn main");
    }

    #[test]
    fn parse_glob_colon_in_literal_query_preserved() {
        // Colons inside a query without glob chars stay
        // literal — users searching for URLs / namespaces
        // shouldn't trip the parser.
        let (g, q) = parse_glob_query("http://example.com");
        assert_eq!(g, None);
        assert_eq!(q, "http://example.com");
        let (g2, q2) = parse_glob_query("foo::bar");
        assert_eq!(g2, None);
        assert_eq!(q2, "foo::bar");
    }

    #[test]
    fn parse_glob_shorthand_star_ext() {
        // `*.rs:fn` extracts glob=*.rs, query="fn"
        let (g, q) = parse_glob_query("*.rs:fn main");
        assert_eq!(g, Some("*.rs".to_string()));
        assert_eq!(q, "fn main");
    }

    #[test]
    fn parse_glob_shorthand_path_pattern() {
        // src/*.rs:foo
        let (g, q) = parse_glob_query("src/*.rs:foo");
        assert_eq!(g, Some("src/*.rs".to_string()));
        assert_eq!(q, "foo");
    }

    #[test]
    fn parse_glob_shorthand_with_double_star() {
        let (g, q) = parse_glob_query("**/*.toml:version");
        assert_eq!(g, Some("**/*.toml".to_string()));
        assert_eq!(q, "version");
    }

    #[test]
    fn parse_glob_shorthand_with_bracket_class() {
        // [abc]*.rs uses a character class — counts as glob
        let (g, q) = parse_glob_query("[mt]*.rs:async");
        assert_eq!(g, Some("[mt]*.rs".to_string()));
        assert_eq!(q, "async");
    }

    #[test]
    fn parse_glob_explicit_form() {
        // glob:PATTERN:QUERY explicit form
        let (g, q) = parse_glob_query("glob:src/**:fn");
        assert_eq!(g, Some("src/**".to_string()));
        assert_eq!(q, "fn");
    }

    #[test]
    fn parse_glob_explicit_form_with_no_glob_chars() {
        // Explicit form bypasses the "looks like a glob" check
        // — user knows what they're doing.
        let (g, q) = parse_glob_query("glob:Cargo.toml:version");
        assert_eq!(g, Some("Cargo.toml".to_string()));
        assert_eq!(q, "version");
    }

    #[test]
    fn parse_glob_empty_query_after_colon_falls_back_to_literal() {
        // "*.rs:" has no query — treat as literal so the
        // user sees the issue.
        let (g, q) = parse_glob_query("*.rs:");
        assert_eq!(g, None);
        assert_eq!(q, "*.rs:");
    }

    #[test]
    fn parse_glob_handles_whitespace_around_separators() {
        // Trim around the components, but don't lose intent.
        let (g, q) = parse_glob_query("  *.rs:  fn main  ");
        assert_eq!(g, Some("*.rs".to_string()));
        assert_eq!(q, "fn main");
    }

    #[test]
    fn parse_glob_empty_input() {
        let (g, q) = parse_glob_query("");
        assert_eq!(g, None);
        assert_eq!(q, "");
        let (g2, q2) = parse_glob_query("   ");
        assert_eq!(g2, None);
        assert_eq!(q2, "");
    }
}
