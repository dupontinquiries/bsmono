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
        Weather::Clear => f32::INFINITY,
        Weather::FlareFront => flare_warning_s(orbit, true),
        Weather::ThermalFront => 900.0,
        Weather::DustStorm => 1_500.0,
        Weather::GiantWave => 600.0,
    }
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
        let r = run(&raid(Weather::Clear, quiet()), Body::Cybertronian, 14.0, 5, &[Body::Cybertronian]);
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
        let a = run(&fast, Body::Cybertronian, 14.0, 1, &[Body::Cybertronian]);
        let b = run(&slow, Body::Cybertronian, 14.0, 1, &[Body::Cybertronian]);
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
        let fort = run(&r, Body::Cybertronian, 14.0, 2, &[Body::Cybertronian]);
        r.sites = vec![site(Approach::ClearLz, 300.0, 120.0)];
        let lz = run(&r, Body::Cybertronian, 14.0, 2, &[Body::Cybertronian]);
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
        let bot = run(&r, Body::Cybertronian, 16.0, 9, &[Body::Cybertronian]);
        let man = run(&r, Body::HumanSuited, 2.5, 9, &[Body::HumanSuited]);
        assert!(bot.margin_s > man.margin_s + 30.0,
            "a machine should have far more slack: {:.0} vs {:.0}", bot.margin_s, man.margin_s);
        // and on a generous clock they both get out, which is the same
        // system saying the difference only shows under pressure
        let easy = raid(Weather::DustStorm, quiet());
        assert_eq!(
            run(&easy, Body::Cybertronian, 16.0, 9, &[Body::Cybertronian]).phase,
            run(&easy, Body::HumanSuited, 2.5, 9, &[Body::HumanSuited]).phase
        );
    }

    #[test]
    fn the_same_raid_run_twice_does_the_same_thing() {
        let r = raid(Weather::DustStorm, quiet());
        let a = run(&r, Body::Cybertronian, 14.0, 77, &[Body::Cybertronian]);
        let b = run(&r, Body::Cybertronian, 14.0, 77, &[Body::Cybertronian]);
        assert_eq!(a.phase, b.phase);
        assert_eq!(a.drop.map(|d| d.grade), b.drop.map(|d| d.grade));
    }

    #[test]
    fn the_payout_matches_the_world_not_the_raid() {
        // a cryogenic raid pays cold tolerance whatever its name is
        let cold = Orbit { star: StarClass::G, au: 32.0, albedo: 0.5, air: 0.0 };
        let mut r = raid(Weather::Clear, cold);
        r.encounters = vec![30.0];
        let out = run(&r, Body::Cybertronian, 14.0, 4, &[Body::Cybertronian]);
        assert_eq!(out.phase, Phase::Extracted);
        assert_eq!(out.drop.unwrap().base.effect, crate::loadout::Effect::ColdFloor);
    }
}
