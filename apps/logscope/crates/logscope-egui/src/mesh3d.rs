//! mesh3d.rs — a tiny software forward-renderer. It transforms 3D meshes, shades each vertex with a
//! directional light (Lambert + ambient), sorts triangles back-to-front (painter's algorithm), and emits
//! the result as ONE `egui::epaint::Mesh` of lit, coloured triangles. It reuses `OrbitCam` for world→screen
//! projection, so the meshes line up exactly with the spatial conversation graph (same camera, same scale).
//!
//! This is the step "past UI components into mesh rendering": real geometry (spheres, tubes, faceted rocks,
//! stacked cairns) lit by a sun, drawn through egui's painter. It is NOT a GPU pipeline — there's no
//! z-buffer, so it depth-sorts triangles, which is correct for the convex shapes here (orbs, edges, rocks).
//! The next rung up is a depth-tested wgpu render pass via an egui paint callback with real shaders.

use egui::{Color32, Pos2, epaint::Mesh};
use crate::spatial::{OrbitCam, Vec3};

#[inline] fn v(x: f32, y: f32, z: f32) -> Vec3 { Vec3::new(x, y, z) }
#[inline] fn sub(a: Vec3, b: Vec3) -> Vec3 { v(a.x - b.x, a.y - b.y, a.z - b.z) }
#[inline] fn addv(a: Vec3, b: Vec3) -> Vec3 { v(a.x + b.x, a.y + b.y, a.z + b.z) }
#[inline] fn mul(a: Vec3, s: f32) -> Vec3 { v(a.x * s, a.y * s, a.z * s) }
#[inline] fn dot(a: Vec3, b: Vec3) -> f32 { a.x * b.x + a.y * b.y + a.z * b.z }
#[inline] fn cross(a: Vec3, b: Vec3) -> Vec3 { v(a.y * b.z - a.z * b.y, a.z * b.x - a.x * b.z, a.x * b.y - a.y * b.x) }
#[inline] fn norm(a: Vec3) -> Vec3 { let l = dot(a, a).sqrt(); if l > 1e-6 { mul(a, 1.0 / l) } else { v(0.0, 1.0, 0.0) } }
#[inline] fn hash3(x: i32, y: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343) ^ (y as u32).wrapping_mul(0xD816_3841) ^ (z as u32).wrapping_mul(0xCB1A_B31F);
    h ^= h >> 15; h = h.wrapping_mul(0x2C1B_3C6D); h ^= h >> 12;
    (h & 0xFF_FFFF) as f32 / 0xFF_FFFF as f32
}

/// A 3D mesh: world-space vertices + per-vertex normals + triangle indices + a base albedo. `vcol`, when
/// set, gives a per-vertex albedo (procedural "texture" — moss on top, bark by height). `offset` is added
/// to every vertex at render time, so an object can bob/drift without rebuilding its geometry.
#[derive(Clone)]
pub struct Mesh3 {
    pub verts: Vec<Vec3>, pub norms: Vec<Vec3>, pub tris: Vec<[u32; 3]>,
    pub color: Color32, pub vcol: Option<Vec<Color32>>, pub offset: Vec3,
    /// Material id (a `texgen` layer) sampled triplanar in the scene shader; the per-vertex/`color` tint
    /// modulates it. Default 0 (paper). Set per-mesh by the scene builders (bark, stone, moss, water…).
    pub mat: f32,
    /// Self-illumination 0..N: added to the lit colour (post-exposure, pre-fog) so the surface glows on its
    /// own and the bloom pass catches it. 0 = inert. This is how runes, crystal, and "lit-from-within" text
    /// read as magic rather than paint. The glow takes the mesh's own `color`/`vcol` as its tint.
    pub emissive: f32,
}
impl Mesh3 {
    /// Builder: flag this mesh as self-illuminated (glows + blooms in its own colour).
    pub fn emit(mut self, e: f32) -> Self { self.emissive = e; self }
}

/// A sun plus a hemispherical ambient (a cool sky fill from above, a dim warm bounce from below) — the
/// "live sunshine". `ambient` scales the hemispherical term; `dir` points TOWARD the sun.
pub struct Light { pub dir: Vec3, pub color: Color32, pub sky: Color32, pub ground: Color32, pub ambient: f32 }
impl Light {
    /// A sun of `color` from `dir`, with a sensible cool-sky / warm-ground hemispherical fill.
    pub fn sun(dir: Vec3, color: Color32) -> Self {
        Light { dir: norm(dir), color, sky: Color32::from_rgb(140, 168, 205), ground: Color32::from_rgb(96, 82, 66), ambient: 0.72 }
    }
}

#[inline] fn lerp_c(a: Color32, b: Color32, f: f32) -> Color32 {
    let f = f.clamp(0.0, 1.0);
    Color32::from_rgb(
        (a.r() as f32 * (1.0 - f) + b.r() as f32 * f) as u8,
        (a.g() as f32 * (1.0 - f) + b.g() as f32 * f) as u8,
        (a.b() as f32 * (1.0 - f) + b.b() as f32 * f) as u8,
    )
}
/// Set a mesh.s material id (a `texgen` layer) and return it — for chaining in scene builders.
pub fn with_mat(mut m: Mesh3, mat: f32) -> Mesh3 { m.mat = mat; m }

/// Procedural "texture": tint vertices facing up toward `top` (moss/snow), sides toward `side` (stone/bark).
/// Make a mesh translucent — sets the alpha on the base colour and every per-vertex colour. With the scene
/// pass blending back-to-front (painter's order), alpha<255 meshes (glass) show the scene through them.
pub fn set_alpha(m: &mut Mesh3, a: u8) {
    let wa = |c: Color32| Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a);
    m.color = wa(m.color);
    if let Some(v) = &mut m.vcol { for c in v.iter_mut() { *c = wa(*c); } }
}

pub fn tint_up(m: &mut Mesh3, top: Color32, side: Color32) {
    m.vcol = Some(m.norms.iter().map(|n| lerp_c(side, top, norm(*n).y.clamp(0.0, 1.0).powf(1.4))).collect());
}
/// Procedural "texture": tint vertices by world height between `lo`→`hi` (bark gradient, mossy waterline).
pub fn tint_height(m: &mut Mesh3, lo: f32, hi: f32, low: Color32, high: Color32) {
    let span = (hi - lo).max(1e-3);
    m.vcol = Some(m.verts.iter().map(|p| lerp_c(low, high, ((p.y - lo) / span).clamp(0.0, 1.0))).collect());
}

thread_local! { static TESS_MUL: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) }; }
/// Global tessellation multiplier for the procedural primitives, driven by the detail variant (1.0 = base).
/// Set once per frame before building meshes; every primitive below scales its segment counts by it.
pub fn set_tess_mul(m: f32) { TESS_MUL.with(|c| c.set(m.clamp(0.25, 6.0))); }
#[inline] fn tess(base: usize, min: usize) -> usize { ((base as f32 * TESS_MUL.with(|c| c.get())).round() as usize).max(min) }

/// LOD DETAIL abstraction — maps a feature's distance (from the scene centre, for baked scenes) to a detail
/// factor in [floor, 1.0]: full detail within `near`, linearly thinning to `floor` by `far`. Lets callers stop
/// hand-rolling `(0.95 - d/80).clamp(..)` formulas and keep the density falloff consistent across scenes.
pub fn lod_detail(dist: f32, near: f32, far: f32, floor: f32) -> f32 {
    if dist <= near { 1.0 }
    else if dist >= far { floor }
    else { (1.0 - (dist - near) / (far - near).max(1e-3) * (1.0 - floor)).clamp(floor, 1.0) }
}

/// A smooth UV sphere (vertex normals are radial). `segs` around, `rings` top-to-bottom.
/// Parse a Wavefront **OBJ** into a `Mesh3` (positions `v`, optional normals `vn`, faces `f` with any of
/// `v`, `v/vt`, `v//vn`, `v/vt/vn`, polygons triangulated as a fan). Missing normals are computed per face.
/// A flat `color` + `mat` are applied (per-vertex tinting can be layered afterwards). This is the hook for
/// dropping in CC0 models (e.g. Quaternius / Poly Pizza fish): load, then scale/orient and place in a scene.
pub fn load_obj_str(src: &str, color: Color32, mat: f32) -> Mesh3 {
    let (mut pos, mut nrm): (Vec<Vec3>, Vec<Vec3>) = (Vec::new(), Vec::new());
    let mut verts: Vec<Vec3> = Vec::new();
    let mut norms: Vec<Vec3> = Vec::new();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    let idx = |tok: &str, len: usize| -> Option<usize> {        // OBJ indices are 1-based; negatives wrap
        tok.split('/').next().and_then(|s| s.parse::<i32>().ok()).map(|i| if i < 0 { (len as i32 + i) as usize } else { (i - 1) as usize })
    };
    let nidx = |tok: &str, len: usize| -> Option<usize> {
        let mut it = tok.split('/'); it.next(); it.next();
        it.next().and_then(|s| s.parse::<i32>().ok()).map(|i| if i < 0 { (len as i32 + i) as usize } else { (i - 1) as usize })
    };
    for line in src.lines() {
        let mut t = line.split_whitespace();
        match t.next() {
            Some("v") => { let a: Vec<f32> = t.filter_map(|x| x.parse().ok()).collect(); if a.len() >= 3 { pos.push(v(a[0], a[1], a[2])); } }
            Some("vn") => { let a: Vec<f32> = t.filter_map(|x| x.parse().ok()).collect(); if a.len() >= 3 { nrm.push(v(a[0], a[1], a[2])); } }
            Some("f") => {
                let toks: Vec<&str> = t.collect();
                if toks.len() < 3 { continue; }
                for k in 1..toks.len() - 1 {              // fan-triangulate
                    let tri = [toks[0], toks[k], toks[k + 1]];
                    let base = verts.len() as u32;
                    let p: Vec<Vec3> = tri.iter().filter_map(|tk| idx(tk, pos.len()).and_then(|i| pos.get(i).copied())).collect();
                    if p.len() != 3 { continue; }
                    let fnz = { let e1 = v(p[1].x - p[0].x, p[1].y - p[0].y, p[1].z - p[0].z); let e2 = v(p[2].x - p[0].x, p[2].y - p[0].y, p[2].z - p[0].z); let n = v(e1.y * e2.z - e1.z * e2.y, e1.z * e2.x - e1.x * e2.z, e1.x * e2.y - e1.y * e2.x); let l = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt().max(1e-6); v(n.x / l, n.y / l, n.z / l) };
                    for (j, tk) in tri.iter().enumerate() {
                        verts.push(p[j]);
                        norms.push(nidx(tk, nrm.len()).and_then(|i| nrm.get(i).copied()).unwrap_or(fnz));
                    }
                    tris.push([base, base + 1, base + 2]);
                }
            }
            _ => {}
        }
    }
    Mesh3 { verts, norms, tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat, emissive: 0.0 }
}

pub fn uv_sphere(center: Vec3, r: f32, segs: usize, rings: usize, color: Color32) -> Mesh3 {
    let (segs, rings) = (tess(segs, 3), tess(rings, 2));
    let (mut verts, mut norms, mut tris) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..=rings {
        let phi = std::f32::consts::PI * i as f32 / rings as f32;
        let (sp, cp) = (phi.sin(), phi.cos());
        for j in 0..=segs {
            let th = std::f32::consts::TAU * j as f32 / segs as f32;
            let n = v(sp * th.cos(), cp, sp * th.sin());
            verts.push(addv(center, mul(n, r)));
            norms.push(n);
        }
    }
    let row = segs + 1;
    for i in 0..rings {
        for j in 0..segs {
            let a = (i * row + j) as u32;
            let c = ((i + 1) * row + j) as u32;
            tris.push([a, c, a + 1]);
            tris.push([a + 1, c, c + 1]);
        }
    }
    Mesh3 { emissive: 0.0, verts, norms, tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }
}

