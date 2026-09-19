//! Processes worth offering for attach: this user's, with a command line, not
//! ourselves. Read from /proc so it works on a phone image that has no `ps`
//! worth parsing (busybox's differs from procps').

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    pub exe: Option<PathBuf>,
    pub cmdline: String,
}

/// Newest first, by pid. Pids wrap, so this is recency only roughly; it puts the
/// thing you just started near the top, which is the one you want.
pub fn list(proc_root: &Path, uid: u32, skip_pid: u32) -> Vec<Proc> {
    let Ok(rd) = std::fs::read_dir(proc_root) else { return Vec::new() };
    let mut v: Vec<Proc> = rd
        .flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            if pid == skip_pid || e.metadata().ok()?.uid() != uid {
                return None;
            }
            let dir = e.path();
            // Kernel threads have an empty cmdline; nothing to attach to.
            let raw = std::fs::read(dir.join("cmdline")).ok()?;
            let cmdline = raw.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect::<Vec<_>>().join(" ");
            if cmdline.is_empty() {
                return None;
            }
            let name = std::fs::read_to_string(dir.join("comm")).map(|s| s.trim().to_string()).unwrap_or_default();
            Some(Proc { pid, name, exe: std::fs::read_link(dir.join("exe")).ok(), cmdline })
        })
        .collect();
    v.sort_by(|a, b| b.pid.cmp(&a.pid));
    v
}

pub fn list_mine() -> Vec<Proc> {
    // SAFETY: getuid cannot fail and takes no arguments.
    let uid = unsafe { libc::getuid() };
    list(Path::new("/proc"), uid, std::process::id())
}

/// Yama's ptrace_scope. 0: any same-uid process. 1: only descendants of the
/// tracer, so attach to an already-running program fails with EPERM - which is
/// the default on most distributions and needs saying BEFORE the attempt.
pub fn ptrace_scope(proc_root: &Path) -> Option<u8> {
    std::fs::read_to_string(proc_root.join("sys/kernel/yama/ptrace_scope")).ok()?.trim().parse().ok()
}

pub fn attach_caveat(scope: Option<u8>) -> Option<&'static str> {
    match scope? {
        0 => None,
        1 => Some("kernel.yama.ptrace_scope=1: only programs started by the debugger can be attached to. Launch it from here, or `sysctl kernel.yama.ptrace_scope=0`."),
        _ => Some("kernel.yama.ptrace_scope>=2: attach needs CAP_SYS_PTRACE or is disabled outright."),
    }
}
