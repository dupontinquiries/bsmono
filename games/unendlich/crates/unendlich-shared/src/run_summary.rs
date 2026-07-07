//! Persistent per-run summary log file (v0.24.2c.2, extended in c.3).
//!
//! Every run of `unendlich-world` (or any binary that calls this)
//! appends a single block of summary information to a project-root
//! file, `run_summaries.txt`. Blocks are framed with `==== RUN START
//! YYYY-MM-DDTHH:MM:SSZ ====` and `==== RUN END YYYY-MM-DDTHH:MM:SSZ
//! ====` lines so a human or grep can scan/cut blocks easily.
//!
//! v0.24.2c.3 adds **version-transition markers**: when a run with
//! a different version than the previous run is appended, an END
//! marker is written for the previous version and a START marker
//! is written for the new version. The very first run in a file
//! gets a START marker but no END (there's no previous version to
//! close). Version-transition markers wrap each version's run
//! blocks visually:
//!
//! ```text
//! ======================================================================
//! ==== VERSION 0.24.2-c.2 START 2026-05-11T13:15:12Z ====
//! ======================================================================
//!
//! ==== RUN START ... ====
//!   ... run 1 ...
//! ==== RUN END ... ====
//!
//! ==== RUN START ... ====
//!   ... run 2 ...
//! ==== RUN END ... ====
//!
//! ======================================================================
//! ==== VERSION 0.24.2-c.2 END 2026-05-11T14:00:00Z ====
//! ======================================================================
//! ======================================================================
//! ==== VERSION 0.24.2-c.3 START 2026-05-11T14:00:00Z ====
//! ======================================================================
//!
//! ==== RUN START ... ====
//!   ... run 3 (under new version) ...
//! ==== RUN END ... ====
//! ```
//!
//! ## Why a separate file (not stderr/stdout)
//!
//! Stderr logs are voluminous (per-second FPS lines, debug warnings,
//! per-frame breakdowns). The lifetime summary is the *aggregate*
//! signal — the thing you want to A/B across runs. Putting it in a
//! file means:
//! - Trivial cross-run diff (`diff <(awk start..end run N) <(awk N+1)`).
//! - Survives terminal scrollback loss.
//! - Doesn't get truncated when piping to other tools.
//!
//! ## Atomic appends
//!
//! Each call to `append_block` opens the file in append mode and does
//! one `write_all` of the entire block. POSIX `O_APPEND` is atomic
//! for writes ≤ `PIPE_BUF` (typically 4 KB); our blocks usually
//! exceed that, so concurrent runs *could* interleave. The framing
//! lines make manual recovery easy if it ever happens, but in
//! practice unendlich-world is single-instance so the risk is low.
//!
//! The version-marker logic also reads the file before writing,
//! which adds another non-atomic window. Two concurrent appends
//! with the same version are fine (no markers needed). Two
//! concurrent appends with DIFFERENT versions could both observe
//! "previous version is X" and both write transition markers,
//! resulting in duplicates. Acceptable; the file remains
//! human-readable.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Default file path, relative to the cwd of the binary. For
/// `cargo run` workflows this resolves to the project root, which
/// matches where DESIGN_NOTES.md and other dev artifacts live.
pub const DEFAULT_PATH: &str = "run_summaries.txt";

/// One run-summary block to append. Fill in fields from the
/// shutdown path and call `append`.
pub struct RunBlock<'a> {
    /// Build version, typically `env!("CARGO_PKG_VERSION")`.
    pub version: &'a str,
    /// v0.24.2c.8: run UUID from `util::run_id::fresh()`. Lets a
    /// reader cross-reference this block with the matching `events.jsonl`
    /// records and the UUID-prefixed terminal log.
    pub uuid: &'a str,
    /// Run duration in seconds.
    pub duration_secs: f32,
    /// Binary name (e.g. "unendlich-world") and arguments. Used to
    /// reconstruct the invocation if needed.
    pub cmdline: &'a str,
    /// Env-var flags-in-effect block (already-formatted text).
    /// Typically `runtime_flags::flags_summary("...")`.
    pub env_flags: &'a str,
    /// The body — concatenated GPU/CPU/mesher summaries plus any
    /// extra free-form sections. Lines should NOT include the
    /// `==== RUN START/END` markers — those are added here.
    pub body: &'a str,
}

