//! Rebindable input. Bindings are *logical* keys — the character a key types on
//! YOUR layout — so a Dvorak player binding "e" gets the key labeled e, not the
//! QWERTY position. Every gameplay action can be bound to keys, mouse buttons,
//! or controller buttons, edited in-game (pause → ENTER), and persisted in the
//! save file. Menu navigation itself uses fixed named keys (arrows, Enter,
//! Esc, Tab) which are identical on every layout.

use winit::event::MouseButton;
use winit::keyboard::{Key, NamedKey, SmolStr};

// ------------------------------------------------------------- actions ----
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Forward,
    Reverse,
    SteerL,
    SteerR,
    RollL,
    RollR,
    Jump,
    Boost,
    Slide,
    BallCam,
    Toss,
    BallFront,
    ResetCar,
    Minimap,
    Music,
    VolDown,
    VolUp,
    FlipVis,
    Car1,
    Car2,
    Car3,
    /// vehicle <-> robot, for a blackstar chassis
    Transform,
    /// hand the point-of-interest camera an opponent instead of the ball
    AutoTarget,
}

pub const ACTIONS: [Action; 23] = [
    Action::Forward,
    Action::Reverse,
    Action::SteerL,
    Action::SteerR,
    Action::RollL,
    Action::RollR,
    Action::Jump,
    Action::Boost,
    Action::Slide,
    Action::BallCam,
    Action::Toss,
    Action::BallFront,
    Action::ResetCar,
    Action::Minimap,
    Action::Music,
    Action::VolDown,
    Action::VolUp,
    Action::FlipVis,
    Action::Car1,
    Action::Car2,
    Action::Car3,
    Action::Transform,
    Action::AutoTarget,
];

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Forward => "THROTTLE FWD / NOSE DOWN",
            Action::Reverse => "REVERSE / NOSE UP",
            Action::SteerL => "STEER LEFT / YAW LEFT",
            Action::SteerR => "STEER RIGHT / YAW RIGHT",
            Action::RollL => "AIR ROLL LEFT",
            Action::RollR => "AIR ROLL RIGHT",
            Action::Jump => "JUMP / DODGE",
            Action::Boost => "BOOST",
            Action::Slide => "POWERSLIDE",
                Action::BallCam => "BALL CAM",
            Action::Toss => "TOSS BALL",
            Action::BallFront => "BALL TO FRONT",
            Action::ResetCar => "RESET CAR",
            Action::Minimap => "MINIMAP",
            Action::Music => "MUSIC",
            Action::VolDown => "VOLUME DOWN",
            Action::VolUp => "VOLUME UP",
            Action::FlipVis => "FLIP CAR MODEL",
            Action::Car1 => "CAR: OCTANE",
            Action::Car2 => "CAR: FENNEC",
            Action::Car3 => "CAR: DOMINUS",
            Action::Transform => "TRANSFORM (BOT <-> ROBOT)",
            Action::AutoTarget => "AUTOTARGET",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Action::Forward => "forward",
            Action::Reverse => "reverse",
            Action::SteerL => "steer_l",
            Action::SteerR => "steer_r",
            Action::RollL => "roll_l",
            Action::RollR => "roll_r",
            Action::Jump => "jump",
            Action::Boost => "boost",
            Action::Slide => "slide",
            Action::BallCam => "ball_cam",
            Action::Toss => "toss",
            Action::BallFront => "ball_front",
            Action::ResetCar => "reset_car",
            Action::Minimap => "minimap",
            Action::Music => "music",
            Action::VolDown => "vol_down",
            Action::VolUp => "vol_up",
            Action::FlipVis => "flip_vis",
            Action::Car1 => "car1",
            Action::Car2 => "car2",
            Action::Car3 => "car3",
            Action::Transform => "transform",
            Action::AutoTarget => "autotarget",
        }
    }
    pub fn from_id(s: &str) -> Option<Action> {
        ACTIONS.iter().copied().find(|a| a.id() == s)
    }
}

