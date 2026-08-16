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

use egui::{Align2, Color32, CornerRadius, FontId, Key, Rect, Sense, Stroke, StrokeKind, Vec2};
use hatch_core::Launcher;
use uikit_egui::PaletteColors;

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
    query: String,
    /// Keyboard selection into the current results; drawn as the highlight
    /// and launched by Enter. Reset on query change.
    sel: usize,
    focus_search: bool,
    /// Bumping this resalts the ScrollArea, which resets its offset — the
    /// "re-opened launchers start at the top" rule.
    scroll_gen: u64,
    icon_cache: HashMap<String, Option<egui::TextureHandle>>,
    // Long-press bookkeeping (egui has clicks and drags, not holds).
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
}

struct PressTrack {
    key: String,
    started: Instant,
    origin: egui::Pos2,
    fired: bool,
}

impl HatchUi {
    pub fn new(core: Launcher) -> Self {
        HatchUi {
            core,
            query: String::new(),
            sel: 0,
            focus_search: true,
            scroll_gen: 0,
            icon_cache: HashMap::new(),
            press: None,
            swallow_click: false,
            grabber_drag: 0.0,
            last_error: None,
            tile_rects: Vec::new(),
        }
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
        // A re-summoned launcher is the natural moment for favorites toggled
        // last time to take their new places.
        self.core.resettle();
        self.sel = 0;
        self.scroll_gen += 1;
        self.focus_search = true;
        self.grabber_drag = 0.0;
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
    /// ("Fir…fox Nightly" beats "Firefox Nig…").
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
        let cell = Vec2::new(cfg_icon + 44.0, cfg_icon + 40.0);

        // Reserve the bottom stack first so the grid can take the remainder.
        let sys_actions = self.core.system_actions();
        let sys_h = if sys_actions.is_empty() { 0.0 } else { 56.0 };
        let search_h = 52.0;
        let grabber_h = 26.0;
        let grid_h =
            (ui.available_height() - grabber_h - sys_h - search_h - 16.0).max(cell.y);

        if self.grabber(ui, grabber_h) {
            event = UiEvent::Dismiss;
        }

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
        });

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
                if self.query.is_empty() {
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
        if count == 0 {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| ui.weak("No matches"));
            return;
        }
        let mut decode_budget = 3usize; // icons decoded per frame; keeps first paint under a frame
        let side = (ui.available_width() - cols as f32 * cell.x).max(0.0) / 2.0;
        let rows = count.div_ceil(cols);
        let mut launch: Option<String> = None;
        let mut fav_toggle: Option<String> = None;
        self.tile_rects.clear();

        for row in 0..rows {
            ui.horizontal(|ui| {
                ui.add_space(side);
                for col in 0..cols {
                    let n = row * cols + col;
                    let Some(e) = self.core.result_at(n) else { break };
                    let id = e.id.clone();
                    let name = e.name.clone();
                    let fav = self.core.is_favorite(&id);
                    let tex = self.texture_for(ui.ctx(), &id, &mut decode_budget);
                    let selected = n == self.sel;

                    let (rect, resp) =
                        ui.allocate_exact_size(cell, Sense::click());
                    self.tile_rects.push((id.clone(), rect));
                    if resp.hovered() {
                        self.sel = n;
                    }
                    self.tile(ui, rect, &resp, &name, tex, fav, selected, colors);

                    match self.hold(&resp, &id) {
                        Hold::Fired => fav_toggle = Some(id),
                        Hold::SwallowClick => {}
                        Hold::Free => {
                            if resp.clicked() {
                                launch = Some(id);
                            }
                        }
                    }
                }
            });
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
        if let Some(id) = fav_toggle {
            self.core.toggle_favorite(&id);
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
        colors: &PaletteColors,
    ) {
        let p = ui.painter();
        let pressed = resp.is_pointer_button_down_on();
        // The QML "squish": egui's animated bool gives the same 80ms feel.
        let t = ui.ctx().animate_bool_with_time(resp.id, pressed, 0.08);
        let scale = 1.0 - 0.10 * t;

        if selected || resp.hovered() {
            let a = if selected { 70 } else { 40 };
            p.rect_filled(
                rect.shrink(3.0),
                CornerRadius::same(12),
                colors.accent.gamma_multiply(a as f32 / 255.0),
            );
            if selected {
                p.rect_stroke(
                    rect.shrink(3.0),
                    CornerRadius::same(12),
                    Stroke::new(1.0, colors.accent),
                    StrokeKind::Inside,
                );
            }
        }

        let icon_px = self.core.config.icon_px as f32 * scale;
        let icon_center = egui::pos2(rect.center().x, rect.top() + 8.0 + self.core.config.icon_px as f32 / 2.0);
        let icon_rect = Rect::from_center_size(icon_center, Vec2::splat(icon_px));
        match tex {
            Some(id) => {
                p.image(
                    id,
                    icon_rect,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                // Same deterministic letter tile the rest of the fleet uses.
                let seed = uikit_egui::mono_tile::seed(name);
                let hue = uikit_egui::mono_tile::hue(name);
                let ground = uikit_egui::mono_tile::hsla(hue, 0.45, 0.28, 1.0);
                p.rect_filled(icon_rect, CornerRadius::same((icon_px * 0.24) as u8), ground);
                let ini = uikit_egui::mono_tile::initials(name);
                let bright = uikit_egui::mono_tile::hsla((hue + (seed % 40) as f32) % 360.0, 0.7, 0.8, 1.0);
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
                colors.accent,
            );
        }

        let label = Self::elide(name, (rect.width() / 7.5) as usize);
        p.text(
            egui::pos2(rect.center().x, rect.bottom() - 14.0),
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(12.5),
            ui.visuals().text_color(),
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
            egui::TextEdit::singleline(&mut self.query)
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
    use std::path::PathBuf;

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
    }

    impl Harness {
        fn new(entries: Vec<AppEntry>) -> Harness {
            // Private config home: the frame driver toggles favorites and
            // records recents, which must not touch a real launcher's state.
            let tmp = std::env::temp_dir().join(format!("hatch-ui-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&tmp);
            std::env::set_var("XDG_CONFIG_HOME", &tmp);

            let core = hatch_core::Launcher::with_entries(hatch_core::Config::default(), entries);
            Harness {
                ctx: egui::Context::default(),
                app: HatchUi::new(core),
                palette: uikit_egui::default_palette(),
                size: Vec2::new(420.0, 900.0),
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> UiEvent {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::pos2(0.0, 0.0), self.size)),
                events,
                ..Default::default()
            };
            let mut out = UiEvent::None;
            let app = &mut self.app;
            let palette = &self.palette;
            self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    out = app.ui(ui, palette);
                });
            });
            out
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
            path: PathBuf::new(),
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
    fn a_long_press_favorites_the_tile_and_swallows_the_release_click() {
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
        assert!(h.app.core.is_favorite("maps"), "long press toggled the favorite");

        let released = h.frame(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]);
        assert_eq!(released, UiEvent::None, "the release must not also launch");
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
}
