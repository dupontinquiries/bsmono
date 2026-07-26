//! Renders the menu's per-seed "surface run" preview thumbnails into one grid so
//! the previews can be eyeballed (terrain-shaped + visibly different per seed).
//! Run: cargo run --example seed_preview --features gpu

use unendlich_engine::world::cpu_generator::surface_run_preview;

fn main() {
    let seeds: [u32; 6] = [1, 7, 42, 99, 1234, 2025];
    let (cw, ch) = (220usize, 90usize); // each thumbnail
    let (cols, rows) = (3usize, 2usize);
    let pad = 8usize;
    let gw = cols * cw + (cols + 1) * pad;
    let gh = rows * ch + (rows + 1) * pad;
    let mut grid = vec![0u8; gw * gh * 4];
    for px in grid.chunks_mut(4) {
        px[0] = 18;
        px[1] = 20;
        px[2] = 26;
        px[3] = 255;
    }
    for (i, &seed) in seeds.iter().enumerate() {
        let (r, c) = (i / cols, i % cols);
        let thumb = surface_run_preview(seed, cw, ch);
        let ox = pad + c * (cw + pad);
        let oy = pad + r * (ch + pad);
        for y in 0..ch {
            for x in 0..cw {
                let s = (y * cw + x) * 4;
                let d = ((oy + y) * gw + (ox + x)) * 4;
                grid[d..d + 4].copy_from_slice(&thumb[s..s + 4]);
            }
        }
        println!("cell (row {r}, col {c}) = seed {seed}");
    }
    write_png("/mnt/user-data/outputs/seed_map_topdown.png", gw, gh, &grid).expect("png");
    println!("wrote seed_map_topdown.png ({gw}x{gh})");
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
