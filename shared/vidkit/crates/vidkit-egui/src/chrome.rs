//! Chrome that gets out of the way.
//!
//! While a video is rolling the controls fade out and stop taking space; a tap
//! brings them back. Pure timing and opacity, so the rules below are tested
//! without laying out a frame.
//!
//! # The tap that reveals is not the tap that acts
//!
//! When the chrome is hidden, the first tap brings it back and does nothing
//! else. Otherwise the gesture that means "show me the controls" also pauses
//! the video, and the viewer has to undo something they never asked for. Every
//! phone video player works this way and it is the one rule people notice when
//! it is missing.
//!
//! # It never hides when hiding would be wrong
//!
//! Paused means the viewer is looking at the controls on purpose. A modal open
//! means they are using one. Neither is a moment to take the UI away, and both
//! are easy to forget — a timer that only knows about elapsed time hides the
//! transport out from under a paused player.

use std::time::{Duration, Instant};

#[derive(Copy, Clone, Debug)]
pub struct ChromePolicy {
    /// Idle time before the chrome starts fading, while playing.
    pub hide_after: Duration,
    /// How long the fade out takes.
    pub fade_out: Duration,
    /// How long the fade in takes.
    ///
    /// Much shorter than the fade out on purpose: appearing is a response to
    /// something the viewer just did and should feel immediate, while
    /// disappearing is the UI's own idea and should be gentle enough not to
    /// catch the eye.
    pub fade_in: Duration,
}

impl Default for ChromePolicy {
    fn default() -> Self {
        Self {
            hide_after: Duration::from_secs(3),
            fade_out: Duration::from_millis(450),
            fade_in: Duration::from_millis(120),
        }
    }
}

/// Tracks how visible the chrome should be.
pub struct Chrome {
    pub policy: ChromePolicy,
    /// When the chrome was last woken.
    since: Option<Instant>,
    /// Opacity at the moment of the last state change, so a fade interrupted
    /// halfway resumes from where it was rather than snapping.
    from: f32,
    /// What it is heading towards.
    target: f32,
    current: f32,
}

impl Default for Chrome {
    fn default() -> Self {
        Self::new(ChromePolicy::default())
    }
}

impl Chrome {
    pub fn new(policy: ChromePolicy) -> Self {
        Self {
            policy,
            since: None,
            from: 1.0,
            target: 1.0,
            current: 1.0,
        }
    }

    /// Current opacity, 0..1.
    pub fn alpha(&self) -> f32 {
        self.current
    }

    /// Is the chrome interactive? Below this it is too faint to aim at, and a
    /// control that is visible but not clickable is worse than one that is
    /// gone.
    pub fn interactive(&self) -> bool {
        self.current > 0.35
    }

    /// Fully out of the way, so the caller can skip laying it out at all.
    pub fn hidden(&self) -> bool {
        self.current <= 0.001
    }

    /// The viewer did something. Brings the chrome back and restarts the timer.
    pub fn wake(&mut self, now: Instant) {
        self.from = self.current;
        self.target = 1.0;
        self.since = Some(now);
    }

    /// Should this tap be consumed by revealing the chrome?
    ///
    /// True only when the chrome is not interactive — so a tap on a visible
    /// control does what the control says, and a tap on a video with the
    /// chrome hidden brings it back instead of pausing.
    pub fn tap_reveals(&self) -> bool {
        !self.interactive()
    }

    /// Advance. `holding` is true whenever the chrome must stay up regardless
    /// of the timer: paused, a modal open, a drag in progress.
    pub fn update(&mut self, now: Instant, playing: bool, holding: bool) {
        if holding || !playing {
            // Not a `wake`: that would restart the fade from wherever it had
            // got to and make the chrome ramp back up every frame. Just hold
            // the target and let the interpolation below finish the job.
            if self.target != 1.0 {
                self.from = self.current;
                self.since = Some(now);
            }
            self.target = 1.0;
        } else if let Some(since) = self.since {
            if self.target != 0.0 && now.saturating_duration_since(since) >= self.policy.hide_after {
                self.from = self.current;
                self.target = 0.0;
                self.since = Some(now);
            }
        } else {
            self.since = Some(now);
        }

        let span = if self.target > self.from {
            self.policy.fade_in
        } else {
            self.policy.fade_out
        };
        let elapsed = self.since.map(|s| now.saturating_duration_since(s)).unwrap_or_default();

        // The fade only starts once the hold has expired, so while waiting the
        // opacity must not drift.
        let t = if self.target == 0.0 {
            let after_hold = elapsed;
            if span.is_zero() {
                1.0
            } else {
                (after_hold.as_secs_f32() / span.as_secs_f32()).clamp(0.0, 1.0)
            }
        } else if span.is_zero() {
            1.0
        } else {
            (elapsed.as_secs_f32() / span.as_secs_f32()).clamp(0.0, 1.0)
        };

        self.current = self.from + (self.target - self.from) * t;
        self.current = self.current.clamp(0.0, 1.0);
    }

