//! theme_bridge — turns a `snapclone-theme::Palette` into live egui styling.
//!
//! snapclone-theme already states it is egui-ready ("compatible with egui
//! Color32::from_hex"); this is the small glue that actually applies a palette
//! to an [`egui::Context`] and hands the carved painters their accent. The 9
//! palettes (Dark = the SnapClone warm-sepia look, plus Pitch Black, Paper,
//! Neon, Minimal, Modern, Earth, Pastels, Metallic) therefore drive the egui
//! front-end with no duplicated color tables.

use egui::{Color32, Context, Rounding, Stroke, Visuals};
use snapclone_theme::Palette;

use crate::tokens;

/// Parse a `#rrggbb` (or `#rrggbbaa`) hex string from the palette into a
/// `Color32`. Falls back to magenta on malformed input so mistakes are loud.
pub fn hex(s: &str) -> Color32 {
    let h = s.trim_start_matches('#');
    let b = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("ff"), 16).unwrap_or(0xff);
    match h.len() {
        6 => Color32::from_rgb(b(0), b(2), b(4)),
        8 => Color32::from_rgba_unmultiplied(b(0), b(2), b(4), b(6)),
        _ => Color32::from_rgb(0xff, 0x00, 0xff),
    }
}

/// The colors a carved surface needs, resolved from a palette.
#[derive(Clone, Copy)]
pub struct PaletteColors {
    pub bg_base: Color32,
    pub bg_card: Color32,
    pub bg_raised: Color32,
    pub bg_hover: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub border: Color32,
    pub glow: Color32,
}

impl PaletteColors {
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            bg_base: hex(p.bg_base),
            bg_card: hex(p.bg_card),
            bg_raised: hex(p.bg_raised),
            bg_hover: hex(p.bg_hover),
            text: hex(p.text_primary),
            text_weak: hex(p.text_secondary),
            accent: hex(p.accent),
            accent_soft: hex(p.accent_soft),
            danger: hex(p.danger),
            success: hex(p.success),
            warning: hex(p.warning),
            border: hex(p.border),
            glow: hex(p.glow_online),
        }
    }
}

/// Apply a palette to the whole context: base Visuals + the app's token radius
/// and spacing. Call when the user picks a theme in Settings ▸ Look.
pub fn apply_palette(ctx: &Context, p: &Palette) -> PaletteColors {
    let c = PaletteColors::from_palette(p);
    let mut v = Visuals::dark(); // start dark; pale palettes override below

    v.override_text_color = Some(c.text);
    v.panel_fill = c.bg_base;
    v.window_fill = c.bg_card;
    v.extreme_bg_color = c.bg_base;
    v.faint_bg_color = c.bg_raised;
    v.hyperlink_color = c.accent;
    v.selection.bg_fill = c.accent.linear_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, c.accent);

    let r = Rounding::same(tokens::RADIUS_SM as f32);
    let widgets = &mut v.widgets;
    for (state, fill) in [
        (&mut widgets.noninteractive, c.bg_card),
        (&mut widgets.inactive, c.bg_card),
        (&mut widgets.hovered, c.bg_hover),
        (&mut widgets.active, c.bg_hover),
        (&mut widgets.open, c.bg_raised),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, c.border);
        state.fg_stroke = Stroke::new(1.0, c.text);
        state.rounding = r;
    }
    widgets.hovered.fg_stroke = Stroke::new(1.0, c.text);
    widgets.active.bg_stroke = Stroke::new(1.0, c.accent);

    // token-driven spacing so the egui app matches the FP5-tuned C++ metrics
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(tokens::SPACE_SM as f32, tokens::SPACE_SM as f32);
        s.spacing.button_padding = egui::vec2(tokens::SPACE_MD as f32, tokens::SPACE_SM as f32);
        s.spacing.interact_size.y = tokens::TOUCH_MIN as f32; // touch targets >= 40px
    });
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parses_six_and_eight() {
        assert_eq!(hex("#1A1917"), Color32::from_rgb(0x1A, 0x19, 0x17));
        assert_eq!(hex("#C9B99A"), Color32::from_rgb(0xC9, 0xB9, 0x9A)); // DARK accent
        let a = hex("#11111080");
        assert_eq!(a.a(), 0x80);
    }
}
