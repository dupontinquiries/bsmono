//! tray — the minimized keyboard, ported from `ui/minimized_tray.rs`.
//!
//! The compact mode the keyboard drops into when you swipe up off the top edge:
//! an expand button, a spacebar, and a close button, anchored in a corner so one
//! thumb can reach all three.
//!
//! Its spacebar carries three gesture vocabularies at once, and the original
//! documents them exactly:
//!
//! * **Tap** → a space, and it also exits selection mode if active.
//! * **Double-tap within 350 ms** → toggles selection mode.
//! * **Cardinal swipe** → an arrow key, or Shift+arrow in selection mode.
//!
//! ## Why a tap must also exit selection mode
//!
//! Selection mode makes every swipe extend a selection. Without an escape on the
//! most ordinary gesture there is, a user who does not know how they got into it
//! cannot get out — and the tray is three buttons, so there is nowhere to put a
//! visible exit. The overload is the design.
//!
//! ## The ordering that makes it work
//!
//! A double-tap is two taps, so the first tap has already emitted a space by the
//! time the second arrives. Emitting the space and *then* toggling on the second
//! is correct — the stray space is one keystroke and undoable, whereas waiting
//! 350 ms to find out whether every tap was a double would make the spacebar
//! feel broken.

/// Double-tap window, from the original.
pub const DOUBLE_TAP_MS: u64 = 350;

/// How far a press must move to count as a swipe rather than a tap, in pixels.
/// Below this a thumb roll on a small target would fire arrows nobody asked for.
pub const SWIPE_PX: f32 = 18.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir { Left, Right, Up, Down }

/// What a gesture produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayEvent {
    Space,
    /// An arrow, with `shift` set in selection mode so the caller does not have
    /// to know about the mode at all.
    Arrow { dir: Dir, shift: bool },
    SelectionMode(bool),
    Expand,
    Close,
}

#[derive(Debug, Clone, Default)]
pub struct Tray {
    pub selecting: bool,
    /// Timestamp of the last tap, for the double-tap window.
    last_tap_ms: Option<u64>,
    press: Option<(f32, f32)>,
}

impl Tray {
    pub fn press(&mut self, x: f32, y: f32) { self.press = Some((x, y)); }

    /// Release. `now_ms` is a monotonic clock.
    ///
    /// Returns every event the gesture produced, in order — a double-tap emits
    /// the space from its first tap and then the mode toggle, and the caller
    /// needs both.
    pub fn release(&mut self, x: f32, y: f32, now_ms: u64) -> Vec<TrayEvent> {
        let Some((px, py)) = self.press.take() else { return Vec::new() };
        let (dx, dy) = (x - px, y - py);
        if dx.abs().max(dy.abs()) >= SWIPE_PX {
            // A swipe cancels any pending double-tap: a tap then a swipe is not
            // a double-tap, and leaving the timestamp would make the NEXT tap
            // toggle the mode unexpectedly.
            self.last_tap_ms = None;
            let dir = if dx.abs() > dy.abs() {
                if dx < 0.0 { Dir::Left } else { Dir::Right }
            } else if dy < 0.0 { Dir::Up } else { Dir::Down };
            return vec![TrayEvent::Arrow { dir, shift: self.selecting }];
        }
        // a tap
        let double = matches!(self.last_tap_ms, Some(t) if now_ms.saturating_sub(t) <= DOUBLE_TAP_MS);
        if double {
            self.last_tap_ms = None;
            self.selecting = !self.selecting;
            return vec![TrayEvent::SelectionMode(self.selecting)];
        }
        self.last_tap_ms = Some(now_ms);
        // A tap also LEAVES selection mode: without an escape on the most
        // ordinary gesture there is, a user who does not know how they got into
        // it cannot get out, and the tray has nowhere to put a visible exit.
        if self.selecting {
            self.selecting = false;
            return vec![TrayEvent::Space, TrayEvent::SelectionMode(false)];
        }
        vec![TrayEvent::Space]
    }

    /// Abandon a press without emitting — for the keyboard being dismissed
    /// mid-gesture.
    pub fn cancel(&mut self) { self.press = None; }

    pub fn expand(&mut self) -> TrayEvent { self.selecting = false; TrayEvent::Expand }
    pub fn close(&mut self) -> TrayEvent { self.selecting = false; TrayEvent::Close }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap(t: &mut Tray, at: u64) -> Vec<TrayEvent> {
        t.press(10.0, 10.0);
        t.release(11.0, 10.0, at)
    }

