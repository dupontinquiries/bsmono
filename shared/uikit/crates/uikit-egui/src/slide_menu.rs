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
//! Feature parity with `qml/SlidePopup.qml` (added for the voiceterm port; each item names the QML
//! version that introduced it, so the two histories stay cross-referenced):
//! - explicit `gridX/gridY` cell positions via [`SlideOption::at`], so menus like the 3×2 pipe grid
//!   or 1×6 brace column keep their directional muscle-memory layout;
//! - the press point lands on the **grid center** and the whole popup clamps into the screen with a
//!   4 px margin (v0.1.158's mobile-overflow fix) — a click-without-drag hovers the centermost cell,
//!   and a drag moves through the grid immediately without first entering it;
//! - deadband **variants** (v0.1.14x): with [`SlideMenu::deadband`], a cell's horizontal extent
//!   splits into open / both / close zones — the brace menu's `[` vs `[]` vs `]`;
//! - directional variants: [`SlideOption::split_dir`] (v0.1.812 — up = vertical, right = horizontal)
//!   and [`SlideOption::close_dir`] (v0.1.813 — down = pane, right/default = tab), with the ◀ ▶ ↕ ↔ /
//!   pane / tab hint glyph drawn in the hovered cell;
//! - a full-width **title band** above the grid ([`SlideMenu::title`], v0.1.800 — the per-tab menu
//!   shows the tab's name there), included in hit-test offsets and clamping;
//! - the **vertical-edge freeze** (v0.1.14x): once the pointer travels past the grid's vertical
//!   extent (±4 px), the hover freezes at its last in-bounds cell and the panel border blooms
//!   2 px → 4 px as feedback — release still commits the frozen cell;
//! - **tap-select** mode ([`SlideMenu::show_tap`], v0.1.538 — the link menu): opened by a discrete
//!   click rather than a held drag, each cell individually clickable, outside-click/Esc cancels.
//!
//! Deliberately NOT ported: the drag-speed-adaptive fade (v0.1.166 fought QML Behavior ghosting;
//! egui's spring pop-in has no such double-animation problem) and inline-SVG cell icons (egui cells
//! render text/emoji; a per-cell painter hook can come with the first real user).
//!
//! The grid geometry + hit-testing + variant rules are pure and unit-tested; only drawing + input
//! needs egui.

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

/// Which zone of the committed cell the pointer was in — QML SlidePopup's `variant` string, typed.
/// [`Variant::as_str`] round-trips to the exact QML spellings for ported dispatch tables.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    #[default]
    Single,
    /// Deadband: left of cell center — e.g. the opening brace alone.
    Open,
    /// Deadband: within ±deadband px of center — e.g. both braces with the cursor between.
    Both,
    /// Deadband: right of cell center — the closing brace alone.
    Close,
    /// split_dir: pointer toward the cell's top → vertical split.
    Vertical,
    /// split_dir: pointer toward the cell's right → horizontal split.
    Horizontal,
    /// close_dir: pointer toward the cell's bottom → close the pane.
    Pane,
    /// close_dir: default / toward the right → close the tab.
    Tab,
}

impl Variant {
    /// The QML SlidePopup `variant` string this maps to.
    pub fn as_str(self) -> &'static str {
        match self {
            Variant::Single => "single",
            Variant::Open => "open",
            Variant::Both => "both",
            Variant::Close => "close",
            Variant::Vertical => "vertical",
            Variant::Horizontal => "horizontal",
            Variant::Pane => "pane",
            Variant::Tab => "tab",
        }
    }
    /// The in-cell hint glyph QML draws for this variant (`""` when there is none).
    pub fn hint(self) -> &'static str {
        match self {
            Variant::Open => "\u{25C0}",       // ◀
            Variant::Close => "\u{25B6}",      // ▶
            Variant::Vertical => "\u{2195}",   // ↕
            Variant::Horizontal => "\u{2194}", // ↔
            Variant::Pane => "pane",
            Variant::Tab => "tab",
            _ => "",
        }
    }
}

/// One choice in the menu. `id` is the opaque value returned on select; `label` is the glyph/text.
#[derive(Clone, Debug)]
pub struct SlideOption {
    pub id: String,
    pub label: String,
    /// Explicit (gridX, gridY) cell; `None` = index-order flow layout.
    pub at: Option<(u8, u8)>,
    /// v0.1.812 directional split: up = [`Variant::Vertical`], right = [`Variant::Horizontal`].
    pub split_dir: bool,
    /// v0.1.813 directional close: down = [`Variant::Pane`], right/default = [`Variant::Tab`].
    pub close_dir: bool,
}

