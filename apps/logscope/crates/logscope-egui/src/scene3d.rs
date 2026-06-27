//! scene3d.rs — the "baked scene": a themed 3D environment built ONCE from `mesh3d` primitives (a tree, a
//! moon-gate portal, floating stone islands, cairns, ground), procedurally textured with per-vertex colour,
//! lit by the live clock-sun, and gently FLOATING. The geometry is cached per palette; each frame only the
//! per-object float offset updates (cheap), then the whole set is rendered behind the conversation graph.
//!
//! No image textures — "texture" here is procedural vertex colour (moss on upward faces, bark by height,
//! mossy waterlines). The next rung up is a wgpu pass with real textures + shadows; this proves the scene.

use egui::Color32;
use crate::mesh3d::{self as m3, Mesh3, Light};
use crate::spatial::{OrbitCam, Vec3};
use crate::texgen::mat as M;

#[inline] fn v(x: f32, y: f32, z: f32) -> Vec3 { Vec3::new(x, y, z) }
#[inline] fn addv(a: Vec3, b: Vec3) -> Vec3 { v(a.x + b.x, a.y + b.y, a.z + b.z) }
#[inline] fn mulv(a: Vec3, s: f32) -> Vec3 { v(a.x * s, a.y * s, a.z * s) }
#[inline] fn normv(a: Vec3) -> Vec3 { let l = (a.x * a.x + a.y * a.y + a.z * a.z).sqrt(); if l > 1e-6 { mulv(a, 1.0 / l) } else { v(0.0, 1.0, 0.0) } }
#[inline] fn crossv(a: Vec3, b: Vec3) -> Vec3 { v(a.y * b.z - a.z * b.y, a.z * b.x - a.x * b.z, a.x * b.y - a.y * b.x) }

/// Recursive triplet branch (trunk → 3 → 9): a tapering wooden tube that splits into three children
/// splayed around the parent direction, with a leaf tuft at each tip. Stout + leaning is the caller's job
/// (the trunk direction). Matches the prototype's `branch()`.
fn tree_branch(s: &mut Scene, origin: Vec3, dir: Vec3, len: f32, rad: f32, depth: u32, seed: i32) {
    let dir = normv(dir);
    let end = addv(origin, mulv(dir, len));
    let mut br = m3::tube(origin, end, rad, 9, c(0x5b, 0x42, 0x28));
    m3::tint_height(&mut br, origin.y, end.y, c(0x3a, 0x28, 0x16), c(0x70, 0x55, 0x34)); br.mat = M::WOOD;
    s.add(br, Float::STILL);
    if depth == 0 {
        let mut puff = m3::leaf_cluster(end, len * 0.85, (seed as u32).wrapping_mul(131).wrapping_add(7), c(0x6e, 0x88, 0x34));
        m3::tint_up(&mut puff, c(0x9a, 0xb0, 0x46), c(0x4c, 0x66, 0x26)); puff.mat = M::MOSS;
        s.add(puff, Float { amp: 0.10, speed: 0.7, phase: hash(seed, 7) * 6.28, drift: 0.06 });
        return;
    }
    // a frame perpendicular to the branch to splay three children around it
    let up = if dir.y.abs() < 0.9 { v(0.0, 1.0, 0.0) } else { v(1.0, 0.0, 0.0) };
    let ax = normv(crossv(dir, up));
    let ay = crossv(dir, ax);
    let spread = 0.6; // how far children lean off the parent
    for k in 0..3 {
        let ang = std::f32::consts::TAU * k as f32 / 3.0 + hash(seed + k, 5) * 0.6;
        let nd = normv(addv(dir, addv(mulv(ax, ang.cos() * spread), mulv(ay, ang.sin() * spread))));
        tree_branch(s, end, nd, len * 0.72, rad * 0.66, depth - 1, seed * 3 + k + 1);
    }
}
#[inline] fn c(r: u8, g: u8, b: u8) -> Color32 { Color32::from_rgb(r, g, b) }
#[inline] fn hash(x: i32, s: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (s as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13; h = h.wrapping_mul(0xC2B2_AE35); h ^= h >> 16;
    (h & 0xFF_FFFF) as f32 / 0xFF_FFFF as f32
}

/// A gentle floating motion: vertical bob + a little lateral drift, on a per-object phase.
#[derive(Clone, Copy)]
pub struct Float { pub amp: f32, pub speed: f32, pub phase: f32, pub drift: f32 }
impl Float {
    pub const STILL: Float = Float { amp: 0.0, speed: 0.0, phase: 0.0, drift: 0.0 };
    fn offset(self, t: f32) -> Vec3 {
        v(self.drift * (t * self.speed * 0.5 + self.phase).sin(),
          self.amp * (t * self.speed + self.phase).sin(),
          self.drift * (t * self.speed * 0.4 + self.phase * 1.7).cos())
    }
}

/// A baked environment: parallel meshes + their float motions.
pub struct Scene { pub meshes: Vec<Mesh3>, floats: Vec<Float> }
impl Scene {
    fn new() -> Self { Scene { meshes: Vec::new(), floats: Vec::new() } }
    fn add(&mut self, m: Mesh3, f: Float) { self.meshes.push(m); self.floats.push(f); }
    fn add_all(&mut self, ms: Vec<Mesh3>, f: Float) { for m in ms { self.add(m, f); } }
    /// Update every object's float offset for time `t` (seconds). Cheap — no geometry rebuild.
    pub fn animate(&mut self, t: f32) {
        for (m, f) in self.meshes.iter_mut().zip(&self.floats) { m.offset = f.offset(t); }
    }
}

// ── themed builders ───────────────────────────────────────────────────────────

/// Canopy: a mossy ground, a bark-textured tree (trunk + branches + a foliage dome), and floating leaves.
/// A moss-covered stone bookshelf wall holding stone book-slates — the building block of the Canopy library
/// maze. `ang` is the direction the wall RUNS along (its face normal is perpendicular).
fn shelf_wall(s: &mut Scene, cx: f32, cz: f32, ang: f32, length: f32, height: f32, seed: u32) {
    let yb = -3.2_f32;
    let (dx, dz) = (ang.cos(), ang.sin());   // along the wall
    let (nx, nz) = (-dz, dx);                 // face normal
    let stone_lo = c(0x2a, 0x32, 0x28);
    let mut back = m3::panel(v(cx, yb + height * 0.5, cz), v(dx, 0.0, dz), v(0.0, 1.0, 0.0), length, height, stone_lo);
    m3::tint_height(&mut back, yb, yb + height, c(0x1e, 0x28, 0x1e), c(0x42, 0x4e, 0x3c)); back.mat = M::STONE; s.add(back, Float::STILL);
    let rows = (height / 0.95).max(1.0) as usize;
    let cols = (length / 0.34).max(1.0) as usize;
    for r in 0..rows {
        let sy = yb + 0.55 + r as f32 * 0.95;
        let mut ledge = m3::tube(v(cx - dx * length * 0.5, sy - 0.04, cz - dz * length * 0.5), v(cx + dx * length * 0.5, sy - 0.04, cz + dz * length * 0.5), 0.06, 4, c(0x48, 0x52, 0x42));
        ledge.mat = M::STONE; s.add(ledge, Float::STILL);
        for cc in 0..cols {
            let idx = (seed as usize).wrapping_add(r * 13 + cc * 7);
            if idx % 7 == 0 { continue; } // a few gaps on the shelf
            let t = (cc as f32 + 0.5) / cols as f32 - 0.5;
            let px = cx + dx * length * t + nx * 0.13;
            let pz = cz + dz * length * t + nz * 0.13;
            let slate = [c(0x9a, 0x86, 0x5c), c(0x82, 0x6e, 0x4c), c(0xa6, 0x8e, 0x62), c(0x70, 0x60, 0x46)][idx % 4]; // warm book-slates read against the stone
            let mut sl = m3::rock(v(px + nx * 0.08, sy + 0.28, pz + nz * 0.08), 0.2 + (idx % 3) as f32 * 0.04, (seed as i32 + r as i32 * 31 + cc as i32) as u32, slate);
            m3::scale_xz(&mut sl, 0.55, 1.0); // flatten into a slab/book that protrudes from the shelf
            sl.mat = M::STONE; s.add(sl, Float::STILL);
        }
    }
    // moss creeping over the stone
    for m in 0..3 {
        let tt = (hash(seed as i32, m * 3 + 1) - 0.5) * length;
        let my = if m == 0 { yb + height - 0.2 } else { yb + 0.2 + hash(seed as i32, m) * (height - 0.6) };
        let mut moss = m3::leaf_cluster(v(cx + dx * tt + nx * 0.16, my, cz + dz * tt + nz * 0.16), 0.45 + hash(seed as i32, m * 5) * 0.4, seed.wrapping_add(m as u32), c(0x4a, 0x70, 0x34));
        m3::tint_up(&mut moss, c(0x6e, 0x90, 0x40), c(0x32, 0x4e, 0x22)); moss.mat = M::MOSS;
        s.add(moss, Float { amp: 0.03, speed: 0.5, phase: hash(seed as i32, m) * 6.28, drift: 0.04 });
    }
}

/// Canopy: ONE great tree in an open library CIRCLE, ringed by a MAZE that fans out into moss-covered stone
/// shelves of book-slates — concentric broken arcs (gaps = the paths) plus radial spoke walls, a labyrinth
/// library grown around the tree.
/// An axis-aligned rectangular SLAB (5 flat faces, bottom omitted) at `c` with half-extents (hx,hy,hz).
fn box_slab(c: Vec3, hx: f32, hy: f32, hz: f32, color: Color32) -> Mesh3 {
    let faces = [
        (v(hx, 0.0, 0.0), v(0.0, 1.0, 0.0), v(0.0, 0.0, 1.0), 2.0 * hy, 2.0 * hz),
        (v(-hx, 0.0, 0.0), v(0.0, 0.0, 1.0), v(0.0, 1.0, 0.0), 2.0 * hz, 2.0 * hy),
        (v(0.0, hy, 0.0), v(0.0, 0.0, 1.0), v(1.0, 0.0, 0.0), 2.0 * hz, 2.0 * hx),
        (v(0.0, 0.0, hz), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0), 2.0 * hx, 2.0 * hy),
        (v(0.0, 0.0, -hz), v(0.0, 1.0, 0.0), v(1.0, 0.0, 0.0), 2.0 * hy, 2.0 * hx),
    ];
    let mut m = m3::panel(addv(c, faces[0].0), faces[0].1, faces[0].2, faces[0].3, faces[0].4, color);
    for f in &faces[1..] {
        let p = m3::panel(addv(c, f.0), f.1, f.2, f.3, f.4, color);
        let off = m.verts.len() as u32;
        m.verts.extend(p.verts); m.norms.extend(p.norms);
        for t in p.tris { m.tris.push([t[0] + off, t[1] + off, t[2] + off]); }
    }
    m
}

/// A DEEP, STRONG STONE shelf-wall for the cozy Canopy chamber, decorated with petrified-wood accents: a thick
/// slate slab + heavy stone ledges, with a vertical petrified-wood inlay strip and agate cabochon medallion(s)
/// proud of the inner face. `natural` segments carry a second, larger medallion (chosen off the seed).
fn petrified_wall(s: &mut Scene, wx: f32, wz: f32, ang: f32, len: f32, h: f32, yf: f32, seed: u32, natural: bool, deco: bool) {
    let along = v(ang.cos(), 0.0, ang.sin());
    let across = v(-along.z, 0.0, along.x);
    let up = v(0.0, 1.0, 0.0);
    let place = |m: &mut Mesh3, base: Vec3| {
        for p in m.verts.iter_mut() { let (x, y, z) = (p.x, p.y, p.z); *p = addv(base, addv(addv(mulv(across, x), mulv(up, y)), mulv(along, z))); }
        for nn in m.norms.iter_mut() { let (x, y, z) = (nn.x, nn.y, nn.z); *nn = addv(addv(mulv(across, x), mulv(up, y)), mulv(along, z)); }
    };
    // DEEP STRONG STONE base + heavy ledges
    let (slo, shi) = (c(0x33, 0x33, 0x3a), c(0x66, 0x68, 0x70));
    let thick = 1.0; // doubled shelf depth
    let mut wall = box_slab(v(0.0, 0.0, 0.0), thick, h * 0.5, len * 0.5, slo);
    place(&mut wall, v(wx, yf + h * 0.5, wz));
    m3::tint_height(&mut wall, yf, yf + h, slo, shi); wall.mat = M::STONE; s.add(wall, Float::STILL);
    let nsh = ((h / 1.9) as usize).max(1);
    for li in 1..=nsh {
        let sy = yf + h * li as f32 / (nsh as f32 + 1.0);
        let mut led = box_slab(v(0.0, 0.0, 0.0), thick + 0.13, 0.09, len * 0.5, shi);
        place(&mut led, v(wx, sy, wz)); m3::tint_up(&mut led, shi, slo); led.mat = M::STONE; s.add(led, Float::STILL);
    }
    // PETRIFIED-WOOD DECORATIONS proud of the inner (+across) face
    if deco {
        let (plo, phi) = (c(0x6e, 0x38, 0x1c), c(0xcc, 0x88, 0x40));
        let mut inlay = box_slab(v(thick + 0.03, 0.0, 0.0), 0.05, h * 0.40, len * 0.12, plo); // vertical agate strip
        place(&mut inlay, v(wx, yf + h * 0.5, wz));
        m3::tint_height(&mut inlay, yf, yf + h, plo, phi); inlay.mat = M::WOOD; inlay.emissive = 0.06; s.add(inlay, Float::STILL);
        for mi in 0..(if natural { 2 } else { 1 }) {
            let my = yf + h * (0.34 + 0.3 * mi as f32);
            let rr = (if natural && mi == 1 { 0.34 } else { 0.24 }) * (0.85 + hash(seed as i32, 7) * 0.3); // agate cabochon
            let mut cab = m3::uv_sphere(v(0.0, 0.0, 0.0), 1.0, 12, 9, plo);
            m3::scale3(&mut cab, 0.08, rr, rr);
            for p in cab.verts.iter_mut() { *p = addv(*p, v(thick + 0.06, 0.0, 0.0)); }
            place(&mut cab, v(wx, my, wz));
            m3::tint_up(&mut cab, phi, plo); cab.mat = M::WOOD; cab.emissive = 0.10; s.add(cab, Float::STILL);
        }
    }
}

/// A CURVED petrified-stone shelf-wall hugging a ring: an arc of radius `rad` spanning `arc` radians around
/// `a_mid`, faceted into short tangent slabs so the shelf reads as a concentric curve. Petrified decorations
/// land once, on the middle facet.
fn petrified_arc(s: &mut Scene, rad: f32, a_mid: f32, arc: f32, h: f32, yf: f32, seed: u32, natural: bool) {
    use std::f32::consts::PI;
    let n = ((arc * rad / 1.5).ceil() as usize).max(2);
    let mid = n / 2;
    for i in 0..n {
        let t = (i as f32 + 0.5) / n as f32;
        let a = a_mid - arc * 0.5 + arc * t;
        let (wx, wz) = (a.cos() * rad, a.sin() * rad);
        let seg_len = arc * rad / n as f32 * 1.06; // slight overlap so facets join cleanly
        petrified_wall(s, wx, wz, a + PI * 0.5, seg_len, h, yf, seed.wrapping_add(i as u32 * 7), natural, i == mid);
    }
}

/// A small colored LIGHT SHAFT slanting down from a high aperture: a soft translucent glowing column + a bright
/// little aperture disc at the top + a glow pool where it lands. Intimate, occasional — not a flooding window.
fn light_shaft(s: &mut Scene, x: f32, z: f32, y_top: f32, y_floor: f32, col: Color32, seed: u32) {
    let mut beam = m3::tube(v(x, y_top, z), v(x, y_floor + 0.08, z), 0.42, 14, col);
    beam.mat = M::MOSS; beam.emissive = 1.0; m3::set_alpha(&mut beam, 60); // soft translucent glow
    s.add(beam, Float { amp: 0.0, speed: 0.3, phase: hash(seed as i32, 1) * 6.28, drift: 0.0 });
    let mut ap = m3::disc(v(x, y_top, z), 0.5, 20, col); ap.mat = M::MOSS; ap.emissive = 1.9; s.add(ap, Float::STILL);
    let mut pool = m3::disc(v(x, y_floor + 0.05, z), 0.85, 26, col); m3::set_alpha(&mut pool, 140); pool.mat = M::MOSS; pool.emissive = 0.9;
    s.add(pool, Float { amp: 0.0, speed: 0.4, phase: hash(seed as i32, 2) * 6.28, drift: 0.04 });
}

fn canopy() -> Scene {
    use std::f32::consts::TAU;
    let pr = env_params();
    let mut s = Scene::new();
    let yf = -3.2_f32;
    // dark polished stone floor — the chamber is enclosed + cozy, lit mostly by the skylight's coloured light
    let mut floor = m3::disc(v(0.0, yf, 0.0), 30.0, 110, c(0x24, 0x20, 0x1c));
    m3::tint_up(&mut floor, c(0x3a, 0x32, 0x28), c(0x16, 0x13, 0x10)); floor.mat = M::STONE; floor.emissive = 0.03; s.add(floor, Float::STILL);

    // stained-glass palette (cobalt / ruby / gold / emerald / amethyst) — the angels read pale among these
    let glass = [c(0x3c, 0x6e, 0xc8), c(0xc8, 0x3a, 0x4e), c(0xe6, 0xc2, 0x54), c(0x46, 0xb0, 0x6e), c(0x8a, 0x5e, 0xc0)];

    // CENTRAL TREE — leafy green crown (tree 10% shorter, WIDER canopy). The calcite leaves were emissive and
    // blew out fully white at night, so the canopy is now natural non-emissive foliage sitting ABOVE the trunk
    // divergence (no more white bloomy clump at the fork).
    let tdir = normv(v(0.45, 1.0, 0.12)); let tlen = 6.0; // Canopy-only: ~24° lean off axis, short + wide trunk
    tree_branch(&mut s, v(0.0, yf, 0.0), tdir, tlen, 1.3, 2, 7);
    let cc = addv(v(0.0, yf, 0.0), addv(mulv(tdir, tlen), v(0.0, 0.6, 0.0))); // canopy rides the lean
    for k in 0..22 {
        let a = TAU * k as f32 / 22.0;
        let tier = (k % 3) as f32 * 0.7;
        let p = addv(cc, v(a.cos() * 3.4, tier + hash(k, 3) * 0.8 + 0.4, a.sin() * 3.4)); // wider spread + raised above the fork
        let mut leaf = m3::leaf_cluster(p, 1.4, 50 + k as u32, c(0x6c, 0x90, 0x40));
        m3::tint_up(&mut leaf, c(0x96, 0xb4, 0x58), c(0x46, 0x66, 0x2c)); leaf.mat = M::MOSS; // natural green foliage, no glow
        s.add(leaf, Float { amp: 0.06, speed: 0.5, phase: hash(k, 5) * 6.28, drift: 0.05 });
    }
    contact_shadow(&mut s, 0.0, 0.0, 3.0, yf, 110);

    // TALL CURVED PETRIFIED-STONE shelf-walls — concentric arcs that hug the circle (widened courtyard so the
    // central tree + graph have room). ~1 in 8 segments carries a raw "natural face" (seed-driven).
    let nat = |k: u32| -> bool { let mut h = pr.seed ^ k.wrapping_mul(0x9E37_79B9); h ^= h >> 13; (h & 7) == 0 };
    let radii = [16.0_f32, 20.5, 25.0]; // inner courtyard widened ~2x (8.5→16)
    for (ri, &rad) in radii.iter().enumerate() {
        let segs = 7 + ri * 2;
        for k in 0..segs {
            if hash(k as i32, 50 + ri as i32) < 0.16 { continue; } // few gaps → tight corridors
            let a_mid = TAU * (k as f32 + 0.5) / segs as f32 + ri as f32 * 0.3;
            let arc = (TAU / segs as f32) * 0.82; // the remainder is the gap = the way through
            let h = (6.5 + hash(k as i32, 60 + ri as i32) * 2.2) * 1.25; // shelves 25% taller
            petrified_arc(&mut s, rad, a_mid, arc, h, yf, (ri * 100 + k) as u32, nat((ri * 100 + k) as u32));
        }
    }
    // tall CURVED outer wall ring — the cozy chamber boundary, widened
    let osegs = 20;
    for k in 0..osegs {
        let a_mid = TAU * (k as f32 + 0.5) / osegs as f32;
        petrified_arc(&mut s, 28.0, a_mid, (TAU / osegs as f32) * 0.97, 11.25, yf, 800 + k as u32, nat(800 + k as u32));
    }
    // STEPPING STONES to walk the halls on — flat slabs scattered along the corridors + courtyard
    for k in 0..40 {
        let a = hash(k, 71) * TAU;
        let rad = 3.0 + (hash(k, 72)).sqrt() * 26.0;
        let (px, pz) = (a.cos() * rad, a.sin() * rad);
        let mut st = m3::disc(v(px, yf + 0.04, pz), 0.5 + hash(k, 73) * 0.55, 7, c(0x4a, 0x46, 0x40));
        m3::scale_xz(&mut st, 1.0, 0.7 + hash(k, 74) * 0.5);
        m3::tint_up(&mut st, c(0x68, 0x64, 0x5a), c(0x2e, 0x2c, 0x28)); st.mat = M::STONE; s.add(st, Float::STILL);
    }

    // SMALL OCCASIONAL coloured light SHAFTS slanting in from high apertures — intimate, not a flooding window.
    // Each is a soft glowing column + a bright little aperture + a glow pool; hue from the stained-glass palette.
    let nshaft = 3 + (pr.seed % 2) as usize;
    for k in 0..nshaft {
        let a = hash(k as i32, 41) * TAU; let rad = 2.5 + hash(k as i32, 42) * 7.5;
        let (sx, sz) = (a.cos() * rad, a.sin() * rad);
        let col = glass[(pr.seed as usize + k) % glass.len()];
        light_shaft(&mut s, sx, sz, yf + 9.5 + hash(k as i32, 43) * 1.5, yf, col, 970 + k as u32);
    }
    // dust motes drifting in the coloured shafts
    for k in 0..16 { let a = hash(k, 46) * TAU; let rad = hash(k, 47) * 11.0; firefly(&mut s, v(a.cos() * rad, yf + 1.0 + hash(k, 48) * 6.0, a.sin() * rad), 950 + k as u32); }
    s
}
/// floating stone islands topped with cairns.
/// A far-off ANTIGRAVITY waterfall: instead of falling to the ground the stream loops back on itself in a
/// vertical ring and recondenses at the top — a closed loop of flowing water + droplets, with a brighter
/// "recondense" knot at the crown. Sits out in the mist (fog blots the far side). `face_ang` aims the loop plane.
fn antigrav_fall(s: &mut Scene, cx: f32, cz: f32, base_y: f32, r: f32, face_ang: f32, seed: u32, col: Color32) {
    use std::f32::consts::TAU;
    let cen = v(cx, base_y + r, cz);
    let tang = v(face_ang.cos(), 0.0, face_ang.sin());
    let up = v(0.0, 1.0, 0.0);
    let pt = |th: f32| addv(cen, addv(mulv(tang, r * th.cos()), mulv(up, r * th.sin())));
    let n = 30;
    let mut prev = pt(0.0);
    for i in 1..=n {
        let th = TAU * i as f32 / n as f32;
        let p = pt(th);
        let mut seg = m3::tube(prev, p, 0.05 + 0.04 * th.sin().abs(), 5, col); // thins toward the bottom of the loop
        seg.mat = M::WATER; seg.emissive = 0.22;
        s.add(seg, Float { amp: 0.05, speed: 0.7, phase: hash(seed as i32, 1) * 6.28 + th, drift: 0.1 });
        prev = p;
        if i % 3 == 0 { // droplets streaming along the loop (the "falling" water)
            let di = (seed as i32).wrapping_add(i);
            let mut d = m3::uv_sphere(p, 0.10 + hash(di, 2) * 0.1, 10, 7, col);
            m3::tint_up(&mut d, c(0x9a, 0xd0, 0xdc), c(0x2c, 0x50, 0x58)); d.mat = M::WATER; d.emissive = 0.3;
            s.add(d, Float { amp: 0.25, speed: 0.8 + hash(di, 3) * 0.5, phase: hash(di, 4) * 6.28, drift: 0.35 });
        }
    }
    let mut knot = m3::uv_sphere(pt(TAU * 0.25), r * 0.16, 12, 9, c(0xb6, 0xe2, 0xec)); // recondense at the crown
    knot.mat = M::WATER; knot.emissive = 0.5; s.add(knot, Float { amp: 0.1, speed: 0.5, phase: 0.0, drift: 0.0 });
}

fn mistfall() -> Scene {
    let mut s = Scene::new();
    let ms = (map_radius() / 40.0).clamp(1.0, 2.6); // scale the water + scatter to fill the doubled room
    let td = tune_density(); // LOGSCOPE_TUNE_DENSITY → harness/Markov can search detail amount
    // DEEP water bed — a dark layer well below the surface; the translucent surface lets the depth read through
    let mut deep = m3::disc(v(0.0, -6.8, 0.0), 24.0 * ms, 90, c(0x04, 0x0c, 0x10));
    m3::tint_up(&mut deep, c(0x08, 0x16, 0x1c), c(0x02, 0x06, 0x08)); deep.mat = M::WATER; deep.emissive = 0.015; s.add(deep, Float::STILL);
    for k in 0..((10.0 * ms * td) as i32) { // submerged boulders on the bed → a depth cue
        let a = hash(k, 41) * std::f32::consts::TAU; let r = (hash(k, 42)).sqrt() * 14.0 * ms;
        let mut bd = m3::rock(v(a.cos() * r, -6.4 + hash(k, 43) * 0.4, a.sin() * r), 0.8 + hash(k, 44) * 1.2, 60 + k as u32, c(0x16, 0x24, 0x2a));
        m3::tint_up(&mut bd, c(0x22, 0x34, 0x3a), c(0x0c, 0x16, 0x1a)); bd.mat = M::STONE; s.add(bd, Float::STILL);
    }
    let mut water = m3::disc(v(0.0, -6.3, 0.0), 22.0 * ms, 110, c(0x12, 0x1e, 0x22)); // a low contained POOL (not a frame-filling sea) so it reads at the cavern bottom, well below the floating scene
    m3::tint_up(&mut water, c(0x1c, 0x30, 0x36), c(0x0a, 0x14, 0x16)); water.mat = M::WATER;
    water.emissive = 0.04; m3::set_alpha(&mut water, 104); // translucent → the deep bed + low objects show through as depth
    s.add(water, Float::STILL);
    // drifting MIST LAYERS — faint translucent sheets at varied heights for depth + atmosphere (height variation)
    for k in 0..((6.0 * ms) as i32) {
        let yy = -3.0 + hash(k, 90) * 9.0;
        let rad = (8.0 + hash(k, 91) * 16.0) * ms;
        let off = v((hash(k, 92) - 0.5) * 10.0 * ms, yy, (hash(k, 93) - 0.5) * 10.0 * ms);
        let mut veil = m3::disc(off, rad, 40, c(0x9c, 0xb0, 0xb8));
        veil.mat = M::WATER; veil.emissive = 0.05; m3::set_alpha(&mut veil, (22.0 + hash(k, 94) * 26.0) as u8);
        s.add(veil, Float { amp: 0.4, speed: 0.1 + hash(k, 95) * 0.12, phase: hash(k, 96) * 6.28, drift: 0.8 });
    }

    // PORTAL RINGS scale with the conversation: a big chat raises a vertical ARCHIPELAGO of standing portals — each
    // with a hue from the bioluminescent palette + its own height / size / phase (its local context) — not one ring.
    let n_portals = (env_params().nodes / 22).clamp(1, 5) as i32;
    let portal_hues = [c(0x4e, 0x66, 0x70), c(0x6a, 0x5a, 0x2e), c(0x5e, 0x2e, 0x52), c(0x2e, 0x66, 0x5a), c(0x46, 0x4e, 0x76)];
    let mut portal = |s: &mut Scene, px: f32, py: f32, pz: f32, rad: f32, idx: i32| {
        let hue = portal_hues[(idx as usize) % portal_hues.len()];
        let mut ring = m3::torus(v(px, py, pz), rad, 0.42 * (rad / 3.2), 72, 22, c(0x33, 0x44, 0x4c));
        m3::tint_height(&mut ring, py - 3.2, py + 3.2, c(0x26, 0x34, 0x3a), hue); ring.mat = M::STONE;
        s.add(ring, Float { amp: 0.05 + 0.04 * hash(idx, 31), speed: 0.4, phase: hash(idx, 32) * 6.28, drift: 0.0 });
    };
    portal(&mut s, 0.0, 0.6, -2.0, 3.2, 0); // the main portal, standing tilted slightly back
    for i in 1..n_portals {
        let ang = hash(i, 40) * std::f32::consts::TAU;
        let rad_out = (7.0 + hash(i, 41) * 5.0) * (1.0 + (ms - 1.0) * 0.55); // spaced out, still near enough for the fog
        let py = 0.6 + (hash(i, 42) - 0.35) * 5.0;     // across heights → the vertical archipelago
        portal(&mut s, ang.cos() * rad_out, py, -2.0 + ang.sin() * rad_out, 1.8 + hash(i, 43) * 1.4, i);
    }

    // floating stone islands + cairns — a VERTICAL ARCHIPELAGO: more islands spread wider and across heights
    let n_isle = ((5.0 + 8.0 * (ms - 1.0)).max(5.0) * td) as i32;
    for k in 0..n_isle {
        let ang = std::f32::consts::TAU * k as f32 / n_isle as f32 + hash(k, 1) * 0.5;
        let rad = (3.4 + hash(k, 2) * 8.0) * ms.min(2.0);
        let p = v(ang.cos() * rad, -1.0 + hash(k, 4) * 7.5, ang.sin() * rad * 0.8 + 0.5); // varied heights
        let mut isle = m3::rock(p, 0.9 + hash(k, 6) * 0.9, 200 + k as u32, c(0x3c, 0x46, 0x4a));
        m3::tint_up(&mut isle, c(0x4a, 0x60, 0x42), c(0x2c, 0x34, 0x38)); isle.mat = M::STONE; // mossy top, wet stone sides
        let fl = Float { amp: 0.22 + hash(k, 8) * 0.12, speed: 0.5 + hash(k, 10) * 0.3, phase: hash(k, 12) * 6.28, drift: 0.14 };
        s.add(isle, fl);
        if hash(k, 14) > 0.4 {
            let mut cairn = m3::cairn(v(p.x, p.y + 0.5, p.z), 3, 0.34, 300 + k as u32, c(0x6a, 0x72, 0x76));
            for cm in &mut cairn { m3::tint_up(cm, c(0x86, 0x90, 0x8e), c(0x4c, 0x54, 0x58)); cm.mat = M::STONE; }
            s.add_all(cairn, fl); // ride the same island
        }
    }
    // floating water droplets — gravity is strange in this room, so the surface breaks into drifting globules
    for k in 0..16 {
        let ang = hash(k as i32, 60) * std::f32::consts::TAU;
        let rad = 1.0 + hash(k as i32, 61) * 6.0;
        let p = v(ang.cos() * rad, -3.0 + hash(k as i32, 62) * 4.8, ang.sin() * rad * 0.8);
        let mut bub = m3::uv_sphere(p, 0.12 + hash(k as i32, 63) * 0.22, 18, 12, c(0x2a, 0x4c, 0x52));
        m3::tint_up(&mut bub, c(0x52, 0x86, 0x90), c(0x16, 0x2e, 0x34)); bub.mat = M::WATER; // shiny → wet
        s.add(bub, Float { amp: 0.5 + hash(k as i32, 64) * 0.6, speed: 0.6 + hash(k as i32, 65) * 0.7, phase: hash(k as i32, 66) * 6.28, drift: 0.45 });
    }
    waterfall(&mut s, -2.0, -1.5, 3.4, -5.1, 1.9, c(0x7a, 0xb0, 0xc0));
    waterfall(&mut s, 2.6, 0.4, 2.4, -5.1, 1.3, c(0x86, 0xc0, 0xcc));
    // a ring of weathered STANDING STONES / ruins around the water — some tall, some broken short, a couple
    // half-sunk near the shore. Gives Mistfall a megalithic, ancient-site feel rising from the mist.
    for k in 0..((14.0 * ms) as i32) {
        let a = hash(k, 79) * std::f32::consts::TAU;          // randomized angle (no longer an even ring)
        let rad = (5.0 + (hash(k, 80)).sqrt() * 11.0) * ms;   // spread out, scaled to the room
        let (sx, sz) = (a.cos() * rad, a.sin() * rad * 0.9);
        let broken = hash(k, 81) < 0.38;
        let h = if broken { 1.0 + hash(k, 82) * 1.2 } else { 2.6 + hash(k, 83) * 3.2 };
        standing_stone(&mut s, sx, sz, -3.4, h, 700 + k as u32);
    }
    // two low half-submerged stones near the centre
    for k in 0..2 { let a = 1.2 + k as f32 * 3.0; standing_stone(&mut s, a.cos() * 2.6, a.sin() * 2.0, -3.55, 0.9, 760 + k as u32); }

    // CAVE WALLS — a rough rock enclosure rising around the misty pool (Mistfall is a cavern, not open sky).
    // Tall irregular rock buttresses ringing the water; the fog blots their far side so there's no hard wall.
    use std::f32::consts::TAU;
    let wall_lo = c(0x20, 0x2a, 0x30); let wall_hi = c(0x3e, 0x4c, 0x54);
    for k in 0..30 {
        let a = TAU * k as f32 / 30.0 + hash(k, 90) * 0.2;
        let rad = 28.0 + hash(k, 91) * 6.0;
        let (wx, wz) = (a.cos() * rad, a.sin() * rad);
        let h = 9.0 + hash(k, 92) * 12.0;
        let rock_r = 3.0 + hash(k, 93) * 2.6;
        let mut wall = m3::rock(v(0.0, 0.0, 0.0), rock_r, 1400 + k as u32, wall_lo); // built at origin (±rock_r)
        m3::scale3(&mut wall, 1.0, h / (2.0 * rock_r), 1.0);                          // y-extent → h (tall buttress)
        let (cy, foot) = (-3.6 + h * 0.5, -3.6_f32);
        for p in wall.verts.iter_mut() { p.x += wx; p.y += cy; p.z += wz; }           // then place
        m3::tint_height(&mut wall, foot, foot + h, wall_lo, wall_hi); wall.mat = M::STONE; s.add(wall, Float::STILL);
    }
    // a dark cavern CEILING the stalactites actually hang from (flat dark roof, faded by the upper mist)
    let ceil_y = 13.5_f32;
    let mut ceil = m3::disc(v(0.0, ceil_y, 0.0), 34.0, 72, c(0x12, 0x18, 0x1c));
    m3::tint_up(&mut ceil, c(0x18, 0x20, 0x24), c(0x0a, 0x0e, 0x12)); ceil.mat = M::STONE;
    for nn in ceil.norms.iter_mut() { nn.x = -nn.x; nn.y = -nn.y; nn.z = -nn.z; } // face DOWN into the cavern
    s.add(ceil, Float::STILL);
    // longer, pointier, DARKER stalactites hanging from that ceiling
    let (tite_lo, tite_hi) = (c(0x0e, 0x14, 0x18), c(0x2c, 0x36, 0x3c));
    for k in 0..30 {
        let a = TAU * hash(k, 95);
        let rad = (hash(k, 96)).sqrt() * 26.0;
        let (sx, sz) = (a.cos() * rad, a.sin() * rad);
        let len = 3.0 + hash(k, 98) * 5.0;        // longer
        let wdt = 0.32 + hash(k, 99) * 0.34;      // narrow base → pointier
        stalactite(&mut s, sx, sz, ceil_y - 0.15, len, wdt, tite_lo, tite_hi);
    }

    // ANTIGRAVITY LOOPING WATERFALLS — far off in the mist: streams that fall away then loop back on themselves
    // and recondense at the crown. The conversation seed nudges how many + how big (graph-derived weather).
    let pr = env_params();
    let nfalls = 4 + (pr.seed % 3) as usize;
    for k in 0..nfalls {
        let a = TAU * k as f32 / nfalls as f32 + hash(k as i32, 70) * 0.6;
        let rad = 16.0 + hash(k as i32, 71) * 10.0;
        let (fx, fz) = (a.cos() * rad, a.sin() * rad);
        let loop_r = 2.6 + hash(k as i32, 72) * 2.4;
        antigrav_fall(&mut s, fx, fz, -2.4 + hash(k as i32, 73) * 1.5, loop_r, a + 1.57, 1500 + k as u32, c(0x8a, 0xc2, 0xd0));
    }
    // BIOLUMINESCENCE — glowing COLONIES in varied hues (amber, jade, magenta, cyan, lavender) lift the all-blue
    // palette into a living grotto. Each is a small CLUSTER (real screen area), not a speck, so colour actually reads.
    let glow_hues = [c(0xff, 0xb0, 0x5a), c(0x7a, 0xe0, 0x9a), c(0xe0, 0x7a, 0xc8), c(0x6a, 0xd0, 0xe0), c(0xc8, 0xb0, 0xff)];
    for k in 0..((22.0 * ms * td) as i32) {
        let a = hash(k, 110) * TAU;
        let rr = (2.0 + (hash(k, 111)).sqrt() * 13.0) * ms;
        let cy = -2.0 + hash(k, 112) * 8.0;
        let col = glow_hues[(k as usize) % glow_hues.len()];
        let cp = v(a.cos() * rr, cy, a.sin() * rr);
        let fl = Float { amp: 0.5 + hash(k, 114) * 0.7, speed: 0.3 + hash(k, 115) * 0.4, phase: hash(k, 116) * 6.28, drift: 0.5 };
        for j in 0..5i32 {                                  // a colony of glowing bulbs
            let off = v((hash(k * 9 + j, 1) - 0.5) * 1.3, (hash(k * 9 + j, 2) - 0.5) * 1.3, (hash(k * 9 + j, 3) - 0.5) * 1.3);
            let mut bulb = m3::uv_sphere(addv(cp, off), 0.16 + hash(k * 9 + j, 4) * 0.18, 10, 7, col);
            bulb.mat = M::PAPER; bulb.emissive = 0.95; s.add(bulb, fl);
        }
    }
    // a few larger glowing CORAL FANS in saturated hues, rooted near the shore — bigger colour masses
    for k in 0..((5.0 * ms * td) as i32) {
        let a = hash(k, 130) * TAU; let rr = (3.0 + hash(k, 131) * 9.0) * ms;
        let col = glow_hues[(k as usize + 2) % glow_hues.len()];
        let base = v(a.cos() * rr, -1.2 + hash(k, 132) * 2.0, a.sin() * rr);
        for j in 0..7i32 {
            let br = (j as f32 / 6.0) * 1.6;
            let bj = a + (hash(k, 133) - 0.5) + (j as f32 - 3.0) * 0.18;
            let tip = addv(base, v(bj.cos() * br * 0.5, 0.5 + br, bj.sin() * br * 0.5));
            let mut branch = m3::tube(base, tip, 0.05, 5, col); branch.mat = M::PAPER; branch.emissive = 0.7;
            s.add(branch, Float { amp: 0.08, speed: 0.5, phase: hash(k, 134) * 6.28, drift: 0.1 });
        }
    }
    // warm LANTERN orbs on the nearer islands — warm anchors against the cool water
    for k in 0..((4.0 * ms) as i32) {
        let a = hash(k, 120) * TAU; let rr = (4.0 + hash(k, 121) * 7.0) * ms;
        let mut lant = m3::uv_sphere(v(a.cos() * rr, -0.4 + hash(k, 122) * 3.0, a.sin() * rr), 0.32, 12, 9, c(0xff, 0xc2, 0x6a));
        lant.mat = M::PAPER; lant.emissive = 1.1;
        s.add(lant, Float { amp: 0.15, speed: 0.4, phase: hash(k, 123) * 6.28, drift: 0.1 });
    }
    s
}

/// Archive: warm wood floor and a rising column of autumn leaves caught in a magic gust (strong flutter +
/// an upward spiral).
fn archive() -> Scene {
    let mut s = Scene::new();
    let asf = (map_radius() / 40.0).clamp(1.0, 2.4); // scale the nave footprint to fill the doubled shell
    let yf = -3.2_f32;
    let (half, depth, h) = (6.5_f32 * asf, 12.0_f32 * asf, 9.2_f32 * (0.8 + 0.2 * asf));
    let yc = yf + h;
    let stone_lo = c(0x8e, 0x84, 0x72); // warm marble
    let stone_hi = c(0xd4, 0xca, 0xb6);
    // rectangular marble floor (a NAVE, not a rotunda) + a warm central aisle runner
    let mut floor = m3::panel(v(0.0, yf, 0.0), v(1.0, 0.0, 0.0), v(0.0, 0.0, 1.0), half * 2.2, depth * 2.0, c(0xb6, 0xac, 0x98));
    m3::tint_up(&mut floor, stone_hi, scale_rgb(stone_lo, 0.8)); floor.mat = M::STONE; s.add(floor, Float::STILL);
    let mut aisle = m3::panel(v(0.0, yf + 0.02, 0.0), v(1.0, 0.0, 0.0), v(0.0, 0.0, 1.0), 2.6, depth * 2.0, c(0x8a, 0x5c, 0x32));
    aisle.mat = M::STONE; aisle.emissive = 0.05; s.add(aisle, Float::STILL);
    // two rows of TALL columns down the long axis — the nave colonnade (more bays as the nave grows)
    let rows = (6.0 * asf).clamp(6.0, 14.0) as usize;
    for side in [-1.0_f32, 1.0] {
        for k in 0..rows {
            let zz = -depth + 1.7 + (k as f32 / (rows - 1) as f32) * (2.0 * depth - 3.4);
            room_column(&mut s, half * side, zz, yf, h, stone_lo, stone_hi, M::STONE);
        }
    }
    // tall arched CLERESTORY windows between the columns on each side — warm light pouring in (emissive).
    // The arch STYLE is graph-derived (env seed) so different conversations raise different architecture:
    // 0 Round·1 Segmental·2 Pointed·3 Lancet·4 Tudor·5 Ogee·6 Horseshoe·7 Multifoil·8 Keyhole·9 Trefoil·
    // 10 Cinquefoil·11 Elliptical·12 Parabolic·13 Catenary·14 Three-centered · COMPOSITE: 15 Serliana·
    // 16 Venetian·17 Triumphal (multi-opening). Override: LOGSCOPE_ARCH_STYLE.
    let arch_style = std::env::var("LOGSCOPE_ARCH_STYLE").ok().and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(env_params().seed % 18);
    let glasscol = c(0xff, 0xdc, 0x9a);
    for side in [-1.0_f32, 1.0] {
        for k in 0..(rows - 1) {
            let zz = -depth + 1.7 + ((k as f32 + 0.5) / (rows - 1) as f32) * (2.0 * depth - 3.4);
            let pos = v((half + 0.45) * side, yf + 3.0, zz);
            let (rt, upv) = (v(0.0, 0.0, side), v(0.0, 1.0, 0.0));
            if arch_style >= 15 {
                for (mut m, glass) in m3::composite_window(arch_style - 15, pos, rt, upv, 1.9, 3.4, glasscol) {
                    m.mat = M::STONE; m.emissive = if glass { 0.40 } else { 0.05 }; s.add(m, Float::STILL);
                }
            } else {
                let mut win = m3::arch_window_styled(pos, rt, upv, 1.9, 3.4, arch_style, glasscol);
                win.mat = M::STONE; win.emissive = 0.40; s.add(win, Float::STILL);
            }
        }
    }
    // vault ribs: each column top arches to a central ridge (the cathedral ceiling)
    for k in 0..rows {
        let zz = -depth + 1.7 + (k as f32 / (rows - 1) as f32) * (2.0 * depth - 3.4);
        for side in [-1.0_f32, 1.0] {
            let mut rib = m3::tube(v(half * side, yc - 0.2, zz), v(0.0, yc + 2.6, zz), 0.16, 8, stone_hi);
            rib.mat = M::STONE; s.add(rib, Float::STILL);
        }
    }
    let mut ridge = m3::tube(v(0.0, yc + 2.6, -depth), v(0.0, yc + 2.6, depth), 0.18, 8, stone_hi); ridge.mat = M::STONE; s.add(ridge, Float::STILL);
    // hanging CHANDELIERS down the nave centre at varied heights — warm pools of light
    for k in 0..rows {
        let zz = -depth + 2.0 + (k as f32 / (rows.max(2) - 1) as f32) * (2.0 * depth - 4.0);
        let yy = yc - 1.0 - (k as f32 * 1.3).sin().abs() * 1.6;
        let mut ring = m3::torus(v(0.0, yy, zz), 0.7, 0.06, 20, 5, c(0xff, 0xd2, 0x8a)); ring.mat = M::STONE; ring.emissive = 0.2; s.add(ring, Float::STILL);
        for cdl in 0..5 {
            let ca = cdl as f32 / 5.0 * std::f32::consts::TAU;
            let mut fl = m3::uv_sphere(v(ca.cos() * 0.7, yy + 0.05, zz + ca.sin() * 0.7), 0.12, 8, 6, c(0xff, 0xe6, 0xb0)); fl.mat = M::PAPER; fl.emissive = 1.1; s.add(fl, Float::STILL);
        }
        let mut chain = m3::tube(v(0.0, yy + 0.1, zz), v(0.0, yc + 2.4, zz), 0.02, 4, c(0x40, 0x36, 0x26)); chain.mat = M::STONE; s.add(chain, Float::STILL);
    }
    // floor TOMBS / reliquary chests along the side aisles
    for side in [-1.0_f32, 1.0] {
        for k in (0..rows).step_by(2) {
            let zz = -depth + 2.4 + (k as f32 / (rows.max(2) - 1) as f32) * (2.0 * depth - 4.8);
            let bx = (half - 1.6) * side;
            let mut chest = m3::tube(v(bx, yf, zz), v(bx, yf + 0.7, zz), 0.7, 4, c(0x6a, 0x5a, 0x3e)); m3::scale_xz(&mut chest, 1.6, 0.8); chest.mat = M::WOOD; s.add(chest, Float::STILL);
            let mut lid = m3::disc(v(bx, yf + 0.74, zz), 0.8, 4, c(0x8a, 0x76, 0x4e)); m3::scale_xz(&mut lid, 1.6, 0.8); lid.mat = M::WOOD; s.add(lid, Float::STILL);
            contact_shadow(&mut s, bx, zz, 1.0, yf, 100);
        }
    }
    // hanging BANNERS between the columns — deep warm cloth, faintly swaying
    for side in [-1.0_f32, 1.0] {
        for k in 0..(rows - 1) {
            let zz = -depth + 1.7 + ((k as f32 + 0.5) / (rows - 1) as f32) * (2.0 * depth - 3.4);
            let bcol = [c(0x7a, 0x2a, 0x2a), c(0x2a, 0x3a, 0x6a), c(0x2a, 0x5a, 0x3a)][(k as usize) % 3];
            let mut ban = m3::panel(v((half - 0.5) * side, yf + h * 0.5, zz), v(0.0, 0.0, side), v(0.0, 1.0, 0.0), 1.0, h * 0.5, bcol);
            ban.mat = M::PAPER; s.add(ban, Float { amp: 0.05, speed: 0.3, phase: hash(k as i32, 90) * 6.28, drift: 0.1 });
        }
    }
    // LECTERNS scale with the conversation: a long chat lines the nave with several glowing tomes (stations down
    // the aisle), each varied by a per-station seed, instead of one lectern marooned at the far end.
    let n_lec = (env_params().nodes / 22).clamp(1, 5) as i32;
    let mut lectern = |s: &mut Scene, px: f32, lz: f32, idx: i32| {
        let mut stand = m3::tube(v(px, yf, lz), v(px, yf + 1.4, lz), 0.34, 12, c(0x5a, 0x46, 0x2c)); stand.mat = M::WOOD; s.add(stand, Float::STILL);
        let mut tome = m3::paper_fold(v(px, yf + 1.6, lz), 1.2, 0.5, c(0xf6, 0xea, 0xc8)); tome.mat = M::PAPER; tome.emissive = 0.5 + 0.2 * hash(idx, 71);
        s.add(tome, Float { amp: 0.02, speed: 0.5, phase: hash(idx, 72) * 6.28, drift: 0.0 });
        contact_shadow(s, px, lz, 1.3, yf, 120);
    };
    lectern(&mut s, 0.0, depth - 1.6, 0); // the archive's heart, at the far end of the nave
    for i in 1..n_lec {
        let lz = (depth - 1.6) - i as f32 * ((2.0 * depth - 3.0) / n_lec as f32); // spread down the FULL nave
        let px = ((i % 2) as f32 - 0.5) * 3.6;                            // alternate further off the central aisle
        lectern(&mut s, px, lz, i);
    }
    // (removed: the autumn leaf-clusters that drifted up the nave — they read as floating green/amber balls
    //  and cluttered the architecture; the arch/window rhythm now carries the Archive's character instead.)
    s
}

/// Reading Room: pale paper floor and flying folded-paper origami drifting on magic currents.
fn reading_room() -> Scene {
    use std::f32::consts::TAU;
    let mut s = Scene::new();
    let mr = map_radius();
    let rs = (mr / 40.0).clamp(1.0, 2.8);          // scale the fit-out to fill the doubled room
    let yf = -3.0_f32;
    let warm_lo = c(0x6a, 0x60, 0x50);
    let warm_hi = c(0x86, 0x7c, 0x68);
    let stone_lo = c(0x8e, 0x86, 0x74);
    let stone_hi = c(0xc8, 0xc0, 0xae);
    let wood_lo = c(0x4a, 0x36, 0x20);
    let wood_hi = c(0x7a, 0x5a, 0x36);
    // raised reading-floor platform + a warm inlaid ring
    let mut floor = m3::disc(v(0.0, yf, 0.0), 12.0 * rs, 72, warm_lo);
    m3::tint_up(&mut floor, warm_hi, c(0x4c, 0x44, 0x38)); floor.mat = M::PAPER; s.add(floor, Float::STILL);
    let mut inlay = m3::annulus(v(0.0, yf + 0.02, 0.0), 11.0 * rs, 11.8 * rs, 72, c(0x9a, 0x7c, 0x4e));
    inlay.mat = M::PAPER; inlay.emissive = 0.05; s.add(inlay, Float::STILL);
    // a patterned RUG anchoring the reading floor — but CONTENT-DRIVEN: it stays a muted neutral mat until the
    // conversation actually talks about colour, then its jewel-tone bands emerge and intensify with the topic
    // (env_params().color_salience mixes raw frequency + term-frequency % over a colour lexicon — see chat.rs).
    let sal = std::env::var("LOGSCOPE_COLOR_SALIENCE").ok().and_then(|v| v.trim().parse::<f32>().ok())
        .unwrap_or(env_params().color_salience).clamp(0.0, 1.0); // env override for testing/tuning; else conversation-driven
    let neutral = c(0x6a, 0x60, 0x52); // undyed warm taupe — what the rug looks like when colour is never mentioned
    let rug_cols = [c(0x6e, 0x20, 0x26), c(0x9a, 0x7a, 0x30), c(0x1e, 0x4a, 0x44), c(0x28, 0x30, 0x5a), c(0x5e, 0x28, 0x40)];
    for (bi, &rc) in rug_cols.iter().enumerate() {
        let r_out = (6.2 - bi as f32 * 1.1) * rs;
        if r_out <= 0.3 { break; }
        let col = mix_rgb(neutral, rc, sal); // colour bands fade in with salience
        let mut band = m3::disc(v(0.0, yf + 0.03 + bi as f32 * 0.004, 0.0), r_out, 56, col);
        band.mat = M::PAPER; band.emissive = 0.02 + 0.04 * sal; s.add(band, Float::STILL);
    }
    // drifting tomes — more of them, spread + at varied heights
    for k in 0..((28.0 * rs) as i32) {
        let ang = hash(k, 2) * TAU;
        let rad = (1.0 + hash(k, 3) * 8.0) * rs;
        let p = v(ang.cos() * rad, -1.4 + hash(k, 4) * 6.5, ang.sin() * rad);
        let warm = [c(0xff, 0xf2, 0xd8), c(0xf0, 0xe2, 0xc0), c(0xe8, 0xd0, 0xa8)][(k as usize) % 3];
        let mut pf = m3::paper_fold(p, 0.30 + hash(k, 5) * 0.20, 0.24 + hash(k, 6) * 0.20, warm); pf.mat = M::PAPER;
        s.add(pf, Float { amp: 0.4 + hash(k, 7) * 0.5, speed: 0.7 + hash(k, 8) * 0.6, phase: hash(k, 9) * 6.28, drift: 0.6 });
    }
    // GROUND colonnade + an UPPER MEZZANINE gallery (height variation): a balcony walkway with railing + columns
    colonnade_ring(&mut s, (10.0 * rs) as usize, 9.0 * rs, yf, 6.5, stone_lo, stone_hi, M::STONE);
    let y_mezz = yf + 6.8;
    let mut balc = m3::annulus(v(0.0, y_mezz, 0.0), 9.0 * rs, 11.5 * rs, 80, stone_lo);
    m3::tint_up(&mut balc, stone_hi, stone_lo); balc.mat = M::STONE; s.add(balc, Float::STILL);
    let mut rail = m3::torus(v(0.0, y_mezz + 0.7, 0.0), 9.3 * rs, 0.07, 80, 6, stone_hi); rail.mat = M::STONE; s.add(rail, Float::STILL);
    colonnade_ring(&mut s, (10.0 * rs) as usize, 10.8 * rs, y_mezz, 5.5, stone_lo, stone_hi, M::STONE);
    // PERIMETER BOOKSHELVES ringing the wall on BOTH levels — the room reads as a deep library
    for lvl in 0..2u32 {
        let (yb, rad, hh) = if lvl == 0 { (yf, mr * 0.80, 6.4) } else { (y_mezz, mr * 0.86, 5.0) };
        let shelves = ((rad * 0.5) as usize).clamp(12, 40);
        for k in 0..shelves {
            let a = k as f32 / shelves as f32 * TAU + 0.15;
            let (bx, bz) = (a.cos() * rad, a.sin() * rad);
            let inward = normv(v(-bx, 0.0, -bz));
            let tangent = v(-a.sin(), 0.0, a.cos());
            bookcase_fill(&mut s, v(bx, yb, bz), inward, tangent, 3.0, hh, k as u32 * 7 + lvl * 131, wood_lo, wood_hi, 1.0);
        }
    }
    // reading desks (more) off-centre so the node strand stays clear
    for k in 0..8 {
        let a = TAU * k as f32 / 8.0 + 0.4;
        reading_desk(&mut s, a.cos() * 5.4 * rs, a.sin() * 5.4 * rs, yf);
    }
    // hanging LAMPS at varied heights — warm pools of light
    for k in 0..((7.0 * rs) as i32) {
        let a = hash(k, 70) * TAU;
        let rad = (2.0 + hash(k, 71) * 8.0) * rs;
        let y = yf + 4.5 + hash(k, 72) * 5.5;
        let mut globe = m3::uv_sphere(v(a.cos() * rad, y, a.sin() * rad), 0.28, 12, 10, c(0xff, 0xe2, 0xa6));
        globe.mat = M::PAPER; globe.emissive = 1.0;
        s.add(globe, Float { amp: 0.1, speed: 0.3, phase: hash(k, 73) * 6.28, drift: 0.0 });
        let mut cord = m3::tube(v(a.cos() * rad, y + 0.3, a.sin() * rad), v(a.cos() * rad, y + 3.4, a.sin() * rad), 0.02, 4, c(0x30, 0x28, 0x20));
        cord.mat = M::WOOD; s.add(cord, Float::STILL);
    }
    // loose book stacks between the desks
    for k in 0..((8.0 * rs) as i32) {
        let a = hash(k, 50) * TAU;
        let rad = (2.4 + hash(k, 51) * 6.0) * rs;
        let (bx, bz) = (a.cos() * rad, a.sin() * rad);
        let n = 2 + (hash(k, 52) * 4.0) as usize;
        for j in 0..n {
            let cols = [c(0xe8, 0xd6, 0xae), c(0xd6, 0xc2, 0x96), c(0xc0, 0xa6, 0x78), c(0xb0, 0x96, 0x68)];
            let mut bk = m3::paper_fold(v(bx, yf + 0.05 + j as f32 * 0.08, bz), 0.34 + hash(k, 53) * 0.1, 0.08, cols[j % 4]);
            bk.mat = M::PAPER; s.add(bk, Float::STILL);
        }
        contact_shadow(&mut s, bx, bz, 0.6, yf, 90);
    }
    // READING TABLES scale with the conversation: a big chat furnishes the floor with several glowing-tome tables
    // arranged around the central one (each varied per-station), instead of a single lonely table on the rug.
    let n_tbl = (env_params().nodes / 22).clamp(1, 5) as i32;
    let mut table = |s: &mut Scene, px: f32, pz: f32, scl: f32, idx: i32| {
        let mut tbl = m3::tube(v(px, yf, pz), v(px, yf + 0.9, pz), 1.4 * scl, 20, wood_lo); tbl.mat = M::WOOD; s.add(tbl, Float::STILL);
        let mut ttop = m3::disc(v(px, yf + 0.92, pz), 1.8 * scl, 30, c(0x6a, 0x50, 0x30)); ttop.mat = M::WOOD; s.add(ttop, Float::STILL);
        let mut tome = m3::paper_fold(v(px, yf + 1.0, pz), 0.5 * scl, 0.3, c(0xff, 0xf0, 0xd0)); tome.mat = M::PAPER; tome.emissive = 0.4 + 0.2 * hash(idx, 71);
        s.add(tome, Float { amp: 0.06, speed: 0.5, phase: hash(idx, 72) * 6.28, drift: 0.0 });
    };
    table(&mut s, 0.0, 0.0, 1.0, 0); // the central reading table
    for i in 1..n_tbl {
        let ang = std::f32::consts::FRAC_PI_4 + (i as f32 - 1.0) * std::f32::consts::FRAC_PI_2;
        let rad = (7.0 + i as f32 * 0.3) * rs; // pushed out toward the colonnade, staggered
        table(&mut s, ang.cos() * rad, ang.sin() * rad, 0.8, i);
    }
    s
}

/// Default (warm interior themes): a few floating stones over a plinth, so every theme gets some depth.
/// Folding@home: a biology void — a tall DNA/protein double-helix as the centerpiece (no stones), with
/// glossy residues, a twin backbone and base-pair rungs, drifting molecules around it, and a dim membrane
/// far below. Residues ride the PAPER material (palest) so their amino-acid colours read true.
fn foldingathome() -> Scene {
    use std::f32::consts::{PI, TAU};
    let mut s = Scene::new();
    let ms = (map_radius() / 40.0).clamp(1.0, 2.6);
    let res = [c(0x6e, 0xc6, 0xff), c(0x8b, 0xe0, 0x9b), c(0xc9, 0x8f, 0xff), c(0xff, 0x9b, 0x8a), c(0xe6, 0xb4, 0x5c)];
    // membrane bed scaled to the room + a lipid-bilayer speckle
    let mut membrane = m3::disc(v(0.0, -5.6, 0.0), 16.0 * ms, 64, c(0x1c, 0x22, 0x2e));
    m3::tint_up(&mut membrane, c(0x2a, 0x32, 0x42), c(0x12, 0x16, 0x20)); membrane.mat = M::STONE; s.add(membrane, Float::STILL);
    for k in 0..((60.0 * ms) as i32) {
        let a = hash(k, 80) * TAU; let rr = (hash(k, 81)).sqrt() * 16.0 * ms;
        let mut dot = m3::uv_sphere(v(a.cos() * rr, -5.5, a.sin() * rr), 0.12, 8, 6, c(0x35, 0x46, 0x58));
        dot.mat = M::STONE; dot.emissive = 0.1; s.add(dot, Float::STILL);
    }
    // double-helix builder (placed several times across the cell at varied positions + heights)
    let mut helix = |s: &mut Scene, cx: f32, cz: f32, y0: f32, turns: usize, rad: f32, seed: i32| {
        let (dy, twist) = (0.36f32, 0.5f32);
        let (mut pa, mut pb): (Option<Vec3>, Option<Vec3>) = (None, None);
        for k in 0..turns {
            let ang = k as f32 * twist + hash(seed, 1) * 6.28;
            let y = y0 + k as f32 * dy;
            let a = v(cx + rad * ang.cos(), y, cz + rad * ang.sin());
            let b = v(cx + rad * (ang + PI).cos(), y, cz + rad * (ang + PI).sin());
            if let Some(p) = pa { let mut t = m3::tube(p, a, 0.10, 12, c(0x4a, 0x86, 0xa8)); m3::tint_up(&mut t, c(0x6a, 0xb0, 0xcc), c(0x26, 0x48, 0x5c)); t.mat = M::STONE; s.add(t, Float::STILL); }
            if let Some(p) = pb { let mut t = m3::tube(p, b, 0.10, 12, c(0x4a, 0x86, 0xa8)); m3::tint_up(&mut t, c(0x6a, 0xb0, 0xcc), c(0x26, 0x48, 0x5c)); t.mat = M::STONE; s.add(t, Float::STILL); }
            if k % 2 == 0 { let mut rung = m3::tube(a, b, 0.05, 8, c(0xb6, 0xc2, 0xd0)); rung.mat = M::PAPER; s.add(rung, Float::STILL); }
            let mut sa = m3::uv_sphere(a, 0.24, 16, 12, res[k % res.len()]); sa.mat = M::PAPER;
            let mut sb = m3::uv_sphere(b, 0.24, 16, 12, res[(k + 2) % res.len()]); sb.mat = M::PAPER;
            s.add(sa, Float::STILL); s.add(sb, Float::STILL);
            pa = Some(a); pb = Some(b);
        }
    };
    helix(&mut s, 0.0, 0.0, -3.2, 26, 1.7, 0);                       // central tall helix
    for k in 0..((3.0 + 3.0 * (ms - 1.0)) as i32) {                  // satellite helices
        let a = hash(k, 10) * TAU; let rr = (5.0 + hash(k, 11) * 6.0) * ms;
        helix(&mut s, a.cos() * rr, a.sin() * rr, -3.0 + hash(k, 12) * 2.0, 10 + (hash(k, 13) * 10.0) as usize, 1.1 + hash(k, 14) * 0.6, k * 7 + 1);
    }
    // BETA-SHEET ribbons — short stacks of folded panels at varied heights
    for k in 0..((3.0 * ms) as i32) {
        let a = hash(k, 20) * TAU; let rr = (4.0 + hash(k, 21) * 7.0) * ms;
        let (px, pz) = (a.cos() * rr, a.sin() * rr);
        let yy = -2.0 + hash(k, 22) * 5.0;
        for j in 0..5i32 {
            let mut pf = m3::panel(v(px + j as f32 * 0.5 - 1.0, yy + (j as f32 * 1.3).sin() * 0.4, pz), v(1.0, 0.0, 0.0), v(0.0, 0.3, 1.0), 0.5, 1.2, res[(k as usize + j as usize) % res.len()]);
            pf.mat = M::PAPER; pf.emissive = 0.2; s.add(pf, Float { amp: 0.2, speed: 0.3, phase: hash(k, 23) * 6.28, drift: 0.2 });
        }
    }
    // folded GLOBULES — clusters of residue spheres at varied heights
    for k in 0..((5.0 * ms) as i32) {
        let a = hash(k, 30) * TAU; let rr = (3.0 + hash(k, 31) * 8.0) * ms;
        let cp = v(a.cos() * rr, -1.0 + hash(k, 32) * 7.0, a.sin() * rr);
        let fl = Float { amp: 0.3, speed: 0.3 + hash(k, 33) * 0.3, phase: hash(k, 34) * 6.28, drift: 0.3 };
        for j in 0..6i32 {
            let off = v((hash(k * 7 + j, 1) - 0.5) * 1.2, (hash(k * 7 + j, 2) - 0.5) * 1.2, (hash(k * 7 + j, 3) - 0.5) * 1.2);
            let mut sp = m3::uv_sphere(addv(cp, off), 0.18 + hash(k * 7 + j, 4) * 0.16, 12, 9, res[(k as usize + j as usize) % res.len()]); sp.mat = M::PAPER;
            s.add(sp, fl);
        }
    }
    // VESICLES — translucent membrane bubbles drifting at varied heights
    for k in 0..((6.0 * ms) as i32) {
        let a = hash(k, 40) * TAU; let rr = (4.0 + hash(k, 41) * 9.0) * ms;
        let p = v(a.cos() * rr, -2.0 + hash(k, 42) * 8.0, a.sin() * rr);
        let mut ves = m3::uv_sphere(p, 0.5 + hash(k, 43) * 0.8, 16, 12, c(0x4c, 0x86, 0x9a));
        ves.mat = M::WATER; ves.emissive = 0.08; m3::set_alpha(&mut ves, 70);
        s.add(ves, Float { amp: 0.5, speed: 0.2 + hash(k, 44) * 0.3, phase: hash(k, 45) * 6.28, drift: 0.5 });
    }
    // a faint distributed-COMPUTE LATTICE filling the upper volume (Folding@home = distributed compute): sparse
    // emissive work-unit nodes each linked to a pseudo-neighbour, slowly drifting — fills height + carries theme.
    for k in 0..((40.0 * ms) as i32) {
        let a = hash(k, 50) * TAU; let rr = (hash(k, 51)).sqrt() * 14.0 * ms;
        let p = v(a.cos() * rr, 1.0 + hash(k, 52) * 9.0, a.sin() * rr);
        let mut node = m3::uv_sphere(p, 0.06 + hash(k, 53) * 0.06, 6, 5, c(0x7a, 0xd0, 0xe0)); node.mat = M::PAPER; node.emissive = 0.9;
        s.add(node, Float { amp: 0.3, speed: 0.2, phase: hash(k, 54) * 6.28, drift: 0.4 });
        let a2 = hash(k, 55) * TAU; let rr2 = (hash(k, 56)).sqrt() * 14.0 * ms;
        let p2 = v(a2.cos() * rr2, 1.0 + hash(k, 57) * 9.0, a2.sin() * rr2);
        let mut link = m3::tube(p, p2, 0.012, 4, c(0x3a, 0x6a, 0x80)); link.mat = M::STONE; link.emissive = 0.3; s.add(link, Float::STILL);
    }
    // free molecules drifting through the cell (more, scaled out)
    for k in 0..((18.0 * ms) as i32) {
        let ang = hash(k, 2) * 6.28; let rr = (3.2 + hash(k, 3) * 5.0) * ms;
        let p = v(ang.cos() * rr, -2.5 + hash(k, 4) * 8.0, ang.sin() * rr);
        let mut mol = m3::uv_sphere(p, 0.10 + hash(k, 5) * 0.16, 14, 10, res[k as usize % res.len()]); mol.mat = M::PAPER;
        s.add(mol, Float { amp: 0.5 + hash(k, 6) * 0.7, speed: 0.5 + hash(k, 7) * 0.6, phase: hash(k, 8) * 6.28, drift: 0.5 });
    }
    s
}

/// Wrap a scene in an enclosure — the architecture that makes the graph feel contained, not adrift (the
/// references are dominated by enclosure geometry, not by the centerpiece). A wide sunken outer floor, a tall
/// circular wall (bookshelf wood for the libraries, raw stone for the grottoes) banded with shelving or rock
/// strata, and an oculus ring high overhead hinting at a dome beyond — the source of light from outside the
/// room. Distance + the shader's fog turn the far wall atmospheric. Proportioned after the references: wall
/// height ≈ ⅓ the room diameter, the centerpiece staying ~15% of the volume.
fn enclose(s: &mut Scene, wall_mat: f32, wall_lo: Color32, wall_hi: Color32, shelf: bool, outer_r: f32) {
    let r = outer_r.max(40.0);
    let sc = r / 40.0;            // scale the room's vertical + inner proportions with its radius
    let h = 22.0 * sc;           // wall height
    let y0 = -4.4_f32;           // a step below the central platform
    let mut floor = m3::disc(v(0.0, y0, 0.0), r + 2.0, 88, wall_lo);
    m3::tint_up(&mut floor, wall_hi, wall_lo); floor.mat = wall_mat;
    s.add(floor, Float::STILL);
    let mut wall = m3::wall_ring(v(0.0, y0, 0.0), r, h, 96, wall_lo);
    m3::tint_height(&mut wall, y0, y0 + h, wall_lo, wall_hi); wall.mat = wall_mat; // lit toward the dome
    s.add(wall, Float::STILL);
    let bands = if shelf { 7 } else { 4 }; // bookshelves vs rock strata
    for i in 1..bands {
        let hb = y0 + h * i as f32 / bands as f32;
        let mut band = m3::torus(v(0.0, hb, 0.0), r - 0.25, if shelf { 0.22 } else { 0.42 }, 96, 6, wall_hi);
        m3::tint_up(&mut band, wall_hi, wall_lo); band.mat = wall_mat;
        s.add(band, Float::STILL);
    }
    // (removed: the 3 concentric floor TERRACES — those big origin-centred nested annuli read as distracting
    //  rings cutting across the room. The floor disc + central platform carry the ground; objects sit directly.)
    // the dome closing the room, narrowing to an oculus skylight high overhead
    let rise = 14.0 * sc;
    let mut dome = m3::dome_cap(v(0.0, y0 + h, 0.0), r, rise, r * 0.28, 72, 8, wall_lo);
    m3::tint_height(&mut dome, y0 + h, y0 + h + rise, wall_lo, wall_hi); dome.mat = wall_mat;
    s.add(dome, Float::STILL);
    // (removed: the oculus rim TORUS — that wide overhead ring read as a stray 'concentric' cutting the view.
    //  The bright skylight disc below stays as the bloom/god-ray source.)
    // a bright skylight disc filling the oculus — bloom source + something for the god-ray pass to streak from
    let mut sky = m3::disc(v(0.0, y0 + h + 0.9, 0.0), r * 0.27, 56, wall_hi);
    m3::tint_up(&mut sky, wall_hi, wall_hi); sky.mat = wall_mat; sky.emissive = 1.7;
    s.add(sky, Float::STILL);
}

/// GARDEN MARBLE PLANET ("Mansion Garden") — a grand cream-marble conservatory: tiered dais under a ribbed glass
/// dome, a colonnade of marble columns, turquoise pools + a moat, palms/ferns around the edges (centre kept clear
/// for the graph), GOLD VEINS the shader sweeps light-pulses along at night, and GOLDEN RAIN that falls and
/// re-stains the "memory marble" floor (which slowly returns to white). `sc` scales the whole conservatory.
fn mansion_garden() -> Scene {
    use std::f32::consts::TAU;
    let mut s = Scene::new();
    let yf = -3.4_f32;
    let sc = 1.6_f32 * (map_radius() / 64.0).clamp(1.0, 2.2); // grander conservatory, growing with the graph
    let (mlo, mhi) = (c(0xa0, 0x98, 0x82), c(0xc8, 0xc2, 0xae)); // cream marble (toned further)
    let gold = c(0xc2, 0x98, 0x40);
    let water = c(0x2c, 0x96, 0xa0);
    let mem = 0.22_f32;               // "memory marble" flag read by the shader (golden-rain re-texturing)

    // MARBLE FLOOR (memory marble) + a deep water layer beneath
    let mut floor = m3::disc(v(0.0, yf, 0.0), 24.0 * sc, 112, mlo);
    m3::tint_up(&mut floor, mhi, mlo); floor.mat = M::STONE; floor.emissive = mem; s.add(floor, Float::STILL);
    let mut deep = m3::disc(v(0.0, yf - 1.2, 0.0), 24.0 * sc, 80, c(0x06, 0x2e, 0x36));
    deep.mat = M::WATER; deep.emissive = 0.02; s.add(deep, Float::STILL);

    // WATER: scattered pools toward the edges (the central MOAT RING was removed per request — varied pools instead)
    for k in 0..8 {
        let a = hash(k, 21) * TAU; let r = (12.0 + hash(k, 22) * 9.0) * sc;
        let mut pool = m3::disc(v(a.cos() * r, yf + 0.05, a.sin() * r), (1.6 + hash(k, 23) * 2.2) * sc, 44, water);
        m3::tint_up(&mut pool, c(0x4c, 0xc4, 0xc8), c(0x16, 0x6c, 0x76)); pool.mat = M::WATER; pool.emissive = 0.10; m3::set_alpha(&mut pool, 205); s.add(pool, Float::STILL);
    }

    // CENTRAL TIERED DAIS (memory marble; low so the graph floats above it)
    for i in 0..3 {
        let r = (6.4 - i as f32 * 1.7) * sc; let y = yf + (0.12 + i as f32 * 0.34) * sc;
        let mut step = m3::disc(v(0.0, y, 0.0), r, 72, mhi);
        m3::tint_up(&mut step, mhi, mlo); step.mat = M::STONE; step.emissive = mem; s.add(step, Float::STILL);
        // (rim rings removed — they read as concentric rings cutting the centre)
    }

    // COLONNADE of cream-marble columns carrying the dome (one entrance bay open) + architrave
    let cols = 18; let r_col = 11.5 * sc; let y_arch = yf + 9.6 * sc;
    for k in 0..cols {
        let a = TAU * k as f32 / cols as f32;
        if a.sin().abs() < 0.16 && a.cos() > 0.0 { continue; }
        room_column(&mut s, a.cos() * r_col, a.sin() * r_col, yf, 9.5 * sc, mlo, mhi, M::STONE);
    }
    let mut arch = m3::torus(v(0.0, y_arch, 0.0), r_col, 0.4 * sc, 112, 10, mhi);
    m3::tint_up(&mut arch, mhi, mlo); arch.mat = M::STONE; arch.emissive = 0.05; s.add(arch, Float::STILL);

    // RIBBED GLASS DOME
    let mut dome = m3::dome_cap(v(0.0, y_arch, 0.0), r_col + 0.4 * sc, 7.0 * sc, 0.0, 88, 10, c(0xcf, 0xe8, 0xea));
    dome.mat = M::WATER; dome.emissive = 0.05; m3::set_alpha(&mut dome, 66); s.add(dome, Float::STILL);
    let apex = v(0.0, y_arch + 7.0 * sc, 0.0);
    for k in 0..18 {
        let a = TAU * k as f32 / 18.0;
        let base = v(a.cos() * (r_col + 0.4 * sc), y_arch, a.sin() * (r_col + 0.4 * sc));
        let mut rib = m3::tube(base, apex, 0.1 * sc, 6, mhi); m3::tint_up(&mut rib, mhi, mlo); rib.mat = M::STONE; rib.emissive = 0.05; s.add(rib, Float::STILL);
    }

    // GOLD VEINS — emissive 5.5 (shader sweeps a travelling pulse along them: the 4.5..6.0 branch)
    for k in 0..18 {
        let a = TAU * k as f32 / 18.0 + 0.1;
        let p0 = v(a.cos() * 6.6 * sc, yf + 0.07, a.sin() * 6.6 * sc);
        let p1 = v(a.cos() * 23.0 * sc, yf + 0.07, a.sin() * 23.0 * sc);
        let mut vein = m3::tube(p0, p1, 0.06 * sc, 5, gold); vein.mat = M::STONE; vein.emissive = 5.5; s.add(vein, Float::STILL);
    }
    for k in 0..cols {
        if hash(k as i32, 40) < 0.5 { continue; }
        let a = TAU * k as f32 / cols as f32;
        let base = v(a.cos() * r_col, yf, a.sin() * r_col);
        let top = v(a.cos() * r_col, yf + 9.4 * sc, a.sin() * r_col);
        let mut vein = m3::tube(base, top, 0.05 * sc, 5, gold); vein.mat = M::STONE; vein.emissive = 5.5; s.add(vein, Float::STILL);
    }

    // GOLDEN RAIN — thin gold streaks the vertex shader loops downward (emissive 7.0 = raindrop flag). Where they
    // land, the memory-marble shader restains the floor gold, then it fades back to white.
    for k in 0..150 {
        let a = hash(k, 81) * TAU; let r = (hash(k, 82)).sqrt() * 21.0 * sc;
        let (px, pz) = (a.cos() * r, a.sin() * r);
        let topy = yf + (17.0 + hash(k, 83) * 3.0) * sc;
        let mut drop = m3::tube(v(px, topy, pz), v(px, topy - 0.7, pz), 0.03 * sc, 4, c(0xe8, 0xc4, 0x60));
        drop.mat = M::WATER; drop.emissive = 7.0; s.add(drop, Float::STILL);
    }

    // PALMS — fuller, smaller, more-connected crowns; GENTLE wind (less severe than other scenes)
    for k in 0..20 {
        let a = hash(k, 51) * TAU; let r = (13.5 + hash(k, 52) * 8.0) * sc;
        let (px, pz) = (a.cos() * r, a.sin() * r);
        let lean = normv(v((hash(k, 53) - 0.5) * 0.4, 1.0, (hash(k, 54) - 0.5) * 0.4));
        let th = (3.0 + hash(k, 55) * 2.5) * sc;
        let top = addv(v(px, yf, pz), mulv(lean, th));
        let mut trunk = m3::tube(v(px, yf, pz), top, 0.16 * sc, 7, c(0x6e, 0x56, 0x32));
        m3::tint_up(&mut trunk, c(0x86, 0x6a, 0x3e), c(0x4a, 0x38, 0x20)); trunk.mat = M::WOOD; s.add(trunk, Float::STILL);
        for f in 0..9 { // two tight tiers of smaller clusters → a fuller, connected crown
            let fa = TAU * f as f32 / 9.0 + (f % 2) as f32 * 0.35;
            let rr = (0.45 + (f % 2) as f32 * 0.5) * sc;
            let fp = addv(top, v(fa.cos() * rr, (0.15 - (f % 2) as f32 * 0.12) * sc, fa.sin() * rr));
            let mut frond = m3::leaf_cluster(fp, 0.7 * sc, 60 + (k * 9 + f) as u32, c(0x4e, 0x80, 0x36));
            m3::tint_up(&mut frond, c(0x6e, 0x9c, 0x40), c(0x32, 0x56, 0x22)); frond.mat = M::MOSS;
            s.add(frond, Float { amp: 0.03, speed: 0.45, phase: hash((k * 9 + f) as i32, 1) * 6.28, drift: 0.04 });
        }
        contact_shadow(&mut s, px, pz, 1.0 * sc, yf, 90);
    }
    for k in 0..26 { // low ground ferns, gentle wind
        let a = hash(k, 61) * TAU; let r = (10.0 + hash(k, 62) * 12.0) * sc;
        let mut fern = m3::leaf_cluster(v(a.cos() * r, yf + 0.4, a.sin() * r), 0.62 * sc, 80 + k as u32, c(0x46, 0x76, 0x30));
        m3::tint_up(&mut fern, c(0x64, 0x92, 0x3a), c(0x2c, 0x4c, 0x1e)); fern.mat = M::MOSS;
        s.add(fern, Float { amp: 0.025, speed: 0.45, phase: hash(k, 2) * 6.28, drift: 0.035 });
    }
    // ---- TROPICAL OVERGROWTH (user: too barren → fill it in, vertically too) ----
    // broad-leaf tropical plants fanning up from the floor
    for k in 0..30 {
        let a = hash(k, 180) * TAU; let r = (5.0 + (hash(k, 181)).sqrt() * 16.0) * sc;
        let (px, pz) = (a.cos() * r, a.sin() * r);
        let blades = 4 + (hash(k, 182) * 3.0) as usize;
        for b in 0..blades {
            let ba = TAU * b as f32 / blades as f32 + hash(k, 183) * 6.28;
            let tip = v(px + ba.cos() * 0.6 * sc, yf + (1.1 + hash(k * 7 + b as i32, 1) * 0.8) * sc, pz + ba.sin() * 0.6 * sc);
            let mut leaf = m3::leaf_cluster(tip, 0.7 * sc, 190 + (k * 7 + b as i32) as u32, c(0x44, 0x78, 0x30));
            m3::scale3(&mut leaf, 0.7, 1.5, 0.7); m3::tint_up(&mut leaf, c(0x64, 0x96, 0x3a), c(0x2c, 0x4e, 0x1c)); leaf.mat = M::MOSS;
            s.add(leaf, Float { amp: 0.03, speed: 0.5, phase: hash(k * 7 + b as i32, 2) * 6.28, drift: 0.04 });
        }
        contact_shadow(&mut s, px, pz, 0.8 * sc, yf, 70);
    }
    // VINES spiralling UP the main columns (vertical growth) with leaves along the way
    for k in 0..cols {
        if hash(k as i32, 140) < 0.5 { continue; }
        let a = TAU * k as f32 / cols as f32;
        let (cx, cz) = (a.cos() * r_col, a.sin() * r_col);
        let segs = 11; let mut prev = v(cx + 0.3 * sc, yf, cz);
        for sgi in 1..=segs {
            let t = sgi as f32 / segs as f32; let spin = t * TAU * 2.0; let off = 0.34 * sc;
            let pt = v(cx + spin.cos() * off, yf + t * 9.0 * sc, cz + spin.sin() * off);
            let mut vseg = m3::tube(prev, pt, 0.05 * sc, 5, c(0x3e, 0x60, 0x2a));
            m3::tint_up(&mut vseg, c(0x56, 0x7c, 0x30), c(0x26, 0x3e, 0x18)); vseg.mat = M::MOSS; s.add(vseg, Float::STILL);
            if sgi % 2 == 0 {
                let mut lf = m3::leaf_cluster(pt, 0.3 * sc, 150 + (k as u32 * 13 + sgi as u32), c(0x52, 0x84, 0x38));
                m3::tint_up(&mut lf, c(0x70, 0x9c, 0x42), c(0x30, 0x52, 0x20)); lf.mat = M::MOSS;
                s.add(lf, Float { amp: 0.03, speed: 0.5, phase: hash(k as i32 * 13 + sgi, 1) * 6.28, drift: 0.04 });
            }
            prev = pt;
        }
    }
    // SUB-PILLARS — shorter marble pillars with a planter + tropical tuft on top (more verticality)
    for k in 0..10 {
        let a = hash(k, 120) * TAU + 0.3; let r = (7.0 + hash(k, 121) * 6.0) * sc;
        let (px, pz) = (a.cos() * r, a.sin() * r); let ph = (3.5 + hash(k, 122) * 2.0) * sc;
        room_column(&mut s, px, pz, yf, ph, mlo, mhi, M::STONE);
        let mut bowl = m3::disc(v(px, yf + ph, pz), 0.55 * sc, 18, mhi); m3::tint_up(&mut bowl, mhi, mlo); bowl.mat = M::STONE; s.add(bowl, Float::STILL);
        let mut tuft = m3::leaf_cluster(v(px, yf + ph + 0.5 * sc, pz), 0.85 * sc, 130 + k as u32, c(0x4e, 0x82, 0x36));
        m3::tint_up(&mut tuft, c(0x70, 0x9e, 0x42), c(0x32, 0x56, 0x22)); tuft.mat = M::MOSS;
        s.add(tuft, Float { amp: 0.03, speed: 0.5, phase: hash(k, 123) * 6.28, drift: 0.05 });
    }
    // HALF-GLASS-DOME cloches scattered over plant clusters
    for k in 0..6 {
        let a = hash(k, 160) * TAU + 0.7; let r = (8.0 + hash(k, 161) * 9.0) * sc;
        let (gx, gz) = (a.cos() * r, a.sin() * r); let gr = (1.5 + hash(k, 162) * 1.1) * sc;
        let mut gdome = m3::dome_cap(v(gx, yf + 0.05, gz), gr, gr, 0.0, 40, 7, c(0xcf, 0xe8, 0xea));
        gdome.mat = M::STONE; gdome.emissive = 0.05; m3::set_alpha(&mut gdome, 88); s.add(gdome, Float::STILL);
        let mut rim = m3::torus(v(gx, yf + 0.1, gz), gr, 0.08 * sc, 32, 6, mhi); rim.mat = M::STONE; s.add(rim, Float::STILL);
        let mut plant = m3::leaf_cluster(v(gx, yf + 0.6 * sc, gz), gr * 0.5, 170 + k as u32, c(0x4e, 0x82, 0x36));
        m3::tint_up(&mut plant, c(0x70, 0x9e, 0x42), c(0x32, 0x56, 0x22)); plant.mat = M::MOSS;
        s.add(plant, Float { amp: 0.02, speed: 0.5, phase: hash(k, 163) * 6.28, drift: 0.02 });
    }
    // POOL PATHWAYS — elongated water channels linking the round pools into varied paths
    for k in 0..6 {
        let a = hash(k, 200) * TAU; let r = (6.0 + hash(k, 201) * 10.0) * sc;
        let (cx, cz) = (a.cos() * r, a.sin() * r);
        let mut chan = m3::disc(v(cx, yf + 0.05, cz), (2.6 + hash(k, 202) * 2.2) * sc, 44, water);
        m3::tint_up(&mut chan, c(0x4c, 0xc4, 0xc8), c(0x16, 0x6c, 0x76)); chan.mat = M::WATER; chan.emissive = 0.10; m3::set_alpha(&mut chan, 200);
        m3::scale_xz(&mut chan, 1.0, 0.30); s.add(chan, Float::STILL); // elongated → channel
    }
    s
}

fn islands() -> Scene {
    let mut s = Scene::new();
    let mut floor = m3::disc(v(0.0, -3.0, 0.0), 12.0, 64, c(0x2a, 0x24, 0x1c));
    m3::tint_up(&mut floor, c(0x40, 0x36, 0x26), c(0x1e, 0x1a, 0x14)); floor.mat = M::STONE;
    s.add(floor, Float::STILL);
    for k in 0..5 {
        let ang = std::f32::consts::TAU * k as f32 / 5.0;
        let p = v(ang.cos() * (3.0 + hash(k, 2) * 2.0), -0.5 + hash(k, 4) * 2.0, ang.sin() * (3.0 + hash(k, 3) * 2.0));
        let mut r = m3::rock(p, 0.7 + hash(k as i32, 6) * 0.4, 400 + k as u32, c(0x5a, 0x4c, 0x38));
        m3::tint_up(&mut r, c(0x7a, 0x68, 0x46), c(0x3c, 0x32, 0x24)); r.mat = M::STONE;
        s.add(r, Float { amp: 0.18 + hash(k, 8) * 0.1, speed: 0.5 + hash(k, 9) * 0.3, phase: hash(k, 11) * 6.28, drift: 0.12 });
    }
    s
}

/// Old Library: instead of stones, a great lectern with an open, faintly-glowing tome at the centre, and
/// loose pages drifting up around it (a page turning itself — presence). Enclosed by bookshelf walls.
fn old_library() -> Scene {
    let mut s = Scene::new();
    let mut floor = m3::disc(v(0.0, -3.0, 0.0), 34.0, 120, c(0x2e, 0x22, 0x14));
    m3::tint_up(&mut floor, c(0x4a, 0x38, 0x20), c(0x20, 0x18, 0x0e)); floor.mat = M::WOOD;
    s.add(floor, Float::STILL);
    // CENTERPIECES SCALE WITH THE CONVERSATION: a big chat earns several lecterns (focal "open tomes") spread
    // into the diagonal gaps between the bookshelf corridors, not one lonely stand in a vast hall. Each is varied
    // by a per-station seed (tome glow hue + intensity, page count) so they read as distinct stations of thought
    // distinguished by their local context — the procedural answer to "one centerpiece feels empty when big".
    let pr = env_params();
    let n_centers = (pr.nodes / 22).clamp(1, 5) as i32;
    let glow_tints = [c(0xf2, 0xe6, 0xc6), c(0xf0, 0xdc, 0xb0), c(0xe8, 0xe2, 0xd0), c(0xf4, 0xd8, 0xa8), c(0xe6, 0xd0, 0xc4)];
    let mut lectern = |s: &mut Scene, px: f32, pz: f32, scl: f32, idx: i32| {
        let yb = -3.0_f32;
        let mut stand = m3::tube(v(px, yb, pz), v(px, yb + 2.5 * scl, pz), 0.42 * scl, 16, c(0x46, 0x32, 0x1c));
        m3::tint_height(&mut stand, yb, yb + 2.5 * scl, c(0x30, 0x20, 0x12), c(0x5a, 0x42, 0x26)); stand.mat = M::WOOD; s.add(stand, Float::STILL);
        let dy = yb + 2.55 * scl;
        let mut desk = m3::disc(v(px, dy, pz), 1.5 * scl, 28, c(0x52, 0x3c, 0x22));
        m3::tint_up(&mut desk, c(0x6a, 0x4e, 0x2c), c(0x3c, 0x2c, 0x18)); desk.mat = M::WOOD; s.add(desk, Float::STILL);
        // the open tome — glowing focal point; hue + glow vary per station (its local context)
        let tint = glow_tints[(idx as usize) % glow_tints.len()];
        let mut tome = m3::paper_fold(v(px, dy + 0.45 * scl, pz), 1.3 * scl, 0.55 * scl, tint);
        tome.mat = M::PAPER; tome.emissive = 0.45 + 0.22 * hash(idx, 71);
        s.add(tome, Float { amp: 0.02, speed: 0.5, phase: hash(idx, 72) * 6.28, drift: 0.0 });
        let mut pool = m3::disc(v(px, yb + 0.02, pz), 2.4 * scl, 24, tint); // soft glow pool grounds the station as a lit focal point
        pool.mat = M::PAPER; pool.emissive = 0.10; s.add(pool, Float::STILL);
        for k in 0..(5 + (3.0 * scl) as i32) {                       // loose pages drifting up around this lectern
            let ang = hash(idx * 13 + k, 3) * std::f32::consts::TAU;
            let rad = (2.0 + hash(idx * 13 + k, 4) * 3.0) * scl;
            let p = v(px + ang.cos() * rad, -0.6 + hash(idx * 13 + k, 5) * 4.0 * scl, pz + ang.sin() * rad);
            let mut pf = m3::paper_fold(p, (0.26 + hash(idx * 13 + k, 6) * 0.16) * scl, 0.30, c(0xe8, 0xd6, 0xae));
            pf.mat = M::PAPER;
            s.add(pf, Float { amp: 0.5 + hash(idx * 13 + k, 7) * 0.5, speed: 0.6 + hash(idx * 13 + k, 8) * 0.5, phase: hash(idx * 13 + k, 9) * 6.28, drift: 0.6 });
        }
    };
    lectern(&mut s, 0.0, 0.0, 1.0, 0);                               // the main lectern at the heart
    for i in 1..n_centers {
        // satellites sit in the diagonal gaps BETWEEN the four bookshelf corridors, so they never block an aisle
        let ang = std::f32::consts::FRAC_PI_4 + (i as f32 - 1.0) * std::f32::consts::FRAC_PI_2;
        let rad = 10.5 + i as f32 * 2.4; // pushed well out into the diagonal gaps, staggered so they don't ring up
        lectern(&mut s, ang.cos() * rad, ang.sin() * rad, 0.7, i);
    }
    // RECTANGULAR BOOKSHELF HALLWAYS radiating from the lectern — wooden stacks lining each corridor, capped at
    // the end, so the room reads as aisles of a wooden library.
    let (wlo, whi) = (c(0x46, 0x32, 0x1c), c(0x6a, 0x4e, 0x2c));
    let yfl = -3.0_f32;
    let (cw, bw, bh) = (2.8_f32, 2.2_f32, 4.2_f32);
    let pr = env_params();
    let filled = ((pr.nodes as f32 / 6.0).round() as usize).clamp(3, 9); // how far the graph has filled the stacks
    let ncases = 12usize;
    for (d, (tx, tz)) in [(0usize, (1.0_f32, 0.0_f32)), (1, (-1.0, 0.0)), (2, (0.0, 1.0)), (3, (0.0, -1.0))] {
        let (px, pz) = (-tz, tx); // across-corridor axis
        // MAIN aisle: packed near the lectern, then EMPTY shelves further out (room for the graph to grow into)
        for i in 0..ncases {
            let along = 4.6 + i as f32 * (bw + 0.12);
            let fill = if i < filled { 1.0 } else { 0.0 };
            for sidx in 0..2u32 {
                let side = if sidx == 0 { 1.0 } else { -1.0 };
                let (cxp, czp) = (tx * along + px * cw * side, tz * along + pz * cw * side);
                let inward = v(-px * side, 0.0, -pz * side);
                let tangent = v(tx, 0.0, tz);
                let base = v(cxp - tx * bw * 0.5, yfl, czp - tz * bw * 0.5);
                bookcase_fill(&mut s, base, inward, tangent, bw, bh, (d * 100 + i * 10 + sidx as usize) as u32, wlo, whi, fill);
            }
        }
        // PARALLEL aisles flanking this hall — entirely EMPTY (the graph hasn't reached them yet)
        for poff in [1.0_f32, -1.0] {
            let perp = (2.0 * cw + 3.2) * poff;
            for i in 2..ncases {
                let along = 4.6 + i as f32 * (bw + 0.12);
                for sidx in 0..2u32 {
                    let side = if sidx == 0 { 1.0 } else { -1.0 };
                    let (cxp, czp) = (tx * along + px * (cw * side + perp), tz * along + pz * (cw * side + perp));
                    let inward = v(-px * side, 0.0, -pz * side);
                    let tangent = v(tx, 0.0, tz);
                    let base = v(cxp - tx * bw * 0.5, yfl, czp - tz * bw * 0.5);
                    bookcase_fill(&mut s, base, inward, tangent, bw, bh, (d * 1000 + i * 10 + sidx as usize + (poff > 0.0) as usize * 7) as u32, wlo, whi, 0.0);
                }
            }
        }
    }
    s
}

/// Atlantis: an underwater glass-and-stone hall — a wide blue-obsidian floor, a ring of stone columns
/// carrying a great glass dome with a stone oculus, a central stone plinth crowned by a glowing glass
/// crystal (the mind-palace heart), an interior gallery ring, and rising bubbles. The conversation flows
/// through the hall between the columns.
/// A randomly-posed MARBLE statue of a Poseidon-like trident wielder, built from primitives: pedestal, flared
/// robe, torso, head, two arms (one raised holding the trident), and a trident (staff + 3 prongs + crossbar).
/// Pose, robe length and which arm holds the trident vary with `seed`. A region/landmark building block.
/// A soft planar CONTACT SHADOW on the floor: a dark disc with a radial alpha falloff (opaque-ish at the
/// centre → transparent at the rim) that blends over the lit/caustic floor to ground an object. Cheap stand-in
/// for a cast shadow until the sun depth-map lands; the overhead oculus sun means shadows sit ~under the object.
fn contact_shadow(s: &mut Scene, x: f32, z: f32, r: f32, y: f32, strength: u8) {
    // Offset the blob AWAY from the sun (anti-sun ground direction) and enlarge a touch, so it reads as a cast
    // shadow with a light direction rather than a centred disc — cheap grounding short of a full shadow-map.
    let sd = m3::sun_dir_3d();
    let gl = (sd.x * sd.x + sd.z * sd.z).sqrt().max(1e-3);
    let (ox, oz) = (-sd.x / gl * r * 0.5, -sd.z / gl * r * 0.5);
    let rr = r * 1.2;
    let cen = v(x + ox, y + 0.09, z + oz); // above the emissive mosaic inlay (y+0.03..0.05) so the shadow darkens it
    let mut sh = m3::disc(cen, rr, 24, c(0x05, 0x09, 0x0b));
    sh.vcol = Some(sh.verts.iter().map(|p| {
        let d = (((p.x - cen.x).powi(2) + (p.z - cen.z).powi(2)).sqrt() / rr).clamp(0.0, 1.0);
        let a = ((1.0 - d * d) * strength as f32) as u8; // quadratic falloff → soft edge
        Color32::from_rgba_unmultiplied(4, 8, 10, a)
    }).collect());
    sh.mat = M::STONE; sh.emissive = 0.0;
    s.add(sh, Float::STILL);
}

/// A ring of columns borrowed from the Atlantis colonnade, parameterised for texture/material variation so
/// Archive (warm wood) and the Reading Room (pale stone) can start growing main halls from the same kit.
/// Each column emits a contact shadow + a collider.
fn colonnade_ring(s: &mut Scene, count: usize, radius: f32, y_floor: f32, height: f32, lo: Color32, hi: Color32, mat: f32) {
    let y_cap = y_floor + height;
    for k in 0..count {
        let a = (k as f32 / count as f32) * std::f32::consts::TAU + 0.2;
        let (cx, cz) = (a.cos() * radius, a.sin() * radius);
        let mut shaft = m3::tube(v(cx, y_floor, cz), v(cx, y_cap - 0.3, cz), 0.44, 22, lo);
        m3::tint_height(&mut shaft, y_floor, y_cap, lo, hi); shaft.mat = mat; s.add(shaft, Float::STILL);
        let mut base = m3::tube(v(cx, y_floor, cz), v(cx, y_floor + 0.4, cz), 0.62, 18, hi); base.mat = mat; s.add(base, Float::STILL);
        let mut base_top = m3::disc(v(cx, y_floor + 0.4, cz), 0.62, 18, hi); base_top.mat = mat; s.add(base_top, Float::STILL);
        let mut cap = m3::tube(v(cx, y_cap - 0.4, cz), v(cx, y_cap, cz), 0.62, 18, hi); cap.mat = mat; s.add(cap, Float::STILL);
        let mut cap_top = m3::disc(v(cx, y_cap, cz), 0.62, 18, hi); cap_top.mat = mat; s.add(cap_top, Float::STILL);
        let mut collar = m3::torus(v(cx, y_cap - 0.4, cz), 0.52, 0.1, 18, 7, hi); collar.mat = mat; s.add(collar, Float::STILL);
        contact_shadow(s, cx, cz, 1.05, y_floor, 120);
        push_collider(Collider::Pillar { x: cx, z: cz, r: 0.66, y0: y_floor, y1: y_cap });
    }
}

/// A WATERFALL sheet: a tall thin curved slab on the WATER material falling from `top` to `base_y`, given a
/// downward flow by the water shader. A first water-feature for Mistfall (pairs with the lake disc).
fn waterfall(s: &mut Scene, x: f32, z: f32, top_y: f32, base_y: f32, width: f32, color: Color32) {
    use std::f32::consts::TAU;
    let h = (top_y - base_y).max(0.5);
    // TURBULENT sheet: three overlapping translucent strands that sway out of phase (reads as churning water)
    for i in 0..3 {
        let off = (i as f32 - 1.0) * width * 0.26;
        let mut sheet = m3::panel(v(x + off * 0.5, (top_y + base_y) * 0.5, z + off * 0.18), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0), width * 0.6, h, color);
        m3::tint_height(&mut sheet, base_y, top_y, c(0x2c, 0x52, 0x5e), c(0xbf, 0xe6, 0xf0)); // foam-bright at the lip
        sheet.mat = M::WATER; sheet.emissive = 0.07; m3::set_alpha(&mut sheet, 128);
        s.add(sheet, Float { amp: 0.0, speed: 0.9, phase: i as f32 * 1.3, drift: 0.12 });
    }
    // BUBBLES / foam gobs tumbling down the fall
    for k in 0..14 {
        let py = base_y + hash((x * 7.0 + z * 3.0) as i32 + k, 1) * h;
        let px = x + (hash(k, 2) - 0.5) * width * 0.85;
        let mut bub = m3::uv_sphere(v(px, py, z + (hash(k, 3) - 0.5) * 0.3), 0.05 + hash(k, 4) * 0.09, 7, 6, c(0xd8, 0xee, 0xf4));
        bub.mat = M::WATER; bub.emissive = 0.10;
        s.add(bub, Float { amp: 0.6, speed: 1.4 + hash(k, 5), phase: hash(k, 6) * 6.28, drift: 0.2 });
    }
    // plunge-pool foam ring where it lands
    let mut foam = m3::annulus(v(x, base_y + 0.04, z), width * 0.3, width * 0.8, 28, c(0xcf, 0xe8, 0xf0));
    foam.mat = M::WATER; foam.emissive = 0.14; m3::set_alpha(&mut foam, 120); s.add(foam, Float::STILL);
    // MISTING — lots of translucent puffs billowing up off the plunge pool (taller, denser)
    for k in 0..18 {
        let a = hash(k, 7) * TAU; let r = hash(k, 8) * width * 0.95;
        let mut mist = m3::uv_sphere(v(x + a.cos() * r, base_y + 0.2 + hash(k, 9) * 2.4, z + a.sin() * r), 0.42 + hash(k, 10) * 0.6, 8, 6, c(0xc4, 0xda, 0xe2));
        mist.mat = M::STONE; mist.emissive = 0.06; m3::set_alpha(&mut mist, 48);
        s.add(mist, Float { amp: 0.45, speed: 0.4, phase: hash(k, 11) * 6.28, drift: 0.6 });
    }
}

/// A standing megalith — a rough stone squeezed thin so it reads as an upright monolith/ruin, with MOSS creeping
/// over its crown + a couple of side clumps (graph-varied). For Mistfall.
fn standing_stone(s: &mut Scene, x: f32, z: f32, yb: f32, h: f32, seed: u32) {
    let mut stone = m3::rock(v(x, yb + h * 0.5, z), h * 0.5, seed, c(0x4e, 0x58, 0x5c));
    m3::tint_up(&mut stone, c(0x72, 0x80, 0x80), c(0x30, 0x3a, 0x3e));
    m3::scale_xz(&mut stone, 0.40, 0.34); stone.mat = M::STONE;
    s.add(stone, Float::STILL);
    // MOSS hugging the stone — low rounded mounds on the crown + flanks (flattened uv-spheres, attached; NOT leaves)
    let mut cap = m3::uv_sphere(v(x, yb + h - 0.08, z), 0.26 + h * 0.05, 10, 7, c(0x44, 0x5e, 0x2a));
    m3::scale3(&mut cap, 1.0, 0.38, 0.85); m3::tint_up(&mut cap, c(0x56, 0x72, 0x30), c(0x2a, 0x40, 0x1a)); cap.mat = M::MOSS;
    s.add(cap, Float::STILL);
    for mi in 0..3 {
        if hash(seed as i32, 20 + mi) < 0.4 { continue; }
        let a = hash(seed as i32, 30 + mi) * 6.28; let my = yb + h * (0.28 + 0.46 * hash(seed as i32, 36 + mi));
        let mut clump = m3::uv_sphere(v(x + a.cos() * 0.17, my, z + a.sin() * 0.14), 0.14 + hash(seed as i32, 38 + mi) * 0.08, 8, 6, c(0x3c, 0x54, 0x24));
        m3::scale3(&mut clump, 1.0, 0.55, 0.82); m3::tint_up(&mut clump, c(0x50, 0x6c, 0x2c), c(0x24, 0x3a, 0x16)); clump.mat = M::MOSS;
        s.add(clump, Float::STILL);
    }
    contact_shadow(s, x, z, h * 0.28, yb, 120);
}

/// A pointed STALACTITE cone hanging from the cave roof: an n-gon top tapering to a sharp apex `len` below.
fn stalactite(s: &mut Scene, x: f32, z: f32, top: f32, len: f32, wdt: f32, lo: Color32, hi: Color32) {
    use std::f32::consts::TAU;
    let n = 8usize;
    let (mut verts, mut norms, mut tris) = (Vec::new(), Vec::new(), Vec::new());
    verts.push(v(x, top - len, z)); norms.push(v(0.0, -1.0, 0.0)); // 0 = apex
    for i in 0..n {
        let a = TAU * i as f32 / n as f32;
        verts.push(v(x + a.cos() * wdt, top, z + a.sin() * wdt));
        norms.push(normv(v(a.cos(), 0.35, a.sin())));
    }
    let ct = verts.len() as u32; verts.push(v(x, top, z)); norms.push(v(0.0, 1.0, 0.0)); // top centre
    for i in 0..n as u32 {
        let i0 = 1 + i; let i1 = 1 + (i + 1) % n as u32;
        tris.push([0, i1, i0]);   // taper side
        tris.push([ct, i0, i1]);  // top cap
    }
    let mut m = Mesh3 { verts, norms, tris, color: lo, vcol: None, offset: v(0.0, 0.0, 0.0), mat: 0.0, emissive: 0.0 };
    m3::tint_height(&mut m, top - len, top, lo, hi); m.mat = M::STONE; s.add(m, Float::STILL);
}

/// A reading nook: a pedestal table with an open glowing tome + a small book stack. For the Reading Room.
fn reading_desk(s: &mut Scene, x: f32, z: f32, yf: f32) {
    let mut ped = m3::tube(v(x, yf, z), v(x, yf + 0.84, z), 0.18, 10, c(0x4a, 0x36, 0x20)); ped.mat = M::WOOD; s.add(ped, Float::STILL);
    let mut top = m3::disc(v(x, yf + 0.9, z), 0.85, 22, c(0x6a, 0x50, 0x30)); top.mat = M::WOOD; s.add(top, Float::STILL);
    let mut rim = m3::tube(v(x, yf + 0.82, z), v(x, yf + 0.9, z), 0.85, 22, c(0x52, 0x3e, 0x24)); rim.mat = M::WOOD; s.add(rim, Float::STILL);
    let mut tome = m3::paper_fold(v(x, yf + 1.0, z), 0.62, 0.22, c(0xf4, 0xe8, 0xc8)); tome.mat = M::PAPER; tome.emissive = 0.28; s.add(tome, Float::STILL);
    let cols = [c(0xe8, 0xd6, 0xae), c(0xd6, 0xc2, 0x96), c(0xc6, 0xae, 0x82)];
    for j in 0..3 { let mut bk = m3::paper_fold(v(x + 0.42, yf + 0.93 + j as f32 * 0.07, z + 0.22), 0.32, 0.07, cols[j]); bk.mat = M::PAPER; s.add(bk, Float::STILL); }
    contact_shadow(s, x, z, 1.0, yf, 110);
}

/// A grass tuft: a fan of thin blades. Cheap ground-cover foliage for Canopy/Forest/garden scenes.
fn grass_tuft(s: &mut Scene, x: f32, z: f32, yb: f32, seed: u32, col: Color32) {
    let sd = seed as i32;
    let n = 5 + (hash(sd, 1) * 4.0) as usize;
    for j in 0..n {
        let ji = sd.wrapping_add((j as i32) * 7);
        let a = hash(ji, 2) * std::f32::consts::TAU;
        let lean = 0.10 + hash(ji, 3) * 0.22;
        let h = 0.28 + hash(ji, 4) * 0.4;
        let mut blade = m3::tube(v(x, yb, z), v(x + a.cos() * lean, yb + h, z + a.sin() * lean), 0.02, 4, col);
        m3::tint_height(&mut blade, yb, yb + h, scale_rgb(col, 0.7), scale_rgb(col, 1.25)); blade.mat = M::MOSS;
        s.add(blade, Float { amp: 0.04, speed: 0.8, phase: hash(ji, 5) * 6.28, drift: 0.03 });
    }
}

/// A wildflower: a thin stem + a ring of petals around a glowing center. Spot colour for meadows/understory.
fn flower(s: &mut Scene, x: f32, z: f32, yb: f32, seed: u32, petal: Color32) {
    let sd = seed as i32;
    let h = 0.4 + hash(sd, 1) * 0.35;
    let mut stem = m3::tube(v(x, yb, z), v(x, yb + h, z), 0.016, 4, c(0x4a, 0x6e, 0x2e)); stem.mat = M::MOSS; s.add(stem, Float { amp: 0.05, speed: 0.7, phase: hash(sd, 2) * 6.28, drift: 0.03 });
    for j in 0..5 {
        let a = std::f32::consts::TAU * j as f32 / 5.0;
        let mut pet = m3::uv_sphere(v(x + a.cos() * 0.08, yb + h, z + a.sin() * 0.08), 0.06, 8, 6, petal); pet.mat = M::MOSS; pet.emissive = 0.10; s.add(pet, Float::STILL);
    }
    let mut ctr = m3::uv_sphere(v(x, yb + h, z), 0.05, 8, 6, c(0xff, 0xd2, 0x44)); ctr.mat = M::MOSS; ctr.emissive = 0.22; s.add(ctr, Float::STILL);
}

/// A bird in flight: two angled wing-quads as a dark silhouette, drifting + flapping (Float). Ambient life.
fn bird(s: &mut Scene, p: Vec3, seed: u32) {
    for side in [-1.0_f32, 1.0] {
        let mut wing = m3::panel(v(p.x + side * 0.2, p.y, p.z), v(side, 0.4, 0.0), v(0.0, 0.0, 1.0), 0.42, 0.16, c(0x24, 0x28, 0x2e));
        wing.mat = M::STONE; s.add(wing, Float { amp: 0.35, speed: 1.8, phase: hash(seed as i32, 1) * 6.28, drift: 0.9 });
    }
}

/// A bee: a tiny amber body with a faint glow, hovering fast + erratic (Float). Drift near flowers. Ambient life.
fn bee(s: &mut Scene, p: Vec3, seed: u32) {
    let mut body = m3::uv_sphere(p, 0.07, 8, 6, c(0xf0, 0xc0, 0x30)); body.mat = M::MOSS; body.emissive = 0.35;
    s.add(body, Float { amp: 0.45, speed: 2.6 + hash(seed as i32, 1) * 1.4, phase: hash(seed as i32, 2) * 6.28, drift: 1.2 });
}

/// A firefly: a tiny bioluminescent mote (warm-green, varied) with a strong glow that TWINKLES in the shader
/// (emissive > 1.6 triggers the per-mote flicker). Drifts slowly through the dusk canopy.
fn firefly(s: &mut Scene, p: Vec3, seed: u32) {
    let col = [c(0xc6, 0xf0, 0x6e), c(0xe2, 0xf0, 0x72), c(0x9e, 0xe8, 0x84), c(0xf0, 0xe2, 0x70)][(seed as usize) % 4];
    let mut f = m3::uv_sphere(p, 0.045 + hash(seed as i32, 5) * 0.03, 6, 5, col); f.mat = M::MOSS; f.emissive = 1.9;
    s.add(f, Float { amp: 0.9, speed: 0.45 + hash(seed as i32, 1) * 0.5, phase: hash(seed as i32, 2) * 6.28, drift: 1.15 });
}

/// A canopy treehouse platform: a round wooden deck with a railing, four posts and a little hut roof with a
/// warm lantern glow underneath. The "rooms up in the trees" of the arboretum canopy walk.
fn tree_platform(s: &mut Scene, cx: f32, cz: f32, y: f32, r: f32, seed: u32) {
    use std::f32::consts::TAU;
    let mut deck = m3::disc(v(cx, y, cz), r, 24, c(0x5a, 0x42, 0x28)); deck.mat = M::WOOD; s.add(deck, Float::STILL);
    let mut under = m3::tube(v(cx, y - 0.2, cz), v(cx, y, cz), r, 24, c(0x3a, 0x2a, 0x18)); under.mat = M::WOOD; s.add(under, Float::STILL);
    for k in 0..12 { let a = TAU * k as f32 / 12.0; let (px, pz) = (cx + a.cos() * (r - 0.12), cz + a.sin() * (r - 0.12)); let mut post = m3::tube(v(px, y, pz), v(px, y + 0.72, pz), 0.05, 5, c(0x4a, 0x36, 0x20)); post.mat = M::WOOD; s.add(post, Float::STILL); }
    let mut rail = m3::torus(v(cx, y + 0.72, cz), r - 0.12, 0.05, 32, 6, c(0x60, 0x46, 0x2a)); rail.mat = M::WOOD; s.add(rail, Float::STILL);
    for k in 0..4 { let a = TAU * k as f32 / 4.0 + 0.4; let (px, pz) = (cx + a.cos() * (r * 0.78), cz + a.sin() * (r * 0.78)); let mut sp = m3::tube(v(px, y + 0.72, pz), v(px, y + 2.5, pz), 0.06, 5, c(0x4a, 0x36, 0x20)); sp.mat = M::WOOD; s.add(sp, Float::STILL); }
    let mut roof = m3::dome_cap(v(cx, y + 2.5, cz), r * 0.95, 1.1, 0.0, 20, 6, c(0x46, 0x32, 0x1e)); roof.mat = M::WOOD; s.add(roof, Float::STILL);
    let mut lantern = m3::uv_sphere(v(cx, y + 1.9, cz), 0.17, 10, 8, c(0xff, 0xc8, 0x6a)); lantern.mat = M::MOSS; lantern.emissive = 1.35;
    s.add(lantern, Float { amp: 0.03, speed: 0.6, phase: hash(seed as i32, 3) * 6.28, drift: 0.0 });
    contact_shadow(s, cx, cz, r * 0.9, y - 0.2, 70);
}

/// A swinging rope bridge between two points: two sagging side ropes (catenary) + plank rungs across, the
/// whole span gently swaying together (shared Float). Connects the treehouse platforms.
fn rope_bridge(s: &mut Scene, a: Vec3, b: Vec3, seed: u32) {
    let n = 14;
    let (dx, dy, dz) = (b.x - a.x, b.y - a.y, b.z - a.z);
    let hlen = (dx * dx + dz * dz).sqrt().max(1e-3);
    let (ndx, ndz) = (dx / hlen, dz / hlen);
    let (sx, sz) = (-ndz, ndx); // horizontal perpendicular = bridge half-width axis
    let (halfw, sag) = (0.5_f32, 1.3_f32);
    let (rope_col, plank_col) = (c(0x36, 0x28, 0x18), c(0x5a, 0x42, 0x28));
    let sway = Float { amp: 0.12, speed: 0.5, phase: hash(seed as i32, 1) * 6.28, drift: 0.6 }; // the whole span swings (more pronounced)
    let (mut plx, mut ply, mut plz) = (0.0, 0.0, 0.0);
    let (mut prx, mut pry, mut prz) = (0.0, 0.0, 0.0);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let droop = sag * 4.0 * t * (1.0 - t);
        let (bx, by, bz) = (a.x + dx * t, a.y + dy * t - droop, a.z + dz * t);
        let (lx, ly, lz) = (bx + sx * halfw, by, bz + sz * halfw);
        let (rx, ry, rz) = (bx - sx * halfw, by, bz - sz * halfw);
        if i > 0 {
            let mut r1 = m3::tube(v(plx, ply, plz), v(lx, ly, lz), 0.035, 5, rope_col); r1.mat = M::WOOD; s.add(r1, sway);
            let mut r2 = m3::tube(v(prx, pry, prz), v(rx, ry, rz), 0.035, 5, rope_col); r2.mat = M::WOOD; s.add(r2, sway);
        }
        if i < n {
            // procedural HOLES: ~18% of planks are missing (a rickety, weathered walkway)
            if hash((seed as i32).wrapping_add(i as i32 * 7), 3) > 0.18 {
                let pw = halfw * 2.0 * (0.82 + hash((seed as i32) + i as i32, 9) * 0.18);
                let mut plank = m3::panel(v(bx, by, bz), v(sx, 0.0, sz), v(ndx, 0.0, ndz), pw, 0.32, plank_col); plank.mat = M::WOOD; s.add(plank, sway);
                // underside batten → the plank reads as THICK from the side
                let mut bat = m3::tube(v(bx - sx * halfw * 0.82, by - 0.05, bz - sz * halfw * 0.82), v(bx + sx * halfw * 0.82, by - 0.05, bz + sz * halfw * 0.82), 0.05, 4, c(0x46, 0x32, 0x1e));
                bat.mat = M::WOOD; s.add(bat, sway);
            }
        }
        plx = lx; ply = ly; plz = lz; prx = rx; pry = ry; prz = rz;
    }
}

/// A procedural SEA-DRAGON head (STONE) for the Atlantis "dragon forge": cranium + 3-seg tapering snout +
/// dropped lower jaw + a row of teeth + brow ridges + glowing eyes + two swept-back horns + a midline frill of
/// crest spines + cheek gill-fins. Built ALONG the `face` vector (no axis-scaling) so it reads correctly at any
/// yaw — used both upright on the poseidon statues and tilted outward as cornice gargoyles.
///
/// Returns the meshes as a `Vec` rather than pushing into `&mut Scene`: `poseidon_statue` holds a `put` closure
/// that mutably borrows the scene across the head position, so the caller adds these AFTER that borrow ends.
/// `pos` = head centre (where the human head ball sat, ~0.26·sc radius); `sc` = statue scale; `col`/`col_lo` =
/// the statue's stone tint (high/low); `seed` = per-instance variation (eye hue, slight asymmetry).
fn dragon_head(pos: Vec3, face: f32, sc: f32, seed: u32, col: Color32, col_lo: Color32) -> Vec<(Mesh3, Float)> {
    let (fc, fs) = (face.cos(), face.sin());
    let fwd = v(fc, 0.0, fs);          // snout direction (forward)
    let side = v(-fs, 0.0, fc);        // dragon's right
    let up = v(0.0, 1.0, 0.0);
    let rnd = |k: u32| { let mut h = seed.wrapping_mul(0x9E37_79B1).wrapping_add(k.wrapping_mul(0x85EB_CA77)); h ^= h >> 15; h = h.wrapping_mul(0x2C1B_3C6D); h ^= h >> 13; (h & 0xFFFF) as f32 / 65535.0 };
    let r = 0.26 * sc;                 // base unit = the head radius the human skull used
    // head-local point: f forward (snout), s sideways (+ = right), u up, all in units of r
    let p = |f: f32, s: f32, u: f32| addv(pos, addv(addv(mulv(fwd, f * r), mulv(side, s * r)), mulv(up, u * r)));
    let ivory = c(0xe4, 0xe0, 0xd2);   // teeth
    // eye glow: cool cyan, drifting toward amber for some seeds, so a row of gargoyles isn't uniform
    let eye = { let t = rnd(7); c((0x6a as f32 + t * 0x8a as f32) as u8, 0xf2, (0xe8 as f32 - t * 0x60 as f32) as u8) };

    let mut out: Vec<(Mesh3, Float)> = Vec::new();
    // value-returning builders (capture nothing of `out`, so no borrow tangles)
    let orb = |f: f32, s: f32, u: f32, rad: f32, color: Color32| m3::uv_sphere(p(f, s, u), rad * r, 12, 9, color);
    let seg = |a: (f32, f32, f32), b: (f32, f32, f32), rad: f32, color: Color32|
        m3::tube(p(a.0, a.1, a.2), p(b.0, b.1, b.2), rad * r, 10, color);
    let stone = |mut m: Mesh3| -> Mesh3 { m.mat = M::STONE; m };

    // CRANIUM — an ellipsoid skull (built as an overlapping orb pair so it reads longer than wide without
    // world-axis scaling). Height-tinted so the crown catches light and the jaw stays in shadow.
    {
        let mut sk = orb(0.15, 0.0, 0.05, 0.92, col);
        m3::tint_height(&mut sk, pos.y - 0.9 * r, pos.y + 0.9 * r, col_lo, col);
        out.push((stone(sk), Float::STILL));
        out.push((stone(orb(0.55, 0.0, -0.05, 0.66, col)), Float::STILL)); // muzzle root, blends into snout
    }

    // SNOUT — three tapering segments from the muzzle root forward, drooping slightly (a sea-dragon muzzle).
    let snout = [(0.85, 0.00, 0.62), (1.45, -0.05, 0.50), (2.05, -0.13, 0.38), (2.55, -0.24, 0.26)];
    for w in snout.windows(2) {
        let (a, b) = (w[0], w[1]);
        out.push((stone(seg((a.0, 0.0, a.1), (b.0, 0.0, b.1), (a.2 + b.2) * 0.5, col)), Float::STILL));
    }
    for &(f, u, rad) in &snout { out.push((stone(orb(f, 0.0, u, rad, col)), Float::STILL)); }
    out.push((stone(orb(2.6, 0.0, -0.26, 0.22, col_lo)), Float::STILL)); // nose tip
    for s in [-1.0f32, 1.0] { out.push((stone(orb(2.5, s * 0.16, -0.18, 0.07, col_lo)), Float::STILL)); } // nostrils

    // LOWER JAW — a thinner bar set below, slightly open (the mouth gaps between this and the snout underside).
    let jaw = [(0.70, -0.42, 0.34), (1.45, -0.40, 0.28), (2.10, -0.42, 0.20), (2.55, -0.40, 0.13)];
    for w in jaw.windows(2) {
        let (a, b) = (w[0], w[1]);
        out.push((stone(seg((a.0, 0.0, a.1), (b.0, 0.0, b.1), (a.2 + b.2) * 0.5, col_lo)), Float::STILL));
    }
    out.push((stone(orb(0.62, 0.0, -0.40, 0.34, col_lo)), Float::STILL)); // jaw hinge

    // TEETH — pegs along the upper line (pointing down) and the lower jaw (pointing up).
    for i in 0..5 {
        let f = 1.0 + i as f32 * 0.38;
        for s in [-1.0f32, 1.0] {
            out.push((stone(seg((f, 0.30 * s, -0.05), (f, 0.30 * s, -0.34), 0.045, ivory)), Float::STILL)); // upper
            if i < 4 {
                let fl = 1.05 + i as f32 * 0.36;
                out.push((stone(seg((fl, 0.24 * s, -0.40), (fl, 0.24 * s, -0.14), 0.04, ivory)), Float::STILL)); // lower
            }
        }
    }

    // BROW ridges + glowing EYES, set into the front of the skull.
    for s in [-1.0f32, 1.0] {
        out.push((stone(orb(0.55, s * 0.55, 0.42, 0.26, col)), Float::STILL));         // brow ridge
        out.push((stone(orb(0.62, s * 0.5, 0.34, 0.18, col_lo)), Float::STILL));        // socket rim
        let mut e = orb(0.7, s * 0.5, 0.30, 0.14, eye); e.emissive = 1.8;               // glowing eye
        out.push((e, Float::STILL));
    }

    // HORNS — two thick stone horns sweeping up-and-back off the crown, tapering to a tip.
    let horn = [(-0.05, 0.50, 0.75, 0.13), (-0.65, 0.66, 1.12, 0.105), (-1.35, 0.72, 1.05, 0.075), (-1.92, 0.74, 0.66, 0.045)];
    for s in [-1.0f32, 1.0] {
        for w in horn.windows(2) {
            let (a, b) = (w[0], w[1]);
            out.push((stone(seg((a.0, a.1 * s, a.2), (b.0, b.1 * s, b.2), (a.3 + b.3) * 0.5, col)), Float::STILL));
        }
        for &(f, sd, u, rad) in &horn { out.push((stone(orb(f, sd * s, u, rad, col)), Float::STILL)); }
    }

    // FRILL — a midline row of crest spines from crown back toward the neck, shrinking rearward.
    for i in 0..6 {
        let t = i as f32 / 5.0;
        let f = 0.25 - t * 1.25;
        let h = 1.05 - t * 0.35;       // spine base height on the crown
        let len = 0.55 - t * 0.18;     // tip rises up-and-back
        let base = (f, 0.0, h);
        let tip = (f - 0.22, 0.0, h + len);
        out.push((stone(seg(base, tip, 0.05, mix_rgb(col, col_lo, 0.4))), Float::STILL));
    }

    // CHEEK GILL-FINS — a fan of swept stone fins on each cheek, behind the jaw hinge.
    for s in [-1.0f32, 1.0] {
        for j in 0..3 {
            let spread = (j as f32 - 1.0) * 0.35;
            let base = (0.05, s * 0.78, -0.05 + spread * 0.4);
            let tip = (-0.85, s * 1.05, 0.05 + spread);
            out.push((stone(seg(base, tip, 0.04, mix_rgb(col, col_lo, 0.5))), Float::STILL));
        }
    }

    out
}

fn poseidon_statue(s: &mut Scene, base: Vec3, face: f32, sc: f32, seed: u32) {
    use std::f32::consts::TAU;
    let marble = c(0xe6, 0xe2, 0xd8); let marble_lo = c(0x98, 0x94, 0x8c);
    let rnd = |k: u32| { let mut h = seed.wrapping_mul(0x9E37_79B1).wrapping_add(k.wrapping_mul(0x85EB_CA77)); h ^= h >> 15; h = h.wrapping_mul(0x2C1B_3C6D); h ^= h >> 13; (h & 0xFFFF) as f32 / 65535.0 };
    let (fc, fs) = (face.cos(), face.sin());
    let fwd = v(fc, 0.0, fs); let side = v(-fs, 0.0, fc);
    let at = |u: f32, so: f32, fo: f32| v(base.x + side.x * so + fwd.x * fo, base.y + u, base.z + side.z * so + fwd.z * fo);
    let mut put = |mut m: m3::Mesh3| { m.mat = M::STONE; s.add(m, Float::STILL); };
    let tube = |s: f32, a: Vec3, b: Vec3, r: f32| { let mut m = m3::tube(a, b, r * s, 12, marble); m3::tint_height(&mut m, a.y, b.y.max(a.y + 0.01), marble_lo, marble); m };
    let ball = |s: f32, p: Vec3, r: f32| m3::uv_sphere(p, r * s, 14, 10, marble);

    // pedestal
    let ped = 0.5 * sc;
    put({ let mut m = m3::tube(at(0.0, 0.0, 0.0), at(ped, 0.0, 0.0), 0.95 * sc, 24, marble); m3::tint_height(&mut m, base.y, base.y + ped, marble_lo, marble); m });
    put(m3::disc(at(ped, 0.0, 0.0), 0.95 * sc, 24, marble));

    let arm_side = if rnd(9) > 0.5 { 1.0f32 } else { -1.0 }; // which arm holds the trident
    let stance = if rnd(5) > 0.5 { 1.0f32 } else { -1.0 };   // contrapposto: which leg is planted/forward

    // LEGS — a planted stance: two tapered legs (thigh→calf), knees, feet. One leg eased forward (contrapposto).
    let leg_h = 1.5 * sc; let hip_y = ped + leg_h;
    for lside in [-1.0f32, 1.0] {
        let fwd_off = if lside == stance { 0.18 * sc } else { -0.06 * sc };
        let foot = at(ped, lside * 0.26 * sc, fwd_off);
        let knee = at(ped + leg_h * 0.5, lside * 0.24 * sc, fwd_off * 0.6);
        let hip  = at(hip_y, lside * 0.2 * sc, 0.0);
        put(tube(sc, foot, knee, 0.21));            // calf+thigh lower
        put(ball(sc, knee, 0.17));                  // knee
        put(tube(sc, knee, hip, 0.24));             // thigh upper (heavier)
        put(ball(sc, foot, 0.16));                  // foot
    }
    put(ball(sc, at(hip_y, 0.0, 0.0), 0.34));       // pelvis joining the legs

    // TORSO — V-taper: heavy chest narrowing to waist, with pec + ab definition.
    let waist_y = hip_y + 0.15 * sc; let chest_y = waist_y + 0.95 * sc;
    put(tube(sc, at(waist_y, 0.0, 0.0), at(waist_y + 0.45 * sc, 0.0, 0.0), 0.34)); // waist (narrow)
    put(tube(sc, at(waist_y + 0.4 * sc, 0.0, 0.0), at(chest_y, 0.0, 0.0), 0.5));    // chest (broad)
    put(ball(sc, at(waist_y + 0.55 * sc, 0.0, 0.16 * sc), 0.22)); // abdomen swell (front)
    for psd in [-1.0f32, 1.0] { put(ball(sc, at(chest_y - 0.18 * sc, psd * 0.22 * sc, 0.22 * sc), 0.19)); } // pectorals

    // SHOULDERS — broad deltoids cap the chest.
    let y_sh = chest_y + 0.05 * sc;
    for dsd in [-1.0f32, 1.0] { put(ball(sc, at(y_sh, dsd * 0.5 * sc, 0.0), 0.26)); } // deltoids → broad shoulders

    // NECK + HEAD + BEARD
    put(tube(sc, at(y_sh - 0.05 * sc, 0.0, 0.04 * sc), at(y_sh + 0.28 * sc, 0.0, 0.04 * sc), 0.14)); // neck
    let head = at(y_sh + 0.6 * sc, 0.0, 0.06 * sc);
    // SEA-DRAGON head in place of the human head+beard (Atlantis "dragon forge"). Captured now, added at the
    // end of the fn — the `put` closure mutably borrows `s` through the arm/trident code below.
    let dragon = dragon_head(head, face, sc, seed, marble, marble_lo);

    // ARMS — two segments (upper arm + forearm) with a bicep bulge; one raised to grip the trident.
    let mut arm_hand = at(y_sh, arm_side * 0.5 * sc, 0.0); // overwritten by the raised arm below
    for (sd, raised) in [(-arm_side, false), (arm_side, true)] {
        let sh = at(y_sh, sd * 0.5 * sc, 0.0);
        let (elbow, hand) = if raised {
            (at(y_sh + 0.4 * sc, sd * 0.62 * sc, 0.1 * sc), at(y_sh + raise_h(rnd(2), sc), sd * 0.5 * sc, 0.28 * sc))
        } else {
            (at(y_sh - 0.55 * sc, sd * 0.6 * sc, 0.06 * sc), at(y_sh - 1.05 * sc, sd * 0.46 * sc, 0.2 * sc))
        };
        put(tube(sc, sh, elbow, 0.17));                                                    // upper arm
        put(ball(sc, v((sh.x+elbow.x)*0.5, (sh.y+elbow.y)*0.5, (sh.z+elbow.z)*0.5), 0.16)); // bicep bulge
        put(ball(sc, elbow, 0.13));                                                        // elbow
        put(tube(sc, elbow, hand, 0.13));                                                  // forearm
        put(ball(sc, hand, 0.12));                                                         // fist
        if raised { arm_hand = hand; }
    }
    let hand = arm_hand;

    // TRIDENT in the raised fist.
    let staff_b = v(hand.x, base.y + ped, hand.z);
    let staff_t = v(hand.x, hand.y + 1.8 * sc, hand.z);
    put(tube(sc, staff_b, staff_t, 0.05));
    for p in -1i32..=1 {
        let off = p as f32 * 0.16 * sc;
        let pb = v(staff_t.x + side.x * off, staff_t.y, staff_t.z + side.z * off);
        put(tube(sc, pb, v(pb.x, pb.y + 0.5 * sc, pb.z), 0.035));
    }
    put(tube(sc, v(staff_t.x - side.x * 0.18 * sc, staff_t.y, staff_t.z - side.z * 0.18 * sc),
                 v(staff_t.x + side.x * 0.18 * sc, staff_t.y, staff_t.z + side.z * 0.18 * sc), 0.035));
    for (m, f) in dragon { s.add(m, f); }   // dragon head — added once `put`'s borrow of `s` has ended
    let _ = TAU;
}
/// raised-hand height for the trident arm (kept as a free fn so the arm closure can call it).
#[inline] fn raise_h(r: f32, sc: f32) -> f32 { (0.95 + 0.5 * r) * sc }

/// A BOOKCASE lining the library wall: a wood case with horizontal shelves packed with procedurally-coloured
/// book spines (varied width/height/lean), facing `inward`. Built along `tangent` against the wall at `base`.
/// This is the library identity — what fills the walls so any interior view reads as a flooded archive.
fn bookcase(s: &mut Scene, base: Vec3, inward: Vec3, tangent: Vec3, width: f32, height: f32, seed: u32, wood_lo: Color32, wood_hi: Color32) {
    bookcase_fill(s, base, inward, tangent, width, height, seed, wood_lo, wood_hi, 1.0);
}
/// `fill` in 0..1 controls how packed the shelves are: 1.0 = full, 0.0 = bare wooden case (empty, "graph not yet grown here").
fn bookcase_fill(s: &mut Scene, base: Vec3, inward: Vec3, tangent: Vec3, width: f32, height: f32, seed: u32, wood_lo: Color32, wood_hi: Color32, fill: f32) {
    let up = v(0.0, 1.0, 0.0);
    let inw = normv(inward); let tan = normv(tangent);
    let depth = 0.55_f32;
    let rnd = |k: u32| { let mut h = seed.wrapping_mul(0x27D4_EB2F).wrapping_add(k.wrapping_mul(0x9E37_79B1)); h ^= h >> 15; h = h.wrapping_mul(0x85EB_CA77); h ^= h >> 13; (h & 0xFFFF) as f32 / 65535.0 };
    let books = [c(0x9e, 0x2e, 0x22), c(0x9a, 0x6a, 0x1e), c(0x2a, 0x6a, 0x3a), c(0x24, 0x4a, 0x8e),
                 c(0xae, 0x8a, 0x2e), c(0x7a, 0x2e, 0x56), c(0x2e, 0x7a, 0x82), c(0x6a, 0x5e, 0x2e),
                 c(0xae, 0x50, 0x28), c(0x2e, 0x6a, 0x7a)]; // richer jewel tones — saturation reads as hue variety
    let at = |t: f32, y: f32, d: f32| addv(addv(base, mulv(tan, t)), addv(mulv(up, y), mulv(inw, d)));
    // back panel + side posts + top
    let mut back = m3::panel(at(0.0, height * 0.5, 0.04), tan, up, width, height, wood_lo);
    m3::tint_up(&mut back, wood_hi, wood_lo); back.mat = M::WOOD; s.add(back, Float::STILL);
    for sgn in [-1.0f32, 1.0] {
        let mut post = m3::tube(at(sgn * width * 0.5, 0.0, depth * 0.5), at(sgn * width * 0.5, height, depth * 0.5), 0.09, 8, wood_hi);
        m3::tint_height(&mut post, base.y, base.y + height, wood_lo, wood_hi); post.mat = M::WOOD; s.add(post, Float::STILL);
    }
    let nsh = (height / 1.3).round().max(3.0) as usize;
    for j in 0..=nsh {
        let yy = height * j as f32 / nsh as f32;
        let mut board = m3::panel(at(0.0, yy, depth * 0.5), tan, inw, width, depth, wood_hi);
        board.mat = M::WOOD; s.add(board, Float::STILL);
        if j == nsh { break; }
        let gap = height / nsh as f32;
        let mut x = -width * 0.5 + 0.12;
        let mut bk = 0u32;
        while x < width * 0.5 - 0.18 {
            let bw = 0.10 + rnd(j as u32 * 200 + bk) * 0.16;
            let bh = gap * (0.6 + rnd(j as u32 * 200 + bk + 71) * 0.32);
            let col = books[(rnd(j as u32 * 200 + bk + 131) * books.len() as f32) as usize % books.len()];
            if rnd(j as u32 * 200 + bk + 311) <= fill {
                let mut spine = m3::panel(at(x + bw * 0.5, yy + bh * 0.5 + 0.06, depth * 0.74), tan, up, bw * 0.92, bh, col);
                spine.mat = M::STONE; spine.emissive = 0.05; s.add(spine, Float::STILL);
            }
            x += bw + 0.012;
            bk += 1;
        }
    }
}

/// A READING NOOK: a round pedestal table with a warm lamp (a bloom source casting a faint floor pool), a
/// chair, and a small stack of books — a pocket of warmth in the moat between colonnade and shelves.
fn reading_nook(s: &mut Scene, pos: Vec3, facing: Vec3, seed: u32, wood_lo: Color32, wood_hi: Color32) {
    let up = v(0.0, 1.0, 0.0);
    let fwd = normv(facing);
    let side = { let cxv = crossv(up, fwd); if cxv.x * cxv.x + cxv.z * cxv.z < 1e-6 { v(1.0, 0.0, 0.0) } else { normv(cxv) } };
    let rnd = |k: u32| { let mut h = seed.wrapping_mul(0x27D4_EB2F).wrapping_add(k.wrapping_mul(0x9E37_79B1)); h ^= h >> 15; h = h.wrapping_mul(0x85EB_CA77); h ^= h >> 13; (h & 0xFFFF) as f32 / 65535.0 };
    let th = 1.05_f32;
    let top_c = addv(pos, mulv(up, th));
    // faint warm floor pool (bloom anchor, reads as lamplight on the wet stone)
    let mut pool = m3::disc(addv(pos, mulv(up, 0.03)), 1.9, 30, c(0x6e, 0x4c, 0x24));
    pool.mat = M::STONE; pool.emissive = 0.07; s.add(pool, Float::STILL);
    // pedestal table: column + base + round top slab
    let mut col = m3::tube(pos, addv(pos, mulv(up, th)), 0.16, 10, wood_hi); col.mat = M::WOOD; s.add(col, Float::STILL);
    let mut base = m3::disc(addv(pos, mulv(up, 0.05)), 0.5, 18, wood_lo); m3::tint_up(&mut base, wood_hi, wood_lo); base.mat = M::WOOD; s.add(base, Float::STILL);
    let mut top = m3::tube(addv(top_c, mulv(up, -0.06)), addv(top_c, mulv(up, 0.06)), 0.92, 24, wood_hi); top.mat = M::WOOD; s.add(top, Float::STILL);
    let mut topd = m3::disc(addv(top_c, mulv(up, 0.06)), 0.92, 24, wood_hi); m3::tint_up(&mut topd, wood_hi, wood_lo); topd.mat = M::WOOD; s.add(topd, Float::STILL);
    // lamp on the table — post + glowing warm shade
    let lbase = addv(top_c, addv(mulv(side, 0.42), mulv(up, 0.06)));
    let mut lpost = m3::tube(lbase, addv(lbase, mulv(up, 0.4)), 0.04, 6, wood_hi); lpost.mat = M::WOOD; s.add(lpost, Float::STILL);
    let mut shade = m3::uv_sphere(addv(lbase, mulv(up, 0.55)), 0.22, 14, 10, c(0xff, 0xdc, 0xa2));
    shade.mat = M::STONE; shade.emissive = 1.12; s.add(shade, Float { amp: 0.0, speed: 0.0, phase: 0.0, drift: 0.0 });
    // a small stack of books on the table
    let stack = addv(top_c, addv(mulv(side, -0.4), mulv(up, 0.06)));
    let bcol = [c(0x6e, 0x2a, 0x22), c(0x2c, 0x4a, 0x32), c(0x22, 0x36, 0x5e), c(0x8a, 0x6e, 0x2a)];
    for b in 0..3 {
        let mut bk = m3::tube(addv(stack, mulv(up, b as f32 * 0.09)), addv(stack, mulv(up, b as f32 * 0.09 + 0.08)), 0.26, 4, bcol[(rnd(b) * 4.0) as usize % 4]);
        bk.mat = M::STONE; s.add(bk, Float::STILL);
    }
    // chair behind the table (relative to facing): seat + back + legs
    let cpos = addv(pos, mulv(fwd, -1.45));
    let mut seat = m3::tube(addv(cpos, mulv(up, 0.52)), addv(cpos, mulv(up, 0.58)), 0.34, 12, wood_lo); seat.mat = M::WOOD; s.add(seat, Float::STILL);
    let mut seatd = m3::disc(addv(cpos, mulv(up, 0.58)), 0.34, 12, wood_hi); seatd.mat = M::WOOD; s.add(seatd, Float::STILL);
    let backc = addv(cpos, addv(mulv(fwd, -0.3), mulv(up, 0.92)));
    let mut back = m3::panel(backc, side, up, 0.6, 0.7, wood_hi); back.mat = M::WOOD; s.add(back, Float::STILL);
    for (lx, lz) in [(0.24f32, 0.24f32), (-0.24, 0.24), (0.24, -0.24), (-0.24, -0.24)] {
        let lp = addv(cpos, addv(mulv(side, lx), mulv(fwd, lz)));
        let mut leg = m3::tube(lp, addv(lp, mulv(up, 0.52)), 0.04, 5, wood_lo); leg.mat = M::WOOD; s.add(leg, Float::STILL);
    }
}

/// A WALL ALCOVE: a recessed arched niche set among the bookshelves, holding a softly glowing urn on a small
/// plinth — warm punctuation in the long wall of books.
fn wall_alcove(s: &mut Scene, base: Vec3, inward: Vec3, tangent: Vec3, width: f32, height: f32, seed: u32, stone_lo: Color32, stone_hi: Color32) {
    let up = v(0.0, 1.0, 0.0);
    let inw = normv(inward); let tan = normv(tangent);
    let at = |t: f32, y: f32, d: f32| addv(addv(base, mulv(tan, t)), addv(mulv(up, y), mulv(inw, d)));
    // dark recessed back (arched)
    let mut back = m3::arch_window(at(0.0, 0.25, 0.06), tan, up, width * 0.82, height * 0.82, c(0x05, 0x0e, 0x12));
    back.mat = M::STONE; back.emissive = 0.0; s.add(back, Float::STILL);
    // side jambs + a keystone-ish lintel of lighter stone framing the niche
    for sgn in [-1.0f32, 1.0] {
        let mut jamb = m3::tube(at(sgn * width * 0.42, 0.0, 0.18), at(sgn * width * 0.42, height * 0.78, 0.18), 0.12, 8, stone_hi);
        m3::tint_height(&mut jamb, base.y, base.y + height, stone_lo, stone_hi); jamb.mat = M::STONE; s.add(jamb, Float::STILL);
    }
    // plinth + glowing urn
    let pl = at(0.0, 0.0, -0.15);
    let mut plinth = m3::tube(pl, addv(pl, mulv(up, height * 0.32)), 0.26, 12, stone_lo); m3::tint_height(&mut plinth, base.y, base.y + height * 0.32, stone_lo, stone_hi); plinth.mat = M::STONE; s.add(plinth, Float::STILL);
    let uc = at(0.0, height * 0.32, -0.15);
    let mut bowl = m3::tube(uc, addv(uc, mulv(up, 0.26)), 0.2, 12, c(0xc9, 0x9a, 0x52)); bowl.mat = M::STONE; bowl.emissive = 0.2; s.add(bowl, Float::STILL);
    let _ = seed;
    let mut glow = m3::uv_sphere(addv(uc, mulv(up, 0.34)), 0.2, 14, 10, c(0xff, 0xcf, 0x8a));
    glow.mat = M::STONE; glow.emissive = 1.25; s.add(glow, Float { amp: 0.03, speed: 0.6, phase: (seed & 7) as f32, drift: 0.0 });
}

/// FLOATING BOOKS — a few open books drifting in the air of the half-dreamt library, each a two-leaf "V"
/// (pale pages on a coloured cover) bobbing + turning on its own Float. Whimsical procedural ambience.
fn floating_books(s: &mut Scene, r_inner: f32, y_floor: f32, y_cap: f32, seed: u32, count: usize) {
    use std::f32::consts::TAU;
    let rnd = |k: u32| { let mut h = seed.wrapping_mul(0x9E37_79B1).wrapping_add(k.wrapping_mul(0x85EB_CA77)); h ^= h >> 15; h = h.wrapping_mul(0x2C1B_3C6D); h ^= h >> 13; (h & 0xFFFF) as f32 / 65535.0 };
    let covers = [c(0x6e, 0x2a, 0x22), c(0x2c, 0x4a, 0x32), c(0x22, 0x36, 0x5e), c(0x7a, 0x52, 0x1e)];
    for i in 0..count as u32 {
        let a = rnd(i * 7) * TAU;
        let rad = (0.2 + rnd(i * 11) * 0.7) * r_inner;
        let center = v(a.cos() * rad, y_floor + 1.8 + rnd(i * 13) * (y_cap - y_floor - 1.0).max(1.0), a.sin() * rad);
        let cover = covers[(rnd(i * 17) * 4.0) as usize % 4];
        let fwd = v((a + 1.3).cos(), 0.0, (a + 1.3).sin());                 // spine direction (horizontal)
        let right = normv(crossv(v(0.0, 1.0, 0.0), fwd));
        let sz = 0.45 + rnd(i * 23) * 0.3;
        let fl = Float { amp: 0.25 + rnd(i * 29) * 0.35, speed: 0.3 + rnd(i * 31) * 0.3, phase: rnd(i * 37) * 6.28, drift: 0.5 + rnd(i * 41) * 0.9 };
        for sgn in [-1.0f32, 1.0] {
            let leafdir = normv(addv(mulv(right, sgn), v(0.0, 0.42, 0.0)));  // page splays out + up from the spine
            let pc = addv(center, mulv(leafdir, sz * 0.55));
            let mut page = m3::panel(pc, fwd, leafdir, sz, sz * 1.25, c(0xef, 0xe7, 0xd6));
            page.mat = M::WOOD; page.emissive = 0.06; s.add(page, fl);
            let cc = addv(center, mulv(leafdir, sz * 0.5));
            let mut cov = m3::panel(addv(cc, mulv(leafdir, -0.03)), fwd, leafdir, sz * 1.1, sz * 1.35, cover);
            cov.mat = M::WOOD; s.add(cov, fl);
        }
    }
}

/// A SEA GARDEN: a scattered patch of vegetation — swaying kelp strands, branching coral, and tentacled
/// anemones — seeded by `seed`. A region building block (sea-garden / terrace spaces).
/// A FISH SPECIES archetype — body proportions, fin sizes, colours. `len/h/w` are body length / height /
/// lateral width (×sc): fish are tall & thin, rays wide & flat, eels long & low. `fluke_horiz` gives a
/// whale's horizontal tail instead of a forked vertical one.
#[derive(Clone, Copy)]
struct FishSpecies { len: f32, h: f32, w: f32, full: f32, tail: f32, fork: f32, dorsal: f32, anal: f32, pect: f32, body: Color32, belly: Color32, fin: Color32, emis: f32, eye: bool, fluke_horiz: bool }

/// The bestiary (index 7 = whale, used for the big drifters): tuna, angelfish, clownfish, eel, grouper, ray,
/// lanternfish, whale — a varied school built procedurally from primitives.
fn fish_species() -> [FishSpecies; 8] {
    [
        FishSpecies { len: 2.4, h: 0.70, w: 0.32, full: 0.52, tail: 0.7,  fork: 0.9,  dorsal: 0.25, anal: 0.18, pect: 0.30, body: c(0x29,0x5a,0x84), belly: c(0xcf,0xe2,0xea), fin: c(0x3a,0x72,0x9c), emis: 0.14, eye: true,  fluke_horiz: false }, // tuna
        FishSpecies { len: 1.2, h: 1.50, w: 0.18, full: 0.62, tail: 0.4,  fork: 0.3,  dorsal: 0.9,  anal: 0.8,  pect: 0.28, body: c(0xe6,0xc1,0x3a), belly: c(0xf4,0xee,0xcf), fin: c(0xf0,0xd8,0x7a), emis: 0.14, eye: true,  fluke_horiz: false }, // angelfish
        FishSpecies { len: 1.0, h: 0.85, w: 0.30, full: 0.60, tail: 0.45, fork: 0.4,  dorsal: 0.4,  anal: 0.3,  pect: 0.34, body: c(0xe4,0x6a,0x22), belly: c(0xf2,0xe8,0xdc), fin: c(0xf2,0xe8,0xdc), emis: 0.15, eye: true,  fluke_horiz: false }, // clownfish
        FishSpecies { len: 3.4, h: 0.42, w: 0.22, full: 0.42, tail: 0.3,  fork: 0.1,  dorsal: 0.5,  anal: 0.2,  pect: 0.0,  body: c(0x36,0x6e,0x40), belly: c(0x8a,0xb0,0x66), fin: c(0x52,0x86,0x52), emis: 0.12, eye: true,  fluke_horiz: false }, // eel
        FishSpecies { len: 2.0, h: 0.95, w: 0.45, full: 0.60, tail: 0.5,  fork: 0.25, dorsal: 0.45, anal: 0.3,  pect: 0.40, body: c(0x5e,0x5a,0x3a), belly: c(0xb6,0xae,0x82), fin: c(0x7a,0x72,0x4e), emis: 0.10, eye: true,  fluke_horiz: false }, // grouper
        FishSpecies { len: 1.8, h: 0.26, w: 1.70, full: 0.55, tail: 0.25, fork: 0.1,  dorsal: 0.0,  anal: 0.0,  pect: 1.6,  body: c(0x2c,0x3c,0x52), belly: c(0x9a,0xa8,0xb6), fin: c(0x33,0x46,0x5e), emis: 0.08, eye: false, fluke_horiz: false }, // ray / manta
        FishSpecies { len: 1.1, h: 0.60, w: 0.30, full: 0.55, tail: 0.5,  fork: 0.5,  dorsal: 0.3,  anal: 0.2,  pect: 0.26, body: c(0x1e,0x3a,0x66), belly: c(0x3a,0x6e,0x9a), fin: c(0x7a,0xe0,0xf0), emis: 0.55, eye: true,  fluke_horiz: false }, // lanternfish (glows)
        FishSpecies { len: 5.5, h: 1.60, w: 1.10, full: 0.60, tail: 1.4,  fork: 0.0,  dorsal: 0.3,  anal: 0.0,  pect: 0.9,  body: c(0x26,0x3a,0x4a), belly: c(0x6a,0x82,0x92), fin: c(0x38,0x52,0x64), emis: 0.06, eye: true,  fluke_horiz: true  }, // whale
    ]
}

/// Build ONE fish/whale of `sp` at `pos`, facing `fwd`, scaled by `sc`. Built in a LOCAL frame (+x forward,
/// +y up, +z lateral): a chain of laterally-compressed, tapering ellipsoid body segments + a forked (or, for
/// a whale, horizontal) caudal fin + dorsal/anal/pectoral fins + eyes — then transformed into world. All
/// parts share one `Float` so the creature sways/bobs as a rigid body.
fn add_fish(s: &mut Scene, pos: Vec3, fwd: Vec3, up_hint: Vec3, sp: &FishSpecies, sc: f32, fl: Float) {
    let fwd = normv(fwd);
    let side = { let cx = crossv(up_hint, fwd); if cx.x * cx.x + cx.y * cx.y + cx.z * cx.z < 1e-6 { v(0.0, 0.0, 1.0) } else { normv(cx) } };
    let up = normv(crossv(fwd, side));
    let mut emit = |mut m: m3::Mesh3, emis: f32| {
        for p in &mut m.verts { let l = *p; *p = addv(pos, addv(mulv(fwd, l.x), addv(mulv(up, l.y), mulv(side, l.z)))); }
        for nn in &mut m.norms { let l = *nn; *nn = addv(mulv(fwd, l.x), addv(mulv(up, l.y), mulv(side, l.z))); }
        m.mat = M::WATER; m.emissive = emis; s.add(m, fl);
    };
    let (len, h, w) = (sp.len * sc, sp.h * sc, sp.w * sc);
    let nseg = 5;
    for i in 0..nseg {
        let t = (i as f32 + 0.5) / nseg as f32;                  // 0 head → 1 tail
        let xc = len * (0.5 - t);
        let prof = (sp.full * (1.0 - ((t - 0.3) * 1.55).powi(2))).max(0.06 * sp.full); // teardrop, fuller near head
        let (rx, ry, rz) = (len / nseg as f32 * 0.8, h * prof, w * prof);
        let mut seg = m3::uv_sphere(v(0.0, 0.0, 0.0), 1.0, 12, 8, sp.body);
        m3::scale3(&mut seg, rx, ry, rz);
        for p in &mut seg.verts { p.x += xc; }
        m3::tint_height(&mut seg, -ry * 1.1, ry * 1.1, sp.belly, sp.body);  // countershade: belly light, back dark
        emit(seg, sp.emis);
    }
    let tx = -len * 0.5;
    if sp.fluke_horiz {                                          // whale: broad horizontal fluke
        let fin = m3::panel(v(tx - sp.tail * sc * 0.5, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 0.0, 1.0), sp.tail * sc, sp.tail * sc * 1.6, sp.fin);
        emit(fin, sp.emis);
    } else {                                                     // fish: forked vertical caudal (two lobes)
        for lobe in [-1.0f32, 1.0] {
            let fin = m3::panel(v(tx - sp.tail * sc * 0.45, lobe * sp.fork * sc * 0.5, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0), sp.tail * sc, sp.fork * sc * 0.6 + 0.25 * sc, sp.fin);
            emit(fin, sp.emis);
        }
    }
    if sp.dorsal > 0.01 { let d = m3::panel(v(len * 0.02, h * sp.full * 0.55 + sp.dorsal * sc * 0.4, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0), len * 0.5, sp.dorsal * sc, sp.fin); emit(d, sp.emis); }
    if sp.anal  > 0.01 { let a = m3::panel(v(-len * 0.1, -(h * sp.full * 0.5 + sp.anal * sc * 0.35), 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0), len * 0.35, sp.anal * sc, sp.fin); emit(a, sp.emis); }
    if sp.pect  > 0.01 { for sgn in [-1.0f32, 1.0] { let pf = m3::panel(v(len * 0.16, -h * 0.08, sgn * w * 0.55), v(1.0, 0.0, 0.0), v(0.0, 0.0, sgn), sp.pect * sc, sp.pect * sc * 0.7, sp.fin); emit(pf, sp.emis); } }
    if sp.eye { for sgn in [-1.0f32, 1.0] { let e = m3::uv_sphere(v(len * 0.38, h * 0.16, sgn * w * 0.52), (h * 0.12).max(0.05), 8, 6, c(0x0e, 0x10, 0x14)); emit(e, 0.0); } }
}

/// SEA LIFE beyond the boundary — schools of fish and a whale or two drifting in the water OUTSIDE the dome,
/// seen through the colonnade, the clear windows and the open hallway sides (the reference's depth-life). The
/// interior stays air; this is all placed past the perimeter. Count/variety follow the conversation.
fn sea_life(s: &mut Scene, r_perim: f32, y_floor: f32, y_cap: f32, y_high: f32, seed: u32, nodes: u32) {
    use std::f32::consts::TAU;
    let r = |i: i32, salt: i32| hash(i.wrapping_mul(2654435), salt ^ (seed as i32));
    let sp = fish_species();
    let n = (10 + nodes / 9).clamp(12, 30) as i32;
    for i in 0..n {
        let a = r(i, 7) * TAU;
        let rad = r_perim + 4.0 + r(i, 11) * 18.0;
        let y = y_floor + 1.5 + r(i, 13) * (y_cap - y_floor + 6.0);
        let pos = v(a.cos() * rad, y, a.sin() * rad);
        let swim = a + std::f32::consts::FRAC_PI_2 + (r(i, 17) - 0.5) * 1.4; // roughly tangential, jittered
        let dir = v(swim.cos(), (r(i, 19) - 0.5) * 0.3, swim.sin());
        let species = &sp[((r(i, 23) * 7.0) as usize).min(6)];   // 0..6 (7 is the whale)
        let sc = (0.7 + r(i, 29) * 0.8) * if species.len > 3.0 { 0.7 } else { 1.0 };
        let fl = Float { amp: 0.3 + r(i, 31) * 0.4, speed: 0.4 + r(i, 37) * 0.5, phase: r(i, 41) * 6.28, drift: 0.8 + r(i, 43) * 1.3 };
        add_fish(s, pos, dir, v(0.0, 1.0, 0.0), species, sc, fl);
    }
    let nw = 1 + (seed % 2) as i32;
    for w in 0..nw {
        let a = r(w, 51) * TAU;
        let rad = r_perim + 12.0 + r(w, 53) * 14.0;
        let y = y_cap + 2.0 + r(w, 57) * (y_high - y_cap).max(2.0);
        let pos = v(a.cos() * rad, y, a.sin() * rad);
        let swim = a + std::f32::consts::FRAC_PI_2 + (r(w, 59) - 0.5) * 0.6;
        let dir = v(swim.cos(), -0.04, swim.sin());
        let sc = 0.85 + r(w, 61) * 0.5;
        let fl = Float { amp: 0.5, speed: 0.13 + r(w, 63) * 0.06, phase: r(w, 67) * 6.28, drift: 1.6 };
        add_fish(s, pos, dir, v(0.0, 1.0, 0.0), &sp[7], sc, fl); // whale
    }
}

fn sea_garden(s: &mut Scene, center: Vec3, radius: f32, seed: u32) {
    use std::f32::consts::TAU;
    let kelp_lo = c(0x22, 0x5e, 0x32); let kelp_hi = c(0x52, 0xa0, 0x56);
    let coral_lo = c(0xc4, 0x5e, 0x6e); let coral_hi = c(0xf2, 0xa2, 0x8c);
    let rnd = |k: u32| { let mut h = seed.wrapping_mul(0x27D4_EB2F).wrapping_add(k.wrapping_mul(0x9E37_79B1)); h ^= h >> 15; h = h.wrapping_mul(0x85EB_CA77); h ^= h >> 13; (h & 0xFFFF) as f32 / 65535.0 };
    let n = 9 + (seed % 7) as usize;
    for i in 0..n {
        let ik = i as u32;
        let a = rnd(ik * 3) * TAU; let rr = radius * rnd(ik * 3 + 1).sqrt();
        let p = v(center.x + a.cos() * rr, center.y, center.z + a.sin() * rr);
        match (rnd(ik * 3 + 2) * 3.0) as u32 {
            0 => { // kelp — tall wavy frond
                let h = (1.6 + 2.6 * rnd(ik * 7)) * 1.0;
                let mut k = m3::strand(p, v(p.x, p.y + h, p.z), 0.06, 5, 9, 0.0, 0.4, kelp_lo);
                m3::tint_up(&mut k, kelp_hi, kelp_lo); k.mat = M::MOSS;
                s.add(k, Float { amp: 0.5 + rnd(ik), speed: 0.4 + 0.4 * rnd(ik + 1), phase: rnd(ik + 2) * 6.28, drift: 0.35 });
            }
            1 => { // coral — branching cluster
                let bn = 3 + (rnd(ik * 5) * 3.0) as usize;
                for b in 0..bn {
                    let ba = rnd(ik * 5 + b as u32) * TAU; let bl = (0.5 + 0.7 * rnd(ik + b as u32)) * 1.0;
                    let tip = v(p.x + ba.cos() * bl * 0.6, p.y + bl, p.z + ba.sin() * bl * 0.6);
                    let mut cbr = m3::tube(p, tip, 0.05, 6, coral_lo); m3::tint_up(&mut cbr, coral_hi, coral_lo);
                    cbr.mat = M::MOSS; cbr.emissive = 0.05; s.add(cbr, Float::STILL);
                    let mut tb = m3::uv_sphere(tip, 0.08, 8, 6, coral_hi); tb.mat = M::MOSS; tb.emissive = 0.12; s.add(tb, Float::STILL);
                }
            }
            _ => { // anemone — bulb with swaying tentacles
                let mut bulb = m3::uv_sphere(p, 0.18, 10, 8, coral_lo); bulb.mat = M::MOSS; s.add(bulb, Float::STILL);
                for tt in 0..6 {
                    let ta = (tt as f32 / 6.0) * TAU;
                    let tip = v(p.x + ta.cos() * 0.26, p.y + 0.5 + 0.3 * rnd(ik + tt as u32), p.z + ta.sin() * 0.26);
                    let mut tent = m3::tube(p, tip, 0.025, 5, coral_hi); tent.mat = M::MOSS; tent.emissive = 0.12;
                    s.add(tent, Float { amp: 0.3, speed: 1.0, phase: ta, drift: 0.2 });
                }
            }
        }
    }
}

/// A REEF: a natural rock outcrop — a cluster of boulders mounded together, crusted with a few coral knobs
/// and a frond or two of kelp. Sits on the sea floor outside the dome (with sea_garden, fish + whales).
fn reef(s: &mut Scene, center: Vec3, scale: f32, seed: u32, lo: Color32, hi: Color32) {
    use std::f32::consts::TAU;
    let rnd = |k: u32| { let mut h = seed.wrapping_mul(0x27D4_EB2F).wrapping_add(k.wrapping_mul(0x9E37_79B1)); h ^= h >> 15; h = h.wrapping_mul(0x85EB_CA77); h ^= h >> 13; (h & 0xFFFF) as f32 / 65535.0 };
    let bn = 3 + (rnd(1) * 4.0) as usize;
    for i in 0..bn as u32 {                                  // mounded boulders
        let a = rnd(i * 7) * TAU; let rr = rnd(i * 11) * scale * 0.9;
        let p = v(center.x + a.cos() * rr, center.y + rnd(i * 13) * 0.4 * scale, center.z + a.sin() * rr);
        let mut bo = m3::rock(p, (0.5 + 0.8 * rnd(i * 17)) * scale, seed.wrapping_add(i * 31), lo);
        m3::tint_up(&mut bo, hi, lo); bo.mat = M::STONE; s.add(bo, Float::STILL);
    }
    let (coral_lo, coral_hi) = (c(0xc4, 0x5e, 0x6e), c(0xf2, 0xa2, 0x8c));
    for i in 0..(2 + (rnd(3) * 3.0) as u32) {                // coral knobs crusting the rock
        let a = rnd(i * 19) * TAU; let rr = rnd(i * 23) * scale * 0.8;
        let p = v(center.x + a.cos() * rr, center.y + 0.3 * scale, center.z + a.sin() * rr);
        let bl = (0.4 + 0.6 * rnd(i * 29)) * scale;
        let tip = v(p.x + (a.cos()) * bl * 0.4, p.y + bl, p.z + a.sin() * bl * 0.4);
        let mut br = m3::tube(p, tip, 0.05 * scale, 6, coral_lo); m3::tint_up(&mut br, coral_hi, coral_lo);
        br.mat = M::MOSS; br.emissive = 0.06; s.add(br, Float::STILL);
        let mut tb = m3::uv_sphere(tip, 0.09 * scale, 8, 6, coral_hi); tb.mat = M::MOSS; tb.emissive = 0.12; s.add(tb, Float::STILL);
    }
    for i in 0..(1 + (rnd(5) * 2.0) as u32) {                // a kelp frond or two
        let a = rnd(i * 37) * TAU; let rr = rnd(i * 41) * scale;
        let p = v(center.x + a.cos() * rr, center.y, center.z + a.sin() * rr);
        let h = (1.4 + 2.2 * rnd(i * 43)) * scale;
        let mut k = m3::strand(p, v(p.x, p.y + h, p.z), 0.06 * scale, 5, 9, 0.0, 0.4, c(0x22, 0x5e, 0x32));
        m3::tint_up(&mut k, c(0x52, 0xa0, 0x56), c(0x22, 0x5e, 0x32)); k.mat = M::MOSS;
        s.add(k, Float { amp: 0.5 + rnd(i), speed: 0.4, phase: rnd(i + 2) * 6.28, drift: 0.35 });
    }
}

/// A ROCK ARCH: two leaning rock piers bridged by a span of boulders — a natural sea-floor archway the
/// fish thread through. `dir` is the axis it spans.
fn rock_arch(s: &mut Scene, center: Vec3, dir: Vec3, scale: f32, seed: u32, lo: Color32, hi: Color32) {
    let dir = normv(dir);
    let span = scale * 2.2;
    let foot0 = addv(center, mulv(dir, -span * 0.5));
    let foot1 = addv(center, mulv(dir, span * 0.5));
    let apex = v(center.x, center.y + scale * 2.0, center.z);
    for (j, foot) in [foot0, foot1].iter().enumerate() {     // two piers leaning toward the apex
        for k in 0..3u32 {
            let t = k as f32 / 3.0;
            let p = v(foot.x + (apex.x - foot.x) * t * 0.85, foot.y + (apex.y - foot.y) * t, foot.z + (apex.z - foot.z) * t * 0.85);
            let mut bo = m3::rock(p, (0.8 - 0.18 * k as f32) * scale, seed.wrapping_add(j as u32 * 17 + k), lo);
            m3::tint_up(&mut bo, hi, lo); bo.mat = M::STONE; s.add(bo, Float::STILL);
        }
    }
    for k in 0..3u32 {                                       // keystone span across the top
        let t = (k as f32 + 0.5) / 3.0;
        let p = v(foot0.x + (foot1.x - foot0.x) * t, apex.y + scale * 0.1, foot0.z + (foot1.z - foot0.z) * t);
        let mut bo = m3::rock(p, 0.7 * scale, seed.wrapping_add(90 + k), hi);
        m3::tint_up(&mut bo, hi, lo); bo.mat = M::STONE; s.add(bo, Float::STILL);
    }
}

#[inline] fn mix_rgb(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    c((a.r() as f32 + (b.r() as f32 - a.r() as f32) * t) as u8,
      (a.g() as f32 + (b.g() as f32 - a.g() as f32) * t) as u8,
      (a.b() as f32 + (b.b() as f32 - a.b() as f32) * t) as u8)
}

/// Render quality 0.6..1.3 (mesh-density LOD lever): scales procedural grid/scatter density. Default 1.0;
/// LOGSCOPE_QUALITY lowers it for weak GPUs / raises it for stills. (Frame-update + optical-effect LODs hook
/// elsewhere — see TODO.)
fn render_quality() -> f32 {
    std::env::var("LOGSCOPE_QUALITY").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0).clamp(0.6, 1.3)
}

/// Value-noise fractal (3 octaves), 0..1. Cheap hills for the sea floor.
fn fbm2(mut x: f32, mut z: f32, seed: u32) -> f32 {
    let vh = |ix: i32, iz: i32| -> f32 {
        let mut h = (ix as u32).wrapping_mul(374_761_393).wrapping_add((iz as u32).wrapping_mul(668_265_263)).wrapping_add(seed.wrapping_mul(362_437));
        h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
        ((h ^ (h >> 16)) & 0xFFFF) as f32 / 65535.0
    };
    let vn = |x: f32, z: f32| -> f32 {
        let (gx, gz) = (x.floor(), z.floor());
        let (fx, fz) = (x - gx, z - gz);
        let (sx, sz) = (fx * fx * (3.0 - 2.0 * fx), fz * fz * (3.0 - 2.0 * fz));
        let (ix, iz) = (gx as i32, gz as i32);
        let (n00, n10) = (vh(ix, iz), vh(ix + 1, iz));
        let (n01, n11) = (vh(ix, iz + 1), vh(ix + 1, iz + 1));
        let a = n00 + (n10 - n00) * sx; let b = n01 + (n11 - n01) * sx; a + (b - a) * sz
    };
    let (mut amp, mut sum, mut nrm) = (1.0f32, 0.0f32, 0.0f32);
    for _ in 0..3 { sum += amp * vn(x, z); nrm += amp; x *= 2.0; z *= 2.0; amp *= 0.5; }
    sum / nrm
}

/// Height of the ocean floor at (x,z): a general downward slope away from the dome (DEPTH) plus rolling hills
/// and finer ridges (value-noise fbm). Shared by the terrain mesh AND whatever sits on it (reefs, gardens), so
/// formations rest ON the hillsides instead of hovering over a flat plane.
fn seafloor_h(x: f32, z: f32, r_inner: f32, y_base: f32, seed: u32) -> f32 {
    let r = (x * x + z * z).sqrt();
    let slope = (r - r_inner).max(0.0) * 0.12;                          // deeper further out
    let hills = (fbm2(x * 0.055, z * 0.055, seed) - 0.5) * 5.2;         // broad rolling hills
    let ridges = (fbm2(x * 0.16 + 11.0, z * 0.16 + 7.0, seed ^ 0x99) - 0.5) * 1.5; // finer relief
    y_base - slope + hills + ridges
}

/// A SEA-FLOOR TERRAIN: a displaced radial heightfield from `r_inner` out to `r_outer`, sand in the low
/// hollows grading to rock on the mounds, sloping into depth at the rim. Hillsides + depth for the open ocean.
fn seafloor(s: &mut Scene, r_inner: f32, r_outer: f32, y_base: f32, sz: f32, seed: u32, sand: Color32, rock: Color32) {
    use std::f32::consts::TAU;
    let q = render_quality();
    let rings = ((24.0 * q) as usize).max(10);
    let spokes = ((84.0 * q) as usize).max(40);
    let mut verts = Vec::with_capacity((rings + 1) * spokes);
    let mut vcol = Vec::with_capacity((rings + 1) * spokes);
    for ri in 0..=rings {
        let rr = r_inner + (r_outer - r_inner) * ri as f32 / rings as f32;
        for si in 0..spokes {
            let a = si as f32 / spokes as f32 * TAU;
            let (x, z) = (a.cos() * rr, a.sin() * rr * sz);
            let y = seafloor_h(x, z, r_inner, y_base, seed);
            verts.push(v(x, y, z));
            let t = (((y - y_base) + 2.6) / 5.2).clamp(0.0, 1.0);       // low→sand, high→rock
            vcol.push(mix_rgb(sand, rock, t * t));
        }
    }
    let idx = |ri: usize, si: usize| (ri * spokes + (si % spokes)) as u32;
    let mut tris = Vec::with_capacity(rings * spokes * 2);
    for ri in 0..rings {
        for si in 0..spokes {
            let (a, b, cc, d) = (idx(ri, si), idx(ri, si + 1), idx(ri + 1, si), idx(ri + 1, si + 1));
            tris.push([a, cc, b]); tris.push([b, cc, d]);
        }
    }
    let mut norms = vec![v(0.0, 0.0, 0.0); verts.len()];
    for t in &tris {
        let (p0, p1, p2) = (verts[t[0] as usize], verts[t[1] as usize], verts[t[2] as usize]);
        let n = crossv(v(p1.x - p0.x, p1.y - p0.y, p1.z - p0.z), v(p2.x - p0.x, p2.y - p0.y, p2.z - p0.z));
        for &i in t.iter() { norms[i as usize] = addv(norms[i as usize], n); }
    }
    for nrm in norms.iter_mut() { *nrm = normv(*nrm); }
    s.add(Mesh3 { verts, norms, tris, color: sand, vcol: Some(vcol), offset: v(0.0, 0.0, 0.0), mat: M::STONE, emissive: 0.0 }, Float::STILL);
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// MIND-PALACE REGIONS — the graph→space program (build-path steps 1·2·3 + geometry of 4 + 5).
//
// A conversation BRANCH becomes a DOOR in the hall's perimeter wall, and beyond that door sits a
// generated side-wing (a "region") whose SPACE TYPE is chosen from the branch's graph SHAPE:
//     loop (back-ref)   → Rotunda      (a ring you circle and return from)
//     deep thread       → Stairs       (a staired descent)
//     long tangent      → Terrace      (an open sea-garden platform)
//     token-heavy node  → Chamber      (a larger enclosed room)
//     pruned / dead-end → SealedCave   (a blocked, unlit dead end)
//
// Each generator emits geometry AND colliders from one seed, reusing the element library
// (columns, arches, statues, sea-garden, rocks) so the wings read as part of the same palace.
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The space a region takes, chosen from a conversation branch's graph shape.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RegionType { Rotunda, Stairs, Terrace, Chamber, SealedCave }

/// A side-wing placed BEYOND a perimeter door. Polar placement (bearing from the hall centre +
/// distance out), a size `scale` (token-heavy branches grow), and a `seed` for its generator.
#[derive(Clone, Copy)]
pub struct Region {
    pub kind: RegionType,
    pub angle: f32, // bearing from hall centre to the region (also its door bearing)
    pub dist: f32,  // distance from hall centre to the region centre
    pub scale: f32, // size multiplier
    pub seed: u32,
}

/// Plan one region per door from the conversation params.
///
/// SHAPE SOURCE (the seam): real per-branch graph shape — is-loop / depth / length / token-mass /
/// is-pruned — is not yet plumbed from the graph into `EnvParams` (that lands when the per-branch
/// descriptor is wired through `chat.rs`, deliberately untouched here while perf work is inbound).
/// Until then the type is derived DETERMINISTICALLY from `(seed, branch index)` so it is stable
/// per conversation (same convo → same palace, brain-sticky) and the mix stays varied. Swap the
/// body of `kind_for` for the real branch shape when the descriptor arrives — nothing else changes.
fn region_plan(pr: &EnvParams, doors: usize, r_hall: f32) -> Vec<Region> {
    use std::f32::consts::TAU;
    let rnd = |k: u32| -> f32 {
        let mut h = pr.seed.wrapping_mul(0x9E37_79B1).wrapping_add(k.wrapping_mul(0x85EB_CA77));
        h ^= h >> 15; h = h.wrapping_mul(0x2C1B_3C6D); h ^= h >> 13;
        (h & 0xFFFF) as f32 / 65535.0
    };
    // type mix derived from the per-branch shape stand-in (see doc above)
    let kind_for = |i: u32| -> RegionType {
        // a per-branch shape signature; replace with real {is_loop, depth, len, tokens, pruned}
        let shape = ((pr.seed ^ i.wrapping_mul(0x68E3_1DA4)).rotate_left(i % 13 + 1)) % 100;
        match shape {
            0..=14  => RegionType::SealedCave, // ~15% pruned/dead ends
            15..=39 => RegionType::Rotunda,    // loops
            40..=63 => RegionType::Terrace,    // long tangents
            64..=83 => RegionType::Stairs,     // deep threads
            _       => RegionType::Chamber,    // token-heavy
        }
    };
    let mut out = Vec::with_capacity(doors);
    for d in 0..doors as u32 {
        // ROUTE to the branch's ACTUAL bearing when chat.rs has plumbed it (bear[d] != 0, encoded as a turn
        // fraction in 1..=65535); otherwise fall back to an even seeded spread. Then nudge any wing clear of
        // the hallway axis (±x) so a wing and a hallway mouth don't fight for the same opening.
        let angle = if (d as usize) < pr.bear.len() && pr.bear[d as usize] != 0 {
            (pr.bear[d as usize] as f32 / 65535.0) * TAU
        } else {
            (d as f32 + 0.5) / doors as f32 * TAU + (rnd(d * 7) - 0.5) * (TAU / doors as f32) * 0.45
        };
        let angle = {
            let mut a = angle.rem_euclid(TAU);
            for axis in [0.0_f32, std::f32::consts::PI] {
                let dd = (a - axis).abs(); let dd = dd.min(TAU - dd);
                if dd < 0.42 { a = axis + if (a - axis).rem_euclid(TAU) < std::f32::consts::PI { 0.42 } else { -0.42 }; }
            }
            a.rem_euclid(TAU)
        };
        let kind = kind_for(d);
        let scale = match kind {
            RegionType::Chamber => 1.25 + 0.35 * rnd(d * 3 + 1), // token-heavy → larger
            RegionType::SealedCave => 0.8,
            _ => 0.95 + 0.3 * rnd(d * 3 + 2),
        };
        // sit the wing in the outer ring, clear of the perimeter wall, under the enclosing dome
        let dist = r_hall + 6.5 + scale * 3.2;
        out.push(Region { kind, angle, dist, scale, seed: pr.seed ^ d.wrapping_mul(0x9E37_79B1) });
    }
    out
}

/// A passable door set into the perimeter wall at `wc` (on the wall), facing `inward`. A stone arch
/// surround + a glowing WATER threshold (passable — the centre carries NO collider) + jambs with
/// jamb-only colliders, so the opening stays clear for the fly camera to thread later.
fn region_door(s: &mut Scene, wc: Vec3, right: Vec3, up: Vec3, inward: Vec3, y_floor: f32,
               stone_lo: Color32, stone_hi: Color32) {
    let mut surround = m3::arch_window(v(wc.x, y_floor, wc.z), right, up, 3.0, 3.4, stone_hi);
    m3::tint_up(&mut surround, stone_hi, stone_lo); surround.mat = M::STONE; s.add(surround, Float::STILL);
    // glowing threshold, pulled a touch toward the region (outward) so it reads as a way through
    let tc = v(wc.x - inward.x * 0.12, y_floor, wc.z - inward.z * 0.12);
    let mut portal = m3::arch_window(tc, right, up, 2.3, 2.8, c(0x7c, 0xe6, 0xff));
    m3::tint_up(&mut portal, c(0xbf, 0xf4, 0xff), c(0x16, 0x44, 0x52)); portal.mat = M::WATER; portal.emissive = 1.4;
    s.add(portal, Float::STILL);
    for sgn in [-1.55f32, 1.55] {
        let (mx, mz) = (wc.x + right.x * sgn, wc.z + right.z * sgn);
        let mut jamb = m3::tube(v(mx, y_floor, mz), v(mx, y_floor + 3.6, mz), 0.16, 12, stone_hi);
        jamb.mat = M::STONE; s.add(jamb, Float::STILL);
        push_collider(Collider::Pillar { x: mx, z: mz, r: 0.28, y0: y_floor, y1: y_floor + 3.6 }); // jamb only — centre is clear
    }
}

/// Dispatch: build the region's space at an explicit `center` (facing `inward` toward the hall).
fn build_region_at(s: &mut Scene, reg: &Region, center: Vec3, inward: Vec3, stone_lo: Color32, stone_hi: Color32) {
    match reg.kind {
        RegionType::Rotunda    => rotunda(s, center, reg.scale, reg.seed, stone_lo, stone_hi),
        RegionType::Stairs     => stair_descent(s, center, inward, reg.scale, reg.seed, stone_lo, stone_hi),
        RegionType::Terrace    => terrace(s, center, reg.scale, reg.seed, stone_lo, stone_hi),
        RegionType::Chamber    => chamber(s, center, inward, reg.scale, reg.seed, stone_lo, stone_hi),
        RegionType::SealedCave => sealed_cave(s, center, reg.scale, reg.seed, stone_lo, stone_hi),
    }
}

/// Dispatch: place the region's centre from its polar coords (circular) and build the right space.
fn build_region(s: &mut Scene, reg: &Region, y_floor: f32, stone_lo: Color32, stone_hi: Color32) {
    let (cx, cz) = (reg.angle.cos() * reg.dist, reg.angle.sin() * reg.dist);
    let center = v(cx, y_floor, cz);
    let inward = normv(v(-cx, 0.0, -cz)); // toward the hall
    build_region_at(s, reg, center, inward, stone_lo, stone_hi);
}

// ── per-region seeded generators ───────────────────────────────────────────────────────────────

/// ROTUNDA (loops): a ring of short columns under a low dome, circling a glowing focus — you walk
/// around it and return the way you came. Each column emits a collider.
fn rotunda(s: &mut Scene, center: Vec3, sc: f32, seed: u32, stone_lo: Color32, stone_hi: Color32) {
    use std::f32::consts::TAU;
    let r = 4.2 * sc; let y = center.y; let y_cap = y + 4.0 * sc;
    let mut floor = m3::disc(v(center.x, y, center.z), r + 1.0, 56, stone_lo);
    m3::tint_up(&mut floor, stone_hi, stone_lo); floor.mat = M::STONE; s.add(floor, Float::STILL);
    let cols = 8;
    for k in 0..cols {
        let a = (k as f32 / cols as f32) * TAU;
        let (px, pz) = (center.x + a.cos() * r, center.z + a.sin() * r);
        let mut shaft = m3::tube(v(px, y, pz), v(px, y_cap, pz), 0.34 * sc, 18, stone_lo);
        m3::tint_height(&mut shaft, y, y_cap, stone_lo, stone_hi); shaft.mat = M::STONE; s.add(shaft, Float::STILL);
        let mut cap = m3::torus(v(px, y_cap, pz), 0.42 * sc, 0.1, 16, 6, stone_hi); cap.mat = M::STONE; s.add(cap, Float::STILL);
        push_collider(Collider::Pillar { x: px, z: pz, r: 0.5 * sc, y0: y, y1: y_cap });
    }
    let mut ring = m3::torus(v(center.x, y_cap, center.z), r, 0.22, 56, 8, stone_hi);
    m3::tint_up(&mut ring, stone_hi, stone_lo); ring.mat = M::STONE; s.add(ring, Float::STILL);
    let mut dome = m3::dome_cap(v(center.x, y_cap, center.z), r + 0.4, 3.2 * sc, r * 0.3, 48, 8, stone_lo);
    m3::tint_height(&mut dome, y_cap, y_cap + 3.2 * sc, stone_lo, stone_hi); dome.mat = M::WATER; dome.emissive = 0.1; s.add(dome, Float::STILL);
    // the loop's focus — a slowly-bobbing glass orb at the centre
    let mut orb = m3::uv_sphere(v(center.x, y + 1.5 * sc, center.z), 0.7 * sc, 20, 16, c(0x6a, 0xe0, 0xf0));
    m3::tint_up(&mut orb, c(0xbf, 0xf4, 0xff), c(0x14, 0x4c, 0x58)); orb.mat = M::WATER; orb.emissive = 0.85;
    s.add(orb, Float { amp: 0.12, speed: 0.5, phase: (seed & 0xFF) as f32, drift: 0.0 });
}

/// STAIRS (deep threads): concentric stone tiers stepping DOWN to a sunken landing with a lit
/// obelisk marker at the bottom (the depth's reward). A central collider guards the marker.
fn stair_descent(s: &mut Scene, center: Vec3, inward: Vec3, sc: f32, seed: u32, stone_lo: Color32, stone_hi: Color32) {
    let steps = 6;
    let r0 = 5.0 * sc;
    for i in 0..steps {
        let t = i as f32 / steps as f32;
        let ri = r0 * (1.0 - t * 0.78);
        let ro = ri + 0.7 * sc;
        let y = center.y - 0.55 * sc * i as f32; // descend
        let mut tier = m3::annulus(v(center.x, y, center.z), ri, ro, 56, stone_lo);
        m3::tint_up(&mut tier, stone_hi, stone_lo); tier.mat = M::STONE; s.add(tier, Float::STILL);
        // a short riser face under each tier
        let mut riser = m3::tube(v(center.x, y - 0.55 * sc, center.z), v(center.x, y, center.z), ri, 48, stone_lo);
        m3::tint_height(&mut riser, y - 0.55 * sc, y, stone_lo, stone_hi); riser.mat = M::STONE; s.add(riser, Float::STILL);
    }
    let y_bot = center.y - 0.55 * sc * steps as f32;
    let mut landing = m3::disc(v(center.x, y_bot, center.z), r0 * 0.24, 40, stone_hi);
    m3::tint_up(&mut landing, stone_hi, stone_lo); landing.mat = M::STONE; s.add(landing, Float::STILL);
    let mut obel = m3::tube(v(center.x, y_bot, center.z), v(center.x, y_bot + 2.6 * sc, center.z), 0.3 * sc, 6, c(0x6a, 0xe0, 0xf0));
    m3::tint_height(&mut obel, y_bot, y_bot + 2.6 * sc, c(0x12, 0x4a, 0x56), c(0xbf, 0xf4, 0xff)); obel.mat = M::WATER; obel.emissive = 0.7;
    s.add(obel, Float::STILL);
    let _ = (inward, seed); // descent is geometric; inward/seed reserved for future orientation/jitter
    push_collider(Collider::Pillar { x: center.x, z: center.z, r: 0.5 * sc, y0: y_bot, y1: y_bot + 2.6 * sc });
}

/// A straight FLIGHT OF STEPS descending a hall along the radial ray (unit dir dx,dz, z scaled by `sz` for the
/// ellipse), from radius r0 at height y0 out to radius r1 at height y1, `width` wide. Tread + riser faces plus a
/// threshold sill at the top and a landing slab at the foot — the architectural joints between hall and wing.
fn hall_steps(s: &mut Scene, dx: f32, dz: f32, r0: f32, r1: f32, y0: f32, y1: f32, sz: f32, width: f32, lo: Color32, hi: Color32) {
    let n = (((y0 - y1).abs() / 0.42).ceil() as usize).clamp(2, 10);
    let right = normv(v(-dz, 0.0, dx));
    let outward = v(dx, 0.0, dz);
    let (run, dy) = ((r1 - r0) / n as f32, (y1 - y0) / n as f32);
    let pt = |rr: f32, y: f32| v(dx * rr, y, dz * rr * sz);
    for i in 0..n {
        let (ra, rb) = (r0 + run * i as f32, r0 + run * (i as f32 + 1.0));
        let (ya, yb) = (y0 + dy * i as f32, y0 + dy * (i as f32 + 1.0));
        // tread: horizontal slab at the step's lower edge, centred over ra..rb
        let mut tread = m3::panel(pt((ra + rb) * 0.5, yb), right, outward, width, run.abs() * 1.04, lo);
        m3::tint_up(&mut tread, hi, lo); tread.mat = M::STONE; s.add(tread, Float::STILL);
        // riser: vertical face at the step front, from the upper to the lower height
        let mut riser = m3::panel(pt(ra, (ya + yb) * 0.5), right, v(0.0, 1.0, 0.0), width, (ya - yb).abs() + 0.04, lo);
        m3::tint_height(&mut riser, yb, ya, lo, hi); riser.mat = M::STONE; s.add(riser, Float::STILL);
    }
    // architectural joints: a threshold sill flush with the hall, a landing slab where it meets the wing
    let mut sill = m3::panel(pt(r0, y0 + 0.02), right, outward, width + 0.6, 0.7, hi);
    m3::tint_up(&mut sill, hi, lo); sill.mat = M::STONE; s.add(sill, Float::STILL);
    let mut land = m3::panel(pt(r1, y1 + 0.02), right, outward, width + 1.0, 1.4, hi);
    m3::tint_up(&mut land, hi, lo); land.mat = M::STONE; s.add(land, Float::STILL);
}

/// TERRACE (long tangents): an open platform with a low balustrade, a sea-garden, and a marble
/// statue landmark — the conversation's wandering opens out into a garden. Reuses sea_garden +
/// poseidon_statue. The statue carries a collider; the soft vegetation does not.
fn terrace(s: &mut Scene, center: Vec3, sc: f32, seed: u32, stone_lo: Color32, stone_hi: Color32) {
    use std::f32::consts::TAU;
    let r = 5.0 * sc; let y = center.y;
    let mut plat = m3::disc(v(center.x, y, center.z), r, 64, stone_lo);
    m3::tint_up(&mut plat, stone_hi, stone_lo); plat.mat = M::STONE; s.add(plat, Float::STILL);
    let mut rail = m3::torus(v(center.x, y + 0.7, center.z), r - 0.2, 0.06, 64, 6, stone_hi);
    m3::tint_up(&mut rail, stone_hi, stone_lo); rail.mat = M::STONE; s.add(rail, Float::STILL);
    let balus = 24;
    for k in 0..balus {
        let a = (k as f32 / balus as f32) * TAU;
        let (bx, bz) = (center.x + a.cos() * (r - 0.2), center.z + a.sin() * (r - 0.2));
        let mut bb = m3::tube(v(bx, y, bz), v(bx, y + 0.7, bz), 0.04, 5, stone_hi); bb.mat = M::STONE; s.add(bb, Float::STILL);
    }
    // the terrace OVERLOOKS a sea garden — kelp/coral sit in the water beyond the rim and a step below,
    // not on the (air-side) platform itself.
    {
        let rim = seed.wrapping_mul(0x9E37_79B1);
        let frnd = |k: u32| { let mut h = rim.wrapping_add(k.wrapping_mul(0x85EB_CA77)); h ^= h >> 15; (h & 0xFFFF) as f32 / 65535.0 };
        for k in 0..3u32 {
            let a = (k as f32 + 0.5) / 3.0 * TAU + frnd(k) * 0.8;
            let gr = r * (1.25 + 0.25 * frnd(k + 9));
            sea_garden(s, v(center.x + a.cos() * gr, y - 0.6, center.z + a.sin() * gr), 1.8 * sc, seed ^ 0x5EA0 ^ k.wrapping_mul(0x68E3));
        }
    }
    // a statue facing the hall as the wing's landmark
    let face = (-center.z).atan2(-center.x);
    poseidon_statue(s, v(center.x + 0.4, y, center.z + 0.4), face, sc, seed ^ 0x9E37);
    push_collider(Collider::Pillar { x: center.x + 0.4, z: center.z + 0.4, r: 1.0 * sc, y0: y, y1: y + 5.0 * sc });
}

/// CHAMBER (token-heavy): a larger enclosed room — a partial wall (open toward the hall via a gap
/// facing `inward`), inner columns, and a central plinth + glowing crystal holding the heavy
/// content. Columns + plinth emit colliders.
fn chamber(s: &mut Scene, center: Vec3, inward: Vec3, sc: f32, seed: u32, stone_lo: Color32, stone_hi: Color32) {
    use std::f32::consts::TAU;
    let r = 6.0 * sc; let y = center.y; let h = 6.5 * sc;
    let mut floor = m3::disc(v(center.x, y, center.z), r, 64, stone_lo);
    m3::tint_up(&mut floor, stone_hi, stone_lo); floor.mat = M::STONE; s.add(floor, Float::STILL);
    // wall with a gap facing the hall (so the door reads through into the room)
    let gap_ang = inward.z.atan2(inward.x);
    let mut wall = m3::wall_ring_gapped(v(center.x, y, center.z), r, h, 72, stone_lo, &[(gap_ang, 0.5)]);
    m3::tint_height(&mut wall, y, y + h, stone_lo, stone_hi); wall.mat = M::STONE; s.add(wall, Float::STILL);
    let mut dome = m3::dome_cap(v(center.x, y + h, center.z), r, 4.0 * sc, r * 0.22, 56, 8, stone_lo);
    m3::tint_height(&mut dome, y + h, y + h + 4.0 * sc, stone_lo, stone_hi); dome.mat = M::WATER; dome.emissive = 0.12; s.add(dome, Float::STILL);
    let inner = 4;
    for k in 0..inner {
        let a = (k as f32 / inner as f32) * TAU + 0.4;
        let (px, pz) = (center.x + a.cos() * (r * 0.6), center.z + a.sin() * (r * 0.6));
        let mut shaft = m3::tube(v(px, y, pz), v(px, y + h - 0.6, pz), 0.4 * sc, 22, stone_lo);
        m3::tint_height(&mut shaft, y, y + h, stone_lo, stone_hi); shaft.mat = M::STONE; s.add(shaft, Float::STILL);
        push_collider(Collider::Pillar { x: px, z: pz, r: 0.55 * sc, y0: y, y1: y + h });
    }
    let mut plinth = m3::tube(v(center.x, y, center.z), v(center.x, y + 1.2 * sc, center.z), 1.3 * sc, 28, stone_lo);
    m3::tint_height(&mut plinth, y, y + 1.2 * sc, stone_lo, stone_hi); plinth.mat = M::STONE; s.add(plinth, Float::STILL);
    let mut crys = m3::rock(v(center.x, y + 2.2 * sc, center.z), 1.0 * sc, seed.wrapping_add(7), c(0x2a, 0x8a, 0x96));
    m3::tint_up(&mut crys, c(0x6a, 0xe0, 0xf0), c(0x10, 0x42, 0x4c)); crys.mat = M::WATER; crys.emissive = 0.4;
    s.add(crys, Float { amp: 0.08, speed: 0.5, phase: (seed & 0x7F) as f32, drift: 0.0 });
    push_collider(Collider::Pillar { x: center.x, z: center.z, r: 1.5 * sc, y0: y, y1: y + 3.4 * sc });
}

/// SEALED CAVE (pruned / dead ends): a blocked, UNLIT mouth — a clump of rocks and a dark sealed
/// arch (no glowing threshold). It reads as "no way through". One collider bounds the rubble.
fn sealed_cave(s: &mut Scene, center: Vec3, sc: f32, seed: u32, stone_lo: Color32, stone_hi: Color32) {
    let y = center.y;
    let dark = c(0x0a, 0x18, 0x1e);
    // a sealed (unlit) arch facing the hall
    let right = normv(crossv(v(0.0, 1.0, 0.0), normv(v(center.x, 0.0, center.z))));
    let mut arch = m3::arch_window(v(center.x, y, center.z), right, v(0.0, 1.0, 0.0), 2.6, 2.8, dark);
    m3::tint_up(&mut arch, stone_lo, dark); arch.mat = M::STONE; s.add(arch, Float::STILL);
    // rubble blocking it
    for k in 0..5u32 {
        let rr = 0.6 + (seed.wrapping_mul(k + 1) % 100) as f32 / 100.0 * 0.7;
        let off = (k as f32 - 2.0) * 0.7 * sc;
        let mut rk = m3::rock(v(center.x + right.x * off, y + 0.4 + 0.2 * (k % 3) as f32, center.z + right.z * off), rr * sc, 900 + seed.wrapping_add(k), stone_lo);
        m3::tint_up(&mut rk, stone_hi, dark); rk.mat = M::STONE; s.add(rk, Float::STILL);
    }
    push_collider(Collider::Pillar { x: center.x, z: center.z, r: 2.0 * sc, y0: y, y1: y + 3.0 });
    let _ = stone_hi;
}

/// HALLWAY — a straight colonnaded corridor extending `length` along `dir` from `start`, `width` wide. Two
/// rows of columns flank a (water) floor under a glass roof, tied by side lintels and capped by an end
/// archway; the sides stay open so the ocean shows between the columns. Carries a LINEAR run of the graph
/// out of the rotunda. Emits a Pillar collider per column so the fly evades them.
fn hallway(s: &mut Scene, start: Vec3, dir: Vec3, length: f32, width: f32, y_floor: f32, y_cap: f32, seed: u32, stone_lo: Color32, stone_hi: Color32) {
    let dir = normv(dir);
    let right = normv(crossv(v(0.0, 1.0, 0.0), dir));
    let up = v(0.0, 1.0, 0.0);
    let bays = ((length / 3.6).round() as usize).max(2);
    let step = length / bays as f32;
    let mid = addv(start, mulv(dir, length * 0.5));
    // water floor causeway + glass roof
    let mut floor = m3::panel(v(mid.x, y_floor + 0.02, mid.z), right, dir, width, length, c(0x06, 0x14, 0x1f));
    m3::tint_up(&mut floor, c(0x0e, 0x28, 0x34), c(0x03, 0x0c, 0x14)); floor.mat = M::WATER; s.add(floor, Float::STILL);
    let mut roof = m3::panel(v(mid.x, y_cap + 0.4, mid.z), right, dir, width + 0.6, length, c(0x10, 0x4a, 0x58));
    m3::tint_up(&mut roof, c(0x2a, 0x74, 0x86), c(0x08, 0x30, 0x3c)); roof.mat = M::WATER; roof.emissive = 0.07; s.add(roof, Float::STILL);
    // two rows of columns + side lintels
    let mut tops: [Vec<Vec3>; 2] = [Vec::new(), Vec::new()];
    for i in 0..=bays {
        let d = i as f32 * step;
        let base = addv(start, mulv(dir, d));
        for (si, sgn) in [-1.0f32, 1.0].iter().enumerate() {
            let p = addv(base, mulv(right, sgn * width * 0.5));
            let mut shaft = m3::tube(v(p.x, y_floor, p.z), v(p.x, y_cap, p.z), 0.42, 20, stone_lo);
            m3::tint_height(&mut shaft, y_floor, y_cap, stone_lo, stone_hi); shaft.mat = M::STONE; s.add(shaft, Float::STILL);
            let mut cap = m3::torus(v(p.x, y_cap - 0.2, p.z), 0.52, 0.12, 18, 6, stone_hi); cap.mat = M::STONE; s.add(cap, Float::STILL);
            let mut foot = m3::tube(v(p.x, y_floor, p.z), v(p.x, y_floor + 0.4, p.z), 0.6, 16, stone_hi); foot.mat = M::STONE; s.add(foot, Float::STILL);
            push_collider(Collider::Pillar { x: p.x, z: p.z, r: 0.7, y0: y_floor, y1: y_cap });
            tops[si].push(v(p.x, y_cap, p.z));
        }
    }
    for row in &tops {
        for w in row.windows(2) {
            let mut lint = m3::tube(w[0], w[1], 0.12, 6, stone_hi);
            m3::tint_up(&mut lint, stone_hi, stone_lo); lint.mat = M::STONE; s.add(lint, Float::STILL);
        }
    }
    // end ARCHWAY framing the continuation
    let endp = addv(start, mulv(dir, length));
    let mut arch = m3::arch_window(v(endp.x, y_floor, endp.z), right, up, width, y_cap - y_floor + 0.6, stone_hi);
    m3::tint_up(&mut arch, stone_hi, stone_lo); arch.mat = M::STONE; s.add(arch, Float::STILL);
    // LIBRARY STACKS line the hall — low bookcases between the columns (kept short so the upper colonnade
    // stays open to the ocean above), so travelling down a hall feels like walking the stacks.
    let (wood_lo, wood_hi) = (c(0x2c, 0x1c, 0x10), c(0x5a, 0x3c, 0x20));
    let sh_h = (y_cap - y_floor) * 0.6;
    for i in 0..bays {
        let d = (i as f32 + 0.5) * step;
        let base = addv(start, mulv(dir, d));
        for sgn in [-1.0f32, 1.0] {
            let p = addv(base, mulv(right, sgn * (width * 0.5 - 0.35)));
            bookcase(s, v(p.x, y_floor, p.z), mulv(right, -sgn), dir, step * 0.82, sh_h, seed ^ (i as u32).wrapping_mul(0x51A3) ^ ((sgn > 0.0) as u32).wrapping_mul(0x9E1), wood_lo, wood_hi);
        }
    }
    // SOFT END-CAP: a virtual pillar just past the archway so the fly stops at the continuation instead of
    // sailing off into open ocean (the camera evades it and holds at the hall's end).
    let cap_r = width * 0.9;
    let capc = addv(endp, mulv(dir, cap_r + 0.4));
    push_collider(Collider::Pillar { x: capc.x, z: capc.z, r: cap_r, y0: y_floor - 2.0, y1: y_cap + 4.0 });
    // ARCHED WINDOWS glaze the upper band of each bay (above the stacks, below the lintels) so the hall reads
    // as a gallery looking out into the ocean — stained glass on alternating bays, clear aqua between.
    let glass_pal = [c(0x2a, 0x6e, 0xc4), c(0x2c, 0xa0, 0xb0), c(0x8a, 0x4c, 0xb0), c(0xc8, 0x9a, 0x3a), c(0x3a, 0xb0, 0x7a)];
    let (y0w, y1w) = (y_floor + sh_h + 0.15, y_cap - 0.55);
    let (wh, wy) = ((y1w - y0w).max(0.7), (y0w + y1w) * 0.5);
    for i in 0..bays {
        let bc = addv(start, mulv(dir, (i as f32 + 0.5) * step));
        for sgn in [-1.0f32, 1.0] {
            let p = addv(bc, mulv(right, sgn * (width * 0.5 - 0.06)));
            let (cen, ww) = (v(p.x, wy, p.z), step * 0.72);
            if i % 2 == 0 {
                for mut g in m3::stained_glass(cen, dir, up, ww, wh, 3, 4, &glass_pal, seed ^ (i as u32).wrapping_mul(0x77) ^ (sgn > 0.0) as u32) { g.emissive = 0.38; s.add(g, Float::STILL); }
            } else {
                let mut pane = m3::panel(cen, dir, up, ww, wh, c(0x36, 0x8e, 0xa6)); pane.mat = M::WATER; pane.emissive = 0.22; s.add(pane, Float::STILL);
            }
            let mut fr = m3::arch_window(v(p.x, y0w, p.z), dir, up, ww + 0.22, wh + 0.45, stone_hi);
            m3::tint_up(&mut fr, stone_hi, stone_lo); fr.mat = M::STONE; s.add(fr, Float::STILL);
        }
    }
    let _ = seed;
}

/// A hanging heraldic BANNER that waves: a vertical stack of cloth segments, each swaying a touch more than the
/// one above (phase-staggered) so a ripple travels down the free end. `ang` faces it inward; hung from `y_top`.
fn banner(s: &mut Scene, x: f32, z: f32, ang: f32, y_top: f32, h: f32, col: Color32, seed: u32) {
    let wdir = v(-ang.sin(), 0.0, ang.cos()); // width axis (tangent to the colonnade)
    let face = v(ang.cos(), 0.0, ang.sin());  // inward normal
    let segs = 5usize;
    let seg_h = h / segs as f32;
    // crossbar the banner hangs from
    let mut rod = m3::tube(addv(v(x, y_top, z), mulv(wdir, -0.55)), addv(v(x, y_top, z), mulv(wdir, 0.55)), 0.04, 6, c(0x6e, 0x52, 0x28));
    rod.mat = M::WOOD; s.add(rod, Float::STILL);
    for i in 0..segs {
        let yc = y_top - seg_h * (i as f32 + 0.5);
        let t = i as f32 / (segs - 1) as f32;
        let band = if i % 2 == 0 { col } else { c(((col.r() as u16 + 0xf0) / 2) as u8, ((col.g() as u16 + 0xd8) / 2) as u8, ((col.b() as u16 + 0x8c) / 2) as u8) }; // gold stripe
        let mut seg = m3::panel(v(x, yc, z), wdir, v(0.0, 1.0, 0.0), 0.9 - 0.06 * t, seg_h * 1.04, band);
        seg.mat = M::PAPER; // flat matte cloth
        let sway = Float { amp: 0.0, speed: 0.8, phase: hash(seed as i32, 1) * 6.28 + i as f32 * 0.7, drift: 0.05 + 0.16 * t };
        s.add(seg, sway);
        let _ = face;
    }
}

fn atlantis() -> Scene {
    use std::f32::consts::TAU;
    let pr = env_params();
    let mut s = Scene::new();
    // PROCEDURAL: the hall is generated FROM the conversation's shape. It is sized AND shaped to contain the
    // flow — an OVAL ROTUNDA elongated along the spine (x). Density/tint also follow the conversation.
    let hh = |k: u32| -> f32 { let mut h = pr.seed ^ k.wrapping_mul(0x9E37_79B9); h ^= h >> 15; h = h.wrapping_mul(0x2C1B_3C6D); h ^= h >> 13; (h & 0xFFFF) as f32 / 65535.0 };
    let lerp_c = |a: (u8, u8, u8), b: (u8, u8, u8), f: f32| c(
        (a.0 as f32 + (b.0 as f32 - a.0 as f32) * f) as u8,
        (a.1 as f32 + (b.1 as f32 - a.1 as f32) * f) as u8,
        (a.2 as f32 + (b.2 as f32 - a.2 as f32) * f) as u8);
    let warm = hh(1); // 0..1: cool teal ←→ kelp-green bias for THIS conversation
    // STONE FAMILY varies per conversation (hh(2)): basalt / sandstone / limestone / slate — each with its own
    // sea-floor sand+rock tones. Env variation so two different logs don't read as the same building.
    let rock_kind = (hh(2) * 4.0) as u32 % 4;
    let (stone_lo, stone_hi, sand_c, rock_c) = match rock_kind {
        0 => (lerp_c((0x0c, 0x28, 0x30), (0x14, 0x2e, 0x22), warm), lerp_c((0x1b, 0x55, 0x60), (0x2c, 0x60, 0x40), warm), c(0x39, 0x45, 0x40), c(0x27, 0x4e, 0x55)), // teal basalt
        1 => (lerp_c((0x46, 0x38, 0x24), (0x4e, 0x3e, 0x22), warm), lerp_c((0x86, 0x6c, 0x42), (0x90, 0x76, 0x40), warm), c(0x7a, 0x66, 0x40), c(0x5c, 0x46, 0x2c)), // sandstone
        2 => (lerp_c((0x48, 0x4c, 0x50), (0x4c, 0x50, 0x4a), warm), lerp_c((0x96, 0x9a, 0x98), (0x9e, 0xa0, 0x92), warm), c(0x84, 0x82, 0x74), c(0x5e, 0x62, 0x64)), // limestone
        _ => (lerp_c((0x20, 0x28, 0x38), (0x24, 0x2c, 0x36), warm), lerp_c((0x44, 0x50, 0x66), (0x48, 0x52, 0x60), warm), c(0x3c, 0x42, 0x50), c(0x28, 0x30, 0x42)), // slate
    };
    let glass_lo = lerp_c((0x05, 0x2e, 0x38), (0x07, 0x33, 0x2a), warm);
    let glass_hi = lerp_c((0x12, 0x6a, 0x78), (0x20, 0x74, 0x55), warm);

    // COMPACT DOMED ROTUNDA (the heart): sized by node count, not the full spine. The conversation's linear
    // CONTINUATION extends OUT of the rotunda into HALLWAYS (built below) along the spine axis (x) — so the
    // graph reads as "rotunda + corridors", and the rotunda never balloons to a giant empty oval.
    let r_hall = (13.0 + 1.0 * (pr.nodes.max(1) as f32).sqrt()).clamp(15.0, 26.0);
    let sz = 1.0_f32;                          // circular rotunda; hallways carry the long axis
    let ez = |z: f32| z * sz;
    let y_floor = -3.2_f32;
    let y_cap = (5.6 + 0.10 * r_hall).clamp(6.2, 8.6);
    let cols = (9 + pr.segments as usize * 2).clamp(12, 34);
    let lanterns = (6 + pr.branches as usize).clamp(8, 22);
    // how far the spine reaches past the rotunda → hallway length each way (0 if the spine fits inside)
    let spine_reach = (pr.ext_x.max(pr.ext_z)) as f32;
    let hall_len = (spine_reach - (r_hall - 2.0)).max(0.0);
    let hallways = hall_len > 3.0;
    let hall_w = (2.0 * pr.ext_z as f32 + 4.0).clamp(7.0, 15.0);
    // hallway mouths are on the x-axis (angles 0 and π); leave gaps there in the colonnade + perimeter
    let near_axis = |a: f32| { let d = (a % std::f32::consts::PI).min(std::f32::consts::PI - (a % std::f32::consts::PI)); hallways && d < (hall_w * 0.6 / r_hall) };

    // wide blue-obsidian floor — wet sheen, faint mirror
    let mut floor = m3::disc(v(0.0, y_floor, 0.0), r_hall + 2.0, 96, c(0x06, 0x14, 0x1f));
    m3::tint_up(&mut floor, c(0x0e, 0x28, 0x34), c(0x03, 0x0c, 0x14)); floor.mat = M::WATER;
    m3::scale_xz(&mut floor, 1.0, sz); s.add(floor, Float::STILL);
    // FLOOR MOSAIC — concentric inlaid rings + a central medallion + a ring of tesserae, low-emissive so it
    // reads as polished inlay catching the caustics (procedural, sized to the floor ellipse).
    {
        let mpal = [c(0x1a, 0x6e, 0x8a), c(0x9a, 0x7a, 0x2e), c(0x6a, 0x2e, 0x5a), c(0x2a, 0x8a, 0x6a), c(0x24, 0x4c, 0x86)];
        let r_mos = r_hall - 1.6;
        let rings = 7usize;
        for k in 0..rings {
            let (r0, r1) = (r_mos * k as f32 / rings as f32, r_mos * (k as f32 + 1.0) / rings as f32);
            let mut ring = m3::annulus(v(0.0, y_floor + 0.03, 0.0), r0, r1, 84, mpal[k % mpal.len()]);
            ring.mat = M::WATER; ring.emissive = 0.05 + 0.05 * (k % 2) as f32;
            m3::scale_xz(&mut ring, 1.0, sz); s.add(ring, Float::STILL);
        }
        let mut med = m3::disc(v(0.0, y_floor + 0.05, 0.0), r_mos * 0.15, 36, c(0xe8, 0xd8, 0x9a));
        med.mat = M::WATER; med.emissive = 0.14; m3::scale_xz(&mut med, 1.0, sz); s.add(med, Float::STILL);
        let mut med_ring = m3::annulus(v(0.0, y_floor + 0.045, 0.0), r_mos * 0.15, r_mos * 0.19, 36, c(0x24, 0x4c, 0x86));
        med_ring.mat = M::STONE; med_ring.emissive = 0.06; m3::scale_xz(&mut med_ring, 1.0, sz); s.add(med_ring, Float::STILL);
        // a ring of tesserae (small inlay tiles) at mid-radius
        let (rmid, tess) = (r_mos * 0.62, 40usize);
        for k in 0..tess {
            let a = k as f32 / tess as f32 * TAU;
            let mut t = m3::disc(v(a.cos() * rmid, y_floor + 0.06, a.sin() * rmid * sz), r_mos * 0.03, 10, mpal[k % mpal.len()]);
            t.mat = M::WATER; t.emissive = 0.10; s.add(t, Float::STILL);
        }
    }

    // STONE colonnade — a ring of columns carrying the dome (the hall's bones), OPEN at the hallway mouths
    let r_col = r_hall - 1.2;
    for k in 0..cols {
        let a = (k as f32 / cols as f32) * TAU;
        if near_axis(a) { continue; }          // gap the colonnade where a hallway joins
        let (cx, cz) = (a.cos() * r_col, ez(a.sin() * r_col));
        let mut shaft = m3::tube(v(cx, y_floor, cz), v(cx, y_cap - 0.3, cz), 0.5, 26, stone_lo);
        m3::tint_height(&mut shaft, y_floor, y_cap, stone_lo, stone_hi); shaft.mat = M::STONE; s.add(shaft, Float::STILL);
        let mut base = m3::tube(v(cx, y_floor, cz), v(cx, y_floor + 0.5, cz), 0.72, 22, stone_hi); base.mat = M::STONE; s.add(base, Float::STILL);
        let mut base_top = m3::disc(v(cx, y_floor + 0.5, cz), 0.72, 22, stone_hi); base_top.mat = M::STONE; s.add(base_top, Float::STILL);
        let mut cap = m3::tube(v(cx, y_cap - 0.5, cz), v(cx, y_cap, cz), 0.72, 22, stone_hi); cap.mat = M::STONE; s.add(cap, Float::STILL);
        let mut cap_top = m3::disc(v(cx, y_cap, cz), 0.72, 22, stone_hi); cap_top.mat = M::STONE; s.add(cap_top, Float::STILL);
        let mut collar0 = m3::torus(v(cx, y_floor + 0.5, cz), 0.6, 0.12, 22, 8, stone_hi); collar0.mat = M::STONE; s.add(collar0, Float::STILL);
        let mut collar1 = m3::torus(v(cx, y_cap - 0.5, cz), 0.6, 0.12, 22, 8, stone_hi); collar1.mat = M::STONE; s.add(collar1, Float::STILL);
        push_collider(Collider::Pillar { x: cx, z: cz, r: 0.8, y0: y_floor, y1: y_cap });
        contact_shadow(&mut s, cx, cz, 1.25, y_floor, 130); // grounding shadow at the column foot
    }
    // stone architrave ring tying the columns together (elliptical)
    let mut ring = m3::torus(v(0.0, y_cap, 0.0), r_col, 0.34, 96, 10, stone_hi);
    m3::tint_up(&mut ring, stone_hi, stone_lo); ring.mat = M::STONE; m3::scale_xz(&mut ring, 1.0, sz); s.add(ring, Float::STILL);

    // WAVING BANNERS hung from the architrave, in the bays between columns
    let ban_pal = [c(0x1c, 0x52, 0xb0), c(0x14, 0x6e, 0x88), c(0x6a, 0x2e, 0x5a), c(0x2a, 0x8a, 0x6a)];
    for k in 0..cols {
        if k % 2 == 1 { continue; }
        let a = (k as f32 / cols as f32) * TAU + (TAU / cols as f32) * 0.5; // mid-bay, between two columns
        if near_axis(a) { continue; }
        let (bx, bz) = (a.cos() * (r_col - 0.35), ez(a.sin() * (r_col - 0.35)));
        banner(&mut s, bx, bz, a + std::f32::consts::PI, y_cap - 0.25, 3.4, ban_pal[(k / 2) % ban_pal.len()], (k * 13 + 1) as u32);
    }

    // UPPER BALCONY (the reference's railed gallery): a walkway ring at the architrave with a low railing on
    // its INNER edge (high up — it rings the perimeter, so it doesn't cage the central graph below), and ABOVE
    // it a tall arcade of BIG arched windows that the dome springs from. Most arches are LARGE STAINED GLASS
    // covering the upper drum; a few are clear to the ocean. Sizes + cadence follow the conversation.
    let y_bal = y_cap + 0.15;
    let mut walk = m3::annulus(v(0.0, y_bal, 0.0), r_col - 1.8, r_col - 0.2, 96, stone_lo);
    m3::tint_up(&mut walk, stone_hi, stone_lo); walk.mat = M::STONE; m3::scale_xz(&mut walk, 1.0, sz); s.add(walk, Float::STILL);
    // (removed: the inner balcony RAIL torus + its balusters — that mid-height ring read as cutting across
    //  the central open volume. The flat walkway annulus above stays; the rim is left open over the centre.)

    // tall arcade drum the dome springs from, pierced by LARGE windows (mostly stained glass)
    let y_cler0 = y_bal + 0.7; let y_cler1 = y_cler0 + (5.6 + 1.0 * hh(2)); // taller drum → bigger windows
    let arches = (cols / 2).clamp(6, 16);
    let win_w = 4.6 + 1.4 * hh(3);                       // LARGER windows
    let win_h = (y_cler1 - y_cler0) - 0.7;               // fill most of the drum
    let facet_x = (1 + pr.segments as usize).clamp(3, 5);
    let facet_y = (3 + pr.branches as usize).clamp(4, 7);
    let blue_pal = [c(0x10, 0x2c, 0x66), c(0x1c, 0x52, 0xb0), c(0x2f, 0x8f, 0xd8), c(0x6a, 0xc8, 0xf0), c(0x14, 0x6e, 0x88), c(0xd8, 0xa8, 0x40)];
    let gold_pal = [c(0x6e, 0x4e, 0x12), c(0xb8, 0x86, 0x22), c(0xf0, 0xc8, 0x5a), c(0xf6, 0xe0, 0xa0), c(0xc0, 0x70, 0x2a), c(0x2f, 0x8f, 0xd8)];
    // mostly STAINED now (user: larger stained glass covering more of the balcony); clear only every 3rd → ocean peeks
    let is_clear = |k: usize| k % 3 == 0;
    let drum_half = (win_w * 0.6) / r_col;
    let mut gaps: Vec<(f32, f32)> = Vec::new();
    for k in 0..arches {
        let a = (k as f32 / arches as f32) * TAU + std::f32::consts::PI / arches as f32;
        if near_axis(a) || is_clear(k) { gaps.push((a, drum_half)); } // open behind clear panes + at hallway mouths
    }
    let mut cler = m3::wall_ring_gapped(v(0.0, y_cler0, 0.0), r_col + 0.3, y_cler1 - y_cler0, 96, stone_lo, &gaps);
    m3::tint_height(&mut cler, y_cler0, y_cler1, stone_lo, stone_hi); cler.mat = M::STONE; m3::scale_xz(&mut cler, 1.0, sz); s.add(cler, Float::STILL);
    for k in 0..arches {
        let a = (k as f32 / arches as f32) * TAU + std::f32::consts::PI / arches as f32;
        if near_axis(a) { continue; } // the dome opens to the hallway here
        let rw = r_col + 0.16;
        let (wx, wz) = (a.cos() * rw, ez(a.sin() * rw));
        let right = v(-a.sin(), 0.0, a.cos());
        let up = v(0.0, 1.0, 0.0);
        if is_clear(k) {
            let mut pane = m3::arch_window(v(wx, y_cler0 + 0.45, wz), right, up, win_w, win_h, c(0x9a, 0xe8, 0xff));
            m3::tint_up(&mut pane, c(0xcf, 0xf6, 0xff), c(0x2a, 0x6a, 0x80)); pane.mat = M::WATER; pane.emissive = 0.06;
            m3::set_alpha(&mut pane, 120); // clear glass — the ocean reads through it
            s.add(pane, Float::STILL);
            for mfrac in [-0.34f32, 0.34] {
                let (mx, mz) = (wx + right.x * win_w * mfrac, wz + ez(right.z * win_w * mfrac));
                let mut mull = m3::tube(v(mx, y_cler0 + 0.4, mz), v(mx, y_cler0 + 0.4 + win_h, mz), 0.06, 6, stone_hi);
                mull.mat = M::STONE; s.add(mull, Float::STILL);
            }
        } else {
            // LARGER leaded mosaic — wider + much taller arch, covering most of the bay
            let pal: &[egui::Color32] = if (k / 1) % 2 == 0 { &blue_pal } else { &gold_pal };
            for mut m in m3::stained_glass(v(wx, y_cler0 + 0.45, wz), right, up, win_w * 0.96, win_h * 0.62, facet_x, facet_y, pal, pr.seed ^ (k as u32).wrapping_mul(0x68E3)) {
                m3::set_alpha(&mut m, 175); // leaded glass — translucent, lit from the ocean behind
                s.add(m, Float::STILL);
            }
        }
    }
    // UPPER-WALL DETAIL behind the balcony: PILASTERS (engaged columns) between the windows + a moulded
    // CORNICE band at the drum's base and springing line, so the clerestory isn't a blank ring of stone.
    for k in 0..arches {
        let a = (k as f32 / arches as f32) * TAU; // on the column lines (between window centres)
        if near_axis(a) { continue; }
        let (px, pz) = (a.cos() * (r_col + 0.5), ez(a.sin() * (r_col + 0.5)));
        let mut pil = m3::tube(v(px, y_cler0, pz), v(px, y_cler1 - 0.3, pz), 0.28, 12, stone_hi);
        m3::tint_height(&mut pil, y_cler0, y_cler1, stone_lo, stone_hi); pil.mat = M::STONE; s.add(pil, Float::STILL);
        let mut cap = m3::torus(v(px, y_cler1 - 0.3, pz), 0.34, 0.1, 12, 6, stone_hi); cap.mat = M::STONE; s.add(cap, Float::STILL);
    }
    for yb in [y_cler0 - 0.05, y_cler1 - 0.15] {
        let mut corn = m3::torus(v(0.0, yb, 0.0), r_col + 0.42, 0.18, 96, 8, stone_hi);
        m3::tint_up(&mut corn, stone_hi, stone_lo); corn.mat = M::STONE; m3::scale_xz(&mut corn, 1.0, sz); s.add(corn, Float::STILL);
    }
    // DOORS → REGIONS. Each BRANCH plans a side-wing (`region_plan`) whose space type comes from the branch's
    // graph shape; the perimeter wall opens at that bearing and the wing is generated BEYOND it (all placed on
    // the ellipse). Fly-through (gapping the perimeter Bound) is still the next step in chat.rs.
    let doors = (pr.branches as usize).clamp(1, 6);
    let r_perim = r_hall + 2.5;
    let regions = region_plan(&pr, doors, r_hall);
    // terrain sampler (matches the sea floor built below) — used to seat each wing on the dipping sea bed
    let terr = |x: f32, z: f32| seafloor_h(x, z, r_perim + 1.0, y_floor - 1.0, pr.seed ^ 0x7E44_A1);
    for reg in &regions {
        let a = reg.angle;
        let (wx, wz) = (a.cos() * r_perim, ez(a.sin() * r_perim));
        let right = v(-a.sin(), 0.0, a.cos());
        let up = v(0.0, 1.0, 0.0);
        let inward = normv(v(-wx, 0.0, -wz));
        region_door(&mut s, v(wx, y_floor, wz), right, up, inward, y_floor, stone_lo, stone_hi);
        // wing centre on the ray past the door (compressed onto the ellipse); faces the hall centre
        let (cx, cz) = (a.cos() * reg.dist, ez(a.sin() * reg.dist));
        // TERRAIN-CONFORMING HALL: seat the wing on the sea bed and STEP DOWN to it from the door, instead of
        // floating the wing at the flat interior height over a dipping floor.
        let y_reg = terr(cx, cz).min(y_floor).max(y_floor - 7.0);
        if y_floor - y_reg > 0.5 {
            hall_steps(&mut s, a.cos(), a.sin(), r_perim, (reg.dist - 2.0).max(r_perim + 1.0), y_floor, y_reg, sz, 3.2, stone_lo, stone_hi);
        }
        build_region_at(&mut s, reg, v(cx, y_reg, cz), normv(v(-cx, 0.0, -cz)), stone_lo, stone_hi);
    }

    // RIBBED GLASS DOME (reference-adapted): a transparent glass cap crossed by radial meridian RIBS and a
    // few latitudinal RINGS, crowned by an oculus + bright skylight. Ocean & fish read through the glass.
    let y_dome = y_cler1;
    let rise = 11.0 + 0.2 * r_hall;
    let r_oc = r_hall * 0.18;
    let mut gdome = m3::dome_cap(v(0.0, y_dome, 0.0), r_hall, rise, r_oc, 110, 12, glass_lo);
    m3::tint_height(&mut gdome, y_dome, y_dome + rise, glass_lo, glass_hi); gdome.mat = M::WATER; gdome.emissive = 0.09; m3::scale_xz(&mut gdome, 1.0, sz); m3::set_alpha(&mut gdome, 110); s.add(gdome, Float::STILL);
    // dome profile: radius(φ)=r·cosφ at height y_dome+rise·sinφ — ribs follow it, rings sit at fixed φ
    let dome_pt = |a: f32, phi: f32| { let r = (r_hall * phi.cos()).max(r_oc); v(a.cos() * r, y_dome + rise * phi.sin(), ez(a.sin() * r)) };
    let ribs = (cols).clamp(12, 28);
    for k in 0..ribs {
        let a = (k as f32 / ribs as f32) * TAU;
        let segs = 7;
        for sgmt in 0..segs {
            let p0 = dome_pt(a, std::f32::consts::FRAC_PI_2 * sgmt as f32 / segs as f32);
            let p1 = dome_pt(a, std::f32::consts::FRAC_PI_2 * (sgmt + 1) as f32 / segs as f32);
            let mut rib = m3::tube(p0, p1, 0.13, 6, stone_hi);
            m3::tint_up(&mut rib, stone_hi, stone_lo); rib.mat = M::STONE; s.add(rib, Float::STILL);
        }
    }
    for i in 0..4 {                                   // latitudinal rings; i=0 is the SPRING-LINE ring that
        let phi = std::f32::consts::FRAC_PI_2 * i as f32 / 4.0; // closes the gap where the dome meets the drum
        let rr = (r_hall * phi.cos()).max(r_oc);
        let mut band = m3::torus(v(0.0, y_dome + rise * phi.sin(), 0.0), rr, 0.09, 96, 8, stone_hi);
        m3::tint_up(&mut band, stone_hi, stone_lo); band.mat = M::STONE; m3::scale_xz(&mut band, 1.0, sz); s.add(band, Float::STILL);
    }
    let y_oc = y_dome + rise + 0.2;
    let mut oculus = m3::torus(v(0.0, y_oc, 0.0), r_oc + 0.3, 0.5, 72, 10, stone_hi);
    m3::tint_up(&mut oculus, stone_hi, stone_lo); oculus.mat = M::STONE; m3::scale_xz(&mut oculus, 1.0, sz); s.add(oculus, Float::STILL);
    // COVER outer-ceiling gaps: a down-facing stone soffit ring sealing the perimeter between the dome base and
    // the outer wall (that ring was open to the void above the colonnade).
    let mut soffit = m3::annulus(v(0.0, y_dome - 0.05, 0.0), r_hall - 0.5, r_hall + 3.0, 96, stone_lo);
    m3::tint_up(&mut soffit, stone_hi, stone_lo); soffit.mat = M::STONE; m3::scale_xz(&mut soffit, 1.0, sz);
    for n in soffit.norms.iter_mut() { n.x = -n.x; n.y = -n.y; n.z = -n.z; } // face DOWN (seen from inside)
    s.add(soffit, Float::STILL);
    let mut sky = m3::disc(v(0.0, y_oc + 0.15, 0.0), r_oc + 0.3, 64, c(0xbf, 0xf4, 0xff));
    sky.mat = M::WATER; sky.emissive = 1.5; m3::scale_xz(&mut sky, 1.0, sz); s.add(sky, Float::STILL);
    let mut halo = m3::torus(v(0.0, y_oc - 0.3, 0.0), r_oc + 1.0, 0.22, 64, 8, c(0x8c, 0xe8, 0xff));
    halo.mat = M::WATER; halo.emissive = 0.7; m3::scale_xz(&mut halo, 1.0, sz); s.add(halo, Float::STILL);

    // CENTRAL stone plinth + glowing GLASS crystal — the round centerpiece (NOT compressed; it stays circular)
    let mut plinth = m3::tube(v(0.0, y_floor, 0.0), v(0.0, y_floor + 1.5, 0.0), 2.3, 36, stone_lo);
    m3::tint_height(&mut plinth, y_floor, y_floor + 1.5, stone_lo, stone_hi); plinth.mat = M::STONE; s.add(plinth, Float::STILL);
    let mut step = m3::disc(v(0.0, y_floor + 1.55, 0.0), 2.5, 40, stone_hi);
    m3::tint_up(&mut step, stone_hi, stone_lo); step.mat = M::STONE; s.add(step, Float::STILL);
    push_collider(Collider::Pillar { x: 0.0, z: 0.0, r: 2.7, y0: y_floor, y1: y_floor + 5.4 });
    contact_shadow(&mut s, 0.0, 0.0, 3.4, y_floor, 150); // central plinth grounding
    push_collider(Collider::Bound { r: r_hall + 1.9 });

    // MARBLE POSEIDON STATUES — landmarks around the colonnade (on the ellipse), each posed from the seed.
    let statues = (2 + (pr.seed % 3) as usize + (r_hall as usize) / 12).clamp(2, 5);
    for k in 0..statues {
        let a = (k as f32 + 0.5) / statues as f32 * TAU + hh(40) * TAU;
        let rs = r_col - 2.6;
        let (sx, szz) = (a.cos() * rs, ez(a.sin() * rs));
        let face = (-szz).atan2(-sx); // face the centre from the compressed position
        poseidon_statue(&mut s, v(sx, y_floor, szz), face, 1.0, pr.seed ^ (k as u32).wrapping_mul(0x9E37));
        push_collider(Collider::Pillar { x: sx, z: szz, r: 1.1, y0: y_floor, y1: y_floor + 5.0 });
        contact_shadow(&mut s, sx, szz, 1.5, y_floor, 140); // statue cast on the floor
    }

    // GARGOYLES — small sea-dragon heads perched on the architrave/cornice rim, facing OUTWARD over the water.
    // Reuses dragon_head at small scale; gapped at the hallway mouths so they don't float in the open doorways.
    {
        let ngarg = (cols / 2).clamp(6, 14);
        let r_g = r_col;                      // sit ON the architrave ring (was just outside it → occluded)
        let y_g = y_cap + 0.55;               // perched atop the architrave torus
        for k in 0..ngarg {
            let a = (k as f32 / ngarg as f32) * TAU + 0.12;
            if near_axis(a) { continue; }     // leave the hallway openings clear
            let (gx, gz) = (a.cos() * r_g, ez(a.sin() * r_g));
            let pos = v(gx, y_g, gz);
            let out_face = gz.atan2(gx);       // look outward, away from the centre
            // a stout corbel block the gargoyle crouches on
            let mut corbel = m3::tube(v(gx, y_cap, gz), v(gx, y_g, gz), 0.5, 12, stone_lo);
            m3::tint_height(&mut corbel, y_cap, y_g, stone_lo, stone_hi); corbel.mat = M::STONE; s.add(corbel, Float::STILL);
            for (m, f) in dragon_head(pos, out_face, 1.1, pr.seed ^ (k as u32).wrapping_mul(0x51ED), stone_hi, stone_lo) {
                s.add(m, f);
            }
        }
    }

    // SEA-FLOOR TERRAIN + natural formations in the WATER outside the perimeter (air interior stays clear).
    {
        let (sf_in, sf_yb, sf_seed) = (r_perim + 1.0, y_floor - 1.0, pr.seed ^ 0x7E44_A1);
        let inner = r_perim + 3.0;
        let outer = (wing_reach(&pr) + 4.0).max(inner + 6.0);
        // the rolling hillside the formations rest on (sand in the hollows, rock on the mounds, depth at the rim)
        seafloor(&mut s, sf_in, outer + 12.0, sf_yb, sz, sf_seed, sand_c, rock_c);
        let on_floor = |x: f32, z: f32| seafloor_h(x, z, sf_in, sf_yb, sf_seed);
        let patches = (4 + (pr.nodes / 24) as usize).clamp(5, 12);
        let (rk_lo, rk_hi) = (mix_rgb(rock_c, c(0x20, 0x24, 0x28), 0.3), rock_c);
        for k in 0..patches {
            let a = (k as f32 + 0.5) / patches as f32 * TAU + hh(50 + k as u32) * 0.6;
            let rr = inner + (outer - inner) * hh(70 + k as u32);
            let (gx, gz) = (a.cos() * rr, ez(a.sin() * rr));
            sea_garden(&mut s, v(gx, on_floor(gx, gz) + 0.05, gz), 2.6 + 1.4 * hh(90 + k as u32), pr.seed ^ 0x5EA0 ^ (k as u32).wrapping_mul(0x9E37));
            let a2 = a + 0.4 + hh(120 + k as u32) * 0.5;
            let rr2 = inner + (outer - inner) * hh(140 + k as u32);
            let (rx, rz) = (a2.cos() * rr2, ez(a2.sin() * rr2));
            reef(&mut s, v(rx, on_floor(rx, rz) + 0.1, rz), 1.3 + 1.1 * hh(160 + k as u32), pr.seed ^ 0x4EEF ^ (k as u32).wrapping_mul(0x68E3), rk_lo, rk_hi);
        }
        for k in 0..(2 + (pr.branches / 3) as usize).clamp(2, 4) {
            let a = hh(200 + k as u32) * TAU;
            let rr = inner + (outer - inner) * (0.4 + 0.5 * hh(220 + k as u32));
            let (ax, az) = (a.cos() * rr, ez(a.sin() * rr));
            rock_arch(&mut s, v(ax, on_floor(ax, az) + 0.2, az), v((a + 1.6).cos(), 0.0, (a + 1.6).sin()), 1.4 + 0.8 * hh(240 + k as u32), pr.seed ^ 0xA9C7 ^ (k as u32).wrapping_mul(0x51A3), rk_lo, rk_hi);
        }
    }
    // FISH SCHOOLS + WHALES drifting in the water OUTSIDE the dome — depth-life seen through the colonnade,
    // the clear windows and the open hallway sides (the air interior stays clear).
    sea_life(&mut s, r_perim, y_floor, y_cap, y_dome + rise * 0.6, pr.seed ^ 0xF1_5A11, pr.nodes);
    for k in 0..4 {                                   // central faceted glass spire (round, not compressed)
        let yy = y_floor + 2.1 + k as f32 * 0.95;
        let rr = 1.15 - k as f32 * 0.22;
        let mut cr = m3::rock(v(0.0, yy, 0.0), rr, 700 + k as u32, c(0x2a, 0x8a, 0x96));
        m3::tint_up(&mut cr, c(0x6a, 0xe0, 0xf0), c(0x10, 0x42, 0x4c)); cr.mat = M::WATER; cr.emissive = 0.30 + 0.08 * k as f32;
        s.add(cr, Float { amp: 0.10, speed: 0.5, phase: k as f32 * 1.3, drift: 0.0 });
    }

    // outer perimeter wall, OPEN at each door bearing (wings) AND at the hallway mouths on the spine axis.
    let mut door_gaps: Vec<(f32, f32)> = regions.iter().map(|r| (r.angle, 0.16)).collect();
    if hallways {
        let hw_half = (hall_w * 0.55 / r_perim).min(0.5);
        door_gaps.push((0.0, hw_half));
        door_gaps.push((std::f32::consts::PI, hw_half));
    }
    let mut perimeter = m3::wall_ring_gapped(v(0.0, y_floor, 0.0), r_perim, 8.0, 120, stone_lo, &door_gaps);
    m3::tint_height(&mut perimeter, y_floor, y_floor + 8.0, stone_lo, stone_hi); perimeter.mat = M::STONE; m3::scale_xz(&mut perimeter, 1.0, sz); s.add(perimeter, Float::STILL);
    // GATES — the camera may cross the perimeter Bound at each opening (so the fly travels down hallways +
    // through doors to the wings). Door gates are wide enough to pass; hallway gates match the corridor width.
    for reg in &regions { push_collider(Collider::Gate { ang: reg.angle, half: 0.22 }); }
    if hallways {
        let hw_half = (hall_w * 0.6 / r_perim).min(0.6);
        push_collider(Collider::Gate { ang: 0.0, half: hw_half });
        push_collider(Collider::Gate { ang: std::f32::consts::PI, half: hw_half });
    }
    let mut band = m3::annulus(v(0.0, y_floor - 0.05, 0.0), r_hall - 1.0, r_hall + 2.5, 120, stone_lo);
    m3::tint_up(&mut band, stone_hi, stone_lo); band.mat = M::WOOD; m3::scale_xz(&mut band, 1.0, sz); s.add(band, Float::STILL);

    // BOOKSHELVES + ALCOVES lining the inside of the perimeter — the library identity, so every interior view
    // reads as a flooded archive. A ring of cases against the wall, OPENED at doors + hallway mouths; every
    // few bays is a recessed arched ALCOVE with a glowing urn instead, for warm punctuation.
    {
        let wood_lo = c(0x2c, 0x1c, 0x10); let wood_hi = c(0x5a, 0x3c, 0x20);
        let rb = r_perim - 0.7;
        let case_w = 3.0_f32;
        let ncase = ((TAU * rb / (case_w + 0.4)) as usize).clamp(8, 30);
        let door_at = |a: f32| regions.iter().any(|r| { let d = (a - r.angle).abs() % TAU; d.min(TAU - d) < 0.28 }) || near_axis(a);
        for k in 0..ncase {
            let a = (k as f32 + 0.5) / ncase as f32 * TAU;
            if door_at(a) { continue; }
            let (bx, bz) = (a.cos() * rb, ez(a.sin() * rb));
            let inward = normv(v(-bx, 0.0, -bz));
            let tangent = v(-a.sin(), 0.0, a.cos());
            if k % 4 == 2 {
                wall_alcove(&mut s, v(bx, y_floor, bz), inward, tangent, case_w, 5.4, pr.seed ^ (k as u32).wrapping_mul(0x1357), stone_lo, stone_hi);
            } else {
                bookcase(&mut s, v(bx, y_floor, bz), inward, tangent, case_w, 6.6, pr.seed ^ (k as u32).wrapping_mul(0x2545_F491), wood_lo, wood_hi);
            }
        }
        // READING NOOKS scattered in the moat between the colonnade and the shelves, facing the books
        let wood_lo2 = c(0x33, 0x22, 0x14); let wood_hi2 = c(0x6a, 0x48, 0x26);
        let nnook = (2 + (pr.nodes / 30) as usize).clamp(3, 6);
        for k in 0..nnook {
            let a = (k as f32 + 0.3) / nnook as f32 * TAU + hh(33) * 1.7;
            if near_axis(a) { continue; }
            let rr = r_hall + 0.6;
            let (nx, nz) = (a.cos() * rr, ez(a.sin() * rr));
            reading_nook(&mut s, v(nx, y_floor, nz), normv(v(-nx, 0.0, -nz)), pr.seed ^ (k as u32).wrapping_mul(0x9E37_1357), wood_lo2, wood_hi2);
            push_collider(Collider::Pillar { x: nx, z: nz, r: 1.0, y0: y_floor, y1: y_floor + 1.6 });
        }
    }

    // HALLWAYS — the conversation's linear CONTINUATION. A long colonnaded corridor extends from the rotunda
    // along each end of the spine (±x), so the graph flows out of the rotunda and down a hall (the vision's
    // "linear runs → halls/colonnades"). Open colonnade + glass roof → ocean above and between the columns.
    if hallways {
        for sgn in [1.0f32, -1.0] {
            let dir = v(sgn, 0.0, 0.0);
            let start = v(sgn * (r_hall - 2.0), y_floor, 0.0);
            hallway(&mut s, start, dir, hall_len + 4.0, hall_w, y_floor, y_cap, pr.seed ^ if sgn > 0.0 { 0x4A11_0001 } else { 0x4A11_0002 }, stone_lo, stone_hi);
        }
    }

    // warm lanterns hung below the architrave (on the ellipse) — bloom anchors / counter-light to the cool
    // water. Brighter + warmer now, each casting a faint warm pool on the wet floor below it.
    for k in 0..lanterns {
        let a = (k as f32 / lanterns as f32) * TAU + 0.17;
        let (lx, lz) = (a.cos() * (r_col - 0.4), ez(a.sin() * (r_col - 0.4)));
        let mut cord = m3::tube(v(lx, y_cap - 0.4, lz), v(lx, y_cap - 1.6, lz), 0.03, 5, c(0x2a, 0x44, 0x48));
        cord.mat = M::STONE; s.add(cord, Float::STILL);
        let mut lan = m3::uv_sphere(v(lx, y_cap - 1.85, lz), 0.24, 14, 10, c(0xff, 0xcf, 0x86));
        lan.mat = M::STONE; lan.emissive = 1.02;
        s.add(lan, Float { amp: 0.14, speed: 0.5 + hash(k as i32, 71) * 0.4, phase: hash(k as i32, 72) * 6.28, drift: 0.0 });
        let mut pool = m3::disc(v(lx, y_floor + 0.04, lz), 2.1, 24, c(0x6a, 0x4a, 0x22));
        pool.mat = M::WATER; pool.emissive = 0.06; s.add(pool, Float::STILL); // lamplight on the water
    }

    // a few BOOKS drifting in the air (the library is half-dream)
    // (flying books removed per feedback — they cluttered the central air)

    // KELP BEDS: clusters of tall fronds rising from the sea floor in the OPEN OCEAN *outside* the glass dome
    // (beyond the perimeter wall) — seen THROUGH the colonnade/windows, not flooding the air-filled library.
    for k in 0..12 {
        let a = hash(k, 90) * TAU;
        let rad = r_hall + 2.0 + hash(k, 91) * 7.0; // OUTSIDE the hall, in the surrounding sea
        let (bx, bz) = (a.cos() * rad, ez(a.sin() * rad));
        let n = 3 + (hash(k, 92) * 3.0) as usize;
        for j in 0..n {
            let ji = k * 11 + j as i32;
            let (ox, oz) = ((hash(ji, 1) - 0.5) * 0.9, (hash(ji, 2) - 0.5) * 0.9);
            let h = 2.6 + hash(ji, 3) * 3.4;
            let base = v(bx + ox, y_floor - 0.4, bz + oz);
            let top = v(bx + ox + (hash(ji, 4) - 0.5) * 1.4, y_floor - 0.4 + h, bz + oz + (hash(ji, 5) - 0.5) * 1.4);
            let mut frond = m3::strand(base, top, 0.07 + hash(ji, 6) * 0.04, 5, 8, 0.0, 0.4 + hash(ji, 7) * 0.3, c(0x2c, 0x66, 0x40));
            m3::tint_height(&mut frond, y_floor, y_floor + h, c(0x1e, 0x4a, 0x2e), c(0x4c, 0x92, 0x5a)); frond.mat = M::MOSS; frond.emissive = 0.05;
            s.add(frond, Float { amp: 0.12, speed: 0.4 + hash(ji, 8) * 0.3, phase: hash(ji, 9) * 6.28, drift: 0.5 });
        }
    }

    // rising bubbles — ambient life, faintly glowing (compressed onto the ellipse)
    let nbub = (14 + (r_hall as usize)).min(34);
    for k in 0..nbub {
        let ang = hash(k as i32, 60) * TAU;
        let rad = 1.5 + hash(k as i32, 61) * (r_hall - 3.0);
        let p = v(ang.cos() * rad, y_floor + hash(k as i32, 62) * 8.0, ez(ang.sin() * rad));
        let mut bub = m3::uv_sphere(p, 0.08 + hash(k as i32, 63) * 0.14, 14, 10, c(0x33, 0x88, 0x92));
        m3::tint_up(&mut bub, c(0x66, 0xcc, 0xcc), c(0x1a, 0x44, 0x4c)); bub.mat = M::WATER; bub.emissive = 0.18;
        s.add(bub, Float { amp: 0.8 + hash(k as i32, 64) * 0.8, speed: 0.7 + hash(k as i32, 65) * 0.6, phase: hash(k as i32, 66) * 6.28, drift: 0.5 });
    }
    s
}

/// Cloudscape: an open, airy sky-deck. A pale cloud-deck you stand on + soft cumulus puffs drifting at many
/// heights (the floating-globule idea from Mistfall, but white and roomy). Open — no walls/roof — for an
/// unconfined idea-space. Brightness + airiness come from its palette + the Cloudscape light profile.
/// Forest: an open woodland clearing — varied trees ringing a glade, a grass-and-wildflower floor, drifting
/// birds. A bright, unconfined nature space (and the workshop scene for leaves/grass/foliage).
/// A small ground CRITTER — kind 0 rabbit (round body, long ears, puff tail), kind 1 cat (longer body, pointed
/// ears, curved tail). Built at the origin then oriented to `face` and dropped at `pos`; given a gentle hop/idle
/// bob. Low-poly + MOSS-shaded so it reads as fuzzy. (Random wander between bushes is a later, stateful pass.)
fn critter(s: &mut Scene, kind: u32, pos: Vec3, face: f32, seed: u32) {
    let bob = Float { amp: if kind == 0 { 0.13 } else { 0.05 }, speed: if kind == 0 { 2.4 } else { 1.2 },
        phase: hash(seed as i32, 1) * 6.28, drift: 0.0 };
    critter_m(s, kind, pos, face, seed, bob);
}
/// Critter with an explicit shared MOTION Float — so a critter walking a swaying bridge rides the exact same
/// sway as the planks (no detaching) instead of its own ground bob.
fn critter_m(s: &mut Scene, kind: u32, pos: Vec3, face: f32, seed: u32, bob: Float) {
    let (body_col, ear_col, tail_col) = if kind == 0 {
        (c(0xb6, 0xac, 0x9e), c(0xca, 0xc0, 0xb6), c(0xf0, 0xec, 0xe4)) // rabbit: warm grey, pale ears + tail
    } else {
        (c(0xd6, 0x8c, 0x3e), c(0xb0, 0x6e, 0x2c), c(0xc8, 0x80, 0x36)) // cat: ginger
    };
    let fwd = v(face.cos(), 0.0, face.sin());
    let right = v(-face.sin(), 0.0, face.cos());
    let up = v(0.0, 1.0, 0.0);
    // local (x=along body/fwd, y=up, z=side) → world
    let place = |m: &mut Mesh3, off: Vec3| {
        for p in m.verts.iter_mut() { *p = addv(addv(pos, off), addv(addv(mulv(fwd, p.x), mulv(up, p.y)), mulv(right, p.z))); }
        for n in m.norms.iter_mut() { *n = addv(addv(mulv(fwd, n.x), mulv(up, n.y)), mulv(right, n.z)); }
    };
    let sz = if kind == 0 { 0.34 } else { 0.40 };
    // body
    let mut body = m3::uv_sphere(v(0.0, 0.0, 0.0), 1.0, 12, 9, body_col);
    if kind == 0 { m3::scale3(&mut body, sz * 1.15, sz * 1.05, sz * 0.95); } else { m3::scale3(&mut body, sz * 1.7, sz * 0.8, sz * 0.7); }
    body.mat = M::MOSS; place(&mut body, v(0.0, sz, 0.0)); s.add(body, bob.clone());
    // head (forward, lifted)
    let hx = if kind == 0 { sz * 1.0 } else { sz * 1.7 };
    let mut head = m3::uv_sphere(v(0.0, 0.0, 0.0), 1.0, 10, 8, body_col);
    m3::scale3(&mut head, sz * 0.7, sz * 0.7, sz * 0.62); head.mat = M::MOSS;
    place(&mut head, v(0.0, sz * (if kind == 0 { 1.5 } else { 1.25 }), 0.0)); // placed via fwd below
    for p in head.verts.iter_mut() { *p = addv(*p, mulv(fwd, hx)); } s.add(head, bob.clone());
    // ears
    for sgn in [-1.0f32, 1.0] {
        let mut ear = if kind == 0 {
            m3::tube(v(0.0, 0.0, 0.0), v(0.0, sz * 1.5, 0.0), sz * 0.16, 6, ear_col) // long upright rabbit ear
        } else {
            m3::tube(v(0.0, 0.0, 0.0), v(0.0, sz * 0.5, 0.0), sz * 0.2, 5, ear_col)  // short pointed cat ear
        };
        let earpos = addv(addv(pos, mulv(fwd, hx + sz * 0.1)), addv(mulv(up, sz * (if kind == 0 { 2.0 } else { 1.7 })), mulv(right, sgn * sz * 0.32)));
        for p in ear.verts.iter_mut() { *p = addv(*p, earpos); } ear.mat = M::MOSS; s.add(ear, bob.clone());
    }
    // tail
    if kind == 0 {
        let mut tail = m3::uv_sphere(addv(addv(pos, mulv(fwd, -sz * 1.0)), mulv(up, sz * 1.1)), sz * 0.34, 8, 6, tail_col);
        tail.mat = M::MOSS; s.add(tail, bob.clone());
    } else {
        let base = addv(addv(pos, mulv(fwd, -sz * 1.6)), mulv(up, sz * 1.0));
        let tip = addv(base, addv(mulv(fwd, -sz * 0.6), mulv(up, sz * 1.4)));
        let mut tail = m3::tube(base, tip, sz * 0.16, 6, tail_col); tail.mat = M::MOSS; s.add(tail, bob.clone());
    }
    contact_shadow(s, pos.x, pos.z, sz * 1.4, pos.y, 90);
}

fn forest() -> Scene {
    use std::f32::consts::TAU;
    let mut s = Scene::new();
    let yf = -3.2_f32;
    // SEASON — from LOGSCOPE_SEASON (spring|summer|autumn|winter) or seed-derived; drives crown colour + winter barrenness
    let pr = env_params();
    let season: u8 = match std::env::var("LOGSCOPE_SEASON").ok().map(|v| v.to_lowercase()).as_deref() {
        Some("spring") => 0, Some("summer") => 1, Some("autumn") | Some("fall") => 2, Some("winter") => 3,
        _ => (pr.seed % 4) as u8,
    };
    let crown_pal: &[Color32] = match season {
        0 => &[c(0x7e, 0xa6, 0x4a), c(0x96, 0xc0, 0x5e), c(0xc8, 0xa6, 0xcc)],                                 // spring: fresh green + blossom
        1 => &[c(0x44, 0x6e, 0x28), c(0x5a, 0x82, 0x34), c(0x6e, 0x90, 0x3e)],                                 // summer: deep greens
        2 => &[c(0xc0, 0x6a, 0x28), c(0xd2, 0x9a, 0x40), c(0x9a, 0x44, 0x22), c(0xb0, 0x86, 0x30), c(0x7c, 0x8a, 0x34)], // autumn mix
        _ => &[c(0x8a, 0x76, 0x54), c(0x9a, 0x86, 0x60), c(0xc4, 0xc4, 0xcc)],                                 // winter: brown + snow
    };
    let keep_p: f32 = if season == 3 { 0.28 } else { 1.0 };                                                    // winter barrenness
    let shade = |col: Color32, f: f32| -> Color32 {
        c((col.r() as f32 * f).min(255.0) as u8, (col.g() as f32 * f).min(255.0) as u8, (col.b() as f32 * f).min(255.0) as u8)
    };
    let crown = |seed: i32| -> Option<Color32> {
        if hash(seed, 91) > keep_p { None } else { Some(crown_pal[(hash(seed, 92) * crown_pal.len() as f32) as usize % crown_pal.len()]) }
    };
    // larger floor + a darker far apron so the ground reads out to the tree line instead of ending in a rim
    // rolling TERRAIN — layered noise displaces the ground so it isn't a flat plate (gentle, so trees still read seated)
    let terrain = |x: f32, z: f32| -> f32 {
        0.30 * (x * 0.10).sin() * (z * 0.085).cos()
            + 0.18 * (x * 0.22 + z * 0.17).sin()
            + 0.10 * (x * 0.5 - z * 0.4).sin()
    };
    let mut ground = m3::disc(v(0.0, yf, 0.0), 100.0, 140, c(0x2a, 0x3a, 0x1c));
    for vtx in ground.verts.iter_mut() { vtx.y += terrain(vtx.x, vtx.z); }
    m3::tint_up(&mut ground, c(0x46, 0x5e, 0x26), c(0x1c, 0x28, 0x12)); ground.mat = M::MOSS; s.add(ground, Float::STILL);

    // GROUND LEAF LITTER — flat fallen leaves scattered across the floor (greens fading to autumn ambers/browns)
    let leafcols: &[Color32] = match season {
        0 => &[c(0x6e, 0x8a, 0x36), c(0x8a, 0xa0, 0x44), c(0x9a, 0xb0, 0x50)],
        1 => &[c(0x52, 0x6e, 0x28), c(0x6e, 0x8a, 0x36), c(0x8a, 0x96, 0x34)],
        2 => &[c(0x6e, 0x8a, 0x36), c(0x8a, 0x96, 0x34), c(0xb0, 0x86, 0x30), c(0xc0, 0x6a, 0x28), c(0x9a, 0x5a, 0x26), c(0x52, 0x6e, 0x28)],
        _ => &[c(0x7a, 0x6a, 0x4e), c(0x8a, 0x7a, 0x5c), c(0xc4, 0xc4, 0xcc), c(0x6a, 0x5c, 0x44)],
    };
    for k in 0..320 {
        let a = hash(k, 70) * TAU; let rad = (hash(k, 71)).sqrt() * 72.0; // sqrt → even areal density
        let (lx, lz) = (a.cos() * rad, a.sin() * rad);
        let r = 0.16 + hash(k, 72) * 0.22;
        let mut leaf = m3::disc(v(lx, yf + terrain(lx, lz) + 0.02 + hash(k, 73) * 0.02, lz), r, 5, leafcols[k as usize % leafcols.len()]);
        m3::scale_xz(&mut leaf, 1.0, 0.6 + hash(k, 74) * 0.5); // elongate slightly → leaf-ish, not perfect circles
        leaf.mat = M::MOSS; s.add(leaf, Float::STILL);
    }

    // HERO trees near the centre — full canopies
    for k in 0..7 {
        let a = TAU * hash(k, 1);
        let rad = 5.0 + hash(k, 2) * 9.0;
        let (tx, tz) = (a.cos() * rad, a.sin() * rad);
        let hgt = 5.0 + hash(k, 3) * 3.5;
        let origin = v(tx, yf + terrain(tx, tz), tz);
        let lean = normv(v((hash(k, 4) - 0.5) * 0.2, 1.0, (hash(k, 5) - 0.5) * 0.2));
        tree_branch(&mut s, origin, lean, hgt, 0.5 + hash(k, 6) * 0.3, 2, 7);
        let cc = addv(origin, mulv(lean, hgt + 0.4));
        for j in 0..7 {
            let aa = TAU * j as f32 / 7.0;
            let p = addv(cc, v(aa.cos() * 1.7, hash(k * 7 + j, 1) * 0.9 - 0.3, aa.sin() * 1.7));
            if let Some(bc) = crown((k * 7 + j) as i32) {
                let mut puff = m3::leaf_cluster(p, 1.15, (k * 7 + j) as u32, bc);
                m3::tint_up(&mut puff, shade(bc, 1.25), shade(bc, 0.6)); puff.mat = M::MOSS;
                s.add(puff, Float { amp: 0.08, speed: 0.6, phase: hash(k * 7 + j, 2) * 6.28, drift: 0.05 });
            }
        }
        contact_shadow(&mut s, tx, tz, 1.5, yf + terrain(tx, tz), 110);
    }

    // MANY TALL THIN trees — slender trunks rising high with a small high canopy (the "way more tall thin trees")
    for k in 0..80 {
        let a = hash(k, 10) * TAU;
        let rad = 8.0 + (hash(k, 11)).sqrt() * 56.0;
        let (tx, tz) = (a.cos() * rad, a.sin() * rad);
        let hgt = 8.0 + hash(k, 12) * 6.0;
        let thin = 0.18 + hash(k, 13) * 0.16;          // slender
        let lean = normv(v((hash(k, 14) - 0.5) * 0.08, 1.0, (hash(k, 15) - 0.5) * 0.08));
        tree_branch(&mut s, v(tx, yf + terrain(tx, tz), tz), lean, hgt, thin, 1, 70 + k);
        let cc = v(tx, yf + terrain(tx, tz) + hgt, tz);
        for j in 0..4 {                                 // small tight crown high up
            let aa = TAU * j as f32 / 4.0 + hash(k, 16) * 6.28;
            let p = addv(cc, v(aa.cos() * 1.0, hash(k * 4 + j, 1) * 0.7, aa.sin() * 1.0));
            if let Some(bc) = crown((k * 4 + j) as i32 + 5000) {
                let mut puff = m3::leaf_cluster(p, 0.55 + hash(k, 17) * 0.3, (k * 4 + j) as u32, bc);
                m3::tint_up(&mut puff, shade(bc, 1.25), shade(bc, 0.6)); puff.mat = M::MOSS;
                s.add(puff, Float { amp: 0.12, speed: 0.7, phase: hash(k * 4 + j, 2) * 6.28, drift: 0.08 });
            }
        }
    }

    // DISTANT LOD "blot-out" trees — a far band of simple slim trunks + a single dim puff, out past the main
    // grove. They shrink with distance and dissolve into the fog so the wood has no hard edge (no walling-in).
    for k in 0..64 {
        let a = hash(k, 80) * TAU;
        let rad = 56.0 + hash(k, 81) * 42.0;
        let (tx, tz) = (a.cos() * rad, a.sin() * rad);
        let hgt = 7.0 + hash(k, 82) * 7.0;
        tree_branch(&mut s, v(tx, yf, tz), v(0.0, 1.0, 0.0), hgt, 0.16 + hash(k, 83) * 0.12, 0, 200 + k);
        if let Some(bc) = crown(k as i32 + 9000) {
            let mut puff = m3::leaf_cluster(v(tx, yf + hgt, tz), 0.8 + hash(k, 84) * 0.5, 800 + k as u32, bc); // LOD: smaller distant crowns
            m3::tint_up(&mut puff, shade(bc, 1.2), shade(bc, 0.55)); puff.mat = M::MOSS;
            s.add(puff, Float { amp: 0.06, speed: 0.5, phase: hash(k, 85) * 6.28, drift: 0.04 });
        }
    }

    // FALLEN LOGS — mossy trunks down on the floor
    for k in 0..8 {
        let a = hash(k, 120) * TAU; let rad = 6.0 + hash(k, 121) * 40.0;
        let (lx, lz) = (a.cos() * rad, a.sin() * rad);
        let dir = hash(k, 122) * TAU; let len = 2.5 + hash(k, 123) * 3.0; let rr = 0.32 + hash(k, 124) * 0.18;
        let p0 = v(lx, yf + rr, lz);
        let p1 = v(lx + dir.cos() * len, yf + rr, lz + dir.sin() * len);
        let mut log = m3::tube(p0, p1, rr, 8, c(0x52, 0x3e, 0x26));
        m3::tint_up(&mut log, c(0x6e, 0x54, 0x34), c(0x36, 0x28, 0x18)); log.mat = M::WOOD; s.add(log, Float::STILL);
        let mut moss = m3::leaf_cluster(v((p0.x + p1.x) * 0.5, yf + rr + 0.22, (p0.z + p1.z) * 0.5), 0.5 + len * 0.06, 130 + k as u32, c(0x4e, 0x70, 0x2e));
        m3::scale_xz(&mut moss, len * 0.32, 0.4); m3::tint_up(&mut moss, c(0x68, 0x8c, 0x38), c(0x32, 0x4e, 0x20)); moss.mat = M::MOSS; s.add(moss, Float::STILL);
        contact_shadow(&mut s, (p0.x + p1.x) * 0.5, (p0.z + p1.z) * 0.5, len * 0.4, yf, 90);
    }
    // SKUNK-CABBAGE clusters — occasional clumps of broad upright blades with a purple spathe
    for k in 0..10 {
        if hash(k, 140) < 0.45 { continue; }
        let a = hash(k, 141) * TAU; let rad = 4.0 + hash(k, 142) * 44.0;
        let (sx, sz) = (a.cos() * rad, a.sin() * rad);
        for b in 0..5 {
            let ba = TAU * b as f32 / 5.0 + hash(k, 143) * 6.28;
            let tip = v(sx + ba.cos() * 0.5, yf + 1.0 + hash(k * 5 + b, 1) * 0.5, sz + ba.sin() * 0.5);
            let mut blade = m3::leaf_cluster(tip, 0.66, 150 + (k * 5 + b) as u32, c(0x3a, 0x66, 0x24));
            m3::scale3(&mut blade, 0.6, 1.5, 0.6); m3::tint_up(&mut blade, c(0x56, 0x86, 0x30), c(0x28, 0x46, 0x1a)); blade.mat = M::MOSS;
            s.add(blade, Float { amp: 0.04, speed: 0.5, phase: hash(k * 5 + b, 3) * 6.28, drift: 0.05 });
        }
        let mut spathe = m3::uv_sphere(v(sx, yf + 0.5, sz), 0.3, 8, 6, c(0x5a, 0x2e, 0x52));
        m3::scale3(&mut spathe, 0.7, 1.3, 0.7); m3::tint_up(&mut spathe, c(0x74, 0x3e, 0x6a), c(0x3a, 0x1e, 0x36)); spathe.mat = M::MOSS; s.add(spathe, Float::STILL);
    }
    // ROCKS — occasional weathered stones
    for k in 0..14 {
        let a = hash(k, 160) * TAU; let rad = 4.0 + (hash(k, 161)).sqrt() * 50.0;
        let (rx, rz) = (a.cos() * rad, a.sin() * rad); let rs = 0.5 + hash(k, 162) * 1.4;
        let mut rock = m3::rock(v(rx, yf + rs * 0.4, rz), rs, 170 + k as u32, c(0x52, 0x56, 0x52));
        m3::tint_up(&mut rock, c(0x70, 0x74, 0x70), c(0x32, 0x36, 0x34)); m3::scale_xz(&mut rock, 1.0, 0.85); rock.mat = M::STONE; s.add(rock, Float::STILL);
        contact_shadow(&mut s, rx, rz, rs * 0.6, yf, 90);
    }

    // GRASS TUFTS — many small upright blades following the terrain
    for k in 0..220 {
        let a = hash(k, 200) * TAU; let rad = 3.0 + (hash(k, 201)).sqrt() * 58.0;
        let (gx, gz) = (a.cos() * rad, a.sin() * rad);
        let gy = yf + terrain(gx, gz);
        let base = if season == 3 { c(0x84, 0x86, 0x60) } else { crown_pal[(hash(k, 202) * crown_pal.len() as f32) as usize % crown_pal.len()] };
        let mut tuft = m3::leaf_cluster(v(gx, gy + 0.28, gz), 0.3 + hash(k, 203) * 0.22, 200 + k as u32, base);
        m3::scale3(&mut tuft, 0.42, 1.7, 0.42); // thin + tall → blades
        m3::tint_up(&mut tuft, shade(base, 1.25), shade(base, 0.55)); tuft.mat = M::MOSS;
        s.add(tuft, Float { amp: 0.03, speed: 0.7, phase: hash(k, 204) * 6.28, drift: 0.10 });
    }
    // BUSH TUFTS — low leafy mounds dotted across the floor
    for k in 0..34 {
        let a = hash(k, 210) * TAU; let rad = 4.0 + (hash(k, 211)).sqrt() * 54.0;
        let (bx, bz) = (a.cos() * rad, a.sin() * rad);
        let by = yf + terrain(bx, bz);
        let bc = crown_pal[(hash(k, 212) * crown_pal.len() as f32) as usize % crown_pal.len()];
        let mut bush = m3::leaf_cluster(v(bx, by + 0.5, bz), 0.7 + hash(k, 213) * 0.7, 240 + k as u32, bc);
        m3::scale3(&mut bush, 1.0, 0.7, 1.0); m3::tint_up(&mut bush, shade(bc, 1.2), shade(bc, 0.55)); bush.mat = M::MOSS;
        s.add(bush, Float { amp: 0.04, speed: 0.5, phase: hash(k, 214) * 6.28, drift: 0.06 });
        contact_shadow(&mut s, bx, bz, 0.8, by, 80);
    }

    // BUSHES — low leafy mounds the critters dart between
    let mut bushes: Vec<(f32, f32)> = Vec::new();
    for k in 0..18 {
        let a = hash(k, 90) * TAU; let rad = 3.0 + (hash(k, 91)).sqrt() * 48.0;
        let (bx, bz) = (a.cos() * rad, a.sin() * rad);
        bushes.push((bx, bz));
        for j in 0..3 {
            let off = v((hash(k * 3 + j, 1) - 0.5) * 1.1, 0.2 + hash(k * 3 + j, 2) * 0.4, (hash(k * 3 + j, 3) - 0.5) * 1.1);
            let mut bush = m3::leaf_cluster(addv(v(bx, yf, bz), off), 0.9 + hash(k * 3 + j, 4) * 0.5, (900 + k * 3 + j) as u32, c(0x3a, 0x5a, 0x24));
            m3::tint_up(&mut bush, c(0x5e, 0x80, 0x32), c(0x28, 0x40, 0x1c)); bush.mat = M::MOSS;
            s.add(bush, Float { amp: 0.05, speed: 0.8, phase: hash(k * 3 + j, 5) * 6.28, drift: 0.03 });
        }
        contact_shadow(&mut s, bx, bz, 1.1, yf, 90);
    }

    // CRITTERS — rabbits + cats tucked beside the bushes (a basic placement; random wander is a later pass)
    for k in 0..8 {
        let (bx, bz) = bushes[(k * 5 + 2) % bushes.len()];
        let a = hash(k as i32, 60) * TAU; let off = 1.0 + hash(k as i32, 61) * 1.4;
        let (cx, cz) = (bx + a.cos() * off, bz + a.sin() * off);
        let face = (bz - cz).atan2(bx - cx) + std::f32::consts::PI; // face away from its bush
        critter(&mut s, if k % 3 == 0 { 1 } else { 0 }, v(cx, yf, cz), face, 1000 + k as u32);
    }

    for k in 0..110 { let a = hash(k, 20) * TAU; let rad = (hash(k, 21)).sqrt() * 66.0; grass_tuft(&mut s, a.cos() * rad, a.sin() * rad, yf, 300 + k as u32, c(0x5a, 0x7e, 0x30)); }
    for k in 0..24 {
        let a = hash(k, 30) * TAU; let rad = 1.5 + (hash(k, 31)).sqrt() * 52.0;
        let cols = [c(0xff, 0x6a, 0x8a), c(0xff, 0xd0, 0x40), c(0xa0, 0x86, 0xff), c(0xff, 0xa0, 0x50)];
        flower(&mut s, a.cos() * rad, a.sin() * rad, yf, 400 + k as u32, cols[k as usize % 4]);
    }
    for k in 0..6 { let a = hash(k, 40) * TAU; let rad = 3.0 + hash(k, 41) * 12.0; bird(&mut s, v(a.cos() * rad, yf + 5.5 + hash(k, 42) * 5.0, a.sin() * rad), 500 + k as u32); }
    for k in 0..10 { let a = hash(k, 50) * TAU; let rad = 1.5 + hash(k, 51) * 16.0; bee(&mut s, v(a.cos() * rad, yf + 0.5 + hash(k, 52) * 1.2, a.sin() * rad), 600 + k as u32); }
    s
}

/// Treehouse: a canopy walk among the trees (inspired by Morris Arboretum's "Out on a Limb") — big trunks
/// rising from a dim forest floor far below, wooden platforms with hut roofs up in the canopy, swinging rope
/// bridges between them, and fireflies drifting at dusk. The height + the warm lantern glow are the feeling.
/// A fuzzy little creature hanging from a canopy branch by its arms, swinging gently like a pendulum (shared
/// Float with drift). Round mossy body + head + ears, dangling legs, a long curled tail. Treehouse dweller.
fn swinger(s: &mut Scene, anchor: Vec3, drop: f32, seed: u32) {
    let col = [c(0x7c, 0x5a, 0x36), c(0x68, 0x4e, 0x30), c(0x8c, 0x68, 0x40)][(seed % 3) as usize];
    let body = v(anchor.x, anchor.y - drop, anchor.z);
    let swing = Float { amp: 0.10, speed: 0.6 + hash(seed as i32, 1) * 0.5, phase: hash(seed as i32, 2) * 6.28, drift: 0.7 };
    for sgn in [-1.0f32, 1.0] { // arms up to the branch
        let mut arm = m3::tube(addv(anchor, v(sgn * 0.12, 0.0, 0.0)), addv(body, v(sgn * 0.18, 0.28, 0.0)), 0.05, 5, col);
        arm.mat = M::WOOD; s.add(arm, swing);
    }
    let mut torso = m3::uv_sphere(body, 0.32, 12, 9, col); torso.mat = M::MOSS; s.add(torso, swing);
    let mut head = m3::uv_sphere(addv(body, v(0.0, 0.33, 0.0)), 0.21, 10, 8, col); head.mat = M::MOSS; s.add(head, swing);
    for sgn in [-1.0f32, 1.0] {
        let mut ear = m3::uv_sphere(addv(body, v(sgn * 0.15, 0.45, 0.0)), 0.07, 6, 5, col); ear.mat = M::MOSS; s.add(ear, swing);
        let mut eye = m3::uv_sphere(addv(body, v(sgn * 0.08, 0.36, 0.18)), 0.035, 6, 5, c(0xf6, 0xe6, 0x9a)); eye.mat = M::MOSS; eye.emissive = 1.2; s.add(eye, swing); // bioluminescent eyes
        let mut leg = m3::tube(addv(body, v(sgn * 0.14, -0.2, 0.0)), addv(body, v(sgn * 0.18, -0.55, 0.06)), 0.05, 5, col); leg.mat = M::WOOD; s.add(leg, swing);
    }
    let tb = addv(body, v(0.0, -0.08, -0.26));
    let mut tail = m3::tube(tb, addv(tb, v(0.0, 0.34, -0.5)), 0.05, 6, col); tail.mat = M::WOOD; s.add(tail, swing);
}

fn treehouse() -> Scene {
    use std::f32::consts::TAU;
    let pr = env_params();
    // GLOBAL graph-derived height factor (like Mistfall's unified rune colour off the variant): one hash stream
    // off the conversation seed sets the whole walk's canopy height + how much the platforms stagger around it,
    // so each conversation gets a differently-pitched canopy walk.
    let hsh = |k: u32| -> f32 { let mut h = pr.seed ^ k.wrapping_mul(0x9E37_79B9); h ^= h >> 15; h = h.wrapping_mul(0x2C1B_3C6D); h ^= h >> 13; (h & 0xFFFF) as f32 / 65535.0 };
    let canopy_lift = -1.0 + hsh(1) * 3.0;   // base height of the walk (global, graph-derived)
    let canopy_spread = 1.6 + hsh(2) * 2.2;  // platform height stagger (global)
    let mut s = Scene::new();
    // dim forest floor far below — you sense the height of the walk
    let mut floor = m3::disc(v(0.0, -8.0, 0.0), 48.0, 84, c(0x18, 0x1e, 0x12)); // doubled map
    m3::tint_up(&mut floor, c(0x28, 0x30, 0x1a), c(0x10, 0x14, 0x0c)); floor.mat = M::MOSS; floor.emissive = 0.02; s.add(floor, Float::STILL);
    // big trees rising from the floor up through the canopy
    let trees = [(-6.0_f32, -3.0_f32), (5.5, -4.0), (6.0, 4.5), (-5.0, 5.0)]; // central tree removed → room for the graph
    for (i, (tx, tz)) in trees.iter().enumerate() {
        let th = 13.5 + (hsh(i as u32 * 5 + 10) - 0.5) * 4.0 + canopy_lift; // tree height varies w/ graph + global lift
        tree_branch(&mut s, v(*tx, -8.0, *tz), normv(v(0.04 * (i as f32 - 2.0), 1.0, 0.03)), th, 0.9, 2, 7);
        let cc = v(*tx, -8.0 + th - 2.0, *tz);
        for j in 0..6 {
            let a = TAU * j as f32 / 6.0;
            let p = addv(cc, v(a.cos() * 2.0, hash((i * 8 + j) as i32, 1) * 1.1 - 0.4, a.sin() * 2.0));
            let mut puff = m3::leaf_cluster(p, 1.0, (i * 8 + j) as u32, c(0x3a, 0x5e, 0x26));
            m3::tint_up(&mut puff, c(0x60, 0x82, 0x34), c(0x2a, 0x42, 0x1e)); puff.mat = M::MOSS;
            s.add(puff, Float { amp: 0.1, speed: 0.5, phase: hash((i * 8 + j) as i32, 2) * 6.28, drift: 0.06 });
        }
    }
    // DENSER FOREST: many more trees WITHOUT platforms, filling out the canopy around the walk so the four
    // treehouses sit in a real forest rather than a sparse clearing.
    for k in 0..24 {
        let a = hash(k, 40) * TAU;
        let rad = 14.0 + hash(k, 41) * 36.0;
        let (tx, tz) = (a.cos() * rad, a.sin() * rad);
        let hgt = 10.0 + hash(k, 42) * 6.0 + canopy_lift * 1.2; // whole canopy rises/falls with the global factor
        tree_branch(&mut s, v(tx, -8.0, tz), normv(v((hash(k, 43) - 0.5) * 0.12, 1.0, (hash(k, 44) - 0.5) * 0.12)), hgt, 0.55 + hash(k, 45) * 0.5, 2, 6);
        let cc = v(tx, -8.0 + hgt - 1.0, tz);
        for j in 0..6 {
            let aa = TAU * j as f32 / 6.0;
            let p = addv(cc, v(aa.cos() * 1.6, hash((k * 6 + j) as i32, 1) * 1.0 - 0.3, aa.sin() * 1.6));
            let mut puff = m3::leaf_cluster(p, 0.9, (k * 6 + j + 99) as u32, c(0x34, 0x56, 0x22));
            m3::tint_up(&mut puff, c(0x58, 0x78, 0x30), c(0x26, 0x3c, 0x1a)); puff.mat = M::MOSS;
            s.add(puff, Float { amp: 0.1, speed: 0.5, phase: hash((k * 6 + j) as i32, 2) * 6.28, drift: 0.06 });
        }
    }
    // treehouse platforms — heights from the GLOBAL canopy lift + a per-platform stagger (graph-derived), so the
    // walk steps up and down through the trees differently for each conversation.
    let pxz = [(-6.0_f32, -3.0_f32), (5.5, -4.0), (6.0, 4.5), (-5.0, 5.0)];
    let mut plats = [(0.0_f32, 0.0_f32, 0.0_f32); 4];
    for (i, (px, pz)) in pxz.iter().enumerate() {
        let py = canopy_lift + (hsh(i as u32 * 7 + 3) - 0.5) * canopy_spread;
        plats[i] = (*px, *pz, py);
        tree_platform(&mut s, *px, *pz, py, 2.2, (i * 17) as u32);
    }
    // swinging rope bridges connecting the platforms in a loop (the sag spans the height differences)
    for i in 0..4 {
        let a = plats[i]; let b = plats[(i + 1) % 4];
        rope_bridge(&mut s, v(a.0, a.2, a.1), v(b.0, b.2, b.1), (i * 31) as u32);
    }
    // MORE TREEHOUSES — extra platforms perched on the surrounding trees (canopy leans toward structures, not orbs)
    for k in 0..24 {
        if hash(k, 47) < 0.68 { continue; }
        let a = hash(k, 40) * TAU; let rad = 14.0 + hash(k, 41) * 36.0; // same placement as the surrounding trees
        let (tx, tz) = (a.cos() * rad, a.sin() * rad);
        let hgt = 10.0 + hash(k, 42) * 6.0 + canopy_lift * 1.2;
        tree_platform(&mut s, tx, tz, -8.0 + hgt - 2.6, 1.7, (400 + k) as u32);
    }
    // procedural MOSS creeping over the main trunks + platform bases (clumps at graph-varied spots)
    for (i, (tx, tz)) in trees.iter().enumerate() {
        for m in 0..3 {
            let a = hash((i * 3 + m) as i32, 80) * TAU;
            let my = -6.0 + hash((i * 3 + m) as i32, 81) * 9.0;
            let mut moss = m3::leaf_cluster(v(*tx + a.cos() * 0.9, my, *tz + a.sin() * 0.9), 0.4 + hash((i * 3 + m) as i32, 82) * 0.4, (700 + i * 3 + m) as u32, c(0x46, 0x70, 0x30));
            m3::tint_up(&mut moss, c(0x68, 0x90, 0x3c), c(0x2c, 0x4a, 0x1e)); moss.mat = M::MOSS;
            s.add(moss, Float { amp: 0.03, speed: 0.5, phase: hash((i * 3 + m) as i32, 83) * 6.28, drift: 0.04 });
        }
    }
    // fireflies drifting through the canopy (denser now — they read as a living dusk swarm)
    for k in 0..72 { let a = hash(k, 70) * TAU; let rad = hash(k, 71) * 44.0; firefly(&mut s, v(a.cos() * rad, -4.0 + hash(k, 72) * 9.0, a.sin() * rad), 900 + k as u32); }

    // DISTANT LOD "blot-out" trees — a far ring of slim trunks + one dim puff, dissolving into the fog so the
    // canopy walk sits inside a deep wood with no hard edge (matches the Forest LOD treatment).
    for k in 0..30 {
        let a = hash(k, 95) * TAU; let rad = 56.0 + hash(k, 96) * 36.0;
        let (tx, tz) = (a.cos() * rad, a.sin() * rad);
        let hgt = 11.0 + hash(k, 97) * 7.0 + canopy_lift;
        tree_branch(&mut s, v(tx, -8.0, tz), v(0.0, 1.0, 0.0), hgt, 0.2 + hash(k, 98) * 0.14, 0, 300 + k);
        let mut puff = m3::leaf_cluster(v(tx, -8.0 + hgt, tz), 0.9 + hash(k, 99) * 0.5, 1200 + k as u32, c(0x34, 0x54, 0x22));
        m3::tint_up(&mut puff, c(0x56, 0x76, 0x30), c(0x24, 0x3c, 0x18)); puff.mat = M::MOSS;
        s.add(puff, Float { amp: 0.06, speed: 0.5, phase: hash(k, 100) * 6.28, drift: 0.04 });
    }

    // CREATURES — rabbits + cats roam the FOREST FLOOR far below; hanging creatures dangle from procedural ROPES
    // in the canopy ABOVE the walk; a few cross the bridges (one gripping the handrail rope as it goes).
    let bsag = 1.3_f32;
    // rabbits + cats on the forest floor (not perched on the canopy decks)
    for k in 0..8 {
        let a = hash(k, 60) * TAU; let rad = 3.0 + hash(k, 61) * 18.0;
        critter(&mut s, if k % 3 == 0 { 1 } else { 0 }, v(a.cos() * rad, -8.0, a.sin() * rad), hash(k, 62) * TAU, 1300 + k as u32);
    }
    // hanging creatures on procedural ROPES dropped from the tree canopy ABOVE the walk
    for (i, (tx, tz)) in trees.iter().enumerate() {
        for r in 0..2i32 {
            let th = 13.5 + (hsh(i as u32 * 5 + 10) - 0.5) * 4.0 + canopy_lift;
            let key = i as i32 * 2 + r;
            let aa = hash(key, 52) * TAU;
            let off = 1.3 + hash(key, 51) * 1.4;
            let top = v(*tx + aa.cos() * off, -8.0 + th - 3.0 - hash(key, 50) * 2.0, *tz + aa.sin() * off);
            let drop = 1.6 + hash(key, 53) * 1.6;
            let bot = v(top.x, top.y - drop, top.z);
            let mut rope = m3::tube(top, bot, 0.04, 5, c(0x6a, 0x52, 0x32)); rope.mat = M::WOOD;
            s.add(rope, Float { amp: 0.06, speed: 0.5, phase: hash(key, 54) * 6.28, drift: 0.5 });
            swinger(&mut s, bot, 0.4, 1180 + key as u32);
        }
    }
    // a couple WALK the bridges — grounded on the swinging plank, facing along the span, riding the bridge's sway
    for i in 0..4 {
        if hash(i as i32, 9) < 0.4 { continue; }
        let (a, b) = (plats[i], plats[(i + 1) % 4]);
        let t = 0.38 + hash(i as i32, 12) * 0.24;
        let (dx, dy, dz) = (b.0 - a.0, b.2 - a.2, b.1 - a.1);
        let droop = bsag * 4.0 * t * (1.0 - t);
        let pos = v(a.0 + dx * t, a.2 + dy * t - droop, a.1 + dz * t); // on the plank
        let sway = Float { amp: 0.12, speed: 0.5, phase: hash((i * 31) as i32, 1) * 6.28, drift: 0.6 };
        critter_m(&mut s, (i % 2) as u32, pos, dz.atan2(dx), 1160 + i as u32, sway);
    }
    // a HANGING creature CROSSES a bridge, gripping the handrail rope as it goes (the new traverse pose: it dangles
    // from the rope with its feet skimming the plank, riding the same sway as the span)
    {
        let (a, b) = (plats[0], plats[1]);
        let t = 0.5; let (dx, dy, dz) = (b.0 - a.0, b.2 - a.2, b.1 - a.1);
        let droop = bsag * 4.0 * t * (1.0 - t);
        let grip = v(a.0 + dx * t, a.2 + dy * t - droop + 1.05, a.1 + dz * t); // hand at handrail height above the plank
        swinger(&mut s, grip, 0.95, 1190);
    }
    s
}

/// A jagged LIGHTNING bolt from `from` to `to`: a perpendicular-jittered polyline of bright emissive tube
/// segments (zero jitter at the ends, max in the middle) with the odd short fork. `col` is the storm's hue.
fn lightning_arc(s: &mut Scene, from: Vec3, to: Vec3, col: Color32, seed: u32) {
    let d = v(to.x - from.x, to.y - from.y, to.z - from.z);
    let len = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt().max(1e-3);
    let dir = mulv(d, 1.0 / len);
    let up = if dir.y.abs() > 0.9 { v(1.0, 0.0, 0.0) } else { v(0.0, 1.0, 0.0) };
    let p1 = normv(v(dir.y * up.z - dir.z * up.y, dir.z * up.x - dir.x * up.z, dir.x * up.y - dir.y * up.x));
    let p2 = v(dir.y * p1.z - dir.z * p1.y, dir.z * p1.x - dir.x * p1.z, dir.x * p1.y - dir.y * p1.x);
    let segs = 9;
    let spread = len * 0.14;
    let mut prev = from;
    for i in 1..=segs {
        let t = i as f32 / segs as f32;
        let taper = 1.0 - (2.0 * t - 1.0).abs();
        let j1 = (hash((seed as i32).wrapping_add(i * 13), 1) - 0.5) * 2.0 * spread * taper;
        let j2 = (hash((seed as i32).wrapping_add(i * 29), 2) - 0.5) * 2.0 * spread * taper;
        let base = v(from.x + d.x * t, from.y + d.y * t, from.z + d.z * t);
        let p = if i == segs { to } else { addv(base, addv(mulv(p1, j1), mulv(p2, j2))) };
        let mut bolt = m3::tube(prev, p, 0.05, 4, col); bolt.mat = M::MOSS; bolt.emissive = 2.3; s.add(bolt, Float::STILL);
        if i < segs && hash((seed as i32).wrapping_add(i * 3), 7) > 0.62 {
            let bt = addv(p, addv(mulv(dir, len * 0.13), mulv(p1, spread * 1.6 * (hash((seed as i32).wrapping_add(i), 8) - 0.5))));
            let mut fork = m3::tube(p, bt, 0.03, 3, col); fork.mat = M::MOSS; fork.emissive = 1.8; s.add(fork, Float::STILL);
        }
        prev = p;
    }
}

fn cloudscape() -> Scene {
    use std::f32::consts::TAU;
    let mut s = Scene::new();
    let ms = (map_radius() / 56.0).clamp(1.0, 2.4); // grow the deck + cloud field with the graph (gentler than a room)
    let mut deck = m3::disc(v(0.0, -3.2, 0.0), 30.0 * ms, 96, c(0x9a, 0xa6, 0xc0));
    m3::tint_up(&mut deck, c(0xb2, 0xbe, 0xd6), c(0x6e, 0x7a, 0x96)); deck.mat = M::STONE; deck.emissive = 0.04; s.add(deck, Float::STILL);
    // CLOUDS — particleized masses over a DOUBLED map. Per-particle drift morphs/deforms each cloud (not a rigid
    // blob); varied per-cloud drift makes groups MERGE and DIVERGE; wide alpha spread DITHERS the density.
    for k in 0..42 {
        let ki = k as i32;
        let a = hash(ki, 2) * TAU;
        let rad = (4.0 + hash(ki, 3) * 26.0) * ms;                     // spread scales with the map
        let p = v(a.cos() * rad, -2.0 + hash(ki, 4) * 13.0, a.sin() * rad);
        let r = 1.0 + hash(ki, 5) * 2.4;
        let cdrift = 0.4 + hash(ki, 7) * 1.4;                          // varied → clouds merge & diverge
        let parts = 26 + (hash(ki, 6) * 14.0) as usize;
        for j in 0..parts {
            let ji = k as i32 * 53 + j as i32;
            let rr = (hash(ji, 1)).powf(0.6);
            let aa = hash(ji, 2) * TAU; let bb = (hash(ji, 3) * 2.0 - 1.0).asin();
            let off = v(aa.cos() * bb.cos() * r * 1.5 * rr, bb.sin() * r * 0.55 * rr, aa.sin() * bb.cos() * r * 1.5 * rr);
            let prad = 0.16 + hash(ji, 4) * 0.3;
            let mut pf = m3::uv_sphere(addv(p, off), prad, 6, 5, c(0xb4, 0xc0, 0xd6));
            pf.mat = M::STONE; pf.emissive = 0.06; m3::set_alpha(&mut pf, (55.0 + hash(ji, 5) * 170.0) as u8); // dithered density
            s.add(pf, Float { amp: 0.3 + hash(ji, 6) * 0.7, speed: 0.2 + hash(ji, 7) * 0.4, phase: hash(ji, 8) * 6.28, drift: cdrift * (0.6 + hash(ji, 9) * 0.8) });
        }
    }
    // FAR CLOUDS — a distant ring band, smaller + dimmer, for depth on the bigger map
    for k in 0..40 {
        let ki = k as i32 + 1000;
        let a = hash(ki, 2) * TAU; let rad = (34.0 + hash(ki, 3) * 30.0) * ms;
        let p = v(a.cos() * rad, hash(ki, 4) * 12.0, a.sin() * rad);
        let r = 1.4 + hash(ki, 5) * 2.2;
        for j in 0..16 {
            let ji = ki * 53 + j as i32;
            let aa = hash(ji, 2) * TAU; let bb = (hash(ji, 3) * 2.0 - 1.0).asin();
            let off = v(aa.cos() * bb.cos() * r * 1.6, bb.sin() * r * 0.5, aa.sin() * bb.cos() * r * 1.6);
            let mut pf = m3::uv_sphere(addv(p, off), 0.3 + hash(ji, 4) * 0.4, 6, 5, c(0x9a, 0xa6, 0xbe));
            pf.mat = M::STONE; pf.emissive = 0.05; m3::set_alpha(&mut pf, (40.0 + hash(ji, 5) * 90.0) as u8);
            s.add(pf, Float { amp: 0.3, speed: 0.15, phase: hash(ji, 8) * 6.28, drift: 1.0 });
        }
    }

    let pr = env_params();
    // WISPY drifting veils — thin, stretched, faint cloud sheets that read as moving fog high in the open sky
    for k in 0..18 {
        let ki = k as i32 + 200;
        let a = hash(ki, 2) * TAU; let rad = 6.0 + hash(ki, 3) * 32.0;
        let p = v(a.cos() * rad, -1.0 + hash(ki, 4) * 13.0, a.sin() * rad);
        let mut wisp = m3::uv_sphere(p, 1.0, 12, 7, c(0xa2, 0xae, 0xc8));
        m3::scale3(&mut wisp, 2.6 + hash(ki, 5) * 2.5, 0.32, 1.4 + hash(ki, 6) * 1.4); // stretched flat veil
        wisp.mat = M::STONE; wisp.emissive = 0.06; m3::set_alpha(&mut wisp, 120);       // faint + translucent
        s.add(wisp, Float { amp: 0.4, speed: 0.18 + hash(ki, 8) * 0.2, phase: hash(ki, 10) * 6.28, drift: 1.3 });
    }

    // MOON + STARS — the night variation: a pale cool disc high up with a soft halo, and faint stars. Reads at
    // night (cool tod light); by day it's just a pale moon in the blue. (Moonlight tint comes from the tod grade.)
    let moon_p = v(9.0, 13.0, -7.0);
    let mut moon = m3::disc(moon_p, 1.7, 44, c(0xe8, 0xee, 0xf6)); moon.mat = M::STONE; moon.emissive = 1.5; s.add(moon, Float::STILL);
    let mut halo = m3::disc(addv(moon_p, v(0.0, 0.0, 0.1)), 2.6, 44, c(0xbf, 0xd2, 0xf0)); halo.mat = M::STONE; halo.emissive = 0.5; m3::set_alpha(&mut halo, 90); s.add(halo, Float::STILL);
    for k in 0..60 {
        let ki = k as i32 + 400;
        let a = hash(ki, 1) * TAU; let rad = 8.0 + hash(ki, 2) * 12.0;
        let p = v(a.cos() * rad, 6.0 + hash(ki, 3) * 9.0, a.sin() * rad - 4.0);
        let mut star = m3::uv_sphere(p, 0.04 + hash(ki, 4) * 0.04, 5, 4, c(0xea, 0xf2, 0xff));
        star.mat = M::MOSS; star.emissive = 1.6; s.add(star, Float { amp: 0.05, speed: 0.4, phase: hash(ki, 5) * 6.28, drift: 0.0 });
    }

    // STORM CELLS with CONTENT-TUNED lightning — the bolt hue is graph-derived (one storm palette per
    // conversation, like Mistfall's unified rune colour), so different logs storm in different colours.
    let bolt_pal = [c(0x9c, 0xd8, 0xff), c(0xc8, 0x9a, 0xff), c(0xff, 0xe2, 0x88), c(0x8a, 0xff, 0xd2), c(0xff, 0x9a, 0xc4)];
    let bolt_col = bolt_pal[(pr.seed as usize) % bolt_pal.len()];
    let storms = 3 + (pr.seed % 2) as usize;
    for k in 0..storms {
        let ki = k as i32 + 500;
        let a = hash(ki, 1) * TAU; let rad = 4.0 + hash(ki, 2) * 9.0;
        let cx = a.cos() * rad; let cz = a.sin() * rad; let cy = 7.0 + hash(ki, 3) * 5.0;
        // a darker, denser storm head
        for j in 0..5 {
            let ji = ki * 11 + j;
            let off = v((hash(ji, 1) - 0.5) * 3.0, (hash(ji, 2) - 0.5) * 1.0, (hash(ji, 3) - 0.5) * 3.0);
            let mut pf = m3::uv_sphere(addv(v(cx, cy, cz), off), 1.2 + hash(ji, 4) * 1.2, 12, 9, c(0x6e, 0x78, 0x8c));
            m3::tint_up(&mut pf, c(0x8a, 0x94, 0xa8), c(0x44, 0x4c, 0x60)); pf.mat = M::STONE; pf.emissive = 0.05; s.add(pf, Float { amp: 0.4, speed: 0.2, phase: hash(ji, 5) * 6.28, drift: 0.7 });
        }
        // bolts: a few downward strikes + an inter-cloud arc
        let strikes = 2 + (hash(ki, 6) * 2.0) as usize;
        for b in 0..strikes {
            let bi = ki * 7 + b as i32;
            let end = v(cx + (hash(bi, 1) - 0.5) * 5.0, cy - 4.0 - hash(bi, 2) * 4.0, cz + (hash(bi, 3) - 0.5) * 5.0);
            lightning_arc(&mut s, v(cx, cy - 0.5, cz), end, bolt_col, (bi as u32).wrapping_mul(0x9E37));
        }
    }
    s
}

/// SLATE — a contemplative slate-quarry moor: a loose HENGE of standing stones around scattered BOULDERS and
/// scree, stacked CAIRNS, stepped STRATA LEDGES at the rim, and faint glowing QUARTZ VEINS seaming the floor.
/// Density scales gently with the graph size (expand-with-graph). enclose() still wraps it with the stone shell.
fn slate() -> Scene {
    use std::f32::consts::TAU;
    let mut s = Scene::new();
    let pr = env_params();
    let y0 = -4.4_f32;
    let lo = c(0x34, 0x3c, 0x46);
    let hi = c(0x72, 0x7e, 0x8c);
    let vein = c(0x7a, 0xb6, 0xd0);
    let rh = |k: u32| -> f32 { let x = (k.wrapping_mul(2_654_435_761).wrapping_add(pr.seed)).wrapping_mul(40_503); ((x >> 8) & 0xFF_FFFF) as f32 / 16_777_215.0 };
    // a loose HENGE of standing stones (menhirs)
    let menhirs = (8 + pr.nodes / 12).clamp(8, 14);
    for k in 0..menhirs {
        let a = k as f32 / menhirs as f32 * TAU + rh(k * 7) * 0.3;
        let rr = 15.0 + (rh(k * 13) - 0.5) * 5.0;
        standing_stone(&mut s, a.cos() * rr, a.sin() * rr, y0, 3.2 + rh(k * 17) * 3.2, pr.seed ^ (k * 0x51));
    }
    // scattered BOULDERS, each with a little scree skirt
    let boulders = (16 + pr.nodes / 3).clamp(16, 40);
    for k in 0..boulders {
        let a = rh(k * 3 + 1) * TAU;
        let rr = 4.0 + rh(k * 5 + 2) * 26.0;
        let (x, z) = (a.cos() * rr, a.sin() * rr);
        let rad = 0.6 + rh(k * 9 + 3).powi(2) * 2.0;
        let mut bo = m3::rock(v(x, y0 + rad * 0.35, z), rad, pr.seed ^ (k * 0x9E37), mix_rgb(lo, hi, rh(k * 11)));
        bo.mat = M::STONE; s.add(bo, Float::STILL);
        contact_shadow(&mut s, x, z, rad * 1.3, y0, 110);
        for j in 0..3u32 {
            let aj = rh(k * 23 + j) * TAU;
            let dj = rad + 0.3 + rh(k * 31 + j) * 0.8;
            let mut sc = m3::rock(v(x + aj.cos() * dj, y0 + 0.12, z + aj.sin() * dj), 0.18 + rh(k * 37 + j) * 0.22, pr.seed ^ (k * 7 + j), lo);
            sc.mat = M::STONE; s.add(sc, Float::STILL);
        }
    }
    // CAIRNS — tapered stacks of stones
    for k in 0..5u32 {
        let a = k as f32 / 5.0 * TAU + 0.4;
        let rr = 9.0 + rh(k * 41) * 14.0;
        let (cx, cz) = (a.cos() * rr, a.sin() * rr);
        let stack = 3 + (rh(k * 43) * 3.0) as u32;
        let mut yy = y0;
        for j in 0..stack {
            let rad = (0.7 - j as f32 * 0.12).max(0.22);
            let mut st = m3::rock(v(cx, yy + rad * 0.6, cz), rad, pr.seed ^ (k * 100 + j), mix_rgb(lo, hi, 0.4 + j as f32 * 0.12));
            st.mat = M::STONE; s.add(st, Float::STILL);
            yy += rad * 1.05;
        }
        contact_shadow(&mut s, cx, cz, 1.0, y0, 120);
    }
    // STRATA LEDGES — stepped slab shelves near the rim
    for k in 0..4u32 {
        let a = k as f32 / 4.0 * TAU + 0.8;
        let (lx, lz) = (a.cos() * 30.0, a.sin() * 30.0);
        for j in 0..4u32 {
            let rad = 4.5 - j as f32 * 0.9;
            let f = 1.0 - j as f32 * 0.04;
            let mut slab = m3::disc(v(lx * f, y0 + 0.2 + j as f32 * 0.45, lz * f), rad, 28, mix_rgb(lo, hi, 0.3 + j as f32 * 0.15));
            slab.mat = M::STONE; s.add(slab, Float::STILL);
        }
    }
    // QUARTZ VEINS — faint glowing seams across the floor
    for k in 0..7u32 {
        let a0 = rh(k * 53) * TAU;
        let a1 = a0 + 0.6 + rh(k * 59) * 1.2;
        let (r0v, r1v) = (3.0 + rh(k * 61) * 8.0, 12.0 + rh(k * 67) * 16.0);
        let mut vn = m3::tube(v(a0.cos() * r0v, y0 + 0.04, a0.sin() * r0v), v(a1.cos() * r1v, y0 + 0.04, a1.sin() * r1v), 0.06, 6, vein);
        vn.mat = M::STONE; vn.emissive = 0.32; s.add(vn, Float::STILL);
    }
    // central cracked SLAB with a glowing vein cross
    let mut slab = m3::disc(v(0.0, y0 + 0.06, 0.0), 4.0, 40, mix_rgb(lo, hi, 0.5));
    slab.mat = M::STONE; s.add(slab, Float::STILL);
    for k in 0..2u32 {
        let a = k as f32 * 1.57 + 0.4;
        let mut cr = m3::tube(v(-a.cos() * 3.5, y0 + 0.09, -a.sin() * 3.5), v(a.cos() * 3.5, y0 + 0.09, a.sin() * 3.5), 0.05, 5, vein);
        cr.mat = M::STONE; cr.emissive = 0.4; s.add(cr, Float::STILL);
    }
    // DRIFTING DUST MOTES — faint glints catching the light above the henge, slowly rising/falling. Gives the
    // static stone some ambient life (and ensures the scene has animated geometry).
    let td = tune_density();
    for k in 0..((18.0 * td) as i32) {
        let a = hash(k, 210) * std::f32::consts::TAU;
        let rr = 1.0 + (hash(k, 211)).sqrt() * 7.0;
        let mote = m3::uv_sphere(v(a.cos() * rr, y0 + 0.6 + hash(k, 212) * 3.4, a.sin() * rr), 0.025 + hash(k, 213) * 0.03, 6, 5, mix_rgb(hi, c(0xff, 0xf2, 0xd0), 0.6));
        let mut mote = mote; mote.emissive = 0.5;
        s.add(mote, Float { amp: 0.4 + hash(k, 214) * 0.7, speed: 0.25 + hash(k, 215) * 0.4, phase: hash(k, 216) * 6.28, drift: 0.5 });
    }
    s
}

/// MAP RADIUS — a DOUBLED base that GROWS to contain the conversation graph. `flow` spreads the graph ~±ext_x
/// along x (and ~±nodes·0.5 for a long chain); we size the shell/terrain to enclose that with generous margin,
/// so the room scales up as the chat grows and the graph always fits. Self-enclosing scenes derive radii from
/// this too. Clamped so a tiny chat still gets a real room and a huge one stays bounded.
fn map_radius() -> f32 {
    let pr = env_params();
    let reach = (pr.ext_x.max(pr.ext_z) as f32).max(pr.nodes as f32 * 0.55);
    let tune = std::env::var("LOGSCOPE_TUNE_SCALE").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0);
    ((reach * 2.4 + 40.0).clamp(80.0, 260.0) * tune).max(50.0)
}

/// Per-scene scatter-density multiplier (LOGSCOPE_TUNE_DENSITY, default 1.0). Lets the harness/Markov search
/// explore "more vs less stuff" without editing the tree. Clamped so a chain can't request absurd counts.
fn tune_density() -> f32 {
    std::env::var("LOGSCOPE_TUNE_DENSITY").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0).clamp(0.3, 3.0)
}

fn build(palette_name: &str) -> Scene {
    clear_colliders(); // the generator repopulates (only halls with solids emit any)
    // ROOM-VARIETY MODE: LOGSCOPE_ROOM=<n> bypasses the themed scene and builds one of the procedural
    // "inspired rooms" so we can survey a broad range of mind-spaces to choose from.
    if let Some(idx) = std::env::var("LOGSCOPE_ROOM").ok().and_then(|v| v.trim().parse::<usize>().ok()) {
        return room_variant(idx);
    }
    // DRAGON DEBUG: an isolated close-up of the procedural sea-dragon head (frontal / 3-4 / profile) on a
    // plain floor, muted stone tint so llvmpipe doesn't blow it out — purely for verifying the geometry.
    // DRAGON DEBUG: an isolated close-up of the procedural sea-dragon head on a plain floor, muted stone tint
    // so llvmpipe doesn't blow it out — purely for verifying/tuning the geometry. LOGSCOPE_DRAGON_DEBUG=<scale>
    // sets the head scale (default 16, sized to the overview frame); the head is lifted clear of the node strand.
    if let Ok(dbg) = std::env::var("LOGSCOPE_DRAGON_DEBUG") {
        let sc = dbg.trim().parse::<f32>().ok().filter(|v| *v > 0.0).unwrap_or(16.0);
        let mut s = Scene::new();
        let hi = c(0x9c, 0x98, 0x8e); let lo = c(0x54, 0x50, 0x4a);
        let mut floor = m3::disc(v(0.0, -sc * 0.5, 0.0), sc * 4.0, 48, c(0x10, 0x16, 0x1c));
        m3::tint_up(&mut floor, c(0x1a, 0x22, 0x2a), c(0x08, 0x0c, 0x10)); floor.mat = M::STONE; s.add(floor, Float::STILL);
        for (m, f) in dragon_head(v(0.0, sc * 0.6, 0.0), 0.0, sc, 7, hi, lo) { s.add(m, f); }
        return s;
    }
    // LIBNODE DEBUG: isolated page / book / scroll (Old-Library node bodies) on a plain floor for verifying
    // the geometry. LOGSCOPE_LIBNODE_DEBUG=<scale> (default 6), lifted clear of the synthetic node strand.
    if let Ok(_dbg) = std::env::var("LOGSCOPE_CRITTER_DEBUG") {
        let mut s = Scene::new();
        let mut floor = m3::disc(v(0.0, 0.0, 0.0), 2.4, 40, c(0x26, 0x34, 0x18));
        m3::tint_up(&mut floor, c(0x3e, 0x52, 0x22), c(0x1a, 0x26, 0x10)); floor.mat = M::MOSS; s.add(floor, Float::STILL);
        critter(&mut s, 0, v(-1.1, 0.0, 0.0), 0.6, 7);  // rabbit
        critter(&mut s, 1, v(0.2, 0.0, 0.0), 2.4, 9);   // cat
        swinger(&mut s, v(1.3, 1.7, 0.0), 1.0, 3);      // hanging creature (anchor above floor)
        return s;
    }
    if let Ok(dbg) = std::env::var("LOGSCOPE_LIBNODE_DEBUG") {
        let sc = dbg.trim().parse::<f32>().ok().filter(|v| *v > 0.0).unwrap_or(7.0);
        let mut s = Scene::new();
        let muted = c(0x96, 0x8c, 0x7a);   // STONE+muted so llvmpipe doesn't blow it out — verifying FORM only
        for (i, (kind, unroll)) in [(0u32, 0.0f32), (1, 0.0), (2, 0.85)].into_iter().enumerate() {
            let x = (i as f32 - 1.0) * sc * 2.6;
            let mut nm = m3::library_node(kind, v(x, sc * 1.1, 0.0), sc, unroll, 3 + i as u32 * 7, muted);
            nm.mat = M::STONE; s.add(nm, Float::STILL);
        }
        return s;
    }
    let mut s = match palette_name {
        "Canopy" => canopy(),
        "Mistfall" => mistfall(),
        "Archive" => archive(),
        "Reading Room" => reading_room(),
        "Folding@home" => foldingathome(),
        "Slate" => slate(),
        "Old Library" => old_library(),
        "Atlantis" => atlantis(),
        "Cloudscape" => cloudscape(),
        "Forest" => forest(),
        "Treehouse" => treehouse(),
        "Mansion Garden" => mansion_garden(),
        _ => islands(),
    };
    // Atlantis is SELF-ENCLOSING (its own ribbed glass dome + perimeter wall + open ocean beyond), so it does
    // NOT get the generic outer room — that second shell was the "giant structure" dominating every overview.
    if palette_name != "Atlantis" && palette_name != "Cloudscape" && palette_name != "Forest" && palette_name != "Treehouse" && palette_name != "Canopy" && palette_name != "Mansion Garden" {
        let (mat, lo, hi, shelf) = match palette_name {
            "Canopy" | "Old Library" => (M::WOOD, c(0x2c, 0x20, 0x12), c(0x5a, 0x44, 0x28), true),
            "Archive" => (M::STONE, c(0x66, 0x5e, 0x4e), c(0xa8, 0x9e, 0x88), false), // cathedral: pale marble shell, no shelves
            "Reading Room" => (M::STONE, c(0x4a, 0x44, 0x3a), c(0x9a, 0x92, 0x80), true),
            "Mistfall" => (M::STONE, c(0x1c, 0x26, 0x2c), c(0x44, 0x52, 0x5a), false),
            "Folding@home" => (M::STONE, c(0x16, 0x1c, 0x26), c(0x32, 0x3c, 0x4a), false),
            _ => (M::STONE, c(0x24, 0x20, 0x18), c(0x4c, 0x44, 0x36), false),
        };
        enclose(&mut s, mat, lo, hi, shelf, map_radius());
    }
    if std::env::var("LOGSCOPE_DEBUG_LABELS").is_ok() { debug_dump_labels(&s, palette_name); }
    s
}

/// DEBUG SCENE LABELER (LOGSCOPE_DEBUG_LABELS=1): dumps a per-mesh inventory — index, centroid, bounding box,
/// vertex count, material, emissive, and a shape guess — to /tmp/scene_labels_<theme>.txt (+ a stderr summary).
/// Lets any scene element be identified by eye (esp. wide-flat ring/disc geometry) without guesswork.
fn debug_dump_labels(s: &Scene, theme: &str) {
    use std::io::Write;
    let path = format!("/tmp/scene_labels_{}.txt", theme.replace(' ', "_"));
    let Ok(mut f) = std::fs::File::create(&path) else { return };
    let _ = writeln!(f, "# {} — {} meshes", theme, s.meshes.len());
    let _ = writeln!(f, "# idx | centroid x,y,z | bbox w,h,d | verts | mat | emis | shape");
    let mut rings = 0u32;
    for (i, m) in s.meshes.iter().enumerate() {
        if m.verts.is_empty() { continue; }
        let (mut mnx, mut mny, mut mnz) = (m.verts[0].x, m.verts[0].y, m.verts[0].z);
        let (mut mxx, mut mxy, mut mxz) = (mnx, mny, mnz);
        let (mut cx, mut cy, mut cz) = (0.0f32, 0.0f32, 0.0f32);
        for p in &m.verts {
            cx += p.x; cy += p.y; cz += p.z;
            mnx = mnx.min(p.x); mny = mny.min(p.y); mnz = mnz.min(p.z);
            mxx = mxx.max(p.x); mxy = mxy.max(p.y); mxz = mxz.max(p.z);
        }
        let n = m.verts.len() as f32; cx /= n; cy /= n; cz /= n;
        let (w, h, d) = (mxx - mnx, mxy - mny, mxz - mnz);
        let shape = if h < 0.18 * w.max(d) && w.max(d) > 4.0 { rings += 1; "WIDE-FLAT (ring/disc/annulus)" }
            else if (w - h).abs() < 0.5 * w.max(h) && (d - h).abs() < 0.5 * d.max(h) { "blob/sphere" }
            else if h > 2.0 * w.max(d) { "tall (tube/column)" }
            else { "other" };
        let _ = writeln!(f, "{:4} | {:7.1},{:7.1},{:7.1} | {:5.1},{:5.1},{:5.1} | {:5} | m{:.0} | e{:.2} | {}",
            i, cx, cy, cz, w, h, d, m.verts.len(), m.mat, m.emissive, shape);
    }
    eprintln!("[debug] scene '{}': {} meshes ({} wide-flat ring/disc) -> {}", theme, s.meshes.len(), rings, path);
}

// ───────────────────────── PROCEDURAL ROOM VARIETY ─────────────────────────
// A survey set of "inspired rooms" so the mind-palace can offer many distinct workspaces. Each is assembled
// from a compact spec (floor shape · perimeter · roof · focal object · accent · prop scatter) so the range is
// broad without 24 bespoke builders. Select with LOGSCOPE_ROOM=<n>.
pub const ROOM_COUNT: usize = 16;
pub fn room_name(idx: usize) -> &'static str { ROOMS[idx % ROOMS.len()].0 }

// (name, floor 0round/1square/2nave, floor_rgb, floor_mat, perim 0none/1ring/2rows/3walls/4arcade, perim_rgb,
//  roof 0sky/1flat/2dome/3vault, focal 0plinth/1altar/2fountain/3tree/4brazier/5obelisk/6orrery/7lectern, accent, prop 0none/1crystal/2book/3lantern/4plant/5banner)
const ROOMS: [(&str, u8, [u8; 3], f32, u8, [u8; 3], u8, u8, [u8; 3], u8); ROOM_COUNT] = [
    ("Cathedral",    2, [0xc8, 0xc2, 0xb4], M::STONE, 2, [0xe0, 0xdc, 0xd0], 3, 1, [0xff, 0xe6, 0xb0], 5),
    ("Observatory",  0, [0x1a, 0x1e, 0x2c], M::STONE, 1, [0x3a, 0x40, 0x52], 2, 6, [0x88, 0xc6, 0xff], 1),
    ("Greenhouse",   1, [0x3a, 0x46, 0x30], M::MOSS,  4, [0xcf, 0xe6, 0xd8], 0, 3, [0x9a, 0xe0, 0x60], 4),
    ("Crypt",        0, [0x22, 0x20, 0x24], M::STONE, 1, [0x40, 0x3c, 0x44], 3, 0, [0x6a, 0x90, 0xa0], 3),
    ("Scriptorium",  2, [0x4a, 0x38, 0x22], M::WOOD,  2, [0x6e, 0x52, 0x30], 1, 7, [0xff, 0xd0, 0x80], 2),
    ("Foundry",      1, [0x26, 0x22, 0x20], M::STONE, 3, [0x44, 0x3a, 0x32], 1, 4, [0xff, 0x7a, 0x2a], 0),
    ("Zen Garden",   1, [0xcf, 0xc6, 0xae], M::PAPER, 0, [0x9a, 0x90, 0x7a], 0, 0, [0x86, 0xb0, 0x86], 1),
    ("Lighthouse",   0, [0x2c, 0x3a, 0x42], M::STONE, 1, [0x6a, 0x6e, 0x70], 2, 4, [0xff, 0xd2, 0x7a], 0),
    ("Temple",       0, [0x52, 0x46, 0x30], M::STONE, 1, [0x8a, 0x76, 0x50], 0, 4, [0xff, 0xb0, 0x50], 5),
    ("Conservatory", 1, [0x4a, 0x36, 0x22], M::WOOD,  4, [0x7a, 0x5c, 0x36], 1, 0, [0xff, 0xd6, 0xa0], 4),
    ("Cavern",       0, [0x20, 0x22, 0x26], M::STONE, 0, [0x3a, 0x3e, 0x44], 0, 0, [0x70, 0xd0, 0xc0], 1),
    ("Planetarium",  0, [0x14, 0x16, 0x22], M::STONE, 3, [0x2a, 0x2e, 0x3e], 2, 6, [0xc0, 0xb0, 0xff], 1),
    ("Dojo",         1, [0x5a, 0x42, 0x28], M::WOOD,  3, [0xd8, 0xcf, 0xb8], 1, 0, [0xff, 0xe0, 0xa0], 5),
    ("Vault",        0, [0x2a, 0x2e, 0x32], M::STONE, 3, [0x52, 0x58, 0x60], 1, 5, [0x9a, 0xc0, 0xd0], 0),
    ("Atrium",       1, [0x86, 0x8e, 0x86], M::STONE, 4, [0xc0, 0xc6, 0xc0], 0, 2, [0x90, 0xd0, 0xe0], 4),
    ("Hearth Hall",  2, [0x4a, 0x3a, 0x2a], M::WOOD,  2, [0x6e, 0x54, 0x38], 3, 4, [0xff, 0x9a, 0x40], 5),
];

fn scale_rgb(col: Color32, f: f32) -> Color32 {
    let g = |x: u8| (x as f32 * f).clamp(0.0, 255.0) as u8;
    Color32::from_rgb(g(col.r()), g(col.g()), g(col.b()))
}

fn room_column(s: &mut Scene, x: f32, z: f32, yf: f32, h: f32, lo: Color32, hi: Color32, mat: f32) {
    let yc = yf + h;
    let mut shaft = m3::tube(v(x, yf, z), v(x, yc - 0.3, z), 0.42, 18, lo);
    m3::tint_height(&mut shaft, yf, yc, lo, hi); shaft.mat = mat; s.add(shaft, Float::STILL);
    let mut base = m3::tube(v(x, yf, z), v(x, yf + 0.4, z), 0.58, 14, hi); base.mat = mat; s.add(base, Float::STILL);
    let mut cap = m3::tube(v(x, yc - 0.4, z), v(x, yc, z), 0.58, 14, hi); cap.mat = mat; s.add(cap, Float::STILL);
    contact_shadow(s, x, z, 0.95, yf, 120);
    push_collider(Collider::Pillar { x, z, r: 0.6, y0: yf, y1: yc });
}

#[allow(clippy::too_many_arguments)]
fn build_room(floor: u8, fcol: [u8; 3], fmat: f32, perim: u8, pcol: [u8; 3], roof: u8, focal: u8, accent: [u8; 3], props: u8) -> Scene {
    use std::f32::consts::TAU;
    let mut s = Scene::new();
    let yf = -3.2_f32;
    let fc = c(fcol[0], fcol[1], fcol[2]);
    let pc = c(pcol[0], pcol[1], pcol[2]);
    let ac = c(accent[0], accent[1], accent[2]);
    let (lo, hi) = (scale_rgb(pc, 0.6), pc);
    let (half, depth) = match floor { 1 => (9.0_f32, 9.0_f32), 2 => (7.0, 13.0), _ => (11.0, 11.0) };
    // FLOOR
    if floor == 0 {
        let mut fl = m3::disc(v(0.0, yf, 0.0), half, 72, fc);
        m3::tint_up(&mut fl, scale_rgb(fc, 1.3), scale_rgb(fc, 0.7)); fl.mat = fmat; s.add(fl, Float::STILL);
    } else {
        let mut fl = m3::panel(v(0.0, yf, 0.0), v(1.0, 0.0, 0.0), v(0.0, 0.0, 1.0), half * 2.0, depth * 2.0, fc);
        m3::tint_up(&mut fl, scale_rgb(fc, 1.3), scale_rgb(fc, 0.7)); fl.mat = fmat; s.add(fl, Float::STILL);
    }
    let yc = yf + 8.0;
    // PERIMETER
    match perim {
        1 => for k in 0..10 { let a = (k as f32 / 10.0) * TAU + 0.2; room_column(&mut s, a.cos() * (half - 1.0), a.sin() * (half - 1.0), yf, 8.0, lo, hi, fmat); }
        2 => for side in [-1.0_f32, 1.0] { for k in 0..6 { let zz = -depth + 1.6 + (k as f32 / 5.0) * (2.0 * depth - 3.2); room_column(&mut s, (half - 1.2) * side, zz, yf, 8.5, lo, hi, fmat); } }
        3 => { let hz = if floor == 2 { depth } else { half };
            for (cen, right, w) in [ (v(0.0, yc * 0.0 + (yf + yc) * 0.5, -hz), v(1.0, 0.0, 0.0), half * 2.0), (v(0.0, (yf + yc) * 0.5, hz), v(1.0, 0.0, 0.0), half * 2.0), (v(-half, (yf + yc) * 0.5, 0.0), v(0.0, 0.0, 1.0), hz * 2.0), (v(half, (yf + yc) * 0.5, 0.0), v(0.0, 0.0, 1.0), hz * 2.0) ] {
                let mut wmesh = m3::panel(cen, right, v(0.0, 1.0, 0.0), w, 8.0, lo); m3::tint_height(&mut wmesh, yf, yc, lo, hi); wmesh.mat = fmat; s.add(wmesh, Float::STILL); } }
        4 => { for k in 0..12 { let a = (k as f32 / 12.0) * TAU; room_column(&mut s, a.cos() * (half - 0.8), a.sin() * (half - 0.8), yf, 7.0, lo, hi, fmat); }
            let mut ring = m3::torus(v(0.0, yf + 7.0, 0.0), half - 0.8, 0.22, 64, 8, hi); ring.mat = fmat; s.add(ring, Float::STILL); }
        _ => {}
    }
    // ROOF
    match roof {
        1 => { let mut rf = if floor == 0 { m3::disc(v(0.0, yc, 0.0), half, 64, hi) } else { m3::panel(v(0.0, yc, 0.0), v(1.0, 0.0, 0.0), v(0.0, 0.0, 1.0), half * 2.0, if floor == 2 { depth * 2.0 } else { half * 2.0 }, hi) }; rf.mat = fmat; s.add(rf, Float::STILL); }
        2 => { let mut dm = m3::dome_cap(v(0.0, yc, 0.0), half * 0.94, half * 0.6, half * 0.1, 48, 12, scale_rgb(pc, 1.1)); dm.mat = fmat; s.add(dm, Float::STILL); }
        3 => for k in 0..7 { let zz = -depth + (k as f32 / 6.0) * 2.0 * depth; for sx in [-1.0_f32, 1.0] { let mut rib = m3::tube(v(half * sx, yc - 0.5, zz), v(0.0, yc + 1.8, zz), 0.18, 8, hi); rib.mat = fmat; s.add(rib, Float::STILL); } }
        _ => {}
    }
    // FOCAL
    match focal {
        0 => { let mut p = m3::tube(v(0.0, yf, 0.0), v(0.0, yf + 1.0, 0.0), 1.4, 24, c(0x6a, 0x64, 0x58)); p.mat = M::STONE; s.add(p, Float::STILL); }
        1 => { let mut a = m3::tube(v(0.0, yf, 0.0), v(0.0, yf + 1.2, 0.0), 1.0, 6, scale_rgb(pc, 1.1)); a.mat = fmat; s.add(a, Float::STILL); let mut fire = m3::uv_sphere(v(0.0, yf + 1.6, 0.0), 0.5, 16, 12, ac); fire.emissive = 1.4; fire.mat = M::STONE; s.add(fire, Float { amp: 0.06, speed: 2.0, phase: 0.0, drift: 0.0 }); }
        2 => { let mut bowl = m3::disc(v(0.0, yf + 0.5, 0.0), 2.0, 32, c(0x2a, 0x4c, 0x52)); bowl.mat = M::WATER; bowl.emissive = 0.12; s.add(bowl, Float::STILL); let mut col = m3::tube(v(0.0, yf, 0.0), v(0.0, yf + 0.5, 0.0), 0.5, 12, c(0x4a, 0x58, 0x5e)); col.mat = M::STONE; s.add(col, Float::STILL); }
        3 => { let mut trunk = m3::tube(v(0.0, yf, 0.0), v(0.0, yf + 3.0, 0.0), 0.4, 12, c(0x4a, 0x36, 0x22)); trunk.mat = M::WOOD; s.add(trunk, Float::STILL); let mut crown = m3::leaf_cluster(v(0.0, yf + 4.0, 0.0), 2.2, 77, c(0x4a, 0x7a, 0x32)); crown.mat = M::MOSS; s.add(crown, Float { amp: 0.1, speed: 0.6, phase: 0.0, drift: 0.2 }); }
        4 => { let mut br = m3::tube(v(0.0, yf, 0.0), v(0.0, yf + 0.8, 0.0), 0.7, 12, c(0x3a, 0x32, 0x2c)); br.mat = M::STONE; s.add(br, Float::STILL); let mut fire = m3::uv_sphere(v(0.0, yf + 1.1, 0.0), 0.62, 16, 12, ac); fire.emissive = 1.9; fire.mat = M::STONE; s.add(fire, Float { amp: 0.08, speed: 2.4, phase: 0.0, drift: 0.0 }); }
        5 => { let mut ob = m3::tube(v(0.0, yf, 0.0), v(0.0, yf + 4.5, 0.0), 0.5, 4, scale_rgb(ac, 0.7)); ob.mat = M::STONE; ob.emissive = 0.25; s.add(ob, Float::STILL); }
        6 => { let mut core = m3::uv_sphere(v(0.0, yf + 2.2, 0.0), 0.5, 18, 14, ac); core.emissive = 1.3; core.mat = M::STONE; s.add(core, Float { amp: 0.05, speed: 1.0, phase: 0.0, drift: 0.0 }); for k in 0..3 { let mut ring = m3::torus(v(0.0, yf + 2.2, 0.0), 1.0 + k as f32 * 0.55, 0.05, 48, 6, scale_rgb(ac, 1.2)); ring.mat = M::STONE; ring.emissive = 0.45; s.add(ring, Float { amp: 0.12, speed: 0.6 + k as f32 * 0.3, phase: k as f32, drift: 0.0 }); } }
        7 => { let mut stand = m3::tube(v(0.0, yf, 0.0), v(0.0, yf + 1.4, 0.0), 0.3, 10, c(0x46, 0x32, 0x1c)); stand.mat = M::WOOD; s.add(stand, Float::STILL); let mut tome = m3::paper_fold(v(0.0, yf + 1.6, 0.0), 1.0, 0.4, c(0xf2, 0xe6, 0xc6)); tome.mat = M::PAPER; tome.emissive = 0.5; s.add(tome, Float::STILL); }
        _ => {}
    }
    // PROPS
    match props {
        1 => for k in 0..14 { let a = hash(k, 2) * TAU; let rad = 2.0 + hash(k, 3) * (half - 2.5); let mut cr = m3::rock(v(a.cos() * rad, yf + 0.2 + hash(k, 4) * 0.5, a.sin() * rad), 0.2 + hash(k, 5) * 0.3, (k * 13) as u32, scale_rgb(ac, 0.85)); cr.mat = M::STONE; cr.emissive = 0.5; s.add(cr, Float::STILL); }
        2 => for k in 0..16 { let a = hash(k, 2) * TAU; let rad = 2.0 + hash(k, 3) * (half - 2.0); let mut bk = m3::paper_fold(v(a.cos() * rad, yf + 1.0 + hash(k, 4) * 5.0, a.sin() * rad), 0.3, 0.2, c(0xe8, 0xd6, 0xae)); bk.mat = M::PAPER; s.add(bk, Float { amp: 0.4, speed: 0.6, phase: hash(k, 7) * 6.28, drift: 0.5 }); }
        3 => for k in 0..8 { let a = (k as f32 / 8.0) * TAU; let mut lan = m3::uv_sphere(v(a.cos() * (half - 1.5), yc - 2.0, a.sin() * (half - 1.5)), 0.22, 12, 9, ac); lan.emissive = 1.2; lan.mat = M::STONE; s.add(lan, Float { amp: 0.1, speed: 0.5, phase: k as f32, drift: 0.0 }); }
        4 => for k in 0..12 { let a = hash(k, 2) * TAU; let rad = 2.5 + hash(k, 3) * (half - 3.0); let mut pl = m3::leaf_cluster(v(a.cos() * rad, yf + 0.6, a.sin() * rad), 0.7, (k * 7) as u32, c(0x4a, 0x7a, 0x36)); pl.mat = M::MOSS; s.add(pl, Float { amp: 0.05, speed: 0.7, phase: hash(k, 7) * 6.28, drift: 0.1 }); }
        5 => for k in 0..6 { let a = (k as f32 / 6.0) * TAU; let mut ban = m3::panel(v(a.cos() * (half - 0.6), yc - 3.0, a.sin() * (half - 0.6)), v(-a.sin(), 0.0, a.cos()), v(0.0, 1.0, 0.0), 0.8, 2.6, scale_rgb(ac, 0.9)); ban.mat = M::PAPER; ban.emissive = 0.12; s.add(ban, Float::STILL); }
        _ => {}
    }
    s
}

fn room_variant(idx: usize) -> Scene {
    let r = ROOMS[idx % ROOMS.len()];
    build_room(r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8, r.9)
}

// Conversation-derived parameters that drive PROCEDURAL layout of the hall (size, colonnade, tint, life).
// Set per-frame from the live graph before building the env; the baked scene is re-generated when they change.
#[derive(Clone, Copy, PartialEq)]
pub struct EnvParams { pub nodes: u32, pub branches: u32, pub segments: u32, pub seed: u32, pub ext_x: u32, pub ext_z: u32, pub bear: [u16; 6], pub color_salience: f32 }
impl Default for EnvParams { fn default() -> Self { Self { nodes: 40, branches: 3, segments: 5, seed: 0x51A7_C0DE, ext_x: 18, ext_z: 14, bear: [0; 6], color_salience: 0.0 } } }
thread_local! { static ENV_PARAMS: std::cell::Cell<EnvParams> = const { std::cell::Cell::new(EnvParams { nodes: 40, branches: 3, segments: 5, seed: 0x51A7_C0DE, ext_x: 18, ext_z: 14, bear: [0; 6], color_salience: 0.0 }) }; }
/// Feed the live conversation's shape to the environment generator (call before `with_env`/`render_env`).
pub fn set_env_params(p: EnvParams) { ENV_PARAMS.with(|c| c.set(p)); }
pub fn env_params() -> EnvParams { ENV_PARAMS.with(|c| c.get()) }

/// Atlantis hall semi-axes (rx along the spine, rz across it) — large enough to CONTAIN the conversation
/// flow on each axis. `ext_x/ext_z` are the graph's world-space half-spans (fed from the live layout);
/// we wrap each with margin, never drop below the node-count floor, and clamp the aspect so the hall
/// stays a believable oval rotunda (not a hairline corridor). Shared by atlantis() and enclose().
fn hall_dims(pr: &EnvParams) -> (f32, f32) {
    let floor = 12.0 + 1.05 * (pr.nodes.max(1) as f32).sqrt();
    let mut rx = (pr.ext_x as f32 + 6.0).max(floor).clamp(14.0, 120.0);
    let mut rz = (pr.ext_z as f32 + 6.0).max(floor).clamp(14.0, 120.0);
    // clamp aspect to ≤ 2.6:1 either way so the rotunda reads as a hall, not a slit
    let (lo, hi) = (rx.min(rz), rx.max(rz));
    if hi > lo * 2.6 { let cap = lo * 2.6; if rx > rz { rx = cap; } else { rz = cap; } }
    (rx, rz)
}
/// Mean hall radius (for things kept circular/centred: the crystal, lanterns ring math, etc.).
fn hall_radius(pr: &EnvParams) -> f32 { let (rx, rz) = hall_dims(pr); (rx + rz) * 0.5 }
/// How far the door-wings reach from the hall centre — `region_plan` places wing centres at
/// r + 6.5 + scale·3.2 (scale ≤ ~1.6) and the widest wing (chamber) has radius 6·scale. Used to size
/// the enclosing room. Uses the LONGER axis so the room clears wings on the spine ends.
fn wing_reach(pr: &EnvParams) -> f32 { let (rx, rz) = hall_dims(pr); rx.max(rz) + 22.0 }

/// Collision volumes for the OPAQUE solids of the current environment (columns, central plinth, perimeter
/// wall) — emitted by the generator from the SAME EnvParams that build the geometry. The fly camera carries a
/// bounding sphere and uses these to evade solids, so it never ends up inside a column/plinth (→ black frame).
#[derive(Clone, Copy)]
pub enum Collider {
    /// A vertical solid the camera must stay OUTSIDE (column / plinth), between y0..y1.
    Pillar { x: f32, z: f32, r: f32, y0: f32, y1: f32 },
    /// A perimeter the camera must stay INSIDE (the hall wall).
    Bound { r: f32 },
    /// An OPENING in the perimeter (a door or hallway mouth) at bearing `ang` with angular half-width `half`,
    /// where the camera may cross the Bound — so the fly can travel down hallways and through doors to wings.
    Gate { ang: f32, half: f32 },
}
thread_local! { static COLLIDERS: std::cell::RefCell<Vec<Collider>> = const { std::cell::RefCell::new(Vec::new()) }; }
fn clear_colliders() { COLLIDERS.with(|c| c.borrow_mut().clear()); }
fn push_collider(col: Collider) { COLLIDERS.with(|c| c.borrow_mut().push(col)); }
/// The current environment's collision volumes (for the camera to evade).
pub fn env_colliders() -> Vec<Collider> { COLLIDERS.with(|c| c.borrow().clone()) }

// cached baked scene (rebuilt when the palette, detail variant, OR conversation params change); per-frame only floats update.
thread_local! { static ENV: std::cell::RefCell<Option<(String, u8, EnvParams, Scene)>> = const { std::cell::RefCell::new(None) }; }

/// Build/cache the themed environment for `palette_name`, float it for `t`, and hand the animated meshes to
/// `f` (for the GPU path to pack into a vertex buffer). Geometry is cached; only the float offsets update.
pub fn with_env(palette_name: &str, t: f32, f: impl FnOnce(&[Mesh3])) {
    ENV.with(|cell| {
        let mut b = cell.borrow_mut();
        let var = crate::theme::variant();
        let pr = env_params();
        if b.as_ref().map_or(true, |(n, v, p, _)| n != palette_name || *v != var || *p != pr) {
            *b = Some((palette_name.to_string(), var, pr, build(palette_name)));
        }
        let scene = &mut b.as_mut().unwrap().3;
        scene.animate(t);
        f(&scene.meshes);
    });
}

/// Build/cache the themed environment for `palette_name`, float it for time `t`, and paint it through `cam`
/// (same `cx,cy,scale` as the graph) with `light`. Draw this BEFORE the graph so the nodes sit in front.
pub fn render_env(painter: &egui::Painter, cam: &OrbitCam, cx: f32, cy: f32, scale: f32, palette_name: &str, light: &Light, t: f32) {
    ENV.with(|cell| {
        let mut b = cell.borrow_mut();
        let var = crate::theme::variant();
        let pr = env_params();
        if b.as_ref().map_or(true, |(n, v, p, _)| n != palette_name || *v != var || *p != pr) {
            *b = Some((palette_name.to_string(), var, pr, build(palette_name)));
        }
        let scene = &mut b.as_mut().unwrap().3;
        scene.animate(t);
        painter.add(m3::render(&scene.meshes, cam, cx, cy, scale, light));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenes_build_and_float() {
        for name in ["Canopy", "Mistfall", "Archive", "Reading Room", "Folding@home", "Old Library", "Atlantis", "Slate"] {
            let mut s = build(name);
            assert!(!s.meshes.is_empty(), "{name} produces geometry");
            assert_eq!(s.meshes.len(), s.floats.len(), "floats parallel to meshes");
            // animating moves the floating objects but not the still ones
            let still_before = s.meshes[0].offset; // ground is first, STILL
            s.animate(3.0);
            assert_eq!(s.meshes[0].offset, still_before, "ground does not float");
            assert!(s.meshes.iter().any(|m| m.offset.y.abs() > 1e-4), "something floats after animate");
        }
    }

    #[test]
    fn float_still_is_zero() {
        assert_eq!(Float::STILL.offset(5.0), v(0.0, 0.0, 0.0));
    }

    #[test]
    fn region_plan_counts_and_is_stable() {
        let pr = EnvParams { nodes: 136, branches: 5, segments: 7, seed: 0x51A7_C0DE, ext_x: 68, ext_z: 22, bear: [0; 6], color_salience: 0.0 };
        let a = region_plan(&pr, 5, 24.0);
        let b = region_plan(&pr, 5, 24.0);
        assert_eq!(a.len(), 5, "one region per door");
        // same conversation → same palace (deterministic kinds + bearings)
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.kind, y.kind);
            assert!((x.angle - y.angle).abs() < 1e-6 && (x.dist - y.dist).abs() < 1e-6);
        }
        // wings sit outside the perimeter wall so they read as side-spaces
        assert!(a.iter().all(|r| r.dist > 24.0 + 2.5), "regions are beyond the perimeter");
    }

    #[test]
    fn region_kind_mix_is_varied() {
        // across a spread of conversations, every space type should appear at least once
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for seed in [0x51A7_C0DEu32, 0xDEAD_BEEF, 1234, 0xFACE, 777, 42, 0x1357] {
            let pr = EnvParams { nodes: 80, branches: 6, segments: 6, seed, ext_x: 40, ext_z: 24, bear: [0; 6], color_salience: 0.0 };
            for r in region_plan(&pr, 6, 24.0) { seen.insert(r.kind); }
        }
        for k in [RegionType::Rotunda, RegionType::Stairs, RegionType::Terrace, RegionType::Chamber, RegionType::SealedCave] {
            assert!(seen.contains(&k), "{k:?} never appears across the sample");
        }
    }

    #[test]
    fn branchy_atlantis_emits_wing_geometry_and_colliders() {
        // a branchy conversation builds doors→wings, each emitting column/statue/plinth COLLIDERS — the
        // unambiguous branch signal (total mesh count is confounded by the door-gapped bookshelf ring).
        set_env_params(EnvParams { nodes: 60, branches: 1, segments: 5, seed: 7, ext_x: 30, ext_z: 20, bear: [0; 6], color_salience: 0.0 });
        let linear = build("Atlantis");
        let linear_meshes = linear.meshes.len();
        let linear_cols = env_colliders().len();

        set_env_params(EnvParams { nodes: 60, branches: 6, segments: 5, seed: 7, ext_x: 30, ext_z: 20, bear: [0; 6], color_salience: 0.0 });
        let branchy = build("Atlantis");
        assert!(linear_meshes > 0 && branchy.meshes.len() > 0, "both palaces build geometry");
        assert!(env_colliders().len() > linear_cols, "more branches → more wings → more colliders the fly evades");
        set_env_params(EnvParams::default()); // restore
    }

    #[test]
    fn wall_ring_gapped_removes_doorways() {
        let full = m3::wall_ring_gapped(v(0.0, 0.0, 0.0), 20.0, 8.0, 110, c(0, 0, 0), &[]);
        let gapped = m3::wall_ring_gapped(v(0.0, 0.0, 0.0), 20.0, 8.0, 110, c(0, 0, 0), &[(0.0, 0.16), (1.5, 0.16)]);
        assert_eq!(full.verts.len(), gapped.verts.len(), "same vertex ring");
        assert!(gapped.tris.len() < full.tris.len(), "doorways remove wall quads");
    }
}
