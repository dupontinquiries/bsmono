//! log.rs — category-routed logging, ported from voiceterm's `clog!`. `clog!("voice", "…")` ALWAYS
//! echoes to stderr (drop-in for the `eprintln!("voice: …")` convention already used across the voice
//! modules); when the category is selected via LOGSCOPE_LOG, it ALSO appends a timestamped line to
//! `~/.config/logscope/logs/<category>.log`, so a debugging session can `tail -f` exactly one category
//! (mic level meters, whisper results, dispatch, fast-path races, phonetic matches, …) instead of
//! grepping one interleaved stream. Zero behavior change and near-zero cost when LOGSCOPE_LOG is unset.
//!
//! Flag syntax (parsed once, cached):
//!   LOGSCOPE_LOG=all          → route every category to a file
//!   LOGSCOPE_LOG=voice,whisper → route only these
//!   LOGSCOPE_LOG=off / unset  → stderr only (today's behavior)
//!
//! Usage: `crate::clog!("voice", "mic level {:.4}", peak);`

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

enum Selection { Off, All, Some(HashSet<String>) }

fn selection() -> &'static Selection {
    static SEL: OnceLock<Selection> = OnceLock::new();
    SEL.get_or_init(|| parse_selection(std::env::var("LOGSCOPE_LOG").ok().as_deref()))
}

/// Pure parse (no env access) so the logic is unit-testable without process-global env races.
fn parse_selection(raw: Option<&str>) -> Selection {
    match raw {
        None => Selection::Off,
        Some(v) => {
            let v = v.trim().to_lowercase();
            if v.is_empty() || v == "off" || v == "0" { Selection::Off }
            else if v == "all" || v == "1" { Selection::All }
            else {
                let set: HashSet<String> = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                if set.is_empty() { Selection::Off } else { Selection::Some(set) }
            }
        }
    }
}

/// Is file-routing enabled for this category? Cheap after the first call (OnceLock-cached).
pub fn routed(category: &str) -> bool {
    match selection() { Selection::Off => false, Selection::All => true, Selection::Some(set) => set.contains(category) }
}

fn logs_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join(".config/logscope/logs")
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Core entry point for `clog!`. ALWAYS echoes to stderr with the `[category]` prefix; if routed, ALSO
/// appends a timestamped line to its per-category file. Both best-effort; a file error never panics or
/// suppresses the stderr echo.
pub fn emit(category: &str, message: &str) {
    eprintln!("[{category}] {message}");
    if !routed(category) { return; }
    let dir = logs_dir();
    if std::fs::create_dir_all(&dir).is_err() { return; }
    let safe: String = category.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    let path = dir.join(format!("{safe}.log"));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} {}", epoch_secs(), message);
    }
}

/// Category-routed log. `clog!("voice", "mic level {:.4}", peak)`. Drop-in for `eprintln!("[cat] …")`.
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
    fn selection_parsing() {
        assert!(matches!(parse_selection(None), Selection::Off));
        assert!(matches!(parse_selection(Some("off")), Selection::Off));
        assert!(matches!(parse_selection(Some("0")), Selection::Off));
        assert!(matches!(parse_selection(Some("")), Selection::Off));
        assert!(matches!(parse_selection(Some("all")), Selection::All));
        assert!(matches!(parse_selection(Some("ALL")), Selection::All));
        assert!(matches!(parse_selection(Some("1")), Selection::All));
    }

    #[test]
    fn selection_list_and_whitespace() {
        match parse_selection(Some("voice,whisper")) {
            Selection::Some(s) => { assert!(s.contains("voice")); assert!(s.contains("whisper")); assert_eq!(s.len(), 2); }
            _ => panic!("expected a category set"),
        }
        match parse_selection(Some(" voice , , whisper ")) {
            Selection::Some(s) => assert_eq!(s.len(), 2, "whitespace + empty entries tolerated"),
            _ => panic!("expected a category set"),
        }
    }

    #[test]
    fn filename_sanitization() {
        let bad = "../etc/passwd";
        let safe: String = bad.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
        assert!(!safe.contains('/'), "path separators must be stripped from the category filename");
        assert!(!safe.contains(".."));
    }
}
