//! CPU rasterizer for egui's tessellated output.
//!
//! egui emits textured, vertex-coloured triangles (`ClippedPrimitive`).
//! The panel has no GPU we can use, so this draws them in software.
//!
//! ## Memory shape
//!
//! There is no full-size scratch frame and no "previous frame" copy. The
//! screen is cut into `TILE`×`TILE` tiles; each tile is rasterized into a
//! 16 KiB scratch buffer, encoded to the display format, and compared
//! against the SAME region of the display server's shared buffer. The
//! shared buffer IS the previous frame. Only rows that differ are written,
//! and the tile is marked dirty. That comparison is also the damage
//! tracking: egui does not report damage, so it is derived from pixels.
//!
//! Tiles are independent, so bands of tile rows run on separate threads,
//! each owning a disjoint slice of the output buffer. Memory cost of the
//! parallelism is threads × one tile.
//!
//! ## Blending
//!
//! egui colours and textures are premultiplied, gamma-encoded. Blending is
//! done in gamma space (`dst = src + dst·(1−αsrc)`), which is what egui's
//! own glow painter does by default, so output matches a desktop build.
//!
//! ## Coverage
//!
//! Pixel centres are sampled, with a consistent tie-break on shared edges
//! (see `Edge::includes_zero`). Without it, the diagonal shared by the two
//! triangles of every egui rectangle is drawn twice, and a translucent fill
//! shows a visible diagonal line.

use crate::format::{encode, is_chromatic, PixelFormat};
use crate::geom::IRect;
use egui::epaint::textures::TexturesDelta;
use egui::epaint::{ClippedPrimitive, ImageDelta, Mesh, Primitive, TextureId, WHITE_UV};
use std::collections::HashMap;

pub const TILE: usize = 64;

/// Textures uploaded by egui (font atlas + user images), premultiplied RGBA.
#[derive(Default)]
pub struct Textures {
    map: HashMap<TextureId, Tex>,
    /// Bumped by every upload or free: a tile signature (`TileCache`) hashes the triangles that cover a tile, not the
    /// pixels of the textures they sample, so any texture change invalidates every cached tile.
    generation: u64,
}

struct Tex {
    w: usize,
    h: usize,
    px: Vec<[u8; 4]>,
}

impl Textures {
    /// Apply the `set` half of a delta. Call before painting the frame.
    pub fn apply_set(&mut self, delta: &TexturesDelta) {
        if !delta.set.is_empty() { self.generation += 1; }
        for (id, d) in &delta.set {
            self.set(*id, d);
        }
    }
    /// Apply the `free` half. egui's contract is: free AFTER painting.
    pub fn apply_free(&mut self, delta: &TexturesDelta) {
        if !delta.free.is_empty() { self.generation += 1; }
        for id in &delta.free {
            self.map.remove(id);
        }
    }
    fn set(&mut self, id: TextureId, d: &ImageDelta) {
        let egui::ImageData::Color(img) = &d.image;
        let [w, h] = img.size;
        let src: Vec<[u8; 4]> = img.pixels.iter().map(|c| c.to_array()).collect();
        match d.pos {
            None => {
                self.map.insert(id, Tex { w, h, px: src });
            }
            Some([x0, y0]) => {
                let Some(t) = self.map.get_mut(&id) else { return };
                for y in 0..h {
                    if y0 + y >= t.h {
                        break;
                    }
                    for x in 0..w {
                        if x0 + x < t.w {
                            t.px[(y0 + y) * t.w + x0 + x] = src[y * w + x];
                        }
                    }
                }
            }
        }
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Where to draw: the display server's buffer.
pub struct Target<'a> {
    pub buf: &'a mut [u8],
    pub stride: usize,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
}

/// Which tiles changed, and whether their new content has colour.
#[derive(Debug, Clone)]
pub struct DirtyTiles {
    pub cols: usize,
    pub rows: usize,
    pub dirty: Vec<bool>,
    pub chroma: Vec<bool>,
    pub width: u32,
    pub height: u32,
}

impl DirtyTiles {
    pub fn count(&self) -> usize {
        self.dirty.iter().filter(|d| **d).count()
    }
    pub fn tile_rect(&self, c: usize, r: usize) -> IRect {
        let x0 = (c * TILE) as i32;
        let y0 = (r * TILE) as i32;
        IRect::new(x0, y0, (x0 + TILE as i32).min(self.width as i32), (y0 + TILE as i32).min(self.height as i32))
    }
}

/// What the last frame painted into each tile, as a hash of the ordered triangles that cover it.
///
/// Without it every frame clears, paints, encodes and compares EVERY tile of the screen (~40-60 ms on the Paper Pro
/// at 1620x2160) even when a pen adds one stroke: the cost is the screen, not the change. With it, a tile whose
/// triangles, clip, dithering and textures are the same as last frame is skipped outright: its pixels in the shared
/// buffer are already right, and it is not dirty. The output (buffer bytes and dirty tiles) is identical to `render`.
///
/// Invalidated whole, never partly, by: a different screen size, clear colour or pixel format, any texture upload
/// or free, and `force_all` (the buffer was replaced, so nothing in it can be trusted).
#[derive(Default)]
pub struct TileCache {
    sigs: Vec<u64>,
    key: Option<(usize, usize, [u8; 4], u64, usize)>,
    /// How many tiles the last render skipped as unchanged (for the trace, and to prove the cache is doing something).
    pub skipped: usize,
}

/// A mesh in pixel space, prepared once per frame.
struct Prim<'a> {
    clip: IRect,
    pos: Vec<[f32; 2]>,
    mesh: &'a Mesh,
    tex: Option<&'a Tex>,
}

