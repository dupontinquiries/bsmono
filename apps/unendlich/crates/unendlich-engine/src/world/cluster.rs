//! Cluster identity: emergent block groupings under connectivity rules.
//!
//! A "cluster" is a set of blocks that the engine treats as one thing
//! under some rule. Examples:
//! - A vertical column of log blocks → structural cluster (a tree trunk)
//! - A region of contiguous sand blocks → granular cluster (a sand pile)
//! - A heated zone around a lava block → thermal cluster
//!
//! Clusters are NOT pre-defined entities. They're computed on demand by
//! flood-filling from a seed block under a connectivity rule, with a
//! bounds cap that prevents pathological cases (a single connected
//! component of bedrock must not consume hours).
//!
//! ## Identity
//!
//! Each cluster is identified by `ClusterId { bounds_min, content_hash }`.
//! The content hash is XOR-combined per-block tuple hashes, so any
//! flood-fill traversal order produces the same final hash. This means:
//! - Same blocks at same positions → same ClusterId regardless of who
//!   computed it or when
//! - Cluster identity survives reload (deterministic from world state)
//! - Cluster identity survives parallel computation (multiple workers
//!   computing the same cluster get the same id)
//!
//! ## Two size tiers
//!
//! Clusters come in two cap sizes for two different purposes:
//!
//! - **Physics-bound** (≤16³ blocks): drives per-tick simulation. Small
//!   enough that even the largest fast-changing thing (a falling tree
//!   trunk, a slumping sand pile) stays bounded in tick time.
//! - **Mesh-bound** (≤64³ blocks): drives visual identity (texture
//!   coherence, climate-averaged tinting). Computed rarely; doesn't
//!   participate in per-tick physics.
//!
//! Both tiers can coexist over the same blocks. A 30-block-tall tree
//! is one mesh-bound cluster (consistent climate-driven character) and,
//! during a fall, also one physics-bound cluster (the moving rigid
//! piece, capped at 16 blocks of relevant motion).
//!
//! ## Cross-chunk clusters
//!
//! Clusters can span chunk boundaries. We use **per-chunk slices with
//! reconciliation** (not centralised ownership): each chunk stores its
//! local portion of any cluster touching it. The ChunkManager has a
//! reconciliation pass that walks adjacent chunk pairs and merges
//! shared cluster slices. This design extends naturally to multiplayer
//! (server reconciles clients' versions of cross-chunk clusters).
//!
//! ## Determinism
//!
//! Every step is deterministic given identical inputs:
//! - Flood-fill traversal: canonical Z-order from the seed block
//! - Hash combining: XOR (commutative + associative)
//! - Tie-breaking: lower-index ChunkCoord wins for cross-chunk merges
//!
//! Two clients running the same simulation against the same world
//! state produce identical cluster ids. This is the multiplayer-prep
//! property we keep protecting.

use crate::world::blocks::BlockId;
use crate::world::chunk::Chunk;

/// Maximum bounding-box size for a physics-bound cluster, per axis.
/// A tree trunk falling, a sand pile slumping, a cliff fragment
/// breaking off — each must fit in this bound. If a connectivity walk
/// would exceed this, treat the cluster as "anchored" (connected to
/// the implicit bedrock cluster, hence stable).
pub const PHYSICS_CLUSTER_MAX: i32 = 16;

/// Maximum bounding-box size for a mesh-bound cluster, per axis.
/// Identifies coherent visual features: a long cliff face, a tall
/// tree, a wide dune. Used for climate-averaged tinting and
/// "what kind of feature is this" queries. Larger than physics
/// because it doesn't participate in per-tick simulation.
pub const MESH_CLUSTER_MAX: i32 = 64;

/// Maximum bounding-box size for a tree-growth cluster, per axis.
/// Trees can grow large via the lifecycle handler's TreeGrowth
/// sampler, and we measure their size by cluster diameter
/// (rotation-invariant). Cap chosen to give 100-block-diameter
/// trees comfortable headroom. Larger than mesh because trees are
/// the largest naturally-grown structures we expect.
pub const TREE_CLUSTER_MAX: i32 = 128;

/// A cluster's identity. Content-addressed: same blocks at same
/// positions produce the same `ClusterId` regardless of who computed it.
///
/// `bounds_min` is the minimum corner of the cluster's bounding box in
/// world coordinates. Used as a spatial anchor for queries like "what
/// cluster is at (x, y, z)?" — start by looking up clusters whose
/// `bounds_min ≤ (x, y, z)`.
///
/// `content_hash` is an XOR-combined hash of every block's
/// `(relative_position, block_id, instance_state)` tuple. XOR makes
/// it order-independent, so any flood-fill traversal produces the same
/// hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClusterId {
    pub bounds_min: [i32; 3],
    pub content_hash: u64,
}

