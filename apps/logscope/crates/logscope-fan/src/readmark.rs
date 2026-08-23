//! readmark — how much of a node the operator has actually gone past.
//!
//! ## Why this changes the continuation rule
//!
//! [`crate::continuation`] stops dead when the model ends on a question, and
//! the reason given was that continuing "buries it above the next wall of
//! output". That reason assumes a linear transcript. This has a tree graph, so
//! the question does not have to be buried — it can be MARKED, and stay
//! findable while the run carries on.
//!
//! So the stop becomes conditional on whether anything is going to flag it.
//! With marking, a question is a badge on a node; without, it is a hard stop.
//!
//! ## "Passed by" is not "read", and the names say so
//!
//! Nothing here claims the operator read anything. Scroll position is evidence
//! that content went past their eyes, which is weaker than comprehension and
//! stronger than nothing. [`Seen::PassedBy`] is the honest middle: it carries
//! the fraction that was on screen, and it never pretends to be
//! [`Seen::Acknowledged`], which requires a deliberate act.
//!
//! Conflating the two is how a "you have read this" marker ends up lying, and
//! an unanswered question wrongly marked read is worse than one not marked at
//! all — the badge is the thing keeping it findable.
//!
//! ## The watermark is monotonic
//!
//! Coverage only ever rises. Scrolling back up does not un-see what was already
//! passed, and a fraction that fell when the operator scrolled away would make
//! the badge flicker on a node they had finished with.
//!
//! Pure: no clock, no UI, no IO.

use std::collections::HashMap;

/// How far the operator has got with one node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seen {
    /// Never on screen.
    Unseen,
    /// Went past, with the greatest fraction ever visible at once, in `0.0..=1.0`.
    PassedBy(f32),
    /// A deliberate act — expanded, replied to, marked. The only state that
    /// claims intent.
    Acknowledged,
}

impl Seen {
    /// The fraction to show. `Acknowledged` reads as complete regardless of
    /// how much was ever scrolled, because the operator said so.
    pub fn fraction(self) -> f32 {
        match self {
            Seen::Unseen => 0.0,
            Seen::PassedBy(f) => f,
            Seen::Acknowledged => 1.0,
        }
    }

    /// Is this node still owed attention?
    ///
    /// Passing a node at 100% is NOT enough on its own — scrolling past a
    /// question at speed is exactly the case the badge exists for. Only a
    /// deliberate acknowledgement clears one.
    pub fn needs_attention(self, is_question: bool) -> bool {
        if !is_question { return false }
        !matches!(self, Seen::Acknowledged)
    }

    /// A short label for the node in the graph.
    pub fn label(self) -> &'static str {
        match self {
            Seen::Unseen => "",
            Seen::PassedBy(f) if f >= 0.999 => "passed",
            Seen::PassedBy(_) => "part",
            Seen::Acknowledged => "seen",
        }
    }
}

/// Per-node coverage plus the operator's read watermark.
#[derive(Debug, Clone, Default)]
pub struct ReadMarks {
    seen: HashMap<u64, Seen>,
    /// The last node the operator themselves wrote or acted on. Everything
    /// after it is unreviewed by definition.
    watermark: Option<u64>,
}

impl ReadMarks {
    pub fn get(&self, node: u64) -> Seen { self.seen.get(&node).copied().unwrap_or(Seen::Unseen) }

    /// Record that `fraction` of a node was on screen.
    ///
    /// MONOTONIC: only ever raises the recorded coverage, and never downgrades
    /// an acknowledgement. Clamped to `0.0..=1.0` — a scroll computation that
    /// briefly reports 1.4 because of a partial layout pass should not produce
    /// "140% passed".
    pub fn observe(&mut self, node: u64, fraction: f32) {
        let f = fraction.clamp(0.0, 1.0);
        let cur = self.get(node);
        let next = match cur {
            Seen::Acknowledged => Seen::Acknowledged,
            Seen::Unseen => Seen::PassedBy(f),
            Seen::PassedBy(prev) => Seen::PassedBy(prev.max(f)),
        };
        self.seen.insert(node, next);
    }

    /// A deliberate act. Never reversible by scrolling.
    pub fn acknowledge(&mut self, node: u64) { self.seen.insert(node, Seen::Acknowledged); }

    /// Move the read watermark to the operator's latest turn.
    pub fn set_watermark(&mut self, node: u64) { self.watermark = Some(node); }
    pub fn watermark(&self) -> Option<u64> { self.watermark }