/// Rasterize `prims` over a `clear` background into `target`, writing only
/// changed rows. `force_all` marks every tile dirty (first frame, or after
/// the server's buffer was repainted by someone else).
pub fn render(
    prims: &[ClippedPrimitive],
    pixels_per_point: f32,
    textures: &Textures,
    target: Target<'_>,
    clear: [u8; 4],
    threads: usize,
    force_all: bool,
    dither: Option<&[bool]>,
) -> DirtyTiles {
    render_impl(prims, pixels_per_point, textures, target, clear, threads, force_all, dither, None)
}

/// [`render`] that skips tiles whose content did not change since the last call with the same `cache`.
pub fn render_cached(
    prims: &[ClippedPrimitive],
    pixels_per_point: f32,
    textures: &Textures,
    target: Target<'_>,
    clear: [u8; 4],
    threads: usize,
    force_all: bool,
    dither: Option<&[bool]>,
    cache: &mut TileCache,
) -> DirtyTiles {
    render_impl(prims, pixels_per_point, textures, target, clear, threads, force_all, dither, Some(cache))
}

/// A signature of everything that decides a tile's pixels, apart from texture CONTENT (see `Textures::generation`).
fn tile_sig(bin: &[(u32, u32)], prepared: &[Prim], dith: bool) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write_u8(dith as u8);
    h.write_usize(bin.len());
    for &(pi, ti) in bin {
        let p = &prepared[pi as usize];
        h.write_u32(pi);
        let c = &p.clip;
        for v in [c.x0, c.y0, c.x1, c.y1] { h.write_i32(v); }
        match p.mesh.texture_id {
            TextureId::Managed(n) => { h.write_u8(0); h.write_u64(n); }
            TextureId::User(n) => { h.write_u8(1); h.write_u64(n); }
        }
        let tri = &p.mesh.indices[ti as usize * 3..ti as usize * 3 + 3];
        for &i in tri {
            let v = &p.mesh.vertices[i as usize];
            let pos = p.pos[i as usize];
            h.write_u32(pos[0].to_bits());
            h.write_u32(pos[1].to_bits());
            h.write_u32(v.uv.x.to_bits());
            h.write_u32(v.uv.y.to_bits());
            h.write(&v.color.to_array());
        }
    }
    h.finish() | 1 // 0 means "nothing cached"
}

