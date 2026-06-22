#![allow(dead_code)]
// src/rtt_tracker.rs
//
// Rolling window of observed round-trip times to the active device.
// Keeps the most recent N samples and exposes p50/p95 readouts so the
// cue scheduler can size cues to ACTUAL network conditions rather than
// a hardcoded estimate.
//
// Why this matters: if the controller is on the same Wi-Fi AP as the
// device, typical RTT is 5-15 ms and a 250 ms cue is overkill
// (perceived as a clunky "system sound"). If the controller is across
// a mesh hop with iffy reception, RTT can balloon to 200-400 ms and a
// 250 ms cue is barely long enough to mask the gap. Adaptive sizing
// makes the cue feel like a natural part of the music regardless.
//
// The tracker is also the foundation for measuring latency budgets: if
// p95 RTT crosses a threshold, the controller knows the network is
// degrading and can adjust pre-staging policy (or surface a warning).

use std::collections::VecDeque;
use std::time::Duration;

/// Default rolling window size. 50 samples at one per command is ~30s
/// of activity for an active user, enough to track network changes
/// without lagging behind transient blips.
pub const DEFAULT_WINDOW: usize = 50;

/// Floor for the cue-duration recommendation, in ms. Below this the cue
/// is too short to be perceptually useful (it just sounds like a click).
pub const MIN_CUE_MS: u32 = 100;

/// Ceiling for the cue-duration recommendation. Above this the cue
/// starts to feel like its own thing rather than a fade-into-music.
pub const MAX_CUE_MS: u32 = 800;

#[derive(Debug)]
pub struct RttTracker {
    samples: VecDeque<Duration>,
    max_samples: usize,
}

impl Default for RttTracker {
    fn default() -> Self {
        Self {
            samples: VecDeque::with_capacity(DEFAULT_WINDOW),
            max_samples: DEFAULT_WINDOW,
        }
    }
}

impl RttTracker {
    pub fn new() -> Self { Self::default() }

