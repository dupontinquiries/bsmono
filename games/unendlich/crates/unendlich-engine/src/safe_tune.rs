//! Crash-safe runtime tuning.
//!
//! Some perf settings (today: the renderer mesh cache) can, on a bad
//! GPU/driver combination, hard-crash the process — and a hard GPU/driver
//! crash can't be caught in-process (`catch_unwind` never sees it). So instead
//! of catching the crash we *survive* it across restarts with a trial protocol:
//!
//!   1. Before applying a risky value, [`SafeTune::arm`] writes a pending marker
//!      (`setting`, `value`, this machine's `driver_hash`) to the config dir.
//!   2. If the game runs long enough to prove the setting stable,
//!      [`SafeTune::disarm`] removes the marker (a clean trial).
//!   3. On the NEXT startup, [`SafeTune::load`] checks the marker. If it's still
//!      there, the previous run died while the setting was on trial. We do NOT
//!      reapply it, and we add it to a per-driver blacklist so it stays off **as
//!      long as the same driver is reported**. A driver update changes
//!      `driver_hash`, which no longer matches the blacklist line, so the
//!      setting is retried fresh on the new driver.
//!
//! The blacklist is keyed on `driver_hash` (a composite of vendor/device/driver
//! version + name), so a block is per-device-per-driver, never global. The files
//! are plain TSV in the config dir; corruption or absence degrades to "nothing
//! blacklisted, nothing pending" rather than failing the launch.

use std::path::{Path, PathBuf};

/// Subdirectory under the platform config dir where tune state lives.
const SUBDIR: &str = "unendlich";
const PENDING_FILE: &str = "tune_pending";
const BLACKLIST_FILE: &str = "tune_blacklist";

pub struct SafeTune {
    dir: PathBuf,
    driver_hash: String,
    /// Settings blacklisted for THIS driver_hash.
    blacklist: Vec<String>,
    /// `(setting, value)` reverted during this `load` because the previous run
    /// crashed on trial. Surfaced for a one-line startup log.
    reverted: Option<(String, String)>,
}

impl SafeTune {
    /// Load tune state for this machine. Resolves the pending marker: if one
    /// survived from a previous run on the *same* driver, that run crashed while
    /// the setting was on trial — blacklist it and report it as reverted.
    ///
    /// `config_dir` is the platform config root (e.g. `~/.config`); the
    /// `unendlich/` subdir is created if missing.
    ///
    /// r-safe-tune-reset: checks `UNENDLICH_RESET_SAFE_TUNE` first and, if set,
    /// clears both the blacklist and any pending marker for every driver before
    /// doing anything else. Why this exists: the blacklist is keyed ONLY on
    /// `driver_hash` (deliberately, per this module's doc -- a block is
    /// per-device-per-driver) with no notion of the ENGINE BUILD at all. A
    /// setting blacklisted by a crash that had nothing to do with it (a
    /// GPU-watchdog self-stop from unrelated excessive geometry, say) stays
    /// blacklisted forever on that driver, through any number of rebuilds that
    /// fix the actual cause -- there's no way to tell "this crash implicates
    /// the setting" from "this crash happened to occur while the setting
    /// happened to be on trial" from the marker alone. Recompiling doesn't
    /// clear it; only a driver update (changing driver_hash) or manually
    /// editing the TSV file would. This env var is the honest alternative to
    /// silently guessing at a same-build-vs-different-build heuristic: an
    /// explicit, logged, opt-in "give every setting a fresh trial" escape
    /// hatch for exactly the situation of iterating on a crash fix and wanting
    /// previously-blacklisted settings retried without hunting down a hidden
    /// config file by hand.
    pub fn load(config_dir: &Path, driver_hash: String) -> Self {
        let dir = config_dir.join(SUBDIR);
        let _ = std::fs::create_dir_all(&dir);

        if std::env::var("UNENDLICH_RESET_SAFE_TUNE").is_ok() {
            let _ = std::fs::remove_file(dir.join(BLACKLIST_FILE));
            let _ = std::fs::remove_file(dir.join(PENDING_FILE));
            log::info!(
                "[safe-tune] UNENDLICH_RESET_SAFE_TUNE set -- cleared the blacklist and any \
                 pending trial marker for all drivers. Every safe-tune setting gets a fresh \
                 trial this run."
            );
        }

        // Existing blacklist entries for this driver.
        let mut blacklist: Vec<String> = read_lines(&dir.join(BLACKLIST_FILE))
            .into_iter()
            .filter_map(|l| {
                let (h, s) = l.split_once('\t')?;
                (h == driver_hash).then(|| s.to_string())
            })
            .collect();

        // Pending marker => a setting was mid-trial when the process last ended.
        let mut reverted = None;
        let pending_path = dir.join(PENDING_FILE);
        if let Some(line) = read_lines(&pending_path).into_iter().next() {
            let parts: Vec<&str> = line.split('\t').collect();
            // format: setting \t value \t driver_hash
            if parts.len() == 3 && parts[2] == driver_hash {
                let setting = parts[0].to_string();
                if !blacklist.contains(&setting) {
                    blacklist.push(setting.clone());
                    append_blacklist(&dir, &driver_hash, &setting);
                }
                reverted = Some((setting, parts[1].to_string()));
            }
            // Clear the stale marker regardless (mismatched driver => irrelevant).
            let _ = std::fs::remove_file(&pending_path);
        }

        SafeTune { dir, driver_hash, blacklist, reverted }
    }

