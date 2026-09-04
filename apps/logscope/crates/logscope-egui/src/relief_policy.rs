//! What to do when the prompt has grown enough to cost the reply.
//!
//! # Why this is three states and not a checkbox
//!
//! It was `summarize_on_squeeze: bool`. Two states cannot express the thing the
//! code around it kept asking for: several comments on the compaction path say
//! a human should be in the loop, and a checkbox can only say "always" or
//! "never" — so the human-in-the-loop case had no way to be selected, and the
//! default silently squeezed.
//!
//! A field log makes the cost concrete. One run reported
//! `COMPACTION REQUESTED` 113 times across 120 turns, each one cutting the
//! MIDDLE out of an 11403-token prompt to fit a 7936-token window. The middle
//! is where the task was, so the model spent the run answering "there is no
//! question here" — correctly, by the time it saw the prompt.
//!
//! Neither automatic policy is right for that. Squeezing lost the request;
//! summarizing unattended would have compressed it into a paraphrase nobody
//! checked. [`ReliefPolicy::Ask`] is the state that was missing.

/// How to make room when the reply budget would drop below usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReliefPolicy {
    /// Shrink every reply from here on. The old default, and the reason it is
    /// still the default: it never blocks, and changing what an unattended run
    /// does is a bigger decision than adding the option.
    ///
    /// It walked one session's budget from 2470 tokens down to 814 over
    /// seventeen turns without saying so.
    #[default]
    Squeeze,
    /// Stop, summarize the older turns, continue with the freed room.
    Summarize,
    /// Stop and ASK, once, before doing either.
    ///
    /// The state a checkbox could not hold. For a long unattended run this is
    /// the honest setting: the alternative is discovering after 120 turns that
    /// the prompt was being cut in the middle the whole time.
    Ask,
}

impl ReliefPolicy {
    /// Label for the dropdown.
    pub fn label(self) -> &'static str {
        match self {
            ReliefPolicy::Squeeze => "squeeze replies",
            ReliefPolicy::Summarize => "summarize older turns",
            ReliefPolicy::Ask => "ask each time",
        }
    }

    pub fn hover(self) -> &'static str {
        match self {
            ReliefPolicy::Squeeze =>
                "Keep going and shrink every reply from here on. Never blocks, and never says so — \
                 one session walked its budget from 2470 tokens down to 814 over seventeen turns.",
            ReliefPolicy::Summarize =>
                "Stop, summarize the older turns, and continue with the freed room. Costs a turn \
                 each time, and the summary is not reviewed.",
            ReliefPolicy::Ask =>
                "Stop and ask before doing either. The right setting for a long unattended run: \
                 the alternative is finding out afterwards that the middle of every prompt was cut.",
        }
    }

    /// Every variant, in the order the dropdown should show them.
    pub fn all() -> [ReliefPolicy; 3] {
        [ReliefPolicy::Squeeze, ReliefPolicy::Summarize, ReliefPolicy::Ask]
    }

    /// The engine flag this maps to.
    ///
    /// `Params` carries a bool and the engine does one of two things; `Ask` is
    /// resolved BEFORE it gets there, by the human. Sending `Ask` down as
    /// `true` would summarize without asking, which is the bug this type
    /// exists to prevent — so it is `None` and the caller must resolve it.
    pub fn engine_flag(self) -> Option<bool> {
        match self {
            ReliefPolicy::Squeeze => Some(false),
            ReliefPolicy::Summarize => Some(true),
            ReliefPolicy::Ask => None,
        }
    }

    /// Does this need a human before the run can continue?
    pub fn needs_prompt(self) -> bool {
        self == ReliefPolicy::Ask
    }

    /// Resolve an `Ask` with the human's answer. Any other policy is returned
    /// unchanged — a run already set to Squeeze must not be silently converted
    /// by a stale dialog.
    pub fn resolved(self, chose_summarize: bool) -> ReliefPolicy {
        match self {
            ReliefPolicy::Ask if chose_summarize => ReliefPolicy::Summarize,
            ReliefPolicy::Ask => ReliefPolicy::Squeeze,
            other => other,
        }
    }
}

