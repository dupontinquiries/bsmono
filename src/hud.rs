//! Screen furniture: HUD (boost, speed, rings, goals, combo, air-dribble
//! timer, FPS), a north-up minimap, and the menu stack — pause, settings,
//! controls rebinding, map select, online. Everything is quads + the built-in
//! pixel font, drawn on translucent "glass" card panels (a homage to the
//! unendlich menu kit) with drop-shadowed text for readability. Layout
//! metrics are exported so main.rs can hit-test the mouse against the same
//! geometry that gets drawn.

use glam::Vec3;

use crate::font;
use crate::input::{Bindings, ACTIONS};
use crate::physics::{Ball, Car, CAR_MAX};
use crate::render::UiVert;
use crate::save::Save;
use crate::world::World;

pub struct HudInfo<'a> {
    pub aspect: f32,
    pub can_flip: bool,
    pub airborne: bool,
    pub time: f32,
    pub boost: f32,
    pub speed: f32,
    pub supersonic: bool,
    pub rings: u64,
    pub car_rings: u64,
    pub goals: u64,
    pub combo: u32,
    pub combo_left: f32, // 0..1 fraction of the combo window remaining
    pub air_t: f32,
    pub best_air: f32,
    pub fps: f32,
    pub car_name: &'a str,
    pub ball_cam: bool,
    pub minimap: bool,
    pub air_roll: bool,
    pub map_name: &'a str,
    pub online: Option<(usize, &'a str)>, // (player count, status/toast)
    pub scores: Option<(u32, u32)>,       // sparta scoreboard: (blue, orange)
}

fn quad(ui: &mut Vec<UiVert>, x0: f32, y0: f32, x1: f32, y1: f32, c: [f32; 4]) {
    let v = |x: f32, y: f32| UiVert { pos: [x, y], color: c };
    ui.extend_from_slice(&[v(x0, y0), v(x1, y0), v(x1, y1), v(x0, y0), v(x1, y1), v(x0, y1)]);
}

const INK: [f32; 4] = [0.10, 0.09, 0.13, 0.85];
const PAPER: [f32; 4] = [1.0, 0.98, 0.94, 0.95];
const GOLD: [f32; 4] = [1.0, 0.84, 0.30, 0.95];
const DIM: [f32; 4] = [1.0, 0.98, 0.94, 0.55];
const SEL: [f32; 4] = [0.17, 0.21, 0.25, 0.92]; // soldier's button fill
const CYAN: [f32; 4] = [0.59, 0.82, 0.92, 0.95];  // soldier's info color
const SHADOW: [f32; 4] = [0.02, 0.02, 0.05, 0.75];

/// Drop-shadowed text: one dark offset copy under the real one.
pub fn label(ui: &mut Vec<UiVert>, x: f32, y: f32, px: f32, a: f32, c: [f32; 4], s: &str) {
    font::text(ui, x + px * 0.16 / a, y - px * 0.16, px, a, SHADOW, s);
    font::text(ui, x, y, px, a, c, s);
}
fn label_c(ui: &mut Vec<UiVert>, y: f32, px: f32, a: f32, c: [f32; 4], s: &str) {
    let w = font::width(s, px, a);
    label(ui, -w * 0.5, y, px, a, c, s);
}

/// Truncate a string so it renders no wider than `maxw` NDC units.
fn fit(s: &str, maxw: f32, px: f32, a: f32) -> String {
    if font::width(s, px, a) <= maxw {
        return s.to_string();
    }
    let mut out: String = s.to_string();
    while !out.is_empty() && font::width(&format!("{out}.."), px, a) > maxw {
        out.pop();
    }
    format!("{out}..")
}

/// Footer legend: gold [KEY] tags with dim descriptions, centered.
pub fn legend(ui: &mut Vec<UiVert>, y: f32, a: f32, px_in: f32, pairs: &[(&str, &str)]) {
    let px = px_in.min(0.0078);
    let gap = font::width("   ", px, a);
    let mut total = 0.0;
    for (k, d) in pairs {
        total += font::width(&format!("[{k}] "), px, a) + font::width(d, px, a) + gap;
    }
    let mut x = -(total - gap) * 0.5;
    for (k, d) in pairs {
        let kt = format!("[{k}] ");
        label(ui, x, y, px, a, GOLD, &kt);
        x += font::width(&kt, px, a);
        label(ui, x, y, px, a, DIM, d);
        x += font::width(d, px, a) + gap;
    }
}

