//! stall -- when did the shell's own loop fall behind, and was it our work or the scheduler?
//!
//! Two different problems look the same from outside (a swipe that stutters):
//! * **slow work**: the loop body ran for a long time after waking (a heavy frame, a blocking call);
//! * **late wake**: the loop asked to sleep N ms and was not running again until well after N ms
//!   (the process was runnable but got no CPU: something else had the cores).
//! The first is ours to fix in code; the second is what `CPUWeight` and the per-app policy are for. A line says which,
//! with the numbers, so a day of the phone's journal answers the question instead of a guess. Pure and clock-free
//! (callers pass durations), rate-limited so a bad minute cannot flood the journal.
use std::time::Duration;

/// A loop body longer than this after waking is slow work (a 90 Hz panel frame is 11 ms).
pub const SLOW_WORK_MS: u128 = 20;
/// A sleep that overran its timeout by more than this is a late wake.
pub const LATE_WAKE_MS: u128 = 12;
/// At most one line per this long (the count of suppressed ones is added to the next line).
pub const MIN_GAP_MS: u128 = 500;

#[derive(Debug, Default)]
pub struct StallLog {
    since_last_line: Option<u128>,
    suppressed: u32,
}

impl StallLog {
    /// `clock_ms` is any monotonic millisecond reading.
    fn emit(&mut self, clock_ms: u128, what: String) -> Option<String> {
        if self.since_last_line.is_some_and(|t| clock_ms.saturating_sub(t) < MIN_GAP_MS) {
            self.suppressed += 1;
            return None;
        }
        self.since_last_line = Some(clock_ms);
        let extra = if self.suppressed > 0 { format!(" (+{} more since the last line)", self.suppressed) } else { String::new() };
        self.suppressed = 0;
        Some(format!("stall: {what}{extra}"))
    }

    /// The loop body took `work` after the wake. `ctx` says what the shell was doing.
    pub fn on_work(&mut self, clock_ms: u128, work: Duration, ctx: &str) -> Option<String> {
        (work.as_millis() >= SLOW_WORK_MS).then(|| ()).and_then(|_| self.emit(clock_ms, format!("slow work {} ms [{ctx}]", work.as_millis())))
    }

    /// The loop slept `slept` after asking for `timeout_ms`. Input wakes it early, so only an overrun counts.
    pub fn on_wake(&mut self, clock_ms: u128, slept: Duration, timeout_ms: i32, ctx: &str) -> Option<String> {
        if timeout_ms <= 0 { return None; }
        let over = slept.as_millis().saturating_sub(timeout_ms as u128);
        (over > LATE_WAKE_MS).then(|| ()).and_then(|_| self.emit(clock_ms, format!("late wake +{over} ms over a {timeout_ms} ms sleep [{ctx}]")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ms(v: u64) -> Duration { Duration::from_millis(v) }

    #[test]
    fn slow_work_and_late_wakes_are_told_apart_and_ordinary_frames_say_nothing() {
        let mut s = StallLog::default();
        assert_eq!(s.on_work(0, ms(8), "x"), None, "a normal frame");
        assert_eq!(s.on_work(0, ms(19), "x"), None, "just under the limit");
        assert!(s.on_work(1_000, ms(45), "swipe").unwrap().contains("slow work 45 ms [swipe]"));
        assert_eq!(s.on_wake(2_000, ms(16), 16, "x"), None, "slept what it asked");
        assert_eq!(s.on_wake(2_000, ms(10), 16, "x"), None, "input woke it early");
        assert_eq!(s.on_wake(2_000, ms(27), 16, "x"), None, "11 ms over is within tolerance");
        let l = s.on_wake(3_000, ms(60), 16, "busy").unwrap();
        assert!(l.contains("late wake +44 ms over a 16 ms sleep [busy]"), "{l}");
        assert_eq!(s.on_wake(9_000, ms(500), -1, "x"), None, "no timeout was requested");
    }

    #[test]
    fn a_bad_minute_cannot_flood_the_journal_and_the_count_is_not_lost() {
        let mut s = StallLog::default();
        assert!(s.on_work(0, ms(50), "a").is_some());
        for t in [10, 100, 200, 499] { assert_eq!(s.on_work(t, ms(50), "a"), None, "inside the gap at {t}"); }
        let l = s.on_work(600, ms(50), "a").unwrap();
        assert!(l.contains("+4 more"), "{l}");
        assert!(!s.on_work(1_200, ms(50), "a").unwrap().contains("more"), "the count resets once reported");
    }
}
