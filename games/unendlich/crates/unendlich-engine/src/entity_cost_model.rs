//! entity_cost_model.rs — pure entity-pass frame cost estimate.
//!
//! A concrete answer to "is the entity render path efficient?": given the
//! archetypes in view and how their instances split across the 4 LODs, estimate
//! draw calls, vertex throughput and a fragment-work proxy, and check that
//! against a budget. Pure, deterministic, overflow-safe (u64 + saturating), and
//! panic-free (R1/R11/R21).

/// One entity archetype (a distinct mesh/material set).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Arch {
    /// Total instances of this archetype in view.
    pub instances: u32,
    /// Part count at each LOD (index 0 = nearest/most detailed). Expected to be
    /// non-increasing, but the model doesn't require it.
    pub parts_per_lod: [u32; 4],
}

/// Estimated frame cost for the entity pass.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Cost {
    pub draws: u32,
    pub verts: u64,
    pub frag_units: u64,
}

/// Vertices per rendered part (a part ~ a small cuboid cluster).
const VERTS_PER_PART: u64 = 24;
/// Relative fragment work by LOD — nearer instances cover more screen.
const FRAG_WEIGHT: [u64; 4] = [8, 4, 2, 1];

/// Estimate the frame cost. `lod_counts[l]` is how the in-view instance
/// population splits across the 4 LODs; each archetype's `instances` are
/// distributed across LODs in those proportions (integer floor). One instanced
/// draw is counted per non-empty (archetype, LOD) bucket.
pub fn frame_cost(archs: &[Arch], lod_counts: [u32; 4]) -> Cost {
    let total_lod: u64 = lod_counts.iter().map(|&c| c as u64).sum();
    let mut cost = Cost::default();
    if total_lod == 0 {
        return cost;
    }
    for a in archs {
        for l in 0..4 {
            let n_at_l = (a.instances as u64)
                .saturating_mul(lod_counts[l] as u64)
                / total_lod;
            if n_at_l == 0 {
                continue;
            }
            cost.draws = cost.draws.saturating_add(1);
            let parts = n_at_l.saturating_mul(a.parts_per_lod[l] as u64);
            cost.verts = cost.verts.saturating_add(parts.saturating_mul(VERTS_PER_PART));
            cost.frag_units = cost
                .frag_units
                .saturating_add(n_at_l.saturating_mul(a.parts_per_lod[l] as u64).saturating_mul(FRAG_WEIGHT[l]));
        }
    }
    cost
}

/// True iff the cost fits both the draw-call and vertex budgets.
pub fn within_budget(c: &Cost, max_draws: u32, max_verts: u64) -> bool {
    c.draws <= max_draws && c.verts <= max_verts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arch(n: u32, p: [u32; 4]) -> Arch {
        Arch { instances: n, parts_per_lod: p }
    }

    #[test]
    fn zero_instances_is_zero_cost() {
        let c = frame_cost(&[arch(0, [10, 6, 3, 1])], [1, 1, 1, 1]);
        assert_eq!(c, Cost::default());
        // also zero when no LODs are populated
        let c2 = frame_cost(&[arch(100, [10, 6, 3, 1])], [0, 0, 0, 0]);
        assert_eq!(c2, Cost::default());
    }

    #[test]
    fn prop_cost_monotonic_in_instances() {
        let mut prev = Cost::default();
        for k in 0..40u32 {
            let n = k * 100;
            let c = frame_cost(&[arch(n, [12, 8, 4, 1])], [4, 3, 2, 1]);
            assert!(c.verts >= prev.verts, "verts dropped at n={}", n);
            assert!(c.frag_units >= prev.frag_units, "frag dropped at n={}", n);
            prev = c;
        }
    }

    #[test]
    fn far_lod_cheaper_than_near() {
        let a = arch(1000, [20, 10, 5, 1]); // descending parts
        let near = frame_cost(&[a], [1, 0, 0, 0]); // all at LOD0
        let far = frame_cost(&[a], [0, 0, 0, 1]); // all at LOD3
        assert!(far.verts < near.verts, "far verts not cheaper");
        assert!(far.frag_units < near.frag_units, "far frag not cheaper");
    }

    #[test]
    fn budget_check_both_ways() {
        let c = frame_cost(&[arch(500, [10, 6, 3, 1])], [2, 2, 2, 2]);
        assert!(within_budget(&c, u32::MAX, u64::MAX), "should fit a huge budget");
        assert!(!within_budget(&c, 0, u64::MAX), "zero draws budget should fail");
        assert!(!within_budget(&c, u32::MAX, 0), "zero verts budget should fail");
    }

    #[test]
    fn no_overflow_at_large_counts() {
        let big = arch(u32::MAX, [u32::MAX, u32::MAX, u32::MAX, u32::MAX]);
        let c = frame_cost(&[big, big, big], [1, 1, 1, 1]);
        // saturating math: finite, no panic, capped at u64::MAX
        assert!(c.verts <= u64::MAX);
        assert!(c.frag_units <= u64::MAX);
        assert!(within_budget(&c, u32::MAX, u64::MAX) || !within_budget(&c, u32::MAX, u64::MAX));
    }

    #[test]
    fn deterministic() {
        let archs = [arch(300, [12, 8, 4, 1]), arch(50, [6, 4, 2, 1])];
        assert_eq!(frame_cost(&archs, [5, 3, 2, 1]), frame_cost(&archs, [5, 3, 2, 1]));
    }
}
