//! state — the repo as facts and operators, so a plan can be CHECKED.
//!
//! The rest of this crate answers "how do I build this crate" — an action
//! vocabulary. This is the layer under it: what is true of the repo right
//! now, and which changes are legal from here.
//!
//! ## Why not just learn the tool calls
//!
//! A model trained on recorded tool calls learns a policy: given a situation
//! that looks like this, type that. It is fast to train and it degrades badly
//! — the tools change, the repo changes, and the policy has no way to notice
//! it is now wrong because it never represented WHY the call was valid.
//!
//! An operator carries its preconditions. `DeleteSuperseded` is legal only
//! when something else already holds the content; `WireModule` only when the
//! module exists and nothing reaches it. A plan over these is checkable
//! without running anything, and — the part that matters for bootstrapping —
//! checkable *without a human or a model in the loop*. A proposer can be
//! wrong as often as it likes; the checker is what makes the wrongness cheap.
//!
//! ## This is grounded in a real mistake
//!
//! Earlier in this repo's history someone (me) ran `git rm` on twenty
//! `scene.bin` files because a packed form existed — on a branch where the
//! packs had not landed yet. That would have destroyed the only copy of that
//! geometry. It was caught by looking, one command later.
//!
//! `DeleteSuperseded` cannot express that mistake. Its precondition is that
//! the superseding artifact is present *in this state*, so the plan fails to
//! validate before anything runs. That is the whole argument for this file:
//! the class of error is removed by the representation rather than by
//! remembering to check.

use std::collections::BTreeSet;

/// One thing known to be true.
///
/// Deliberately a small closed set. An open-ended fact type invites encoding
/// prose as data, and a precondition that reads a free-text field is a
/// precondition nothing can check.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fact {
    /// A source module exists in a crate.
    Module { krate: String, module: String },
    /// Something outside the module references it — the wiring_check question.
    Reachable { krate: String, module: String },
    /// A path is tracked in git.
    Tracked { path: String },
    /// `path`'s content is recoverable from `by`, which must also be tracked.
    Superseded { path: String, by: String },
    /// A test run has been observed green for this crate.
    TestsPass { krate: String },
    /// The artifact at `path` was produced by a tool and can be regenerated.
    Derived { path: String },
}

/// What is true right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    facts: BTreeSet<Fact>,
}

impl State {
    pub fn new(facts: impl IntoIterator<Item = Fact>) -> State {
        State { facts: facts.into_iter().collect() }
    }
    pub fn holds(&self, f: &Fact) -> bool {
        self.facts.contains(f)
    }
    pub fn insert(&mut self, f: Fact) {
        self.facts.insert(f);
    }
    pub fn remove(&mut self, f: &Fact) {
        self.facts.remove(f);
    }
    pub fn len(&self) -> usize {
        self.facts.len()
    }
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = &Fact> {
        self.facts.iter()
    }
}

/// A legal change to the repo.
///
/// Each variant is a change someone actually makes here, not a generic file
/// operation — `Edit(path)` would be true of everything and constrain nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Make an unreachable module reachable from its app.
    WireModule { krate: String, module: String },
    /// Remove a tracked file whose content lives elsewhere.
    DeleteSuperseded { path: String },
    /// Record that a tool produced `path` from `from`.
    Generate { path: String, from: String },
    /// Run the crate's tests.
    RunTests { krate: String },
}

/// Why an operator could not be applied.
///
/// Carries the missing fact rather than a message, so a caller can act on it —
/// a planner that reads "needs Tracked(x)" can go and satisfy it, where one
/// reading "precondition failed" can only give up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unmet {
    Missing(Fact),
    /// The fact must NOT hold and does.
    Present(Fact),
    /// A structural problem the fact base cannot express.
    Invalid(String),
}

