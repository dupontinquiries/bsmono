//! `unendlich-launcher` — run `.unendtest` specs against arbitrary
//! git refs of the engine. The cross-version-compare foundation.
//!
//! # Architecture
//!
//! The launcher resolves a version (a git ref like `@HEAD`,
//! `@v0.24.2-c.7`, `@abc1234`, or `@my-branch`) to a commit hash via
//! `git rev-parse`. It checks a local cache at
//! `~/.unendlich/versions/<hash>/` for a built binary. On miss it
//! creates a fresh local clone of the engine at
//! `~/.unendlich/clones/<hash>/`, checks out the target commit, runs
//! `cargo build --release`, and copies the resulting binary into the
//! cache.
//!
//! Once a cached binary is available, the launcher creates a per-run
//! output directory at `~/.unendlich/runs/<run-uuid>/`. The binary is
//! spawned with `--test-spec <path-to-original-spec>` plus env vars
//! `UNENDLICH_RESULT_PATH` and `UNENDLICH_EVENTS_PATH` pointing into
//! the per-run dir. The spec on disk is never modified.
//!
//! # Why local clones (not git worktree)
//!
//! Worktrees share `.git/` with the main repo. Stale worktree
//! metadata can produce cryptic errors like
//! `fatal: 'main' is already checked out at...` in the main repo.
//! Recovery requires knowing `git worktree prune` and `git worktree
//! remove`, which most people don't.
//!
//! Local clones (`git clone --local <repo-path> <dst>`) make a fully
//! independent copy. They share git objects via hard-links (fast,
//! space-efficient) but have their own `.git/` and don't interfere
//! with the main repo. Cleanup is `rm -rf`. No worktree commands.
//!
//! # Driver awareness (c.11 captures, c.12 uses)
//!
//! GPU driver updates can shift perf numbers in ways that have
//! nothing to do with code changes. Every result file records the
//! driver identity (vendor/device IDs + driver version string). c.12
//! will filter cross-version comparisons by matching driver hash;
//! c.11 just makes sure the data is there.
//!
//! Without this, a c.5-vs-c.10 comparison after a Mesa upgrade would
//! conflate "engine changed" with "driver changed."
//!
//! # Concurrency
//!
//! c.11 is single-process. A second `unendlich-launcher run` against
//! the same uncached version would race on the cache directory.
//! Sketched mitigation (c.12): file-lock on `~/.unendlich/versions/<hash>/`
//! during build. For now, document "don't run two launchers at once
//! on the same version."
//!
//! # Not yet implemented (c.12+)
//!
//! - `compare` command (multi-version diff).
//! - `run-suite` command (test-all-specs button, driver-filtered).
//! - `gc` / `list` / `install` commands.
//! - Build concurrency lock.

use std::path::{Path, PathBuf};
use std::process::Command;

// ============================================================
// Errors
// ============================================================

#[derive(Debug)]
pub enum LauncherError {
    /// `~/.unendlich/` could not be created, or `$HOME` was unset.
    PathSetup(String),
    /// A subprocess (`git`, `cargo`) failed to spawn or exited with
    /// a non-zero status. Includes captured stderr.
    Subprocess { what: String, status: Option<i32>, stderr: String },
    /// I/O error while reading/writing files in the launcher's
    /// scratch directories.
    Io(String, std::io::Error),
    /// A version string didn't resolve to any git ref.
    UnknownVersion(String),
    /// The cached binary couldn't be found at the expected path
    /// after a successful build (build did something unexpected).
    BinaryNotFound(PathBuf),
    /// The binary that ran exited with a non-zero status. Status 1
    /// means assertions failed (still a "successful" run that
    /// produced a result file). Status 2 means the binary itself
    /// aborted before writing a result.
    BinaryFailed { status: Option<i32> },
}

impl std::fmt::Display for LauncherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LauncherError::PathSetup(msg) => write!(f, "could not set up launcher paths: {}", msg),
            LauncherError::Subprocess { what, status, stderr } => write!(
                f, "{} failed (status={:?}):\n{}", what, status, stderr,
            ),
            LauncherError::Io(path, e) => write!(f, "I/O error on {}: {}", path, e),
            LauncherError::UnknownVersion(v) => write!(f, "git couldn't resolve version: {}", v),
            LauncherError::BinaryNotFound(p) => write!(f, "expected binary not found at {}", p.display()),
            LauncherError::BinaryFailed { status } => write!(
                f, "binary exited with non-zero status: {:?}", status,
            ),
        }
    }
}

impl std::error::Error for LauncherError {}

// ============================================================
// Paths
// ============================================================

