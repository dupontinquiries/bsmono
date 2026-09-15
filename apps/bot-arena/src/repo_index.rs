//! repo_index — resolve a path from a conversation log to a file in this repo.
//!
//! ## Why this is suffix matching and not prefix stripping
//!
//! The obvious implementation is "drop `/home/claude/repo/` and you have the
//! repo-relative path". That was the first guess and it is wrong. Measured
//! across the 219-conversation sample, tool paths are rooted at over a dozen
//! different sandbox directories, because each session mounted the tree
//! wherever it liked:
//!
//! ```text
//!   8842  /home/claude/work           1334  /home/claude/monorepo
//!   3771  /home/claude/rusty-board    1210  /home/claude/logscope-suite
//!   3096  /home/claude/unendlich      1049  /home/claude/repo
//!   2834  /home/claude/bo-suite        911  /home/claude/clawbound-rs
//!   2281  /home/claude/early-fps       892  /home/claude/mono
//! ```
//!
//! A prefix rule would resolve the 1,049 `/home/claude/repo` hits and miss the
//! other ~25,000. So the key is the LONGEST TRAILING SEGMENT SEQUENCE that
//! names exactly one file in the repo: `/home/claude/unendlich/src/main.rs`
//! and `/tmp/x/src/main.rs` both reduce to whatever `src/main.rs` is, if the
//! repo has exactly one.
//!
//! ## Ambiguity is refused, not guessed
//!
//! `src/main.rs` matches dozens of crates in a monorepo this size. Returning
//! the first one would attach a confident, wrong `repo_path:` line to a corpus
//! entry — worse than no line, because a reader cannot tell a guess from a
//! fact. [`RepoIndex::resolve`] returns `None` when the shortest unambiguous
//! suffix does not exist, and [`RepoIndex::resolve_detailed`] says which case
//! it hit so the converter can report the three counts separately.
//!
//! Measured on the sample: 25,186 unique matches, 6,284 ambiguous, 2,657 with
//! no match at all (files from other repositories, `/tmp` scratch, and outputs
//! that were never committed).

use std::collections::HashMap;
use std::path::Path;

/// Why a path did or did not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match {
    /// Exactly one repo file ends with this suffix.
    Unique(String),
    /// Several do. Carries how many, because "ambiguous (2)" is a naming
    /// problem worth fixing and "ambiguous (40)" is just `mod.rs`.
    Ambiguous(usize),
    /// Nothing in the repo ends with any suffix of this path.
    None,
}

/// An index from path suffix to the repo files that end with it.
///
/// Built once and queried per tool call — an export has thousands of them, and
/// re-walking the tree for each would dominate the conversion.
#[derive(Debug, Clone, Default)]
pub struct RepoIndex {
    /// suffix -> repo-relative paths ending with it.
    by_suffix: HashMap<String, Vec<String>>,
    n_files: usize,
}

impl RepoIndex {
    pub fn is_empty(&self) -> bool {
        self.n_files == 0
    }

    pub fn file_count(&self) -> usize {
        self.n_files
    }

    /// Build from repo-relative paths (`git ls-files` output, or a walk).
    ///
    /// Every suffix of every path is indexed, which is O(total path segments)
    /// entries — ~100k for a 17k-file tree, a few MB. That buys an O(depth)
    /// lookup instead of an O(files) scan per query, and there are tens of
    /// thousands of queries per run.
    pub fn from_paths<I, S>(paths: I) -> RepoIndex
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut by_suffix: HashMap<String, Vec<String>> = HashMap::new();
        let mut n_files = 0;
        for p in paths {
            let p = p.as_ref().trim();
            if p.is_empty() {
                continue;
            }
            n_files += 1;
            let segs: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
            for i in 0..segs.len() {
                let key = segs[i..].join("/");
                by_suffix.entry(key).or_default().push(p.to_string());
            }
        }
        RepoIndex { by_suffix, n_files }
    }

    /// Walk a directory tree, skipping the places that would poison the index.
    ///
    /// `target/` and `.git/` are excluded because a build directory contains
    /// thousands of generated files with source-like names, and indexing them
    /// makes real sources look ambiguous.
    pub fn from_dir(root: &Path) -> RepoIndex {
        let mut rel = Vec::new();
        walk(root, root, &mut rel);
        RepoIndex::from_paths(rel)
    }

    /// The repo path for a logged path, if exactly one file matches.
    pub fn resolve(&self, path: &str) -> Option<String> {
        match self.resolve_detailed(path) {
            Match::Unique(p) => Some(p),
            _ => None,
        }
    }

    /// As [`Self::resolve`], but distinguishes ambiguous from absent.
    ///
    /// Tries the LONGEST suffix first and walks down. Longest-first matters:
    /// `apps/bot-arena/src/main.rs` should resolve on its full tail before the
    /// search ever reaches the hopelessly common `main.rs`, and a shortest-
    /// first search would report ambiguity for a path that was perfectly
    /// specific.
    pub fn resolve_detailed(&self, path: &str) -> Match {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if segs.is_empty() {
            return Match::None;
        }
        let mut best_ambiguous: Option<usize> = None;
        for i in 0..segs.len() {
            let key = segs[i..].join("/");
            match self.by_suffix.get(&key) {
                Some(hits) if hits.len() == 1 => return Match::Unique(hits[0].clone()),
                Some(hits) => {
                    // Remember the longest (first-tried) ambiguous run, but
                    // keep going: a shorter suffix can never be unique if a
                    // longer one was ambiguous, so this loop will only find
                    // more ambiguity -- recording the tightest count is the
                    // useful part.
                    if best_ambiguous.is_none() {
                        best_ambiguous = Some(hits.len());
                    }
                }
                None => {}
            }
        }
        match best_ambiguous {
            Some(n) => Match::Ambiguous(n),
            None => Match::None,
        }
    }
}

