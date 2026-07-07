//! examples/flora_scatter_map.rs — proves flora_placement is driven by the LIVE
//! generator via world::flora_map. Top-down over the real world: terrain base
//! (water/sand/ground/rock/snow by height) with each placed plant overlaid in its
//! species colour. Flora should cluster on temperate ground, thin out in desert,
//! and never appear on water, beach, bare rock, or peaks.
//! Run: cargo run --release --example flora_scatter_map --features gpu

use unendlich_engine::world::cpu_generator::WorldGenerator;
use unendlich_engine::world::flora_map::region_flora;
use unendlich_engine::world::flora_placement::FloraSpecies;

const SEED: u32 = 7;
const X0: i32 = -140;
const Z0: i32 = -140;
const N: i32 = 140; // cells per side
const SCALE: usize = 2;

fn species_color(s: FloraSpecies) -> [u8; 3] {
    use FloraSpecies::*;
    match s {
        Grass => [120, 205, 80],
        Shrub => [72, 140, 60],
        Flower => [236, 128, 200],
        Fern => [78, 182, 150],
        Reed => [150, 172, 70],
        Mushroom => [212, 170, 120],
        Cactus => [96, 172, 110],
        Conifer => [40, 112, 70],
        Broadleaf => [92, 192, 92],
        Palm => [184, 202, 92],
    }
}

fn terrain_base(sy: i32, sea: i32) -> [u8; 3] {
    if sy <= sea {
        let d = (sea - sy).clamp(0, 30) as f32 / 30.0;
        [(40.0 - 24.0 * d) as u8, (90.0 - 50.0 * d) as u8, (150.0 - 60.0 * d) as u8]
    } else if sy <= sea + 2 {
        [206, 196, 150] // beach
    } else {
        let a = (sy - sea) as f32;
        if a > 60.0 {
            [236, 240, 246] // snow peaks
        } else if a > 38.0 {
            [120, 116, 110] // bare rock
        } else {
            let t = (a / 38.0).clamp(0.0, 1.0);
            [(70.0 + 30.0 * t) as u8, (96.0 + 24.0 * t) as u8, (58.0 + 16.0 * t) as u8] // ground
        }
    }
}

fn main() {
    let g = WorldGenerator::new(SEED);
    let w = N as usize * SCALE;
    let h = N as usize * SCALE;
    let mut buf = vec![0u8; w * h * 4];

    // terrain base — cheap direct probes (no slope neighbours needed here)
    for cz in 0..N {
        for cx in 0..N {
            let (x, z) = (X0 + cx, Z0 + cz);
            let sy = g.surface_y_at(x, z, -16, 248).unwrap_or(0);
            let sea = g.sea_level_at(x, z).floor() as i32;
            put_cell(&mut buf, w, cx as usize, cz as usize, terrain_base(sy, sea));
        }
    }

    // flora overlay
    let flora = region_flora(&g, X0, Z0, N, N);
    let mut counts = [0u32; 10];
    for f in &flora {
        counts[f.species.index()] += 1;
        let cx = (f.pos[0] - X0) as usize;
        let cz = (f.pos[2] - Z0) as usize;
        put_cell(&mut buf, w, cx, cz, species_color(f.species));
    }

    let total: u32 = counts.iter().sum();
    println!("flora_scatter_map seed {SEED}: {total} plants over {N}x{N} cells");
    use FloraSpecies::*;
    for s in FloraSpecies::ALL {
        println!("  {:?}: {}", s, counts[s.index()]);
    }
    let _ = (Grass, Palm); // silence unused-import lint paths

    write_png("/mnt/user-data/outputs/flora_scatter_map.png", w, h, &buf).expect("png");
    println!("wrote flora_scatter_map.png ({w}x{h}) — terrain + per-species flora scatter");
}

fn put_cell(buf: &mut [u8], w: usize, cx: usize, cz: usize, c: [u8; 3]) {
    for dy in 0..SCALE {
        for dx in 0..SCALE {
            let x = cx * SCALE + dx;
            let y = cz * SCALE + dy;
            let i = (y * w + x) * 4;
            buf[i] = c[0];
            buf[i + 1] = c[1];
            buf[i + 2] = c[2];
            buf[i + 3] = 255;
        }
    }
}

fn write_png(path: &str, w: usize, h: usize, rgba: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ih = Vec::new();
    ih.extend_from_slice(&(w as u32).to_be_bytes());
    ih.extend_from_slice(&(h as u32).to_be_bytes());
    ih.extend_from_slice(&[8, 6, 0, 0, 0]);
    ch(&mut out, b"IHDR", &ih);
    let mut raw = Vec::with_capacity(h * (1 + w * 4));
    for y in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }
    ch(&mut out, b"IDAT", &zl(&raw));
    ch(&mut out, b"IEND", &[]);
    std::fs::File::create(path)?.write_all(&out)
}
fn ch(out: &mut Vec<u8>, k: &[u8; 4], d: &[u8]) {
    out.extend_from_slice(&(d.len() as u32).to_be_bytes());
    out.extend_from_slice(k);
    out.extend_from_slice(d);
    let mut c = 0xffff_ffffu32;
    for b in k.iter().chain(d.iter()) {
        c ^= *b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xedb8_8320 } else { c >> 1 };
        }
    }
    out.extend_from_slice(&(c ^ 0xffff_ffff).to_be_bytes());
}
fn zl(d: &[u8]) -> Vec<u8> {
    let mut z = vec![0x78, 0x01];
    let mut i = 0;
    while i < d.len() {
        let n = (d.len() - i).min(0xffff);
        let f = i + n >= d.len();
        z.push(if f { 1 } else { 0 });
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&d[i..i + n]);
        i += n;
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in d {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    z
}
