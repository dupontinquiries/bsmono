//! Entity-mesh sanitization — drops geometry with non-finite (NaN/±Inf)
//! vertex positions before it reaches the GPU.
//!
//! # Why this exists
//!
//! The chunk-mesh path (`render::mesher::PackedVertex`) is structurally
//! immune to this bug class: every position gets `.clamp(0.0, 1.0)`
//! before packing, and Rust's `as u32` float→int cast saturates NaN to
//! 0 rather than producing an undefined bit pattern. It ALSO carries
//! explicit `is_finite`/`is_nan` guards on top of that (2 hits in
//! `mesher.rs`).
//!
//! The creature/entity path (`world::entity::forge_to_vertices`,
//! `creature_to_vertices`, feeding `render::entity_pipeline`) has
//! neither. It uses full-precision `EntityVertex { position: [f32; 3],
//! .. }`, taken directly from `creature.body.position` /`.yaw` / `.pitch`
//! / `.roll` — physics-sim-driven f32 state with no invariant enforced
//! by the type system — and uploads it to the GPU with zero guards
//! anywhere in `physics.rs`, `creature.rs`, or `entity_pipeline.rs`
//! (grep confirms 0 `is_finite`/`is_nan` hits in all three).
//!
//! Any one of the many systems that write creature transform state
//! (combat knockback, bending-ability launches, dragon flight/breath
//! impulses, teleport-on-respawn, ...) producing one NaN or a runaway
//! velocity integrating into a huge position is enough: that value goes
//! straight into a vertex buffer with no packing clamp to absorb it —
//! and a NaN in clip-space position after the MVP transform is a
//! textbook cause of the "triangles filling the screen" artifact,
//! since rasterizer edge functions are undefined for NaN inputs.
//!
//! This module is the missing guard, applied at the render boundary
//! (where creature meshes are about to be concatenated into the
//! frame's upload buffer) rather than chasing every upstream writer —
//! same "gate at the boundary" shape as the packed-vertex clamp, and
//! the same "drop and log rather than upload garbage" shape as
//! `mesh_buffer_pool`'s exhaustion handling.
//!
//! # Design: drop triangles, not creatures
//!
//! A creature's boxes are independent (each box's quad is a closed,
//! self-contained set of 4 vertices / 2 triangles that no other box's
//! triangles reference — see `push_box_oriented`). So the least
//! destructive fix is per-triangle: drop only the triangle(s) touching
//! a non-finite vertex, keep the rest of the creature intact. A dragon
//! with one NaN wing-tip vertex loses that wing-tip's triangles, not
//! its whole body.

/// Minimal shape this module needs from a vertex — kept generic so it
/// doesn't need to depend on `world::entity::EntityVertex` directly
/// (avoids a circular/heavy dependency for this small pure module; the
/// real integration just passes `.position` through).
pub trait HasPosition {
    fn position(&self) -> [f32; 3];
}

#[inline]
pub fn position_is_finite(p: [f32; 3]) -> bool {
    p[0].is_finite() && p[1].is_finite() && p[2].is_finite()
}

/// Result of a sanitize pass. `dropped_triangles == 0` on the (expected,
/// overwhelmingly common) clean path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SanitizeReport {
    pub dropped_triangles: usize,
    pub dropped_vertices: usize,
}

