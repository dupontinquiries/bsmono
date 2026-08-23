//! Does `readmark` actually feed `continuation`? (C9-8b)
//!
//! These two modules were built to compose and have never touched. `decide`'s
//! `questions_are_flagged` exists precisely so a graph that can BADGE an
//! unanswered question may keep working instead of stopping dead, and
//! `readmark::open_questions` is named in that field's own doc comment as
//! where the answer comes from. Nothing asserted the pairing, so the contract
//! lived only in prose across two files.
//!
//! An INTEGRATION test on purpose: it can reach only the public surface, which
//! is the surface a real caller — the graph renderer — would have. If the
//! composition needs something not exported, that is a finding, not an
//! inconvenience to work around with `pub(crate)`.
//!
//! WHAT THIS DOES NOT CLOSE. C9-8b's actual gap is two missing UI consumers:
//! nothing supplies scroll fractions to `observe`, and nothing draws the badge.
//! Both are renderer work and neither is verifiable headlessly. What is pinned
//! here is that the logic either side of that UI agrees, so whoever writes the
//! renderer is wiring two halves already known to fit.
//!
//! The dangerous misuse is NOT reachable from here and is worth naming:
//! `questions_are_flagged: true` is a caller's PROMISE to render the badge, not
//! a fact anyone can derive. A caller that passes true and draws nothing gets
//! the worst outcome of all — the run carries on and the question is buried
//! with no mark. No type can catch that; only the renderer can honour it.

use logscope_fan::continuation::{decide, Command, Facts, Verdict};
use logscope_fan::readmark::{ReadMarks, Seen};

/// The reply that ends the turn with a question, in every case below.
const ASKED: &str = "I can take either branch here.\nWhich should I do?";

fn facts(reply: &str, flagged: bool) -> Facts<'_> {
    Facts { reply, remaining_s: Some(3600), auto_turns: 0, max_auto_turns: 8,
            questions_are_flagged: flagged }
}

/// The pairing the field's doc describes: a question readmark still lists as
/// open is one the graph can badge, and a badged question converts the hard
/// stop into a check rather than ending the run.
#[test]
fn a_question_readmark_still_lists_is_one_continuation_will_carry_past() {
    let nodes = [(1u64, false), (2, true)]; // node 2 is the question
    let marks = ReadMarks::default();

    let open = marks.open_questions(&nodes);
    assert_eq!(open, vec![2],
        "readmark did not report the unanswered question, so there is nothing for the graph to \
badge and the flag below would be a lie");

    // A caller that can badge exactly those ids may pass the flag.
    match decide(facts(ASKED, !open.is_empty())) {
        Verdict::Decided(Command::KeepGoingCheck, _) => {}
        other => panic!("a badged question should become a CHECK, not {other:?}"),
    }
}

/// The conservative half, which is the one that must never regress: with no
/// renderer there is nothing to badge, and the run stops rather than burying
/// the question above the next wall of output.
#[test]
fn with_nothing_able_to_badge_it_the_run_stops_dead() {
    match decide(facts(ASKED, false)) {
        Verdict::Decided(Command::StopAndWait, _) => {}
        other => panic!("an unbadgeable question must stop the run, got {other:?}"),
    }
}

/// Scrolling past a question at speed is exactly the case the badge exists
/// for, so it must NOT clear it — even at 100% visibility.
///
/// This is the one that would break silently if `needs_attention` were ever
/// relaxed to treat a full-height pass as reading: the question would vanish
/// from the badge list while the reply still ends in a question mark, and the
/// caller would hand `decide` a flag with nothing behind it.
#[test]
fn scrolling_past_a_question_at_full_height_does_not_answer_it() {
    let nodes = [(2u64, true)];
    let mut marks = ReadMarks::default();
    marks.observe(2, 1.0);

    assert_eq!(marks.get(2), Seen::PassedBy(1.0));
    assert_eq!(marks.open_questions(&nodes), vec![2],
        "a question scrolled fully past was treated as answered — only a deliberate \
acknowledgement may clear one");
}

/// And the deliberate act does clear it. Once acknowledged the question is off
/// the badge list, so the graph stops marking a node the operator has dealt
/// with.
#[test]
fn acknowledging_a_question_takes_it_off_the_badge_list() {
    let nodes = [(2u64, true)];
    let mut marks = ReadMarks::default();
    marks.observe(2, 0.4);
    assert_eq!(marks.open_questions(&nodes), vec![2]);

    marks.acknowledge(2);
    assert!(marks.open_questions(&nodes).is_empty(),
        "an acknowledged question is still being badged, so the mark never clears and the \
operator learns to ignore it");
}

/// The watermark half of the same wiring: what the operator has not returned
/// to since they last spoke. The renderer needs this to know which span to
/// mark, and a fork must not silently report the branch as reviewed.
#[test]
fn a_watermark_on_a_forked_away_node_reports_everything_unreviewed() {
    let mut marks = ReadMarks::default();
    marks.set_watermark(7);

    // 7 is not on this path — the operator forked away from it.
    let order = [1u64, 2, 3];
    assert_eq!(marks.since_watermark(&order), vec![1, 2, 3],
        "a watermark pointing off this branch must read as nothing reviewed; reporting the \
branch as seen would hide unread nodes from the operator");
}
