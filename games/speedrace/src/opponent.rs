//! opponent — who you are actually racing.
//!
//! ## Why not a live LLM
//!
//! The obvious build is "stream tokens from the model and see who finishes
//! first". It does not work as a game, for three reasons that are properties
//! of the matchup rather than of any particular model:
//!
//! 1. **It is not close.** A model emits a correct two-pointer solution in a
//!    few seconds. A human types it in ninety. There is no race; there is a
//!    result you watch arrive. A game needs the outcome in doubt at the
//!    halfway mark.
//! 2. **It is not reproducible.** Same problem, same you, different day, and
//!    the opponent is different because sampling and latency moved. You
//!    cannot tell whether you improved.
//! 3. **You end up racing the network.** On a slow link you win by default;
//!    on a fast one you lose by default. The variable that decides the race
//!    is the one variable that has nothing to do with coding.
//!
//! ## What this is instead
//!
//! A **pace car**: a recorded solve, replayed against the clock at a speed
//! calibrated to *your* history on problems of that difficulty. The model
//! still writes the solution — but offline, once, into a [`Recording`]. At
//! race time the opponent is a deterministic function of elapsed time.
//!
//! That buys everything the live version loses: the finish is close by
//! construction ([`Calibration`] targets a chosen win rate), the race is
//! reproducible, it runs on a plane, and the opponent's pace is a number you
//! can see and argue with rather than a black box.
//!
//! [`Live`] still exists behind the same trait, for when you want to be
//! humbled. It is not the default.

use serde::{Deserialize, Serialize};

/// Milliseconds since the race started.
pub type Ms = u64;

/// What the opponent has produced at a given instant. The shell renders this
/// as a ghost pane next to your buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    /// Characters of the reference solution emitted so far.
    pub chars: usize,
    /// Fraction of the whole solve, 0.0..=1.0.
    pub frac: f32,
    /// True once the opponent has finished and its tests would pass.
    pub done: bool,
}

/// Anything that can play the other side of a race.
///
/// Deliberately a pure function of elapsed time: no threads, no IO, no
/// interior mutability. That is what makes a race replayable and the engine
/// testable without a clock.
pub trait Opponent {
    fn progress_at(&self, t: Ms) -> Progress;
    /// Total time the opponent will take. `None` for an unbounded opponent.
    fn finish_time(&self) -> Option<Ms>;
    fn label(&self) -> &str;
}

/// A solve captured offline: the final source plus the shape of how it
/// arrived. Segment boundaries let the ghost pause where the model paused
/// (thinking before the tricky loop), which reads far better than a uniform
/// crawl.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recording {
    pub problem_id: String,
    pub model: String,
    pub source: String,
    /// (cumulative_chars, cumulative_ms) samples, ascending in both.
    pub samples: Vec<(usize, Ms)>,
}

impl Recording {
    /// A recording with no intermediate samples: straight-line typing.
    pub fn flat(problem_id: &str, model: &str, source: &str, total_ms: Ms) -> Self {
        let n = source.chars().count();
        Self {
            problem_id: problem_id.into(),
            model: model.into(),
            source: source.into(),
            samples: vec![(0, 0), (n, total_ms)],
        }
    }

    pub fn total_chars(&self) -> usize {
        self.source.chars().count()
    }

    pub fn native_ms(&self) -> Ms {
        self.samples.last().map(|s| s.1).unwrap_or(0)
    }

    /// Linear interpolation between samples. Held flat past the end.
    fn chars_at(&self, t: Ms) -> usize {
        let s = &self.samples;
        if s.is_empty() {
            return 0;
        }
        if t >= s[s.len() - 1].1 {
            return s[s.len() - 1].0;
        }
        for w in s.windows(2) {
            let ((c0, t0), (c1, t1)) = (w[0], w[1]);
            if t < t1 {
                if t1 == t0 {
                    return c1;
                }
                let f = (t - t0) as f64 / (t1 - t0) as f64;
                return c0 + ((c1 - c0) as f64 * f) as usize;
            }
        }
        s[s.len() - 1].0
    }
}