/// Translucent glass card with a thin accent border and a light top edge.
pub fn panel(ui: &mut Vec<UiVert>, a: f32, x0: f32, y0: f32, x1: f32, y1: f32) {
    // soldier's menu_frame: slate fill, slate-blue hairline stroke
    quad(ui, x0, y0, x1, y1, [0.070, 0.086, 0.110, 0.96]);
    let b = 0.0035;
    let stroke = [0.27, 0.33, 0.38, 0.9];
    quad(ui, x0, y1, x1, y1 + b, stroke);
    quad(ui, x0, y0 - b, x1, y0, stroke);
    quad(ui, x0 - b / a, y0, x0, y1, stroke);
    quad(ui, x1, y0, x1 + b / a, y1, stroke);
}

/// A darker inset card for grouping rows inside a menu (soldier-style).
fn inset(ui: &mut Vec<UiVert>, a: f32, x0: f32, y0: f32, x1: f32, y1: f32) {
    quad(ui, x0, y0, x1, y1, [0.094, 0.110, 0.133, 0.9]);
    let b = 0.0025;
    let stroke = [0.27, 0.33, 0.38, 0.55];
    quad(ui, x0, y1, x1, y1 + b, stroke);
    quad(ui, x0, y0 - b, x1, y0, stroke);
    quad(ui, x0 - b / a, y0, x0, y1, stroke);
    quad(ui, x1, y0, x1 + b / a, y1, stroke);
}

// -------------------------------------------------------- layout consts ----
pub const TW_TOP: f32 = 0.70;
pub const TW_BAR_X0: f32 = 0.26;
pub const TW_BAR_X1: f32 = 0.60;
pub fn rows_h(n: usize) -> f32 {
    (1.44 / n.max(1) as f32).min(0.095)
}
pub const BIND_TOP: f32 = 0.76;
pub const BIND_ROW_H: f32 = 0.049;
pub const MAPS_TOP: f32 = 0.70;
pub fn maps_row_h() -> f32 {
    rows_h(crate::world::STYLE_COUNT)
}
pub const ONL_TOP: f32 = 0.60;
pub const ONL_ROW_H: f32 = 0.105;

/// Which row (if any) is under a cursor y for a menu laid out like ours.
pub fn hit_row(top: f32, row_h: f32, n: usize, y: f32) -> Option<usize> {
    if y > top + row_h * 0.55 {
        return None;
    }
    let i = ((top + row_h * 0.45 - y) / row_h).floor();
    if i >= 0.0 && (i as usize) < n {
        Some(i as usize)
    } else {
        None
    }
}

