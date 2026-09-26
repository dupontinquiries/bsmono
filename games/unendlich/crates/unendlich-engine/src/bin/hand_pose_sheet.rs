//! hand-pose-sheet — a contact sheet of the hand's moves, rendered offline.
//!
//! Rows are `sim::weapon_motion` moves, columns are points through each move
//! (rest, cocked, the blow, follow-through, recovering). Each cell draws the
//! hero rig (`hero_forge::build`, the same call the game makes for a forge
//! hero) holding the item from the side, with the player facing right. The
//! right-hand column of each row repeats the blow for a body with no rig of its
//! own, using `weapon_motion::hand_frame` on a stick figure (what the game uses
//! to put the item in a blueprint avatar's hand).
//!
//! Software-rasterised, std-only; writes a binary PPM. So a pose can be looked
//! at without a GPU, a world, or a lucky camera angle:
//!
//! ```sh
//! cargo run -p unendlich-engine --bin hand-pose-sheet -- /tmp/sheet.ppm axe:fire
//! convert /tmp/sheet.ppm /tmp/sheet.png
//! ```

use unendlich_engine::hero_forge::{self, HeroKind, HeroPose};
use unendlich_engine::item::{ItemKind, LegendarySpec};
use unendlich_engine::sim::legendary_weapon::WeaponClass;
use unendlich_engine::sim::weapon_motion::{self as wm, Motion};

const CELL_W: usize = 230;
const CELL_H: usize = 250;
/// Metres per `item_model` voxel (hero_forge::U).
const VOXEL: f32 = 0.072;

struct Canvas {
    w: usize,
    h: usize,
    rgb: Vec<[u8; 3]>,
    z: Vec<f32>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Canvas {
        Canvas { w, h, rgb: vec![[236, 236, 230]; w * h], z: vec![f32::MIN; w * h] }
    }

    /// A filled triangle (screen x, screen y, depth: larger is nearer).
    fn tri(&mut self, p: [[f32; 3]; 3], c: [u8; 3], clip: (usize, usize, usize, usize)) {
        let (x0, y0, x1, y1) = clip;
        let minx = p.iter().map(|q| q[0]).fold(f32::MAX, f32::min).floor().max(x0 as f32) as i64;
        let maxx = p.iter().map(|q| q[0]).fold(f32::MIN, f32::max).ceil().min(x1 as f32 - 1.0) as i64;
        let miny = p.iter().map(|q| q[1]).fold(f32::MAX, f32::min).floor().max(y0 as f32) as i64;
        let maxy = p.iter().map(|q| q[1]).fold(f32::MIN, f32::max).ceil().min(y1 as f32 - 1.0) as i64;
        let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
        if area.abs() < 1e-6 {
            return;
        }
        for y in miny..=maxy {
            for x in minx..=maxx {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((p[1][0] - px) * (p[2][1] - py) - (p[2][0] - px) * (p[1][1] - py)) / area;
                let w1 = ((p[2][0] - px) * (p[0][1] - py) - (p[0][0] - px) * (p[2][1] - py)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * p[0][2] + w1 * p[1][2] + w2 * p[2][2];
                let i = y as usize * self.w + x as usize;
                if z > self.z[i] {
                    self.z[i] = z;
                    self.rgb[i] = c;
                }
            }
        }
    }

    fn rect(&mut self, x: usize, y: usize, w: usize, h: usize, c: [u8; 3]) {
        for yy in y..(y + h).min(self.h) {
            for xx in x..(x + w).min(self.w) {
                self.rgb[yy * self.w + xx] = c;
            }
        }
    }

    fn save(&self, path: &str) -> std::io::Result<()> {
        let mut out = format!("P6\n{} {}\n255\n", self.w, self.h).into_bytes();
        for p in &self.rgb {
            out.extend_from_slice(p);
        }
        std::fs::write(path, out)
    }
}

/// Side view: world +X (the hero's forward at yaw 0) to screen right, +Y up,
/// nearer (+Z, the hero's right side) drawn over farther. A little turn
/// (`turn` radians about Y) shows depth.
fn project(p: [f32; 3], cell: (usize, usize), turn: f32, scale: f32, feet_y: f32) -> [f32; 3] {
    if top_view() {
        // From above: forward to screen right, the right side (+Z) down the
        // screen, higher drawn over lower; centred on the shoulders.
        return [
            cell.0 as f32 + CELL_W as f32 * 0.4 + p[0] * scale,
            cell.1 as f32 + CELL_H as f32 * 0.5 + p[2] * scale,
            p[1],
        ];
    }
    let (s, c) = turn.sin_cos();
    let x = p[0] * c - p[2] * s;
    let z = p[0] * s + p[2] * c;
    [
        cell.0 as f32 + CELL_W as f32 * 0.5 + x * scale,
        cell.1 as f32 + CELL_H as f32 - 18.0 - (p[1] - feet_y) * scale,
        z,
    ]
}

/// `HAND_SHEET_VIEW=top` draws every cell from above instead of the side.
fn top_view() -> bool {
    std::env::var("HAND_SHEET_VIEW").map(|v| v == "top").unwrap_or(false)
}

fn shade(c: [f32; 3], n: [f32; 3]) -> [u8; 3] {
    let l = [0.35f32, 0.8, 0.5];
    let ll = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
    let d = ((n[0] * l[0] + n[1] * l[1] + n[2] * l[2]) / ll).abs();
    let k = 0.45 + 0.55 * d;
    [(c[0] * k * 255.0).min(255.0) as u8, (c[1] * k * 255.0).min(255.0) as u8, (c[2] * k * 255.0).min(255.0) as u8]
}

fn draw_mesh(cv: &mut Canvas, verts: &[unendlich_engine::world::entity::EntityVertex], idx: &[u32], cell: (usize, usize), turn: f32, scale: f32, feet_y: f32) {
    let clip = (cell.0, cell.1, cell.0 + CELL_W, cell.1 + CELL_H);
    for t in idx.chunks(3) {
        if t.len() < 3 {
            break;
        }
        let a = verts[t[0] as usize].position;
        let b = verts[t[1] as usize].position;
        let c = verts[t[2] as usize].position;
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let nl = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-9);
        let col = shade(verts[t[0] as usize].color, [n[0] / nl, n[1] / nl, n[2] / nl]);
        cv.tri(
            [project(a, cell, turn, scale, feet_y), project(b, cell, turn, scale, feet_y), project(c, cell, turn, scale, feet_y)],
            col,
            clip,
        );
    }
}

