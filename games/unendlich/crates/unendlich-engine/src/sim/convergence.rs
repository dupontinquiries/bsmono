//! `sim::convergence` — convergence classes + the FREE structural pre-filter for
//! deciding whether a channel (e.g. animal behaviour) can change a region's
//! deep-time outcome at all.
//!
//! Background (see `docs/design/deeptime_convergence_classes.md` §4.5 and
//! `timebending_set_algebra.md`): when fast-forwarding a region across deep time,
//! the cost should be proportional to what actually *diverges*. A channel only
//! needs simulating if it can influence the region's coarse outcome. The
//! definitive test is perturbation *bracketing* (run the channel's extremes as
//! parallel sim vectors, see if they converge) — but that costs 2×+ a sim, so you
//! run cheaper pre-filters first and only bracket when they're ambiguous.
//!
//! This module is the **cheapest pre-filter (test #1): structural reachability.**
//! A channel rooted in some region(s) can only affect another region if there's a
//! coupling path — above a coarse strength threshold, within the horizon — from
//! the channel's region to it. If its influence set reaches nothing observable,
//! the channel **provably cannot matter**: it's `Convergent` by graph analysis,
//! with zero simulation. We build this directly on the existing, tested
//! [`crate::world::rollback_scope::rollback_closure`] (BFS over
//! [`crate::world::region_graph::Edge`]).
//!
//! Pure & deterministic: graph in, verdict out. No clock/RNG/IO, no panics.

use crate::world::region_graph::Edge;
use crate::world::rollback_scope::rollback_closure;

/// How a channel's deep-time history is bounded (the datatype from §2 of the
/// convergence-classes doc). Carried per (channel, region-type) and cached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConvergenceClass {
    /// Class A — pure procedural function of (pos, seed, time); regenerable in
    /// O(1), so deep time is free (a jump just re-evaluates). No history.
    Procedural,
    /// Class B — has history but provably reaches a steady state by `horizon`
    /// ticks; a jump past the horizon snaps to the converged value. The cheap
    /// case the structural pre-filter and the bracket both aim to prove.
    Convergent { horizon_ticks: u64 },
    /// Class C — path-dependent: the trajectory matters and never fully settles
    /// (genome under moving selection, populations, learned habit). Must be
    /// simulated (statistically coarsened with depth), never collapsed.
    PathDependent,
}

impl ConvergenceClass {
    /// The fidelity horizon in ticks — how long this channel stays within
    /// tolerance of exact before a jump must re-anchor. Procedural never drifts
    /// (`u64::MAX` = unbounded); Convergent is bounded by its steady-state
    /// horizon; PathDependent has the shortest useful horizon (caller-supplied,
    /// since it depends on the system) — `0` means "no free horizon, always sim".
    pub fn fidelity_horizon(self) -> u64 {
        match self {
            ConvergenceClass::Procedural => u64::MAX,
            ConvergenceClass::Convergent { horizon_ticks } => horizon_ticks,
            ConvergenceClass::PathDependent => 0,
        }
    }

    /// Whether a deep-time jump may SKIP simulating this channel and use the
    /// cheap path (true for Procedural + Convergent — both have a bounded/zero
    /// influence on the coarse outcome past their horizon). PathDependent must
    /// be simulated.
    pub fn skippable_past_horizon(self) -> bool {
        !matches!(self, ConvergenceClass::PathDependent)
    }
}

/// The set of regions a channel rooted in `seed_regions` can influence: a
/// coupling-reachability closure above the coarse `min_coupling` strength, within
/// `max_hops` of the seeds. Thin wrapper over [`rollback_closure`] (same BFS) —
/// named for the deep-time use: "what can this channel's state reach?". The
/// result always includes the seeds themselves.
pub fn influence_set(seed_regions: &[u32], edges: &[Edge], min_coupling: i64, max_hops: u32) -> Vec<u32> {
    rollback_closure(seed_regions, edges, min_coupling, max_hops)
}

/// FREE structural pre-filter (test #1). `true` ⇒ the channel rooted in
/// `seed_regions` is **structurally irrelevant**: it reaches no region other than
/// its own seeds through couplings at/above `min_coupling` within `max_hops`, so
/// it provably cannot change any other region's coarse outcome. Such a channel is
/// `Convergent` by graph analysis with zero simulation.
///
/// "Reaches nothing else" = the influence set equals exactly the seed set. (A
/// single isolated region with no qualifying outgoing edge reaches only itself.)
pub fn is_structurally_irrelevant(seed_regions: &[u32], edges: &[Edge], min_coupling: i64, max_hops: u32) -> bool {
    if seed_regions.is_empty() {
        return true; // nothing to influence anything
    }
    let reached = influence_set(seed_regions, edges, min_coupling, max_hops);
    // sorted-unique seeds to compare against the (sorted-unique) closure
    let mut seeds: Vec<u32> = seed_regions.to_vec();
    seeds.sort_unstable();
    seeds.dedup();
    reached == seeds
}

