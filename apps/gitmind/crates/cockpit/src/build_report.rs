//! build_report — turn a raw build/test log (as the Fleet page's `Task` already
//! captures it) into the three things a "hold to see more options" matrix
//! button on that page offers beyond "copy the whole log": errors only, time
//! by crate/app, and a portable file with both plus the full log.
//!
//! Pure and std-only, like the rest of this crate — no egui dependency, so
//! it's testable headless and reusable from any caller that has a
//! `Vec<TimedLine>` (currently `gui-egui::task::Task::timed_log`).

use std::path::{Path, PathBuf};

/// One captured log line, with its elapsed time since the run started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimedLine {
    pub ms: u64,
    pub text: String,
}

/// What a report is *of* — the summary line every export/markdown starts with.
pub struct ReportMeta {
    pub title: String,
    pub ok: usize,
    pub failed: Vec<String>,
}

/// Line-level error extraction: any line starting with `error` after
/// trimming (rustc/cargo's own convention — `error[E0308]:`, `error:`,
/// `error: could not compile …`). Deliberately NOT a full multi-line
/// diagnostic block extractor (that needs `--message-format=json`, which
/// `cargo build`/`cargo test`'s plain streamed output here doesn't give) —
/// this is "grep for error lines," honest about being a heuristic rather
/// than pretending to be `cargo.rs`'s structured `Diag` parsing.
pub fn errors_only(lines: &[TimedLine]) -> String {
    lines
        .iter()
        .filter(|l| l.text.trim_start().starts_with("error"))
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Per-section/per-crate timing. Two heuristics, tried in order:
///
/// 1. `── name ──` boundary lines — the exact format
///    `cockpit::flow::build_all_logged`/`test_all_logged` already emit once
///    per app before building/testing it. If present, these are authoritative
///    (they ARE the app boundaries), so they win outright.
/// 2. `Compiling <crate> …` / `Checking <crate> …` cargo lines — what a
///    single-app `cargo build`/`cargo test` prints per dependency crate, used
///    when there are no `── ── ` markers (a single-app run, not build-all).
///
/// A section's duration is the time to the next section's start (or to the
/// last captured line, for the final section) — an approximation, since
/// cargo's plain output has no per-crate END marker without `--timings`, but
/// a reasonable one: it's exactly the wall-clock gap the user watched.
pub fn times_by_crate(lines: &[TimedLine]) -> Vec<(String, u64)> {
    let boundaries: Vec<(String, u64)> =
        lines.iter().filter_map(|l| section_name(&l.text).map(|n| (n, l.ms))).collect();
    let starts = if !boundaries.is_empty() {
        boundaries
    } else {
        lines.iter().filter_map(|l| compiling_crate_name(&l.text).map(|n| (n, l.ms))).collect()
    };
    let last_ms = lines.last().map(|l| l.ms).unwrap_or(0);
    starts
        .iter()
        .enumerate()
        .map(|(i, (name, start))| {
            let end = starts.get(i + 1).map(|(_, ms)| *ms).unwrap_or(last_ms);
            (name.clone(), end.saturating_sub(*start))
        })
        .collect()
}

fn section_name(line: &str) -> Option<String> {
    let t = line.trim();
    let t = t.strip_prefix("── ")?;
    let t = t.strip_suffix(" ──")?;
    Some(t.to_string())
}

fn compiling_crate_name(line: &str) -> Option<String> {
    let t = line.trim_start();
    let rest = t.strip_prefix("Compiling ").or_else(|| t.strip_prefix("Checking "))?;
    Some(rest.split_whitespace().next()?.to_string())
}

/// The portable-file format: a summary, a time-by-crate table, an errors
/// section (only if non-empty), then the full log — everything the matrix
/// button's other options can copy individually, plus context, in one file
/// meant to be attached to a chat or bug report.
pub fn to_markdown(meta: &ReportMeta, lines: &[TimedLine]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# gitmind report: {}\n\n", meta.title));
    out.push_str(&format!("- ok: {}\n", meta.ok));
    if meta.failed.is_empty() {
        out.push_str("- failed: none\n\n");
    } else {
        out.push_str(&format!("- failed ({}): {}\n\n", meta.failed.len(), meta.failed.join(", ")));
    }

    let times = times_by_crate(lines);
    if !times.is_empty() {
        out.push_str("## time by crate/app\n\n");
        for (name, ms) in &times {
            out.push_str(&format!("- {name}: {:.1}s\n", *ms as f64 / 1000.0));
        }
        out.push('\n');
    }

    let errs = errors_only(lines);
    if !errs.is_empty() {
        out.push_str("## errors\n\n```\n");
        out.push_str(&errs);
        out.push_str("\n```\n\n");
    }

    out.push_str("## full log\n\n```\n");
    for l in lines {
        out.push_str(&l.text);
        out.push('\n');
    }
    out.push_str("```\n");
    out
}

/// Write the markdown report under `dir` (created if missing), named
/// `gitmind-<slug-of-title>-<unix-seconds>.md`, and return the path — the
/// "portable file" the matrix button's export option hands back so the
/// caller can show/copy the path (e.g. to paste into a chat).
pub fn export(dir: &Path, meta: &ReportMeta, lines: &[TimedLine]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let slug: String = meta
        .title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let path = dir.join(format!("gitmind-{slug}-{ts}.md"));
    std::fs::write(&path, to_markdown(meta, lines))?;
    Ok(path)
}

/// Default export directory: `$HOME/gitmind-reports` — same `$HOME`
/// convention `cockpit::deploy` already uses elsewhere in this crate, and a
/// flat, discoverable location rather than burying reports in an XDG cache
/// dir nobody thinks to look in when they want to grab a file to share.
pub fn default_export_dir() -> PathBuf {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/tmp"));
    home.join("gitmind-reports")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ms: u64, text: &str) -> TimedLine {
        TimedLine { ms, text: text.to_string() }
    }

    #[test]
    fn errors_only_keeps_error_lines_and_drops_everything_else() {
        let lines = vec![
            line(0, "   Compiling foo v0.1.0"),
            line(10, "error[E0308]: mismatched types"),
            line(20, "  --> src/lib.rs:3:5"),
            line(30, "warning: unused variable"),
            line(40, "error: aborting due to 1 previous error"),
        ];
        let out = errors_only(&lines);
        assert!(out.contains("error[E0308]: mismatched types"));
        assert!(out.contains("error: aborting due to 1 previous error"));
        assert!(!out.contains("Compiling"));
        assert!(!out.contains("warning:"));
        // arrow continuation line isn't itself an "error"-prefixed line —
        // documented as a line-level heuristic, not a block extractor.
        assert!(!out.contains("-->"));
    }

    #[test]
    fn errors_only_is_empty_for_a_clean_run() {
        let lines = vec![line(0, "   Compiling foo v0.1.0"), line(10, "    Finished dev profile")];
        assert_eq!(errors_only(&lines), "");
    }

    #[test]
    fn times_by_crate_uses_section_boundaries_when_present() {
        let lines = vec![
            line(0, "── voiceterm ──"),
            line(100, "  Compiling stuff"),
            line(500, "── gitmind ──"),
            line(600, "  Compiling other stuff"),
            line(900, "✓ gitmind ok"),
        ];
        let times = times_by_crate(&lines);
        assert_eq!(times, vec![("voiceterm".to_string(), 500), ("gitmind".to_string(), 400)]);
    }

    #[test]
    fn times_by_crate_falls_back_to_compiling_lines_without_boundaries() {
        let lines = vec![
            line(0, "   Compiling core v0.1.0 (/a)"),
            line(300, "   Compiling utils v0.2.0 (/b)"),
            line(700, "    Finished dev profile"),
        ];
        let times = times_by_crate(&lines);
        assert_eq!(times, vec![("core".to_string(), 300), ("utils".to_string(), 400)]);
    }

    #[test]
    fn times_by_crate_is_empty_when_neither_pattern_is_present() {
        let lines = vec![line(0, "hello"), line(10, "world")];
        assert!(times_by_crate(&lines).is_empty());
    }

    #[test]
    fn to_markdown_includes_summary_times_errors_and_full_log() {
        let meta = ReportMeta { title: "build all".to_string(), ok: 5, failed: vec!["bad-app".to_string()] };
        let lines = vec![
            line(0, "── ok-app ──"),
            line(100, "── bad-app ──"),
            line(150, "error: could not compile `bad-app`"),
        ];
        let md = to_markdown(&meta, &lines);
        assert!(md.contains("# gitmind report: build all"));
        assert!(md.contains("- ok: 5"));
        assert!(md.contains("failed (1): bad-app"));
        assert!(md.contains("## time by crate/app"));
        assert!(md.contains("ok-app: 0.1s"));
        assert!(md.contains("## errors"));
        assert!(md.contains("error: could not compile"));
        assert!(md.contains("## full log"));
        assert!(md.contains("── bad-app ──"));
    }

    #[test]
    fn export_writes_a_real_readable_file_and_returns_its_path() {
        let dir = std::env::temp_dir().join(format!("gitmind-build-report-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let meta = ReportMeta { title: "test all".to_string(), ok: 2, failed: vec![] };
        let lines = vec![line(0, "── a ──"), line(50, "✓ a ok")];
        let path = export(&dir, &meta, &lines).expect("export should succeed");
        assert!(path.exists());
        assert!(path.file_name().unwrap().to_string_lossy().starts_with("gitmind-test-all-"));
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("# gitmind report: test all"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
