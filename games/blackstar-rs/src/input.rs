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
    /// pull the trigger
    Fire,
    /// step the Watch to the next elemental form
    CycleForm,
    /// raid-only: ask for a grapple toward the nearest live AIR node
    Grapple,
    /// FLIGHT MODE ONLY: pitch the nose down independent of throttle.
    /// Ground vehicles keep the existing Forward/Reverse-derived pitch
    /// (deliberate for the Rocket-League-style aerial trick — see
    /// `App::controls`'s doc); a jet needs throttle and pitch to be
    /// independently controllable, which the shared keys can't give it.
    PitchDown,
    /// FLIGHT MODE ONLY: pitch the nose up independent of throttle.
    PitchUp,
    /// FLIGHT MODE ONLY: cycle between VTOL and fast-fly sub-modes (see
    /// `physics::FlightSubMode`). A dedicated key rather than reusing
    /// `Transform` -- a flying chassis can already fold vehicle<->robot
    /// through Transform, and this needs to not conflict with that.
    FlightMode,
    /// TRACKED CHASSIS ONLY: cycle between roll and siege sub-modes (see
    /// `physics::TankSiegeMode`). Same reasoning as `FlightMode`: a
    /// dedicated key rather than a third meaning stacked onto
    /// `Transform`'s existing vehicle<->robot fold.
    SiegeMode,
}

