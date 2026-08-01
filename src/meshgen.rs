//! Procedural meshes for the playful-shape world. Everything is unit-sized and
//! gets scaled per instance; capsules come in a few height classes because a
//! capsule can't be non-uniformly scaled without squashing its caps.

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

/// Unit UV sphere, radius 1, centered at origin.
/// Bake a transform into a mesh's vertices (normals rotated, not scaled-skewed
/// beyond uniformish use). For assembling composite procedural models.
pub fn bake(m: &mut MeshData, scale: glam::Vec3, rot: glam::Quat, off: glam::Vec3) {
    for v in m.verts.iter_mut() {
        let p = rot * (glam::Vec3::from(v.pos) * scale) + off;
        v.pos = p.to_array();
        let n = (rot * (glam::Vec3::from(v.nrm) / scale.max(glam::Vec3::splat(1e-4)))).normalize_or(glam::Vec3::Y);
        v.nrm = n.to_array();
    }
}

/// Append `other` into `into` (indices rebased).
#[allow(dead_code)] // composite-mesh API; procars currently uses one mesh per prim
pub fn merge(into: &mut MeshData, other: &MeshData) {
    let base = into.verts.len() as u32;
    into.verts.extend_from_slice(&other.verts);
    into.idx.extend(other.idx.iter().map(|i| i + base));
}

/// Hollow tube: outer + inner cylinder walls and annulus end caps. Unit
/// outer radius 1, inner 0.9, y in [-1, 1] — scale (R, H, R) at push and you
/// can drive on the INSIDE (matched by Sdf::Shell in world + shader kind 5).
/// A hollow TORUS — a pipe bent into a closed ring, with an inner surface
/// you can drive on. Unit major radius 1, tube radius `TT_R`, wall `TT_T`
/// baked in (like the other torus meshes), so a loop is ONE smooth shape
/// instead of a chain of straight segments with a seam at every elbow.
pub const TT_R: f32 = 0.45;
pub const TT_T: f32 = 0.045;

/// A flat RING DECK — an annulus band with a top, a bottom and two rims.
/// Unit outer radius 1; the bore is `RD_IN` of it and the slab is `RD_H`
/// half-thick, so a planetary ring is a road instead of a wire.
pub const RD_IN: f32 = 0.62;
pub const RD_H: f32 = 0.055;

pub fn ring_deck(seg: u32) -> MeshData {
    let mut m = MeshData::new();
    let n = seg.max(24);
    let mut band = |r0: f32, r1: f32, y: f32, up: bool| {
        let base = m.verts.len() as u32;
        let nrm = [0.0, if up { 1.0 } else { -1.0 }, 0.0];
        for i in 0..=n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let (c, sn) = (a.cos(), a.sin());
            for r in [r0, r1] {
                m.verts.push(Vertex { pos: [c * r, y, sn * r], nrm, uv: [i as f32 / n as f32, if r == r0 { 0.0 } else { 1.0 }] });
            }
        }
        for i in 0..n {
            let k = base + i * 2;
            let (a, b, c2, d) = (k, k + 1, k + 2, k + 3);
            if up {
                m.idx.extend_from_slice(&[a, c2, b, b, c2, d]);
            } else {
                m.idx.extend_from_slice(&[a, b, c2, b, d, c2]);
            }
        }
    };
    band(RD_IN, 1.0, RD_H, true);   // the road
    band(RD_IN, 1.0, -RD_H, false); // underside
    let mut rim = |r: f32, outward: bool| {
        let base = m.verts.len() as u32;
        for i in 0..=n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let (c, sn) = (a.cos(), a.sin());
            let nrm = if outward { [c, 0.0, sn] } else { [-c, 0.0, -sn] };
            for y in [-RD_H, RD_H] {
                m.verts.push(Vertex { pos: [c * r, y, sn * r], nrm, uv: [i as f32 / n as f32, y * 0.5 + 0.5] });
            }
        }
        for i in 0..n {
            let k = base + i * 2;
            let (a, b, c2, d) = (k, k + 1, k + 2, k + 3);
            if outward {
                m.idx.extend_from_slice(&[a, b, c2, b, d, c2]);
            } else {
                m.idx.extend_from_slice(&[a, c2, b, b, c2, d]);
            }
        }
    };
    rim(1.0, true);
    rim(RD_IN, false);
    m
}

