//! v0.1.664: debug-mode detection → gdb/monitor picker (U1).
//!
//! Hector manually stands up tracers and debuggers and toggles them. This
//! NOTICES "you're debugging this" from terminal output (a crash, a panic, a
//! kernel oops, a debugger launch) and offers the next move — "re-run under
//! gdb", "open the core dump", "backtrace" — as a fuzzy voice-pickable list
//! routed through the assist-bubble (never auto-runs).
//!
//! PURE + testable: signal detection, option generation, and the fuzzy
//! picker are here; spawning gdb is the wiring layer.

/// A detected debugging-relevant event, from a line of terminal output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugSignal {
    /// a process crashed (SIGSEGV/SIGABRT/etc.), optional binary name
    Crash { signal: String, binary: Option<String> },
    /// a userspace panic (Rust panic, C++ terminate), with a location
    Panic { location: Option<String> },
    /// a kernel oops/BUG/panic (relevant for Hector's kernel work)
    KernelFault { kind: String },
    /// a core dump was written
    CoreDump,
    /// a debugger was launched (gdb/lldb) on a target
    DebuggerStarted { tool: String, target: Option<String> },
}

/// Detect a debug signal from one line of terminal output. None if the line
/// isn't debug-relevant. Matches well-known crash/panic/oops formats.
pub fn detect_debug_signal(line: &str) -> Option<DebugSignal> {
    let l = line.trim();
    let low = l.to_lowercase();

    // userspace crashes
    if low.contains("segmentation fault") {
        return Some(DebugSignal::Crash { signal: "SIGSEGV".into(), binary: None });
    }
    if low.contains("bus error") {
        return Some(DebugSignal::Crash { signal: "SIGBUS".into(), binary: None });
    }
    if low.contains("aborted") && low.contains("core dumped") {
        return Some(DebugSignal::Crash { signal: "SIGABRT".into(), binary: None });
    }
    // glibc heap errors → abort
    if low.contains("double free") || low.contains("free(): ")
        || low.contains("malloc(): ") || low.contains("stack smashing detected") {
        return Some(DebugSignal::Crash { signal: "heap/abort".into(), binary: None });
    }
    // Rust panic
    if low.contains("panicked at") {
        // location is the text after "panicked at " up to a trailing ':'
        let loc = l.split("panicked at").nth(1)
            .map(|s| s.trim().trim_end_matches(':').trim().to_string())
            .filter(|s| !s.is_empty());
        return Some(DebugSignal::Panic { location: loc });
    }
    if low.contains("terminate called after throwing") {
        return Some(DebugSignal::Panic { location: None });
    }
    // kernel faults (Hector's domain)
    if low.contains("kernel panic") {
        return Some(DebugSignal::KernelFault { kind: "panic".into() });
    }
    if low.starts_with("bug:") || low.contains("kernel null pointer") {
        return Some(DebugSignal::KernelFault { kind: "BUG".into() });
    }
    if low.contains("oops:") || low.contains("] oops") {
        return Some(DebugSignal::KernelFault { kind: "oops".into() });
    }
    // core dump line on its own
    if low.contains("core dumped") {
        return Some(DebugSignal::CoreDump);
    }
    // debugger launch — a command line starting with gdb/lldb
    for tool in ["gdb", "lldb", "cuda-gdb"] {
        if low == tool || low.starts_with(&format!("{} ", tool)) {
            let target = l.split_whitespace().nth(1)
                .filter(|t| !t.starts_with('-'))
                .map(|t| t.to_string());
            return Some(DebugSignal::DebuggerStarted { tool: tool.into(), target });
        }
    }
    None
}

/// A pickable next-action offered when a debug signal fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorOption {
    /// short label shown in the picker / spoken to match against
    pub label: String,
    /// the command this would run (NOT auto-run — user picks)
    pub command: String,
}

/// Generate the offered options for a signal, given the last command run
/// (so "re-run under gdb" can reconstruct the invocation).
pub fn monitor_options(sig: &DebugSignal, last_cmd: &str) -> Vec<MonitorOption> {
    let mut out = Vec::new();
    let bin = first_token(last_cmd);
    match sig {
        DebugSignal::Crash { .. } => {
            if !last_cmd.is_empty() {
                out.push(MonitorOption {
                    label: "re-run under gdb".into(),
                    command: format!("gdb --args {}", last_cmd),
                });
            }
            out.push(MonitorOption { label: "open the core dump".into(),
                command: format!("gdb {} core", bin) });
            out.push(MonitorOption { label: "backtrace".into(),
                command: format!("gdb -batch -ex run -ex bt --args {}", last_cmd) });
        }
        DebugSignal::Panic { .. } => {
            if !last_cmd.is_empty() {
                out.push(MonitorOption { label: "re-run with backtrace".into(),
                    command: format!("RUST_BACKTRACE=1 {}", last_cmd) });
                out.push(MonitorOption { label: "re-run with full backtrace".into(),
                    command: format!("RUST_BACKTRACE=full {}", last_cmd) });
            }
        }
        DebugSignal::KernelFault { .. } => {
            out.push(MonitorOption { label: "show the kernel log".into(),
                command: "dmesg | tail -50".into() });
            out.push(MonitorOption { label: "watch the kernel log".into(),
                command: "dmesg -w".into() });
        }
        DebugSignal::CoreDump => {
            out.push(MonitorOption { label: "open the core dump".into(),
                command: format!("gdb {} core", bin) });
        }
        DebugSignal::DebuggerStarted { target, .. } => {
            // already debugging — offer common monitor setups
            let t = target.clone().unwrap_or_default();
            out.push(MonitorOption { label: "break at main".into(),
                command: format!("break main") });
            if !t.is_empty() {
                out.push(MonitorOption { label: "watch a register".into(),
                    command: "info registers".into() });
            }
        }
    }
    out
}

