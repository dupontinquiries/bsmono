//! `sim::visible_delta` — per-frame visible-set diff for dirty staging.
//!
//! The renderer re-stages per-chunk metadata for every visible chunk each frame,
//! so `record`/`draw` CPU cost is linear in the visible-chunk count. If it knows
//! only which chunks *entered* or *left* the visible set since last frame, it can
//! stage just those. This module computes that diff — pure set logic; the
//! coordinator wires it into the render path.
//!
//! [`diff_visible`] is a single linear merge-walk over the two sorted key lists:
//! `O(prev + curr)` time and no allocation beyond the two output vecs. Pure,
//! deterministic, no IO/clock/RNG (R1/R6); standalone `rustc --test`-able.

use std::cmp::Ordering;

/// The change in the visible set between two frames.
///
/// `added` and `removed` are ascending-sorted and duplicate-free (they inherit
/// the ordering of the inputs); `retained` is how many keys were present in both.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct VisibleDelta {
    /// Keys in `curr` but not `prev` (entered the visible set), ascending.
    pub added: Vec<i64>,
    /// Keys in `prev` but not `curr` (left the visible set), ascending.
    pub removed: Vec<i64>,
    /// Count of keys present in both frames.
    pub retained: usize,
}

// Defined unconditionally: it is referenced by the `debug_assert!`s in
// `diff_visible`, whose expansion is compiled (as `if false { … }`) even when
// debug-assertions are off, so gating this on `cfg(debug_assertions)` breaks
// release builds. `#[allow(dead_code)]` covers the release case where the call
// is never actually taken.
#[allow(dead_code)]
fn is_sorted_unique(s: &[i64]) -> bool {
    s.windows(2).all(|w| w[0] < w[1])
}

/// Diff two visible-set snapshots of packed chunk keys.
///
/// # Preconditions
/// Both `prev` and `curr` must be **ascending-sorted and duplicate-free**. This
/// is the natural shape of a packed-coord visible set kept in a sorted vec /
/// `BTreeSet`; it is `debug_assert`ed in debug builds and assumed in release. If
/// violated, the result is unspecified (but still panic-free).
///
/// Returns the `added` / `removed` keys (each ascending) and the `retained`
/// count, in one `O(prev + curr)` merge-walk.
pub fn diff_visible(prev: &[i64], curr: &[i64]) -> VisibleDelta {
    debug_assert!(is_sorted_unique(prev), "prev must be ascending + dup-free");
    debug_assert!(is_sorted_unique(curr), "curr must be ascending + dup-free");

    let mut added: Vec<i64> = Vec::new();
    let mut removed: Vec<i64> = Vec::new();
    let mut retained: usize = 0;

    let (mut i, mut j) = (0usize, 0usize);
    while i < prev.len() && j < curr.len() {
        match prev[i].cmp(&curr[j]) {
            Ordering::Less => {
                // prev key not in curr → it left the set
                removed.push(prev[i]);
                i += 1;
            }
            Ordering::Greater => {
                // curr key not in prev → it entered the set
                added.push(curr[j]);
                j += 1;
            }
            Ordering::Equal => {
                retained += 1;
                i += 1;
                j += 1;
            }
        }
    }
    // tails: leftover prev all removed, leftover curr all added
    while i < prev.len() {
        removed.push(prev[i]);
        i += 1;
    }
    while j < curr.len() {
        added.push(curr[j]);
        j += 1;
    }

    VisibleDelta { added, removed, retained }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ascending_unique(v: &[i64]) -> bool {
        v.windows(2).all(|w| w[0] < w[1])
    }

    #[test]
    fn identical_sets() {
        let s = [1i64, 4, 9, 16, 25];
        let d = diff_visible(&s, &s);
        assert!(d.added.is_empty() && d.removed.is_empty());
        assert_eq!(d.retained, s.len());
    }

    #[test]
    fn disjoint_sets() {
        let prev = [1i64, 3, 5];
        let curr = [2i64, 4, 6];
        let d = diff_visible(&prev, &curr);
        assert_eq!(d.added, curr.to_vec());
        assert_eq!(d.removed, prev.to_vec());
        assert_eq!(d.retained, 0);
    }

    #[test]
    fn subset_and_superset() {
        // curr is a superset of prev → only additions
        let prev = [2i64, 5, 8];
        let curr = [1i64, 2, 5, 8, 13];
        let d = diff_visible(&prev, &curr);
        assert_eq!(d.added, vec![1, 13]);
        assert!(d.removed.is_empty());
        assert_eq!(d.retained, 3);
        // and the reverse: prev superset of curr → only removals
        let d2 = diff_visible(&curr, &prev);
        assert_eq!(d2.removed, vec![1, 13]);
        assert!(d2.added.is_empty());
        assert_eq!(d2.retained, 3);
    }

    #[test]
    fn empty_prev_all_added_empty_curr_all_removed() {
        let s = [10i64, 20, 30];
        let a = diff_visible(&[], &s);
        assert_eq!(a.added, s.to_vec());
        assert!(a.removed.is_empty() && a.retained == 0);
        let r = diff_visible(&s, &[]);
        assert_eq!(r.removed, s.to_vec());
        assert!(r.added.is_empty() && r.retained == 0);
        // both empty
        let e = diff_visible(&[], &[]);
        assert!(e.added.is_empty() && e.removed.is_empty() && e.retained == 0);
    }

    #[test]
    fn outputs_ascending_and_dup_free() {
        let prev = [-5i64, 0, 3, 7, 100];
        let curr = [-5i64, 1, 3, 50, 100, 200];
        let d = diff_visible(&prev, &curr);
        assert!(ascending_unique(&d.added), "added not ascending/unique: {:?}", d.added);
        assert!(ascending_unique(&d.removed), "removed not ascending/unique: {:?}", d.removed);
    }

    #[test]
    fn invariants_over_random_sorted_inputs() {
        let mut s = 0x33FA_u64; // documented seed (R20)
        let mut rng = || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (s >> 33) as i64
        };
        // build a sorted, dup-free vec by sampling a value range then sort+dedup
        let sorted_unique = |n: usize, rng: &mut dyn FnMut() -> i64| -> Vec<i64> {
            let mut v: Vec<i64> = (0..n).map(|_| rng() % 200).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        for _ in 0..2000 {
            let prev = sorted_unique((rng().unsigned_abs() % 40) as usize, &mut rng);
            let curr = sorted_unique((rng().unsigned_abs() % 40) as usize, &mut rng);
            let d = diff_visible(&prev, &curr);
            // counts partition both inputs
            assert_eq!(d.added.len() + d.retained, curr.len(), "curr partition");
            assert_eq!(d.removed.len() + d.retained, prev.len(), "prev partition");
            // ordering preserved
            assert!(ascending_unique(&d.added) && ascending_unique(&d.removed));
            // added ⊆ curr and disjoint from prev; removed ⊆ prev and disjoint from curr
            for x in &d.added {
                assert!(curr.contains(x) && !prev.contains(x));
            }
            for x in &d.removed {
                assert!(prev.contains(x) && !curr.contains(x));
            }
        }
    }

    #[test]
    fn deterministic() {
        let prev = [1i64, 2, 9, 40];
        let curr = [2i64, 3, 9, 41];
        assert_eq!(diff_visible(&prev, &curr), diff_visible(&prev, &curr));
    }
}
