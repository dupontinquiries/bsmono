//! Settings screen: typography controls, reading-theme picker, app
//! palette picker (bridging to `snapclone_theme`), and gesture/touch
//! preferences that flow into the shared `settings_kit::Prefs` so other
//! apps in the ecosystem stay in sync on shared knobs like touch target
//! size.

use crate::layout::{FontFamily, Typography};
use crate::theme::{ReaderPrefs, ReadingTheme};
use eframe::egui::{self, RichText, Slider};
use snapclone_theme::PaletteId;
use uikit_egui::{toggle_grid, PaletteColors};

/// Draw the typography section: font size, line height, margins, columns,
/// justification, hyphenation, font family. Returns true if any value
/// changed (callers use this to know when to invalidate the pagination
/// cache).
pub fn draw_typography_settings(ui: &mut egui::Ui, typo: &mut Typography, colors: &PaletteColors) -> bool {
    let mut changed = false;
    ui.colored_label(colors.text_weak, "TEXT");
    changed |= ui
        .add(Slider::new(&mut typo.font_size, 10.0..=48.0).text("Font size"))
        .changed();
    changed |= ui
        .add(Slider::new(&mut typo.line_height_mult, 1.0..=2.2).text("Line spacing"))
        .changed();
    changed |= ui
        .add(Slider::new(&mut typo.paragraph_spacing_mult, 0.0..=2.0).text("Paragraph spacing"))
        .changed();
    changed |= ui
        .add(Slider::new(&mut typo.margin_frac, 0.0..=0.25).text("Margins"))
        .changed();

    ui.add_space(8.0);
    ui.colored_label(colors.text_weak, "FONT");
    egui::ComboBox::from_label("Family")
        .selected_text(typo.font_family.label())
        .show_ui(ui, |ui| {
            for f in FontFamily::all() {
                if ui
                    .selectable_label(typo.font_family == *f, f.label())
                    .clicked()
                {
                    typo.font_family = *f;
                    changed = true;
                }
            }
        });
    changed |= ui.checkbox(&mut typo.bold_headings, "Bold headings").changed();
    changed |= ui
        .checkbox(&mut typo.indent_paragraphs, "Indent paragraphs")
        .changed();

    ui.add_space(8.0);
    ui.colored_label(colors.text_weak, "LAYOUT");
    let mut columns_idx = (typo.columns.max(1) - 1) as usize;
    let col_labels = ["1 column", "2 columns", "3 columns"];
    egui::ComboBox::from_label("Columns")
        .selected_text(col_labels.get(columns_idx).copied().unwrap_or("1 column"))
        .show_ui(ui, |ui| {
            for (i, label) in col_labels.iter().enumerate() {
                if ui.selectable_label(columns_idx == i, *label).clicked() {
                    columns_idx = i;
                    typo.columns = (i + 1) as u8;
                    changed = true;
                }
            }
        });
    changed |= ui.checkbox(&mut typo.justify, "Justify text").changed();
    changed |= ui.checkbox(&mut typo.hyphenate, "Hyphenate long words").changed();

    changed
}

/// Draw the reading-theme picker as a grid of swatches.
pub fn draw_theme_picker(ui: &mut egui::Ui, prefs: &mut ReaderPrefs, colors: &PaletteColors) -> bool {
    let mut changed = false;
    ui.colored_label(colors.text_weak, "READING THEME");
    ui.horizontal_wrapped(|ui| {
        for theme in ReadingTheme::all() {
            let (bg, text, _) = theme.colors();
            let selected = prefs.reading_theme == *theme;
            let size = egui::vec2(64.0, 44.0);
            let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
            ui.painter().rect_filled(rect, egui::CornerRadius::same(6), bg);
            if selected {
                ui.painter().rect_stroke(
                    rect,
                    egui::CornerRadius::same(6),
                    egui::Stroke::new(2.0, colors.accent),
                    egui::StrokeKind::Inside,
                );
            }
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Aa",
                egui::FontId::proportional(16.0),
                text,
            );
            if response.clicked() {
                prefs.reading_theme = *theme;
                changed = true;
            }
            response.on_hover_text(theme.label());
        }
    });
    changed |= ui
        .checkbox(&mut prefs.auto_night_by_clock, "Switch to Night automatically")
        .changed();
    if prefs.auto_night_by_clock {
        ui.horizontal(|ui| {
            ui.label("From");
            changed |= ui
                .add(Slider::new(&mut prefs.night_start_hour, 0..=23))
                .changed();
            ui.label("to");
            changed |= ui.add(Slider::new(&mut prefs.night_end_hour, 0..=23)).changed();
        });
    }
    changed
}

