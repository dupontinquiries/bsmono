//! Per-commit build/check status — turn the log into "what's green".
//!
//! For each commit we can materialize its tree, run a check command (default
//! `cargo check`), and cache the verdict keyed by commit oid in the store's
//! CHECKS table. The native log then shows a marker per commit (✓ builds, ✗
//! broken, · unknown) so good candidates to test/run/pin are visible at a
//! glance. Results are cached: a commit is only re-checked on demand.

use crate::store::ChunkStore;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckStatus {
    pub commit: String,
    /// Whether the check command exited 0.
    pub ok: bool,
    /// Process exit code (or -1 if killed by signal / unknown).
    pub code: i32,
    pub duration_ms: u64,
    /// Unix seconds when the check ran.
    pub ran_at: u64,
    /// The command that was run (for provenance / re-runs).
    pub cmd: Vec<String>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Run a check command against a commit's materialized tree and cache the
/// result. The command runs in a fresh temp worktree containing the commit's
/// files (default for the caller: `cargo check`). Output is inherited so the
/// user sees compiler errors live; only the verdict is stored.
pub fn run(
    store: &ChunkStore,
    commit_ref: &str,
    cmd: &[String],
) -> Result<CheckStatus, String> {
    let commit = crate::native::resolve_ref_or_commit(store, commit_ref)?;
    let meta = crate::portage::native_commit(store, &commit)?;
    let work = std::env::temp_dir().join(format!("gitmind-check-{}-{}", &commit[..commit.len().min(8)], std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let mut n = 0;
    crate::native::materialize_tree(store, &meta.tree, &work, &mut n)?;

    let (prog, rest) = cmd.split_first().ok_or("empty check command")?;
    let start = std::time::Instant::now();
    let status = std::process::Command::new(prog)
        .args(rest)
        .current_dir(&work)
        .status()
        .map_err(|e| format!("check command failed to start: {e}"))?;
    let duration_ms = start.elapsed().as_millis() as u64;
    let _ = std::fs::remove_dir_all(&work);

    let st = CheckStatus {
        commit: commit.clone(),
        ok: status.success(),
        code: status.code().unwrap_or(-1),
        duration_ms,
        ran_at: now_secs(),
        cmd: cmd.to_vec(),
    };
    let json = serde_json::to_vec(&st).map_err(|e| e.to_string())?;
    store.put_check(&commit, &json)?;
    Ok(st)
}

/// Fetch a cached status for a commit, if present.
pub fn get(store: &ChunkStore, commit: &str) -> Result<Option<CheckStatus>, String> {
    match store.get_check(commit)? {
        Some(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
        None => Ok(None),
    }
}

/// A one-char marker for the log: ✓ green / ✗ broken / · unknown.
pub fn marker(status: Option<&CheckStatus>) -> &'static str {
    match status {
        Some(s) if s.ok => "✓",
        Some(_) => "✗",
        None => "·",
    }
}

/// Commits with a cached passing check — the good test/run/pin candidates.
pub fn candidates(store: &ChunkStore) -> Result<Vec<CheckStatus>, String> {
    let mut out: Vec<CheckStatus> = store
        .list_checks()?
        .into_iter()
        .filter_map(|(_, b)| serde_json::from_slice::<CheckStatus>(&b).ok())
        .filter(|s| s.ok)
        .collect();
    out.sort_by(|a, b| b.ran_at.cmp(&a.ran_at));
    Ok(out)
}

/// The default check command (overridable on the CLI).
pub fn default_cmd() -> Vec<String> {
    vec!["cargo".to_string(), "check".to_string(), "--quiet".to_string()]
}

/// Materialize-and-run is unavailable when a worktree path is supplied; this
/// helper lets callers run a check against an already-present directory
/// (e.g. the live worktree) without going through the store.
pub fn run_in_dir(commit: &str, dir: &Path, cmd: &[String]) -> Result<CheckStatus, String> {
    let (prog, rest) = cmd.split_first().ok_or("empty check command")?;
    let start = std::time::Instant::now();
    let status = std::process::Command::new(prog)
        .args(rest)
        .current_dir(dir)
        .status()
        .map_err(|e| format!("check command failed to start: {e}"))?;
    Ok(CheckStatus {
        commit: commit.to_string(),
        ok: status.success(),
        code: status.code().unwrap_or(-1),
        duration_ms: start.elapsed().as_millis() as u64,
        ran_at: now_secs(),
        cmd: cmd.to_vec(),
    })
}
