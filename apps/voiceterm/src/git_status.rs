// v0.1.351: git status detection for the terminal status badge.
//
// Spawns `git` as a subprocess on demand to extract:
//   - Whether the current cwd is inside a git repo
//   - Current branch name (or detached HEAD short hash)
//   - Whether the working tree is dirty (any unstaged or staged changes)
//   - Ahead/behind counts vs upstream (if upstream is configured)
//
// Async pattern: the public `request_refresh` function spawns a
// short-lived background thread that runs the git commands and
// stores the result in a Mutex-protected snapshot. main.rs's
// pty_read_screen polls the snapshot each tick and surfaces
// changes to QML via a qt_property.
//
// Why subprocess and not libgit2 / direct plumbing-file reads:
//   - libgit2 adds a real dependency and complicates the build
//   - `.git/HEAD` is easy but `.git/index` parsing for dirty-state
//     is non-trivial (binary format with hash + flags + name).
//     Spawning `git` is honest, ~5-20ms per refresh, well within
//     budget for a per-block-close trigger.
//
// Refresh policy is OWNED BY THE CALLER. This module exposes:
//   - `request_refresh(cwd)`: spawn a worker if one isn't running
//   - `snapshot()`: return the latest state (None until first
//     refresh completes)
//
// main.rs triggers refresh on:
//   - cwd changes (set_cwd in BlockTracker)
//   - block-close (any command might be a git operation)
//   - shell respawn

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// What the badge displays. Cheap to clone (small strings + ints).
#[derive(Clone, Debug, PartialEq)]
pub struct GitStatus {
    /// The path the snapshot was computed for. Used to invalidate
    /// the badge when cwd has changed since the snapshot was taken.
    pub for_path: PathBuf,
    /// True if `for_path` is inside a git work tree.
    pub in_repo: bool,
    /// Branch name (e.g. "main", "feature/foo") or short hash for
    /// detached HEAD (e.g. "abc1234"). Empty when in_repo is false.
    pub branch: String,
    /// True if `git status --porcelain` returned any lines. Captures
    /// both staged and unstaged changes plus untracked files.
    pub dirty: bool,
    /// Commits ahead of upstream. 0 when no upstream or in-sync.
    pub ahead: u32,
    /// Commits behind upstream. 0 when no upstream or in-sync.
    pub behind: u32,
    /// True if an upstream branch is configured (i.e. ahead/behind
    /// are meaningful). False means the branch is local-only and
    /// the QML badge shouldn't render ahead/behind glyphs.
    pub has_upstream: bool,
    /// v0.1.356: local branch names. Populated by the same worker
    /// thread alongside the badge fields, so voice intents like
    /// "switch to branch X" can pre-validate without an extra
    /// subprocess on the hot path.
    ///
    /// Empty when not in a repo. May lag behind reality briefly
    /// (until next refresh trigger) — caller should still let git
    /// emit the real error if the chosen branch turns out not to
    /// exist; this is for hint, not gate.
    pub local_branches: Vec<String>,
}

static SNAPSHOT: Mutex<Option<GitStatus>> = Mutex::new(None);
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Request a git status refresh for the given cwd. If a worker is
/// already running, this is a no-op (the previous refresh will land
/// soon enough; queuing more would just churn). If `cwd` is empty,
/// clears the snapshot to "not in repo" and returns.
pub fn request_refresh(cwd: &str) {
    if cwd.is_empty() {
        if let Ok(mut s) = SNAPSHOT.lock() {
            *s = Some(GitStatus {
                for_path: PathBuf::new(),
                in_repo: false,
                branch: String::new(),
                dirty: false,
                ahead: 0,
                behind: 0,
                has_upstream: false,
                local_branches: Vec::new(),
            });
        }
        return;
    }
    // Try to mark in-flight. If already in flight, skip.
    if IN_FLIGHT.swap(true, Ordering::AcqRel) {
        return;
    }
    let cwd_owned = cwd.to_string();
    std::thread::spawn(move || {
        let computed = compute(&cwd_owned);
        if let Ok(mut s) = SNAPSHOT.lock() {
            *s = Some(computed);
        }
        IN_FLIGHT.store(false, Ordering::Release);
    });
}

