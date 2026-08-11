//! RAID CONTENT — the nineteen wardens, their arenas, and the lock they are.
//!
//! Ported from the C engine's raid roster (assetkit `raids.rs`, the design
//! reference in `docs/RAIDS.md`) and the relic fiction in
//! `docs/plan/RAIDS_AND_RELICS.md`. Three rules carried over intact:
//!
//! **The mechanic is derived from the lore, not assigned.** Each boss records
//! the premise line its mechanic came from, so the reasoning stays
//! inspectable — disagree with a mapping and you can see exactly what drove
//! it. A walking reactor that must never stop is a GROUND fight; a drill
//! that leaves the floor is an AIR fight; a resonance that several bodies
//! make together or not at all is SYNC.
//!
//! **Every arena a raid references must exist.** The C header once pointed
//! two of three phases at maps that had never been defined; the test
//! `every_boss_arena_exists` is that bug made impossible here.
//!
//! **The roster is a lock, not a list.** The relic fiction's frame: none of
//! these machines were the greatest weapons ever built. They were wardens,
//! judges, archivists, seals — a distributed lock positioned around the
//! galaxy on purpose. Every raid victory recovers a memory shard and
//! weakens that lock, which is the hidden campaign (`Seal`) that makes
//! nineteen separate fights one long decision.

use crate::sun::{Orbit, StarClass};
use crate::survival::Weather;

// ---------------------------------------------------------------- mech ---

/// What an encounter asks of the player. Codes match the C engine's
/// `raid_enc_t.mech` (0..3) so the two implementations stay comparable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mech {
    /// clear every hostile in the zone — the null case the others are
    /// measured against
    Purge,
    /// anti-air: flyers are forced down and fight on foot
    Ground,
    /// elevated: reachable only by flight or grapple
    Air,
    /// plates need several units standing together
    Sync,
}

impl Mech {
    pub fn code(self) -> u8 {
        match self {
            Mech::Purge => 0,
            Mech::Ground => 1,
            Mech::Air => 2,
            Mech::Sync => 3,
        }
    }
    pub fn from_code(c: u8) -> Mech {
        match c {
            1 => Mech::Ground,
            2 => Mech::Air,
            3 => Mech::Sync,
            _ => Mech::Purge,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Mech::Purge => "PURGE",
            Mech::Ground => "GROUND",
            Mech::Air => "AIR",
            Mech::Sync => "SYNC",
        }
    }
    /// What it asks, in the words the HUD uses.
    pub fn asks(self) -> &'static str {
        match self {
            Mech::Purge => "clear every hostile in the zone",
            Mech::Ground => "the field slams flyers down — fight it on foot",
            Mech::Air => "the node is up high — fly or grapple to it",
            Mech::Sync => "hold the plates together — one unit is not enough",
        }
    }
    /// How many units a plate of this mechanic wants. The reason SYNC is the
    /// only mechanic a single strong unit cannot solve.
    pub fn min_units(self) -> u8 {
        if self == Mech::Sync {
            3
        } else {
            1
        }
    }
}

// --------------------------------------------------------------- arena ---

/// Somewhere a raid happens. Some arenas sit on patrol destinations (`via`
/// names the world you reach them from); the deep sites are bridge-only,
/// per the plan — "raid locations will be scattered ... very far outside
/// the solar system", reached by space bridge rather than cruise.
pub struct Arena {
    pub id: &'static str,
    pub name: &'static str,
    /// the patrol destination it hangs off, if any
    pub via: Option<&'static str>,
    /// drives the loot economy and the survival maths — an arena has a star
    /// and a distance the moment it exists, so `loot::demanded` works on it
    pub orbit: Orbit,
    /// the sky the raid clock runs against
    pub weather: Weather,
    pub note: &'static str,
}

