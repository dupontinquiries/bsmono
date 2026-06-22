// src/circuit_breaker.rs
#![allow(dead_code)]
//
// Per-URL circuit breaker. The motivating bug: when a device becomes
// unreachable, queued user gestures (volume drags) keep hitting it for
// the full TCP timeout (~3-5 s each). With ~10 commands queued and
// failing one at a time, the user sees "No route to host" errors spam
// for 30-50 seconds and auto-failover keeps re-triggering against the
// same downed device because each fresh failure ratchets the failure
// count.
//
// The breaker tracks per-URL failure counts and a recovery deadline:
//
//   CLOSED   — normal. Allow all traffic.
//   OPEN     — too many failures. Fast-fail without even attempting the
//              network call. Reverts to HALF_OPEN after BACKOFF.
//   HALF_OPEN — allow ONE attempt through. Success → CLOSED. Failure
//              → OPEN with longer BACKOFF (capped).
//
// Decisions are pure functions of (current state, now). The worker
// thread queries before sending; the controller updates state on each
// network result. Shared cross-thread via Arc<Mutex<>>.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Failures within this short window count toward tripping the breaker.
/// Older failures age out so transient blips don't permanently mark a
/// device as dead.
pub const FAILURE_WINDOW: Duration = Duration::from_secs(30);

/// Number of consecutive failures within FAILURE_WINDOW required to
/// open the breaker. 3 matches our existing failover trigger threshold.
pub const TRIP_THRESHOLD: u32 = 3;

/// Initial backoff when the breaker first opens. Doubles each retry up
/// to MAX_BACKOFF.
pub const INITIAL_BACKOFF: Duration = Duration::from_secs(5);
pub const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// How long to keep `HalfOpen` blocking subsequent callers after the
/// one probe is dispatched. The probe's actual reqwest call has a
/// 5s timeout so this matches the worst-case wait the probe will
/// take to resolve to note_success/note_failure. If the probe never
/// resolves (caller bug — should always pair check with one of the
/// note_* methods), we fall back to "block for at most 10s, then
/// re-evaluate."
const HALF_OPEN_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq)]
pub enum BreakerState {
    Closed,
    Open    { until: Instant, attempt: u32 },
    /// One probe has been allowed; we're waiting for `note_success` or
    /// `note_failure` to resolve it. `probe_until` is a safety net — if
    /// the caller never reports back, we re-evaluate after this point.
    HalfOpen { attempt: u32, probe_until: Instant },
}

#[derive(Debug, Clone)]
struct UrlState {
    state: BreakerState,
    /// Timestamps of recent failures, trimmed to FAILURE_WINDOW. Used
    /// to decide when to trip the breaker.
    recent_failures: Vec<Instant>,
}

impl UrlState {
    fn new() -> Self { Self { state: BreakerState::Closed, recent_failures: Vec::new() } }

    fn trim_old(&mut self, now: Instant) {
        self.recent_failures.retain(|t| now.duration_since(*t) < FAILURE_WINDOW);
    }
}

#[derive(Debug, Default)]
pub struct CircuitBreaker {
    urls: HashMap<String, UrlState>,
}

#[derive(Debug, PartialEq)]
pub enum BreakerDecision {
    /// OK to send.
    Allow,
    /// Breaker open — fast-fail. `retry_at` is when the next attempt
    /// will be allowed.
    Blocked { retry_at: Instant },
}

/// Exponential-backoff formula: 5s, 10s, 20s, 40s, 60s (capped). Used
/// both when an OPEN breaker re-opens after a half-open failure and
/// when a half-open probe times out without resolving.
fn backoff_for_attempt(attempt: u32) -> Duration {
    std::cmp::min(
        INITIAL_BACKOFF.saturating_mul(1u32 << attempt.min(4)),
        MAX_BACKOFF,
    )
}

impl CircuitBreaker {
    pub fn new() -> Self { Self::default() }

