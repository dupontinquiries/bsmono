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
//! ### What does work, partly: [`Seams`]
//!
//! Acting on the SAMPLES instead. A boundary cell facing a coarser neighbour
//! is meshed as though it were the coarse cell containing it — same eight
//! samples, same interpolation, same answer — so every fine cell inside one
//! coarse footprint emits an identical vertex derived from data the neighbour
//! reads too. Detail is lost in one cell layer, which is the right place to
//! lose it: next to a chunk that already threw that detail away.
//!
//! Measured on the fixture in `declaring_the_coarse_neighbour_closes_the_seam`:
//! 1.46 cells of surface disagreement down to 0.14. Not zero, and the
//! remainder is structural rather than a tuning failure — surface nets puts a
//! vertex in each CELL, and two abutting chunks own different cells either
//! side of the shared plane. At a boundary on x = 8 the fine chunk's cell
//! spans 6..8 and the coarse chunk's spans 8..10. Reading the same samples
//! shrinks the error; it cannot make them the same cell.
//!
//! Note the borrowed layer is `r` cells THICK, not one: a coarse cell covers
//! `r` fine cells per axis, and a single-cell layer collapses only part of
//! each footprint.
//!
//! ### And the composition that closes it entirely
//!
//! No new code — the fine chunk meshes one coarse footprint PAST the shared
//! plane. It then owns the same cell the neighbour owns, computes the
//! identical vertex from the identical samples, and emits the quads bridging
//! the boundary because all four cells around those edges are finally in
//! range. The neighbour skips its first layer, or emits a coincident
//! duplicate; either is harmless.
//!
//! `overlapping_into_the_neighbour_closes_the_seam_completely` holds every
//! coarse vertex to within 1e-3 of one the fine chunk also produced. This is
//! the recipe a chunk scheduler should follow, and it is why the remaining
//! work is scheduling rather than geometry.

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
    /// Outward unit normal per position, from the ANALYTIC GRADIENT of the
    /// trilinear interpolant inside each cell — not from averaging adjacent
    /// triangle normals.
    ///
    /// Face averaging is the usual approach and it is the wrong one here. It
    /// derives the normal from the mesh, which is a lossy reconstruction of
    /// the field, so it inherits the reconstruction's error and adds the
    /// topology's: a vertex with an uneven fan of triangles gets a biased
    /// normal, and the bias moves when the LOD tier changes. The field knows
    /// the true surface direction at every point and costs eight values this
    /// loop has already read.
    ///
    /// It also matters for seams. A boundary cell borrowing a coarse grid
    /// computes its gradient from the coarse corners, so both sides of an LOD
    /// boundary derive the SAME normal — shading matches across the join for
    /// the same reason the geometry does. Averaged normals would visibly
    /// crease there even with the vertices in exactly the right place.
    pub normals: Vec<Vec3>,
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

/// The neighbours' sampling strides, per face, in the order
/// `[-x, +x, -y, +y, -z, +z]`. `0` means unknown or equal — no seam to manage.
/// A value larger than this chunk's own stride means the neighbour is coarser.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Seams(pub [i32; 6]);

impl Seams {
    pub const NONE: Seams = Seams([0; 6]);
    pub fn all(stride: i32) -> Seams {
        Seams([stride; 6])
    }
    pub fn face(mut self, face: usize, stride: i32) -> Seams {
        self.0[face] = stride;
        self
    }
    pub fn x_plus(self, stride: i32) -> Seams {
        self.face(1, stride)
    }
    pub fn x_minus(self, stride: i32) -> Seams {
        self.face(0, stride)
    }
}

