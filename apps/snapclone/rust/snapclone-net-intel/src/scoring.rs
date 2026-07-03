//! Path scoring with time decay.
//!
//! NETWORK_ARCHITECTURE.md §1: "Time decay (recent success weighted higher),
//! confidence scoring, negative caching."

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Protocol used for a connection attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Protocol { Udp, TcpDirect, TcpRelay, Ipv6, Bluetooth }

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Udp       => "udp",
            Self::TcpDirect => "tcp_direct",
            Self::TcpRelay  => "tcp_relay",
            Self::Ipv6      => "ipv6",
            Self::Bluetooth => "bluetooth",
        }
    }
}

/// Score for a single (peer, network_bin, protocol) combination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathScore {
    pub successes:    u32,
    pub failures:     u32,
    pub last_success: Option<DateTime<Utc>>,
    pub last_failure: Option<DateTime<Utc>>,
    /// Exponentially weighted moving average of RTT on success (ms).
    pub ewma_rtt_ms:  Option<f32>,
}

impl PathScore {
    pub fn new() -> Self {
        Self { successes: 0, failures: 0,
               last_success: None, last_failure: None, ewma_rtt_ms: None }
    }

    pub fn record_success(&mut self, rtt_ms: Option<f32>) {
        self.successes += 1;
        self.last_success = Some(Utc::now());
        if let Some(rtt) = rtt_ms {
            self.ewma_rtt_ms = Some(match self.ewma_rtt_ms {
                None    => rtt,
                Some(e) => e * 0.8 + rtt * 0.2,
            });
        }
    }

    pub fn record_failure(&mut self) {
        self.failures += 1;
        self.last_failure = Some(Utc::now());
    }

    /// Confidence in [0.0, 1.0] with time decay.
    /// Recent successes score high; old data decays toward 0.5 (uncertain).
    ///
    /// Decay half-life: 24 hours for successes, 4 hours for failures
    /// (failures are "forgotten" faster — network conditions change).
    ///
    /// V32.34+ FIX: the weight for each side must scale with the NUMBER of
    /// observations, not just the recency of the latest one. The previous
    /// version multiplied a single decay factor (≤1.0, from the most-recent
    /// timestamp) — so five successes counted the same as one, `total`
    /// capped near 1.0, and the `evidence = total/5` term capped `evidence`
    /// at ~0.2. That made confidence unable to exceed ~0.6 no matter how many
    /// successes accrued (and unable to drop below ~0.4 on failures) — the
    /// blend-toward-0.5 term dominated forever. Multiplying the decay by the
    /// count lets evidence actually accumulate: N recent successes →
    /// weight≈N → evidence saturates → confidence approaches the raw ratio.
    pub fn confidence(&self) -> f32 {
        let success_weight =
            self.successes as f32 * self.decayed_weight(self.last_success, 86400.0);
        let failure_weight =
            self.failures as f32 * self.decayed_weight(self.last_failure, 14400.0);
        let total = success_weight + failure_weight;
        if total < 0.001 { return 0.5; } // no data → uncertain
        let raw = success_weight / total;
        // Blend toward 0.5 when total evidence is low. With count-weighting,
        // ~5 recent observations saturate `evidence` to 1.0 so the raw ratio
        // dominates; a single old observation stays near 0.5 (uncertain).
        let evidence = (total / 5.0).min(1.0);
        raw * evidence + 0.5 * (1.0 - evidence)
    }

    fn decayed_weight(&self, ts: Option<DateTime<Utc>>, half_life_secs: f64) -> f32 {
        let Some(t) = ts else { return 0.0 };
        let age_secs = Utc::now().signed_duration_since(t).num_seconds() as f64;
        let decay = (-age_secs * std::f64::consts::LN_2 / half_life_secs).exp();
        decay as f32
    }

    /// True if this path is "warm" — high confidence + recent success.
    pub fn is_warm(&self) -> bool {
        self.confidence() > 0.7
            && self.last_success
               .map(|t| Utc::now().signed_duration_since(t).num_hours() < 4)
               .unwrap_or(false)
    }

    /// True if this path should be skipped (negative cache).
    pub fn is_negcached(&self) -> bool {
        self.confidence() < 0.2
            && self.last_failure
               .map(|t| Utc::now().signed_duration_since(t).num_minutes() < 30)
               .unwrap_or(false)
    }
}

impl Default for PathScore { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fresh_is_uncertain() {
        assert!((PathScore::new().confidence() - 0.5).abs() < 0.01);
    }
    #[test]
    fn success_raises_confidence() {
        let mut s = PathScore::new();
        for _ in 0..5 { s.record_success(Some(50.0)); }
        assert!(s.confidence() > 0.7);
        assert!(s.is_warm());
    }
    #[test]
    fn repeated_failure_negcaches() {
        let mut s = PathScore::new();
        for _ in 0..5 { s.record_failure(); }
        assert!(s.confidence() < 0.3);
    }

    #[test]
    fn negcached_predicate_fires_after_recent_failures() {
        // is_negcached needs confidence < 0.2 AND a failure in the last 30
        // min. Pre-fix this could never fire because confidence floored
        // ~0.4. With count-weighting, several recent failures push it under
        // 0.2 and the path is correctly skipped.
        let mut s = PathScore::new();
        for _ in 0..6 { s.record_failure(); }
        assert!(s.is_negcached(), "recent repeated failures should negcache");
    }

    #[test]
    fn warm_predicate_fires_after_recent_successes() {
        // is_warm needs confidence > 0.7 AND a success in the last 4h.
        // Pre-fix confidence capped ~0.6 so is_warm never fired and the
        // path-preference ranking was inert.
        let mut s = PathScore::new();
        for _ in 0..5 { s.record_success(Some(40.0)); }
        assert!(s.is_warm(), "recent repeated successes should be warm");
    }

    #[test]
    fn mostly_success_with_one_failure_stays_confident() {
        // Realistic case: a path that works almost always. Many successes
        // and a single failure (both recent) should still read as
        // high-confidence, not get dragged to the middle.
        let mut s = PathScore::new();
        for _ in 0..10 { s.record_success(Some(50.0)); }
        s.record_failure();
        let c = s.confidence();
        assert!(c > 0.7, "10 successes vs 1 failure should stay confident, got {c}");
        assert!(!s.is_negcached());
    }

    #[test]
    fn single_observation_stays_near_uncertain() {
        // One success shouldn't immediately read as fully confident — with
        // only 1 unit of evidence, the blend keeps it closer to 0.5 than to
        // 1.0 (evidence = 1/5 = 0.2 → 1.0*0.2 + 0.5*0.8 = 0.6).
        let mut s = PathScore::new();
        s.record_success(Some(50.0));
        let c = s.confidence();
        assert!(c > 0.5 && c < 0.7,
                "a single success should be mildly-positive, not certain, got {c}");
    }
}
