//! Menu + settings — the platform-agnostic UI *logic* (screens, navigation,
//! settings, keybinds), kept separate from rendering the way unendlich splits
//! its `menu.rs` logic from the `egui_overlay` renderer. The app drives this with
//! key/pointer input and an egui (or any) front-end reads it to draw.

use crate::ancestor::{Ancestor, ALL as ANCESTORS};
use crate::class::{Class, CLASSES};
use crate::hunter::Kind;
use crate::sim::Mode;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Screen { Title, Play, Draft, Settings, Keybinds }

/// A bindable in-game action.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Action { Up, Down, Left, Right, Crouch, Dodge, Leap, Ability, Restart, Menu }

pub const ACTIONS: [Action; 10] = [
    Action::Up, Action::Down, Action::Left, Action::Right,
    Action::Crouch, Action::Dodge, Action::Leap, Action::Ability, Action::Restart, Action::Menu,
];

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Up => "Move up", Action::Down => "Move down", Action::Left => "Move left",
            Action::Right => "Move right", Action::Crouch => "Hide / Climb", Action::Dodge => "Dodge roll",
            Action::Leap => "Leap", Action::Ability => "Ultimate", Action::Restart => "Restart round", Action::Menu => "Menu",
        }
    }
    fn default_key(self) -> &'static str {
        match self {
            Action::Up => "W", Action::Down => "S", Action::Left => "A", Action::Right => "D",
            Action::Crouch => "Shift", Action::Dodge => "Space", Action::Leap => "F", Action::Ability => "Q",
            Action::Restart => "R", Action::Menu => "Esc",
        }
    }
    /// The stable name in `settings.cfg` (`bind <name> <key>`).
    pub fn name(self) -> &'static str {
        match self {
            Action::Up => "up", Action::Down => "down", Action::Left => "left", Action::Right => "right",
            Action::Crouch => "crouch", Action::Dodge => "dodge", Action::Leap => "leap", Action::Ability => "ability",
            Action::Restart => "restart", Action::Menu => "menu",
        }
    }
    pub fn from_name(name: &str) -> Option<Action> {
        ACTIONS.into_iter().find(|a| a.name() == name)
    }
}

#[derive(Clone)]
pub struct Keybinds { pub keys: [(Action, String); 10] }
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
    /// Ability guides on the ground: the ring, the disc, the dash line.
    pub show_guides: bool,
    /// How a press becomes a cast — see [`crate::castguide::CastMode`].
    pub cast_mode: crate::castguide::CastMode,
    /// Camera distance, 0 (close) to 4 (the whole lane).
    pub camera_zoom: u8,
    pub show_cone: bool,   // debug: draw the AI vision cone
    pub master_volume: f32,
    pub difficulty: u8,    // 0 easy .. 2 hard — scales hunter speed, suspicion, and flush radius
    pub hires: bool,       // AAA graphics: supersample + denser geometry
    pub clan_abilities: bool,  // survival mode: give each clan a small passive + ultimate (off = hardcore, hunter feels dominant)
    pub prowl_fixed_wind: bool,  // Prowl: freeze the wind for a more readable, planned-around map
    pub keybinds: Keybinds,
}
impl Default for Settings {
    fn default() -> Self {
        Settings {
            show_guides: true,
            cast_mode: crate::castguide::CastMode::Quick,
            camera_zoom: 1,
            show_cone: false, master_volume: 0.7, difficulty: 1, hires: false,
            clan_abilities: false, prowl_fixed_wind: false, keybinds: Keybinds::default(),
        }
    }
}

impl Settings {
    /// The file: one `key value` per line, `bind <action> <key>` for the keybinds. Hand-rolled
    /// because the crate has no serde and the web build must not grow one; unknown keys are
    /// kept out of the way (ignored) and a missing key is its default, so a file from an
    /// older or newer build still loads.
    pub fn to_text(&self) -> String {
        let mut s = String::from("# clawbound settings -- written by the game; edit while it is closed\n");
        s.push_str(&format!("show_guides {}\n", self.show_guides as u8));
        s.push_str(&format!("cast_mode {}\n", match self.cast_mode {
            crate::castguide::CastMode::Quick => "quick",
            crate::castguide::CastMode::Normal => "normal",
            crate::castguide::CastMode::Instant => "instant",
        }));
        s.push_str(&format!("camera_zoom {}\n", self.camera_zoom));
        s.push_str(&format!("show_cone {}\n", self.show_cone as u8));
        s.push_str(&format!("master_volume {:.2}\n", self.master_volume));
        s.push_str(&format!("difficulty {}\n", self.difficulty));
        s.push_str(&format!("hires {}\n", self.hires as u8));
        s.push_str(&format!("clan_abilities {}\n", self.clan_abilities as u8));
        s.push_str(&format!("prowl_fixed_wind {}\n", self.prowl_fixed_wind as u8));
        for (a, k) in &self.keybinds.keys {
            s.push_str(&format!("bind {} {}\n", a.name(), k));
        }
        s
    }

