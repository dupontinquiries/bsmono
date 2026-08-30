//! promptroute — which model answers this, and what it is allowed to say.
//!
//! ## The gap this fills
//!
//! Three finished pieces had no path between them:
//!
//! - `liaison` knows the difference between what the OPERATOR said, what
//!   the ORGANIZATION knows, and what a model INFERRED — and enforces
//!   that a model may not mint the middle one.
//! - `workergen` can build varied workers with stances and taboos.
//! - the chat box takes a prompt and sends it to one model.
//!
//! The chat box was the weak link: every prompt went to the same place
//! with the same permissions, so the liaison's epistemic discipline
//! applied to nothing and the workers had nothing to answer. This is the
//! matrix that decides, per prompt, WHICH destination and WHAT the reply
//! is allowed to assert.
//!
//! ## Why routing is a table rather than a model call
//!
//! The design note behind `liaison` is explicit that discipline beats
//! intelligence for this job. Asking a model which model should answer
//! adds a round trip, a failure mode, and a second thing to debug when
//! an answer comes back wrong. A table is inspectable, testable, and
//! wrong in ways you can see.
//!
//! ## The matrix
//!
//! Two axes. WHAT THE PROMPT NEEDS (facts about the run, an opinion, a
//! transformation of text the operator supplied) against WHAT IS
//! AVAILABLE (a local engine, a daemon, a set of workers). The output is
//! a [`Route`]: a destination plus the epistemic ceiling for its reply.
//!
//! The ceiling is the part that matters. A worker riffing on a stance
//! must not be able to say "the index is rebuilt" — that is an
//! organizational fact, and only the runtime may assert one. Routing
//! without a ceiling would hand every model the authority of the
//! runtime.

use crate::liaison::Epistemic;

/// What a prompt is asking for.
///
/// Deliberately coarse. A finer taxonomy would need a classifier, and a
/// classifier is the model call this module exists to avoid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// "What happened in this run?" — answerable only from the
    /// organization's own state.
    RunFact,
    /// "Rewrite this", "explain this" — a transformation of text the
    /// operator already supplied. Needs no knowledge of the world.
    TextWork,
    /// "What should I do?" — an opinion, which is where a worker's
    /// stance earns its keep.
    Judgement,
    /// Anything else. Routed conservatively.
    Open,
}

/// What the app can reach right now.
#[derive(Debug, Clone, Copy, Default)]
pub struct Available {
    /// A daemon or in-process engine is loaded.
    pub engine: bool,
    /// At least one generated worker exists.
    pub workers: bool,
    /// The runtime can answer questions about the run itself.
    pub runtime: bool,
}

/// Where a prompt goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dest {
    /// Answered from the runtime's own state, no model involved.
    Runtime,
    /// The general engine.
    Engine,
    /// A worker, chosen by the caller from the roster.
    Worker,
    /// Nothing can answer this. Better than routing to a model that will
    /// answer anyway.
    Nowhere,
}

/// A routing decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub dest: Dest,
    /// The strongest thing the reply may assert.
    ///
    /// Enforced by the caller against `Epistemic::model_may_assert`; this
    /// module decides the ceiling, `liaison` decides what a ceiling
    /// permits. Two places, one rule each.
    pub ceiling: Epistemic,
}

/// Decide where a prompt goes.
///
/// The conservative direction is always "fewer permissions, and
/// `Nowhere` rather than a guess". A wrong route that refuses is a
/// visible failure; a wrong route that answers confidently is the
/// failure `liaison` was written to prevent.
pub fn route(need: Need, have: Available) -> Route {
    match need {
        // Only the runtime may assert organizational fact, so a question
        // about the run either goes there or goes nowhere. Handing it to
        // a model is exactly how "I've reached out to our team" gets
        // said — the model has no way to know, and every incentive to
        // sound like it does.
        Need::RunFact => {
            if have.runtime {
                Route { dest: Dest::Runtime, ceiling: Epistemic::OrganizationFact }
            } else {
                Route { dest: Dest::Nowhere, ceiling: Epistemic::Unknown }
            }
        }
        // Transforming the operator's own text needs no world knowledge,
        // so the ceiling is what the operator said — the model is
        // rephrasing, not learning.
        Need::TextWork => {
            if have.engine {
                Route { dest: Dest::Engine, ceiling: Epistemic::OperatorFact }
            } else {
                Route { dest: Dest::Nowhere, ceiling: Epistemic::Unknown }
            }
        }
        // A judgement is an inference by definition, and a worker's
        // stance is what makes one worth reading. Falls back to the
        // engine — a plain opinion beats no opinion — but never to the
        // runtime, which has no opinions and should not be made to
        // pretend.
        Need::Judgement => {
            if have.workers {
                Route { dest: Dest::Worker, ceiling: Epistemic::Inference }
            } else if have.engine {
                Route { dest: Dest::Engine, ceiling: Epistemic::Inference }
            } else {
                Route { dest: Dest::Nowhere, ceiling: Epistemic::Unknown }
            }
        }
        // Unclassified: the engine at the lowest useful ceiling. NOT a
        // worker — a stance applied to a question nobody classified is
        // how a character answers a factual question in character.
        Need::Open => {
            if have.engine {
                Route { dest: Dest::Engine, ceiling: Epistemic::Inference }
            } else {
                Route { dest: Dest::Nowhere, ceiling: Epistemic::Unknown }
            }
        }
    }
}