    /// When the caller should repaint next to keep the fade smooth, or `None`
    /// when nothing is animating.
    ///
    /// Returning `None` when settled is what stops the fade costing battery: a
    /// player showing static chrome, or none, has no reason to redraw.
    pub fn next_repaint(&self) -> Option<Duration> {
        if (self.current - self.target).abs() > 0.001 {
            // Mid-fade: a frame at a time.
            return Some(Duration::from_millis(16));
        }
        if self.target == 1.0 && self.since.is_some() {
            // Settled visible, waiting to hide.
            return Some(self.policy.hide_after);
        }
        None
    }
}

/// Whether pointer motion is the viewer meaning it, or a bump.
///
/// A mouse on a couch cushion or a trackpad brushed by a sleeve moves a few
/// pixels, and every one of those used to bring the controls back over the
/// film. Motion counts only when the pointer travels more than `px` within
/// `window` -- a deliberate move covers that in a blink; jitter never does,
/// and slow drift over minutes never does either (a plain anchor would
/// accumulate drift until it woke anyway). Buttons, scrolling and keys
/// always count. `px = 0` restores "any motion".
#[derive(Clone, Debug)]
pub struct PointerDeadzone {
    pub px: f32,
    pub window: Duration,
    recent: std::collections::VecDeque<(Instant, [f32; 2])>,
}

impl PointerDeadzone {
    pub fn new(px: f32) -> Self {
        Self { px, window: Duration::from_millis(300), recent: Default::default() }
    }

