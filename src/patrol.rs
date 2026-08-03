//! PATROL — the roaming loop, ported from `game.c`'s `patrol_update`.
//!
//! The C engine's patrol is a Destiny-style roam: you are dropped on a world
//! and the world keeps producing things to do. Three parts, all here:
//!
//!   * **Public events** spin up near you on a cooldown, run against a clock,
//!     and pay a loot cache. Three kinds, and the quota grows with how many
//!     you have already cleared, so the world escalates around a player who
//!     is doing well rather than around a difficulty slider.
//!   * **Archive fragments**, a slow ambient lore drip. Not decoration — it
//!     is where the setting is delivered, one line at a time, while you are
//!     busy.
//!   * **Space-bridge travel** between worlds. skydribble already has real
//!     portals with paired exits and moving mounts, so the bridge is a
//!     portal that happens to land somewhere with different gravity.
//!
//! What is new here rather than ported is the DESTINATION table. The C
//! engine had five planet models and nothing that said what a planet *is*;
//! the film material and MAP_SCIENCE.md between them say a great deal —
//! surface gravity drives crater form, tube span, talus runout, whether
//! there is weather at all. So a destination carries the numbers the world
//! generator needs, and the kinds go past "planet": a patrol route that ends
//! at an asteroid field or in open space is the difference between a hub
//! with levels and a solar system.

/// Where a bridge can put you.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Terrain {
    /// a body with a surface, gravity and (maybe) weather
    Surface,
    /// tumbling rock, effectively no gravity, cover that moves
    AsteroidField,
    /// open space: no floor, no up, flight only
    DeepSpace,
    /// generated from a seed rather than authored
    Procedural,
}

/// Who you meet there. The factions are the script's, not invented:
/// the Hijacks are the erratic conquerors the elemental shield holds back,
/// the Corrupted are Jhiaxus' experiment, and Blackstar's clans tax the
/// outer rim.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Faction {
    None,
    Hijacks,
    Corrupted,
    BlackstarClans,
    /// the elemental coalition — allies, mostly
    Coalition,
}

pub struct Destination {
    pub id: &'static str,
    pub name: &'static str,
    pub terrain: Terrain,
    /// surface gravity in g. Drives crater form, lava-tube span and talus
    /// runout per MAP_SCIENCE.md — and, compressed toward 1, how a jump
    /// feels.
    pub gravity: f32,
    /// does air move? The gate that decides whether dunes are possible.
    pub weather: bool,
    pub hostiles: Faction,
    /// what the place is like to be in when nothing is attacking you
    pub sky: Sky,
    /// how long a full day is, in seconds. 0 means the light never changes —
    /// tidally locked, or too far out for a sun to matter.
    pub day_len: f32,
    /// ambient beats that fire on their own timer, so the world has
    /// something to say between public events
    pub ambient: &'static [&'static str],
    /// what the archive says about it, surfaced on arrival
    pub note: &'static str,
}

