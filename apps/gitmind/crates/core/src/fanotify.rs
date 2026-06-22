// gitmind-core/src/fanotify.rs
//
// The privileged realtime watcher. Unlike inotify (watcher.rs), fanotify
// reports the PID that caused each event — so we learn the ACTUAL process
// that wrote a file and attribute the change to it (AI agent vs build tool
// vs human editor) instead of relying on a self-reported env var.
//
// Cost of that power: fanotify_init needs CAP_SYS_ADMIN (root or
// `setcap cap_sys_admin+ep`), and it's Linux-only. So this lives behind the
// `fanotify` feature; the default build and non-Linux targets are
// unaffected and fall back to the inotify watcher.
//
// Approach: mark the MOUNT the repo lives on for FAN_CLOSE_WRITE | FAN_MODIFY
// (recursive coverage of the whole mount), read events, resolve each event's
// fd to a path via /proc/self/fd, attribute via the PID, and filter to the
// repo + watch policy. Snapshots carry the real writer's origin.

#[cfg(all(target_os = "linux", feature = "fanotify"))]
mod imp {
    use crate::backend::{GitCli, RepoBackend};
    use crate::checkpoint::{Checkpoint, CheckpointEngine};
    use crate::provenance::{self, Origin, ProcessInfo};
    use crate::watch::WatchPolicy;
    use crate::watcher::WatchStats;
    use std::os::fd::RawFd;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{Receiver, TryRecvError};
    use std::time::{Duration, Instant};

    const FANOTIFY_METADATA_VERSION: u8 = 3;

    /// A single attributed write event.
    #[derive(Clone, Debug)]
    pub struct FanEvent {
        pub path: PathBuf,
        pub pid: u32,
    }

    pub struct FanotifyWatcher {
        fd: RawFd,
    }

    impl FanotifyWatcher {
        /// Initialize fanotify and mark the mount containing `path` for
        /// write/close-write events. Requires CAP_SYS_ADMIN.
        pub fn mark_mount(path: &Path) -> Result<Self, String> {
            let fd = unsafe {
                libc::fanotify_init(
                    libc::FAN_CLOEXEC | libc::FAN_CLASS_NOTIF | libc::FAN_NONBLOCK,
                    libc::O_RDONLY as u32,
                )
            };
            if fd < 0 {
                let e = std::io::Error::last_os_error();
                return Err(format!(
                    "fanotify_init failed ({e}) — needs CAP_SYS_ADMIN (run as root or setcap cap_sys_admin+ep)"
                ));
            }
            let cpath = std::ffi::CString::new(path.as_os_str().to_string_lossy().as_bytes())
                .map_err(|e| e.to_string())?;
            let rc = unsafe {
                libc::fanotify_mark(
                    fd,
                    libc::FAN_MARK_ADD | libc::FAN_MARK_MOUNT,
                    (libc::FAN_CLOSE_WRITE | libc::FAN_MODIFY) as u64,
                    libc::AT_FDCWD,
                    cpath.as_ptr(),
                )
            };
            if rc < 0 {
                let e = std::io::Error::last_os_error();
                unsafe { libc::close(fd) };
                return Err(format!("fanotify_mark failed: {e}"));
            }
            Ok(FanotifyWatcher { fd })
        }

        /// Poll for up to `timeout_ms`, then drain available events.
        pub fn next_events(&self, timeout_ms: i32) -> Result<Vec<FanEvent>, String> {
            let mut pfd = libc::pollfd { fd: self.fd, events: libc::POLLIN, revents: 0 };
            let pr = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
            if pr <= 0 {
                return Ok(Vec::new()); // timeout or interrupted
            }

            // 8-aligned buffer: fanotify records are 8-aligned.
            #[repr(C, align(8))]
            struct Buf([u8; 8192]);
            let mut buf = Buf([0u8; 8192]);
            let n = unsafe {
                libc::read(self.fd, buf.0.as_mut_ptr() as *mut libc::c_void, buf.0.len())
            };
            if n <= 0 {
                return Ok(Vec::new());
            }
            let n = n as usize;

            let meta_size = std::mem::size_of::<libc::fanotify_event_metadata>();
            let mut out = Vec::new();
            let mut off = 0usize;
            while n - off >= meta_size {
                // SAFETY: buffer is 8-aligned and we verified enough bytes remain.
                let meta = unsafe {
                    &*(buf.0.as_ptr().add(off) as *const libc::fanotify_event_metadata)
                };
                if meta.vers != FANOTIFY_METADATA_VERSION {
                    break; // version mismatch — bail rather than misparse
                }
                let elen = meta.event_len as usize;
                if elen < meta_size || off + elen > n {
                    break;
                }
                if meta.fd >= 0 {
                    if let Some(path) = path_of_fd(meta.fd) {
                        out.push(FanEvent { path, pid: meta.pid as u32 });
                    }
                    unsafe { libc::close(meta.fd) };
                }
                off += elen;
            }
            Ok(out)
        }
    }

    impl Drop for FanotifyWatcher {
        fn drop(&mut self) {
            unsafe { libc::close(self.fd) };
        }
    }

