//! `sim::menu_screen_layout` — turn a title/pause `Screen` into a renderable
//! list of focusable widgets, and route focus across them.
//!
//! The bin owns pixels and input devices; this owns *what is on the screen and
//! which item is focused*. It composes with [`crate::sim::title_flow::Screen`]
//! (which panel is showing) and [`crate::sim::menu_nav::NavDir`] (the direction
//! of a focus move).
//!
//! Pure and deterministic: data in, data out. No GPU, IO, clock, or RNG; no
//! `.unwrap()`/`.expect()` in non-test code. Never panics on empty/all-disabled
//! screens.

use crate::sim::menu_nav::NavDir;
use crate::sim::title_flow::Screen;

/// The kind of widget — drives how the bin draws and edits it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetKind {
    Button,
    Toggle,
    Slider,
    Label,
    TextField,
}

/// One focusable (or static, for `Label`) widget on a screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub id: u16,
    pub kind: WidgetKind,
    pub label: String,
    pub enabled: bool,
    /// r288: for Sliders, the current value rendered next to the label (e.g.
    /// "12" for Render Distance). `None` for non-valued widgets. The renderer
    /// shows `"{label}: {value}"` when present.
    pub value: Option<String>,
}

impl Widget {
    fn new(id: u16, kind: WidgetKind, label: &str, enabled: bool) -> Widget {
        Widget { id, kind, label: label.to_string(), enabled, value: None }
    }

    /// Builder: attach a current-value string (for Sliders) shown next to the label.
    pub fn with_value(mut self, value: impl Into<String>) -> Widget {
        self.value = Some(value.into());
        self
    }

    /// The text the renderer should display: `"label: value"` for valued widgets,
    /// else just the label.
    pub fn display_text(&self) -> String {
        match &self.value {
            Some(v) => format!("{}: {}", self.label, v),
            None => self.label.clone(),
        }
    }
}

/// A whole screen's renderable layout + which widget index is focused.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenLayout {
    pub title: String,
    pub widgets: Vec<Widget>,
    pub focused: usize,
}

impl ScreenLayout {
    /// Build a layout, setting `focused` to the first enabled widget (or 0).
    fn new(title: &str, widgets: Vec<Widget>) -> ScreenLayout {
        let focused = widgets.iter().position(|w| w.enabled).unwrap_or(0);
        ScreenLayout { title: title.to_string(), widgets, focused }
    }
}

// Stable widget ids for the Title screen (so the bin can match on intent).
pub const ID_CONTINUE: u16 = 0;
pub const ID_NEW: u16 = 1;
pub const ID_LOAD: u16 = 2;
pub const ID_SETTINGS: u16 = 3;
pub const ID_MULTIPLAYER: u16 = 4;
pub const ID_QUIT: u16 = 5;
pub const ID_TUTORIAL: u16 = 6; // r656: bending practice world (Singleplayer submenu)
pub const ID_BACK: u16 = 100;

