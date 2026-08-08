//! longrun.rs — noticing that a long session has stopped making progress.
//!
//! `thinkerwatch` catches a stall inside one generation and `deliberate` catches a repeat inside one
//! round. Neither sees the failure that matters over hours: **many attempts, each fluent and
//! plausible, none of which changes the observable state.**
//!
//! This session is the worked example. One gate took **ten hypotheses and five measurements**; four
//! of the hypotheses shipped as fixes that moved the failure count by zero. Every one was reasoned
//! from source, argued confidently, and wrong. Nothing in the loop noticed, because each attempt
//! looked like progress from inside.
//!
//! ## Progress is a state change, not an action
//!
//! An edit is not progress. A commit is not progress. **Progress is a number that moved** — a gate
//! going green, a failure count dropping, a test appearing. Anything else is activity.
//!
//! That distinction is the whole module: a run that counts actions will always feel productive.

/// An observable, verifiable state.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Marker {
    /// Failing checks. Lower is progress.
    pub failures: u32,
    /// Passing tests. Higher is progress.
    pub passing: u32,
    /// Whether the target condition is met.
    pub done: bool,
}

impl Marker {
    /// Did anything verifiable change?
    ///
    /// **Not "did it improve".** A change in either direction is information — the ties-to-even
    /// attempt made the count 8.8x worse and that was the most useful run of the day, because it
    /// refuted a theory outright. A run that only counts improvements would have scored it as another
    /// wasted step.
    pub fn differs(&self, other: &Marker) -> bool { self != other }

    pub fn improved(&self, prev: &Marker) -> bool {
        self.done && !prev.done || self.failures < prev.failures || self.passing > prev.passing
    }
}

