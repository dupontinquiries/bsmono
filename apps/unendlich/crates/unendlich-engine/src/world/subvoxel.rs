//! Sub-voxel detail for select block types (path C of the architecture fork).
//!
//! Most blocks in the world are single-cell — a block of stone is just stone,
//! 1×1×1, no internal structure. But some block types want **interactable
//! detail at finer resolution**: grass tufts you can mow blade by blade,
//! ferns whose fronds can burn separately, flowers with petals you can pick.
//!
//! For those host blocks, we define a **sub-voxel grid**: an N×N×N pattern
//! of mini-cells inside the block's volume, each mini-cell carrying its own
//! state. Resolution is `SUBVOXEL_RESOLUTION` (currently 8) — so each mini-
//! cell is `1/8` of a block. That's small enough for stylized voxel-art
//! plants without exploding memory.
//!
//! ## Storage model
//!
//! Sub-voxel grids are *sparse*. Most cells in a host block are empty
//! (the negative space where the plant *isn't*). We store them as
//! `Vec<MiniCell>` rather than a dense `[[[u8; 8]; 8]; 8]` because that's
//! how plant geometry actually looks — a few dozen cells out of 512.
//!
//! ## Procedural vs explicit
//!
//! Like top-level blocks, sub-voxel grids are *born procedural*. A grass
//! block at world (x, y, z) gets a tuft sprite picked deterministically
//! from a hash of those coords. No storage cost — the recipe regenerates
//! it on demand.
//!
//! When the player edits a sub-cell (mowing grass, picking flowers, fire),
//! the grid is **promoted** to explicit storage: the chunk records the
//! delta, persistence saves it, multiplayer replicates it. Same pattern
//! we already use for whole-block edits, just one level finer.
//!
//! This turn implements rendering only. Edits, raycast hits, and physics
//! interactions land in subsequent turns; the data type is shaped so they
//! can hook in without rework.

use crate::world::blocks::BlockId;

/// Number of sub-cells per block axis. 8 means each mini-cell is 1/8 block.
/// Keep small — total cells per host = N³, and we touch lots of hosts.
pub const SUBVOXEL_RESOLUTION: usize = 8;

/// Width of a single sub-cell in block-units.
pub const SUBVOXEL_SIZE: f32 = 1.0 / SUBVOXEL_RESOLUTION as f32;

/// One filled mini-cell within a host block's sub-grid.
///
/// Position is in sub-cell coordinates [0, SUBVOXEL_RESOLUTION). Color is
/// what the renderer paints. We carry color directly (rather than via
/// BlockId) because plant-cell colors vary continuously with hash — every
/// blade of grass is slightly different green, and centralizing that in
/// BlockId would force a lookup table of every shade.
///
/// Future: if sub-cells need rich behavior (fire state, moisture, age),
/// extend with a `block_id` or `tags` field. For now, color suffices.
#[derive(Debug, Clone, Copy)]
pub struct MiniCell {
    pub sub_x: u8,
    pub sub_y: u8,
    pub sub_z: u8,
    pub color: [f32; 3],
}

/// Sparse sub-voxel grid for one host block. Position coords are local
/// to the block; world coords are `host_block_world + sub_coord * SIZE`.
#[derive(Debug, Clone, Default)]
pub struct SubVoxelGrid {
    pub cells: Vec<MiniCell>,
}

impl SubVoxelGrid {
    pub fn empty() -> Self { Self { cells: Vec::new() } }

    pub fn cell_count(&self) -> usize { self.cells.len() }
}

/// Hash to (`f32` in [0,1), `u32` for picking variants). Same shape as
/// `top_corner_offset` etc. but with a different salt so plants don't
/// correlate with terrain bumpiness.
///
/// Public so the RC42 template-stamp path can pick a variant using the
/// SAME hash the procedural path used to drive `procedural_subgrid`.
/// That keeps visual variation deterministic across the two paths.
pub fn plant_hash(world_x: i32, world_y: i32, world_z: i32) -> (f32, u32) {
    let mut h: u32 = (world_x as u32).wrapping_mul(0x6c8e_9cf5)
        ^ (world_y as u32).wrapping_mul(0xa129_b2d3)
        ^ (world_z as u32).wrapping_mul(0xf2d9_c5d1);
    h ^= h >> 16;
    h = h.wrapping_mul(0x21f0_aaad);
    h ^= h >> 15;
    h = h.wrapping_mul(0x735a_2d97);
    h ^= h >> 15;
    let unit = (h & 0xFFFF) as f32 / 65535.0;
    (unit, h)
}

