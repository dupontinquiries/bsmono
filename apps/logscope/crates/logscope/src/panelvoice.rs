//! panelvoice.rs — the panel as a way to loop the operator in, and what selective sharing costs.
//!
//! ## The reframe
//!
//! `panelmode` treats the panel as a cheaper thinker, judged on whether its answer is as good. That is
//! not where its advantage is. A panel can **stop and ask** — several small models producing short
//! turns can each surface a question between messages, where one long unguided generation cannot
//! interrupt itself usefully.
//!
//! So the division is by SESSION SHAPE rather than by question difficulty: the panel is for oriented,
//! guided work where the operator is present and cheap to consult; the thinker takes over when nobody
//! is watching and the run has to carry itself.
//!
//! ## Selective sharing is a control group, not just a hazard
//!
//! Telling one debater something and not another creates an information asymmetry, and
//! `deliberate`'s independence guard does not survive it: that guard assumes debaters differ by MODEL.
//! Once they differ by what they KNOW, agreement among the informed is not corroboration — it may be
//! the hint reaching the same place twice.
//!
//! But run deliberately, that is a **blind control**. If the informed converge and the blind one
//! dissents, the hint drove the conclusion. If the blind one arrives independently, the corroboration
//! is real in a way a fully-informed panel could never demonstrate.
//!
//! So the asymmetry must be *recorded*, and a consensus read without it is worse than no consensus:
//! it carries the authority of agreement and the information content of one hint.
//!
//! ## Interruptions have a budget the operator sets by existing
//!
//! Four debaters each asking a question is four interruptions, and the human becomes the bottleneck the
//! panel was meant to relieve. Similar questions merge; only the highest-value one is asked; and a
//! question whose moment has passed is dropped rather than delivered late.

/// What a debater was told.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Informed {
    /// Saw the operator's input for this round.
    Yes,
    /// Deliberately not told — a control.
    Blind,
}

/// One debater's position, with what it knew.
#[derive(Clone, PartialEq, Debug)]
pub struct Position { pub model: String, pub answer: String, pub informed: Informed }

/// What a set of positions actually demonstrates.
#[derive(Clone, PartialEq, Debug)]
pub enum Reading {
    /// Informed and blind agree. The strongest signal available.
    Corroborated { answer: String },
    /// Only the informed agree; the blind dissents. The hint drove it.
    HintDriven { answer: String, blind_said: String },
    /// Genuine disagreement across the board.
    Split,
    /// Everyone was informed, so nothing controls for the hint.
    Uncontrolled { note: String },
}

/// Read a round, accounting for who knew what.
///
/// Refuses to report corroboration when nobody was blind. That is the case where a consensus reads as
/// strong and is not, because every member saw the same steer.
pub fn read(positions: &[Position]) -> Reading {
    if positions.is_empty() { return Reading::Split; }
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let (blind, informed): (Vec<_>, Vec<_>) =
        positions.iter().partition(|p| p.informed == Informed::Blind);

    if blind.is_empty() {
        let first = norm(&positions[0].answer);
        let all_agree = positions.iter().all(|p| norm(&p.answer) == first);
        return Reading::Uncontrolled { note: if all_agree {
            "every member saw the operator's input, so this agreement may be the hint arriving three \
times rather than three findings. Hold one member blind to tell the difference".into()
        } else { "split, and every member was informed".into() } };
    }
    if informed.is_empty() { return Reading::Split; }

    let inf = norm(&informed[0].answer);
    if !informed.iter().all(|p| norm(&p.answer) == inf) { return Reading::Split; }
    let bl = norm(&blind[0].answer);
    if blind.iter().all(|p| norm(&p.answer) == bl) && bl == inf {
        Reading::Corroborated { answer: informed[0].answer.clone() }
    } else {
        Reading::HintDriven { answer: informed[0].answer.clone(), blind_said: blind[0].answer.clone() }
    }
}

/// A question a debater wants to put to the operator.
#[derive(Clone, PartialEq, Debug)]
pub struct Ask {
    pub from: String,
    pub text: String,
    /// How much the answer would change what this debater does, 0..1.
    pub leverage: f32,
    /// Round it was raised in.
    pub round: u32,
}

/// Rounds after which a question is stale.
///
/// Short: the panel has moved on, and an answer to a question the run no longer depends on costs the
/// operator's attention for nothing. A late question is worse than none — it teaches people to ignore
/// the prompts.
pub const ASK_TTL_ROUNDS: u32 = 2;

/// Choose what to actually put to the operator.
///
/// Merges near-duplicates and returns at most `budget` questions, highest leverage first. Four debaters
/// each asking is four interruptions, and then the human is the bottleneck the panel was meant to
/// relieve.
pub fn triage(asks: &[Ask], now_round: u32, budget: usize) -> Vec<Ask> {
    // Punctuation is stripped before comparing: "threads" and "threads?" are the same question, and a
    // key that treats them as different lets the same interruption through twice.
    let key = |t: &str| t.to_lowercase().split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|w| w.len() > 3).take(6).collect::<Vec<_>>().join(" ");
    let mut kept: Vec<Ask> = Vec::new();
    let mut fresh: Vec<&Ask> = asks.iter()
        .filter(|a| now_round.saturating_sub(a.round) <= ASK_TTL_ROUNDS).collect();
    fresh.sort_by(|a, b| b.leverage.partial_cmp(&a.leverage).unwrap_or(std::cmp::Ordering::Equal)
        .then(a.from.cmp(&b.from)));
    for a in fresh {
        // A near-duplicate does not earn a second interruption; the first asker carries it.
        if kept.iter().any(|k| key(&k.text) == key(&a.text)) { continue; }
        if kept.len() >= budget { break; }
        kept.push(a.clone());
    }
    kept
}

