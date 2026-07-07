//! `sim::chunk_frustum_cull` — pure, allocation-light AABB-vs-frustum culling.
//!
//! Each frame the CPU narrows the loaded chunk set down to the on-screen
//! candidates before the GPU does fine cull (M2 perf data: ~307 loaded → ~98
//! candidates), and that narrowing is `O(loaded)` work inside the
//! frame-limiting `draw` zone. This module is the inner test it needs: a
//! conservative axis-aligned-box-vs-frustum visibility check with no render or
//! `ash`/`vk` types — plain arrays in, indices out. The coordinator extracts the
//! six frustum planes from the view-projection matrix at integration and feeds
//! them here.
//!
//! ## Plane convention
//! A plane is `[a, b, c, d]`; a point `p` is INSIDE its half-space iff
//! `a·pₓ + b·p_y + c·p_z + d >= 0`. The six planes face **inward**, so a box is
//! visible iff it is inside (or straddling) ALL of them.
//!
//! ## The test (positive-vertex / p-vertex)
//! For each plane, the **p-vertex** is the box corner farthest along the plane
//! normal `(a, b, c)` — per axis, `max` if that normal component is `>= 0` else
//! `min`. The p-vertex maximizes the plane function over the whole box, so if
//! even the p-vertex is outside (`< 0`), every corner is outside and the box is
//! entirely beyond that plane ⇒ cull. If the p-vertex is inside for all six
//! planes, the box is kept. This is **conservative**: it culls only boxes that
//! lie wholly outside some single plane, so it never culls a box that is even
//! partially inside the frustum (it may keep a few boxes just outside a frustum
//! corner — the cheap, safe trade GPU fine-cull then resolves). Planes need not
//! be normalized; only the sign of the p-vertex evaluation matters.
//!
//! Pure: data in, indices out. No render/Vulkan, no IO, no clock, no RNG, no
//! allocation beyond the caller's `out` vector, no `.unwrap()`/`.expect()`.

/// Axis-aligned bounding box in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Is `bb` inside (or straddling) all six inward-facing frustum planes?
///
/// Conservative p-vertex test: returns `false` only when the box lies wholly
/// outside at least one plane.
pub fn aabb_visible(planes: &[[f32; 4]; 6], bb: &Aabb) -> bool {
    let mut i = 0;
    while i < 6 {
        let pl = planes[i];
        // p-vertex: farthest corner along this plane's normal.
        let px = if pl[0] >= 0.0 { bb.max[0] } else { bb.min[0] };
        let py = if pl[1] >= 0.0 { bb.max[1] } else { bb.min[1] };
        let pz = if pl[2] >= 0.0 { bb.max[2] } else { bb.min[2] };
        // If even the farthest corner is outside this half-space, the whole box
        // is outside ⇒ not visible.
        if pl[0] * px + pl[1] * py + pl[2] * pz + pl[3] < 0.0 {
            return false;
        }
        i += 1;
    }
    true
}