/// Guess what a chat prompt needs, from its text.
///
/// A HEURISTIC, and named as one. It exists so the chat box has a
/// default rather than routing everything to `Open`; it is not a
/// classifier and should lose to an explicit choice by the operator
/// whenever there is one.
///
/// Biased toward `RunFact` on the phrases that ask about this run,
/// because that is the class where a wrong route does damage: a factual
/// question answered by a model is the failure mode, and a factual
/// question answered by "I don't know" is merely unhelpful.
pub fn classify(prompt: &str) -> Need {
    let p = prompt.to_lowercase();
    let asks_about_run = [
        "this run", "the run", "last run", "did it", "how many", "how long",
        "what happened", "why did", "the log", "the error", "did the",
    ]
    .iter()
    .any(|k| p.contains(k));
    if asks_about_run {
        return Need::RunFact;
    }
    let text_work = ["rewrite", "rephrase", "summarize", "summarise", "translate", "shorten"]
        .iter()
        .any(|k| p.contains(k));
    if text_work {
        return Need::TextWork;
    }
    let judgement = ["should i", "what would you", "which is better", "recommend", "your opinion"]
        .iter()
        .any(|k| p.contains(k));
    if judgement {
        return Need::Judgement;
    }
    Need::Open
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Available {
        Available { engine: true, workers: true, runtime: true }
    }

    #[test]
    fn a_question_about_the_run_never_reaches_a_model() {
        // THE ONE THAT MATTERS. Only the runtime may assert
        // organizational fact; a model asked "did the index rebuild"
        // has no way to know and every incentive to sound like it does.
        // That is the "I've reached out to our team" failure, and this
        // is the routing rule that makes it unreachable.
        let r = route(Need::RunFact, all());
        assert_eq!(r.dest, Dest::Runtime);
        assert_eq!(r.ceiling, Epistemic::OrganizationFact);
    }

    #[test]
    fn without_a_runtime_a_run_question_goes_nowhere_rather_than_to_a_model() {
        // The tempting fallback is the engine. It is the wrong one: a
        // refusal is a visible failure, an invented answer is not.
        let have = Available { engine: true, workers: true, runtime: false };
        let r = route(Need::RunFact, have);
        assert_eq!(r.dest, Dest::Nowhere, "fell back to a model");
        assert_eq!(r.ceiling, Epistemic::Unknown);
    }

    #[test]
    fn no_route_lets_a_model_assert_organizational_fact() {
        // The ceiling is the whole point of routing here. Whatever is
        // available, a model-backed destination must never carry the
        // runtime's authority.
        for need in [Need::RunFact, Need::TextWork, Need::Judgement, Need::Open] {
            for runtime in [true, false] {
                let r = route(need, Available { engine: true, workers: true, runtime });
                if matches!(r.dest, Dest::Engine | Dest::Worker) {
                    assert!(
                        r.ceiling.model_may_assert(),
                        "{need:?} routed to {:?} with a ceiling a model may not assert",
                        r.dest
                    );
                }
            }
        }
    }

    #[test]
    fn text_work_is_capped_at_what_the_operator_said() {
        // Rephrasing the operator's own text teaches the model nothing
        // about the world, so the reply may not claim more than the
        // operator did.
        let r = route(Need::TextWork, all());
        assert_eq!(r.dest, Dest::Engine);
        assert_eq!(r.ceiling, Epistemic::OperatorFact);
    }

    #[test]
    fn a_judgement_prefers_a_worker_and_falls_back_to_the_engine() {
        assert_eq!(route(Need::Judgement, all()).dest, Dest::Worker);
        let no_workers = Available { engine: true, workers: false, runtime: true };
        assert_eq!(route(Need::Judgement, no_workers).dest, Dest::Engine);
        assert_eq!(route(Need::Judgement, no_workers).ceiling, Epistemic::Inference);
    }

    #[test]
    fn an_unclassified_prompt_does_not_get_a_stance() {
        // A worker's stance applied to a question nobody classified is
        // how a character answers a factual question in character.
        let r = route(Need::Open, all());
        assert_eq!(r.dest, Dest::Engine, "Open must not reach a worker");
    }

    #[test]
    fn nothing_available_routes_nowhere_for_every_need() {
        let none = Available::default();
        for need in [Need::RunFact, Need::TextWork, Need::Judgement, Need::Open] {
            assert_eq!(route(need, none).dest, Dest::Nowhere, "{need:?}");
        }
    }

    #[test]
    fn the_classifier_leans_toward_run_facts() {
        // Deliberate bias: a factual question answered by a model is the
        // damaging error; one answered "I don't know" is merely
        // unhelpful.
        assert_eq!(classify("what happened in this run?"), Need::RunFact);
        assert_eq!(classify("how many GPU submits did it do"), Need::RunFact);
        assert_eq!(classify("why did the log stop"), Need::RunFact);

        assert_eq!(classify("rewrite this paragraph"), Need::TextWork);
        assert_eq!(classify("should i use int8 here"), Need::Judgement);
        assert_eq!(classify("hello"), Need::Open);
    }

    #[test]
    fn the_classifier_is_case_insensitive() {
        // Chat input is typed by hand; a rule that only fires on
        // lowercase would misroute half of everything.
        assert_eq!(classify("WHAT HAPPENED in This Run"), Need::RunFact);
        assert_eq!(classify("Rewrite This"), Need::TextWork);
    }
}
