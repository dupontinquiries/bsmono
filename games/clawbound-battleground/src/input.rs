// Rebindable, backend-neutral input — the pattern from monorepo/games
// (skydribble's `input.rs` for logical-key bindings + Dvorak scheme + save
// format, unendlich's `InputCache` for edge latching that never drops a
// press). No winit types in here: `app.rs` converts events at the boundary,
// so this whole module runs under `cargo test`.
//
//   Action    what the player means (Forward, Attack, Ability3, Shop…)
//   Bind      a logical key (the character it types on YOUR layout), a named
//             key, a mouse button, or a controller button
//   Bindings  Action → [Bind]; classic (WASD) and Dvorak schemes; capture-to-
//             rebind; `save_lines` / `load_line` text persistence
//   InputState raw held set + just-pressed latch + mouse deltas
//   Controls  one frame's resolved intent: move vector, look, casts, edges —
//             merged from keyboard/mouse, gamepad (`PadState`) and touch
//             (`TouchPad`), the same way skydribble's `Game::controls` does.

use glam::{Vec2, Vec3};
use std::collections::HashSet;

// ------------------------------------------------------------- actions ----

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Action {
    Forward,
    Back,
    Left,
    Right,
    Attack,
    Ability1,
    Ability2,
    Ability3,
    Ability4,
    Shop,
    Scoreboard,
    Menu,
    ZoomIn,
    ZoomOut,
    /// Snap the camera behind the ancestor.
    Recenter,
    /// Hold to walk toward the crosshair (one-handed / handheld play).
    AutoRun,
    /// An aimed leap: onto whatever rock or log is ahead, else forward.
    Leap,
    /// Toggle the stalk. A plain letter on purpose: a held Ctrl plus any
    /// movement or ability key is some compositor's shortcut, and a handset
    /// has no Ctrl at all.
    Sneak,
}

pub const ACTIONS: [Action; 18] = [
    Action::Forward,
    Action::Back,
    Action::Left,
    Action::Right,
    Action::Attack,
    Action::Ability1,
    Action::Ability2,
    Action::Ability3,
    Action::Ability4,
    Action::Shop,
    Action::Scoreboard,
    Action::Menu,
    Action::ZoomIn,
    Action::ZoomOut,
    Action::Recenter,
    Action::AutoRun,
    Action::Leap,
    Action::Sneak,
];

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Forward => "MOVE FORWARD",
            Action::Back => "MOVE BACK",
            Action::Left => "STRAFE LEFT",
            Action::Right => "STRAFE RIGHT",
            Action::Attack => "ATTACK",
            Action::Ability1 => "ABILITY 1",
            Action::Ability2 => "ABILITY 2",
            Action::Ability3 => "ABILITY 3",
            Action::Ability4 => "ULTIMATE",
            Action::Shop => "SHOP",
            Action::Scoreboard => "SCOREBOARD (HOLD)",
            Action::Menu => "MENU / RELEASE CURSOR",
            Action::ZoomIn => "ZOOM IN",
            Action::ZoomOut => "ZOOM OUT",
            Action::Recenter => "CAMERA BEHIND ME",
            Action::AutoRun => "AUTO-RUN (HOLD)",
            Action::Leap => "LEAP",
            Action::Sneak => "STALK (TOGGLE)",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Action::Forward => "forward",
            Action::Back => "back",
            Action::Left => "left",
            Action::Right => "right",
            Action::Attack => "attack",
            Action::Ability1 => "ability1",
            Action::Ability2 => "ability2",
            Action::Ability3 => "ability3",
            Action::Ability4 => "ability4",
            Action::Shop => "shop",
            Action::Scoreboard => "scoreboard",
            Action::Menu => "menu",
            Action::ZoomIn => "zoom_in",
            Action::ZoomOut => "zoom_out",
            Action::Recenter => "recenter",
            Action::AutoRun => "autorun",
            Action::Leap => "leap",
            Action::Sneak => "sneak",
        }
    }
    pub fn from_id(s: &str) -> Option<Action> {
        ACTIONS.iter().copied().find(|a| a.id() == s)
    }
    /// Menu-driving actions keep working while the cursor is free / a panel
    /// is open; gameplay actions don't.
    pub fn is_meta(self) -> bool {
        matches!(self, Action::Shop | Action::Scoreboard | Action::Menu)
    }
}

// --------------------------------------------------------------- binds ----

/// Keys that are the same on every layout (winit's NamedKey subset we use).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Named {
    Space,
    Shift,
    Control,
    Alt,
    Tab,
    Escape,
    Enter,
    Backspace,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    PageUp,
    PageDown,
    Home,
    End,
    CapsLock,
    F1,
    F2,
    F3,
    F4,
    F5,
}

