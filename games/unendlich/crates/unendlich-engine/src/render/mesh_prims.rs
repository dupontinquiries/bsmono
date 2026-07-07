//! `render::mesh_prims` — canonical cube-face mesh primitives (WID `r340-sl-mesh`).
//!
//! Shared-library extraction for the r340 candidate hunt. The six axis-aligned
//! face directions, their outward normals, their neighbour offsets, and the
//! per-face quad winding are currently re-declared in at least three places with
//! **different orderings and types**:
//!
//!   - `render/mesher.rs`  — `FACE_NORMALS: [[f32;3];6]` @736, `FACE_OFFSETS:
//!     [(i32,i32,i32);6]` @784, `FACE_VERTS: [[[f32;3];4];6]` @762, `FACE_LIGHT`
//!     @726, and `mod face { TOP=0,BOTTOM=1,EAST=2,WEST=3,SOUTH=4,NORTH=5 }`.
//!   - `world/voxel_raycast.rs` — `enum Face { None, PosX, NegX, PosY, NegY,
//!     PosZ, NegZ }` + `face_normal(Face)->(i32,i32,i32)` @41 (a DIFFERENT order,
//!     plus a `None`).
//!   - `world/flora_sites.rs` — `const FACES: [(i8,i8,i8);6]` @21 in order
//!     `+X,-X,+Y,-Y,+Z,-Z` (a THIRD order).
//!
//! This module makes `mesher`'s index order canonical (it is the GPU/vertex-pack
//! load-bearing one) and offers **direction-keyed** bridges so the other sites
//! consolidate WITHOUT renumbering any index a buffer/wire format depends on.
//!
//! RESPECT (per the r340 constraints): this is the cube-face primitive the voxel
//! mesher legitimately uses; it does NOT model the world as a 6-face cube grid.
//! Sub-voxel geometry and the r332 directional sub-mesh scheme (`mesh_partition`,
//! which buckets greedy quads by face normal) keep their own logic — they would
//! consume `face_from_offset`/`FACE_NORMALS` here rather than re-deriving them.
//! Pure, deterministic, panic-free (no IO/RNG/clock, no `unwrap`/`expect`).

/// Canonical face indices — `mesher`'s order. Buffer/vertex-pack code depends on
/// these exact values; treat them as the wire order for faces.
pub mod face {
    pub const TOP: usize = 0; // +Y
    pub const BOTTOM: usize = 1; // -Y
    pub const EAST: usize = 2; // +X
    pub const WEST: usize = 3; // -X
    pub const SOUTH: usize = 4; // +Z
    pub const NORTH: usize = 5; // -Z
}

/// Outward unit normal per face (canonical order). Mirror of `mesher::FACE_NORMALS`.
pub const FACE_NORMALS: [[f32; 3]; 6] = [
    [0.0, 1.0, 0.0],  // TOP    +Y
    [0.0, -1.0, 0.0], // BOTTOM -Y
    [1.0, 0.0, 0.0],  // EAST   +X
    [-1.0, 0.0, 0.0], // WEST   -X
    [0.0, 0.0, 1.0],  // SOUTH  +Z
    [0.0, 0.0, -1.0], // NORTH  -Z
];

/// Integer neighbour offset per face (canonical order). Mirror of
/// `mesher::FACE_OFFSETS`; equals `sign(FACE_NORMALS)`.
pub const FACE_OFFSETS: [[i32; 3]; 6] = [
    [0, 1, 0],
    [0, -1, 0],
    [1, 0, 0],
    [-1, 0, 0],
    [0, 0, 1],
    [0, 0, -1],
];

/// Per-face quad corners on the unit cube (canonical order), counter-clockwise
/// viewed from outside; tessellate as triangles (0,1,2),(0,2,3). Mirror of
/// `mesher::FACE_VERTS`.
pub const FACE_VERTS: [[[f32; 3]; 4]; 6] = [
    [[0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0]], // TOP
    [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 0.0, 0.0]], // BOTTOM
    [[1.0, 0.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 0.0], [1.0, 0.0, 0.0]], // EAST
    [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 1.0], [0.0, 0.0, 1.0]], // WEST
    [[0.0, 0.0, 1.0], [0.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 0.0, 1.0]], // SOUTH
    [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]], // NORTH
];