/// Build the renderable layout for a given title/pause screen.
///
/// - `Title`: Continue (enabled iff `has_save`) / New / Load / Settings /
///   Multiplayer / Quit.
/// - `LoadSlots`: exactly `slot_count` save-slot buttons (ids `0..slot_count`).
/// - other screens: a small, sensible widget set; never empty-panics.
pub fn layout_for_title(screen: Screen, has_save: bool, slot_count: u8) -> ScreenLayout {
    use WidgetKind::*;
    match screen {
        Screen::Title => ScreenLayout::new(
            "unendlich",
            vec![
                Widget::new(ID_CONTINUE, Button, "Continue", has_save),
                Widget::new(ID_NEW, Button, "New Game", true),
                Widget::new(ID_LOAD, Button, "Load Game", true),
                Widget::new(ID_SETTINGS, Button, "Settings", true),
                Widget::new(ID_MULTIPLAYER, Button, "Multiplayer", true),
                Widget::new(ID_QUIT, Button, "Quit", true),
            ],
        ),
        Screen::Singleplayer => ScreenLayout::new(
            "Singleplayer",
            vec![
                Widget::new(ID_NEW, Button, "Create World", true),
                Widget::new(ID_LOAD, Button, "Load World", true),
                Widget::new(ID_TUTORIAL, Button, "Bending Tutorial", true),
                Widget::new(ID_BACK, Button, "Back", true),
            ],
        ),
        Screen::LoadSlots => {
            let mut widgets = Vec::with_capacity(slot_count as usize);
            for i in 0..slot_count {
                widgets.push(Widget::new(i as u16, Button, "Save Slot", true));
            }
            ScreenLayout::new("Load Game", widgets)
        }
        Screen::NewGame => ScreenLayout::new(
            "New Game",
            vec![
                Widget::new(0, TextField, "World Name", true),
                Widget::new(1, Slider, "World Size", true),
                Widget::new(2, Toggle, "Peaceful", true),
                Widget::new(3, Button, "Create", true),
                Widget::new(ID_BACK, Button, "Back", true),
            ],
        ),
        Screen::Settings => ScreenLayout::new(
            "Settings",
            vec![
                Widget::new(0, Slider, "Render Distance", true),
                Widget::new(1, Slider, "FOV", true),
                Widget::new(2, Toggle, "Fullscreen", true),
                Widget::new(3, Slider, "Master Volume", true),
                Widget::new(ID_BACK, Button, "Back", true),
            ],
        ),
        Screen::Multiplayer => ScreenLayout::new(
            "Multiplayer",
            vec![
                Widget::new(0, TextField, "Server Address", true),
                Widget::new(1, Button, "Connect", true),
                Widget::new(ID_BACK, Button, "Back", true),
            ],
        ),
        Screen::Credits => ScreenLayout::new(
            "Credits",
            vec![
                Widget::new(0, Label, "unendlich", false),
                Widget::new(1, Label, "made with care", false),
                Widget::new(ID_BACK, Button, "Back", true),
            ],
        ),
        Screen::ConfirmQuit => ScreenLayout::new(
            "Quit to desktop?",
            vec![
                Widget::new(0, Label, "Unsaved progress will be lost.", false),
                Widget::new(1, Button, "Quit", true),
                Widget::new(2, Button, "Cancel", true),
            ],
        ),
    }
}

// ---- r330-menus: Camera / Gameplay settings page ----------------------------
//
// A standalone settings sub-page listing the existing FOV control plus the r330
// graphics knobs (render scale, shadow resolution, dynamic lights) and the
// gameplay toggles. Provided as a free builder rather than a new `Screen`
// variant so it stays inside this worker's files (SHARED_RULES §4); the bin
// handoff proposes a `Screen::CameraGameplay` route that delegates here.

/// Stable widget ids for the Camera/Gameplay page, so the bin can map a focused
/// widget back to the `sim::settings_model::SettingField` it edits.
pub const ID_CG_FOV: u16 = 0;
pub const ID_CG_RENDER_SCALE: u16 = 1;
pub const ID_CG_SHADOW_RES: u16 = 2;
pub const ID_CG_DYNAMIC_LIGHTS: u16 = 3;
pub const ID_CG_INVERT_Y: u16 = 4;
// ID_BACK (100) closes the page.

/// Build the Camera/Gameplay settings page: FOV first, then the r330 graphics
/// knobs in order, then the gameplay toggle(s), then Back. Pure and
/// deterministic; the bin attaches live value strings via [`Widget::with_value`]
/// when it draws each control.
pub fn camera_gameplay_layout() -> ScreenLayout {
    use WidgetKind::*;
    ScreenLayout::new(
        "Camera / Gameplay",
        vec![
            Widget::new(ID_CG_FOV, Slider, "FOV", true),
            Widget::new(ID_CG_RENDER_SCALE, Slider, "Render Scale", true),
            Widget::new(ID_CG_SHADOW_RES, Slider, "Shadow Resolution", true),
            Widget::new(ID_CG_DYNAMIC_LIGHTS, Slider, "Dynamic Lights", true),
            Widget::new(ID_CG_INVERT_Y, Toggle, "Invert Y", true),
            Widget::new(ID_BACK, Button, "Back", true),
        ],
    )
}

/// Move focus to the next enabled widget in `dir`, skipping disabled widgets
/// and wrapping at the ends. No-op on an empty or all-disabled screen.
pub fn focus(layout: &mut ScreenLayout, dir: NavDir) {
    let n = layout.widgets.len();
    if n == 0 {
        return;
    }
    // step forward for Down, backward (n-1 ≡ -1 mod n) for Up.
    let step = match dir {
        NavDir::Down => 1usize,
        NavDir::Up => n - 1,
    };
    let start = layout.focused.min(n - 1);
    let mut i = start;
    for _ in 0..n {
        i = (i + step) % n;
        if layout.widgets[i].enabled {
            layout.focused = i;
            return;
        }
    }
    // nothing enabled — leave focus where it is.
}

