//! v0.1.896: process memory-footprint instrumentation.
//!
//! Off by default. Set `WEB2QML_MEMLOG=1` to have the app emit a periodic
//! `[mem] …` line carrying the process RSS / peak-RSS plus a per-subsystem
//! breakdown; route it to a file with `WEB2QML_LOG=mem`. This is the data
//! we'll read in the later optimization pass — instrument first, optimize on
//! evidence rather than guesswork.
//!
//! The parsing and truthiness rules are split into pure functions so they're
//! testable without a live `/proc`.

use std::sync::OnceLock;

/// Snapshot of the relevant `/proc/self/status` fields (all in kB, as the
/// kernel reports them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProcMem {
    /// Resident set size — what we actually occupy in RAM right now.
    pub vm_rss_kb: u64,
    /// Peak RSS ("high water mark") — the worst we've hit this run.
    pub vm_hwm_kb: u64,
    /// Data+heap segment — tracks allocator growth over time.
    pub vm_data_kb: u64,
    /// Total virtual size.
    pub vm_size_kb: u64,
}

/// Pure parser for the `/proc/self/status` text. Lines look like
/// `VmRSS:\t   12345 kB`; we take the first integer after the colon.
/// Returns `None` if none of the fields we care about were present (e.g.
/// non-Linux, or a malformed read) so callers can degrade gracefully.
pub fn parse_status(content: &str) -> Option<ProcMem> {
    let mut m = ProcMem::default();
    let mut saw_any = false;
    for line in content.lines() {
        let (key, rest) = match line.split_once(':') {
            Some(kv) => kv,
            None => continue,
        };
        let val_kb: u64 = match rest.split_whitespace().next().and_then(|n| n.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        match key.trim() {
            "VmRSS" => { m.vm_rss_kb = val_kb; saw_any = true; }
            "VmHWM" => { m.vm_hwm_kb = val_kb; saw_any = true; }
            "VmData" => { m.vm_data_kb = val_kb; saw_any = true; }
            "VmSize" => { m.vm_size_kb = val_kb; saw_any = true; }
            _ => {}
        }
    }
    if saw_any { Some(m) } else { None }
}

/// Read this process's memory from `/proc/self/status`. `None` on platforms
/// without procfs.
pub fn proc_mem() -> Option<ProcMem> {
    let content = std::fs::read_to_string("/proc/self/status").ok()?;
    parse_status(&content)
}

/// Is footprint logging switched on? Cached after the first read so the poll
/// path stays a single atomic load.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| parse_enabled(std::env::var("WEB2QML_MEMLOG").ok().as_deref()))
}

/// Pure helper for [`enabled`] so the truthiness rule is testable.
pub fn parse_enabled(v: Option<&str>) -> bool {
    match v {
        None => false,
        Some(s) => matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        ),
    }
}

/// kB → human MB string, one decimal (e.g. 41234 → "40.3MB").
pub fn fmt_mb(kb: u64) -> String {
    format!("{:.1}MB", kb as f64 / 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_core_fields() {
        let s = "Name:\tweb2qml\nVmPeak:\t  900000 kB\nVmSize:\t  800000 kB\n\
                 VmHWM:\t   50000 kB\nVmRSS:\t   42000 kB\nVmData:\t  300000 kB\n";
        let m = parse_status(s).unwrap();
        assert_eq!(m.vm_rss_kb, 42000);
        assert_eq!(m.vm_hwm_kb, 50000);
        assert_eq!(m.vm_data_kb, 300000);
        assert_eq!(m.vm_size_kb, 800000);
    }

    #[test]
    fn none_when_no_known_fields() {
        assert!(parse_status("Name:\tx\nThreads:\t4\n").is_none());
        assert!(parse_status("").is_none());
    }

    #[test]
    fn tolerates_odd_spacing_and_partial() {
        // only RSS present, no space before the number
        let m = parse_status("VmRSS:1234 kB\n").unwrap();
        assert_eq!(m.vm_rss_kb, 1234);
        assert_eq!(m.vm_hwm_kb, 0); // absent fields stay 0
    }

    #[test]
    fn enabled_truthiness() {
        assert!(parse_enabled(Some("1")));
        assert!(parse_enabled(Some("true")));
        assert!(parse_enabled(Some("ON")));
        assert!(parse_enabled(Some(" yes ")));
        assert!(!parse_enabled(Some("0")));
        assert!(!parse_enabled(Some("off")));
        assert!(!parse_enabled(Some("")));
        assert!(!parse_enabled(None));
    }

    #[test]
    fn fmt_mb_rounds() {
        assert_eq!(fmt_mb(1024), "1.0MB");
        assert_eq!(fmt_mb(0), "0.0MB");
        assert_eq!(fmt_mb(1536), "1.5MB");
    }
}
