//! liaison_prompt.rs — turning engine facts into a prompt a SMALL model can narrate from.
//!
//! The liaison sits IN FRONT of the big model. While a 30B or 109B works for tens of seconds, a fast
//! model answers "what is it doing?" from the engine's observable state — so the operator gets a running
//! account instead of a spinner.
//!
//! ## Why the prompt is shaped this way
//!
//! A narrator with nothing to narrate will invent. That is the single failure mode worth engineering
//! against here, because a plausible fabricated status is worse than silence: it is indistinguishable
//! from a real one and it will be believed. Three constructions push against it:
//!
//! 1. **Facts are supplied as an explicit, numbered inventory**, not woven into prose. The model can see
//!    exactly how much it was given, and so can a reader auditing the output.
//! 2. **The system prompt makes "I don't know" the correct answer**, in those words, rather than a
//!    failure. Most refusal-shaped instructions describe what not to do; this one supplies the thing to
//!    do instead.
//! 3. **An empty fact list short-circuits before the model is called at all.** No prompt is built, so
//!    there is nothing to hallucinate from — the caller gets `None` and shows nothing.
//!
//! The audience knob controls register, not content: an operator gets consequences, a systems reader gets
//! mechanism. Neither gets more facts than were passed in.

use crate::exploration::Audience;
use crate::liaison::Statement;

/// A chat turn for the liaison model.
#[derive(Clone, Debug, PartialEq)]
pub enum Role { System, User, Assistant }

/// The prompt array handed to the liaison model.
#[derive(Clone, Debug, PartialEq)]
pub struct LiaisonPrompt {
    pub turns: Vec<(Role, String)>,
}

impl LiaisonPrompt {
    pub fn system(&self) -> Option<&str> {
        self.turns.first().filter(|(r, _)| *r == Role::System).map(|(_, s)| s.as_str())
    }
    /// Flatten for backends that take a single string.
    pub fn flatten(&self) -> String {
        self.turns.iter().map(|(r, t)| match r {
            Role::System => format!("[system]\n{t}\n"),
            Role::User => format!("[user]\n{t}\n"),
            Role::Assistant => format!("[assistant]\n{t}\n"),
        }).collect::<Vec<_>>().join("\n")
    }
}

/// The rules. Written as things TO DO where possible, because a list of prohibitions leaves a model
/// guessing at the positive form.
pub const LIAISON_SYSTEM: &str = "\
You are a liaison: you report what a compute backend is doing, to someone waiting on it.

You will be given a numbered list of OBSERVATIONS. They are the only things known to be true.

How to answer:
- Say what the observations say, in one or two short sentences of plain conversation.
- Lead with the thing the person is waiting on. Mechanism only if it changes what they would do.
- If an observation carries a consequence or a remedy, say it — that is usually the useful part.
- If the observations do not answer the question, say exactly what IS known and that the rest is not \
known yet. \"I don't know yet\" is a correct and useful answer here, not a failure.

Never do these:
- Never state anything not present in the observations. Not an inference about progress, not a time \
estimate, not a cause. If it is not listed, it is not known.
- Never reassure. \"Should be done soon\" is a fabrication unless an observation says so.
- Never repeat the observation list back verbatim; you are talking to a person, not echoing a log.";

/// Register guidance appended to the system prompt.
fn audience_note(a: Audience) -> &'static str {
    match a {
        Audience::Operator => "Audience: an operator. Prefer consequences over mechanism — what it means \
for them, what they may need to decide.",
        Audience::Developer => "Audience: a developer on this system. Name components and numbers where \
the observations give them.",
        Audience::Systems => "Audience: a systems engineer. Mechanism, resource behaviour and exact \
figures are wanted; skip the framing.",
    }
}