/// The raid map pool. Five hang off patrol worlds so a patrol can FIND a
/// raid; five are deep sites the bridge reaches. `unicron` is where every
/// SYNC finale ends, because the plate mechanic wants the arena where the
/// horizon curves the wrong way.
pub const ARENAS: &[Arena] = &[
    Arena { id: "mars", name: "THE RUST LINE", via: Some("mars"),
        orbit: Orbit { star: StarClass::G, au: 1.52, albedo: 0.25, air: 0.01 },
        weather: Weather::DustStorm,
        note: "the extraction rig's graveyard; the dust never fully settles" },
    Arena { id: "pluto", name: "THE CONTAINMENT LINE", via: Some("pluto"),
        orbit: Orbit { star: StarClass::G, au: 39.5, albedo: 0.5, air: 0.0 },
        weather: Weather::Clear,
        note: "where the defense ring stood, and did not hold" },
    Arena { id: "cybertron", name: "THE SILENT FORGES", via: Some("cybertron"),
        orbit: Orbit { star: StarClass::K, au: 0.9, albedo: 0.3, air: 0.0 },
        weather: Weather::Clear,
        note: "a thousand cities gone quiet in one century" },
    Arena { id: "outer_rim", name: "THE KUIPER DRIFT", via: Some("outer_rim"),
        orbit: Orbit { star: StarClass::G, au: 44.0, albedo: 0.4, air: 0.0 },
        weather: Weather::Clear,
        note: "the warlord's tithe routes cross here; so do the wrecks" },
    Arena { id: "shield_edge", name: "THE BOUNDARY LATTICE", via: Some("shield_edge"),
        orbit: Orbit { star: StarClass::G, au: 55.0, albedo: 0.2, air: 0.0 },
        weather: Weather::Clear,
        note: "the coalition's wall; past it, the Hijacks are still gathering" },
    Arena { id: "kuiper", name: "THE SHEPHERD BRAID", via: None,
        orbit: Orbit { star: StarClass::K, au: 30.0, albedo: 0.55, air: 0.0 },
        weather: Weather::Clear,
        note: "shepherd moons and a frozen debris braid, off every chart" },
    Arena { id: "frostline", name: "FROSTLINE", via: None,
        orbit: Orbit { star: StarClass::M, au: 1.8, albedo: 0.6, air: 0.2 },
        weather: Weather::ThermalFront,
        note: "a fast-rotating ice world; the terminator sweep is the clock" },
    Arena { id: "emberpit", name: "THE EMBERPIT", via: None,
        orbit: Orbit { star: StarClass::K, au: 0.18, albedo: 0.1, air: 0.3 },
        weather: Weather::FlareFront,
        note: "a scorched shell around a restless dwarf; the sky is the enemy" },
    Arena { id: "trench", name: "THE DROWNED TRENCH", via: None,
        orbit: Orbit { star: StarClass::K, au: 0.45, albedo: 0.28, air: 0.85 },
        weather: Weather::GiantWave,
        note: "an ocean world; the water rises to meet whatever walks it" },
    Arena { id: "unicron", name: "THE DEVOURER'S MAW", via: None,
        orbit: Orbit { star: StarClass::M, au: 0.08, albedo: 0.05, air: 0.1 },
        weather: Weather::FlareFront,
        note: "the horizon curves the wrong way; every SYNC ends here" },
];

pub fn arena(id: &str) -> Option<&'static Arena> {
    ARENAS.iter().find(|a| a.id == id)
}

// ---------------------------------------------------------------- boss ---

/// One of the nineteen. `premise` is the lore line the mechanic was derived
/// from; `relic` is the codex entry a clear unlocks (original terse prose in
/// the style of the C engine's lore.c); `shard` is what this machine was
/// FOR — the memory-shard line that feeds the Seal's hidden campaign.
pub struct Boss {
    pub id: &'static str,
    pub name: &'static str,
    pub title: &'static str,
    pub mech: Mech,
    /// the finale arena its own fight belongs to
    pub arena: &'static str,
    /// the line of fiction that decided the mechanic
    pub premise: &'static str,
    /// how hard the thing at the end is, 0..3 — feeds `raid::Raid.boss_tier`
    /// and the loot grade
    pub tier: u8,
    pub relic: &'static [&'static str],
    pub shard: &'static str,
}

