//! `sim::pause_overlay` — in-game pause-menu FSM.
//!
//! Drives the layer `game_flow` shows in `AppState::Paused`. Pure state
//! machine: a [`PauseCmd`] in, a [`PauseEvent`] out, with the current
//! [`PauseScreen`] updated. No rendering, no IO.
//!
//! The Settings sub-screen is where `settings_model` would be composed once it
//! exists; this FSM only tracks *which* screen is showing, so it stays
//! self-contained and that wiring lives at the call site.
//!
//! # Screen flow
//!
//! ```text
//! Root ──OpenSettings──▶ Settings ──Back──▶ Root
//! Root ──QuitToMenu────▶ ConfirmQuitToMenu ──Confirm──▶ (QuitToMenu event)
//!                                           └─Cancel──▶ Root
//! Root ──QuitToDesktop─▶ ConfirmQuitToDesktop ─Confirm─▶ (QuitToDesktop event)
//!                                              └Cancel─▶ Root
//! any  ──Resume────────▶ (Resumed event)
//! Root ──Save──────────▶ (Saved event, stays Root)
//! ```

// ── Types ─────────────────────────────────────────────────────────────────────

/// Which screen of the pause overlay is currently showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseScreen {
    /// The top-level pause menu.
    Root,
    /// The settings sub-screen.
    Settings,
    /// "Quit to main menu?" confirmation.
    ConfirmQuitToMenu,
    /// "Quit to desktop?" confirmation.
    ConfirmQuitToDesktop,
}

/// A command issued to the pause overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseCmd {
    /// Close the overlay and resume play.
    Resume,
    /// Open the Settings sub-screen.
    OpenSettings,
    /// Go back one screen (Settings → Root; confirm screens → Root).
    Back,
    /// Save the game (only meaningful on Root).
    Save,
    /// Begin quit-to-menu (opens confirmation).
    QuitToMenu,
    /// Begin quit-to-desktop (opens confirmation).
    QuitToDesktop,
    /// Confirm the pending action on a confirmation screen.
    Confirm,
    /// Cancel the pending action (back to Root).
    Cancel,
}

/// The observable result of applying a [`PauseCmd`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseEvent {
    /// Nothing happened (inert command for the current screen).
    None,
    /// The overlay should close and play resumes.
    Resumed,
    /// The game was saved.
    Saved,
    /// The Settings sub-screen opened.
    OpenedSettings,
    /// Navigated back a screen.
    NavigatedBack,
    /// Confirmed quit to the main menu.
    QuitToMenu,
    /// Confirmed quit to desktop.
    QuitToDesktop,
}

// ── FSM ───────────────────────────────────────────────────────────────────────

/// The pause-overlay state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PauseOverlay {
    screen: PauseScreen,
}

impl PauseOverlay {
    /// Create a fresh overlay on the Root screen.
    pub fn new() -> PauseOverlay {
        PauseOverlay { screen: PauseScreen::Root }
    }

    /// The currently-showing screen.
    pub fn screen(&self) -> PauseScreen {
        self.screen
    }

    /// Apply a command, returning the resulting event and updating state.
    pub fn apply(&mut self, cmd: PauseCmd) -> PauseEvent {
        use PauseCmd::*;
        use PauseScreen::*;

        // Resume always resumes, from any screen.
        if cmd == Resume {
            self.screen = Root;
            return PauseEvent::Resumed;
        }

        match (self.screen, cmd) {
            // ── Root ──
            (Root, OpenSettings) => {
                self.screen = Settings;
                PauseEvent::OpenedSettings
            }
            (Root, Save) => PauseEvent::Saved, // stays on Root
            (Root, QuitToMenu) => {
                self.screen = ConfirmQuitToMenu;
                PauseEvent::None
            }
            (Root, QuitToDesktop) => {
                self.screen = ConfirmQuitToDesktop;
                PauseEvent::None
            }

            // ── Settings ──
            (Settings, Back) => {
                self.screen = Root;
                PauseEvent::NavigatedBack
            }

            // ── Confirm quit-to-menu ──
            (ConfirmQuitToMenu, Confirm) => {
                // Stay on the confirm screen conceptually; the call site tears
                // the overlay down. We reset to Root for a clean state.
                self.screen = Root;
                PauseEvent::QuitToMenu
            }
            (ConfirmQuitToMenu, Cancel) | (ConfirmQuitToMenu, Back) => {
                self.screen = Root;
                PauseEvent::NavigatedBack
            }

            // ── Confirm quit-to-desktop ──
            (ConfirmQuitToDesktop, Confirm) => {
                self.screen = Root;
                PauseEvent::QuitToDesktop
            }
            (ConfirmQuitToDesktop, Cancel) | (ConfirmQuitToDesktop, Back) => {
                self.screen = Root;
                PauseEvent::NavigatedBack
            }

            // ── Everything else is inert ──
            _ => PauseEvent::None,
        }
    }
}

