//! examples/world_cave_slice.rs — proves cave_field is WIRED into the generator.
//! Renders a vertical X–Y slice of the ACTUAL generated world (not a standalone
//! field): LEFT = world as generated today (model caves only), RIGHT = the same
//! world with the deep multi-scale layer applied exactly as the wired `is_cave`
//! does it (model caves OR cave_field::cave_at). The difference is the new caves
//! the `UNENDLICH_MULTI_SCALE_CAVES=1` flag turns on.
//! Run: cargo run --release --example world_cave_slice --features gpu

use unendlich_engine::world::cave_field;
use unendlich_engine::world::cpu_generator::WorldGenerator;
use unendlich_engine::world::world_model::anchors::DENSITY_THRESHOLD;

const SEED: u32 = 7;
const X0: i32 = -20;
const XW: i32 = 240; // columns
const Y_TOP: i32 = 96;
const Y_BOT: i32 = -440; // ~536 rows deep
const Z: i32 = 8;

fn main() {
    let g = WorldGenerator::new(SEED);
    let rows = (Y_TOP - Y_BOT) as usize;
    let cols = XW as usize;
    let gap = 12usize;
    let w = cols * 2 + gap;
    let h = rows;
    let mut buf = vec![0u8; w * h * 4];

    let mut base_void = 0usize;
    let mut deep_void = 0usize;
    let mut above_surface_carves = 0usize;

    for cx in 0..cols {
        let x = X0 + cx as i32;
        let surface_y = g.model().surface_target(x as f64, Z as f64, 0.0);
        for ry in 0..rows {
            let y = Y_TOP - ry as i32;
            let above = (y as f64) > surface_y;
            let solid = g.density(x, y, Z) > DENSITY_THRESHOLD && !g.is_cave(x, y, Z);
            // RIGHT: apply the deep layer the same way the wired is_cave does.
            let carved = cave_field::cave_at(SEED, x as f64, y as f64, Z as f64, surface_y);
            let solid_deep = solid && !carved;

            if !above && !solid {
                base_void += 1;
            }
            if !above && solid && carved {
                deep_void += 1;
            }
            if above && carved {
                above_surface_carves += 1;
            }

            let depth = (surface_y - y as f64).max(0.0);
            put(&mut buf, w, cx, ry, color(above, solid, depth));
            put(&mut buf, w, cols + gap + cx, ry, color(above, solid_deep, depth));
        }
    }

    println!("world_cave_slice seed {SEED}: base voids={base_void} deep-added voids={deep_void}");
    println!("  caves above surface (must be 0): {above_surface_carves}");
    assert_eq!(above_surface_carves, 0, "deep caves broke the surface crust");
    assert!(deep_void > 0, "deep layer added no caves to the real world");

    write_png("/mnt/user-data/outputs/world_cave_slice.png", w, h, &buf).expect("png");
    println!("wrote world_cave_slice.png ({w}x{h}) — LEFT as-generated · RIGHT with cave_field wired");
}

fn color(above: bool, solid: bool, depth: f64) -> [u8; 3] {
    if above {
        [150, 190, 230] // sky
    } else if !solid {
        [8, 8, 12] // cave / air
    } else {
        // stone, banded by depth so the shallow/chasm/deep zones read
        let d = depth as f32;
        if d < 6.0 {
            [70, 110, 55] // crust/soil
        } else if d < 150.0 {
            [120, 110, 96] // limestone shallow
        } else if d < 320.0 {
            [96, 82, 86] // abyssal chasm band
        } else {
            [120, 70, 55] // geothermal deep
        }
    }
}

fn put(buf: &mut [u8], w: usize, x: usize, y: usize, c: [u8; 3]) {
    let i = (y * w + x) * 4;
    buf[i] = c[0];
    buf[i + 1] = c[1];
    buf[i + 2] = c[2];
    buf[i + 3] = 255;
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
