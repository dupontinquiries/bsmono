//! Tab-completion-as-voice-scoring.
//!
//! Voice "open colorchooser.cpp" only resolves to the right file if the
//! file appeared in recent terminal output (rg/tree/dust/ls). But the
//! shell has the answer for free — that's what tab completion is for.
//!
//! This module is the lightweight first pass: when voice mentions a
//! filename-shaped token, glob the shell's CWD recursively (bounded depth)
//! and surface matches as candidate filenames the resolver can score
//! against. It's a poor approximation of real shell completion (we don't
//! consult zsh's compsys, we don't get branch/PID/host completions, just
//! files) but it covers the most common case at near-zero cost.
//!
//! The full version — a hidden second shell that we drive via TAB and
//! parse output from — is documented in TODO_recent_fact_lookup.md and is
//! its own session. This file is the cheap-and-good version.
//!
//! Design constraints:
//!   - Must be FAST. Voice latency is already 800ms+ from whisper alone;
//!     we can't add another second of glob walking.
//!   - Must respect .gitignore-ish patterns (skip target/, node_modules/,
//!     .git/) to avoid drowning in build artifacts.
//!   - Must be bounded. Hard cap on entries returned. Hard cap on walk depth.
//!   - Must handle the user's actual cwd, not the app's. We get this
//!     from PtyTerminal::shell_cwd() which reads /proc/<pid>/cwd.

use std::path::{Path, PathBuf};

/// Maximum number of files to return. Tuned to keep search_corpus
/// matching cheap — the resolver runs O(n_entities * n_ref_tokens) so
/// 200 entities × 3 tokens × ~50 corpus chars = ~30k char comparisons
/// per voice utterance, which is fine.
const MAX_FILES: usize = 200;

/// Maximum directory recursion depth from cwd. Most projects fit in 6.
const MAX_DEPTH: usize = 6;

/// Skip these directory names entirely. Build artifacts and VCS internals
/// produce thousands of files that aren't relevant for voice matching.
const SKIP_DIRS: &[&str] = &[
    "target", "node_modules", ".git", ".cargo", "build",
    "dist", ".next", ".cache", "vendor", "__pycache__",
    ".venv", "venv", ".tox", ".mypy_cache", ".pytest_cache",
    ".idea", ".vscode", "Pods", ".gradle",
];

/// Walk the cwd and return up to MAX_FILES path strings (relative to cwd).
/// Filters skip dirs aggressively. Order: depth-first, alphabetical within
/// each directory — the resolver doesn't care about order so we don't sort.
pub fn glob_cwd_files(cwd: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(MAX_FILES);
    walk(cwd, cwd, 0, &mut out);
    out
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    if out.len() >= MAX_FILES { return; }
    if depth >= MAX_DEPTH { return; }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_FILES { return; }
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        // Skip hidden files except a few well-known config files we WOULD
        // want to match (.zshrc, .bashrc, .gitignore, .envrc).
        if name.starts_with('.') {
            let allowed = matches!(name, ".zshrc" | ".bashrc" | ".gitignore"
                | ".envrc" | ".profile" | ".bash_profile" | ".vimrc");
            if !allowed { continue; }
        }
        let ft = match entry.file_type() { Ok(f) => f, Err(_) => continue };
        if ft.is_dir() {
            if SKIP_DIRS.contains(&name) { continue; }
            // Recurse
            walk(root, &path, depth + 1, out);
        } else if ft.is_file() || ft.is_symlink() {
            // Compute relative path from the project root for nicer display.
            let rel = path.strip_prefix(root).unwrap_or(&path);
            if let Some(rel_str) = rel.to_str() {
                out.push(rel_str.to_string());
            }
        }
    }
}

