//! UploadRepr — per-chunk GPU upload-representation routing policy.
//!
//! This is the *pure decision core* of perf-queue item #2 ("render far LOD from
//! surface-runs"). It answers one question, for one chunk, with no GPU and no
//! side effects:
//!
//! > Given this chunk's LOD band and its shape, **how should its geometry reach
//! > the GPU** — as a full octree, as run-length columns, or not at all?
//!
//! # Why this matters
//!
//! At `RENDER_DISTANCE=64` the octree pool fragmented and OOM'd: the runtime
//! reports each chunk's octree as ~180–196 KB but its surface-run form as ~5 KB
//! — a **~33× density gap**. The pool is the binding constraint on render
//! distance. If far chunks ride the cheap path, the same 256 MB pool holds
//! roughly 33× more of them, so the distance can climb back up without the
//! fragmentation death (and the buddy allocator keeps the pool from
//! re-fragmenting as those slots churn).
//!
//! The three destinations each already have a real GPU path in the tree:
//!   - [`UploadRepr::Octree`]     → `chunk_mesh_octree.comp` (full fidelity)
//!   - [`UploadRepr::SurfaceRun`] → `chunk_mesh_runs.comp`   (~5 KB resident)
//!   - [`UploadRepr::Procedural`] → `chunk_generation.comp`  (0 bytes resident;
//!     the chunk is byte-identical to a fresh GPU regeneration from its seed, so
//!     it is regenerated on demand instead of being uploaded at all)
//!
//! # Architecture seam
//!
//! This module is *render-feature-free* on purpose: it lives in `world/` beside
//! [`crate::world::lod`] and [`crate::world::remesh_policy`], depends only on
//! `LodLevel`, `SurfaceRunChunk`, and `constants`, and is fully unit-tested
//! under `--no-default-features`. The GPU draw paths that consume the decision
//! live behind `feature = "render"` (PARALLEL's side). Wiring is:
//!
//! ```ignore
//! let srun  = SurfaceRunChunk::from_chunk(&chunk);          // (often cached)
//! let stats = ChunkStats::from_surface_run(&srun, lod, chunk.is_edited());
//! match upload_repr::classify(&stats, &policy) {
//!     UploadRepr::Octree     => upload_octree(&chunk),
//!     UploadRepr::SurfaceRun => upload_runs(&srun),
//!     UploadRepr::Procedural => { /* nothing — GPU regenerates on demand */ }
//! }
//! ```
//!
//! `chunk.is_edited()` is whatever the engine already uses to mark a chunk as
//! diverged from worldgen (the "edited chunks" persistence set — the same
//! chunks the shutdown path saves). When a chunk has *not* diverged, regenerating
//! it from seed is exact, so it needs no resident bytes.

use crate::constants::CHUNK_SIZE;
use crate::world::lod::LodLevel;
use crate::world::surface_run::SurfaceRunChunk;

/// Columns per chunk (`CHUNK_SIZE²`); equals `SurfaceRunChunk::col_runs.len()`
/// for any non-uniform chunk.
const COLS: usize = (CHUNK_SIZE as usize) * (CHUNK_SIZE as usize);

/// How a chunk's geometry should be made resident on the GPU.
///
/// Ordered cheapest-last is *not* meaningful here (the variants are a tagged
/// choice, not a scale), so this enum is deliberately not `Ord`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum UploadRepr {
    /// Full GPU octree (~180–196 KB). Highest fidelity, editable, AO-correct.
    /// The near band lives here, as do far chunks too noisy for runs to help.
    Octree,
    /// Run-length columns (~5 KB resident). The cheap workhorse for mid/far
    /// chunks of modest column complexity.
    SurfaceRun,
    /// Nothing uploaded. The chunk reproduces exactly from the worldgen seed, so
    /// the GPU regenerates it on demand. Reserved for untouched far/very-far
    /// terrain — the bulk of a large world, and the memory win that unlocks
    /// high render distance.
    Procedural,
}

/// The shape facts the policy needs about one chunk. Cheap to build from a
/// [`SurfaceRunChunk`] the engine already computes for compression accounting.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ChunkStats {
    /// This chunk's desired LOD band for the current frame.
    pub lod: LodLevel,
    /// `true` if the chunk has diverged from what the generator would produce
    /// (player edits / persisted). When `false`, a GPU regeneration from seed is
    /// byte-identical, so the chunk needs no resident upload.
    pub edited: bool,
    /// `SurfaceRunChunk.uniform.is_some()` — the whole chunk is a single block
    /// id (sky / deep stone). Encodes in one byte; never "noisy".
    pub uniform: bool,
    /// `max(col_runs)` — the noisiest column's run count, in `0..=CHUNK_SIZE`.
    /// `1` is a flat column; `CHUNK_SIZE` is pathological alternation where a run
    /// encoding saves nothing and the octree mesher is preferable. `0` only for
    /// uniform chunks (which carry no per-column data).
    pub worst_col_runs: u8,
    /// `runs.len()` — total runs across all columns. Drives the resident byte
    /// size of the [`UploadRepr::SurfaceRun`] path (see [`srun_bytes`]).
    pub total_runs: u32,
}