/// A box with centre `c`, half extents along the three axes `ax` (columns).
fn push_box(verts: &mut Vec<unendlich_engine::world::entity::EntityVertex>, idx: &mut Vec<u32>, c: [f32; 3], h: [f32; 3], ax: [[f32; 3]; 3], color: [f32; 3]) {
    let base = verts.len() as u32;
    for &(sx, sy, sz) in &[
        (-1.0f32, -1.0f32, -1.0f32), (1.0, -1.0, -1.0), (1.0, 1.0, -1.0), (-1.0, 1.0, -1.0),
        (-1.0, -1.0, 1.0), (1.0, -1.0, 1.0), (1.0, 1.0, 1.0), (-1.0, 1.0, 1.0),
    ] {
        let p = [
            c[0] + ax[0][0] * sx * h[0] + ax[1][0] * sy * h[1] + ax[2][0] * sz * h[2],
            c[1] + ax[0][1] * sx * h[0] + ax[1][1] * sy * h[1] + ax[2][1] * sz * h[2],
            c[2] + ax[0][2] * sx * h[0] + ax[1][2] * sy * h[1] + ax[2][2] * sz * h[2],
        ];
        verts.push(unendlich_engine::world::entity::EntityVertex { position: p, color, alpha: 1.0 });
    }
    for f in [[0u32, 1, 2, 0, 2, 3], [4, 6, 5, 4, 7, 6], [0, 4, 5, 0, 5, 1], [3, 2, 6, 3, 6, 7], [0, 3, 7, 0, 7, 4], [1, 5, 6, 1, 6, 2]] {
        idx.extend(f.iter().map(|i| i + base));
    }
}

