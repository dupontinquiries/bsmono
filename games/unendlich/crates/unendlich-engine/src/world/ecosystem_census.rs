//! A runnable **census harness** for the ecosystem placement layer
//! ([`crate::world::ecosystem`]). It sweeps a grid of surface cells, runs the
//! per-cell spawn roll, and tallies the result into a readable report: totals,
//! a per-kind breakdown, a per-biome breakdown, and a one-number fingerprint.
//!
//! Why this exists: the spawn *logic* is pure and unit-tested, but the only way
//! to see the *emergent distribution over a real generated world* is to run it.
//! The aggregation here is pure (and unit-tested), while the `ecosystem_census`
//! test drives it against the CPU world generator and prints the report. The
//! generator is deterministic per seed, so the same `CENSUS_SEED` yields the
//! same report — a matching `fingerprint` line confirms the placement build
//! agrees end to end across machines, and the breakdown is the eyeball check
//! (fauna across the surface, dinosaurs in the warm lowlands, dragons in the
//! heights, lauschers in the cave-climate cells).
//!
//! Run it (prints to stdout):
//! ```text
//! CENSUS=1 cargo test -p unendlich-engine --no-default-features --lib \
//!     world::ecosystem_census::ecosystem_census -- --nocapture
//! ```
//! Knobs (env vars): `CENSUS_SEED` (default 1), `CENSUS_AREA` (side length,
//! default 384), `CENSUS_RARITY` (default `ecosystem::DEFAULT_RARITY`),
//! `CENSUS_Y` (sampling height, default 72).

use crate::item::CreatureKind;
use crate::world::biomes::Biome;
use crate::world::ecosystem::roll_spawn;

/// Every biome in declaration order, so the report can name each by index.
pub const ALL_BIOMES: [Biome; 19] = [
    Biome::Plains,
    Biome::TemperateForest,
    Biome::TemperateHighlands,
    Biome::BirchForest,
    Biome::MapleForest,
    Biome::CloudForest,
    Biome::MistyMountains,
    Biome::AlpineHighlands,
    Biome::Desert,
    Biome::RedSandstoneCanyon,
    Biome::BloomingCanyon,
    Biome::Ocean,
    Biome::Beach,
    Biome::Swamp,
    Biome::Tundra,
    Biome::Skylands,
    Biome::IceCave,
    Biome::FungalCave,
    Biome::ThermalCave,
];

const N_BIOMES: usize = ALL_BIOMES.len();

/// Tallies from a census sweep.
#[derive(Debug, Clone)]
pub struct Census {
    pub cells: u64,
    pub spawns: u64,
    /// Counts indexed by position in [`CreatureKind::ALL`].
    pub per_kind: [u64; CreatureKind::ALL.len()],
    /// Cells seen per biome, indexed by [`ALL_BIOMES`].
    pub biome_cells: [u64; N_BIOMES],
    /// Spawns per biome, indexed by [`ALL_BIOMES`].
    pub biome_spawns: [u64; N_BIOMES],
}

impl Default for Census {
    fn default() -> Self {
        Census {
            cells: 0,
            spawns: 0,
            per_kind: [0; CreatureKind::ALL.len()],
            biome_cells: [0; N_BIOMES],
            biome_spawns: [0; N_BIOMES],
        }
    }
}

fn kind_index(k: CreatureKind) -> usize {
    CreatureKind::ALL.iter().position(|&x| x == k).unwrap()
}

fn biome_index(b: Biome) -> usize {
    ALL_BIOMES.iter().position(|&x| x == b).unwrap_or(0)
}

impl Census {
    /// Record one swept cell and the spawn (if any) the roll produced there.
    pub fn record(&mut self, biome: Biome, spawn: Option<CreatureKind>) {
        self.cells += 1;
        self.biome_cells[biome_index(biome)] += 1;
        if let Some(k) = spawn {
            self.spawns += 1;
            self.biome_spawns[biome_index(biome)] += 1;
            self.per_kind[kind_index(k)] += 1;
        }
    }

