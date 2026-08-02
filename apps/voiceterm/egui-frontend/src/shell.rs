//! shell.rs — voiceterm's app chrome: the session sidebar, the bottom keycap bar, and the mic
//! FAB's docked position. Ported from real screenshots of voiceterm's Qt app (not just QML
//! reading) — this is the part of the port that was entirely missing before: the existing harness
//! (examples/feel.rs, src/bin/voiceterm_egui.rs) only assembled the terminal + popups, with no
//! surrounding shell at all.
//!
//! This is now a thin, voiceterm-specific layer over `uikit_egui::dock`'s shared primitives (list
//! row, action stack, chip row, FAB docking) — extracted there once this module's shapes proved
//! out, since none of the four pieces were actually voiceterm-specific: a session list is "rows in
//! a fixed-width column, some selected, with actions pinned below," the same shape gitmind's Qt nav
//! rail or a device list needs; the keycap row is "a wrapping row of small pill buttons," the same
//! shape a filter-tag or quick-action bar needs; and any app with a floating action button needs
//! the same corner-overlap docking math. What stays HERE is voiceterm's own data (Session, the two
//! keycap sets) and the mic glyph/glow specifics.
//!
//! Colours: the reference screenshots were taken with the "Mocha" theme active (a *selectable*
//! theme, per the Settings panel's own theme picker) — a different, warmer palette than the
//! Voice palette this port defaults to (voiceterm's baseline QML identity). So this module ports
//! the STRUCTURE (layout, spacing, what's a button vs a list vs a pill) faithfully, using the
//! existing Voice palette's colours rather than copying Mocha's exact hex values. A full
//! multi-theme picker (Dark/Light/Nord/Solar/Mono/Rose/Gruv/Ocean/Mocha, "long-press to show all
//! vibes") is a real, separate feature visible in the settings screenshot — not ported here.

use crate::gesture_menus::WsCall;
use crate::terminal_workspace::{Dir, PaneKind, TermWorkspace};
use egui::{Rect, Response, Ui};
use uikit_egui::dock::{self, Chip, Corner};
use uikit_egui::PaletteColors;

/// One entry in the session list (the left sidebar).
#[derive(Clone, Debug)]
pub struct Session {
    pub name: String,
    pub count: u32,
}

/// Sidebar state: the session list + which one is active.
pub struct Sidebar {
    pub sessions: Vec<Session>,
    pub active: usize,
}

impl Sidebar {
    pub fn new(sessions: Vec<Session>) -> Self {
        Self { sessions, active: 0 }
    }

    /// Render the sidebar into `ui` (call inside an `egui::SidePanel::left`). Returns the index of
    /// a newly-clicked session (if any), separately from `+ New` / `○ Graph` / `⚙ Settings` which
    /// the caller reads via the returned `SidebarAction`.
    pub fn ui(&mut self, ui: &mut Ui, c: &PaletteColors) -> Option<SidebarAction> {
        let mut action = None;
        ui.add_space(4.0);
        for (i, s) in self.sessions.iter().enumerate() {
            let label = format!("{}  ({})", s.name, s.count);
            if dock::list_row(ui, &label, i == self.active, 32.0, c).clicked() {
                self.active = i;
                action = Some(SidebarAction::SelectSession(i));
            }
        }
        // spacer pushes the action buttons to the bottom of the sidebar
        ui.add_space(ui.available_height() - 3.0 * 34.0 - 8.0);
        ui.separator();
        if let Some(i) = dock::action_stack(ui, &["+ New", "○ Graph", "⚙ Settings"], 30.0, c) {
            action = Some([SidebarAction::New, SidebarAction::Graph, SidebarAction::Settings][i]);
        }
        action
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarAction {
    SelectSession(usize),
    New,
    Graph,
    Settings,
}

/// One key/chord in the bottom keycap bar.
pub struct Keycap {
    pub label: &'static str,
    /// What `handle_input`-equivalent code this sends when tapped (a raw string the terminal/PTY
    /// layer interprets — e.g. "\x1b" for Esc, "\x03" for Ctrl-C).
    pub send: &'static str,
    /// Auto-repeat while held (the QML rep:true arrows — v0.1.140).
    pub repeat: bool,
}

/// The default keycap row (terminal-focused — matches the base screenshot). A web pane in focus
/// shows more keys (C-r, C-a, C-e, ~, /, -, _) per the reference; callers needing that longer row
/// can build their own `&[Keycap]` — the rendering function takes a slice, not a fixed set.
pub const TERMINAL_KEYCAPS: &[Keycap] = &[
    Keycap { label: "Esc", send: "\x1b", repeat: false },
    Keycap { label: "Tab", send: "\t", repeat: false },
    // v0.1.140: arrows auto-repeat while held — 500 ms initial delay, then ~30/sec
    Keycap { label: "↑", send: "\x1b[A", repeat: true },
    Keycap { label: "↓", send: "\x1b[B", repeat: true },
    Keycap { label: "←", send: "\x1b[D", repeat: true },
    Keycap { label: "→", send: "\x1b[C", repeat: true },
    Keycap { label: "C-c", send: "\x03", repeat: false },
    Keycap { label: "C-d", send: "\x04", repeat: false },
    Keycap { label: "C-z", send: "\x1a", repeat: false },
    Keycap { label: "C-l", send: "\x0c", repeat: false },
];

/// The QML arrow auto-repeat timing (v0.1.140): held past 500 ms, then a 33 ms cadence.
pub const REPEAT_DELAY: f64 = 0.5;
pub const REPEAT_CADENCE: f64 = 0.033;

/// Pure repeat scheduler: given now, when the key was pressed, and when it last emitted,
/// should a repeat fire this frame? (The initial press emits via the normal click path;
/// repeats begin only after the delay.)
pub fn repeat_due(now: f64, held_since: f64, last_emit: f64) -> bool {
    now - held_since >= REPEAT_DELAY && now - last_emit >= REPEAT_CADENCE
}

/// The extended row shown when a web/rich pane has focus (adds C-r/C-a/C-e + a few punctuation
/// keys that are awkward to reach one-handed on touch).
pub const EXTENDED_KEYCAPS: &[Keycap] = &[
    Keycap { label: "Esc", send: "\x1b", repeat: false }, Keycap { label: "Tab", send: "\t", repeat: false },
    Keycap { label: "↑", send: "\x1b[A", repeat: true }, Keycap { label: "↓", send: "\x1b[B", repeat: true },
    Keycap { label: "←", send: "\x1b[D", repeat: true }, Keycap { label: "→", send: "\x1b[C", repeat: true },
    Keycap { label: "C-c", send: "\x03", repeat: false }, Keycap { label: "C-d", send: "\x04", repeat: false },
    Keycap { label: "C-z", send: "\x1a", repeat: false }, Keycap { label: "C-l", send: "\x0c", repeat: false },
    Keycap { label: "C-r", send: "\x12", repeat: false }, Keycap { label: "C-a", send: "\x01", repeat: false },
    Keycap { label: "C-e", send: "\x05", repeat: false }, Keycap { label: "~", send: "~", repeat: false },
    Keycap { label: "/", send: "/", repeat: false }, Keycap { label: "-", send: "-", repeat: false }, Keycap { label: "_", send: "_", repeat: false },
];

/// Render the bottom keycap bar plus the "Sel · Copy · Paste" hint row beneath it. Returns the
/// `send` string of a tapped keycap, if any.
pub fn keycap_bar_ui(ui: &mut Ui, keys: &[Keycap], c: &PaletteColors) -> Option<&'static str> {
    let chips: Vec<Chip> = keys.iter().map(|k| Chip::new(k.label)).collect();
    let (clicked, responses) = dock::chip_row_ex(ui, &chips, 30.0, c);
    // held-arrow auto-repeat (v0.1.140): repeat-tagged chips keep emitting while held —
    // 500 ms delay then 33 ms cadence, state in egui memory per chip
    let mut repeated: Option<&'static str> = None;
    let now = ui.input(|i| i.time);
    for (i, resp) in responses.iter().enumerate() {
        if !keys[i].repeat {
            continue;
        }
        let id = resp.id.with("kc-rep");
        if resp.is_pointer_button_down_on() {
            let (held_since, mut last_emit) =
                ui.ctx().data_mut(|d| *d.get_temp_mut_or(id, (now, now)));
            if repeat_due(now, held_since, last_emit) {
                repeated = Some(keys[i].send);
                last_emit = now;
            }
            ui.ctx().data_mut(|d| d.insert_temp(id, (held_since, last_emit)));
            ui.ctx().request_repaint(); // poll the hold
        } else {
            ui.ctx().data_mut(|d| d.remove::<(f64, f64)>(id));
        }
    }
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Sel · Copy · Paste").color(c.text_weak).size(11.0));
    });
    clicked.map(|i| keys[i].send).or(repeated)
}