    /// Should the caller send a request to this URL right now?
    ///
    /// Closed                 → Allow
    /// Open with backoff left → Blocked
    /// Open with backoff up   → transition to HalfOpen, Allow (this is
    ///                          the single probe attempt)
    /// HalfOpen with probe
    ///   still in flight      → Blocked (one probe at a time — that's
    ///                          the entire point of half-open in a
    ///                          standard breaker)
    /// HalfOpen with probe
    ///   timed out            → re-evaluate as if Open with new backoff,
    ///                          which usually means transition straight
    ///                          to a fresh probe
    ///
    /// The "one probe at a time" property was missing before: subsequent
    /// callers in HalfOpen used to all get Allow, defeating the breaker.
    /// With queued requests in the worker, ten Allow returns would
    /// hammer the device the moment backoff elapsed.
    pub fn check(&mut self, url: &str, now: Instant) -> BreakerDecision {
        let entry = self.urls.entry(url.to_string()).or_insert_with(UrlState::new);
        match &entry.state {
            BreakerState::Closed => BreakerDecision::Allow,
            BreakerState::Open { until, attempt } => {
                if now >= *until {
                    // Backoff elapsed — allow this single probe and
                    // immediately mark probe-in-flight so the next
                    // check (likely on a queued request) gets Blocked.
                    let a = *attempt;
                    entry.state = BreakerState::HalfOpen {
                        attempt: a,
                        probe_until: now + HALF_OPEN_PROBE_TIMEOUT,
                    };
                    BreakerDecision::Allow
                } else {
                    BreakerDecision::Blocked { retry_at: *until }
                }
            }
            BreakerState::HalfOpen { attempt, probe_until } => {
                if now >= *probe_until {
                    // Probe timed out without resolving — caller may
                    // have dropped the request. Fall back to Open with
                    // a fresh backoff (extends to discourage hammering),
                    // and let the next check after that probe again.
                    let a = *attempt;
                    let backoff = backoff_for_attempt(a);
                    let until = now + backoff;
                    entry.state = BreakerState::Open {
                        until, attempt: a,
                    };
                    BreakerDecision::Blocked { retry_at: until }
                } else {
                    BreakerDecision::Blocked { retry_at: *probe_until }
                }
            }
        }
    }

    /// Record a failure. Trips the breaker when consecutive failures
    /// cross the threshold; extends the backoff exponentially on
    /// repeated trips (HalfOpen → Open).
    pub fn note_failure(&mut self, url: &str, now: Instant) {
        let entry = self.urls.entry(url.to_string()).or_insert_with(UrlState::new);
        entry.recent_failures.push(now);
        entry.trim_old(now);
        let next_attempt = match &entry.state {
            BreakerState::Open { attempt, .. }       => attempt + 1,
            BreakerState::HalfOpen { attempt, .. }   => attempt + 1,
            BreakerState::Closed                     => 0,
        };
        let was_half_open =
            matches!(entry.state, BreakerState::HalfOpen { .. });
        if was_half_open
            || entry.recent_failures.len() as u32 >= TRIP_THRESHOLD
        {
            entry.state = BreakerState::Open {
                until: now + backoff_for_attempt(next_attempt),
                attempt: next_attempt,
            };
        }
    }

    /// Record a success. Resets to CLOSED — past failures forgiven.
    pub fn note_success(&mut self, url: &str) {
        let entry = self.urls.entry(url.to_string()).or_insert_with(UrlState::new);
        entry.state = BreakerState::Closed;
        entry.recent_failures.clear();
    }

    /// Diagnostic: is this URL currently open (i.e. being fast-failed)?
    pub fn is_open(&self, url: &str) -> bool {
        matches!(
            self.urls.get(url).map(|s| &s.state),
            Some(BreakerState::Open { .. })
        )
    }

