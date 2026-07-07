//! Raycast — engine primitive for visibility, hit detection, lighting,
//! mob vision, and physics queries.
//!
//! Many systems need to ask "what block does this ray hit, and where?":
//!   - Player input: which block am I aiming at? Where do I place a new one?
//!   - Lighting: does sunlight reach this point?
//!   - Mob AI: can the zombie see the player past the tall grass?
//!   - Light physics: how far does this glow propagate before being absorbed?
//!   - Water/fluid: where does this stream of water flow next?
//!   - Wind: which exposed surfaces does this gust hit?
//!
//! Each call site has different constraints (max distance, what counts as
//! opaque, what to ignore) so the API takes a `RayQuery` describing the ray
//! plus a filter callback.
//!
//! `RayKind` is a metadata tag — the algorithm is the same for all kinds,
//! but tagging lets future systems budget rays per kind, prioritize between
//! them, log per-kind stats, etc.
//!
//! Algorithm: Amanatides & Woo (1987) grid-DDA. Walks integer block cells
//! along the ray, jumping minimum t to next axis crossing. O(L) where L is
//! the ray length in cells. No floating-point error accumulation issues
//! that plague naive stepped sampling.

use crate::physics::BlockSampler;
use crate::world::blocks::BlockId;

/// What's this ray for? Metadata only — doesn't change algorithm. Lets
/// engine code budget / prioritize / log per-kind without re-architecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RayKind {
    /// Player looking at a block (placement, breaking, info).
    PlayerVision,
    /// Player ability or attack (sword reach, projectile path).
    AbilityHit,
    /// Mob trying to see the player.
    MobVision,
    /// Sunlight or torch light propagation.
    Lighting,
    /// Light through translucent materials (water, glass, crystal).
    LightTransport,
    /// Fluid flow (water, lava drops looking for next cell).
    Fluid,
    /// Wind / airflow. Used for erosion, particles, sound propagation.
    Wind,
    /// Generic physics query (object trying to find resting place).
    Physics,
    /// Debug / dev-tool query.
    Debug,
}

/// Which face of the block was struck. Useful for placement (you place the
/// new block adjacent to the struck face).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockFace {
    PosX, NegX,
    PosY, NegY,
    PosZ, NegZ,
}

impl BlockFace {
    /// Unit normal vector for this face (pointing outward from the block).
    pub fn normal(self) -> [i32; 3] {
        match self {
            BlockFace::PosX => [ 1,  0,  0],
            BlockFace::NegX => [-1,  0,  0],
            BlockFace::PosY => [ 0,  1,  0],
            BlockFace::NegY => [ 0, -1,  0],
            BlockFace::PosZ => [ 0,  0,  1],
            BlockFace::NegZ => [ 0,  0, -1],
        }
    }

    /// Inverse of `normal()`: recover the face from a unit normal
    /// vector in {-1, 0, 1}^3. Returns `None` if the vector isn't a
    /// canonical face normal (e.g., zero, diagonal).
    ///
    /// v0.24.23: bridge for `OctreeWorldRayHit { normal: (i32, i32,
    /// i32) }` → legacy `RayHit { face: BlockFace }`. Lets callers
    /// adopt `ChunkManager::raycast_octree` without changing the
    /// downstream "place block adjacent to hit face" logic.
    pub fn from_normal(n: (i32, i32, i32)) -> Option<Self> {
        match n {
            ( 1,  0,  0) => Some(BlockFace::PosX),
            (-1,  0,  0) => Some(BlockFace::NegX),
            ( 0,  1,  0) => Some(BlockFace::PosY),
            ( 0, -1,  0) => Some(BlockFace::NegY),
            ( 0,  0,  1) => Some(BlockFace::PosZ),
            ( 0,  0, -1) => Some(BlockFace::NegZ),
            _ => None,
        }
    }

