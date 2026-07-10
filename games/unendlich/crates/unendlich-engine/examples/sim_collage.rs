//! examples/sim_collage.rs — render a sim's spatial state to an image collage,
//! headlessly, so failures are VISIBLE instead of numeric. Demo: the dragon walk
//! cycle (the leg-swelling / leg-spacing bug we fought without being able to see
//! it). Each cell is a side-orthographic silhouette of `dragon_forge::skin` at one
//! gait phase, with LEG boxes tinted so swelling or spacing jumps out. The same
//! pattern renders biome maps, flow fields, or CPU meshes — anything spatial.
//!
//! Run: cargo run --example sim_collage --features gpu

use unendlich_engine::sim::dragon_archetype::Genome;
use unendlich_engine::pngutil::write_png;
use unendlich_engine::sim::dragon_forge::{
    self, ForgeAnim, ForgeBox, WingPose, J_HEAD, J_HIPS, J_JAW, J_LEG_BL, J_LEG_BR, J_LEG_FL, J_LEG_FR,
    J_NECK, J_SPINE, J_TAIL, J_WING_L, J_WING_R,
};

const CELL: usize = 480;
const COLS: usize = 3;
const ROWS: usize = 2;
const PAD: usize = 10;

fn main() {
    let model = dragon_forge::build(&Genome::from_seed(7), 48);
    let n = COLS * ROWS;
    let phases: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
    let frames: Vec<Vec<ForgeBox>> = phases
        .iter()
        .map(|&p| {
            let anim = ForgeAnim { gait_phase: p, anim_time: p * 2.0, ..Default::default() };
            dragon_forge::skin(&model, &anim)
        })
        .collect();

    // shared bounds (x = forward, y = up) so scale is constant across cells
    let (mut mnx, mut mny, mut mxx, mut mxy) = (1e9f32, 1e9f32, -1e9f32, -1e9f32);
    for f in &frames {
        for b in f {
            mnx = mnx.min(b.center[2] - b.half[2]);
            mxx = mxx.max(b.center[2] + b.half[2]);
            mny = mny.min(b.center[1] - b.half[1]);
            mxy = mxy.max(b.center[1] + b.half[1]);
        }
    }

    let w = COLS * CELL + (COLS + 1) * PAD;
    let h = ROWS * CELL + (ROWS + 1) * PAD;
    let mut buf = vec![0u8; w * h * 4];
    for px in buf.chunks_mut(4) {
        px[0] = 16;
        px[1] = 18;
        px[2] = 24;
        px[3] = 255;
    }

    for (i, f) in frames.iter().enumerate() {
        let ox = PAD + (i % COLS) * (CELL + PAD);
        let oy = PAD + (i / COLS) * (CELL + PAD);
        draw_cell(&mut buf, w, ox, oy, f, (mnx, mny, mxx, mxy));
    }

    write_png("/mnt/user-data/outputs/dragon_walk_collage.png", w, h, &buf).expect("png");
    println!("wrote dragon_walk_collage.png ({w}x{h}) phases={phases:?}");

    // Diagnostic: rest pose colored BY ASSIGNED JOINT, to see what's mis-rigged.
    let rest = dragon_forge::skin(&model, &ForgeAnim::default());
    let (mut nx, mut ny, mut xx, mut xy) = (1e9f32, 1e9f32, -1e9f32, -1e9f32);
    for b in &rest {
        nx = nx.min(b.center[2] - b.half[2]); xx = xx.max(b.center[2] + b.half[2]);
        ny = ny.min(b.center[1] - b.half[1]); xy = xy.max(b.center[1] + b.half[1]);
    }
    let jw = 1040usize; let jh = 840usize;
    let mut jb = vec![0u8; jw * jh * 4];
    for px in jb.chunks_mut(4) { px[0]=16; px[1]=18; px[2]=24; px[3]=255; }
    let sw=(xx-nx).max(1e-3); let sh=(xy-ny).max(1e-3);
    let sc=((jw as f32-30.0)/sw).min((jh as f32-30.0)/sh);
    let dx=(jw as f32-sw*sc)*0.5; let dy=(jh as f32-sh*sc)*0.5;
    let mut counts=[0u32;12];
    for (bi, b) in rest.iter().enumerate() {
        counts[b.joint as usize]+=1;
        let col = match b.joint {
            J_SPINE|J_HIPS => [150,156,168],
            J_NECK|J_HEAD => [240,210,90],
            J_JAW => [240,150,60],
            J_TAIL => [90,210,120],
            J_LEG_FL|J_LEG_FR|J_LEG_BL|J_LEG_BR => [235,70,70],   // RED = leg/foot
            J_WING_L|J_WING_R => [90,160,235],
            _ => [200,200,200],
        };
        let x0=dx+(b.center[2]-b.half[2]-nx)*sc; let y0=dy+(xy-(b.center[1]+b.half[1]))*sc;
        let x1=dx+(b.center[2]+b.half[2]-nx)*sc; let y1=dy+(xy-(b.center[1]-b.half[1]))*sc;
        let a = if b.glow { 255 } else { 220 };
        fill_rect_px(&mut jb, jw, x0, y0, x1, y1, col, a);
        if b.glow { // mark glow boxes with a magenta dot in the center
            let mx=dx+(b.center[2]-nx)*sc; let my=dy+(xy-b.center[1])*sc;
            fill_rect_px(&mut jb, jw, mx-4.0, my-4.0, mx+4.0, my+4.0, [255,40,220], 255);
        }
        // index label at the box center (point at the number when it breaks)
        let lx=dx+(b.center[2]-nx)*sc - 6.0; let ly=dy+(xy-b.center[1])*sc - 8.0;
        draw_int(&mut jb, jw, lx, ly, bi as u32, 3.0, [250,250,255]);
    }
    // indexed inventory: map a number you see -> what that box is
    let jn=|j:u8| ["SPINE","HIPS","NECK","HEAD","JAW","TAIL","LEG_FL","LEG_FR","LEG_BL","LEG_BR","WING_L","WING_R"].get(j as usize).copied().unwrap_or("?");
    println!("--- box inventory (index -> joint glow mouth center) ---");
    for (bi,b) in rest.iter().enumerate() {
        println!("  #{:<2} {:<6} glow={} mouth={} c=[{:.2},{:.2},{:.2}]",
            bi, jn(b.joint), b.glow as u8, b.mouth as u8, b.center[0],b.center[1],b.center[2]);
    }

        write_png("/mnt/user-data/outputs/dragon_joints.png", jw, jh, &jb).expect("png");
    println!("wrote dragon_joints.png — boxes/joint: SPINE={} HIPS={} NECK={} JAW={} TAIL={} LEGS={} WINGS={}",
        counts[0],counts[1],counts[2],counts[4],counts[5],
        counts[6]+counts[7]+counts[8]+counts[9], counts[10]+counts[11]);

    // TOP-DOWN view (x = lateral across, z = forward up) — the right angle to
    // judge wings + leg quadrants, which the side view collapses.
    let (mut tnx,mut tnz,mut txx,mut txz)=(1e9f32,1e9f32,-1e9f32,-1e9f32);
    for b in &rest {
        tnx=tnx.min(b.center[0]-b.half[0]); txx=txx.max(b.center[0]+b.half[0]);
        tnz=tnz.min(b.center[2]-b.half[2]); txz=txz.max(b.center[2]+b.half[2]);
    }
    let tw=1040usize; let th=840usize;
    let mut tb=vec![0u8; tw*th*4];
    for px in tb.chunks_mut(4){px[0]=16;px[1]=18;px[2]=24;px[3]=255;}
    let tsw=(txx-tnx).max(1e-3); let tsh=(txz-tnz).max(1e-3);
    let tsc=((tw as f32-40.0)/tsw).min((th as f32-40.0)/tsh);
    let tdx=(tw as f32-tsw*tsc)*0.5; let tdz=(th as f32-tsh*tsc)*0.5;
    for (bi,b) in rest.iter().enumerate() {
        let col = match b.joint {
            J_SPINE|J_HIPS => [150,156,168],
            J_NECK|J_HEAD => [240,210,90],
            J_JAW => [240,150,60],
            J_TAIL => [90,210,120],
            J_LEG_FL|J_LEG_FR|J_LEG_BL|J_LEG_BR => [235,70,70],
            J_WING_L|J_WING_R => [90,160,235],
            _ => [200,200,200],
        };
        // forward (high z) drawn at top
        let x0=tdx+(b.center[0]-b.half[0]-tnx)*tsc; let y0=tdz+(txz-(b.center[2]+b.half[2]))*tsc;
        let x1=tdx+(b.center[0]+b.half[0]-tnx)*tsc; let y1=tdz+(txz-(b.center[2]-b.half[2]))*tsc;
        fill_rect_px(&mut tb, tw, x0,y0,x1,y1, col, if b.glow {255} else {200});
        let lx=tdx+(b.center[0]-tnx)*tsc - 6.0; let ly=tdz+(txz-b.center[2])*tsc - 8.0;
        draw_int(&mut tb, tw, lx, ly, bi as u32, 3.0, [250,250,255]);
    }
    write_png("/mnt/user-data/outputs/dragon_joints_top.png", tw, th, &tb).expect("png");
    println!("wrote dragon_joints_top.png (top-down; wings spread laterally)");

    // TOP-DOWN WALK: render a few gait phases from above so wing radius is visible
    // (the side view collapses wings). Wings should hold their spread, not pull in.
    let tcell = 480usize; let tpad = 10usize; let tphases = [0.0f32, 1.57, 3.14, 4.71];
    let tcols = tphases.len();
    let ww = tcols*tcell + (tcols+1)*tpad; let wh = tcell + 2*tpad;
    let mut wb = vec![0u8; ww*wh*4];
    for px in wb.chunks_mut(4){px[0]=16;px[1]=18;px[2]=24;px[3]=255;}
    for (i,&ph) in tphases.iter().enumerate() {
        let frame = dragon_forge::skin(&model, &ForgeAnim{ gait_phase: ph, anim_time: ph, wing_pose: WingPose::Folded, ..Default::default() });
        let ox = tpad + i*(tcell+tpad); let oy = tpad;
        let wsc=((tcell as f32-30.0)/tsw).min((tcell as f32-30.0)/tsh);
        let wdx=ox as f32+(tcell as f32-tsw*wsc)*0.5; let wdz=oy as f32+(tcell as f32-tsh*wsc)*0.5;
        for b in &frame {
            let col = match b.joint {
                J_SPINE|J_HIPS => [150,156,168], J_NECK|J_HEAD => [240,210,90],
                J_JAW => [240,150,60], J_TAIL => [90,210,120],
                J_LEG_FL|J_LEG_FR|J_LEG_BL|J_LEG_BR => [235,70,70],
                J_WING_L|J_WING_R => [90,160,235], _ => [200,200,200],
            };
            let x0=wdx+(b.center[0]-b.half[0]-tnx)*wsc; let y0=wdz+(txz-(b.center[2]+b.half[2]))*wsc;
            let x1=wdx+(b.center[0]+b.half[0]-tnx)*wsc; let y1=wdz+(txz-(b.center[2]-b.half[2]))*wsc;
            fill_rect_px(&mut wb, ww, x0,y0,x1,y1, col, if b.glow {255} else {200});
        }
    }
    write_png("/mnt/user-data/outputs/dragon_walk_top.png", ww, wh, &wb).expect("png");
    println!("wrote dragon_walk_top.png (top-down gait; wings should hold radius)");
}

