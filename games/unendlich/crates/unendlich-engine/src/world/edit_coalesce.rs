//! `edit_coalesce` — merge edit dirty-AABBs into a minimal remesh set.
//!
//! Worker-03 / WORLD pure core, round 13. A burst of edits produces many small
//! dirty boxes; this unions overlapping or touching boxes into the fewest
//! bounding boxes that still cover them, so the remesher runs once per region
//! instead of once per edit. Boxes separated by an empty cell stay distinct.
//! Pairs with `block_edit` (which emits the per-edit AABBs).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

type Aabb = ([i32; 3], [i32; 3]);

/// Coalesce inclusive AABBs, merging any that overlap or touch (gap 0) into
/// their bounding union, to a fixpoint. Output is sorted, so it's independent of
/// input order. Empty input → empty output.
pub fn coalesce(boxes: &[Aabb]) -> Vec<Aabb> {
    let mut active: Vec<Aabb> = boxes.iter().map(|b| normalize(*b)).collect();

    let mut merged = true;
    while merged {
        merged = false;
        let mut i = 0;
        'scan: while i < active.len() {
            for j in (i + 1)..active.len() {
                if mergeable(active[i], active[j]) {
                    active[i] = union(active[i], active[j]);
                    active.remove(j);
                    merged = true;
                    break 'scan;
                }
            }
            i += 1;
        }
    }

    // Canonical order ⇒ result independent of input order.
    active.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    active
}

fn normalize(b: Aabb) -> Aabb {
    let (lo, hi) = b;
    (
        [lo[0].min(hi[0]), lo[1].min(hi[1]), lo[2].min(hi[2])],
        [lo[0].max(hi[0]), lo[1].max(hi[1]), lo[2].max(hi[2])],
    )
}

/// Overlap or touch (no empty cell between) on every axis.
fn mergeable(a: Aabb, b: Aabb) -> bool {
    (0..3).all(|k| b.0[k] <= a.1[k] + 1 && a.0[k] <= b.1[k] + 1)
}

fn union(a: Aabb, b: Aabb) -> Aabb {
    (
        [a.0[0].min(b.0[0]), a.0[1].min(b.0[1]), a.0[2].min(b.0[2])],
        [a.1[0].max(b.1[0]), a.1[1].max(b.1[1]), a.1[2].max(b.1[2])],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_empty() {
        assert!(coalesce(&[]).is_empty());
    }

    #[test]
    fn single_is_itself_normalized() {
        let b = ([2, 2, 2], [0, 1, 0]); // reversed corners
        let out = coalesce(&[b]);
        assert_eq!(out, vec![([0, 1, 0], [2, 2, 2])]);
    }

    #[test]
    fn disjoint_stay_separate() {
        // gap of an empty cell on x (max 1, next min 3 → cell 2 empty)
        let a = ([0, 0, 0], [1, 1, 1]);
        let b = ([3, 0, 0], [4, 1, 1]);
        assert_eq!(coalesce(&[a, b]).len(), 2);
    }

    #[test]
    fn overlapping_merge() {
        let a = ([0, 0, 0], [3, 3, 3]);
        let b = ([2, 2, 2], [5, 5, 5]);
        let out = coalesce(&[a, b]);
        assert_eq!(out, vec![([0, 0, 0], [5, 5, 5])]);
    }

    #[test]
    fn adjacent_merge() {
        // touching on x (a.max.x=1, b.min.x=2 → no empty cell between)
        let a = ([0, 0, 0], [1, 1, 1]);
        let b = ([2, 0, 0], [3, 1, 1]);
        let out = coalesce(&[a, b]);
        assert_eq!(out, vec![([0, 0, 0], [3, 1, 1])]);
    }

    #[test]
    fn order_independent_and_deterministic() {
        let a = ([0, 0, 0], [1, 1, 1]);
        let b = ([2, 0, 0], [3, 1, 1]); // touches a
        let c = ([10, 10, 10], [11, 11, 11]); // separate
        let fwd = coalesce(&[a, b, c]);
        let rev = coalesce(&[c, b, a]);
        assert_eq!(fwd, rev);
        assert_eq!(fwd.len(), 2);
        assert_eq!(coalesce(&[a, b, c]), coalesce(&[a, b, c]));
    }

    #[test]
    fn chain_merges_transitively() {
        // Three boxes in a touching chain → one union.
        let a = ([0, 0, 0], [1, 1, 1]);
        let b = ([2, 0, 0], [3, 1, 1]);
        let c = ([4, 0, 0], [5, 1, 1]);
        let out = coalesce(&[a, b, c]);
        assert_eq!(out, vec![([0, 0, 0], [5, 1, 1])]);
    }
}
