//! Ecosystem placement — *where* creatures live, and a deterministic roll for
//! seeding them as the world generates.
//!
//! Pure and seed-deterministic: the same `(cell, seed)` always yields the same
//! decision, so a region repopulates identically on reload and across clients.
//! [`habitat_weight`] is the design surface — it says how densely each creature
//! belongs in each biome (0 = never here) — and [`roll_spawn`] turns that table
//! plus a rarity gate into "does a creature appear at this surface cell, and
//! which one". The world-gen / chunk-populate path calls `roll_spawn` per
//! candidate surface cell; registering the returned creature (via
//! [`crate::spawn::spawn`]) and wiring its brain is engine integration.

use crate::item::CreatureKind;
use crate::world::biomes::Biome;

/// How densely `kind` populates `biome`: 0 = absent, higher = more common. This
/// is the habitat design table — read it as "the Späher is a treeline bird, so
/// it's thick in forests, present on plains and highlands, rare at the harsh
/// edges, and never underground or at sea."
pub fn habitat_weight(kind: CreatureKind, biome: Biome) -> u16 {
    match kind {
        CreatureKind::Spaeher => match biome {
            Biome::TemperateForest | Biome::BirchForest | Biome::MapleForest | Biome::CloudForest | Biome::Jungle | Biome::Taiga => 8,
            Biome::Plains | Biome::Beach | Biome::Swamp => 4,
            Biome::TemperateHighlands | Biome::AlpineHighlands | Biome::MistyMountains | Biome::Skylands => 3,
            Biome::Tundra | Biome::RedSandstoneCanyon | Biome::BloomingCanyon => 1,
            // Caves, ocean, deep desert: no treeline sentinel here.
            _ => 0,
        },
        // The Lauscher ("listener") — an air-attuned creature that SEEKS CAVES to
        // live in: common underground, only a rare wanderer on the surface.
        CreatureKind::Lauscher => match biome {
            b if b.is_cave_biome() => 6,           // its home
            Biome::Plains | Biome::Beach | Biome::Savanna => 1, // rare surface wanderer
            b if b.is_forest() => 1,
            _ => 0,
        }
        // The dragon haunts the heights and the canyons, sparsely.
        CreatureKind::Dragon => match biome {
            Biome::MistyMountains | Biome::AlpineHighlands | Biome::Skylands => 2,
            b if b.is_canyon() => 1,
            _ => 0,
        },
        // Generic wild fauna — the staple of the overworld. Thick in the
        // temperate lowlands and forests, present across most surface biomes,
        // thin at the harsh edges, and never underground or at sea.
        CreatureKind::Fauna => match biome {
            Biome::Plains | Biome::Beach => 9,
            Biome::Savanna => 9,                  // open grassland teems with grazers
            b if b.is_forest() => 8,              // includes Jungle + Taiga now
            Biome::Swamp | Biome::BloomingCanyon => 6,
            Biome::TemperateHighlands | Biome::AlpineHighlands | Biome::Skylands => 4,
            Biome::Desert | Biome::RedSandstoneCanyon | Biome::Tundra => 2,
            // Caves and open ocean: no everyday surface fauna.
            _ => 0,
        },
        // Great reptilian megafauna — removed from the ambient overworld (operator
        // request: the oversized long-necks crowded the surface). The kind still
        // exists for spawn eggs + observation mode; it just no longer ambient-spawns.
        CreatureKind::Dinosaur => 0,
    }
}

/// Default rarity gate: roughly one in this many eligible cells even attempts a
/// spawn, keeping populations sparse. Larger = rarer.
pub const DEFAULT_RARITY: u32 = 64;

