//! sim::provocation — context-sensitive threat response for fauna.
//!
//! From `design_library/04/fauna_behaviors`: hostile animals aren't monsters,
//! they're responding to stimulus. A bear is safe until you're between it and
//! its cubs; a moose is safe until you're too close in rut. `disposition`
//! already turns a genome into an aggression *baseline*; this adds the missing
//! **provocation** layer — a NEUTRAL animal ignores you until a specific trigger
//! fires, then commits, and stays committed for a hold time even after the
//! trigger clears (a provoked bear doesn't instantly calm). Passive animals flee
//! and never attack; hostile ones engage on detection. Pure, deterministic,
//! std-only; the caller supplies the trigger booleans from the world.

/// An animal's baseline temperament (the doc's PASSIVE / NEUTRAL / HOSTILE;
/// ECOLOGICAL animals aren't combat-relevant and don't use this).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Temperament {
    /// Flees, never attacks (deer, rabbit).
    Passive,
    /// Safe until provoked by a specific condition (bear, moose, boar).
    Neutral,
    /// Active threat — engages on detection (apex predator).
    Hostile,
}

/// The situation around the animal this tick — booleans the caller derives from
/// the world (distance, damage events, spatial relationships).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Triggers {
    /// The animal took damage from the target.
    pub attacked: bool,
    /// The target is inside the animal's personal space.
    pub too_close: bool,
    /// The target is between the animal and its offspring (bear + cubs).
    pub between_and_young: bool,
    /// The animal has no escape route (cornered → fights).
    pub cornered: bool,
    /// The target is contesting the animal's resource (its fishing spot, nest,
    /// or rut territory).
    pub resource_contested: bool,
    /// The target is within detection range (used by hostile animals).
    pub detected: bool,
}

/// What the animal decides to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Response {
    /// Carry on — no threat.
    Ignore,
    /// Display/warn before escalating (a bluff charge, a snort).
    Warn,
    /// Retreat from the target.
    Flee,
    /// Commit to attacking the target.
    Attack,
}

/// Do this tick's triggers meet a NEUTRAL animal's provocation bar? These are
/// the specific conditions that turn "safe" into "hostile".
fn neutral_provoked(t: &Triggers) -> bool {
    t.attacked || t.between_and_young || t.cornered || t.resource_contested
}

/// A stateful animal that remembers it was provoked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Provocable {
    pub temperament: Temperament,
    /// Time until which the animal stays committed after provocation (0 = calm).
    provoked_until: f32,
    /// r716: time until which a prior *warning* still counts — a repeat defensive
    /// offense inside this window escalates a bluff to a real attack.
    warned_until: f32,
}

impl Provocable {
    pub fn new(temperament: Temperament) -> Provocable {
        Provocable { temperament, provoked_until: 0.0, warned_until: 0.0 }
    }

    /// Is the animal still in its post-provocation aggressive window?
    pub fn is_provoked(&self, now: f32) -> bool {
        now < self.provoked_until
    }

    /// Decide this tick's [`Response`] and update provocation memory. `hold` is
    /// how long (seconds) a provoked animal stays committed after the triggers
    /// clear.
    pub fn update(&mut self, t: &Triggers, now: f32, hold: f32) -> Response {
        // A still-hot neutral/hostile animal keeps attacking regardless of the
        // instantaneous triggers.
        if self.temperament != Temperament::Passive && self.is_provoked(now) {
            return Response::Attack;
        }
        match self.temperament {
            Temperament::Passive => {
                if t.attacked || t.too_close {
                    Response::Flee
                } else {
                    Response::Ignore
                }
            }
            Temperament::Neutral => {
                if neutral_provoked(t) {
                    self.provoked_until = now + hold.max(0.0);
                    Response::Attack
                } else if t.too_close {
                    // proximity alone is a warning, not an attack
                    Response::Warn
                } else {
                    Response::Ignore
                }
            }
            Temperament::Hostile => {
                if t.detected || t.too_close || t.attacked {
                    self.provoked_until = now + hold.max(0.0);
                    Response::Attack
                } else {
                    Response::Ignore
                }
            }
        }
    }

