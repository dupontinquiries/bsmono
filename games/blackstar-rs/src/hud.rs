//! Screen furniture: HUD (boost, speed, rings, goals, combo, air-dribble
//! timer, FPS), a north-up minimap, and the menu stack — pause, settings,
//! controls rebinding, map select, online. Everything is quads + the built-in
//! pixel font, drawn on translucent "glass" card panels (a homage to the
//! unendlich menu kit) with drop-shadowed text for readability. Layout
//! metrics are exported so main.rs can hit-test the mouse against the same
//! geometry that gets drawn.

use glam::Vec3;

use crate::font;
use crate::ui as ui_layout;
use crate::input::{Bindings, ACTIONS};
use crate::physics::{Ball, Car, CAR_MAX};
use crate::render::UiVert;
use crate::save::Save;
use crate::world::World;

/// The equipped form and the Watch's attunement, together — the two
/// numbers that decide what a trigger pull actually does right now.
#[derive(Clone, Copy)]
pub struct WeaponHud {
    pub name: &'static str,
    /// 0 = ready, 1 = just fired
    pub cool_frac: f32,
    /// the attuned element's display name, or "attuning" mid-swap —
    /// `Watch::active()` is `Element::None` during that window, and the
    /// advantage table (`Element::advantage`) genuinely goes flat then,
    /// so the HUD saying so is accurate, not decorative
    pub element: &'static str,
    pub attuning: bool,
}

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
    /// A trial/lesson banner already owns the top-centre band this frame
    /// (`draw_hud` draws them before calling here). The combo readout sits
    /// in that same band (label at y=0.88, bar at 0.815..0.823) and a trial
    /// run legitimately drives combo — Do-tricks lessons read `self.combo`
    /// straight into their progress formula — so the two are not mutually
    /// exclusive in-game, only on screen. Suppress the readout rather than
    /// relocate it: the banner is the thing the player is meant to be
    /// reading in that moment, and a combo bar sliding underneath it would
    /// be more distracting than losing the number for a few seconds.
    pub banner_active: bool,
    pub air_t: f32,
    pub best_air: f32,
    pub fps: f32,
    /// frames abandoned in the last window (surface lost, outdated, timeout)
    pub frame_drops: u32,
    pub car_name: &'a str,
    pub ball_cam: bool,
    /// which camera, spelled out — there are three now
    pub cam_label: &'static str,
    /// first person wants a reticle and does not want a car-name card
    pub first_person: bool,
    /// `anim::Gait::wounded` — badly hurt and either hunched or hugging
    /// a wall. The gait logic already picks this correctly; it just had
    /// no way out of `anim.rs` to reach a HUD until now.
    pub wounded: bool,
    /// The torso's health as a fraction of its maximum (`sections::Body`): the number the
    /// death check runs on, which the HUD never showed -- "DAMAGED" pulsed, and the next hit
    /// was a surprise either way.
    pub hull: f32,
    /// Seconds until the downed body is rebuilt (`sections::Rebuild`), 0 while standing.
    pub rebuild_in: f32,
    /// the equipped form's name and how much of its cooldown remains
    /// (0 = ready, 1 = just fired), or `None` out of robot mode entirely
    pub weapon: Option<WeaponHud>,
    pub minimap: bool,
    pub air_roll: bool,
    pub map_name: &'a str,
    pub online: Option<(usize, &'a str)>, // (player count, status/toast)
    pub scores: Option<(u32, u32)>,       // sparta scoreboard: (blue, orange)
    /// MASTER ACCESS is on: this save has every progression gate disabled.
    ///
    /// Drawn because the flag persists and nothing else says so. A tester
    /// sends a screenshot or a recording, and without a marker in the frame
    /// neither of you can tell whether the bug came from a state a player can
    /// actually reach. The stderr line at launch does not survive a video.
    pub debug_access: bool,
}

fn quad(ui: &mut Vec<UiVert>, x0: f32, y0: f32, x1: f32, y1: f32, c: [f32; 4]) {
    let v = |x: f32, y: f32| UiVert { pos: [x, y], color: c };
    ui.extend_from_slice(&[v(x0, y0), v(x1, y0), v(x1, y1), v(x0, y0), v(x1, y1), v(x0, y1)]);
}

// ── THEME: "neon glass" — cyberpunk × unendlich (operator-directed mix) ──
// The custom blackstar art style arrives later; until then, unendlich's
// frosted-glass language (deep blue-glass panels, ice text, the #6fc3ff
// accent) crossed with cyberpunk neon (electric amber for the pick, hot
// magenta for warnings, harder darks). One palette block so every screen
// — pause, tweaks, maps, catalog, the shipboard consoles — shifts together
// and cannot disagree with itself. Previous "warm paper" palette in git.
const INK: [f32; 4] = [0.045, 0.060, 0.105, 0.88]; // deep glass backdrop
const PAPER: [f32; 4] = [0.874, 0.902, 0.957, 0.97]; // unendlich ice text (223,230,244)
const GOLD: [f32; 4] = [1.0, 0.80, 0.22, 0.97]; // electric amber — the pick
const DIM: [f32; 4] = [0.72, 0.77, 0.86, 0.55]; // ice text, faded
const SEL: [f32; 4] = [0.24, 0.35, 0.53, 0.62]; // unendlich glass-blue button fill
const CYAN: [f32; 4] = [0.435, 0.765, 1.0, 0.97]; // the unendlich accent, #6fc3ff
const WARN: [f32; 4] = [1.0, 0.24, 0.55, 0.97]; // hot magenta — cyber warn
const SHADOW: [f32; 4] = [0.01, 0.02, 0.05, 0.80];

/// Drop-shadowed text: one dark offset copy under the real one.
/// A filled circle in NDC (aspect-corrected), for the touch controls.
pub fn disc(ui: &mut Vec<UiVert>, cx: f32, cy: f32, r: f32, aspect: f32, c: [f32; 4]) {
    let seg = 28;
    for i in 0..seg {
        let a0 = i as f32 / seg as f32 * std::f32::consts::TAU;
        let a1 = (i + 1) as f32 / seg as f32 * std::f32::consts::TAU;
        let p = |a: f32| [cx + a.cos() * r / aspect, cy + a.sin() * r];
        let (p0, p1) = (p(a0), p(a1));
        ui.push(UiVert { pos: [cx, cy], color: c });
        ui.push(UiVert { pos: p0, color: c });
        ui.push(UiVert { pos: p1, color: c });
    }
}

/// A ring outline (two radii), same conventions as `disc`.
pub fn ring(ui: &mut Vec<UiVert>, cx: f32, cy: f32, r: f32, w: f32, aspect: f32, c: [f32; 4]) {
    let seg = 30;
    for i in 0..seg {
        let a0 = i as f32 / seg as f32 * std::f32::consts::TAU;
        let a1 = (i + 1) as f32 / seg as f32 * std::f32::consts::TAU;
        let p = |a: f32, rr: f32| [cx + a.cos() * rr / aspect, cy + a.sin() * rr];
        let (o0, o1) = (p(a0, r), p(a1, r));
        let (i0, i1) = (p(a0, r - w), p(a1, r - w));
        ui.push(UiVert { pos: o0, color: c });
        ui.push(UiVert { pos: i0, color: c });
        ui.push(UiVert { pos: o1, color: c });
        ui.push(UiVert { pos: i0, color: c });
        ui.push(UiVert { pos: i1, color: c });
        ui.push(UiVert { pos: o1, color: c });
    }
}

/// THE ACTION BAR, drawn — nine slots along the bottom edge, Minecraft's
/// own layout because it is the one players already read without being
/// taught: a row of equal cells, the active one lifted and outlined.
///
/// Only the active slot's label is written out. Nine simultaneous labels
/// is the clutter this project's own art bible warns against ("one
/// enormous readable idea beats twenty greebles"), and in a driving game
/// the bar is read peripherally — the slot NUMBER is the thing the hand
/// needs, the name is the thing the eye wants only for what it just
/// pressed.
pub fn draw_hotbar(ui: &mut Vec<UiVert>, a: f32, hb: &crate::hotbar::Hotbar) {
    // Used to return here on hb.is_empty() ("nothing armed: no furniture
    // on screen") -- but a fresh save has no hotbar line at all
    // (Hotbar::from_line("") assigns nothing), so a brand-new player
    // never saw the bar exist in the first place, empty or otherwise,
    // and had no way to discover where assigned weapons would even show
    // up. Always drawn now: nine numbered, empty-but-visible slots read
    // as "here is where your kit goes," not as nothing.
    let cells = crate::hotbar::SLOTS;
    let cw = 0.072;
    let gap = 0.008;
    let total = cells as f32 * cw + (cells - 1) as f32 * gap;
    let x0 = -total * 0.5;
    let y = HOTBAR_Y;
    let h = HOTBAR_H;
    for i in 0..cells {
        let cx = x0 + i as f32 * (cw + gap);
        let active = i == hb.active_index();
        let filled = hb.slot(i).is_some();
        // the active cell sits slightly proud and brighter — the lift is
        // what makes the row readable at a glance without reading text
        let lift = if active { HOTBAR_LIFT } else { 0.0 };
        let bg = if active {
            [0.10, 0.13, 0.18, 0.88]
        } else if filled {
            [0.05, 0.06, 0.09, 0.66]
        } else {
            [0.03, 0.03, 0.05, 0.42]
        };
        quad(ui, cx, y + lift, cx + cw, y + h + lift, bg);
        if active {
            // outline, drawn as four thin quads (no line primitive here)
            let t = 0.004;
            let c = [0.55, 0.78, 1.0, 0.95];
            quad(ui, cx, y + h + lift - t, cx + cw, y + h + lift, c);
            quad(ui, cx, y + lift, cx + cw, y + lift + t, c);
            quad(ui, cx, y + lift, cx + t, y + h + lift, c);
            quad(ui, cx + cw - t, y + lift, cx + cw, y + h + lift, c);
        }
        // the slot number, always — this is what the hand actually uses.
        // font::text draws DOWNWARD from its y, so the number starts just
        // under the cell's top edge; at 0.0045 the glyph is 0.0315 tall
        // and stays inside the 0.062 cell. It used to start near the cell
        // BOTTOM and hang 0.03 below it, which is where the recording
        // showed every digit: under the bar, in the item name's lane.
        let num = [b'1' + i as u8];
        let s = std::str::from_utf8(&num).unwrap_or("?");
        let ncol = if filled { [0.86, 0.90, 0.96, 0.95] } else { [0.45, 0.47, 0.52, 0.70] };
        label(ui, cx + 0.006, y + h + lift - 0.006, 0.0045, a, ncol, s);
    }
    // the active slot's name, centred under the bar
    if let Some(slot) = hb.active() {
        let name = match &slot.item {
            crate::gear::Item::Weapon { verb, element, .. } => {
                format!("{} {}", element.name(), crate::gear::verb_name(*verb))
            }
            crate::gear::Item::Armor(m) => m.name.to_string(),
        };
        let px = 0.0060;
        let w = font::width(&name, px, a);
        label(ui, -w * 0.5, y - 0.036, px, a, [0.78, 0.84, 0.92, 0.92], &name);
    }
}

pub fn label(ui: &mut Vec<UiVert>, x: f32, y: f32, px: f32, a: f32, c: [f32; 4], s: &str) {
    font::text(ui, x + px * 0.16 / a, y - px * 0.16, px, a, SHADOW, s);
    font::text(ui, x, y, px, a, c, s);
}
/// Centre a label on an arbitrary x (buttons, not just the screen middle).
/// RENDER A LAYOUT'S OUTPUT. The bridge between `ui.rs` and the primitives
/// this file already has.
///
/// `ui.rs` decides WHERE things go and never draws; this turns its answer
/// into quads and text. It is four lines, and it is what makes the layout
/// layer adoptable one panel at a time instead of as a rewrite — a migrated
/// panel and a hand-placed one sit side by side and neither knows.
pub fn draw_cmds(ui: &mut Vec<UiVert>, a: f32, cmds: &[ui_layout::Cmd]) {
    for c in cmds {
        match c {
            ui_layout::Cmd::Quad { r, color } => quad(ui, r.x0, r.y0, r.x1, r.y1, *color),
            ui_layout::Cmd::Text { x, y, size, color, text } => {
                label(ui, *x, *y, *size, a, *color, text)
            }
        }
    }
}