/// An open tube (cylinder wall) between `a` and `b` of radius `r`, smooth around the ring. Good for edges.
pub fn tube(a: Vec3, b: Vec3, r: f32, segs: usize, color: Color32) -> Mesh3 {
    let segs = tess(segs, 3);
    let axis = norm(sub(b, a));
    let up = if axis.y.abs() < 0.9 { v(0.0, 1.0, 0.0) } else { v(1.0, 0.0, 0.0) };
    let u = norm(cross(axis, up));
    let w = cross(axis, u);
    let (mut verts, mut norms, mut tris) = (Vec::new(), Vec::new(), Vec::new());
    for j in 0..=segs {
        let th = std::f32::consts::TAU * j as f32 / segs as f32;
        let n = addv(mul(u, th.cos()), mul(w, th.sin()));
        verts.push(addv(a, mul(n, r))); norms.push(n);
        verts.push(addv(b, mul(n, r))); norms.push(n);
    }
    for j in 0..segs {
        let a0 = (j * 2) as u32;
        tris.push([a0, a0 + 2, a0 + 1]);
        tris.push([a0 + 1, a0 + 2, a0 + 3]);
    }
    Mesh3 { emissive: 0.0, verts, norms, tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }
}

/// A tapered, gently CURVED branch (not a straight pillar): a multi-ring tube swept along a curve that bows in
/// `side` (accelerating toward the tip) and wobbles with low-frequency noise for gnarl. Radius lerps r0→r1
/// (taper). Returns (mesh, tip point, tip tangent) so children/twigs attach at the curved end with the right
/// heading — this is what lets a tree bend and keep growing in the bent direction instead of standing straight.
pub fn branch_curved(a: Vec3, dir: Vec3, len: f32, r0: f32, r1: f32, side: Vec3, bend: f32, seed: u32, color: Color32) -> (Mesh3, Vec3, Vec3) {
    let dir = norm(dir);
    // make `side` perpendicular to dir so the bow is purely lateral
    let side = {
        let s = sub(side, mul(dir, dot(side, dir)));
        if dot(s, s) > 1e-5 { norm(s) } else { norm(cross(dir, v(0.0, 1.0, 0.2))) }
    };
    let wdir = cross(dir, side);
    let rings = tess(7, 4);
    let segs = tess(8, 4);
    let (mut verts, mut norms, mut tris) = (Vec::new(), Vec::new(), Vec::new());
    let (mut cen, mut tan) = (Vec::with_capacity(rings + 1), Vec::with_capacity(rings + 1));
    for i in 0..=rings {
        let t = i as f32 / rings as f32;
        let bow = t * t; // accelerating C-bend → the tip leans, base stays put
        let wob = (hash3(i as i32, seed as i32, 1) - 0.5) * 0.10 * len * t; // gnarl grows toward the tip
        let p = addv(addv(a, mul(dir, len * t)), addv(mul(side, bend * len * bow), mul(wdir, wob)));
        cen.push(p);
        tan.push(norm(addv(mul(dir, len), mul(side, bend * len * 2.0 * t)))); // d/dt of the centreline
    }
    for i in 0..=rings {
        let t = i as f32 / rings as f32;
        let r = r0 + (r1 - r0) * t;
        let axis = tan[i];
        let up = if axis.y.abs() < 0.9 { v(0.0, 1.0, 0.0) } else { v(1.0, 0.0, 0.0) };
        let u = norm(cross(axis, up));
        let w = cross(axis, u);
        for j in 0..=segs {
            let th = std::f32::consts::TAU * j as f32 / segs as f32;
            let n = addv(mul(u, th.cos()), mul(w, th.sin()));
            verts.push(addv(cen[i], mul(n, r))); norms.push(n);
        }
    }
    let ring = segs + 1;
    for i in 0..rings {
        for j in 0..segs {
            let a0 = (i * ring + j) as u32;
            let b0 = ((i + 1) * ring + j) as u32;
            tris.push([a0, b0, a0 + 1]);
            tris.push([a0 + 1, b0, b0 + 1]);
        }
    }
    (Mesh3 { emissive: 0.0, verts, norms, tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }, cen[rings], tan[rings])
}
pub fn rock(center: Vec3, r: f32, seed: u32, color: Color32) -> Mesh3 {
    let base = uv_sphere(center, r, 16, 12, color); // higher-res base (also scales with detail)
    let s = seed as f32 * 0.137;
    // displace the SHARED vertices by a smooth, low-frequency field so neighbours move together (no spikes),
    // with a gentle ellipsoid squash → a weathered, half-buried river stone rather than a jagged ball
    let pv: Vec<Vec3> = base.verts.iter().map(|&p| {
        let d = norm(sub(p, center));
        let lump = 0.50 * (d.x * 3.1 + s).sin() * (d.y * 2.3 + s * 1.7).cos()
                 + 0.34 * (d.y * 2.7 + s * 0.6).sin()
                 + 0.24 * (d.z * 3.5 + s * 2.1).sin() * (d.x * 1.9 + s).cos();
        let rr = r * (1.0 + 0.11 * lump);
        addv(center, v(d.x * rr, d.y * rr * 0.82, d.z * rr))
    }).collect();
    // smooth (vertex) normals over the shared topology → rounded shading instead of faceted
    let mut norms = vec![v(0.0, 0.0, 0.0); pv.len()];
    for t in &base.tris {
        let nf = cross(sub(pv[t[1] as usize], pv[t[0] as usize]), sub(pv[t[2] as usize], pv[t[0] as usize]));
        for &vi in t { norms[vi as usize] = addv(norms[vi as usize], nf); }
    }
    for nn in &mut norms { *nn = norm(*nn); }
    Mesh3 { emissive: 0.0, verts: pv, norms, tris: base.tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }
}

/// A zen cairn — a stack of rocks of decreasing size, jittered a little off-axis. Returns one mesh per
/// stone (the renderer depth-sorts them together with everything else).
pub fn cairn(base: Vec3, stones: usize, r0: f32, seed: u32, color: Color32) -> Vec<Mesh3> {
    let mut out = Vec::new();
    let mut y = base.y;
    for k in 0..stones.max(1) {
        let r = r0 * (1.0 - 0.13 * k as f32).max(0.25);
        let jx = (hash3(k as i32, seed as i32, 3) - 0.5) * r * 0.3;
        let jz = (hash3(k as i32, seed as i32, 9) - 0.5) * r * 0.3;
        out.push(rock(v(base.x + jx, y + r, base.z + jz), r, seed.wrapping_add(k as u32 * 97), color));
        y += r * 1.8;
    }
    out
}

/// Starry "bacteria" speckle inside a pearl: many tiny self-lit cells that twinkle, returned as low-poly
/// emissive spheres (caught by the bloom pass) so the pearl reads as a living point-cloud rather than a solid
/// orb. `lit` (0..1) is the resonance brightness — e.g. grows as the camera approaches the node.
pub fn speckle_cells(center: Vec3, r: f32, seed: u32, t: f32, col: Color32, n: usize, lit: f32) -> Vec<Mesh3> {
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        // uniform-ish point in the ball
        let a = hash3(k as i32, seed as i32, 1) * std::f32::consts::TAU;
        let u = hash3(k as i32, seed as i32, 2) * 2.0 - 1.0;
        let rad = r * (0.12 + 0.84 * hash3(k as i32, seed as i32, 3).cbrt());
        let s = (1.0 - u * u).max(0.0).sqrt();
        let p = v(center.x + rad * s * a.cos(), center.y + rad * u, center.z + rad * s * a.sin());
        // twinkle: each cell has its own phase + speed (the living-pearl flicker)
        let phase = hash3(k as i32, seed as i32, 7) * std::f32::consts::TAU;
        let speed = 0.8 + 1.8 * hash3(k as i32, seed as i32, 8);
        let tw = 0.30 + 0.70 * (0.5 + 0.5 * (t * speed + phase).sin());
        // a fraction burn white-hot (stars) among the tinted cells → the starry-speckle look
        let star = hash3(k as i32, seed as i32, 4) > 0.72;
        let cc = if star { Color32::from_rgb(228, 244, 255) } else { col };
        let mut m = uv_sphere(p, r * 0.05 * if star { 1.25 } else { 1.0 }, 4, 3, cc);
        m.emissive = (0.25 + 1.35 * tw) * (0.20 + 0.95 * lit) * if star { 1.3 } else { 1.0 };
        out.push(m);
    }
    out
}

/// Compact 5×7 column-major font (bit0 = top row). Covers upper+lowercase, digits and common punctuation;
/// unknowns → blank.
pub fn glyph5x7(ch: char) -> [u8; 5] {
    match ch {
        '0' => [0x3E,0x51,0x49,0x45,0x3E], '1' => [0x00,0x42,0x7F,0x40,0x00],
        '2' => [0x42,0x61,0x51,0x49,0x46], '3' => [0x21,0x41,0x45,0x4B,0x31],
        '4' => [0x18,0x14,0x12,0x7F,0x10], '5' => [0x27,0x45,0x45,0x45,0x39],
        '6' => [0x3C,0x4A,0x49,0x49,0x30], '7' => [0x01,0x71,0x09,0x05,0x03],
        '8' => [0x36,0x49,0x49,0x49,0x36], '9' => [0x06,0x49,0x49,0x29,0x1E],
        'A' => [0x7E,0x11,0x11,0x11,0x7E], 'B' => [0x7F,0x49,0x49,0x49,0x36],
        'C' => [0x3E,0x41,0x41,0x41,0x22], 'D' => [0x7F,0x41,0x41,0x22,0x1C],
        'E' => [0x7F,0x49,0x49,0x49,0x41], 'F' => [0x7F,0x09,0x09,0x09,0x01],
        'G' => [0x3E,0x41,0x49,0x49,0x7A], 'H' => [0x7F,0x08,0x08,0x08,0x7F],
        'I' => [0x00,0x41,0x7F,0x41,0x00], 'J' => [0x20,0x40,0x41,0x3F,0x01],
        'K' => [0x7F,0x08,0x14,0x22,0x41], 'L' => [0x7F,0x40,0x40,0x40,0x40],
        'M' => [0x7F,0x02,0x0C,0x02,0x7F], 'N' => [0x7F,0x04,0x08,0x10,0x7F],
        'O' => [0x3E,0x41,0x41,0x41,0x3E], 'P' => [0x7F,0x09,0x09,0x09,0x06],
        'Q' => [0x3E,0x41,0x51,0x21,0x5E], 'R' => [0x7F,0x09,0x19,0x29,0x46],
        'S' => [0x46,0x49,0x49,0x49,0x31], 'T' => [0x01,0x01,0x7F,0x01,0x01],
        'U' => [0x3F,0x40,0x40,0x40,0x3F], 'V' => [0x1F,0x20,0x40,0x20,0x1F],
        'W' => [0x7F,0x20,0x18,0x20,0x7F], 'X' => [0x63,0x14,0x08,0x14,0x63],
        'Y' => [0x07,0x08,0x70,0x08,0x07], 'Z' => [0x61,0x51,0x49,0x45,0x43],
        'a' => [0x20,0x54,0x54,0x54,0x78], 'b' => [0x7F,0x48,0x44,0x44,0x38],
        'c' => [0x38,0x44,0x44,0x44,0x20], 'd' => [0x38,0x44,0x44,0x48,0x7F],
        'e' => [0x38,0x54,0x54,0x54,0x18], 'f' => [0x08,0x7E,0x09,0x01,0x02],
        'g' => [0x0C,0x52,0x52,0x52,0x3E], 'h' => [0x7F,0x08,0x04,0x04,0x78],
        'i' => [0x00,0x44,0x7D,0x40,0x00], 'j' => [0x20,0x40,0x44,0x3D,0x00],
        'k' => [0x7F,0x10,0x28,0x44,0x00], 'l' => [0x00,0x41,0x7F,0x40,0x00],
        'm' => [0x7C,0x04,0x18,0x04,0x78], 'n' => [0x7C,0x08,0x04,0x04,0x78],
        'o' => [0x38,0x44,0x44,0x44,0x38], 'p' => [0x7C,0x14,0x14,0x14,0x08],
        'q' => [0x08,0x14,0x14,0x18,0x7C], 'r' => [0x7C,0x08,0x04,0x04,0x08],
        's' => [0x48,0x54,0x54,0x54,0x20], 't' => [0x04,0x3F,0x44,0x40,0x20],
        'u' => [0x3C,0x40,0x40,0x20,0x7C], 'v' => [0x1C,0x20,0x40,0x20,0x1C],
        'w' => [0x3C,0x40,0x30,0x40,0x3C], 'x' => [0x44,0x28,0x10,0x28,0x44],
        'y' => [0x0C,0x50,0x50,0x50,0x3C], 'z' => [0x44,0x64,0x54,0x4C,0x44],
        '!' => [0x00,0x00,0x5F,0x00,0x00], '?' => [0x02,0x01,0x51,0x09,0x06],
        '.' => [0x00,0x00,0x60,0x60,0x00], ',' => [0x00,0x00,0x50,0x30,0x00],
        ':' => [0x00,0x36,0x36,0x00,0x00], ';' => [0x00,0x56,0x36,0x00,0x00],
        '\'' => [0x00,0x05,0x03,0x00,0x00], '"' => [0x00,0x07,0x00,0x07,0x00],
        '-' => [0x08,0x08,0x08,0x08,0x08], '_' => [0x40,0x40,0x40,0x40,0x40],
        '—' | '–' => [0x08,0x08,0x08,0x08,0x08], // em/en dash → dash
        '+' => [0x08,0x08,0x3E,0x08,0x08], '=' => [0x14,0x14,0x14,0x14,0x14],
        '*' => [0x14,0x08,0x3E,0x08,0x14], '/' => [0x20,0x10,0x08,0x04,0x02],
        '\\' => [0x02,0x04,0x08,0x10,0x20], '(' => [0x00,0x1C,0x22,0x41,0x00],
        ')' => [0x00,0x41,0x22,0x1C,0x00], '[' => [0x00,0x7F,0x41,0x41,0x00],
        ']' => [0x00,0x41,0x41,0x7F,0x00], '<' => [0x08,0x14,0x22,0x41,0x00],
        '>' => [0x00,0x41,0x22,0x14,0x08], '@' => [0x32,0x49,0x79,0x41,0x3E],
        '&' => [0x36,0x49,0x55,0x22,0x50], '%' => [0x23,0x13,0x08,0x64,0x62],
        '#' => [0x14,0x7F,0x14,0x7F,0x14], '$' => [0x24,0x2A,0x7F,0x2A,0x12],
        '·' => [0x00,0x18,0x18,0x00,0x00],
        _ => [0x00,0x00,0x00,0x00,0x00],
    }
}

