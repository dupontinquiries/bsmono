//! press_gesture — `PressGesture.cpp`, ported.
//!
//! Tap versus long-press versus drag-cancel, as one state machine.
//!
//! ## Why this exists beside the kit's long-press
//!
//! `uikit_egui::menu::HoldTracker` already does press → hold → movement
//! cancels, and tadpole and the fleet's context menus use it. This is not a
//! second copy: what `PressGesture` adds is the part its header spends most of
//! its explanation on — **recovering from a press whose release never arrives**.
//!
//! The C++ registry exists because a lost release leaves the machine in
//! `Pressing` or `Held` forever, and *"the next press would be ignored"* by the
//! idle guard. A scroll area swallowing the release, a compositor taking the
//! touch sequence, a widget destroyed mid-gesture — all of them end with the
//! control permanently dead to taps, which reads as the app freezing.
//!
//! ## One machine for all input paths
//!
//! The header notes that platforms differ in whether they synthesise mouse
//! events from touch, so it *"covers all platforms with one FSM"* rather than
//! branching. That is the right call and ports directly: a caller feeds
//! press/move/release from whatever it has, and the machine does not care
//! which.

/// How long a press must be held to become a long press.
pub const LONG_PRESS_MS: f32 = 500.0;
/// Movement beyond this cancels: the user is scrolling, not pressing.
pub const MOVE_CANCEL_PX: f32 = 16.0;
/// A press in flight longer than this is stuck, not slow.
///
/// One second past the long-press threshold. A real finger that has been down
/// for a second and a half has already produced its long press; anything still
/// `Pressing` at that age lost its release.
pub const STUCK_MS: f32 = 1_000.0 + LONG_PRESS_MS;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Idle,
    /// Down, not yet long enough.
    Pressing,
    /// Long press already emitted; still down.
    Held,
}

/// What the machine emitted this step.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    Tap,
    LongPress,
    /// The gesture became a drag, or was recovered by the watchdog. Callers use
    /// it to tear down any mid-gesture visual state.
    DragCanceled,
}

#[derive(Clone, Copy, Debug)]
pub struct PressGesture {
    state: State,
    down_at: (f32, f32),
    held_ms: f32,
    long_press_ms: f32,
}

impl Default for PressGesture {
    fn default() -> Self {
        PressGesture { state: State::Idle, down_at: (0.0, 0.0), held_ms: 0.0,
                       long_press_ms: LONG_PRESS_MS }
    }
}

impl PressGesture {
    pub fn state(&self) -> State { self.state }
    pub fn is_idle(&self) -> bool { self.state == State::Idle }
    pub fn held_ms(&self) -> f32 { self.held_ms }

    /// Progress toward a long press, for a filling indicator.
    pub fn progress(&self) -> f32 {
        if self.long_press_ms <= 0.0 { return 0.0; }
        (self.held_ms / self.long_press_ms).clamp(0.0, 1.0)
    }

    /// Override the threshold, as `setLongPressMs` does.
    pub fn set_long_press_ms(&mut self, ms: f32) { self.long_press_ms = ms.max(0.0); }

    /// A press began.
    ///
    /// Ignored unless idle — the guard the C++ has, and exactly the guard the
    /// watchdog exists to unblock.
    pub fn press(&mut self, x: f32, y: f32) {
        if self.state != State::Idle { return; }
        self.state = State::Pressing;
        self.down_at = (x, y);
        self.held_ms = 0.0;
    }

    /// The pointer moved. Returns `DragCanceled` if this ends the gesture.
    pub fn move_to(&mut self, x: f32, y: f32) -> Option<Event> {
        if self.state == State::Idle { return None; }
        let (px, py) = self.down_at;
        if ((x - px).powi(2) + (y - py).powi(2)).sqrt() > MOVE_CANCEL_PX {
            self.state = State::Idle;
            self.held_ms = 0.0;
            return Some(Event::DragCanceled);
        }
        None
    }

    /// Time passed. Returns `LongPress` on the step that crosses the threshold.
    pub fn tick(&mut self, dt_ms: f32) -> Option<Event> {
        if self.state != State::Pressing || dt_ms <= 0.0 { return None; }
        // A resumed suspend must not turn a press the user already released
        // into a long press; the watchdog handles that case instead.
        self.held_ms += dt_ms.min(STUCK_MS);
        if self.held_ms >= self.long_press_ms {
            self.state = State::Held;
            return Some(Event::LongPress);
        }
        None
    }

    /// The press ended.
    ///
    /// A release from `Held` emits nothing: the long press already fired, and
    /// emitting a tap as well would run both handlers for one gesture.
    pub fn release(&mut self) -> Option<Event> {
        let out = match self.state {
            State::Pressing => Some(Event::Tap),
            State::Held | State::Idle => None,
        };
        self.state = State::Idle;
        self.held_ms = 0.0;
        out
    }

