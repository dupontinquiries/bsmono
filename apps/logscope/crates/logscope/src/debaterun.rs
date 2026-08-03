//! debaterun.rs — the LiaisonDebate round: N debaters in one batch, then one liaison read.
//!
//! `Mode::LiaisonDebate` existed as a name and a blurb in `coordinator.rs` and nothing executed it.
//! Every other piece was built — `panelmode::route` decides whether a question suits a panel,
//! `panelvoice` handles position and reading discipline, `liaison_prompt` frames the summariser, and
//! `gen_batch` runs several prompts in one width-capped pass — and none of them were joined.
//!
//! ## The shape, and where the speed comes from
//!
//! The debaters are the batchable part: N independent generations over identical weights, differing
//! only in their prompts. They go out as ONE call so the engine reads the weight set once per token
//! instead of N times.
//!
//! The liaison is not batchable and it is worth being precise about why, because I got this backwards
//! once already: it does not run N passes over N outputs. It runs ONE pass over a prompt that contains
//! all N. It is a single stream and always was.
//!
//! So the win here is entirely in the debaters, and the liaison is serial tail time that batching
//! cannot touch.
//!
//! ## Generation is injected
//!
//! `Round::run` takes the batch generator as a closure, so the orchestration is testable without a
//! model — which matters because the ordering rules below are the part that goes wrong, not the
//! inference.

/// One debater's contribution.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Voice {
    pub name: String,
    pub text: String,
    /// Whether this seat was given the others' positions before speaking.
    ///
    /// `panelvoice` calls the withheld case Blind, and it is a control rather than a handicap: a panel
    /// where everyone reads everyone converges, and a converged panel that is wrong looks exactly like
    /// a panel that is right.
    pub informed: bool,
}

/// What a completed round produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Round {
    pub voices: Vec<Voice>,
    /// The liaison's single reading of all of them.
    pub summary: String,
}

/// Errors that are worth distinguishing from an empty answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RoundError {
    /// Fewer than two debaters is not a panel.
    NotAPanel { seats: usize },
    /// The batch returned a different number of replies than prompts.
    ///
    /// This is the one that must never pass silently: a reply count mismatch means the row-to-seat
    /// mapping is wrong, and attributing one debater's text to another reads as a model producing
    /// nonsense rather than as a batching bug.
    ReplyCountMismatch { sent: usize, got: usize },
}

impl RoundError {
    pub fn explain(&self) -> String {
        match self {
            RoundError::NotAPanel { seats } => format!(
                "a panel needs at least two voices and this has {seats} — one debater plus a liaison \
is a summariser reading itself, which costs two generations to produce what one would"),
            RoundError::ReplyCountMismatch { sent, got } => format!(
                "sent {sent} debater prompts and got {got} replies. The row-to-seat mapping cannot be \
recovered from that, and guessing it would attribute one voice's words to another — which reads as a \
model producing nonsense rather than as a batching fault"),
        }
    }
}

/// Run one round: all debaters in a single batch, then the liaison over their outputs.
///
/// `gen_batch` receives every debater prompt at once. Whether the engine batches them, and how wide,
/// is the engine's decision — `batchplan` caps it at what the kernel and the device's watchdog allow.
///
/// `gen_one` runs the liaison. Separate because it is a different shape, not a batch of one.
pub fn run(
    seats: &[(String, String, bool)],          // (name, prompt, informed)
    liaison_frame: impl Fn(&[Voice]) -> String,
    gen_batch: impl Fn(&[String]) -> Vec<String>,
    gen_one: impl Fn(&str) -> String,
) -> Result<Round, RoundError> {
    if seats.len() < 2 { return Err(RoundError::NotAPanel { seats: seats.len() }); }

    let prompts: Vec<String> = seats.iter().map(|(_, p, _)| p.clone()).collect();
    let replies = gen_batch(&prompts);
    if replies.len() != prompts.len() {
        return Err(RoundError::ReplyCountMismatch { sent: prompts.len(), got: replies.len() });
    }

    let voices: Vec<Voice> = seats.iter().zip(replies).map(|((name, _, informed), text)| {
        Voice { name: name.clone(), text, informed: *informed }
    }).collect();

    // The liaison reads all of them in ONE pass. Framing is the caller's, because what the liaison is
    // asked to do — reconcile, pick, or report disagreement — is a product decision and not this
    // function's business.
    let summary = gen_one(&liaison_frame(&voices));
    Ok(Round { voices, summary })
}

