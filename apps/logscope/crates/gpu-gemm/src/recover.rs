//! recover.rs — deciding what a failed branch is allowed to do next, from evidence alone.
//!
//! The failure being designed against is the one that makes text feedback unreliable:
//!
//! ```text
//! error -> plausible interpretation -> wrong edit -> new error -> plausible interpretation -> ...
//! ```
//!
//! Every step of that loop looks locally reasonable, and no amount of rewording the feedback stops
//! it, because the model is not failing to understand the error — it is failing to leave the region
//! of the state space the error came from. Steering (see `steer.rs`) tries to move that state. This
//! module does something cruder and much more likely to work at 8B: it takes the decision away.
//!
//! ## The rule that does the work
//!
//! A branch may keep patching only while the diagnostic set is STRICTLY SHRINKING. The moment a
//! diagnostic recurs, patching has demonstrably not worked, and the branch is rolled back to the
//! checkpoint before the attempt that first produced that diagnostic — not to an arbitrary depth,
//! and not to a depth the model picks.
//!
//! That last clause is the point. Rollback depth derived from evidence is a bounded state change;
//! rollback depth chosen by the failing model is another prompt.
//!
//! ## Why diagnostic IDENTITY and not diagnostic count
//!
//! Counting is defeated immediately: an edit that trades one type error for two is progress by no
//! measure, and an edit that trades three for one may have deleted the code rather than fixed it.
//! Identity is what distinguishes "moved" from "moved in a circle". A `DiagKey` is whatever the
//! evaluator can make stable across edits — rustc code plus a span-free normalisation of the
//! message is the obvious one, and deliberately not this module's business, because a pure policy
//! that also owns error parsing cannot be tested without a compiler.
//!
//! ## Budget is the feedback of last resort
//!
//! Attempts cost budget; unproductive attempts cost more. When budget runs out the branch is
//! abandoned and selection reclaims its pages. Nothing is ever told it is doing badly — it simply
//! stops being given compute. That is the one signal in this whole design that a model cannot
//! misread.
//!
//! Pure: no wgpu, no model, no compiler. Every rule below is decided from counts and sets, which is
//! why the awkward cases have tests.

use std::collections::{BTreeMap, BTreeSet};

/// A stable identity for one diagnostic, supplied by the evaluator.
///
/// Stability across edits is the whole contract: the same underlying problem must hash the same
/// after an unrelated line is added above it, or the recurrence rule below never fires and this
/// module degrades to "patch until budget runs out".
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct DiagKey(pub u64);

/// What executable reality reported about one attempt.
#[derive(Clone, Debug, Default)]
pub struct Evidence {
    pub compiled: bool,
    pub tests_passed: u32,
    pub tests_total: u32,
    /// Compiler errors, failing lints, and failing test identities, all as stable keys. Empty and
    /// `compiled` means the attempt is a candidate; empty and not `compiled` is an evaluator bug and
    /// is treated as a failure with an unknown cause (see `Move::Abandon`).
    pub diags: Vec<DiagKey>,
}

impl Evidence {
    pub fn set(&self) -> BTreeSet<DiagKey> { self.diags.iter().copied().collect() }

    /// Did everything pass? The one state this module has no opinion about — the caller promotes it.
    pub fn clean(&self) -> bool {
        self.compiled && self.diags.is_empty() && self.tests_passed == self.tests_total
    }

    /// A scalar for ranking at the round boundary. Compilation dominates because an uncompilable
    /// branch has produced no evidence about its own design, only about its syntax; test ratio
    /// breaks ties among branches that at least ran.
    pub fn score(&self) -> f32 {
        if !self.compiled { return 0.0; }
        let ratio = if self.tests_total == 0 { 0.0 } else { self.tests_passed as f32 / self.tests_total as f32 };
        0.25 + 0.75 * ratio
    }
}

/// What the branch is permitted to do next.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Move {
    /// Keep going from the current state. Earned by strictly shrinking the diagnostic set.
    Patch,
    /// Discard everything after checkpoint `to_depth` — KV, workspace, and generated tokens — and
    /// resume from there. `cause` is the diagnostic whose recurrence triggered it, for the log.
    Rollback { to_depth: usize, cause: DiagKey },
    /// Out of budget, or already at the shallowest checkpoint with nothing left to undo. The
    /// scheduler stops funding it and selection reclaims its pages.
    Abandon,
}

/// Costs, in budget units. Not tuned — chosen so that the ordering is visible in the constants
/// rather than buried in the arithmetic: productive work is cheap, churn is expensive, and undoing
/// a commitment costs most because it throws away real compute.
pub const COST_PRODUCTIVE: u32 = 1;
pub const COST_CHURN: u32 = 3;
pub const COST_ROLLBACK: u32 = 5;