/// Default per-face brightness (canonical order). Mirror of `mesher::FACE_LIGHT`
/// — the no-sky fallback shading the mesher uses; kept here as the canonical
/// table so offline tools and the sky-aware path agree.
pub const FACE_LIGHT: [f32; 6] = [1.00, 0.55, 0.80, 0.80, 0.65, 0.65];

/// Outward normal for a canonical face index, or `[0;3]` if out of range.
#[inline]
pub fn normal_f32(face: usize) -> [f32; 3] {
    if face < 6 { FACE_NORMALS[face] } else { [0.0; 3] }
}

/// Integer neighbour offset for a canonical face index, or `[0;3]` if out of range.
#[inline]
pub fn offset_i32(face: usize) -> [i32; 3] {
    if face < 6 { FACE_OFFSETS[face] } else { [0; 3] }
}

/// `i8` neighbour offset for a canonical face index (the `flora_sites` shape),
/// or `[0;3]` if out of range.
#[inline]
pub fn offset_i8(face: usize) -> [i8; 3] {
    let o = offset_i32(face);
    [o[0] as i8, o[1] as i8, o[2] as i8]
}

/// Map any unit axis direction to its canonical face index. This is the bridge
/// that lets the differently-ordered duplicates (`voxel_raycast::Face`,
/// `flora_sites::FACES`) share this one table WITHOUT renumbering: each site
/// keeps reading its own direction vector; this resolves it to the canonical
/// index. Returns `None` for the zero vector or any non-unit / diagonal input.
#[inline]
pub fn face_from_offset(off: [i32; 3]) -> Option<usize> {
    FACE_OFFSETS.iter().position(|&o| o == off)
}

/// The four world-space corners of a greedy quad on `face`, whose merged rect
/// starts at `origin` and spans `du`×`dv` along the face's two in-plane axes.
/// (This is the per-quad emission the greedy mesher does; pulled here so the
/// mesher, `mesh_partition`, and offline tools emit identical geometry.) The
/// unit `FACE_VERTS` are scaled along their varying axes by `du`/`dv`.
pub fn quad_corners(face: usize, origin: [f32; 3], du: f32, dv: f32) -> [[f32; 3]; 4] {
    if face >= 6 {
        return [[0.0; 3]; 4];
    }
    let base = FACE_VERTS[face];
    // The face's constant axis is where |normal| == 1; the other two vary 0..1.
    let n = FACE_NORMALS[face];
    let const_axis = if n[0] != 0.0 { 0 } else if n[1] != 0.0 { 1 } else { 2 };
    // pick scale per axis: constant axis unscaled, the two in-plane axes scaled.
    // assign du to the lower-index in-plane axis, dv to the higher one.
    let in_axes: [usize; 2] = match const_axis {
        0 => [1, 2],
        1 => [0, 2],
        _ => [0, 1],
    };
    let mut out = [[0.0f32; 3]; 4];
    for (i, c) in base.iter().enumerate() {
        let mut p = *c;
        p[in_axes[0]] = origin[in_axes[0]] + c[in_axes[0]] * du;
        p[in_axes[1]] = origin[in_axes[1]] + c[in_axes[1]] * dv;
        p[const_axis] = origin[const_axis] + c[const_axis];
        out[i] = p;
    }
    out
}

/// Axis-aligned bounds `[min, max]` of a quad's four corners.
pub fn quad_aabb(corners: &[[f32; 3]; 4]) -> [[f32; 3]; 2] {
    let mut lo = corners[0];
    let mut hi = corners[0];
    for c in &corners[1..] {
        for k in 0..3 {
            if c[k] < lo[k] { lo[k] = c[k]; }
            if c[k] > hi[k] { hi[k] = c[k]; }
        }
    }
    [lo, hi]
}

/// A greedy quad to emit: face index + world origin + in-plane span.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuadSpec {
    pub face: usize,
    pub origin: [f32; 3],
    pub du: f32,
    pub dv: f32,
}

