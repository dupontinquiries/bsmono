//! carved — the L2 "how the look is drawn" painters + the L3 semantic widgets,
//! ported from `logscope-egui theme.rs` (`paint_carved`, `carved_button`, …).
//!
//! L2 ([`paint_carved`]) renders the material: a rounded translucent fill
//! tinted toward the accent, a lit top bevel and shadowed bottom bevel for
//! relief, and an animated specular sheen. L3 ([`button`], [`pill`], [`chip`],
//! [`panel`]) are the names the app codes against; restyling means editing the
//! [`Skin`] only.
//!
//! Call sites live against L3. They never see the `Skin` math — that is the
//! whole point of the sandwich.

use egui::{
    Align2, Color32, FontId, Pos2, Rect, Response, CornerRadius, Sense, Stroke, Ui, Vec2,
};

use crate::skin::Skin;
use crate::tokens;

/// Per-state fill opacity selector.
#[derive(Clone, Copy)]
pub enum State {
    Rest,
    Hover,
    Down,
    Off,
}

fn fill_opacity(skin: &Skin, s: State) -> u8 {
    match s {
        State::Rest => skin.fill_rest,
        State::Hover => skin.fill_hover,
        State::Down => skin.fill_down,
        State::Off => skin.fill_off,
    }
}

/// Blend `over` onto `base` by `over`'s alpha (simple src-over).
fn over(base: Color32, ov: Color32) -> Color32 {
    let a = ov.a() as f32 / 255.0;
    let mix = |b: u8, o: u8| (b as f32 * (1.0 - a) + o as f32 * a) as u8;
    Color32::from_rgb(mix(base.r(), ov.r()), mix(base.g(), ov.g()), mix(base.b(), ov.b()))
}

/// L2: paint the carved material into `rect`. `base` is the surface behind it,
/// `accent` tints the fill.
///
/// **Energy note.** `sheen_phase` is a *one-shot* progress in `0.0..=1.0` (e.g.
/// from `ui.ctx().animate_bool_with_time(...)`), **not** wall-clock time. The
/// sheen band is drawn only while `0 < sheen_phase < 1`; at rest (0 or 1) it is
/// skipped entirely. This is deliberate: a continuously-animating sheen would
/// force the app to `request_repaint()` every frame forever, pinning a core —
/// an energy footgun on the FP5. Drive `sheen_phase` from an interaction
/// (hover/press) so the surface is still at idle. The L3 widgets below do this
/// for you; static surfaces (list rows, etc.) should pass `0.0`.
pub fn paint_carved(
    ui: &Ui,
    rect: Rect,
    base: Color32,
    accent: Color32,
    skin: &Skin,
    state: State,
    sheen_phase: f32,
) {
    let painter = ui.painter_at(rect);
    let rounding = CornerRadius::same(skin.radius as u8);

    // 1) translucent fill, lifted toward white then tinted toward accent
    let op = fill_opacity(skin, state) as f32 / 255.0;
    let lift = over(base, Color32::from_white_alpha((op * 0.16 * 255.0) as u8));
    let fill = lerp_color(lift, accent, skin.accent_tint);
    painter.rect_filled(rect, rounding, fill);

    // 2) relief bevels: lit top edge, shadowed bottom edge
    let top = Stroke::new(1.0, Color32::from_white_alpha(skin.bevel_lit));
    let bot = Stroke::new(1.5, Color32::from_black_alpha(skin.bevel_shadow));
    painter.line_segment(
        [rect.left_top() + Vec2::new(2.0, 1.0), rect.right_top() + Vec2::new(-2.0, 1.0)],
        top,
    );
    painter.line_segment(
        [rect.left_bottom() + Vec2::new(2.0, -1.0), rect.right_bottom() + Vec2::new(-2.0, -1.0)],
        bot,
    );

    // 3) one-shot specular sheen — only while an interaction sweep is active.
    //    Skipped at rest so the surface needs no idle repaints.
    if skin.sheen > 0.0 && sheen_phase > 0.0 && sheen_phase < 1.0 {
        let cx = rect.left() + (rect.width() + rect.height()) * sheen_phase - rect.height();
        let band = Rect::from_min_max(
            Pos2::new(cx, rect.top()),
            Pos2::new(cx + rect.height() * 0.6, rect.bottom()),
        )
        .intersect(rect);
        if band.width() > 0.0 {
            // fade the glint out across the sweep so the tail is soft
            let glint = (28.0 * skin.sheen * (1.0 - sheen_phase)) as u8;
            painter.rect_filled(band, rounding, Color32::from_white_alpha(glint));
        }
    }

    // 4) outer hairline so adjacent carved tiles read as separate
    painter.rect_stroke(rect, rounding, Stroke::new(1.0, Color32::from_black_alpha(40)), egui::StrokeKind::Inside);
}

