//! hatch-egui — the plasmoid's dashboard, redrawn in egui over `hatch-core`.
//!
//! The interaction contract mirrors the QML surface deliberately:
//!
//! * a fullscreen translucent sheet; the app grid centred, kinetic-scrolling;
//! * the search pill anchored at the BOTTOM (thumb reach), system row above it;
//! * tap launches; **tapping empty grid space does nothing** (the QML bug this
//!   project started from — a missed tap must never dismiss the launcher);
//! * long-press a tile toggles favorite (the touch stand-in for the QML
//!   context menu);
//! * dismissal is explicit: swipe down on the top grabber, Esc with an empty
//!   query, or focus loss (the shell decides that part).
//!
//! Everything except the pixels is testable: selection math, eliding, column
//! fitting, and the event the shell must act on come back as values.

pub mod icons_img;
pub mod toggle;

use std::collections::HashMap;
use std::time::{Duration, Instant};

const NOTICE_W: f32 = 230.0;
const NOTICE_H: f32 = 74.0;

/// Share of the configured icon size an app icon is drawn at inside its card.
const ICON_FRAC: f32 = 0.72;

use egui::{Align2, Color32, CornerRadius, FontId, Key, Rect, Sense, Stroke, StrokeKind, Vec2};
use hatch_core::anim::{Board, TilePose};
use hatch_core::grid::GridSpec;
use hatch_core::Launcher;
use uikit_egui::{A11yScene, PaletteColors, Role, Semantics};

/// What the shell must do after a frame. The lib never touches the window —
/// hiding/quitting is viewport business and stays in the bin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiEvent {
    None,
    /// An app (or system action) was started; the shell should hide.
    Launched,
    /// The user asked to dismiss (swipe-down / Esc on empty query).
    Dismiss,
}

/// How long a still press becomes a favorite-toggle. Matches the QML
/// TapHandler's default long-press feel rather than desktop context-menu 800ms.
const LONG_PRESS: Duration = Duration::from_millis(550);
/// Movement beyond this cancels a long-press (it's a scroll, not a hold).
const LONG_PRESS_SLOP: f32 = 12.0;
/// Swipe distance on the grabber that dismisses.
const DISMISS_DRAG: f32 = 80.0;

pub struct HatchUi {
    pub core: Launcher,
    /// The one line from the boxes at home (logscope on the Ultra / the mini), off the DHT.
    /// Empty channel = never asks, draws nothing. `HATCH_STATUS_CHANNEL` sets it.
    pub status: hatch_core::status::Feed,
    query: String,
    /// Keyboard selection into the current results; drawn as the highlight
    /// and launched by Enter. Reset on query change.
    sel: usize,
    focus_search: bool,
    /// Bumping this resalts the ScrollArea, which resets its offset — the
    /// "re-opened launchers start at the top" rule.
    scroll_gen: u64,
    /// Decoded icons by entry id; `None` records a resolve/decode failure so
    /// a missing icon isn't retried every frame. Bounded in practice by the
    /// number of installed apps whose tiles have been on screen — ~200 apps
    /// at 128px RGBA is roughly 13 MB of GPU textures, which is acceptable
    /// for a resident launcher and is the price of instant re-summon.
    /// Cleared wholesale when the scan changes; see `on_shown`.
    icon_cache: HashMap<String, Option<egui::TextureHandle>>,
    // Long-press bookkeeping (egui has clicks and drags, not holds).
    /// The tile whose context menu is open, and where to draw it. Long-press
    /// (and right-click) open it; it is the touch path to everything that
    /// isn't "launch": favorites, and the entry's own [Desktop Action] items
    /// like New Window — the same contents the plasmoid's menu offers.
    menu: Option<(String, egui::Pos2)>,
    press: Option<PressTrack>,
    /// Set when the current press has already done its job (a long-press
    /// fired, or it turned into a scroll). Scoped to the press, not to a
    /// tile: the grid can restack between press and release, and a swallow
    /// that matched on tile id would miss and launch whatever moved in.
    swallow_click: bool,
    grabber_drag: f32,
    last_error: Option<(String, Instant)>,
    /// Screen rects of the tiles painted last frame, in display order.
    /// The grid is hand-laid-out, so this is the only record of where a tile
    /// actually ended up — needed to answer "which tile is under this point"
    /// from outside `ui()` (tests aim real taps with it today; a gesture
    /// layer that has to know which tile a swipe began on would use the same
    /// answer rather than recomputing the layout and drifting from it).
    tile_rects: Vec<(String, Rect)>,
    /// Demo notices (see hatch_core::notices); a tap dismisses one.
    notices: Vec<hatch_core::notices::Notice>,
    /// Voice/screen-reader registration for the grid. `sel` (above) stays the
    /// single source of truth for WHICH tile is selected and how arrow keys
    /// move it — 2D, row/col, wraps at grid edges — because `A11yScene`'s own
    /// Next/Prev is linear and would regress that. This scene is re-populated
    /// every frame from the same tiles `grid()` already draws, and `sel` is
    /// pushed into it via `set_focus` so the two are never out of sync; it
    /// does not decide anything, it reports.
    a11y: A11yScene,
    /// The grid's per-tile motion — enter/restack/exit — driven every frame
    /// from `self.core.results()`'s order. Was ported into `hatch-core`
    /// (headless, tested) and never called from here; see `PARITY.md`. Kept
    /// as a resident field, not rebuilt per frame, because `Board::retarget`
    /// diffs against its OWN previous state to know what's new/moved/leaving
    /// — a fresh `Board` every frame would see every tile as new, every time.
    board: Board,
}

struct PressTrack {
    key: String,
    started: Instant,
    origin: egui::Pos2,
    fired: bool,
}

impl HatchUi {
    pub fn new(core: Launcher) -> Self {
        let board_config = core.config.anim; // captured before `core` moves into the struct below
        HatchUi {
            status: hatch_core::status::Feed::new(&std::env::var("HATCH_STATUS_CHANNEL").unwrap_or_default()),
            core,
            query: String::new(),
            sel: 0,
            focus_search: true,
            scroll_gen: 0,
            icon_cache: HashMap::new(),
            menu: None,
            press: None,
            swallow_click: false,
            grabber_drag: 0.0,
            last_error: None,
            tile_rects: Vec::new(),
            notices: if std::env::var("HATCH_NOTICES").as_deref() == Ok("0") { Vec::new() } else { hatch_core::notices::demo() },
            a11y: A11yScene::new(),
            board: Board::new(board_config),
        }
    }

    /// A stable id for a11y registration, independent of grid position —
    /// the grid reorders every keystroke (favorites/recents float, a filter
    /// narrows the set) and `A11yScene` keys focus by id, not index. Reusing
    /// position as id would make focus visually "stick to a screen slot"
    /// across a reorder instead of following the app the user actually
    /// selected. `DefaultHasher` isn't stable across Rust versions/processes,
    /// which is fine here: ids only need to compare equal within one run.
    fn a11y_id(entry_id: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        entry_id.hash(&mut h);
        h.finish()
    }

    /// Where a tile was painted last frame, if it was on screen.
    pub fn tile_rect(&self, entry_id: &str) -> Option<Rect> {
        self.tile_rects.iter().find(|(id, _)| id == entry_id).map(|(_, r)| *r)
    }

    /// The union of the tiles painted last frame — the occupied part of the
    /// grid, as distinct from the empty space around it.
    pub fn tiles_bounds(&self) -> Option<Rect> {
        self.tile_rects
            .iter()
            .map(|(_, r)| *r)
            .reduce(|a, b| a.union(b))
    }

