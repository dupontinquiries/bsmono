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

/// The chamfered lip of a tube MOUTH: a tapered band joining the outer wall
/// to the inner one, so an opening shows the wall's thickness instead of a
/// zero-width edge. Unit radius, sitting in the z=0 plane and facing -Z.
pub fn tube_rim(seg: u32) -> MeshData {
    let mut m = MeshData::new();
    let n = seg.max(12);
    let (ro, ri) = (1.0f32, 0.86f32);
    let cham = (ro - ri) * 0.9;
    let base = m.verts.len() as u32;
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let (c, sn) = (a.cos(), a.sin());
        let nrm = [c * 0.7, sn * 0.7, -0.71];
        m.push(Vec3::new(c * ro, sn * ro, 0.0), Vec3::from_array(nrm), Vec2::new(i as f32 / n as f32, 0.0));
        m.push(Vec3::new(c * ri, sn * ri, cham), Vec3::from_array(nrm), Vec2::new(i as f32 / n as f32, 1.0));
    }
    for i in 0..n {
        let k = base + i * 2;
        m.idx.extend_from_slice(&[k, k + 1, k + 2, k + 1, k + 3, k + 2]);
    }
    m
}

pub fn tube_open(seg: u32) -> MeshData {
    let mut m = MeshData::new();
    let n = seg.max(8);
    let (ro, ri) = (1.0f32, 0.86f32);
    // FLUTES: a perfectly smooth cylinder of translucent glass has nothing
    // for the light to catch, which is why the tubes kept reading flat. A
    // shallow ribbing around the circumference gives the wall form — the
    // highlight now runs along the flutes and the pipe looks round.
    let flutes = 12.0f32;
    let mut ring = |r: f32, inward: bool| {
        let base = m.verts.len() as u32;
        for i in 0..=n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let (c, sn) = (a.cos(), a.sin());
            let rib = 1.0 + (a * flutes).cos() * if inward { 0.012 } else { 0.028 };
            let rr = r * rib;
            // tilt the normal with the flute so it shades as a ridge
            let dt = -(a * flutes).sin() * flutes * 0.028;
            let tang = [-sn + c * dt, 0.0, c + sn * dt];
            let len = (tang[0] * tang[0] + tang[2] * tang[2]).sqrt().max(1e-5);
            let nb = [tang[2] / len, 0.0, -tang[0] / len];
            let nrm = if inward { [-nb[0], 0.0, -nb[2]] } else { nb };
            for y in [-1.0f32, 1.0] {
                m.verts.push(Vertex { pos: [c * rr, y, sn * rr], nrm, uv: [i as f32 / n as f32 * flutes * 0.25, y * 0.5 + 0.5] });
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
    // NO RIM HERE. A chamfer on every segment draws a ring at both ends of
    // every one of them — and a tube is dozens of overlapping segments, so
    // the bore filled with concentric circles that appear and vanish as the
    // view angle changes. The chamfer belongs only at a real MOUTH, so it
    // is its own mesh now (`tube_rim`), placed by the generator at the two
    // ends of a run rather than baked into every link.
    m
}

/// A CHAMFERED BOX. The procedural cars were built from hard-edged boxes,
/// which is what made them read as blocks rather than vehicles: a real body
/// catches a highlight along every edge. `bevel` is in unit-box space.
pub fn cbox(bevel: f32) -> MeshData {
    let b = bevel.clamp(0.02, 0.45);
    let i = 1.0 - b;
    let mut m = MeshData::new();
    // eight face quads (inset) plus the bevel bands between them
    let faces: [(Vec3, Vec3, Vec3); 6] = [
        (Vec3::X, Vec3::Y, Vec3::Z),
        (Vec3::NEG_X, Vec3::Y, Vec3::NEG_Z),
        (Vec3::Y, Vec3::NEG_Z, Vec3::X),
        (Vec3::NEG_Y, Vec3::Z, Vec3::X),
        (Vec3::Z, Vec3::Y, Vec3::NEG_X),
        (Vec3::NEG_Z, Vec3::Y, Vec3::X),
    ];
    for (n, u, v) in faces {
        let c = n;
        let base = m.verts.len() as u32;
        for (su, sv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            let p = c + u * (su * i) + v * (sv * i);
            m.push(p, n, Vec2::new(su * 0.5 + 0.5, sv * 0.5 + 0.5));
        }
        m.idx.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
    }
    // bevel strips: join each face edge to its neighbour with a slanted quad
    for (n, u, v) in faces {
        for (eu, ev) in [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            let e = u * eu + v * ev; // the edge direction on this face
            let t = u * ev + v * eu; // along the edge
            let base = m.verts.len() as u32;
            let nrm = (n + e).normalize();
            for st in [-1.0f32, 1.0] {
                m.push(n + e * i + t * (st * i), nrm, Vec2::new(st * 0.5 + 0.5, 0.0));
                m.push(n * i + e + t * (st * i), nrm, Vec2::new(st * 0.5 + 0.5, 1.0));
            }
            m.idx.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
        }
    }
    m
}

/// A RIM in one of the garage's styles, as a unit-radius disc facing X.
///
/// This is drawn OVER whatever wheel a car already has, at the wheel's own
/// pivot and with its own spin, so a style applies to every car in the game
/// — the imported bodies included — without touching their meshes.
/// `spokes` of 0 means a solid disc; `dish` is how far the face is inset.
pub fn rim(spokes: u32, dish: f32, seg: u32) -> MeshData {
    let mut m = MeshData::new();
    let n = seg.max(16);
    let x = 0.30 - dish * 0.5; // the face plane, inset by the dish
    // the outer lip: a short cylinder so the rim has depth against the tyre
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let (c, s2) = (a.cos(), a.sin());
        for xx in [x, 0.30] {
            m.push(Vec3::new(xx, c * 0.94, s2 * 0.94), Vec3::new(0.0, c, s2), Vec2::new(i as f32 / n as f32, 0.0));
        }
    }
    for i in 0..n {
        let k = i * 2;
        m.idx.extend_from_slice(&[k, k + 1, k + 2, k + 1, k + 3, k + 2]);
    }
    // the face: a hub, plus either a solid disc or a set of spokes
    let base = m.verts.len() as u32;
    let nx = Vec3::new(1.0, 0.0, 0.0);
    m.push(Vec3::new(x, 0.0, 0.0), nx, Vec2::new(0.5, 0.5));
    let hub = if spokes == 0 { 0.94 } else { 0.30 };
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        m.push(Vec3::new(x, a.cos() * hub, a.sin() * hub), nx, Vec2::new(0.5, 0.5));
    }
    for i in 0..n {
        m.idx.extend_from_slice(&[base, base + 1 + i, base + 2 + i]);
    }
    // spokes: flat bars from the hub to the lip, each a quad
    if spokes > 0 {
        let w = (0.9 / spokes as f32).clamp(0.03, 0.16);
        for k in 0..spokes {
            let a = k as f32 / spokes as f32 * std::f32::consts::TAU;
            let (ca, sa) = (a.cos(), a.sin());
            let (px, py) = (-sa * w, ca * w); // across the spoke
            let b2 = m.verts.len() as u32;
            for (r, sgn) in [(0.28f32, 1.0f32), (0.28, -1.0), (0.93, 1.0), (0.93, -1.0)] {
                m.push(
                    Vec3::new(x, ca * r + px * sgn, sa * r + py * sgn),
                    nx,
                    Vec2::new(0.5, 0.5),
                );
            }
            m.idx.extend_from_slice(&[b2, b2 + 1, b2 + 2, b2 + 1, b2 + 3, b2 + 2]);
        }
    }
    m
}

