//! Mesh bricks — the unit of re-meshing, and the one place every dynamic system
//! funnels through.
//!
//! A 32³ chunk is subdivided into 8³ **bricks** (4×4×4 = 64 per chunk, so a
//! chunk's dirty set is a single `u64` bitset). The point: water flow, foliage
//! growth, fire spread, falling blocks, and player edits all call the *same*
//! [`DirtyBricks::mark`] when they touch a voxel, which dirties only the one
//! brick containing it. The mesher then re-meshes dirty bricks on a per-frame
//! budget ([`select_bricks`]) instead of re-baking a whole chunk — so a flood of
//! water cells or a field of growing plants becomes many sub-millisecond brick
//! re-meshes spread across frames, not one 100ms+ whole-chunk stall.
//!
//! Every dynamic system is "fixed the same way" because they share one dirtying
//! path at one granularity. Pairs with a merged/indirect draw of the brick
//! meshes (so fine-grained meshing doesn't explode the draw-call count).
//!
//! Pure, integer, deterministic.

use crate::constants::CHUNK_SIZE;

/// Brick edge in voxels.
pub const BRICK_SIZE: i32 = 8;
/// Bricks along one chunk axis (4 for a 32³ chunk with 8³ bricks).
pub const BRICKS_PER_AXIS: i32 = CHUNK_SIZE / BRICK_SIZE;
/// Bricks per chunk (64 → fits a u64 bitset).
pub const BRICKS_PER_CHUNK: usize = (BRICKS_PER_AXIS * BRICKS_PER_AXIS * BRICKS_PER_AXIS) as usize;

/// Brick index (0..BRICKS_PER_CHUNK) for a chunk-local voxel coordinate.
pub fn brick_index_of(local: [i32; 3]) -> usize {
    let bx = (local[0] / BRICK_SIZE).clamp(0, BRICKS_PER_AXIS - 1);
    let by = (local[1] / BRICK_SIZE).clamp(0, BRICKS_PER_AXIS - 1);
    let bz = (local[2] / BRICK_SIZE).clamp(0, BRICKS_PER_AXIS - 1);
    (bx + by * BRICKS_PER_AXIS + bz * BRICKS_PER_AXIS * BRICKS_PER_AXIS) as usize
}

/// The brick's grid coordinate (each component 0..BRICKS_PER_AXIS).
pub fn brick_coord(idx: usize) -> [i32; 3] {
    let i = idx as i32;
    [
        i % BRICKS_PER_AXIS,
        (i / BRICKS_PER_AXIS) % BRICKS_PER_AXIS,
        i / (BRICKS_PER_AXIS * BRICKS_PER_AXIS),
    ]
}

/// The chunk-local voxel bounds `[min, max)` covered by a brick.
pub fn brick_bounds(idx: usize) -> ([i32; 3], [i32; 3]) {
    let c = brick_coord(idx);
    let min = [c[0] * BRICK_SIZE, c[1] * BRICK_SIZE, c[2] * BRICK_SIZE];
    let max = [min[0] + BRICK_SIZE, min[1] + BRICK_SIZE, min[2] + BRICK_SIZE];
    (min, max)
}

/// In-chunk 6-neighbours of a brick (meshing a brick reads its neighbours' edge
/// voxels for face culling). Neighbours across the chunk boundary live in the
/// adjacent chunk and are handled by the caller.
pub fn brick_neighbors(idx: usize) -> Vec<usize> {
    let c = brick_coord(idx);
    let mut out = Vec::with_capacity(6);
    for d in [[1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1]] {
        let n = [c[0] + d[0], c[1] + d[1], c[2] + d[2]];
        if n.iter().all(|&v| v >= 0 && v < BRICKS_PER_AXIS) {
            out.push((n[0] + n[1] * BRICKS_PER_AXIS + n[2] * BRICKS_PER_AXIS * BRICKS_PER_AXIS) as usize);
        }
    }
    out
}

/// Per-chunk dirty set: one bit per brick. Every system that mutates a voxel
/// marks here; the mesher consumes it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirtyBricks {
    bits: u64,
}

impl DirtyBricks {
    pub const fn new() -> DirtyBricks {
        DirtyBricks { bits: 0 }
    }

    /// Mark the brick containing this chunk-local voxel. THE shared entry point
    /// for water/foliage/fire/edits — they all call this.
    pub fn mark(&mut self, local_voxel: [i32; 3]) {
        self.bits |= 1u64 << brick_index_of(local_voxel);
    }
    pub fn mark_brick(&mut self, idx: usize) {
        debug_assert!(idx < BRICKS_PER_CHUNK);
        self.bits |= 1u64 << idx;
    }
    /// Dirty the whole chunk (worldgen, bulk edit).
    pub fn mark_all(&mut self) {
        self.bits = u64::MAX;
    }
    pub fn is_dirty(&self, idx: usize) -> bool {
        self.bits & (1u64 << idx) != 0
    }
    pub fn clear(&mut self, idx: usize) {
        self.bits &= !(1u64 << idx);
    }
    pub fn clear_all(&mut self) {
        self.bits = 0;
    }
    pub fn any(&self) -> bool {
        self.bits != 0
    }
    pub fn count(&self) -> u32 {
        self.bits.count_ones()
    }
    /// Indices of the dirty bricks.
    pub fn iter_dirty(&self) -> impl Iterator<Item = usize> + '_ {
        (0..BRICKS_PER_CHUNK).filter(move |&i| self.is_dirty(i))
    }
}

