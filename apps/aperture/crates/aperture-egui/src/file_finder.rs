//! file_finder — the TODO.md-named next step for Ctrl+O: "walk the workspace
//! root and reuse `uikit_egui::fuzzy` for ranking." Typed-path entry stays as
//! the fallback (it's honest and works over ssh, per palette_ui's original
//! reasoning) — this adds fuzzy matching against real files on top of it,
//! the way it should have worked from the start.
//!
//! No new dependency: `std::fs::read_dir`, an explicit stack instead of
//! recursion (a repo with a deeply nested `node_modules` shouldn't blow the
//! call stack even though we skip that directory by name — belt and
//! suspenders), and a hard cap so a huge monorepo can't turn Ctrl+O into a
//! multi-second stall.

use std::path::{Path, PathBuf};

/// Directory names never descended into: VCS internals, build output, and
/// the one dependency directory big enough to matter across ecosystems.
const SKIP_DIRS: &[&str] = &["target", "node_modules", "dist", "build"];

/// Walk `root` breadth-first-ish (an explicit LIFO stack; order is a
/// filesystem detail, not a promise — callers rank the result, they don't
/// rely on walk order) collecting file paths, capped at `max`. Hidden
/// entries (dotfiles/dotdirs — `.git`, `.cargo`, editor swap dirs) are
/// skipped by convention: a file picker showing `.git/COMMIT_EDITMSG` isn't
/// helping anyone find their source.
pub fn walk_files(root: &Path, max: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= max {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            if out.len() >= max {
                break;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
                continue;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                out.push(entry.path());
            }
        }
    }
    out
}

/// Fuzzy-rank `files` (repo-relative for both matching and display — nobody
/// wants to read `/home/dev/monorepo/apps/aperture/...` in a picker) against
/// `query`, capped at `limit`. Tie-break: shorter relative paths first, then
/// lexical — same convention the symbol popup uses, so ranking "feels" the
/// same across the app's pickers.
pub fn rank(root: &Path, files: &[PathBuf], query: &str, limit: usize) -> Vec<(PathBuf, String)> {
    let mut scored: Vec<(i32, String, PathBuf)> = files
        .iter()
        .filter_map(|f| {
            let rel = f.strip_prefix(root).unwrap_or(f).to_string_lossy().into_owned();
            uikit_egui::fuzzy::score(query, &rel).map(|s| (s, rel, f.clone()))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, rel, f)| (f, rel)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aperture-file-finder-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn walks_real_files_and_skips_noise_directories() {
        let root = tmp("walk");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join(".git/objects")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/leftpad")).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("target/debug/binary"), "").unwrap();
        std::fs::write(root.join(".git/objects/abc"), "").unwrap();
        std::fs::write(root.join("node_modules/leftpad/index.js"), "").unwrap();

        let found = walk_files(&root, 1000);
        let rels: Vec<String> = found.iter().map(|f| f.strip_prefix(&root).unwrap().to_string_lossy().into_owned()).collect();
        assert!(rels.contains(&"src/main.rs".to_string()));
        assert!(rels.contains(&"Cargo.toml".to_string()));
        assert!(!rels.iter().any(|r| r.starts_with("target")), "{rels:?}");
        assert!(!rels.iter().any(|r| r.starts_with(".git")), "{rels:?}");
        assert!(!rels.iter().any(|r| r.starts_with("node_modules")), "{rels:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn walk_respects_the_cap_even_with_more_files_available() {
        let root = tmp("cap");
        for i in 0..20 {
            std::fs::write(root.join(format!("f{i}.txt")), "").unwrap();
        }
        let found = walk_files(&root, 5);
        assert_eq!(found.len(), 5, "stops at the cap rather than walking everything");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rank_scores_fuzzy_matches_and_tie_breaks_by_shorter_path() {
        let root = tmp("rank");
        let files = vec![
            root.join("apps/aperture/src/state.rs"),
            root.join("apps/aperture/src/view.rs"),
            root.join("state.rs"),
            root.join("unrelated/thing.txt"),
        ];
        let ranked = rank(&root, &files, "state", 10);
        let rels: Vec<&str> = ranked.iter().map(|(_, r)| r.as_str()).collect();
        assert!(rels.contains(&"state.rs"));
        assert!(rels.contains(&"apps/aperture/src/state.rs"));
        assert!(!rels.iter().any(|r| r.contains("unrelated")), "{rels:?}");
        assert_eq!(rels[0], "state.rs", "shorter path wins the tie-break at equal fuzzy quality");
    }

    #[test]
    fn rank_with_empty_query_returns_everything_up_to_the_limit() {
        let root = tmp("empty-query");
        let files: Vec<PathBuf> = (0..5).map(|i| root.join(format!("f{i}.rs"))).collect();
        let ranked = rank(&root, &files, "", 3);
        assert_eq!(ranked.len(), 3, "empty query still returns candidates, capped at the display limit");
    }
}
