//! `sim::menu_controller` — the one object the fullgame menu loop drives. It owns
//! the screen FSM (`game_flow`/`title_flow`) AND the concrete sub-state behind each
//! screen (the new-game wizard, the saved-world list, the settings editor, the
//! multiplayer server browser), and turns a player's menu input into either a
//! navigation or a committed [`MenuOutcome`] the bin acts on (start/load/host/join/
//! quit). This replaces the bin's loose `menu_open`/`wizard_open`/`menu_choice`
//! flags with a single coherent backend. Pure: no IO/clock/RNG (the bin passes
//! entropy at commit time for a random world seed).

use crate::sim::game_flow::{AppCmd, AppEvent, AppState, GameFlow};
use crate::sim::new_game_wizard::NewGameWizard;
use crate::sim::new_world_config::WorldSpec;
use crate::sim::session_mode::ServerBrowser;
use crate::sim::settings_model::{self, Settings, SettingsEditor};
use crate::sim::title_flow::{MenuId, Screen, TitleCmd};
use crate::sim::world_catalog::WorldCatalog;

/// What the bin should do when the menu commits. `None` = stayed in the menu.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuOutcome {
    None,
    /// Wizard finished — generate + enter this world.
    StartWorld(WorldSpec),
    /// Load the saved world in this slot.
    LoadWorld(u8),
    /// Host the local world to the network (see session_mode/HostFlow).
    HostWorld,
    /// Join the server currently selected in the browser.
    JoinSelected,
    /// Leave to desktop.
    QuitToDesktop,
}

/// The unified menu backend.
#[derive(Debug)]
pub struct MenuController {
    flow: GameFlow,
    catalog: WorldCatalog,
    settings: SettingsEditor,
    /// Live only while on the NewGame screen.
    wizard: Option<NewGameWizard>,
    /// Live only while on the Multiplayer screen.
    browser: ServerBrowser,
}

impl MenuController {
    pub fn new(catalog: WorldCatalog, settings: Settings) -> MenuController {
        let flow = GameFlow::new(catalog.has_save(), catalog.slot_count());
        MenuController {
            flow,
            catalog,
            settings: SettingsEditor::new(settings),
            wizard: None,
            browser: ServerBrowser::new(),
        }
    }

    pub fn with_defaults() -> MenuController {
        MenuController::new(WorldCatalog::new(), settings_model::defaults())
    }

    // ── observation ──
    pub fn state(&self) -> AppState {
        self.flow.state()
    }
    pub fn screen(&self) -> Screen {
        // The wizard phase is a MainMenu screen the controller owns; it does NOT
        // navigate game_flow (so no world is generated until the wizard finishes).
        if self.wizard.is_some() {
            Screen::NewGame
        } else {
            self.flow.title().screen
        }
    }
    pub fn catalog(&self) -> &WorldCatalog {
        &self.catalog
    }

    /// r126: mutable catalog access — the shell persists/restores the world
    /// list across launches (sim stays IO-free; the bin owns the file).
    pub fn catalog_mut(&mut self) -> &mut WorldCatalog {
        &mut self.catalog
    }
    pub fn wizard(&self) -> Option<&NewGameWizard> {
        self.wizard.as_ref()
    }
    pub fn wizard_mut(&mut self) -> Option<&mut NewGameWizard> {
        self.wizard.as_mut()
    }
    pub fn settings(&self) -> &SettingsEditor {
        &self.settings
    }
    pub fn settings_mut(&mut self) -> &mut SettingsEditor {
        &mut self.settings
    }
    pub fn browser(&self) -> &ServerBrowser {
        &self.browser
    }
    pub fn browser_mut(&mut self) -> &mut ServerBrowser {
        &mut self.browser
    }

    // ── top-level navigation ──
    /// Pick a root menu entry. Sets up the screen's sub-state; never commits a
    /// world by itself (commit happens via [`confirm`]).
    pub fn select(&mut self, id: MenuId) -> MenuOutcome {
        match id {
            // Open the wizard in-menu; do NOT touch game_flow (world stays unloaded).
            MenuId::NewGame => {
                self.wizard = Some(NewGameWizard::new());
                MenuOutcome::None
            }
            // Continue is a one-shot: jump straight into the most-recent save.
            MenuId::Continue if self.catalog.has_save() => MenuOutcome::LoadWorld(0),
            other => {
                let _ = self.flow.apply(AppCmd::Title(TitleCmd::Select(other)));
                if self.screen() == Screen::Multiplayer {
                    self.browser = ServerBrowser::new();
                }
                MenuOutcome::None
            }
        }
    }

    /// Go back one screen. Tears down the screen's sub-state.
    pub fn back(&mut self) -> MenuOutcome {
        if self.wizard.is_some() {
            self.wizard = None; // back to the title menu
        } else {
            let _ = self.flow.apply(AppCmd::Title(TitleCmd::Back));
        }
        MenuOutcome::None
    }