fn draw_cell(
    buf: &mut [u8],
    w: usize,
    ox: usize,
    oy: usize,
    boxes: &[ForgeBox],
    (mnx, mny, mxx, mxy): (f32, f32, f32, f32),
) {
    // faint cell panel
    fill_rect_px(
        buf,
        w,
        ox as f32,
        oy as f32,
        (ox + CELL) as f32,
        (oy + CELL) as f32,
        [26, 28, 36],
        255,
    );
    let inset = 14.0;
    let sw = (mxx - mnx).max(1e-3);
    let sh = (mxy - mny).max(1e-3);
    let s = ((CELL as f32 - inset) / sw).min((CELL as f32 - inset) / sh);
    let dxw = (CELL as f32 - sw * s) * 0.5;
    let dyw = (CELL as f32 - sh * s) * 0.5;
    let proj = |wx: f32, wy: f32| {
        (
            ox as f32 + dxw + (wx - mnx) * s,
            oy as f32 + dyw + (mxy - wy) * s, // flip Y (world up → image up)
        )
    };
    // ground line at world y = mny
    let (_, gy) = proj(mnx, mny);
    fill_rect_px(buf, w, ox as f32 + 6.0, gy, (ox + CELL) as f32 - 6.0, gy + 1.5, [60, 64, 74], 255);

    // body first, then legs, then glow on top
    for pass in 0..3 {
        for b in boxes {
            let is_leg = dragon_forge::is_leg_joint(b.joint);
            let (col, a) = if b.glow {
                ([255, 170, 60], 255u8)
            } else if b.mouth {
                ([30, 16, 16], 230)
            } else if is_leg {
                ([86, 200, 214], 235)
            } else {
                ([150, 156, 168], 220)
            };
            let layer = if b.glow { 2 } else if is_leg { 1 } else { 0 };
            if layer != pass {
                continue;
            }
            let (x0, y0) = proj(b.center[2] - b.half[2], b.center[1] + b.half[1]);
            let (x1, y1) = proj(b.center[2] + b.half[2], b.center[1] - b.half[1]);
            fill_rect_px(buf, w, x0, y0, x1, y1, col, a);
        }
    }
}

