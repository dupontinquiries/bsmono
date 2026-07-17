//! render.rs — slice 3 of the Rust duplication: the GRAPHICAL STYLE.
//!
//! The look is a marriage of the two houses in this monorepo:
//!   • clawbound's papercraft language (`games/clawbound/src/mesh.rs`):
//!     flat-shaded triangle soup with PER-FACE normals — the hard "origami
//!     facet" shading — and its exact face-shade grammar for boxes
//!     (top 1.0, sides 0.92/0.86/0.96/0.82, underside 0.7). The `Mesh`,
//!     `rgb`, `shade`, `mix` API here mirrors that file on purpose, so the
//!     two Rust games speak the same geometry dialect.
//!   • paperbound's page camera (rts.py): the iso view — world y
//!     foreshortened by ISO (0.62), elevation lifted off the page — and its
//!     paper palette: warm page ground, ruled grid, ink wells as dark
//!     ellipses, team-tinted folded actors with soft ground shadows.
//!
//! No GPU, no windowing, no external crates: a tiny software rasterizer
//! (painter's algorithm over world-depth, flat fill, one sun) draws into an
//! RGBA buffer, and a hand-rolled PNG writer (stored-deflate zlib, so only
//! std) gets frames to disk. Good for headless golden-image tests today and
//! an honest reference implementation for a wgpu backend tomorrow.

use crate::match_sim::{Job, World};

pub const ISO: f64 = 0.62; //   rts.py's page angle
pub const HG_LIFT: f64 = 26.0;

// ── colors (clawbound mesh.rs grammar) ──────────────────────────────────────

pub fn rgb(hex: u32) -> [f32; 3] {
    [
        ((hex >> 16) & 255) as f32 / 255.0,
        ((hex >> 8) & 255) as f32 / 255.0,
        (hex & 255) as f32 / 255.0,
    ]
}

pub fn shade(c: [f32; 3], f: f32) -> [f32; 3] {
    [c[0] * f, c[1] * f, c[2] * f]
}

pub fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] * (1.0 - t) + b[0] * t,
        a[1] * (1.0 - t) + b[1] * t,
        a[2] * (1.0 - t) + b[2] * t,
    ]
}

// ── mesh: flat-shaded facets in world space (x, elev, y-depth) ──────────────

#[derive(Clone, Copy)]
pub struct V3 {
    pub x: f64,
    pub e: f64, // elevation off the page
    pub y: f64, // depth along the page (drives iso + painter's sort)
}

pub fn v3(x: f64, e: f64, y: f64) -> V3 {
    V3 { x, e, y }
}

pub struct Tri {
    pub p: [V3; 3],
    pub col: [f32; 3],
}

#[derive(Default)]
pub struct Mesh {
    pub tris: Vec<Tri>,
}

impl Mesh {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tri(&mut self, a: V3, b: V3, c: V3, col: [f32; 3]) {
        self.tris.push(Tri { p: [a, b, c], col });
    }

    pub fn quad(&mut self, a: V3, b: V3, c: V3, d: V3, col: [f32; 3]) {
        self.tri(a, b, c, col);
        self.tri(a, c, d, col);
    }

    /// clawbound's `boxx`: an axis-aligned solid with its face-shade grammar.
    pub fn boxx(&mut self, cx: f64, ce: f64, cy: f64, hx: f64, he: f64, hy: f64, col: [f32; 3]) {
        let (x0, x1) = (cx - hx, cx + hx);
        let (e0, e1) = (ce - he, ce + he);
        let (y0, y1) = (cy - hy, cy + hy);
        // top / bottom
        self.quad(v3(x0, e1, y0), v3(x1, e1, y0), v3(x1, e1, y1), v3(x0, e1, y1), col);
        self.quad(v3(x0, e0, y1), v3(x1, e0, y1), v3(x1, e0, y0), v3(x0, e0, y0), shade(col, 0.7));
        // near / far (page-depth faces)
        self.quad(v3(x0, e0, y1), v3(x1, e0, y1), v3(x1, e1, y1), v3(x0, e1, y1), shade(col, 0.92));
        self.quad(v3(x1, e0, y0), v3(x0, e0, y0), v3(x0, e1, y0), v3(x1, e1, y0), shade(col, 0.86));
        // +x / -x flanks
        self.quad(v3(x1, e0, y1), v3(x1, e0, y0), v3(x1, e1, y0), v3(x1, e1, y1), shade(col, 0.96));
        self.quad(v3(x0, e0, y0), v3(x0, e0, y1), v3(x0, e1, y1), v3(x0, e1, y0), shade(col, 0.82));
    }

