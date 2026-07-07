//! examples/flythrough_snapshot.rs — render the title menu fly-in across time into
//! a collage, headless, so we can see the pieces assemble (and print the cinematic
//! camera pose at each step — the renderer flies the real camera to those).
//! Run: cargo run --example flythrough_snapshot --features egui

use egui::epaint::Primitive;
use egui::{Color32, Context, ImageData, RawInput, Rect, TextureId};
use unendlich_engine::app::menu_view::title_flythrough_ui;
use unendlich_engine::sim::title_cinematic::CinematicPath;

const CELL_W: usize = 300;
const CELL_H: usize = 230;
const COLS: usize = 3;
const ROWS: usize = 2;
const PAD: usize = 8;
const PPP: f32 = 2.0;
const FLY: f32 = 0.6;

fn main() {
    let times = [0.0f32, 0.5, 1.0, 1.6, 2.4, 3.2];
    let path = CinematicPath::intro();
    let w = COLS * CELL_W + (COLS + 1) * PAD;
    let h = ROWS * CELL_H + (ROWS + 1) * PAD;
    let mut buf = vec![0u8; w * h * 4];
    for (i, px) in buf.chunks_mut(4).enumerate() {
        let y = (i / w) as f32 / h as f32;
        px[0] = (18.0 + 12.0 * y) as u8;
        px[1] = (22.0 + 16.0 * y) as u8;
        px[2] = (32.0 + 24.0 * y) as u8;
        px[3] = 255;
    }

    for (i, &t) in times.iter().enumerate() {
        let pose = path.sample(t);
        println!(
            "t={t:.1}s  camera eye=[{:.1},{:.1},{:.1}] target=[{:.1},{:.1},{:.1}] fov={:.0}",
            pose.eye[0], pose.eye[1], pose.eye[2], pose.target[0], pose.target[1], pose.target[2], pose.fov_deg
        );
        let ox = PAD + (i % COLS) * (CELL_W + PAD);
        let oy = PAD + (i / COLS) * (CELL_H + PAD);
        render_cell(&mut buf, w, ox, oy, t);
    }
    write_png("/mnt/user-data/outputs/menu_flythrough.png", w, h, &buf).expect("png");
    println!("wrote menu_flythrough.png ({w}x{h}) times={times:?}");
}

fn render_cell(buf: &mut [u8], stride_w: usize, ox: usize, oy: usize, elapsed: f32) {
    let ctx = Context::default();
    ctx.set_pixels_per_point(PPP);
    let lw = CELL_W as f32 / PPP;
    let lh = CELL_H as f32 / PPP;
    let mut input = RawInput::default();
    input.screen_rect = Some(Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(lw, lh)));
    let _ = ctx.run(input.clone(), |c| {
        let _ = title_flythrough_ui(c, elapsed, FLY);
    });
    let full = ctx.run(input, |c| {
        let _ = title_flythrough_ui(c, elapsed, FLY);
    });

    let mut atlas = (1usize, 1usize, vec![1.0f32]);
    for (id, delta) in &full.textures_delta.set {
        if *id == TextureId::Managed(0) {
            let ImageData::Color(img) = &delta.image;
            atlas = (
                img.size[0],
                img.size[1],
                img.pixels.iter().map(|c| c.a() as f32 / 255.0).collect(),
            );
        }
    }
    let (aw, ah, ref apx) = atlas;
    // faint cell panel
    fill_panel(buf, stride_w, ox, oy);
    let prims = ctx.tessellate(full.shapes, full.pixels_per_point);
    for prim in &prims {
        if let Primitive::Mesh(mesh) = &prim.primitive {
            for tri in mesh.indices.chunks(3) {
                if tri.len() < 3 {
                    continue;
                }
                raster_tri(
                    buf,
                    stride_w,
                    ox,
                    oy,
                    &mesh.vertices[tri[0] as usize],
                    &mesh.vertices[tri[1] as usize],
                    &mesh.vertices[tri[2] as usize],
                    aw,
                    ah,
                    apx,
                );
            }
        }
    }
}

fn fill_panel(buf: &mut [u8], stride_w: usize, ox: usize, oy: usize) {
    let total_h = buf.len() / (stride_w * 4);
    for y in oy..(oy + CELL_H).min(total_h) {
        for x in ox..(ox + CELL_W).min(stride_w) {
            let i = (y * stride_w + x) * 4;
            buf[i] = 26;
            buf[i + 1] = 30;
            buf[i + 2] = 40;
            buf[i + 3] = 255;
        }
    }
}

#[inline]
fn edge(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (px - ax) * (by - ay) - (py - ay) * (bx - ax)
}

