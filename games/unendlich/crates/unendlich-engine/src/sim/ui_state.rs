//! Unified UI state for the fullgame.
//!
//! Two levels, mirroring how the game actually behaves:
//!
//! * [`AppMode`] — the TOP level. Either we're on the [`AppMode::TitleScreen`]
//!   (main menu / world select, no world simulated) or we're [`AppMode::Playing`]
//!   in a live world.
//!
//! * [`MenuState`] — a SUBSET that only has meaning while `Playing`: which single
//!   overlay (if any) is currently up. [`MenuState::NoMenu`] is the default —
//!   playing with the cursor captured for mouselook. Exactly one overlay can be
//!   open at a time, so the ambiguous combinations of the old independent bools
//!   (`show_inventory` + `ui_panel_open` + `pause_menu_open` + `settings_open` …)
//!   simply can't occur.
//!
//! Key bindings (the operator's layout):
//!   * `p`   → [`MenuState::Inventory`] — the player inventory grid.
//!   * `Esc` → [`MenuState::Pause`]     — pause menu (inventory + settings access).
//!   * `F4`  → [`MenuState::Debug`]     — debug panels, on their OWN key.
//!   * `C`   → [`MenuState::Craft`]     — crafting.
//!
//! The bin keeps the old renderer flags as a thin *rendering* interface, but they
//! are PUSHED from this state each frame and PULLED back when a renderer button
//! (Resume / Done / Quit) closes a panel — so this enum is the single source of
//! truth for "what is open", while the FSM/save plumbing underneath is untouched.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppMode {
    /// Main menu / world-select. No world is being simulated.
    TitleScreen,
    /// In a live world.
    Playing,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuState {
    /// Playing, no overlay — cursor captured for mouselook. The default.
    NoMenu,
    /// Player inventory grid (key: `p`, or the rebindable OpenInventory alias).
    Inventory,
    /// Pause menu: Resume / Settings / Save&Quit, with the inventory shown too
    /// (key: `Esc`).
    Pause,
    /// The full settings screen, reached from the pause menu's Settings button.
    Settings,
    /// Debug panels (frame-time, shader pack, counters) — key: `F4`.
    Debug,
    /// Crafting (key: `C`).
    Craft,
}

impl MenuState {
    /// Is any overlay open? (i.e. not [`MenuState::NoMenu`]).
    #[inline]
    pub fn is_open(self) -> bool {
        self != MenuState::NoMenu
    }

    /// Should the mouse be captured for mouselook? Only with no overlay up.
    #[inline]
    pub fn captures_cursor(self) -> bool {
        self == MenuState::NoMenu
    }

    /// Does the game simulation pause in this state? Any overlay pauses.
    #[inline]
    pub fn pauses_game(self) -> bool {
        self.is_open()
    }

    /// Should the inventory grid be drawn? Both the dedicated inventory and the
    /// pause menu show it.
    #[inline]
    pub fn shows_inventory(self) -> bool {
        matches!(self, MenuState::Inventory | MenuState::Pause)
    }

    #[inline]
    pub fn shows_pause_menu(self) -> bool {
        self == MenuState::Pause
    }

    #[inline]
    pub fn shows_settings(self) -> bool {
        self == MenuState::Settings
    }

    #[inline]
    pub fn shows_debug(self) -> bool {
        self == MenuState::Debug
    }

    #[inline]
    pub fn shows_craft(self) -> bool {
        self == MenuState::Craft
    }

    /// Press the key for `target`: if that overlay is already up, close it
    /// (back to [`MenuState::NoMenu`]); otherwise switch to it, replacing any
    /// other open overlay. This is the "one toggle key per overlay" rule.
    #[inline]
    pub fn toggle(self, target: MenuState) -> MenuState {
        if self == target {
            MenuState::NoMenu
        } else {
            target
        }
    }

    /// Esc semantics: if anything is open, back out one level; otherwise open the
    /// pause menu. From the full Settings screen, Esc returns to the pause menu
    /// (not all the way out), matching how nested settings should feel.
    #[inline]
    pub fn on_escape(self) -> MenuState {
        match self {
            MenuState::NoMenu => MenuState::Pause,
            MenuState::Settings => MenuState::Pause,
            _ => MenuState::NoMenu,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_playing_no_overlay() {
        let s = MenuState::NoMenu;
        assert!(!s.is_open());
        assert!(s.captures_cursor());
        assert!(!s.pauses_game());
    }

    #[test]
    fn toggle_opens_then_closes() {
        let s = MenuState::NoMenu;
        let s = s.toggle(MenuState::Inventory);
        assert_eq!(s, MenuState::Inventory);
        let s = s.toggle(MenuState::Inventory);
        assert_eq!(s, MenuState::NoMenu);
    }

    #[test]
    fn toggle_replaces_other_overlay() {
        // Pressing Debug while Inventory is open switches straight to Debug.
        let s = MenuState::Inventory.toggle(MenuState::Debug);
        assert_eq!(s, MenuState::Debug);
    }

    #[test]
    fn any_overlay_pauses_and_frees_cursor() {
        for s in [MenuState::Inventory, MenuState::Pause, MenuState::Settings, MenuState::Debug, MenuState::Craft] {
            assert!(s.is_open());
            assert!(s.pauses_game());
            assert!(!s.captures_cursor());
        }
    }

    #[test]
    fn pause_shows_inventory_too() {
        assert!(MenuState::Pause.shows_inventory());
        assert!(MenuState::Inventory.shows_inventory());
        assert!(!MenuState::Debug.shows_inventory());
    }

    #[test]
    fn escape_from_playing_opens_pause() {
        assert_eq!(MenuState::NoMenu.on_escape(), MenuState::Pause);
    }

    #[test]
    fn escape_from_overlay_backs_out() {
        assert_eq!(MenuState::Inventory.on_escape(), MenuState::NoMenu);
        assert_eq!(MenuState::Debug.on_escape(), MenuState::NoMenu);
        // Settings backs out to the pause menu, one level.
        assert_eq!(MenuState::Settings.on_escape(), MenuState::Pause);
    }

    #[test]
    fn exactly_one_overlay_render_flag() {
        // No state should claim two different *primary* panels at once.
        for s in [MenuState::NoMenu, MenuState::Inventory, MenuState::Pause, MenuState::Settings, MenuState::Debug, MenuState::Craft] {
            let primaries = [s.shows_pause_menu(), s.shows_settings(), s.shows_debug(), s.shows_craft()];
            let count = primaries.iter().filter(|b| **b).count();
            assert!(count <= 1, "{:?} claims {} primary panels", s, count);
        }
    }
}
