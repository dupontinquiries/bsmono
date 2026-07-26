//! examples/menu_snapshot.rs — render the real `app::menu_view::menu_ui` to a PNG,
//! headlessly, in-sandbox (egui tessellation + a tiny CPU rasterizer + a
//! dependency-free PNG encoder). This is the verification loop for UI: no glfw,
//! no GPU, no window — just the same drawing code the renderer uses, turned into
//! an image we can actually look at.
//!
//! Run: cargo run --example menu_snapshot --features egui

use egui::epaint::Primitive;
use egui::{Color32, Context, ImageData, RawInput, Rect, TextureId};
use unendlich_engine::sim::menu_controller::MenuController;
use unendlich_engine::sim::title_flow::MenuId;

const W: usize = 720;
const H: usize = 520;
const PPP: f32 = 3.0; // supersample: lay out in points, rasterize at 3x for crisp text

fn main() {
    // Title screen.
    render(&MenuController::with_defaults(), "/mnt/user-data/outputs/menu_title.png");
    // New-game wizard screen.
    let mut m = MenuController::with_defaults();
    m.select(MenuId::NewGame);
    render(&m, "/mnt/user-data/outputs/menu_wizard.png");
}

fn render(menu: &MenuController, out: &str) {
    let ctx = Context::default();
    ctx.set_pixels_per_point(PPP);
    let mut input = RawInput::default();
    input.screen_rect = Some(Rect::from_min_size(
        egui::pos2(0.0, 0.0),
        egui::vec2(W as f32 / PPP, H as f32 / PPP),
    ));
    // run twice: the FIRST frame uploads the font atlas (it appears in that
    // frame's textures_delta); the second frame has stable shapes. We must read
    // the atlas from the first frame — the second's delta is empty.
    let first = ctx.run(input.clone(), |c| {
        let _ = unendlich_engine::app::menu_view::menu_ui(c, menu);
    });
    let full = ctx.run(input, |c| {
        let _ = unendlich_engine::app::menu_view::menu_ui(c, menu);
    });

    // Grab the font atlas from the texture deltas. In egui 0.34 the atlas is a
    // ColorImage; glyph coverage lives in the alpha channel (and the solid
    // WHITE_UV texel is opaque, so this path covers fills too).
    let mut atlas = (1usize, 1usize, vec![1.0f32]);
    for (id, delta) in first.textures_delta.set.iter().chain(full.textures_delta.set.iter()) {
        if *id == TextureId::Managed(0) {
            let ImageData::Color(img) = &delta.image;
            let cov: Vec<f32> = img.pixels.iter().map(|c| c.a() as f32 / 255.0).collect();
            atlas = (img.size[0], img.size[1], cov);
        }
    }
    let (aw, ah, ref apx) = atlas;

    // Canvas: a dark backdrop (stand-in for the frozen world behind the menu).
    let mut buf = vec![0u8; W * H * 4];
    for (i, px) in buf.chunks_mut(4).enumerate() {
        // subtle vertical gradient so it reads as a scene, not a flat fill
        let y = (i / W) as f32 / H as f32;
        px[0] = (20.0 + 14.0 * y) as u8;
        px[1] = (24.0 + 18.0 * y) as u8;
        px[2] = (34.0 + 26.0 * y) as u8;
        px[3] = 255;
    }

    let prims = ctx.tessellate(full.shapes, full.pixels_per_point);
    for prim in &prims {
        if let Primitive::Mesh(mesh) = &prim.primitive {
            let clip = prim.clip_rect;
            for tri in mesh.indices.chunks(3) {
                if tri.len() < 3 {
                    continue;
                }
                let a = &mesh.vertices[tri[0] as usize];
                let b = &mesh.vertices[tri[1] as usize];
                let c = &mesh.vertices[tri[2] as usize];
                raster_tri(&mut buf, a, b, c, clip, aw, ah, apx);
            }
        }
    }

    write_png(out, W, H, &buf).expect("write png");
    println!("wrote {out} ({W}x{H})");
}

#[inline]
fn edge(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (px - ax) * (by - ay) - (py - ay) * (bx - ax)
}

