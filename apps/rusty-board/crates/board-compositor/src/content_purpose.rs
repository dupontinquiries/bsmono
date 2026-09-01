//! The focused field's content purpose (plain text, password, terminal,
//! URL, ...), as reported by the compositor's `ContentType` event.
//!
//! Lives here rather than in the GTK app's `ui::window` because it's
//! genuinely compositor-sourced data — `wayland_backend`'s dispatch
//! handlers are what write it, on both the v1 and v2 protocol paths.
//! The GTK app's focus-show logic reads it (e.g. a terminal gets the
//! full keyboard even when focus-tray mode is on); it re-exports these
//! functions from `ui::window` so existing call sites there are
//! unaffected by the move.

use std::sync::atomic::{AtomicU8, Ordering};

/// 0 = normal, 1 = alpha, ... 5 = URL, 6 = Email, 7 = Name, 8 = Password,
/// 9 = Date, 10 = Time, 11 = DateTime, 12 = Terminal. 255 = "unknown /
/// not reported yet". See `zwp_text_input_v3`'s `content_purpose` enum
/// for the authoritative list; we only special-case a few values.
static CONTENT_PURPOSE: AtomicU8 = AtomicU8::new(255);

/// Wayland's `ContentPurpose::Terminal` constant, hoisted here so call
/// sites don't need to depend on the wayland-protocols crate directly
/// just to compare against it.
pub const CONTENT_PURPOSE_TERMINAL: u8 = 12;

pub fn set_content_purpose(purpose: u8) {
    CONTENT_PURPOSE.store(purpose, Ordering::Relaxed);
}

pub fn content_purpose() -> u8 {
    CONTENT_PURPOSE.load(Ordering::Relaxed)
}
