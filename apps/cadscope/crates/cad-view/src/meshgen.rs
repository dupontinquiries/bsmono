//! meshgen.rs — procedural display meshes, ported from `games/skydribble/src/meshgen.rs` (v1.26)
//! with the game-only generators (tube, pad_disc, terrain) left upstream until a CAD feature pulls
//! them. Everything is unit-sized and scaled per instance; a capsule can't be non-uniformly scaled
//! without squashing its caps (same caveat as upstream — the forge keeps capsule scale uniform).
//!
//! M0 rule: every kernel primitive that the viewport shows has its meshgen twin here, so the SDF a
//! pick-ray marches and the triangles on screen are the same shape by construction. When the M1
//! mesher lands (dual contouring over arbitrary `Sdf` trees), these stay as the fast path for
//! untransformed primitives.

use glam::{Vec2, Vec3};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub nrm: [f32; 3],
    pub uv: [f32; 2],
}

pub struct MeshData {
    pub verts: Vec<Vertex>,
    pub idx: Vec<u32>,
}

impl MeshData {
    pub fn new() -> Self {
        MeshData { verts: Vec::new(), idx: Vec::new() }
    }
    fn push(&mut self, p: Vec3, n: Vec3, uv: Vec2) -> u32 {
        self.verts.push(Vertex { pos: p.to_array(), nrm: n.to_array(), uv: uv.to_array() });
        (self.verts.len() - 1) as u32
    }
    fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.idx.extend_from_slice(&[a, b, c]);
    }
}

impl Default for MeshData {
    fn default() -> Self { Self::new() }
}

/// Bake a transform into a mesh's vertices (normals rotated, not scaled-skewed beyond uniformish
/// use). For assembling composite procedural models. (Upstream skydribble, verbatim.)
pub fn bake(m: &mut MeshData, scale: Vec3, rot: glam::Quat, off: Vec3) {
    for v in m.verts.iter_mut() {
        let p = rot * (Vec3::from(v.pos) * scale) + off;
        v.pos = p.to_array();
        let n = (rot * (Vec3::from(v.nrm) / scale.max(Vec3::splat(1e-4)))).normalize_or(Vec3::Y);
        v.nrm = n.to_array();
    }
}

/// Append `other` into `into` (indices rebased). (Upstream skydribble, verbatim.)
pub fn merge(into: &mut MeshData, other: &MeshData) {
    let base = into.verts.len() as u32;
    into.verts.extend_from_slice(&other.verts);
    into.idx.extend(other.idx.iter().map(|i| i + base));
}

/// Unit UV sphere, radius 1, centered at origin.
pub fn sphere(rows: u32, cols: u32) -> MeshData {
    let mut m = MeshData::new();
    for r in 0..=rows {
        let v = r as f32 / rows as f32;
        let phi = v * std::f32::consts::PI;
        for c in 0..=cols {
            let u = c as f32 / cols as f32;
            let th = u * std::f32::consts::TAU;
            let p = Vec3::new(phi.sin() * th.cos(), phi.cos(), phi.sin() * th.sin());
            m.push(p, p, Vec2::new(u, v));
        }
    }
    let w = cols + 1;
    for r in 0..rows {
        for c in 0..cols {
            let a = r * w + c;
            let b = a + 1;
            let d = a + w;
            let e = d + 1;
            m.tri(a, b, d);
            m.tri(b, e, d);
        }
    }
    m
}

/// Torus around the +Y axis: major radius 1, tube radius `ratio`.
pub fn torus(ratio: f32, segs: u32, sides: u32) -> MeshData {
    let mut m = MeshData::new();
    for s in 0..=segs {
        let u = s as f32 / segs as f32;
        let a = u * std::f32::consts::TAU;
        let center = Vec3::new(a.cos(), 0.0, a.sin());
        for t in 0..=sides {
            let v = t as f32 / sides as f32;
            let b = v * std::f32::consts::TAU;
            let n = center * b.cos() + Vec3::Y * b.sin();
            m.push(center + n * ratio, n, Vec2::new(u * 6.0, v));
        }
    }
    let w = sides + 1;
    for s in 0..segs {
        for t in 0..sides {
            let a = s * w + t;
            let b = a + 1;
            let d = a + w;
            let e = d + 1;
            m.tri(a, b, d);
            m.tri(b, e, d);
        }
    }
    m
}

/// Vertical capsule: radius 1, cylinder half-height `hh` (total height 2*hh + 2).
pub fn capsule(hh: f32, rows: u32, cols: u32) -> MeshData {
    let mut m = MeshData::new();
    let total = rows * 2 + 2; // top cap rows, 1 cylinder band, bottom cap rows
    for r in 0..=total {
        // map row to latitude, inserting a straight cylinder section at the equator
        let (phi, yoff) = if r <= rows {
            (r as f32 / rows as f32 * std::f32::consts::FRAC_PI_2, hh)
        } else if r == rows + 1 {
            (std::f32::consts::FRAC_PI_2, -hh)
        } else {
            (
                std::f32::consts::FRAC_PI_2
                    + (r - rows - 1) as f32 / (rows + 1) as f32 * std::f32::consts::FRAC_PI_2,
                -hh,
            )
        };
        for c in 0..=cols {
            let u = c as f32 / cols as f32;
            let th = u * std::f32::consts::TAU;
            let n = Vec3::new(phi.sin() * th.cos(), phi.cos(), phi.sin() * th.sin());
            let p = n + Vec3::Y * yoff;
            m.push(p, n, Vec2::new(u, r as f32 / total as f32));
        }
    }
    let w = cols + 1;
    for r in 0..total {
        for c in 0..cols {
            let a = r * w + c;
            let b = a + 1;
            let d = a + w;
            let e = d + 1;
            m.tri(a, b, d);
            m.tri(b, e, d);
        }
    }
    m
}