impl SlideOption {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { id: id.into(), label: label.into(), at: None, split_dir: false, close_dir: false }
    }
    /// Pin this option to an explicit grid cell (QML's `gridX`/`gridY`).
    pub fn at(mut self, gx: u8, gy: u8) -> Self {
        self.at = Some((gx, gy));
        self
    }
    /// Mark as a directional split cell (up = vertical, right = horizontal).
    pub fn split_dir(mut self) -> Self {
        self.split_dir = true;
        self
    }
    /// Mark as a directional close cell (down = pane, right = tab).
    pub fn close_dir(mut self) -> Self {
        self.close_dir = true;
        self
    }
}

/// Effective (cols, rows): explicit `at` positions win; otherwise index-order flow at `max_cols`.
pub fn layout_dims(options: &[SlideOption], max_cols: usize) -> (usize, usize) {
    if options.is_empty() {
        return (0, 0);
    }
    if options.iter().any(|o| o.at.is_some()) {
        let mut w = 1usize;
        let mut h = 1usize;
        for (i, o) in options.iter().enumerate() {
            let (gx, gy) = o.at.map(|(x, y)| (x as usize, y as usize)).unwrap_or((i, 0));
            w = w.max(gx + 1);
            h = h.max(gy + 1);
        }
        (w, h)
    } else {
        grid_dims(options.len(), max_cols)
    }
}

/// Cell rects honoring explicit `at` positions (falling back to index-order flow), with the grid's
/// top-left at `origin`.
pub fn cell_rects_opts(origin: Pos2, options: &[SlideOption], cols: usize, item: f32, spacing: f32) -> Vec<Rect> {
    let cols = cols.max(1);
    options
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let (gx, gy) = o.at.map(|(x, y)| (x as usize, y as usize)).unwrap_or((i % cols, i / cols));
            let x = origin.x + gx as f32 * (item + spacing);
            let y = origin.y + gy as f32 * (item + spacing);
            Rect::from_min_size(egui::pos2(x, y), Vec2::splat(item))
        })
        .collect()
}

/// The variant for a pointer at `p` inside `cell` — QML `_recomputeHover`'s zone rules, verbatim:
/// `split_dir` beats `close_dir` beats deadband beats [`Variant::Single`].
pub fn variant_at(p: Pos2, cell: Rect, opt: &SlideOption, deadband_px: Option<f32>) -> Variant {
    let c = cell.center();
    if opt.split_dir {
        // up = vertical, right = horizontal; ties (the exact diagonal) go vertical, as in QML's >=
        let upness = c.y - p.y;
        let rightness = p.x - c.x;
        if upness >= rightness {
            Variant::Vertical
        } else {
            Variant::Horizontal
        }
    } else if opt.close_dir {
        // down = pane, right (or default/center) = tab; QML uses a strict >
        let downness = p.y - c.y;
        let rightness = p.x - c.x;
        if downness > rightness {
            Variant::Pane
        } else {
            Variant::Tab
        }
    } else if let Some(db) = deadband_px {
        let d = p.x - c.x;
        if d.abs() <= db {
            Variant::Both
        } else if d < 0.0 {
            Variant::Open
        } else {
            Variant::Close
        }
    } else {
        Variant::Single
    }
}

/// Clamp a panel of `size` whose desired top-left is `target` fully inside `screen`, 4 px margin —
/// QML show()'s v0.1.158 overflow fix. When the panel is larger than the screen the top/left edge
/// wins (matches QML's clamp order: low bound applied first, then high bound overrides).
pub fn clamp_origin(target: Pos2, size: Vec2, screen: Rect) -> Pos2 {
    let mut x = target.x.max(screen.left() + 4.0);
    if x + size.x > screen.right() - 4.0 {
        x = screen.right() - size.x - 4.0;
    }
    let mut y = target.y.max(screen.top() + 4.0);
    if y + size.y > screen.bottom() - 4.0 {
        y = screen.bottom() - size.y - 4.0;
    }
    egui::pos2(x, y)
}

/// True when `y` is past the grid's vertical extent (±4 px tolerance) — the QML edge-freeze zone
/// where hover holds its last value and the border blooms.
pub fn beyond_vertical(y: f32, grid_top: f32, grid_bot: f32) -> bool {
    y < grid_top - 4.0 || y > grid_bot + 4.0
}

/// The result of a frame of [`SlideMenu::show`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlideOutcome {
    /// Nothing resolved this frame (menu closed, or open and still dragging).
    Idle,
    /// Released without a highlighted cell.
    Cancelled,
    /// Released over a cell — its `id`, and which zone of the cell the pointer was in
    /// ([`Variant::Single`] unless the option/menu enables directional or deadband variants).
    Selected(String, Variant),
}

