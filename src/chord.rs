//! ONE KEY, MEANING DECIDED BY CONTEXT.
//!
//! Jump and autotarget both want space, and moving one of them off is the
//! wrong fix — it was the fix I reached for last round and it made jump
//! worse on a keyboard for no gain. They want space for the same reason: it
//! is the thing your thumb is already on, and it is the thing you press when
//! something is about to happen.
//!
//! But they never want it at the same time. A robot on its feet jumps. A jet
//! does not jump — it has no legs out — and what its pilot wants under that
//! thumb is the next target. So the key is not overloaded; it is one key
//! whose meaning is fully determined by what you are currently driving, and
//! there is a test that no context resolves to two things.
//!
//! TAP AND HOLD split the cases that genuinely want both. A racer does jump
//! — that is half of what a racer does — so on a racer a tap is always a
//! jump and holding reaches the weapons instead. A tank is the same, because
//! a tank is a racer that shoots. Nothing has to be learned twice.
//!
//! And the mouse does the deliberate half. Right button HELD is a scope: you
//! zoom, you acquire, and it lets go when you do. Right button TAPPED
//! latches that lock so you can put both hands back on the flying, and taps
//! again to drop it. Hold is for a glance, latch is for a commitment, and
//! the same button does both because they are the same intent at different
//! durations.

/// Which half of a transforming chassis is out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Form {
    Robot,
    Vehicle,
}

/// What the vehicle half IS. Categories the roster already sorts by.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    /// wheels, jumps, wants weapons on hold
    Racer,
    /// treads, same hybrid as a racer with a turret on the end of it
    Tank,
    /// no wheels to jump on; several things worth targeting
    Hoverer,
    /// no jump at all; the thumb belongs to target selection
    Flyer,
    Other,
}

/// How long the key was down.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Press {
    Tap,
    Hold,
}

/// A press becomes a hold after this long. Short enough that holding feels
/// deliberate, long enough that a hurried jump is never read as one.
pub const HOLD_SEC: f32 = 0.22;

/// What the player actually asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Intent {
    Jump,
    /// step to the next target
    CycleTarget,
    /// hold a target while the key is down
    HoldTarget,
    /// open the weapons layer — fire group select, form cycle
    Weapons,
    /// countermeasures
    Flare,
    None,
}

impl Intent {
    pub fn label(self) -> &'static str {
        match self {
            Intent::Jump => "JUMP",
            Intent::CycleTarget => "NEXT TARGET",
            Intent::HoldTarget => "HOLD TARGET",
            Intent::Weapons => "WEAPONS",
            Intent::Flare => "FLARES",
            Intent::None => "",
        }
    }
    /// Does this one persist while the key is down?
    pub fn is_sustained(self) -> bool {
        matches!(self, Intent::HoldTarget | Intent::Weapons)
    }
}

/// THE TABLE. Everything about what space does lives here, in one place that
/// can be read top to bottom and argued with.
pub fn resolve(form: Form, class: Class, press: Press) -> Intent {
    match (form, press) {
        // ON ITS FEET IT JUMPS, whatever it turns into. A bot is a bot.
        (Form::Robot, Press::Tap) => Intent::Jump,
        // and holding still reaches a target, because a robot fights too
        (Form::Robot, Press::Hold) => Intent::HoldTarget,

        (Form::Vehicle, p) => match (class, p) {
            // a racer jumps — that is half of what a racer is — so the tap
            // is never taken away from it, and the hold does the extra
            (Class::Racer, Press::Tap) | (Class::Tank, Press::Tap) => Intent::Jump,
            (Class::Racer, Press::Hold) | (Class::Tank, Press::Hold) => Intent::Weapons,

            // a jet has no legs out. The thumb belongs to target selection,
            // which is what a pilot actually needs it for.
            (Class::Flyer, Press::Tap) => Intent::CycleTarget,
            (Class::Flyer, Press::Hold) => Intent::HoldTarget,

            // a hovercar has nothing to jump off either, and enough going on
            // that countermeasures earn the hold
            (Class::Hoverer, Press::Tap) => Intent::CycleTarget,
            (Class::Hoverer, Press::Hold) => Intent::Flare,

            (Class::Other, Press::Tap) => Intent::Jump,
            (Class::Other, Press::Hold) => Intent::HoldTarget,
        },
    }
}