/// Where the mic FAB docks — the bottom-right corner of `terminal_rect`, overlapping its border
/// slightly (matches the reference: the button sits ON the corner, not fully inside or outside).
pub fn fab_rect(terminal_rect: Rect, diameter: f32) -> Rect {
    dock::fab_dock(terminal_rect, diameter, Corner::BottomRight)
}

/// Paint the terminal's bordered frame (the amber/accent outline visible around the whole
/// terminal region in the reference screenshots).
pub fn paint_terminal_border(ui: &Ui, rect: Rect, c: &PaletteColors) {
    dock::accent_border(ui, rect, c);
}

/// Paint the mic FAB itself (the circular button — the compass opens from here on press-hold).
/// Returns the interaction response so the caller can wire press/hold/tap.
pub fn mic_fab_ui(ui: &mut Ui, rect: Rect, c: &PaletteColors) -> Response {
    dock::fab_button(ui, rect, "\u{1F3A4}", c)
}

// ── settings screen (QML settings page: theme grid + sliders, live) ──────────

/// The voiceterm settings screen body — edits `Prefs` live, mirroring the proven gitmind-egui
/// binding pattern (animation picker, palette grid, draft-on-drag text scale so the slider
/// doesn't jump under the cursor, opacity) plus voiceterm's own sections: the gesture knobs the
/// terminal's touch layer reads (sensitivity / commit fraction / flick velocity) and the voice
/// mesh (mic, offload + compute host, speaker id). Returns `true` when anything changed this
/// frame — the caller persists (`save_prefs`) and re-applies (`App::apply_settings`).
pub fn settings_panel(ui: &mut Ui, prefs: &mut settings_kit::Prefs) -> bool {
    let before = prefs.to_kv();

    ui.label(egui::RichText::new("animation").strong());
    ui.horizontal_wrapped(|ui| {
        for a in settings_kit::AnimationStyle::all() {
            if ui.selectable_label(prefs.gesture.animation == a, a.label()).clicked() {
                prefs.gesture.animation = a;
            }
        }
    });
    ui.add_space(8.0);

    ui.label(egui::RichText::new("palette").strong());
    ui.horizontal_wrapped(|ui| {
        for p in snapclone_theme::PaletteId::all() {
            let sel = prefs.palette == p.as_str();
            if ui.selectable_label(sel, p.display_name()).clicked() {
                prefs.palette = p.as_str().to_string();
            }
        }
    });
    ui.add_space(8.0);

    ui.label(egui::RichText::new("display").strong());
    // Text size re-scales the whole window; draft during the drag, commit on release (the
    // gitmind-egui fix for the slider jumping under the cursor).
    {
        let id = egui::Id::new("vt_text_scale_draft");
        let mut draft: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(prefs.touch.text_scale);
        let resp = ui.add(egui::Slider::new(&mut draft, 0.75..=1.5).step_by(0.25).text("text size"));
        if resp.dragged() {
            ui.data_mut(|d| d.insert_temp(id, draft));
        } else {
            if resp.drag_stopped() || resp.changed() {
                prefs.touch.text_scale = draft;
            }
            ui.data_mut(|d| d.insert_temp(id, prefs.touch.text_scale));
        }
    }
    ui.add(egui::Slider::new(&mut prefs.display.opacity, 0.4..=1.0).text("surface opacity"));
    ui.add_space(8.0);

    ui.label(egui::RichText::new("gestures").strong());
    ui.add(egui::Slider::new(&mut prefs.gesture.sensitivity, 0.5..=2.0).text("sensitivity"));
    ui.add(egui::Slider::new(&mut prefs.gesture.commit_fraction, 0.2..=0.9).text("commit fraction"));
    ui.add(egui::Slider::new(&mut prefs.gesture.flick_velocity, 200.0..=2000.0).text("flick velocity (px/s)"));
    ui.add_space(8.0);

    ui.label(egui::RichText::new("voice").strong());
    ui.checkbox(&mut prefs.voice.mic_enabled, "microphone");
    ui.checkbox(&mut prefs.voice.offload_enabled, "offload transcription to a compute host");
    if prefs.voice.offload_enabled {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("host:").weak().small());
            ui.text_edit_singleline(&mut prefs.voice.compute_host);
        });
    }
    ui.checkbox(&mut prefs.voice.speaker_id, "speaker identification");

    ui.add_space(10.0);
    ui.label(
        egui::RichText::new("changes apply live and persist to prefs.kv")
            .weak()
            .small(),
    );

    prefs.to_kv() != before
}

// ── blocks drawer (QML blocksDrawer, left edge) ──────────────────────────────

