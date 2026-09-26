//! "AI in-between frames": when to show a synthesised frame. Pure; the
//! presenter does the synthesis (Nv12Gpu::show_between) and the app wires
//! the two together in `produce`.
//!
//! The decoder skips AV1's non-reference frames (every other frame in an
//! SVT-AV1 hierarchical GOP), so decoded frames arrive two native intervals
//! apart. Halfway between the frame on screen and the next queued one, the
//! presenter shows the frame it synthesises from those two; when the next
//! one falls due it is promoted, not uploaded again.
//!
//! Only a gap of about TWO native intervals gets a frame: one interval is
//! nothing skipped (a reference frame followed by a reference frame), and
//! anything longer is a seek, a stall or a different GOP shape, where one
//! midpoint frame would be the wrong number of frames.

use std::time::Duration;

/// Syntheses not counted towards the cost guard.
const WARM_UP: u32 = 3;

/// What was shown and what was synthesised, per playback.
#[derive(Debug, Default)]
pub struct Schedule {
    last: Option<Duration>,
    /// The next frame's pts that an in-between frame was shown (or refused)
    /// for: one attempt per gap.
    done_for: Option<Duration>,
    /// In-between frames shown, and attempts the presenter refused.
    pub shown: u64,
    pub refused: u64,
    /// The presenter's last reason for refusing, for the status line.
    pub last_refusal: Option<String>,
    /// Smoothed wall time of one synthesis on the UI thread, and how many
    /// were measured. Past half a frame interval it is costing more than it
    /// saves (and delaying real frames): the mode turns itself off.
    pub cost: Duration,
    measured: u32,
    pub too_slow: bool,
}

impl Schedule {
    /// A real frame at `pts` went on screen.
    pub fn shown_real(&mut self, pts: Duration) {
        self.last = Some(pts);
    }

    /// One synthesis took `took` of wall time on the UI thread (the
    /// driver's submission; on a software rasteriser, the whole render).
    pub fn record_cost(&mut self, took: Duration, native: Duration) {
        self.measured += 1;
        // The first few include shader compilation and target allocation:
        // left out entirely -- averaged in, one 200 ms compile still
        // dominated the sixth reading and tripped the guard (the test did).
        if self.measured <= WARM_UP {
            self.cost = took;
            return;
        }
        self.cost = if self.measured == WARM_UP + 1 { took } else { (self.cost * 3 + took) / 4 };
        if self.measured >= WARM_UP + 4 && self.cost > native / 2 {
            self.too_slow = true;
        }
    }

    /// Forget the last frame (a seek or a new file).
    pub fn reset(&mut self) {
        self.last = None;
        self.done_for = None;
    }

    /// The media time to show the in-between frame of the gap before the
    /// queued frame at `front`, when that gap wants one.
    pub fn midpoint(&self, front: Duration, native: Duration) -> Option<Duration> {
        let last = self.last?;
        if front <= last || self.done_for == Some(front) || native.is_zero() {
            return None;
        }
        let gap = front - last;
        let ratio = gap.as_secs_f64() / native.as_secs_f64();
        (1.5..2.5).contains(&ratio).then(|| last + gap / 2)
    }

    /// The attempt for the gap before `front` was made: `Ok` shown, `Err`
    /// refused. Either way, not again for this gap.
    pub fn attempted(&mut self, front: Duration, r: Result<(), String>) {
        self.done_for = Some(front);
        match r {
            Ok(()) => self.shown += 1,
            Err(e) => {
                self.refused += 1;
                self.last_refusal = Some(e);
            }
        }
    }
}

/// What to do at media time `at` (the time the drawn frame will be seen)
/// about the gap before the queued frame at `front`.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// Nothing to do for this gap.
    None,
    /// Too early: wake after this long.
    WakeIn(Duration),
    /// Show the in-between frame now.
    ShowNow,
}

pub fn step(s: &Schedule, front: Duration, native: Duration, at: Duration) -> Step {
    let Some(mid) = s.midpoint(front, native) else { return Step::None };
    if at >= front {
        // The next frame is due itself: the engine shows it.
        Step::None
    } else if at + Duration::from_millis(1) >= mid {
        Step::ShowNow
    } else {
        Step::WakeIn(mid - at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }
    const I60: Duration = Duration::from_micros(16_667);

    #[test]
    fn a_skipped_frame_gets_one_in_between_frame_at_its_time() {
        let mut s = Schedule::default();
        s.shown_real(ms(1000));
        let front = ms(1000) + I60 * 2;
        assert_eq!(s.midpoint(front, I60), Some(ms(1000) + I60));
        assert_eq!(step(&s, front, I60, ms(1005)), Step::WakeIn(ms(1000) + I60 - ms(1005)));
        assert_eq!(step(&s, front, I60, ms(1000) + I60), Step::ShowNow);
        s.attempted(front, Ok(()));
        assert_eq!(step(&s, front, I60, ms(1020)), Step::None, "once per gap");
        assert_eq!(s.shown, 1);
        // The next gap is fresh.
        s.shown_real(front);
        assert!(s.midpoint(front + I60 * 2, I60).is_some());
    }

    #[test]
    fn only_a_gap_of_two_intervals() {
        let mut s = Schedule::default();
        s.shown_real(ms(0));
        assert_eq!(s.midpoint(I60, I60), None, "nothing skipped");
        assert_eq!(s.midpoint(I60 * 3, I60), None, "two skipped: one midpoint would be wrong");
        assert_eq!(s.midpoint(ms(5000), I60), None, "a jump");
        assert_eq!(s.midpoint(ms(0), I60), None, "backwards (a seek)");
        assert!(s.midpoint(I60 * 2, I60).is_some());
        // Timestamp rounding either way still counts as two.
        assert!(s.midpoint(ms(33), I60).is_some() && s.midpoint(ms(34), I60).is_some());
        s.reset();
        assert_eq!(s.midpoint(I60 * 2, I60), None, "nothing shown since the reset");
    }

    #[test]
    fn too_slow_turns_it_off_but_not_on_warm_up() {
        let mut s = Schedule::default();
        for _ in 0..6 {
            s.record_cost(ms(40), I60);
        }
        assert!(!s.too_slow, "shader compile and allocation on the first ones");
        s.record_cost(ms(40), I60);
        assert!(s.too_slow, "40 ms for a 16.7 ms frame, seven times");
        let mut fast = Schedule::default();
        for k in 0..50 {
            fast.record_cost(if k == 0 { ms(200) } else { ms(2) }, I60);
        }
        assert!(!fast.too_slow && fast.cost < ms(4), "{:?}", fast.cost);
        // Right at the line: half an interval is still allowed.
        let mut edge = Schedule::default();
        (0..20).for_each(|_| edge.record_cost(I60 / 2, I60));
        assert!(!edge.too_slow);
    }

    #[test]
    fn a_refusal_is_not_retried_and_the_due_frame_wins() {
        let mut s = Schedule::default();
        s.shown_real(ms(0));
        let front = I60 * 2;
        assert_eq!(step(&s, front, I60, front), Step::None, "late: show the real frame");
        s.attempted(front, Err("dma-buf".into()));
        assert_eq!(step(&s, front, I60, I60), Step::None);
        assert_eq!((s.shown, s.refused, s.last_refusal.as_deref()), (0, 1, Some("dma-buf")));
    }
}