#[allow(clippy::too_many_arguments)]
fn render_impl(
    prims: &[ClippedPrimitive],
    pixels_per_point: f32,
    textures: &Textures,
    target: Target<'_>,
    clear: [u8; 4],
    threads: usize,
    force_all: bool,
    dither: Option<&[bool]>,
    mut cache: Option<&mut TileCache>,
) -> DirtyTiles {
    let (w, h) = (target.width as usize, target.height as usize);
    let cols = w.div_ceil(TILE);
    let rows = h.div_ceil(TILE);
    let screen = IRect::from_size(target.width, target.height);

    // Prepare meshes in pixel space and bin triangles to tiles, in paint order.
    let mut prepared: Vec<Prim> = Vec::new();
    for cp in prims {
        let Primitive::Mesh(mesh) = &cp.primitive else { continue }; // PaintCallback: GPU-only, skipped
        let r = cp.clip_rect;
        let clip = IRect::new(
            (r.min.x * pixels_per_point).round() as i32,
            (r.min.y * pixels_per_point).round() as i32,
            (r.max.x * pixels_per_point).round() as i32,
            (r.max.y * pixels_per_point).round() as i32,
        )
        .intersect(&screen);
        if clip.is_empty() || mesh.indices.is_empty() {
            continue;
        }
        let pos = mesh.vertices.iter().map(|v| [v.pos.x * pixels_per_point, v.pos.y * pixels_per_point]).collect();
        prepared.push(Prim { clip, pos, mesh, tex: textures.map.get(&mesh.texture_id) });
    }
    let mut bins: Vec<Vec<(u32, u32)>> = vec![Vec::new(); cols * rows];
    for (pi, p) in prepared.iter().enumerate() {
        for (ti, tri) in p.mesh.indices.chunks_exact(3).enumerate() {
            let [a, b, c] = [p.pos[tri[0] as usize], p.pos[tri[1] as usize], p.pos[tri[2] as usize]];
            let bb = IRect::new(
                a[0].min(b[0]).min(c[0]).floor() as i32,
                a[1].min(b[1]).min(c[1]).floor() as i32,
                a[0].max(b[0]).max(c[0]).ceil() as i32,
                a[1].max(b[1]).max(c[1]).ceil() as i32,
            )
            .intersect(&p.clip);
            if bb.is_empty() {
                continue;
            }
            let (c0, c1) = (bb.x0 as usize / TILE, (bb.x1 as usize - 1) / TILE);
            let (r0, r1) = (bb.y0 as usize / TILE, (bb.y1 as usize - 1) / TILE);
            for r in r0..=r1.min(rows - 1) {
                for c in c0..=c1.min(cols - 1) {
                    bins[r * cols + c].push((pi as u32, ti as u32));
                }
            }
        }
    }

    // Split the output into bands of whole tile rows, one per worker.
    let threads = threads.clamp(1, rows.max(1));
    let rows_per_band = rows.div_ceil(threads);
    let band_bytes = rows_per_band * TILE * target.stride;
    let mut dirty = vec![false; cols * rows];
    let mut chroma = vec![false; cols * rows];
    let bpp = target.format.bytes_per_pixel();
    // The cache is only valid for the same screen, colour, format and textures; otherwise start it over.
    let mut sig_bands: Vec<Option<&mut [u64]>> = Vec::new();
    if let Some(c) = cache.as_deref_mut() {
        let key = (w, h, clear, textures.generation, bpp);
        if c.key != Some(key) || c.sigs.len() != cols * rows || force_all {
            c.sigs.clear();
            c.sigs.resize(cols * rows, 0);
            c.key = Some(key);
        }
        sig_bands = c.sigs.chunks_mut(rows_per_band * cols).map(Some).collect();
    }
    sig_bands.resize_with(threads.max(1).min(rows.max(1)).max(sig_bands.len()), || None);
    let skipped = std::sync::atomic::AtomicUsize::new(0);
    let skipped_ref = &skipped;
    let (stride, fmt) = (target.stride, target.format);

    std::thread::scope(|s| {
        let mut handles = Vec::new();
        for (band, (buf, (dchunk, cchunk))) in target
            .buf
            .chunks_mut(band_bytes)
            .zip(dirty.chunks_mut(rows_per_band * cols).zip(chroma.chunks_mut(rows_per_band * cols)))
            .enumerate()
        {
            let (bins, prepared) = (&bins, &prepared);
            let dither = dither;
            let mut sigs: Option<&mut [u64]> = sig_bands.get_mut(band).and_then(|o| o.take());
            handles.push(s.spawn(move || {
                let mut scratch = vec![[0u8; 4]; TILE * TILE];
                let mut rowbuf = vec![0u8; TILE * bpp];
                let r_base = band * rows_per_band;
                for lr in 0..(dchunk.len() / cols) {
                    let r = r_base + lr;
                    for c in 0..cols {
                        let tr = IRect::new(
                            (c * TILE) as i32,
                            (r * TILE) as i32,
                            ((c + 1) * TILE).min(w) as i32,
                            ((r + 1) * TILE).min(h) as i32,
                        );
                        let tw = tr.width() as usize;
                        let dith = dither.is_some_and(|m| m[r * cols + c]);
                        // unchanged since the last frame: its pixels in the shared buffer are already right
                        let sig = sigs.as_ref().map(|_| tile_sig(&bins[r * cols + c], prepared, dith));
                        if let (Some(s), Some(v)) = (sig, sigs.as_ref()) {
                            if !force_all && v[lr * cols + c] == s {
                                skipped_ref.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                dchunk[lr * cols + c] = false;
                                cchunk[lr * cols + c] = false;
                                continue;
                            }
                        }
                        for px in scratch.iter_mut() {
                            *px = clear;
                        }
                        for &(pi, ti) in &bins[r * cols + c] {
                            raster_triangle(&prepared[pi as usize], ti as usize, tr, &mut scratch);
                        }
                        // encode + diff against the live buffer
                        let mut changed = force_all;
                        let mut col = false;
                        // Motion tiles are dithered HERE, at encode, so an
                        // unchanged dithered image compares equal to what is
                        // already in the buffer (no phantom damage).
                        for y in 0..tr.height() as usize {
                            let src = &scratch[y * TILE..y * TILE + tw];
                            for (x, p) in src.iter().enumerate() {
                                if dith {
                                    let l = crate::format::luma(p[0], p[1], p[2]);
                                    let v = crate::motion::dither_bw(l, c * TILE + x, r * TILE + y);
                                    encode(fmt, [v, v, v, 255], &mut rowbuf[x * bpp..(x + 1) * bpp]);
                                } else {
                                    encode(fmt, *p, &mut rowbuf[x * bpp..(x + 1) * bpp]);
                                    col |= is_chromatic(p[0], p[1], p[2]);
                                }
                            }
                            let off = (lr * TILE + y) * stride + c * TILE * bpp;
                            let dst = &mut buf[off..off + tw * bpp];
                            if dst != &rowbuf[..tw * bpp] {
                                dst.copy_from_slice(&rowbuf[..tw * bpp]);
                                changed = true;
                            }
                        }
                        dchunk[lr * cols + c] = changed;
                        cchunk[lr * cols + c] = changed && col;
                        if let (Some(s), Some(v)) = (sig, sigs.as_mut()) { v[lr * cols + c] = s; }
                    }
                }
            }));
        }
        for h in handles {
            h.join().expect("raster worker panicked");
        }
    });

    if let Some(c) = cache.as_deref_mut() { c.skipped = skipped.load(std::sync::atomic::Ordering::Relaxed); }
    DirtyTiles { cols, rows, dirty, chroma, width: target.width, height: target.height }
}

