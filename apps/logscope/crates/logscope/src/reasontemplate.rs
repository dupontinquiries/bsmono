//! reasontemplate.rs — templates that make reasoning CHECKABLE, not merely structured.
//!
//! ## The observation this starts from
//!
//! Small models are good at copying structure. Give one a format and it will fill the format. That is
//! why cargo checking worked: the template was "write code, run it, read the error", and the middle
//! step produced a fact the model could not argue with.
//!
//! ## Why a structure-only template fails on Fermat's Last Theorem
//!
//! A template cannot supply knowledge. Asked about FLT, a small model does not lack ORGANISATION — it
//! lacks the mathematics. Give it "State the claim / Give the argument / Conclude" and it produces
//! confidently-formatted nonsense, which is strictly worse than an admitted failure: the structure
//! signals rigour that the content does not have, and a reader who trusts the format is misled by
//! exactly the thing meant to help.
//!
//! This is the `smallrole` distinction again. A template helps CONTEXT-BOUND reasoning — deriving from
//! premises that are present — and cannot help KNOWLEDGE-BOUND reasoning, where the answer depends on
//! facts the model does not have.
//!
//! ## What does work: slots that produce checkable claims
//!
//! The productive form is not "show your reasoning" but "state something that could be WRONG, and say
//! how one would find out". That converts an assertion into a test, which is what made the cargo loop
//! work — and unlike a free-form rationale, a reader can act on it without re-deriving the whole thing.
//!
//! A step is only useful if its check is cheaper than the step. "Verify by proving the theorem" is not
//! a check; "verify by substituting n=3" is.

/// One slot in a reasoning template.
#[derive(Clone, PartialEq, Debug)]
pub struct Slot {
    pub prompt: &'static str,
    /// How an answer in this slot could be shown wrong. None means the slot is decorative.
    pub check: Option<&'static str>,
}

impl Slot {
    /// Is this slot doing work, or just shaping prose?
    ///
    /// A slot with no check is where confidently-formatted nonsense goes. It is not always wrong to
    /// have one, but a template made entirely of them is a costume.
    pub fn is_load_bearing(&self) -> bool { self.check.is_some() }
}

/// A template, with the fraction of it that is checkable.
#[derive(Clone, Debug)]
pub struct Template { pub name: &'static str, pub slots: Vec<Slot> }

impl Template {
    /// Fraction of slots that produce something falsifiable.
    ///
    /// The number that distinguishes a reasoning template from a prose format. Below about half, the
    /// template is mostly shaping how confident the output SOUNDS.
    pub fn checkable_fraction(&self) -> f32 {
        if self.slots.is_empty() { return 0.0; }
        self.slots.iter().filter(|s| s.is_load_bearing()).count() as f32 / self.slots.len() as f32
    }