/// Generate the sub-voxel grid for a host block at the given world coords.
///
/// Returns `None` if this block doesn't host sub-voxels (e.g. stone).
/// Returns `Some(SubVoxelGrid)` for plant blocks; the grid is procedural,
/// derived from `(host_id, world position)` so it's stable across reloads.
pub fn procedural_subgrid(host_id: BlockId, world_x: i32, world_y: i32, world_z: i32) -> Option<SubVoxelGrid> {
    if !host_id.has_subvoxels() { return None; }

    let (rng01, rng_int) = plant_hash(world_x, world_y, world_z);

    // Pick a recipe variant. Different host block types use different
    // recipe pools.
    Some(match host_id {
        BlockId::Grass => {
            // Use the blueprint format for grass tufts: blades have
            // per-blade curvature so they look natural rather than
            // rigidly straight columns. Blade count = 3..7 by hash for
            // variety. Color is the green palette tinted by shade.
            let n_blades = 3 + ((rng_int >> 6) & 0x3) as usize; // 3..6
            let color = recipes::green(rng01);
            crate::world::plant_blueprint::grass_tuft_blueprint(
                n_blades, color, rng_int
            ).to_subgrid()
        }
        BlockId::Fern => recipes::fern(rng01),
        BlockId::Flower => recipes::flower(rng01, rng_int),
        BlockId::TallGrass => {
            // Same as Grass but more blades and taller blades.
            let color = recipes::green(rng01 * 0.8);
            crate::world::plant_blueprint::grass_tuft_blueprint(
                7, color, rng_int.wrapping_mul(2)
            ).to_subgrid()
        }
        BlockId::OakLeaves   => recipes::leaves(rng01, rng_int, /*birch=*/false),
        BlockId::BirchLeaves => recipes::leaves(rng01, rng_int, /*birch=*/true),
        // r513: crops render as small leafy plants instead of cubes.
        BlockId::Carrot  => recipes::crop(rng01, rng_int, 0),
        BlockId::Potato  => recipes::crop(rng01, rng_int, 1),
        BlockId::Cabbage => recipes::crop(rng01, rng_int, 2),
        // r520: a young seedling — a couple of tiny green shoots.
        BlockId::Sprout  => recipes::sprout(rng01),
        // Default for any other host: empty grid (no decoration).
        _ => SubVoxelGrid::empty(),
    })
}

/// Hand-designed plant recipes. Each builds a small SubVoxelGrid by hand.
///
/// Design notes:
///   - Sub-cell coords are 0..8 (8³ grid).
///   - Y=0 is the bottom of the host block (where the plant attaches to
///     the ground below). Y=7 is the top.
///   - Plants typically only fill 4-6 vertical layers; the top of the
///     host block is reserved as breathing room.
///   - Cells are added in any order; meshing handles the rest.
mod recipes {
    use super::*;

    /// Helper: add a single mini-cell.
    fn cell(grid: &mut SubVoxelGrid, x: u8, y: u8, z: u8, color: [f32; 3]) {
        grid.cells.push(MiniCell { sub_x: x, sub_y: y, sub_z: z, color });
    }

    /// Color helper: shade a base green by a hash, gives per-tuft variation.
    pub(super) fn green(shade: f32) -> [f32; 3] {
        // Base "grass" green, varied by `shade` (0..1) toward yellow at
        // high values, toward dark at low.
        let r = 0.18 + 0.18 * shade;
        let g = 0.45 + 0.20 * shade;
        let b = 0.10 + 0.05 * shade;
        [r, g, b]
    }

    /// Grass tuft. Three variants for visual variety — all small clumps of
    /// blades attached to the bottom. Variant 0 = sparse 3 blades, 1 =
    /// medium 5 blades, 2 = dense/tall.
    ///
    /// Currently superseded by procedural generation in
    /// `procedural_subgrid`; kept here as the canonical hand-authored
    /// reference for grass-tuft shapes.
    #[allow(dead_code)]
    pub fn grass_tuft(variant: usize, shade: f32) -> SubVoxelGrid {
        let mut grid = SubVoxelGrid::empty();
        let c = green(shade);
        match variant {
            0 => {
                // 3 short blades scattered across the cell base.
                for &(x, z, h) in &[(2u8, 3u8, 3u8), (4, 2, 2), (5, 5, 4)] {
                    for y in 0..h { cell(&mut grid, x, y, z, c); }
                }
            }
            1 => {
                // 5 medium blades.
                for &(x, z, h) in &[(2u8, 2u8, 4u8), (3, 5, 3), (4, 3, 5),
                                     (5, 4, 3), (6, 6, 4)] {
                    for y in 0..h { cell(&mut grid, x, y, z, c); }
                }
            }
            _ => {
                // Tall dense variant. 7 blades, taller.
                for &(x, z, h) in &[(2u8, 2u8, 5u8), (3, 4, 6), (3, 6, 4),
                                     (4, 3, 6), (5, 5, 5), (5, 2, 4),
                                     (6, 4, 5)] {
                    for y in 0..h { cell(&mut grid, x, y, z, c); }
                }
            }
        }
        grid
    }