/// Per-gesture state (open + anchor + frozen hover). `Clone`+`Copy` so it can live in egui memory
/// for stateless callers.
#[derive(Clone, Copy, Default)]
struct SlideState {
    open: bool,
    /// Panel top-left (the grid sits `padding` (+ title band) further in).
    origin: Pos2,
    /// When the current press began (for the long-press delay); `None` when not pressed.
    pressed_at: Option<f64>,
    /// Last in-bounds hover — held during the vertical-edge freeze so release still commits it.
    hot: Option<usize>,
    variant: Variant,
}

/// A reusable gesture grid menu. Hold one instance per host affordance, OR use
/// [`SlideMenu::show_stored`] to keep state in egui memory (for per-item renderers like a keyboard).
pub struct SlideMenu {
    pub max_cols: usize,
    pub item_size: f32,
    pub spacing: f32,
    /// Panel padding around the grid (QML `padding`).
    pub padding: f32,
    /// Seconds the host must be held before the grid opens. `0.0` = open immediately (a press-drag
    /// menu, the original GestureHost feel); ~`0.3` = long-press (so a quick tap still does the host's
    /// normal action, e.g. typing the key).
    pub hold_secs: f32,
    /// Minimum swipe distance (px) that counts as a cardinal swipe rather than a tap.
    pub deadzone: f32,
    /// 3-zone horizontal deadband half-width (px); `None` = plain single-variant cells.
    pub deadband_px: Option<f32>,
    /// Full-width header band above the grid (the per-tab menu shows the tab name here).
    pub title: Option<String>,
    state: SlideState,
    id: Id,
}

impl SlideMenu {
    pub fn new(id_source: impl std::hash::Hash) -> Self {
        Self {
            max_cols: 3,
            item_size: 52.0,
            spacing: 8.0,
            padding: 10.0,
            hold_secs: 0.0,
            deadzone: 18.0,
            deadband_px: None,
            title: None,
            state: SlideState::default(),
            id: Id::new(id_source),
        }
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
    /// Enable the 3-zone open/both/close deadband with the given center half-width in px
    /// (QML `enableDeadband` + `deadbandPx`, default 20).
    pub fn deadband(mut self, px: f32) -> Self {
        self.deadband_px = Some(px);
        self
    }
    /// Show a full-width title band above the grid (QML v0.1.800).
    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.title = Some(t.into());
        self
    }
    /// True while the grid is showing.
    pub fn is_open(&self) -> bool {
        self.state.open
    }
    /// Header band height (QML `_titleH`: titleFontSize 13 + 12; 0 with no title).
    fn title_h(&self) -> f32 {
        if self.title.as_deref().is_some_and(|t| !t.is_empty()) {
            25.0
        } else {
            0.0
        }
    }

    /// Drive the menu from a host widget's `response`. Call every frame. Returns the outcome; act on
    /// `Selected(id, variant)` and stop reading the host for that gesture.
    pub fn show(&mut self, ui: &mut Ui, host: &Response, options: &[SlideOption]) -> SlideOutcome {
        let id = self.id;
        let mut st = self.state;
        let out = run(self, id, &mut st, ui, host, options);
        self.state = st;
        out
    }

    /// Memory-backed variant for stateless per-item renderers (a keyboard key's long-press variants, a
    /// per-row action button): state lives in egui memory keyed by `id`, so the caller needn't hold a
    /// `SlideMenu`. Configure columns/size on `self`; `self`'s own state field is unused here.
    pub fn show_stored(&self, ui: &mut Ui, id: Id, host: &Response, options: &[SlideOption]) -> SlideOutcome {
        let mut st = ui.ctx().data_mut(|d| d.get_temp::<SlideState>(id).unwrap_or_default());
        let out = run(self, id, &mut st, ui, host, options);
        ui.ctx().data_mut(|d| d.insert_temp(id, st));
        out
    }

