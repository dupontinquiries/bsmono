//! chips — a row of options with the chosen one filled: the Qt duration selector's
//! `3s 5s 10s ∞`, a palette picker, any small closed set where one is on.
//!
//! A chip is a glass key at 32 high; the chosen one is the accent, solid, with the base
//! colour for its text (the C++ `QPushButton:checked`). Returns the index tapped this frame,
//! if any — the caller owns the selection, so a chip row cannot disagree with the model it
//! shows.

use crate::PaletteColors;
use egui::{CornerRadius, Stroke, Ui};

/// The chips' height (the Qt buttons' `setFixedHeight(32)`).
pub const CHIP_HEIGHT: f32 = 32.0;

/// Draw the row. `label`, when given, leads it in weak small text ("Timer:").
pub fn row(ui: &mut Ui, label: Option<&str>, options: &[&str], selected: usize, c: &PaletteColors) -> Option<usize> {
    let mut tapped = None;
    ui.horizontal(|ui| {
        if let Some(l) = label {
            ui.label(egui::RichText::new(l).small().color(c.text_weak));
        }
        for (i, opt) in options.iter().enumerate() {
            let on = i == selected;
            let chip = egui::Button::new(egui::RichText::new(*opt).strong().color(if on { c.bg_base } else { c.text }))
                .fill(if on { c.accent } else { crate::glass::key_fill(c.bg_card) })
                .stroke(Stroke::new(1.0_f32, if on { c.accent } else { crate::glass::tint(c.border, crate::glass::GlassAlphas::BORDER) }))
                .corner_radius(CornerRadius::same(6))
                .min_size(egui::vec2(0.0, CHIP_HEIGHT));
            if ui.add(chip).clicked() {
                tapped = Some(i);
            }
        }
    });
    tapped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_draws_every_option_and_reports_nothing_without_a_tap() {
        let c = crate::default_palette();
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 200.0)));
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert_eq!(row(ui, Some("Timer:"), &["3s", "5s", "10s", "\u{221e}"], 1, &c), None);
            });
        });
        let texts = out.shapes.iter().filter(|s| matches!(s.shape, egui::Shape::Text(_))).count();
        assert_eq!(texts, 5, "the label and four chips");
    }
}
