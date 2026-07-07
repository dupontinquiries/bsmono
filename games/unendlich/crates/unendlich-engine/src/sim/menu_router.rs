//! `sim::menu_router` — between-screen routing with a back-stack.
//!
//! Owns the navigation BETWEEN top-level menu screens (Main, Settings, NewGame,
//! the in-game Pause overlay). Each individual screen owns its *within-screen*
//! focus/cursor state elsewhere — `sim::menu_nav` for the focus cursor,
//! `sim::main_menu` / `sim::ingame_menu` / `sim::new_game_wizard` /
//! `sim::settings_model` for each screen's own model. This router is the layer
//! above them: a screen stack you `push`/`pop`/`replace`/`back` over, with
//! transition guards (you can't pop the root; a screen can demand a
//! confirm-on-discard before it is left).
//!
//! Deliberately NOT reusing `title_flow::Screen` / `main_menu::Screen` /
//! `ingame_menu::Overlay`: each of those is a single screen's *internal* state
//! machine. The router's [`Screen`] is the orthogonal *between-screen* vocabulary
//! — the set of destinations the back-stack moves among. Keeping it separate is
//! what lets the router stay pure and screen-agnostic.
//!
//! Pure: no IO, no rendering, no winit/GLFW, no wall-clock, no RNG. Deterministic
//! and panic-free (the only failure mode is a refused transition, returned as a
//! [`Transition`] value, never a panic).

// ---------------------------------------------------------------------------
// Screens
// ---------------------------------------------------------------------------

/// The top-level destinations the router moves between. This is the
/// *between-screen* vocabulary; a screen's own internal state lives in its
/// dedicated module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// Title / main menu — always the root of the stack.
    Main,
    /// New-game wizard (`sim::new_game_wizard` drives its steps).
    NewGame,
    /// Settings editor (`sim::settings_model` holds the values).
    Settings,
    /// In-game (world is live; no overlay).
    InGame,
    /// Pause overlay on top of the live game (`sim::ingame_menu`).
    Pause,
}

impl Screen {
    /// Whether leaving this screen requires a discard confirmation by default.
    /// The NewGame wizard holds unsaved configuration, so backing out of it is
    /// a discard. Other screens leave freely. A caller can always override by
    /// using the `*_forced` entry points after the user confirms.
    pub fn guards_discard(self) -> bool {
        matches!(self, Screen::NewGame)
    }
}

// ---------------------------------------------------------------------------
// Transition result
// ---------------------------------------------------------------------------

/// The outcome of a routing request. Every router call returns one of these;
/// none of them panic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    /// The stack changed: now showing `to` (was `from`).
    Moved { from: Screen, to: Screen },
    /// Refused: the root screen cannot be popped.
    BlockedRoot,
    /// Refused: leaving `from` needs a discard confirmation first. The caller
    /// should show a confirm prompt and, on confirmation, retry with the
    /// `*_forced` variant.
    NeedsConfirm { from: Screen },
    /// No-op: the request would not change the current screen (e.g. `replace`
    /// with the same screen, or `back`/`pop` requested but already handled).
    NoChange,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// A screen back-stack. The bottom element is the root and can never be popped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Router {
    stack: Vec<Screen>,
}

impl Router {
    /// New router rooted at `root` (typically [`Screen::Main`]).
    pub fn new(root: Screen) -> Self {
        Router { stack: vec![root] }
    }

    /// The screen currently shown (top of stack). Always present — the root
    /// guarantees the stack is never empty.
    pub fn current(&self) -> Screen {
        // Safe: invariants guarantee at least the root is present.
        *self.stack.last().unwrap_or(&Screen::Main)
    }

    /// The root screen (bottom of stack).
    pub fn root(&self) -> Screen {
        *self.stack.first().unwrap_or(&Screen::Main)
    }

    /// Stack depth (1 == only the root is showing).
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// True when only the root remains.
    pub fn at_root(&self) -> bool {
        self.stack.len() <= 1
    }

    /// The full stack, root first. For inspection/tests; the router owns it.
    pub fn stack(&self) -> &[Screen] {
        &self.stack
    }

