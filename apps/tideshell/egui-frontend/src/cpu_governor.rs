//! cpu_governor — cpupower's scaling governor, read and written over sysfs.
//!
//! `tideshell_core::control_center::CpuGovernor` decides the pure three-way
//! cycle (Powersave -> Conservative -> Performance) with no notion of what a
//! given kernel actually supports. This module is the other half: find every
//! CPU's `cpufreq` directory, read what governors the driver actually offers,
//! and write the requested one to all of them.
//!
//! ## Why "all of them"
//!
//! A big.LITTLE SoC (the Fairphone 5's included) can expose per-cluster
//! `cpufreq` policies, and in principle two clusters could even support
//! different governor sets. Writing to every `cpuN/cpufreq/scaling_governor`
//! this machine has, rather than assuming `cpu0` speaks for all of them,
//! is what makes "performance mode" actually mean every core.
//!
//! ## Verified and not
//!
//! `parse_available` (splitting `scaling_available_governors`) and
//! `next_available` (skipping a requested governor the kernel does not
//! offer) are pure and tested. The sysfs scan and the actual reads/writes are
//! real code, gated to Linux, checked only by `cargo check`/`cargo test`
//! compiling them -- there is no cpufreq sysfs tree in this container to run
//! them against, the same caveat every other direct-sysfs path in this
//! codebase already carries (tideshell's own backlight writer, eink-kit's
//! device probes).

use tideshell_core::control_center::CpuGovernor;

/// Every governor name from a `scaling_available_governors` file that this
/// shell recognises. Order does not matter; the CALLER's cycle order does.
/// Unrecognised names (`schedutil`, `ondemand`, `userspace`, ...) are silently
/// skipped -- this shell only ever offers the three it cycles through.
pub fn parse_available(contents: &str) -> Vec<CpuGovernor> {
    contents.split_whitespace().filter_map(CpuGovernor::from_token).collect()
}

/// What to actually request, given what a cycle produced and what the kernel
/// offers.
///
/// If `wanted` is not available, skip forward through the SAME cycle order
/// (`wanted.next()`, then that governor's `.next()`, ...) until one is, or
/// until every governor has been tried -- which returns `None`: a kernel that
/// offers none of the three names this shell knows (e.g. only `schedutil`)
/// gets no request at all rather than a wrong one.
pub fn next_available(wanted: CpuGovernor, available: &[CpuGovernor]) -> Option<CpuGovernor> {
    let mut g = wanted;
    for _ in 0..CpuGovernor::ALL.len() {
        if available.contains(&g) {
            return Some(g);
        }
        g = g.next();
    }
    None
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::path::PathBuf;

    const ROOT: &str = "/sys/devices/system/cpu";

    /// Every `cpuN/cpufreq` directory this machine has, in numeric order (not
    /// directory-listing order, which is lexical: cpu10 would sort before
    /// cpu2).
    pub(super) fn cpufreq_dirs(root: &std::path::Path) -> Vec<PathBuf> {
        let mut nums: Vec<u32> = std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_prefix("cpu")?.parse().ok()))
            .filter(|n: &u32| root.join(format!("cpu{n}/cpufreq")).is_dir())
            .collect();
        nums.sort_unstable();
        nums.into_iter().map(|n| root.join(format!("cpu{n}/cpufreq"))).collect()
    }

    /// The governors offered, read from the FIRST cpu found (every cpufreq
    /// policy on a given SoC generation offers the same set in practice; a
    /// truly asymmetric kernel is outside what this shell tries to handle).
    pub fn available(root: &std::path::Path) -> Result<Vec<CpuGovernor>, String> {
        let dirs = cpufreq_dirs(root);
        let first = dirs.first().ok_or("no cpuN/cpufreq directories found")?;
        let s = std::fs::read_to_string(first.join("scaling_available_governors"))
            .map_err(|e| format!("reading {}: {e}", first.display()))?;
        Ok(parse_available(&s))
    }

    /// Write `governor` to every cpu's `scaling_governor`. Continues past a
    /// failing core rather than stopping at the first -- a phone with one
    /// stuck core should still get the others -- and reports every failure.
    pub fn write_all(root: &std::path::Path, governor: CpuGovernor) -> Result<(), String> {
        let dirs = cpufreq_dirs(root);
        if dirs.is_empty() {
            return Err(format!("no cpuN/cpufreq directories under {}", root.display()));
        }
        let mut failed = Vec::new();
        for dir in &dirs {
            if let Err(e) = std::fs::write(dir.join("scaling_governor"), governor.token()) {
                failed.push(format!("{}: {e}", dir.display()));
            }
        }
        if failed.is_empty() { Ok(()) } else { Err(failed.join("; ")) }
    }

    /// Read, choose, and write: the whole real path, called with the real
    /// root by `apply`. Split out so tests can point `root` at a fixture.
    pub fn apply_at(root: &std::path::Path, wanted: CpuGovernor) -> Result<CpuGovernor, String> {
        let avail = available(root)?;
        let chosen = next_available(wanted, &avail)
            .ok_or_else(|| format!("kernel offers none of powersave/conservative/performance: {avail:?} from {:?}", parse_available("")))?;
        write_all(root, chosen)?;
        Ok(chosen)
    }

    pub fn apply(wanted: CpuGovernor) -> Result<CpuGovernor, String> {
        apply_at(std::path::Path::new(ROOT), wanted)
    }
}

#[cfg(target_os = "linux")]
pub use linux::apply;

