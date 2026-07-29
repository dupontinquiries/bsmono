//! smallrole.rs — what a small model should and should not be asked to do.
//!
//! ## The distinction that decides it: knowledge-bound versus context-bound
//!
//! A small model is bad at code for two specific reasons — it does not KNOW the APIs, and it cannot
//! hold a plan across a long output. Neither is fixed by giving it a smaller job; both are fixed by
//! giving it a job whose answer is already present in front of it.
//!
//! So the test for any proposed role is: **could a careful reader with no prior knowledge of this
//! codebase produce the answer from the context alone?** If yes, a small model can attempt it. If it
//! requires knowing what `tokio::select!` does, or deciding how the module should be laid out, it does
//! not matter how small the task is — the model does not have the information and will invent it.
//!
//! ## Why scaffolding is the wrong job, despite feeling like the small one
//!
//! Producing signatures, module layout and types LOOKS like scaffolding because it is short. It is the
//! design, and it is the part that requires knowing what exists. A wrong scaffold is also the most
//! expensive error available: everything downstream is built against it, so a bad signature is not one
//! bad function, it is every call site.
//!
//! The inversion works: the LARGE model produces signatures, types and doc comments; the SMALL model
//! fills bodies against them. A precise signature plus the names in scope turns implementation into a
//! largely mechanical transformation — which is exactly the shape a small model handles.
//!
//! ## Checking is the best fit of all
//!
//! "Does this function do what its doc comment says?" is a classification with the answer in the
//! context. It needs no API knowledge, no plan, and no long output — and `reflex::verdict_from_probs`
//! already reads that answer from logits rather than prose, so it costs one forward pass.

/// A job someone might give a small model.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Job {
    /// Choose module layout, signatures, types. DESIGN.
    Scaffold,
    /// Write a body against a signature that already exists.
    FillBody,
    /// Judge whether code matches a stated contract.
    Check,
    /// Rename, extract, inline — a transformation with a determined answer.
    MechanicalEdit,
    /// Explain what existing code does.
    Summarize,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Fit {
    /// The answer is in the context. A small model can attempt it.
    Suited { why: String },
    /// Needs knowledge the model does not have, or a plan it cannot hold.
    Unsuited { why: String, give_to: &'static str },
}

impl Fit {
    pub fn is_suited(&self) -> bool { matches!(self, Fit::Suited { .. }) }
}

/// Judge a job.
pub fn fit(job: Job) -> Fit {
    match job {
        Job::Scaffold => Fit::Unsuited {
            why: "choosing signatures and layout is DESIGN, and it requires knowing what already \
exists. It looks like the small job because it is short. It is also the most expensive place to be \
wrong: everything downstream is built against it, so a bad signature is not one bad function, it is \
every call site".into(),
            give_to: "thinker",
        },
        Job::FillBody => Fit::Suited {
            why: "a precise signature plus the names in scope makes implementation largely mechanical \
— the types say what must be produced and the scope says what may be used".into(),
        },
        Job::Check => Fit::Suited {
            why: "judging whether code matches a stated contract is a classification with the answer \
in the context: no API knowledge, no plan, no long output. reflex::verdict_from_probs reads it from \
logits in one forward pass".into(),
        },
        Job::MechanicalEdit => Fit::Suited {
            why: "a rename or an extract has a determined answer that the code itself specifies".into(),
        },
        Job::Summarize => Fit::Suited {
            why: "the code being described is present; nothing has to be recalled".into(),
        },
    }
}

/// Does a body-filling task carry enough context to be mechanical?
///
/// The condition under which `FillBody` is actually suited. Without a signature the small model is
/// designing, which is `Scaffold` wearing a different name — and that is how a sensible division of
/// labour quietly becomes the arrangement it was meant to replace.
pub fn body_task_is_well_posed(has_signature: bool, has_scope: bool, has_contract: bool) -> bool {
    has_signature && (has_scope || has_contract)
}

/// The instruction given to a small model filling a body.
pub const FILL_PROMPT: &str = "\
Implement the body of the function whose signature and doc comment are given. Use ONLY the names listed \
as in scope. If the signature and the doc comment disagree, say so and write nothing — a body that \
satisfies one and not the other is worse than no body, because it will pass review by whoever wrote the \
part you matched. Do not change the signature. Do not add dependencies.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffolding_is_the_one_job_to_withhold() {
        // It feels like the small job because it is short. It is the design, it is knowledge-bound, and
        // it is the most expensive place to be wrong.
        let f = fit(Job::Scaffold);
        assert!(!f.is_suited());
        match f {
            Fit::Unsuited { why, give_to } => {
                assert_eq!(give_to, "thinker");
                assert!(why.contains("every call site"), "the cost of a wrong scaffold is stated");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn filling_a_body_against_a_signature_is_suited() {
        // The inversion: the large model scaffolds, the small model fills. A precise signature plus
        // scope turns implementation into a transformation rather than a design decision.
        assert!(fit(Job::FillBody).is_suited());
    }

    #[test]
    fn checking_is_the_best_fit_because_the_answer_is_present() {
        // No API knowledge, no plan, no long output — and reflex reads it from logits in one pass.
        let f = fit(Job::Check);
        assert!(f.is_suited());
        if let Fit::Suited { why } = f { assert!(why.contains("classification")); }
    }

    #[test]
    fn a_body_task_without_a_signature_is_scaffolding_in_disguise() {
        // The way this arrangement quietly reverts. Without a signature the small model is choosing
        // the interface, which is exactly the job just withheld from it.
        assert!(!body_task_is_well_posed(false, true, true), "no signature: it is designing");
        assert!(!body_task_is_well_posed(true, false, false), "signature but nothing to build from");
        assert!(body_task_is_well_posed(true, true, false));
        assert!(body_task_is_well_posed(true, false, true));
    }

    #[test]
    fn the_fill_prompt_forbids_widening_the_task() {
        // A small model given room to change the signature will change it, and then the scaffold it was
        // supposed to be constrained by is gone.
        assert!(FILL_PROMPT.contains("Do not change the signature"));
        assert!(FILL_PROMPT.contains("Do not add dependencies"), "no `ratatouille`");
        assert!(FILL_PROMPT.contains("ONLY the names listed as in scope"));
    }

    #[test]
    fn a_signature_and_doc_that_disagree_produce_NOTHING() {
        // The instruction that matters most, and the least obvious. Satisfying one of two conflicting
        // specifications produces code that passes review by whoever wrote the half it matched —
        // which is a worse outcome than an empty body, because nobody looks again.
        assert!(FILL_PROMPT.contains("say so and write nothing"));
        assert!(FILL_PROMPT.contains("pass review by whoever wrote the part you matched"));
    }
}