    /// Tap-select mode (QML v0.1.538, the link menu): the menu OPENS on a discrete click of the
    /// host (not a held press), stays open, and each cell is individually clickable — a click on a
    /// cell selects it (always [`Variant::Single`], as in QML), a click anywhere else or Esc
    /// cancels. State is memory-backed by `id`.
    pub fn show_tap(&self, ui: &mut Ui, id: Id, host: &Response, options: &[SlideOption]) -> SlideOutcome {
        let mut st = ui.ctx().data_mut(|d| d.get_temp::<SlideState>(id).unwrap_or_default());
        if !st.open {
            if host.clicked() {
                st.open = true;
                st.hot = None;
                st.variant = Variant::Single;
                st.origin = self.place(ui, host, options, host.interact_pointer_pos().unwrap_or_else(|| host.rect.center()));
                ui.ctx().data_mut(|d| d.insert_temp(id, st));
                return SlideOutcome::Idle;
            }
            ui.ctx().data_mut(|d| d.insert_temp(id, st));
            return SlideOutcome::Idle;
        }

        let (cols, _) = layout_dims(options, self.max_cols);
        let grid_origin = st.origin + Vec2::new(self.padding, self.padding + self.title_h());
        let cells = cell_rects_opts(grid_origin, options, cols, self.item_size, self.spacing);
        let panel = self.panel_rect(st.origin, options);
        let pointer = ui.ctx().pointer_latest_pos();
        st.hot = pointer.and_then(|p| hit(p, &cells));
        st.variant = Variant::Single;

        self.paint(ui, id, &st, options, &cells, panel, false);

        let mut out = SlideOutcome::Idle;
        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
        let clicked_at = ui.input(|i| i.pointer.any_click().then(|| i.pointer.interact_pos()).flatten());
        if esc {
            st.open = false;
            out = SlideOutcome::Cancelled;
        } else if let Some(p) = clicked_at {
            st.open = false;
            out = match hit(p, &cells) {
                Some(i) => SlideOutcome::Selected(options[i].id.clone(), Variant::Single),
                None => SlideOutcome::Cancelled,
            };
        }
        ui.ctx().request_repaint();
        ui.ctx().data_mut(|d| d.insert_temp(id, st));
        out
    }

    /// Open the menu programmatically at `press` — the detached equivalent of QML
    /// `popup.show(host, x, y, ctx)` for hosts that aren't egui widgets (the terminal pane's
    /// long-press, a canvas hit, ...). Placement + clamping are identical to the host path; drive
    /// the open menu each frame with [`SlideMenu::show_detached`]. If the pointer is still held
    /// from the originating gesture (the QML drag-multimenu flow), the release commits; a
    /// subsequent press-drag-release works too.
    pub fn open_at(&mut self, ui: &Ui, press: Pos2, options: &[SlideOption]) {
        self.state.open = true;
        self.state.hot = None;
        self.state.variant = Variant::Single;
        self.state.origin = self.place_at(ui, options, press);
    }

    /// Drive a menu opened by [`SlideMenu::open_at`] (no host `Response` involved). Call every
    /// frame; safe to call while closed (returns `Idle`).
    pub fn show_detached(&mut self, ui: &mut Ui, options: &[SlideOption]) -> SlideOutcome {
        if options.is_empty() {
            return SlideOutcome::Idle;
        }
        let id = self.id;
        let mut st = self.state;
        let out = drive(self, id, &mut st, ui, options);
        self.state = st;
        out
    }

    /// Panel rect (grid + padding + title band) for a panel whose top-left is `origin`.
    fn panel_rect(&self, origin: Pos2, options: &[SlideOption]) -> Rect {
        let (cols, rows) = layout_dims(options, self.max_cols);
        let w = cols as f32 * self.item_size + cols.saturating_sub(1) as f32 * self.spacing + 2.0 * self.padding;
        let h = rows as f32 * self.item_size + rows.saturating_sub(1) as f32 * self.spacing + 2.0 * self.padding + self.title_h();
        Rect::from_min_size(origin, Vec2::new(w, h))
    }

    /// QML show()'s placement: the press point lands on the GRID center, clamped into the screen.
    fn place(&self, ui: &Ui, _host: &Response, options: &[SlideOption], press: Pos2) -> Pos2 {
        self.place_at(ui, options, press)
    }

    /// Placement core (no host needed) — used by both the host path and [`SlideMenu::open_at`].
    fn place_at(&self, ui: &Ui, options: &[SlideOption], press: Pos2) -> Pos2 {
        let (cols, rows) = layout_dims(options, self.max_cols);
        let grid_w = cols as f32 * self.item_size + cols.saturating_sub(1) as f32 * self.spacing;
        let grid_h = rows as f32 * self.item_size + rows.saturating_sub(1) as f32 * self.spacing;
        let panel = self.panel_rect(Pos2::ZERO, options);
        let target = egui::pos2(
            press.x - (self.padding + grid_w / 2.0),
            press.y - (self.padding + self.title_h() + grid_h / 2.0),
        );
        clamp_origin(target, panel.size(), ui.ctx().screen_rect())
    }