impl ClusterId {
    /// The "anchored" cluster: blocks connected to bedrock-implicit
    /// space via a path that exceeds the cluster bounds cap. This is
    /// shared by every block that's effectively "part of the world
    /// foundation" — they don't fall, don't move, don't have their
    /// own cluster identity.
    ///
    /// Querying `cluster_at` for a block in the anchored set returns
    /// `Cluster { id: ANCHORED, .. }`.
    pub const ANCHORED: ClusterId = ClusterId {
        bounds_min: [i32::MIN, i32::MIN, i32::MIN],
        content_hash: 0,
    };

    /// Whether this id represents the implicit anchored cluster.
    #[inline]
    pub fn is_anchored(&self) -> bool {
        *self == Self::ANCHORED
    }
}

/// The connectivity rule that defines what blocks belong to the same
/// cluster. Rules are passed to `cluster_at` to flood-fill under
/// different criteria.
pub trait ClusterRule {
    /// Should two adjacent blocks be considered part of the same cluster?
    /// `from` is the block we're walking from; `to` is the candidate.
    fn connects(&self, from: BlockId, to: BlockId) -> bool;

    /// Maximum size for this rule's clusters, per axis. If the flood-fill
    /// would exceed this in any axis, the cluster is treated as anchored.
    fn max_axis(&self) -> i32;
}

/// Structural connectivity: same-material solid blocks adjacent with
/// no air break between. Used for trees, cliff overhangs, player
/// structures. Capped at `PHYSICS_CLUSTER_MAX` because structural
/// clusters can move (fall, fracture) and that wants tight bounds.
#[derive(Debug, Clone, Copy)]
pub struct StructuralRule;

impl ClusterRule for StructuralRule {
    #[inline]
    fn connects(&self, from: BlockId, to: BlockId) -> bool {
        // Both must be solid AND same material. Air, fluids, and
        // soft sub-voxel blocks don't form structural clusters.
        if from == BlockId::Air || to == BlockId::Air { return false; }
        if from != to { return false; }
        // Soft natural materials don't form rigid structural clusters
        // (sand, gravel) — they're granular, handled by GranularRule.
        // (Granular rule isn't implemented yet; this is forward-looking.)
        match from {
            BlockId::Sand | BlockId::Gravel => false,
            _ => true,
        }
    }

    #[inline]
    fn max_axis(&self) -> i32 { PHYSICS_CLUSTER_MAX }
}

/// Mesh-bound connectivity: same-category materials connected, with a
/// larger size cap. Used to identify coherent visual features for
/// texture and climate sampling. Less strict than structural — for
/// example, all "stone variants" might cluster together for tinting
/// purposes even though they wouldn't fall as one rigid piece.
///
/// For now, MeshBoundRule uses the same connects() logic as
/// StructuralRule but with the larger cap. We can relax connects()
/// later when actual visual clustering needs it (e.g. for "this
/// whole stone-and-cobblestone cliff face is one feature").
#[derive(Debug, Clone, Copy)]
pub struct MeshBoundRule;

impl ClusterRule for MeshBoundRule {
    #[inline]
    fn connects(&self, from: BlockId, to: BlockId) -> bool {
        StructuralRule.connects(from, to)
    }
    #[inline]
    fn max_axis(&self) -> i32 { MESH_CLUSTER_MAX }
}

/// Tree-growth connectivity: same-species wood blocks (OakLog connects
/// to OakLog, BirchLog to BirchLog, but no cross-species connection).
/// Used by the lifecycle handler's TreeGrowth pass to compute a
/// candidate tree's current diameter so growth can be capped.
///
/// Leaves are NOT included in the cluster — only the woody structure.
/// This means a tree's diameter is the extent of its trunk + branches,
/// not its canopy. We pick this because growth events convert leaves
/// to wood (extending the trunk/branches), so the cap should bound
/// the woody form, not the inflated leafy silhouette.
#[derive(Debug, Clone, Copy)]
pub struct TreeRule;

impl ClusterRule for TreeRule {
    #[inline]
    fn connects(&self, from: BlockId, to: BlockId) -> bool {
        match (from, to) {
            (BlockId::OakLog,   BlockId::OakLog)   => true,
            (BlockId::BirchLog, BlockId::BirchLog) => true,
            _ => false,
        }
    }
    #[inline]
    fn max_axis(&self) -> i32 { TREE_CLUSTER_MAX }
}