// ------------------------------------------------------------------ hud ----
pub fn hud(ui: &mut Vec<UiVert>, info: &HudInfo, world: &World, car: &Car, ball: &Ball, bots: &[(Vec3, u8)]) {
    let a = info.aspect;

    // ---- boost bar (bottom center) ----
    quad(ui, -0.285, -0.955, 0.285, -0.895, [0.05, 0.05, 0.08, 0.62]);
    let f = (info.boost / 100.0).clamp(0.0, 1.0);
    let col = if info.supersonic { [1.0, 0.95, 0.75, 0.95] } else { [0.98, 0.68, 0.12, 0.92] };
    quad(ui, -0.28, -0.948, -0.28 + 0.56 * f, -0.902, col);
    font::text(ui, -0.275, -0.906, 0.015, a, INK, &format!("BOOST {:>3.0}", info.boost));

    // flip pip: shows in the air, gold while a flip is banked
    if info.airborne {
        let c = if info.can_flip { GOLD } else { [0.5, 0.48, 0.45, 0.55] };
        label(ui, 0.31, -0.906, 0.013, a, c, "FLIP");
    }

    // speed readout above the bar
    let sp = format!("{:>5.1} M/S", info.speed);
    label_c(ui, -0.845, 0.018, a, PAPER, &sp);
    let sf = (info.speed / CAR_MAX).clamp(0.0, 1.0);
    quad(ui, -0.28, -0.965, -0.28 + 0.56 * sf, -0.958, [0.55, 0.85, 1.0, 0.85]);
    if info.supersonic && (info.time * 6.0).sin() > -0.4 {
        label_c(ui, -0.80, 0.014, a, GOLD, "SUPERSONIC");
    }

    // ---- top-left: stat card ----
    let card_h = 0.185
        + if info.online.is_some() { 0.055 } else { 0.0 }
        + if info.air_roll { 0.028 } else { 0.0 };
    quad(ui, -1.0, 0.995 - card_h, -0.46, 0.995, [0.045, 0.045, 0.095, 0.45]);
    label(ui, -0.975, 0.975, 0.017, a, PAPER, &format!("RINGS {}", info.rings));
    label(ui, -0.975, 0.942, 0.013, a, GOLD, &format!("GOALS {}", info.goals));
    label(ui, -0.975, 0.913, 0.011, a, DIM, &format!("CAR-THROUGHS {}", info.car_rings));
    label(ui, -0.975, 0.888, 0.010, a, DIM, &format!("{}  ({})", info.car_name, if info.ball_cam { "BALL CAM" } else { "CAR CAM" }));
    label(ui, -0.975, 0.864, 0.010, a, DIM, info.map_name);
    let mut y = 0.838;
    if let Some((n, status)) = &info.online {
        label(ui, -0.975, y, 0.010, a, GOLD, &format!("ONLINE: {n} PILOTS"));
        y -= 0.026;
        if !status.is_empty() {
            label(ui, -0.975, y, 0.009, a, DIM, status);
        }
        y -= 0.028;
    }
    if info.air_roll {
        label(ui, -0.975, y, 0.010, a, GOLD, "AIR STEER = ROLL");
    }

    // ---- sparta scoreboard: BLUE n - m ORANGE across the top ----
    if let Some((b, o)) = info.scores {
        let px = 0.016;
        let bl = format!("BLUE {b}");
        let or = format!("{o} ORANGE");
        let dash = "  -  ";
        let w = font::width(&bl, px, a) + font::width(dash, px, a) + font::width(&or, px, a);
        let x0 = -w * 0.5;
        quad(ui, x0 - 0.03 / a, 0.906, x0 + w + 0.03 / a, 0.985, [0.070, 0.086, 0.110, 0.85]);
        let mut x = x0;
        label(ui, x, 0.93, px, a, [0.45, 0.65, 1.0, 1.0], &bl);
        x += font::width(&bl, px, a);
        label(ui, x, 0.93, px, a, DIM, dash);
        x += font::width(dash, px, a);
        label(ui, x, 0.93, px, a, [1.0, 0.55, 0.2, 1.0], &or);
    }

    // ---- top-right: fps ----
    let fps = format!("{:>3.0} FPS", info.fps);
    label(ui, 0.985 - font::width(&fps, 0.011, a), 0.98, 0.011, a, DIM, &fps);

    // ---- combo banner ----
    if info.combo >= 2 && info.combo_left > 0.0 {
        let pulse = 0.020 + 0.004 * (info.time * 9.0).sin().max(0.0);
        label_c(ui, 0.93, pulse, a, GOLD, &format!("COMBO X{}", info.combo));
        let w = 0.16;
        quad(ui, -w, 0.845, -w + 2.0 * w * info.combo_left, 0.855, GOLD);
    }

    // ---- air-dribble timer ----
    if info.air_t > 0.25 {
        let s = format!("AIR {:>4.1}S", info.air_t);
        let c = if info.air_t > info.best_air && info.best_air > 0.5 { GOLD } else { PAPER };
        label_c(ui, 0.80, 0.016, a, c, &s);
    }

    if info.minimap {
        minimap(ui, a, world, car, ball, bots);
    }
}

