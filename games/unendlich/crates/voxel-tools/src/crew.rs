//! crew — who plans, who builds, who narrates.
//!
//! ## Borrowed vocabulary, different relationship
//!
//! `logscope`'s `modelroles.rs` already solved the naming half of this, and
//! its point stands: the engine had three vocabularies (`draft`/`target`,
//! `primary`/`delegate`, `liaison`) for the same jobs and nobody could hold
//! the mapping. So [`Role::Liaison`] keeps its name and its meaning exactly.
//!
//! But logscope's Disciple/Thinker pair is **speculative decoding**: the
//! Disciple proposes tokens, the Thinker verifies every one, and
//! `Role::is_provisional` exists to make that obvious. Voxel building wants
//! something different, and calling it Disciple would import the wrong
//! promise:
//!
//! | | logscope (speculative) | here (delegated) |
//! |---|---|---|
//! | small model's output | provisional, verified token by token | **final**, never seen by the big model |
//! | what the big model does | verifies everything | plans, then steps back |
//! | what makes it safe | the Thinker | **the validators** |
//! | why use the small one | latency | context length, and cost per step |
//!
//! The last two rows are the whole design. A 3B model can be trusted to emit
//! the blocks for "just the canopy" WITHOUT a larger model checking each
//! line, because [`crate::scheme::Scheme::validate`] and
//! [`crate::job::check_line`] check them mechanically — negative extents,
//! materials that do not exist, a canopy the same block as the trunk. Those
//! two modules are the prerequisite for this one: without them, delegating to
//! a small model is just hoping.
//!
//! ## The split, concretely
//!
//! * **Architect** — large, short context, called rarely. Produces the
//!   [`crate::job::Job`] (what the parts are, in what order) and the
//!   [`crate::scheme::Scheme`] (what each part is made of). Structure and
//!   judgement: a few hundred tokens of output for a whole build.
//! * **Mason** — small, LONG context, called once per step. Turns one step
//!   into plan lines. Mechanical, high-volume, and the thing that actually
//!   ran out of room at 5200 tokens.
//! * **Liaison** — small and fast, narrates progress. Decides nothing.
//!
//! ## Assignment is by CONTEXT, not by memory
//!
//! logscope's `auto_assign` picks by a GiB budget, which is right when the
//! constraint is fitting a model in VRAM. It is the wrong axis here. The
//! observed failure was a 5200-token truncation, so the Mason's binding
//! constraint is its context window — and the biggest model is often the one
//! with the *shortest* usable context once a long readback is in it. A crew
//! that assigned the largest model to the Mason seat would reproduce the bug
//! it exists to fix.
//!
//! This module takes [`ModelSpec`]s from the host rather than parsing sizes
//! out of model names. logscope already has `score_from_name` and copying it
//! would be duplication for its own sake; the host knows the real numbers
//! from the loaded model anyway, and a name is a guess.

use crate::job::{Job, STEP_TOKENS};

/// What a configured model is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Large. Decides the parts and the materials; writes no blocks.
    Architect,
    /// Small, long context. Turns one step into blocks.
    Mason,
    /// Small and fast. Reports progress, decides nothing.
    Liaison,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Architect => "Architect",
            Role::Mason => "Mason",
            Role::Liaison => "Liaison",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Role::Architect => "Large. Decides what the parts are and what each is made of, then \
steps back. Writes no blocks, and is asked once per build rather than once per step.",
            Role::Mason => "Small, with the longest context available. Turns one part at a time \
into blocks. Its output is final — nothing re-reads it — so the checks, not a bigger model, are \
what keep it honest.",
            Role::Liaison => "Small and fast. Says what the build is doing while you wait. Never \
decides anything.",
        }
    }

    /// logscope's name for the nearest equivalent, so the two vocabularies
    /// can be reconciled rather than silently diverging.
    ///
    /// The Mason has NO equivalent: logscope's Disciple is provisional and
    /// verified, and a Mason's output is neither. Returning None says that
    /// rather than pretending a mapping exists.
    pub fn logscope_role(self) -> Option<&'static str> {
        match self {
            Role::Architect => Some("Thinker"),
            Role::Liaison => Some("Liaison"),
            Role::Mason => None,
        }
    }

    /// Is this role's output subject to another MODEL's approval?
    ///
    /// False for every role here, unlike logscope's Disciple. The Mason's
    /// work is checked by `scheme::validate` and `job::check_line` — code,
    /// not a model. Stating it as a method so the difference from logscope is
    /// testable rather than only documented.
    pub fn is_provisional(self) -> bool {
        false
    }
}