/// Camera-facing glowing TEXT for a pearl: rasterizes `text` through the 5×7 font into emissive cells laid on
/// the (right, up) billboard plane, floated slightly toward the camera in front of the node. `lit` (0..1) is
/// the resonance — the text resolves out of the speckle only as the camera approaches. `scroll` shifts it up.
/// Per-scene text-animation idiom for physical/scene labels — each scene animates its glyph-cells in its own
/// character (water ripple, leaf sway, ice shimmer, …). `Plain` is the original uniform twinkle.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TextAnim { #[default] Plain, Ripple, Sway, Shimmer, Drift, Fade, Pulse }

/// Per-cell animation: given the column fraction `cf` (0..1 across the text), char index `ci`, time `t`, and
/// cell `scale`, returns `(d_up, d_right, emissive_mul)` — a small world-space offset (proportional to scale)
/// and a brightness multiplier. Pure, so the idioms are unit-testable. Amplitudes stay small: text must read.
pub fn anim_cell(mode: TextAnim, t: f32, cf: f32, ci: usize, scale: f32) -> (f32, f32, f32) {
    match mode {
        TextAnim::Plain => { let tw = 0.55 + 0.45 * (0.5 + 0.5 * (t * 2.6 + ci as f32 * 0.7).sin()); (0.0, 0.0, 0.5 + 1.6 * tw) }
        // Atlantis: glyphs bob on a traveling water wave, with a caustic brightness ripple.
        TextAnim::Ripple => (0.16 * scale * (t * 3.0 + cf * 7.0).sin(), 0.0, 1.15 + 0.75 * (0.5 + 0.5 * (t * 4.0 + cf * 10.0).sin())),
        // Forest/Treehouse/Canopy: whole line sways sideways like leaves in a breeze.
        TextAnim::Sway => (0.05 * scale * (t * 1.1).sin(), 0.15 * scale * (t * 1.4 + cf * 1.5).sin(), 1.25 + 0.55 * (0.5 + 0.5 * (t * 2.0 + ci as f32).sin())),
        // Frozen Bifrost: crystalline glints — occasional sharp per-cell sparkle.
        TextAnim::Shimmer => { let g = ((t * 5.0 + ci as f32 * 2.3).sin() * 0.5 + 0.5).powi(6); (0.0, 0.0, 1.0 + 2.4 * g) }
        // Cloudscape: gentle upward drift.
        TextAnim::Drift => (0.10 * scale * (t * 1.0 + cf * 3.0).sin() + 0.04 * scale * (t * 0.5).sin(), 0.0, 1.2 + 0.6 * (0.5 + 0.5 * (t * 1.5 + cf * 4.0).sin())),
        // Mistfall: glyphs breathe in and out like mist.
        TextAnim::Fade => (0.0, 0.0, 0.7 + 1.5 * (0.5 + 0.5 * (t * 1.6 + cf * 3.0).sin())),
        // Folding@home: a bright band of "compute" sweeps across the text.
        TextAnim::Pulse => { let band = (cf - (t * 0.28).fract()).abs(); (0.0, 0.0, 1.1 + 1.3 * (-band * band * 26.0).exp()) }
    }
}

/// Per-dot sphere resolution for `label_cells`'s dot-matrix text. Was 4×3 (≈12 triangles) — visibly
/// faceted/blocky up close, the literal "dots are too low-res" complaint about the physical UI's
/// message plaque. 6×4 (≈24 triangles) reads as a genuinely round dot instead of a diamond-ish blob,
/// roughly doubling per-dot triangle count — a deliberate moderate bump, not maxed out, since a single
/// 32-char plaque line can already place 100+ dots (up to 5×7×32) and this scene renders many labels.
const DOT_SEGMENTS: usize = 6;
const DOT_RINGS: usize = 4;

pub fn label_cells(center: Vec3, right: Vec3, up: Vec3, toward_cam: Vec3, text: &str,
                   scale: f32, t: f32, col: Color32, lit: f32, scroll: f32, anim: TextAnim) -> Vec<Mesh3> {
    let chars: Vec<char> = text.chars().take(32).collect();
    if chars.is_empty() || lit <= 0.02 { return Vec::new(); }
    let w = chars.len() as f32 * 6.0 * scale;
    let h = 7.0 * scale;
    let base = addv(addv(center, mul(toward_cam, 0.34)), mul(up, scroll)); // float in front, scrolled up
    let denom = (chars.len().saturating_sub(1)).max(1) as f32;
    let mut out = Vec::new();
    for (ci, ch) in chars.iter().enumerate() {
        let g = glyph5x7(*ch);
        let cf = ci as f32 / denom;
        let (d_up, d_right, emis) = anim_cell(anim, t, cf, ci, scale);
        for x in 0..5usize {
            let colb = g[x];
            for y in 0..7usize {
                if (colb >> y) & 1 == 1 {
                    let px = (ci as f32 * 6.0 + x as f32) * scale - w * 0.5 + d_right;
                    let py = h * 0.5 - y as f32 * scale + d_up;
                    let p = addv(addv(base, mul(right, px)), mul(up, py));
                    let mut m = uv_sphere(p, scale * 0.46, DOT_SEGMENTS, DOT_RINGS, col);
                    m.emissive = emis * lit;
                    out.push(m);
                }
            }
        }
    }
    out
}

#[inline] fn shade(albedo: Color32, light: &Light, ny: f32, ndotl: f32) -> Color32 {
    let h = 0.5 + 0.5 * ny; // 1 facing the sky, 0 facing the ground
    let ch = |a: u8, sky: u8, gnd: u8, sun: u8| {
        let amb = (gnd as f32 * (1.0 - h) + sky as f32 * h) / 255.0 * light.ambient;
        let lit = amb + ndotl * (sun as f32 / 255.0);
        (((a as f32 / 255.0) * lit) * 1.65 * 255.0).min(255.0) as u8
    };
    Color32::from_rgb(
        ch(albedo.r(), light.sky.r(), light.ground.r(), light.color.r()),
        ch(albedo.g(), light.sky.g(), light.ground.g(), light.color.g()),
        ch(albedo.b(), light.sky.b(), light.ground.b(), light.color.b()),
    )
}

/// Render `meshes` through `cam` (same `cx,cy,scale` as the spatial view) into one lit, depth-sorted
/// `egui::Mesh`. Per-vertex albedo (`vcol` or `color`), hemispherical sun shading, per-mesh `offset` applied
/// (so floating objects move without rebuilding); triangles behind the camera are dropped; painted far→near.
pub fn render(meshes: &[Mesh3], cam: &OrbitCam, cx: f32, cy: f32, scale: f32, light: &Light) -> Mesh {
    let l = norm(light.dir);
    let mut tris: Vec<(f32, [Pos2; 3], [Color32; 3])> = Vec::new();
    for m in meshes {
        for tri in &m.tris {
            let idx = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            let mut sp = [Pos2::ZERO; 3];
            let mut dep = [0f32; 3];
            let mut cols = [Color32::BLACK; 3];
            let mut ok = true;
            for k in 0..3 {
                match cam.project(addv(m.verts[idx[k]], m.offset), cx, cy, scale) {
                    Some((sx, sy, d)) => { sp[k] = Pos2::new(sx, sy); dep[k] = d; }
                    None => { ok = false; break; }
                }
                let n = norm(m.norms[idx[k]]);
                let base = m.vcol.as_ref().map_or(m.color, |vc| vc[idx[k]]);
                cols[k] = shade(base, light, n.y, dot(n, l).max(0.0));
            }
            if !ok { continue; }
            tris.push(((dep[0] + dep[1] + dep[2]) / 3.0, sp, cols));
        }
    }
    tris.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)); // far → near
    let mut mesh = Mesh::default();
    for (_, sp, cols) in tris {
        let i = mesh.vertices.len() as u32;
        for k in 0..3 { mesh.colored_vertex(sp[k], cols[k]); }
        mesh.add_triangle(i, i + 1, i + 2);
    }
    mesh
}

/// A vertical ring (a moon-gate portal) in the XY plane, tube radius `tube_r` around major radius `big_r`.
pub fn torus(center: Vec3, big_r: f32, tube_r: f32, nmaj: usize, nmin: usize, color: Color32) -> Mesh3 {
    let (nmaj, nmin) = (tess(nmaj, 3), tess(nmin, 3));
    let (mut verts, mut norms, mut tris) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..=nmaj {
        let u = std::f32::consts::TAU * i as f32 / nmaj as f32;
        let cdir = v(u.cos(), u.sin(), 0.0); // around the ring, in XY
        for j in 0..=nmin {
            let vv = std::f32::consts::TAU * j as f32 / nmin as f32;
            let (cv, sv) = (vv.cos(), vv.sin());
            let n = addv(mul(cdir, cv), v(0.0, 0.0, sv));
            verts.push(addv(center, addv(mul(cdir, big_r + tube_r * cv), v(0.0, 0.0, tube_r * sv))));
            norms.push(norm(n));
        }
    }
    let row = nmin + 1;
    for i in 0..nmaj {
        for j in 0..nmin {
            let a = (i * row + j) as u32;
            let c = ((i + 1) * row + j) as u32;
            tris.push([a, c, a + 1]);
            tris.push([a + 1, c, c + 1]);
        }
    }
    Mesh3 { emissive: 0.0, verts, norms, tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }
}

/// A flat disc on the XZ plane facing up — ground / water / a floating-island top.
pub fn disc(center: Vec3, r: f32, segs: usize, color: Color32) -> Mesh3 {
    let segs = tess(segs, 3);
    let (mut verts, mut norms, mut tris) = (vec![center], vec![v(0.0, 1.0, 0.0)], Vec::new());
    for j in 0..=segs {
        let th = std::f32::consts::TAU * j as f32 / segs as f32;
        verts.push(addv(center, v(r * th.cos(), 0.0, r * th.sin())));
        norms.push(v(0.0, 1.0, 0.0));
    }
    for j in 0..segs as u32 { tris.push([0, j + 1, j + 2]); }
    Mesh3 { emissive: 0.0, verts, norms, tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }
}

