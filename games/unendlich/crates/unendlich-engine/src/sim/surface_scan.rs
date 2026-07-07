//! `sim::surface_scan` — read the **actual** top solid voxel of a generated
//! column, so placement can stop trusting the CPU FP64 *prediction*.
//!
//! WID: `r330-world` (dispatch r330, section `T-world`). Grounds
//! `NEXT_SESSION_GUIDE.md §4a` — the highest-value bug.
//!
//! ## The bug this core exists to kill
//! Terrain voxels are filled by the **GPU compute generator in FP32** (ported
//! for Honeykrisp / Apple M2), but `world/chunk_manager.rs::surface_y_at`
//! (~line 1831) returns the **CPU FP64** noise *prediction*
//! (`self.generator.cpu().surface_y_at`). Flora placement, mob ground/physics
//! and spawning all read that prediction, so on FP32-generated terrain the
//! predicted surface and the real top block disagree: plants/mobs float, trees
//! root on air, water sits below `SEA_LEVEL = 64` (`src/constants.rs:23`).
//!
//! Fix direction (a) from §4a: **placement reads the actual top solid block of
//! the generated chunk.** This module is that read — pure, deterministic,
//! generator-free. It scans voxels that already exist; it never re-predicts.
//!
//! ## Purity (SHARED_RULES §1)
//! No IO / clock / RNG. No `unwrap` / `expect` / `panic!`. No allocation in the
//! scan paths. Saturating arithmetic on the Y math. The core is fully
//! self-contained (no `crate::` references), so it gates standalone with
//! `rustc --edition 2021 --test surface_scan.rs` — the documented fast floor,
//! the same gate `sim::net_channels` used.
//!
//! ## Solidity
//! "Top solid" means the first block, scanning down from the top, that a body
//! or plant base can actually rest on. That is the engine's coarse
//! `BlockId::is_solid()` notion: **not** air, fluid (water/lava) or thin plant
//! decoration (flowers, grass, ferns, vines, cobweb, mushroom, lichen). The
//! core takes that judgement as a predicate / trait method so it stays
//! dependency-free; the bin bridges `BlockId` in (handoff md) with a one-liner
//! — no shared-file edit, no new impl needed because the predicate form accepts
//! `|b| b.is_solid()` directly.

/// The only voxel property the surface scan needs: can a body rest on it?
///
/// The engine bridges `world::blocks::BlockId` to this by passing
/// `|b: &BlockId| b.is_solid()` to the `_by` variants — so no impl on the
/// foreign type is required at all. The trait form is provided for callers who
/// prefer it (and to give the brief's `top_solid_y(column, y_min)` signature a
/// concrete shape).
pub trait Voxel {
    /// `true` if a body/plant base can rest on this block (mirrors
    /// `BlockId::is_solid()`: excludes air, fluids and thin plant decoration).
    fn is_solid(&self) -> bool;
}

/// Top-down scan of a **contiguous** voxel column for the first solid block.
///
/// `column[i]` is the voxel at world `y = y_min + i` (index 0 is the bottom).
/// Scanning top-down, return the world Y of the first solid voxel, or `None`
/// if the column is empty or entirely non-solid (all air / fluid / plants).
///
/// This is the brief's `top_solid_y(column: &[Voxel], y_min) -> Option<i32>`,
/// generic over the solidity trait. Allocation-free; O(column.len()) worst case
/// but returns on the first solid from the top, so it is O(air-thickness) in the
/// common open-sky case.
pub fn top_solid_y<V: Voxel>(column: &[V], y_min: i32) -> Option<i32> {
    top_solid_y_by(column, y_min, |v| v.is_solid())
}

/// Predicate form of [`top_solid_y`] — the integration-facing entry point.
///
/// Lets the bin pass `|b: &BlockId| b.is_solid()` (or any narrower placement
/// predicate, e.g. `blocks_collision`) with **zero** trait impls and **zero**
/// shared-file edits. Same semantics as [`top_solid_y`].
pub fn top_solid_y_by<V, F>(column: &[V], y_min: i32, is_solid: F) -> Option<i32>
where
    F: Fn(&V) -> bool,
{
    // Walk from the top index down; first solid wins. `enumerate().rev()` keeps
    // the index, so the world Y is `y_min + idx` with no separate counter.
    for (idx, voxel) in column.iter().enumerate().rev() {
        if is_solid(voxel) {
            // idx < column.len() <= i32::MAX in any real chunk column; guard
            // anyway so the core can never panic on a pathological slice.
            let dy = i32::try_from(idx).unwrap_or(i32::MAX);
            return Some(y_min.saturating_add(dy));
        }
    }
    None
}