fn minimap(ui: &mut Vec<UiVert>, a: f32, world: &World, car: &Car, ball: &Ball, bots: &[(Vec3, u8)]) {
    let h = 0.28;
    let cx = 1.0 - (h + 0.05) / a;
    let cy = -1.0 + h + 0.05;
    let range = 170.0;
    let k = h / range;

    let x0 = cx - h / a;
    let x1 = cx + h / a;
    let (y0, y1) = (cy - h, cy + h);
    quad(ui, x0, y0, x1, y1, [0.06, 0.06, 0.10, 0.55]);
    let b = 0.005;
    quad(ui, x0, y1, x1, y1 + b, DIM);
    quad(ui, x0, y0 - b, x1, y0, DIM);
    quad(ui, x0 - b / a, y0, x0, y1, DIM);
    quad(ui, x1, y0, x1 + b / a, y1, DIM);
    font::text(ui, cx - font::width("N", 0.011, a) * 0.5, y1 + 0.028, 0.011, a, DIM, "N");

    let dot = |ui: &mut Vec<UiVert>, wp: Vec3, r: f32, c: [f32; 4]| {
        let dx = (wp.x - car.pos.x) * k;
        let dz = (wp.z - car.pos.z) * k;
        if dx.abs() > h - 0.01 || dz.abs() > h - 0.01 {
            return;
        }
        let px = cx + dx / a;
        let py = cy - dz;
        quad(ui, px - r / a, py - r, px + r / a, py + r, c);
    };

    for r in &world.rings {
        let c = world.shapes[r.shape].color;
        dot(ui, r.pos, 0.007, [c[0].sqrt(), c[1].sqrt(), c[2].sqrt(), 0.9]);
    }
    for n in &world.nets {
        // goals: bright green targets
        dot(ui, n.pos, 0.010, [0.45, 1.0, 0.55, 0.95]);
    }
    for p in world.pads.iter().filter(|p| p.big) {
        let c = if p.timer <= 0.0 { GOLD } else { [0.5, 0.42, 0.15, 0.6] };
        dot(ui, p.pos, 0.006, c);
    }
    for (bp, team) in bots {
        let t = crate::bots::team_tint(*team);
        dot(ui, *bp, 0.004, [t[0], t[1], t[2], 0.9]);
    }
    dot(ui, Vec3::ZERO, 0.005, [0.7, 0.9, 0.7, 0.8]);
    dot(ui, ball.pos, 0.008, [1.0, 1.0, 1.0, 1.0]);

    let f = car.forward();
    let d = glam::Vec2::new(f.x, -f.z).normalize_or(glam::Vec2::Y);
    let p = glam::Vec2::new(d.y, -d.x);
    let r = 0.018;
    let c2 = |v: glam::Vec2| [cx + v.x / a, cy + v.y];
    let tip = c2(d * r * 1.4);
    let bl = c2(-d * r + p * r * 0.8);
    let br = c2(-d * r - p * r * 0.8);
    let col = [0.55, 0.9, 1.0, 1.0];
    ui.extend_from_slice(&[
        UiVert { pos: tip, color: col },
        UiVert { pos: bl, color: col },
        UiVert { pos: br, color: col },
    ]);
}

// ---------------------------------------------------------------- pause ----
pub fn pause(ui: &mut Vec<UiVert>, a: f32, save: &Save, best_air_now: f32) {
    quad(ui, -1.0, -1.0, 1.0, 1.0, [0.05, 0.04, 0.09, 0.45]);
    panel(ui, a, -0.78, -0.90, 0.78, 0.90);
    label_c(ui, 0.78, 0.024, a, PAPER, "PAUSED");

    let lines = [
        "JUMP AGAIN IN THE AIR + A DIRECTION = FLIP.  OPPOSITE PITCH CANCELS IT",
        "HOLD SLIDE IN THE AIR: STEER BECOMES BARREL ROLL (ANALOG DEPTH BLENDS)",
        "WHEELS ON THE BALL MID-AIR = FLIP RESET.  GREEN-BACKED NETS = GOALS",
    ];
    let mut y = 0.60;
    for l in lines {
        label_c(ui, y, 0.0086, a, PAPER, l);
        y -= 0.050;
    }

    y -= 0.05;
    label_c(ui, y, 0.0115, a, GOLD, &format!(
        "BEST COMBO X{}    BEST AIR {:.1}S    TOP SPEED {:.1} M/S",
        save.best_combo.max(1),
        save.best_air.max(best_air_now),
        save.top_speed,
    ));
    y -= 0.05;
    label_c(ui, y, 0.0115, a, GOLD, &format!(
        "LIFETIME - RINGS {}   GOALS {}   CAR-THROUGHS {}",
        save.total_rings, save.total_goals, save.total_car_rings
    ));

    label_c(ui, -0.52, 0.0078, a, DIM, "MODELS: JAKO (FAIRLIGHT51) + 97MASON, CC-BY 4.0.  A FAN HOMAGE - ROCKET LEAGUE IS PSYONIX/EPIC'S.");
    label_c(ui, -0.56, 0.0078, a, DIM, "NETCODE, MAP MOODS + MENU KIT IDEAS FROM THE LOGSCOPE-C4 MONOREPO");

    legend(ui, -0.70, a, 0.0105, &[("ESC", "RESUME"), ("ENTER", "SETTINGS"), ("G", "MAPS"), ("O", "ONLINE")]);
    legend(ui, -0.765, a, 0.0105, &[("Q / X", "SAVE + QUIT"), ("MOUSE", "WORKS IN EVERY MENU")]);
}