    // ---- push --------------------------------------------------------------

    /// Push `to` on top of the stack. Pushing the screen that's already on top
    /// is a [`Transition::NoChange`] (no duplicate stacking).
    pub fn push(&mut self, to: Screen) -> Transition {
        let from = self.current();
        if from == to {
            return Transition::NoChange;
        }
        self.stack.push(to);
        Transition::Moved { from, to }
    }

    // ---- pop / back --------------------------------------------------------

    /// Pop the top screen, honoring the discard guard on the screen being left.
    /// Refuses to pop the root ([`Transition::BlockedRoot`]). If the current
    /// screen guards discard, returns [`Transition::NeedsConfirm`] WITHOUT
    /// changing the stack — confirm, then call [`Router::pop_forced`].
    pub fn pop(&mut self) -> Transition {
        if self.at_root() {
            return Transition::BlockedRoot;
        }
        let from = self.current();
        if from.guards_discard() {
            return Transition::NeedsConfirm { from };
        }
        self.pop_unchecked()
    }

    /// Pop the top screen, bypassing the discard guard (call after the user has
    /// confirmed). Still refuses to pop the root.
    pub fn pop_forced(&mut self) -> Transition {
        if self.at_root() {
            return Transition::BlockedRoot;
        }
        self.pop_unchecked()
    }

    /// `back` is an alias for [`Router::pop`] — the conventional "go back one
    /// screen" action, with the same guards.
    pub fn back(&mut self) -> Transition {
        self.pop()
    }

    fn pop_unchecked(&mut self) -> Transition {
        // Precondition (checked by callers): depth > 1.
        if self.stack.len() <= 1 {
            return Transition::BlockedRoot;
        }
        let from = self.stack.pop().unwrap_or_else(|| self.root());
        let to = self.current();
        Transition::Moved { from, to }
    }

    // ---- replace -----------------------------------------------------------

    /// Replace the current top with `to` (same depth). Honors the discard guard
    /// of the screen being replaced. Replacing a screen with itself is
    /// [`Transition::NoChange`].
    pub fn replace(&mut self, to: Screen) -> Transition {
        let from = self.current();
        if from == to {
            return Transition::NoChange;
        }
        if from.guards_discard() {
            return Transition::NeedsConfirm { from };
        }
        self.replace_unchecked(to)
    }

    /// Replace the top with `to`, bypassing the discard guard (post-confirm).
    pub fn replace_forced(&mut self, to: Screen) -> Transition {
        let from = self.current();
        if from == to {
            return Transition::NoChange;
        }
        self.replace_unchecked(to)
    }

    fn replace_unchecked(&mut self, to: Screen) -> Transition {
        let from = self.current();
        let last = self.stack.len() - 1; // depth ≥ 1 always
        self.stack[last] = to;
        Transition::Moved { from, to }
    }

    // ---- reset -------------------------------------------------------------

