//! Dependency-scoped rollback — the deterministic way to "bump rollback": when a
//! correction arrives (netcode reconciliation, a speculative misprediction, a
//! late input), you don't have to re-simulate the whole world forward. Only the
//! regions **coupled** to the changed ones — transitively, in the region graph —
//! can have diverged. Everything else is provably identical to its checkpoint and
//! is reused as-is.
//!
//! So rollback cost scales with the *blast radius* of the change, not the world
//! size. No accuracy lost, no model needed: this is exact. The closure can be
//! tightened two ways, which is also where the "prune the bad / fast-forward the
//! rest" idea lives honestly — stop propagating across couplings weaker than
//! `min_coupling` (weakly-coupled regions are deferred or approximated), and cap
//! the `max_hops` blast radius. Regions left *outside* the closure are exactly the
//! ones a cheap predictor can fast-forward, because by construction the change
//! barely reaches them.
//!
//! Pure, deterministic. Built on [`region_graph::Edge`](super::region_graph::Edge).

use crate::world::region_graph::Edge;
use std::collections::{HashMap, HashSet, VecDeque};

/// The set of regions that must be exactly re-simulated after `changed` regions
/// took a correction. BFS over couplings with weight `>= min_coupling`, bounded
/// to `max_hops` from any seed. Returns the set (including the seeds), sorted.
///
/// Everything NOT returned reuses its checkpoint (or is fast-forwarded cheaply).
pub fn rollback_closure(changed: &[u32], edges: &[Edge], min_coupling: i64, max_hops: u32) -> Vec<u32> {
    let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
    for e in edges {
        if e.weight >= min_coupling && e.a != e.b {
            adj.entry(e.a).or_default().push(e.b);
            adj.entry(e.b).or_default().push(e.a);
        }
    }
    let mut seen: HashSet<u32> = changed.iter().copied().collect();
    let mut q: VecDeque<(u32, u32)> = changed.iter().map(|&c| (c, 0u32)).collect();
    while let Some((node, hop)) = q.pop_front() {
        if hop >= max_hops {
            continue;
        }
        if let Some(neighbors) = adj.get(&node) {
            for &nb in neighbors {
                if seen.insert(nb) {
                    q.push_back((nb, hop + 1));
                }
            }
        }
    }
    let mut out: Vec<u32> = seen.into_iter().collect();
    out.sort_unstable();
    out
}

/// Fraction of `total` regions that must be re-simulated — the rollback cost
/// after scoping, in `[0,1]`. The smaller, the cheaper the rollback.
pub fn rollback_fraction(closure_len: usize, total: usize) -> f32 {
    if total == 0 {
        0.0
    } else {
        (closure_len as f32 / total as f32).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::region_graph::Edge;

    fn e(a: u32, b: u32, w: i64) -> Edge {
        Edge { a, b, weight: w }
    }

    #[test]
    fn closure_follows_strong_couplings() {
        // 0-1-2-3 chain, all strongly coupled.
        let edges = [e(0, 1, 10), e(1, 2, 10), e(2, 3, 10)];
        let c = rollback_closure(&[0], &edges, 1, 99);
        assert_eq!(c, vec![0, 1, 2, 3], "a change at 0 can reach the whole chain");
    }

    #[test]
    fn max_hops_bounds_the_blast_radius() {
        let edges = [e(0, 1, 10), e(1, 2, 10), e(2, 3, 10)];
        let c = rollback_closure(&[0], &edges, 1, 1);
        assert_eq!(c, vec![0, 1], "only one hop out → just the immediate neighbour");
    }

    #[test]
    fn weak_couplings_are_pruned_and_left_to_fast_forward() {
        // 0-1 strong, 1-2 weak. A change at 0 re-sims {0,1}; 2 is fast-forwarded.
        let edges = [e(0, 1, 10), e(1, 2, 1)];
        let c = rollback_closure(&[0], &edges, 5, 99);
        assert_eq!(c, vec![0, 1], "the weak edge to 2 isn't crossed");
        assert!(!c.contains(&2), "region 2 is outside the closure → predict it");
    }

    #[test]
    fn isolated_change_re_sims_only_itself() {
        let c = rollback_closure(&[5], &[], 1, 99);
        assert_eq!(c, vec![5]);
    }

    #[test]
    fn rollback_gets_cheaper_the_more_local_the_change() {
        // 100 regions, but the change only couples to a handful.
        let edges = [e(0, 1, 10), e(1, 2, 10)];
        let c = rollback_closure(&[0], &edges, 1, 99);
        let frac = rollback_fraction(c.len(), 100);
        assert!(frac < 0.05, "re-sim 3 of 100 → 3% rollback cost, not 100%: {frac}");
    }

    #[test]
    fn deterministic() {
        let edges = [e(0, 1, 5), e(1, 2, 5), e(0, 2, 5)];
        assert_eq!(rollback_closure(&[0], &edges, 1, 99), rollback_closure(&[0], &edges, 1, 99));
    }
}
