//! continuation — should the run carry on when the model stops talking?
//!
//! ## The gap
//!
//! An operator types "keep working for the next hour" and the model answers
//! once. The hour is real — [`crate::sessiontime`] can now read it — but
//! nothing acts on it, so the operator types "keep working" again, and again,
//! and the transcript that prompted this has four of those in a row with the
//! last reply empty.
//!
//! ## Dumb by default, and that is the design
//!
//! A decision that costs tokens on every turn gets switched off. So the normal
//! path is [`decide`]: a handful of rules over facts already in hand — time
//! left, what the reply looked like, how many continuations have already
//! fired — costing nothing and adding nothing to the context.
//!
//! Only when those rules genuinely cannot tell does it return
//! [`Verdict::Escalate`], and the caller may put the question to a small model
//! or a fresh instance. That call answers from a FIXED VOCABULARY
//! ([`Command`]), so its reply is parseable rather than prose, and a model that
//! rambles is treated as no answer rather than as an instruction.
//!
//! The same shape serves the hive-mind matrix for ordinary turns: "what should
//! happen next" is the same question with the same small answer set, and a
//! coordinator that can only fan out is one that must guess when not to.
//!
//! ## What it must never do
//!
//! Continue past a question. If the model asked the operator something, an
//! auto-continuation answers on their behalf and buries the question — the
//! worst outcome available here, and worse than stopping early. Same for an
//! empty or degenerate reply: something has gone wrong and another identical
//! prompt will not fix it.
//!
//! Pure: facts in, verdict out. No clock, no model, no I/O.

/// What the controller can decide to do. Deliberately small — a fixed set is
/// what lets an escalated answer be parsed instead of interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Carry on the way the last turn ran.
    KeepGoingLinear,
    /// Carry on, but fan the next step across parallel workers.
    KeepGoingFan,
    /// Carry on with a verification pass rather than more work.
    KeepGoingCheck,
    /// Hand control back to the operator.
    StopAndWait,
}

impl Command {
    /// The vocabulary an escalated model is given, as it should be printed.
    pub const ALL: &'static [Command] = &[
        Command::KeepGoingLinear, Command::KeepGoingFan,
        Command::KeepGoingCheck, Command::StopAndWait,
    ];

    pub fn token(self) -> &'static str {
        match self {
            Command::KeepGoingLinear => "CONTINUE_LINEAR",
            Command::KeepGoingFan => "CONTINUE_FAN",
            Command::KeepGoingCheck => "CONTINUE_CHECK",
            Command::StopAndWait => "STOP",
        }
    }

    pub fn is_continue(self) -> bool { !matches!(self, Command::StopAndWait) }
}

/// The verdict, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Decided without spending anything. `&'static str` is the reason, for the
    /// log — an auto-continuation the operator cannot account for is one they
    /// will turn off.
    Decided(Command, &'static str),
    /// The cheap rules cannot tell. The caller MAY ask a small model; doing
    /// nothing and stopping is also a correct response to this.
    Escalate(&'static str),
}

/// What the controller gets to look at. All of it is already known — nothing
/// here requires a model call to compute.
#[derive(Debug, Clone, Copy)]
pub struct Facts<'a> {
    /// The assistant's last reply.
    pub reply: &'a str,
    /// Seconds left on the operator's work limit; `None` for open-ended.
    pub remaining_s: Option<u64>,
    /// Auto-continuations already fired since the operator last spoke.
    pub auto_turns: usize,
    /// Ceiling on those. Beyond it, stop regardless.
    pub max_auto_turns: usize,
    /// Will an unanswered question stay VISIBLE if the run carries on?
    ///
    /// True when the caller is going to badge it — `readmark::open_questions`
    /// over the graph's nodes. The original rule stopped dead on any question
    /// because continuing "buries it above the next wall of output", and that
    /// reasoning assumes a linear transcript. With a tree graph the question
    /// does not have to be buried: it can be marked and stay findable.
    ///
    /// So the stop is conditional rather than absolute. False — no marking —
    /// keeps the old behaviour, which is correct for any caller that cannot
    /// show it.
    pub questions_are_flagged: bool,
}

