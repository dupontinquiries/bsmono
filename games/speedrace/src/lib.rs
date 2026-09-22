//! speedrace-core — the headless race engine.
//!
//! You versus a pace car, on one hard problem, against a clock.
//!
//! ```text
//!   speedrace-core   Problem, Race, Opponent, Wheels, Verdict
//!     (serde only)   no egui, no threads, no clock, no network
//!
//!   speedrace-egui   the shell: aperture_core::Editor for your buffer,
//!                    aperture_core::syntax for highlighting, a ghost pane
//!                    driven by Opponent::progress_at(elapsed)
//!
//!   speedrace-judge  runs the test cases in a sandbox and returns a Verdict
//! ```
//!
//! The engine never reads a clock. Every entry point takes `now: Ms`, so a
//! whole race is a fold over timestamps and the test suite runs instantly —
//! the same bargain `aperture-core` makes by taking no IO.

pub mod opponent;
pub mod wheels;

use serde::{Deserialize, Serialize};

pub use opponent::{Calibration, Live, Ms, Opponent, PaceCar, Progress, Recording};
pub use wheels::{Refused, Wheel, Wheels};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Difficulty {
    Medium,
    Hard,
    Brutal,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestCase {
    pub input: String,
    pub expect: String,
    /// Shown before you start. The rest are held back, so you cannot
    /// pattern-match the suite instead of solving the problem.
    pub visible: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Problem {
    pub id: String,
    pub title: String,
    pub statement: String,
    pub difficulty: Difficulty,
    pub starter: String,
    pub tests: Vec<TestCase>,
    /// Text for each rung of the ladder. Missing rungs are simply unavailable.
    pub hints: Vec<(Wheel, String)>,
}

impl Problem {
    pub fn hint(&self, w: Wheel) -> Option<&str> {
        self.hints.iter().find(|(k, _)| *k == w).map(|(_, v)| v.as_str())
    }
    pub fn visible_tests(&self) -> impl Iterator<Item = &TestCase> {
        self.tests.iter().filter(|t| t.visible)
    }
}

/// Result of running the buffer against the suite.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Verdict {
    Passed,
    Failed { passed: usize, total: usize, first_failure: Option<String> },
    CompileError(String),
    Timeout,
}

impl Verdict {
    pub fn is_pass(&self) -> bool {
        matches!(self, Verdict::Passed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Reading,
    Racing,
    /// Terminal. Nothing after this changes the result.
    Over,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// You passed before the car finished.
    Won,
    /// The car finished first. You may keep going — see `kept_going`.
    Lost,
    /// You gave up and read the solution.
    Studied,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RaceResult {
    pub outcome: Outcome,
    /// Wall time to your passing submission.
    pub raw_ms: Ms,
    /// `raw_ms` plus wheel penalties. This is the number that counts.
    pub adjusted_ms: Ms,
    pub car_ms: Option<Ms>,
    pub wheels: Vec<Wheel>,
    pub failed_submissions: u32,
    /// True if you finished after losing. Worth tracking: finishing a lost
    /// race is the habit the tool is trying to build.
    pub kept_going: bool,
    /// Whether this may enter calibration history.
    pub scoreable: bool,
}

/// One race. Drive it by feeding timestamps.
pub struct Race<O: Opponent> {
    pub problem: Problem,
    pub car: O,
    pub wheels: Wheels,
    phase: Phase,
    started_at: Option<Ms>,
    failed: u32,
    result: Option<RaceResult>,
    /// Set when the car finishes while you are still typing.
    lost_at: Option<Ms>,
    /// Verdict of the most recent submission.
    last: Option<Verdict>,
}

impl<O: Opponent> Race<O> {
    pub fn new(problem: Problem, car: O) -> Self {
        Self {
            problem,
            car,
            wheels: Wheels::new(),
            phase: Phase::Reading,
            started_at: None,
            failed: 0,
            result: None,
            lost_at: None,
            last: None,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn result(&self) -> Option<&RaceResult> {
        self.result.as_ref()
    }

    /// Reading the statement is free; the clock starts when you say so.
    /// Without this, the tool would train you to skim the problem.
    pub fn start(&mut self, now: Ms) {
        if self.phase == Phase::Reading {
            self.phase = Phase::Racing;
            self.started_at = Some(now);
        }
    }

    pub fn elapsed(&self, now: Ms) -> Ms {
        self.started_at.map(|s| now.saturating_sub(s)).unwrap_or(0)
    }

    pub fn car_progress(&self, now: Ms) -> Progress {
        self.car.progress_at(self.elapsed(now))
    }

    /// Call every frame. Notices the car crossing the line.
    pub fn tick(&mut self, now: Ms) {
        if self.phase != Phase::Racing || self.lost_at.is_some() {
            return;
        }
        if self.car_progress(now).done {
            self.lost_at = Some(now);
        }
    }

    /// Take a rung. Returns the penalty, or why not.
    /// Reading the solution ends the race as [`Outcome::Studied`].
    pub fn take_wheel(&mut self, w: Wheel, now: Ms) -> Result<Ms, Refused> {
        if self.phase == Phase::Over {
            return Err(Refused::AlreadyTaken);
        }
        let pen = self.wheels.take(w)?;
        if w == Wheel::Solution {
            self.finish(Outcome::Studied, now);
        }
        Ok(pen)
    }

    /// Submit the buffer. `verdict` comes from the judge.
    pub fn submit(&mut self, verdict: Verdict, now: Ms) -> &Verdict {
        if self.phase != Phase::Racing {
            return self.stored_verdict();
        }
        if verdict.is_pass() {
            let outcome = if self.lost_at.is_some() { Outcome::Lost } else { Outcome::Won };
            self.finish(outcome, now);
        } else {
            // A failed submission is not punished with time. The clock is
            // already running; double-charging would push people to sit on a
            // solution they could have tested, which is the opposite of the
            // habit worth building.
            self.failed += 1;
        }
        self.last = Some(verdict);
        self.stored_verdict()
    }

    fn stored_verdict(&self) -> &Verdict {
        self.last.as_ref().unwrap_or(&Verdict::Timeout)
    }

    fn finish(&mut self, outcome: Outcome, now: Ms) {
        let raw = self.elapsed(now);
        self.result = Some(RaceResult {
            outcome,
            raw_ms: raw,
            adjusted_ms: raw + self.wheels.penalty_ms(),
            car_ms: self.car.finish_time(),
            wheels: self.wheels.taken().to_vec(),
            failed_submissions: self.failed,
            kept_going: outcome == Outcome::Lost,
            scoreable: self.wheels.is_scoreable() && outcome != Outcome::Studied,
        });
        self.phase = Phase::Over;
    }

    /// Give up without reading the solution.
    pub fn abandon(&mut self, now: Ms) {
        if self.phase != Phase::Over {
            self.finish(Outcome::Studied, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem() -> Problem {
        Problem {
            id: "two-sum-hard".into(),
            title: "Pair Sum in a Rotated Array".into(),
            statement: "…".into(),
            difficulty: Difficulty::Hard,
            starter: "fn solve(v: &[i32], t: i32) -> Option<(usize, usize)> {\n    todo!()\n}\n".into(),
            tests: vec![
                TestCase { input: "[1,2,3], 5".into(), expect: "(1,2)".into(), visible: true },
                TestCase { input: "[], 0".into(), expect: "None".into(), visible: false },
            ],
            hints: vec![
                (Wheel::Category, "Two pointers on a rotated sorted array.".into()),
                (Wheel::TargetComplexity, "O(n) time, O(1) space.".into()),
            ],
        }
    }

    fn car(ms: Ms) -> PaceCar {
        PaceCar::finishing_in(Recording::flat("two-sum-hard", "test", &"x".repeat(200), ms), ms)
    }

    #[test]
    fn clock_does_not_run_while_reading() {
        let mut r = Race::new(problem(), car(60_000));
        assert_eq!(r.phase(), Phase::Reading);
        assert_eq!(r.elapsed(30_000), 0, "reading must be free");
        r.start(30_000);
        assert_eq!(r.elapsed(45_000), 15_000);
    }

    #[test]
    fn beating_the_car_is_a_win() {
        let mut r = Race::new(problem(), car(60_000));
        r.start(0);
        r.tick(40_000);
        r.submit(Verdict::Passed, 40_000);
        let res = r.result().unwrap();
        assert_eq!(res.outcome, Outcome::Won);
        assert_eq!(res.raw_ms, 40_000);
        assert_eq!(res.adjusted_ms, 40_000);
        assert!(res.scoreable);
    }

    #[test]
    fn finishing_after_the_car_is_a_loss_you_still_complete() {
        let mut r = Race::new(problem(), car(60_000));
        r.start(0);
        r.tick(61_000); // car crosses
        r.submit(Verdict::Passed, 75_000);
        let res = r.result().unwrap();
        assert_eq!(res.outcome, Outcome::Lost);
        assert!(res.kept_going, "finishing a lost race is the habit worth logging");
        assert!(res.scoreable, "a clean loss is still data");
    }

    #[test]
    fn wheels_inflate_the_adjusted_time_only() {
        let mut r = Race::new(problem(), car(120_000));
        r.start(0);
        r.take_wheel(Wheel::Category, 5_000).unwrap();
        r.take_wheel(Wheel::TargetComplexity, 9_000).unwrap();
        r.submit(Verdict::Passed, 50_000);
        let res = r.result().unwrap();
        assert_eq!(res.raw_ms, 50_000);
        assert_eq!(res.adjusted_ms, 75_000, "10s + 15s of wheels");
        assert_eq!(res.wheels.len(), 2);
    }

    #[test]
    fn failed_submissions_cost_no_extra_time() {
        let mut r = Race::new(problem(), car(120_000));
        r.start(0);
        r.submit(Verdict::Failed { passed: 1, total: 2, first_failure: None }, 10_000);
        r.submit(Verdict::CompileError("borrow".into()), 20_000);
        r.submit(Verdict::Passed, 30_000);
        let res = r.result().unwrap();
        assert_eq!(res.raw_ms, 30_000);
        assert_eq!(res.adjusted_ms, 30_000, "only the clock charges you");
        assert_eq!(res.failed_submissions, 2);
    }

    #[test]
    fn reading_the_solution_ends_the_race_unscored() {
        let mut r = Race::new(problem(), car(120_000));
        r.start(0);
        for rung in Wheel::LADDER {
            let _ = r.take_wheel(rung, 10_000);
        }
        assert_eq!(r.phase(), Phase::Over);
        let res = r.result().unwrap();
        assert_eq!(res.outcome, Outcome::Studied);
        assert!(!res.scoreable, "a read solution must not train the pace car");
    }

    #[test]
    fn nothing_changes_after_the_race_is_over() {
        let mut r = Race::new(problem(), car(60_000));
        r.start(0);
        r.submit(Verdict::Passed, 20_000);
        let before = r.result().unwrap().clone();
        r.submit(Verdict::Passed, 90_000);
        r.tick(200_000);
        let _ = r.take_wheel(Wheel::Category, 200_000);
        assert_eq!(r.result().unwrap(), &before);
    }

    #[test]
    fn hidden_tests_are_hidden() {
        let p = problem();
        assert_eq!(p.visible_tests().count(), 1);
        assert_eq!(p.tests.len(), 2);
    }

    #[test]
    fn a_whole_race_is_a_fold_over_timestamps() {
        // The property that makes this testable without a clock.
        let mut r = Race::new(problem(), car(60_000));
        r.start(0);
        for t in (0..60_000).step_by(1000) {
            r.tick(t);
            assert_eq!(r.phase(), Phase::Racing);
        }
        r.tick(60_000);
        r.submit(Verdict::Passed, 60_500);
        assert_eq!(r.result().unwrap().outcome, Outcome::Lost);
    }
}