pub const ACTIONS: [Action; 30] = [
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
    Action::Fire,
    Action::CycleForm,
    Action::Grapple,
    Action::PitchDown,
    Action::PitchUp,
    Action::FlightMode,
    Action::SiegeMode,
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
            Action::Fire => "FIRE",
            Action::CycleForm => "WATCH: NEXT FORM",
            Action::Grapple => "RAID: GRAPPLE TO NODE",
            Action::PitchDown => "FLIGHT: NOSE DOWN",
            Action::PitchUp => "FLIGHT: NOSE UP",
            Action::FlightMode => "FLIGHT: CYCLE VTOL/FAST-FLY",
            Action::SiegeMode => "TANK: CYCLE ROLL/SIEGE",
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
            Action::Fire => "fire",
            Action::CycleForm => "cycle_form",
            Action::Grapple => "grapple",
            Action::PitchDown => "pitch_down",
            Action::PitchUp => "pitch_up",
            Action::FlightMode => "flight_mode",
            Action::SiegeMode => "siege_mode",
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
    /// The current default control scheme, replacing the old WASD
    /// layout with the home-row cluster tuned in playtesting, plus a
    /// deliberate move: Jump off the thumb's most-reached keys entirely
    /// (mouse-right + pad south) so Autotarget can own Space outright,
    /// Boost and Fire moving to a trigger-first feel, and 1/2/3 freed
    /// from the legacy car-picker for the hotbar (`hotbar.rs`) to use.
    pub fn default_scheme() -> Bindings {
        use Action::*;
        let mut b = Bindings::classic();
        let set = |b: &mut Bindings, a: Action, v: Vec<Bind>| {
            *b.slot(a) = v;
        };
        set(&mut b, Forward, vec![ch(".")]);
        set(&mut b, Reverse, vec![ch("e")]);
        set(&mut b, SteerL, vec![ch("o")]);
        set(&mut b, SteerR, vec![ch("u")]);
        // ROLL LEFT HAS NO DEFAULT KEY. Left off on purpose in this
        // pass — flag it if that reads as a mistake rather than a
        // choice; RollR keeps its key so air roll is not silently
        // one-sided, and a pad's left bumper/stick still reaches it
        // through the existing pad table untouched.
        set(&mut b, RollL, vec![]);
        set(&mut b, RollR, vec![ch("p")]);
        set(&mut b, Jump, vec![Bind::Mouse(MouseButton::Right), Bind::Pad(crate::gamepad::PB_SOUTH)]);
        set(&mut b, Boost, vec![Bind::Pad(crate::gamepad::PB_EAST)]);
        set(&mut b, Slide, vec![ch("a"), Bind::Pad(crate::gamepad::PB_WEST)]);
        set(&mut b, Transform, vec![named(NamedKey::PageDown), Bind::Pad(crate::gamepad::PB_LB)]);
        set(&mut b, AutoTarget, vec![named(NamedKey::Space), Bind::Pad(crate::gamepad::PB_R3)]);
        // FIRE gets its mouse-left bind back alongside the pad trigger.
        // The prior pass moved Fire to "trigger-first feel" and left
        // keyboard/mouse players with no bind at all, flagged in its own
        // comment as an open gap rather than a final decision ("left as
        // given rather than quietly keeping the old mouse-left default").
        // classic() (below) already pairs Fire with mouse-left + pad
        // trigger; this matches it instead of leaving a second, silently
        // different answer for the scheme players actually start on.
        set(&mut b, Fire, vec![Bind::Mouse(MouseButton::Left), Bind::Pad(crate::gamepad::PB_RT)]);
        set(&mut b, CycleForm, vec![Bind::Mouse(MouseButton::Middle), Bind::Pad(crate::gamepad::PB_DUP)]);
        // CAR1/2/3 ARE UNBOUND BY DEFAULT. 1-9 now drive the hotbar
        // (`hotbar.rs`'s nine slots); the actions still exist for
        // anyone who wants to bind them back in the Binds menu.
        set(&mut b, Car1, vec![]);
        set(&mut b, Car2, vec![]);
        set(&mut b, Car3, vec![]);
        b
    }

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
            // JUMP GETS SPACE BACK. Moving it off was the wrong fix: jump
            // and autotarget want space for the SAME reason — it is the
            // thing your thumb is on — and they never want it at the same
            // moment. chord::resolve decides which one a press means from
            // the form and the chassis, so the key is contextual rather than
            // overloaded. J stays as a second binding.
            (Jump, vec![named(NamedKey::Space), ch("j"), Bind::Mouse(MouseButton::Middle), Bind::Pad(crate::gamepad::PB_SOUTH)]),
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
            // Autotarget's own binding is the SCOPE: right mouse, held for
            // a glance and tapped to latch. Space reaches it too, through
            // the context resolver, on the chassis that have no jump to
            // spend it on.
            (AutoTarget, vec![Bind::Mouse(MouseButton::Right), ch("x"), Bind::Pad(crate::gamepad::PB_R3)]),
            // MOUSE. Shooting belongs under the trigger finger, so Fire is
            // left mouse and the right trigger. Boost and jump pick up right
            // and middle as SECOND bindings rather than replacements — the
            // keyboard ones still work, and on foot the mouse hand is
            // already doing the aiming, so putting thrust and hop under it
            // means a robot fight needs one hand less travel.
            (Fire, vec![Bind::Mouse(MouseButton::Left), Bind::Pad(crate::gamepad::PB_RT)]),
            (CycleForm, vec![ch("z"), Bind::Pad(crate::gamepad::PB_DUP)]),
            // RAID-ONLY. Was a temp raw-key match on 'g'/'G' inside the
            // Assault stage's input handler; "g" is already Transform's
            // second binding here, so this gets its own key rather than
            // fighting for one that's spoken for.
            (Grapple, vec![ch("h")]),
            // FLIGHT-ONLY. `i`/`k` chosen because they're unused by every
            // scheme in this file (checked classic/default_scheme/dvorak,
            // not just this one) -- arrow keys were the obvious first
            // choice but are reserved by the shell/deck
            // (`RESERVED_NAMED`), which `nothing_is_bound_to_a_key_the_
            // shell_handles_itself` below would have caught.
            (PitchDown, vec![ch("k")]),
            (PitchUp, vec![ch("i")]),
            (FlightMode, vec![ch("l")]),
            (SiegeMode, vec![ch("y")]),
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

    /// Is this exact input bound to that action?
    ///
    /// Needed by the context resolver: space has to know it is space
    /// WITHOUT hard-coding the key, or rebinding jump in the Binds menu
    /// silently stops the tap-and-hold from working.
    pub fn is_bound(&self, a: Action, b: &Bind) -> bool {
        self.binds(a).iter().any(|x| x == b)
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

// ------------------------------------------------------------ conflicts ---

/// Two actions bound to the same input.
#[derive(Clone, Debug, PartialEq)]
pub struct Clash {
    pub bind: Bind,
    pub actions: Vec<Action>,
}

/// Every input that more than one action listens for.
///
/// There was no conflict check of any kind: `slot()` hands out a `Vec<Bind>`
/// per action and nothing ever looked across actions, so a rebind could
/// silently shadow another and the only way to find out was to press the key
/// and watch two things happen.
///
/// Binds are compared after `norm_key`, because that is what the dispatcher
/// compares. Without it 'E' and 'e' look like different keys here and the
/// same key at runtime, which would make this report clean and wrong.
pub fn clashes(b: &Bindings) -> Vec<Clash> {
    let mut seen: Vec<(Bind, Vec<Action>)> = Vec::new();
    for (a, binds) in &b.map {
        for bind in binds {
            let key = match bind {
                Bind::Key(k) => Bind::Key(norm_key(k)),
                other => other.clone(),
            };
            match seen.iter_mut().find(|(k, _)| *k == key) {
                Some((_, acts)) => {
                    if !acts.contains(a) {
                        acts.push(*a);
                    }
                }
                None => seen.push((key, vec![*a])),
            }
        }
    }
    seen.into_iter()
        .filter(|(_, acts)| acts.len() > 1)
        .map(|(bind, actions)| Clash { bind, actions })
        .collect()
}

/// Keys the shell consumes before the action table ever sees them.
///
/// The hotbar takes 1-9 in `main.rs` directly and the deck takes these while
/// a console is on stage; `F11` is fullscreen. Binding an action to one of
/// them is an overlap the clash detector cannot see, because only one side
/// of it is in the table.
pub const RESERVED_NAMED: &[NamedKey] = &[
    NamedKey::Escape,
    NamedKey::Tab,
    NamedKey::Enter,
    NamedKey::ArrowUp,
    NamedKey::ArrowDown,
    NamedKey::ArrowLeft,
    NamedKey::ArrowRight,
    NamedKey::F11,
    NamedKey::Backspace,
    NamedKey::Delete,
];

/// Is this bind one the shell will also act on?
pub fn reserved(b: &Bind) -> Option<&'static str> {
    match b {
        Bind::Key(Key::Character(c))
            if c.chars().next().map_or(false, |c| c.is_ascii_digit()) =>
        {
            Some("the hotbar takes 1-9 directly")
        }
        Bind::Key(Key::Named(n)) if RESERVED_NAMED.contains(n) => {
            Some("the shell/deck handles this key itself")
        }
        _ => None,
    }
}

/// Actions with nothing bound at all.
pub fn unbound(b: &Bindings) -> Vec<Action> {
    b.map.iter().filter(|(_, v)| v.is_empty()).map(|(a, _)| *a).collect()
}

#[cfg(test)]
mod clash_tests {
    use super::*;

    /// Print the shipped scheme's conflicts, so the report is the test's
    /// failure message rather than something to go and run separately.
    fn report(b: &Bindings) -> String {
        clashes(b)
            .iter()
            .map(|c| {
                let who: Vec<String> = c.actions.iter().map(|a| format!("{a:?}")).collect();
                format!("{:?} <- {}", c.bind, who.join(", "))
            })
            .collect::<Vec<_>>()
            .join("\n  ")
    }

    #[test]
    fn the_detector_sees_a_conflict_it_is_given() {
        // guard the guard: a detector that reports nothing is indistinguishable
        // from a clean scheme
        let mut b = Bindings::default_scheme();
        let k = ch("z");
        *b.slot(Action::Forward) = vec![k.clone()];
        *b.slot(Action::Reverse) = vec![k];
        let c = clashes(&b);
        assert!(
            c.iter().any(|c| c.actions.contains(&Action::Forward)
                && c.actions.contains(&Action::Reverse)),
            "the detector missed a deliberate clash: {c:?}"
        );
    }

    #[test]
    fn case_does_not_hide_a_clash() {
        // 'E' and 'e' are the same key to the dispatcher, so they must be the
        // same key here or the report is clean and wrong
        let mut b = Bindings::default_scheme();
        *b.slot(Action::Forward) = vec![Bind::Key(Key::Character("E".into()))];
        *b.slot(Action::Reverse) = vec![Bind::Key(Key::Character("e".into()))];
        assert!(!clashes(&b).is_empty(), "case folding is not being applied");
    }

    #[test]
    fn nothing_is_bound_to_a_key_the_shell_handles_itself() {
        // The clash detector only sees the bindings TABLE. Some keys never
        // reach it: the hotbar takes 1-9 directly, the deck takes Escape,
        // Tab, Enter and the arrows, and F11 is fullscreen. An action bound
        // to one of those fires alongside the hardcoded behaviour, which is
        // an overlap no amount of looking at the table would reveal.
        for (a, binds) in &Bindings::default_scheme().map {
            for b in binds {
                match b {
                    Bind::Key(Key::Character(c)) => assert!(
                        !c.chars().next().map_or(false, |c| c.is_ascii_digit()),
                        "{a:?} is bound to {c:?}, which the hotbar takes directly"
                    ),
                    Bind::Key(Key::Named(n)) => assert!(
                        !RESERVED_NAMED.contains(n),
                        "{a:?} is bound to {n:?}, which the shell handles itself"
                    ),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn the_shipped_scheme_has_no_overlapping_binds() {
        let b = Bindings::default_scheme();
        let c = clashes(&b);
        assert!(c.is_empty(), "the default scheme double-binds:\n  {}", report(&b));
    }

    #[test]
    fn the_classic_scheme_has_no_overlapping_binds() {
        let b = Bindings::classic();
        let c = clashes(&b);
        assert!(c.is_empty(), "the classic scheme double-binds:\n  {}", report(&b));
    }
}

// --------------------------------------------------------------- driver ---

/// `--keys` — the live control scheme, and everything wrong with it.
///
/// Reads the bindings out of the SAVE, not the defaults, because the defaults
/// are clean and a player's overlaps are in their own file. There was no way
/// to see this short of pressing every key and watching what happened twice.
pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--keys").unwrap_or(0);
    let want_defaults = argv.get(i + 1).map(String::as_str) == Some("defaults");
    let (_, binds, _) = crate::save::load();
    let b = if want_defaults { Bindings::default_scheme() } else { binds };

    println!(
        "CONTROL SCHEME — {}\n",
        if want_defaults { "shipped defaults" } else { "from your save" }
    );

    let cl = clashes(&b);
    if cl.is_empty() {
        println!("  no two actions share an input.");
    } else {
        println!("  {} OVERLAP(S):", cl.len());
        for c in &cl {
            let who: Vec<String> = c.actions.iter().map(|a| format!("{a:?}")).collect();
            println!("    {:<22} {}", bind_name(&c.bind), who.join(" + "));
        }
    }

    // the other half: keys the shell eats before the table sees them
    let mut stolen = Vec::new();
    for (a, list) in &b.map {
        for bind in list {
            if let Some(why) = reserved(bind) {
                stolen.push((*a, bind.clone(), why));
            }
        }
    }
    if !stolen.is_empty() {
        println!("\n  {} BOUND TO A KEY THE SHELL ALSO USES:", stolen.len());
        for (a, bind, why) in &stolen {
            println!("    {:<22} {a:?}  ({why})", bind_name(bind));
        }
        println!("    these fire TWICE — once as the action, once as the shell's own");
        println!("    behaviour — and no amount of reading the bindings table shows it.");
    }

    let un = unbound(&b);
    if !un.is_empty() {
        let names: Vec<String> = un.iter().map(|a| format!("{a:?}")).collect();
        println!("\n  {} unbound: {}", un.len(), names.join(", "));
        println!("    (RollL is deliberately unbound; the rest are worth a look)");
    }
    println!("\n  --keys defaults  to compare against the shipped scheme");
}

fn bind_name(b: &Bind) -> String {
    match b {
        Bind::Key(Key::Character(c)) => format!("'{c}'"),
        Bind::Key(Key::Named(n)) => format!("{n:?}"),
        Bind::Key(k) => format!("{k:?}"),
        Bind::Mouse(m) => format!("mouse {m:?}"),
        Bind::Pad(p) => format!("pad {}", crate::gamepad::pad_button_name(*p)),
    }
}
