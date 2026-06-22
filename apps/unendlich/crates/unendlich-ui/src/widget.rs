//! widget.rs — the layered "sandwich" for toolkit buttons (r315).
//!
//! The operator asked for buttons abstracted so toolkit styling sits cleanly over
//! egui, as layers:
//!
//!   egui base → unify → button work → abstract to toolkit → toolkit language
//!
//!   • LAYER 1  egui base   — raw egui primitives (allocate / Sense / Painter). Only
//!                            touched inside layer 2 and the paint pass.
//!   • LAYER 2  unify       — [`interact`] turns an egui allocation into a normalized
//!                            [`Interaction`] (rect, response, animated hover, pressed).
//!                            Reusable by ANY clickable widget, not just buttons.
//!   • LAYER 3  button work — [`geometry`] (sizing/padding/min) + label casing. The
//!                            button-specific mechanics, independent of how it looks.
//!   • LAYER 4  abstract    — [`ButtonSkin`]: a *declarative* description of a look in
//!                            toolkit terms (fill / left-bar / dither / border / ink /
//!                            radius). No egui drawing here — just the design intent.
//!   • LAYER 5  toolkit lang— [`candidate`] (the live style candidates expressed as
//!                            skins) + [`paint`] (renders any skin via toolkit
//!                            primitives). [`button`] / [`button_sized`] compose 2→5.
//!
//! Adding or tweaking a look = editing a declarative [`ButtonSkin`] in [`candidate`];
//! no egui plumbing. The current candidates riff on `dest_button`'s minimalist feel
//! (dark panel, growing left accent bar, uppercase letter-spaced label) and push a
//! dithered-pixel character in ten different ways.

use crate::{current_theme, menu, r, Theme};
use egui::{Color32, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

// ── live style index (which candidate the debug slider has selected) ──────────
static STYLE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);
pub fn set_button_style(v: u8) {
    STYLE.store(v, std::sync::atomic::Ordering::Relaxed);
}
pub fn button_style() -> u8 {
    STYLE.load(std::sync::atomic::Ordering::Relaxed)
}

// ── LAYER 2 — unify: one normalized interaction for any clickable box ─────────
pub struct Interaction {
    pub rect: Rect,
    pub resp: Response,
    pub hover: f32, // animated 0..1
    pub down: bool,
}
pub fn interact(ui: &mut Ui, size: Vec2) -> Interaction {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = ui.ctx().animate_bool(resp.id, resp.hovered());
    let down = resp.is_pointer_button_down_on();
    Interaction { rect, resp, hover, down }
}

// ── LAYER 3 — button work: sizing + label casing ─────────────────────────────
const FONT_PX: f32 = 13.0;

/// The minimalist label treatment shared by every candidate: uppercase, thin-space
/// letter-spaced (mirrors `dest_button`).
fn label_spaced(label: &str) -> String {
    let s: String = label
        .to_uppercase()
        .chars()
        .flat_map(|c| [c, '\u{2009}'])
        .collect();
    s.trim_end().to_owned()
}

fn geometry(ui: &Ui, label: &str, min: Vec2) -> Vec2 {
    let galley = ui.painter().layout_no_wrap(
        label_spaced(label),
        egui::FontId::proportional(FONT_PX),
        Color32::WHITE,
    );
    let want = egui::vec2(galley.size().x + 34.0, galley.size().y + 14.0);
    want.max(min)
}

// ── LAYER 4 — abstract to toolkit: the declarative skin ──────────────────────
#[derive(Clone, Copy)]
pub struct ButtonSkin {
    pub radius: u8,
    pub fill: Fill,
    /// Left accent bar base width in px (0 = none). Grows with hover (dest_button).
    pub left_bar: f32,
    pub border: Border,
    pub dither: Dither,
    pub ink: Ink,
    /// Center-align the label (pulse skins) vs left-align (bar / quiet skins).
    pub center: bool,
    /// Hover "bump": a right-nudge when left-aligned, a slight size grow when centered.
    pub bump: bool,
}
#[derive(Clone, Copy)]
pub enum Fill {
    None,
    /// Panel tint at the given base alpha (brightens slightly on hover).
    Panel(f32),
    /// Accent tint at the given base alpha.
    Accent(f32),
}
#[derive(Clone, Copy)]
pub enum Border {
    None,
    Hairline,
    Accent,
}
#[derive(Clone, Copy)]
pub enum Dither {
    Off,
    /// Dot wash across the whole body, always on.
    Always,
    /// Dots reveal left→right with hover (the mockup's reveal).
    HoverReveal,
    /// Animated: a center band of dither that random-walks its width and breathes.
    Pulse,
    /// A dithered underline strip.
    Underline,
    /// Dithered top+bottom edge strips.
    Border,
    /// Dither dots clustered at the four corners.
    Corners,
    /// "Elegant": faint static edges + a soft dither infill that FOLLOWS THE
    /// CURSOR and eases in with hover. The magical / enchantment button.
    Elegant,
}
#[derive(Clone, Copy)]
pub enum Ink {
    Light,
    Accent,
}