/// Deterministic hash of a surface cell + world seed → 64 bits.
fn hash_cell(x: i32, z: i32, seed: u64) -> u64 {
    // splitmix64-style mixing of the packed coordinates with the seed.
    let mut h = seed
        ^ (x as u32 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (z as u32 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F).rotate_left(31);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    h
}

/// Decide whether a creature spawns at surface cell `(x, z)` in `biome`, and
/// which one — weighted by [`habitat_weight`], gated by `rarity` (one in
/// `rarity` eligible cells attempts). Returns `None` in unsuitable biomes, on
/// the rarity gate, or where nothing has a habitat here. Deterministic in
/// `(x, z, seed)`.
pub fn roll_spawn(biome: Biome, x: i32, z: i32, seed: u64, rarity: u32) -> Option<CreatureKind> {
    let total: u32 = CreatureKind::ALL.iter().map(|&k| habitat_weight(k, biome) as u32).sum();
    if total == 0 {
        return None;
    }
    let rarity = rarity.max(1);
    let h = hash_cell(x, z, seed);
    if (h % rarity as u64) != 0 {
        return None;
    }
    // Weighted pick among the kinds present in this biome.
    let pick = (hash_cell(x, z, seed ^ 0xA5A5_5A5A_0F0F_F0F0) % total as u64) as u32;
    let mut acc = 0u32;
    for &k in &CreatureKind::ALL {
        acc += habitat_weight(k, biome) as u32;
        if pick < acc {
            return Some(k);
        }
    }
    None
}

/// A creature world-gen decided to place: its kind and spawn position (block
/// centre, one block above the surface).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spawned {
    pub kind: CreatureKind,
    pub pos: [f32; 3],
}

/// Walk a rectangular block of `w` by `h` surface columns starting at
/// `(x0, z0)` and collect the creatures to seed there. `biome_at` and `surface_y` are the
/// world-gen lookups the caller already has; each spawn sits centred one block
/// above the surface. Pure + deterministic — the chunk-populate path calls this
/// and registers each [`Spawned`] via [`crate::spawn::spawn`].
pub fn populate_region(
    x0: i32,
    z0: i32,
    w: i32,
    h: i32,
    seed: u64,
    rarity: u32,
    biome_at: impl Fn(i32, i32) -> Biome,
    surface_y: impl Fn(i32, i32) -> i32,
) -> Vec<Spawned> {
    let mut out = Vec::new();
    for dz in 0..h {
        for dx in 0..w {
            let (x, z) = (x0 + dx, z0 + dz);
            if let Some(kind) = roll_spawn(biome_at(x, z), x, z, seed, rarity) {
                let y = surface_y(x, z);
                out.push(Spawned { kind, pos: [x as f32 + 0.5, y as f32 + 1.0, z as f32 + 0.5] });
            }
        }
    }
    out
}

