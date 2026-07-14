//! split_pane — a resizable two-pane divider (vertical or horizontal split), shared across any
//! app with a multi-pane layout: voiceterm's terminal-beside-a-web-pane, logscope's Files/Graph/
//! Context panels, gitmind's push-page (terminal | rendered web content, seen in its own
//! reference screenshots), av1tuner-m1's multi-pane editor, unendlich's analytics dashboards.
//! Every one of these needs the same thing — two regions sharing a rect, a draggable divider
//! between them, and a remembered split fraction — so it belongs here once, not reimplemented
//! per app.
//!
//! Kept as a small persistent-state struct (not a one-shot function) because the fraction needs
//! to survive across frames; the caller owns one `SplitPane` per divider and calls `.show()`
//! each frame with closures for the two panes' content.

use egui::{Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    /// Divider runs vertically; panes sit side by side (left | right).
    Vertical,
    /// Divider runs horizontally; panes stack (top / bottom).
    Horizontal,
}

/// A resizable two-pane split. `fraction` is how much of the total space the FIRST pane gets
/// (0..1), clamped to `[min_fraction, max_fraction]` so neither pane can be dragged to nothing.
pub struct SplitPane {
    pub orientation: Orientation,
    pub fraction: f32,
    pub min_fraction: f32,
    pub max_fraction: f32,
    /// Divider thickness in points — also the hit-test width for the drag handle.
    pub divider_width: f32,
}

impl SplitPane {
    pub fn new(orientation: Orientation, initial_fraction: f32) -> Self {
        Self {
            orientation,
            fraction: initial_fraction.clamp(0.0, 1.0),
            min_fraction: 0.15,
            max_fraction: 0.85,
            divider_width: 6.0,
        }
    }

    /// Split `total` into (first_rect, divider_rect, second_rect) at the current fraction —
    /// pure geometry, no egui interaction, so it's directly testable without a live `Ui`.
    pub fn split_rect(&self, total: Rect) -> (Rect, Rect, Rect) {
        let f = self.fraction.clamp(self.min_fraction, self.max_fraction);
        match self.orientation {
            Orientation::Vertical => {
                let split_x = total.left() + total.width() * f;
                let first = Rect::from_min_max(total.left_top(), egui::pos2(split_x - self.divider_width * 0.5, total.bottom()));
                let divider = Rect::from_min_max(egui::pos2(split_x - self.divider_width * 0.5, total.top()), egui::pos2(split_x + self.divider_width * 0.5, total.bottom()));
                let second = Rect::from_min_max(egui::pos2(split_x + self.divider_width * 0.5, total.top()), total.right_bottom());
                (first, divider, second)
            }
            Orientation::Horizontal => {
                let split_y = total.top() + total.height() * f;
                let first = Rect::from_min_max(total.left_top(), egui::pos2(total.right(), split_y - self.divider_width * 0.5));
                let divider = Rect::from_min_max(egui::pos2(total.left(), split_y - self.divider_width * 0.5), egui::pos2(total.right(), split_y + self.divider_width * 0.5));
                let second = Rect::from_min_max(egui::pos2(total.left(), split_y + self.divider_width * 0.5), total.right_bottom());
                (first, divider, second)
            }
        }
    }

    /// Update `fraction` from a divider drag delta (in points) against `total`'s size — pure,
    /// so the drag math is testable without simulating real pointer events.
    pub fn drag(&mut self, delta: Vec2, total_size: Vec2) {
        let (delta_along, extent) = match self.orientation {
            Orientation::Vertical => (delta.x, total_size.x),
            Orientation::Horizontal => (delta.y, total_size.y),
        };
        if extent <= 0.0 { return; }
        self.fraction = (self.fraction + delta_along / extent).clamp(self.min_fraction, self.max_fraction);
    }

    /// Render both panes with a draggable divider between them. `first`/`second` are given a
    /// child `Ui` scoped to their pane's rect; the divider itself is drawn as a thin bar that
    /// highlights on hover/drag.
    pub fn show(&mut self, ui: &mut Ui, accent: Color32, border: Color32, first: impl FnOnce(&mut Ui), second: impl FnOnce(&mut Ui)) {
        let total = ui.available_rect_before_wrap();
        let (first_rect, divider_rect, second_rect) = self.split_rect(total);

        let mut first_ui = ui.new_child(egui::UiBuilder::new().max_rect(first_rect));
        first(&mut first_ui);

        let resp = ui.interact(divider_rect, ui.id().with("split_divider"), Sense::click_and_drag());
        if resp.dragged() {
            self.drag(resp.drag_delta(), total.size());
        }
        let fill = if resp.dragged() { accent } else if resp.hovered() { accent.gamma_multiply(0.6) } else { border };
        ui.painter().rect_filled(divider_rect, CornerRadius::same(2), fill);
        if resp.hovered() || resp.dragged() {
            let cursor = match self.orientation {
                Orientation::Vertical => egui::CursorIcon::ResizeHorizontal,
                Orientation::Horizontal => egui::CursorIcon::ResizeVertical,
            };
            ui.ctx().set_cursor_icon(cursor);
        }

        let mut second_ui = ui.new_child(egui::UiBuilder::new().max_rect(second_rect));
        second(&mut second_ui);
    }
}

