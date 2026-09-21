//! A visual preset for e-ink panels.
//!
//! egui's light theme is grey text on light-grey controls over a grey
//! panel. On a 16-level e-ink panel that washes out, and worse, pressing a
//! button changes a few grey levels, which the fast waveforms barely show:
//! the finger gets no feedback. This preset is paper and ink:
//!
//! * pure white surfaces (paper is the panel's natural state and costs
//!   nothing to hold), pure black text, 2 px black outlines on controls;
//! * **pressed = inverted** (black fill, white text): the largest possible
//!   pixel change, so the fast waveform shows the press immediately;
//! * hover looks identical to rest: the pen digitizer reports hover, and a
//!   hover highlight would spend a refresh on every pen approach;
//! * no shadows (grey gradients are ghosting fodder), and bigger hit
//!   targets and spacing, because this is driven by fingers.
//!
//! Colour panels keep one accent (selection, slider fill) so colour means
//! "this is selected", not decoration.
//!
//! The engine applies it at startup; `EINK_THEME=egui` keeps egui's own
//! theme, and an app that sets its own visuals simply overrides it.

use egui::{Color32, CornerRadius, Shadow, Stroke, Style, Vec2, Visuals};

pub fn apply(style: &mut Style, color_panel: bool) {
    style.visuals = visuals(color_panel);
    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(10.0, 10.0);
    s.button_padding = Vec2::new(14.0, 10.0);
    s.interact_size = Vec2::new(48.0, 44.0);
    s.icon_width = 22.0;
    s.icon_width_inner = 12.0;
    s.slider_rail_height = 10.0;
    style.animation_time = 0.0;
}

pub fn visuals(color_panel: bool) -> Visuals {
    let ink = Color32::BLACK;
    let paper = Color32::WHITE;
    let accent = if color_panel { Color32::from_rgb(30, 70, 200) } else { ink };
    let mut v = Visuals::light();
    v.override_text_color = Some(ink);
    v.panel_fill = paper;
    v.window_fill = paper;
    v.extreme_bg_color = paper;
    v.faint_bg_color = paper;
    v.code_bg_color = Color32::from_gray(235);
    v.window_stroke = Stroke::new(2.0, ink);
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow::NONE;
    v.hyperlink_color = accent;
    v.selection.bg_fill = if color_panel { Color32::from_rgb(170, 190, 245) } else { Color32::from_gray(190) };
    v.selection.stroke = Stroke::new(2.0, ink);
    v.slider_trailing_fill = true;

    let rest = egui::style::WidgetVisuals {
        // bg_fill draws slider rails and checkbox wells; white made the rail
        // invisible. Buttons use weak_bg_fill, which stays paper.
        bg_fill: Color32::from_gray(215),
        weak_bg_fill: paper,
        bg_stroke: Stroke::new(2.0, ink),
        corner_radius: CornerRadius::same(6),
        fg_stroke: Stroke::new(2.0, ink),
        expansion: 0.0,
    };
    let pressed = egui::style::WidgetVisuals {
        bg_fill: ink,
        weak_bg_fill: ink,
        fg_stroke: Stroke::new(2.0, paper),
        ..rest
    };
    v.widgets.noninteractive = egui::style::WidgetVisuals { bg_stroke: Stroke::new(1.0, ink), fg_stroke: Stroke::new(1.0, ink), ..rest };
    v.widgets.inactive = rest;
    v.widgets.hovered = rest; // deliberately identical: hover costs a refresh
    v.widgets.active = pressed;
    v.widgets.open = egui::style::WidgetVisuals { bg_stroke: Stroke::new(3.0, ink), ..rest };
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_is_an_inversion_and_hover_costs_nothing() {
        let v = visuals(false);
        assert_eq!(v.widgets.inactive.weak_bg_fill, Color32::WHITE);
        assert_ne!(v.widgets.inactive.bg_fill, v.panel_fill, "slider rail must be visible on the panel");
        assert_eq!(v.widgets.active.weak_bg_fill, Color32::BLACK);
        assert_eq!(v.widgets.active.fg_stroke.color, Color32::WHITE);
        assert_eq!(v.widgets.hovered, v.widgets.inactive);
        assert_eq!(v.override_text_color, Some(Color32::BLACK));
    }

    #[test]
    fn colour_is_only_an_accent_on_colour_panels() {
        assert!(crate::format::is_chromatic(
            visuals(true).hyperlink_color.r(), visuals(true).hyperlink_color.g(), visuals(true).hyperlink_color.b()));
        let m = visuals(false).selection.bg_fill;
        assert!(!crate::format::is_chromatic(m.r(), m.g(), m.b()));
    }
}