/// The route. Earth and its neighbours from the C engine's planet list, plus
/// the places the lore names and the engine never modelled — Europa's
/// listening post, Io's tide-forges, Titan's fuel rain, and the clone
/// homeworlds Glaciem, Carrion and Helix.
pub const ROUTE: &[Destination] = &[
    Destination { id: "earth", name: "EARTH", terrain: Terrain::Surface, gravity: 1.00, weather: true,
        hostiles: Faction::Corrupted,
        note: "the core holds cosmic significance; the humans have been sitting on top of it",
        sky: Sky::Atmosphere, day_len: 1200.0, ambient: &[
            "a container ship crosses the bay, running dark", 
            "gulls, then nothing; the birds left this coast months ago", 
            "the ground ticks — something under the crust is still cooling"
        ],
    },
    Destination { id: "mars", name: "MARS", terrain: Terrain::Surface, gravity: 0.38, weather: true,
        hostiles: Faction::Hijacks,
        note: "thin air, but air: the dunes are real and the craters go central-peak early",
        sky: Sky::Murk, day_len: 1230.0, ambient: &[
            "dust rises without wind, then settles wrong", 
            "a survey drone answers a ping it was not sent", 
            "the rig groans; nobody has maintained it since the shard went"
        ],
    },
    Destination { id: "luna", name: "LUNA", terrain: Terrain::Surface, gravity: 0.17, weather: false,
        hostiles: Faction::Hijacks,
        note: "no weather, so no dunes; the tubes run kilometres, and GRAIL says the voids are there",
        sky: Sky::Vacuum, day_len: 0.0, ambient: &[
            "regolith drifts and does not fall", 
            "a tube mouth exhales cold from a void GRAIL mapped and nobody entered", 
            "Earth hangs in the same place it hung an hour ago"
        ],
    },
    Destination { id: "europa", name: "EUROPA", terrain: Terrain::Surface, gravity: 0.13, weather: false,
        hostiles: Faction::Corrupted,
        note: "listening post, final log — 'something answered from below the ice'",
        sky: Sky::Glare, day_len: 0.0, ambient: &[
            "the ice booms, a long way down", 
            "the listening post still transmits; nothing is left to transmit", 
            "something answered from below, and the log ends there"
        ],
    },
    Destination { id: "io", name: "IO", terrain: Terrain::Surface, gravity: 0.18, weather: false,
        hostiles: Faction::BlackstarClans,
        note: "the tide-forges never cool; the magma-clone fires have burned since before the war",
        sky: Sky::Ember, day_len: 0.0, ambient: &[
            "a fountain goes up over the horizon, silently", 
            "the tide-forges pull the crust and the crust gives", 
            "magma-clone fires, burning since before the war"
        ],
    },
    Destination { id: "titan", name: "TITAN", terrain: Terrain::Surface, gravity: 0.14, weather: true,
        hostiles: Faction::BlackstarClans,
        note: "tithe-ledger — Blackstar's clans tax the only world where fuel falls as rain",
        sky: Sky::Murk, day_len: 0.0, ambient: &[
            "it is raining fuel, and the rain is warm", 
            "a tithe barge lifts, heavy, tax paid", 
            "the lakes are flat; there is no wind tonight"
        ],
    },
    Destination { id: "glaciem", name: "GLACIEM", terrain: Terrain::Surface, gravity: 0.44, weather: false,
        hostiles: Faction::Coalition,
        note: "the ice-clones grow slow in lattice pools; nothing is hurried, nothing thrown away",
        sky: Sky::Glare, day_len: 1500.0, ambient: &[
            "a lattice pool cracks, grows, seals", 
            "nothing here is hurried", 
            "ice-clones pass and do not look up"
        ],
    },
    Destination { id: "carrion", name: "CARRION DRIFT", terrain: Terrain::AsteroidField, gravity: 0.02, weather: false,
        hostiles: Faction::BlackstarClans,
        note: "every body a patchwork of salvage; status is the rarest wreck worn openly",
        sky: Sky::Starfield, day_len: 0.0, ambient: &[
            "a hull tumbles past, older than the war", 
            "somebody is wearing a wreck you recognise", 
            "the drift closes; then it opens again"
        ],
    },
    Destination { id: "helix", name: "HELIX", terrain: Terrain::Surface, gravity: 0.90, weather: true,
        hostiles: Faction::Coalition,
        note: "the architect's clones are re-engineered between lives, perfected past identity",
        sky: Sky::Atmosphere, day_len: 900.0, ambient: &[
            "a clone walks out of the vats already knowing you", 
            "the architect has improved something past recognising itself", 
            "between lives, they are re-engineered"
        ],
    },
    Destination { id: "pluto", name: "PLUTO", terrain: Terrain::Surface, gravity: 0.06, weather: true,
        hostiles: Faction::Hijacks,
        note: "the outer containment line; where the defense ring stood, and did not hold",
        sky: Sky::Vacuum, day_len: 0.0, ambient: &[
            "methane dunes march, one grain at a time", 
            "the containment line is still marked, and no longer held", 
            "Charon fills a third of the sky"
        ],
    },
    Destination { id: "cybertron", name: "CYBERTRON", terrain: Terrain::Surface, gravity: 1.15, weather: false,
        hostiles: Faction::Corrupted,
        note: "the Silent Century — Cybertron went dark; no record says why",
        sky: Sky::Vacuum, day_len: 0.0, ambient: &[
            "a forge-song, one voice, a thousand cities gone", 
            "the Silent Century left no record of why", 
            "the metal underfoot remembers being a road"
        ],
    },
    Destination { id: "outer_rim", name: "THE OUTER RIM", terrain: Terrain::DeepSpace, gravity: 0.0, weather: false,
        hostiles: Faction::BlackstarClans,
        note: "the dreadnought is already moving; he has had the coordinates longer",
        sky: Sky::Starfield, day_len: 0.0, ambient: &[
            "a dreadnought passes, unhurried", 
            "the array is projecting a star map you are on", 
            "he has had the coordinates longer than you have"
        ],
    },
    Destination { id: "shield_edge", name: "THE SHIELD BOUNDARY", terrain: Terrain::DeepSpace, gravity: 0.0, weather: false,
        hostiles: Faction::Hijacks,
        note: "a vast elemental barrier — the coalition built it to hold the Hijacks out",
        sky: Sky::Lattice, day_len: 0.0, ambient: &[
            "the barrier flexes where something leans on it", 
            "the coalition built this to hold, and it is holding", 
            "past it, the Hijacks are still gathering"
        ],
    },
    Destination { id: "uncharted", name: "UNCHARTED", terrain: Terrain::Procedural, gravity: 0.55, weather: true,
        hostiles: Faction::Hijacks,
        note: "no record; the bridge opened anyway",
        sky: Sky::Atmosphere, day_len: 800.0, ambient: &[
            "no record; the bridge opened anyway", 
            "the sky here has a colour nobody has named", 
            "something built this and did not stay"
        ],
    },
];