/// Should an auto-continuing run STOP because a compaction is pending?
///
/// # The mechanism behind a 120-turn run that produced nothing
///
/// The human-in-the-loop offer already exists — "Summarize & continue" beside
/// "Keep squeezing" — and `compact_poll` raises it only when the stream has
/// ended, because taking it mid-stream threw the request away before it could
/// be shown.
///
/// Auto-continue removes the gap. One reply ends and the next turn starts, so
/// `streaming` is true on essentially every frame and the offer is deferred
/// forever. A field log shows the result: `COMPACTION REQUESTED` 113 times
/// across 120 turns, the middle cut out of every prompt, and an offer that
/// nobody could ever act on.
///
/// So `Ask` has to mean something stronger than "show a panel". It means the
/// run WAITS. That is the only reading under which the setting does what its
/// name says on an unattended run, which is precisely when it matters.
///
/// `Squeeze` and `Summarize` never pause — they are the two answers, and a run
/// configured with an answer should not stop to be asked the question.
pub fn should_pause_auto_continue(policy: ReliefPolicy, compaction_pending: bool) -> bool {
    compaction_pending && policy == ReliefPolicy::Ask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_is_never_sent_to_the_engine_as_a_silent_summarize() {
        // The whole reason `engine_flag` returns an Option. `Params` takes a
        // bool; mapping Ask onto `true` would summarize without asking, which
        // is the failure this type was added to prevent.
        assert_eq!(ReliefPolicy::Ask.engine_flag(), None);
        assert_eq!(ReliefPolicy::Squeeze.engine_flag(), Some(false));
        assert_eq!(ReliefPolicy::Summarize.engine_flag(), Some(true));
    }

    #[test]
    fn the_default_is_the_old_behaviour() {
        // Adding an option must not change what an unattended run already does.
        // That is a separate decision from making the option available.
        assert_eq!(ReliefPolicy::default(), ReliefPolicy::Squeeze);
        assert_eq!(ReliefPolicy::default().engine_flag(), Some(false));
        assert!(!ReliefPolicy::default().needs_prompt());
    }

    #[test]
    fn only_ask_stops_the_run() {
        assert!(ReliefPolicy::Ask.needs_prompt());
        assert!(!ReliefPolicy::Squeeze.needs_prompt());
        assert!(!ReliefPolicy::Summarize.needs_prompt());
    }

    #[test]
    fn a_stale_answer_cannot_convert_a_run_that_was_not_asking() {
        // The dialog is async: the policy can change while it is open. An
        // answer arriving for a run now set to Squeeze must not summarize it.
        assert_eq!(ReliefPolicy::Squeeze.resolved(true), ReliefPolicy::Squeeze);
        assert_eq!(ReliefPolicy::Summarize.resolved(false), ReliefPolicy::Summarize);
        // And Ask resolves both ways.
        assert_eq!(ReliefPolicy::Ask.resolved(true), ReliefPolicy::Summarize);
        assert_eq!(ReliefPolicy::Ask.resolved(false), ReliefPolicy::Squeeze);
    }

    #[test]
    fn a_resolved_policy_can_always_reach_the_engine() {
        // After resolution there is no Ask left, so no caller can be stuck
        // holding a policy with no flag.
        for p in ReliefPolicy::all() {
            for chose in [true, false] {
                assert!(p.resolved(chose).engine_flag().is_some(),
                        "{p:?} resolved with {chose} still has no engine flag");
            }
        }
    }

    #[test]
    fn every_variant_is_offered_and_labelled_distinctly() {
        // A dropdown with two identical rows is worse than a checkbox.
        let all = ReliefPolicy::all();
        assert_eq!(all.len(), 3);
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.label(), b.label(), "{a:?} and {b:?} share a label");
                assert_ne!(a.hover(), b.hover(), "{a:?} and {b:?} share a tooltip");
            }
            assert!(!a.label().is_empty() && !a.hover().is_empty());
        }
    }
    #[test]
    fn ask_pauses_an_auto_continuing_run_and_the_others_do_not() {
        // The whole point of the third state. Without the pause, `Ask` shows a
        // panel that the next turn scrolls past — which is what 113 unanswered
        // compaction requests in one run looked like.
        assert!(should_pause_auto_continue(ReliefPolicy::Ask, true));
        // A run configured with an answer does not stop to be asked.
        assert!(!should_pause_auto_continue(ReliefPolicy::Squeeze, true));
        assert!(!should_pause_auto_continue(ReliefPolicy::Summarize, true));
    }

    #[test]
    fn nothing_pauses_when_there_is_nothing_to_decide() {
        // Ask must not stall a run that is under its budget. The pause is
        // triggered by a pending request, not by the setting.
        for p in ReliefPolicy::all() {
            assert!(!should_pause_auto_continue(p, false), "{p:?} paused with no request");
        }
    }

}