// --------------------------------------------------------------- binds ----
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Bind {
    Key(Key),         // logical key, Character(...) lowercased or Named(...)
    Mouse(MouseButton),
    Pad(u32),         // controller button code (see gamepad::pad_button_name)
}

/// Lowercase a logical key so Shift never changes identity ('E' == 'e'),
/// and fold shifted symbol pairs onto their base key (':' == ';') so a key
/// held through a Shift press/release can't get stuck in the held set.
pub fn norm_key(k: &Key) -> Key {
    match k {
        Key::Character(s) => {
            let lower = s.to_lowercase();
            let folded = match lower.as_str() {
                ":" => ";",
                "<" => ",",
                ">" => ".",
                "?" => "/",
                "\"" => "'",
                "{" => "[",
                "}" => "]",
                "+" => "=",
                "_" => "-",
                other => other,
            };
            Key::Character(SmolStr::new(folded))
        }
        other => other.clone(),
    }
}

fn ch(s: &str) -> Bind {
    Bind::Key(Key::Character(SmolStr::new(s)))
}
fn named(n: NamedKey) -> Bind {
    Bind::Key(Key::Named(n))
}

fn named_name(n: NamedKey) -> Option<&'static str> {
    Some(match n {
        NamedKey::Space => "space",
        NamedKey::Shift => "shift",
        NamedKey::Control => "ctrl",
        NamedKey::Alt => "alt",
        NamedKey::PageUp => "pageup",
        NamedKey::PageDown => "pagedown",
        NamedKey::ArrowUp => "up",
        NamedKey::ArrowDown => "down",
        NamedKey::ArrowLeft => "left",
        NamedKey::ArrowRight => "right",
        NamedKey::Home => "home",
        NamedKey::End => "end",
        NamedKey::Insert => "insert",
        NamedKey::CapsLock => "capslock",
        _ => return None,
    })
}
fn named_from(s: &str) -> Option<NamedKey> {
    Some(match s {
        "space" => NamedKey::Space,
        "shift" => NamedKey::Shift,
        "ctrl" => NamedKey::Control,
        "alt" => NamedKey::Alt,
        "pageup" => NamedKey::PageUp,
        "pagedown" => NamedKey::PageDown,
        "up" => NamedKey::ArrowUp,
        "down" => NamedKey::ArrowDown,
        "left" => NamedKey::ArrowLeft,
        "right" => NamedKey::ArrowRight,
        "home" => NamedKey::Home,
        "end" => NamedKey::End,
        "insert" => NamedKey::Insert,
        "capslock" => NamedKey::CapsLock,
        _ => return None,
    })
}
fn mouse_name(b: MouseButton) -> String {
    match b {
        MouseButton::Left => "left".into(),
        MouseButton::Right => "right".into(),
        MouseButton::Middle => "middle".into(),
        MouseButton::Back => "back".into(),
        MouseButton::Forward => "fwd".into(),
        MouseButton::Other(n) => format!("btn{n}"),
    }
}
fn mouse_from(s: &str) -> Option<MouseButton> {
    Some(match s {
        "left" => MouseButton::Left,
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        "back" => MouseButton::Back,
        "fwd" => MouseButton::Forward,
        other => MouseButton::Other(other.strip_prefix("btn")?.parse().ok()?),
    })
}

