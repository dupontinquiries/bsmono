//! promptframe.rs — say WHOSE text this is, because the content cannot.
//!
//! A model working inside this tool sees three kinds of text that look identical and mean completely
//! different things:
//!
//! 1. **Tool status** — logscope's own engine: "the build failed", "48 submits per token", "expert cache
//!    62%". This is the machinery the model is running INSIDE.
//! 2. **User data** — the logs, code and files the person is asking about. This is the SUBJECT.
//! 3. **Candidate output** — code a model just produced, awaiting review. This is a PROPOSAL.
//!
//! `error[E0308]: mismatched types` is byte-identical in all three, and the right response to each is
//! opposite: fix it, explain it, or reject it. A model that conflates them will confidently answer the
//! wrong question — and, worse, will do so fluently, because nothing in the text signals the mistake.
//!
//! The confusion is not hypothetical for this codebase in particular: it debugs log files for a living,
//! writes code, and reports on its own inference engine. All three streams are live at once, and they
//! all contain stack traces.
//!
//! ## The rule
//!
//! Every block is labelled with WHOSE it is and WHAT the model should do with it, in the imperative.
//! A label that only names the source ("System:") still leaves the model to infer the appropriate
//! response; naming the response removes the inference.

/// What a block of text IS, from the model's point of view.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Frame {
    /// Logscope's own engine and coordinator. The model is running inside this.
    ToolStatus,
    /// What the person asked about — their logs, their files, their code.
    UserData,
    /// A candidate answer awaiting judgement.
    CandidateOutput,
    /// The person's own words.
    UserMessage,
}

impl Frame {
    /// The banner. Names the owner AND the expected response, because a label that only names the
    /// source leaves the model to infer what to do with it — and inference is what goes wrong.
    pub fn banner(self) -> &'static str {
        match self {
            Frame::ToolStatus => "=== TOOL STATUS — the inference engine you are running inside. \
This is NOT the user's program and NOT code under review. Report it; do not try to fix it. ===",
            Frame::UserData => "=== USER DATA — files and logs the person asked about. This is the \
SUBJECT of the question. Errors here are what they want explained, not defects in you or in the tool. ===",
            Frame::CandidateOutput => "=== CANDIDATE OUTPUT — code a model just produced, not yet \
accepted. Judge it. Errors here mean the candidate is wrong and should be revised or rejected. ===",
            Frame::UserMessage => "=== USER MESSAGE ===",
        }
    }

    /// Short tag for a log line or a UI badge.
    pub fn tag(self) -> &'static str {
        match self {
            Frame::ToolStatus => "tool", Frame::UserData => "subject",
            Frame::CandidateOutput => "candidate", Frame::UserMessage => "user",
        }
    }

    /// Whose problem is an error appearing in this block?
    ///
    /// The distinction the framing exists to make. Three sources of "error" text, three different owners.
    pub fn error_owner(self) -> &'static str {
        match self {
            Frame::ToolStatus => "the tool — report it to the operator, do not attempt a fix",
            Frame::UserData => "the subject under study — explaining it IS the task",
            Frame::CandidateOutput => "the candidate — revise or reject it",
            Frame::UserMessage => "not an error; it is what was asked",
        }
    }
}

/// A framed section of prompt.
#[derive(Clone, PartialEq, Debug)]
pub struct Block { pub frame: Frame, pub body: String }

/// Assemble framed blocks into one prompt body.
///
/// Empty blocks are DROPPED rather than emitted with just a banner: a banner over nothing invites the
/// model to explain the absence, and "the tool reported nothing" is not a thing worth a sentence.
pub fn assemble(blocks: &[Block]) -> String {
    let mut out = String::new();
    for b in blocks {
        if b.body.trim().is_empty() { continue; }
        out.push_str(b.frame.banner());
        out.push('\n');
        out.push_str(b.body.trim());
        out.push_str("\n\n");
    }
    out.trim_end().to_string()
}