/// A computed cluster. Holds the identity, the bounds, and the set of
/// block positions (in world coordinates) that belong to it.
///
/// Member positions are stored as a sorted `Vec<[i32; 3]>` in canonical
/// Z-order (Morton-like). Sorted-and-canonical means iteration is
/// deterministic, which the hash relies on for stable identity.
#[derive(Debug, Clone)]
pub struct Cluster {
    pub id: ClusterId,
    pub bounds_min: [i32; 3],
    pub bounds_max: [i32; 3],
    /// World-coordinate positions of every block in this cluster.
    /// Sorted in canonical order (lexicographic by [y, z, x] — y first
    /// because pressure usually propagates vertically, so this
    /// ordering tends to be cache-friendly for typical queries).
    pub members: Vec<[i32; 3]>,
}

impl Cluster {
    /// The implicit "anchored" cluster, returned when a flood-fill
    /// exceeds the rule's bounds cap. This represents "you're connected
    /// to bedrock, you don't move."
    pub fn anchored() -> Self {
        Self {
            id: ClusterId::ANCHORED,
            bounds_min: [i32::MIN; 3],
            bounds_max: [i32::MAX; 3],
            members: Vec::new(),
        }
    }

    /// Number of blocks in the cluster (excluding the anchored case,
    /// where members is empty by convention).
    #[inline]
    pub fn size(&self) -> usize { self.members.len() }

    /// Largest Chebyshev distance between any two members of this
    /// cluster. The "size" measure that's rotation-invariant: a tall
    /// thin tree and a wide squat tree of the same diameter are
    /// considered the same size, regardless of orientation.
    ///
    /// Cost: O(N²) where N = cluster size. For N up to a few thousand
    /// this is fast (an N=1000 cluster pays 1e6 distance evals,
    /// each a few cycles). For very large clusters this is too
    /// expensive and we'd want a smarter algorithm (rotating-calipers
    /// or convex-hull-then-diameter), but real trees won't exceed
    /// a few hundred members.
    ///
    /// Returns 0 for empty clusters (the anchored case).
    pub fn diameter(&self) -> i32 {
        let n = self.members.len();
        if n < 2 { return 0; }
        let mut max_d: i32 = 0;
        for i in 0..n {
            for j in (i+1)..n {
                let a = self.members[i];
                let b = self.members[j];
                // Chebyshev: max of |dx|, |dy|, |dz|.
                let d = (a[0] - b[0]).abs()
                    .max((a[1] - b[1]).abs())
                    .max((a[2] - b[2]).abs());
                if d > max_d { max_d = d; }
            }
        }
        max_d
    }
}

/// Hash a single block's (relative_position, block_id) into a u64 for
/// XOR-combining. Uses a simple multiplicative hash — fast and
/// deterministic across architectures (no std hasher randomness).
#[inline]
fn hash_block_tuple(rel_pos: [i32; 3], block: BlockId) -> u64 {
    let x = rel_pos[0] as i64 as u64;
    let y = rel_pos[1] as i64 as u64;
    let z = rel_pos[2] as i64 as u64;
    let b = block as u64;
    let h = x.wrapping_mul(73856093u64)
        ^ y.wrapping_mul(19349663u64)
        ^ z.wrapping_mul(83492791u64)
        ^ b.wrapping_mul(2654435761u64);
    // Final bit mixing — tries to avoid clustered hash values for
    // close-by positions, which would cause hash collisions for
    // structurally-similar clusters.
    let h = h ^ (h >> 33);
    let h = h.wrapping_mul(0xff51afd7ed558ccdu64);
    let h = h ^ (h >> 33);
    let h = h.wrapping_mul(0xc4ceb9fe1a85ec53u64);
    h ^ (h >> 33)
}

/// Compare two positions in canonical lexicographic order. We use
/// (y, z, x) so vertical features (trees, columns) iterate naturally
/// from bottom to top.
#[inline]
fn pos_cmp(a: &[i32; 3], b: &[i32; 3]) -> std::cmp::Ordering {
    a[1].cmp(&b[1])
        .then_with(|| a[2].cmp(&b[2]))
        .then_with(|| a[0].cmp(&b[0]))
}

/// World-position-to-chunk lookup. Caller-supplied so this module
/// doesn't depend on any specific chunk container — works with both
/// `&Chunk` directly and with multi-chunk maps.
pub trait BlockSource {
    /// Return the block id at world position, or None if the chunk
    /// containing that position isn't loaded.
    fn block_at(&self, wx: i32, wy: i32, wz: i32) -> Option<BlockId>;
}