    /// One frame's pointer position; `true` when it is a deliberate move.
    pub fn moved(&mut self, now: Instant, pos: Option<[f32; 2]>) -> bool {
        let Some(p) = pos else {
            self.recent.clear();
            return false;
        };
        while self.recent.front().is_some_and(|(t, _)| now.duration_since(*t) > self.window) {
            self.recent.pop_front();
        }
        let moved = if self.px <= 0.0 {
            // Any motion since the last frame.
            self.recent.back().is_some_and(|(_, q)| *q != p)
        } else {
            self.recent.iter().any(|(_, q)| {
                let (dx, dy) = (p[0] - q[0], p[1] - q[1]);
                (dx * dx + dy * dy).sqrt() > self.px
            })
        };
        self.recent.push_back((now, p));
        moved
    }
}

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

    fn playing(c: &mut Chrome, ms: u64) {
        c.update(at(ms), true, false);
    }

    /// Run past the hold AND the fade. The fade only STARTS at the moment the
    /// hold expires, so a single update at that instant leaves alpha at 1.0 --
    /// three tests asserted `hidden()` there and were asserting against a fade
    /// that had not begun.
    fn fade_out_fully(c: &mut Chrome, from_ms: u64) -> u64 {
        let start = from_ms + 3000;
        c.update(at(start), true, false);
        let done = start + 500;
        c.update(at(done), true, false);
        done
    }

    #[test]
    fn chrome_starts_visible() {
        let c = Chrome::default();
        assert_eq!(c.alpha(), 1.0);
        assert!(c.interactive());
    }

    #[test]
    fn it_fades_out_after_the_idle_time() {
        let mut c = Chrome::default();
        c.wake(at(0));
        playing(&mut c, 1000);
        assert_eq!(c.alpha(), 1.0, "faded before the hold expired");

        playing(&mut c, 3000); // hold expires, fade begins
        playing(&mut c, 3225); // half the 450ms fade
        assert!(
            c.alpha() > 0.3 && c.alpha() < 0.7,
            "mid-fade alpha is {}",
            c.alpha()
        );

        playing(&mut c, 3500);
        assert!(c.hidden(), "still visible at {}", c.alpha());
    }

    /// Paused means the viewer is looking at the controls on purpose.
    #[test]
    fn it_never_hides_while_paused() {
        let mut c = Chrome::default();
        c.wake(at(0));
        for ms in [1000, 5000, 60_000] {
            c.update(at(ms), false, false);
        }
        assert_eq!(c.alpha(), 1.0, "hid the transport under a paused player");
    }

    /// A modal open means they are using one.
    #[test]
    fn it_never_hides_while_something_is_holding_it() {
        let mut c = Chrome::default();
        c.wake(at(0));
        for ms in [1000, 5000, 60_000] {
            c.update(at(ms), true, true);
        }
        assert_eq!(c.alpha(), 1.0, "hid the UI with a modal open");
    }

    #[test]
    fn waking_brings_it_back() {
        let mut c = Chrome::default();
        c.wake(at(0));
        let t = fade_out_fully(&mut c, 0);
        assert!(c.hidden(), "alpha {}", c.alpha());

        c.wake(at(t));
        playing(&mut c, t + 60); // half the 120ms fade in
        assert!(c.alpha() > 0.3, "fade in stalled at {}", c.alpha());
        playing(&mut c, t + 200);
        assert_eq!(c.alpha(), 1.0);
    }

    /// Appearing responds to something the viewer just did and must feel
    /// immediate; disappearing is the UI's own idea and should not catch the
    /// eye.
    #[test]
    fn it_appears_faster_than_it_disappears() {
        let p = ChromePolicy::default();
        assert!(
            p.fade_in < p.fade_out,
            "fade in {:?} is not quicker than fade out {:?}",
            p.fade_in,
            p.fade_out
        );
    }

    /// The rule people notice when it is missing: the tap that reveals is not
    /// the tap that acts.
    #[test]
    fn the_first_tap_reveals_and_does_not_act() {
        let mut c = Chrome::default();
        assert!(!c.tap_reveals(), "a visible control should act, not reveal");

        c.wake(at(0));
        let t = fade_out_fully(&mut c, 0);
        assert!(c.hidden());
        assert!(c.tap_reveals(), "a tap on hidden chrome must only reveal");

        c.wake(at(t));
        playing(&mut c, t + 200);
        assert!(!c.tap_reveals(), "once back, taps should act again");
    }

    /// A control visible but too faint to aim at is worse than one that is
    /// gone, so interactivity cuts off well above zero.
    #[test]
    fn faint_chrome_is_not_interactive() {
        let mut c = Chrome::default();
        c.wake(at(0));
        playing(&mut c, 3000);
        playing(&mut c, 3400); // nearly faded
        assert!(c.alpha() < 0.35, "alpha {} for this case", c.alpha());
        assert!(!c.interactive());
    }

    /// A fade interrupted halfway must resume from where it was, not snap to
    /// full and jump.
    #[test]
    fn an_interrupted_fade_resumes_from_where_it_was() {
        let mut c = Chrome::default();
        c.wake(at(0));
        playing(&mut c, 3000);
        playing(&mut c, 3225);
        let mid = c.alpha();
        assert!(mid > 0.3 && mid < 0.7);

        c.wake(at(3225));
        c.update(at(3226), true, false);
        assert!(
            (c.alpha() - mid).abs() < 0.1,
            "snapped from {mid} to {}",
            c.alpha()
        );
    }

    /// Settled chrome must stop asking for repaints, or the fade costs battery
    /// forever -- the same drain as the paused-repaint bug.
    #[test]
    fn settled_hidden_chrome_asks_for_no_repaints() {
        let mut c = Chrome::default();
        c.wake(at(0));
        fade_out_fully(&mut c, 0);
        assert!(c.hidden());
        assert_eq!(c.next_repaint(), None, "hidden chrome kept waking the GPU");
    }

    #[test]
    fn a_fade_in_progress_asks_for_frames() {
        let mut c = Chrome::default();
        c.wake(at(0));
        playing(&mut c, 3000);
        playing(&mut c, 3100);
        assert!(c.next_repaint().is_some(), "a fade needs repaints to animate");
    }

    #[test]
    fn jitter_and_drift_do_not_wake_but_a_move_does() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut d = PointerDeadzone::new(6.0);
        // A couch bump: +-3 px at 60 Hz for a second.
        for k in 0..60u64 {
            let j = if k % 2 == 0 { 3.0 } else { -3.0 };
            assert!(!d.moved(ms(k * 16), Some([100.0 + j, 100.0])), "jitter woke at {k}");
        }
        // Slow drift: 40 px over 10 s (2 px per 500 ms).
        let mut d = PointerDeadzone::new(6.0);
        for k in 0..20u64 {
            assert!(!d.moved(ms(k * 500), Some([100.0 + 2.0 * k as f32, 100.0])), "drift woke at {k}");
        }
        // A deliberate flick: 30 px in 100 ms.
        let mut d = PointerDeadzone::new(6.0);
        let mut woke = false;
        for k in 0..6u64 {
            woke |= d.moved(ms(k * 16), Some([100.0 + 6.0 * k as f32, 100.0]));
        }
        assert!(woke);
        // Zero: any motion, as before.
        let mut d = PointerDeadzone::new(0.0);
        assert!(!d.moved(ms(0), Some([1.0, 1.0])));
        assert!(d.moved(ms(16), Some([2.0, 1.0])));
        assert!(!d.moved(ms(32), Some([2.0, 1.0])), "no motion, no wake");
    }
}
