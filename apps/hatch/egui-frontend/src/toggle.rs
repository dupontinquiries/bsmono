//! toggle — how a hardware key reaches a running launcher.
//!
//! `single_instance` (uikit-egui) answers "is one running?"; this answers
//! "then tell it to show/hide". The mechanism is a poke file next to the lock:
//! the second invocation writes it and exits, the running instance polls its
//! mtime a few times a second and flips visibility on change. A file rather
//! than a socket or signal for the same reasons the lock is a file — per-user
//! by construction, survives nothing past the session, and debuggable with
//! `ls` when a shortcut "does nothing".
//!
//! Binding it on Plasma: point a global shortcut at `hatch-egui --toggle`.

use std::path::PathBuf;
use std::time::SystemTime;

pub fn toggle_path(app_id: &str) -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir());
    base.join(format!("{app_id}.toggle"))
}

/// Poke the running instance. Best-effort: a failed write is reported so the
/// caller can fall back to starting fresh.
pub fn poke(app_id: &str) -> std::io::Result<()> {
    let p = toggle_path(app_id);
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    // Content is irrelevant; mtime is the message. Writing the nanos anyway
    // makes consecutive pokes distinct filesystems with coarse mtimes.
    std::fs::write(&p, format!("{:?}", SystemTime::now()))
}

/// Watches the poke file from inside the running instance.
#[derive(Default)]
pub struct ToggleWatch {
    last: Option<SystemTime>,
    primed: bool,
}

impl ToggleWatch {
    /// True exactly when the file's mtime moved since the last call. The
    /// first call PRIMES rather than fires — a poke file left over from a
    /// previous session must not hide the window the moment it opens.
    pub fn poked(&mut self, app_id: &str) -> bool {
        let mtime = std::fs::metadata(toggle_path(app_id))
            .and_then(|m| m.modified())
            .ok();
        if !self.primed {
            self.primed = true;
            self.last = mtime;
            return false;
        }
        if mtime.is_some() && mtime != self.last {
            self.last = mtime;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(tag: &str) -> String {
        format!("test.hatch.toggle.{tag}.{}", std::process::id())
    }

    #[test]
    fn a_poke_fires_the_watcher_exactly_once() {
        let id = id("once");
        let _ = std::fs::remove_file(toggle_path(&id));
        let mut w = ToggleWatch::default();
        assert!(!w.poked(&id), "no file yet");
        poke(&id).unwrap();
        assert!(w.poked(&id), "the poke is seen");
        assert!(!w.poked(&id), "and not seen twice");
        let _ = std::fs::remove_file(toggle_path(&id));
    }

    #[test]
    fn a_stale_file_from_a_previous_session_does_not_fire_on_startup() {
        let id = id("stale");
        poke(&id).unwrap();
        let mut w = ToggleWatch::default(); // "session start" happens after the file exists
        assert!(!w.poked(&id), "first observation primes, never fires");
        poke(&id).unwrap();
        assert!(w.poked(&id), "a fresh poke after priming fires");
        let _ = std::fs::remove_file(toggle_path(&id));
    }
}
