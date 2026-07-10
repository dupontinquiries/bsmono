//! WG1 r102 deep-depth proof: 20-panel vertical collage, surface → world floor.
//!   cargo run -p unendlich-engine --example cave_depth_collage
use unendlich_engine::world::cave_field::{cave_at, SURFACE_BUFFER, WORLD_FLOOR_Y};
use unendlich_engine::pngutil::write_png;

fn surf(x: f64) -> f64 { 72.0 + 10.0*(x*0.03).sin() + 4.0*(x*0.11+1.3).cos() }

fn main() {
    let seed = 12345u32;
    let panels = 20usize;
    let top0 = 80i32;                          // top of panel 0 (above surface)
    let bottom = WORLD_FLOOR_Y as i32 - 16;    // a bit below the floor
    let span = top0 - bottom;                  // total world-y covered
    let win = (span + panels as i32 - 1) / panels as i32; // blocks per panel

    let pw = 300usize;
    let ph = win as usize;
    let pad = 6usize; let label_h = 10usize;
    let cols = 4usize; let rows = (panels + cols - 1) / cols;
    let cellw = pw + pad; let cellh = ph + label_h + pad;
    let w = cols*cellw + pad; let h = rows*cellh + pad;
    let mut buf = vec![18u8; w*h*4];
    for p in buf.chunks_mut(4) { p[3]=255; }

    let mut deepest = i32::MAX; let mut total = 0u64;
    let z = 7.0;
    for pi in 0..panels {
        let win_top = top0 - (pi as i32)*win;
        let win_bot = win_top - win;
        let (cx, cy) = (pi % cols, pi / cols);
        let ox = pad + cx*cellw; let oy = pad + cy*cellh + label_h;
        let mut pc = 0u64;
        for sx in 0..pw {
            let x = (sx as f64) - (pw as f64)/2.0;
            let sy = surf(x);
            for row in 0..ph {
                let wy_i = win_top - 1 - row as i32; let wy = wy_i as f64;
                let ceiling = sy - SURFACE_BUFFER;
                let col: [u8;3] = if wy >= sy { [70,90,120] }
                    else if wy >= ceiling { [120,95,70] }
                    else if wy <= WORLD_FLOOR_Y { [40,40,46] }
                    else if cave_at(seed, x, wy, z, sy) {
                        pc += 1; if wy_i < deepest { deepest = wy_i; } [6,6,9]
                    } else { [95,70,50] };
                let o = ((oy+row)*w + (ox+sx))*4;
                buf[o]=col[0]; buf[o+1]=col[1]; buf[o+2]=col[2]; buf[o+3]=255;
                if wy_i == WORLD_FLOOR_Y as i32 { buf[o]=0; buf[o+1]=200; buf[o+2]=210; }
            }
        }
        total += pc;
        let lum = (40.0 + (pc as f64).min(4000.0)/4000.0*200.0) as u8;
        for lx in 0..pw { for ly in 0..label_h.saturating_sub(2) {
            let o = ((oy-label_h+ly)*w + (ox+lx))*4; buf[o]=lum; buf[o+1]=lum/2; buf[o+2]=20; buf[o+3]=255;
        }}
        println!("panel {:2}: y[{:5}..{:5}]  carved={}", pi, win_bot, win_top, pc);
    }
    println!("---\ndeepest carved voxel: y = {}", if deepest==i32::MAX {0} else {deepest});
    println!("WORLD_FLOOR_Y = {}  total carved = {}", WORLD_FLOOR_Y as i32, total);
    let path = "/mnt/user-data/outputs/cave_depth_collage.png";
    write_png(path, w, h, &buf).expect("png");
    println!("wrote {path} ({w}x{h}) — 20 panels, ~{win} blocks each, surface→world floor");
}
