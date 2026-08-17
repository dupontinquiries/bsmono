//! watch — noticing that the set of installed applications changed.
//!
//! Polling directory stamps rather than inotify, for three reasons: no new
//! dependency in a fleet that has almost none; the question is only ever
//! asked at a moment the user is waiting anyway (the launcher being
//! summoned), so a few `stat` calls are free; and a poll is legible — you can
//! see exactly what it compares, and `probe` can print it.
//!
//! Each watched directory contributes a (mtime, entry count) pair. mtime
//! alone would be enough on a modern filesystem, but coarse timestamps exist
//! and a same-second install-then-summon is exactly the case that matters, so
//! the count is a second opinion. What this deliberately does NOT catch is an
//! edit to an existing .desktop file that leaves the directory alone —
//! `Launcher::refresh` remains available for anyone who needs certainty.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq)]
struct Stamp {
    dir: PathBuf,
    mtime: Option<SystemTime>,
    entries: usize,
}

fn stamp(dir: &Path) -> Stamp {
    let mtime = std::fs::metadata(dir).and_then(|m| m.modified()).ok();
    let entries = std::fs::read_dir(dir).map(|r| r.count()).unwrap_or(0);
    Stamp { dir: dir.to_path_buf(), mtime, entries }
}

/// Watches a fixed set of directories, one level deep (vendor subdirectories
/// like `applications/kde4/` hold entries too).
#[derive(Debug, Default)]
pub struct DirWatch {
    stamps: Vec<Stamp>,
}

impl DirWatch {
    /// Take an initial snapshot. A freshly created watch never reports a
    /// change on its first `changed()` — the caller has just scanned.
    pub fn new(dirs: &[PathBuf]) -> DirWatch {
        DirWatch { stamps: snapshot(dirs) }
    }

    /// True if any watched directory gained, lost, or re-dated an entry since
    /// the last call. Updates the snapshot, so a change is reported once.
    pub fn changed(&mut self, dirs: &[PathBuf]) -> bool {
        let now = snapshot(dirs);
        if now == self.stamps {
            return false;
        }
        self.stamps = now;
        true
    }
}

fn snapshot(dirs: &[PathBuf]) -> Vec<Stamp> {
    let mut out = Vec::new();
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        out.push(stamp(dir));
        // One level down: vendor subdirectories are scanned, so they are
        // watched. Deeper nesting is legal but vanishingly rare, and each
        // level costs a readdir on every summon.
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut subs: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            subs.sort(); // read_dir order is not stable; comparison needs it
            for sub in subs {
                out.push(stamp(&sub));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("hatch-watch-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn a_fresh_watch_reports_no_change() {
        let d = tmp("fresh");
        std::fs::write(d.join("a.desktop"), "x").unwrap();
        let dirs = vec![d.clone()];
        let mut w = DirWatch::new(&dirs);
        assert!(!w.changed(&dirs), "nothing has happened yet");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn installing_and_removing_an_app_is_noticed_once_each() {
        let d = tmp("install");
        let dirs = vec![d.clone()];
        let mut w = DirWatch::new(&dirs);

        std::fs::write(d.join("new.desktop"), "x").unwrap();
        assert!(w.changed(&dirs), "an install is a change");
        assert!(!w.changed(&dirs), "and is reported exactly once");

        std::fs::remove_file(d.join("new.desktop")).unwrap();
        assert!(w.changed(&dirs), "so is an uninstall");
        assert!(!w.changed(&dirs));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_vendor_subdirectory_is_watched_too() {
        let d = tmp("vendor");
        let sub = d.join("kde4");
        std::fs::create_dir_all(&sub).unwrap();
        let dirs = vec![d.clone()];
        let mut w = DirWatch::new(&dirs);

        std::fs::write(sub.join("old.desktop"), "x").unwrap();
        assert!(w.changed(&dirs), "entries one level down count");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_directory_appearing_or_vanishing_is_a_change() {
        let base = tmp("appear");
        let missing = base.join("not-yet");
        let dirs = vec![missing.clone()];
        let mut w = DirWatch::new(&dirs);
        assert!(!w.changed(&dirs), "still absent");

        std::fs::create_dir_all(&missing).unwrap();
        assert!(w.changed(&dirs), "a new applications dir appeared");
        let _ = std::fs::remove_dir_all(&base);
    }
}