/// Sub-pixel precision for edge evaluation: 1/256 px.
const SUB: i64 = 256;

/// One edge function in exact fixed-point arithmetic.
///
/// `E(a,b,p) = (b.x−a.x)(p.y−a.y) − (b.y−a.y)(p.x−a.x)` on integers is
/// exactly `−E(b,a,p)`, so two triangles sharing an edge never disagree
/// about a pixel on it. The first version used f32 and a "== 0.0"
/// tie-break; float evaluation is not antisymmetric, a pixel centre lying
/// on a rect's diagonal was rejected by BOTH triangles, and the background
/// showed through (a white speck on eink-demo's red swatch).
#[derive(Clone, Copy)]
struct Edge {
    /// ∂E/∂x per whole pixel (already × SUB); rows restart from `at`.
    step_x: i64,
    /// Coefficients before scaling, for the tie-break.
    a: i64,
    b: i64,
    ax: i64,
    ay: i64,
    dx: i64,
    dy: i64,
}

impl Edge {
    fn new(p: [i64; 2], q: [i64; 2]) -> Edge {
        let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
        Edge { step_x: -dy * SUB, a: -dy, b: dx, ax: p[0], ay: p[1], dx, dy }
    }
    /// Value at a fixed-point position.
    #[inline]
    fn at(&self, x: i64, y: i64) -> i64 {
        self.dx * (y - self.ay) - self.dy * (x - self.ax)
    }
    /// Tie-break for pixels exactly on the edge. The neighbouring triangle
    /// sees the same edge reversed (a, b negated, exactly), so exactly one
    /// of the two claims those pixels.
    fn includes_zero(&self) -> bool {
        self.a > 0 || (self.a == 0 && self.b > 0)
    }
}

#[inline]
fn fixed(v: [f32; 2]) -> [i64; 2] {
    [(v[0] * SUB as f32).round() as i64, (v[1] * SUB as f32).round() as i64]
}

#[inline]
fn mul8(a: u8, b: u8) -> u8 {
    ((a as u32 * b as u32 + 127) / 255) as u8
}

#[inline]
fn blend(dst: &mut [u8; 4], src: [u8; 4]) {
    let inv = 255 - src[3];
    for i in 0..4 {
        dst[i] = src[i].saturating_add(mul8(dst[i], inv));
    }
}

fn sample(t: &Tex, u: f32, v: f32) -> [u8; 4] {
    let x = u * t.w as f32 - 0.5;
    let y = v * t.h as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let cx = |x: f32| (x as i64).clamp(0, t.w as i64 - 1) as usize;
    let cy = |y: f32| (y as i64).clamp(0, t.h as i64 - 1) as usize;
    let (xa, xb, ya, yb) = (cx(x0), cx(x0 + 1.0), cy(y0), cy(y0 + 1.0));
    let (p00, p10, p01, p11) = (t.px[ya * t.w + xa], t.px[ya * t.w + xb], t.px[yb * t.w + xa], t.px[yb * t.w + xb]);
    let mut out = [0u8; 4];
    for i in 0..4 {
        let top = p00[i] as f32 + (p10[i] as f32 - p00[i] as f32) * fx;
        let bot = p01[i] as f32 + (p11[i] as f32 - p01[i] as f32) * fx;
        out[i] = (top + (bot - top) * fy + 0.5) as u8;
    }
    out
}

