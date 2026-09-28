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
//! ## Why `pump()` is a frame-loop call, not a thread or a GLib timer
//!
//! `wayland_backend` used to dispatch its event queue from a
//! self-rescheduling `glib::timeout_add_local_once`, which meant an
//! eframe app had to iterate a GLib `MainContext` every frame to make it
//! fire — and had to link `glib` to do so. The backend now exposes
//! `pump()` (read-if-readable, dispatch, flush — non-blocking) and the
//! GTK app's GLib timer is one caller of it among others. This module
//! calls it once per frame from `demo_app`'s `update()`; the same call
//! is what a layer-shell loop would make after each poll. No toolkit
//! inside.
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

use board_compositor::virtual_keyboard::{VirtualKeyboard, WaylandVirtualKeyboard};
use board_compositor::wayland_backend::{self, InputMethodActiveCallback, SurroundingTextCallback};

/// Owns the real Wayland input-method backend and the focus flag it
/// drives. One instance lives for the app's lifetime.
pub struct WaylandIntegration {
    pub(crate) vk: Box<dyn VirtualKeyboard>,
    visible: Rc<Cell<bool>>,
    /// The field's surrounding text reported anchor != cursor.
    selection: Rc<Cell<bool>>,
}

impl WaylandIntegration {
    /// Whether the focused field last reported a selection.
    pub fn has_selection(&self) -> bool {
        self.selection.get()
    }

    /// Try to stand up the real backend. `Err` covers everything from
    /// "not actually on Wayland" to "compositor has neither input-method
    /// protocol" — `sink::detect()` treats any `Err` here the same way:
    /// log once, fall back to `Preview`. The distinction between those
    /// failure modes matters for a human reading logs, not for the
    /// caller's control flow.
    pub fn try_new() -> Result<Self, String> {
        // KWin launches its virtual keyboard with the privileged socket in
        // WAYLAND_SOCKET (captured into im_socket by the binary's main);
        // that is a Wayland session with no WAYLAND_DISPLAY needed for
        // the input-method side, so the gate is "either".
        if std::env::var_os("WAYLAND_DISPLAY").is_none()
            && !board_compositor::im_socket::has_inherited_fd()
        {
            return Err("neither $WAYLAND_DISPLAY nor an inherited WAYLAND_SOCKET".to_string());
        }
        let visible = Rc::new(Cell::new(false));
        let visible_for_cb = visible.clone();
        let im_callback: InputMethodActiveCallback =
            Rc::new(move |active| visible_for_cb.set(active));
        // Surrounding text: only whether something is SELECTED (anchor !=
        // cursor), for the Cut/Copy commands (edit_actions.rs). The text
        // itself is not kept.
        let selection = Rc::new(Cell::new(false));
        let selection_for_cb = selection.clone();
        let surrounding_text_callback: SurroundingTextCallback =
            Rc::new(move |_text: String, cursor: u32, anchor: u32| selection_for_cb.set(cursor != anchor));

        let backend = wayland_backend::try_new_standalone(
            im_callback,
            surrounding_text_callback,
        )
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            "compositor exposes neither input-method-v2 nor input-method-v1".to_string()
        })?;

        Ok(Self {
            vk: Box::new(WaylandVirtualKeyboard::with_backend(backend)),
            visible,
            selection,
        })
    }

    /// Call once per frame. Non-blocking: reads what the compositor has
    /// sent, dispatches it (Activate/Deactivate land in `visible` from
    /// here), flushes, and returns. A dead connection is logged once per
    /// frame rather than panicking — the keyboard degrades to a preview
    /// that says so in its status line, which beats a crash mid-typing.
    pub fn pump(&self) {
        if let Err(e) = self.vk.pump() {
            tracing::warn!("wayland pump: {e}");
        }
    }

    /// Whether the compositor currently has a text field focused —
    /// exactly the Activate/Deactivate state the GTK build's
    /// `build_vkbd_with_im_callback` uses to show/hide its window. The
    /// caller is expected to call `pump()` first each frame so this
    /// reflects events actually processed this frame, not the previous
    /// one.
    ///
    /// Named `is_active`, not `is_visible`, because it is not the answer
    /// to "should the keyboard be on screen" and was mistaken for it:
    /// this returns `false` both when a field has been deselected and
    /// when the compositor has never spoken to us at all, and only the
    /// first of those is a reason to hide. `focus_policy` makes that
    /// distinction; see its module doc.
    pub fn is_active(&self) -> bool {
        self.visible.get()
    }

    /// Whether another client owns the seat's input method, so
    /// [`Self::is_active`] is frozen and meaningless. Re-exported off the
    /// process-wide flag `wayland_backend` sets from the `Unavailable`
    /// event, so callers do not need to depend on `board_compositor`
    /// directly to ask.
    pub fn input_method_unavailable(&self) -> bool {
        board_compositor::im_health::input_method_unavailable()
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