    pub fn from_text(text: &str) -> Settings {
        let mut st = Settings::default();
        let flag = |v: &str| matches!(v, "1" | "true" | "on" | "yes");
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
            let rest = rest.trim();
            match key {
                "show_guides" => st.show_guides = flag(rest),
                "cast_mode" => {
                    st.cast_mode = match rest {
                        "quick" => crate::castguide::CastMode::Quick,
                        "normal" => crate::castguide::CastMode::Normal,
                        "instant" => crate::castguide::CastMode::Instant,
                        _ => st.cast_mode,
                    }
                }
                "camera_zoom" => st.camera_zoom = rest.parse::<i32>().map_or(st.camera_zoom, |z| z.clamp(0, 4) as u8),
                "show_cone" => st.show_cone = flag(rest),
                "master_volume" => st.master_volume = rest.parse::<f32>().ok().filter(|v| v.is_finite()).map_or(st.master_volume, |v| v.clamp(0.0, 1.0)),
                "difficulty" => st.difficulty = rest.parse::<i32>().map_or(st.difficulty, |d| d.clamp(0, 2) as u8),
                "hires" => st.hires = flag(rest),
                "clan_abilities" => st.clan_abilities = flag(rest),
                "prowl_fixed_wind" => st.prowl_fixed_wind = flag(rest),
                "bind" => {
                    if let Some((name, k)) = rest.split_once(' ') {
                        if let Some(a) = Action::from_name(name) {
                            let k = k.trim();
                            if !k.is_empty() {
                                st.keybinds.rebind(a, k);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        st
    }

    /// `$XDG_CONFIG_HOME/clawbound/settings.cfg`, or `~/.config/...` -- beside the
    /// battleground's `controls.cfg`. `None` where there is no home (the web build).
    pub fn path() -> Option<std::path::PathBuf> {
        if cfg!(target_arch = "wasm32") {
            return None;
        }
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
        Some(base.join("clawbound").join("settings.cfg"))
    }

    pub fn load() -> Settings {
        match Self::path().and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(t) => Settings::from_text(&t),
            None => Settings::default(),
        }
    }

    pub fn save(&self) {
        if let Some(p) = Self::path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&p, self.to_text()) {
                eprintln!("[clawbound] could not save settings to {}: {e}", p.display());
            }
        }
    }
}

/// Map the difficulty setting (0 easy .. 2 hard) to a hunter-aggression multiplier.
pub fn difficulty_mult(d: u8) -> f32 { match d { 0 => 0.82, 2 => 1.22, _ => 1.0 } }

/// The game modes the player can launch from the menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameMode { Rendezvous, Survive, Prowl, Capture, Battleground }
pub const GAME_MODES: [GameMode; 5] =
    [GameMode::Rendezvous, GameMode::Survive, GameMode::Prowl, GameMode::Capture, GameMode::Battleground];
impl GameMode {
    pub fn name(self) -> &'static str {
        match self {
            GameMode::Rendezvous => "Rendezvous (6v1)",
            GameMode::Survive => "Survive (6v1)",
            GameMode::Prowl => "Prowl",
            GameMode::Capture => "Pop Island",
            GameMode::Battleground => "Battleground of the Ancestors",
        }
    }

    /// Whether this mode picks a **legendary ancestor** rather than a hunter,
    /// class and clan. The battleground's row set is different because its
    /// choices are: which of the thirteen you lead with, and nothing else the
    /// other modes ask about.
    pub fn picks_ancestor(self) -> bool { self == GameMode::Battleground }
    /// The two 6v1 modes map onto the existing sim; Prowl/Capture are their own sims.
    pub fn is_sim(self) -> bool { matches!(self, GameMode::Rendezvous | GameMode::Survive) }
    pub fn sim_mode(self) -> Mode { if self == GameMode::Survive { Mode::Survive } else { Mode::Rendezvous } }
}

/// What the app should do when the player commits a menu choice.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MenuCmd {
    None, StartSingle(Kind, Mode), StartMode(GameMode, Kind, Class), StartTutorial,
    /// The clan-war draft is confirmed: three picks per side, straight into
    /// `battleground::new_draft`. A distinct variant from `StartMode` rather
    /// than shoehorning ancestor picks through `Class`/`Kind` — Battleground
    /// isn't a 6v1 sim and has nothing to say about hunter kind or class.
    /// Lead ONE ancestor into a clan war, picked on the Play screen. Its own
    /// command rather than a `StartMode` because the battleground takes an
    /// ancestor and nothing else — no hunter, no class, no clan.
    StartBattleground(Ancestor),
    /// Open the 5v5 ancestor-select page ([`crate::lobby`]) — the one-press
    /// route into a match, with the clock doing the deciding.
    QuickPlay,
    /// The clan-war draft is confirmed: three picks per side, straight into
    /// `battleground::new_draft`. A distinct variant from `StartMode` rather
    /// than shoehorning ancestor picks through `Class`/`Kind` — Battleground
    /// isn't a 6v1 sim and has nothing to say about hunter kind or class.
    ///
    /// TWO ENTRY POINTS, BOTH KEPT. The c8/c9 split produced a one-ancestor
    /// quick pick (incoming, Play screen) and a full six-pick draft (this side,
    /// `Screen::Draft`, already wired to app.rs) under the SAME variant name
    /// with different signatures. They are different journeys, not two attempts
    /// at one, so the draft took the longer name and both survive.
    StartBattlegroundDraft([Ancestor; 3], [Ancestor; 3]),
    Multiplayer, Quit, Close,
}

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
    /// Index into [`crate::ancestor::ALL`] — the battleground's only choice.
    pub sel_ancestor: usize,
    /// The draft's six pick slots: clan A's three, then clan B's three (rows
    /// 0..3 and 3..6 of the Draft screen). Seeded to the roster's first six
    /// distinct ancestors so a player who never touches left/right still
    /// gets a legal, no-duplicate draft on Start.
    pub draft: [Ancestor; 6],
    pub rebinding: Option<Action>,   // Some(action) while waiting for a key press
    /// A setting or a bind changed since the app last took it (`take_dirty`): the app saves.
    dirty: bool,
}