impl Op {
    /// What must be true before this may run.
    pub fn preconditions(&self, s: &State) -> Vec<Unmet> {
        let mut out = Vec::new();
        match self {
            Op::WireModule { krate, module } => {
                let m = Fact::Module { krate: krate.clone(), module: module.clone() };
                if !s.holds(&m) {
                    out.push(Unmet::Missing(m));
                }
                let r = Fact::Reachable { krate: krate.clone(), module: module.clone() };
                // Wiring something already wired is not an error worth
                // running; it is a plan that has drifted from the state.
                if s.holds(&r) {
                    out.push(Unmet::Present(r));
                }
            }
            Op::DeleteSuperseded { path } => {
                let t = Fact::Tracked { path: path.clone() };
                if !s.holds(&t) {
                    out.push(Unmet::Missing(t));
                }
                // THE PRECONDITION THIS FILE EXISTS FOR. Something must say
                // where the content went, and that something must itself be
                // present — a Superseded fact naming an absent artifact is
                // exactly the near-miss described at the top.
                let by = s.iter().find_map(|f| match f {
                    Fact::Superseded { path: p, by } if p == path => Some(by.clone()),
                    _ => None,
                });
                match by {
                    None => out.push(Unmet::Invalid(format!(
                        "nothing supersedes {path}; deleting it loses the only copy"
                    ))),
                    Some(by) => {
                        let b = Fact::Tracked { path: by.clone() };
                        if !s.holds(&b) {
                            out.push(Unmet::Missing(b));
                        }
                    }
                }
            }
            Op::Generate { from, .. } => {
                let f = Fact::Tracked { path: from.clone() };
                if !s.holds(&f) {
                    out.push(Unmet::Missing(f));
                }
            }
            Op::RunTests { .. } => {}
        }
        out
    }

    pub fn applicable(&self, s: &State) -> bool {
        self.preconditions(s).is_empty()
    }

    /// The state after this runs. Only call when `applicable`.
    pub fn apply(&self, s: &State) -> State {
        let mut n = s.clone();
        match self {
            Op::WireModule { krate, module } => {
                n.insert(Fact::Reachable { krate: krate.clone(), module: module.clone() });
                // Wiring changes code, so a previously green crate is no
                // longer known green. Dropping this is how a plan "proves"
                // something it never re-checked.
                n.remove(&Fact::TestsPass { krate: krate.clone() });
            }
            Op::DeleteSuperseded { path } => {
                n.remove(&Fact::Tracked { path: path.clone() });
            }
            Op::Generate { path, from } => {
                n.insert(Fact::Tracked { path: path.clone() });
                n.insert(Fact::Derived { path: path.clone() });
                n.insert(Fact::Superseded { path: from.clone(), by: path.clone() });
            }
            Op::RunTests { krate } => {
                n.insert(Fact::TestsPass { krate: krate.clone() });
            }
        }
        n
    }
}

/// Where a plan first stops being legal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError {
    /// Index of the failing step. The point of validating rather than running:
    /// knowing WHICH step is wrong, not merely that the plan is.
    pub step: usize,
    pub op: Op,
    pub unmet: Vec<Unmet>,
}

/// Validate a plan against a state without performing anything.
///
/// Returns the final state on success. This is the function that makes the
/// whole representation worth having: a proposer may be a model, a search, or
/// a person, and none of them has to be trusted, because being wrong is caught
/// here for free.
pub fn check_plan(start: &State, plan: &[Op]) -> Result<State, PlanError> {
    let mut s = start.clone();
    for (i, op) in plan.iter().enumerate() {
        let unmet = op.preconditions(&s);
        if !unmet.is_empty() {
            return Err(PlanError { step: i, op: op.clone(), unmet });
        }
        s = op.apply(&s);
    }
    Ok(s)
}

/// Does the state satisfy every goal fact?
pub fn satisfies(s: &State, goals: &[Fact]) -> bool {
    goals.iter().all(|g| s.holds(g))
}