/// Who the liaison should share an operator's answer with.
#[derive(Clone, PartialEq, Debug)]
pub enum Share {
    /// Everyone. Fast, and destroys the control.
    All,
    /// Only the asker, keeping the rest blind.
    AskerOnly,
    /// Everyone except a named control.
    AllButControl { control: String },
}

impl Share {
    /// Does this preserve a blind member?
    pub fn keeps_control(&self) -> bool { !matches!(self, Share::All) }

    pub fn why(&self) -> &'static str {
        match self {
            Share::All => "fastest, and it ends the round's ability to distinguish a corroborated \
answer from a hint arriving several times",
            Share::AskerOnly => "keeps every other member blind, so the next round can tell whether the \
answer or the question drove them",
            Share::AllButControl { .. } => "propagates the answer while holding one member back as a \
control — the usual choice when the answer matters and the check still does too",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(m: &str, a: &str, i: Informed) -> Position {
        Position { model: m.into(), answer: a.into(), informed: i }
    }

    #[test]
    fn agreement_between_informed_and_BLIND_is_the_strong_signal() {
        // The blind member arrived there without the steer, which a fully-informed panel can never
        // demonstrate however many members it has.
        let r = read(&[p("a", "use a BTreeMap", Informed::Yes),
                       p("b", "use a BTreeMap", Informed::Yes),
                       p("c", "Use a btreemap", Informed::Blind)]);
        assert!(matches!(r, Reading::Corroborated { .. }), "{r:?}");
    }

    #[test]
    fn informed_agreement_with_a_dissenting_blind_is_HINT_DRIVEN() {
        // The finding this design exists to produce: the operator's steer reached the same place twice
        // and the question did not. Reported as such rather than as a 2-1 majority.
        let r = read(&[p("a", "use a BTreeMap", Informed::Yes),
                       p("b", "use a BTreeMap", Informed::Yes),
                       p("c", "use a HashMap", Informed::Blind)]);
        match r {
            Reading::HintDriven { blind_said, .. } => assert!(blind_said.contains("HashMap")),
            other => panic!("expected HintDriven, got {other:?}"),
        }
    }

    #[test]
    fn a_fully_informed_panel_cannot_report_corroboration() {
        // The case where consensus reads strong and is not: every member saw the same steer, so
        // agreement may be the hint arriving three times. Refusing to call that corroborated is the
        // whole point of tracking who knew what.
        let r = read(&[p("a", "X", Informed::Yes), p("b", "X", Informed::Yes),
                       p("c", "X", Informed::Yes)]);
        match r {
            Reading::Uncontrolled { note } => {
                assert!(note.contains("hint arriving three times"));
                assert!(note.contains("Hold one member blind"));
            }
            other => panic!("expected Uncontrolled, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_questions_earn_ONE_interruption() {
        // Four debaters each asking is four interruptions, and then the human is the bottleneck the
        // panel was meant to relieve.
        let asks = vec![
            Ask { from: "a".into(), text: "should this use async or threads".into(), leverage: 0.9, round: 1 },
            Ask { from: "b".into(), text: "Should this use async, or threads?".into(), leverage: 0.8, round: 1 },
            Ask { from: "c".into(), text: "which database are we targeting".into(), leverage: 0.7, round: 1 },
        ];
        let out = triage(&asks, 1, 4);
        assert_eq!(out.len(), 2, "the near-duplicate merged: {out:?}");
        assert_eq!(out[0].from, "a", "highest leverage carries it");
    }

    #[test]
    fn a_stale_question_is_DROPPED_rather_than_asked_late() {
        // The panel has moved on. An answer to a question the run no longer depends on costs attention
        // for nothing, and late prompts teach people to ignore prompts.
        let old = vec![Ask { from: "a".into(), text: "still relevant?".into(), leverage: 1.0, round: 1 }];
        assert!(triage(&old, 1 + ASK_TTL_ROUNDS, 4).len() == 1, "just inside the window");
        assert!(triage(&old, 2 + ASK_TTL_ROUNDS, 4).is_empty(), "past it, dropped");
    }

    #[test]
    fn the_budget_caps_interruptions_even_when_every_question_is_good() {
        // Genuinely different questions. My first fixture varied only by a one-character number,
        // which the key filters out as too short — so all six deduped to one, and the dedup was right.
        let topics = ["async runtime choice", "database engine selection", "error type design",
                      "logging framework pick", "serialization format", "test harness layout"];
        let asks: Vec<Ask> = topics.iter().enumerate().map(|(i, t)| Ask { from: format!("m{i}"),
            text: format!("which {t} should we commit to here"), leverage: 0.9, round: 0 }).collect();
        assert_eq!(triage(&asks, 0, 2).len(), 2);
    }

    #[test]
    fn sharing_with_everyone_is_the_option_that_ENDS_the_control() {
        // Fast and sometimes right, but it must be a visible choice rather than a default — the cost is
        // paid in the next round's readability, not now.
        assert!(!Share::All.keeps_control());
        assert!(Share::AskerOnly.keeps_control());
        assert!(Share::AllButControl { control: "c".into() }.keeps_control());
        assert!(Share::All.why().contains("ends the round's ability to distinguish"));
    }

    #[test]
    fn a_round_with_no_informed_members_is_not_corroboration_either() {
        // Everyone blind means nobody saw the question's context; that is a different failure from
        // everyone informed, and neither is a corroborated answer.
        let r = read(&[p("a", "X", Informed::Blind), p("b", "X", Informed::Blind)]);
        assert_eq!(r, Reading::Split);
    }
}
