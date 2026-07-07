//! SurfaceRun — per-column run-length encoding of a chunk.
//!
//! For each `(lx, lz)` column the 32 vertical cells are encoded as a list
//! of `(count, block_id)` runs. Surface chunks are highly compressible along
//! `y` (long runs of air, then a thin organic layer, then long runs of stone),
//! so this gets ~3–10× over a flat `Vec<BlockId>` depending on contents.
//!
//! This module is the foundation: a working compress/decompress pair with
//! round-trip tests. Wiring it into the meshing pipeline / GPU-resident
//! storage is follow-on work — once the user wants to measure live savings,
//! the call site is `Chunk::from_blocks(...) <-> SurfaceRunChunk` and the
//! `byte_size` / `compression_ratio` accessors give the per-chunk number.
//!
//! Layout invariants:
//!   - `col_runs` has exactly `CHUNK_SIZE*CHUNK_SIZE` entries, one per column.
//!   - Each `col_runs[k]` is in `1..=CHUNK_SIZE` (a column always has ≥1 run;
//!     worst case is alternating blocks → CHUNK_SIZE runs).
//!   - `runs.len() == sum(col_runs)`, runs laid out column-major matching the
//!     `lx + lz*CHUNK_SIZE` indexing used by the rest of the engine.
//!   - Within a column, counts sum to exactly `CHUNK_SIZE`.

use crate::constants::{CHUNK_SIZE, CHUNK_VOLUME};
use crate::world::blocks::BlockId;
use crate::world::chunk::{Chunk, ChunkCoord};

const CS: usize = CHUNK_SIZE as usize;
const COLS: usize = CS * CS;

/// A run-length-encoded chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceRunChunk {
    pub coord: ChunkCoord,
    /// Geometry flag for the dominant common case: if the entire chunk is a
    /// single block id, this is `Some(block_u8)` and `col_runs`/`runs` are
    /// empty. A tall voxel world is mostly all-air (sky) and all-solid (deep
    /// underground) chunks; encoding those in 1 byte instead of the
    /// ~3 KB per-column form is the single largest aggregate-ratio lever.
    pub uniform: Option<u8>,
    /// Number of runs per column, indexed by `lx + lz*CHUNK_SIZE`. Empty when
    /// `uniform` is set.
    pub col_runs: Vec<u8>,
    /// Flat `(count, block_id_u8)` runs in column-major order. Column `k`'s
    /// runs start at `col_offsets[k]` = prefix-sum of `col_runs[0..k]`. Empty
    /// when `uniform` is set.
    pub runs: Vec<(u8, u8)>,
}

impl SurfaceRunChunk {
    /// An empty scratch instance (no allocations). Fill it with
    /// [`from_chunk_into`](Self::from_chunk_into) and reuse it across many
    /// chunks to avoid per-chunk `Vec` churn.
    pub fn empty() -> Self {
        Self {
            coord: ChunkCoord { x: 0, y: 0, z: 0 },
            uniform: None,
            col_runs: Vec::new(),
            runs: Vec::new(),
        }
    }

    /// Compress a chunk by walking each column along `y`.
    pub fn from_chunk(chunk: &Chunk) -> Self {
        let mut s = Self::empty();
        s.from_chunk_into(chunk);
        s
    }

    /// Refill `self` from `chunk` in place, **reusing** the existing
    /// `col_runs` / `runs` backing stores (`clear` keeps capacity) instead of
    /// allocating fresh `Vec`s. The result is bit-identical to
    /// [`from_chunk`](Self::from_chunk) — this only removes allocation churn
    /// for hot loops that classify many chunks back-to-back (prefetch upload,
    /// compression telemetry). Hold one scratch `SurfaceRunChunk` and call
    /// this per chunk.
    pub fn from_chunk_into(&mut self, chunk: &Chunk) {
        self.coord = chunk.coord;

        // Geometry flag: whole-chunk uniform block. Cheap single pass; the
        // common sky/deep cases short-circuit to a 1-byte encoding.
        let first = chunk.blocks[0] as u8;
        if chunk.blocks.iter().all(|&b| b as u8 == first) {
            self.uniform = Some(first);
            self.col_runs.clear();
            self.runs.clear();
            return;
        }

        self.uniform = None;
        self.col_runs.clear();
        self.col_runs.resize(COLS, 0u8);
        self.runs.clear();
        // Heuristic capacity: surface chunks average ~4 runs/column. A no-op
        // once the scratch buffer is warm.
        self.runs.reserve(COLS * 4);
        for lz in 0..CS {
            for lx in 0..CS {
                let col = lx + lz * CS;
                // Seed from ly=0.
                let mut cur = chunk.blocks[lx + lz * CS] as u8;
                let mut count: u8 = 1;
                let mut n_runs: u8 = 0;
                for ly in 1..CS {
                    let idx = lx + lz * CS + ly * CS * CS;
                    let b = chunk.blocks[idx] as u8;
                    if b == cur && count < CS as u8 {
                        count += 1;
                    } else {
                        self.runs.push((count, cur));
                        n_runs += 1;
                        cur = b;
                        count = 1;
                    }
                }
                // Flush the final run for this column.
                self.runs.push((count, cur));
                n_runs += 1;
                self.col_runs[col] = n_runs;
            }
        }
    }

