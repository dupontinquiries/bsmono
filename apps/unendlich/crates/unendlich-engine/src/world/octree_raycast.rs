//! Hierarchical raycast against the octree.
//!
//! v0.24.23. CPU implementation of the `raycast` functional unit
//! documented in `architecture-v1-addendum.md §A3.1`. This is the
//! primitive everything else builds on: physics swept-volume tests,
//! line-of-sight checks, projectile collision, item beam paths,
//! AI vision cones, mining-tool reach, and so on.
//!
//! ## Why the octree is fast for raycasts
//!
//! A naive dense-grid raycast walks the DDA (Digital Differential
//! Analyzer) algorithm cell-by-cell; for a 32-block ray across a
//! chunk, that's 32+ cell checks regardless of what's actually in
//! those cells. A raycast through 32 blocks of air does 32 reads.
//!
//! On a sparse octree, a ray through 32 blocks of air hits **one
//! node** — the depth-5 collapsed-air leaf at the chunk root.
//! Cost: one read, regardless of ray length within the homogeneous
//! region. This is the "skip pass" property the architecture doc
//! talks about: cost scales with the *complexity of the geometry*
//! the ray traverses, not with ray length.
//!
//! For a ray that bisects a chunk diagonally through varied
//! terrain, the cost is still O(traversed nodes) ≪ O(traversed
//! cells) for any reasonable octree. The win compounds when rays
//! get longer (cross-chunk casts at world scale).
//!
//! ## Algorithm
//!
//! Standard slab method against each octree node's AABB, then
//! recursive descent into intersecting children in front-to-back
//! order (closest first). At each leaf:
//!
//! - If the leaf is "passable" (Air, traversable plant, etc.):
//!   the ray continues through this region. The traversal advances
//!   `t_min` to the leaf's exit.
//! - If the leaf is "blocking" (Stone, Dirt, etc.): hit recorded.
//!   Return.
//!
//! Children are visited in the order determined by the ray's sign
//! per axis (positive direction → octant 0 before octant 1 on that
//! axis; negative → reverse). This guarantees front-to-back so the
//! first blocking leaf encountered IS the first hit.
//!
//! ## What this ship does NOT include
//!
//! - **GPU raycast.** The CPU implementation is the reference. A
//!   future ship transcribes the algorithm to a compute shader so
//!   item GPU forms can call it.
//! - **Cross-chunk casts.** Each chunk's octree is self-contained, so
//!   the single-chunk [`raycast_chunk`] only sees one chunk. The
//!   world-level [`raycast_world`] dispatcher (added below) walks the
//!   chunk grid with a 3D DDA and casts into each loaded chunk's octree
//!   front-to-back, returning the globally nearest hit in world
//!   coordinates — this is what the interaction layer (break/place/
//!   reach) actually calls.
//! - **Sub-leaf hit precision.** For chunks with a fully-collapsed
//!   homogeneous leaf, the hit position is the leaf's entry point
//!   (the surface of the cube). A future "expand-leaf-on-hit"
//!   refinement could subdivide for sub-block precision when
//!   needed.

use crate::constants::CHUNK_SIZE;
use crate::world::blocks::BlockId;
use crate::world::chunk::ChunkCoord;
use crate::world::octree::{OctreeArray, OCTREE_DEPTH};
use std::sync::Arc;

/// Result of a successful raycast.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RaycastHit {
    /// `t` value along the ray at the hit point (origin + t*dir).
    pub t: f32,
    /// Chunk-local position of the hit cell.
    pub cell: (i32, i32, i32),
    /// Block id encountered. Useful for items that care about
    /// "what did I hit" (a sword hitting a creature's hitbox vs a
    /// wood block triggers different effects).
    pub block: BlockId,
    /// Which axis the ray entered the hit cell on, encoded as a
    /// vector in {-1, 0, 1}^3. Used by mining-tool placement
    /// (face-attachment logic) and by item particle-spawning.
    pub normal: (i32, i32, i32),
}

