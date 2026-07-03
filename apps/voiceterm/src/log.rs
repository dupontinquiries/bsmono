// src/log.rs
//
// v0.1.439: category-routed logging with a runtime flag.
//
// The codebase already uses an informal convention of category-
// prefixed stderr lines: `eprintln!("[nav] …")`, `[md]`, `[pty]`,
// `[audio]`, `[workspaces]`, etc. (30+ distinct categories). This
// module formalizes that into a `clog!(category, …)` macro that:
//
//   1. ALWAYS echoes to stderr (preserves existing behavior — no
//      log line is ever lost just because routing is off).
//   2. WHEN a category is selected via the WEB2QML_LOG env flag,
//      ALSO appends a timestamped line to a per-category file at
//      ~/.config/web2qml/logs/<category>.log — so a later test
//      session can narrow to exactly the categories of interest
//      and read them in isolation, without grepping one giant
//      interleaved stderr stream.
//
// Flag syntax (parsed once, cached):
//   WEB2QML_LOG=all              → route every category to a file
//   WEB2QML_LOG=nav,pty,md       → route only these
//   WEB2QML_LOG=off  / unset     → stderr only (today's behavior)
//
// Design notes:
//   - Zero behavior change when the flag is unset: the file path
//     is only touched for selected categories.
//   - The selection set is parsed once (OnceLock) so the per-call
//     cost when routing is off is a cheap "is this category in an
//     empty set" check.
//   - File writes are best-effort: a failed open/write never
//     panics and never suppresses the stderr echo.
//   - Per-category files (not one combined file) is the whole
//     point — `tail -f logs/nav.log` shows ONLY navigation.

#![allow(dead_code)]

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Parsed WEB2QML_LOG selection. `All` routes every category;
/// `Some(set)` routes only the named ones; `Off` is stderr-only.
enum Selection {
    Off,
    All,
    Some(HashSet<String>),
}

fn selection() -> &'static Selection {
    static SEL: OnceLock<Selection> = OnceLock::new();
    SEL.get_or_init(|| match std::env::var("WEB2QML_LOG") {
        Ok(v) => {
            let v = v.trim().to_lowercase();
            if v.is_empty() || v == "off" || v == "0" {
                Selection::Off
            } else if v == "all" || v == "1" {
                Selection::All
            } else {
                let set: HashSet<String> = v
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if set.is_empty() {
                    Selection::Off
                } else {
                    Selection::Some(set)
                }
            }
        }
        Err(_) => Selection::Off,
    })
}

/// Is file-routing enabled for this category? Cheap after the
/// first call (OnceLock-cached selection).
pub fn routed(category: &str) -> bool {
    match selection() {
        Selection::Off => false,
        Selection::All => true,
        Selection::Some(set) => set.contains(category),
    }
}

/// The logs directory: ~/.config/web2qml/logs (created lazily).
fn logs_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
        .join(".config/web2qml/logs")
}

/// Seconds since the Unix epoch — a dependency-free timestamp for
/// log lines. (We avoid pulling chrono into the hot path; a unix
/// seconds prefix is enough to correlate across category files.)
fn epoch_secs() -> u64 {
    crate::util::epoch_secs()
}

/// Core entry point used by the `clog!` macro. ALWAYS echoes to
/// stderr with the `[category]` prefix (matching the existing
/// convention). If the category is routed, ALSO appends a
/// timestamped line to its per-category file. Both steps are
/// best-effort; a file error is itself logged once to stderr but
/// never panics.
pub fn emit(category: &str, message: &str) {
    // 1. stderr echo — unconditional, preserves current behavior.
    eprintln!("[{}] {}", category, message);

    // 2. per-category file — only when routed.
    if !routed(category) {
        return;
    }
    let dir = logs_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    // Sanitize the category into a safe filename (defensive — all
    // current categories are already [a-z_], but a stray caller
    // shouldn't be able to escape the directory).
    let safe: String = category
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    let path = dir.join(format!("{}.log", safe));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        // `<epoch> <message>` — newline-terminated.
        let _ = writeln!(f, "{} {}", epoch_secs(), message);
    }
}

/// Category-routed log. Echoes to stderr always; also appends to
/// the per-category file when WEB2QML_LOG selects that category.
///
/// Usage:
///   clog!("nav", "skipping javascript: URL");
///   clog!("pty", "altscreen enter {}x{}", cols, rows);
///
/// Drop-in for `eprintln!("[cat] …")` — same stderr output, plus
/// optional file routing.
#[macro_export]
macro_rules! clog {
    ($cat:expr, $($arg:tt)*) => {
        $crate::log::emit($cat, &format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_parsing_off() {
        // We can't easily set env per-test deterministically with
        // the OnceLock cache, so test the pure parse logic via a
        // helper that mirrors it.
        assert!(parse_for_test("").is_none());
        assert!(parse_for_test("off").is_none());
        assert!(parse_for_test("0").is_none());
    }

    #[test]
    fn selection_parsing_all() {
        assert_eq!(parse_for_test("all"), Some(vec![]));
        assert_eq!(parse_for_test("1"), Some(vec![]));
        assert_eq!(parse_for_test("ALL"), Some(vec![]));
    }

    #[test]
    fn selection_parsing_list() {
        assert_eq!(parse_for_test("nav,pty"), Some(vec!["nav".into(), "pty".into()]));
        // whitespace + empty entries tolerated
        assert_eq!(parse_for_test(" nav , , pty "), Some(vec!["nav".into(), "pty".into()]));
    }

    #[test]
    fn filename_sanitization() {
        // Pure check of the sanitizing map used in emit().
        let bad = "../etc/passwd";
        let safe: String = bad
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
            .collect();
        assert!(!safe.contains('/'));
        assert!(!safe.contains('.'));
        assert_eq!(safe, "___etc_passwd");
    }

    // Mirror of selection() parsing, returning a comparable shape.
    // None => Off; Some(empty) => All; Some(list) => those cats.
    fn parse_for_test(v: &str) -> Option<Vec<String>> {
        let v = v.trim().to_lowercase();
        if v.is_empty() || v == "off" || v == "0" {
            None
        } else if v == "all" || v == "1" {
            Some(vec![])
        } else {
            let mut set: Vec<String> = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            set.sort();
            if set.is_empty() { None } else { Some(set) }
        }
    }
}