/// Id of the currently focused widget, or `None` if the screen is empty.
pub fn focused_id(layout: &ScreenLayout) -> Option<u16> {
    layout.widgets.get(layout.focused).map(|w| w.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_enables_continue_only_with_save() {
        let no = layout_for_title(Screen::Title, false, 0);
        let yes = layout_for_title(Screen::Title, true, 0);
        let cont_no_enabled = no.widgets.iter().find(|w| w.id == ID_CONTINUE).map(|w| w.enabled);
        let cont_yes_enabled = yes.widgets.iter().find(|w| w.id == ID_CONTINUE).map(|w| w.enabled);
        assert_eq!(cont_no_enabled, Some(false));
        assert_eq!(cont_yes_enabled, Some(true));
        // expected six entries
        let ids: Vec<u16> = no.widgets.iter().map(|w| w.id).collect();
        assert_eq!(ids, vec![ID_CONTINUE, ID_NEW, ID_LOAD, ID_SETTINGS, ID_MULTIPLAYER, ID_QUIT]);
    }

    #[test]
    fn initial_focus_is_first_enabled() {
        // Continue disabled (no save) → focus starts on New (index 1).
        let no = layout_for_title(Screen::Title, false, 0);
        assert_eq!(no.focused, 1);
        assert_eq!(focused_id(&no), Some(ID_NEW));
        // with a save, focus starts on Continue.
        let yes = layout_for_title(Screen::Title, true, 0);
        assert_eq!(yes.focused, 0);
        assert_eq!(focused_id(&yes), Some(ID_CONTINUE));
    }

    #[test]
    fn loadslots_emits_exactly_slot_count() {
        for &n in &[0u8, 1, 3, 8, 32] {
            let l = layout_for_title(Screen::LoadSlots, true, n);
            assert_eq!(l.widgets.len(), n as usize, "slot count {}", n);
            for (i, w) in l.widgets.iter().enumerate() {
                assert_eq!(w.id, i as u16);
                assert_eq!(w.kind, WidgetKind::Button);
            }
        }
    }

    #[test]
    fn focus_skips_disabled_and_wraps() {
        // widgets: [en, dis, en, dis] focused starts at 0
        let mut l = ScreenLayout {
            title: "t".into(),
            widgets: vec![
                Widget::new(10, WidgetKind::Button, "a", true),
                Widget::new(11, WidgetKind::Button, "b", false),
                Widget::new(12, WidgetKind::Button, "c", true),
                Widget::new(13, WidgetKind::Button, "d", false),
            ],
            focused: 0,
        };
        focus(&mut l, NavDir::Down); // 0 -> skip 1 -> 2
        assert_eq!(l.focused, 2);
        focus(&mut l, NavDir::Down); // 2 -> wrap, skip 3 -> 0
        assert_eq!(l.focused, 0);
        focus(&mut l, NavDir::Up); // 0 -> wrap backward, skip 3 -> 2
        assert_eq!(l.focused, 2);
        focus(&mut l, NavDir::Up); // 2 -> skip 1 -> 0
        assert_eq!(l.focused, 0);
    }

    #[test]
    fn focused_id_tracks_focus() {
        let mut l = layout_for_title(Screen::Settings, true, 0);
        let id0 = focused_id(&l);
        assert_eq!(id0, Some(l.widgets[l.focused].id));
        focus(&mut l, NavDir::Down);
        assert_eq!(focused_id(&l), Some(l.widgets[l.focused].id));
    }

    #[test]
    fn empty_and_all_disabled_no_panic() {
        // empty (LoadSlots, 0 slots)
        let mut empty = layout_for_title(Screen::LoadSlots, true, 0);
        focus(&mut empty, NavDir::Down);
        focus(&mut empty, NavDir::Up);
        assert_eq!(focused_id(&empty), None);

        // all-disabled
        let mut all_dis = ScreenLayout {
            title: "t".into(),
            widgets: vec![
                Widget::new(1, WidgetKind::Label, "x", false),
                Widget::new(2, WidgetKind::Label, "y", false),
            ],
            focused: 0,
        };
        focus(&mut all_dis, NavDir::Down); // no enabled → stays
        assert_eq!(all_dis.focused, 0);
        assert_eq!(focused_id(&all_dis), Some(1)); // valid widget, just disabled
    }

    #[test]
    fn every_screen_builds_without_panic() {
        for s in [
            Screen::Title, Screen::NewGame, Screen::LoadSlots, Screen::Settings,
            Screen::Multiplayer, Screen::Credits, Screen::ConfirmQuit,
        ] {
            let mut l = layout_for_title(s, true, 4);
            // navigating a full cycle never panics and keeps focus in range
            for _ in 0..(l.widgets.len() + 2) {
                focus(&mut l, NavDir::Down);
                assert!(l.widgets.is_empty() || l.focused < l.widgets.len());
            }
            let _ = focused_id(&l);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(layout_for_title(Screen::Title, true, 0), layout_for_title(Screen::Title, true, 0));
        assert_eq!(layout_for_title(Screen::LoadSlots, false, 5), layout_for_title(Screen::LoadSlots, false, 5));
    }

    // ---- r330-menus tests ---------------------------------------------------

    #[test]
    fn r330_camera_page_lists_fov_then_new_fields_in_order() {
        let l = camera_gameplay_layout();
        assert_eq!(l.title, "Camera / Gameplay");
        let ids: Vec<u16> = l.widgets.iter().map(|w| w.id).collect();
        assert_eq!(
            ids,
            vec![
                ID_CG_FOV,
                ID_CG_RENDER_SCALE,
                ID_CG_SHADOW_RES,
                ID_CG_DYNAMIC_LIGHTS,
                ID_CG_INVERT_Y,
                ID_BACK,
            ],
            "FOV leads, then the r330 knobs in order, gameplay toggle, then Back"
        );
        // FOV leads as a slider; the gameplay knob is a toggle.
        assert_eq!(l.widgets[0].kind, WidgetKind::Slider);
        assert_eq!(l.widgets[0].label, "FOV");
        let invert_kind = l.widgets.iter().find(|w| w.id == ID_CG_INVERT_Y).map(|w| w.kind);
        assert_eq!(invert_kind, Some(WidgetKind::Toggle));
    }

    #[test]
    fn r330_camera_page_focus_starts_on_fov_and_cycles() {
        let mut l = camera_gameplay_layout();
        assert_eq!(focused_id(&l), Some(ID_CG_FOV), "first enabled widget is FOV");
        // a full Down cycle keeps focus in range and returns to FOV.
        let n = l.widgets.len();
        for _ in 0..n {
            focus(&mut l, NavDir::Down);
            assert!(l.focused < l.widgets.len());
        }
        assert_eq!(focused_id(&l), Some(ID_CG_FOV), "wrapped back to FOV");
    }

    #[test]
    fn r330_camera_page_has_back_and_is_deterministic() {
        let l = camera_gameplay_layout();
        assert!(l.widgets.iter().any(|w| w.id == ID_BACK), "page carries a Back button");
        assert_eq!(camera_gameplay_layout(), camera_gameplay_layout(), "pure/deterministic");
    }
}

// ── r335-menus ───────────────────────────────────────────────────────────────
// The nested main-menu submenu TREE the new screens compose into, consumed by
// `menu_router::MenuTreeRouter` (which drives focus/back over it via `MenuNav`).
// Additive: builds `menu_nav::MenuItem` trees only; no rendering. Every submenu
// carries its `MenuPage::id()` and every leaf an `MenuAction::id()`, so the
// router recovers page/action identity. Placeholder slots are left for the
// gamemode toggle (T-gamemode) and the World-Management page (T-worldmgmt).
use crate::sim::menu_nav::MenuItem;
use crate::sim::menu_router::{MenuAction, MenuPage};

fn act(a: MenuAction) -> MenuItem {
    MenuItem::action(a.id(), true)
}
fn back() -> MenuItem {
    // `menu_router::ID_BACK` (fully qualified to avoid clashing with this
    // module's own Title-screen `ID_BACK` widget constant).
    MenuItem::back(crate::sim::menu_router::ID_BACK)
}
fn page(p: MenuPage, children: Vec<MenuItem>) -> MenuItem {
    MenuItem::submenu(p.id(), true, children)
}

fn singleplayer_page() -> Vec<MenuItem> {
    vec![act(MenuAction::NewWorld), act(MenuAction::LoadWorld), back()]
}

/// The World-Management page (Open to DHT / Whitelist / Max Players / Name).
/// These are placeholder action slots; the coordinator wires T-worldmgmt's
/// `world_management` model (validated setters) onto them on merge.
fn world_management_page() -> Vec<MenuItem> {
    vec![
        act(MenuAction::OpenToDht),
        act(MenuAction::WhitelistToggle),
        act(MenuAction::MaxPlayers),
        act(MenuAction::WorldName),
        back(),
    ]
}

fn host_page() -> Vec<MenuItem> {
    vec![page(MenuPage::WorldManagement, world_management_page()), back()]
}

fn join_page() -> Vec<MenuItem> {
    vec![act(MenuAction::ServerBrowser), act(MenuAction::DirectConnect), back()]
}

fn settings_page() -> Vec<MenuItem> {
    vec![
        // r330 settings sub-pages — placeholder leaves (Back only) the coordinator
        // fills from `settings_model` / `camera_gameplay_layout` on merge.
        page(MenuPage::SettingsGraphics, vec![back()]),
        page(MenuPage::SettingsCameraGameplay, vec![back()]),
        page(MenuPage::SettingsAudio, vec![back()]),
        // T-gamemode hook: the coordinator wires `game_mode::toggle()` to this id.
        act(MenuAction::GamemodeToggle),
        back(),
    ]
}

/// Build the full nested main-menu tree:
/// `Root → { Singleplayer, Host → { World Management }, Join, Settings →
/// { Graphics, Camera & Gameplay, Audio, Gamemode toggle } }`.
///
/// Feed it to `menu_router::MenuTreeRouter::new`. Pure and deterministic.
pub fn main_menu_tree() -> Vec<MenuItem> {
    vec![
        page(MenuPage::Singleplayer, singleplayer_page()),
        page(MenuPage::Host, host_page()),
        page(MenuPage::Join, join_page()),
        page(MenuPage::Settings, settings_page()),
    ]
}

#[cfg(test)]
mod r335_menus_layout_tests {
    use super::*;
    use crate::sim::menu_nav::ItemKind;

    // Recursively collect (submenu page ids) and (leaf action ids).
    fn walk(items: &[MenuItem], pages: &mut Vec<u16>, actions: &mut Vec<u16>) {
        for it in items {
            match &it.kind {
                ItemKind::Submenu(children) => {
                    pages.push(it.id);
                    walk(children, pages, actions);
                }
                ItemKind::Action => actions.push(it.id),
                ItemKind::Back => {}
            }
        }
    }

    #[test]
    fn root_has_the_four_top_pages() {
        let t = main_menu_tree();
        let ids: Vec<u16> = t.iter().map(|i| i.id).collect();
        assert_eq!(
            ids,
            vec![
                MenuPage::Singleplayer.id(),
                MenuPage::Host.id(),
                MenuPage::Join.id(),
                MenuPage::Settings.id()
            ]
        );
    }

    #[test]
    fn every_submenu_maps_to_a_page_and_every_action_to_an_action() {
        let t = main_menu_tree();
        let (mut pages, mut actions) = (Vec::new(), Vec::new());
        walk(&t, &mut pages, &mut actions);
        assert!(!pages.is_empty() && !actions.is_empty());
        for id in &pages {
            assert!(MenuPage::from_id(*id).is_some(), "unmapped submenu id {id:#x}");
        }
        for id in &actions {
            assert!(MenuAction::from_id(*id).is_some(), "unmapped action id {id:#x}");
        }
    }

    #[test]
    fn world_management_under_host_and_gamemode_under_settings() {
        let t = main_menu_tree();
        // WorldManagement is a child of Host.
        let host = t.iter().find(|i| i.id == MenuPage::Host.id()).unwrap();
        if let ItemKind::Submenu(children) = &host.kind {
            assert!(children.iter().any(|c| c.id == MenuPage::WorldManagement.id()));
        } else {
            panic!("Host is not a submenu");
        }
        // Gamemode toggle is a child action of Settings.
        let settings = t.iter().find(|i| i.id == MenuPage::Settings.id()).unwrap();
        if let ItemKind::Submenu(children) = &settings.kind {
            assert!(children
                .iter()
                .any(|c| c.id == MenuAction::GamemodeToggle.id()
                    && matches!(c.kind, ItemKind::Action)));
        } else {
            panic!("Settings is not a submenu");
        }
    }

    #[test]
    fn no_duplicate_sibling_ids_and_back_present_per_submenu() {
        // `require_back` is false at the root (root exits, it has no Back) and
        // true inside every submenu the user can back out of.
        fn check(items: &[MenuItem], require_back: bool) {
            let mut seen = std::collections::BTreeSet::new();
            let mut has_back = false;
            for it in items {
                match &it.kind {
                    ItemKind::Back => has_back = true,
                    _ => {
                        assert!(seen.insert(it.id), "duplicate sibling id {:#x}", it.id);
                    }
                }
                if let ItemKind::Submenu(children) = &it.kind {
                    check(children, true);
                }
            }
            if require_back {
                assert!(has_back, "every submenu level should offer a Back");
            }
        }
        check(&main_menu_tree(), false);
    }

    #[test]
    fn deterministic() {
        assert_eq!(main_menu_tree(), main_menu_tree());
    }
}
