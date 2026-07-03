//! uikit-gesture — a reusable two-tier scroll/swipe gesture model.
//!
//! Promoted, unchanged in behaviour, from
//! `system/snapclone/rust/snapclone-gesture`. It is the gesture *feel* that
//! landed well on the FP5 touchpad, lifted out of snapclone's Qt widget so any
//! app — egui, Qt, Slint — can reuse it. logscope is the first reverse-port
//! consumer (its mouse-first scene gains touch navigation by feeding the same
//! model).
//!
//! Two tiers:
//!   * **Small sustained scroll → sub-page detent.** Keep nudging past the end
//!     of a scroll area and, once accumulated motion crosses a threshold, it
//!     fires ONE sub-page advance. A short post-fire lock gates re-fires so a
//!     sustained scroll doesn't rip through several.
//!   * **Big fast swipe → page jump.** A flick (enough distance AND enough
//!     velocity) or a drag past a fraction of the page commits a whole page.
//!
//! Why platform-free: the feel is pure scalar/time logic — an accumulator with
//! a wall-clock RATE budget (a 240 Hz trackpad and a 60 Hz mouse reach the
//! threshold in the same real time), an arm-window that re-arms rather than
//! dying, a fire-once-per-streak latch, a post-fire takeover lock, and a
//! distance+velocity test for the page tier. None of that needs a toolkit. The
//! app keeps the toolkit-specific bits (reading deltas, knowing whether the
//! inner view is at its limit, rendering the "barrier", actually moving) and
//! feeds this module deltas + timestamps, getting back decisions.
//!
//! Wiring (per gesture surface):
//! 1. Build a [`TwoTierGesture`] with a [`GestureConfig`] (defaults match the
//!    tuned mobile feel).
//! 2. On each at-limit scroll/drag sample call [`TwoTierGesture::on_scroll`]
//!    with signed delta + monotonic ms; act on the [`Outcome`].
//! 3. On release call [`TwoTierGesture::on_release`] with release velocity
//!    (px/ms) and total drag distance for the page-tier [`ReleaseOutcome`].
//! 4. Call [`TwoTierGesture::reset_streak`] when the inner view is NOT at its
//!    limit (normal scrolling) so a fresh pull starts clean.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod ffi;

/// Which way a detent / page move goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Toward the next page / sub-page (content moved up/left).
    Next,
    /// Toward the previous page / sub-page.
    Prev,
}

impl Direction {
    fn from_sign(v: f64) -> Direction {
        if v >= 0.0 { Direction::Next } else { Direction::Prev }
    }
    /// +1 for Next, -1 for Prev.
    pub fn sign(self) -> i32 {
        match self { Direction::Next => 1, Direction::Prev => -1 }
    }
}

/// What an input sample produced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    /// Nothing actionable this sample.
    None,
    /// The sub-page detent fired once. Advance one sub-page in `dir`.
    DetentFired { dir: Direction },
    /// Detent progress changed (0..1) — render the "almost there" barrier.
    /// `edge` is the side being pulled. No commit yet.
    DetentProgress { fraction: f64, edge: Direction },
}

/// The page-tier decision at release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseOutcome {
    /// Stay on the current page (drag too small / too slow).
    Stay,
    /// Commit a whole-page jump in `dir`.
    PageJump { dir: Direction },
}

/// Tunable thresholds. Defaults match the FP5-tuned mobile/touchpad feel.
#[derive(Debug, Clone, Copy)]
pub struct GestureConfig {
    /// Logical page width/height in px (page-jump thresholds are fractions of
    /// this, so the feel is identical on a 390 px phone and a 1440 px desktop).
    pub page_size: f64,
    /// Raw accumulated motion (px) past the limit to fire a sub-page detent.
    pub detent_threshold: f64,
    /// How long (ms) the detent stays armed before it re-arms a fresh streak.
    pub detent_arm_window_ms: f64,
    /// Max raw units per elapsed ms the accumulator will take — the refresh-rate
    /// normalizer. A burst of same-frame events can't blast it.
    pub detent_rate_per_ms: f64,
    /// After a detent fires, ignore accumulation for this long (ms).
    pub takeover_lock_ms: f64,
    /// Fraction of `page_size` a drag must cover to commit a page jump.
    pub commit_fraction: f64,
    /// Min release velocity (px/ms) to count as a flick.
    pub flick_velocity_px_ms: f64,
    /// Min drag distance (px) for a flick to count (guards micro-flicks).
    pub flick_min_px: f64,
}

