//! snapclone-gesture — a reusable two-tier scroll/swipe gesture model.
//!
//! This is the gesture *feel* that landed well on the touchpad, lifted out of
//! the app's Qt widget so any app can reuse it:
//!
//!   * **Small sustained scroll → sub-page detent.** Keep nudging past the end
//!     of a scroll area and, once your accumulated motion crosses a threshold,
//!     it fires ONE sub-page advance. Sustained motion within the same streak
//!     does not rip through several sub-pages — a short lock gates re-fires.
//!   * **Big fast swipe → page jump.** A flick (enough distance AND enough
//!     velocity) commits a whole-page move.
//!
//! # Why a platform-free module
//!
//! The thing that makes this feel good is pure scalar/time logic: an
//! accumulator with a wall-clock RATE budget (so a 240Hz trackpad and a 60Hz
//! mouse reach the threshold in the same real time), an arm-window that
//! re-arms rather than dying, a fire-once-per-streak latch, a post-fire
//! takeover lock that stops runaway, and a distance+velocity test for the
//! page tier. None of that needs a UI toolkit. The app keeps the toolkit-
//! specific bits — reading wheel/touch deltas, knowing whether the inner view
//! is scrolled to its limit, rendering the progress "barrier", actually moving
//! to the next page/sub-page — and feeds this module deltas + timestamps,
//! getting back decisions.
//!
//! # Wiring (per gesture surface)
//!
//! 1. Build a [`TwoTierGesture`] with a [`GestureConfig`] (page size +
//!    thresholds; defaults match the tuned mobile feel).
//! 2. On each scroll/drag sample where the inner view is AT ITS LIMIT (the app
//!    decides this — it owns the scrollbars), call
//!    [`TwoTierGesture::on_scroll`] with the signed delta and a monotonic
//!    timestamp in ms. Act on the returned [`Outcome`].
//! 3. On gesture release (finger/trackpad lift), call
//!    [`TwoTierGesture::on_release`] with the release velocity (px/ms) and the
//!    total drag distance to get the page-tier decision.
//! 4. Call [`TwoTierGesture::reset_streak`] when the inner view is NOT at its
//!    limit (normal scrolling), so a fresh pull starts clean.
//!
//! Pure logic, no deps, fully unit-tested; a C FFI seam lets non-Rust apps use
//! it too.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod ffi;

/// Which way a detent / page move goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Toward the next page / sub-page (conventionally: content moved up/left).
    Next,
    /// Toward the previous page / sub-page.
    Prev,
}

impl Direction {
    fn from_sign(v: f64) -> Direction {
        if v >= 0.0 {
            Direction::Next
        } else {
            Direction::Prev
        }
    }
    /// +1 for Next, -1 for Prev.
    pub fn sign(self) -> i32 {
        match self {
            Direction::Next => 1,
            Direction::Prev => -1,
        }
    }
}

/// What an input sample produced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    /// Nothing actionable this sample.
    None,
    /// The sub-page detent fired once. The app advances one sub-page in `dir`.
    DetentFired { dir: Direction },
    /// Detent progress changed (0..1) — render the "almost there" barrier. `edge`
    /// is the side being pulled (Next/Prev). No commit yet.
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

/// Tunable thresholds. Defaults match the tuned mobile/touchpad feel.
#[derive(Debug, Clone, Copy)]
pub struct GestureConfig {
    /// Logical page width/height in px (page-jump thresholds are fractions of
    /// this, so the feel is identical on a 390px phone and a 1440px desktop).
    pub page_size: f64,
    /// Raw accumulated motion (px) past the limit to fire a sub-page detent.
    pub detent_threshold: f64,
    /// How long (ms) the detent stays armed after the limit is first hit before
    /// it re-arms a fresh streak.
    pub detent_arm_window_ms: f64,
    /// Max raw units per elapsed millisecond the accumulator will take — the
    /// refresh-rate normalizer. A burst of same-frame events can't blast it.
    pub detent_rate_per_ms: f64,
    /// After a detent fires, ignore further accumulation for this long (ms) so
    /// one sustained scroll doesn't rip through several sub-pages.
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
        // These mirror the app's tuned constants (kVDetentRawThreshold=220,
        // kDetentArmWindowMs=1000, kDetentRawPerMs=0.75, kCommitFraction=0.22,
        // kFlickThresholdPxMs=0.30, kFlickMinPx=14), with a takeover lock in the
        // same range as the app's scrollTakeoverLockMs.
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
    /// Convenience: a config for a given page size with default thresholds.
    pub fn for_page_size(page_size: f64) -> Self {
        GestureConfig {
            page_size,
            ..Default::default()
        }
    }
}