/// Root directory for everything the launcher manages. Defaults to
/// `$HOME/.unendlich`. Override via `UNENDLICH_LAUNCHER_HOME` (useful
/// for tests that mustn't touch the real `$HOME`).
pub fn launcher_home() -> Result<PathBuf, LauncherError> {
    if let Ok(p) = std::env::var("UNENDLICH_LAUNCHER_HOME") {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var("HOME").map_err(|_| {
        LauncherError::PathSetup("HOME env var not set".to_string())
    })?;
    Ok(PathBuf::from(home).join(".unendlich"))
}

/// Path to the cached binary for a given commit hash.
pub fn cached_binary_path(commit: &str) -> Result<PathBuf, LauncherError> {
    Ok(launcher_home()?.join("versions").join(commit).join("unendlich-world"))
}

/// Path to the local clone (working tree) used to build a given
/// commit. Lives outside the engine repo to avoid `.git/worktrees/`
/// pollution.
pub fn clone_path(commit: &str) -> Result<PathBuf, LauncherError> {
    Ok(launcher_home()?.join("clones").join(commit))
}

/// Path to the per-run output directory, keyed by run UUID.
/// Each invocation gets its own dir; the binary's result + events
/// files land here.
pub fn run_dir(run_uuid: &str) -> Result<PathBuf, LauncherError> {
    Ok(launcher_home()?.join("runs").join(run_uuid))
}

/// Ensure a directory exists, creating it (and parents) if not.
pub fn ensure_dir(p: &Path) -> Result<(), LauncherError> {
    std::fs::create_dir_all(p)
        .map_err(|e| LauncherError::Io(p.display().to_string(), e))
}

// ============================================================
// Git operations
// ============================================================

/// Resolve a version string (e.g. `@HEAD`, `@v0.24.2-c.7`, `@abc1234`,
/// `@my-branch`) to a full commit hash. The leading `@` is stripped.
/// Runs `git rev-parse --verify` in the supplied repo directory; an
/// unknown or ambiguous ref returns `UnknownVersion`.
pub fn resolve_version(repo_dir: &Path, version: &str)
    -> Result<String, LauncherError>
{
    let ref_str = version.strip_prefix('@').unwrap_or(version);
    let output = Command::new("git")
        .arg("-C").arg(repo_dir)
        .arg("rev-parse")
        .arg("--verify")
        .arg(ref_str)
        .output()
        .map_err(|e| LauncherError::Subprocess {
            what: format!("git rev-parse {}", ref_str),
            status: None,
            stderr: format!("could not invoke git: {}", e),
        })?;
    if !output.status.success() {
        // `git rev-parse --verify` exits non-zero for unknown refs
        // with a useful stderr message. Translate to UnknownVersion
        // so callers can give a friendly error.
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let is_unknown_ref = stderr.contains("Needed a single revision")
            || stderr.contains("unknown revision")
            || stderr.contains("ambiguous argument")
            || stderr.contains("bad revision")
            || stderr.contains("not a tree")
            || stderr.contains("malformed object name");
        if is_unknown_ref {
            return Err(LauncherError::UnknownVersion(version.to_string()));
        }
        return Err(LauncherError::Subprocess {
            what: format!("git rev-parse {}", ref_str),
            status: output.status.code(),
            stderr,
        });
    }
    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if hash.is_empty() {
        return Err(LauncherError::UnknownVersion(version.to_string()));
    }
    Ok(hash)
}

/// Make a local clone of the source repo at `dst`, checked out to
/// `commit`. Uses `git clone --local` which hard-links the git
/// object database to the source repo for speed and space.
///
/// If `dst` already exists, returns Ok without re-cloning. (Same
/// commit hash → same source state, no rebuild needed.)
pub fn ensure_clone(source_repo: &Path, commit: &str, dst: &Path)
    -> Result<(), LauncherError>
{
    if dst.exists() {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        ensure_dir(parent)?;
    }

    let output = Command::new("git")
        .arg("clone")
        .arg("--local")
        .arg("--no-hardlinks") // safer cross-filesystem; can revisit.
        .arg(source_repo)
        .arg(dst)
        .output()
        .map_err(|e| LauncherError::Subprocess {
            what: "git clone".to_string(),
            status: None,
            stderr: format!("could not invoke git: {}", e),
        })?;
    if !output.status.success() {
        return Err(LauncherError::Subprocess {
            what: format!("git clone {} → {}", source_repo.display(), dst.display()),
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    // Detach to the specific commit. This handles all ref types
    // uniformly: HEAD, tags, branches, raw hashes all check out to
    // a detached HEAD pointing at the resolved commit.
    let output = Command::new("git")
        .arg("-C").arg(dst)
        .arg("checkout")
        .arg("--detach")
        .arg(commit)
        .output()
        .map_err(|e| LauncherError::Subprocess {
            what: format!("git checkout {}", commit),
            status: None,
            stderr: format!("could not invoke git: {}", e),
        })?;
    if !output.status.success() {
        return Err(LauncherError::Subprocess {
            what: format!("git checkout {}", commit),
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(())
}

// ============================================================
// Build
// ============================================================

/// Build the `unendlich-world` binary in a cloned source tree and
/// copy it into the cache at `cached_binary_path(commit)`.
///
/// Features used are the ones the binary requires for normal play:
/// `render`, `gpu`, `hot-shaders`. Hardcoded here intentionally —
/// this is the canonical "playtest binary" build.
pub fn build_and_cache(clone_dir: &Path, commit: &str)
    -> Result<PathBuf, LauncherError>
{
    let cache_dst = cached_binary_path(commit)?;

    // Cache hit: already built, no work to do.
    if cache_dst.exists() {
        return Ok(cache_dst);
    }

    let output = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .arg("--bin").arg("unendlich-world")
        .arg("--features").arg("render")
        .arg("--features").arg("gpu")
        .arg("--features").arg("hot-shaders")
        .current_dir(clone_dir)
        .output()
        .map_err(|e| LauncherError::Subprocess {
            what: "cargo build".to_string(),
            status: None,
            stderr: format!("could not invoke cargo: {}", e),
        })?;
    if !output.status.success() {
        return Err(LauncherError::Subprocess {
            what: format!("cargo build (in {})", clone_dir.display()),
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let built_binary = clone_dir.join("target/release/unendlich-world");
    if !built_binary.exists() {
        return Err(LauncherError::BinaryNotFound(built_binary));
    }

    if let Some(parent) = cache_dst.parent() {
        ensure_dir(parent)?;
    }
    std::fs::copy(&built_binary, &cache_dst)
        .map_err(|e| LauncherError::Io(cache_dst.display().to_string(), e))?;

    Ok(cache_dst)
}

// ============================================================
// Run
// ============================================================

/// Outcome of running a spec via the launcher.
#[derive(Debug)]
pub struct RunOutcome {
    /// Path to the `.unendresult.json` the binary wrote.
    pub result_path: PathBuf,
    /// Path to the `.events.jsonl` the binary wrote. May have been
    /// deleted by post-run triage if the run was healthy — see
    /// `triage` field. Use `triage.events_kept` to check.
    pub events_path: PathBuf,
    /// c.13-instrumentation-followup: path to the captured engine
    /// stderr log. Always written, even on clean runs (cheap, and
    /// it's the first thing you want when something goes wrong).
    pub engine_log_path: PathBuf,
    /// Exit status the binary returned (0 = pass, 1 = assertion fail,
    /// 2 = run aborted).
    pub exit_status: Option<i32>,
    /// Whether the result was a clean assertion pass.
    pub assertions_passed: bool,
    /// c.12-e: post-run triage verdict. None if triage couldn't run
    /// (e.g. events file missing).
    pub triage: Option<TriageSummary>,
}

/// c.12-e: post-run triage of a run's events.jsonl.
///
/// Computes per-1Hz-window p50 frame-time from `frame_window` events
/// in `mode: "steady"`, and decides whether to keep or delete the
/// events log:
///
/// - **All windows healthy** (p50 < threshold_ms in every steady window):
///   events.jsonl gets deleted to save disk. The result.unendresult.json
///   alone is enough.
/// - **At least one bad window**: events.jsonl is kept and an
///   `incident_summary.json` is written alongside, recording which
///   windows triggered.
///
/// Threshold: configurable via env var `UNENDLICH_TRIAGE_THRESHOLD_MS`
/// (default 33ms = ~30fps p50). Boot-mode events are excluded — early
/// warmup is expected to be slow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TriageSummary {
    /// Threshold used for the decision.
    pub threshold_ms: f64,
    /// Number of "steady" 1Hz windows examined.
    pub steady_windows: usize,
    /// Worst (highest) p50_ms across all steady windows.
    pub worst_p50_ms: f64,
    /// Whether any window exceeded the threshold.
    pub had_incident: bool,
    /// Whether the events file was kept (true) or deleted (false).
    pub events_kept: bool,
}

/// Threshold defaults. Configurable via env var.
fn triage_threshold_ms() -> f64 {
    std::env::var("UNENDLICH_TRIAGE_THRESHOLD_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(33.0)  // ~30fps p50
}

/// Read events.jsonl, examine "steady" frame_window events, decide
/// keep-or-delete. Writes incident_summary.json next to the events
/// file (always, regardless of verdict — useful even for clean runs
/// so the analyzer can confirm "yes, this was actually triaged").
fn run_triage(events_path: &Path) -> Option<TriageSummary> {
    let contents = std::fs::read_to_string(events_path).ok()?;
    let threshold_ms = triage_threshold_ms();

    let mut steady_windows = 0usize;
    let mut worst_p50_ms: f64 = 0.0;
    let mut had_incident = false;

    for line in contents.lines() {
        if line.trim().is_empty() { continue; }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        // Only consider frame_window events in steady mode.
        let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        if kind != "frame_window" { continue; }
        let payload = match v.get("payload") {
            Some(p) => p,
            None => continue,
        };
        let mode = payload.get("mode").and_then(|m| m.as_str()).unwrap_or("");
        if mode != "steady" { continue; }
        let p50 = payload.get("frame_p50_ms").and_then(|v| v.as_f64()).unwrap_or(0.0);
        steady_windows += 1;
        if p50 > worst_p50_ms { worst_p50_ms = p50; }
        if p50 > threshold_ms { had_incident = true; }
    }

    // If no steady windows at all, the run was too short to triage —
    // keep the events to be safe.
    let events_kept = had_incident || steady_windows == 0;

    // Delete events file if we judged it healthy.
    if !events_kept {
        let _ = std::fs::remove_file(events_path);
    }

    let summary = TriageSummary {
        threshold_ms,
        steady_windows,
        worst_p50_ms,
        had_incident,
        events_kept,
    };

    // Always write the summary, so the triage outcome is preserved.
    if let Some(parent) = events_path.parent() {
        let summary_path = parent.join("incident_summary.json");
        let _ = std::fs::write(
            &summary_path,
            serde_json::to_string_pretty(&summary).unwrap_or_default(),
        );
    }

    Some(summary)
}

pub fn run_spec(binary: &Path, spec_path: &Path)
    -> Result<RunOutcome, LauncherError>
{
    let run_uuid = unendlich_shared::run_id::fresh();
    let outdir = run_dir(&run_uuid)?;
    ensure_dir(&outdir)?;
    let result_path = outdir.join("result.unendresult.json");
    let events_path = outdir.join("events.jsonl");
    // c.13-instrumentation-followup: capture the engine's stderr to a
    // file in the run directory so the full log is recoverable after
    // the fact. Today engines tee through to the controlling terminal;
    // if the user piped or scrolled away, the log is gone. Saving
    // engine.log into the run dir means it's always there.
    let engine_log_path = outdir.join("engine.log");

    // The spec on disk is never modified. We tell the binary where
    // to write outputs via env vars; the spec's `[output]` table is
    // overridden by these (see `test_spec::LoadedSpec::result_path`).
    //
    // Stderr is captured to engine.log. Stdout still flows through
    // to the user's terminal (the engine prints minimal stdout — most
    // logs go to stderr via env_logger). That keeps the existing UX
    // (live progress visible) while archiving the full log.
    let engine_log_file = std::fs::File::create(&engine_log_path)
        .map_err(|e| LauncherError::Subprocess {
            what: format!("create engine.log at {}", engine_log_path.display()),
            status: None,
            stderr: e.to_string(),
        })?;
    // We use `tee`-via-thread to keep stderr live AND capture it.
    // Spawning the binary with .stderr(Stdio::piped()) and reading
    // from a thread is the standard pattern.
    use std::process::Stdio;
    use std::io::{BufRead, BufReader, Write};
    let mut child = Command::new(binary)
        .arg("--test-spec").arg(spec_path)
        .env("UNENDLICH_RESULT_PATH", &result_path)
        .env("UNENDLICH_EVENTS_PATH", &events_path)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| LauncherError::Subprocess {
            what: format!("spawn {}", binary.display()),
            status: None,
            stderr: format!("could not invoke binary: {}", e),
        })?;

    // Tee the child's stderr: every line goes both to our stderr (so
    // the user still sees progress) and to engine.log (so we have it
    // archived).
    let child_stderr = child.stderr.take().expect("piped stderr");
    let tee_thread = std::thread::spawn(move || {
        let mut log = engine_log_file;
        let reader = BufReader::new(child_stderr);
        for line in reader.lines().map_while(Result::ok) {
            eprintln!("{}", line);
            let _ = writeln!(log, "{}", line);
        }
        let _ = log.flush();
    });

    let status = child.wait()
        .map_err(|e| LauncherError::Subprocess {
            what: format!("wait on {}", binary.display()),
            status: None,
            stderr: format!("could not wait for child: {}", e),
        })?;
    let _ = tee_thread.join();

    let exit_status = status.code();
    let assertions_passed = exit_status == Some(0);

    // c.12-e: post-run triage. Decides whether to keep the events.jsonl
    // for forensics (slow run) or delete it (clean run). Always writes
    // incident_summary.json so the triage decision is preserved.
    let triage = run_triage(&events_path);

    // c.13-instrumentation-followup: update `latest-*` symlinks in
    // ~/.unendlich so copy-paste commands can reference a stable
    // path. Best-effort: failures are logged but not fatal (e.g.
    // a non-symlink-supporting filesystem, although we don't expect
    // those on Linux/macOS where unendlich runs).
    update_latest_symlinks(&result_path, &events_path, &engine_log_path,
        spec_path, &triage);

    Ok(RunOutcome {
        result_path,
        events_path,
        engine_log_path,
        exit_status,
        assertions_passed,
        triage,
    })
}

/// Update `~/.unendlich/latest-*` symlinks to point at the most
/// recent run. Three global symlinks:
///   latest-result.unendresult.json
///   latest-events.jsonl
///   latest-engine.log
/// Plus a per-spec symlink:
///   latest-{spec_name}.unendresult.json
///
/// "Per-spec" means: if you alternate `rd2_smoke` and `rd8_smoke`
/// runs, the global `latest-*` points at whichever ran last, but
/// `latest-rd2_smoke.unendresult.json` always points at the latest
/// rd2_smoke run. Useful for `run-suite` where the global latest
/// only reflects the last spec in the suite.
///
/// Best-effort. Errors are logged but don't fail the run.
///
/// For runs where triage deleted events.jsonl (healthy steady runs),
/// the events symlink is removed rather than left pointing at a
/// nonexistent path.
fn update_latest_symlinks(
    result_path: &Path,
    events_path: &Path,
    engine_log_path: &Path,
    spec_path: &Path,
    triage: &Option<TriageSummary>,
) {
    let unendlich_root = match unendlich_root_dir() {
        Ok(p) => p,
        Err(_) => return,
    };

    // Helper: atomically replace `link` to point at `target`. On Linux
    // this is "unlink then symlink"; the small race is acceptable for
    // a developer-convenience feature.
    let relink = |link: &Path, target: &Path| {
        // Use absolute paths in the symlink. Relative paths from the
        // ~/.unendlich/ root would work too, but absolute is robust
        // against future reorganization.
        let _ = std::fs::remove_file(link);
        #[cfg(unix)]
        {
            if let Err(e) = std::os::unix::fs::symlink(target, link) {
                log::warn!("Could not symlink {} -> {}: {}",
                    link.display(), target.display(), e);
            }
        }
        #[cfg(not(unix))]
        {
            log::debug!("Symlinks not supported on this platform; \
                        skipping {} -> {}", link.display(), target.display());
        }
    };

    relink(&unendlich_root.join("latest-result.unendresult.json"), result_path);
    relink(&unendlich_root.join("latest-engine.log"), engine_log_path);

    // Events symlink: only if events.jsonl still exists (triage may
    // have deleted it for clean runs). Remove the symlink if not.
    let events_link = unendlich_root.join("latest-events.jsonl");
    if events_path.exists() {
        relink(&events_link, events_path);
    } else {
        let _ = std::fs::remove_file(&events_link);
    }

    // Per-spec symlink. Use the spec's file stem (rd8_smoke.unendtest
    // → "rd8_smoke") as the suffix. Falls back to "unknown" if the
    // path has no stem (shouldn't happen for a valid run).
    let spec_name = spec_path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let per_spec_link = unendlich_root.join(
        format!("latest-{}.unendresult.json", spec_name));
    relink(&per_spec_link, result_path);

    // Note: incident-detection from triage isn't used here — the
    // symlinks always point at the latest run regardless of health.
    // If you want to find the latest incident specifically, look at
    // events.jsonl on the latest-events.jsonl symlink (only kept
    // for short or unhealthy runs).
    let _ = triage;
}

/// Resolve `~/.unendlich/` as an absolute path.
fn unendlich_root_dir() -> Result<std::path::PathBuf, LauncherError> {
    let home = std::env::var("HOME")
        .map_err(|_| LauncherError::Subprocess {
            what: "resolve $HOME".to_string(),
            status: None,
            stderr: "HOME env var not set".to_string(),
        })?;
    Ok(std::path::PathBuf::from(home).join(".unendlich"))
}

// ============================================================
// End-to-end
// ============================================================

/// Top-level `run` operation: given a source repo, a version
/// (e.g. `@HEAD`), and a spec path, produce a result file.
///
/// 1. Resolve the version to a commit hash.
/// 2. Ensure a built binary is cached for that commit (clone +
///    build if not).
/// 3. Run the spec against the cached binary in a per-run output dir.
pub fn run(source_repo: &Path, version: &str, spec_path: &Path)
    -> Result<(String, RunOutcome), LauncherError>
{
    let commit = resolve_version(source_repo, version)?;
    let clone_dir = clone_path(&commit)?;
    ensure_clone(source_repo, &commit, &clone_dir)?;
    let binary = build_and_cache(&clone_dir, &commit)?;
    let outcome = run_spec(&binary, spec_path)?;
    Ok((commit, outcome))
}

// ============================================================
// compare — multi-version result diff
// ============================================================

/// A column in a comparison table: one labelled source (typically a
/// version ref or a file path) with its result file's key metrics.
#[derive(Debug, Clone)]
pub struct CompareColumn {
    /// Human label for this column: a version ref ("@HEAD",
    /// "@v0.24.2-c.7"), an absolute file path, or whatever the
    /// caller chose for the headline.
    pub label: String,
    /// Parsed result. None means we couldn't read the file (file
    /// missing, JSON malformed). The compare table renders these
    /// as "—".
    pub result: Option<unendlich_shared::test_spec::TestResult>,
}

/// Row of the compare table: one metric name, value per column.
#[derive(Debug, Clone)]
pub struct CompareRow {
    /// Metric name as it appears in the result's `stats` map. May be
    /// dotted (e.g. `gpu_timer_p50.shadow`).
    pub metric: String,
    /// One entry per column, in input order. `None` = missing in
    /// that column's result.
    pub values: Vec<Option<serde_json::Value>>,
}

/// Output of `compare` — a structured comparison ready to be
/// rendered. Pure data, no formatting concerns. Easy to test.
#[derive(Debug, Clone)]
pub struct CompareReport {
    pub columns: Vec<CompareColumn>,
    pub rows: Vec<CompareRow>,
    /// Driver-hash row (always present even if all empty). If the
    /// hashes differ across columns, the renderer will print a
    /// warning. We model this as a regular row so it slots into the
    /// same table format.
    pub driver_hashes: Vec<Option<String>>,
    /// True if any pair of present driver hashes differ. The
    /// renderer prints a "⚠ driver epoch differs" warning.
    pub driver_mismatch: bool,
    /// c.13-scout: multiview-csm-capable flag per column (None if no
    /// gpu_info or no multiview field in the result). If any pair of
    /// present values differ, the renderer surfaces it.
    pub multiview_csm: Vec<Option<bool>>,
    /// Whether each result's assertions passed.
    pub assertion_pass: Vec<Option<bool>>,
}

/// Build a comparison from a slice of columns. Pure: no I/O. The
/// caller is responsible for reading result files into the
/// `CompareColumn::result` field.
///
/// Metric selection: by default we surface the *union* of metric
/// keys across all present results. The caller can post-filter via
/// `keep_metrics` to limit to specific keys (e.g., only `*_p50_ms`
/// metrics).
///
/// Metric ordering: alphabetic. Deterministic for test-friendliness.
pub fn build_compare_report(
    columns: Vec<CompareColumn>,
    keep_metrics: Option<&[&str]>,
) -> CompareReport {
    // Collect the union of metric keys across present results.
    let mut metric_set: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    for col in &columns {
        if let Some(r) = &col.result {
            for k in r.stats.keys() {
                if let Some(filter) = keep_metrics {
                    if !filter.iter().any(|f| f == k) { continue; }
                }
                metric_set.insert(k.clone());
            }
        }
    }

    // Build rows.
    let rows: Vec<CompareRow> = metric_set.iter().map(|m| {
        let values: Vec<Option<serde_json::Value>> = columns.iter().map(|col| {
            col.result.as_ref().and_then(|r| r.stats.get(m).cloned())
        }).collect();
        CompareRow { metric: m.clone(), values }
    }).collect();

    // Driver hashes.
    let driver_hashes: Vec<Option<String>> = columns.iter().map(|col| {
        col.result.as_ref()
            .and_then(|r| r.gpu_info.as_ref())
            .map(|g| g.driver_hash.clone())
    }).collect();

    // Mismatch: any two non-None hashes that differ.
    let mut driver_mismatch = false;
    let present: Vec<&String> = driver_hashes.iter().flatten().collect();
    if present.len() >= 2 {
        let first = present[0];
        driver_mismatch = present.iter().any(|h| *h != first);
    }

    // c.13-scout: per-column multiview-csm capability. None if the
    // result file predates the field or has no gpu_info.
    let multiview_csm: Vec<Option<bool>> = columns.iter().map(|col| {
        col.result.as_ref()
            .and_then(|r| r.gpu_info.as_ref())
            .map(|g| g.multiview.supports_csm())
    }).collect();

    // Assertion pass per column.
    let assertion_pass: Vec<Option<bool>> = columns.iter().map(|col| {
        col.result.as_ref().map(|r| r.passed)
    }).collect();

    CompareReport {
        columns, rows,
        driver_hashes, driver_mismatch,
        multiview_csm,
        assertion_pass,
    }
}

/// Render a CompareReport as plain text aligned for terminals.
/// Renderer is pure: produces a String. Callers print it.
pub fn render_compare_report(report: &CompareReport) -> String {
    use std::fmt::Write;

    if report.columns.is_empty() {
        return "(no columns to compare)\n".to_string();
    }

    // Column widths.
    let metric_col_width = report.rows.iter()
        .map(|r| r.metric.len())
        .chain(std::iter::once("Metric".len()))
        .chain(std::iter::once("driver_hash".len()))
        .chain(std::iter::once("assertions".len()))
        .max()
        .unwrap_or(8);

    let value_widths: Vec<usize> = report.columns.iter().enumerate().map(|(i, col)| {
        let mut w = col.label.len();
        for row in &report.rows {
            if let Some(v) = &row.values[i] {
                w = w.max(format_value_short(v).len());
            } else {
                w = w.max(1); // "—"
            }
        }
        if let Some(h) = &report.driver_hashes[i] {
            w = w.max(h.len());
        }
        w.max("PASS".len())
    }).collect();

    let mut out = String::new();

    // Header.
    write!(out, "{:<w$}", "Metric", w = metric_col_width).unwrap();
    for (i, col) in report.columns.iter().enumerate() {
        write!(out, " | {:<w$}", col.label, w = value_widths[i]).unwrap();
    }
    out.push('\n');

    // Separator.
    write!(out, "{:-<w$}", "", w = metric_col_width).unwrap();
    for w in &value_widths {
        out.push_str("-+-");
        write!(out, "{:-<w$}", "", w = w).unwrap();
    }
    out.push('\n');

    // Data rows.
    for row in &report.rows {
        write!(out, "{:<w$}", row.metric, w = metric_col_width).unwrap();
        for (i, v) in row.values.iter().enumerate() {
            let s = v.as_ref().map(format_value_short).unwrap_or_else(|| "—".to_string());
            write!(out, " | {:<w$}", s, w = value_widths[i]).unwrap();
        }
        out.push('\n');
    }

    // Driver hash row.
    write!(out, "{:-<w$}", "", w = metric_col_width).unwrap();
    for w in &value_widths {
        out.push_str("-+-");
        write!(out, "{:-<w$}", "", w = w).unwrap();
    }
    out.push('\n');
    write!(out, "{:<w$}", "driver_hash", w = metric_col_width).unwrap();
    for (i, h) in report.driver_hashes.iter().enumerate() {
        let s = h.as_deref().unwrap_or("—");
        write!(out, " | {:<w$}", s, w = value_widths[i]).unwrap();
    }
    out.push('\n');

    // c.13-scout: multiview-csm-capable row. Only rendered if at least
    // one column has the info — older result files predate the field.
    if report.multiview_csm.iter().any(|m| m.is_some()) {
        write!(out, "{:<w$}", "csm_capable", w = metric_col_width).unwrap();
        for (i, m) in report.multiview_csm.iter().enumerate() {
            let s = match m {
                Some(true) => "yes",
                Some(false) => "no",
                None => "—",
            };
            write!(out, " | {:<w$}", s, w = value_widths[i]).unwrap();
        }
        out.push('\n');
    }

    // Assertion row.
    write!(out, "{:<w$}", "assertions", w = metric_col_width).unwrap();
    for (i, a) in report.assertion_pass.iter().enumerate() {
        let s = match a {
            Some(true) => "PASS",
            Some(false) => "FAIL",
            None => "—",
        };
        write!(out, " | {:<w$}", s, w = value_widths[i]).unwrap();
    }
    out.push('\n');

    // Warning.
    if report.driver_mismatch {
        out.push('\n');
        out.push_str("⚠  Driver hashes differ across compared results. Cross-version\n");
        out.push_str("   perf comparisons across a driver epoch boundary may be\n");
        out.push_str("   misleading. Consider re-running the older versions under\n");
        out.push_str("   the current driver to fill gaps within a single epoch.\n");
    }

    out
}

/// Format a serde_json value compactly for table cells.
fn format_value_short(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                // Compact float: 3 sig figs typically.
                if f.abs() >= 100.0 {
                    format!("{:.1}", f)
                } else if f.abs() >= 1.0 {
                    format!("{:.2}", f)
                } else {
                    format!("{:.3}", f)
                }
            } else {
                n.to_string()
            }
        }
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "—".to_string(),
        // Objects/arrays: a one-line summary.
        serde_json::Value::Object(o) => format!("{{...{} keys}}", o.len()),
        serde_json::Value::Array(a) => format!("[...{} items]", a.len()),
    }
}

/// Load a result file from disk. Convenience wrapper for the CLI.
pub fn load_result(path: &Path)
    -> Result<unendlich_shared::test_spec::TestResult, LauncherError>
{
    let contents = std::fs::read_to_string(path)
        .map_err(|e| LauncherError::Io(path.display().to_string(), e))?;
    serde_json::from_str(&contents)
        .map_err(|e| LauncherError::Io(
            path.display().to_string(),
            std::io::Error::new(std::io::ErrorKind::InvalidData,
                format!("could not parse result JSON: {}", e)),
        ))
}

// ============================================================
// run-suite — iterate over a directory of specs
// ============================================================

/// One spec's outcome within a suite run.
#[derive(Debug, Clone)]
pub struct SuiteOutcome {
    pub spec_name: String,
    pub spec_path: PathBuf,
    /// None = launcher couldn't run the spec at all (build failed,
    /// binary aborted before writing result). Some(true/false) =
    /// ran successfully, with that pass/fail.
    pub assertions_passed: Option<bool>,
    pub result_path: Option<PathBuf>,
    pub events_path: Option<PathBuf>,
    pub error: Option<String>,
}

/// Discover all `.unendtest` specs under a directory. Sorted by name
/// for deterministic ordering.
pub fn discover_specs(dir: &Path) -> Result<Vec<PathBuf>, LauncherError> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| LauncherError::Io(dir.display().to_string(), e))?;
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| LauncherError::Io(
            dir.display().to_string(), e))?;
        let p = entry.path();
        if p.is_file() {
            if let Some(ext) = p.extension() {
                if ext == "unendtest" {
                    paths.push(p);
                }
            }
        }
    }
    paths.sort();
    Ok(paths)
}

/// Run every spec in `specs_dir` against `version`, return a per-spec
/// outcome summary.
///
/// Failures are caught — a spec that fails to run doesn't abort the
/// suite. The caller sees an error string in that spec's outcome.
pub fn run_suite(
    source_repo: &Path,
    version: &str,
    specs_dir: &Path,
) -> Result<(String, Vec<SuiteOutcome>), LauncherError> {
    // Resolve the version ONCE so we don't run git rev-parse per spec.
    let commit = resolve_version(source_repo, version)?;
    let clone_dir = clone_path(&commit)?;
    ensure_clone(source_repo, &commit, &clone_dir)?;
    let binary = build_and_cache(&clone_dir, &commit)?;

    let specs = discover_specs(specs_dir)?;

    let mut outcomes: Vec<SuiteOutcome> = Vec::new();
    for spec_path in specs {
        let spec_name = spec_path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "?".to_string());

        match run_spec(&binary, &spec_path) {
            Ok(outcome) => {
                outcomes.push(SuiteOutcome {
                    spec_name,
                    spec_path,
                    assertions_passed: Some(outcome.assertions_passed),
                    result_path: Some(outcome.result_path),
                    events_path: Some(outcome.events_path),
                    error: None,
                });
            }
            Err(e) => {
                outcomes.push(SuiteOutcome {
                    spec_name,
                    spec_path,
                    assertions_passed: None,
                    result_path: None,
                    events_path: None,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    Ok((commit, outcomes))
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Test scoping: each test gets its own fake `$HOME` so it can
    /// exercise the path helpers without touching the real home dir.
    /// We use `UNENDLICH_LAUNCHER_HOME` to avoid needing to also
    /// modify `$HOME` (which the test runner relies on for many
    /// things).
    fn scoped_home(label: &str) -> (PathBuf, EnvGuard) {
        let tmp = std::env::temp_dir().join(format!(
            "unendlich-launcher-test-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        std::env::set_var("UNENDLICH_LAUNCHER_HOME", &tmp);
        (tmp, EnvGuard)
    }

    struct EnvGuard;
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("UNENDLICH_LAUNCHER_HOME");
        }
    }

    #[test]
    fn launcher_home_uses_override_env() {
        let (tmp, _g) = scoped_home("home_override");
        assert_eq!(launcher_home().unwrap(), tmp);
    }

    #[test]
    fn cached_binary_path_is_under_versions() {
        let (tmp, _g) = scoped_home("cached_path");
        let p = cached_binary_path("abc1234").unwrap();
        assert!(p.starts_with(&tmp));
        assert!(p.ends_with("versions/abc1234/unendlich-world"));
    }

    #[test]
    fn clone_path_is_under_clones() {
        let (tmp, _g) = scoped_home("clone_path");
        let p = clone_path("abc1234").unwrap();
        assert!(p.starts_with(&tmp));
        assert!(p.ends_with("clones/abc1234"));
    }

    #[test]
    fn run_dir_is_under_runs() {
        let (tmp, _g) = scoped_home("run_dir");
        let p = run_dir("r-0123456789abcdef").unwrap();
        assert!(p.starts_with(&tmp));
        assert!(p.ends_with("runs/r-0123456789abcdef"));
    }

    #[test]
    fn ensure_dir_creates_and_idempotent() {
        let (tmp, _g) = scoped_home("ensure_dir");
        let deep = tmp.join("a/b/c");
        ensure_dir(&deep).expect("first create");
        assert!(deep.exists());
        ensure_dir(&deep).expect("second create (idempotent)");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `@HEAD` should resolve to a commit hash in any git repo. We
    /// run against the current working dir (which is the engine
    /// repo when tests are invoked from cargo). If that's not a git
    /// repo (some CI configurations), skip.
    #[test]
    fn resolve_version_at_head_returns_hash_in_real_repo() {
        // Sniff first: is `git rev-parse --git-dir` happy in CWD?
        let probe = Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .output();
        let in_git = match probe {
            Ok(o) => o.status.success(),
            Err(_) => false,
        };
        if !in_git {
            eprintln!("Skipping resolve_version test — not in a git repo");
            return;
        }
        let cwd = std::env::current_dir().unwrap();
        let h = resolve_version(&cwd, "@HEAD").expect("resolve HEAD");
        // Full SHA-1 is 40 hex chars; some configs may shorten — but
        // `rev-parse --verify HEAD` returns the full hash.
        assert_eq!(h.len(), 40, "expected 40-char hash, got {:?}", h);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// Unknown refs should produce UnknownVersion, not a generic
    /// subprocess error.
    #[test]
    fn resolve_version_unknown_returns_typed_error() {
        let probe = Command::new("git").args(["rev-parse", "--git-dir"]).output();
        let in_git = matches!(probe, Ok(o) if o.status.success());
        if !in_git { return; }
        let cwd = std::env::current_dir().unwrap();
        let err = resolve_version(&cwd, "@this-ref-does-not-exist-123abc").unwrap_err();
        match err {
            LauncherError::UnknownVersion(_) => {}
            other => panic!("expected UnknownVersion, got {:?}", other),
        }
    }

    // ============================================================
    // compare / run-suite tests (pure, no I/O for compare logic)
    // ============================================================

    /// Synthesize a TestResult for tests. Reduces boilerplate.
    fn make_result(
        label: &str,
        stats: &[(&str, f64)],
        driver_hash: Option<&str>,
        passed: bool,
    ) -> unendlich_shared::test_spec::TestResult {
        use unendlich_shared::test_spec::*;
        let mut stats_map = serde_json::Map::new();
        for (k, v) in stats {
            stats_map.insert(k.to_string(),
                serde_json::Value::from(*v));
        }
        let gpu_info = driver_hash.map(|h| GpuInfo {
            vendor_id: 0x106B,
            device_id: 0x1024,
            driver_version_raw: 0x00400000,
            driver_name: "Asahi".to_string(),
            driver_info: format!("info-{}", h),
            driver_hash: h.to_string(),
            multiview: Default::default(),
        });
        TestResult {
            spec_version: 1,
            spec_path: format!("/tmp/{}.unendtest", label),
            spec_name: label.to_string(),
            binary_version: "0.24.2-c.12-b".to_string(),
            binary: "unendlich-world".to_string(),
            run_uuid: format!("r-{:016x}", label.len() as u64),
            ran_at_utc: "2026-05-12T00:00:00Z".to_string(),
            duration_secs: 5.0,
            shutdown_reason: "max_duration".to_string(),
            env: std::collections::BTreeMap::new(),
            gpu_info,
            stats: stats_map,
            assertions: vec![],
            passed,
        }
    }

    /// Compare with one column: trivially the same row values.
    #[test]
    fn compare_single_column_renders_each_metric() {
        let cols = vec![CompareColumn {
            label: "@HEAD".to_string(),
            result: Some(make_result("rd2_smoke",
                &[("shadow_p50_ms", 1.5), ("acquire_p50_ms", 0.02)],
                Some("drv-abc"), true)),
        }];
        let report = build_compare_report(cols, None);
        assert_eq!(report.rows.len(), 2);
        assert_eq!(report.rows[0].metric, "acquire_p50_ms");
        assert_eq!(report.rows[1].metric, "shadow_p50_ms");
        assert_eq!(report.driver_hashes, vec![Some("drv-abc".to_string())]);
        assert!(!report.driver_mismatch);
        assert_eq!(report.assertion_pass, vec![Some(true)]);
    }

    /// Two columns, same driver hash: no mismatch warning.
    #[test]
    fn compare_two_columns_same_driver_no_warning() {
        let cols = vec![
            CompareColumn {
                label: "@v0.24.2-c.10".to_string(),
                result: Some(make_result("rd2_smoke",
                    &[("shadow_p50_ms", 1.5)], Some("drv-abc"), true)),
            },
            CompareColumn {
                label: "@HEAD".to_string(),
                result: Some(make_result("rd2_smoke",
                    &[("shadow_p50_ms", 1.2)], Some("drv-abc"), true)),
            },
        ];
        let report = build_compare_report(cols, None);
        assert!(!report.driver_mismatch);
    }

    /// Two columns, different driver hashes: mismatch warning.
    #[test]
    fn compare_two_columns_different_drivers_triggers_warning() {
        let cols = vec![
            CompareColumn {
                label: "@v0.24.2-c.5".to_string(),
                result: Some(make_result("rd2_smoke",
                    &[("shadow_p50_ms", 1.5)], Some("drv-old"), true)),
            },
            CompareColumn {
                label: "@HEAD".to_string(),
                result: Some(make_result("rd2_smoke",
                    &[("shadow_p50_ms", 1.2)], Some("drv-new"), true)),
            },
        ];
        let report = build_compare_report(cols, None);
        assert!(report.driver_mismatch);
        let rendered = render_compare_report(&report);
        assert!(rendered.contains("Driver hashes differ"),
            "rendered output should warn:\n{}", rendered);
    }

    /// One result missing gpu_info (e.g. an old c.11 result file).
    /// Should NOT trigger a mismatch warning — only Some-vs-Some
    /// comparisons can mismatch.
    #[test]
    fn compare_missing_gpu_info_does_not_warn() {
        let cols = vec![
            CompareColumn {
                label: "@v0.24.2-c.11".to_string(),
                result: Some(make_result("rd2_smoke",
                    &[("shadow_p50_ms", 1.5)], None, true)),
            },
            CompareColumn {
                label: "@HEAD".to_string(),
                result: Some(make_result("rd2_smoke",
                    &[("shadow_p50_ms", 1.2)], Some("drv-new"), true)),
            },
        ];
        let report = build_compare_report(cols, None);
        assert!(!report.driver_mismatch,
            "one None driver_hash + one Some shouldn't mismatch");
    }

    /// Metrics present in one column but not another: row exists,
    /// other column shows None.
    #[test]
    fn compare_metric_only_in_one_column() {
        let cols = vec![
            CompareColumn {
                label: "@a".to_string(),
                result: Some(make_result("rd2_smoke",
                    &[("shadow_p50_ms", 1.5)], Some("drv-x"), true)),
            },
            CompareColumn {
                label: "@b".to_string(),
                result: Some(make_result("rd2_smoke",
                    &[("new_metric_ms", 0.3)], Some("drv-x"), true)),
            },
        ];
        let report = build_compare_report(cols, None);
        assert_eq!(report.rows.len(), 2);
        let shadow_row = report.rows.iter()
            .find(|r| r.metric == "shadow_p50_ms").unwrap();
        assert!(shadow_row.values[0].is_some());
        assert!(shadow_row.values[1].is_none());
        let new_row = report.rows.iter()
            .find(|r| r.metric == "new_metric_ms").unwrap();
        assert!(new_row.values[0].is_none());
        assert!(new_row.values[1].is_some());
    }

    /// Missing column (failed to load) shows as None throughout.
    #[test]
    fn compare_missing_column_shows_none() {
        let cols = vec![
            CompareColumn {
                label: "@a".to_string(),
                result: Some(make_result("rd2_smoke",
                    &[("shadow_p50_ms", 1.5)], Some("drv-x"), true)),
            },
            CompareColumn {
                label: "@missing".to_string(),
                result: None,
            },
        ];
        let report = build_compare_report(cols, None);
        let shadow_row = &report.rows[0];
        assert!(shadow_row.values[1].is_none());
        assert_eq!(report.driver_hashes[1], None);
        assert_eq!(report.assertion_pass[1], None);
    }

    /// Renderer doesn't panic on empty columns.
    #[test]
    fn compare_empty_renders_message() {
        let report = build_compare_report(vec![], None);
        let out = render_compare_report(&report);
        assert!(out.contains("no columns"));
    }

    /// Renderer produces a table with the metric column on the left.
    #[test]
    fn compare_render_has_expected_structure() {
        let cols = vec![
            CompareColumn {
                label: "A".to_string(),
                result: Some(make_result("s",
                    &[("metric_x", 1.5)], Some("drv-x"), true)),
            },
            CompareColumn {
                label: "B".to_string(),
                result: Some(make_result("s",
                    &[("metric_x", 2.0)], Some("drv-x"), false)),
            },
        ];
        let report = build_compare_report(cols, None);
        let out = render_compare_report(&report);
        // Metric is present in output.
        assert!(out.contains("metric_x"));
        // Both labels present.
        assert!(out.contains("A"));
        assert!(out.contains("B"));
        // Driver hash row.
        assert!(out.contains("driver_hash"));
        // Assertion summary row, with PASS and FAIL.
        assert!(out.contains("PASS"));
        assert!(out.contains("FAIL"));
    }

    /// keep_metrics filter restricts to listed keys.
    #[test]
    fn compare_keep_metrics_filters_rows() {
        let cols = vec![CompareColumn {
            label: "X".to_string(),
            result: Some(make_result("s",
                &[("a", 1.0), ("b", 2.0), ("c", 3.0)],
                Some("drv-x"), true)),
        }];
        let report = build_compare_report(cols, Some(&["a", "c"]));
        let metrics: Vec<&String> = report.rows.iter().map(|r| &r.metric).collect();
        assert_eq!(metrics, vec!["a", "c"]);
    }

    // ============================================================
    // run-suite spec discovery tests
    // ============================================================

    #[test]
    fn discover_specs_finds_unendtest_files() {
        let dir = std::env::temp_dir().join(format!(
            "unendlich-suite-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("foo.unendtest"), "").unwrap();
        std::fs::write(dir.join("bar.unendtest"), "").unwrap();
        std::fs::write(dir.join("not-a-spec.txt"), "").unwrap();
        std::fs::write(dir.join("readme.md"), "").unwrap();

        let specs = discover_specs(&dir).unwrap();
        assert_eq!(specs.len(), 2);
        // Sorted: bar before foo.
        assert!(specs[0].ends_with("bar.unendtest"));
        assert!(specs[1].ends_with("foo.unendtest"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn discover_specs_empty_dir_returns_empty() {
        let dir = std::env::temp_dir().join(format!(
            "unendlich-suite-empty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let specs = discover_specs(&dir).unwrap();
        assert_eq!(specs.len(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn discover_specs_missing_dir_errors() {
        let result = discover_specs(std::path::Path::new(
            "/this/path/does/not/exist/anywhere"));
        assert!(result.is_err());
    }

    // ============================================================
    // Post-run triage tests
    // ============================================================

    /// Synthesize a temp dir + events.jsonl with the given window p50 values.
    /// Returns the events file path. The dir lives in /tmp and is unique to
    /// this test invocation.
    fn make_test_events(label: &str, steady_p50_values: &[f64]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "unendlich-triage-test-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.jsonl");
        let mut lines: Vec<String> = Vec::new();
        // Always include a boot-mode event up front; triage must skip it.
        lines.push(serde_json::json!({
            "kind": "frame_window",
            "payload": {"mode": "boot", "frame": 0, "frame_ms": 200.0}
        }).to_string());
        // Then steady-mode events with the given p50 values.
        for (i, p50) in steady_p50_values.iter().enumerate() {
            lines.push(serde_json::json!({
                "kind": "frame_window",
                "payload": {
                    "mode": "steady",
                    "window_secs": 1.0,
                    "frames": 60,
                    "frame_p50_ms": p50,
                    "frame_p99_ms": p50 * 2.0,
                    "frame_max_ms": p50 * 3.0,
                    "loaded": 33,
                    "rendering": 24,
                    "gen_pending": 0,
                    "mesh_q": 0,
                    "upload_fails": 0,
                    "_index": i,
                }
            }).to_string());
        }
        std::fs::write(&path, lines.join("\n")).unwrap();
        path
    }

    struct TriageThresholdGuard {
        prior: Option<String>,
    }
    impl TriageThresholdGuard {
        fn set(val: &str) -> Self {
            let prior = std::env::var("UNENDLICH_TRIAGE_THRESHOLD_MS").ok();
            std::env::set_var("UNENDLICH_TRIAGE_THRESHOLD_MS", val);
            Self { prior }
        }
    }
    impl Drop for TriageThresholdGuard {
        fn drop(&mut self) {
            match &self.prior {
                Some(v) => std::env::set_var("UNENDLICH_TRIAGE_THRESHOLD_MS", v),
                None => std::env::remove_var("UNENDLICH_TRIAGE_THRESHOLD_MS"),
            }
        }
    }

    /// Healthy run: all windows under threshold. Triage deletes events.
    #[test]
    fn triage_healthy_run_deletes_events() {
        let _g = TriageThresholdGuard::set("33.0");
        let events = make_test_events("healthy", &[2.0, 2.1, 1.9, 2.5]);
        let summary = run_triage(&events).expect("triage returns summary");

        assert_eq!(summary.steady_windows, 4);
        assert!(!summary.had_incident,
            "no window exceeded 33ms; should not be flagged. worst={}",
            summary.worst_p50_ms);
        assert!(!summary.events_kept, "healthy events should be deleted");
        assert!(!events.exists(), "events file should have been deleted");

        // incident_summary.json should be written regardless.
        let summary_path = events.parent().unwrap().join("incident_summary.json");
        assert!(summary_path.exists(), "incident_summary.json should exist");

        std::fs::remove_dir_all(events.parent().unwrap()).ok();
    }

    /// Slow run: one window over threshold. Triage keeps events.
    #[test]
    fn triage_slow_run_keeps_events() {
        let _g = TriageThresholdGuard::set("33.0");
        let events = make_test_events("slow", &[2.0, 45.0, 2.1, 1.9]);
        let summary = run_triage(&events).expect("triage returns summary");

        assert_eq!(summary.steady_windows, 4);
        assert!(summary.had_incident, "window at 45ms > 33ms should flag");
        assert!((summary.worst_p50_ms - 45.0).abs() < 0.001);
        assert!(summary.events_kept, "incident events should be preserved");
        assert!(events.exists(), "events file should still exist");

        std::fs::remove_dir_all(events.parent().unwrap()).ok();
    }

    /// No steady windows (run too short to triage): keep events to be safe.
    #[test]
    fn triage_no_steady_windows_keeps_events() {
        let _g = TriageThresholdGuard::set("33.0");
        // Only boot events, no steady ones.
        let events = make_test_events("short", &[]);
        let summary = run_triage(&events).expect("triage returns summary");

        assert_eq!(summary.steady_windows, 0);
        assert!(!summary.had_incident, "no windows to flag");
        assert!(summary.events_kept, "too-short runs should preserve events");
        assert!(events.exists());

        std::fs::remove_dir_all(events.parent().unwrap()).ok();
    }

    /// Boot-mode events MUST NOT count toward triage. A run with one
    /// terrible boot frame followed by clean steady frames should be
    /// classified as healthy.
    #[test]
    fn triage_ignores_boot_mode_events() {
        let _g = TriageThresholdGuard::set("33.0");
        // The boot event in make_test_events has frame_ms=200.0 (terrible),
        // but should be skipped. Only the steady values matter.
        let events = make_test_events("boot_only_jank", &[2.0, 2.5, 2.1]);
        let summary = run_triage(&events).expect("triage returns summary");

        assert!(!summary.had_incident,
            "boot-mode jank should not trigger incident classification");
        assert!(!summary.events_kept);

        std::fs::remove_dir_all(events.parent().unwrap()).ok();
    }

    /// Threshold is configurable via env var. Lowering it makes more
    /// runs incident-classify.
    #[test]
    fn triage_threshold_env_var_works() {
        let _g = TriageThresholdGuard::set("5.0");
        // 8ms p50 wouldn't flag at default 33ms, but DOES at 5ms.
        let events = make_test_events("low_threshold", &[2.0, 8.0, 2.1]);
        let summary = run_triage(&events).expect("triage returns summary");

        assert!(summary.had_incident,
            "8ms > 5ms threshold should flag");
        assert_eq!(summary.threshold_ms, 5.0);
        assert!(summary.events_kept);

        std::fs::remove_dir_all(events.parent().unwrap()).ok();
    }

    /// Missing events file: triage returns None (don't crash, don't
    /// invent a summary).
    #[test]
    fn triage_missing_events_returns_none() {
        let bogus = PathBuf::from("/tmp/this/path/definitely/does/not/exist/events.jsonl");
        let result = run_triage(&bogus);
        assert!(result.is_none(), "missing events should produce None");
    }

    // ============================================================
    // c.13-scout: multiview in compare output
    // ============================================================

    /// Helper that lets a test supply a custom multiview state.
    fn make_result_with_multiview(
        label: &str,
        driver_hash: &str,
        multiview_csm: bool,
    ) -> unendlich_shared::test_spec::TestResult {
        use unendlich_shared::test_spec::*;
        let mv = if multiview_csm {
            MultiviewSupport {
                multiview: true,
                max_view_count: 6,
                max_instance_index: 0x7fff,
                geometry_shader: false,
                tessellation_shader: false,
            }
        } else {
            MultiviewSupport::default()
        };
        let gpu_info = Some(GpuInfo {
            vendor_id: 0x106B,
            device_id: 0x1024,
            driver_version_raw: 0x00400000,
            driver_name: "Asahi".to_string(),
            driver_info: "info".to_string(),
            driver_hash: driver_hash.to_string(),
            multiview: mv,
        });
        TestResult {
            spec_version: 1,
            spec_path: format!("/tmp/{}.unendtest", label),
            spec_name: label.to_string(),
            binary_version: "0.24.2-c.13-scout".to_string(),
            binary: "unendlich-world".to_string(),
            run_uuid: format!("r-{}", label),
            ran_at_utc: "2026-05-13T00:00:00Z".to_string(),
            duration_secs: 5.0,
            shutdown_reason: "max_duration".to_string(),
            env: std::collections::BTreeMap::new(),
            gpu_info,
            stats: serde_json::Map::new(),
            assertions: vec![],
            passed: true,
        }
    }

    /// Two columns both supporting CSM: report shows yes/yes.
    #[test]
    fn compare_multiview_both_supported() {
        let cols = vec![
            CompareColumn {
                label: "@a".to_string(),
                result: Some(make_result_with_multiview("s", "drv-x", true)),
            },
            CompareColumn {
                label: "@b".to_string(),
                result: Some(make_result_with_multiview("s", "drv-x", true)),
            },
        ];
        let report = build_compare_report(cols, None);
        assert_eq!(report.multiview_csm, vec![Some(true), Some(true)]);
        let out = render_compare_report(&report);
        assert!(out.contains("csm_capable"), "renderer should show csm_capable row");
        assert!(out.contains("yes"), "renderer should show 'yes' for capable");
    }

    /// Asymmetric capability: row still rendered.
    #[test]
    fn compare_multiview_asymmetric() {
        let cols = vec![
            CompareColumn {
                label: "@old_driver".to_string(),
                result: Some(make_result_with_multiview("s", "drv-old", false)),
            },
            CompareColumn {
                label: "@new_driver".to_string(),
                result: Some(make_result_with_multiview("s", "drv-new", true)),
            },
        ];
        let report = build_compare_report(cols, None);
        assert_eq!(report.multiview_csm, vec![Some(false), Some(true)]);
        let out = render_compare_report(&report);
        assert!(out.contains("csm_capable"));
        assert!(out.contains("no"));
        assert!(out.contains("yes"));
    }

    /// No columns expose multiview: row is omitted (backward-compat
    /// with pre-c.13 results that didn't have the field).
    #[test]
    fn compare_multiview_row_omitted_when_all_missing() {
        let cols = vec![
            CompareColumn {
                label: "@a".to_string(),
                result: Some(make_result("s",
                    &[("shadow_p50_ms", 1.5)], None, true)),
            },
            CompareColumn {
                label: "@b".to_string(),
                result: Some(make_result("s",
                    &[("shadow_p50_ms", 1.6)], None, true)),
            },
        ];
        let report = build_compare_report(cols, None);
        assert_eq!(report.multiview_csm, vec![None, None]);
        let out = render_compare_report(&report);
        // No csm_capable row should be present when no column has the info.
        assert!(!out.contains("csm_capable"),
            "csm_capable row should be omitted when all columns lack multiview info");
    }
}
