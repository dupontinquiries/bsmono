//! Virtual keyboard backend — delivers committed text and keysyms to the
//! focused application.
//!
//! The trait `VirtualKeyboard` abstracts over the concrete delivery
//! mechanism. At runtime we instantiate one of:
//!
//! - `WaylandVirtualKeyboard` — uses `zwp_virtual_keyboard_v1` to inject
//!   keysym press/release pairs. This is what Phosh, Sway, and friends
//!   support, and what Maliit uses on those compositors.
//! - `X11VirtualKeyboard` — shells out to `xdotool type` / `xdotool key`
//!   for legacy desktops.
//! - `NoopVirtualKeyboard` — fallback that just logs. Used when neither
//!   protocol is available, so the keyboard still *visually* works for
//!   testing/screenshots.

use anyhow::Result;
use tracing::{debug, warn};

/// What the keyboard can ask the backend to do.
///
/// Single-threaded — used only from the GTK main thread via the GLib
/// timeout that drains engine events. We deliberately don't require
/// `Send + Sync` so backends can hold thread-local resources like
/// `Rc<>` callbacks or GTK widgets.
pub trait VirtualKeyboard {
    /// Commit literal text (e.g. from a tap on `a`, a swipe word, or a
    /// voice transcript). The backend is free to either type each
    /// character as a keysym press/release, or — when supported — paste
    /// the whole string in one go via the input-method commit_text.
    fn commit_text(&self, text: &str) -> Result<()>;

    /// Synthesize a single keysym press+release. Used for non-character
    /// keys: backspace, return, tab, arrow keys.
    fn press_keysym(&self, name: &str) -> Result<()>;

    /// Human-readable backend name for diagnostics.
    fn backend_name(&self) -> &'static str;
}

/// Which backend strategy is in use. Picked at startup.
#[derive(Debug, Clone, Copy)]
pub enum VirtualKeyboardBackend {
    /// `zwp_virtual_keyboard_v1` over Wayland.
    Wayland,
    /// `xdotool` shell-out on X11.
    X11Xdotool,
    /// Logging-only fallback.
    Noop,
}

impl VirtualKeyboardBackend {
    /// Detect what's available on the current display server. Tries
    /// Wayland first (most modern), then X11.
    pub fn detect() -> Self {
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            #[cfg(feature = "wayland")]
            {
                return Self::Wayland;
            }
        }
        Self::detect_non_wayland()
    }

    /// Detect a non-Wayland fallback (X11/xdotool or noop). Used when
    /// the Wayland backend was tried and failed; we still want to do
    /// the best we can.
    pub fn detect_non_wayland() -> Self {
        if std::env::var("DISPLAY").is_ok() && which_xdotool().is_some() {
            return Self::X11Xdotool;
        }
        Self::Noop
    }

    pub fn build(self) -> Box<dyn VirtualKeyboard> {
        match self {
            // For Wayland, callers should NOT use this factory — they
            // should construct the WaylandBackend themselves (so the
            // input-method callback can be wired to the keyboard window)
            // and pass it to `WaylandVirtualKeyboard::with_backend`. If
            // we end up here it means callers fell through to the simple
            // path; we return an empty Wayland backend that logs.
            #[cfg(feature = "wayland")]
            Self::Wayland => Box::new(WaylandVirtualKeyboard::empty()),
            #[cfg(not(feature = "wayland"))]
            Self::Wayland => Box::new(NoopVirtualKeyboard),
            Self::X11Xdotool => Box::new(X11VirtualKeyboard),
            Self::Noop => Box::new(NoopVirtualKeyboard),
        }
    }
}

