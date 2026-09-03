//! PROCGEN — a world, its weather and a raid on it, from a seed.
//!
//! `tools/worldgen.py` proved the idea: a world's physics constrains which
//! chassis can be there, so a generated place is a filter rather than
//! decoration. But it lives in Python and the raid loop lives in Rust, so
//! the two could never meet at runtime. This is that generator in the engine,
//! producing an `Orbit` the star model already understands and a `Raid` the
//! raid loop already runs.
//!
//! **Nothing here invents a rule.** Gravity comes from a mass-radius guess,
//! temperature from `sun::Orbit`, the loot table from `loot::demanded`, the
//! foothold options from what the terrain allows. The generator's only job
//! is to pick a seed's worth of parameters and let the existing systems say
//! what that means.
//!
//! The one judgement it does make is REJECTION: a seed that produces a world
//! nothing can survive is regenerated rather than shipped, because a
//! destination the player cannot enter is a bug wearing procedural clothing.

use crate::foothold::{Approach, Site};
use crate::raid::Raid;
use crate::sun::{Orbit, StarClass, Surface};
use crate::survival::{habitable_fraction, Body, Bunker, Weather};

/// A generated place, before it is a raid.
#[derive(Clone, Copy, Debug)]
pub struct World {
    pub seed: u32,
    pub orbit: Orbit,
    pub weather: Weather,
    /// surface gravity in g, from a mass-radius guess rather than free choice
    pub gravity_g: f32,
    pub has_ground: bool,
}

fn rng(seed: u32) -> impl FnMut() -> f32 {
    // MIX AND WARM. A raw xorshift on a small seed correlates for its first
    // several draws — worldgen.py produced three heavy worlds in a row from
    // seeds 3, 11 and 42 before this was added, and it looked like a biased
    // distribution rather than a cold generator.
    let mut s = seed
        .wrapping_mul(2_654_435_761)
        .wrapping_add(0x9E37_79B9);
    s ^= s >> 16;
    s = s.wrapping_mul(0x45D9_F3B);
    s ^= s >> 16;
    if s == 0 {
        s = 0x1234_567;
    }
    let mut warm = 0;
    move || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        let v = (s >> 8) as f32 / 16_777_216.0;
        if warm < 8 {
            warm += 1;
            return {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s >> 8) as f32 / 16_777_216.0
            };
        }
        v
    }
}

impl World {
    pub fn generate(seed: u32) -> World {
        let mut r = rng(seed);
        let star = match (r() * 100.0) as u32 {
            0..=1 => StarClass::A,
            2..=9 => StarClass::F,
            10..=34 => StarClass::G,
            35..=64 => StarClass::K,
            _ => StarClass::M,
        };
        // ORBIT SCALED TO THE STAR. Sampling AU uniformly would put most
        // M-dwarf worlds in vacuum-cold and most A-star worlds in lava; what
        // is interesting is where a world sits relative to ITS OWN habitable
        // zone, so draw a multiple of that instead.
        let (_, _, lum) = star.spec();
        let hz = lum.sqrt().max(0.02);
        let au = (hz * (0.15 + r() * 3.5)).clamp(0.01, 60.0);
        let airless = r() < 0.45;
        let air = if airless { 0.0 } else { 10f32.powf(r() * 2.4 - 2.4) };
        let albedo = 0.08 + r() * 0.6;
        let orbit = Orbit { star, au, albedo, air };
        // gravity from a rocky mass-radius guess: bigger bodies hold more
        // air, so the two are correlated rather than rolled independently
        let gravity_g = (0.08 + air * 1.6 + r() * 1.4).clamp(0.05, 4.5);
        let has_ground = r() > 0.06;
        let weather = if !has_ground {
            Weather::Clear
        } else if orbit.flare_severity() > 0.45 && r() < 0.7 {
            Weather::FlareFront
        } else if air < 0.02 {
            // no air, no storms — the terminator sweep is what is left
            if r() < 0.4 { Weather::ThermalFront } else { Weather::Clear }
        } else if air > 0.4 && r() < 0.35 {
            Weather::GiantWave
        } else if r() < 0.45 {
            Weather::DustStorm
        } else {
            Weather::Clear
        };
        World { seed, orbit, weather, gravity_g, has_ground }
    }

    /// Can anything the campaign fields actually operate here?
    pub fn viable(&self) -> bool {
        if self.orbit.surface() == Surface::Molten {
            return false;
        }
        habitable_fraction(Body::Machine, &self.orbit) > 0.0
    }

