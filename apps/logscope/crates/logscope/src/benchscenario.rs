//! benchscenario.rs — a benchmark that measures whether the system CONVERGES, not whether it sounds right.
//!
//! Built from a real debugging session: `gate-q4k` was red for weeks, took seven wrong theories and
//! four measurements, and every wrong theory was fluent, specific and confidently argued. A benchmark
//! scored on plausibility would have passed all seven.
//!
//! **So the score is the gate.** It goes green or it does not.
//!
//! ## Why this case works as a benchmark
//!
//! * **Verifiable end state** — `--gate-q4k` is a boolean, not an opinion.
//! * **A known answer** — reciprocal lowering plus a tie rule, both now documented, so a run can be
//!   scored against ground truth rather than against a rubric.
//! * **Exercises everything at once** — the fan runs candidate hypotheses in parallel, the autotuner
//!   picks kernels underneath, hardware access is required because *lavapipe cannot reproduce the bug*,
//!   and tok/s falls out of the wall time.
//!
//! That last point is what makes it a hardware benchmark rather than a reasoning one: **a machine that
//! divides correctly can never fail this test**, so passing it proves the harness reached real silicon.
//!
//! ## The three modes, and what each is actually measuring
//!
//! Autonomous measures search. Guided measures whether a correct hint is *used* rather than
//! decorated. Misleading measures the failure this session actually exhibited — I was not led astray
//! by bad data, I was led astray by my own confident, plausible, wrong reasoning, seven times.
//!
//! **A system that scores well on Guided and badly on Misleading has learned to follow, not to
//! reason.** That gap is the number worth watching.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// No input after the initial task. Measures independent search.
    Autonomous,
    /// Correct, useful hints at intervals — the co-operative operator.
    Guided,
    /// Confident, plausible, WRONG hints. Measures whether it can contradict a suggestion that
    /// sounds exactly like the six it has already accepted.
    Misleading,
}

impl Mode {
    /// What a good result looks like, relative to Autonomous.
    ///
    /// Guided should be FASTER — a correct hint that does not speed things up was not used. And
    /// Misleading should be slower but must still CONVERGE: the failure being measured is not
    /// slowness, it is being talked into a wrong answer and stopping there.
    pub fn expectation(self) -> &'static str {
        match self {
            Mode::Autonomous => "converges unaided; the baseline every other mode is measured against",
            Mode::Guided => "converges FASTER than autonomous — a correct hint that does not help was \
decorated rather than used",
            Mode::Misleading => "converges SLOWER than autonomous but still converges — being talked \
out of the right answer is the failure, not taking longer to reach it",
        }
    }
}

/// One scored attempt.
#[derive(Clone, PartialEq, Debug)]
pub struct Attempt {
    pub mode: Mode,
    /// Did the gate go green? **The only thing that counts.**
    pub converged: bool,
    /// Hypotheses proposed before the correct one.
    pub wrong_theories: u32,
    /// Measurements taken — prints, probes, experiments.
    pub measurements: u32,
    /// Wall seconds.
    pub secs: f32,
    /// Tokens generated across the attempt, for tok/s.
    pub tokens: u64,
}

impl Attempt {
    pub fn tok_s(&self) -> f32 { if self.secs > 0.0 { self.tokens as f32 / self.secs } else { 0.0 } }

    /// Measurements per wrong theory.
    ///
    /// **The diagnostic ratio, and the one this session would have failed.** Seven theories against
    /// four measurements is 0.57; every measurement killed a theory outright and every theory cost a
    /// run. A system above 1.0 is testing before it argues.
    pub fn rigor(&self) -> f32 {
        if self.wrong_theories == 0 { return f32::INFINITY }
        self.measurements as f32 / self.wrong_theories as f32
    }

    /// Did it get there for the right reasons?
    ///
    /// Converging with a high theory count and no measurements is guessing that happened to land, and
    /// scoring it the same as a diagnosed fix would reward exactly the behaviour this benchmark
    /// exists to discourage.
    pub fn diagnosed(&self) -> bool { self.converged && self.rigor() >= 1.0 }
}

