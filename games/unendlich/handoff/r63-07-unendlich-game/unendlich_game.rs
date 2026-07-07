//! `unendlich-game` — main-menu → gameplay shell (r63-07 BIN GLUE).
//!
//! Wraps the world loop ([`app::world_session::run_world`]) in a menu shell:
//! Title → New Game / Continue / Load / Settings / Multiplayer / Credits / Quit.
//! Render/window only: `required-features = ["render", "window"]`.
//!
//! COMPOSES (pure models — control logic lives in the lib, this bin is glue):
//!   - `sim::title_flow`     — title-screen FSM (PRESENT, r57): Screen/MenuId/
//!                             TitleCmd/TitleEvent/TitleFlow{new,apply}.
//!   - `sim::menu_nav`       — focus/navigation over a list of focusables (PRESENT).
//!   - `sim::viewport_split` — split-screen layout (PRESENT; reused for menu panes).
//!   - `sim::net_rendezvous`,`sim::net_candidates` — MP browser (PRESENT).
//!   - `app::world_session`  — run_world + ExitReason + WorldSpec (this thread).
//!
//! ASSUMED sibling APIs from r63-01..06 (NOT in this base tree — coordinator
//! merges them before building; signatures below are what this glue calls, so
//! reconcile names/shapes there if they differ):
//!   - `sim::game_flow::{GameFlow, AppEvent}`:
//!         GameFlow::new(has_save: bool, slot_count: u8) -> GameFlow
//!         GameFlow::title_screen(&self) -> &title_flow::TitleFlow
//!         GameFlow::apply(&mut self, cmd: title_flow::TitleCmd) -> AppEvent
//!         GameFlow::on_exit(&mut self, reason: world_session::ExitReason) -> AppEvent
//!         enum AppEvent { None, Navigated, EnterLoading, BeginPlay,
//!                         OpenSettings, EnterMultiplayer, ExitToDesktop }
//!   - `world::world_catalog::WorldCatalog`:
//!         WorldCatalog::scan(dir: &Path) -> WorldCatalog
//!         .has_save()->bool  .slot_count()->u8  .entry(slot:u8)->Option<&WorldEntry>
//!         WorldEntry { seed:u32, name:String, dir:PathBuf }  (→ build a WorldSpec)
//!   - `sim::settings_model::{SettingsEditor, defaults}`:
//!         settings_model::defaults() -> settings_schema::Settings
//!         SettingsEditor::load(&Path)->SettingsEditor / ::from(Settings)
//!         .settings(&self)->&Settings  .save(&self,&Path)->io::Result<()>
//!   - `sim::menu_screen_layout::layout_for_title(&TitleFlow, viewport) -> Layout`
//!         (a list of focusable items + their activation TitleCmd).
//!   - `sim::new_world_config::{NewWorldConfig, to_spec}`:
//!         NewWorldConfig::default()->_  .with_name/.with_seed  ->
//!         new_world_config::to_spec(&cfg) -> world_session::WorldSpec
//!
//! If `world_session::WorldSpec` is superseded by a richer `world::WorldSpec`,
//! swap the import; `run_world` takes `&WorldSpec` either way.

use unendlich_engine::app::world_session::{self, ExitReason, WorldSpec};
use unendlich_engine::settings_schema::Settings;
use unendlich_engine::sim::title_flow::TitleCmd;
// Sibling models (merged by the coordinator before build):
use unendlich_engine::sim::game_flow::{AppEvent, GameFlow};
use unendlich_engine::sim::{menu_screen_layout, menu_nav, new_world_config, settings_model};
use unendlich_engine::world::world_catalog::WorldCatalog;

fn worlds_dir() -> std::path::PathBuf {
    // ~/.unendlich/worlds/ ; fall back to ./.unendlich/worlds if HOME is unset.
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    home.join(".unendlich").join("worlds")
}

