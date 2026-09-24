//! paths — where checkpoints, mel caches and scratch go.
//!
//! Training wrote to `./checkpoints`, `./cache` and `./data`, all relative to
//! the CURRENT DIRECTORY. That has three failure modes and a training run is
//! long enough to hit all of them:
//!
//! * A run started from a different directory silently trains from scratch
//!   instead of resuming, because `./checkpoints` is somewhere else now. Hours
//!   of GPU time, and the symptom is a loss curve that looks wrong rather than
//!   an error.
//! * Two runs from two directories produce two checkpoint sets nobody can
//!   tell apart later.
//! * `./cache` puts tens of gigabytes of decoded mel frames wherever the shell
//!   happened to be — often a source tree, occasionally a network mount.
//!
//! ## Checkpoints are DATA, caches are DISPOSABLE, scratch is EPHEMERAL
//!
//! The split matters because the recovery is different for each:
//!
//! * `~/models/beatgen` — checkpoints. Losing these costs the training run.
//!   Not under `~/.cache`, which tools are entitled to delete: a cache cleaner
//!   eating a week of GPU time is a real outcome and the directory name is the
//!   only thing preventing it.
//! * `$XDG_CACHE_HOME/beatspace` (`~/.cache/beatspace`) — decoded mel frames.
//!   Losing these costs a re-decode, which is minutes. This is what a cache
//!   directory is FOR, and putting them here is what makes the checkpoint
//!   directory safe to treat as precious.
//! * `$TMPDIR` (`/tmp`) — per-run scratch. Losing these mid-run is fatal to
//!   the run and costs nothing after it. On most Linux systems /tmp is tmpfs,
//!   so this is also the one that must NOT hold the mel cache: a 40 GB cache
//!   in RAM is an out-of-memory kill rather than a cache.
//!
//! ## Every one is overridable, and an explicit path always wins
//!
//! These are DEFAULTS. A `--ckpt-dir` on the command line is an instruction,
//! and an environment variable is how a box with a small home partition or a
//! fast scratch disk says so once instead of on every invocation.

use std::path::{Path, PathBuf};

/// Where checkpoints live by default: `~/models/beatgen`.
///
/// Deliberately NOT under `~/.cache`. Checkpoints are the output of the work,
/// and `~/.cache` is a directory whose whole contract is that something else
/// may delete it.
pub fn checkpoints() -> PathBuf {
    if let Some(p) = env_path("BEATSPACE_CKPT_DIR") {
        return p;
    }
    match home() {
        Some(h) => h.join("models").join("beatgen"),
        // No HOME is a service account or a broken container. A relative path
        // is the old behaviour and still better than writing to `/`.
        None => PathBuf::from("./checkpoints"),
    }
}

/// Where decoded mel frames live: `$XDG_CACHE_HOME/beatspace`, else
/// `~/.cache/beatspace`.
///
/// The XDG variable first, because a user who set it meant it — that is
/// usually how a small home partition is worked around, which is exactly the
/// case a multi-gigabyte mel cache cares about.
pub fn mel_cache() -> PathBuf {
    if let Some(p) = env_path("BEATSPACE_CACHE_DIR") {
        return p;
    }
    if let Some(x) = env_path("XDG_CACHE_HOME") {
        return x.join("beatspace");
    }
    match home() {
        Some(h) => h.join(".cache").join("beatspace"),
        None => PathBuf::from("./cache"),
    }
}

/// Per-run scratch under `$TMPDIR`, named for the run.
///
/// Takes a run id so two concurrent trainings cannot collide — on a shared
/// box `/tmp/beatspace` is a directory two users can both own half of, and the
/// failure is a decode from one run being read by the other.
///
/// NOT where the mel cache goes: /tmp is tmpfs on most Linux systems, so a
/// large cache here is an OOM kill rather than a cache.
pub fn scratch(run_id: &str) -> PathBuf {
    let base = env_path("TMPDIR").unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join(format!("beatspace-{}", sanitise(run_id)))
}

/// A run id safe to put in a path.
///
/// Anything that is not alphanumeric, `-` or `_` becomes `-`. A run id can
/// come from a dataset name or a timestamp, and one containing `/` or `..`
/// would otherwise write outside the scratch directory entirely.
pub fn sanitise(id: &str) -> String {
    let s: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "run".to_string() } else { s }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())
}

fn env_path(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())
}

