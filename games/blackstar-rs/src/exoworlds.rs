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

use crate::campaign::CampaignLog;
use crate::patrol::Faction;

pub struct ExoWorld {
    pub id: &'static str,
    pub name: &'static str,
    pub star: &'static str,
    pub star_kind: &'static str,
    pub distance_ly: f32,
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
        distance_ly: 40.7, mass_earths: 0.692, radius_earths: 0.920, orbital_period_days: 6.101,
        discovered: 2017, hostiles: Faction::Hijacks,
        note: "one of three TRAPPIST-1 worlds in the habitable zone; tidally locked, one face always to the star",
    },
    ExoWorld {
        id: "trappist1g", name: "TRAPPIST-1g", star: "TRAPPIST-1", star_kind: "ultracool red dwarf",
        distance_ly: 40.7, mass_earths: 1.321, radius_earths: 1.129, orbital_period_days: 12.352,
        discovered: 2017, hostiles: Faction::Hijacks,
        note: "the outer edge of the same system's habitable zone -- colder, heavier, denser air",
    },
    ExoWorld {
        id: "proxima_b", name: "PROXIMA CENTAURI b", star: "Proxima Centauri", star_kind: "flare-active red dwarf",
        distance_ly: 4.24, mass_earths: 1.07, radius_earths: 1.10, orbital_period_days: 11.186,
        discovered: 2016, hostiles: Faction::Corrupted,
        note: "the nearest star system to Earth's own; radius is a modelled estimate, not a transit measurement",
    },
    ExoWorld {
        id: "kepler186f", name: "KEPLER-186f", star: "Kepler-186", star_kind: "red dwarf",
        distance_ly: 582.0, mass_earths: 1.44, radius_earths: 1.17, orbital_period_days: 129.94,
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
}