    /// Shared painter for the panel + title + cells (+ hover hint). `frozen` = the vertical-edge
    /// freeze is active, so the border blooms.
    #[allow(clippy::too_many_arguments)]
    fn paint(&self, ui: &mut Ui, id: Id, st: &SlideState, options: &[SlideOption], cells: &[Rect], panel: Rect, frozen: bool) {
        /// The gesture-system hot-cell blue — deliberately NOT the theme accent (see the
        /// action-compass note; confirmed against the Qt reference screenshots).
        const HOT_CELL_BLUE: Color32 = Color32::from_rgb(0x4d, 0x9e, 0xff);

        // Spring pop-in (OutBack) — ticked every frame so it resets when closed and springs on each open.
        let appear = crate::motion::animate_eased(ui.ctx(), id.with("appear"), st.open, 0.14, crate::motion::ease::out_back);
        // Border blooms 2 → 4 px in the edge-freeze zone (QML animated 120 ms; smoothed the same way).
        let bloom = crate::motion::animate_eased(ui.ctx(), id.with("bloom"), frozen, 0.12, crate::motion::ease::out_quad);

        egui::Area::new(id).order(Order::Foreground).fixed_pos(panel.min).show(ui.ctx(), |ui| {
            let vis = ui.visuals().clone();
            let ctx = ui.ctx().clone();
            let painter = ui.painter();

            // Panel: bg + border (radius 12, the QML _bg rectangle).
            let pr = CornerRadius::same(12);
            painter.rect_filled(panel, pr, vis.window_fill);
            painter.rect_stroke(
                panel,
                pr,
                Stroke::new(2.0 + 2.0 * bloom, vis.widgets.noninteractive.bg_stroke.color),
                StrokeKind::Inside,
            );

            // Title band (full width, centered, bold — QML v0.1.800).
            if let Some(t) = self.title.as_deref().filter(|t| !t.is_empty()) {
                let band = Rect::from_min_size(panel.min, Vec2::new(panel.width(), self.padding + self.title_h()));
                painter.text(band.center(), Align2::CENTER_CENTER, t, FontId::monospace(13.0), vis.strong_text_color());
            }

            let radius = CornerRadius::same(8);
            for (i, (opt, rect)) in options.iter().zip(cells).enumerate() {
                let hot_i = st.hot == Some(i);
                // scale each cell around its center by the spring value
                let rect = Rect::from_center_size(rect.center(), rect.size() * appear.max(0.01));
                // highlight glides between rest/selected instead of snapping (QML highlightColor feel)
                let target = if hot_i { HOT_CELL_BLUE } else { vis.extreme_bg_color };
                let fill = crate::motion::animate_color(&ctx, id.with(("cell", i)), target, 0.12);
                painter.rect_filled(rect, radius, fill);
                painter.rect_stroke(rect, radius, Stroke::new(1.0, vis.widgets.noninteractive.bg_stroke.color), StrokeKind::Inside);
                let text_col = if hot_i { Color32::WHITE } else { vis.text_color() };
                painter.text(rect.center(), Align2::CENTER_CENTER, &opt.label, FontId::proportional(18.0), text_col);
                // Directional/deadband hint glyph at the cell's bottom edge (QML variant indicator).
                if hot_i {
                    let hint = st.variant.hint();
                    if !hint.is_empty() {
                        painter.text(
                            egui::pos2(rect.center().x, rect.bottom() - 2.0),
                            Align2::CENTER_BOTTOM,
                            hint,
                            FontId::proportional(10.0),
                            HOT_CELL_BLUE.gamma_multiply(1.5).to_opaque(),
                        );
                    }
                }
            }
        });
        ui.ctx().request_repaint();
    }
}

/// The shared press-drag-release gesture logic used by `show` and `show_stored`.
fn run(menu: &SlideMenu, id: Id, st: &mut SlideState, ui: &mut Ui, host: &Response, options: &[SlideOption]) -> SlideOutcome {
    if options.is_empty() {
        return SlideOutcome::Idle;
    }

    // Open once the host has been held past `hold_secs` (immediately when it's 0).
    if host.is_pointer_button_down_on() {
        let now = ui.input(|i| i.time);
        let start = *st.pressed_at.get_or_insert(now);
        if !st.open && (now - start) >= menu.hold_secs as f64 {
            st.open = true;
            st.hot = None;
            st.variant = Variant::Single;
            let press = host.interact_pointer_pos().unwrap_or_else(|| host.rect.center());
            st.origin = menu.place(ui, host, options, press);
        }
    } else if !st.open {
        st.pressed_at = None;
    }
    drive(menu, id, st, ui, options)
}

