//! `world::octree_encode_choice` — SPECULATIVE per-chunk representation chooser.
//!
//! ============================ SPECULATIVE / UNVERIFIED ============================
//! Authored by the COORDINATOR as a stand-in for worker r62-04-octree-encode-or-skip,
//! which returned no implementation. This file has NOT been compiled or benchmarked:
//! the coordinator sandbox has no Rust toolchain and static.rust-lang.org is
//! network-blocked, so the brief's deliverables (perf-probe `octree_ratio`,
//! `rt_fail=0`, world-bench "all match", winding-check, HARNESS GREEN) could NOT be
//! produced here. DO NOT MERGE on this file alone — compile, run the round-trip
//! tests, and measure perf-probe on a toolchain box (M2 / worker env) first.
//! =================================================================================
//!
//! Distinct from the existing `world::octree_compact` (that one is leaf re-merge /
//! recompaction — the *how* of shrinking an edited tree). THIS module decides which
//! *encoding* a chunk is stored in: sparse octree vs dense palette+RLE, per chunk.
//!
//! ## Problem (brief r62-04)
//! perf-probe shows the sparse octree is ~4× dense-u8 size for mixed terrain. The
//! octree only wins on homogeneous regions.
//!
//! ## Approach
//! Per chunk, compare the sparse octree byte size (`node_count × OCTREE_NODE_BYTES`)
//! to a dense palette+RLE encoding of the same chunk, and keep the smaller.
//! `to_blocks` reconstructs from whichever was chosen. Public round-trip contract
//! identical: encode→decode reproduces the chunk's blocks exactly.
//!
//! ## SCOPE LIMIT a reviewer MUST resolve before merge
//! `OctreeArray.nodes` is read directly by the GPU upload path and by
//! `chunk_manager` (`byte_size`, `node_count`). The GPU SSBO format REQUIRES the
//! sparse node table. So this chooser can only shrink CPU-resident / cold-stored /
//! serialized chunks — NOT chunks currently uploaded to the GPU (those still need
//! `nodes`). If perf-probe's `octree_ratio` measures GPU SSBO bytes, this is a
//! CLEAN NEGATIVE: the sparse tree is already the mandated GPU format and can't be
//! shrunk without a shader change (OFF-LIMITS per the brief). If it measures CPU
//! residency / serialization, this should drive the ratio well below 1.0 on mixed
//! chunks. The integrator must confirm which metric perf-probe reports.
//!
//! ## Integration notes
//! - On wiring in: `use crate::world::octree::OCTREE_NODE_BYTES` instead of the
//!   local copy; feed `sparse_node_count` from `OctreeArray::from_chunk(chunk).node_count()`;
//!   feed `blocks` from the chunk's linear block slice (`BlockId as u8`).
//! - The round-trip tests below use synthetic `u8` slices. Re-run the REAL
//!   round-trip (`Chunk` → choose → `to_blocks` → `Chunk::from_blocks`) in
//!   perf-probe / octree-bench before acceptance.

#![allow(dead_code)]

/// Mirror of `octree::OCTREE_NODE_BYTES` (40). On integration, import the real one.
pub const OCTREE_NODE_BYTES: usize = 40;

// ---------------------------------------------------------------------------
// Dense palette + RLE
// ---------------------------------------------------------------------------

/// A dense chunk encoded as a small block-id palette plus run-length pairs over
/// linear voxel order. Compact for chunks with few distinct blocks and long runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteRle {
    /// Distinct block ids, first-seen order. `runs` indexes into this.
    pub palette: Vec<u8>,
    /// `(palette_index, run_length)` over linear voxel order; run_lengths sum to volume.
    pub runs: Vec<(u16, u32)>,
}

impl PaletteRle {
    /// Encode a linear block-id slice (len == volume). Pure, deterministic.
    pub fn encode(blocks: &[u8]) -> Self {
        let mut palette: Vec<u8> = Vec::new();
        let mut runs: Vec<(u16, u32)> = Vec::new();
        let mut i = 0usize;
        while i < blocks.len() {
            let b = blocks[i];
            let pal_idx = match palette.iter().position(|&p| p == b) {
                Some(idx) => idx as u16,
                None => { palette.push(b); (palette.len() - 1) as u16 }
            };
            let mut run = 1u32;
            while i + (run as usize) < blocks.len() && blocks[i + run as usize] == b {
                run += 1;
            }
            runs.push((pal_idx, run));
            i += run as usize;
        }
        PaletteRle { palette, runs }
    }

    /// Decode back to a linear block-id Vec of length `volume`.
    pub fn decode(&self, volume: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(volume);
        for &(pal_idx, run) in &self.runs {
            let b = self.palette.get(pal_idx as usize).copied().unwrap_or(0);
            for _ in 0..run {
                if out.len() == volume { break; }
                out.push(b);
            }
        }
        while out.len() < volume {
            out.push(self.palette.first().copied().unwrap_or(0));
        }
        out.truncate(volume);
        out
    }

    /// Estimated serialized byte size: 8 header + palette + 6 bytes/run.
    pub fn byte_size(&self) -> usize {
        8 + self.palette.len() + self.runs.len() * 6
    }
}

// ---------------------------------------------------------------------------
// Chooser
// ---------------------------------------------------------------------------

/// Which encoding a chunk is stored in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChunkRepr {
    /// Sparse octree won (homogeneous-ish). `blocks` kept so this review shim is
    /// representation-independent; on integration this variant holds the real
    /// `OctreeArray` and `to_blocks` calls `OctreeArray::to_chunk`.
    Sparse { node_count: usize, blocks: Vec<u8> },
    /// Dense palette+RLE won (heterogeneous).
    Dense(PaletteRle),
}

