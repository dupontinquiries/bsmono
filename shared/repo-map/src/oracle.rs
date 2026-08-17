//! oracle — one interface over the checks this repo already has.
//!
//! Four independent verifiers exist here and none of them know about each
//! other:
//!
//! | what it judges | where it lives |
//! |---|---|
//! | does this code compile | `logscope-fan::verdict` — spawns a compiler, believes the exit code |
//! | is this state change legal | [`crate::state::check_plan`] |
//! | is every ported module reachable | `maintenance/tools/wiring_check.py` |
//! | does the behaviour hold | each crate's test suite |
//!
//! A proposer — a model, a search, a person — currently has to know which to
//! call and how to read each answer. This is the common shape, so a candidate
//! can be put to all of them at once and scored without the proposer knowing
//! what any of them are.
//!
//! ## Why a trait and not a function
//!
//! Because the interesting question is which oracles *decline*. An oracle that
//! cannot judge a candidate must say so — [`Verdict::NotApplicable`] — rather
//! than passing it. A compiler asked about a design decision has no opinion,
//! and a scoring loop that reads silence as approval will optimise exactly the
//! things nobody is checking.
//!
//! ## The part with no oracle
//!
//! Compilation, legality, reachability and behaviour are all decidable and
//! cheap. Whether the design is *right* is neither. Wiring a mail view into a
//! tab bar compiled, passed, was legal and reachable — and was the wrong shape
//! for a tiling workspace, which no verifier here would ever have said. The
//! honest use of this file is to make the verified region explicit so the
//! unverified one stays visible; the temptation with a good oracle is to
//! optimise only what it scores.

/// What an oracle concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Judged and accepted.
    Pass,
    /// Judged and rejected, with the reason a proposer can act on.
    Fail(String),
    /// This oracle has no opinion on this candidate.
    ///
    /// Distinct from `Pass` on purpose, and the distinction is the point of
    /// the enum. Folding "cannot judge" into "fine" is how a scoring loop
    /// concludes that unverifiable work is verified work.
    NotApplicable(String),
}

impl Verdict {
    pub fn passed(&self) -> bool {
        matches!(self, Verdict::Pass)
    }
    /// Did this oracle actually judge?
    pub fn judged(&self) -> bool {
        !matches!(self, Verdict::NotApplicable(_))
    }
}

/// Something that can judge a candidate.
///
/// `Candidate` is generic because the four existing verifiers take different
/// things — source text, a plan, a whole tree — and forcing them into one
/// input type would mean inventing a union that fits none of them well.
pub trait Oracle<C> {
    /// A short name, for reporting which oracles spoke.
    fn name(&self) -> &str;
    fn judge(&self, candidate: &C) -> Verdict;
}

/// The result of putting one candidate to several oracles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    pub verdicts: Vec<(String, Verdict)>,
}

impl Panel {
    /// Accepted only if at least one oracle judged it and none rejected it.
    ///
    /// The "at least one" clause matters: a candidate every oracle declined is
    /// unverified, not good. Without it, the easiest way to score well is to
    /// propose something nothing can check.
    pub fn accepted(&self) -> bool {
        self.verdicts.iter().any(|(_, v)| v.judged())
            && self.verdicts.iter().all(|(_, v)| !matches!(v, Verdict::Fail(_)))
    }

    /// Oracles that had no opinion. The unverified surface of this candidate.
    pub fn silent(&self) -> Vec<&str> {
        self.verdicts
            .iter()
            .filter(|(_, v)| !v.judged())
            .map(|(n, _)| n.as_str())
            .collect()
    }

    pub fn failures(&self) -> Vec<(&str, &str)> {
        self.verdicts
            .iter()
            .filter_map(|(n, v)| match v {
                Verdict::Fail(why) => Some((n.as_str(), why.as_str())),
                _ => None,
            })
            .collect()
    }
}

/// Put a candidate to every oracle in the panel.
pub fn judge_all<C>(oracles: &[&dyn Oracle<C>], candidate: &C) -> Panel {
    Panel {
        verdicts: oracles
            .iter()
            .map(|o| (o.name().to_string(), o.judge(candidate)))
            .collect(),
    }
}

/// The plan checker as an oracle — [`crate::state::check_plan`] behind the
/// common shape, so a proposer can be scored on legality alongside anything
/// else without knowing this module exists.
pub struct PlanOracle {
    pub start: crate::state::State,
}

impl Oracle<Vec<crate::state::Op>> for PlanOracle {
    fn name(&self) -> &str {
        "plan"
    }
    fn judge(&self, plan: &Vec<crate::state::Op>) -> Verdict {
        if plan.is_empty() {
            // An empty plan is legal and achieves nothing. Declining rather
            // than passing keeps it from being the highest-scoring proposal.
            return Verdict::NotApplicable("empty plan".into());
        }
        match crate::state::check_plan(&self.start, plan) {
            Ok(_) => Verdict::Pass,
            Err(e) => Verdict::Fail(format!(
                "step {} ({:?}) unmet: {:?}",
                e.step, e.op, e.unmet
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Fact, Op, State};

    struct Always(Verdict, &'static str);
    impl<C> Oracle<C> for Always {
        fn name(&self) -> &str {
            self.1
        }
        fn judge(&self, _: &C) -> Verdict {
            self.0.clone()
        }
    }

    #[test]
    fn a_candidate_nothing_can_judge_is_not_accepted() {
        // The failure this guards: if silence counted as approval, the
        // best-scoring proposal would be one no oracle understands.
        let a = Always(Verdict::NotApplicable("no opinion".into()), "compile");
        let b = Always(Verdict::NotApplicable("no opinion".into()), "tests");
        let p = judge_all(&[&a as &dyn Oracle<u8>, &b], &0u8);
        assert!(!p.accepted());
        assert_eq!(p.silent(), vec!["compile", "tests"]);
    }

    #[test]
    fn one_judgement_is_enough_and_one_failure_is_fatal() {
        let pass = Always(Verdict::Pass, "compile");
        let quiet = Always(Verdict::NotApplicable("n/a".into()), "plan");
        let p = judge_all(&[&pass as &dyn Oracle<u8>, &quiet], &0u8);
        assert!(p.accepted(), "judged by one, rejected by none");
        assert_eq!(p.silent(), vec!["plan"], "the unverified surface stays visible");

        let fail = Always(Verdict::Fail("does not build".into()), "compile");
        let p = judge_all(&[&fail as &dyn Oracle<u8>, &quiet], &0u8);
        assert!(!p.accepted());
        assert_eq!(p.failures(), vec![("compile", "does not build")]);
    }

    #[test]
    fn the_plan_checker_answers_through_the_common_shape() {
        let start = State::new([Fact::Tracked { path: "in.bin".into() }]);
        let o = PlanOracle { start };
        let good = vec![
            Op::Generate { path: "out.zst".into(), from: "in.bin".into() },
            Op::DeleteSuperseded { path: "in.bin".into() },
        ];
        assert_eq!(o.judge(&good), Verdict::Pass);

        // Reversed: the deletion precedes the artifact that supersedes it.
        let bad: Vec<Op> = good.into_iter().rev().collect();
        match o.judge(&bad) {
            // The reason names the step, so a proposer can repair rather than
            // resample.
            Verdict::Fail(why) => assert!(why.starts_with("step 0"), "{why}"),
            v => panic!("expected a located failure, got {v:?}"),
        }
    }

    #[test]
    fn an_empty_plan_declines_rather_than_passing() {
        let o = PlanOracle { start: State::new([]) };
        assert!(!o.judge(&vec![]).judged(), "doing nothing is not a legal plan");
    }
}