/// The drag-highlight-release phase, shared by the host-driven paths and the detached one.
fn drive(menu: &SlideMenu, id: Id, st: &mut SlideState, ui: &mut Ui, options: &[SlideOption]) -> SlideOutcome {
    if !st.open {
        return SlideOutcome::Idle;
    }
    ui.ctx().request_repaint(); // poll the hold timer / follow the pointer

    let (cols, _) = layout_dims(options, menu.max_cols);
    let grid_origin = st.origin + Vec2::new(menu.padding, menu.padding + menu.title_h());
    let cells = cell_rects_opts(grid_origin, options, cols, menu.item_size, menu.spacing);
    let panel = menu.panel_rect(st.origin, options);
    let pointer = ui.ctx().pointer_latest_pos();

    // Grid vertical extent for the edge-freeze rule (title band included, as in QML v0.1.801).
    let grid_top = grid_origin.y;
    let grid_bot = cells.iter().fold(grid_top, |m, r| m.max(r.bottom()));
    let frozen = pointer.is_some_and(|p| beyond_vertical(p.y, grid_top, grid_bot));

    if !frozen {
        if let Some(p) = pointer {
            match hit(p, &cells) {
                Some(i) => {
                    st.hot = Some(i);
                    st.variant = variant_at(p, cells[i], &options[i], menu.deadband_px);
                }
                None => {
                    // in bounds vertically but in a gutter — clear, so release here cancels
                    st.hot = None;
                    st.variant = Variant::Single;
                }
            }
        }
    } // else: hold the last in-bounds hover (release still commits it)

    menu.paint(ui, id, st, options, &cells, panel, frozen);

    if ui.input(|i| i.pointer.any_released()) {
        let out = match st.hot {
            Some(i) => SlideOutcome::Selected(options[i].id.clone(), st.variant),
            None => SlideOutcome::Cancelled,
        };
        st.open = false;
        st.pressed_at = None;
        st.hot = None;
        st.variant = Variant::Single;
        return out;
    }
    SlideOutcome::Idle
}

/// Eight compass directions for cardinal-swipe (MessagEase-style) keys.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir8 {
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
}

impl Dir8 {
    /// Unit offset for placing this direction's cell around a center (screen space: +y is down).
    pub fn offset(self) -> Vec2 {
        const D: f32 = std::f32::consts::FRAC_1_SQRT_2; // 0.707, the diagonal component
        let (x, y) = match self {
            Dir8::N => (0.0, -1.0),
            Dir8::NE => (D, -D),
            Dir8::E => (1.0, 0.0),
            Dir8::SE => (D, D),
            Dir8::S => (0.0, 1.0),
            Dir8::SW => (-D, D),
            Dir8::W => (-1.0, 0.0),
            Dir8::NW => (-D, -D),
        };
        Vec2::new(x, y)
    }
}

/// Snap a swipe `delta` (screen space, +y down) to a compass direction, or `None` if it's shorter than
/// `deadzone` — i.e. a tap, not a swipe. Pure — the tested core of cardinal dispatch.
pub fn swipe_dir8(delta: Vec2, deadzone: f32) -> Option<Dir8> {
    if delta.length() < deadzone {
        return None;
    }
    // Flip y so "up" is positive; 0deg = E, 90deg = N. Snap to 45deg sectors.
    let ang = (-delta.y).atan2(delta.x).to_degrees();
    let a = (ang + 360.0) % 360.0;
    let sector = (((a + 22.5) / 45.0) as i32) % 8;
    Some(match sector {
        0 => Dir8::E,
        1 => Dir8::NE,
        2 => Dir8::N,
        3 => Dir8::NW,
        4 => Dir8::W,
        5 => Dir8::SW,
        6 => Dir8::S,
        _ => Dir8::SE,
    })
}

