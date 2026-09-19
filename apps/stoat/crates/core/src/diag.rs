//! Structured diagnostics. The point borrowed from rustc is not the ASCII art but
//! that a message is data (code, primary location, notes, help) built where the
//! context is still available, and rendered later by whichever shell is attached.

use crate::model::*;
use std::fmt::Write as _;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub title: String,
    pub at: Option<(PathBuf, u32)>,
    pub notes: Vec<String>,
    pub help: Vec<String>,
}

impl Diagnostic {
    fn new(severity: Severity, code: &str, title: String) -> Self {
        Diagnostic { severity, code: code.into(), title, at: None, notes: Vec::new(), help: Vec::new() }
    }

    pub fn render(&self, context_lines: u32) -> String {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        };
        let mut s = format!("{sev}[{}]: {}\n", self.code, self.title);
        if let Some((file, line)) = &self.at {
            let _ = writeln!(s, "  --> {}:{line}", file.display());
            if let Ok(src) = std::fs::read_to_string(file) {
                let lo = line.saturating_sub(context_lines).max(1);
                let hi = line + context_lines;
                let w = hi.to_string().len();
                let _ = writeln!(s, "{:w$} |", "");
                for (n, text) in src.lines().enumerate().map(|(i, t)| (i as u32 + 1, t)).filter(|(n, _)| (lo..=hi).contains(n)) {
                    let mark = if n == *line { ">" } else { " " };
                    let _ = writeln!(s, "{n:w$} |{mark} {text}");
                }
                let _ = writeln!(s, "{:w$} |", "");
            }
        }
        for n in &self.notes {
            let _ = writeln!(s, "  = note: {n}");
        }
        for h in &self.help {
            let _ = writeln!(s, "  = help: {h}");
        }
        s
    }
}

fn fname(f: Option<&Frame>) -> String {
    f.and_then(|f| f.func.clone()).unwrap_or_else(|| "??".into())
}

pub fn for_stop(stop: &Stop, frames: &[Frame], blame: usize, fault_addr: Option<u64>, rust_panic: bool) -> Diagnostic {
    let top = frames.first().or(stop.frame.as_ref());
    let blamed = frames.get(blame).or(top);
    let mut d = match &stop.reason {
        StopReason::Signal { name, meaning } => {
            let mut d = Diagnostic::new(
                if stop.is_crash() { Severity::Error } else { Severity::Warning },
                name,
                format!("{} in `{}`", meaning.to_lowercase(), fname(blamed)),
            );
            match (name.as_str(), fault_addr) {
                ("SIGSEGV" | "SIGBUS", Some(a)) if a < 4096 => d.notes.push(format!(
                    "fault address {a:#x} is in the null page: a null pointer was dereferenced{}",
                    if a == 0 { String::new() } else { format!(" at offset {a} (a field or index off a null base)") }
                )),
                ("SIGSEGV" | "SIGBUS", Some(a)) => d.notes.push(format!("fault address {a:#x}")),
                ("SIGABRT", _) => d.help.push(
                    "abort() is usually an assert, a Rust panic under panic=abort, or g_error(); the message is in the program output just before this".into(),
                ),
                _ => {}
            }
            d
        }
        StopReason::Exited { code: 0 } => Diagnostic::new(Severity::Info, "exit", "program exited normally".into()),
        StopReason::Exited { code } => {
            let mut d = Diagnostic::new(Severity::Warning, "exit", format!("program exited with code {code}"));
            if *code == 101 {
                d.notes.push("101 is the exit code of a Rust panic that unwound out of main".into());
                d.help.push("rerun with the panic breakpoint armed (`__rustc::rust_panic`; plain `rust_panic` on older toolchains) to stop where it happens".into());
            }
            d
        }
        StopReason::ExitedSignalled { name } => Diagnostic::new(Severity::Error, name, format!("program was killed by {name}")),
        StopReason::Breakpoint { .. } if rust_panic => {
            let mut d = Diagnostic::new(Severity::Error, "panic", format!("Rust panic in `{}`", fname(blamed)));
            d.notes.push("the panic message is in the program output above; the process is stopped before unwinding, so every frame is still intact".into());
            d
        }
        StopReason::Breakpoint { id } => Diagnostic::new(Severity::Info, "break", format!("breakpoint {id} hit in `{}`", fname(blamed))),
        StopReason::Watchpoint { id } => Diagnostic::new(Severity::Info, "watch", format!("watchpoint {id} triggered in `{}`", fname(blamed))),
        StopReason::StepDone => Diagnostic::new(Severity::Info, "step", format!("stepped to `{}`", fname(blamed))),
        StopReason::FunctionFinished => Diagnostic::new(Severity::Info, "finish", format!("returned to `{}`", fname(blamed))),
        StopReason::Other(r) => Diagnostic::new(Severity::Info, "stop", format!("stopped ({r}) in `{}`", fname(blamed))),
    };
    if let Some(f) = blamed {
        if let (Some(file), Some(line)) = (&f.file, f.line) {
            d.at = Some((file.clone(), line));
        }
    }
    if blame != 0 && rust_panic {
        d.notes.push(format!("frames 0..{} are the panic machinery in std; showing frame {blame}", blame - 1));
    } else if blame != 0 && !stop.is_exit() {
        d.notes.push(format!(
            "stopped in `{}` (frame 0), which has no source here; showing frame {blame}, the innermost one that does",
            fname(top)
        ));
    }
    if !frames.is_empty() && !frames.iter().any(|f| f.line.is_some()) {
        d.help.push("no frame has line info: the binary was built without debug info (-g, or `debug = true` in the Cargo profile)".into());
    }
    d
}

