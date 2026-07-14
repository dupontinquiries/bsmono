//! Menu + settings — the platform-agnostic UI *logic* (screens, navigation,
//! settings, keybinds), kept separate from rendering the way unendlich splits
//! its `menu.rs` logic from the `egui_overlay` renderer. The app drives this with
//! key/pointer input and an egui (or any) front-end reads it to draw.

use crate::class::{Class, CLASSES};
use crate::hunter::Kind;
use crate::sim::Mode;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Screen { Title, Play, Settings, Keybinds }

/// A bindable in-game action.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Action { Up, Down, Left, Right, Crouch, Dodge, Restart, Menu }

pub const ACTIONS: [Action; 8] = [
    Action::Up, Action::Down, Action::Left, Action::Right,
    Action::Crouch, Action::Dodge, Action::Restart, Action::Menu,
];

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Up => "Move up", Action::Down => "Move down", Action::Left => "Move left",
            Action::Right => "Move right", Action::Crouch => "Hide / Climb", Action::Dodge => "Dodge roll",
            Action::Restart => "Restart round", Action::Menu => "Menu",
        }
    }
    fn default_key(self) -> &'static str {
        match self {
            Action::Up => "W", Action::Down => "S", Action::Left => "A", Action::Right => "D",
            Action::Crouch => "Shift", Action::Dodge => "Space", Action::Restart => "R", Action::Menu => "Esc",
        }
    }
}

#[derive(Clone)]
pub struct Keybinds { pub keys: [(Action, String); 8] }
impl Default for Keybinds {
    fn default() -> Self {
        let mut i = 0;
        let keys = ACTIONS.map(|a| { i += 1; let _ = i; (a, a.default_key().to_string()) });
        Keybinds { keys }
    }
}
impl Keybinds {
    pub fn key_for(&self, a: Action) -> &str { &self.keys.iter().find(|(x, _)| *x == a).unwrap().1 }
    pub fn rebind(&mut self, a: Action, key: &str) {
        if let Some(slot) = self.keys.iter_mut().find(|(x, _)| *x == a) { slot.1 = key.to_string(); }
    }
}

#[derive(Clone)]
pub struct Settings {
    pub show_cone: bool,   // debug: draw the AI vision cone
    pub master_volume: f32,
    pub difficulty: u8,    // 0 easy .. 2 hard — scales hunter speed, suspicion, and flush radius
    pub hires: bool,       // AAA graphics: supersample + denser geometry
    pub clan_abilities: bool,  // survival mode: give each clan a small passive + ultimate (off = hardcore, hunter feels dominant)
    pub keybinds: Keybinds,
}
impl Default for Settings {
    fn default() -> Self { Settings { show_cone: false, master_volume: 0.7, difficulty: 1, hires: false, clan_abilities: false, keybinds: Keybinds::default() } }
}

/// Map the difficulty setting (0 easy .. 2 hard) to a hunter-aggression multiplier.
pub fn difficulty_mult(d: u8) -> f32 { match d { 0 => 0.82, 2 => 1.22, _ => 1.0 } }

/// The game modes the player can launch from the menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameMode { Rendezvous, Survive, Prowl, Capture }
pub const GAME_MODES: [GameMode; 4] = [GameMode::Rendezvous, GameMode::Survive, GameMode::Prowl, GameMode::Capture];
impl GameMode {
    pub fn name(self) -> &'static str {
        match self { GameMode::Rendezvous => "Rendezvous (6v1)", GameMode::Survive => "Survive (6v1)", GameMode::Prowl => "Prowl", GameMode::Capture => "Pop Island" }
    }
    /// The two 6v1 modes map onto the existing sim; Prowl/Capture are their own sims.
    pub fn is_sim(self) -> bool { matches!(self, GameMode::Rendezvous | GameMode::Survive) }
    pub fn sim_mode(self) -> Mode { if self == GameMode::Survive { Mode::Survive } else { Mode::Rendezvous } }
}

/// What the app should do when the player commits a menu choice.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MenuCmd { None, StartSingle(Kind, Mode), StartMode(GameMode, Kind, Class), Multiplayer, Quit, Close }

pub struct Menu {
    pub open: bool,
    pub screen: Screen,
    pub cursor: usize,
    pub settings: Settings,
    pub sel_hunter: Kind,
    pub sel_mode: Mode,
    pub sel_game: GameMode,
    pub sel_class: Class,
    pub sel_clan: u8,
    pub rebinding: Option<Action>,   // Some(action) while waiting for a key press
}

impl Default for Menu { fn default() -> Self { Self::new() } }

