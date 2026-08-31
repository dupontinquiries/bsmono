//! WHAT A PLANET LOOKS LIKE, DERIVED FROM WHAT IT IS.
//!
//! The 36 `MapStyle`s are inherited from skydribble, hand-painted for a
//! Rocket-League arena and kept only because they were there. Replacing them
//! with generated planets needs one thing neither existing generator has:
//! `procgen::World` produces an orbit, a weather roll and a gravity from a
//! mass-radius guess, and `exoworlds::ExoWorld` carries real published mass,
//! radius and semi-major axis — and NEITHER says what the place looks like.
//!
//! Hand-painting a palette per generated world defeats the point, and rolling
//! colours from the seed produces noise wearing a planet's name. So the look
//! comes from the physics that is already there.
//!
//! ## What actually sets a planet's colour
//!
//! * **The star.** A planet is lit by one blackbody, and its temperature sets
//!   the illuminant. A red dwarf's light is genuinely red, so nothing under it
//!   can be neutral; an F-type is blue-white.
//! * **The atmosphere.** Rayleigh scattering goes as 1/λ⁴, which is why a
//!   thick atmosphere has a blue zenith and a reddened horizon — the horizon
//!   is a longer path through the same air. Thin it and both collapse toward
//!   the star's own colour against black. Mars has 0.006 bar and a butterscotch
//!   sky; the Moon has none and a black one at noon.
//! * **The ground.** Albedo is a material property with published values, and
//!   the range is not subtle: basalt near 0.07, regolith around 0.12, sand
//!   0.35, fresh ice past 0.6. `valuestruct.rs` already caught MEADOW shipping
//!   grass at 0.456 — five times life — so getting this from a table rather
//!   than by eye is the entire lesson applied forward.
//! * **The distance.** Insolation falls as 1/d², which scales everything.
//!
//! ## The property that matters more than any single colour
//!
//! A generated planet must have VALUE STRUCTURE — `valuestruct.rs`'s log₂
//! separation between ground, horizon and sky. A hand table can be audited
//! once; a generator has to hold the property across every seed it can
//! produce, and that is a test rather than a review. It is the check that
//! stops "procedural" from meaning "grey".

use glam::Vec3;

/// What the ground is made of. Albedos are published material values, not
/// choices — see the module doc on MEADOW.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// Fresh basaltic rock. The darkest common planetary surface.
    Basalt,
    /// Impact-gardened dust. The Moon, Mercury, most airless bodies.
    Regolith,
    /// Oxidised dust. Mars.
    Ferric,
    /// Dry silicate sand.
    Sand,
    /// Water ice, old and dirty rather than fresh snow.
    Ice,
    /// Vegetated land, at a real leaf albedo.
    Verdant,
}

impl Surface {
    /// Linear reflectance per channel. Bond-albedo scale: these are what the
    /// surface RETURNS, before any light lands on it.
    pub fn albedo(self) -> Vec3 {
        match self {
            Surface::Basalt => Vec3::new(0.070, 0.068, 0.066),
            Surface::Regolith => Vec3::new(0.130, 0.122, 0.110),
            Surface::Ferric => Vec3::new(0.220, 0.120, 0.075),
            Surface::Sand => Vec3::new(0.400, 0.340, 0.230),
            Surface::Ice => Vec3::new(0.580, 0.610, 0.640),
            Surface::Verdant => Vec3::new(0.075, 0.150, 0.060),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Surface::Basalt => "basalt",
            Surface::Regolith => "regolith",
            Surface::Ferric => "ferric dust",
            Surface::Sand => "sand",
            Surface::Ice => "ice",
            Surface::Verdant => "verdant",
        }
    }
}

