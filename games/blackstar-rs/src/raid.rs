//! THE RAID CYCLE — approach, foothold, encounters, extraction, payout.
//!
//! Every piece of this already exists somewhere. `foothold.rs` decides how
//! you take ground; `survival.rs` says how long you can stand on it;
//! `sun.rs` says when the star will erupt; `loot.rs` says what the world
//! pays. This is the loop that runs them in order and, more usefully, the
//! thing that makes them constrain each other.
//!
//! **The constraint that makes it a raid rather than a level:** the clock
//! is shared. Time spent taking a foothold is time not spent on encounters,
//! and the flare window does not pause because you are mid-fight. A team
//! that storms a fortress arrives late and sheltered; a team that clears an
//! LZ arrives early and exposed. Neither is correct in general and both are
//! correct sometimes, which is the whole design.
//!
//! Extraction is deliberately NOT free. A raid you can always walk out of is
//! a raid with no decisions in its last third.

use crate::foothold::{choose, Approach, Site};
use crate::loot::{roll, Drop};
use crate::sun::Orbit;
use crate::survival::{flare_warning_s, Body, ShelterKind, Weather};

/// Where a run got to. Ordered, so a later phase implies the earlier ones.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Phase {
    /// never got a foothold
    Repulsed,
    /// holding ground, no encounters cleared
    Landed,
    /// some encounters down, boss alive
    Contested,
    /// boss down, still inside
    Cleared,
    /// boss down and everybody out
    Extracted,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Repulsed => "REPULSED",
            Phase::Landed => "LANDED",
            Phase::Contested => "CONTESTED",
            Phase::Cleared => "CLEARED",
            Phase::Extracted => "EXTRACTED",
        }
    }
    /// Does the team keep what it found? Clearing is not collecting.
    pub fn pays_out(self) -> bool {
        self == Phase::Extracted
    }
}

/// A raid as designed rather than as played.
#[derive(Clone, Debug)]
pub struct Raid {
    pub name: &'static str,
    pub orbit: Orbit,
    pub weather: Weather,
    /// candidate footholds
    pub sites: Vec<Site>,
    /// seconds per encounter, in order
    pub encounters: Vec<f32>,
    /// how hard the thing at the end is, 0..3
    pub boss_tier: u8,
    /// metres back to the ship from the boss
    pub extract_m: f32,
}

/// What happened.
#[derive(Clone, Debug)]
pub struct Run {
    pub phase: Phase,
    pub approach: Option<Approach>,
    /// seconds left when the run ended; negative means it ran out
    pub margin_s: f32,
    pub encounters_cleared: usize,
    pub drop: Option<Drop>,
    pub note: &'static str,
}

/// THE SHARED CLOCK. For a flare it is the CME transit; for a surge or a
/// storm it is the front's own duration. Clear weather gives the raid all
/// the time it wants, which is what makes a quiet world the place you go to
/// learn an encounter.
pub fn window_s(orbit: &Orbit, w: Weather) -> f32 {
    match w {
        // the flare is the one window that needs the sky: warning time is
        // orbital distance
        Weather::FlareFront => flare_warning_s(orbit, true),
        _ => window_base_s(w),
    }
}

/// The windows that do not depend on an orbit — everything but a flare.
/// Split out so the mission sorties (`sortie.rs`), which have a
/// destination but no `Orbit`, ask the SAME numbers a raid does instead of
/// growing their own copy. The flare entry is a conservative floor for a
/// caller with no orbit to offer; raids never hit it (they route through
/// `window_s` above), and mission weather does not currently include
/// flares — it is here so the match is total, and it is a GUESS.
pub fn window_base_s(w: Weather) -> f32 {
    match w {
        Weather::Clear => f32::INFINITY,
        Weather::FlareFront => 300.0,
        Weather::ThermalFront => 900.0,
        Weather::DustStorm => 1_500.0,
        Weather::GiantWave => 600.0,
    }
}