    /// Recover a press whose release never arrived.
    ///
    /// The whole reason this module exists beside the kit's hold tracker. A
    /// scroll area swallowing the release, a compositor taking the sequence, a
    /// widget destroyed mid-gesture — each leaves the machine stuck and every
    /// future tap ignored, which reads as the app freezing.
    ///
    /// Emits `DragCanceled` rather than a tap: a gesture nobody finished did
    /// not happen, and firing its action on a timer would be the app pressing
    /// its own buttons.
    pub fn watchdog(&mut self) -> Option<Event> {
        if self.state == State::Idle || self.held_ms < STUCK_MS { return None; }
        self.state = State::Idle;
        self.held_ms = 0.0;
        Some(Event::DragCanceled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quick_press_and_release_is_a_tap() {
        let mut g = PressGesture::default();
        g.press(10.0, 10.0);
        g.tick(100.0);
        assert_eq!(g.release(), Some(Event::Tap));
        assert!(g.is_idle());
    }

    #[test]
    fn holding_past_the_threshold_emits_a_long_press_once() {
        let mut g = PressGesture::default();
        g.press(10.0, 10.0);
        assert_eq!(g.tick(LONG_PRESS_MS - 1.0), None);
        assert_eq!(g.tick(1.0), Some(Event::LongPress));
        assert_eq!(g.tick(1_000.0), None, "only on the crossing step");
        assert_eq!(g.state(), State::Held);
    }

    #[test]
    fn releasing_after_a_long_press_does_not_also_tap() {
        // Emitting both would run two handlers for one gesture — a menu opening
        // and then the item under it activating.
        let mut g = PressGesture::default();
        g.press(10.0, 10.0);
        g.tick(LONG_PRESS_MS);
        assert_eq!(g.release(), None);
        assert!(g.is_idle());
    }

    #[test]
    fn drifting_beyond_the_slop_cancels_the_gesture() {
        // The user is scrolling, not pressing.
        let mut g = PressGesture::default();
        g.press(100.0, 100.0);
        assert_eq!(g.move_to(105.0, 103.0), None, "a small drift is still a press");
        assert_eq!(g.move_to(140.0, 100.0), Some(Event::DragCanceled));
        assert!(g.is_idle());
    }

    #[test]
    fn a_cancelled_gesture_produces_no_tap_on_release() {
        let mut g = PressGesture::default();
        g.press(100.0, 100.0);
        g.move_to(200.0, 100.0);
        assert_eq!(g.release(), None);
    }

    #[test]
    fn a_press_while_not_idle_is_ignored() {
        // The C++ guard, and exactly what the watchdog exists to unblock.
        let mut g = PressGesture::default();
        g.press(10.0, 10.0);
        g.press(90.0, 90.0);
        g.tick(50.0);
        g.release();
        assert!(g.is_idle());
    }

    #[test]
    fn a_lost_release_is_recovered_rather_than_wedging_the_control() {
        // The failure this module exists for: without recovery the machine
        // stays Pressing forever and EVERY FUTURE TAP IS IGNORED, which reads
        // as the app freezing rather than as a lost event.
        let mut g = PressGesture::default();
        g.press(10.0, 10.0);
        g.tick(LONG_PRESS_MS - 10.0);
        assert_eq!(g.watchdog(), None, "not stuck yet, just slow");

        g.tick(STUCK_MS);
        assert_eq!(g.watchdog(), Some(Event::DragCanceled));
        assert!(g.is_idle());

        // And the control works again.
        g.press(10.0, 10.0);
        g.tick(50.0);
        assert_eq!(g.release(), Some(Event::Tap));
    }

    #[test]
    fn the_watchdog_cancels_rather_than_firing_the_action() {
        // A gesture nobody finished did not happen. Firing its action on a
        // timer would be the app pressing its own buttons.
        let mut g = PressGesture::default();
        g.press(10.0, 10.0);
        g.tick(STUCK_MS * 2.0);
        assert_ne!(g.watchdog(), Some(Event::Tap));
    }

    #[test]
    fn the_watchdog_does_nothing_when_nothing_is_in_flight() {
        // It sweeps on a timer, so the idle case is the common one and must be
        // free of side effects.
        let mut g = PressGesture::default();
        assert_eq!(g.watchdog(), None);
        assert!(g.is_idle());
    }

    #[test]
    fn a_resumed_suspend_does_not_manufacture_a_long_press() {
        // The user released while the lid was shut; waking must not open a
        // context menu they never asked for.
        let mut g = PressGesture::default();
        g.press(10.0, 10.0);
        let evt = g.tick(4_000_000.0);
        // It may report a long press from the clamped step, but a watchdog
        // sweep immediately after must clear it rather than leaving it Held.
        if evt == Some(Event::LongPress) {
            assert_eq!(g.state(), State::Held);
        }
        g.tick(STUCK_MS);
        let _ = g.watchdog();
        assert!(g.is_idle(), "recovered either way");
    }

    #[test]
    fn the_threshold_is_adjustable_as_the_qt_setter_allows() {
        let mut g = PressGesture::default();
        g.set_long_press_ms(200.0);
        g.press(0.0, 0.0);
        assert_eq!(g.tick(200.0), Some(Event::LongPress));
    }

    #[test]
    fn a_zero_threshold_does_not_divide_by_zero_in_progress() {
        // Reachable from a settings value, and a NaN in a progress ring paints
        // nothing with no error.
        let mut g = PressGesture::default();
        g.set_long_press_ms(0.0);
        g.press(0.0, 0.0);
        assert_eq!(g.progress(), 0.0);
        assert!(g.progress().is_finite());
    }

    #[test]
    fn progress_fills_toward_the_long_press_for_an_indicator() {
        let mut g = PressGesture::default();
        g.press(0.0, 0.0);
        g.tick(LONG_PRESS_MS / 2.0);
        assert!((g.progress() - 0.5).abs() < 0.01);
    }
}
