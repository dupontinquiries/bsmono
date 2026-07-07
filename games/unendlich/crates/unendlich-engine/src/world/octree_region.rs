//! Region reads against the octree (item-side batched query).
//!
//! v0.24.23. CPU implementation of the `read_region` operation
//! documented in `architecture-v1-addendum.md §A1.2`:
//!
//! > Item code that reads many positions in a small region (sphere
//! > of candidates, line cast, AOE check) should NOT call
//! > `ctx.block_at(pos)` in a loop — each call walks overlay-then-
//! > octree. Instead, item code requests a region read: the engine
//! > consults overlay-then-octree once for the whole region, hands
//! > back a dense buffer the item can index in O(1).
//!
//! ## Sequence
//!
//! 1. Item asks: `let read = ctx.read_region(shape, current_tick);`
//! 2. Engine resolves shape → AABB, samples octree once per cell,
//!    masks cells outside the shape (Sphere/Cone etc.) to Air.
//! 3. Returns a `RegionRead` carrying the AABB, dense block array,
//!    and capture tick.
//! 4. Item iterates with O(1) `get_local(pos)` lookups inside its
//!    inner loop.
//! 5. Next tick (or next item invocation): item checks
//!    `read.is_stable(&octree)` — if true, reuse the snapshot; if
//!    false, rebuild. Stability is answered in O(log depth) at the
//!    typical "nothing in this region changed" case.
//!
//! ## What this ship does NOT include
//!
//! - **Cross-chunk reads.** A region that spans chunk boundaries
//!   needs a world-level octree-of-chunks (filed for the world-level
//!   octree ship). v0.24.23 limits reads to one chunk.
//! - **Overlay consultation.** §A1.2 says reads go "overlay first,
//!   then octree." The CPU overlay buffer (§3.2) doesn't exist yet.
//!   When it lands, this module gains an overlay-aware wrapper.

use crate::world::blocks::BlockId;
use crate::world::chunk::ChunkCoord;
use crate::world::octree::OctreeArray;

/// Closed-open axis-aligned bounding box in chunk-local coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aabb3 {
    pub min: (i32, i32, i32),
    pub max: (i32, i32, i32),
}

impl Aabb3 {
    pub fn new(min: (i32, i32, i32), max: (i32, i32, i32)) -> Self {
        debug_assert!(min.0 < max.0 && min.1 < max.1 && min.2 < max.2,
            "AABB collapsed: min={:?} max={:?}", min, max);
        Aabb3 { min, max }
    }

    pub fn extents(&self) -> (i32, i32, i32) {
        (self.max.0 - self.min.0, self.max.1 - self.min.1, self.max.2 - self.min.2)
    }

    pub fn cell_count(&self) -> usize {
        let (ex, ey, ez) = self.extents();
        (ex * ey * ez) as usize
    }

    pub fn contains(&self, p: (i32, i32, i32)) -> bool {
        self.min.0 <= p.0 && p.0 < self.max.0
            && self.min.1 <= p.1 && p.1 < self.max.1
            && self.min.2 <= p.2 && p.2 < self.max.2
    }
}

/// Shape catalog items can request. The engine resolves each to a
/// covering AABB before issuing the actual read, then masks the
/// result so out-of-shape cells return Air.
#[derive(Debug, Clone, Copy)]
pub enum RegionShape {
    Cube   { center: (i32, i32, i32), radius: i32 },
    Sphere { center: (i32, i32, i32), radius: i32 },
    Box(Aabb3),
}

impl RegionShape {
    pub fn aabb(&self) -> Aabb3 {
        match *self {
            RegionShape::Cube { center, radius }
          | RegionShape::Sphere { center, radius } => {
                let r = radius;
                Aabb3::new(
                    (center.0 - r, center.1 - r, center.2 - r),
                    (center.0 + r + 1, center.1 + r + 1, center.2 + r + 1),
                )
            }
            RegionShape::Box(aabb) => aabb,
        }
    }

    /// True if (in shape's local frame) this point should carry a
    /// real block_id; false → post-fill mask sets it to Air.
    fn includes(&self, p: (i32, i32, i32)) -> bool {
        match *self {
            RegionShape::Cube { .. } | RegionShape::Box(_) => true,
            RegionShape::Sphere { center, radius } => {
                let dx = p.0 - center.0;
                let dy = p.1 - center.1;
                let dz = p.2 - center.2;
                dx * dx + dy * dy + dz * dz <= radius * radius
            }
        }
    }
}

/// Snapshot of blocks within a region. O(1) lookups + a cheap
/// validity check against the live octree.
#[derive(Debug, Clone)]
pub struct RegionRead {
    pub chunk_coord: ChunkCoord,
    pub aabb: Aabb3,
    /// Dense block array, indexed `x + sx * (z + sz * y)` where
    /// `(sx, _, sz) = aabb.extents()`. Row-major with X fastest.
    pub blocks: Vec<BlockId>,
    /// Tick at which this snapshot was captured. The validity check
    /// asks the octree whether any leaf in the AABB has been
    /// modified at or after this tick.
    pub captured_tick: u32,
}