    /// Spawns per 1000 cells swept.
    pub fn density_per_1000(&self) -> f64 {
        if self.cells == 0 {
            0.0
        } else {
            self.spawns as f64 * 1000.0 / self.cells as f64
        }
    }

    /// A cheap order-independent hash of the tallies — a single number both ends
    /// can compare to confirm an identical distribution without diffing tables.
    pub fn fingerprint(&self) -> u64 {
        let mut h = 0xCBF2_9CE4_8422_2325u64; // FNV-ish offset
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        };
        mix(self.cells);
        mix(self.spawns);
        for (i, &v) in self.per_kind.iter().enumerate() {
            mix((i as u64) << 56 | v);
        }
        for i in 0..N_BIOMES {
            mix((i as u64) << 56 | self.biome_spawns[i]);
        }
        h
    }

    /// A readable multi-line report.
    pub fn report(&self, title: &str) -> String {
        let mut s = String::new();
        s.push_str(&format!("=== {title} ===\n"));
        s.push_str(&format!(
            "cells: {}   spawns: {}   density: {:.1} / 1000 cells\n\n",
            self.cells,
            self.spawns,
            self.density_per_1000()
        ));
        s.push_str("by kind:\n");
        for (i, &count) in self.per_kind.iter().enumerate() {
            let pct = if self.spawns == 0 {
                0.0
            } else {
                count as f64 * 100.0 / self.spawns as f64
            };
            s.push_str(&format!(
                "  {:<14} {:>7}  ({:>4.1}%)\n",
                CreatureKind::ALL[i].name(),
                count,
                pct
            ));
        }
        s.push_str("\nby biome (cells -> spawns, density/1000):\n");
        for i in 0..N_BIOMES {
            let cells = self.biome_cells[i];
            if cells == 0 {
                continue;
            }
            let sp = self.biome_spawns[i];
            let d = sp as f64 * 1000.0 / cells as f64;
            s.push_str(&format!(
                "  {:<18} {:>7} -> {:>6}   {:>6.1}\n",
                format!("{:?}", ALL_BIOMES[i]),
                cells,
                sp,
                d
            ));
        }
        s.push_str(&format!("\nfingerprint: 0x{:016X}\n", self.fingerprint()));
        s
    }
}