    /// The neighbor cell adjacent to this face of the given block. Used for
    /// "place block adjacent to the face I clicked on" logic.
    pub fn neighbor(self, block: [i32; 3]) -> [i32; 3] {
        let n = self.normal();
        [block[0] + n[0], block[1] + n[1], block[2] + n[2]]
    }
}

/// Result of a successful raycast.
#[derive(Debug, Clone, Copy)]
pub struct RayHit {
    /// Integer block coordinates of the hit cell.
    pub block: [i32; 3],
    /// Block ID at the hit cell.
    pub block_id: BlockId,
    /// Which face of the block was crossed by the ray.
    pub face: BlockFace,
    /// Distance from ray origin to the hit point, in blocks.
    pub distance: f32,
    /// World-space hit point (where the ray crossed the face).
    pub point: [f32; 3],
}

/// Ray query specification.
#[derive(Debug, Clone, Copy)]
pub struct RayQuery {
    pub origin: [f32; 3],
    /// Direction (need not be normalized — we normalize internally).
    pub direction: [f32; 3],
    /// Maximum distance to traverse, in blocks.
    pub max_distance: f32,
    pub kind: RayKind,
}

/// Filter trait: decides whether a given block stops the ray.
///
/// Default `OpaqueOnlyFilter` stops on any solid, non-transparent block —
/// the right default for player vision and most physics queries. Custom
/// filters can ignore certain block types (mob vision through tall grass),
/// require specific block types (fluid flowing into water), etc.
pub trait BlockFilter {
    fn stops(&self, block: BlockId) -> bool;
}

/// Stops on any non-transparent solid block. Most common case.
pub struct OpaqueOnlyFilter;
impl BlockFilter for OpaqueOnlyFilter {
    fn stops(&self, block: BlockId) -> bool {
        block.is_solid() && !block.is_transparent()
    }
}

/// Stops on any solid block (including transparent ones — leaves, glass,
/// crystals). Right for placement: you can target leaves to place against.
pub struct AnySolidFilter;
impl BlockFilter for AnySolidFilter {
    fn stops(&self, block: BlockId) -> bool {
        block.is_solid()
    }
}

/// Stops on any non-air block. Right for fluids — water flows until it hits
/// something to displace or rest against.
pub struct NonAirFilter;
impl BlockFilter for NonAirFilter {
    fn stops(&self, block: BlockId) -> bool {
        !block.is_air()
    }
}

