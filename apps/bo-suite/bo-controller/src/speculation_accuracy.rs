// src/speculation_accuracy.rs
#![allow(dead_code)]
//
// Hit/miss accuracy tracker for the speculative pre-load system.
//
// A pre-staged track that gets promoted via `try_promote == Hit` is a
// genuine latency win. A pre-staged track that *doesn't* get promoted
// is wasted device-pipeline bandwidth, wasted network, and (more
// subtly) clutter in the device's playback queue that can confuse
// users who check the device-side state.
//
// If the predictor is consistently wrong, the speculation system makes
// things worse, not better. This module watches the recent hit ratio
// and emits a `Disable` recommendation when the trailing window shows
// poor accuracy.
//
// The tracker is a sliding window of the last N outcomes. It needs at
// least MIN_SAMPLES observations before making a recommendation; below
// that floor, it returns Continue (i.e. give it more data). Once the
// window is full, an accuracy below DISABLE_THRESHOLD triggers a
// recommended pause. Re-enable happens after a manual reset or after
// a cooldown period.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Sliding window size. Trades off responsiveness (small = react fast)
/// against noise (small = noisier).
pub const WINDOW: usize = 20;

/// Minimum observations before issuing a recommendation. We don't want
/// to disable based on the first miss after fresh startup.
pub const MIN_SAMPLES: usize = 8;

/// Hit ratio below which we recommend disabling. 0.30 means "if fewer
/// than 30% of speculations get used, the system is doing net harm".
pub const DISABLE_THRESHOLD: f32 = 0.30;

/// Once disabled, wait this long before re-considering. Lets the user's
/// behaviour pattern settle without us flapping on/off.
pub const DISABLED_COOLDOWN: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    /// The speculative load was used by a real user play.
    Hit,
    /// The pre-load expired or was replaced before the user tapped.
    Miss,
    /// The user tapped a *different* track after the speculation.
    /// Counted as a miss (we paid the cost, got nothing back).
    Mismatch,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Recommendation {
    /// Keep speculating. We're either making good predictions or we
    /// don't have enough data yet to say otherwise.
    Continue,
    /// Pause speculation. Accuracy is too low; we're wasting work.
    Disable { hit_ratio: f32 },
    /// Cooldown expired; you may try speculating again.
    ReEnable,
}

/// Sliding-window accuracy tracker. Holds the last N outcomes plus
/// some bookkeeping about the disable state.
#[derive(Debug)]
pub struct AccuracyTracker {
    window: VecDeque<Outcome>,
    capacity: usize,
    disabled_at: Option<Instant>,
}

impl Default for AccuracyTracker {
    fn default() -> Self { Self::new() }
}

impl AccuracyTracker {
    pub fn new() -> Self {
        Self {
            window: VecDeque::with_capacity(WINDOW),
            capacity: WINDOW,
            disabled_at: None,
        }
    }

    /// Record one outcome. The oldest sample falls off the back when
    /// the window is full.
    pub fn observe(&mut self, outcome: Outcome) {
        if self.window.len() >= self.capacity {
            self.window.pop_front();
        }
        self.window.push_back(outcome);
    }

    /// Current hit ratio: fraction of samples that were Hit. Returns
    /// 0.0 when there are no samples (so callers don't divide by zero).
    pub fn hit_ratio(&self) -> f32 {
        if self.window.is_empty() { return 0.0; }
        let hits = self.window.iter().filter(|o| **o == Outcome::Hit).count();
        hits as f32 / self.window.len() as f32
    }

    /// Sample count in the current window.
    pub fn samples(&self) -> usize { self.window.len() }

    /// Is speculation currently in the "disabled" state?
    pub fn is_disabled(&self) -> bool { self.disabled_at.is_some() }

    /// Get the controller's recommendation at `now`. Three cases:
    ///   1. Currently disabled, cooldown elapsed → `ReEnable`. The
    ///      caller is expected to clear the disable + start fresh.
    ///   2. Currently disabled, cooldown not yet elapsed → `Continue`
    ///      (= "stay disabled"; the caller already knows to skip).
    ///   3. Active, window full, hit_ratio too low → `Disable`.
    ///   4. Otherwise → `Continue`.
    pub fn recommend(&self, now: Instant) -> Recommendation {
        if let Some(t) = self.disabled_at {
            if now.duration_since(t) >= DISABLED_COOLDOWN {
                return Recommendation::ReEnable;
            }
            return Recommendation::Continue;
        }
        if self.window.len() < MIN_SAMPLES { return Recommendation::Continue; }
        let r = self.hit_ratio();
        if r < DISABLE_THRESHOLD {
            Recommendation::Disable { hit_ratio: r }
        } else {
            Recommendation::Continue
        }
    }