impl ChunkStats {
    /// Derive stats from a compressed chunk plus its band and edited flag.
    ///
    /// For a uniform chunk `col_runs`/`runs` are empty, so `worst_col_runs` and
    /// `total_runs` are `0` — exactly the "never noisy, trivially cheap" case.
    pub fn from_surface_run(srun: &SurfaceRunChunk, lod: LodLevel, edited: bool) -> Self {
        ChunkStats {
            lod,
            edited,
            uniform: srun.uniform.is_some(),
            worst_col_runs: srun.col_runs.iter().copied().max().unwrap_or(0),
            total_runs: srun.runs.len() as u32,
        }
    }
}

/// Tunable thresholds for [`classify`]. Defaults are deliberately conservative;
/// they are runtime knobs once the GPU paths are wired (mirror the existing
/// `UNENDLICH_*` env-flag pattern).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct UploadPolicy {
    /// LOD at/beyond which edited or unedited chunks may use the run encoding
    /// instead of a full octree. Bands strictly below this always get an octree.
    pub surface_run_from: LodLevel,
    /// LOD at/beyond which *untouched* chunks are dropped to the procedural path
    /// (zero bytes resident). Held one band out from `surface_run_from` so the
    /// mid band keeps cheap-but-resident runs (no regeneration thrash as the
    /// player strafes), while the genuinely-far band pays nothing.
    pub procedural_from: LodLevel,
    /// Above this worst-column run count, a chunk's columns are too noisy for the
    /// run encoding to beat the octree, so it stays an octree even when far.
    pub max_worst_col_runs: u8,
    /// Above this total run count the run encoding's bytes approach the octree's
    /// and the win evaporates; keep the octree. (~`COLS * 8` ≈ 8 runs/column.)
    pub max_total_runs: u32,
}

impl Default for UploadPolicy {
    fn default() -> Self {
        UploadPolicy {
            surface_run_from: LodLevel::Mid,
            procedural_from: LodLevel::Far,
            max_worst_col_runs: 24,
            max_total_runs: (COLS as u32) * 8,
        }
    }
}

/// `true` when a chunk's columns are too noisy for the run encoding to be worth
/// it. Uniform chunks are never noisy (their run encoding is a single byte).
#[inline]
fn too_noisy(stats: &ChunkStats, policy: &UploadPolicy) -> bool {
    !stats.uniform
        && (stats.worst_col_runs > policy.max_worst_col_runs
            || stats.total_runs > policy.max_total_runs)
}

/// Decide how this chunk should reach the GPU. Pure and deterministic.
///
/// The ordering encodes the priorities:
///   1. An untouched chunk far enough out costs nothing — regenerate it.
///   2. Otherwise, if it's far enough for runs and not pathologically noisy,
///      upload the cheap run form.
///   3. Otherwise (near band, or a noisy far chunk runs can't compress) upload
///      the full octree.
pub fn classify(stats: &ChunkStats, policy: &UploadPolicy) -> UploadRepr {
    // 1. Untouched + far → free regeneration, no resident bytes. Noise is
    //    irrelevant here: the GPU rebuilds it from seed either way.
    if !stats.edited && stats.lod >= policy.procedural_from {
        return UploadRepr::Procedural;
    }
    // 2. Far enough for the run encoding, and the columns compress well.
    if stats.lod >= policy.surface_run_from && !too_noisy(stats, policy) {
        return UploadRepr::SurfaceRun;
    }
    // 3. Near band, or an edited/noisy far chunk: full fidelity.
    UploadRepr::Octree
}

/// Resident bytes of the [`UploadRepr::SurfaceRun`] path for these stats,
/// mirroring `SurfaceRunChunk::byte_size()` exactly: one byte when uniform, else
/// the per-column header (`COLS`) plus two bytes per run. `Octree` is variable
/// and `Procedural` is zero, so only the run path has a stable closed form here.
pub fn srun_bytes(stats: &ChunkStats) -> usize {
    if stats.uniform {
        return 1;
    }
    COLS + stats.total_runs as usize * 2
}

