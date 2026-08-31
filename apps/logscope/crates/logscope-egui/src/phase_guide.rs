//! phase_guide — the wiring `coordinator::Mode::StatefulGuide` promised: the adapter that feeds
//! `logscope_fan::engmode`'s tested phase machine from what the workspace agent loop actually
//! observes, and hands its per-phase instructions back into the prompt.
//!
//! ## Why this is a separate module
//!
//! `engmode` was the c-session example of the one failure shape found ~15 times: built carefully,
//! unit-tested, and never called. Its own charter names the seam — "the runner that calls cargo and
//! the model lives outside" — and `workspace_exec::step_turn` IS that runner. This module is the
//! adapter between them, kept out of `step_turn` so the derivation (loop observation → Signal) is a
//! pure function testable without a workspace, a model, or egui — the same layering engmode itself
//! uses (Fsm has no I/O; this has no Fsm logic).
//!
//! ## The derivation rule
//!
//! Every `Signal` fed to the machine corresponds to something the loop OBSERVED — a tool result, a
//! parsed plan, a checker's counted output, the done marker in the text. Nothing is synthesized: a
//! model that skips surveying and writes code on turn 1 does not get a fabricated `Surveyed`; the
//! machine simply stays where the evidence left it (its `Checked` arm fires from any phase, so the
//! trace stays truthful and the run still gates on evidence). That keeps engmode's "nothing here is
//! an inference" property intact across the wiring.

use logscope_fan::engmode::{prompt_for, Fsm, PatternStats, Phase, Signal};

/// What one agent turn observably did — gathered by `step_turn` from results it already has.
/// All counts are of SUCCESSFUL tool applications unless the field says otherwise.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnObs {
    /// Read-only tool calls that succeeded (read_file / read_range / list_dir / search / git_log).
    pub reads: usize,
    /// A plan parsed out of this turn's text (None = no new plan appeared).
    pub plan_steps: Option<usize>,
    /// write_file / edit applications that succeeded.
    pub writes_ok: usize,
    /// write_file / edit applications that failed, with the first failure's reason.
    pub writes_failed: usize,
    pub first_write_failure: Option<String>,
    /// The checker ran: (errors, warnings, tests_failed) as counted from its output.
    pub check: Option<(usize, usize, usize)>,
    /// The turn contained the done marker (with no tool calls alongside it).
    pub claims_done: bool,
}

/// What the guide decided after observing a turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Keep going; inject `Guide::prompt()` so the model knows its position.
    Continue,
    /// Evidence-backed completion — the callout carries the numbers.
    Done(String),
    /// Evidence-backed non-completion (stalled error count, or cancellation).
    Blocked(String),
}

/// The stateful thought guide for one agent run: owns the Fsm and the observation → signal mapping.
#[derive(Clone, Debug)]
pub struct Guide {
    fsm: Fsm,
}

impl Guide {
    pub fn new() -> Self {
        Self { fsm: Fsm::new() }
    }

    /// Perf-goal runs insert the Perf phase after the first clean check (disassembly discipline).
    pub fn with_perf_goal(mut self, yes: bool) -> Self {
        self.fsm = std::mem::take(&mut self.fsm).with_perf_goal(yes);
        self
    }

