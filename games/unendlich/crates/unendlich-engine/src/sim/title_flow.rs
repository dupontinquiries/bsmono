//! `sim::title_flow` — main-menu / title-screen state machine.
//!
//! The flow before a world is loaded: Title → New Game / Continue / Load /
//! Settings / Multiplayer / Credits / Quit, with confirm-quit and save-slot
//! selection. Pure state only — the host renders the current [`Screen`] and acts
//! on the returned [`TitleEvent`] (start a world, load a slot, open settings,
//! quit). Distinct from `ingame_menu` (the in-play pause overlay). Pure,
//! deterministic, panic-free (R1/R6/R11); standalone `rustc --test`-able,
//! release-safe.

/// Which title-screen panel is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Title,
    Singleplayer,
    NewGame,
    LoadSlots,
    Settings,
    Multiplayer,
    Credits,
    ConfirmQuit,
}

/// A top-level menu entry on the title screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuId {
    Singleplayer,
    NewGame,
    Tutorial,
    Continue,
    Load,
    Settings,
    Multiplayer,
    Credits,
    Quit,
}

/// An input command from the host.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TitleCmd {
    Select(MenuId),
    SelectSlot(u8),
    Back,
    Confirm,
    Cancel,
}

/// What the host should do as a result of a command.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TitleEvent {
    None,
    Navigated,
    StartNewGame,
    LoadGame(u8),
    EnterMultiplayer,
    OpenSettings,
    StartTutorial,
    QuitConfirmed,
}

/// Slot index treated as the most-recent save for `Continue`.
const MOST_RECENT_SLOT: u8 = 0;

/// Title-screen flow state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TitleFlow {
    pub screen: Screen,
    pub has_save: bool,
    pub slot_count: u8,
}

impl TitleFlow {
    /// New flow at the title screen.
    pub fn new(has_save: bool, slot_count: u8) -> TitleFlow {
        TitleFlow { screen: Screen::Title, has_save, slot_count }
    }

    /// Apply a command, transitioning the screen and returning the host event.
    pub fn apply(&mut self, cmd: TitleCmd) -> TitleEvent {
        match cmd {
            TitleCmd::Select(id) => self.select(id),
            TitleCmd::SelectSlot(s) => {
                // only meaningful on the load screen; out-of-range ignored
                if self.screen == Screen::LoadSlots && s < self.slot_count {
                    TitleEvent::LoadGame(s)
                } else {
                    TitleEvent::None
                }
            }
            TitleCmd::Back => {
                if self.screen == Screen::Title {
                    TitleEvent::None
                } else {
                    self.screen = Screen::Title;
                    TitleEvent::Navigated
                }
            }
            TitleCmd::Confirm => {
                if self.screen == Screen::ConfirmQuit {
                    TitleEvent::QuitConfirmed
                } else {
                    TitleEvent::None
                }
            }
            TitleCmd::Cancel => {
                if self.screen == Screen::ConfirmQuit {
                    self.screen = Screen::Title;
                    TitleEvent::Navigated
                } else {
                    TitleEvent::None
                }
            }
        }
    }