fn fill_rect_px(buf: &mut [u8], w: usize, x0: f32, y0: f32, x1: f32, y1: f32, col: [u8; 3], a: u8) {
    let h = buf.len() / (w * 4);
    let xa = x0.min(x1).floor().max(0.0) as usize;
    let xb = x1.max(x0).ceil().min(w as f32) as usize;
    let ya = y0.min(y1).floor().max(0.0) as usize;
    let yb = y1.max(y0).ceil().min(h as f32) as usize;
    let af = a as f32 / 255.0;
    for y in ya..yb {
        for x in xa..xb {
            let i = (y * w + x) * 4;
            for k in 0..3 {
                let d = buf[i + k] as f32;
                buf[i + k] = (col[k] as f32 * af + d * (1.0 - af)) as u8;
            }
        }
    }
}

// ── dependency-free PNG encoder (8-bit RGBA, stored zlib) ──
// ── tiny 3x5 bitmap digit font (dependency-free numeric labels) ──────────────
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111], // 0
    [0b010, 0b110, 0b010, 0b010, 0b111], // 1
    [0b111, 0b001, 0b111, 0b100, 0b111], // 2
    [0b111, 0b001, 0b111, 0b001, 0b111], // 3
    [0b101, 0b101, 0b111, 0b001, 0b001], // 4
    [0b111, 0b100, 0b111, 0b001, 0b111], // 5
    [0b111, 0b100, 0b111, 0b101, 0b111], // 6
    [0b111, 0b001, 0b010, 0b010, 0b010], // 7
    [0b111, 0b101, 0b111, 0b101, 0b111], // 8
    [0b111, 0b101, 0b111, 0b001, 0b111], // 9
];

fn draw_digit(buf: &mut [u8], w: usize, x: f32, y: f32, d: u8, scale: f32, col: [u8; 3]) {
    let g = DIGITS[(d % 10) as usize];
    for (row, bits) in g.iter().enumerate() {
        for c in 0..3 {
            if bits & (0b100 >> c) != 0 {
                let px = x + c as f32 * scale;
                let py = y + row as f32 * scale;
                fill_rect_px(buf, w, px, py, px + scale, py + scale, col, 255);
            }
        }
    }
}

fn draw_int(buf: &mut [u8], w: usize, x: f32, y: f32, n: u32, scale: f32, col: [u8; 3]) {
    let s = n.to_string();
    // dark shadow plate behind for legibility over colored boxes
    let glyph_w = 3.0 * scale + scale; // glyph + 1px gap
    let plate_w = s.len() as f32 * glyph_w + 2.0;
    fill_rect_px(buf, w, x - 1.0, y - 1.0, x + plate_w, y + 5.0 * scale + 1.0, [10, 10, 14], 200);
    for (i, ch) in s.bytes().enumerate() {
        draw_digit(buf, w, x + i as f32 * glyph_w, y, ch - b'0', scale, col);
    }
}