impl<'a> RunBlock<'a> {
    /// Format the block to a String. Useful for unit testing
    /// without touching the filesystem.
    pub fn format(&self) -> String {
        let start_ts = iso8601_utc_now();
        let mut out = String::with_capacity(4096);
        out.push_str(&format!("==== RUN START {} ====\n", start_ts));
        out.push_str(&format!("version: {}\n", self.version));
        out.push_str(&format!("uuid: {}\n", self.uuid));
        out.push_str(&format!("duration: {:.1}s\n", self.duration_secs));
        out.push_str(&format!("cmdline: {}\n", self.cmdline));
        out.push_str("env:\n");
        for line in self.env_flags.lines() {
            // Indent each line of the env flags block by 2 spaces
            // so it nests visually under "env:".
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("---- summaries ----\n");
        out.push_str(self.body);
        if !self.body.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("==== RUN END {} ====\n\n", start_ts));
        out
    }

    /// Append this block to `path`. v0.24.2c.3: if the file's
    /// most-recent version marker (if any) differs from
    /// `self.version`, emit the version-transition markers
    /// (`==== VERSION old END ====` then `==== VERSION new START ====`)
    /// before the run block. On a fresh file, emit only the START
    /// marker.
    ///
    /// Returns an io::Error if the open/read/write fails; caller
    /// decides whether to log and continue or propagate.
    ///
    /// Race note: detecting the previous version requires reading
    /// the file before writing, which is not atomic with the
    /// append. If two `unendlich-world` processes run concurrently
    /// they could both observe "no version marker" and both write
    /// a START. Acceptable for a single-instance dev tool; the
    /// run-block framing makes manual cleanup easy if it ever
    /// happens.
    pub fn append<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let path_ref = path.as_ref();

        // Read the full file once so we can detect:
        // (a) the most recent VERSION marker (the canonical "current
        //     version" of the file's tail), and
        // (b) for pre-marker files (written by binaries before
        //     v0.24.2c.3), the most recent `version:` line from a
        //     RUN block — used to emit a retroactive END so the
        //     transition is visible in the file.
        let existing = match std::fs::read_to_string(path_ref) {
            Ok(s) => Some(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };

        let prev_marker = existing.as_deref()
            .and_then(parse_last_version_marker);
        let prev_inblock = existing.as_deref()
            .and_then(parse_last_inblock_version);

        let mut prelude = String::new();
        let ts = iso8601_utc_now();
        match (prev_marker.as_deref(), prev_inblock.as_deref()) {
            (Some(prev), _) if prev == self.version => {
                // Same marker as current version — just continue.
            }
            (Some(prev), _) => {
                // Marker present, different version — normal transition.
                prelude.push_str(&format_version_end_marker(prev, &ts));
                prelude.push_str(&format_version_start_marker(self.version, &ts));
            }
            (None, Some(inblock)) if inblock == self.version => {
                // No markers but prior runs of the SAME version exist
                // (pre-c.3 file, now upgraded to a c.3-aware binary
                // at the same version — e.g. local build). Just emit
                // a START so subsequent transitions render correctly.
                prelude.push_str(&format_version_start_marker(self.version, &ts));
            }
            (None, Some(inblock)) => {
                // No markers but prior runs of a DIFFERENT version
                // exist. Emit a RETROACTIVE end for the observed prior
                // version, then a START for the current version.
                // Marked "(retroactive)" so a reader knows the END
                // timestamp is the start-of-new-run, not the actual
                // close-of-prior-run.
                prelude.push_str(&format_version_end_marker_retroactive(
                    inblock, &ts));
                prelude.push_str(&format_version_start_marker(self.version, &ts));
            }
            (None, None) => {
                // Empty/missing file with no prior content.
                prelude.push_str(&format_version_start_marker(self.version, &ts));
            }
        }

        let body = self.format();
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path_ref)?;
        if !prelude.is_empty() {
            f.write_all(prelude.as_bytes())?;
        }
        f.write_all(body.as_bytes())?;
        // Flush before drop so a subsequent crash doesn't drop the
        // most recent block.
        f.flush()?;
        Ok(())
    }
}