    /// Top-level menu selection — honoured from the Title screen and from the
    /// Singleplayer submenu (which groups Create / Load / Tutorial).
    fn select(&mut self, id: MenuId) -> TitleEvent {
        if self.screen != Screen::Title && self.screen != Screen::Singleplayer {
            return TitleEvent::None;
        }
        match id {
            MenuId::Singleplayer => {
                self.screen = Screen::Singleplayer;
                TitleEvent::Navigated
            }
            MenuId::NewGame => {
                self.screen = Screen::NewGame;
                TitleEvent::StartNewGame
            }
            MenuId::Tutorial => {
                // Launch the bending practice world. The screen stays put; the
                // host acts on the event (world load is host-side).
                TitleEvent::StartTutorial
            }
            MenuId::Continue => {
                if self.has_save {
                    TitleEvent::LoadGame(MOST_RECENT_SLOT)
                } else {
                    TitleEvent::None // no save → Continue is inert
                }
            }
            MenuId::Load => {
                self.screen = Screen::LoadSlots;
                TitleEvent::Navigated
            }
            MenuId::Settings => {
                self.screen = Screen::Settings;
                TitleEvent::OpenSettings
            }
            MenuId::Multiplayer => {
                self.screen = Screen::Multiplayer;
                TitleEvent::EnterMultiplayer
            }
            MenuId::Credits => {
                self.screen = Screen::Credits;
                TitleEvent::Navigated
            }
            MenuId::Quit => {
                self.screen = Screen::ConfirmQuit;
                TitleEvent::Navigated
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continue_only_fires_with_save() {
        let mut with = TitleFlow::new(true, 3);
        assert_eq!(with.apply(TitleCmd::Select(MenuId::Continue)), TitleEvent::LoadGame(MOST_RECENT_SLOT));
        let mut without = TitleFlow::new(false, 3);
        assert_eq!(without.apply(TitleCmd::Select(MenuId::Continue)), TitleEvent::None);
    }

    #[test]
    fn quit_confirm_then_confirm_or_cancel() {
        let mut f = TitleFlow::new(false, 3);
        assert_eq!(f.apply(TitleCmd::Select(MenuId::Quit)), TitleEvent::Navigated);
        assert_eq!(f.screen, Screen::ConfirmQuit);
        assert_eq!(f.apply(TitleCmd::Confirm), TitleEvent::QuitConfirmed);
        // cancel path returns to Title
        let mut g = TitleFlow::new(false, 3);
        g.apply(TitleCmd::Select(MenuId::Quit));
        assert_eq!(g.apply(TitleCmd::Cancel), TitleEvent::Navigated);
        assert_eq!(g.screen, Screen::Title);
        // Confirm when not confirming-quit is inert
        let mut h = TitleFlow::new(false, 3);
        assert_eq!(h.apply(TitleCmd::Confirm), TitleEvent::None);
    }

    #[test]
    fn load_slots_selection_bounds() {
        let mut f = TitleFlow::new(true, 3);
        assert_eq!(f.apply(TitleCmd::Select(MenuId::Load)), TitleEvent::Navigated);
        assert_eq!(f.screen, Screen::LoadSlots);
        assert_eq!(f.apply(TitleCmd::SelectSlot(2)), TitleEvent::LoadGame(2));
        // out-of-range slot ignored
        assert_eq!(f.apply(TitleCmd::SelectSlot(3)), TitleEvent::None);
        assert_eq!(f.apply(TitleCmd::SelectSlot(200)), TitleEvent::None);
        // SelectSlot off the load screen is inert
        let mut g = TitleFlow::new(true, 3);
        assert_eq!(g.apply(TitleCmd::SelectSlot(0)), TitleEvent::None);
    }

    #[test]
    fn singleplayer_submenu_groups_create_load_tutorial() {
        // Title → Singleplayer opens the submenu (navigation, not an action).
        let mut f = TitleFlow::new(true, 3);
        assert_eq!(f.apply(TitleCmd::Select(MenuId::Singleplayer)), TitleEvent::Navigated);
        assert_eq!(f.screen, Screen::Singleplayer);
        // From the submenu, Create starts a new game.
        assert_eq!(f.apply(TitleCmd::Select(MenuId::NewGame)), TitleEvent::StartNewGame);

        // Load from the submenu goes to the slot picker.
        let mut g = TitleFlow::new(true, 3);
        g.apply(TitleCmd::Select(MenuId::Singleplayer));
        assert_eq!(g.apply(TitleCmd::Select(MenuId::Load)), TitleEvent::Navigated);
        assert_eq!(g.screen, Screen::LoadSlots);

        // Tutorial launches the practice world; the screen stays on the submenu.
        let mut h = TitleFlow::new(true, 3);
        h.apply(TitleCmd::Select(MenuId::Singleplayer));
        assert_eq!(h.apply(TitleCmd::Select(MenuId::Tutorial)), TitleEvent::StartTutorial);
        assert_eq!(h.screen, Screen::Singleplayer);

        // Back from the submenu returns to the title.
        let mut k = TitleFlow::new(true, 3);
        k.apply(TitleCmd::Select(MenuId::Singleplayer));
        assert_eq!(k.apply(TitleCmd::Back), TitleEvent::Navigated);
        assert_eq!(k.screen, Screen::Title);
    }

    #[test]
    fn tutorial_inert_from_bare_title() {
        // Tutorial is only reachable through the Singleplayer submenu; selecting
        // it from a screen where it isn't offered is a no-op, not a launch.
        let mut f = TitleFlow::new(true, 3);
        f.apply(TitleCmd::Select(MenuId::Multiplayer)); // now on a non-SP screen
        assert_eq!(f.apply(TitleCmd::Select(MenuId::Tutorial)), TitleEvent::None);
    }

    #[test]
    fn direct_actions() {
        let mut f = TitleFlow::new(true, 3);
        assert_eq!(f.apply(TitleCmd::Select(MenuId::NewGame)), TitleEvent::StartNewGame);
        let mut g = TitleFlow::new(true, 3);
        assert_eq!(g.apply(TitleCmd::Select(MenuId::Multiplayer)), TitleEvent::EnterMultiplayer);
        assert_eq!(g.screen, Screen::Multiplayer);
        let mut h = TitleFlow::new(true, 3);
        assert_eq!(h.apply(TitleCmd::Select(MenuId::Settings)), TitleEvent::OpenSettings);
        assert_eq!(h.screen, Screen::Settings);
    }

    #[test]
    fn back_from_any_subscreen_returns_to_title() {
        let entries = [
            MenuId::Load,
            MenuId::Settings,
            MenuId::Multiplayer,
            MenuId::Credits,
            MenuId::Quit,
            MenuId::NewGame,
        ];
        for &e in &entries {
            let mut f = TitleFlow::new(true, 3);
            f.apply(TitleCmd::Select(e));
            assert_ne!(f.screen, Screen::Title, "should have left Title for {:?}", e);
            assert_eq!(f.apply(TitleCmd::Back), TitleEvent::Navigated);
            assert_eq!(f.screen, Screen::Title, "Back should return to Title from {:?}", e);
        }
        // Back at Title is inert
        let mut t = TitleFlow::new(true, 3);
        assert_eq!(t.apply(TitleCmd::Back), TitleEvent::None);
        assert_eq!(t.screen, Screen::Title);
    }

    #[test]
    fn select_ignored_off_title() {
        // once on a subscreen, a top-level Select is inert until Back
        let mut f = TitleFlow::new(true, 3);
        f.apply(TitleCmd::Select(MenuId::Load));
        assert_eq!(f.apply(TitleCmd::Select(MenuId::Quit)), TitleEvent::None);
        assert_eq!(f.screen, Screen::LoadSlots);
    }

    #[test]
    fn deterministic() {
        let seq = [
            TitleCmd::Select(MenuId::Load),
            TitleCmd::SelectSlot(1),
            TitleCmd::Back,
            TitleCmd::Select(MenuId::Quit),
            TitleCmd::Cancel,
            TitleCmd::Select(MenuId::Continue),
        ];
        let mut a = TitleFlow::new(true, 4);
        let mut b = TitleFlow::new(true, 4);
        let ra: Vec<TitleEvent> = seq.iter().map(|&c| a.apply(c)).collect();
        let rb: Vec<TitleEvent> = seq.iter().map(|&c| b.apply(c)).collect();
        assert_eq!(ra, rb);
        assert_eq!(a, b);
    }
}