/// Sweep a `w` by `h` grid of cells from `(x0, z0)`, rolling a spawn per cell
/// from `biome_at`. Pure — feed it the real worldgen's biome lookup or a
/// synthetic map. This is the same per-cell roll the chunk-populate path uses
/// (via [`crate::world::ecosystem::roll_spawn`]); the harness just counts it.
pub fn census<F: Fn(i32, i32) -> Biome>(
    x0: i32,
    z0: i32,
    w: i32,
    h: i32,
    seed: u64,
    rarity: u32,
    biome_at: F,
) -> Census {
    let mut c = Census::default();
    for dz in 0..h {
        for dx in 0..w {
            let (x, z) = (x0 + dx, z0 + dz);
            let b = biome_at(x, z);
            c.record(b, roll_spawn(b, x, z, seed, rarity));
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::ecosystem::DEFAULT_RARITY;

    #[test]
    fn density_and_recording_add_up() {
        let mut c = Census::default();
        c.record(Biome::Plains, Some(CreatureKind::Fauna));
        c.record(Biome::Plains, None);
        c.record(Biome::Plains, Some(CreatureKind::Dinosaur));
        c.record(Biome::FungalCave, Some(CreatureKind::Lauscher));
        assert_eq!(c.cells, 4);
        assert_eq!(c.spawns, 3);
        assert_eq!(c.per_kind[kind_index(CreatureKind::Fauna)], 1);
        assert_eq!(c.per_kind[kind_index(CreatureKind::Lauscher)], 1);
        assert_eq!(c.biome_cells[biome_index(Biome::Plains)], 3);
        assert_eq!(c.biome_spawns[biome_index(Biome::Plains)], 2);
        assert_eq!(c.density_per_1000(), 750.0);
    }

    #[test]
    fn a_plains_sweep_is_fauna_dominant_with_listeners_and_no_megafauna() {
        // rarity 1 = every cell attempts; a big sweep so the distribution settles.
        let c = census(0, 0, 200, 200, 31, 1, |_x, _z| Biome::Plains);
        assert!(c.spawns > 0);
        let fauna = c.per_kind[kind_index(CreatureKind::Fauna)];
        let dino = c.per_kind[kind_index(CreatureKind::Dinosaur)];
        let spaeher = c.per_kind[kind_index(CreatureKind::Spaeher)];
        let lauscher = c.per_kind[kind_index(CreatureKind::Lauscher)];
        // Plains weights now: Fauna 9, Lauscher 5, Spaeher 4, Dinosaur 0 — the
        // little sheep (fauna) lead, the listeners roam, the megafauna are gone.
        assert!(fauna > spaeher && fauna > lauscher, "fauna dominates the plains");
        assert_eq!(dino, 0, "ambient megafauna removed");
        assert!(lauscher > 0, "little listeners now roam the surface");
        assert_eq!(c.per_kind[kind_index(CreatureKind::Dragon)], 0, "no peak dragon");
    }

    #[test]
    fn a_cave_sweep_is_all_listeners() {
        let c = census(0, 0, 100, 100, 5, 1, |_x, _z| Biome::IceCave);
        assert!(c.spawns > 0);
        assert_eq!(c.per_kind[kind_index(CreatureKind::Lauscher)], c.spawns, "only listeners underground");
    }

    #[test]
    fn rarity_thins_the_population() {
        let dense = census(0, 0, 128, 128, 9, 8, |_x, _z| Biome::Plains).spawns;
        let sparse = census(0, 0, 128, 128, 9, 128, |_x, _z| Biome::Plains).spawns;
        assert!(dense > sparse, "a larger rarity divisor thins the population");
    }

    #[test]
    fn report_names_the_kinds_and_is_deterministic() {
        let c = census(0, 0, 64, 64, 7, 4, |x, _z| if x < 32 { Biome::FungalCave } else { Biome::Plains });
        let r = c.report("test");
        assert!(r.contains("Wild Animal") && r.contains("Lauscher"), "report lists kinds");
        // The fingerprint is stable for identical tallies.
        let c2 = census(0, 0, 64, 64, 7, 4, |x, _z| if x < 32 { Biome::FungalCave } else { Biome::Plains });
        assert_eq!(c.fingerprint(), c2.fingerprint());
    }

    /// The runnable harness. Skipped unless `CENSUS=1`. Drives the census against
    /// the real CPU world generator and prints the report; deterministic per
    /// `CENSUS_SEED`, so the fingerprint line should match across machines.
    ///
    /// It samples an `AREA x AREA` grid of points spaced `STRIDE` blocks apart
    /// (a coarse net over a wide span, since biomes vary over long distances) and
    /// rolls the real per-cell spawn at each sampled world cell. With a stride the
    /// density is "per 1000 sampled cells" — relative rates and the kind/biome mix
    /// stay representative.
    #[test]
    fn ecosystem_census() {
        if std::env::var("CENSUS").is_err() {
            return;
        }
        use crate::world::cpu_generator::WorldGenerator;
        let env_u = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        let seed = env_u("CENSUS_SEED", 1);
        let area = env_u("CENSUS_AREA", 384) as i32;
        let stride = env_u("CENSUS_STRIDE", 24) as i32;
        let rarity = env_u("CENSUS_RARITY", DEFAULT_RARITY as u64) as u32;
        let y = env_u("CENSUS_Y", 72) as i32;

        let gen = WorldGenerator::new(seed as u32);
        let half = (area * stride) / 2;
        let mut c = Census::default();
        for dz in 0..area {
            for dx in 0..area {
                let x = -half + dx * stride;
                let z = -half + dz * stride;
                let b = gen.biome_at_3d(x, y, z).biome;
                c.record(b, roll_spawn(b, x, z, seed, rarity));
            }
        }
        let span = area * stride;
        println!(
            "\n{}",
            c.report(&format!(
                "ECOSYSTEM CENSUS seed={seed} grid={area}x{area} stride={stride} (span {span}x{span} blocks) rarity={rarity} y={y}"
            ))
        );
    }
}