/// L2: carved (engraved) text — a dark shadow below the ink and a lit lip above
/// give the impression the glyphs are cut into the surface.
pub fn carved_text(ui: &Ui, center: Pos2, text: &str, font: FontId, ink: Color32, skin: &Skin) {
    let p = ui.painter();
    p.text(
        center + Vec2::new(0.0, 1.0),
        Align2::CENTER_CENTER,
        text,
        font.clone(),
        Color32::from_black_alpha(skin.engrave_shadow),
    );
    p.text(
        center + Vec2::new(0.0, -1.0),
        Align2::CENTER_CENTER,
        text,
        font.clone(),
        Color32::from_white_alpha(skin.engrave_lip),
    );
    p.text(center, Align2::CENTER_CENTER, text, font, ink);
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

// ─────────────────────────────────────────────────────────────────────────────
// L3 · semantic widgets (the names the app calls)
// ─────────────────────────────────────────────────────────────────────────────

/// A plain carved button.
pub fn button(ui: &mut Ui, text: &str, base: Color32, accent: Color32, ink: Color32, skin: &Skin) -> Response {
    carved_clickable(ui, text, base, accent, ink, skin, None)
}

/// A carved button tinted toward an accent (the prominent call-to-action).
pub fn button_accent(ui: &mut Ui, text: &str, base: Color32, accent: Color32, ink: Color32, skin: &Skin) -> Response {
    carved_clickable(ui, text, base, accent, ink, skin, Some(accent))
}

/// A rounded toggle-ish pill. `on` swaps to the down fill + accent ring.
pub fn pill(ui: &mut Ui, text: &str, base: Color32, accent: Color32, ink: Color32, skin: &Skin, on: bool) -> Response {
    let font = FontId::proportional(tokens::FONT_SM as f32);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font.clone(), ink);
    let size = Vec2::new(galley.size().x + 28.0, (tokens::TOUCH_MIN as f32 - 6.0).max(28.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let mut s = if on { State::Down } else if resp.hovered() { State::Hover } else { State::Rest };
    if resp.is_pointer_button_down_on() { s = State::Down; }
    let pill_skin = Skin { radius: tokens::RADIUS_FULL as u8, ..*skin };
    let phase = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered() || resp.is_pointer_button_down_on(), 0.25);
    paint_carved(ui, rect, base, accent, &pill_skin, s, phase);
    if on {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same((rect.height() / 2.0) as u8),
            Stroke::new(1.0, accent),
            egui::StrokeKind::Inside,
        );
    }
    let label_ink = if on { accent } else { ink };
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        ui.painter().layout_no_wrap(text.to_owned(), font, label_ink),
        label_ink,
    );
    resp
}

/// A small static tag.
pub fn chip(ui: &mut Ui, text: &str, fg: Color32, bg: Color32, border: Color32) {
    let font = FontId::proportional(tokens::FONT_XS as f32);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, fg);
    let size = Vec2::new(galley.size().x + 18.0, galley.size().y + 6.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let r = CornerRadius::same((rect.height() / 2.0) as u8);
    ui.painter().rect_filled(rect, r, bg);
    ui.painter().rect_stroke(rect, r, Stroke::new(1.0, border), egui::StrokeKind::Inside);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, fg);
}

/// A titled carved panel; runs `body` inside it.
pub fn panel(ui: &mut Ui, title: &str, base: Color32, accent: Color32, ink: Color32, skin: &Skin, body: impl FnOnce(&mut Ui)) {
    egui::Frame::none()
        .corner_radius(CornerRadius::same(tokens::RADIUS_MD as u8))
        .fill(base)
        .inner_margin(egui::Margin::same(tokens::SPACE_MD as i8))
        .show(ui, |ui| {
            if !title.is_empty() {
                let _ = accent; // available for a future titled accent rule
                ui.label(egui::RichText::new(title).color(ink).size(tokens::FONT_SM as f32).strong());
                ui.add_space(tokens::SPACE_SM as f32);
            }
            body(ui);
        });
    let _ = skin;
}

fn carved_clickable(
    ui: &mut Ui,
    text: &str,
    base: Color32,
    accent: Color32,
    ink: Color32,
    skin: &Skin,
    tint: Option<Color32>,
) -> Response {
    let font = FontId::proportional(tokens::FONT_BASE as f32);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font.clone(), ink);
    let size = Vec2::new(
        galley.size().x + (tokens::SPACE_MD as f32) * 2.0,
        tokens::TOUCH_MIN as f32,
    );
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let mut s = if resp.hovered() { State::Hover } else { State::Rest };
    if resp.is_pointer_button_down_on() { s = State::Down; }
    let acc = tint.unwrap_or(accent);
    // sheen sweeps only while hover/press ramps in or out; egui repaints during
    // that ~0.25s transition and then parks — no idle cost.
    let phase = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered() || resp.is_pointer_button_down_on(), 0.25);
    paint_carved(ui, rect, base, acc, skin, s, phase);
    carved_text(ui, rect.center(), text, font, ink, skin);
    resp
}
