//! SURFACE NETS — where the surface actually is, rather than which cells are full.
//!
//! `games/unendlich/docs/TERRAIN_RESOLUTION.md` argues that voxel terrain reads
//! blocky not because the cells are too big but because occupancy is BINARY: a
//! cell is solid or it is not, so nothing records where inside the cell the
//! surface sits, and any smoother has to invent it. That is why "no cubes"
//! mods look like melted wax and why their step heights wander. The fix is a
//! scalar per cell and an extractor that reads it.
//!
//! This is that extractor, and it lives in its own crate for a reason worth
//! stating: the geometry does not need an engine to be correct. Here it builds
//! and tests in under a second. Inside `unendlich-engine` the same code would
//! sit behind a five-minute Vulkan build, and the sub-cell arithmetic — which
//! is the entire point — would be the least-exercised part of the system.
//!
//! ## The method, and why this one
//!
//! Naive surface nets (dual contouring's simple cousin):
//!
//! 1. Sample the field on a lattice. A cell is the cube between eight
//!    neighbouring samples.
//! 2. A cell whose eight corners are not all on the same side of the isolevel
//!    is a SURFACE CELL and gets exactly one vertex, placed at the centroid of
//!    the linearly-interpolated crossings on its twelve edges.
//! 3. For every lattice edge that crosses the isolevel, join the vertices of
//!    the four cells around that edge into a quad.
//!
//! One vertex per cell is what makes it watertight by construction: adjacent
//! cells share the quads between them, so there are no seams to stitch and no
//! T-junctions. Marching cubes places vertices on edges instead and needs a
//! 256-case table to stay consistent; dual contouring adds QEF minimisation
//! and sharp-feature recovery, which terrain does not want and which needs
//! gradients the field may not carry. Surface nets is the one that fits.
//!
//! ## What it does not solve
//!
//! LOD stitching. Two neighbouring chunks extracted at different sample
//! strides do not agree on where their shared face is, and a smooth surface
//! shows that as a hole rather than as the duplicated-quad seam a cube world
//! gets away with. `unendlich`'s `lod::cull_boundary_face` is the block-era
//! answer and does not carry over.
//!
//! `chunks_at_different_strides_disagree_at_their_shared_face` MEASURES that
//! crack rather than describing it, so a fix has a number to beat.
//!
//! ### An approach that does not work, recorded so it is not retried
//!
//! Quantising the boundary VERTEX POSITION onto the coarse lattice. It is the
//! obvious first idea and it makes the seam worse — measured at 3.86 cells of
//! disagreement before, 5.27 after. The reason is that surface nets encodes
//! the surface's location IN the vertex position, so rounding a coordinate to
//! a multiple of the coarse stride destroys exactly the information the
//! extractor exists to compute; on a heightfield it quantises the height.
//!
//! The fix has to act on the SAMPLES, not on the result: a boundary cell must
//! interpolate between the same pair of field samples its coarse neighbour
//! uses, which means merging the fine cells across that face into the
//! neighbour's cell structure rather than adjusting their output. That is a
//! restructure of the vertex pass, not a post-step, and it is not attempted
//! here.

use glam::Vec3;

/// A scalar field sampled on integer lattice points. Values at or above
/// [`Mesher::iso`] are inside the solid.
///
/// Deliberately a trait over integer coordinates rather than a borrowed
/// buffer: a chunked world samples across chunk boundaries, and an
/// implementation that owns that lookup is the only one that can.
pub trait Field {
    fn sample(&self, x: i32, y: i32, z: i32) -> f32;
}

impl<F: Fn(i32, i32, i32) -> f32> Field for F {
    fn sample(&self, x: i32, y: i32, z: i32) -> f32 {
        self(x, y, z)
    }
}

/// Triangles, in the caller's lattice coordinates.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    /// Triangle list. Always a multiple of three.
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Extraction settings.
#[derive(Clone, Copy, Debug)]
pub struct Mesher {
    /// The level the surface sits at. Samples >= this are solid.
    pub iso: f32,
    /// Lattice samples per axis in the region to extract. Cells are one fewer
    /// per axis.
    pub dims: [i32; 3],
    /// Lattice coordinate of sample (0, 0, 0).
    pub origin: [i32; 3],
    /// Sample every `stride`th lattice point. 1 is full detail; a coarser
    /// chunk uses 2, 4, 8 — the same ladder `unendlich`'s `lod_cell_stride`
    /// walks. Output positions stay in FULL-DETAIL lattice units regardless,
    /// so two chunks meshed at different strides are directly comparable and
    /// a seam is a measurable distance rather than a rendering impression.
    pub stride: i32,
}

