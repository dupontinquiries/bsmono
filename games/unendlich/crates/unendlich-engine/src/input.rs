//! Per-frame input state cache.
//!
//! Tracks which keys and mouse buttons are currently pressed, and offers
//! "just pressed this frame" detection by comparing against the previous frame.
//!
//! ## Decoupling button edges from frame pacing
//!
//! Discrete press-edges (esc, inventory, jump, toggle-flight, …) must be
//! observed by exactly one consumer pass and never dropped, regardless of how
//! the bin drains events or how slow a frame is. The bin drains events TWICE
//! per frame: once at the top (before consumers run) and once late (just
//! before draw, to minimise camera latency). A press that lands in the LATE
//! drain used to be latched and then wiped by `end_frame()` before any
//! consumer saw it — the queue had already been emptied, so the next frame's
//! top drain never re-saw it either. That is the "eaten esc / jump" bug.
//!
//! Fix: the late drain routes through [`InputCache::handle_event_late`], which
//! records press-edges into a separate `late_*` set. `end_frame()` then *carries*
//! those late edges into the next frame's just-pressed set instead of clearing
//! them. Net effect: every physical press is observed by consumers exactly once,
//! whether it was drained early or late, at 5 fps or 500.

// r328: input is now backend-neutral — consumes the native InputEvent
// abstraction (input_codes), so this whole module compiles & tests without
// glfw. The bin converts glfw/winit events into InputEvent at the boundary.
use crate::input_codes::{InputEvent, KeyAction as Action, Keycode as Key, MouseBtn as MouseButton};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct InputCache {
    key_states: HashMap<Key, Action>,
    prev_key_states: HashMap<Key, Action>,

    /// Keys that received a `Press` event THIS frame, observable by consumers
    /// during this frame's logic pass. Latched at event receipt (top drain),
    /// rotated at end_frame. Necessary because at high FPS a Press and Release
    /// can both arrive within a single frame; `key_states` only retains the
    /// last (Release), so the transition would be lost without this latch.
    just_pressed_keys: HashSet<Key>,
    /// r187: presses received THIS frame per key (low-fps double-press
    /// support: toggles consume parity, not a boolean).
    press_counts: HashMap<Key, u8>,

    /// Press-edges caught in the LATE (post-consumer) drain. They have NOT been
    /// seen by any consumer yet, so end_frame moves them into
    /// `just_pressed_keys` for the next frame rather than dropping them.
    late_presses: HashSet<Key>,

    mouse_button_states: HashMap<MouseButton, Action>,
    prev_mouse_button_states: HashMap<MouseButton, Action>,
    /// Same idea as just_pressed_keys, for mouse buttons.
    just_pressed_buttons: HashSet<MouseButton>,
    /// Late-drained mouse press-edges, carried like `late_presses`.
    late_buttons: HashSet<MouseButton>,

    pub cursor_pos: (f64, f64),
    pub cursor_delta: (f64, f64),
    last_cursor_pos: (f64, f64),
    cursor_initialized: bool,

    pub scroll_delta: f64,
}

impl InputCache {
    /// Drain an event in the normal (top-of-frame) phase: press-edges become
    /// immediately observable by this frame's consumers.
    pub fn handle_event(&mut self, event: &InputEvent) {
        match *event {
            InputEvent::Key { code, action } => {
                if matches!(action, Action::Press) {
                    self.just_pressed_keys.insert(code);
                    let n = self.press_counts.get(&code).copied().unwrap_or(0);
                    self.press_counts.insert(code, n.saturating_add(1));
                }
                self.key_states.insert(code, action);
            }
            InputEvent::MouseButton { button, action } => {
                if matches!(action, Action::Press) {
                    self.just_pressed_buttons.insert(button);
                }
                self.mouse_button_states.insert(button, action);
            }
            InputEvent::CursorPos { x, y } => {
                if !self.cursor_initialized {
                    self.last_cursor_pos = (x, y);
                    self.cursor_initialized = true;
                }
                let dx = x - self.last_cursor_pos.0;
                let dy = y - self.last_cursor_pos.1;
                self.cursor_delta.0 += dx;
                self.cursor_delta.1 += dy;
                self.last_cursor_pos = (x, y);
                self.cursor_pos = (x, y);
            }
            InputEvent::Scroll { y, .. } => {
                self.scroll_delta += y;
            }
            InputEvent::UnknownKey { .. } => {}
        }
    }