    /// r716: two-strike defensive escalation — the doc's "bluff charge, then full
    /// charge." A NEUTRAL animal provoked by a *defensive* trigger (young in
    /// danger, resource contested, or cornered) issues ONE warning (a roar / bluff
    /// charge that stops short); a repeat offense within `warn_grace` seconds
    /// escalates to a committed attack. Direct damage (`attacked`) always skips the
    /// bluff. Passive and Hostile animals behave exactly as in [`update`].
    pub fn update_escalating(
        &mut self,
        t: &Triggers,
        now: f32,
        hold: f32,
        warn_grace: f32,
    ) -> Response {
        if self.temperament != Temperament::Passive && self.is_provoked(now) {
            return Response::Attack;
        }
        match self.temperament {
            Temperament::Passive => {
                if t.attacked || t.too_close {
                    Response::Flee
                } else {
                    Response::Ignore
                }
            }
            Temperament::Neutral => {
                // Direct damage: commit immediately, no bluff.
                if t.attacked {
                    self.provoked_until = now + hold.max(0.0);
                    return Response::Attack;
                }
                let defensive = t.between_and_young || t.cornered || t.resource_contested;
                if defensive {
                    if now < self.warned_until {
                        // Second offense while the warning still stands → full attack.
                        self.provoked_until = now + hold.max(0.0);
                        self.warned_until = 0.0;
                        Response::Attack
                    } else {
                        // First offense → a single warning (bluff charge), and open
                        // the grace window during which a repeat escalates.
                        self.warned_until = now + warn_grace.max(0.0);
                        Response::Warn
                    }
                } else if t.too_close {
                    // Plain proximity is a warning but never escalates on its own.
                    Response::Warn
                } else {
                    Response::Ignore
                }
            }
            Temperament::Hostile => {
                if t.detected || t.too_close || t.attacked {
                    self.provoked_until = now + hold.max(0.0);
                    Response::Attack
                } else {
                    Response::Ignore
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trig() -> Triggers {
        Triggers::default()
    }

    #[test]
    fn a_passive_animal_flees_and_never_attacks() {
        let mut deer = Provocable::new(Temperament::Passive);
        assert_eq!(deer.update(&Triggers { too_close: true, ..trig() }, 0.0, 5.0), Response::Flee);
        // even when attacked it flees, it doesn't fight back
        assert_eq!(deer.update(&Triggers { attacked: true, ..trig() }, 1.0, 5.0), Response::Flee);
        // calm otherwise
        assert_eq!(deer.update(&trig(), 2.0, 5.0), Response::Ignore);
    }

    #[test]
    fn a_neutral_animal_is_safe_until_the_specific_trigger() {
        let mut bear = Provocable::new(Temperament::Neutral);
        // just wandering nearby → ignored
        assert_eq!(bear.update(&trig(), 0.0, 5.0), Response::Ignore);
        // getting close → a warning, not an attack
        assert_eq!(bear.update(&Triggers { too_close: true, ..trig() }, 0.1, 5.0), Response::Warn);
    }

    #[test]
    fn coming_between_a_bear_and_its_cubs_provokes_an_attack() {
        let mut bear = Provocable::new(Temperament::Neutral);
        assert_eq!(bear.update(&Triggers { between_and_young: true, ..trig() }, 0.0, 5.0), Response::Attack);
    }

    #[test]
    fn contesting_a_resource_or_cornering_provokes() {
        let mut boar = Provocable::new(Temperament::Neutral);
        assert_eq!(boar.update(&Triggers { resource_contested: true, ..trig() }, 0.0, 5.0), Response::Attack);
        let mut moose = Provocable::new(Temperament::Neutral);
        assert_eq!(moose.update(&Triggers { cornered: true, ..trig() }, 0.0, 5.0), Response::Attack);
    }

    #[test]
    fn provocation_is_sticky_then_calms() {
        let mut bear = Provocable::new(Temperament::Neutral);
        // provoked at t=0, hold 5s
        assert_eq!(bear.update(&Triggers { attacked: true, ..trig() }, 0.0, 5.0), Response::Attack);
        // trigger cleared, but still hot at t=3
        assert_eq!(bear.update(&trig(), 3.0, 5.0), Response::Attack);
        assert!(bear.is_provoked(3.0));
        // past the hold window it calms
        assert_eq!(bear.update(&trig(), 6.0, 5.0), Response::Ignore);
        assert!(!bear.is_provoked(6.0));
    }

    #[test]
    fn a_hostile_animal_attacks_on_detection() {
        let mut wolf = Provocable::new(Temperament::Hostile);
        assert_eq!(wolf.update(&Triggers { detected: true, ..trig() }, 0.0, 3.0), Response::Attack);
        // out of range → ignores
        let mut wolf2 = Provocable::new(Temperament::Hostile);
        assert_eq!(wolf2.update(&trig(), 0.0, 3.0), Response::Ignore);
    }

    #[test]
    fn passive_animals_never_get_stuck_provoked() {
        // A passive animal has no sticky-attack state even if attacked.
        let mut rabbit = Provocable::new(Temperament::Passive);
        rabbit.update(&Triggers { attacked: true, ..trig() }, 0.0, 5.0);
        assert!(!rabbit.is_provoked(1.0), "passive never enters the provoked window");
        assert_eq!(rabbit.update(&trig(), 1.0, 5.0), Response::Ignore);
    }

    #[test]
    fn decisions_are_deterministic() {
        let mut a = Provocable::new(Temperament::Neutral);
        let mut b = Provocable::new(Temperament::Neutral);
        let t = Triggers { between_and_young: true, ..trig() };
        assert_eq!(a.update(&t, 0.0, 5.0), b.update(&t, 0.0, 5.0));
        assert_eq!(a, b);
    }

    #[test]
    fn escalation_warns_then_attacks_on_repeat() {
        let mut bear = Provocable::new(Temperament::Neutral);
        let cub = Triggers { between_and_young: true, ..trig() };
        // First offense near cubs → a warning (bluff charge), not an attack.
        assert_eq!(bear.update_escalating(&cub, 0.0, 5.0, 3.0), Response::Warn);
        // Repeat offense inside the 3s grace → full attack.
        assert_eq!(bear.update_escalating(&cub, 1.0, 5.0, 3.0), Response::Attack);
    }

    #[test]
    fn escalation_resets_after_grace_expires() {
        let mut moose = Provocable::new(Temperament::Neutral);
        let res = Triggers { resource_contested: true, ..trig() };
        assert_eq!(moose.update_escalating(&res, 0.0, 2.0, 3.0), Response::Warn);
        // Let the whole provoked+grace window pass, then a fresh approach → warn again.
        assert_eq!(moose.update_escalating(&res, 10.0, 2.0, 3.0), Response::Warn);
    }

    #[test]
    fn escalation_direct_attack_skips_the_bluff() {
        let mut boar = Provocable::new(Temperament::Neutral);
        let hit = Triggers { attacked: true, ..trig() };
        // Being struck is immediate commitment — no courtesy warning.
        assert_eq!(boar.update_escalating(&hit, 0.0, 5.0, 3.0), Response::Attack);
    }

    #[test]
    fn escalation_plain_proximity_only_warns() {
        let mut deer = Provocable::new(Temperament::Neutral);
        let near = Triggers { too_close: true, ..trig() };
        // Proximity without a defensive trigger warns but never escalates.
        assert_eq!(deer.update_escalating(&near, 0.0, 5.0, 3.0), Response::Warn);
        assert_eq!(deer.update_escalating(&near, 1.0, 5.0, 3.0), Response::Warn);
    }
}