/// Alloc-free top-solid scan **straight out of chunk storage** — no column copy.
///
/// Chunk blocks are a flat `Vec<BlockId>` indexed `x + z*cs + y*cs*cs`
/// (`Chunk::block_index`). Gathering a `&[V]` column first would allocate per
/// query on the placement hot path; this variant strides the flat buffer in
/// place instead. This is the form the perf note in the handoff recommends the
/// call-sites use (scan once per chunk, cache the result).
///
/// `blocks` is the flat chunk buffer, `(x, z)` the chunk-local column,
/// `chunk_size` the x/z edge length (the index stride), and `y_base` the world
/// Y of local y = 0. The number of Y layers is **derived from the buffer
/// length** (`blocks.len() / chunk_size²`) rather than assumed equal to
/// `chunk_size` — so the scan covers exactly the data present and works for
/// cubic chunks (`len = cs³` → depth `cs`) and any other rectangular extent.
/// Returns the **world** Y of the top solid block, or `None`.
///
/// Out-of-range `x`/`z`/`chunk_size`, or a buffer too short for the column,
/// yield `None` rather than indexing out of bounds — pure and panic-free.
pub fn top_solid_y_in_chunk<V, F>(
    blocks: &[V],
    x: i32,
    z: i32,
    chunk_size: i32,
    y_base: i32,
    is_solid: F,
) -> Option<i32>
where
    F: Fn(&V) -> bool,
{
    if chunk_size <= 0 || x < 0 || z < 0 || x >= chunk_size || z >= chunk_size {
        return None;
    }
    let cs = chunk_size as usize;
    let (x, z) = (x as usize, z as usize);
    let plane = cs.checked_mul(cs)?; // cs*cs, the per-Y stride
    let base = x + z.checked_mul(cs)?; // index of local y = 0 for this column
    let depth = blocks.len() / plane; // Y layers actually present in the buffer

    // Top-down over local y. `block_index = base + y*plane`.
    for ly in (0..depth).rev() {
        let idx = base.checked_add(ly.checked_mul(plane)?)?;
        match blocks.get(idx) {
            Some(v) if is_solid(v) => {
                let dy = i32::try_from(ly).unwrap_or(i32::MAX);
                return Some(y_base.saturating_add(dy));
            }
            Some(_) => continue,
            None => return None, // buffer shorter than the column → give up cleanly
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── A minimal stand-in for the engine's `BlockId`, self-contained so the
    //    module gates with a dependency-free `rustc --test`. Only the property
    //    the scan cares about (solidity) is modelled; the solid set mirrors
    //    `BlockId::is_solid()` (air / water / lava / thin plants are non-solid).
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum TestBlock {
        Air,
        Water,
        Plant, // flower / tall grass / fern — thin decoration, non-solid
        Dirt,
        Stone,
        Grass, // the solid grass *block* top (distinct from the Plant decoration)
    }

    impl Voxel for TestBlock {
        fn is_solid(&self) -> bool {
            !matches!(self, TestBlock::Air | TestBlock::Water | TestBlock::Plant)
        }
    }

    use TestBlock::*;

    #[test]
    fn finds_first_solid_from_the_top() {
        // y_min = 60. Column bottom→top: Stone,Stone,Dirt,Grass,Air,Air.
        // Top solid is the Grass block at index 3 → world Y 63.
        let col = [Stone, Stone, Dirt, Grass, Air, Air];
        assert_eq!(top_solid_y(&col, 60), Some(63));
    }

    #[test]
    fn all_air_is_none() {
        let col = [Air, Air, Air, Air];
        assert_eq!(top_solid_y(&col, 0), None);
        // Empty column is also cleanly None (no panic, no underflow).
        let empty: [TestBlock; 0] = [];
        assert_eq!(top_solid_y(&empty, 64), None);
    }

    #[test]
    fn solid_at_the_very_top_returns_top_y() {
        // Fully solid column: top index 4 at y_min = 100 → 104.
        let col = [Stone, Stone, Stone, Stone, Stone];
        assert_eq!(top_solid_y(&col, 100), Some(104));
        // Single solid block: index 0 → y_min itself.
        assert_eq!(top_solid_y(&[Stone], 64), Some(64));
    }

    #[test]
    fn fluids_and_plants_above_ground_are_skipped() {
        // The core of the float bug class: a grassy bank under shallow water,
        // with a flower poking up. Placement must land on the Dirt top — NOT
        // on the water surface and NOT on the floating flower.
        //  idx: 0 Stone, 1 Dirt, 2 Dirt(top), 3 Water, 4 Water, 5 Plant, 6 Air
        let col = [Stone, Dirt, Dirt, Water, Water, Plant, Air];
        // y_min = 60 → Dirt top at idx 2 → world Y 62. Water (idx 3-4) and the
        // Plant (idx 5) are non-solid and correctly skipped.
        assert_eq!(top_solid_y(&col, 60), Some(62));
    }

    #[test]
    fn divergence_harness_follows_voxels_not_the_fp64_prediction() {
        // §4a reproduced as a unit test. The GPU FP32 generator actually placed
        // the top solid block at world Y = 67 for this column. The CPU FP64
        // noise *prediction* (what `surface_y_at` returns today) says 70 —
        // a few blocks of FP divergence. Flora/mob placement using the
        // prediction floats 3 blocks above the real ground.
        //
        // Column for y_min = 64 (SEA_LEVEL), len 12 → covers world Y 64..=75.
        // True top solid (FP32 voxels): world Y 67 = index 3.
        let mut col = [Air; 12];
        col[0] = Stone; // Y 64
        col[1] = Stone; // Y 65
        col[2] = Dirt; //  Y 66
        col[3] = Grass; // Y 67  <- the ACTUAL generated surface
        // indices 4..=11 (Y 68..=75) are Air — empty sky above the real ground.

        let fp64_prediction: i32 = 70; // what the buggy path would hand placement
        let actual = top_solid_y(&col, 64).expect("column has solid ground");

        assert_eq!(actual, 67, "scan must report the real FP32 voxel top");
        assert_ne!(
            actual, fp64_prediction,
            "the whole point: the voxel read must DIVERGE from the FP64 prediction"
        );
        // And it floats *below* the prediction here (prediction too high → things
        // hover). The sign matters for the symptom; assert it explicitly.
        assert!(
            actual < fp64_prediction,
            "prediction sat above the real surface → that is why placement floats"
        );
    }

    #[test]
    fn strided_chunk_scan_matches_contiguous_scan() {
        // Build a tiny 2x2x4 "chunk" (chunk_size = 2 in x/z, 4 layers in y)
        // in the engine's flat layout: index = x + z*cs + y*cs*cs.
        let cs = 2usize;
        let plane = cs * cs;
        let mut blocks = vec![Air; plane * 4]; // y 0..4
        // Fill column (x=1, z=0): y0 Stone, y1 Dirt, y2 Grass, y3 Air.
        let base = 1 + 0 * cs; // x=1, z=0
        blocks[base + 0 * plane] = Stone;
        blocks[base + 1 * plane] = Dirt;
        blocks[base + 2 * plane] = Grass;
        // (other columns left Air)

        // Strided in-place scan, y_base = 64 → top solid at y2 → world Y 66.
        let strided = top_solid_y_in_chunk(&blocks, 1, 0, cs as i32, 64, |v| v.is_solid());
        assert_eq!(strided, Some(66));

        // Same answer as gathering the column and using the contiguous path.
        let gathered: Vec<TestBlock> = (0..4).map(|y| blocks[base + y * plane]).collect();
        assert_eq!(top_solid_y(&gathered, 64), strided);

        // An all-air column (x=0,z=1) is cleanly None.
        assert_eq!(
            top_solid_y_in_chunk(&blocks, 0, 1, cs as i32, 64, |v| v.is_solid()),
            None
        );
    }

    #[test]
    fn strided_scan_rejects_bad_coords_without_panicking() {
        let cs = 2i32;
        let blocks = vec![Stone; (cs * cs * 4) as usize];
        // Out-of-range x/z, non-positive size, and a too-short buffer all → None.
        assert_eq!(top_solid_y_in_chunk(&blocks, 2, 0, cs, 0, |v| v.is_solid()), None);
        assert_eq!(top_solid_y_in_chunk(&blocks, -1, 0, cs, 0, |v| v.is_solid()), None);
        assert_eq!(top_solid_y_in_chunk(&blocks, 0, 0, 0, 0, |v| v.is_solid()), None);
        let short = vec![Stone; 2]; // far shorter than cs*cs*depth
        assert_eq!(top_solid_y_in_chunk(&short, 0, 0, cs, 0, |v| v.is_solid()), None);
    }

    #[test]
    fn air_pocket_returns_the_upper_lip_not_the_cave_floor() {
        // A cave: solid roof, air gap, solid floor. Top-down placement must land
        // on the ROOF (the first solid from the sky), not tunnel down to the
        // floor — otherwise mobs spawn inside the hill.
        //  idx: 0 Stone(floor) 1 Air 2 Air 3 Stone(roof) 4 Air
        let col = [Stone, Air, Air, Stone, Air];
        assert_eq!(top_solid_y(&col, 0), Some(3));
    }

    #[test]
    fn predicate_and_trait_forms_agree_and_are_deterministic() {
        let col = [Stone, Dirt, Water, Plant, Air, Grass, Air];
        let a = top_solid_y(&col, 50);
        let b = top_solid_y_by(&col, 50, |v: &TestBlock| v.is_solid());
        assert_eq!(a, b);
        // Pure → identical output on repeat.
        assert_eq!(a, top_solid_y(&col, 50));
        // A narrower predicate (treat Grass *block* as non-rest, say) changes the
        // answer deterministically, proving the predicate is honoured.
        let strict = top_solid_y_by(&col, 50, |v: &TestBlock| {
            v.is_solid() && *v != Grass
        });
        assert_eq!(strict, Some(51)); // falls through Grass(idx5) to Dirt(idx1)
    }

    #[test]
    fn y_min_offset_is_applied_to_the_world_y() {
        // Same column, different y_min → the returned Y tracks the offset.
        let col = [Air, Stone, Air]; // solid at index 1
        assert_eq!(top_solid_y(&col, 0), Some(1));
        assert_eq!(top_solid_y(&col, 64), Some(65));
        assert_eq!(top_solid_y(&col, -10), Some(-9));
    }
}
