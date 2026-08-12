//! shutter — `ShutterButton`, ported.
//!
//! One control, four gestures, and they have to coexist without a mode switch:
//!
//! * **Tap** (release under 500ms, no motion) → photo.
//! * **Hold** (>500ms) → recording starts, and the button turns red.
//! * **Swipe up while recording** → recording locks; the finger can leave.
//! * **Swipe down while idle** → the voice/text mode picker.
//!
//! ## Why the thresholds do not collide, in the Qt author's own words
//!
//! *"a tap stays within kTapMotionTolerancePx (8px) — well under the 40px
//! threshold — so none of the three gestures collide."* That is the load-bearing
//! sentence, and it is a claim about NUMBERS rather than about code: 8 for tap
//! tolerance, 40 for both swipes, 500ms for hold. Change any of them and the
//! gestures start eating each other — a 20px tap tolerance and a 25px lock
//! swipe would make every slightly-sloppy tap a lock.
//!
//! There is a test asserting the separation directly, so a future tweak to one
//! constant fails loudly rather than producing a shutter that occasionally does
//! the wrong thing.
//!
//! ## Direction is measured from the press, not between frames
//!
//! *"We measure FROM the press position, so the user can swipe in any
//! direction; only sustained upward motion locks."* Frame-to-frame deltas would
//! let a wobble accumulate into a lock, and a hand holding a phone at arm's
//! length wobbles constantly. Displacement from the anchor cannot.
//!
//! ## The subclassing note is a porting instruction
//!
//! The Qt version subclasses `QWidget` rather than `QPushButton`, because
//! *"Qt's click-detection logic in QPushButton would fight us — it auto-emits
//! clicked on release regardless of how long the press lasted"*. The same trap
//! exists in egui: `Button::clicked()` fires on any release. So this takes raw
//! press/move/release and never asks a widget whether it was clicked.

/// Press longer than this is a hold, not a tap.
pub const HOLD_MS: f32 = 500.0;
/// Maximum movement for a press to still count as a tap.
pub const TAP_TOLERANCE_PX: f32 = 8.0;
/// Upward movement, while recording, that locks.
pub const SWIPE_UP_PX: f32 = 40.0;
/// Downward movement, while idle, that opens the mode picker.
pub const SWIPE_DOWN_PX: f32 = 40.0;

/// What the shutter decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shutter {
    /// Take a photo.
    Photo,
    RecordStarted,
    /// Recording continues without the finger.
    RecordLocked,
    /// Finalised, with its duration.
    RecordStopped { duration_ms: u32 },
    /// Swipe-down while idle.
    OpenModePicker,
}

/// The button's state machine.
#[derive(Debug, Clone, PartialEq)]
pub struct ShutterButton {
    down: bool,
    press_at: (f32, f32),
    held_ms: f32,
    recording: bool,
    locked: bool,
    record_ms: f32,
    /// True once this press has produced a non-tap outcome, so releasing it
    /// cannot also fire a tap.
    consumed: bool,
}

impl Default for ShutterButton {
    fn default() -> Self {
        ShutterButton { down: false, press_at: (0.0, 0.0), held_ms: 0.0,
                        recording: false, locked: false, record_ms: 0.0,
                        consumed: false }
    }
}

impl ShutterButton {
    pub fn recording(&self) -> bool { self.recording }
    pub fn locked(&self) -> bool { self.locked }
    pub fn record_ms(&self) -> u32 { self.record_ms as u32 }
    pub fn held_ms(&self) -> f32 { self.held_ms }

    /// The caller reports recording started or stopped elsewhere.
    ///
    /// Present in the Qt widget for the same reason: recording can be ended by
    /// the encoder running out of space, and a button still glowing red after
    /// that is a button lying about the device.
    pub fn set_recording(&mut self, recording: bool) {
        self.recording = recording;
        if !recording { self.locked = false; self.record_ms = 0.0; }
    }