impl RegionRead {
    /// Build a region read from one chunk's octree.
    pub fn from_octree_chunk_local(
        octree: &OctreeArray,
        shape: RegionShape,
        captured_tick: u32,
    ) -> Self {
        let aabb = shape.aabb();
        let (sx, sy, sz) = aabb.extents();
        let mut blocks = Vec::with_capacity((sx * sy * sz) as usize);
        let chunk_size = crate::constants::CHUNK_SIZE;

        for y in aabb.min.1..aabb.max.1 {
            for z in aabb.min.2..aabb.max.2 {
                for x in aabb.min.0..aabb.max.0 {
                    let in_range = (0..chunk_size).contains(&x)
                        && (0..chunk_size).contains(&y)
                        && (0..chunk_size).contains(&z);
                    let block = if in_range && shape.includes((x, y, z)) {
                        octree.sample(x, y, z)
                    } else {
                        BlockId::Air
                    };
                    blocks.push(block);
                }
            }
        }

        RegionRead { chunk_coord: octree.coord, aabb, blocks, captured_tick }
    }

    /// O(1) lookup at chunk-local coordinates. Air for out-of-AABB
    /// positions (so an item with a bug doesn't panic).
    pub fn get_local(&self, p: (i32, i32, i32)) -> BlockId {
        if !self.aabb.contains(p) {
            return BlockId::Air;
        }
        let (sx, _sy, sz) = self.aabb.extents();
        let dx = p.0 - self.aabb.min.0;
        let dy = p.1 - self.aabb.min.1;
        let dz = p.2 - self.aabb.min.2;
        let idx = (dx + sx * (dz + sz * dy)) as usize;
        self.blocks[idx]
    }

    /// Cache-deferment check: is this snapshot still valid?
    ///
    /// Returns true if no leaf within the AABB was modified after
    /// `captured_tick`. Short-circuits at the highest ancestor whose
    /// `last_modified_tick <= captured_tick`, so the typical "region
    /// untouched for many ticks" answer is near-constant cost.
    pub fn is_stable(&self, octree: &OctreeArray) -> bool {
        octree.region_stable_since(
            self.aabb.min,
            self.aabb.max,
            self.captured_tick + 1,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::CHUNK_VOLUME;
    use crate::world::chunk::Chunk;

    fn empty_chunk() -> Chunk {
        Chunk::from_blocks(ChunkCoord::new(0, 0, 0), vec![BlockId::Air; CHUNK_VOLUME])
    }

    #[test]
    fn aabb_cube_radius_4_has_729_cells() {
        let aabb = RegionShape::Cube { center: (16, 16, 16), radius: 4 }.aabb();
        assert_eq!(aabb.cell_count(), 9 * 9 * 9);
    }

    #[test]
    fn region_read_returns_blocks() {
        let mut chunk = empty_chunk();
        chunk.set(15, 16, 17, BlockId::Stone);
        chunk.set(20, 16, 17, BlockId::Dirt);
        let octree = OctreeArray::from_chunk(&chunk);

        let read = RegionRead::from_octree_chunk_local(
            &octree,
            RegionShape::Cube { center: (16, 16, 16), radius: 4 },
            0,
        );
        assert_eq!(read.get_local((15, 16, 17)), BlockId::Stone);
        assert_eq!(read.get_local((20, 16, 17)), BlockId::Dirt);
        assert_eq!(read.get_local((16, 16, 16)), BlockId::Air);
        assert_eq!(read.get_local((0, 0, 0)), BlockId::Air); // outside AABB
    }

    #[test]
    fn sphere_masks_corners_to_air() {
        let mut chunk = empty_chunk();
        for y in 14..18 {
            for z in 14..18 {
                for x in 14..18 {
                    chunk.set(x, y, z, BlockId::Stone);
                }
            }
        }
        let octree = OctreeArray::from_chunk(&chunk);

        let read = RegionRead::from_octree_chunk_local(
            &octree,
            RegionShape::Sphere { center: (16, 16, 16), radius: 1 },
            0,
        );
        assert_eq!(read.get_local((16, 16, 16)), BlockId::Stone);
        assert_eq!(read.get_local((15, 16, 16)), BlockId::Stone);
        assert_eq!(read.get_local((17, 16, 16)), BlockId::Stone);
        // Corner at distance √3 ≈ 1.73 > radius=1: masked to Air.
        assert_eq!(read.get_local((15, 15, 15)), BlockId::Air);
    }

    #[test]
    fn is_stable_true_when_quiet() {
        let chunk = empty_chunk();
        let octree = OctreeArray::from_chunk(&chunk);
        let read = RegionRead::from_octree_chunk_local(
            &octree,
            RegionShape::Cube { center: (16, 16, 16), radius: 2 },
            10,
        );
        assert!(read.is_stable(&octree));
    }

    #[test]
    fn is_stable_false_after_in_region_edit() {
        // Use a chunk with structure so the octree decomposes
        // (a single-leaf chunk poisons cache invalidation across
        // the whole cube — documented limitation; future
        // leaf-expansion-on-write fixes it).
        let mut chunk = empty_chunk();
        chunk.set(0, 0, 0, BlockId::Stone);
        chunk.set(20, 20, 20, BlockId::Dirt);
        let mut octree = OctreeArray::from_chunk(&chunk);

        let read = RegionRead::from_octree_chunk_local(
            &octree,
            RegionShape::Cube { center: (16, 16, 16), radius: 2 },
            10,
        );
        octree.mark_path_modified(16, 16, 16, 20);
        assert!(!read.is_stable(&octree),
            "read at tick 10 must see tick-20 edit inside its AABB");
    }
}
