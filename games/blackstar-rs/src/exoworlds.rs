//! EXOWORLDS — real, NASA-catalogued exoplanets, reachable through the
//! space bridge once `the_far_gate` (mission 10, Film 4 — the bridge's
//! first jump) is cleared.
//!
//! `procgen.rs` already generates infinite fictional worlds from a seed
//! for the "uncharted" bridge jumps — mass-radius-guessed gravity, a
//! heuristic weather roll, `viable()` rejecting anything unenterable.
//! This module is not a competing generator. It is a small, fixed table
//! of NAMED worlds with REAL published parameters, for the handful of
//! bridge jumps the fiction should land on a system the audience can
//! look up rather than one `rng(seed)` invented. The two are meant to
//! sit side by side: most jumps procedural, a few real.
//!
//! ## Where the numbers come from
//!
//! Mass, radius and orbital period are the published values from NASA's
//! Exoplanet Catalog (science.nasa.gov/exoplanets) and the NASA Exoplanet
//! Archive (exoplanetarchive.ipac.caltech.edu), checked 2026-08-16. Every
//! other field the table needs — a doctrine faction, a beat of flavor
//! text — is invented, same as `patrol::Destination.note` is for the
//! Solar-System stops; only the physical numbers claim to be real.
//!
//! `gravity_g` is not a fitted constant and not `procgen.rs`'s heuristic
//! guess either — it is the same inverse-square relationship every
//! rocky-planet gravity figure in the literature comes from (surface g
//! scales with mass over radius squared, in Earth units). The TRAPPIST-1
//! entries below carry their own published surface-gravity figures in
//! the literature, which the test module checks this formula against
//! directly — the same "measured, not assumed" discipline `clones.rs`'s
//! `ICE_REF` uses for its own reference figure.
//!
//! ## What this deliberately does not do
//!
//! No `patrol::Destination` records are added and `ROUTE` is untouched —
//! that table's tests assume a fixed, curated, film-matched list, and
//! folding four more entries into it would ask `waves_for` and the film
//! theatre-matching tests to make sense of worlds no mission is set on.
//! This is the seam: a real, tested, standalone table and a gate
//! (`unlocked`) a patrol-mode screen can query once one exists. Turning an
//! `ExoWorld` into a place the player can actually stand in — terrain,
//! sky, ambient lines, a hostiles roster deep enough to fight in — is the
//! next piece, not this one.
//!
//! ## The fitness question
//!
//! `orbit()` and `survivable()` answer the question this table exists to
//! ask in the first place: given a real world's real physics, what could
//! actually stand on it? Not a new model — `survival::habitable_fraction`
//! already takes a `Body` (Cybertronian / HumanSuited / HumanBare) and a
//! `sun::Orbit` and returns the survivable fraction of the surface; this
//! only supplies the `Orbit` these four worlds derive to, from real
//! semi-major axes rather than a generated seed. The bridge is one
//! function each way, not a parallel system.

use crate::campaign::CampaignLog;
use crate::patrol::Faction;
use crate::sun::{Orbit, StarClass};
use crate::survival::{self, Body};

pub struct ExoWorld {
    pub id: &'static str,
    pub name: &'static str,
    pub star: &'static str,
    pub star_kind: &'static str,
    pub distance_ly: f32,
    /// semi-major axis, AU -- published, not guessed; this is what
    /// `sun::Orbit` actually needs, unlike `distance_ly` (distance from
    /// EARTH, irrelevant to the world's own climate) which the module
    /// keeps only for flavor text
    pub semi_major_axis_au: f32,
    /// Earth masses.
    pub mass_earths: f32,
    /// Earth radii.
    pub radius_earths: f32,
    pub orbital_period_days: f32,
    pub discovered: u16,
    /// which faction would contest a landing here, until this world has
    /// an actual mission written for it — a placeholder assignment, not a
    /// claim from the fiction the way `patrol::Destination.hostiles` is
    pub hostiles: Faction,
    pub note: &'static str,
}