fn raster_triangle(p: &Prim, ti: usize, tile: IRect, scratch: &mut [[u8; 4]]) {
    let idx = &p.mesh.indices[ti * 3..ti * 3 + 3];
    let (mut i0, mut i1, i2) = (idx[0] as usize, idx[1] as usize, idx[2] as usize);
    let (f0, f1, f2) = (fixed(p.pos[i0]), fixed(p.pos[i1]), fixed(p.pos[i2]));
    let area = Edge::new(f0, f1).at(f2[0], f2[1]);
    if area == 0 {
        return;
    }
    let (q0, q1, q2) = if area < 0 {
        std::mem::swap(&mut i0, &mut i1);
        (f1, f0, f2)
    } else {
        (f0, f1, f2)
    };
    let area = area.abs();
    let (v0, v1, v2) = (&p.mesh.vertices[i0], &p.mesh.vertices[i1], &p.mesh.vertices[i2]);
    // w0 weights v0 (edge opposite it is q1→q2), etc.
    let e0 = Edge::new(q1, q2);
    let e1 = Edge::new(q2, q0);
    let e2 = Edge::new(q0, q1);
    let fl = |v: i64| v.div_euclid(SUB) as i32;
    let ce = |v: i64| (v + SUB - 1).div_euclid(SUB) as i32;
    let bb = IRect::new(
        fl(q0[0].min(q1[0]).min(q2[0])),
        fl(q0[1].min(q1[1]).min(q2[1])),
        ce(q0[0].max(q1[0]).max(q2[0])),
        ce(q0[1].max(q1[1]).max(q2[1])),
    )
    .intersect(&p.clip)
    .intersect(&tile);
    if bb.is_empty() {
        return;
    }
    let solid = v0.color == v1.color && v1.color == v2.color;
    let white = v0.uv == WHITE_UV && v1.uv == WHITE_UV && v2.uv == WHITE_UV;
    let tex = if white { None } else { p.tex };
    let (c0, c1, c2) = (v0.color.to_array(), v1.color.to_array(), v2.color.to_array());
    let (z0, z1, z2) = (e0.includes_zero(), e1.includes_zero(), e2.includes_zero());
    let inv_area = 1.0 / area as f32;
    let half = SUB / 2;

    for y in bb.y0..bb.y1 {
        let (px0, py) = (bb.x0 as i64 * SUB + half, y as i64 * SUB + half);
        let (mut w0, mut w1, mut w2) = (e0.at(px0, py), e1.at(px0, py), e2.at(px0, py));
        let row = ((y - tile.y0) as usize) * TILE;
        for x in bb.x0..bb.x1 {
            let inside = (w0 > 0 || (w0 == 0 && z0)) && (w1 > 0 || (w1 == 0 && z1)) && (w2 > 0 || (w2 == 0 && z2));
            if inside {
                let (l0, l1, l2) = (w0 as f32 * inv_area, w1 as f32 * inv_area, w2 as f32 * inv_area);
                let col = if solid {
                    c0
                } else {
                    let mut c = [0u8; 4];
                    for i in 0..4 {
                        c[i] = (c0[i] as f32 * l0 + c1[i] as f32 * l1 + c2[i] as f32 * l2 + 0.5).clamp(0.0, 255.0) as u8;
                    }
                    c
                };
                let src = match tex {
                    None => col,
                    Some(t) => {
                        let u = v0.uv.x * l0 + v1.uv.x * l1 + v2.uv.x * l2;
                        let v = v0.uv.y * l0 + v1.uv.y * l1 + v2.uv.y * l2;
                        let s = sample(t, u, v);
                        [mul8(col[0], s[0]), mul8(col[1], s[1]), mul8(col[2], s[2]), mul8(col[3], s[3])]
                    }
                };
                if src != [0, 0, 0, 0] {
                    blend(&mut scratch[row + (x - tile.x0) as usize], src);
                }
            }
            w0 += e0.step_x;
            w1 += e1.step_x;
            w2 += e2.step_x;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::{Color32, Vertex};
    use egui::{pos2, Rect};

    fn quad(x0: f32, y0: f32, x1: f32, y1: f32, color: Color32) -> ClippedPrimitive {
        let mut m = Mesh::default();
        for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
            m.vertices.push(Vertex { pos: pos2(x, y), uv: WHITE_UV, color });
        }
        m.indices = vec![0, 1, 2, 0, 2, 3];
        ClippedPrimitive { clip_rect: Rect::EVERYTHING, primitive: Primitive::Mesh(m) }
    }

    fn frame(w: u32, h: u32, fmt: PixelFormat) -> Vec<u8> {
        vec![0u8; w as usize * h as usize * fmt.bytes_per_pixel()]
    }

    fn px(buf: &[u8], w: u32, fmt: PixelFormat, x: u32, y: u32) -> [u8; 4] {
        let bpp = fmt.bytes_per_pixel();
        let o = (y * w) as usize * bpp + x as usize * bpp;
        crate::format::decode(fmt, &buf[o..o + bpp])
    }

    #[test]
    fn a_translucent_quad_has_no_seam_on_its_diagonal() {
        // 50% black over white. If the shared diagonal were blended twice,
        // pixels on it would come out darker than the rest.
        let (w, h, fmt) = (32, 32, PixelFormat::Rgb888);
        let mut buf = frame(w, h, fmt);
        let half = Color32::from_rgba_premultiplied(0, 0, 0, 128);
        render(&[quad(0.0, 0.0, 32.0, 32.0, half)], 1.0, &Textures::default(),
               Target { buf: &mut buf, stride: w as usize * 3, width: w, height: h, format: fmt },
               [255, 255, 255, 255], 1, false, None);
        let reference = px(&buf, w, fmt, 3, 20);
        for i in 0..32 {
            assert_eq!(px(&buf, w, fmt, i, i), reference, "diagonal pixel {i}");
        }
        assert!(reference[0] > 120 && reference[0] < 135, "{reference:?}");
    }

    /// Opaque quads must cover every pixel inside them. Found on the demo:
    /// a pixel centre exactly on a rect's diagonal was dropped by BOTH
    /// triangles (float edge values are not exact negatives), leaving a
    /// background-coloured hole. Sweeps many sizes and offsets at ppp 2.
    #[test]
    fn opaque_quads_have_no_holes_on_their_diagonal() {
        let (w, h, fmt) = (420u32, 420u32, PixelFormat::Rgb888);
        let mut holes = 0;
        let mut checked = 0;
        for i in 0..400 {
            let (x0, y0) = (3.0 + (i % 13) as f32 * 0.37, 3.0 + (i % 7) as f32 * 0.61);
            let (ww, hh) = (40.0 + (i * 7 % 150) as f32 * 0.4, 20.0 + (i * 11 % 170) as f32 * 0.3);
            let mut buf = frame(w, h, fmt);
            render(&[quad(x0, y0, x0 + ww, y0 + hh, Color32::RED)], 2.0, &Textures::default(),
                   Target { buf: &mut buf, stride: w as usize * 3, width: w, height: h, format: fmt }, [255; 4], 1, true, None);
            // every pixel whose centre is strictly inside the quad must be red
            for py in 0..h {
                for pxx in 0..w {
                    let (cx, cy) = (pxx as f32 + 0.5, py as f32 + 0.5);
                    if cx > x0 * 2.0 + 0.01 && cx < (x0 + ww) * 2.0 - 0.01 && cy > y0 * 2.0 + 0.01 && cy < (y0 + hh) * 2.0 - 0.01 {
                        checked += 1;
                        if px(&buf, w, fmt, pxx, py) != [255, 0, 0, 255] {
                            holes += 1;
                        }
                    }
                }
            }
        }
        assert!(checked > 1_000_000);
        assert_eq!(holes, 0, "{holes} uncovered pixels inside opaque quads");
    }

    /// The exact quad (egui's red swatch in eink-demo) whose diagonal passes
    /// through pixel centre (68.5, 738.5) at ppp 2.
    #[test]
    fn the_quad_found_on_the_demo_has_no_hole() {
        let (w, h, fmt) = (220u32, 800u32, PixelFormat::Rgb888);
        let mut buf = frame(w, h, fmt);
        render(&[quad(7.9999995, 358.4, 98.0, 395.6, Color32::RED)], 2.0, &Textures::default(),
               Target { buf: &mut buf, stride: w as usize * 3, width: w, height: h, format: fmt }, [255; 4], 1, true, None);
        assert_eq!(px(&buf, w, fmt, 68, 738), [255, 0, 0, 255]);
    }

    #[test]
    fn unchanged_frames_report_no_damage() {
        let (w, h, fmt) = (200, 150, PixelFormat::Gray8);
        let mut buf = frame(w, h, fmt);
        let prims = [quad(10.0, 10.0, 50.0, 40.0, Color32::BLACK)];
        let mk = |b: &mut Vec<u8>, force| render(&prims, 1.0, &Textures::default(),
            Target { buf: b, stride: w as usize, width: w, height: h, format: fmt }, [255; 4], 4, force, None);
        let first = mk(&mut buf, true);
        assert_eq!(first.count(), first.cols * first.rows);
        let second = mk(&mut buf, false);
        assert_eq!(second.count(), 0);
    }

    #[test]
    fn damage_is_confined_to_the_tiles_that_changed() {
        let (w, h, fmt) = (256, 256, PixelFormat::Rgb565);
        let mut buf = frame(w, h, fmt);
        let run = |b: &mut Vec<u8>, prims: &[ClippedPrimitive], force| render(prims, 1.0, &Textures::default(),
            Target { buf: b, stride: w as usize * 2, width: w, height: h, format: fmt }, [255; 4], 3, force, None);
        run(&mut buf, &[], true);
        // a small red square entirely inside tile (1,1)
        let d = run(&mut buf, &[quad(70.0, 70.0, 90.0, 90.0, Color32::RED)], false);
        assert_eq!(d.count(), 1);
        assert!(d.dirty[1 * d.cols + 1]);
        assert!(d.chroma[1 * d.cols + 1], "red is colour");
        assert_eq!(px(&buf, w, fmt, 80, 80), [255, 0, 0, 255]);
        assert_eq!(px(&buf, w, fmt, 60, 80), [255, 255, 255, 255]);
    }

    #[test]
    fn clip_rects_and_pixels_per_point_are_honoured() {
        let (w, h, fmt) = (64, 64, PixelFormat::Gray8);
        let mut buf = frame(w, h, fmt);
        let mut q = quad(0.0, 0.0, 32.0, 32.0, Color32::BLACK); // 64x64 px at ppp 2
        q.clip_rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(16.0, 32.0)); // left half only
        render(&[q], 2.0, &Textures::default(),
               Target { buf: &mut buf, stride: w as usize, width: w, height: h, format: fmt }, [255; 4], 2, true, None);
        assert_eq!(px(&buf, w, fmt, 10, 60)[0], 0);
        assert_eq!(px(&buf, w, fmt, 40, 10)[0], 255);
    }

    #[test]
    fn thread_count_does_not_change_the_image() {
        let (w, h, fmt) = (300, 700, PixelFormat::Rgb888);
        let prims: Vec<_> = (0..40)
            .map(|i| quad(i as f32 * 7.0, i as f32 * 17.0, i as f32 * 7.0 + 45.0, i as f32 * 17.0 + 33.0,
                          Color32::from_rgba_premultiplied((i * 6) as u8, 0, 100, 200)))
            .collect();
        let mut a = frame(w, h, fmt);
        let mut b = frame(w, h, fmt);
        render(&prims, 1.0, &Textures::default(), Target { buf: &mut a, stride: w as usize * 3, width: w, height: h, format: fmt }, [255; 4], 1, true, None);
        render(&prims, 1.0, &Textures::default(), Target { buf: &mut b, stride: w as usize * 3, width: w, height: h, format: fmt }, [255; 4], 7, true, None);
        assert!(a == b);
    }

    // ---- the tile cache must change the COST, never the picture ----

    fn both(frames: &[(Vec<ClippedPrimitive>, Option<Vec<bool>>, bool)], w: u32, h: u32, threads: usize, tex: &Textures) -> Vec<usize> {
        let fmt = PixelFormat::Rgb888;
        let (mut plain, mut cached) = (frame(w, h, fmt), frame(w, h, fmt));
        let mut cache = TileCache::default();
        let mut skipped = Vec::new();
        for (i, (prims, dither, force)) in frames.iter().enumerate() {
            let t1 = Target { buf: &mut plain, stride: w as usize * 3, width: w, height: h, format: fmt };
            let a = render(prims, 1.0, tex, t1, [255, 255, 255, 255], threads, *force || i == 0, dither.as_deref());
            let t2 = Target { buf: &mut cached, stride: w as usize * 3, width: w, height: h, format: fmt };
            let b = render_cached(prims, 1.0, tex, t2, [255, 255, 255, 255], threads, *force || i == 0, dither.as_deref(), &mut cache);
            assert!(plain == cached, "frame {i}: the cached picture differs from the plain one");
            assert_eq!(a.dirty, b.dirty, "frame {i}: dirty tiles differ");
            assert_eq!(a.chroma, b.chroma, "frame {i}: chroma flags differ");
            skipped.push(cache.skipped);
        }
        skipped
    }

    #[test]
    fn the_tile_cache_gives_the_same_picture_and_dirty_tiles_over_a_sequence_and_skips_what_did_not_change() {
        let (w, h) = (256, 192); // 4 x 3 tiles
        let red = Color32::from_rgb(220, 30, 30);
        let blue = Color32::from_rgb(30, 30, 220);
        let a = || quad(10.0, 10.0, 60.0, 40.0, red);
        let b = |dx: f32| quad(150.0 + dx, 100.0, 200.0 + dx, 150.0, blue);
        let mask = |v: bool| Some(vec![v; 12]);
        let frames = vec![
            (vec![a()], None, false),                        // 0: first frame, everything painted
            (vec![a(), b(0.0)], None, false),                // 1: a second shape appears
            (vec![a(), b(0.0)], None, false),                // 2: nothing changed
            (vec![a(), b(20.0)], None, false),               // 3: it moves
            (vec![b(20.0)], None, false),                    // 4: the first is removed
            (vec![b(20.0)], None, false),                    // 5: nothing changed
            (vec![b(20.0)], mask(true), false),              // 6: the same shapes, now dithered (a different picture)
            (vec![b(20.0)], mask(false), false),             // 7: dither off again
            (vec![b(20.0)], None, true),                     // 8: forced (the shared buffer was replaced): all repainted
            (vec![b(20.0)], None, false),                    // 9: nothing changed
        ];
        for threads in [1, 3] {
            let skipped = both(&frames, w, h, threads, &Textures::default());
            let total = 12;
            assert_eq!(skipped[0], 0, "threads {threads}: the first frame paints everything");
            assert!(skipped[2] == total, "threads {threads}: an unchanged frame skips every tile ({} of {total})", skipped[2]);
            assert!(skipped[3] > 0 && skipped[3] < total, "threads {threads}: a moved shape repaints only the tiles it touched ({} skipped)", skipped[3]);
            assert!(skipped[5] == total, "threads {threads}: unchanged again");
            assert!(skipped[6] < total, "threads {threads}: turning dither on is a different picture, not a skip ({})", skipped[6]);
            assert_eq!(skipped[8], 0, "threads {threads}: a forced frame skips nothing");
            assert!(skipped[9] == total, "threads {threads}: and the next unchanged frame skips everything again");
        }
    }

    #[test]
    fn a_texture_that_changes_under_the_same_triangles_is_never_shown_stale() {
        use egui::epaint::textures::TexturesDelta;
        use egui::epaint::{ColorImage, ImageDelta};
        use egui::TextureOptions;
        let (w, h) = (128, 128);
        let id = TextureId::Managed(7);
        let textured = || {
            let mut m = Mesh::default();
            m.texture_id = id;
            for (x, y) in [(8.0, 8.0), (100.0, 8.0), (100.0, 100.0), (8.0, 100.0)] {
                m.vertices.push(Vertex { pos: pos2(x, y), uv: pos2(0.5, 0.5), color: Color32::WHITE });
            }
            m.indices = vec![0, 1, 2, 0, 2, 3];
            ClippedPrimitive { clip_rect: Rect::EVERYTHING, primitive: Primitive::Mesh(m) }
        };
        let upload = |tex: &mut Textures, c: Color32| tex.apply_set(&TexturesDelta { set: vec![(id, ImageDelta::full(ColorImage::filled([4, 4], c), TextureOptions::LINEAR))], free: vec![] });
        let fmt = PixelFormat::Rgb888;
        let (mut plain, mut cached) = (frame(w, h, fmt), frame(w, h, fmt));
        let mut cache = TileCache::default();
        let mut tex = Textures::default();
        for (i, colour) in [Color32::RED, Color32::RED, Color32::GREEN, Color32::GREEN, Color32::BLUE].into_iter().enumerate() {
            if i == 0 || i == 2 || i == 4 { upload(&mut tex, colour); } // frames 1 and 3 keep the texture and the triangles
            let prims = vec![textured()];
            let t1 = Target { buf: &mut plain, stride: w as usize * 3, width: w, height: h, format: fmt };
            render(&prims, 1.0, &tex, t1, [255; 4], 2, i == 0, None);
            let t2 = Target { buf: &mut cached, stride: w as usize * 3, width: w, height: h, format: fmt };
            render_cached(&prims, 1.0, &tex, t2, [255; 4], 2, i == 0, None, &mut cache);
            assert!(plain == cached, "frame {i}: a stale texture was shown");
            if i == 1 || i == 3 { assert!(cache.skipped > 0, "frame {i}: with nothing changed the cache should skip tiles"); }
        }
        let p = px(&cached, w, fmt, 50, 50);
        assert!(p[2] > 200 && p[0] < 60, "the last texture (blue) is what is on screen: {p:?}");
    }

    #[test]
    fn a_change_of_screen_size_or_clear_colour_starts_the_cache_over() {
        let fmt = PixelFormat::Rgb888;
        let mut cache = TileCache::default();
        let prims = vec![quad(5.0, 5.0, 40.0, 40.0, Color32::from_rgb(10, 200, 10))];
        for (w, h, clear) in [(128u32, 128u32, [255u8; 4]), (128, 128, [255; 4]), (128, 128, [0, 0, 0, 255]), (192, 128, [0, 0, 0, 255])] {
            let (mut plain, mut cached) = (frame(w, h, fmt), frame(w, h, fmt));
            // a persistent buffer that the cache believes is already right would be wrong here: use fresh buffers and
            // force the first frame of each configuration, as the engine does after a buffer is replaced
            let t1 = Target { buf: &mut plain, stride: w as usize * 3, width: w, height: h, format: fmt };
            render(&prims, 1.0, &Textures::default(), t1, clear, 2, true, None);
            let t2 = Target { buf: &mut cached, stride: w as usize * 3, width: w, height: h, format: fmt };
            render_cached(&prims, 1.0, &Textures::default(), t2, clear, 2, true, None, &mut cache);
            assert!(plain == cached, "{w}x{h} clear {clear:?}");
        }
    }

    /// `cargo test --release --lib bench_the_tile_cache -- --ignored --nocapture`: the cost of a frame where one small
    /// shape changed, on a Paper-Pro-sized screen, plain against cached. Prints; asserts only that cached is not slower.
    #[test]
    #[ignore]
    fn bench_the_tile_cache() {
        let (w, h, fmt) = (1620u32, 2160u32, PixelFormat::Rgb565);
        let bpp = fmt.bytes_per_pixel();
        let scene = |dx: f32| {
            let mut v = vec![quad(0.0, 0.0, 1620.0, 2160.0, Color32::WHITE)];
            for i in 0..60 { v.push(quad(40.0 + (i % 6) as f32 * 250.0, 120.0 + (i / 6) as f32 * 190.0, 240.0 + (i % 6) as f32 * 250.0, 260.0 + (i / 6) as f32 * 190.0, Color32::from_gray(200 + (i % 5) as u8 * 10))); }
            v.push(quad(700.0 + dx, 1000.0, 740.0 + dx, 1040.0, Color32::BLACK)); // the "pen": a small mark that moves
            v
        };
        let (mut a, mut b) = (vec![0u8; w as usize * h as usize * bpp], vec![0u8; w as usize * h as usize * bpp]);
        let mut cache = TileCache::default();
        let tex = Textures::default();
        fn tgt<'a>(buf: &'a mut Vec<u8>, w: u32, h: u32, fmt: PixelFormat) -> Target<'a> { Target { buf, stride: w as usize * fmt.bytes_per_pixel(), width: w, height: h, format: fmt } }
        render(&scene(0.0), 1.0, &tex, tgt(&mut a, w, h, fmt), [255; 4], 4, true, None);
        render_cached(&scene(0.0), 1.0, &tex, tgt(&mut b, w, h, fmt), [255; 4], 4, true, None, &mut cache);
        let (mut plain_ms, mut cached_ms) = (0.0, 0.0);
        let n = 20;
        for i in 1..=n {
            let p = scene(i as f32 * 6.0);
            let t0 = std::time::Instant::now();
            render(&p, 1.0, &tex, tgt(&mut a, w, h, fmt), [255; 4], 4, false, None);
            plain_ms += t0.elapsed().as_secs_f64() * 1000.0;
            let t0 = std::time::Instant::now();
            render_cached(&p, 1.0, &tex, tgt(&mut b, w, h, fmt), [255; 4], 4, false, None, &mut cache);
            cached_ms += t0.elapsed().as_secs_f64() * 1000.0;
            assert!(a == b, "frame {i}");
        }
        let total = (w as usize).div_ceil(TILE) * (h as usize).div_ceil(TILE);
        println!("BENCH {w}x{h}, {n} frames with one small mark moving: plain {:.1} ms/frame, cached {:.1} ms/frame ({:.1}x), {} of {} tiles skipped", plain_ms / n as f64, cached_ms / n as f64, plain_ms / cached_ms.max(0.001), cache.skipped, total);
        assert!(cached_ms <= plain_ms * 1.1, "the cache made it slower");
    }
}
