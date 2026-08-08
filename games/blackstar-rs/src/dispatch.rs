//! DISPATCH — what this crew can actually do next, and why.
//!
//! The bridge menu has been blocked on having something to put in it. A list
//! of fourteen destinations is not a menu; it is a wall. What a player needs
//! is the short list of things that are POSSIBLE right now and the reason
//! each of the others is not, because "Titan (no delta-v)" teaches the
//! system and "Titan" greyed out teaches nothing.
//!
//! Everything needed is already measured. `voyage` says whether the ship can
//! get there and what it costs in supplies. `damage` says what the hull has
//! left. `survival` says whether the bodies aboard can stand on it. `loot`
//! says what it pays. So dispatch is not new logic — it is those four
//! answered together and sorted.
//!
//! **The ordering rule is that a menu should teach.** Reachable options come
//! first by what they pay; unreachable ones stay VISIBLE with the specific
//! obstacle named, because that is how a player learns the reactor is the
//! upgrade they need rather than guessing.

use crate::damage::Condition;
use crate::loadout::Effect;
use crate::loot::demanded;
use crate::sun::Orbit;
use crate::sun::SunState;
use crate::survival::{threat, Body, Threat};
use crate::voyage::{plan, Drive, Leg};

/// A place the crew could go.
#[derive(Clone, Copy, Debug)]
pub struct Destination {
    pub name: &'static str,
    pub orbit: Orbit,
    /// how hard the thing at the end is, 0..3
    pub boss_tier: u8,
}

/// Why an option is closed, in the order a player should hear it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Blocked {
    /// the ship cannot make the trip
    Range,
    /// it could get there and not back
    OneWay,
    /// the crew would not survive standing on it
    Lethal,
    /// nothing aboard could carry what it pays
    Pointless,
}

impl Blocked {
    /// The line the menu shows. Specific, because "unavailable" teaches
    /// nothing and "no delta-v for the return" teaches the whole system.
    pub fn reason(self) -> &'static str {
        match self {
            Blocked::Range => "out of range",
            Blocked::OneWay => "no delta-v to come back",
            Blocked::Lethal => "the crew cannot stand there",
            Blocked::Pointless => "nothing aboard can use what it pays",
        }
    }
}

/// One row of the bridge menu.
#[derive(Clone, Debug)]
pub struct Option_ {
    pub dest: Destination,
    pub leg: Leg,
    /// what it would pay out
    pub pays: Effect,
    /// None if the crew can go
    pub blocked: core::option::Option<Blocked>,
    /// higher is a better use of the trip
    pub value: f32,
}

impl Option_ {
    pub fn open(&self) -> bool {
        self.blocked.is_none()
    }
}

fn noon() -> SunState {
    SunState {
        dir: [0.0, 1.0, 0.0],
        strength: 0.6,
        color: [1.0; 3],
        day: 1.0,
        elev: 1.0,
        flaring: false,
    }
}

/// Assess one destination.
pub fn assess(
    d: &Destination,
    from_au: f64,
    drive: &Drive,
    cond: &Condition,
    crew: &[Body],
    wanted: &[Effect],
) -> Option_ {
    // the hull's actual delta-v, not its nameplate
    let live = Drive { dv_kms: cond.dv_kms(drive.dv_kms), ..*drive };
    let leg = plan(from_au, d.orbit.au as f64, &live, crew.len().max(1) as u32);
    let pays = demanded(&d.orbit);

    let blocked = if !leg.reachable {
        Some(Blocked::Range)
    } else {
        // THE RETURN IS PART OF THE TRIP, and forgetting it is how a crew
        // ends up somewhere interesting forever. Budget both halves against
        // what is left after the outbound burn.
        let back = plan(d.orbit.au as f64, from_au, &Drive { dv_kms: live.dv_kms - leg.dv_kms, ..live }, crew.len().max(1) as u32);
        if !back.reachable {
            Some(Blocked::OneWay)
        } else if crew
            .iter()
            .all(|b| threat(*b, &d.orbit, &noon(), false) != Threat::None)
        {
            Some(Blocked::Lethal)
        } else if !wanted.is_empty() && !wanted.contains(&pays) {
            Some(Blocked::Pointless)
        } else {
            None
        }
    };

    // value: what it pays, weighted by the boss and discounted by what the
    // trip costs in supplies. A rich world you arrive at empty is not rich.
    let supply_frac = (leg.supply_left_s / drive.supply_s.max(1.0)).clamp(0.0, 1.0);
    let value = if blocked.is_some() {
        0.0
    } else {
        (0.4 + 0.2 * d.boss_tier as f32) * (0.35 + 0.65 * supply_frac)
    };

    Option_ { dest: *d, leg, pays, blocked, value }
}

