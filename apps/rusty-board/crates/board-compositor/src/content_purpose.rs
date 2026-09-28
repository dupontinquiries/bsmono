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

/// Store a purpose that arrived in `text-input-v3` numbering — what
/// `input-method-v2`'s `content_type` carries.
///
/// The two enums DIVERGE after `password` (8): v3 inserts `pin` at 9, so
/// date/time/datetime/terminal are 10/11/12/13 there and 9/10/11/12 in v1.
/// The v2 handler stored the raw value, so on every v2 compositor (sway,
/// cosmic-comp, phoc) a terminal (13) was not recognised as one and a
/// date-and-time field (12) was taken for a terminal. Normalised here to
/// the v1 numbering the rest of this module documents; `pin` is a numeric
/// password and is stored as `password`.
pub fn set_content_purpose_v3(purpose: u32) {
    let v1 = match purpose {
        0..=8 => purpose,
        9 => 8,             // pin -> password
        10..=13 => purpose - 1,
        other => other,
    };
    set_content_purpose(v1.min(255) as u8);
}

/// The password purpose (v1 numbering, which is what is stored).
pub const CONTENT_PURPOSE_PASSWORD: u8 = 8;

#[cfg(test)]
mod v3_tests {
    use super::*;

    #[test]
    fn v3_numbers_are_stored_in_v1_numbering() {
        set_content_purpose_v3(13);
        assert_eq!(content_purpose(), CONTENT_PURPOSE_TERMINAL, "v3 terminal is a terminal");
        set_content_purpose_v3(12);
        assert_ne!(content_purpose(), CONTENT_PURPOSE_TERMINAL, "v3 datetime is not");
        set_content_purpose_v3(9);
        assert_eq!(content_purpose(), CONTENT_PURPOSE_PASSWORD, "a pin is a password");
        set_content_purpose_v3(5);
        assert_eq!(content_purpose(), 5, "url is the same in both");
    }
}

pub fn content_purpose() -> u8 {
    CONTENT_PURPOSE.load(Ordering::Relaxed)
}