    /// clawbound's `cone` (roofs, spire caps) — faceted, not smooth.
    pub fn cone(&mut self, bx: f64, be: f64, by: f64, r: f64, h: f64, seg: usize, col: [f32; 3]) {
        let apex = v3(bx, be + h, by);
        for i in 0..seg {
            let a0 = i as f64 / seg as f64 * std::f64::consts::TAU;
            let a1 = (i + 1) as f64 / seg as f64 * std::f64::consts::TAU;
            let p0 = v3(bx + a0.cos() * r, be, by + a0.sin() * r);
            let p1 = v3(bx + a1.cos() * r, be, by + a1.sin() * r);
            // hand-faceted shading: faces toward the sun (up-left) run lighter
            let f = 0.82 + 0.18 * (0.5 + 0.5 * (a0 + 2.4).cos()) as f32;
            self.tri(p0, p1, apex, shade(col, f));
        }
    }

    /// A flat disc lying on the page (wells, shadows, pools).
    pub fn disc(&mut self, cx: f64, ce: f64, cy: f64, r: f64, seg: usize, col: [f32; 3]) {
        let c = v3(cx, ce, cy);
        for i in 0..seg {
            let a0 = i as f64 / seg as f64 * std::f64::consts::TAU;
            let a1 = (i + 1) as f64 / seg as f64 * std::f64::consts::TAU;
            self.tri(
                c,
                v3(cx + a0.cos() * r, ce, cy + a0.sin() * r),
                v3(cx + a1.cos() * r, ce, cy + a1.sin() * r),
                col,
            );
        }
    }
}

// ── the framebuffer + rasterizer ────────────────────────────────────────────

pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>, // RGBA
}

impl Frame {
    pub fn new(w: usize, h: usize) -> Self {
        Frame { w, h, px: vec![0; w * h * 4] }
    }

    pub fn clear(&mut self, c: [f32; 3]) {
        let (r, g, b) = (to8(c[0]), to8(c[1]), to8(c[2]));
        for p in self.px.chunks_exact_mut(4) {
            p[0] = r;
            p[1] = g;
            p[2] = b;
            p[3] = 255;
        }
    }

    fn put(&mut self, x: i64, y: i64, c: [f32; 3]) {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 4;
        self.px[i] = to8(c[0]);
        self.px[i + 1] = to8(c[1]);
        self.px[i + 2] = to8(c[2]);
        self.px[i + 3] = 255;
    }

    /// Flat-fill a screen-space triangle (integer scanline, no AA — the hard
    /// paper edge IS the style).
    pub fn fill_tri(&mut self, p: [(f64, f64); 3], c: [f32; 3]) {
        let minx = p.iter().map(|q| q.0).fold(f64::INFINITY, f64::min).floor().max(0.0) as i64;
        let maxx = p.iter().map(|q| q.0).fold(f64::NEG_INFINITY, f64::max).ceil().min(self.w as f64) as i64;
        let miny = p.iter().map(|q| q.1).fold(f64::INFINITY, f64::min).floor().max(0.0) as i64;
        let maxy = p.iter().map(|q| q.1).fold(f64::NEG_INFINITY, f64::max).ceil().min(self.h as f64) as i64;
        let edge = |a: (f64, f64), b: (f64, f64), x: f64, y: f64| {
            (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0)
        };
        let area = edge(p[0], p[1], p[2].0, p[2].1);
        if area.abs() < 1e-9 {
            return;
        }
        for y in miny..maxy {
            for x in minx..maxx {
                let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
                let w0 = edge(p[1], p[2], fx, fy) / area;
                let w1 = edge(p[2], p[0], fx, fy) / area;
                let w2 = edge(p[0], p[1], fx, fy) / area;
                if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0
                    || w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0
                {
                    self.put(x, y, c);
                }
            }
        }
    }
}

fn to8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// The paperbound page camera: rts.py's w2s with elevation lift.
pub struct Camera {
    pub cam_x: f64,
    pub cam_y: f64,
    pub zoom: f64,
}