pub fn torus_tube(maj: u32, min: u32) -> MeshData {
    let mut m = MeshData::new();
    let (nm, nn) = (maj.max(16), min.max(8));
    let mut surface = |tr: f32, inward: bool| {
        let base = m.verts.len() as u32;
        for i in 0..=nm {
            let u = i as f32 / nm as f32 * std::f32::consts::TAU;
            let (cu, su) = (u.cos(), u.sin());
            for j in 0..=nn {
                let v = j as f32 / nn as f32 * std::f32::consts::TAU;
                let (cv, sv) = (v.cos(), v.sin());
                // ring centre at (cu, 0, su); the minor circle sweeps it
                let px = cu * (1.0 + tr * cv);
                let py = tr * sv;
                let pz = su * (1.0 + tr * cv);
                let n = [cu * cv, sv, su * cv];
                let nrm = if inward { [-n[0], -n[1], -n[2]] } else { n };
                m.verts.push(Vertex {
                    pos: [px, py, pz],
                    nrm,
                    uv: [i as f32 / nm as f32, j as f32 / nn as f32],
                });
            }
        }
        let stride = nn + 1;
        for i in 0..nm {
            for j in 0..nn {
                let k = base + i * stride + j;
                let (a, b, c, d) = (k, k + 1, k + stride, k + stride + 1);
                if inward {
                    m.idx.extend_from_slice(&[a, c, b, b, c, d]);
                } else {
                    m.idx.extend_from_slice(&[a, b, c, b, d, c]);
                }
            }
        }
    };
    surface(TT_R + TT_T, false); // outer skin
    surface(TT_R - TT_T, true);  // the bore you drive
    m
}

pub fn tube(seg: u32) -> MeshData {
    let mut m = MeshData::new();
    let n = seg.max(8);
    let (ro, ri) = (1.0f32, 0.9f32);
    let mut ring = |r: f32, inward: bool| {
        let base = m.verts.len() as u32;
        for i in 0..=n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let (c, sn) = (a.cos(), a.sin());
            let nrm = if inward { [-c, 0.0, -sn] } else { [c, 0.0, sn] };
            for y in [-1.0f32, 1.0] {
                m.verts.push(Vertex { pos: [c * r, y, sn * r], nrm, uv: [i as f32 / n as f32, y * 0.5 + 0.5] });
            }
        }
        for i in 0..n {
            let k = base + i * 2;
            let (a, b, c2, d) = (k, k + 1, k + 2, k + 3);
            if inward {
                m.idx.extend_from_slice(&[a, c2, b, b, c2, d]);
            } else {
                m.idx.extend_from_slice(&[a, b, c2, b, d, c2]);
            }
        }
    };
    ring(ro, false);
    ring(ri, true);
    // annulus caps
    for y in [-1.0f32, 1.0] {
        let base = m.verts.len() as u32;
        let nrm = [0.0, y, 0.0];
        for i in 0..=n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let (c, sn) = (a.cos(), a.sin());
            m.verts.push(Vertex { pos: [c * ro, y, sn * ro], nrm, uv: [0.0, 0.0] });
            m.verts.push(Vertex { pos: [c * ri, y, sn * ri], nrm, uv: [1.0, 1.0] });
        }
        for i in 0..n {
            let k = base + i * 2;
            let (a, b, c2, d) = (k, k + 1, k + 2, k + 3);
            if y > 0.0 {
                m.idx.extend_from_slice(&[a, c2, b, b, c2, d]);
            } else {
                m.idx.extend_from_slice(&[a, b, c2, b, d, c2]);
            }
        }
    }
    m
}

