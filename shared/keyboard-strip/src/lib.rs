//! keyboard-strip — app commands, offered on the on-screen keyboard.
//!
//! A phone has no room for a toolbar, and a desktop does not want one: the
//! commands already have shortcuts there. The on-screen keyboard is the one
//! surface that exists exactly when a phone user is working in an app and
//! never on a desktop with a real keyboard. So the app DECLARES a few
//! commands; the keyboard shows them as a strip above the keys while that app
//! has focus; a tap sends the command's key chord (`Alt+Left`, `Shift+F12`)
//! through the keyboard's normal virtual-keyboard path, to the focused app —
//! which already binds those chords. Nothing new travels back: the app needs
//! no listener, and a keyboard that does not know the protocol simply ignores
//! it.
//!
//! ## Why not a Wayland protocol, and why not D-Bus (yet)
//!
//! Wayland clients cannot talk to each other; a private protocol between an
//! app and the input method would need every compositor (cosmic-comp, KWin,
//! sway, phoc) to implement it. `text-input-v3` only carries fixed hints.
//!
//! D-Bus would work and has one real advantage — the bus tells the keyboard
//! when the app's name vanishes, so a crashed app's strip clears itself. It
//! would also put zbus (and an executor thread) into a keyboard whose loop is
//! a plain poll over a handful of fds. This crate uses the keyboard's
//! existing control socket and gets the crash case from a pid in the message
//! ([`pid_alive`]); the protocol here is transport-free, so a D-Bus carrier
//! is an addition, not a rewrite.
//!
//! ## Wire format
//!
//! One datagram, lines of tab-separated fields escaped by `net_kit::wire`:
//!
//! ```text
//! strip-show  <app-id>  <pid>
//! action      <label>   <keys>   <hint>
//! action      ...
//! ```
//! or the single line `strip-clear <app-id>`. Unknown lines are skipped, so a
//! newer app can add fields an older keyboard ignores.

use net_kit::wire::{decode_fields, encode_fields};
use std::path::PathBuf;

/// One command. `keys` is a chord in the keyboard's keysym syntax:
/// modifiers joined by `+`, then a keysym name or a single character
/// (`Alt+Left`, `Shift+F12`, `Ctrl+k`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub label: String,
    pub keys: String,
    /// Longer text for an accessibility label or a long-press.
    pub hint: String,
}

impl Action {
    pub fn new(label: &str, keys: &str, hint: &str) -> Action {
        Action { label: label.into(), keys: keys.into(), hint: hint.into() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    /// `app` has focus and offers these.
    Show { app: String, pid: u32, actions: Vec<Action> },
    /// `app` lost focus (or is quitting). Clears only that app's strip, so a
    /// late clear from the app you LEFT cannot wipe the one you went to.
    Clear { app: String },
}

/// The most actions a strip carries: past this it is a toolbar again.
pub const MAX_ACTIONS: usize = 10;

pub fn encode(m: &Msg) -> String {
    match m {
        Msg::Show { app, pid, actions } => {
            let mut s = encode_fields(&["strip-show", app, &pid.to_string()]);
            for a in actions.iter().take(MAX_ACTIONS) {
                s.push('\n');
                s.push_str(&encode_fields(&["action", &a.label, &a.keys, &a.hint]));
            }
            s
        }
        Msg::Clear { app } => encode_fields(&["strip-clear", app]),
    }
}

/// `None` for anything that is not a strip message — the keyboard's control
/// socket carries other commands too.
pub fn decode(text: &str) -> Option<Msg> {
    let mut lines = text.lines();
    let head = decode_fields(lines.next()?);
    match head.first().map(String::as_str) {
        Some("strip-clear") => Some(Msg::Clear { app: head.get(1)?.clone() }),
        Some("strip-show") => {
            let app = head.get(1)?.clone();
            let pid = head.get(2)?.parse().ok()?;
            let actions = lines
                .map(decode_fields)
                .filter(|f| f.first().map(String::as_str) == Some("action"))
                .filter_map(|f| {
                    let label = f.get(1)?.clone();
                    let keys = f.get(2)?.clone();
                    (!label.is_empty() && !keys.is_empty()).then(|| Action { label, keys, hint: f.get(3).cloned().unwrap_or_default() })
                })
                .take(MAX_ACTIONS)
                .collect();
            Some(Msg::Show { app, pid, actions })
        }
        _ => None,
    }
}

/// The keyboard's control socket (rusty-board's `ctl.rs`).
pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("rusty-board.ctl"))
}

/// Send to a running keyboard. `Err` when none is listening, which on a
/// desktop is the normal case and not worth reporting.
pub fn send(m: &Msg) -> Result<(), String> {
    let path = socket_path().ok_or("no XDG_RUNTIME_DIR")?;
    let tx = std::os::unix::net::UnixDatagram::unbound().map_err(|e| e.to_string())?;
    tx.send_to(encode(m).as_bytes(), &path).map(|_| ()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Whether the process that declared a strip is still running — so a
/// crashed app's strip does not outlive it. Linux `/proc`; elsewhere every
/// pid counts as alive and the strip waits for a clear or the next show.
pub fn pid_alive(pid: u32) -> bool {
    if cfg!(target_os = "linux") {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    } else {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_including_tabs_and_newlines_in_labels() {
        let m = Msg::Show {
            app: "aperture".into(),
            pid: 42,
            actions: vec![Action::new("‹ back", "Alt+Left", "where the jump\\tstarted\nhere"), Action::new("Callers", "Shift+F12", "")],
        };
        assert_eq!(decode(&encode(&m)), Some(m));
        let c = Msg::Clear { app: "aperture".into() };
        assert_eq!(decode(&encode(&c)), Some(c));
    }

    #[test]
    fn other_commands_and_junk_are_not_strips() {
        assert_eq!(decode("voice-start"), None);
        assert_eq!(decode(""), None);
        assert_eq!(decode("strip-show\taperture\tnot-a-pid"), None);
        let m = decode("strip-show\ta\t1\naction\t\tAlt+Left\nfuture-line\tx\naction\tOk\tReturn").unwrap();
        let Msg::Show { actions, .. } = m else { panic!() };
        assert_eq!(actions, vec![Action::new("Ok", "Return", "")], "empty label dropped, unknown line skipped");
    }

    #[test]
    fn a_strip_is_capped() {
        let actions = (0..30).map(|i| Action::new(&i.to_string(), "a", "")).collect();
        let m = Msg::Show { app: "x".into(), pid: 1, actions };
        let Some(Msg::Show { actions, .. }) = decode(&encode(&m)) else { panic!() };
        assert_eq!(actions.len(), MAX_ACTIONS);
    }

    #[test]
    fn this_process_is_alive_and_an_absurd_pid_is_not() {
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(u32::MAX - 1));
    }
}