/// Everything the look is derived from. Every field is a physical quantity
/// the existing generators already produce or could.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanetSpec {
    /// Effective temperature of the primary, kelvin. M dwarf ~3000, Sun 5772,
    /// F ~7000.
    pub star_temp_k: f32,
    /// Starlight at the surface relative to Earth's. `1.0` is Earth; Mars is
    /// 0.43; Pluto is about 0.001.
    pub insolation: f32,
    /// Surface pressure in bar. 0 is airless, Earth is 1.0, Mars 0.006.
    pub atmosphere_bar: f32,
    pub surface: Surface,
}

/// The derived palette, in the shape `MapStyle` wants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanetLook {
    pub ground: Vec3,
    pub horizon: Vec3,
    pub zenith: Vec3,
    /// Fog density for the shader's `1 - exp(-d * fog_d)`.
    pub fog_d: f32,
    /// Ground wetness / smoothness, which `optics.rs` turns into a reflection
    /// ceiling. Only ice is smooth enough to reflect at grazing.
    pub wet: f32,
}

/// Blackbody colour at `t` kelvin, normalised so the brightest channel is 1.
///
/// A cheap Planck-locus fit rather than a spectrum integral: the point is that
/// a 3000 K dwarf is unmistakably red and a 7000 K star is blue-white, and the
/// fit reproduces that ordering across the range stars actually occupy.
pub fn star_color(t: f32) -> Vec3 {
    let t = t.clamp(1500.0, 12_000.0);
    let x = t / 1000.0;
    // monotone in the right directions: red falls slightly with temperature,
    // blue climbs steeply and saturates
    let r = 1.0;
    let g = (0.38 + 0.093 * x - 0.0032 * x * x).clamp(0.20, 1.0);
    let b = if t < 2000.0 { 0.02 } else { ((x - 1.9) * 0.19).clamp(0.02, 1.0) };
    let v = Vec3::new(r, g, b);
    v / v.max_element().max(1e-4)
}

/// How strongly the air scatters, 0 (vacuum) to 1 (thick).
///
/// Saturating rather than linear: Earth at 1 bar is already an opaque blue
/// sky, and Venus at 92 bar is not ninety-two times bluer.
fn air(bar: f32) -> f32 {
    let b = bar.max(0.0);
    b / (b + 0.35)
}

/// Derive the look.
pub fn look(spec: &PlanetSpec) -> PlanetLook {
    let sun = star_color(spec.star_temp_k);
    let a = air(spec.atmosphere_bar);
    // Insolation scales what lands, but the eye adapts, so this enters as a
    // compressed factor rather than a raw multiplier — a cube root is the
    // usual stand-in for that adaptation and it keeps Pluto visible.
    let lit = spec.insolation.max(1e-4).cbrt().clamp(0.15, 1.6);

    // GROUND: albedo times the light that reaches it.
    let ground = spec.surface.albedo() * sun * lit;

    // ZENITH: Rayleigh, so the blue channel survives the long climb and red
    // does not. With no air there is nothing to scatter and the sky is space.
    let rayleigh = Vec3::new(0.16, 0.42, 1.0);
    let zenith = rayleigh * sun * a * a * lit * 0.55;

    // HORIZON: the same air along a much longer path, so the short
    // wavelengths have already scattered out and what is left is red. This is
    // why every atmosphere has a warm horizon regardless of its zenith.
    let reddened = Vec3::new(1.0, 0.52, 0.28);
    // an airless world still has a horizon — it is the ground, seen edge-on
    let horizon = (reddened * sun * a * lit * 0.75) + ground * (1.0 - a) * 0.9;

    PlanetLook {
        ground,
        horizon,
        zenith,
        // fog needs air AND light to be visible at all: an airless world has
        // no haze, and a dark one has haze nobody can see
        fog_d: 0.0004 + a * lit * 0.0022,
        wet: match spec.surface {
            Surface::Ice => 0.55,
            Surface::Basalt => 0.12,
            _ => 0.05,
        },
    }
}