pub const BOSSES: &[Boss] = &[
    Boss { id: "speakerman", name: "Astro Armor Titan Speakerman", title: "The Last Resonance",
        mech: Mech::Sync, arena: "unicron", tier: 3,
        premise: "sings across half a galaxy; fractures planets by matching tectonic resonance",
        relic: &[
            "a walking observatory that made moons into transmitters.",
            "when the war infected the frequencies, the voice became",
            "the weapon. it still broadcasts. no fleet that answers returns.",
        ],
        shard: "it was not singing to anyone. it was holding a note." },
    Boss { id: "jhiaxus_omega", name: "Devastator Jhiaxus", title: "Prototype Omega",
        mech: Mech::Purge, arena: "kuiper", tier: 3,
        premise: "destroying one only caused it to rebuild itself",
        relic: &[
            "the architect thought individuals obsolete. his answer:",
            "a body whose organs are factories. it drifted unfinished",
            "into the dark, and has spent four million years finishing.",
        ],
        shard: "the foundries were not building it. they were reinforcing something." },
    Boss { id: "mechagodzilla", name: "Plasma's Mechagodzilla", title: "The Planet Burner",
        mech: Mech::Ground, arena: "emberpit", tier: 3,
        premise: "its spinal plasma spine could ignite gas giants",
        relic: &[
            "built around a captured shard of a stellar core. it never",
            "transformed; its body was the weapon. civilizations surrendered",
            "at the sight of its fins over the horizon. the star inside refuses to die.",
        ],
        shard: "the fire was never aimed outward. it was a furnace kept lit." },
    Boss { id: "cherno", name: "Cherno Alpha", title: "The Walking Reactor",
        mech: Mech::Ground, arena: "frostline", tier: 2,
        premise: "if it ever stops walking, the reactor reaches criticality",
        relic: &[
            "its armor is not armor. it is containment. every step",
            "relieves pressure, and the engineers removed the shutdown",
            "on purpose. millions of years later, it is still marching.",
        ],
        shard: "the march has a route. the route is a circle around something." },
    Boss { id: "gigadrill", name: "Gigadrill", title: "The Core Piercer",
        mech: Mech::Air, arena: "cybertron", tier: 2,
        premise: "it drilled through moons, worlds, dwarf planets",
        relic: &[
            "built to learn why planets have metallic hearts. some worlds",
            "screamed; some contained machinery older than cybertron.",
            "it vanished under a rogue planet and came back marked.",
        ],
        shard: "it stopped drilling down. the last bore was a way OUT for something." },
    Boss { id: "hardboil", name: "Hardboil", title: "The Pressure King",
        mech: Mech::Purge, arena: "trench", tier: 2,
        premise: "built for depths that crush ordinary alloys",
        relic: &[
            "gas-giant warfare: no missiles, only pressure. it compressed",
            "atmosphere into shells of neutron density; fleets vanished",
            "without explosions. its chambers still cycle. moons feel it.",
        ],
        shard: "the pressure was a hand. the hand was holding a lid shut." },
    Boss { id: "hussar", name: "Hussar Tank", title: "The Charge",
        mech: Mech::Ground, arena: "mars", tier: 1,
        premise: "it does not manoeuvre; it commits",
        relic: &[
            "built by an order of knights. it accelerates and does not",
            "stop; past a certain distance nothing survives the arrival.",
            "one destroyed an orbital ring by refusing to. its engines never cooled.",
        ],
        shard: "the charge was never at an enemy. it was AWAY from a place." },
    Boss { id: "judgementor", name: "Judgementor", title: "Executor of Empires",
        mech: Mech::Sync, arena: "unicron", tier: 3,
        premise: "it did not fight wars, it ended them",
        relic: &[
            "not a tank. a mobile courtroom. civilizations stood trial",
            "under its cathedral body, and its rail cannons were called",
            "sentences. ancient defendants still orbit as asteroid belts.",
        ],
        shard: "one verdict was never delivered. the case is still open." },
    Boss { id: "mantis", name: "Primitive Mantis Mech", title: "The First Hunter",
        mech: Mech::Purge, arena: "frostline", tier: 1,
        premise: "the oldest predator pattern Cybertron ever built",
        relic: &[
            "older than language, older than cities, older than elegant",
            "transformation. its nerves evolved rather than being engineered.",
            "it studies prey, learns habits, and returns centuries later.",
        ],
        shard: "it was not hunting. it was patrolling a perimeter that predates it." },
    Boss { id: "pyrachnid", name: "Pyrachnid", title: "Forge Widow",
        mech: Mech::Sync, arena: "unicron", tier: 2,
        premise: "it spins its own foundry around its prey",
        relic: &[
            "born inside stellar furnaces. the web is superconducting",
            "plasma; battleships are caught, then slowly melted. forge",
            "worlds called it sacred — every web made alloys known nowhere else.",
        ],
        shard: "the web is a weave. the weave is a patch over a tear." },
    Boss { id: "scorpion", name: "Scorpion Commander", title: "The Frontier Marshal",
        mech: Mech::Ground, arena: "mars", tier: 1,
        premise: "held a border that no longer exists",
        relic: &[
            "not a soldier; a colonizer. its tail deployed factories,",
            "grids, terraformers — every claw a city, every leg a road.",
            "abandoned colonies still wait for orders from a marshal presumed dead.",
        ],
        shard: "the border it held was not around territory. it was around a site." },
    Boss { id: "starlord", name: "Starlord", title: "Twin Hammers",
        mech: Mech::Air, arena: "pluto", tier: 1,
        premise: "two hammers that are sentry guns",
        relic: &[
            "human-sized; almost overlooked. the ceremonial hammers unfold",
            "into autonomous fortresses — batteries, arrays, nests, towers —",
            "and no one knows whether they protect their bearer or imprison it.",
        ],
        shard: "the hammers do not answer to it. they answer to the same thing it does." },
    Boss { id: "strider", name: "Strider Tank", title: "The Horizon Walker",
        mech: Mech::Air, arena: "outer_rim", tier: 2,
        premise: "it walks toward the horizon and the horizon moves",
        relic: &[
            "built for impossible terrain: ranges, trenches, asteroid",
            "fields. its stride predicts gravity seconds before it changes.",
            "explorers said: if a strider chose a destination, it would arrive.",
        ],
        shard: "it has a destination. it has been refusing to arrive for an age." },
    Boss { id: "tremor", name: "Tremor", title: "Planetbreaker",
        mech: Mech::Ground, arena: "cybertron", tier: 2,
        premise: "the ground is its weapon",
        relic: &[
            "not seismic — gravitational. singularity oscillators in the",
            "legs shake spacetime; quakes are a side effect. satellites",
            "fall slowly. stars wobble. its heartbeat carries light-years.",
        ],
        shard: "the heartbeat is a countermeasure. it is jamming a deeper signal." },
    Boss { id: "trenchmaster", name: "Trenchmaster", title: "The Ocean King",
        mech: Mech::Sync, arena: "unicron", tier: 3,
        premise: "the ocean king; the water rises to meet it",
        relic: &[
            "made for abyssal wars. it treats the deep as a highway and",
            "commands water by pressure resonance; continental shelves",
            "rose at its word. some sonar recordings continue today.",
        ],
        shard: "the flood was a moat. what it circled has no name in the record." },
    Boss { id: "sigillum", name: "Wuthering Waves Sigillum", title: "The Seal",
        mech: Mech::Purge, arena: "shield_edge", tier: 2,
        premise: "a seal is only a door that has not been opened",
        relic: &[
            "nobody knows what it is. it is not a warrior; it is a lock.",
            "the inscriptions repeat: THE SIGILLUM SHALL NEVER AWAKEN.",
            "every attack it makes looks like a defense of something behind it.",
        ],
        shard: "destroying it may not be victory. it may simply open the door." },
    Boss { id: "archivist", name: "Titan Cameraman 3.0 Armor", title: "Archivist",
        mech: Mech::Air, arena: "pluto", tier: 2,
        premise: "it records everything; it has never stopped",
        relic: &[
            "the camera titans were never soldiers. every photon they",
            "captured is stored in quantum crystal. this one remembers",
            "cybertron's birth. damaging its armor physically erases history.",
        ],
        shard: "the oldest frame in its memory is of the lock being built." },
    Boss { id: "observer", name: "Sphere Robot", title: "The Observer",
        mech: Mech::Purge, arena: "kuiper", tier: 1,
        premise: "it watches, and being watched changes what you do",
        relic: &[
            "found drifting between galaxies. no engines, no weapons, no",
            "visible sensors — and it always arrives before the event.",
            "its shell predates known matter. a probe, or an eye.",
        ],
        shard: "it does not watch events. it watches the thing that causes them." },
    Boss { id: "divergence", name: "Titan Cameraman 3.0 (What If...)", title: "The Divergence",
        mech: Mech::Sync, arena: "unicron", tier: 3,
        premise: "the recording and the event no longer agree",
        relic: &[
            "not from this timeline. its memory holds events that never",
            "happened: fallen primes who live, victors who lost. its armor",
            "phases, and pieces of other histories briefly become real.",
        ],
        shard: "in every timeline it remembers, the lock is in the same place." },
];