/// A branch's attempt history and remaining budget.
///
/// `depth` is the checkpoint index the branch is currently working above. It increases when the
/// branch commits (the caller decides what a commitment is — typically a passing compile, or a
/// structural edit it wants to be able to return to) and is what rollback targets.
pub struct History {
    budget: u32,
    depth: usize,
    /// One entry per attempt: the diagnostic set it produced, and the depth it was made at.
    attempts: Vec<(BTreeSet<DiagKey>, usize)>,
    /// First attempt index at which each diagnostic was seen, and the depth in force then.
    first_seen: BTreeMap<DiagKey, (usize, usize)>,
}

impl History {
    pub fn new(budget: u32) -> Self {
        History { budget, depth: 0, attempts: Vec::new(), first_seen: BTreeMap::new() }
    }

    pub fn budget(&self) -> u32 { self.budget }
    pub fn depth(&self) -> usize { self.depth }
    pub fn attempts(&self) -> usize { self.attempts.len() }

    /// Mark a point worth returning to. Called by the caller, not inferred here — this module has no
    /// way to tell a structural commitment from an ordinary edit, and guessing would put rollback
    /// targets in arbitrary places.
    pub fn commit(&mut self) { self.depth += 1; }

    /// Record an attempt's evidence and decide the next move.
    ///
    /// The rules, in the order they are checked:
    ///
    /// 1. **Clean** — no move; the caller promotes. Recorded so the history stays complete.
    /// 2. **Budget exhausted** — `Abandon`, whatever the evidence says.
    /// 3. **A diagnostic recurs** — `Rollback` to just below the depth at which it first appeared.
    ///    Recurrence is the objective statement that patching this problem has not worked.
    /// 4. **Strictly shrinking** — `Patch`, cheap.
    /// 5. **Anything else** (grew, or changed without shrinking) — churn. Charged more, and allowed
    ///    to continue only until budget says otherwise. Churn is not immediately fatal because the
    ///    first edit after a rollback legitimately looks like churn: a different approach produces
    ///    different errors.
    pub fn record(&mut self, ev: &Evidence) -> Option<Move> {
        let now = ev.set();
        let idx = self.attempts.len();
        self.attempts.push((now.clone(), self.depth));
        for k in &now { self.first_seen.entry(*k).or_insert((idx, self.depth)); }

        if ev.clean() { return None; }

        if self.budget == 0 { return Some(Move::Abandon); }

        // 3. Recurrence: a key present now that was also present in some EARLIER attempt, with at
        // least one attempt in between where it was absent. A key that has simply never gone away
        // is not recurrence — it is a problem being worked on, and the shrink rule governs it.
        if let Some((cause, first_depth)) = self.recurred(&now, idx) {
            self.budget = self.budget.saturating_sub(COST_ROLLBACK);
            if first_depth == 0 {
                // The commitment that caused this is the branch's own root. There is nothing above
                // it to undo, so the branch cannot recover by rolling back and must be replaced by
                // a fork from elsewhere in the population.
                return Some(Move::Abandon);
            }
            self.depth = first_depth - 1;
            return Some(Move::Rollback { to_depth: self.depth, cause });
        }

        // 4. Strict shrink against the immediately preceding attempt.
        if idx > 0 {
            let prev = &self.attempts[idx - 1].0;
            if now.is_subset(prev) && now.len() < prev.len() {
                self.budget = self.budget.saturating_sub(COST_PRODUCTIVE);
                return Some(if self.budget == 0 { Move::Abandon } else { Move::Patch });
            }
        }

        // 5. Churn, and the first attempt (which has nothing to shrink against).
        self.budget = self.budget.saturating_sub(if idx == 0 { COST_PRODUCTIVE } else { COST_CHURN });
        Some(if self.budget == 0 { Move::Abandon } else { Move::Patch })
    }