/// Turns raw key-down/up into taps and holds.
///
/// The hold has to fire WHILE THE KEY IS STILL DOWN — waiting for the
/// release to decide would mean a hold-to-target that only engages after you
/// let go, which is the opposite of holding.
#[derive(Default)]
pub struct Chord {
    down: bool,
    t: f32,
    fired_hold: bool,
}

impl Chord {
    pub fn new() -> Chord {
        Chord::default()
    }

    pub fn press(&mut self) {
        self.down = true;
        self.t = 0.0;
        self.fired_hold = false;
    }

    /// Returns Tap if the key came up before the hold threshold.
    pub fn release(&mut self) -> Option<Press> {
        let was = self.down;
        self.down = false;
        (was && !self.fired_hold).then_some(Press::Tap)
    }

    /// Returns Hold once, at the moment the threshold is crossed.
    pub fn update(&mut self, dt: f32) -> Option<Press> {
        if !self.down || self.fired_hold {
            return None;
        }
        self.t += dt;
        if self.t >= HOLD_SEC {
            self.fired_hold = true;
            return Some(Press::Hold);
        }
        None
    }

    /// True while a hold is in force, so a sustained intent knows to stop.
    pub fn holding(&self) -> bool {
        self.down && self.fired_hold
    }
    pub fn down(&self) -> bool {
        self.down
    }
}

/// THE SCOPE, on the right mouse button.
///
/// Held: zoom and acquire while the button is down, release and it goes.
/// Tapped: latch, so you can put both hands back on the flying. Tap again to
/// drop it. One button, two commitments, told apart by how long you meant it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Scope {
    #[default]
    Off,
    /// zooming while held
    Held,
    /// locked on and left there
    Latched,
}

#[derive(Default)]
pub struct Sight {
    pub scope: Scope,
    chord: Chord,
}

impl Sight {
    pub fn new() -> Sight {
        Sight::default()
    }

    pub fn press(&mut self) {
        self.chord.press();
    }

    /// A release either ends a glance or toggles a latch.
    pub fn release(&mut self) -> Scope {
        match self.chord.release() {
            // a tap: latch, or unlatch if we already were
            Some(Press::Tap) => {
                self.scope = if self.scope == Scope::Latched { Scope::Off } else { Scope::Latched };
            }
            // a hold ended: the glance is over, unless it was latched before
            _ => {
                if self.scope == Scope::Held {
                    self.scope = Scope::Off;
                }
            }
        }
        self.scope
    }

    pub fn update(&mut self, dt: f32) -> Scope {
        if self.chord.update(dt).is_some() && self.scope != Scope::Latched {
            self.scope = Scope::Held;
        }
        self.scope
    }