/// Decision result + both candidate sizes, so callers can log the perf ratio.
#[derive(Clone, Debug)]
pub struct ChooseResult {
    pub repr: ChunkRepr,
    pub sparse_bytes: usize,
    pub dense_bytes: usize,
}

impl ChooseResult {
    /// `chosen_bytes / sparse_bytes` — perf-probe `octree_ratio`. < 1.0 = saved memory.
    pub fn ratio(&self) -> f32 {
        if self.sparse_bytes == 0 { return 1.0; }
        self.chosen_bytes() as f32 / self.sparse_bytes as f32
    }
    pub fn chosen_bytes(&self) -> usize {
        match &self.repr {
            ChunkRepr::Sparse { node_count, .. } => node_count * OCTREE_NODE_BYTES,
            ChunkRepr::Dense(p) => p.byte_size(),
        }
    }
}

/// Choose the smaller of {sparse octree, dense palette+RLE}.
///
/// `sparse_node_count` = `OctreeArray::from_chunk(chunk).node_count()` (caller-supplied
/// so this stays independent of the octree builder). `blocks` = chunk's linear
/// block-id slice (len == volume). Pure, deterministic.
pub fn choose(blocks: &[u8], sparse_node_count: usize) -> ChooseResult {
    let sparse_bytes = sparse_node_count * OCTREE_NODE_BYTES;
    let dense = PaletteRle::encode(blocks);
    let dense_bytes = dense.byte_size();
    let repr = if dense_bytes < sparse_bytes {
        ChunkRepr::Dense(dense)
    } else {
        ChunkRepr::Sparse { node_count: sparse_node_count, blocks: blocks.to_vec() }
    };
    ChooseResult { repr, sparse_bytes, dense_bytes }
}

impl ChunkRepr {
    /// Reconstruct the linear block-id Vec regardless of representation.
    pub fn to_blocks(&self, volume: usize) -> Vec<u8> {
        match self {
            ChunkRepr::Sparse { blocks, .. } => {
                let mut b = blocks.clone();
                b.resize(volume, b.first().copied().unwrap_or(0));
                b
            }
            ChunkRepr::Dense(p) => p.decode(volume),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (synthetic slices; integration must re-run the real Chunk round-trip)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const VOL: usize = 32 * 32 * 32;

    fn homogeneous(block: u8) -> Vec<u8> { vec![block; VOL] }
    fn striped() -> Vec<u8> { (0..VOL).map(|i| ((i / 256) % 4) as u8).collect() }
    fn noisy() -> Vec<u8> {
        let mut s = 0x51A9_F00Du32; // documented seed (R20-style)
        (0..VOL).map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((s >> 24) % 6) as u8
        }).collect()
    }

    #[test]
    fn rle_round_trip_homogeneous() {
        let blocks = homogeneous(3);
        let p = PaletteRle::encode(&blocks);
        assert_eq!(p.decode(VOL), blocks);
        assert_eq!(p.runs.len(), 1, "homogeneous chunk = one run");
    }

    #[test]
    fn rle_round_trip_striped() {
        let blocks = striped();
        assert_eq!(PaletteRle::encode(&blocks).decode(VOL), blocks);
    }

    #[test]
    fn rle_round_trip_noisy() {
        let blocks = noisy();
        assert_eq!(PaletteRle::encode(&blocks).decode(VOL), blocks);
    }

    #[test]
    fn rle_palette_distinct() {
        let p = PaletteRle::encode(&striped());
        let mut s = p.palette.clone(); s.sort_unstable(); s.dedup();
        assert_eq!(s.len(), p.palette.len());
    }

    #[test]
    fn rle_runs_sum_to_volume() {
        for blocks in [homogeneous(1), striped(), noisy()] {
            let total: u32 = PaletteRle::encode(&blocks).runs.iter().map(|&(_, r)| r).sum();
            assert_eq!(total as usize, VOL);
        }
    }

    #[test]
    fn heterogeneous_prefers_dense() {
        let blocks = striped();
        let r = choose(&blocks, 8000);
        assert!(matches!(r.repr, ChunkRepr::Dense(_)));
        assert!(r.ratio() < 1.0, "ratio should be < 1.0: {}", r.ratio());
        assert_eq!(r.repr.to_blocks(VOL), blocks);
    }

    #[test]
    fn round_trip_exact_either_repr() {
        let blocks = noisy();
        for nc in [1usize, 20_000] {
            assert_eq!(choose(&blocks, nc).repr.to_blocks(VOL), blocks, "nc={nc}");
        }
    }

    #[test]
    fn ratio_never_exceeds_one() {
        for blocks in [homogeneous(0), striped(), noisy()] {
            for nc in [1usize, 100, 5000, 30000] {
                let r = choose(&blocks, nc);
                assert!(r.ratio() <= 1.0 + 1e-6, "picked larger repr: {} nc={nc}", r.ratio());
            }
        }
    }

    #[test]
    fn choose_deterministic() {
        let blocks = noisy();
        let a = choose(&blocks, 4000);
        let b = choose(&blocks, 4000);
        assert_eq!(a.repr, b.repr);
        assert_eq!(a.chosen_bytes(), b.chosen_bytes());
    }

    #[test]
    fn empty_palette_decode_safe() {
        let p = PaletteRle { palette: vec![], runs: vec![] };
        assert_eq!(p.decode(VOL).len(), VOL);
    }
}
