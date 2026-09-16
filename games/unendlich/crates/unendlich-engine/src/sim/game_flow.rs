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

use crate::sim::pause_overlay::{PauseOverlay, PauseScreen};
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
    /// The pause menu's sub-screens.
    ///
    /// Owned here for exactly the reason `title` is: an `AppState` with
    /// sub-screens needs something to hold them, and `game_flow` is what holds
    /// the app's shape. `pause_overlay` said its wiring "lives at the call
    /// site" and no call site ever did it — the FSM had 26 tests and zero
    /// references, so the pause menu could not actually open a settings page.
    pause: PauseOverlay,
}

impl GameFlow {
    /// New app, sitting at the main menu. `has_save`/`slot_count` seed the menu
    /// (and are reused to reset it on quit-to-menu).
    pub fn new(has_save: bool, slot_count: u8) -> GameFlow {
        GameFlow {
            state: AppState::MainMenu,
            title: TitleFlow::new(has_save, slot_count),
            pause: PauseOverlay::new(),
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

    /// The pause overlay, for a caller that needs to drive its sub-screens.
    pub fn pause(&self) -> &PauseOverlay {
        &self.pause
    }

    pub fn pause_mut(&mut self) -> &mut PauseOverlay {
        &mut self.pause
    }

    /// Which pause screen is showing, or `None` when not paused.
    ///
    /// `None` rather than `Root` when unpaused: a renderer asking "what pause
    /// screen is up" during play should get "no pause menu", not "the root
    /// one, which you should not draw".
    pub fn pause_screen(&self) -> Option<PauseScreen> {
        matches!(self.state, AppState::Paused).then(|| self.pause.screen())
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
                    // RESET ON ENTRY, the same rule quit-to-menu already
                    // applies to the title. Without it you pause, open
                    // Settings, resume, pause again — and are staring at
                    // Settings, because the overlay kept the screen it was
                    // last on. A menu should open where it opens.
                    self.pause = PauseOverlay::new();
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

    /// r603: enter `Loading` for a launch that did NOT come from the title
    /// menu — joining a LAN host adopts the host's world (name + seed ride
    /// the Accept) and bypasses the wizard entirely. Same edge as
    /// `StartNewGame`, so the later `world_ready()` (Loading → InGame)
    /// works identically for both paths. Idempotent outside `MainMenu`:
    /// already Loading/InGame → no transition (a duplicate Accept must not
    /// bounce the state machine).
    pub fn begin_external_load(&mut self) -> AppEvent {
        if self.state == AppState::MainMenu {
            self.state = AppState::Loading;
            AppEvent::EnterLoading
        } else {
            AppEvent::None
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

    /// r603: the LAN-join synthetic launch takes the same edges as the menu
    /// path — MainMenu → (external load) → Loading → (WorldReady) → InGame —
    /// and duplicate calls don't bounce the state machine.
    #[test]
    fn external_load_drives_loading_then_ingame_and_is_idempotent() {
        let mut gf = GameFlow::new(false, 3);
        assert_eq!(gf.state(), AppState::MainMenu);
        assert_eq!(gf.begin_external_load(), AppEvent::EnterLoading);
        assert_eq!(gf.state(), AppState::Loading);
        // A duplicate Accept mid-load must not re-fire the transition.
        assert_eq!(gf.begin_external_load(), AppEvent::None);
        assert_eq!(gf.state(), AppState::Loading);
        assert_eq!(gf.apply(AppCmd::WorldReady), AppEvent::BeginPlay);
        assert_eq!(gf.state(), AppState::InGame);
        // And once InGame it stays a no-op.
        assert_eq!(gf.begin_external_load(), AppEvent::None);
        assert_eq!(gf.state(), AppState::InGame);
    }

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

    // ── the pause overlay, finally attached ──────────────────────────────

    use crate::sim::pause_overlay::PauseCmd;

    fn paused() -> GameFlow {
        // `into_ingame` is the house helper; reaching for the private `state`
        // field would test a path the app cannot take.
        let mut gf = into_ingame(true, 3);
        gf.apply(AppCmd::PauseToggled);
        gf
    }

    /// THE BUG THE COMPOSITION PREVENTS. Pause, open Settings, resume, pause
    /// again — without a reset you are staring at Settings, because the
    /// overlay kept the screen it was last on. A menu should open where it
    /// opens.
    #[test]
    fn pausing_again_reopens_at_the_root_screen() {
        let mut gf = paused();
        gf.pause_mut().apply(PauseCmd::OpenSettings);
        assert_eq!(gf.pause_screen(), Some(PauseScreen::Settings));
        gf.apply(AppCmd::BackToGame);
        gf.apply(AppCmd::PauseToggled);
        assert_eq!(gf.pause_screen(), Some(PauseScreen::Root),
                   "the menu must open where it opens");
    }

    /// `None` rather than `Root` when unpaused: a renderer asking "what pause
    /// screen is up" during play should get "no pause menu", not "the root
    /// one, which you should not draw".
    #[test]
    fn there_is_no_pause_screen_while_playing() {
        let menu = GameFlow::new(true, 3);
        assert_eq!(menu.pause_screen(), None, "not at the main menu either");
        let mut gf = into_ingame(true, 3);
        assert_eq!(gf.pause_screen(), None);
        gf.apply(AppCmd::PauseToggled);
        assert!(gf.pause_screen().is_some());
    }

    /// The sub-screens are reachable at all, which they were not before: the
    /// FSM had 26 tests and no owner, so nothing could open a settings page.
    #[test]
    fn the_pause_sub_screens_are_reachable_from_the_app() {
        let mut gf = paused();
        assert_eq!(gf.pause_screen(), Some(PauseScreen::Root));
        gf.pause_mut().apply(PauseCmd::OpenSettings);
        assert_eq!(gf.pause_screen(), Some(PauseScreen::Settings));
        gf.pause_mut().apply(PauseCmd::Back);
        assert_eq!(gf.pause_screen(), Some(PauseScreen::Root));
        gf.pause_mut().apply(PauseCmd::QuitToMenu);
        assert_eq!(gf.pause_screen(), Some(PauseScreen::ConfirmQuitToMenu),
                   "quitting asks first");
    }

    /// Quitting to the menu resets BOTH sub-machines. The title already had
    /// this; the pause menu now gets the same treatment, so returning to a
    /// paused game does not find a stale confirmation dialog.
    #[test]
    fn quitting_to_the_menu_leaves_no_stale_pause_screen() {
        let mut gf = paused();
        gf.pause_mut().apply(PauseCmd::QuitToDesktop);
        assert_eq!(gf.pause_screen(), Some(PauseScreen::ConfirmQuitToDesktop));
        gf.apply(AppCmd::QuitToMenu);
        assert_eq!(gf.state(), AppState::MainMenu);
        assert_eq!(gf.pause_screen(), None);
        // ...and a later game pauses cleanly.
        let mut gf = into_ingame(true, 3);
        gf.apply(AppCmd::PauseToggled);
        assert_eq!(gf.pause_screen(), Some(PauseScreen::Root));
    }
}
