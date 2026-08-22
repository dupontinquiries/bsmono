//! memlog — a periodic memory-footprint line, off unless asked for.
//!
//! v0.1.896 in `main.qml`, and the one version marker present there and absent
//! from this port. A 15-second timer calls the backend, which **no-ops unless
//! `WEB2QML_MEMLOG` is set** — so the cost in normal use is an env check and an
//! early return, and when enabled it emits a `[mem] …` line for a later
//! optimisation pass.
//!
//! ## Why the gate is checked first and cached
//!
//! The QML's own note is that the tick "costs a single env check + early
//! return in normal use". `mem_footprint::enabled()` caches in a `OnceLock`, so
//! the steady-state cost is an atomic load — which matters because this runs
//! forever on a phone, and a feature nobody enabled should not read
//! `/proc/self/status` 5,760 times a day.
//!
//! ## Why the counts are counts and not byte totals
//!
//! Straight from the app's implementation: *"Counts only where bytes would mean
//! an O(n) walk of large strings — the mem logger should not itself allocate
//! big buffers each tick."* A memory logger that allocates to measure memory
//! reports its own overhead as the program's.
//!
//! The reader is `voiceterm::mem_footprint`, std-only, included by path.

#[path = "../../src/mem_footprint.rs"]
#[allow(dead_code, unused_imports)]
pub mod mem_footprint;

pub use mem_footprint::{enabled, fmt_mb, parse_enabled, parse_status, proc_mem, ProcMem};

/// How often to sample. 15s, as in the QML.
///
/// Not tunable: a knob on a diagnostic that is already behind an env var is a
/// second thing to get wrong, and the interval only matters relative to how
/// long a leak takes to show — which is minutes, not seconds.
pub const INTERVAL_SECS: f64 = 15.0;

/// What the frontend can cheaply count.
///
/// Counts, not bytes, for the reason above. Every field here is an O(1) `len()`
/// on something the app already holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Holders {
    pub terminals: usize,
    pub scrollback_rows: usize,
    pub workspaces: usize,
    pub cards: usize,
    pub blocks: usize,
    /// Bytes, because this one IS O(1): a flat buffer's capacity.
    pub audio_kb: usize,
}

/// Whether enough time has passed to sample again.
///
/// Takes the clock rather than reading it, so the schedule is testable — a
/// timer that can only be observed by waiting fifteen seconds is one nobody
/// writes a test for.
pub fn due(now_s: f64, last_s: f64) -> bool {
    now_s - last_s >= INTERVAL_SECS
}

/// Build the line. `None` when logging is off.
///
/// Returns the string rather than printing it, so a caller can route it — the
/// QML pairs this with `WEB2QML_LOG=mem` to send it to a file, and a function
/// that hardcodes stderr cannot be routed anywhere.
pub fn line(h: Holders) -> Option<String> {
    if !enabled() { return None; }
    let pm = proc_mem().unwrap_or_default();
    Some(format!(
        // hwm as well as rss: the peak is what decides whether this fits on a
        // 4 GiB handset, and a sample taken between two spikes shows neither.
        "[mem] rss {} hwm {} data {} | terms {} rows {} ws {} cards {} blocks {} audio {}k",
        fmt_mb(pm.vm_rss_kb), fmt_mb(pm.vm_hwm_kb), fmt_mb(pm.vm_data_kb),
        h.terminals, h.scrollback_rows, h.workspaces, h.cards, h.blocks, h.audio_kb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interval_matches_the_qml() {
        // 15s. Changing it silently would make two builds' logs
        // non-comparable, which is the entire use of the feature.
        assert_eq!(INTERVAL_SECS, 15.0);
    }

    #[test]
    fn the_schedule_is_testable_without_waiting() {
        // A timer that can only be observed by waiting fifteen seconds is one
        // nobody writes a test for — so the clock is an argument.
        assert!(!due(10.0, 0.0));
        assert!(due(15.0, 0.0));
        assert!(due(100.0, 84.0));
        assert!(!due(100.0, 90.0));
    }

    #[test]
    fn a_clock_that_goes_backwards_does_not_spin() {
        // Suspend and resume move a monotonic-looking clock in surprising
        // ways on a handset. A negative delta must not read as "overdue".
        assert!(!due(5.0, 100.0));
    }

    #[test]
    fn the_gate_is_off_unless_the_variable_says_otherwise() {
        // The QML's promise: no-ops unless WEB2QML_MEMLOG is set, so the tick
        // costs an env check in normal use. parse_enabled is the pure half and
        // this pins its shape.
        assert!(!parse_enabled(None));
        assert!(!parse_enabled(Some("")));
        assert!(!parse_enabled(Some("0")));
        for on in ["1", "true", "on", "yes", " YES "] {
            assert!(parse_enabled(Some(on)), "{on:?} should enable it");
        }
    }

    #[test]
    fn the_line_is_returned_rather_than_printed() {
        // The QML routes it with WEB2QML_LOG=mem. A function that hardcodes
        // stderr cannot be routed anywhere, and this is a diagnostic whose
        // whole point is going into a file for later.
        let src = include_str!("memlog.rs");
        for probe in [format!("e{}!(", "println"), format!("{}!(", "println")] {
            assert!(!src.contains(probe.as_str()), "memlog prints instead of returning");
        }
    }

    #[test]
    fn holders_are_counts_not_byte_walks() {
        // The app's own reasoning: "Counts only where bytes would mean an O(n)
        // walk of large strings — the mem logger should not itself allocate
        // big buffers each tick." A logger that allocates to measure memory
        // reports its own overhead as the program's.
        //
        // Every field is usize and the only byte figure is audio, which is a
        // flat buffer's capacity and therefore O(1).
        let h = Holders { audio_kb: 64, ..Default::default() };
        assert_eq!(h.audio_kb, 64);
        let src = include_str!("memlog.rs");
        let walk = format!(".{}().map(", "values");
        assert!(!src.contains(walk.as_str()), "a holder count walks a collection");
    }

    #[test]
    fn parsing_a_real_proc_status_gives_both_figures() {
        // The reader is shared with the app, so this checks the port did not
        // reimplement it — same parse, same fields.
        let s = "Name:\tvoiceterm\nVmRSS:\t  123456 kB\nVmSize:\t 9876543 kB\n";
        let pm = parse_status(s).expect("a normal /proc/self/status did not parse");
        assert_eq!(pm.vm_rss_kb, 123456);
        assert_eq!(pm.vm_size_kb, 9876543);
    }
}
