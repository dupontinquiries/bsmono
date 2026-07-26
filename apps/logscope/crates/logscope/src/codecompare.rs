//! codecompare.rs — fan one task to several models, then score where they DISAGREE.
//!
//! The useful question when a big model and two small ones answer the same task is not "which is best".
//! It is **where do they diverge**, because agreement is cheap evidence and divergence is where a human
//! should look. Three models writing the same function body is weak but real confirmation; three models
//! writing three different bodies means the task was ambiguous, under-specified, or hard — and that is
//! worth surfacing before the code is applied rather than after it fails.
//!
//! ## Why comparison is structural, not textual
//!
//! Two correct implementations differ in whitespace, comment wording, and identifier choice while being
//! the same program. A textual diff scores those as disagreement and drowns the real signal. So blocks
//! are reduced to a **normalized signature** — comments stripped, whitespace collapsed, string literals
//! folded — and compared on that. Identifiers are deliberately KEPT: `retry_count` versus `n` is a real
//! difference in a code review, and folding them would hide it.
//!
//! ## Why the big model must yield more often
//!
//! Comparison needs comparable units. A model that emits one 400-line answer offers exactly one
//! comparison point, at the end, after all the work is done. `checkpoint_prompt` asks for the task in
//! named, bounded pieces so divergence is caught at piece 2 rather than at the end — which is the only
//! way the feedback loop can steer anything.

use std::collections::BTreeMap;

/// One model's answer: the code blocks it produced, keyed by path.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub model: String,
    /// (path, body) pairs, in the order produced.
    pub blocks: Vec<(String, String)>,
}

/// How much the candidates agreed on one path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Agreement {
    /// Every candidate that produced this path produced the same thing.
    Unanimous,
    /// A clear majority agreed; `0` is how many, `1` is out of how many.
    Majority(usize, usize),
    /// No majority — every candidate differs, or the vote is tied.
    Split,
    /// Only one candidate produced this path at all. Not agreement; absence of comparison.
    Alone,
}

impl Agreement {
    /// Does this warrant a human looking before the code is used?
    ///
    /// `Alone` counts: a block only one model produced has had no review at all, which is a different
    /// risk from a block three models disagreed about but the same answer — look at it.
    pub fn needs_review(self) -> bool { !matches!(self, Agreement::Unanimous) }

    pub fn label(self) -> String {
        match self {
            Agreement::Unanimous => "unanimous".into(),
            Agreement::Majority(n, of) => format!("{n} of {of} agree"),
            Agreement::Split => "split — no majority".into(),
            Agreement::Alone => "only one model produced this".into(),
        }
    }
}

/// The verdict for one path.
#[derive(Clone, Debug, PartialEq)]
pub struct PathVerdict {
    pub path: String,
    pub agreement: Agreement,
    /// Models grouped by the variant they produced, largest group first.
    pub groups: Vec<Vec<String>>,
    /// Which models omitted this path entirely.
    pub missing: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Comparison {
    pub paths: Vec<PathVerdict>,
}

impl Comparison {
    /// Paths a human should look at, worst first.
    pub fn review_queue(&self) -> Vec<&PathVerdict> {
        let mut v: Vec<&PathVerdict> = self.paths.iter().filter(|p| p.agreement.needs_review()).collect();
        let rank = |a: Agreement| match a {
            Agreement::Split => 0, Agreement::Alone => 1, Agreement::Majority(..) => 2, Agreement::Unanimous => 3,
        };
        v.sort_by_key(|p| rank(p.agreement));
        v
    }

    /// Fraction of paths every candidate agreed on. None when nothing was compared.
    pub fn consensus(&self) -> Option<f32> {
        if self.paths.is_empty() { return None; }
        let u = self.paths.iter().filter(|p| p.agreement == Agreement::Unanimous).count();
        Some(u as f32 / self.paths.len() as f32)
    }
}

/// Reduce a code block to what a reviewer would call "the same implementation".
///
/// Strips line comments and blank lines, collapses runs of whitespace, and folds string literals to a
/// placeholder — two blocks differing only in a log message are the same implementation. Identifiers are
/// KEPT deliberately: a different variable name is a real review difference, not noise.
pub fn signature(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut line_buf = String::new();
    for line in body.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with("//") || l.starts_with('#') { continue; }
        // drop a trailing line comment, but not a `//` inside a string
        let code = match l.find("//") {
            Some(i) if l[..i].matches('"').count() % 2 == 0 => &l[..i],
            _ => l,
        };
        let mut in_str = false;
        let mut prev_space = false;
        for ch in code.chars() {
            if ch == '"' {
                if !in_str { out.push_str("\"S\""); }
                in_str = !in_str;
                continue;
            }
            if in_str { continue; }
            if ch.is_whitespace() {
                if !prev_space { line_buf.push(' '); prev_space = true; }
            } else { line_buf.push(ch); prev_space = false; }
        }
        // trim per LINE, not just at the end: stripping a trailing comment leaves the space that
        // preceded it, so `x * 2 // twice` and `x * 2` would otherwise differ by one character.
        let t = line_buf.trim();
        if !t.is_empty() { out.push_str(t); out.push('\n'); }
        line_buf.clear();
    }
    out.trim().to_string()
}

