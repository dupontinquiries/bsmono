//! mesh.rs — turning the f-rep truth into triangles. Dual contouring on a uniform grid with QEF
//! vertex placement: for every cell straddling the surface we solve for the one point that best
//! satisfies all the tangent planes recorded on its crossing edges, then stitch one quad per
//! crossing edge across the four cells that share it.
//!
//! Why dual contouring and not marching cubes: CAD parts are full of sharp features — a subtracted
//! pocket has square corners, a chamfer has a crisp lip — and marching cubes rounds every one of
//! them off, because its vertices are pinned to edge midpoints. DC places its vertex anywhere in
//! the cell, so an edge reconstructs as an edge. The Hermite data (crossing point + field gradient)
//! comes straight out of the SDF, which is exactly what `Sdf::eval`/`normal` already provide.
//!
//! This is the *uniform* variant: one fixed cell size over the part's bounds. Octree adaptivity
//! (fine cells only near high curvature) is the follow-on; see PARITY.md. What it means today is
//! that resolution costs cubically, so `MeshOpts::resolution` is per-part, not global.

use glam::Vec3;

use crate::sdf::Sdf;

/// An indexed triangle mesh with per-vertex normals sampled from the field.
#[derive(Clone, Debug, Default)]
pub struct TriMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl TriMesh {
    pub fn tri_count(&self) -> usize {
        self.indices.len() / 3
    }
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Signed volume by the divergence theorem. For a closed, outward-wound mesh this is the
    /// enclosed volume and is positive; a negative result means the winding is inverted, and a
    /// value far off the analytic volume means the surface has holes. Tests lean on all three.
    pub fn signed_volume(&self) -> f32 {
        let mut v = 0.0;
        for t in self.indices.chunks_exact(3) {
            let a = Vec3::from(self.positions[t[0] as usize]);
            let b = Vec3::from(self.positions[t[1] as usize]);
            let c = Vec3::from(self.positions[t[2] as usize]);
            v += a.dot(b.cross(c));
        }
        v / 6.0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MeshOpts {
    /// Cells across the part's longest axis. Cost is O(resolution³) in sampling.
    pub resolution: u32,
}

impl Default for MeshOpts {
    fn default() -> Self {
        MeshOpts { resolution: 48 }
    }
}

/// Local corner numbering inside a cell: bit0 = +x, bit1 = +y, bit2 = +z.
const EDGES: [(usize, usize); 12] = [
    (0, 1), (2, 3), (4, 5), (6, 7), // along x
    (0, 2), (1, 3), (4, 6), (5, 7), // along y
    (0, 4), (1, 5), (2, 6), (3, 7), // along z
];

fn corner_offset(n: usize) -> (usize, usize, usize) {
    (n & 1, (n >> 1) & 1, (n >> 2) & 1)
}

/// Multiply a symmetric 3×3 (packed a00,a01,a02,a11,a12,a22) by a vector.
fn mul_sym3(a: [f32; 6], v: Vec3) -> Vec3 {
    Vec3::new(
        a[0] * v.x + a[1] * v.y + a[2] * v.z,
        a[1] * v.x + a[3] * v.y + a[4] * v.z,
        a[2] * v.x + a[4] * v.y + a[5] * v.z,
    )
}

/// Solve (A + λI)x = b for symmetric A, by cofactor inverse. None when near-singular.
fn solve_sym3(a: [f32; 6], b: Vec3, lambda: f32) -> Option<Vec3> {
    let (a00, a01, a02) = (a[0] + lambda, a[1], a[2]);
    let (a11, a12, a22) = (a[3] + lambda, a[4], a[5] + lambda);
    let c00 = a11 * a22 - a12 * a12;
    let c01 = a02 * a12 - a01 * a22;
    let c02 = a01 * a12 - a02 * a11;
    let det = a00 * c00 + a01 * c01 + a02 * c02;
    if det.abs() < 1e-10 {
        return None;
    }
    let c11 = a00 * a22 - a02 * a02;
    let c12 = a02 * a01 - a00 * a12;
    let c22 = a00 * a11 - a01 * a01;
    let inv = 1.0 / det;
    Some(Vec3::new(
        (c00 * b.x + c01 * b.y + c02 * b.z) * inv,
        (c01 * b.x + c11 * b.y + c12 * b.z) * inv,
        (c02 * b.x + c12 * b.y + c22 * b.z) * inv,
    ))
}

/// Locate the surface crossing on a segment whose endpoint distances have opposite signs.
/// Linear interpolation is already close for a true SDF; a few bisection steps clean up the
/// curvature error cheaply, which matters because this point is also where we sample the normal.
fn crossing(sdf: &Sdf, pa: Vec3, da: f32, pb: Vec3, db: f32) -> Vec3 {
    let t = (da / (da - db)).clamp(0.0, 1.0);
    let mut p = pa + (pb - pa) * t;
    let (mut lo, mut hi) = (pa, pb);
    let mut d_lo = da;
    for _ in 0..4 {
        let d = sdf.eval(p);
        if (d < 0.0) == (d_lo < 0.0) {
            lo = p;
            d_lo = d;
        } else {
            hi = p;
        }
        p = (lo + hi) * 0.5;
    }
    p
}

/// Mesh `sdf` over its own bounds. Returns an empty mesh when the field has no surface inside them.
pub fn dual_contour(sdf: &Sdf, opts: MeshOpts) -> TriMesh {
    let (bmin, bmax) = sdf.aabb();
    if !bmin.is_finite() || !bmax.is_finite() || (bmax - bmin).min_element() <= 0.0 {
        return TriMesh::default();
    }
    let res = opts.resolution.max(4);
    let h = ((bmax - bmin).max_element() / res as f32).max(1e-5);
    // Two cells of padding so the surface never touches the grid boundary (an unclosed mesh
    // otherwise, and DC has no boundary case).
    let lo = bmin - Vec3::splat(h * 2.0);
    let hi = bmax + Vec3::splat(h * 2.0);
    let dims = ((hi - lo) / h).ceil();
    let (nx, ny, nz) = (dims.x as usize, dims.y as usize, dims.z as usize);
    if nx == 0 || ny == 0 || nz == 0 {
        return TriMesh::default();
    }
    // Guard against a pathological aspect ratio eating all the memory.
    if nx.saturating_mul(ny).saturating_mul(nz) > 40_000_000 {
        return TriMesh::default();
    }

    let (cx, cy) = (nx + 1, ny + 1);
    let cidx = |i: usize, j: usize, k: usize| i + cx * (j + cy * k);
    let cpos = |i: usize, j: usize, k: usize| lo + Vec3::new(i as f32, j as f32, k as f32) * h;

    // ── pass 1: sample the field on every grid corner ────────────────────────────────────────
    let mut d = vec![0.0f32; cx * cy * (nz + 1)];
    for k in 0..=nz {
        for j in 0..=ny {
            for i in 0..=nx {
                d[cidx(i, j, k)] = sdf.eval(cpos(i, j, k));
            }
        }
    }

    // ── pass 2: one QEF-placed vertex per straddling cell ────────────────────────────────────
    let mut cell_v = vec![u32::MAX; nx * ny * nz];
    let vidx = |i: usize, j: usize, k: usize| i + nx * (j + ny * k);
    let mut mesh = TriMesh::default();

    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let mut ata = [0.0f32; 6];
                let mut atb = Vec3::ZERO;
                let mut mass = Vec3::ZERO;
                let mut n_cross = 0.0f32;

                for &(ea, eb) in &EDGES {
                    let (ax, ay, az) = corner_offset(ea);
                    let (bx, by, bz) = corner_offset(eb);
                    let da = d[cidx(i + ax, j + ay, k + az)];
                    let db = d[cidx(i + bx, j + by, k + bz)];
                    if (da < 0.0) == (db < 0.0) {
                        continue;
                    }
                    let pa = cpos(i + ax, j + ay, k + az);
                    let pb = cpos(i + bx, j + by, k + bz);
                    let p = crossing(sdf, pa, da, pb, db);
                    let n = sdf.normal(p);
                    let dist = n.dot(p);
                    ata[0] += n.x * n.x;
                    ata[1] += n.x * n.y;
                    ata[2] += n.x * n.z;
                    ata[3] += n.y * n.y;
                    ata[4] += n.y * n.z;
                    ata[5] += n.z * n.z;
                    atb += n * dist;
                    mass += p;
                    n_cross += 1.0;
                }
                if n_cross == 0.0 {
                    continue;
                }

                let mass = mass / n_cross;
                // Solve around the mass point: better conditioned, and the regularizer then pulls
                // an underdetermined cell (a flat face gives a rank-1 system) toward the centroid
                // instead of somewhere arbitrary along the plane.
                let rhs = atb - mul_sym3(ata, mass);
                let lambda = 1e-3 * n_cross;
                let v = match solve_sym3(ata, rhs, lambda) {
                    Some(delta) => mass + delta,
                    None => mass,
                };
                // Clamp into the cell: a vertex escaping its own cell is how DC produces
                // self-intersecting geometry.
                let cmin = cpos(i, j, k);
                let v = v.clamp(cmin, cmin + Vec3::splat(h));

                cell_v[vidx(i, j, k)] = mesh.positions.len() as u32;
                mesh.positions.push(v.to_array());
                mesh.normals.push(sdf.normal(v).to_array());
            }
        }
    }
    if mesh.positions.is_empty() {
        return TriMesh::default();
    }

    // ── pass 3: one quad per crossing edge, wound outward ────────────────────────────────────
    // The four cells sharing an axis-aligned edge are listed counter-clockwise around that axis,
    // so the quad's normal is +axis; if the field goes inside→outside along the axis the surface
    // faces that way and the listing stands, otherwise it reverses.
    let quad = |mesh: &mut TriMesh, c: [usize; 4], flip: bool| {
        let v: [u32; 4] = [cell_v[c[0]], cell_v[c[1]], cell_v[c[2]], cell_v[c[3]]];
        if v.iter().any(|&x| x == u32::MAX) {
            return;
        }
        if flip {
            mesh.indices.extend_from_slice(&[v[0], v[2], v[1], v[0], v[3], v[2]]);
        } else {
            mesh.indices.extend_from_slice(&[v[0], v[1], v[2], v[0], v[2], v[3]]);
        }
    };

    for k in 1..nz {
        for j in 1..ny {
            for i in 0..nx {
                let (da, db) = (d[cidx(i, j, k)], d[cidx(i + 1, j, k)]);
                if (da < 0.0) == (db < 0.0) {
                    continue;
                }
                let c = [
                    vidx(i, j - 1, k - 1),
                    vidx(i, j, k - 1),
                    vidx(i, j, k),
                    vidx(i, j - 1, k),
                ];
                quad(&mut mesh, c, da >= 0.0);
            }
        }
    }
    for k in 1..nz {
        for j in 0..ny {
            for i in 1..nx {
                let (da, db) = (d[cidx(i, j, k)], d[cidx(i, j + 1, k)]);
                if (da < 0.0) == (db < 0.0) {
                    continue;
                }
                let c = [
                    vidx(i - 1, j, k - 1),
                    vidx(i - 1, j, k),
                    vidx(i, j, k),
                    vidx(i, j, k - 1),
                ];
                quad(&mut mesh, c, da >= 0.0);
            }
        }
    }
    for k in 0..nz {
        for j in 1..ny {
            for i in 1..nx {
                let (da, db) = (d[cidx(i, j, k)], d[cidx(i, j, k + 1)]);
                if (da < 0.0) == (db < 0.0) {
                    continue;
                }
                let c = [
                    vidx(i - 1, j - 1, k),
                    vidx(i, j - 1, k),
                    vidx(i, j, k),
                    vidx(i - 1, j, k),
                ];
                quad(&mut mesh, c, da >= 0.0);
            }
        }
    }

    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdf::{blend, cuboid, cylinder, shell, sphere, subtract, translate};
    use std::f32::consts::PI;

    fn max_surface_error(sdf: &Sdf, m: &TriMesh) -> f32 {
        m.positions.iter().map(|p| sdf.eval(Vec3::from(*p)).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn sphere_volume_and_winding() {
        let s = sphere(2.0);
        let m = dual_contour(&s, MeshOpts { resolution: 40 });
        assert!(!m.is_empty());
        let want = 4.0 / 3.0 * PI * 8.0;
        let got = m.signed_volume();
        assert!(got > 0.0, "winding is inverted (volume {got})");
        assert!((got - want).abs() / want < 0.02, "volume {got} vs {want}");
    }

    #[test]
    fn vertices_land_on_the_surface() {
        let s = translate(Vec3::new(1.0, -2.0, 0.5), sphere(3.0));
        let m = dual_contour(&s, MeshOpts { resolution: 32 });
        // Cell size here is ~6/32; vertices should sit well inside a tenth of that.
        assert!(max_surface_error(&s, &m) < 0.02, "err {}", max_surface_error(&s, &m));
    }

    #[test]
    fn cube_reconstructs_flat_faces_and_volume() {
        let c = cuboid(Vec3::splat(1.5));
        let m = dual_contour(&c, MeshOpts { resolution: 24 });
        let want = 3.0f32.powi(3);
        let got = m.signed_volume();
        assert!((got - want).abs() / want < 0.03, "volume {got} vs {want}");
        assert!(max_surface_error(&c, &m) < 0.05);
    }

    #[test]
    fn subtracted_hole_removes_its_volume() {
        // 4×4×4 block, drilled through by a radius-1 cylinder along Y.
        let block = subtract(cuboid(Vec3::splat(2.0)), cylinder(1.0, 5.0));
        let m = dual_contour(&block, MeshOpts { resolution: 48 });
        let want = 64.0 - PI * 1.0 * 4.0;
        let got = m.signed_volume();
        assert!((got - want).abs() / want < 0.04, "volume {got} vs {want}");
    }

    #[test]
    fn blend_meshes_as_one_connected_solid() {
        let a = sphere(1.2);
        let b = translate(Vec3::new(1.6, 0.0, 0.0), sphere(1.2));
        let m = dual_contour(&blend(a, b, 0.5), MeshOpts { resolution: 40 });
        assert!(m.signed_volume() > 0.0);
        // Two r=1.2 spheres are 14.5 total; blended and overlapping, less than that but well
        // above one sphere alone — i.e. it fused rather than meshing as two shells.
        let one = 4.0 / 3.0 * PI * 1.2f32.powi(3);
        let v = m.signed_volume();
        assert!(v > one * 1.2 && v < one * 2.0, "volume {v}, one sphere {one}");
    }

    #[test]
    fn shell_produces_a_hollow_with_both_walls() {
        let hollow = shell(0.15, sphere(2.0));
        let m = dual_contour(&hollow, MeshOpts { resolution: 48 });
        // Outer 2.15 minus inner 1.85 — a thin wall, so volume is small but decidedly positive.
        let want = 4.0 / 3.0 * PI * (2.15f32.powi(3) - 1.85f32.powi(3));
        let got = m.signed_volume();
        assert!(got > 0.0, "inverted");
        assert!((got - want).abs() / want < 0.08, "volume {got} vs {want}");
    }

    #[test]
    fn empty_field_yields_empty_mesh() {
        // An intersection of two disjoint solids has no surface at all.
        let none = crate::sdf::intersect(sphere(1.0), translate(Vec3::new(50.0, 0.0, 0.0), sphere(1.0)));
        assert!(dual_contour(&none, MeshOpts { resolution: 16 }).is_empty());
    }
}