impl Bind {
    /// Short human label for menus.
    pub fn label(&self) -> String {
        match self {
            Bind::Key(Key::Character(s)) => s.to_uppercase(),
            Bind::Key(Key::Named(n)) => named_name(*n).unwrap_or("?").to_uppercase(),
            Bind::Key(_) => "?".into(),
            Bind::Mouse(b) => format!("MOUSE {}", mouse_name(*b).to_uppercase()),
            Bind::Pad(c) => format!("PAD {}", crate::gamepad::pad_button_name(*c)),
        }
    }
    /// Save-file spec.
    pub fn spec(&self) -> Option<String> {
        Some(match self {
            Bind::Key(Key::Character(s)) => format!("k:{s}"),
            Bind::Key(Key::Named(n)) => format!("n:{}", named_name(*n)?),
            Bind::Key(_) => return None,
            Bind::Mouse(b) => format!("m:{}", mouse_name(*b)),
            Bind::Pad(c) => format!("p:{c}"),
        })
    }
    pub fn from_spec(s: &str) -> Option<Bind> {
        let (kind, rest) = s.split_once(':')?;
        Some(match kind {
            "k" => ch(rest),
            "n" => named(named_from(rest)?),
            "m" => Bind::Mouse(mouse_from(rest)?),
            "p" => Bind::Pad(rest.parse().ok()?),
            _ => return None,
        })
    }
    pub fn is_pad(&self) -> bool {
        matches!(self, Bind::Pad(_))
    }
}

// ------------------------------------------------------------ bindings ----
pub struct Bindings {
    pub map: Vec<(Action, Vec<Bind>)>,
}

impl Bindings {
    /// Classic layout: WASD as characters (identical to the original scheme on
    /// QWERTY; on other layouts it follows the letters, not the positions).
    pub fn classic() -> Bindings {
        use Action::*;
        let m = vec![
            (Forward, vec![ch("w")]),
            (Reverse, vec![ch("s")]),
            (SteerL, vec![ch("a")]),
            (SteerR, vec![ch("d")]),
            (RollL, vec![ch("q")]),
            (RollR, vec![ch("e")]),
            (Jump, vec![named(NamedKey::Space), Bind::Pad(crate::gamepad::PB_SOUTH)]),
            // BOOST LOSES SHIFT. Transform was asked for on Shift, and two
            // actions on one key is worse than moving one of them. V is the
            // boost key the Dvorak scheme already uses, so it is not a new
            // idea to anyone, and both pad bindings are untouched — on a
            // controller nothing about boost changes at all.
            (Boost, vec![ch("v"), Bind::Pad(crate::gamepad::PB_EAST), Bind::Pad(crate::gamepad::PB_RB)]),
            (Slide, vec![named(NamedKey::Control), Bind::Pad(crate::gamepad::PB_WEST)]),
            (BallCam, vec![ch("c"), Bind::Pad(crate::gamepad::PB_NORTH)]),
            (Toss, vec![ch("t")]),
            (BallFront, vec![ch("b")]),
            (ResetCar, vec![ch("r"), Bind::Pad(crate::gamepad::PB_SELECT)]),
            (Minimap, vec![ch("m")]),
            (Music, vec![ch("n")]),
            (VolDown, vec![ch("[")]),
            (VolUp, vec![ch("]")]),
            (FlipVis, vec![ch("f")]),
            (Car1, vec![ch("1")]),
            (Car2, vec![ch("2")]),
            (Car3, vec![ch("3")]),
            // SHIFT, with G kept as the second binding for anyone already
            // used to it, and L-bumper on a pad.
            (Transform, vec![named(NamedKey::Shift), ch("g"), Bind::Pad(crate::gamepad::PB_LB)]),
            // X, and the right stick click — where a lock-on lives on a pad
            (AutoTarget, vec![ch("x"), Bind::Pad(crate::gamepad::PB_R3)]),
        ];
        Bindings { map: m }
    }