/// Compare candidates path by path.
pub fn compare(candidates: &[Candidate]) -> Comparison {
    let n = candidates.len();
    // path -> model -> signature
    let mut by_path: BTreeMap<&str, Vec<(&str, String)>> = BTreeMap::new();
    for c in candidates {
        for (path, body) in &c.blocks {
            by_path.entry(path.as_str()).or_default().push((c.model.as_str(), signature(body)));
        }
    }
    let mut paths = Vec::new();
    for (path, entries) in by_path {
        let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (m, sig) in &entries { groups.entry(sig.clone()).or_default().push((*m).to_string()); }
        let mut g: Vec<Vec<String>> = groups.into_values().collect();
        g.sort_by_key(|v| std::cmp::Reverse(v.len()));
        let produced = entries.len();
        let largest = g.first().map(|v| v.len()).unwrap_or(0);
        let agreement = if produced <= 1 { Agreement::Alone }
            else if g.len() == 1 { Agreement::Unanimous }
            // a tie is not a majority: two groups of two tells you nothing about which is right
            else if largest * 2 > produced { Agreement::Majority(largest, produced) }
            else { Agreement::Split };
        let missing: Vec<String> = candidates.iter().map(|c| c.model.clone())
            .filter(|m| !entries.iter().any(|(em, _)| em == m)).collect();
        let _ = n;
        paths.push(PathVerdict { path: path.to_string(), agreement, groups: g, missing });
    }
    Comparison { paths }
}

/// The instruction that makes comparison possible at all.
///
/// A model that answers in one long block gives exactly one comparison point, after all the work. This
/// asks for named, bounded pieces so a divergence at piece 2 is caught before piece 7 is built on it.
pub const CHECKPOINT_PROMPT: &str = "\n\nWork in NAMED, BOUNDED pieces. After each piece:\n\
- emit only that piece's code block, with its file path,\n\
- then write a line `--- CHECKPOINT: <piece name> ---` and stop.\n\
Do not continue to the next piece until told to. A piece should be one function, one type, or one \
self-contained change — small enough that another model asked for the same piece would produce something \
directly comparable.";

/// Feedback for the next round, naming exactly where the models diverged.
///
/// Returns None when everything was unanimous: sending "everyone agreed" back is noise that costs context
/// and teaches the model nothing.
pub fn feedback(c: &Comparison) -> Option<String> {
    let q = c.review_queue();
    if q.is_empty() { return None; }
    let mut s = String::from("Other models were asked for the same pieces. They diverged here:\n");
    for p in q.iter().take(6) {
        s.push_str(&format!("- {} — {}", p.path, p.agreement.label()));
        if !p.missing.is_empty() { s.push_str(&format!("; not produced by {}", p.missing.join(", "))); }
        s.push('\n');
    }
    s.push_str("\nFor each: either say why your version is right, or revise it. \
Do not simply restate it — a divergence that neither side explains is still unresolved.");
    Some(s)
}

/// Drives fan-out rounds: ask, compare, feed divergence back, stop when it is not learning.
///
/// The stopping rule matters more than the loop. Two failure modes to avoid: running forever on a task
/// where models will never agree, and stopping at the first disagreement when one more round would have
/// resolved it. So the coordinator stops on CONVERGENCE (nothing left to review), on a ROUND LIMIT, or —
/// the interesting one — when consensus has stopped improving, because a round that changes no minds is
/// evidence the disagreement is substantive rather than accidental.
#[derive(Clone, Debug)]
pub struct Coordinator {
    pub max_rounds: usize,
    history: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Round {
    /// Send this back to the models and go again.
    Continue(String),
    /// Stop, with the reason.
    Stop(StopReason),
}

#[derive(Clone, Debug, PartialEq)]
pub enum StopReason {
    /// Every path is unanimous.
    Converged,
    /// The round budget is spent; the review queue is what remains.
    RoundLimit(usize),
    /// Consensus did not improve — the disagreement is real, not a misunderstanding.
    Stalled(f32),
    /// Nothing was produced to compare.
    NothingToCompare,
}

impl StopReason {
    /// Does a human need to look? Converged is the only clean finish.
    pub fn needs_human(&self) -> bool { !matches!(self, StopReason::Converged) }

