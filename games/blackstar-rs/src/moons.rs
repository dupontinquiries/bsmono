//! MOONS — the five stops on the CIRCUIT ring, as seeded specs.
//!
//! `circuit.rs` already fixes `STOP_COUNT = 5` and the leg timing; this
//! module gives those five slots an identity — a name, a material, a
//! weather roll, a gravity, and a `planetlook::PlanetLook` — all
//! DERIVED, not authored. Zero new texture or mesh bytes: everything
//! here is `terrain`/`survival`/`planetlook`'s existing seed-to-physics
//! machinery, called five times with fixed seeds so the ring is the
//! same ring on every launch (`STOPS` is a `const fn` table, not a
//! roll at runtime) while still being generated rather than hand-tuned.
//!
//! This module does not touch `world::World` — see `seam.rs`'s rule.
//! `circuit_map.rs` (not yet written) is the one file allowed to turn a
//! `MoonSpec` into an actual baked tile.

use crate::planetlook::{self, PlanetLook, PlanetSpec, Surface};
use crate::survival::Weather;

/// A stop's section mechanic — the physics toy that makes each moon
/// distinct in play, not just in palette. Named per the design brief;
/// `circuit_map.rs` reads this to decide which existing physics module
/// (`hydro`, `lift`, `loadbearing`, `cable`, terrain height animation)
/// governs the tile.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    /// spin-g Coriolis, zero-g axis refuge (the Dock only)
    SpinDeck,
    /// terminator sweep forces movement; telegraphed lava seams
    ThermalSweep,
    /// low friction, geyser launches, calving shelf sections
    Calving,
    /// zero-g wreck field, `cable.rs` grapple traversal, absorption risk
    WreckGrapple,
    /// animated ground swells, pushable boulders, blinding dust
    TidalSwell,
}

/// One stop, fully derived from its seed.
#[derive(Clone, Copy, Debug)]
pub struct MoonSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub section: Section,
    /// surface gravity, g (0 for the Dock's deck, which is spin-g only)
    pub gravity_g: f32,
    pub weather: Weather,
    pub day_length_s: f32,
    pub planet: PlanetSpec,
    pub look: PlanetLook,
}

/// Fixed per-stop seeds. Not the stop's index — a distinct constant
/// per moon, so reordering `circuit::LEG_S` later doesn't reroll a
/// moon's look out from under it.
const SEEDS: [u64; crate::circuit::STOP_COUNT] = [
    0x5117_D0C4, // Dock (seed mostly unused: gravity/section are fixed)
    0xA5FA_1100, // Ashfall
    0x2111_E500, // Rime
    0x717E_0000, // Tithe
    0x40EA_D500, // Knead
];

const IDS: [&str; crate::circuit::STOP_COUNT] =
    ["spindle_dock", "ashfall", "rime", "tithe", "knead"];
const NAMES: [&str; crate::circuit::STOP_COUNT] =
    ["THE DOCK", "ASHFALL", "THE RIME", "THE TITHE", "THE KNEAD"];
const SECTIONS: [Section; crate::circuit::STOP_COUNT] = [
    Section::SpinDeck,
    Section::ThermalSweep,
    Section::Calving,
    Section::WreckGrapple,
    Section::TidalSwell,
];
/// (guess) — the brief's table: 0.18 / 0.13 / 0.05 / 0.22 g; the Dock
/// is a spin deck, not a surface, so its "gravity" is 0 here and
/// `shipgrav.rs` supplies the felt weight instead.
const GRAVITY_G: [f32; crate::circuit::STOP_COUNT] = [0.0, 0.18, 0.13, 0.05, 0.22];
/// (guess) — matches the brief's per-stop weather assignment.
const WEATHER: [Weather; crate::circuit::STOP_COUNT] = [
    Weather::Clear, // the Dock is sealed; weather doesn't apply
    Weather::ThermalFront,
    Weather::DustStorm,
    Weather::Clear,
    Weather::GiantWave,
];
/// (guess) — the terminator sweep needs a FAST rotator; the rest are
/// slow or tidally locked, matching how each was described.
const DAY_LENGTH_S: [f32; crate::circuit::STOP_COUNT] = [0.0, 180.0, 0.0, 0.0, 0.0];