    /// Fern: taller, more vertical structure. A central "stem" with side
    /// fronds. Looks distinct from grass at a glance.
    pub fn fern(shade: f32) -> SubVoxelGrid {
        let mut grid = SubVoxelGrid::empty();
        let stem = green(shade * 0.5); // darker for stem
        let frond = green(shade * 0.7 + 0.3); // brighter for fronds

        // Central stem, 6 cells tall.
        for y in 0..6 { cell(&mut grid, 4, y, 4, stem); }
        // Fronds in pairs at heights 1, 3, 5.
        for &y in &[1u8, 3, 5] {
            cell(&mut grid, 3, y, 4, frond);
            cell(&mut grid, 5, y, 4, frond);
            cell(&mut grid, 4, y, 3, frond);
            cell(&mut grid, 4, y, 5, frond);
        }
        grid
    }

    /// Flower: green stem with a colored top. Color picked by hash from a
    /// small palette so flowers don't all look the same.
    pub fn flower(shade: f32, rng_int: u32) -> SubVoxelGrid {
        let mut grid = SubVoxelGrid::empty();
        let stem = green(shade * 0.4 + 0.1);

        // Stem: 4 cells tall, central.
        for y in 0..4 { cell(&mut grid, 4, y, 4, stem); }

        // Flower head: 2x2 pattern at top, color from palette.
        let palette = [
            [0.95_f32, 0.85, 0.20], // yellow
            [0.85, 0.30, 0.30],     // red
            [0.55, 0.30, 0.85],     // purple
            [0.95, 0.95, 0.95],     // white
            [0.95, 0.55, 0.20],     // orange
        ];
        let head = palette[(rng_int >> 8) as usize % palette.len()];
        for &(x, z) in &[(3u8, 3u8), (3, 4), (4, 3), (4, 4), (5, 4), (4, 5)] {
            cell(&mut grid, x, 4, z, head);
        }
        grid
    }

    /// r520: a young seedling — two or three short shoots poking up from the soil,
    /// small enough to read as "just sprouted" next to a full-grown crop.
    pub fn sprout(shade: f32) -> SubVoxelGrid {
        let mut grid = SubVoxelGrid::empty();
        let young = green(shade * 0.5 + 0.35);
        cell(&mut grid, 4, 0, 4, young);
        cell(&mut grid, 4, 1, 4, young);
        cell(&mut grid, 3, 0, 4, young);
        cell(&mut grid, 5, 1, 5, young);
        grid
    }

    /// r513: Vegetable crops — small leafy plants so a tilled field reads as rows
    /// of greenery rather than a wall of colored cubes. `kind`: 0=Carrot (leafy fan
    /// with an orange crown peeking out), 1=Potato (low bushy mound), 2=Cabbage
    /// (a tight round green head close to the soil). The host block's base_color
    /// already tints the cube path; here we shape the silhouette.
    pub fn crop(shade: f32, _rng_int: u32, kind: u8) -> SubVoxelGrid {
        let mut grid = SubVoxelGrid::empty();
        let leaf = green(shade * 0.7 + 0.25);
        let stem = green(shade * 0.45 + 0.1);
        match kind {
            0 => {
                // Carrot: a central stalk with leaves fanning out, orange crown at base.
                let carrot = [0.90_f32, 0.49, 0.13];
                for y in 0..5 { cell(&mut grid, 4, y, 4, stem); }
                for &y in &[2u8, 3, 4] {
                    cell(&mut grid, 3, y, 4, leaf);
                    cell(&mut grid, 5, y, 4, leaf);
                    cell(&mut grid, 4, y, 3, leaf);
                    cell(&mut grid, 4, y, 5, leaf);
                }
                cell(&mut grid, 4, 0, 4, carrot);
                cell(&mut grid, 4, 1, 4, carrot);
            }
            2 => {
                // Cabbage: a dense round head hugging the ground.
                let head = green(shade * 0.6 + 0.3);
                for &(x, z) in &[
                    (3u8, 3u8), (3, 4), (3, 5), (4, 3), (4, 4),
                    (4, 5), (5, 3), (5, 4), (5, 5),
                ] {
                    cell(&mut grid, x, 0, z, head);
                    cell(&mut grid, x, 1, z, head);
                }
                cell(&mut grid, 4, 2, 4, head);
            }
            _ => {
                // Potato: a low bushy mound of leaves.
                for y in 0..3 { cell(&mut grid, 4, y, 4, stem); }
                for &(x, z) in &[(3u8, 4u8), (5, 4), (4, 3), (4, 5), (3, 3), (5, 5)] {
                    cell(&mut grid, x, 1, z, leaf);
                    cell(&mut grid, x, 2, z, leaf);
                }
                cell(&mut grid, 4, 3, 4, leaf);
            }
        }
        grid
    }