/// Y-axis cylinder: radius 1, half-height 1, capped. (New here; upstream's closest is pad_disc.)
pub fn cylinder(cols: u32) -> MeshData {
    let mut m = MeshData::new();
    let n = cols.max(8);
    // side wall
    for c in 0..=n {
        let u = c as f32 / n as f32;
        let th = u * std::f32::consts::TAU;
        let nrm = Vec3::new(th.cos(), 0.0, th.sin());
        m.push(nrm + Vec3::Y, nrm, Vec2::new(u, 0.0));
        m.push(nrm - Vec3::Y, nrm, Vec2::new(u, 1.0));
    }
    for c in 0..n {
        let a = c * 2;
        m.tri(a, a + 2, a + 1);
        m.tri(a + 1, a + 2, a + 3);
    }
    // caps (fans)
    for y in [1.0f32, -1.0] {
        let nrm = Vec3::Y * y;
        let center = m.push(nrm, nrm, Vec2::new(0.5, 0.5));
        let ring0 = m.verts.len() as u32;
        for c in 0..=n {
            let th = c as f32 / n as f32 * std::f32::consts::TAU;
            m.push(
                Vec3::new(th.cos(), y, th.sin()),
                nrm,
                Vec2::new(0.5 + th.cos() * 0.5, 0.5 + th.sin() * 0.5),
            );
        }
        for c in 0..n {
            if y > 0.0 {
                m.tri(center, ring0 + c + 1, ring0 + c);
            } else {
                m.tri(center, ring0 + c, ring0 + c + 1);
            }
        }
    }
    m
}

/// Unit box (half extents 1), hard normals. (Upstream name kept: `boxx`.)
pub fn boxx() -> MeshData {
    let mut m = MeshData::new();
    let faces: [(Vec3, Vec3, Vec3); 6] = [
        (Vec3::X, Vec3::Y, Vec3::Z),
        (-Vec3::X, Vec3::Y, -Vec3::Z),
        (Vec3::Y, Vec3::Z, Vec3::X),
        (-Vec3::Y, Vec3::Z, -Vec3::X),
        (Vec3::Z, Vec3::Y, -Vec3::X),
        (-Vec3::Z, Vec3::Y, Vec3::X),
    ];
    for (n, up, right) in faces {
        let base = m.verts.len() as u32;
        for (sy, sx, uv) in [
            (-1.0, -1.0, Vec2::new(0.0, 1.0)),
            (-1.0, 1.0, Vec2::new(1.0, 1.0)),
            (1.0, 1.0, Vec2::new(1.0, 0.0)),
            (1.0, -1.0, Vec2::new(0.0, 0.0)),
        ] {
            m.push(n + up * sy + right * sx, n, uv);
        }
        m.tri(base, base + 2, base + 1);
        m.tri(base, base + 3, base + 2);
    }
    m
}

/// Adopt a kernel `TriMesh` (dual-contoured CSG geometry) as a renderable mesh. UVs are zeroed:
/// the shader only reads them for the ground grid, and real parameterization is cad-tex's job (M4).
pub fn from_trimesh(t: &cad_kernel::TriMesh) -> MeshData {
    MeshData {
        verts: t
            .positions
            .iter()
            .zip(t.normals.iter())
            .map(|(p, n)| Vertex { pos: *p, nrm: *n, uv: [0.0, 0.0] })
            .collect(),
        idx: t.indices.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(m: &MeshData) {
        assert!(!m.verts.is_empty() && !m.idx.is_empty());
        assert_eq!(m.idx.len() % 3, 0, "triangle list");
        let n = m.verts.len() as u32;
        assert!(m.idx.iter().all(|&i| i < n), "indices in range");
        for v in &m.verts {
            let l = Vec3::from(v.nrm).length();
            assert!((l - 1.0).abs() < 1e-3, "unit normals ({l})");
        }
    }

    #[test]
    fn all_generators_produce_sane_meshes() {
        for m in [boxx(), sphere(12, 18), torus(0.35, 24, 12), capsule(0.8, 6, 12), cylinder(24)] {
            check(&m);
        }
    }

    #[test]
    fn sphere_vertices_sit_on_the_unit_sphere() {
        for v in sphere(8, 12).verts {
            assert!((Vec3::from(v.pos).length() - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn trimesh_adoption_preserves_counts() {
        let solid = cad_kernel::sdf::subtract(
            cad_kernel::sdf::cuboid(Vec3::splat(2.0)),
            cad_kernel::sdf::cylinder(0.8, 3.0),
        );
        let t = cad_kernel::dual_contour(&solid, cad_kernel::MeshOpts { resolution: 24 });
        let m = from_trimesh(&t);
        assert_eq!(m.verts.len(), t.positions.len());
        assert_eq!(m.idx.len(), t.indices.len());
        check(&m);
    }

    #[test]
    fn merge_rebases_indices() {
        let mut a = boxx();
        let base = a.verts.len() as u32;
        let b = boxx();
        merge(&mut a, &b);
        assert!(a.idx[b.idx.len()..].iter().rev().take(3).all(|&i| i >= base));
        check(&a);
    }
}
