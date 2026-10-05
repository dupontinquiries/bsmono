//! cues — what the car just did, named, for the shell to play.
//!
//! The vehicle model has no events: `land()` is private and decides clean-or-rough by
//! itself, a boss attack is a health that is smaller than it was, nitro is a number that
//! goes down. The shell wants moments — a launch, a landing, a hit — to put a sound (or a
//! flash, or a shake) on, and this is the seam that turns one step's before-and-after into
//! those moments. Pure: a [`Listener`] remembers the last state it saw and names the
//! differences; it knows nothing about audio.
//!
//! The engine is the one continuous sound, so it is not a cue but a note ([`engine`]): a
//! speed and a load, read every frame.

use crate::encounter::EncounterOutcome;
use crate::vehicle::Vehicle;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cue {
    /// The wheels left the ground: a jump, or a ramp's end.
    Launch,
    /// Back on the ground. `clean` is the nitro-earning kind; the other knocked armor off.
    Land { clean: bool },
    /// Armor lost for any reason but a rough landing: a boss attack, a ram's own cost.
    Hit { amount: f32 },
    /// The boss took ram damage from us.
    Ram { amount: f32 },
    /// Nitro started burning (the edge, not every tick it burns).
    Nitro,
    /// The tank just ran dry.
    OutOfGas,
}

/// The engine's note. `rpm` is speed over the car's top speed (a little over 1 under
/// nitro), `load` is how hard the throttle is on -- zero in the air and with a dry tank,
/// where the throttle does nothing to the car and should do nothing to the sound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Engine {
    pub rpm: f32,
    pub load: f32,
}

pub fn engine(v: &Vehicle, throttle: f32) -> Engine {
    let rpm = (v.vel_x.abs() / v.stats.top_speed.max(1.0)).clamp(0.0, 1.25);
    let load = if v.gas > 0.0 && !v.airborne && throttle.is_finite() { throttle.abs().clamp(0.0, 1.0) } else { 0.0 };
    Engine { rpm, load }
}

/// Watches one vehicle step to step.
#[derive(Clone, Copy, Debug)]
pub struct Listener {
    airborne: bool,
    health: f32,
    nitro: f32,
    gas_empty: bool,
    burning: bool,
}

impl Listener {
    pub fn new(v: &Vehicle) -> Listener {
        Listener { airborne: v.airborne, health: v.health, nitro: v.nitro, gas_empty: v.gas <= 0.0, burning: false }
    }

    /// Forget the last state: after a respawn, so the teleport back to the start is not
    /// heard as a landing and the refilled tank is not a hit undone.
    pub fn reset(&mut self, v: &Vehicle) {
        *self = Listener::new(v);
    }