/// Result of a successful world-level raycast — the cross-chunk analogue
/// of [`RaycastHit`]. The cell is in **world** coordinates (not
/// chunk-local), which is what interaction code wants: break the block at
/// `cell`, place against `place_cell()`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldRaycastHit {
    /// `t` along the ray (world `origin + t*dir`). In units of `|dir|`,
    /// so a unit `dir` makes `t` a distance in blocks.
    pub t: f32,
    /// World-space integer cell of the block struck.
    pub cell: (i32, i32, i32),
    /// Block id encountered.
    pub block: BlockId,
    /// Entry-face normal in {-1,0,1}^3.
    pub normal: (i32, i32, i32),
    /// Chunk the hit block lives in.
    pub chunk: ChunkCoord,
}

impl WorldRaycastHit {
    /// The empty cell adjacent to the struck face — where a placed block
    /// goes (the block-placement target). `cell + normal`.
    #[inline]
    pub fn place_cell(&self) -> (i32, i32, i32) {
        (
            self.cell.0 + self.normal.0,
            self.cell.1 + self.normal.1,
            self.cell.2 + self.normal.2,
        )
    }
}

/// Predicate for which blocks block a ray. Most callers want
/// `block_is_blocking`, but specialty cases (water rays, lasers
/// that pass through glass) can pass their own filter. Accepts any
/// callable so trait-object filters from the legacy raycast can
/// adapt with `|b| filter.stops(b)`.
pub fn block_is_blocking(b: BlockId) -> bool {
    !b.is_air() && b.is_solid()
}

/// Cast a ray within one chunk's octree. Returns the first hit, or
/// `None` if the ray exits the chunk without striking a blocking
/// block.
///
/// Inputs are in chunk-local coordinates. `max_t` bounds how far
/// the ray travels.
pub fn raycast_chunk(
    octree: &OctreeArray,
    origin: [f32; 3],
    dir: [f32; 3],
    max_t: f32,
    blocks: impl Fn(BlockId) -> bool + Copy,
) -> Option<RaycastHit> {
    debug_assert!(max_t > 0.0);

    // Inverse direction for slab tests. Zero components are handled
    // explicitly (rays parallel to a slab axis never enter/exit
    // that axis).
    let inv = [
        if dir[0].abs() > 1e-6 { 1.0 / dir[0] } else { f32::INFINITY },
        if dir[1].abs() > 1e-6 { 1.0 / dir[1] } else { f32::INFINITY },
        if dir[2].abs() > 1e-6 { 1.0 / dir[2] } else { f32::INFINITY },
    ];

    let chunk_size = crate::constants::CHUNK_SIZE as f32;
    let (t_enter, t_exit) = slab_intersect(origin, inv, [0.0, 0.0, 0.0], [chunk_size; 3]);
    if t_enter >= t_exit || t_exit <= 0.0 || t_enter >= max_t {
        return None;
    }

    let t_start = t_enter.max(0.0);
    let t_stop = t_exit.min(max_t);

    descend(
        octree,
        0,
        OCTREE_DEPTH,
        (0, 0, 0),
        origin,
        dir,
        inv,
        t_start,
        t_stop,
        blocks,
    )
}

