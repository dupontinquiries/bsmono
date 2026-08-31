//! WHY IS ANYONE HERE?
//!
//! A generated planet is a natural landscape. The moment something is BUILT on
//! it, the build has to answer a question the landscape does not: why here,
//! and why this shape. Skip that and you get the failure every procedural
//! settlement generator has — buildings scattered by a noise function, which
//! reads as debris rather than as a place, because nothing about it is a
//! consequence of anything.
//!
//! So nothing here is rolled from a seed. A settlement is derived twice over:
//!
//! * **From the faction, what it is FOR.** The lore is unusually specific and
//!   does the work: the Hijacks "do not conquer, they gather data", which
//!   makes their installations instruments rather than forts, and instruments
//!   are placed by what they need to see. The Coalition holds ground. The
//!   Clans take what a world has. The Corrupted do not choose at all — they
//!   arrive at whatever drew them.
//! * **From the planet, what is POSSIBLE.** `survival.rs` already ladders
//!   shelter (None, Suit, Bunker, Surface, SkyCity), and a planet's pressure
//!   and insolation decide which rung a settlement must start at. An open-air
//!   camp on a 92-bar world is not a style choice, it is a contradiction —
//!   and one a generator produces happily unless the rung is COMPUTED.
//!
//! Purpose picks the roles and the grammar; the planet picks the rung and
//! overrides both where it must. That is where the variety comes from, and it
//! is why two factions on one world build differently and one faction builds
//! differently across two worlds — with no style table anywhere.
//!
//! ## What Spore's stages 3-4 actually got right
//!
//! Not the generator — the vocabulary. A Spore city reads as a place because
//! it is a SMALL SET OF ROLES arranged by a GRAMMAR with one consistent style,
//! rather than a large set of unique buildings. Six roles and four grammars
//! carry an enormous amount of apparent variety, because the eye reads the
//! ARRANGEMENT and not the inventory. That is the cheap half and the part
//! worth stealing.
//!
//! ## Scope, honestly
//!
//! This is the PLAN, not the buildings. It says that a Hijack survey post on a
//! thin-atmosphere world is nine dispersed instrument masts around one buried
//! core. Turning that into geometry someone can walk, drive and fly through is
//! a much larger piece of work against `world.rs`'s shape pipeline, and it is
//! the half that cannot be checked without a screen. The plan is the half that
//! can be made true, tested, and argued with first.

use crate::patrol::Faction;

/// WHO BUILT IT — and the correction that this needed.
///
/// The first version of this module took a `patrol::Faction`, which is the
/// SOLAR SYSTEM's four: Hijacks, Corrupted, Blackstar Clans, Coalition. That
/// is the wrong scope for a galaxy. The authored raid roster is a universe
/// that was populated and had a history long before Sol mattered — wardens
/// titled "Executor of Empires", "The Frontier Marshal", "Archivist", "The
/// Observer", one that "sings across half a galaxy", and relic text that says
/// plainly "civilizations surrendered" and "a mobile courtroom; civilizations
/// stood trial". Sol's factions are a local detail inside that.
///
/// So a builder is either one of Sol's factions or an outsider, and an
/// outsider is described by its POSTURE — what it does to places it arrives
/// at. Each posture below cites the authored material it comes from, because
/// inventing a species taxonomy would be exactly the invention this module
/// exists to avoid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builder {
    /// One of the solar system's own.
    Sol(Faction),
    Outsider(Posture),
}

/// What an outsider civilisation does where it lands. Derived from the
/// wardens already written, not invented.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Posture {
    /// They write it down. `Archivist`, `The Observer` — and the archive's
    /// own "every civilisation that finds this" framing.
    Recorders,
    /// They arrive to judge. `Judgementor`, "a mobile courtroom;
    /// civilizations stood trial"; `Executor of Empires`, "it did not fight
    /// wars, it ended them".
    Tribunal,
    /// They take the world apart for what is in it. `The Core Piercer`, "it
    /// drilled through moons, worlds, dwarf planets"; `Planetbreaker`.
    Harvesters,
    /// They build, endlessly. `The Silent Forges`, `Forge Widow`.
    Forgeworks,
    /// They hold a line at the edge of somewhere. `The Frontier Marshal`.
    Marchers,
}