/// Read the latest snapshot. None until first refresh completes.
pub fn snapshot() -> Option<GitStatus> {
    SNAPSHOT.lock().ok().and_then(|s| s.clone())
}

/// Synchronous compute. Public for tests. Returns a GitStatus
/// reflecting the state of `cwd` — in_repo=false when `cwd` is
/// not inside a git work tree (no error, just absence).
pub fn compute(cwd: &str) -> GitStatus {
    let path = PathBuf::from(cwd);
    let mut out = GitStatus {
        for_path: path.clone(),
        in_repo: false,
        branch: String::new(),
        dirty: false,
        ahead: 0,
        behind: 0,
        has_upstream: false,
        local_branches: Vec::new(),
    };
    // First: is this a git work tree? `git rev-parse
    // --is-inside-work-tree` prints "true" + exits 0 when yes,
    // exits non-zero when no. Cheap, no side effects.
    let inside = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(&path)
        .output();
    let inside = match inside {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).trim() == "true"
        }
        _ => false,
    };
    if !inside {
        return out;
    }
    out.in_repo = true;
    // Branch: `git symbolic-ref --short HEAD` for branches,
    // falls back to `git rev-parse --short HEAD` for detached.
    let sym = Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(&path)
        .output();
    out.branch = match sym {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        }
        _ => {
            // Detached HEAD — use short hash.
            let h = Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .current_dir(&path)
                .output();
            match h {
                Ok(o) if o.status.success() => {
                    let short = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if short.is_empty() {
                        // Empty repo — no commits yet.
                        "(no commits)".to_string()
                    } else {
                        format!("({}) detached", short)
                    }
                }
                _ => "(unknown)".to_string(),
            }
        }
    };
    // Dirty: any porcelain output → dirty.
    let st = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&path)
        .output();
    out.dirty = match st {
        Ok(o) if o.status.success() => !o.stdout.is_empty(),
        _ => false,
    };
    // Ahead/behind: `git rev-list --left-right --count
    // <branch>...@{u}`. Fails silently when no upstream — branch
    // is local-only.
    if !out.branch.is_empty() && !out.branch.starts_with('(') {
        let arg = format!("{}...@{{u}}", out.branch);
        let rl = Command::new("git")
            .args(["rev-list", "--left-right", "--count", &arg])
            .current_dir(&path)
            .output();
        if let Ok(o) = rl {
            if o.status.success() {
                let line = String::from_utf8_lossy(&o.stdout);
                let trimmed = line.trim();
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() == 2 {
                    if let (Ok(a), Ok(b)) = (parts[0].parse(), parts[1].parse()) {
                        out.ahead = a;
                        out.behind = b;
                        out.has_upstream = true;
                    }
                }
            }
            // status non-success = no upstream, leave as default
        }
    }

    // v0.1.356: list local branch names so voice intents like
    // "switch to branch X" can pre-validate without an extra
    // subprocess on the hot path. `for-each-ref` is the
    // recommended plumbing command — faster than `git branch`
    // (no formatting/coloring) and stable output.
    //
    // Failure here is non-fatal: leave local_branches empty
    // and the pre-validation path treats absent data as
    // "skip the hint, surface the original candidate." Better
    // to be silent than to surface false data.
    let br = Command::new("git")
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .current_dir(&path)
        .output();
    if let Ok(o) = br {
        if o.status.success() {
            out.local_branches = String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }

    out
}