impl Camera {
    pub fn project(&self, p: V3) -> (f64, f64) {
        (
            (p.x - self.cam_x) * self.zoom,
            (p.y - self.cam_y) * self.zoom * ISO - p.e * self.zoom,
        )
    }
}

/// Painter's algorithm: sort by world depth (max y of the tri, elevation as a
/// tiebreak so tops beat walls), then flat-fill.
pub fn draw_mesh(frame: &mut Frame, cam: &Camera, mesh: &Mesh) {
    let mut order: Vec<usize> = (0..mesh.tris.len()).collect();
    let key = |t: &Tri| {
        let y = t.p.iter().map(|q| q.y).fold(f64::NEG_INFINITY, f64::max);
        let e = t.p.iter().map(|q| q.e).fold(0.0f64, f64::max);
        y + e * 0.02
    };
    order.sort_by(|&a, &b| key(&mesh.tris[a]).partial_cmp(&key(&mesh.tris[b])).unwrap());
    for i in order {
        let t = &mesh.tris[i];
        let p = [cam.project(t.p[0]), cam.project(t.p[1]), cam.project(t.p[2])];
        frame.fill_tri(p, t.col);
    }
}

// ── the paperbound scene: a match state in the paper style ──────────────────

pub struct Plateau {
    pub x: f64,
    pub y: f64,
    pub r: f64,
}

pub fn lift_at(plateaus: &[Plateau], x: f64, y: f64) -> f64 {
    for p in plateaus {
        if (x - p.x).powi(2) + (y - p.y).powi(2) < p.r * p.r {
            return HG_LIFT;
        }
    }
    0.0
}

/// Build the whole scene mesh from a live match. Palette = the game's theme.
pub fn build_scene(world: &World, plateaus: &[Plateau], team_cols: [[f32; 3]; 2]) -> Mesh {
    let ground = rgb(0xC4B294);
    let mut m = Mesh::new();

    // plateaus: lifted faceted tops with a cliff wall (16 wall panels)
    for p in plateaus {
        let seg = 20;
        let top = shade(ground, 1.12);
        let cliff = shade(ground, 0.72);
        for i in 0..seg {
            let a0 = i as f64 / seg as f64 * std::f64::consts::TAU;
            let a1 = (i + 1) as f64 / seg as f64 * std::f64::consts::TAU;
            let (x0, y0) = (p.x + a0.cos() * p.r, p.y + a0.sin() * p.r);
            let (x1, y1) = (p.x + a1.cos() * p.r, p.y + a1.sin() * p.r);
            // cliff wall panel — lit toward the sun like clawbound flanks
            let f = 0.85 + 0.15 * (0.5 + 0.5 * (a0 + 2.4).cos()) as f32;
            m.quad(
                v3(x0, 0.0, y0),
                v3(x1, 0.0, y1),
                v3(x1, HG_LIFT, y1),
                v3(x0, HG_LIFT, y0),
                shade(cliff, f),
            );
            m.tri(v3(p.x, HG_LIFT, p.y), v3(x0, HG_LIFT, y0), v3(x1, HG_LIFT, y1), top);
        }
    }

    // ink wells: rim + pool discs sized by what's left
    for n in &world.nodes {
        let e = lift_at(plateaus, n.x, n.y) + 0.5;
        m.disc(n.x, e, n.y, 22.0, 14, rgb(0x24203A));
        let f = (n.amount / 2000.0).clamp(0.15, 1.0);
        m.disc(n.x, e + 0.4, n.y, 22.0 * f, 12, rgb(0x4E4482));
        m.disc(n.x, e + 0.8, n.y, 22.0 * f * 0.6, 10, rgb(0x786CBE));
    }

    // buildings: folded tent solids — a boxx body with a cone roof, tinted to
    // the team, sized by spec radius; construction shows as a lower body
    for b in &world.buildings {
        if !b.alive() {
            continue;
        }
        let tc = team_cols[b.team];
        let body = mix(rgb(0xD8CCAE), tc, 0.55);
        let e = lift_at(plateaus, b.x, b.y);
        let h = if b.complete { b.r * 0.9 } else { b.r * 0.35 };
        let shadow = shade(ground, 0.8);
        m.disc(b.x, e + 0.2, b.y + b.r * 0.25, b.r * 1.05, 12, shadow);
        m.boxx(b.x, e + h / 2.0, b.y, b.r * 0.7, h / 2.0, b.r * 0.6, body);
        if b.complete {
            m.cone(b.x, e + h, b.y, b.r * 0.62, b.r * 0.55, 8, shade(body, 1.08));
        }
    }

    // units: little folded diamonds (two facing facets + a shadow), workers
    // rounder, fighters sharper — the origami silhouette in three triangles
    for u in &world.units {
        if !u.alive() {
            continue;
        }
        let tc = team_cols[u.team];
        let e = lift_at(plateaus, u.x, u.y);
        let s = if u.worker { 9.0 } else { 12.0 };
        m.disc(u.x, e + 0.15, u.y + s * 0.35, s * 0.9, 8, shade(ground, 0.78));
        let tip = v3(u.x, e + s * 1.7, u.y);
        let l = v3(u.x - s, e, u.y + s * 0.4);
        let r = v3(u.x + s, e, u.y + s * 0.4);
        let back = v3(u.x, e, u.y - s * 0.7);
        // two lit facets + a darker rear fold — the hard papercraft crease
        m.tri(l, r, tip, mix(rgb(0xEDE4CC), tc, 0.6));
        m.tri(r, back, tip, shade(mix(rgb(0xEDE4CC), tc, 0.6), 0.84));
        m.tri(back, l, tip, shade(mix(rgb(0xEDE4CC), tc, 0.6), 0.72));
        // a fighting unit flashes its blade facet brighter
        if matches!(u.job, Job::Fight { .. }) {
            m.tri(
                v3(u.x, e + s * 1.7, u.y),
                v3(u.x + s * 0.5, e + s * 0.9, u.y),
                v3(u.x, e + s * 0.8, u.y + s * 0.3),
                rgb(0xF2E0A0),
            );
        }
    }
    m
}

