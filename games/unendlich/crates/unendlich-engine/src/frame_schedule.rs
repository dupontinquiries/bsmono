//! Staged frame scheduling — grouping per-frame work into ordered *bulk* phases
//! so the engine processes all of one kind together (better locality, and the
//! setup for firing one GPU dispatch per phase rather than interleaving):
//!
//! 1. **Load** — bring chunks in: generate + mesh + establish physics.
//! 2. **Light** — the lighting pass (AO bake, block-light/skylight flood; see
//!    `light.rs`).
//! 3. **Path** — mob pathing/AI setup.
//!
//! The ordering reflects dependency (you can't light or path a chunk that isn't
//! loaded yet) AND priority (filling the world dominates while streaming). But a
//! pure priority drain would *starve* lighting and pathing during a load storm —
//! mobs would freeze and lights wouldn't update until the world finished
//! loading. So [`plan_frame`] first reserves a minimum slice for light and path,
//! then drains the remaining budget load-first. The result is a per-frame plan:
//! "process N loads, M lighting, K pathing", bounded by `budget`.
//!
//! Pure + deterministic; the renderer/bin executes each phase's batch.

/// Work waiting to be done, by phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PendingWork {
    pub load: usize,
    pub light: usize,
    pub path: usize,
}

/// How much of each phase to do this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PhasePlan {
    pub load: usize,
    pub light: usize,
    pub path: usize,
}

impl PhasePlan {
    pub fn total(self) -> usize {
        self.load + self.light + self.path
    }
}

/// Allocate a bounded `budget` of work units across the three phases.
///
/// `reserve_light` / `reserve_path` are minimum slices guaranteed to those
/// phases (capped by what's actually pending and by the budget) so they keep
/// making progress even while loading is saturated. After the reserves, the
/// remaining budget is drained in priority order: load first, then any extra
/// light, then any extra path.
pub fn plan_frame(
    pending: PendingWork,
    budget: usize,
    reserve_light: usize,
    reserve_path: usize,
) -> PhasePlan {
    // 1. Protect minimum slices for light then path (dependency/priority order
    //    means load isn't reserved — it's drained first from the remainder).
    let light_min = pending.light.min(reserve_light).min(budget);
    let mut rem = budget - light_min;
    let path_min = pending.path.min(reserve_path).min(rem);
    rem -= path_min;

    // 2. Drain the remainder: load first (unblocks future light/path), then top
    //    up light, then path.
    let load = pending.load.min(rem);
    rem -= load;
    let light_extra = (pending.light - light_min).min(rem);
    rem -= light_extra;
    let path_extra = (pending.path - path_min).min(rem);

    PhasePlan {
        load,
        light: light_min + light_extra,
        path: path_min + path_extra,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(load: usize, light: usize, path: usize) -> PendingWork {
        PendingWork { load, light, path }
    }

    #[test]
    fn everything_fits_under_a_large_budget() {
        let p = pending(10, 5, 3);
        let plan = plan_frame(p, 100, 4, 4);
        assert_eq!(plan, PhasePlan { load: 10, light: 5, path: 3 }, "all work done");
        assert_eq!(plan.total(), 18);
    }

    #[test]
    fn reserves_protect_light_and_path_during_a_load_storm() {
        // Huge load backlog, tiny budget — without reserves, load would eat it all.
        let p = pending(10_000, 50, 50);
        let plan = plan_frame(p, 20, 4, 4);
        assert_eq!(plan.total(), 20, "spends the whole budget");
        assert!(plan.light >= 4, "light keeps its reserved slice: {}", plan.light);
        assert!(plan.path >= 4, "path keeps its reserved slice: {}", plan.path);
        // The remainder (20 - 4 - 4 = 12) goes to load.
        assert_eq!(plan.load, 12);
    }

    #[test]
    fn remainder_goes_to_load_first() {
        // Budget beyond the reserves should prioritize loading.
        let p = pending(100, 100, 100);
        let plan = plan_frame(p, 30, 5, 5);
        // 5 light + 5 path reserved, remaining 20 → all load.
        assert_eq!(plan.load, 20);
        assert_eq!(plan.light, 5);
        assert_eq!(plan.path, 5);
    }

    #[test]
    fn reserves_capped_by_actual_pending() {
        // Only 1 lighting job pending though we reserve 4 — don't over-allocate.
        let p = pending(100, 1, 0);
        let plan = plan_frame(p, 10, 4, 4);
        assert_eq!(plan.light, 1, "can't light more than is pending");
        assert_eq!(plan.path, 0, "nothing to path");
        assert_eq!(plan.load, 9, "the rest goes to load");
        assert_eq!(plan.total(), 10);
    }

    #[test]
    fn zero_budget_does_nothing() {
        let plan = plan_frame(pending(5, 5, 5), 0, 4, 4);
        assert_eq!(plan, PhasePlan::default());
    }

    #[test]
    fn tiny_budget_smaller_than_reserves() {
        // Budget 3 but reserves want 4+4 — light reserve takes first, then path,
        // never exceeding the budget.
        let plan = plan_frame(pending(100, 100, 100), 3, 4, 4);
        assert_eq!(plan.total(), 3, "never exceeds budget");
        assert_eq!(plan.light, 3, "light reserve consumes the whole tiny budget");
        assert_eq!(plan.path, 0);
        assert_eq!(plan.load, 0);
    }

    #[test]
    fn never_exceeds_budget_across_random_shapes() {
        for load in [0, 1, 50, 9999] {
            for light in [0, 3, 100] {
                for path in [0, 7, 100] {
                    for budget in [0, 1, 8, 64, 100_000] {
                        let plan = plan_frame(pending(load, light, path), budget, 4, 4);
                        assert!(plan.total() <= budget, "over budget");
                        assert!(plan.load <= load && plan.light <= light && plan.path <= path,
                            "planned more than pending");
                    }
                }
            }
        }
    }
}