    /// Pop everything back to the root in one step (e.g. "return to main menu").
    /// Does NOT consult discard guards — it's an explicit, deliberate reset the
    /// caller is responsible for confirming. Returns `Moved` if anything was
    /// popped, else `NoChange`.
    pub fn reset_to_root(&mut self) -> Transition {
        if self.at_root() {
            return Transition::NoChange;
        }
        let from = self.current();
        let root = self.root();
        self.stack.truncate(1);
        Transition::Moved { from, to: root }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn router() -> Router {
        Router::new(Screen::Main)
    }

    // ---- construction ------------------------------------------------------

    #[test]
    fn new_router_is_at_root() {
        let r = router();
        assert_eq!(r.current(), Screen::Main);
        assert_eq!(r.root(), Screen::Main);
        assert_eq!(r.depth(), 1);
        assert!(r.at_root());
    }

    // ---- push --------------------------------------------------------------

    #[test]
    fn push_moves_and_grows_stack() {
        let mut r = router();
        let t = r.push(Screen::Settings);
        assert_eq!(t, Transition::Moved { from: Screen::Main, to: Screen::Settings });
        assert_eq!(r.current(), Screen::Settings);
        assert_eq!(r.depth(), 2);
    }

    #[test]
    fn push_same_screen_is_no_change() {
        let mut r = router();
        r.push(Screen::Settings);
        let t = r.push(Screen::Settings);
        assert_eq!(t, Transition::NoChange);
        assert_eq!(r.depth(), 2, "duplicate push must not grow the stack");
    }

    #[test]
    fn push_order_preserved_in_stack() {
        let mut r = router();
        r.push(Screen::InGame);
        r.push(Screen::Pause);
        assert_eq!(r.stack(), &[Screen::Main, Screen::InGame, Screen::Pause]);
    }

    // ---- pop / back --------------------------------------------------------

    #[test]
    fn pop_returns_to_previous() {
        let mut r = router();
        r.push(Screen::Settings);
        let t = r.pop();
        assert_eq!(t, Transition::Moved { from: Screen::Settings, to: Screen::Main });
        assert_eq!(r.current(), Screen::Main);
        assert!(r.at_root());
    }

    #[test]
    fn root_cannot_be_popped() {
        let mut r = router();
        let t = r.pop();
        assert_eq!(t, Transition::BlockedRoot);
        assert_eq!(r.depth(), 1, "stack unchanged after blocked root pop");
        assert_eq!(r.current(), Screen::Main);
    }

    #[test]
    fn back_is_pop_alias() {
        let mut r = router();
        r.push(Screen::Settings);
        let t = r.back();
        assert_eq!(t, Transition::Moved { from: Screen::Settings, to: Screen::Main });
    }

    #[test]
    fn back_at_root_blocked() {
        let mut r = router();
        assert_eq!(r.back(), Transition::BlockedRoot);
    }

    #[test]
    fn back_stack_order_lifo() {
        let mut r = router();
        r.push(Screen::InGame);
        r.push(Screen::Pause);
        // Pop should peel Pause first, then InGame, then block at Main.
        assert_eq!(r.pop(), Transition::Moved { from: Screen::Pause, to: Screen::InGame });
        assert_eq!(r.pop(), Transition::Moved { from: Screen::InGame, to: Screen::Main });
        assert_eq!(r.pop(), Transition::BlockedRoot);
    }

    // ---- discard guard -----------------------------------------------------

    #[test]
    fn guard_blocks_discard_on_pop() {
        let mut r = router();
        r.push(Screen::NewGame); // NewGame guards discard
        let t = r.pop();
        assert_eq!(t, Transition::NeedsConfirm { from: Screen::NewGame });
        assert_eq!(r.current(), Screen::NewGame, "stack unchanged while confirm pending");
        assert_eq!(r.depth(), 2);
    }

    #[test]
    fn forced_pop_bypasses_guard() {
        let mut r = router();
        r.push(Screen::NewGame);
        let t = r.pop_forced();
        assert_eq!(t, Transition::Moved { from: Screen::NewGame, to: Screen::Main });
        assert_eq!(r.current(), Screen::Main);
    }

    #[test]
    fn forced_pop_still_blocks_root() {
        let mut r = router();
        assert_eq!(r.pop_forced(), Transition::BlockedRoot);
    }

    // ---- replace -----------------------------------------------------------

    #[test]
    fn replace_swaps_top_same_depth() {
        let mut r = router();
        r.push(Screen::Settings);
        let d = r.depth();
        let t = r.replace(Screen::InGame);
        assert_eq!(t, Transition::Moved { from: Screen::Settings, to: Screen::InGame });
        assert_eq!(r.current(), Screen::InGame);
        assert_eq!(r.depth(), d, "replace must not change depth");
    }

    #[test]
    fn replace_same_screen_is_no_change() {
        let mut r = router();
        r.push(Screen::Settings);
        assert_eq!(r.replace(Screen::Settings), Transition::NoChange);
    }

    #[test]
    fn replace_honors_discard_guard() {
        let mut r = router();
        r.push(Screen::NewGame);
        let t = r.replace(Screen::Settings);
        assert_eq!(t, Transition::NeedsConfirm { from: Screen::NewGame });
        assert_eq!(r.current(), Screen::NewGame, "unchanged while confirm pending");
    }

    #[test]
    fn replace_forced_bypasses_guard() {
        let mut r = router();
        r.push(Screen::NewGame);
        let t = r.replace_forced(Screen::Settings);
        assert_eq!(t, Transition::Moved { from: Screen::NewGame, to: Screen::Settings });
        assert_eq!(r.current(), Screen::Settings);
    }

    #[test]
    fn replace_root_is_allowed() {
        // Replacing the root (not popping it) is fine — depth stays 1.
        let mut r = router();
        let t = r.replace(Screen::InGame);
        assert_eq!(t, Transition::Moved { from: Screen::Main, to: Screen::InGame });
        assert_eq!(r.depth(), 1);
        assert_eq!(r.root(), Screen::InGame);
    }

    // ---- reset -------------------------------------------------------------

    #[test]
    fn reset_to_root_pops_all() {
        let mut r = router();
        r.push(Screen::InGame);
        r.push(Screen::Pause);
        let t = r.reset_to_root();
        assert_eq!(t, Transition::Moved { from: Screen::Pause, to: Screen::Main });
        assert!(r.at_root());
        assert_eq!(r.depth(), 1);
    }

    #[test]
    fn reset_at_root_is_no_change() {
        let mut r = router();
        assert_eq!(r.reset_to_root(), Transition::NoChange);
    }

    // ---- invariants / property --------------------------------------------

    #[test]
    fn stack_never_empty_after_any_sequence() {
        // Drive a deterministic sequence of ops; stack must always keep ≥1.
        let mut r = router();
        let ops = [0u8, 1, 2, 3, 1, 1, 1, 1, 2, 0, 3, 3, 0, 1];
        let screens = [Screen::Settings, Screen::NewGame, Screen::InGame, Screen::Pause];
        let mut si = 0usize;
        for &op in &ops {
            match op {
                0 => { let _ = r.push(screens[si % screens.len()]); si += 1; }
                1 => { let _ = r.pop_forced(); }      // forced to actually move
                2 => { let _ = r.replace_forced(screens[si % screens.len()]); si += 1; }
                _ => { let _ = r.back(); }
            }
            assert!(r.depth() >= 1, "stack went empty after op {op}");
            // current() must always be the actual top of the stack.
            assert_eq!(r.current(), *r.stack().last().unwrap());
        }
    }

    #[test]
    fn depth_matches_push_minus_pop() {
        let mut r = router();
        r.push(Screen::Settings);   // 2
        r.push(Screen::InGame);     // 3
        r.push(Screen::Pause);      // 4
        assert_eq!(r.depth(), 4);
        r.pop_forced();             // 3
        r.pop_forced();             // 2
        assert_eq!(r.depth(), 2);
    }

    #[test]
    fn screen_guards_discard_only_newgame() {
        assert!(Screen::NewGame.guards_discard());
        for s in [Screen::Main, Screen::Settings, Screen::InGame, Screen::Pause] {
            assert!(!s.guards_discard(), "{s:?} should not guard discard");
        }
    }
}

// ── r335-menus ───────────────────────────────────────────────────────────────
// Nested-submenu page routing for the new screens (Singleplayer / Host / Join /
// Settings → sub-pages, plus the World-Management page and the gamemode toggle).
//
// This is ADDITIVE and sits alongside the existing `Router`/`Screen` above (the
// between-top-level-screen back-stack). `MenuPage` is the one new enum this
// thread adds (flagged per the brief): the *page identity* the renderer draws
// and titles as the user descends the `MenuNav` submenu tree. The tree itself is
// built in `menu_screen_layout::main_menu_tree`; this router keeps a page stack
// in lockstep with `MenuNav`'s frame stack so the renderer always knows the
// current page and `back` restores both the page and the parent focus.
use crate::sim::menu_nav::{MenuNav, NavDir, NavEvent};

/// A named page in the nested main-menu tree. Submenu `MenuItem`s carry the
/// matching [`MenuPage::id`] so the router can recover the page on descent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuPage {
    Root,
    Singleplayer,
    Host,
    Join,
    Settings,
    SettingsGraphics,
    SettingsCameraGameplay,
    SettingsAudio,
    /// Hosts T-worldmgmt's "Open to DHT / Whitelist / Max Players / Name" page
    /// (coordinator wires the `world_management` model into this slot).
    WorldManagement,
    Multiplayer,
}