/// The cheap decision.
pub fn decide(f: Facts<'_>) -> Verdict {
    let reply = f.reply.trim();

    // ── things that stop, in order of how bad it is to get them wrong ──

    // An empty reply is a fault, not a pause. The transcript that prompted
    // this ends with one, after three degrading answers; another identical
    // prompt produces another empty reply and burns the remaining time.
    if reply.is_empty() {
        return Verdict::Decided(Command::StopAndWait, "the model produced nothing");
    }
    // A budget exists so that something eventually stops.
    if f.auto_turns >= f.max_auto_turns {
        return Verdict::Decided(Command::StopAndWait, "auto-continue budget spent");
    }
    match f.remaining_s {
        // No limit was asked for, so nobody licensed unattended work.
        None => return Verdict::Decided(Command::StopAndWait, "no work limit was given"),
        Some(0) => return Verdict::Decided(Command::StopAndWait, "the work limit has run out"),
        // Too little left to finish a step. Starting one produces a half-done
        // thing and a stop, which is worse than stopping now.
        Some(s) if s < 60 => {
            return Verdict::Decided(Command::StopAndWait, "under a minute left");
        }
        Some(_) => {}
    }

    // AFTER the time and budget stops, deliberately. A flagged question turns
    // into a CHECK, which is a continuation — so placing it above them let a
    // spent work limit continue anyway. Caught by
    // `flagging_does_not_defeat_the_other_stop_conditions`; a badge makes a
    // question survivable, not the clock.
    //
    // A question belongs to the operator — but "buried" is a property of a
    // linear transcript, not of a graph that can badge the node.
    //
    // Unflagged, this is still a hard stop: an unanswered question scrolling
    // away with nothing to mark it is the case the rule was written for.
    //
    // Flagged, it becomes a CHECK rather than a stop. The run continues, the
    // node carries its badge, and the next step verifies rather than building
    // further on an answer the operator never gave — carrying on with more
    // construction would compound whatever the question was uncertain about.
    if ends_with_question(reply) {
        return if f.questions_are_flagged {
            Verdict::Decided(Command::KeepGoingCheck,
                "a question was asked and is flagged in the graph — verifying rather than building on it")
        } else {
            Verdict::Decided(Command::StopAndWait, "the model asked a question")
        };
    }

    // ── things that continue ──

    // Mid-thought: the reply stops inside a code fence or on a colon. Whatever
    // it was doing is not finished and the next step is the same step.
    if unclosed_fence(reply) {
        return Verdict::Decided(Command::KeepGoingLinear, "reply ends inside a code block");
    }
    if reply.ends_with(':') {
        return Verdict::Decided(Command::KeepGoingLinear, "reply ends on a colon");
    }
    // An explicit promise to continue is the clearest signal there is.
    if promises_more(reply) {
        return Verdict::Decided(Command::KeepGoingLinear, "the reply says it will continue");
    }

    // ── genuinely ambiguous ──
    //
    // A reply that looks finished, with time left on the clock. Whether that
    // means "the task is done" or "this step is done and there are more" is
    // exactly the judgement the cheap rules cannot make, and the only case
    // worth spending a model call on.
    Verdict::Escalate("the reply looks complete but time remains")
}

/// Does the reply end by asking the operator something?
///
/// The LAST non-empty line only. A question mark anywhere in a long answer is
/// usually rhetorical or quoted; one at the end is addressed to the reader.
fn ends_with_question(reply: &str) -> bool {
    reply.lines().rev().find(|l| !l.trim().is_empty())
        .map(|l| l.trim_end().ends_with('?'))
        .unwrap_or(false)
}