/// Render a full frame: page ground with ruled grid, then the scene mesh.
pub fn render(world: &World, plateaus: &[Plateau], cam: &Camera, w: usize, h: usize) -> Frame {
    let mut f = Frame::new(w, h);
    let ground = rgb(0xC4B294);
    f.clear(ground);
    // ruled paper grid (world-anchored so it pans correctly)
    let grid = shade(ground, 0.94);
    let step = 160.0;
    let mut gx = (cam.cam_x / step).floor() * step;
    while (gx - cam.cam_x) * cam.zoom < w as f64 {
        let sx = ((gx - cam.cam_x) * cam.zoom) as i64;
        for y in 0..h as i64 {
            f.put(sx, y, grid);
        }
        gx += step;
    }
    let mut gy = (cam.cam_y / step).floor() * step;
    while (gy - cam.cam_y) * cam.zoom * ISO < h as f64 {
        let sy = ((gy - cam.cam_y) * cam.zoom * ISO) as i64;
        for x in 0..w as i64 {
            f.put(x, sy, grid);
        }
        gy += step;
    }
    // one warm margin rule, notebook style
    let rule = mix(ground, rgb(0xC89678), 0.5);
    for y in 0..h as i64 {
        f.put((0.08 * w as f64) as i64, y, rule);
    }
    let mesh = build_scene(world, plateaus, [rgb(0xD8B45A), rgb(0x8C7CC8)]);
    draw_mesh(&mut f, cam, &mesh);
    f
}

// ── PNG writer: std-only (zlib stored blocks + adler32 + crc32) ─────────────

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for i in 0..256u32 {
        let mut c = i;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        table[i as usize] = c;
    }
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_in = kind.to_vec();
    crc_in.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_in).to_be_bytes());
}