    /// Is a target being held right now, by either means?
    pub fn active(&self) -> bool {
        self.scope != Scope::Off
    }
    /// Zoom factor for the camera: a scope narrows the view.
    pub fn zoom(&self) -> f32 {
        match self.scope {
            Scope::Off => 1.0,
            Scope::Held => 0.62,
            // a latched lock does not keep you zoomed in — you latched it so
            // you could go back to flying
            Scope::Latched => 0.88,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_CLASS: [Class; 5] =
        [Class::Racer, Class::Tank, Class::Hoverer, Class::Flyer, Class::Other];

    #[test]
    fn a_bot_on_its_feet_always_jumps() {
        // whatever it folded out of
        for c in ALL_CLASS {
            assert_eq!(resolve(Form::Robot, c, Press::Tap), Intent::Jump, "{c:?}");
        }
    }

    #[test]
    fn nothing_that_cannot_jump_is_offered_a_jump() {
        // the actual conflict: a jet has no legs out, so the thumb is free
        for c in [Class::Flyer, Class::Hoverer] {
            for p in [Press::Tap, Press::Hold] {
                assert_ne!(resolve(Form::Vehicle, c, p), Intent::Jump, "{c:?} {p:?}");
            }
        }
        // and a jet's tap is target selection, which is what it wants it for
        assert_eq!(resolve(Form::Vehicle, Class::Flyer, Press::Tap), Intent::CycleTarget);
    }

    #[test]
    fn a_racer_never_loses_its_tap() {
        // half of what a racer does is jump; taking that away to make room
        // for a menu would be the worse trade
        for c in [Class::Racer, Class::Tank] {
            assert_eq!(resolve(Form::Vehicle, c, Press::Tap), Intent::Jump, "{c:?}");
            assert_eq!(resolve(Form::Vehicle, c, Press::Hold), Intent::Weapons, "{c:?}");
        }
    }

    #[test]
    fn every_context_resolves_to_exactly_one_thing() {
        // the property that makes this NOT an overloaded key: for any state
        // the player can be in, there is one answer, and it is never None
        for f in [Form::Robot, Form::Vehicle] {
            for c in ALL_CLASS {
                for p in [Press::Tap, Press::Hold] {
                    assert_ne!(resolve(f, c, p), Intent::None, "{f:?} {c:?} {p:?} has no meaning");
                }
            }
        }
    }

    #[test]
    fn a_hold_fires_while_the_key_is_still_down() {
        // waiting for the release would mean hold-to-target engages after
        // you let go, which is the opposite of holding
        let mut ch = Chord::new();
        ch.press();
        let mut fired = None;
        for _ in 0..60 {
            if let Some(p) = ch.update(1.0 / 60.0) {
                fired = Some(p);
                break;
            }
        }
        assert_eq!(fired, Some(Press::Hold));
        assert!(ch.down() && ch.holding(), "the hold ended before the key came up");
        // and the release is not ALSO a tap
        assert_eq!(ch.release(), None, "one press produced both a hold and a tap");
    }

    #[test]
    fn a_quick_press_is_a_tap_and_never_a_hold() {
        let mut ch = Chord::new();
        ch.press();
        ch.update(HOLD_SEC * 0.5);
        assert!(!ch.holding());
        assert_eq!(ch.release(), Some(Press::Tap));
        // and nothing fires afterwards
        assert_eq!(ch.update(1.0), None);
    }

    #[test]
    fn the_hold_threshold_is_longer_than_a_hurried_jump() {
        // if this gets too short, a player mashing jump starts opening the
        // weapons layer, which is the failure that would kill the whole idea
        assert!(HOLD_SEC >= 0.18, "a hurried jump will register as a hold");
        assert!(HOLD_SEC <= 0.35, "holding feels unresponsive");
    }

    #[test]
    fn the_scope_glances_when_held_and_commits_when_tapped() {
        let mut s = Sight::new();
        // held: zoomed while down, gone on release
        s.press();
        for _ in 0..60 {
            s.update(1.0 / 60.0);
        }
        assert_eq!(s.scope, Scope::Held);
        assert!(s.zoom() < 0.7, "a scope that does not zoom is not a scope");
        s.release();
        assert_eq!(s.scope, Scope::Off, "the glance outlived the button");

        // tapped: latched, and stays
        s.press();
        s.update(HOLD_SEC * 0.4);
        s.release();
        assert_eq!(s.scope, Scope::Latched);
        assert!(s.active());
        // and it does NOT keep you zoomed — you latched it to go back to
        // flying
        assert!(s.zoom() > 0.8, "a latched lock kept the tunnel vision");
    }

    #[test]
    fn tapping_again_drops_a_latched_lock() {
        let mut s = Sight::new();
        s.press();
        s.release();
        assert_eq!(s.scope, Scope::Latched);
        s.press();
        s.release();
        assert_eq!(s.scope, Scope::Off, "a latched lock could not be cancelled");
    }

    #[test]
    fn holding_over_a_latched_lock_does_not_cancel_it_by_accident() {
        // you latched, then later held to glance at something else; letting
        // go should leave the latch alone rather than silently dropping it
        let mut s = Sight::new();
        s.press();
        s.release();
        assert_eq!(s.scope, Scope::Latched);
        s.press();
        for _ in 0..60 {
            s.update(1.0 / 60.0);
        }
        s.release();
        assert_eq!(s.scope, Scope::Latched, "a glance dropped the commitment");
    }
}
