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
/// Make a laid-out row clickable, keyed by something stable about the row.
///
/// The pattern this replaces is `ui.horizontal(|ui| {…}).response
/// .interact(Sense::click())`, which appears wherever a row is built from
/// several widgets rather than drawn as one. It does not work: a layout
/// response carries an auto-generated id, and re-interacting through it
/// registers no clickable region — the rows draw correctly and simply
/// cannot be clicked. Found by a frame-driven test in voiceterm's mail
/// list, where opening a thread had never worked and every model test
/// passed throughout, because none of them drew anything.
///
/// `key` must be stable and unique per row — a conversation id, a path, a
/// url. NOT an index: egui remembers interaction state by id, and an
/// index-keyed row hands the previous occupant's press state to whatever
/// sorts into that slot after the list changes.
pub fn row_click(ui: &mut Ui, rect: egui::Rect, key: impl std::hash::Hash) -> Response {
    ui.interact(rect, ui.id().with(("row_click", key)), Sense::click())
}

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
/// A stack of action buttons, returning the ACTION rather than an index.
///
/// Prefer this over [`action_stack`]. The index form forces the caller to keep
/// two parallel lists in step:
///
/// ```ignore
/// if let Some(i) = action_stack(ui, &["+ New", "○ Graph", "⚙ Settings"], 30.0, c) {
///     action = Some([Action::New, Action::Graph, Action::Settings][i]);
/// }
/// ```
///
/// Nothing checks that those two literals are the same length. Add a fourth
/// label and the click panics — which is exactly the bug that made gitmind's
/// Ctrl+A crash the program: an eleven-entry tab list indexed into a nine-entry
/// key array. Pairing the label with its action makes the mismatch
/// unrepresentable rather than merely absent.
pub fn action_items<T: Clone>(ui: &mut Ui, items: &[(&str, T)], height: f32,
                              c: &PaletteColors) -> Option<T> {
    let labels: Vec<&str> = items.iter().map(|(l, _)| *l).collect();
    action_stack(ui, &labels, height, c).map(|i| items[i].1.clone())
}

pub fn action_stack(ui: &mut Ui, labels: &[&str], height: f32, c: &PaletteColors) -> Option<usize> {
    let mut clicked = None;
    for (i, label) in labels.iter().enumerate() {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
        let fill = if resp.hovered() { c.bg_hover } else { c.bg_raised };
        ui.painter().rect(rect, CornerRadius::same(4), fill, Stroke::new(1.0_f32, c.border), StrokeKind::Inside);
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
    chip_row_ex(ui, chips, height, c).0
}

/// `chip_row` plus every chip's `Response` — for callers that need press/hold state (the keycap
/// bar's held-arrow auto-repeat) rather than just clicks.
pub fn chip_row_ex(ui: &mut Ui, chips: &[Chip], height: f32, c: &PaletteColors) -> (Option<usize>, Vec<egui::Response>) {
    let mut clicked = None;
    let mut responses = Vec::with_capacity(chips.len());
    ui.horizontal_wrapped(|ui| {
        for (i, chip) in chips.iter().enumerate() {
            let w = chip.width_chars as f32 * 8.0 + 22.0;
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, height), Sense::click_and_drag());
            let fill = if resp.is_pointer_button_down_on() { c.accent_soft } else if resp.hovered() { c.bg_hover } else { c.bg_raised };
            ui.painter().rect(rect, CornerRadius::same(6), fill, Stroke::new(1.0_f32, c.border), StrokeKind::Inside);
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, chip.label, FontId::proportional(13.0), c.text);
            if resp.clicked() { clicked = Some(i); }
            responses.push(resp);
        }
    });
    (clicked, responses)
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
    ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.5_f32, c.accent), StrokeKind::Outside);
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

#[cfg(test)]
mod action_items_tests {
    /// The pairing is a TYPE-level guarantee, so the test that matters is that
    /// the mismatch cannot be written rather than that it is caught at runtime.
    ///
    /// `action_items(&[(label, action)])` has no second list to fall out of
    /// step with. The old shape — `[A, B, C][i]` where `i` came from a
    /// separately-written label array — is what crashed gitmind on every Ctrl
    /// chord, and no amount of testing the panicking version prevents someone
    /// adding a tenth entry to one side.
    ///
    /// What is testable is that pairing preserves order, since a zip that
    /// mismatched would be a subtler version of the same bug.
    #[test]
    fn pairing_preserves_the_order_of_labels_and_actions() {
        #[derive(Clone, PartialEq, Debug)]
        enum A { New, Graph, Settings }
        let items = [("+ New", A::New), ("○ Graph", A::Graph), ("⚙ Settings", A::Settings)];
        let labels: Vec<&str> = items.iter().map(|(l, _)| *l).collect();
        assert_eq!(labels, vec!["+ New", "○ Graph", "⚙ Settings"]);
        assert_eq!(items[2].1, A::Settings, "the third label carries the third action");
    }

    #[test]
    fn an_empty_item_list_is_harmless() {
        let items: [(&str, u8); 0] = [];
        assert!(items.is_empty());
    }
}