impl crate::chatlog::Correlate for RepoIndex {
    fn resolve(&self, path: &str) -> Option<String> {
        RepoIndex::resolve(self, path)
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            // A build dir has more files than the source tree and they collide
            // with real module names; `.git` is objects, not paths.
            if name == "target" || name == ".git" || name == "node_modules" {
                continue;
            }
            walk(root, &p, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> RepoIndex {
        RepoIndex::from_paths([
            "apps/bot-arena/src/main.rs",
            "apps/logscope/src/main.rs",
            "apps/bot-arena/src/engine.rs",
            "shared/device-kit/src/source_plan.rs",
            "maintenance/tools/lock_check.py",
        ])
    }

    /// The whole reason this is not prefix stripping: the same repo file is
    /// reached through a dozen different sandbox roots.
    #[test]
    fn resolves_through_any_sandbox_root() {
        let ix = index();
        for root in [
            "/home/claude/repo",
            "/home/claude/work",
            "/home/claude/unendlich",
            "/tmp/scratch-42",
        ] {
            let p = format!("{root}/shared/device-kit/src/source_plan.rs");
            assert_eq!(
                ix.resolve(&p),
                Some("shared/device-kit/src/source_plan.rs".to_string()),
                "failed for root {root}"
            );
        }
    }

    /// A guessed correlation is worse than none: the reader cannot tell it
    /// from a real one.
    #[test]
    fn ambiguity_is_refused_rather_than_guessed() {
        let ix = index();
        assert_eq!(ix.resolve("/home/claude/x/src/main.rs"), None);
        assert_eq!(ix.resolve_detailed("/x/src/main.rs"), Match::Ambiguous(2));
    }

    /// Longest-suffix-first is what makes a specific path resolve even though
    /// its own tail is ambiguous.
    #[test]
    fn a_longer_suffix_disambiguates() {
        let ix = index();
        assert_eq!(
            ix.resolve("/home/claude/work/apps/bot-arena/src/main.rs"),
            Some("apps/bot-arena/src/main.rs".to_string())
        );
        assert_eq!(
            ix.resolve("/whatever/apps/logscope/src/main.rs"),
            Some("apps/logscope/src/main.rs".to_string())
        );
    }

    #[test]
    fn paths_from_elsewhere_do_not_match() {
        let ix = index();
        assert_eq!(ix.resolve_detailed("/mnt/user-data/outputs/x.bundle"), Match::None);
        assert_eq!(ix.resolve_detailed(""), Match::None);
        assert_eq!(ix.resolve("/etc/passwd"), None);
    }

    #[test]
    fn an_exact_repo_relative_path_resolves_to_itself() {
        let ix = index();
        assert_eq!(
            ix.resolve("apps/bot-arena/src/engine.rs"),
            Some("apps/bot-arena/src/engine.rs".to_string())
        );
    }

    #[test]
    fn an_empty_index_resolves_nothing_without_panicking() {
        let ix = RepoIndex::default();
        assert!(ix.is_empty());
        assert_eq!(ix.resolve("/home/claude/repo/src/main.rs"), None);
    }
}