#[allow(clippy::too_many_arguments)]
fn raster_tri(
    buf: &mut [u8],
    a: &egui::epaint::Vertex,
    b: &egui::epaint::Vertex,
    c: &egui::epaint::Vertex,
    clip: Rect,
    aw: usize,
    ah: usize,
    apx: &[f32],
) {
    let (ax, ay) = (a.pos.x * PPP, a.pos.y * PPP);
    let (bx, by) = (b.pos.x * PPP, b.pos.y * PPP);
    let (cx, cy) = (c.pos.x * PPP, c.pos.y * PPP);
    let area = edge(ax, ay, bx, by, cx, cy);
    if area.abs() < 1e-6 {
        return;
    }
    let inv = 1.0 / area;

    let (clx0, cly0, clx1, cly1) =
        (clip.min.x * PPP, clip.min.y * PPP, clip.max.x * PPP, clip.max.y * PPP);
    let min_x = ax.min(bx).min(cx).max(clx0).max(0.0).floor() as i32;
    let max_x = ax.max(bx).max(cx).min(clx1).min(W as f32 - 1.0).ceil() as i32;
    let min_y = ay.min(by).min(cy).max(cly0).max(0.0).floor() as i32;
    let max_y = ay.max(by).max(cy).min(cly1).min(H as f32 - 1.0).ceil() as i32;

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            // barycentric (handle both windings via sign of area)
            let mut w0 = edge(bx, by, cx, cy, px, py) * inv;
            let mut w1 = edge(cx, cy, ax, ay, px, py) * inv;
            let mut w2 = edge(ax, ay, bx, by, px, py) * inv;
            if !(w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) && !(w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0) {
                continue;
            }
            // normalize for safety
            let s = w0 + w1 + w2;
            if s.abs() < 1e-9 {
                continue;
            }
            w0 /= s;
            w1 /= s;
            w2 /= s;

            // interpolate uv + color
            let u = w0 * a.uv.x + w1 * b.uv.x + w2 * c.uv.x;
            let v = w0 * a.uv.y + w1 * b.uv.y + w2 * c.uv.y;
            let cov = sample(apx, aw, ah, u, v).clamp(0.0, 1.0).powf(0.85);
            if cov <= 0.0 {
                continue;
            }
            let (cr, cg, cb, ca) = blend_color(a.color, b.color, c.color, w0, w1, w2);
            let alpha = (ca / 255.0) * cov;
            if alpha <= 0.003 {
                continue;
            }
            let idx = (y as usize * W + x as usize) * 4;
            for k in 0..3 {
                let src = [cr, cg, cb][k];
                let dst = buf[idx + k] as f32;
                buf[idx + k] = (src * alpha + dst * (1.0 - alpha)).clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn blend_color(
    a: Color32,
    b: Color32,
    c: Color32,
    w0: f32,
    w1: f32,
    w2: f32,
) -> (f32, f32, f32, f32) {
    (
        w0 * a.r() as f32 + w1 * b.r() as f32 + w2 * c.r() as f32,
        w0 * a.g() as f32 + w1 * b.g() as f32 + w2 * c.g() as f32,
        w0 * a.b() as f32 + w1 * b.b() as f32 + w2 * c.b() as f32,
        w0 * a.a() as f32 + w1 * b.a() as f32 + w2 * c.a() as f32,
    )
}

fn sample(apx: &[f32], aw: usize, ah: usize, u: f32, v: f32) -> f32 {
    // bilinear
    let fx = (u * aw as f32 - 0.5).clamp(0.0, aw as f32 - 1.0);
    let fy = (v * ah as f32 - 0.5).clamp(0.0, ah as f32 - 1.0);
    let x0 = fx.floor() as usize;
    let y0 = fy.floor() as usize;
    let x1 = (x0 + 1).min(aw - 1);
    let y1 = (y0 + 1).min(ah - 1);
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;
    let g = |x: usize, y: usize| *apx.get(y * aw + x).unwrap_or(&1.0);
    let top = g(x0, y0) * (1.0 - tx) + g(x1, y0) * tx;
    let bot = g(x0, y1) * (1.0 - tx) + g(x1, y1) * tx;
    top * (1.0 - ty) + bot * ty
}

// ── minimal, dependency-free PNG encoder (8-bit RGBA, uncompressed zlib) ──
fn write_png(path: &str, w: usize, h: usize, rgba: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);

    // IHDR
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit, RGBA, deflate, no filter, no interlace
    chunk(&mut out, b"IHDR", &ihdr);

    // raw filtered scanlines: each row prefixed with filter byte 0
    let mut raw = Vec::with_capacity(h * (1 + w * 4));
    for y in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }
    chunk(&mut out, b"IDAT", &zlib_store(&raw));
    chunk(&mut out, b"IEND", &[]);

    let mut f = std::fs::File::create(path)?;
    f.write_all(&out)
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = 0xffff_ffffu32;
    for b in kind.iter().chain(data.iter()) {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    out.extend_from_slice(&(crc ^ 0xffff_ffff).to_be_bytes());
}

/// zlib stream with stored (uncompressed) deflate blocks.
fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut z = vec![0x78, 0x01]; // zlib header (CM=8, no preset dict)
    let mut i = 0;
    while i < data.len() {
        let n = (data.len() - i).min(0xffff);
        let final_block = i + n >= data.len();
        z.push(if final_block { 1 } else { 0 }); // BFINAL, BTYPE=00 (stored)
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&data[i..i + n]);
        i += n;
    }
    // adler32 of the uncompressed data
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    z
}
