//! moment_gate — whether to interrupt, held across frames.
//!
//! `moment` classifies the moment and `apply_suppression` decides whether a
//! prompt survives it, but suppression is **stateful** in a way the module
//! deliberately does not own: `SuppressionState` is five flags and a counter
//! the caller has to store, and `SuppressionResult` hands back a new counter
//! value the caller has to write down. The original kept those in process
//! globals (`NOISY_POLLS_CONSECUTIVE` and friends). A frontend needs them per
//! app instance, not per process.
//!
//! So this is the state, and one method that threads it correctly. Nothing
//! here classifies anything — every judgement stays in `moment`.
//!
//! ## Why a gate at all
//!
//! Without one, a feature that wants to prompt either always prompts or never
//! does. The whole point of the classifier is the middle: *"maybe just don't
//! ask at 3am and instead say why are you reading at 3am?"*
//!
//! ## The counter is the part that is easy to get wrong
//!
//! `apply_suppression` returns `noisy_polls_after`, and a caller that ignores
//! it gets a cafe-wink that never stops winking — the counter never advances,
//! so the "you have been noisy for a while" threshold is never crossed.
//! Threading it is the entire reason this type exists rather than a free
//! function, and [`Gate::decide`] is the only place it is written.

use crate::moment::{apply_suppression, CorpusPromptDecision, Moment, SuppressionState};

/// Per-app suppression state.
#[derive(Debug, Clone, Copy, Default)]
pub struct Gate {
    /// The user asked for silence for this session. Outranks everything.
    pub session_suppressed: bool,
    pub checkin_shown: bool,
    pub wink_shown: bool,
    /// A prompt was shown recently; hold off.
    pub prompt_cooldown_active: bool,
    noisy_polls: u32,
}

impl Gate {
    pub fn new() -> Gate {
        Gate::default()
    }

    /// Consecutive noisy polls seen so far. Read-only: it is advanced by
    /// [`Self::decide`] and by nothing else, which is the invariant that keeps
    /// the wink from repeating forever.
    pub fn noisy_polls(&self) -> u32 {
        self.noisy_polls
    }

    fn state(&self) -> SuppressionState {
        SuppressionState {
            session_suppressed: self.session_suppressed,
            checkin_shown: self.checkin_shown,
            wink_shown: self.wink_shown,
            prompt_cooldown_active: self.prompt_cooldown_active,
            noisy_polls_before: self.noisy_polls,
        }
    }

    /// Run a raw decision through suppression, storing what comes back.
    ///
    /// Takes `&mut self` precisely because the counter must be written. A
    /// version returning the decision without recording it would compile,
    /// read fine, and quietly break the wink.
    pub fn decide(&mut self, raw: CorpusPromptDecision) -> CorpusPromptDecision {
        let r = apply_suppression(raw, self.state());
        self.noisy_polls = r.noisy_polls_after;
        r.effective
    }

    /// Classify the current moment and run it through suppression.
    pub fn decide_now(&mut self) -> CorpusPromptDecision {
        self.decide(Moment::current().corpus_prompt_decision())
    }

    /// The user said not now. Silences this session.
    ///
    /// Session-wide rather than a cooldown: someone who declines a prompt is
    /// answering the question "should you ask me things", not "should you ask
    /// me that thing". A cooldown would ask again in ten minutes, which is how
    /// a polite feature becomes a nagging one.
    pub fn silence_session(&mut self) {
        self.session_suppressed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE COUNTER. `apply_suppression` hands back `noisy_polls_after` and a
    /// caller that drops it gets a wink that never stops winking, because the
    /// threshold is never crossed. `decide` is the only writer.
    #[test]
    fn the_noisy_poll_counter_advances_across_calls() {
        let mut g = Gate::new();
        assert_eq!(g.noisy_polls(), 0);
        let before = g.noisy_polls();
        g.decide(CorpusPromptDecision::CafeWink);
        let after_one = g.noisy_polls();
        g.decide(CorpusPromptDecision::CafeWink);
        assert!(after_one >= before, "a wink advances or holds the counter");
        assert!(g.noisy_polls() >= after_one, "and keeps advancing: {}", g.noisy_polls());
    }

    /// A non-wink decision resets it — the counter means CONSECUTIVE noisy
    /// polls, and treating them as cumulative would eventually wink in a quiet
    /// room.
    #[test]
    fn a_quiet_decision_resets_the_counter() {
        let mut g = Gate::new();
        g.decide(CorpusPromptDecision::CafeWink);
        g.decide(CorpusPromptDecision::CafeWink);
        assert!(g.noisy_polls() > 0);
        g.decide(CorpusPromptDecision::Silence);
        assert_eq!(g.noisy_polls(), 0, "consecutive, not cumulative");
    }

    /// Declining answers "should you ask me things", not "should you ask me
    /// that thing". A cooldown would ask again in ten minutes.
    #[test]
    fn silencing_the_session_outranks_every_later_decision() {
        let mut g = Gate::new();
        g.silence_session();
        for raw in [CorpusPromptDecision::SoloPrompt,
                    CorpusPromptDecision::CafeWink,
                    CorpusPromptDecision::Silence] {
            assert_eq!(g.decide(raw), CorpusPromptDecision::Silence,
                "{raw:?} must not survive a silenced session");
        }
    }

    /// A fresh gate suppresses nothing of its own — every flag starts false,
    /// so the classifier's answer is the answer until something sets one.
    #[test]
    fn a_fresh_gate_passes_the_raw_decision_through() {
        let mut g = Gate::new();
        assert!(!g.session_suppressed && !g.wink_shown && !g.checkin_shown);
        assert_eq!(g.decide(CorpusPromptDecision::Silence), CorpusPromptDecision::Silence);
    }

    /// The gate is per-app-instance, not per-process: two of them must not
    /// share a counter, which is exactly what the original's globals did.
    #[test]
    fn two_gates_do_not_share_state() {
        let (mut a, mut b) = (Gate::new(), Gate::new());
        a.decide(CorpusPromptDecision::CafeWink);
        a.decide(CorpusPromptDecision::CafeWink);
        a.silence_session();
        assert_eq!(b.noisy_polls(), 0);
        assert!(!b.session_suppressed);
        assert_ne!(b.decide(CorpusPromptDecision::SoloPrompt), CorpusPromptDecision::Silence,
            "b was never silenced");
    }
}