/// Is `p` somewhere a cache cleaner may delete?
///
/// For warning when checkpoints have been pointed at a cache directory — an
/// override is allowed, and silently losing a week of GPU time to `rm -rf
/// ~/.cache` is worth one line of warning first.
pub fn is_cache_territory(p: &Path) -> bool {
    let s = p.to_string_lossy();
    s.contains("/.cache/") || s.ends_with("/.cache")
        || s.starts_with("/tmp/") || s == "/tmp"
        || s.contains("/var/tmp/")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CHECKPOINTS ARE NOT A CACHE. `~/.cache` is a directory whose contract
    /// is that something else may delete it, and a cache cleaner eating a week
    /// of GPU time is a real outcome.
    #[test]
    fn checkpoints_do_not_live_under_a_cache_directory() {
        let p = checkpoints();
        assert!(!is_cache_territory(&p), "{}", p.display());
    }

    /// The mel cache DOES, because losing it costs a re-decode and that is
    /// what makes the checkpoint directory safe to treat as precious.
    #[test]
    fn the_mel_cache_lives_where_caches_go() {
        // Only meaningful with a HOME; a container without one falls back.
        if home().is_some() && std::env::var_os("BEATSPACE_CACHE_DIR").is_none() {
            let p = mel_cache();
            assert!(p.to_string_lossy().contains("beatspace"), "{}", p.display());
        }
    }

    /// SCRATCH IS PER-RUN. On a shared box a single /tmp/beatspace is a
    /// directory two users own half of, and one run reads the other's decode.
    #[test]
    fn two_runs_get_different_scratch() {
        assert_ne!(scratch("alpha"), scratch("beta"));
        assert!(scratch("alpha").to_string_lossy().contains("alpha"));
    }

    /// A run id from a dataset name or a timestamp must not escape the
    /// scratch directory.
    #[test]
    fn a_run_id_cannot_escape_the_scratch_directory() {
        for evil in ["../../etc", "a/b", "..", "/absolute", "with space"] {
            let s = sanitise(evil);
            assert!(!s.contains('/'), "{evil} -> {s}");
            assert!(!s.contains(".."), "{evil} -> {s}");
            let p = scratch(evil);
            assert!(p.to_string_lossy().contains("beatspace-"), "{}", p.display());
        }
    }

    /// An id that sanitises to nothing still yields a usable directory rather
    /// than the parent.
    #[test]
    fn an_empty_id_does_not_become_the_tmp_root() {
        assert_eq!(sanitise("///"), "run");
        assert_eq!(sanitise(""), "run");
        let p = scratch("///");
        assert!(p.to_string_lossy().ends_with("beatspace-run"), "{}", p.display());
    }

    /// AN EXPLICIT OVERRIDE ALWAYS WINS. A box with a small home partition
    /// says so once rather than on every invocation.
    #[test]
    fn the_environment_overrides_every_default() {
        // SAFETY: single-threaded test, restored immediately.
        unsafe { std::env::set_var("BEATSPACE_CKPT_DIR", "/mnt/big/ckpt") };
        assert_eq!(checkpoints(), PathBuf::from("/mnt/big/ckpt"));
        unsafe { std::env::remove_var("BEATSPACE_CKPT_DIR") };

        unsafe { std::env::set_var("BEATSPACE_CACHE_DIR", "/mnt/fast/mel") };
        assert_eq!(mel_cache(), PathBuf::from("/mnt/fast/mel"));
        unsafe { std::env::remove_var("BEATSPACE_CACHE_DIR") };
    }

    /// An EMPTY variable is not an override. Shells set these to "" more often
    /// than anyone expects, and honouring it would put checkpoints in the
    /// filesystem root.
    #[test]
    fn an_empty_variable_is_ignored_rather_than_obeyed() {
        unsafe { std::env::set_var("BEATSPACE_CKPT_DIR", "") };
        let p = checkpoints();
        assert_ne!(p, PathBuf::from(""), "an empty override must not be taken");
        unsafe { std::env::remove_var("BEATSPACE_CKPT_DIR") };
    }

    /// The warning fires on the cases it exists for, and not on the default.
    #[test]
    fn cache_territory_recognises_the_places_that_get_cleaned() {
        assert!(is_cache_territory(Path::new("/home/u/.cache/beatspace")));
        assert!(is_cache_territory(Path::new("/tmp/ckpt")));
        assert!(is_cache_territory(Path::new("/var/tmp/ckpt")));
        assert!(!is_cache_territory(Path::new("/home/u/models/beatgen")));
        assert!(!is_cache_territory(Path::new("/mnt/big/ckpt")));
    }
}