/// Apply the structural pre-filter to assign a class without simulating. If the
/// channel is structurally irrelevant, it's `Convergent { horizon_ticks: 0 }`
/// (converged immediately — it never moves the coarse outcome). Otherwise the
/// structural test is INCONCLUSIVE (the channel *can* propagate, so it might be
/// Class C) and we return `None` — the caller falls through to the coarse
/// pre-bracket / sensitivity bound / full bracket, then caches the verdict.
pub fn classify_structural(
    seed_regions: &[u32],
    edges: &[Edge],
    min_coupling: i64,
    max_hops: u32,
) -> Option<ConvergenceClass> {
    if is_structurally_irrelevant(seed_regions, edges, min_coupling, max_hops) {
        Some(ConvergenceClass::Convergent { horizon_ticks: 0 })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(a: u32, b: u32, w: i64) -> Edge {
        Edge { a, b, weight: w }
    }

    #[test]
    fn isolated_region_is_irrelevant() {
        // region 5 with no edges reaches only itself → structurally irrelevant.
        assert!(is_structurally_irrelevant(&[5], &[], 1, 8));
        assert_eq!(
            classify_structural(&[5], &[], 1, 8),
            Some(ConvergenceClass::Convergent { horizon_ticks: 0 })
        );
    }

    #[test]
    fn weak_coupling_below_threshold_is_irrelevant() {
        // 0—1 exists but only at weight 3; with min_coupling 10 it's pruned, so
        // the channel in region 0 reaches nobody at the coarse scale.
        let edges = [e(0, 1, 3)];
        assert!(is_structurally_irrelevant(&[0], &edges, 10, 8));
    }

    #[test]
    fn strong_coupling_makes_it_relevant() {
        // 0—1 at weight 50 ≥ threshold 10 → region 0 reaches region 1 → the
        // channel CAN propagate → structural test inconclusive (might be Class C).
        let edges = [e(0, 1, 50)];
        assert!(!is_structurally_irrelevant(&[0], &edges, 10, 8));
        assert_eq!(classify_structural(&[0], &edges, 10, 8), None);
    }

    #[test]
    fn horizon_bounds_reach() {
        // chain 0—1—2—3 strongly coupled; from 0 with max_hops 1 it reaches {0,1}
        // (relevant). The point: reach is horizon-bounded, like the jump span.
        let edges = [e(0, 1, 50), e(1, 2, 50), e(2, 3, 50)];
        // hop 0 only: 0 reaches just itself within 0 hops? rollback_closure stops
        // expanding AT max_hops, so max_hops=1 lets one expansion: {0,1}.
        assert!(!is_structurally_irrelevant(&[0], &edges, 10, 1));
        let reached = influence_set(&[0], &edges, 10, 1);
        assert!(reached.contains(&0) && reached.contains(&1));
        assert!(!reached.contains(&3), "3 is 3 hops away, beyond max_hops=1");
    }

    #[test]
    fn empty_seeds_trivially_irrelevant() {
        assert!(is_structurally_irrelevant(&[], &[e(0, 1, 50)], 1, 8));
    }

    #[test]
    fn class_horizon_and_skippable() {
        assert_eq!(ConvergenceClass::Procedural.fidelity_horizon(), u64::MAX);
        assert_eq!(
            ConvergenceClass::Convergent { horizon_ticks: 100 }.fidelity_horizon(),
            100
        );
        assert_eq!(ConvergenceClass::PathDependent.fidelity_horizon(), 0);
        assert!(ConvergenceClass::Procedural.skippable_past_horizon());
        assert!(ConvergenceClass::Convergent { horizon_ticks: 50 }.skippable_past_horizon());
        assert!(!ConvergenceClass::PathDependent.skippable_past_horizon());
    }

    #[test]
    fn influence_set_includes_seeds() {
        // even with no edges, the closure contains the seeds themselves.
        let r = influence_set(&[7, 2], &[], 1, 8);
        assert!(r.contains(&7) && r.contains(&2));
    }

    #[test]
    fn determinism_same_inputs_same_verdict() {
        let edges = [e(0, 1, 50), e(1, 2, 5), e(3, 4, 99)];
        let a = classify_structural(&[0], &edges, 10, 4);
        let b = classify_structural(&[0], &edges, 10, 4);
        assert_eq!(a, b);
    }
}