fn first_token(s: &str) -> &str { s.split_whitespace().next().unwrap_or("") }

/// Fuzzy-pick an option from a voiced phrase ("re-run it under gdb" →
/// "re-run under gdb"). Token-overlap score; returns the best index over a
/// bar, else None.
pub fn pick_option(voiced: &str, options: &[MonitorOption]) -> Option<usize> {
    let vt = tokens(voiced);
    if vt.is_empty() || options.is_empty() { return None; }
    let mut best: Option<(usize, f32)> = None;
    for (i, opt) in options.iter().enumerate() {
        let ot = tokens(&opt.label);
        if ot.is_empty() { continue; }
        let overlap = ot.iter().filter(|t| vt.contains(*t)).count() as f32;
        let score = overlap / ot.len() as f32;
        if score > 0.0 && best.map(|(_, s)| score > s).unwrap_or(true) {
            best = Some((i, score));
        }
    }
    best.filter(|(_, s)| *s >= 0.5).map(|(i, _)| i)
}

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_lowercase())
        .collect()
}

/// assist-bubble kind for a signal (dedups so the same crash doesn't
/// re-fire the picker).
pub fn bubble_kind(sig: &DebugSignal) -> &'static str {
    match sig {
        DebugSignal::Crash { .. } => "debug:crash",
        DebugSignal::Panic { .. } => "debug:panic",
        DebugSignal::KernelFault { .. } => "debug:kernel",
        DebugSignal::CoreDump => "debug:core",
        DebugSignal::DebuggerStarted { .. } => "debug:gdb",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_segfault() {
        let s = detect_debug_signal("Segmentation fault (core dumped)");
        assert!(matches!(s, Some(DebugSignal::Crash { .. })));
    }

    #[test]
    fn detects_rust_panic_with_location() {
        let s = detect_debug_signal("thread 'main' panicked at src/main.rs:42:5:");
        match s {
            Some(DebugSignal::Panic { location: Some(loc) }) =>
                assert!(loc.contains("src/main.rs:42:5"), "got {}", loc),
            other => panic!("expected Panic with location, got {:?}", other),
        }
    }

    #[test]
    fn detects_kernel_faults() {
        assert!(matches!(detect_debug_signal("Kernel panic - not syncing: Fatal exception"),
            Some(DebugSignal::KernelFault { .. })));
        assert!(matches!(detect_debug_signal("BUG: kernel NULL pointer dereference"),
            Some(DebugSignal::KernelFault { .. })));
        assert!(matches!(detect_debug_signal("[ 1234.56] Oops: 0000 [#1] SMP"),
            Some(DebugSignal::KernelFault { .. })));
    }

    #[test]
    fn detects_heap_errors() {
        assert!(matches!(detect_debug_signal("free(): double free detected in tcache 2"),
            Some(DebugSignal::Crash { .. })));
        assert!(matches!(detect_debug_signal("*** stack smashing detected ***: terminated"),
            Some(DebugSignal::Crash { .. })));
    }

    #[test]
    fn detects_debugger_launch() {
        match detect_debug_signal("gdb ./tracer") {
            Some(DebugSignal::DebuggerStarted { tool, target }) => {
                assert_eq!(tool, "gdb");
                assert_eq!(target.as_deref(), Some("./tracer"));
            }
            other => panic!("got {:?}", other),
        }
    }

    #[test]
    fn ignores_normal_output() {
        assert_eq!(detect_debug_signal("Compiling web2qml v0.1.0"), None);
        assert_eq!(detect_debug_signal("test result: ok. 5 passed"), None);
        assert_eq!(detect_debug_signal("hello world"), None);
    }

    #[test]
    fn crash_options_include_gdb_rerun() {
        let sig = DebugSignal::Crash { signal: "SIGSEGV".into(), binary: None };
        let opts = monitor_options(&sig, "./tracer --verbose");
        assert!(opts.iter().any(|o| o.command.contains("gdb --args ./tracer --verbose")));
        assert!(opts.iter().any(|o| o.label.contains("core dump")));
    }

    #[test]
    fn panic_options_set_backtrace() {
        let sig = DebugSignal::Panic { location: None };
        let opts = monitor_options(&sig, "cargo run");
        assert!(opts.iter().any(|o| o.command.contains("RUST_BACKTRACE=1 cargo run")));
    }

    #[test]
    fn fuzzy_pick_option() {
        let opts = monitor_options(&DebugSignal::Crash { signal: "SIGSEGV".into(), binary: None }, "./a");
        // "let's re-run it under gdb" → the "re-run under gdb" option
        let idx = pick_option("let's re-run it under gdb", &opts).unwrap();
        assert!(opts[idx].label.contains("re-run under gdb"));
        // "open the core" → core dump option
        let idx2 = pick_option("open the core dump", &opts).unwrap();
        assert!(opts[idx2].label.contains("core dump"));
    }

    #[test]
    fn fuzzy_pick_rejects_unrelated() {
        let opts = monitor_options(&DebugSignal::Crash { signal: "SIGSEGV".into(), binary: None }, "./a");
        assert_eq!(pick_option("make me a sandwich", &opts), None);
    }

    #[test]
    fn kind_dedups_by_signal_type() {
        let a = DebugSignal::Crash { signal: "SIGSEGV".into(), binary: None };
        let b = DebugSignal::Crash { signal: "SIGABRT".into(), binary: None };
        assert_eq!(bubble_kind(&a), bubble_kind(&b));
    }
}
