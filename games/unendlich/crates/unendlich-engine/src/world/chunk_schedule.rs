//! `chunk_schedule` — budget-aware chunk selection.
//!
//! Worker-03 / WORLD pure core, round 22 (GPU_COMPUTE_PLAN #5). Given candidate
//! chunks with priorities (from `chunk_priority`, R11) and a per-frame budget,
//! returns the highest-priority subset to process this frame, in priority order.
//! Stable on ties so a secondary ordering the caller pre-applied survives.
//! Pairs with `prefetch_predict` / `gen_estimate`.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Select up to `budget` chunk ids from `(id, priority)` candidates, highest
/// priority first; ties keep input order. `budget == 0` or empty → empty.
///
/// Runs in `O(n log budget)` via a bounded top-`budget` heap rather than sorting
/// all `n` candidates (`O(n log n)`) — a win on the common per-frame case where
/// there are many candidate chunks but only a small budget. Output is identical to
/// a full stable sort (priority desc, ties by input order); the `deterministic`
/// and `matches_full_sort` tests pin that equivalence.
pub fn schedule(cands: &[(u32, f32)], budget: u32) -> Vec<u32> {
    if budget == 0 || cands.is_empty() {
        return Vec::new();
    }
    let b = (budget as usize).min(cands.len());

    // Selecting (almost) everything: a single stable sort is already optimal.
    if b == cands.len() {
        let mut idx: Vec<usize> = (0..cands.len()).collect();
        idx.sort_by(|&a, &c| rank_cmp(cands, a, c));
        return idx.into_iter().map(|i| cands[i].0).collect();
    }

    // Bounded selection: keep the `b` best indices in a max-heap keyed on "worst
    // kept" (Worst), so the root is the weakest of the current keep-set and can be
    // evicted in O(log b) when a stronger candidate arrives.
    use std::collections::BinaryHeap;
    let mut keep: BinaryHeap<Worst> = BinaryHeap::with_capacity(b + 1);
    for i in 0..cands.len() {
        let cand = Worst { pri: cands[i].1, idx: i };
        if keep.len() < b {
            keep.push(cand);
        } else if let Some(worst) = keep.peek() {
            // Replace the worst kept if this candidate ranks strictly before it.
            if ranks_before(cand.pri, cand.idx, worst.pri, worst.idx) {
                keep.pop();
                keep.push(cand);
            }
        }
    }

    // Emit in final priority order (priority desc, ties by input index).
    let mut chosen: Vec<Worst> = keep.into_vec();
    chosen.sort_by(|a, c| {
        if ranks_before(a.pri, a.idx, c.pri, c.idx) {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });
    chosen.into_iter().map(|w| cands[w.idx].0).collect()
}

/// Does candidate `(pa, ia)` rank strictly before `(pb, ib)`? Higher priority
/// first; ties (including any NaN, which compares neither `>` nor `<`) fall back
/// to lower input index — matching the original stable-sort semantics exactly.
#[inline]
fn ranks_before(pa: f32, ia: usize, pb: f32, ib: usize) -> bool {
    if pa > pb {
        true
    } else if pa < pb {
        false
    } else {
        ia < ib
    }
}

#[inline]
fn rank_cmp(cands: &[(u32, f32)], a: usize, c: usize) -> std::cmp::Ordering {
    if ranks_before(cands[a].1, a, cands[c].1, c) {
        std::cmp::Ordering::Less
    } else {
        std::cmp::Ordering::Greater
    }
}

/// Heap element ordered so the *worst* keep-candidate is greatest (the max-heap
/// root), i.e. reverse of `ranks_before`. Index ties are impossible (unique).
#[derive(Clone, Copy)]
struct Worst {
    pri: f32,
    idx: usize,
}
impl PartialEq for Worst {
    fn eq(&self, o: &Self) -> bool {
        self.idx == o.idx
    }
}
impl Eq for Worst {}
impl Ord for Worst {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        if self.idx == o.idx {
            std::cmp::Ordering::Equal
        } else if ranks_before(o.pri, o.idx, self.pri, self.idx) {
            // `o` ranks before `self` ⇒ `self` is worse ⇒ greater (heap root).
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Less
        }
    }
}
impl PartialOrd for Worst {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_highest_priority() {
        let c = [(10u32, 0.2f32), (11, 0.9), (12, 0.5)];
        assert_eq!(schedule(&c, 2), vec![11, 12]);
    }

    #[test]
    fn respects_budget() {
        let c = [(1u32, 0.1f32), (2, 0.2), (3, 0.3), (4, 0.4)];
        assert_eq!(schedule(&c, 2).len(), 2);
        assert_eq!(schedule(&c, 10).len(), 4); // budget exceeds candidates
    }

    #[test]
    fn empty_is_empty() {
        assert!(schedule(&[], 5).is_empty());
    }

    #[test]
    fn stable_on_ties() {
        // Equal priorities → original input order preserved.
        let c = [(7u32, 0.5f32), (3, 0.5), (9, 0.5), (1, 0.9)];
        assert_eq!(schedule(&c, 4), vec![1, 7, 3, 9]);
    }

    #[test]
    fn budget_zero_is_empty() {
        let c = [(1u32, 0.9f32)];
        assert!(schedule(&c, 0).is_empty());
    }

    #[test]
    fn deterministic() {
        let c = [(1u32, 0.3f32), (2, 0.3), (3, 0.7)];
        assert_eq!(schedule(&c, 2), schedule(&c, 2));
    }

    /// Reference: the original full stable-sort implementation.
    fn schedule_ref(cands: &[(u32, f32)], budget: u32) -> Vec<u32> {
        if budget == 0 || cands.is_empty() {
            return Vec::new();
        }
        let mut idx: Vec<usize> = (0..cands.len()).collect();
        idx.sort_by(|&a, &b| {
            let (pa, pb) = (cands[a].1, cands[b].1);
            if pa > pb {
                std::cmp::Ordering::Less
            } else if pa < pb {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
        idx.into_iter().take(budget as usize).map(|i| cands[i].0).collect()
    }

    #[test]
    fn matches_full_sort_on_random_inputs() {
        // The bounded top-k must be byte-identical to the full stable sort for
        // every budget, including heavy tie clusters (few distinct priorities).
        let mut s: u32 = 0x1234_5678;
        let mut rng = || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s
        };
        for _ in 0..2000 {
            let n = (rng() % 40) as usize;
            let distinct = 1 + rng() % 5; // few priority levels ⇒ many ties
            let cands: Vec<(u32, f32)> = (0..n)
                .map(|i| (i as u32 + 100, (rng() % distinct) as f32 / distinct as f32))
                .collect();
            for budget in 0..(n as u32 + 3) {
                assert_eq!(
                    schedule(&cands, budget),
                    schedule_ref(&cands, budget),
                    "mismatch n={n} budget={budget}"
                );
            }
        }
    }
}