/// Cast a ray against the world. Returns the first hit, or None if the ray
/// reaches `max_distance` without hitting anything matching the filter.
pub fn raycast(
    query: &RayQuery,
    world: &dyn BlockSampler,
    filter: &dyn BlockFilter,
) -> Option<RayHit> {
    // Normalize the direction.
    let dir_len = (query.direction[0] * query.direction[0]
        + query.direction[1] * query.direction[1]
        + query.direction[2] * query.direction[2]).sqrt();
    if dir_len < 1e-9 { return None; }
    let dir = [
        query.direction[0] / dir_len,
        query.direction[1] / dir_len,
        query.direction[2] / dir_len,
    ];

    // Amanatides-Woo grid traversal setup.
    let mut block = [
        query.origin[0].floor() as i32,
        query.origin[1].floor() as i32,
        query.origin[2].floor() as i32,
    ];

    // Step direction per axis.
    let step = [
        if dir[0] > 0.0 {  1 } else if dir[0] < 0.0 { -1 } else { 0 },
        if dir[1] > 0.0 {  1 } else if dir[1] < 0.0 { -1 } else { 0 },
        if dir[2] > 0.0 {  1 } else if dir[2] < 0.0 { -1 } else { 0 },
    ];

    // Compute t value to advance to next grid line on each axis.
    // tMax[i] = distance from origin to first grid line along axis i.
    // tDelta[i] = distance between grid lines along axis i = 1 / |dir[i]|.
    let next_boundary = |axis: usize| -> f32 {
        if step[axis] > 0 {
            (block[axis] + 1) as f32
        } else if step[axis] < 0 {
            block[axis] as f32
        } else {
            // Direction is zero on this axis — we'll never hit a boundary;
            // tMax is infinity.
            return f32::INFINITY;
        }
    };
    let mut t_max = [
        if dir[0] != 0.0 { (next_boundary(0) - query.origin[0]) / dir[0] } else { f32::INFINITY },
        if dir[1] != 0.0 { (next_boundary(1) - query.origin[1]) / dir[1] } else { f32::INFINITY },
        if dir[2] != 0.0 { (next_boundary(2) - query.origin[2]) / dir[2] } else { f32::INFINITY },
    ];
    let t_delta = [
        if dir[0] != 0.0 { (1.0_f32 / dir[0]).abs() } else { f32::INFINITY },
        if dir[1] != 0.0 { (1.0_f32 / dir[1]).abs() } else { f32::INFINITY },
        if dir[2] != 0.0 { (1.0_f32 / dir[2]).abs() } else { f32::INFINITY },
    ];

    // Check the starting cell first (in case the ray begins inside a solid).
    if let Some(b) = world.block_at(block[0], block[1], block[2]) {
        if filter.stops(b) {
            return Some(RayHit {
                block,
                block_id: b,
                face: BlockFace::PosY, // arbitrary — origin is inside the block
                distance: 0.0,
                point: query.origin,
            });
        }
    }

    // t_total tracks distance from origin to the current cell boundary;
    // it's overwritten on every loop iteration before being read.
    let mut t_total: f32;

    // Step until we exceed max_distance.
    loop {
        // Advance to the closest next axis boundary.
        let axis = if t_max[0] < t_max[1] && t_max[0] < t_max[2] {
            0
        } else if t_max[1] < t_max[2] {
            1
        } else {
            2
        };
        t_total = t_max[axis];
        if t_total > query.max_distance { return None; }
        block[axis] += step[axis];

        // The face crossed is the one we just stepped through, opposite to step direction.
        let face = match (axis, step[axis]) {
            (0,  1) => BlockFace::NegX,
            (0, -1) => BlockFace::PosX,
            (1,  1) => BlockFace::NegY,
            (1, -1) => BlockFace::PosY,
            (2,  1) => BlockFace::NegZ,
            (2, -1) => BlockFace::PosZ,
            _ => unreachable!(),
        };

        if let Some(b) = world.block_at(block[0], block[1], block[2]) {
            if filter.stops(b) {
                let point = [
                    query.origin[0] + dir[0] * t_total,
                    query.origin[1] + dir[1] * t_total,
                    query.origin[2] + dir[2] * t_total,
                ];
                return Some(RayHit {
                    block,
                    block_id: b,
                    face,
                    distance: t_total,
                    point,
                });
            }
        }

        t_max[axis] += t_delta[axis];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct MapWorld(HashMap<(i32, i32, i32), BlockId>);
    impl MapWorld {
        fn new() -> Self { Self(HashMap::new()) }
        fn set(&mut self, x: i32, y: i32, z: i32, b: BlockId) { self.0.insert((x,y,z), b); }
    }
    impl BlockSampler for MapWorld {
        fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
            Some(self.0.get(&(x,y,z)).copied().unwrap_or(BlockId::Air))
        }
    }

    #[test]
    fn ray_through_empty_returns_none() {
        let world = MapWorld::new();
        let hit = raycast(&RayQuery {
            origin: [0.5, 0.5, 0.5],
            direction: [1.0, 0.0, 0.0],
            max_distance: 100.0,
            kind: RayKind::PlayerVision,
        }, &world, &OpaqueOnlyFilter);
        assert!(hit.is_none());
    }

    #[test]
    fn ray_hits_wall_along_x() {
        let mut world = MapWorld::new();
        world.set(5, 0, 0, BlockId::Stone);
        let hit = raycast(&RayQuery {
            origin: [0.5, 0.5, 0.5],
            direction: [1.0, 0.0, 0.0],
            max_distance: 10.0,
            kind: RayKind::PlayerVision,
        }, &world, &OpaqueOnlyFilter).unwrap();
        assert_eq!(hit.block, [5, 0, 0]);
        assert_eq!(hit.face, BlockFace::NegX, "hit should be on -X face (the side ray crossed)");
        // Origin x=0.5, target block starts at x=5, so distance = 4.5.
        assert!((hit.distance - 4.5).abs() < 1e-3, "distance was {}", hit.distance);
    }

    #[test]
    fn ray_respects_max_distance() {
        let mut world = MapWorld::new();
        world.set(20, 0, 0, BlockId::Stone);
        let hit = raycast(&RayQuery {
            origin: [0.5, 0.5, 0.5],
            direction: [1.0, 0.0, 0.0],
            max_distance: 5.0,
            kind: RayKind::PlayerVision,
        }, &world, &OpaqueOnlyFilter);
        assert!(hit.is_none(), "ray should not reach block at x=20 within distance 5");
    }

    #[test]
    fn ray_hits_along_y_axis() {
        let mut world = MapWorld::new();
        world.set(0, 10, 0, BlockId::Stone);
        let hit = raycast(&RayQuery {
            origin: [0.5, 0.5, 0.5],
            direction: [0.0, 1.0, 0.0],
            max_distance: 20.0,
            kind: RayKind::PlayerVision,
        }, &world, &OpaqueOnlyFilter).unwrap();
        assert_eq!(hit.block, [0, 10, 0]);
        assert_eq!(hit.face, BlockFace::NegY);
    }

    #[test]
    fn ray_hits_along_diagonal() {
        let mut world = MapWorld::new();
        world.set(3, 3, 3, BlockId::Stone);
        let hit = raycast(&RayQuery {
            origin: [0.5, 0.5, 0.5],
            direction: [1.0, 1.0, 1.0],
            max_distance: 20.0,
            kind: RayKind::PlayerVision,
        }, &world, &OpaqueOnlyFilter).unwrap();
        assert_eq!(hit.block, [3, 3, 3]);
    }

    #[test]
    fn opaque_filter_passes_through_glass_air() {
        let mut world = MapWorld::new();
        world.set(2, 0, 0, BlockId::OakLeaves); // transparent (in our blocks model)
        world.set(5, 0, 0, BlockId::Stone);
        let hit = raycast(&RayQuery {
            origin: [0.5, 0.5, 0.5],
            direction: [1.0, 0.0, 0.0],
            max_distance: 10.0,
            kind: RayKind::PlayerVision,
        }, &world, &OpaqueOnlyFilter).unwrap();
        // Should pass through leaves to stone.
        assert_eq!(hit.block, [5, 0, 0]);
    }

    #[test]
    fn any_solid_filter_stops_at_glass() {
        let mut world = MapWorld::new();
        world.set(2, 0, 0, BlockId::OakLeaves);
        world.set(5, 0, 0, BlockId::Stone);
        let hit = raycast(&RayQuery {
            origin: [0.5, 0.5, 0.5],
            direction: [1.0, 0.0, 0.0],
            max_distance: 10.0,
            kind: RayKind::PlayerVision,
        }, &world, &AnySolidFilter).unwrap();
        // Should stop at leaves.
        assert_eq!(hit.block, [2, 0, 0]);
    }

    #[test]
    fn neighbor_face_for_placement() {
        // The block adjacent to a +Y face of [3,4,5] is [3,5,5].
        assert_eq!(BlockFace::PosY.neighbor([3,4,5]), [3,5,5]);
        assert_eq!(BlockFace::NegX.neighbor([3,4,5]), [2,4,5]);
        assert_eq!(BlockFace::PosZ.neighbor([3,4,5]), [3,4,6]);
    }
}