/// Filter the glob results down to candidates that share at least one
/// alphanumeric character-cluster with the voice reference. This avoids
/// stuffing 200 unrelated filenames into the entity scorer when the
/// voice reference has nothing to do with most of them.
///
/// Uses a separator-collapsed match: voice "color chooser cpp" → "colorchoosercpp"
/// and we keep filenames whose collapsed form contains a substring of
/// length ≥ 4 from the voice reference. That ratio of recall vs noise
/// works well for common filenames.
pub fn filter_by_voice_overlap(files: &[String], voice_text: &str) -> Vec<String> {
    let voice_collapsed: String = voice_text.to_lowercase().chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if voice_collapsed.len() < 4 { return Vec::new(); }
    // Build all 4+ char substrings of the voice reference. Cheap because
    // voice utterances are short.
    let needles: Vec<&str> = (0..voice_collapsed.len().saturating_sub(3))
        .map(|i| &voice_collapsed[i..i+4])
        .collect();
    let mut out: Vec<String> = Vec::new();
    for f in files {
        let collapsed: String = f.to_lowercase().chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        if needles.iter().any(|n| collapsed.contains(n)) {
            out.push(f.clone());
            if out.len() >= 50 { break; }  // hard cap on what we hand to scorer
        }
    }
    out
}

/// Convenience: do both walking and filtering in one call. Returns the
/// candidate filenames most likely to be what the user voiced. Empty Vec
/// if cwd isn't readable or no candidates overlap.
pub fn candidates_for_voice(cwd: &Path, voice_text: &str) -> Vec<String> {
    let files = glob_cwd_files(cwd);
    filter_by_voice_overlap(&files, voice_text)
}

/// v0.1.580: turn a list of project file paths into File entities that
/// `entity_extract::resolve_reference` can rank. This is the adapter
/// that was the missing link between the recall-biased prefilter
/// (filter_by_voice_overlap, returns up to 50 unranked candidates) and
/// the ranking engine (resolve_reference, which scores a spoken
/// reference against an entity's display + aliases, with the filename
/// separator-collapse logic that handles "color chooser" → colorchooser.cpp).
///
/// Each path becomes an Entity{kind: File, primary: path, display:
/// basename, aliases: [basename, stem, full path]} so resolve_reference
/// can match on any of those forms. Mirrors the File-entity shape the
/// per-tool extractors already produce (entity_extract.rs ~358).
pub fn files_to_entities(paths: &[String]) -> Vec<crate::entity_extract::Entity> {
    use crate::entity_extract::{Entity, EntityKind};
    paths
        .iter()
        .map(|path| {
            let basename = path.rsplit('/').next().unwrap_or(path).to_string();
            // Stem = basename without the final extension, so "main.rs"
            // also matches a voiced "main".
            let stem = basename
                .rsplit_once('.')
                .map(|(s, _ext)| s.to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| basename.clone());
            let mut aliases = vec![basename.clone(), path.clone()];
            if stem != basename {
                aliases.push(stem);
            }
            Entity {
                kind: EntityKind::File,
                primary: path.clone(),
                display: basename,
                aliases,
                confidence: 1.0,
                metadata: std::collections::HashMap::new(),
            }
        })
        .collect()
}

/// v0.1.580: the full pure pipeline for "open the X" voice file
/// resolution — prefilter the CWD file set by voice overlap, then RANK
/// the survivors against the spoken phrase. Returns (path, score) pairs
/// best-first. This composes the existing pieces; it doesn't duplicate
/// the matcher. The IO (glob) is isolated in glob_cwd_files; everything
/// downstream is pure (and tested via rank_paths_for_voice below).
pub fn resolve_file_for_voice(cwd: &Path, voice_text: &str, top_n: usize)
    -> Vec<(String, f32)>
{
    let candidates = candidates_for_voice(cwd, voice_text);
    rank_paths_for_voice(&candidates, voice_text, top_n)
}