#[cfg(not(target_os = "linux"))]
pub fn apply(_wanted: CpuGovernor) -> Result<CpuGovernor, String> {
    Err("cpu governor: needs Linux's cpufreq sysfs".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_looking_available_governors_line() {
        assert_eq!(parse_available("conservative ondemand userspace powersave performance schedutil\n"),
                   vec![CpuGovernor::Conservative, CpuGovernor::Powersave, CpuGovernor::Performance]);
    }

    #[test]
    fn an_empty_or_unrecognised_line_yields_nothing() {
        assert_eq!(parse_available(""), vec![]);
        assert_eq!(parse_available("schedutil ondemand userspace"), vec![]);
    }

    #[test]
    fn the_wanted_governor_is_used_when_available() {
        assert_eq!(next_available(CpuGovernor::Performance, &CpuGovernor::ALL), Some(CpuGovernor::Performance));
    }

    /// THE REAL-WORLD CASE: many current desktop kernels dropped
    /// `conservative` for `schedutil`. Cycling onto it must still land
    /// somewhere real rather than silently doing nothing.
    #[test]
    fn an_unavailable_governor_skips_forward_through_the_cycle() {
        let no_conservative = [CpuGovernor::Powersave, CpuGovernor::Performance];
        assert_eq!(next_available(CpuGovernor::Conservative, &no_conservative), Some(CpuGovernor::Performance),
                   "skips to the next in cycle order, not just any available one");
    }

    #[test]
    fn none_available_yields_none_rather_than_a_wrong_request() {
        assert_eq!(next_available(CpuGovernor::Powersave, &[]), None);
    }

    /// A single governor available, requested indirectly: the search must
    /// wrap all the way around the cycle rather than stopping after one step.
    #[test]
    fn skipping_wraps_the_whole_cycle_if_needed() {
        let only_performance = [CpuGovernor::Performance];
        assert_eq!(next_available(CpuGovernor::Powersave, &only_performance), Some(CpuGovernor::Performance));
    }

    #[cfg(target_os = "linux")]
    mod linux_fs {
        use super::super::linux::*;
        use tideshell_core::control_center::CpuGovernor;

        fn fixture(cpus: &[(&str, &str)]) -> std::path::PathBuf {
            let root = std::env::temp_dir().join(format!("cpufreq_test_{}_{}", std::process::id(), cpus.len()));
            let _ = std::fs::remove_dir_all(&root);
            for (i, (avail, _)) in cpus.iter().enumerate() {
                let dir = root.join(format!("cpu{i}/cpufreq"));
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("scaling_available_governors"), avail).unwrap();
                std::fs::write(dir.join("scaling_governor"), "powersave").unwrap();
            }
            // A non-cpufreq entry (e.g. a cooling device) must not be mistaken
            // for one -- no cpufreq subdirectory, so it is skipped.
            std::fs::create_dir_all(root.join("cpu0/thermal_throttle")).unwrap();
            root
        }

        /// END TO END on a real (fixture) filesystem: apply writes the SAME
        /// governor to every cpu, in numeric order, and reports what it chose.
        #[test]
        fn apply_at_writes_every_cpu_and_returns_what_it_chose() {
            let root = fixture(&[("powersave conservative performance", ""); 4]);
            let chosen = apply_at(&root, CpuGovernor::Performance).unwrap();
            assert_eq!(chosen, CpuGovernor::Performance);
            for i in 0..4 {
                let g = std::fs::read_to_string(root.join(format!("cpu{i}/cpufreq/scaling_governor"))).unwrap();
                assert_eq!(g, "performance");
            }
            let _ = std::fs::remove_dir_all(&root);
        }

        /// Directories are visited cpu0, cpu1, ..., cpu10 -- NOT the lexical
        /// order a plain directory listing gives ("cpu10" < "cpu2").
        #[test]
        fn directories_are_visited_in_numeric_not_lexical_order() {
            let root = std::env::temp_dir().join(format!("cpufreq_numeric_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            for n in [0, 1, 2, 10] {
                let dir = root.join(format!("cpu{n}/cpufreq"));
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("scaling_available_governors"), "powersave").unwrap();
            }
            let dirs = cpufreq_dirs(&root);
            let nums: Vec<_> = dirs.iter().map(|d| {
                d.parent().unwrap().file_name().unwrap().to_str().unwrap().strip_prefix("cpu").unwrap().parse::<u32>().unwrap()
            }).collect();
            assert_eq!(nums, vec![0, 1, 2, 10]);
            let _ = std::fs::remove_dir_all(&root);
        }

        /// One stuck core must not stop the others from getting the request.
        #[test]
        fn a_failure_on_one_core_does_not_stop_the_rest() {
            let root = fixture(&[("powersave performance", ""); 3]);
            // Replace cpu1's writable file with a read-only directory of the
            // same name, so the write to it fails while cpu0 and cpu2 succeed.
            std::fs::remove_file(root.join("cpu1/cpufreq/scaling_governor")).unwrap();
            std::fs::create_dir(root.join("cpu1/cpufreq/scaling_governor")).unwrap();
            let err = write_all(&root, CpuGovernor::Performance).unwrap_err();
            assert!(err.contains("cpu1"), "{err}");
            for i in [0, 2] {
                let g = std::fs::read_to_string(root.join(format!("cpu{i}/cpufreq/scaling_governor"))).unwrap();
                assert_eq!(g, "performance", "cpu{i} should still have been written");
            }
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn a_kernel_offering_none_of_the_three_is_a_clean_error() {
            let root = fixture(&[("schedutil ondemand", "")]);
            let err = apply_at(&root, CpuGovernor::Powersave).unwrap_err();
            assert!(err.contains("none"), "{err}");
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}