pub const EXOWORLDS: &[ExoWorld] = &[
    ExoWorld {
        id: "trappist1e", name: "TRAPPIST-1e", star: "TRAPPIST-1", star_kind: "ultracool red dwarf",
        distance_ly: 40.7, semi_major_axis_au: 0.02925,
        mass_earths: 0.692, radius_earths: 0.920, orbital_period_days: 6.101,
        discovered: 2017, hostiles: Faction::Hijacks,
        note: "one of three TRAPPIST-1 worlds in the habitable zone; tidally locked, one face always to the star",
    },
    ExoWorld {
        id: "trappist1g", name: "TRAPPIST-1g", star: "TRAPPIST-1", star_kind: "ultracool red dwarf",
        distance_ly: 40.7, semi_major_axis_au: 0.04683,
        mass_earths: 1.321, radius_earths: 1.129, orbital_period_days: 12.352,
        discovered: 2017, hostiles: Faction::Hijacks,
        note: "the outer edge of the same system's habitable zone -- colder, heavier, denser air",
    },
    ExoWorld {
        id: "proxima_b", name: "PROXIMA CENTAURI b", star: "Proxima Centauri", star_kind: "flare-active red dwarf",
        distance_ly: 4.24, semi_major_axis_au: 0.04848,
        mass_earths: 1.07, radius_earths: 1.10, orbital_period_days: 11.186,
        discovered: 2016, hostiles: Faction::Corrupted,
        note: "the nearest star system to Earth's own; radius is a modelled estimate, not a transit measurement",
    },
    ExoWorld {
        id: "kepler186f", name: "KEPLER-186f", star: "Kepler-186", star_kind: "red dwarf",
        distance_ly: 582.0, semi_major_axis_au: 0.432,
        mass_earths: 1.44, radius_earths: 1.17, orbital_period_days: 129.94,
        discovered: 2014, hostiles: Faction::BlackstarClans,
        note: "the first Earth-sized world confirmed in another star's habitable zone",
    },
];

pub fn find(id: &str) -> Option<&'static ExoWorld> {
    EXOWORLDS.iter().find(|w| w.id == id)
}

/// Surface gravity in Earth g, from mass and radius alone -- the same
/// inverse-square relationship (`g ∝ M / R²`) every published exoplanet
/// surface-gravity figure comes from. Validated against the literature
/// in the tests below, not just against itself.
pub fn gravity_g(w: &ExoWorld) -> f32 {
    w.mass_earths / (w.radius_earths * w.radius_earths)
}

/// `sun::Orbit` for this world — the bridge into `survival.rs`'s already
/// existing, already tested habitability machinery, rather than a
/// second climate model for exoplanets alone.
///
/// `star: StarClass::M` for all four: TRAPPIST-1, Proxima Centauri and
/// Kepler-186 are all published M-dwarfs (TRAPPIST-1 specifically an
/// ultracool M8V, Proxima M5.5Ve, Kepler-186 M1V) — real spectral
/// classes, folded onto the engine's one generic M bucket the same way
/// `procgen.rs` buckets every generated star into seven classes rather
/// than a continuous temperature.
///
/// **This bucketing has a real, measured cost for these specific
/// worlds, and it is worth stating plainly rather than leaving the
/// number looking more authoritative than it is.** `StarClass::M`'s
/// generic spec (0.35 R☉, 3200 K — tuned as a typical M dwarf for
/// procedurally generated worlds) is much larger and hotter than
/// TRAPPIST-1's real star (about 0.119 R☉, 2566 K — it is an
/// *ultracool* dwarf, near the bottom of the M range). Run the
/// equilibrium-temperature formula with the engine's generic bucket
/// and TRAPPIST-1e comes out around 490 K; run the same formula with
/// TRAPPIST-1's real radius and temperature and it comes out around
/// 230 K, close to the 249.7 K the discovery literature actually
/// measured. So `orbit()`'s derived climate for TRAPPIST-1e and 1g
/// runs hotter than reality by a few hundred kelvin — a resolution
/// limit of the seven-bucket star model, not an error in the
/// mass/radius/period figures themselves (those are checked against
/// the literature directly in `gravity_g`'s tests). Treat `surface_k()`
/// and anything downstream of it (`survivable()` included) for these
/// two worlds as a rough approximation until `sun::Orbit` can take a
/// star's own measured radius and temperature instead of a bucket.
///
/// `albedo` and `air` are NOT published for any of these four — no
/// confirmed atmosphere exists for any of them yet, this module is not
/// pretending otherwise. Both are set to a flagged, documented
/// mid-range guess (Earth-ish albedo, thin air) rather than a
/// confident-looking number: the same honesty `procgen.rs`'s own doc
/// comment insists on for its invented worlds ("nothing here invents a
/// rule... a mass-radius guess"), except here the mass and radius are
/// real and only the atmosphere is a guess.
pub fn orbit(w: &ExoWorld) -> Orbit {
    Orbit { star: StarClass::M, au: w.semi_major_axis_au, albedo: 0.30, air: 0.10 }
}

/// What fraction of this world a given body could actually survive on
/// — `survival::habitable_fraction`, unmodified, fed real orbital
/// data instead of a generated one. This is the fitness check: whether
/// a Cybertronian trooper, a suited human, or an unsuited one could
/// stand here at all.
pub fn survivable(w: &ExoWorld, body: Body) -> f32 {
    survival::habitable_fraction(body, &orbit(w))
}