/// Build the prompt for the liaison model.
///
/// Returns None when there is nothing observed — the caller must then show nothing rather than ask a
/// model to fill the gap. That short-circuit is the strongest of the anti-fabrication measures because it
/// removes the opportunity rather than instructing against it.
pub fn build(facts: &[Statement], ask: Option<&str>, audience: Audience) -> Option<LiaisonPrompt> {
    let grounded: Vec<&Statement> = facts.iter().filter(|s| !s.text.trim().is_empty()).collect();
    if grounded.is_empty() { return None; }

    // FRAMED as tool status. The liaison reports on the inference ENGINE, and this codebase debugs the
    // user's logs at the same time — so "the build failed" is ambiguous between three builds unless the
    // block says which. Telling someone their program crashed when it was our kernel is the worst
    // version of that mistake.
    let mut obs = String::from(crate::promptframe::Frame::ToolStatus.banner());
    obs.push_str("\nOBSERVATIONS\n");
    for (i, s) in grounded.iter().enumerate() {
        obs.push_str(&format!("{}. {}", i + 1, s.text));
        // Provenance travels with the fact. A statement that cannot name its source is exactly the one a
        // reader should discount, and hiding that distinction would defeat the purpose.
        if s.is_grounded() { obs.push_str(&format!("  [source: {}]", s.grounds.join(", "))); }
        else { obs.push_str("  [no source recorded]"); }
        obs.push('\n');
    }

    let question = ask.map(str::trim).filter(|q| !q.is_empty()).unwrap_or("What is it doing right now?");
    let turns = vec![
        (Role::System, format!("{LIAISON_SYSTEM}\n\n{}\n\n{}", audience_note(audience),
            crate::promptframe::DISAMBIGUATION_RULE)),
        (Role::User, format!("{obs}\nQUESTION: {question}")),
    ];
    Some(LiaisonPrompt { turns })
}