/// What a run should do next.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Course {
    /// Keep going; the last attempt moved something.
    Continue,
    /// Attempts are not changing the state. **Stop proposing and start measuring** — the specific
    /// instruction, not a vague "try harder".
    Measure { attempts_since_change: u32 },
    /// Measurement has also stopped yielding. The approach is wrong, not the execution.
    ChangeApproach { reason: &'static str },
    /// Ask the operator. Reached only after both of the above, so it is a real dead end rather than
    /// the first sign of difficulty.
    Escalate { reason: &'static str },
}

impl Course {
    pub fn explain(&self) -> String {
        match self {
            Course::Continue => "the last attempt moved something observable".into(),
            Course::Measure { attempts_since_change } => format!(
                "{attempts_since_change} attempts without an observable change — stop proposing \
explanations and print the inputs instead. Every theory so far has been consistent with the evidence, \
which is why more theories will not separate them"),
            Course::ChangeApproach { reason } => format!(
                "measurement has stopped narrowing: {reason}. The approach is wrong rather than the \
execution, and another careful attempt at the same thing is the most expensive way to learn that"),
            Course::Escalate { reason } => format!(
                "{reason} — this needs a decision that is not mine to make, and continuing would be \
spending someone else's time to avoid saying so"),
        }
    }
}

/// After this many state-unchanged attempts, stop theorising.
///
/// **Three, from evidence.** This session's first three theories were reasonable and each ruled
/// something out; theories four through seven were the same reasoning applied harder and cost a run
/// apiece. The line sits where the returns went negative, not at a round number.
pub const THEORIES_BEFORE_MEASURING: u32 = 3;

/// After this many measurements that do not narrow, the approach is wrong.
pub const MEASUREMENTS_BEFORE_RETHINK: u32 = 3;

#[derive(Clone, Debug, Default)]
pub struct Run {
    last: Marker,
    attempts_since_change: u32,
    measurements_since_narrowing: u32,
    total_attempts: u32,
}

impl Run {
    pub fn new(start: Marker) -> Self { Run { last: start, ..Default::default() } }

    /// Record an attempt that proposed a fix.
    pub fn attempted(&mut self, now: Marker) -> Course {
        self.total_attempts += 1;
        if now.differs(&self.last) {
            self.attempts_since_change = 0;
            self.last = now;
            return Course::Continue;
        }
        self.attempts_since_change += 1;
        if self.attempts_since_change >= THEORIES_BEFORE_MEASURING {
            Course::Measure { attempts_since_change: self.attempts_since_change }
        } else {
            Course::Continue
        }
    }

    /// Record a measurement — a print, a probe, an experiment.
    ///
    /// `narrowed` is whether it eliminated something. A measurement that eliminates nothing is as
    /// inert as a theory, and pretending otherwise would let "I added a print" count as progress.
    pub fn measured(&mut self, narrowed: bool) -> Course {
        if narrowed {
            self.measurements_since_narrowing = 0;
            self.attempts_since_change = 0;
            return Course::Continue;
        }
        self.measurements_since_narrowing += 1;
        if self.measurements_since_narrowing >= MEASUREMENTS_BEFORE_RETHINK {
            Course::ChangeApproach { reason: "measurements are no longer eliminating candidates" }
        } else {
            Course::Continue
        }
    }

    /// Measurements per attempt. **Below 1.0 is guessing**, whatever the outcome.
    pub fn rigor(&self) -> f32 {
        if self.total_attempts == 0 { return f32::INFINITY }
        // Attempts that changed state were informative; the rest were theories.
        (self.total_attempts - self.attempts_since_change) as f32 / self.total_attempts as f32
    }

    pub fn total_attempts(&self) -> u32 { self.total_attempts }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(f: u32, p: u32) -> Marker { Marker { failures: f, passing: p, done: false } }

    #[test]
    fn THREE_unchanged_attempts_switches_to_measuring() {
        // The line is where this session's returns went negative: theories one to three each ruled
        // something out, four through seven were the same reasoning applied harder.
        let mut r = Run::new(m(4, 100));
        assert_eq!(r.attempted(m(4, 100)), Course::Continue);
        assert_eq!(r.attempted(m(4, 100)), Course::Continue);
        let c = r.attempted(m(4, 100));
        assert!(matches!(c, Course::Measure { .. }), "{c:?}");
        assert!(c.explain().contains("print the inputs instead"));
    }

    #[test]
    fn a_change_in_EITHER_direction_counts() {
        // The ties-to-even attempt made the failure count 8.8x worse and was the most useful run of
        // the day, because it refuted a theory outright. Counting only improvements would score it as
        // another wasted step.
        let mut r = Run::new(m(1104, 500));
        assert_eq!(r.attempted(m(9680, 500)), Course::Continue, "worse is still information");
    }

    #[test]
    fn an_IDENTICAL_result_is_not_a_change_however_large_the_edit() {
        // Four fixes in this session produced byte-identical output. Each felt like progress and none
        // was — which is exactly what this counter exists to notice.
        let mut r = Run::new(m(276, 500));
        for _ in 0..2 { r.attempted(m(276, 500)); }
        assert!(matches!(r.attempted(m(276, 500)), Course::Measure { .. }));
    }

    #[test]
    fn a_measurement_that_NARROWS_resets_the_clock() {
        let mut r = Run::new(m(4, 100));
        for _ in 0..3 { r.attempted(m(4, 100)); }
        assert_eq!(r.measured(true), Course::Continue);
        assert_eq!(r.attempted(m(4, 100)), Course::Continue, "the budget is restored");
    }

    #[test]
    fn measurements_that_ELIMINATE_NOTHING_are_as_inert_as_theories() {
        // "I added a print" is not progress if the print ruled nothing out. Letting it count would
        // make the escape hatch from theorising into another way of spinning.
        let mut r = Run::new(m(4, 100));
        r.measured(false); r.measured(false);
        let c = r.measured(false);
        assert!(matches!(c, Course::ChangeApproach { .. }), "{c:?}");
        assert!(c.explain().contains("approach is wrong rather than the execution"));
    }

    #[test]
    fn ESCALATION_says_what_it_is_avoiding() {
        // Continuing past a real dead end spends someone else's time to avoid admitting one.
        let c = Course::Escalate { reason: "the fix needs a hardware capability this machine lacks" };
        assert!(c.explain().contains("not mine to make"));
    }

    #[test]
    fn RIGOR_scores_this_session_as_guessing() {
        // Ten attempts, three still-unchanged at the end: 0.7 by this measure, and the honest read is
        // that the theories outnumbered the eliminations.
        let mut r = Run::new(m(300, 500));
        for _ in 0..7 { r.attempted(m(300, 500)); }
        assert!(r.rigor() < 1.0);
        assert_eq!(r.total_attempts(), 7);
    }
}
