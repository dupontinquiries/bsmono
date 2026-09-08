//! Tooltips that teach without getting in the way.
//!
//! A video player has gestures nobody can discover by looking, so it needs to
//! say what they are — and a player that covers the picture to explain itself
//! is worse than one that stays quiet. Three rules follow, and all three are
//! about restraint:
//!
//! 1. **Delayed, then faded in.** A tooltip that appears the instant the
//!    pointer crosses a control fires constantly while someone is just moving
//!    the mouse across the window. The delay means it only appears when
//!    somebody has stopped and looked.
//! 2. **An icon, not a sentence.** The hint is a glyph plus a few words. The
//!    long explanation belongs in settings, where the reader went looking for
//!    it.
//! 3. **They stop appearing once learned.** A hint shown three times has done
//!    its job; showing it forever is nagging, and it is the fourth time that
//!    makes people resent it. [`Tips::seen`] tracks that.
//!
//! The fade itself is `alpha`, so a caller can use it with `set_opacity` on
//! whatever surface it likes rather than being handed a widget.

use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Copy, Clone, Debug)]
pub struct TipPolicy {
    /// Pointer must rest this long before a tip appears.
    pub delay: Duration,
    /// Fade-in duration once the delay has passed.
    pub fade: Duration,
    /// After this many showings a tip is considered learned and stops.
    pub teach_times: u32,
}

impl Default for TipPolicy {
    fn default() -> Self {
        Self {
            delay: Duration::from_millis(600),
            fade: Duration::from_millis(180),
            teach_times: 3,
        }
    }
}

/// Tracks what has been taught and how far the current tip has faded in.
#[derive(Default)]
pub struct Tips {
    pub policy: TipPolicy,
    shown: HashMap<&'static str, u32>,
    /// Which tip the pointer is currently resting on, and since when.
    hovering: Option<(&'static str, Instant)>,
}

impl Tips {
    pub fn new(policy: TipPolicy) -> Self {
        Self {
            policy,
            ..Default::default()
        }
    }

    /// How many times a tip has been shown to completion.
    pub fn seen(&self, id: &'static str) -> u32 {
        self.shown.get(id).copied().unwrap_or(0)
    }

    /// Has this hint done its job?
    pub fn learned(&self, id: &'static str) -> bool {
        self.seen(id) >= self.policy.teach_times
    }

    /// Forget everything, so the hints teach again.
    ///
    /// Worth a button in settings: someone who has not opened the app in six
    /// months has not "learned" the gestures in any useful sense, and a hint
    /// system with no way back is one that silently stops helping.
    pub fn reset(&mut self) {
        self.shown.clear();
        self.hovering = None;
    }

    /// The pointer left every control.
    pub fn idle(&mut self) {
        self.hovering = None;
    }

    /// Opacity for `id` while the pointer rests on it, 0 when it should not
    /// show at all.
    ///
    /// Call every frame for the control under the pointer; the state machine
    /// is here so the widget code stays a single call.
    pub fn alpha(&mut self, id: &'static str, now: Instant) -> f32 {
        if self.learned(id) {
            return 0.0;
        }
        match self.hovering {
            Some((cur, since)) if cur == id => {
                let waited = now.saturating_duration_since(since);
                if waited < self.policy.delay {
                    return 0.0;
                }
                let into = waited - self.policy.delay;
                if self.policy.fade.is_zero() {
                    return 1.0;
                }
                let a = (into.as_secs_f32() / self.policy.fade.as_secs_f32()).clamp(0.0, 1.0);
                if a >= 1.0 {
                    // Counted once it is fully visible, not when it starts
                    // fading: a tip glimpsed at 10% opacity while the pointer
                    // passes over has taught nobody anything, and counting it
                    // would retire the hint before it was ever read.
                    let n = self.shown.entry(id).or_insert(0);
                    if *n < self.policy.teach_times {
                        *n += 1;
                    }
                }
                a
            }
            _ => {
                // Moved to a different control: restart, so crossing three
                // buttons does not show three half-faded tips.
                self.hovering = Some((id, now));
                0.0
            }
        }
    }

