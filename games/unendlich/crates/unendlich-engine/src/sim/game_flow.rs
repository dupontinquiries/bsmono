//! Top-level **application flow** — the outer menu↔gameplay state machine
//! (r63-01). This is the spine of the menu→gameplay binary.
//!
//! The app moves: `Boot → MainMenu → Loading(WorldSpec) → InGame → Paused →
//! (resume | quit-to-menu | quit-to-desktop)`. This module owns that *outer*
//! FSM; the main-menu screens are owned by [`crate::sim::title_flow`], which is
//! embedded here and driven through — never duplicated. `game_flow` translates
//! the menu's [`title_flow::TitleEvent`]s into app-level transitions (starting
//! or loading a game leaves the menu and enters `Loading`).
//!
//! Pure & deterministic: a command sequence yields the same states/events every
//! run. No RNG, I/O, GPU, or wall-clock; no `unwrap`/`expect`/`panic!`. Commands
//! that don't apply in the current state are inert (no transition).

use crate::sim::title_flow::{self, TitleEvent, TitleFlow};

/// Outer application state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppState {
    MainMenu,
    Loading,
    InGame,
    Paused,
    Exiting,
}

/// A command into the app FSM. `Title(..)` carries a menu command, forwarded to
/// the embedded [`TitleFlow`] while in [`AppState::MainMenu`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppCmd {
    Title(title_flow::TitleCmd),
    WorldReady,
    PauseToggled,
    QuitToMenu,
    QuitToDesktop,
    BackToGame,
}

/// The app-level event produced by applying a command (what the host/bin should
/// react to). `None` means "no outer transition happened".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppEvent {
    None,
    EnterLoading,
    BeginPlay,
    ShowPause,
    ResumePlay,
    ReturnedToMenu,
    Quit,
}

/// The outer FSM plus the embedded main-menu flow.
#[derive(Clone, Debug)]
pub struct GameFlow {
    state: AppState,
    title: TitleFlow,
}

impl GameFlow {
    /// New app, sitting at the main menu. `has_save`/`slot_count` seed the menu
    /// (and are reused to reset it on quit-to-menu).
    pub fn new(has_save: bool, slot_count: u8) -> GameFlow {
        GameFlow {
            state: AppState::MainMenu,
            title: TitleFlow::new(has_save, slot_count),
        }
    }

    /// Current outer state.
    pub fn state(&self) -> AppState {
        self.state
    }

    /// Read-only view of the embedded menu flow (e.g. for rendering the menu).
    pub fn title(&self) -> &TitleFlow {
        &self.title
    }

    /// Apply a command, transitioning the outer FSM and returning the app event.
    pub fn apply(&mut self, cmd: AppCmd) -> AppEvent {
        match self.state {
            AppState::MainMenu => self.apply_main_menu(cmd),

            AppState::Loading => match cmd {
                AppCmd::WorldReady => {
                    self.state = AppState::InGame;
                    AppEvent::BeginPlay
                }
                _ => AppEvent::None,
            },

            AppState::InGame => match cmd {
                AppCmd::PauseToggled => {
                    self.state = AppState::Paused;
                    AppEvent::ShowPause
                }
                _ => AppEvent::None,
            },

            AppState::Paused => match cmd {
                // Either an explicit resume or a second pause-toggle resumes.
                AppCmd::BackToGame | AppCmd::PauseToggled => {
                    self.state = AppState::InGame;
                    AppEvent::ResumePlay
                }
                AppCmd::QuitToMenu => {
                    // Back to the menu, with the title reset to its root screen.
                    self.title = TitleFlow::new(self.title.has_save, self.title.slot_count);
                    self.state = AppState::MainMenu;
                    AppEvent::ReturnedToMenu
                }
                AppCmd::QuitToDesktop => {
                    self.state = AppState::Exiting;
                    AppEvent::Quit
                }
                _ => AppEvent::None,
            },

            // Terminal: nothing applies once exiting.
            AppState::Exiting => AppEvent::None,
        }
    }