/// Pure core of resolve_file_for_voice: given already-globbed candidate
/// paths + the spoken phrase, rank them. Split out from the IO so it's
/// directly testable. "open the readme" over [README.md,
/// src/reader/header.rs] ranks README.md first.
pub fn rank_paths_for_voice(paths: &[String], voice_text: &str, top_n: usize)
    -> Vec<(String, f32)>
{
    let entities = files_to_entities(paths);
    crate::entity_extract::resolve_reference(voice_text, &entities, top_n)
        .into_iter()
        .map(|m| (m.entity.primary, m.score))
        .collect()
}

/// Convenience: try the running shell's cwd via /proc/<pid>/cwd. Returns
/// None on platforms without /proc or when the path has been deleted
/// (which can happen after `rmdir` on the cwd's parent).
pub fn shell_cwd_or_app_cwd(pty: Option<&crate::pty_term::PtyTerminal>) -> Option<PathBuf> {
    if let Some(p) = pty {
        if let Some(cwd) = p.shell_cwd() {
            return Some(cwd);
        }
    }
    std::env::current_dir().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse_matches_separators() {
        let files = vec![
            "src/components/colorchooser.cpp".to_string(),
            "src/main.rs".to_string(),
            "tests/test.rs".to_string(),
        ];
        // "color chooser cpp" → "colorchoosercpp" — should match colorchooser.cpp
        let got = filter_by_voice_overlap(&files, "color chooser cpp");
        assert!(got.iter().any(|f| f.contains("colorchooser")),
            "expected colorchooser.cpp to match 'color chooser cpp', got {:?}", got);
    }

    #[test]
    fn collapse_handles_underscore_filenames() {
        let files = vec![
            "src/color_chooser.cpp".to_string(),
            "src/main.rs".to_string(),
        ];
        let got = filter_by_voice_overlap(&files, "color chooser");
        assert!(got.iter().any(|f| f.contains("color_chooser")),
            "expected color_chooser.cpp to match, got {:?}", got);
    }

    #[test]
    fn short_voice_returns_nothing() {
        let files = vec!["a.rs".to_string()];
        // 3-char voice text → no needles, return empty
        let got = filter_by_voice_overlap(&files, "abc");
        assert!(got.is_empty());
    }

    #[test]
    fn files_to_entities_builds_aliases() {
        let ents = files_to_entities(&["src/main.rs".to_string()]);
        assert_eq!(ents.len(), 1);
        let e = &ents[0];
        assert_eq!(e.primary, "src/main.rs", "primary is the full path");
        assert_eq!(e.display, "main.rs", "display is the basename");
        // aliases include basename, full path, and the stem ("main")
        assert!(e.aliases.contains(&"main.rs".to_string()));
        assert!(e.aliases.contains(&"src/main.rs".to_string()));
        assert!(e.aliases.contains(&"main".to_string()),
            "stem 'main' must be an alias so a voiced 'main' matches");
    }

    #[test]
    fn rank_surfaces_best_file_first() {
        // "open the readme" should rank README.md ahead of an unrelated
        // file that happens to share a substring.
        let files = vec![
            "src/reader/header.rs".to_string(),
            "README.md".to_string(),
            "src/main.rs".to_string(),
        ];
        let ranked = rank_paths_for_voice(&files, "open the readme", 5);
        assert!(!ranked.is_empty(), "should rank something for 'readme'");
        assert_eq!(ranked[0].0, "README.md",
            "README.md must rank first for 'readme', got {:?}", ranked);
    }

    #[test]
    fn rank_matches_stem_without_extension() {
        // "open main" → main.rs via the stem alias.
        let files = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];
        let ranked = rank_paths_for_voice(&files, "open main", 5);
        assert_eq!(ranked.first().map(|(p, _)| p.as_str()), Some("src/main.rs"),
            "stem match should surface main.rs, got {:?}", ranked);
    }

    #[test]
    fn rank_empty_candidates_is_empty() {
        assert!(rank_paths_for_voice(&[], "open the readme", 5).is_empty());
    }
}