/// Local-clock hour (0..24, fractional). Uses the OS timezone so the scene tracks the real time of day.
/// `LOGSCOPE_HOUR` overrides it (for deterministic renders / forcing a time of day).
fn local_hour() -> f32 {
    if let Ok(h) = std::env::var("LOGSCOPE_HOUR") { if let Ok(v) = h.trim().parse::<f32>() { return v.rem_euclid(24.0); } }
    use chrono::Timelike;
    let now = chrono::Local::now();
    now.hour() as f32 + now.minute() as f32 / 60.0 + now.second() as f32 / 3600.0
}

/// A 3D sun direction from the real LOCAL clock: a low sun at dawn/dusk arcing overhead at noon, dipping
/// below at night. Returns a unit direction pointing TOWARD the sun.
pub fn sun_dir_3d() -> Vec3 {
    let h = local_hour();
    let elev = (std::f32::consts::PI * ((h - 6.0) / 12.0)).sin(); // <0 at night
    let az = std::f32::consts::TAU * ((h - 6.0) / 24.0);
    norm(v(az.cos() * 0.7, elev.max(0.05) + 0.30, az.sin() * 0.7)) // keep a little fill at night
}

/// Day-cycle lighting from the local clock: `strength` 0..1 (sun brightness — peaks at midday, ~0 at night),
/// `warmth` -1..1 (warm orange near sunrise/sunset, neutral at noon, cool blue at night), and `ambient` mul.
/// Lets the scene's sun colour, exposure and fill track the real time of day on top of the per-theme grade.
pub fn sun_clock() -> (f32, f32, f32) {
    let h = local_hour();
    // elevation 0..1 over the day; smooth, with civil-twilight tails
    let elev = (std::f32::consts::PI * ((h - 6.0) / 12.0)).sin();
    let day = elev.max(0.0);                       // 0 at/under horizon, 1 at noon
    let strength = (0.12 + 0.88 * day).clamp(0.10, 1.0);
    // warmth: hottest when the sun is low but up (dawn/dusk), neutral high noon, cool at night
    let low_sun = (1.0 - (elev.abs())).clamp(0.0, 1.0); // 1 near the horizon
    let warmth = if elev > 0.04 { low_sun * 0.85 } else { -0.55 * (1.0 - day) }; // warm by day-edge, cool by night
    let ambient = 0.56 + 0.62 * day;               // dim fill at night, more at midday
    (strength, warmth, ambient)
}

/// Build a lit 3D scene for the conversation graph: a shaded orb per node (coloured by `node_cols`, the
/// active tip in `hot`) and a tube per parent edge. Drop it into `render` with the spatial camera.
pub fn graph_scene(centers: &[Vec3], node_cols: &[Color32], edges: &[(usize, usize)], active: Option<usize>,
    node_r: f32, edge_r: f32, hot: Color32, edge: Color32) -> Vec<Mesh3> {
    let mut out = Vec::with_capacity(centers.len() + edges.len());
    for &(a, b) in edges {
        if a < centers.len() && b < centers.len() { out.push(tube(centers[a], centers[b], edge_r, 6, edge)); }
    }
    for (i, &c) in centers.iter().enumerate() {
        let col = if Some(i) == active { hot } else { *node_cols.get(i).unwrap_or(&hot) };
        out.push(uv_sphere(c, node_r, 10, 7, col));
    }
    out
}

/// A small foliage puff — several leaf quads at jittered positions/orientations around `center`. The node
/// body for leaf-themed graphs (the tree library).
/// An inward-facing circular wall — a cylinder shell seen from the inside. This is the enclosure that turns a
/// graph adrift in a void into a room: `segs` facets around, normals point inward so the interior catches the
/// light while the far side fades into fog. The wall rises `height` from `center.y`.
/// A flat quad in the (right, up) plane centred at `center`, `w`×`h`. Shared helper for windows, signs, panes.
pub fn panel(center: Vec3, right: Vec3, up: Vec3, w: f32, h: f32, color: Color32) -> Mesh3 {
    let r = mul(norm(right), w * 0.5);
    let u = mul(norm(up), h * 0.5);
    let n = norm(cross(right, up));
    let verts = vec![sub(sub(center, r), u), sub(addv(center, r), u), addv(addv(center, r), u), addv(sub(center, r), u)];
    let norms = vec![n; 4];
    let tris = vec![[0u32, 1, 2], [0, 2, 3]];
    Mesh3 { emissive: 0.0, verts, norms, tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }
}

/// A tall arched window: a `w`×`h` rectangular pane topped by a semicircular arch (radius w/2), all in the
/// (right, up) plane with `center` at the BOTTOM-centre. One mesh — used for the palace clerestory and reusable
/// for any themed opening. Total height ≈ h + w/2.
pub fn arch_window(center: Vec3, right: Vec3, up: Vec3, w: f32, h: f32, color: Color32) -> Mesh3 {
    arch_window_styled(center, right, up, w, h, 0, color)
}

/// Crown outline for a styled arch: points `(du, dv)` from the RIGHT springing (+hw,0) up over the top to
/// the LEFT springing (-hw,0), `dv` measured ABOVE the springing line. Ordered by angle about the springing
/// midpoint so a triangle-fan from that point reproduces the exact silhouette (incl. concavities/lobes).
/// Styles: 0 Round · 1 Segmental · 2 Pointed(equilateral) · 3 Lancet · 4 Tudor(flattened point) · 5 Ogee ·
/// 6 Horseshoe · 7 Multifoil/Trefoil. (Roman/Round is the classic default.)
pub fn arch_crown(style: u32, hw: f32, segs: u32) -> Vec<[f32; 2]> {
    use std::f32::consts::PI;
    let n = segs.max(10) & !1;                 // even, so left/right halves split cleanly
    let mut p = Vec::with_capacity(n as usize + 2);
    // a single-radius pointed arch with apex height `a`: arc centred on the springing axis.
    let pointed = |a: f32, bulge: f32, p: &mut Vec<[f32; 2]>| {
        let cx = (hw * hw - a * a) / (2.0 * hw); let r = hw - cx;   // passes (hw,0) & (0,a)
        let half = n / 2;
        for j in 0..=half { let t = j as f32 / half as f32; let du = hw * (1.0 - t);
            let dv = (r * r - (du - cx) * (du - cx)).max(0.0).sqrt();
            p.push([du + bulge * (PI * t).sin(), dv]); }                 // right half (bulge → ogee belly)
        for j in 1..=half { let t = j as f32 / half as f32; let du = -hw * t;
            let dv = (r * r - (-du - cx) * (-du - cx)).max(0.0).sqrt();
            p.push([du - bulge * (PI * t).sin(), dv]); }                 // left half (mirror)
    };
    match style {
        1 => { let a = 52f32.to_radians(); let r = hw / a.sin(); let base = r * a.cos(); // Segmental: shallow arc
               for j in 0..=n { let t = a - 2.0 * a * (j as f32 / n as f32); p.push([r * t.sin(), r * t.cos() - base]); } }
        2 => pointed(1.732 * hw, 0.0, &mut p),                  // Pointed — equilateral (apex √3·hw)
        3 => pointed(2.236 * hw, 0.0, &mut p),                  // Lancet — tall narrow point (apex √5·hw)
        4 => pointed(0.62 * hw, 0.0, &mut p),                   // Tudor — flattened/depressed point
        5 => pointed(1.30 * hw, 0.20 * hw, &mut p),             // Ogee — pointed with an S-belly
        6 => { let e = 0.22 * hw; let r = (hw * hw + e * e).sqrt(); // Horseshoe — >180° arc, tucks at the base
               let th0 = (-e / r).asin(); let th1 = PI - th0;
               for j in 0..=n { let th = th0 + (th1 - th0) * (j as f32 / n as f32); p.push([r * th.cos(), e + r * th.sin()]); } }
        7 => { for j in 0..=n { let th = PI * j as f32 / n as f32;   // Multifoil — generic lobed round arch
                   let rad = hw * (1.0 + 0.15 * th.sin() * (4.0 * th).cos());
                   p.push([rad * th.cos(), rad * th.sin()]); } }
        8 => { let e = 0.5 * hw; let r = (hw * hw + e * e).sqrt();    // Keyhole — circular top, pinched sides
               let th0 = (-e / r).asin(); let th1 = PI - th0;
               for j in 0..=n { let th = th0 + (th1 - th0) * (j as f32 / n as f32); p.push([r * th.cos(), e + r * th.sin()]); } }
        9 => { for j in 0..=n { let th = PI * j as f32 / n as f32;   // Trefoil — three lobes
                   let rad = hw * (1.0 + 0.22 * th.sin() * (3.0 * th).cos());
                   p.push([rad * th.cos(), rad * th.sin()]); } }
        10 => { for j in 0..=n { let th = PI * j as f32 / n as f32;  // Cinquefoil — five lobes
                    let rad = hw * (1.0 + 0.16 * th.sin() * (5.0 * th).cos());
                    p.push([rad * th.cos(), rad * th.sin()]); } }
        11 => { for j in 0..=n { let th = PI * j as f32 / n as f32; p.push([hw * th.cos(), 0.66 * hw * th.sin()]); } } // Elliptical/Basket-handle — wide flat half-ellipse
        12 => { let a = 1.3 * hw;                                     // Parabolic
                for j in 0..=n { let du = hw - 2.0 * hw * (j as f32 / n as f32); p.push([du, a * (1.0 - (du / hw) * (du / hw))]); } }
        13 => { let a = 1.25 * hw; let k = 1.6f32;                   // Catenary — inverted hanging chain
                let ch = k.cosh();
                for j in 0..=n { let du = hw - 2.0 * hw * (j as f32 / n as f32); let dv = a * (ch - (k * du / hw).cosh()) / (ch - 1.0); p.push([du, dv]); } }
        14 => { let sh = 0.78 * hw;                                   // Three-centered — flat crown, rounder shoulders
                for j in 0..=n { let th = PI * j as f32 / n as f32; let s = th.sin(); p.push([hw * th.cos(), sh * s.powf(0.62)]); } }
        _ => { for j in 0..=n { let th = PI * j as f32 / n as f32; p.push([hw * th.cos(), hw * th.sin()]); } } // Round
    }
    p
}

/// A flat window/opening pane: rectangular body up to `h`, then a styled arch crown on top (see `arch_crown`).
pub fn arch_window_styled(center: Vec3, right: Vec3, up: Vec3, w: f32, h: f32, style: u32, color: Color32) -> Mesh3 {
    let rr = norm(right); let uu = norm(up); let n = norm(cross(right, up));
    let hw = w * 0.5;
    let at = |du: f32, dv: f32| addv(addv(center, mul(rr, du)), mul(uu, dv));
    let mut verts = vec![at(-hw, 0.0), at(hw, 0.0), at(hw, h), at(-hw, h)];
    let mut tris = vec![[0u32, 1, 2], [0, 2, 3]];
    let cidx = verts.len() as u32;
    verts.push(at(0.0, h)); // arch fan centre (springing midpoint)
    let crown = arch_crown(style, hw, 20);
    for c in &crown { verts.push(at(c[0], h + c[1])); }
    let m = crown.len() as u32;
    for j in 0..m - 1 { tris.push([cidx, cidx + 1 + j, cidx + 2 + j]); }
    let norms = vec![n; verts.len()];
    Mesh3 { emissive: 0.0, verts, norms, tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }
}