impl Default for GestureConfig {
    fn default() -> Self {
        // Mirrors snapclone's tuned constants (kVDetentRawThreshold=220,
        // kDetentArmWindowMs=1000, kDetentRawPerMs=0.75, kCommitFraction=0.22,
        // kFlickThresholdPxMs=0.30, kFlickMinPx=14) == C++ include/ui/Theme.hpp.
        GestureConfig {
            page_size: 390.0,
            detent_threshold: 220.0,
            detent_arm_window_ms: 1000.0,
            detent_rate_per_ms: 0.75,
            takeover_lock_ms: 350.0,
            commit_fraction: 0.22,
            flick_velocity_px_ms: 0.30,
            flick_min_px: 14.0,
        }
    }
}

impl GestureConfig {
    /// A config for a given page size with default thresholds.
    pub fn for_page_size(page_size: f64) -> Self {
        GestureConfig { page_size, ..Default::default() }
    }
}

/// The two-tier gesture state machine. One per gesture surface. Holds the
/// detent accumulator + timing latches; the page tier is stateless (decided
/// wholly at release).
#[derive(Debug, Clone)]
pub struct TwoTierGesture {
    cfg: GestureConfig,
    raw: f64,
    armed: bool,
    armed_at_ms: f64,
    fired_this_streak: bool,
    last_feed_ms: Option<f64>,
    lock_until_ms: f64,
    last_progress: f64,
    last_edge: Direction,
}

impl TwoTierGesture {
    pub fn new(cfg: GestureConfig) -> Self {
        TwoTierGesture {
            cfg,
            raw: 0.0,
            armed: false,
            armed_at_ms: 0.0,
            fired_this_streak: false,
            last_feed_ms: None,
            lock_until_ms: 0.0,
            last_progress: 0.0,
            last_edge: Direction::Next,
        }
    }

    pub fn config(&self) -> &GestureConfig { &self.cfg }
    /// Mutable config — used by snapclone's Settings ▸ Gestures live retune.
    pub fn config_mut(&mut self) -> &mut GestureConfig { &mut self.cfg }
    /// Current accumulated raw motion (signed). For tests/diagnostics/HUD.
    pub fn raw(&self) -> f64 { self.raw }
    /// True if the takeover lock is currently holding (post-fire cooldown).
    pub fn locked(&self, now_ms: f64) -> bool { now_ms < self.lock_until_ms }
    /// True while a detent streak is armed (for HUD LEDs).
    pub fn armed(&self) -> bool { self.armed }

    /// Feed one at-the-limit scroll/drag sample. `delta` is signed motion past
    /// the limit (+ toward Next, - toward Prev); `now_ms` is monotonic. Call
    /// ONLY when the inner view is at its scroll limit; call [`reset_streak`]
    /// otherwise.
    pub fn on_scroll(&mut self, delta: f64, now_ms: f64) -> Outcome {
        // While the post-fire lock holds, swallow input and keep the streak
        // zeroed so the first post-lock sample starts clean.
        if self.locked(now_ms) {
            self.raw = 0.0;
            self.last_feed_ms = Some(now_ms);
            return Outcome::None;
        }

        // Refresh-rate normalization: cap this sample to a wall-clock budget.
        let dt = match self.last_feed_ms {
            Some(prev) => (now_ms - prev).max(1.0),
            None => 16.0, // assume one 60 Hz frame on first feed
        };
        self.last_feed_ms = Some(now_ms);
        let budget = self.cfg.detent_rate_per_ms * dt;
        let contrib = if delta.abs() > budget { budget.copysign(delta) } else { delta };
        self.raw += contrib;

        // Arm / re-arm.
        if !self.armed {
            self.armed = true;
            self.armed_at_ms = now_ms;
            self.fired_this_streak = false;
        } else if now_ms - self.armed_at_ms > self.cfg.detent_arm_window_ms {
            self.armed = true;
            self.armed_at_ms = now_ms;
            self.fired_this_streak = false;
            self.raw = contrib; // keep just this sample's contribution
        }

        // Fire once per streak when the threshold is crossed.
        if !self.fired_this_streak && self.raw.abs() >= self.cfg.detent_threshold {
            self.fired_this_streak = true;
            let dir = Direction::from_sign(self.raw);
            self.lock_until_ms = now_ms + self.cfg.takeover_lock_ms;
            self.raw = 0.0;
            self.last_progress = 0.0;
            return Outcome::DetentFired { dir };
        }

        // Otherwise report progress toward the threshold (for the barrier UI).
        let fraction = (self.raw.abs() / self.cfg.detent_threshold).clamp(0.0, 1.0);
        let edge = Direction::from_sign(self.raw);
        if (fraction - self.last_progress).abs() > f64::EPSILON || edge != self.last_edge {
            self.last_progress = fraction;
            self.last_edge = edge;
            return Outcome::DetentProgress { fraction, edge };
        }
        Outcome::None
    }

    /// Call when the inner view is NOT at its limit (normal scrolling).
    pub fn reset_streak(&mut self) {
        self.raw = 0.0;
        self.armed = false;
        self.fired_this_streak = false;
        self.last_progress = 0.0;
        // Deliberately keep last_feed_ms / lock_until_ms: both are time-based.
    }