/// Cast a ray across the loaded world, returning the first blocking hit.
///
/// This is the cross-chunk dispatcher promised in the module header: it
/// walks the chunk grid along the ray with a 3D DDA and casts
/// [`raycast_chunk`] into each loaded chunk's octree in front-to-back
/// order, so the first hit found is the globally nearest one. Chunks that
/// aren't loaded are skipped (the ray passes through them as if empty).
///
/// `get_octree` resolves a chunk coordinate to its octree — pass
/// `|c| chunk_manager.octree(c)`; the closure indirection keeps this
/// module free of a `ChunkManager` dependency and makes it unit-testable
/// with a plain map. `origin`/`dir` are in world space; `max_t` bounds the
/// reach (in units of `|dir|`, so a unit `dir` gives reach in blocks).
///
/// This is the picking primitive the interaction layer sits on: a camera
/// ray (`origin = eye`, `dir = look`, `max_t = reach`) yields the targeted
/// block to break (`hit.cell`), the face to place against
/// (`hit.place_cell()`), and what was struck (`hit.block`).
pub fn raycast_world<F, B>(
    get_octree: F,
    origin: [f32; 3],
    dir: [f32; 3],
    max_t: f32,
    blocks: B,
) -> Option<WorldRaycastHit>
where
    F: Fn(ChunkCoord) -> Option<Arc<OctreeArray>>,
    B: Fn(BlockId) -> bool + Copy,
{
    debug_assert!(max_t > 0.0);
    // Degenerate direction: nothing to march.
    if dir[0].abs() < 1e-9 && dir[1].abs() < 1e-9 && dir[2].abs() < 1e-9 {
        return None;
    }
    let cs = CHUNK_SIZE as f32;

    // Chunk containing the origin point (floor-divide handles negatives).
    let mut chunk = [
        (origin[0] / cs).floor() as i32,
        (origin[1] / cs).floor() as i32,
        (origin[2] / cs).floor() as i32,
    ];

    // Amanatides–Woo DDA over chunk-sized cells.
    let mut step = [0i32; 3];
    let mut t_max = [f32::INFINITY; 3];
    let t_delta = [
        if dir[0].abs() > 1e-9 { (cs / dir[0]).abs() } else { f32::INFINITY },
        if dir[1].abs() > 1e-9 { (cs / dir[1]).abs() } else { f32::INFINITY },
        if dir[2].abs() > 1e-9 { (cs / dir[2]).abs() } else { f32::INFINITY },
    ];
    for ax in 0..3 {
        if dir[ax].abs() > 1e-9 {
            step[ax] = if dir[ax] > 0.0 { 1 } else { -1 };
            let next_boundary = if step[ax] > 0 {
                (chunk[ax] + 1) as f32 * cs
            } else {
                chunk[ax] as f32 * cs
            };
            t_max[ax] = (next_boundary - origin[ax]) / dir[ax];
        }
    }

    loop {
        let coord = ChunkCoord::new(chunk[0], chunk[1], chunk[2]);
        if let Some(oct) = get_octree(coord) {
            let local_origin = [
                origin[0] - chunk[0] as f32 * cs,
                origin[1] - chunk[1] as f32 * cs,
                origin[2] - chunk[2] as f32 * cs,
            ];
            // Same `dir`, origin merely translated → `hit.t` stays the
            // world ray parameter, directly comparable to `max_t`.
            if let Some(hit) = raycast_chunk(&oct, local_origin, dir, max_t, blocks) {
                return Some(WorldRaycastHit {
                    t: hit.t,
                    cell: (
                        chunk[0] * CHUNK_SIZE + hit.cell.0,
                        chunk[1] * CHUNK_SIZE + hit.cell.1,
                        chunk[2] * CHUNK_SIZE + hit.cell.2,
                    ),
                    block: hit.block,
                    normal: hit.normal,
                    chunk: coord,
                });
            }
        }

        // Step into the next chunk along the nearest boundary axis.
        let axis = if t_max[0] < t_max[1] {
            if t_max[0] < t_max[2] { 0 } else { 2 }
        } else if t_max[1] < t_max[2] {
            1
        } else {
            2
        };
        if t_max[axis] > max_t || !t_max[axis].is_finite() {
            return None;
        }
        chunk[axis] += step[axis];
        t_max[axis] += t_delta[axis];
    }
}

/// Slab method for an axis-aligned box. Returns (t_enter, t_exit).
/// If the ray misses, t_enter > t_exit.
///
/// Edge case: when the ray is parallel to an axis AND the origin lies
/// exactly on that axis's slab face, the naive `(box - origin) * inv`
/// computation evaluates to `0 * INFINITY = NaN`. NaN comparisons all
/// return false in Rust, so the `if t1 < t2 { } else { }` branching
/// silently took the wrong branch, producing bogus (t_min, t_max)
/// pairs that wrongly rejected the child octant. Explicit handling:
/// for a ray parallel to an axis, treat the slab as a YES/NO check
/// on whether the origin is inside the slab range. Inside → axis
/// doesn't constrain. Outside → ray misses, return empty.
fn slab_intersect(
    origin: [f32; 3],
    inv_dir: [f32; 3],
    box_min: [f32; 3],
    box_max: [f32; 3],
) -> (f32, f32) {
    let mut t_min: f32 = f32::NEG_INFINITY;
    let mut t_max: f32 = f32::INFINITY;
    for ax in 0..3 {
        if inv_dir[ax].is_infinite() {
            // Ray parallel to this axis. Boundary is inclusive — the
            // ray is "inside" the slab if origin lies in [min, max].
            if origin[ax] < box_min[ax] || origin[ax] > box_max[ax] {
                return (f32::INFINITY, f32::NEG_INFINITY); // miss
            }
            // Otherwise this axis contributes no constraint.
            continue;
        }
        let t1 = (box_min[ax] - origin[ax]) * inv_dir[ax];
        let t2 = (box_max[ax] - origin[ax]) * inv_dir[ax];
        let (a, b) = if t1 < t2 { (t1, t2) } else { (t2, t1) };
        t_min = t_min.max(a);
        t_max = t_max.min(b);
    }
    (t_min, t_max)
}

