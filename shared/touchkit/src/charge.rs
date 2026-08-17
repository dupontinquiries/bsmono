//! charge — a press that has to be *held* before it commits, with a charge level to draw.
//!
//! The plain long-press (see [`crate::Gesture::LongPress`]) answers "was this held?". This answers
//! "how long, out of how long is required, and did the finger stay put?" — because the interesting
//! part is the FEEDBACK. A control that fires after an invisible delay feels broken twice: once
//! when nothing happens, and again when something happens after you gave up.
//!
//! ## Why the duration is a parameter and not a constant
//!
//! It came out of a real disagreement between two apps sharing this kit.
//!
//! A quick pane-flipper (voiceterm) wants ~250ms: the fan is how you move around, it is used
//! constantly, and a long charge would make the whole UI feel stuck. A long-horizon thinking tool
//! (logscope) wants ~1000ms for the same widget, because there the fan changes SETTINGS on runs
//! that continue for hours or days — a mis-swipe that silently retunes a worker is a fault nobody
//! notices until the output is wrong, long after the gesture is forgotten. Same widget, opposite
//! correct answers, and the deciding question is not screen size or input device but *what the
//! control costs when it fires by accident*.
//!
//! So: [`ChargeConfig::quick`] and [`ChargeConfig::deliberate`] as named presets, arbitrary
//! durations allowed, and [`Charge::level`] exposed so the shell can draw the wait instead of
//! hiding it.
//!
//! ## The scroll rule
//!
//! A drag that begins before the charge completes is a SCROLL, and the finger stays disqualified
//! even if it later sits still long enough to finish charging. Without this, every attempt to
//! scroll a list by starting the drag on a button opens that button's menu instead — the most
//! irritating failure a touch surface has. `slop` is the distance that decides.

/// How long the press must last, and how far it may wander first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChargeConfig {
    pub hold_ms: u64,
    pub slop: f32,
}

impl ChargeConfig {
    /// Fast surfaces where the gesture is navigation and gets used constantly.
    pub const fn quick() -> Self { Self { hold_ms: 250, slop: 12.0 } }

    /// Surfaces where firing by accident is expensive: settings on long-running work, anything
    /// whose damage is discovered days later. The charge is long enough to be *interrupted* on
    /// purpose — that is the feature, not friction for its own sake.
    pub const fn deliberate() -> Self { Self { hold_ms: 1000, slop: 12.0 } }

    pub const fn with_hold_ms(mut self, ms: u64) -> Self { self.hold_ms = ms; self }
}

impl Default for ChargeConfig {
    fn default() -> Self { Self::quick() }
}

/// What the gesture produced this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChargeOutcome {
    /// Nothing yet (may still be charging — check [`Charge::level`]).
    Idle,
    /// Short press, released before the charge completed: the primary action.
    Tap,
    /// Charged and open; the payload is whatever option the caller says is under the pointer.
    Open(Option<usize>),
    /// Released while open, over an option.
    Commit(usize),
    /// Released over nothing, or the press became a scroll.
    Cancel,
}

/// The live gesture. One per control, kept by the caller across frames.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Charge {
    cfg_hold_ms: u64,
    held_ms: u64,
    open: bool,
    /// Latched once the press is disqualified as a scroll. It does not re-arm: by the time the
    /// finger stops, the list under it has already moved, and opening then would be a menu the
    /// operator did not ask for on content they were not looking at.
    scrolling: bool,
}

impl Charge {
    pub fn new() -> Self { Self::default() }

    /// 0.0 → 1.0, how far this press has charged. The shell draws this — a border that fills, a
    /// ring that closes. Zero when nothing is pressed or the press was disqualified, so the
    /// animation also communicates the scroll rule: start moving and the charge visibly dies.
    pub fn level(&self) -> f32 {
        if self.scrolling || self.cfg_hold_ms == 0 { return 0.0; }
        (self.held_ms as f32 / self.cfg_hold_ms as f32).clamp(0.0, 1.0)
    }

    pub fn is_open(&self) -> bool { self.open }

