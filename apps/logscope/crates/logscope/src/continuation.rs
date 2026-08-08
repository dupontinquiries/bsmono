//! continuation.rs — what "keep going" should mean to the model.
//!
//! A capable model reconstructs where it was from the transcript. A small one restarts, or repeats
//! the last thing, or produces the plan again — which is the exact failure a real transcript showed:
//! "keep working on planning then implement it" answered with another outline, twice.
//!
//! The transcript is present and the STEP is not. Nothing in it says "you are on step 3 of 7", so
//! the model infers it, and inference is what small models do worst.
//!
//! ## The plan must be the model's own
//!
//! Injecting a plan the model did not write produces compliance with someone else's structure. The
//! plan is whatever it said it would do; this only remembers it and says which part is unfinished.
//!
//! ## A step advances on EVIDENCE, not on a claim
//!
//! "Done with step 2" is a sentence. Code appearing is a fact. Advancing on the sentence lets a model
//! walk through a plan without building anything, which is the failure this exists to prevent rather
//! than to formalise.

/// One step of a plan the model stated.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Step {
    pub what: String,
    /// Did something observable happen — code, a file, a passing test?
    pub evidenced: bool,
}

/// The plan, as the model described it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Plan { pub steps: Vec<Step> }

impl Plan {
    /// The first step without evidence. **What "keep going" means.**
    pub fn next_unfinished(&self) -> Option<(usize, &Step)> {
        self.steps.iter().enumerate().find(|(_, s)| !s.evidenced)
    }

    pub fn done(&self) -> usize { self.steps.iter().filter(|s| s.evidenced).count() }

    /// The line to inject on a continuation.
    ///
    /// Deliberately short. A long reminder competes with the work for attention, and the model
    /// already has the transcript — this supplies the ONE fact the transcript does not carry.
    pub fn continuation_hint(&self) -> String {
        match self.next_unfinished() {
            Some((i, s)) => format!(
                "CONTINUING: step {} of {} — \"{}\". {} step(s) already produced code. Do this step \
now; do not restate the plan.", i + 1, self.steps.len(), s.what, self.done()),
            None if self.steps.is_empty() => "CONTINUING: no plan was stated, so there is no step to \
resume. Say in one line what you are about to build, then build it.".into(),
            // Every step has evidence. Saying "the plan is complete" invites a summary, which is the
            // least useful thing a continuation can produce.
            None => format!("CONTINUING: all {} steps have produced code. Either extend the plan with \
a step that is not yet done, or say plainly that the goal is met and stop.", self.steps.len()),
        }
    }

    /// Record that something observable happened for the current step.
    ///
    /// **Takes evidence, not a claim.** A model that says it finished a step does not advance it;
    /// code appearing does.
    pub fn mark_evidenced(&mut self, produced_code: bool) -> Result<usize, &'static str> {
        if !produced_code {
            return Err("no code was produced, so the step does not advance. A model that says it \
finished and shows nothing has restated the plan, which is the failure this tracking exists to catch");
        }
        match self.steps.iter().position(|s| !s.evidenced) {
            Some(i) => { self.steps[i].evidenced = true; Ok(i) }
            None => Err("every step already has evidence; extend the plan rather than re-marking it"),
        }
    }

    /// Has the run stopped producing across repeated continuations?
    ///
    /// Mirrors `longrun`: progress is a state change, and "keep going" answered three times with no
    /// new evidence is the same plateau by a different route.
    pub fn stalled(&self, continuations_since_evidence: u32) -> bool {
        continuations_since_evidence >= 3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(n: usize, done: usize) -> Plan {
        Plan { steps: (0..n).map(|i| Step { what: format!("step {i}"), evidenced: i < done }).collect() }
    }

    #[test]
    fn KEEP_GOING_names_the_step_rather_than_leaving_it_inferred() {
        // The transcript is present and the STEP is not. Inference is what small models do worst.
        let h = plan(7, 2).continuation_hint();
        assert!(h.contains("step 3 of 7"));
        assert!(h.contains("do not restate the plan"));
    }

    #[test]
    fn a_step_advances_on_EVIDENCE_not_on_a_claim() {
        // "Done with step 2" is a sentence; code appearing is a fact. Advancing on the sentence lets
        // a model walk a plan without building anything.
        let mut p = plan(3, 0);
        assert!(p.mark_evidenced(false).is_err());
        assert_eq!(p.done(), 0, "a claim moved nothing");
        assert_eq!(p.mark_evidenced(true), Ok(0));
        assert_eq!(p.done(), 1);
    }

    #[test]
    fn the_refusal_names_the_FAILURE_it_is_catching() {
        let e = plan(3, 0).mark_evidenced(false).unwrap_err();
        assert!(e.contains("has restated the plan"));
    }

    #[test]
    fn NO_PLAN_asks_for_one_line_and_then_the_work() {
        // Not "please provide a plan" — that spends a turn producing an outline, which is what the
        // whole layer is trying to avoid.
        let h = Plan::default().continuation_hint();
        assert!(h.contains("Say in one line what you are about to build, then build it"));
    }

    #[test]
    fn a_COMPLETE_plan_does_not_invite_a_summary() {
        // "The plan is complete" produces a recap, which is the least useful thing a continuation
        // can return.
        let h = plan(3, 3).continuation_hint();
        assert!(h.contains("extend the plan") && h.contains("say plainly that the goal is met"));
        assert!(!h.contains("summar"));
    }

    #[test]
    fn three_continuations_with_no_evidence_is_a_STALL() {
        // Mirrors longrun's threshold and for the same reason: "keep going" answered three times
        // with nothing new is a plateau by a different route.
        assert!(!plan(5, 1).stalled(2));
        assert!(plan(5, 1).stalled(3));
    }

    #[test]
    fn the_hint_is_SHORT_because_it_competes_with_the_work() {
        // The model already has the transcript; this supplies the one fact the transcript lacks.
        assert!(plan(7, 2).continuation_hint().len() < 200);
    }
}
