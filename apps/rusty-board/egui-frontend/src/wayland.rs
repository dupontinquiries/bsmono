//! wayland — the real Wayland input-method integration: key injection
//! AND focus-driven show/hide, the two things a "keyboard that behaves
//! like its GTK predecessor / like Maliit" needs.
//!
//! ## Why this can't just call `VirtualKeyboardBackend::detect().build()`
//!
//! That's the generic factory `sink.rs` used to call, and it deliberately
//! returns an EMPTY Wayland backend with no input-method callback wired —
//! see that function's own doc comment in `virtual_keyboard.rs`: "For
//! Wayland, callers should NOT use this factory — they should construct
//! the WaylandBackend themselves ... and pass it to
//! `WaylandVirtualKeyboard::with_backend`." The GTK app follows that
//! advice in `ui::window::build_vkbd_with_im_callback`, which calls
//! `wayland_backend::try_new(cb, st_cb)` directly so the
//! Activate/Deactivate events reach the GTK window. This module is the
//! same thing for an eframe/winit window instead of a GTK one.
//!
//! ## Why `try_new_standalone`, not `try_new`
//!
//! `wayland_backend::try_new` (what the GTK app uses) starts by finding a
//! GDK Wayland display and, when there's no inherited privileged fd,
//! shares GDK's own connection. An eframe/winit app has no GDK display at
//! all. `try_new_standalone` (added alongside this module) skips that
//! gate and opens its own fresh connection via `$WAYLAND_DISPLAY`
//! instead — which is architecturally the *normal* way a real input
//! method does this anyway: Maliit is a separate process/connection from
//! the app being typed into. See `wayland_backend.rs`'s doc comment on
//! why a second connection is fine for that role specifically (the
//! per-seat-singleton concern it raises is about not fragmenting one
//! toolkit's OWN two connections, not about separate processes).
//!
//! ## Why this crate has to pump GLib itself
//!
//! `wayland_backend.rs` dispatches its event queue from a
//! self-rescheduling `glib::timeout_add_local_once` (see
//! `install_glib_dispatcher`'s doc — adaptive polling, not an fd-watch).
//! That only fires if something is actually iterating the thread-default
//! `glib::MainContext`. A GTK app gets that for free from its own main
//! loop; an eframe/winit app has no GLib loop running at all. So
//! `pump()` below calls `glib::MainContext::default().iteration(false)`
//! once per frame from `demo_app`'s `update()` — the non-blocking
//! (`false`) form so it never stalls the render loop waiting for a wl
//! event that may never come.
//!
//! ## What this does NOT do
//!
//! Layer-shell anchoring (docking the window to the screen edge,
//! exclusive zone, the actual Maliit-style "other windows resize around
//! it") is still the pre-existing, separately-tracked gap
//! (`PORT_STATUS.md`'s "Platform placement" entry) — winit doesn't speak
//! wlr-layer-shell, and reaching for `gtk4-layer-shell` here would need
//! an actual `gtk4::Window`, which an eframe app doesn't have. This
//! module only makes the window itself appear and disappear
//! (`ViewportCommand::Visible`) in response to focus, at whatever
//! position/size it would otherwise have. That's the part of "like
//! Maliit" this pass addresses; docking is not.
//!
//! ## Verification status
//!
//! Compiles (verified). NOT run against a live compositor — this
//! environment has no Wayland session to test the Activate/Deactivate
//! wiring, the standalone-connection strategy, or the GLib pump against.

use std::cell::Cell;
use std::rc::Rc;

/// Owns the real Wayland input-method backend and the focus flag it
/// drives. One instance lives for the app's lifetime.
pub struct WaylandIntegration {
    pub(crate) vk: Box<dyn rusty_board::compositor::virtual_keyboard::VirtualKeyboard>,
    visible: Rc<Cell<bool>>,
}

impl WaylandIntegration {
    /// Try to stand up the real backend. `Err` covers everything from
    /// "not actually on Wayland" to "compositor has neither input-method
    /// protocol" — `sink::detect()` treats any `Err` here the same way:
    /// log once, fall back to `Preview`. The distinction between those
    /// failure modes matters for a human reading logs, not for the
    /// caller's control flow.
    pub fn try_new() -> Result<Self, String> {
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return Err("$WAYLAND_DISPLAY not set".to_string());
        }
        let visible = Rc::new(Cell::new(false));
        let visible_for_cb = visible.clone();
        let im_callback: rusty_board::compositor::wayland_backend::InputMethodActiveCallback =
            Rc::new(move |active| visible_for_cb.set(active));
        // Surrounding-text isn't consumed by anything in this port yet
        // (no Synonym/Rewrite-style feature reaching for editor context
        // the way the GTK build's does) — an intentionally empty sink,
        // not a placeholder for something broken.
        let surrounding_text_callback: rusty_board::compositor::wayland_backend::SurroundingTextCallback =
            Rc::new(|_text: String, _cursor: u32, _anchor: u32| {});

        let backend = rusty_board::compositor::wayland_backend::try_new_standalone(
            im_callback,
            surrounding_text_callback,
        )
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            "compositor exposes neither input-method-v2 nor input-method-v1".to_string()
        })?;

        Ok(Self {
            vk: Box::new(
                rusty_board::compositor::virtual_keyboard::WaylandVirtualKeyboard::with_backend(
                    backend,
                ),
            ),
            visible,
        })
    }

    /// Call once per frame. Non-blocking: drives whatever GLib timeout
    /// sources are due (the wl dispatcher's adaptive poll) and returns
    /// immediately either way.
    pub fn pump(&self) {
        while glib::MainContext::default().iteration(false) {}
    }

    /// Whether the compositor currently has a text field focused —
    /// exactly the Activate/Deactivate state the GTK build's
    /// `build_vkbd_with_im_callback` uses to show/hide its window. The
    /// caller is expected to call `pump()` first each frame so this
    /// reflects events actually processed this frame, not the previous
    /// one.
    pub fn is_visible(&self) -> bool {
        self.visible.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_wayland_display_set_try_new_fails_fast() {
        // SAFETY: tests in this crate don't run in parallel against this
        // var (nothing else in the suite reads/writes WAYLAND_DISPLAY),
        // and this restores it afterward regardless of outcome.
        let prev = std::env::var_os("WAYLAND_DISPLAY");
        unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
        let result = WaylandIntegration::try_new();
        if let Some(v) = prev {
            unsafe { std::env::set_var("WAYLAND_DISPLAY", v) };
        }
        assert!(result.is_err());
    }
}