    /// Called by the shell whenever the window becomes visible again.
    pub fn on_shown(&mut self) {
        self.query.clear();
        self.core.set_query("");
        // Summoning is the moment to notice apps installed while hidden. The
        // icon cache is keyed by entry id, so a changed entry set can leave
        // it holding textures for apps that no longer exist (and, worse,
        // stale art for an app whose icon the update replaced) — drop it and
        // let the per-frame decode budget refill it.
        if self.core.refresh_if_changed() {
            self.icon_cache.clear();
        }
        // A re-summoned launcher is the natural moment for favorites toggled
        // last time to take their new places.
        self.core.resettle();
        self.sel = 0;
        self.scroll_gen += 1;
        self.focus_search = true;
        self.grabber_drag = 0.0;
        self.menu = None;
    }

    /// Whether a tile context menu is currently open.
    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    // ---- pure helpers, tested below -------------------------------------

    /// Columns that fit: config wins when set, else as many cells as fit,
    /// never fewer than 2 (a 1-column launcher is a list, not this widget).
    pub fn grid_cols(avail_w: f32, cell_w: f32, cfg_cols: u32) -> usize {
        if cfg_cols > 0 {
            return cfg_cols as usize;
        }
        ((avail_w / cell_w).floor() as usize).max(2)
    }

    /// Char-based elide with a middle ellipsis — matches the QML label's
    /// `Text.ElideMiddle`, which keeps both the brand and the suffix visible
    /// ("Fir…fox Nightly" beats "Firefox Nig…"). This decides the SHAPE of
    /// the elision; [`HatchUi::elide_fitting`] decides how much of it fits,
    /// because a character count cannot see that "W" is three times "l".
    pub fn elide(name: &str, max_chars: usize) -> String {
        let chars: Vec<char> = name.chars().collect();
        if chars.len() <= max_chars || max_chars < 3 {
            return name.to_string();
        }
        let keep = max_chars - 1;
        let head = keep / 2 + keep % 2;
        let tail = keep / 2;
        let mut s: String = chars[..head].iter().collect();
        s.push('…');
        s.extend(&chars[chars.len() - tail..]);
        s
    }

