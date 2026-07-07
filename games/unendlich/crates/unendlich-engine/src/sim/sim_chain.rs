//! `sim::sim_chain` — assemble a deep-time jump as a CHAIN of sim vectors and
//! collapse its quiescent stretches by convergence merge.
//!
//! Background: `docs/design/vector_chaining_and_timewarp.md`. A single
//! [`SimVector`] can't honestly span more than `min(fidelity_len,
//! VECTOR_MAX_SPAN)`, so a deep jump (e.g. 1,000,000 years) is a chain of vectors,
//! each re-anchored to a committed checkpoint. The chain is cheap because most
//! deep time reaches a steady regime: **consecutive links whose end-checkpoints
//! land in the same coarse bucket (same [`region_digest`]) MERGE** — "nothing
//! distinct happened for 900,000 years" collapses to one repeated edge. So the
//! cost is proportional to the number of **distinct** (un-merged) vectors, not the
//! raw span. The chain's per-link checkpoints double as the scrub animation's
//! keyframes and the benchmark's assert points.
//!
//! This module is the R2 *structural* planner: it lays out the chain (link spans,
//! per-link vectors) from the start state, and provides the merge collapse over
//! the per-link end-digests the R1 fill produces. Pure & deterministic: no
//! clock/RNG/IO/panic.

use crate::sim::convergence::ConvergenceClass;
use crate::sim::sim_scheme::chain_len;
use crate::sim::sim_vector::SimVector;

/// One maximal run of consecutive links that converged to the same coarse state
/// (same end-digest) — the merged unit. A run of `len > 1` means "this steady
/// state held across `len` links"; the chain pays for it once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MergeRun {
    /// The coarse end-digest shared by every link in the run.
    pub digest: u64,
    /// Index of the first link in the run (into the chain).
    pub first_link: usize,
    /// How many consecutive links the run spans.
    pub len: usize,
}

/// The (link_start_tick, link_span) segments of a chain covering `total_span`
/// ticks from `start_tick`, each segment ≤ `vector_max_span`. Segments are
/// contiguous and the last ends exactly at `start_tick + total_span`. A `0` cap
/// or `total_span == 0` yields a single segment.
pub fn chain_spans(start_tick: u64, total_span: u64, vector_max_span: u64) -> Vec<(u64, u64)> {
    if total_span == 0 {
        return vec![(start_tick, 0)];
    }
    let n = chain_len(total_span, vector_max_span).max(1);
    let mut out = Vec::with_capacity(n as usize);
    let mut covered = 0u64;
    let mut t = start_tick;
    for k in 1..=n {
        // distribute total_span as evenly as possible; the last link absorbs the
        // remainder so the chain ends exactly at start_tick + total_span.
        let target_covered = (total_span as u128 * k as u128 / n as u128) as u64;
        let seg = target_covered - covered;
        out.push((t, seg));
        t += seg;
        covered = target_covered;
    }
    out
}

/// Lay out the chain: one [`SimVector`] per segment (see [`chain_spans`]), each
/// planned from the start state. This is the R2 structural plan — at plan time
/// only the start state is known, so every link is seeded from it plus its
/// segment; the R1 fill re-anchors each link's seed/state from the actual
/// computed checkpoint as it advances. Useful now for the chain *structure*
/// (count, contiguity, span coverage); the fill supplies real per-link digests.
pub fn plan_chain(
    world_seed: u64,
    region: u32,
    start_tick: u64,
    total_span: u64,
    quantized_start_state: &[i64],
    classes: &[ConvergenceClass],
    vector_max_span: u64,
) -> Vec<SimVector> {
    chain_spans(start_tick, total_span, vector_max_span)
        .into_iter()
        .map(|(seg_start, seg_span)| {
            SimVector::plan(
                world_seed,
                region,
                seg_start,
                seg_span,
                quantized_start_state,
                classes,
            )
        })
        .collect()
}

/// Collapse the chain by convergence merge: group consecutive links whose
/// end-digests are equal into [`MergeRun`]s. `end_digests[i]` is the coarse
/// digest at the end of link `i` (computed by the fill). Returns the runs in
/// order; `runs.len()` is the number of **distinct** vectors — the chain's true
/// cost (`vector_chaining_and_timewarp.md` §2).
pub fn merge_runs(end_digests: &[u64]) -> Vec<MergeRun> {
    let mut runs: Vec<MergeRun> = Vec::new();
    for (i, &d) in end_digests.iter().enumerate() {
        match runs.last_mut() {
            Some(r) if r.digest == d => r.len += 1,
            _ => runs.push(MergeRun {
                digest: d,
                first_link: i,
                len: 1,
            }),
        }
    }
    runs
}