impl Named {
    pub fn name(self) -> &'static str {
        match self {
            Named::Space => "space",
            Named::Shift => "shift",
            Named::Control => "ctrl",
            Named::Alt => "alt",
            Named::Tab => "tab",
            Named::Escape => "esc",
            Named::Enter => "enter",
            Named::Backspace => "backspace",
            Named::ArrowUp => "up",
            Named::ArrowDown => "down",
            Named::ArrowLeft => "left",
            Named::ArrowRight => "right",
            Named::PageUp => "pageup",
            Named::PageDown => "pagedown",
            Named::Home => "home",
            Named::End => "end",
            Named::CapsLock => "capslock",
            Named::F1 => "f1",
            Named::F2 => "f2",
            Named::F3 => "f3",
            Named::F4 => "f4",
            Named::F5 => "f5",
        }
    }
    pub fn from_name(s: &str) -> Option<Named> {
        [
            Named::Space,
            Named::Shift,
            Named::Control,
            Named::Alt,
            Named::Tab,
            Named::Escape,
            Named::Enter,
            Named::Backspace,
            Named::ArrowUp,
            Named::ArrowDown,
            Named::ArrowLeft,
            Named::ArrowRight,
            Named::PageUp,
            Named::PageDown,
            Named::Home,
            Named::End,
            Named::CapsLock,
            Named::F1,
            Named::F2,
            Named::F3,
            Named::F4,
            Named::F5,
        ]
        .into_iter()
        .find(|n| n.name() == s)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum MouseBtn {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

impl MouseBtn {
    fn name(self) -> String {
        match self {
            MouseBtn::Left => "left".into(),
            MouseBtn::Right => "right".into(),
            MouseBtn::Middle => "middle".into(),
            MouseBtn::Back => "back".into(),
            MouseBtn::Forward => "fwd".into(),
            MouseBtn::Other(n) => format!("btn{n}"),
        }
    }
    fn from_name(s: &str) -> Option<MouseBtn> {
        Some(match s {
            "left" => MouseBtn::Left,
            "right" => MouseBtn::Right,
            "middle" => MouseBtn::Middle,
            "back" => MouseBtn::Back,
            "fwd" => MouseBtn::Forward,
            other => MouseBtn::Other(other.strip_prefix("btn")?.parse().ok()?),
        })
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub enum Bind {
    /// A logical character key, already normalized by `norm_char`.
    Char(String),
    Named(Named),
    Mouse(MouseBtn),
    /// Controller button code (`gamepad::PB_*`).
    Pad(u32),
}

/// Lowercase a logical key so Shift never changes its identity ('E' == 'e'),
/// and fold shifted symbol pairs onto their base key (':' == ';') so a key
/// held through a Shift press/release can't get stuck in the held set.
pub fn norm_char(s: &str) -> String {
    let lower = s.to_lowercase();
    match lower.as_str() {
        ":" => ";",
        "<" => ",",
        ">" => ".",
        "?" => "/",
        "\"" => "'",
        "{" => "[",
        "}" => "]",
        "+" => "=",
        "_" => "-",
        "!" => "1",
        "@" => "2",
        "#" => "3",
        "$" => "4",
        other => other,
    }
    .to_string()
}

pub fn ch(s: &str) -> Bind {
    Bind::Char(norm_char(s))
}

impl Bind {
    pub fn label(&self) -> String {
        match self {
            Bind::Char(s) => s.to_uppercase(),
            Bind::Named(n) => n.name().to_uppercase(),
            Bind::Mouse(b) => format!("MOUSE {}", b.name().to_uppercase()),
            Bind::Pad(c) => format!("PAD {}", crate::gamepad::pad_button_name(*c)),
        }
    }
    pub fn spec(&self) -> String {
        match self {
            Bind::Char(s) => format!("k:{s}"),
            Bind::Named(n) => format!("n:{}", n.name()),
            Bind::Mouse(b) => format!("m:{}", b.name()),
            Bind::Pad(c) => format!("p:{c}"),
        }
    }
    pub fn from_spec(s: &str) -> Option<Bind> {
        let (kind, rest) = s.split_once(':')?;
        Some(match kind {
            "k" => ch(rest),
            "n" => Bind::Named(Named::from_name(rest)?),
            "m" => Bind::Mouse(MouseBtn::from_name(rest)?),
            "p" => Bind::Pad(rest.parse().ok()?),
            _ => return None,
        })
    }
    pub fn is_pad(&self) -> bool {
        matches!(self, Bind::Pad(_))
    }
}

// ------------------------------------------------------------ bindings ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scheme {
    Classic,
    Dvorak,
    Custom,
}

impl Scheme {
    pub fn name(self) -> &'static str {
        match self {
            Scheme::Classic => "QWERTY (WASD)",
            Scheme::Dvorak => "DVORAK (.OEU)",
            Scheme::Custom => "CUSTOM",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Bindings {
    pub map: Vec<(Action, Vec<Bind>)>,
    pub scheme: Scheme,
}

impl Bindings {
    /// WASD as characters — on QWERTY exactly the old scheme; on other
    /// layouts it follows the letters, not the positions.
    pub fn classic() -> Bindings {
        use crate::gamepad::*;
        use Action::*;
        let map = vec![
            (Forward, vec![ch("w"), Bind::Named(Named::ArrowUp)]),
            (Back, vec![ch("s"), Bind::Named(Named::ArrowDown)]),
            (Left, vec![ch("a"), Bind::Named(Named::ArrowLeft)]),
            (Right, vec![ch("d"), Bind::Named(Named::ArrowRight)]),
            (Attack, vec![Bind::Mouse(MouseBtn::Left), Bind::Pad(PB_RT)]),
            (Ability1, vec![ch("1"), ch("q"), Bind::Pad(PB_WEST)]),
            (Ability2, vec![ch("2"), ch("e"), Bind::Pad(PB_NORTH)]),
            (Ability3, vec![ch("3"), ch("r"), Bind::Pad(PB_EAST)]),
            (Ability4, vec![ch("4"), ch("f"), Bind::Pad(PB_RB)]),
            (Shop, vec![ch("b"), Bind::Pad(PB_DUP)]),
            (Scoreboard, vec![Bind::Named(Named::Tab), ch("c"), Bind::Pad(PB_SELECT)]),
            (Menu, vec![Bind::Named(Named::Escape), Bind::Pad(PB_START)]),
            (ZoomIn, vec![Bind::Named(Named::PageUp), Bind::Pad(PB_DRIGHT)]),
            (ZoomOut, vec![Bind::Named(Named::PageDown), Bind::Pad(PB_DLEFT)]),
            (Recenter, vec![ch("x"), Bind::Pad(PB_R3)]),
            (AutoRun, vec![Bind::Named(Named::Shift), Bind::Pad(PB_L3)]),
            // Space and the pad's south button: where a jump lives on every
            // controller, and free in both schemes (Dvorak inherits this).
            (Leap, vec![Bind::Named(Named::Space), Bind::Pad(PB_SOUTH)]),
            (Sneak, vec![ch("z"), Bind::Pad(PB_LB)]),
        ];
        Bindings { map, scheme: Scheme::Classic }
    }

    /// Dvorak: the `.oeu` cluster the monorepo's other games settled on from
    /// playtesting — `.` forward, `e` back, `o` LEFT, `u` RIGHT, abilities on
    /// the rest of the top row (`'` `,` `p` `y`), shop `x`, board `j`,
    /// recenter `q`.
    ///
    /// These are the Dvorak letters at the QWERTY **ESDF** positions, not
    /// WASD's. That is deliberate and is what skydribble and blackstar-rs
    /// both use: it centres the left hand one column right, which keeps the
    /// ability row reachable without moving off the movement keys.
    ///
    /// ## What was wrong before
    ///
    /// This was the `,aoe` cluster (the WASD *positions*) with `a` left and
    /// `e` right. Two things were wrong with it. The cluster disagreed with
    /// every other game in the tree, and more importantly `e` was bound to
    /// **Right** here while skydribble and blackstar-rs bind `e` to **back /
    /// reverse** — so muscle memory carried between games produced a strafe
    /// where a reverse was intended, and the left/right pair read as
    /// inverted relative to the `o`/`u` the other games train you on.
    pub fn dvorak() -> Bindings {
        let mut b = Bindings::classic();
        let set = |b: &mut Bindings, a: Action, v: Vec<Bind>| b.replace_kbm(a, v);
        set(&mut b, Action::Forward, vec![ch("."), Bind::Named(Named::ArrowUp)]);
        set(&mut b, Action::Back, vec![ch("e"), Bind::Named(Named::ArrowDown)]);
        set(&mut b, Action::Left, vec![ch("o"), Bind::Named(Named::ArrowLeft)]);
        set(&mut b, Action::Right, vec![ch("u"), Bind::Named(Named::ArrowRight)]);
        // `.` moved to Forward, so Ability2 takes `,` — the abilities are the
        // top row MINUS whatever movement claimed, kept in left-to-right
        // order so the 1..4 numbering matches the physical order.
        set(&mut b, Action::Ability1, vec![ch("'"), ch("1")]);
        set(&mut b, Action::Ability2, vec![ch(","), ch("2")]);
        set(&mut b, Action::Ability3, vec![ch("p"), ch("3")]);
        set(&mut b, Action::Ability4, vec![ch("y"), ch("4")]);
        set(&mut b, Action::Shop, vec![ch("x")]);
        set(&mut b, Action::Scoreboard, vec![Bind::Named(Named::Tab), ch("j")]);
        set(&mut b, Action::Recenter, vec![ch("q")]);
        b.scheme = Scheme::Dvorak;
        b
    }

    pub fn for_scheme(s: Scheme) -> Bindings {
        match s {
            Scheme::Classic | Scheme::Custom => Bindings::classic(),
            Scheme::Dvorak => Bindings::dvorak(),
        }
    }

    fn slot(&mut self, a: Action) -> &mut Vec<Bind> {
        let i = self.map.iter().position(|(x, _)| *x == a).unwrap_or_else(|| {
            self.map.push((a, Vec::new()));
            self.map.len() - 1
        });
        &mut self.map[i].1
    }

    pub fn binds(&self, a: Action) -> &[Bind] {
        self.map.iter().find(|(x, _)| *x == a).map(|(_, v)| v.as_slice()).unwrap_or(&[])
    }

    /// Replace the keyboard+mouse binds of `a`; pad binds always survive.
    pub fn replace_kbm(&mut self, a: Action, binds: Vec<Bind>) {
        let v = self.slot(a);
        let pads: Vec<Bind> = v.iter().filter(|b| b.is_pad()).cloned().collect();
        let mut out: Vec<Bind> = binds.into_iter().filter(|b| !b.is_pad()).collect();
        out.extend(pads);
        *v = out;
    }

    pub fn replace_pad(&mut self, a: Action, code: u32) {
        let v = self.slot(a);
        v.retain(|b| !b.is_pad());
        v.push(Bind::Pad(code));
    }

    /// Capture semantics: a key/mouse press replaces the kb+mouse binds of
    /// the action; a pad press replaces its pad bind. The same bind is taken
    /// away from every other action first so nothing is double-bound.
    pub fn capture(&mut self, a: Action, b: Bind) {
        for (_, v) in self.map.iter_mut() {
            v.retain(|x| *x != b);
        }
        match b {
            Bind::Pad(c) => self.replace_pad(a, c),
            other => self.replace_kbm(a, vec![other]),
        }
        self.scheme = Scheme::Custom;
    }

    pub fn clear(&mut self, a: Action) {
        self.slot(a).clear();
        self.scheme = Scheme::Custom;
    }

    pub fn labels(&self, a: Action) -> String {
        let v = self.binds(a);
        if v.is_empty() {
            return "-".into();
        }
        v.iter().map(|b| b.label()).collect::<Vec<_>>().join("  ")
    }

    /// Which action (if any) a raw bind drives.
    pub fn action_of(&self, b: &Bind) -> Option<Action> {
        self.map.iter().find(|(_, v)| v.contains(b)).map(|(a, _)| *a)
    }

    // ---- persistence (one `bind <id> <spec,spec>` line per action) ----
    pub fn save_lines(&self, out: &mut String) {
        use std::fmt::Write as _;
        let _ = writeln!(out, "scheme {}", match self.scheme {
            Scheme::Classic => "classic",
            Scheme::Dvorak => "dvorak",
            Scheme::Custom => "custom",
        });
        for (a, v) in &self.map {
            let specs: Vec<String> = v.iter().map(|b| b.spec()).collect();
            let _ = writeln!(out, "bind {} {}", a.id(), specs.join(","));
        }
    }
    pub fn load_line(&mut self, key: &str, rest: &str) -> bool {
        match key {
            "scheme" => {
                self.scheme = match rest.trim() {
                    "dvorak" => Scheme::Dvorak,
                    "custom" => Scheme::Custom,
                    _ => Scheme::Classic,
                };
                true
            }
            "bind" => {
                let (id, specs) = rest.split_once(' ').unwrap_or((rest, ""));
                let Some(a) = Action::from_id(id) else { return false };
                let v: Vec<Bind> = specs.split(',').filter_map(Bind::from_spec).collect();
                *self.slot(a) = v;
                true
            }
            _ => false,
        }
    }
}

// --------------------------------------------------------- raw state ----

/// Backend-neutral event, produced at the winit boundary.
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    Press(Bind),
    Release(Bind),
    /// Raw mouse motion (pixels), from the device — not cursor position.
    MouseDelta(f32, f32),
    Scroll(f32),
    /// All keys released (focus lost): nothing may stay stuck down.
    FocusLost,
}

/// Held set + just-pressed latch (a press and release inside one frame still
/// counts as a press; presses are observed exactly once) + mouse deltas.
#[derive(Default, Debug)]
pub struct InputState {
    held: HashSet<Bind>,
    just_pressed: Vec<Bind>,
    pub mouse_delta: Vec2,
    pub scroll: f32,
}

impl InputState {
    pub fn handle(&mut self, e: InputEvent) {
        match e {
            InputEvent::Press(b) => {
                if !self.held.contains(&b) {
                    self.just_pressed.push(b.clone());
                }
                self.held.insert(b);
            }
            InputEvent::Release(b) => {
                self.held.remove(&b);
            }
            InputEvent::MouseDelta(x, y) => {
                self.mouse_delta += Vec2::new(x, y);
            }
            InputEvent::Scroll(y) => self.scroll += y,
            InputEvent::FocusLost => self.held.clear(),
        }
    }
    pub fn down(&self, b: &Bind) -> bool {
        self.held.contains(b)
    }
    pub fn edge(&self, b: &Bind) -> bool {
        self.just_pressed.contains(b)
    }
    /// First press this frame, for bind capture.
    pub fn any_edge(&self) -> Option<Bind> {
        self.just_pressed.first().cloned()
    }
    /// Consume this frame's one-shot state. Call once per frame, after the
    /// consumers have run.
    pub fn end_frame(&mut self) {
        self.just_pressed.clear();
        self.mouse_delta = Vec2::ZERO;
        self.scroll = 0.0;
    }
    pub fn action_down(&self, b: &Bindings, a: Action) -> bool {
        b.binds(a).iter().any(|x| !x.is_pad() && self.held.contains(x))
    }
    pub fn action_edge(&self, b: &Bindings, a: Action) -> bool {
        b.binds(a).iter().any(|x| !x.is_pad() && self.just_pressed.contains(x))
    }
}

// ----------------------------------------------------------- settings ----

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub mouse_sens: f32,
    pub pad_sens: f32,
    pub invert_y: bool,
    pub touch_enabled: bool,
    pub touch_scale: f32,
    /// What a flick does when the finger lifts. See `crate::glide`.
    ///
    /// `Light` by default rather than `Rigid`: the carry is short enough that
    /// a player who never flicks will not notice it, and long enough that one
    /// who does gets the overlap. Someone who wants the old behaviour exactly
    /// picks Rigid, and someone who wants to flick-and-reach picks Smooth.
    pub touch_glide: crate::glide::Glide,
    /// Hold-to-aim vs tap: abilities fire on press (true) or on release (false).
    pub cast_on_press: bool,
    /// Graphics quality (`--ultra` / CLAWBOUND_QUALITY override it).
    pub quality: crate::quality::Quality,
    /// The ultra preview's fur (CLAWBOUND_FUR overrides it).
    pub fur: crate::render::FurStyle,
    /// Volumes, 0..1: everything, then each bus.
    pub vol_master: f32,
    pub vol_music: f32,
    pub vol_sfx: f32,
    pub vol_voice: f32,
    /// A frame-rate readout in the corner.
    pub show_fps: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { mouse_sens: 1.0, pad_sens: 1.0, invert_y: false, touch_enabled: false, touch_scale: 1.0, cast_on_press: true,
                   touch_glide: crate::glide::Glide::Light,
                   quality: crate::quality::Quality::High, fur: crate::render::FurStyle::Tape,
                   vol_master: 0.8, vol_music: 0.7, vol_sfx: 1.0, vol_voice: 1.0, show_fps: false }
    }
}

impl Settings {
    pub fn save_lines(&self, out: &mut String) {
        use std::fmt::Write as _;
        let _ = writeln!(out, "mouse_sens {:.3}", self.mouse_sens);
        let _ = writeln!(out, "pad_sens {:.3}", self.pad_sens);
        let _ = writeln!(out, "invert_y {}", self.invert_y as u8);
        let _ = writeln!(out, "touch {}", self.touch_enabled as u8);
        let _ = writeln!(out, "touch_scale {:.3}", self.touch_scale);
        // Saved by NAME, not by index. An enum written as 0/1/2 silently
        // changes meaning the day a mode is inserted, and the player's config
        // would then mean something they never chose.
        let _ = writeln!(out, "touch_glide {}", match self.touch_glide {
            crate::glide::Glide::Smooth => "smooth",
            crate::glide::Glide::Light => "light",
            crate::glide::Glide::Rigid => "rigid",
        });
        let _ = writeln!(out, "cast_on_press {}", self.cast_on_press as u8);
        // by name, as touch_glide is
        let _ = writeln!(out, "quality {}", self.quality.key());
        let _ = writeln!(out, "fur {}", match self.fur {
            crate::render::FurStyle::Tape => "tape",
            crate::render::FurStyle::Locks => "locks",
        });
        let _ = writeln!(out, "vol_master {:.3}", self.vol_master);
        let _ = writeln!(out, "vol_music {:.3}", self.vol_music);
        let _ = writeln!(out, "vol_sfx {:.3}", self.vol_sfx);
        let _ = writeln!(out, "vol_voice {:.3}", self.vol_voice);
        let _ = writeln!(out, "show_fps {}", self.show_fps as u8);
    }
    pub fn load_line(&mut self, key: &str, rest: &str) -> bool {
        let f = || rest.trim().parse::<f32>().ok();
        let b = || rest.trim() == "1";
        match key {
            "mouse_sens" => self.mouse_sens = f().unwrap_or(1.0).clamp(0.1, 5.0),
            "pad_sens" => self.pad_sens = f().unwrap_or(1.0).clamp(0.1, 5.0),
            "invert_y" => self.invert_y = b(),
            "touch" => self.touch_enabled = b(),
            "touch_glide" => self.touch_glide = match rest.trim() {
                "smooth" => crate::glide::Glide::Smooth,
                "rigid" => crate::glide::Glide::Rigid,
                // An unknown value is the default, not a parse failure: a
                // config from a newer build should not stop this one loading.
                _ => crate::glide::Glide::Light,
            },
            "touch_scale" => self.touch_scale = f().unwrap_or(1.0).clamp(0.5, 2.0),
            "cast_on_press" => self.cast_on_press = b(),
            "quality" => self.quality = crate::quality::Quality::from_key(rest.trim()).unwrap_or(crate::quality::Quality::High),
            "fur" => self.fur = if rest.trim() == "locks" { crate::render::FurStyle::Locks } else { crate::render::FurStyle::Tape },
            "vol_master" => self.vol_master = f().unwrap_or(0.8).clamp(0.0, 1.0),
            "vol_music" => self.vol_music = f().unwrap_or(0.7).clamp(0.0, 1.0),
            "vol_sfx" => self.vol_sfx = f().unwrap_or(1.0).clamp(0.0, 1.0),
            "vol_voice" => self.vol_voice = f().unwrap_or(1.0).clamp(0.0, 1.0),
            "show_fps" => self.show_fps = b(),
            _ => return false,
        }
        true
    }
}

/// Everything the player configured, as one text file.
#[derive(Clone, Debug)]
pub struct Config {
    pub bindings: Bindings,
    pub settings: Settings,
}

impl Default for Config {
    fn default() -> Self {
        Config { bindings: Bindings::classic(), settings: Settings::default() }
    }
}

impl Config {
    pub fn to_text(&self) -> String {
        let mut s = String::from("# clawbound controls — edit or delete; the game rewrites it on change\n");
        self.bindings.save_lines(&mut s);
        self.settings.save_lines(&mut s);
        s
    }
    pub fn from_text(text: &str) -> Config {
        let mut c = Config::default();
        let mut saw_bind = false;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
            if key == "bind" && !saw_bind {
                // a file with binds fully replaces the defaults
                c.bindings.map.clear();
                saw_bind = true;
            }
            if !c.bindings.load_line(key, rest) {
                c.settings.load_line(key, rest);
            }
        }
        // a scheme line without binds means "the stock scheme"
        if !saw_bind {
            c.bindings = Bindings::for_scheme(c.bindings.scheme);
        }
        c
    }

    /// `$XDG_CONFIG_HOME/clawbound/controls.cfg`, or `~/.config/...`.
    pub fn path() -> Option<std::path::PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
        Some(base.join("clawbound").join("controls.cfg"))
    }
    pub fn load() -> Config {
        match Self::path().and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(t) => Config::from_text(&t),
            None => Config::default(),
        }
    }
    pub fn save(&self) {
        if let Some(p) = Self::path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&p, self.to_text()) {
                eprintln!("[clawbound] could not save controls to {}: {e}", p.display());
            }
        }
    }
}

