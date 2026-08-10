//! gen_profile — generation defaults that do not truncate a thinking model.
//!
//! ## The defaults re-create the bug the guards fixed
//!
//! The looping symptom was fixed properly: the stop-token SET rather than one
//! EOS id, ChatML framing detection, a repeat penalty, and `runaway_stop`. But
//! the chat defaults — `max_tokens: 512`, `n_ctx: 4096`, `temperature: 0.35` —
//! reproduce the *appearance* of it on exactly the models worth running.
//!
//! A thinking model emits **thousands** of `<think>` tokens before its answer.
//! A 512-token cap truncates mid-think every single time, which presents as
//! "it never finishes" — indistinguishable from a loop, and the guards are not
//! at fault. And 0.35 is the repetition-lock regime the TODO already named:
//! *"temp 0.35 alone locks them in"*.
//!
//! ## Detection from the model, not from the user remembering
//!
//! A person who has to know their model is a reasoner before it works has a
//! broken default, not a configuration step. The profile is chosen from the
//! model's own name and its declared context length, both of which the loader
//! already has.
//!
//! ## The 4-gram guard needs a wider window when reasoning
//!
//! `runaway_stop` fires at a 4-gram repeat ratio of 0.5, and legitimate
//! reasoning repeats n-grams — *"wait, let me reconsider"*, restating a
//! sub-goal, re-deriving a step. `repetition.rs` already suspects this: *"it's
//! binary"*. So the threshold is part of the profile rather than a constant, and
//! a thinking model gets a looser one.

/// How a model generates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Profile {
    /// Answers directly.
    Direct,
    /// Emits a reasoning span before answering.
    Thinking,
}

/// Generation settings for one profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GenDefaults {
    pub n_ctx: u32,
    pub max_tokens: u32,
    pub temperature: f32,
    pub repeat_penalty: f32,
    pub min_p: f32,
    /// 4-gram repeat ratio at which `runaway_stop` cuts.
    pub runaway_threshold: f64,
}

impl Profile {
    pub fn defaults(self) -> GenDefaults {
        match self {
            Profile::Direct => GenDefaults {
                n_ctx: 4096, max_tokens: 512, temperature: 0.35,
                repeat_penalty: 1.15, min_p: 0.05, runaway_threshold: 0.5,
            },
            Profile::Thinking => GenDefaults {
                // The think span alone can exceed the old cap several times
                // over; a budget that cannot hold one is a truncation, not a
                // limit.
                n_ctx: 16_384,
                max_tokens: 8_192,
                // 0.6 rather than 0.35: the low temperature is what locks a
                // reasoner into restating its last step.
                temperature: 0.6,
                // Lower penalty, because reasoning legitimately revisits terms
                // and a heavy penalty pushes it off its own chain of thought.
                repeat_penalty: 1.05,
                min_p: 0.05,
                // Looser, since "wait, let me reconsider" repeats n-grams.
                runaway_threshold: 0.75,
            },
        }
    }
}

/// Guess the profile from what the loader already knows.
///
/// Name first, declared context second. Both are heuristics and neither is
/// asked of the user, because a person who must know their model is a reasoner
/// before it works has a broken default rather than a configuration step.
pub fn detect(model_name: &str, declared_ctx: Option<u32>) -> Profile {
    let n = model_name.to_ascii_lowercase();
    // Substrings vendors actually ship. "-thinking", "reasoner", "r1" and the
    // QwQ line are the reliable ones.
    const MARKERS: &[&str] = &["thinking", "reason", "qwq", "-r1", "r1-", "deepthink", "cot"];
    if MARKERS.iter().any(|m| n.contains(m)) { return Profile::Thinking; }
    // A model declaring a very large context is usually built for long
    // generations even when its name says nothing. Weaker signal, so it sits
    // second and needs a high bar.
    if declared_ctx.is_some_and(|c| c >= 131_072) { return Profile::Thinking; }
    Profile::Direct
}

/// Why generation stopped, for the log line that tells you which killer fired.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ended { Eos, CapHit, RunawayGuard }