impl MenuPage {
    pub const ALL: [MenuPage; 10] = [
        MenuPage::Root, MenuPage::Singleplayer, MenuPage::Host, MenuPage::Join,
        MenuPage::Settings, MenuPage::SettingsGraphics, MenuPage::SettingsCameraGameplay,
        MenuPage::SettingsAudio, MenuPage::WorldManagement, MenuPage::Multiplayer,
    ];

    /// Stable submenu id (disjoint from [`MenuAction`] ids). `Root` has no item.
    pub fn id(self) -> u16 {
        0xE000
            + match self {
                MenuPage::Root => 0,
                MenuPage::Singleplayer => 1,
                MenuPage::Host => 2,
                MenuPage::Join => 3,
                MenuPage::Settings => 4,
                MenuPage::SettingsGraphics => 5,
                MenuPage::SettingsCameraGameplay => 6,
                MenuPage::SettingsAudio => 7,
                MenuPage::WorldManagement => 8,
                MenuPage::Multiplayer => 9,
            }
    }

    /// Recover the page from a submenu id (inverse of [`MenuPage::id`]).
    pub fn from_id(id: u16) -> Option<MenuPage> {
        MenuPage::ALL.iter().copied().find(|p| p.id() == id)
    }

    /// Renderer title for the page.
    pub fn title(self) -> &'static str {
        match self {
            MenuPage::Root => "Menu",
            MenuPage::Singleplayer => "Singleplayer",
            MenuPage::Host => "Host",
            MenuPage::Join => "Join",
            MenuPage::Settings => "Settings",
            MenuPage::SettingsGraphics => "Graphics",
            MenuPage::SettingsCameraGameplay => "Camera & Gameplay",
            MenuPage::SettingsAudio => "Audio",
            MenuPage::WorldManagement => "World Management",
            MenuPage::Multiplayer => "Multiplayer",
        }
    }
}