// ----------------------------------------------------------- controls ----

/// One frame of resolved player intent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Controls {
    /// x = strafe right, y = forward; camera-relative, |v| <= 1
    pub mv: Vec2,
    /// camera turn this frame, radians (x = yaw, y = pitch, already inverted/scaled)
    pub look: Vec2,
    pub zoom: f32,
    pub attack: bool,
    pub cast: [bool; 4],
    pub shop: bool,
    pub scoreboard: bool,
    pub menu: bool,
    pub recenter: bool,
    /// Leap pressed this frame.
    pub leap: bool,
    /// Stalk toggled this frame (keyboard), or held (pad LB).
    pub sneak_toggle: bool,
    pub sneak_held: bool,
    /// Normalized screen point tapped in the open area (walk-to), if any.
    pub tap: Option<Vec2>,
    /// Per-slot aim override, screen-space direction (y up): set only by a
    /// committed touch drag-release on a drag-capable ability button (see
    /// touch.rs). When set for a slot in `cast`, app.rs casts toward this
    /// world direction instead of the camera crosshair. Never set for an
    /// ultimate or a non-directional archetype — `AbilityDef::
    /// quick_aimable()` is what makes a slot drag-capable in the first
    /// place, and ultimates are never quick-aimable.
    pub cast_dir: [Option<Vec2>; 4],
}