/// Faces, for [`Seams::face`].
pub const FACE_X_MINUS: usize = 0;
pub const FACE_X_PLUS: usize = 1;
pub const FACE_Y_MINUS: usize = 2;
pub const FACE_Y_PLUS: usize = 3;
pub const FACE_Z_MINUS: usize = 4;
pub const FACE_Z_PLUS: usize = 5;

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
    /// What the neighbours are doing. Boundary cells facing a coarser chunk
    /// are meshed on the NEIGHBOUR'S cell grid, so both sides interpolate the
    /// same samples and land on the same point.
    pub seams: Seams,
    /// Sample every `stride`th lattice point. 1 is full detail; a coarser
    /// chunk uses 2, 4, 8 — the same ladder `unendlich`'s `lod_cell_stride`
    /// walks. Output positions stay in FULL-DETAIL lattice units regardless,
    /// so two chunks meshed at different strides are directly comparable and
    /// a seam is a measurable distance rather than a rendering impression.
    pub stride: i32,
}

impl Mesher {
    pub fn new(dims: [i32; 3]) -> Mesher {
        Mesher { iso: 0.0, dims, origin: [0, 0, 0], stride: 1, seams: Seams::NONE }
    }
    pub fn at(mut self, origin: [i32; 3]) -> Mesher {
        self.origin = origin;
        self
    }
    pub fn iso(mut self, iso: f32) -> Mesher {
        self.iso = iso;
        self
    }
    /// Declare the neighbours' strides so shared faces agree.
    pub fn seams(mut self, seams: Seams) -> Mesher {
        self.seams = seams;
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

/// Gradient of the trilinear interpolant through `v` at local position `p`,
/// with corners indexed `x + 2y + 4z`. Analytic rather than sampled: the
/// eight values are already in hand and finite differencing them again would
/// be both slower and blurrier.
fn trilinear_gradient(v: &[f32; 8], p: Vec3) -> Vec3 {
    let (a, b, c) = (p.x, p.y, p.z);
    let (ia, ib, ic) = (1.0 - a, 1.0 - b, 1.0 - c);
    Vec3::new(
        (v[1] - v[0]) * ib * ic
            + (v[3] - v[2]) * b * ic
            + (v[5] - v[4]) * ib * c
            + (v[7] - v[6]) * b * c,
        (v[2] - v[0]) * ia * ic
            + (v[3] - v[1]) * a * ic
            + (v[6] - v[4]) * ia * c
            + (v[7] - v[5]) * a * c,
        (v[4] - v[0]) * ia * ib
            + (v[5] - v[1]) * a * ib
            + (v[6] - v[2]) * ia * b
            + (v[7] - v[3]) * a * b,
    )
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
                // ON A COARSER NEIGHBOUR'S FACE, BORROW ITS CELL GRID.
                //
                // The failed first attempt quantised the resulting vertex
                // POSITION, which destroys the surface location surface nets
                // encodes there. The fix has to act one level earlier: a
                // boundary cell is meshed as though it were the coarse cell
                // that contains it — same eight samples, same interpolation,
                // same answer. Every fine cell inside one coarse footprint
                // therefore emits the SAME vertex (degenerate triangles
                // between them, which cost nothing and hide nothing), and it
                // is a vertex the neighbour derives independently from the
                // same data. The seam does not need stitching because the two
                // sides never disagreed.
                //
                // Detail is lost in one cell layer. That is the correct place
                // to lose it: the cell next to a chunk that already threw that
                // detail away.
                // The borrowed layer is r cells THICK, not one. A coarse cell
                // covers r fine cells per axis, so a one-cell layer collapses
                // only part of each footprint and leaves the rest of it
                // disagreeing — which is where a chunk of the residual came
                // from before this was fixed.
                let mut r = 1i32;
                for (face, lo, hi) in [
                    (0usize, x, cx - 1 - x),
                    (1, cx - 1 - x, x),
                    (2, y, cy - 1 - y),
                    (3, cy - 1 - y, y),
                    (4, z, cz - 1 - z),
                    (5, cz - 1 - z, z),
                ] {
                    let n = m.seams.0[face];
                    if n <= k {
                        continue;
                    }
                    let ratio = (n / k).max(1);
                    let _ = hi;
                    if lo < ratio {
                        r = ratio;
                        break;
                    }
                }
                let base = [
                    x.div_euclid(r) * r,
                    y.div_euclid(r) * r,
                    z.div_euclid(r) * r,
                ];

                let mut v = [0.0f32; 8];
                let mut inside = 0u8;
                for (i, s) in v.iter_mut().enumerate() {
                    let o = corner_offset(i);
                    *s = at(base[0] + o[0] * r, base[1] + o[1] * r, base[2] + o[2] * r);
                    if *s >= m.iso {
                        inside |= 1 << i;
                    }
                }
                // all in or all out: no surface through this cell. On a
                // borrowed grid this can refuse a cell whose own samples DO
                // cross — a feature finer than the neighbour can see — and
                // dropping it is right, because the neighbour has no vertex
                // there to join to.
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
                // placed on the grid it was COMPUTED on, then back into
                // full-detail units so chunks at different strides share one
                // coordinate frame
                let cell = Vec3::new(base[0] as f32, base[1] as f32, base[2] as f32);
                mesh.positions.push((cell + local * r as f32) * k as f32);

                // Values rise INTO the solid (>= iso is inside), so the
                // gradient points inward and the outward normal is its
                // negation. A cell whose eight samples are equal has no
                // direction to offer; +Y is arbitrary but finite, which is
                // what a renderer needs more than it needs correctness in a
                // case that cannot be shaded anyway.
                let g = trilinear_gradient(&v, local);
                mesh.normals.push((-g).try_normalize().unwrap_or(Vec3::Y));
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
                        !here,
                        &mut mesh,
                    );
                }
                if y + 1 < ny && x > 0 && z > 0 && (at(x, y + 1, z) >= m.iso) != here {
                    quad(
                        vertex_of[idx(x - 1, y, z - 1)],
                        vertex_of[idx(x, y, z - 1)],
                        vertex_of[idx(x, y, z)],
                        vertex_of[idx(x - 1, y, z)],
                        here,
                        &mut mesh,
                    );
                }
                if z + 1 < nz && x > 0 && y > 0 && (at(x, y, z + 1) >= m.iso) != here {
                    quad(
                        vertex_of[idx(x - 1, y - 1, z)],
                        vertex_of[idx(x, y - 1, z)],
                        vertex_of[idx(x, y, z)],
                        vertex_of[idx(x - 1, y, z)],
                        !here,
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


    /// The seam metric: for each fine boundary vertex, how far the coarse
    /// chunk puts the surface at the nearest matching z.
    fn seam_error(fine: &Mesh, coarse: &Mesh) -> f32 {
        let a = on_face(fine, 8.0);
        let b: Vec<Vec3> =
            on_face(coarse, 0.0).iter().map(|p| *p + Vec3::new(8.0, 0.0, 0.0)).collect();
        assert!(!a.is_empty() && !b.is_empty(), "no boundary vertices to compare");
        a.iter()
            .filter_map(|p| {
                b.iter()
                    .min_by(|u, v| (u.z - p.z).abs().partial_cmp(&(v.z - p.z).abs()).unwrap())
                    .map(|q| (q.y - p.y).abs())
            })
            .fold(0.0f32, f32::max)
    }

    #[test]
    fn declaring_the_coarse_neighbour_closes_the_seam() {
        // Same two chunks and the same field as the characterisation test; the
        // only change is that the fine side is TOLD its +x neighbour samples
        // at stride 2, so its boundary layer is meshed on the neighbour's grid.
        let f = ridge();
        let coarse = extract(&f, Mesher::new([5, 8, 5]).at([8, 0, 0]).stride(2));
        let naive = extract(&f, Mesher::new([9, 14, 9]));
        let stitched = extract(&f, Mesher::new([9, 14, 9]).seams(Seams::NONE.x_plus(2)));

        let before = seam_error(&naive, &coarse);
        let after = seam_error(&stitched, &coarse);
        assert!(after < before * 0.5, "not much better: {before:.3} -> {after:.3}");
        // NOT ZERO, and the remainder is structural rather than a tuning
        // failure. Surface nets puts a vertex in each CELL, and two abutting
        // chunks own different cells either side of the shared plane — the
        // fine chunk's boundary cell spans world x 6..8, the coarse chunk's
        // spans 8..10. Borrowing the grid makes both sides read the same
        // samples, which is what shrinks the error; it cannot make them the
        // same cell. Closing the last of it means one chunk emitting the quads
        // that BRIDGE the boundary, which needs the neighbour's vertices and
        // is a different piece of work (see the module header).
        assert!(after < 0.2, "residual grew past its recorded value: {after:.4}");
    }

    #[test]
    fn borrowing_the_grid_touches_only_the_declared_boundary_layer() {
        // If a coarsening neighbour degraded the whole chunk, LOD would be
        // quietly wrecking the world every time the player moved.
        let f = ridge();
        let naive = extract(&f, Mesher::new([9, 14, 9]));
        let stitched = extract(&f, Mesher::new([9, 14, 9]).seams(Seams::NONE.x_plus(2)));
        // Compare by POSITION, not by index: borrowing the grid collapses and
        // sometimes drops boundary cells, so the two meshes do not have
        // matching vertex orders. An earlier version of this test zipped them
        // and reported interior vertices as moved when they had simply
        // shifted position in the list.
        let interior = |m: &Mesh| {
            let mut v: Vec<[i32; 3]> = m
                .positions
                .iter()
                .filter(|p| p.x < 6.0)
                .map(|p| [(p.x * 1000.0) as i32, (p.y * 1000.0) as i32, (p.z * 1000.0) as i32])
                .collect();
            v.sort_unstable();
            v
        };
        assert_eq!(
            interior(&naive),
            interior(&stitched),
            "a coarsening neighbour changed the chunk away from its boundary"
        );
        assert_ne!(
            on_face(&naive, 8.0).len(),
            0,
            "no boundary vertices at all; the fixture is wrong"
        );
    }

    #[test]
    fn cells_sharing_a_coarse_footprint_collapse_onto_one_point() {
        // The mechanism, asserted directly: within one coarse cell the fine
        // cells must agree exactly, or they have not borrowed the same grid.
        let f = ridge();
        let stitched = extract(&f, Mesher::new([9, 14, 9]).seams(Seams::NONE.x_plus(2)));
        let face = on_face(&stitched, 8.0);
        let mut buckets: HashMap<(i32, i32), Vec<Vec3>> = HashMap::new();
        for p in face {
            buckets.entry(((p.y / 2.0) as i32, (p.z / 2.0) as i32)).or_default().push(p);
        }
        let shared = buckets.values().filter(|v| v.len() > 1).count();
        assert!(shared > 0, "no coarse footprint held more than one fine cell");
        for v in buckets.values().filter(|v| v.len() > 1) {
            for q in &v[1..] {
                assert!((v[0] - *q).length() < 1e-4, "{:?} vs {:?} in one footprint", v[0], q);
            }
        }
    }

    #[test]
    fn a_stitched_chunk_gains_no_folded_edges() {
        let f = ridge();
        let mesh = extract(&f, Mesher::new([9, 14, 9]).seams(Seams::all(2)));
        let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
        for t in mesh.indices.chunks_exact(3) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let k = if a < b { (a, b) } else { (b, a) };
                *edges.entry(k).or_insert(0) += 1;
            }
        }
        let folded = edges.values().filter(|&&n| n > 2).count();
        assert_eq!(folded, 0, "{folded} folded edges after borrowing the grid");
    }


