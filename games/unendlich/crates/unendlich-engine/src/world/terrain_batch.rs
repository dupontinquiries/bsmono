//! `world::terrain_batch` — generate chunks across worker threads.
//!
//! [`WorldGenerator::generate_chunk`](crate::world::cpu_generator::WorldGenerator::generate_chunk)
//! is, by its own contract, a pure function of `(coord, seed)` with no neighbour
//! dependencies — and worldgen places water blocks as it fills, so a chunk carries
//! its terrain *and* its water placement. That makes generating a set of chunks a
//! **Class-P** parallel map over the engine's rayon lane
//! ([`util::task_lanes::map_pure`](crate::util::task_lanes)): the generator is read
//! through a shared `&self` (no interior mutability — only `Arc` fields), each
//! `Chunk` is an independent owned output, and `map_pure` preserves order, so the
//! batch is identical to generating the chunks serially. This threads terrain and
//! water placement together off the main loop.

use crate::util::task_lanes::{map_pure, map_pure_threshold};
use crate::world::chunk::{Chunk, ChunkCoord};
use crate::world::cpu_generator::WorldGenerator;

/// Generate every requested chunk in parallel. Order-preserving and deterministic:
/// `generate_chunks(gen, &coords)[i]` is exactly `gen.generate_chunk(coords[i])`.
pub fn generate_chunks(gen: &WorldGenerator, coords: &[ChunkCoord]) -> Vec<Chunk> {
    map_pure(coords, |c| gen.generate_chunk(*c))
}

/// As [`generate_chunks`], but run serially below `min_parallel` chunks — a tiny
/// streaming batch isn't worth a fork.
pub fn generate_chunks_threshold(
    gen: &WorldGenerator,
    coords: &[ChunkCoord],
    min_parallel: usize,
) -> Vec<Chunk> {
    map_pure_threshold(coords, min_parallel, |c| gen.generate_chunk(*c))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Correctness of the parallel map (order, determinism, equals-serial) is proven
    // generically by `sim::water_batch` over the same `map_pure` lane; here we assert
    // the terrain-specific contract: the batch dispatches the right coords in order, so
    // streaming can fan chunks across threads and reassemble them in order. (Per-chunk
    // content equality follows from `generate_chunk` being a deterministic pure fn.)
    #[test]
    fn batch_dispatches_coords_in_order() {
        let gen = WorldGenerator::new(1234);
        let coords: Vec<ChunkCoord> = (0..6)
            .map(|i| ChunkCoord::new(i % 3, 0, i / 3))
            .collect();
        let batch = generate_chunks(&gen, &coords);
        assert_eq!(batch.len(), coords.len());
        for (c, got) in coords.iter().zip(batch.iter()) {
            assert_eq!(got.coord, *c, "chunk i is the one generated for coords[i]");
        }
        // threshold path produces the same coords in the same order
        let thr = generate_chunks_threshold(&gen, &coords, 4);
        assert_eq!(
            thr.iter().map(|c| c.coord).collect::<Vec<_>>(),
            coords,
        );
        assert!(generate_chunks(&gen, &[]).is_empty());
    }
}
