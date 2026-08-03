//! mesh.rs — 2D triangular meshes, generated structurally rather than by a mesher.
//!
//! Constrained Delaunay tetrahedralisation is a project in its own right (TetGen is ~40k lines),
//! and none of it is needed to verify a finite element formulation. What verification needs is
//! meshes of a domain whose exact answer is known, at several refinement levels — which structured
//! generators give directly, with the refinement parameter in hand for a convergence study.
//!
//! A real mesher, or CADscope's SDF via an immersed-boundary formulation, comes later. This is
//! deliberately the boring part.

/// A 2D triangulation. Nodes are (x, y); each triangle indexes three of them.
#[derive(Clone, Debug)]
pub struct Mesh {
    pub xy: Vec<[f64; 2]>,
    pub tri: Vec<[usize; 3]>,
}

impl Mesh {
    pub fn nodes(&self) -> usize {
        self.xy.len()
    }

    /// Twice the signed area of a triangle. Positive means counter-clockwise winding; the element
    /// stiffness formula divides by this, so a degenerate or inverted element is a real error and
    /// not something to paper over with an absolute value.
    pub fn double_area(&self, t: usize) -> f64 {
        let [a, b, c] = self.tri[t];
        let (p, q, r) = (self.xy[a], self.xy[b], self.xy[c]);
        (q[0] - p[0]) * (r[1] - p[1]) - (r[0] - p[0]) * (q[1] - p[1])
    }

    /// Longest edge in the mesh — the `h` a convergence study is measured against.
    pub fn h_max(&self) -> f64 {
        let mut h: f64 = 0.0;
        for t in &self.tri {
            for k in 0..3 {
                let p = self.xy[t[k]];
                let q = self.xy[t[(k + 1) % 3]];
                h = h.max((p[0] - q[0]).hypot(p[1] - q[1]));
            }
        }
        h
    }

    /// An annulus between radii `a` and `b`, `nr` cells radially and `nt` around.
    ///
    /// Node layout: ring by ring from the inside out, `nt` nodes per ring, so ring `i` occupies
    /// `i*nt .. (i+1)*nt`. That makes the boundary sets trivial to name, which matters because a
    /// boundary condition applied to the wrong nodes is a silent wrong answer.
    ///
    /// Radii are graded geometrically. For a coaxial field the potential varies as ln(r), so
    /// uniform radial spacing wastes elements near the outer conductor and under-resolves the
    /// inner one where the field is strongest.
    pub fn annulus(a: f64, b: f64, nr: usize, nt: usize) -> Mesh {
        let mut xy = Vec::with_capacity((nr + 1) * nt);
        for i in 0..=nr {
            let f = i as f64 / nr as f64;
            let r = a * (b / a).powf(f); // geometric grading: uniform in ln(r)
            for j in 0..nt {
                let th = std::f64::consts::TAU * j as f64 / nt as f64;
                xy.push([r * th.cos(), r * th.sin()]);
            }
        }
        let mut tri = Vec::with_capacity(nr * nt * 2);
        for i in 0..nr {
            for j in 0..nt {
                let jn = (j + 1) % nt;
                let (i0, i1) = (i * nt, (i + 1) * nt);
                // Two triangles per quad, wound counter-clockwise.
                //
                // The order matters and the obvious one is wrong: going inner-j -> inner-j+1 ->
                // outer-j+1 -> outer-j traces the quad CLOCKWISE (shoelace on a single cell gives
                // -3*eps), because advancing in angle on the inner ring and returning on the outer
                // one runs the loop backwards. Radially outward first, then around.
                tri.push([i0 + j, i1 + j, i1 + jn]);
                tri.push([i0 + j, i1 + jn, i0 + jn]);
            }
        }
        Mesh { xy, tri }
    }

    /// A rectangle [0,w] x [0,h] with `nx` by `ny` cells, each split into two triangles.
    /// Node (i, j) is at index `j*(nx+1) + i`.
    pub fn rectangle(w: f64, h: f64, nx: usize, ny: usize) -> Mesh {
        let mut xy = Vec::with_capacity((nx + 1) * (ny + 1));
        for j in 0..=ny {
            for i in 0..=nx {
                xy.push([w * i as f64 / nx as f64, h * j as f64 / ny as f64]);
            }
        }
        let idx = |i: usize, j: usize| j * (nx + 1) + i;
        let mut tri = Vec::with_capacity(nx * ny * 2);
        for j in 0..ny {
            for i in 0..nx {
                tri.push([idx(i, j), idx(i + 1, j), idx(i + 1, j + 1)]);
                tri.push([idx(i, j), idx(i + 1, j + 1), idx(i, j + 1)]);
            }
        }
        Mesh { xy, tri }
    }