/// Tube WITHOUT annulus end caps: at a carved junction the cap ring is
/// the visible wall you drive into — the SDF is open but the raster was
/// not. Chained/junction segments use this.
/// A flat-top hexagonal prism, circumradius 1 in XZ, half-height 1 in Y —
/// the DROPSHOT tile. Scale gives the real dimensions.
pub fn hex_prism() -> MeshData {
    let mut m = MeshData { verts: Vec::new(), idx: Vec::new() };
    let ang = |k: usize| std::f32::consts::TAU * (k as f32) / 6.0;
    let pts: Vec<(f32, f32)> = (0..6).map(|k| (ang(k).cos(), ang(k).sin())).collect();
    // top and bottom faces
    for (sign, ny) in [(1.0f32, 1.0f32), (-1.0, -1.0)] {
        let base = m.verts.len() as u32;
        m.verts.push(Vertex { pos: [0.0, sign, 0.0], nrm: [0.0, ny, 0.0], uv: [0.5, 0.5] });
        for &(x, z) in &pts {
            m.verts.push(Vertex { pos: [x, sign, z], nrm: [0.0, ny, 0.0], uv: [x * 0.5 + 0.5, z * 0.5 + 0.5] });
        }
        for k in 0..6u32 {
            let (a, b) = (base + 1 + k, base + 1 + (k + 1) % 6);
            if sign > 0.0 {
                m.idx.extend_from_slice(&[base, a, b]);
            } else {
                m.idx.extend_from_slice(&[base, b, a]);
            }
        }
    }
    // sides
    for k in 0..6usize {
        let (x0, z0) = pts[k];
        let (x1, z1) = pts[(k + 1) % 6];
        let nx = (x0 + x1) * 0.5;
        let nz = (z0 + z1) * 0.5;
        let l = (nx * nx + nz * nz).sqrt().max(1e-5);
        let n = [nx / l, 0.0, nz / l];
        let b = m.verts.len() as u32;
        m.verts.push(Vertex { pos: [x0, 1.0, z0], nrm: n, uv: [0.0, 0.0] });
        m.verts.push(Vertex { pos: [x1, 1.0, z1], nrm: n, uv: [1.0, 0.0] });
        m.verts.push(Vertex { pos: [x1, -1.0, z1], nrm: n, uv: [1.0, 1.0] });
        m.verts.push(Vertex { pos: [x0, -1.0, z0], nrm: n, uv: [0.0, 1.0] });
        m.idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    m
}

pub fn tube_open(seg: u32) -> MeshData {
    let mut m = MeshData::new();
    let n = seg.max(8);
    let (ro, ri) = (1.0f32, 0.9f32);
    let mut ring = |r: f32, inward: bool| {
        let base = m.verts.len() as u32;
        for i in 0..=n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let (c, sn) = (a.cos(), a.sin());
            let nrm = if inward { [-c, 0.0, -sn] } else { [c, 0.0, sn] };
            for y in [-1.0f32, 1.0] {
                m.verts.push(Vertex { pos: [c * r, y, sn * r], nrm, uv: [i as f32 / n as f32, y * 0.5 + 0.5] });
            }
        }
        for i in 0..n {
            let k = base + i * 2;
            let (a, b, c2, d) = (k, k + 1, k + 2, k + 3);
            if inward {
                m.idx.extend_from_slice(&[a, c2, b, b, c2, d]);
            } else {
                m.idx.extend_from_slice(&[a, b, c2, b, d, c2]);
            }
        }
    };
    ring(ro, false);
    ring(ri, true);
    // CHAMFERED MOUTHS. With bare outer and inner walls the rim was a
    // zero-thickness edge — the tube read as a rolled sheet rather than a
    // pipe with a wall. This band joins the two surfaces at each end and is
    // pulled back along the axis as it goes in, so the mouth is a TAPERED
    // chamfer you can see the thickness of, and it catches a highlight.
    let cham = (ro - ri) * 0.9; // axial setback of the inner lip
    let mut rim = |end: f32| {
        let base = m.verts.len() as u32;
        for i in 0..=n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let (c, sn) = (a.cos(), a.sin());
            // the chamfer faces outward AND along the axis: halfway between
            let nrm = [c * 0.7, end * 0.71, sn * 0.7];
            m.verts.push(Vertex { pos: [c * ro, end, sn * ro], nrm, uv: [i as f32 / n as f32, 0.0] });
            m.verts.push(Vertex {
                pos: [c * ri, end - end * cham, sn * ri],
                nrm,
                uv: [i as f32 / n as f32, 1.0],
            });
        }
        for i in 0..n {
            let k = base + i * 2;
            let (a2, b2, c2, d2) = (k, k + 1, k + 2, k + 3);
            if end > 0.0 {
                m.idx.extend_from_slice(&[a2, b2, c2, b2, d2, c2]);
            } else {
                m.idx.extend_from_slice(&[a2, c2, b2, b2, c2, d2]);
            }
        }
    };
    rim(1.0);
    rim(-1.0);
    m
}

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