// ------------------------------------------------------------- controls ----
pub const BIND_ROWS: usize = ACTIONS.len() + 3; // + classic, dvorak, back

pub fn bindings_menu(ui: &mut Vec<UiVert>, a: f32, b: &Bindings, sel: usize, capturing: bool) {
    quad(ui, -1.0, -1.0, 1.0, 1.0, [0.05, 0.04, 0.09, 0.45]);
    panel(ui, a, -0.80, -0.985, 0.80, 0.955);
    label_c(ui, 0.885, 0.019, a, PAPER, "CONTROLS");

    let px = 0.0074;
    let row_h = BIND_ROW_H;
    let top = BIND_TOP;
    let lx = -0.70;
    let rx = 0.16;
    for (i, act) in ACTIONS.iter().enumerate() {
        let y = top - i as f32 * row_h;
        if i == sel {
            quad(ui, -0.74, y - row_h + 0.016, 0.74, y + 0.016, SEL);
            quad(ui, -0.755, y - row_h + 0.016, -0.74, y + 0.016, GOLD);
        }
        let lab = fit(act.label(), rx - lx - 0.03, px, a);
        font::text(ui, lx, y, px, a, if i == sel { GOLD } else { PAPER }, &lab);
        let raw = if capturing && i == sel { "PRESS A KEY / MOUSE / PAD ...".to_string() } else { b.labels(*act) };
        let val = fit(&raw, 0.72 - rx, px, a);
        font::text(ui, rx, y, px, a, if capturing && i == sel { GOLD } else { DIM }, &val);
    }
    let extras = ["PRESET: CLASSIC (WASD)", "PRESET: DVORAK (.OEU HOME ROW)", "BACK"];
    for (k, lab) in extras.iter().enumerate() {
        let i = ACTIONS.len() + k;
        let y = top - i as f32 * row_h;
        if i == sel {
            quad(ui, -0.72, y - row_h + 0.016, 0.72, y + 0.016, SEL);
            quad(ui, -0.735, y - row_h + 0.016, -0.72, y + 0.016, GOLD);
        }
        font::text(ui, lx, y, px, a, if i == sel { GOLD } else { PAPER }, lab);
    }

    if capturing {
        legend(ui, -0.93, a, 0.0085, &[("ANY INPUT", "BIND IT"), ("DEL", "CLEAR"), ("ESC", "CANCEL")]);
    } else {
        legend(ui, -0.93, a, 0.0085, &[("CLICK / ENTER", "REBIND"), ("DEL", "CLEAR"), ("ESC", "BACK")]);
    }
}

// ----------------------------------------------------------------- maps ----
pub fn maps_menu(ui: &mut Vec<UiVert>, a: f32, sel: usize, current: usize, seed: u64) {
    quad(ui, -1.0, -1.0, 1.0, 1.0, [0.05, 0.04, 0.09, 0.45]);
    panel(ui, a, -0.62, -0.985, 0.62, 0.955);
    label_c(ui, 0.885, 0.019, a, PAPER, "MAPS");
    label_c(ui, 0.825, 0.0088, a, DIM, "TWENTY MOODS - ROOM STYLES FROM THE LOGSCOPE SCENE, PLUS NET-GOAL ARENAS");

    let row_h = maps_row_h();
    let px = 0.0090;
    let top = MAPS_TOP;
    for i in 0..crate::world::STYLE_COUNT {
        let st = crate::world::style(i);
        let y = top - i as f32 * row_h;
        if i == sel {
            quad(ui, -0.55, y - row_h + 0.020, 0.55, y + 0.020, SEL);
            quad(ui, -0.565, y - row_h + 0.020, -0.55, y + 0.020, GOLD);
        }
        quad(ui, -0.50, y - 0.024, -0.50 + 0.04 / a, y + 0.008, [st.grass[0].sqrt(), st.grass[1].sqrt(), st.grass[2].sqrt(), 1.0]);
        quad(ui, -0.50 + 0.05 / a, y - 0.024, -0.50 + 0.09 / a, y + 0.008, [st.accent[0].sqrt(), st.accent[1].sqrt(), st.accent[2].sqrt(), 1.0]);
        let mark = if i == current { ">" } else { " " };
        let pair = if st.focal == 8 || st.focal == 9 { 2 } else { 0 };
        let total_nets = st.nets + pair;
        let tag = if total_nets > 0 { format!("  ({total_nets} NETS)") } else { String::new() };
        font::text(ui, -0.40, y, px, a, if i == sel { GOLD } else { PAPER }, &format!("{mark} {}{tag}", st.name));
    }

    label_c(ui, -0.86, 0.0095, a, DIM, &format!("SEED {seed}"));
    legend(ui, -0.93, a, 0.0085, &[("CLICK / ENTER", "LOAD"), ("R", "NEW SEED"), ("ESC", "BACK")]);
}

