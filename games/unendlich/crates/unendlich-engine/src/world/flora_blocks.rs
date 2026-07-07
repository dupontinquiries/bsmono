//! `world::flora_blocks` — turn a [`FloraInstance`] (species + position) into the
//! actual voxel blocks that represent it: a single plant block for ground cover, a
//! short stack for cacti, a trunk + canopy for trees. Pure + deterministic; this is
//! what chunk-populate places after `flora_map` decides what grows where.

use crate::world::blocks::BlockId;
use crate::world::cpu_generator::WorldGenerator;
use crate::world::flora_map::region_flora;
use crate::world::flora_placement::FloraSpecies;

/// A block to set at a world position when instantiating flora.
pub type PlacedBlock = ([i32; 3], BlockId);

fn add(v: &mut Vec<PlacedBlock>, x: i32, y: i32, z: i32, b: BlockId) {
    v.push(([x, y, z], b));
}

/// Blocks for one plant rooted at `base` (the air cell directly above the surface).
/// Small flora is a single block; trees are a trunk column with a compact canopy.
pub fn flora_to_blocks(species: FloraSpecies, base: [i32; 3]) -> Vec<PlacedBlock> {
    use FloraSpecies::*;
    let [x, y, z] = base;
    let mut out = Vec::new();
    match species {
        Grass | Reed => add(&mut out, x, y, z, BlockId::TallGrass),
        Flower => add(&mut out, x, y, z, BlockId::Flower),
        Fern => add(&mut out, x, y, z, BlockId::Fern),
        Mushroom => add(&mut out, x, y, z, BlockId::Mushroom),
        Shrub => {
            // a low leafy clump
            add(&mut out, x, y, z, BlockId::OakLeaves);
            add(&mut out, x, y + 1, z, BlockId::OakLeaves);
        }
        Cactus => {
            for h in 0..3 {
                add(&mut out, x, y + h, z, BlockId::Cactus);
            }
        }
        Conifer => tree(&mut out, base, BlockId::OakLog, BlockId::OakLeaves, 6, Canopy::Cone),
        Broadleaf => tree(&mut out, base, BlockId::OakLog, BlockId::OakLeaves, 5, Canopy::Round),
        Palm => tree(&mut out, base, BlockId::BirchLog, BlockId::BirchLeaves, 7, Canopy::Fronds),
    }
    out
}

enum Canopy {
    Cone,
    Round,
    Fronds,
}

fn tree(out: &mut Vec<PlacedBlock>, base: [i32; 3], log: BlockId, leaf: BlockId, trunk_h: i32, shape: Canopy) {
    let [x, y, z] = base;
    for h in 0..trunk_h {
        add(out, x, y + h, z, log);
    }
    let top = y + trunk_h;
    match shape {
        Canopy::Cone => {
            // tapering pine: wide ring low, narrow up, a tip on top
            for (i, r) in [2i32, 2, 1].iter().enumerate() {
                let cy = top - 1 + i as i32;
                for dx in -r..=*r {
                    for dz in -r..=*r {
                        if dx * dx + dz * dz <= r * r + 1 && !(dx == 0 && dz == 0 && cy < top + 1) {
                            add(out, x + dx, cy, z + dz, leaf);
                        }
                    }
                }
            }
            add(out, x, top + 2, z, leaf); // tip
        }
        Canopy::Round => {
            // a rough sphere of leaves around the top
            for dy in -1..=2 {
                let r = if dy == -1 || dy == 2 { 1 } else { 2 };
                for dx in -r..=r {
                    for dz in -r..=r {
                        if dx * dx + dz * dz <= r * r + 1 {
                            add(out, x + dx, top + dy, z + dz, leaf);
                        }
                    }
                }
            }
        }
        Canopy::Fronds => {
            // palm: a crown of fronds radiating from the top
            add(out, x, top, z, leaf);
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (2, 0), (-2, 0), (0, 2), (0, -2)] {
                add(out, x + dx, top, z + dz, leaf);
            }
        }
    }
}

/// Chunk-populate seam: every flora block for a horizontal region of the live
/// world — `flora_map::region_flora` decides, `flora_to_blocks` instantiates.
/// The caller sets each block (e.g. `chunk_manager.set_block`).
pub fn region_flora_blocks(gen: &WorldGenerator, x0: i32, z0: i32, w: i32, d: i32) -> Vec<PlacedBlock> {
    let mut out = Vec::new();
    for inst in region_flora(gen, x0, z0, w, d) {
        out.extend(flora_to_blocks(inst.species, inst.pos));
    }
    out
}

/// Prober-aware variant — memoizes the per-column surface/biome scans (the flora
/// half of the ecosystem-stream region-crossing freeze; r296 did the spawn half).
/// The caller supplies a `WorldProber` (its own — flora's probe range differs from
/// the spawn pass's, so it must not share that prober) and clears it per batch.
pub fn region_flora_blocks_cached(
    gen: &WorldGenerator,
    prober: &mut crate::sim::world_prober::WorldProber,
    x0: i32,
    z0: i32,
    w: i32,
    d: i32,
) -> Vec<PlacedBlock> {
    let mut out = Vec::new();
    for inst in crate::world::flora_map::region_flora_cached(gen, prober, x0, z0, w, d) {
        out.extend(flora_to_blocks(inst.species, inst.pos));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ground_cover_is_one_block_trees_are_many() {
        let g = flora_to_blocks(FloraSpecies::Grass, [0, 64, 0]);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].1, BlockId::TallGrass);
        let tree = flora_to_blocks(FloraSpecies::Conifer, [0, 64, 0]);
        assert!(tree.len() > 8, "a conifer has a trunk + canopy");
        // trunk starts at base and rises
        assert!(tree.iter().any(|(p, b)| *p == [0, 64, 0] && *b == BlockId::OakLog));
        assert!(tree.iter().any(|(_, b)| *b == BlockId::OakLeaves));
    }

    #[test]
    fn cactus_stacks_upward() {
        let c = flora_to_blocks(FloraSpecies::Cactus, [3, 70, -2]);
        assert_eq!(c.len(), 3);
        assert!(c.iter().all(|(_, b)| *b == BlockId::Cactus));
        let ys: Vec<i32> = c.iter().map(|(p, _)| p[1]).collect();
        assert_eq!(ys, vec![70, 71, 72]);
    }

    #[test]
    fn region_instantiates_real_world_flora() {
        let gen = WorldGenerator::new(7);
        let blocks = region_flora_blocks(&gen, -30, -30, 60, 60);
        assert!(!blocks.is_empty());
        // everything is a plant/foliage block, never air/stone
        assert!(blocks.iter().all(|(_, b)| !matches!(*b, BlockId::Air | BlockId::Stone)));
    }
}
