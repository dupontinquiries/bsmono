//! CPU render pre-pass — the render-side analog of `bending_runtime`.
//!
//! Composes the pure render cores into one "build the draw list" call the
//! renderer runs each frame:
//!   `frustum_plane_extract` (camera view-proj → 6 inward planes)
//!     → `chunk_frustum_cull`  (planes → visible chunks)
//!     → `lod_budget`          (assign LOD so the visible set fits a triangle budget)
//!     → `draw_order_sort`     (order to minimise pipeline rebinds + overdraw)
//!
//! One camera in, an ordered+budgeted+culled draw list out. Pure and
//! deterministic — no GPU, no I/O — so the renderer just consumes the result.

use crate::sim::chunk_frustum_cull::{self, Aabb};
use crate::sim::draw_order_sort::{self, DrawItem};
use crate::sim::frustum_plane_extract::{self, Plane};
use crate::sim::lod_budget::{self, Budget, ChunkLodInput};

/// One candidate chunk the renderer might draw this frame.
#[derive(Clone, Copy, Debug)]
pub struct ChunkInfo {
    /// World-space bounds, for the frustum cull.
    pub aabb: Aabb,
    /// Squared camera distance (coarsened first / drawn front-to-back).
    pub dist2: u32,
    /// Natural LOD from the distance bands; the floor for assignment.
    pub base_lod: u8,
    /// Estimated triangles at `base_lod`.
    pub est_tris: u32,
    /// GPU pipeline id (primary draw-order group key).
    pub pipeline: u16,
    /// Caller payload (e.g. mesh slot), carried through untouched.
    pub slot: u32,
}

/// One chunk to draw, in final order, with its budget-assigned LOD.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlannedDraw {
    pub slot: u32,
    pub lod: u8,
    pub dist2: u32,
    pub pipeline: u16,
}

/// Reusable scratch so the per-frame call need not reallocate. Pass the same
/// instance every frame; `build_into` clears it as needed.
#[derive(Clone, Debug, Default)]
pub struct Scratch {
    aabbs: Vec<Aabb>,
    visible: Vec<usize>,
    lod_inputs: Vec<ChunkLodInput>,
    lods: Vec<u8>,
    items: Vec<DrawItem>,
    order: Vec<u32>,
}

/// The finished frame plan.
#[derive(Clone, Debug, Default)]
pub struct RenderPlan {
    /// Visible chunks, LOD-assigned, in draw order.
    pub draws: Vec<PlannedDraw>,
    /// Total estimated triangles after budgeting.
    pub est_tris: u64,
    /// How many candidate chunks were frustum-culled.
    pub culled: usize,
}

fn planes_to_arrays(p: &[Plane; 6]) -> [[f32; 4]; 6] {
    let mut out = [[0.0f32; 4]; 6];
    for i in 0..6 {
        out[i] = [p[i].a, p[i].b, p[i].c, p[i].d];
    }
    out
}

/// Build the ordered, budget-fitted, frustum-culled draw list (allocating a
/// fresh plan). For a hot loop prefer [`build_into`] with reused scratch.
pub fn build(viewproj: &[[f32; 4]; 4], chunks: &[ChunkInfo], budget: Budget) -> RenderPlan {
    let mut scratch = Scratch::default();
    let mut plan = RenderPlan::default();
    build_into(viewproj, chunks, budget, &mut scratch, &mut plan);
    plan
}