/// Stable leaf-action ids in the nested tree. The bin matches on
/// `NavEvent::Activated(id)` to run the action (e.g. toggle gamemode).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    NewWorld,
    LoadWorld,
    ServerBrowser,
    DirectConnect,
    /// Hook for T-gamemode: the coordinator wires `game_mode::toggle()` here.
    GamemodeToggle,
    /// Hooks for T-worldmgmt's page fields (placeholders until wired).
    OpenToDht,
    WhitelistToggle,
    MaxPlayers,
    WorldName,
}

impl MenuAction {
    pub const ALL: [MenuAction; 9] = [
        MenuAction::NewWorld, MenuAction::LoadWorld, MenuAction::ServerBrowser,
        MenuAction::DirectConnect, MenuAction::GamemodeToggle, MenuAction::OpenToDht,
        MenuAction::WhitelistToggle, MenuAction::MaxPlayers, MenuAction::WorldName,
    ];

    pub fn id(self) -> u16 {
        0xE100
            + match self {
                MenuAction::NewWorld => 0,
                MenuAction::LoadWorld => 1,
                MenuAction::ServerBrowser => 2,
                MenuAction::DirectConnect => 3,
                MenuAction::GamemodeToggle => 4,
                MenuAction::OpenToDht => 5,
                MenuAction::WhitelistToggle => 6,
                MenuAction::MaxPlayers => 7,
                MenuAction::WorldName => 8,
            }
    }

    pub fn from_id(id: u16) -> Option<MenuAction> {
        MenuAction::ALL.iter().copied().find(|a| a.id() == id)
    }
}

/// Shared id for every `Back` sentinel in the tree (Back items needn't be unique).
pub const ID_BACK: u16 = 0xE0FF;