/// Merge keyboard/mouse, controller and touch into `Controls`. Pure.
pub fn resolve(
    state: &InputState,
    cfg: &Config,
    pad: &crate::gamepad::PadState,
    touch: Option<&crate::touch::TouchPad>,
    gameplay_active: bool,
) -> Controls {
    let b = &cfg.bindings;
    let s = &cfg.settings;
    let mut c = Controls::default();

    // --- keyboard/mouse (gameplay actions only while the cursor is captured)
    let kb = |a: Action| gameplay_active && state.action_down(b, a);
    let kb_edge = |a: Action| gameplay_active && state.action_edge(b, a);
    let mut mv = Vec2::ZERO;
    if kb(Action::Forward) {
        mv.y += 1.0;
    }
    if kb(Action::Back) {
        mv.y -= 1.0;
    }
    if kb(Action::Right) {
        mv.x += 1.0;
    }
    if kb(Action::Left) {
        mv.x -= 1.0;
    }
    if kb(Action::AutoRun) {
        mv.y = 1.0;
    }
    if gameplay_active {
        let inv = if s.invert_y { -1.0 } else { 1.0 };
        c.look += Vec2::new(-state.mouse_delta.x * 0.0028, state.mouse_delta.y * 0.0022 * inv) * s.mouse_sens;
        c.zoom -= state.scroll;
    }
    c.attack |= kb(Action::Attack);
    for (i, a) in [Action::Ability1, Action::Ability2, Action::Ability3, Action::Ability4].iter().enumerate() {
        c.cast[i] |= kb_edge(*a);
    }
    c.zoom += (kb_edge(Action::ZoomIn) as i32 - kb_edge(Action::ZoomOut) as i32) as f32 * 1.5;
    c.recenter |= kb_edge(Action::Recenter);
    c.leap |= kb_edge(Action::Leap);
    c.sneak_toggle |= kb_edge(Action::Sneak);
    // meta actions work regardless of capture
    c.shop |= state.action_edge(b, Action::Shop);
    c.scoreboard |= state.action_down(b, Action::Scoreboard);
    c.menu |= state.action_edge(b, Action::Menu);

    // --- controller: sticks have fixed roles, buttons go through the bindings
    if pad.connected {
        mv += Vec2::new(pad.stick_x, pad.stick_y);
        let inv = if s.invert_y { -1.0 } else { 1.0 };
        c.look += Vec2::new(-pad.rstick_x * 0.055, pad.rstick_y * 0.04 * inv) * s.pad_sens;
        let pd = |a: Action| b.binds(a).iter().any(|x| matches!(x, Bind::Pad(k) if pad.down(*k)));
        let pe = |a: Action| b.binds(a).iter().any(|x| matches!(x, Bind::Pad(k) if pad.edge(*k)));
        c.attack |= pd(Action::Attack) || pad.throttle > 0.4;
        for (i, a) in [Action::Ability1, Action::Ability2, Action::Ability3, Action::Ability4].iter().enumerate() {
            c.cast[i] |= pe(*a);
        }
        c.zoom += (pe(Action::ZoomIn) as i32 - pe(Action::ZoomOut) as i32) as f32 * 1.5;
        c.recenter |= pe(Action::Recenter);
        c.leap |= pe(Action::Leap);
        c.sneak_held |= pd(Action::Sneak);
        c.shop |= pe(Action::Shop);
        c.scoreboard |= pd(Action::Scoreboard);
        c.menu |= pe(Action::Menu);
    }

    // --- touch: floating stick + buttons + open-area look drag
    if let Some(t) = touch {
        if t.enabled {
            mv += t.stick_vec();
            let inv = if s.invert_y { -1.0 } else { 1.0 };
            c.look += Vec2::new(-t.look.x * 3.2, t.look.y * 2.4 * inv) * s.mouse_sens;
            c.attack |= t.attack_down();
            for i in 0..4 {
                c.cast[i] |= t.ability_edge(i);
            }
            for &(slot, dir) in &t.ability_release {
                if slot < 4 {
                    c.cast[slot] = true;
                    c.cast_dir[slot] = Some(dir);
                }
            }
            c.shop |= t.shop_edge();
            c.leap |= t.leap_edge();
            c.menu |= t.menu_edge();
            c.scoreboard |= t.board_down();
            c.tap = t.tap_pending;
        }
    }

    if mv.length_squared() > 1.0 {
        mv = mv.normalize();
    }
    c.mv = mv;
    c
}

