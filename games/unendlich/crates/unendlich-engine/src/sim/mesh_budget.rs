//! hh-r332-t1 — per-frame near-mesh **build** budget (pure core).
//!
//! ## The hitch (r331 M2 profile)
//! `mesher_timer` near tier: p50 0.13 ms but **p99 104.5 / max 1020.8 ms** — a
//! single near chunk can build for ~1 s *synchronously*, blowing one frame from
//! ~11 ms to 640 ms. Uploads are already budgeted
//! (`runtime_config.max_uploads_per_frame = 64`, `upload_budget_ms = 4.0`), but
//! the CPU **mesh build** is not: the near-mesh queue drains greedily, so one
//! oversized chunk on a fast descent monopolises the frame.
//!
//! ## The lever
//! A pure per-frame *build-admission* core the `chunk_manager` queue-drain
//! consults to decide **which** queued near meshes to build this frame, under a
//! hard count cap + a cumulative cost budget, **nearest-first**, deferring the
//! rest to later frames. It never meshes here — it ranks/limits using a cheap
//! cost proxy ([`estimate_cost`]) computed from the solid-cell count the mesher
//! already has (`render::mesher` keeps `solid_count`), so the scheduler picks
//! before paying.
//!
//! ## Progress guarantee (no permanent starvation)
//! A chunk whose estimated cost alone exceeds the whole frame budget would, under
//! a naive "stay within budget" rule, *never* be admitted — a permanent hole in
//! the world. So [`admit`] **always admits the nearest queued chunk**, even if it
//! alone blows the budget; in that case it is admitted *alone* and everything
//! else defers. The unavoidable big build happens once, for the nearest chunk,
//! and the frame contains exactly one such spike instead of a pile-up.
//!
//! Pure / deterministic / panic-free: no IO, clock, RNG, `unwrap`/`expect`, or
//! float NaN traps; saturating integer arithmetic throughout. The env-overridable
//! knobs live in `runtime_config` (see HANDOFF_mesh_budget.md); this core carries
//! only their defaults so it stays self-contained for the standalone gate.

// =============================================================================
// Cost proxy
// =============================================================================

/// Number of LOD tiers (mirrors `world::lod::LodLevel`: Near=0, Mid=1, Far=2,
/// VeryFar=3). `estimate_cost` accepts any `u8`; tiers beyond this are simply
/// treated as progressively cheaper.
pub const NUM_LOD: u8 = 4;

/// Fixed per-build setup cost (μs): allocation, neighbour fetch, finalize. Floor
/// for even a trivial chunk so the scheduler never thinks a build is free.
pub const COST_SETUP_US: u32 = 50;

/// Marginal cost per solid cell at the Near (lod 0) tier, in **nanoseconds**.
/// Greedy meshing with AO/tints/plant voxelization is the dominant near cost and
/// scales with solid-cell count. This is a deliberately **coarse proxy**, not a
/// calibrated model — only its monotonicity and cheapness matter for ranking; the
/// M2 can recalibrate the constant later without touching the scheduler.
pub const COST_PER_CELL_NS: u32 = 900;

/// Estimate a chunk's mesh-build cost in **microseconds** from its solid-cell
/// count and LOD tier, without meshing it.
///
/// * Monotonically **non-decreasing** in `solid_cells`.
/// * Monotonically **non-increasing** in `lod` (coarser tiers do less work;
///   modelled as halving effective cells per tier).
/// * Saturating: never overflows or panics, even at `u32::MAX` cells / large lod.
#[inline]
pub fn estimate_cost(solid_cells: u32, lod: u8) -> u32 {
    // Coarser LOD → fewer effective cells. Clamp the shift so it can never hit
    // the u32 shift-overflow trap (>=32) regardless of caller input.
    let shift = if lod > 31 { 31 } else { lod as u32 };
    let effective = (solid_cells >> shift) as u64;
    // μs = effective_cells * ns_per_cell / 1000, in u64 to avoid overflow.
    let marginal_us = effective.saturating_mul(COST_PER_CELL_NS as u64) / 1000;
    let total = (COST_SETUP_US as u64).saturating_add(marginal_us);
    if total > u32::MAX as u64 { u32::MAX } else { total as u32 }
}

