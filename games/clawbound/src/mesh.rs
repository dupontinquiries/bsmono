//! Flat-shaded mesh builder — the papercraft "origami" facet look comes from
//! per-face normals (hard shading) on simple extruded solids.

use crate::math::{v3, Vec3};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex { pub pos: [f32; 3], pub nrm: [f32; 3], pub col: [f32; 3], pub op: f32 }

pub struct Mesh { pub verts: Vec<Vertex>, pub op: f32 }

pub fn rgb(hex: u32) -> [f32; 3] {
    [((hex >> 16) & 255) as f32 / 255.0, ((hex >> 8) & 255) as f32 / 255.0, (hex & 255) as f32 / 255.0]
}
pub fn shade(c: [f32; 3], f: f32) -> [f32; 3] { [c[0] * f, c[1] * f, c[2] * f] }
pub fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] * (1.0 - t) + b[0] * t, a[1] * (1.0 - t) + b[1] * t, a[2] * (1.0 - t) + b[2] * t]
}

impl Mesh {
    pub fn new() -> Self { Mesh { verts: Vec::new(), op: 1.0 } }
    pub fn set_op(&mut self, o: f32) { self.op = o; }

    pub fn tri(&mut self, a: Vec3, b: Vec3, c: Vec3, col: [f32; 3]) {
        let n = b.sub(a).cross(c.sub(a)).norm();
        let op = self.op;
        for p in [a, b, c] {
            self.verts.push(Vertex { pos: [p.x, p.y, p.z], nrm: [n.x, n.y, n.z], col, op });
        }
    }
    pub fn quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3, col: [f32; 3]) {
        self.tri(a, b, c, col); self.tri(a, c, d, col);
    }

    /// Axis-aligned box centred at `c` with half-extents `h`, flat-shaded.
    pub fn boxx(&mut self, c: Vec3, h: Vec3, col: [f32; 3]) {
        let (x0, x1) = (c.x - h.x, c.x + h.x);
        let (y0, y1) = (c.y - h.y, c.y + h.y);
        let (z0, z1) = (c.z - h.z, c.z + h.z);
        let p = |x, y, z| v3(x, y, z);
        // top / bottom
        self.quad(p(x0, y1, z0), p(x1, y1, z0), p(x1, y1, z1), p(x0, y1, z1), col);
        self.quad(p(x0, y0, z1), p(x1, y0, z1), p(x1, y0, z0), p(x0, y0, z0), shade(col, 0.7));
        // +z / -z
        self.quad(p(x0, y0, z1), p(x0, y1, z1), p(x1, y1, z1), p(x1, y0, z1), shade(col, 0.92));
        self.quad(p(x1, y0, z0), p(x1, y1, z0), p(x0, y1, z0), p(x0, y0, z0), shade(col, 0.86));
        // +x / -x
        self.quad(p(x1, y0, z1), p(x1, y1, z1), p(x1, y1, z0), p(x1, y0, z0), shade(col, 0.96));
        self.quad(p(x0, y0, z0), p(x0, y1, z0), p(x0, y1, z1), p(x0, y0, z1), shade(col, 0.82));
    }

    /// A cone (foliage / ears), `seg` facets, apex up.
    pub fn cone(&mut self, base: Vec3, r: f32, height: f32, seg: usize, col: [f32; 3]) {
        let apex = v3(base.x, base.y + height, base.z);
        for i in 0..seg {
            let a0 = i as f32 / seg as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / seg as f32 * std::f32::consts::TAU;
            let p0 = v3(base.x + a0.cos() * r, base.y, base.z + a0.sin() * r);
            let p1 = v3(base.x + a1.cos() * r, base.y, base.z + a1.sin() * r);
            self.tri(p0, apex, p1, col);
            self.tri(base, p1, p0, shade(col, 0.7)); // underside
        }
    }

    pub fn extend(&mut self, o: &Mesh) { self.verts.extend_from_slice(&o.verts); }
    pub fn count(&self) -> u32 { self.verts.len() as u32 }

    /// A tapered round segment from `a` (radius r0) to `b` (radius r1) — a limb/branch.
    pub fn segment(&mut self, a: Vec3, b: Vec3, r0: f32, r1: f32, seg: usize, col: [f32; 3]) {
        let axis = b.sub(a);
        let len = axis.len();
        if len < 1e-4 { return; }
        let t = axis.scale(1.0 / len);
        let up = if t.y.abs() < 0.9 { v3(0.0, 1.0, 0.0) } else { v3(1.0, 0.0, 0.0) };
        let u = t.cross(up).norm();
        let w = t.cross(u).norm();
        let tau = std::f32::consts::TAU;
        for i in 0..seg {
            let a0 = i as f32 / seg as f32 * tau;
            let a1 = (i + 1) as f32 / seg as f32 * tau;
            let d0 = u.scale(a0.cos()).add(w.scale(a0.sin()));
            let d1 = u.scale(a1.cos()).add(w.scale(a1.sin()));
            let p00 = a.add(d0.scale(r0)); let p10 = a.add(d1.scale(r0));
            let p01 = b.add(d0.scale(r1)); let p11 = b.add(d1.scale(r1));
            self.quad(p00, p10, p11, p01, col);
        }
    }
}
