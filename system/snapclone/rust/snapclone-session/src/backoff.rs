//! Exponential backoff with jitter for reconnection attempts.
//!
//! Jitter prevents thundering herd when many peers reconnect simultaneously
//! (e.g., after network comes back up after outage).

use std::time::Duration;

/// Backoff schedule: [1s, 2s, 4s, 8s, 16s, 30s, 30s, ...]
pub struct RetryBackoff {
    attempt:  u32,
    base_ms:  u64,
    cap_ms:   u64,
    jitter:   bool,
}

impl RetryBackoff {
    /// Default: exponential from 1s, capped at 30s, with jitter.
    pub fn default_session() -> Self {
        Self { attempt: 0, base_ms: 1000, cap_ms: 30_000, jitter: true }
    }

    /// Relay-specific: start at 2s (relay connections take longer to establish).
    pub fn relay() -> Self {
        Self { attempt: 0, base_ms: 2000, cap_ms: 8_000, jitter: false }
    }

    /// Next delay to wait before retrying.
    pub fn next(&mut self) -> Duration {
        let exp = self.base_ms.saturating_mul(1 << self.attempt.min(5));
        let capped = exp.min(self.cap_ms);
        self.attempt += 1;
        if self.jitter {
            // ±25% jitter using simple LCG
            let jitter_range = capped / 4;
            let jitter = simple_rand() % (jitter_range * 2 + 1);
            Duration::from_millis(capped - jitter_range + jitter)
        } else {
            Duration::from_millis(capped)
        }
    }

    pub fn reset(&mut self) { self.attempt = 0; }
    pub fn attempt(&self) -> u32 { self.attempt }
}

fn simple_rand() -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    std::time::Instant::now().hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exponential_growth() {
        let mut b = RetryBackoff { attempt:0, base_ms:1000, cap_ms:30000, jitter:false };
        let d0 = b.next().as_millis();
        let d1 = b.next().as_millis();
        let d2 = b.next().as_millis();
        assert_eq!(d0, 1000);
        assert_eq!(d1, 2000);
        assert_eq!(d2, 4000);
    }
    #[test]
    fn caps_at_max() {
        let mut b = RetryBackoff { attempt:0, base_ms:1000, cap_ms:5000, jitter:false };
        for _ in 0..10 { b.next(); }
        assert!(b.next().as_millis() <= 5000);
    }
    #[test]
    fn reset_restarts() {
        let mut b = RetryBackoff { attempt:0, base_ms:1000, cap_ms:30000, jitter:false };
        b.next(); b.next(); b.reset();
        assert_eq!(b.next().as_millis(), 1000);
    }
}