    #[test]
    fn a_tap_emits_a_space() {
        let mut t = Tray::default();
        assert_eq!(tap(&mut t, 0), vec![TrayEvent::Space]);
    }

    #[test]
    fn two_taps_inside_the_window_toggle_selection_mode() {
        let mut t = Tray::default();
        assert_eq!(tap(&mut t, 0), vec![TrayEvent::Space], "the first tap still types");
        assert_eq!(tap(&mut t, 300), vec![TrayEvent::SelectionMode(true)]);
        assert!(t.selecting);
    }

    #[test]
    fn two_taps_outside_the_window_are_two_spaces() {
        let mut t = Tray::default();
        tap(&mut t, 0);
        assert_eq!(tap(&mut t, DOUBLE_TAP_MS + 1), vec![TrayEvent::Space]);
        assert!(!t.selecting);
    }

    #[test]
    fn a_tap_escapes_selection_mode() {
        // Without an escape on the most ordinary gesture there is, a user who
        // does not know how they got into selection mode cannot get out — and
        // the tray is three buttons, with nowhere to put a visible exit.
        let mut t = Tray::default();
        tap(&mut t, 0);
        tap(&mut t, 200);
        assert!(t.selecting);
        let ev = tap(&mut t, 1000);
        assert_eq!(ev, vec![TrayEvent::Space, TrayEvent::SelectionMode(false)]);
        assert!(!t.selecting);
    }

    #[test]
    fn a_cardinal_swipe_emits_an_arrow() {
        let mut t = Tray::default();
        t.press(50.0, 50.0);
        assert_eq!(t.release(50.0 - SWIPE_PX - 1.0, 50.0, 0),
                   vec![TrayEvent::Arrow { dir: Dir::Left, shift: false }]);
        t.press(50.0, 50.0);
        assert_eq!(t.release(50.0, 50.0 - SWIPE_PX - 1.0, 0),
                   vec![TrayEvent::Arrow { dir: Dir::Up, shift: false }]);
    }

    #[test]
    fn arrows_carry_shift_in_selection_mode() {
        // The caller does not have to know about the mode at all.
        let mut t = Tray::default();
        tap(&mut t, 0);
        tap(&mut t, 100);
        assert!(t.selecting);
        t.press(50.0, 50.0);
        assert_eq!(t.release(50.0 + SWIPE_PX + 1.0, 50.0, 200),
                   vec![TrayEvent::Arrow { dir: Dir::Right, shift: true }]);
        assert!(t.selecting, "a swipe does not leave the mode");
    }

    #[test]
    fn a_small_movement_is_still_a_tap() {
        // A thumb roll on a small target would otherwise fire arrows nobody
        // asked for.
        let mut t = Tray::default();
        t.press(50.0, 50.0);
        assert_eq!(t.release(50.0 + SWIPE_PX - 1.0, 50.0, 0), vec![TrayEvent::Space]);
    }

    #[test]
    fn a_tap_then_a_swipe_is_not_a_double_tap() {
        // Leaving the timestamp would make the NEXT tap toggle the mode
        // unexpectedly, which is the kind of bug users describe as "it just
        // does things".
        let mut t = Tray::default();
        tap(&mut t, 0);
        t.press(50.0, 50.0);
        t.release(50.0 + SWIPE_PX + 1.0, 50.0, 100);
        assert_eq!(tap(&mut t, 200), vec![TrayEvent::Space], "not a toggle");
        assert!(!t.selecting);
    }

    #[test]
    fn a_release_without_a_press_emits_nothing() {
        let mut t = Tray::default();
        assert!(t.release(0.0, 0.0, 0).is_empty());
    }

    #[test]
    fn a_cancelled_gesture_emits_nothing() {
        let mut t = Tray::default();
        t.press(10.0, 10.0);
        t.cancel();
        assert!(t.release(90.0, 10.0, 0).is_empty());
    }

    #[test]
    fn expanding_or_closing_leaves_selection_mode_behind() {
        // Returning to the full keyboard still in selection mode means every
        // swipe there extends a selection, with no indication why.
        let mut t = Tray::default();
        tap(&mut t, 0);
        tap(&mut t, 100);
        assert!(t.selecting);
        assert_eq!(t.expand(), TrayEvent::Expand);
        assert!(!t.selecting);
    }
}