/// Camera-relative movement basis for a third-person yaw: `forward` is the
/// direction the camera looks (horizontal component — matches `cam_dir`'s
/// x/z exactly), `right` is screen-right at that yaw.
///
/// `right` must be `cross(forward, WORLD_UP)`, not `cross(WORLD_UP, forward)`
/// — those two differ only by sign, and the wrong one silently strafes
/// every player backwards while auto-attack, ability aim and everything
/// else (which reads world position, not this vector) keeps working, so
/// nothing else catches it. Verified numerically against
/// `Mat4::look_at_rh` in the test below, not just by hand.
pub fn camera_basis(yaw: f32) -> (Vec3, Vec3) {
    let forward = Vec3::new(yaw.sin(), 0.0, yaw.cos());
    let right = Vec3::new(-forward.z, 0.0, forward.x);
    (forward, right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamepad::PadState;

    #[test]
    fn shift_never_changes_a_keys_identity() {
        assert_eq!(norm_char("E"), "e");
        assert_eq!(norm_char(":"), ";");
        assert_eq!(ch("W"), ch("w"));
    }

    #[test]
    fn dvorak_scheme_moves_on_the_home_cluster_and_keeps_pad_binds() {
        let d = Bindings::dvorak();
        assert!(d.binds(Action::Forward).contains(&ch(".")));
        assert!(d.binds(Action::Attack).iter().any(|b| b.is_pad()), "pad binds survive a scheme swap");
        let c = Bindings::classic();
        assert!(c.binds(Action::Forward).contains(&ch("w")));
    }

    /// The bug this cluster was changed for: `o` and `u` are LEFT and RIGHT,
    /// in that order. The previous scheme bound `e` to Right while
    /// skydribble and blackstar-rs bind `e` to back/reverse, so anyone
    /// moving between games strafed when they meant to back up.
    #[test]
    fn dvorak_strafe_is_o_left_u_right_not_inverted() {
        let d = Bindings::dvorak();
        assert!(d.binds(Action::Left).contains(&ch("o")), "o is LEFT");
        assert!(d.binds(Action::Right).contains(&ch("u")), "u is RIGHT");
        assert!(d.binds(Action::Back).contains(&ch("e")), "e is BACK, matching the other games");
        // The inverted pairing must not come back.
        assert!(!d.binds(Action::Right).contains(&ch("o")));
        assert!(!d.binds(Action::Left).contains(&ch("u")));
        assert!(!d.binds(Action::Right).contains(&ch("e")));
    }

    /// `.` is Forward, so nothing else may claim it — Ability2 used to.
    #[test]
    fn dvorak_movement_keys_are_not_double_bound_to_abilities() {
        let d = Bindings::dvorak();
        for mv in [ch("."), ch("o"), ch("e"), ch("u")] {
            for ab in [Action::Ability1, Action::Ability2, Action::Ability3, Action::Ability4] {
                assert!(!d.binds(ab).contains(&mv), "{ab:?} must not share a movement key");
            }
            assert!(!d.binds(Action::Shop).contains(&mv));
            assert!(!d.binds(Action::Recenter).contains(&mv));
            assert!(!d.binds(Action::Scoreboard).contains(&mv));
        }
    }

    #[test]
    fn capture_rebinds_and_steals_from_other_actions() {
        let mut b = Bindings::classic();
        b.capture(Action::Shop, ch("w"));
        assert_eq!(b.action_of(&ch("w")), Some(Action::Shop));
        assert!(!b.binds(Action::Forward).contains(&ch("w")), "w must leave Forward");
        assert_eq!(b.scheme, Scheme::Custom);
        // pad capture only touches pad binds
        b.capture(Action::Ability1, Bind::Pad(crate::gamepad::PB_SOUTH));
        assert!(b.binds(Action::Ability1).contains(&ch("1")));
        assert!(b.binds(Action::Ability1).contains(&Bind::Pad(crate::gamepad::PB_SOUTH)));
    }

    #[test]
    fn config_round_trips_through_text() {
        let mut c = Config::default();
        c.bindings.capture(Action::Ability4, ch("g"));
        c.settings.invert_y = true;
        c.settings.mouse_sens = 1.75;
        let text = c.to_text();
        let back = Config::from_text(&text);
        assert_eq!(back.bindings.binds(Action::Ability4), c.bindings.binds(Action::Ability4));
        assert_eq!(back.bindings.scheme, Scheme::Custom);
        assert!(back.settings.invert_y);
        assert!((back.settings.mouse_sens - 1.75).abs() < 1e-3);
        // a bare scheme line selects the stock scheme
        let d = Config::from_text("scheme dvorak\n");
        assert!(d.bindings.binds(Action::Forward).contains(&ch(".")));
    }

    #[test]
    fn a_press_and_release_in_one_frame_still_fires_once() {
        let mut st = InputState::default();
        let cfg = Config::default();
        st.handle(InputEvent::Press(ch("1")));
        st.handle(InputEvent::Release(ch("1")));
        let c = resolve(&st, &cfg, &PadState::default(), None, true);
        assert!(c.cast[0], "the edge must not be lost");
        st.end_frame();
        let c2 = resolve(&st, &cfg, &PadState::default(), None, true);
        assert!(!c2.cast[0], "and must not repeat");
    }

    #[test]
    fn held_keys_move_and_meta_keys_work_without_capture() {
        let mut st = InputState::default();
        let cfg = Config::default();
        st.handle(InputEvent::Press(ch("w")));
        st.handle(InputEvent::Press(ch("d")));
        let c = resolve(&st, &cfg, &PadState::default(), None, true);
        assert!((c.mv.length() - 1.0).abs() < 1e-4 && c.mv.x > 0.0 && c.mv.y > 0.0);
        st.handle(InputEvent::Press(Bind::Named(Named::Tab)));
        let free = resolve(&st, &cfg, &PadState::default(), None, false);
        assert_eq!(free.mv, Vec2::ZERO, "gameplay keys are ignored while the cursor is free");
        assert!(free.scoreboard, "but the scoreboard still opens");
        st.handle(InputEvent::FocusLost);
        let gone = resolve(&st, &cfg, &PadState::default(), None, true);
        assert_eq!(gone.mv, Vec2::ZERO, "focus loss releases everything");
    }

    #[test]
    fn camera_right_matches_the_actual_view_matrix() {
        // Ground truth: for a camera at `eye` looking at `target` via
        // Mat4::look_at_rh(eye, target, Y) — exactly what render.rs builds —
        // screen-right is whatever world offset from `target` gives a larger
        // (more positive) clip-space x. `camera_basis`'s `right` must agree,
        // at an arbitrary yaw, not just yaw = 0.
        use glam::Mat4;
        let yaw = 0.7_f32;
        let target = Vec3::new(3.0, 0.0, -2.0);
        let (forward, right) = camera_basis(yaw);
        let eye = target - forward * 8.0;
        let view = Mat4::look_at_rh(eye, target, Vec3::Y);
        let proj = Mat4::perspective_rh(1.0, 1.6, 0.1, 100.0);
        let vp = proj * view;
        let ndc_x = |p: Vec3| {
            let c = vp * p.extend(1.0);
            c.x / c.w
        };
        let base = ndc_x(target);
        let toward_right = ndc_x(target + right);
        let toward_left = ndc_x(target - right);
        assert!(toward_right > base, "camera_basis's right must point toward larger clip-x: {toward_right} vs base {base}");
        assert!(toward_left < base, "and its negation must point the other way: {toward_left} vs base {base}");
    }

    #[test]
    fn camera_basis_is_orthonormal_and_matches_cam_dir_horizontally() {
        for i in 0..12 {
            let yaw = i as f32 * 0.5;
            let (fwd, right) = camera_basis(yaw);
            assert!((fwd.length() - 1.0).abs() < 1e-5);
            assert!((right.length() - 1.0).abs() < 1e-5);
            assert!(fwd.dot(right).abs() < 1e-5, "forward and right must be perpendicular");
            // must match the horizontal component of the render-side camera
            // direction (see render.rs's Camera / app.rs's cam_dir): both are
            // (sin(yaw), *, cos(yaw)), so moving forward walks the way the
            // camera is actually looking, not some other convention.
            assert!((fwd.x - yaw.sin()).abs() < 1e-5 && (fwd.z - yaw.cos()).abs() < 1e-5);
        }
    }

    #[test]
    fn controller_sticks_and_buttons_merge() {
        let st = InputState::default();
        let cfg = Config::default();
        let pad = PadState { connected: true, stick_x: 0.0, stick_y: 0.8, rstick_x: 0.5, rstick_y: 0.0, throttle: 0.9, pressed: 1 << crate::gamepad::PB_WEST, just_pressed: 1 << crate::gamepad::PB_WEST, start_pressed: false };
        let c = resolve(&st, &cfg, &pad, None, true);
        assert!(c.mv.y > 0.7 && c.attack && c.cast[0] && c.look.x < 0.0);
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    /// Everything new in the settings survives a save and a load, by name;
    /// an unknown quality from a newer build loads as High, not an error.
    #[test]
    fn graphics_and_audio_settings_round_trip() {
        let mut a = Settings::default();
        a.quality = crate::quality::Quality::Ultra;
        a.fur = crate::render::FurStyle::Locks;
        a.vol_master = 0.4;
        a.vol_music = 0.2;
        a.vol_sfx = 0.9;
        a.vol_voice = 0.6;
        a.show_fps = true;
        let mut text = String::new();
        a.save_lines(&mut text);
        let mut b = Settings::default();
        for line in text.lines() {
            let (k, rest) = line.split_once(' ').unwrap();
            assert!(b.load_line(k, rest), "unknown key {k}");
        }
        assert_eq!((b.quality, b.fur, b.show_fps), (a.quality, a.fur, a.show_fps));
        assert!((b.vol_master - 0.4).abs() < 1e-3 && (b.vol_music - 0.2).abs() < 1e-3 && (b.vol_voice - 0.6).abs() < 1e-3);
        let mut c = Settings::default();
        c.load_line("quality", "hyper");
        assert_eq!(c.quality, crate::quality::Quality::High);
        c.load_line("vol_sfx", "7");
        assert_eq!(c.vol_sfx, 1.0, "clamped");
    }
}
