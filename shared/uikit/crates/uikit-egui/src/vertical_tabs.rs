//! vertical_tabs.rs — a thin vertical sidebar of tab buttons for switching between a small number of
//! pages/layouts, sized for touch and themed from the active palette.
//!
//! Ported out of rusty-board: the keyboard used to switch layouts (qwerty ↔ symbols ↔ …) via a whole-
//! board horizontal swipe (`GestureLayer::horizontal_page`) — but that surface reads the SAME raw
//! pointer trajectory that per-key cardinal swipes (`slide_menu::show_cardinal`) also read, with nothing
//! arbitrating between them: a swipe meant for one key's compass could also get interpreted as a page
//! change, and — because per-key cardinal state is keyed by the key's on-screen rect — a layout change
//! mid-swipe orphans that state (the id no longer matches anything drawn next frame). That's the
//! intermittent "swipe doesn't register" failure. A thin sidebar of discrete tap targets has no such
//! race: switching pages is a tap on ITS OWN area, never contested with a key's gesture space, so this
//! bug class doesn't exist for a sidebar. General enough to reuse anywhere a small fixed set of "pages"
//! (keyboard layouts, voiceterm panes, …) needs switching without swipe/tap ambiguity.

use egui::{Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};

/// One tab's content — the label is what's drawn; `hint` (optional) is a hover tooltip for a
/// dense/abbreviated label (e.g. a single glyph on a very narrow sidebar).
pub struct Tab<'a> {
    pub label: &'a str,
    pub hint: Option<&'a str>,
}
impl<'a> Tab<'a> {
    pub fn new(label: &'a str) -> Self { Self { label, hint: None } }
    pub fn with_hint(label: &'a str, hint: &'a str) -> Self { Self { label, hint: Some(hint) } }
}

/// Render a vertical strip of `tabs.len()` stacked buttons filling `ui`'s available height, the
/// `selected` index highlighted. Returns `Some(i)` the frame a DIFFERENT tab is clicked (so the caller
/// can commit the page change), `None` otherwise — mirrors the click-driven contract of a normal button
/// row, just laid out vertically and pre-themed.
///
/// `min_touch: f32` is the minimum per-tab height (pass `Adaptive::min_touch_target` so it grows on
/// touch devices, tightens on desktop, same convention as the rest of the kit).
pub fn show(ui: &mut Ui, tabs: &[Tab], selected: usize, min_touch: f32, colors: &crate::PaletteColors) -> Option<usize> {
    let width = ui.available_width();
    let height = ui.available_height();
    let n = tabs.len().max(1);
    let tab_h = (height / n as f32).max(min_touch);
    let mut clicked = None;
    ui.vertical(|ui| {
        for (i, tab) in tabs.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, tab_h), Sense::click());
            let on = i == selected;
            let fill = if on { colors.accent } else if resp.hovered() { colors.bg_hover } else { colors.bg_raised };
            painter_tab(ui, rect, fill, colors.border, on);
            let text_color = if on { Color32::WHITE } else { colors.text };
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, tab.label,
                egui::FontId::proportional((tab_h * 0.32).clamp(10.0, 18.0)), text_color);
            if let Some(hint) = tab.hint { resp.clone().on_hover_text(hint); }
            if resp.clicked() && !on { clicked = Some(i); }
        }
    });
    clicked
}

fn painter_tab(ui: &Ui, rect: Rect, fill: Color32, border: Color32, active: bool) {
    let rounding = CornerRadius::same(4);
    ui.painter().rect(rect.shrink(1.5), rounding, fill, Stroke::new(1.0, border), StrokeKind::Inside);
    if active {
        // a left accent bar, the common "which tab is active" cue in a vertical rail
        let bar = Rect::from_min_max(rect.min, egui::pos2(rect.min.x + 3.0, rect.max.y));
        ui.painter().rect_filled(bar, 0, border);
    }
}

#[cfg(test)]
mod tests {
    // Pure-logic tests for the bits that don't need a live egui::Ui: the index arithmetic a caller
    // relies on (which tab is "next"/"prev" — used for optional keyboard/voice paging alongside taps).
    #[test]
    fn wrap_next_and_prev_stay_in_bounds() {
        let n = 4usize;
        let next = |i: usize| (i + 1) % n;
        let prev = |i: usize| (i + n - 1) % n;
        assert_eq!(next(3), 0, "wraps forward past the last tab");
        assert_eq!(prev(0), 3, "wraps backward past the first tab");
        assert_eq!(next(0), 1);
        assert_eq!(prev(1), 0);
    }
}