    /// Decompress into a flat `Vec<BlockId>` matching `Chunk::blocks` layout.
    /// Unknown block ids (shouldn't happen for in-engine chunks) become `Air`.
    pub fn to_blocks(&self) -> Vec<BlockId> {
        if let Some(raw) = self.uniform {
            let block = BlockId::try_from(raw).unwrap_or(BlockId::Air);
            return vec![block; CHUNK_VOLUME];
        }
        let mut out = vec![BlockId::Air; CHUNK_VOLUME];
        let mut off = 0usize;
        for lz in 0..CS {
            for lx in 0..CS {
                let col = lx + lz * CS;
                let n = self.col_runs[col] as usize;
                let mut ly = 0usize;
                for r in 0..n {
                    let (count, raw) = self.runs[off + r];
                    let block = BlockId::try_from(raw).unwrap_or(BlockId::Air);
                    for _ in 0..count {
                        let idx = lx + lz * CS + ly * CS * CS;
                        out[idx] = block;
                        ly += 1;
                    }
                }
                off += n;
            }
        }
        out
    }

    /// Decompress and rebuild a `Chunk` at the encoded coordinate.
    pub fn to_chunk(&self) -> Chunk {
        Chunk::from_blocks(self.coord, self.to_blocks())
    }

    /// Bytes for the compressed encoding: the per-column header + 2 bytes per
    /// run (count + block id). Compare against `CHUNK_VOLUME` for raw u8 size,
    /// or `CHUNK_VOLUME * size_of::<BlockId>()` for the live `Vec<BlockId>`.
    pub fn byte_size(&self) -> usize {
        if self.uniform.is_some() {
            return 1;
        }
        self.col_runs.len() + self.runs.len() * 2
    }