impl Posture {
    pub fn label(self) -> &'static str {
        match self {
            Posture::Recorders => "recorders",
            Posture::Tribunal => "tribunal",
            Posture::Harvesters => "harvesters",
            Posture::Forgeworks => "forgeworks",
            Posture::Marchers => "marchers",
        }
    }
}
use crate::planetlook::{PlanetSpec, Surface};
use crate::survival::ShelterKind;

/// Why a settlement exists — the answer to "why here".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Purpose {
    /// Instruments pointed at something. The Hijacks' whole posture.
    Survey,
    /// Taking something out of the ground.
    Extraction,
    /// A stop on a route: fuel, repair, transfer.
    Waystation,
    /// Holding ground against someone.
    Garrison,
    /// Somewhere to survive a planet that is trying to kill you.
    Refuge,
    /// Built around something nobody understands, by people who did not
    /// choose to be near it.
    Anomaly,
}

impl Purpose {
    pub fn label(self) -> &'static str {
        match self {
            Purpose::Survey => "survey",
            Purpose::Extraction => "extraction",
            Purpose::Waystation => "waystation",
            Purpose::Garrison => "garrison",
            Purpose::Refuge => "refuge",
            Purpose::Anomaly => "anomaly",
        }
    }
}

/// How the pieces are arranged. The arrangement carries the character.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Grammar {
    /// Everything faces a centre. Reads as authority.
    Radial,
    /// Strung along a feature — a ridge, a fault, a shore. Reads as
    /// following something.
    Linear,
    /// Spread wide with sightlines between. Reads as instruments, not homes.
    Dispersed,
    /// Packed behind one wall. Reads as fear.
    Warren,
}

impl Grammar {
    pub fn label(self) -> &'static str {
        match self {
            Grammar::Radial => "radial",
            Grammar::Linear => "linear",
            Grammar::Dispersed => "dispersed",
            Grammar::Warren => "warren",
        }
    }
}

/// The building vocabulary. Deliberately small — see the module doc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    /// The thing everything else is arranged around.
    Core,
    /// Where people are when they are not working.
    Habitat,
    /// The reason the place exists, made physical: a rig, a mast, a gun.
    Works,
    /// What keeps it running: power, air, water.
    Plant,
    /// Where things arrive and leave.
    Dock,
    /// What stops someone walking in.
    Ward,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Core => "core",
            Role::Habitat => "habitat",
            Role::Works => "works",
            Role::Plant => "plant",
            Role::Dock => "dock",
            Role::Ward => "ward",
        }
    }
}

/// A settlement, planned.
#[derive(Clone, Debug, PartialEq)]
pub struct Settlement {
    pub faction: Faction,
    pub purpose: Purpose,
    /// The lowest shelter rung this planet permits.
    pub shelter: ShelterKind,
    pub grammar: Grammar,
    /// Roles and how many, largest first.
    pub roles: Vec<(Role, u32)>,
}

impl Settlement {
    pub fn count(&self, r: Role) -> u32 {
        self.roles.iter().find(|(x, _)| *x == r).map(|(_, n)| *n).unwrap_or(0)
    }
    pub fn total(&self) -> u32 {
        self.roles.iter().map(|(_, n)| n).sum()
    }
    pub fn report(&self) -> String {
        format!(
            "{:?} {} / {} / {:?}: {}",
            self.faction,
            self.purpose.label(),
            self.grammar.label(),
            self.shelter,
            self.roles
                .iter()
                .map(|(r, n)| format!("{}x{}", n, r.label()))
                .collect::<Vec<_>>()
                .join(" ")
        )
    }
}

/// The lowest shelter a planet permits.
///
/// Not a style choice. Pressure that would crush, or none at all, decides
/// this before anyone picks an aesthetic — and a generator that does not
/// compute it will cheerfully put an open-air camp on Venus.
pub fn shelter_for(p: &PlanetSpec) -> ShelterKind {
    let bar = p.atmosphere_bar;
    if bar > 20.0 {
        // a crushing atmosphere is survivable at ALTITUDE long before it is
        // survivable at the surface — the one case where up is the answer
        ShelterKind::SkyCity
    } else if bar < 0.05 || p.insolation > 4.0 || p.insolation < 0.02 {
        // vacuum, thin air, a scouring sun, or the cold at the system's edge:
        // walls are not enough, but the ground is
        ShelterKind::Bunker
    } else {
        ShelterKind::Surface
    }
}