/// What the host knows about a loaded model.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelSpec {
    pub name: String,
    /// Billions of parameters. 0 when unknown.
    pub params_b: f32,
    /// Usable context window in tokens — the number that actually decides
    /// whether a Mason can finish a step.
    pub ctx_tokens: u32,
}

impl ModelSpec {
    pub fn new(name: impl Into<String>, params_b: f32, ctx_tokens: u32) -> ModelSpec {
        ModelSpec { name: name.into(), params_b, ctx_tokens }
    }
}

/// Who is doing what.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Crew {
    pub architect: Option<ModelSpec>,
    pub mason: Option<ModelSpec>,
    pub liaison: Option<ModelSpec>,
}

/// Why a crew will not work.
#[derive(Debug, Clone, PartialEq)]
pub enum CrewFault {
    /// No model at all for a role that needs one.
    Unfilled(Role),
    /// The Mason cannot hold a step plus its readback. The 5200-token
    /// truncation, predicted before it happens.
    MasonContextTooSmall { have: u32, need: u32 },
    /// One model in every seat. Allowed, but worth saying — it is the
    /// single-model mode, and the whole point of a crew is that it is not.
    SoloCrew(String),
}

impl CrewFault {
    pub fn explain(&self) -> String {
        match self {
            CrewFault::Unfilled(r) => format!("no {} assigned — {}", r.label(), r.describe()),
            CrewFault::MasonContextTooSmall { have, need } => format!(
                "the Mason's context is {have} tokens and a step needs about {need}. \
                 Either load a longer-context model for the Mason, or the build will be \
                 cut off mid-step the way it was at {} tokens.",
                crate::job::REPLY_TOKENS),
            CrewFault::SoloCrew(n) => format!(
                "`{n}` is doing every job. That works, but planning and detailing want \
                 different models — a large one to decide the parts, a long-context one to fill them."),
        }
    }
}

/// Headroom a Mason needs beyond the step's own output: the system prompt,
/// the job readback, and the step instruction.
///
/// Generous on purpose. The failure mode is asymmetric — an over-cautious
/// estimate costs one extra step, and an under-cautious one costs the whole
/// reply, which is what happened.
pub const MASON_OVERHEAD_TOKENS: u32 = 1400;

impl Crew {
    /// Assign roles from the available models.
    ///
    /// * Architect — most parameters. Judgement per token is what it is for.
    /// * Mason — LONGEST CONTEXT, tie-broken by smaller size. Deliberately
    ///   not the biggest model: the constraint is room to finish a step, and
    ///   a large short-context model in this seat is the observed bug.
    /// * Liaison — smallest. It only narrates.
    ///
    /// With one model it fills every seat and reports [`CrewFault::SoloCrew`]
    /// from [`Self::check`] — degrading to today's behaviour rather than
    /// refusing to run.
    pub fn assign(models: &[ModelSpec]) -> Crew {
        if models.is_empty() {
            return Crew::default();
        }
        let architect = models
            .iter()
            .max_by(|a, b| a.params_b.total_cmp(&b.params_b))
            .cloned();
        let mason = models
            .iter()
            .max_by(|a, b| {
                a.ctx_tokens
                    .cmp(&b.ctx_tokens)
                    // Same context: prefer the smaller model. It is cheaper
                    // per step and a step is mechanical work.
                    .then(b.params_b.total_cmp(&a.params_b))
            })
            .cloned();
        let liaison = models
            .iter()
            .min_by(|a, b| a.params_b.total_cmp(&b.params_b))
            .cloned();
        Crew { architect, mason, liaison }
    }