/// A stick figure facing +X with the item placed by `hand_frame`.
fn stick_figure(arm: &wm::ArmPose, model: &unendlich_engine::item_model::ItemModel) -> (Vec<unendlich_engine::world::entity::EntityVertex>, Vec<u32>) {
    let (mut v, mut i) = (Vec::new(), Vec::new());
    let id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let body = [0.55, 0.52, 0.48];
    push_box(&mut v, &mut i, [0.0, 1.15, 0.0], [0.12, 0.33, 0.2], id, body); // torso
    push_box(&mut v, &mut i, [0.0, 1.62, 0.0], [0.11, 0.12, 0.11], id, body); // head
    push_box(&mut v, &mut i, [0.0, 0.42, 0.1], [0.07, 0.42, 0.07], id, body); // legs
    push_box(&mut v, &mut i, [0.0, 0.42, -0.1], [0.07, 0.42, 0.07], id, body);
    let shoulder = [0.0, 1.42, 0.26];
    let (hand, b) = wm::hand_frame(arm, 0.3, 0.28);
    let hp = [shoulder[0] + hand[0], shoulder[1] + hand[1], shoulder[2] + hand[2]];
    let mid = [shoulder[0] + hand[0] * 0.5, shoulder[1] + hand[1] * 0.5, shoulder[2] + hand[2] * 0.5];
    push_box(&mut v, &mut i, mid, [0.05, 0.05, 0.05], id, [0.8, 0.3, 0.3]); // elbow-ish
    push_box(&mut v, &mut i, hp, [0.05, 0.05, 0.05], id, [0.8, 0.3, 0.3]); // fist
    for mb in &model.boxes {
        let h = mb.half();
        let c = [
            hp[0] + (b[0][0] * mb.center[0] + b[1][0] * mb.center[1] + b[2][0] * mb.center[2]) * VOXEL,
            hp[1] + (b[0][1] * mb.center[0] + b[1][1] * mb.center[1] + b[2][1] * mb.center[2]) * VOXEL,
            hp[2] + (b[0][2] * mb.center[0] + b[1][2] * mb.center[1] + b[2][2] * mb.center[2]) * VOXEL,
        ];
        push_box(&mut v, &mut i, c, [h[0] * VOXEL, h[1] * VOXEL, h[2] * VOXEL], b, mb.color);
    }
    (v, i)
}

fn spec_for(arg: &str) -> LegendarySpec {
    let mut it = arg.split(':');
    let class = match it.next().unwrap_or("axe") {
        "dagger" => WeaponClass::Dagger,
        "sword" | "blade" => WeaponClass::Blade,
        "greatsword" => WeaponClass::Greatsword,
        "greataxe" => WeaponClass::Greataxe,
        "spear" => WeaponClass::Spear,
        "staff" => WeaponClass::Staff,
        "wand" => WeaponClass::Wand,
        _ => WeaponClass::Axe,
    };
    let element_idx = match it.next().unwrap_or("dust") {
        "wood" => 0,
        "water" => 1,
        "air" => 2,
        "fire" => 4,
        "storm" => 5,
        _ => 3,
    };
    LegendarySpec { seed: 1, class, element_idx, tier: 2 }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).cloned().unwrap_or_else(|| "/tmp/hand_pose_sheet.ppm".into());
    let spec = spec_for(args.get(2).map(|s| s.as_str()).unwrap_or("axe:dust"));
    let turn: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.35);
    let model = ItemKind::Legendary(spec).model();
    let class = Some(spec.class);
    let kind = HeroKind::Stormweaver;
    let params = hero_forge::default_params(kind);
    let motions = [Motion::Chop, Motion::SlashR, Motion::SlashL, Motion::Thrust, Motion::Sweep, Motion::Flick, Motion::Throw, Motion::Catch, Motion::Work];
    let cols = 6;
    let mut cv = Canvas::new(CELL_W * cols, CELL_H * motions.len());
    for (row, &m) in motions.iter().enumerate() {
        let sw = wm::Swing::of(class, m, 1.0, 0);
        let s = sw.strike_at;
        let us = if s > 0.0 { [0.0, s * 0.75, s, s + (1.0 - s) * 0.35, s + (1.0 - s) * 0.75] } else { [0.0, 0.2, 0.35, 0.55, 0.8] };
        for (col, &u) in us.iter().enumerate() {
            let arm = wm::arm_at(m, s, u);
            let pose = HeroPose { held: Some(model.clone()), arm, ..HeroPose::default() };
            let (v, i) = hero_forge::build(kind, &params, [0.0, 0.0, 0.0], &pose);
            let feet = v.iter().map(|q| q.position[1]).fold(f32::MAX, f32::min);
            draw_mesh(&mut cv, &v, &i, (col * CELL_W, row * CELL_H), turn, 100.0, feet);
        }
        // The same blow on a body with no rig of its own.
        let (v, i) = stick_figure(&wm::arm_at(m, s, if s > 0.0 { s } else { 0.35 }), &model);
        draw_mesh(&mut cv, &v, &i, (5 * CELL_W, row * CELL_H), turn, 100.0, 0.0);
        // Row separator and a marker bar per motion (its index, as ticks).
        cv.rect(0, row * CELL_H, CELL_W * cols, 1, [120, 120, 120]);
        for k in 0..=row {
            cv.rect(4 + k * 6, row * CELL_H + 4, 4, 10, [60, 60, 160]);
        }
    }
    cv.save(&out).expect("write sheet");
    eprintln!("wrote {out}: rows {:?}; columns rest, cocked, blow, follow, recover, stick-figure blow", motions);
}