/// A COMPOSITE (multi-opening) window assembly — a large central arch flanked by side openings, the forms a
/// single `arch_window` can't make. `base` is the bottom-centre (sill) of the bay; openings rise along `up`.
/// kind 0 Serliana/Palladian (central round arch + two flat-top side lights) · 1 Venetian (central + two
/// smaller round arches) · 2 Triumphal (tall central arch + two short side arches + an entablature band).
/// Returns `(mesh, is_glass)`: glass openings glow (emissive), the mullions/piers/lintels are dark stone.
pub fn composite_window(kind: u32, base: Vec3, right: Vec3, up: Vec3, w: f32, h: f32, color: Color32) -> Vec<(Mesh3, bool)> {
    let rr = norm(right); let uu = norm(up);
    let mut out: Vec<(Mesh3, bool)> = Vec::new();
    let rect = |c: Vec3, ww: f32, hh: f32| panel(addv(c, mul(uu, hh * 0.5)), rr, uu, ww, hh, color); // pane rising from c
    match kind {
        0 => { // SERLIANA / PALLADIAN — round arch centre, flat-top side lights, slim mullions
            let cw = w * 0.40; let ch = h * 0.54;
            out.push((arch_window_styled(base, rr, uu, cw, ch, 0, color), true));
            let sw = w * 0.20; let sh = h * 0.52;
            for s in [-1.0f32, 1.0] {
                let c = addv(base, mul(rr, s * (cw * 0.5 + sw * 0.5 + w * 0.05)));
                out.push((rect(c, sw, sh), true));
                out.push((rect(addv(base, mul(rr, s * (cw * 0.5 + w * 0.025))), w * 0.03, ch), false)); // mullion
                out.push((rect(addv(c, mul(rr, s * (sw * 0.5 + w * 0.02))), w * 0.03, sh), false));      // outer jamb
            }
        }
        1 => { // VENETIAN — central arch flanked by two SMALLER arches
            let cw = w * 0.42; let ch = h * 0.52;
            out.push((arch_window_styled(base, rr, uu, cw, ch, 0, color), true));
            let sw = w * 0.20; let sh = h * 0.40;
            for s in [-1.0f32, 1.0] {
                let c = addv(base, mul(rr, s * (cw * 0.5 + sw * 0.5 + w * 0.05)));
                out.push((arch_window_styled(c, rr, uu, sw, sh, 0, color), true));
                out.push((rect(addv(base, mul(rr, s * (cw * 0.5 + w * 0.025))), w * 0.03, ch), false)); // mullion
            }
        }
        _ => { // TRIUMPHAL — tall central arch, two short side arches, heavy piers + entablature band on top
            let cw = w * 0.40; let ch = h * 0.56;
            out.push((arch_window_styled(base, rr, uu, cw, ch, 0, color), true));
            let sw = w * 0.16; let sh = h * 0.34;
            for s in [-1.0f32, 1.0] {
                let c = addv(base, mul(rr, s * (cw * 0.5 + sw * 0.5 + w * 0.09)));
                out.push((arch_window_styled(c, rr, uu, sw, sh, 0, color), true));
                out.push((rect(addv(base, mul(rr, s * (cw * 0.5 + w * 0.045))), w * 0.06, ch), false)); // thick pier
                out.push((rect(addv(c, mul(rr, s * (sw * 0.5 + w * 0.05))), w * 0.06, sh * 1.4), false)); // outer pier
            }
            // entablature: a stone band across the whole bay above the side arches
            let band_y = h * 0.40;
            out.push((panel(addv(base, mul(uu, band_y + h * 0.05)), rr, uu, w * 0.98, h * 0.1, color), false));
        }
    }
    out
}
/// Leaded STAINED-GLASS arch: a wide glass arch (`arch_window`) tinted glass→lo, divided into tiles by a grid
/// of thin metal CAMES (`cols`×`rows`, the facet density). Returns the glass pane + every came as separate
/// meshes (glass = WATER+emissive, cames = STONE). `metal` is the leading colour. Shared across themes.
pub fn leaded_arch(center: Vec3, right: Vec3, up: Vec3, w: f32, h: f32,
                   cols: usize, rows: usize, glass: Color32, lo: Color32, metal: Color32) -> Vec<Mesh3> {
    let mut out = Vec::new();
    let mut g = arch_window(center, right, up, w, h, glass);
    tint_up(&mut g, glass, lo); g.mat = 3.0; g.emissive = 1.15;
    out.push(g);
    let rr = norm(right); let uu = norm(up); let n = norm(cross(right, up));
    let hw = w * 0.5;
    let front = mul(n, 0.05);                                    // cames sit just in front of the glass
    let at = |du: f32, dv: f32| addv(addv(addv(center, mul(rr, du)), mul(uu, dv)), front);
    let mut came = |c: Vec3, cw: f32, ch: f32| {
        let mut m = panel(c, rr, uu, cw, ch, metal); m.mat = 2.0; m.emissive = 0.03; out.push(m);
    };
    // outer frame (around the rectangular body)
    came(at(-hw, h * 0.5), 0.08, h); came(at(hw, h * 0.5), 0.08, h); came(at(0.0, 0.0), w, 0.08);
    // vertical + horizontal cames → the tile grid
    for i in 1..cols { came(at(-hw + w * i as f32 / cols as f32, h * 0.5), 0.05, h); }
    for j in 1..rows { came(at(0.0, h * j as f32 / rows as f32), w, 0.05); }
    // spring-line came + radial cames fanning across the arch (sections the rounded top into facets too)
    came(at(0.0, h), w, 0.07);
    let fan = cols.max(3);
    for i in 1..fan {
        let th = std::f32::consts::PI * i as f32 / fan as f32;
        let (dx, dy) = (hw * th.cos(), hw * th.sin());
        let mid = at(dx * 0.5, h + dy * 0.5);
        let dir = norm(addv(mul(rr, dx), mul(uu, dy)));          // radius direction in the glass plane
        let perp = norm(cross(n, dir));                          // in-plane, perpendicular to the radius
        let len = (dx * dx + dy * dy).sqrt();
        let mut m = panel(mid, dir, perp, len, 0.05, metal); m.mat = 2.0; m.emissive = 0.03; out.push(m);
    }
    out
}

/// A single flat triangle (for mosaic facets). Normal from the winding.
pub fn tri(a: Vec3, b: Vec3, cc: Vec3, color: Color32) -> Mesh3 {
    let n = norm(cross(sub(b, a), sub(cc, a)));
    Mesh3 { emissive: 0.0, verts: vec![a, b, cc], norms: vec![n; 3], tris: vec![[0, 1, 2]], color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }
}

/// A real leaded STAINED-GLASS arch: a dark leading backing + a MOSAIC of many small glass tiles, each a
/// different shade drawn from `palette`, separated by thin leading gaps, with moderate + varied glow (so it
/// reads as backlit coloured glass, not a flat light). The rectangular body is a `cols`×`rows` tile grid; the
/// rounded top is a fan of inset wedge tiles. Returns backing + every tile. Shared across themes.
pub fn stained_glass(center: Vec3, right: Vec3, up: Vec3, w: f32, h: f32,
                     cols: usize, rows: usize, palette: &[Color32], seed: u32) -> Vec<Mesh3> {
    use std::f32::consts::PI;
    let rr = norm(right); let uu = norm(up); let n = norm(cross(right, up));
    let hw = w * 0.5;
    let front = mul(n, 0.035);
    let at = |du: f32, dv: f32| addv(addv(addv(center, mul(rr, du)), mul(uu, dv)), front);
    let hsh = |x: i32, y: i32| { let mut k = (seed ^ (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77)) as u32; k ^= k >> 13; k = k.wrapping_mul(0x2C1B_3C6D); k ^= k >> 11; (k & 0xFFFF) as f32 / 65535.0 };
    let pick = |x: i32, y: i32| palette[((hsh(x, y) * palette.len() as f32) as usize).min(palette.len() - 1)];
    let mut out = Vec::new();
    // dark leading backing, slightly larger than the glass → shows as the came lines between tiles
    let mut back = arch_window(center, right, up, w * 1.03, h + 0.07, Color32::from_rgb(0x09, 0x10, 0x14));
    back.mat = 2.0; back.emissive = 0.05; out.push(back);
    let gap = 0.07;
    let cw = w / cols as f32; let ch = h / rows as f32;
    for i in 0..cols { for j in 0..rows {
        let du = -hw + (i as f32 + 0.5) * cw;
        let dv = (j as f32 + 0.5) * ch;
        let mut t = panel(at(du, dv), rr, uu, (cw - gap).max(0.04), (ch - gap).max(0.04), pick(i as i32, j as i32));
        t.mat = 3.0; t.emissive = 0.35 + 0.5 * hsh(i as i32 + 7, j as i32 * 3 + 1); // varied glow → not uniform
        out.push(t);
    }}
    // arch: a fan of inset wedge tiles (each a different shade) → the rounded top is mosaicked too
    let segs = (cols * 2).max(7);
    let cen = at(0.0, h);
    for k in 0..segs {
        let th0 = PI * k as f32 / segs as f32; let th1 = PI * (k + 1) as f32 / segs as f32;
        let p0 = at(hw * th0.cos(), h + hw * th0.sin());
        let p1 = at(hw * th1.cos(), h + hw * th1.sin());
        // inset each wedge toward its centroid so leading shows between facets
        let g = mul(addv(addv(cen, p0), p1), 1.0 / 3.0);
        let ins = |p: Vec3| addv(mul(p, 0.90), mul(g, 0.10));
        let mut t = tri(ins(cen), ins(p0), ins(p1), pick(20 + k as i32, 5));
        t.mat = 3.0; t.emissive = 0.35 + 0.5 * hsh(30 + k as i32, 2);
        out.push(t);
    }
    out
}

pub fn wall_ring(center: Vec3, radius: f32, height: f32, segs: usize, color: Color32) -> Mesh3 {
    let segs = tess(segs, 8);
    let (mut verts, mut norms, mut tris) = (Vec::new(), Vec::new(), Vec::new());
    for j in 0..=segs {
        let th = std::f32::consts::TAU * j as f32 / segs as f32;
        let (cs, sn) = (th.cos(), th.sin());
        let nrm = v(-cs, 0.0, -sn); // face inward
        verts.push(addv(center, v(radius * cs, 0.0, radius * sn)));    norms.push(nrm);
        verts.push(addv(center, v(radius * cs, height, radius * sn))); norms.push(nrm);
    }
    for j in 0..segs as u32 {
        let a = j * 2;
        tris.push([a, a + 2, a + 1]);
        tris.push([a + 1, a + 2, a + 3]);
    }
    Mesh3 { emissive: 0.0, verts, norms, tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }
}

/// Non-uniformly scale a mesh in the XZ plane about the origin (Y untouched). Lets a circular builder
/// produce an ELLIPTICAL shell — build at one radius, then `scale_xz(m, 1.0, rz/rx)` to hug a long axis.
/// Normals are left as-is (a baked, procedurally-shaded scene tolerates the small lighting error).
pub fn scale_xz(m: &mut Mesh3, sx: f32, sz: f32) {
    for p in &mut m.verts { p.x *= sx; p.z *= sz; }
}

/// Non-uniformly scale a mesh on all three axes about the origin, fixing normals (inverse-scaled +
/// renormalised) so lighting stays correct. Used to shape a unit sphere into a laterally-compressed,
/// tapered fish-body segment, etc.
pub fn scale3(m: &mut Mesh3, sx: f32, sy: f32, sz: f32) {
    for p in &mut m.verts { p.x *= sx; p.y *= sy; p.z *= sz; }
    let (ix, iy, iz) = (1.0 / sx.max(1e-4), 1.0 / sy.max(1e-4), 1.0 / sz.max(1e-4));
    for n in &mut m.norms {
        let mut q = v(n.x * ix, n.y * iy, n.z * iz);
        let l = (q.x * q.x + q.y * q.y + q.z * q.z).sqrt();
        if l > 1e-6 { q.x /= l; q.y /= l; q.z /= l; }
        *n = q;
    }
}

/// A `wall_ring` with angular OPENINGS — each `gaps` entry is `(centre_angle, half_width)` in
/// radians; any wall quad whose midpoint bearing falls within `half_width` of a gap centre is
/// skipped, leaving a doorway you can pass through. Verts are still emitted for all segments
/// (cheap, keeps indexing simple); only the gapped quads' triangles are omitted.
pub fn wall_ring_gapped(center: Vec3, radius: f32, height: f32, segs: usize, color: Color32, gaps: &[(f32, f32)]) -> Mesh3 {
    let segs = tess(segs, 8);
    let (mut verts, mut norms, mut tris) = (Vec::new(), Vec::new(), Vec::new());
    for j in 0..=segs {
        let th = std::f32::consts::TAU * j as f32 / segs as f32;
        let (cs, sn) = (th.cos(), th.sin());
        let nrm = v(-cs, 0.0, -sn); // face inward
        verts.push(addv(center, v(radius * cs, 0.0, radius * sn)));    norms.push(nrm);
        verts.push(addv(center, v(radius * cs, height, radius * sn))); norms.push(nrm);
    }
    let in_gap = |ang: f32| gaps.iter().any(|&(gc, hw)| {
        let mut d = (ang - gc).rem_euclid(std::f32::consts::TAU);
        if d > std::f32::consts::PI { d -= std::f32::consts::TAU; }
        d.abs() <= hw
    });
    for j in 0..segs as u32 {
        let mid = std::f32::consts::TAU * (j as f32 + 0.5) / segs as f32;
        if in_gap(mid) { continue; }
        let a = j * 2;
        tris.push([a, a + 2, a + 1]);
        tris.push([a + 1, a + 2, a + 3]);
    }
    Mesh3 { emissive: 0.0, verts, norms, tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }
}