/// Aggregate pool-occupancy projection over a set of chunks under a policy.
///
/// This is the *measurement tool* for de-risking the GPU draw path: run it over
/// the loaded-chunk set at RD=16/32/64 and it tells you how much the octree pool
/// would actually shrink if the far bands rode the cheap paths — **before**
/// changing a line of GPU code. Compare `octree_bytes` against the 256 MB pool.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct ResidencyProjection {
    /// Chunks routed to the full octree (the only ones consuming the octree pool).
    pub octree_chunks: u32,
    /// Total octree-pool bytes those chunks would consume.
    pub octree_bytes: u64,
    /// Chunks routed to the cheap run buffer.
    pub surface_run_chunks: u32,
    /// Total run-buffer bytes (exact, via [`srun_bytes`]).
    pub surface_run_bytes: u64,
    /// Chunks needing no resident upload (regenerated from seed on demand).
    pub procedural_chunks: u32,
}

impl ResidencyProjection {
    /// Total chunks across all three representations.
    pub fn total_chunks(&self) -> u32 {
        self.octree_chunks + self.surface_run_chunks + self.procedural_chunks
    }
    /// What the octree pool *would* cost if every chunk stayed an octree — the
    /// baseline the routing improves on. `octree_bytes` is the projected actual.
    pub fn all_octree_baseline_bytes(&self, octree_bytes_each: u64) -> u64 {
        self.total_chunks() as u64 * octree_bytes_each
    }
}

