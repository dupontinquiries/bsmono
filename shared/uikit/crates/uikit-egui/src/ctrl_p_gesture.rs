//! ctrl_p_gesture — classify a Ctrl+P press as a tap (toggle the palette)
//! or a hold (open voice, morphing the palette into it), pure and testable
//! without a frame.
//!
//! ## Why this needs its own state, not just `key_pressed`/`key_down`
//!
//! The existing Ctrl+P binding uses `key_pressed` — an edge-triggered
//! "did the key transition down-to-up... no, up-to-down this frame" check,
//! fine for a single discrete action. Telling a TAP from a HOLD needs to
//! know how long the key has been down, which `key_pressed`/`key_down`
//! alone do not track — egui reports "is the key down right now", not
//! "for how long". This module is the small piece of state that answers
//! that question, kept separate from both the palette's own state and
//! voice's, since press/hold classification is a distinct concern neither
//! of them should have to reimplement if a THIRD hold-triggered binding
//! ever wants the same shape.
//!
//! ## The threshold, and why it is a constant here rather than tuned per
//! call site
//!
//! 220ms. Reasoned, not measured (same caveat every other unmeasured
//! constant in this session carries — `power_profile::
//! FOREGROUND_ECO_UTIL_MAX`, `poll_schedule::ScheduleConfig::default`'s
//! hysteresis): long enough that an ordinary tap-and-release (even a
//! slightly slow one) reads as a tap, short enough that a deliberate hold
//! does not feel laggy before voice starts responding. Needs a real
//! keyboard and a real hand to tune, not arithmetic.
//!
//! ## What this module does NOT decide
//!
//! Whether Ctrl+P is even the right binding for a given app, what "open
//! voice" or "toggle the palette" DO once decided, and how the visual
//! morph is drawn. Those are the caller's job — this module answers
//! exactly one question, "given how long the key has been continuously
//! down, and whether it JUST came up, what should happen", as data.

use std::time::Duration;

/// How long a continuous press must last before it counts as a hold
/// rather than a tap.
pub const HOLD_THRESHOLD: Duration = Duration::from_millis(220);

/// What happened this frame, for the caller to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// Nothing to do — key is up and was not just released, or is down
    /// but has not crossed the hold threshold yet.
    None,
    /// The key was released before crossing the hold threshold: a tap.
    /// Caller should toggle the palette.
    Tap,
    /// The key is down and HAS crossed the hold threshold, for the first
    /// time this press (fires once, not every frame past the threshold —
    /// see [`GestureTracker::poll`]'s doc for why that matters to the
    /// caller). Caller should start voice.
    HoldStarted,
    /// The key was released AFTER crossing the hold threshold (a hold
    /// that is now ending). Caller should stop/finish voice, mirroring
    /// how releasing a push-to-talk key ends the utterance.
    HoldReleased,
}

/// Tracks one key's continuous-press state across frames. One instance
/// per binding — do not share between Ctrl+P and some other hold-triggered
/// key, since `held_since`/`hold_fired` describe exactly one physical key's
/// history.
#[derive(Debug, Clone, Copy, Default)]
pub struct GestureTracker {
    /// `Some(d)` = key has been continuously down for `d` as of the last
    /// poll; `None` = key is up.
    held_for: Option<Duration>,
    /// Whether `HoldStarted` has already been emitted for the CURRENT
    /// press, so it fires exactly once per press rather than every frame
    /// once the threshold is crossed.
    hold_fired: bool,
}