    pub fn with_window(max_samples: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(max_samples),
            max_samples,
        }
    }

    /// Add one RTT observation. Oldest samples evict when at capacity.
    pub fn observe(&mut self, rtt: Duration) {
        if self.samples.len() == self.max_samples {
            self.samples.pop_front();
        }
        self.samples.push_back(rtt);
    }

    pub fn sample_count(&self) -> usize { self.samples.len() }

    /// p50 (median) RTT. Returns None when fewer than 3 samples exist —
    /// percentiles aren't meaningful below that.
    pub fn p50(&self) -> Option<Duration> { self.percentile(0.50) }

    /// p95 RTT. Useful for cue duration sizing: pad above the typical
    /// latency to catch most variation.
    pub fn p95(&self) -> Option<Duration> { self.percentile(0.95) }

    /// Most recent observation. Useful for diagnostic UI.
    pub fn last(&self) -> Option<Duration> { self.samples.back().copied() }

    /// Recommend a cue duration based on the observed network. Returns
    /// a default when we don't have enough samples yet.
    pub fn recommend_cue_ms(&self) -> u32 {
        match self.p95() {
            Some(d) => {
                // Cue should outlast the typical-worst-case latency by
                // ~20% so the device's real audio arrives during the
                // decay tail rather than after it.
                let target = (d.as_millis() as f32 * 1.2) as u32;
                target.clamp(MIN_CUE_MS, MAX_CUE_MS)
            }
            None => 350,    // sensible default when no samples yet
        }
    }

    /// Recommend a network command timeout in milliseconds. Aims at
    /// ~5× the observed p95 RTT so transient blips don't kill commands,
    /// but well below the default reqwest timeout (~30s) so dead
    /// devices fail fast.
    ///
    /// Floor: 500 ms — even on a perfect LAN we want enough margin for
    /// the device to do its own work (parse, dispatch, ACK).
    /// Ceiling: 5000 ms — beyond this the user's better served by an
    /// explicit "device unreachable" than a long hang.
    pub fn recommend_timeout_ms(&self) -> u32 {
        const MIN_TIMEOUT_MS: u32 = 500;
        const MAX_TIMEOUT_MS: u32 = 5000;
        match self.p95() {
            Some(d) => {
                let target = (d.as_millis() as u32).saturating_mul(5);
                target.clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS)
            }
            None => 3000,   // pre-RTT default: trust but verify
        }
    }

    fn percentile(&self, p: f32) -> Option<Duration> {
        if self.samples.len() < 3 { return None; }
        let mut sorted: Vec<Duration> = self.samples.iter().copied().collect();
        sorted.sort();
        let idx = ((sorted.len() as f32 - 1.0) * p).round() as usize;
        sorted.get(idx).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_tracker_has_no_samples() {
        let t = RttTracker::new();
        assert_eq!(t.sample_count(), 0);
        assert!(t.p50().is_none());
        assert!(t.p95().is_none());
    }

    #[test]
    fn recommend_cue_falls_back_to_default_without_samples() {
        let t = RttTracker::new();
        let r = t.recommend_cue_ms();
        assert!(r >= MIN_CUE_MS);
        assert!(r <= MAX_CUE_MS);
    }

    #[test]
    fn observe_grows_sample_count() {
        let mut t = RttTracker::new();
        t.observe(Duration::from_millis(50));
        t.observe(Duration::from_millis(100));
        assert_eq!(t.sample_count(), 2);
    }

    #[test]
    fn p50_below_minimum_samples_returns_none() {
        let mut t = RttTracker::new();
        t.observe(Duration::from_millis(50));
        t.observe(Duration::from_millis(100));
        // 2 samples: still below the 3-sample minimum.
        assert!(t.p50().is_none());
    }

    #[test]
    fn p50_works_at_minimum_samples() {
        let mut t = RttTracker::new();
        t.observe(Duration::from_millis(10));
        t.observe(Duration::from_millis(50));
        t.observe(Duration::from_millis(100));
        // Median of [10,50,100] is 50.
        assert_eq!(t.p50().unwrap(), Duration::from_millis(50));
    }

    #[test]
    fn p95_picks_high_tail() {
        let mut t = RttTracker::with_window(200);  // bigger than our 100 samples
        for ms in 1..=100 {
            t.observe(Duration::from_millis(ms as u64));
        }
        // p95 of [1..=100] (all retained) is the 95th value when sorted, ≈95.
        let p95 = t.p95().unwrap().as_millis() as u64;
        assert!(p95 >= 94 && p95 <= 96,
                "p95 should be near 95, got {}", p95);
    }

    #[test]
    fn samples_evict_oldest_at_capacity() {
        let mut t = RttTracker::with_window(5);
        for ms in 1..=10 {
            t.observe(Duration::from_millis(ms));
        }
        assert_eq!(t.sample_count(), 5);
        // Should contain the last 5: 6,7,8,9,10
        let median = t.p50().unwrap().as_millis() as u64;
        assert_eq!(median, 8);
    }

    #[test]
    fn recommend_cue_scales_with_observed_latency() {
        let mut fast = RttTracker::new();
        for _ in 0..10 { fast.observe(Duration::from_millis(15)); }
        let mut slow = RttTracker::new();
        for _ in 0..10 { slow.observe(Duration::from_millis(250)); }
        let cue_fast = fast.recommend_cue_ms();
        let cue_slow = slow.recommend_cue_ms();
        assert!(cue_slow > cue_fast,
                "slow network should recommend longer cue: fast={} slow={}",
                cue_fast, cue_slow);
    }

    #[test]
    fn recommend_cue_respects_min_and_max() {
        let mut tiny = RttTracker::new();
        for _ in 0..10 { tiny.observe(Duration::from_millis(1)); }
        assert!(tiny.recommend_cue_ms() >= MIN_CUE_MS);

        let mut huge = RttTracker::new();
        for _ in 0..10 { huge.observe(Duration::from_millis(2000)); }
        assert!(huge.recommend_cue_ms() <= MAX_CUE_MS);
    }

    #[test]
    fn last_returns_most_recent() {
        let mut t = RttTracker::new();
        t.observe(Duration::from_millis(10));
        t.observe(Duration::from_millis(20));
        t.observe(Duration::from_millis(30));
        assert_eq!(t.last().unwrap(), Duration::from_millis(30));
    }

    #[test]
    fn recommend_timeout_fast_network_gives_floor() {
        let mut t = RttTracker::new();
        for _ in 0..10 { t.observe(Duration::from_millis(5)); }
        // 5ms × 5 = 25ms which would be far below useful — floor kicks in.
        assert_eq!(t.recommend_timeout_ms(), 500);
    }

    #[test]
    fn recommend_timeout_slow_network_caps_at_ceiling() {
        let mut t = RttTracker::new();
        for _ in 0..10 { t.observe(Duration::from_millis(2000)); }
        // 2000 × 5 = 10000 ms; ceiling at 5000 keeps things responsive.
        assert_eq!(t.recommend_timeout_ms(), 5000);
    }

    #[test]
    fn recommend_timeout_typical_network_in_range() {
        let mut t = RttTracker::new();
        for _ in 0..10 { t.observe(Duration::from_millis(100)); }
        let r = t.recommend_timeout_ms();
        // p95 ≈ 100 → 500 ms recommended (matches the floor for this RTT)
        assert!(r >= 500 && r <= 5000);
    }

    #[test]
    fn recommend_timeout_without_samples_uses_default() {
        let t = RttTracker::new();
        assert_eq!(t.recommend_timeout_ms(), 3000);
    }
}