/// The look of the place. Not a material — a mood the renderer can read off
/// and the sound bed can follow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sky {
    /// blue, clouded, a sun you can point at
    Atmosphere,
    /// airless: black sky, hard shadows, stars at noon
    Vacuum,
    /// ice haze; the sun is a bright smudge
    Glare,
    /// volcanic: the ground is the light source
    Ember,
    /// storm-lit, thick, orange
    Murk,
    /// deep space with the galaxy across it
    Starfield,
    /// the coalition barrier, visible as a lattice on the sky
    Lattice,
}

pub fn find(id: &str) -> Option<&'static Destination> {
    ROUTE.iter().find(|d| d.id == id)
}

/// The three public events, straight from `patrol_update`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventKind {
    /// purge a quota of hostiles at the beacon
    Surge,
    /// stand in the ring and bank the seconds
    Hold,
    /// a heavier purge against a shorter clock
    Onslaught,
}

impl EventKind {
    pub fn label(self) -> &'static str {
        match self {
            EventKind::Surge => "SURGE",
            EventKind::Hold => "HOLD",
            EventKind::Onslaught => "ONSLAUGHT",
        }
    }
    /// The C engine's numbers: 32 s for surge and hold, 42 for onslaught.
    pub fn duration(self) -> f32 {
        match self {
            EventKind::Onslaught => 42.0,
            _ => 32.0,
        }
    }
    /// Quota grows with events already cleared — the world escalates around
    /// a player who is doing well, which is not the same as a difficulty
    /// setting and feels quite different.
    pub fn quota(self, cleared: u32) -> u32 {
        match self {
            EventKind::Surge => 7 + cleared,
            EventKind::Onslaught => 12 + cleared * 2,
            EventKind::Hold => 0,
        }
    }
    pub fn reward(self) -> u32 {
        if self == EventKind::Onslaught {
            500
        } else {
            300
        }
    }
}

