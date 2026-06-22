// gitmind-core/src/checkpoint.rs
//
// The checkpoint engine — "aggressive branch creation with checkpoints,
// backups, better autocommits, code timeline overviews", all from one
// mechanism.
//
// Idea: snapshot the entire working tree into a SHADOW ref namespace
//   refs/checkpoints/<session>
// as a normal commit chain, WITHOUT touching the user's index (.git/
// index) or HEAD. Your real history stays curated; meanwhile every
// change is captured and scrubbable.
//
// How we snapshot without disturbing the user's state — the key trick is
// a throwaway index file via GIT_INDEX_FILE:
//
//   1. GIT_INDEX_FILE=<tmp> git add -A      (stage everything into a
//      private temp index; respects .gitignore so build junk is skipped)
//   2. GIT_INDEX_FILE=<tmp> git write-tree  -> tree oid
//   3. git commit-tree <tree> [-p <prev>] -m "..."  -> commit oid
//   4. git update-ref refs/checkpoints/<session> <commit>
//
// The user's working index and HEAD are never read or written. A
// background file watcher (notify crate, wired in the frontend) calls
// snapshot() on a debounce; the timeline view is just `list()` over the
// checkpoint ref's commit chain.

use crate::backend::{GitCli, RepoBackend};
use std::path::PathBuf;

/// Per-file line delta from a checkpoint diff.
#[derive(Clone, Debug, PartialEq)]
pub struct FileDelta {
    pub path: String,
    pub added: u64,
    pub removed: u64,
}

/// One entry in the checkpoint timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct Checkpoint {
    /// Commit oid (short) of the snapshot.
    pub oid: String,
    /// Unix seconds of the snapshot.
    pub when: i64,
    /// Human label (auto-generated; later AI-named from the diff).
    pub label: String,
}

pub struct CheckpointEngine<'a> {
    repo: &'a GitCli,
    /// Session name → ref is refs/checkpoints/<session>. Lets parallel
    /// worktrees / AI sessions keep independent timelines.
    session: String,
}

impl<'a> CheckpointEngine<'a> {
    pub fn new(repo: &'a GitCli, session: impl Into<String>) -> Self {
        CheckpointEngine { repo, session: session.into() }
    }

    fn refname(&self) -> String {
        format!("refs/checkpoints/{}", self.session)
    }

    /// Current tip of this session's checkpoint chain, if any.
    fn tip(&self) -> Option<String> {
        self.repo
            .git(&["rev-parse", "--verify", "--quiet", &self.refname()])
            .ok()
            .filter(|s| !s.is_empty())
    }

    /// Take a snapshot now. Returns None (Ok) when nothing changed since
    /// the last checkpoint, so the timeline doesn't fill with identical
    /// trees. Returns Some(checkpoint) when a new snapshot was written.
    pub fn snapshot(&self, label: &str) -> Result<Option<Checkpoint>, String> {
        // Private temp index path inside .git so it shares a filesystem
        // with the object store (rename/links stay cheap) and gets
        // cleaned with the repo.
        let tmp_index: PathBuf = self
            .repo
            .root()
            .join(".git")
            .join(format!("gitmind-ckpt-index-{}", self.session));

        // Stage everything into the throwaway index.
        run_with_index(self.repo, &tmp_index, &["add", "-A"])?;
        let tree = run_with_index(self.repo, &tmp_index, &["write-tree"])?;
        let _ = std::fs::remove_file(&tmp_index);

        // Skip if identical to the previous checkpoint's tree (no churn).
        let prev = self.tip();
        if let Some(p) = &prev {
            if let Ok(prev_tree) = self.repo.git(&["rev-parse", &format!("{p}^{{tree}}")]) {
                if prev_tree == tree {
                    return Ok(None);
                }
            }
        }

        // commit-tree with the prior checkpoint as parent → a real,
        // walkable chain that `git log refs/checkpoints/<session>` shows.
        let mut args: Vec<String> = vec!["commit-tree".into(), tree, "-m".into(), label.into()];
        if let Some(p) = &prev {
            args.push("-p".into());
            args.push(p.clone());
        }
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let commit = self.repo.git(&arg_refs)?;

        self.repo.git(&["update-ref", &self.refname(), &commit])?;

        Ok(Some(Checkpoint {
            oid: commit.chars().take(12).collect(),
            when: now_secs(),
            label: label.to_string(),
        }))
    }