    /// The longest middle-elided form of `text` that fits `max_w`, measured
    /// with the font actually being painted. Binary search over the character
    /// count: ~5 layouts for a name that overflows, none for one that
    /// doesn't, and egui caches galleys so a steady grid pays once.
    fn elide_fitting(ui: &egui::Ui, text: &str, font: &FontId, max_w: f32) -> String {
        // Painter::layout_no_wrap, not Ui::fonts: in egui 0.34 the layout
        // methods need &mut Fonts, and the painter is what hands that out.
        let measure = |s: &str| {
            ui.painter()
                .layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE)
                .size()
                .x
        };
        if measure(text) <= max_w {
            return text.to_string();
        }
        let len = text.chars().count();
        let mut best = Self::elide(text, 3);
        let (mut lo, mut hi) = (3usize, len);
        while lo <= hi {
            let mid = (lo + hi) / 2;
            let cand = Self::elide(text, mid);
            if measure(&cand) <= max_w {
                best = cand;
                lo = mid + 1;
            } else if mid == 3 {
                break;
            } else {
                hi = mid - 1;
            }
        }
        best
    }

    /// Arrow-key movement over a `cols`-wide grid of `count` items. Same
    /// rules as the plasmoid's Keys handlers: no wrapping across row edges,
    /// Up on the top row stays put (the shell's search field is "above"),
    /// Down into a partial last row clamps to the final item.
    pub fn move_sel(sel: usize, count: usize, cols: usize, key: Key) -> usize {
        if count == 0 || cols == 0 {
            return 0;
        }
        let last = count - 1;
        let col = sel % cols;
        match key {
            Key::ArrowRight if col != cols - 1 && sel != last => sel + 1,
            Key::ArrowLeft if col != 0 => sel - 1,
            Key::ArrowDown if sel / cols < last / cols => (sel + cols).min(last),
            Key::ArrowUp if sel >= cols => sel - cols,
            _ => sel,
        }
    }

    // ---- drawing ---------------------------------------------------------

    /// Draw one frame into `ui` (the shell's CentralPanel). Returns the event
    /// the shell must act on.
    pub fn ui(&mut self, ui: &mut egui::Ui, colors: &PaletteColors) -> UiEvent {
        let mut event = UiEvent::None;
        let cfg_icon = self.core.config.icon_px as f32;
        // Square, not (icon_px+44, icon_px+40): `GridSpec`/`Board` (hatch-core::anim,
        // hatch-core::grid) assume square cells — every SlidePattern's offset
        // math and cell_pos() take one `cell: f32`. The old rectangular cell
        // predates this wiring and was never exercised against it. Taking the
        // larger of the two paddings only ever grows the tile, never shrinks it.
        let cell = Vec2::splat(cfg_icon + 44.0);

        // Reserve the bottom stack first so the grid can take the remainder.
        let sys_actions = self.core.system_actions();
        let sys_h = if sys_actions.is_empty() { 0.0 } else { 56.0 };
        let search_h = 52.0;
        let grabber_h = 26.0;
        let notices_h = if self.notices.is_empty() { 0.0 } else { NOTICE_H + 10.0 };
        let grid_h =
            (ui.available_height() - grabber_h - sys_h - search_h - notices_h - 16.0).max(cell.y);

        if self.grabber(ui, grabber_h) {
            event = UiEvent::Dismiss;
        }

        self.notice_strip(ui, colors);

        // ---- the grid ----
        let cols = Self::grid_cols(ui.available_width(), cell.x, self.core.config.columns);
        let count = self.core.result_count();
        self.keyboard(ui, cols, count, &mut event);

        ui.allocate_ui(Vec2::new(ui.available_width(), grid_h), |ui| {
            uikit_egui::scroll::area_keyed(ui, ("hatch-grid", self.scroll_gen), |ui| {
                self.grid(ui, cols, cell, colors, &mut event);
            });
        });

        // ---- error line (launch failures must be visible, not silent) ----
        if let Some((msg, when)) = &self.last_error {
            if when.elapsed() < Duration::from_secs(4) {
                ui.colored_label(Color32::from_rgb(230, 90, 90), msg);
                ui.ctx().request_repaint_after(Duration::from_millis(300));
            } else {
                self.last_error = None;
            }
        }

        // ---- system row + search pill, bottom-anchored ----
        self.status.tick();
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            if self.search_pill(ui, search_h, colors) {
                event = UiEvent::Launched;
            }
            if !sys_actions.is_empty() {
                ui.add_space(6.0);
                if self.system_row(ui, colors) {
                    event = UiEvent::Launched;
                }
            }
            // The status line from home, above the system row: `mini · loaded Ornith 71 tok/s`.
            // Dimmed once it is older than the feed's ask interval, so a dead publisher reads as
            // stale rather than as current. Not a tile, not tappable, never launches anything.
            if let Some(line) = self.status.latest() {
                let stale = self.status.age().map(|a| a > self.status.interval * 3).unwrap_or(true);
                ui.add_space(4.0);
                ui.label(egui::RichText::new(line.render()).small().color(if stale { colors.text_weak } else { colors.text }));
                ui.ctx().request_repaint_after(self.status.interval);
            }
        });

        // Last, so it paints over everything and gets first refusal on the
        // click that dismisses it.
        let menu_event = self.tile_menu(ui);
        if menu_event != UiEvent::None {
            event = menu_event;
        }

        event
    }

    /// The strip of notices above the grid: one card each, scrolling sideways, a tap dismisses.
    /// Demo data for now, and says so.
    fn notice_strip(&mut self, ui: &mut egui::Ui, colors: &PaletteColors) {
        if self.notices.is_empty() {
            return;
        }
        let mut dismiss: Option<u32> = None;
        egui::ScrollArea::horizontal().id_salt("hatch-notices").auto_shrink([false, true]).show(ui, |ui| {
            ui.horizontal(|ui| {
                for n in &self.notices {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(NOTICE_W, NOTICE_H), Sense::click());
                    let p = ui.painter();
                    let call = n.kind == hatch_core::notices::Kind::Call && n.when == "now";
                    let ground = if call { colors.accent_soft } else { colors.bg_raised };
                    p.rect_filled(rect, CornerRadius::same(14), ground);
                    p.rect_stroke(rect, CornerRadius::same(14), Stroke::new(1.0, colors.border.gamma_multiply(0.6)), StrokeKind::Inside);
                    p.text(rect.left_top() + Vec2::new(12.0, 10.0), Align2::LEFT_TOP, n.kind.glyph(), FontId::proportional(16.0), colors.accent);
                    p.text(rect.left_top() + Vec2::new(34.0, 11.0), Align2::LEFT_TOP, format!("{} · {}", n.kind.label(), n.when), FontId::proportional(11.0), colors.text_weak);
                    p.text(rect.left_top() + Vec2::new(12.0, 30.0), Align2::LEFT_TOP, Self::elide(&n.title, 26), FontId::proportional(14.0), colors.text);
                    p.text(rect.left_top() + Vec2::new(12.0, 50.0), Align2::LEFT_TOP, Self::elide(&n.body, 34), FontId::proportional(11.5), colors.text_weak);
                    if resp.clicked() {
                        dismiss = Some(n.id);
                    }
                }
            });
        });
        ui.label(egui::RichText::new("demo messages: nothing is publishing these yet").small().color(colors.text_weak));
        if let Some(id) = dismiss {
            self.notices.retain(|n| n.id != id);
        }
    }

    /// Draw the open tile menu, if any. Returns an event when it launched
    /// something. Drawn last, in a foreground Area, so it sits over the grid;
    /// a click anywhere else dismisses it without also hitting a tile
    /// (the grid skips launching while `menu` is set).
    fn tile_menu(&mut self, ui: &mut egui::Ui) -> UiEvent {
        let Some((id, at)) = self.menu.clone() else { return UiEvent::None };
        let mut event = UiEvent::None;
        let mut close = false;

        let is_fav = self.core.is_favorite(&id);
        // Cloned: the menu mutates core while iterating what core lends it.
        let actions: Vec<(String, String)> = self
            .core
            .actions_for(&id)
            .iter()
            .map(|a| (a.id.clone(), a.name.clone()))
            .collect();
        let name = self
            .core
            .results()
            .find(|e| e.id == id)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| id.clone());

        // Keep the menu on screen when the tile is near an edge.
        let screen = ui.ctx().screen_rect();
        let pos = egui::pos2(
            at.x.min(screen.right() - 190.0).max(screen.left() + 8.0),
            at.y.min(screen.bottom() - 60.0 - 26.0 * (actions.len() as f32 + 1.0)),
        );

        let resp = egui::Area::new(egui::Id::new("hatch-tile-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(170.0);
                    ui.label(egui::RichText::new(&name).strong());
                    ui.separator();
                    let fav_label = if is_fav { "Remove from favorites" } else { "Add to favorites" };
                    if ui.button(fav_label).clicked() {
                        self.core.toggle_favorite(&id);
                        close = true;
                    }
                    for (action_id, label) in &actions {
                        if ui.button(label).clicked() {
                            match self.core.activate_action(&id, action_id) {
                                Ok(_) => event = UiEvent::Launched,
                                Err(e) => {
                                    self.last_error = Some((format!("{label}: {e}"), Instant::now()))
                                }
                            }
                            close = true;
                        }
                    }
                });
            })
            .response;

        // Anywhere else: dismiss the menu, and only the menu.
        if ui.input(|i| i.pointer.any_click()) && !resp.contains_pointer() {
            close = true;
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            close = true;
        }
        if close {
            self.menu = None;
        }
        event
    }

    /// The top grabber: a pill you can swipe down to dismiss. Drag-only —
    /// taps on it do nothing, so it cannot eat a mis-aimed tap.
    fn grabber(&mut self, ui: &mut egui::Ui, h: f32) -> bool {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::drag());
        let pill = Rect::from_center_size(rect.center(), Vec2::new(48.0, 5.0));
        let a = if resp.dragged() { 200 } else { 90 };
        ui.painter().rect_filled(pill, CornerRadius::same(3), Color32::from_white_alpha(a));
        if resp.dragged() {
            self.grabber_drag += resp.drag_delta().y;
        }
        if resp.drag_stopped() {
            let fired = self.grabber_drag > DISMISS_DRAG;
            self.grabber_drag = 0.0;
            return fired;
        }
        false
    }

    fn keyboard(&mut self, ui: &egui::Ui, cols: usize, count: usize, event: &mut UiEvent) {
        ui.input(|i| {
            for k in [Key::ArrowLeft, Key::ArrowRight, Key::ArrowUp, Key::ArrowDown] {
                if i.key_pressed(k) {
                    self.sel = Self::move_sel(self.sel, count, cols, k);
                }
            }
            if i.key_pressed(Key::Escape) {
                if self.menu.is_some() {
                    // Handled by tile_menu; the launcher stays put.
                } else if self.query.is_empty() {
                    *event = UiEvent::Dismiss;
                } else {
                    self.query.clear();
                    self.core.set_query("");
                    self.sel = 0;
                    self.focus_search = true;
                }
            }
        });
    }

    fn grid(
        &mut self,
        ui: &mut egui::Ui,
        cols: usize,
        cell: Vec2,
        colors: &PaletteColors,
        event: &mut UiEvent,
    ) {
        let count = self.core.result_count();
        self.a11y.begin_frame(); // before any return: a filtered-to-zero grid must not keep last frame's items/focus alive
        let now_ms = ui.input(|i| i.time) * 1000.0; // egui's own clock, not Instant::now() — deterministic and feedable in headless tests
        // `Config::load()` already zeroes every duration in `config.anim` when
        // `config.animations` is false (see store.rs), so no clamp is needed
        // here — this is the one code path for both cases, exactly as that
        // loader's own comment says it should be.
        self.board.config = self.core.config.anim;

        // GridSpec::rows_for/cell_pos don't read `rows_visible` at all (only
        // `cols` and `cell`) — it exists on the struct for callers that DO
        // need a viewport height (none here, since scrolling is the egui
        // ScrollArea's job, not the Board's). Set honestly rather than
        // faked: it's the total row count, not a "visible" subset, because
        // this call site has no cheaper true answer and nothing reads it.
        let spec = GridSpec { cols, rows_visible: count.div_ceil(cols.max(1)), cell: cell.x };

        // `order` mirrors `self.core.results()` exactly — Board diffs against
        // its OWN previous call's tiles by key, so calling `retarget` with an
        // unchanged order every frame is a correct, cheap no-op for anything
        // that hasn't moved; nothing here needs to detect "did it change".
        let order: Vec<String> = self.core.results().map(|e| e.id.clone()).collect();
        let order_refs: Vec<&str> = order.iter().map(String::as_str).collect();
        // Position within the CURRENT order, for keyboard-selection lookup —
        // Board's own iteration order appends leaving tiles after live ones,
        // so `self.board.poses()`'s index is not this.
        let live_pos: HashMap<&str, usize> =
            order_refs.iter().enumerate().map(|(n, id)| (*id, n)).collect();

        self.board.retarget(&order_refs, &spec, now_ms);
        if self.board.tick(now_ms) {
            ui.ctx().request_repaint(); // still moving — Board's own documented contract for this return value
        }
        if count == 0 && self.board.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| ui.weak("No matches"));
            return;
        }
        // Collected (not iterated live) so the loop below can hold `&mut
        // self` freely — `self.board.poses()` borrows `self.board`
        // immutably and every tile below needs `self.core`/`self.a11y`/etc.
        let poses: Vec<(String, TilePose)> =
            self.board.poses().map(|(k, p)| (k.to_string(), p)).collect();

        let menu_open = self.menu.is_some();
        let mut decode_budget = 3usize; // icons decoded per frame; keeps first paint under a frame
        let side = (ui.available_width() - cols as f32 * cell.x).max(0.0) / 2.0;
        let mut launch: Option<String> = None;
        let mut fav_toggle: Option<(String, egui::Pos2)> = None;
        self.tile_rects.clear();

        // Reserve the full scrollable content height up front — `Board`
        // positions tiles at absolute coordinates rather than through egui's
        // layout cursor, so nothing else advances it, and the ScrollArea
        // wrapping this call needs a real content size to know how far it
        // can scroll.
        let content_h = spec.content_height(count).max(cell.y);
        let (content_rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), content_h), Sense::hover());
        let origin = content_rect.min + Vec2::new(side, 0.0);

        for (id, pose) in &poses {
            let Some(e) = self.core.entry(id) else { continue }; // defensive only: Board never carries an id the core has never known
            let name = e.name.clone();
            let fav = self.core.is_favorite(id);
            let leaving = !live_pos.contains_key(id.as_str());
            let n = live_pos.get(id.as_str()).copied();
            let selected = n == Some(self.sel);

            let rect = Rect::from_min_size(origin + Vec2::new(pose.x, pose.y), cell);
            let tile_id = egui::Id::new(("hatch-tile", id.as_str()));
            let resp = ui.interact(rect, tile_id, if leaving { Sense::hover() } else { Sense::click() });

            if !leaving {
                self.tile_rects.push((id.clone(), rect));
                if resp.hovered() {
                    self.sel = n.expect("live_pos contains this id, checked above via `leaving`");
                }
            }

            let tex = self.texture_for(ui.ctx(), id, &mut decode_budget);
            self.tile(ui, rect, &resp, &name, tex, fav, selected, pose.scale, pose.alpha, colors);

            // A leaving tile is on its way out and un-selectable — no a11y
            // registration, no click handling, no favorite toggle. It is
            // still painted (that's the whole point of an exit fade) and
            // nothing more.
            if leaving {
                continue;
            }

            // Spoken form is always the full app name, regardless of how the
            // tile is drawn (icon vs. letter-tile fallback paint the same
            // either way) — see gitmind's tab-strip comment for why a visual
            // abbreviation must not shrink the spoken one too.
            self.a11y.add(
                Self::a11y_id(id),
                Semantics::new(Role::Button, &name)
                    .with_value(if fav { "favorite" } else { "" })
                    .with_hint("long-press to favorite"),
                Some(rect),
            );
            if selected {
                self.a11y.set_focus(Some(Self::a11y_id(id)));
            }

            match self.hold(&resp, id) {
                Hold::Fired => fav_toggle = Some((id.clone(), rect.center())),
                Hold::SwallowClick => {}
                Hold::Free => {
                    if resp.secondary_clicked() {
                        fav_toggle = Some((id.clone(), rect.center()));
                    } else if resp.clicked() && !menu_open {
                        launch = Some(id.clone());
                    }
                }
            }
        }
        if decode_budget == 0 {
            // More icons still undecoded — come back next frame.
            ui.ctx().request_repaint();
        }
        // Once nothing is held, the press is over: forget it and stop
        // swallowing. Done after the loop so a click delivered in the same
        // frame as the pointer-up is still seen (and still swallowed).
        if !ui.input(|i| i.pointer.any_down()) {
            self.press = None;
            self.swallow_click = false;
        }
        if let Some((id, at)) = fav_toggle {
            self.menu = Some((id, at));
        }
        if let Some(id) = launch {
            match self.core.activate(&id) {
                Ok(_) => *event = UiEvent::Launched,
                Err(e) => self.last_error = Some((format!("Launch failed: {e}"), Instant::now())),
            }
        }
    }

    /// One tile: press-squish, highlight, icon or letter tile, elided label,
    /// favorite star.
    #[allow(clippy::too_many_arguments)]
    fn tile(
        &self,
        ui: &egui::Ui,
        rect: Rect,
        resp: &egui::Response,
        name: &str,
        tex: Option<egui::TextureId>,
        fav: bool,
        selected: bool,
        pose_scale: f32,
        alpha: f32,
        colors: &PaletteColors,
    ) {
        let p = ui.painter();
        let pressed = resp.is_pointer_button_down_on();
        // The QML "squish": egui's animated bool gives the same 80ms feel.
        // Composed multiplicatively with the Board's arrival/exit scale —
        // the two are independent effects (one keyed to touch, one to
        // enter/restack/exit) and either can be 1.0 while the other moves.
        let t = ui.ctx().animate_bool_with_time(resp.id, pressed, 0.08);
        let press_scale = 1.0 - 0.10 * t;
        let scale = press_scale * pose_scale;
        // Rotation (`TilePose::rot`, used by the Scatter slide pattern) is
        // deliberately not applied here — painting a rotated `p.image()`
        // needs a transformed mesh, not just a differently-sized rect, and
        // that's real additional work. Every other pattern's `rot` is 0, so
        // this only mutes Scatter's flourish, not its position/timing.

        // Every app is a card, as in tideshell's window switcher: a raised rounded ground with a hairline,
        // so the launcher and the window manager read as one family. The selection wash goes on top.
        let card = rect.shrink(4.0);
        p.rect_filled(card, CornerRadius::same(14), colors.bg_raised.gamma_multiply(alpha));
        p.rect_stroke(card, CornerRadius::same(14), Stroke::new(1.0, colors.border.gamma_multiply(0.6 * alpha)), StrokeKind::Inside);
        if selected || resp.hovered() {
            let a = if selected { 70 } else { 40 };
            p.rect_filled(
                rect.shrink(3.0),
                CornerRadius::same(12),
                colors.accent.gamma_multiply(a as f32 / 255.0 * alpha),
            );
            if selected {
                p.rect_stroke(
                    rect.shrink(3.0),
                    CornerRadius::same(12),
                    Stroke::new(1.0, colors.accent.gamma_multiply(alpha)),
                    StrokeKind::Inside,
                );
            }
        }

        // Smaller than the cell asks for: the card carries the tile, the icon only labels it.
        let icon_base = self.core.config.icon_px as f32 * ICON_FRAC;
        let icon_px = icon_base * scale;
        let icon_center = egui::pos2(rect.center().x, rect.top() + 14.0 + icon_base / 2.0);
        let icon_rect = Rect::from_center_size(icon_center, Vec2::splat(icon_px));
        match tex {
            Some(id) => {
                p.image(
                    id,
                    icon_rect,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE.gamma_multiply(alpha),
                );
            }
            None => {
                // Same deterministic letter tile the rest of the fleet uses.
                let seed = uikit_egui::mono_tile::seed(name);
                let hue = uikit_egui::mono_tile::hue(name);
                let ground = uikit_egui::mono_tile::hsla(hue, 0.45, 0.28, 1.0).gamma_multiply(alpha);
                p.rect_filled(icon_rect, CornerRadius::same((icon_px * 0.24) as u8), ground);
                let ini = uikit_egui::mono_tile::initials(name);
                let bright = uikit_egui::mono_tile::hsla((hue + (seed % 40) as f32) % 360.0, 0.7, 0.8, 1.0)
                    .gamma_multiply(alpha);
                p.text(
                    icon_rect.center(),
                    Align2::CENTER_CENTER,
                    ini,
                    FontId::proportional(icon_px * 0.42),
                    bright,
                );
            }
        }
        if fav {
            p.text(
                icon_rect.right_top() + Vec2::new(2.0, -2.0),
                Align2::RIGHT_TOP,
                "★",
                FontId::proportional(13.0),
                colors.accent.gamma_multiply(alpha),
            );
        }

        // Measured against the tile, minus a little breathing room, so a
        // wide-glyph name can't run under its neighbours.
        let font = FontId::proportional(12.5);
        let label = Self::elide_fitting(ui, name, &font, rect.width() - 8.0);
        p.text(
            egui::pos2(rect.center().x, rect.bottom() - 14.0),
            Align2::CENTER_CENTER,
            label,
            font,
            ui.visuals().text_color().gamma_multiply(alpha),
        );
    }

    /// Long-press tracking for one response. egui reports presses and clicks;
    /// the hold state machine lives here: still + `LONG_PRESS` fires once,
    /// and the release click after a fire is swallowed.
    fn hold(&mut self, resp: &egui::Response, key: &str) -> Hold {
        if !resp.is_pointer_button_down_on() {
            // Not this tile's press. Release bookkeeping is central, in
            // `grid`, because by release time the press may not belong to any
            // tile still under the pointer.
            return if self.swallow_click { Hold::SwallowClick } else { Hold::Free };
        }
        let pos = resp.interact_pointer_pos();
        match &mut self.press {
            Some(t) if t.key == key => {
                if let Some(p) = pos {
                    if (p - t.origin).length() > LONG_PRESS_SLOP {
                        // Moved: it's a scroll/drag, never a hold and never a tap.
                        t.fired = true;
                        self.swallow_click = true;
                        return Hold::SwallowClick;
                    }
                }
                if !t.fired && t.started.elapsed() >= LONG_PRESS {
                    t.fired = true;
                    self.swallow_click = true;
                    return Hold::Fired;
                }
                // Keep frames coming so the hold can mature without input.
                resp.ctx.request_repaint_after(Duration::from_millis(60));
                if t.fired {
                    return Hold::SwallowClick;
                }
                Hold::Free
            }
            _ => {
                self.press = Some(PressTrack {
                    key: key.to_string(),
                    started: Instant::now(),
                    origin: pos.unwrap_or_default(),
                    fired: false,
                });
                resp.ctx.request_repaint_after(LONG_PRESS);
                Hold::Free
            }
        }
    }

    fn texture_for(
        &mut self,
        ctx: &egui::Context,
        entry_id: &str,
        budget: &mut usize,
    ) -> Option<egui::TextureId> {
        if let Some(cached) = self.icon_cache.get(entry_id) {
            return cached.as_ref().map(|t| t.id());
        }
        if *budget == 0 {
            return None; // undecoded this frame; caller requests a repaint
        }
        *budget -= 1;
        let px = self.core.config.icon_px * 2; // decode at 2x for crispness on hidpi
        let img = self
            .core
            .results()
            .find(|e| e.id == entry_id)
            .and_then(|e| self.core.icon_path(e))
            .and_then(|p| icons_img::decode(&p, px));
        let handle = img.map(|img| {
            ctx.load_texture(format!("icon:{entry_id}"), img, egui::TextureOptions::LINEAR)
        });
        let id = handle.as_ref().map(|t| t.id());
        self.icon_cache.insert(entry_id.to_string(), handle);
        id
    }

    fn sys_texture(&mut self, ctx: &egui::Context, icon_name: &str) -> Option<egui::TextureId> {
        let key = format!("sys:{icon_name}");
        if let Some(cached) = self.icon_cache.get(&key) {
            return cached.as_ref().map(|t| t.id());
        }
        let handle = self
            .core
            .icon_path_for_name(icon_name)
            .and_then(|p| icons_img::decode(&p, 64))
            .map(|img| ctx.load_texture(key.clone(), img, egui::TextureOptions::LINEAR));
        let id = handle.as_ref().map(|t| t.id());
        self.icon_cache.insert(key, handle);
        id
    }

    /// Returns true when a system action was launched.
    fn system_row(&mut self, ui: &mut egui::Ui, colors: &PaletteColors) -> bool {
        let actions: Vec<_> = self.core.system_actions().into_iter().collect();
        let mut run: Option<&'static str> = None;
        ui.horizontal(|ui| {
            let total = actions.len() as f32 * 52.0;
            ui.add_space((ui.available_width() - total).max(0.0) / 2.0);
            for a in &actions {
                let (rect, resp) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::click());
                let t = ui.ctx().animate_bool_with_time(resp.id, resp.is_pointer_button_down_on(), 0.08);
                let r = rect.shrink(2.0 + 3.0 * t);
                ui.painter().rect_filled(
                    r,
                    CornerRadius::same(24),
                    colors.accent.gamma_multiply(if resp.hovered() { 0.30 } else { 0.16 }),
                );
                match self.sys_texture(ui.ctx(), a.icon) {
                    // painter.image returns a ShapeIdx since egui 0.34 — drop it.
                    Some(id) => {
                        ui.painter().image(
                            id,
                            Rect::from_center_size(rect.center(), Vec2::splat(24.0)),
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    None => {
                        ui.painter().text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            a.label.chars().next().unwrap_or('?'),
                            FontId::proportional(18.0),
                            ui.visuals().text_color(),
                        );
                    }
                }
                let resp = resp.on_hover_text(a.label);
                if resp.clicked() {
                    run = Some(a.id);
                }
            }
        });
        if let Some(id) = run {
            match self.core.run_system_action(id) {
                Ok(_) => return true,
                Err(e) => self.last_error = Some((format!("{id}: {e}"), Instant::now())),
            }
        }
        false
    }

    /// Returns true when Enter launched the top/selected result.
    fn search_pill(&mut self, ui: &mut egui::Ui, h: f32, colors: &PaletteColors) -> bool {
        let w = ui.available_width().min(420.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        let p = ui.painter();
        p.rect_filled(
            rect,
            CornerRadius::same((h / 2.0) as u8),
            ui.visuals().extreme_bg_color.gamma_multiply(0.85),
        );
        p.rect_stroke(
            rect,
            CornerRadius::same((h / 2.0) as u8),
            Stroke::new(1.0, colors.accent.gamma_multiply(0.4)),
            StrokeKind::Inside,
        );
        let edit_rect = rect.shrink2(Vec2::new(18.0, 10.0));
        let resp = ui.put(
            edit_rect,
            egui::TextEdit::singleline(&mut self.query).id_salt("lib-self_query")
                .hint_text("Search")
                // 0.34: frame(bool) became frame(Frame); an empty Frame is
                // the old `false` (the pill behind us is the visual frame).
                .frame(egui::Frame::new())
                .desired_width(f32::INFINITY),
        );
        if self.focus_search {
            resp.request_focus();
            self.focus_search = false;
        }
        if resp.changed() {
            self.core.set_query(&self.query);
            self.sel = 0;
        }
        if resp.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            if let Some(e) = self.core.result_at(self.sel).or_else(|| self.core.result_at(0)) {
                let id = e.id.clone();
                match self.core.activate(&id) {
                    Ok(_) => return true,
                    Err(err) => self.last_error = Some((format!("Launch failed: {err}"), Instant::now())),
                }
            }
        }
        false
    }
}

