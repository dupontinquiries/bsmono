//! `sim::fauna_roster` (r460) — the catalogued animal species as data (SECTION_fauna §3a).
//!
//! Encodes the 20 species from `fauna_behaviors.docx` as a table the existing fauna cores read:
//! [`Temperament`] (→ `fauna_behavior` FSM filter), [`BodyPlan`] (→ `fauna_gait` proportions/gait),
//! plus home-range, detection radius and base health for spawn/AI. Pure data; no behaviour here.
//! Drops, habitat weights and per-species FSM specializations (§3a tail / §3b) layer on top later.

use crate::sim::fauna_behavior::Temperament;
use crate::sim::fauna_gait::BodyPlan;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Species {
    // Passive
    Deer, Rabbit, Raven, CaveSalamander, RiverOtter, StoneTortoise, Jellyfish, CaveFish,
    // Neutral
    BrownBear, Moose, WildBoar, Wolf,
    // Hostile
    CaveTroll, Harpy, StoneCrab, SwampLeech, Basilisk,
    // Ecological specialists
    TermiteColony, Firefly, BioluminescentShrimp,
}

#[derive(Clone, Copy, Debug)]
pub struct SpeciesData {
    pub name: &'static str,
    pub latin: &'static str,
    pub temperament: Temperament,
    pub body_plan: BodyPlan,
    pub home_range_blocks: u16,
    pub detect_radius_blocks: u16,
    pub base_health: i16,
}

pub const ALL: [Species; 20] = [
    Species::Deer, Species::Rabbit, Species::Raven, Species::CaveSalamander, Species::RiverOtter,
    Species::StoneTortoise, Species::Jellyfish, Species::CaveFish, Species::BrownBear, Species::Moose,
    Species::WildBoar, Species::Wolf, Species::CaveTroll, Species::Harpy, Species::StoneCrab,
    Species::SwampLeech, Species::Basilisk, Species::TermiteColony, Species::Firefly,
    Species::BioluminescentShrimp,
];

/// Static data for a species. Numbers seeded from `fauna_behaviors.docx` where given (e.g. deer home
/// range 60 / alert 25) and sensible defaults otherwise — initial values, tune against the doc.
pub fn data(s: Species) -> SpeciesData {
    use BodyPlan::*;
    use Species as S;
    use Temperament::*;
    macro_rules! d {
        ($n:expr,$l:expr,$t:expr,$b:expr,$hr:expr,$dr:expr,$hp:expr) => {
            SpeciesData { name: $n, latin: $l, temperament: $t, body_plan: $b,
                home_range_blocks: $hr, detect_radius_blocks: $dr, base_health: $hp }
        };
    }
    match s {
        S::Deer            => d!("Deer", "Cervidae odocoileus", Passive, Quadruped, 60, 25, 14),
        S::Rabbit          => d!("Rabbit", "Leporidae sylvestris", Passive, Quadruped, 16, 12, 4),
        S::Raven           => d!("Crow / Raven", "Corvidae memorans", Passive, Avian, 80, 30, 6),
        S::CaveSalamander  => d!("Cave Salamander", "Caudata bioluminans", Passive, Quadruped, 20, 10, 6),
        S::RiverOtter      => d!("River Otter", "Lutrinae ludens", Passive, Quadruped, 40, 18, 10),
        S::StoneTortoise   => d!("Stone Tortoise", "Testudines antiquus", Passive, Quadruped, 24, 12, 30),
        S::Jellyfish       => d!("Bioluminescent Jellyfish", "Medusozoa lumina", Passive, Aquatic, 30, 8, 4),
        S::CaveFish        => d!("Cave Fish", "Anoptichthys memoris", Passive, Aquatic, 24, 10, 4),
        S::BrownBear       => d!("Brown Bear", "Ursus contextus", Neutral, Quadruped, 120, 28, 60),
        S::Moose           => d!("Moose", "Alces alces rex", Neutral, Quadruped, 100, 30, 50),
        S::WildBoar        => d!("Wild Boar", "Sus sylvestris", Neutral, Quadruped, 70, 22, 30),
        S::Wolf            => d!("Wolf", "Canis lupus gregarius", Neutral, Quadruped, 150, 32, 24),
        S::CaveTroll       => d!("Cave Troll", "Trollus saxum", Hostile, Biped, 40, 20, 90),
        S::Harpy           => d!("Harpy", "Harpyia rupestris", Hostile, Avian, 120, 35, 28),
        S::StoneCrab       => d!("Stone Crab", "Lithodes armatus", Hostile, Quadruped, 24, 14, 30),
        S::SwampLeech      => d!("Swamp Leech", "Hirudinea raptor", Hostile, Serpentine, 16, 10, 10),
        S::Basilisk        => d!("Basilisk", "Basiliscus rex", Hostile, Serpentine, 90, 30, 70),
        S::TermiteColony   => d!("Termite Colony", "Isoptera arboricola", Specialist, Quadruped, 30, 8, 8),
        S::Firefly         => d!("Firefly", "Lampyridae vagans", Specialist, Avian, 24, 6, 2),
        S::BioluminescentShrimp => d!("Bioluminescent Shrimp", "Sergestidae lucens", Specialist, Aquatic, 20, 6, 2),
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn roster_is_twenty() { assert_eq!(ALL.len(), 20); }
    #[test] fn every_species_has_data() {
        for s in ALL { let d = data(s); assert!(!d.name.is_empty() && d.base_health > 0 && d.home_range_blocks > 0); }
    }
    #[test] fn temperament_counts_match_doc() {
        let (mut p, mut n, mut h, mut sp) = (0, 0, 0, 0);
        for s in ALL {
            match data(s).temperament {
                Temperament::Passive => p += 1,
                Temperament::Neutral => n += 1,
                Temperament::Hostile => h += 1,
                Temperament::Specialist => sp += 1,
            }
        }
        assert_eq!((p, n, h, sp), (8, 4, 5, 3));
    }
}