    /// What changed between the last step and this one. `outcome` is the encounter's,
    /// when there is a boss.
    pub fn step(&mut self, v: &Vehicle, nitro_active: bool, outcome: Option<EncounterOutcome>) -> Vec<Cue> {
        let mut cues = Vec::new();
        let launched = !self.airborne && v.airborne;
        let landed = self.airborne && !v.airborne;
        let dropped = (self.health - v.health).max(0.0);
        if launched {
            cues.push(Cue::Launch);
        }
        let rough = landed && dropped > 0.0;
        if landed {
            cues.push(Cue::Land { clean: !rough });
        }
        // a rough landing's armor is the landing's; anything else that cost armor is a hit
        if dropped > 0.0 && !rough {
            cues.push(Cue::Hit { amount: dropped });
        }
        if let Some(o) = outcome {
            if o.boss_damage_dealt > 0.0 {
                cues.push(Cue::Ram { amount: o.boss_damage_dealt });
            }
        }
        // nitro burns only on the ground with the key down and something in the tank;
        // "went down while the key was held" is that, however the step spent it
        let burning = nitro_active && v.nitro < self.nitro;
        if burning && !self.burning {
            cues.push(Cue::Nitro);
        }
        let gas_empty = v.gas <= 0.0;
        if gas_empty && !self.gas_empty {
            cues.push(Cue::OutOfGas);
        }
        self.airborne = v.airborne;
        self.health = v.health;
        self.nitro = v.nitro;
        self.gas_empty = gas_empty;
        self.burning = burning;
        cues
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boss::{Attack, AttackKind, Boss};
    use crate::track::{Segment, Track};
    use crate::vehicle::CarKind;

    const DT: f32 = 1.0 / 60.0;
    const G: f32 = -9.8;

    fn ramp_gap_flat() -> Track {
        Track::new(
            "cues",
            vec![
                Segment::Flat { length: 10.0, height: 0.0 },
                Segment::Ramp { length: 5.0, from_height: 0.0, to_height: 4.0 },
                Segment::Gap { length: 6.0 },
                Segment::Flat { length: 60.0, height: 0.0 },
            ],
        )
    }

    /// Drive the strip at full throttle and collect every cue: one launch at the ramp's
    /// end, one landing on the far side, nothing else -- in that order, and once each.
    #[test]
    fn a_jump_is_one_launch_and_one_landing() {
        let track = ramp_gap_flat();
        let mut v = Vehicle::new(CarKind::Pillager);
        let mut l = Listener::new(&v);
        let mut all = Vec::new();
        for _ in 0..(6.0 / DT) as usize {
            crate::sim::advance(&mut v, &track, DT, 1.0, 0.0, false, G);
            all.extend(l.step(&v, false, None));
        }
        let launches = all.iter().filter(|c| **c == Cue::Launch).count();
        let lands = all.iter().filter(|c| matches!(c, Cue::Land { .. })).count();
        assert_eq!(launches, 1, "{all:?}");
        assert_eq!(lands, 1, "{all:?}");
        assert!(all.iter().position(|c| *c == Cue::Launch) < all.iter().position(|c| matches!(c, Cue::Land { .. })));
        assert!(matches!(all.iter().find(|c| matches!(c, Cue::Land { .. })), Some(Cue::Land { clean: true })), "a flat landing from a ramp is clean: {all:?}");
        assert!(!all.iter().any(|c| matches!(c, Cue::Hit { .. })), "a clean landing costs no armor: {all:?}");
    }

    /// Upside down at touchdown: the landing is rough, the armor it costs is the landing's
    /// and not a separate hit.
    #[test]
    fn a_rough_landing_is_not_also_a_hit() {
        let mut v = Vehicle::new(CarKind::RedSedan);
        v.launch(5.0);
        let mut l = Listener::new(&v);
        let mut all = Vec::new();
        for _ in 0..(3.0 / DT) as usize {
            v.rotation = std::f32::consts::PI; // held upside down all the way down
            v.step(DT, 0.0, 0.0, false, G);
            all.extend(l.step(&v, false, None));
        }
        assert!(v.health < v.stats.armor, "the landing knocked armor off");
        assert_eq!(all, vec![Cue::Land { clean: false }], "{all:?}");
    }

    /// A ram is heard twice: the boss takes it (Ram) and the ram's own cost is a Hit.
    #[test]
    fn a_ram_is_a_ram_and_its_cost_is_a_hit() {
        let track = Track::new("flat", vec![Segment::Flat { length: 200.0, height: 0.0 }]);
        let mut v = Vehicle::new(CarKind::Behemoth);
        v.vel_x = 20.0;
        v.pos_x = 40.0;
        let mut boss = Boss::new("wall", 500.0, 0.5, vec![Attack { kind: AttackKind::GroundSlam, telegraph: 100.0, cooldown: 100.0 }]);
        boss.pos_x = 41.0;
        let mut l = Listener::new(&v);
        let o = crate::encounter::advance(&mut v, &track, &mut boss, DT, 1.0, 0.0, false, G);
        assert!(o.boss_damage_dealt > 0.0, "the fixture rams: {o:?}");
        let cues = l.step(&v, false, Some(o));
        assert!(cues.iter().any(|c| matches!(c, Cue::Ram { amount } if *amount > 0.0)), "{cues:?}");
        assert!(cues.iter().any(|c| matches!(c, Cue::Hit { amount } if *amount > 0.0)), "{cues:?}");
    }

    /// Nitro is heard when it starts burning, once, and not on every tick it burns; a dry
    /// tank is heard once.
    #[test]
    fn nitro_and_an_empty_tank_are_edges() {
        let mut v = Vehicle::new(CarKind::RedSedan);
        v.nitro = 30.0;
        v.gas = 0.02;
        let mut l = Listener::new(&v);
        let mut all = Vec::new();
        for _ in 0..120 {
            v.step(DT, 1.0, 0.0, true, G);
            all.extend(l.step(&v, true, None));
        }
        assert_eq!(all.iter().filter(|c| **c == Cue::Nitro).count(), 1, "{all:?}");
        assert_eq!(all.iter().filter(|c| **c == Cue::OutOfGas).count(), 1, "{all:?}");
        assert!(v.gas <= 0.0 && v.nitro < 30.0, "the fixture burned both");
        // a reset forgets: refilling the tank for a retry is not an event
        let fresh = Vehicle::new(CarKind::RedSedan);
        l.reset(&fresh);
        assert!(l.step(&fresh, false, None).is_empty());
    }

    /// The engine note: louder with the throttle, higher with speed, silent in the air and
    /// with a dry tank where the throttle does nothing to the car.
    #[test]
    fn the_engine_follows_speed_and_load_and_is_quiet_where_the_throttle_does_nothing() {
        let mut v = Vehicle::new(CarKind::RedSedan);
        assert_eq!(engine(&v, 0.0), Engine { rpm: 0.0, load: 0.0 });
        assert_eq!(engine(&v, 1.0).load, 1.0);
        assert_eq!(engine(&v, -0.5).load, 0.5, "reverse is load too");
        v.vel_x = v.stats.top_speed * 0.5;
        assert!((engine(&v, 1.0).rpm - 0.5).abs() < 1e-5);
        v.vel_x = v.stats.top_speed * 2.0;
        assert_eq!(engine(&v, 1.0).rpm, 1.25, "capped a little over the top: nitro");
        v.airborne = true;
        assert_eq!(engine(&v, 1.0).load, 0.0, "no load in the air");
        v.airborne = false;
        v.gas = 0.0;
        assert_eq!(engine(&v, 1.0).load, 0.0, "no load on fumes");
        assert_eq!(engine(&v, f32::NAN).load, 0.0);
    }
}