/// Goals not yet met — what a planner still has to achieve.
pub fn remaining<'a>(s: &State, goals: &'a [Fact]) -> Vec<&'a Fact> {
    goals.iter().filter(|g| !s.holds(g)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracked(p: &str) -> Fact {
        Fact::Tracked { path: p.into() }
    }
    fn module(k: &str, m: &str) -> Fact {
        Fact::Module { krate: k.into(), module: m.into() }
    }
    fn reachable(k: &str, m: &str) -> Fact {
        Fact::Reachable { krate: k.into(), module: m.into() }
    }

    #[test]
    fn deleting_without_a_superseder_is_refused_by_the_representation() {
        // The real mistake this encodes: `git rm` on twenty scene.bin because
        // a packed form existed — on a branch where the packs had not landed.
        // Nothing about the file itself says that is wrong; only the absence
        // of a Superseded fact does.
        let s = State::new([tracked("bots/pyrachnid/f00/scene.bin")]);
        let op = Op::DeleteSuperseded { path: "bots/pyrachnid/f00/scene.bin".into() };
        let unmet = op.preconditions(&s);
        assert_eq!(unmet.len(), 1);
        assert!(matches!(&unmet[0], Unmet::Invalid(m) if m.contains("only copy")));
        assert!(!op.applicable(&s));
    }

    #[test]
    fn a_superseder_that_is_not_present_does_not_count() {
        // The subtler half of the same mistake. A Superseded fact can be
        // stale: the pack exists on another branch, or was planned and never
        // written. Naming it is not having it.
        let s = State::new([
            tracked("f00/scene.bin"),
            Fact::Superseded { path: "f00/scene.bin".into(), by: "bots.pack".into() },
        ]);
        let op = Op::DeleteSuperseded { path: "f00/scene.bin".into() };
        assert_eq!(
            op.preconditions(&s),
            vec![Unmet::Missing(tracked("bots.pack"))],
            "the superseder must itself be tracked"
        );

        // With the pack actually present it becomes legal.
        let mut s2 = s.clone();
        s2.insert(tracked("bots.pack"));
        assert!(op.applicable(&s2));
        assert!(!op.apply(&s2).holds(&tracked("f00/scene.bin")));
    }

    #[test]
    fn generating_establishes_the_fact_that_makes_deletion_legal() {
        // The two-step plan that IS correct, and the order is the content:
        // generate, then delete. Reversed it fails at step 0.
        let s = State::new([tracked("f00/scene.bin")]);
        let plan = vec![
            Op::Generate { path: "bots.pack".into(), from: "f00/scene.bin".into() },
            Op::DeleteSuperseded { path: "f00/scene.bin".into() },
        ];
        let end = check_plan(&s, &plan).expect("generate-then-delete is legal");
        assert!(end.holds(&tracked("bots.pack")));
        assert!(!end.holds(&tracked("f00/scene.bin")));

        let reversed: Vec<Op> = plan.into_iter().rev().collect();
        let err = check_plan(&s, &reversed).unwrap_err();
        assert_eq!(err.step, 0, "the deletion is illegal before the pack exists");
    }

    #[test]
    fn a_plan_reports_which_step_broke_not_merely_that_one_did() {
        // Validating instead of running is only useful if it localises. A
        // planner that learns "the plan is wrong" cannot repair it; one that
        // learns "step 2 needs Tracked(x)" can.
        let s = State::new([module("voiceterm", "mail"), module("voiceterm", "viz")]);
        let plan = vec![
            Op::WireModule { krate: "voiceterm".into(), module: "mail".into() },
            Op::WireModule { krate: "voiceterm".into(), module: "viz".into() },
            Op::WireModule { krate: "voiceterm".into(), module: "absent".into() },
        ];
        let err = check_plan(&s, &plan).unwrap_err();
        assert_eq!(err.step, 2);
        assert_eq!(err.unmet, vec![Unmet::Missing(module("voiceterm", "absent"))]);
    }

    #[test]
    fn wiring_invalidates_a_green_test_result() {
        // A plan must not carry a stale proof. Changing code drops TestsPass,
        // so a goal of "wired AND green" forces the test back into the plan
        // rather than letting an earlier run vouch for later edits.
        let s = State::new([
            module("voiceterm", "mail"),
            Fact::TestsPass { krate: "voiceterm".into() },
        ]);
        let after = Op::WireModule { krate: "voiceterm".into(), module: "mail".into() }
            .apply(&s);
        assert!(!after.holds(&Fact::TestsPass { krate: "voiceterm".into() }));

        let goals = vec![reachable("voiceterm", "mail"),
                         Fact::TestsPass { krate: "voiceterm".into() }];
        assert!(!satisfies(&after, &goals));
        assert_eq!(remaining(&after, &goals).len(), 1);
        let done = Op::RunTests { krate: "voiceterm".into() }.apply(&after);
        assert!(satisfies(&done, &goals));
    }

    #[test]
    fn wiring_something_already_wired_is_a_drifted_plan_not_a_no_op() {
        let s = State::new([module("v", "mail"), reachable("v", "mail")]);
        let op = Op::WireModule { krate: "v".into(), module: "mail".into() };
        assert_eq!(op.preconditions(&s), vec![Unmet::Present(reachable("v", "mail"))]);
    }

    #[test]
    fn the_unmet_reason_names_the_fact_so_a_planner_can_act_on_it() {
        // The difference between a checker and an error message: Missing(f)
        // tells a search what to go and achieve.
        let s = State::new([]);
        let op = Op::Generate { path: "out.zst".into(), from: "in.bin".into() };
        match &op.preconditions(&s)[0] {
            Unmet::Missing(Fact::Tracked { path }) => assert_eq!(path, "in.bin"),
            other => panic!("expected a nameable missing fact, got {other:?}"),
        }
    }
}
