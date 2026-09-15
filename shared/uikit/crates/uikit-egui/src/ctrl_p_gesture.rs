//! ctrl_p_gesture — classify a continuous key-hold (Ctrl+P: tap toggles a
//! palette, hold opens voice) using the SAME timing primitive touch holds
//! already use in this kit.
//!
//! ## This almost became a second implementation of the same idea
//!
//! Built to answer "does the key need its own tap/hold timer" without
//! first checking whether one already existed in this crate. One did:
//! [`crate::cmdstrip::Hold`], built for touch (press-and-hold on a
//! button), citing voiceterm's own hold duration explicitly so "two apps
//! in the same fleet disagreeing about what a long-press feels like" —
//! its own words — would not happen. This module's first draft carried
//! its own independent 220ms threshold, discovered only once `Hold`'s
//! existing 320ms (`cmdstrip::HOLD_SECONDS`) was found — exactly the
//! disagreement `Hold`'s own doc comment already warned against, and
//! this module was about to become a second one.
//!
//! Fixed by making [`GestureTracker`] a thin ADAPTER: it translates
//! keyboard-style continuous polling (`key_down` — "is it physically down
//! right now", queried every frame) into `Hold`'s touch-style discrete
//! calls (`press()` once, `tick(dt)` every frame while held, `release()`
//! once), rather than reimplementing the timer. One real state machine
//! and one real threshold, used by both input shapes.
//!
//! ## Why the translation, not a shared call convention
//!
//! Touch and keyboard genuinely report state differently at the egui
//! layer: a touch press/release are discrete EVENTS (there is a frame
//! where the finger went down, a frame where it came up), while
//! `i.key_down(key)` is a continuous per-frame QUERY with no event to
//! hook `press()`/`release()` onto directly. Rather than change `Hold`'s
//! API to suit one caller, this adapter tracks the previous frame's
//! `down` state itself and calls `press()`/`release()` on the
//! transitions it detects — the minimum translation layer, not a
//! reshaping of the shared primitive around this one consumer.

use crate::cmdstrip::{Hold, Release};

/// What happened this frame, for the caller to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// Nothing to do this frame.
    None,
    /// The key was released before crossing the hold threshold: a tap.
    /// Caller should toggle the palette.
    Tap,
    /// The key is down and has crossed the hold threshold, for the first
    /// time this press (fires once, matching `Hold::tick`'s own
    /// fire-once contract). Caller should start voice.
    HoldStarted,
    /// The key was released after crossing the hold threshold. Caller
    /// should stop/finish voice, mirroring how releasing a push-to-talk
    /// key ends the utterance.
    HoldReleased,
}

/// Tracks one key's continuous-press state across frames by adapting it
/// onto [`Hold`]. One instance per binding.
#[derive(Debug, Clone, Default)]
pub struct GestureTracker {
    hold: Hold,
    was_down: bool,
}