    /// Main-menu handling: forward `Title(..)` to the embedded flow and lift its
    /// events to app transitions. Starting or loading a game enters `Loading`;
    /// confirming quit-to-desktop from the menu exits. Non-`Title` commands are
    /// inert here.
    fn apply_main_menu(&mut self, cmd: AppCmd) -> AppEvent {
        let AppCmd::Title(tc) = cmd else {
            return AppEvent::None;
        };
        match self.title.apply(tc) {
            TitleEvent::StartNewGame | TitleEvent::LoadGame(_) => {
                self.state = AppState::Loading;
                AppEvent::EnterLoading
            }
            // The menu's only app-level exit (ConfirmQuit → Confirm).
            TitleEvent::QuitConfirmed => {
                self.state = AppState::Exiting;
                AppEvent::Quit
            }
            // Navigated / None / EnterMultiplayer / OpenSettings stay within the
            // menu layer (title_flow owns those screens) — no outer transition.
            _ => AppEvent::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::title_flow::{MenuId, Screen, TitleCmd};

    const NEW_GAME: AppCmd = AppCmd::Title(TitleCmd::Select(MenuId::NewGame));

    /// New game: MainMenu → (Select NewGame) → Loading → (WorldReady) → InGame.
    /// `title_flow` emits `StartNewGame` on `Select(NewGame)` in one step.
    #[test]
    fn new_game_drives_menu_to_loading_to_ingame() {
        let mut gf = GameFlow::new(false, 3);
        assert_eq!(gf.state(), AppState::MainMenu);
        assert_eq!(gf.apply(NEW_GAME), AppEvent::EnterLoading);
        assert_eq!(gf.state(), AppState::Loading);
        assert_eq!(gf.apply(AppCmd::WorldReady), AppEvent::BeginPlay);
        assert_eq!(gf.state(), AppState::InGame);
    }

    #[test]
    fn load_game_from_menu_also_enters_loading() {
        // Continue with a save → LoadGame → EnterLoading.
        let mut gf = GameFlow::new(true, 3);
        assert_eq!(
            gf.apply(AppCmd::Title(TitleCmd::Select(MenuId::Continue))),
            AppEvent::EnterLoading
        );
        assert_eq!(gf.state(), AppState::Loading);
    }

    #[test]
    fn world_ready_only_begins_play_from_loading() {
        let mut gf = GameFlow::new(false, 1);
        // Inert in MainMenu.
        assert_eq!(gf.apply(AppCmd::WorldReady), AppEvent::None);
        assert_eq!(gf.state(), AppState::MainMenu);
        gf.apply(NEW_GAME);
        assert_eq!(gf.state(), AppState::Loading);
        assert_eq!(gf.apply(AppCmd::WorldReady), AppEvent::BeginPlay);
        assert_eq!(gf.state(), AppState::InGame);
    }

    fn into_ingame(has_save: bool, slots: u8) -> GameFlow {
        let mut gf = GameFlow::new(has_save, slots);
        assert_eq!(gf.apply(NEW_GAME), AppEvent::EnterLoading);
        assert_eq!(gf.apply(AppCmd::WorldReady), AppEvent::BeginPlay);
        assert_eq!(gf.state(), AppState::InGame);
        gf
    }

    #[test]
    fn pause_then_resume_returns_to_ingame() {
        let mut gf = into_ingame(false, 1);
        assert_eq!(gf.apply(AppCmd::PauseToggled), AppEvent::ShowPause);
        assert_eq!(gf.state(), AppState::Paused);
        assert_eq!(gf.apply(AppCmd::BackToGame), AppEvent::ResumePlay);
        assert_eq!(gf.state(), AppState::InGame);
        // a second pause-toggle also resumes
        gf.apply(AppCmd::PauseToggled);
        assert_eq!(gf.apply(AppCmd::PauseToggled), AppEvent::ResumePlay);
        assert_eq!(gf.state(), AppState::InGame);
    }

    #[test]
    fn quit_to_menu_returns_and_resets_title_to_root() {
        let mut gf = into_ingame(true, 4);
        // Select(NewGame) earlier moved the embedded title to Screen::NewGame,
        // so a reset to Screen::Title is observable.
        assert_eq!(gf.title().screen, Screen::NewGame);
        gf.apply(AppCmd::PauseToggled);
        assert_eq!(gf.state(), AppState::Paused);
        assert_eq!(gf.apply(AppCmd::QuitToMenu), AppEvent::ReturnedToMenu);
        assert_eq!(gf.state(), AppState::MainMenu);
        assert_eq!(gf.title().screen, Screen::Title, "title reset to root");
    }

    #[test]
    fn quit_to_desktop_from_pause_exits_and_is_terminal() {
        let mut gf = into_ingame(false, 1);
        gf.apply(AppCmd::PauseToggled);
        assert_eq!(gf.apply(AppCmd::QuitToDesktop), AppEvent::Quit);
        assert_eq!(gf.state(), AppState::Exiting);
        // terminal — any further command is inert
        for c in [AppCmd::BackToGame, AppCmd::WorldReady, NEW_GAME, AppCmd::PauseToggled] {
            assert_eq!(gf.apply(c), AppEvent::None);
            assert_eq!(gf.state(), AppState::Exiting);
        }
    }

    #[test]
    fn confirm_quit_from_menu_exits_app() {
        let mut gf = GameFlow::new(false, 2);
        // Quit menu → ConfirmQuit (Navigated, no app transition) → Confirm.
        assert_eq!(gf.apply(AppCmd::Title(TitleCmd::Select(MenuId::Quit))), AppEvent::None);
        assert_eq!(gf.state(), AppState::MainMenu);
        assert_eq!(gf.apply(AppCmd::Title(TitleCmd::Confirm)), AppEvent::Quit);
        assert_eq!(gf.state(), AppState::Exiting);
    }

    #[test]
    fn inapplicable_commands_are_inert_not_panics() {
        // MainMenu: gameplay/pause commands don't apply.
        let mut gf = GameFlow::new(false, 2);
        for c in [AppCmd::WorldReady, AppCmd::PauseToggled, AppCmd::BackToGame, AppCmd::QuitToMenu, AppCmd::QuitToDesktop] {
            assert_eq!(gf.apply(c), AppEvent::None);
            assert_eq!(gf.state(), AppState::MainMenu);
        }
        // Loading: only WorldReady applies.
        let mut g2 = GameFlow::new(false, 2);
        g2.apply(NEW_GAME);
        for c in [AppCmd::PauseToggled, AppCmd::BackToGame, AppCmd::QuitToMenu] {
            assert_eq!(g2.apply(c), AppEvent::None);
            assert_eq!(g2.state(), AppState::Loading);
        }
        // InGame: pause-unrelated commands inert.
        let mut g3 = into_ingame(false, 2);
        for c in [AppCmd::WorldReady, AppCmd::BackToGame, AppCmd::QuitToMenu, AppCmd::QuitToDesktop] {
            assert_eq!(g3.apply(c), AppEvent::None);
            assert_eq!(g3.state(), AppState::InGame);
        }
    }

    #[test]
    fn full_loop_and_determinism() {
        let run = || {
            let mut gf = GameFlow::new(true, 3);
            let mut log = Vec::new();
            for c in [
                NEW_GAME,
                AppCmd::WorldReady,
                AppCmd::PauseToggled,
                AppCmd::BackToGame,
                AppCmd::PauseToggled,
                AppCmd::QuitToMenu,
                NEW_GAME,
                AppCmd::WorldReady,
                AppCmd::PauseToggled,
                AppCmd::QuitToDesktop,
            ] {
                log.push((gf.apply(c), gf.state()));
            }
            log
        };
        let first = run();
        // sanity: the loop visits the expected milestones
        assert_eq!(first[0], (AppEvent::EnterLoading, AppState::Loading));
        assert_eq!(first[1], (AppEvent::BeginPlay, AppState::InGame));
        assert_eq!(first[5], (AppEvent::ReturnedToMenu, AppState::MainMenu));
        assert_eq!(first[9], (AppEvent::Quit, AppState::Exiting));
        for _ in 0..50 {
            assert_eq!(run(), first, "non-deterministic");
        }
    }
}
