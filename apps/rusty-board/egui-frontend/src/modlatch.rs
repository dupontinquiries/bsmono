//! modlatch — Ctrl and Alt as latches, and the rail swipes that set them.
//!
//! The boards have no Ctrl or Alt key (four rows on a phone have no room),
//! so they live on the layout rail at the left as swipes, next to Tab:
//!
//! * swipe right, level      → Tab (with whatever is latched: Alt+Tab, Ctrl+Tab)
//! * swipe right and up      → Ctrl
//! * swipe right and down    → Alt
//!
//! A modifier swipe cycles Off → Once → Locked → Off, the same cycle as
//! Shift: Once applies to the next key and clears itself; Locked stays until
//! swiped off. Vertical drags are left to the rail's scrolling, and a short
//! drag is a tap on a rail icon, so neither can latch anything by accident.
//!
//! What a key sends while a modifier is latched is decided here too
//! ([`apply`]): a single character becomes a key chord (`Ctrl+c`) through the
//! sink's keysym path, because committed TEXT cannot carry a modifier. A
//! multi-character commit (a swiped word) is sent as text and the Once
//! latches are spent, since "Ctrl+hello" means nothing.

use egui::Vec2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Latch {
    #[default]
    Off,
    Once,
    Locked,
}

impl Latch {
    pub fn cycle(self) -> Latch {
        match self {
            Latch::Off => Latch::Once,
            Latch::Once => Latch::Locked,
            Latch::Locked => Latch::Off,
        }
    }
    pub fn on(self) -> bool {
        self != Latch::Off
    }
    fn spend(&mut self) {
        if *self == Latch::Once {
            *self = Latch::Off;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub ctrl: Latch,
    pub alt: Latch,
}

impl Mods {
    pub fn any(&self) -> bool {
        self.ctrl.on() || self.alt.on()
    }

    /// `"Ctrl+Alt+"`, the prefix `board_compositor`'s keysym parser reads.
    pub fn prefix(&self) -> String {
        let mut s = String::new();
        if self.ctrl.on() {
            s.push_str("Ctrl+");
        }
        if self.alt.on() {
            s.push_str("Alt+");
        }
        s
    }

    /// After a key: Once latches clear, Locked ones stay.
    pub fn spend(&mut self) {
        self.ctrl.spend();
        self.alt.spend();
    }

    pub fn apply_swipe(&mut self, s: RailSwipe) {
        match s {
            RailSwipe::Ctrl => self.ctrl = self.ctrl.cycle(),
            RailSwipe::Alt => self.alt = self.alt.cycle(),
            RailSwipe::Tab | RailSwipe::NextBoard | RailSwipe::PrevBoard => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailSwipe {
    Tab,
    Ctrl,
    Alt,
    /// Steeply up / down: the next / previous board.
    NextBoard,
    PrevBoard,
}

/// Shortest drag that counts, in points. Above egui's click slop (a tap on a
/// rail icon stays a tap) and short enough for a thumb flick off a 40pt rail.
pub const MIN_SWIPE: f32 = 32.0;
/// Within this angle of horizontal: Tab.
pub const LEVEL_DEG: f32 = 25.0;
/// Up to this angle from horizontal: a diagonal (Ctrl/Alt). Steeper is a
/// vertical swipe: next / previous board.
pub const DIAGONAL_DEG: f32 = 70.0;

/// Classify a drag that started on the rail (screen y grows downward).
/// Only rightward drags count: the rail sits at the left screen edge, where a
/// leftward drag has no room and meets the compositor's edge gestures.
pub fn classify(delta: Vec2) -> Option<RailSwipe> {
    if delta.length() < MIN_SWIPE {
        return None;
    }
    // Steep enough in either horizontal direction: a board switch. Checked
    // first, and allowed to lean left, because a thumb swiping "down the
    // rail" at the screen edge drifts inward OR outward.
    let steep = (delta.y.abs()).atan2(delta.x.abs()).to_degrees();
    if steep > DIAGONAL_DEG {
        return Some(if delta.y < 0.0 { RailSwipe::NextBoard } else { RailSwipe::PrevBoard });
    }
    if delta.x <= 0.0 {
        return None;
    }
    let deg = (-delta.y).atan2(delta.x).to_degrees();
    match deg {
        d if d.abs() <= LEVEL_DEG => Some(RailSwipe::Tab),
        d if d > LEVEL_DEG && d <= DIAGONAL_DEG => Some(RailSwipe::Ctrl),
        d if d < -LEVEL_DEG && d >= -DIAGONAL_DEG => Some(RailSwipe::Alt),
        _ => None,
    }
}

/// What to send for a key's output while `mods` are latched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    Text(String),
    Keysym(String),
}

/// A key's text output under the latched modifiers. Spends Once latches.
pub fn apply_text(mods: &mut Mods, text: &str) -> Out {
    if !mods.any() {
        return Out::Text(text.to_string());
    }
    let mut chars = text.chars();
    let out = match (chars.next(), chars.next()) {
        (Some(' '), None) => Out::Keysym(format!("{}space", mods.prefix())),
        (Some(c), None) if !c.is_control() => Out::Keysym(format!("{}{c}", mods.prefix())),
        _ => Out::Text(text.to_string()),
    };
    mods.spend();
    out
}

/// A named key (BackSpace, Return, Left, Tab…) under the latched modifiers.
pub fn apply_keysym(mods: &mut Mods, keysym: &str) -> String {
    let s = format!("{}{keysym}", mods.prefix());
    mods.spend();
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    #[test]
    fn directions() {
        assert_eq!(classify(vec2(60.0, 5.0)), Some(RailSwipe::Tab));
        assert_eq!(classify(vec2(50.0, -50.0)), Some(RailSwipe::Ctrl), "up-right");
        assert_eq!(classify(vec2(50.0, 50.0)), Some(RailSwipe::Alt), "down-right");
        assert_eq!(classify(vec2(5.0, -80.0)), Some(RailSwipe::NextBoard), "up the rail: next board");
        assert_eq!(classify(vec2(-6.0, 80.0)), Some(RailSwipe::PrevBoard), "down, leaning out: previous");
        assert_eq!(classify(vec2(-60.0, 0.0)), None, "leftward is not offered");
        assert_eq!(classify(vec2(10.0, 0.0)), None, "a tap's wobble");
    }

    #[test]
    fn latches_cycle_like_shift_and_once_is_spent() {
        let mut m = Mods::default();
        m.apply_swipe(RailSwipe::Ctrl);
        assert_eq!(m.ctrl, Latch::Once);
        assert_eq!(apply_text(&mut m, "c"), Out::Keysym("Ctrl+c".into()));
        assert_eq!(m.ctrl, Latch::Off, "once means once");
        assert_eq!(apply_text(&mut m, "c"), Out::Text("c".into()));

        m.apply_swipe(RailSwipe::Alt);
        m.apply_swipe(RailSwipe::Alt);
        assert_eq!(m.alt, Latch::Locked);
        assert_eq!(apply_keysym(&mut m, "Tab"), "Alt+Tab");
        assert_eq!(apply_keysym(&mut m, "Tab"), "Alt+Tab", "locked stays");
        m.apply_swipe(RailSwipe::Alt);
        assert_eq!(m.alt, Latch::Off);
    }

    #[test]
    fn both_and_words() {
        let mut m = Mods { ctrl: Latch::Once, alt: Latch::Once };
        assert_eq!(apply_keysym(&mut m, "Delete"), "Ctrl+Alt+Delete");
        let mut m = Mods { ctrl: Latch::Once, alt: Latch::Off };
        assert_eq!(apply_text(&mut m, "hello"), Out::Text("hello".into()), "a word is not a chord");
        assert!(!m.any(), "but it still spends the latch");
        let mut m = Mods { ctrl: Latch::Locked, alt: Latch::Off };
        assert_eq!(apply_text(&mut m, " "), Out::Keysym("Ctrl+space".into()));
    }
}