impl Menu {
    pub fn new() -> Self {
        Menu { open: true, screen: Screen::Title, cursor: 0, settings: Settings::default(),
               sel_hunter: Kind::Fox, sel_mode: Mode::Rendezvous, sel_game: GameMode::Rendezvous, sel_class: Class::Cat, sel_clan: 0, rebinding: None }
    }

    /// Menu item labels for the current screen (top-to-bottom).
    pub fn items(&self) -> Vec<String> {
        match self.screen {
            Screen::Title => vec!["Single Player".into(), "Multiplayer".into(), "Settings".into(), "Keybinds".into(), "Quit".into()],
            Screen::Play => vec![
                format!("Game: {}", self.sel_game.name()),
                format!("Hunter: {}", self.sel_hunter.params().name),
                format!("Class: {}", self.sel_class.name()),
                format!("Clan: {} ({})", crate::clan::name(self.sel_clan), crate::clan::specialty(self.sel_clan)),
                "Start".into(), "Back".into(),
            ],
            Screen::Settings => vec![
                format!("Show vision cone: {}", if self.settings.show_cone { "On" } else { "Off" }),
                format!("Master volume: {:.0}%", self.settings.master_volume * 100.0),
                format!("Difficulty: {}", ["Easy", "Normal", "Hard"][self.settings.difficulty as usize]),
                format!("Graphics: {}", if self.settings.hires { "High (AAA)" } else { "Standard" }),
                format!("Survival clan abilities: {}", if self.settings.clan_abilities { "On" } else { "Off (hardcore)" }),
                "Back".into(),
            ],
            Screen::Keybinds => {
                let mut v: Vec<String> = self.settings.keybinds.keys.iter()
                    .map(|(a, k)| format!("{:<14} {}", a.label(), if self.rebinding == Some(*a) { "<press a key>" } else { k }))
                    .collect();
                v.push("Back".into());
                v
            }
        }
    }

    fn len(&self) -> usize { self.items().len() }
    pub fn up(&mut self) { let n = self.len(); self.cursor = (self.cursor + n - 1) % n; }
    pub fn down(&mut self) { let n = self.len(); self.cursor = (self.cursor + 1) % n; }

    fn next_hunter(&mut self, dir: i32) {
        let all = [Kind::Fox, Kind::Dog, Kind::Badger, Kind::Hawk, Kind::Snake, Kind::Shadow];
        let cur = all.iter().position(|k| k.params().name == self.sel_hunter.params().name).unwrap_or(0);
        let n = all.len() as i32;
        self.sel_hunter = all[(((cur as i32 + dir) % n + n) % n) as usize];
    }

    fn next_game(&mut self, dir: i32) {
        let cur = GAME_MODES.iter().position(|&g| g == self.sel_game).unwrap_or(0);
        let n = GAME_MODES.len() as i32;
        self.sel_game = GAME_MODES[(((cur as i32 + dir) % n + n) % n) as usize];
        if self.sel_game.is_sim() { self.sel_mode = self.sel_game.sim_mode(); }
    }

    fn next_class(&mut self, dir: i32) {
        let cur = CLASSES.iter().position(|&c| c == self.sel_class).unwrap_or(0);
        let n = CLASSES.len() as i32;
        self.sel_class = CLASSES[(((cur as i32 + dir) % n + n) % n) as usize];
    }

    /// Left/right adjust the value on the current row (game, hunter, class, volume, etc.).
    pub fn adjust(&mut self, dir: i32) {
        match (self.screen, self.cursor) {
            (Screen::Play, 0) => self.next_game(dir),
            (Screen::Play, 1) => self.next_hunter(dir),
            (Screen::Play, 2) => self.next_class(dir),
            (Screen::Play, 3) => self.sel_clan = (self.sel_clan as i32 + dir).rem_euclid(crate::clan::CLAN_COUNT as i32) as u8,
            (Screen::Settings, 0) => self.settings.show_cone = !self.settings.show_cone,
            (Screen::Settings, 1) => self.settings.master_volume = (self.settings.master_volume + dir as f32 * 0.1).clamp(0.0, 1.0),
            (Screen::Settings, 2) => {
                let d = self.settings.difficulty as i32 + dir;
                self.settings.difficulty = d.clamp(0, 2) as u8;
            }
            (Screen::Settings, 3) => self.settings.hires = !self.settings.hires,
            (Screen::Settings, 4) => self.settings.clan_abilities = !self.settings.clan_abilities,
            _ => {}
        }
    }