// =============================================================================
// Budget + queue items
// =============================================================================

/// Per-frame build budget. Mirror of the two `runtime_config` knobs this core is
/// driven by (defaults match — see HANDOFF).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshBudget {
    /// Hard cap on meshes built per frame (count). `0` builds nothing.
    pub max_meshes_per_frame: u32,
    /// Cumulative build-cost budget per frame, in **microseconds**. The sum of
    /// admitted costs stays `<=` this, with the single documented exception of an
    /// oversized nearest chunk admitted alone (progress guarantee).
    pub build_budget_us: u64,
}

impl MeshBudget {
    /// `runtime_config` default for the count cap (`UNENDLICH_MAX_MESHES_PER_FRAME`).
    pub const DEFAULT_MAX_MESHES_PER_FRAME: u32 = 8;
    /// `runtime_config` default for the cost cap, in ms
    /// (`UNENDLICH_MESH_BUILD_BUDGET_MS`). 3 ms leaves headroom under the ~11 ms
    /// p50 frame while still draining the queue in a few frames.
    pub const DEFAULT_MESH_BUILD_BUDGET_MS: f32 = 3.0;

    /// The shipping default budget (8 meshes / 3.0 ms).
    #[inline]
    pub fn default_budget() -> Self {
        MeshBudget {
            max_meshes_per_frame: Self::DEFAULT_MAX_MESHES_PER_FRAME,
            build_budget_us: (Self::DEFAULT_MESH_BUILD_BUDGET_MS * 1000.0) as u64, // 3000
        }
    }
}

/// A queued near-mesh build request. The manager fills `distance` (nearer =
/// smaller; in-view chunks should be biased small by the caller) and `cost_us`
/// (from [`estimate_cost`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingMesh {
    /// Priority key — smaller is built sooner. Caller's choice of metric
    /// (squared block distance, or distance with an in-view discount).
    pub distance: u32,
    /// Estimated build cost in microseconds (see [`estimate_cost`]).
    pub cost_us: u32,
}

impl PendingMesh {
    #[inline]
    pub fn new(distance: u32, cost_us: u32) -> Self {
        PendingMesh { distance, cost_us }
    }
}

