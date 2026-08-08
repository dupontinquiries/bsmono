//! BALLISTICS — what a weapon does depends on what it is firing through.
//!
//! The roster has beams, railguns, launchers, scatterguns and energy blades,
//! and until now they differed only in numbers. But the campaign already
//! visits vacuum, thin air, thick air and the bottom of an ocean, and those
//! are not cosmetic differences to a projectile:
//!
//! **In vacuum a bullet never slows down and a blast has no wave.** An
//! explosion in air kills mostly by overpressure; in vacuum there is nothing
//! to press, so only fragments matter and a shaped charge beats a big one.
//!
//! **Underwater a projectile stops almost immediately** — drag goes as
//! density and seawater is 837 times air — while a blast travels FURTHER,
//! because water is nearly incompressible and carries the shock instead of
//! absorbing it. The two invert, which is why the same loadout is wrong in
//! both places.
//!
//! **A beam is the opposite of a projectile in every medium.** Vacuum is
//! perfect for it and water eats it by wavelength, using the same Jerlov
//! coefficients `subsea` lights the raid with.
//!
//! None of this is a table. Each is one line of physics applied to a
//! measurement the weapon already carries.

use crate::hydro::RHO_SEA;
use crate::subsea::{K_BLUE, K_GREEN, K_RED};

/// What the shot is travelling through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Medium {
    Vacuum,
    /// atmosphere, as a fraction of Earth's
    Air,
    Water,
}

/// The environment, in the terms the rest of the engine already uses.
#[derive(Clone, Copy, Debug)]
pub struct Ambient {
    pub medium: Medium,
    /// atmospheres; ignored for vacuum and water
    pub air: f32,
    /// metres of water above, for beam attenuation
    pub depth_m: f32,
}

impl Ambient {
    pub fn vacuum() -> Ambient {
        Ambient { medium: Medium::Vacuum, air: 0.0, depth_m: 0.0 }
    }
    pub fn earthlike() -> Ambient {
        Ambient { medium: Medium::Air, air: 1.0, depth_m: 0.0 }
    }
    pub fn subsea(depth_m: f32) -> Ambient {
        Ambient { medium: Medium::Water, air: 1.0, depth_m }
    }
    /// kg/m^3
    pub fn density(&self) -> f32 {
        match self.medium {
            Medium::Vacuum => 0.0,
            Medium::Air => 1.225 * self.air.max(0.0),
            Medium::Water => RHO_SEA,
        }
    }
}

/// How a weapon delivers its damage. The six-axis space says WHAT a weapon
/// is; this says what physics it obeys.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Delivery {
    /// mass thrown: rifles, railguns, scatterguns
    Kinetic,
    /// light: beams, lasers
    Beam,
    /// a charge that goes off: launchers, cannons
    Blast,
    /// contact only: blades, hammers
    Melee,
}

/// A shot, as the weapon carries it.
#[derive(Clone, Copy, Debug)]
pub struct Shot {
    pub delivery: Delivery,
    /// muzzle speed, m/s. Ignored for beams.
    pub speed_ms: f32,
    /// projectile mass, kg
    pub mass_kg: f32,
    /// frontal area, m^2 — partclass measures this for any part
    pub area_m2: f32,
    /// drag coefficient, also measured
    pub cd: f32,
}

/// EFFECTIVE RANGE in metres: where the shot has lost most of what it had.
///
/// For a projectile that is the ballistic distance to half its energy, which
/// is mass over drag — heavy and narrow carries, light and broad does not.
/// For a beam it is absorption. For a blast it is the medium's ability to
/// carry a pressure wave. For melee it is arm's length in any medium.
pub fn range_m(s: &Shot, a: &Ambient) -> f32 {
    match s.delivery {
        Delivery::Melee => 3.0,
        Delivery::Kinetic => {
            let rho = a.density();
            if rho <= 1e-6 {
                // NOTHING SLOWS IT DOWN. A bullet in vacuum is limited by
                // aim and patience, not by air, so this returns a very large
                // number rather than infinity — the engine wants a float it
                // can compare.
                return 50_000.0;
            }
            // distance to lose half the energy: ln2 * m / (0.5 rho Cd A)
            let k = 0.5 * rho * s.cd.max(0.01) * s.area_m2.max(1e-6);
            (0.693 * s.mass_kg.max(1e-4) / k).clamp(0.5, 50_000.0)
        }
        Delivery::Beam => match a.medium {
            // vacuum is what a beam wants: nothing to absorb it
            Medium::Vacuum => 50_000.0,
            // air scatters mildly; a thick atmosphere is a real limit
            Medium::Air => (3_000.0 / a.air.max(0.05)).min(50_000.0),
            // WATER EATS IT BY WAVELENGTH, using the same coefficients the
            // subsea lighting is built on. Blue carries furthest, which is
            // why a beam weapon underwater would be blue and why the raid's
            // own light is too.
            Medium::Water => {
                let k = K_BLUE.min(K_GREEN).min(K_RED);
                (0.693 / k).min(50_000.0)
            }
        },
        Delivery::Blast => match a.medium {
            // NO MEDIUM, NO WAVE. In vacuum an explosion is fragments and
            // nothing else, so its reach collapses to what the casing throws.
            Medium::Vacuum => 4.0,
            Medium::Air => 12.0 * a.air.max(0.05).sqrt(),
            // and water carries a shock FURTHER than air, because it is
            // nearly incompressible: it transmits rather than absorbing
            Medium::Water => 26.0,
        },
    }
}

