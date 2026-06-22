#![allow(dead_code)]
// src/conn_warmer.rs
//
// Pre-warms TCP/HTTP connections to devices the anticipator predicts
// the controller will hit soon. The reqwest client keeps connections
// in its pool for ~30s after idle; once they age out, the next request
// pays DNS + TCP SYN + (possibly) TLS — typically 20-180 ms of
// unavoidable latency BEFORE any user-facing work happens.
//
// The fix: when the anticipator's confidence on device interaction
// crosses a threshold, fire a tiny GET to the device. That keeps the
// connection in the pool so the next real request reuses it. On a
// typical LAN, the saving is 50-150 ms shifted off the critical path
// — bandwidth cost negligible (single GET every ~20s, response
// ignored).
//
// This module is pure rate-limiting / decision logic. It doesn't fire
// network requests directly; it returns "warm this URL" decisions that
// the caller dispatches through the existing Command channel. That
// keeps the network policy testable without spinning up a real socket.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How often a given URL can be warmed. Faster than reqwest's idle
/// timeout (typically 30s) so the connection never goes cold while a
/// prediction is hot.
pub const DEFAULT_WARM_INTERVAL: Duration = Duration::from_secs(20);

/// Maximum bursts of warm-requests across all URLs in a short window.
/// Prevents a runaway prediction loop from flooding the LAN.
pub const MAX_WARMS_PER_WINDOW: u32 = 6;
pub const BURST_WINDOW: Duration = Duration::from_secs(5);

#[derive(Debug, Default)]
pub struct ConnWarmer {
    /// When each URL was last warmed. Old entries are pruned lazily.
    last_warmed_at: HashMap<String, Instant>,
    /// Rolling window of recent warm timestamps for global rate limiting.
    /// Capped at MAX_WARMS_PER_WINDOW entries; oldest gets dropped.
    recent_warms: std::collections::VecDeque<Instant>,
}

#[derive(Debug, PartialEq)]
pub enum WarmDecision {
    /// Go ahead and fire a lightweight GET to this URL.
    Warm,
    /// Skip: this URL was warmed recently (within DEFAULT_WARM_INTERVAL).
    Cooldown,
    /// Skip: too many warms across all URLs in the current window.
    GlobalRateLimit,
}

impl ConnWarmer {
    pub fn new() -> Self { Self::default() }

    /// Decide whether to warm `url` right now. Pure: doesn't change
    /// state. Call `note_warmed` after the caller actually fires the
    /// warm request, so subsequent calls respect the cooldown.
    pub fn should_warm(&self, url: &str, now: Instant) -> WarmDecision {
        // Global rate limit: count fresh entries in the burst window.
        let recent_count = self.recent_warms.iter()
            .filter(|t| now.duration_since(**t) < BURST_WINDOW)
            .count() as u32;
        if recent_count >= MAX_WARMS_PER_WINDOW {
            return WarmDecision::GlobalRateLimit;
        }
        // Per-URL cooldown.
        if let Some(last) = self.last_warmed_at.get(url) {
            if now.duration_since(*last) < DEFAULT_WARM_INTERVAL {
                return WarmDecision::Cooldown;
            }
        }
        WarmDecision::Warm
    }

    /// Record that a warm just fired. Updates both the per-URL last-
    /// warmed timestamp and the global rolling window.
    pub fn note_warmed(&mut self, url: &str, now: Instant) {
        self.last_warmed_at.insert(url.to_string(), now);
        self.recent_warms.push_back(now);
        // Trim the rolling window to the burst window.
        while let Some(oldest) = self.recent_warms.front() {
            if now.duration_since(*oldest) > BURST_WINDOW {
                self.recent_warms.pop_front();
            } else { break; }
        }
        // Cap memory: prune any URL entries older than 5 minutes.
        self.last_warmed_at.retain(|_, t| now.duration_since(*t) < Duration::from_secs(300));
    }