/// Build the bridge list: open options first by value, then the closed ones
/// with their reasons, because a menu should teach rather than hide.
pub fn board(
    dests: &[Destination],
    from_au: f64,
    drive: &Drive,
    cond: &Condition,
    crew: &[Body],
    wanted: &[Effect],
) -> Vec<Option_> {
    let mut v: Vec<Option_> = dests
        .iter()
        .map(|d| assess(d, from_au, drive, cond, crew, wanted))
        .collect();
    v.sort_by(|a, b| {
        b.open()
            .cmp(&a.open())
            .then(b.value.partial_cmp(&a.value).unwrap_or(core::cmp::Ordering::Equal))
            // among closed ones, the nearest obstacle first: a range problem
            // is one upgrade away and a lethal world is not
            .then(
                a.blocked
                    .map(|x| x as u8)
                    .unwrap_or(0)
                    .cmp(&b.blocked.map(|x| x as u8).unwrap_or(0)),
            )
    });
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loadout::{Module, Ship};
    use crate::sun::StarClass;

    fn dest(name: &'static str, au: f32, albedo: f32, air: f32, tier: u8) -> Destination {
        Destination {
            name,
            orbit: Orbit { star: StarClass::G, au, albedo, air },
            boss_tier: tier,
        }
    }

    fn ship() -> Ship {
        Ship { hull: 10, modules: vec![Module::Reactor, Module::Hold, Module::Armour] }
    }

    fn worlds() -> Vec<Destination> {
        vec![
            dest("MARS", 1.524, 0.25, 0.02, 1),
            dest("EUROPA", 5.20, 0.6, 0.0, 2),
            dest("PLUTO", 39.5, 0.5, 0.0, 3),
            // a Venus-like world: REACHABLE and lethal, which is the case
            // worth testing. FORGE at 0.03 AU was lethal and out of range,
            // and Range correctly fires first — you cannot die on a world
            // you cannot reach, so the check order is right and the test was
            // asking the wrong question.
            // Venus's real ninety-two atmospheres: 462 K, past a suit's 400 K
            // ceiling. At 40 atm it comes out at 389 and a suit survives it,
            // which is the greenhouse curve being correct rather than the
            // world being safe.
            dest("CINDER", 0.72, 0.77, 92.0, 3),
        ]
    }

    #[test]
    fn a_closed_option_says_which_wall_it_hit() {
        // "Titan" greyed out teaches nothing; "Titan — out of range" teaches
        // the whole system
        let c = Condition::intact(&ship());
        let b = board(&worlds(), 1.0, &Drive::default(), &c, &[Body::Cybertronian], &[]);
        let pluto = b.iter().find(|o| o.dest.name == "PLUTO").unwrap();
        assert!(!pluto.open());
        assert!(!pluto.blocked.unwrap().reason().is_empty());
        // Range or OneWay — both are delta-v walls and both name themselves;
        // which one depends on whether the outbound alone fits, and pinning
        // it exactly would be testing the drive rather than the menu
        assert!(matches!(pluto.blocked, Some(Blocked::Range) | Some(Blocked::OneWay)),
            "{:?}", pluto.blocked);
    }

    #[test]
    fn the_return_is_part_of_the_trip() {
        // forgetting it is how a crew ends up somewhere interesting forever
        let c = Condition::intact(&ship());
        // just enough to get out to Europa and not back
        let tight = Drive { dv_kms: 15.2, supply_s: 40.0 * 3600.0, ..Drive::default() };
        let o = assess(&worlds()[1], 1.0, &tight, &c, &[Body::Cybertronian], &[]);
        assert!(o.leg.reachable, "the outbound alone should be fine");
        assert_eq!(o.blocked, Some(Blocked::OneWay));
    }

    #[test]
    fn a_holed_reactor_closes_options_the_nameplate_says_are_open() {
        // the damage from the last raid decides where you can go next, which
        // is the whole point of it landing on the voyage
        let s = ship();
        let d = Drive { dv_kms: 30.0, supply_s: 40.0 * 3600.0, ..Drive::default() };
        let good = Condition::intact(&s);
        let mut holed = Condition::intact(&s);
        for (m, cond) in holed.modules.iter_mut() {
            if *m == Module::Reactor {
                *cond = 0.0;
            }
        }
        let a = assess(&worlds()[1], 1.0, &d, &good, &[Body::Cybertronian], &[]);
        let b = assess(&worlds()[1], 1.0, &d, &holed, &[Body::Cybertronian], &[]);
        assert!(a.open(), "an intact ship should reach Europa");
        assert!(!b.open(), "a holed one should not");
    }

    #[test]
    fn a_world_that_would_kill_the_crew_is_named_as_such() {
        let c = Condition::intact(&ship());
        let d = Drive { dv_kms: 90.0, supply_s: 80.0 * 3600.0, ..Drive::default() };
        let o = assess(&worlds()[3], 1.0, &d, &c, &[Body::HumanSuited], &[]);
        assert_eq!(o.blocked, Some(Blocked::Lethal), "{:?} at {} K", o.blocked, o.dest.orbit.surface_k());
        // and a machine can stand there, so the same world opens for a
        // different crew — which is the campaign's premise showing up in a
        // menu rather than in prose
        let bot = assess(&worlds()[3], 1.0, &d, &c, &[Body::Cybertronian], &[]);
        assert_ne!(bot.blocked, Some(Blocked::Lethal));
    }

    #[test]
    fn a_trip_for_loot_nobody_can_wear_is_refused() {
        let c = Condition::intact(&ship());
        let d = Drive { dv_kms: 60.0, supply_s: 80.0 * 3600.0, ..Drive::default() };
        let mars = worlds()[0];
        let pays = demanded(&mars.orbit);
        let other = if pays == Effect::ColdFloor { Effect::HeatCeiling } else { Effect::ColdFloor };
        let o = assess(&mars, 1.0, &d, &c, &[Body::Cybertronian], &[other]);
        assert_eq!(o.blocked, Some(Blocked::Pointless));
        // and asking for what it pays opens it
        let want = assess(&mars, 1.0, &d, &c, &[Body::Cybertronian], &[pays]);
        assert!(want.open());
    }

    #[test]
    fn open_options_come_first_and_the_nearest_obstacle_next() {
        // a menu should teach: a range problem is one upgrade away, a lethal
        // world is not, so they sort in that order
        let c = Condition::intact(&ship());
        let b = board(&worlds(), 1.0, &Drive::default(), &c, &[Body::Cybertronian], &[]);
        let first_closed = b.iter().position(|o| !o.open()).unwrap_or(b.len());
        assert!(b[..first_closed].iter().all(|o| o.open()));
        assert!(b[first_closed..].iter().all(|o| !o.open()));
        // and open ones are sorted richest first
        for w in b[..first_closed].windows(2) {
            assert!(w[0].value >= w[1].value);
        }
    }

    #[test]
    fn arriving_empty_makes_a_rich_world_worth_less() {
        // a world you reach with no supplies is not the same prize
        let c = Condition::intact(&ship());
        let fat = Drive { dv_kms: 60.0, supply_s: 200.0 * 3600.0, ..Drive::default() };
        let lean = Drive { supply_s: 2.2 * 3600.0, ..fat };
        let a = assess(&worlds()[1], 1.0, &fat, &c, &[Body::Cybertronian], &[]);
        let b = assess(&worlds()[1], 1.0, &lean, &c, &[Body::Cybertronian], &[]);
        assert!(a.value > b.value, "fat {} lean {}", a.value, b.value);
    }
}
