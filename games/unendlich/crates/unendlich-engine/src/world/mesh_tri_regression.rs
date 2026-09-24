//! `world::mesh_tri_regression` — chunk mesh triangle-count regression guard.
//!
//! Motivated by a live diagnosis (2026-09, via `IndirectDrawSet::max_index_count`
//! added to the running game's per-second log): a single chunk was found
//! carrying 170,000-220,000 triangles -- for reference, `CHUNK_SIZE = 32`, so
//! even a maximally-dense, entirely-unculled 32^3 solid cube tops out around
//! 390K triangles, meaning a single "normal" chunk sitting at roughly half that
//! is wildly outside what greedy-meshed voxel terrain should ever produce
//! (hundreds to low thousands of triangles is typical). The spike tracked
//! `[worldgen] ruin at (...)` log lines closely enough across three
//! consecutive samples to be a real correlation, not noise: `stamp_ruins`
//! (`cpu_generator.rs`) scatters debris/rubble, and greedy meshing -- which
//! merges long runs of *identical adjacent* blocks into big quads, and works
//! great on terrain's long uniform stone/dirt runs -- falls apart on
//! scattered, mostly-isolated rubble, where nearly every block contributes
//! its own unmerged faces. Foliage placement (`flora_blocks.rs`) has the same
//! shape of risk: individually-placed leaves/fronds/decorations rather than
//! large uniform runs.
//!
//! This module doesn't fix the mesher or the structure/foliage generators --
//! it's a tripwire so a future change to either one that makes this worse
//! fails a test instead of silently shipping and being rediscovered live via
//! a GPU-watchdog self-stop. Two things it can catch:
//!   - `sample_grid_worst_case_stays_under_budget`: generates a grid of real
//!     chunks (terrain + whatever ruins/structures the deterministic seed
//!     happens to place in range) and asserts the worst one seen stays under
//!     `MAX_TRIS_ANY_SAMPLED_CHUNK`.
//!   - `foliage_overlay_stays_under_budget`: takes a plain generated chunk and
//!     overlays a full region's worth of flora on top (the densest case the
//!     real streaming path would ever produce for one chunk), asserting
//!     against `MAX_TRIS_FOLIAGE_OVERLAY`.
//!
//! # On the bound constants
//! `MAX_TRIS_ANY_SAMPLED_CHUNK` and `MAX_TRIS_FOLIAGE_OVERLAY` are a
//! first-pass guess, not calibrated against a real run of these tests (no
//! compiler in the environment that wrote them). They're set well above what
//! ordinary greedy-meshed terrain should produce but comfortably below the
//! ~170K-220K triangle range actually observed live, so a first real run
//! should be expected to either pass comfortably or reveal that these bounds
//! need tightening/loosening once real numbers are in hand -- treat the first
//! CI run's actual worst-case numbers as the real calibration data, and
//! consider ratcheting the constants down closer to that observed max once
//! it's known, so future regressions are caught earlier rather than only
//! once they approach today's already-bad worst case.

/// Triangle-count ceiling for the worst chunk seen across a sampled grid of
/// real generated chunks (terrain + any ruins the deterministic seed places
/// in range). See the module doc's "On the bound constants" section.
pub const MAX_TRIS_ANY_SAMPLED_CHUNK: usize = 90_000;

/// Triangle-count ceiling for a single chunk with a full region's flora
/// overlaid on top -- the densest single-chunk foliage case the real
/// streaming path (`region_flora_blocks`) would ever produce.
pub const MAX_TRIS_FOLIAGE_OVERLAY: usize = 90_000;

