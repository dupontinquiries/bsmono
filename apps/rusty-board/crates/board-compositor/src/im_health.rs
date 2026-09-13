//! Whether the input-method channel is usable at all — as opposed to
//! whether a text field is currently focused, which is what the
//! `InputMethodActiveCallback` reports.
//!
//! ## Why this exists
//!
//! `zwp_input_method_v2` allows exactly ONE input method per seat. When a
//! second client calls `get_input_method`, the compositor hands back an
//! object and immediately sends `unavailable` on it; the object is dead
//! and will never fire `activate`. wlroots (so phoc, so Phosh, so most
//! postmarketOS images) implements it exactly that way, and squeekboard
//! is enabled by default on those images — so a second OSK started
//! alongside it binds successfully, logs a warning, and then sits there
//! receiving nothing forever.
//!
//! Before this module, `Unavailable` was handled by a `warn!` and nothing
//! else. Both front-ends drive their window's visibility from the
//! Activate/Deactivate signal, which in that state never arrives — so the
//! keyboard was permanently invisible, with no error the user could see
//! and (in the egui front-end) no setting that could override it. That is
//! the "installs fine, never pops up on some images" report.
//!
//! ## Why a process-wide atomic rather than a callback
//!
//! Same reasoning as `content_purpose`, and the same shape: this is
//! compositor-sourced state written by a `wayland_backend` dispatch
//! handler and read by front-end policy code, there is one input-method
//! connection per process by construction, and threading a third callback
//! through `try_new` / `try_new_standalone` / `finish_setup` would touch
//! every caller to deliver one bit.
//!
//! It is deliberately NOT reset anywhere. `unavailable` is terminal for
//! the object that received it: the protocol gives no way back short of
//! rebinding, which this backend does not do. A flag that could flap
//! would make the front-end's visibility flap with it.

use std::sync::atomic::{AtomicBool, Ordering};

static IM_UNAVAILABLE: AtomicBool = AtomicBool::new(false);

/// Called from the `zwp_input_method_v2::Event::Unavailable` handler.
pub fn set_input_method_unavailable() {
    IM_UNAVAILABLE.store(true, Ordering::Relaxed);
}

/// True once the compositor has told us another client owns the seat's
/// input method. A front-end that hides itself on "no focus" must treat
/// this as "there is no focus signal", not as "no field is focused".
pub fn input_method_unavailable() -> bool {
    IM_UNAVAILABLE.load(Ordering::Relaxed)
}

/// Tests only: the flag is process-wide and latched, so a test that sets
/// it has to be able to put it back or it poisons everything after it.
#[doc(hidden)]
pub fn reset_for_test() {
    IM_UNAVAILABLE.store(false, Ordering::Relaxed);
}
