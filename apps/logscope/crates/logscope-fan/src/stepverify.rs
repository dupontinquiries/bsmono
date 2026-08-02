//! stepverify.rs — check each step before asking for the next one.
//!
//! ## The failure this exists for
//!
//! A 30-turn coordinator run on a 1B model produced the same broken snippet every turn:
//! `std::time::{Duration, Instant}` used as an expression, dangling `else if` with no opening `if`,
//! undefined variables, and a hallucinated crate (`ratatouille`) that survived being corrected. The
//! prompts varied only in tone — "briefly", "as an expert", "be concise" — and never asked the model to
//! verify anything.
//!
//! The diagnosis that matters: **iteration without verification is resampling, not refinement.** Each
//! turn drew a fresh sample from the same distribution and shuffled which lines had comments. Nothing
//! ever told the model that line 4 does not parse.
//!
//! ## Why more temperature would make it worse
//!
//! The tempting reading is that the model got stuck and needs more variation. It was not stuck — it
//! produced visibly different text every turn. Higher temperature yields *differently* broken code, and
//! without a signal distinguishing samples that compile from samples that do not, more variance simply
//! widens the search over a space where nothing is selecting.
//!
//! What was missing is the selection, and `prechecks` has been able to supply it since it was written.
//! It had no caller.
//!
//! ## Repeating a finding is the stop condition
//!
//! If the same fault survives a round of being told about it, another round will not fix it — that is
//! the empirical content of thirty turns. `Verifier` stops there and escalates, rather than spending
//! the remaining budget confirming it.

use std::collections::HashSet;

/// What to do with a step's output.
#[derive(Clone, PartialEq, Debug)]
pub enum Verdict {
    /// Clean enough to continue.
    Advance,
    /// Send these findings back and ask for THIS step again, not the next one.
    Revise { findings: Vec<String>, prompt: String },
    /// The same faults survived being reported. More turns will not fix them.
    Stuck { repeated: Vec<String> },
}

impl Verdict {
    pub fn advances(&self) -> bool { matches!(self, Verdict::Advance) }
}

/// Tracks what has already been reported, so a repeat can be recognised.
#[derive(Clone, Debug, Default)]
pub struct Verifier { reported: HashSet<String>, rounds: u32 }

impl Verifier {
    pub fn new() -> Self { Self::default() }
    pub fn rounds(&self) -> u32 { self.rounds }

    /// Judge one step's output.
    ///
    /// `blocking` are findings the checker is CERTAIN about — code that cannot compile. Suspicions are
    /// deliberately not included: sending a model a list of maybes teaches it to argue rather than fix,
    /// and a checker that cries wolf gets ignored along with its real findings.
    pub fn check(&mut self, blocking: &[String]) -> Verdict {
        self.rounds += 1;
        if blocking.is_empty() { return Verdict::Advance; }
        let repeated: Vec<String> = blocking.iter()
            .filter(|f| self.reported.contains(*f)).cloned().collect();
        if !repeated.is_empty() {
            return Verdict::Stuck { repeated };
        }
        for f in blocking { self.reported.insert(f.clone()); }
        Verdict::Revise { findings: blocking.to_vec(), prompt: revise_prompt(blocking) }
    }
}

/// The message sent back with findings.
///
/// Concrete and imperative. "Please review your code" is what produced thirty turns of tone changes;
/// naming the line and the fault is what a model can act on.
pub fn revise_prompt(findings: &[String]) -> String {
    let mut s = String::from("Your last step does not compile. These are not style notes — each one is \
a definite error found by a checker, not an opinion:\n");
    for f in findings { s.push_str(&format!("  - {f}\n")); }
    s.push_str("\nRewrite THAT STEP with these fixed. Do not move on, do not restate the plan, and do \
not change anything that was not listed — a rewrite that fixes one fault and introduces another leaves \
you where you started.");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_step_advances() {
        let mut v = Verifier::new();
        assert_eq!(v.check(&[]), Verdict::Advance);
        assert!(v.check(&[]).advances());
    }

    #[test]
    fn a_first_finding_asks_for_THAT_step_again_not_the_next() {
        // The whole correction. Advancing past a step that does not compile is how thirty turns of
        // broken code accumulate — each one built on the last one's errors.
        let mut v = Verifier::new();
        let f = vec!["line 4: `std::time::{Duration, Instant}` used as an expression".to_string()];
        match v.check(&f) {
            Verdict::Revise { prompt, findings } => {
                assert_eq!(findings, f);
                assert!(prompt.contains("Rewrite THAT STEP"));
                assert!(prompt.contains("Do not move on"));
            }
            other => panic!("expected Revise, got {other:?}"),
        }
    }

    #[test]
    fn a_finding_that_survives_being_reported_is_STUCK() {
        // The empirical content of the 30-turn transcript: the same syntax error appeared verbatim in
        // nearly every response. If telling the model once did not fix it, telling it again will not
        // either, and the remaining budget is better spent escalating.
        let mut v = Verifier::new();
        let f = vec!["line 4: dangling `else if` with no opening `if`".to_string()];
        assert!(matches!(v.check(&f), Verdict::Revise { .. }));
        match v.check(&f) {
            Verdict::Stuck { repeated } => assert_eq!(repeated, f),
            other => panic!("expected Stuck on the repeat, got {other:?}"),
        }
    }

    #[test]
    fn a_DIFFERENT_finding_still_gets_a_revision() {
        // Stuck is about the same fault surviving, not about having faults at all. A model that fixes
        // one error and reveals another underneath is making progress and should be allowed to.
        let mut v = Verifier::new();
        assert!(matches!(v.check(&["error A".into()]), Verdict::Revise { .. }));
        assert!(matches!(v.check(&["error B".into()]), Verdict::Revise { .. }));
        assert_eq!(v.rounds(), 2);
    }

    #[test]
    fn the_revise_prompt_says_these_are_not_style_notes() {
        // The transcript's prompts varied only in tone — "briefly", "as an expert", "be concise" — and
        // the model responded in kind, shuffling comments. The message has to be unmistakably about
        // correctness or it gets answered as a style request.
        let p = revise_prompt(&["line 9: `ratatouille` is not a dependency".into()]);
        assert!(p.contains("not style notes"));
        assert!(p.contains("found by a checker, not an opinion"));
        assert!(p.contains("ratatouille"), "the finding itself is quoted, not summarised");
    }

    #[test]
    fn the_prompt_forbids_changing_what_was_not_listed() {
        // "Each improvement pass shuffled which lines had comments while introducing a new syntax error
        // elsewhere." Constraining the rewrite to the listed faults is what stops that.
        let p = revise_prompt(&["x".into()]);
        assert!(p.contains("do not change anything that was not listed"));
        assert!(p.contains("fixes one fault and introduces another"));
    }
}
