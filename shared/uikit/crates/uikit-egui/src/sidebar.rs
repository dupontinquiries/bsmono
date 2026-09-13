//! sidebar — a side column whose width belongs to the user, not to its text.
//!
//! ## The fault this replaces
//!
//! `egui::Panel::left(..).resizable(true)` stores, at the end of every
//! frame, the rect its FRAME ended up with — and that frame is as wide as
//! the widest thing inside it (panel.rs: `let rect =
//! inner_response.response.rect; … PanelState { rect }.store(..)`). A row
//! that is 30pt too wide for the column therefore makes the column 30pt
//! wider next frame; a label that wrapped at the old width now fits on one
//! line, so the content is a different width again, and the column
//! re-settles somewhere else. Drag it narrower and the overflowing row puts
//! it straight back. Twelve apps in this tree have a column like that, and
//! the `panel_width_behaviour` tests in lib.rs already record that
//! `default_width` is only a hint.
//!
//! ## The rule here
//!
//! **Content is laid out INTO the width; the width is never read FROM the
//! content.** Concretely:
//!
//! * The column's width is a field on [`Sidebar`], changed only by the
//!   resize handle, the collapse toggle, `reset`, or the caller. Every
//!   frame it is handed to egui as an EXACT width, which pins the panel.
//! * Inside, the content runs in a detached child `Ui` whose max width is
//!   the column's inner width. Its used HEIGHT is allocated back (so the
//!   column scrolls); its used WIDTH is discarded. A row that is too wide
//!   is clipped at the edge and the column does not move. The scroll area's
//!   own width therefore equals the column's, and nothing upstream can
//!   grow.
//! * Text wraps at that inner width and at nothing else. The width is
//!   rounded to whole points, so a label never sits on a half-pixel
//!   boundary deciding between one line and two.
//! * The handle tracks the pointer's absolute position (not accumulated
//!   deltas, which drift), reads it BEFORE the panel is drawn so there is
//!   no one-frame lag, and clamps to `[min_width, ceiling]` where the
//!   ceiling keeps `reserve` points for whatever sits beside the column.
//! * Drag past half the minimum and the column collapses to a rail with a
//!   button to bring it back; drag the rail out past the minimum and it
//!   expands. Hysteresis, so a hand hovering at the threshold does not
//!   flicker. Double-click the handle to return to the default width.
//!
//! The geometry and the clamping are pure and tested on their own; the
//! egui tests below run headless frames with deliberately oversized content
//! and assert the column does not move — and that `egui::Panel` does, so a
//! future egui that fixes this is noticed rather than worked around twice.
//!
//! ## Use
//!
//! ```ignore
//! // Owned by the app, like SplitPane. Loads/saves its width in egui's
//! // memory under `id`, so it survives a restart when the app persists.
//! let mut bar = Sidebar::new(Side::Left, 250.0).min_width(180.0).reserve(240.0);
//! bar.show_inside(ui, "controls", |ui| { /* the column */ });
//! egui::CentralPanel::default().show_inside(ui, |ui| { /* the rest */ });
//! ```
//!
//! `two_pane` uses it for its side-by-side arrangement, so bscale and
//! beatspace get this without touching their call sites.

use egui::{Id, Rect, Sense, TextWrapMode, Ui, UiBuilder, Vec2};

/// Which edge the column hangs from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// A side column. Plain state; the caller owns one per column and calls
/// [`Sidebar::show_inside`] each frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Sidebar {
    pub side: Side,
    /// The width the user has chosen, in points. The ONLY source of the
    /// column's width; content never writes it.
    pub width: f32,
    pub default_width: f32,
    /// Narrower than this and the column collapses instead.
    pub min_width: f32,
    /// Ceiling as a fraction of the available width.
    pub max_fraction: f32,
    /// Points the neighbour must keep, whatever the fraction says. `two_pane`
    /// passes its content minimum here.
    pub reserve: f32,
    /// Collapsed to a rail.
    pub collapsed: bool,
    /// Width of the rail when collapsed. The rail holds the expand button;
    /// 36 is that button inside the panel frame's margins.
    pub rail_width: f32,
    pub resizable: bool,
    /// Load the width from egui memory on the first show and store it every
    /// frame after. Off for a column the caller persists itself.
    pub remember: bool,
    loaded: bool,
}

