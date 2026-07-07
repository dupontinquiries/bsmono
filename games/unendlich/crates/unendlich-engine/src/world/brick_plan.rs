//! `world::brick_plan` — turn a bending shape's world-space bounds into the exact
//! set of mesh bricks it dirties, across chunk boundaries.
//!
//! A massive shape (a radius-8 dome is ~1340 block edits) shouldn't re-bake whole
//! chunks. `mesh_brick` already re-meshes at 8³ brick granularity; this planner
//! is the front half of that: given the shape's voxel AABB it computes which
//! `(chunk, brick)` pairs the edit touches, so the caller marks *only* those
//! bricks dirty in one pass instead of routing 1340 individual voxels through the
//! per-edit path. Feed the result straight into `mesh_brick::DirtyBricks` /
//! `select_bricks` and the big shape becomes a handful of sub-millisecond brick
//! re-meshes spread across frames.
//!
//! Meshing a brick reads its neighbours' edge voxels for face culling, so an edit
//! on a brick boundary also dirties the adjacent brick. Pass an AABB already
//! expanded by one voxel ([`expand1`]) — as `block_edit` emits — and boundary
//! neighbours fall out automatically.
//!
//! Pure/integer/deterministic; reuses the real brick constants so it can't drift.

use crate::constants::CHUNK_SIZE;
use crate::world::mesh_brick::{brick_index_of, BRICKS_PER_AXIS, BRICK_SIZE};

/// One dirtied brick: the chunk it lives in and its 0..64 index within that chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DirtyBrick {
    pub chunk: [i32; 3],
    pub brick: usize,
}

/// Expand an inclusive voxel AABB by one voxel on every side — the face-culling
/// skirt, so edits flush against a brick boundary also dirty the neighbour brick.
pub fn expand1(min: [i32; 3], max: [i32; 3]) -> ([i32; 3], [i32; 3]) {
    ([min[0] - 1, min[1] - 1, min[2] - 1], [max[0] + 1, max[1] + 1, max[2] + 1])
}

/// The world brick coordinate a voxel falls in (floor division, negatives-safe).
fn world_brick(v: i32) -> i32 {
    v.div_euclid(BRICK_SIZE)
}

/// Split a world brick coordinate into (chunk coordinate, in-chunk brick coord).
fn chunk_and_local(wb: i32) -> (i32, i32) {
    (wb.div_euclid(BRICKS_PER_AXIS), wb.rem_euclid(BRICKS_PER_AXIS))
}