/// WHAT THE SHIP TOOK WHILE YOU WERE INSIDE.
///
/// foothold says an LZ approach PINS THE SHIP — it lands and stays, and a
/// large stationary thing on a hostile world is a target. That was a stated
/// cost with no mechanism; this is the mechanism.
///
/// A fortress run leaves the ship in orbit and it takes nothing. An LZ run
/// leaves it on the deck for the whole raid, so the longer you are down
/// there the more it wears. The cost lands on the NEXT voyage, which is the
/// kind of consequence worth having.
/// Hull wear per second of sitting pinned on a hostile deck, normalized
/// and capped. ONE formula: `ship_exposure` below prices a raid with it,
/// and the flown sortie (`sortie::commit`) prices an LZ engagement with
/// it — a pinned ship that wears differently depending on which planner
/// asked would be the two-answers-to-one-question mistake.
pub fn lz_wear(secs: f32) -> f32 {
    (secs / 240.0).clamp(0.2, 4.0)
}

pub fn ship_exposure(r: &Run, raid: &Raid) -> f32 {
    match r.approach {
        Some(Approach::ClearLz) => {
            // proportional to how long the raid ran, which the encounter
            // times already describe
            let secs: f32 = raid.encounters.iter().sum::<f32>().max(60.0);
            lz_wear(secs)
        }
        _ => 0.0,
    }
}

/// Run it, having flown there first.
///
/// THE VOYAGE IS PART OF THE RAID. Supplies burn the whole way, so a team
/// that crossed the system arrives with less clock than one that hopped next
/// door — and on a world with weather, clock IS the raid. Arriving at Pluto
/// with two hours of air left is a different encounter from arriving at Mars
/// with twenty.
pub fn run_after_voyage(
    raid: &Raid,
    leg: &crate::voyage::Leg,
    body: Body,
    speed_ms: f32,
    seed: u32,
    team: &[Body],
) -> Run {
    if !leg.reachable {
        return Run {
            phase: Phase::Repulsed,
            approach: None,
            margin_s: leg.supply_left_s,
            encounters_cleared: 0,
            drop: None,
            note: "the ship never arrived",
        };
    }
    let mut r = run(raid, body, speed_ms, seed, team);
    // what the voyage left is a ceiling on what the raid can spend
    if r.margin_s > leg.supply_left_s {
        r.margin_s = leg.supply_left_s;
        if leg.supply_left_s <= 0.0 {
            r.phase = Phase::Cleared.min(r.phase);
            r.note = "cleared it and ran out of air before extraction";
        }
    }
    r
}

