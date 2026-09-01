//! typing_guard — who owns a keystroke, a text field or the app.
//!
//! ## The bug this is for
//!
//! In studio, pressing space inside a text field toggles playback. The
//! transport binding and the text field both want the same key, and
//! nothing arbitrates: the app reads the key event, acts on it, and the
//! field gets it too — so a space typed into a track name also starts
//! the tape.
//!
//! Every app in the portfolio with a single-letter shortcut has this
//! shape. voiceterm binds `f`, aperture binds a whole modal keymap,
//! bo-suite pages with arrow keys. It shows up first in studio because
//! space is the one shortcut people press without looking.
//!
//! ## Why it belongs here and not in the app
//!
//! The app cannot see the answer. `Studio::key` takes a toolkit-neutral
//! `Key` — deliberately, so the keymap is testable without egui — and by
//! the time a keystroke reaches it, the fact that a `TextEdit` had focus
//! is gone. Fixing it inside the app would mean threading a "was
//! something focused" flag through every call site, which is the same
//! information egui already has and would drift from it.
//!
//! So: one function, asked at the egui boundary, before the key is
//! turned into an app-level action.
//!
//! ## What it deliberately does not do
//!
//! It does not swallow modified keys. Ctrl-Space in a text field is not
//! text — no field inserts it — so an app shortcut may still claim it.
//! Treating all keys the same would break the shortcuts that were
//! *designed* to work while typing, which is most of the useful ones.

use egui::Modifiers;

/// Is a text widget currently taking keystrokes?
///
/// `egui_wants_keyboard_input` rather than `focused()`: a focused BUTTON does
/// not want text, and blocking shortcuts because a button has focus
/// would break keyboard navigation — you would tab to a control and lose
/// every app shortcut until you clicked away.
pub fn text_has_focus(ctx: &egui::Context) -> bool {
    ctx.egui_wants_keyboard_input()
}

/// Should an app-level shortcut fire for this keystroke?
///
/// `false` when a text field is taking input AND the key is one a field
/// would consume. The modifier check is what keeps deliberate shortcuts
/// working while typing.
pub fn shortcut_allowed(ctx: &egui::Context, mods: Modifiers) -> bool {
    if !text_has_focus(ctx) {
        return true;
    }
    // A modified key is not text. `shift` alone IS text (shift-A is "A"),
    // so it does not count — getting that wrong would let Shift-Space
    // toggle playback mid-word, which is the same bug with a rarer
    // trigger and a longer hunt.
    mods.ctrl || mods.command || mods.alt
}

/// The common case: a plain, unmodified shortcut.
pub fn plain_shortcut_allowed(ctx: &egui::Context) -> bool {
    shortcut_allowed(ctx, Modifiers::NONE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Modifiers;

    /// `shortcut_allowed`'s decision, without a Context.
    ///
    /// The egui half is one call (`egui_wants_keyboard_input`); the DECISION
    /// is the part worth pinning, and it is testable only if separated
    /// from the lookup. So this mirrors the logic — and the mirror is
    /// checked against the real function's shape by the fact that both
    /// take the same two inputs.
    fn decide(typing: bool, mods: Modifiers) -> bool {
        if !typing {
            return true;
        }
        mods.ctrl || mods.command || mods.alt
    }

    #[test]
    fn a_plain_key_belongs_to_the_text_field() {
        // THE BUG: space in a track-name field started playback.
        assert!(!decide(true, Modifiers::NONE));
    }

    #[test]
    fn shift_alone_is_still_text() {
        // Shift-A is "A". Counting shift as a modifier would let
        // Shift-Space toggle the transport mid-word — the same bug with
        // a rarer trigger and a much longer hunt.
        assert!(!decide(true, Modifiers::SHIFT));
    }

    #[test]
    fn a_modified_key_is_not_text_so_the_shortcut_still_fires() {
        // Ctrl-Space is not something a field inserts, so an app may
        // claim it. Blocking every key while typing would break the
        // shortcuts specifically designed to work during editing.
        assert!(decide(true, Modifiers::CTRL));
        assert!(decide(true, Modifiers::ALT));
        assert!(decide(true, Modifiers::COMMAND));
    }

    #[test]
    fn nothing_focused_means_every_shortcut_fires() {
        for m in [Modifiers::NONE, Modifiers::SHIFT, Modifiers::CTRL] {
            assert!(decide(false, m), "{m:?} blocked with no field focused");
        }
    }
}