// --------------------------------------------------------------- online ----
pub struct OnlineUi<'a> {
    pub sel: usize,
    pub name: &'a str,
    pub friend: &'a str,
    pub status: &'a str,
    pub players: Vec<String>,
    pub online: bool,
}

pub const ONLINE_ROWS: usize = 6; // name, friend, host, join, leave, back

pub fn online_menu(ui: &mut Vec<UiVert>, a: f32, o: &OnlineUi, blink: f32) {
    quad(ui, -1.0, -1.0, 1.0, 1.0, [0.05, 0.04, 0.09, 0.45]);
    panel(ui, a, -0.70, -0.985, 0.70, 0.955);
    label_c(ui, 0.885, 0.019, a, PAPER, "ONLINE");
    label_c(ui, 0.825, 0.0088, a, DIM, "SESSIONS RIDE THE FLEET NETCODE STORE - SAME MACHINE OR A SHARED TEMP DIR");

    let row_h = ONL_ROW_H;
    let top = ONL_TOP;
    let cursor = if (blink * 2.5).sin() > 0.0 { "_" } else { " " };
    let rows: [String; ONLINE_ROWS] = [
        format!("NAME:   {}{}", o.name, if o.sel == 0 { cursor } else { "" }),
        format!("FRIEND: {}{}", o.friend, if o.sel == 1 { cursor } else { "" }),
        "> HOST A SESSION".to_string(),
        "> JOIN FRIEND".to_string(),
        "> LEAVE / GO OFFLINE".to_string(),
        "BACK".to_string(),
    ];
    for (i, row) in rows.iter().enumerate() {
        let y = top - i as f32 * row_h;
        if i == o.sel {
            quad(ui, -0.62, y - row_h + 0.045, 0.62, y + 0.028, SEL);
            quad(ui, -0.635, y - row_h + 0.045, -0.62, y + 0.028, GOLD);
        }
        font::text(ui, -0.55, y, 0.0112, a, if i == o.sel { GOLD } else { PAPER }, row);
    }

    if !o.status.is_empty() {
        label_c(ui, -0.16, 0.0105, a, GOLD, o.status);
    }
    let mut y = -0.26;
    if o.online {
        label_c(ui, y, 0.0105, a, PAPER, "PILOTS IN SESSION:");
        y -= 0.048;
        for p in o.players.iter().take(8) {
            label_c(ui, y, 0.0095, a, DIM, p);
            y -= 0.042;
        }
    }

    label_c(ui, -0.86, 0.0085, a, DIM, "HOSTING ANNOUNCES YOU AS <NAME> FOR FRIENDS TO JOIN BY THAT NAME");
    legend(ui, -0.93, a, 0.0085, &[("TYPE", "EDIT FIELD"), ("CLICK / ENTER", "ACTIVATE"), ("ESC", "BACK")]);
}

// --------------------------------------------------------------- tweaks ----
pub struct TweakRow {
    pub label: String,
    pub value: String,
    pub bar: Option<f32>, // 0..1 gauge for sliders
    pub header: bool,     // section caption: not selectable, drawn as a rule
    pub action: bool,     // an "activates something" row: gets a chevron
}