/// Mesh `chunk` with the plain (no sky/model) entry point and return its
/// triangle count. Pulled out as its own function so both tests (and any
/// future caller wanting a quick triangle-count check) share one path.
pub fn triangle_count_of(chunk: &crate::world::chunk::Chunk) -> usize {
    crate::render::mesher::mesh_chunk(chunk).indices.len() / 3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::CHUNK_SIZE;
    use crate::world::chunk::{Chunk, ChunkCoord};
    use crate::world::cpu_generator::WorldGenerator;

    /// Generate a grid of real chunks around a few y-bands likely to
    /// straddle the surface (ruins/structures are surface-anchored per
    /// `stamp_ruins`'s `base_y` check; foliage is likewise surface-anchored),
    /// mesh each, and return the worst `(coord, triangle_count)` seen.
    ///
    /// Grid size is deliberately modest (9x9 per y-band x 3 bands = 243
    /// chunks) -- enough to reliably catch at least one ruin at typical
    /// hit-rates (stamp_ruins hosts a ruin in ~38% of 80-block regions, and
    /// a 9x9-chunk span at CHUNK_SIZE=32 covers multiple such regions) without
    /// making the test slow enough to skip in practice.
    fn worst_case_in_sampled_grid(seed: u32) -> (ChunkCoord, usize) {
        let gen = WorldGenerator::new(seed);
        let mut worst: (ChunkCoord, usize) = (ChunkCoord::new(0, 0, 0), 0);
        for y in [2, 3, 4] {
            for cz in -4..=4 {
                for cx in -4..=4 {
                    let coord = ChunkCoord::new(cx, y, cz);
                    let chunk = gen.generate_chunk(coord);
                    let tris = triangle_count_of(&chunk);
                    if tris > worst.1 {
                        worst = (coord, tris);
                    }
                }
            }
        }
        worst
    }

    #[test]
    fn sample_grid_worst_case_stays_under_budget() {
        // Multiple seeds: a single seed could luck into (or out of) placing
        // a ruin in the sampled range at all. A few different seeds make
        // this a real regression guard rather than a coin flip on one map.
        for seed in [1u32, 12345, 999_983] {
            let (coord, tris) = worst_case_in_sampled_grid(seed);
            assert!(
                tris <= MAX_TRIS_ANY_SAMPLED_CHUNK,
                "seed {seed}: chunk {coord:?} produced {tris} triangles, exceeding the \
                 budget of {MAX_TRIS_ANY_SAMPLED_CHUNK} -- likely a mesh-hostile \
                 generation feature (ruin/debris/structure) regressed denser, or the \
                 mesher stopped greedy-merging something it used to. See this module's \
                 doc comment for the live incident this test was written to catch.",
            );
        }
    }

    /// r-1.2M-tri-diag: a first real run of `sample_grid_worst_case_stays_under_budget`
    /// (seed 1) hit chunk (4,3,1) at 1,210,776 triangles -- over 3x the
    /// *theoretical maximum* for a correctly face-culled 32^3 chunk (393,216,
    /// i.e. every one of its 32,768 blocks isolated as a lone speck exposing
    /// all 6 faces -- the worst case adversarial pattern possible). That
    /// number being mathematically exceeded rules out "just a lot of exposed
    /// surface" (dense rubble, dense foliage, missing neighbor culling on the
    /// outer shell -- all bounded well under that ceiling) and points at
    /// either duplicate face emission or the mesher/generator processing more
    /// than one chunk's worth of data. This test doesn't diagnose which --
    /// it dumps the numbers a debugger session needs to. Not a pass/fail
    /// guard; run with `-- --nocapture` and read the output.
    #[test]
    fn dump_diagnostics_for_1_2m_tri_chunk() {
        let gen = WorldGenerator::new(1);
        let coord = ChunkCoord::new(4, 3, 1);
        let chunk = gen.generate_chunk(coord);

        let non_air = chunk
            .blocks
            .iter()
            .filter(|b| !matches!(b, crate::world::blocks::BlockId::Air))
            .count();
        let expected_len = (CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize;
        // r-plant-voxel-diag: chunk.plant_indices is the "inverted plant
        // index" decorate_chunk_styled iterates to stamp a full voxelized
        // sub-grid template (subvoxel_templates::stamp_template) per plant
        // instance -- NOT a cheap 2-quad billboard. Per that module's own
        // doc, each template bakes from an 8^3 sub-grid with "~5-20 filled"
        // cells, each up to 6 faces before occlusion. This runs entirely
        // outside the three emission paths instrumented below (it's a
        // single call after the whole greedy-mask loop, not a per-face
        // special case), so it's exactly the kind of thing that would show
        // up as "unaccounted" in the breakdown -- reading it directly here
        // since the field is already public, no mesher.rs changes needed.
        let plant_count = chunk.plant_indices.len();

        crate::render::mesher::mesh_diag_reset();
        let mesh = crate::render::mesher::mesh_chunk(&chunk);
        let diag = crate::render::mesher::mesh_diag_snapshot();
        let tris = mesh.indices.len() / 3;
        let verts = mesh.vertices.len();

        println!(
            "chunk {coord:?}: blocks.len()={} (expected {expected_len}), non_air={non_air}/{expected_len} ({:.1}%), \
             plant_indices={plant_count}, \
             vertices={verts}, indices={}, triangles={tris}, \
             theoretical_max_triangles_if_every_block_isolated={}",
            chunk.blocks.len(),
            non_air as f32 / expected_len as f32 * 100.0,
            mesh.indices.len(),
            expected_len * 6 * 2,
        );
        println!(
            "  emission breakdown: subdivided_top: {} blocks -> {} quads ({} tris) | \
             jittered_top: {} blocks -> {} quads ({} tris) | soft_side: {} quads ({} tris) | \
             accounted_for={} tris, unaccounted (normal greedy-merge + decorate_chunk's plant \
             stamping + water/lava surfaces)={} tris{}",
            diag.subdivided_top_blocks, diag.subdivided_top_quads, diag.subdivided_top_quads * 2,
            diag.jittered_top_blocks, diag.jittered_top_quads, diag.jittered_top_quads * 2,
            diag.soft_side_faces, diag.soft_side_faces * 2,
            (diag.subdivided_top_quads + diag.jittered_top_quads + diag.soft_side_faces) * 2,
            (tris as i64) - ((diag.subdivided_top_quads + diag.jittered_top_quads + diag.soft_side_faces) as i64 * 2),
            if plant_count > 0 {
                format!(
                    " (if fully attributed to plant stamping: {:.1} tris/plant across {plant_count} plants)",
                    ((tris as i64) - ((diag.subdivided_top_quads + diag.jittered_top_quads + diag.soft_side_faces) as i64 * 2)) as f64
                        / plant_count as f64,
                )
            } else {
                " (plant_indices is empty -- rules out plant stamping entirely; \
                   the unaccounted total must be normal greedy-merge or water/lava surfaces)".to_string()
            },
        );
        // A few things worth reading off this once it actually runs:
        //   - blocks.len() != expected_len would mean generate_chunk itself
        //     returned a wrongly-sized array -- a generation bug, not a mesher one.
        //   - verts far exceeding non_air*8 (8 corners/block, generous upper
        //     bound before any culling/merging) would point at the mesher
        //     visiting cells outside this chunk's own volume.
        //   - tris > expected_len*6*2 (the theoretical max) with blocks.len()
        //     and non_air both looking normal would isolate the bug to
        //     mesh_chunk's merge/emission logic specifically.
        //   - subdivided_top_blocks/jittered_top_blocks should each be
        //     <= non_air (one call per exposed top face, at most one top per
        //     block) -- if either exceeds non_air, that IS a duplicate-
        //     emission bug, isolated to that specific function.
        //   - a large positive "unaccounted" tri count means the normal
        //     greedy-merge path (not instrumented directly, computed here by
        //     subtraction) is contributing more than the three named paths
        //     combined -- worth instrumenting that path directly next if so.
        println!(
            "(diagnostic only, not asserting -- see this test's doc comment for what to \
             read off the numbers above)"
        );
    }

    #[test]
    fn foliage_overlay_stays_under_budget() {
        let gen = WorldGenerator::new(42);
        let coord = ChunkCoord::new(0, 3, 0);
        let mut chunk = gen.generate_chunk(coord);
        let (ox, oy, oz) = coord.block_origin();

        // The densest single-chunk case the real streaming path produces:
        // every flora block a full CHUNK_SIZE x CHUNK_SIZE region would get,
        // stamped directly onto this one chunk's blocks (in reality flora
        // spans neighboring chunks too via region_flora_blocks_cached's
        // wider scan, so this actually concentrates MORE density into one
        // chunk than live play would -- a deliberately pessimistic worst case).
        let placed = crate::world::flora_blocks::region_flora_blocks(
            &gen, ox, oz, CHUNK_SIZE, CHUNK_SIZE,
        );
        for (pos, block) in placed {
            let (lx, ly, lz) = (pos[0] - ox, pos[1] - oy, pos[2] - oz);
            if (0..CHUNK_SIZE).contains(&lx)
                && (0..CHUNK_SIZE).contains(&ly)
                && (0..CHUNK_SIZE).contains(&lz)
            {
                chunk.blocks[Chunk::block_index(lx, ly, lz)] = block;
            }
        }

        let tris = triangle_count_of(&chunk);
        assert!(
            tris <= MAX_TRIS_FOLIAGE_OVERLAY,
            "chunk {coord:?} with a full region's flora overlaid produced {tris} \
             triangles, exceeding the budget of {MAX_TRIS_FOLIAGE_OVERLAY} -- foliage \
             placement got denser, or the mesher stopped greedy-merging it well. See \
             this module's doc comment for the live ruin incident this test's sibling \
             was written to catch; foliage was flagged as likely to do the same thing \
             eventually.",
        );
    }

    #[test]
    fn triangle_count_of_empty_chunk_is_zero() {
        // Sanity check on the helper itself, independent of world generation:
        // an all-air chunk has no faces to mesh at all.
        let coord = ChunkCoord::new(0, 0, 0);
        let blocks = vec![
            crate::world::blocks::BlockId::Air;
            (CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize
        ];
        let chunk = Chunk::from_blocks(coord, blocks);
        assert_eq!(triangle_count_of(&chunk), 0);
    }

    /// DIAGNOSTIC (ignored): where the worst sampled chunk's triangles come from --
    /// the mesher's per-emission-path counters and the chunk's block histogram.
    #[test]
    #[ignore = "diagnostic: cargo test --lib -- --ignored --nocapture where_the_triangles_come_from"]
    fn where_the_triangles_come_from() {
        let gen = crate::world::cpu_generator::WorldGenerator::new(1);
        for coord in [ChunkCoord::new(0, 3, -1), ChunkCoord::new(0, 2, 0)] {
            let chunk = gen.generate_chunk(coord);
            crate::render::mesher::mesh_diag_reset();
            let m = crate::render::mesher::mesh_chunk(&chunk);
            let d = crate::render::mesher::mesh_diag_snapshot();
            let mut hist: std::collections::BTreeMap<u32, usize> = Default::default();
            for y in 0..32 { for z in 0..32 { for x in 0..32 {
                *hist.entry(chunk.get(x, y, z) as u32).or_default() += 1;
            }}}
            let mut top: Vec<(u32, usize)> = hist.into_iter().collect();
            top.sort_by(|a, b| b.1.cmp(&a.1));
            eprintln!("DIAG {coord:?}: tris {} verts {} | subdiv_top quads {} blocks {} | jitter_top {} | soft_side {} | plant_stamps {}",
                m.indices.len() / 3, m.vertices.len(), d.subdivided_top_quads, d.subdivided_top_blocks,
                d.jittered_top_quads, d.soft_side_faces, d.plant_stamps);
            eprintln!("DIAG {coord:?}: blocks {:?}", &top[..top.len().min(10)]);
        }
    }
}