impl GestureTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance by one frame. `down` is whether the key is physically held
    /// right now; `dt` is this frame's duration (egui's `ctx.input(|i|
    /// i.stable_dt)` on a real call site — a parameter here, not read
    /// internally, so this stays testable without a frame).
    pub fn poll(&mut self, down: bool, dt: Duration) -> Gesture {
        match (self.held_for, down) {
            (None, false) => Gesture::None,
            (None, true) => {
                // Press just started.
                self.held_for = Some(dt);
                self.hold_fired = false;
                Gesture::None
            }
            (Some(_), false) => {
                // Release. Classify by whether the hold threshold was
                // ever crossed during this press — not by the duration
                // at the moment of release, which is the same test but
                // stated the way the actual field (`hold_fired`) makes
                // it: a press that crossed the threshold and then got
                // released a frame later is still a hold-that-ended, not
                // a tap, even though the LAST recorded duration might sit
                // right at the boundary.
                let was_hold = self.hold_fired;
                self.held_for = None;
                self.hold_fired = false;
                if was_hold { Gesture::HoldReleased } else { Gesture::Tap }
            }
            (Some(prev), true) => {
                let now = prev + dt;
                self.held_for = Some(now);
                if now >= HOLD_THRESHOLD && !self.hold_fired {
                    self.hold_fired = true;
                    Gesture::HoldStarted
                } else {
                    Gesture::None
                }
            }
        }
    }

    /// How far into the hold this press is, 0.0 at press-down to 1.0 at
    /// the threshold and beyond — the raw input to a morph animation
    /// (feed through `ctx.animate_value_with_time` or similar for
    /// smoothing; this returns the unsmoothed ratio). 0.0 while the key
    /// is up.
    pub fn hold_progress(&self) -> f32 {
        match self.held_for {
            None => 0.0,
            Some(d) => (d.as_secs_f32() / HOLD_THRESHOLD.as_secs_f32()).min(1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration { Duration::from_millis(n) }

    #[test]
    fn a_quick_tap_reports_tap_on_release() {
        let mut t = GestureTracker::new();
        assert_eq!(t.poll(true, ms(0)), Gesture::None); // press-down
        assert_eq!(t.poll(true, ms(50)), Gesture::None); // still held, short
        assert_eq!(t.poll(false, ms(1)), Gesture::Tap); // released before threshold
    }

    #[test]
    fn holding_past_the_threshold_fires_hold_started_exactly_once() {
        let mut t = GestureTracker::new();
        assert_eq!(t.poll(true, ms(0)), Gesture::None);
        assert_eq!(t.poll(true, ms(100)), Gesture::None); // 100ms, under 220
        assert_eq!(t.poll(true, ms(150)), Gesture::HoldStarted); // 250ms, crosses
        // Continuing to hold must NOT re-fire HoldStarted every frame --
        // this is the property `hold_fired` exists to guarantee, and the
        // one most likely to regress silently if a future edit reset the
        // flag somewhere it should not.
        assert_eq!(t.poll(true, ms(50)), Gesture::None);
        assert_eq!(t.poll(true, ms(1000)), Gesture::None);
    }

    #[test]
    fn releasing_after_a_hold_reports_hold_released_not_tap() {
        let mut t = GestureTracker::new();
        t.poll(true, ms(0));
        t.poll(true, ms(300)); // crosses threshold -> HoldStarted (ignored here)
        assert_eq!(t.poll(false, ms(1)), Gesture::HoldReleased);
    }

    #[test]
    fn a_second_press_after_a_full_release_starts_fresh() {
        // The state must not leak between presses -- a tap, then a hold,
        // must classify independently rather than the second press
        // inheriting `hold_fired` from the first.
        let mut t = GestureTracker::new();
        t.poll(true, ms(0));
        assert_eq!(t.poll(false, ms(50)), Gesture::Tap); // press 1: quick tap

        t.poll(true, ms(0));
        assert_eq!(t.poll(true, ms(300)), Gesture::HoldStarted); // press 2: a hold
        assert_eq!(t.poll(false, ms(1)), Gesture::HoldReleased);
    }

    #[test]
    fn key_up_with_no_prior_press_is_a_no_op() {
        let mut t = GestureTracker::new();
        assert_eq!(t.poll(false, ms(16)), Gesture::None);
        assert_eq!(t.poll(false, ms(16)), Gesture::None);
    }

    #[test]
    fn hold_progress_tracks_the_ratio_and_clamps_at_one() {
        let mut t = GestureTracker::new();
        assert_eq!(t.hold_progress(), 0.0); // key up
        t.poll(true, ms(0));
        assert_eq!(t.hold_progress(), 0.0);
        t.poll(true, ms(110)); // half of 220ms
        assert!((t.hold_progress() - 0.5).abs() < 0.01);
        t.poll(true, ms(500)); // well past the threshold
        assert_eq!(t.hold_progress(), 1.0, "must clamp, not overshoot past 1.0");
    }

    #[test]
    fn hold_progress_resets_to_zero_after_release() {
        let mut t = GestureTracker::new();
        t.poll(true, ms(0));
        t.poll(true, ms(300));
        t.poll(false, ms(1));
        assert_eq!(t.hold_progress(), 0.0, "released key must not report a stale progress");
    }

    #[test]
    fn a_press_landing_exactly_on_the_threshold_counts_as_a_hold() {
        // Boundary case: `>=`, not `>`, in the poll() comparison -- pinned
        // explicitly so a future refactor does not flip it to a strict
        // inequality and silently require one frame longer than the
        // documented threshold to trigger.
        let mut t = GestureTracker::new();
        t.poll(true, ms(0));
        assert_eq!(t.poll(true, HOLD_THRESHOLD), Gesture::HoldStarted);
    }
}