/// The two-tier gesture state machine. One per gesture surface (e.g. one per
/// page/scroll area). Holds the detent accumulator + timing latches; the page
/// tier is stateless (decided wholly at release).
#[derive(Debug, Clone)]
pub struct TwoTierGesture {
    cfg: GestureConfig,
    // Detent accumulator (signed: + = past Next edge, - = past Prev edge).
    raw: f64,
    armed: bool,
    armed_at_ms: f64,
    fired_this_streak: bool,
    last_feed_ms: Option<f64>,
    // Takeover lock: absolute ms timestamp until which feeds are ignored.
    lock_until_ms: f64,
    // Last reported progress, to suppress duplicate DetentProgress.
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

    pub fn config(&self) -> &GestureConfig {
        &self.cfg
    }

    /// Current accumulated raw motion (signed). Mostly for tests/diagnostics.
    pub fn raw(&self) -> f64 {
        self.raw
    }

    /// True if the takeover lock is currently holding (post-fire cooldown).
    pub fn locked(&self, now_ms: f64) -> bool {
        now_ms < self.lock_until_ms
    }

    /// Feed one at-the-limit scroll/drag sample. `delta` is signed motion past
    /// the limit (+ toward Next, - toward Prev); `now_ms` is a monotonic
    /// timestamp. Returns what to do. Call ONLY when the inner view is at its
    /// scroll limit (the app owns that check); call [`reset_streak`] otherwise.
    pub fn on_scroll(&mut self, delta: f64, now_ms: f64) -> Outcome {
        // While the post-fire lock holds, swallow input and keep the streak
        // zeroed so the first post-lock sample starts clean (no inherited
        // built-up value re-firing the instant the lock lifts).
        if self.locked(now_ms) {
            self.raw = 0.0;
            self.last_feed_ms = Some(now_ms);
            return Outcome::None;
        }

        // Refresh-rate normalization: cap this sample's contribution to a
        // wall-clock budget so a 240Hz burst and a 60Hz trickle of the same
        // physical motion reach the threshold in the same real time.
        let dt = match self.last_feed_ms {
            Some(prev) => (now_ms - prev).max(1.0),
            None => 16.0, // assume one 60Hz frame on first feed
        };
        self.last_feed_ms = Some(now_ms);
        let budget = self.cfg.detent_rate_per_ms * dt;
        let contrib = if delta.abs() > budget {
            budget.copysign(delta)
        } else {
            delta
        };
        self.raw += contrib;

        // Arm / re-arm.
        if !self.armed {
            self.armed = true;
            self.armed_at_ms = now_ms;
            self.fired_this_streak = false;
        } else if now_ms - self.armed_at_ms > self.cfg.detent_arm_window_ms {
            // Window lapsed without crossing — re-arm a fresh streak and drop
            // stale progress so a renewed sustained pull starts clean.
            self.armed = true;
            self.armed_at_ms = now_ms;
            self.fired_this_streak = false;
            self.raw = contrib; // keep just this sample's contribution
        }

        // Fire once per streak when the threshold is crossed.
        if !self.fired_this_streak && self.raw.abs() >= self.cfg.detent_threshold {
            self.fired_this_streak = true;
            let dir = Direction::from_sign(self.raw);
            // Start the post-fire lock and consume the accumulation.
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

    /// Call when the inner view is NOT at its limit (normal scrolling), so the
    /// next time the limit is hit a fresh detent streak begins.
    pub fn reset_streak(&mut self) {
        self.raw = 0.0;
        self.armed = false;
        self.fired_this_streak = false;
        self.last_progress = 0.0;
        // Note: we deliberately do NOT clear last_feed_ms or lock_until_ms; the
        // rate budget and post-fire lock are time-based and should persist.
    }

    /// Page-tier decision at gesture release. `velocity` is px/ms at lift
    /// (signed: + toward Next), `drag_distance` is total signed px moved.
    /// A flick (fast enough AND far enough) or a drag past commit_fraction
    /// commits a page jump.
    pub fn on_release(&mut self, velocity: f64, drag_distance: f64) -> ReleaseOutcome {
        let abs_vel = velocity.abs();
        let abs_drag = drag_distance.abs();
        let commit_px = self.cfg.page_size * self.cfg.commit_fraction;

        let is_flick = abs_vel >= self.cfg.flick_velocity_px_ms
            && abs_drag >= self.cfg.flick_min_px;
        let is_drag_commit = abs_drag >= commit_px;

        if is_flick || is_drag_commit {
            // A flick's direction comes from velocity; a slow drag-commit from
            // the drag distance (velocity may be ~0 at a slow release).
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

    fn cfg() -> GestureConfig {
        GestureConfig::default()
    }

    #[test]
    fn small_sustained_scroll_fires_one_detent() {
        let mut g = TwoTierGesture::new(cfg());
        // Feed small steps past the limit at ~one 16ms frame apart. Budget per
        // 16ms frame = 0.75*16 = 12px, so steps of 12px accumulate cleanly.
        let mut fired = 0;
        let mut t = 0.0;
        for _ in 0..40 {
            if let Outcome::DetentFired { dir } = g.on_scroll(12.0, t) {
                assert_eq!(dir, Direction::Next);
                fired += 1;
            }
            t += 16.0;
        }
        assert_eq!(fired, 1, "exactly one detent across a sustained scroll");
    }

    #[test]
    fn one_sustained_scroll_does_not_rip_through_subpages() {
        // The runaway the lock prevents: a flurry of large same-streak deltas.
        let mut g = TwoTierGesture::new(cfg());
        let mut fired = 0;
        let mut t = 0.0;
        for _ in 0..200 {
            if matches!(g.on_scroll(50.0, t), Outcome::DetentFired { .. }) {
                fired += 1;
            }
            t += 2.0; // 2ms apart — high-rate burst
        }
        // Without rate-limiting + lock this would fire many times; with them,
        // far fewer (lock is 350ms, burst spans 400ms → at most ~2).
        assert!(fired <= 2, "lock + rate budget cap runaway fires, got {fired}");
        assert!(fired >= 1);
    }

    #[test]
    fn refresh_rate_normalization_240hz_vs_60hz_match() {
        // Same physical motion (the same px/ms), delivered at 240Hz vs 60Hz,
        // should fire at about the same WALL-CLOCK time.
        let fire_time = |frame_ms: f64, px_per_frame: f64| -> f64 {
            let mut g = TwoTierGesture::new(cfg());
            let mut t = 0.0;
            loop {
                if matches!(g.on_scroll(px_per_frame, t), Outcome::DetentFired { .. }) {
                    return t;
                }
                t += frame_ms;
                if t > 5000.0 {
                    return t;
                }
            }
        };
        // 60Hz: 16ms frames, 12px each (0.75 px/ms). 240Hz: 4ms frames, 3px each
        // (also 0.75 px/ms). Both are at the budget rate.
        let t60 = fire_time(16.0, 12.0);
        let t240 = fire_time(4.0, 3.0);
        // Within a couple frames of each other (not 4× apart like the old bug).
        assert!((t60 - t240).abs() <= 40.0, "60Hz fired @{t60}, 240Hz @{t240}");
    }

    #[test]
    fn direction_follows_sign() {
        let mut g = TwoTierGesture::new(cfg());
        let mut t = 0.0;
        let mut last = Outcome::None;
        for _ in 0..40 {
            let o = g.on_scroll(-12.0, t); // negative = Prev
            if matches!(o, Outcome::DetentFired { .. }) {
                last = o;
                break;
            }
            t += 16.0;
        }
        assert_eq!(last, Outcome::DetentFired { dir: Direction::Prev });
    }

    #[test]
    fn arm_window_lapse_rearms_fresh() {
        let mut g = TwoTierGesture::new(cfg());
        // Accumulate partway, then a long gap (> arm window) before resuming.
        g.on_scroll(12.0, 0.0);
        g.on_scroll(12.0, 16.0); // raw ~24
        // Big time gap: next feed is > 1000ms later → re-arm, raw resets to the
        // new sample only. So it should take a full fresh streak to fire.
        let o = g.on_scroll(12.0, 2000.0);
        assert!(matches!(o, Outcome::DetentProgress { .. } | Outcome::None));
        assert!(g.raw().abs() < cfg().detent_threshold, "streak restarted, not carried");
    }

    #[test]
    fn progress_is_reported_and_monotonic_until_fire() {
        let mut g = TwoTierGesture::new(cfg());
        let mut last_frac = 0.0;
        let mut t = 0.0;
        for _ in 0..18 {
            match g.on_scroll(12.0, t) {
                Outcome::DetentProgress { fraction, edge } => {
                    assert_eq!(edge, Direction::Next);
                    assert!(fraction >= last_frac - 1e-9);
                    last_frac = fraction;
                }
                Outcome::DetentFired { .. } => break,
                Outcome::None => {}
            }
            t += 16.0;
        }
        assert!(last_frac > 0.0, "saw increasing progress before firing");
    }

    #[test]
    fn big_fast_swipe_is_a_page_jump() {
        let mut g = TwoTierGesture::new(cfg());
        // Fast flick: 0.5 px/ms, 30px drag.
        assert_eq!(
            g.on_release(0.5, 30.0),
            ReleaseOutcome::PageJump { dir: Direction::Next }
        );
        // Backward flick.
        assert_eq!(
            g.on_release(-0.5, -30.0),
            ReleaseOutcome::PageJump { dir: Direction::Prev }
        );
    }

    #[test]
    fn slow_long_drag_commits_by_distance() {
        let mut g = TwoTierGesture::new(cfg());
        // Slow release (vel ~0) but dragged well past commit fraction (0.22*390
        // = 85.8px). 120px drag commits even with no flick velocity.
        assert_eq!(
            g.on_release(0.01, 120.0),
            ReleaseOutcome::PageJump { dir: Direction::Next }
        );
    }

    #[test]
    fn tiny_drag_stays() {
        let mut g = TwoTierGesture::new(cfg());
        // Below both the flick distance and the commit fraction → stay.
        assert_eq!(g.on_release(0.05, 8.0), ReleaseOutcome::Stay);
        // Fast but too short (micro-flick guard).
        assert_eq!(g.on_release(0.6, 6.0), ReleaseOutcome::Stay);
    }

    #[test]
    fn commit_fraction_scales_with_page_size() {
        // Same 100px drag is a commit on a small page, a stay on a huge one.
        let mut small = TwoTierGesture::new(GestureConfig::for_page_size(390.0));
        let mut big = TwoTierGesture::new(GestureConfig::for_page_size(1440.0));
        assert_eq!(
            small.on_release(0.0, 100.0),
            ReleaseOutcome::PageJump { dir: Direction::Next }
        );
        // 1440*0.22 = 316.8px; 100px is well under → stay.
        assert_eq!(big.on_release(0.0, 100.0), ReleaseOutcome::Stay);
    }

    #[test]
    fn reset_streak_clears_accumulation() {
        let mut g = TwoTierGesture::new(cfg());
        g.on_scroll(12.0, 0.0);
        g.on_scroll(12.0, 16.0);
        assert!(g.raw().abs() > 0.0);
        g.reset_streak();
        assert_eq!(g.raw(), 0.0);
    }
}