    /// Questions still owed an answer, in the order given.
    ///
    /// `nodes` is `(id, is_question)` in conversation order. This is what lets
    /// the graph badge them and what lets the continuation rule keep going
    /// instead of stopping: a question that is listed here is not buried.
    pub fn open_questions(&self, nodes: &[(u64, bool)]) -> Vec<u64> {
        nodes.iter()
            .filter(|(id, q)| self.get(*id).needs_attention(*q))
            .map(|(id, _)| *id)
            .collect()
    }

    /// Nodes after the watermark: everything the operator has not returned to.
    pub fn since_watermark(&self, order: &[u64]) -> Vec<u64> {
        match self.watermark {
            None => order.to_vec(),
            Some(w) => match order.iter().position(|n| *n == w) {
                Some(i) => order[i + 1..].to_vec(),
                // A watermark on a node that is no longer in this branch — the
                // operator forked away. Everything on this path is unreviewed,
                // which is the safe reading.
                None => order.to_vec(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_only_ever_rises() {
        let mut m = ReadMarks::default();
        m.observe(1, 0.4);
        m.observe(1, 0.9);
        m.observe(1, 0.2); // scrolled back up
        assert_eq!(m.get(1), Seen::PassedBy(0.9),
                   "scrolling away must not un-see what was already passed");
    }

    #[test]
    fn a_fraction_is_capped_at_one() {
        let mut m = ReadMarks::default();
        m.observe(1, 1.4);   // a partial layout pass can report this
        assert_eq!(m.get(1).fraction(), 1.0);
        m.observe(2, -0.3);
        assert_eq!(m.get(2).fraction(), 0.0);
    }

    /// The honesty rule: scrolling past is not acknowledging.
    #[test]
    fn passing_a_question_at_full_coverage_does_not_clear_it() {
        let mut m = ReadMarks::default();
        m.observe(1, 1.0);
        assert!(m.get(1).needs_attention(true),
                "scrolling past a question at speed is exactly what the badge is for");
        m.acknowledge(1);
        assert!(!m.get(1).needs_attention(true));
    }

    #[test]
    fn an_acknowledgement_survives_further_scrolling() {
        let mut m = ReadMarks::default();
        m.acknowledge(1);
        m.observe(1, 0.1);
        assert_eq!(m.get(1), Seen::Acknowledged,
                   "a deliberate act must not be undone by a layout event");
        assert_eq!(m.get(1).fraction(), 1.0);
    }

    #[test]
    fn only_questions_are_ever_owed_attention() {
        let mut m = ReadMarks::default();
        m.observe(1, 0.0);
        assert!(!m.get(1).needs_attention(false), "an ordinary turn is not a debt");
    }

    /// What the graph badges, and what lets continuation carry on.
    #[test]
    fn open_questions_lists_the_unacknowledged_ones_in_order() {
        let mut m = ReadMarks::default();
        m.acknowledge(20);
        let nodes = [(10, true), (20, true), (30, false), (40, true)];
        assert_eq!(m.open_questions(&nodes), vec![10, 40],
                   "20 was acknowledged; 30 is not a question");
    }

    #[test]
    fn everything_after_the_watermark_is_unreviewed() {
        let mut m = ReadMarks::default();
        m.set_watermark(2);
        assert_eq!(m.since_watermark(&[1, 2, 3, 4]), vec![3, 4]);
    }

    /// A fork moves the operator off the branch the watermark sits on.
    #[test]
    fn a_watermark_on_another_branch_treats_the_whole_path_as_unreviewed() {
        let mut m = ReadMarks::default();
        m.set_watermark(99); // a node on a branch they forked away from
        assert_eq!(m.since_watermark(&[1, 2, 3]), vec![1, 2, 3],
                   "assuming they had reviewed this path would hide turns they never saw");
    }

    #[test]
    fn no_watermark_means_nothing_has_been_reviewed() {
        let m = ReadMarks::default();
        assert_eq!(m.since_watermark(&[1, 2]), vec![1, 2]);
    }

    /// Labels must distinguish the three states a reader can act on.
    #[test]
    fn labels_separate_partial_from_complete_from_deliberate() {
        assert_eq!(Seen::Unseen.label(), "");
        assert_eq!(Seen::PassedBy(0.5).label(), "part");
        assert_eq!(Seen::PassedBy(1.0).label(), "passed");
        assert_eq!(Seen::Acknowledged.label(), "seen");
        assert_ne!(Seen::PassedBy(1.0).label(), Seen::Acknowledged.label(),
                   "'scrolled past all of it' and 'said yes to it' must not read the same");
    }
}