    /// Tree leaves: dense clusters of small mini-leaves filling most of
    /// the host cell, with deliberate gaps for the silhouette to read as
    /// "foliage" rather than "solid block." Variation per (world position)
    /// hash: different leaves have different shades and slightly different
    /// gap patterns.
    ///
    /// `birch=true` shifts the palette toward yellow-green; otherwise a
    /// deeper oak green.
    ///
    /// Future: leaf decay state will skip cells based on age, producing
    /// progressively sparser canopies and eventually full holes (sky
    /// shows through).
    pub fn leaves(shade: f32, rng_int: u32, birch: bool) -> SubVoxelGrid {
        let mut grid = SubVoxelGrid::empty();

        // Color palette tinted by birch flag and per-block shade.
        let leaf_color = if birch {
            // Birch leaves: yellow-green, lighter overall.
            [0.45 + 0.15 * shade, 0.62 + 0.15 * shade, 0.30 + 0.10 * shade]
        } else {
            // Oak leaves: deeper green, more saturation.
            [0.18 + 0.10 * shade, 0.50 + 0.20 * shade, 0.20 + 0.08 * shade]
        };

        // r496: STRINGY, DENSE foliage — rebuilt from clumps into thin vertical
        // strands (like grass blades) packed across the whole cell footprint, so
        // a leaf block reads as a dense mass of hanging fronds rather than a few
        // blobby clusters. Most (x,z) columns bear a 1-wide strand at a hashed
        // height; some hang downward (droop). Many more cells than the old clump
        // recipe → much denser canopy.
        let res = SUBVOXEL_RESOLUTION as i32; // 8
        let mut h = rng_int | 1;
        for x in 0..res {
            for z in 0..res {
                h = h.wrapping_mul(0x9E37_79B9).wrapping_add(0x1_2345);
                h ^= h >> 13;
                // r523: SPARSER canopy — ~45% of columns bear a strand (was ~64%, which read
                // as a near-solid mass even without the cube). Real gaps let sky show through so
                // it reads as foliage. Coverage = keep when (h&0xFF) <= 115.
                if (h & 0xFF) > 115 {
                    continue;
                }
                // A thin vertical strand: a top somewhere up the cell, hanging
                // down a few cells (droop). 1 sub-voxel wide → stringy.
                let len = 2 + ((h >> 8) & 0x3) as i32; // 2..5 tall
                let top = 2 + ((h >> 11) & 0x5) as i32; // 2..7
                let y0 = (top - len).max(0);
                let y1 = top.min(res - 1);
                // Per-strand colour jitter (greens vary most).
                let j = ((h >> 16) & 0xFF) as f32 / 255.0 - 0.5;
                let c = [
                    (leaf_color[0] + j * 0.10).clamp(0.0, 1.0),
                    (leaf_color[1] + j * 0.16).clamp(0.0, 1.0),
                    (leaf_color[2] + j * 0.10).clamp(0.0, 1.0),
                ];
                for y in y0..=y1 {
                    cell(&mut grid, x as u8, y as u8, z as u8, c);
                }
            }
        }
        grid
    }

