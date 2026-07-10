//! slide_menu — an egui-native replacement for QML's `GestureHost` + `SlidePopup`.
//!
//! The QML pair was a press-hold → grid-menu → drag-to-highlight → release-to-select gesture picker,
//! split across two files only because Qt needed one object to own the input grab and hand lifecycle
//! calls (show/update/commit/cancel) to a passive visualizer — a plumbing problem egui's immediate mode
//! simply doesn't have. Here it's ONE widget: give it the host's [`egui::Response`] and a set of
//! options; while the host is pressed the grid opens near it, the pointer highlights a cell, and the
//! release commits the highlighted option (or cancels). No input-grab juggling, no reparenting.
//!
//! Every app with a "hold a thing to reveal a few choices" affordance can use it: a keyboard key's
//! long-press variants (rusty-board), a toolbar/tab action menu (voiceterm), etc.
//!
//! The grid geometry + hit-testing are pure and unit-tested; only drawing + input needs egui.

use egui::{Align2, Color32, CornerRadius, FontId, Id, Order, Pos2, Rect, Response, Stroke, StrokeKind, Ui, Vec2};

/// Grid dimensions (cols, rows) for `n` items capped at `max_cols` columns.
pub fn grid_dims(n: usize, max_cols: usize) -> (usize, usize) {
    if n == 0 {
        return (0, 0);
    }
    let cols = n.min(max_cols.max(1));
    let rows = n.div_ceil(cols);
    (cols, rows)
}

/// The square rect for each of `n` cells laid out in a `cols`-wide grid whose top-left is `origin`.
pub fn cell_rects(origin: Pos2, n: usize, cols: usize, item: f32, spacing: f32) -> Vec<Rect> {
    let cols = cols.max(1);
    (0..n)
        .map(|i| {
            let (r, c) = (i / cols, i % cols);
            let x = origin.x + c as f32 * (item + spacing);
            let y = origin.y + r as f32 * (item + spacing);
            Rect::from_min_size(egui::pos2(x, y), Vec2::splat(item))
        })
        .collect()
}

/// Index of the cell containing `p`, if any.
pub fn hit(p: Pos2, cells: &[Rect]) -> Option<usize> {
    cells.iter().position(|r| r.contains(p))
}

/// One choice in the menu. `id` is the opaque value returned on select; `label` is the glyph/text.
#[derive(Clone, Debug)]
pub struct SlideOption {
    pub id: String,
    pub label: String,
}

impl SlideOption {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { id: id.into(), label: label.into() }
    }
}

/// The result of a frame of [`SlideMenu::show`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlideOutcome {
    /// Nothing resolved this frame (menu closed, or open and still dragging).
    Idle,
    /// Released without a highlighted cell.
    Cancelled,
    /// Released over a cell — its `id`.
    Selected(String),
}

/// Per-gesture state (open + anchor). `Clone` so it can live in egui memory for stateless callers.
#[derive(Clone, Copy, Default)]
struct SlideState {
    open: bool,
    origin: Pos2,
    /// When the current press began (for the long-press delay); `None` when not pressed.
    pressed_at: Option<f64>,
}

/// A reusable gesture grid menu. Hold one instance per host affordance, OR use
/// [`SlideMenu::show_stored`] to keep state in egui memory (for per-item renderers like a keyboard).
pub struct SlideMenu {
    pub max_cols: usize,
    pub item_size: f32,
    pub spacing: f32,
    /// Seconds the host must be held before the grid opens. `0.0` = open immediately (a press-drag
    /// menu, the original GestureHost feel); ~`0.3` = long-press (so a quick tap still does the host's
    /// normal action, e.g. typing the key).
    pub hold_secs: f32,
    state: SlideState,
    id: Id,
}

impl SlideMenu {
    pub fn new(id_source: impl std::hash::Hash) -> Self {
        Self { max_cols: 3, item_size: 52.0, spacing: 8.0, hold_secs: 0.0, state: SlideState::default(), id: Id::new(id_source) }
    }
    pub fn columns(mut self, cols: usize) -> Self {
        self.max_cols = cols.max(1);
        self
    }
    pub fn item_size(mut self, px: f32) -> Self {
        self.item_size = px;
        self
    }
    /// Require a long-press of `secs` before opening (so a quick tap does the host's normal action).
    pub fn hold(mut self, secs: f32) -> Self {
        self.hold_secs = secs;
        self
    }
    /// True while the grid is showing.
    pub fn is_open(&self) -> bool {
        self.state.open
    }

    /// Drive the menu from a host widget's `response`. Call every frame. Returns the outcome; act on
    /// `Selected(id)` and stop reading the host for that gesture.
    pub fn show(&mut self, ui: &mut Ui, host: &Response, options: &[SlideOption]) -> SlideOutcome {
        let id = self.id;
        run(self.max_cols, self.item_size, self.spacing, self.hold_secs, id, &mut self.state, ui, host, options)
    }