/// A nested-menu router: the real [`MenuNav`] over the page tree, plus a parallel
/// stack of the [`MenuPage`]s descended into. The invariant
/// `pages.len() == nav.depth() + 1` is maintained by every transition, so
/// [`MenuTreeRouter::current_page`] always names the visible page and
/// [`MenuTreeRouter::back`] restores both the page and the parent's focus.
#[derive(Clone, Debug)]
pub struct MenuTreeRouter {
    nav: MenuNav,
    pages: Vec<MenuPage>,
}

impl MenuTreeRouter {
    /// Root the router at `tree` (typically `menu_screen_layout::main_menu_tree()`).
    pub fn new(tree: Vec<crate::sim::menu_nav::MenuItem>) -> MenuTreeRouter {
        MenuTreeRouter { nav: MenuNav::new(tree), pages: vec![MenuPage::Root] }
    }

    /// The page currently shown (top of the page stack).
    pub fn current_page(&self) -> MenuPage {
        *self.pages.last().unwrap_or(&MenuPage::Root)
    }

    /// The id of the focused item, or `None` if the page is empty.
    pub fn focused_id(&self) -> Option<u16> {
        self.nav.focused_id()
    }

    /// Nesting depth (0 = root page).
    pub fn depth(&self) -> usize {
        self.nav.depth()
    }

    /// Move focus within the current page (wraps, skips disabled — delegates to
    /// `MenuNav`).
    pub fn move_focus(&mut self, dir: NavDir) -> NavEvent {
        self.nav.move_focus(dir)
    }

    /// Activate the focused item, keeping the page stack in lockstep:
    /// - submenu → descend in `MenuNav` and push the item's [`MenuPage`];
    /// - `Back` item → pop a page (`MenuNav` already popped its frame);
    /// - action → `Activated(id)`, no page change.
    pub fn activate(&mut self) -> NavEvent {
        // Capture the focused submenu's id BEFORE descending, so we can map it
        // to a page if this turns out to be a Descended.
        let focused = self.nav.focused_id();
        let ev = self.nav.activate();
        match ev {
            NavEvent::Descended => {
                // Map the descended submenu to its page; fall back to the parent
                // page only to preserve the depth invariant for an unmapped tree
                // (the shipped `main_menu_tree` maps every submenu).
                let page = focused.and_then(MenuPage::from_id).unwrap_or(self.current_page());
                self.pages.push(page);
            }
            NavEvent::Backed => {
                if self.pages.len() > 1 {
                    self.pages.pop();
                }
            }
            _ => {}
        }
        ev
    }

    /// Back out one page. Pops the page stack and restores parent focus (via
    /// `MenuNav`). Returns `false` at the root page (the caller may treat that as
    /// "close the menu").
    pub fn back(&mut self) -> bool {
        let backed = self.nav.back();
        if backed && self.pages.len() > 1 {
            self.pages.pop();
        }
        backed
    }

    /// The page stack, root first (for inspection/tests).
    pub fn pages(&self) -> &[MenuPage] {
        &self.pages
    }
}

#[cfg(test)]
mod r335_menus_tests {
    use super::*;
    use crate::sim::menu_nav::{NavDir, NavEvent};
    use crate::sim::menu_screen_layout::main_menu_tree;

    fn router() -> MenuTreeRouter {
        MenuTreeRouter::new(main_menu_tree())
    }

    #[test]
    fn ids_round_trip_and_are_disjoint() {
        for p in MenuPage::ALL {
            assert_eq!(MenuPage::from_id(p.id()), Some(p));
            assert!(!p.title().is_empty());
        }
        for a in MenuAction::ALL {
            assert_eq!(MenuAction::from_id(a.id()), Some(a));
            // page ids and action ids never collide
            assert!(MenuPage::from_id(a.id()).is_none());
        }
        assert!(MenuAction::from_id(ID_BACK).is_none());
        assert!(MenuPage::from_id(ID_BACK).is_none());
    }