impl Default for PauseOverlay {
    fn default() -> Self {
        PauseOverlay::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> PauseOverlay {
        PauseOverlay::new()
    }

    // ── Construction ──

    #[test]
    fn starts_on_root() {
        assert_eq!(fresh().screen(), PauseScreen::Root);
    }

    #[test]
    fn default_equals_new() {
        assert_eq!(PauseOverlay::default(), PauseOverlay::new());
    }

    // ── Settings toggle ──

    #[test]
    fn open_settings_from_root() {
        let mut p = fresh();
        let ev = p.apply(PauseCmd::OpenSettings);
        assert_eq!(ev, PauseEvent::OpenedSettings);
        assert_eq!(p.screen(), PauseScreen::Settings);
    }

    #[test]
    fn back_from_settings_returns_root() {
        let mut p = fresh();
        p.apply(PauseCmd::OpenSettings);
        let ev = p.apply(PauseCmd::Back);
        assert_eq!(ev, PauseEvent::NavigatedBack);
        assert_eq!(p.screen(), PauseScreen::Root);
    }

    #[test]
    fn settings_back_toggle_roundtrip() {
        let mut p = fresh();
        for _ in 0..3 {
            p.apply(PauseCmd::OpenSettings);
            assert_eq!(p.screen(), PauseScreen::Settings);
            p.apply(PauseCmd::Back);
            assert_eq!(p.screen(), PauseScreen::Root);
        }
    }

    // ── Resume ──

    #[test]
    fn resume_from_root() {
        let mut p = fresh();
        assert_eq!(p.apply(PauseCmd::Resume), PauseEvent::Resumed);
        assert_eq!(p.screen(), PauseScreen::Root);
    }

    #[test]
    fn resume_from_settings() {
        let mut p = fresh();
        p.apply(PauseCmd::OpenSettings);
        assert_eq!(p.apply(PauseCmd::Resume), PauseEvent::Resumed);
        assert_eq!(p.screen(), PauseScreen::Root);
    }

    #[test]
    fn resume_from_confirm_quit_screens() {
        let mut p = fresh();
        p.apply(PauseCmd::QuitToMenu);
        assert_eq!(p.apply(PauseCmd::Resume), PauseEvent::Resumed);
        assert_eq!(p.screen(), PauseScreen::Root);

        let mut q = fresh();
        q.apply(PauseCmd::QuitToDesktop);
        assert_eq!(q.apply(PauseCmd::Resume), PauseEvent::Resumed);
    }

    // ── Save ──

    #[test]
    fn save_returns_saved_and_stays_root() {
        let mut p = fresh();
        assert_eq!(p.apply(PauseCmd::Save), PauseEvent::Saved);
        assert_eq!(p.screen(), PauseScreen::Root);
    }

    // ── Quit-to-menu requires confirm ──

    #[test]
    fn quit_to_menu_opens_confirm_no_quit() {
        let mut p = fresh();
        let ev = p.apply(PauseCmd::QuitToMenu);
        assert_eq!(ev, PauseEvent::None); // not yet quitting
        assert_eq!(p.screen(), PauseScreen::ConfirmQuitToMenu);
    }

    #[test]
    fn quit_to_menu_confirm_quits() {
        let mut p = fresh();
        p.apply(PauseCmd::QuitToMenu);
        let ev = p.apply(PauseCmd::Confirm);
        assert_eq!(ev, PauseEvent::QuitToMenu);
    }

    #[test]
    fn quit_to_menu_cancel_returns_root_no_quit() {
        let mut p = fresh();
        p.apply(PauseCmd::QuitToMenu);
        let ev = p.apply(PauseCmd::Cancel);
        assert_eq!(ev, PauseEvent::NavigatedBack);
        assert_eq!(p.screen(), PauseScreen::Root);
    }

    #[test]
    fn quit_to_menu_back_also_cancels() {
        let mut p = fresh();
        p.apply(PauseCmd::QuitToMenu);
        let ev = p.apply(PauseCmd::Back);
        assert_eq!(ev, PauseEvent::NavigatedBack);
        assert_eq!(p.screen(), PauseScreen::Root);
    }

    // ── Quit-to-desktop requires confirm ──

    #[test]
    fn quit_to_desktop_opens_confirm_no_quit() {
        let mut p = fresh();
        let ev = p.apply(PauseCmd::QuitToDesktop);
        assert_eq!(ev, PauseEvent::None);
        assert_eq!(p.screen(), PauseScreen::ConfirmQuitToDesktop);
    }

    #[test]
    fn quit_to_desktop_confirm_quits() {
        let mut p = fresh();
        p.apply(PauseCmd::QuitToDesktop);
        assert_eq!(p.apply(PauseCmd::Confirm), PauseEvent::QuitToDesktop);
    }

    #[test]
    fn quit_to_desktop_cancel_returns_root() {
        let mut p = fresh();
        p.apply(PauseCmd::QuitToDesktop);
        let ev = p.apply(PauseCmd::Cancel);
        assert_eq!(ev, PauseEvent::NavigatedBack);
        assert_eq!(p.screen(), PauseScreen::Root);
    }

    // ── Confirm screens don't cross-wire ──

    #[test]
    fn menu_confirm_does_not_emit_desktop() {
        let mut p = fresh();
        p.apply(PauseCmd::QuitToMenu);
        assert_ne!(p.apply(PauseCmd::Confirm), PauseEvent::QuitToDesktop);
    }

    #[test]
    fn desktop_confirm_does_not_emit_menu() {
        let mut p = fresh();
        p.apply(PauseCmd::QuitToDesktop);
        assert_ne!(p.apply(PauseCmd::Confirm), PauseEvent::QuitToMenu);
    }

    // ── Inert commands are no-ops ──

    #[test]
    fn confirm_on_root_is_inert() {
        let mut p = fresh();
        assert_eq!(p.apply(PauseCmd::Confirm), PauseEvent::None);
        assert_eq!(p.screen(), PauseScreen::Root);
    }

    #[test]
    fn cancel_on_root_is_inert() {
        let mut p = fresh();
        assert_eq!(p.apply(PauseCmd::Cancel), PauseEvent::None);
        assert_eq!(p.screen(), PauseScreen::Root);
    }

    #[test]
    fn back_on_root_is_inert() {
        let mut p = fresh();
        assert_eq!(p.apply(PauseCmd::Back), PauseEvent::None);
        assert_eq!(p.screen(), PauseScreen::Root);
    }

    #[test]
    fn save_on_settings_is_inert() {
        let mut p = fresh();
        p.apply(PauseCmd::OpenSettings);
        assert_eq!(p.apply(PauseCmd::Save), PauseEvent::None);
        assert_eq!(p.screen(), PauseScreen::Settings);
    }

    #[test]
    fn open_settings_on_confirm_screen_is_inert() {
        let mut p = fresh();
        p.apply(PauseCmd::QuitToMenu);
        assert_eq!(p.apply(PauseCmd::OpenSettings), PauseEvent::None);
        assert_eq!(p.screen(), PauseScreen::ConfirmQuitToMenu);
    }

    #[test]
    fn quit_to_menu_on_settings_is_inert() {
        let mut p = fresh();
        p.apply(PauseCmd::OpenSettings);
        // QuitToMenu only acts from Root.
        assert_eq!(p.apply(PauseCmd::QuitToMenu), PauseEvent::None);
        assert_eq!(p.screen(), PauseScreen::Settings);
    }

    // ── Determinism ──

    #[test]
    fn deterministic_sequence() {
        let run = || {
            let mut p = PauseOverlay::new();
            let mut events = Vec::new();
            for &cmd in &[
                PauseCmd::OpenSettings, PauseCmd::Back,
                PauseCmd::QuitToMenu, PauseCmd::Cancel,
                PauseCmd::QuitToDesktop, PauseCmd::Confirm,
                PauseCmd::Save, PauseCmd::Resume,
            ] {
                events.push(p.apply(cmd));
            }
            (events, p.screen())
        };
        assert_eq!(run(), run());
    }

    // ── Full flow ──

    #[test]
    fn full_pause_flow() {
        let mut p = fresh();
        // Open settings, come back.
        assert_eq!(p.apply(PauseCmd::OpenSettings), PauseEvent::OpenedSettings);
        assert_eq!(p.apply(PauseCmd::Back), PauseEvent::NavigatedBack);
        // Save.
        assert_eq!(p.apply(PauseCmd::Save), PauseEvent::Saved);
        // Try to quit to menu, change mind.
        assert_eq!(p.apply(PauseCmd::QuitToMenu), PauseEvent::None);
        assert_eq!(p.apply(PauseCmd::Cancel), PauseEvent::NavigatedBack);
        // Now actually quit to desktop.
        assert_eq!(p.apply(PauseCmd::QuitToDesktop), PauseEvent::None);
        assert_eq!(p.apply(PauseCmd::Confirm), PauseEvent::QuitToDesktop);
    }
}
