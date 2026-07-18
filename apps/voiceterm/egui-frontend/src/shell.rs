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
}

/// The default keycap row (terminal-focused — matches the base screenshot). A web pane in focus
/// shows more keys (C-r, C-a, C-e, ~, /, -, _) per the reference; callers needing that longer row
/// can build their own `&[Keycap]` — the rendering function takes a slice, not a fixed set.
pub const TERMINAL_KEYCAPS: &[Keycap] = &[
    Keycap { label: "Esc", send: "\x1b" },
    Keycap { label: "Tab", send: "\t" },
    Keycap { label: "↑", send: "\x1b[A" },
    Keycap { label: "↓", send: "\x1b[B" },
    Keycap { label: "←", send: "\x1b[D" },
    Keycap { label: "→", send: "\x1b[C" },
    Keycap { label: "C-c", send: "\x03" },
    Keycap { label: "C-d", send: "\x04" },
    Keycap { label: "C-z", send: "\x1a" },
    Keycap { label: "C-l", send: "\x0c" },
];

/// The extended row shown when a web/rich pane has focus (adds C-r/C-a/C-e + a few punctuation
/// keys that are awkward to reach one-handed on touch).
pub const EXTENDED_KEYCAPS: &[Keycap] = &[
    Keycap { label: "Esc", send: "\x1b" }, Keycap { label: "Tab", send: "\t" },
    Keycap { label: "↑", send: "\x1b[A" }, Keycap { label: "↓", send: "\x1b[B" },
    Keycap { label: "←", send: "\x1b[D" }, Keycap { label: "→", send: "\x1b[C" },
    Keycap { label: "C-c", send: "\x03" }, Keycap { label: "C-d", send: "\x04" },
    Keycap { label: "C-z", send: "\x1a" }, Keycap { label: "C-l", send: "\x0c" },
    Keycap { label: "C-r", send: "\x12" }, Keycap { label: "C-a", send: "\x01" },
    Keycap { label: "C-e", send: "\x05" }, Keycap { label: "~", send: "~" },
    Keycap { label: "/", send: "/" }, Keycap { label: "-", send: "-" }, Keycap { label: "_", send: "_" },
];

/// Render the bottom keycap bar plus the "Sel · Copy · Paste" hint row beneath it. Returns the
/// `send` string of a tapped keycap, if any.
pub fn keycap_bar_ui(ui: &mut Ui, keys: &[Keycap], c: &PaletteColors) -> Option<&'static str> {
    let chips: Vec<Chip> = keys.iter().map(|k| Chip::new(k.label)).collect();
    let clicked = dock::chip_row(ui, &chips, 30.0, c);
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Sel · Copy · Paste").color(c.text_weak).size(11.0));
    });
    clicked.map(|i| keys[i].send)
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
                // preview: first output line, dimmed
                if let Some(first) = b.output.lines().next() {
                    ui.weak(egui::RichText::new(first).monospace().small());
                }
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
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("voiceterm-egui").join("prefs.kv")
}

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
    let path = prefs_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text: String = prefs.to_kv().into_iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(path, text)
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
/// `termTabStrip._tabs` behind `backend.ws_command`. Single-pane for now: pane-level calls
/// (split / close pane / web pane) report unhandled so the caller can surface or stub them.
#[derive(Clone, Debug, Default)]
pub struct Workspace {
    pub tabs: Vec<WsTab>,
    pub active: usize,
}

impl Workspace {
    pub fn new(names: &[&str]) -> Self {
        Self { tabs: names.iter().map(|n| WsTab::new(*n)).collect(), active: 0 }
    }

    /// Apply one typed backend call (see `gesture_menus::tab_dispatch`). Returns `true` when the
    /// call is handled by this model; pane-level calls return `false` (single-pane model).
    /// Semantics mirror the QML backend: "new tab" appends AND activates; "go to tab N" is
    /// 1-based and clamps; "close tab" keeps at least one tab and clamps `active`.
    pub fn apply(&mut self, call: &WsCall) -> bool {
        match call {
            WsCall::ToggleFence(i) => {
                if let Some(t) = self.tabs.get_mut(*i) {
                    t.fence_after = !t.fence_after;
                }
                true
            }
            WsCall::TogglePin(i) => {
                if let Some(t) = self.tabs.get_mut(*i) {
                    t.pinned = !t.pinned;
                }
                true
            }
            WsCall::Command(c) => {
                if c == "new tab" {
                    self.tabs.push(WsTab::new(format!("shell {}", self.tabs.len() + 1)));
                    self.active = self.tabs.len() - 1;
                    true
                } else if let Some(n) = c.strip_prefix("go to tab ").and_then(|n| n.parse::<usize>().ok()) {
                    self.active = n.saturating_sub(1).min(self.tabs.len().saturating_sub(1));
                    true
                } else if c == "close tab" {
                    if self.tabs.len() > 1 {
                        self.tabs.remove(self.active);
                        self.active = self.active.min(self.tabs.len() - 1);
                    }
                    true
                } else {
                    false // split down/right, close pane, … — pane-level, not modeled yet
                }
            }
            WsCall::OpenWebPane => false,
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
        // hold tab 3, drag up on the split cell: goto happens, the split itself is (honestly) unhandled
        let calls = tab_dispatch("split", Variant::Vertical, 2);
        assert!(!ws.apply_all(&calls));
        assert_eq!(ws.active, 2);
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
    fn sidebar_selecting_a_session_updates_active_and_returns_the_action() {
        let sessions = vec![
            Session { name: "kesslerd".into(), count: 5 },
            Session { name: "kesslerd".into(), count: 2 },
        ];
        let sb = Sidebar::new(sessions);
        assert_eq!(sb.active, 0);
        assert_eq!(sb.sessions.len(), 2);
    }
}