fn which_xdotool() -> Option<std::path::PathBuf> {
    for path in std::env::var_os("PATH")?.to_string_lossy().split(':') {
        let candidate = std::path::PathBuf::from(path).join("xdotool");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

// ============================================================
// Wayland backend (zwp_virtual_keyboard_v1 + zwp_input_method_v2)
// ============================================================

#[cfg(feature = "wayland")]
pub struct WaylandVirtualKeyboard {
    /// `None` if the backend couldn't be brought up (no Wayland session,
    /// compositor doesn't support virtual_keyboard, etc). In that case
    /// commit_text/press_keysym become no-ops with a warning. The factory
    /// in `VirtualKeyboardBackend::build()` should return a
    /// `NoopVirtualKeyboard` instead, but we keep the Option here so a
    /// late-failing setup doesn't crash the keyboard.
    backend: Option<crate::compositor::wayland_backend::WaylandBackend>,
}

#[cfg(feature = "wayland")]
impl WaylandVirtualKeyboard {
    /// Build with an existing backend (typical path — `window.rs` creates
    /// the backend once and passes it here so the same connection is
    /// shared with the input-method listener).
    pub fn with_backend(backend: crate::compositor::wayland_backend::WaylandBackend) -> Self {
        Self {
            backend: Some(backend),
        }
    }

    /// Empty backend — for cases where Wayland init failed but we still
    /// want to construct *something* implementing the trait. All calls
    /// log a warning and return Ok.
    pub fn empty() -> Self {
        Self { backend: None }
    }
}

#[cfg(feature = "wayland")]
impl VirtualKeyboard for WaylandVirtualKeyboard {
    fn commit_text(&self, text: &str) -> Result<()> {
        match &self.backend {
            Some(b) => b.commit_text(text),
            None => {
                debug!(target: "rusty_board::vkbd", "[wayland-empty] {:?}", text);
                Ok(())
            }
        }
    }

    fn press_keysym(&self, name: &str) -> Result<()> {
        match &self.backend {
            Some(b) => b.press_keysym(name),
            None => {
                debug!(target: "rusty_board::vkbd", "[wayland-empty] keysym {}", name);
                Ok(())
            }
        }
    }

    fn backend_name(&self) -> &'static str {
        match &self.backend {
            Some(b) if b.uses_v2() => "zwp_input_method_v2 + zwp_virtual_keyboard_v1",
            Some(_) => "zwp_input_method_v1 (Plasma Mobile)",
            None => "wayland-failed",
        }
    }
}

// ============================================================
// X11 backend (xdotool shell-out)
// ============================================================

pub struct X11VirtualKeyboard;

impl VirtualKeyboard for X11VirtualKeyboard {
    fn commit_text(&self, text: &str) -> Result<()> {
        let status = std::process::Command::new("xdotool")
            .arg("type")
            .arg("--clearmodifiers")
            .arg("--")
            .arg(text)
            .status();
        match status {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => {
                warn!("xdotool type exited non-zero: {:?}", s);
                Ok(())
            }
            Err(e) => {
                warn!("xdotool type failed to spawn: {}", e);
                Err(e.into())
            }
        }
    }

    fn press_keysym(&self, name: &str) -> Result<()> {
        let status = std::process::Command::new("xdotool")
            .arg("key")
            .arg("--clearmodifiers")
            .arg(name)
            .status();
        match status {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => {
                warn!("xdotool key {} exited non-zero: {:?}", name, s);
                Ok(())
            }
            Err(e) => {
                warn!("xdotool key {} failed to spawn: {}", name, e);
                Err(e.into())
            }
        }
    }

    fn backend_name(&self) -> &'static str {
        "x11-xdotool"
    }
}

// ============================================================
// No-op fallback
// ============================================================

pub struct NoopVirtualKeyboard;

impl VirtualKeyboard for NoopVirtualKeyboard {
    fn commit_text(&self, text: &str) -> Result<()> {
        debug!(target: "rusty_board::vkbd", "[noop] commit_text: {:?}", text);
        Ok(())
    }

    fn press_keysym(&self, name: &str) -> Result<()> {
        debug!(target: "rusty_board::vkbd", "[noop] press_keysym: {}", name);
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "noop"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_backend_succeeds() {
        let kb = NoopVirtualKeyboard;
        assert!(kb.commit_text("hello").is_ok());
        assert!(kb.press_keysym("Return").is_ok());
        assert_eq!(kb.backend_name(), "noop");
    }

    #[test]
    fn detect_returns_some_backend() {
        // Whatever the test environment, detect() should return a usable
        // variant. Ensure it doesn't panic.
        let _b = VirtualKeyboardBackend::detect();
    }
}
