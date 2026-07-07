//! Global egui theme that mirrors the HTML telemetry dashboard: the exact
//! palette (bg #0a0c10, panels, amber/cyan/violet/green accents), a monospace
//! type ramp, and compact spacing. Installed once from the eframe creator so
//! every widget inherits the look. Palette constants live in `widgets` so both
//! modules share one source of truth.

use crate::widgets as w;
use egui::{Color32, Context, FontFamily, FontId, Stroke, TextStyle, Visuals};

pub fn install(ctx: &Context) {
    let mut style = (*ctx.style()).clone();

    let mut v = Visuals::dark();
    v.override_text_color = Some(w::INK);
    v.panel_fill = w::BG;
    v.window_fill = w::PANEL;
    v.window_stroke = Stroke::new(1.0, w::LINE);
    v.extreme_bg_color = w::TRACK_BG;
    v.faint_bg_color = w::PANEL2;
    v.hyperlink_color = w::CYAN;
    v.warn_fg_color = w::AMBER;
    v.error_fg_color = w::RED;
    v.selection.bg_fill = w::CYAN.gamma_multiply(0.22);
    v.selection.stroke = Stroke::new(1.0, w::CYAN);

    // Widget states: idle panels read as the card colour; hover glows amber;
    // an active/pressed control flips to the amber "button" look from the web.
    let amber_ink = Color32::from_rgb(0x1a, 0x12, 0x06);
    v.widgets.noninteractive.bg_fill = w::PANEL;
    v.widgets.noninteractive.weak_bg_fill = w::PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, w::LINE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, w::INK);
    v.widgets.inactive.bg_fill = w::PANEL2;
    v.widgets.inactive.weak_bg_fill = w::PANEL2;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, w::LINE);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, w::DIM);
    v.widgets.hovered.bg_fill = amber_ink;
    v.widgets.hovered.weak_bg_fill = amber_ink;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, w::AMBER);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, w::AMBER_SOFT);
    v.widgets.active.bg_fill = w::AMBER;
    v.widgets.active.weak_bg_fill = w::AMBER;
    v.widgets.active.bg_stroke = Stroke::new(1.0, w::AMBER);
    v.widgets.active.fg_stroke = Stroke::new(1.0, amber_ink);
    style.visuals = v;

    // Monospace everywhere, sized to echo the web's 13px-base ramp.
    let m = FontFamily::Monospace;
    style.text_styles = [
        (TextStyle::Heading, FontId::new(15.0, m.clone())),
        (TextStyle::Body, FontId::new(12.0, m.clone())),
        (TextStyle::Monospace, FontId::new(11.5, m.clone())),
        (TextStyle::Button, FontId::new(11.0, m.clone())),
        (TextStyle::Small, FontId::new(9.5, m)),
    ]
    .into();

    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(9.0, 4.0);
    style.spacing.interact_size.y = 22.0;

    ctx.set_style(style);
}