impl Round {
    /// Did the panel actually disagree?
    ///
    /// Worth asking before showing a summary: a panel whose voices are near-identical produced one
    /// answer at N times the cost, and saying so is more useful than presenting the agreement as a
    /// finding. `panelmode::route` exists to avoid entering that state; this notices it afterwards.
    pub fn diverged(&self) -> bool {
        let mut it = self.voices.iter().filter(|v| !v.text.trim().is_empty());
        let first = match it.next() { Some(v) => v.text.trim(), None => return false };
        it.any(|v| v.text.trim() != first)
    }

    /// Voices that spoke without seeing the others.
    pub fn blind(&self) -> Vec<&Voice> { self.voices.iter().filter(|v| !v.informed).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seats(n: usize) -> Vec<(String, String, bool)> {
        (0..n).map(|i| (format!("m{i}"), format!("prompt {i}"), i != 0)).collect()
    }
    fn frame(v: &[Voice]) -> String {
        format!("summarise {} voices", v.len())
    }

    #[test]
    fn every_debater_goes_out_in_ONE_batch_call() {
        // The whole speed argument: N independent generations over identical weights become one call,
        // so the engine reads the weight set once per token instead of N times.
        let calls = std::cell::Cell::new(0);
        let r = run(&seats(4), frame,
            |p| { calls.set(calls.get() + 1); p.iter().map(|s| format!("re:{s}")).collect() },
            |_| "summary".into()).expect("runs");
        assert_eq!(calls.get(), 1, "one batch call, not four");
        assert_eq!(r.voices.len(), 4);
        assert_eq!(r.summary, "summary");
    }

    #[test]
    fn the_liaison_is_ONE_pass_over_all_voices_not_one_per_voice() {
        // I got this backwards once: the liaison does not run N passes over N outputs, it runs one
        // pass over a prompt containing all of them. It is a single stream and cannot be batched.
        let seen = std::cell::RefCell::new(String::new());
        run(&seats(5), frame, |p| p.iter().map(|_| "x".into()).collect(),
            |s| { *seen.borrow_mut() = s.to_string(); "s".into() }).unwrap();
        assert_eq!(*seen.borrow(), "summarise 5 voices", "one prompt, five voices inside it");
    }

    #[test]
    fn a_REPLY_COUNT_MISMATCH_is_refused_rather_than_zipped() {
        // The failure that must never pass silently. Zip would have truncated to the shorter list and
        // attributed one voice's words to another — which reads as a model producing nonsense.
        let e = run(&seats(4), frame, |_| vec!["a".into(), "b".into()], |_| "s".into()).unwrap_err();
        assert_eq!(e, RoundError::ReplyCountMismatch { sent: 4, got: 2 });
        assert!(e.explain().contains("attribute one voice's words to another"));
    }

    #[test]
    fn one_debater_is_not_a_panel() {
        let e = run(&seats(1), frame, |p| p.iter().map(|_| "x".into()).collect(),
                    |_| "s".into()).unwrap_err();
        assert!(matches!(e, RoundError::NotAPanel { seats: 1 }));
        assert!(e.explain().contains("costs two generations to produce what one would"));
    }

    #[test]
    fn the_voice_order_matches_the_seat_order() {
        // The mapping that the count check protects. Asserted directly rather than trusted.
        let r = run(&seats(3), frame,
            |p| p.iter().map(|s| format!("re:{s}")).collect(), |_| "s".into()).unwrap();
        for (i, v) in r.voices.iter().enumerate() {
            assert_eq!(v.name, format!("m{i}"));
            assert_eq!(v.text, format!("re:prompt {i}"));
        }
    }

    #[test]
    fn a_panel_that_AGREES_is_reported_as_not_diverged() {
        // Four voices saying the same thing produced one answer at four times the cost, and saying so
        // is more useful than presenting the agreement as a finding.
        let same = run(&seats(4), frame, |p| p.iter().map(|_| "identical".into()).collect(),
                       |_| "s".into()).unwrap();
        assert!(!same.diverged());
        let differ = run(&seats(4), frame,
            |p| p.iter().enumerate().map(|(i, _)| format!("v{i}")).collect(),
            |_| "s".into()).unwrap();
        assert!(differ.diverged());
    }

    #[test]
    fn the_blind_seat_is_identifiable_afterwards() {
        // panelvoice's control: a panel where everyone reads everyone converges, and a converged panel
        // that is wrong looks exactly like one that is right.
        let r = run(&seats(4), frame, |p| p.iter().map(|_| "x".into()).collect(),
                    |_| "s".into()).unwrap();
        assert_eq!(r.blind().len(), 1);
        assert_eq!(r.blind()[0].name, "m0");
    }
}

/// Why a voice was restarted rather than accepted.
///
/// **The debate group's real advantage is not N opinions — it is N cheap RESTARTS.** A small model can
/// be thrown away and re-run with a different framing for a fraction of what it costs to nudge a large
/// one, and the evidence for that is direct: a 3B model continuing Scout's output produced better code
/// than Scout continuing itself, because Scout was three steps into a decaying trajectory and the 3B
/// entered clean.
///
/// That is a context-rot result rather than a model-size one, and it means a seat that has gone wrong
/// should be RESTARTED, not coaxed. These are the conditions worth restarting on — each detectable
/// without reading the content.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Reframe {
    /// Produced nothing usable.
    Empty,
    /// Repeated itself — the clearest sign a small model has lost the thread.
    Looping { repeated: String },
    /// Answered a different question than the one asked, detectable when it echoes the frame back.
    EchoedThePrompt,
}