/// Trivial single-chunk BlockSource. Returns None for any position
/// outside this one chunk's bounds.
pub struct SingleChunkSource<'a> {
    pub chunk: &'a Chunk,
}

impl<'a> BlockSource for SingleChunkSource<'a> {
    fn block_at(&self, wx: i32, wy: i32, wz: i32) -> Option<BlockId> {
        use crate::constants::CHUNK_SIZE;
        let (ox, oy, oz) = self.chunk.coord.block_origin();
        let lx = wx - ox;
        let ly = wy - oy;
        let lz = wz - oz;
        if lx < 0 || lx >= CHUNK_SIZE
            || ly < 0 || ly >= CHUNK_SIZE
            || lz < 0 || lz >= CHUNK_SIZE
        {
            return None;
        }
        let idx = Chunk::block_index(lx, ly, lz);
        Some(self.chunk.blocks[idx])
    }
}

/// Multi-chunk BlockSource backed by a function the caller supplies.
/// Use this when a cluster might span chunk boundaries: provide a
/// closure that knows how to look up chunks (e.g. via DashMap,
/// Arc<RwLock<...>>, or any other container).
///
/// The closure takes a chunk coordinate and returns Some(&Chunk) if
/// loaded, None otherwise. cluster_at uses None as a hard boundary
/// (treats unloaded space like air for connectivity purposes).
pub struct MultiChunkSource<F>
where
    F: Fn(crate::world::chunk::ChunkCoord) -> Option<std::sync::Arc<Chunk>>,
{
    pub get_chunk: F,
}

impl<F> BlockSource for MultiChunkSource<F>
where
    F: Fn(crate::world::chunk::ChunkCoord) -> Option<std::sync::Arc<Chunk>>,
{
    fn block_at(&self, wx: i32, wy: i32, wz: i32) -> Option<BlockId> {
        use crate::constants::CHUNK_SIZE;
        use crate::world::chunk::ChunkCoord;
        let cx = wx.div_euclid(CHUNK_SIZE);
        let cy = wy.div_euclid(CHUNK_SIZE);
        let cz = wz.div_euclid(CHUNK_SIZE);
        let lx = wx.rem_euclid(CHUNK_SIZE);
        let ly = wy.rem_euclid(CHUNK_SIZE);
        let lz = wz.rem_euclid(CHUNK_SIZE);
        let chunk = (self.get_chunk)(ChunkCoord::new(cx, cy, cz))?;
        let idx = Chunk::block_index(lx, ly, lz);
        Some(chunk.blocks[idx])
    }
}