/// A flat ring (annulus) lying in the XZ plane, facing up — one concentric terrace step. `r_inner`..`r_outer`.
pub fn annulus(center: Vec3, r_inner: f32, r_outer: f32, segs: usize, color: Color32) -> Mesh3 {
    let segs = tess(segs, 8);
    let (mut verts, mut norms, mut tris) = (Vec::new(), Vec::new(), Vec::new());
    for j in 0..=segs {
        let th = std::f32::consts::TAU * j as f32 / segs as f32;
        let (cs, sn) = (th.cos(), th.sin());
        verts.push(addv(center, v(r_inner * cs, 0.0, r_inner * sn))); norms.push(v(0.0, 1.0, 0.0));
        verts.push(addv(center, v(r_outer * cs, 0.0, r_outer * sn))); norms.push(v(0.0, 1.0, 0.0));
    }
    for j in 0..segs as u32 {
        let a = j * 2;
        tris.push([a, a + 2, a + 1]);
        tris.push([a + 1, a + 2, a + 3]);
    }
    Mesh3 { emissive: 0.0, verts, norms, tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }
}

/// An inward-facing dome cap closing the top of an enclosure: rings sweep up from `base_r` (at `center.y`)
/// over `rise`, narrowing to an `oculus_r` opening — the skylight that lets light in from beyond the room.
/// Normals face inward and down so the underside is lit. `rings` vertical bands, `segs` around.
pub fn dome_cap(center: Vec3, base_r: f32, rise: f32, oculus_r: f32, segs: usize, rings: usize, color: Color32) -> Mesh3 {
    let (segs, rings) = (tess(segs, 8), tess(rings, 2));
    let (mut verts, mut norms, mut tris) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..=rings {
        let t = i as f32 / rings as f32;
        let r = base_r + (oculus_r - base_r) * t;
        let y = rise * (t * std::f32::consts::FRAC_PI_2).sin(); // a domed rise (quarter-sine)
        for j in 0..=segs {
            let th = std::f32::consts::TAU * j as f32 / segs as f32;
            let (cs, sn) = (th.cos(), th.sin());
            verts.push(addv(center, v(r * cs, y, r * sn)));
            norms.push(norm(v(-cs, -0.4, -sn))); // inward + slightly down
        }
    }
    let row = segs + 1;
    for i in 0..rings {
        for j in 0..segs {
            let a = (i * row + j) as u32;
            let (b, cc, d) = (a + 1, a + row as u32, a + row as u32 + 1);
            tris.push([a, cc, b]);
            tris.push([b, cc, d]);
        }
    }
    Mesh3 { emissive: 0.0, verts, norms, tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }
}
/// reads as a real folded sheet rather than a flat triangle. `size` is the half-width, `rise` the fold
/// height. Smooth-averaged normals (the geometry still carries the creases).
pub fn paper_fold(center: Vec3, size: f32, rise: f32, color: Color32) -> Mesh3 {
    let n = tess(7, 4); // higher res → the curl stays smooth (scales with detail)
    let (mut verts, mut tris) = (Vec::new(), Vec::new());
    for iy in 0..n {
        for ix in 0..n {
            let u = ix as f32 / (n as f32 - 1.0) - 0.5; // -0.5..0.5
            let w = iy as f32 / (n as f32 - 1.0) - 0.5;
            // a gently curling vellum page — soft cosine bow + edges curling up, NO sharp crease
            let bow = (std::f32::consts::PI * u).cos() * 0.5 + 0.5; // smooth arch across u
            let curl = (w * 2.0).powi(2) * 0.6;                     // far edges (|w|→0.5) lift
            let wave = 0.10 * (u * 6.0).sin();                      // faint paper undulation
            let h = rise * (0.35 * bow + curl + wave - 0.28);
            verts.push(addv(center, v(u * 2.0 * size, h, w * 2.0 * size)));
        }
    }
    let idx = |ix: usize, iy: usize| (iy * n + ix) as u32;
    for iy in 0..n - 1 {
        for ix in 0..n - 1 {
            let (a, b, c2, d) = (idx(ix, iy), idx(ix + 1, iy), idx(ix + 1, iy + 1), idx(ix, iy + 1));
            tris.push([a, b, c2]);
            tris.push([a, c2, d]);
        }
    }
    let mut norms = vec![v(0.0, 0.0, 0.0); verts.len()];
    for t in &tris {
        let nf = cross(sub(verts[t[1] as usize], verts[t[0] as usize]), sub(verts[t[2] as usize], verts[t[0] as usize]));
        for &vi in t { norms[vi as usize] = addv(norms[vi as usize], nf); }
    }
    for nn in &mut norms { *nn = norm(*nn); }
    Mesh3 { emissive: 0.0, verts, norms, tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }
}

pub fn leaf_cluster(center: Vec3, r: f32, seed: u32, color: Color32) -> Mesh3 {
    leaf_cluster_lod(center, r, seed, color, 1.0)
}
/// LOD variant of `leaf_cluster`: `detail` (≈0.15..1) scales the leaf COUNT so distant clusters cull down to a
/// few blades while near ones stay dense. Blades are kept SMALL (subdivided foliage) rather than big flat cards.
pub fn leaf_cluster_lod(center: Vec3, r: f32, seed: u32, color: Color32, detail: f32) -> Mesh3 {
    let (mut verts, mut norms, mut tris) = (Vec::new(), Vec::new(), Vec::new());
    // leaf count: a modest cut everywhere, biggest on small decorations (a 0.3 flower had ~44 blades — absurd)
    // and a lower ceiling on huge crowns, since the tree-heavy scenes are dominated by big-r canopies. Curve:
    // r 0.3→~19, 0.55→~27, 1.0→~41, 1.4→~52, 2.5→~86(cap 88). Net FPS win at high graphics; the dense-canopy
    // look is preserved (big crowns stay near their old density). `detail` still lets callers LOD further.
    let leaves = tess((((10.0 + r * 24.0).min(84.0)) * detail.clamp(0.15, 1.0)) as usize, 5);
    // a flat leaf-blade silhouette in (along, across) local coords: pointed tip → rounded body → base
    const BLADE: [(f32, f32); 8] = [(1.0, 0.0), (0.6, 0.22), (0.2, 0.32), (-0.35, 0.30), (-0.95, 0.0), (-0.35, -0.30), (0.2, -0.32), (0.6, -0.22)];
    for k in 0..leaves {
        let k = k as i32;
        let a = hash3(k, seed as i32, 1) * std::f32::consts::TAU;
        let b = (hash3(k, seed as i32, 2) * 2.0 - 1.0).acos(); // even spherical scatter
        let dir = v(b.sin() * a.cos(), b.cos(), b.sin() * a.sin());
        let rad = r * (0.70 + 0.28 * hash3(k, seed as i32, 3));
        let cpt = addv(center, mul(dir, rad));
        let up = if dir.y.abs() < 0.9 { v(0.0, 1.0, 0.0) } else { v(1.0, 0.0, 0.0) };
        let along = norm(cross(dir, up));   // long axis (tip→base)
        let across = cross(dir, along);     // short axis
        let len = (r * 0.10 * (0.8 + 0.4 * hash3(k, seed as i32, 4))).clamp(0.05, 0.30); // ABSOLUTE-capped small blades — big crowns get MANY small leaves, not few big cards
        let curl = len * (0.30 + 0.22 * hash3(k, seed as i32, 5)); // per-leaf cup depth
        let i = verts.len() as u32;
        // raised midrib centre (cup high point) — gives the blade a fold + a non-flat normal field
        let cv = addv(cpt, mul(dir, curl * 0.45));
        verts.push(cv); norms.push(dir);
        for &(lx, ly) in &BLADE {
            let edge = addv(cpt, addv(mul(along, lx * len), mul(across, ly * len)));
            let dip = curl * (0.35 * lx * lx + 1.1 * ly * ly); // edges + base curl back → cupped blade
            let p = addv(edge, mul(dir, -dip));
            // normal follows the cup: tilt from the surface point back toward the raised midrib
            let nrm = norm(addv(dir, mul(norm(sub(cv, p)), 0.55)));
            verts.push(p); norms.push(nrm);
        }
        for j in 0..BLADE.len() as u32 {
            tris.push([i, i + 1 + j, i + 1 + ((j + 1) % BLADE.len() as u32)]);
        }
        // PETIOLE — a thin tapered stem from the blade base back toward the cluster centre (the twig), so every
        // leaf visibly ATTACHES to the stem instead of floating free. Part of the LOD loop ⇒ culls with distance.
        let bb = addv(cpt, mul(along, -0.95 * len)); // pointed base of the blade silhouette
        let tow = { let d = sub(center, cpt); let l = dot(d, d).sqrt(); if l > 1e-4 { mul(d, 1.0 / l) } else { mul(dir, -1.0) } };
        let slen = (len * 1.8).min(rad * 0.6);
        let stip = addv(bb, mul(tow, slen));
        let sw = (len * 0.055).max(0.010);
        let si = verts.len() as u32;
        verts.push(addv(bb, mul(across, sw)));          norms.push(dir);
        verts.push(addv(bb, mul(across, -sw)));         norms.push(dir);
        verts.push(addv(stip, mul(across, sw * 0.55))); norms.push(dir);
        verts.push(addv(stip, mul(across, -sw * 0.55)));norms.push(dir);
        tris.push([si, si + 2, si + 1]);
        tris.push([si + 1, si + 2, si + 3]);
    }
    Mesh3 { emissive: 0.0, verts, norms, tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }
}

/// A SINGLE leaf blade (one cupped blade with a raised midrib), tilted up-and-out by `seed`. Used for leaf-style
/// nodes so a turn reads as one clear leaf rather than a dense orb of foliage. Uses `color` directly (no vcol).
pub fn leaf_single(center: Vec3, r: f32, seed: u32, color: Color32) -> Mesh3 {
    let (mut verts, mut norms, mut tris) = (Vec::new(), Vec::new(), Vec::new());
    const BLADE: [(f32, f32); 8] = [(1.0, 0.0), (0.6, 0.22), (0.2, 0.32), (-0.35, 0.30), (-0.95, 0.0), (-0.35, -0.30), (0.2, -0.32), (0.6, -0.22)];
    let yaw = hash3(seed as i32, 0, 1) * std::f32::consts::TAU;
    let tilt = 0.55 + hash3(seed as i32, 0, 2) * 0.4;          // mostly up-facing → readable from above/oblique
    let dir = norm(v(yaw.cos() * 0.5, tilt, yaw.sin() * 0.5)); // blade normal
    let up = if dir.y.abs() < 0.9 { v(0.0, 1.0, 0.0) } else { v(1.0, 0.0, 0.0) };
    let along = norm(cross(dir, up));
    let across = cross(dir, along);
    let len = r * 1.15;
    let curl = len * 0.34;
    let cv = addv(center, mul(dir, curl * 0.45)); // raised midrib centre
    verts.push(cv); norms.push(dir);
    for &(lx, ly) in &BLADE {
        let edge = addv(center, addv(mul(along, lx * len), mul(across, ly * len)));
        let dip = curl * (0.35 * lx * lx + 1.1 * ly * ly);
        let p = addv(edge, mul(dir, -dip));
        let nrm = norm(addv(dir, mul(norm(sub(cv, p)), 0.55)));
        verts.push(p); norms.push(nrm);
    }
    for j in 0..BLADE.len() as u32 { tris.push([0, 1 + j, 1 + ((j + 1) % BLADE.len() as u32)]); }
    Mesh3 { emissive: 0.0, verts, norms, tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }
}

