//! sketchexpand.rs — a large model writes a sketch, a small model expands it.
//!
//! ## The economics, which are better than they look
//!
//! At the measured rates — 40.7 ms/token for Scout at its best, 12.5 ms for a 1B on llama.cpp — a
//! 400-token function costs 16.3 s from the large model alone. Split:
//!
//! ```text
//!   1.5x denser sketch:  15.9 s   (1.03x — not worth the machinery)
//!   3.0x denser sketch:  10.4 s   (1.56x)
//!   6.0x denser sketch:   7.7 s   (2.11x)
//! ```
//!
//! Break-even is at `big_ms / (big_ms - small_ms)` = **1.44x**. Any sketch denser than that pays,
//! which is a low bar — an AST skeleton against formatted Rust clears it easily.
//!
//! ## The sharp edge
//!
//! If expansion is DETERMINISTIC, no model is needed — that is a pretty-printer, and it should be one,
//! because a pretty-printer cannot hallucinate. If expansion is NOT deterministic, the small model is
//! making decisions, which is the job `smallrole` withholds from it.
//!
//! The design only works in the narrow band where the sketch captures every HIGH-stakes decision and
//! leaves only low-stakes ones: local variable names, which of several equivalent forms to use,
//! boilerplate shape. Getting that band wrong in either direction wastes the idea — too much detail and
//! the sketch is just the code, too little and the small model is designing.
//!
//! ## The test that says which side you are on
//!
//! **Round-trip.** Expand the sketch, then re-derive a sketch from the expansion. If they match, the
//! expansion preserved every decision the sketch encoded, and whatever the small model added was by
//! definition not encoded — which is exactly the low-stakes residue the design permits.
//!
//! If they differ, the small model changed something the sketch had specified, and the split is unsafe
//! for that sketch. This is checkable per-generation rather than argued once, which matters because the
//! band is narrow and a given sketch language will sit inside it for some constructs and not others.

/// Is a sketch dense enough for the split to pay?
pub fn worth_splitting(sketch_tokens: usize, code_tokens: usize, big_ms: f32, small_ms: f32) -> bool {
    if sketch_tokens == 0 || big_ms <= small_ms { return false; }
    let alone = code_tokens as f32 * big_ms;
    let split = sketch_tokens as f32 * big_ms + code_tokens as f32 * small_ms;
    split < alone
}

/// The density at which splitting breaks even.
///
/// Below this the machinery costs more than it saves, and the honest answer is to let the large model
/// write the code.
pub fn breakeven_ratio(big_ms: f32, small_ms: f32) -> f32 {
    if big_ms <= small_ms { return f32::INFINITY; }
    big_ms / (big_ms - small_ms)
}

/// Verdict on one expansion.
#[derive(Clone, PartialEq, Debug)]
pub enum RoundTrip {
    /// Re-deriving a sketch from the expansion reproduces the original. Every encoded decision survived.
    Faithful,
    /// The expansion changed something the sketch specified. Unsafe for this sketch.
    Diverged { expected: String, got: String },
    /// No sketch could be derived back — the expansion is not in the sketch language's image.
    Unrecoverable { why: String },
}

impl RoundTrip {
    pub fn is_safe(&self) -> bool { matches!(self, RoundTrip::Faithful) }
}

/// Check an expansion by re-deriving its sketch.
///
/// `derive` is the same function that produced the original sketch, applied to the expansion. Using
/// the SAME function in both directions is what makes this a real check rather than two opinions —
/// a second implementation could differ from the first and the mismatch would prove nothing.
pub fn round_trip(original_sketch: &str, expansion: &str, derive: impl Fn(&str) -> Option<String>)
    -> RoundTrip {
    match derive(expansion) {
        None => RoundTrip::Unrecoverable {
            why: "no sketch could be derived from the expansion — it is outside the sketch language, \
so nothing can be said about which decisions survived".into() },
        Some(s) if s == original_sketch => RoundTrip::Faithful,
        Some(s) => RoundTrip::Diverged { expected: original_sketch.to_string(), got: s },
    }
}