/// The standing instruction for any model that may see more than one frame at once.
pub const DISAMBIGUATION_RULE: &str = "\
Text in this conversation comes from three different places and they look alike. Before responding to any \
error, stack trace or failure message, identify which block it came from:
- TOOL STATUS is the engine running you. Report it. Do not try to fix it, and never present it as though \
the user's program failed.
- USER DATA is what the person asked about. Explaining its errors is the job.
- CANDIDATE OUTPUT is a proposed answer. Its errors mean the proposal is wrong.
If a block is unlabelled, say which you think it is before acting on it.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_gets_opposite_instructions_under_different_frames() {
        // The reason this module exists. `error[E0308]` is byte-identical whether it came from the
        // engine, the user's log, or code we just wrote — and the correct response to each is opposite.
        let err = "error[E0308]: mismatched types";
        let a = assemble(&[Block { frame: Frame::ToolStatus, body: err.into() }]);
        let b = assemble(&[Block { frame: Frame::UserData, body: err.into() }]);
        let c = assemble(&[Block { frame: Frame::CandidateOutput, body: err.into() }]);
        assert!(a.contains("do not try to fix it"));
        assert!(b.contains("what they want explained"));
        assert!(c.contains("revised or rejected"));
        // the bodies are the same; only the framing differs
        assert!(a.contains(err) && b.contains(err) && c.contains(err));
        assert_ne!(a, b); assert_ne!(b, c);
    }

    #[test]
    fn every_frame_names_who_owns_an_error_in_it() {
        // A label that only names the SOURCE ("System:") leaves the model to infer the right response,
        // and that inference is exactly what goes wrong.
        for f in [Frame::ToolStatus, Frame::UserData, Frame::CandidateOutput] {
            let o = f.error_owner();
            assert!(o.len() > 20, "{f:?} needs a real answer, got {o:?}");
        }
        assert!(Frame::ToolStatus.error_owner().contains("do not attempt a fix"));
        assert!(Frame::UserData.error_owner().contains("IS the task"));
        assert!(Frame::CandidateOutput.error_owner().contains("revise or reject"));
    }

    #[test]
    fn the_tool_frame_forbids_presenting_engine_faults_as_the_users() {
        // The specific confusion worth engineering against: logscope reports on its own inference engine
        // WHILE debugging someone's logs. "The build failed" is ambiguous between three builds, and
        // telling a user their program crashed when it was our kernel is the worst version of it.
        assert!(Frame::ToolStatus.banner().contains("NOT the user's program"));
        assert!(DISAMBIGUATION_RULE.contains("never present it as though the user's program failed"));
    }

    #[test]
    fn an_empty_block_is_dropped_not_announced() {
        // A banner over nothing invites the model to explain the absence, and "the tool reported
        // nothing" is not worth a sentence.
        let s = assemble(&[
            Block { frame: Frame::ToolStatus, body: "   ".into() },
            Block { frame: Frame::UserData, body: "real content".into() },
        ]);
        assert!(!s.contains("TOOL STATUS"));
        assert!(s.contains("USER DATA") && s.contains("real content"));
    }

    #[test]
    fn the_rule_tells_the_model_what_to_do_when_a_block_is_UNLABELLED() {
        // Prompts get assembled by several code paths and one will forget. The fallback has to be "say
        // which you think it is", not silent guessing.
        assert!(DISAMBIGUATION_RULE.contains("If a block is unlabelled, say which you think it is"));
    }

    #[test]
    fn tags_are_distinct_and_short_enough_for_a_badge() {
        let t: Vec<&str> = [Frame::ToolStatus, Frame::UserData, Frame::CandidateOutput, Frame::UserMessage]
            .iter().map(|f| f.tag()).collect();
        let mut u = t.clone(); u.sort(); u.dedup();
        assert_eq!(u.len(), t.len(), "tags must be distinct: {t:?}");
        assert!(t.iter().all(|s| s.len() <= 10));
    }
}