    /// A key in `now` that was present earlier, then absent, and is back. Returns it with the depth
    /// at which it was first seen. Picks the earliest-introduced such key, because rolling back to
    /// the oldest offending commitment subsumes the newer ones.
    fn recurred(&self, now: &BTreeSet<DiagKey>, idx: usize) -> Option<(DiagKey, usize)> {
        let mut best: Option<(usize, DiagKey, usize)> = None;
        for k in now {
            // `record` inserts every key of `now` before calling this, so a miss is impossible
            // today — but `?` here would abort the whole scan on one absent key and silently return
            // "no recurrence", which is the failure this module exists to catch.
            let Some(&(first_idx, first_depth)) = self.first_seen.get(k) else { continue };
            if first_idx >= idx { continue; }
            // Was it absent at any point between first sighting and now?
            let gapped = self.attempts[first_idx + 1..idx].iter().any(|(s, _)| !s.contains(k));
            if !gapped { continue; }
            if best.map_or(true, |(bi, _, _)| first_idx < bi) { best = Some((first_idx, *k, first_depth)); }
        }
        best.map(|(_, k, d)| (k, d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(compiled: bool, diags: &[u64]) -> Evidence {
        Evidence {
            compiled,
            tests_passed: 0,
            tests_total: 0,
            diags: diags.iter().map(|k| DiagKey(*k)).collect(),
        }
    }

    #[test]
    fn a_clean_attempt_asks_for_no_move() {
        let mut h = History::new(20);
        let e = Evidence { compiled: true, tests_passed: 5, tests_total: 5, diags: vec![] };
        assert!(e.clean());
        assert_eq!(h.record(&e), None);
        assert_eq!(h.attempts(), 1, "clean attempts still enter the history");
    }

    #[test]
    fn compiling_beats_not_compiling_regardless_of_tests() {
        let broken = Evidence { compiled: false, tests_passed: 0, tests_total: 10, diags: vec![DiagKey(1)] };
        let runs = Evidence { compiled: true, tests_passed: 0, tests_total: 10, diags: vec![DiagKey(2)] };
        assert!(runs.score() > broken.score());
    }

    #[test]
    fn strictly_shrinking_diagnostics_earn_cheap_patches() {
        let mut h = History::new(20);
        h.record(&ev(false, &[1, 2, 3]));
        let before = h.budget();
        assert_eq!(h.record(&ev(false, &[1, 2])), Some(Move::Patch));
        assert_eq!(before - h.budget(), COST_PRODUCTIVE);
        assert_eq!(h.record(&ev(false, &[1])), Some(Move::Patch));
    }

    #[test]
    fn a_recurring_diagnostic_forces_a_rollback_to_before_its_first_sighting() {
        let mut h = History::new(40);
        h.commit();               // depth 1
        h.record(&ev(false, &[7]));   // key 7 first seen at depth 1
        h.commit();               // depth 2
        h.record(&ev(false, &[9]));   // 7 gone
        let m = h.record(&ev(false, &[7]));
        match m {
            Some(Move::Rollback { to_depth, cause }) => {
                assert_eq!(cause, DiagKey(7));
                assert_eq!(to_depth, 0, "first seen at depth 1, so roll back below it");
            }
            other => panic!("expected rollback, got {other:?}"),
        }
        assert_eq!(h.depth(), 0);
    }

    #[test]
    fn a_diagnostic_that_never_went_away_is_not_recurrence() {
        // The distinction that keeps a branch from being rolled back while it is legitimately
        // chipping away at a persistent error.
        let mut h = History::new(40);
        h.record(&ev(false, &[1, 2, 3]));
        h.record(&ev(false, &[1, 2]));
        assert_eq!(h.record(&ev(false, &[1])), Some(Move::Patch));
    }

    #[test]
    fn churn_costs_more_than_progress() {
        let mut h = History::new(40);
        h.record(&ev(false, &[1]));
        let before = h.budget();
        // Traded one error for two different ones: not a shrink, not a recurrence.
        assert_eq!(h.record(&ev(false, &[2, 3])), Some(Move::Patch));
        assert_eq!(before - h.budget(), COST_CHURN);
    }

    #[test]
    fn oscillation_is_caught_as_recurrence_not_as_endless_churn() {
        let mut h = History::new(60);
        h.commit();
        h.record(&ev(false, &[1]));
        h.record(&ev(false, &[2]));
        let m = h.record(&ev(false, &[1]));
        assert!(matches!(m, Some(Move::Rollback { cause: DiagKey(1), .. })));
    }

    #[test]
    fn a_recurrence_rooted_at_depth_zero_abandons_because_there_is_nothing_to_undo() {
        let mut h = History::new(40);
        h.record(&ev(false, &[5]));   // depth 0
        h.record(&ev(false, &[6]));
        assert_eq!(h.record(&ev(false, &[5])), Some(Move::Abandon));
    }

    #[test]
    fn budget_exhaustion_abandons_even_while_making_progress() {
        // The signal that cannot be misread: it stops being funded.
        let mut h = History::new(2);
        h.record(&ev(false, &[1, 2, 3]));      // first attempt, COST_PRODUCTIVE -> 1
        assert_eq!(h.record(&ev(false, &[1, 2])), Some(Move::Abandon)); // -> 0
        assert_eq!(h.budget(), 0);
        assert_eq!(h.record(&ev(false, &[1])), Some(Move::Abandon));
    }

    #[test]
    fn rollback_lowers_the_depth_so_a_second_rollback_targets_shallower_ground() {
        let mut h = History::new(80);
        h.commit(); h.commit(); h.commit();   // depth 3
        h.record(&ev(false, &[10]));
        h.record(&ev(false, &[11]));
        assert!(matches!(h.record(&ev(false, &[10])), Some(Move::Rollback { to_depth: 2, .. })));
        assert_eq!(h.depth(), 2);
        h.record(&ev(false, &[20]));
        h.record(&ev(false, &[21]));
        assert!(matches!(h.record(&ev(false, &[20])), Some(Move::Rollback { to_depth: 1, .. })));
    }

    #[test]
    fn an_empty_diag_set_that_did_not_compile_is_still_a_failure() {
        // Evaluator bug shape: exit code says no, diagnostics say nothing. Must not be read as clean.
        let mut h = History::new(10);
        let e = ev(false, &[]);
        assert!(!e.clean());
        assert!(h.record(&e).is_some());
    }
}
