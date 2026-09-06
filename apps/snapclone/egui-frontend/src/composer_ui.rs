//! composer_ui — `SnapComposerDialog`'s look, over the viewfinder.
//!
//! The Qt composer is a full-screen photo with the caption painted ON it in
//! `paintEvent`: bold, at the tapped position, word-wrapped to three quarters of
//! the width, a soft black drop shadow two pixels down-right so it reads on any
//! background. Under the photo: an **A** button that cycles the colour and shows
//! it, a one-line editor, a size slider between a small A and a large A, the
//! hint *"Tap image to reposition text"*, a row of timer chips `3s 5s 10s ∞`
//! with the chosen one filled, and a big accent **Send ➤** at `TOUCH_LARGE`.
//!
//! The port had a caption field, a button that said "colour" and a button that
//! said "5s", and drew nothing on the frame. What you were composing was not
//! visible anywhere.
//!
//! Model in `composer.rs`; this only draws it.

use crate::composer::{Composer, Duration, COLORS};
use uikit_egui::PaletteColors;

/// Timer chips, as the Qt `kDurations[]`: 3s, 5s, 10s, ∞ (zero = unlimited).
pub const TIMER_CHIPS: [Duration; 4] =
    [Duration::Seconds(3), Duration::Seconds(5), Duration::Seconds(10), Duration::Unlimited];

/// Qt `Theme::TOUCH_LARGE` — the Send button's minimum height.
pub const TOUCH_LARGE: f32 = 48.0;
pub const TOUCH_ICON: f32 = 40.0;
/// The Qt slider's range.
pub const FONT_MIN: f32 = 14.0;
pub const FONT_MAX: f32 = 72.0;

/// What the bottom panel asked for this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComposeAction {
    #[default]
    None,
    /// The big accent button: composition done, choose recipients.
    Next,
    Retake,
    Cancel,
}

/// The caption on the photo, exactly as `paintEvent` lays it: bold at the
/// composer's font size, wrapped to 3/4 of the frame, centred on
/// `(0.5, effective_caption_y)`, shadow first then the colour.
pub fn caption_overlay(painter: &egui::Painter, frame: egui::Rect, composer: &Composer) {
    let text = composer.caption.trim();
    if text.is_empty() {
        return;
    }
    let font = egui::FontId::new(composer.font_size.clamp(FONT_MIN, FONT_MAX), egui::FontFamily::Proportional);
    let wrap = frame.width() * 0.75;
    let [r, g, b] = composer.color_rgb();
    let color = egui::Color32::from_rgb(r, g, b);
    let centre = egui::pos2(frame.center().x, frame.top() + composer.effective_caption_y() * frame.height());
    let galley = painter.layout(text.to_string(), font, color, wrap);
    let rect = egui::Align2::CENTER_CENTER.anchor_size(centre, galley.size());
    // egui has no bold weight per galley without a font family for it; the
    // shadow doubled at two offsets thickens the strokes the way bold reads.
    let shadow = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 160);
    for off in [egui::vec2(2.0, 2.0), egui::vec2(1.0, 1.0)] {
        let sg = painter.layout(text.to_string(), egui::FontId::new(composer.font_size.clamp(FONT_MIN, FONT_MAX), egui::FontFamily::Proportional), shadow, wrap);
        painter.galley(rect.min + off, sg, shadow);
    }
    painter.galley(rect.min, galley, color);
}

/// A tap on the frame while composing moves the caption there (Qt: "Tap image
/// to reposition text"). Returns whether the tap was inside the frame.
pub fn reposition_on_tap(composer: &mut Composer, tap: egui::Pos2, frame: egui::Rect) -> bool {
    if !frame.contains(tap) {
        return false;
    }
    composer.caption_y = ((tap.y - frame.top()) / frame.height().max(1.0)).clamp(0.05, 0.95);
    true
}

