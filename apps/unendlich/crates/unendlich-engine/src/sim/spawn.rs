//! Deterministic mob spawning.
//!
//! Turns "mob types exist" into "the world is populated". Given a world seed, a
//! square region of columns, and the world generator (for surface height +
//! sea level), this produces a deterministic set of [`Mob`]s placed on the
//! ground. Same `(seed, region)` → same mobs, every time and on every peer, so
//! it's rollback- and netcode-safe like the rest of the sim.
//!
//! Population shape (v1, intentionally simple — biome weighting is a follow-on):
//!   - Each region rolls a small mob budget from `hash(seed, region)`.
//!   - Each mob picks a column in the region, samples the surface, and spawns
//!     one block above it — but only on *dry land* (surface at or above sea
//!     level). Underwater columns are skipped so we don't spawn walkers in the
//!     ocean.
//!   - Kind is weighted toward [`MobKind::Grazer`] (prey common) with an
//!     occasional [`MobKind::Seeker`] (predator rare), so a populated region
//!     reads as a herd with the odd hunter — the predator/prey dynamic the
//!     Grazer flee behavior was built for.
//!
//! Rendering is a separate concern (no GPU pipeline consumes these yet); this
//! module is the population source the renderer + sim will draw from.

use crate::sim::fixed::{Fixed, Vec3};
use crate::sim::mob::Mob;
use crate::world::cpu_generator::WorldGenerator;

/// Side length (in columns/blocks) of a spawn region.
pub const REGION_SIZE: i32 = 64;

/// Hard ceiling on mobs spawned per region, so a bad roll can't flood the sim.
pub const MAX_MOBS_PER_REGION: u32 = 6;

/// 1-in-N rolls land a Seeker instead of a Grazer (predators are rarer).
const SEEKER_ONE_IN: u64 = 5;

/// Default HP for spawned mobs.
const GRAZER_HP: i16 = 12;
const SEEKER_HP: i16 = 20;

#[inline]
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Region coordinate = floor(block / REGION_SIZE). Stable bucketing so a given
/// world position always belongs to the same spawn region.
pub fn region_of(block_x: i32, block_z: i32) -> (i32, i32) {
    (block_x.div_euclid(REGION_SIZE), block_z.div_euclid(REGION_SIZE))
}

/// Deterministically spawn the mobs for one region. `(rx, rz)` is the region
/// coordinate (see [`region_of`]). Pure given `(seed, rx, rz, generator)`.
pub fn spawn_region(gen: &WorldGenerator, world_seed: u64, rx: i32, rz: i32) -> Vec<Mob> {
    let mut s = world_seed
        ^ ((rx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
        ^ ((rz as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
    let budget = (splitmix64(&mut s) % (MAX_MOBS_PER_REGION as u64 + 1)) as u32;

    let base_x = rx * REGION_SIZE;
    let base_z = rz * REGION_SIZE;
    let mut out = Vec::with_capacity(budget as usize);
    for _ in 0..budget {
        let lx = (splitmix64(&mut s) % REGION_SIZE as u64) as i32;
        let lz = (splitmix64(&mut s) % REGION_SIZE as u64) as i32;
        let wx = base_x + lx;
        let wz = base_z + lz;
        // Surface within a generous vertical band; skip if none found.
        let Some(surface) = gen.surface_y_at(wx, wz, -16, 320) else { continue };
        // Dry land only: don't spawn on submerged columns.
        let sea = gen.sea_level_at(wx, wz);
        if (surface as f64) < sea - 1.0 {
            continue;
        }
        let pos = Vec3 {
            x: Fixed::from_int(wx as i64),
            y: Fixed::from_int(surface as i64 + 1),
            z: Fixed::from_int(wz as i64),
        };
        let mob = if splitmix64(&mut s) % SEEKER_ONE_IN == 0 {
            Mob::new_seeker(pos, SEEKER_HP)
        } else {
            Mob::new_grazer(pos, GRAZER_HP)
        };
        out.push(mob);
    }
    out
}

/// Spawn every region overlapping a square of `radius` regions around the
/// region containing `(center_x, center_z)`. Useful for populating around a
/// player. Deterministic and order-stable (regions visited in a fixed sweep).
pub fn spawn_around(
    gen: &WorldGenerator,
    world_seed: u64,
    center_x: i32,
    center_z: i32,
    region_radius: i32,
) -> Vec<Mob> {
    let (crx, crz) = region_of(center_x, center_z);
    let mut out = Vec::new();
    for rz in (crz - region_radius)..=(crz + region_radius) {
        for rx in (crx - region_radius)..=(crx + region_radius) {
            out.extend(spawn_region(gen, world_seed, rx, rz));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::mob::MobKind;

    fn gen() -> WorldGenerator { WorldGenerator::new(42) }

    #[test]
    fn spawning_is_deterministic() {
        let g = gen();
        let a = spawn_region(&g, 0xABCD, 1, -2);
        let b = spawn_region(&g, 0xABCD, 1, -2);
        assert_eq!(a.len(), b.len());
        for (m, n) in a.iter().zip(b.iter()) {
            assert_eq!(m.body.pos.x, n.body.pos.x);
            assert_eq!(m.body.pos.y, n.body.pos.y);
            assert_eq!(m.body.pos.z, n.body.pos.z);
            assert_eq!(m.kind, n.kind);
        }
    }

    #[test]
    fn different_regions_differ() {
        let g = gen();
        // Two regions producing the same exact mob set would indicate the
        // region coords aren't feeding the PRNG. Compare a span of regions;
        // at least one pair must differ in count or first position.
        let r1 = spawn_region(&g, 7, 0, 0);
        let r2 = spawn_region(&g, 7, 5, 9);
        let same = r1.len() == r2.len()
            && r1.iter().zip(r2.iter()).all(|(a, b)| a.body.pos.x == b.body.pos.x);
        assert!(!same, "distinct regions should not spawn identical mobs");
    }

    #[test]
    fn respects_population_ceiling() {
        let g = gen();
        // Sweep many regions; none may exceed the ceiling.
        for rx in -8..8 {
            for rz in -8..8 {
                let n = spawn_region(&g, 0xF00D, rx, rz).len();
                assert!(n as u32 <= MAX_MOBS_PER_REGION, "region ({rx},{rz}) spawned {n}");
            }
        }
    }

    #[test]
    fn spawns_on_dry_land_above_surface() {
        let g = gen();
        // Every spawned mob sits one block above a real surface, never below
        // sea level - 1 (the dry-land gate).
        for rx in -4..4 {
            for rz in -4..4 {
                for m in spawn_region(&g, 0x1234, rx, rz) {
                    let wx = m.body.pos.x.floor_int() as i32;
                    let wz = m.body.pos.z.floor_int() as i32;
                    let surface = g.surface_y_at(wx, wz, -16, 320).expect("spawned over a surface");
                    assert_eq!(m.body.pos.y.floor_int(), surface as i64 + 1);
                    assert!((surface as f64) >= g.sea_level_at(wx, wz) - 1.0);
                }
            }
        }
    }

    #[test]
    fn grazers_outnumber_seekers() {
        // Over a wide area, prey should dominate (predators are 1-in-5 rolls).
        let g = gen();
        let mobs = spawn_around(&g, 0xBEEF, 0, 0, 6);
        let seekers = mobs.iter().filter(|m| m.kind == MobKind::Seeker).count();
        let grazers = mobs.iter().filter(|m| m.kind == MobKind::Grazer).count();
        assert!(mobs.len() > 10, "expected a populated area, got {}", mobs.len());
        assert!(grazers > seekers, "grazers {grazers} should outnumber seekers {seekers}");
    }
}