/// Encode the frame as a valid PNG using zlib "stored" (uncompressed) blocks —
/// zero dependencies, byte-deterministic, fine for goldens and previews.
pub fn png_bytes(frame: &Frame) -> Vec<u8> {
    // raw scanlines with filter byte 0
    let mut raw = Vec::with_capacity(frame.h * (1 + frame.w * 4));
    for y in 0..frame.h {
        raw.push(0);
        let row = &frame.px[y * frame.w * 4..(y + 1) * frame.w * 4];
        raw.extend_from_slice(row);
    }
    // zlib: header + stored deflate blocks (max 65535 each) + adler
    let mut z = vec![0x78, 0x01];
    let mut off = 0;
    while off < raw.len() {
        let n = (raw.len() - off).min(65535);
        let last = off + n == raw.len();
        z.push(if last { 1 } else { 0 });
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&raw[off..off + n]);
        off += n;
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(frame.w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(frame.h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

pub fn save_png(frame: &Frame, path: &std::path::Path) -> std::io::Result<()> {
    std::fs::write(path, png_bytes(frame))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::load_factbook;
    use crate::match_sim::{Node, World};
    use std::path::Path;

    #[test]
    fn renders_a_live_match_frame_to_png() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let book = load_factbook(&dir).unwrap();
        let mut w = World::new(&book["rootbound"], &book["swarm"]);
        w.add_building(0, "scriptorium", 320.0, 620.0, true);
        w.add_building(0, "spire", 470.0, 700.0, true);
        w.add_building(1, "scriptorium", 1180.0, 380.0, true);
        w.nodes.push(Node { x: 520.0, y: 560.0, amount: 1800.0 });
        w.nodes.push(Node { x: 980.0, y: 430.0, amount: 900.0 });
        for i in 0..3 {
            w.add_unit(0, "scribe", 350.0 + i as f64 * 26.0, 640.0);
        }
        for i in 0..4 {
            w.add_unit(0, "soldier", 620.0 + i as f64 * 30.0, 560.0);
            w.add_unit(1, "soldier", 860.0 + i as f64 * 30.0, 470.0);
        }
        let plateaus = [Plateau { x: 900.0, y: 430.0, r: 150.0 }];
        for _ in 0..(8 * 30) {
            w.tick(1.0 / 30.0);
        }
        let cam = Camera { cam_x: 180.0, cam_y: 60.0, zoom: 0.95 };
        let frame = render(&w, &plateaus, &cam, 1280, 800);
        // content check: the frame isn't flat paper — count distinct colours
        let mut set = std::collections::HashSet::new();
        for p in frame.px.chunks_exact(4).step_by(97) {
            set.insert((p[0], p[1], p[2]));
        }
        // flat-shaded paper is INTENTIONALLY low-count; ~30 facet shades is
        // the style working, not failing — so assert a floor plus targeted
        // content: a rootbound-tinted pixel must exist where the hall projects
        assert!(set.len() > 22, "frame too flat: {} colours", set.len());
        let (hx, hy) = cam.project(v3(320.0, 20.0, 620.0));
        let idx = ((hy as usize).min(799) * 1280 + (hx as usize).min(1279)) * 4;
        let (r, g, b2) = (frame.px[idx], frame.px[idx + 1], frame.px[idx + 2]);
        assert!(g as i32 >= r as i32 - 30 && b2 < r, 
                "expected a warm team-tinted hall pixel, got {r},{g},{b2}");
        // the PNG is structurally valid and deterministic
        let png = png_bytes(&frame);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(png, png_bytes(&frame), "encoder must be deterministic");
        let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/rust_paper_frame.png");
        save_png(&frame, &out).unwrap();
        assert!(out.metadata().unwrap().len() > 10_000);
    }

    #[test]
    fn clawbound_face_grammar_is_preserved() {
        // the boxx face factors ARE clawbound's mesh.rs numbers
        let mut m = Mesh::new();
        m.boxx(0.0, 5.0, 0.0, 4.0, 5.0, 4.0, [1.0, 1.0, 1.0]);
        assert_eq!(m.tris.len(), 12);
        let mut factors: Vec<i32> = m
            .tris
            .chunks(2)
            .map(|q| (q[0].col[0] * 100.0).round() as i32)
            .collect();
        factors.sort();
        assert_eq!(factors, vec![70, 82, 86, 92, 96, 100]);
    }

    #[test]
    fn iso_camera_matches_the_python_page_angle() {
        let cam = Camera { cam_x: 0.0, cam_y: 0.0, zoom: 1.0 };
        let (sx, sy) = cam.project(v3(100.0, 0.0, 100.0));
        assert!((sx - 100.0).abs() < 1e-9);
        assert!((sy - 62.0).abs() < 1e-9, "world y foreshortens by ISO 0.62");
        // elevation lifts straight up the screen, unforeshortened
        let (_, sy2) = cam.project(v3(100.0, HG_LIFT, 100.0));
        assert!((sy - sy2 - HG_LIFT).abs() < 1e-9);
    }
}