    /// Is this template safe to give a small model on a knowledge-bound question?
    ///
    /// Only if most slots are checkable. Otherwise it manufactures the appearance of rigour on top of
    /// content the model does not have, which is the specific failure worth preventing.
    pub fn safe_for_small_model(&self) -> bool { self.checkable_fraction() >= 0.5 }
}

/// The template that worked: code, compiled.
///
/// Kept as the reference case, because it is the one with evidence behind it.
pub fn cargo_shaped() -> Template {
    Template { name: "write-run-read", slots: vec![
        Slot { prompt: "What should this code do? One sentence.",
               check: Some("compare against the doc comment or the test name") },
        Slot { prompt: "Write it.", check: Some("cargo check") },
        Slot { prompt: "What did the compiler say?",
               check: Some("the compiler output is the answer; it cannot be argued with") },
        Slot { prompt: "Fix exactly what it named.",
               check: Some("cargo check again; the same error means no progress") },
    ]}
}

/// A template whose slots feed an executable refuter.
///
/// The upgrade over `claim_shaped`: its check is not a description of how one would verify, it is a
/// domain the machine can substitute into. `refute::PROBEABLE_SLOT` is the slot that produces it, and
/// `refute::refute` is what consumes it.
///
/// This is the pairing that makes templates work on knowledge-bound questions after all — not because
/// the template supplies knowledge, but because the refuter does not need any. It only needs the claim
/// in a shape it can execute, and destroying a false claim requires no understanding of why it is
/// false.
pub fn probeable_shaped() -> Template {
    Template { name: "claim-domain-refute", slots: vec![
        Slot { prompt: "State the claim in one sentence, with no hedging.",
               check: Some("a claim that cannot be stated plainly is not yet a claim") },
        Slot { prompt: crate::refute::PROBEABLE_SLOT,
               check: Some("either the variables and range can be named, or the claim is not universal") },
        Slot { prompt: "Now substitute the small values and the edges. Report the first failure.",
               check: Some("EXECUTED, not described: refute::refute returns a counter-example or a probe count, and a counter-example is certain where survival is not") },
        Slot { prompt: "If nothing failed, say what remains unproven and stop.",
               check: Some("survival is the absence of cheap counter-examples, never a proof") },
    ]}
}

/// A template for a claim the model may not have the knowledge for.
///
/// The key slot is the third: naming what would falsify the claim is what stops a small model producing
/// a confident wrong one, because it forces the claim into a form where being wrong is visible.
pub fn claim_shaped() -> Template {
    Template { name: "claim-evidence-falsifier", slots: vec![
        Slot { prompt: "State the claim in one sentence, with no hedging.",
               check: Some("a claim that cannot be stated plainly is not yet a claim") },
        Slot { prompt: "What in the given material supports it? Quote it.",
               check: Some("the quote either appears in the material or it does not") },
        Slot { prompt: "What single observation would show this is FALSE?",
               check: Some("if nothing would, the claim is not doing any work") },
        Slot { prompt: "Can you make that observation with what you have? If not, say so and stop.",
               check: Some("checkable directly: either the material contains it or it does not") },
    ]}
}

/// A structure-only template, kept as the counter-example.
pub fn prose_shaped() -> Template {
    Template { name: "state-argue-conclude", slots: vec![
        Slot { prompt: "State the claim.", check: None },
        Slot { prompt: "Give the argument.", check: None },
        Slot { prompt: "Conclude.", check: None },
    ]}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cargo_template_is_entirely_checkable() {
        // The reference case, and the only one with evidence behind it: every slot produces something
        // the model cannot argue with.
        let t = cargo_shaped();
        assert_eq!(t.checkable_fraction(), 1.0);
        assert!(t.safe_for_small_model());
    }

    #[test]
    fn a_state_argue_conclude_template_is_a_COSTUME() {
        // Why a structure-only template fails on Fermat's Last Theorem. It cannot supply knowledge, so
        // it produces confidently-formatted nonsense — strictly worse than an admitted failure, because
        // the structure signals rigour the content does not have.
        let t = prose_shaped();
        assert_eq!(t.checkable_fraction(), 0.0);
        assert!(!t.safe_for_small_model(), "this is the shape that manufactures false confidence");
    }

    #[test]
    fn the_claim_template_forces_a_falsifier() {
        // The slot that does the work. Naming what would show the claim false is what stops a small
        // model producing a confident wrong one — it forces the claim into a form where being wrong
        // is visible.
        let t = claim_shaped();
        assert!(t.safe_for_small_model());
        let falsifier = t.slots.iter().find(|s| s.prompt.contains("FALSE")).expect("present");
        assert!(falsifier.check.unwrap().contains("not doing any work"));
    }

    #[test]
    fn the_last_slot_permits_stopping() {
        // The escape a template usually lacks. Without it the model must produce SOMETHING for every
        // slot, and a slot it cannot fill honestly gets filled dishonestly.
        let t = claim_shaped();
        let last = t.slots.last().unwrap();
        assert!(last.prompt.contains("say so and stop"));
    }

    #[test]
    fn the_probeable_template_EXECUTES_rather_than_describing() {
        // The upgrade. claim_shaped's checks are prose telling a reader how they might verify;
        // probeable_shaped's third slot is run by refute::refute and returns a counter-example. A
        // template cannot supply knowledge, but a refuter does not need any — destroying a false claim
        // requires no understanding of why it is false.
        let t = probeable_shaped();
        assert_eq!(t.checkable_fraction(), 1.0);
        let exec = t.slots.iter().find(|s| s.check.unwrap().contains("EXECUTED")).expect("present");
        assert!(exec.check.unwrap().contains("counter-example is certain where survival is not"));
    }

    #[test]
    fn the_probeable_template_still_permits_stopping() {
        // A claim that cannot be put in executable form is a finding, not a failure to comply. Without
        // this the model invents variables for a claim that has none, and the refuter then probes
        // something the claim never said.
        let t = probeable_shaped();
        assert!(t.slots.iter().any(|s| s.prompt.contains("say so and stop")
                                    || s.prompt.contains("and stop")));
    }

    #[test]
    fn a_half_checkable_template_is_the_boundary() {
        // Below half, the template is mostly shaping how confident the output SOUNDS rather than
        // whether it is right.
        let t = Template { name: "mixed", slots: vec![
            Slot { prompt: "a", check: Some("x") },
            Slot { prompt: "b", check: None },
        ]};
        assert_eq!(t.checkable_fraction(), 0.5);
        assert!(t.safe_for_small_model(), "exactly half passes; below it does not");
    }
}