/// The bottom panel. Returns what the person pressed.
pub fn compose_controls(ui: &mut egui::Ui, composer: &mut Composer, c: &PaletteColors) -> ComposeAction {
    let mut action = ComposeAction::None;
    // ── caption row: the colour "A" (shows the colour it will cycle FROM), the editor ──
    ui.horizontal(|ui| {
        let [r, g, b] = composer.color_rgb();
        let swatch = egui::Color32::from_rgb(r, g, b);
        let a = egui::Button::new(egui::RichText::new("A").size(20.0).strong().color(swatch))
            .fill(c.bg_raised)
            .corner_radius(TOUCH_ICON / 2.0)
            .min_size(egui::vec2(TOUCH_ICON, TOUCH_ICON));
        if ui.add(a).on_hover_text("caption colour").clicked() {
            composer.cycle_color();
        }
        let room = (ui.available_width() - 8.0).max(80.0);
        ui.add(
            egui::TextEdit::singleline(&mut composer.caption)
                .id_salt("composer_ui-caption")
                .hint_text("Type your message…")
                .desired_width(room),
        );
    });
    // ── size slider between a small A and a large A ──
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("A").size(12.0).color(c.text));
        ui.add(egui::Slider::new(&mut composer.font_size, FONT_MIN..=FONT_MAX).show_value(false));
        ui.label(egui::RichText::new("A").size(24.0).color(c.text));
    });
    ui.label(egui::RichText::new("Tap image to reposition text").small().color(c.text_weak));
    // ── timer chips ──
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Timer:").small().color(c.text_weak));
        for d in TIMER_CHIPS {
            let on = composer.duration == d;
            let chip = egui::Button::new(
                egui::RichText::new(d.label()).strong().color(if on { c.bg_base } else { c.text }),
            )
            .fill(if on { c.accent } else { c.bg_raised })
            .stroke(egui::Stroke::new(1.0, if on { c.accent } else { c.border }))
            .corner_radius(6.0)
            .min_size(egui::vec2(0.0, 32.0));
            if ui.add(chip).clicked() {
                composer.duration = d;
            }
        }
    });
    // what the timer MEANS, said before the send (see outbox::Delivery)
    let delivery = crate::outbox::Delivery::of(composer.duration);
    ui.label(
        egui::RichText::new(delivery.explain())
            .small()
            .color(if delivery.replicates() { c.warning } else { c.text_weak }),
    );
    ui.add_space(4.0);
    // ── [retake] [cancel] ……… [Next ➤] — Next dominant, as Send is in Qt ──
    ui.horizontal(|ui| {
        if ui.add(egui::Button::new("retake").min_size(egui::vec2(0.0, TOUCH_LARGE))).clicked() {
            action = ComposeAction::Retake;
        }
        if ui.add(egui::Button::new("cancel").min_size(egui::vec2(0.0, TOUCH_LARGE))).clicked() {
            action = ComposeAction::Cancel;
        }
        let next = egui::Button::new(
            egui::RichText::new("Next: choose recipients  \u{27a4}").size(17.0).strong().color(c.bg_base),
        )
        .fill(c.accent)
        .corner_radius(12.0)
        .min_size(egui::vec2(ui.available_width().max(160.0), TOUCH_LARGE));
        if ui.add(next).clicked() {
            action = ComposeAction::Next;
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tap_on_the_frame_moves_the_caption_and_one_outside_does_not() {
        let mut cmp = Composer::default();
        let frame = egui::Rect::from_min_size(egui::pos2(0.0, 100.0), egui::vec2(400.0, 600.0));
        assert!(reposition_on_tap(&mut cmp, egui::pos2(200.0, 400.0), frame));
        assert!((cmp.caption_y - 0.5).abs() < 1e-4);
        assert!(reposition_on_tap(&mut cmp, egui::pos2(200.0, 130.0), frame));
        assert!((cmp.caption_y - 0.05).abs() < 1e-4, "clamped to the top margin");
        assert!(!reposition_on_tap(&mut cmp, egui::pos2(200.0, 50.0), frame), "above the frame");
        assert!((cmp.caption_y - 0.05).abs() < 1e-4, "and unchanged");
    }

    #[test]
    fn the_panel_and_the_overlay_draw_headless_and_a_frame_without_clicks_does_nothing() {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 700.0)));
        let c = PaletteColors::from_palette(snapclone_theme::palette_by_id("midnight"));
        let mut cmp = Composer::default();
        cmp.caption = "hello there".into();
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let frame = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(380.0, 300.0));
                caption_overlay(&ui.painter_at(frame), frame, &cmp);
                ui.add_space(300.0);
                assert_eq!(compose_controls(ui, &mut cmp, &c), ComposeAction::None);
            });
        });
        assert!(!out.shapes.is_empty());
        assert_eq!(cmp.duration, Duration::Seconds(5), "untouched without a click");
    }

    #[test]
    fn the_chips_are_the_qt_durations_and_zero_is_the_infinity_chip() {
        assert_eq!(TIMER_CHIPS[3], Duration::Unlimited);
        assert_eq!(TIMER_CHIPS[3].to_secs(), 0);
        assert_eq!(TIMER_CHIPS.iter().map(|d| d.label()).collect::<Vec<_>>(), vec!["3s", "5s", "10s", "∞"]);
    }
}
