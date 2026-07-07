//! Area mining — shapes + selection for area/vein tools.
//!
//! The engine-side port of the mod's `AreaPickaxeBase` and its `Shapes`
//! utility. An *area tool* breaks not only the targeted block but a whole
//! *shape* of cells around it, restricted to cells of the **same block
//! type** as the one struck — an area-vein miner. (Mine one stone in a
//! cliff and a 3×3×3 pocket of stone goes with it; the dirt and ore mixed
//! in are left alone.)
//!
//! This module is deliberately pure: offset geometry plus a selection
//! pass. It has no opinion on *how* blocks break, what they drop, or tool
//! durability beyond the cost helper — that's the interaction layer's job.
//! It pairs with [`crate::world::octree_raycast::WorldRaycastHit`] (the
//! picking primitive): the hit gives the origin cell and the block type to
//! match; this gives back the world cells the tool should clear, which the
//! caller then breaks.
//!
//! ## Faithfulness to the original
//!
//! - `Shapes.makeCube(n)` → [`cube_offsets`]. The original had a small
//!   vertical-offset quirk (its `y` range ran `-1..n-1` rather than
//!   centered); this normalizes to a true origin-centered cube, and rounds
//!   even sizes up to the next odd size so the shape stays centered.
//! - `Shapes.makeSphere(..)` (filled) → [`sphere_offsets`].
//! - "only break blocks of the same type you hit" → [`area_break_cells`].
//! - tool wear `1 + area/1.3` → [`area_durability_cost`].

use crate::world::blocks::BlockId;

/// Offsets forming an axis-aligned cube of edge `size`, centered on the
/// origin. `size = 3` → the 3×3×3 = 27-cell neighborhood including
/// `(0,0,0)`. Even sizes round up to the next odd size so the cube stays
/// centered; `size <= 1` yields just the origin.
pub fn cube_offsets(size: i32) -> Vec<(i32, i32, i32)> {
    let mut n = size.max(1);
    if n % 2 == 0 {
        n += 1;
    }
    let r = n / 2;
    let mut out = Vec::with_capacity((n * n * n) as usize);
    for y in -r..=r {
        for x in -r..=r {
            for z in -r..=r {
                out.push((x, y, z));
            }
        }
    }
    out
}

/// Offsets forming a filled sphere of `radius` cells, centered on the
/// origin: a cell is included when its center is within `radius + 0.5` of
/// the origin (matching the mod's `makeSphere` filled variant). `radius = 0`
/// yields just the origin. Used by radial tools (e.g. a charge/blast tool)
/// rather than the cube pickaxe.
pub fn sphere_offsets(radius: i32) -> Vec<(i32, i32, i32)> {
    let r = radius.max(0);
    let rad = r as f64 + 0.5;
    let rad_sq = rad * rad;
    let mut out = Vec::new();
    for y in -r..=r {
        for x in -r..=r {
            for z in -r..=r {
                let d = (x * x + y * y + z * z) as f64;
                if d <= rad_sq {
                    out.push((x, y, z));
                }
            }
        }
    }
    out
}

/// The cells an area tool actually breaks: for each offset in `shape`, the
/// world cell `origin + offset` is included **iff** the block there equals
/// `target` and isn't air. This is the mod's area-vein rule — only blocks of
/// the same type as the one struck are taken. `sample` resolves a world cell
/// to its block (e.g. an octree sampler or `ChunkManager`). Returns cells in
/// `shape` order; if `target` is air, returns empty (you don't vein-mine
/// air).
pub fn area_break_cells<S>(
    origin: (i32, i32, i32),
    shape: &[(i32, i32, i32)],
    target: BlockId,
    sample: S,
) -> Vec<(i32, i32, i32)>
where
    S: Fn((i32, i32, i32)) -> BlockId,
{
    if target.is_air() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for &(dx, dy, dz) in shape {
        let c = (origin.0 + dx, origin.1 + dy, origin.2 + dz);
        let b = sample(c);
        if b == target && !b.is_air() {
            out.push(c);
        }
    }
    out
}