    /// Memory-backed variant for stateless per-item renderers (a keyboard key's long-press variants, a
    /// per-row action button): state lives in egui memory keyed by `id`, so the caller needn't hold a
    /// `SlideMenu`. Configure columns/size on `self`; `self`'s own state field is unused here.
    pub fn show_stored(&self, ui: &mut Ui, id: Id, host: &Response, options: &[SlideOption]) -> SlideOutcome {
        let mut st = ui.ctx().data_mut(|d| d.get_temp::<SlideState>(id).unwrap_or_default());
        let out = run(self.max_cols, self.item_size, self.spacing, self.hold_secs, id, &mut st, ui, host, options);
        ui.ctx().data_mut(|d| d.insert_temp(id, st));
        out
    }
}

/// The shared gesture logic used by both the owned and memory-backed entry points.
#[allow(clippy::too_many_arguments)]
fn run(
    max_cols: usize,
    item_size: f32,
    spacing: f32,
    hold_secs: f32,
    id: Id,
    st: &mut SlideState,
    ui: &mut Ui,
    host: &Response,
    options: &[SlideOption],
) -> SlideOutcome {
    if options.is_empty() {
        return SlideOutcome::Idle;
    }
    let (cols, _rows) = grid_dims(options.len(), max_cols);

    // Open once the host has been held past `hold_secs` (immediately when it's 0).
    if host.is_pointer_button_down_on() {
        let now = ui.input(|i| i.time);
        let start = *st.pressed_at.get_or_insert(now);
        if !st.open && (now - start) >= hold_secs as f64 {
            st.open = true;
            let grid_w = cols as f32 * item_size + cols.saturating_sub(1) as f32 * spacing;
            st.origin = egui::pos2(host.rect.center().x - grid_w / 2.0, host.rect.top() - item_size - 12.0);
        }
    } else {
        st.pressed_at = None;
    }
    if !st.open {
        return SlideOutcome::Idle;
    }
    ui.ctx().request_repaint(); // poll the hold timer / follow the pointer

    let cells = cell_rects(st.origin, options.len(), cols, item_size, spacing);
    let pointer = ui.ctx().pointer_latest_pos();
    let hot = pointer.and_then(|p| hit(p, &cells));

    egui::Area::new(id).order(Order::Foreground).fixed_pos(st.origin).show(ui.ctx(), |ui| {
        let vis = ui.visuals().clone();
        let painter = ui.painter();
        let radius = CornerRadius::same(8);
        for (i, (opt, rect)) in options.iter().zip(&cells).enumerate() {
            let hot_i = hot == Some(i);
            let fill = if hot_i { vis.selection.bg_fill } else { vis.extreme_bg_color };
            painter.rect_filled(*rect, radius, fill);
            painter.rect_stroke(*rect, radius, Stroke::new(1.0, vis.widgets.noninteractive.bg_stroke.color), StrokeKind::Inside);
            let text_col = if hot_i { Color32::WHITE } else { vis.text_color() };
            painter.text(rect.center(), Align2::CENTER_CENTER, &opt.label, FontId::proportional(18.0), text_col);
        }
    });
    ui.ctx().request_repaint();

    if ui.input(|i| i.pointer.any_released()) {
        st.open = false;
        st.pressed_at = None;
        return match hot {
            Some(i) => SlideOutcome::Selected(options[i].id.clone()),
            None => SlideOutcome::Cancelled,
        };
    }
    SlideOutcome::Idle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_dims_caps_columns_and_wraps_rows() {
        assert_eq!(grid_dims(0, 3), (0, 0));
        assert_eq!(grid_dims(2, 3), (2, 1));
        assert_eq!(grid_dims(3, 3), (3, 1));
        assert_eq!(grid_dims(5, 3), (3, 2)); // 5 items, 3 cols → 2 rows
        assert_eq!(grid_dims(6, 3), (3, 2));
        assert_eq!(grid_dims(4, 100), (4, 1));
    }

    #[test]
    fn cells_lay_out_left_to_right_top_to_bottom() {
        let cells = cell_rects(Pos2::ZERO, 4, 2, 50.0, 10.0);
        assert_eq!(cells.len(), 4);
        assert_eq!(cells[0].min, egui::pos2(0.0, 0.0));
        assert_eq!(cells[1].min, egui::pos2(60.0, 0.0)); // next column: 50 + 10
        assert_eq!(cells[2].min, egui::pos2(0.0, 60.0)); // next row
        assert_eq!(cells[3].min, egui::pos2(60.0, 60.0));
        assert_eq!(cells[0].size(), Vec2::splat(50.0));
    }

    #[test]
    fn hit_finds_the_cell_under_the_pointer() {
        let cells = cell_rects(Pos2::ZERO, 4, 2, 50.0, 10.0);
        assert_eq!(hit(egui::pos2(25.0, 25.0), &cells), Some(0));
        assert_eq!(hit(egui::pos2(85.0, 85.0), &cells), Some(3));
        assert_eq!(hit(egui::pos2(55.0, 5.0), &cells), None); // in the gutter between columns
        assert_eq!(hit(egui::pos2(500.0, 500.0), &cells), None);
    }
}
