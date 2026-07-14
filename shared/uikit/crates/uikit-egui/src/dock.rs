//! dock — shared primitives for an app's surrounding chrome: a fixed-width side list (sessions,
//! nav rails, file lists — whatever an app's left column needs), a stack of full-width action
//! buttons anchored to the bottom of that column, a horizontal row of small "chip" buttons (quick
//! keys, filters, tags), and the docking math for a floating action button pinned to a content
//! area's corner.
//!
//! Extracted out of voiceterm's `shell.rs` (built to match its real Qt app's session sidebar + mic
//! FAB + bottom keycap bar from actual screenshots) once the shapes proved out: none of the four
//! pieces here are voiceterm-specific — a session list, a nav rail (gitmind's left icon column), a
//! file list, and a device list are all "rows in a fixed-width column, some selected, with actions
//! pinned below" — and any app with a floating action button (a mic, a compose button, a scan
//! trigger) needs the same corner-overlap docking math. Kept generic (label + selected/on-click, not
//! a fixed data shape) so each app supplies its own row content and a `PaletteColors` for the widget
//! to draw itself in — the app's own theme, not a shared "dock" colour identity.

use egui::{Align2, Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};
use crate::PaletteColors;

/// One row in a side list — a session, a nav-rail entry, a device, a file. Draws itself as a
/// selectable, hoverable block; returns the click response so the caller decides what a click
/// means (switch session, open a file, navigate).
pub fn list_row(ui: &mut Ui, label: &str, selected: bool, height: f32, c: &PaletteColors) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    let fill = if selected { c.accent_soft } else if resp.hovered() { c.bg_hover } else { c.bg_base };
    ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
    let text_color = if selected { c.bg_base } else { c.text };
    ui.painter().text(rect.left_center() + egui::vec2(10.0, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(13.0), text_color);
    resp
}

/// A stack of full-width bordered action buttons (e.g. "+ New" / "○ Graph" / "⚙ Settings"),
/// meant to sit at the bottom of a side list. Returns the index of a clicked button, if any.
pub fn action_stack(ui: &mut Ui, labels: &[&str], height: f32, c: &PaletteColors) -> Option<usize> {
    let mut clicked = None;
    for (i, label) in labels.iter().enumerate() {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
        let fill = if resp.hovered() { c.bg_hover } else { c.bg_raised };
        ui.painter().rect(rect, CornerRadius::same(4), fill, Stroke::new(1.0, c.border), StrokeKind::Inside);
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, *label, FontId::proportional(13.0), c.text);
        if resp.clicked() { clicked = Some(i); }
        ui.add_space(4.0);
    }
    clicked
}

/// One entry in a `chip_row`.
pub struct Chip<'a> {
    pub label: &'a str,
    /// Rendered width in characters — lets a caller size a "C-c"-style chord chip the same as a
    /// plain "Esc"/"Tab" one without every chip needing identical width.
    pub width_chars: usize,
}
impl<'a> Chip<'a> {
    pub fn new(label: &'a str) -> Self { Self { label, width_chars: label.len() } }
}

/// A horizontal row of small pill/keycap-style buttons — quick keys, filter tags, chord chips.
/// Wraps to a new line if the row doesn't fit (`ui.horizontal_wrapped`). Returns the index of a
/// clicked chip, if any.
pub fn chip_row(ui: &mut Ui, chips: &[Chip], height: f32, c: &PaletteColors) -> Option<usize> {
    let mut clicked = None;
    ui.horizontal_wrapped(|ui| {
        for (i, chip) in chips.iter().enumerate() {
            let w = chip.width_chars as f32 * 8.0 + 22.0;
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, height), Sense::click());
            let fill = if resp.is_pointer_button_down_on() { c.accent_soft } else if resp.hovered() { c.bg_hover } else { c.bg_raised };
            ui.painter().rect(rect, CornerRadius::same(6), fill, Stroke::new(1.0, c.border), StrokeKind::Inside);
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, chip.label, FontId::proportional(13.0), c.text);
            if resp.clicked() { clicked = Some(i); }
        }
    });
    clicked
}

/// Where a floating action button docks — the bottom-right corner of `content_rect`, overlapping
/// it slightly (the button sits ON the corner, not fully inside or outside — matches how a real
/// FAB reads against its host surface). `corner` picks which corner; the overlap fraction is fixed
/// at 35% of the FAB's own diameter, tuned against voiceterm's reference screenshots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner { TopLeft, TopRight, BottomLeft, BottomRight }

pub fn fab_dock(content_rect: Rect, diameter: f32, corner: Corner) -> Rect {
    let inset = diameter * 0.35;
    let center = match corner {
        Corner::TopLeft => content_rect.left_top() + egui::vec2(inset, inset),
        Corner::TopRight => content_rect.right_top() + egui::vec2(-inset, inset),
        Corner::BottomLeft => content_rect.left_bottom() + egui::vec2(inset, -inset),
        Corner::BottomRight => content_rect.right_bottom() + egui::vec2(-inset, -inset),
    };
    Rect::from_center_size(center, Vec2::splat(diameter))
}

/// Paint a floating circular action button at `rect` (see `fab_dock`); returns the interaction
/// response so the caller can wire press/hold/tap (e.g. anchoring a compass/menu to it).
pub fn fab_button(ui: &mut Ui, rect: Rect, glyph: &str, c: &PaletteColors) -> Response {
    let resp = ui.interact(rect, ui.id().with(("fab", rect.center().x as i32, rect.center().y as i32)), Sense::click());
    let fill = if resp.is_pointer_button_down_on() { c.accent } else { c.accent.gamma_multiply(0.9) };
    ui.painter().circle_filled(rect.center(), rect.width() * 0.5, fill);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(rect.width() * 0.4), Color32::WHITE);
    resp
}

/// Paint an accent-coloured border around a content area (e.g. the reference screenshots' visible
/// outline around the terminal/main surface).
pub fn accent_border(ui: &Ui, rect: Rect, c: &PaletteColors) {
    ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.5, c.accent), StrokeKind::Outside);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fab_dock_overlaps_the_chosen_corner_by_a_fixed_fraction() {
        let content = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(400.0, 300.0));
        let fab = fab_dock(content, 60.0, Corner::BottomRight);
        // centre pulled inward from the corner by inset = 0.35*diameter, so it should be inside
        // the content rect's bounds but the FAB's own edge should overlap past the border.
        assert!(fab.center().x < content.right() && fab.center().x > content.right() - 60.0);
        assert!(fab.center().y < content.bottom() && fab.center().y > content.bottom() - 60.0);
        assert!(fab.right() > content.right(), "the fab should overlap past the corner, not sit fully inside");
        assert!(fab.bottom() > content.bottom());
    }

    #[test]
    fn fab_dock_handles_all_four_corners() {
        let content = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(400.0, 300.0));
        let tl = fab_dock(content, 60.0, Corner::TopLeft);
        assert!(tl.left() < content.left() && tl.top() < content.top());
        let tr = fab_dock(content, 60.0, Corner::TopRight);
        assert!(tr.right() > content.right() && tr.top() < content.top());
        let bl = fab_dock(content, 60.0, Corner::BottomLeft);
        assert!(bl.left() < content.left() && bl.bottom() > content.bottom());
        let br = fab_dock(content, 60.0, Corner::BottomRight);
        assert!(br.right() > content.right() && br.bottom() > content.bottom());
    }

    #[test]
    fn chip_new_defaults_width_to_label_length() {
        let c = Chip::new("C-c");
        assert_eq!(c.width_chars, 3);
        let c2 = Chip::new("Esc");
        assert_eq!(c2.width_chars, 3);
    }
}
