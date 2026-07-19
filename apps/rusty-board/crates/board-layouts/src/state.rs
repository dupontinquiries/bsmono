//! Keyboard state machine.
//!
//! Tracks which "page" of keys is currently shown (letters, symbols, emoji, ...)
//! and which modifiers are active. The engine queries this when rendering
//! and when deciding what character a key press emits.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeypadState {
    /// Lowercase letters
    Normal,
    /// One-shot uppercase
    Shifted,
    /// Locked uppercase
    CapsLock,
    /// Symbols and punctuation
    Symbols,
    /// Numbers only
    Numbers,
    /// Emoji picker
    Emoji,
    /// Voice recording active
    Voice,
}

impl Default for KeypadState {
    fn default() -> Self {
        KeypadState::Normal
    }
}

impl KeypadState {
    pub fn is_shifted(&self) -> bool {
        matches!(self, KeypadState::Shifted | KeypadState::CapsLock)
    }

    pub fn next_shift(&self) -> Self {
        match self {
            KeypadState::Normal => KeypadState::Shifted,
            KeypadState::Shifted => KeypadState::CapsLock,
            KeypadState::CapsLock => KeypadState::Normal,
            other => *other,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ModifierState {
    pub ctrl: bool,
    pub alt: bool,
    pub super_key: bool,
    pub shift_held: bool,
}

impl ModifierState {
    pub fn any_active(&self) -> bool {
        self.ctrl || self.alt || self.super_key
    }

    pub fn clear(&mut self) {
        self.ctrl = false;
        self.alt = false;
        self.super_key = false;
        self.shift_held = false;
    }
}

#[derive(Debug, Clone, Default)]
pub struct KeyboardState {
    pub keypad: KeypadState,
    pub modifiers: ModifierState,
    /// Most recent text near the cursor (for word prediction)
    pub surrounding_text: String,
    /// Cursor position in surrounding_text
    pub cursor_pos: usize,
    /// True when the last key press was an "auto-cap" trigger
    pub auto_cap_pending: bool,
    /// Letter-shift level saved when the user transitioned into a
    /// transient page (Symbols/Numbers/Emoji) — restored when they
    /// return via `ABC`. Used to preserve `CapsLock` across a quick
    /// dip into symbols.
    ///
    /// CapsLock is sticky by deliberate user action — losing it
    /// just because the user typed a digit mid-sentence is bad UX.
    /// One-shot `Shifted` is NOT preserved here (always saves as
    /// Normal) because touch-keyboard convention treats any context
    /// switch as consuming the shift intent.
    ///
    /// Always one of `Normal` or `CapsLock`. Reset to `Normal`
    /// after restoration so a subsequent symbols visit without a
    /// fresh CapsLock doesn't accidentally re-engage it.
    pub pre_transient_shift: KeypadState,
    /// True when the most recent commit was a swipe-typed word and
    /// nothing has happened since that would break a word boundary
    /// (manual space, punctuation, cursor move, focus change). Used by
    /// the auto-space feature: a swipe following another swipe gets a
    /// leading space so words separate without tapping space. Reset by
    /// any non-swipe commit, manual space, or cursor jump.
    pub last_commit_was_swipe: bool,
    /// Recent committed-text history for undo. Each entry is one
    /// commit (single character, full word, voice/LLM final). The
    /// engine pushes here on every TextCommitted and pops on Undo.
    /// Bounded to avoid unbounded memory; the cap of 32 entries is
    /// enough to cover several sentences of typing without keeping
    /// the user's entire session in memory.
    pub undo_stack: std::collections::VecDeque<UndoEntry>,
}

/// One step on the undo stack. Records what was committed so we can
/// remove it with the right number of backspaces.
#[derive(Debug, Clone)]
pub struct UndoEntry {
    /// The text that was committed.
    pub text: String,
    /// Human-readable origin label for logging. Doesn't affect the
    /// undo mechanism itself.
    pub source: &'static str,
}

impl KeyboardState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn toggle_shift(&mut self) {
        self.keypad = self.keypad.next_shift();
    }

    pub fn reset_to_normal(&mut self) {
        self.keypad = KeypadState::Normal;
        self.modifiers.clear();
        self.auto_cap_pending = false;
    }

    /// Called after a key emits a character. Implements one-shot shift
    /// (Shifted -> Normal after one keystroke), which matches user
    /// expectation on touch keyboards.
    pub fn after_keypress(&mut self) {
        if self.keypad == KeypadState::Shifted {
            self.keypad = KeypadState::Normal;
        }
        self.auto_cap_pending = false;
    }

    /// Heuristic: trigger auto-cap when surrounding text ends with `. `, `! `,
    /// `? `, or is empty (start of input).
    ///
    /// Gated by a process-wide AUTO_CAP_ENABLED flag (initialized
    /// from config at startup, toggled by settings). The flag
    /// lives in this module so engine code and state code share
    /// the same source of truth.
    pub fn check_auto_cap(&mut self) {
        if !self.auto_cap_pending {
            return;
        }
        if !AUTO_CAP_ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let trimmed = self.surrounding_text.trim_end();
        let should_cap = trimmed.is_empty()
            || trimmed.ends_with('.')
            || trimmed.ends_with('!')
            || trimmed.ends_with('?');
        if should_cap && self.keypad == KeypadState::Normal {
            self.keypad = KeypadState::Shifted;
        }
    }
}

/// Process-wide auto-cap toggle. Defaults to true; settings panel
/// updates this via `set_auto_cap_enabled`. Reads are lock-free
/// AtomicBool — auto-cap is checked on every keypress so we want
/// the check cheap.
static AUTO_CAP_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

pub fn set_auto_cap_enabled(on: bool) {
    AUTO_CAP_ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn auto_cap_enabled() -> bool {
    AUTO_CAP_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_cycle_normal_shifted_caps_normal() {
        let mut s = KeyboardState::new();
        assert_eq!(s.keypad, KeypadState::Normal);
        s.toggle_shift();
        assert_eq!(s.keypad, KeypadState::Shifted);
        s.toggle_shift();
        assert_eq!(s.keypad, KeypadState::CapsLock);
        s.toggle_shift();
        assert_eq!(s.keypad, KeypadState::Normal);
    }

    #[test]
    fn one_shot_shift_resets_after_keypress() {
        let mut s = KeyboardState::new();
        s.keypad = KeypadState::Shifted;
        s.after_keypress();
        assert_eq!(s.keypad, KeypadState::Normal);
    }

    #[test]
    fn caps_lock_persists_through_keypress() {
        let mut s = KeyboardState::new();
        s.keypad = KeypadState::CapsLock;
        s.after_keypress();
        assert_eq!(s.keypad, KeypadState::CapsLock);
    }
}
