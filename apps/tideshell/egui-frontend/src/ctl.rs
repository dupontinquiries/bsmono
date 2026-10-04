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

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    PowerKey,
    VolumeUp,
    VolumeDown,
    Lock,
    Palette,
    Switcher,
    /// Toggle carousel mode, exactly as the carousel tile does.
    Carousel,
    /// Take a screenshot, save it and put it on the clipboard.
    Screenshot,
    /// Re-read the prefs file. Sent by the settings app after it writes, so
    /// a change applies now rather than at the next launch — which matters
    /// most for the settings someone reaches for BECAUSE something is wrong
    /// right now: cold mode with gloves on, a text size they cannot read.
    ReloadPrefs,
    /// Resolved speaker-chain controls, from the settings app. Carries a
    /// payload, unlike the others, because the sender is deciding values
    /// rather than asking for an action.
    SpeakerProps(Vec<(String, f32)>),
    /// `gaze 0.42 0.31`: where the eyes are, as fractions of the screen. Sent by the eye tracker, which filters the
    /// point itself; the shell eases the fat cursor after it.
    Gaze(f32, f32),
}

impl Command {
    pub fn parse(s: &str) -> Option<Command> {
        if let Some(pairs) = tideshell_core::chaingraph::decode_props(s) {
            return Some(Command::SpeakerProps(pairs));
        }
        if let Some(rest) = s.trim().strip_prefix("gaze ") {
            let mut it = rest.split_whitespace().map(|v| v.parse::<f32>());
            return match (it.next(), it.next(), it.next()) {
                (Some(Ok(x)), Some(Ok(y)), None) if x.is_finite() && y.is_finite() => Some(Command::Gaze(x, y)),
                _ => None,
            };
        }
        match s.trim() {
            "power-key" => Some(Command::PowerKey),
            "volume-up" => Some(Command::VolumeUp),
            "volume-down" => Some(Command::VolumeDown),
            "lock" => Some(Command::Lock),
            "palette" => Some(Command::Palette),
            "switcher" => Some(Command::Switcher),
            "carousel" => Some(Command::Carousel),
            "screenshot" => Some(Command::Screenshot),
            "reload-prefs" => Some(Command::ReloadPrefs),
            _ => None,
        }
    }