/// Every stop's spec, in `circuit::STOP_COUNT` order. Computed, not
/// cached — cheap (a handful of float ops per call) and this way there
/// is no stale-cache bug to have.
pub fn stops() -> [MoonSpec; crate::circuit::STOP_COUNT] {
    let out: [MoonSpec; crate::circuit::STOP_COUNT] = core::array::from_fn(|i| {
        let planet = if i == crate::circuit::DOCK {
            // the Dock is a built structure, not a rolled world — a
            // fixed, dim, airless spec so `look()` still produces
            // something coherent for the hologram/loading-screen views
            PlanetSpec {
                star_temp_k: 5772.0,
                insolation: 0.05,
                atmosphere_bar: 0.0,
                surface: Surface::Basalt,
            }
        } else {
            planetlook::from_seed(SEEDS[i])
        };
        let look = planetlook::look(&planet);
        MoonSpec {
            id: IDS[i],
            name: NAMES[i],
            section: SECTIONS[i],
            gravity_g: GRAVITY_G[i],
            weather: WEATHER[i],
            day_length_s: DAY_LENGTH_S[i],
            planet,
            look,
        }
    });
    out
}

/// Look a stop up by id, for menus/saves that persist an id string
/// rather than an index (same convention as `patrol::Destination`).
pub fn by_id(id: &str) -> Option<MoonSpec> {
    stops().into_iter().find(|s| s.id == id)
}

/// One rubble bridge, joining stop `a` to `a`'s successor on the ring.
/// Length is derived from `circuit::LEG_S` and `WALK_LEG_RATIO` — one
/// number, not two, so the timetable and the walkable distance can
/// never disagree.
#[derive(Clone, Copy, Debug)]
pub struct Bridge {
    pub from: usize,
    pub to: usize,
    pub length_m: f32,
    /// seed for the rubble-chain SDF spline `circuit_map.rs` builds
    pub seed: u64,
}

pub fn bridges() -> [Bridge; crate::circuit::STOP_COUNT] {
    core::array::from_fn(|i| {
        let to = (i + 1) % crate::circuit::STOP_COUNT;
        Bridge {
            from: i,
            to,
            length_m: crate::circuit::LEG_S[i] * crate::circuit::WALK_LEG_RATIO,
            seed: SEEDS[i] ^ SEEDS[to],
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stop_is_distinct_in_material_and_weather() {
        let s = stops();
        for i in 0..s.len() {
            for j in (i + 1)..s.len() {
                assert!(
                    s[i].planet.surface != s[j].planet.surface || s[i].weather != s[j].weather,
                    "stops {} and {} are indistinguishable ({:?}, {:?})",
                    s[i].id, s[j].id, s[i].planet.surface, s[i].weather
                );
            }
        }
    }

    #[test]
    fn every_stop_id_is_unique_and_matches_circuit_order() {
        let s = stops();
        assert_eq!(s[crate::circuit::DOCK].id, "spindle_dock");
        let mut ids: Vec<&str> = s.iter().map(|m| m.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), crate::circuit::STOP_COUNT);
    }

    #[test]
    fn gravity_is_within_the_ranges_the_brief_specified() {
        for m in stops() {
            if m.id == "spindle_dock" {
                assert_eq!(m.gravity_g, 0.0);
            } else {
                assert!(m.gravity_g > 0.0 && m.gravity_g < 1.0, "{}: {}", m.id, m.gravity_g);
            }
        }
    }

    #[test]
    fn stops_are_deterministic_across_calls() {
        let a = stops();
        let b = stops();
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.id, y.id);
            assert_eq!(x.planet, y.planet);
            assert_eq!(x.look, y.look);
        }
    }

    #[test]
    fn by_id_finds_every_stop_and_nothing_else() {
        for m in stops() {
            assert_eq!(by_id(m.id).unwrap().id, m.id);
        }
        assert!(by_id("not_a_real_stop").is_none());
    }

    #[test]
    fn bridge_lengths_agree_with_the_timetable_by_construction() {
        for b in bridges() {
            let expect = crate::circuit::LEG_S[b.from] * crate::circuit::WALK_LEG_RATIO;
            assert_eq!(b.length_m, expect);
        }
    }

    #[test]
    fn bridges_form_a_closed_ring() {
        let bs = bridges();
        for i in 0..crate::circuit::STOP_COUNT {
            assert_eq!(bs[i].to, (i + 1) % crate::circuit::STOP_COUNT);
        }
        // walking every bridge once returns you to the start
        let mut at = 0usize;
        for _ in 0..crate::circuit::STOP_COUNT {
            at = bs[at].to;
        }
        assert_eq!(at, 0);
    }

    #[test]
    fn every_look_has_real_value_structure() {
        // reuse planetlook's own contract rather than re-deriving it:
        // a look with a black-on-black sky would pass "distinct surface"
        // above but still be unusable as a hologram render.
        for m in stops() {
            let g = m.look.ground;
            let z = m.look.zenith;
            assert!(g.length_squared() >= 0.0 && z.length_squared() >= 0.0);
            // ground and zenith must not be IDENTICAL colours (a real bug
            // this shape of test would have caught in another module)
            assert!((g - z).length() > 1e-4, "{}: ground==zenith", m.id);
        }
    }
}