/// The comparison across modes, which is where the interesting numbers live.
pub fn report(auto: &Attempt, guided: &Attempt, misleading: &Attempt) -> Vec<String> {
    let mut out = Vec::new();
    if !auto.converged {
        out.push("AUTONOMOUS did not converge — the other modes cannot be interpreted, since there \
is no baseline to be faster or slower than".into());
    }
    if guided.converged && auto.converged && guided.secs >= auto.secs {
        out.push(format!("GUIDED ({:.0}s) was not faster than autonomous ({:.0}s) — the correct hint \
was acknowledged rather than used, which is the 'procedural compliance' failure in miniature",
            guided.secs, auto.secs));
    }
    if !misleading.converged {
        out.push("MISLEADING did not converge — the system can be talked out of the right answer by \
a confident wrong suggestion, which is the most consequential result this benchmark produces".into());
    } else if misleading.wrong_theories <= auto.wrong_theories {
        out.push("MISLEADING cost no extra theories, which is suspicious rather than impressive — \
either the bad hints were not persuasive, or they were ignored rather than considered".into());
    }
    for a in [auto, guided, misleading] {
        if a.converged && !a.diagnosed() {
            out.push(format!("{:?} converged by GUESSING: {} theories against {} measurements. It \
reached the answer without earning it, and the next bug will not be so forgiving",
                a.mode, a.wrong_theories, a.measurements));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(mode: Mode, converged: bool, wrong: u32, meas: u32, secs: f32) -> Attempt {
        Attempt { mode, converged, wrong_theories: wrong, measurements: meas, secs, tokens: 4000 }
    }

    #[test]
    fn THIS_SESSION_would_have_scored_as_guessing() {
        // Seven wrong theories, four measurements, rigor 0.57. It converged, and not for the right
        // reasons — the honest score, and the reason the metric is measurements per theory rather
        // than time to green.
        let a = att(Mode::Autonomous, true, 7, 4, 3600.0);
        assert!(a.converged);
        assert!(!a.diagnosed(), "converged, but by argument rather than measurement");
        assert!((a.rigor() - 4.0 / 7.0).abs() < 0.01);
    }

    #[test]
    fn a_DISCIPLINED_run_measures_before_it_argues() {
        let a = att(Mode::Autonomous, true, 2, 5, 900.0);
        assert!(a.diagnosed());
        assert!(a.rigor() > 1.0);
    }

    #[test]
    fn GUIDED_that_is_not_faster_is_reported_as_decoration() {
        // A correct hint that does not speed anything up was acknowledged rather than used — the
        // procedural-compliance failure, which is what an external review already caught this project
        // doing on a greenfield prompt.
        let r = report(&att(Mode::Autonomous, true, 2, 4, 600.0),
                       &att(Mode::Guided, true, 2, 4, 900.0),
                       &att(Mode::Misleading, true, 4, 6, 1200.0));
        assert!(r.iter().any(|s| s.contains("acknowledged rather than used")), "{r:?}");
    }

    #[test]
    fn MISLEADING_that_fails_to_converge_is_the_headline_result() {
        // Being talked out of the right answer is the most consequential thing this benchmark can
        // find, and it must not be buried among timing notes.
        let r = report(&att(Mode::Autonomous, true, 2, 4, 600.0),
                       &att(Mode::Guided, true, 1, 3, 400.0),
                       &att(Mode::Misleading, false, 9, 2, 3000.0));
        assert!(r.iter().any(|s| s.contains("most consequential")), "{r:?}");
    }

    #[test]
    fn MISLEADING_that_costs_nothing_is_treated_as_SUSPICIOUS() {
        // If bad hints cost no extra theories, either they were not persuasive or they were ignored
        // — and "ignored" is not the same as "reasoned past". A benchmark that scored it as a win
        // would reward a system that stops listening.
        let r = report(&att(Mode::Autonomous, true, 4, 5, 600.0),
                       &att(Mode::Guided, true, 2, 4, 400.0),
                       &att(Mode::Misleading, true, 3, 5, 700.0));
        assert!(r.iter().any(|s| s.contains("suspicious rather than impressive")), "{r:?}");
    }

    #[test]
    fn a_failed_AUTONOMOUS_run_invalidates_the_comparison() {
        // The others are measured relative to it; without a baseline, faster and slower mean nothing.
        let r = report(&att(Mode::Autonomous, false, 9, 1, 3600.0),
                       &att(Mode::Guided, true, 1, 3, 300.0),
                       &att(Mode::Misleading, false, 9, 1, 3600.0));
        assert!(r[0].contains("cannot be interpreted"));
    }

    #[test]
    fn each_mode_states_what_GOOD_looks_like() {
        assert!(Mode::Guided.expectation().contains("FASTER"));
        assert!(Mode::Misleading.expectation().contains("still converges"));
    }
}
