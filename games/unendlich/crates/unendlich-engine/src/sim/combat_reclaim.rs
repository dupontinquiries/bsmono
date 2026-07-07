//! `sim::combat_reclaim` — Kaironyx fold-decision core (spec §15).
//!
//! Kaironyx (the Recurrent) can fold its own combat state back up to 30 s to
//! the most *favorable* past snapshot when a fight turns against it. This
//! module is the pure decision core: given a ring of snapshots, current damage
//! pressure, and favorability weights, it decides **whether** to fold and
//! **which** snapshot to fold to.
//!
//! # Decision logic
//!
//! 1. Discard any snapshot with `t > 30.0` (outside the fold window).
//! 2. Discard any snapshot older than the **most-recent anchor** (`anchor ==
//!    true`). Tier-0 anchors mark un-rewindable moments; the dragon cannot fold
//!    past one. If no anchor exists, all remaining snapshots are candidates.
//! 3. Compute `favorability = hp_weight * hp + pos_weight * pos_score` for each
//!    candidate and for the current state (the snapshot with the smallest `t`
//!    among candidates, or a synthetic "current" derived from index 0 if
//!    present).
//! 4. Fold only if **both** conditions hold:
//!    - `damage_pressure >= FOLD_THRESHOLD` (enough pressure to warrant a fold)
//!    - The best candidate is *strictly more favorable* than the current state
//! 5. Return the index into the **original `snaps` slice** of the chosen
//!    snapshot. Ties in favorability break toward the snapshot with smaller `t`
//!    (more recent).
//!
//! # Constants
//!
//! | constant | value | meaning |
//! |---|---|---|
//! | `FOLD_THRESHOLD` | 0.5 | min `damage_pressure` to even consider folding |
//! | `MAX_FOLD_AGE` | 30.0 | max `t` for a snapshot to be a candidate |

/// Minimum damage pressure required before a fold is considered.
pub const FOLD_THRESHOLD: f32 = 0.5;

/// Snapshots older than this (seconds-ago) are outside the fold window.
pub const MAX_FOLD_AGE: f32 = 30.0;

/// A single self-state snapshot on the ring buffer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snapshot {
    /// Seconds ago this snapshot was captured (`0` = current moment).
    pub t: f32,
    /// Health at this moment (normalised `[0, 1]` typical, unclamped here).
    pub hp: f32,
    /// Positional advantage score at this moment.
    pub pos_score: f32,
    /// Whether this is a Tier-0 anchor (cannot rewind past it).
    pub anchor: bool,
}

/// The output of [`decide`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldDecision {
    /// Whether to fold at all.
    pub fold: bool,
    /// Index into the original `snaps` slice of the chosen snapshot.
    /// `None` when `fold == false`.
    pub index: Option<usize>,
}

impl FoldDecision {
    fn no() -> FoldDecision {
        FoldDecision { fold: false, index: None }
    }
    fn yes(index: usize) -> FoldDecision {
        FoldDecision { fold: true, index: Some(index) }
    }
}

/// Compute the favorability score for a snapshot under the given weights.
#[inline]
fn favorability(snap: &Snapshot, hp_weight: f32, pos_weight: f32) -> f32 {
    hp_weight * snap.hp + pos_weight * snap.pos_score
}