impl Default for Menu { fn default() -> Self { Self::new() } }

impl Menu {
    pub fn new() -> Self {
        Menu { open: true, screen: Screen::Title, cursor: 0, settings: Settings::default(),
               sel_hunter: Kind::Fox, sel_mode: Mode::Rendezvous, sel_game: GameMode::Rendezvous, sel_class: Class::Cat, sel_clan: 0, sel_ancestor: 0,
               draft: [ANCESTORS[0], ANCESTORS[1], ANCESTORS[2], ANCESTORS[3], ANCESTORS[4], ANCESTORS[5]],
               rebinding: None, dirty: false }
    }

    /// The menu with the player's saved settings (`Settings::load`).
    pub fn with_saved_settings() -> Self {
        let mut m = Menu::new();
        m.settings = Settings::load();
        m
    }

    /// Did a setting or a bind change since the last call? The app saves when it did.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Menu item labels for the current screen (top-to-bottom).
    pub fn items(&self) -> Vec<String> {
        match self.screen {
            // Quick Play sits at the top because it is the button most players
            // want: one press, five a side, a clock, a match. Everything below
            // it is for the player who wants to choose the shape of the evening.
            // The Battleground row below it is this side's full six-pick draft;
            // the two are different journeys into the same mode.
            Screen::Title => vec!["Quick Play (5v5)".into(), "Single Player".into(), "Battleground of the Ancestors".into(), "Multiplayer".into(), "Tutorial".into(), "Settings".into(), "Keybinds".into(), "Quit".into()],
            Screen::Play if self.sel_game.picks_ancestor() => {
                let a = self.ancestor();
                let k = crate::ancestor::kit(a);
                vec![
                    format!("Game: {}", self.sel_game.name()),
                    format!("Ancestor: {} ({})", k.name, crate::draft::species(a)),
                    format!("  {} — {}", k.passive_name, k.passive),
                    format!("  1 {}   2 {}   3 {}", k.abilities[0].name, k.abilities[1].name, k.ult.name),
                    "Start".into(),
                    "Back".into(),
                ]
            }
            Screen::Play => vec![
                format!("Game: {}", self.sel_game.name()),
                format!("Hunter: {}", self.sel_hunter.params().name),
                format!("Class: {}", self.sel_class.name()),
                format!("Clan: {} ({})", crate::clan::name(self.sel_clan), crate::clan::specialty(self.sel_clan)),
                "Start".into(), "Back".into(),
            ],
            Screen::Draft => {
                let mut v: Vec<String> = (0..6).map(|i| {
                    let clan = if i < 3 { 'A' } else { 'B' };
                    format!("Clan {} — Pick {}: {}", clan, i % 3 + 1, self.draft[i].name())
                }).collect();
                v.push("Start".into());
                v.push("Back".into());
                v
            }
            Screen::Settings => vec![
                format!("Ability guides: {}", if self.settings.show_guides { "On" } else { "Off" }),
                format!("Casting: {}", self.settings.cast_mode.name()),
                format!(
                    "Camera: {} ({:.0}m)",
                    ["Closest", "Close", "Middle", "Wide", "Widest"][self.settings.camera_zoom.min(4) as usize],
                    crate::castguide::camera_dist(self.settings.camera_zoom)
                ),
                format!("Show vision cone: {}", if self.settings.show_cone { "On" } else { "Off" }),
                format!("Master volume: {:.0}%", self.settings.master_volume * 100.0),
                format!("Difficulty: {}", ["Easy", "Normal", "Hard"][self.settings.difficulty as usize]),
                format!("Graphics: {}", if self.settings.hires { "High (AAA)" } else { "Standard" }),
                format!("Survival clan abilities: {}", if self.settings.clan_abilities { "On" } else { "Off (hardcore)" }),
                format!("Prowl fixed wind: {}", if self.settings.prowl_fixed_wind { "On (readable)" } else { "Off (dynamic)" }),
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

    /// The currently highlighted ancestor.
    pub fn ancestor(&self) -> crate::ancestor::Ancestor {
        crate::ancestor::ALL[self.sel_ancestor % crate::ancestor::ALL.len()]
    }
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

    /// Advance draft slot `i` to the next ancestor in roster order (wrapping),
    /// skipping any ancestor already picked in one of the OTHER five slots —
    /// a draft where the same legend is on both sides (or twice on one) isn't
    /// a real pick, so duplicates are simply unreachable via adjust rather
    /// than allowed-then-validated.
    fn next_draft_pick(&mut self, i: usize, dir: i32) {
        let n = ANCESTORS.len() as i32;
        let cur = ANCESTORS.iter().position(|&a| a == self.draft[i]).unwrap_or(0) as i32;
        let mut probe = cur;
        for _ in 0..n {
            probe = ((probe + dir) % n + n) % n;
            let candidate = ANCESTORS[probe as usize];
            let taken_elsewhere = self.draft.iter().enumerate().any(|(j, &a)| j != i && a == candidate);
            if !taken_elsewhere {
                self.draft[i] = candidate;
                return;
            }
        }
        // Every ancestor is taken elsewhere (impossible with 13 ancestors and
        // 6 slots, but a stuck picker beats a panic if that ever changes).
    }

    /// Left/right adjust the value on the current row (game, hunter, class, volume, etc.).
    pub fn adjust(&mut self, dir: i32) {
        match (self.screen, self.cursor) {
            (Screen::Play, 0) => self.next_game(dir),
            // the battleground's row 1 is its ancestor; rows 2-3 are that
            // ancestor's own text, and adjusting them means the same thing
            (Screen::Play, 1..=3) if self.sel_game.picks_ancestor() => {
                let n = crate::ancestor::ALL.len() as i32;
                self.sel_ancestor = (self.sel_ancestor as i32 + dir).rem_euclid(n) as usize;
            }
            (Screen::Play, 1) => self.next_hunter(dir),
            (Screen::Play, 2) => self.next_class(dir),
            (Screen::Play, 3) => self.sel_clan = (self.sel_clan as i32 + dir).rem_euclid(crate::clan::CLAN_COUNT as i32) as u8,
            (Screen::Draft, i) if i < 6 => self.next_draft_pick(i, dir),
            (Screen::Settings, _) => {
                self.dirty = true;
                self.adjust_setting(dir);
            }
            _ => {}
        }
    }

    fn adjust_setting(&mut self, dir: i32) {
        match (self.screen, self.cursor) {
            (Screen::Settings, 0) => self.settings.show_guides = !self.settings.show_guides,
            (Screen::Settings, 1) => {
                // one row, cycled either way, because three modes do not deserve
                // three rows and a player flicking left should not have to wrap
                self.settings.cast_mode = if dir >= 0 {
                    self.settings.cast_mode.next()
                } else {
                    self.settings.cast_mode.next().next()
                };
            }
            (Screen::Settings, 2) => {
                let z = self.settings.camera_zoom as i32 + dir;
                self.settings.camera_zoom = z.clamp(0, 4) as u8;
            }
            (Screen::Settings, 3) => self.settings.show_cone = !self.settings.show_cone,
            (Screen::Settings, 4) => self.settings.master_volume = (self.settings.master_volume + dir as f32 * 0.1).clamp(0.0, 1.0),
            (Screen::Settings, 5) => {
                let d = self.settings.difficulty as i32 + dir;
                self.settings.difficulty = d.clamp(0, 2) as u8;
            }
            (Screen::Settings, 6) => self.settings.hires = !self.settings.hires,
            (Screen::Settings, 7) => self.settings.clan_abilities = !self.settings.clan_abilities,
            (Screen::Settings, 8) => self.settings.prowl_fixed_wind = !self.settings.prowl_fixed_wind,
            _ => {}
        }
    }

    /// Activate the current row. Returns a command for the app to act on.
    pub fn activate(&mut self) -> MenuCmd {
        match self.screen {
            Screen::Title => match self.cursor {
                0 => { self.open = false; MenuCmd::QuickPlay }
                1 => { self.screen = Screen::Play; self.cursor = 0; MenuCmd::None }
                2 => { self.screen = Screen::Draft; self.cursor = 0; MenuCmd::None }
                3 => MenuCmd::Multiplayer,
                4 => { self.open = false; MenuCmd::StartTutorial }
                5 => { self.screen = Screen::Settings; self.cursor = 0; MenuCmd::None }
                6 => { self.screen = Screen::Keybinds; self.cursor = 0; MenuCmd::None }
                _ => MenuCmd::Quit,
            },
            Screen::Play => match self.cursor {
                4 => {
                    self.open = false;
                    if self.sel_game.picks_ancestor() {
                        MenuCmd::StartBattleground(self.ancestor())
                    } else if self.sel_game.is_sim() {
                        MenuCmd::StartSingle(self.sel_hunter, self.sel_game.sim_mode())
                    } else {
                        MenuCmd::StartMode(self.sel_game, self.sel_hunter, self.sel_class)
                    }
                }
                5 => { self.screen = Screen::Title; self.cursor = 0; MenuCmd::None }
                _ => MenuCmd::None,   // rows 0..3 are adjusted with left/right
            },
            Screen::Draft => match self.cursor {
                6 => {
                    self.open = false;
                    let a = [self.draft[0], self.draft[1], self.draft[2]];
                    let b = [self.draft[3], self.draft[4], self.draft[5]];
                    MenuCmd::StartBattlegroundDraft(a, b)
                }
                7 => { self.screen = Screen::Title; self.cursor = 0; MenuCmd::None }
                _ => MenuCmd::None,   // rows 0..5 are adjusted with left/right
            },
            Screen::Settings => {
                let back = self.items().len() - 1;
                if self.cursor == back { self.screen = Screen::Title; self.cursor = 0; }
                MenuCmd::None
            }
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
        if let Some(a) = self.rebinding.take() { self.settings.keybinds.rebind(a, key); self.dirty = true; true } else { false }
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

    /// Every setting and every bind round-trips through the file, a missing or unknown key
    /// is harmless, and a bad value falls back to the default rather than out of range.
    #[test]
    fn settings_round_trip_through_the_file_and_a_bad_file_loads_as_defaults() {
        use crate::castguide::CastMode;
        let mut st = Settings::default();
        st.show_guides = false;
        st.cast_mode = CastMode::Instant;
        st.camera_zoom = 3;
        st.show_cone = true;
        st.master_volume = 0.3;
        st.difficulty = 2;
        st.hires = true;
        st.clan_abilities = true;
        st.prowl_fixed_wind = true;
        st.keybinds.rebind(Action::Leap, "G");
        st.keybinds.rebind(Action::Menu, "Backquote");
        let text = st.to_text();
        let back = Settings::from_text(&text);
        assert_eq!(back.show_guides, false);
        assert_eq!(back.cast_mode, CastMode::Instant);
        assert_eq!(back.camera_zoom, 3);
        assert!(back.show_cone);
        assert!((back.master_volume - 0.3).abs() < 1e-3);
        assert_eq!(back.difficulty, 2);
        assert!(back.hires && back.clan_abilities && back.prowl_fixed_wind);
        assert_eq!(back.keybinds.key_for(Action::Leap), "G");
        assert_eq!(back.keybinds.key_for(Action::Menu), "Backquote");
        assert_eq!(back.keybinds.key_for(Action::Up), "W", "an untouched bind is its default");
        assert_eq!(text.lines().filter(|l| l.starts_with("bind ")).count(), ACTIONS.len(), "every action is written");
        // the defaults round-trip too, and the file says what it is
        assert_eq!(Settings::from_text(&Settings::default().to_text()).to_text(), Settings::default().to_text());
        assert!(text.starts_with("# clawbound settings"));
        // a file from somewhere else: unknown keys, out-of-range and junk values, no binds
        let odd = "shiny 1\ncamera_zoom 9\ndifficulty -4\nmaster_volume nope\ncast_mode sideways\nbind nothing X\nbind leap\nhires yes\n";
        let o = Settings::from_text(odd);
        assert_eq!(o.camera_zoom, 4, "clamped");
        assert_eq!(o.difficulty, 0, "clamped");
        assert_eq!(o.master_volume, Settings::default().master_volume, "junk is the default");
        assert_eq!(o.cast_mode, CastMode::Quick);
        assert!(o.hires, "yes is on");
        assert_eq!(o.keybinds.key_for(Action::Leap), "F", "a bind with no key keeps the default");
        assert_eq!(Settings::from_text("").to_text(), Settings::default().to_text());
    }

    /// The app saves on the frame something changed: `adjust` on a settings row and a
    /// rebind set the flag, a Play-screen adjust does not, and taking it clears it.
    #[test]
    fn a_changed_setting_or_bind_is_dirty_once() {
        let mut m = Menu::new();
        assert!(!m.take_dirty());
        m.screen = Screen::Play;
        m.cursor = 0;
        m.adjust(1);
        assert!(!m.take_dirty(), "the Play screen's rows are not settings");
        m.screen = Screen::Settings;
        m.cursor = 4;
        m.adjust(-1);
        assert!(m.take_dirty());
        assert!(!m.take_dirty(), "taken once");
        m.screen = Screen::Keybinds;
        m.rebinding = Some(Action::Dodge);
        assert!(m.feed_rebind("X"));
        assert_eq!(m.settings.keybinds.key_for(Action::Dodge), "X");
        assert!(m.take_dirty());
        assert!(!m.feed_rebind("Y"), "nothing was waiting");
        assert!(!m.take_dirty());
    }

    #[test]
    fn the_new_graphics_rows_adjust_and_back_still_backs_out() {
        use crate::castguide::CastMode;
        let mut m = Menu::new();
        m.screen = Screen::Settings;
        // guides toggle
        m.cursor = 0;
        assert!(m.settings.show_guides);
        m.adjust(1);
        assert!(!m.settings.show_guides);
        // casting cycles both ways from one row
        m.cursor = 1;
        assert_eq!(m.settings.cast_mode, CastMode::Quick);
        m.adjust(1);
        assert_eq!(m.settings.cast_mode, CastMode::Normal);
        m.adjust(-1);
        assert_eq!(m.settings.cast_mode, CastMode::Quick);
        // camera clamps rather than wrapping — nobody wants closest after widest
        m.cursor = 2;
        for _ in 0..9 { m.adjust(1); }
        assert_eq!(m.settings.camera_zoom, 4);
        for _ in 0..9 { m.adjust(-1); }
        assert_eq!(m.settings.camera_zoom, 0);
        // the rows that were already there still work, three lower
        m.cursor = 3;
        m.adjust(1);
        assert!(m.settings.show_cone);
        m.cursor = 6;
        m.adjust(1);
        assert!(m.settings.hires);
        // and Back is wherever the list ends, not a hard-coded 6
        m.cursor = m.items().len() - 1;
        m.activate();
        assert_eq!(m.screen, Screen::Title);
    }

    #[test]
    fn quick_play_is_the_first_thing_on_the_title_screen() {
        let mut m = Menu::new();
        assert_eq!(m.items()[0], "Quick Play (5v5)");
        assert_eq!(m.activate(), MenuCmd::QuickPlay);
        assert!(!m.open, "and it leaves the menu for the select page");
        // The rows below it still go where they went — but there is one more of
        // them than the series assumed: this side's six-pick draft row sits at 2,
        // so everything after it shifts down by one. Both routes into the mode
        // are asserted here on purpose, since a renumbering is exactly how one
        // of them would quietly stop being reachable.
        let mut m = Menu::new();
        m.cursor = 1;
        assert_eq!(m.activate(), MenuCmd::None);
        assert_eq!(m.screen, Screen::Play);
        let mut m = Menu::new();
        m.cursor = 2;
        assert_eq!(m.activate(), MenuCmd::None);
        assert_eq!(m.screen, Screen::Draft, "the Battleground draft row must still open the draft");
        let mut m = Menu::new();
        m.cursor = 3;
        assert_eq!(m.activate(), MenuCmd::Multiplayer);
        let mut m = Menu::new();
        m.cursor = 4;
        assert_eq!(m.activate(), MenuCmd::StartTutorial);
        let mut m = Menu::new();
        m.cursor = 7;
        assert_eq!(m.activate(), MenuCmd::Quit);
    }

    #[test]
    fn the_battleground_is_reachable_and_starts_with_an_ancestor() {
        // The mode existed, was tested, was playable in a terminal — and could
        // not be reached from the menu, which is the only door most players use.
        let mut m = Menu::new();
        assert!(GAME_MODES.contains(&GameMode::Battleground));
        m.screen = Screen::Play;
        m.cursor = 0;
        while m.sel_game != GameMode::Battleground {
            m.adjust(1);
        }
        // its rows are its own: an ancestor, not a hunter and a clan
        let items = m.items();
        assert!(items[1].starts_with("Ancestor:"), "{items:?}");
        assert!(items[2].contains('—'), "the passive is on screen: {items:?}");
        assert!(items[3].contains('1') && items[3].contains('3'), "abilities listed: {items:?}");
        // row 1 walks the roster, and rows 2-3 are the same choice by another name
        m.cursor = 1;
        let first = m.ancestor();
        m.adjust(1);
        assert_ne!(m.ancestor(), first);
        m.cursor = 3;
        m.adjust(-1);
        assert_eq!(m.ancestor(), first, "the ancestor's own rows adjust the ancestor");
        // and Start hands the app that ancestor, not a hunter it would ignore
        m.cursor = 4;
        assert_eq!(m.activate(), MenuCmd::StartBattleground(first));
        assert!(!m.open, "starting a match closes the menu");
    }

    #[test]
    fn the_other_modes_still_pick_a_hunter_and_a_clan() {
        // Adding a mode with different rows is the kind of change that quietly
        // rewires the rows of every other mode.
        let mut m = Menu::new();
        m.screen = Screen::Play;
        m.sel_game = GameMode::Capture;
        let items = m.items();
        assert!(items[1].starts_with("Hunter:"), "{items:?}");
        assert!(items[3].starts_with("Clan:"), "{items:?}");
        m.cursor = 4;
        assert!(matches!(m.activate(), MenuCmd::StartMode(GameMode::Capture, _, _)));
        let mut m = Menu::new();
        m.screen = Screen::Play;
        m.sel_game = GameMode::Survive;
        m.cursor = 4;
        assert!(matches!(m.activate(), MenuCmd::StartSingle(_, _)));
    }

    #[test]
    fn navigates_and_wraps() {
        let mut m = Menu::new();
        assert_eq!(m.cursor, 0);
        m.up(); assert_eq!(m.cursor, m.len() - 1);  // wraps to bottom
        m.down(); assert_eq!(m.cursor, 0);
    }

    #[test]
    fn tutorial_entry_launches_from_the_title_screen() {
        let mut m = Menu::new();
        m.cursor = 4;   // Tutorial row (0=QuickPlay, 1=Single, 2=Battleground, 3=Multiplayer, 4=Tutorial)
        assert_eq!(m.activate(), MenuCmd::StartTutorial);
        assert!(!m.open, "starting the tutorial should close the menu");
    }

    #[test]
    fn single_player_flow_starts_a_match() {
        let mut m = Menu::new();
        m.cursor = 1;                                  // Single Player, under Quick Play
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
        m.cursor = 1;
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
        // rows moved down by three when guides, casting and camera went in at
        // the top; the behaviour they assert has not changed
        let mut m = Menu::new();
        m.screen = Screen::Settings;
        m.cursor = 3; let before = m.settings.show_cone; m.adjust(1);
        assert_ne!(before, m.settings.show_cone);
        m.cursor = 4; m.adjust(-1); m.adjust(-1);
        assert!(m.settings.master_volume < 0.71);
        m.cursor = 6; let g0 = m.settings.hires; m.adjust(1);
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

    #[test]
    fn leap_and_ability_are_independently_bindable() {
        assert_eq!(ACTIONS.len(), 10, "Leap and Ability should be real, separately-bindable actions");
        let mut m = Menu::new();
        m.screen = Screen::Keybinds; m.cursor = 6; // Leap row
        m.activate();
        assert_eq!(m.rebinding, Some(Action::Leap));
        assert!(m.feed_rebind("PageUp"));
        assert_eq!(m.settings.keybinds.key_for(Action::Leap), "PageUp");
        m.cursor = 7; // Ability row
        m.activate();
        assert_eq!(m.rebinding, Some(Action::Ability));
        assert!(m.feed_rebind("PageDown"));
        assert_eq!(m.settings.keybinds.key_for(Action::Ability), "PageDown");
        // rebinding one action doesn't disturb another
        assert_eq!(m.settings.keybinds.key_for(Action::Leap), "PageUp");
    }

    #[test]
    fn battleground_entry_opens_the_draft_screen() {
        let mut m = Menu::new();
        m.cursor = 2;   // Battleground row, one lower since Quick Play took the top
        assert_eq!(m.activate(), MenuCmd::None);
        assert_eq!(m.screen, Screen::Draft);
    }

    #[test]
    fn draft_defaults_are_six_distinct_ancestors() {
        let m = Menu::new();
        let mut seen = m.draft.to_vec();
        seen.sort_by_key(|a| format!("{a:?}"));
        seen.dedup();
        assert_eq!(seen.len(), 6, "the seeded draft must not start with a duplicate pick");
    }

    #[test]
    fn adjusting_a_draft_slot_never_lands_on_an_ancestor_taken_elsewhere() {
        let mut m = Menu::new();
        m.screen = Screen::Draft;
        let clashing = m.draft[1]; // slot 0 must never be adjusted onto this
        m.cursor = 0;
        for _ in 0..ANCESTORS.len() {
            m.adjust(1);
            assert_ne!(m.draft[0], clashing, "slot 0 must never equal slot 1's pick");
            let mut rest = m.draft[1..].to_vec();
            rest.sort_by_key(|a| format!("{a:?}"));
            rest.dedup();
            assert_eq!(rest.len(), 5, "the other five slots stay distinct throughout");
        }
    }

    #[test]
    fn starting_the_draft_sends_three_and_three_to_the_app() {
        let mut m = Menu::new();
        let expected = m.draft;
        m.screen = Screen::Draft;
        m.cursor = 6; // Start row
        match m.activate() {
            MenuCmd::StartBattlegroundDraft(a, b) => {
                assert_eq!(a, [expected[0], expected[1], expected[2]]);
                assert_eq!(b, [expected[3], expected[4], expected[5]]);
            }
            c => panic!("expected StartBattlegroundDraft, got {c:?}"),
        }
        assert!(!m.open, "starting a battleground should close the menu");
    }

    #[test]
    fn back_from_draft_returns_to_title_without_losing_picks() {
        let mut m = Menu::new();
        m.screen = Screen::Draft;
        m.cursor = 0;
        m.adjust(1);
        let picked = m.draft[0];
        m.cursor = 7; // Back row
        assert_eq!(m.activate(), MenuCmd::None);
        assert_eq!(m.screen, Screen::Title);
        assert_eq!(m.draft[0], picked, "leaving the draft screen doesn't reset picks");
    }
}