/// How hard the pace car tries.
///
/// `speed` scales the recording's own timeline: 1.0 replays it as recorded,
/// 0.5 takes twice as long. [`Calibration`] picks it from your history so the
/// race lands close.
#[derive(Clone, Debug)]
pub struct PaceCar {
    rec: Recording,
    speed: f64,
    label: String,
}

impl PaceCar {
    pub fn new(rec: Recording, speed: f64) -> Self {
        let label = format!("{} @{:.2}x", rec.model, speed);
        Self { rec, speed: speed.max(0.01), label }
    }

    /// Replay so the car finishes in exactly `target`.
    pub fn finishing_in(rec: Recording, target: Ms) -> Self {
        let native = rec.native_ms().max(1);
        let speed = native as f64 / target.max(1) as f64;
        Self::new(rec, speed)
    }

    pub fn source(&self) -> &str {
        &self.rec.source
    }
}

impl Opponent for PaceCar {
    fn progress_at(&self, t: Ms) -> Progress {
        let scaled = (t as f64 * self.speed) as Ms;
        let chars = self.rec.chars_at(scaled);
        let total = self.rec.total_chars().max(1);
        Progress {
            chars,
            frac: (chars as f32 / total as f32).min(1.0),
            done: chars >= total,
        }
    }

    fn finish_time(&self) -> Option<Ms> {
        Some((self.rec.native_ms() as f64 / self.speed) as Ms)
    }

    fn label(&self) -> &str {
        &self.label
    }
}

/// Picks a pace-car speed from your own history so races stay close.
///
/// The rule: aim the car at the time you would hit if you performed at your
/// `target_percentile` on this difficulty. Beat your usual self and you win;
/// coast and you lose. A fixed difficulty setting cannot do this, because
/// "hard" means something different in week one and week twenty.
#[derive(Clone, Debug)]
pub struct Calibration {
    /// Your finish times on this difficulty, in ms. Unsorted is fine.
    pub history: Vec<Ms>,
    /// 0.0..=1.0. 0.5 = the car finishes at your median, so you win half.
    pub target_percentile: f64,
    /// Used when history is too thin to mean anything.
    pub fallback_ms: Ms,
}

impl Calibration {
    pub fn new(fallback_ms: Ms) -> Self {
        Self { history: Vec::new(), target_percentile: 0.5, fallback_ms }
    }

    /// The time to aim the pace car at.
    pub fn target_ms(&self) -> Ms {
        // Under three samples, percentile is noise dressed as a number.
        if self.history.len() < 3 {
            return self.fallback_ms;
        }
        let mut h = self.history.clone();
        h.sort_unstable();
        let p = self.target_percentile.clamp(0.0, 1.0);
        let idx = ((h.len() - 1) as f64 * p).round() as usize;
        h[idx]
    }

    pub fn car(&self, rec: Recording) -> PaceCar {
        PaceCar::finishing_in(rec, self.target_ms())
    }
}

/// The honest opponent: a real model call, for when you want the humbling.
/// Held behind the same trait so the engine cannot tell the difference.
#[derive(Clone, Debug)]
pub struct Live {
    /// Filled in by the shell as tokens arrive.
    pub observed: Vec<(usize, Ms)>,
    pub total_chars_hint: usize,
    pub finished: Option<Ms>,
}