    /// The Dvorak scheme from playtesting: home-row cluster o/e/u with '.'
    /// above for reverse, ',' and 'p' flanking for air roll, ';' powerslide
    /// (bottom-left on Dvorak), right-hand h/t/n-row utilities.
    pub fn dvorak() -> Bindings {
        // The requested Dvorak scheme, verbatim:
        //   . fwd+nose-down   E rev+nose-up   O/U steer   ,/P air roll
        //   Z jump/dodge   V boost   A powerslide   SPACE ball cam
        let mut b = Bindings::classic();
        let set = |b: &mut Bindings, a: Action, keep: bool, v: Vec<Bind>| {
            if !keep {
                *b.slot(a) = v;
            }
        };
        set(&mut b, Action::Forward, false, vec![ch(".")]);
        set(&mut b, Action::Reverse, false, vec![ch("e")]);
        set(&mut b, Action::SteerL, false, vec![ch("o")]);
        set(&mut b, Action::SteerR, false, vec![ch("u")]);
        set(&mut b, Action::RollL, false, vec![ch(",")]);
        set(&mut b, Action::RollR, false, vec![ch("p")]);
        set(&mut b, Action::Jump, false, vec![ch("z"), Bind::Pad(crate::gamepad::PB_SOUTH)]);
        set(&mut b, Action::Boost, false, vec![ch("v"), Bind::Pad(crate::gamepad::PB_EAST)]);
        set(&mut b, Action::Slide, false, vec![ch("a"), Bind::Pad(crate::gamepad::PB_WEST)]);
        set(&mut b, Action::BallCam, false, vec![Bind::Key(Key::Named(NamedKey::Space)), Bind::Pad(crate::gamepad::PB_NORTH)]);
        b
    }


    fn slot(&mut self, a: Action) -> &mut Vec<Bind> {
        let i = self.map.iter().position(|(x, _)| *x == a).unwrap_or_else(|| {
            self.map.push((a, Vec::new()));
            self.map.len() - 1
        });
        &mut self.map[i].1
    }

    pub fn binds(&self, a: Action) -> &[Bind] {
        self.map
            .iter()
            .find(|(x, _)| *x == a)
            .map(|(_, v)| v.as_slice())
            .unwrap_or(&[])
    }

    /// Replace the keyboard+mouse binds of `a` (keeping pad binds), or if
    /// `keep_pad` is false replace only kb/mouse anyway — pad binds always
    /// survive a keyboard capture and vice versa.
    pub fn replace_kbm(&mut self, a: Action, mut binds: Vec<Bind>, _keep_pad: bool) {
        let v = self.slot(a);
        v.retain(|b| b.is_pad());
        binds.retain(|b| !b.is_pad());
        let mut pads: Vec<Bind> = v.clone();
        let mut out = binds;
        out.append(&mut pads);
        *v = out;
    }

    pub fn replace_pad(&mut self, a: Action, code: u32) {
        let v = self.slot(a);
        v.retain(|b| !b.is_pad());
        v.push(Bind::Pad(code));
    }

    /// Capture semantics: a key/mouse press replaces kb+mouse binds of the
    /// action; a pad press replaces its pad bind. The same bind is removed
    /// from every other action first so nothing is double-bound.
    pub fn capture(&mut self, a: Action, b: Bind) {
        for (_, v) in self.map.iter_mut() {
            v.retain(|x| *x != b);
        }
        match b {
            Bind::Pad(c) => self.replace_pad(a, c),
            other => self.replace_kbm(a, vec![other], true),
        }
    }

    pub fn clear(&mut self, a: Action) {
        self.slot(a).clear();
    }

    pub fn labels(&self, a: Action) -> String {
        let v = self.binds(a);
        if v.is_empty() {
            return "-".into();
        }
        v.iter().map(|b| b.label()).collect::<Vec<_>>().join("  ")
    }

    // ---- save file ----
    pub fn save_lines(&self, out: &mut String) {
        use std::fmt::Write as _;
        for (a, v) in &self.map {
            let specs: Vec<String> = v.iter().filter_map(|b| b.spec()).collect();
            let _ = writeln!(out, "bind {} {}", a.id(), specs.join(","));
        }
    }
    pub fn load_line(&mut self, action_id: &str, specs: &str) {
        let Some(a) = Action::from_id(action_id) else { return };
        let v: Vec<Bind> = specs.split(',').filter_map(Bind::from_spec).collect();
        *self.slot(a) = v;
    }
}