/// Turn one of gdb's error strings into something with a next step. The strings
/// are matched loosely because they are not a stable interface.
pub fn for_error(msg: &str, names_in_scope: &[String]) -> Diagnostic {
    let mut d = Diagnostic::new(Severity::Error, "gdb", msg.trim_end_matches('.').to_string());
    if let Some(sym) = msg.strip_prefix("No symbol \"").and_then(|r| r.split('"').next()) {
        d.code = "no-symbol".into();
        if msg.contains("in current context") {
            let mut close: Vec<_> =
                names_in_scope.iter().map(|n| (edit_distance(n, sym), n)).filter(|(dist, n)| *dist <= 1 + n.len() / 4).collect();
            close.sort();
            if let Some((_, n)) = close.first() {
                d.help.push(format!("a variable with a similar name is in scope: `{n}`"));
            }
            d.notes.push("names resolve in the selected frame; a local of a caller needs that frame selected".into());
            d.notes.push("in optimized builds a variable can also be absent from debug info entirely".into());
        }
    } else if msg.contains("No symbol table") || msg.contains("no debugging symbols") {
        d.code = "no-debuginfo".into();
        d.help.push("Cargo: `debug = true` under the profile you built; C/C++: compile with -g".into());
        d.help.push("for distro libraries, enable debuginfod or install the -dbg/-debuginfo package".into());
    } else if let Some(addr) = msg.strip_prefix("Cannot access memory at address ").and_then(|a| stoat_mi::parse_u64(a.trim())) {
        d.code = "bad-address".into();
        if addr < 4096 {
            d.notes.push(format!("{addr:#x} is in the null page: the pointer this came from is null"));
        }
    } else if msg.contains("optimized out") {
        d.code = "optimized-out".into();
        d.notes.push("the compiler kept no location for this value at the current pc".into());
        d.help.push("step to a line that uses it, select the caller's frame, or rebuild with opt-level 0/1".into());
    } else if msg.contains("Operation not permitted") && msg.contains("ptrace") {
        d.code = "ptrace-denied".into();
        d.help.push("kernel.yama.ptrace_scope=1 only allows tracing children: launch the program from here, or lower it via sysctl".into());
        d.help.push("inside Flatpak, host processes are not reachable at all; run the debugger on the host".into());
    } else if msg.contains("not being run") {
        d.code = "not-running".into();
        d.help.push("start it with run".into());
    }
    d
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}