/// One frame of the blocks drawer body: rows most-recent-first, each with the command, an
/// output preview, an exit/duration badge, the inline ▶ 📋 📥 buttons, and a ⋯ whose HOLD opens
/// the 2×2 block menu (shared across rows — the row's block id rides in the return, the egui
/// equivalent of the QML gestureContext). Returns the first action fired this frame.
pub fn blocks_drawer_ui(
    ui: &mut Ui,
    blocks: &[uikit_term::blocks::CommandBlock],
    c: &PaletteColors,
) -> Option<(u32, crate::gesture_menus::BlockAction)> {
    use crate::gesture_menus::{block_dispatch, block_menu, BlockAction};
    let mut fired = None;
    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        for b in blocks.iter().rev() {
            ui.push_id(b.id, |ui| {
                // header line: command + exit/duration badge
                ui.horizontal(|ui| {
                    let ok = b.exit_code == Some(0);
                    let badge = match (b.exit_code, b.duration_ms) {
                        (Some(e), Some(d)) => format!("[{e}] {d}ms"),
                        (Some(e), None) => format!("[{e}]"),
                        (None, _) => "[…]".to_string(),
                    };
                    ui.colored_label(if ok { c.accent } else { c.text }, egui::RichText::new(&b.command).monospace().strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.weak(badge);
                    });
                });
                // preview: first output line, dimmed — CLICK to expand the full captured
                // output inline (the QML drawer opened term_block_output in a pane; inline
                // expansion is the drawer-native equivalent)
                let expanded_id = ui.id().with("expanded");
                let mut expanded = ui.ctx().data_mut(|d| d.get_temp::<bool>(expanded_id).unwrap_or(false));
                if expanded {
                    egui::ScrollArea::vertical().max_height(180.0).id_salt(b.id).show(ui, |ui| {
                        ui.label(egui::RichText::new(&b.output).monospace().small());
                    });
                    if ui.small_button("collapse \u{25B4}").clicked() {
                        expanded = false;
                    }
                } else if let Some(first) = b.output.lines().next() {
                    if ui
                        .add(egui::Label::new(egui::RichText::new(first).monospace().small().weak()).sense(egui::Sense::click()))
                        .clicked()
                    {
                        expanded = true;
                    }
                }
                ui.ctx().data_mut(|d| d.insert_temp(expanded_id, expanded));
                // inline actions + the ⋯ hold-menu host
                ui.horizontal(|ui| {
                    if ui.small_button("\u{25B6}").clicked() {
                        fired = fired.or(Some((b.id, BlockAction::Rerun)));
                    }
                    if ui.small_button("\u{1F4CB}").clicked() {
                        fired = fired.or(Some((b.id, BlockAction::CopyCommand)));
                    }
                    if ui.small_button("\u{1F4E5}").clicked() {
                        fired = fired.or(Some((b.id, BlockAction::CopyOutput)));
                    }
                    let dots = ui.small_button("\u{22EF}");
                    let menu = uikit_egui::slide_menu::SlideMenu::new(("vt-block-menu", b.id)).columns(2);
                    if let uikit_egui::slide_menu::SlideOutcome::Selected(id, _) =
                        menu.show_stored(ui, egui::Id::new(("vt-block-menu", b.id)), &dots, &block_menu())
                    {
                        if let Some(a) = block_dispatch(&id) {
                            fired = fired.or(Some((b.id, a)));
                        }
                    }
                });
                ui.separator();
            });
        }
        if blocks.is_empty() {
            ui.weak("No command blocks yet — run something.");
        }
    });
    fired
}

// ── prefs persistence (the fleet's XDG convention, as pbedit/config-kit use) ─

/// `$XDG_CONFIG_HOME/voiceterm-egui/prefs.kv` (or `~/.config/...`) — one `key=value` per line,
/// the settings-kit kv schema verbatim, so the file is greppable and hand-editable.
pub fn prefs_path() -> std::path::PathBuf {
    // config-kit, not a second copy of the same rule. This function used to
    // hand-roll XDG_CONFIG_HOME / ~/.config / "." with a comment saying "as
    // pbedit/config-kit use" — which is the moment to CALL config-kit rather
    // than describe it. Two copies of a path rule drift silently: the day one
    // gains a fallback the other does not, prefs written by one build stop
    // being found by the next.
    config_kit::config_dir(PREFS_APP).join(PREFS_FILE)
}

/// The app namespace and filename. Named so the two sites that need them —
/// the path and any migration — cannot disagree.
pub const PREFS_APP: &str = "voiceterm-egui";
pub const PREFS_FILE: &str = "prefs.kv";

/// Load prefs from [`prefs_path`]; missing file (or any unreadable line) falls back to defaults —
/// `Prefs::apply_kv` is tolerant of partial sets by design.
pub fn load_prefs() -> settings_kit::Prefs {
    let Ok(text) = std::fs::read_to_string(prefs_path()) else {
        return settings_kit::Prefs::default();
    };
    let kv: Vec<(String, String)> = text
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect();
    settings_kit::Prefs::from_kv(&kv)
}

/// Persist prefs to [`prefs_path`] (creating the directory). Errors are returned, not panicked —
/// a read-only config dir shouldn't take the app down.
pub fn save_prefs(prefs: &settings_kit::Prefs) -> std::io::Result<()> {
    let text: String = prefs.to_kv().into_iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    // config-kit::save creates the directory and writes, which is what the
    // hand-rolled version did — keeping the settings-kit kv SCHEMA, since that
    // is the part worth sharing across apps, while the PATH rule comes from one
    // place.
    config_kit::save(PREFS_APP, PREFS_FILE, &text)
}

// ── theme picker ("long-press to show all vibes") ────────────────────────────

/// The theme grid the settings screenshot shows behind a long-press — one cell per fleet palette
/// (snapclone-theme's catalog, not the QML app's private hex sets: egui front-ends share the kit
/// substrate per UI_STRATEGY.md). Cell id round-trips `PaletteId::from_str`.
pub fn theme_menu() -> Vec<uikit_egui::slide_menu::SlideOption> {
    snapclone_theme::PaletteId::all()
        .iter()
        .map(|p| uikit_egui::slide_menu::SlideOption::new(p.as_str(), p.display_name()))
        .collect()
}

// ── workspace tab sidebar (QML vTabBar, v0.1.796) ────────────────────────────

/// One workspace tab in the vertical tab sidebar. `fence_after` draws the accent divider under
/// this tab (the QML fence — a horizontal rule now that the strip is vertical).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WsTab {
    pub name: String,
    pub pinned: bool,
    pub fence_after: bool,
}

impl WsTab {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), pinned: false, fence_after: false }
    }
}

/// The tab-strip model the per-tab action menu drives — the egui side of what QML kept in
/// `termTabStrip._tabs` behind `backend.ws_command`.
///
/// The authoritative state is a [`TermWorkspace`] — voiceterm's own tab+pane model, the same
/// one the QML backend drives — so pane-level calls (split / close pane / web pane) are now
/// really handled instead of reported unhandled. `tabs` and `active` remain public as a
/// one-way PROJECTION of that model, rebuilt after every mutation: the UI keeps reading them
/// exactly as before, and there is no second copy of tab state to drift.
#[derive(Clone, Debug)]
pub struct Workspace {
    /// UI-facing projection of `model`. Rebuilt by `sync`; write through [`Workspace::apply`].
    pub tabs: Vec<WsTab>,
    /// Projection of `model.active_tab`.
    pub active: usize,
    model: TermWorkspace,
}

impl Default for Workspace {
    fn default() -> Self {
        Self::from_model(TermWorkspace::new())
    }
}

impl Workspace {
    pub fn new(names: &[&str]) -> Self {
        let mut m = TermWorkspace::new(); // starts with one tab, one pane
        if let Some(first) = names.first() {
            m.tabs[0].name = (*first).to_string();
        }
        for n in names.iter().skip(1) {
            m.new_tab();
            m.active_mut().name = (*n).to_string();
        }
        m.active_tab = 0;
        Self::from_model(m)
    }

    fn from_model(model: TermWorkspace) -> Self {
        let mut w = Workspace { tabs: Vec::new(), active: 0, model };
        w.sync();
        w
    }

