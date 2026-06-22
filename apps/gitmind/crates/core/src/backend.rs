// gitmind-core/src/backend.rs
//
// All git-facing operations go through the RepoBackend trait. The
// default implementation, GitCli, shells out to the `git` binary —
// the same deliberate choice the reference's git_status.rs made:
// honest, fully ecosystem-compatible (hooks, config, credential
// helpers all just work), and fast enough (~5-20 ms per query). The
// trait boundary means a future "hybrid" backend can intercept the
// large-file paths to route through the chunk store without any
// frontend changing.
//
// Everything here is sync + cheap-per-call; frontends run it on the
// background-thread + snapshot pattern the reference uses, never on the
// UI tick.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One changed path in the working tree, parsed from porcelain output.
#[derive(Clone, Debug, PartialEq)]
pub struct ChangedFile {
    pub path: String,
    /// Two-char XY porcelain code, e.g. " M", "A ", "??", "UU".
    pub xy: String,
    pub staged: bool,
    pub untracked: bool,
    pub conflicted: bool,
}

/// Snapshot of repo state for the status panel / badge.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepoStatus {
    pub in_repo: bool,
    pub branch: String,
    pub detached: bool,
    pub ahead: u32,
    pub behind: u32,
    pub has_upstream: bool,
    pub files: Vec<ChangedFile>,
}

impl RepoStatus {
    pub fn dirty(&self) -> bool {
        !self.files.is_empty()
    }
}

pub trait RepoBackend {
    fn status(&self) -> Result<RepoStatus, String>;
    fn local_branches(&self) -> Result<Vec<String>, String>;
    /// Stage a single path (swipe-right gesture in the UI).
    fn stage(&self, path: &str) -> Result<(), String>;
    /// Unstage a single path (swipe-left, reversible half).
    fn unstage(&self, path: &str) -> Result<(), String>;
    /// Repo root (worktree top).
    fn root(&self) -> &Path;
}

/// Subprocess-backed backend rooted at a worktree directory.
pub struct GitCli {
    root: PathBuf,
}

impl GitCli {
    /// Discover the repo containing `start`. Returns None if not in a
    /// work tree (caller renders the not-a-repo state).
    pub fn discover(start: impl AsRef<Path>) -> Option<Self> {
        let out = Command::new("git")
            .arg("-C")
            .arg(start.as_ref())
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if root.is_empty() {
            return None;
        }
        Some(GitCli { root: PathBuf::from(root) })
    }

    /// Run a git subcommand in the repo, returning trimmed stdout. Errors
    /// carry stderr so the UI can show the real git message.
    pub fn git(&self, args: &[&str]) -> Result<String, String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()
            .map_err(|e| format!("spawn git: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    }
}

impl RepoBackend for GitCli {
    fn root(&self) -> &Path {
        &self.root
    }

    fn status(&self) -> Result<RepoStatus, String> {
        let mut st = RepoStatus { in_repo: true, ..Default::default() };

        // Branch / detached.
        match self.git(&["symbolic-ref", "--quiet", "--short", "HEAD"]) {
            Ok(b) if !b.is_empty() => st.branch = b,
            _ => {
                st.detached = true;
                st.branch = self.git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
            }
        }

        // Ahead/behind vs upstream, if configured.
        if let Ok(counts) = self.git(&["rev-list", "--left-right", "--count", "@{upstream}...HEAD"]) {
            let mut it = counts.split_whitespace();
            if let (Some(b), Some(a)) = (it.next(), it.next()) {
                st.behind = b.parse().unwrap_or(0);
                st.ahead = a.parse().unwrap_or(0);
                st.has_upstream = true;
            }
        }

        // Changed files via porcelain v1 (-z would be more robust for
        // odd filenames; v1 line parsing is fine for the first cut).
        let porc = self.git(&["status", "--porcelain"])?;
        for line in porc.lines() {
            if line.len() < 3 {
                continue;
            }
            let xy = &line[..2];
            let path = line[3..].to_string();
            let untracked = xy == "??";
            let conflicted = xy.contains('U') || xy == "AA" || xy == "DD";
            let staged = !untracked && xy.as_bytes()[0] != b' ' && !conflicted;
            st.files.push(ChangedFile {
                path,
                xy: xy.to_string(),
                staged,
                untracked,
                conflicted,
            });
        }
        Ok(st)
    }

    fn local_branches(&self) -> Result<Vec<String>, String> {
        let out = self.git(&["for-each-ref", "--format=%(refname:short)", "refs/heads"])?;
        Ok(out.lines().map(|s| s.to_string()).collect())
    }

    fn stage(&self, path: &str) -> Result<(), String> {
        self.git(&["add", "--", path]).map(|_| ())
    }

    fn unstage(&self, path: &str) -> Result<(), String> {
        // `restore --staged` is the modern, index-only unstage; it never
        // touches working-tree contents, so a swipe-left to unstage is
        // safely reversible.
        self.git(&["restore", "--staged", "--", path]).map(|_| ())
    }
}