/// Project pool occupancy for an iterator of chunk stats under `policy`.
///
/// `octree_bytes_each` is the measured per-chunk octree cost — feed it the
/// runtime's `octree_per_chunk_kb * 1024` (≈169–196 KB in the RD-6 logs).
/// SurfaceRun bytes are computed exactly; Procedural costs nothing.
pub fn project_residency<I>(
    stats: I,
    policy: &UploadPolicy,
    octree_bytes_each: u64,
) -> ResidencyProjection
where
    I: IntoIterator<Item = ChunkStats>,
{
    let mut proj = ResidencyProjection::default();
    for s in stats {
        match classify(&s, policy) {
            UploadRepr::Octree => {
                proj.octree_chunks += 1;
                proj.octree_bytes += octree_bytes_each;
            }
            UploadRepr::SurfaceRun => {
                proj.surface_run_chunks += 1;
                proj.surface_run_bytes += srun_bytes(&s) as u64;
            }
            UploadRepr::Procedural => {
                proj.procedural_chunks += 1;
            }
        }
    }
    proj
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::blocks::BlockId;
    use crate::world::chunk::{Chunk, ChunkCoord};
    use crate::constants::CHUNK_VOLUME;

    fn stats(lod: LodLevel, edited: bool, uniform: bool, worst: u8, total: u32) -> ChunkStats {
        ChunkStats { lod, edited, uniform, worst_col_runs: worst, total_runs: total }
    }

    // A modest, well-compressing chunk (typical surface terrain).
    fn modest(lod: LodLevel, edited: bool) -> ChunkStats {
        stats(lod, edited, false, 4, 4096)
    }

    #[test]
    fn near_is_always_octree() {
        let p = UploadPolicy::default();
        // Regardless of edited / uniform / noise, the near band is full fidelity.
        assert_eq!(classify(&modest(LodLevel::Near, false), &p), UploadRepr::Octree);
        assert_eq!(classify(&modest(LodLevel::Near, true), &p), UploadRepr::Octree);
        assert_eq!(
            classify(&stats(LodLevel::Near, false, true, 0, 0), &p),
            UploadRepr::Octree
        );
    }

    #[test]
    fn untouched_far_is_procedural() {
        let p = UploadPolicy::default();
        assert_eq!(classify(&modest(LodLevel::Far, false), &p), UploadRepr::Procedural);
        assert_eq!(classify(&modest(LodLevel::VeryFar, false), &p), UploadRepr::Procedural);
    }

    #[test]
    fn edited_far_modest_is_surface_run() {
        let p = UploadPolicy::default();
        // Edited → can't regenerate; modest complexity → runs are the win.
        assert_eq!(classify(&modest(LodLevel::Far, true), &p), UploadRepr::SurfaceRun);
    }

    #[test]
    fn edited_far_noisy_falls_back_to_octree() {
        let p = UploadPolicy::default();
        // Noisy by worst-column count...
        let noisy_cols = stats(LodLevel::Far, true, false, 30, 4096);
        assert_eq!(classify(&noisy_cols, &p), UploadRepr::Octree);
        // ...or noisy by total runs.
        let noisy_total = stats(LodLevel::Far, true, false, 4, (COLS as u32) * 16);
        assert_eq!(classify(&noisy_total, &p), UploadRepr::Octree);
    }

    #[test]
    fn untouched_mid_is_surface_run_not_procedural() {
        let p = UploadPolicy::default();
        // Mid is below procedural_from (Far), so an untouched mid chunk stays
        // resident as cheap runs rather than re-generating every time it appears.
        assert_eq!(classify(&modest(LodLevel::Mid, false), &p), UploadRepr::SurfaceRun);
    }

    #[test]
    fn uniform_is_never_too_noisy() {
        let p = UploadPolicy::default();
        // A uniform chunk reports worst=0,total=0 so it can never trip the noise
        // gate; at a run-eligible band it is always the cheap path.
        let u_far_edited = stats(LodLevel::Far, true, true, 0, 0);
        assert_eq!(classify(&u_far_edited, &p), UploadRepr::SurfaceRun);
        // Even if the fields were (impossibly) high, uniform short-circuits noise.
        assert!(!too_noisy(&stats(LodLevel::Far, true, true, 32, 99_999), &p));
    }

    #[test]
    fn procedural_beats_noise() {
        let p = UploadPolicy::default();
        // Untouched + far wins even when the columns are pathologically noisy:
        // regeneration is exact and free of resident bytes regardless of shape.
        let s = stats(LodLevel::VeryFar, false, false, 32, (COLS as u32) * 32);
        assert_eq!(classify(&s, &p), UploadRepr::Procedural);
    }

    #[test]
    fn farther_bands_never_get_more_expensive() {
        // Holding shape fixed, moving a chunk outward must only make its repr
        // cheaper-or-equal, never more expensive. Encode cost rank Octree(2) >
        // SurfaceRun(1) > Procedural(0) and assert monotonic non-increasing.
        let p = UploadPolicy::default();
        let rank = |r: UploadRepr| match r {
            UploadRepr::Octree => 2,
            UploadRepr::SurfaceRun => 1,
            UploadRepr::Procedural => 0,
        };
        for &edited in &[false, true] {
            let bands = [LodLevel::Near, LodLevel::Mid, LodLevel::Far, LodLevel::VeryFar];
            let mut prev = i32::MAX;
            for &b in &bands {
                let cost = rank(classify(&modest(b, edited), &p));
                assert!(cost <= prev, "cost rose moving outward at {:?} (edited={})", b, edited);
                prev = cost;
            }
        }
    }

    #[test]
    fn srun_bytes_matches_byte_size_formula() {
        // Uniform → 1 byte.
        assert_eq!(srun_bytes(&stats(LodLevel::Far, true, true, 0, 0)), 1);
        // Non-uniform → header + 2 bytes/run, matching SurfaceRunChunk::byte_size.
        let s = stats(LodLevel::Far, true, false, 4, 4096);
        assert_eq!(srun_bytes(&s), COLS + 4096 * 2);
    }

    #[test]
    fn policy_is_tunable() {
        // Pulling procedural_from in to Mid makes untouched mid chunks free.
        let aggressive = UploadPolicy { procedural_from: LodLevel::Mid, ..Default::default() };
        assert_eq!(classify(&modest(LodLevel::Mid, false), &aggressive), UploadRepr::Procedural);
        // Pushing surface_run_from out to VeryFar keeps Far chunks on the octree.
        let conservative = UploadPolicy { surface_run_from: LodLevel::VeryFar, ..Default::default() };
        assert_eq!(classify(&modest(LodLevel::Far, true), &conservative), UploadRepr::Octree);
    }

    #[test]
    fn is_deterministic() {
        let p = UploadPolicy::default();
        let s = modest(LodLevel::Far, true);
        let first = classify(&s, &p);
        for _ in 0..100 {
            assert_eq!(classify(&s, &p), first);
        }
    }

    #[test]
    fn from_surface_run_maps_fields() {
        let coord = ChunkCoord { x: 0, y: 0, z: 0 };

        // Uniform all-air chunk → uniform=true, no per-column data.
        let air = Chunk::from_blocks(coord, vec![BlockId::Air; CHUNK_VOLUME]);
        let s_air = SurfaceRunChunk::from_chunk(&air);
        let cs_air = ChunkStats::from_surface_run(&s_air, LodLevel::Far, false);
        assert!(cs_air.uniform);
        assert_eq!(cs_air.worst_col_runs, 0);
        assert_eq!(cs_air.total_runs, 0);
        assert_eq!(classify(&cs_air, &UploadPolicy::default()), UploadRepr::Procedural);

        // A two-layer chunk (air over stone): non-uniform, modest runs.
        let mut blocks = vec![BlockId::Air; CHUNK_VOLUME];
        let cs = CHUNK_SIZE as usize;
        // Fill the bottom half of every column with stone.
        for lz in 0..cs {
            for lx in 0..cs {
                for ly in 0..(cs / 2) {
                    blocks[lx + lz * cs + ly * cs * cs] = BlockId::Stone;
                }
            }
        }
        let mixed = Chunk::from_blocks(coord, blocks);
        let s_mixed = SurfaceRunChunk::from_chunk(&mixed);
        let cs_mixed = ChunkStats::from_surface_run(&s_mixed, LodLevel::Far, true);
        assert!(!cs_mixed.uniform);
        // Each column is two runs (stone then air, or air then stone): worst = 2.
        assert_eq!(cs_mixed.worst_col_runs, 2);
        assert_eq!(cs_mixed.total_runs as usize, COLS * 2);
        // Edited + far + well-compressed → the cheap run path.
        assert_eq!(classify(&cs_mixed, &UploadPolicy::default()), UploadRepr::SurfaceRun);
    }

    const OCT: u64 = 180 * 1024; // ~180 KB/chunk, mid-range of the RD-6 logs.

    #[test]
    fn project_empty_is_zero() {
        let empty: Vec<ChunkStats> = Vec::new();
        let p = project_residency(empty, &UploadPolicy::default(), OCT);
        assert_eq!(p, ResidencyProjection::default());
        assert_eq!(p.total_chunks(), 0);
    }

    #[test]
    fn project_all_near_is_the_octree_baseline() {
        // RD-6 reality: every chunk is Near → every chunk is an octree, so the
        // projection equals the all-octree baseline and nothing rides the cheap
        // paths. This is exactly why the far-LOD machinery is dormant at RD-6.
        let pol = UploadPolicy::default();
        let near: Vec<_> = (0..100).map(|_| modest(LodLevel::Near, true)).collect();
        let proj = project_residency(near, &pol, OCT);
        assert_eq!(proj.octree_chunks, 100);
        assert_eq!(proj.surface_run_chunks, 0);
        assert_eq!(proj.procedural_chunks, 0);
        assert_eq!(proj.octree_bytes, proj.all_octree_baseline_bytes(OCT));
    }

    #[test]
    fn project_far_world_collapses_the_octree_pool() {
        // A high-RD-shaped population: a near core, then mostly untouched far
        // terrain with a sprinkle of edited far chunks. The octree footprint
        // should fall to a small fraction of the all-octree baseline — the whole
        // point of #2.
        let pol = UploadPolicy::default();
        let mut chunks = Vec::new();
        chunks.extend((0..50).map(|_| modest(LodLevel::Near, true))); // near core → octree
        chunks.extend((0..800).map(|_| modest(LodLevel::VeryFar, false))); // untouched far → procedural
        chunks.extend((0..150).map(|_| modest(LodLevel::Far, true))); // edited far → surface-run

        let proj = project_residency(chunks, &pol, OCT);
        assert_eq!(proj.octree_chunks, 50);
        assert_eq!(proj.procedural_chunks, 800);
        assert_eq!(proj.surface_run_chunks, 150);

        let baseline = proj.all_octree_baseline_bytes(OCT); // 1000 chunks all-octree
        // Only the 50 near chunks hit the octree pool: 5% of the baseline.
        assert_eq!(proj.octree_bytes, 50 * OCT);
        assert!(proj.octree_bytes * 10 < baseline, "octree pool should collapse to <10% of baseline");
        // The run buffer costs far less than those same chunks would as octrees.
        let runs_as_octree = proj.surface_run_chunks as u64 * OCT;
        assert!(proj.surface_run_bytes * 10 < runs_as_octree, "runs should be <10% of octree for the same chunks");
    }

    #[test]
    fn project_is_deterministic() {
        let pol = UploadPolicy::default();
        let mk = || vec![
            modest(LodLevel::Near, true),
            modest(LodLevel::Far, false),
            modest(LodLevel::Far, true),
            stats(LodLevel::VeryFar, true, false, 30, 4096), // noisy edited far → octree
        ];
        let a = project_residency(mk(), &pol, OCT);
        let b = project_residency(mk(), &pol, OCT);
        assert_eq!(a, b);
        // Near+edited and the noisy very-far+edited chunk both land on the
        // octree; far+edited→runs; far+untouched→procedural.
        assert_eq!(a.octree_chunks, 2);
        assert_eq!(a.surface_run_chunks, 1);
        assert_eq!(a.procedural_chunks, 1);
    }
}