/// Earth's own system, at published insolation and pressure. The fiction
/// wants real worlds beside invented ones, and `exoworlds.rs` already
/// establishes that a named body uses catalogued numbers rather than a roll.
pub const SOL: &[(&str, PlanetSpec)] = &[
    ("MERCURY", PlanetSpec { star_temp_k: 5772.0, insolation: 6.67, atmosphere_bar: 0.0, surface: Surface::Regolith }),
    ("VENUS", PlanetSpec { star_temp_k: 5772.0, insolation: 1.91, atmosphere_bar: 92.0, surface: Surface::Basalt }),
    ("EARTH", PlanetSpec { star_temp_k: 5772.0, insolation: 1.00, atmosphere_bar: 1.0, surface: Surface::Verdant }),
    ("LUNA", PlanetSpec { star_temp_k: 5772.0, insolation: 1.00, atmosphere_bar: 0.0, surface: Surface::Regolith }),
    ("MARS", PlanetSpec { star_temp_k: 5772.0, insolation: 0.43, atmosphere_bar: 0.006, surface: Surface::Ferric }),
    ("EUROPA", PlanetSpec { star_temp_k: 5772.0, insolation: 0.037, atmosphere_bar: 0.0, surface: Surface::Ice }),
    ("TITAN", PlanetSpec { star_temp_k: 5772.0, insolation: 0.011, atmosphere_bar: 1.45, surface: Surface::Sand }),
    ("PLUTO", PlanetSpec { star_temp_k: 5772.0, insolation: 0.00095, atmosphere_bar: 0.00001, surface: Surface::Ice }),
];