fn main() {
    // Help/version parity with the world bin.
    if unendlich_engine::runtime_flags::wants_help() {
        unendlich_engine::runtime_flags::print_help("unendlich-game");
        return;
    }
    if unendlich_engine::runtime_flags::wants_version() {
        unendlich_engine::runtime_flags::print_version("unendlich-game");
        return;
    }
    env_logger::init();

    // ---- Disk-scanned catalog + settings ----------------------------------
    let dir = worlds_dir();
    let mut catalog = WorldCatalog::scan(&dir);
    let settings_path = dir.parent().map(|p| p.join("settings.toml"));
    let mut editor = match settings_path.as_ref() {
        Some(p) if p.exists() => settings_model::SettingsEditor::load(p),
        _ => settings_model::SettingsEditor::from(settings_model::defaults()),
    };

    // ---- App FSM -----------------------------------------------------------
    let mut flow = GameFlow::new(catalog.has_save(), catalog.slot_count());

    // ---- Menu window/renderer ---------------------------------------------
    // The menu reuses the world bin's window + a lightweight 2D/UI render path.
    // Kept behind a small local helper so this glue is testable in structure;
    // the operator wires it to the actual menu renderer (same VulkanContext as
    // run_world uses). See docs/R63-07-WALKTHROUGH.md for the exact calls.
    let mut menu = MenuApp::open(WINDOW_W, WINDOW_H);

    // ---- Outer shell loop --------------------------------------------------
    // The menu runs until the FSM asks to enter a world; run_world then takes
    // over the same window until the player quits; its ExitReason feeds back in.
    'shell: loop {
        // ===== Menu loop =====
        loop {
            if menu.should_close() {
                break 'shell; // window X = quit to desktop
            }
            let tf = flow.title_screen();
            let layout = menu_screen_layout::layout_for_title(tf, menu.viewport());
            menu.draw(&layout, editor.settings());

            // Route this frame's input through menu_nav, producing at most one
            // activation, then map it to a TitleCmd.
            let nav = menu.poll_nav(); // menu_nav::Input (Up/Down/Left/Right/Activate/Back)
            if let Some(cmd) = menu_nav::resolve(&layout, nav) {
                // Settings/New-Game screens edit local editors before applying.
                match cmd {
                    TitleCmd::SelectSlot(s) => editor.note_slot(s),
                    _ => {}
                }
                match flow.apply(cmd) {
                    AppEvent::EnterLoading | AppEvent::BeginPlay => break,
                    AppEvent::ExitToDesktop => break 'shell,
                    AppEvent::OpenSettings => menu.focus_settings(&mut editor),
                    AppEvent::EnterMultiplayer => menu.open_mp_browser(),
                    AppEvent::Navigated | AppEvent::None => {}
                }
            }
        }

        // ===== Resolve the world to launch =====
        let spec: WorldSpec = match flow.pending_load_slot() {
            // Load/Continue: an existing catalog entry.
            Some(slot) => match catalog.entry(slot) {
                Some(e) => WorldSpec { seed: e.seed, name: e.name.clone(), save_dir: Some(e.dir.clone()) },
                None => WorldSpec::default_world(),
            },
            // New Game: the new-world config the menu collected.
            None => new_world_config::to_spec(menu.new_world_config()),
        };

        // Persist any settings edits before play (best-effort).
        if let Some(p) = settings_path.as_ref() {
            let _ = editor.save(p);
        }
        let settings: Settings = editor.settings().clone();

        // ===== Play the world (hands the window to run_world) =====
        let reason: ExitReason = world_session::run_world(&spec, &settings);

        // ===== Feed the exit back into the FSM =====
        match flow.on_exit(reason) {
            AppEvent::ExitToDesktop => break 'shell,
            _ => {
                // Returned to the menu — re-scan in case a new world was saved.
                catalog = WorldCatalog::scan(&dir);
                flow = GameFlow::new(catalog.has_save(), catalog.slot_count());
            }
        }
    }
}

// --------------------------------------------------------------------------
// Local menu window/render adapter.
//
// This is the ONLY render-specific surface this bin owns; everything else is
// pure-model glue. It is intentionally thin — the operator points each method
// at the existing UI render path (same VulkanContext/WorldRenderer the world
// bin builds). Listed here as the contract to satisfy; see the walkthrough.
// --------------------------------------------------------------------------
const WINDOW_W: u32 = unendlich_engine::constants::WINDOW_WIDTH;
const WINDOW_H: u32 = unendlich_engine::constants::WINDOW_HEIGHT;

struct MenuApp {
    // glfw window + VulkanContext + a 2D UI pass + the MP browser state
    // (net_rendezvous / net_candidates). Fields elided in this glue skeleton.
    _w: u32,
    _h: u32,
}

impl MenuApp {
    fn open(w: u32, h: u32) -> MenuApp { MenuApp { _w: w, _h: h } }
    fn should_close(&self) -> bool { false }
    fn viewport(&self) -> menu_screen_layout::Viewport {
        menu_screen_layout::Viewport { w: self._w, h: self._h }
    }
    fn draw(&mut self, _layout: &menu_screen_layout::Layout, _s: &Settings) {}
    fn poll_nav(&mut self) -> menu_nav::Input { menu_nav::Input::None }
    fn focus_settings(&mut self, _editor: &mut settings_model::SettingsEditor) {}
    fn open_mp_browser(&mut self) {}
    fn new_world_config(&self) -> &new_world_config::NewWorldConfig {
        // The menu builds this from the New Game screen fields.
        new_world_config::NewWorldConfig::default_ref()
    }
}
