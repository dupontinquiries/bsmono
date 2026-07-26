//! WG1 (r102) headless proof: render a 2D vertical slice of the cave carve to a
//! PNG, for several seeds, and print the void-fraction assertions. Dependency-
//! free CPU rasteriser + PNG encoder (pattern copied from sim_collage.rs).
//!
//!   cargo run -p unendlich-engine --example cave_slice
//!
//! Each panel is a vertical (x,y) slice at fixed z: sky/crust grey, carved void
//! black, solid rock brown. A red line marks the surface; a faint line marks the
//! preserved crust ceiling (surface - SURFACE_BUFFER). You should see connected
//! tunnels + the occasional chamber, NONE breaching the crust, and the layout
//! changing per seed.

use unendlich_engine::world::cave_field::{cave_at, SURFACE_BUFFER, WORLD_FLOOR_Y};

fn surf(x: f64) -> f64 {
    72.0 + 10.0 * (x * 0.03).sin() + 4.0 * (x * 0.11 + 1.3).cos()
}

fn main() {
    let seeds = [12345u32, 2024, 777, 99999];
    let panel_w = 320usize;
    let panel_h = 96usize; // shallow detail window near surface
    let pad = 8usize;
    let cols = 2usize;
    let rows = (seeds.len() + cols - 1) / cols;
    let w = cols * panel_w + (cols + 1) * pad;
    let h = rows * panel_h + (rows + 1) * pad;
    let mut buf = vec![20u8; w * h * 4];
    for px in buf.chunks_mut(4) { px[3] = 255; }

    let z = 7.0;
    for (i, &seed) in seeds.iter().enumerate() {
        let (cx, cy) = (i % cols, i / cols);
        let ox = pad + cx * (panel_w + pad);
        let oy = pad + cy * (panel_h + pad);

        let (mut carved, mut considered) = (0u64, 0u64);
        for sx in 0..panel_w {
            let x = (sx as f64) - (panel_w as f64) / 2.0 + seed as f64 * 0.0; // centred
            let sy = surf(x);
            for syv in 0..panel_h {
                // screen y is top-down; world y is bottom-up
                let world_y = (panel_h - 1 - syv) as f64;
                let crust_ceiling = sy - SURFACE_BUFFER;

                let color: [u8; 3] = if world_y >= sy {
                    [70, 90, 120] // sky
                } else if world_y >= crust_ceiling {
                    [120, 95, 70] // preserved crust (never carved)
                } else if world_y < WORLD_FLOOR_Y {
                    [45, 45, 50] // bedrock
                } else {
                    considered += 1;
                    if cave_at(seed, x, world_y, z, sy) {
                        carved += 1;
                        [8, 8, 10] // void
                    } else {
                        [95, 70, 50] // rock
                    }
                };

                let pxo = ((oy + syv) * w + (ox + sx)) * 4;
                buf[pxo] = color[0];
                buf[pxo + 1] = color[1];
                buf[pxo + 2] = color[2];
                buf[pxo + 3] = 255;

                // surface line (red) + crust ceiling (faint)
                if (world_y - sy).abs() < 0.5 {
                    buf[pxo] = 220; buf[pxo + 1] = 40; buf[pxo + 2] = 40;
                } else if (world_y - crust_ceiling).abs() < 0.5 {
                    buf[pxo] = 180; buf[pxo + 1] = 160; buf[pxo + 2] = 60;
                }
            }
        }
        let frac = if considered > 0 { carved as f64 / considered as f64 } else { 0.0 };
        println!("seed {seed:>6}: void fraction = {:.3}% ({carved}/{considered})", frac * 100.0);
        assert!(frac > 0.001 && frac < 0.35, "seed {seed} void fraction out of range: {frac}");
    }

    let path = "/mnt/user-data/outputs/cave_slice.png";
    write_png(path, w, h, &buf).expect("png");
    println!("wrote {path} ({w}x{h}) — 4 seeds; black=void, brown=rock, red=surface, gold=crust ceiling");
}

// ── dependency-free PNG encoder (8-bit RGBA, stored zlib) — from sim_collage.rs ──
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