/// A rippling strand between `a` and `b`: a tube around a wavy centreline that bows out in the middle
/// (anchored at both nodes) and undulates with `t` — a vine / water trickle / gravity-air perturbation
/// connecting two stones. `amp` is the ripple size in world units.
pub fn strand(a: Vec3, b: Vec3, r: f32, around: usize, segments: usize, t: f32, amp: f32, color: Color32) -> Mesh3 {
    let (around, segments) = (around.max(3), segments.max(2));
    let axis = norm(sub(b, a));
    let up0 = if axis.y.abs() < 0.9 { v(0.0, 1.0, 0.0) } else { v(1.0, 0.0, 0.0) };
    let u = norm(cross(axis, up0));
    let w = cross(axis, u);
    let mut center = Vec::with_capacity(segments + 1);
    for s in 0..=segments {
        let f = s as f32 / segments as f32;
        let env = (f * std::f32::consts::PI).sin(); // 0 at ends → ripple anchored at the nodes
        let ph = f * 6.0 + t * 1.6;
        let off = addv(mul(u, ph.sin() * amp * env), mul(w, (ph * 0.7 + 1.3).cos() * amp * 0.6 * env));
        center.push(addv(addv(a, mul(sub(b, a), f)), off));
    }
    let (mut verts, mut norms, mut tris) = (Vec::new(), Vec::new(), Vec::new());
    for s in 0..=segments {
        let tan = if s == 0 { norm(sub(center[1], center[0])) }
            else if s == segments { norm(sub(center[segments], center[segments - 1])) }
            else { norm(sub(center[s + 1], center[s - 1])) };
        let lu = norm(cross(tan, up0));
        let lw = cross(tan, lu);
        for j in 0..=around {
            let th = std::f32::consts::TAU * j as f32 / around as f32;
            let n = addv(mul(lu, th.cos()), mul(lw, th.sin()));
            verts.push(addv(center[s], mul(n, r)));
            norms.push(n);
        }
    }
    let row = around + 1;
    for s in 0..segments {
        for j in 0..around {
            let a0 = (s * row + j) as u32;
            let a1 = ((s + 1) * row + j) as u32;
            tris.push([a0, a1, a0 + 1]);
            tris.push([a0 + 1, a1, a1 + 1]);
        }
    }
    Mesh3 { emissive: 0.0, verts, norms, tris, color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0 }
}

/// What a graph node's body looks like — chosen per scene/palette.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum NodeStyle { Orb, Stone, Leaf, Pearl }

/// The mesh for one node in the chosen style.
pub fn node_mesh(style: NodeStyle, center: Vec3, r: f32, seed: u32, color: Color32) -> Mesh3 {
    match style {
        NodeStyle::Orb => uv_sphere(center, r, 22, 14, color),
        NodeStyle::Stone => rock(center, r, seed, color),
        NodeStyle::Leaf => leaf_single(center, r * 1.7, seed, color), // one clear leaf per turn (not a foliage orb)
        NodeStyle::Pearl => cracked_pearl(center, r, seed, color), // faceted/cracked → nacre catches each facet
    }
}

/// Append `add`'s geometry into `into` (offsetting triangle indices) — lets a node be assembled from several
/// primitives yet stay ONE mesh (one draw, one material/colour), the contract `node_mesh` returns.
fn mesh_merge(into: &mut Mesh3, add: &Mesh3) {
    let base = into.verts.len() as u32;
    into.verts.extend_from_slice(&add.verts);
    into.norms.extend_from_slice(&add.norms);
    for t in &add.tris { into.tris.push([t[0] + base, t[1] + base, t[2] + base]); }
}

/// Old-Library node bodies — a turn rendered as PAPER. `kind`: 0 page (a quick snippet) · 1 book (a closed
/// tome, for a normal prose turn) · 2 scroll (code/file/long turns; `unroll` 0..1 = how far the sheet spills
/// open, so longer content unrolls further). Returns one merged mesh; pair with the PAPER material.
pub fn library_node(kind: u32, center: Vec3, r: f32, unroll: f32, seed: u32, color: Color32) -> Mesh3 {
    let mut m = Mesh3 { verts: Vec::new(), norms: Vec::new(), tris: Vec::new(), color, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0, emissive: 0.0 };
    let yaw = ((seed % 17) as f32 / 17.0 - 0.5) * 0.9;       // a little per-node turn so they don't line up
    let right = v(yaw.cos(), 0.0, yaw.sin());
    let fwd = v(-yaw.sin(), 0.0, yaw.cos());
    let up = v(0.0, 1.0, 0.0);
    match kind {
        0 => { // PAGE — a single thin sheet (quick snippet): a flattened card with a hint of curl
            let (w, h, th) = (r * 1.45, r * 1.85, r * 0.1);
            let c = center;
            mesh_merge(&mut m, &panel(addv(c, mul(fwd, th * 0.5)), right, up, w, h, color));   // front
            mesh_merge(&mut m, &panel(addv(c, mul(fwd, -th * 0.5)), right, up, w, h, color));  // back
            mesh_merge(&mut m, &panel(addv(c, mul(up, h * 0.5)), right, fwd, w, th, color));    // top edge
            mesh_merge(&mut m, &panel(addv(c, mul(up, -h * 0.5)), right, fwd, w, th, color));   // bottom edge
            mesh_merge(&mut m, &panel(addv(c, mul(right, w * 0.5)), fwd, up, th, h, color));    // right edge
            mesh_merge(&mut m, &panel(addv(c, mul(right, -w * 0.5)), fwd, up, th, h, color));   // left edge
        }
        1 => { // BOOK — a closed flattened tome (cover box + page block)
            let (w, h, th) = (r * 1.5, r * 1.95, r * 0.62);
            let c = center;
            mesh_merge(&mut m, &panel(addv(c, mul(fwd, th * 0.5)), right, up, w, h, color));   // front cover
            mesh_merge(&mut m, &panel(addv(c, mul(fwd, -th * 0.5)), right, up, w, h, color));  // back cover
            mesh_merge(&mut m, &panel(addv(c, mul(right, -w * 0.5)), fwd, up, th, h, color));  // spine
            mesh_merge(&mut m, &panel(addv(c, mul(right, w * 0.5)), fwd, up, th, h * 0.96, color)); // fore-edge (pages)
            mesh_merge(&mut m, &panel(addv(c, mul(up, h * 0.5)), right, fwd, w, th, color));   // top
            mesh_merge(&mut m, &panel(addv(c, mul(up, -h * 0.5)), right, fwd, w, th, color));  // bottom
        }
        _ => { // SCROLL — a rolled rod with a sheet spilling down; longer content → longer unroll
            let halfw = r * 0.92;
            let a = addv(center, mul(right, -halfw));
            let b = addv(center, mul(right, halfw));
            let rod = r * 0.5;
            mesh_merge(&mut m, &tube(a, b, rod, 12, color));                 // the roll
            mesh_merge(&mut m, &uv_sphere(a, rod * 1.06, 8, 6, color));      // end knobs (the rod caps)
            mesh_merge(&mut m, &uv_sphere(b, rod * 1.06, 8, 6, color));
            let sheet_h = r * (0.5 + 2.7 * unroll.clamp(0.0, 1.0));
            let down = v(0.0, -1.0, 0.0);
            let sc = addv(addv(center, mul(fwd, rod * 0.85)), mul(down, sheet_h * 0.5));
            mesh_merge(&mut m, &panel(sc, right, down, halfw * 1.9, sheet_h, color)); // the unrolled sheet
            let bot = addv(center, mul(down, sheet_h));                       // a little curl at the spilling end
            mesh_merge(&mut m, &tube(addv(bot, mul(right, -halfw)), addv(bot, mul(right, halfw)), rod * 0.55, 8, color));
        }
    }
    m
}

/// A CRACKED PEARL: a faceted, slightly irregular sphere — FLAT-shaded so each facet reads as its own plane
/// (the cracked-gem look), with a seeded per-vertex radial jitter for the cracks. Paired with the nacre/water
/// shader, every facet catches the iridescence at a different angle — the 3D echo of the composer's cracked
/// pearl. Per-facet vertices are duplicated (flat normals); positions come from the shared jittered sphere so
/// adjacent facets still meet (no gaps).
pub fn cracked_pearl(center: Vec3, r: f32, seed: u32, color: Color32) -> Mesh3 {
    let base = uv_sphere(v(0.0, 0.0, 0.0), r, 16, 10, color); // local sphere at origin
    let jv: Vec<Vec3> = base.verts.iter().enumerate().map(|(i, p)| {
        let mut h = seed.wrapping_mul(0x9E37_79B1).wrapping_add((i as u32).wrapping_mul(0x85EB_CA77));
        h ^= h >> 15; h = h.wrapping_mul(0x2C1B_3C6D); h ^= h >> 13;
        let jit = 1.0 + ((h & 0xFFFF) as f32 / 65535.0 - 0.5) * 0.20; // ±10% radius → crack relief
        mul(*p, jit)
    }).collect();
    let mut verts = Vec::with_capacity(base.tris.len() * 3);
    let mut norms = Vec::with_capacity(base.tris.len() * 3);
    let mut vcol = Vec::with_capacity(base.tris.len() * 3);
    let mut tris = Vec::with_capacity(base.tris.len());
    let (br, bg, bb) = (color.r() as f32, color.g() as f32, color.b() as f32);
    for tri in &base.tris {
        let (a, b, cc) = (jv[tri[0] as usize], jv[tri[1] as usize], jv[tri[2] as usize]);
        let n = norm(cross(sub(b, a), sub(cc, a)));
        // per-facet NACRE tint (spectral by facet orientation) blended with the pearl's base colour, so the
        // cracked surface reads as iridescent facets even under bloom
        let ph = n.y * 3.0 + n.z.atan2(n.x);
        let sr = 0.5 + 0.5 * ph.cos();
        let sg = 0.5 + 0.5 * (ph + 2.094).cos();
        let sb = 0.5 + 0.5 * (ph + 4.188).cos();
        let fc = Color32::from_rgb(
            (br * 0.5 + sr * 255.0 * 0.5) as u8,
            (bg * 0.5 + sg * 255.0 * 0.5) as u8,
            (bb * 0.5 + sb * 255.0 * 0.5) as u8);
        let bi = verts.len() as u32;
        for p in [a, b, cc] { verts.push(addv(p, center)); norms.push(n); vcol.push(fc); }
        tris.push([bi, bi + 1, bi + 2]);
    }
    Mesh3 { verts, norms, tris, color, vcol: Some(vcol), offset: v(0.0, 0.0, 0.0), mat: 0.0, emissive: 0.0 }
}