// ── LAYER 5 — toolkit language: the candidates + the paint pass ──────────────

/// The live style candidates (0..=9), each a declarative [`ButtonSkin`]. All keep the
/// minimalist bar + spaced-label feel and vary the dithered-pixel character.
pub fn candidate(i: u8, _th: &Theme, _accent: Color32) -> ButtonSkin {
    match i {
        // 0 — header bar (the dest_button header class)
        0 => ButtonSkin { radius: 3, fill: Fill::Panel(0.50), left_bar: 3.0, border: Border::None, dither: Dither::Off, ink: Ink::Light, center: false, bump: true },
        // 1 — SPECIAL: bar + hover dither reveal (the DEFAULT; main-menu button)
        1 => ButtonSkin { radius: 3, fill: Fill::Panel(0.40), left_bar: 3.0, border: Border::None, dither: Dither::HoverReveal, ink: Ink::Light, center: false, bump: true },
        // 2 — pulse fill
        2 => ButtonSkin { radius: 4, fill: Fill::Panel(0.30), left_bar: 0.0, border: Border::None, dither: Dither::Pulse, ink: Ink::Light, center: true, bump: true },
        // 3 — pulse ghost (accent ink)
        3 => ButtonSkin { radius: 4, fill: Fill::None, left_bar: 0.0, border: Border::None, dither: Dither::Pulse, ink: Ink::Accent, center: true, bump: true },
        // 4 — pulse framed
        4 => ButtonSkin { radius: 3, fill: Fill::Panel(0.22), left_bar: 0.0, border: Border::Hairline, dither: Dither::Pulse, ink: Ink::Light, center: true, bump: true },
        // 6 — ELEGANT: faint edges + cursor-tied smooth dither infill (enchantments)
        6 => ButtonSkin { radius: 3, fill: Fill::None, left_bar: 0.0, border: Border::Hairline, dither: Dither::Elegant, ink: Ink::Light, center: true, bump: false },
        // 5, 7, 8, 9 deleted per feedback -> fall back to the default (special).
        _ => ButtonSkin { radius: 3, fill: Fill::Panel(0.40), left_bar: 3.0, border: Border::None, dither: Dither::HoverReveal, ink: Ink::Light, center: false, bump: true },
    }
}

/// Dithered dots (three octaves) within `area`, revealed left→right up to `reveal_x`.
/// A painter-level twin of `menu::dither_fill` so the paint pass holds a single borrow.
fn dither_dots(p: &egui::Painter, area: Rect, reveal_x: f32, color: Color32, alpha: f32) {
    let wash = Rect::from_min_max(area.min, Pos2::new(reveal_x.min(area.right()), area.max.y));
    p.rect_filled(wash, menu::SQUARE, menu::a(color, 0.16 * alpha));
    for (grid, dot, oa) in [(2.0f32, 0.8f32, 0.9f32), (4.0, 1.0, 0.7), (8.0, 1.4, 0.5)] {
        let mut y = area.top() + 1.0;
        let mut row = 0u32;
        while y < area.bottom() {
            let xoff = if row % 2 == 0 { 0.0 } else { grid * 0.5 };
            let mut x = area.left() + 1.0 + xoff;
            while x < reveal_x && x < area.right() {
                p.rect_filled(
                    Rect::from_center_size(Pos2::new(x, y), Vec2::splat(dot)),
                    menu::SQUARE,
                    menu::a(color, oa * alpha),
                );
                x += grid;
            }
            y += grid;
            row += 1;
        }
    }
}