pub fn boss(id: &str) -> Option<&'static Boss> {
    BOSSES.iter().find(|b| b.id == id)
}

// ---------------------------------------------------------------- seal ---
//
// The hidden campaign. Across every relic the same glyph: THE NINTH FORGE
// SLEEPS. The machines were wardens — parts of a distributed lock — and
// every raid victory recovers a shard and weakens it. Nineteen victories is
// not a clean sweep; it is a decision the player made one raid at a time.

/// How awake the thing under the lock is.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum SealState {
    /// the lock holds; the glyph is just a glyph
    Dormant,
    /// enough wardens down that the pattern is visible
    Stirring,
    /// the lock is failing and the shards say so out loud
    Waking,
    /// every warden down. the ninth forge no longer sleeps
    Open,
}

impl SealState {
    pub fn label(self) -> &'static str {
        match self {
            SealState::Dormant => "THE NINTH FORGE SLEEPS",
            SealState::Stirring => "THE LOCK HAS A PATTERN",
            SealState::Waking => "THE LOCK IS FAILING",
            SealState::Open => "THE NINTH FORGE WAKES",
        }
    }
    /// The line the codex shows when the state is reached — the reveal,
    /// paced so the player earns it rather than being told it.
    pub fn reveal(self) -> &'static str {
        match self {
            SealState::Dormant => "the same glyph on every relic, older than the first transformer.",
            SealState::Stirring => "wardens, judges, archivists, seals. not weapons. POSITIONS.",
            SealState::Waking => "they were placed. the placing is a lock, and you are the key.",
            SealState::Open => "none of them were the greatest weapon ever built. the thing they held is.",
        }
    }
}