/// A symmetric identicon for `seed`: the filled cells of a `grid`×`grid` pattern, mirrored left↔right (so
/// every node's short-id renders as a distinct, stable rune). The caller draws these as glowing squares on
/// the stone, scaled to its projected size.
pub fn rune_cells(seed: u64, grid: usize) -> Vec<(usize, usize)> {
    let grid = grid.max(3);
    let half = (grid + 1) / 2;
    let mut s = seed | 1;
    let mut cells = Vec::new();
    for y in 0..grid {
        for x in 0..half {
            s ^= s << 13; s ^= s >> 7; s ^= s << 17;
            if s & 1 == 1 {
                cells.push((x, y));
                let mx = grid - 1 - x;
                if mx != x { cells.push((mx, y)); }
            }
        }
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_dots_are_rounder_than_the_old_low_res_blob() {
        // the literal "dots too low-res" complaint: each lit pixel used to be a 4×3 sphere (~12 tris,
        // reads as a faceted diamond up close). Confirms the fix actually raised the segment counts,
        // and that label_cells uses the shared constants rather than a hand-rolled number that could
        // silently drift back down.
        assert!(DOT_SEGMENTS > 4 && DOT_RINGS > 3, "must be a real improvement over the old 4×3");
        let dots = label_cells(Vec3 { x: 0.0, y: 0.0, z: 0.0 }, Vec3 { x: 1.0, y: 0.0, z: 0.0 }, Vec3 { x: 0.0, y: 1.0, z: 0.0 },
            Vec3 { x: 0.0, y: 0.0, z: 1.0 }, "Hi", 0.2, 0.0, Color32::WHITE, 1.0, 0.0, TextAnim::Plain);
        assert!(!dots.is_empty());
        // a uv_sphere(segs, rings) mesh has 2 + segs*(rings-1) vertices and 2*segs + segs*(rings-2)*2
        // triangles (poles + rings of quads) — just check it's meaningfully more detailed than the old
        // 4×3 sphere would have produced, without hard-coding uv_sphere's internal formula here.
        let old_tri_count = uv_sphere(Vec3 { x: 0.0, y: 0.0, z: 0.0 }, 0.1, 4, 3, Color32::WHITE).tris.len();
        let new_tri_count = uv_sphere(Vec3 { x: 0.0, y: 0.0, z: 0.0 }, 0.1, DOT_SEGMENTS, DOT_RINGS, Color32::WHITE).tris.len();
        assert!(new_tri_count > old_tri_count, "new dot ({new_tri_count} tris) should be more detailed than the old one ({old_tri_count} tris)");
    }

    #[test]
    fn text_anim_idioms_are_distinct_and_bounded() {
        use TextAnim::*;
        // Plain never displaces the cell (back-compat with the old twinkle)
        for &tt in &[0.0_f32, 0.5, 1.7, 3.3] {
            let (du, dr, _) = anim_cell(Plain, tt, 0.5, 3, 0.03);
            assert!(du == 0.0 && dr == 0.0, "Plain has no displacement");
        }
        // Ripple bobs vertically (non-zero d_up somewhere), scaled by `scale`, and stays small
        let mut max_up = 0.0_f32;
        for i in 0..40 { let (du, _, e) = anim_cell(Ripple, i as f32 * 0.1, 0.3, 3, 0.03); max_up = max_up.max(du.abs()); assert!(e > 0.0 && e < 4.0); }
        assert!(max_up > 0.0 && max_up < 0.03, "ripple bob is present but subtle: {max_up}");
        // Sway pushes sideways (non-zero d_right somewhere)
        let mut max_r = 0.0_f32;
        for i in 0..40 { let (_, dr, _) = anim_cell(Sway, i as f32 * 0.1, 0.3, 3, 0.03); max_r = max_r.max(dr.abs()); }
        assert!(max_r > 0.0, "sway moves sideways");
        // every idiom keeps emissive positive + bounded (text stays lit + readable)
        for &m in &[Plain, Ripple, Sway, Shimmer, Drift, Fade, Pulse] {
            for i in 0..30 { let (_, _, e) = anim_cell(m, i as f32 * 0.13, (i as f32 * 0.03) % 1.0, i, 0.03); assert!(e > 0.0 && e < 5.0, "{m:?} emissive {e} in range"); }
        }
    }

    #[test]
    fn library_node_forms_are_distinct_and_unroll_extends_scroll() {
        let bbox = |m: &Mesh3| {
            let (mut lo, mut hi) = (v(1e9, 1e9, 1e9), v(-1e9, -1e9, -1e9));
            for p in &m.verts { lo = v(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z)); hi = v(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z)); }
            (hi.x - lo.x, hi.y - lo.y, hi.z - lo.z, lo.y) // (dx,dy,dz, min_y)
        };
        let c = v(0.0, 0.0, 0.0); let r = 1.0; let col = Color32::WHITE;
        let page = library_node(0, c, r, 0.0, 1, col);
        let book = library_node(1, c, r, 0.0, 2, col);
        let scroll0 = library_node(2, c, r, 0.0, 3, col);
        let scroll1 = library_node(2, c, r, 0.9, 3, col);
        // all well-formed (non-empty, finite)
        for m in [&page, &book, &scroll0, &scroll1] {
            assert!(!m.verts.is_empty() && !m.tris.is_empty());
            assert!(m.verts.iter().all(|p| p.x.is_finite() && p.y.is_finite() && p.z.is_finite()));
        }
        // a book is a closed box: ≥ 6 quads (12 tris), 3D
        let (bx, by, bz, _) = bbox(&book);
        assert!(book.tris.len() >= 12 && bx > 0.0 && by > 0.0 && bz > 0.0, "book is a 3D box");
        // a page (thin card) occupies far less bounding volume than a closed book — robust under the
        // per-seed yaw (volume doesn't inflate the way a single rotated axis-extent does).
        let vol = |m: &Mesh3| { let (dx, dy, dz, _) = bbox(m); dx * dy * dz };
        assert!(vol(&page) < vol(&book) * 0.6, "page slimmer than book: {} vs {}", vol(&page), vol(&book));
        // unrolling a scroll spills the sheet DOWNWARD (world -Y) → greater vertical extent and a lower min-y
        let (_, dy0, _, miny0) = bbox(&scroll0);
        let (_, dy1, _, miny1) = bbox(&scroll1);
        assert!(dy1 > dy0 + 0.5 && miny1 < miny0 - 0.5, "unroll extends the scroll: dy {dy0}->{dy1}, miny {miny0}->{miny1}");
    }

    #[test]
    fn obj_loader_parses_faces_and_scale3_compresses() {
        // a unit quad (two tris) with explicit normals + a triangle using v//vn form
        let obj = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvn 0 0 1\nf 1//1 2//1 3//1\nf 1//1 3//1 4//1\n";
        let mut m = load_obj_str(obj, Color32::WHITE, 1.0);
        assert_eq!(m.tris.len(), 2, "two triangles");
        assert_eq!(m.verts.len(), 6, "3 verts per tri, unindexed");
        assert!(m.norms.iter().all(|n| n.z > 0.9), "explicit +z normals survive");
        // polygon fan-triangulation: a quad on one `f` line → 2 tris
        let quad = load_obj_str("v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n", Color32::WHITE, 0.0);
        assert_eq!(quad.tris.len(), 2, "quad fans into 2 tris");
        assert!(quad.norms.iter().all(|n| n.z.abs() > 0.9), "missing vn → computed face normal");
        // scale3 compresses geometry and keeps normals unit-length
        m.norms.clear(); m.norms.push(v(1.0, 0.0, 0.0));
        m.verts = vec![v(2.0, 4.0, 6.0)];
        scale3(&mut m, 0.5, 0.25, 2.0);
        assert!((m.verts[0].x - 1.0).abs() < 1e-4 && (m.verts[0].y - 1.0).abs() < 1e-4 && (m.verts[0].z - 12.0).abs() < 1e-4);
        let n = m.norms[0]; assert!((n.x * n.x + n.y * n.y + n.z * n.z - 1.0).abs() < 1e-3, "normal renormalised");
    }

    #[test]
    fn annulus_and_dome_are_well_formed() {
        let a = annulus(v(0.0, 1.0, 0.0), 4.0, 6.0, 24, Color32::WHITE);
        assert_eq!(a.verts.len(), (24 + 1) * 2);
        for nrm in &a.norms { assert!(nrm.y > 0.9, "terrace faces up"); }
        // inner/outer radii present
        let radii: Vec<f32> = a.verts.iter().map(|p| (p.x * p.x + p.z * p.z).sqrt()).collect();
        assert!(radii.iter().any(|&r| (r - 4.0).abs() < 1e-3) && radii.iter().any(|&r| (r - 6.0).abs() < 1e-3));
        let d = dome_cap(v(0.0, 0.0, 0.0), 10.0, 6.0, 3.0, 16, 4, Color32::WHITE);
        // base ring sits at the base radius/height; top ring has narrowed to the oculus and risen
        assert!((d.verts[0].x.hypot(d.verts[0].z) - 10.0).abs() < 1e-3, "dome base at base_r");
        let top = d.verts.last().unwrap();
        assert!(top.x.hypot(top.z) < 10.0 && top.y > 0.0, "dome narrows + rises toward the oculus");
    }

    #[test]
    fn wall_ring_faces_inward() {
        let w = wall_ring(v(0.0, 0.0, 0.0), 10.0, 5.0, 16, Color32::WHITE);
        assert_eq!(w.verts.len(), (16 + 1) * 2); // two rings of segs+1
        assert_eq!(w.tris.len(), 16 * 2);
        // every wall vertex sits on the radius, and its normal points back toward the axis (inward)
        for (p, nrm) in w.verts.iter().zip(w.norms.iter()) {
            let radial = ((p.x * p.x + p.z * p.z) as f32).sqrt();
            assert!((radial - 10.0).abs() < 1e-3, "vertex on the wall radius");
            assert!(nrm.x * p.x + nrm.z * p.z < 0.0, "normal faces inward (opposes the radial)");
        }
    }

    #[test]
    fn sphere_has_expected_counts() {
        let s = uv_sphere(v(0.0, 0.0, 0.0), 1.0, 8, 6, Color32::WHITE);
        assert_eq!(s.verts.len(), 9 * 7);          // (segs+1)·(rings+1)
        assert_eq!(s.tris.len(), 8 * 6 * 2);        // 2 tris per quad
        assert_eq!(s.verts.len(), s.norms.len());
        // vertex normals are unit
        assert!((dot(s.norms[20], s.norms[20]) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn shade_floor_and_ceiling() {
        let light = Light::sun(v(0.0, 1.0, 0.0), Color32::WHITE);
        let a = shade(Color32::from_rgb(200, 200, 200), &light, 1.0, 0.0); // up-facing, no sun → sky ambient only
        let b = shade(Color32::from_rgb(200, 200, 200), &light, 1.0, 1.0); // up-facing, full sun → brighter
        assert!(b.r() > a.r(), "facing the sun is brighter than ambient alone");
        assert!(a.r() > 0, "hemispherical ambient keeps unlit faces off pure black");
    }

    #[test]
    fn render_projects_and_sorts() {
        let cam = OrbitCam::default();
        let scene = vec![uv_sphere(cam.target, 1.0, 10, 7, Color32::from_rgb(120, 180, 220))];
        let light = Light::sun(v(0.3, 1.0, 0.4), Color32::WHITE);
        let mesh = render(&scene, &cam, 300.0, 200.0, 400.0, &light);
        assert!(!mesh.vertices.is_empty() && !mesh.indices.is_empty(), "a sphere in front of the camera renders triangles");
        assert_eq!(mesh.indices.len() % 3, 0, "indices come in whole triangles");
    }

    #[test]
    fn sun_dir_is_unit() {
        let s = sun_dir_3d();
        assert!((dot(s, s) - 1.0).abs() < 1e-3, "sun direction is normalized");
    }

    #[test]
    fn graph_scene_has_orbs_and_edges() {
        let centers = [v(0.0, 0.0, 0.0), v(1.0, -1.0, 0.0), v(-1.0, -1.0, 0.5)];
        let cols = [Color32::WHITE; 3];
        let edges = [(1usize, 0usize), (2, 0)];
        let scene = graph_scene(&centers, &cols, &edges, Some(2), 0.25, 0.05, Color32::from_rgb(0, 255, 255), Color32::GRAY);
        assert_eq!(scene.len(), 5, "3 orbs + 2 edge tubes");
    }

    #[test]
    fn node_styles_and_strand_build() {
        for st in [NodeStyle::Orb, NodeStyle::Stone, NodeStyle::Leaf, NodeStyle::Pearl] {
            let m = node_mesh(st, v(0.0, 0.0, 0.0), 1.0, 7, Color32::WHITE);
            assert!(!m.verts.is_empty() && !m.tris.is_empty(), "{st:?} builds geometry");
            assert_eq!(m.verts.len(), m.norms.len());
        }
        let s = strand(v(0.0, 0.0, 0.0), v(3.0, 0.0, 0.0), 0.1, 6, 8, 0.5, 0.4, Color32::GRAY);
        assert_eq!(s.verts.len(), 7 * 9, "(around+1)·(segments+1) ring verts");
    }

    #[test]
    fn runes_are_symmetric_and_stable() {
        let a = rune_cells(0xABCD_1234, 5);
        let b = rune_cells(0xABCD_1234, 5);
        assert_eq!(a, b, "deterministic for a given seed");
        // every off-centre filled cell has its left↔right mirror present
        for &(x, y) in &a {
            let mx = 4 - x;
            if mx != x { assert!(a.contains(&(mx, y)), "cell ({x},{y}) is mirrored"); }
        }
        assert_ne!(rune_cells(1, 5), rune_cells(2, 5), "different ids → different runes");
    }
}