/// Decide whether Kaironyx should fold, and to which snapshot.
///
/// See the [module-level documentation](self) for the full decision algorithm.
///
/// # Panics
///
/// Never panics regardless of input (including NaN/inf in snapshot fields).
pub fn decide(
    snaps: &[Snapshot],
    damage_pressure: f32,
    hp_weight: f32,
    pos_weight: f32,
) -> FoldDecision {
    // Gate 1: pressure must meet threshold.
    if !(damage_pressure >= FOLD_THRESHOLD) {
        return FoldDecision::no();
    }

    if snaps.is_empty() {
        return FoldDecision::no();
    }

    // Find the most-recent anchor (smallest t among anchored snapshots).
    // "Most recent" = smallest t. We want to EXCLUDE snapshots OLDER than it,
    // i.e. with larger t.
    let anchor_t: Option<f32> = snaps
        .iter()
        .filter(|s| s.anchor && s.t <= MAX_FOLD_AGE)
        .map(|s| s.t)
        .reduce(f32::min);

    // Build candidate set: within fold window AND not past anchor.
    // "Past the anchor" means older, i.e. t > anchor_t.
    let candidates: Vec<(usize, &Snapshot)> = snaps
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            // Must be within fold window.
            if !(s.t <= MAX_FOLD_AGE) {
                return false;
            }
            // Must not be older than the most-recent anchor.
            if let Some(at) = anchor_t {
                if s.t > at {
                    return false;
                }
            }
            true
        })
        .collect();

    if candidates.is_empty() {
        return FoldDecision::no();
    }

    // Identify the "current" state: the candidate with the smallest t.
    // (t ≈ 0 = now).
    let current = candidates
        .iter()
        .min_by(|a, b| a.1.t.partial_cmp(&b.1.t).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or(&candidates[0]);

    let current_fav = favorability(current.1, hp_weight, pos_weight);

    // Find the best candidate: highest favorability, ties broken by smallest t.
    let best = candidates
        .iter()
        .max_by(|a, b| {
            let fa = favorability(a.1, hp_weight, pos_weight);
            let fb = favorability(b.1, hp_weight, pos_weight);
            match fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal) {
                std::cmp::Ordering::Equal => {
                    // Tie: prefer more-recent (smaller t). Reverse order: smaller t → Greater.
                    b.1.t.partial_cmp(&a.1.t).unwrap_or(std::cmp::Ordering::Equal)
                }
                ord => ord,
            }
        })
        .unwrap_or(&candidates[0]);

    let best_fav = favorability(best.1, hp_weight, pos_weight);

    // Gate 2: best must be strictly more favorable than current.
    if !(best_fav > current_fav) {
        return FoldDecision::no();
    }

    FoldDecision::yes(best.0)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(t: f32, hp: f32, pos_score: f32, anchor: bool) -> Snapshot {
        Snapshot { t, hp, pos_score, anchor }
    }

    // ── Low pressure → no fold ──

    #[test]
    fn low_pressure_no_fold() {
        let snaps = [snap(5.0, 0.9, 0.8, false)];
        let d = decide(&snaps, FOLD_THRESHOLD - 0.01, 1.0, 1.0);
        assert!(!d.fold);
        assert!(d.index.is_none());
    }

    #[test]
    fn zero_pressure_no_fold() {
        let snaps = [snap(5.0, 1.0, 1.0, false)];
        let d = decide(&snaps, 0.0, 1.0, 1.0);
        assert!(!d.fold);
    }

    // ── Exact threshold folds when there's a better candidate ──

    #[test]
    fn exact_threshold_folds() {
        // current: t=1, hp=0.3; past: t=10, hp=0.9 → past is better
        let snaps = [snap(1.0, 0.3, 0.0, false), snap(10.0, 0.9, 0.0, false)];
        let d = decide(&snaps, FOLD_THRESHOLD, 1.0, 0.0);
        assert!(d.fold);
        assert_eq!(d.index, Some(1));
    }

    // ── High pressure + strictly-better past → fold ──

    #[test]
    fn high_pressure_better_past_folds() {
        let snaps = [
            snap(0.5, 0.2, 0.1, false),  // current (smallest t)
            snap(10.0, 0.8, 0.9, false), // clearly better
            snap(20.0, 0.5, 0.5, false), // middling
        ];
        let d = decide(&snaps, 0.8, 1.0, 1.0);
        assert!(d.fold);
        assert_eq!(d.index, Some(1)); // index 1 is most favorable
    }

    // ── Most favorable chosen, not oldest ──

    #[test]
    fn most_favorable_chosen_not_oldest() {
        let snaps = [
            snap(1.0, 0.2, 0.0, false),  // current
            snap(10.0, 0.5, 0.0, false), // ok
            snap(20.0, 0.3, 0.0, false), // older but worse
            snap(15.0, 0.9, 0.0, false), // best
        ];
        let d = decide(&snaps, 0.9, 1.0, 0.0);
        assert!(d.fold);
        assert_eq!(d.index, Some(3)); // index 3 has hp=0.9
    }

    // ── Current state best → no fold ──

    #[test]
    fn current_state_best_no_fold() {
        let snaps = [
            snap(0.0, 1.0, 1.0, false), // current — the best
            snap(10.0, 0.3, 0.2, false),
            snap(20.0, 0.4, 0.1, false),
        ];
        let d = decide(&snaps, 0.9, 1.0, 1.0);
        assert!(!d.fold);
    }

    #[test]
    fn equal_favorability_no_fold() {
        // Past snapshot has identical favorability to current → not strictly better
        let snaps = [
            snap(1.0, 0.5, 0.5, false),
            snap(15.0, 0.5, 0.5, false),
        ];
        let d = decide(&snaps, 0.9, 1.0, 1.0);
        assert!(!d.fold);
    }

    // ── Anchor blocking ──

    #[test]
    fn cannot_fold_past_anchor() {
        // Anchor at t=8; snapshot at t=20 is beyond it → excluded
        let snaps = [
            snap(1.0, 0.2, 0.0, false),  // current
            snap(8.0, 0.5, 0.0, true),   // anchor
            snap(20.0, 0.9, 0.0, false), // beyond anchor → excluded
        ];
        let d = decide(&snaps, 0.9, 1.0, 0.0);
        // Only candidates: t=1 and t=8 (anchor itself is a candidate)
        // anchor (t=8, hp=0.5) > current (t=1, hp=0.2) → should fold to index 1
        assert!(d.fold);
        assert_eq!(d.index, Some(1));
    }

    #[test]
    fn anchor_at_t0_blocks_all_past_snapshots() {
        // Anchor at t=0: no snapshot older than 0 is a candidate
        let snaps = [
            snap(0.0, 0.2, 0.0, true),  // anchor at present
            snap(10.0, 0.9, 0.0, false), // older → excluded
        ];
        let d = decide(&snaps, 0.9, 1.0, 0.0);
        // Only candidate is t=0 which is current → no strictly-better past
        assert!(!d.fold);
    }

    #[test]
    fn most_recent_anchor_used_not_oldest() {
        // Two anchors: t=5 and t=15. Most recent = t=5.
        // Snapshot at t=12 would be past anchor t=5 → excluded.
        // Snapshot at t=3 is within anchor → candidate.
        let snaps = [
            snap(1.0, 0.2, 0.0, false),  // current
            snap(3.0, 0.7, 0.0, false),  // within t=5 anchor
            snap(5.0, 0.6, 0.0, true),   // anchor (most recent)
            snap(12.0, 0.9, 0.0, false), // past anchor → excluded
            snap(15.0, 0.0, 0.0, true),  // older anchor (not the binding one)
        ];
        let d = decide(&snaps, 0.9, 1.0, 0.0);
        assert!(d.fold);
        // Best within window (t<=5): t=3 (hp=0.7) vs t=5 anchor (hp=0.6)
        assert_eq!(d.index, Some(1)); // index 1 (t=3, hp=0.7)
    }

    // ── Snapshots with t > 30 ignored ──

    #[test]
    fn snapshots_beyond_30s_ignored() {
        let snaps = [
            snap(1.0, 0.2, 0.0, false),
            snap(31.0, 0.99, 0.99, false), // outside window
        ];
        let d = decide(&snaps, 0.9, 1.0, 1.0);
        // Only candidate is t=1 (current) → no better past → no fold
        assert!(!d.fold);
    }

    #[test]
    fn exactly_30s_is_included() {
        let snaps = [
            snap(1.0, 0.1, 0.0, false),
            snap(30.0, 0.9, 0.0, false), // exactly at boundary → included
        ];
        let d = decide(&snaps, 0.9, 1.0, 0.0);
        assert!(d.fold);
        assert_eq!(d.index, Some(1));
    }

    // ── Edge cases ──

    #[test]
    fn empty_snaps_no_fold() {
        let d = decide(&[], 1.0, 1.0, 1.0);
        assert!(!d.fold);
    }

    #[test]
    fn single_snapshot_is_current_no_fold() {
        // Single snapshot is the current state — nothing to fold to
        let snaps = [snap(1.0, 0.9, 0.9, false)];
        let d = decide(&snaps, 0.9, 1.0, 1.0);
        assert!(!d.fold);
    }

    #[test]
    fn single_snapshot_at_t0_no_fold() {
        let snaps = [snap(0.0, 1.0, 1.0, false)];
        let d = decide(&snaps, 0.9, 1.0, 1.0);
        assert!(!d.fold);
    }

    // ── Tie-breaking: more recent wins ──

    #[test]
    fn tie_broken_by_more_recent() {
        // Two past snapshots with identical favorability; more recent (smaller t) wins.
        let snaps = [
            snap(1.0, 0.1, 0.0, false),  // current
            snap(10.0, 0.8, 0.0, false), // fav = 0.8
            snap(20.0, 0.8, 0.0, false), // fav = 0.8, older
        ];
        let d = decide(&snaps, 0.9, 1.0, 0.0);
        assert!(d.fold);
        assert_eq!(d.index, Some(1)); // t=10 preferred over t=20
    }

    // ── Weights ──

    #[test]
    fn pos_weight_dominates_when_hp_equal() {
        let snaps = [
            snap(1.0, 0.5, 0.2, false),  // current:  fav = 0.5+0.2=0.7 at (1,1)
            snap(10.0, 0.5, 0.9, false), // fav = 0.5+0.9 = 1.4
            snap(15.0, 0.5, 0.1, false), // fav = 0.5+0.1 = 0.6
        ];
        let d = decide(&snaps, 0.9, 1.0, 1.0);
        assert!(d.fold);
        assert_eq!(d.index, Some(1));
    }

    #[test]
    fn zero_weights_no_favorability_difference() {
        // All favorabilities are 0.0 → no snapshot strictly better
        let snaps = [
            snap(1.0, 0.9, 0.9, false),
            snap(10.0, 0.1, 0.1, false),
        ];
        let d = decide(&snaps, 0.9, 0.0, 0.0);
        assert!(!d.fold);
    }

    // ── Determinism ──

    #[test]
    fn deterministic() {
        let snaps = [
            snap(0.5, 0.3, 0.2, false),
            snap(5.0, 0.8, 0.7, false),
            snap(12.0, 0.6, 0.9, true),
            snap(25.0, 0.9, 0.5, false),
        ];
        let d1 = decide(&snaps, 0.7, 1.0, 0.5);
        let d2 = decide(&snaps, 0.7, 1.0, 0.5);
        assert_eq!(d1, d2);
    }

    // ── No panic on degenerate float inputs ──

    #[test]
    fn no_panic_on_nan_inf_in_snapshots() {
        let snaps = [
            snap(f32::NAN, 0.5, 0.5, false),
            snap(1.0, f32::INFINITY, 0.0, false),
            snap(5.0, 0.8, f32::NEG_INFINITY, false),
        ];
        // Should not panic regardless of outcome
        let _ = decide(&snaps, 0.9, 1.0, 1.0);
    }
}