    /// Tokens a Mason needs for one step of `job`.
    ///
    /// The readback is included because it grows — slowly, by design
    /// (`Job::readback` is O(steps)) — and a crew check that ignored it would
    /// pass at step one and fail at step six.
    pub fn mason_need(job: &Job) -> u32 {
        // 4 chars/token is the usual rough conversion and is close enough for
        // a headroom check; being wrong here costs a warning, not a build.
        let readback = (job.readback().len() as u32) / 4;
        STEP_TOKENS + MASON_OVERHEAD_TOKENS + readback
    }

    /// Is this crew able to run `job`?
    pub fn check(&self, job: &Job) -> Vec<CrewFault> {
        let mut out = Vec::new();
        for (role, slot) in [
            (Role::Architect, &self.architect),
            (Role::Mason, &self.mason),
        ] {
            if slot.is_none() {
                out.push(CrewFault::Unfilled(role));
            }
        }
        if let Some(m) = &self.mason {
            let need = Self::mason_need(job);
            if m.ctx_tokens < need {
                out.push(CrewFault::MasonContextTooSmall { have: m.ctx_tokens, need });
            }
        }
        let names: Vec<&str> = [&self.architect, &self.mason, &self.liaison]
            .iter()
            .filter_map(|s| s.as_ref().map(|m| m.name.as_str()))
            .collect();
        if names.len() == 3 && names.iter().all(|n| *n == names[0]) {
            out.push(CrewFault::SoloCrew(names[0].to_string()));
        }
        out
    }