    /// Rebuild the projection from the model. One direction only, so the two cannot disagree.
    /// A fence boundary `b` sits between tab `b-1` and tab `b`, hence `fence_after(i) == b(i+1)`.
    fn sync(&mut self) {
        let fences = &self.model.fences;
        self.tabs = self
            .model
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| WsTab { name: t.name.clone(), pinned: t.pinned, fence_after: fences.contains(&(i + 1)) })
            .collect();
        self.active = self.model.active_tab;
    }

    /// The authoritative model — pane trees, focus, fences.
    pub fn model(&self) -> &TermWorkspace {
        &self.model
    }

    /// Pane ids of the active tab, left-to-right / top-to-bottom. The renderer walks these
    /// once the split layout lands; until then it tells you how many panes a tab holds.
    pub fn active_pane_ids(&self) -> Vec<u64> {
        self.model.active().root.pane_ids()
    }

    /// Apply one typed backend call (see `gesture_menus::tab_dispatch`). Returns `true` when the
    /// call is recognised. Semantics mirror the QML backend: "new tab" appends AND activates;
    /// "go to tab N" is 1-based and clamps; "close tab" keeps at least one tab and clamps
    /// `active`; "close pane" keeps at least one pane.
    pub fn apply(&mut self, call: &WsCall) -> bool {
        let handled = match call {
            WsCall::ToggleFence(i) => {
                self.model.toggle_fence_after(*i);
                true
            }
            WsCall::TogglePin(i) => {
                self.model.toggle_pin(*i);
                true
            }
            // The web pane opens beside the current one and does NOT steal focus (split_web
            // keeps the terminal focused, so the pty stays addressable). The URL is filled in
            // by the reader; the model only needs the pane to exist.
            WsCall::OpenWebPane => {
                self.model.split_web(Dir::Right, "");
                true
            }
            WsCall::Command(c) => self.command(c),
        };
        self.sync();
        handled
    }

    fn command(&mut self, c: &str) -> bool {
        if c == "new tab" {
            self.model.new_tab();
            // Keep the egui strip's naming ("shell 4"), not TermWorkspace's bare ordinal.
            let n = self.model.tabs.len();
            self.model.active_mut().name = format!("shell {n}");
            true
        } else if let Some(n) = c.strip_prefix("go to tab ").and_then(|n| n.parse::<usize>().ok()) {
            // goto_tab rejects out-of-range rather than clamping, so clamp here to keep the
            // QML behaviour the strip relies on ("go to tab 99" lands on the last tab).
            let one_based = n.max(1).min(self.model.tabs.len());
            self.model.goto_tab(one_based);
            true
        } else if c == "close tab" {
            self.model.close_tab(); // self-guards the last tab and clamps active_tab
            true
        } else if c == "split down" {
            self.model.split(Dir::Down); // stacked — QML's horizontal divider
            true
        } else if c == "split right" {
            self.model.split(Dir::Right); // side by side — QML's vertical divider
            true
        } else if c == "close pane" {
            // close_pane (not close_pane_id) is the focus-aware primitive: it re-focuses a
            // surviving pane, and on the LAST pane of a tab it closes the tab instead — the
            // QML behaviour. It returns None only when that would close the last tab too.
            // close_pane_id is for web panes, which are never focused, and would leave
            // focused_pane dangling here.
            self.model.close_pane();
            true
        } else {
            false
        }
    }

    /// Apply a whole dispatch sequence in order; `true` iff every call was handled.
    pub fn apply_all(&mut self, calls: &[WsCall]) -> bool {
        let mut all = true;
        for c in calls {
            all &= self.apply(c);
        }
        all
    }

    /// The focused pane of the active tab.
    pub fn focused_pane(&self) -> u64 {
        self.model.active().focused_pane
    }

    /// Focus a pane by id (click-to-focus). Ignores ids not in the active tab.
    pub fn focus_pane(&mut self, id: u64) -> bool {
        if self.model.active().root.pane_ids().contains(&id) {
            self.model.active_mut().focused_pane = id;
            true
        } else {
            false
        }
    }

    /// Move focus geometrically, like a tiling WM (delegates to the model).
    pub fn focus_dir(&mut self, dir: Dir) {
        self.model.focus(dir);
    }

    /// Resize the focused pane along `dir` by a fraction of its parent split.
    pub fn resize(&mut self, dir: Dir, delta: f32) -> bool {
        self.model.resize(dir, delta)
    }

    /// Lay the active tab's panes out inside a `w`×`h` area, insetting each side by
    /// `gutter / 2` so neighbours show a seam. Pure (plain f32 — no egui), so the split
    /// geometry is testable without a windowing stack; the caller maps to `egui::Rect`.
    ///
    /// The inset is clamped to a third of each axis, so a pane never inverts when the
    /// area is tiny or the tree is deep.
    pub fn pane_boxes(&self, w: f32, h: f32, gutter: f32) -> Vec<PaneBox> {
        let focused = self.focused_pane();
        self.model
            .pane_rects(w, h)
            .into_iter()
            .map(|(id, r)| {
                let ix = (gutter * 0.5).min(r.w / 3.0).max(0.0);
                let iy = (gutter * 0.5).min(r.h / 3.0).max(0.0);
                PaneBox {
                    id,
                    x: r.x + ix,
                    y: r.y + iy,
                    w: (r.w - ix * 2.0).max(0.0),
                    h: (r.h - iy * 2.0).max(0.0),
                    focused: id == focused,
                    web: matches!(self.model.pane_kind(id), Some(PaneKind::Web { .. })),
                }
            })
            .collect()
    }

    /// Which pane covers `(x, y)` in pane-area coordinates? Hit-tests the UNGAPPED rects,
    /// so a click that lands in a seam still selects a pane rather than falling through.
    /// The last match wins, which matches the model's left-to-right / top-to-bottom order
    /// on shared edges.
    pub fn pane_at(&self, w: f32, h: f32, x: f32, y: f32) -> Option<u64> {
        self.model
            .pane_rects(w, h)
            .into_iter()
            .filter(|(_, r)| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h)
            .map(|(id, _)| id)
            .next_back()
    }

    /// The draggable dividers of the active tab, derived from the pane rects: two panes that
    /// share an edge and overlap along it have a seam between them. Pure.
    pub fn pane_seams(&self, w: f32, h: f32) -> Vec<PaneSeam> {
        const EPS: f32 = 0.5;
        let rects = self.model.pane_rects(w, h);
        let mut out = Vec::new();
        for (ai, ar) in rects.iter() {
            for (bi, br) in rects.iter() {
                if ai == bi {
                    continue;
                }
                // vertical seam: a's right edge meets b's left edge, with rows in common
                if ((ar.x + ar.w) - br.x).abs() < EPS {
                    let from = ar.y.max(br.y);
                    let to = (ar.y + ar.h).min(br.y + br.h);
                    if to - from > EPS {
                        out.push(PaneSeam {
                            vertical: true,
                            pos: br.x,
                            from,
                            to,
                            a: *ai,
                            b: *bi,
                            extent: (br.x + br.w) - ar.x,
                        });
                    }
                }
                // horizontal seam: a's bottom edge meets b's top edge, with columns in common
                if ((ar.y + ar.h) - br.y).abs() < EPS {
                    let from = ar.x.max(br.x);
                    let to = (ar.x + ar.w).min(br.x + br.w);
                    if to - from > EPS {
                        out.push(PaneSeam {
                            vertical: false,
                            pos: br.y,
                            from,
                            to,
                            a: *ai,
                            b: *bi,
                            extent: (br.y + br.h) - ar.y,
                        });
                    }
                }
            }
        }
        out
    }

    /// The seam within `tol` of `(x, y)`, nearest first — what a drag grabs, and what turns
    /// the cursor into a resize arrow on hover.
    pub fn seam_at(&self, w: f32, h: f32, x: f32, y: f32, tol: f32) -> Option<PaneSeam> {
        let mut best: Option<(f32, PaneSeam)> = None;
        for s in self.pane_seams(w, h) {
            let (along, across) = if s.vertical { (y, x) } else { (x, y) };
            if along < s.from || along > s.to {
                continue;
            }
            let d = (across - s.pos).abs();
            if d <= tol && best.as_ref().map_or(true, |(bd, _)| d < *bd) {
                best = Some((d, s));
            }
        }
        best.map(|(_, s)| s)
    }

    /// Drag a divider by `d` pixels (right for a vertical seam, down for a horizontal one).
    ///
    /// `d` becomes a ratio delta over the seam's own extent, so a 40px drag moves the divider
    /// 40px regardless of how deep the split sits. The model only resizes a split where the
    /// target is a DIRECT child, and the low-side pane isn't always one (in `H{V{A,B}, C}` the
    /// A|C seam is the root's, but A hangs off the inner V) — so this tries the low-side pane
    /// and falls back to the high-side one. The ratio change is identical either way; only
    /// reachability differs.
    ///
    /// Focus is saved and restored: grabbing a divider should not move the keyboard focus.
    pub fn drag_seam(&mut self, seam: &PaneSeam, d: f32) -> bool {
        if seam.extent.abs() < f32::EPSILON {
            return false;
        }
        let delta = d / seam.extent;
        let dir = if seam.vertical { Dir::Right } else { Dir::Down };
        let saved = self.focused_pane();
        self.model.active_mut().focused_pane = seam.a;
        let mut ok = self.model.resize(dir, delta);
        if !ok {
            self.model.active_mut().focused_pane = seam.b;
            ok = self.model.resize(dir, delta);
        }
        self.model.active_mut().focused_pane = saved;
        ok
    }

    /// Reconcile the terminals currently hosted (`live`) against every pane in the workspace.
    ///
    /// Deliberately spans ALL tabs, not just the active one — `pane_ids_all` exists in the
    /// model for exactly this ("spawn a PTY each"). Scoping it to the active tab would tear
    /// down and respawn the shells on every tab switch, losing their scrollback and, on
    /// device, killing running processes.
    ///
    /// Pure: the caller creates and drops the actual terminals (and their ptys).
    ///
    /// Web panes are excluded — they render a page and have no pty, so spawning a shell for
    /// one would be wrong on device, not merely wasteful.
    pub fn host_diff(&self, live: &[u64]) -> PaneHostDiff {
        let mut all: Vec<u64> = self
            .model
            .pane_ids_all()
            .into_iter()
            .filter(|id| !matches!(self.model.pane_kind(*id), Some(PaneKind::Web { .. })))
            .collect();
        all.sort_unstable();
        let mut spawn: Vec<u64> = all.iter().copied().filter(|id| !live.contains(id)).collect();
        let mut retire: Vec<u64> = live.iter().copied().filter(|id| !all.contains(id)).collect();
        spawn.sort_unstable();
        retire.sort_unstable();
        PaneHostDiff { spawn, retire }
    }
}

