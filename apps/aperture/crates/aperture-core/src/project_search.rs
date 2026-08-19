//! project_search — "find in project" (Pulsar/Atom's `Ctrl+Shift+F`):
//! grep a candidate file list for a plain substring, case-insensitively,
//! and return hits as (path, line, column, line text) so a shell can render
//! a jump list and land the caret at the exact spot, not just the file.
//!
//! Deliberately NOT here (v0): regex (a `String::contains` scan is the
//! entire dependency-free cost; regex is a real follow-up once someone
//! actually asks for `\bfoo\b`), and multi-threaded scanning (the walk cap
//! that already bounds `file_finder::walk_files` bounds this too — a
//! sequential scan over a few thousand files finishes well under the
//! frame budget of a modal search panel, which isn't rendered every frame
//! the way the editor view is).
//!
//! Binary/unreadable files are skipped silently (`read_to_string` failing
//! is the cheap, correct binary-file test here — no magic-byte sniffing);
//! a project search that chokes on a stray `.png` isn't useful.

use std::path::{Path, PathBuf};

/// One matching line. `col` is a byte offset into the line (matches
/// `Buffer::pos_to_char`'s column convention elsewhere in aperture-core:
/// callers that need a char index still have to convert, same as any other
/// byte-indexed find result).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    /// Repo-relative display path — same convention as `file_finder::rank`.
    pub rel: String,
    /// 0-based line index (matches `Pos::line` / `Editor::goto_line`).
    pub line: usize,
    pub col: usize,
    /// The full line text, trimmed of its trailing newline — shown as the
    /// result row so a jump doesn't require opening the file first to see
    /// whether it's the right hit.
    pub text: String,
}

/// Search `files` (already walked — this module doesn't walk the
/// filesystem itself, the same split `file_finder` uses between "find
/// candidates" and "rank/search candidates") for `query`, case-insensitive.
/// Stops at `max_hits` total; `max_per_file` keeps one huge generated file
/// (a lockfile, a bundled JS blob) from burying every other result.
///
/// An empty query returns no hits rather than "everything" — unlike
/// `file_finder::rank`'s empty-query browse mode, a blank project search is
/// almost never what someone meant to see (every line of every file), and
/// scanning the whole tree for it would be wasted work on every keystroke
/// before the person's typed anything.
pub fn search_files(root: &Path, files: &[PathBuf], query: &str, max_hits: usize, max_per_file: usize) -> Vec<Hit> {
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    let needle = query.to_lowercase();
    'files: for f in files {
        let Ok(contents) = std::fs::read_to_string(f) else { continue };
        let rel = f.strip_prefix(root).unwrap_or(f).to_string_lossy().into_owned();
        let mut hits_this_file = 0;
        for (line_ix, line) in contents.lines().enumerate() {
            let Some(col) = line.to_lowercase().find(&needle) else { continue };
            out.push(Hit { path: f.clone(), rel: rel.clone(), line: line_ix, col, text: line.to_string() });
            hits_this_file += 1;
            if hits_this_file >= max_per_file {
                continue 'files;
            }
            if out.len() >= max_hits {
                break 'files;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aperture-project-search-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn finds_matches_across_files_case_insensitively() {
        let root = tmp("basic");
        std::fs::write(root.join("a.rs"), "fn main() {\n    let Needle = 1;\n}\n").unwrap();
        std::fs::write(root.join("b.rs"), "// nothing here\n").unwrap();
        let files = vec![root.join("a.rs"), root.join("b.rs")];

        let hits = search_files(&root, &files, "needle", 100, 100);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rel, "a.rs");
        assert_eq!(hits[0].line, 1);
        assert_eq!(hits[0].col, 8);
        assert_eq!(hits[0].text, "    let Needle = 1;");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_query_returns_nothing() {
        let root = tmp("empty");
        std::fs::write(root.join("a.rs"), "anything\n").unwrap();
        let files = vec![root.join("a.rs")];
        assert!(search_files(&root, &files, "", 100, 100).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn per_file_cap_stops_one_file_from_dominating() {
        let root = tmp("per-file-cap");
        let many = "target\n".repeat(50);
        std::fs::write(root.join("noisy.txt"), &many).unwrap();
        std::fs::write(root.join("quiet.txt"), "one target here\n").unwrap();
        let files = vec![root.join("noisy.txt"), root.join("quiet.txt")];

        let hits = search_files(&root, &files, "target", 1000, 3);
        let noisy: Vec<_> = hits.iter().filter(|h| h.rel == "noisy.txt").collect();
        assert_eq!(noisy.len(), 3, "per-file cap applies even with room left in the total cap");
        assert!(hits.iter().any(|h| h.rel == "quiet.txt"), "the capped file didn't crowd out the other file entirely");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn total_cap_stops_the_whole_scan() {
        let root = tmp("total-cap");
        let files: Vec<PathBuf> = (0..10)
            .map(|i| {
                let p = root.join(format!("f{i}.txt"));
                std::fs::write(&p, "hit\n").unwrap();
                p
            })
            .collect();

        let hits = search_files(&root, &files, "hit", 4, 100);
        assert_eq!(hits.len(), 4);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn skips_unreadable_binary_like_files_without_panicking() {
        let root = tmp("binary");
        std::fs::write(root.join("a.bin"), [0xffu8, 0xfe, 0x00, 0xff, 0xfd]).unwrap();
        std::fs::write(root.join("a.txt"), "needle\n").unwrap();
        let files = vec![root.join("a.bin"), root.join("a.txt")];
        let hits = search_files(&root, &files, "needle", 100, 100);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rel, "a.txt");
        let _ = std::fs::remove_dir_all(&root);
    }
}