enum Hold {
    /// No hold in play — a click is a click.
    Free,
    /// The hold just matured: toggle favorite now.
    Fired,
    /// A hold already fired (or the press became a drag): eat the click.
    SwallowClick,
}

#[cfg(test)]
mod tests {
    use super::*;
    use hatch_core::AppEntry;

    // ---- headless frame driver -------------------------------------------
    //
    // egui::Context::run needs no window, no GL, no eframe — it takes RawInput
    // and returns shapes. That makes the drawing path testable, which matters
    // more here than it sounds: layout code fails by PANICKING (bad index, id
    // collision, allocate on a zero-size ui), and until this harness existed
    // not one line of `HatchUi::ui` had ever been executed. Everything below
    // asserts on the returned UiEvent; the shapes are discarded. What it does
    // NOT prove is that the result looks right — only the phone can say that.

    struct Harness {
        ctx: egui::Context,
        app: HatchUi,
        palette: uikit_egui::PaletteColors,
        size: Vec2,
        /// Every text run painted by the last frame, with where it landed.
        /// Asserting on geometry alone proves a menu is *shaped* right; this
        /// proves it *says* the right thing, and lets a test click a button
        /// by its label instead of at a fraction down the popup that quietly
        /// stops being correct the next time the styling moves.
        texts: Vec<(String, Rect)>,
        /// Synthetic clock fed into `RawInput.time`, seconds. Without this,
        /// `RawInput.time: None` falls back to egui's own real-wallclock
        /// timer, and `Board`'s enter/restack/exit durations (hundreds of ms)
        /// would need real `sleep`s to ever complete in a test — slow, and
        /// the anti-pattern `anim.rs`'s own Board tests deliberately avoid
        /// with a synthetic `now_ms`. Advances a fixed 1/60s per `frame()`
        /// call, independent of how fast the test itself actually runs.
        time: f64,
    }