    /// The timeline: most-recent-first list of this session's
    /// checkpoints. Drives the code-timeline overview UI.
    pub fn list(&self, limit: usize) -> Result<Vec<Checkpoint>, String> {
        if self.tip().is_none() {
            return Ok(Vec::new());
        }
        let fmt = "%h%x00%ct%x00%s";
        let out = self.repo.git(&[
            "log",
            &format!("--max-count={limit}"),
            &format!("--format={fmt}"),
            &self.refname(),
        ])?;
        let mut v = Vec::new();
        for line in out.lines() {
            let mut parts = line.split('\u{0}');
            let oid = parts.next().unwrap_or("").to_string();
            let when = parts.next().unwrap_or("0").parse().unwrap_or(0);
            let label = parts.next().unwrap_or("").to_string();
            if !oid.is_empty() {
                v.push(Checkpoint { oid, when, label });
            }
        }
        Ok(v)
    }

    /// Materialize a checkpoint into a fresh side-by-side worktree
    /// (never clobbers current work). This is the safe restore path and
    /// also the basis for "worktree side-by-side": each checkpoint or
    /// branch can be checked out next to the others.
    pub fn materialize(&self, oid: &str, into: &str) -> Result<(), String> {
        self.repo.git(&["worktree", "add", "--detach", into, oid]).map(|_| ())
    }

    /// Diff a checkpoint against another revision, or (when `to` is None) the
    /// current working tree. Returns per-file line deltas — the "what changed
    /// in this snapshot" view for the timeline.
    pub fn diff(&self, from: &str, to: Option<&str>) -> Result<Vec<FileDelta>, String> {
        let mut args: Vec<&str> = vec!["diff", "--numstat", from];
        if let Some(t) = to {
            args.push(t);
        }
        let out = self.repo.git(&args)?;
        let mut deltas = Vec::new();
        for line in out.lines() {
            let mut f = line.split('\t');
            let added = f.next().unwrap_or("0");
            let removed = f.next().unwrap_or("0");
            let path = f.next().unwrap_or("");
            if path.is_empty() {
                continue;
            }
            deltas.push(FileDelta {
                path: path.to_string(),
                // binary files show "-" in numstat; treat as 0.
                added: added.parse().unwrap_or(0),
                removed: removed.parse().unwrap_or(0),
            });
        }
        Ok(deltas)
    }

    /// Extract a checkpoint's full tree into `into` as a plain directory
    /// (via `git archive | tar`), without registering a linked worktree the
    /// way `materialize` does. Returns the number of files written.
    pub fn extract(&self, oid: &str, into: &std::path::Path) -> Result<usize, String> {
        use std::process::{Command, Stdio};
        std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
        let mut archive = Command::new("git")
            .arg("-C")
            .arg(self.repo.root())
            .args(["archive", "--format=tar", oid])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("git archive: {e}"))?;
        let out = archive.stdout.take().ok_or("no archive output")?;
        let status = Command::new("tar")
            .arg("-x")
            .arg("-C")
            .arg(into)
            .stdin(Stdio::from(out))
            .status()
            .map_err(|e| format!("tar: {e}"))?;
        let ar_status = archive.wait().map_err(|e| e.to_string())?;
        if !status.success() || !ar_status.success() {
            return Err("extract failed".into());
        }
        let listing = self.repo.git(&["ls-tree", "-r", "--name-only", oid])?;
        Ok(listing.lines().filter(|l| !l.is_empty()).count())
    }
}

/// Run a git command with a private GIT_INDEX_FILE so the user's real
/// index is untouched.
fn run_with_index(repo: &GitCli, index: &PathBuf, args: &[&str]) -> Result<String, String> {
    use std::process::Command;
    let out = Command::new("git")
        .arg("-C")
        .arg(repo.root())
        .env("GIT_INDEX_FILE", index)
        .args(args)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