    /// Consume one frame. `down` is whether the pointer is held on the control, `elapsed_ms` how
    /// long since the press began, `travel` how far it has moved since, `over` the option index
    /// under the pointer (the caller hit-tests, since it owns the geometry).
    pub fn feed(&mut self, cfg: ChargeConfig, down: bool, elapsed_ms: u64, travel: f32,
                over: Option<usize>) -> ChargeOutcome {
        self.cfg_hold_ms = cfg.hold_ms;
        if down {
            self.held_ms = elapsed_ms;
            if !self.open && travel > cfg.slop && elapsed_ms < cfg.hold_ms { self.scrolling = true; }
            if !self.scrolling && elapsed_ms >= cfg.hold_ms { self.open = true; }
            return if self.open { ChargeOutcome::Open(over) } else { ChargeOutcome::Idle };
        }
        let (was_open, scrolled) = (self.open, self.scrolling);
        // The RELEASE frame carries the true elapsed time; the stored value is only as fresh as
        // the last frame we were handed. Trusting the stored one turns a press that spanned a
        // backgrounded app into a Tap — a stale clock reporting a short press is exactly what the
        // long-charge case exists to refuse.
        let held = elapsed_ms.max(self.held_ms);
        *self = Self::default();
        if was_open {
            match over { Some(i) => ChargeOutcome::Commit(i), None => ChargeOutcome::Cancel }
        } else if scrolled || held >= cfg.hold_ms {
            // Held past the threshold but never opened: only reachable if frames stopped arriving
            // mid-charge. Acting on a gesture we lost track of is worse than dropping it.
            ChargeOutcome::Cancel
        } else {
            ChargeOutcome::Tap
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_gesture_needs_four_times_the_hold_on_a_deliberate_surface() {
        // The reason this is a parameter: 400ms opens the quick surface and is still charging on
        // the deliberate one, where firing by accident costs a day of wrong work.
        let (q, d) = (ChargeConfig::quick(), ChargeConfig::deliberate());
        let mut a = Charge::new();
        let mut b = Charge::new();
        assert_eq!(a.feed(q, true, 400, 0.0, None), ChargeOutcome::Open(None));
        assert_eq!(b.feed(d, true, 400, 0.0, None), ChargeOutcome::Idle);
        assert!((b.level() - 0.4).abs() < 0.01, "and the wait is DRAWN: {}", b.level());
        assert_eq!(b.feed(d, true, 1000, 0.0, None), ChargeOutcome::Open(None));
    }

    #[test]
    fn releasing_mid_charge_is_a_TAP_not_a_half_commit() {
        let mut c = Charge::new();
        c.feed(ChargeConfig::deliberate(), true, 300, 0.0, None);
        assert_eq!(c.feed(ChargeConfig::deliberate(), false, 320, 0.0, None), ChargeOutcome::Tap);
        assert_eq!(c.level(), 0.0, "and the charge resets, so the next press starts empty");
    }

    #[test]
    fn an_EARLY_drag_is_a_scroll_forever_and_the_charge_visibly_dies() {
        let cfg = ChargeConfig::quick();
        let mut c = Charge::new();
        c.feed(cfg, true, 40, 0.0, None);
        c.feed(cfg, true, 80, cfg.slop + 4.0, None);
        assert_eq!(c.level(), 0.0, "the animation must show the charge dying, not keep filling");
        assert_eq!(c.feed(cfg, true, 5_000, cfg.slop + 4.0, None), ChargeOutcome::Idle,
                   "a scrolling finger must not open later, however long it then rests");
        assert_eq!(c.feed(cfg, false, 5_100, cfg.slop + 4.0, None), ChargeOutcome::Cancel);
    }

    #[test]
    fn a_drag_AFTER_opening_is_selection_not_scrolling() {
        let cfg = ChargeConfig::quick();
        let mut c = Charge::new();
        c.feed(cfg, true, cfg.hold_ms, 1.0, None);
        assert_eq!(c.feed(cfg, true, cfg.hold_ms + 90, 80.0, Some(2)), ChargeOutcome::Open(Some(2)));
        assert_eq!(c.feed(cfg, false, cfg.hold_ms + 120, 80.0, Some(2)), ChargeOutcome::Commit(2));
    }

    #[test]
    fn releasing_off_the_options_cancels_and_fires_nothing() {
        let cfg = ChargeConfig::quick();
        let mut c = Charge::new();
        c.feed(cfg, true, cfg.hold_ms, 0.0, None);
        assert_eq!(c.feed(cfg, false, cfg.hold_ms + 10, 90.0, None), ChargeOutcome::Cancel);
    }

    #[test]
    fn a_gesture_that_LOSES_its_frames_mid_charge_cancels_rather_than_guessing() {
        let cfg = ChargeConfig::deliberate();
        let mut c = Charge::new();
        c.feed(cfg, true, 10, 0.0, None);
        assert_eq!(c.feed(cfg, false, 8_000, 0.0, None), ChargeOutcome::Cancel,
                   "an app that was backgrounded mid-press must not commit on return");
    }

    #[test]
    fn a_zero_hold_config_is_not_a_division_by_zero() {
        let cfg = ChargeConfig::quick().with_hold_ms(0);
        let mut c = Charge::new();
        assert_eq!(c.feed(cfg, true, 0, 0.0, None), ChargeOutcome::Open(None));
        assert_eq!(c.level(), 0.0);
    }
}