/// Unit box (half extents 1), hard normals.
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

/// Flat capped cylinder for boost pad markers: radius 1, height 0.12, sitting on y=0.
pub fn pad_disc(cols: u32) -> MeshData {
    let mut m = MeshData::new();
    let h = 0.12;
    // side
    for c in 0..=cols {
        let u = c as f32 / cols as f32;
        let th = u * std::f32::consts::TAU;
        let n = Vec3::new(th.cos(), 0.0, th.sin());
        m.push(n + Vec3::Y * h, n, Vec2::new(u, 0.0));
        m.push(n, n, Vec2::new(u, 1.0));
    }
    for c in 0..cols {
        let a = c * 2;
        m.tri(a, a + 2, a + 1);
        m.tri(a + 1, a + 2, a + 3);
    }
    // top fan
    let center = m.push(Vec3::Y * h, Vec3::Y, Vec2::new(0.5, 0.5));
    let ring0 = m.verts.len() as u32;
    for c in 0..=cols {
        let th = c as f32 / cols as f32 * std::f32::consts::TAU;
        m.push(
            Vec3::new(th.cos(), h, th.sin()),
            Vec3::Y,
            Vec2::new(0.5 + th.cos() * 0.5, 0.5 + th.sin() * 0.5),
        );
    }
    for c in 0..cols {
        m.tri(center, ring0 + c + 1, ring0 + c);
    }
    m
}

/// Terrain disk: radius `radius`, `n` cells across, displaced by `h(x, z)`.
/// Normals from central differences of the same height function, so lighting
/// matches the physics SDF exactly.
pub fn terrain<F: Fn(f32, f32) -> f32>(radius: f32, n: u32, h: F) -> MeshData {
    let mut m = MeshData::new();
    let step = radius * 2.0 / n as f32;
    for j in 0..=n {
        for i in 0..=n {
            let x = -radius + i as f32 * step;
            let z = -radius + j as f32 * step;
            let y = h(x, z);
            let e = 0.6;
            let nx = h(x - e, z) - h(x + e, z);
            let nz = h(x, z - e) - h(x, z + e);
            let nrm = Vec3::new(nx, 2.0 * e, nz).normalize();
            m.push(Vec3::new(x, y, z), nrm, Vec2::new(x, z));
        }
    }
    let w = n + 1;
    for j in 0..n {
        for i in 0..n {
            let a = j * w + i;
            let b = a + 1;
            let c = a + w;
            let d = c + 1;
            m.tri(a, c, b);
            m.tri(b, c, d);
        }
    }
    m
}