/// True where the settlement has to manufacture its own atmosphere.
fn sealed(s: ShelterKind) -> bool {
    matches!(s, ShelterKind::Bunker | ShelterKind::SkyCity)
}

/// What a faction is doing on this particular world.
///
/// Lore first, environment as the tiebreak. The Hijacks are the clearest case
/// and the one the codex is explicit about: they gather data, so they survey —
/// always, everywhere. What changes between worlds is what they are pointed
/// at, not why they came.
pub fn purpose_for_builder(b: Builder, p: &PlanetSpec) -> Purpose {
    match b {
        Builder::Sol(f) => purpose_for(f, p),
        Builder::Outsider(Posture::Recorders) => Purpose::Survey,
        Builder::Outsider(Posture::Tribunal) => Purpose::Garrison,
        Builder::Outsider(Posture::Harvesters) => Purpose::Extraction,
        // a forgeworld makes things wherever it is; the world is feedstock
        Builder::Outsider(Posture::Forgeworks) => Purpose::Extraction,
        Builder::Outsider(Posture::Marchers) => Purpose::Waystation,
    }
}

pub fn purpose_for(f: Faction, p: &PlanetSpec) -> Purpose {
    match f {
        Faction::Hijacks => Purpose::Survey,
        Faction::Corrupted => Purpose::Anomaly,
        Faction::BlackstarClans => {
            if matches!(p.surface, Surface::Basalt | Surface::Ferric | Surface::Regolith) {
                Purpose::Extraction
            } else {
                Purpose::Garrison
            }
        }
        Faction::Coalition => {
            // the Coalition holds ground — unless the world itself is the
            // enemy, in which case surviving it is the whole job
            let hostile = matches!(shelter_for(p), ShelterKind::SkyCity)
                || p.insolation > 4.0
                || p.insolation < 0.02;
            if hostile { Purpose::Refuge } else { Purpose::Garrison }
        }
        Faction::None => Purpose::Waystation,
    }
}

/// Plan a settlement for one of Sol's factions.
pub fn plan(f: Faction, p: &PlanetSpec) -> Settlement {
    plan_for(Builder::Sol(f), p)
}