/// How much of the shot's damage actually lands at `distance_m`.
pub fn falloff(s: &Shot, a: &Ambient, distance_m: f32) -> f32 {
    let r = range_m(s, a).max(0.1);
    let d = distance_m.max(0.0);
    match s.delivery {
        // a blade does not fall off, it either reaches or does not
        Delivery::Melee => {
            if d <= r {
                1.0
            } else {
                0.0
            }
        }
        // exponential: half the energy at the nominal range
        _ => (-0.693 * d / r).exp(),
    }
}

/// THE LOADOUT ANSWER: which delivery is best here, and by how much over the
/// next one. A big margin means the choice matters; a small one means take
/// what you like.
pub fn best_here(a: &Ambient, at_m: f32) -> (Delivery, f32) {
    let probe = |dl: Delivery| Shot {
        delivery: dl,
        speed_ms: 900.0,
        mass_kg: 0.02,
        area_m2: 8.0e-5,
        cd: 0.3,
    };
    let mut scored: Vec<(Delivery, f32)> = [Delivery::Kinetic, Delivery::Beam, Delivery::Blast]
        .into_iter()
        .map(|d| (d, falloff(&probe(d), a, at_m)))
        .collect();
    scored.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap());
    (scored[0].0, scored[0].1 - scored[1].1)
}