    /// A structured mesh on an explicit tensor grid.
    ///
    /// The caller supplies the exact x and y coordinate lines, which is what a layered
    /// cross-section needs: a dielectric interface or a conductor edge that falls *between* nodes
    /// cannot be represented, and the resulting error looks like a physics result rather than a
    /// meshing mistake. Passing the lines explicitly makes "the interface is on a node" a
    /// precondition the caller can guarantee instead of a coincidence.
    ///
    /// Node (i, j) is at index `j * xs.len() + i`.
    pub fn structured(xs: &[f64], ys: &[f64]) -> Mesh {
        let (nx, ny) = (xs.len(), ys.len());
        let mut xy = Vec::with_capacity(nx * ny);
        for &y in ys {
            for &x in xs {
                xy.push([x, y]);
            }
        }
        let idx = |i: usize, j: usize| j * nx + i;
        let mut tri = Vec::with_capacity((nx - 1) * (ny - 1) * 2);
        for j in 0..ny - 1 {
            for i in 0..nx - 1 {
                // Alternate the diagonal so the mesh has no systematic directional bias, which
                // would show up as a small anisotropy in the extracted capacitance.
                if (i + j) % 2 == 0 {
                    tri.push([idx(i, j), idx(i + 1, j), idx(i + 1, j + 1)]);
                    tri.push([idx(i, j), idx(i + 1, j + 1), idx(i, j + 1)]);
                } else {
                    tri.push([idx(i, j), idx(i + 1, j), idx(i, j + 1)]);
                    tri.push([idx(i + 1, j), idx(i + 1, j + 1), idx(i, j + 1)]);
                }
            }
        }
        Mesh { xy, tri }
    }

    /// Every node lying on the axis-aligned segment from `p0` to `p1`, within `tol`.
    /// Used to name conductor surfaces; an empty result means the geometry missed the grid.
    pub fn nodes_on_segment(&self, p0: [f64; 2], p1: [f64; 2], tol: f64) -> Vec<usize> {
        let (lo_x, hi_x) = (p0[0].min(p1[0]) - tol, p0[0].max(p1[0]) + tol);
        let (lo_y, hi_y) = (p0[1].min(p1[1]) - tol, p0[1].max(p1[1]) + tol);
        (0..self.xy.len())
            .filter(|&i| {
                let p = self.xy[i];
                p[0] >= lo_x && p[0] <= hi_x && p[1] >= lo_y && p[1] <= hi_y
            })
            .collect()
    }

    /// Centroid of an element, for deciding which material region it belongs to.
    pub fn centroid(&self, t: usize) -> [f64; 2] {
        let [a, b, c] = self.tri[t];
        [
            (self.xy[a][0] + self.xy[b][0] + self.xy[c][0]) / 3.0,
            (self.xy[a][1] + self.xy[b][1] + self.xy[c][1]) / 3.0,
        ]
    }

    /// Ring `i` of an annulus mesh: inner boundary is 0, outer is `nr`.
    pub fn annulus_ring(nt: usize, ring: usize) -> std::ops::Range<usize> {
        ring * nt..(ring + 1) * nt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annulus_areas_sum_to_the_analytic_area() {
        // A sanity check on the triangulation itself: if any element is inverted or duplicated,
        // the signed areas will not add up.
        let (a, b, nt) = (1.0, 3.0, 256);
        let m = Mesh::annulus(a, b, 20, nt);
        let total: f64 = (0..m.tri.len()).map(|t| m.double_area(t) / 2.0).sum();
        // A polygonal approximation of an annulus, so compare against the polygon's exact area.
        let poly = |r: f64| 0.5 * (nt as f64) * r * r * (std::f64::consts::TAU / nt as f64).sin();
        let want = poly(b) - poly(a);
        assert!((total - want).abs() / want < 1e-12, "{total} vs {want}");
    }

    #[test]
    fn every_element_is_positively_oriented() {
        for m in [Mesh::annulus(1.0, 3.0, 8, 32), Mesh::rectangle(2.0, 1.0, 10, 5)] {
            for t in 0..m.tri.len() {
                assert!(m.double_area(t) > 0.0, "element {t} is inverted or degenerate");
            }
        }
    }

    #[test]
    fn ring_indexing_lands_on_the_right_radii() {
        let (a, b, nr, nt) = (1.0, 4.0, 6, 16);
        let m = Mesh::annulus(a, b, nr, nt);
        for i in Mesh::annulus_ring(nt, 0) {
            assert!((m.xy[i][0].hypot(m.xy[i][1]) - a).abs() < 1e-12, "inner ring");
        }
        for i in Mesh::annulus_ring(nt, nr) {
            assert!((m.xy[i][0].hypot(m.xy[i][1]) - b).abs() < 1e-12, "outer ring");
        }
    }

    #[test]
    fn a_structured_grid_is_well_formed_and_lands_on_its_lines() {
        let xs: Vec<f64> = vec![0.0, 0.3, 0.5, 0.9, 1.4];
        let ys: Vec<f64> = vec![0.0, 0.2, 0.7];
        let m = Mesh::structured(&xs, &ys);
        assert_eq!(m.nodes(), 15);
        assert_eq!(m.tri.len(), 4 * 2 * 2);
        for t in 0..m.tri.len() {
            assert!(m.double_area(t) > 0.0, "element {t} inverted");
        }
        // The interface line y = 0.2 must be findable, which is the whole point of naming the
        // grid explicitly.
        let seg = m.nodes_on_segment([0.3, 0.2], [0.9, 0.2], 1e-12);
        assert_eq!(seg.len(), 3, "expected nodes at x = 0.3, 0.5, 0.9");
        for i in seg {
            assert!((m.xy[i][1] - 0.2).abs() < 1e-12);
        }
    }

    #[test]
    fn refinement_halves_the_element_size() {
        let coarse = Mesh::rectangle(1.0, 1.0, 8, 8);
        let fine = Mesh::rectangle(1.0, 1.0, 16, 16);
        assert!((coarse.h_max() / fine.h_max() - 2.0).abs() < 1e-9);
    }
}