/// Animated dither for [`Dither::Pulse`]: a center band whose width random-walks
/// (id-seeded layered sines, deterministic per button) and whose alpha breathes.
/// Hover widens the band — the "center bump".
fn dither_pulse(p: &egui::Painter, area: Rect, color: Color32, time: f32, phase: f32, hover: f32) {
    let pulse = 0.30 + 0.45 * (0.5 + 0.5 * (time * 2.4 + phase).sin());
    let walk = 0.5 + 0.30 * (time * 0.9 + phase).sin() + 0.16 * (time * 1.7 + phase * 2.1).sin();
    let cover = walk.clamp(0.06, 0.94) * (0.55 + 0.45 * hover);
    let half = area.width() * 0.5 * cover;
    let cx = area.center().x;
    let band = Rect::from_min_max(Pos2::new(cx - half, area.top()), Pos2::new(cx + half, area.bottom()));
    dither_dots(p, band, band.right(), color, pulse * (0.85 + 0.15 * hover));
}

fn paint(ui: &Ui, it: &Interaction, sk: &ButtonSkin, label: &str, accent: Color32, th: &Theme) {
    let rect = it.rect;
    let hover = it.hover;
    let press = if it.down { 0.85 } else { 1.0 };
    let rad = r(sk.radius);
    let time = ui.input(|i| i.time) as f32;
    let p = ui.painter();

    // body fill
    match sk.fill {
        Fill::None => {}
        Fill::Panel(b) => { p.rect_filled(rect, rad, menu::a(th.panel, ((b + 0.15 * hover) * press).min(1.0))); }
        Fill::Accent(b) => { p.rect_filled(rect, rad, accent.gamma_multiply((b + 0.15 * hover) * press)); }
    }

    // dithered-pixel layer
    let body = rect.shrink(2.0);
    match sk.dither {
        Dither::Off => {}
        Dither::Always => dither_dots(p, body, body.right(), accent, 0.55),
        Dither::HoverReveal => {
            let rx = body.left() + body.width() * hover;
            dither_dots(p, body, rx, accent, 0.7);
        }
        Dither::Pulse => {
            dither_pulse(p, body, accent, time, rect.left() * 0.13 + rect.top() * 0.07, hover);
            ui.ctx().request_repaint();
        }
        Dither::Underline => {
            let bar = Rect::from_min_max(
                Pos2::new(rect.left() + 4.0, rect.bottom() - 6.0),
                Pos2::new(rect.right() - 4.0, rect.bottom() - 1.0),
            );
            dither_dots(p, bar, bar.right(), accent, 0.55 + 0.4 * hover);
        }
        Dither::Border => {
            let t = 3.0;
            let top = Rect::from_min_max(rect.left_top(), Pos2::new(rect.right(), rect.top() + t));
            let bot = Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - t), rect.right_bottom());
            dither_dots(p, top, top.right(), accent, 0.5 + 0.4 * hover);
            dither_dots(p, bot, bot.right(), accent, 0.5 + 0.4 * hover);
        }
        Dither::Corners => {
            let c = 7.0;
            for o in [
                rect.left_top(),
                Pos2::new(rect.right() - c, rect.top()),
                Pos2::new(rect.left(), rect.bottom() - c),
                Pos2::new(rect.right() - c, rect.bottom() - c),
            ] {
                let a = Rect::from_min_size(o, Vec2::splat(c));
                dither_dots(p, a, a.right(), accent, 0.5 + 0.4 * hover);
            }
        }
        Dither::Elegant => {
            // faint static edges (the quiet base)
            let te = 2.5;
            let top = Rect::from_min_max(rect.left_top(), Pos2::new(rect.right(), rect.top() + te));
            let bot = Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - te), rect.right_bottom());
            dither_dots(p, top, top.right(), accent, 0.35);
            dither_dots(p, bot, bot.right(), accent, 0.35);
            // mouse-tied smooth infill: a dither band centered on the cursor x,
            // its width eased in by hover (animate_bool). The cursor is already
            // smooth, so the fill tracks the mouse elegantly.
            if hover > 0.001 {
                let cx = it.resp.hover_pos().map(|q| q.x).unwrap_or(rect.center().x).clamp(rect.left(), rect.right());
                let half = rect.width() * (0.16 + 0.40 * hover);
                let band = Rect::from_min_max(
                    Pos2::new((cx - half).max(body.left()), body.top()),
                    Pos2::new((cx + half).min(body.right()), body.bottom()),
                );
                dither_dots(p, band, band.right(), accent, 0.28 + 0.30 * hover);
                ui.ctx().request_repaint();
            }
        }
    }

    // left accent bar (the dest_button signature; grows on hover)
    if sk.left_bar > 0.0 {
        let bw = sk.left_bar + 9.0 * hover;
        p.rect_filled(
            Rect::from_min_size(rect.min, egui::vec2(bw, rect.height())),
            menu::SQUARE,
            menu::a(accent, 0.7 + 0.3 * hover),
        );
    }

    // frame
    match sk.border {
        Border::None => {}
        Border::Hairline => { p.rect_stroke(rect, rad, Stroke::new(1.0, th.line), StrokeKind::Inside); }
        Border::Accent => { p.rect_stroke(rect, rad, Stroke::new(1.0, accent.gamma_multiply(0.35 + 0.5 * hover)), StrokeKind::Inside); }
    }

    // label — uppercase, letter-spaced. centered skins center it (hover size "bump");
    // bar / quiet skins left-align (hover right-nudge only when `bump`).
    let ink = match sk.ink {
        Ink::Light => menu::a(th.ink, 0.82 + 0.18 * hover),
        Ink::Accent => menu::a(accent, 0.85 + 0.15 * hover),
    };
    if sk.center {
        let fs = FONT_PX + if sk.bump { 1.5 * hover } else { 0.0 };
        p.text(rect.center(), egui::Align2::CENTER_CENTER, label_spaced(label), egui::FontId::proportional(fs), ink);
    } else {
        let nudge = if sk.bump { 4.0 * hover } else { 0.0 };
        let lx = rect.left() + if sk.left_bar > 0.0 { 16.0 } else { 12.0 } + nudge;
        p.text(Pos2::new(lx, rect.center().y), egui::Align2::LEFT_CENTER, label_spaced(label), egui::FontId::proportional(FONT_PX), ink);
    }
}