    /// Press down.
    pub fn press(&mut self, x: f32, y: f32) -> Option<Shutter> {
        // A tap while LOCKED stops the recording, and does so on press rather
        // than release: the user is stopping something, and waiting for the
        // lift adds latency to the one gesture that wants none.
        if self.locked {
            let d = self.record_ms as u32;
            self.set_recording(false);
            self.consumed = true;
            self.down = true;
            self.press_at = (x, y);
            return Some(Shutter::RecordStopped { duration_ms: d });
        }
        self.down = true;
        self.press_at = (x, y);
        self.held_ms = 0.0;
        self.consumed = false;
        None
    }

    /// Finger moved.
    ///
    /// Displacement is measured FROM THE PRESS, not between frames: a wobble
    /// accumulating frame to frame would eventually cross any threshold, and a
    /// hand holding a phone wobbles constantly.
    pub fn moved(&mut self, x: f32, y: f32) -> Option<Shutter> {
        if !self.down || self.consumed { return None; }
        let dy = y - self.press_at.1;

        if self.recording && !self.locked {
            // Upward is negative in screen coordinates.
            if -dy >= SWIPE_UP_PX {
                self.locked = true;
                self.consumed = true;
                return Some(Shutter::RecordLocked);
            }
            return None;
        }

        if !self.recording && dy >= SWIPE_DOWN_PX {
            // Swiping down neither snaps a photo on release nor lets the hold
            // timer start a recording — the Qt comment is explicit, and
            // `consumed` is what enforces it here.
            self.consumed = true;
            self.down = false;
            return Some(Shutter::OpenModePicker);
        }
        None
    }

    /// Advance time while held.
    pub fn tick(&mut self, dt_ms: f32) -> Option<Shutter> {
        if self.recording { self.record_ms += dt_ms; }
        if !self.down || self.consumed { return None; }
        self.held_ms += dt_ms;
        if !self.recording && self.held_ms >= HOLD_MS {
            self.recording = true;
            self.record_ms = 0.0;
            return Some(Shutter::RecordStarted);
        }
        None
    }

    /// Lift.
    pub fn release(&mut self, x: f32, y: f32) -> Option<Shutter> {
        if !self.down { return None; }
        self.down = false;
        if self.consumed { return None; }

        let moved = (x - self.press_at.0).hypot(y - self.press_at.1);

        if self.recording {
            // Released while recording and not locked: stop. This is the
            // press-and-hold-to-film gesture completing.
            if self.locked { return None; }
            let d = self.record_ms as u32;
            self.set_recording(false);
            return Some(Shutter::RecordStopped { duration_ms: d });
        }

        // A quick press that stayed still is a photo. Both conditions: a long
        // press has already started recording, and a press that wandered was
        // aiming at something else.
        if self.held_ms < HOLD_MS && moved <= TAP_TOLERANCE_PX {
            return Some(Shutter::Photo);
        }
        None
    }

    /// The gesture was interrupted — a call, an app switch.
    ///
    /// Stops an unlocked recording rather than leaving it running: the user is
    /// no longer holding anything, and a recording nobody is holding and nobody
    /// locked is one nobody asked to continue.
    pub fn cancel(&mut self) -> Option<Shutter> {
        self.down = false;
        self.consumed = false;
        if self.recording && !self.locked {
            let d = self.record_ms as u32;
            self.set_recording(false);
            return Some(Shutter::RecordStopped { duration_ms: d });
        }
        None
    }
}

