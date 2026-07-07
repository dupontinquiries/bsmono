//! examples/creature_surface_collage.rs — the C3 "failures as pictures" proof.
//!
//! Headless, no bin: synthesize a reference image with bright (bioluminescent)
//! spots, run `surface_from_image`, build a forge creature and place glow from the
//! extracted traits, then render a side-on, joint-colored projection next to the
//! source so you can SEE the glow land on the creature (magenta dots). Also
//! asserts numerically that magenta glow pixels appear.
//!
//! Run: cargo run --example creature_surface_collage

use unendlich_engine::sim::creature_from_image::surface_from_image;
use unendlich_engine::sim::dragon_archetype::Genome;
use unendlich_engine::sim::dragon_forge::{build, place_surface_glow};

const CELL: usize = 256;
const PAD: usize = 12;
const MAGENTA: [u8; 3] = [255, 0, 255];

fn main() {
    // ── 1. synthesize a reference image: dim body blob + 3 bright spots ──
    let (iw, ih) = (64usize, 64usize);
    let spots = [(32usize, 14usize), (24usize, 34usize), (40usize, 50usize)];
    let mut src = vec![0u8; iw * ih * 4];
    for y in 0..ih {
        for x in 0..iw {
            // a soft vertical body blob in the middle
            let dx = (x as f32 - 32.0) / 18.0;
            let body = (1.0 - dx * dx).max(0.0);
            let mut g = (40.0 + 50.0 * body) as i32;
            for &(sx, sy) in &spots {
                if x.abs_diff(sx) <= 2 && y.abs_diff(sy) <= 2 {
                    g = 255;
                }
            }
            let i = (y * iw + x) * 4;
            src[i] = g.min(255) as u8;
            src[i + 1] = (g as f32 * 0.55) as u8; // warmish body
            src[i + 2] = (g as f32 * 0.85) as u8;
            src[i + 3] = 255;
        }
    }

    // ── 2. extract surface traits ──
    let traits = surface_from_image(&src, iw, ih);
    println!(
        "surface_from_image: {} glow points, glow_strength={:.3}, roughness={:.3}",
        traits.glow_points.len(),
        traits.glow_strength,
        traits.roughness
    );

    // ── 3. build a creature and place glow from the image ──
    let mut model = build(&Genome::from_seed(7), 44);
    let glow_before = model.boxes.iter().filter(|b| b.glow).count();
    place_surface_glow(&mut model, &traits);
    let glow_after = model.boxes.iter().filter(|b| b.glow).count();
    println!("forge glow boxes: {glow_before} → {glow_after} (+{} from image)", glow_after - glow_before);

    // ── 4. collage: [ source | creature side view ] ──
    let w = 2 * CELL + 3 * PAD;
    let h = CELL + 2 * PAD;
    let mut buf = vec![0u8; w * h * 4];
    for px in buf.chunks_mut(4) {
        px[0] = 14;
        px[1] = 16;
        px[2] = 22;
        px[3] = 255;
    }

    // left panel: source image, nearest-neighbour upscaled
    blit_scaled(&mut buf, w, PAD, PAD, CELL, &src, iw, ih);

    // right panel: project model boxes side-on (z→x, y→up). Body grey-by-joint,
    // emissive boxes magenta, drawn last so glow sits on top.
    let ox = 2 * PAD + CELL;
    let oy = PAD;
    let (mn, mx) = (model.min, model.max);
    let zr = (mx[2] - mn[2]).max(1e-3);
    let yr = (mx[1] - mn[1]).max(1e-3);
    let mut magenta_px = 0usize;
    for pass in 0..2 {
        for b in &model.boxes {
            if b.glow != (pass == 1) {
                continue; // pass 0 = body, pass 1 = glow
            }
            let col = if b.glow { MAGENTA } else { joint_color(b.joint) };
            let cz = (b.center[2] - mn[2]) / zr;
            let cy = (b.center[1] - mn[1]) / yr;
            let hz = (b.half[2] / zr) * (CELL as f32 - 1.0);
            let hy = (b.half[1] / yr) * (CELL as f32 - 1.0);
            let px = cz * (CELL as f32 - 1.0);
            let py = (1.0 - cy) * (CELL as f32 - 1.0);
            let x0 = (px - hz).max(0.0) as usize;
            let x1 = ((px + hz) as usize).min(CELL - 1);
            let y0 = (py - hy).max(0.0) as usize;
            let y1 = ((py + hy) as usize).min(CELL - 1);
            for yy in y0..=y1 {
                for xx in x0..=x1 {
                    set_px(&mut buf, w, ox + xx, oy + yy, col);
                    if b.glow {
                        magenta_px += 1;
                    }
                }
            }
        }
    }

    let out = "/mnt/user-data/outputs/creature_surface_collage.png";
    write_png(out, w, h, &buf).expect("write png");
    println!("wrote {out} ({w}x{h}); magenta glow pixels in creature panel = {magenta_px}");
    assert!(magenta_px > 0, "glow should be visible (magenta) on the creature");
    assert!(glow_after > glow_before, "image glow should have been placed");
    println!("OK — image glow landed on the creature.");
}

fn joint_color(j: u8) -> [u8; 3] {
    let base = 70 + (j as u16 * 13 % 150) as u8;
    [base, (base as u16 * 3 / 4) as u8, (base as u16 * 6 / 5).min(255) as u8]
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

fn blit_scaled(buf: &mut [u8], w: usize, ox: usize, oy: usize, cell: usize, src: &[u8], iw: usize, ih: usize) {
    for yy in 0..cell {
        for xx in 0..cell {
            let sx = xx * iw / cell;
            let sy = yy * ih / cell;
            let si = (sy * iw + sx) * 4;
            set_px(buf, w, ox + xx, oy + yy, [src[si], src[si + 1], src[si + 2]]);
        }
    }
}

// ── dependency-free PNG (uncompressed zlib STORE) — copied from heightmap_collage ──
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