#[allow(clippy::too_many_arguments)]
fn descend(
    octree: &OctreeArray,
    node_idx: u32,
    depth: u32,
    origin_cell: (i32, i32, i32),
    ray_origin: [f32; 3],
    ray_dir: [f32; 3],
    inv_dir: [f32; 3],
    t_enter: f32,
    t_exit: f32,
    blocks: impl Fn(BlockId) -> bool + Copy,
) -> Option<RaycastHit> {
    let node = octree.nodes[node_idx as usize];
    let size = 1i32 << depth;
    let size_f = size as f32;

    let box_min = [
        origin_cell.0 as f32,
        origin_cell.1 as f32,
        origin_cell.2 as f32,
    ];
    let box_max = [
        box_min[0] + size_f,
        box_min[1] + size_f,
        box_min[2] + size_f,
    ];

    if node.is_leaf() {
        let block = node.leaf_block();
        if !blocks(block) {
            return None; // passable leaf, ray continues
        }
        // Hit. Recover the entering face normal: the ray entered on
        // the axis whose slab gave the largest `t_enter`.
        let normal = entry_normal(ray_origin, inv_dir, box_min, box_max, t_enter);
        // Hit cell = whichever 1x1x1 cell within the leaf the ray
        // strikes first. For a depth-0 leaf that's the leaf
        // itself; for a coarser leaf (homogeneous collapsed
        // region) we report the cell at the entry face.
        // Hit cell = whichever 1x1x1 cell within the leaf the ray
        // strikes first. The entry point lies EXACTLY on the leaf's
        // entry face, so a bare `floor` of it is ambiguous on the
        // boundary: a ray entering a cell's MAX face (i.e. looking
        // toward the -axis) would floor to the air cell in FRONT of the
        // block instead of the solid cell itself — breaking break/place
        // for all -X/-Y/-Z facings. Nudge a hair ALONG the ray (into the
        // leaf) before flooring so the solid entry cell is reported for
        // every facing. `ray_dir` is unit in the picking path, so this is
        // a sub-millimetre step, far less than one cell.
        const CELL_EPS: f32 = 1e-3;
        let hit_pos = [
            ray_origin[0] + ray_dir[0] * (t_enter + CELL_EPS),
            ray_origin[1] + ray_dir[1] * (t_enter + CELL_EPS),
            ray_origin[2] + ray_dir[2] * (t_enter + CELL_EPS),
        ];
        let cell = (
            hit_pos[0].floor() as i32,
            hit_pos[1].floor() as i32,
            hit_pos[2].floor() as i32,
        );
        return Some(RaycastHit {
            t: t_enter,
            cell,
            block,
            normal,
        });
    }

    // Internal node. Visit the 8 children in ray order. Compute
    // each child's slab intersection, sort by t_enter, recurse.
    let half = size >> 1;
    let half_f = half as f32;

    // Octant entries: (octant_index, t_enter, t_exit).
    let mut octants: [(usize, f32, f32); 8] = [(0, 0.0, 0.0); 8];
    let mut n = 0;
    for o in 0..8 {
        let dx = (o & 1) as i32 * half;
        let dy = ((o >> 1) & 1) as i32 * half;
        let dz = ((o >> 2) & 1) as i32 * half;
        let child_min = [
            box_min[0] + dx as f32,
            box_min[1] + dy as f32,
            box_min[2] + dz as f32,
        ];
        let child_max = [
            child_min[0] + half_f,
            child_min[1] + half_f,
            child_min[2] + half_f,
        ];
        let (ce, cx) = slab_intersect(ray_origin, inv_dir, child_min, child_max);
        // Clamp to parent's [t_enter, t_exit] window.
        let ce = ce.max(t_enter);
        let cx = cx.min(t_exit);
        if ce < cx {
            octants[n] = (o, ce, cx);
            n += 1;
        }
    }

    // Bubble sort by t_enter (8 elements, sort cost is negligible).
    for i in 0..n {
        for j in 0..n - 1 - i {
            if octants[j].1 > octants[j + 1].1 {
                octants.swap(j, j + 1);
            }
        }
    }

    // Recurse front-to-back. First blocking hit wins.
    for &(o, ce, cx) in &octants[..n] {
        let dx = (o & 1) as i32 * half;
        let dy = ((o >> 1) & 1) as i32 * half;
        let dz = ((o >> 2) & 1) as i32 * half;
        let child_origin = (
            origin_cell.0 + dx,
            origin_cell.1 + dy,
            origin_cell.2 + dz,
        );
        if let Some(hit) = descend(
            octree,
            node.children[o],
            depth - 1,
            child_origin,
            ray_origin,
            ray_dir,
            inv_dir,
            ce,
            cx,
            blocks,
        ) {
            return Some(hit);
        }
    }
    None
}