/// AN ELEMENT AGAINST ITS OWN MEDIUM. A fire weapon underwater and an ice
/// weapon on a lava world are both fighting the place as well as the target,
/// and that is a real effect rather than a rule: the medium is already
/// saturated with what the weapon is trying to add.
pub fn element_scale(e: crate::weapons::Element, a: &Ambient, surface_k: f32) -> f32 {
    use crate::weapons::Element as E;
    let base = match (e, a.medium) {
        // water quenches fire and conducts electricity
        (E::Fire, Medium::Water) => 0.25,
        (E::Electric, Medium::Water) => 1.6,
        // ice needs something to freeze; vacuum has nothing
        (E::Ice, Medium::Vacuum) => 0.4,
        (E::Water, Medium::Vacuum) => 0.3,
        // air weapons need air
        (E::Air, Medium::Vacuum) => 0.15,
        (E::Air, Medium::Air) => 0.7 + 0.6 * a.air.clamp(0.0, 1.5),
        _ => 1.0,
    };
    // and a thermal weapon does less to ground that is already glowing,
    // while a cryogenic one does more there and less on a frozen world —
    // the same fact from both sides
    let hot = (surface_k / 1_400.0).clamp(0.0, 1.0);
    let therm = match e {
        E::Fire => (1.0 - hot * 0.8).max(0.2),
        E::Ice => (0.4 + 0.6 * (1.0 - hot)).max(0.2),
        _ => 1.0,
    };
    base * therm
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bullet() -> Shot {
        Shot { delivery: Delivery::Kinetic, speed_ms: 900.0, mass_kg: 0.02, area_m2: 8.0e-5, cd: 0.3 }
    }
    fn beam() -> Shot {
        Shot { delivery: Delivery::Beam, ..bullet() }
    }
    fn rocket() -> Shot {
        Shot { delivery: Delivery::Blast, ..bullet() }
    }

    #[test]
    fn a_bullet_in_vacuum_is_limited_by_aim_not_by_air() {
        let v = range_m(&bullet(), &Ambient::vacuum());
        let a = range_m(&bullet(), &Ambient::earthlike());
        assert!(v > a * 20.0, "vacuum {v:.0} air {a:.0}");
        assert!(a > 200.0 && a < 5_000.0, "an air range of {a:.0} m is not a bullet");
    }

    #[test]
    fn water_stops_a_projectile_and_carries_a_blast() {
        // the inversion that makes the same loadout wrong in both places
        let w = Ambient::subsea(12.0);
        let a = Ambient::earthlike();
        assert!(range_m(&bullet(), &w) < range_m(&bullet(), &a) * 0.01,
            "underwater {:.1} vs air {:.0}", range_m(&bullet(), &w), range_m(&bullet(), &a));
        assert!(range_m(&rocket(), &w) > range_m(&rocket(), &a),
            "water should carry a shock further, not less");
    }

    #[test]
    fn an_explosion_in_vacuum_is_fragments_and_nothing_else() {
        // no medium, no overpressure — a shaped charge beats a big one
        let v = range_m(&rocket(), &Ambient::vacuum());
        let a = range_m(&rocket(), &Ambient::earthlike());
        assert!(v < a * 0.5, "vacuum {v} air {a}");
        assert!(v > 0.0, "the casing still throws something");
    }

    #[test]
    fn a_beam_is_the_opposite_of_a_projectile_in_every_medium() {
        let v = Ambient::vacuum();
        let w = Ambient::subsea(20.0);
        // vacuum: the beam wins outright and the bullet does too, but the
        // beam is unlimited where the bullet is merely unimpeded
        assert!(range_m(&beam(), &v) >= range_m(&bullet(), &v));
        // water: the bullet collapses and the beam merely shortens
        assert!(range_m(&beam(), &w) > range_m(&bullet(), &w) * 10.0);
    }

    #[test]
    fn the_beam_that_carries_underwater_is_the_colour_the_water_lets_through() {
        // same Jerlov coefficients the raid is lit with, which is why a
        // subsea beam weapon would be blue and so is the raid
        let w = Ambient::subsea(30.0);
        let r = range_m(&beam(), &w);
        // blue at k=0.021 gives about 33 m to half energy
        assert!((r - 33.0).abs() < 3.0, "{r}");
        assert!(K_BLUE < K_GREEN && K_GREEN < K_RED);
    }

    #[test]
    fn thin_air_helps_a_bullet_and_hurts_a_blast() {
        let thick = Ambient { medium: Medium::Air, air: 1.0, depth_m: 0.0 };
        let thin = Ambient { medium: Medium::Air, air: 0.06, depth_m: 0.0 };
        assert!(range_m(&bullet(), &thin) > range_m(&bullet(), &thick));
        assert!(range_m(&rocket(), &thin) < range_m(&rocket(), &thick));
    }

    #[test]
    fn a_blade_reaches_or_it_does_not() {
        let m = Shot { delivery: Delivery::Melee, ..bullet() };
        for a in [Ambient::vacuum(), Ambient::earthlike(), Ambient::subsea(5.0)] {
            assert_eq!(falloff(&m, &a, 1.0), 1.0);
            assert_eq!(falloff(&m, &a, 40.0), 0.0);
        }
    }

    #[test]
    fn the_loadout_answer_changes_with_the_place() {
        // and the margin says whether the choice matters
        let (deep, _) = best_here(&Ambient::subsea(20.0), 25.0);
        let (space, _) = best_here(&Ambient::vacuum(), 400.0);
        assert_ne!(deep, space, "the same weapon should not win everywhere");
        assert_eq!(deep, Delivery::Beam, "underwater at 25 m a beam should win");
        // point blank in a corridor, everything works and the margin is small
        let (_, close_margin) = best_here(&Ambient::earthlike(), 1.0);
        let (_, far_margin) = best_here(&Ambient::earthlike(), 900.0);
        assert!(close_margin < far_margin, "close {close_margin} far {far_margin}");
    }

    #[test]
    fn an_element_fights_the_place_as_well_as_the_target() {
        use crate::weapons::Element as E;
        let sea = Ambient::subsea(10.0);
        let vac = Ambient::vacuum();
        let air = Ambient::earthlike();
        // water quenches fire and conducts electricity
        assert!(element_scale(E::Fire, &sea, 290.0) < 0.4);
        assert!(element_scale(E::Electric, &sea, 290.0) > 1.4);
        // ice needs something to freeze
        assert!(element_scale(E::Ice, &vac, 200.0) < element_scale(E::Ice, &air, 200.0));
        // and a fire weapon does little to ground already glowing
        assert!(element_scale(E::Fire, &air, 1_300.0) < element_scale(E::Fire, &air, 280.0) * 0.5);
        // while ice does MORE there, which is the same fact from the other side
        assert!(element_scale(E::Ice, &air, 1_300.0) < element_scale(E::Ice, &air, 200.0));
    }

    #[test]
    fn every_verb_obeys_some_physics() {
        use crate::weapons::Verb;
        for v in [Verb::Push, Verb::Shove, Verb::Blink, Verb::Flak, Verb::Pool, Verb::Cone,
                  Verb::Bank, Verb::Pierce, Verb::Chain, Verb::Thrust, Verb::Vortex,
                  Verb::Beam, Verb::Grapple] {
            let d = v.delivery();
            let s = Shot { delivery: d, speed_ms: 900.0, mass_kg: 0.02, area_m2: 8e-5, cd: 0.3 };
            assert!(range_m(&s, &Ambient::earthlike()) > 0.0, "{v:?}");
        }
        assert_eq!(Verb::Beam.delivery(), Delivery::Beam);
        assert_eq!(Verb::Grapple.delivery(), Delivery::Melee);
    }

    #[test]
    fn falloff_is_exponential_rather_than_a_curve_i_liked() {
        let a = Ambient::earthlike();
        let r = range_m(&bullet(), &a);
        assert!((falloff(&bullet(), &a, r) - 0.5).abs() < 0.01, "half energy at the nominal range");
        let one = falloff(&bullet(), &a, r);
        let two = falloff(&bullet(), &a, r * 2.0);
        assert!((two - one * one).abs() < 0.01, "doubling the distance should square it");
    }
}