/// Plan a settlement for anyone.
pub fn plan_for(b: Builder, p: &PlanetSpec) -> Settlement {
    let f = match b {
        Builder::Sol(f) => f,
        Builder::Outsider(_) => Faction::None,
    };
    let purpose = purpose_for_builder(b, p);
    let shelter = shelter_for(p);

    // Grammar follows PURPOSE, because that is what an arrangement expresses.
    // The one environmental override is the warren: when the planet is trying
    // to kill you, everything packs behind one wall whatever the place is for.
    let grammar = match purpose {
        Purpose::Survey => Grammar::Dispersed,
        Purpose::Extraction | Purpose::Waystation => Grammar::Linear,
        Purpose::Garrison | Purpose::Anomaly => Grammar::Radial,
        Purpose::Refuge => Grammar::Warren,
    };
    let grammar = if matches!(shelter, ShelterKind::SkyCity) { Grammar::Warren } else { grammar };

    // Counts are small on purpose: nine masts in a ring is a survey post,
    // ninety is a texture.
    let mut roles: Vec<(Role, u32)> = match purpose {
        Purpose::Survey => vec![(Role::Works, 9), (Role::Core, 1), (Role::Plant, 2)],
        Purpose::Extraction => {
            vec![(Role::Works, 6), (Role::Dock, 2), (Role::Habitat, 3), (Role::Plant, 2)]
        }
        Purpose::Waystation => vec![(Role::Dock, 4), (Role::Habitat, 2), (Role::Plant, 1)],
        Purpose::Garrison => {
            vec![(Role::Ward, 6), (Role::Core, 1), (Role::Habitat, 4), (Role::Plant, 2)]
        }
        Purpose::Refuge => vec![(Role::Habitat, 8), (Role::Plant, 3), (Role::Core, 1)],
        Purpose::Anomaly => vec![(Role::Core, 1), (Role::Works, 4), (Role::Habitat, 2)],
    };

    // A sealed world makes its own air, and that is a building rather than an
    // assumption. The environment has to reach the ROLES or "shelter" is a
    // label the settlement never acts on.
    if sealed(shelter) {
        match roles.iter_mut().find(|(r, _)| *r == Role::Plant) {
            Some((_, n)) => *n += 2,
            None => roles.push((Role::Plant, 2)),
        }
    }
    roles.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Settlement { faction: f, purpose, shelter, grammar, roles }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planetlook::SOL;
    use std::collections::HashSet;

    fn sol(name: &str) -> PlanetSpec {
        SOL.iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("no {name}")).1
    }

    const FACTIONS: [Faction; 4] =
        [Faction::Hijacks, Faction::Corrupted, Faction::BlackstarClans, Faction::Coalition];

    #[test]
    fn nobody_builds_an_open_air_camp_on_a_world_that_would_kill_them() {
        assert_eq!(shelter_for(&sol("VENUS")), ShelterKind::SkyCity, "92 bar at the surface");
        assert_eq!(shelter_for(&sol("LUNA")), ShelterKind::Bunker, "vacuum");
        assert_eq!(shelter_for(&sol("MARS")), ShelterKind::Bunker, "0.006 bar");
        assert_eq!(shelter_for(&sol("EARTH")), ShelterKind::Surface, "Earth allows walls");
        // and nowhere gets the two rungs that mean "not sheltered": a built
        // place is by definition more than a suit
        for (name, spec) in SOL {
            assert!(
                !matches!(shelter_for(spec), ShelterKind::None | ShelterKind::Suit),
                "{name} planned a settlement with no shelter"
            );
        }
    }

    #[test]
    fn two_factions_on_one_world_build_differently() {
        // If they do not, the faction is decoration. Earth allows anything,
        // so only doctrine can be doing the work here.
        let e = sol("EARTH");
        let plans: Vec<Settlement> = FACTIONS.iter().map(|f| plan(*f, &e)).collect();
        let distinct: HashSet<Purpose> = plans.iter().map(|p| p.purpose).collect();
        assert!(
            distinct.len() >= 3,
            "four factions produced {} distinct purposes on one world: {:?}",
            distinct.len(),
            plans.iter().map(|p| p.report()).collect::<Vec<_>>()
        );
        let h = plan(Faction::Hijacks, &e);
        assert_eq!(h.purpose, Purpose::Survey, "the Hijacks stopped gathering data");
        assert_eq!(h.grammar, Grammar::Dispersed, "instruments packed together see nothing");
    }

    #[test]
    fn one_faction_builds_differently_across_worlds() {
        // The complement: if the environment does nothing, the PLANET is the
        // decoration instead.
        let worlds = ["EARTH", "VENUS", "LUNA", "PLUTO"];
        let plans: Vec<Settlement> =
            worlds.iter().map(|w| plan(Faction::Coalition, &sol(w))).collect();
        let shelters: HashSet<&str> = plans
            .iter()
            .map(|p| match p.shelter {
                ShelterKind::SkyCity => "sky",
                ShelterKind::Bunker => "bunker",
                ShelterKind::Surface => "surface",
                _ => "none",
            })
            .collect();
        assert!(
            shelters.len() >= 2,
            "the Coalition built the same way everywhere: {:?}",
            plans.iter().map(|p| p.report()).collect::<Vec<_>>()
        );
        assert_eq!(
            plan(Faction::Coalition, &sol("VENUS")).grammar,
            Grammar::Warren,
            "a sky city sprawled"
        );
    }

    #[test]
    fn a_sealed_world_builds_its_own_air() {
        // The environment has to reach the ROLES, not just a label.
        let earth = plan(Faction::Coalition, &sol("EARTH"));
        let luna = plan(Faction::Coalition, &sol("LUNA"));
        assert!(
            luna.count(Role::Plant) > earth.count(Role::Plant),
            "a vacuum world planned no more life support than Earth: {} vs {}",
            luna.count(Role::Plant),
            earth.count(Role::Plant)
        );
    }

    #[test]
    fn every_plan_is_a_place_rather_than_a_pile() {
        // A plan with one building is not a settlement; one with a hundred is
        // texture nobody reads.
        for (name, spec) in SOL {
            for f in FACTIONS {
                let s = plan(f, spec);
                assert!(
                    (4..=40).contains(&s.total()),
                    "{name}/{f:?}: {} buildings — {}",
                    s.total(),
                    s.report()
                );
                assert!(s.roles.len() >= 3, "{name}/{f:?} used too few roles: {}", s.report());
                assert!(
                    s.roles.windows(2).all(|w| w[0].1 >= w[1].1),
                    "{name}/{f:?} roles are not ordered largest-first"
                );
            }
        }
    }

    const POSTURES: [Posture; 5] = [
        Posture::Recorders,
        Posture::Tribunal,
        Posture::Harvesters,
        Posture::Forgeworks,
        Posture::Marchers,
    ];

    #[test]
    fn outsiders_build_things_sols_factions_do_not() {
        // The correction this module needed. Sol's four are a local detail;
        // the authored raid roster is a populated universe with its own
        // history — "Executor of Empires", "Archivist", "civilizations stood
        // trial". If every outsider collapsed onto a Sol faction's plan, the
        // widening would be a type change and nothing else.
        let e = sol("EARTH");
        let sol_purposes: HashSet<Purpose> =
            FACTIONS.iter().map(|f| plan(*f, &e).purpose).collect();
        let out_purposes: HashSet<Purpose> = POSTURES
            .iter()
            .map(|p| plan_for(Builder::Outsider(*p), &e).purpose)
            .collect();
        assert!(
            out_purposes.iter().any(|p| !sol_purposes.contains(p)),
            "outsiders produced only purposes Sol already had: {out_purposes:?}"
        );
        // and they are not all the same as each other either
        assert!(out_purposes.len() >= 3, "five postures gave {} purposes", out_purposes.len());
    }

    #[test]
    fn the_environment_still_rules_an_outsider() {
        // Being from outside does not exempt you from physics: an outsider on
        // Venus still cannot pitch a camp at 92 bar. The two derivations have
        // to compose, or the widening quietly bypassed the planet.
        for p in POSTURES {
            let v = plan_for(Builder::Outsider(p), &sol("VENUS"));
            assert_eq!(v.shelter, ShelterKind::SkyCity, "{}: 92 bar ignored", p.label());
            assert_eq!(v.grammar, Grammar::Warren, "{}: sprawled on Venus", p.label());
            let luna = plan_for(Builder::Outsider(p), &sol("LUNA"));
            let earth = plan_for(Builder::Outsider(p), &sol("EARTH"));
            assert!(
                luna.count(Role::Plant) > earth.count(Role::Plant),
                "{}: a vacuum world planned no extra life support",
                p.label()
            );
        }
    }

    #[test]
    fn every_outsider_plan_is_still_a_place() {
        for (name, spec) in SOL {
            for p in POSTURES {
                let s = plan_for(Builder::Outsider(p), spec);
                assert!(
                    (4..=40).contains(&s.total()),
                    "{name}/{}: {} buildings — {}",
                    p.label(),
                    s.total(),
                    s.report()
                );
                assert!(s.roles.len() >= 3, "{name}/{} used too few roles", p.label());
            }
        }
    }

    #[test]
    fn the_dominant_role_matches_what_the_place_is_for() {
        // The module's whole claim: a settlement's shape is a consequence of
        // its purpose. If the biggest thing about a survey post is not its
        // instruments, the derivation is decorative.
        let survey = plan(Faction::Hijacks, &sol("EARTH"));
        assert_eq!(survey.roles[0].0, Role::Works, "a survey post is not mostly instruments");
        let refuge = plan(Faction::Coalition, &sol("PLUTO"));
        assert_eq!(refuge.purpose, Purpose::Refuge, "Pluto is not hostile enough?");
        assert_eq!(refuge.roles[0].0, Role::Habitat, "a refuge is not mostly people");
        let garrison = plan(Faction::Coalition, &sol("EARTH"));
        assert_eq!(garrison.roles[0].0, Role::Ward, "a garrison is not mostly defences");
    }
}