impl SlideMenu {
    /// A cardinal-swipe key popup: press shows `tap` at the center with each `dirs` variant floated in
    /// its compass direction; the live swipe from center highlights a direction; release commits the
    /// swiped direction's variant, or `tap` when the swipe stayed inside the deadzone (a plain tap).
    /// State is memory-backed by `id`, so a keyboard can call this per-key without holding a menu.
    /// Always resolves to a `Selected` (a key press never cancels).
    pub fn show_cardinal(
        &self,
        ui: &mut Ui,
        id: Id,
        host: &Response,
        tap: &SlideOption,
        dirs: &[(Dir8, SlideOption)],
    ) -> SlideOutcome {
        let mut st = ui.ctx().data_mut(|d| d.get_temp::<SlideState>(id).unwrap_or_default());
        if !st.open && host.is_pointer_button_down_on() {
            st.open = true;
            st.origin = host.rect.center();
        }
        if !st.open {
            ui.ctx().data_mut(|d| d.insert_temp(id, st));
            return SlideOutcome::Idle;
        }
        let center = st.origin;
        let pointer = ui.ctx().pointer_latest_pos().unwrap_or(center);
        let hot = swipe_dir8(pointer - center, self.deadzone);
        let radius = self.item_size + 6.0;

        // Spring pop-in for the whole compass, and a smoothly-eased per-cell highlight.
        let appear = crate::motion::animate_eased(ui.ctx(), id.with("appear"), st.open, 0.14, crate::motion::ease::out_back);
        let item = self.item_size;

        egui::Area::new(id).order(Order::Foreground).fixed_pos(Pos2::ZERO).show(ui.ctx(), |ui| {
            let vis = ui.visuals().clone();
            let ctx = ui.ctx().clone();
            let painter = ui.painter();
            let cr = CornerRadius::same(8);
            let cell = |c: Pos2, label: &str, on: bool, key: usize| {
                let rect = Rect::from_center_size(c, Vec2::splat(item * appear.max(0.01)));
                let target = if on { vis.selection.bg_fill } else { vis.extreme_bg_color };
                let fill = crate::motion::animate_color(&ctx, id.with(("card", key)), target, 0.12);
                painter.rect_filled(rect, cr, fill);
                painter.rect_stroke(rect, cr, Stroke::new(1.0, vis.widgets.noninteractive.bg_stroke.color), StrokeKind::Inside);
                let tc = if on { Color32::WHITE } else { vis.text_color() };
                painter.text(c, Align2::CENTER_CENTER, label, FontId::proportional(18.0), tc);
            };
            // center (tap) is highlighted only when there's no active swipe
            cell(center, &tap.label, hot.is_none(), 0);
            for (idx, (d, opt)) in dirs.iter().enumerate() {
                let c = center + d.offset() * radius;
                cell(c, &opt.label, hot == Some(*d), idx + 1);
            }
        });
        ui.ctx().request_repaint();

        if ui.input(|i| i.pointer.any_released()) {
            st.open = false;
            ui.ctx().data_mut(|d| d.insert_temp(id, st));
            let chosen = hot
                .and_then(|d| dirs.iter().find(|(dd, _)| *dd == d))
                .map(|(_, o)| o.id.clone())
                .unwrap_or_else(|| tap.id.clone());
            return SlideOutcome::Selected(chosen, Variant::Single);
        }
        ui.ctx().data_mut(|d| d.insert_temp(id, st));
        SlideOutcome::Idle
    }
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

    #[test]
    fn swipe_dir8_snaps_cardinals_and_ignores_taps() {
        assert_eq!(swipe_dir8(Vec2::new(0.0, -30.0), 18.0), Some(Dir8::N)); // up
        assert_eq!(swipe_dir8(Vec2::new(30.0, 0.0), 18.0), Some(Dir8::E));
        assert_eq!(swipe_dir8(Vec2::new(0.0, 30.0), 18.0), Some(Dir8::S)); // down
        assert_eq!(swipe_dir8(Vec2::new(-30.0, 0.0), 18.0), Some(Dir8::W));
        assert_eq!(swipe_dir8(Vec2::new(25.0, -25.0), 18.0), Some(Dir8::NE));
        assert_eq!(swipe_dir8(Vec2::new(5.0, 5.0), 18.0), None); // inside deadzone → a tap
    }

    #[test]
    fn dir_offsets_point_the_right_way() {
        assert!(Dir8::N.offset().y < 0.0); // up
        assert!(Dir8::S.offset().y > 0.0); // down
        assert!(Dir8::E.offset().x > 0.0);
        assert!(Dir8::W.offset().x < 0.0);
    }

    // ── parity additions ─────────────────────────────────────────────────────

    /// The voiceterm pipe menu: 3×2 with explicit positions.
    fn pipe_menu() -> Vec<SlideOption> {
        vec![
            SlideOption::new("pipe", "|").at(0, 0),
            SlideOption::new("bg", "&").at(1, 0),
            SlideOption::new("semi", ";").at(2, 0),
            SlideOption::new("stdin", "<").at(0, 1),
            SlideOption::new("stdout", ">").at(1, 1),
            SlideOption::new("append", ">>").at(2, 1),
        ]
    }

    #[test]
    fn explicit_grid_positions_shape_the_layout() {
        let opts = pipe_menu();
        assert_eq!(layout_dims(&opts, 3), (3, 2));
        // a 1×6 column stays a column even with max_cols = 3 (explicit positions win)
        let brace: Vec<_> = (0..6).map(|i| SlideOption::new(format!("b{i}"), "x").at(0, i as u8)).collect();
        assert_eq!(layout_dims(&brace, 3), (1, 6));
        // rects land where gridX/gridY say
        let cells = cell_rects_opts(Pos2::ZERO, &opts, 3, 50.0, 10.0);
        assert_eq!(cells[2].min, egui::pos2(120.0, 0.0)); // (2,0)
        assert_eq!(cells[3].min, egui::pos2(0.0, 60.0)); // (0,1)
        // no explicit positions → identical to the flow layout
        let flow: Vec<_> = (0..4).map(|i| SlideOption::new(format!("f{i}"), "x")).collect();
        assert_eq!(cell_rects_opts(Pos2::ZERO, &flow, 2, 50.0, 10.0), cell_rects(Pos2::ZERO, 4, 2, 50.0, 10.0));
        assert_eq!(layout_dims(&flow, 2), grid_dims(4, 2));
    }