/// Format a `==== VERSION X START ts ====` block, with a rule line
/// above and below for visibility in `less`.
fn format_version_start_marker(version: &str, ts: &str) -> String {
    let rule = "=".repeat(70);
    format!(
        "{rule}\n==== VERSION {version} START {ts} ====\n{rule}\n\n",
        rule = rule, version = version, ts = ts,
    )
}

/// Format a `==== VERSION X END ts ====` block. Mirrors START.
fn format_version_end_marker(version: &str, ts: &str) -> String {
    let rule = "=".repeat(70);
    format!(
        "{rule}\n==== VERSION {version} END {ts} ====\n{rule}\n\n",
        rule = rule, version = version, ts = ts,
    )
}

/// Like `format_version_end_marker` but marks the END as
/// "(retroactive)" so a reader knows the timestamp isn't the actual
/// close-of-prior-run — it's the start-of-this-new-run that triggered
/// the markers. Used when upgrading a pre-marker file (no
/// `==== VERSION X START ====` lines anywhere) at a different
/// version than the file's most recent `version:` line.
fn format_version_end_marker_retroactive(version: &str, ts: &str) -> String {
    let rule = "=".repeat(70);
    format!(
        "{rule}\n==== VERSION {version} END (retroactive) {ts} ====\n{rule}\n\n",
        rule = rule, version = version, ts = ts,
    )
}

/// Read `path` and return the most recent version that appears in a
/// `==== VERSION X START ts ====` line. Returns `Ok(None)` if the
/// file is missing or contains no version markers, `Err` only on
/// real I/O failures.
///
/// We scan the WHOLE file (typical run-summaries.txt is small —
/// kilobytes per block, tens of blocks = ~1 MB max for casual use).
/// For very large files an optimisation would be to seek-from-end
/// in chunks, but YAGNI until it bites.
pub fn find_last_version_marker(path: &Path) -> std::io::Result<Option<String>> {
    use std::io::ErrorKind;
    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(parse_last_version_marker(&contents))
}

/// Pure-function half of `find_last_version_marker`. Given the file
/// contents, return the version of the most recent
/// `==== VERSION X START ts ====` line, or None if no such line.
///
/// Robust to malformed markers (skips them) and to lines that
/// happen to begin with `==== VERSION ` but aren't START markers.
fn parse_last_version_marker(contents: &str) -> Option<String> {
    let mut last: Option<String> = None;
    for line in contents.lines() {
        let line = line.trim();
        // Match `==== VERSION X START ts ====`.
        if let Some(rest) = line.strip_prefix("==== VERSION ") {
            // rest = "X START ts ===="  OR  "X END ts ===="
            // We look for " START " specifically.
            if let Some(start_pos) = rest.find(" START ") {
                let version = &rest[..start_pos];
                if !version.is_empty() {
                    last = Some(version.to_string());
                }
            }
            // END markers don't update `last` — they document the
            // close of a version's blocks, not which version is
            // "active". The most recent START is what we compare to.
        }
    }
    last
}

/// Parse the most recent `version: X` line from the file body
/// (lines inside RUN blocks). Used for retroactive version-transition
/// detection when the file has prior runs but no `VERSION X START`
/// markers (i.e. it was written by a pre-v0.24.2c.3 binary).
///
/// Returns `None` if no `version:` line is found anywhere.
fn parse_last_inblock_version(contents: &str) -> Option<String> {
    let mut last: Option<String> = None;
    for line in contents.lines() {
        // Match `version: X.Y.Z` at start of line (after trim).
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("version: ") {
            let v = rest.trim();
            if !v.is_empty() {
                last = Some(v.to_string());
            }
        }
    }
    last
}

/// Format `SystemTime::now()` as ISO 8601 UTC: `YYYY-MM-DDTHH:MM:SSZ`.
/// Self-contained so we don't need a chrono/time dep just for this.
///
/// Algorithm: take seconds since UNIX epoch, decompose into
/// year/month/day/hour/minute/second using days-since-epoch math.
/// Matches the format used by `env_logger`'s default decorator.
pub fn iso8601_utc_now() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    iso8601_utc_from_secs(now.as_secs())
}