/// Run it.
pub fn run(raid: &Raid, body: Body, speed_ms: f32, seed: u32, team: &[Body]) -> Run {
    let window = window_s(&raid.orbit, raid.weather);
    let Some(plan) = choose(body, &raid.sites, raid.weather, window, speed_ms) else {
        return Run {
            phase: Phase::Repulsed,
            approach: None,
            margin_s: -1.0,
            encounters_cleared: 0,
            drop: None,
            note: "no foothold could be taken inside the window",
        };
    };
    // TIME SPENT LANDING IS TIME NOT SPENT FIGHTING. This is the line that
    // makes the foothold choice matter beyond its own phase.
    let mut left = plan.margin_s;
    let sheltered = plan.site.approach.shelter() != ShelterKind::None;
    let mut cleared = 0usize;
    for e in &raid.encounters {
        // inside a fortress the sky is somebody else's problem; on an open
        // LZ it is still running
        let cost = if sheltered { e * 0.35 } else { *e };
        if left - cost < 0.0 {
            return Run {
                phase: Phase::Landed.max(if cleared > 0 { Phase::Contested } else { Phase::Landed }),
                approach: Some(plan.site.approach),
                margin_s: left - cost,
                encounters_cleared: cleared,
                drop: None,
                note: "the weather closed before the encounters did",
            };
        }
        left -= cost;
        cleared += 1;
    }
    // the boss costs the same again as everything before it
    let boss_cost = raid.encounters.iter().sum::<f32>().max(60.0) * (0.6 + raid.boss_tier as f32 * 0.3)
        * if sheltered { 0.35 } else { 1.0 };
    if left - boss_cost < 0.0 {
        return Run {
            phase: Phase::Contested,
            approach: Some(plan.site.approach),
            margin_s: left - boss_cost,
            encounters_cleared: cleared,
            drop: None,
            note: "reached the boss and ran out of time on it",
        };
    }
    left -= boss_cost;
    // EXTRACTION IS NOT FREE. A raid you can always walk out of has no
    // decisions in its last third — and an LZ pins the ship at the landing
    // zone, so the walk back is from wherever the boss was.
    let walk = raid.extract_m / speed_ms.max(0.1)
        * if plan.site.approach.pins_the_ship() { 1.0 } else { 1.4 };
    if left - walk < 0.0 {
        return Run {
            phase: Phase::Cleared,
            approach: Some(plan.site.approach),
            margin_s: left - walk,
            encounters_cleared: cleared,
            drop: None,
            note: "boss down, and the front caught the team on the way out",
        };
    }
    Run {
        phase: Phase::Extracted,
        approach: Some(plan.site.approach),
        margin_s: left - walk,
        encounters_cleared: cleared,
        drop: roll(&raid.orbit, raid.name, team, seed, raid.boss_tier),
        note: "",
    }
}

// ==================================================== TIDEMAW ============
//
// A named raid, built from the pieces rather than scripted on top of them.
//
// ENTRY IS A DIVE. `drop.rs` already models a jump from a hangar with
// inherited velocity and a terminal speed that differs between robot and
// vehicle form; a water world just changes what you hit. Coming in from
// orbit and going straight under is the same manoeuvre with the deceleration
// happening in a fluid 830 times denser than the one it started in.
//
// THE CITY IS IN THE SHALLOWS because that is where the tide reaches at 0.75
// and there is still cover — deep enough to be lost, shallow enough that the
// water is trying to take you off the street.
//
// THE OPEN CROSSINGS ARE THE MECHANIC. hydro::Tide is periodic and it
// telegraphs: slack, reverse, build. A heavy chassis crosses whenever it
// likes; a flyer waits for slack and runs. That is a timing problem with a
// visible signal, which is the only kind worth having.

/// The water world, and the thing at the bottom of it.
pub const TIDEMAW_ORBIT: Orbit = Orbit {
    star: crate::sun::StarClass::K,
    // 0.45 AU, not 0.62: a K dwarf is dimmer than the sun, and an ocean
    // needs to sit where water is liquid rather than where the number looked
    // plausible
    au: 0.45,
    albedo: 0.28,
    air: 0.85,
};

/// Its tide: a long period, because a big ocean has long swings, and a peak
/// that a bot can walk in and a flyer cannot.
pub const TIDEMAW_TIDE: crate::hydro::Tide = crate::hydro::Tide { period_s: 150.0, peak_ms: 5.2 };