/// What a frame did with the column.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarResponse {
    /// The column's rect, rail included.
    pub rect: Rect,
    pub collapsed: bool,
    /// The user changed the width or collapsed/expanded it this frame.
    pub changed: bool,
}

impl Sidebar {
    pub fn new(side: Side, default_width: f32) -> Self {
        Self {
            side,
            width: default_width,
            default_width,
            min_width: (default_width * 0.5).max(48.0),
            max_fraction: 0.6,
            reserve: 0.0,
            collapsed: false,
            rail_width: 36.0,
            resizable: true,
            remember: true,
            loaded: false,
        }
    }

    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w.max(0.0);
        self
    }
    pub fn max_fraction(mut self, f: f32) -> Self {
        self.max_fraction = f.clamp(0.05, 1.0);
        self
    }
    pub fn reserve(mut self, points: f32) -> Self {
        self.reserve = points.max(0.0);
        self
    }
    pub fn rail_width(mut self, w: f32) -> Self {
        self.rail_width = w.max(0.0);
        self
    }
    pub fn resizable(mut self, r: bool) -> Self {
        self.resizable = r;
        self
    }
    pub fn remember(mut self, r: bool) -> Self {
        self.remember = r;
        self
    }
    pub fn collapsed(mut self, c: bool) -> Self {
        self.collapsed = c;
        self
    }

    // ---------------------------------------------------------------- pure

    /// The widest the column may be beside `avail_w` of space.
    pub fn ceiling(&self, avail_w: f32) -> f32 {
        let avail = avail_w.max(0.0);
        (avail * self.max_fraction).min(avail - self.reserve).max(0.0)
    }

    /// The width the column is drawn at this frame. Whole points.
    pub fn shown_width(&self, avail_w: f32) -> f32 {
        let avail = avail_w.max(0.0);
        let w = if self.collapsed {
            self.rail_width
        } else {
            self.width.clamp(self.min_width, self.ceiling(avail).max(self.min_width))
        };
        w.min(avail).max(0.0).round()
    }

    /// The x of the column's inner edge — where the handle lives — when
    /// the available rect is `avail`.
    pub fn edge_x(&self, avail: Rect) -> f32 {
        let w = self.shown_width(avail.width());
        match self.side {
            Side::Left => avail.left() + w,
            Side::Right => avail.right() - w,
        }
    }

    /// Set the width from the pointer's absolute x. Returns whether
    /// anything changed. Past half the minimum the column collapses; out
    /// past the minimum a collapsed column expands.
    pub fn drag_to(&mut self, pointer_x: f32, avail: Rect) -> bool {
        let want = match self.side {
            Side::Left => pointer_x - avail.left(),
            Side::Right => avail.right() - pointer_x,
        };
        let before = (self.width, self.collapsed);
        if self.collapsed {
            if want >= self.min_width {
                self.collapsed = false;
                self.width = want.min(self.ceiling(avail.width()).max(self.min_width));
            }
        } else if want < self.min_width * 0.5 {
            self.collapsed = true;
            // Keep the last real width so expanding returns to it.
            self.width = self.width.max(self.min_width);
        } else {
            self.width = want.clamp(self.min_width, self.ceiling(avail.width()).max(self.min_width));
        }
        self.width = self.width.round();
        (self.width, self.collapsed) != before
    }

    pub fn toggle(&mut self) {
        self.collapsed = !self.collapsed;
    }

    pub fn reset(&mut self) {
        self.width = self.default_width;
        self.collapsed = false;
    }

    // -------------------------------------------------------------- memory

    fn mem_id(id: Id) -> (Id, Id) {
        (id.with("sidebar_width"), id.with("sidebar_collapsed"))
    }

    /// Read the width and collapsed state stored under `id`, if any.
    pub fn load(&mut self, ctx: &egui::Context, id: Id) {
        let (wid, cid) = Self::mem_id(id);
        ctx.data_mut(|d| {
            if let Some(w) = d.get_persisted::<f32>(wid) {
                if w.is_finite() && w > 0.0 {
                    self.width = w;
                }
            }
            if let Some(c) = d.get_persisted::<bool>(cid) {
                self.collapsed = c;
            }
        });
        self.loaded = true;
    }

    pub fn save(&self, ctx: &egui::Context, id: Id) {
        let (wid, cid) = Self::mem_id(id);
        ctx.data_mut(|d| {
            d.insert_persisted(wid, self.width);
            d.insert_persisted(cid, self.collapsed);
        });
    }

    // ---------------------------------------------------------------- egui

    /// Draw the column inside `ui`, then leave `ui`'s cursor beside it for
    /// whatever follows (a `CentralPanel::show_inside`, or plain widgets).
    pub fn show_inside<R>(
        &mut self,
        ui: &mut Ui,
        id: impl std::hash::Hash,
        content: impl FnOnce(&mut Ui) -> R,
    ) -> (SidebarResponse, Option<R>) {
        let id = Id::new(("uikit_sidebar", id));
        let handle_id = id.with("handle");
        let ctx = ui.ctx().clone();
        if self.remember && !self.loaded {
            self.load(&ctx, id);
        }

        let avail = ui.available_rect_before_wrap();
        let mut changed = false;

        // READ THE DRAG BEFORE DRAWING. egui does the same for its own
        // panels: the pointer's position this frame sets this frame's
        // width, so the edge stays under the finger.
        if self.resizable && ctx.is_being_dragged(handle_id) {
            if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
                changed |= self.drag_to(p.x, avail);
            }
        }

        let width = self.shown_width(avail.width());
        let collapsed = self.collapsed;
        let frame = egui::Frame::side_top_panel(ui.style());
        let panel = match self.side {
            Side::Left => egui::Panel::left(id),
            Side::Right => egui::Panel::right(id),
        }
        .resizable(false)
        .show_separator_line(false)
        .frame(frame)
        .exact_size(width);

        let side = self.side;
        let mut want_toggle = false;
        let inner = panel.show_inside(ui, |ui| {
            if collapsed {
                Self::rail(ui, id, side, &mut want_toggle);
                return None;
            }
            Some(Self::body(ui, id, content))
        });
        let panel_rect = inner.response.rect;

        // The handle, on the inner edge, interacted AFTER the content so a
        // scroll area or a slider at the edge cannot eat the drag.
        if self.resizable {
            let x = match side {
                Side::Left => panel_rect.right(),
                Side::Right => panel_rect.left(),
            };
            let grab = 8.0;
            let handle_rect = Rect::from_min_max(
                egui::pos2(x - grab * 0.5, panel_rect.top()),
                egui::pos2(x + grab * 0.5, panel_rect.bottom()),
            );
            let resp = ui.interact(handle_rect, handle_id, Sense::click_and_drag());
            if resp.double_clicked() {
                self.reset();
                changed = true;
            }
            if resp.hovered() || resp.dragged() {
                ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            let visuals = ui.style().visuals.clone();
            let stroke = if resp.dragged() {
                visuals.widgets.active.fg_stroke
            } else if resp.hovered() {
                visuals.widgets.hovered.fg_stroke
            } else {
                visuals.widgets.noninteractive.bg_stroke
            };
            ui.painter().vline(x, panel_rect.y_range(), stroke);
        } else {
            let x = match side {
                Side::Left => panel_rect.right(),
                Side::Right => panel_rect.left(),
            };
            ui.painter().vline(x, panel_rect.y_range(), ui.style().visuals.widgets.noninteractive.bg_stroke);
        }

        if want_toggle {
            self.toggle();
            changed = true;
        }
        if self.remember {
            // Every frame, not only on change: it is two map inserts, and
            // it is what makes a width chosen on one page still there after
            // a restart when the app persists egui's memory.
            self.save(&ctx, id);
        }

        (
            SidebarResponse { rect: panel_rect, collapsed, changed },
            inner.inner,
        )
    }

    /// A button that collapses or expands the column, for a toolbar the
    /// caller draws elsewhere.
    pub fn toggle_button(&mut self, ui: &mut Ui) -> egui::Response {
        let glyph = match (self.side, self.collapsed) {
            (Side::Left, false) | (Side::Right, true) => "‹",
            _ => "›",
        };
        let resp = ui.button(glyph).on_hover_text(if self.collapsed { "show the column" } else { "hide the column" });
        if resp.clicked() {
            self.toggle();
        }
        resp
    }

    fn rail(ui: &mut Ui, id: Id, side: Side, want_toggle: &mut bool) {
        Self::into_width(ui, id.with("rail"), |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(4.0);
                let glyph = match side {
                    Side::Left => "›",
                    Side::Right => "‹",
                };
                if ui.small_button(glyph).on_hover_text("show the column").clicked() {
                    *want_toggle = true;
                }
            });
        });
    }

    /// The content, laid out into the column's inner width and nothing
    /// else. See the module docs for why the child's width is discarded.
    fn body<R>(ui: &mut Ui, id: Id, content: impl FnOnce(&mut Ui) -> R) -> R {
        // Everything in the column wraps at the column. This includes
        // labels inside horizontal rows, which egui would otherwise let
        // run off the edge (and, in a resizable panel, push it).
        ui.style_mut().wrap_mode = Some(TextWrapMode::Wrap);
        // Sliders take a share of the column rather than a fixed 100pt
        // that either dwarfs a narrow column or rattles in a wide one.
        let w = ui.available_width();
        ui.spacing_mut().slider_width = (w * 0.5).clamp(48.0, 220.0);

        egui::ScrollArea::vertical()
            .id_salt(id.with("scroll"))
            .auto_shrink([false, false])
            .show(ui, |ui| Self::into_width(ui, id.with("body"), content))
            .inner
    }

    /// Run `f` in a child `Ui` exactly as wide as what `ui` has available,
    /// and hand back only the HEIGHT it used.
    ///
    /// `ui.available_width()` is read here, inside any scroll area, so a
    /// scroll bar that takes layout width is already subtracted. An
    /// over-wide row inside `f` is clipped by the panel and reports nothing
    /// upward, so the scroll area, the frame and the panel all end exactly
    /// as wide as the column. `advance_cursor_after_rect`, not
    /// `allocate_rect`: it moves the cursor and grows `min_rect` without
    /// registering a widget, so nothing sits on top of the column's own
    /// buttons.
    fn into_width<R>(ui: &mut Ui, id: Id, f: impl FnOnce(&mut Ui) -> R) -> R {
        let inner_w = ui.available_width().floor().max(1.0);
        let origin = ui.cursor().min;
        let max_rect = Rect::from_min_size(origin, Vec2::new(inner_w, f32::INFINITY));
        let mut child = ui.new_child(
            UiBuilder::new()
                .id_salt(id)
                .max_rect(max_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        let r = f(&mut child);
        let used_h = child.min_rect().height().max(0.0);
        ui.advance_cursor_after_rect(Rect::from_min_size(origin, Vec2::new(inner_w, used_h)));
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn avail() -> Rect {
        Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0))
    }

    #[test]
    fn the_width_is_clamped_to_the_minimum_the_fraction_and_the_reserve() {
        let mut s = Sidebar::new(Side::Left, 250.0).min_width(180.0).max_fraction(0.6).reserve(240.0);
        assert_eq!(s.shown_width(800.0), 250.0);
        s.width = 100.0;
        assert_eq!(s.shown_width(800.0), 180.0, "never under the minimum while open");
        s.width = 700.0;
        assert_eq!(s.shown_width(800.0), 480.0, "60% of 800");
        s.width = 700.0;
        assert_eq!(s.shown_width(600.0), 360.0, "600 - 240 reserved beats 60% of 600 = 360");
        assert_eq!(s.shown_width(500.0), 260.0, "500 - 240 reserved");
        // A screen too small for both: the minimum still wins over the
        // ceiling (two_pane stacks before this can happen).
        assert_eq!(s.shown_width(300.0), 180.0);
        assert_eq!(s.shown_width(100.0), 100.0, "and never wider than what exists");
    }

    #[test]
    fn widths_are_whole_points() {
        let mut s = Sidebar::new(Side::Left, 250.4);
        assert_eq!(s.shown_width(800.0), 250.0);
        s.width = 250.6;
        assert_eq!(s.shown_width(800.0), 251.0);
        s.drag_to(200.49, avail());
        assert_eq!(s.width, 200.0);
    }

    #[test]
    fn a_drag_tracks_the_pointer_absolutely_on_either_side() {
        let a = avail();
        let mut l = Sidebar::new(Side::Left, 250.0).min_width(100.0);
        assert!(l.drag_to(300.0, a));
        assert_eq!(l.width, 300.0);
        assert_eq!(l.edge_x(a), 300.0, "the edge is under the pointer");
        // Repeated drags to the same x change nothing: no drift.
        assert!(!l.drag_to(300.0, a));
        assert_eq!(l.width, 300.0);

        let mut r = Sidebar::new(Side::Right, 250.0).min_width(100.0);
        r.drag_to(500.0, a);
        assert_eq!(r.width, 300.0, "800 - 500");
        assert_eq!(r.edge_x(a), 500.0);
    }

    #[test]
    fn dragging_past_half_the_minimum_collapses_and_out_past_it_expands() {
        let a = avail();
        let mut s = Sidebar::new(Side::Left, 250.0).min_width(200.0);
        // Down to the minimum: pinned, open.
        s.drag_to(150.0, a);
        assert!(!s.collapsed);
        assert_eq!(s.width, 200.0);
        // Below half: collapses, and the width kept is the minimum so a
        // later expand does not open a sliver.
        assert!(s.drag_to(90.0, a));
        assert!(s.collapsed);
        assert_eq!(s.width, 200.0);
        assert_eq!(s.shown_width(800.0), s.rail_width);
        // Hysteresis: between half and the minimum it stays collapsed…
        assert!(!s.drag_to(150.0, a));
        assert!(s.collapsed);
        // …and past the minimum it opens at the pointer.
        assert!(s.drag_to(230.0, a));
        assert!(!s.collapsed);
        assert_eq!(s.width, 230.0);
    }

    #[test]
    fn reset_and_toggle_are_what_they_say() {
        let mut s = Sidebar::new(Side::Left, 250.0);
        s.width = 400.0;
        s.toggle();
        assert!(s.collapsed);
        s.reset();
        assert_eq!((s.width, s.collapsed), (250.0, false));
    }

    // ------------------------------------------------ headless egui frames

    /// Run `frames` frames of a 720x500 screen with the given sidebar and
    /// content, returning the panel widths seen and the x at which the rest
    /// of the row began on the last frame.
    fn run_frames(
        s: &mut Sidebar,
        frames: usize,
        content: impl Fn(&mut Ui) + Copy,
    ) -> (Vec<f32>, f32) {
        let ctx = egui::Context::default();
        let mut widths = Vec::new();
        let mut rest_offset = 0.0;
        for _ in 0..frames {
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(720.0, 500.0)));
            let _ = ctx.run_ui(input, |ui| {
                let start = ui.available_rect_before_wrap().left();
                let (resp, _) = s.show_inside(ui, "t", content);
                widths.push(resp.rect.width());
                // Where the rest of the row begins, relative to where the
                // column began.
                rest_offset = ui.available_rect_before_wrap().left() - start;
            });
        }
        (widths, rest_offset)
    }

    fn too_wide(ui: &mut Ui) {
        ui.label("a label that is long enough to need wrapping in a narrow column, and then some more words");
        ui.horizontal(|ui| {
            ui.label("row");
            ui.allocate_space(egui::vec2(600.0, 20.0));
        });
        ui.add(egui::TextEdit::singleline(&mut String::from("x")).desired_width(500.0));
    }

    #[test]
    fn oversized_content_does_not_widen_the_column_or_push_the_neighbour() {
        let mut s = Sidebar::new(Side::Left, 220.0).min_width(120.0).remember(false);
        let (widths, rest_left) = run_frames(&mut s, 4, too_wide);
        assert!(widths.iter().all(|&w| (w - 220.0).abs() < 0.51), "widths {widths:?}");
        assert!((rest_left - 220.0).abs() < 0.51, "the rest of the row begins at the column's edge, got {rest_left}");
        assert_eq!(s.width, 220.0, "state untouched by content");
    }

    #[test]
    fn a_narrowed_column_stays_narrow_under_the_same_content() {
        let mut s = Sidebar::new(Side::Left, 220.0).min_width(120.0).remember(false);
        s.drag_to(140.0, Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(720.0, 500.0)));
        let (widths, rest_left) = run_frames(&mut s, 4, too_wide);
        assert!(widths.iter().all(|&w| (w - 140.0).abs() < 0.51), "widths {widths:?}");
        assert!((rest_left - 140.0).abs() < 0.51);
    }

    #[test]
    fn the_width_is_identical_every_frame_while_text_wraps() {
        // The oscillation: wrap at W, widen, un-wrap, narrow, wrap… Five
        // frames of a wrapping label must give five identical widths.
        let mut s = Sidebar::new(Side::Left, 160.0).min_width(100.0).remember(false);
        let (widths, _) = run_frames(&mut s, 5, |ui| {
            for _ in 0..6 {
                ui.label("words that wrap at one hundred and sixty points and would not at two hundred");
            }
        });
        assert!(widths.windows(2).all(|p| p[0] == p[1]), "widths {widths:?}");
    }

    #[test]
    fn a_collapsed_column_is_a_rail_and_the_content_is_not_run() {
        let mut s = Sidebar::new(Side::Left, 220.0).remember(false).collapsed(true);
        let ran = std::cell::Cell::new(false);
        let (widths, _) = run_frames(&mut s, 2, |ui| {
            ran.set(true);
            ui.label("x");
        });
        assert!(widths.iter().all(|&w| (w - s.rail_width).abs() < 0.51), "widths {widths:?}");
        assert!(!ran.get());
    }

    #[test]
    fn egui_own_panel_still_grows_to_its_content_which_is_the_reason_this_module_exists() {
        // Pinned so that an egui release which stops doing this is noticed:
        // then the detached-child trick in `body` can be reconsidered.
        let ctx = egui::Context::default();
        let mut widths = Vec::new();
        for _ in 0..3 {
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(720.0, 500.0)));
            let _ = ctx.run_ui(input, |ui| {
                let r = egui::Panel::left("plain")
                    .resizable(true)
                    .default_size(220.0)
                    .show_inside(ui, too_wide);
                widths.push(r.response.rect.width());
            });
        }
        assert!(
            widths.last().copied().unwrap_or(0.0) > 400.0,
            "egui's panel used to follow its 600pt row; got {widths:?}. If it no longer does, \
egui changed its sizing rule — re-read Sidebar::body before simplifying it."
        );
    }

    #[test]
    fn the_width_survives_in_egui_memory_under_its_id() {
        let ctx = egui::Context::default();
        let id = Id::new(("uikit_sidebar", "keep"));
        let mut a = Sidebar::new(Side::Left, 250.0);
        a.width = 333.0;
        a.collapsed = true;
        a.save(&ctx, id);
        let mut b = Sidebar::new(Side::Left, 250.0);
        b.load(&ctx, id);
        assert_eq!((b.width, b.collapsed), (333.0, true));
    }
}