    /// When to repaint next to keep a fade smooth, or `None` when nothing is
    /// animating — a settled tooltip must not keep the GPU awake.
    pub fn next_repaint(&self, id: &'static str, now: Instant) -> Option<Duration> {
        if self.learned(id) {
            return None;
        }
        let (cur, since) = self.hovering?;
        if cur != id {
            return None;
        }
        let waited = now.saturating_duration_since(since);
        if waited < self.policy.delay {
            return Some(self.policy.delay - waited);
        }
        if waited < self.policy.delay + self.policy.fade {
            return Some(Duration::from_millis(16));
        }
        None
    }
}

/// A hint: a glyph and a few words. Deliberately not a sentence.
#[derive(Copy, Clone, Debug)]
pub struct Tip {
    pub id: &'static str,
    pub icon: &'static str,
    pub text: &'static str,
}

/// The gesture hints, which are the ones nobody can discover by looking.
pub const GESTURE_TIPS: &[Tip] = &[
    Tip { id: "scrub", icon: "↔", text: "drag to scrub · higher = faster" },
    Tip { id: "volume", icon: "🔊", text: "left half · drag up" },
    Tip { id: "speed", icon: "⏩", text: "right half · drag up" },
    Tip { id: "rotate", icon: "↻", text: "right edge · drag down" },
    Tip { id: "zoom", icon: "⛶", text: "pinch · 5% steps" },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Instant {
        static B: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *B.get_or_init(Instant::now)
    }

    fn at(ms: u64) -> Instant {
        base() + Duration::from_millis(ms)
    }

    /// A tooltip that appears the instant the pointer crosses a control fires
    /// constantly while someone is just moving the mouse across the window.
    #[test]
    fn a_tip_waits_before_appearing() {
        let mut t = Tips::default();
        assert_eq!(t.alpha("scrub", at(0)), 0.0);
        assert_eq!(t.alpha("scrub", at(300)), 0.0, "appeared during a pass-over");
        assert!(t.alpha("scrub", at(700)) > 0.0, "never appeared");
    }

    #[test]
    fn it_fades_in_rather_than_snapping() {
        let mut t = Tips::default();
        t.alpha("scrub", at(0));
        let mid = t.alpha("scrub", at(690)); // 90ms into a 180ms fade
        assert!(mid > 0.2 && mid < 0.8, "mid-fade alpha is {mid}");
        assert_eq!(t.alpha("scrub", at(900)), 1.0);
    }

    /// Crossing three buttons must not leave three half-faded tips behind.
    #[test]
    fn moving_to_another_control_restarts_the_wait() {
        let mut t = Tips::default();
        t.alpha("scrub", at(0));
        t.alpha("scrub", at(500));
        assert_eq!(t.alpha("volume", at(500)), 0.0, "carried the wait over");
        assert_eq!(t.alpha("volume", at(900)), 0.0, "did not restart the delay");
        assert!(t.alpha("volume", at(1400)) > 0.0);
    }

    /// A hint shown three times has done its job; the fourth is nagging.
    #[test]
    fn a_learned_tip_stops_appearing() {
        let mut t = Tips::default();
        for i in 0..3 {
            let base = i * 10_000;
            t.alpha("scrub", at(base));
            t.alpha("scrub", at(base + 900));
            t.idle();
        }
        assert!(t.learned("scrub"));
        t.alpha("scrub", at(50_000));
        assert_eq!(
            t.alpha("scrub", at(51_000)),
            0.0,
            "kept nagging after it was learned"
        );
    }

    /// Counted at full opacity, not when the fade starts: a tip glimpsed at
    /// 10% while the pointer passes over has taught nobody anything, and
    /// counting it would retire the hint before it was ever read.
    #[test]
    fn a_glimpsed_tip_does_not_count_as_taught() {
        let mut t = Tips::default();
        for i in 0..5 {
            let base = i * 10_000;
            t.alpha("scrub", at(base));
            // Past the delay but only part way through the fade.
            t.alpha("scrub", at(base + 650));
            t.idle();
        }
        assert_eq!(t.seen("scrub"), 0, "part-faded tips were counted");
        assert!(!t.learned("scrub"));
    }

    /// Someone who has not opened the app in six months has not learned the
    /// gestures in any useful sense.
    #[test]
    fn hints_can_be_taught_again() {
        let mut t = Tips::default();
        for i in 0..3 {
            t.alpha("scrub", at(i * 10_000));
            t.alpha("scrub", at(i * 10_000 + 900));
            t.idle();
        }
        assert!(t.learned("scrub"));
        t.reset();
        assert!(!t.learned("scrub"));
        assert_eq!(t.seen("scrub"), 0);
    }

    /// A settled tooltip must not keep asking for frames -- the same drain as
    /// the paused-repaint and chrome-fade bugs.
    #[test]
    fn a_settled_tip_asks_for_no_repaints() {
        let mut t = Tips::default();
        t.alpha("scrub", at(0));
        t.alpha("scrub", at(900));
        assert_eq!(t.next_repaint("scrub", at(900)), None);
    }

    #[test]
    fn a_waiting_tip_asks_to_be_woken_when_it_is_due() {
        let mut t = Tips::default();
        t.alpha("scrub", at(0));
        let d = t.next_repaint("scrub", at(100)).expect("should want waking");
        assert!(d > Duration::from_millis(400) && d <= Duration::from_millis(600));
    }

    /// The hint is a glyph plus a few words; the long explanation belongs in
    /// settings, where the reader went looking for it.
    #[test]
    fn hints_are_short_and_carry_an_icon() {
        assert!(!GESTURE_TIPS.is_empty());
        for tip in GESTURE_TIPS {
            assert!(!tip.icon.is_empty(), "{} has no icon", tip.id);
            // Counting WORDS, not tokens: the separators (· and =) are
            // punctuation and inflate the count without adding anything to
            // read. Limit tightened to 5 at the same time so the check is no
            // weaker than it was -- relaxing a rule to fit my own text would
            // be fixing the test instead of the hint.
            let words = tip
                .text
                .split_whitespace()
                .filter(|w| w.chars().any(|c| c.is_alphanumeric()))
                .count();
            assert!(
                words <= 5,
                "{} is a sentence, not a hint ({words} words): {:?}",
                tip.id,
                tip.text
            );
        }
    }

    /// Duplicate ids would share a counter and retire each other early.
    #[test]
    fn hint_ids_are_unique() {
        let mut seen = Vec::new();
        for tip in GESTURE_TIPS {
            assert!(!seen.contains(&tip.id), "{} appears twice", tip.id);
            seen.push(tip.id);
        }
    }
}
