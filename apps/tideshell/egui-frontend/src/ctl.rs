//! The control socket: how the compositor (and anything else in the
//! session) hands tideshell a command. cosmic-comp owns the power key: it
//! gets it from libinput like every other key and runs the command bound to
//! it in `com.system76.CosmicSettings.Shortcuts/v1/custom`. Bound to
//! `tideshell ctl power-key`, that command lands here. Same route as the
//! volume keys, which is why those worked while the power key did not: the
//! compositor handled them, and tideshell's evdev reader could not open the
//! power key's device.
//!
//! A Unix datagram socket at `$XDG_RUNTIME_DIR/tideshell.ctl`; one datagram
//! per command, plain text. Only the session's user can reach the runtime
//! dir, which is the access control.

use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    PowerKey,
    VolumeUp,
    VolumeDown,
    Lock,
    Palette,
    Switcher,
}

impl Command {
    pub fn parse(s: &str) -> Option<Command> {
        match s.trim() {
            "power-key" => Some(Command::PowerKey),
            "volume-up" => Some(Command::VolumeUp),
            "volume-down" => Some(Command::VolumeDown),
            "lock" => Some(Command::Lock),
            "palette" => Some(Command::Palette),
            "switcher" => Some(Command::Switcher),
            _ => None,
        }
    }

    pub const NAMES: [&'static str; 6] = ["power-key", "volume-up", "volume-down", "lock", "palette", "switcher"];
}

/// The keys cosmic-comp should hand to tideshell, as (keysym name, command).
pub const COMPOSITOR_KEYS: [(&str, &str); 3] = [("XF86PowerOff", "power-key"), ("XF86AudioRaiseVolume", "volume-up"), ("XF86AudioLowerVolume", "volume-down")];

/// Where cosmic reads user shortcuts that override its defaults.
pub fn cosmic_custom_shortcuts(config_home: &Path) -> PathBuf {
    config_home.join("cosmic/com.system76.CosmicSettings.Shortcuts/v1/custom")
}

/// `custom` is a bare RON map (the Shortcuts type is serde-transparent),
/// entries of the form `(modifiers: [], key: "XF86PowerOff"): Spawn("..."),`.
/// Merge ours in, keeping the user's other bindings and replacing any
/// existing binding of the same keys. Returns the new text.
pub fn merge_custom_shortcuts(existing: &str, bin: &str) -> String {
    let mut body: Vec<String> = Vec::new();
    let inner = existing.trim().trim_start_matches('{').trim_end_matches('}');
    // One entry per line in files cosmic-settings writes; keep those lines
    // whose key is not one of ours.
    for line in inner.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if COMPOSITOR_KEYS.iter().any(|(k, _)| line.contains(&format!("key: \"{k}\""))) {
            continue;
        }
        body.push(format!("    {}", line.trim_end_matches(',').to_string() + ","));
    }
    for (key, cmd) in COMPOSITOR_KEYS {
        body.push(format!("    (modifiers: [], key: \"{key}\"): Spawn(\"{bin} ctl {cmd}\"),"));
    }
    format!("{{\n{}\n}}\n", body.join("\n"))
}

/// Whether the compositor route is installed (the custom file binds the
/// power key to `tideshell ctl power-key`).
pub fn compositor_route_installed(config_home: &Path) -> bool {
    std::fs::read_to_string(cosmic_custom_shortcuts(config_home)).map(|t| t.contains("ctl power-key")).unwrap_or(false)
}

pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|r| PathBuf::from(r).join("tideshell.ctl"))
}

pub struct Server {
    sock: UnixDatagram,
    path: PathBuf,
}

impl Server {
    /// Bind (replacing a stale socket from a crashed run), non-blocking.
    pub fn bind(path: &Path) -> std::io::Result<Server> {
        let _ = std::fs::remove_file(path);
        let sock = UnixDatagram::bind(path)?;
        sock.set_nonblocking(true)?;
        Ok(Server { sock, path: path.to_path_buf() })
    }

    /// Every command waiting; unknown text is returned as Err for the trace.
    pub fn poll(&self) -> Vec<Result<Command, String>> {
        let mut out = Vec::new();
        let mut buf = [0u8; 256];
        while let Ok(n) = self.sock.recv(&mut buf) {
            let text = String::from_utf8_lossy(&buf[..n]).into_owned();
            out.push(Command::parse(&text).ok_or(text));
        }
        out
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The client side: `tideshell ctl power-key`. Returns a message for stderr
/// on failure (tideshell not running, unknown command).
pub fn send(path: &Path, cmd: &str) -> Result<(), String> {
    if Command::parse(cmd).is_none() {
        return Err(format!("unknown command '{cmd}' (one of: {})", Command::NAMES.join(", ")));
    }
    let s = UnixDatagram::unbound().map_err(|e| e.to_string())?;
    s.send_to(cmd.as_bytes(), path).map_err(|e| format!("{}: {e} (is tideshell running?)", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_shortcuts_merge_keeps_the_users_bindings_and_replaces_ours() {
        let user = "{\n    (modifiers: [Super], key: \"t\"): Spawn(\"cosmic-term\"),\n    (modifiers: [], key: \"XF86PowerOff\"): System(PowerOff),\n}\n";
        let m = merge_custom_shortcuts(user, "/home/user/.local/bin/tideshell");
        assert!(m.contains("(modifiers: [Super], key: \"t\"): Spawn(\"cosmic-term\"),"), "{m}");
        assert!(!m.contains("System(PowerOff)"), "the old power binding is replaced");
        assert!(m.contains("(modifiers: [], key: \"XF86PowerOff\"): Spawn(\"/home/user/.local/bin/tideshell ctl power-key\"),"));
        assert!(m.contains("XF86AudioLowerVolume\"): Spawn(\"/home/user/.local/bin/tideshell ctl volume-down\")"));
        assert!(m.starts_with("{\n") && m.trim_end().ends_with('}'));
        let twice = merge_custom_shortcuts(&m, "/home/user/.local/bin/tideshell");
        assert_eq!(twice, m, "idempotent");
        let fresh = merge_custom_shortcuts("", "tideshell");
        assert_eq!(fresh.matches("Spawn(").count(), 3);
        let base = std::env::temp_dir().join(format!("tideshell-cc-{}", std::process::id()));
        let f = cosmic_custom_shortcuts(&base);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        assert!(!compositor_route_installed(&base));
        std::fs::write(&f, &fresh).unwrap();
        assert!(compositor_route_installed(&base));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn commands_travel_from_client_to_server() {
        let p = std::env::temp_dir().join(format!("tideshell-ctl-{}", std::process::id()));
        let srv = Server::bind(&p).unwrap();
        assert!(srv.poll().is_empty());
        send(&p, "power-key").unwrap();
        send(&p, "lock").unwrap();
        let got = srv.poll();
        assert_eq!(got, vec![Ok(Command::PowerKey), Ok(Command::Lock)]);
        assert!(send(&p, "reboot").unwrap_err().contains("unknown command"));
        // A stale socket from a crash is replaced, not an error.
        drop(srv);
        std::fs::write(&p, b"").unwrap();
        let srv = Server::bind(&p).unwrap();
        send(&p, "palette").unwrap();
        assert_eq!(srv.poll(), vec![Ok(Command::Palette)]);
        drop(srv);
        assert!(!p.exists(), "removed on drop");
        assert!(send(&p, "lock").unwrap_err().contains("is tideshell running"));
    }
}
