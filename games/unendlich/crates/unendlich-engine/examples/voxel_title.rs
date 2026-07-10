//! Renders the floating voxel title front-on so it can be read as "UNENDLICH".
//! Run: cargo run --example voxel_title --features gpu

use unendlich_engine::sim::voxel_text::{voxel_text, voxel_text_width};
use unendlich_engine::pngutil::write_png;

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