    /// Generate until a seed produces somewhere enterable. A destination the
    /// player cannot enter is a bug wearing procedural clothing.
    pub fn generate_viable(seed: u32) -> World {
        let mut s = seed;
        for _ in 0..64 {
            let w = World::generate(s);
            if w.viable() {
                return w;
            }
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        }
        // fall back to somewhere dull rather than looping: a quiet K-dwarf
        // world is a worse destination than a strange one and a better one
        // than none
        World {
            seed,
            orbit: Orbit { star: StarClass::K, au: 0.6, albedo: 0.3, air: 0.3 },
            weather: Weather::Clear,
            gravity_g: 0.9,
            has_ground: true,
        }
    }

    pub fn descriptor(&self) -> &'static str {
        match self.orbit.surface() {
            Surface::Molten => "MOLTEN",
            Surface::Scorched => "SCORCHED",
            Surface::Temperate => "TEMPERATE",
            Surface::Frozen => "FROZEN",
            Surface::Cryogenic => "CRYOGENIC",
        }
    }
}

/// Build a raid on a generated world.
///
/// Sites reflect the ground: an airless rock has fewer structures worth
/// seizing, a temperate world with atmosphere has more. Encounter count and
/// boss tier scale with how hostile the place is, so a hard world is hard in
/// its content as well as its sky.
pub fn raid_for(w: &World, name: &'static str) -> Raid {
    let mut r = rng(w.seed ^ 0x5EED);
    let hazard = w.orbit.flare_severity().max(match w.orbit.surface() {
        Surface::Molten => 1.0,
        Surface::Scorched | Surface::Cryogenic => 0.6,
        Surface::Frozen => 0.4,
        Surface::Temperate => 0.15,
    });
    let mut sites = Vec::new();
    if w.has_ground {
        sites.push(Site {
            approach: Approach::ClearLz,
            at: Bunker { distance_m: 120.0 + r() * 400.0, shielded: false },
            garrison: 0.2 + r() * 0.4,
            take_s: 40.0 + r() * 90.0,
        });
        // somewhere worth seizing only exists where somebody could live
        if w.orbit.air > 0.02 || hazard > 0.5 {
            sites.push(Site {
                approach: Approach::SeizeFortress,
                at: Bunker { distance_m: 300.0 + r() * 900.0, shielded: true },
                garrison: 0.8 + r() * 0.8,
                take_s: 120.0 + r() * 240.0,
            });
        }
    } else {
        // no ground: the only foothold is the ship itself
        sites.push(Site {
            approach: Approach::ClearLz,
            at: Bunker { distance_m: 60.0 + r() * 120.0, shielded: false },
            garrison: 0.3,
            take_s: 30.0 + r() * 40.0,
        });
    }
    let n = 2 + (hazard * 3.0) as usize;
    let encounters = (0..n).map(|i| 60.0 + r() * 80.0 + i as f32 * 10.0).collect();
    Raid {
        name,
        orbit: w.orbit,
        weather: w.weather,
        sites,
        encounters,
        boss_tier: (hazard * 3.0).round().clamp(0.0, 3.0) as u8,
        extract_m: 300.0 + r() * 1_200.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raid::{run, Phase};

    #[test]
    fn generated_worlds_actually_vary() {
        // three heavy worlds in a row is what a cold generator looks like
        let ws: Vec<World> = (1..60).map(World::generate).collect();
        let mut kinds = std::collections::BTreeSet::new();
        let mut weathers = std::collections::BTreeSet::new();
        for w in &ws {
            kinds.insert(w.descriptor());
            weathers.insert(w.weather.label());
        }
        assert!(kinds.len() >= 3, "only {} surface kinds in 59 worlds", kinds.len());
        assert!(weathers.len() >= 3, "only {} weathers", weathers.len());
        let g: Vec<f32> = ws.iter().map(|w| w.gravity_g).collect();
        let lo = g.iter().cloned().fold(f32::MAX, f32::min);
        let hi = g.iter().cloned().fold(f32::MIN, f32::max);
        assert!(hi > lo * 3.0, "gravity spanned {lo:.2} to {hi:.2}");
    }

    #[test]
    fn orbits_are_scaled_to_their_star() {
        // THE TEMPERATE BAND IS GENUINELY NARROW and I asserted the wrong
        // thing first. Scaling to the habitable zone does not make temperate
        // worlds common — only two M-dwarf worlds in 199 land there, and
        // that is realistic: the band spans about 0.4 to 0.7 of the HZ
        // distance out of a 0.15-to-3.65 sample.
        //
        // What the scaling actually buys is that M-dwarf worlds are not ALL
        // frozen and bright-star worlds are not all molten — the spread
        // follows the star instead of the same AU range being applied to
        // both. That is the property worth pinning.
        let mut m_kinds = std::collections::BTreeSet::new();
        let mut bright_kinds = std::collections::BTreeSet::new();
        for s in 1..400u32 {
            let w = World::generate(s);
            match w.orbit.star {
                StarClass::M => {
                    m_kinds.insert(w.descriptor());
                }
                StarClass::F | StarClass::A | StarClass::G => {
                    bright_kinds.insert(w.descriptor());
                }
                _ => {}
            }
        }
        assert!(m_kinds.len() >= 3, "M-dwarf worlds were all one kind: {m_kinds:?}");
        assert!(bright_kinds.len() >= 3, "bright-star worlds were all one kind: {bright_kinds:?}");
        // and neither population is entirely at one extreme
        assert!(m_kinds.contains("SCORCHED") || m_kinds.contains("TEMPERATE"),
            "every M-dwarf world was cold: {m_kinds:?}");
    }

    #[test]
    fn a_world_nobody_can_enter_is_regenerated_rather_than_shipped() {
        for s in 1..120u32 {
            let w = World::generate_viable(s);
            assert!(w.viable(), "seed {s} shipped an unenterable world");
            assert_ne!(w.orbit.surface(), Surface::Molten);
        }
    }

    #[test]
    fn generation_is_deterministic() {
        for s in [1u32, 99, 12345] {
            let a = World::generate(s);
            let b = World::generate(s);
            assert_eq!(a.orbit.au, b.orbit.au);
            assert_eq!(a.weather.label(), b.weather.label());
        }
    }

    #[test]
    fn airless_worlds_get_no_storms_and_no_waves() {
        // no air, no weather that needs air to exist
        for s in 1..200u32 {
            let w = World::generate(s);
            if w.orbit.air < 0.02 && w.has_ground {
                assert!(
                    !matches!(w.weather, Weather::DustStorm | Weather::GiantWave),
                    "seed {s}: {} on an airless world",
                    w.weather.label()
                );
            }
        }
    }

    #[test]
    fn a_hostile_world_is_hard_in_its_content_too() {
        // otherwise a brutal sky over a trivial raid reads as unfair rather
        // than as difficult
        let mut easy_tier = 0u32;
        let mut hard_tier = 0u32;
        let mut easy_n = 0u32;
        let mut hard_n = 0u32;
        for s in 1..160u32 {
            let w = World::generate_viable(s);
            let r = raid_for(&w, "GEN");
            let hostile = w.orbit.flare_severity() > 0.4
                || matches!(w.orbit.surface(), Surface::Scorched | Surface::Cryogenic);
            if hostile {
                hard_tier += r.boss_tier as u32;
                hard_n += 1;
            } else {
                easy_tier += r.boss_tier as u32;
                easy_n += 1;
            }
        }
        assert!(hard_n > 0 && easy_n > 0, "no spread of hostility at all");
        let ha = hard_tier as f32 / hard_n as f32;
        let ea = easy_tier as f32 / easy_n as f32;
        assert!(ha > ea, "hostile worlds averaged tier {ha:.2} vs {ea:.2}");
    }

    #[test]
    fn a_generated_raid_is_runnable_end_to_end() {
        // the whole point: seed in, payout out, with no authoring anywhere
        let mut extracted = 0;
        let mut paid = 0;
        for s in 1..90u32 {
            let w = World::generate_viable(s);
            let r = raid_for(&w, "GEN");
            let out = run(&r, Body::Machine, 14.0, s, &[Body::Machine]);
            if out.phase == Phase::Extracted {
                extracted += 1;
                if out.drop.is_some() {
                    paid += 1;
                }
            }
        }
        assert!(extracted > 10, "only {extracted} of 89 generated raids were finishable");
        assert_eq!(extracted, paid, "a finished raid paid nothing");
    }

    #[test]
    fn no_ground_means_the_ship_is_the_only_foothold() {
        for s in 1..300u32 {
            let w = World::generate(s);
            if !w.has_ground {
                let r = raid_for(&w, "VOID");
                assert!(r.sites.iter().all(|x| x.approach == Approach::ClearLz),
                    "seed {s} put a fortress in open space");
                return;
            }
        }
    }
}