/// An odd number of fence openers means the reply stopped inside one.
fn unclosed_fence(reply: &str) -> bool {
    reply.lines().filter(|l| l.trim_start().starts_with("```")).count() % 2 == 1
}

fn promises_more(reply: &str) -> bool {
    let tail: String = reply.chars().rev().take(200).collect::<String>()
        .chars().rev().collect::<String>().to_ascii_lowercase();
    ["next i", "i'll continue", "continuing", "next step", "let me now", "i will now"]
        .iter().any(|p| tail.contains(p))
}

/// The prompt for the escalation call.
///
/// Deliberately tiny and answer-shaped: a small model given a paragraph of
/// context will write a paragraph back. It is handed the vocabulary and the
/// tail of the reply, and asked for one token.
pub fn escalation_prompt(reply_tail: &str) -> String {
    let opts = Command::ALL.iter().map(|c| c.token()).collect::<Vec<_>>().join(", ");
    format!(
        "An assistant was told to keep working and has just stopped. Decide what happens next.\n\
         Answer with EXACTLY ONE of: {opts}\n\
         No explanation.\n\n\
         Its last words were:\n{reply_tail}"
    )
}

/// Read an escalated answer.
///
/// Anything that is not exactly one known token is `None` — treated as no
/// answer, and the caller should stop rather than guess. A model that replies
/// "I think you should probably CONTINUE_LINEAR unless..." has not followed the
/// instruction, and honouring the token buried in it rewards the behaviour that
/// makes this layer unpredictable.
pub fn parse_escalation(answer: &str) -> Option<Command> {
    let a = answer.trim();
    Command::ALL.iter().copied().find(|c| a.eq_ignore_ascii_case(c.token()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(reply: &str, remaining: Option<u64>, auto: usize) -> Facts<'_> {
        Facts { reply, remaining_s: remaining, auto_turns: auto, max_auto_turns: 8,
                questions_are_flagged: false }
    }

    fn cmd(v: Verdict) -> Option<Command> {
        match v { Verdict::Decided(c, _) => Some(c), Verdict::Escalate(_) => None }
    }

    /// The case from the reported transcript: the model produced nothing.
    #[test]
    fn an_empty_reply_stops_rather_than_retrying() {
        let v = decide(facts("", Some(3000), 0));
        assert_eq!(cmd(v.clone()), Some(Command::StopAndWait));
        assert!(matches!(v, Verdict::Decided(_, r) if r.contains("nothing")));
    }

    /// Never answer the operator's question for them.
    #[test]
    fn a_question_stops_even_with_hours_left() {
        let v = decide(facts("Which backend should I target?", Some(3599), 0));
        assert_eq!(cmd(v), Some(Command::StopAndWait));
        // But a question mark mid-reply is not a question TO the operator.
        let v2 = decide(facts("Is it slow? No — it is bound by sync.\nNext I'll measure.",
                              Some(3599), 0));
        assert_eq!(cmd(v2), Some(Command::KeepGoingLinear));
    }

    #[test]
    fn an_unclosed_code_fence_continues() {
        let v = decide(facts("Here is the fix:\n```rust\nfn main() {", Some(3000), 0));
        assert_eq!(cmd(v), Some(Command::KeepGoingLinear));
    }

    #[test]
    fn an_explicit_promise_continues() {
        let v = decide(facts("That covers the parser. Next I'll wire the loader.", Some(3000), 0));
        assert_eq!(cmd(v), Some(Command::KeepGoingLinear));
    }

    /// Nobody licensed unattended work.
    #[test]
    fn no_work_limit_means_stop() {
        assert_eq!(cmd(decide(facts("Done.", None, 0))), Some(Command::StopAndWait));
    }

    #[test]
    fn a_spent_or_nearly_spent_limit_stops() {
        assert_eq!(cmd(decide(facts("Done.", Some(0), 0))), Some(Command::StopAndWait));
        assert_eq!(cmd(decide(facts("Done.", Some(30), 0))), Some(Command::StopAndWait),
                   "under a minute buys a half-done step and a stop");
    }

    #[test]
    fn the_auto_budget_is_a_hard_stop() {
        assert_eq!(cmd(decide(facts("Next I'll keep going.", Some(3000), 8))),
                   Some(Command::StopAndWait));
    }

    /// The ONLY case worth spending a model call on.
    #[test]
    fn a_finished_looking_reply_with_time_left_escalates() {
        let v = decide(facts("The loader now refuses fused experts.", Some(3000), 0));
        assert!(matches!(v, Verdict::Escalate(_)), "{v:?}");
    }

    /// Stopping conditions must outrank continuing ones: a reply that both
    /// promises more AND asks a question must stop.
    #[test]
    fn a_stop_signal_beats_a_continue_signal() {
        let v = decide(facts("Next I'll wire it. Which backend do you want?", Some(3000), 0));
        assert_eq!(cmd(v), Some(Command::StopAndWait));
    }

    #[test]
    fn an_escalated_answer_must_be_exactly_a_token() {
        assert_eq!(parse_escalation("CONTINUE_FAN"), Some(Command::KeepGoingFan));
        assert_eq!(parse_escalation("  stop \n"), Some(Command::StopAndWait));
        // Prose containing a token is NOT an answer.
        assert_eq!(parse_escalation("I think CONTINUE_LINEAR unless you disagree"), None,
                   "honouring a buried token rewards ignoring the instruction");
        assert_eq!(parse_escalation(""), None);
    }

    /// Every decision carries a reason, because an auto-continuation the
    /// operator cannot account for is one they will turn off.
    #[test]
    fn every_decision_explains_itself() {
        for f in [facts("", Some(9), 0), facts("Done?", Some(3000), 0),
                  facts("x", None, 0), facts("```\nfn f() {", Some(3000), 0)] {
            if let Verdict::Decided(_, reason) = decide(f) {
                assert!(reason.len() > 8, "thin reason: {reason:?}");
            }
        }
    }

    /// With the graph badging it, a question no longer has to stop the run.
    ///
    /// The original rule assumed a linear transcript where an unanswered
    /// question scrolls away and is lost. A tree graph can mark the node, so
    /// the reason for stopping does not apply.
    #[test]
    fn a_flagged_question_continues_as_a_check() {
        let f = Facts { questions_are_flagged: true, ..facts("Which backend?", Some(3000), 0) };
        assert_eq!(cmd(decide(f)), Some(Command::KeepGoingCheck),
                   "flagged, the question stays findable and the run can verify instead of stopping");
    }

    /// And VERIFY rather than build — the distinction that makes continuing safe.
    #[test]
    fn a_flagged_question_does_not_continue_with_more_construction() {
        let f = Facts { questions_are_flagged: true, ..facts("Shall I use wgpu?", Some(3000), 0) };
        let got = cmd(decide(f));
        assert_ne!(got, Some(Command::KeepGoingLinear),
                   "building further compounds whatever the question was uncertain about");
        assert_ne!(got, Some(Command::KeepGoingFan));
    }

    /// Without marking, the old hard stop stands.
    #[test]
    fn an_unflagged_question_still_stops() {
        let f = Facts { questions_are_flagged: false, ..facts("Which backend?", Some(3000), 0) };
        assert_eq!(cmd(decide(f)), Some(Command::StopAndWait));
    }

    /// Flagging does not override the other stops. A spent limit or an empty
    /// reply is not made safe by a badge.
    #[test]
    fn flagging_does_not_defeat_the_other_stop_conditions() {
        for (reply, rem, auto) in [("", Some(3000), 0), ("Which?", Some(0), 0),
                                   ("Which?", Some(3000), 8)] {
            let f = Facts { questions_are_flagged: true, ..facts(reply, rem, auto) };
            assert_eq!(cmd(decide(f)), Some(Command::StopAndWait),
                       "reply={reply:?} remaining={rem:?} auto={auto}");
        }
    }

}