    /// Confirm the current screen. `entropy` feeds a random world seed (the bin
    /// passes a frame/time value). Returns the committed outcome, if any.
    pub fn confirm(&mut self, entropy: u64) -> MenuOutcome {
        match self.screen() {
            Screen::NewGame => {
                let done = self
                    .wizard
                    .as_ref()
                    .filter(|w| w.at_confirm())
                    .and_then(|w| w.finish(entropy, &self.catalog).ok());
                if let Some(spec) = done {
                    self.wizard = None;
                    // NOW drive the outer FSM MainMenu → Loading (title is still on
                    // the Title screen, so Select(NewGame) → StartNewGame → Loading).
                    let _ = self.flow.apply(AppCmd::Title(TitleCmd::Select(MenuId::NewGame)));
                    return MenuOutcome::StartWorld(spec);
                }
                MenuOutcome::None
            }
            Screen::ConfirmQuit => {
                let ev = self.flow.apply(AppCmd::Title(TitleCmd::Confirm));
                if ev == AppEvent::Quit {
                    MenuOutcome::QuitToDesktop
                } else {
                    MenuOutcome::None
                }
            }
            Screen::Settings => {
                self.settings.apply();
                self.back()
            }
            _ => MenuOutcome::None,
        }
    }

    /// Choose a saved slot (on the LoadSlots screen).
    pub fn select_slot(&mut self, slot: u8) -> MenuOutcome {
        if self.screen() == Screen::LoadSlots {
            let ev = self.flow.apply(AppCmd::Title(TitleCmd::SelectSlot(slot)));
            if ev == AppEvent::EnterLoading {
                return MenuOutcome::LoadWorld(slot);
            }
        }
        MenuOutcome::None
    }

    /// Multiplayer: host the local world.
    pub fn host(&mut self) -> MenuOutcome {
        if self.screen() == Screen::Multiplayer {
            MenuOutcome::HostWorld
        } else {
            MenuOutcome::None
        }
    }

    /// Multiplayer: join the currently selected advert (browser must have one).
    pub fn join(&mut self) -> MenuOutcome {
        if self.screen() == Screen::Multiplayer && self.browser.selected().is_some() {
            MenuOutcome::JoinSelected
        } else {
            MenuOutcome::None
        }
    }

    // ── in-game pause (game_flow outer FSM) ──
    pub fn pause(&mut self) -> AppEvent {
        self.flow.apply(AppCmd::PauseToggled)
    }
    pub fn resume(&mut self) -> AppEvent {
        self.flow.apply(AppCmd::BackToGame)
    }
    /// World finished loading → begin play.
    pub fn world_ready(&mut self) -> AppEvent {
        self.flow.apply(AppCmd::WorldReady)
    }
    /// Leave the world, back to the title menu.
    pub fn quit_to_menu(&mut self) -> AppEvent {
        self.wizard = None;
        self.flow.apply(AppCmd::QuitToMenu)
    }

    // ── internals ──
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::world_catalog::{GameMode as CatMode, WorldEntry};

    #[test]
    fn starts_at_title_in_mainmenu() {
        let c = MenuController::with_defaults();
        assert_eq!(c.state(), AppState::MainMenu);
        assert_eq!(c.screen(), Screen::Title);
        assert!(c.wizard().is_none());
    }

    #[test]
    fn new_game_path_runs_the_wizard_and_commits_a_spec() {
        let mut c = MenuController::with_defaults();
        assert_eq!(c.select(MenuId::NewGame), MenuOutcome::None);
        assert_eq!(c.screen(), Screen::NewGame);
        let w = c.wizard_mut().expect("wizard created on NewGame");
        w.set_name("Vale");
        // walk to confirm
        assert!(w.next() && w.next() && w.next());
        assert!(w.at_confirm());
        match c.confirm(0xBEEF) {
            MenuOutcome::StartWorld(spec) => {
                assert_eq!(spec.name, "Vale");
                assert_eq!(spec.seed, 0xBEEF);
            }
            other => panic!("expected StartWorld, got {other:?}"),
        }
    }

    #[test]
    fn back_from_new_game_drops_the_wizard() {
        let mut c = MenuController::with_defaults();
        c.select(MenuId::NewGame);
        assert!(c.wizard().is_some());
        c.back();
        assert_eq!(c.screen(), Screen::Title);
        assert!(c.wizard().is_none());
    }

    #[test]
    fn load_slot_emits_loadworld() {
        // a catalog with a save enables Load
        let mut cat = WorldCatalog::new();
        cat.add(WorldEntry {
            name: "Save A".into(),
            seed: 1,
            mode: CatMode::Survival,
            created_tick: 0,
            last_played_tick: 0,
        });
        let mut c = MenuController::new(cat, settings_model::defaults());
        c.select(MenuId::Load);
        assert_eq!(c.screen(), Screen::LoadSlots);
        assert_eq!(c.select_slot(0), MenuOutcome::LoadWorld(0));
    }

    #[test]
    fn quit_is_two_step_confirm() {
        let mut c = MenuController::with_defaults();
        c.select(MenuId::Quit);
        assert_eq!(c.screen(), Screen::ConfirmQuit);
        assert_eq!(c.confirm(0), MenuOutcome::QuitToDesktop);
    }

    #[test]
    fn pause_and_resume_through_the_outer_fsm() {
        let mut c = MenuController::with_defaults();
        // get into play: NewGame → wizard → start → world_ready
        c.select(MenuId::NewGame);
        let w = c.wizard_mut().unwrap();
        w.set_name("P");
        assert!(w.next() && w.next() && w.next());
        let _ = c.confirm(1);
        // confirm() drove MainMenu → Loading; now finish loading
        assert_eq!(c.state(), AppState::Loading);
        assert_eq!(c.world_ready(), AppEvent::BeginPlay);
        assert_eq!(c.state(), AppState::InGame);
        assert_eq!(c.pause(), AppEvent::ShowPause);
        assert_eq!(c.state(), AppState::Paused);
        assert_eq!(c.resume(), AppEvent::ResumePlay);
        assert_eq!(c.state(), AppState::InGame);
    }
}