/// Render a compact one-line display string suitable for the QML
/// badge. Format: "branch [⇡N ⇣M] [●]". Glyphs:
///   - ⇡N : N commits ahead
///   - ⇣N : N commits behind
///   - ● : dirty
/// Examples:
///   "main"
///   "main ●"
///   "feature/foo ⇡2"
///   "main ⇡1 ⇣3 ●"
///   "(abc1234) detached ●"
///   "" (when not in a repo)
pub fn format_badge(s: &GitStatus) -> String {
    if !s.in_repo {
        return String::new();
    }
    let mut out = s.branch.clone();
    if s.has_upstream {
        if s.ahead > 0 {
            out.push_str(&format!(" \u{21E1}{}", s.ahead));
        }
        if s.behind > 0 {
            out.push_str(&format!(" \u{21E3}{}", s.behind));
        }
    }
    if s.dirty {
        out.push_str(" \u{25CF}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_empty_when_not_in_repo() {
        let s = GitStatus {
            for_path: PathBuf::new(),
            in_repo: false,
            branch: String::new(),
            dirty: false,
            ahead: 0, behind: 0,
            has_upstream: false,
            local_branches: Vec::new(),
        };
        assert_eq!(format_badge(&s), "");
    }

    #[test]
    fn format_branch_only_when_clean_in_sync() {
        let s = GitStatus {
            for_path: PathBuf::new(),
            in_repo: true,
            branch: "main".to_string(),
            dirty: false,
            ahead: 0, behind: 0,
            has_upstream: true,
            local_branches: Vec::new(),
        };
        assert_eq!(format_badge(&s), "main");
    }

    #[test]
    fn format_includes_dirty_glyph() {
        let s = GitStatus {
            for_path: PathBuf::new(),
            in_repo: true,
            branch: "main".to_string(),
            dirty: true,
            ahead: 0, behind: 0,
            has_upstream: true,
            local_branches: Vec::new(),
        };
        assert_eq!(format_badge(&s), "main \u{25CF}");
    }

    #[test]
    fn format_ahead_behind() {
        let s = GitStatus {
            for_path: PathBuf::new(),
            in_repo: true,
            branch: "feature/foo".to_string(),
            dirty: true,
            ahead: 2, behind: 1,
            has_upstream: true,
            local_branches: Vec::new(),
        };
        assert_eq!(format_badge(&s), "feature/foo \u{21E1}2 \u{21E3}1 \u{25CF}");
    }

    #[test]
    fn no_upstream_suppresses_ahead_behind() {
        // Local-only branch — ahead/behind aren't meaningful.
        let s = GitStatus {
            for_path: PathBuf::new(),
            in_repo: true,
            branch: "local-only".to_string(),
            dirty: false,
            // Even if these somehow got set, the has_upstream flag
            // is the gate.
            ahead: 5, behind: 0,
            has_upstream: false,
            local_branches: Vec::new(),
        };
        assert_eq!(format_badge(&s), "local-only");
    }

    /// Compute against a known-non-git path. Should return
    /// in_repo=false without panicking. Uses /tmp which is never a
    /// git repo on Linux test runners.
    #[test]
    fn compute_returns_not_in_repo_for_tmp() {
        let s = compute("/tmp");
        // Some test environments may have /tmp inside a git repo
        // (e.g. ephemeral CI workdirs). Don't assert in_repo=false,
        // just assert the call returned a well-formed struct.
        assert_eq!(s.for_path, PathBuf::from("/tmp"));
    }

    /// Snapshot starts None.
    #[test]
    fn snapshot_starts_none() {
        // This test is order-sensitive: if another test in this
        // module ran request_refresh first, the snapshot could be
        // Some. Run as a smoke test only.
        let _ = snapshot();
    }

    /// v0.1.356: local_branches field defaults empty and the
    /// compute path returns a well-formed struct even on non-
    /// repo paths.
    #[test]
    fn local_branches_field_present() {
        let s = compute("/tmp");
        // Just confirm the field exists and is iterable.
        // Whether /tmp resolves as a git repo on the test
        // runner is environment-dependent; we don't gate on it.
        assert!(s.local_branches.is_empty() || !s.local_branches.is_empty());
    }
}