    #[test]
    fn deadband_splits_open_both_close_like_qml() {
        let cell = Rect::from_min_size(Pos2::ZERO, Vec2::splat(56.0)); // center x = 28
        let o = SlideOption::new("curly", "{ }");
        let v = |x: f32| variant_at(egui::pos2(x, 28.0), cell, &o, Some(20.0));
        assert_eq!(v(28.0), Variant::Both); // dead center
        assert_eq!(v(8.0), Variant::Both); // exactly at −deadband (QML <=)
        assert_eq!(v(48.0), Variant::Both); // exactly at +deadband
        assert_eq!(v(7.0), Variant::Open); // just past the left zone edge
        assert_eq!(v(49.0), Variant::Close); // just past the right zone edge
        // without deadband: single, wherever the pointer is
        assert_eq!(variant_at(egui::pos2(1.0, 1.0), cell, &o, None), Variant::Single);
    }

    #[test]
    fn split_and_close_directional_variants_match_qml() {
        let cell = Rect::from_min_size(Pos2::ZERO, Vec2::splat(56.0)); // center (28, 28)
        let split = SlideOption::new("split", "⊞").split_dir();
        // up beats right when upness >= rightness (QML's >=): straight up → vertical
        assert_eq!(variant_at(egui::pos2(28.0, 5.0), cell, &split, None), Variant::Vertical);
        // to the right → horizontal
        assert_eq!(variant_at(egui::pos2(50.0, 28.0), cell, &split, None), Variant::Horizontal);
        // exact diagonal ties go vertical
        assert_eq!(variant_at(egui::pos2(38.0, 18.0), cell, &split, None), Variant::Vertical);
        // dead center: upness == rightness == 0 → vertical (the >= tie)
        assert_eq!(variant_at(egui::pos2(28.0, 28.0), cell, &split, None), Variant::Vertical);

        let close = SlideOption::new("close", "✕").close_dir();
        // down → pane; right or center → tab (QML's strict >)
        assert_eq!(variant_at(egui::pos2(28.0, 50.0), cell, &close, None), Variant::Pane);
        assert_eq!(variant_at(egui::pos2(50.0, 28.0), cell, &close, None), Variant::Tab);
        assert_eq!(variant_at(egui::pos2(28.0, 28.0), cell, &close, None), Variant::Tab); // center default = tab

        // directional flags beat deadband
        assert_eq!(variant_at(egui::pos2(5.0, 28.0), cell, &split, Some(20.0)), Variant::Vertical);
    }

    #[test]
    fn variant_strings_round_trip_the_qml_spellings() {
        for (v, s) in [
            (Variant::Single, "single"),
            (Variant::Open, "open"),
            (Variant::Both, "both"),
            (Variant::Close, "close"),
            (Variant::Vertical, "vertical"),
            (Variant::Horizontal, "horizontal"),
            (Variant::Pane, "pane"),
            (Variant::Tab, "tab"),
        ] {
            assert_eq!(v.as_str(), s);
        }
        assert_eq!(Variant::Open.hint(), "\u{25C0}");
        assert_eq!(Variant::Vertical.hint(), "\u{2195}");
        assert_eq!(Variant::Single.hint(), "");
    }

    #[test]
    fn clamping_keeps_the_panel_on_screen_with_a_4px_margin() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let size = Vec2::new(200.0, 120.0);
        // fits: unchanged
        assert_eq!(clamp_origin(egui::pos2(100.0, 100.0), size, screen), egui::pos2(100.0, 100.0));
        // off every edge: pulled to the 4px margin
        assert_eq!(clamp_origin(egui::pos2(-50.0, -50.0), size, screen), egui::pos2(4.0, 4.0));
        assert_eq!(clamp_origin(egui::pos2(700.0, 550.0), size, screen), egui::pos2(800.0 - 204.0, 600.0 - 124.0));
        // taller than the screen: the top edge wins (high bound applied last, as in QML)
        let tall = Vec2::new(100.0, 700.0);
        assert_eq!(clamp_origin(egui::pos2(10.0, 10.0), tall, screen).y, 600.0 - 704.0);
    }

    #[test]
    fn vertical_edge_zone_has_the_qml_4px_tolerance() {
        // grid from y=100 to y=300
        assert!(!beyond_vertical(200.0, 100.0, 300.0)); // inside
        assert!(!beyond_vertical(97.0, 100.0, 300.0)); // within top tolerance
        assert!(!beyond_vertical(303.0, 100.0, 300.0)); // within bottom tolerance
        assert!(beyond_vertical(95.0, 100.0, 300.0)); // past the top
        assert!(beyond_vertical(305.0, 100.0, 300.0)); // past the bottom
    }
}
