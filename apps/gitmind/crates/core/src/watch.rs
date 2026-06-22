// gitmind-core/src/watch.rs
//
// File-write "time machine". A background watcher (the `notify` crate,
// wired in the frontend) calls CheckpointEngine::snapshot whenever a
// watched file is written. Two deliberate constraints from the design
// discussion:
//
//   - "select default types" — only certain file types trigger snapshots
//     (source + small text by default). Large media does NOT; media goes
//     through explicit ingest, not per-keystroke snapshots.
//   - "no log updates in realtime" — snapshots land in the shadow ref
//     namespace (refs/checkpoints/*), never on a branch, so your visible
//     history stays clean while every write is still recoverable.
//
// This module is the pure, testable POLICY (what to snapshot, how often).
// The event loop that calls it is a thin notify wrapper; keeping the
// decision logic here means it can be unit-tested without filesystem
// events or timers.

/// Default file types that get per-write snapshots. Code and small text —
/// the things you edit by hand and want infinite undo on. Intentionally
/// excludes media/archives/binaries (those are ingested, not snapshotted).
pub const DEFAULT_TYPES: &[&str] = &[
    "rs", "py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "go", "c", "h", "cpp", "cc", "hpp",
    "java", "rb", "php", "cs", "swift", "kt", "scala", "lua", "sh", "bash",
    "md", "txt", "toml", "yaml", "yml", "json", "xml", "html", "css", "sql",
];

#[derive(Clone, Debug)]
pub struct WatchPolicy {
    /// Lower-cased extensions that trigger a snapshot.
    exts: Vec<String>,
    /// Coalesce bursts of writes within this window into one snapshot, so
    /// a save-on-every-keystroke editor doesn't create hundreds of them.
    pub debounce_ms: u64,
}

impl Default for WatchPolicy {
    fn default() -> Self {
        WatchPolicy {
            exts: DEFAULT_TYPES.iter().map(|s| s.to_string()).collect(),
            debounce_ms: 1500,
        }
    }
}

impl WatchPolicy {
    /// Custom set of extensions (without dots).
    pub fn with_types<I, S>(types: I, debounce_ms: u64) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        WatchPolicy {
            exts: types.into_iter().map(|s| s.as_ref().to_ascii_lowercase()).collect(),
            debounce_ms,
        }
    }

    /// Should a write to `path` trigger a time-machine snapshot?
    pub fn watches(&self, path: &str) -> bool {
        // Ignore VCS internals and editor temp/backup files outright.
        if path.contains("/.git/") || path.ends_with('~') || path.ends_with(".tmp") || path.ends_with(".swp") {
            return false;
        }
        let ext = match path.rsplit('.').next() {
            Some(e) if e != path => e.to_ascii_lowercase(),
            _ => return false, // no extension → not a default-watched type
        };
        self.exts.iter().any(|e| e == &ext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watches_default_code_types() {
        let p = WatchPolicy::default();
        assert!(p.watches("src/store.rs"));
        assert!(p.watches("README.md"));
        assert!(p.watches("DEEP/nested/Main.JAVA")); // case-insensitive
    }

    #[test]
    fn ignores_media_and_noise() {
        let p = WatchPolicy::default();
        assert!(!p.watches("assets/clip.mp4")); // media → ingest, not snapshot
        assert!(!p.watches("photo.jpg"));
        assert!(!p.watches("src/store.rs.swp")); // editor swap
        assert!(!p.watches(".git/index")); // vcs internals
        assert!(!p.watches("Makefile")); // no extension
    }

    #[test]
    fn custom_types() {
        let p = WatchPolicy::with_types(["tex", "bib"], 500);
        assert!(p.watches("paper.tex"));
        assert!(!p.watches("main.rs"));
        assert_eq!(p.debounce_ms, 500);
    }
}