#[allow(clippy::too_many_arguments)]
fn raster_tri(
    buf: &mut [u8],
    stride_w: usize,
    ox: usize,
    oy: usize,
    a: &egui::epaint::Vertex,
    b: &egui::epaint::Vertex,
    c: &egui::epaint::Vertex,
    aw: usize,
    ah: usize,
    apx: &[f32],
) {
    let total_h = buf.len() / (stride_w * 4);
    let (ax, ay) = (a.pos.x * PPP + ox as f32, a.pos.y * PPP + oy as f32);
    let (bx, by) = (b.pos.x * PPP + ox as f32, b.pos.y * PPP + oy as f32);
    let (cx, cy) = (c.pos.x * PPP + ox as f32, c.pos.y * PPP + oy as f32);
    let area = edge(ax, ay, bx, by, cx, cy);
    if area.abs() < 1e-6 {
        return;
    }
    let inv = 1.0 / area;
    let min_x = ax.min(bx).min(cx).max(ox as f32).floor() as i32;
    let max_x = ax.max(bx).max(cx).min((ox + CELL_W) as f32).min(stride_w as f32 - 1.0).ceil() as i32;
    let min_y = ay.min(by).min(cy).max(oy as f32).floor() as i32;
    let max_y = ay.max(by).max(cy).min((oy + CELL_H) as f32).min(total_h as f32 - 1.0).ceil() as i32;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let mut w0 = edge(bx, by, cx, cy, px, py) * inv;
            let mut w1 = edge(cx, cy, ax, ay, px, py) * inv;
            let mut w2 = edge(ax, ay, bx, by, px, py) * inv;
            if !(w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) && !(w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0) {
                continue;
            }
            let s = w0 + w1 + w2;
            if s.abs() < 1e-9 {
                continue;
            }
            w0 /= s;
            w1 /= s;
            w2 /= s;
            let u = w0 * a.uv.x + w1 * b.uv.x + w2 * c.uv.x;
            let v = w0 * a.uv.y + w1 * b.uv.y + w2 * c.uv.y;
            let cov = sample(apx, aw, ah, u, v).clamp(0.0, 1.0).powf(0.85);
            if cov <= 0.0 {
                continue;
            }
            let (cr, cg, cb, ca) = mix(a.color, b.color, c.color, w0, w1, w2);
            let alpha = (ca / 255.0) * cov;
            if alpha <= 0.003 {
                continue;
            }
            let idx = (y as usize * stride_w + x as usize) * 4;
            for k in 0..3 {
                let src = [cr, cg, cb][k];
                let dst = buf[idx + k] as f32;
                buf[idx + k] = (src * alpha + dst * (1.0 - alpha)).clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn mix(a: Color32, b: Color32, c: Color32, w0: f32, w1: f32, w2: f32) -> (f32, f32, f32, f32) {
    (
        w0 * a.r() as f32 + w1 * b.r() as f32 + w2 * c.r() as f32,
        w0 * a.g() as f32 + w1 * b.g() as f32 + w2 * c.g() as f32,
        w0 * a.b() as f32 + w1 * b.b() as f32 + w2 * c.b() as f32,
        w0 * a.a() as f32 + w1 * b.a() as f32 + w2 * c.a() as f32,
    )
}

fn sample(apx: &[f32], aw: usize, ah: usize, u: f32, v: f32) -> f32 {
    let fx = (u * aw as f32 - 0.5).clamp(0.0, aw as f32 - 1.0);
    let fy = (v * ah as f32 - 0.5).clamp(0.0, ah as f32 - 1.0);
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(aw - 1), (y0 + 1).min(ah - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let g = |x: usize, y: usize| *apx.get(y * aw + x).unwrap_or(&1.0);
    let top = g(x0, y0) * (1.0 - tx) + g(x1, y0) * tx;
    let bot = g(x0, y1) * (1.0 - tx) + g(x1, y1) * tx;
    top * (1.0 - ty) + bot * ty
}

// dependency-free PNG (stored zlib)
fn write_png(path: &str, w: usize, h: usize, rgba: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let mut raw = Vec::with_capacity(h * (1 + w * 4));
    for y in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }
    chunk(&mut out, b"IDAT", &zlib_store(&raw));
    chunk(&mut out, b"IEND", &[]);
    std::fs::File::create(path)?.write_all(&out)
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
fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut z = vec![0x78, 0x01];
    let mut i = 0;
    while i < data.len() {
        let n = (data.len() - i).min(0xffff);
        let fin = i + n >= data.len();
        z.push(if fin { 1 } else { 0 });
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&data[i..i + n]);
        i += n;
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    z
}