    /// Diagnostic: how many recent failures has this URL accumulated?
    pub fn recent_failure_count(&self, url: &str) -> usize {
        self.urls.get(url).map(|s| s.recent_failures.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Instant { Instant::now() }

    #[test]
    fn fresh_breaker_allows_traffic() {
        let mut b = CircuitBreaker::new();
        assert_eq!(b.check("http://x", now()), BreakerDecision::Allow);
    }

    #[test]
    fn one_failure_does_not_trip() {
        let mut b = CircuitBreaker::new();
        b.note_failure("http://x", now());
        assert_eq!(b.check("http://x", now()), BreakerDecision::Allow);
    }

    #[test]
    fn threshold_failures_trip_breaker() {
        let mut b = CircuitBreaker::new();
        let t = now();
        for i in 0..TRIP_THRESHOLD {
            b.note_failure("http://x", t + Duration::from_millis(i as u64 * 100));
        }
        assert!(matches!(b.check("http://x", t + Duration::from_millis(500)),
                         BreakerDecision::Blocked { .. }));
    }

    #[test]
    fn success_resets_breaker() {
        let mut b = CircuitBreaker::new();
        for _ in 0..TRIP_THRESHOLD {
            b.note_failure("http://x", now());
        }
        b.note_success("http://x");
        assert_eq!(b.check("http://x", now()), BreakerDecision::Allow);
    }

    #[test]
    fn half_open_allows_one_probe() {
        let mut b = CircuitBreaker::new();
        let t = now();
        for _ in 0..TRIP_THRESHOLD {
            b.note_failure("http://x", t);
        }
        // Past the initial backoff
        let later = t + INITIAL_BACKOFF + Duration::from_secs(1);
        let decision = b.check("http://x", later);
        assert_eq!(decision, BreakerDecision::Allow);
    }

    #[test]
    fn half_open_failure_reopens_with_longer_backoff() {
        let mut b = CircuitBreaker::new();
        let t = now();
        for _ in 0..TRIP_THRESHOLD {
            b.note_failure("http://x", t);
        }
        let later = t + INITIAL_BACKOFF + Duration::from_secs(1);
        // Enter half-open and fail again.
        b.check("http://x", later);
        b.note_failure("http://x", later);
        // Should be open with a longer backoff than the initial one.
        if let Some(state) = b.urls.get("http://x") {
            match &state.state {
                BreakerState::Open { attempt, .. } => assert!(*attempt > 0),
                _ => panic!("expected Open after half-open failure"),
            }
        }
    }

    #[test]
    fn different_urls_independent() {
        let mut b = CircuitBreaker::new();
        for _ in 0..TRIP_THRESHOLD {
            b.note_failure("http://a", now());
        }
        // "b" untouched, still allowed
        assert_eq!(b.check("http://b", now()), BreakerDecision::Allow);
        assert!(matches!(b.check("http://a", now()), BreakerDecision::Blocked { .. }));
    }

    #[test]
    fn old_failures_age_out() {
        let mut b = CircuitBreaker::new();
        let t0 = now();
        // One old failure, way outside the window.
        b.note_failure("http://x", t0);
        // Forward by 2× the window
        let t1 = t0 + FAILURE_WINDOW * 2;
        // Two more recent failures — total 3 raw, but old one aged out.
        b.note_failure("http://x", t1);
        b.note_failure("http://x", t1);
        // Should still be allowed — only 2 recent failures.
        // (We're using the closed-state path, not the open state.)
        // Actually note_failure already trimmed before deciding. With 2
        // recent failures, threshold isn't met.
        assert_eq!(b.recent_failure_count("http://x"), 2);
    }

    #[test]
    fn is_open_reflects_state() {
        let mut b = CircuitBreaker::new();
        assert!(!b.is_open("http://x"));
        for _ in 0..TRIP_THRESHOLD {
            b.note_failure("http://x", now());
        }
        assert!(b.is_open("http://x"));
        b.note_success("http://x");
        assert!(!b.is_open("http://x"));
    }

    /// Regression: half-open used to leak ALL subsequent callers
    /// through, defeating the breaker's purpose. After the first
    /// probe is allowed, queued callers must be Blocked until the
    /// probe resolves via note_success or note_failure.
    #[test]
    fn half_open_blocks_concurrent_probes() {
        let mut b = CircuitBreaker::new();
        let t = now();
        for _ in 0..TRIP_THRESHOLD {
            b.note_failure("http://x", t);
        }
        let later = t + INITIAL_BACKOFF + Duration::from_secs(1);

        // First check after backoff: Allow (we issue the probe).
        assert_eq!(b.check("http://x", later), BreakerDecision::Allow);

        // SECOND check while probe is still in flight: must be Blocked.
        // Previously this returned Allow — letting all queued requests
        // hammer the device the moment the backoff elapsed.
        match b.check("http://x", later + Duration::from_millis(1)) {
            BreakerDecision::Blocked { .. } => {}
            other => panic!(
                "expected Blocked on concurrent probe, got {:?}", other,
            ),
        }

        // And a third, just to be sure.
        match b.check("http://x", later + Duration::from_millis(50)) {
            BreakerDecision::Blocked { .. } => {}
            other => panic!(
                "third concurrent probe should also be Blocked, got {:?}",
                other,
            ),
        }
    }

    /// Successful probe resolution: Allow → success → Closed → Allow.
    #[test]
    fn half_open_success_returns_to_closed() {
        let mut b = CircuitBreaker::new();
        let t = now();
        for _ in 0..TRIP_THRESHOLD {
            b.note_failure("http://x", t);
        }
        let later = t + INITIAL_BACKOFF + Duration::from_secs(1);
        assert_eq!(b.check("http://x", later), BreakerDecision::Allow);
        b.note_success("http://x");
        // Should immediately allow new requests.
        assert_eq!(b.check("http://x", later), BreakerDecision::Allow);
        assert!(!b.is_open("http://x"));
    }

    /// Probe-timeout safety net. If the caller never reports back, the
    /// breaker mustn't sit forever in HalfOpen — it should fall back
    /// to Open with extended backoff after HALF_OPEN_PROBE_TIMEOUT.
    #[test]
    fn half_open_probe_timeout_reopens() {
        let mut b = CircuitBreaker::new();
        let t = now();
        for _ in 0..TRIP_THRESHOLD {
            b.note_failure("http://x", t);
        }
        let later = t + INITIAL_BACKOFF + Duration::from_secs(1);
        // Issue probe but never resolve.
        assert_eq!(b.check("http://x", later), BreakerDecision::Allow);

        // Way past the probe timeout — next check should reset state to
        // Open and return Blocked.
        let far_later = later + HALF_OPEN_PROBE_TIMEOUT
            + Duration::from_secs(1);
        match b.check("http://x", far_later) {
            BreakerDecision::Blocked { .. } => {}
            other => panic!(
                "after probe timeout, expected Blocked but got {:?}",
                other,
            ),
        }
        // The very next check ALSO doesn't get probed (since the new
        // backoff hasn't elapsed). True "fast-fail" recovery.
        match b.check("http://x", far_later + Duration::from_millis(10)) {
            BreakerDecision::Blocked { .. } => {}
            other => panic!("expected Blocked, got {:?}", other),
        }
    }
}