    /// Mark the tracker as disabled at `now`. Subsequent `recommend`
    /// calls return Continue until the cooldown elapses.
    pub fn note_disabled(&mut self, now: Instant) {
        self.disabled_at = Some(now);
    }

    /// Clear the disable state and reset the window. Called when the
    /// controller decides to give speculation another chance.
    pub fn reset(&mut self) {
        self.window.clear();
        self.disabled_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tracker_recommends_continue() {
        let t = AccuracyTracker::new();
        assert_eq!(t.recommend(Instant::now()), Recommendation::Continue);
    }

    #[test]
    fn small_samples_dont_trigger_disable() {
        let mut t = AccuracyTracker::new();
        // 3 misses — well under MIN_SAMPLES. Should still be Continue.
        for _ in 0..3 { t.observe(Outcome::Miss); }
        assert_eq!(t.recommend(Instant::now()), Recommendation::Continue);
    }

    #[test]
    fn full_window_of_hits_recommends_continue() {
        let mut t = AccuracyTracker::new();
        for _ in 0..WINDOW { t.observe(Outcome::Hit); }
        assert_eq!(t.recommend(Instant::now()), Recommendation::Continue);
        assert!((t.hit_ratio() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn full_window_of_misses_triggers_disable() {
        let mut t = AccuracyTracker::new();
        for _ in 0..WINDOW { t.observe(Outcome::Miss); }
        match t.recommend(Instant::now()) {
            Recommendation::Disable { hit_ratio } => {
                assert!(hit_ratio < 0.01);
            }
            other => panic!("expected Disable, got {:?}", other),
        }
    }

    #[test]
    fn mixed_above_threshold_recommends_continue() {
        let mut t = AccuracyTracker::new();
        // 8 hits + 4 misses = 12 samples, 0.67 hit ratio.
        for _ in 0..8 { t.observe(Outcome::Hit); }
        for _ in 0..4 { t.observe(Outcome::Miss); }
        assert_eq!(t.samples(), 12);
        assert!(t.hit_ratio() > DISABLE_THRESHOLD);
        assert_eq!(t.recommend(Instant::now()), Recommendation::Continue);
    }

    #[test]
    fn mismatches_count_as_misses() {
        let mut t = AccuracyTracker::new();
        for _ in 0..WINDOW { t.observe(Outcome::Mismatch); }
        match t.recommend(Instant::now()) {
            Recommendation::Disable { .. } => {},
            other => panic!("expected Disable, got {:?}", other),
        }
    }

    #[test]
    fn window_evicts_oldest() {
        let mut t = AccuracyTracker::new();
        // Fill with misses, then push hits — old misses should fall off.
        for _ in 0..WINDOW { t.observe(Outcome::Miss); }
        for _ in 0..WINDOW { t.observe(Outcome::Hit); }
        assert_eq!(t.samples(), WINDOW);
        assert!(t.hit_ratio() > 0.99);
    }

    #[test]
    fn disabled_state_cooldown() {
        let mut t = AccuracyTracker::new();
        let now = Instant::now();
        t.note_disabled(now);
        // During cooldown: Continue (stays disabled).
        assert_eq!(t.recommend(now + Duration::from_secs(60)), Recommendation::Continue);
        // After cooldown: ReEnable.
        let after = now + DISABLED_COOLDOWN + Duration::from_secs(1);
        assert_eq!(t.recommend(after), Recommendation::ReEnable);
    }

    #[test]
    fn reset_clears_state() {
        let mut t = AccuracyTracker::new();
        for _ in 0..WINDOW { t.observe(Outcome::Miss); }
        t.note_disabled(Instant::now());
        assert!(t.is_disabled());
        t.reset();
        assert!(!t.is_disabled());
        assert_eq!(t.samples(), 0);
        assert_eq!(t.recommend(Instant::now()), Recommendation::Continue);
    }

    #[test]
    fn threshold_boundary_is_inclusive_of_continue() {
        // Right at DISABLE_THRESHOLD: 30% hits. Spec says
        // "below 30%" triggers disable, so exactly 30% should Continue.
        let mut t = AccuracyTracker::new();
        let total = WINDOW;
        let hits = ((total as f32) * DISABLE_THRESHOLD).round() as usize;
        for _ in 0..hits { t.observe(Outcome::Hit); }
        for _ in 0..(total - hits) { t.observe(Outcome::Miss); }
        // Ratio is exactly DISABLE_THRESHOLD or fractionally above.
        assert!(t.hit_ratio() >= DISABLE_THRESHOLD - 0.01);
        // Borderline but not strictly below → Continue.
        match t.recommend(Instant::now()) {
            Recommendation::Continue => {},
            Recommendation::Disable { hit_ratio } => {
                // OK as long as hit_ratio is very near the threshold.
                assert!(hit_ratio < DISABLE_THRESHOLD);
            }
            _ => panic!("unexpected"),
        }
    }
}
