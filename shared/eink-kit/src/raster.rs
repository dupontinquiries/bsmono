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
}

struct Tex {
    w: usize,
    h: usize,
    px: Vec<[u8; 4]>,
}

impl Textures {
    /// Apply the `set` half of a delta. Call before painting the frame.
    pub fn apply_set(&mut self, delta: &TexturesDelta) {
        for (id, d) in &delta.set {
            self.set(*id, d);
        }
    }
    /// Apply the `free` half. egui's contract is: free AFTER painting.
    pub fn apply_free(&mut self, delta: &TexturesDelta) {
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
                        let dith = dither.is_some_and(|m| m[r * cols + c]);
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
                    }
                }
            }));
        }
        for h in handles {
            h.join().expect("raster worker panicked");
        }
    });

    DirtyTiles { cols, rows, dirty, chroma, width: target.width, height: target.height }
}

/// One edge function `w = a·x + b·y + c`, positive inside.
#[derive(Clone, Copy)]
struct Edge {
    a: f32,
    b: f32,
    c: f32,
}

impl Edge {
    fn new(p: [f32; 2], q: [f32; 2]) -> Edge {
        let a = -(q[1] - p[1]);
        let b = q[0] - p[0];
        Edge { a, b, c: -(a * p[0] + b * p[1]) }
    }
    #[inline]
    fn at(&self, x: f32, y: f32) -> f32 {
        self.a * x + self.b * y + self.c
    }
    /// Tie-break for pixels exactly on the edge. The neighbouring triangle
    /// sees the same edge reversed (a, b negated), so exactly one of the two
    /// claims those pixels.
    fn includes_zero(&self) -> bool {
        self.a > 0.0 || (self.a == 0.0 && self.b > 0.0)
    }
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
    let area = Edge::new(p.pos[i0], p.pos[i1]).at(p.pos[i2][0], p.pos[i2][1]);
    if area.abs() < 1e-6 {
        return;
    }
    if area < 0.0 {
        std::mem::swap(&mut i0, &mut i1);
    }
    let area = area.abs();
    let (v0, v1, v2) = (&p.mesh.vertices[i0], &p.mesh.vertices[i1], &p.mesh.vertices[i2]);
    let (q0, q1, q2) = (p.pos[i0], p.pos[i1], p.pos[i2]);
    // w0 weights v0 (edge opposite it is q1→q2), etc.
    let e0 = Edge::new(q1, q2);
    let e1 = Edge::new(q2, q0);
    let e2 = Edge::new(q0, q1);
    let bb = IRect::new(
        q0[0].min(q1[0]).min(q2[0]).floor() as i32,
        q0[1].min(q1[1]).min(q2[1]).floor() as i32,
        q0[0].max(q1[0]).max(q2[0]).ceil() as i32,
        q0[1].max(q1[1]).max(q2[1]).ceil() as i32,
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
    let inv_area = 1.0 / area;

    for y in bb.y0..bb.y1 {
        let py = y as f32 + 0.5;
        let px0 = bb.x0 as f32 + 0.5;
        let (mut w0, mut w1, mut w2) = (e0.at(px0, py), e1.at(px0, py), e2.at(px0, py));
        let row = ((y - tile.y0) as usize) * TILE;
        for x in bb.x0..bb.x1 {
            let inside = (w0 > 0.0 || (w0 == 0.0 && z0))
                && (w1 > 0.0 || (w1 == 0.0 && z1))
                && (w2 > 0.0 || (w2 == 0.0 && z2));
            if inside {
                let (l0, l1, l2) = (w0 * inv_area, w1 * inv_area, w2 * inv_area);
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
                if src[3] != 0 || src[0] != 0 || src[1] != 0 || src[2] != 0 {
                    blend(&mut scratch[row + (x - tile.x0) as usize], src);
                }
            }
            w0 += e0.a;
            w1 += e1.a;
            w2 += e2.a;
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
}
