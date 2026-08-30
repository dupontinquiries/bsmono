//! voiceask — the one hop between hearing an operator and asking a panel.
//!
//! ## What this closes
//!
//! Both ends were built and nothing joined them:
//!
//! ```text
//!   mic → whisper_pool → voice::poll_passes → voice_session::poll
//!       → transcriber → VoiceTick.utterances
//!                                  ↑ here
//!   panelvoice::triage(&[Ask], round, budget) → what gets asked
//! ```
//!
//! `voice_session`'s own comment on `utterances` says "turn these into
//! chat turns or commands", and nothing did the second half.
//!
//! ## Why the direction is what it is
//!
//! `panelvoice::Ask` models a DEBATER asking the operator a question.
//! This converts the reverse: the operator speaking into the room while
//! a panel runs. They meet at `triage`, which decides what is worth
//! interrupting for — so an operator interjection has to be expressed in
//! the same currency as a debater's question or the two cannot be ranked
//! against each other.
//!
//! That is the whole reason for going through `Ask` rather than
//! shoving the transcript straight into the prompt: an interjection
//! competes for the same attention budget, and `triage` already knows
//! how to spend it.
//!
//! ## Leverage, and why it is not guessed
//!
//! `Ask::leverage` is "how much the answer would change what this
//! debater does, 0..1". A debater can estimate that about its own work.
//! An operator's remark cannot be scored the same way without a model
//! call, and this module deliberately does not make one — `promptroute`
//! argues the case for tables over model calls in decisions this small,
//! and the same applies here.
//!
//! So leverage comes from FORM, not meaning: a question outranks a
//! statement, and an imperative outranks both, because those are the
//! utterances that change what happens next. Crude, inspectable, and
//! wrong in ways a person can see and correct.

use logscope::panelvoice::Ask;

/// The speaker name used for operator interjections.
///
/// A constant rather than the mic's speaker label: `triage` dedupes and
/// ranks BY `from`, and a fleet with two microphones would otherwise
/// have the same operator competing with themselves for the interrupt
/// budget.
pub const OPERATOR: &str = "operator";

/// Leverage for an utterance, from its shape.
///
/// The ordering is what matters, not the absolute values — `triage`
/// sorts by leverage and takes a budget, so only the ranking is load
/// bearing.
pub fn leverage_of(text: &str) -> f32 {
    let t = text.trim();
    if t.is_empty() {
        return 0.0;
    }
    let lower = t.to_lowercase();
    // An imperative changes what the panel DOES, so it outranks a
    // question, which only changes what it knows.
    const IMPERATIVE: &[&str] = &[
        "stop", "wait", "hold on", "scrap", "drop", "focus on", "instead",
        "don't", "do not", "skip", "use ", "try ", "forget",
    ];
    if IMPERATIVE.iter().any(|k| lower.starts_with(k) || lower.contains(&format!(" {k}"))) {
        return 0.9;
    }
    if t.ends_with('?') {
        return 0.6;
    }
    // A bare statement is context. Worth carrying, rarely worth
    // interrupting for — which is exactly what a low leverage means to
    // `triage`.
    0.3
}

/// Is this worth offering to the panel at all?
///
/// Filler and mic noise reach the transcriber as short fragments, and an
/// interjection budget spent on "um" is one not spent on "stop, that is
/// the wrong file". Length rather than a stopword list: whisper's filler
/// output varies by language and model, and a two-word floor catches it
/// without a list to maintain.
pub fn worth_asking(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty() && t.split_whitespace().count() >= 2
}

/// Convert this frame's finished utterances into asks.
///
/// `round` is the panel's current round — `triage` expires anything
/// older than `ASK_TTL_ROUNDS`, so an interjection from three rounds ago
/// drops out on its own rather than needing to be cleaned up here.
pub fn asks_from(utterances: &[crate::voicefan::Utterance], round: u32) -> Vec<Ask> {
    utterances
        .iter()
        .filter(|u| worth_asking(&u.text))
        .map(|u| Ask {
            from: OPERATOR.to_string(),
            text: u.text.trim().to_string(),
            leverage: leverage_of(&u.text),
            round,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voicefan::Utterance;

    fn utt(text: &str) -> Utterance {
        Utterance {
            speaker: Default::default(),
            name: "Alice".into(),
            text: text.into(),
            t_ms: 0,
        }
    }

    #[test]
    fn an_imperative_outranks_a_question_outranks_a_statement() {
        // The ordering is the whole contract: `triage` sorts by leverage
        // and takes a budget, so only the ranking is load bearing and
        // the absolute numbers are not.
        let stop = leverage_of("stop, that's the wrong file");
        let ask = leverage_of("which file is it reading?");
        let note = leverage_of("the logs look fine to me");
        assert!(stop > ask, "{stop} vs {ask}");
        assert!(ask > note, "{ask} vs {note}");
    }

    #[test]
    fn filler_is_not_worth_the_interrupt_budget() {
        // An interjection budget spent on "um" is one not spent on
        // "stop, that is the wrong file".
        assert!(!worth_asking("um"));
        assert!(!worth_asking("  "));
        assert!(!worth_asking(""));
        assert!(worth_asking("stop there"));
    }

    #[test]
    fn every_interjection_comes_from_one_speaker() {
        // `triage` dedupes and ranks BY `from`. A fleet with two
        // microphones would otherwise have the same operator competing
        // with themselves for the interrupt budget.
        let asks = asks_from(&[utt("stop there"), utt("check the other branch")], 3);
        assert_eq!(asks.len(), 2);
        assert!(asks.iter().all(|a| a.from == OPERATOR));
    }

    #[test]
    fn the_round_is_carried_so_triage_can_expire_it() {
        // Staleness is `triage`'s job, not this module's — an
        // interjection from three rounds ago drops out on its own rather
        // than needing to be cleaned up here.
        let asks = asks_from(&[utt("what about the cache")], 7);
        assert_eq!(asks[0].round, 7);
    }

    #[test]
    fn text_is_trimmed_but_not_otherwise_rewritten() {
        // Whatever the operator said, verbatim. Paraphrasing an
        // interjection before a panel reads it would put words in their
        // mouth at exactly the moment they were trying to correct
        // something.
        let asks = asks_from(&[utt("  use the int8 path  ")], 1);
        assert_eq!(asks[0].text, "use the int8 path");
    }

    #[test]
    fn a_frame_with_nothing_said_produces_nothing() {
        assert!(asks_from(&[], 1).is_empty());
        assert!(asks_from(&[utt("uh")], 1).is_empty());
    }
}