pub fn tweaks_menu(ui: &mut Vec<UiVert>, a: f32, title: &str, rows: &[TweakRow], sel: usize) {
    quad(ui, -1.0, -1.0, 1.0, 1.0, [0.05, 0.04, 0.09, 0.45]);
    panel(ui, a, -0.80, -0.985, 0.80, 0.955);
    label_c(ui, 0.885, 0.017, a, PAPER, title);

    let row_h = rows_h(rows.len());
    let px = (row_h * 0.17).clamp(0.0072, 0.0094);
    let top = TW_TOP;
    // section inset cards first: each header owns the rows until the next
    let mut i0 = 0usize;
    while i0 < rows.len() {
        if rows[i0].header {
            let mut i1 = i0 + 1;
            while i1 < rows.len() && !rows[i1].header {
                i1 += 1;
            }
            if i1 > i0 + 1 {
                let y_top = top - i0 as f32 * row_h - row_h * 0.42;
                let y_bot = top - (i1 - 1) as f32 * row_h - row_h + 0.018;
                inset(ui, a, -0.76, y_bot, 0.76, y_top);
            }
            i0 = i1;
        } else {
            i0 += 1;
        }
    }
    for (i, r) in rows.iter().enumerate() {
        let y = top - i as f32 * row_h;
        if r.header {
            // section caption: cyan caps sitting on the inset's top edge
            label(ui, -0.73, y, px * 0.78, a, CYAN, &r.label);
            continue;
        }
        if i == sel {
            quad(ui, -0.74, y - row_h + 0.020, 0.74, y + 0.020, SEL);
            quad(ui, -0.755, y - row_h + 0.020, -0.74, y + 0.020, GOLD);
        }
        let lx = if r.action { -0.65 } else { -0.68 };
        if r.action {
            font::text(ui, -0.70, y, px, a, GOLD, ">");
        }
        let lab = fit(&r.label, 0.24 - lx, px, a);
        font::text(ui, lx, y, px, a, if i == sel { GOLD } else { PAPER }, &lab);
        let val = fit(&r.value, 0.42, px, a);
        let vw = font::width(&val, px, a);
        let vcol = if i == sel { CYAN } else { [1.0, 0.98, 0.94, 0.42] };
        font::text(ui, 0.70 - vw, y, px, a, vcol, &val);
        // slider bars only under the selected row: everything else is text,
        // which is most of the de-cluttering
        if i != sel {
            continue;
        }
        if let Some(t) = r.bar {
            let by = y - row_h * 0.55;
            quad(ui, TW_BAR_X0, by, TW_BAR_X1, by + 0.010, [0.22, 0.27, 0.32, 0.6]);
            quad(ui, TW_BAR_X0, by, TW_BAR_X0 + (TW_BAR_X1 - TW_BAR_X0) * t.clamp(0.0, 1.0), by + 0.010, GOLD);
        }
    }
    legend(ui, -0.93, a, 0.0085, &[
        ("ARROWS", "ADJUST"),
        ("ENTER", "OPEN / RESET"),
        ("WHEEL", "NUDGE"),
        ("ESC / RCLICK", "BACK"),
    ]);
}

// --------------------------------------------------------------- splash ----
const SLOGANS: [&str; 8] = [
    "THREAD THE RINGS. BURY IT IN THE NETS. NEVER LET IT DROP.",
    "SETUP IS 80% OF THE DRIBBLE.",
    "FEATHER THE BOOST.",
    "THE FLOOR IS OPTIONAL. SO ARE THE WALLS.",
    "TOUCH IT SOFTLY. IT REMEMBERS.",
    "ALL FOUR WHEELS ON THE BALL.",
    "HOLD SLIDE. BECOME THE BARREL.",
    "THIS. IS. SKYDRIBBLE.",
];

pub fn splash(ui: &mut Vec<UiVert>, a: f32, t: f32, map_name: &str) {
    quad(ui, -1.0, -1.0, 1.0, 1.0, [0.05, 0.04, 0.09, 0.40]);
    panel(ui, a, -0.72, -0.52, 0.72, 0.72);
    label_c(ui, 0.475, 0.046, a, [0.0, 0.0, 0.0, 0.5], "SKYDRIBBLE");
    label_c(ui, 0.49, 0.046, a, PAPER, "SKYDRIBBLE");
    label_c(ui, 0.26, 0.014, a, DIM, "AN OPEN-WORLD AIR-DRIBBLE PLAYGROUND");
    let slog = SLOGANS[((t / 3.6) as usize) % SLOGANS.len()];
    label_c(ui, 0.20, 0.010, a, GOLD, slog);
    label_c(ui, 0.08, 0.011, a, GOLD, &format!("MAP: {map_name}"));
    if (t * 2.2).sin() > -0.35 {
        label_c(ui, -0.14, 0.018, a, GOLD, "PRESS ANY KEY OR CLICK");
    }
    label_c(ui, -0.30, 0.010, a, DIM, "G MAPS    O ONLINE");
    label_c(ui, -0.44, 0.0085, a, DIM, "V1.26    ESC QUITS FROM THIS SCREEN");
}
