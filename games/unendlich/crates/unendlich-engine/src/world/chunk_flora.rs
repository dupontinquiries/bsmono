//! Per-chunk flora — the seam that makes the flora modules a loadable feature.
//!
//! When a chunk loads, [`ChunkFlora::populate`] scans its world box for cave
//! sites ([`crate::world::flora_sites`]), grows a [`CaveFlora`] at each, and
//! holds them; [`ChunkFlora::tick`] then advances all their lights together.
//! Everything is reproducible from `(coord, world_seed)`, so a chunk lights the
//! same way every time it loads, on every host.
//!
//! Still `BlockId`-agnostic: occupancy, moisture and temperature arrive as
//! world-space closures the caller wires to the real chunk + climate fields, and
//! the resulting pearl `emission` is what the renderer reads. The only thing this
//! adds over the lower modules is the chunk-scoping, the per-chunk plant cap, and
//! ticking the set.

use crate::constants::CHUNK_SIZE;
use crate::world::cave_flora::{sky_exposure, CaveFlora};
use crate::world::chunk::ChunkCoord;
use crate::world::flora_sites::find_sites;
use crate::world::stem::Cell;

/// Growth steps each plant runs at populate time.
const DEFAULT_GROW_STEPS: u32 = 24;
/// Hard cap on plants per chunk (perf guard against dense surfaces).
const MAX_PLANTS: usize = 64;

/// The grown, lightable flora belonging to one chunk.
#[derive(Clone, Debug)]
pub struct ChunkFlora {
    pub coord: ChunkCoord,
    pub plants: Vec<CaveFlora>,
}

impl ChunkFlora {
    /// Scan this chunk's world box for sites and grow a plant at each. `max_y`
    /// keeps flora underground; the field closures sample world cells (occupancy
    /// for sites/growth/exposure, plus moisture and temperature for growth).
    pub fn populate<O, M, T>(
        coord: ChunkCoord,
        world_seed: u64,
        max_y: i32,
        occupied: &O,
        moisture: &M,
        temperature: &T,
    ) -> Self
    where
        O: Fn(Cell) -> bool,
        M: Fn(Cell) -> f32,
        T: Fn(Cell) -> f32,
    {
        let origin = coord.block_origin();
        let dims = (CHUNK_SIZE, CHUNK_SIZE, CHUNK_SIZE);
        let sites = find_sites(origin, dims, world_seed, max_y, occupied);

        let exposure = |c: Cell| sky_exposure(c, occupied);
        let mut plants = Vec::new();
        for site in sites.into_iter().take(MAX_PLANTS) {
            plants.push(CaveFlora::grow(
                site.root,
                site.dir,
                site.seed,
                DEFAULT_GROW_STEPS,
                moisture,
                temperature,
                occupied,
                &exposure,
            ));
        }
        Self { coord, plants }
    }

    /// Advance every plant's light by one tick at the given `daylight`.
    pub fn tick(&mut self, daylight: f32) {
        for p in &mut self.plants {
            p.tick(daylight);
        }
    }

    /// Total pearls across all plants in the chunk.
    pub fn pearl_count(&self) -> usize {
        self.plants.iter().map(|p| p.lumen.pearls.len()).sum()
    }

    /// Brightest pearl anywhere in the chunk right now.
    pub fn peak_emission(&self) -> f32 {
        self.plants.iter().fold(0.0, |m, p| m.max(p.peak_emission()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(v: f32) -> impl Fn(Cell) -> f32 {
        move |_| v
    }

    #[test]
    fn populates_and_lights_a_floored_chunk() {
        let coord = ChunkCoord::from_block_pos(0, 0, 0);
        // Solid floor below y=0; open air above (so floor flora grows up into
        // fully sky-exposed columns).
        let occ = |c: Cell| c.1 < 0;
        let mut cf = ChunkFlora::populate(coord, 7, CHUNK_SIZE, &occ, &k(0.6), &k(0.5));
        assert!(!cf.plants.is_empty(), "a floored chunk grows flora");
        assert!(cf.pearl_count() > cf.plants.len(), "stems grew past their roots");

        // Charge through a day, then confirm the night chain actually lights.
        for _ in 0..120 {
            cf.tick(1.0);
        }
        let mut lit = false;
        for _ in 0..60 {
            cf.tick(0.0);
            if cf.peak_emission() >= 1.0 {
                lit = true;
            }
        }
        assert!(lit, "charged flora pulses at night");
    }

    #[test]
    fn empty_chunk_has_no_flora() {
        let coord = ChunkCoord::from_block_pos(0, 0, 0);
        let cf = ChunkFlora::populate(coord, 7, CHUNK_SIZE, &|_: Cell| false, &k(0.6), &k(0.5));
        assert!(cf.plants.is_empty(), "no surface, no flora");
        assert_eq!(cf.pearl_count(), 0);
    }

    #[test]
    fn deterministic_for_coord_and_seed() {
        let coord = ChunkCoord::from_block_pos(0, 0, 0);
        let occ = |c: Cell| c.1 < 0;
        let a = ChunkFlora::populate(coord, 7, CHUNK_SIZE, &occ, &k(0.6), &k(0.5));
        let b = ChunkFlora::populate(coord, 7, CHUNK_SIZE, &occ, &k(0.6), &k(0.5));
        assert_eq!(a.plants.len(), b.plants.len(), "same coord+seed → same plant count");
        assert_eq!(a.pearl_count(), b.pearl_count(), "and the same pearls");
    }
}