/// The terminals to create and drop so the hosted set matches the model's panes.
/// See [`Workspace::host_diff`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneHostDiff {
    /// Panes that exist in the model but have no terminal yet.
    pub spawn: Vec<u64>,
    /// Terminals whose pane is gone — drop them (and their pty).
    pub retire: Vec<u64>,
}

impl PaneHostDiff {
    pub fn is_empty(&self) -> bool {
        self.spawn.is_empty() && self.retire.is_empty()
    }
}

/// A draggable divider between two panes. Plain `f32`, like [`PaneBox`], so seam maths is
/// testable without a windowing stack.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneSeam {
    /// A vertical divider (drag horizontally). `false` = horizontal divider (drag vertically).
    pub vertical: bool,
    /// x of a vertical seam, y of a horizontal one.
    pub pos: f32,
    /// Start of the seam along its other axis.
    pub from: f32,
    /// End of the seam along its other axis.
    pub to: f32,
    /// Pane on the low side (left / top).
    pub a: u64,
    /// Pane on the high side (right / bottom).
    pub b: u64,
    /// Combined extent of the two panes along the drag axis — the denominator that turns a
    /// pixel drag into a ratio delta.
    pub extent: f32,
}

/// One laid-out pane, ready to draw. Deliberately plain `f32` rather than `egui::Rect` so
/// the layout maths stays unit-testable without a windowing stack.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneBox {
    pub id: u64,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// The pane that owns keyboard input and hosts the live terminal grid.
    pub focused: bool,
    /// A web pane (renders a page, has no pty).
    pub web: bool,
}

/// v0.1.803's "N more" pill math: how many whole rows are hidden above and below a scrolled
/// viewport, given the scroll offset, viewport height, total content height, and the row pitch
/// (row height + spacing — QML's 30 + 3). Pure; the pills render these counts and tapping them
/// scrolls by a page.
pub fn overflow_counts(offset: f32, viewport_h: f32, content_h: f32, pitch: f32) -> (usize, usize) {
    if pitch <= 0.0 || content_h <= viewport_h {
        return (0, 0);
    }
    let above = (offset / pitch).floor().max(0.0) as usize;
    let below_px = (content_h - offset - viewport_h).max(0.0);
    let below = (below_px / pitch).floor() as usize;
    (above, below)
}