    pub fn explain(&self) -> String {
        match self {
            StopReason::Converged => "all models agree on every piece".into(),
            StopReason::RoundLimit(n) => format!("round limit ({n}) reached with disagreements open"),
            StopReason::Stalled(c) => format!(
                "consensus stopped improving at {:.0}% — another round changed no minds, so the disagreement is substantive rather than a misunderstanding", c * 100.0),
            StopReason::NothingToCompare => "no code blocks were produced".into(),
        }
    }
}

impl Default for Coordinator {
    fn default() -> Self { Coordinator { max_rounds: 3, history: Vec::new() } }
}

impl Coordinator {
    pub fn new(max_rounds: usize) -> Self { Coordinator { max_rounds: max_rounds.max(1), history: Vec::new() } }
    pub fn rounds_done(&self) -> usize { self.history.len() }
    pub fn consensus_history(&self) -> &[f32] { &self.history }

    /// Record a round's comparison and decide what happens next.
    pub fn step(&mut self, c: &Comparison) -> Round {
        let Some(cons) = c.consensus() else { return Round::Stop(StopReason::NothingToCompare) };
        let prev = self.history.last().copied();
        self.history.push(cons);
        if c.review_queue().is_empty() { return Round::Stop(StopReason::Converged); }
        if self.history.len() >= self.max_rounds { return Round::Stop(StopReason::RoundLimit(self.max_rounds)); }
        // A round that did not raise consensus will probably not be improved by repeating it. Strict
        // improvement is required: equal counts as stalled, or a stable disagreement loops to the budget.
        if let Some(p) = prev {
            if cons <= p { return Round::Stop(StopReason::Stalled(cons)); }
        }
        match feedback(c) {
            Some(f) => Round::Continue(f),
            None => Round::Stop(StopReason::Converged),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cand(m: &str, blocks: &[(&str, &str)]) -> Candidate {
        Candidate { model: m.into(),
            blocks: blocks.iter().map(|(p, b)| (p.to_string(), b.to_string())).collect() }
    }

    #[test]
    fn formatting_and_comments_are_not_disagreement() {
        // Two correct implementations differ in whitespace and comment wording while being the same
        // program. Scoring those as divergence would drown the real signal.
        let a = "fn f(x: u32) -> u32 {\n    // double it\n    x * 2\n}";
        let b = "fn f(x: u32) -> u32 {\n\n        x   *   2   // twice\n}";
        assert_eq!(signature(a), signature(b));
        let c = compare(&[cand("big", &[("a.rs", a)]), cand("small", &[("a.rs", b)])]);
        assert_eq!(c.paths[0].agreement, Agreement::Unanimous);
        assert_eq!(c.consensus(), Some(1.0));
    }

    #[test]
    fn a_different_identifier_IS_disagreement() {
        // `retry_count` vs `n` is a real difference in review, so identifiers are deliberately kept.
        let a = "let retry_count = 3;";
        let b = "let n = 3;";
        assert_ne!(signature(a), signature(b));
    }

    #[test]
    fn a_different_string_literal_is_not() {
        let a = r#"log("starting up");"#;
        let b = r#"log("begin");"#;
        assert_eq!(signature(a), signature(b), "a log message is not the implementation");
    }

    #[test]
    fn a_tie_is_split_not_a_majority() {
        // Two groups of two tells you nothing about which is right; calling it a majority would give
        // false confidence to a coin flip.
        let c = compare(&[cand("m1", &[("x.rs", "a()")]), cand("m2", &[("x.rs", "a()")]),
                          cand("m3", &[("x.rs", "b()")]), cand("m4", &[("x.rs", "b()")])]);
        assert_eq!(c.paths[0].agreement, Agreement::Split);
        assert!(c.paths[0].agreement.needs_review());
    }

    #[test]
    fn a_real_majority_is_reported_with_its_numbers() {
        let c = compare(&[cand("m1", &[("x.rs", "a()")]), cand("m2", &[("x.rs", "a()")]),
                          cand("m3", &[("x.rs", "b()")])]);
        assert_eq!(c.paths[0].agreement, Agreement::Majority(2, 3));
        assert_eq!(c.paths[0].agreement.label(), "2 of 3 agree");
        assert_eq!(c.paths[0].groups[0].len(), 2, "largest group first");
    }

    #[test]
    fn a_block_only_one_model_produced_is_flagged_not_trusted() {
        // Absence of disagreement is not agreement: nobody checked it.
        let c = compare(&[cand("big", &[("only.rs", "x()")]), cand("small", &[])]);
        assert_eq!(c.paths[0].agreement, Agreement::Alone);
        assert!(c.paths[0].agreement.needs_review());
        assert_eq!(c.paths[0].missing, vec!["small".to_string()]);
    }

    #[test]
    fn the_review_queue_puts_the_worst_first() {
        let c = compare(&[
            cand("m1", &[("split.rs", "a()"), ("agree.rs", "z()"), ("solo.rs", "q()")]),
            cand("m2", &[("split.rs", "b()"), ("agree.rs", "z()")]),
        ]);
        let q = c.review_queue();
        assert_eq!(q[0].path, "split.rs", "a split needs looking at before a solo block");
        assert!(q.iter().all(|p| p.path != "agree.rs"), "unanimous paths stay out of the queue");
    }

    #[test]
    fn unanimous_rounds_send_no_feedback() {
        // "Everyone agreed" costs context and teaches nothing.
        let c = compare(&[cand("m1", &[("x.rs", "a()")]), cand("m2", &[("x.rs", "a()")])]);
        assert!(feedback(&c).is_none());
    }

    #[test]
    fn feedback_names_the_paths_and_demands_a_reason() {
        let c = compare(&[cand("big", &[("x.rs", "a()")]), cand("small", &[("x.rs", "b()")])]);
        let f = feedback(&c).expect("divergence");
        assert!(f.contains("x.rs"));
        assert!(f.contains("either say why your version is right, or revise it"));
        assert!(f.contains("neither side explains is still unresolved"));
    }

    #[test]
    fn the_checkpoint_prompt_asks_for_comparable_units() {
        assert!(CHECKPOINT_PROMPT.contains("CHECKPOINT"));
        assert!(CHECKPOINT_PROMPT.contains("stop"));
        assert!(CHECKPOINT_PROMPT.contains("directly comparable"),
            "the reason for the constraint has to be stated, or a model optimises it away");
    }

    #[test]
    fn the_coordinator_stops_when_a_round_changes_no_minds() {
        // The interesting stopping rule: repeating a round that improved nothing burns budget and tokens
        // on a disagreement that is substantive rather than a misunderstanding.
        let split = compare(&[cand("m1", &[("x.rs", "a()")]), cand("m2", &[("x.rs", "b()")])]);
        let mut co = Coordinator::new(5);
        assert!(matches!(co.step(&split), Round::Continue(_)), "first divergence is worth one retry");
        match co.step(&split) {
            Round::Stop(StopReason::Stalled(c)) => {
                assert!((c - 0.0).abs() < 1e-6);
                assert!(StopReason::Stalled(c).explain().contains("changed no minds"));
                assert!(StopReason::Stalled(c).needs_human());
            }
            other => panic!("expected Stalled, got {other:?}"),
        }
    }

    #[test]
    fn improving_consensus_keeps_going() {
        let mut co = Coordinator::new(5);
        let bad = compare(&[cand("m1", &[("x.rs", "a()"), ("y.rs", "p()")]),
                            cand("m2", &[("x.rs", "b()"), ("y.rs", "q()")])]);
        let better = compare(&[cand("m1", &[("x.rs", "a()"), ("y.rs", "p()")]),
                               cand("m2", &[("x.rs", "b()"), ("y.rs", "p()")])]);
        assert!(matches!(co.step(&bad), Round::Continue(_)));
        assert!(matches!(co.step(&better), Round::Continue(_)), "0% -> 50% is progress; keep going");
        assert_eq!(co.consensus_history(), &[0.0, 0.5]);
    }

    #[test]
    fn convergence_is_the_only_clean_finish() {
        let mut co = Coordinator::new(5);
        let agreed = compare(&[cand("m1", &[("x.rs", "a()")]), cand("m2", &[("x.rs", "a()")])]);
        assert_eq!(co.step(&agreed), Round::Stop(StopReason::Converged));
        assert!(!StopReason::Converged.needs_human());
        for r in [StopReason::RoundLimit(3), StopReason::Stalled(0.5), StopReason::NothingToCompare] {
            assert!(r.needs_human(), "{r:?} leaves something open");
        }
    }

    #[test]
    fn the_round_budget_is_honoured_and_named() {
        let mut co = Coordinator::new(2);
        let split = compare(&[cand("m1", &[("x.rs", "a()")]), cand("m2", &[("x.rs", "b()")])]);
        assert!(matches!(co.step(&split), Round::Continue(_)));
        // second call hits the limit before the stall check, and says which
        assert_eq!(co.step(&split), Round::Stop(StopReason::RoundLimit(2)));
        assert_eq!(co.rounds_done(), 2);
    }

    #[test]
    fn nothing_compared_yields_no_consensus_figure() {
        assert!(Comparison::default().consensus().is_none());
        assert!(compare(&[]).paths.is_empty());
    }
}