/// Same as [`build`] but writes into a caller-owned `plan` using reusable
/// `scratch`, so a steady frame loop does no per-frame allocation after warmup.
pub fn build_into(
    viewproj: &[[f32; 4]; 4],
    chunks: &[ChunkInfo],
    budget: Budget,
    scratch: &mut Scratch,
    plan: &mut RenderPlan,
) {
    plan.draws.clear();
    plan.est_tris = 0;
    plan.culled = 0;

    // 1. camera → 6 inward-facing planes.
    let planes = planes_to_arrays(&frustum_plane_extract::extract(viewproj));

    // 2. cull: which candidate chunks are visible.
    scratch.aabbs.clear();
    scratch.aabbs.extend(chunks.iter().map(|c| c.aabb));
    chunk_frustum_cull::cull(&planes, &scratch.aabbs, &mut scratch.visible);
    plan.culled = chunks.len().saturating_sub(scratch.visible.len());
    if scratch.visible.is_empty() {
        return;
    }

    // 3. assign LOD so the visible set fits the triangle budget.
    scratch.lod_inputs.clear();
    scratch.lod_inputs.extend(scratch.visible.iter().map(|&i| {
        let c = &chunks[i];
        ChunkLodInput { dist2: c.dist2, base_lod: c.base_lod, est_tris: c.est_tris }
    }));
    lod_budget::assign(&scratch.lod_inputs, budget, &mut scratch.lods);
    plan.est_tris = lod_budget::est_total_tris(&scratch.lod_inputs, &scratch.lods);

    // 4. order to minimise pipeline rebinds + overdraw (uses the assigned LOD).
    scratch.items.clear();
    scratch.items.extend(scratch.visible.iter().enumerate().map(|(k, &i)| {
        let c = &chunks[i];
        DrawItem { pipeline: c.pipeline, lod: scratch.lods[k], dist2: c.dist2, slot: c.slot }
    }));
    draw_order_sort::order(&scratch.items, &mut scratch.order);

    // 5. emit in draw order.
    plan.draws.reserve(scratch.order.len());
    for &o in &scratch.order {
        let it = scratch.items[o as usize];
        plan.draws.push(PlannedDraw {
            slot: it.slot,
            lod: it.lod,
            dist2: it.dist2,
            pipeline: it.pipeline,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> [[f32; 4]; 4] {
        [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ]
    }
    fn near_box() -> Aabb {
        Aabb { min: [-0.5, -0.5, -0.5], max: [0.5, 0.5, 0.5] }
    }
    fn chunk(pipeline: u16, slot: u32, dist2: u32, est_tris: u32, base_lod: u8) -> ChunkInfo {
        ChunkInfo { aabb: near_box(), dist2, base_lod, est_tris, pipeline, slot }
    }

    #[test]
    fn draws_plus_culled_equals_total() {
        let chunks = [
            chunk(0, 1, 10, 1000, 0),
            chunk(1, 2, 20, 1000, 0),
            chunk(0, 3, 30, 1000, 0),
        ];
        let budget = Budget { max_tris: u64::MAX, max_lod: 3 };
        let plan = build(&identity(), &chunks, budget);
        assert_eq!(plan.draws.len() + plan.culled, chunks.len());
    }

    #[test]
    fn far_box_is_culled() {
        let mut far = chunk(0, 9, 1_000_000, 100, 0);
        far.aabb = Aabb { min: [500.0, 500.0, 500.0], max: [501.0, 501.0, 501.0] };
        let chunks = [chunk(0, 1, 1, 100, 0), far];
        let plan = build(&identity(), &chunks, Budget { max_tris: u64::MAX, max_lod: 3 });
        assert!(plan.culled >= 1, "a box far outside clip space should be culled");
        assert!(plan.draws.iter().all(|d| d.slot != 9), "the far chunk must not be drawn");
    }

    #[test]
    fn generous_budget_keeps_base_lod() {
        let chunks = [chunk(0, 1, 10, 5000, 1), chunk(0, 2, 20, 5000, 1)];
        let plan = build(&identity(), &chunks, Budget { max_tris: u64::MAX, max_lod: 4 });
        assert!(plan.draws.iter().all(|d| d.lod == 1), "under budget, LOD stays at base");
    }

    #[test]
    fn tight_budget_fits_or_maxes_lod() {
        let chunks = [
            chunk(0, 1, 10, 100_000, 0),
            chunk(0, 2, 9_999_999, 100_000, 0),
            chunk(0, 3, 5_000_000, 100_000, 0),
        ];
        let budget = Budget { max_tris: 50_000, max_lod: 4 };
        let plan = build(&identity(), &chunks, budget);
        if !plan.draws.is_empty() {
            // Either under budget, or everything was coarsened to the ceiling.
            let maxed = plan.draws.iter().all(|d| d.lod == 4);
            assert!(plan.est_tris <= budget.max_tris || maxed,
                "tight budget must fit or hit the LOD ceiling: est={}", plan.est_tris);
            assert!(plan.draws.iter().all(|d| d.lod >= 0));
        }
    }

    #[test]
    fn draws_grouped_by_pipeline() {
        let chunks = [
            chunk(2, 1, 10, 100, 0),
            chunk(1, 2, 20, 100, 0),
            chunk(2, 3, 30, 100, 0),
            chunk(1, 4, 40, 100, 0),
        ];
        let plan = build(&identity(), &chunks, Budget { max_tris: u64::MAX, max_lod: 3 });
        // Once a pipeline ends in the order it must not reappear (contiguous groups).
        let mut seen = std::collections::HashSet::new();
        let mut prev: Option<u16> = None;
        for d in &plan.draws {
            if prev != Some(d.pipeline) {
                assert!(seen.insert(d.pipeline), "pipeline {} is not contiguous", d.pipeline);
                prev = Some(d.pipeline);
            }
        }
    }

    #[test]
    fn empty_input_empty_plan() {
        let plan = build(&identity(), &[], Budget { max_tris: 100, max_lod: 3 });
        assert!(plan.draws.is_empty() && plan.culled == 0 && plan.est_tris == 0);
    }

    #[test]
    fn deterministic_and_scratch_matches_alloc() {
        let chunks = [
            chunk(1, 1, 10, 200, 0),
            chunk(0, 2, 20, 200, 0),
            chunk(1, 3, 5, 200, 0),
        ];
        let budget = Budget { max_tris: 1000, max_lod: 3 };
        let a = build(&identity(), &chunks, budget);
        let b = build(&identity(), &chunks, budget);
        assert_eq!(a.draws, b.draws);
        // build_into with reused scratch must match the allocating build.
        let mut s = Scratch::default();
        let mut p = RenderPlan::default();
        build_into(&identity(), &chunks, budget, &mut s, &mut p);
        build_into(&identity(), &chunks, budget, &mut s, &mut p); // reuse proves clearing
        assert_eq!(p.draws, a.draws);
        assert_eq!(p.culled, a.culled);
    }
}