/// Convenience: the cube-area break set for a raycast hit. `size` is the
/// cube edge (`3` → 3×3×3). Equivalent to
/// `area_break_cells(hit.cell, &cube_offsets(size), hit.block, sample)`.
pub fn cube_area_break<S>(
    hit: &crate::world::octree_raycast::WorldRaycastHit,
    size: i32,
    sample: S,
) -> Vec<(i32, i32, i32)>
where
    S: Fn((i32, i32, i32)) -> BlockId,
{
    area_break_cells(hit.cell, &cube_offsets(size), hit.block, sample)
}

/// Durability a single area break costs the tool, mirroring the mod's
/// `1 + area/1.3` scaling (so a larger shape wears the tool faster).
/// `shape_len` is the number of offsets in the shape, not the number that
/// matched — the swing costs the same whether the pocket was full or sparse.
pub fn area_durability_cost(shape_len: usize) -> u32 {
    (1.0 + shape_len as f64 / 1.3) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::octree_raycast::WorldRaycastHit;
    use crate::world::chunk::ChunkCoord;

    #[test]
    fn cube_sizes_and_centering() {
        assert_eq!(cube_offsets(1).len(), 1);
        assert_eq!(cube_offsets(1)[0], (0, 0, 0));
        assert_eq!(cube_offsets(3).len(), 27);
        assert_eq!(cube_offsets(5).len(), 125);
        // Even sizes round up to stay centered.
        assert_eq!(cube_offsets(2).len(), 27);
        let c3 = cube_offsets(3);
        assert!(c3.contains(&(0, 0, 0)), "cube must include the origin");
        assert!(c3.iter().all(|&(x, y, z)| (-1..=1).contains(&x)
            && (-1..=1).contains(&y) && (-1..=1).contains(&z)));
        // No duplicate offsets.
        let mut sorted = c3.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), c3.len());
    }

    #[test]
    fn sphere_sizes_and_corners() {
        assert_eq!(sphere_offsets(0).len(), 1);
        let s1 = sphere_offsets(1);
        assert_eq!(s1.len(), 19); // 27-cube minus the 8 corners (d²=3 > 2.25)
        assert!(s1.contains(&(1, 0, 0)));
        assert!(!s1.contains(&(1, 1, 1)), "corner is outside radius 1");
        assert_eq!(sphere_offsets(2).len(), 81);
    }

    #[test]
    fn area_break_selects_only_matching_nonair() {
        // Stone fills the 3×3×3 around the origin, except (1,0,0)=Dirt and
        // (0,1,0)=Air. A stone-targeted cube break takes 27 - 2 = 25 cells.
        let sample = |c: (i32, i32, i32)| -> BlockId {
            if c == (1, 0, 0) {
                BlockId::Dirt
            } else if c == (0, 1, 0) {
                BlockId::Air
            } else if (-1..=1).contains(&c.0)
                && (-1..=1).contains(&c.1)
                && (-1..=1).contains(&c.2)
            {
                BlockId::Stone
            } else {
                BlockId::Air
            }
        };
        let cells = area_break_cells((0, 0, 0), &cube_offsets(3), BlockId::Stone, sample);
        assert_eq!(cells.len(), 25);
        assert!(!cells.contains(&(1, 0, 0)), "different block type excluded");
        assert!(!cells.contains(&(0, 1, 0)), "air excluded");
    }

    #[test]
    fn area_break_of_air_target_is_empty() {
        let cells = area_break_cells((0, 0, 0), &cube_offsets(3), BlockId::Air, |_| BlockId::Air);
        assert!(cells.is_empty());
    }

    #[test]
    fn cube_area_break_uses_hit_cell_and_block() {
        let hit = WorldRaycastHit {
            t: 1.0,
            cell: (10, 20, 30),
            block: BlockId::Stone,
            normal: (-1, 0, 0),
            chunk: ChunkCoord::new(0, 0, 0),
        };
        // Everything stone → all 27 offsets selected, recentered on the hit.
        let cells = cube_area_break(&hit, 3, |_| BlockId::Stone);
        assert_eq!(cells.len(), 27);
        assert!(cells.contains(&(10, 20, 30)));
        assert!(cells.contains(&(11, 21, 31)));
        assert!(cells.contains(&(9, 19, 29)));
    }

    #[test]
    fn durability_cost_scales_with_shape() {
        assert_eq!(area_durability_cost(27), 21); // 1 + 27/1.3
        assert_eq!(area_durability_cost(1), 1);
        assert!(area_durability_cost(125) > area_durability_cost(27));
    }
}