    #[test]
    fn overlapping_into_the_neighbour_closes_the_seam_completely() {
        // The remaining structural gap: two abutting chunks own DIFFERENT
        // cells either side of the shared plane, so reading the same samples
        // is not enough. The composition that fixes it needs no new code —
        // the fine chunk simply meshes one coarse footprint PAST the boundary,
        // so it owns the cell the neighbour owns and computes the identical
        // vertex, and it emits the quads bridging the gap because all four
        // cells around those edges are now in range.
        //
        // The neighbour then skips its first layer, or emits a coincident
        // duplicate; either is harmless. This is the recipe a chunk scheduler
        // should follow.
        let f = ridge();
        let coarse = extract(&f, Mesher::new([5, 8, 5]).at([8, 0, 0]).stride(2));
        let overlapped =
            extract(&f, Mesher::new([11, 14, 9]).seams(Seams::NONE.x_plus(2)));

        // every vertex the coarse chunk puts on the shared plane must also be
        // produced by the overlapping fine chunk, exactly
        let want: Vec<Vec3> =
            on_face(&coarse, 0.0).iter().map(|p| *p + Vec3::new(8.0, 0.0, 0.0)).collect();
        assert!(!want.is_empty());
        for q in &want {
            let best = overlapped
                .positions
                .iter()
                .map(|p| (*p - *q).length())
                .fold(f32::INFINITY, f32::min);
            assert!(best < 1e-3, "coarse vertex {q:?} unmatched by {best:.4}");
        }
    }