impl GestureTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance by one frame. `down` is whether the key is physically held
    /// right now; `dt` is this frame's duration.
    pub fn poll(&mut self, down: bool, dt: std::time::Duration) -> Gesture {
        if down && !self.was_down {
            // Rising edge: translate into Hold's press() event.
            self.hold.press();
        }
        self.was_down = down;

        if down {
            if self.hold.tick(dt.as_secs_f32()) {
                return Gesture::HoldStarted;
            }
            return Gesture::None;
        }

        // Falling edge or already-up: release() is a no-op (returns
        // None) if nothing was held, so this is safe to call every frame
        // the key is up, not just on the exact transition frame.
        match self.hold.release() {
            Some(Release::Tap) => Gesture::Tap,
            Some(Release::Hold) => Gesture::HoldReleased,
            None => Gesture::None,
        }
    }

    /// How far into the hold this press is, 0.0 to 1.0 — delegates to
    /// [`Hold::progress`] directly rather than tracking a second copy of
    /// the same elapsed time.
    pub fn hold_progress(&self) -> f32 {
        self.hold.progress()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmdstrip::HOLD_SECONDS;
    use std::time::Duration;

    fn secs(s: f32) -> Duration {
        Duration::from_secs_f32(s)
    }

    #[test]
    fn a_quick_tap_reports_tap_on_release() {
        let mut t = GestureTracker::new();
        assert_eq!(t.poll(true, secs(0.0)), Gesture::None); // press-down
        assert_eq!(t.poll(true, secs(0.05)), Gesture::None); // still held, short
        assert_eq!(t.poll(false, secs(0.001)), Gesture::Tap); // released before threshold
    }

    #[test]
    fn holding_past_the_threshold_fires_hold_started_exactly_once() {
        let mut t = GestureTracker::new();
        assert_eq!(t.poll(true, secs(0.0)), Gesture::None);
        assert_eq!(t.poll(true, secs(0.10)), Gesture::None); // under threshold
        assert_eq!(t.poll(true, secs(0.25)), Gesture::HoldStarted); // crosses HOLD_SECONDS (0.32)
        // Continuing to hold must NOT re-fire HoldStarted every frame.
        assert_eq!(t.poll(true, secs(0.05)), Gesture::None);
        assert_eq!(t.poll(true, secs(1.0)), Gesture::None);
    }

    #[test]
    fn releasing_after_a_hold_reports_hold_released_not_tap() {
        let mut t = GestureTracker::new();
        t.poll(true, secs(0.0));
        t.poll(true, secs(0.40)); // crosses threshold -> HoldStarted (ignored here)
        assert_eq!(t.poll(false, secs(0.001)), Gesture::HoldReleased);
    }

    #[test]
    fn a_second_press_after_a_full_release_starts_fresh() {
        let mut t = GestureTracker::new();
        t.poll(true, secs(0.0));
        assert_eq!(t.poll(false, secs(0.05)), Gesture::Tap); // press 1: quick tap

        t.poll(true, secs(0.0));
        assert_eq!(t.poll(true, secs(0.40)), Gesture::HoldStarted); // press 2: a hold
        assert_eq!(t.poll(false, secs(0.001)), Gesture::HoldReleased);
    }

    #[test]
    fn key_up_with_no_prior_press_is_a_no_op() {
        let mut t = GestureTracker::new();
        assert_eq!(t.poll(false, secs(0.016)), Gesture::None);
        assert_eq!(t.poll(false, secs(0.016)), Gesture::None);
    }

    #[test]
    fn hold_progress_tracks_the_ratio_and_clamps_at_one() {
        let mut t = GestureTracker::new();
        assert_eq!(t.hold_progress(), 0.0); // key up
        t.poll(true, secs(0.0));
        assert_eq!(t.hold_progress(), 0.0);
        t.poll(true, secs(HOLD_SECONDS / 2.0));
        assert!((t.hold_progress() - 0.5).abs() < 0.01);
        t.poll(true, secs(2.0)); // well past the threshold
        assert_eq!(t.hold_progress(), 1.0, "must clamp, not overshoot past 1.0");
    }

    #[test]
    fn hold_progress_resets_to_zero_after_release() {
        let mut t = GestureTracker::new();
        t.poll(true, secs(0.0));
        t.poll(true, secs(0.40));
        t.poll(false, secs(0.001));
        assert_eq!(t.hold_progress(), 0.0, "released key must not report a stale progress");
    }

    #[test]
    fn a_press_landing_exactly_on_the_threshold_counts_as_a_hold() {
        let mut t = GestureTracker::new();
        t.poll(true, secs(0.0));
        assert_eq!(t.poll(true, secs(HOLD_SECONDS)), Gesture::HoldStarted);
    }

    #[test]
    fn uses_the_shared_threshold_not_an_independent_one() {
        // Pins the whole point of this rewrite: the threshold this module
        // fires HoldStarted at must be cmdstrip's own HOLD_SECONDS, not a
        // value chosen independently by this file. A press just short of
        // it must NOT fire; just at or past it must.
        let mut t = GestureTracker::new();
        t.poll(true, secs(0.0));
        assert_eq!(t.poll(true, secs(HOLD_SECONDS - 0.01)), Gesture::None);
        assert_eq!(t.poll(true, secs(0.01)), Gesture::HoldStarted);
    }
}