/// Compute the cluster containing `(wx, wy, wz)` under `rule`.
///
/// Returns `Cluster::anchored()` if:
/// - The seed block is air, fluid, or otherwise not part of any
///   cluster under this rule.
/// - The flood-fill exceeds the rule's bounds cap (cluster is too
///   large to be a movable thing — it's part of the static world).
///
/// Returns a populated `Cluster` otherwise. Members are sorted in
/// canonical (y, z, x) order. ID's content_hash is XOR-combined over
/// all members, so two queries that find the same blocks produce
/// identical IDs regardless of flood-fill traversal order.
pub fn cluster_at<R: ClusterRule, S: BlockSource>(
    wx: i32, wy: i32, wz: i32,
    rule: &R,
    source: &S,
) -> Cluster {
    let seed_block = match source.block_at(wx, wy, wz) {
        Some(b) => b,
        None => return Cluster::anchored(),
    };

    // Seed self-test: a block must "connect to itself" under the rule
    // to be a valid cluster seed. Air doesn't (StructuralRule rejects
    // air-air pair); air seeds therefore return anchored.
    if !rule.connects(seed_block, seed_block) {
        return Cluster::anchored();
    }

    let max_axis = rule.max_axis();
    let max_volume = (max_axis as usize).pow(3);

    // BFS / flood-fill from the seed. We use a Vec as a stack +
    // a HashSet for visited tracking. BFS order is NOT canonical —
    // that's fine because we sort + hash at the end, which is
    // order-independent.
    let mut visited: std::collections::HashSet<[i32; 3]> =
        std::collections::HashSet::new();
    let mut stack: Vec<[i32; 3]> = vec![[wx, wy, wz]];
    let mut members: Vec<[i32; 3]> = Vec::new();
    let mut bounds_min = [wx, wy, wz];
    let mut bounds_max = [wx, wy, wz];

    while let Some(pos) = stack.pop() {
        if !visited.insert(pos) { continue; }

        // Update bounds.
        for i in 0..3 {
            if pos[i] < bounds_min[i] { bounds_min[i] = pos[i]; }
            if pos[i] > bounds_max[i] { bounds_max[i] = pos[i]; }
        }
        // Bounds-cap check: if any axis-extent exceeds max, this
        // cluster is too large — treat as anchored.
        for i in 0..3 {
            if bounds_max[i] - bounds_min[i] >= max_axis {
                return Cluster::anchored();
            }
        }
        // Volume cap as a defensive measure (catches pathological
        // sparse-but-spread cases that stay within bounds_max - min
        // but visit too many cells).
        if members.len() >= max_volume {
            return Cluster::anchored();
        }

        members.push(pos);

        // 6-neighbor expansion. Diagonal-connected clusters are not
        // structural (a diagonal stack of stones isn't a single rigid
        // piece). 6-neighbor matches what physics engines call
        // "face-connected." Future granular/fluid rules might want
        // 26-neighbor; that becomes a rule property.
        let from_block = match source.block_at(pos[0], pos[1], pos[2]) {
            Some(b) => b,
            None => continue,
        };
        const NEIGHBORS: [[i32; 3]; 6] = [
            [1, 0, 0], [-1, 0, 0],
            [0, 1, 0], [0, -1, 0],
            [0, 0, 1], [0, 0, -1],
        ];
        for d in NEIGHBORS.iter() {
            let np = [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
            if visited.contains(&np) { continue; }
            let to_block = match source.block_at(np[0], np[1], np[2]) {
                Some(b) => b,
                None => continue, // unloaded chunk; treat as boundary
            };
            if rule.connects(from_block, to_block) {
                stack.push(np);
            }
        }
    }

    // Sort members in canonical order and compute content_hash.
    members.sort_by(pos_cmp);

    let mut content_hash: u64 = 0;
    for &m in &members {
        let rel = [m[0] - bounds_min[0], m[1] - bounds_min[1], m[2] - bounds_min[2]];
        let block = source.block_at(m[0], m[1], m[2]).unwrap_or(BlockId::Air);
        content_hash ^= hash_block_tuple(rel, block);
    }

    let id = ClusterId { bounds_min, content_hash };
    Cluster { id, bounds_min, bounds_max, members }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::CHUNK_SIZE;
    use crate::world::chunk::ChunkCoord;

    fn make_chunk_with(blocks: &[(i32, i32, i32, BlockId)]) -> Chunk {
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for &(x, y, z, b) in blocks {
            assert!(x >= 0 && x < CHUNK_SIZE && y >= 0 && y < CHUNK_SIZE && z >= 0 && z < CHUNK_SIZE,
                "test block ({}, {}, {}) out of single-chunk bounds", x, y, z);
            let idx = Chunk::block_index(x, y, z);
            c.blocks[idx] = b;
        }
        c
    }

    #[test]
    fn empty_world_returns_anchored() {
        let chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let source = SingleChunkSource { chunk: &chunk };
        let cluster = cluster_at(5, 5, 5, &StructuralRule, &source);
        assert!(cluster.id.is_anchored());
    }

    #[test]
    fn isolated_stone_block_is_its_own_cluster() {
        let chunk = make_chunk_with(&[(5, 5, 5, BlockId::Stone)]);
        let source = SingleChunkSource { chunk: &chunk };
        let cluster = cluster_at(5, 5, 5, &StructuralRule, &source);
        assert!(!cluster.id.is_anchored());
        assert_eq!(cluster.size(), 1);
        assert_eq!(cluster.members[0], [5, 5, 5]);
    }

    #[test]
    fn three_adjacent_stones_form_one_cluster() {
        let chunk = make_chunk_with(&[
            (5, 5, 5, BlockId::Stone),
            (5, 6, 5, BlockId::Stone),
            (5, 7, 5, BlockId::Stone),
        ]);
        let source = SingleChunkSource { chunk: &chunk };
        let cluster = cluster_at(5, 5, 5, &StructuralRule, &source);
        assert_eq!(cluster.size(), 3);
        // Querying from any of the three should give the same cluster id.
        let from_top = cluster_at(5, 7, 5, &StructuralRule, &source);
        let from_mid = cluster_at(5, 6, 5, &StructuralRule, &source);
        assert_eq!(cluster.id, from_top.id);
        assert_eq!(cluster.id, from_mid.id);
    }

    #[test]
    fn different_materials_dont_cluster() {
        let chunk = make_chunk_with(&[
            (5, 5, 5, BlockId::Stone),
            (5, 6, 5, BlockId::OakLog),
        ]);
        let source = SingleChunkSource { chunk: &chunk };
        let stone_cluster = cluster_at(5, 5, 5, &StructuralRule, &source);
        let log_cluster = cluster_at(5, 6, 5, &StructuralRule, &source);
        assert_eq!(stone_cluster.size(), 1);
        assert_eq!(log_cluster.size(), 1);
        assert_ne!(stone_cluster.id, log_cluster.id);
    }

    #[test]
    fn diagonal_blocks_dont_cluster() {
        // Stone at (5,5,5) and (6,6,5) are diagonal — touching at
        // an edge, not a face. Structural rule uses 6-neighbor.
        let chunk = make_chunk_with(&[
            (5, 5, 5, BlockId::Stone),
            (6, 6, 5, BlockId::Stone),
        ]);
        let source = SingleChunkSource { chunk: &chunk };
        let a = cluster_at(5, 5, 5, &StructuralRule, &source);
        let b = cluster_at(6, 6, 5, &StructuralRule, &source);
        assert_eq!(a.size(), 1);
        assert_eq!(b.size(), 1);
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn sand_and_gravel_dont_form_structural_clusters() {
        let chunk = make_chunk_with(&[
            (5, 5, 5, BlockId::Sand),
            (5, 6, 5, BlockId::Sand),
        ]);
        let source = SingleChunkSource { chunk: &chunk };
        let cluster = cluster_at(5, 5, 5, &StructuralRule, &source);
        // Sand is granular; structural rule rejects it. Anchored.
        assert!(cluster.id.is_anchored());
    }

    #[test]
    fn cluster_id_is_deterministic_across_runs() {
        let chunk = make_chunk_with(&[
            (5, 5, 5, BlockId::Stone),
            (5, 6, 5, BlockId::Stone),
            (6, 5, 5, BlockId::Stone),
        ]);
        let source = SingleChunkSource { chunk: &chunk };
        let id1 = cluster_at(5, 5, 5, &StructuralRule, &source).id;
        let id2 = cluster_at(5, 5, 5, &StructuralRule, &source).id;
        let id3 = cluster_at(6, 5, 5, &StructuralRule, &source).id;
        assert_eq!(id1, id2);
        assert_eq!(id1, id3);
    }

    #[test]
    fn cluster_id_changes_when_blocks_change() {
        let chunk_a = make_chunk_with(&[
            (5, 5, 5, BlockId::Stone),
            (5, 6, 5, BlockId::Stone),
        ]);
        let chunk_b = make_chunk_with(&[
            (5, 5, 5, BlockId::Stone),
            (5, 6, 5, BlockId::Stone),
            (5, 7, 5, BlockId::Stone),
        ]);
        let id_a = cluster_at(5, 5, 5, &StructuralRule, &SingleChunkSource { chunk: &chunk_a }).id;
        let id_b = cluster_at(5, 5, 5, &StructuralRule, &SingleChunkSource { chunk: &chunk_b }).id;
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn large_cluster_returns_anchored() {
        // Build a chunk filled with a contiguous stone slab > 16 blocks
        // in any axis (full 32×32×32 chunk). Should be anchored.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let idx = Chunk::block_index(x, y, z);
                    chunk.blocks[idx] = BlockId::Stone;
                }
            }
        }
        let source = SingleChunkSource { chunk: &chunk };
        let cluster = cluster_at(5, 5, 5, &StructuralRule, &source);
        assert!(cluster.id.is_anchored(),
            "32-cube stone slab should anchor (exceeds 16-axis cap)");
    }

    #[test]
    fn members_are_sorted_in_canonical_order() {
        // Build a small cluster, verify members come out sorted.
        let chunk = make_chunk_with(&[
            (5, 5, 5, BlockId::Stone),
            (5, 6, 5, BlockId::Stone),
            (6, 5, 5, BlockId::Stone),
            (5, 5, 6, BlockId::Stone),
        ]);
        let source = SingleChunkSource { chunk: &chunk };
        let cluster = cluster_at(5, 5, 5, &StructuralRule, &source);
        assert_eq!(cluster.size(), 4);
        for i in 1..cluster.members.len() {
            let prev = &cluster.members[i - 1];
            let curr = &cluster.members[i];
            assert!(pos_cmp(prev, curr) == std::cmp::Ordering::Less,
                "members not in canonical order: {:?} then {:?}", prev, curr);
        }
    }

    #[test]
    fn mesh_bound_rule_allows_larger_cluster() {
        // A 20-block tall stone column would anchor under StructuralRule
        // (16 cap) but should cluster under MeshBoundRule (64 cap).
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for y in 5..25 {
            let idx = Chunk::block_index(5, y, 5);
            chunk.blocks[idx] = BlockId::Stone;
        }
        let source = SingleChunkSource { chunk: &chunk };
        let structural = cluster_at(5, 5, 5, &StructuralRule, &source);
        let mesh = cluster_at(5, 5, 5, &MeshBoundRule, &source);
        assert!(structural.id.is_anchored(),
            "20-block column should anchor under structural (16-cap)");
        assert!(!mesh.id.is_anchored(),
            "20-block column should cluster under mesh-bound (64-cap)");
        assert_eq!(mesh.size(), 20);
    }

    #[test]
    fn anchored_cluster_id_is_constant() {
        let chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let source = SingleChunkSource { chunk: &chunk };
        // Two different positions both yielding anchored should have
        // the same anchored id.
        let a = cluster_at(0, 0, 0, &StructuralRule, &source);
        let b = cluster_at(15, 15, 15, &StructuralRule, &source);
        assert!(a.id.is_anchored());
        assert!(b.id.is_anchored());
        assert_eq!(a.id, b.id);
    }

    /// Multiplayer-prep determinism: same blocks, different module
    /// instances (simulated by creating two separate chunks with
    /// identical content), produce identical cluster ids.
    #[test]
    fn cluster_id_is_consistent_across_chunk_instances() {
        let chunk_a = make_chunk_with(&[
            (5, 5, 5, BlockId::Stone),
            (5, 6, 5, BlockId::Stone),
            (5, 7, 5, BlockId::Stone),
            (6, 7, 5, BlockId::Stone),
        ]);
        let chunk_b = make_chunk_with(&[
            (5, 5, 5, BlockId::Stone),
            (5, 6, 5, BlockId::Stone),
            (5, 7, 5, BlockId::Stone),
            (6, 7, 5, BlockId::Stone),
        ]);
        let id_a = cluster_at(5, 5, 5, &StructuralRule,
            &SingleChunkSource { chunk: &chunk_a }).id;
        let id_b = cluster_at(5, 5, 5, &StructuralRule,
            &SingleChunkSource { chunk: &chunk_b }).id;
        assert_eq!(id_a, id_b,
            "identical content in different chunk instances should produce same id");
    }

    /// XOR-based content hashing means flood-fill traversal order does
    /// NOT affect the resulting hash. Verify by querying from different
    /// seeds in the same cluster.
    #[test]
    fn content_hash_is_traversal_order_independent() {
        let chunk = make_chunk_with(&[
            (5, 5, 5, BlockId::Stone),
            (5, 6, 5, BlockId::Stone),
            (5, 7, 5, BlockId::Stone),
            (6, 6, 5, BlockId::Stone),
            (4, 6, 5, BlockId::Stone),
        ]);
        let source = SingleChunkSource { chunk: &chunk };
        let from_a = cluster_at(5, 5, 5, &StructuralRule, &source).id;
        let from_b = cluster_at(5, 7, 5, &StructuralRule, &source).id;
        let from_c = cluster_at(6, 6, 5, &StructuralRule, &source).id;
        let from_d = cluster_at(4, 6, 5, &StructuralRule, &source).id;
        assert_eq!(from_a, from_b);
        assert_eq!(from_a, from_c);
        assert_eq!(from_a, from_d);
    }

    /// MultiChunkSource lets a cluster span chunk boundaries.
    /// A stone column at the edge of one chunk extending into the
    /// next should be one cluster, querying from either side.
    #[test]
    fn multi_chunk_source_clusters_across_boundary() {
        use std::sync::Arc;
        // Chunk (0,0,0): stone at (31, 5, 5)
        let mut chunk_a = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let idx_a = Chunk::block_index(31, 5, 5);
        chunk_a.blocks[idx_a] = BlockId::Stone;
        // Chunk (1,0,0): stone at (0, 5, 5) — adjacent to chunk_a's (31,5,5)
        let mut chunk_b = Chunk::empty(ChunkCoord::new(1, 0, 0));
        let idx_b = Chunk::block_index(0, 5, 5);
        chunk_b.blocks[idx_b] = BlockId::Stone;

        let chunk_a = Arc::new(chunk_a);
        let chunk_b = Arc::new(chunk_b);
        let chunks: std::collections::HashMap<ChunkCoord, Arc<Chunk>> = [
            (ChunkCoord::new(0, 0, 0), Arc::clone(&chunk_a)),
            (ChunkCoord::new(1, 0, 0), Arc::clone(&chunk_b)),
        ].into_iter().collect();

        let source = MultiChunkSource {
            get_chunk: |c: ChunkCoord| chunks.get(&c).cloned(),
        };

        // Querying from chunk A's edge stone (world position (31, 5, 5)).
        let from_a = cluster_at(31, 5, 5, &StructuralRule, &source);
        // Querying from chunk B's edge stone (world position (32, 5, 5)).
        let from_b = cluster_at(32, 5, 5, &StructuralRule, &source);

        assert!(!from_a.id.is_anchored());
        assert!(!from_b.id.is_anchored());
        assert_eq!(from_a.id, from_b.id,
            "cross-chunk cluster should have one identity from either side");
        assert_eq!(from_a.size(), 2,
            "cluster should contain both blocks across the boundary");
    }

    // ─── Diameter & TreeRule tests ─────────────────────────────────────

    #[test]
    fn diameter_of_singleton_is_zero() {
        let c = Cluster {
            id: ClusterId::ANCHORED,
            bounds_min: [0; 3], bounds_max: [0; 3],
            members: vec![[5, 5, 5]],
        };
        assert_eq!(c.diameter(), 0);
    }

    #[test]
    fn diameter_of_empty_is_zero() {
        let c = Cluster::anchored();
        assert_eq!(c.diameter(), 0);
    }

    #[test]
    fn diameter_chebyshev_pairwise() {
        // 3 blocks: (0,0,0), (5,0,0), (0,3,0). Pairwise Chebyshev:
        // (0,0,0)-(5,0,0) → 5
        // (0,0,0)-(0,3,0) → 3
        // (5,0,0)-(0,3,0) → max(5, 3, 0) = 5
        // Diameter = 5.
        let c = Cluster {
            id: ClusterId::ANCHORED,
            bounds_min: [0; 3], bounds_max: [5, 3, 0],
            members: vec![[0, 0, 0], [5, 0, 0], [0, 3, 0]],
        };
        assert_eq!(c.diameter(), 5);
    }

    #[test]
    fn diameter_uses_chebyshev_not_euclidean() {
        // (0,0,0) to (3,4,0): Euclidean = 5, Chebyshev = 4.
        let c = Cluster {
            id: ClusterId::ANCHORED,
            bounds_min: [0; 3], bounds_max: [3, 4, 0],
            members: vec![[0, 0, 0], [3, 4, 0]],
        };
        assert_eq!(c.diameter(), 4, "Chebyshev = max(|dx|, |dy|, |dz|)");
    }

    #[test]
    fn tree_rule_connects_same_species() {
        // Two adjacent OakLog should form one cluster under TreeRule.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(5, 5, 5, BlockId::OakLog);
        chunk.set(5, 6, 5, BlockId::OakLog);
        let source = SingleChunkSource { chunk: &chunk };
        let cluster = cluster_at(5, 5, 5, &TreeRule, &source);
        assert_eq!(cluster.size(), 2);
        assert_eq!(cluster.diameter(), 1);
    }

    #[test]
    fn tree_rule_does_not_connect_different_species() {
        // OakLog touching BirchLog should NOT cluster under TreeRule.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(5, 5, 5, BlockId::OakLog);
        chunk.set(5, 6, 5, BlockId::BirchLog);
        let source = SingleChunkSource { chunk: &chunk };
        let oak_cluster = cluster_at(5, 5, 5, &TreeRule, &source);
        assert_eq!(oak_cluster.size(), 1, "different species shouldn't cluster");
        let birch_cluster = cluster_at(5, 6, 5, &TreeRule, &source);
        assert_eq!(birch_cluster.size(), 1);
        assert_ne!(oak_cluster.id, birch_cluster.id,
            "different species should have distinct cluster IDs");
    }

    #[test]
    fn tree_rule_excludes_leaves() {
        // OakLog with adjacent OakLeaves: only the log should cluster.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(5, 5, 5, BlockId::OakLog);
        chunk.set(5, 6, 5, BlockId::OakLeaves);
        chunk.set(6, 5, 5, BlockId::OakLeaves);
        let source = SingleChunkSource { chunk: &chunk };
        let cluster = cluster_at(5, 5, 5, &TreeRule, &source);
        assert_eq!(cluster.size(), 1, "leaves shouldn't be in the wood cluster");
    }

    #[test]
    fn tree_rule_clusters_realistic_tree() {
        // A small tree: 4-block trunk at (5, 5..9, 5).
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for y in 5..9 {
            chunk.set(5, y, 5, BlockId::OakLog);
        }
        let source = SingleChunkSource { chunk: &chunk };
        let cluster = cluster_at(5, 5, 5, &TreeRule, &source);
        assert_eq!(cluster.size(), 4);
        // Trunk goes from y=5 to y=8: diameter = 3 (Chebyshev).
        assert_eq!(cluster.diameter(), 3);
    }
}