    /// Page-tier decision at release. `velocity` is px/ms at lift (signed; +
    /// toward Next), `drag_distance` total signed px. A flick (fast AND far) or
    /// a drag past commit_fraction commits a page jump.
    pub fn on_release(&mut self, velocity: f64, drag_distance: f64) -> ReleaseOutcome {
        let abs_vel = velocity.abs();
        let abs_drag = drag_distance.abs();
        let commit_px = self.cfg.page_size * self.cfg.commit_fraction;

        let is_flick = abs_vel >= self.cfg.flick_velocity_px_ms && abs_drag >= self.cfg.flick_min_px;
        let is_drag_commit = abs_drag >= commit_px;

        if is_flick || is_drag_commit {
            let dir = if is_flick {
                Direction::from_sign(velocity)
            } else {
                Direction::from_sign(drag_distance)
            };
            ReleaseOutcome::PageJump { dir }
        } else {
            ReleaseOutcome::Stay
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> GestureConfig { GestureConfig::default() }

    #[test]
    fn detent_fires_once_then_locks() {
        let mut g = TwoTierGesture::new(cfg());
        // Feed enough motion across frames to cross 220 (rate cap 0.75/ms).
        // 30px every 50ms => budget 37.5, so 30 is taken whole; needs ~8 samples.
        let mut fired = 0;
        let mut t = 0.0;
        for _ in 0..12 {
            if let Outcome::DetentFired { dir } = g.on_scroll(30.0, t) {
                assert_eq!(dir, Direction::Next);
                fired += 1;
            }
            t += 50.0;
        }
        assert_eq!(fired, 1, "detent must fire exactly once per streak");
        // Immediately after, the lock holds and swallows input.
        assert!(g.locked(t - 50.0 + 1.0));
    }

    #[test]
    fn rate_budget_normalizes_burst_vs_trickle() {
        // A 240Hz burst (many tiny same-interval samples) and a 60Hz trickle of
        // the SAME physical motion should reach threshold in ~the same wall time.
        let reach = |dt: f64, per_frame: f64| {
            let mut g = TwoTierGesture::new(cfg());
            let mut t = 0.0;
            loop {
                if let Outcome::DetentFired { .. } = g.on_scroll(per_frame, t) {
                    return t;
                }
                t += dt;
                if t > 100_000.0 { return t; }
            }
        };
        // Same speed (px/ms) delivered at 4ms vs 16ms frames.
        let fast = reach(4.0, 0.75 * 4.0 * 4.0); // oversized: gets capped to budget
        let slow = reach(16.0, 0.75 * 16.0 * 4.0);
        // Both are budget-limited, so wall-clock-to-threshold is close.
        let ratio = fast / slow;
        assert!((0.5..2.0).contains(&ratio), "burst/trickle wall time diverged: {ratio}");
    }

    #[test]
    fn progress_is_reported_before_fire() {
        let mut g = TwoTierGesture::new(cfg());
        let o = g.on_scroll(20.0, 0.0);
        match o {
            Outcome::DetentProgress { fraction, edge } => {
                assert!(fraction > 0.0 && fraction < 1.0);
                assert_eq!(edge, Direction::Next);
            }
            other => panic!("expected progress, got {other:?}"),
        }
    }

    #[test]
    fn reset_streak_clears_accumulation() {
        let mut g = TwoTierGesture::new(cfg());
        g.on_scroll(100.0, 0.0);
        assert!(g.raw().abs() > 0.0);
        g.reset_streak();
        assert_eq!(g.raw(), 0.0);
        assert!(!g.armed());
    }

    #[test]
    fn flick_commits_page_by_velocity() {
        let mut g = TwoTierGesture::new(cfg());
        // Fast (>=0.30 px/ms) and far enough (>=14px) toward Prev.
        assert_eq!(g.on_release(-0.5, -20.0), ReleaseOutcome::PageJump { dir: Direction::Prev });
    }

    #[test]
    fn slow_short_drag_stays() {
        let mut g = TwoTierGesture::new(cfg());
        // Slow and short: below both flick and commit thresholds.
        assert_eq!(g.on_release(0.05, 10.0), ReleaseOutcome::Stay);
    }

    #[test]
    fn slow_long_drag_commits_by_distance() {
        let mut g = TwoTierGesture::new(cfg());
        // Velocity ~0 but dragged past 22% of 390 (= 85.8px).
        assert_eq!(g.on_release(0.0, 90.0), ReleaseOutcome::PageJump { dir: Direction::Next });
    }

    #[test]
    fn micro_flick_is_guarded() {
        let mut g = TwoTierGesture::new(cfg());
        // Fast but only 5px — below flick_min_px and below commit: no jump.
        assert_eq!(g.on_release(0.9, 5.0), ReleaseOutcome::Stay);
    }
}
