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
pub fn schedule(cands: &[(u32, f32)], budget: u32) -> Vec<u32> {
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
            std::cmp::Ordering::Equal // stable ⇒ original order on ties
        }
    });
    idx.into_iter()
        .take(budget as usize)
        .map(|i| cands[i].0)
        .collect()
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
}