/// A LOFTED HULL — the real answer to "these look like stacked boxes".
///
/// Takes cross-sections along Z (nose to tail) and skins between them, so a
/// body can taper into its nose, swell over the wheel arches and pinch at
/// the tail the way a car actually does. Each section is (z, half_width,
/// bottom_y, top_y, corner) and the corner value rounds the section from a
/// rectangle (0) toward an ellipse (1), which is what gives a shoulder line
/// instead of an edge.
pub fn loft(sections: &[(f32, f32, f32, f32, f32)], ring: u32) -> MeshData {
    let mut m = MeshData::new();
    if sections.len() < 2 {
        return m;
    }
    // RESAMPLE ALONG THE LENGTH.
    //
    // Measured on the art target: Octane's painted shell has a median
    // dihedral of about 2 degrees — neighbouring triangles are very nearly
    // coplanar, which is what makes a surface read as continuous bodywork
    // rather than as a low-poly prop. Five hand-placed control sections
    // cannot do that however many segments the RING has: the ridges run the
    // wrong way, across the car instead of around it.
    //
    // So the control sections are treated as a spline and resampled. A
    // Catmull-Rom through them keeps every shape the author asked for and
    // fills in the surface between.
    let sections: Vec<(f32, f32, f32, f32, f32)> = if sections.len() >= 3 {
        let n = sections.len();
        let get = |i: isize| -> (f32, f32, f32, f32, f32) {
            sections[(i.max(0) as usize).min(n - 1)]
        };
        let sub = 4; // four samples per control span
        let mut out = Vec::with_capacity((n - 1) * sub + 1);
        for i in 0..(n - 1) {
            let (p0, p1, p2, p3) = (get(i as isize - 1), get(i as isize), get(i as isize + 1), get(i as isize + 2));
            for k in 0..sub {
                let t = k as f32 / sub as f32;
                let (t2, t3) = (t * t, t * t * t);
                // Catmull-Rom on each of the five channels
                let cr = |a: f32, b: f32, c: f32, d: f32| -> f32 {
                    0.5 * ((2.0 * b) + (-a + c) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (-a + 3.0 * b - 3.0 * c + d) * t3)
                };
                out.push((
                    cr(p0.0, p1.0, p2.0, p3.0),
                    cr(p0.1, p1.1, p2.1, p3.1).max(0.001),
                    cr(p0.2, p1.2, p2.2, p3.2),
                    cr(p0.3, p1.3, p2.3, p3.3),
                    cr(p0.4, p1.4, p2.4, p3.4).clamp(0.0, 1.0),
                ));
            }
        }
        out.push(sections[n - 1]);
        out
    } else {
        sections.to_vec()
    };
    let sections = &sections[..];
    let n = ring.max(8);
    // a rounded-rectangle ring in the section's own space
    let ring_pt = |hw: f32, y0: f32, y1: f32, corner: f32, t: f32| -> Vec3 {
        let a = t * std::f32::consts::TAU;
        let (ca, sa) = (a.cos(), a.sin());
        let (cy, hy) = ((y0 + y1) * 0.5, (y1 - y0) * 0.5);
        // blend between a box outline and an ellipse
        let bx = ca.signum() * (ca.abs().powf(0.35));
        let by = sa.signum() * (sa.abs().powf(0.35));
        Vec3::new(
            hw * (bx * (1.0 - corner) + ca * corner),
            cy + hy * (by * (1.0 - corner) + sa * corner),
            0.0,
        )
    };
    let stride = n + 1;
    for (si, &(z, hw, y0, y1, corner)) in sections.iter().enumerate() {
        for i in 0..=n {
            let t = i as f32 / n as f32;
            let p = ring_pt(hw, y0, y1, corner, t);
            // normal from neighbouring sections, so the skin shades smoothly
            let (zp, zn) = (
                sections[si.saturating_sub(1)],
                sections[(si + 1).min(sections.len() - 1)],
            );
            let a = ring_pt(zp.1, zp.2, zp.3, zp.4, t) + Vec3::new(0.0, 0.0, zp.0);
            let b = ring_pt(zn.1, zn.2, zn.3, zn.4, t) + Vec3::new(0.0, 0.0, zn.0);
            let along = (b - a).normalize_or(Vec3::Z);
            let t2 = (i as f32 + 0.25) / n as f32;
            let q = ring_pt(hw, y0, y1, corner, t2);
            let tang = (q - p).normalize_or(Vec3::X);
            let nrm = tang.cross(along).normalize_or(Vec3::Y);
            m.push(Vec3::new(p.x, p.y, z), nrm, Vec2::new(t, si as f32 / sections.len() as f32));
        }
    }
    for si in 0..sections.len() - 1 {
        for i in 0..n {
            let a = (si as u32) * stride + i;
            let b = a + 1;
            let c = a + stride;
            let d = c + 1;
            m.idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    // cap the ends so the hull is closed
    for (end, si) in [(0usize, 0u32), (sections.len() - 1, (sections.len() - 1) as u32)] {
        let (z, hw, y0, y1, corner) = sections[end];
        let base = m.verts.len() as u32;
        let nz = if end == 0 { -1.0 } else { 1.0 };
        m.push(Vec3::new(0.0, (y0 + y1) * 0.5, z), Vec3::new(0.0, 0.0, nz), Vec2::new(0.5, 0.5));
        for i in 0..=n {
            let p = ring_pt(hw, y0, y1, corner, i as f32 / n as f32);
            m.push(Vec3::new(p.x, p.y, z), Vec3::new(0.0, 0.0, nz), Vec2::new(0.5, 0.5));
        }
        let _ = si;
        for i in 0..n {
            if nz > 0.0 {
                m.idx.extend_from_slice(&[base, base + 1 + i, base + 2 + i]);
            } else {
                m.idx.extend_from_slice(&[base, base + 2 + i, base + 1 + i]);
            }
        }
    }
    m
}

/// A WHEEL: a tyre torus with a rim disc, instead of a squashed sphere.
/// Unit radius 1 in Y/Z, `width` along X.
pub fn wheel(width: f32, seg: u32) -> MeshData {
    let mut m = MeshData::new();
    let n = seg.max(12);
    let tube = 0.30f32;
    let ring = 1.0 - tube;
    // tyre
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let (ca, sa) = (a.cos(), a.sin());
        for j in 0..=8u32 {
            let b = j as f32 / 8.0 * std::f32::consts::TAU;
            let (cb, sb) = (b.cos(), b.sin());
            let r = ring + tube * cb;
            let p = Vec3::new(sb * tube * width, ca * r, sa * r);
            let nr = Vec3::new(sb, ca * cb, sa * cb).normalize();
            m.push(p, nr, Vec2::new(i as f32 / n as f32, j as f32 / 8.0));
        }
    }
    let stride = 9u32;
    for i in 0..n {
        for j in 0..8 {
            let k = i * stride + j;
            m.idx.extend_from_slice(&[k, k + stride, k + 1, k + 1, k + stride, k + stride + 1]);
        }
    }
    // rim faces, so the wheel is not hollow when you see it side-on
    for sx in [-1.0f32, 1.0] {
        let base = m.verts.len() as u32;
        m.push(Vec3::new(sx * tube * width * 0.9, 0.0, 0.0), Vec3::new(sx, 0.0, 0.0), Vec2::new(0.5, 0.5));
        for i in 0..=n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            m.push(
                Vec3::new(sx * tube * width * 0.9, a.cos() * ring, a.sin() * ring),
                Vec3::new(sx, 0.0, 0.0),
                Vec2::new(a.cos() * 0.5 + 0.5, a.sin() * 0.5 + 0.5),
            );
        }
        for i in 0..n {
            if sx > 0.0 {
                m.idx.extend_from_slice(&[base, base + 1 + i, base + 2 + i]);
            } else {
                m.idx.extend_from_slice(&[base, base + 2 + i, base + 1 + i]);
            }
        }
    }
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
/// One CHUNK of the terrain grid: the same nodes as `terrain`, but only the
/// cells in [i0,i1) x [j0,j1). Splitting the ground into chunks is what
/// makes it cullable — as one 97k-triangle mesh, every blade of it was
/// drawn every frame no matter where you looked. Returns the mesh and its
/// bounding sphere (centre, radius).
pub fn terrain_chunk<F: Fn(f32, f32) -> f32>(
    radius: f32,
    n: u32,
    i0: u32,
    i1: u32,
    j0: u32,
    j1: u32,
    h: &F,
) -> (MeshData, Vec3, f32) {
    let mut m = MeshData::new();
    let step = radius * 2.0 / n as f32;
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    let w = i1 - i0 + 1;
    for j in j0..=j1 {
        for i in i0..=i1 {
            let x = -radius + i as f32 * step;
            let z = -radius + j as f32 * step;
            let y = h(x, z);
            lo = lo.min(y);
            hi = hi.max(y);
            let e = 0.6;
            let nx = h(x - e, z) - h(x + e, z);
            let nz = h(x, z - e) - h(x, z + e);
            let nrm = Vec3::new(nx, 2.0 * e, nz).normalize();
            m.push(Vec3::new(x, y, z), nrm, Vec2::new(x, z));
        }
    }
    for j in 0..(j1 - j0) {
        for i in 0..(i1 - i0) {
            let a = j * w + i;
            let b = a + 1;
            let c = a + w;
            let d = c + 1;
            m.idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let cx = -radius + (i0 + i1) as f32 * 0.5 * step;
    let cz = -radius + (j0 + j1) as f32 * 0.5 * step;
    let cy = (lo + hi) * 0.5;
    let half_xz = (i1 - i0) as f32 * step * 0.5;
    let r = (half_xz * half_xz * 2.0 + ((hi - lo) * 0.5).powi(2)).sqrt() + step;
    (m, Vec3::new(cx, cy, cz), r)
}

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
