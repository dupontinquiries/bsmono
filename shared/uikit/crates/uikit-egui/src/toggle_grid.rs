//! toggle_grid — a hold-to-open, swipe-to-toggle grid, for flipping a set of flags in one gesture
//! (gitmind's build flags, feature switches, …). Android swipe-unlock crossed with the shared gesture
//! popup: press-hold the host to open a grid of on/off cells, drag across cells to flip each one you
//! enter (each cell toggles once per gesture, like connecting unlock dots), and release to confirm.
//!
//! It reuses `slide_menu`'s grid geometry and the `motion` kit (spring pop-in + gliding cell colour).
//! The "toggle once on enter" logic is pure and unit-tested.

use crate::motion;
use crate::slide_menu::{cell_rects, grid_dims, hit};
use egui::{Align2, Color32, CornerRadius, FontId, Id, Order, Pos2, Rect, Response, Stroke, StrokeKind, Ui, Vec2};
use std::collections::HashSet;

/// One flag cell.
#[derive(Clone, Debug)]
pub struct ToggleOption {
    pub id: String,
    pub label: String,
    pub on: bool,
}

impl ToggleOption {
    pub fn new(id: impl Into<String>, label: impl Into<String>, on: bool) -> Self {
        Self { id: id.into(), label: label.into(), on }
    }
}

/// Result of a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToggleOutcome {
    /// Open and dragging, or closed — nothing to apply this frame.
    Idle,
    /// Released — the final `(id, on)` state of every cell.
    Committed(Vec<(String, bool)>),
}

/// Flip `states[i]` the first time the pointer enters cell `i` in this gesture. Pure — the tested core
/// of the swipe-toggle mechanic (each cell toggles at most once per drag).
pub fn toggle_on_enter(states: &mut [bool], visited: &mut HashSet<usize>, hot: Option<usize>) {
    if let Some(i) = hot {
        if i < states.len() && visited.insert(i) {
            states[i] = !states[i];
        }
    }
}

/// A reusable swipe-toggle grid. Hold one per host affordance.
pub struct ToggleGrid {
    pub max_cols: usize,
    pub item_size: f32,
    pub spacing: f32,
    open: bool,
    origin: Pos2,
    states: Vec<bool>,
    visited: HashSet<usize>,
    id: Id,
}

impl ToggleGrid {
    pub fn new(id_source: impl std::hash::Hash) -> Self {
        Self {
            max_cols: 3,
            item_size: 64.0,
            spacing: 8.0,
            open: false,
            origin: Pos2::ZERO,
            states: Vec::new(),
            visited: HashSet::new(),
            id: Id::new(id_source),
        }
    }
    pub fn columns(mut self, cols: usize) -> Self {
        self.max_cols = cols.max(1);
        self
    }
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Drive from the host's `response`. Press-hold opens the grid seeded with `options`' current
    /// states; dragging across cells flips each one entered; release commits the final states.
    pub fn show(&mut self, ui: &mut Ui, host: &Response, options: &[ToggleOption]) -> ToggleOutcome {
        if options.is_empty() {
            return ToggleOutcome::Idle;
        }
        let (cols, _rows) = grid_dims(options.len(), self.max_cols);

        // Open on press: seed the working states from the options and reset the visited set.
        if !self.open && host.is_pointer_button_down_on() {
            self.open = true;
            self.states = options.iter().map(|o| o.on).collect();
            self.visited.clear();
            let grid_w = cols as f32 * self.item_size + cols.saturating_sub(1) as f32 * self.spacing;
            self.origin = egui::pos2(host.rect.center().x - grid_w / 2.0, host.rect.top() - self.item_size - 12.0);
        }
        if !self.open {
            return ToggleOutcome::Idle;
        }

        let cells = cell_rects(self.origin, options.len(), cols, self.item_size, self.spacing);
        let hot = ui.ctx().pointer_latest_pos().and_then(|p| hit(p, &cells));
        // flip the entered cell (once per gesture)
        toggle_on_enter(&mut self.states, &mut self.visited, hot);

        let appear = motion::animate_eased(ui.ctx(), self.id.with("appear"), self.open, 0.14, motion::ease::out_back);
        let states = self.states.clone();
        let item = self.item_size;
        let gid = self.id;
        egui::Area::new(self.id).order(Order::Foreground).fixed_pos(self.origin).show(ui.ctx(), |ui| {
            let vis = ui.visuals().clone();
            let ctx = ui.ctx().clone();
            let painter = ui.painter();
            let cr = CornerRadius::same(10);
            for (i, (opt, rect)) in options.iter().zip(&cells).enumerate() {
                let on = states.get(i).copied().unwrap_or(opt.on);
                let hot_i = hot == Some(i);
                let rect = Rect::from_center_size(rect.center(), rect.size() * appear.max(0.01));
                // fill glides between off (dim) and on (accent); the entered cell brightens
                let target = if on { vis.selection.bg_fill } else { vis.extreme_bg_color };
                let mut fill = motion::animate_color(&ctx, gid.with(("cell", i)), target, 0.12);
                if hot_i {
                    fill = fill.gamma_multiply(1.25);
                }
                painter.rect_filled(rect, cr, fill);
                let stroke_col = if on { vis.selection.stroke.color } else { vis.widgets.noninteractive.bg_stroke.color };
                painter.rect_stroke(rect, cr, Stroke::new(if hot_i { 2.0 } else { 1.0 }, stroke_col), StrokeKind::Inside);
                let text_col = if on { Color32::WHITE } else { vis.weak_text_color() };
                // label + a small on/off mark
                let center = rect.center();
                painter.text(center - Vec2::new(0.0, 7.0), Align2::CENTER_CENTER, &opt.label, FontId::proportional(13.0), text_col);
                painter.text(center + Vec2::new(0.0, 10.0), Align2::CENTER_CENTER, if on { "on" } else { "off" }, FontId::proportional(11.0), text_col);
            }
        });
        ui.ctx().request_repaint();

        if ui.input(|i| i.pointer.any_released()) {
            self.open = false;
            let out: Vec<(String, bool)> = options
                .iter()
                .enumerate()
                .map(|(i, o)| (o.id.clone(), self.states.get(i).copied().unwrap_or(o.on)))
                .collect();
            return ToggleOutcome::Committed(out);
        }
        ToggleOutcome::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_cell_toggles_at_most_once_per_gesture() {
        let mut states = vec![false, false, true];
        let mut visited = HashSet::new();
        // enter cell 0 → flips to true
        toggle_on_enter(&mut states, &mut visited, Some(0));
        assert_eq!(states, vec![true, false, true]);
        // stay on cell 0 (already visited) → no change
        toggle_on_enter(&mut states, &mut visited, Some(0));
        assert_eq!(states, vec![true, false, true]);
        // enter cell 2 → flips true→false
        toggle_on_enter(&mut states, &mut visited, Some(2));
        assert_eq!(states, vec![true, false, false]);
        // pointer off any cell → nothing
        toggle_on_enter(&mut states, &mut visited, None);
        assert_eq!(states, vec![true, false, false]);
    }

    #[test]
    fn out_of_range_hot_is_ignored() {
        let mut states = vec![false];
        let mut visited = HashSet::new();
        toggle_on_enter(&mut states, &mut visited, Some(9));
        assert_eq!(states, vec![false]);
    }
}