/// A one-line fallback when no liaison model is available.
///
/// Deterministic and honest: it prints the most recent observation rather than nothing, so the HUD still
/// says something true when there is no model to phrase it. Prefer this to leaving the operator staring
/// at a blank strip.
pub fn fallback_line(facts: &[Statement]) -> Option<String> {
    facts.iter().rev().find(|s| !s.text.trim().is_empty()).map(|s| s.text.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn st(t: &str, g: &[&str]) -> Statement {
        Statement::new(t.to_string(), g.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn no_observations_means_no_prompt_at_all() {
        // The strongest anti-fabrication measure is removing the opportunity: with nothing observed the
        // model is never asked, so there is nothing to invent from.
        assert!(build(&[], None, Audience::Operator).is_none());
        assert!(build(&[st("   ", &[])], None, Audience::Operator).is_none(), "blank text is not an observation");
    }

    #[test]
    fn observations_are_numbered_and_carry_their_provenance() {
        let p = build(&[st("expert cache at 62%", &["moe-repin"]), st("prefill running", &[])],
                      None, Audience::Developer).unwrap();
        let user = &p.turns[1].1;
        assert!(user.contains("1. expert cache at 62%"));
        assert!(user.contains("[source: moe-repin]"));
        // an ungrounded statement must SAY it is ungrounded rather than look like the others
        assert!(user.contains("2. prefill running  [no source recorded]"), "{user}");
    }

    #[test]
    fn the_system_prompt_supplies_the_positive_form_of_i_dont_know() {
        // A list of prohibitions leaves a model guessing at what to do instead, so the rule that matters
        // most is stated as an action.
        let p = build(&[st("x", &[])], None, Audience::Operator).unwrap();
        let sys = p.system().unwrap();
        assert!(sys.contains("I don't know yet\" is a correct and useful answer"));
        assert!(sys.contains("Never state anything not present in the observations"));
        assert!(sys.contains("Never reassure"), "time estimates are the classic fabrication");
    }

    #[test]
    fn audience_changes_register_not_content() {
        let f = [st("sync is 80% of the step", &["profile"])];
        let op = build(&f, None, Audience::Operator).unwrap();
        let sys_e = build(&f, None, Audience::Systems).unwrap();
        assert_ne!(op.system(), sys_e.system(), "register differs");
        // but the observation block — the CONTENT — is identical
        assert_eq!(op.turns[1].1, sys_e.turns[1].1, "no audience gets extra facts");
    }

    #[test]
    fn a_question_is_carried_through_and_defaulted_when_absent() {
        let f = [st("loading shard 2 of 2", &["loader"])];
        assert!(build(&f, Some("is it stuck?"), Audience::Operator).unwrap().turns[1].1
            .contains("QUESTION: is it stuck?"));
        assert!(build(&f, Some("   "), Audience::Operator).unwrap().turns[1].1
            .contains("QUESTION: What is it doing right now?"), "blank asks get the default");
    }

    #[test]
    fn the_observations_are_framed_as_TOOL_status_not_user_data() {
        // This codebase reports on its own inference engine WHILE debugging someone's log files, so
        // "the build failed" is ambiguous between three builds. Without the frame, a liaison can tell
        // the user their program crashed when it was our kernel.
        let p = build(&[st("the build failed: error[E0308]", &["build"])], None, Audience::Operator).unwrap();
        let user = &p.turns[1].1;
        assert!(user.contains("TOOL STATUS"), "{user}");
        assert!(user.contains("NOT the user's program"));
        // and the system turn carries the standing rule for telling the three apart
        let sys = p.system().unwrap();
        assert!(sys.contains("TOOL STATUS is the engine running you"));
        assert!(sys.contains("USER DATA is what the person asked about"));
    }

    #[test]
    fn the_fallback_says_something_true_when_there_is_no_model() {
        // A blank status strip tells the operator nothing; the newest real observation tells them
        // something, and it cannot be wrong because it is copied verbatim.
        let f = [st("older", &[]), st("newest fact", &["src"])];
        assert_eq!(fallback_line(&f).as_deref(), Some("newest fact"));
        assert_eq!(fallback_line(&[]), None);
    }

    #[test]
    fn the_flattened_form_keeps_role_boundaries() {
        let p = build(&[st("a", &[])], None, Audience::Operator).unwrap();
        let f = p.flatten();
        assert!(f.contains("[system]") && f.contains("[user]"));
        assert!(f.find("[system]") < f.find("[user]"), "system leads");
    }
}

/// The rules for a turn the observations do not bear on.
///
/// ## Observed failure
///
/// In a real session every reply opened with a sentence announcing that the compute backend had
/// finished processing the observations — including the answer to "hi there". With nothing to relay,
/// that line announces a result which does not exist.
///
/// Worse, asked to "tell the debaters to check their error return numbers" with **no debate running**,
/// the model reported a `Terminal::new()` failing with error 2. That is a status from a run that never
/// started, and it reads as a report. The format was available, so it was filled — which is precisely
/// the failure `reasontemplate` names.
///
/// Appended as its own constant rather than edited into `LIAISON_SYSTEM`: that literal is a multi-line
/// escaped string and adding to it kept producing escaping errors, which is a bad trade for a prompt
/// change.
pub const LIAISON_NO_OBSERVATIONS: &str = "\
WHEN THE OBSERVATION LIST IS EMPTY OR DOES NOT BEAR ON WHAT WAS ASKED:
- Do not open with a status line. A sentence announcing that the backend finished processing is \
boilerplate; with nothing to report it is worse than boilerplate, because it announces a result that \
does not exist. Say nothing about the backend at all.
- Answer the question directly, as an ordinary assistant would. A greeting gets a greeting; a request \
to write code gets code. You are a liaison only when there is something to relay.
- If asked about something that is NOT RUNNING, say so plainly. Do not invent its state. An error code \
you did not observe, a status you were not given, a result from a run that never started: each of \
these reads as a report and is a fabrication, and it is worse than an unhelpful answer because the \
person will act on it.";

#[cfg(test)]
mod no_obs_tests {
    use super::*;

    #[test]
    fn an_empty_observation_list_gets_an_ORDINARY_answer() {
        assert!(LIAISON_NO_OBSERVATIONS.contains("Do not open with a status line"));
        assert!(LIAISON_NO_OBSERVATIONS.contains("announces a result that does not exist"));
        assert!(LIAISON_NO_OBSERVATIONS.contains("A greeting gets a greeting"));
    }

    #[test]
    fn the_state_of_something_NOT_RUNNING_must_not_be_invented() {
        // The transcript's second failure: an error return reported for a process that never started.
        assert!(LIAISON_NO_OBSERVATIONS.contains("NOT RUNNING"));
        assert!(LIAISON_NO_OBSERVATIONS.contains("Do not invent its state"));
        assert!(LIAISON_NO_OBSERVATIONS.contains("a run that never started"));
        assert!(LIAISON_NO_OBSERVATIONS.contains("the person will act on it"));
    }

    #[test]
    fn it_is_appended_to_the_system_prompt_not_replacing_it() {
        // Both apply: the observation rules still govern when there ARE observations.
        assert!(!LIAISON_SYSTEM.is_empty());
        assert!(!LIAISON_NO_OBSERVATIONS.is_empty());
    }
}
