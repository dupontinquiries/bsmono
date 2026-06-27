//! Captured input-method Wayland socket fd.
//!
//! When KWin/Weston launches us as `--inputmethod`, it passes a privileged
//! Wayland connection via inherited file descriptor (`WAYLAND_SOCKET=<fd>`).
//! That fd is bound to a different connection than the public socket —
//! it's the one where `zwp_input_method_v2` and `zwp_virtual_keyboard_v1`
//! are exposed.
//!
//! main.rs captures this fd EARLY (before GTK init) and stashes it here
//! via `set_inherited_fd`. The Wayland backend later retrieves it via
//! `take_inherited_fd` and uses it to open a second Wayland connection
//! independent of GDK's.
//!
//! Why a separate connection: GTK/GDK must use the PUBLIC socket (where
//! `wl_compositor`, `zwlr_layer_shell_v1`, etc. live) for rendering and
//! window placement. The privileged socket lacks those. Different sockets
//! → different `wl_display`s → different `wl_seat`s. We can't multiplex.

use std::os::unix::io::RawFd;
use std::sync::Mutex;

static INHERITED_FD: Mutex<Option<RawFd>> = Mutex::new(None);

/// Stash an inherited Wayland fd for later use by the input-method backend.
/// Called from main() before GTK initializes.
pub fn set_inherited_fd(fd: RawFd) {
    if let Ok(mut slot) = INHERITED_FD.lock() {
        *slot = Some(fd);
    }
}

/// Take the inherited fd (consuming it). Returns `None` if there isn't one,
/// or if it was already taken. The caller owns the fd from this point.
pub fn take_inherited_fd() -> Option<RawFd> {
    INHERITED_FD.lock().ok().and_then(|mut slot| slot.take())
}

/// Peek at whether an inherited fd is available without consuming it.
pub fn has_inherited_fd() -> bool {
    INHERITED_FD.lock().ok().is_some_and(|slot| slot.is_some())
}