/// An invented world from a seed. Physically plausible rather than uniform:
/// most stars are red dwarfs, most surfaces are rock, and thick atmospheres
/// are rarer than thin ones.
pub fn from_seed(seed: u64) -> PlanetSpec {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0xDA3E_39CB_94B9_5BDB);
    let mut next = || {
        s ^= s >> 30;
        s = s.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        s ^= s >> 27;
        ((s >> 11) as f32) / ((1u64 << 53) as f32)
    };
    let r = next();
    // the real initial mass function is bottom-heavy; so is this
    let star_temp_k = if r < 0.70 { 2600.0 + next() * 1200.0 } else if r < 0.93 { 4200.0 + next() * 1800.0 } else { 6000.0 + next() * 1800.0 };
    let a = next();
    let atmosphere_bar = if a < 0.45 { 0.0 } else if a < 0.75 { next() * 0.05 } else { 0.2 + next() * 2.5 };
    let surface = match (next() * 6.0) as u32 {
        0 => Surface::Basalt,
        1 | 2 => Surface::Regolith,
        3 => Surface::Ferric,
        4 => Surface::Sand,
        _ => Surface::Ice,
    };
    PlanetSpec {
        star_temp_k,
        insolation: (0.02f32).max(next().powf(2.2) * 4.0),
        atmosphere_bar,
        surface,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::valuestruct::{luma, Values, MIN_STOPS};

    fn values(l: &PlanetLook) -> Values {
        Values { ground: luma(l.ground.to_array()), horizon: luma(l.horizon.to_array()), sky: luma(l.zenith.to_array()) }
    }

    #[test]
    fn a_red_dwarf_is_red_and_an_f_star_is_blue_white() {
        let m = star_color(3000.0);
        let f = star_color(7000.0);
        assert!(m.x > m.z * 3.0, "a 3000 K star was not red: {m:?}");
        assert!(f.z > m.z * 2.0, "a 7000 K star was not bluer than a 3000 K one");
        // monotone in temperature across the stellar range, not just at ends
        let mut prev = -1.0f32;
        let mut t = 2500.0f32;
        while t <= 9000.0 {
            let b = star_color(t).z;
            assert!(b >= prev - 1e-4, "blue fell going up in temperature at {t} K");
            prev = b;
            t += 250.0;
        }
    }

    #[test]
    fn an_airless_world_has_a_black_sky_and_an_earthlike_one_does_not() {
        // The single most recognisable consequence of atmosphere. Getting it
        // backwards would make every generated world look the same.
        let luna = look(&SOL.iter().find(|(n, _)| *n == "LUNA").expect("LUNA").1);
        let earth = look(&SOL.iter().find(|(n, _)| *n == "EARTH").expect("EARTH").1);
        assert!(luma(luna.zenith.to_array()) < 0.005, "the Moon has a lit sky: {:?}", luna.zenith);
        assert!(
            luma(earth.zenith.to_array()) > luma(luna.zenith.to_array()) * 20.0,
            "Earth's sky is not meaningfully brighter than the Moon's"
        );
        // and Earth's sky is blue: more blue than red
        assert!(earth.zenith.z > earth.zenith.x * 1.5, "Earth's sky is not blue: {:?}", earth.zenith);
    }

    #[test]
    fn the_horizon_is_warmer_than_the_zenith_wherever_there_is_air() {
        // Rayleigh along a longer path. True on Earth, Titan and Venus alike,
        // and the reason a sky reads as a sky rather than a gradient.
        for (name, spec) in SOL.iter().filter(|(_, s)| s.atmosphere_bar > 0.1) {
            let l = look(spec);
            let hz_warm = l.horizon.x / l.horizon.z.max(1e-6);
            let zn_warm = l.zenith.x / l.zenith.z.max(1e-6);
            assert!(hz_warm > zn_warm, "{name}: horizon is not warmer than zenith");
        }
    }

    #[test]
    fn every_body_in_sol_has_value_structure() {
        // The property a generator must hold that a hand table only had to
        // pass once. If a derived world is flat, the derivation is wrong —
        // grading cannot recover contrast that is not in the scene.
        let mut flat = Vec::new();
        for (name, spec) in SOL {
            let v = values(&look(spec));
            if v.stops() < MIN_STOPS {
                flat.push(format!("{name}: {:.2} stops", v.stops()));
            }
        }
        assert!(flat.is_empty(), "flat worlds in Sol: {flat:?}");
    }

    #[test]
    fn a_thousand_random_worlds_are_all_composed() {
        // The whole difference between auditing a table and auditing a
        // generator. Any seed the game can roll must produce a frame with a
        // picture in it.
        let mut worst = (f32::MAX, 0u64);
        let mut flat = 0;
        for seed in 0..1000u64 {
            let v = values(&look(&from_seed(seed)));
            if v.stops() < worst.0 {
                worst = (v.stops(), seed);
            }
            if v.stops() < MIN_STOPS {
                flat += 1;
            }
        }
        assert_eq!(flat, 0, "{flat} of 1000 seeds produced a flat world; worst {:.2} stops at seed {}", worst.0, worst.1);
        // and the generator must not be producing ONE world a thousand times
        let a = look(&from_seed(7));
        let b = look(&from_seed(9001));
        assert!(
            (a.ground - b.ground).length() + (a.zenith - b.zenith).length() > 0.02,
            "two distant seeds produced the same planet"
        );
    }

    #[test]
    fn ground_albedo_comes_from_the_material_not_from_taste() {
        // MEADOW shipped grass at 0.456 luma; life is 0.06-0.15. This is that
        // lesson as a guard: no surface may return more light than the real
        // material does, whatever the lighting on top of it.
        for s in [Surface::Basalt, Surface::Regolith, Surface::Ferric, Surface::Sand, Surface::Ice, Surface::Verdant] {
            let a = luma(s.albedo().to_array());
            assert!(a > 0.0 && a < 0.70, "{}: albedo {a:.3} is not a real material", s.label());
        }
        assert!(
            luma(Surface::Verdant.albedo().to_array()) < 0.20,
            "vegetation is back above life's albedo"
        );
        assert!(
            luma(Surface::Basalt.albedo().to_array()) < luma(Surface::Ice.albedo().to_array()),
            "basalt is brighter than ice"
        );
    }
}