    pub fn phase(&self) -> Phase {
        self.fsm.phase()
    }
    pub fn phase_label(&self) -> &'static str {
        self.fsm.phase().label()
    }

    /// The per-phase instruction block for the prompt: a position line plus engmode's phase text.
    /// Injected every model-facing turn — an append-only transcript means an instruction stated
    /// once at turn 3 is forty turns behind the cursor by turn 8.
    ///
    /// The Done phase gets runner-layer text instead of engmode's bare "PHASE: DONE.": the machine
    /// reaching Done says the EVIDENCE supports completion, but only the model knows whether the
    /// task's scope is covered (a refactor's first patch compiles green with three plan steps left).
    /// So a green state prompts for the claim rather than asserting it — and the claim, once made,
    /// is confirmed against this same evidence in `observe`.
    pub fn prompt(&self) -> String {
        if self.fsm.phase() == Phase::Done {
            return format!(
                "\n\n[PHASE: {}]\nCHECK IS GREEN — 0 errors, 0 failing tests. If the task is fully \
complete, reply {} now (it will be accepted: the evidence supports it). If plan steps remain, \
continue with the next one; the next check decides again.\n",
                self.phase_label(),
                crate::workspace_exec::DONE_MARKER,
            );
        }
        format!("\n\n[PHASE: {}]\n{}\n", self.phase_label(), prompt_for(self.fsm.phase(), &self.fsm))
    }

    /// What actually moved the error count on THIS repo, as observed fact for the prompt or the
    /// operator. Empty until enough beats accumulate.
    pub fn pattern_report(&self) -> Vec<String> {
        PatternStats::from_traces([self.fsm.trace()]).report(2)
    }

    /// Feed one turn's observations, in evidence order (reads → plan → patches → check → claim),
    /// and return the machine's verdict. Signal order matters: a turn that both patched and got
    /// checked must record the patch before the check, or the trace attributes the check to the
    /// wrong phase.
    pub fn observe(&mut self, obs: &TurnObs) -> Verdict {
        // Reading before writing is the Survey evidence — but only while the machine is actually
        // surveying, and only if the turn didn't also write (a read-alongside-a-patch is context
        // gathering for the patch, not a survey).
        if obs.reads > 0 && obs.writes_ok == 0 && obs.writes_failed == 0 && self.fsm.phase() == Phase::Survey {
            self.fsm.step(&Signal::Surveyed { targets: obs.reads });
        }
        if let Some(steps) = obs.plan_steps {
            self.fsm.step(&Signal::Planned { steps });
        }
        if obs.writes_ok > 0 {
            self.fsm.step(&Signal::PatchApplied { files: obs.writes_ok });
        } else if obs.writes_failed > 0 {
            self.fsm.step(&Signal::PatchRejected {
                reason: obs.first_write_failure.clone().unwrap_or_else(|| "edit did not apply".into()),
            });
        }
        if let Some((errors, warnings, tests_failed)) = obs.check {
            self.fsm.step(&Signal::Checked { errors, warnings, tests_failed });
        }
        if obs.claims_done {
            self.fsm.step(&Signal::ModelClaimsDone);
        }
        // RUN-TERMINATION POLICY (runner layer, deliberately not in the machine): Blocked is always
        // terminal — the evidence says the run cannot move the number. Done is terminal only when
        // this turn CLAIMED completion and the machine confirmed it; a machine that reached Done
        // from a bare green check keeps the run alive, because green is necessary for completion
        // and not sufficient (the first patch of a four-step plan compiles too). `prompt()` carries
        // the green-state nudge that asks for the claim.
        match self.verdict() {
            Verdict::Done(c) if obs.claims_done => Verdict::Done(c),
            Verdict::Done(_) => Verdict::Continue,
            v => v,
        }
    }

    /// The operator asked to stop.
    pub fn cancel(&mut self) -> Verdict {
        self.fsm.step(&Signal::Cancelled);
        self.verdict()
    }

    fn verdict(&self) -> Verdict {
        if let Some(c) = self.fsm.done_callout() {
            return Verdict::Done(c);
        }
        if let Some(c) = self.fsm.blocked_callout() {
            return Verdict::Blocked(c);
        }
        Verdict::Continue
    }
}

impl Default for Guide {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(f: impl FnOnce(&mut TurnObs)) -> TurnObs {
        let mut t = TurnObs::default();
        f(&mut t);
        t
    }