impl Mesher {
    pub fn new(dims: [i32; 3]) -> Mesher {
        Mesher { iso: 0.0, dims, origin: [0, 0, 0], stride: 1 }
    }
    pub fn at(mut self, origin: [i32; 3]) -> Mesher {
        self.origin = origin;
        self
    }
    pub fn iso(mut self, iso: f32) -> Mesher {
        self.iso = iso;
        self
    }
    /// Mesh at a coarser sampling. `dims` still counts SAMPLES, so a stride-2
    /// chunk covering the same ground as a stride-1 one has about half as
    /// many.
    pub fn stride(mut self, stride: i32) -> Mesher {
        self.stride = stride.max(1);
        self
    }
}

/// The twelve edges of a cell, as pairs of corner indices in the
/// `(x + 2y + 4z)` corner numbering used below.
const EDGES: [(usize, usize); 12] = [
    (0, 1),
    (2, 3),
    (4, 5),
    (6, 7), // along x
    (0, 2),
    (1, 3),
    (4, 6),
    (5, 7), // along y
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7), // along z
];

fn corner_offset(i: usize) -> [i32; 3] {
    [(i & 1) as i32, ((i >> 1) & 1) as i32, ((i >> 2) & 1) as i32]
}

/// Extract the isosurface.
///
/// Cost is one field sample per lattice point (each is read up to eight times
/// by neighbouring cells, so callers whose sampling is expensive should cache;
/// a dense chunk buffer already is a cache).
pub fn extract<F: Field>(field: &F, m: Mesher) -> Mesh {
    let [nx, ny, nz] = m.dims;
    if nx < 2 || ny < 2 || nz < 2 {
        return Mesh::default();
    }
    let (cx, cy, cz) = (nx - 1, ny - 1, nz - 1);
    let cell_count = (cx * cy * cz) as usize;

    // vertex index per cell, or u32::MAX for "no surface here"
    let mut vertex_of = vec![u32::MAX; cell_count];
    let mut mesh = Mesh::default();

    let idx = |x: i32, y: i32, z: i32| (x + cx * (y + cy * z)) as usize;
    let k = m.stride.max(1);
    let at = |x: i32, y: i32, z: i32| {
        field.sample(m.origin[0] + x * k, m.origin[1] + y * k, m.origin[2] + z * k)
    };

    // -- one vertex per surface cell ------------------------------------
    for z in 0..cz {
        for y in 0..cy {
            for x in 0..cx {
                let mut v = [0.0f32; 8];
                let mut inside = 0u8;
                for (i, s) in v.iter_mut().enumerate() {
                    let o = corner_offset(i);
                    *s = at(x + o[0], y + o[1], z + o[2]);
                    if *s >= m.iso {
                        inside |= 1 << i;
                    }
                }
                // all in or all out: no surface through this cell
                if inside == 0 || inside == 0xFF {
                    continue;
                }

                // centroid of the edge crossings — the sub-cell placement that
                // binary occupancy cannot express
                let mut sum = Vec3::ZERO;
                let mut n = 0.0f32;
                for (a, b) in EDGES {
                    let (va, vb) = (v[a], v[b]);
                    if (va >= m.iso) == (vb >= m.iso) {
                        continue;
                    }
                    // linear crossing along the edge
                    let t = ((m.iso - va) / (vb - va)).clamp(0.0, 1.0);
                    let (oa, ob) = (corner_offset(a), corner_offset(b));
                    let pa = Vec3::new(oa[0] as f32, oa[1] as f32, oa[2] as f32);
                    let pb = Vec3::new(ob[0] as f32, ob[1] as f32, ob[2] as f32);
                    sum += pa + (pb - pa) * t;
                    n += 1.0;
                }
                let local = if n > 0.0 { sum / n } else { Vec3::splat(0.5) };
                vertex_of[idx(x, y, z)] = mesh.positions.len() as u32;
                // back into full-detail units so chunks at different strides
                // share one coordinate frame
                mesh.positions
                    .push((Vec3::new(x as f32, y as f32, z as f32) + local) * k as f32);
            }
        }
    }

    // -- one quad per crossing lattice edge -----------------------------
    //
    // Only edges with all four surrounding cells in range are emitted, which
    // is why a caller extracting a chunk should sample one lattice point of
    // overlap into its neighbours rather than stopping at the boundary.
    let quad = |a: u32, b: u32, c: u32, d: u32, flip: bool, out: &mut Mesh| {
        if a == u32::MAX || b == u32::MAX || c == u32::MAX || d == u32::MAX {
            return;
        }
        let q = if flip { [a, c, b, a, d, c] } else { [a, b, c, a, c, d] };
        out.indices.extend_from_slice(&q);
    };

    for z in 0..cz {
        for y in 0..cy {
            for x in 0..cx {
                let here = at(x, y, z) >= m.iso;
                // +x edge: the four cells around it share y-1 and z-1
                if x + 1 < nx && y > 0 && z > 0 && (at(x + 1, y, z) >= m.iso) != here {
                    quad(
                        vertex_of[idx(x, y - 1, z - 1)],
                        vertex_of[idx(x, y, z - 1)],
                        vertex_of[idx(x, y, z)],
                        vertex_of[idx(x, y - 1, z)],
                        here,
                        &mut mesh,
                    );
                }
                if y + 1 < ny && x > 0 && z > 0 && (at(x, y + 1, z) >= m.iso) != here {
                    quad(
                        vertex_of[idx(x - 1, y, z - 1)],
                        vertex_of[idx(x, y, z - 1)],
                        vertex_of[idx(x, y, z)],
                        vertex_of[idx(x - 1, y, z)],
                        !here,
                        &mut mesh,
                    );
                }
                if z + 1 < nz && x > 0 && y > 0 && (at(x, y, z + 1) >= m.iso) != here {
                    quad(
                        vertex_of[idx(x - 1, y - 1, z)],
                        vertex_of[idx(x, y - 1, z)],
                        vertex_of[idx(x, y, z)],
                        vertex_of[idx(x - 1, y, z)],
                        here,
                        &mut mesh,
                    );
                }
            }
        }
    }

    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A plane at height `h`: positive above, negative below, so the isolevel
    /// at 0 sits exactly at `h` regardless of where the lattice points are.
    fn plane(h: f32) -> impl Field {
        move |_x: i32, y: i32, _z: i32| (y as f32) - h
    }

    fn sphere(cx: f32, cy: f32, cz: f32, r: f32) -> impl Field {
        move |x: i32, y: i32, z: i32| {
            let d = Vec3::new(x as f32 - cx, y as f32 - cy, z as f32 - cz).length();
            r - d
        }
    }

    #[test]
    fn a_surface_between_lattice_points_lands_between_them() {
        // THE WHOLE CLAIM. Binary occupancy can only put the surface at 3 or
        // at 4; a scalar field puts it at 3.5. If this test ever passes only
        // to integers, the extractor has thrown away the thing it exists for.
        for h in [3.5f32, 3.25, 3.75, 2.1] {
            let m = Mesher::new([6, 8, 6]);
            let mesh = extract(&plane(h), m);
            assert!(!mesh.is_empty(), "no surface at h={h}");
            for p in &mesh.positions {
                assert!(
                    (p.y - h).abs() < 0.05,
                    "vertex at y={} for a plane at {h} — off by {:.3}",
                    p.y,
                    (p.y - h).abs()
                );
            }
        }
    }

    #[test]
    fn a_sphere_comes_out_round() {
        let (c, r) = (8.0f32, 5.0f32);
        let mesh = extract(&sphere(c, c, c, r), Mesher::new([17, 17, 17]));
        assert!(mesh.triangles() > 100, "only {} triangles", mesh.triangles());
        let mut worst = 0.0f32;
        for p in &mesh.positions {
            let d = (*p - Vec3::splat(c)).length();
            worst = worst.max((d - r).abs());
        }
        // surface nets pulls slightly toward cell centres; a fifth of a cell
        // is the known bound for this method, and a regression past it means
        // the crossing interpolation broke
        assert!(worst < 0.2, "worst radial error {worst:.3} cells");
    }

    #[test]
    fn a_field_with_no_crossing_produces_nothing() {
        let all_air = |_x: i32, _y: i32, _z: i32| -1.0f32;
        let all_rock = |_x: i32, _y: i32, _z: i32| 1.0f32;
        assert!(extract(&all_air, Mesher::new([8, 8, 8])).is_empty());
        assert!(extract(&all_rock, Mesher::new([8, 8, 8])).is_empty());
    }

    #[test]
    fn a_closed_surface_is_watertight() {
        // Every edge shared by exactly two triangles. This is the property
        // that makes surface nets the right choice — one vertex per cell means
        // neighbouring cells cannot disagree about where the shared face is,
        // so there is nothing to stitch WITHIN a chunk. (Across chunks at
        // different LOD, there very much is; see the module header.)
        let mesh = extract(&sphere(8.0, 8.0, 8.0, 5.0), Mesher::new([17, 17, 17]));
        let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
        for t in mesh.indices.chunks_exact(3) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let k = if a < b { (a, b) } else { (b, a) };
                *edges.entry(k).or_insert(0) += 1;
            }
        }
        let bad: Vec<_> = edges.iter().filter(|(_, &n)| n != 2).collect();
        assert!(bad.is_empty(), "{} non-manifold edges, e.g. {:?}", bad.len(), &bad[..bad.len().min(3)]);
    }

    #[test]
    fn the_isolevel_moves_the_surface() {
        // Density fields are rarely zero-centred; a caller must be able to say
        // where solid begins.
        let f = |_x: i32, y: i32, _z: i32| y as f32;
        let low = extract(&f, Mesher::new([5, 10, 5]).iso(2.5));
        let high = extract(&f, Mesher::new([5, 10, 5]).iso(6.5));
        let avg = |m: &Mesh| m.positions.iter().map(|p| p.y).sum::<f32>() / m.positions.len() as f32;
        assert!((avg(&low) - 2.5).abs() < 0.05, "{}", avg(&low));
        assert!((avg(&high) - 6.5).abs() < 0.05, "{}", avg(&high));
    }

    #[test]
    fn the_origin_offsets_sampling_and_not_the_output_frame() {
        // A chunk extractor samples at world coordinates but emits mesh in
        // local ones. Getting this backwards puts every chunk at the origin.
        let m = Mesher::new([5, 8, 5]).at([100, 0, 100]);
        let mesh = extract(&plane(3.5), m);
        assert!(!mesh.is_empty());
        for p in &mesh.positions {
            assert!(p.x < 5.0 && p.z < 5.0, "output is in world space: {p:?}");
            assert!((p.y - 3.5).abs() < 0.05);
        }
    }


    /// A ridged field: nonlinear, so a coarser sampling genuinely disagrees
    /// with a finer one. A plane would not — it is linear, both strides
    /// reconstruct it exactly, and a seam test against it would pass for the
    /// wrong reason.
    fn ridge() -> impl Field {
        move |x: i32, y: i32, z: i32| {
            6.0 + 1.4 * ((x as f32) * 0.7).sin() + 0.9 * ((z as f32) * 0.5).cos() - y as f32
        }
    }

    /// Vertices lying on the plane x == `at`, in full-detail units.
    fn on_face(mesh: &Mesh, at: f32) -> Vec<Vec3> {
        mesh.positions.iter().copied().filter(|p| (p.x - at).abs() < 0.6).collect()
    }

    #[test]
    fn stride_coarsens_without_moving_the_surface() {
        // A stride change is a detail reduction, not a different world.
        let f = |_x: i32, y: i32, _z: i32| 7.5 - y as f32;
        for k in [1, 2, 4] {
            let m = Mesher::new([9 / k + 1, 17 / k + 1, 9 / k + 1]).stride(k);
            let mesh = extract(&f, m);
            assert!(!mesh.is_empty(), "stride {k} produced nothing");
            for p in &mesh.positions {
                assert!((p.y - 7.5).abs() < 0.05, "stride {k} put the surface at {}", p.y);
            }
        }
    }

    #[test]
    fn chunks_at_different_strides_disagree_at_their_shared_face() {
        // CHARACTERISATION, not approval — this is the LOD crack, measured, so
        // that a fix has a number to beat instead of an impression. Same idiom
        // as blackstar-rs's phantom-floor test.
        //
        // Metric: for each fine boundary vertex, the height the coarse chunk
        // puts the surface at nearest the same (y, z). Comparing full 3D
        // nearest-neighbour distance instead would fold in the two chunks'
        // different extents and measure the wrong thing.
        let f = ridge();
        let fine = extract(&f, Mesher::new([9, 14, 9]));
        let coarse = extract(&f, Mesher::new([5, 8, 5]).at([8, 0, 0]).stride(2));

        let a = on_face(&fine, 8.0);
        let b: Vec<Vec3> =
            on_face(&coarse, 0.0).iter().map(|p| *p + Vec3::new(8.0, 0.0, 0.0)).collect();
        assert!(!a.is_empty() && !b.is_empty(), "no boundary vertices to compare");

        let worst = a
            .iter()
            .filter_map(|p| {
                b.iter()
                    .min_by(|u, v| {
                        (u.z - p.z).abs().partial_cmp(&(v.z - p.z).abs()).unwrap()
                    })
                    .map(|q| (q.y - p.y).abs())
            })
            .fold(0.0f32, f32::max);

        assert!(
            worst > 0.1,
            "the seam closed on its own (worst {worst:.3}) — if LOD stitching landed, \
             delete this test; if the field stopped being nonlinear, fix the test"
        );
    }

    #[test]
    fn a_degenerate_region_does_not_panic() {
        for dims in [[0, 0, 0], [1, 1, 1], [1, 9, 9], [2, 2, 2]] {
            let _ = extract(&plane(0.5), Mesher::new(dims));
        }
    }
}