    pub const NAMES: [&'static str; 10] = ["power-key", "volume-up", "volume-down", "lock", "palette", "switcher", "carousel", "screenshot", "reload-prefs", "gaze X Y"];
}

/// The keys cosmic-comp should hand to tideshell, as (keysym name, command).
///
/// The power key only. A compositor shortcut fires on PRESS and reports no
/// release, so anything routed this way can never tell a tap from a hold —
/// and the volume keys are the shell's hold surface (volkeys.rs), the one
/// control that works with gloves on and the screen off. They are read from
/// evdev instead, where releases exist.
///
/// The power key has no such need: a short press is the whole gesture, and
/// the long press belongs to logind, which handles it below us and will
/// power the phone off even if this shell has hung.
pub const COMPOSITOR_KEYS: [(&str, &str); 1] = [("XF86PowerOff", "power-key")];

/// Where cosmic reads user shortcuts that override its defaults.
pub fn cosmic_custom_shortcuts(config_home: &Path) -> PathBuf {
    config_home.join("cosmic/com.system76.CosmicSettings.Shortcuts/v1/custom")
}

/// `custom` is a bare RON map (the Shortcuts type is serde-transparent),
/// entries of the form `(modifiers: [], key: "XF86PowerOff"): Spawn("..."),`.
/// Merge ours in, keeping the user's other bindings and replacing any
/// existing binding of the same keys. Returns the new text.
///
/// Entries are split at top-level commas (depth-aware, strings respected),
/// NOT per line: cosmic-settings writes pretty RON with one entry over
/// several lines, and cosmic drops the WHOLE custom file if it fails to
/// parse, which brings the default `System(PowerOff)` back silently.
pub fn merge_custom_shortcuts(existing: &str, bin: &str) -> String {
    let mut body: Vec<String> = Vec::new();
    for entry in map_entries(existing) {
        let squashed: String = entry.chars().filter(|c| !c.is_whitespace()).collect();
        if COMPOSITOR_KEYS.iter().any(|(k, _)| squashed.contains(&format!("key:\"{k}\"")) && squashed.contains("modifiers:[]")) {
            continue;
        }
        body.push(format!("    {entry},"));
    }
    for (key, cmd) in COMPOSITOR_KEYS {
        body.push(format!("    (modifiers: [], key: \"{key}\"): Spawn(\"{bin} ctl {cmd}\"),"));
    }
    format!("{{\n{}\n}}\n", body.join("\n"))
}

/// The entries of a RON map `{ a: b, c: d, }`, each trimmed, without the
/// separating commas. Tracks (), [], {} depth and "..." strings (with
/// escapes) so commas inside an entry do not split it.
pub fn map_entries(text: &str) -> Vec<String> {
    let t = text.trim();
    let inner = t.strip_prefix('{').and_then(|x| x.strip_suffix('}')).unwrap_or(if t.is_empty() { "" } else { t });
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for c in inner.chars() {
        if in_str {
            cur.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                cur.push(c);
            }
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                let e = cur.trim().to_string();
                if !e.is_empty() {
                    out.push(e);
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let e = cur.trim().to_string();
    if !e.is_empty() {
        out.push(e);
    }
    out
}

/// Whether an entry is one tideshell wrote: a `Spawn` of `... ctl <cmd>`
/// for one of the control commands. Recognised by content, not by a marker,
/// because cosmic-settings rewrites the file without comments.
fn is_ours(entry: &str) -> bool {
    entry.contains("Spawn(") && Command::NAMES.iter().any(|c| entry.contains(&format!(" ctl {c}\"")))
}

/// The custom map without tideshell's entries; the user's own untouched.
pub fn unmerge_custom_shortcuts(existing: &str) -> String {
    let body: Vec<String> = map_entries(existing).into_iter().filter(|e| !is_ours(e)).map(|e| format!("    {e},")).collect();
    format!("{{\n{}\n}}\n", body.join("\n"))
}

/// Whether `text` already routes every compositor key to `bin`.
pub fn routes_to(text: &str, bin: &str) -> bool {
    let entries = map_entries(text);
    COMPOSITOR_KEYS.iter().all(|(key, cmd)| {
        entries.iter().any(|e| {
            let sq: String = e.chars().filter(|c| !c.is_whitespace()).collect();
            sq.contains(&format!("key:\"{key}\"")) && sq.contains("modifiers:[]") && e.contains(&format!("Spawn(\"{bin} ctl {cmd}\")"))
        })
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeysOutcome {
    /// Already bound to this binary: nothing written.
    AlreadyBound,
    /// Written. `backup` is set the first time a user file is replaced;
    /// later writes never overwrite that backup with tideshell's own file.
    Written { backup: Option<PathBuf>, replaced_other_path: bool },
}

/// Make cosmic route the power and volume keys to `bin ctl ...`. Idempotent:
/// writes only when something differs, so running it at every start is
/// cheap and leaves the file alone once right. cosmic-comp reloads the file
/// live (seen on the FP5), so the binding takes effect without a restart.
pub fn ensure_keys(config_home: &Path, bin: &str) -> Result<KeysOutcome, String> {
    let file = cosmic_custom_shortcuts(config_home);
    let old = std::fs::read_to_string(&file).unwrap_or_default();
    if routes_to(&old, bin) {
        return Ok(KeysOutcome::AlreadyBound);
    }
    let replaced_other_path = map_entries(&old).iter().any(|e| is_ours(e));
    let bak = file.with_extension("before-tideshell");
    let backup = if !old.trim().is_empty() && !bak.exists() && !replaced_other_path {
        std::fs::write(&bak, &old).map_err(|e| format!("{}: {e}", bak.display()))?;
        Some(bak)
    } else {
        None
    };
    let new = merge_custom_shortcuts(&old, bin);
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    // Write-then-rename: cosmic watches the file and must never read half of it.
    let tmp = file.with_extension("tideshell-tmp");
    std::fs::write(&tmp, &new).and_then(|_| std::fs::rename(&tmp, &file)).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(KeysOutcome::Written { backup, replaced_other_path })
}

/// Take tideshell's bindings out again (the user's stay). With none left
/// the keys fall back to cosmic's defaults -- the power key's shutdown
/// dialog included. Returns whether anything was removed.
pub fn remove_keys(config_home: &Path) -> Result<bool, String> {
    let file = cosmic_custom_shortcuts(config_home);
    let Ok(old) = std::fs::read_to_string(&file) else { return Ok(false) };
    if !map_entries(&old).iter().any(|e| is_ours(e)) {
        return Ok(false);
    }
    let tmp = file.with_extension("tideshell-tmp");
    std::fs::write(&tmp, unmerge_custom_shortcuts(&old)).and_then(|_| std::fs::rename(&tmp, &file)).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(true)
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

    /// The socket, so the main loop can sleep on it alongside Wayland
    /// rather than discovering a command up to a timeout later.
    pub fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        std::os::fd::AsFd::as_fd(&self.sock)
    }

    /// Every command waiting; unknown text is returned as Err for the trace.
    pub fn poll(&self) -> Vec<Result<Command, String>> {
        let mut out = Vec::new();
            // Big enough for a whole chain's props: about twenty short lines.
        // A datagram is all-or-nothing, so a buffer that is too small would
        // silently truncate rather than fail.
        let mut buf = [0u8; 8192];
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
    #[test]
    fn carousel_and_screenshot_are_socket_commands_and_listed() {
        assert_eq!(Command::parse("carousel"), Some(Command::Carousel));
        assert_eq!(Command::parse("screenshot\n"), Some(Command::Screenshot));
        for n in ["carousel", "screenshot"] {
            assert!(Command::NAMES.contains(&n), "{n} is advertised in the unknown-command error");
        }
        assert_eq!(Command::NAMES.len(), 10);
        assert_eq!(Command::parse("screenshots"), None);
    }

    #[test]
    fn gaze_is_a_socket_command_with_two_fractions_and_nothing_else() {
        assert_eq!(Command::parse("gaze 0.42 0.31"), Some(Command::Gaze(0.42, 0.31)));
        assert_eq!(Command::parse("gaze  1 0\n"), Some(Command::Gaze(1.0, 0.0)));
        for bad in ["gaze", "gaze 0.5", "gaze a b", "gaze 0.5 0.5 0.5", "gaze nan 0.5", "gaze inf 0.5", "gazes 0.5 0.5"] {
            assert_eq!(Command::parse(bad), None, "{bad:?}");
        }
    }

    use super::*;

    #[test]
    fn custom_shortcuts_merge_keeps_the_users_bindings_and_replaces_ours() {
        let user = "{\n    (modifiers: [Super], key: \"t\"): Spawn(\"cosmic-term\"),\n    (modifiers: [], key: \"XF86PowerOff\"): System(PowerOff),\n}\n";
        let m = merge_custom_shortcuts(user, "/home/user/.local/bin/tideshell");
        assert!(m.contains("(modifiers: [Super], key: \"t\"): Spawn(\"cosmic-term\"),"), "{m}");
        assert!(!m.contains("System(PowerOff)"), "the old power binding is replaced");
        assert!(m.contains("(modifiers: [], key: \"XF86PowerOff\"): Spawn(\"/home/user/.local/bin/tideshell ctl power-key\"),"));
        // The volume keys are deliberately NOT bound here: a compositor
        // shortcut fires on press and never reports the release, so a hold
        // could not be told from a tap, and the volume keys are the shell's
        // hold surface. They are read from evdev instead.
        assert!(!m.contains("XF86AudioLowerVolume"), "volume must stay on evdev: {m}");
        assert!(!m.contains("XF86AudioRaiseVolume"));
        assert!(m.starts_with("{\n") && m.trim_end().ends_with('}'));
        let twice = merge_custom_shortcuts(&m, "/home/user/.local/bin/tideshell");
        assert_eq!(twice, m, "idempotent");
        // Pretty RON as cosmic-settings writes it: one entry over several
        // lines, commas inside the entry. The per-line merge this replaces
        // turned "(" into "(," here, cosmic dropped the file, and the
        // default power-off popup came back.
        let pretty = r#"{
    (
        modifiers: [
            Super,
            Shift,
        ],
        key: "t",
    ): Spawn("cosmic-term, --x"),
    (
        modifiers: [],
        key: "XF86PowerOff",
    ): System(PowerOff),
}"#;
        let m2 = merge_custom_shortcuts(pretty, "tideshell");
        let entries = map_entries(&m2);
        // One user entry kept, plus ours. Ours is a single binding now:
        // only the power key goes through the compositor.
        assert_eq!(entries.len(), 2, "{m2}");
        assert!(entries[0].starts_with('(') && entries[0].contains("Super") && entries[0].contains("Shift") && entries[0].ends_with(r#"Spawn("cosmic-term, --x")"#), "the user's multi-line entry kept whole: {}", entries[0]);
        assert!(!m2.contains("System(PowerOff)") && !m2.contains("(,") && !m2.contains(",,"), "{m2}");
        // A binding of the same key WITH modifiers is a different binding: kept.
        let modded = merge_custom_shortcuts(r#"{(modifiers: [Ctrl], key: "XF86PowerOff"): System(LockScreen),}"#, "t");
        assert!(modded.contains("System(LockScreen)"), "{modded}");
        let tight = merge_custom_shortcuts(r#"{(modifiers:[],key:"XF86PowerOff"):System(PowerOff)}"#, "t");
        assert!(!tight.contains("System(PowerOff)"), "spacing does not hide the default: {tight}");
        let fresh = merge_custom_shortcuts("", "tideshell");
        assert_eq!(fresh.matches("Spawn(").count(), 1, "the power key, and only the power key");
        let base = std::env::temp_dir().join(format!("tideshell-cc-{}", std::process::id()));
        let f = cosmic_custom_shortcuts(&base);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        assert!(!compositor_route_installed(&base));
        std::fs::write(&f, &fresh).unwrap();
        assert!(compositor_route_installed(&base));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn ensure_is_idempotent_keeps_the_first_backup_and_follows_a_moved_binary() {
        let home = std::env::temp_dir().join(format!("tideshell-ek-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let f = cosmic_custom_shortcuts(&home);
        // No file at all (the FP5's state): created, nothing to back up.
        assert_eq!(ensure_keys(&home, "/a/tideshell").unwrap(), KeysOutcome::Written { backup: None, replaced_other_path: false });
        assert!(routes_to(&std::fs::read_to_string(&f).unwrap(), "/a/tideshell"));
        let t1 = std::fs::metadata(&f).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(ensure_keys(&home, "/a/tideshell").unwrap(), KeysOutcome::AlreadyBound);
        assert_eq!(std::fs::metadata(&f).unwrap().modified().unwrap(), t1, "not rewritten when already right");
        // A user's own file with the hand-made Disable: backed up once, Disable replaced.
        std::fs::write(&f, "{\n    (modifiers: [Super], key: \"t\"): Spawn(\"cosmic-term\"),\n    (modifiers: [], key: \"XF86PowerOff\"): Disable,\n}\n").unwrap();
        let out = ensure_keys(&home, "/a/tideshell").unwrap();
        let bak = f.with_extension("before-tideshell");
        assert_eq!(out, KeysOutcome::Written { backup: Some(bak.clone()), replaced_other_path: false });
        assert!(std::fs::read_to_string(&bak).unwrap().contains("Disable"));
        let now = std::fs::read_to_string(&f).unwrap();
        assert!(now.contains("cosmic-term") && !now.contains("Disable") && routes_to(&now, "/a/tideshell"), "{now}");
        // The binary moved: rebound to the new path, the first backup kept.
        assert_eq!(ensure_keys(&home, "/b/tideshell").unwrap(), KeysOutcome::Written { backup: None, replaced_other_path: true });
        let moved = std::fs::read_to_string(&f).unwrap();
        assert!(routes_to(&moved, "/b/tideshell") && !moved.contains("/a/tideshell"), "{moved}");
        assert!(std::fs::read_to_string(&bak).unwrap().contains("Disable"), "the user's original survives later writes");
        // Remove: ours go, the user's stay.
        assert!(remove_keys(&home).unwrap());
        let left = std::fs::read_to_string(&f).unwrap();
        assert!(left.contains("cosmic-term") && !left.contains(" ctl "), "{left}");
        assert!(!remove_keys(&home).unwrap(), "nothing left to remove");
        assert!(!f.with_extension("tideshell-tmp").exists());
        let _ = std::fs::remove_dir_all(&home);
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