/// Recover the entering face normal from which slab the t_enter
/// belonged to. The slab whose `(box_*[ax] - origin[ax]) * inv[ax]`
/// equals `t_enter` is the entering axis; sign of inv_dir picks
/// which face on that axis.
fn entry_normal(
    origin: [f32; 3],
    inv_dir: [f32; 3],
    box_min: [f32; 3],
    box_max: [f32; 3],
    t_enter: f32,
) -> (i32, i32, i32) {
    for ax in 0..3 {
        // Skip axes the ray is parallel to: they can't be the
        // entering axis (no slab crossing on a parallel axis), and
        // their t1/t2 may evaluate to NaN via 0 * INF. Same fix
        // pattern as `slab_intersect`.
        if inv_dir[ax].is_infinite() {
            continue;
        }
        let t1 = (box_min[ax] - origin[ax]) * inv_dir[ax];
        let t2 = (box_max[ax] - origin[ax]) * inv_dir[ax];
        let near = t1.min(t2);
        if (near - t_enter).abs() < 1e-4 {
            // The face is the one whose value is `near`. If
            // inv_dir is positive (dir is positive on this axis),
            // the entering face is min → normal points -axis.
            // If inv_dir is negative, entering face is max →
            // normal points +axis.
            let n = if inv_dir[ax] > 0.0 { -1 } else { 1 };
            return match ax {
                0 => (n, 0, 0),
                1 => (0, n, 0),
                _ => (0, 0, n),
            };
        }
    }
    (0, 0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::CHUNK_VOLUME;
    use crate::world::chunk::{Chunk, ChunkCoord};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_chunk() -> Chunk {
        Chunk::from_blocks(
            ChunkCoord::new(0, 0, 0),
            vec![BlockId::Air; CHUNK_VOLUME],
        )
    }

    #[test]
    fn empty_chunk_no_hit() {
        let chunk = make_chunk();
        let oct = OctreeArray::from_chunk(&chunk);
        let hit = raycast_chunk(
            &oct,
            [0.5, 0.5, 0.5],
            [1.0, 0.0, 0.0],
            64.0,
            block_is_blocking,
        );
        assert!(hit.is_none(), "ray through air should not hit");
    }

    #[test]
    fn ray_finds_stone_wall() {
        let mut chunk = make_chunk();
        for y in 0..32 {
            for z in 0..32 {
                chunk.set(15, y, z, BlockId::Stone);
            }
        }
        let oct = OctreeArray::from_chunk(&chunk);
        let hit = raycast_chunk(
            &oct,
            [0.5, 16.0, 16.0],
            [1.0, 0.0, 0.0],
            32.0,
            block_is_blocking,
        );
        let hit = hit.expect("must hit stone wall");
        assert_eq!(hit.block, BlockId::Stone);
        assert_eq!(hit.cell.0, 15);
        // Entered from -x face.
        assert_eq!(hit.normal, (-1, 0, 0));
        // t ≈ 14.5 (distance from x=0.5 to x=15.0).
        assert!((hit.t - 14.5).abs() < 0.01, "t={}", hit.t);
    }

    #[test]
    fn ray_hits_first_block_not_second() {
        let mut chunk = make_chunk();
        chunk.set(5, 16, 16, BlockId::Stone);
        chunk.set(10, 16, 16, BlockId::Dirt);
        let oct = OctreeArray::from_chunk(&chunk);
        let hit = raycast_chunk(
            &oct,
            [0.5, 16.5, 16.5],
            [1.0, 0.0, 0.0],
            32.0,
            block_is_blocking,
        ).expect("must hit");
        assert_eq!(hit.block, BlockId::Stone, "front-to-back ordering required");
        assert_eq!(hit.cell.0, 5);
    }

    #[test]
    fn ray_negative_dir_reports_solid_cell_not_air() {
        // Regression (v0.30): a ray looking toward -X/-Y/-Z enters the target
        // cell at its MAX face, where the entry point sits exactly on the
        // boundary. Pre-fix, `floor` of that boundary reported the AIR cell in
        // front of the block, so break hit air (no-op) and place landed a cell
        // too far out — only on negative facings. The hit cell must be the
        // SOLID cell, with the normal pointing back toward the origin.
        let solid = (5i32, 16i32, 16i32);
        for (dir, origin, want_normal) in [
            ([-1.0, 0.0, 0.0], [20.5, 16.5, 16.5], (1, 0, 0)),
            ([0.0, -1.0, 0.0], [5.5, 28.5, 16.5], (0, 1, 0)),
            ([0.0, 0.0, -1.0], [5.5, 16.5, 28.5], (0, 0, 1)),
        ] {
            let mut chunk = make_chunk();
            chunk.set(solid.0, solid.1, solid.2, BlockId::Stone);
            let oct = OctreeArray::from_chunk(&chunk);
            let hit = raycast_chunk(&oct, origin, dir, 32.0, block_is_blocking)
                .expect("must hit the solid cell");
            assert_eq!(hit.block, BlockId::Stone);
            assert_eq!(hit.cell, solid, "hit cell must be the solid cell, not the air cell in front (dir={dir:?})");
            assert_eq!(hit.normal, want_normal, "normal points back toward origin (dir={dir:?})");
        }
    }

    #[test]
    fn diagonal_ray_in_collapsed_air() {
        // All-air chunk → collapses to one node. Ray traverses the
        // diagonal of the whole chunk in a single octree visit.
        // The point of the test is "the algorithm doesn't choke on
        // the collapsed case" — there's no hit to find, but the
        // call should return cleanly.
        let chunk = make_chunk();
        let oct = OctreeArray::from_chunk(&chunk);
        let hit = raycast_chunk(
            &oct,
            [0.5, 0.5, 0.5],
            [1.0, 1.0, 1.0],
            64.0,
            block_is_blocking,
        );
        assert!(hit.is_none());
    }

    #[test]
    fn ray_misses_box() {
        let mut chunk = make_chunk();
        for y in 14..18 {
            for z in 14..18 {
                for x in 14..18 {
                    chunk.set(x, y, z, BlockId::Stone);
                }
            }
        }
        let oct = OctreeArray::from_chunk(&chunk);
        // Ray that misses the box entirely (parallel to x, well
        // above the box's y range).
        let hit = raycast_chunk(
            &oct,
            [0.5, 25.5, 16.5],
            [1.0, 0.0, 0.0],
            32.0,
            block_is_blocking,
        );
        assert!(hit.is_none());
    }

    // ── World-level (cross-chunk) dispatcher ────────────────────────────

    /// Octree for a chunk with a full Stone wall at chunk-local x = `wall`.
    /// The octree is chunk-local (coord-agnostic), so it can be mapped at
    /// any `ChunkCoord` in a test scene.
    fn wall_octree(wall: i32) -> Arc<OctreeArray> {
        let mut chunk =
            Chunk::from_blocks(ChunkCoord::new(0, 0, 0), vec![BlockId::Air; CHUNK_VOLUME]);
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                chunk.set(wall, y, z, BlockId::Stone);
            }
        }
        Arc::new(OctreeArray::from_chunk(&chunk))
    }

    #[test]
    fn world_ray_hits_in_origin_chunk() {
        let mut map: HashMap<ChunkCoord, Arc<OctreeArray>> = HashMap::new();
        map.insert(ChunkCoord::new(0, 0, 0), wall_octree(15));
        let hit = raycast_world(
            |c| map.get(&c).cloned(),
            [0.5, 16.5, 16.5],
            [1.0, 0.0, 0.0],
            32.0,
            block_is_blocking,
        )
        .expect("must hit wall in origin chunk");
        assert_eq!(hit.cell, (15, 16, 16));
        assert_eq!(hit.chunk, ChunkCoord::new(0, 0, 0));
        assert_eq!(hit.normal, (-1, 0, 0));
        assert!((hit.t - 14.5).abs() < 0.01, "t={}", hit.t);
    }

    #[test]
    fn world_ray_crosses_chunk_boundary() {
        // Origin chunk all air; the wall lives in the next chunk over (+x),
        // at local x=5 → world x = 1*CHUNK_SIZE + 5.
        let mut map: HashMap<ChunkCoord, Arc<OctreeArray>> = HashMap::new();
        map.insert(
            ChunkCoord::new(0, 0, 0),
            Arc::new(OctreeArray::from_chunk(&Chunk::from_blocks(
                ChunkCoord::new(0, 0, 0),
                vec![BlockId::Air; CHUNK_VOLUME],
            ))),
        );
        map.insert(ChunkCoord::new(1, 0, 0), wall_octree(5));
        let expected_x = CHUNK_SIZE + 5;
        let hit = raycast_world(
            |c| map.get(&c).cloned(),
            [0.5, 16.5, 16.5],
            [1.0, 0.0, 0.0],
            64.0,
            block_is_blocking,
        )
        .expect("must hit wall across the chunk boundary");
        assert_eq!(hit.cell, (expected_x, 16, 16));
        assert_eq!(hit.chunk, ChunkCoord::new(1, 0, 0));
        assert!((hit.t - (expected_x as f32 - 0.5)).abs() < 0.01, "t={}", hit.t);
    }

    #[test]
    fn world_ray_skips_unloaded_chunk() {
        // Origin chunk NOT loaded — the ray should pass through it as empty
        // and still find the wall in the loaded chunk beyond.
        let mut map: HashMap<ChunkCoord, Arc<OctreeArray>> = HashMap::new();
        map.insert(ChunkCoord::new(1, 0, 0), wall_octree(5));
        let hit = raycast_world(
            |c| map.get(&c).cloned(),
            [0.5, 16.5, 16.5],
            [1.0, 0.0, 0.0],
            64.0,
            block_is_blocking,
        )
        .expect("unloaded origin chunk must be traversed as empty");
        assert_eq!(hit.chunk, ChunkCoord::new(1, 0, 0));
        assert_eq!(hit.cell, (CHUNK_SIZE + 5, 16, 16));
    }

    #[test]
    fn world_ray_respects_max_t() {
        let mut map: HashMap<ChunkCoord, Arc<OctreeArray>> = HashMap::new();
        map.insert(ChunkCoord::new(1, 0, 0), wall_octree(5));
        // Wall is ~37 blocks away; a 10-block reach can't touch it.
        let hit = raycast_world(
            |c| map.get(&c).cloned(),
            [0.5, 16.5, 16.5],
            [1.0, 0.0, 0.0],
            10.0,
            block_is_blocking,
        );
        assert!(hit.is_none(), "hit beyond reach must be rejected");
    }

    #[test]
    fn place_cell_is_adjacent_to_struck_face() {
        let hit = WorldRaycastHit {
            t: 14.5,
            cell: (15, 16, 16),
            block: BlockId::Stone,
            normal: (-1, 0, 0),
            chunk: ChunkCoord::new(0, 0, 0),
        };
        assert_eq!(hit.place_cell(), (14, 16, 16));
    }
}