/// Append the indices of the visible boxes to `out`, in ascending order.
///
/// `out` is cleared first; no allocation other than `out`'s own growth. The
/// emitted indices are exactly the `i` for which `aabb_visible(planes, &boxes[i])`.
pub fn cull(planes: &[[f32; 4]; 6], boxes: &[Aabb], out: &mut Vec<usize>) {
    out.clear();
    let mut i = 0;
    while i < boxes.len() {
        if aabb_visible(planes, &boxes[i]) {
            out.push(i);
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn randf(seed: &mut u32, lo: f32, hi: f32) -> f32 {
        lo + (lcg(seed) % 1_000_001) as f32 / 1_000_000.0 * (hi - lo)
    }

    /// Axis-aligned box frustum: the cube [-H, H]^3, six inward planes.
    const H: f32 = 10.0;
    fn box_frustum() -> [[f32; 4]; 6] {
        [
            [1.0, 0.0, 0.0, H],  // x >= -H
            [-1.0, 0.0, 0.0, H], // x <=  H
            [0.0, 1.0, 0.0, H],  // y >= -H
            [0.0, -1.0, 0.0, H], // y <=  H
            [0.0, 0.0, 1.0, H],  // z >= -H
            [0.0, 0.0, -1.0, H], // z <=  H
        ]
    }
    /// Exact visibility for the axis-aligned box frustum: AABB overlaps the
    /// cube [-H,H]^3 iff it overlaps on every axis. (For an axis-aligned
    /// frustum the p-vertex test is exact, so this is the ground truth.)
    fn overlaps_cube(bb: &Aabb) -> bool {
        for k in 0..3 {
            if bb.max[k] < -H || bb.min[k] > H {
                return false;
            }
        }
        true
    }

    fn aabb(min: [f32; 3], max: [f32; 3]) -> Aabb {
        Aabb { min, max }
    }
    fn corners(bb: &Aabb) -> [[f32; 3]; 8] {
        let mut c = [[0.0f32; 3]; 8];
        let mut k = 0;
        for &xi in &[bb.min[0], bb.max[0]] {
            for &yi in &[bb.min[1], bb.max[1]] {
                for &zi in &[bb.min[2], bb.max[2]] {
                    c[k] = [xi, yi, zi];
                    k += 1;
                }
            }
        }
        c
    }
    fn point_inside_all(planes: &[[f32; 4]; 6], p: [f32; 3]) -> bool {
        planes
            .iter()
            .all(|pl| pl[0] * p[0] + pl[1] * p[1] + pl[2] * p[2] + pl[3] >= 0.0)
    }

    #[test]
    fn fully_inside_is_visible() {
        let f = box_frustum();
        assert!(aabb_visible(&f, &aabb([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0])));
    }

    #[test]
    fn fully_past_one_plane_is_culled() {
        let f = box_frustum();
        // entirely beyond +X wall (x in [20,30])
        assert!(!aabb_visible(&f, &aabb([20.0, -1.0, -1.0], [30.0, 1.0, 1.0])));
        // entirely below -Y wall
        assert!(!aabb_visible(&f, &aabb([-1.0, -50.0, -1.0], [1.0, -20.0, 1.0])));
    }

    #[test]
    fn straddling_a_plane_is_visible() {
        let f = box_frustum();
        // crosses the x = H wall (x in [5,15]) — partially inside ⇒ kept
        assert!(aabb_visible(&f, &aabb([5.0, -1.0, -1.0], [15.0, 1.0, 1.0])));
        // a huge box enclosing the whole frustum is visible
        assert!(aabb_visible(&f, &aabb([-100.0, -100.0, -100.0], [100.0, 100.0, 100.0])));
    }

    #[test]
    fn degenerate_point_box_handled() {
        let f = box_frustum();
        // point inside
        assert!(aabb_visible(&f, &aabb([0.0, 0.0, 0.0], [0.0, 0.0, 0.0])));
        // point on the boundary (x == H) is inside (>= 0)
        assert!(aabb_visible(&f, &aabb([H, 0.0, 0.0], [H, 0.0, 0.0])));
        // point outside
        assert!(!aabb_visible(&f, &aabb([H + 1.0, 0.0, 0.0], [H + 1.0, 0.0, 0.0])));
    }

    #[test]
    fn matches_exact_overlap_on_box_frustum() {
        let f = box_frustum();
        let mut s = 0xFC11_0000u32;
        for _ in 0..50_000 {
            let lo = [
                randf(&mut s, -30.0, 30.0),
                randf(&mut s, -30.0, 30.0),
                randf(&mut s, -30.0, 30.0),
            ];
            let ext = [
                randf(&mut s, 0.0, 12.0),
                randf(&mut s, 0.0, 12.0),
                randf(&mut s, 0.0, 12.0),
            ];
            let bb = aabb(lo, [lo[0] + ext[0], lo[1] + ext[1], lo[2] + ext[2]]);
            assert_eq!(
                aabb_visible(&f, &bb),
                overlaps_cube(&bb),
                "p-vertex disagreed with exact overlap for {:?}",
                bb
            );
        }
    }

    #[test]
    fn never_culls_a_box_with_a_corner_inside_arbitrary_planes() {
        // Conservativeness for GENERAL (non-axis-aligned, unnormalized) planes:
        // if any corner is inside all six planes, the box must be kept.
        let mut s = 0xFC11_0001u32;
        for _ in 0..20_000 {
            let mut planes = [[0.0f32; 4]; 6];
            for p in planes.iter_mut() {
                *p = [
                    randf(&mut s, -2.0, 2.0),
                    randf(&mut s, -2.0, 2.0),
                    randf(&mut s, -2.0, 2.0),
                    randf(&mut s, -20.0, 20.0),
                ];
            }
            let lo = [
                randf(&mut s, -20.0, 20.0),
                randf(&mut s, -20.0, 20.0),
                randf(&mut s, -20.0, 20.0),
            ];
            let ext = [
                randf(&mut s, 0.0, 15.0),
                randf(&mut s, 0.0, 15.0),
                randf(&mut s, 0.0, 15.0),
            ];
            let bb = aabb(lo, [lo[0] + ext[0], lo[1] + ext[1], lo[2] + ext[2]]);
            if corners(&bb).iter().any(|&c| point_inside_all(&planes, c)) {
                assert!(
                    aabb_visible(&planes, &bb),
                    "culled a box with a corner inside the frustum: {:?}",
                    bb
                );
            }
        }
    }

    #[test]
    fn cull_clears_first_and_matches_per_box() {
        let f = box_frustum();
        let boxes = vec![
            aabb([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]),    // 0 inside
            aabb([20.0, 20.0, 20.0], [21.0, 21.0, 21.0]), // 1 outside
            aabb([5.0, -1.0, -1.0], [15.0, 1.0, 1.0]),    // 2 straddling -> visible
            aabb([-50.0, -50.0, -50.0], [-40.0, -40.0, -40.0]), // 3 outside
            aabb([0.0, 0.0, 0.0], [0.0, 0.0, 0.0]),       // 4 point inside
        ];
        // pre-fill out with junk to prove it's cleared
        let mut out = vec![999usize, 998, 997];
        cull(&f, &boxes, &mut out);
        assert_eq!(out, vec![0, 2, 4]);

        // ascending + exactly matches aabb_visible per box
        let expected: Vec<usize> = (0..boxes.len())
            .filter(|&i| aabb_visible(&f, &boxes[i]))
            .collect();
        assert_eq!(out, expected);
        for w in out.windows(2) {
            assert!(w[0] < w[1], "indices must be strictly ascending");
        }
    }

    #[test]
    fn empty_boxes_yield_empty_out() {
        let f = box_frustum();
        let mut out = vec![1, 2, 3];
        cull(&f, &[], &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn cull_matches_aabb_visible_on_random_inputs() {
        let f = box_frustum();
        let mut s = 0xFC11_0002u32;
        for _ in 0..2000 {
            let n = (lcg(&mut s) % 40) as usize;
            let mut boxes = Vec::with_capacity(n);
            for _ in 0..n {
                let lo = [
                    randf(&mut s, -25.0, 25.0),
                    randf(&mut s, -25.0, 25.0),
                    randf(&mut s, -25.0, 25.0),
                ];
                let ext = [
                    randf(&mut s, 0.0, 10.0),
                    randf(&mut s, 0.0, 10.0),
                    randf(&mut s, 0.0, 10.0),
                ];
                boxes.push(aabb(lo, [lo[0] + ext[0], lo[1] + ext[1], lo[2] + ext[2]]));
            }
            let mut out = Vec::new();
            cull(&f, &boxes, &mut out);
            let expected: Vec<usize> =
                (0..boxes.len()).filter(|&i| aabb_visible(&f, &boxes[i])).collect();
            assert_eq!(out, expected);
        }
    }

    #[test]
    fn deterministic() {
        let f = box_frustum();
        let mut s = 0xFC11_0003u32;
        let mut boxes = Vec::new();
        for _ in 0..64 {
            let lo = [randf(&mut s, -20.0, 20.0), randf(&mut s, -20.0, 20.0), randf(&mut s, -20.0, 20.0)];
            boxes.push(aabb(lo, [lo[0] + 3.0, lo[1] + 3.0, lo[2] + 3.0]));
        }
        let mut a = Vec::new();
        let mut b = Vec::new();
        cull(&f, &boxes, &mut a);
        cull(&f, &boxes, &mut b);
        assert_eq!(a, b);
    }
}