    /// Resolve an event fd to its path via /proc/self/fd/<fd>.
    fn path_of_fd(fd: i32) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/self/fd/{fd}")).ok()
    }

    /// Run the fanotify watcher with real PID attribution. Mirrors
    /// watcher::run, but the snapshot label reflects the ACTUAL writing
    /// process (e.g. "[ai-agent:claude] autosave"), determined per event.
    pub fn run(
        repo: &GitCli,
        policy: &WatchPolicy,
        session: &str,
        stop: Receiver<()>,
        mut on_snapshot: impl FnMut(&Checkpoint),
    ) -> Result<WatchStats, String> {
        let watcher = FanotifyWatcher::mark_mount(repo.root())?;
        let engine = CheckpointEngine::new(repo, session);
        let window = Duration::from_millis(policy.debounce_ms.max(50));
        let root = repo.root().to_path_buf();
        let ignore = crate::gmignore::GmIgnore::load(repo.root());

        let mut stats = WatchStats::default();
        let mut dirty = false;
        let mut last_change = Instant::now();
        // Track the most informative writer seen in the current burst.
        let mut writer: Option<ProcessInfo> = None;

        loop {
            match stop.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => {
                    if dirty {
                        if let Some(c) = take_snapshot(&engine, &writer)? {
                            stats.snapshots += 1;
                            on_snapshot(&c);
                        }
                    }
                    break;
                }
                Err(TryRecvError::Empty) => {}
            }

            for ev in watcher.next_events(100)? {
                // Only events under the repo, matching the policy.
                let rel = match ev.path.strip_prefix(&root) {
                    Ok(r) => r.to_string_lossy().to_string(),
                    Err(_) => continue, // elsewhere on the mount
                };
                let s = ev.path.to_string_lossy();
                if s.contains("/.git/") || s.contains("/.gitmind/") {
                    continue;
                }
                if ignore.ignored(&rel) {
                    continue; // smart fanning: .gmignore restricts the filespace
                }
                if !policy.watches(&rel) {
                    continue;
                }
                dirty = true;
                last_change = Instant::now();
                stats.events += 1;
                // Keep the most meaningful attribution (an agent/editor beats
                // an unknown/our own process).
                if let Some(info) = ProcessInfo::from_pid(ev.pid) {
                    writer = Some(choose_writer(writer.take(), info));
                }
            }

            if dirty && last_change.elapsed() >= window {
                if let Some(c) = take_snapshot(&engine, &writer)? {
                    stats.snapshots += 1;
                    on_snapshot(&c);
                }
                dirty = false;
                writer = None;
            }
        }
        Ok(stats)
    }

    /// Prefer the more attributable writer (AI agent / editor) over noise
    /// (our own process, build tooling, unknown).
    fn choose_writer(prev: Option<ProcessInfo>, cur: ProcessInfo) -> ProcessInfo {
        fn rank(o: Origin) -> u8 {
            match o {
                Origin::AiAgent => 4,
                Origin::Editor => 3,
                Origin::BuildTool => 2,
                Origin::Shell => 1,
                Origin::VersionControl | Origin::Unknown => 0,
            }
        }
        match prev {
            Some(p) if rank(p.origin) >= rank(cur.origin) => p,
            _ => cur,
        }
    }

    fn take_snapshot(
        engine: &CheckpointEngine,
        writer: &Option<ProcessInfo>,
    ) -> Result<Option<Checkpoint>, String> {
        let label = match writer {
            Some(w) if w.origin != Origin::Unknown && w.origin != Origin::VersionControl => {
                format!("[{}:{}] autosave", w.origin.as_str(), w.comm)
            }
            // Fall back to env self-report if the PID wasn't informative.
            _ => provenance::tag_label_if_agent("autosave"),
        };
        engine.snapshot(&label)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn captures_write_with_real_pid() {
            let dir = tempfile::tempdir().unwrap();
            let w = match FanotifyWatcher::mark_mount(dir.path()) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("skipping fanotify test (not permitted here): {e}");
                    return;
                }
            };
            let file = dir.path().join("hello.txt");
            std::fs::write(&file, b"hello fanotify").unwrap();

            let mut hit: Option<FanEvent> = None;
            for _ in 0..30 {
                for ev in w.next_events(100).unwrap() {
                    if ev.path == file {
                        hit = Some(ev);
                        break;
                    }
                }
                if hit.is_some() {
                    break;
                }
            }
            let ev = hit.expect("fanotify should have captured our write");
            // The PID that wrote the file is THIS process.
            assert_eq!(ev.pid, std::process::id(), "fanotify reported the writer PID");
        }
    }
}

#[cfg(all(target_os = "linux", feature = "fanotify"))]
pub use imp::{run, FanEvent, FanotifyWatcher};

// Fallback when the feature/OS isn't available: a clear error so callers can
// degrade to the inotify watcher.
#[cfg(not(all(target_os = "linux", feature = "fanotify")))]
pub mod imp_stub {
    use crate::backend::GitCli;
    use crate::checkpoint::Checkpoint;
    use crate::watch::WatchPolicy;
    use crate::watcher::WatchStats;
    use std::sync::mpsc::Receiver;

    pub fn run(
        _repo: &GitCli,
        _policy: &WatchPolicy,
        _session: &str,
        _stop: Receiver<()>,
        _on_snapshot: impl FnMut(&Checkpoint),
    ) -> Result<WatchStats, String> {
        Err("fanotify support not built (enable the `fanotify` feature on Linux)".into())
    }
}

#[cfg(not(all(target_os = "linux", feature = "fanotify")))]
pub use imp_stub::run;
