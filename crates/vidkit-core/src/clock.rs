//! Timing.
//!
//! Two things live here: a clock that can be driven by audio, and the
//! show/hold/drop decision. Both are pure and testable, which matters because
//! A/V sync bugs are miserable to debug against real hardware.

use crate::traits::Clock;
use std::time::{Duration, Instant};

/// Monotonic clock with pause and rate. Used directly when there is no audio,
/// and used as the fallback while the audio device is still priming.
pub struct MonotonicClock {
    origin: Instant,
    /// Media time accumulated before the current running span.
    banked: Duration,
    rate: f64,
    paused: bool,
}

impl MonotonicClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            banked: Duration::ZERO,
            rate: 1.0,
            paused: true,
        }
    }

    /// Jump the timeline, e.g. after a seek.
    pub fn set_position(&mut self, pos: Duration) {
        self.banked = pos;
        self.origin = Instant::now();
    }

    fn elapsed_media(&self) -> Duration {
        if self.paused {
            return Duration::ZERO;
        }
        let wall = self.origin.elapsed();
        Duration::from_secs_f64(wall.as_secs_f64() * self.rate)
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.banked + self.elapsed_media()
    }

    fn set_rate(&mut self, rate: f64) {
        // Fold elapsed time in at the old rate before changing it, or a rate
        // change mid-playback retroactively rewrites history.
        self.banked += self.elapsed_media();
        self.origin = Instant::now();
        self.rate = rate.clamp(0.25, 4.0);
    }

    fn pause(&mut self) {
        if !self.paused {
            self.banked += self.elapsed_media();
            self.paused = true;
        }
    }

    fn resume(&mut self) {
        if self.paused {
            self.origin = Instant::now();
            self.paused = false;
        }
    }

    fn is_paused(&self) -> bool {
        self.paused
    }
}

/// What to do with the frame at the head of the queue.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Action {
    /// Show it now.
    Show,
    /// Not yet — sleep this long, then ask again.
    Hold(Duration),
    /// Too late to be useful. Discard and ask about the next one.
    Drop,
}

/// Sync policy.
///
/// `slack` is how early a frame may be shown without anyone noticing — set it
/// to about half a display refresh interval. `late_limit` is how far behind a
/// frame may be before showing it is worse than dropping it.
#[derive(Copy, Clone, Debug)]
pub struct SyncPolicy {
    pub slack: Duration,
    pub late_limit: Duration,
}

impl SyncPolicy {
    /// Derive from the display refresh rate. At 60 Hz that is ~8.3 ms slack.
    pub fn for_refresh_hz(hz: u32) -> Self {
        let interval = Duration::from_secs_f64(1.0 / hz.max(1) as f64);
        Self {
            slack: interval / 2,
            late_limit: interval * 3,
        }
    }

    pub fn decide(&self, frame_pts: Duration, clock_now: Duration) -> Action {
        if frame_pts > clock_now {
            let ahead = frame_pts - clock_now;
            if ahead <= self.slack {
                Action::Show
            } else {
                Action::Hold(ahead - self.slack)
            }
        } else {
            let behind = clock_now - frame_pts;
            if behind > self.late_limit {
                Action::Drop
            } else {
                Action::Show
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn slack_tracks_refresh_rate() {
        let p60 = SyncPolicy::for_refresh_hz(60);
        let p90 = SyncPolicy::for_refresh_hz(90);
        assert!(p90.slack < p60.slack);
    }

    #[test]
    fn on_time_frame_shows() {
        let p = SyncPolicy::for_refresh_hz(60);
        assert_eq!(p.decide(ms(1000), ms(1000)), Action::Show);
    }

    #[test]
    fn slightly_early_frame_shows_rather_than_stuttering() {
        let p = SyncPolicy::for_refresh_hz(60); // slack ~8.3ms
        assert_eq!(p.decide(ms(1004), ms(1000)), Action::Show);
    }

    #[test]
    fn far_future_frame_yields_a_sleep_not_a_spin() {
        let p = SyncPolicy::for_refresh_hz(60);
        match p.decide(ms(1100), ms(1000)) {
            Action::Hold(d) => {
                assert!(d > ms(80) && d < ms(100), "unexpected hold {d:?}");
            }
            other => panic!("expected Hold, got {other:?}"),
        }
    }

    #[test]
    fn slightly_late_frame_still_shows() {
        let p = SyncPolicy::for_refresh_hz(60); // late_limit ~50ms
        assert_eq!(p.decide(ms(980), ms(1000)), Action::Show);
    }

    #[test]
    fn very_late_frame_drops() {
        let p = SyncPolicy::for_refresh_hz(60);
        assert_eq!(p.decide(ms(500), ms(1000)), Action::Drop);
    }

    #[test]
    fn paused_clock_does_not_advance() {
        let mut c = MonotonicClock::new();
        c.set_position(ms(5000));
        assert_eq!(c.now(), ms(5000));
        std::thread::sleep(ms(20));
        assert_eq!(c.now(), ms(5000));
    }

    #[test]
    fn resumed_clock_advances_then_holds_on_pause() {
        let mut c = MonotonicClock::new();
        c.set_position(ms(1000));
        c.resume();
        std::thread::sleep(ms(30));
        c.pause();
        let stopped = c.now();
        assert!(stopped >= ms(1025), "clock did not advance: {stopped:?}");
        std::thread::sleep(ms(20));
        assert_eq!(c.now(), stopped, "paused clock drifted");
    }

    #[test]
    fn rate_change_does_not_rewrite_elapsed_time() {
        let mut c = MonotonicClock::new();
        c.set_position(ms(1000));
        c.resume();
        std::thread::sleep(ms(30));
        let before = c.now();
        c.set_rate(2.0);
        let after = c.now();
        // Position must be continuous across the rate change.
        assert!(after >= before);
        assert!(after < before + ms(10), "position jumped: {before:?} -> {after:?}");
    }
}