/// Sized button (the box is at least `min_size`). Composes layers 2→5 and honors the
/// live [`button_style`] candidate.
pub fn button_sized(ui: &mut Ui, label: &str, accent: Color32, min_size: Vec2) -> Response {
    let size = geometry(ui, label, min_size);
    let it = interact(ui, size);
    let th = current_theme(ui.ctx());
    let sk = candidate(button_style(), &th, accent);
    paint(ui, &it, &sk, label, accent, &th);
    it.resp
}

pub fn button(ui: &mut Ui, label: &str, accent: Color32) -> Response {
    button_sized(ui, label, accent, Vec2::ZERO)
}

/// The "elegant" button — faint edges + a dither infill that follows the cursor and
/// eases in with hover. For enchantment / magical / special-feel actions; forces the
/// elegant skin regardless of the live style index. Accent comes from the theme.
pub fn elegant_button_sized(ui: &mut Ui, label: &str, min_size: Vec2) -> Response {
    let th = current_theme(ui.ctx());
    let size = geometry(ui, label, min_size);
    let it = interact(ui, size);
    let sk = candidate(6, &th, th.accent);
    paint(ui, &it, &sk, label, th.accent, &th);
    it.resp
}
pub fn elegant_button(ui: &mut Ui, label: &str) -> Response {
    elegant_button_sized(ui, label, Vec2::ZERO)
}

/// A tab button: the toolkit look with a persistent selected state (selected =
/// accent fill + accent ink; unselected = quiet transparent). Centered label.
/// Returns the click `Response`. For settings / inventory tab strips.
pub fn tab_button(ui: &mut Ui, label: &str, selected: bool) -> Response {
    let th = current_theme(ui.ctx());
    let size = geometry(ui, label, Vec2::ZERO);
    let it = interact(ui, size);
    let sk = if selected {
        ButtonSkin { radius: 3, fill: Fill::Accent(0.22), left_bar: 0.0, border: Border::None, dither: Dither::Off, ink: Ink::Accent, center: true, bump: false }
    } else {
        ButtonSkin { radius: 3, fill: Fill::None, left_bar: 0.0, border: Border::None, dither: Dither::Off, ink: Ink::Light, center: true, bump: false }
    };
    paint(ui, &it, &sk, label, th.accent, &th);
    it.resp
}
