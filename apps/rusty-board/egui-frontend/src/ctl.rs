//! ctl — the keyboard's control socket: `$XDG_RUNTIME_DIR/rusty-board.ctl`,
//! one Unix datagram per command, plain text.
//!
//! Why a socket: the egui keyboard serves no D-Bus (only the GTK build does),
//! so the shell had no way to reach it. The shell needs three things from
//! the keyboard, all about voice typing with gloves on:
//!
//! * `cold on` / `cold off` — tideshell's cold mode. A keyboard shown while
//!   cold starts voice typing by itself: gloves cannot type on glass.
//! * `voice-done` / `voice-cancel` — glove flicks (tideshell's gloveflick.rs)
//!   finishing or throwing away the utterance.
//! * `voice-start` / `voice-toggle` — from the shell's palette or keys.
//!
//! The same datagram shape as tideshell's own `tideshell.ctl`, so one tool
//! (`socat - UNIX-SENDTO:$XDG_RUNTIME_DIR/rusty-board.ctl`) drives both.
//! Unknown commands are ignored and counted, never fatal.
//!
//! `reload-settings` / `reload-layouts` come from the companion app
//! (`rusty-board-settings`) after it writes `settings.conf` or a layout file,
//! so a running keyboard picks the change up without a restart.

use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    VoiceStart,
    VoiceDone,
    VoiceCancel,
    VoiceToggle,
    Cold(bool),
    ReloadSettings,
    ReloadLayouts,
    /// An app's command strip (`keyboard_strip`): show it, or clear it.
    Strip(keyboard_strip::Msg),
}

pub fn parse(s: &str) -> Option<Cmd> {
    match s.trim() {
        "voice-start" => Some(Cmd::VoiceStart),
        "voice-done" => Some(Cmd::VoiceDone),
        "voice-cancel" => Some(Cmd::VoiceCancel),
        "voice-toggle" => Some(Cmd::VoiceToggle),
        "cold on" => Some(Cmd::Cold(true)),
        "cold off" => Some(Cmd::Cold(false)),
        "reload-settings" => Some(Cmd::ReloadSettings),
        "reload-layouts" => Some(Cmd::ReloadLayouts),
        other => keyboard_strip::decode(other).map(Cmd::Strip),
    }
}

pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("rusty-board.ctl"))
}

/// Send one command to a running keyboard. `Err` when none is listening
/// (no socket, or nobody bound to it), which a caller can show as
/// "applies next time the keyboard starts" rather than as a failure.
pub fn send(cmd: &str) -> Result<(), String> {
    let path = socket_path().ok_or("no XDG_RUNTIME_DIR")?;
    let tx = UnixDatagram::unbound().map_err(|e| e.to_string())?;
    tx.send_to(cmd.as_bytes(), &path).map(|_| ()).map_err(|e| format!("{}: {e}", path.display()))
}

pub struct Ctl {
    sock: UnixDatagram,
    path: PathBuf,
    pub unknown: u32,
}

impl Ctl {
    /// Bind the socket, replacing a stale one (a crashed keyboard leaves the
    /// file behind; the single-instance lock already guarantees we are the
    /// only live keyboard). `None` when there is no runtime dir or bind fails.
    pub fn bind() -> Option<Ctl> {
        Self::bind_at(socket_path()?)
    }

    pub fn bind_at(path: PathBuf) -> Option<Ctl> {
        let _ = std::fs::remove_file(&path);
        let sock = match UnixDatagram::bind(&path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("ctl: cannot bind {}: {e}", path.display());
                return None;
            }
        };
        sock.set_nonblocking(true).ok()?;
        Some(Ctl { sock, path, unknown: 0 })
    }

    pub fn fd(&self) -> std::os::fd::RawFd {
        use std::os::fd::AsRawFd;
        self.sock.as_raw_fd()
    }

    /// Everything queued, oldest first. Non-blocking.
    pub fn drain(&mut self) -> Vec<Cmd> {
        let mut out = Vec::new();
        // A strip declaration is several lines; 256 bytes cut it off.
        let mut buf = [0u8; 8192];
        while let Ok(n) = self.sock.recv(&mut buf) {
            match std::str::from_utf8(&buf[..n]).ok().and_then(parse) {
                Some(c) => out.push(c),
                None => self.unknown += 1,
            }
        }
        out
    }
}

impl Drop for Ctl {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_round_trip_through_a_real_socket_and_junk_is_counted() {
        let dir = std::env::temp_dir().join(format!("rb-ctl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rusty-board.ctl");
        let mut ctl = Ctl::bind_at(path.clone()).unwrap();
        let tx = UnixDatagram::unbound().unwrap();
        for m in ["cold on", "voice-done", "nonsense", "voice-cancel"] {
            tx.send_to(m.as_bytes(), &path).unwrap();
        }
        assert_eq!(ctl.drain(), vec![Cmd::Cold(true), Cmd::VoiceDone, Cmd::VoiceCancel]);
        assert_eq!(ctl.unknown, 1);
        assert!(ctl.drain().is_empty(), "non-blocking when empty");
        drop(ctl);
        assert!(!path.exists(), "the socket file goes with the keyboard");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_is_exact_and_trims() {
        assert_eq!(parse(" voice-toggle\n"), Some(Cmd::VoiceToggle));
        assert_eq!(parse("cold off"), Some(Cmd::Cold(false)));
        assert_eq!(parse("cold"), None);
        assert_eq!(parse("reload-layouts"), Some(Cmd::ReloadLayouts));
        let show = keyboard_strip::Msg::Show { app: "aperture".into(), pid: 7, actions: vec![keyboard_strip::Action::new("Back", "Alt+Left", "")] };
        assert_eq!(parse(&keyboard_strip::encode(&show)), Some(Cmd::Strip(show)));
        assert_eq!(parse("reload-settings\n"), Some(Cmd::ReloadSettings));
    }
}