/// Format a unix timestamp (seconds since 1970-01-01 UTC) as
/// `YYYY-MM-DDTHH:MM:SSZ`. Pure function — unit-testable.
pub fn iso8601_utc_from_secs(secs: u64) -> String {
    // Time within the day.
    let sod = (secs % 86_400) as u32;
    let hour = sod / 3600;
    let minute = (sod % 3600) / 60;
    let second = sod % 60;

    // Days since 1970-01-01.
    let days = (secs / 86_400) as i64;

    // Civil-from-days (Howard Hinnant's algorithm, simplified for
    // dates well past 1970). Works for any reasonable run date.
    // Reference: https://howardhinnant.github.io/date_algorithms.html
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // 0..146096
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // 0..399
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // 0..365
    let mp = (5 * doy + 2) / 153; // 0..11
    let d = doy - (153 * mp + 2) / 5 + 1; // 1..31
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // 1..12
    let year = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, m, d, hour, minute, second,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known unix timestamps map to known ISO 8601 strings.
    #[test]
    fn iso8601_from_known_timestamps() {
        // 0 → 1970-01-01T00:00:00Z
        assert_eq!(iso8601_utc_from_secs(0),
            "1970-01-01T00:00:00Z");
        // 86400 → 1970-01-02T00:00:00Z (one day later)
        assert_eq!(iso8601_utc_from_secs(86_400),
            "1970-01-02T00:00:00Z");
        // 1_577_836_800 → 2020-01-01T00:00:00Z
        assert_eq!(iso8601_utc_from_secs(1_577_836_800),
            "2020-01-01T00:00:00Z");
        // 1_746_576_000 → 2025-05-07T00:00:00Z (verified via
        // `date -u -d @1746576000 +"%FT%TZ"`).
        assert_eq!(iso8601_utc_from_secs(1_746_576_000),
            "2025-05-07T00:00:00Z");
        // A leap year boundary: 2024 is a leap year.
        // 2024-03-01T00:00:00Z = ts 1_709_251_200
        assert_eq!(iso8601_utc_from_secs(1_709_251_200),
            "2024-03-01T00:00:00Z");
    }

    /// Time-of-day extraction is correct.
    #[test]
    fn iso8601_extracts_time_of_day() {
        // 13:45:30 on 2020-01-01: 1_577_836_800 + 13*3600 + 45*60 + 30
        let ts = 1_577_836_800u64 + 13*3600 + 45*60 + 30;
        assert_eq!(iso8601_utc_from_secs(ts), "2020-01-01T13:45:30Z");
    }

    /// `iso8601_utc_now` returns a syntactically valid string.
    #[test]
    fn iso8601_now_is_well_formed() {
        let s = iso8601_utc_now();
        // 20 chars: "YYYY-MM-DDTHH:MM:SSZ"
        assert_eq!(s.len(), 20);
        assert_eq!(s.chars().nth(4), Some('-'));
        assert_eq!(s.chars().nth(7), Some('-'));
        assert_eq!(s.chars().nth(10), Some('T'));
        assert_eq!(s.chars().nth(13), Some(':'));
        assert_eq!(s.chars().nth(16), Some(':'));
        assert!(s.ends_with('Z'));
    }

    /// `RunBlock::format` produces a well-framed block with the
    /// expected header lines and footer.
    #[test]
    fn run_block_format_has_markers_and_fields() {
        let block = RunBlock {
            version: "0.24.2-c.2",
            uuid: "r-0000000000000001",
            duration_secs: 81.0,
            cmdline: "unendlich-world",
            env_flags: "RD=64\nVSYNC=1",
            body: "GPU timer:\n  cull n=100\n",
        };
        let s = block.format();
        assert!(s.starts_with("==== RUN START"));
        assert!(s.contains("version: 0.24.2-c.2"));
        assert!(s.contains("uuid: r-0000000000000001"),
            "uuid line missing from formatted block; got:\n{}", s);
        assert!(s.contains("duration: 81.0s"));
        assert!(s.contains("cmdline: unendlich-world"));
        assert!(s.contains("env:\n  RD=64\n  VSYNC=1"));
        assert!(s.contains("---- summaries ----"));
        assert!(s.contains("GPU timer:\n  cull n=100"));
        // Footer present, on its own line, followed by blank line.
        assert!(s.contains("==== RUN END "));
        assert!(s.ends_with("\n\n"));
    }

    /// Body that already ends in a newline doesn't get a doubled
    /// newline before the END marker.
    #[test]
    fn run_block_does_not_double_newline_body() {
        let block = RunBlock {
            version: "v",
            uuid: "r-0000000000000002", duration_secs: 1.0,
            cmdline: "x", env_flags: "",
            body: "line1\nline2\n",
        };
        let s = block.format();
        // The transition from body to footer should be exactly one '\n'.
        assert!(s.contains("line2\n==== RUN END"));
    }

    /// Body without trailing newline is given one before the END marker.
    #[test]
    fn run_block_adds_newline_when_body_lacks_one() {
        let block = RunBlock {
            version: "v",
            uuid: "r-0000000000000003", duration_secs: 1.0,
            cmdline: "x", env_flags: "",
            body: "no_trailing_newline",
        };
        let s = block.format();
        assert!(s.contains("no_trailing_newline\n==== RUN END"));
    }

    /// Append to a temp path works and is idempotent (file grows by
    /// one block per call).
    #[test]
    fn append_to_file_grows_by_one_block_per_call() {
        let tmpdir = std::env::temp_dir();
        let path = tmpdir.join(format!(
            "unendlich_run_summary_test_{}.txt",
            std::process::id(),
        ));
        // Start clean.
        let _ = std::fs::remove_file(&path);

        let block = RunBlock {
            version: "test",
            uuid: "r-0000000000000004",
            duration_secs: 1.0,
            cmdline: "test",
            env_flags: "FOO=bar",
            body: "BODY1\n",
        };
        block.append(&path).expect("first append");
        block.append(&path).expect("second append");

        let contents = std::fs::read_to_string(&path).expect("read back");
        let start_count = contents.matches("==== RUN START ").count();
        let end_count = contents.matches("==== RUN END ").count();
        assert_eq!(start_count, 2);
        assert_eq!(end_count, 2);
        assert!(contents.contains("BODY1"));

        // Cleanup.
        let _ = std::fs::remove_file(&path);
    }

    // ===== Version-transition markers (v0.24.2c.3) =====

    /// Helper: a fresh, unique tempfile path per test. Uses process
    /// id + nanos-since-epoch so concurrent tests don't collide.
    fn unique_tempfile_path(tag: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "unendlich_run_summary_{}_{}_{}.txt",
            tag, std::process::id(), nanos,
        ))
    }

    /// `parse_last_version_marker` returns None on empty input.
    #[test]
    fn parse_version_marker_empty() {
        assert_eq!(parse_last_version_marker(""), None);
    }

    /// `parse_last_version_marker` finds a single START line.
    #[test]
    fn parse_version_marker_single_start() {
        let s = "\
==== VERSION 0.24.2-c.3 START 2026-05-11T13:00:00Z ====
==== RUN START 2026-05-11T13:00:01Z ====
...
==== RUN END 2026-05-11T13:00:02Z ====
";
        assert_eq!(parse_last_version_marker(s),
            Some("0.24.2-c.3".to_string()));
    }

    /// When multiple START markers exist, the most recent wins.
    #[test]
    fn parse_version_marker_returns_most_recent_start() {
        let s = "\
==== VERSION 0.24.2-c.2 START 2026-05-11T12:00:00Z ====
==== RUN START ... ====
==== RUN END ... ====
==== VERSION 0.24.2-c.2 END 2026-05-11T13:00:00Z ====
==== VERSION 0.24.2-c.3 START 2026-05-11T13:00:01Z ====
==== RUN START ... ====
==== RUN END ... ====
";
        assert_eq!(parse_last_version_marker(s),
            Some("0.24.2-c.3".to_string()));
    }

    /// END markers don't update the "last START" — they document
    /// closure but the most-recent START is what we compare to.
    #[test]
    fn parse_version_marker_ignores_end_lines() {
        let s = "\
==== VERSION 0.24.2-c.3 START 2026-05-11T13:00:00Z ====
==== VERSION 0.24.2-c.3 END 2026-05-11T14:00:00Z ====
";
        // The last START is c.3, which is what we return.
        assert_eq!(parse_last_version_marker(s),
            Some("0.24.2-c.3".to_string()));
    }

    /// A file without any version markers returns None.
    #[test]
    fn parse_version_marker_no_markers_returns_none() {
        let s = "\
==== RUN START 2026-05-11T13:00:00Z ====
version: 0.24.2-c.2
...
==== RUN END 2026-05-11T13:00:01Z ====
";
        assert_eq!(parse_last_version_marker(s), None);
    }

    /// Lines that look superficially similar but aren't START
    /// markers don't trigger false positives.
    #[test]
    fn parse_version_marker_robust_to_lookalikes() {
        let s = "\
This line mentions ==== VERSION 0.0.0 but doesn't have a START
==== VERSION  END 2026-05-11T14:00:00Z ====
==== VERSION something STOP 2026-05-11T15:00:00Z ====
";
        // None of the above are valid START markers — first lacks
        // the boundary, second has empty version, third uses STOP.
        assert_eq!(parse_last_version_marker(s), None);
    }

    /// First-ever append writes a START marker before the run.
    #[test]
    fn append_first_run_emits_start_marker() {
        let path = unique_tempfile_path("first_run");
        let _ = std::fs::remove_file(&path);

        let block = RunBlock {
            version: "0.24.2-c.3",
            uuid: "r-0000000000000005",
            duration_secs: 1.0,
            cmdline: "test",
            env_flags: "",
            body: "body1",
        };
        block.append(&path).expect("first append");

        let contents = std::fs::read_to_string(&path).expect("read back");
        assert!(contents.contains("==== VERSION 0.24.2-c.3 START "),
                "first append should emit a START marker; got:\n{}",
                contents);
        // No END marker on first append (nothing to close).
        assert!(!contents.contains("==== VERSION") || contents.matches("==== VERSION").count() == 1,
                "first append should not emit END markers; got:\n{}", contents);

        let _ = std::fs::remove_file(&path);
    }

    /// Second append with the SAME version emits no extra markers.
    #[test]
    fn append_same_version_emits_no_extra_markers() {
        let path = unique_tempfile_path("same_version");
        let _ = std::fs::remove_file(&path);

        let block = RunBlock {
            version: "0.24.2-c.3",
            uuid: "r-0000000000000006",
            duration_secs: 1.0,
            cmdline: "test",
            env_flags: "",
            body: "body",
        };
        block.append(&path).expect("first");
        block.append(&path).expect("second");

        let contents = std::fs::read_to_string(&path).expect("read");
        // Exactly one START marker, no END.
        assert_eq!(contents.matches("==== VERSION 0.24.2-c.3 START ").count(), 1,
                "expected exactly 1 START; got:\n{}", contents);
        assert_eq!(contents.matches("==== VERSION 0.24.2-c.3 END ").count(), 0,
                "expected 0 ENDs; got:\n{}", contents);
        // Two RUN blocks.
        assert_eq!(contents.matches("==== RUN START ").count(), 2);

        let _ = std::fs::remove_file(&path);
    }

    /// Append with a DIFFERENT version emits END for the previous
    /// version and START for the new one, back-to-back.
    #[test]
    fn append_different_version_emits_transition_markers() {
        let path = unique_tempfile_path("diff_version");
        let _ = std::fs::remove_file(&path);

        let v_old = RunBlock {
            version: "0.24.2-c.2",
            uuid: "r-0000000000000007",
            duration_secs: 1.0,
            cmdline: "test",
            env_flags: "",
            body: "old",
        };
        v_old.append(&path).expect("v_old");

        let v_new = RunBlock {
            version: "0.24.2-c.3",
            uuid: "r-0000000000000008",
            duration_secs: 1.0,
            cmdline: "test",
            env_flags: "",
            body: "new",
        };
        v_new.append(&path).expect("v_new");

        let contents = std::fs::read_to_string(&path).expect("read");
        assert!(contents.contains("==== VERSION 0.24.2-c.2 START "),
                "missing initial START; got:\n{}", contents);
        assert!(contents.contains("==== VERSION 0.24.2-c.2 END "),
                "missing transition END for old version; got:\n{}",
                contents);
        assert!(contents.contains("==== VERSION 0.24.2-c.3 START "),
                "missing transition START for new version; got:\n{}",
                contents);

        // Ordering: c.2 START → run-old → c.2 END → c.3 START → run-new.
        let pos_v2_start = contents.find("==== VERSION 0.24.2-c.2 START ").unwrap();
        let pos_v2_end   = contents.find("==== VERSION 0.24.2-c.2 END ").unwrap();
        let pos_v3_start = contents.find("==== VERSION 0.24.2-c.3 START ").unwrap();
        assert!(pos_v2_start < pos_v2_end);
        assert!(pos_v2_end < pos_v3_start);

        let _ = std::fs::remove_file(&path);
    }

    /// Three-version sequence: each transition adds the right
    /// END+START pair.
    #[test]
    fn append_three_version_sequence() {
        let path = unique_tempfile_path("three_version");
        let _ = std::fs::remove_file(&path);

        for v in ["v1", "v1", "v2", "v3", "v3"] {
            let block = RunBlock {
                version: v,
                uuid: "r-000000000000000b",
                duration_secs: 1.0,
                cmdline: "test",
                env_flags: "",
                body: "body",
            };
            block.append(&path).expect("append");
        }

        let contents = std::fs::read_to_string(&path).expect("read");
        // STARTs: v1 (initial), v2 (transition), v3 (transition) = 3.
        assert_eq!(contents.matches("==== VERSION v1 START ").count(), 1);
        assert_eq!(contents.matches("==== VERSION v2 START ").count(), 1);
        assert_eq!(contents.matches("==== VERSION v3 START ").count(), 1);
        // ENDs: v1 (closed when v2 arrives), v2 (closed when v3
        // arrives), v3 is still open = 2.
        assert_eq!(contents.matches("==== VERSION v1 END ").count(), 1);
        assert_eq!(contents.matches("==== VERSION v2 END ").count(), 1);
        assert_eq!(contents.matches("==== VERSION v3 END ").count(), 0);
        // RUNs: 5 total.
        assert_eq!(contents.matches("==== RUN START ").count(), 5);

        let _ = std::fs::remove_file(&path);
    }

    /// `find_last_version_marker` on a missing file returns `Ok(None)`
    /// (not an error).
    #[test]
    fn find_last_version_marker_missing_file_is_ok_none() {
        let path = unique_tempfile_path("missing_file");
        let _ = std::fs::remove_file(&path);
        // File does not exist.
        let result = find_last_version_marker(&path);
        assert!(matches!(result, Ok(None)),
            "missing file should be Ok(None); got: {:?}", result);
    }

    // ===== Retroactive-marker upgrade path (v0.24.2c.3) =====
    //
    // A real file that exists from v0.24.2c.2 days has many runs but
    // zero `==== VERSION X START ====` lines. The first v0.24.2c.3
    // append should detect this and emit a retroactive END for the
    // observed prior version (from `version:` lines) plus a START for
    // the current version — giving the file the expected visual
    // structure from that point forward.

    /// `parse_last_inblock_version` returns None on empty input.
    #[test]
    fn parse_inblock_version_empty() {
        assert_eq!(parse_last_inblock_version(""), None);
    }

    /// `parse_last_inblock_version` finds a single `version:` line.
    #[test]
    fn parse_inblock_version_single() {
        let s = "\
==== RUN START 2026-05-11T13:00:00Z ====
version: 0.24.2-c.2
duration: 30.0s
==== RUN END 2026-05-11T13:00:30Z ====
";
        assert_eq!(parse_last_inblock_version(s),
            Some("0.24.2-c.2".to_string()));
    }

    /// When multiple `version:` lines exist, the most recent (last)
    /// one wins.
    #[test]
    fn parse_inblock_version_returns_most_recent() {
        let s = "\
==== RUN START 2026-05-11T12:00:00Z ====
version: 0.24.2-c.2
==== RUN END 2026-05-11T12:00:30Z ====
==== RUN START 2026-05-11T13:00:00Z ====
version: 0.24.2-c.2
==== RUN END 2026-05-11T13:00:30Z ====
";
        assert_eq!(parse_last_inblock_version(s),
            Some("0.24.2-c.2".to_string()));
    }

    /// A pre-marker file with a DIFFERENT version triggers a
    /// retroactive END for the observed prior version + START for
    /// the current.
    #[test]
    fn append_upgrades_pre_marker_file_with_different_version() {
        let path = unique_tempfile_path("upgrade_diff");
        let _ = std::fs::remove_file(&path);

        // Hand-write a pre-marker file (simulating v0.24.2c.2 output).
        std::fs::write(&path, "\
==== RUN START 2026-05-11T13:00:00Z ====
version: 0.24.2-c.2
duration: 30.0s
cmdline: pre-marker run
env:
  RD=8
---- summaries ----
old summary body
==== RUN END 2026-05-11T13:00:30Z ====

").expect("write");

        let block = RunBlock {
            version: "0.24.2-c.3",
            uuid: "r-0000000000000009",
            duration_secs: 1.0,
            cmdline: "first c.3 run",
            env_flags: "RD=8",
            body: "new summary body",
        };
        block.append(&path).expect("append");

        let contents = std::fs::read_to_string(&path).expect("read");
        // Should contain a retroactive END for c.2.
        assert!(contents.contains("==== VERSION 0.24.2-c.2 END (retroactive)"),
            "expected retroactive END for c.2; got:\n{}", contents);
        // Should contain a normal START for c.3.
        assert!(contents.contains("==== VERSION 0.24.2-c.3 START "),
            "expected START for c.3; got:\n{}", contents);
        // Order: c.2 END comes after the existing run block, before
        // the c.3 START.
        let pos_old_run = contents.find("old summary body").unwrap();
        let pos_retro_end = contents.find("==== VERSION 0.24.2-c.2 END (retroactive)").unwrap();
        let pos_new_start = contents.find("==== VERSION 0.24.2-c.3 START ").unwrap();
        let pos_new_run = contents.find("new summary body").unwrap();
        assert!(pos_old_run < pos_retro_end);
        assert!(pos_retro_end < pos_new_start);
        assert!(pos_new_start < pos_new_run);

        let _ = std::fs::remove_file(&path);
    }

    /// A pre-marker file with the SAME version triggers just a START
    /// (no retroactive END, since there's no transition).
    #[test]
    fn append_upgrades_pre_marker_file_with_same_version() {
        let path = unique_tempfile_path("upgrade_same");
        let _ = std::fs::remove_file(&path);

        // Hand-write a pre-marker file at v0.24.2c.3 (e.g. user
        // built locally before the marker logic landed in their tree).
        std::fs::write(&path, "\
==== RUN START 2026-05-11T13:00:00Z ====
version: 0.24.2-c.3
duration: 30.0s
==== RUN END 2026-05-11T13:00:30Z ====

").expect("write");

        let block = RunBlock {
            version: "0.24.2-c.3",
            uuid: "r-000000000000000a",
            duration_secs: 1.0,
            cmdline: "test",
            env_flags: "",
            body: "new run on same version",
        };
        block.append(&path).expect("append");

        let contents = std::fs::read_to_string(&path).expect("read");
        // Should contain a START for c.3 (one only).
        assert_eq!(contents.matches("==== VERSION 0.24.2-c.3 START ").count(), 1,
            "expected exactly 1 START; got:\n{}", contents);
        // No END at all — same version means no transition.
        assert_eq!(contents.matches("==== VERSION 0.24.2-c.3 END ").count(), 0,
            "expected no END; got:\n{}", contents);
        // Subsequent appends with the same version should also not
        // add another START.
        block.append(&path).expect("append2");
        let contents2 = std::fs::read_to_string(&path).expect("read2");
        assert_eq!(contents2.matches("==== VERSION 0.24.2-c.3 START ").count(), 1,
            "expected still 1 START after 2nd append; got:\n{}", contents2);

        let _ = std::fs::remove_file(&path);
    }
}