/// DRAW A MENU, end to end, through the abstractions.
///
/// `menu::Menu` -> `ui::menu_cmds` (layout) -> `draw_cmds` (primitives).
/// This is the first screen in the game that is native to the layer rather
/// than hand-placed: `Ship::bridge_menu` builds rows, this draws them, and
/// nobody wrote a coordinate. Adding a row to the bridge is now adding a
/// row, not editing a draw function and an input branch and hoping they
/// still agree.
/// DRAW THE DECK — a station console, posed by `menufx` and viewed
/// through `deck::DeckView`.
///
/// The look, in order of intent:
/// * the WORLD STAYS LIT. No full-screen sheet: a vignette of four edge
///   quads dims the frame's border and leaves the middle alive, because
///   a console on a ship has a ship behind it.
/// * the station's ACCENT is the room's light: a chamfered header rail
///   and the cursor bar carry it, so walking from armory to hold reads
///   as a lighting change before a single row is read.
/// * every row is drawn AT ITS POSE: slid along x by `offset`, faded by
///   `alpha`, and squeezed toward its centreline by `unfold` — the
///   cascade and the fold both fall out of the same three numbers.
/// * the cursor is the SPRING, not the selection index: it draws where
///   the motion says it is, stretched by its own speed, so it arrives a
///   beat after the input the way a thing with mass does.
pub fn draw_deck(ui: &mut Vec<UiVert>, a: f32, v: &crate::deck::DeckView) {
    let [ar, ag, ab] = v.accent();
    // vignette: edges only, centre untouched
    quad(ui, -1.0, -1.0, 1.0, -0.72, [0.02, 0.02, 0.05, 0.72]);
    quad(ui, -1.0, 0.72, 1.0, 1.0, [0.02, 0.02, 0.05, 0.72]);
    quad(ui, -1.0, -0.72, -0.62, 0.72, [0.02, 0.02, 0.05, 0.55]);
    quad(ui, 0.62, -0.72, 1.0, 0.72, [0.02, 0.02, 0.05, 0.55]);
    // header rail: the room's light, with a chamfer step — a plate, not
    // a line
    quad(ui, -0.60, 0.80, 0.60, 0.812, [ar, ag, ab, 0.85]);
    quad(ui, -0.58, 0.776, 0.40, 0.784, [ar, ag, ab, 0.35]);
    label(ui, -0.58, 0.83, 0.011, a, PAPER, v.title());
    label(ui, -0.58, 0.744, 0.0066, a, DIM, v.room());

    let top = DECK_TOP;
    let row_h = DECK_ROW_H;
    let rows = v.rows();
    // the cursor bar first, under the text: spring y in visible units
    let (cy, squash) = v.cursor();
    if cy >= -0.5 && (cy as usize) < rows.len().max(1) + 1 {
        let y = top - cy * row_h;
        let stretch = row_h * 0.5 * (1.0 + squash * 2.2);
        quad(ui, -0.585, y - stretch + row_h * 0.28, 0.585, y + stretch - row_h * 0.22, [ar, ag, ab, 0.20]);
        quad(ui, -0.585, y - stretch + row_h * 0.28, -0.575, y + stretch - row_h * 0.22, [ar, ag, ab, 0.9]);
    }
    for (vis_i, (_, row, pose, selected)) in rows.iter().enumerate() {
        if pose.alpha <= 0.01 {
            continue;
        }
        let y = top - vis_i as f32 * row_h;
        let x = -0.56 + pose.offset * 0.12;
        let fold = pose.unfold.clamp(0.05, 1.0);
        let px = T_BODY * fold;
        let base = if row.enabled { PAPER } else { DIM };
        let col = [base[0], base[1], base[2], base[3] * pose.alpha];
        let vcol = [ar, ag, ab, 0.9 * pose.alpha];
        let heading = matches!(row.item, crate::menu::Item::Heading);
        let hx = if heading { x - 0.02 } else { x };
        row_pair(ui, hx, 0.58, y, &row.label, col, &row.item.value(), vcol, px, a);
        let _ = selected;
    }
    // the selected row's hint, pinned low; feedback under it in accent
    if let Some(h) = rows.iter().find(|(_, _, _, s)| *s).and_then(|(_, r, _, _)| r.hint) {
        label(ui, -0.56, -0.80, 0.0066, a, DIM, h);
    }
    if let Some(f) = v.feedback() {
        label(ui, -0.56, -0.86, 0.0072, a, [ar, ag, ab, 0.95], f);
    }
    label(ui, -0.56, -0.93, 0.0058, a, DIM, "TAB next station   ESC fold shut   ENTER press");
}

pub fn draw_menu(ui: &mut Vec<UiVert>, a: f32, m: &crate::menu::Menu) {
    quad(ui, -1.0, -1.0, 1.0, 1.0, [0.05, 0.04, 0.09, 0.55]);
    let scr = ui_layout::Screen::new(a);
    // The menu's `view` is a hardcoded 12, set where no screen is in
    // scope. `menu_rows` now clamps to the bottom edge, so the rows past
    // the edge are no longer DRAWN — but `view` still decides which rows
    // `visible()` yields, and therefore what scrolling can reach.
    //
    // Not mutated here: `draw_menu` takes `&Menu`, and a draw function
    // that edits its model is how a value ends up depending on how often
    // the frame ran. The scroll clamp in `menu.rs` is the right place;
    // this comment marks the seam.
    let _fits = ui_layout::rows_that_fit(&scr, T_BODY);
    let cmds = ui_layout::menu_cmds(&scr, m, T_BODY, PAPER, DIM, [0.27, 0.45, 0.66, 0.55]);
    draw_cmds(ui, a, &cmds);
}

fn label_c_at(ui: &mut Vec<UiVert>, cx: f32, y: f32, px: f32, a: f32, c: [f32; 4], s: &str) {
    let w = font::width(s, px, a);
    label(ui, cx - w * 0.5, y, px, a, c, s);
}
fn label_c(ui: &mut Vec<UiVert>, y: f32, px: f32, a: f32, c: [f32; 4], s: &str) {
    let w = font::width(s, px, a);
    label(ui, -w * 0.5, y, px, a, c, s);
}

/// Truncate a string so it renders no wider than `maxw` NDC units.
/// Draw a NAME on the left and a VALUE on the right of the same row without
/// them ever touching.
///
/// The trick is measuring the value first: text width scales with 1/aspect,
/// so on a narrow window the same string is far wider in NDC and a label
/// laid out against a fixed budget marched straight into the value beside
/// it. The label now gets whatever is left after the value and a gap, so the
/// two cannot collide at any window shape.
#[allow(clippy::too_many_arguments)]
pub fn row_pair(
    ui: &mut Vec<UiVert>,
    left: f32,
    right: f32,
    y: f32,
    name: &str,
    name_col: [f32; 4],
    value: &str,
    value_col: [f32; 4],
    px: f32,
    a: f32,
) {
    let gap = 0.035;
    let vw = font::width(value, px, a).min((right - left) * 0.62);
    let avail = (right - left - vw - gap).max(0.08);
    font::text(ui, left, y, px, a, name_col, &fit(name, avail, px, a));
    if !value.is_empty() {
        let v = fit(value, (right - left) * 0.62, px, a);
        let w = font::width(&v, px, a);
        font::text(ui, right - w, y, px, a, value_col, &v);
    }
}

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
/// The sortie flow's panel — brief, drop, engagement, debrief — as rows the
/// flow itself composed (`Sortie::hud_rows`). The panel only knows where
/// text goes; the flow knows what matters. Row zero is the stage title;
/// rows starting with `·` are the event ticker and render dim.
pub fn sortie_panel(ui: &mut Vec<UiVert>, a: f32, rows: &[String]) {
    if rows.is_empty() {
        return;
    }
    let (x0, x1) = (-0.98, -0.50);
    // Below the stat card's tallest configuration (its panel bottom is
    // ~0.564 fully populated); the two share the left edge.
    let top = 0.54;
    // The pitch is the font's line advance for the row size. It was a
    // flat 0.034 — half a glyph — so a four-row brief drew every line
    // through the next.
    let line = font::CH * 0.0082;
    let h = 0.032 + line * rows.len() as f32;
    panel(ui, a, x0, top - h, x1, top);
    label(ui, x0 + 0.02 / a, top - 0.020, 0.0098, a, GOLD, &rows[0]);
    for (i, r) in rows.iter().enumerate().skip(1) {
        let c = if r.starts_with('·') { DIM } else { PAPER };
        label(ui, x0 + 0.02 / a, top - 0.020 - line * i as f32, 0.0082, a, c, r);
    }
}