/// Choose which queued near meshes to build **this frame**.
///
/// Returns indices into `pending`, in build order (nearest first). Guarantees:
/// * `result.len() <= budget.max_meshes_per_frame` (hard count cap).
/// * `sum(cost_us of result) <= budget.build_budget_us`, **except** when the
///   single nearest chunk alone exceeds the budget — then it is admitted alone
///   (progress guarantee), and that is the only way the cost cap is exceeded.
/// * Nearest-first: results are ordered by `distance` (ties by original index,
///   so the output is deterministic). A nearer chunk is never skipped to pack a
///   farther one — strict priority, so the player's immediate surroundings fill
///   in first.
/// * `pending` empty or `max_meshes_per_frame == 0` → empty.
///
/// Alloc-light: one index-sort scratch + the result vector, both `<= pending.len()`.
pub fn admit(pending: &[PendingMesh], budget: MeshBudget) -> Vec<usize> {
    if pending.is_empty() || budget.max_meshes_per_frame == 0 {
        return Vec::new();
    }

    // Nearest-first, stable: sort indices by (distance, original index).
    let mut order: Vec<usize> = (0..pending.len()).collect();
    order.sort_by(|&a, &b| {
        pending[a]
            .distance
            .cmp(&pending[b].distance)
            .then_with(|| a.cmp(&b))
    });

    let mut out: Vec<usize> = Vec::new();
    let mut cost_sum: u64 = 0;

    for &i in &order {
        if out.len() as u32 >= budget.max_meshes_per_frame {
            break;
        }
        let c = pending[i].cost_us as u64;
        if out.is_empty() {
            // Progress guarantee: always take the nearest, even if it alone
            // blows the budget. If it did, admit it alone and stop.
            out.push(i);
            cost_sum = c;
            if cost_sum > budget.build_budget_us {
                break;
            }
        } else if cost_sum.saturating_add(c) <= budget.build_budget_us {
            out.push(i);
            cost_sum += c;
        } else {
            // Strict nearest-first: this near chunk doesn't fit, so stop rather
            // than skipping ahead to a farther (possibly smaller) one. It is the
            // next frame's nearest and will be admitted then (alone if oversized).
            break;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cost_of(out: &[usize], pending: &[PendingMesh]) -> u64 {
        out.iter().map(|&i| pending[i].cost_us as u64).sum()
    }

    // ---------- estimate_cost ----------

    #[test]
    fn cost_has_setup_floor_and_grows_with_cells() {
        assert_eq!(estimate_cost(0, 0), COST_SETUP_US);
        assert!(estimate_cost(10_000, 0) > estimate_cost(1_000, 0));
        assert!(estimate_cost(1_000, 0) > estimate_cost(0, 0));
    }

    #[test]
    fn cost_monotonic_nondecreasing_in_cells() {
        let mut prev = 0;
        for cells in (0..=200_000).step_by(5_000) {
            let c = estimate_cost(cells, 0);
            assert!(c >= prev, "cost dropped as cells grew at {cells}");
            prev = c;
        }
    }

    #[test]
    fn cost_monotonic_nonincreasing_in_lod() {
        let cells = 100_000;
        let mut prev = u32::MAX;
        for lod in 0..=6u8 {
            let c = estimate_cost(cells, lod);
            assert!(c <= prev, "cost rose as lod coarsened at lod {lod}");
            prev = c;
        }
        // Near strictly costlier than VeryFar for a non-trivial chunk.
        assert!(estimate_cost(cells, 0) > estimate_cost(cells, 3));
    }

    #[test]
    fn cost_saturates_without_panic() {
        // u32::MAX cells at the most expensive tier must not overflow/panic.
        let c = estimate_cost(u32::MAX, 0);
        assert!(c > 0);
        // Absurd lod must not hit the shift-overflow trap.
        let _ = estimate_cost(u32::MAX, 200);
        assert_eq!(estimate_cost(u32::MAX, 200), estimate_cost(u32::MAX, 31));
    }

    // ---------- admit ----------

    #[test]
    fn empty_queue_admits_nothing() {
        assert!(admit(&[], MeshBudget::default_budget()).is_empty());
    }

    #[test]
    fn zero_count_cap_admits_nothing() {
        let p = [PendingMesh::new(0, 100), PendingMesh::new(1, 100)];
        let b = MeshBudget { max_meshes_per_frame: 0, build_budget_us: 1_000_000 };
        assert!(admit(&p, b).is_empty());
    }

    #[test]
    fn never_exceeds_count_cap() {
        // 20 cheap chunks, generous cost budget, count cap 8.
        let p: Vec<PendingMesh> = (0..20).map(|d| PendingMesh::new(d, 10)).collect();
        let b = MeshBudget { max_meshes_per_frame: 8, build_budget_us: 10_000_000 };
        let out = admit(&p, b);
        assert_eq!(out.len(), 8);
    }

    #[test]
    fn never_exceeds_cost_budget_when_multiple_fit() {
        // each 800 us, budget 3000 us → at most 3 fit (3*800=2400, 4th would be 3200).
        let p: Vec<PendingMesh> = (0..10).map(|d| PendingMesh::new(d, 800)).collect();
        let b = MeshBudget { max_meshes_per_frame: 100, build_budget_us: 3000 };
        let out = admit(&p, b);
        assert!(cost_of(&out, &p) <= b.build_budget_us);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn oversized_nearest_admitted_alone_rest_deferred() {
        // nearest chunk costs 5 ms, budget 3 ms. It must be admitted ALONE.
        let p = [
            PendingMesh::new(0, 5000), // nearest, over budget
            PendingMesh::new(1, 100),
            PendingMesh::new(2, 100),
        ];
        let b = MeshBudget { max_meshes_per_frame: 8, build_budget_us: 3000 };
        let out = admit(&p, b);
        assert_eq!(out, vec![0], "oversized nearest should be admitted alone");
    }

    #[test]
    fn nearest_first_ordering() {
        // distances out of order; result must be sorted nearest-first.
        let p = [
            PendingMesh::new(30, 100),
            PendingMesh::new(10, 100),
            PendingMesh::new(20, 100),
        ];
        let b = MeshBudget { max_meshes_per_frame: 8, build_budget_us: 1_000_000 };
        let out = admit(&p, b);
        assert_eq!(out, vec![1, 2, 0]); // distances 10,20,30
    }

    #[test]
    fn ties_break_by_index_deterministically() {
        let p = [
            PendingMesh::new(5, 100),
            PendingMesh::new(5, 100),
            PendingMesh::new(5, 100),
        ];
        let b = MeshBudget { max_meshes_per_frame: 8, build_budget_us: 1_000_000 };
        let out = admit(&p, b);
        assert_eq!(out, vec![0, 1, 2]);
        // determinism: same input → same output
        assert_eq!(admit(&p, b), out);
    }

    #[test]
    fn strict_priority_does_not_skip_near_to_pack_far() {
        // near chunk (dist 0) costs 2500; budget 3000. A far cheap chunk (dist 9,
        // cost 100) would "fit" but must NOT be packed ahead of the deferred work:
        // after the near one, remaining budget is 500; the next by distance is
        // dist 1 cost 2500 which doesn't fit → stop. The far cheap chunk waits.
        let p = [
            PendingMesh::new(0, 2500),
            PendingMesh::new(1, 2500),
            PendingMesh::new(9, 100),
        ];
        let b = MeshBudget { max_meshes_per_frame: 8, build_budget_us: 3000 };
        let out = admit(&p, b);
        assert_eq!(out, vec![0], "must not reorder a far cheap chunk ahead of nearer work");
    }

    #[test]
    fn head_always_makes_progress_no_starvation() {
        // Simulate frames: an oversized nearest chunk is built (alone) each frame
        // and removed; the queue eventually fully drains — nothing starves.
        let mut queue: Vec<PendingMesh> = vec![
            PendingMesh::new(0, 9000), // huge, over budget
            PendingMesh::new(1, 9000), // huge, over budget
            PendingMesh::new(2, 500),
            PendingMesh::new(3, 500),
        ];
        let b = MeshBudget { max_meshes_per_frame: 8, build_budget_us: 3000 };
        let mut frames = 0;
        while !queue.is_empty() {
            let out = admit(&queue, b);
            assert!(!out.is_empty(), "non-empty queue must admit at least one (progress)");
            // remove admitted (descending index so removal is stable)
            let mut idxs = out.clone();
            idxs.sort_unstable_by(|a, b| b.cmp(a));
            for i in idxs {
                queue.remove(i);
            }
            frames += 1;
            assert!(frames < 100, "queue failed to drain — starvation");
        }
        // 2 oversized (alone each) + the two small ones together = 3 frames.
        assert_eq!(frames, 3);
    }

    #[test]
    fn integrates_with_estimate_cost() {
        // Build pending costs via estimate_cost; ensure admit stays within budget.
        let p = [
            PendingMesh::new(0, estimate_cost(2_000, 0)),
            PendingMesh::new(1, estimate_cost(1_500, 0)),
            PendingMesh::new(2, estimate_cost(1_000, 0)),
        ];
        let b = MeshBudget::default_budget(); // 8 / 3000us
        let out = admit(&p, b);
        // costs are ~1.8/1.4/0.95 ms → first two fit (~3.2ms? check), determinism holds
        assert!(!out.is_empty());
        assert_eq!(out, admit(&p, b));
        // either within budget, or a lone oversized head
        let total = cost_of(&out, &p);
        assert!(total <= b.build_budget_us || out.len() == 1);
    }
}
