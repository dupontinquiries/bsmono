// gitmind-core/src/watcher.rs
//
// The realtime time-machine engine. Watches the working tree, and whenever
// a burst of writes to policy-matched files settles, takes a checkpoint
// snapshot — into the SHADOW ref namespace, so your visible history never
// churns (the "no realtime log updates" constraint). Each autosnapshot is
// provenance-tagged, so a change made while an AI agent is reporting itself
// (GITMIND_AGENT) is attributable later.
//
// Design for testability + robustness:
//   - `WatchPolicy` (watch.rs) decides which paths matter.
//   - a debounce window coalesces editor save-storms into one snapshot.
//   - the loop is driven by the `notify` crate (inotify/FSEvents/etc.) but
//     polls a stop channel so it shuts down cleanly and can be unit-tested
//     by spawning it, touching a file, and signalling stop.
//
// PID-level attribution of arbitrary writers needs fanotify (root); inotify
// (what `notify` uses) reports the change but not the writer, so provenance
// here comes from the self-reported GITMIND_AGENT / this process.

use crate::backend::{GitCli, RepoBackend};
use crate::checkpoint::{Checkpoint, CheckpointEngine};
use crate::gmignore::GmIgnore;
use crate::provenance;
use crate::watch::WatchPolicy;
use notify::{Config, EventKind, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WatchStats {
    pub events: u64,
    pub snapshots: u64,
}

/// Run the watcher until `stop` receives a message (or its sender drops).
/// Calls `on_snapshot` for each checkpoint taken. Uses the OS-native backend
/// (inotify/FSEvents). Blocking.
pub fn run(
    repo: &GitCli,
    policy: &WatchPolicy,
    session: &str,
    stop: Receiver<()>,
    on_snapshot: impl FnMut(&Checkpoint),
) -> Result<WatchStats, String> {
    run_with_mode(repo, policy, session, false, stop, on_snapshot)
}

/// As `run`, but `poll = true` selects notify's PollWatcher (periodic
/// directory scan) instead of the native backend. The native backend is
/// faster and lower-latency, but inotify is unavailable or silently broken
/// in some sandboxes/containers and on some network filesystems; polling
/// works anywhere at the cost of a scan interval.
pub fn run_with_mode(
    repo: &GitCli,
    policy: &WatchPolicy,
    session: &str,
    poll: bool,
    stop: Receiver<()>,
    mut on_snapshot: impl FnMut(&Checkpoint),
) -> Result<WatchStats, String> {
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let handler = move |res| {
        let _ = tx.send(res);
    };
    // Both backends implement the Watcher trait; box so the loop is identical.
    let mut watcher: Box<dyn Watcher> = if poll {
        Box::new(
            PollWatcher::new(handler, Config::default().with_poll_interval(Duration::from_secs(2)))
                .map_err(|e| format!("poll watcher init: {e}"))?,
        )
    } else {
        Box::new(
            RecommendedWatcher::new(handler, Config::default())
                .map_err(|e| format!("watcher init: {e}"))?,
        )
    };
    watcher
        .watch(repo.root(), RecursiveMode::Recursive)
        .map_err(|e| format!("watch {}: {e}", repo.root().display()))?;

    // Smart fanning: load .gmignore once and restrict the watched filespace.
    let ignore = GmIgnore::load(repo.root());

    let engine = CheckpointEngine::new(repo, session);
    let window = Duration::from_millis(policy.debounce_ms.max(50));
    let mut stats = WatchStats::default();
    let mut dirty = false;
    let mut last_change = Instant::now();

    loop {
        // Cooperative shutdown.
        match stop.try_recv() {
            Ok(()) | Err(TryRecvError::Disconnected) => {
                // Final flush of any pending change before exiting.
                if dirty {
                    if let Some(c) = snapshot(&engine)? {
                        stats.snapshots += 1;
                        on_snapshot(&c);
                    }
                }
                break;
            }
            Err(TryRecvError::Empty) => {}
        }

        // Wait briefly for the next filesystem event.
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(event)) => {
                if event_is_relevant(&event, policy, repo, &ignore) {
                    dirty = true;
                    last_change = Instant::now();
                    stats.events += 1;
                }
            }
            Ok(Err(_)) => {} // a watcher error event; ignore and keep going
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        // Fire once the burst has settled.
        if dirty && last_change.elapsed() >= window {
            if let Some(c) = snapshot(&engine)? {
                stats.snapshots += 1;
                on_snapshot(&c);
            }
            dirty = false;
        }
    }
    Ok(stats)
}