/// Drop any triangle that references a non-finite vertex position, and
/// drop any vertex left completely unreferenced as a result. Returns
/// `None` on the fast path (everything already finite) so the caller
/// can skip reallocating — the overwhelmingly common case, checked with
/// one linear scan and no allocation.
///
/// `indices` must be a flat triangle list (`len() % 3 == 0`); any
/// trailing partial triangle is dropped defensively (can't be a valid
/// triangle anyway) rather than causing an out-of-bounds read. Any
/// index `>= verts.len()` makes its triangle invalid the same way a
/// non-finite vertex does — defense against a garbage index reaching
/// this far, not just a garbage position.
pub fn sanitize<V: HasPosition + Clone>(
    verts: &[V],
    indices: &[u32],
) -> Option<(Vec<V>, Vec<u32>, SanitizeReport)> {
    let finite: Vec<bool> = verts.iter().map(|v| position_is_finite(v.position())).collect();
    if finite.iter().all(|&f| f) {
        return None; // fast path: nothing to do
    }

    let mut remap = vec![u32::MAX; verts.len()];
    let mut new_verts: Vec<V> = Vec::with_capacity(verts.len());
    let mut used = vec![false; verts.len()];

    let mut new_indices = Vec::with_capacity(indices.len());
    let mut dropped_triangles = 0usize;
    for tri in indices.chunks(3) {
        if tri.len() < 3 {
            dropped_triangles += 1; // malformed trailing partial triangle
            continue;
        }
        let ok = tri.iter().all(|&idx| {
            let i = idx as usize;
            i < verts.len() && finite[i]
        });
        if !ok {
            dropped_triangles += 1;
            continue;
        }
        for &idx in tri {
            used[idx as usize] = true;
        }
        new_indices.push(tri[0]);
        new_indices.push(tri[1]);
        new_indices.push(tri[2]);
    }

    for (i, v) in verts.iter().enumerate() {
        if used[i] {
            remap[i] = new_verts.len() as u32;
            new_verts.push(v.clone());
        }
    }
    // Remap indices to the compacted vertex array.
    for idx in new_indices.iter_mut() {
        *idx = remap[*idx as usize];
    }

    let dropped_vertices = verts.len() - new_verts.len();
    Some((new_verts, new_indices, SanitizeReport { dropped_triangles, dropped_vertices }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct V(f32, f32, f32);
    impl HasPosition for V {
        fn position(&self) -> [f32; 3] { [self.0, self.1, self.2] }
    }

    fn quad(x: f32) -> (Vec<V>, Vec<u32>) {
        // one quad = 4 verts, 2 triangles (matches push_box_oriented's
        // per-face shape closely enough for this test's purposes)
        (
            vec![V(x, 0.0, 0.0), V(x, 1.0, 0.0), V(x, 1.0, 1.0), V(x, 0.0, 1.0)],
            vec![0, 1, 2, 0, 2, 3],
        )
    }

    #[test]
    fn clean_mesh_takes_the_fast_path() {
        let (v, i) = quad(5.0);
        assert!(sanitize(&v, &i).is_none());
    }

    #[test]
    fn nan_position_drops_only_its_triangles() {
        let (mut v, i) = quad(5.0);
        v[1].0 = f32::NAN; // corrupt one vertex of the quad
        let (nv, ni, report) = sanitize(&v, &i).expect("must not be fast-path");
        // Quad indices are a fan: (0,1,2) and (0,2,3). Vertex 1 belongs
        // ONLY to the first triangle — the sanitizer is precise enough
        // to drop just that one, not the whole quad (this is the
        // "surgical, not per-creature" property the module is for).
        assert_eq!(report.dropped_triangles, 1, "only the triangle actually touching the bad vertex should drop");
        assert_eq!(ni.len(), 3, "the untouched second triangle (0,2,3) survives");
        assert!(nv.iter().all(|v| position_is_finite(v.position())));
    }

    #[test]
    fn inf_position_is_caught_same_as_nan() {
        let (mut v, i) = quad(5.0);
        v[0].1 = f32::INFINITY;
        let (_, _, report) = sanitize(&v, &i).unwrap();
        assert_eq!(report.dropped_triangles, 2);
        v[0].1 = f32::NEG_INFINITY;
        let (_, _, report2) = sanitize(&v, &i).unwrap();
        assert_eq!(report2.dropped_triangles, 2);
    }

    #[test]
    fn independent_boxes_one_bad_one_clean_only_drops_the_bad_one() {
        // Two independent quads (like two boxes on a creature): first
        // is corrupted, second is clean. Only the first's triangles
        // should drop; the second renders untouched.
        let (v1, i1) = quad(1.0);
        let (mut v2, i2) = quad(2.0);
        v2[2].2 = f32::NAN;

        let mut verts = v1.clone();
        verts.extend(v2.clone());
        let mut indices = i1.clone();
        indices.extend(i2.iter().map(|&x| x + 4)); // offset into the second quad's verts

        let (nv, ni, report) = sanitize(&verts, &indices).unwrap();
        assert_eq!(report.dropped_triangles, 2, "only quad 2's triangles should drop");
        assert_eq!(ni.len(), 6, "quad 1's 2 triangles (6 indices) survive");
        assert_eq!(nv.len(), 4, "only quad 1's 4 verts survive (quad 2's are unreferenced -> dropped)");
        for v in &nv {
            assert!(position_is_finite(v.position()));
        }
    }

    #[test]
    fn remapped_indices_still_point_at_the_right_vertices() {
        let (v1, i1) = quad(1.0);
        let (mut v2, i2) = quad(2.0);
        v2[0].0 = f32::NAN; // corrupt first vertex of second quad

        let mut verts = v1.clone();
        verts.extend(v2.clone());
        let mut indices = i1.clone();
        indices.extend(i2.iter().map(|&x| x + 4));

        let (nv, ni, _report) = sanitize(&verts, &indices).unwrap();
        // Every surviving index must be in-bounds and point at a vertex
        // whose position matches what it originally referenced (modulo
        // compaction) — catches an off-by-one in the remap table.
        for &idx in &ni {
            assert!((idx as usize) < nv.len(), "index out of bounds after remap");
        }
        // The 4 survivors should all be quad 1's original verts (x==1.0).
        assert!(nv.iter().all(|v| v.position()[0] == 1.0));
    }

    #[test]
    fn malformed_trailing_partial_triangle_is_dropped_not_oob() {
        // Combine a real NaN (forces the slow path) with a malformed
        // trailing partial triangle (4 indices, not a multiple of 3) to
        // exercise the `tri.len() < 3` branch — a length-only defect
        // with no NaN present would never reach the index walk at all,
        // since the fast path only checks vertex finiteness (by design:
        // this module sanitizes positions, not index-list shape; a
        // clean-vertex malformed-index mesh is a different bug class).
        let (mut v, _) = quad(5.0);
        v[0].0 = f32::NAN;
        let bad_indices = vec![0u32, 1, 2, 3]; // second "triangle" is short
        let (_, ni, _report) = sanitize(&v, &bad_indices).expect("NaN forces the slow path");
        assert!(ni.len() % 3 == 0, "no partial triangle should ever reach the output");
    }

    #[test]
    fn all_nan_mesh_produces_fully_empty_result() {
        let v = vec![V(f32::NAN, f32::NAN, f32::NAN); 4];
        let i = vec![0u32, 1, 2, 0, 2, 3];
        let (nv, ni, report) = sanitize(&v, &i).unwrap();
        assert!(nv.is_empty());
        assert!(ni.is_empty());
        assert_eq!(report.dropped_triangles, 2);
        assert_eq!(report.dropped_vertices, 4);
    }
}