/// The set of `(chunk, brick)` pairs an **inclusive** voxel AABB spans. Iterates
/// at brick granularity (not per voxel), so cost is proportional to the number of
/// bricks touched, not the block count. Deduplicated and sorted (deterministic).
pub fn dirty_bricks(min: [i32; 3], max: [i32; 3]) -> Vec<DirtyBrick> {
    let mut out = Vec::new();
    if min[0] > max[0] || min[1] > max[1] || min[2] > max[2] {
        return out;
    }
    let wb_min = [world_brick(min[0]), world_brick(min[1]), world_brick(min[2])];
    let wb_max = [world_brick(max[0]), world_brick(max[1]), world_brick(max[2])];

    for wbz in wb_min[2]..=wb_max[2] {
        let (cz, lz) = chunk_and_local(wbz);
        for wby in wb_min[1]..=wb_max[1] {
            let (cy, ly) = chunk_and_local(wby);
            for wbx in wb_min[0]..=wb_max[0] {
                let (cx, lx) = chunk_and_local(wbx);
                // brick index from the in-chunk brick coordinate (voxel at the
                // brick's origin resolves to that brick)
                let brick = brick_index_of([lx * BRICK_SIZE, ly * BRICK_SIZE, lz * BRICK_SIZE]);
                out.push(DirtyBrick { chunk: [cx, cy, cz], brick });
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Same coverage, grouped into per-chunk `u64` brick bitsets — the shape
/// `mesh_brick::DirtyBricks` stores, so the caller can OR these straight in.
/// Returned sorted by chunk coordinate.
pub fn dirty_brick_bitsets(min: [i32; 3], max: [i32; 3]) -> Vec<([i32; 3], u64)> {
    let bricks = dirty_bricks(min, max);
    let mut out: Vec<([i32; 3], u64)> = Vec::new();
    for db in bricks {
        match out.last_mut() {
            Some((chunk, mask)) if *chunk == db.chunk => *mask |= 1u64 << db.brick,
            _ => out.push((db.chunk, 1u64 << db.brick)),
        }
    }
    out
}

/// How many bricks a shape's (inclusive) AABB touches — a cheap pre-check for
/// deciding whether to remesh eagerly or coarse-to-fine. Counts without
/// allocating the full set.
pub fn brick_count(min: [i32; 3], max: [i32; 3]) -> usize {
    if min[0] > max[0] || min[1] > max[1] || min[2] > max[2] {
        return 0;
    }
    let span = |lo: i32, hi: i32| (world_brick(hi) - world_brick(lo) + 1) as usize;
    span(min[0], max[0]) * span(min[1], max[1]) * span(min[2], max[2])
}

/// Distinct chunks a shape touches (for chunk-level bookkeeping).
pub fn touched_chunks(min: [i32; 3], max: [i32; 3]) -> Vec<[i32; 3]> {
    let mut cs: Vec<[i32; 3]> = dirty_brick_bitsets(min, max).into_iter().map(|(c, _)| c).collect();
    cs.sort();
    cs.dedup();
    cs
}

/// Batch a whole shape in one pass: from the list of voxels a shape actually
/// **changed**, produce the per-chunk dirty-brick bitsets — dirtying only bricks
/// that contain a changed voxel (or a face-neighbour of one). This is the precise
/// alternative to [`dirty_brick_bitsets`] over the union AABB: a *hollow* shape
/// (dome shell, ring, cage) leaves its interior bricks clean instead of re-baking
/// them. Each cell contributes its ±1 skirt so shared brick faces are covered.
///
/// The caller applies its edits (via `block_edit`), collects the world positions
/// that actually changed, and hands them here once — then ORs the result into
/// `mesh_brick::DirtyBricks`. Returned sorted by chunk.
pub fn dirty_bitsets_for_cells(changed_cells: &[[i32; 3]]) -> Vec<([i32; 3], u64)> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<[i32; 3], u64> = BTreeMap::new();
    for &c in changed_cells {
        let (mn, mx) = expand1(c, c);
        for db in dirty_bricks(mn, mx) {
            *map.entry(db.chunk).or_insert(0) |= 1u64 << db.brick;
        }
    }
    map.into_iter().collect()
}

/// Total dirty bricks across all chunks for a changed-cell set (cheap summary /
/// budget pre-check).
pub fn dirty_brick_total(changed_cells: &[[i32; 3]]) -> usize {
    dirty_bitsets_for_cells(changed_cells).iter().map(|(_, m)| m.count_ones() as usize).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_voxel_maps_to_one_brick_in_chunk_zero() {
        // voxel (3,3,3) → brick 0 of chunk (0,0,0)
        let d = dirty_bricks([3, 3, 3], [3, 3, 3]);
        assert_eq!(d, vec![DirtyBrick { chunk: [0, 0, 0], brick: 0 }]);
    }

    #[test]
    fn box_spanning_two_bricks_in_x() {
        // x from 7 to 8 straddles the 8-voxel brick boundary → bricks 0 and 1
        let d = dirty_bricks([7, 0, 0], [8, 0, 0]);
        assert_eq!(d.len(), 2);
        assert!(d.iter().all(|b| b.chunk == [0, 0, 0]));
        let idxs: Vec<usize> = d.iter().map(|b| b.brick).collect();
        assert!(idxs.contains(&brick_index_of([0, 0, 0])) && idxs.contains(&brick_index_of([8, 0, 0])));
    }

    #[test]
    fn box_crossing_a_chunk_boundary() {
        // CHUNK_SIZE = 32; x from 30 to 34 crosses into chunk 1.
        let d = dirty_bricks([30, 0, 0], [34, 0, 0]);
        let chunks: std::collections::BTreeSet<[i32; 3]> = d.iter().map(|b| b.chunk).collect();
        assert!(chunks.contains(&[0, 0, 0]) && chunks.contains(&[1, 0, 0]), "spans both chunks");
        // the chunk-0 side is the last brick (x-brick 3), the chunk-1 side the first
        assert!(d.iter().any(|b| b.chunk == [0, 0, 0] && b.brick == brick_index_of([24, 0, 0])));
        assert!(d.iter().any(|b| b.chunk == [1, 0, 0] && b.brick == brick_index_of([0, 0, 0])));
    }

    #[test]
    fn negative_coordinates_are_handled() {
        // voxel (-1,-1,-1) is in chunk (-1,-1,-1), last brick on each axis
        let d = dirty_bricks([-1, -1, -1], [-1, -1, -1]);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].chunk, [-1, -1, -1]);
        assert_eq!(d[0].brick, brick_index_of([24, 24, 24]), "brick coord (3,3,3)");
    }

    #[test]
    fn count_matches_the_enumerated_set() {
        let (mn, mx) = ([5, 5, 5], [40, 20, 9]);
        assert_eq!(brick_count(mn, mx), dirty_bricks(mn, mx).len());
    }

    #[test]
    fn radius8_dome_is_a_handful_of_bricks_not_whole_chunks() {
        // a radius-8 dome centred at (100,64,100): a 17³ voxel box.
        let (mn, mx) = ([92, 64, 92], [108, 80, 108]);
        let (emn, emx) = expand1(mn, mx); // include the skirt
        let n = brick_count(emn, emx);
        // 19-voxel span / 8 ⇒ at most 3–4 bricks per axis ⇒ ≤ 64 bricks total,
        // versus re-baking the (up to 8) 32³ chunks it overlaps.
        assert!(n <= 64, "touches {n} bricks, still bounded well under a chunk-rebake blizzard");
        assert!(n >= 9, "but more than a trivial handful for a big shape");
    }

    #[test]
    fn bitsets_group_by_chunk_and_or_the_bits() {
        let (mn, mx) = ([30, 0, 0], [34, 0, 0]); // crosses chunk boundary
        let sets = dirty_brick_bitsets(mn, mx);
        // one entry per touched chunk, bits correspond to the dirty bricks
        let total_bits: u32 = sets.iter().map(|(_, m)| m.count_ones()).sum();
        assert_eq!(total_bits as usize, dirty_bricks(mn, mx).len());
        assert_eq!(sets.len(), touched_chunks(mn, mx).len());
        // chunk-0 mask has the x=3 brick set
        let (_, m0) = sets.iter().find(|(c, _)| *c == [0, 0, 0]).unwrap();
        assert!(m0 & (1u64 << brick_index_of([24, 0, 0])) != 0);
    }

    #[test]
    fn skirt_expansion_pulls_in_the_neighbour_brick() {
        // a voxel exactly on a brick boundary (x=8) alone touches brick 1;
        // with the ±1 skirt it also dirties brick 0 (the shared face).
        let bare = dirty_bricks([8, 0, 0], [8, 0, 0]);
        assert_eq!(bare.len(), 1);
        let (mn, mx) = expand1([8, 0, 0], [8, 0, 0]);
        let with_skirt = dirty_bricks(mn, mx);
        assert!(with_skirt.len() >= 2, "skirt reaches the neighbour brick(s)");
    }

    #[test]
    fn empty_or_inverted_box_is_no_bricks() {
        assert!(dirty_bricks([5, 5, 5], [4, 5, 5]).is_empty());
        assert_eq!(brick_count([5, 5, 5], [4, 5, 5]), 0);
    }

    #[test]
    fn hollow_shell_leaves_interior_bricks_clean() {
        // Shell of a 24³ box (only the 6 faces). Compared against the SOLID box
        // (same bounds, skirted) the shell must dirty strictly fewer bricks —
        // it skips fully-interior bricks that never contained a changed voxel.
        // The saving scales with interior volume; a curved dome saves far more.
        let mut shell = Vec::new();
        for x in 0..24 {
            for y in 0..24 {
                for z in 0..24 {
                    if x == 0 || x == 23 || y == 0 || y == 23 || z == 0 || z == 23 {
                        shell.push([x, y, z]);
                    }
                }
            }
        }
        // solid box, skirted, via the AABB planner
        let (emn, emx) = expand1([0, 0, 0], [23, 23, 23]);
        let solid_total: usize =
            dirty_brick_bitsets(emn, emx).iter().map(|(_, m)| m.count_ones() as usize).sum();
        let shell_total = dirty_brick_total(&shell);
        assert!(shell_total < solid_total, "shell {shell_total} < solid {solid_total}");
        // the centre brick (voxels 8..16, no changed cell) stays clean
        let sets = dirty_bitsets_for_cells(&shell);
        let (_, m) = sets.iter().find(|(c, _)| *c == [0, 0, 0]).unwrap();
        assert!(m & (1u64 << brick_index_of([8, 8, 8])) == 0, "hollow interior brick stays clean");
    }

    #[test]
    fn batch_of_a_full_box_matches_the_aabb_planner() {
        // for a SOLID fill, the cell-set batch and the AABB planner agree
        let mut solid = Vec::new();
        for x in 5..11 {
            for y in 5..11 {
                for z in 5..11 {
                    solid.push([x, y, z]);
                }
            }
        }
        let batch = dirty_bitsets_for_cells(&solid);
        let (emn, emx) = expand1([5, 5, 5], [10, 10, 10]);
        let aabb = dirty_brick_bitsets(emn, emx);
        assert_eq!(batch, aabb, "solid fill: precise batch == expanded-AABB");
    }
}