impl Opponent for Live {
    fn progress_at(&self, t: Ms) -> Progress {
        let chars = self
            .observed
            .iter()
            .rev()
            .find(|(_, ts)| *ts <= t)
            .map(|(c, _)| *c)
            .unwrap_or(0);
        let total = self.total_chars_hint.max(1);
        Progress {
            chars,
            frac: (chars as f32 / total as f32).min(1.0),
            done: self.finished.map(|f| t >= f).unwrap_or(false),
        }
    }
    fn finish_time(&self) -> Option<Ms> {
        self.finished
    }
    fn label(&self) -> &str {
        "live model"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec() -> Recording {
        // 100 chars, 10s, with a deliberate stall from 3s to 7s
        Recording {
            problem_id: "p".into(),
            model: "test".into(),
            source: "x".repeat(100),
            samples: vec![(0, 0), (40, 3000), (45, 7000), (100, 10_000)],
        }
    }

    #[test]
    fn interpolates_between_samples() {
        let c = PaceCar::new(rec(), 1.0);
        assert_eq!(c.progress_at(0).chars, 0);
        assert_eq!(c.progress_at(1500).chars, 20); // halfway to the first sample
        assert_eq!(c.progress_at(3000).chars, 40);
        assert_eq!(c.progress_at(10_000).chars, 100);
    }

    #[test]
    fn stall_is_preserved() {
        // The model paused; the ghost must pause too, or it reads as a
        // uniform crawl and loses all its character.
        let c = PaceCar::new(rec(), 1.0);
        let a = c.progress_at(3500).chars;
        let b = c.progress_at(6500).chars;
        assert!(b - a < 6, "stall flattened: {a} -> {b}");
    }

    #[test]
    fn progress_is_monotonic() {
        let c = PaceCar::new(rec(), 1.0);
        let mut last = 0;
        for t in (0..12_000).step_by(100) {
            let p = c.progress_at(t).chars;
            assert!(p >= last, "went backwards at {t}: {last} -> {p}");
            last = p;
        }
    }

    #[test]
    fn holds_flat_past_the_end() {
        let c = PaceCar::new(rec(), 1.0);
        assert_eq!(c.progress_at(99_999).chars, 100);
        assert!(c.progress_at(99_999).done);
    }

    #[test]
    fn finishing_in_hits_its_target() {
        let c = PaceCar::finishing_in(rec(), 30_000);
        assert!(!c.progress_at(29_000).done);
        assert!(c.progress_at(30_000).done);
        let f = c.finish_time().unwrap();
        assert!((f as i64 - 30_000).abs() < 50, "finish {f}");
    }

    #[test]
    fn calibration_ignores_thin_history() {
        let mut c = Calibration::new(60_000);
        assert_eq!(c.target_ms(), 60_000);
        c.history = vec![10_000, 20_000];
        assert_eq!(c.target_ms(), 60_000, "2 samples is not a distribution");
    }

    #[test]
    fn calibration_tracks_the_percentile() {
        let mut c = Calibration::new(60_000);
        c.history = vec![50_000, 10_000, 30_000, 40_000, 20_000];
        c.target_percentile = 0.5;
        assert_eq!(c.target_ms(), 30_000, "median");
        c.target_percentile = 0.0;
        assert_eq!(c.target_ms(), 10_000, "your best: car is brutal");
        c.target_percentile = 1.0;
        assert_eq!(c.target_ms(), 50_000, "your worst: car is generous");
    }

    #[test]
    fn calibration_follows_you_as_you_improve() {
        // The point of calibrating off history rather than a difficulty knob.
        let slow = Calibration { history: vec![90_000, 88_000, 92_000], target_percentile: 0.5, fallback_ms: 0 };
        let fast = Calibration { history: vec![30_000, 28_000, 32_000], target_percentile: 0.5, fallback_ms: 0 };
        assert!(slow.car(rec()).finish_time() > fast.car(rec()).finish_time());
    }

    #[test]
    fn live_opponent_reports_last_observation() {
        let l = Live { observed: vec![(0, 0), (50, 2000), (120, 5000)], total_chars_hint: 120, finished: Some(5000) };
        assert_eq!(l.progress_at(1000).chars, 0);
        assert_eq!(l.progress_at(2500).chars, 50);
        assert!(l.progress_at(5000).done);
    }
}