pub fn panel(ui: &mut Vec<UiVert>, a: f32, x0: f32, y0: f32, x1: f32, y1: f32) {
    // soldier's menu_frame: slate fill, slate-blue hairline stroke
    quad(ui, x0, y0, x1, y1, [0.070, 0.086, 0.110, 0.96]);
    let b = PANEL_STROKE;
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
// TYPE SCALE — one ratio ladder instead of ad-hoc sizes per menu. Each step
// is ~1.25x the last, which is the standard readable progression, and the
// body size is deliberately small: dense settings screens read better with
// air around modest text than with big text crammed together.
/// Phone screens are narrow: a label sized for a desktop aspect renders
/// WIDER in NDC there and runs out of its button. Everything in the menus
/// scales through this, and long labels additionally get `fit()`.
pub fn ui_k(a: f32) -> f32 {
    // The floor used to be 0.62, which on a tall narrow window left text
    // wider than the panel it sat in — everything crowded and overlapped.
    (a / 1.70).clamp(0.46, 1.0)
}

pub const T_TITLE: f32 = 0.0138;
pub const T_SECTION: f32 = 0.0074;
pub const T_BODY: f32 = 0.0082;
pub const T_HINT: f32 = 0.0068;

// A row is a fixed, comfortable height — it does NOT shrink to fit the list.
// (It used to: `1.44 / n`, so every setting added made every menu tighter.
// Long lists scroll instead, which is what keeps rows readable and finger
// -sized no matter how many there are.)
pub const TW_ROW_H: f32 = 0.062;
/// Bottom of the scrolling viewport (above the legend).
pub const TW_BOTTOM: f32 = -0.78; // above the nav strip

/// The station console's own row geometry.
///
/// These were literals inside the draw call, so nothing else could ask how
/// many rows the panel holds — and `Menu::view` was a hardcoded 12 while the
/// panel fits twenty. The deck scrolled eight rows early, and the wardeck's
/// twenty rows scrolled when they would have fitted exactly.
///
/// Normalised UI space, so the answer does not depend on the display: this
/// is a geometry question, not a screen question, which is why it can be
/// settled here rather than by looking.
pub const DECK_TOP: f32 = 0.66;
pub const DECK_ROW_H: f32 = 0.072;

/// How many console rows the panel actually holds.
pub fn deck_rows() -> usize {
    vis_rows(DECK_ROW_H, DECK_TOP, TW_BOTTOM)
}

/// How many rows fit in a menu viewport.
pub fn vis_rows(row_h: f32, top: f32, bottom: f32) -> usize {
    (((top - bottom) / row_h).floor() as usize).max(3)
}

/// First visible index for a selection: the list scrolls only when the
/// selection would leave the viewport, and keeps a row of context (the
/// standard "scrolloff") at each edge.
pub fn scroll_for(sel: usize, n: usize, vis: usize) -> usize {
    if n <= vis {
        return 0;
    }
    let pad = 1usize;
    let max_start = n - vis;
    if sel < pad {
        0
    } else if sel + pad >= n {
        max_start
    } else {
        // centre-ish: keep the selection off both edges
        sel.saturating_sub(vis / 2).min(max_start)
    }
}

/// Draw a scrollbar for a scrolled list (only when it can scroll).
pub fn scrollbar(ui: &mut Vec<UiVert>, a: f32, x: f32, top: f32, bottom: f32, n: usize, vis: usize, start: usize) {
    if n <= vis {
        return;
    }
    let h = top - bottom;
    quad(ui, x, bottom, x + 0.008 / a, top, [0.20, 0.23, 0.30, 0.55]);
    let frac = vis as f32 / n as f32;
    let th = (h * frac).max(0.04);
    let t = start as f32 / (n - vis) as f32;
    let y1 = top - (h - th) * t;
    quad(ui, x, y1 - th, x + 0.008 / a, y1, [GOLD[0], GOLD[1], GOLD[2], 0.75]);
}

pub const TW_TOP: f32 = 0.70;
pub const TW_BAR_X0: f32 = 0.26;
pub const TW_BAR_X1: f32 = 0.60;
pub fn rows_h(n: usize) -> f32 {
    (1.44 / n.max(1) as f32).min(0.095)
}
pub const BIND_TOP: f32 = 0.76;
pub const BIND_ROW_H: f32 = 0.062; // finger-sized; the list scrolls instead
pub const MAPS_TOP: f32 = 0.70;
pub fn maps_row_h() -> f32 {
    // fixed: the map list scrolls rather than squeezing 30 rooms onto one page.
    // 0.062, not 0.058: at 0.058 a 0.0574 glyph clears by 1%, about a third of
    // a pixel at 1080p, and the list reads as a block rather than as rows.
    // This was missed by the row-pitch table one commit ago because it is a
    // FUNCTION and the table only walked constants — the same value, the same
    // bug, hidden by a different shape.
    MAPS_ROW_H
}

pub const MAPS_ROW_H: f32 = 0.062;

/// The replay banner: what the game decided you just did.
pub fn replay_banner(ui: &mut Vec<UiVert>, a: f32, title: &str, t: f32, total: f32) {
    // letterbox: the clearest possible "this is a replay"
    let bar = 0.12;
    quad(ui, -1.0, 1.0 - bar, 1.0, 1.0, [0.02, 0.02, 0.04, 0.92]);
    quad(ui, -1.0, -1.0, 1.0, -1.0 + bar, [0.02, 0.02, 0.04, 0.92]);
    label_c(ui, 1.0 - bar * 0.62, 0.0128, a, GOLD, title);
    label_c(ui, 1.0 - bar * 0.93, 0.0068, a, DIM, "REPLAY - ESC TO SKIP");
    // a scrub line along the bottom bar
    let p = (t / total.max(0.01)).clamp(0.0, 1.0);
    let y = -1.0 + bar * 0.45;
    quad(ui, -0.7, y, 0.7, y + 0.006, [0.25, 0.28, 0.34, 0.8]);
    quad(ui, -0.7, y, -0.7 + 1.4 * p, y + 0.006, GOLD);
}

/// The in-run banner: goal, clock, progress, and the time to beat.
#[allow(clippy::too_many_arguments)]
pub fn trial_banner(
    ui: &mut Vec<UiVert>,
    a: f32,
    name: &str,
    t: f32,
    limit: f32,
    progress: f32,
    best: f32,
    ghost: bool,
) {
    let (x0, x1) = (-0.34, 0.34);
    let y = 0.86;
    quad(ui, x0, y - 0.20, x1, y + 0.035, [0.06, 0.07, 0.11, 0.72]);
    quad(ui, x0, y + 0.030, x1, y + 0.035, [GOLD[0], GOLD[1], GOLD[2], 0.8]);
    // name glyphs: y+0.022 down to y-0.0466
    label_c_at(ui, 0.0, y + 0.022, 0.0098, a, PAPER, name);
    // the clock runs down, and reddens as it goes
    let left = (limit - t).max(0.0);
    let urgent = left < limit * 0.25;
    let col = if urgent { [1.0, 0.45, 0.35, 1.0] } else { CYAN };
    let clock = format!("{left:.1}s");
    // clock glyphs: y-0.058 down to y-0.1196, clear of the name above
    label_c_at(ui, 0.0, y - 0.058, 0.0088, a, col, &clock);
    // progress, below the clock's glyph bottom
    let by = y - 0.132;
    quad(ui, x0 + 0.02, by, x1 - 0.02, by + 0.008, [0.20, 0.24, 0.30, 0.7]);
    let w = (x1 - 0.02) - (x0 + 0.02);
    quad(ui, x0 + 0.02, by, x0 + 0.02 + w * progress.clamp(0.0, 1.0), by + 0.008, GOLD);
    let mut tail = String::new();
    if best > 0.0 {
        tail.push_str(&format!("BEST {best:.1}s"));
    }
    if ghost {
        if !tail.is_empty() {
            tail.push_str("  ");
        }
        tail.push_str("GHOST");
    }
    if !tail.is_empty() {
        // tail glyphs: y-0.144 down to y-0.1916, inside the box
        label_c_at(ui, 0.0, y - 0.144, 0.0068, a, DIM, &tail);
    }
}

/// The academy list: what each lesson teaches, and whether you have it.
pub fn academy_menu(
    ui: &mut Vec<UiVert>,
    a: f32,
    sel: usize,
    passed: &crate::academy::Passed,
    scroll: usize,
    cur: (f32, f32),
) {
    quad(ui, -1.0, -1.0, 1.0, 1.0, [0.05, 0.04, 0.09, 0.45]);
    panel(ui, a, -0.86, -0.985, 0.86, 0.955);
    label_c(ui, 0.885, T_TITLE * ui_k(a), a, PAPER, "ACADEMY");
    label_c(
        ui,
        0.828,
        T_HINT * ui_k(a),
        a,
        DIM,
        &fit(
            &format!("{} OF {} LEARNED", passed.count(), crate::academy::LESSONS.len()),
            1.62,
            T_HINT * ui_k(a),
            a,
        ),
    );
    let row_h = ACADEMY_ROW_H;
    let top = LIST_TOP;
    let n = crate::academy::LESSONS.len();
    let vis = vis_rows(row_h, top, TW_BOTTOM);
    let start = scroll.min(n.saturating_sub(vis.min(n)));
    let end = (start + vis).min(n);
    scrollbar(ui, a, 0.80, TW_BOTTOM, top + 0.02, n, vis, start);
    let k = ui_k(a);
    for i in start..end {
        let l = &crate::academy::LESSONS[i];
        let y = top - (i - start) as f32 * row_h;
        let got = passed.has(i);
        if i == sel {
            quad(ui, -0.80, y - row_h + 0.026, 0.80, y + 0.022, SEL);
            quad(ui, -0.817, y - row_h + 0.026, -0.802, y + 0.022, GOLD);
        }
        let col = if i == sel { GOLD } else if got { CYAN } else { PAPER };
        row_pair(ui, -0.78, 0.78, y, l.name, col, if got { "LEARNED" } else { "" }, CYAN, T_BODY * k, a);
        if i == sel {
            font::text(ui, -0.78, y - row_h * 0.5, T_HINT * k, a, DIM, &fit(l.hint, 1.5, T_HINT * k, a));
        }
    }
    nav_strip(ui, a, cur, 0);
    legend(ui, -0.93, a, T_HINT, &[("ENTER", "PRACTISE"), ("ESC", "BACK")]);
}

/// The in-lesson banner: the instruction, and how close you are.
pub fn lesson_banner(ui: &mut Vec<UiVert>, a: f32, name: &str, hint: &str, progress: f32, learned: bool) {
    let (x0, x1) = (-0.44, 0.44);
    let y = 0.86;
    quad(ui, x0, y - 0.130, x1, y + 0.035, [0.06, 0.07, 0.11, 0.72]);
    quad(ui, x0, y + 0.030, x1, y + 0.035, [GOLD[0], GOLD[1], GOLD[2], 0.8]);
    // name glyphs: y+0.022 down to y-0.0466
    label_c_at(ui, 0.0, y + 0.022, 0.0098, a, if learned { CYAN } else { PAPER }, name);
    // hint glyphs: y-0.058 down to y-0.1084, clear of the name
    label_c_at(ui, 0.0, y - 0.058, 0.0072, a, DIM, &fit(hint, 0.84, 0.0072, a));
    let by = y - 0.120;
    quad(ui, x0 + 0.02, by, x1 - 0.02, by + 0.008, [0.20, 0.24, 0.30, 0.7]);
    let w = (x1 - 0.02) - (x0 + 0.02);
    quad(ui, x0 + 0.02, by, x0 + 0.02 + w * progress.clamp(0.0, 1.0), by + 0.008, GOLD);
}

/// The garage. Four slots, the current pick, and what it does — the blurb
/// matters here because "MOIRE" tells you nothing on its own.
/// The catalogue: every vehicle, grouped by family, with the highlighted one
/// on the stage beside it.
///
/// The picker was a single row you cycled through — fine for nine cars and
/// useless for forty. Grouping by family turns one long run into seven short
/// ones, and showing the description of whatever is highlighted means you can
/// read the catalogue instead of driving it to find out.
#[allow(clippy::too_many_arguments)]
pub fn catalog_menu(
    ui: &mut Vec<UiVert>,
    a: f32,
    sel: usize,
    active: usize,
    names: &[&str],
    info: &[(&str, &str)],
    scroll: usize,
    cur: (f32, f32),
) {
    let wide = a > 1.25;
    let (px0, px1, py0, py1) = if wide {
        (-0.98, 0.10, -0.985, 0.955)
    } else {
        (-0.92, 0.92, -0.985, 0.42)
    };
    let (rx0, rx1) = (px0 + 0.07, px1 - 0.07);
    if wide {
        quad(ui, -1.0, -1.0, 0.14, 1.0, [0.05, 0.04, 0.09, 0.45]);
    } else {
        quad(ui, -1.0, -1.0, 1.0, 0.46, [0.05, 0.04, 0.09, 0.45]);
    }
    panel(ui, a, py1, py0, px1, px0);
    let k = ui_k(a);
    let title_y = if wide { 0.885 } else { 0.35 };
    label_c_at(ui, (px0 + px1) * 0.5, title_y, T_TITLE * k, a, PAPER, "CATALOGUE");
    label_c_at(
        ui,
        (px0 + px1) * 0.5,
        title_y - 0.052,
        T_HINT * k,
        a,
        DIM,
        &fit(&format!("{} VEHICLES", names.len()), (rx1 - rx0) * 0.9, T_HINT * k, a),
    );

    // build the display list: a header wherever the family changes
    let mut rows: Vec<(bool, usize)> = Vec::new(); // (is_header, car index)
    let mut last = "";
    for i in 0..names.len() {
        let cls = info.get(i).map(|x| x.0).unwrap_or("");
        if cls != last {
            rows.push((true, i));
            last = cls;
        }
        rows.push((false, i));
    }
    let row_h = if wide { CAT_ROW_H } else { CAT_ROW_H_TALL };
    let top = if wide { 0.72 } else { 0.26 };
    // through the shared helper: this was the one panel with its own copy of
    // the formula, so it also missed `vis_rows`'s floor — a layout that ever
    // squeezed to nothing would have given a window of zero rows here and a
    // usable three everywhere else
    let vis = vis_rows(row_h, top, TW_BOTTOM);
    let start = scroll.min(rows.len().saturating_sub(vis.min(rows.len())));
    let end = (start + vis).min(rows.len());
    scrollbar(ui, a, px1 - 0.03, TW_BOTTOM, top + 0.02, rows.len(), vis, start);
    for r in start..end {
        let (is_head, ci) = rows[r];
        let y = top - (r - start) as f32 * row_h;
        if is_head {
            font::text(ui, rx0, y, T_HINT * k, a, GOLD, info.get(ci).map(|x| x.0).unwrap_or(""));
            continue;
        }
        let picked = ci == sel;
        if picked {
            quad(ui, rx0 - 0.02, y - row_h + 0.022, rx1 + 0.02, y + 0.020, SEL);
            quad(ui, rx0 - 0.035, y - row_h + 0.022, rx0 - 0.024, y + 0.020, GOLD);
        }
        let col = if picked { GOLD } else if ci == active { CYAN } else { PAPER };
        let mark = if ci == active { "IN USE" } else { "" };
        row_pair(ui, rx0, rx1, y, names[ci], col, mark, CYAN, T_BODY * k, a);
    }
    // what the highlighted car is like
    let blurb = info.get(sel).map(|x| x.1).unwrap_or("");
    label_c_at(
        ui,
        (px0 + px1) * 0.5,
        TW_BOTTOM - 0.045,
        T_HINT * k,
        a,
        DIM,
        &fit(blurb, (rx1 - rx0) * 0.98, T_HINT * k, a),
    );
    nav_strip(ui, a, cur, 0);
    legend(ui, -0.93, a, T_HINT, &[("ENTER", "DRIVE IT"), ("ESC", "BACK")]);
}

/// The garage. The car itself is on stage beside (or above) this panel —
/// see the live view in `main.rs` — so the panel deliberately does NOT fill
/// the screen: it takes the left half on a wide window and the lower part of
/// a tall one, leaving the stage clear.
pub fn garage_menu(
    ui: &mut Vec<UiVert>,
    a: f32,
    sel: usize,
    look: &crate::custom::Loadout,
    cur: (f32, f32),
) {
    let wide = a > 1.25;
    // the panel's own bounds, and the x range its rows live in
    let (px0, px1, py0, py1) = if wide {
        (-0.98, 0.10, -0.985, 0.955)
    } else {
        (-0.92, 0.92, -0.985, 0.38)
    };
    let (rx0, rx1) = (px0 + 0.08, px1 - 0.08);
    // a dimmer that spares the stage, so the car stays bright
    if wide {
        quad(ui, -1.0, -1.0, 0.14, 1.0, [0.05, 0.04, 0.09, 0.45]);
    } else {
        quad(ui, -1.0, -1.0, 1.0, 0.42, [0.05, 0.04, 0.09, 0.45]);
    }
    panel(ui, a, py1, py0, px1, px0);
    let title_y = if wide { 0.885 } else { 0.31 };
    label_c_at(ui, (px0 + px1) * 0.5, title_y, T_TITLE * ui_k(a), a, PAPER, "GARAGE");
    label_c_at(ui, (px0 + px1) * 0.5, title_y - 0.057, T_HINT * ui_k(a), a, DIM, &fit("LEFT / RIGHT TO CHANGE", (rx1 - rx0) * 0.95, T_HINT * ui_k(a), a));
    let k = ui_k(a);
    let coll = look.matches_collection();
    let rows: [(&str, String, usize, usize); 7] = [
        (
            "COLLECTION",
            match coll {
                Some(i) => crate::custom::COLLECTIONS[i].name.to_string(),
                None => "CUSTOM".to_string(),
            },
            coll.unwrap_or(0),
            crate::custom::COLLECTIONS.len(),
        ),
        ("SKIN", look.skin_name().to_string(), look.skin, crate::custom::SKINS.len()),
        ("PAINT", look.paint_name().to_string(), look.paint, crate::custom::PAINTS.len()),
        ("RIG", look.rig_name().to_string(), look.rig, crate::custom::RIGS.len()),
        ("WHEELS", look.wheel_name().to_string(), look.wheels, crate::custom::WHEELS.len()),
        ("BOOST TRAIL", look.trail_name().to_string(), look.trail, crate::custom::TRAILS.len()),
        ("GOAL EXPLOSION", look.explosion_name().to_string(), look.explosion, crate::custom::EXPLOSIONS.len()),
    ];
    let top = if wide { GARAGE_TOP } else { 0.22 };
    let row_h = GARAGE_ROW_H;
    for (i, (name, val, idx, n)) in rows.iter().enumerate() {
        let y = top - i as f32 * row_h;
        if i == sel {
            quad(ui, rx0 - 0.02, y - row_h + 0.04, rx1 + 0.02, y + 0.030, SEL);
            quad(ui, rx0 - 0.037, y - row_h + 0.04, rx0 - 0.022, y + 0.030, GOLD);
        }
        let col = if i == sel { GOLD } else { PAPER };
        row_pair(ui, rx0, rx1, y, name, col, val, CYAN, T_BODY * k, a);
        // the paint row shows the palette itself; everything else gets dots
        if i == 2 {
            for (d, (_, rgb)) in crate::custom::PAINTS.iter().enumerate() {
                let x = rx0 + d as f32 * ((rx1 - rx0) / 13.0).min(0.030);
                let on = d == *idx;
                let h = if on { 0.020 } else { 0.012 };
                quad(ui, x, y - 0.040, x + ((rx1 - rx0) / 16.0).min(0.024), y - 0.040 + h, [rgb[0], rgb[1], rgb[2], 1.0]);
                if on {
                    quad(ui, x, y - 0.046, x + ((rx1 - rx0) / 16.0).min(0.024), y - 0.042, GOLD);
                }
            }
        } else {
            for d in 0..*n {
                let x = rx0 + d as f32 * ((rx1 - rx0) / 21.0).min(0.020);
                let on = d == *idx;
                quad(ui, x, y - 0.036, x + 0.009, y - 0.031, if on { GOLD } else { [0.3, 0.33, 0.38, 0.7] });
            }
        }
    }
    // what the current pick actually does
    let blurb = match sel {
        0 => match coll {
            Some(i) => crate::custom::COLLECTIONS[i].blurb.to_string(),
            None => "your own set — pick a collection to start from".to_string(),
        },
        1 => look.skin_blurb().to_string(),
        2 => "the finish is a behaviour; the paint is whose car it is".to_string(),
        3 => look.rig_blurb().to_string(),
        4 => format!("{} spokes", crate::custom::WHEELS[look.wheels].1),
        5 => crate::custom::TRAILS[look.trail].1.to_string(),
        _ => crate::custom::EXPLOSIONS[look.explosion].1.to_string(),
    };
    label_c_at(ui, (px0 + px1) * 0.5, top - 7.0 * row_h - 0.03, T_HINT * k, a, DIM, &fit(&blurb, (rx1 - rx0) * 0.98, T_HINT * k, a));
    nav_strip(ui, a, cur, 0);
    legend(ui, if wide { -0.93 } else { -0.93 }, a, T_HINT, &[("LEFT/RIGHT", "CHANGE"), ("ESC", "BACK")]);
}

/// Layout constants for the list menus, so hit-testing uses the SAME
/// numbers the drawing does. When these lived only inside the draw
/// functions, the new screens had no way to answer "which row is under the
/// cursor" and mouse input silently did nothing on them.
pub const LIST_TOP: f32 = 0.70;
pub const TRIALS_ROW_H: f32 = 0.062;
pub const ACADEMY_ROW_H: f32 = 0.062;
pub const GARAGE_TOP: f32 = 0.66;
/// the tall-window layout puts the rows lower, under the stage
pub const GARAGE_TOP_TALL: f32 = 0.22;
/// catalogue rows: the same numbers the drawing uses, so hit-testing agrees
pub const CAT_TOP: f32 = 0.72;
pub const CAT_TOP_TALL: f32 = 0.26;
/// 0.062, was 0.052. `catalog_menu` draws rows at `T_BODY * ui_k(a)`, and
/// `ui_k` clamps to 1.0 for any aspect at or above 1.70 — so at 16:9 a 0.0574
/// glyph was being drawn in a 0.052 row and the rows OVERLAPPED, on every
/// normal widescreen display, on the screen you pick a chassis from.
///
/// It survived the row-pitch sweep twice: the constants were not in the
/// clearance table, and `catalog_menu` did not even use them — it repeated
/// `0.052` and `0.046` as literals, so the constants here and the draw agreed
/// only by coincidence, which is what the comment above about hit-testing was
/// relying on.
pub const CAT_ROW_H: f32 = 0.062;
/// The narrow layout has less height, but a row that cannot hold its own text
/// is not denser, it is unreadable. The list scrolls.
pub const CAT_ROW_H_TALL: f32 = 0.062;
pub const GARAGE_ROW_H: f32 = 0.082;

/// EVERY LIST ROW PITCH, in one place.
///
/// Read by both `a_settings_row_of_text_fits_in_a_settings_row_of_space`
/// (which measures each against the glyph) and
/// `no_row_pitch_escapes_the_clearance_table` (which checks nothing exists
/// outside it). One table, so the check and the allowlist cannot drift — they
/// did, for exactly one commit, and GARAGE_ROW_H fell straight through the
/// gap.
///
/// `cfg(test)`: it is test data, and shipping it would be one more unreached
/// symbol in a binary that already carries 314 — the ratchet caught it as
/// exactly that.
#[cfg(test)]
pub const PITCHES: &[(&str, &str, f32)] = &[
    ("SETTINGS", "TW_ROW_H", TW_ROW_H),
    ("DECK", "DECK_ROW_H", DECK_ROW_H),
    ("CONTROLS", "BIND_ROW_H", BIND_ROW_H),
    ("TRIALS", "TRIALS_ROW_H", TRIALS_ROW_H),
    ("ACADEMY", "ACADEMY_ROW_H", ACADEMY_ROW_H),
    ("MAPS", "MAPS_ROW_H", MAPS_ROW_H),
    ("CATALOGUE", "CAT_ROW_H", CAT_ROW_H),
    ("CATALOGUE (narrow)", "CAT_ROW_H_TALL", CAT_ROW_H_TALL),
    ("GARAGE", "GARAGE_ROW_H", GARAGE_ROW_H),
];

/// The trials list: name, what it asks, which map, and your best.
pub fn trials_menu(
    ui: &mut Vec<UiVert>,
    a: f32,
    sel: usize,
    bests: &crate::trials::Bests,
    scroll: usize,
    cur: (f32, f32),
) {
    quad(ui, -1.0, -1.0, 1.0, 1.0, [0.05, 0.04, 0.09, 0.45]);
    panel(ui, a, -0.86, -0.985, 0.86, 0.955);
    label_c(ui, 0.885, T_TITLE * ui_k(a), a, PAPER, "TRIALS");
    let done = bests.finished();
    label_c(
        ui,
        0.828,
        T_HINT * ui_k(a),
        a,
        DIM,
        &fit(&format!("{done} OF {} COMPLETE", crate::trials::TRIALS.len()), 1.62, T_HINT * ui_k(a), a),
    );
    let row_h = TRIALS_ROW_H;
    let top = LIST_TOP;
    let n = crate::trials::TRIALS.len();
    let vis = vis_rows(row_h, top, TW_BOTTOM);
    let start = scroll.min(n.saturating_sub(vis.min(n)));
    let end = (start + vis).min(n);
    scrollbar(ui, a, 0.80, TW_BOTTOM, top + 0.02, n, vis, start);
    let k = ui_k(a);
    for i in start..end {
        let t = &crate::trials::TRIALS[i];
        let y = top - (i - start) as f32 * row_h;
        let best = bests.get(i);
        if i == sel {
            quad(ui, -0.80, y - row_h + 0.026, 0.80, y + 0.022, SEL);
            quad(ui, -0.817, y - row_h + 0.026, -0.802, y + 0.022, GOLD);
        }
        let col = if i == sel { GOLD } else if best > 0.0 { CYAN } else { PAPER };
        let right = if best > 0.0 { format!("{best:.1}s") } else { "-".to_string() };
        // the map name only appears when there is room for it
        let label = if a > 1.35 {
            format!("{}   {}", t.name, crate::world::style(t.map).name)
        } else {
            t.name.to_string()
        };
        row_pair(ui, -0.78, 0.78, y, &label, col, &right, if best > 0.0 { CYAN } else { DIM }, T_BODY * k, a);
        if i == sel {
            font::text(ui, -0.78, y - row_h * 0.52, T_HINT * k, a, DIM, &fit(t.blurb, 1.4, T_HINT * k, a));
        }
    }
    nav_strip(ui, a, cur, 0);
    legend(ui, -0.93, a, T_HINT, &[("ENTER", "START"), ("ESC", "BACK")]);
}

// ---- menu NAV STRIP: jump straight to any screen with a thumb ----
pub const NAV_BTNS: [&str; 6] = ["BACK", "SETUP", "CONTROLS", "MAPS", "VIDEO", "ONLINE"];
pub const NAV_Y: f32 = -0.885;
pub const NAV_H: f32 = 0.070;
pub const NAV_W: f32 = 0.235;
pub const NAV_GAP: f32 = 0.012;

pub fn nav_x0(i: usize) -> f32 {
    let n = NAV_BTNS.len() as f32;
    let total = n * NAV_W + (n - 1.0) * NAV_GAP;
    -total * 0.5 + i as f32 * (NAV_W + NAV_GAP)
}

pub fn hit_nav(x: f32, y: f32) -> Option<usize> {
    if y < NAV_Y - NAV_H * 0.5 || y > NAV_Y + NAV_H * 0.5 {
        return None;
    }
    (0..NAV_BTNS.len()).find(|&i| {
        let x0 = nav_x0(i);
        x >= x0 && x <= x0 + NAV_W
    })
}

pub fn nav_strip(ui: &mut Vec<UiVert>, a: f32, cur: (f32, f32), here: usize) {
    let k = ui_k(a);
    for (i, name) in NAV_BTNS.iter().enumerate() {
        let x0 = nav_x0(i);
        let (y0, y1) = (NAV_Y - NAV_H * 0.5, NAV_Y + NAV_H * 0.5);
        let hot = hit_nav(cur.0, cur.1) == Some(i);
        let on = here == i;
        let bg = if hot {
            [0.24, 0.30, 0.38, 0.95 * a]
        } else if on {
            [0.18, 0.21, 0.28, 0.92 * a]
        } else {
            [0.12, 0.14, 0.19, 0.82 * a]
        };
        quad(ui, x0, y0, x0 + NAV_W, y1, bg);
        if hot || on {
            quad(ui, x0, y1 - 0.005, x0 + NAV_W, y1, [GOLD[0], GOLD[1], GOLD[2], (if hot { 0.9 } else { 0.5 }) * a]);
        }
        let txt = fit(name, NAV_W * 0.86, T_HINT * k, a);
        label_c_at(ui, x0 + NAV_W * 0.5, NAV_Y - 0.010 * k, T_HINT * k, a, if hot || on { PAPER } else { DIM }, &txt);
    }
}

// ---- pause screen touch buttons (a phone has no ESC key) ----
pub const PAUSE_BTNS: [&str; 5] = ["RESUME", "SETTINGS", "MAPS", "ONLINE", "QUIT"];
pub const PBTN_Y: f32 = -0.70;
pub const PBTN_H: f32 = 0.075;
pub const PBTN_W: f32 = 0.285;
pub const PBTN_GAP: f32 = 0.015;

/// Left edge of pause button `i` (buttons are centred as a row).
pub fn pbtn_x0(i: usize) -> f32 {
    let n = PAUSE_BTNS.len() as f32;
    let total = n * PBTN_W + (n - 1.0) * PBTN_GAP;
    -total * 0.5 + i as f32 * (PBTN_W + PBTN_GAP)
}

/// Which pause button (if any) is under a point in clip space.
pub fn hit_pause_btn(x: f32, y: f32) -> Option<usize> {
    if y < PBTN_Y - PBTN_H * 0.5 || y > PBTN_Y + PBTN_H * 0.5 {
        return None;
    }
    (0..PAUSE_BTNS.len()).find(|&i| {
        let x0 = pbtn_x0(i);
        x >= x0 && x <= x0 + PBTN_W
    })
}

/// Slider value fraction for a touch/click x on the bar (clamped).
pub fn bar_t(x: f32) -> f32 {
    ((x - TW_BAR_X0) / (TW_BAR_X1 - TW_BAR_X0)).clamp(0.0, 1.0)
}

/// Is this x within grabbing distance of a slider bar? Phones need a fatter
/// target than a mouse, so the band is generous on both sides.
pub fn near_bar(x: f32) -> bool {
    x >= TW_BAR_X0 - 0.10 && x <= TW_BAR_X1 + 0.10
}

/// Which row is under a cursor y in a SCROLLED viewport (absolute index).
pub fn hit_row_scrolled(top: f32, row_h: f32, n: usize, vis: usize, start: usize, y: f32) -> Option<usize> {
    let i = hit_row(top, row_h, vis.min(n.saturating_sub(start)), y)?;
    let abs = start + i;
    (abs < n).then_some(abs)
}

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
// ── in-game HUD geometry, shared between drawing and its tests ──────────
//
// The hotbar band: cells sit at HOTBAR_Y..HOTBAR_Y+HOTBAR_H, the active one
// lifted by HOTBAR_LIFT, the active item's name below the cells. Everything
// stacked bottom-centre has to stay ABOVE this band, and the constant that
// enforces it is exported so a test can assert it instead of a human
// noticing it in a recording.
pub const HOTBAR_Y: f32 = -0.90;
pub const HOTBAR_H: f32 = 0.062;
pub const HOTBAR_LIFT: f32 = 0.010;
/// Top of anything the hotbar draws (a lifted active cell).
pub const HOTBAR_TOP: f32 = HOTBAR_Y + HOTBAR_H + HOTBAR_LIFT;
/// Trailing gap on the bottom-centre panel: its content must end above the
/// hotbar band with margin. safe-area floor is -0.972; -0.972 + 0.165 =
/// -0.807, which clears HOTBAR_TOP (-0.828) by ~0.02.
pub const HOTBAR_CLEARANCE: f32 = 0.165;

/// The hull bar: above the DAMAGED line (whose glyphs span 0.039..0.075), below the reticle's
/// clear zone.
pub const HULL_Y0: f32 = 0.090;
pub const HULL_H: f32 = 0.006;

/// What the hull bar draws for a torso fraction: `None` while whole (nothing is drawn), else
/// the fill and its colour -- the accent down to a third, the warning colour below it.
pub fn hull_bar(hull: f32) -> Option<(f32, [f32; 4])> {
    if !(hull < 1.0) {
        return None;
    }
    let fill = hull.clamp(0.0, 1.0);
    Some((fill, if fill < 1.0 / 3.0 { WARN } else { CYAN }))
}

// The centre weapon readout: name, cooldown bar, element, top to bottom.
// One set of numbers for the draw code and the test that proves the three
// stay disjoint (name glyphs end above the bar, element starts below it).
pub const W_NAME_Y: f32 = -0.11;
pub const W_NAME_PX: f32 = 0.0052;
pub const W_BAR_Y0: f32 = -0.155;
pub const W_BAR_H: f32 = 0.006;
pub const W_ELEM_Y: f32 = -0.175;
pub const W_ELEM_PX: f32 = 0.0045;

/// The top-left stat card as ONE stack. These eight lines used to be
/// hand-placed at y-coordinates ~2.4x their nominal size apart, a spacing
/// that only works for a text metric font.rs does not have — the real
/// glyphs are GH (7) font-pixels tall, so every line drew through the next
/// and the card was an unreadable pile in the first live recording. As a
/// panel the spacing comes from the same font metrics the rasterizer uses,
/// and a test can prove the lines disjoint.
#[allow(clippy::too_many_arguments)]
pub fn stat_card_panel(
    rings: u64,
    goals: u64,
    car_rings: u64,
    car_line: &str,
    map_name: &str,
    online: Option<(usize, &str)>,
    air_roll: bool,
) -> ui_layout::Panel {
    let mut p = ui_layout::Panel::new(ui_layout::Anchor::TopLeft);
    p.gap = 0.004; // the font's own leading (CH-GH rows) does most of it
    p = p
        .push(ui_layout::Widget::label(&format!("RINGS {rings}"), 0.008, PAPER))
        .push(ui_layout::Widget::label(&format!("GOALS {goals}"), 0.006, GOLD))
        .push(ui_layout::Widget::label(&format!("CAR-THROUGHS {car_rings}"), 0.005, DIM))
        .push(ui_layout::Widget::label(car_line, 0.0045, DIM))
        .push(ui_layout::Widget::label(map_name, 0.0045, DIM));
    if let Some((n, status)) = online {
        p = p.push(ui_layout::Widget::label(&format!("ONLINE: {n} PILOTS"), 0.005, GOLD));
        if !status.is_empty() {
            p = p.push(ui_layout::Widget::label(status, 0.0042, DIM));
        }
    }
    if air_roll {
        p = p.push(ui_layout::Widget::label("AIR STEER = ROLL", 0.005, GOLD));
    }
    p
}

/// The bottom-centre readout stack — the supersonic call-out, speed, and
/// boost — as ONE panel with a trailing gap that keeps all of it above the
/// hotbar band. It replaces four hand-placed fragments (speed text, speed
/// bar, boost panel, supersonic flash) that each landed somewhere in the
/// hotbar once rasterized at real glyph size.
///
/// `supersonic` picks the boost bar's colour; `flash` is the blink gate for
/// the call-out line. The line goes at the TOP of the stack: the panel is
/// bottom-anchored, so appearing/disappearing up there grows the box upward
/// and never moves the readouts below it.
pub fn bottom_panel(speed: f32, boost: f32, supersonic: bool, flash: bool) -> ui_layout::Panel {
    let boost_col = if supersonic {
        [1.0, 0.95, 0.75, 0.95]
    } else {
        [0.98, 0.68, 0.12, 0.92]
    };
    let mut p = ui_layout::Panel::new(ui_layout::Anchor::BottomCentre);
    p.gap = 0.006;
    if flash {
        p = p.push(ui_layout::Widget::label("SUPERSONIC", 0.005, GOLD));
    }
    p.push(ui_layout::Widget::label(&format!("{:>5.1} M/S", speed), 0.0062, PAPER))
        .push(ui_layout::Widget::bar(
            (speed / CAR_MAX).clamp(0.0, 1.0),
            0.56,
            0.008,
            [0.55, 0.85, 1.0, 0.85],
            [0.05, 0.05, 0.08, 0.50],
        ))
        .push(ui_layout::Widget::label(&format!("BOOST {boost:>3.0}"), 0.0052, INK))
        .push(ui_layout::Widget::bar(
            boost / 100.0,
            0.56,
            0.045,
            boost_col,
            [0.05, 0.05, 0.08, 0.62],
        ))
        .push(ui_layout::Widget::Gap { h: HOTBAR_CLEARANCE })
}

pub fn hud(ui: &mut Vec<UiVert>, info: &HudInfo, world: &World, car: &Car, ball: &Ball, bots: &[(Vec3, u8)]) {
    let a = info.aspect;

    // ---- bottom centre: supersonic / speed / boost, one stack ----
    //
    // A single panel because the recording showed what fragments do: the
    // speed readout, the boost label and the flash each rasterized into
    // the hotbar. The stack queues them, and the trailing gap inside
    // `bottom_panel` keeps the whole thing above the hotbar band.
    {
        let scr = ui_layout::Screen::new(a);
        let flash = info.supersonic && (info.time * 6.0).sin() > -0.4;
        let p = bottom_panel(info.speed, info.boost, info.supersonic, flash);
        draw_cmds(ui, a, &scr.emit(&p));
    }

    // flip pip: shows in the air, gold while a flip is banked. Right of the
    // bottom stack (the bars are ~0.16 NDC half-width), above the hotbar.
    if info.airborne {
        let c = if info.can_flip { GOLD } else { [0.5, 0.48, 0.45, 0.55] };
        label(ui, 0.31, -0.77, 0.0052, a, c, "FLIP");
    }

    // ---- top-left: stat card, one stack behind one glass quad ----
    {
        let scr = ui_layout::Screen::new(a);
        let car_line = format!("{}  ({})", info.car_name, info.cam_label);
        let p = stat_card_panel(
            info.rings,
            info.goals,
            info.car_rings,
            &car_line,
            info.map_name,
            info.online,
            info.air_roll,
        );
        // The backdrop is derived from the panel's own bounds, so it grows
        // with the online lines instead of a hand-maintained card_h sum.
        let b = scr.bounds(&p);
        quad(ui, b.x0 - 0.012 / a, b.y0 - 0.012, b.x1 + 0.012 / a, b.y1 + 0.010, [0.045, 0.045, 0.095, 0.45]);
        draw_cmds(ui, a, &scr.emit(&p));
    }
    // FIRST-PERSON RETICLE. From the head there is no body on screen to aim
    // relative to, so the frame needs a centre. Four ticks around a gap
    // rather than a dot: it reads at any background brightness and does not
    // sit on top of the thing you are aiming at.
    if info.first_person {
        let (t, g, w) = (0.030f32, 0.009f32, 0.0016f32);
        let ar = 0.5625; // the ticks are in NDC, so the vertical pair is scaled
        let c = [0.85, 0.90, 1.0, 0.55];
        quad(ui, -g - t, -w, -g, w, c);
        quad(ui, g, -w, g + t, w, c);
        quad(ui, -w * ar, (g + t) * ar, w * ar, g * ar, c);
        quad(ui, -w * ar, -(g + t) * ar, w * ar, -g * ar, c);
    }
    // WEAPON READOUT. Sits under the reticle rather than the stat card —
    // the stat card is about the car/robot, this is about the trigger.
    // Ready reads gold; cooling reads dim with a shrinking bar so the
    // player can eyeball the remaining wait without a number.
    if let Some(w) = info.weapon {
        let ready = w.cool_frac <= 0.0;
        let c = if ready { GOLD } else { DIM };
        label_c(ui, W_NAME_Y, W_NAME_PX, a, c, w.name);
        if !ready {
            let bw = 0.09;
            let bx = -bw * 0.5;
            quad(ui, bx, W_BAR_Y0, bx + bw, W_BAR_Y0 + W_BAR_H, [0.2, 0.2, 0.24, 0.5]);
            quad(ui, bx, W_BAR_Y0, bx + bw * (1.0 - w.cool_frac), W_BAR_Y0 + W_BAR_H, CYAN);
        }
        // ATTUNEMENT. Below the form name rather than beside it — the
        // form is what fires, the element is what it's worth against
        // whatever you're aimed at (`Element::advantage`, now live
        // against raid wardens too). Dimmed to DIM while attuning since
        // that window genuinely fights with no bonus at all — the label
        // should read as a warning, not a status. Glyphs start below the
        // cooldown bar; a test proves the three pieces disjoint.
        let ec = if w.attuning { DIM } else { CYAN };
        label_c(ui, W_ELEM_Y, W_ELEM_PX, a, ec, w.element);
    }
    // WOUNDED. Above the reticle rather than below it, clear of the
    // weapon readout — this is about the body, not the trigger. Pulses
    // rather than sitting solid so it reads as urgent without being a
    // constant distraction while it's true for a while. Glyph bottom
    // lands at 0.0386, above the reticle's top tick (0.0219).
    if info.rebuild_in > 0.0 {
        label_c(ui, 0.075, 0.0052, a, WARN, &format!("STRUCTURAL FAILURE — REBUILDING {:.1}s", info.rebuild_in));
    } else if info.wounded && (info.time * 4.0).sin() > -0.3 {
        label_c(ui, 0.075, 0.0052, a, WARN, "DAMAGED — TAKE COVER");
    }
    // HULL. The torso's own number, above the warning line, drawn only once it is less than
    // whole: a full bar is furniture, a shrinking one is information.
    if let Some((fill, colour)) = hull_bar(info.hull) {
        let bw = 0.09;
        let bx = -bw * 0.5;
        quad(ui, bx, HULL_Y0, bx + bw, HULL_Y0 + HULL_H, [0.2, 0.2, 0.24, 0.5]);
        quad(ui, bx, HULL_Y0, bx + bw * fill, HULL_Y0 + HULL_H, colour);
        label(ui, bx - font::width("HULL ", 0.0042, a), HULL_Y0 + HULL_H + 0.012, 0.0042, a, DIM, "HULL");
    }

    // ---- sparta scoreboard: BLUE n - m ORANGE across the top ----
    if let Some((b, o)) = info.scores {
        // px 0.0085: glyphs 0.0595 tall, drawn at 0.976 they end at
        // 0.9165 — inside the plate (0.906..0.985). The old 0.016 put
        // 0.112-tall glyphs through the plate's bottom and into the
        // combo banner's slot.
        let px = 0.0085;
        let bl = format!("BLUE {b}");
        let or = format!("{o} ORANGE");
        let dash = "  -  ";
        let w = font::width(&bl, px, a) + font::width(dash, px, a) + font::width(&or, px, a);
        let x0 = -w * 0.5;
        quad(ui, x0 - 0.03 / a, 0.906, x0 + w + 0.03 / a, 0.985, [0.070, 0.086, 0.110, 0.85]);
        let mut x = x0;
        label(ui, x, 0.976, px, a, [0.45, 0.65, 1.0, 1.0], &bl);
        x += font::width(&bl, px, a);
        label(ui, x, 0.976, px, a, DIM, dash);
        x += font::width(dash, px, a);
        label(ui, x, 0.976, px, a, [1.0, 0.55, 0.2, 1.0], &or);
    }

    // ---- top-right: fps, and dropped frames when there are any ----
    // The count is PRESENTED frames per second. If it sits well under the
    // refresh rate while drops are non-zero, the renderer is abandoning
    // frames rather than the GPU being slow — a distinction the old counter
    // hid completely by counting loop iterations.
    // MASTER ACCESS BANNER. Top-centre under the scoreboard band, always
    // drawn, never pulsed: this is not an alert, it is a watermark that has to
    // survive being screenshotted. Dim enough to ignore while playing, legible
    // enough to read back off a recording.
    if info.debug_access {
        label_c(ui, 0.755, 0.0058, a, [1.0, 0.55, 0.2, 0.75], "MASTER ACCESS — GATES OFF");
    }
    let fps = format!("{:>3.0} FPS", info.fps);
    label(ui, 0.985 - font::width(&fps, 0.011, a), 0.98, 0.011, a, DIM, &fps);
    if info.frame_drops > 0 {
        // The FPS glyphs (7 * 0.011) descend to 0.903; this line starts
        // below them instead of through their middle.
        let d = format!("{} DROPPED", info.frame_drops);
        label(ui, 0.985 - font::width(&d, 0.007, a), 0.888, 0.007, a, [1.0, 0.55, 0.35, 1.0], &d);
    }

    // ---- combo banner: below the scoreboard plate, bar below the text ----
    // Suppressed while a trial/lesson banner owns the same top-centre band
    // (see HudInfo::banner_active) — both can be legitimately active at
    // once, only their screen space collides.
    if info.combo >= 2 && info.combo_left > 0.0 && !info.banner_active {
        let pulse = 0.0060 + 0.0012 * (info.time * 9.0).sin().max(0.0);
        // at max pulse the glyphs descend from 0.88 to 0.8296, above the
        // window bar at 0.815..0.823
        label_c(ui, 0.88, pulse, a, GOLD, &format!("COMBO X{}", info.combo));
        let w = 0.16;
        quad(ui, -w, 0.815, -w + 2.0 * w * info.combo_left, 0.823, GOLD);
    }

    // ---- air-dribble timer: below the combo bar (0.815) ----
    if info.air_t > 0.25 {
        let s = format!("AIR {:>4.1}S", info.air_t);
        let c = if info.air_t > info.best_air && info.best_air > 0.5 { GOLD } else { PAPER };
        label_c(ui, 0.80, 0.007, a, c, &s);
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
/// The pause panel's half-height. Named so the test can read the SAME number
/// the draw uses: hardcoding 0.90 in the test meant shrinking the panel under
/// its own content passed cleanly, since both sides moved independently.
pub const PAUSE_PANEL_Y: f32 = 0.90;
/// The hairline `panel` strokes outside its own bounds.
pub const PANEL_STROKE: f32 = 0.0035;

pub fn pause(ui: &mut Vec<UiVert>, a: f32, save: &Save, best_air_now: f32, cur: (f32, f32), sel: usize) {
    quad(ui, -1.0, -1.0, 1.0, 1.0, [0.05, 0.04, 0.09, 0.45]);
    panel(ui, a, -0.78, -PAUSE_PANEL_Y, 0.78, PAUSE_PANEL_Y);
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

    // tappable buttons: the whole pause menu works with a finger — and now
    // with arrows too (`sel` is the keyboard cursor; hover and selection
    // share the highlight so the two input styles read as one).
    for (i, name) in PAUSE_BTNS.iter().enumerate() {
        let x0 = pbtn_x0(i);
        let (y0, y1) = (PBTN_Y - PBTN_H * 0.5, PBTN_Y + PBTN_H * 0.5);
        let hot = hit_pause_btn(cur.0, cur.1) == Some(i) || i == sel;
        let bg = if hot { [0.24, 0.30, 0.38, 0.95 * a] } else { [0.13, 0.15, 0.20, 0.85 * a] };
        quad(ui, x0, y0, x0 + PBTN_W, y1, bg);
        quad(ui, x0, y1 - 0.006, x0 + PBTN_W, y1, [GOLD[0], GOLD[1], GOLD[2], (if hot { 0.9 } else { 0.35 }) * a]);
        // the keyboard cursor gets a second rail on the bottom edge, so the
        // selected button is findable even while the mouse hovers another
        if i == sel {
            quad(ui, x0, y0, x0 + PBTN_W, y0 + 0.006, [CYAN[0], CYAN[1], CYAN[2], 0.9 * a]);
        }
        let k = ui_k(a);
        let txt = fit(name, PBTN_W * 0.86, 0.0105 * k, a);
        label_c_at(ui, x0 + PBTN_W * 0.5, PBTN_Y - 0.011 * k, 0.0105 * k, a, if hot { PAPER } else { DIM }, &txt);
    }
    legend(ui, -0.80, a, 0.0098, &[("ARROWS", "SELECT"), ("ENTER", "ACTIVATE"), ("ESC", "RESUME"), ("TAP", "ANY BUTTON")]);
}

// ------------------------------------------------------------- controls ----
pub const BIND_ROWS: usize = ACTIONS.len() + 4; // + default, classic, dvorak, back

pub fn bindings_menu(ui: &mut Vec<UiVert>, a: f32, b: &Bindings, sel: usize, capturing: bool, scroll: usize, cur: (f32, f32)) {
    quad(ui, -1.0, -1.0, 1.0, 1.0, [0.05, 0.04, 0.09, 0.45]);
    panel(ui, a, -0.80, -0.985, 0.80, 0.955);
    label_c(ui, 0.885, T_TITLE, a, PAPER, "CONTROLS");

    let px = T_BODY;
    let row_h = BIND_ROW_H;
    let top = BIND_TOP;
    let lx = -0.70;
    let rx = 0.16;
    let vis = vis_rows(row_h, top, TW_BOTTOM);
    let start = scroll.min(BIND_ROWS.saturating_sub(vis.min(BIND_ROWS)));
    let end = (start + vis).min(BIND_ROWS);
    scrollbar(ui, a, 0.762, TW_BOTTOM, top + 0.02, BIND_ROWS, vis, start);
    for (i, act) in ACTIONS.iter().enumerate() {
        if i < start || i >= end {
            continue;
        }
        let y = top - (i - start) as f32 * row_h;
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
    let extras = ["PRESET: DEFAULT (.EOU + TRIGGERS)", "PRESET: CLASSIC (WASD)", "PRESET: DVORAK (.OEU HOME ROW)", "BACK"];
    for (k, lab) in extras.iter().enumerate() {
        let i = ACTIONS.len() + k;
        if i < start || i >= end {
            continue;
        }
        let y = top - (i - start) as f32 * row_h;
        if i == sel {
            quad(ui, -0.72, y - row_h + 0.016, 0.72, y + 0.016, SEL);
            quad(ui, -0.735, y - row_h + 0.016, -0.72, y + 0.016, GOLD);
        }
        font::text(ui, lx, y, px, a, if i == sel { GOLD } else { PAPER }, lab);
    }

    nav_strip(ui, a, cur, 2);
    if capturing {
        legend(ui, -0.93, a, T_HINT, &[("ANY INPUT", "BIND IT"), ("DEL", "CLEAR"), ("ESC", "CANCEL")]);
    } else {
        legend(ui, -0.93, a, T_HINT, &[("CLICK / ENTER", "REBIND"), ("DEL", "CLEAR"), ("ESC", "BACK")]);
    }
}

// ----------------------------------------------------------------- maps ----
pub fn maps_menu(ui: &mut Vec<UiVert>, a: f32, sel: usize, current: usize, seed: u64, scroll: usize, cur: (f32, f32)) {
    quad(ui, -1.0, -1.0, 1.0, 1.0, [0.05, 0.04, 0.09, 0.45]);
    panel(ui, a, -0.62, -0.985, 0.62, 0.955);
    label_c(ui, 0.885, T_TITLE, a, PAPER, "MAPS");
    label_c(ui, 0.828, T_HINT, a, DIM, "ROOM STYLES FROM THE LOGSCOPE SCENE, PLUS NET-GOAL ARENAS");

    let row_h = maps_row_h();
    let px = T_BODY;
    let top = MAPS_TOP;
    let n = crate::world::STYLE_COUNT;
    let vis = vis_rows(row_h, top, TW_BOTTOM);
    let start = scroll.min(n.saturating_sub(vis.min(n)));
    let end = (start + vis).min(n);
    scrollbar(ui, a, 0.575, TW_BOTTOM, top + 0.02, n, vis, start);
    for i in start..end {
        let st = crate::world::style(i);
        let y = top - (i - start) as f32 * row_h;
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
    nav_strip(ui, a, cur, 3);
    legend(ui, -0.93, a, T_HINT, &[("CLICK / ENTER", "LOAD"), ("R", "NEW SEED"), ("ESC", "BACK")]);
}

// --------------------------------------------------------------- online ----
// OnlineUi retired: the Online screen is a menu::Menu now (online_screen.rs)
// drawn via draw_menu + online_footer.


/// The Online screen's live footer — status line + pilots-in-session list —
/// drawn UNDER the shared `draw_menu` output (the rows themselves are a
/// `menu::Menu` now; see main's Mode::Online arm and `online_screen.rs`).
pub fn online_footer(ui: &mut Vec<UiVert>, a: f32, status: &str, players: &[String], online: bool) {
    label_c(ui, 0.825, 0.0088, a, DIM, "SESSIONS RIDE THE FLEET NETCODE STORE - SAME MACHINE OR A SHARED TEMP DIR");
    if !status.is_empty() {
        label_c(ui, -0.16, 0.0105, a, GOLD, status);
    }
    let mut y = -0.26;
    if online {
        label_c(ui, y, 0.0105, a, PAPER, "PILOTS IN SESSION:");
        y -= 0.048;
        for p in players.iter().take(8) {
            label_c(ui, y, 0.0095, a, DIM, p);
            y -= 0.042;
        }
    }
    label_c(ui, -0.86, 0.0085, a, DIM, "HOSTING SHOWS YOUR ADDRESS - A FRIEND ON THE LAN TYPES IT UNDER FRIEND AND JOINS");
    legend(ui, -0.93, a, T_HINT, &[("TYPE", "EDIT FIELD"), ("CLICK / ENTER", "ACTIVATE"), ("ESC", "BACK")]);
}

// --------------------------------------------------------------- tweaks ----
pub struct TweakRow {
    pub label: String,
    pub value: String,
    pub bar: Option<f32>, // 0..1 gauge for sliders
    pub header: bool,     // section caption: not selectable, drawn as a rule
    pub action: bool,     // an "activates something" row: gets a chevron
}

pub fn tweaks_menu(ui: &mut Vec<UiVert>, a: f32, title: &str, rows: &[TweakRow], sel: usize, scroll: usize, cur: (f32, f32), here: usize) {
    quad(ui, -1.0, -1.0, 1.0, 1.0, [0.05, 0.04, 0.09, 0.45]);
    panel(ui, a, -0.80, -0.985, 0.80, 0.955);
    label_c(ui, 0.885, T_TITLE, a, PAPER, title);

    let row_h = TW_ROW_H;
    let top = TW_TOP;
    let vis = vis_rows(row_h, top, TW_BOTTOM);
    let start = scroll.min(rows.len().saturating_sub(vis.min(rows.len())));
    let end = (start + vis).min(rows.len());
    let px = T_BODY;

    // section cards, clipped to the viewport: a header owns rows until the
    // next header, and the card is what makes groups scannable
    let mut i0 = start;
    while i0 < end {
        if rows[i0].header {
            let mut i1 = i0 + 1;
            while i1 < end && !rows[i1].header {
                i1 += 1;
            }
            if i1 > i0 + 1 {
                let y_top = top - (i0 - start) as f32 * row_h - row_h * 0.40;
                let y_bot = top - (i1 - 1 - start) as f32 * row_h - row_h + 0.016;
                inset(ui, a, -0.76, y_bot, 0.76, y_top);
            }
            i0 = i1;
        } else {
            i0 += 1;
        }
    }

    for i in start..end {
        let r = &rows[i];
        let y = top - (i - start) as f32 * row_h;
        if r.header {
            // section caption sitting on the card's top edge
            label(ui, -0.735, y, T_SECTION, a, CYAN, &r.label);
            continue;
        }
        let selected = i == sel;
        if selected {
            quad(ui, -0.74, y - row_h + 0.026, 0.74, y + 0.022, SEL);
            quad(ui, -0.757, y - row_h + 0.026, -0.742, y + 0.022, GOLD);
        }
        // ALIGNMENT GRID: labels share one left edge, values share one right
        // edge, and the action caret lives outside the label column so text
        // never shifts as you move the selection
        if r.action {
            font::text(ui, -0.715, y, px, a, GOLD, ">");
        }
        let k = ui_k(a);
        let px = px * k;
        // the label column stops where the value column starts, so a long
        // setting name is truncated rather than running under its own value
        let lab = fit(&r.label, 1.02, px, a);
        font::text(ui, -0.68, y, px, a, if selected { GOLD } else { PAPER }, &lab);
        let val = fit(&r.value, 0.42, px, a);
        let vw = font::width(&val, px, a);
        let vcol = if selected { CYAN } else { [1.0, 0.98, 0.94, 0.45] };
        font::text(ui, 0.70 - vw, y, px, a, vcol, &val);
        // the bar draws under the SELECTED row only: one moving part at a
        // time is the whole de-cluttering idea
        if selected {
            if let Some(t) = r.bar {
                let by = y - row_h * 0.46;
                quad(ui, TW_BAR_X0, by, TW_BAR_X1, by + 0.009, [0.22, 0.27, 0.32, 0.6]);
                quad(ui, TW_BAR_X0, by, TW_BAR_X0 + (TW_BAR_X1 - TW_BAR_X0) * t.clamp(0.0, 1.0), by + 0.009, GOLD);
            }
        }
    }
    scrollbar(ui, a, 0.762, TW_BOTTOM, top + 0.02, rows.len(), vis, start);
    if rows.len() > vis {
        label_c(ui, TW_BOTTOM - 0.035, T_HINT, a, DIM, &format!("{}-{} OF {}", start + 1, end, rows.len()));
    }
    nav_strip(ui, a, cur, here);
    legend(ui, -0.93, a, T_HINT, &[
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
    label_c(ui, -0.44, 0.0085, a, DIM, "V1.101    ESC QUITS FROM THIS SCREEN");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whole: nothing drawn. Below whole: the fraction, in the accent until a third, then the
    /// warning colour; clamped so a stale negative never draws backwards.
    #[test]
    fn the_hull_bar_appears_when_hurt_and_turns_to_warning_at_a_third() {
        assert_eq!(hull_bar(1.0), None);
        assert_eq!(hull_bar(1.5), None);
        assert_eq!(hull_bar(0.9), Some((0.9, CYAN)));
        assert_eq!(hull_bar(1.0 / 3.0), Some((1.0 / 3.0, CYAN)));
        assert_eq!(hull_bar(0.2), Some((0.2, WARN)));
        assert_eq!(hull_bar(-0.5), Some((0.0, WARN)));
        assert_eq!(hull_bar(f32::NAN), None, "NaN is not less than whole; nothing is drawn");
    }

    /// The hull bar sits above the DAMAGED line's glyphs (0.039..0.075) and below the clear
    /// band under the top plate -- the same disjointness the weapon readout is held to.
    #[test]
    fn the_hull_bar_stays_clear_of_the_damaged_line() {
        let a = 16.0 / 9.0;
        let damaged = raster(-0.2, 0.075, 0.0052, a, "DAMAGED — TAKE COVER");
        assert!(HULL_Y0 > damaged.y1, "bar bottom {HULL_Y0} must clear the warning's top {}", damaged.y1);
        assert!(HULL_Y0 + HULL_H + 0.012 + 0.0042 * 10.0 < 0.5, "and stay out of the middle of the screen");
    }

    /// Rasterize one label the way the renderer will (shadow pass included,
    /// since `label` draws it offset and it extends the ink's box) and
    /// return the bounding box of every vertex.
    fn raster(x: f32, y: f32, px: f32, a: f32, s: &str) -> ui_layout::Rect {
        let mut v: Vec<UiVert> = Vec::new();
        label(&mut v, x, y, px, a, [1.0; 4], s);
        let mut r = ui_layout::Rect { x0: f32::MAX, y0: f32::MAX, x1: f32::MIN, y1: f32::MIN };
        for p in &v {
            r.x0 = r.x0.min(p.pos[0]);
            r.y0 = r.y0.min(p.pos[1]);
            r.x1 = r.x1.max(p.pos[0]);
            r.y1 = r.y1.max(p.pos[1]);
        }
        r
    }

    /// Boxes for every Cmd a panel emits, text rasterized for real.
    fn cmd_boxes(cmds: &[ui_layout::Cmd], a: f32) -> Vec<ui_layout::Rect> {
        cmds.iter()
            .map(|c| match c {
                ui_layout::Cmd::Quad { r, .. } => *r,
                ui_layout::Cmd::Text { x, y, size, text, .. } => raster(*x, *y, *size, a, text),
            })
            .collect()
    }

    /// A watermark that only draws sometimes, and must not collide when it
    /// does. Both halves matter: absent on a normal save (or it is noise), and
    /// present AND legible on a debug one (or a tester's recording cannot be
    /// told apart from a player's).
    /// THE SCREEN A TESTER IS BEING SENT TO.
    ///
    /// `6d9475d2` fixed the in-game HUD's text metric and explicitly left the
    /// hand-placed menus — pause, garage, tweaks, academy, maps — sharing the
    /// bug class. Then this session told a playtester to open SETTINGS →
    /// DEBUG → MASTER ACCESS, which is one of those screens.
    ///
    /// I WROTE THIS THREE TIMES BADLY BEFORE WRITING IT SIMPLY, and the
    /// failures are worth recording because they are all one mistake. The
    /// first bucketed rasterized vertices into bands at
    /// `LIST_TOP - i * TW_ROW_H` — deriving the bands from the constant under
    /// test, so they shrank with it and the test passed against both a halved
    /// row height and a 50% larger font. The second clustered the ink by gap
    /// instead, which is pitch-independent but counts the section card and
    /// cursor quads too, so `clusters >= rows` was satisfied by furniture. The
    /// third would have been more of the same.
    ///
    /// The property is one inequality: a row of text must fit inside a row of
    /// space. Rasterizing was ceremony around a comparison, and the ceremony
    /// is what kept measuring the wrong thing.
    #[test]
    fn a_settings_row_of_text_fits_in_a_settings_row_of_space() {
        // EVERY LIST SCREEN, not just the one a tester was sent to. Written as
        // a table because checking them one at a time is how two got missed:
        // CONTROLS and TRIALS were at 0.058 against a 0.0574 glyph — 1%
        // clear, about a third of a pixel at 1080p. Not overlapping, but with
        // no visible gap either, which reads as a block of text rather than a
        // list. Both raised to 0.062, the pitch the other screens already use.
        //
        // `ui_k` clamps at 1.0, so T_BODY is the largest the text ever gets
        // and these margins are the worst case rather than a typical one.
        let glyph = font::GH * T_BODY;
        for (name, _, row_h) in PITCHES {
            let row_h = *row_h;
            assert!(
                glyph < row_h,
                "{name}: text is {glyph:.4} tall in a {row_h:.4} row — rows overlap"
            );
            // and with enough margin to read as separate rows rather than a
            // block. Below this it is cramped even when it technically fits,
            // which is the state the in-game HUD was in before anyone measured
            // it.
            assert!(
                row_h - glyph > row_h * 0.05,
                "{name}: rows clear by only {:.4} ({:.0}%) — legible but cramped",
                row_h - glyph,
                (row_h - glyph) / row_h * 100.0
            );
        }
        // the title has its own size and its own row: it must not reach the
        // first row's band either
        let title_glyph = font::GH * T_TITLE;
        assert!(
            0.885 - title_glyph > LIST_TOP,
            "the title descends into the first row (title bottom {:.3}, list top {LIST_TOP:.3})",
            0.885 - title_glyph
        );
    }

    /// THE NON-LIST SCREENS, where the inequality does not reach.
    ///
    /// `pause` places ten labels at hand-chosen coordinates inside a panel at
    /// ±0.78 by ±0.90. There is no row pitch to compare a glyph against, so
    /// the previous commit's table says nothing about it — this is the case
    /// the rasteriser was built for, and the property it checks is the one the
    /// original HUD bug violated: text that claims a box and then draws
    /// outside it.
    ///
    /// Escape is the first key a playtester presses.
    /// THE TABLE HAS TO KNOW ABOUT EVERY ROW PITCH, or it is a table of the
    /// ones somebody remembered.
    ///
    /// `maps_row_h()` sat at 0.058 through the sweep that fixed CONTROLS and
    /// TRIALS, because it is a function returning a literal and the sweep
    /// walked `pub const ... ROW_H` declarations. Same value, same bug, a
    /// different shape. So this scans for the shape instead of trusting the
    /// list, and fails when a pitch exists that the table above does not
    /// name.
    #[test]
    fn no_row_pitch_escapes_the_clearance_table() {
        let src = include_str!("hud.rs");
        // needle assembled at runtime: a literal `ROW_H` in this test would
        // match itself, which cost two attempts when the same scan pattern
        // was added for Model::settle
        let needle = format!("{}_H: f32 = 0.0", "ROW");
        let mut found: Vec<String> = Vec::new();
        for line in src.lines() {
            if let Some(i) = line.find(&needle) {
                let name: String = line[..i + 3]
                    .rsplit(' ')
                    .next()
                    .unwrap_or("")
                    .trim_end_matches(':')
                    .to_string();
                found.push(name);
            }
        }
        assert!(found.len() >= 5, "the scan found only {found:?} — it has stopped matching");

        // AND THE DRAW MUST USE THEM. `catalog_menu` repeated `0.052` and
        // `0.046` as literals while the constants sat beside it claiming to be
        // "the same numbers the drawing uses" — they agreed by coincidence,
        // and that duplication is why a row pitch smaller than its own glyph
        // survived two sweeps. A constant nothing reads is a comment.
        let body = &src[src.find("pub fn catalog_menu").unwrap_or(0)..];
        let body = &body[..body.find("\n}\n").unwrap_or(body.len())];
        assert!(
            body.contains("CAT_ROW_H"),
            "catalog_menu no longer uses CAT_ROW_H — it is back to a literal"
        );
        for stale in [format!("{{ 0.0{}2 }}", 5), format!("{{ 0.0{}6 }}", 4)] {
            assert!(!body.contains(&stale), "catalog_menu has a literal row pitch: {stale}");
        }
        // THE SAME TABLE the clearance test iterates. It was two lists for one
        // commit — an allowlist here and a checked table there — and
        // GARAGE_ROW_H immediately ended up in one and not the other:
        // allowlisted, so the scan passed it, and unchecked, so nothing
        // measured it. Two lists of the same thing is the bug this whole
        // sweep has been about, and I reproduced it while fixing it.
        for f in &found {
            assert!(
                PITCHES.iter().any(|(_, t, _)| f.contains(t) || t.contains(f.as_str())),
                "{f} is a row pitch the clearance table does not check"
            );
        }
    }

    #[test]
    fn the_pause_screen_keeps_its_text_inside_its_panel() {
        let save = Save::default();
        // widest and narrowest, because `ui_k` scales some of this text with
        // aspect and a panel that fits at 16:9 can spill at 4:3
        for a in [21.0 / 9.0, 16.0 / 9.0, 4.0 / 3.0] {
            let mut v: Vec<UiVert> = Vec::new();
            pause(&mut v, a, &save, 1.25, (0.0, 0.0), 0);
            assert!(!v.is_empty(), "pause drew nothing at {a:.2}");

            // The full-screen scrim is deliberately ±1.0, so measure the TEXT
            // and furniture inside the panel by excluding it: anything at
            // exactly the scrim corners is the scrim.
            let inked: Vec<&UiVert> = v
                .iter()
                .filter(|p| p.pos[0].abs() < 0.999 && p.pos[1].abs() < 0.999)
                .collect();
            assert!(!inked.is_empty(), "only the scrim drew at {a:.2}");

            let mut worst_y: f32 = 0.0;
            for p in &inked {
                worst_y = worst_y.max(p.pos[1].abs());
            }
            // `panel` strokes a 0.0035 hairline OUTSIDE its own bounds (see
            // its four border quads), so the frame legitimately reaches
            // 0.9035. My first version asserted a flat 0.90 and failed on the
            // panel's own border — measuring the furniture, not the content,
            // which is the same mistake the three settings attempts made.
            //
            // WHAT THIS COSTS, stated rather than hidden: admitting the stroke
            // means text spilling by less than 3.5e-3 passes. It still catches
            // the class that matters — the in-game HUD bug was text drawing
            // THREE TIMES its claimed height — but it is not a tight bound and
            // should not be read as one.
            assert!(
                worst_y <= PAUSE_PANEL_Y + PANEL_STROKE + 1e-4,
                "pause draws to y={worst_y:.4} at aspect {a:.2}, past its ±{PAUSE_PANEL_Y} panel"
            );
        }
    }

    #[test]
    fn the_master_access_watermark_appears_only_when_it_should() {
        let a = 16.0 / 9.0;
        let draw = |on: bool| -> Vec<UiVert> {
            let mut v = Vec::new();
            if on {
                label_c(&mut v, 0.755, 0.0058, a, [1.0, 0.55, 0.2, 0.75], "MASTER ACCESS — GATES OFF");
            }
            v
        };
        assert!(draw(false).is_empty(), "a normal save drew the watermark");
        let on = draw(true);
        assert!(!on.is_empty(), "master access drew nothing");

        // it sits clear of the scoreboard plate (0.906..0.985) and of the
        // combo bar (0.815..0.823), which is the band it was slotted between
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for p in &on {
            lo = lo.min(p.pos[1]);
            hi = hi.max(p.pos[1]);
        }
        assert!(hi < 0.815, "the watermark overlaps the combo bar (top {hi:.3})");
        assert!(lo > 0.70, "the watermark drifted into the play area (bottom {lo:.3})");
        // and it is legible rather than a smear: real glyph height
        assert!(hi - lo > 0.02, "the watermark is too small to read back: {:.3}", hi - lo);

        // AND THE CALL SITE. The block above re-draws the label itself, so it
        // would pass even if `hud` never drew one — the same
        // correct-function-nobody-calls trap this session has hit five times.
        // Needles assembled at runtime so the scan cannot match its own source.
        let src = include_str!("hud.rs");
        let guard = format!("if info.{} {{", "debug_access");
        assert!(src.contains(&guard), "hud() no longer checks debug_access");
        let text = format!("MASTER ACCESS {} GATES OFF", "—");
        assert_eq!(
            src.matches(&text).count(),
            2,
            "expected the banner string once in hud() and once in this test"
        );

        // and main.rs must feed the flag rather than a constant. Pinning it to
        // `true` would watermark every player's screen; pinning it to `false`
        // would watermark nobody's, and neither shows up in a HUD test.
        let m = include_str!("main.rs");
        let wired = format!("debug_access: self.cfg.{}.debug_ship,", "story");
        assert!(m.contains(&wired), "main.rs no longer feeds debug_access from the config");
        for bad in ["debug_access: true,", "debug_access: false,"] {
            assert!(!m.contains(bad), "debug_access is hardcoded: {bad}");
        }
    }

    /// The trial/lesson banner and the combo readout both legitimately want
    /// the top-centre band at once (a Do-a-trick lesson reads `self.combo`
    /// straight into its own progress bar), so this isn't "can they both be
    /// active" — they can — it's "do they draw on top of each other." They
    /// did: `trial_banner` occupies roughly y∈[0.66,0.895], x∈[-0.34,0.34],
    /// and the combo label/bar sit at y=0.88 / y∈[0.815,0.823], centred at
    /// x=0 — squarely inside it. `HudInfo::banner_active` is the fix: same
    /// two-part shape as the watermark test above, a local redraw plus a
    /// call-site scan, because a flag that exists but nothing reads or sets
    /// is worse than no flag — it looks fixed.
    #[test]
    fn the_combo_readout_is_suppressed_under_a_trial_or_lesson_banner() {
        let a = 16.0 / 9.0;
        let draw = |banner_active: bool| -> Vec<UiVert> {
            let mut v = Vec::new();
            if 3 >= 2 && 1.0_f32 > 0.0 && !banner_active {
                let pulse = 0.0060;
                label_c(&mut v, 0.88, pulse, a, GOLD, &format!("COMBO X{}", 3));
            }
            v
        };
        assert!(!draw(false).is_empty(), "a bare combo (no banner) drew nothing");
        assert!(draw(true).is_empty(), "the combo readout drew through a trial/lesson banner");

        // AND THE CALL SITE — the redraw above would pass even if `hud()`
        // never checked the flag at all.
        let src = include_str!("hud.rs");
        assert!(
            src.contains("info.combo >= 2 && info.combo_left > 0.0 && !info.banner_active"),
            "hud()'s combo block no longer gates on banner_active"
        );
        let m = include_str!("main.rs");
        assert!(
            m.contains("banner_active: self.lesson.is_some() || self.trial.is_some(),"),
            "main.rs no longer feeds banner_active from lesson/trial state"
        );
    }

    #[test]
    fn the_stat_card_lines_rasterize_disjoint_and_inside_their_backdrop() {
        // The first live recording, top-left: eight lines drawn through
        // each other. The card is a panel now, so this asserts the panel's
        // OUTPUT — real glyph boxes — not its self-reported metrics.
        let a = 16.0 / 9.0;
        let scr = ui_layout::Screen::new(a);
        let p = stat_card_panel(
            12,
            3,
            4,
            "VORPAL BLADE PALOMINO  (CHASE CAM)",
            "HALO ZEO",
            Some((1, "A CONTAINER SHIP CROSSES THE BAY, RUNNING DARK")),
            true,
        );
        let b = scr.bounds(&p);
        let boxes = cmd_boxes(&scr.emit(&p), a);
        assert!(boxes.len() >= 8, "the fully-populated card should draw 8 lines, drew {}", boxes.len());
        for (i, r) in boxes.iter().enumerate() {
            assert!(b.contains_rect(r), "line {i} {r:?} escapes the card bounds {b:?}");
            for (j, q) in boxes.iter().enumerate().skip(i + 1) {
                assert!(!r.overlaps(q), "card line {i} {r:?} overlaps line {j} {q:?}");
            }
        }
    }

    #[test]
    fn the_bottom_stack_stays_above_the_hotbar_band() {
        // The recording's bottom centre: BOOST 100 and the speed readout
        // across the hotbar cells. Everything the panel emits — including
        // the supersonic flash, its tallest configuration — must end above
        // the hotbar's lifted-cell top.
        let a = 16.0 / 9.0;
        let scr = ui_layout::Screen::new(a);
        let p = bottom_panel(43.2, 100.0, true, true);
        let boxes = cmd_boxes(&scr.emit(&p), a);
        assert!(boxes.len() >= 4, "speed text+bar and boost text+bar at minimum, drew {}", boxes.len());
        for r in &boxes {
            assert!(
                r.y0 >= HOTBAR_TOP,
                "bottom-stack box {r:?} reaches into the hotbar band (top {HOTBAR_TOP})"
            );
        }
        // and the two stacked panels of the in-game HUD never collide
        let card = stat_card_panel(0, 0, 0, "X (CHASE CAM)", "MAP", Some((1, "S")), true);
        let (_, clashes) = scr.emit_all(&[card, bottom_panel(0.0, 0.0, false, false)]);
        assert!(clashes.is_empty(), "hud panels collide: {clashes:?}");
    }

    #[test]
    fn the_weapon_readout_pieces_are_disjoint_top_to_bottom() {
        // Name glyphs end above the cooldown bar; the element line starts
        // below it. The recording showed the name and element drawn through
        // each other ("PHOTON" over "ATTUNING").
        let a = 16.0 / 9.0;
        let name = raster(-0.2, W_NAME_Y, W_NAME_PX, a, "PHOTON");
        let elem = raster(-0.2, W_ELEM_Y, W_ELEM_PX, a, "ATTUNING");
        let bar_top = W_BAR_Y0 + W_BAR_H;
        assert!(name.y0 > bar_top, "name glyphs ({}) reach the cooldown bar ({bar_top})", name.y0);
        assert!(elem.y1 < W_BAR_Y0, "element glyphs ({}) reach the cooldown bar ({W_BAR_Y0})", elem.y1);
        assert!(!name.overlaps(&elem), "name {name:?} overlaps element {elem:?}");
    }

    #[test]
    fn hotbar_ink_stays_inside_the_hotbar_band() {
        // Slot numbers used to start near the cell bottom and hang 0.03
        // below the bar — the recording shows every digit under its box.
        // With no active item there is no name line, so every vertex the
        // hotbar emits (cells, outline, digits) must lie inside the band.
        let hb = crate::hotbar::Hotbar::from_line("");
        let mut v: Vec<UiVert> = Vec::new();
        draw_hotbar(&mut v, 16.0 / 9.0, &hb);
        assert!(!v.is_empty(), "an empty hotbar still draws its nine slots");
        for p in &v {
            assert!(
                p.pos[1] >= HOTBAR_Y - 1e-5 && p.pos[1] <= HOTBAR_TOP + 1e-5,
                "hotbar vertex at y {} is outside the band [{HOTBAR_Y}, {HOTBAR_TOP}]",
                p.pos[1]
            );
        }
    }
}