// ── habitat: which species live in which biome (SECTION_fauna §3a habitat weights) ──
use crate::world::biomes::Biome;

/// Spawn weights for a biome — `(species, weight)` pairs for `mob_spawn_table::pick`-style selection.
/// Empty for the most barren biomes. Aquatic species only appear in watery biomes; cave fauna only
/// underground. Pure data; tune freely.
pub fn habitat_weights(biome: Biome) -> Vec<(Species, f32)> {
    use Biome::*;
    use Species as S;
    match biome {
        Plains | Savanna | HighlandSteppe => vec![
            (S::Deer, 1.0), (S::Rabbit, 1.0), (S::Raven, 0.6), (S::WildBoar, 0.5),
            (S::Wolf, 0.4), (S::Moose, 0.2),
        ],
        TemperateForest | BirchForest | MapleForest | CloudForest => vec![
            (S::Deer, 1.0), (S::Rabbit, 0.8), (S::Raven, 0.5), (S::BrownBear, 0.4),
            (S::WildBoar, 0.6), (S::Wolf, 0.5),
        ],
        Taiga => vec![(S::Deer, 0.8), (S::Wolf, 0.7), (S::BrownBear, 0.5), (S::Raven, 0.5), (S::Moose, 0.4)],
        Jungle | Mangrove | Swamp => vec![
            (S::WildBoar, 0.7), (S::SwampLeech, 0.6), (S::Raven, 0.4), (S::RiverOtter, 0.5),
            (S::Basilisk, 0.3),
        ],
        Desert | Mesa | RedSandstoneCanyon | BloomingCanyon | VolcanicWastes => vec![
            (S::Raven, 0.4), (S::Basilisk, 0.3), (S::StoneCrab, 0.2),
        ],
        Ocean | FrozenOcean | Beach => vec![
            (S::Jellyfish, 0.7), (S::RiverOtter, 0.5), (S::StoneCrab, 0.6),
            (S::BioluminescentShrimp, 0.4),
        ],
        Tundra => vec![(S::Wolf, 0.6), (S::Raven, 0.5), (S::Moose, 0.3)],
        MistyMountains | AlpineHighlands | TemperateHighlands | Skylands => vec![
            (S::Harpy, 0.6), (S::Raven, 0.6), (S::Deer, 0.4), (S::Wolf, 0.4), (S::Moose, 0.2),
        ],
        IceCave | FungalCave | ThermalCave | CrystalCave | LushCave => vec![
            (S::CaveSalamander, 0.7), (S::CaveFish, 0.6), (S::CaveTroll, 0.3),
            (S::Firefly, 0.5), (S::BioluminescentShrimp, 0.4),
        ],
    }
}

/// Pick a species for a biome from its habitat weights. `roll` in `[0,1)`. None if the biome is barren.
pub fn pick_for_biome(biome: Biome, roll: f32) -> Option<Species> {
    let w = habitat_weights(biome);
    if w.is_empty() {
        return None;
    }
    let total: f32 = w.iter().map(|(_, x)| *x).sum();
    if total <= 0.0 {
        return None;
    }
    let mut acc = 0.0;
    let target = roll.clamp(0.0, 0.999) * total;
    for (s, x) in &w {
        acc += *x;
        if target < acc {
            return Some(*s);
        }
    }
    w.last().map(|(s, _)| *s)
}

#[cfg(test)]
mod habitat_tests {
    use super::*;
    use crate::world::biomes::Biome;
    #[test]
    fn aquatic_only_in_water() {
        // Jellyfish (Aquatic) should appear in Ocean weights but not in Plains weights.
        let ocean: Vec<_> = habitat_weights(Biome::Ocean).into_iter().map(|(s, _)| s).collect();
        let plains: Vec<_> = habitat_weights(Biome::Plains).into_iter().map(|(s, _)| s).collect();
        assert!(ocean.contains(&Species::Jellyfish));
        assert!(!plains.contains(&Species::Jellyfish));
    }
    #[test]
    fn caves_have_cave_fauna() {
        let cave: Vec<_> = habitat_weights(Biome::LushCave).into_iter().map(|(s, _)| s).collect();
        assert!(cave.contains(&Species::CaveSalamander));
    }
    #[test]
    fn pick_is_deterministic_and_in_habitat() {
        for r in [0.0_f32, 0.3, 0.7, 0.99] {
            let s = pick_for_biome(Biome::Plains, r).unwrap();
            let habitat: Vec<_> = habitat_weights(Biome::Plains).into_iter().map(|(x, _)| x).collect();
            assert!(habitat.contains(&s));
        }
    }
}