    /// Place a small leaf-cluster shape at (cx, cy, cz). Variant chooses
    /// the pattern: roughly oval, vertical, horizontal, or single dot.
    /// (r496: superseded by the stringy-strand `leaves` recipe; kept for
    /// reference and possible reuse.)
    #[allow(dead_code)]
    fn place_leaf_clump(grid: &mut SubVoxelGrid,
        cx: i32, cy: i32, cz: i32, color: [f32; 3], variant: u32)
    {
        // Clump patterns: list of (dx, dy, dz) offsets from clump center.
        // Sized 3-5 cells each.
        let pattern: &[(i32, i32, i32)] = match variant {
            0 => &[(0, 0, 0), (1, 0, 0), (0, 0, 1), (1, 0, 1), (0, 1, 0)],  // L-cluster
            1 => &[(0, 0, 0), (0, 1, 0), (1, 1, 0), (0, 1, 1)],             // upright
            2 => &[(0, 0, 0), (1, 0, 0), (0, 0, 1), (1, 0, 1)],             // flat 2x2
            _ => &[(0, 0, 0), (1, 0, 0), (-1, 0, 0), (0, 0, 1)],            // cross
        };
        let max = SUBVOXEL_RESOLUTION as i32;
        for &(dx, dy, dz) in pattern {
            let x = cx + dx;
            let y = cy + dy;
            let z = cz + dz;
            if x < 0 || x >= max || y < 0 || y >= max || z < 0 || z >= max { continue; }
            cell(grid, x as u8, y as u8, z as u8, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_host_blocks_have_no_subgrid() {
        assert!(procedural_subgrid(BlockId::Stone, 0, 0, 0).is_none());
        assert!(procedural_subgrid(BlockId::Air, 0, 0, 0).is_none());
        assert!(procedural_subgrid(BlockId::Dirt, 0, 0, 0).is_none());
    }

    #[test]
    fn grass_block_has_nonempty_subgrid() {
        let g = procedural_subgrid(BlockId::Grass, 5, 10, -3).unwrap();
        assert!(g.cell_count() > 0, "grass tuft should have at least one cell");
        // Cells should be in-bounds.
        for c in &g.cells {
            assert!((c.sub_x as usize) < SUBVOXEL_RESOLUTION);
            assert!((c.sub_y as usize) < SUBVOXEL_RESOLUTION);
            assert!((c.sub_z as usize) < SUBVOXEL_RESOLUTION);
        }
    }

    #[test]
    fn subgrid_is_deterministic() {
        let a = procedural_subgrid(BlockId::Grass, 7, 64, -12).unwrap();
        let b = procedural_subgrid(BlockId::Grass, 7, 64, -12).unwrap();
        assert_eq!(a.cells.len(), b.cells.len());
        for (ca, cb) in a.cells.iter().zip(b.cells.iter()) {
            assert_eq!(ca.sub_x, cb.sub_x);
            assert_eq!(ca.sub_y, cb.sub_y);
            assert_eq!(ca.sub_z, cb.sub_z);
        }
    }

    #[test]
    fn flower_picks_from_color_palette() {
        // Across many positions, flower head colors should vary (pick from
        // multiple palette entries).
        let mut head_colors = std::collections::HashSet::new();
        for x in 0..50 {
            if let Some(g) = procedural_subgrid(BlockId::Flower, x, 0, 0) {
                // Top-most cells (y=4) are the head.
                for c in g.cells.iter().filter(|c| c.sub_y == 4) {
                    let key = (
                        (c.color[0] * 1000.0) as i32,
                        (c.color[1] * 1000.0) as i32,
                        (c.color[2] * 1000.0) as i32,
                    );
                    head_colors.insert(key);
                }
            }
        }
        assert!(head_colors.len() > 1, "expected multiple flower colors across positions");
    }

    #[test]
    fn oak_leaves_produces_subgrid() {
        let g = procedural_subgrid(BlockId::OakLeaves, 0, 0, 0).unwrap();
        // Distinct leaf clumps (~6-9 clumps × 3-5 cells each).
        assert!(g.cell_count() > 5, "expected leaf cells; got {}", g.cell_count());
        assert!(g.cell_count() < 80, "expected sparse fill; got {}", g.cell_count());
        // All cells in valid sub-voxel range [0..RES).
        for c in &g.cells {
            assert!((c.sub_x as usize) < SUBVOXEL_RESOLUTION);
            assert!((c.sub_y as usize) < SUBVOXEL_RESOLUTION);
            assert!((c.sub_z as usize) < SUBVOXEL_RESOLUTION);
        }
    }

    #[test]
    fn oak_and_birch_leaves_have_different_palettes() {
        let oak = procedural_subgrid(BlockId::OakLeaves, 0, 0, 0).unwrap();
        let birch = procedural_subgrid(BlockId::BirchLeaves, 0, 0, 0).unwrap();
        // Same hash inputs (position) → same gap pattern, but different
        // base color. Compare a sample cell from each.
        let oak_c = oak.cells[0].color;
        let birch_c = birch.cells[0].color;
        // Birch should be brighter (higher R + G).
        assert!(birch_c[0] > oak_c[0] + 0.05, "birch should be brighter");
    }
}