/// Collect ecosystem spawns in a `radius`-block box around `(cx, cz)`, sampled on
/// a coarse `stride`-spaced grid so the count stays small, and capped at `max`.
/// Each spawn sits one block above the sampled surface. Pure — the live world
/// passes the real worldgen's `biome_at` / `surface_y` (e.g. `WorldGenerator`'s
/// `biome_at_3d` and `surface_y_at`). This is the bridge for scattering a bounded
/// ambient population near the player without overrunning the per-frame entity
/// bake; the chunk-streaming version (spawn as new chunks load) layers on top.
pub fn spawns_around<B, S>(
    cx: i32,
    cz: i32,
    radius: i32,
    stride: i32,
    seed: u64,
    rarity: u32,
    max: usize,
    biome_at: B,
    surface_y: S,
) -> Vec<Spawned>
where
    B: Fn(i32, i32) -> Biome,
    S: Fn(i32, i32) -> i32,
{
    let mut out = Vec::new();
    let stride = stride.max(1);
    let mut z = cz - radius;
    while z <= cz + radius {
        let mut x = cx - radius;
        while x <= cx + radius {
            if let Some(kind) = roll_spawn(biome_at(x, z), x, z, seed, rarity) {
                let y = surface_y(x, z);
                out.push(Spawned {
                    kind,
                    pos: [x as f32 + 0.5, y as f32 + 1.0, z as f32 + 0.5],
                });
                if out.len() >= max {
                    return out;
                }
            }
            x += stride;
        }
        z += stride;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaeher_lives_in_forests_not_caves_or_sea() {
        assert!(habitat_weight(CreatureKind::Spaeher, Biome::TemperateForest) > 0);
        assert!(habitat_weight(CreatureKind::Spaeher, Biome::BirchForest) > 0);
        assert!(
            habitat_weight(CreatureKind::Spaeher, Biome::TemperateForest)
                > habitat_weight(CreatureKind::Spaeher, Biome::AlpineHighlands),
            "denser in forest than on cold highlands"
        );
        assert_eq!(habitat_weight(CreatureKind::Spaeher, Biome::Ocean), 0);
        assert_eq!(habitat_weight(CreatureKind::Spaeher, Biome::FungalCave), 0);
    }

    #[test]
    fn lauscher_lives_on_the_surface_and_in_caves() {
        assert!(habitat_weight(CreatureKind::Lauscher, Biome::IceCave) > 0);
        assert!(habitat_weight(CreatureKind::Lauscher, Biome::FungalCave) > 0);
        // restored to the overworld (the air-attuned listener)
        assert!(habitat_weight(CreatureKind::Lauscher, Biome::Plains) > 0);
        assert!(habitat_weight(CreatureKind::Lauscher, Biome::TemperateForest) > 0);
        assert_eq!(habitat_weight(CreatureKind::Lauscher, Biome::Ocean), 0);
    }

    #[test]
    fn roll_is_deterministic() {
        let a = roll_spawn(Biome::TemperateForest, 12, -7, 99, DEFAULT_RARITY);
        let b = roll_spawn(Biome::TemperateForest, 12, -7, 99, DEFAULT_RARITY);
        assert_eq!(a, b, "same cell + seed → same decision");
    }

    #[test]
    fn empty_habitat_biomes_never_spawn() {
        // Ocean has zero weight for every creature.
        for x in 0..200 {
            assert_eq!(roll_spawn(Biome::Ocean, x, x * 3, 7, 1), None);
        }
    }

    #[test]
    fn a_cave_yields_only_the_listener_and_woods_hold_no_megafauna() {
        // rarity 1 = every cell attempts, so we exercise the weighted pick fully.
        for x in 0..400 {
            if let Some(k) = roll_spawn(Biome::FungalCave, x, 0, 5, 1) {
                assert_eq!(k, CreatureKind::Lauscher, "only the listener underground");
            }
            // forests now hold birds, fauna AND the surface listener — but never
            // the (removed) ambient megafauna.
            if let Some(k) = roll_spawn(Biome::TemperateForest, x, 1, 5, 1) {
                assert_ne!(k, CreatureKind::Dinosaur, "ambient dinosaurs removed");
            }
        }
    }

    #[test]
    fn forests_are_populated_and_rarity_thins_them() {
        let count = |rarity: u32| {
            (0..2000)
                .filter(|&x| roll_spawn(Biome::TemperateForest, x, 0, 42, rarity).is_some())
                .count()
        };
        let dense = count(4);
        let sparse = count(256);
        assert!(dense > 0, "forests should hold birds");
        assert!(dense > sparse, "a larger rarity divisor thins the population");
    }

    #[test]
    fn populate_region_places_surface_life_in_a_forest_above_the_surface() {
        // A uniform forest at a flat surface height of 64.
        let spawns = populate_region(0, 0, 48, 48, 2026, 8, |_, _| Biome::TemperateForest, |_, _| 64);
        assert!(!spawns.is_empty(), "a forest region should seed some creatures");
        for s in &spawns {
            // Forests now hold the treeline bird, general fauna AND the surface
            // listener — but never the (removed) ambient megafauna.
            assert_ne!(s.kind, CreatureKind::Dinosaur, "ambient dinosaurs removed");
            assert_eq!(s.pos[1], 65.0, "spawns rest one block above the surface");
        }
        // An ocean region of the same size seeds nothing.
        let sea = populate_region(0, 0, 48, 48, 2026, 8, |_, _| Biome::Ocean, |_, _| 1);
        assert!(sea.is_empty());
    }

    #[test]
    fn fauna_fills_the_overworld_not_caves_or_ocean() {
        assert!(habitat_weight(CreatureKind::Fauna, Biome::Plains) > 0);
        assert!(habitat_weight(CreatureKind::Fauna, Biome::TemperateForest) > 0);
        assert!(
            habitat_weight(CreatureKind::Fauna, Biome::Plains)
                > habitat_weight(CreatureKind::Fauna, Biome::Tundra),
            "denser in the lowlands than the frozen edge"
        );
        assert_eq!(habitat_weight(CreatureKind::Fauna, Biome::Ocean), 0);
        assert_eq!(habitat_weight(CreatureKind::Fauna, Biome::IceCave), 0);
        assert_eq!(habitat_weight(CreatureKind::Fauna, Biome::FungalCave), 0);
    }

    #[test]
    fn dinosaurs_are_removed_from_ambient_spawns() {
        // The megafauna kind still exists (spawn eggs / observation mode) but no
        // biome ambient-spawns it anymore.
        assert_eq!(habitat_weight(CreatureKind::Dinosaur, Biome::Swamp), 0);
        assert_eq!(habitat_weight(CreatureKind::Dinosaur, Biome::Plains), 0);
        assert_eq!(habitat_weight(CreatureKind::Dinosaur, Biome::Tundra), 0);
        assert_eq!(habitat_weight(CreatureKind::Dinosaur, Biome::Ocean), 0);
    }

    #[test]
    fn the_overworld_scatters_a_mix_of_kinds() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        // rarity 1 = every cell attempts, so the weighted pick is fully exercised.
        for x in 0..600 {
            if let Some(k) = roll_spawn(Biome::Plains, x, 11, 4242, 1) {
                seen.insert(k);
            }
        }
        assert!(seen.contains(&CreatureKind::Fauna), "fauna roam the plains");
        assert!(seen.contains(&CreatureKind::Lauscher), "and the little listeners");
        // No ambient megafauna, and the peak dragon never leaks onto open plains.
        assert!(!seen.contains(&CreatureKind::Dinosaur), "no ambient megafauna anymore");
        assert!(!seen.contains(&CreatureKind::Dragon), "no peak/canyon dragon on the plains");
    }

    #[test]
    fn each_biome_seeds_its_own_residents() {
        // Cave on the left half, open plains on the right.
        let spawns = populate_region(0, 0, 40, 40, 7, 1, |x, _z| if x < 20 { Biome::FungalCave } else { Biome::Plains }, |_, _| 64);
        assert!(!spawns.is_empty());
        let mut cave_kinds = std::collections::HashSet::new();
        let mut plain_kinds = std::collections::HashSet::new();
        for s in &spawns {
            if s.pos[0] < 20.5 {
                cave_kinds.insert(s.kind);
            } else {
                plain_kinds.insert(s.kind);
            }
        }
        assert!(cave_kinds.iter().all(|&k| k == CreatureKind::Lauscher), "only listeners underground");
        assert!(plain_kinds.contains(&CreatureKind::Fauna), "fauna out on the plains");
        // listeners now also roam the daylight surface (no longer cave-bound)
    }

    #[test]
    fn spawns_around_caps_and_sits_above_the_surface() {
        // Uniform plains, flat surface at y=70, rarity 1 so cells fire freely.
        let got = spawns_around(0, 0, 200, 1, 7, 1, 25, |_, _| Biome::Plains, |_, _| 70);
        assert_eq!(got.len(), 25, "the population is capped");
        for s in &got {
            assert_eq!(s.pos[1], 71.0, "one block above the surface");
            assert_ne!(s.kind, CreatureKind::Dinosaur, "ambient dinosaurs removed");
        }
    }

    #[test]
    fn spawns_around_respects_biome_and_is_deterministic() {
        // Ocean has zero habitat for everyone → nothing spawns.
        let sea = spawns_around(0, 0, 64, 2, 7, 1, 100, |_, _| Biome::Ocean, |_, _| 1);
        assert!(sea.is_empty());
        // Same inputs → same scatter.
        let a = spawns_around(10, -4, 80, 3, 99, 8, 40, |_, _| Biome::Plains, |_, _| 64);
        let b = spawns_around(10, -4, 80, 3, 99, 8, 40, |_, _| Biome::Plains, |_, _| 64);
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(&b).all(|(p, q)| p.kind == q.kind && p.pos == q.pos));
    }
}
