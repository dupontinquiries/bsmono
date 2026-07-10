//! examples/heightmap_collage.rs — render the world surface for several seeds into
//! one labeled collage, headless, so seed→worldgen is verifiable WITHOUT the bin:
//! different seeds must produce visibly different terrain. This is the landing pad
//! for the "worlds with various seeds" thread.
//! Run: cargo run --example heightmap_collage --features gpu

use unendlich_engine::world::cpu_generator::WorldGenerator;
use unendlich_engine::pngutil::write_png;

const SEEDS: [u32; 6] = [1337, 42, 7, 2024, 99999, 555];
const N: usize = 128; // samples per side
const STEP: i32 = 3; // world units between samples
const SCALE: usize = 2; // px per sample
const PAD: usize = 12;
const COLS: usize = 3;
const Y_MIN: i32 = 0;
const Y_MAX: i32 = 200;

fn main() {
    let cell = N * SCALE;
    let rows = SEEDS.len().div_ceil(COLS);
    let w = COLS * cell + (COLS + 1) * PAD;
    let h = rows * cell + (rows + 1) * PAD;
    let mut buf = vec![0u8; w * h * 4];
    for px in buf.chunks_mut(4) {
        px[0] = 14;
        px[1] = 16;
        px[2] = 22;
        px[3] = 255;
    }

    for (i, &seed) in SEEDS.iter().enumerate() {
        let gen = WorldGenerator::new(seed);
        // sample the heightmap + collect stats (proof the seeds differ)
        let mut hs = vec![0i32; N * N];
        let (mut lo, mut hi, mut sum) = (i32::MAX, i32::MIN, 0i64);
        for gz in 0..N {
            for gx in 0..N {
                let wx = gx as i32 * STEP;
                let wz = gz as i32 * STEP;
                let y = gen.surface_y_at(wx, wz, Y_MIN, Y_MAX).unwrap_or(Y_MIN);
                hs[gz * N + gx] = y;
                lo = lo.min(y);
                hi = hi.max(y);
                sum += y as i64;
            }
        }
        let mean = sum as f32 / (N * N) as f32;
        println!("seed {seed:>6}: surface y  min={lo} max={hi} mean={mean:.1}");

        let ox = PAD + (i % COLS) * (cell + PAD);
        let oy = PAD + (i / COLS) * (cell + PAD);
        for gz in 0..N {
            for gx in 0..N {
                let col = terrain_color(hs[gz * N + gx]);
                let px0 = ox + gx * SCALE;
                let py0 = oy + gz * SCALE;
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        let idx = ((py0 + dy) * w + (px0 + dx)) * 4;
                        buf[idx] = col[0];
                        buf[idx + 1] = col[1];
                        buf[idx + 2] = col[2];
                        buf[idx + 3] = 255;
                    }
                }
            }
        }
        // seed label
        draw_int(&mut buf, w, (ox + 4) as f32, (oy + 4) as f32, seed, 3.0, [250, 250, 255]);
    }
    write_png("/mnt/user-data/outputs/world_heightmaps.png", w, h, &buf).expect("png");
    println!("wrote world_heightmaps.png ({w}x{h}) seeds={SEEDS:?}");
}

/// A simple elevation ramp: deep→water blue, shore→tan, low→green, high→brown, peak→white.
fn terrain_color(y: i32) -> [u8; 3] {
    let sea = 62;
    if y <= sea - 6 {
        [30, 60, 120] // deep water
    } else if y <= sea {
        [60, 110, 165] // shallow water
    } else if y <= sea + 3 {
        [200, 190, 140] // beach
    } else if y <= sea + 30 {
        let t = (y - sea - 3) as f32 / 27.0;
        [(70.0 + 30.0 * t) as u8, (140.0 - 30.0 * t) as u8, (60.0 - 10.0 * t) as u8] // green→darker
    } else if y <= sea + 70 {
        let t = ((y - sea - 30) as f32 / 40.0).min(1.0);
        [(100.0 + 40.0 * t) as u8, (90.0 + 10.0 * t) as u8, (60.0 + 20.0 * t) as u8] // rock/brown
    } else {
        [225, 225, 235] // snow peak
    }
}

// ── tiny 3x5 digit font + PNG encoder (dependency-free) ──────────────────────
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];
fn fill_px(buf: &mut [u8], w: usize, x: usize, y: usize, col: [u8; 3]) {
    let total = buf.len() / (w * 4);
    if x < w && y < total {
        let i = (y * w + x) * 4;
        buf[i] = col[0];
        buf[i + 1] = col[1];
        buf[i + 2] = col[2];
        buf[i + 3] = 255;
    }
}
fn draw_int(buf: &mut [u8], w: usize, x: f32, y: f32, n: u32, scale: f32, col: [u8; 3]) {
    let s = n.to_string();
    let gw = 4.0 * scale;
    // dark plate
    for py in 0..(5.0 * scale + 2.0) as usize {
        for px in 0..(s.len() as f32 * gw + 2.0) as usize {
            fill_px(buf, w, x as usize + px, y as usize + py, [10, 10, 14]);
        }
    }
    for (i, ch) in s.bytes().enumerate() {
        let g = DIGITS[(ch - b'0') as usize];
        let gx0 = x as usize + 1 + i * (gw as usize);
        for (row, bits) in g.iter().enumerate() {
            for c in 0..3 {
                if bits & (0b100 >> c) != 0 {
                    for dy in 0..scale as usize {
                        for dx in 0..scale as usize {
                            let px = gx0 + c * scale as usize + dx;
                            let py = y as usize + 1 + row * scale as usize + dy;
                            fill_px(buf, w, px, py, col);
                        }
                    }
                }
            }
        }
    }
}