    /// Compression factor vs raw u8 storage (`>1.0` means smaller).
    pub fn compression_ratio(&self) -> f32 {
        CHUNK_VOLUME as f32 / self.byte_size() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coord() -> ChunkCoord {
        ChunkCoord { x: 0, y: 0, z: 0 }
    }

    #[test]
    fn all_air_round_trips_and_compresses_hard() {
        let blocks = vec![BlockId::Air; CHUNK_VOLUME];
        let c = Chunk::from_blocks(coord(), blocks.clone());
        let s = SurfaceRunChunk::from_chunk(&c);
        // v0.24.NEXT+13-uniform: an all-air chunk is now a 1-byte uniform flag,
        // not a per-column run set.
        assert_eq!(s.uniform, Some(BlockId::Air as u8));
        assert!(s.col_runs.is_empty());
        assert!(s.runs.is_empty());
        // Round-trip.
        let back = s.to_chunk();
        assert_eq!(back.blocks, blocks);
        // 1 byte vs CHUNK_VOLUME=32768 raw -> 32768×.
        assert_eq!(s.byte_size(), 1);
        assert!(s.compression_ratio() > 10000.0, "ratio was {}", s.compression_ratio());
    }

    #[test]
    fn all_stone_is_uniform() {
        // Deep-underground chunk: the other dominant common case.
        let blocks = vec![BlockId::Stone; CHUNK_VOLUME];
        let c = Chunk::from_blocks(coord(), blocks.clone());
        let s = SurfaceRunChunk::from_chunk(&c);
        assert_eq!(s.uniform, Some(BlockId::Stone as u8));
        assert_eq!(s.byte_size(), 1);
        let back = s.to_chunk();
        assert_eq!(back.blocks, blocks);
    }

    #[test]
    fn one_differing_cell_defeats_uniform() {
        // A single non-matching voxel must fall back to the per-column form
        // and still round-trip exactly.
        let mut blocks = vec![BlockId::Air; CHUNK_VOLUME];
        blocks[CHUNK_VOLUME / 2] = BlockId::Stone;
        let c = Chunk::from_blocks(coord(), blocks.clone());
        let s = SurfaceRunChunk::from_chunk(&c);
        assert_eq!(s.uniform, None);
        assert_eq!(s.col_runs.len(), COLS);
        let back = s.to_chunk();
        assert_eq!(back.blocks, blocks);
    }

    #[test]
    fn alternating_y_is_worst_case_and_round_trips() {
        // Worst case: every adjacent y-cell differs.
        let mut blocks = vec![BlockId::Air; CHUNK_VOLUME];
        for ly in 0..CS {
            let b = if ly % 2 == 0 { BlockId::Stone } else { BlockId::Air };
            for lz in 0..CS {
                for lx in 0..CS {
                    blocks[lx + lz * CS + ly * CS * CS] = b;
                }
            }
        }
        let c = Chunk::from_blocks(coord(), blocks.clone());
        let s = SurfaceRunChunk::from_chunk(&c);
        // CHUNK_SIZE runs per column — the maximum.
        assert!(s.col_runs.iter().all(|&n| n == CS as u8));
        let back = s.to_chunk();
        assert_eq!(back.blocks, blocks);
        // Worst case still beats raw because each run only costs 2B and a raw
        // u8 column costs CHUNK_SIZE B; with 32 runs/col it's 64B+1B header
        // vs 32B raw — actually WORSE than raw. Verify we don't claim a win.
        assert!(s.compression_ratio() < 1.0, "alt-y shouldn't compress: {}", s.compression_ratio());
    }

    #[test]
    fn surface_terrain_round_trips_and_compresses_well() {
        // Realistic-ish: stone bottom, dirt band, single grass cap, air above.
        let mut blocks = vec![BlockId::Air; CHUNK_VOLUME];
        let half = CS / 2;
        let dirt_top = half + 3;
        for lz in 0..CS {
            for lx in 0..CS {
                for ly in 0..half {
                    blocks[lx + lz * CS + ly * CS * CS] = BlockId::Stone;
                }
                for ly in half..dirt_top {
                    blocks[lx + lz * CS + ly * CS * CS] = BlockId::Dirt;
                }
                blocks[lx + lz * CS + dirt_top * CS * CS] = BlockId::Grass;
                // remaining ly > dirt_top stays Air (already initialized).
            }
        }
        let c = Chunk::from_blocks(coord(), blocks.clone());
        let s = SurfaceRunChunk::from_chunk(&c);
        // 4 runs per column: stone, dirt, grass, air.
        assert!(s.col_runs.iter().all(|&n| n == 4));
        let back = s.to_chunk();
        assert_eq!(back.blocks, blocks);
        // 1024 cols * 4 runs * 2B + 1024 header = 9216 bytes vs 32768 raw -> ~3.5×.
        assert!(s.compression_ratio() > 3.0, "ratio was {}", s.compression_ratio());
    }

    #[test]
    fn empty_runs_invariant_sum_equals_chunk_size() {
        // For an arbitrary pattern, the per-column counts must sum to CS.
        let mut blocks = vec![BlockId::Air; CHUNK_VOLUME];
        for ly in 0..CS {
            for lz in 0..CS {
                for lx in 0..CS {
                    // pseudo-random-ish pattern
                    let v = ((lx * 7 + lz * 13 + ly * 5) % 3) as u8;
                    blocks[lx + lz * CS + ly * CS * CS] = match v {
                        0 => BlockId::Air,
                        1 => BlockId::Stone,
                        _ => BlockId::Dirt,
                    };
                }
            }
        }
        let c = Chunk::from_blocks(coord(), blocks.clone());
        let s = SurfaceRunChunk::from_chunk(&c);
        let mut off = 0usize;
        for col in 0..COLS {
            let n = s.col_runs[col] as usize;
            let sum: u32 = s.runs[off..off + n].iter().map(|(c, _)| *c as u32).sum();
            assert_eq!(sum, CS as u32, "col {} sum {}", col, sum);
            off += n;
        }
        // And full round-trip.
        let back = s.to_chunk();
        assert_eq!(back.blocks, blocks);
    }

    #[test]
    fn from_chunk_into_matches_from_chunk_and_reuses_safely() {
        // A reused scratch buffer must produce a result bit-identical to a
        // freshly-allocated `from_chunk`, for each chunk shape, and carry no
        // stale state when refilled from a different chunk.
        let mixed = {
            let mut b = vec![BlockId::Air; CHUNK_VOLUME];
            for lz in 0..CS { for lx in 0..CS { for ly in 0..(CS / 2) {
                b[lx + lz * CS + ly * CS * CS] = BlockId::Stone;
            }}}
            Chunk::from_blocks(ChunkCoord { x: 1, y: 2, z: 3 }, b)
        };
        let uniform = Chunk::from_blocks(
            ChunkCoord { x: -4, y: 5, z: -6 },
            vec![BlockId::Air; CHUNK_VOLUME],
        );

        let mut scratch = SurfaceRunChunk::empty();
        // Mixed → uniform → mixed: each refill must equal the fresh form.
        for chunk in [&mixed, &uniform, &mixed] {
            scratch.from_chunk_into(chunk);
            assert_eq!(scratch, SurfaceRunChunk::from_chunk(chunk));
            // Round-trip through the reused scratch is also exact.
            assert_eq!(scratch.to_chunk().blocks, chunk.blocks);
        }
        // The scratch retained its capacity across refills (reuse, not realloc).
        assert!(scratch.runs.capacity() >= COLS * 4);
    }
}