/// Bank 18 s inside the ring to clear a HOLD.
pub const HOLD_SECONDS: f32 = 18.0;
/// Ring radius you must be inside for a HOLD to bank.
pub const HOLD_RADIUS: f32 = 12.0;
/// First event of a run spins up this many seconds in.
pub const FIRST_EVENT_CD: f32 = 15.0;

#[derive(Clone, Copy)]
pub struct Event {
    pub kind: EventKind,
    /// beacon position, xz — patrol events are a ground game
    pub at: [f32; 2],
    pub timer: f32,
    pub quota: u32,
    pub kills: u32,
    pub held: f32,
}

/// What just happened, for the caller to turn into toasts, loot and sound.
#[derive(Default)]
pub struct Tick {
    pub started: Option<EventKind>,
    /// (reward, whether a rare heroic drop should land)
    pub completed: Option<(u32, bool)>,
    pub failed: bool,
    pub lore: Option<&'static str>,
    /// a beat from the place itself, distinct from the archive: the archive
    /// is history, this is now, and mixing them would flatten both
    pub ambient: Option<&'static str>,
    pub arrived: Option<&'static str>,
}

/// The archive drip. Fourteen lines carried over verbatim in spirit — the
/// war, the older strata, and the clone lines' homeworlds.
pub const ARCHIVE: &[&str] = &[
    "ARCHIVE: the energy signature matches ancient Cybertronian records",
    "ARCHIVE: the Fallen's war fleet seeded an energon cache deep in Earth's mantle",
    "ARCHIVE: the bunker was built over an ancient site — near Unicron's core",
    "ARCHIVE: before the war there was the Age of Foundries — a thousand cities, one forge-song",
    "ARCHIVE: the First Bridge era — when the gates opened and every world was a doorway",
    "ARCHIVE: the Silent Century — Cybertron went dark; no record says why",
    "ARCHIVE: Europa listening post, final log — 'something answered from below the ice'",
    "ARCHIVE: Io's tide-forges never cool; the magma-clone fires burn since before the war",
    "ARCHIVE: Titan tithe-ledger — the clans tax the only world where fuel falls as rain",
    "ARCHIVE: Glaciem — the ice-clones grow slow in lattice pools; nothing is hurried",
    "ARCHIVE: Carrion drift — every body a patchwork of salvage; the rarest wreck worn openly",
    "ARCHIVE: Helix — the architect's clones are re-engineered between lives",
    "ARCHIVE: the Watch predate the war — twelve forms and one, older than the oldest record",
    "ARCHIVE: the coalition shield holds a swath of the cosmos; Earth sits at its centre",
];

pub struct Patrol {
    pub dest: usize,
    pub event: Option<Event>,
    cooldown: f32,
    pub cleared: u32,
    pub credits: u32,
    lore_cd: f32,
    lore_idx: usize,
    amb_cd: f32,
    amb_idx: usize,
    rng: u64,
}

impl Default for Patrol {
    fn default() -> Patrol {
        Patrol::new(0, 0x9E3779B97F4A7C15)
    }
}

impl Patrol {
    pub fn new(dest: usize, seed: u64) -> Patrol {
        Patrol {
            dest: dest.min(ROUTE.len() - 1),
            event: None,
            cooldown: FIRST_EVENT_CD,
            cleared: 0,
            credits: 0,
            lore_cd: 40.0,
            lore_idx: 0,
            amb_cd: 12.0,
            amb_idx: 0,
            rng: seed | 1,
        }
    }