impl Reframe {
    /// What to change about the prompt on the retry.
    ///
    /// Restarting with the SAME frame reproduces the same failure — the model is deterministic given
    /// its input, so a retry that changes nothing is a wasted generation. Each condition implies a
    /// different edit.
    pub fn adjustment(&self) -> &'static str {
        match self {
            Reframe::Empty =>
                "ask for the answer directly, without the framing that produced nothing",
            Reframe::Looping { .. } =>
                "shorten the context — a loop usually means the window is full of its own output",
            Reframe::EchoedThePrompt =>
                "state the question as an instruction rather than a description of the task",
        }
    }
}

/// Does this voice need restarting?
///
/// Deliberately cheap and content-blind: a judgement about quality needs another model, and a debate
/// that needs a referee to decide who to re-run has spent the saving it was supposed to make.
pub fn needs_reframe(text: &str, prompt: &str) -> Option<Reframe> {
    let t = text.trim();
    if t.is_empty() { return Some(Reframe::Empty); }

    // A line repeated three times or more. Two is a coincidence — a list can legitimately repeat a
    // stem — and three is a loop.
    let lines: Vec<&str> = t.lines().map(|l| l.trim()).filter(|l| l.len() > 8).collect();
    for l in &lines {
        if lines.iter().filter(|x| x == &l).count() >= 3 {
            return Some(Reframe::Looping { repeated: l.to_string() });
        }
    }

    // Echoing back a distinctive chunk of the prompt rather than answering it.
    let p = prompt.trim();
    if p.len() >= 40 && t.contains(&p[..40]) { return Some(Reframe::EchoedThePrompt); }

    None
}

#[cfg(test)]
mod reframe_tests {
    use super::*;

    #[test]
    fn a_looping_voice_is_caught_and_names_what_repeated() {
        let t = "considering the options\nconsidering the options\nconsidering the options";
        match needs_reframe(t, "q") {
            Some(Reframe::Looping { repeated }) => assert!(repeated.contains("considering")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn TWO_repeats_is_not_a_loop() {
        // A list can legitimately repeat a stem, and restarting a working voice costs a whole
        // generation to fix nothing.
        let t = "step: parse the file\nstep: parse the file\nthen we are done";
        assert!(needs_reframe(t, "q").is_none());
    }

    #[test]
    fn an_ECHOED_prompt_is_distinguished_from_a_real_answer() {
        let p = "Compare the arena allocator against the streaming path and say which fits better";
        assert_eq!(needs_reframe(&format!("{p} — here is my view"), p),
                   Some(Reframe::EchoedThePrompt));
    }

    #[test]
    fn a_short_prompt_cannot_trigger_the_echo_rule() {
        // Otherwise "yes" answering "yes?" is an echo, and the rule fires on the shortest exchanges
        // where it is least likely to be right.
        assert!(needs_reframe("ok that works", "ok").is_none());
    }

    #[test]
    fn every_reason_implies_a_DIFFERENT_adjustment() {
        // Restarting with the same frame reproduces the same failure — the model is deterministic
        // given its input, so a retry that changes nothing is a wasted generation.
        let a = Reframe::Empty.adjustment();
        let b = Reframe::Looping { repeated: "x".into() }.adjustment();
        let c = Reframe::EchoedThePrompt.adjustment();
        assert_ne!(a, b); assert_ne!(b, c); assert_ne!(a, c);
        assert!(b.contains("shorten"), "a loop means the window is full of its own output");
    }

    #[test]
    fn a_healthy_voice_is_left_alone() {
        assert!(needs_reframe("The arena fits better because slabs are contiguous.", "which fits?")
            .is_none());
    }

    #[test]
    fn the_check_is_CONTENT_BLIND() {
        // A judgement about quality needs another model, and a debate that needs a referee to decide
        // who to re-run has spent the saving it was supposed to make. Nonsense that does not loop or
        // echo passes — that is the panel's job, not this function's.
        assert!(needs_reframe("purple monday runs the compiler sideways", "q").is_none());
    }
}
