//! pace — keep GPU submissions short enough that the compositor still gets
//! its frame, and report what they actually cost.
//!
//! This is the real cause of the stutter, and it is not CPU scheduling.
//! A GPU is not preemptible the way a CPU is: on most Linux drivers a
//! submitted command buffer runs to completion, and while it does, the
//! COMPOSITOR cannot get its own work through. So one long dispatch — a
//! whole-frame upscale, or a scene with every mesh in one submission —
//! stalls the entire desktop, not just the app that issued it. Nicing the
//! process does nothing, because the queue is not scheduled by the CPU
//! scheduler. That is why it shows up in dandere2x and in logscope-egui
//! alike: same shape, two apps.
//!
//! The fix is to submit LESS AT A TIME, and the only way to know how much
//! is to measure. This is a controller: feed it how long the last
//! submission took, ask it how much work to put in the next one.
//!
//! # Why it backs off fast and grows slow
//!
//! The asymmetry is deliberate and is the same one TCP uses. An overrun has
//! ALREADY cost a dropped frame — the damage is done and the only question
//! is whether it happens again, so the response is a hard multiplicative
//! cut. Growth is the opposite: nothing is wrong, so there is no hurry, and
//! creeping up additively means a single overrun is never followed by a
//! second one caused by the recovery.
//!
//! Pure: no GPU, no clock. The caller times its own submissions (a
//! timestamp query, or wall-clock around `poll`) and feeds the number in,
//! which is also what makes the policy testable without a GPU.

/// One compositor frame at 60 Hz.
pub const FRAME_MS: f32 = 16.7;

/// The share of a frame our submissions may take. The rest is the
/// compositor's, plus headroom — aiming at a whole frame means every
/// scheduling jitter is a dropped one.
pub const TARGET_FRACTION: f32 = 0.6;

/// Cut to this fraction of the current chunk when a submission overruns.
pub const BACKOFF: f32 = 0.5;
/// Grow by this fraction of the current chunk when it does not.
pub const GROWTH: f32 = 0.12;

/// Rolling statistics, for a metrics pane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// Most recent submission, milliseconds.
    pub last_ms: f32,
    /// Exponential moving average.
    pub avg_ms: f32,
    /// Worst submission seen since the last reset.
    pub peak_ms: f32,
    /// Submissions that exceeded the target — each one is a frame the
    /// compositor probably missed, which is the number a user actually
    /// feels.
    pub overruns: u32,
    /// Submissions total.
    pub submissions: u32,
    /// Work units in the next submission.
    pub chunk: u32,
}

impl Metrics {
    /// Share of submissions that overran. The one number worth putting in a
    /// pane if there is only room for one.
    pub fn overrun_rate(&self) -> f32 {
        if self.submissions == 0 { return 0.0; }
        self.overruns as f32 / self.submissions as f32
    }

    /// A line for the pane.
    pub fn summary(&self) -> String {
        format!(
            "gpu {:.1} ms avg · {:.1} peak · chunk {} · {:.0}% over budget",
            self.avg_ms, self.peak_ms, self.chunk, self.overrun_rate() * 100.0
        )
    }
}

/// The controller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pacer {
    target_ms: f32,
    chunk: f32,
    min_chunk: f32,
    max_chunk: f32,
    m: Metrics,
}

impl Pacer {
    /// `max_chunk` is the whole job in one go — what an unpaced app does
    /// today. Starting there and letting the first overrun cut it means a
    /// fast machine never pays for the caution.
    pub fn new(max_chunk: u32) -> Pacer {
        let max = max_chunk.max(1) as f32;
        Pacer {
            target_ms: FRAME_MS * TARGET_FRACTION,
            chunk: max,
            min_chunk: 1.0,
            max_chunk: max,
            m: Metrics {
                last_ms: 0.0, avg_ms: 0.0, peak_ms: 0.0,
                overruns: 0, submissions: 0, chunk: max_chunk.max(1),
            },
        }
    }

    /// Override the per-submission target (a 120 Hz display halves it).
    pub fn with_target_ms(mut self, ms: f32) -> Pacer {
        self.target_ms = ms.max(0.5);
        self
    }

    /// How many work units to submit next. Never zero — a controller that
    /// can starve its own job is worse than the stutter.
    pub fn chunk(&self) -> u32 {
        self.chunk.round().clamp(self.min_chunk, self.max_chunk) as u32
    }

    pub fn metrics(&self) -> Metrics { self.m }

