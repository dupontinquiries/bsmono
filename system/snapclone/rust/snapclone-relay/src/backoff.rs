//! Exponential backoff for relay reconnection.
//! Matches the C++ kMaxReconnects / doubling delay pattern.

use std::time::Duration;

/// Exponential backoff state.
/// Usage: `while let Some(delay) = backoff.next() { sleep(delay); ... }`
pub struct ExponentialBackoff {
    attempt:  u32,
    max:      u32,
    base_ms:  u64,
    max_ms:   u64,
}

impl ExponentialBackoff {
    /// Create with C++ defaults: base=2s, max=3 attempts, cap=8s.
    pub fn default_relay() -> Self {
        Self { attempt: 0, max: 3, base_ms: 2000, max_ms: 8000 }
    }

    /// Next delay, or None if max attempts reached.
    pub fn next(&mut self) -> Option<Duration> {
        if self.attempt >= self.max { return None; }
        let ms = (self.base_ms * (1 << self.attempt)).min(self.max_ms);
        self.attempt += 1;
        Some(Duration::from_millis(ms))
    }

    /// Reset the counter (call on successful connection).
    pub fn reset(&mut self) { self.attempt = 0; }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delays_double() {
        let mut b = ExponentialBackoff::default_relay();
        assert_eq!(b.next().unwrap().as_millis(), 2000);
        assert_eq!(b.next().unwrap().as_millis(), 4000);
        assert_eq!(b.next().unwrap().as_millis(), 8000);
        assert!(b.next().is_none());
    }
    #[test]
    fn reset_restarts() {
        let mut b = ExponentialBackoff::default_relay();
        b.next(); b.next(); b.reset();
        assert_eq!(b.next().unwrap().as_millis(), 2000);
    }
}