/// What the expander is told.
pub const EXPAND_PROMPT: &str = "\
Expand the sketch below into complete code. The sketch is the SPECIFICATION: every construct it names \
must appear, in the order given, with the types it states.

You may choose local variable names, formatting, and which of several equivalent forms to use. You may \
NOT add a branch, remove a branch, change a type, introduce a dependency, or alter control flow — if \
the sketch seems to be missing something, say so and expand no further. A sketch that was wrong is \
cheaper to fix than an expansion that quietly corrected it, because the correction is invisible to \
whoever wrote the sketch.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_break_even_density_is_low_enough_to_be_reachable() {
        // 1.44x at the measured rates. An AST skeleton against formatted Rust clears that easily, which
        // is what makes the idea worth pursuing rather than a nice thought.
        let b = breakeven_ratio(40.7, 12.5);
        assert!((b - 1.44).abs() < 0.01, "got {b:.2}");
        assert!(worth_splitting(133, 400, 40.7, 12.5), "3x denser pays");
        assert!(!worth_splitting(350, 400, 40.7, 12.5), "1.14x denser does not");
    }

    #[test]
    fn splitting_never_pays_when_the_models_are_the_same_speed() {
        // The degenerate case, and a guard against the machinery being enabled where it cannot help.
        assert!(!worth_splitting(100, 400, 20.0, 20.0));
        assert_eq!(breakeven_ratio(20.0, 20.0), f32::INFINITY);
    }

    #[test]
    fn a_faithful_round_trip_means_every_encoded_decision_survived() {
        // The test that says which side of the sharp edge you are on. If re-deriving the sketch
        // reproduces it, whatever the expander added was by definition NOT encoded — which is exactly
        // the low-stakes residue the design permits.
        let r = round_trip("fn f(u32)->String", "fn f(x: u32) -> String { x.to_string() }",
                           |_| Some("fn f(u32)->String".into()));
        assert!(r.is_safe());
    }

    #[test]
    fn an_expansion_that_changed_a_specified_decision_is_caught() {
        // The small model added a branch, changed a type, or dropped a step. The sketch said otherwise
        // and the round trip proves it — per generation, rather than argued once at design time.
        let r = round_trip("fn f(u32)->String", "fn f(x: u64) -> String { x.to_string() }",
                           |_| Some("fn f(u64)->String".into()));
        assert!(!r.is_safe());
        match r {
            RoundTrip::Diverged { expected, got } => {
                assert!(expected.contains("u32") && got.contains("u64"));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn an_unparseable_expansion_is_UNRECOVERABLE_not_faithful() {
        // Failing to derive must not read as success. This is the direction that would silently accept
        // anything the expander produced.
        let r = round_trip("s", "garbage", |_| None);
        assert!(!r.is_safe());
        if let RoundTrip::Unrecoverable { why } = r {
            assert!(why.contains("nothing can be said"));
        }
    }

    #[test]
    fn the_expand_prompt_names_what_may_and_may_not_change() {
        // "Expand this faithfully" is not actionable. The permitted freedoms and the forbidden changes
        // are both listed, because a model given a vague boundary picks its own.
        for allowed in ["local variable names", "formatting", "equivalent forms"] {
            assert!(EXPAND_PROMPT.contains(allowed), "missing permission: {allowed}");
        }
        for forbidden in ["add a branch", "change a type", "alter control flow"] {
            assert!(EXPAND_PROMPT.contains(forbidden), "missing prohibition: {forbidden}");
        }
    }

    #[test]
    fn a_sketch_that_seems_wrong_stops_rather_than_being_corrected() {
        // The instruction that matters most. An expander that quietly fixes a bad sketch produces code
        // nobody reviews against the sketch they actually wrote — the correction is invisible exactly
        // to the person who could judge it.
        assert!(EXPAND_PROMPT.contains("say so and expand no further"));
        assert!(EXPAND_PROMPT.contains("invisible to whoever wrote the sketch"));
    }
}