    #[test]
    fn every_vertex_has_a_finite_unit_normal() {
        let mesh = extract(&sphere(8.0, 8.0, 8.0, 5.0), Mesher::new([17, 17, 17]));
        assert_eq!(mesh.normals.len(), mesh.positions.len());
        for n in &mesh.normals {
            assert!(n.is_finite(), "non-finite normal {n:?}");
            assert!((n.length() - 1.0).abs() < 1e-4, "normal of length {}", n.length());
        }
    }

    /// Normals the conventional way: area-weighted average of the adjacent
    /// triangle normals. Here only to be beaten.
    fn face_averaged_normals(mesh: &Mesh) -> Vec<Vec3> {
        let mut acc = vec![Vec3::ZERO; mesh.positions.len()];
        for t in mesh.indices.chunks_exact(3) {
            let (a, b, c) =
                (mesh.positions[t[0] as usize], mesh.positions[t[1] as usize], mesh.positions[t[2] as usize]);
            // un-normalised cross product is already area-weighted
            let fna = (b - a).cross(c - a);
            for i in t {
                acc[*i as usize] += fna;
            }
        }
        acc.iter().map(|v| v.try_normalize().unwrap_or(Vec3::Y)).collect()
    }

    #[test]
    fn gradient_normals_beat_face_averaged_ones_on_a_known_surface() {
        // The design claim, measured rather than asserted. A sphere's true
        // normal is exactly radial, so both methods can be scored against
        // ground truth on the same mesh.
        //
        // The gradient is not exact either — it is the gradient of the
        // TRILINEAR INTERPOLANT of sampled values, not of the underlying
        // function — so this is a comparison, not a perfection check. An
        // earlier version of this test asserted > 0.995 against ground truth
        // and failed at 0.9903, which is the discretisation error and not a
        // bug.
        let c = Vec3::splat(8.0);
        let mesh = extract(&sphere(8.0, 8.0, 8.0, 5.0), Mesher::new([17, 17, 17]));
        let averaged = face_averaged_normals(&mesh);

        let score = |ns: &[Vec3]| {
            let mut worst = 1.0f32;
            let mut total = 0.0f32;
            for (p, n) in mesh.positions.iter().zip(ns) {
                let d = (*p - c).normalize().dot(*n);
                worst = worst.min(d);
                total += d;
            }
            (worst, total / ns.len() as f32)
        };
        let (gw, gm) = score(&mesh.normals);
        let (aw, am) = score(&averaged);

        // Measured: gradient worst 0.9903 / mean 0.99878, averaged worst
        // -1.0000 / mean 0.98468. The averaged worst case is a vertex whose
        // triangle fan cancels to nothing, leaving no direction at all —
        // exactly the failure the field-based normal cannot have, because it
        // never consults the topology.
        assert!(gw > aw, "worst case: gradient {gw:.4} did not beat averaged {aw:.4}");
        assert!(gm > am, "mean: gradient {gm:.5} did not beat averaged {am:.5}");
        // and the absolute floor the renderer can rely on
        assert!(gw > 0.98, "gradient worst case {gw:.4} is poor even so");
    }

