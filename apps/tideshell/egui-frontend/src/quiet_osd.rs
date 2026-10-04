//! Silence COSMIC's own brightness (and volume) pop-up while tideshell drives the backlight.
//!
//! `cosmic-osd` draws its slider whenever `cosmic-settings-daemon`'s DisplayBrightness changes, and the
//! daemon reports every backlight write -- including tideshell's auto-brightness. The session respawns
//! `cosmic-osd` within a second of a kill and a user cannot shadow `/usr/bin/cosmic-osd`, so it is
//! stopped instead: SIGSTOP leaves it alive (nothing to respawn) but unable to draw. Its D-Bus signals
//! queue; if the broker ever overflows that queue it disconnects the process, the session starts a
//! fresh one, and the next scan stops that. Cost: COSMIC's own volume/layout pop-ups go quiet too.
//! `TIDESHELL_QUIET_OSD=0` turns it off (and `kill -CONT` revives a stopped one).
//!
//! A stopped process cannot hide its own pop-up, so stopping one that is DRAWING one freezes the pill on the screen
//! for good (seen on the phone after a reboot: "27%" stuck over every app). Two rules keep that from happening:
//! the first scan after tideshell starts KILLS the running instance instead (it may well be showing the pill
//! tideshell's own start-up brightness write just raised) and stops the respawn, and the scan runs every second, so a
//! respawn is stopped before a brightness change can make it draw.

/// The state letter of a `/proc/<pid>/stat` line (after the LAST ')': the name may hold anything).
pub fn state_of(stat: &str) -> Option<char> {
    stat[stat.rfind(')')? + 1..].trim_start().chars().next()
}

/// A process in any of these states is either already stopped or gone.
pub fn needs_stop(stat: &str) -> bool {
    !matches!(state_of(stat), Some('T') | Some('t') | Some('Z') | Some('X') | None)
}

pub fn enabled() -> bool {
    std::env::var("TIDESHELL_QUIET_OSD").as_deref() != Ok("0")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Stop,
    /// A fresh instance replaces it, drawing nothing.
    Kill,
}

/// What to do with a `cosmic-osd` whose `/proc/<pid>/stat` is `stat`. `first_scan`: this is the first look since
/// tideshell started, so the process has been running unwatched and may be showing a pill.
pub fn decide(stat: &str, first_scan: bool) -> Option<Move> {
    needs_stop(stat).then_some(if first_scan { Move::Kill } else { Move::Stop })
}

/// Silence every `cosmic-osd` of this user that is running. Returns (stopped, killed).
pub fn quiet(first_scan: bool) -> (usize, usize) {
    let me = std::fs::metadata("/proc/self").map(|m| std::os::unix::fs::MetadataExt::uid(&m)).unwrap_or(u32::MAX);
    let (mut stopped, mut killed) = (0, 0);
    let Ok(dir) = std::fs::read_dir("/proc") else { return (0, 0) };
    for e in dir.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str().filter(|n| n.bytes().all(|b| b.is_ascii_digit())) else { continue };
        let p = e.path();
        if std::fs::read_to_string(p.join("comm")).map(|c| c.trim() != "cosmic-osd").unwrap_or(true) {
            continue;
        }
        if std::fs::metadata(&p).map(|m| std::os::unix::fs::MetadataExt::uid(&m) != me).unwrap_or(true) {
            continue;
        }
        let Some(mv) = std::fs::read_to_string(p.join("stat")).ok().and_then(|s| decide(&s, first_scan)) else { continue };
        let sig = if mv == Move::Kill { "-KILL" } else { "-STOP" };
        if std::process::Command::new("kill").args([sig, pid]).status().is_ok_and(|s| s.success()) {
            if mv == Move::Kill { killed += 1 } else { stopped += 1 }
        }
    }
    (stopped, killed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_comes_from_after_the_last_paren() {
        assert_eq!(state_of("123 (cosmic-osd) S 1 2 3"), Some('S'));
        assert_eq!(state_of("123 (we) ird) T 1 2"), Some('T'), "a ')' in the name does not fool it");
        assert_eq!(state_of("garbage"), None);
    }

    #[test]
    fn the_first_scan_kills_a_running_one_that_may_be_drawing_and_later_scans_only_stop() {
        assert_eq!(decide("1 (cosmic-osd) S 1", true), Some(Move::Kill), "unwatched since boot: it may be showing a pill");
        assert_eq!(decide("1 (cosmic-osd) S 1", false), Some(Move::Stop));
        for first in [true, false] {
            assert_eq!(decide("1 (cosmic-osd) T 1", first), None, "already stopped: a kill would only start a new one");
            assert_eq!(decide("1 (cosmic-osd) Z 1", first), None);
        }
    }

    #[test]
    fn only_a_live_unstopped_process_needs_stopping() {
        assert!(needs_stop("1 (cosmic-osd) S 1"));
        assert!(needs_stop("1 (cosmic-osd) R 1"));
        assert!(!needs_stop("1 (cosmic-osd) T 1"), "already stopped");
        assert!(!needs_stop("1 (cosmic-osd) Z 1"), "a zombie cannot be stopped, and is not drawing");
        assert!(!needs_stop("nonsense"), "unreadable: leave it");
    }
}