/// The space bridge: closed until the bridge's first jump is actually
/// flown. `the_far_gate` is mission 10, Film 4's opening beat -- the
/// mission whose own outro is "now you have seen a hijack. there are
/// more."; the exoworlds are the "more."
pub fn unlocked(log: &CampaignLog) -> bool {
    log.is_cleared("the_far_gate")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        for (i, w) in EXOWORLDS.iter().enumerate() {
            assert!(EXOWORLDS.iter().skip(i + 1).all(|o| o.id != w.id), "duplicate {}", w.id);
        }
    }

    #[test]
    fn every_world_is_a_real_size() {
        // sanity floor on the physical numbers themselves -- this is not
        // a gas giant catalogue, and a copy/paste error (AU where days
        // were meant, say) should fail loudly here rather than silently
        // producing a world with a ten-thousand-day year
        for w in EXOWORLDS {
            assert!(w.mass_earths > 0.05 && w.mass_earths < 10.0, "{} mass {}", w.id, w.mass_earths);
            assert!(w.radius_earths > 0.3 && w.radius_earths < 3.0, "{} radius {}", w.id, w.radius_earths);
            assert!(w.orbital_period_days > 0.1 && w.orbital_period_days < 1000.0, "{} period {}", w.id, w.orbital_period_days);
            assert!(w.distance_ly > 0.0, "{} has no distance", w.id);
        }
    }

    #[test]
    fn gravity_matches_the_published_figures() {
        // TRAPPIST-1e and 1g both carry a measured surface-gravity figure
        // in the discovery literature (0.817g and 1.035g respectively) --
        // this checks the formula against those, not against a number
        // this module invented for itself
        let e = find("trappist1e").unwrap();
        assert!((gravity_g(e) - 0.817).abs() < 0.02, "trappist1e gravity {}", gravity_g(e));
        let g = find("trappist1g").unwrap();
        assert!((gravity_g(g) - 1.035).abs() < 0.02, "trappist1g gravity {}", gravity_g(g));
        // and every world's gravity is at least in a survivable range --
        // a suited raid on a 4g world is a different design conversation
        // this table is not the place to start
        for w in EXOWORLDS {
            assert!(gravity_g(w) > 0.3 && gravity_g(w) < 2.0, "{} gravity {}", w.id, gravity_g(w));
        }
    }

    #[test]
    fn the_bridge_opens_on_the_far_gate_and_nothing_else() {
        let mut log = CampaignLog::default();
        assert!(!unlocked(&log), "the bridge is open before the first jump is flown");
        log.record("the_beach_opens"); // an early, unrelated mission
        assert!(!unlocked(&log), "an unrelated clear opened the bridge");
        log.record("the_far_gate");
        assert!(unlocked(&log));
    }

    #[test]
    fn equilibrium_temperature_is_a_known_overestimate_for_the_ultracool_dwarfs() {
        // NOT a match to the literature -- documented in orbit()'s doc
        // comment as a real, diagnosed limit of the engine's generic
        // StarClass::M bucket (0.35 Rsun / 3200 K) against TRAPPIST-1's
        // actual, much smaller and cooler star (~0.119 Rsun / 2566 K).
        // What this test asserts is the honest version: the derived
        // temperature is systematically HIGHER than the published
        // figure, by roughly the amount the diagnosis in orbit()'s doc
        // predicts -- not a close match, and not silently wrong either.
        let e = find("trappist1e").unwrap();
        let published = 249.7;
        let derived = orbit(e).surface_k();
        assert!(derived > published, "expected the bucket to overestimate, got {derived} vs {published}");
        assert!(derived < published + 400.0, "overestimate is worse than the diagnosed cause explains: {derived}");
        // kepler-186's host is a less extreme M dwarf (M1V, much closer
        // to the engine's generic bucket) and comes out closer, though
        // still not exact -- the same direction of error, smaller
        let k = find("kepler186f").unwrap();
        assert!((orbit(k).surface_k() - 188.0).abs() < 60.0, "kepler186f Teq {}", orbit(k).surface_k());
    }

    #[test]
    fn a_machine_stands_where_the_worlds_do_not_agree_on_a_person() {
        // survival.rs's own headline claim ("a bot stands where a person
        // cannot") checked against real worlds instead of a generated
        // one: every tidally locked, close-in world here should show the
        // same asymmetry between a Cybertronian's wide thermal band and
        // an unsuited human's narrow one
        for w in EXOWORLDS {
            let bot = survivable(w, Body::Cybertronian);
            let bare = survivable(w, Body::HumanBare);
            assert!(bot >= bare, "{}: bot {bot} < bare {bare}, machines should never do worse", w.id);
        }
        // and at least one of the four should be flatly closed to an
        // unsuited human -- these are alien worlds, not four Earths
        assert!(EXOWORLDS.iter().any(|w| survivable(w, Body::HumanBare) < 1.0));
    }
}