    /// Walk a frame's shapes for text. egui hands back the laid-out galleys,
    /// so this is what the user would read, not what we hoped we asked for.
    fn collect_text(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
        match shape {
            egui::Shape::Text(t) => {
                out.push((
                    t.galley.text().to_string(),
                    Rect::from_min_size(t.pos, t.galley.size()),
                ));
            }
            egui::Shape::Vec(shapes) => {
                for s in shapes {
                    collect_text(s, out);
                }
            }
            _ => {}
        }
    }

    impl Harness {
        fn new(entries: Vec<AppEntry>) -> Harness {
            Harness::with_config(hatch_core::Config::default(), entries)
        }

        /// Same as `new`, but with a caller-supplied config — needed for
        /// tests that exercise `config.anim`/`config.animations` themselves
        /// rather than accepting the default.
        fn with_config(config: hatch_core::Config, entries: Vec<AppEntry>) -> Harness {
            // Private config home: the frame driver toggles favorites and
            // records recents, which must not touch a real launcher's state.
            let tmp = std::env::temp_dir().join(format!("hatch-ui-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&tmp);
            std::env::set_var("XDG_CONFIG_HOME", &tmp);

            let core = hatch_core::Launcher::with_entries(config, entries);
            let mut app = HatchUi::new(core);
            // The demo notice strip is not a tile; the label checks scan every painted string.
            app.notices.clear();
            Harness {
                ctx: egui::Context::default(),
                app,
                palette: uikit_egui::default_palette(),
                size: Vec2::new(420.0, 900.0),
                texts: Vec::new(),
                time: 0.0,
            }
        }

        /// The rect of a painted label containing `needle`, if any.
        fn text_rect(&self, needle: &str) -> Option<Rect> {
            self.texts.iter().find(|(t, _)| t.contains(needle)).map(|(_, r)| *r)
        }

        fn has_text(&self, needle: &str) -> bool {
            self.text_rect(needle).is_some()
        }

        /// Click the middle of the label containing `needle`.
        fn click_text(&mut self, needle: &str) -> UiEvent {
            let r = self
                .text_rect(needle)
                .unwrap_or_else(|| panic!("no painted text containing {needle:?}"));
            self.click(r.center())
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> UiEvent {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::pos2(0.0, 0.0), self.size)),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            self.time += 1.0 / 60.0;
            let mut out = UiEvent::None;
            let app = &mut self.app;
            let palette = &self.palette;
            let full = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    out = app.ui(ui, palette);
                });
            });
            self.texts.clear();
            for clipped in &full.shapes {
                collect_text(&clipped.shape, &mut self.texts);
            }
            out
        }

        /// Drive frames until `Board` has nothing left animating, bounded —
        /// same idiom `hatch_core::anim`'s own tests use for convergence.
        /// Needed before any test reads a tile's position for a LATER,
        /// separate interaction: the very first frames after entries appear
        /// are mid-entrance (offset, partly invisible), and a position read
        /// then would go stale the moment the tile keeps moving.
        fn settle(&mut self) {
            for _ in 0..600 {
                // 600 * 1/60s = 10s of simulated time — generous headroom
                // over any real BoardConfig default (longest is exit at
                // 120ms, entrance stagger adds more but nowhere near this).
                if self.app.board.is_settled() {
                    return;
                }
                self.frame(vec![]);
            }
            panic!("grid never settled after 10s of simulated time");
        }

        /// Press-and-release at `pos` across two frames, the way a real click
        /// arrives. Returns the second frame's event, which is where egui
        /// reports the click.
        fn click(&mut self, pos: egui::Pos2) -> UiEvent {
            let down = vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ];
            self.frame(down);
            let up = vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }];
            self.frame(up)
        }

        fn key(&mut self, key: Key) -> UiEvent {
            self.frame(vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }])
        }
    }

    fn entry(name: &str) -> AppEntry {
        AppEntry {
            id: name.to_lowercase(),
            name: name.into(),
            description: String::new(),
            icon: String::new(), // no icon file → letter-tile path gets exercised
            exec: "true".into(), // activate() spawns /bin/true
            terminal: false,
            // One entry carries an action so the menu has something to list.
            actions: if name == "Maps" {
                vec![hatch_core::entry::DesktopAction {
                    id: "new-window".into(),
                    name: "New Window".into(),
                    exec: "true".into(),
                }]
            } else {
                Vec::new()
            },
            ..Default::default()
        }
    }

    fn some_apps() -> Vec<AppEntry> {
        ["Files", "Firefox", "Terminal", "Camera", "Maps", "Notes"]
            .iter()
            .map(|n| entry(n))
            .collect()
    }

    #[test]
    fn a_full_frame_renders_without_panicking() {
        let mut h = Harness::new(some_apps());
        assert_eq!(h.frame(vec![]), UiEvent::None);
        // Several frames: animations, the icon decode budget, and the
        // long-press timer all have per-frame state worth stepping through.
        for _ in 0..4 {
            h.frame(vec![]);
        }
    }

    #[test]
    fn an_empty_grid_renders_too() {
        // The no-results path is the one most likely to divide by zero or
        // index off the end, and the easiest to never look at by hand.
        let mut h = Harness::new(vec![]);
        h.frame(vec![]);
        h.frame(vec![]);
    }

    #[test]
    fn a_narrow_phone_viewport_still_lays_out() {
        let mut h = Harness::new(some_apps());
        h.size = Vec2::new(320.0, 480.0);
        h.frame(vec![]);
        h.frame(vec![]);
    }

    #[test]
    fn clicking_a_tile_launches_and_clicking_the_gap_does_nothing() {
        let mut h = Harness::new(some_apps());
        h.frame(vec![]); // first frame lays the grid out and records the rects

        // Aim at where a tile really landed rather than re-deriving the
        // layout in the test (which would pass even if the layout drifted).
        let camera = h.app.tile_rect("camera").expect("Camera tile was painted");
        assert_eq!(h.click(camera.center()), UiEvent::Launched, "a tap on a tile launches");

        h.app.on_shown();
        h.frame(vec![]);

        // Just below the occupied grid: empty space, still inside the
        // scrollable area, above the search pill. This is the regression the
        // whole project started from — a missed tap must not dismiss.
        let bounds = h.app.tiles_bounds().expect("some tiles painted");
        let gap = egui::pos2(bounds.center().x, bounds.bottom() + 8.0);
        assert!(
            h.app.tile_rects.iter().all(|(_, r)| !r.contains(gap)),
            "the gap point must genuinely miss every tile"
        );
        assert_eq!(h.click(gap), UiEvent::None, "a tap on empty grid space does nothing");
    }

    #[test]
    fn a_long_press_opens_the_menu_and_swallows_the_release_click() {
        // The hold machine is the fiddliest thing in the file: it has to fire
        // once, and then eat exactly the click that ends the same press —
        // otherwise every favorite toggle also launches the app.
        let mut h = Harness::new(some_apps());
        h.frame(vec![]);
        let maps = h.app.tile_rect("maps").expect("Maps tile was painted");
        let pos = maps.center();

        h.frame(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ]);
        assert!(!h.app.core.is_favorite("maps"), "not yet — the hold must mature");

        // Hold time is wall-clock, so the test waits it out rather than
        // injecting a fake clock; one 0.6s test is cheaper than a clock seam
        // that nothing else needs.
        std::thread::sleep(LONG_PRESS + Duration::from_millis(60));
        h.frame(vec![]); // a frame with the finger still down: the hold fires
        assert!(h.app.menu_open(), "long press opens the tile menu");
        assert!(!h.app.core.is_favorite("maps"), "the menu asks; it does not act");

        let released = h.frame(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]);
        assert_eq!(released, UiEvent::None, "the release must not also launch");
    }

    /// Long-press `entry_id` and leave the menu open.
    fn open_menu(h: &mut Harness, entry_id: &str) -> egui::Pos2 {
        h.frame(vec![]);
        h.settle(); // else `tile_rect` below can catch the tile mid-entrance, and the position goes stale before the click below even fires
        let rect = h.app.tile_rect(entry_id).expect("tile painted");
        let pos = rect.center();
        h.frame(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ]);
        std::thread::sleep(LONG_PRESS + Duration::from_millis(60));
        h.frame(vec![]);
        h.frame(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]);
        assert!(h.app.menu_open(), "menu is open");
        pos
    }

    #[test]
    fn the_menu_says_what_it_does_and_the_favorite_item_does_it() {
        let mut h = Harness::new(some_apps());
        open_menu(&mut h, "camera");
        h.frame(vec![]); // the menu needs a frame to lay its buttons out

        assert!(h.has_text("Camera"), "the menu is titled with the app");
        assert!(h.has_text("Add to favorites"), "and offers the toggle by name");
        assert!(!h.has_text("Remove from favorites"));

        h.click_text("Add to favorites");
        assert!(h.app.core.is_favorite("camera"), "clicking the label toggles the favorite");
        assert!(!h.app.menu_open(), "and closes the menu");

        // Re-opening must offer the inverse, or the menu is lying.
        open_menu(&mut h, "camera");
        h.frame(vec![]);
        assert!(h.has_text("Remove from favorites"));
    }

    #[test]
    fn a_click_outside_dismisses_the_menu_without_launching() {
        let mut h = Harness::new(some_apps());
        open_menu(&mut h, "camera");
        h.frame(vec![]);

        // Bottom-left corner: away from the menu and from every tile.
        let away = egui::pos2(14.0, h.size.y - 140.0);
        assert_eq!(h.click(away), UiEvent::None, "dismissal is not a launch");
        assert!(!h.app.menu_open());
        assert!(!h.app.core.is_favorite("camera"), "and nothing was toggled");
    }

    #[test]
    fn a_menu_lists_the_entry_s_desktop_actions_and_invents_none() {
        // Maps is the fixture carrying a [Desktop Action]; the others don't.
        let mut h = Harness::new(some_apps());
        open_menu(&mut h, "maps");
        h.frame(vec![]);
        assert!(h.has_text("New Window"), "the action appears under its own name");

        h.app.on_shown();
        open_menu(&mut h, "camera");
        h.frame(vec![]);
        assert!(!h.has_text("New Window"), "and only for the entry that declares it");
    }

    #[test]
    fn choosing_a_desktop_action_launches_it() {
        let mut h = Harness::new(some_apps());
        open_menu(&mut h, "maps");
        h.frame(vec![]);
        assert_eq!(h.click_text("New Window"), UiEvent::Launched);
        assert!(!h.app.menu_open());
        assert!(!h.app.core.is_favorite("maps"), "an action is not a favorite toggle");
    }

    #[test]
    fn escape_closes_the_menu_before_it_closes_the_launcher() {
        let mut h = Harness::new(some_apps());
        open_menu(&mut h, "camera");
        assert_eq!(h.key(Key::Escape), UiEvent::None, "the launcher stays put");
        assert!(!h.app.menu_open());
        assert_eq!(h.key(Key::Escape), UiEvent::Dismiss, "a second Escape dismisses");
    }

    #[test]
    fn the_grid_paints_app_names_and_says_so_when_there_are_none() {
        let mut h = Harness::new(some_apps());
        h.frame(vec![]);
        assert!(h.has_text("Firefox"), "tiles are labelled");
        assert!(h.has_text("Search"), "the search pill shows its hint");

        h.frame(vec![egui::Event::Text("fi".into())]);
        h.frame(vec![]);
        assert!(h.has_text("Files") && h.has_text("Firefox"));
        assert!(!h.has_text("Terminal"), "filtered-out apps stop being painted");

        h.frame(vec![egui::Event::Text("zzz".into())]);
        h.settle(); // the filtered-out tiles exit-fade rather than vanish; wait for that to finish
        h.frame(vec![]);
        assert!(h.has_text("No matches"), "the empty state explains itself");
    }

    #[test]
    fn a_failed_launch_is_shown_rather_than_swallowed() {
        // A launcher that silently does nothing when Exec is broken is the
        // worst possible failure: the user taps again, harder.
        let mut broken = entry("Broken");
        broken.exec = "hatch-no-such-binary-exists".into();
        let mut h = Harness::new(vec![broken]);
        h.frame(vec![]);

        let tile = h.app.tile_rect("broken").expect("tile painted");
        assert_eq!(h.click(tile.center()), UiEvent::None, "a failed launch does not hide the window");
        h.frame(vec![]);
        assert!(h.has_text("Launch failed"), "and says so on screen");
    }

    #[test]
    fn the_notice_strip_shows_every_kind_and_a_tap_dismisses_one() {
        let mut h = Harness::new(vec![]);
        h.app.notices = hatch_core::notices::demo();
        h.frame(vec![]);
        for needle in ["Incoming call", "Game invite", "3 new photos", "Note"] {
            assert!(h.has_text(needle), "{needle} is painted");
        }
        let r = h.text_rect("Incoming call").expect("painted");
        h.click(r.center());
        assert!(!h.app.notices.iter().any(|n| n.id == 1), "tapping the card dismissed it");
    }

    #[test]
    fn a_long_app_name_stays_inside_its_tile() {
        // Elision is a character count against a proportional font, which is
        // an estimate — this is the check that the estimate holds for the
        // wide-glyph worst case rather than just for "Firefox".
        let mut h = Harness::new(vec![
            entry("WWWWWWWWWWWWWWWWWWWWWWWWWWWW"),
            entry("Mozilla Firefox Developer Edition Nightly"),
        ]);
        h.frame(vec![]);

        let cell_w = h.app.tile_rect("wwwwwwwwwwwwwwwwwwwwwwwwwwww").unwrap().width();
        // Measured elision must still elide in the MIDDLE, matching the QML
        // label — the tail is often the distinguishing part of a name.
        let long = h
            .texts
            .iter()
            .find(|(t, _)| t.starts_with("Mozilla"))
            .map(|(t, _)| t.clone())
            .expect("the long name was painted");
        assert!(long.contains('…') && long.ends_with('y'), "kept the tail: {long:?}");
        for (text, rect) in &h.texts {
            if text == "Search" || text.is_empty() {
                continue;
            }
            assert!(
                rect.width() <= cell_w,
                "label {text:?} is {} wide in a {cell_w} tile",
                rect.width()
            );
        }
    }

    #[test]
    fn escape_clears_a_query_first_and_dismisses_only_when_empty() {
        let mut h = Harness::new(some_apps());
        h.frame(vec![]);
        h.frame(vec![egui::Event::Text("fi".into())]);
        assert_eq!(h.app.core.query(), "fi", "typing reaches the search pill");

        assert_eq!(h.key(Key::Escape), UiEvent::None, "first Escape only clears");
        assert_eq!(h.app.core.query(), "");
        assert_eq!(h.key(Key::Escape), UiEvent::Dismiss, "second Escape dismisses");
    }

    #[test]
    fn typing_narrows_the_rendered_grid() {
        let mut h = Harness::new(some_apps());
        h.frame(vec![]);
        assert_eq!(h.app.core.result_count(), 6);
        h.frame(vec![egui::Event::Text("fi".into())]);
        h.frame(vec![]);
        assert_eq!(h.app.core.result_count(), 2, "Files and Firefox");
    }

    #[test]
    fn reopening_resets_query_and_selection() {
        let mut h = Harness::new(some_apps());
        h.frame(vec![]);
        h.frame(vec![egui::Event::Text("term".into())]);
        h.app.on_shown();
        h.frame(vec![]);
        assert_eq!(h.app.core.query(), "", "a re-summoned launcher starts clean");
        assert_eq!(h.app.core.result_count(), 6);
    }

    // ---- pure helpers -----------------------------------------------------

    #[test]
    fn columns_come_from_config_when_set_and_from_width_when_not() {
        assert_eq!(HatchUi::grid_cols(1000.0, 100.0, 5), 5, "config wins");
        assert_eq!(HatchUi::grid_cols(1000.0, 100.0, 0), 10);
        assert_eq!(HatchUi::grid_cols(90.0, 100.0, 0), 2, "never below 2");
    }

    #[test]
    fn eliding_keeps_head_and_tail_visible() {
        assert_eq!(HatchUi::elide("Firefox", 10), "Firefox");
        let e = HatchUi::elide("Firefox Developer Edition", 12);
        assert_eq!(e.chars().count(), 12);
        assert!(e.starts_with("Firefo") && e.ends_with("ition"), "{e}");
        assert!(e.contains('…'));
    }

    #[test]
    fn selection_moves_like_a_grid_and_clamps_at_edges() {
        // 7 items in 3 columns:  0 1 2 / 3 4 5 / 6
        let cols = 3;
        assert_eq!(HatchUi::move_sel(0, 7, cols, Key::ArrowRight), 1);
        assert_eq!(HatchUi::move_sel(2, 7, cols, Key::ArrowRight), 2, "no wrap at row edge");
        assert_eq!(HatchUi::move_sel(3, 7, cols, Key::ArrowLeft), 3, "no wrap back either");
        assert_eq!(HatchUi::move_sel(2, 7, cols, Key::ArrowDown), 5);
        assert_eq!(HatchUi::move_sel(5, 7, cols, Key::ArrowDown), 6, "partial row clamps to last");
        assert_eq!(HatchUi::move_sel(6, 7, cols, Key::ArrowDown), 6, "last row stays");
        assert_eq!(HatchUi::move_sel(0, 7, cols, Key::ArrowLeft), 0);
        assert_eq!(HatchUi::move_sel(1, 7, cols, Key::ArrowUp), 1, "top row stays");
        assert_eq!(HatchUi::move_sel(6, 7, cols, Key::ArrowUp), 3);
        assert_eq!(HatchUi::move_sel(3, 0, cols, Key::ArrowDown), 0, "empty grid is index 0");
    }

    #[test]
    fn a11y_registers_every_tile_and_tracks_selection() {
        let mut h = Harness::new(some_apps());
        h.frame(vec![]);

        // Every tile actually painted this frame has a matching spoken
        // registration, keyed stably by entry id.
        let registered: std::collections::HashSet<u64> = h
            .app
            .tile_rects
            .iter()
            .map(|(id, _)| HatchUi::a11y_id(id))
            .collect();
        for id in &registered {
            assert!(
                h.app.tile_rects.iter().any(|(tid, _)| HatchUi::a11y_id(tid) == *id),
                "every registered id traces back to a painted tile"
            );
        }
        assert_eq!(registered.len(), h.app.tile_rects.len(), "no id collisions across the painted set");

        // `sel` starts at 0; a11y focus must follow whichever entry that
        // actually is, not a hardcoded id — the ranking (favorites/recents)
        // decides that, not this test.
        let first = h.app.core.result_at(0).expect("some apps").id.clone();
        assert_eq!(
            h.app.a11y.focused().map(|it| it.id),
            Some(HatchUi::a11y_id(&first)),
            "a11y focus follows `sel`, not a hardcoded id"
        );

        // Moving selection with the keyboard moves a11y focus the same way —
        // the sync happens through `grid()` every frame, not once at startup.
        h.key(Key::ArrowRight);
        let second = h.app.core.result_at(1).expect("at least two apps").id.clone();
        assert_eq!(h.app.a11y.focused().map(|it| it.id), Some(HatchUi::a11y_id(&second)));
    }

    #[test]
    fn a11y_scene_is_empty_on_a_query_that_matches_nothing() {
        let mut h = Harness::new(some_apps());
        h.app.core.set_query("zzz_no_such_app");
        h.frame(vec![]);
        assert!(h.app.a11y.focused().is_none(), "no stale focus from before the filter narrowed to zero");
    }

    #[test]
    fn the_grid_actually_animates_in_on_first_paint() {
        // The point of the whole Board-wiring change: it must be genuinely
        // driven, not decorative. A fresh grid's very first frame should NOT
        // already show every tile at rest — if it does, nothing is animating.
        let mut h = Harness::new(some_apps());
        h.frame(vec![]);
        assert!(!h.app.board.is_settled(), "the default pattern (FromBelow) must not land everything instantly");
        h.settle();
        assert!(h.app.board.is_settled(), "and it must actually converge");
    }

    #[test]
    fn zero_duration_config_lands_every_tile_on_the_first_frame() {
        // Config::load()'s own documented contract: animations=false means
        // durations of zero, one code path, not a bypass. Exercised at the
        // SHELL level, not just hatch-core's — the point here is that
        // hatch-egui actually reads `config.anim` into `Board` every frame.
        let mut cfg = hatch_core::Config::default();
        cfg.anim.enter_ms = 0.0;
        cfg.anim.stagger_ms = 0.0;
        cfg.anim.restack_ms = 0.0;
        cfg.anim.restack_stagger_ms = 0.0;
        cfg.anim.exit_ms = 0.0;
        let mut h = Harness::with_config(cfg, some_apps());
        h.frame(vec![]);
        assert!(h.app.board.is_settled(), "zero durations must settle on the very first frame, no wait needed");
    }

    #[test]
    fn filtering_out_a_tile_fades_it_rather_than_vanishing_it_instantly() {
        let mut h = Harness::new(some_apps());
        h.frame(vec![]);
        h.settle();
        assert!(h.has_text("Terminal"));

        h.frame(vec![egui::Event::Text("fi".into())]); // narrows away Camera and Terminal
        h.frame(vec![]); // the very next frame: still fading, not yet gone
        assert!(
            h.app.board.poses().any(|(k, _)| k == "terminal"),
            "a filtered-out tile stays on the board mid-fade, not removed the same frame it dropped out of the query"
        );

        h.settle();
        assert!(
            !h.app.board.poses().any(|(k, _)| k == "terminal"),
            "and is gone once its fade actually finishes"
        );
    }
}