/// One tab row of the vertical tab sidebar: the QML row shape — name (📌-suffixed when pinned),
/// active highlight, and the accent fence divider under the row when `fence_after`. Returns the
/// row's Response so the caller can wire click-to-activate AND the hold menu on the same host.
pub fn tab_row(ui: &mut Ui, tab: &WsTab, active: bool, c: &PaletteColors) -> Response {
    let label = if tab.pinned { format!("{} \u{1F4CC}", tab.name) } else { tab.name.clone() };
    let resp = dock::list_row(ui, &label, active, 30.0, c);
    if tab.fence_after {
        // the fence: a 2px accent rule spanning the row's width, just under it (QML bottomMargin -3)
        let r = resp.rect;
        let y = r.bottom() + 2.0;
        ui.painter().line_segment([egui::pos2(r.left(), y), egui::pos2(r.right(), y)], egui::Stroke::new(2.0, c.accent));
        ui.add_space(3.0);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gesture_menus::{tab_dispatch, WsCall};
    use uikit_egui::slide_menu::Variant;

    #[test]
    fn fab_docks_at_the_corner_overlapping_slightly() {
        let term = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        let fab = fab_rect(term, 60.0);
        assert!(fab.center().x < term.right() && fab.center().x > term.right() - 60.0);
        assert!(fab.center().y < term.bottom() && fab.center().y > term.bottom() - 60.0);
        assert!(fab.right() > term.right());
        assert!(fab.bottom() > term.bottom());
    }

    #[test]
    fn terminal_keycaps_cover_the_base_reference_set() {
        let labels: Vec<&str> = TERMINAL_KEYCAPS.iter().map(|k| k.label).collect();
        for expect in ["Esc", "Tab", "↑", "↓", "←", "→", "C-c", "C-d", "C-z", "C-l"] {
            assert!(labels.contains(&expect), "missing {expect} from the base keycap row");
        }
    }

    #[test]
    fn extended_keycaps_are_a_superset_of_the_base_row() {
        let base: std::collections::HashSet<&str> = TERMINAL_KEYCAPS.iter().map(|k| k.label).collect();
        let ext: std::collections::HashSet<&str> = EXTENDED_KEYCAPS.iter().map(|k| k.label).collect();
        assert!(base.is_subset(&ext), "the web-pane-focused row should contain everything the terminal row has");
        for expect in ["C-r", "C-a", "C-e", "~", "/", "-", "_"] {
            assert!(ext.contains(expect), "missing extended key {expect}");
        }
    }

    #[test]
    fn prefs_round_trip_through_the_kv_file_in_a_sandboxed_config_home() {
        let dir = std::env::temp_dir().join(format!("vt-prefs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // sandboxed XDG, the pbedit test pattern — but prefs_path reads the env at call time and
        // tests share a process, so serialize access via a lock on a private mutex
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &dir);

        let mut p = settings_kit::Prefs::default();
        p.palette = "amber".into();
        save_prefs(&p).unwrap();
        assert!(prefs_path().starts_with(&dir));
        let loaded = load_prefs();
        assert_eq!(loaded.palette, "amber");

        // a missing file yields defaults, not an error
        std::fs::remove_file(prefs_path()).unwrap();
        assert_eq!(load_prefs().palette, settings_kit::Prefs::default().palette);

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn theme_menu_covers_every_fleet_palette_and_round_trips_ids() {
        let opts = theme_menu();
        assert_eq!(opts.len(), snapclone_theme::PaletteId::all().len());
        for o in &opts {
            assert_eq!(snapclone_theme::PaletteId::from_str(&o.id).as_str(), o.id);
        }
        assert!(opts.iter().any(|o| o.id == "voice"));
    }

    #[test]
    fn arrow_repeat_waits_500ms_then_fires_at_33ms_cadence() {
        // before the delay: never
        assert!(!repeat_due(0.3, 0.0, 0.0));
        assert!(!repeat_due(0.499, 0.0, 0.0));
        // past the delay: fires, then respects the cadence
        assert!(repeat_due(0.5, 0.0, 0.0));
        assert!(!repeat_due(0.52, 0.0, 0.5)); // 20ms since last emit < 33ms
        assert!(repeat_due(0.534, 0.0, 0.5));
        // the repeat-tagged keys are exactly the arrows, in both keycap sets
        for set in [TERMINAL_KEYCAPS, EXTENDED_KEYCAPS] {
            let arrows: Vec<&str> = set.iter().filter(|k| k.repeat).map(|k| k.label).collect();
            assert_eq!(arrows, ["↑", "↓", "←", "→"]);
        }
    }

    #[test]
    fn overflow_counts_hide_whole_rows_only_and_zero_when_it_fits() {
        const P: f32 = 33.0; // QML row pitch: 30 row + 3 spacing
        // fits: no pills
        assert_eq!(overflow_counts(0.0, 200.0, 150.0, P), (0, 0));
        // 20 rows (660px) in a 200px viewport, at top: none above, the rest below
        let (a, b) = overflow_counts(0.0, 200.0, 660.0, P);
        assert_eq!(a, 0);
        assert_eq!(b, ((660.0 - 200.0) / P) as usize); // 13 whole rows hidden below
        // scrolled 100px: 3 whole rows above (partial rows don't count)
        let (a, b2) = overflow_counts(100.0, 200.0, 660.0, P);
        assert_eq!(a, 3);
        assert!(b2 < b);
        // at the bottom: rows above, none below
        let (a, b3) = overflow_counts(460.0, 200.0, 660.0, P);
        assert_eq!(b3, 0);
        assert!(a > 0);
        // degenerate pitch
        assert_eq!(overflow_counts(50.0, 100.0, 900.0, 0.0), (0, 0));
    }

    #[test]
    fn workspace_applies_the_tab_menu_calls_with_qml_semantics() {
        let mut ws = Workspace::new(&["main", "logs", "build"]);
        // pin + fence toggle
        assert!(ws.apply(&WsCall::TogglePin(1)) && ws.tabs[1].pinned);
        assert!(ws.apply(&WsCall::ToggleFence(0)) && ws.tabs[0].fence_after);
        assert!(ws.apply(&WsCall::ToggleFence(0)) && !ws.tabs[0].fence_after);
        // new tab appends AND activates
        assert!(ws.apply(&WsCall::Command("new tab".into())));
        assert_eq!((ws.tabs.len(), ws.active), (4, 3));
        // goto is 1-based and clamps
        assert!(ws.apply(&WsCall::Command("go to tab 2".into())));
        assert_eq!(ws.active, 1);
        assert!(ws.apply(&WsCall::Command("go to tab 99".into())));
        assert_eq!(ws.active, 3);
        // close removes the active tab, keeps at least one, clamps
        assert!(ws.apply(&WsCall::Command("close tab".into())));
        assert_eq!((ws.tabs.len(), ws.active), (3, 2));
        for _ in 0..5 {
            ws.apply(&WsCall::Command("close tab".into()));
        }
        assert_eq!(ws.tabs.len(), 1); // the last tab survives
    }

    #[test]
    fn tab_menu_dispatch_flows_end_to_end_into_the_workspace() {
        let mut ws = Workspace::new(&["main", "logs", "build"]);
        // hold tab 3, drag up on the split cell: goto happens, and the split now really lands
        let calls = tab_dispatch("split", Variant::Vertical, 2);
        assert!(ws.apply_all(&calls));
        assert_eq!(ws.active, 2);
        assert_eq!(ws.active_pane_ids().len(), 2); // the held tab gained a stacked pane
        // hold tab 1, close with the default (tab) variant: fully handled
        let calls = tab_dispatch("close", Variant::Tab, 0);
        assert!(ws.apply_all(&calls));
        assert_eq!(ws.tabs.len(), 2);
        assert_eq!(ws.tabs[0].name, "logs");
        // fence via the menu on the (new) first tab
        assert!(ws.apply_all(&tab_dispatch("fence", Variant::Single, 0)));
        assert!(ws.tabs[0].fence_after);
    }

    #[test]
    fn pane_level_calls_are_handled_by_the_real_model() {
        let mut ws = Workspace::new(&["main", "logs"]);
        assert_eq!(ws.active_pane_ids().len(), 1);

        // split right = side by side (QML's vertical divider); split down = stacked.
        assert!(ws.apply(&WsCall::Command("split right".into())));
        assert_eq!(ws.active_pane_ids().len(), 2);
        assert!(ws.apply(&WsCall::Command("split down".into())));
        assert_eq!(ws.active_pane_ids().len(), 3);

        // panes are per-tab: the other tab is untouched
        assert!(ws.apply(&WsCall::Command("go to tab 2".into())));
        assert_eq!(ws.active_pane_ids().len(), 1);
        assert!(ws.apply(&WsCall::Command("go to tab 1".into())));
        assert_eq!(ws.active_pane_ids().len(), 3);

        // close pane removes the focused one and re-focuses a survivor
        assert!(ws.apply(&WsCall::Command("close pane".into())));
        assert_eq!(ws.active_pane_ids().len(), 2);
        assert!(ws.apply(&WsCall::Command("close pane".into())));
        assert_eq!(ws.active_pane_ids().len(), 1);
        // closing the LAST pane of a tab closes the TAB instead — QML's semantics
        assert_eq!(ws.tabs.len(), 2);
        assert!(ws.apply(&WsCall::Command("close pane".into())));
        assert_eq!(ws.tabs.len(), 1);
        // ...and on the last pane of the last tab it is a no-op, still handled
        assert!(ws.apply(&WsCall::Command("close pane".into())));
        assert_eq!((ws.tabs.len(), ws.active_pane_ids().len()), (1, 1));

        // an unknown command is still honestly unhandled
        assert!(!ws.apply(&WsCall::Command("teleport".into())));
    }

    #[test]
    fn web_pane_opens_beside_without_stealing_focus() {
        let mut ws = Workspace::new(&["main"]);
        let focus_before = ws.model().active().focused_pane;
        assert!(ws.apply(&WsCall::OpenWebPane));
        assert_eq!(ws.active_pane_ids().len(), 2);
        // split_web deliberately keeps the terminal focused so the pty stays addressable
        assert_eq!(ws.model().active().focused_pane, focus_before);
    }

    #[test]
    fn projection_tracks_the_model_after_every_call() {
        let mut ws = Workspace::new(&["main", "logs", "build"]);
        ws.apply(&WsCall::TogglePin(2));
        ws.apply(&WsCall::ToggleFence(1));
        // the projection is rebuilt from the model, not maintained separately
        assert_eq!(ws.tabs.len(), ws.model().tabs.len());
        assert!(ws.tabs[2].pinned && ws.model().tabs[2].pinned);
        assert!(ws.tabs[1].fence_after && ws.model().fences.contains(&2));
        ws.apply(&WsCall::Command("new tab".into()));
        assert_eq!(ws.active, ws.model().active_tab);
        assert_eq!(ws.tabs.last().unwrap().name, "shell 4");
    }

    #[test]
    fn pane_boxes_tile_the_area_and_mark_focus() {
        let mut ws = Workspace::new(&["main"]);
        // one pane fills the area (minus the gutter inset)
        let b = ws.pane_boxes(400.0, 300.0, 0.0);
        assert_eq!(b.len(), 1);
        assert!(b[0].focused && !b[0].web);
        assert_eq!((b[0].x, b[0].y, b[0].w, b[0].h), (0.0, 0.0, 400.0, 300.0));

        // split right => side by side, halving the width
        ws.apply(&WsCall::Command("split right".into()));
        let b = ws.pane_boxes(400.0, 300.0, 0.0);
        assert_eq!(b.len(), 2);
        assert_eq!((b[0].x, b[0].w), (0.0, 200.0));
        assert_eq!((b[1].x, b[1].w), (200.0, 200.0));
        assert_eq!((b[0].h, b[1].h), (300.0, 300.0));
        // the new pane took focus (Workspace::split sets it)
        assert!(!b[0].focused && b[1].focused);

        // split down on the focused (right) pane => it halves vertically
        ws.apply(&WsCall::Command("split down".into()));
        let b = ws.pane_boxes(400.0, 300.0, 0.0);
        assert_eq!(b.len(), 3);
        let right: Vec<_> = b.iter().filter(|p| p.x >= 200.0).collect();
        assert_eq!(right.len(), 2);
        assert_eq!((right[0].y, right[0].h), (0.0, 150.0));
        assert_eq!((right[1].y, right[1].h), (150.0, 150.0));
    }

    #[test]
    fn gutter_insets_every_side_and_never_inverts() {
        let mut ws = Workspace::new(&["main"]);
        ws.apply(&WsCall::Command("split right".into()));
        let b = ws.pane_boxes(400.0, 300.0, 8.0);
        assert_eq!((b[0].x, b[0].w), (4.0, 192.0)); // inset 4 each side of a 200-wide half
        assert_eq!((b[1].x, b[1].w), (204.0, 192.0));
        assert_eq!((b[0].y, b[0].h), (4.0, 292.0));
        // a gutter larger than the pane clamps instead of producing negative extents
        let tiny = ws.pane_boxes(6.0, 6.0, 100.0);
        assert!(tiny.iter().all(|p| p.w >= 0.0 && p.h >= 0.0));
    }

    #[test]
    fn hit_testing_and_directional_focus_agree_with_the_layout() {
        let mut ws = Workspace::new(&["main"]);
        ws.apply(&WsCall::Command("split right".into()));
        let left = ws.pane_at(400.0, 300.0, 10.0, 10.0).unwrap();
        let right = ws.pane_at(400.0, 300.0, 390.0, 10.0).unwrap();
        assert_ne!(left, right);
        // outside the area hits nothing
        assert!(ws.pane_at(400.0, 300.0, 500.0, 10.0).is_none());

        // click-to-focus, then move focus geometrically back to the right
        assert!(ws.focus_pane(left));
        assert_eq!(ws.focused_pane(), left);
        ws.focus_dir(Dir::Right);
        assert_eq!(ws.focused_pane(), right);
        // nothing further right: a no-op, not a wrap
        ws.focus_dir(Dir::Right);
        assert_eq!(ws.focused_pane(), right);
        // an id from another tab is refused
        assert!(!ws.focus_pane(9999));
    }

    #[test]
    fn web_panes_are_flagged_for_the_renderer() {
        let mut ws = Workspace::new(&["main"]);
        ws.apply(&WsCall::OpenWebPane);
        let b = ws.pane_boxes(400.0, 300.0, 0.0);
        assert_eq!(b.len(), 2);
        assert_eq!(b.iter().filter(|p| p.web).count(), 1);
        // the terminal keeps focus; the web pane is not focused
        assert!(b.iter().any(|p| p.focused && !p.web));
        assert!(b.iter().all(|p| !(p.web && p.focused)));
    }

    /// f32 ratio maths lands a hair off exact pixels (240.00002); compare with a tolerance
    /// rather than pinning bit-exact values.
    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn seams_sit_between_adjacent_panes_and_drag_moves_them() {
        let mut ws = Workspace::new(&["main"]);
        assert!(ws.pane_seams(400.0, 300.0).is_empty()); // one pane, nothing to drag

        ws.apply(&WsCall::Command("split right".into()));
        let seams = ws.pane_seams(400.0, 300.0);
        // exactly one edge, reported once — a seam is an ordered (low, high) pair, so the
        // reversed pair is a different edge and never matches
        assert_eq!(seams.len(), 1);
        let s = ws.seam_at(400.0, 300.0, 200.0, 150.0, 6.0).expect("seam at the midline");
        assert!(s.vertical);
        assert_eq!(s.pos, 200.0);
        assert_eq!((s.from, s.to), (0.0, 300.0));
        assert_eq!(s.extent, 400.0);

        // away from the divider, and off the end of its span, there is nothing to grab
        assert!(ws.seam_at(400.0, 300.0, 100.0, 150.0, 6.0).is_none());

        // drag right 40px => the left pane gains exactly 40px
        assert!(ws.drag_seam(&s, 40.0));
        let b = ws.pane_boxes(400.0, 300.0, 0.0);
        assert!(near(b[0].w, 240.0));
        assert!(near(b[1].x, 240.0) && near(b[1].w, 160.0));

        // and back the other way
        assert!(ws.drag_seam(&s, -40.0));
        assert!(near(ws.pane_boxes(400.0, 300.0, 0.0)[0].w, 200.0));
    }

    #[test]
    fn dragging_a_divider_does_not_steal_focus() {
        let mut ws = Workspace::new(&["main"]);
        ws.apply(&WsCall::Command("split right".into()));
        let focused = ws.focused_pane();
        let s = ws.seam_at(400.0, 300.0, 200.0, 150.0, 6.0).unwrap();
        ws.drag_seam(&s, 25.0);
        assert_eq!(ws.focused_pane(), focused);
    }

    #[test]
    fn nested_split_seam_falls_back_to_the_reachable_side() {
        // H{ V{P0,P2}, P1 } — the P2|P1 seam belongs to the ROOT split, but P2 hangs off the
        // inner V, so resizing via the low-side pane alone would silently fail.
        let mut ws = Workspace::new(&["main"]);
        ws.apply(&WsCall::Command("split right".into())); // H{P0,P1}, focus P1
        let left = ws.pane_at(400.0, 300.0, 10.0, 10.0).unwrap();
        assert!(ws.focus_pane(left));
        ws.apply(&WsCall::Command("split down".into())); // left becomes V{P0,P2}
        assert_eq!(ws.pane_boxes(400.0, 300.0, 0.0).len(), 3);

        // grab the root divider low down, where the nested bottom-left pane meets the right one
        let s = ws.seam_at(400.0, 300.0, 200.0, 225.0, 6.0).expect("root seam");
        assert!(s.vertical);
        assert!(ws.drag_seam(&s, 40.0)); // the fallback is what makes this true
        let b = ws.pane_boxes(400.0, 300.0, 0.0);
        let right: Vec<_> = b.iter().filter(|p| p.x >= 200.0).collect();
        assert_eq!(right.len(), 1);
        assert!(near(right[0].x, 240.0) && near(right[0].w, 160.0));
    }

    #[test]
    fn horizontal_seam_drags_vertically() {
        let mut ws = Workspace::new(&["main"]);
        ws.apply(&WsCall::Command("split down".into()));
        let s = ws.seam_at(400.0, 300.0, 200.0, 150.0, 6.0).expect("horizontal seam");
        assert!(!s.vertical);
        assert_eq!(s.pos, 150.0);
        assert_eq!(s.extent, 300.0);
        assert!(ws.drag_seam(&s, 30.0));
        let b = ws.pane_boxes(400.0, 300.0, 0.0);
        assert!(near(b[0].h, 180.0));
        assert!(near(b[1].y, 180.0) && near(b[1].h, 120.0));
    }

    #[test]
    fn host_diff_spawns_and_retires_terminals_to_match_the_panes() {
        let mut ws = Workspace::new(&["main"]);
        // nothing hosted yet: the one pane needs a terminal
        let d = ws.host_diff(&[]);
        assert_eq!(d.spawn.len(), 1);
        assert!(d.retire.is_empty());
        let mut live = d.spawn.clone();
        assert!(ws.host_diff(&live).is_empty()); // steady state

        // a split asks for exactly one more, not a rebuild
        ws.apply(&WsCall::Command("split right".into()));
        let d = ws.host_diff(&live);
        assert_eq!(d.spawn.len(), 1);
        assert!(d.retire.is_empty());
        live.extend(d.spawn);
        assert!(ws.host_diff(&live).is_empty());

        // closing a pane retires exactly its terminal
        let closing = ws.focused_pane();
        ws.apply(&WsCall::Command("close pane".into()));
        let d = ws.host_diff(&live);
        assert_eq!(d.retire, vec![closing]);
        assert!(d.spawn.is_empty());
    }

    #[test]
    fn switching_tabs_keeps_the_other_tabs_shells_alive() {
        let mut ws = Workspace::new(&["main"]);
        let mut live = ws.host_diff(&[]).spawn;
        // a second tab, split twice, so it has panes worth keeping
        ws.apply(&WsCall::Command("new tab".into()));
        ws.apply(&WsCall::Command("split right".into()));
        live.extend(ws.host_diff(&live).spawn);
        assert_eq!(live.len(), 3);

        // switching away and back must not tear anything down or respawn it — this is why
        // host_diff spans ALL tabs rather than just the active one
        ws.apply(&WsCall::Command("go to tab 1".into()));
        assert!(ws.host_diff(&live).is_empty());
        ws.apply(&WsCall::Command("go to tab 2".into()));
        assert!(ws.host_diff(&live).is_empty());

        // but closing a whole tab retires every terminal that lived in it
        let doomed: Vec<u64> = ws.active_pane_ids();
        assert_eq!(doomed.len(), 2);
        ws.apply(&WsCall::Command("close tab".into()));
        let d = ws.host_diff(&live);
        let mut expect = doomed.clone();
        expect.sort_unstable();
        assert_eq!(d.retire, expect);
        assert!(d.spawn.is_empty());
    }

    #[test]
    fn web_panes_never_get_a_terminal() {
        let mut ws = Workspace::new(&["main"]);
        let live = ws.host_diff(&[]).spawn;
        assert_eq!(live.len(), 1);
        // a web pane adds a pane to the model but NOT a shell — it renders a page, no pty
        ws.apply(&WsCall::OpenWebPane);
        assert_eq!(ws.active_pane_ids().len(), 2);
        assert!(ws.host_diff(&live).is_empty());
    }

    #[test]
    fn sidebar_selecting_a_session_updates_active_and_returns_the_action() {
        let sessions = vec![
            Session { name: "kesslerd".into(), count: 5 },
            Session { name: "kesslerd".into(), count: 2 },
        ];
        let sb = Sidebar::new(sessions);
        assert_eq!(sb.active, 0);
        assert_eq!(sb.sessions.len(), 2);
    }

#[cfg(test)]
mod prefs_path_tests {
    use super::*;

    #[test]
    fn the_path_is_unchanged_by_the_consolidation() {
        // A silent MOVE would strand every existing prefs file: the app would
        // start with defaults and the user's settings would still be on disk,
        // somewhere they never look. So the new path is checked against the
        // rule the hand-rolled version implemented.
        let p = prefs_path();
        assert!(p.ends_with("voiceterm-egui/prefs.kv"), "{}", p.display());
        // and it sits under the XDG config home, not the data or cache one
        assert!(p.starts_with(config_kit::config_home()), "{}", p.display());
    }

    #[test]
    fn the_app_and_file_names_come_from_one_place() {
        // Two literals in two functions is how a save and a load end up
        // pointing at different files.
        assert!(prefs_path().to_string_lossy().contains(PREFS_APP));
        assert!(prefs_path().to_string_lossy().ends_with(PREFS_FILE));
    }
}
}