impl Ended {
    /// What to show when a run ends without an EOS.
    ///
    /// The distinction is the whole point: a cap hit on a thinking model is a
    /// settings problem and a guard cut is a sampling problem, and they need
    /// opposite fixes. Presenting both as "stopped" is what made the original
    /// bug take a week.
    pub fn advice(self, p: Profile) -> Option<&'static str> {
        match (self, p) {
            (Ended::Eos, _) => None,
            (Ended::CapHit, Profile::Thinking) =>
                Some("hit the token cap mid-answer — a thinking model needs a larger max_tokens, \
                      not a different sampler"),
            (Ended::CapHit, Profile::Direct) =>
                Some("hit the token cap — raise max_tokens if the answer was still going"),
            (Ended::RunawayGuard, Profile::Thinking) =>
                Some("the repetition guard cut this; reasoning legitimately repeats n-grams, so \
                      raise runaway_threshold before lowering temperature"),
            (Ended::RunawayGuard, Profile::Direct) =>
                Some("the repetition guard cut this — the model was looping"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thinking_model_gets_a_budget_that_can_hold_a_think_span() {
        // 512 tokens truncates mid-think every time, which presents as "never
        // finishes" and is indistinguishable from the loop the guards fixed.
        let d = Profile::Thinking.defaults();
        assert!(d.max_tokens >= 8_192, "{}", d.max_tokens);
        assert!(d.n_ctx >= 16_384);
        assert!(d.n_ctx > d.max_tokens, "context must hold the answer plus the prompt");
    }

    #[test]
    fn the_direct_defaults_are_unchanged() {
        // A regression here would change behaviour for every non-reasoning
        // model to fix one class of them.
        let d = Profile::Direct.defaults();
        assert_eq!((d.n_ctx, d.max_tokens), (4096, 512));
        assert_eq!(d.temperature, 0.35);
        assert_eq!(d.repeat_penalty, 1.15);
    }

    #[test]
    fn the_thinking_temperature_leaves_the_repetition_lock_regime() {
        // The TODO's own words: "temp 0.35 alone locks them in".
        assert!(Profile::Thinking.defaults().temperature > 0.5);
        assert!(Profile::Thinking.defaults().temperature <= 0.7, "still not incoherent");
    }

    #[test]
    fn a_reasoner_gets_a_lighter_repeat_penalty_not_a_heavier_one() {
        // Reasoning revisits terms by nature; a heavy penalty pushes the model
        // off its own chain of thought, which looks like it lost the plot.
        assert!(Profile::Thinking.defaults().repeat_penalty
                < Profile::Direct.defaults().repeat_penalty);
        assert!(Profile::Thinking.defaults().repeat_penalty >= 1.0, "never a bonus");
    }

    #[test]
    fn the_runaway_guard_is_looser_while_reasoning() {
        // "Wait, let me reconsider" repeats n-grams. repetition.rs already
        // suspects this guard is too binary.
        assert!(Profile::Thinking.defaults().runaway_threshold
                > Profile::Direct.defaults().runaway_threshold);
        assert!(Profile::Thinking.defaults().runaway_threshold < 1.0,
                "still catches a true loop");
    }

    #[test]
    fn the_names_vendors_actually_ship_are_detected() {
        for n in ["Qwen3-30B-A3B-Thinking-2507-Q4_K_M.gguf",
                  "DeepSeek-R1-Distill-Qwen-14B.gguf",
                  "QwQ-32B-Preview-Q4_K_M.gguf",
                  "some-reasoner-7b.gguf"] {
            assert_eq!(detect(n, None), Profile::Thinking, "{n}");
        }
    }

    #[test]
    fn an_ordinary_model_is_not_misdetected() {
        // A false positive costs a 16k context and a hotter sampler on a model
        // that wanted neither.
        for n in ["Llama-3.3-70B-Instruct-Q4_K_M.gguf",
                  "Qwen3-30B-A3B-Coder-Q4_K_M.gguf",
                  "Llama-3.2-1B-Instruct-Q4_K_M.gguf",
                  "GLM-4.7-Flash-Q4_K_S.gguf"] {
            assert_eq!(detect(n, None), Profile::Direct, "{n}");
        }
    }

    #[test]
    fn detection_is_case_insensitive() {
        // GGUF filenames are not normalised.
        assert_eq!(detect("QWEN3-THINKING.GGUF", None), Profile::Thinking);
    }

    #[test]
    fn a_very_long_declared_context_is_a_weaker_second_signal() {
        // Some reasoners ship without a marker in the name; 262K context is
        // built for long generation. The bar is high so ordinary 128K models
        // do not trip it.
        assert_eq!(detect("mystery-model.gguf", Some(262_144)), Profile::Thinking);
        assert_eq!(detect("mystery-model.gguf", Some(131_072)), Profile::Thinking);
        assert_eq!(detect("mystery-model.gguf", Some(32_768)), Profile::Direct);
        assert_eq!(detect("mystery-model.gguf", Some(8_192)), Profile::Direct);
    }

    #[test]
    fn an_empty_name_falls_back_rather_than_panicking() {
        assert_eq!(detect("", None), Profile::Direct);
    }

    #[test]
    fn a_cap_hit_and_a_guard_cut_give_opposite_advice() {
        // They need opposite fixes, and presenting both as "stopped" is what
        // made the original bug take a week to find.
        let cap = Ended::CapHit.advice(Profile::Thinking).unwrap();
        let guard = Ended::RunawayGuard.advice(Profile::Thinking).unwrap();
        assert!(cap.contains("max_tokens"), "{cap}");
        assert!(guard.contains("runaway_threshold"), "{guard}");
        assert_ne!(cap, guard);
    }

    #[test]
    fn a_clean_eos_needs_no_advice() {
        // A line on every successful generation teaches the user to skip them.
        assert!(Ended::Eos.advice(Profile::Thinking).is_none());
        assert!(Ended::Eos.advice(Profile::Direct).is_none());
    }
}
