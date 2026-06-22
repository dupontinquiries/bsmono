// gitmind-core/src/watch_session.rs
//
// The Qt-free controller behind the GUI's watch toggle. It owns the
// background watcher thread and a queue of checkpoints the UI drains on a
// timer. Keeping this in core (not in the Qt Backend) means the exact
// start → drain → stop → running sequence the QML relies on is unit-tested
// without needing Qt, and the GUI Backend becomes a thin JSON wrapper.

use crate::backend::GitCli;
use crate::checkpoint::Checkpoint;
use crate::watch::WatchPolicy;
use crate::watcher;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};

#[derive(Default)]
struct SessionState {
    /// (oid, label) checkpoints recorded since the last drain. The special
    /// oid "error" carries a human-readable failure in `label`.
    pending: Vec<(String, String)>,
    running: bool,
    stop: Option<mpsc::Sender<()>>,
}

#[derive(Clone, Default)]
pub struct WatchSession {
    shared: Arc<Mutex<SessionState>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrainResult {
    pub running: bool,
    pub items: Vec<(String, String)>,
}

impl WatchSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start watching `repo_root` on a background thread. No-op if already
    /// running. `fanotify` selects the privileged PID-attributing backend
    /// (any init failure is surfaced via `drain` as an ("error", msg) item,
    /// so the UI can show it and retry without fanotify). `poll` forces the
    /// polling backend.
    pub fn start(
        &self,
        repo_root: PathBuf,
        policy: WatchPolicy,
        session: String,
        fanotify: bool,
        poll: bool,
    ) {
        let mut g = self.shared.lock().unwrap();
        if g.running {
            return;
        }
        let (tx, rx) = mpsc::channel();
        g.stop = Some(tx);
        g.running = true;
        drop(g);

        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let push = |c: &Checkpoint| {
                if let Ok(mut g) = shared.lock() {
                    g.pending.push((c.oid.clone(), c.label.clone()));
                }
            };
            let result = match GitCli::discover(&repo_root) {
                Some(r) => {
                    if fanotify {
                        crate::fanotify::run(&r, &policy, &session, rx, push)
                    } else {
                        watcher::run_with_mode(&r, &policy, &session, poll, rx, push)
                    }
                }
                None => Err("not a git repository".to_string()),
            };
            if let Ok(mut g) = shared.lock() {
                g.running = false;
                g.stop = None;
                if let Err(e) = &result {
                    g.pending.push(("error".into(), format!("watch stopped: {e}")));
                }
            }
        });
    }

    /// Signal the watcher thread to stop. Returns immediately; `running`
    /// flips to false once the thread winds down.
    pub fn stop(&self) {
        let mut g = self.shared.lock().unwrap();
        if let Some(tx) = g.stop.take() {
            let _ = tx.send(());
        }
    }

    /// Take everything recorded since the last drain plus the live state.
    pub fn drain(&self) -> DrainResult {
        let mut g = self.shared.lock().unwrap();
        let items = g.pending.drain(..).collect();
        DrainResult { running: g.running, items }
    }

    pub fn running(&self) -> bool {
        self.shared.lock().map(|g| g.running).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::Duration;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let ok = Command::new("git").arg("-C").arg(dir).args(args).status().unwrap().success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn start_drain_stop_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "t@t"]);
        git(dir.path(), &["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.rs"), "fn main() {}\n").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-qm", "init"]);

        let s = WatchSession::new();
        let mut policy = WatchPolicy::default();
        policy.debounce_ms = 250;
        s.start(dir.path().to_path_buf(), policy, "main".into(), false, false);
        assert!(s.running(), "session should report running after start");

        std::thread::sleep(Duration::from_millis(500)); // let the watcher arm
        std::fs::write(dir.path().join("a.rs"), "fn main() { /* edit */ }\n").unwrap();

        // Drain on a cadence like the QML timer until a checkpoint appears.
        let mut saw_checkpoint = false;
        for _ in 0..25 {
            std::thread::sleep(Duration::from_millis(150));
            let d = s.drain();
            if d.items.iter().any(|(oid, _)| oid != "error") {
                saw_checkpoint = true;
                break;
            }
        }
        assert!(saw_checkpoint, "expected a drained checkpoint from a real edit");

        s.stop();
        // Wait for the thread to wind down.
        let mut stopped = false;
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(100));
            if !s.running() {
                stopped = true;
                break;
            }
        }
        assert!(stopped, "session should report not-running after stop");
    }

    #[test]
    fn double_start_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "t@t"]);
        git(dir.path(), &["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.rs"), "x\n").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-qm", "init"]);

        let s = WatchSession::new();
        s.start(dir.path().to_path_buf(), WatchPolicy::default(), "main".into(), false, false);
        s.start(dir.path().to_path_buf(), WatchPolicy::default(), "main".into(), false, false);
        assert!(s.running());
        s.stop();
    }
}
