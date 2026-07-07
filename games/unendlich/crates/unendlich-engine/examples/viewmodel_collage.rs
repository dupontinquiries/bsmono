//! examples/viewmodel_collage.rs — I1 "failures as pictures" proof.
//!
//! Headless, no bin: render the first-person held-item pose for each [`ItemForm`]
//! across a walk cycle (and a sway row) into one PNG collage, so the bob/sway and
//! per-form models are visible without the renderer. CPU perspective projection +
//! dependency-free PNG encoder.
//!
//! Run: cargo run --example viewmodel_collage

use unendlich_engine::sim::elemental::ItemForm;
use unendlich_engine::sim::viewmodel::{box_corners_in_view, item_model, viewmodel_pose, ViewmodelPose};

const CELL: usize = 200;
const PAD: usize = 10;

fn main() {
    let forms = [ItemForm::Staff, ItemForm::Axe, ItemForm::Blade, ItemForm::Tool];
    // columns: 4 walk phases (speed 1, no look) + 1 sway frame (look right, no walk)
    let phases = [0.0f32, 1.57, 3.14, 4.71];
    let cols = phases.len() + 1;
    let rows = forms.len();

    let w = cols * CELL + (cols + 1) * PAD;
    let h = rows * CELL + (rows + 1) * PAD;
    let mut buf = vec![0u8; w * h * 4];
    for px in buf.chunks_mut(4) {
        (px[0], px[1], px[2], px[3]) = (18, 20, 26, 255);
    }

    let mut drawn_any = vec![false; rows * cols];
    for (r, &form) in forms.iter().enumerate() {
        for c in 0..cols {
            let pose = if c < phases.len() {
                viewmodel_pose(form, phases[c], 1.0, [0.0, 0.0]) // walk bob
            } else {
                viewmodel_pose(form, 0.0, 0.0, [70.0, -20.0]) // sway from look
            };
            let ox = PAD + c * (CELL + PAD);
            let oy = PAD + r * (CELL + PAD);
            let painted = draw_panel(&mut buf, w, ox, oy, form, &pose);
            drawn_any[r * cols + c] = painted > 0;
        }
    }

    let out = "/mnt/user-data/outputs/viewmodel_collage.png";
    write_png(out, w, h, &buf).expect("write png");
    println!("wrote {out} ({w}x{h})");
    let panels = drawn_any.iter().filter(|b| **b).count();
    println!("panels with a visible item: {panels}/{}", rows * cols);
    assert!(panels == rows * cols, "every panel should show the held item");
    println!("OK — viewmodel renders across walk phases + sway for all forms.");
}

// CPU perspective projection of the item's boxes into a panel; painter's order
// (far first). Returns the number of pixels painted.
fn draw_panel(buf: &mut [u8], w: usize, ox: usize, oy: usize, form: ItemForm, pose: &ViewmodelPose) -> usize {
    let model = item_model(form);
    // collect (min_z, screen-bbox, color) per box
    let mut quads: Vec<(f32, usize, usize, usize, usize, [u8; 3])> = Vec::new();
    let f = CELL as f32 * 1.1; // focal length (px)
    let (cx, cy) = (ox as f32 + CELL as f32 * 0.5, oy as f32 + CELL as f32 * 0.5);
    for b in &model {
        let (corners, color) = box_corners_in_view(pose, b);
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let mut zsum = 0.0f32;
        for v in &corners {
            let depth = (-v[2]).max(0.05); // distance in front
            let sx = cx + v[0] * f / depth;
            let sy = cy - v[1] * f / depth;
            x0 = x0.min(sx);
            y0 = y0.min(sy);
            x1 = x1.max(sx);
            y1 = y1.max(sy);
            zsum += v[2];
        }
        quads.push((zsum / 8.0, clampx(x0, ox), clampy(y0, oy), clampx(x1, ox), clampy(y1, oy), color));
    }
    // far (more negative z) first
    quads.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut painted = 0usize;
    for (_, qx0, qy0, qx1, qy1, color) in quads {
        for y in qy0..=qy1 {
            for x in qx0..=qx1 {
                set_px(buf, w, x, y, color);
                painted += 1;
            }
        }
    }
    painted
}

fn clampx(v: f32, ox: usize) -> usize {
    (v.max(ox as f32).min((ox + CELL - 1) as f32)) as usize
}
fn clampy(v: f32, oy: usize) -> usize {
    (v.max(oy as f32).min((oy + CELL - 1) as f32)) as usize
}

fn set_px(buf: &mut [u8], w: usize, x: usize, y: usize, col: [u8; 3]) {
    let total = buf.len() / (w * 4);
    if x < w && y < total {
        let i = (y * w + x) * 4;
        buf[i] = col[0];
        buf[i + 1] = col[1];
        buf[i + 2] = col[2];
        buf[i + 3] = 255;
    }
}

// ── dependency-free PNG (uncompressed zlib STORE) — copied from creature_surface_collage ──
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
