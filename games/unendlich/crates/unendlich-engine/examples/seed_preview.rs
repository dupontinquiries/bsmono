//! Renders the menu's per-seed "surface run" preview thumbnails into one grid so
//! the previews can be eyeballed (terrain-shaped + visibly different per seed).
//! Run: cargo run --example seed_preview --features gpu

use unendlich_engine::world::cpu_generator::surface_run_preview;
use unendlich_engine::pngutil::write_png;

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