    #[test]
    fn ground_points_up() {
        // plane(h) is positive ABOVE h, so the solid is above and the outward
        // normal is -Y. Written out because the sign convention is the thing
        // most likely to be wrong and least likely to be noticed until a
        // renderer lights the underside of the world.
        let mesh = extract(&plane(4.5), Mesher::new([8, 10, 8]));
        for n in &mesh.normals {
            assert!(n.y < -0.99, "normal {n:?} on a field whose solid is above");
        }
        // and the inverted field gives the inverted normal
        let below = |_x: i32, y: i32, _z: i32| 4.5 - y as f32;
        let mesh = extract(&below, Mesher::new([8, 10, 8]));
        for n in &mesh.normals {
            assert!(n.y > 0.99, "normal {n:?} on a field whose solid is below");
        }
    }

    #[test]
    fn normals_agree_across_an_lod_boundary() {
        // The reason for using the field rather than the triangles: a boundary
        // cell borrowing the coarse grid computes its gradient from the coarse
        // corners, so both sides derive the same normal and the shading does
        // not crease where the geometry already matches.
        let f = ridge();
        let coarse = extract(&f, Mesher::new([5, 8, 5]).at([8, 0, 0]).stride(2));
        let overlapped = extract(&f, Mesher::new([11, 14, 9]).seams(Seams::NONE.x_plus(2)));
        let mut checked = 0;
        for (q, nq) in coarse.positions.iter().zip(&coarse.normals) {
            let want = *q + Vec3::new(8.0, 0.0, 0.0);
            if let Some((_, np)) = overlapped
                .positions
                .iter()
                .zip(&overlapped.normals)
                .find(|(p, _)| (**p - want).length() < 1e-3)
            {
                assert!(np.dot(*nq) > 0.999, "normals differ at {want:?}: {np:?} vs {nq:?}");
                checked += 1;
            }
        }
        assert!(checked > 0, "no shared vertices found to compare");
    }