/// Serial: AABB of every quad. The per-item kernel `quad_corners`→`quad_aabb` is
/// a pure `Fn(&QuadSpec)->_` with no shared state — see the parallel twin.
pub fn quad_aabbs_serial(quads: &[QuadSpec]) -> Vec<[[f32; 3]; 2]> {
    quads.iter().map(quad_aabb_of).collect()
}

#[inline]
fn quad_aabb_of(q: &QuadSpec) -> [[f32; 3]; 2] {
    quad_aabb(&quad_corners(q.face, q.origin, q.du, q.dv))
}

/// PERF BONUS (r340 option a): the same per-quad AABB pass, data-parallel.
///
/// On the real base this routes through `util::task_lanes::map_pure_threshold(
/// quads, MIN_PARALLEL, quad_aabb_of)` — the kernel is `Send + Sync`, reads only
/// its `&QuadSpec`, holds no shared mutable state, and writes an indexed result,
/// so rayon's index-preserving parallel map yields bit-identical output to the
/// serial path. In-sandbox we avoid pulling the `rayon` dep (SHARED_RULES §5) and
/// prove the SAME parity property with a `std::thread::scope` chunked map; the
/// `serial_equals_parallel` test asserts it is bit-identical to `*_serial`.
pub fn quad_aabbs_parallel(quads: &[QuadSpec], min_parallel: usize) -> Vec<[[f32; 3]; 2]> {
    if quads.len() < min_parallel.max(1) {
        return quad_aabbs_serial(quads);
    }
    let lanes = 4usize;
    let chunk = quads.len().div_ceil(lanes);
    let mut out: Vec<[[f32; 3]; 2]> = vec![[[0.0; 3]; 2]; quads.len()];
    std::thread::scope(|s| {
        let mut rest = &mut out[..];
        let mut handles = Vec::new();
        let mut idx = 0;
        while idx < quads.len() {
            let end = (idx + chunk).min(quads.len());
            let (head, tail) = rest.split_at_mut(end - idx);
            rest = tail;
            let slice = &quads[idx..end];
            handles.push(s.spawn(move || {
                for (o, q) in head.iter_mut().zip(slice.iter()) {
                    *o = quad_aabb_of(q);
                }
            }));
            idx = end;
        }
        for h in handles {
            // join errors only on a panicked kernel; our kernel never panics.
            let _ = h.join();
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
    }

    #[test]
    fn normals_are_unit_axis_aligned() {
        for (i, n) in FACE_NORMALS.iter().enumerate() {
            let nonzero = n.iter().filter(|&&v| v != 0.0).count();
            assert_eq!(nonzero, 1, "face {i} normal not axis-aligned");
            assert!((dot(*n, *n) - 1.0).abs() < 1e-6, "face {i} normal not unit");
        }
    }

    #[test]
    fn offsets_are_sign_of_normals() {
        for i in 0..6 {
            for k in 0..3 {
                assert_eq!(FACE_OFFSETS[i][k] as f32, FACE_NORMALS[i][k], "face {i} axis {k}");
            }
        }
    }

    #[test]
    fn face_verts_lie_on_face_plane_and_span_unit_square() {
        for f in 0..6 {
            let n = FACE_NORMALS[f];
            let const_axis = (0..3).find(|&k| n[k] != 0.0).unwrap_or(0);
            let plane = if n[const_axis] > 0.0 { 1.0 } else { 0.0 };
            let in_axes: Vec<usize> = (0..3).filter(|&k| k != const_axis).collect();
            for (mut lo, mut hi) in [([f32::MAX; 2], [f32::MIN; 2])] {
                for c in &FACE_VERTS[f] {
                    assert!((c[const_axis] - plane).abs() < 1e-6, "face {f} corner off-plane");
                    for (j, &ax) in in_axes.iter().enumerate() {
                        lo[j] = lo[j].min(c[ax]);
                        hi[j] = hi[j].max(c[ax]);
                    }
                }
                // the quad covers the full unit square in its two in-plane axes.
                assert_eq!((lo[0], hi[0], lo[1], hi[1]), (0.0, 1.0, 0.0, 1.0), "face {f} not unit square");
            }
            // and the winding is perpendicular to the face normal (colinear geo-normal).
            let v = &FACE_VERTS[f];
            let g = cross([v[1][0]-v[0][0], v[1][1]-v[0][1], v[1][2]-v[0][2]],
                          [v[2][0]-v[0][0], v[2][1]-v[0][1], v[2][2]-v[0][2]]);
            let gl = dot(g, g).sqrt().max(1e-9);
            let gn = [g[0]/gl, g[1]/gl, g[2]/gl];
            assert!((dot(gn, n).abs() - 1.0).abs() < 1e-5, "face {f} quad not perpendicular to its normal");
        }
    }

    #[test]
    fn face_from_offset_bridges_all_orderings() {
        // canonical round-trip
        for f in 0..6 {
            assert_eq!(face_from_offset(FACE_OFFSETS[f]), Some(f));
        }
        // voxel_raycast::Face order (PosX,NegX,PosY,NegY,PosZ,NegZ) maps onto canonical
        let raycast = [[1,0,0],[-1,0,0],[0,1,0],[0,-1,0],[0,0,1],[0,0,-1]];
        let expect = [face::EAST, face::WEST, face::TOP, face::BOTTOM, face::SOUTH, face::NORTH];
        for (dir, &e) in raycast.iter().zip(expect.iter()) {
            assert_eq!(face_from_offset(*dir), Some(e), "raycast dir {dir:?} should map to {e}");
        }
        // flora_sites FACES order (+X,-X,+Y,-Y,+Z,-Z) also resolves
        let flora = [[1,0,0],[-1,0,0],[0,1,0],[0,-1,0],[0,0,1],[0,0,-1]];
        for dir in &flora {
            assert!(face_from_offset(*dir).is_some(), "flora dir {dir:?} unresolved");
        }
        // non-unit / diagonal / zero → None
        assert_eq!(face_from_offset([0, 0, 0]), None);
        assert_eq!(face_from_offset([1, 1, 0]), None);
        assert_eq!(face_from_offset([2, 0, 0]), None);
    }

    #[test]
    fn quad_corners_match_face_plane_and_scale() {
        // a 3x2 greedy quad on TOP at origin (5,8,9): all corners at y=9 (8+1),
        // spanning x:[5,8], z:[9,11].
        let c = quad_corners(face::TOP, [5.0, 8.0, 9.0], 3.0, 2.0);
        for p in &c { assert!((p[1] - 9.0).abs() < 1e-6, "TOP quad off its +Y plane"); }
        let ab = quad_aabb(&c);
        assert_eq!(ab[0], [5.0, 9.0, 9.0]);
        assert_eq!(ab[1], [8.0, 9.0, 11.0]);
        // out-of-range face → degenerate, not a panic
        assert_eq!(quad_corners(99, [0.0; 3], 1.0, 1.0), [[0.0; 3]; 4]);
    }

    fn sample_quads(n: usize) -> Vec<QuadSpec> {
        let mut seed = 0x51A0_u32;
        let mut next = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); seed };
        (0..n).map(|_| QuadSpec {
            face: (next() % 6) as usize,
            origin: [(next() % 64) as f32, (next() % 64) as f32, (next() % 64) as f32],
            du: 1.0 + (next() % 8) as f32,
            dv: 1.0 + (next() % 8) as f32,
        }).collect()
    }

    #[test]
    fn serial_equals_parallel() {
        // PERF BONUS parity: the data-parallel quad-AABB pass is bit-identical to
        // the serial path (the property that lets it route through task_lanes).
        for n in [0usize, 1, 7, 64, 1000] {
            let quads = sample_quads(n);
            let s = quad_aabbs_serial(&quads);
            let p = quad_aabbs_parallel(&quads, 16);
            assert_eq!(s.len(), p.len());
            for i in 0..s.len() {
                assert_eq!(s[i][0].map(f32::to_bits), p[i][0].map(f32::to_bits), "quad {i} min differs");
                assert_eq!(s[i][1].map(f32::to_bits), p[i][1].map(f32::to_bits), "quad {i} max differs");
            }
        }
    }

    #[test]
    fn deterministic() {
        let quads = sample_quads(200);
        assert_eq!(quad_aabbs_serial(&quads), quad_aabbs_serial(&quads));
        assert_eq!(quad_aabbs_parallel(&quads, 16), quad_aabbs_parallel(&quads, 16));
    }
}