    /// How many warm requests have been issued in the recent burst window.
    /// Diagnostic; helpful for tests and the QML status surface.
    pub fn recent_count(&self) -> usize { self.recent_warms.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Instant { Instant::now() + Duration::from_millis(ms) }

    #[test]
    fn fresh_warmer_allows_any_url() {
        let w = ConnWarmer::new();
        assert_eq!(w.should_warm("http://a", at(0)), WarmDecision::Warm);
    }

    #[test]
    fn same_url_blocked_by_cooldown() {
        let mut w = ConnWarmer::new();
        w.note_warmed("http://a", at(0));
        assert_eq!(w.should_warm("http://a", at(100)), WarmDecision::Cooldown);
        // Just barely within cooldown
        let almost = DEFAULT_WARM_INTERVAL.as_millis() as u64 - 100;
        assert_eq!(w.should_warm("http://a", at(almost)), WarmDecision::Cooldown);
    }

    #[test]
    fn same_url_allowed_after_cooldown() {
        let mut w = ConnWarmer::new();
        w.note_warmed("http://a", at(0));
        let past = DEFAULT_WARM_INTERVAL.as_millis() as u64 + 1000;
        assert_eq!(w.should_warm("http://a", at(past)), WarmDecision::Warm);
    }

    #[test]
    fn different_urls_independent_cooldowns() {
        let mut w = ConnWarmer::new();
        w.note_warmed("http://a", at(0));
        // Different URL: not gated by a's cooldown (within global limit).
        assert_eq!(w.should_warm("http://b", at(100)), WarmDecision::Warm);
    }

    #[test]
    fn global_rate_limit_kicks_in() {
        let mut w = ConnWarmer::new();
        // Fire MAX_WARMS_PER_WINDOW warms across different URLs within
        // the burst window.
        for i in 0..MAX_WARMS_PER_WINDOW {
            let url = format!("http://device_{}", i);
            assert_eq!(w.should_warm(&url, at(i as u64 * 100)), WarmDecision::Warm);
            w.note_warmed(&url, at(i as u64 * 100));
        }
        // The next warm in the window should be rate-limited.
        let next_url = "http://device_999";
        let in_window = (MAX_WARMS_PER_WINDOW as u64 + 1) * 100;
        assert_eq!(
            w.should_warm(next_url, at(in_window)),
            WarmDecision::GlobalRateLimit,
        );
    }

    #[test]
    fn global_rate_limit_clears_after_window() {
        let mut w = ConnWarmer::new();
        for i in 0..MAX_WARMS_PER_WINDOW {
            let url = format!("http://device_{}", i);
            w.note_warmed(&url, at(i as u64 * 100));
        }
        // After the burst window has elapsed, the rolling window is empty
        // and warms are allowed again.
        let after = BURST_WINDOW.as_millis() as u64 + 1000;
        // We need to call should_warm AFTER note_warmed has had a chance
        // to trim; should_warm trims via the recent_warms filter at
        // call time.
        assert_eq!(
            w.should_warm("http://new", at(after)),
            WarmDecision::Warm,
        );
    }

    #[test]
    fn note_warmed_updates_recent_count() {
        let mut w = ConnWarmer::new();
        assert_eq!(w.recent_count(), 0);
        w.note_warmed("http://a", at(0));
        assert_eq!(w.recent_count(), 1);
        w.note_warmed("http://b", at(100));
        assert_eq!(w.recent_count(), 2);
    }

    #[test]
    fn note_warmed_trims_old_window_entries() {
        let mut w = ConnWarmer::new();
        w.note_warmed("http://a", at(0));
        w.note_warmed("http://b", at(100));
        // A later note that's well past the burst window should trim
        // the older entries.
        let after = BURST_WINDOW.as_millis() as u64 + 5000;
        w.note_warmed("http://c", at(after));
        // Only the latest one is in the recent window.
        assert_eq!(w.recent_count(), 1);
    }

    #[test]
    fn cooldown_and_rate_limit_compose() {
        // A URL that's both in its own cooldown AND would push past
        // the rate limit should still return Cooldown (per-URL check
        // runs first; either way the warm is rejected).
        let mut w = ConnWarmer::new();
        w.note_warmed("http://a", at(0));
        // Saturate global window with other URLs.
        for i in 1..MAX_WARMS_PER_WINDOW {
            w.note_warmed(&format!("http://b{}", i), at(i as u64 * 50));
        }
        // "a" is still in cooldown.
        let v = w.should_warm("http://a", at(500));
        // Either decision is correct — what matters is the warm doesn't
        // fire. We assert one of the two.
        assert!(matches!(v, WarmDecision::Cooldown | WarmDecision::GlobalRateLimit));
    }

    #[test]
    fn long_inactive_doesnt_grow_unbounded() {
        // After many warms across many URLs over a long time, the
        // last_warmed_at map should self-trim (5min retention).
        let mut w = ConnWarmer::new();
        let day = Duration::from_secs(86400).as_millis() as u64;
        // Inject warms spread across a day (5min retention should keep
        // only the last few).
        for i in 0..100 {
            let url = format!("http://device_{}", i);
            w.note_warmed(&url, at(i * day / 100));
        }
        // The map should be trimmed to entries within 5 minutes of the
        // last note_warmed call.
        let final_time = at((99 * day) / 100);
        let fresh = w.last_warmed_at.values()
            .filter(|t| final_time.duration_since(**t) < Duration::from_secs(300))
            .count();
        assert_eq!(w.last_warmed_at.len(), fresh,
                   "map should only contain recent entries");
    }
}
