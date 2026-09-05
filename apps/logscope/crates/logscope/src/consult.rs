//! consult — when to ask a human expert, and what to keep of the answer.
//!
//! logscope already decides when to READ something: `context_probe::plan`
//! weighs each probe's estimated cost against a budget and returns the ones
//! worth spending on. This is the same decision for a source the machine
//! cannot read — a person who knows the field.
//!
//! # Why this is not `panelvoice::Ask`
//!
//! `Ask` exists and is right for what it does: a debater's question to the
//! operator, `ASK_TTL_ROUNDS = 2`, because "a late question is worse than
//! none". A consultation is the opposite shape on every axis:
//!
//! | | `Ask` | `Consultation` |
//! |---|---|---|
//! | lifetime | 2 rounds | [`STALE_AFTER_MESSAGES`] |
//! | blocks | the panel waits | never — the run proceeds |
//! | audience | whoever is at the keyboard | someone who knows the domain |
//! | outcome | steers this round | a file in the repo, kept |
//!
//! Merging them would force one TTL on both and lose the reason each was
//! chosen. They are separate on purpose.
//!
//! # Proceed, then branch
//!
//! Raising a consultation never stops the run. The run says what it would
//! ask and carries on with its own best answer, because a run that halts
//! for every uncertainty is a run nobody leaves unattended. What it records
//! is the BRANCH POINT — the message index where the question arose — so an
//! answer arriving later can be applied from there rather than at the end,
//! where the reasoning it should have changed has already happened.
//!
//! # The hundred-message stop
//!
//! Proceeding is not the same as proceeding forever. An open question that
//! has gone [`STALE_AFTER_MESSAGES`] without an answer is not a run making
//! progress despite a gap; it is a run that has built a hundred messages of
//! work on an assumption nobody checked. That is the derailment signal, and
//! it is the one case where this module says stop.
//!
//! # The answer is a source, not a note
//!
//! An answered consultation renders to a markdown file under
//! [`DOC_DIR`], committed with the work. That puts it in the same pool the
//! probe layer already reads: an expert's account becomes a file a later
//! run can survey and cite, with attribution and a date, rather than a fact
//! floating in a transcript nobody kept. First-hand, attributable, and
//! diffable — the three things a transcript is not.

use std::fmt::Write as _;

/// Where consultation records live, relative to the repository root.
pub const DOC_DIR: &str = "docs/consultations";

/// Messages an unanswered consultation may ride along for before the run
/// should stop.
///
/// A hundred rather than a round count because consultations outlive
/// rounds, and because the number is meant to be a derailment alarm rather
/// than a politeness timer — it should be far enough away that a normal run
/// answering its own question never reaches it.
pub const STALE_AFTER_MESSAGES: u64 = 100;

/// Confidence at or below which a question is worth a human, given leverage.
pub const CONSULT_BELOW_CONFIDENCE: f32 = 0.6;
/// Leverage at or above which a question is worth interrupting for.
pub const WORTH_ASKING: f32 = 0.5;

/// What a consultation is about. Not an exhaustive taxonomy — it is the
/// label the doc is filed under and the thing an expert is matched on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Domain(pub String);

