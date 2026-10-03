//! Silence COSMIC's own brightness (and volume) pop-up while tideshell drives the backlight.
//!
//! `cosmic-osd` draws its slider whenever `cosmic-settings-daemon`'s DisplayBrightness changes, and the
//! daemon reports every backlight write -- including tideshell's auto-brightness. The session respawns
//! `cosmic-osd` within a second of a kill and a user cannot shadow `/usr/bin/cosmic-osd`, so it is
//! stopped instead: SIGSTOP leaves it alive (nothing to respawn) but unable to draw. Its D-Bus signals
//! queue; if the broker ever overflows that queue it disconnects the process, the session starts a
//! fresh one, and the next scan stops that. Cost: COSMIC's own volume/layout pop-ups go quiet too.
//! `TIDESHELL_QUIET_OSD=0` turns it off (and `kill -CONT` revives a stopped one).

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

/// Stop every `cosmic-osd` of this user that is running. Returns how many were stopped.
pub fn quiet() -> usize {
    let me = std::fs::metadata("/proc/self").map(|m| std::os::unix::fs::MetadataExt::uid(&m)).unwrap_or(u32::MAX);
    let mut stopped = 0;
    let Ok(dir) = std::fs::read_dir("/proc") else { return 0 };
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
        if std::fs::read_to_string(p.join("stat")).is_ok_and(|s| needs_stop(&s))
            && std::process::Command::new("kill").args(["-STOP", pid]).status().is_ok_and(|s| s.success())
        {
            stopped += 1;
        }
    }
    stopped
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
    fn only_a_live_unstopped_process_needs_stopping() {
        assert!(needs_stop("1 (cosmic-osd) S 1"));
        assert!(needs_stop("1 (cosmic-osd) R 1"));
        assert!(!needs_stop("1 (cosmic-osd) T 1"), "already stopped");
        assert!(!needs_stop("1 (cosmic-osd) Z 1"), "a zombie cannot be stopped, and is not drawing");
        assert!(!needs_stop("nonsense"), "unreadable: leave it");
    }
}