    #[test]
    fn enter_pushes_page_and_resets_focus() {
        let mut r = router();
        assert_eq!(r.current_page(), MenuPage::Root);
        // focus the Host entry, then descend.
        while r.focused_id() != Some(MenuPage::Host.id()) {
            assert_eq!(r.move_focus(NavDir::Down), NavEvent::Navigated);
        }
        let ev = r.activate();
        assert_eq!(ev, NavEvent::Descended);
        assert_eq!(r.current_page(), MenuPage::Host);
        assert_eq!(r.depth(), 1);
        // focus reset to the first child of the Host page.
        assert_eq!(r.focused_id(), Some(MenuPage::WorldManagement.id()));
        assert_eq!(r.pages(), &[MenuPage::Root, MenuPage::Host]);
    }

    #[test]
    fn back_pops_page_and_restores_parent_focus() {
        let mut r = router();
        while r.focused_id() != Some(MenuPage::Settings.id()) {
            r.move_focus(NavDir::Down);
        }
        let parent_focus = r.focused_id();
        r.activate(); // into Settings
        assert_eq!(r.current_page(), MenuPage::Settings);
        assert!(r.back());
        assert_eq!(r.current_page(), MenuPage::Root);
        assert_eq!(r.depth(), 0);
        assert_eq!(r.focused_id(), parent_focus, "parent focus restored");
    }

    #[test]
    fn deep_path_round_trips() {
        // Root → Host → WorldManagement → back → back → Root.
        let mut r = router();
        while r.focused_id() != Some(MenuPage::Host.id()) { r.move_focus(NavDir::Down); }
        r.activate(); // Host (first child is WorldManagement)
        assert_eq!(r.focused_id(), Some(MenuPage::WorldManagement.id()));
        r.activate(); // WorldManagement
        assert_eq!(r.current_page(), MenuPage::WorldManagement);
        assert_eq!(r.depth(), 2);
        assert_eq!(r.pages(), &[MenuPage::Root, MenuPage::Host, MenuPage::WorldManagement]);
        assert!(r.back());
        assert_eq!(r.current_page(), MenuPage::Host);
        assert!(r.back());
        assert_eq!(r.current_page(), MenuPage::Root);
        assert!(!r.back(), "back at root returns false");
        assert_eq!(r.current_page(), MenuPage::Root);
    }

    #[test]
    fn activating_an_action_keeps_the_page() {
        // Navigate Root → Settings, focus the Gamemode toggle action, activate.
        let mut r = router();
        while r.focused_id() != Some(MenuPage::Settings.id()) { r.move_focus(NavDir::Down); }
        r.activate(); // Settings
        let page_before = r.current_page();
        let depth_before = r.depth();
        while r.focused_id() != Some(MenuAction::GamemodeToggle.id()) {
            assert_eq!(r.move_focus(NavDir::Down), NavEvent::Navigated);
        }
        let ev = r.activate();
        assert_eq!(ev, NavEvent::Activated(MenuAction::GamemodeToggle.id()));
        assert_eq!(r.current_page(), page_before, "actions don't change page");
        assert_eq!(r.depth(), depth_before);
    }

    #[test]
    fn page_stack_invariant_holds_across_a_driven_sequence() {
        let mut r = router();
        // Drive a deterministic mix of moves/activates/backs; the invariant
        // pages.len() == depth()+1 must always hold, and current_page is the top.
        let script = [0u8, 1, 0, 1, 2, 0, 1, 2, 2, 1, 0, 1, 2, 2, 2];
        for &op in &script {
            match op {
                0 => { r.move_focus(NavDir::Down); }
                1 => { r.activate(); }
                _ => { r.back(); }
            }
            assert_eq!(r.pages().len(), r.depth() + 1, "page/nav depth desync");
            assert_eq!(*r.pages().last().unwrap(), r.current_page());
        }
    }

    #[test]
    fn deterministic() {
        let run = || {
            let mut r = router();
            while r.focused_id() != Some(MenuPage::Host.id()) { r.move_focus(NavDir::Down); }
            r.activate();
            r.activate();
            (r.current_page(), r.depth(), r.focused_id())
        };
        assert_eq!(run(), run());
    }
}