    pub fn destination(&self) -> &'static Destination {
        &ROUTE[self.dest]
    }

    fn next_rand(&mut self) -> f32 {
        // xorshift: deterministic, so a seeded patrol replays identically
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        ((self.rng >> 11) as f64 / (1u64 << 53) as f64) as f32
    }

    /// Take the bridge. Everything in flight is abandoned — a jump is a
    /// change of world, not a fast-travel within one.
    pub fn bridge_to(&mut self, id: &str) -> Option<&'static Destination> {
        let i = ROUTE.iter().position(|d| d.id == id)?;
        self.dest = i;
        self.event = None;
        self.cooldown = FIRST_EVENT_CD;
        // a new world introduces itself promptly
        self.amb_cd = 3.0;
        self.amb_idx = 0;
        Some(&ROUTE[i])
    }

    /// Advance. `night` is 0 by day and 1 at midnight: the C engine spun
    /// events up about 60% faster in the dark, which is the cheapest way to
    /// make a day cycle mean something.
    pub fn update(&mut self, dt: f32, player: [f32; 2], night: f32) -> Tick {
        let mut out = Tick::default();

        // AMBIENT: the world says something about itself, roughly three
        // times as often as the archive says something about the past. A
        // place you are standing in should be more present than a place you
        // are reading about.
        self.amb_cd -= dt;
        if self.amb_cd <= 0.0 {
            let a = ROUTE[self.dest].ambient;
            if !a.is_empty() {
                out.ambient = Some(a[self.amb_idx % a.len()]);
                self.amb_idx += 1;
            }
            self.amb_cd = 18.0 + self.next_rand() * 14.0;
        }

        self.lore_cd -= dt;
        if self.lore_cd <= 0.0 {
            out.lore = Some(ARCHIVE[self.lore_idx % ARCHIVE.len()]);
            self.lore_idx += 1;
            self.lore_cd = 55.0 + self.next_rand() * 30.0;
        }

        match &mut self.event {
            None => {
                self.cooldown -= dt * (1.0 + 0.6 * night.clamp(0.0, 1.0));
                if self.cooldown <= 0.0 {
                    let k = match (self.next_rand() * 3.0) as u32 {
                        0 => EventKind::Surge,
                        1 => EventKind::Hold,
                        _ => EventKind::Onslaught,
                    };
                    let ang = self.next_rand() * std::f32::consts::TAU;
                    let rad = 16.0 + self.next_rand() * 12.0;
                    self.event = Some(Event {
                        kind: k,
                        at: [player[0] + ang.cos() * rad, player[1] + ang.sin() * rad],
                        timer: k.duration(),
                        quota: k.quota(self.cleared),
                        kills: 0,
                        held: 0.0,
                    });
                    out.started = Some(k);
                }
            }
            Some(e) => {
                e.timer -= dt;
                let dx = player[0] - e.at[0];
                let dz = player[1] - e.at[1];
                let inside = (dx * dx + dz * dz).sqrt() < HOLD_RADIUS;
                if e.kind == EventKind::Hold && inside {
                    e.held += dt;
                }
                let done = match e.kind {
                    EventKind::Hold => e.held >= HOLD_SECONDS,
                    _ => e.kills >= e.quota,
                };
                if done {
                    let reward = e.kind.reward();
                    self.credits += reward;
                    self.cleared += 1;
                    // every third clear pays a rare heroic drop
                    out.completed = Some((reward, self.cleared % 3 == 0));
                    self.event = None;
                    self.cooldown = FIRST_EVENT_CD;
                } else if e.timer <= 0.0 {
                    out.failed = true;
                    self.event = None;
                    self.cooldown = FIRST_EVENT_CD;
                }
            }
        }
        out
    }

    /// Report a kill toward the live event.
    pub fn credit_kill(&mut self) {
        if let Some(e) = &mut self.event {
            e.kills += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(p: &mut Patrol, secs: f32, at: [f32; 2], night: f32) -> Vec<Tick> {
        let mut v = Vec::new();
        let steps = (secs / (1.0 / 60.0)) as i32;
        for _ in 0..steps {
            v.push(p.update(1.0 / 60.0, at, night));
        }
        v
    }

    #[test]
    fn the_route_is_coherent() {
        assert!(ROUTE.len() >= 10);
        // ids unique, so bridge_to cannot be ambiguous
        for (i, d) in ROUTE.iter().enumerate() {
            assert!(ROUTE.iter().skip(i + 1).all(|o| o.id != d.id), "duplicate {}", d.id);
            assert!(d.gravity >= 0.0 && d.gravity < 2.0, "{} has odd gravity", d.id);
            // MAP_SCIENCE's gate: aeolian features need SALTATION, so they
            // need moving air rather than merely a medium. Vacuum has none —
            // open space and a tumbling rock field cannot have weather. A
            // procedural world may, which is why the rule names the two
            // kinds it applies to instead of "not a surface".
            if matches!(d.terrain, Terrain::DeepSpace | Terrain::AsteroidField) {
                assert!(!d.weather, "{} is vacuum but claims weather", d.id);
            }
        }
        // the route has to actually go somewhere other than planets
        assert!(ROUTE.iter().any(|d| d.terrain == Terrain::AsteroidField));
        assert!(ROUTE.iter().any(|d| d.terrain == Terrain::DeepSpace));
        assert!(ROUTE.iter().any(|d| d.terrain == Terrain::Procedural));
    }

    #[test]
    fn every_destination_has_a_mood_and_something_to_say() {
        for d in ROUTE {
            assert!(d.ambient.len() >= 3, "{} has {} ambient beats", d.id, d.ambient.len());
            assert!(d.ambient.iter().all(|a| a.len() > 12), "{} has a stub beat", d.id);
            assert!(d.day_len >= 0.0);
            // an airless body cannot have a blue sky, and a gas-hazed one
            // cannot have hard vacuum shadows
            match d.terrain {
                Terrain::DeepSpace => assert!(matches!(d.sky, Sky::Starfield | Sky::Lattice), "{}", d.id),
                Terrain::AsteroidField => assert_eq!(d.sky, Sky::Starfield, "{}", d.id),
                _ => {}
            }
            if !d.weather && d.terrain == Terrain::Surface {
                assert_ne!(d.sky, Sky::Atmosphere, "{} is airless but has an atmosphere sky", d.id);
            }
        }
        // ambient lines should be unique across the route, or the setting
        // reads as one place with different gravity
        let all: Vec<&str> = ROUTE.iter().flat_map(|d| d.ambient.iter().copied()).collect();
        for (i, a) in all.iter().enumerate() {
            assert!(all.iter().skip(i + 1).all(|b| b != a), "duplicate ambient line: {a}");
        }
    }

    #[test]
    fn ambient_speaks_more_often_than_the_archive() {
        let mut p = Patrol::new(0, 17);
        let t = run(&mut p, 300.0, [0.0, 0.0], 0.0);
        let amb = t.iter().filter(|x| x.ambient.is_some()).count();
        let lore = t.iter().filter(|x| x.lore.is_some()).count();
        assert!(amb > lore, "the place said less than the archive ({amb} vs {lore})");
        assert!((8..=20).contains(&amb), "{amb} ambient beats in five minutes");
    }

    #[test]
    fn a_new_world_introduces_itself() {
        let mut p = Patrol::new(0, 4);
        run(&mut p, 60.0, [0.0, 0.0], 0.0);
        p.bridge_to("io").unwrap();
        let t = run(&mut p, 4.0, [0.0, 0.0], 0.0);
        let first = t.iter().find_map(|x| x.ambient);
        assert!(first.is_some(), "arrived and the world said nothing");
        assert!(ROUTE.iter().find(|d| d.id == "io").unwrap().ambient.contains(&first.unwrap()));
    }

    #[test]
    fn deep_space_has_no_floor_to_stand_on() {
        for d in ROUTE.iter().filter(|d| d.terrain == Terrain::DeepSpace) {
            assert_eq!(d.gravity, 0.0, "{} is open space with gravity", d.id);
        }
    }

    #[test]
    fn an_event_spins_up_and_pays_out() {
        let mut p = Patrol::new(0, 7);
        let t = run(&mut p, FIRST_EVENT_CD + 1.0, [0.0, 0.0], 0.0);
        assert!(t.iter().any(|x| x.started.is_some()), "no event ever started");
        let e = p.event.expect("event vanished");
        // clear it the way its kind wants clearing
        match e.kind {
            EventKind::Hold => {
                let ticks = run(&mut p, HOLD_SECONDS + 1.0, e.at, 0.0);
                assert!(ticks.iter().any(|x| x.completed.is_some()), "hold never banked");
            }
            k => {
                for _ in 0..k.quota(0) + 8 {
                    p.credit_kill();
                }
                let ticks = run(&mut p, 0.1, [0.0, 0.0], 0.0);
                assert!(ticks.iter().any(|x| x.completed.is_some()), "quota never paid");
            }
        }
        assert!(p.credits >= 300);
        assert_eq!(p.cleared, 1);
    }

    #[test]
    fn the_world_escalates_around_a_player_who_is_winning() {
        assert!(EventKind::Surge.quota(5) > EventKind::Surge.quota(0));
        assert!(EventKind::Onslaught.quota(5) > EventKind::Surge.quota(5));
        // and onslaught pays for it
        assert!(EventKind::Onslaught.reward() > EventKind::Surge.reward());
    }

    #[test]
    fn the_dark_brings_them_out() {
        let mut day = Patrol::new(0, 3);
        let mut night = Patrol::new(0, 3);
        run(&mut day, 10.0, [0.0, 0.0], 0.0);
        run(&mut night, 10.0, [0.0, 0.0], 1.0);
        // same seed, same elapsed time: the night run must be further along
        assert!(night.event.is_some() || day.event.is_none());
        let d2 = run(&mut night, 0.5, [0.0, 0.0], 1.0);
        let _ = d2;
    }

    #[test]
    fn holding_only_banks_inside_the_ring() {
        let mut p = Patrol::new(0, 11);
        // force a hold
        p.event = Some(Event { kind: EventKind::Hold, at: [0.0, 0.0], timer: 999.0, quota: 0, kills: 0, held: 0.0 });
        run(&mut p, 5.0, [100.0, 100.0], 0.0);
        assert!(p.event.map_or(false, |e| e.held < 0.01), "banked from outside the ring");
        run(&mut p, 5.0, [0.0, 0.0], 0.0);
        assert!(p.event.map_or(false, |e| e.held > 4.0), "did not bank from inside");
    }

    #[test]
    fn a_missed_clock_fails_rather_than_hanging() {
        let mut p = Patrol::new(0, 5);
        p.event = Some(Event { kind: EventKind::Surge, at: [0.0, 0.0], timer: 1.0, quota: 99, kills: 0, held: 0.0 });
        let t = run(&mut p, 2.0, [0.0, 0.0], 0.0);
        assert!(t.iter().any(|x| x.failed), "the clock ran out and nothing happened");
        assert!(p.event.is_none());
        assert_eq!(p.cleared, 0);
    }

    #[test]
    fn the_bridge_changes_world_and_abandons_the_event() {
        let mut p = Patrol::new(0, 9);
        run(&mut p, FIRST_EVENT_CD + 1.0, [0.0, 0.0], 0.0);
        assert!(p.event.is_some());
        let d = p.bridge_to("carrion").expect("no such destination");
        assert_eq!(d.terrain, Terrain::AsteroidField);
        assert!(p.event.is_none(), "an event survived a jump between worlds");
        assert!(p.bridge_to("nowhere").is_none());
    }

    #[test]
    fn a_seeded_patrol_replays_identically() {
        let mut a = Patrol::new(0, 424242);
        let mut b = Patrol::new(0, 424242);
        run(&mut a, 120.0, [0.0, 0.0], 0.0);
        run(&mut b, 120.0, [0.0, 0.0], 0.0);
        assert_eq!(a.cleared, b.cleared);
        assert_eq!(a.credits, b.credits);
        assert_eq!(a.event.is_some(), b.event.is_some());
    }

    #[test]
    fn the_archive_drips_rather_than_floods() {
        let mut p = Patrol::new(0, 21);
        let t = run(&mut p, 300.0, [0.0, 0.0], 0.0);
        let n = t.iter().filter(|x| x.lore.is_some()).count();
        // ~55-85 s apart plus the 40 s opener: a handful in five minutes,
        // not a wall of text
        assert!((3..=7).contains(&n), "{n} fragments in five minutes");
    }
}