impl Domain {
    pub fn new(s: &str) -> Domain {
        Domain(s.trim().to_lowercase())
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A human's answer, kept with enough attribution to be worth something
/// later.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// Who said it. Required — an unattributed expert opinion is a rumour.
    pub expert: String,
    /// Why they are worth asking: "PhD, numerical analysis", "wrote the
    /// firmware". Free text, because credentials do not enumerate.
    pub standing: String,
    /// What they said, in their words. Not summarised: this is the
    /// first-hand account, and a paraphrase of it is a second-hand one.
    pub text: String,
    /// Message index the answer arrived at.
    pub at_message: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Consultation {
    /// Stable id, used for the filename and for matching an answer to its
    /// question across a restart.
    pub id: String,
    pub domain: Domain,
    /// The question, phrased for someone who has not read the transcript.
    pub question: String,
    /// What the run decided in the meantime, so a reader can see what the
    /// answer would be overturning.
    pub provisional: String,
    /// How much the answer changes what happens, 0..1.
    pub leverage: f32,
    /// The branch point: the message index where this arose.
    pub at_message: u64,
    pub answer: Option<Answer>,
}

/// What the run should do about its open consultations.
///
/// `RunPosture`, not `Posture`: `review_gate::Posture` and
/// `taskclass::posture` already exist in the egui crate and mean different
/// things. The duplicate-name tripwire caught the collision; renaming beat
/// spending its ceiling on a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunPosture {
    /// Carry on. The default, and the common case.
    Proceed,
    /// An open question has gone stale. Stop and get a human.
    Stop,
}

/// Should this uncertainty become a consultation?
///
/// Both conditions, not either: a low-confidence answer nobody depends on
/// is not worth a person's time, and a high-leverage answer the run is
/// already sure of is not a question. `in_competence` is the run's own
/// judgement that the domain is one it handles — a maths question inside a
/// maths tool is not a consultation, the same question inside a log viewer
/// is.
pub fn should_consult(confidence01: f32, leverage01: f32, in_competence: bool) -> bool {
    if in_competence {
        return false;
    }
    confidence01 <= CONSULT_BELOW_CONFIDENCE && leverage01 >= WORTH_ASKING
}

/// Run posture given every consultation and where the run is now.
///
/// Answered ones never stop the run however old they are — the gap they
/// represented is closed, and stopping for a closed question would be the
/// alarm firing at nothing.
pub fn run_posture(all: &[Consultation], now_message: u64) -> RunPosture {
    let stale = all.iter().any(|c| {
        c.answer.is_none() && now_message.saturating_sub(c.at_message) >= STALE_AFTER_MESSAGES
    });
    if stale {
        RunPosture::Stop
    } else {
        RunPosture::Proceed
    }
}

/// The open consultations, oldest first — what a prompt should show.
pub fn open(all: &[Consultation]) -> Vec<&Consultation> {
    let mut v: Vec<&Consultation> = all.iter().filter(|c| c.answer.is_none()).collect();
    v.sort_by_key(|c| c.at_message);
    v
}

/// Messages remaining before `c` triggers a stop. `None` once answered.
pub fn grace_left(c: &Consultation, now_message: u64) -> Option<u64> {
    if c.answer.is_some() {
        return None;
    }
    Some(STALE_AFTER_MESSAGES.saturating_sub(now_message.saturating_sub(c.at_message)))
}

/// A filesystem-safe slug. Lower case, alphanumerics and hyphens only, no
/// runs of hyphens, capped — so a long question does not become a long
/// filename, and a question with a slash does not become a directory.
pub fn slug(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut last_dash = true; // leading dashes suppressed
    for ch in s.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= max {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Where a consultation's record is filed. `date` is `YYYY-MM-DD`, passed
/// in rather than read from the clock so this stays pure and a test can
/// pin it.
pub fn doc_path(c: &Consultation, date: &str) -> String {
    let d = if c.domain.is_empty() { "general".to_string() } else { slug(&c.domain.0, 24) };
    format!("{DOC_DIR}/{date}-{d}-{}.md", slug(&c.id, 32))
}

/// The record, as it is committed.
///
/// Front matter first so the file is machine-readable by whatever surveys
/// the directory, then the question, then the answer verbatim. The
/// provisional decision is kept even after an answer arrives: what the run
/// did before being corrected is the interesting part of the record.
pub fn render_doc(c: &Consultation, date: &str) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "---");
    let _ = writeln!(s, "id: {}", c.id);
    let _ = writeln!(s, "domain: {}", c.domain.0);
    let _ = writeln!(s, "date: {date}");
    let _ = writeln!(s, "branch_point_message: {}", c.at_message);
    let _ = writeln!(s, "leverage: {:.2}", c.leverage);
    let _ = writeln!(s, "status: {}", if c.answer.is_some() { "answered" } else { "open" });
    if let Some(a) = &c.answer {
        let _ = writeln!(s, "expert: {}", a.expert);
        let _ = writeln!(s, "answered_at_message: {}", a.at_message);
    }
    let _ = writeln!(s, "---\n");
    let _ = writeln!(s, "# {}\n", c.question.trim());
    let _ = writeln!(s, "## What the run did without an answer\n");
    let _ = writeln!(s, "{}\n", c.provisional.trim());
    match &c.answer {
        Some(a) => {
            let _ = writeln!(s, "## First-hand account\n");
            let _ = writeln!(s, "**{}** — {}\n", a.expert.trim(), a.standing.trim());
            let _ = writeln!(s, "{}\n", a.text.trim());
            let _ = writeln!(
                s,
                "Answered at message {}, {} after the question was raised. \
                 The run can be branched from message {} to apply it.",
                a.at_message,
                a.at_message.saturating_sub(c.at_message),
                c.at_message
            );
        }
        None => {
            let _ = writeln!(s, "## Open\n");
            let _ = writeln!(
                s,
                "No answer yet. Raised at message {}; the run stops at {} \
                 without one.",
                c.at_message,
                c.at_message + STALE_AFTER_MESSAGES
            );
        }
    }
    s
}

/// The glob that reaches every consultation record.
///
/// This is the whole reason the records are markdown files in the repo
/// rather than rows in a store: no new probe kind is needed. `Grep { glob:
/// Some(consult::probe_glob()) }` and `Slice` already read them, so an
/// expert's account is searchable by exactly the machinery that reads
/// source — it joined the pool by being a file.
pub fn probe_glob() -> String {
    format!("{DOC_DIR}/**/*.md")
}

/// The front-matter line a survey uses to skip open questions. An open
/// consultation has no answer in it and is not worth a probe's budget.
pub const ANSWERED_MARKER: &str = "status: answered";

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, at: u64) -> Consultation {
        Consultation {
            id: id.into(),
            domain: Domain::new("Numerical Analysis"),
            question: "Is Simpson's rule sound on a discontinuous integrand here?".into(),
            provisional: "Used Simpson's rule and flagged the discontinuity.".into(),
            leverage: 0.8,
            at_message: at,
            answer: None,
        }
    }

    fn answered(id: &str, at: u64, ans_at: u64) -> Consultation {
        let mut x = c(id, at);
        x.answer = Some(Answer {
            expert: "R. Okonkwo".into(),
            standing: "numerical analysis, 20 years".into(),
            text: "No — split the interval at the jump and Simpson each side.".into(),
            at_message: ans_at,
        });
        x
    }

    #[test]
    fn an_answered_record_is_reachable_by_the_probes_that_read_source() {
        // The join: no new Probe kind, because the record is a file.
        let g = probe_glob();
        assert!(g.starts_with(DOC_DIR) && g.ends_with(".md"), "{g}");
        let doc = render_doc(&answered("simpson", 10, 34), "2026-09-05");
        assert!(doc.contains(ANSWERED_MARKER), "a survey can tell it is worth reading");
        assert!(!render_doc(&c("simpson", 10), "2026-09-05").contains(ANSWERED_MARKER));
        // And the domain is greppable, which is how an expert is matched.
        assert!(doc.contains("domain: numerical analysis"));
    }

    #[test]
    fn a_question_needs_both_low_confidence_and_real_leverage() {
        assert!(should_consult(0.3, 0.9, false));
        // Unsure but nothing depends on it: not worth a person's time.
        assert!(!should_consult(0.3, 0.1, false));
        // Load-bearing but already known: not a question.
        assert!(!should_consult(0.95, 0.9, false));
        // In the run's own field: it should answer this itself.
        assert!(!should_consult(0.3, 0.9, true));
        // Exactly at the thresholds counts — the constants are the line.
        assert!(should_consult(CONSULT_BELOW_CONFIDENCE, WORTH_ASKING, false));
    }

    #[test]
    fn the_run_proceeds_and_only_stops_when_a_question_has_gone_a_hundred_messages() {
        let all = vec![c("simpson", 10)];
        assert_eq!(run_posture(&all, 10), RunPosture::Proceed, "raising it never blocks");
        assert_eq!(run_posture(&all, 10 + STALE_AFTER_MESSAGES - 1), RunPosture::Proceed);
        assert_eq!(run_posture(&all, 10 + STALE_AFTER_MESSAGES), RunPosture::Stop);
        assert_eq!(grace_left(&all[0], 10), Some(STALE_AFTER_MESSAGES));
        assert_eq!(grace_left(&all[0], 10 + STALE_AFTER_MESSAGES + 50), Some(0));
    }

    #[test]
    fn an_answered_question_never_stops_the_run_however_old() {
        let all = vec![answered("simpson", 10, 20)];
        assert_eq!(run_posture(&all, 10_000), RunPosture::Proceed);
        assert_eq!(grace_left(&all[0], 10_000), None);
        // One stale open question is enough to stop, even beside answered ones.
        let mixed = vec![answered("a", 10, 20), c("b", 30)];
        assert_eq!(run_posture(&mixed, 30 + STALE_AFTER_MESSAGES), RunPosture::Stop);
        assert_eq!(open(&mixed).len(), 1);
        assert_eq!(open(&mixed)[0].id, "b");
    }

    #[test]
    fn a_slug_cannot_escape_its_directory_or_run_away_in_length() {
        assert_eq!(slug("Numerical Analysis", 24), "numerical-analysis");
        assert_eq!(slug("../../etc/passwd", 32), "etc-passwd", "no slashes, no dots");
        assert_eq!(slug("  spaced  out  ", 32), "spaced-out");
        assert_eq!(slug("!!!", 32), "", "nothing usable is empty, not a dash");
        assert!(slug(&"x".repeat(500), 20).len() <= 20);
        let p = doc_path(&c("simpson-rule", 10), "2026-09-05");
        assert!(p.starts_with(DOC_DIR) && p.ends_with(".md"), "{p}");
        assert!(!p.contains(".."), "{p}");
        // An empty domain still files somewhere sensible.
        let mut nodomain = c("x", 1);
        nodomain.domain = Domain::new("   ");
        assert!(doc_path(&nodomain, "2026-09-05").contains("general"));
    }

    #[test]
    fn an_open_record_says_when_the_run_will_stop_and_what_it_did_meanwhile() {
        let doc = render_doc(&c("simpson", 10), "2026-09-05");
        assert!(doc.contains("status: open"));
        assert!(doc.contains("branch_point_message: 10"));
        assert!(doc.contains("stops at 110"), "{doc}");
        // The provisional decision is recorded even with no answer — that is
        // what an answer would be overturning.
        assert!(doc.contains("Used Simpson's rule"));
        assert!(!doc.contains("expert:"));
    }

    #[test]
    fn an_answered_record_is_attributed_verbatim_and_names_the_branch_point() {
        let doc = render_doc(&answered("simpson", 10, 34), "2026-09-05");
        assert!(doc.contains("status: answered"));
        assert!(doc.contains("expert: R. Okonkwo"));
        assert!(doc.contains("numerical analysis, 20 years"), "standing is kept");
        assert!(doc.contains("split the interval at the jump"), "verbatim, not summarised");
        assert!(doc.contains("branched from message 10"), "{doc}");
        assert!(doc.contains("24 after"), "the gap is stated: 34 - 10");
        // And the provisional survives the answer.
        assert!(doc.contains("What the run did without an answer"));
    }
}