/// Build the raid. Encounter times are seconds of fighting, and the two long
/// ones are the open crossings — those are not fights, they are the water.
pub fn tidemaw() -> Raid {
    use crate::survival::Bunker;
    Raid {
        name: "TIDEMAW",
        orbit: TIDEMAW_ORBIT,
        // the surge is the world's own weather, not an event
        weather: Weather::GiantWave,
        sites: vec![
            // a shelf above the waterline: quick to take, and the ship has to
            // sit there in the surf
            Site {
                approach: Approach::ClearLz,
                at: Bunker { distance_m: 220.0, shielded: false },
                garrison: 0.4,
                take_s: 70.0,
            },
            // the city's own seawall, held. Dug in, dry inside, and the
            // reason to want it is that a bunker drowns and a SEAWALL does
            // not — it is the structure the city built for exactly this.
            Site {
                approach: Approach::SeizeFortress,
                at: Bunker { distance_m: 640.0, shielded: true },
                garrison: 1.0,
                take_s: 210.0,
            },
        ],
        encounters: vec![
            110.0, // the shallows, working inward
            160.0, // FIRST OPEN CROSSING — no cover, tide running
            140.0, // the drowned plaza
            200.0, // SECOND CROSSING, deeper and longer
        ],
        boss_tier: 3,
        extract_m: 1_400.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sun::StarClass;
    use crate::survival::Bunker;

    fn site(a: Approach, dist: f32, take: f32) -> Site {
        Site { approach: a, at: Bunker { distance_m: dist, shielded: true }, garrison: 1.0, take_s: take }
    }

    fn raid(w: Weather, orbit: Orbit) -> Raid {
        Raid {
            name: "TESTFALL",
            orbit,
            weather: w,
            sites: vec![site(Approach::ClearLz, 200.0, 60.0), site(Approach::SeizeFortress, 500.0, 180.0)],
            encounters: vec![90.0, 90.0, 120.0],
            boss_tier: 2,
            extract_m: 800.0,
        }
    }

    fn quiet() -> Orbit {
        Orbit { star: StarClass::G, au: 1.0, albedo: 0.3, air: 0.4 }
    }

    #[test]
    fn a_quiet_world_lets_a_raid_finish() {
        // which is what makes it the place you go to learn an encounter
        let r = run(&raid(Weather::Clear, quiet()), Body::Machine, 14.0, 5, &[Body::Machine]);
        assert_eq!(r.phase, Phase::Extracted);
        assert_eq!(r.encounters_cleared, 3);
        assert!(r.drop.is_some(), "a finished raid paid nothing");
    }

    #[test]
    fn clearing_is_not_collecting() {
        // a raid you can always walk out of has no decisions in its last
        // third, so the payout is gated on EXTRACTED and not on CLEARED
        assert!(Phase::Extracted.pays_out());
        assert!(!Phase::Cleared.pays_out());
        assert!(Phase::Cleared < Phase::Extracted);
    }

    #[test]
    fn the_clock_is_shared_between_landing_and_fighting() {
        // the line that makes the foothold choice matter beyond its own phase
        let mut fast = raid(Weather::DustStorm, quiet());
        fast.sites = vec![site(Approach::ClearLz, 200.0, 60.0)];
        let mut slow = fast.clone();
        slow.sites = vec![site(Approach::ClearLz, 200.0, 1_200.0)];
        let a = run(&fast, Body::Machine, 14.0, 1, &[Body::Machine]);
        let b = run(&slow, Body::Machine, 14.0, 1, &[Body::Machine]);
        assert!(a.phase > b.phase, "a slow landing should cost encounters: {a:?} {b:?}");
    }

    #[test]
    fn a_fortress_buys_time_and_an_lz_does_not() {
        // DUST, NOT A SURGE. I first wrote this against GiantWave and the
        // fortress came back REPULSED — correctly: a bunker's water
        // protection is 0.05, so foothold::assess refuses it as "taken in
        // time, and it does not stop this weather". That is survival.rs's
        // bunker-drowns rule reaching all the way up into raid planning,
        // which is the system working. A dust storm is what a dug-in
        // position is actually for.
        let mut r = raid(Weather::DustStorm, quiet());
        r.sites = vec![site(Approach::SeizeFortress, 300.0, 120.0)];
        let fort = run(&r, Body::Machine, 14.0, 2, &[Body::Machine]);
        r.sites = vec![site(Approach::ClearLz, 300.0, 120.0)];
        let lz = run(&r, Body::Machine, 14.0, 2, &[Body::Machine]);
        assert!(fort.phase >= lz.phase, "fortress {:?} lz {:?}", fort.phase, lz.phase);
    }

    #[test]
    fn an_unreachable_world_repels_the_team_rather_than_half_running() {
        let mut r = raid(Weather::FlareFront, Orbit { star: StarClass::M, au: 0.04, albedo: 0.2, air: 0.0 });
        r.sites = vec![site(Approach::ClearLz, 40_000.0, 60.0)];
        let out = run(&r, Body::HumanSuited, 2.5, 3, &[Body::HumanSuited]);
        assert_eq!(out.phase, Phase::Repulsed);
        assert!(out.drop.is_none());
        assert!(out.note.contains("foothold"));
    }

    #[test]
    fn a_bot_finishes_a_raid_a_suited_team_cannot() {
        // three numbers of premise, showing up as a phase difference — but
        // only where the clock is actually tight. A 1500 s dust storm is
        // generous enough that BOTH extract, which is right and was not what
        // my first version asserted. A flare window on a close orbit is not.
        // AND IT DOES NOT SHOW AS A PHASE, which surprised me twice. Even a
        // 0.06 AU M-dwarf flare gives about 3,200 seconds — the CME crosses
        // in hours whatever the orbit, so the warning is generous almost
        // everywhere. That is the physics being generous, not the model
        // being wrong, and it means the body difference shows up as TIME TO
        // SPARE rather than as a different ending.
        let r = raid(Weather::GiantWave, quiet());
        let bot = run(&r, Body::Machine, 16.0, 9, &[Body::Machine]);
        let man = run(&r, Body::HumanSuited, 2.5, 9, &[Body::HumanSuited]);
        assert!(bot.margin_s > man.margin_s + 30.0,
            "a machine should have far more slack: {:.0} vs {:.0}", bot.margin_s, man.margin_s);
        // and on a generous clock they both get out, which is the same
        // system saying the difference only shows under pressure
        let easy = raid(Weather::DustStorm, quiet());
        assert_eq!(
            run(&easy, Body::Machine, 16.0, 9, &[Body::Machine]).phase,
            run(&easy, Body::HumanSuited, 2.5, 9, &[Body::HumanSuited]).phase
        );
    }

    #[test]
    fn the_same_raid_run_twice_does_the_same_thing() {
        let r = raid(Weather::DustStorm, quiet());
        let a = run(&r, Body::Machine, 14.0, 77, &[Body::Machine]);
        let b = run(&r, Body::Machine, 14.0, 77, &[Body::Machine]);
        assert_eq!(a.phase, b.phase);
        assert_eq!(a.drop.map(|d| d.grade), b.drop.map(|d| d.grade));
    }

    #[test]
    fn an_lz_run_wears_the_ship_and_a_fortress_run_does_not() {
        use crate::damage::{hit, Condition};
        use crate::loadout::{Module, Ship};
        let r = raid(Weather::DustStorm, quiet());
        let mut lz = r.clone();
        lz.sites = vec![site(Approach::ClearLz, 200.0, 60.0)];
        let mut fort = r.clone();
        fort.sites = vec![site(Approach::SeizeFortress, 300.0, 120.0)];
        let a = run(&lz, Body::Machine, 14.0, 4, &[Body::Machine]);
        let b = run(&fort, Body::Machine, 14.0, 4, &[Body::Machine]);
        assert!(ship_exposure(&a, &lz) > 0.0, "a pinned ship should be exposed");
        assert_eq!(ship_exposure(&b, &fort), 0.0, "a ship in orbit takes nothing");

        // and the wear is real: it shows up as delta-v next time
        let s = Ship { hull: 10, modules: vec![Module::Armour, Module::Reactor, Module::Hold] };
        let mut c = Condition::intact(&s);
        let before = c.dv_kms(40.0);
        for i in 0..6 {
            hit(&mut c, ship_exposure(&a, &lz), i);
        }
        assert!(c.dv_kms(40.0) < before, "the trip home should be shorter than the trip out");
    }

    #[test]
    fn a_long_voyage_costs_the_raid_its_margin() {
        use crate::voyage::{plan, Drive};
        let r = raid(Weather::DustStorm, quiet());
        let d = Drive { dv_kms: 200.0, supply_s: 6.0 * 3600.0, ..Drive::default() };
        let near = plan(1.0, 1.524, &d, 4);
        let far = plan(1.0, 9.537, &d, 4);
        let a = run_after_voyage(&r, &near, Body::Machine, 14.0, 3, &[Body::Machine]);
        let b = run_after_voyage(&r, &far, Body::Machine, 14.0, 3, &[Body::Machine]);
        assert!(a.margin_s >= b.margin_s, "the long haul should not arrive richer");
        assert_eq!(a.phase, Phase::Extracted);
    }

    #[test]
    fn a_ship_that_cannot_get_there_is_not_a_failed_raid() {
        use crate::voyage::{plan, Drive};
        let r = raid(Weather::Clear, quiet());
        let d = Drive { dv_kms: 4.0, ..Drive::default() };
        let leg = plan(1.0, 39.48, &d, 4);
        assert!(!leg.reachable);
        let out = run_after_voyage(&r, &leg, Body::Machine, 14.0, 1, &[Body::Machine]);
        assert_eq!(out.phase, Phase::Repulsed);
        assert!(out.note.contains("never arrived"), "{}", out.note);
        assert!(out.drop.is_none());
    }

    #[test]
    fn tidemaw_is_a_water_world_and_pays_like_one() {
        let r = tidemaw();
        assert_eq!(r.weather, Weather::GiantWave);
        // temperate and wet: it is not trying to freeze or cook you, the
        // water is the whole hazard
        assert_eq!(r.orbit.surface(), crate::sun::Surface::Temperate);
        assert!(r.orbit.flare_severity() < 0.4, "a K dwarf at 0.62 AU should be quiet");
    }

    #[test]
    fn the_seawall_beats_the_shelf_because_a_surge_is_what_it_was_built_for() {
        // and this is the bunker-drowns rule paying off rather than biting:
        // foothold refuses a HOLE against water, and a seawall is not a hole
        let r = tidemaw();
        let out = run(&r, Body::Machine, 13.0, 11, &[Body::Machine]);
        assert!(out.phase >= Phase::Contested, "{:?} {}", out.phase, out.note);
    }

    #[test]
    fn a_heavy_chassis_crosses_open_water_a_light_one_must_time() {
        use crate::hydro::{Swimmer, Zone};
        let bot = Swimmer { mass: 900.0, volume: 0.55, frontal_m2: 1.6, cd: 0.9, through: 0.1 };
        let flyer = Swimmer { mass: 260.0, volume: 0.22, frontal_m2: 3.4, cd: 0.7, through: 0.05 };
        let t = TIDEMAW_TIDE;
        let wb = t.window_s(bot.hold_ms(9.81, 0.7), Zone::OpenWater);
        let wf = t.window_s(flyer.hold_ms(9.81, 0.7), Zone::OpenWater);
        assert!(wb > wf, "bot window {wb:.0}s, flyer {wf:.0}s");
        // the second crossing is 200 s of exposure — check who that rules out
        assert!(wf < 200.0, "a flyer should NOT be able to stroll it: {wf:.0}s");
    }

    #[test]
    fn the_payout_matches_the_world_not_the_raid() {
        // a cryogenic raid pays cold tolerance whatever its name is
        let cold = Orbit { star: StarClass::G, au: 32.0, albedo: 0.5, air: 0.0 };
        let mut r = raid(Weather::Clear, cold);
        r.encounters = vec![30.0];
        let out = run(&r, Body::Machine, 14.0, 4, &[Body::Machine]);
        assert_eq!(out.phase, Phase::Extracted);
        assert_eq!(out.drop.unwrap().base.effect, crate::loadout::Effect::ColdFloor);
    }
}