/// Shards recovered -> how far gone the lock is. Thresholds sit where the
/// fiction turns: a third of the wardens down is a pattern, two thirds is a
/// failure in progress, all nineteen is the door.
pub fn seal_state(shards: u32) -> SealState {
    match shards {
        0..=5 => SealState::Dormant,
        6..=12 => SealState::Stirring,
        13..=18 => SealState::Waking,
        _ => SealState::Open,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loot;
    use crate::patrol;

    #[test]
    fn nineteen_bosses_with_the_recorded_spread() {
        // the spread RAIDS.md documents: 5 PURGE, 5 GROUND, 4 AIR, 5 SYNC
        assert_eq!(BOSSES.len(), 19);
        let n = |m: Mech| BOSSES.iter().filter(|b| b.mech == m).count();
        assert_eq!(n(Mech::Purge), 5);
        assert_eq!(n(Mech::Ground), 5);
        assert_eq!(n(Mech::Air), 4);
        assert_eq!(n(Mech::Sync), 5);
    }

    #[test]
    fn every_boss_arena_exists() {
        // the C header's defect (phases loading maps that were never
        // defined), made structurally impossible
        for b in BOSSES {
            assert!(arena(b.arena).is_some(), "{} fights at unknown arena {}", b.id, b.arena);
        }
    }

    #[test]
    fn every_sync_boss_ends_at_unicron() {
        for b in BOSSES.iter().filter(|b| b.mech == Mech::Sync) {
            assert_eq!(b.arena, "unicron", "{} is SYNC and ends elsewhere", b.id);
        }
    }

    #[test]
    fn arenas_via_real_patrol_worlds() {
        // an arena either hangs off a destination that exists, or is a
        // bridge-only deep site — never a dangling reference
        let mut on_route = 0;
        for a in ARENAS {
            if let Some(v) = a.via {
                assert!(patrol::find(v).is_some(), "{} hangs off unknown world {v}", a.id);
                on_route += 1;
            }
        }
        assert!(on_route >= 4, "patrol should be able to FIND raids, not just be told");
        assert!(ARENAS.iter().any(|a| a.via.is_none()), "no deep sites — the bridge has nowhere to go");
    }

    #[test]
    fn every_arena_pays_something() {
        // an arena has a loot table the moment it has an orbit; this is
        // that promise checked rather than assumed
        for a in ARENAS {
            let _ = loot::demanded(&a.orbit); // total function; the check is that it types
            assert!(a.orbit.au > 0.0 && a.orbit.air >= 0.0, "{} has a nonsense orbit", a.id);
        }
    }

    #[test]
    fn a_boss_carries_its_reasoning() {
        for b in BOSSES {
            assert!(!b.premise.is_empty(), "{} has no premise — the mapping is uninspectable", b.id);
            assert!(b.relic.len() >= 2, "{} has no codex entry to win", b.id);
            assert!(!b.shard.is_empty(), "{} contributes nothing to the seal arc", b.id);
            assert!(b.tier <= 3);
            for l in b.relic {
                assert!(l.len() <= 78, "{}: relic line too long for the codex card: {l}", b.id);
            }
        }
        // sized as bosses: the top of the roster is actually the top
        assert!(BOSSES.iter().filter(|b| b.tier == 3).count() >= 4);
        assert!(BOSSES.iter().any(|b| b.tier <= 1), "no entry-tier raid to learn on");
    }

    #[test]
    fn mech_codes_roundtrip_the_c_engines_numbering() {
        // the C engine's `raid_enc_t.mech` is 0..3 in this order; the codes
        // are the correspondence, so layouts can be diffed across engines
        for (i, m) in [Mech::Purge, Mech::Ground, Mech::Air, Mech::Sync].iter().enumerate() {
            assert_eq!(m.code() as usize, i);
            assert_eq!(Mech::from_code(m.code()), *m);
        }
    }

    #[test]
    fn the_seal_opens_only_on_a_full_sweep() {
        assert_eq!(seal_state(0), SealState::Dormant);
        assert_eq!(seal_state(6), SealState::Stirring);
        assert_eq!(seal_state(13), SealState::Waking);
        assert_eq!(seal_state(18), SealState::Waking);
        assert_eq!(seal_state(19), SealState::Open);
        // ordered, so "later" comparisons read the way the fiction does
        assert!(SealState::Dormant < SealState::Open);
        // and the arc is monotone: recovering a shard never puts the
        // forge back to sleep
        let mut prev = seal_state(0);
        for s in 1..=19 {
            let now = seal_state(s);
            assert!(now >= prev, "shard {s} put the forge back to sleep");
            prev = now;
        }
    }
}
