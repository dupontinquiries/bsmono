//! time.rs — wall-clock (UNIX epoch) and monotonic time helpers.
//!
//! The epoch helpers are the `SystemTime::now().duration_since(UNIX_EPOCH)…
//! .unwrap_or(0)` pattern (clock-before-epoch → 0) that was reimplemented as
//! `now_secs` / `now_ms` / `now_nanos` across many crates. [`Stopwatch`] wraps
//! `Instant` for interval timing (the monotonic, never-goes-backward clock you
//! want for measuring durations, as opposed to the wall clock above).

use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Seconds since the UNIX epoch (0 if the clock is before 1970).
pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Milliseconds since the UNIX epoch (0 if the clock is before 1970).
pub fn now_millis() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Nanoseconds since the UNIX epoch (0 if the clock is before 1970). `u128`
/// because nanoseconds overflow `u64` in the year 2554.
pub fn now_nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

/// A monotonic stopwatch for measuring elapsed time. Uses `Instant`, so it is
/// immune to wall-clock adjustments (NTP steps, DST) — unlike the epoch helpers.
#[derive(Clone, Copy, Debug)]
pub struct Stopwatch {
    start: Instant,
}

impl Stopwatch {
    /// Start timing now.
    pub fn start() -> Self {
        Self { start: Instant::now() }
    }
    /// Elapsed whole milliseconds since `start`/`reset`.
    pub fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
    /// Elapsed seconds since `start`/`reset`, fractional.
    pub fn elapsed_secs(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
    /// Restart the clock from now.
    pub fn reset(&mut self) {
        self.start = Instant::now();
    }
}

impl Default for Stopwatch {
    fn default() -> Self {
        Self::start()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_helpers_are_plausible_and_consistent() {
        let s = now_secs();
        assert!(s > 1_600_000_000, "after 2020-09; got {s}");
        // millis and secs agree to within a couple seconds (calls aren't atomic)
        let ms = now_millis();
        assert!((ms / 1000).abs_diff(s) <= 2, "ms {ms} vs s {s}");
        assert!(now_nanos() > (s as u128) * 1_000_000_000 - 2_000_000_000);
    }

    #[test]
    fn now_millis_is_monotonic_nondecreasing() {
        let a = now_millis();
        let b = now_millis();
        assert!(b >= a, "wall clock went backwards within a call pair: {a} then {b}");
    }

    #[test]
    fn stopwatch_measures_a_nonnegative_interval() {
        let sw = Stopwatch::start();
        // a little busy-work so elapsed is well-defined
        let mut acc = 0u64;
        for i in 0..200_000u64 {
            acc = acc.wrapping_add(i);
        }
        assert!(acc > 0);
        let _ = sw.elapsed_ms(); // monotonic, never panics
        assert!(sw.elapsed_secs() >= 0.0);
        let mut sw2 = sw;
        sw2.reset();
        assert!(sw2.elapsed_secs() >= 0.0);
    }
}