    /// Which model answers a given role, falling back to whatever exists.
    ///
    /// A missing Mason falls back to the Architect rather than failing: a
    /// slow build beats no build, and the caller has already been told by
    /// [`Self::check`].
    pub fn for_role(&self, role: Role) -> Option<&ModelSpec> {
        let first = match role {
            Role::Architect => &self.architect,
            Role::Mason => &self.mason,
            Role::Liaison => &self.liaison,
        };
        first
            .as_ref()
            .or(self.architect.as_ref())
            .or(self.mason.as_ref())
            .or(self.liaison.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheme::Archetype;

    fn specs() -> Vec<ModelSpec> {
        vec![
            ModelSpec::new("llama-3.2-3b-instruct-q4", 3.0, 32_768),
            ModelSpec::new("qwen3-30b-a3b", 30.0, 8_192),
            ModelSpec::new("tinyllama-1.1b", 1.1, 2_048),
        ]
    }

    /// THE POINT OF THE MODULE. The Mason is chosen for CONTEXT, not size.
    /// Putting the 30B short-context model in the Mason seat reproduces the
    /// 5200-token truncation this exists to prevent.
    #[test]
    fn the_mason_is_the_longest_context_model_not_the_biggest() {
        let c = Crew::assign(&specs());
        assert_eq!(c.mason.as_ref().unwrap().name, "llama-3.2-3b-instruct-q4");
        assert_eq!(c.architect.as_ref().unwrap().name, "qwen3-30b-a3b");
        assert_eq!(c.liaison.as_ref().unwrap().name, "tinyllama-1.1b");
    }

    /// Same context: take the cheaper model, since a step is mechanical.
    #[test]
    fn a_context_tie_goes_to_the_smaller_model() {
        let m = vec![
            ModelSpec::new("big", 70.0, 16_384),
            ModelSpec::new("small", 7.0, 16_384),
        ];
        assert_eq!(Crew::assign(&m).mason.unwrap().name, "small");
    }

    /// The truncation, predicted instead of suffered.
    #[test]
    fn a_short_context_mason_is_reported_before_the_build_starts() {
        let job = Job::plan("a tree", Archetype::Tree);
        let c = Crew {
            architect: Some(ModelSpec::new("big", 30.0, 8192)),
            mason: Some(ModelSpec::new("cramped", 3.0, 2048)),
            liaison: None,
        };
        let faults = c.check(&job);
        let f = faults.iter().find(|f| matches!(f, CrewFault::MasonContextTooSmall { .. }))
            .expect("must predict the cut-off");
        assert!(f.explain().contains("cut off"), "{}", f.explain());
        assert!(f.explain().contains("5200"), "names the observed limit: {}", f.explain());

        // A roomy Mason passes.
        let roomy = Crew { mason: Some(ModelSpec::new("roomy", 3.0, 32_768)), ..c };
        assert!(!roomy.check(&job).iter()
            .any(|f| matches!(f, CrewFault::MasonContextTooSmall { .. })));
    }

    /// The need must include the readback, or the check passes at step one
    /// and fails at step six.
    #[test]
    fn the_context_need_accounts_for_a_growing_readback() {
        let empty = Job::plan("a tree", Archetype::Tree);
        let mut partway = Job::plan("a tree", Archetype::Tree);
        for i in 0..3 {
            partway.record(i, 400, &[18], Some(([0, 0, 0], [9, 9, 9])));
        }
        assert!(Crew::mason_need(&partway) >= Crew::mason_need(&empty));
        // ...but only just: the readback is O(steps), so the need must not
        // balloon. This is what keeps a long build feasible at all.
        assert!(Crew::mason_need(&partway) < Crew::mason_need(&empty) + 400);
    }

    /// One model must still work — degrade, do not refuse.
    #[test]
    fn a_single_model_fills_every_seat_and_says_so() {
        let one = vec![ModelSpec::new("only", 3.0, 32_768)];
        let c = Crew::assign(&one);
        assert!(c.architect.is_some() && c.mason.is_some() && c.liaison.is_some());
        let job = Job::plan("a tree", Archetype::Tree);
        assert!(c.check(&job).iter().any(|f| matches!(f, CrewFault::SoloCrew(_))));
        // And it is a remark, not a blocker: nothing is Unfilled.
        assert!(!c.check(&job).iter().any(|f| matches!(f, CrewFault::Unfilled(_))));
    }

    #[test]
    fn no_models_reports_the_seats_that_matter() {
        let c = Crew::assign(&[]);
        let job = Job::plan("a tree", Archetype::Tree);
        let faults = c.check(&job);
        assert!(faults.contains(&CrewFault::Unfilled(Role::Architect)));
        assert!(faults.contains(&CrewFault::Unfilled(Role::Mason)));
        // A missing Liaison is not a fault -- nothing depends on narration.
        assert!(!faults.contains(&CrewFault::Unfilled(Role::Liaison)));
    }

    /// A missing Mason falls back rather than failing: a slow build beats no
    /// build, and check() has already said so.
    #[test]
    fn a_missing_seat_falls_back_to_whoever_exists() {
        let c = Crew { architect: Some(ModelSpec::new("big", 30.0, 8192)), mason: None, liaison: None };
        assert_eq!(c.for_role(Role::Mason).unwrap().name, "big");
        assert_eq!(c.for_role(Role::Liaison).unwrap().name, "big");
        assert!(Crew::default().for_role(Role::Architect).is_none());
    }

    /// The distinction from logscope, made testable rather than only written
    /// down: nothing here is provisional, because code checks the Mason, not
    /// a larger model.
    #[test]
    fn no_role_here_is_provisional_unlike_logscopes_disciple() {
        for r in [Role::Architect, Role::Mason, Role::Liaison] {
            assert!(!r.is_provisional(), "{r:?}");
        }
        // And the vocabulary reconciles where it can, and admits where it
        // cannot.
        assert_eq!(Role::Architect.logscope_role(), Some("Thinker"));
        assert_eq!(Role::Liaison.logscope_role(), Some("Liaison"));
        assert_eq!(Role::Mason.logscope_role(), None, "a Mason is not a Disciple");
    }
}