    #[test]
    fn the_disciplined_run_finishes_on_a_claim_the_evidence_supports() {
        let mut g = Guide::new();
        assert_eq!(g.phase(), Phase::Survey);
        assert_eq!(g.observe(&turn(|t| t.reads = 3)), Verdict::Continue);
        assert_eq!(g.phase(), Phase::Plan, "reads while surveying advance to Plan");
        assert_eq!(g.observe(&turn(|t| t.plan_steps = Some(2))), Verdict::Continue);
        assert_eq!(g.phase(), Phase::Patch);
        assert_eq!(g.observe(&turn(|t| t.writes_ok = 1)), Verdict::Continue);
        assert_eq!(g.phase(), Phase::Check);
        // Green evidence alone does NOT end the run — the first of two plan steps compiles too.
        assert_eq!(g.observe(&turn(|t| t.check = Some((0, 1, 0)))), Verdict::Continue);
        assert!(g.prompt().contains("CHECK IS GREEN"), "{}", g.prompt());
        // The model covers the remaining step, stays green, and claims completion — accepted.
        assert_eq!(g.observe(&turn(|t| {
            t.writes_ok = 1;
            t.check = Some((0, 0, 0));
        })), Verdict::Continue);
        match g.observe(&turn(|t| t.claims_done = true)) {
            Verdict::Done(c) => assert!(c.contains("0 errors"), "{c}"),
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn a_done_claim_without_a_clean_check_is_rejected_and_named_in_the_prompt() {
        let mut g = Guide::new();
        g.observe(&turn(|t| t.reads = 1));
        g.observe(&turn(|t| t.plan_steps = Some(1)));
        g.observe(&turn(|t| {
            t.writes_ok = 1;
            t.check = Some((4, 0, 0));
        }));
        assert_eq!(g.phase(), Phase::Diagnose);
        assert_eq!(g.observe(&turn(|t| t.claims_done = true)), Verdict::Continue, "claim must not finish the run");
        assert!(g.prompt().contains("decided by the checker"), "{}", g.prompt());
        // With evidence, a renewed claim finishes the run — and the callout remembers the overreach.
        assert_eq!(g.observe(&turn(|t| {
            t.writes_ok = 1;
            t.check = Some((0, 0, 0));
        })), Verdict::Continue);
        match g.observe(&turn(|t| t.claims_done = true)) {
            Verdict::Done(c) => assert!(c.contains("rejected for lack of a clean check"), "{c}"),
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn a_stalled_error_count_reaches_blocked_with_the_stall_named() {
        let mut g = Guide::new();
        g.observe(&turn(|t| {
            t.writes_ok = 1;
            t.check = Some((5, 0, 0));
        }));
        for _ in 0..2 {
            assert_eq!(
                g.observe(&turn(|t| {
                    t.writes_ok = 1;
                    t.check = Some((5, 0, 0));
                })),
                Verdict::Continue
            );
        }
        match g.observe(&turn(|t| {
            t.writes_ok = 1;
            t.check = Some((5, 0, 0));
        })) {
            Verdict::Blocked(c) => assert!(c.contains("without the error count falling"), "{c}"),
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    #[test]
    fn skipping_the_survey_still_gates_on_evidence() {
        // A model that writes code immediately: no Surveyed is fabricated, and the run still
        // finishes only on a claim the counted check supports.
        let mut g = Guide::new();
        assert_eq!(g.observe(&turn(|t| t.writes_ok = 1)), Verdict::Continue);
        assert_eq!(g.phase(), Phase::Survey, "no synthesized signals — the machine stays truthful");
        assert_eq!(g.observe(&turn(|t| t.check = Some((0, 0, 0)))), Verdict::Continue,
                   "a clean check nudges for the claim rather than ending the run");
        match g.observe(&turn(|t| t.claims_done = true)) {
            Verdict::Done(_) => {}
            other => panic!("the claim is supported by the clean check, got {other:?}"),
        }
    }

    #[test]
    fn a_failed_edit_is_a_patch_problem_not_a_diagnosis_problem() {
        let mut g = Guide::new();
        g.observe(&turn(|t| t.reads = 1));
        g.observe(&turn(|t| t.plan_steps = Some(1)));
        assert_eq!(g.phase(), Phase::Patch);
        g.observe(&turn(|t| {
            t.writes_failed = 1;
            t.first_write_failure = Some("anchor matches 2 places".into());
        }));
        assert_eq!(g.phase(), Phase::Patch, "a failed EDIT returns to Patch, not Diagnose");
    }

    #[test]
    fn cancel_blocks_from_anywhere() {
        let mut g = Guide::new();
        g.observe(&turn(|t| t.reads = 2));
        match g.cancel() {
            Verdict::Blocked(_) => {}
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    #[test]
    fn the_prompt_always_names_the_phase() {
        let g = Guide::new();
        assert!(g.prompt().contains("[PHASE: survey]"));
        assert!(g.prompt().contains("Do not write code yet"));
    }
}