    #[test]
    fn triangle_winding_agrees_with_the_normals() {
        // FOUND BY THE COMPARISON ABOVE, not by looking: face-averaged normals
        // scored -0.985 — right magnitude, inverted sign — which meant the
        // quads were wound so their geometric normals faced INTO the solid. A
        // back-face-culling renderer would have drawn the world inside out,
        // and nothing else in this crate would have noticed.
        let mesh = extract(&sphere(8.0, 8.0, 8.0, 5.0), Mesher::new([17, 17, 17]));
        let mut agree = 0;
        let mut total = 0;
        for t in mesh.indices.chunks_exact(3) {
            let (a, b, c) = (
                mesh.positions[t[0] as usize],
                mesh.positions[t[1] as usize],
                mesh.positions[t[2] as usize],
            );
            let Some(geo) = (b - a).cross(c - a).try_normalize() else { continue };
            let want = (mesh.normals[t[0] as usize]
                + mesh.normals[t[1] as usize]
                + mesh.normals[t[2] as usize])
                .normalize();
            total += 1;
            if geo.dot(want) > 0.0 {
                agree += 1;
            }
        }
        assert!(total > 100, "only {total} non-degenerate triangles");
        let frac = agree as f32 / total as f32;
        assert!(frac > 0.99, "only {:.1}% of triangles wind outward", frac * 100.0);
    }

    #[test]
    fn a_degenerate_region_does_not_panic() {
        for dims in [[0, 0, 0], [1, 1, 1], [1, 9, 9], [2, 2, 2]] {
            let _ = extract(&plane(0.5), Mesher::new(dims));
        }
    }
}