    /// Drain an event in the LATE (post-consumer, pre-draw) phase.
    ///
    /// Cursor / scroll updates apply immediately (that's the whole point of the
    /// late drain — freshest camera). But discrete key / mouse PRESS edges are
    /// stashed in the `late_*` sets so `end_frame` can carry them into the next
    /// frame's just-pressed set, instead of them being latched-then-wiped
    /// before any consumer runs. Held-state (`key_states`) is updated normally
    /// so "is currently held" queries stay correct.
    pub fn handle_event_late(&mut self, event: &InputEvent) {
        match *event {
            InputEvent::Key { code, action } => {
                if matches!(action, Action::Press) {
                    self.late_presses.insert(code);
                }
                self.key_states.insert(code, action);
            }
            InputEvent::MouseButton { button, action } => {
                if matches!(action, Action::Press) {
                    self.late_buttons.insert(button);
                }
                self.mouse_button_states.insert(button, action);
            }
            // Cursor + scroll: apply now for minimal camera latency.
            InputEvent::CursorPos { .. } | InputEvent::Scroll { .. } => self.handle_event(event),
            InputEvent::UnknownKey { .. } => {}
        }
    }

    /// Call once per frame, after dispatching events AND after consumers have
    /// read the just-pressed sets.
    /// r187: presses this frame (>=1 when just_pressed). Toggle consumers
    /// use PARITY so a double-tap inside one laggy frame still nets out.
    pub fn presses_this_frame(&self, key: Key) -> u8 {
        self.press_counts.get(&key).copied().unwrap_or(0)
    }

    pub fn end_frame(&mut self) {
        self.prev_key_states = self.key_states.clone();
        self.prev_mouse_button_states = self.mouse_button_states.clone();
        // Rotate: this frame's already-observed edges are dropped; edges caught
        // in the late drain (not yet observed) become next frame's just-pressed.
        self.just_pressed_keys = std::mem::take(&mut self.late_presses);
        self.press_counts.clear(); // r187
        self.just_pressed_buttons = std::mem::take(&mut self.late_buttons);
        self.cursor_delta = (0.0, 0.0);
        self.scroll_delta = 0.0;
    }

    pub fn is_key_pressed(&self, key: Key) -> bool {
        matches!(self.key_states.get(&key), Some(Action::Press) | Some(Action::Repeat))
    }

    /// True if the key transitioned up→down and that edge belongs to THIS
    /// frame's consumer pass (either drained at the top this frame, or carried
    /// forward from last frame's late drain). Non-consuming: multiple consumers
    /// may all observe the same edge within a frame.
    pub fn is_key_just_pressed(&self, key: Key) -> bool {
        self.just_pressed_keys.contains(&key)
    }

    pub fn is_mouse_button_pressed(&self, button: MouseButton) -> bool {
        matches!(self.mouse_button_states.get(&button), Some(Action::Press) | Some(Action::Repeat))
    }

    pub fn is_mouse_button_just_pressed(&self, button: MouseButton) -> bool {
        self.just_pressed_buttons.contains(&button)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: Key) -> InputEvent {
        InputEvent::Key { code: key, action: Action::Press }
    }

    #[test]
    fn top_drain_press_visible_then_cleared() {
        let mut c = InputCache::default();
        c.handle_event(&press(Key::Space));
        assert!(c.is_key_just_pressed(Key::Space), "top-drain edge visible same frame");
        c.end_frame();
        assert!(!c.is_key_just_pressed(Key::Space), "edge cleared after its frame");
    }

    #[test]
    fn late_drain_press_carries_exactly_one_frame() {
        let mut c = InputCache::default();
        c.handle_event_late(&press(Key::Escape));
        // Consumers already ran this frame; a late edge must not be observable
        // yet, but must NOT be wiped either.
        assert!(!c.is_key_just_pressed(Key::Escape), "late edge not observable same frame");
        c.end_frame();
        assert!(c.is_key_just_pressed(Key::Escape), "late edge carried to next frame");
        c.end_frame();
        assert!(!c.is_key_just_pressed(Key::Escape), "carried edge consumed exactly once");
    }
}