/// The number of distinct (un-merged) vectors in a chain — the cost metric.
/// Quiescent deep time collapses toward 1; an active era keeps its links.
pub fn distinct_count(end_digests: &[u64]) -> usize {
    merge_runs(end_digests).len()
}

/// The compression a merge achieves: `1 - distinct/total` in `[0,1)`. 0 means
/// nothing merged (every link distinct); near 1 means a long quiescent chain
/// collapsed to a few states. Empty chain ⇒ 0.
pub fn merge_compression(end_digests: &[u64]) -> f32 {
    let total = end_digests.len();
    if total == 0 {
        return 0.0;
    }
    1.0 - (distinct_count(end_digests) as f32 / total as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: ConvergenceClass = ConvergenceClass::PathDependent;
    fn conv(h: u64) -> ConvergenceClass {
        ConvergenceClass::Convergent { horizon_ticks: h }
    }

    #[test]
    fn chain_spans_are_contiguous_and_cover_exactly() {
        let segs = chain_spans(1000, 5000, 1000);
        assert_eq!(segs.len(), 5);
        // contiguous: each starts where the previous ended
        let mut t = 1000;
        let mut total = 0;
        for (start, span) in &segs {
            assert_eq!(*start, t, "segments contiguous");
            t += span;
            total += span;
            assert!(*span <= 1000, "no segment exceeds the cap");
        }
        assert_eq!(total, 5000, "segments cover the whole span exactly");
        assert_eq!(t, 6000, "chain ends at start+total_span");
    }

    #[test]
    fn uneven_span_remainder_absorbed() {
        // 5001 over cap 1000 → 6 links, total still exact.
        let segs = chain_spans(0, 5001, 1000);
        assert_eq!(segs.len(), 6);
        assert_eq!(segs.iter().map(|(_, s)| s).sum::<u64>(), 5001);
    }

    #[test]
    fn zero_span_is_single_empty_segment() {
        assert_eq!(chain_spans(42, 0, 1000), vec![(42, 0)]);
    }

    #[test]
    fn no_cap_is_single_segment() {
        let segs = chain_spans(0, 9999, 0);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0], (0, 9999));
    }

    #[test]
    fn plan_chain_links_are_contiguous_and_planned() {
        let chain = plan_chain(0xBEEF, 7, 100, 3000, &[3, 5], &[PATH, conv(50)], 1000);
        assert_eq!(chain.len(), 3);
        // contiguous: each link starts at the previous link's end
        for w in chain.windows(2) {
            assert_eq!(w[1].start_tick, w[0].end_tick(), "links contiguous");
        }
        assert_eq!(chain.last().unwrap().end_tick(), 3100, "chain ends at target");
        // each link has the path-dependent waypoint density (1 + 1*4 = 5)
        assert!(chain.iter().all(|v| v.waypoints.len() == 5));
    }

    #[test]
    fn merge_collapses_quiescent_runs() {
        // a chain that reaches steady state: A,A,A then a change B,B then C.
        let digests = [10, 10, 10, 20, 20, 30];
        let runs = merge_runs(&digests);
        assert_eq!(runs.len(), 3, "3 distinct states");
        assert_eq!(runs[0], MergeRun { digest: 10, first_link: 0, len: 3 });
        assert_eq!(runs[1], MergeRun { digest: 20, first_link: 3, len: 2 });
        assert_eq!(runs[2], MergeRun { digest: 30, first_link: 5, len: 1 });
        assert_eq!(distinct_count(&digests), 3);
    }

    #[test]
    fn fully_quiescent_chain_collapses_to_one() {
        // 1000 links all the same steady state → cost 1.
        let digests = vec![7u64; 1000];
        assert_eq!(distinct_count(&digests), 1);
        assert!((merge_compression(&digests) - 0.999).abs() < 1e-3);
    }

    #[test]
    fn fully_active_chain_does_not_merge() {
        let digests = [1, 2, 3, 4, 5];
        assert_eq!(distinct_count(&digests), 5);
        assert_eq!(merge_compression(&digests), 0.0);
    }

    #[test]
    fn empty_chain_metrics() {
        assert_eq!(distinct_count(&[]), 0);
        assert_eq!(merge_compression(&[]), 0.0);
    }

    #[test]
    fn determinism() {
        let a = plan_chain(1, 0, 0, 4000, &[2], &[PATH], 1000);
        let b = plan_chain(1, 0, 0, 4000, &[2], &[PATH], 1000);
        assert_eq!(a, b);
    }
}