/// A brick scheduled to re-mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrickJob {
    pub chunk: [i32; 3],
    pub brick: usize,
    pub center: [f32; 3],
}

/// World-space center of a brick.
pub fn brick_world_center(chunk: [i32; 3], brick: usize) -> [f32; 3] {
    let c = brick_coord(brick);
    let cs = CHUNK_SIZE as f32;
    let bs = BRICK_SIZE as f32;
    [
        chunk[0] as f32 * cs + c[0] as f32 * bs + bs * 0.5,
        chunk[1] as f32 * cs + c[1] as f32 * bs + bs * 0.5,
        chunk[2] as f32 * cs + c[2] as f32 * bs + bs * 0.5,
    ]
}

/// Result of choosing which dirty bricks to re-mesh this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct BrickSelection {
    /// Re-mesh these now (nearest-first), at most `budget`.
    pub run: Vec<BrickJob>,
    /// How many dirty bricks were left for a later frame.
    pub deferred: usize,
}

/// Pick up to `budget` dirty bricks across all chunks, nearest the camera first,
/// leaving the rest dirty for next frame. Small bricks + a budget = the whole-
/// chunk stall becomes a bounded trickle, the same way for every system.
pub fn select_bricks(
    dirty: &[([i32; 3], DirtyBricks)],
    cam: [f32; 3],
    budget: usize,
) -> BrickSelection {
    let mut jobs: Vec<BrickJob> = Vec::new();
    for &(chunk, set) in dirty {
        for brick in set.iter_dirty() {
            jobs.push(BrickJob { chunk, brick, center: brick_world_center(chunk, brick) });
        }
    }
    let d2 = |c: [f32; 3]| {
        let (x, y, z) = (c[0] - cam[0], c[1] - cam[1], c[2] - cam[2]);
        x * x + y * y + z * z
    };
    jobs.sort_by(|a, b| d2(a.center).partial_cmp(&d2(b.center)).unwrap());
    let total = jobs.len();
    let deferred = total.saturating_sub(budget);
    jobs.truncate(budget);
    BrickSelection { run: jobs, deferred }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_voxel_dirties_exactly_one_brick_regardless_of_which_system() {
        let mut d = DirtyBricks::new();
        // A water cell, a growing plant, and a player edit — three different
        // systems, all through the same mark().
        d.mark([1, 2, 3]); // water  -> brick (0,0,0) = 0
        d.mark([9, 1, 1]); // foliage-> brick (1,0,0) = 1
        d.mark([1, 1, 25]); // edit  -> brick (0,0,3) = 48
        assert_eq!(d.count(), 3, "three distinct bricks dirtied");
        assert!(d.is_dirty(0) && d.is_dirty(1) && d.is_dirty(48));
        // Touching the same brick again doesn't grow the set.
        d.mark([2, 2, 2]);
        assert_eq!(d.count(), 3, "second hit in brick 0 is idempotent");
    }

    #[test]
    fn brick_index_and_bounds_round_trip() {
        for idx in 0..BRICKS_PER_CHUNK {
            let (min, _max) = brick_bounds(idx);
            assert_eq!(brick_index_of(min), idx, "min voxel maps back to its brick");
            assert_eq!(brick_index_of([min[0] + 7, min[1] + 7, min[2] + 7]), idx, "far corner too");
        }
    }

    #[test]
    fn mark_all_then_clear() {
        let mut d = DirtyBricks::new();
        d.mark_all();
        assert_eq!(d.count(), BRICKS_PER_CHUNK as u32, "whole chunk dirty = all 64");
        d.clear(0);
        assert!(!d.is_dirty(0) && d.count() == 63);
        d.clear_all();
        assert!(!d.any());
    }

    #[test]
    fn neighbors_corner_vs_center() {
        // Brick (0,0,0) is a corner → 3 in-chunk neighbours.
        assert_eq!(brick_neighbors(0).len(), 3);
        // A center brick (1,1,1) → all 6.
        let center = (1 + 1 * BRICKS_PER_AXIS + 1 * BRICKS_PER_AXIS * BRICKS_PER_AXIS) as usize;
        assert_eq!(brick_neighbors(center).len(), 6);
    }

    #[test]
    fn select_takes_nearest_and_defers_the_rest() {
        let mut near = DirtyBricks::new();
        near.mark_brick(0); // chunk (0,0,0), brick 0 → near the camera
        let mut far = DirtyBricks::new();
        far.mark_brick(0); // chunk (10,0,0) → far away
        let dirty = [([0, 0, 0], near), ([10, 0, 0], far)];
        let sel = select_bricks(&dirty, [0.0, 0.0, 0.0], 1);
        assert_eq!(sel.run.len(), 1);
        assert_eq!(sel.deferred, 1, "the far brick waits for next frame");
        assert_eq!(sel.run[0].chunk, [0, 0, 0], "nearest brick goes first");
    }

    #[test]
    fn flood_of_dirty_bricks_is_bounded_per_frame() {
        // Simulate a whole chunk going dirty (e.g. a wave of water): 64 bricks.
        let mut d = DirtyBricks::new();
        d.mark_all();
        let sel = select_bricks(&[([0, 0, 0], d)], [0.0, 0.0, 0.0], 8);
        assert_eq!(sel.run.len(), 8, "only 8 re-meshed this frame");
        assert_eq!(sel.deferred, 56, "the other 56 trickle over later frames — no stall");
    }
}