/// Paint just a divider's visual (no interaction) — for callers that already have their own
/// drag handling and only want the shared look.
pub fn paint_divider(ui: &Ui, rect: Rect, color: Color32) {
    ui.painter().rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, color), StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total() -> Rect { Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(1000.0, 600.0)) }

    #[test]
    fn vertical_split_divides_at_the_fraction() {
        let sp = SplitPane::new(Orientation::Vertical, 0.5);
        let (first, divider, second) = sp.split_rect(total());
        assert!((first.width() - 500.0 + sp.divider_width * 0.5).abs() < 1.0);
        assert!((second.left() - divider.right()).abs() < 1e-3);
        assert_eq!(first.height(), 600.0);
        assert_eq!(second.height(), 600.0);
    }

    #[test]
    fn horizontal_split_divides_at_the_fraction() {
        let sp = SplitPane::new(Orientation::Horizontal, 0.3);
        let (first, divider, second) = sp.split_rect(total());
        assert!((first.height() - 180.0 + sp.divider_width * 0.5).abs() < 1.0);
        assert!((second.top() - divider.bottom()).abs() < 1e-3);
        assert_eq!(first.width(), 1000.0);
    }

    #[test]
    fn fraction_is_clamped_to_min_max_not_just_0_1() {
        let mut sp = SplitPane::new(Orientation::Vertical, 0.5);
        sp.min_fraction = 0.2;
        sp.max_fraction = 0.8;
        sp.fraction = 0.05; // below min
        let (first, _, _) = sp.split_rect(total());
        assert!((first.width() - 1000.0 * 0.2).abs() < 5.0, "should clamp to min_fraction, not use the raw 0.05");
        sp.fraction = 0.95; // above max
        let (first2, _, _) = sp.split_rect(total());
        assert!((first2.width() - 1000.0 * 0.8).abs() < 5.0, "should clamp to max_fraction");
    }

    #[test]
    fn drag_moves_the_fraction_proportionally_and_stays_clamped() {
        let mut sp = SplitPane::new(Orientation::Vertical, 0.5);
        sp.drag(Vec2::new(100.0, 0.0), Vec2::new(1000.0, 600.0)); // +10% of width
        assert!((sp.fraction - 0.6).abs() < 1e-3);
        // dragging far past max_fraction should clamp, not overshoot
        sp.drag(Vec2::new(10000.0, 0.0), Vec2::new(1000.0, 600.0));
        assert_eq!(sp.fraction, sp.max_fraction);
    }

    #[test]
    fn horizontal_drag_ignores_the_perpendicular_axis() {
        let mut sp = SplitPane::new(Orientation::Horizontal, 0.5);
        sp.drag(Vec2::new(500.0, 0.0), Vec2::new(1000.0, 600.0)); // x-delta shouldn't move a horizontal split
        assert_eq!(sp.fraction, 0.5);
        sp.drag(Vec2::new(0.0, 60.0), Vec2::new(1000.0, 600.0)); // +10% of height
        assert!((sp.fraction - 0.6).abs() < 1e-3);
    }

    #[test]
    fn zero_extent_drag_is_a_no_op_not_a_divide_by_zero() {
        let mut sp = SplitPane::new(Orientation::Vertical, 0.5);
        sp.drag(Vec2::new(50.0, 0.0), Vec2::new(0.0, 600.0));
        assert_eq!(sp.fraction, 0.5, "a collapsed total width must not panic or NaN the fraction");
    }

    #[test]
    fn new_clamps_an_out_of_range_initial_fraction() {
        let sp = SplitPane::new(Orientation::Vertical, 1.5);
        assert_eq!(sp.fraction, 1.0);
        let sp2 = SplitPane::new(Orientation::Vertical, -0.5);
        assert_eq!(sp2.fraction, 0.0);
    }
}