    /// Report how long the last submission took.
    pub fn record(&mut self, elapsed_ms: f32) {
        let e = if elapsed_ms.is_finite() { elapsed_ms.max(0.0) } else { return };
        self.m.last_ms = e;
        self.m.peak_ms = self.m.peak_ms.max(e);
        self.m.avg_ms = if self.m.submissions == 0 { e } else { self.m.avg_ms * 0.8 + e * 0.2 };
        self.m.submissions = self.m.submissions.saturating_add(1);

        if e > self.target_ms {
            self.m.overruns = self.m.overruns.saturating_add(1);
            self.chunk = (self.chunk * BACKOFF).max(self.min_chunk);
        } else {
            self.chunk = (self.chunk + self.chunk * GROWTH).min(self.max_chunk);
        }
        self.m.chunk = self.chunk();
    }

    /// Clear the counters but keep the learned chunk size — a pane's "reset
    /// stats" should not throw away the calibration.
    pub fn reset_stats(&mut self) {
        self.m = Metrics {
            last_ms: 0.0, avg_ms: 0.0, peak_ms: 0.0,
            overruns: 0, submissions: 0, chunk: self.chunk(),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_submission_is_cut_hard_and_a_short_one_grows_gently() {
        let mut p = Pacer::new(1000);
        let before = p.chunk();
        p.record(40.0); // way over a frame
        let after_overrun = p.chunk();
        assert!(after_overrun <= before / 2 + 1, "{before} -> {after_overrun}");
        // Recovery is patient: one good submission must not undo the cut.
        p.record(2.0);
        assert!(p.chunk() < before, "grew straight back: {}", p.chunk());
        assert!(p.chunk() > after_overrun, "did not grow at all");
    }

    #[test]
    fn it_settles_instead_of_oscillating_when_cost_tracks_chunk_size() {
        // Model a GPU where time is proportional to work, calibrated so the
        // full chunk takes 40 ms — three times the target.
        let mut p = Pacer::new(1000);
        let ms_per_unit = 40.0 / 1000.0;
        for _ in 0..200 {
            let c = p.chunk() as f32;
            p.record(c * ms_per_unit);
        }
        let settled = p.chunk() as f32 * ms_per_unit;
        assert!(settled <= FRAME_MS * TARGET_FRACTION + 1.0,
                "settled at {settled:.1} ms, target {:.1}", FRAME_MS * TARGET_FRACTION);
        // And it is still doing useful work, not collapsed to nothing.
        assert!(p.chunk() > 1, "collapsed to {}", p.chunk());
        // Once settled, overruns should be rare rather than every other frame.
        p.reset_stats();
        for _ in 0..50 {
            let c = p.chunk() as f32;
            p.record(c * ms_per_unit);
        }
        assert!(p.metrics().overrun_rate() < 0.25, "{}", p.metrics().summary());
    }

    #[test]
    fn the_chunk_never_reaches_zero_however_bad_the_gpu_is() {
        // A controller that can starve its own job is worse than the stutter.
        let mut p = Pacer::new(64);
        for _ in 0..200 { p.record(9999.0); }
        assert!(p.chunk() >= 1, "starved to {}", p.chunk());
        assert_eq!(p.metrics().overruns, 200);
        assert!((p.metrics().overrun_rate() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_fast_machine_never_pays_for_the_caution() {
        // Starting at the full chunk means an app that was never stuttering
        // keeps its throughput and never learns a smaller size.
        let mut p = Pacer::new(500);
        for _ in 0..50 { p.record(1.0); }
        assert_eq!(p.chunk(), 500);
        assert_eq!(p.metrics().overruns, 0);
    }

    #[test]
    fn the_pane_gets_numbers_a_person_can_act_on() {
        let mut p = Pacer::new(100);
        p.record(5.0);
        p.record(30.0);
        let m = p.metrics();
        assert_eq!(m.peak_ms, 30.0);
        assert_eq!(m.submissions, 2);
        assert_eq!(m.overruns, 1);
        assert!((m.overrun_rate() - 0.5).abs() < 1e-6);
        let s = m.summary();
        assert!(s.contains("gpu") && s.contains("peak") && s.contains("chunk"), "{s}");
        // Resetting the counters keeps the calibration — otherwise pressing
        // "reset" in a pane makes the app stutter again while it relearns.
        let learned = p.chunk();
        p.reset_stats();
        assert_eq!(p.chunk(), learned);
        assert_eq!(p.metrics().submissions, 0);
    }

    #[test]
    fn a_higher_refresh_display_gets_a_tighter_target() {
        let mut p120 = Pacer::new(1000).with_target_ms(FRAME_MS * TARGET_FRACTION / 2.0);
        let mut p60 = Pacer::new(1000);
        for _ in 0..100 {
            p120.record(p120.chunk() as f32 * 0.04);
            p60.record(p60.chunk() as f32 * 0.04);
        }
        assert!(p120.chunk() < p60.chunk(), "120Hz {} vs 60Hz {}", p120.chunk(), p60.chunk());
        // Nonsense targets are floored rather than dividing by ~zero.
        assert!(Pacer::new(10).with_target_ms(-3.0).chunk() >= 1);
    }
}
