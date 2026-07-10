//! `world::occlusion_flood` — CPU chunk-occlusion visibility flood.
//!
//! The cheapest correct occlusion cull for a voxel world: a chunk is only worth
//! drawing if you can actually *reach* it from the camera through the gaps in the
//! terrain. Starting at the camera's chunk, breadth-first flood outward; you may
//! propagate visibility *into* and *through* a chunk you can see through (one that
//! is not fully opaque — `chunk.cached_all_opaque == false`, r702), but a fully
//! opaque chunk is drawn (you see its near face) yet does **not** pass visibility
//! beyond itself. Chunks the flood never reaches are buried behind solid terrain
//! and can be skipped entirely — the classic "cave cull" that stops the renderer
//! from submitting the far side of a mountain.
//!
//! This complements frustum culling (which removes chunks outside the view) rather
//! than replacing it: `in_range` should already fold in the frustum + render
//! distance, so the flood only ever visits chunks the camera could see.
//!
//! Pure & deterministic; `HashSet`/`VecDeque` only. Self-contained for tests. The
//! renderer supplies the two closures from live chunk state; this module owns the
//! traversal so it can be verified in isolation.

use std::collections::{HashSet, VecDeque};

/// The six axis-aligned neighbours of a chunk coordinate.
#[inline]
fn neighbors6(c: [i32; 3]) -> [[i32; 3]; 6] {
    [
        [c[0] - 1, c[1], c[2]],
        [c[0] + 1, c[1], c[2]],
        [c[0], c[1] - 1, c[2]],
        [c[0], c[1] + 1, c[2]],
        [c[0], c[1], c[2] - 1],
        [c[0], c[1], c[2] + 1],
    ]
}

/// Flood the set of chunks visible from `camera`.
///
/// - `in_range(c)`: is chunk `c` a valid candidate this frame (loaded, inside the
///   render distance, and inside the view frustum)? The flood never leaves this set.
/// - `is_opaque(c)`: is `c` fully opaque on every face (`cached_all_opaque`)? An
///   opaque chunk is included in the result (its face is visible) but does not
///   propagate visibility past itself. The camera's own chunk always propagates —
///   you can see out of the block you're standing in even if it's solid.
///
/// Returns the set of visible chunk coordinates. If the camera itself is out of
/// range the result is empty (nothing to draw).
pub fn flood_visible(
    camera: [i32; 3],
    in_range: impl Fn([i32; 3]) -> bool,
    is_opaque: impl Fn([i32; 3]) -> bool,
) -> HashSet<[i32; 3]> {
    let mut visible: HashSet<[i32; 3]> = HashSet::new();
    if !in_range(camera) {
        return visible;
    }
    let mut queue: VecDeque<[i32; 3]> = VecDeque::new();
    queue.push_back(camera);
    visible.insert(camera);

    while let Some(c) = queue.pop_front() {
        // The camera chunk always sees out; any other chunk propagates only if it
        // is see-through. Opaque chunks are already marked visible (drawn) but stop
        // the flood here.
        let propagates = c == camera || !is_opaque(c);
        if !propagates {
            continue;
        }
        for n in neighbors6(c) {
            if in_range(n) && visible.insert(n) {
                queue.push_back(n);
            }
        }
    }
    visible
}

/// Count how many in-range chunks the flood culled (were candidates but not
/// visible) — a cheap metric for logging the cull's effectiveness. `candidates`
/// is the number of chunks `in_range` would accept; `visible` is the flood result.
pub fn culled_count(candidates: usize, visible: &HashSet<[i32; 3]>) -> usize {
    candidates.saturating_sub(visible.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box of chunks `[0,n)^3` all in range, with an explicit opaque set.
    fn in_box(n: i32) -> impl Fn([i32; 3]) -> bool {
        move |c| c.iter().all(|&v| v >= 0 && v < n)
    }

    #[test]
    fn open_field_sees_everything() {
        let n = 4;
        let vis = flood_visible([0, 0, 0], in_box(n), |_| false);
        assert_eq!(vis.len(), (n * n * n) as usize, "no opaque chunks → all visible");
    }

    #[test]
    fn out_of_range_camera_sees_nothing() {
        let vis = flood_visible([9, 9, 9], in_box(4), |_| false);
        assert!(vis.is_empty());
    }

    #[test]
    fn an_opaque_wall_hides_what_is_behind_it() {
        // 1×1 tunnel of 5 chunks along x; chunk x=2 is a solid plug.
        let line = |c: [i32; 3]| c[1] == 0 && c[2] == 0 && c[0] >= 0 && c[0] < 5;
        let plug = |c: [i32; 3]| c == [2, 0, 0];
        let vis = flood_visible([0, 0, 0], line, plug);
        // Reachable: x=0,1 (see-through), x=2 (the plug's near face is drawn).
        assert!(vis.contains(&[0, 0, 0]));
        assert!(vis.contains(&[1, 0, 0]));
        assert!(vis.contains(&[2, 0, 0]), "the wall itself is drawn");
        // Buried behind the plug: culled.
        assert!(!vis.contains(&[3, 0, 0]), "chunk behind the wall is culled");
        assert!(!vis.contains(&[4, 0, 0]));
    }

    #[test]
    fn camera_inside_solid_still_sees_out() {
        // Camera chunk is opaque, neighbours are open — you can see out.
        let line = |c: [i32; 3]| c[1] == 0 && c[2] == 0 && c[0] >= 0 && c[0] < 3;
        let cam_solid = |c: [i32; 3]| c == [0, 0, 0];
        let vis = flood_visible([0, 0, 0], line, cam_solid);
        assert!(vis.contains(&[1, 0, 0]), "can see out of the block you're in");
        assert!(vis.contains(&[2, 0, 0]));
    }

    #[test]
    fn a_full_slab_seals_off_the_far_side() {
        // 5×5×5 box, all in range; a full opaque plane at x==2 spans every (y,z),
        // so the flood cannot slip around it. Camera on the near side.
        let n = 5;
        let slab = |c: [i32; 3]| c[0] == 2;
        let vis = flood_visible([0, 2, 2], in_box(n), slab);
        // Near side + the slab face are visible…
        assert!(vis.contains(&[0, 2, 2]));
        assert!(vis.contains(&[1, 2, 2]));
        assert!(vis.contains(&[2, 2, 2]), "slab face is drawn");
        // …the far side is completely sealed off.
        assert!(!vis.contains(&[3, 2, 2]), "far side is culled");
        assert!(!vis.contains(&[4, 0, 0]), "nothing past the slab is reached");
        // Everything culled lies at x >= 3.
        assert!(vis.iter().all(|c| c[0] <= 2));
    }

    #[test]
    fn culled_count_metric() {
        let n = 4;
        let candidates = (n * n * n) as usize;
        let vis = flood_visible([0, 0, 0], in_box(n), |_| false);
        assert_eq!(culled_count(candidates, &vis), 0); // open field culls nothing
    }
}