    /// True if `setting` crashed on a prior run under the current driver.
    pub fn is_blacklisted(&self, setting: &str) -> bool {
        self.blacklist.iter().any(|s| s == setting)
    }

    /// `(setting, value)` that was reverted this startup, if any (for logging).
    pub fn reverted(&self) -> Option<(&str, &str)> {
        self.reverted.as_ref().map(|(s, v)| (s.as_str(), v.as_str()))
    }

    /// Put a setting on trial: write the pending marker *before* applying the
    /// risky value, so a crash leaves evidence for the next startup.
    pub fn arm(&self, setting: &str, value: &str) {
        let _ = std::fs::write(
            self.dir.join(PENDING_FILE),
            format!("{}\t{}\t{}", setting, value, self.driver_hash),
        );
    }

    /// The setting proved stable (survived the trial window): clear the marker
    /// so the next startup doesn't read it as a crash.
    pub fn disarm(&self) {
        let _ = std::fs::remove_file(self.dir.join(PENDING_FILE));
    }
}

fn read_lines(p: &Path) -> Vec<String> {
    std::fs::read_to_string(p)
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn append_blacklist(dir: &Path, driver_hash: &str, setting: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(BLACKLIST_FILE))
    {
        let _ = writeln!(f, "{}\t{}", driver_hash, setting);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!("unendlich_safetune_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn clean_run_no_blacklist() {
        let d = tmp();
        let st = SafeTune::load(&d, "drvA".into());
        assert!(!st.is_blacklisted("mesh_cache"));
        assert!(st.reverted().is_none());
        st.arm("mesh_cache", "512");
        st.disarm(); // clean trial
        // Next load sees no pending marker -> still clean.
        let st2 = SafeTune::load(&d, "drvA".into());
        assert!(!st2.is_blacklisted("mesh_cache"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn crash_on_trial_blacklists_for_same_driver_only() {
        let d = tmp();
        // Run 1: arm but never disarm (simulates a crash mid-trial).
        SafeTune::load(&d, "drvA".into()).arm("mesh_cache", "512");
        // Run 2, same driver: the marker is read as a crash -> blacklisted.
        let st = SafeTune::load(&d, "drvA".into());
        assert!(st.is_blacklisted("mesh_cache"));
        assert_eq!(st.reverted(), Some(("mesh_cache", "512")));
        // Run 3, DIFFERENT driver (update): block does not apply.
        let st3 = SafeTune::load(&d, "drvB".into());
        assert!(!st3.is_blacklisted("mesh_cache"));
        let _ = std::fs::remove_dir_all(&d);
    }

    // r-safe-tune-reset: tests for UNENDLICH_RESET_SAFE_TUNE. Each test sets
    // the env var immediately before the `load()` call it's testing and
    // clears it immediately after (even on an assertion failure -- these are
    // simple linear tests with a single load() under the var, no early
    // returns, so the removal after the assertions always runs) to keep the
    // window where it could interfere with a concurrently-running test as
    // small as this codebase's existing env-var tests already accept
    // (UNENDLICH_PLANT_BUILD_SPREAD, UNENDLICH_PREFETCH use the same
    // unguarded set_var/remove_var pattern elsewhere in this crate).

    #[test]
    fn reset_env_var_clears_an_existing_blacklist_entry() {
        let d = tmp();
        // Get "mesh_cache" blacklisted the normal way first.
        SafeTune::load(&d, "drvA".into()).arm("mesh_cache", "512");
        let st = SafeTune::load(&d, "drvA".into());
        assert!(st.is_blacklisted("mesh_cache"), "fixture sanity: should be blacklisted before reset");

        std::env::set_var("UNENDLICH_RESET_SAFE_TUNE", "1");
        let st_reset = SafeTune::load(&d, "drvA".into());
        std::env::remove_var("UNENDLICH_RESET_SAFE_TUNE");

        assert!(
            !st_reset.is_blacklisted("mesh_cache"),
            "UNENDLICH_RESET_SAFE_TUNE should have cleared the blacklist entry"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn reset_env_var_also_clears_a_pending_marker_so_it_is_not_read_as_a_new_crash() {
        let d = tmp();
        // Arm but never disarm -- a pending marker sits on disk, as if a
        // crash were about to be attributed on the next load.
        SafeTune::load(&d, "drvA".into()).arm("mesh_cache", "512");

        std::env::set_var("UNENDLICH_RESET_SAFE_TUNE", "1");
        let st_reset = SafeTune::load(&d, "drvA".into());
        std::env::remove_var("UNENDLICH_RESET_SAFE_TUNE");

        assert!(
            !st_reset.is_blacklisted("mesh_cache"),
            "the pending marker should have been cleared, not read as a crash"
        );
        assert_eq!(
            st_reset.reverted(), None,
            "nothing should be reported as reverted this startup -- the marker was cleared, not read"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn without_the_reset_env_var_existing_blacklist_is_unaffected() {
        // Regression guard: adding the reset path must not change behavior
        // when the var isn't set. Explicitly ensures it's absent first, in
        // case a prior test in this process left it set (defense in depth
        // beyond each test's own remove_var).
        std::env::remove_var("UNENDLICH_RESET_SAFE_TUNE");
        let d = tmp();
        SafeTune::load(&d, "drvA".into()).arm("mesh_cache", "512");
        let st = SafeTune::load(&d, "drvA".into());
        assert!(
            st.is_blacklisted("mesh_cache"),
            "normal blacklist behavior should be unchanged when the reset var is not set"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