/// Draw the app-chrome palette picker (bridges to snapclone_theme).
pub fn draw_palette_picker(ui: &mut egui::Ui, current: &mut String, colors: &PaletteColors) -> bool {
    let mut changed = false;
    ui.colored_label(colors.text_weak, "APP THEME");
    ui.horizontal_wrapped(|ui| {
        for id in PaletteId::all() {
            let selected = current == id.as_str();
            if ui.selectable_label(selected, id.display_name()).clicked() {
                *current = id.as_str().to_string();
                changed = true;
            }
        }
    });
    changed
}

/// Draw touch/gesture preferences that map onto `settings_kit::Prefs`
/// fields shared across the whole ecosystem (so tuning "how sensitive is
/// a swipe" here also affects other uikit-egui apps consistently).
pub fn draw_gesture_settings(
    ui: &mut egui::Ui,
    prefs: &mut settings_kit::GesturePrefs,
    touch: &mut settings_kit::TouchPrefs,
    colors: &PaletteColors,
) -> bool {
    let mut changed = false;
    ui.colored_label(colors.text_weak, "GESTURES");
    changed |= ui
        .add(Slider::new(&mut prefs.sensitivity, 0.1..=2.0).text("Swipe sensitivity"))
        .changed();
    changed |= ui
        .add(Slider::new(&mut prefs.commit_fraction, 0.1..=0.9).text("Page-turn commit distance"))
        .changed();
    changed |= ui
        .add(Slider::new(&mut prefs.flick_velocity, 100.0..=3000.0).text("Flick velocity threshold"))
        .changed();

    ui.add_space(8.0);
    ui.colored_label(colors.text_weak, "TOUCH");
    changed |= ui.add(Slider::new(&mut touch.min_target, 32.0..=72.0).text("Minimum touch target")).changed();
    changed |= ui.add(Slider::new(&mut touch.scale, 0.8..=1.6).text("UI scale")).changed();
    changed |= ui.add(Slider::new(&mut touch.text_scale, 0.8..=2.0).text("Text scale")).changed();
    changed
}

/// Draw the reader-behaviour toggles section (status bar, tap zones,
/// screen wake lock, volume-key page turns).
pub fn draw_behavior_settings(ui: &mut egui::Ui, prefs: &mut ReaderPrefs, colors: &PaletteColors) -> bool {
    let mut changed = false;
    ui.colored_label(colors.text_weak, "BEHAVIOR");
    changed |= ui.checkbox(&mut prefs.keep_screen_awake, "Keep screen awake while reading").changed();
    changed |= ui.checkbox(&mut prefs.show_status_bar, "Show status bar").changed();
    changed |= ui.checkbox(&mut prefs.show_progress_percent, "Show progress percentage").changed();
    changed |= ui.checkbox(&mut prefs.tap_zones_enabled, "Tap zones for page turns").changed();
    changed |= ui.checkbox(&mut prefs.volume_key_page_turn, "Volume keys turn pages").changed();
    changed
}

/// Convenience: a labeled section header used across settings tabs.
pub fn section_header(ui: &mut egui::Ui, text: &str, colors: &PaletteColors) {
    ui.add_space(12.0);
    ui.label(RichText::new(text).color(colors.text_weak).small());
    ui.separator();
}

/// A small "reset to defaults" affordance shared by every settings tab,
/// backed by uikit_egui's toggle_grid just to demonstrate composing that
/// widget for a binary "apply now?" confirmation strip.
/// API drift from the drop: this was written against a `toggle_grid` whose
/// `ToggleOption::new` took an `egui::Id` and whose `show` anchored on one.
/// The current widget takes a `String` id and anchors on the host `Response`,
/// which is what places the grid relative to the control that opened it.
pub fn draw_reset_strip(ui: &mut egui::Ui, host: &egui::Response) -> bool {
    let options = vec![toggle_grid::ToggleOption::new(
        "reset_confirm",
        "Reset this section to defaults",
        false,
    )];
    let mut grid = toggle_grid::ToggleGrid::new("reset_strip").columns(1);
    // Committed, not Toggled: the widget now reports on RELEASE with the final
    // state of every cell, rather than firing mid-drag. For a confirmation
    // strip that is the correct moment — a reset that triggers while the finger
    // is still moving is a reset nobody asked for.
    match grid.show(ui, host, &options) {
        toggle_grid::ToggleOutcome::Committed(cells) => cells.iter().any(|(_, on)| *on),
        toggle_grid::ToggleOutcome::Idle => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_colors_readable_contrast() {
        for theme in ReadingTheme::all() {
            let (bg, text, _) = theme.colors();
            let bg_lum = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
            let text_lum = 0.299 * text.r() as f32 + 0.587 * text.g() as f32 + 0.114 * text.b() as f32;
            assert!((bg_lum - text_lum).abs() > 40.0, "{:?} lacks contrast", theme);
        }
    }
}