/// Draw the shutter.
pub fn shutter_ui(ui: &mut egui::Ui, s: &ShutterButton, size: f32,
                  c: &uikit_egui::PaletteColors) -> egui::Response {
    // Sense drag, not click: `clicked()` fires on any release regardless of how
    // long the press lasted, which is exactly the trap the Qt version avoided
    // by not using QPushButton.
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size),
                                              egui::Sense::click_and_drag());
    let centre = rect.center();
    let r = size * 0.5 - 3.0;
    let ring = if s.recording() { c.danger } else { c.text };
    ui.painter().circle_stroke(centre, r, egui::Stroke::new(3.0_f32, ring));
    if s.recording() {
        // A filled square while recording, and a locked recording gets a
        // second ring — the user needs to know their finger is no longer load
        // bearing before they lift it.
        let side = r * 0.8;
        ui.painter().rect_filled(
            egui::Rect::from_center_size(centre, egui::vec2(side, side)), 3.0, c.danger);
        if s.locked() {
            ui.painter().circle_stroke(centre, r + 5.0, egui::Stroke::new(2.0_f32, c.danger));
        }
    } else {
        ui.painter().circle_filled(centre, r * 0.82, c.text);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn btn() -> ShutterButton { ShutterButton::default() }

    #[test]
    fn a_quick_still_press_takes_a_photo() {
        let mut b = btn();
        b.press(100.0, 100.0);
        b.tick(120.0);
        assert_eq!(b.release(100.0, 100.0), Some(Shutter::Photo));
    }

    #[test]
    fn holding_starts_a_recording_without_waiting_for_the_lift() {
        // The button turns red while the finger is still down; waiting for
        // release would mean the user cannot tell it is filming.
        let mut b = btn();
        b.press(100.0, 100.0);
        assert_eq!(b.tick(HOLD_MS - 1.0), None);
        assert_eq!(b.tick(2.0), Some(Shutter::RecordStarted));
        assert!(b.recording());
    }

    #[test]
    fn releasing_a_hold_stops_the_recording_with_its_duration() {
        let mut b = btn();
        b.press(100.0, 100.0);
        b.tick(HOLD_MS + 1.0);
        b.tick(2_000.0);
        match b.release(100.0, 100.0) {
            Some(Shutter::RecordStopped { duration_ms }) => {
                assert!(duration_ms >= 2_000, "got {duration_ms}");
            }
            other => panic!("expected RecordStopped, got {other:?}"),
        }
        assert!(!b.recording());
    }

    #[test]
    fn a_long_press_does_not_also_take_a_photo_on_release() {
        // The trap the Qt version subclassed QWidget to avoid: a button that
        // auto-emits clicked on release regardless of press duration.
        let mut b = btn();
        b.press(100.0, 100.0);
        b.tick(HOLD_MS + 100.0);
        assert!(!matches!(b.release(100.0, 100.0), Some(Shutter::Photo)));
    }

    #[test]
    fn a_press_that_wandered_is_not_a_photo() {
        // It was aiming at something else — a swipe that did not quite reach
        // its threshold should do nothing rather than fire the default action.
        let mut b = btn();
        b.press(100.0, 100.0);
        b.tick(100.0);
        assert_eq!(b.release(100.0, 100.0 + TAP_TOLERANCE_PX + 5.0), None);
    }

    #[test]
    fn a_tap_within_tolerance_still_counts() {
        // Nobody presses a phone perfectly still.
        let mut b = btn();
        b.press(100.0, 100.0);
        b.tick(100.0);
        assert_eq!(b.release(103.0, 104.0), Some(Shutter::Photo));
    }

    #[test]
    fn swiping_up_while_recording_locks_it() {
        let mut b = btn();
        b.press(100.0, 200.0);
        b.tick(HOLD_MS + 1.0);
        assert_eq!(b.moved(100.0, 200.0 - SWIPE_UP_PX), Some(Shutter::RecordLocked));
        assert!(b.locked());
    }

    #[test]
    fn a_locked_recording_survives_the_lift() {
        // That is the entire point of locking.
        let mut b = btn();
        b.press(100.0, 200.0);
        b.tick(HOLD_MS + 1.0);
        b.moved(100.0, 200.0 - SWIPE_UP_PX);
        assert_eq!(b.release(100.0, 150.0), None);
        assert!(b.recording());
    }

    #[test]
    fn tapping_a_locked_recording_stops_it() {
        let mut b = btn();
        b.press(100.0, 200.0);
        b.tick(HOLD_MS + 1.0);
        b.moved(100.0, 200.0 - SWIPE_UP_PX);
        b.release(100.0, 150.0);
        b.tick(1_500.0);
        match b.press(100.0, 200.0) {
            Some(Shutter::RecordStopped { duration_ms }) => assert!(duration_ms >= 1_500),
            other => panic!("expected RecordStopped, got {other:?}"),
        }
        assert!(!b.recording());
    }

    #[test]
    fn swiping_down_while_idle_opens_the_mode_picker() {
        let mut b = btn();
        b.press(100.0, 100.0);
        assert_eq!(b.moved(100.0, 100.0 + SWIPE_DOWN_PX), Some(Shutter::OpenModePicker));
    }

    #[test]
    fn swiping_down_neither_snaps_a_photo_nor_starts_a_recording() {
        // The Qt comment is explicit about both halves.
        let mut b = btn();
        b.press(100.0, 100.0);
        b.moved(100.0, 100.0 + SWIPE_DOWN_PX);
        assert_eq!(b.tick(HOLD_MS * 2.0), None, "no recording");
        assert_eq!(b.release(100.0, 140.0), None, "no photo");
        assert!(!b.recording());
    }

    #[test]
    fn swiping_down_while_recording_does_not_open_the_picker() {
        // Down is the idle gesture; during a recording the only swipe that
        // means anything is up.
        let mut b = btn();
        b.press(100.0, 100.0);
        b.tick(HOLD_MS + 1.0);
        assert_eq!(b.moved(100.0, 100.0 + SWIPE_DOWN_PX * 2.0), None);
        assert!(b.recording() && !b.locked());
    }

    #[test]
    fn the_thresholds_cannot_collide() {
        // The Qt author's load-bearing sentence, asserted rather than trusted:
        // "a tap stays within 8px — well under the 40px threshold — so none of
        // the three gestures collide." A future tweak to one constant should
        // fail here rather than produce a shutter that occasionally does the
        // wrong thing.
        assert!(TAP_TOLERANCE_PX * 2.0 < SWIPE_UP_PX);
        assert!(TAP_TOLERANCE_PX * 2.0 < SWIPE_DOWN_PX);
    }

    #[test]
    fn displacement_is_measured_from_the_press_not_between_frames() {
        // A wobble accumulating frame to frame would eventually cross any
        // threshold, and a hand holding a phone wobbles constantly.
        let mut b = btn();
        b.press(100.0, 200.0);
        b.tick(HOLD_MS + 1.0);
        for i in 0..30 {
            // Drifting up and back, never more than 10px from the anchor.
            let y = 200.0 - (i % 2) as f32 * 10.0;
            assert_eq!(b.moved(100.0, y), None, "wobble at step {i}");
        }
        assert!(!b.locked());
    }

    #[test]
    fn an_interrupted_gesture_stops_an_unlocked_recording() {
        // A call, an app switch. A recording nobody is holding and nobody
        // locked is one nobody asked to continue.
        let mut b = btn();
        b.press(100.0, 100.0);
        b.tick(HOLD_MS + 1.0);
        b.tick(800.0);
        assert!(matches!(b.cancel(), Some(Shutter::RecordStopped { .. })));
        assert!(!b.recording());
    }

    #[test]
    fn an_interruption_does_not_stop_a_locked_recording() {
        // Locked means hands-free; that is what the user asked for, and an
        // incoming notification is not a reason to end it.
        let mut b = btn();
        b.press(100.0, 200.0);
        b.tick(HOLD_MS + 1.0);
        b.moved(100.0, 200.0 - SWIPE_UP_PX);
        b.release(100.0, 150.0);
        assert_eq!(b.cancel(), None);
        assert!(b.recording());
    }

    #[test]
    fn recording_stopped_externally_clears_the_red_ring() {
        // The encoder running out of space ends a recording, and a button still
        // glowing red after that is lying about the device.
        let mut b = btn();
        b.press(100.0, 100.0);
        b.tick(HOLD_MS + 1.0);
        b.set_recording(false);
        assert!(!b.recording());
        assert!(!b.locked());
    }

    #[test]
    fn a_release_without_a_press_does_nothing() {
        // Events can arrive out of order after an interruption.
        let mut b = btn();
        assert_eq!(b.release(100.0, 100.0), None);
        assert_eq!(b.moved(100.0, 200.0), None);
    }
}
