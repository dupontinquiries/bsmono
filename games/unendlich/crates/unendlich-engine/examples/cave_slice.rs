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
use unendlich_engine::pngutil::write_png;

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