/// Convenience for the CLI: watch forever (until the process is killed).
pub fn run_forever(
    repo: &GitCli,
    policy: &WatchPolicy,
    session: &str,
    on_snapshot: impl FnMut(&Checkpoint),
) -> Result<WatchStats, String> {
    let (tx, rx) = mpsc::channel();
    // Keep the sender alive for the process lifetime so the loop never sees
    // a disconnect; Ctrl-C terminates the process.
    std::mem::forget(tx);
    run(repo, policy, session, rx, on_snapshot)
}

fn snapshot(engine: &CheckpointEngine) -> Result<Option<Checkpoint>, String> {
    // Only attach provenance when a process actually self-reports (an AI
    // agent set GITMIND_AGENT). Otherwise the watcher would just tag itself
    // as the writer, which is noise — inotify can't see the real editor.
    let label = if std::env::var("GITMIND_AGENT").map(|v| !v.is_empty()).unwrap_or(false) {
        provenance::tag_label("autosave")
    } else {
        "autosave".to_string()
    };
    engine.snapshot(&label)
}

/// True if the event touches a path the policy watches and isn't VCS noise.
fn event_is_relevant(event: &notify::Event, policy: &WatchPolicy, repo: &GitCli, ignore: &GmIgnore) -> bool {
    if !matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) {
        return false;
    }
    let root = repo.root();
    for p in &event.paths {
        // Skip our own machinery.
        let s = p.to_string_lossy();
        if s.contains("/.git/") || s.contains("/.gitmind/") {
            continue;
        }
        // Match against the policy using the path relative to the repo.
        let rel = p.strip_prefix(root).unwrap_or(p).to_string_lossy().to_string();
        // Smart fanning: honor .gmignore before anything else.
        if ignore.ignored(&rel) {
            continue;
        }
        if policy.watches(&rel) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relevance_filters_by_policy_and_vcs() {
        // Build a throwaway repo just to get a GitCli rooted somewhere.
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git").arg("-C").arg(dir.path()).args(["init", "-q"]).status().unwrap();
        let repo = GitCli::discover(dir.path()).unwrap();
        let policy = WatchPolicy::default();
        let ignore = crate::gmignore::GmIgnore::from_str("ignored_dir/\n*.tmp\n");

        let ev = |path: &str, kind: EventKind| notify::Event {
            kind,
            paths: vec![dir.path().join(path)],
            attrs: Default::default(),
        };
        assert!(event_is_relevant(&ev("src/main.rs", EventKind::Modify(notify::event::ModifyKind::Any)), &policy, &repo, &ignore));
        assert!(!event_is_relevant(&ev("photo.jpg", EventKind::Modify(notify::event::ModifyKind::Any)), &policy, &repo, &ignore));
        assert!(!event_is_relevant(&ev(".git/index", EventKind::Modify(notify::event::ModifyKind::Any)), &policy, &repo, &ignore));
        // Access events (reads) never trigger snapshots.
        assert!(!event_is_relevant(&ev("src/main.rs", EventKind::Access(notify::event::AccessKind::Read)), &policy, &repo, &ignore));
        // .gmignore restricts the filespace even for watched types.
        assert!(!event_is_relevant(&ev("ignored_dir/x.rs", EventKind::Modify(notify::event::ModifyKind::Any)), &policy, &repo, &ignore));
    }
}
