//! Renders the floating voxel title front-on so it can be read as "UNENDLICH".
//! Run: cargo run --example voxel_title --features gpu

use unendlich_engine::sim::voxel_text::{voxel_text, voxel_text_width};

fn main() {
    let text = "UNENDLICH";
    let blocks = voxel_text(text);
    let wx = voxel_text_width(text); // glyph cols
    let hy = 7;
    let cell = 18usize;
    let pad = 16usize;
    let w = wx as usize * cell + 2 * pad;
    let h = hy as usize * cell + 2 * pad;
    let mut img = vec![0u8; w * h * 4];
    for px in img.chunks_mut(4) {
        px[0] = 14;
        px[1] = 16;
        px[2] = 22;
        px[3] = 255;
    }
    // draw far depth layer first (darker), then near (brighter) — a little 3D pop
    for &[bx, by, bz] in &blocks {
        let shade = if bz == 0 { 1.0 } else { 0.62 };
        // a cool cyan→violet gradient across the title
        let t = bx as f32 / wx as f32;
        let r = (90.0 + 120.0 * t) * shade;
        let g = (200.0 - 70.0 * t) * shade;
        let b = (230.0 - 20.0 * t) * shade;
        let ox = pad + bx as usize * cell + bz as usize * 3;
        let oy = pad + (hy as usize - 1 - by as usize) * cell + bz as usize * 3;
        for yy in 0..cell - 2 {
            for xx in 0..cell - 2 {
                let (px, py) = (ox + xx, oy + yy);
                if px < w && py < h {
                    let i = (py * w + px) * 4;
                    img[i] = r as u8;
                    img[i + 1] = g as u8;
                    img[i + 2] = b as u8;
                    img[i + 3] = 255;
                }
            }
        }
    }
    write_png("/mnt/user-data/outputs/unendlich_title.png", w, h, &img).expect("png");
    println!("wrote unendlich_title.png ({w}x{h}) — {} blocks", blocks.len());
}

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