    /// Activate the current row. Returns a command for the app to act on.
    pub fn activate(&mut self) -> MenuCmd {
        match self.screen {
            Screen::Title => match self.cursor {
                0 => { self.screen = Screen::Play; self.cursor = 0; MenuCmd::None }
                1 => MenuCmd::Multiplayer,
                2 => { self.screen = Screen::Settings; self.cursor = 0; MenuCmd::None }
                3 => { self.screen = Screen::Keybinds; self.cursor = 0; MenuCmd::None }
                _ => MenuCmd::Quit,
            },
            Screen::Play => match self.cursor {
                4 => {
                    self.open = false;
                    if self.sel_game.is_sim() { MenuCmd::StartSingle(self.sel_hunter, self.sel_game.sim_mode()) }
                    else { MenuCmd::StartMode(self.sel_game, self.sel_hunter, self.sel_class) }
                }
                5 => { self.screen = Screen::Title; self.cursor = 0; MenuCmd::None }
                _ => MenuCmd::None,   // rows 0..3 are adjusted with left/right
            },
            Screen::Settings => { if self.cursor == 5 { self.screen = Screen::Title; self.cursor = 0; } MenuCmd::None }
            Screen::Keybinds => {
                let n = self.settings.keybinds.keys.len();
                if self.cursor == n { self.screen = Screen::Title; self.cursor = 0; }
                else { self.rebinding = Some(self.settings.keybinds.keys[self.cursor].0); }
                MenuCmd::None
            }
        }
    }

    /// Feed the next pressed key while rebinding; returns true if it consumed it.
    pub fn feed_rebind(&mut self, key: &str) -> bool {
        if let Some(a) = self.rebinding.take() { self.settings.keybinds.rebind(a, key); true } else { false }
    }

    pub fn back(&mut self) -> MenuCmd {
        match self.screen {
            Screen::Title => MenuCmd::Close,
            _ => { self.screen = Screen::Title; self.cursor = 0; MenuCmd::None }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigates_and_wraps() {
        let mut m = Menu::new();
        assert_eq!(m.cursor, 0);
        m.up(); assert_eq!(m.cursor, m.len() - 1);  // wraps to bottom
        m.down(); assert_eq!(m.cursor, 0);
    }

    #[test]
    fn single_player_flow_starts_a_match() {
        let mut m = Menu::new();
        assert_eq!(m.activate(), MenuCmd::None);      // Title → Play screen
        assert_eq!(m.screen, Screen::Play);
        m.cursor = 1; m.adjust(1);                     // change hunter
        m.cursor = 4;                                  // Start (a sim game is selected by default)
        match m.activate() { MenuCmd::StartSingle(_, _) => {}, c => panic!("expected StartSingle, got {c:?}") }
        assert!(!m.open);
    }

    #[test]
    fn play_screen_selects_game_and_class() {
        let mut m = Menu::new();
        m.activate();                                  // Title → Play
        assert_eq!(m.sel_game, GameMode::Rendezvous);
        m.cursor = 0; m.adjust(1); assert_eq!(m.sel_game, GameMode::Survive);
        m.adjust(1); assert_eq!(m.sel_game, GameMode::Prowl);
        m.cursor = 2; let c0 = m.sel_class; m.adjust(1); assert_ne!(m.sel_class, c0, "class should cycle");
        m.cursor = 3; let k0 = m.sel_clan; m.adjust(1); assert_ne!(m.sel_clan, k0, "clan should cycle");
        m.cursor = 4;
        match m.activate() {
            MenuCmd::StartMode(GameMode::Prowl, _, cls) => assert_eq!(cls, m.sel_class),
            c => panic!("expected StartMode(Prowl,..), got {c:?}"),
        }
    }

    #[test]
    fn settings_toggle_cone_and_volume() {
        let mut m = Menu::new();
        m.screen = Screen::Settings;
        m.cursor = 0; let before = m.settings.show_cone; m.adjust(1);
        assert_ne!(before, m.settings.show_cone);
        m.cursor = 1; m.adjust(-1); m.adjust(-1);
        assert!(m.settings.master_volume < 0.71);
        m.cursor = 3; let g0 = m.settings.hires; m.adjust(1);
        assert_ne!(g0, m.settings.hires, "the Graphics setting should toggle");
    }

    #[test]
    fn rebinding_a_key() {
        let mut m = Menu::new();
        m.screen = Screen::Keybinds; m.cursor = 5; // Dodge row
        m.activate();
        assert_eq!(m.rebinding, Some(Action::Dodge));
        assert!(m.feed_rebind("Q"));
        assert_eq!(m.settings.keybinds.key_for(Action::Dodge), "Q");
    }
}
