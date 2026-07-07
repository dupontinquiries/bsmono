//! Flat-buffer sparse octree for chunks.
//!
//! v0.24.23-octree-foundation. First foundation ship for the
//! GPU-resident-world architecture documented in
//! `architecture-v1.md §3.1`:
//!
//! > The world is GPU-resident as an octree. CPU is the orchestrator,
//! > edit author, and the small-sample read-back endpoint. The mesher
//! > samples the same octree the physics samples the same octree the
//! > renderer samples.
//!
//! This ship lands the **CPU side of the wire format**:
//!
//! 1. `OctreeNode` — 36-byte repr(C) struct laid out for GPU
//!    consumption (the GPU shader in v0.24.24+ will read this verbatim
//!    from an SSBO).
//! 2. `OctreeArray` — a flat array of nodes encoding one chunk's
//!    sparse octree. Sparse means: homogeneous 2×2×2 subtrees collapse
//!    to a single leaf node, so a chunk of pure air or pure stone
//!    encodes as one node, not 32,768.
//! 3. `OctreeArray::from_chunk` — convert a dense `Chunk` to its
//!    octree representation.
//! 4. `OctreeArray::sample(x, y, z)` — point-query a block at
//!    chunk-local coordinates. The CPU twin of the GPU shader's
//!    eventual `octree_sample` function.
//! 5. `OctreeArray::to_chunk` — round-trip back to dense (used by
//!    tests to prove the encoding is lossless; not on any hot path).
//!
//! What this ship does NOT do:
//!
//! - **No GPU shader yet.** That's v0.24.24. The data structure is
//!   designed so the GPU shader's traversal will be one descent loop
//!   indexing into a `readonly buffer Nodes { OctreeNode nodes[]; }`.
//! - **No meshing integration.** The CPU mesher still operates on
//!   `Chunk`. Future ships migrate the mesh path to consume
//!   `OctreeArray` directly so the conversion doesn't have to happen.
//! - **No edit overlay.** The CPU overlay buffer
//!   (`architecture-v1.md §3.2`) for pending/predicted edits is a
//!   separate ship. This one just covers canonical-state encoding.
//! - **No persistence format.** Save/load still writes dense Chunks.
//!   Octree serialization is filed as a "genuinely hard" open
//!   problem per the architecture doc; revisit when migration is
//!   further along.
//!
//! ## On-the-wire node format
//!
//! ```text
//! offset  size  field
//! ------  ----  -----
//!     0     4   packed_header (u32)
//!              bit 31:    is_leaf (1 = leaf, 0 = internal)
//!              bits 30..28: depth (0..=5; 5 = chunk-root, 0 = single block)
//!              bits 27..8:  reserved (must be 0)
//!              bits 7..0:   for leaves only: block_id (u8)
//!     4     4   last_modified_tick (u32)
//!              The max tick at which any leaf at/below this node was
//!              modified. Initialized to 0. Producers stamp ancestors
//!              via mark_path_modified / mark_region_modified.
//!              Consumers short-circuit via region_stable_since.
//!     8    32   children[8] (u32 × 8)
//!              For internal nodes: index of each octant child in
//!              the enclosing OctreeArray's node table.
//!              Octant order: child[i] has offsets
//!                dx = i & 1
//!                dy = (i >> 1) & 1
//!                dz = (i >> 2) & 1
//!              (i.e. the i-th bit of i indexes axis i — same
//!               convention as Morton codes / Z-order)
//!              For leaves: ignored (caller can skip the 32 bytes).
//! -------  --
//!    40    total
//! ```
//!
//! Size budget: a fully-dense chunk (32³ unique blocks, no collapse)
//! costs 37,449 × 40 ≈ 1.5 MB. A typical surface chunk after sparse
//! collapse is ~5K–10K nodes ≈ 200–400 KB. A chunk of pure air is 1
//! node ≈ 40 B.
//!
//! **The point of the octree is NOT byte compression** — for surface
//! chunks the octree is several times *larger* than dense `Vec<BlockId>`
//! storage. The point is **compute compression via hierarchical skip**:
//!
//! 1. Raycast, region read, physics queries answer in O(log depth) at
//!    homogeneous subtrees. A raycast through 256³ of pure air visits
//!    one octree node (a coarse leaf), not 16M voxel checks.
//!
//! 2. Cache-deferment via `last_modified_tick` lets consumers (mesher,
//!    raycast, item region reads) check "anything dirty since my last
//!    read?" in O(log depth) and skip the whole pipeline downstream
//!    when the answer is no.
//!
//! Both wins scale with world size, not chunk size. The octree pays
//! its bytes back when the engine runs many consumers reading many
//! regions across many ticks.
//!
//! ## Octant indexing
//!
//! Standard Morton/Z-order convention. Octant `i` (in `0..8`) has
//! local offset (dx, dy, dz) where:
//!   - dx = `i & 1`
//!   - dy = `(i >> 1) & 1`
//!   - dz = `(i >> 2) & 1`
//!
//! So octant 0 = (-x, -y, -z), octant 7 = (+x, +y, +z). This
//! convention is used identically in the build, sample, and (future)
//! GPU shader so all consumers agree on which child is which.

use crate::constants::{CHUNK_SIZE, CHUNK_VOLUME};
use crate::world::blocks::BlockId;
use crate::world::chunk::{Chunk, ChunkCoord};

/// Depth of a chunk's octree. CHUNK_SIZE = 32 = 2⁵, so depth 5.
/// Each depth level halves the cube edge: depth 5 = 32³, depth 4 =
/// 16³, ..., depth 0 = 1³ (single block).
pub const OCTREE_DEPTH: u32 = 5;

/// 40-byte flat-buffer node. `repr(C)` so the layout is identical on
/// CPU and GPU. The GPU shader will declare a matching SSBO struct.
///
/// **Why 40 bytes, not 36** (v0.24.23 second pass):
/// The original 36-byte design was just header + child indices. That
/// covers point queries (`sample`) but doesn't help consumers that
/// want to skip work when an octree subtree hasn't changed since
/// they last read it. The 4-byte `last_modified_tick` field is the
/// reverse-propagation handle for the deferment system documented
/// in `architecture-v1-addendum.md §A4` (deadline-driven scheduling
/// + speculation): when a leaf is edited, the edit walks up the
/// ancestor chain marking each ancestor with the current tick. A
/// downstream consumer (mesher, physics, raycast, item region-read)
/// can then short-circuit: "I last queried at tick T₀; this
/// ancestor's last_modified_tick is ≤ T₀; the entire subtree below
/// it is stable for me, skip it."
///
/// 40 bytes also aligns nicely for std430 SSBOs (multiple of 8) and
/// keeps node-indexed striding simple on the GPU.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OctreeNode {
    /// See module docs for bit layout. Use the helper constructors
    /// `OctreeNode::leaf` and `OctreeNode::internal` rather than
    /// poking the field directly.
    pub packed_header: u32,
    /// The largest tick value at which any leaf at or below this
    /// node was modified. Initialized to 0 by builders. The world's
    /// edit pipeline (future ship) propagates writes upward via
    /// `OctreeArray::mark_path_modified`.
    ///
    /// Consumers query `region_stable_since(aabb, my_last_tick)` —
    /// internally this descends the tree and short-circuits at any
    /// ancestor whose `last_modified_tick ≤ my_last_tick`. That's
    /// the "skip pass" property: O(log depth) check answers
    /// "anything under this region change since I last cared," and
    /// when the answer is "no" the consumer reuses cached output.
    pub last_modified_tick: u32,
    /// For internal nodes: indices into the enclosing array's node
    /// table, one per octant. For leaves: ignored.
    pub children: [u32; 8],
}

const FLAG_IS_LEAF: u32 = 1 << 31;
const SHIFT_DEPTH: u32 = 28;
const MASK_DEPTH: u32 = 0b111 << SHIFT_DEPTH;
const MASK_BLOCK_ID: u32 = 0xFF;

impl OctreeNode {
    /// Build a leaf node carrying a single block_id at the given
    /// depth. Depth tells consumers "this leaf represents a cube of
    /// size 2^depth blocks." A depth-5 leaf would be the entire chunk
    /// collapsed to one block; depth-0 is a single-block leaf.
    pub fn leaf(depth: u32, block: BlockId) -> Self {
        debug_assert!(depth <= OCTREE_DEPTH, "depth {} > OCTREE_DEPTH", depth);
        let raw = block as u8 as u32;
        OctreeNode {
            packed_header: FLAG_IS_LEAF
                | (depth << SHIFT_DEPTH)
                | (raw & MASK_BLOCK_ID),
            last_modified_tick: 0,
            children: [0; 8],
        }
    }

    /// Build an internal node referring to 8 child indices.
    pub fn internal(depth: u32, children: [u32; 8]) -> Self {
        debug_assert!(depth <= OCTREE_DEPTH, "depth {} > OCTREE_DEPTH", depth);
        debug_assert!(depth > 0, "internal node cannot be at depth 0");
        OctreeNode {
            packed_header: (depth << SHIFT_DEPTH),
            last_modified_tick: 0,
            children,
        }
    }

    pub fn is_leaf(self) -> bool {
        (self.packed_header & FLAG_IS_LEAF) != 0
    }

    pub fn depth(self) -> u32 {
        (self.packed_header & MASK_DEPTH) >> SHIFT_DEPTH
    }

    /// Returns the block_id encoded in this leaf. For internal nodes
    /// the return value is meaningless; gate on `is_leaf` first.
    pub fn leaf_block(self) -> BlockId {
        let raw = (self.packed_header & MASK_BLOCK_ID) as u8;
        // Round-trip through TryFrom; default to Air if a stale node
        // carries an unmapped id (defensive — shouldn't happen if
        // build/sample stay in sync).
        BlockId::try_from(raw).unwrap_or(BlockId::Air)
    }
}

// Compile-time assertion that the GPU-visible size matches what the
// docs claim. If padding or alignment changes the on-the-wire size,
// the GPU shader will silently disagree with the CPU encoder. This
// catches it at compile time.
const _: () = {
    assert!(
        std::mem::size_of::<OctreeNode>() == 40,
        "OctreeNode must be exactly 40 bytes for GPU consumption"
    );
};

/// Per-node byte size when laid out in a GPU SSBO. Exposed so other
/// modules (upload, bench) don't hardcode `40`.
pub const OCTREE_NODE_BYTES: usize = 40;

/// One chunk's worth of octree data, laid out as a flat array of
/// `OctreeNode`s. The root is **always at index 0**.
///
/// Building: use `from_chunk`. Querying: `sample`. Round-trip:
/// `to_chunk`.
#[derive(Debug, Clone)]
pub struct OctreeArray {
    /// The chunk coordinate this octree represents. Carried through
    /// from the source `Chunk` so consumers know which chunk this is
    /// without an out-of-band map. Set to `ChunkCoord::new(0, 0, 0)`
    /// by tests that don't care.
    pub coord: ChunkCoord,
    /// Flat node table. Node 0 is the root (covers the whole chunk).
    pub nodes: Vec<OctreeNode>,
}

impl OctreeArray {
    /// Construct an empty octree containing only an air-leaf root.
    /// Used by tests; production code goes through `from_chunk`.
    pub fn empty(coord: ChunkCoord) -> Self {
        OctreeArray {
            coord,
            nodes: vec![OctreeNode::leaf(OCTREE_DEPTH, BlockId::Air)],
        }
    }

    /// Build the sparse octree representation of a dense chunk.
    ///
    /// Algorithm: recursive subdivision from chunk-root down to
    /// depth 0. Each subdivision step builds its 8 children first;
    /// if all 8 turn out to be leaves with the same block_id, the
    /// 8 are discarded and the parent becomes a single leaf at one
    /// higher depth. This is the sparse-octree's "homogeneous
    /// subtree collapse" rule.
    ///
    /// Worst case (every cell unique): 37,449 nodes for a 32³
    /// chunk. Typical surface chunk: ~5K–10K nodes after collapse.
    /// Air-only or stone-only chunk: 1 node.
    pub fn from_chunk(chunk: &Chunk) -> Self {
        let mut nodes = Vec::with_capacity(1024);
        // Root covers the whole chunk: position (0,0,0), depth
        // OCTREE_DEPTH. The build_recursive call returns the index
        // of the root (which we expect to be the last node pushed).
        let root_idx = build_recursive(chunk, 0, 0, 0, OCTREE_DEPTH, &mut nodes);
        // Convention: root MUST be at index 0. If the recursion
        // didn't end with the root at index 0 (e.g., because of how
        // we pop-and-replace during collapse), rotate it there.
        if root_idx != 0 {
            // The root is by construction the LAST node pushed.
            // Move it to position 0 by swapping. Any internal nodes
            // that reference its old position must be updated, but
            // since root is the deepest-collapsed node, NO node
            // refers to it (it's the topmost) — so we can simply
            // swap nodes[0] and nodes[root_idx]. But we have to
            // rewrite any child indices pointing at index 0 to
            // point at root_idx's old index instead.
            let last = nodes.len() - 1;
            debug_assert_eq!(root_idx as usize, last,
                "root must be the last node pushed");
            nodes.swap(0, last);
            // Patch any child pointer that was pointing at 0 (the
            // old occupant) → now at `last`. And any pointer to
            // `last` → 0. Since we only swapped two slots, the
            // patch is constrained.
            for node in nodes.iter_mut() {
                if node.is_leaf() { continue; }
                for c in node.children.iter_mut() {
                    if *c == 0 {
                        *c = last as u32;
                    } else if *c == last as u32 {
                        *c = 0;
                    }
                }
            }
        }

        OctreeArray { coord: chunk.coord, nodes }
    }

    /// Point-query: what block is at chunk-local coordinates
    /// (x, y, z), where each coordinate is in `0..CHUNK_SIZE`.
    ///
    /// Algorithm: descend from root. At each internal node, pick
    /// the octant the query point falls into based on the high bit
    /// of (x, y, z) at the current depth. Stop when a leaf is
    /// reached.
    ///
    /// CPU twin of the GLSL `octree_sample` function (v0.24.24).
    /// Implementations stay aligned so test coverage on the CPU
    /// side validates the encoding the GPU will eventually read.
    pub fn sample(&self, x: i32, y: i32, z: i32) -> BlockId {
        debug_assert!((0..CHUNK_SIZE).contains(&x), "x={} out of range", x);
        debug_assert!((0..CHUNK_SIZE).contains(&y), "y={} out of range", y);
        debug_assert!((0..CHUNK_SIZE).contains(&z), "z={} out of range", z);

        let mut node_idx: u32 = 0;
        // Track which 2^depth cube the current node represents.
        // Start with the chunk-root cube at origin.
        let mut current_depth = OCTREE_DEPTH;
        let mut origin_x: i32 = 0;
        let mut origin_y: i32 = 0;
        let mut origin_z: i32 = 0;

        loop {
            let node = self.nodes[node_idx as usize];
            if node.is_leaf() {
                return node.leaf_block();
            }
            // Internal: pick octant. The current node's cube has
            // edge length 2^current_depth. Its half is 2^(d-1).
            // Octant axis bit i = 1 if query coord >= origin + half.
            let half = 1i32 << (current_depth - 1);
            let bx = if x - origin_x >= half { 1 } else { 0 };
            let by = if y - origin_y >= half { 1 } else { 0 };
            let bz = if z - origin_z >= half { 1 } else { 0 };
            let octant = bx | (by << 1) | (bz << 2);

            // Advance origin into the chosen octant.
            if bx == 1 { origin_x += half; }
            if by == 1 { origin_y += half; }
            if bz == 1 { origin_z += half; }
            current_depth -= 1;

            node_idx = node.children[octant as usize];
        }
    }

    /// Reconstruct a dense `Chunk` by sampling every cell. O(N).
    /// Used for round-trip equivalence tests; not a hot path.
    ///
    /// The reconstructed chunk has empty `explicit_indices`,
    /// `plant_indices`, etc — the octree only encodes block ids,
    /// not the inverted-index structures the Chunk maintains for
    /// gameplay. Round-trip tests should compare `blocks` field
    /// only.
    pub fn to_chunk(&self) -> Chunk {
        let mut blocks: Vec<BlockId> = Vec::with_capacity(CHUNK_VOLUME);
        // Same iteration order as Chunk's storage:
        // index = x + z*CHUNK_SIZE + y*CHUNK_SIZE²
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    blocks.push(self.sample(x, y, z));
                }
            }
        }
        Chunk::from_blocks(self.coord, blocks)
    }

    /// Number of nodes in the encoded tree. Useful for measuring
    /// compression ratio across a world (sum across chunks vs.
    /// 32K-byte dense baseline).
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// True if this chunk collapses to a single pure-air leaf (the common
    /// "sky" case — most chunks above the surface, and almost everything
    /// while flying). Such a chunk emits ZERO mesh faces regardless of its
    /// neighbors: air cells never produce geometry, and the faces on any
    /// adjacent solids belong to those neighbors' own chunks. So the GPU
    /// mesher (which spends ~7 latency-bound octree walks per cell on all
    /// 32768 cells just to discover there's nothing to emit) can be skipped
    /// entirely and an empty mesh returned directly. Conservative: only the
    /// fully-collapsed single-leaf form is detected here; a rare un-collapsed
    /// all-air octree falls through to normal meshing (correct, just not
    /// skipped). Cheap O(1) check — safe to call before every dispatch.
    pub fn is_uniform_air(&self) -> bool {
        self.nodes.len() == 1
            && self.nodes[0].is_leaf()
            && self.nodes[0].leaf_block() == BlockId::Air
    }

    /// Reverse-propagate a "modified at tick `tick`" stamp from a
    /// single leaf at chunk-local (x, y, z) up to the root.
    ///
    /// This is the producer side of the cache-deferment scheme:
    /// every edit calls this, which walks the ancestor chain
    /// (5 hops max for a 32³ chunk) writing `tick` into each
    /// `last_modified_tick`. Subsequent consumers that called
    /// `region_stable_since(region, t_query)` with `t_query >= tick`
    /// will see the modification; consumers with `t_query > tick`
    /// see "stable" and can skip work.
    ///
    /// O(OCTREE_DEPTH) = O(5) per edit. Edits in bulk should batch
    /// through `mark_region_modified` instead.
    /// Incrementally edit a single block in this octree. Expands
    /// collapsed leaves along the path as needed ("split on write"),
    /// updates the affected depth-0 leaf, and stamps every ancestor's
    /// `last_modified_tick` with the given tick.
    ///
    /// O(depth) work — typically 5 levels for our 32³ chunk size, vs
    /// the O(`CHUNK_VOLUME`) cost of `from_chunk`. The first edit
    /// through a previously-collapsed subtree adds up to `depth × 8`
    /// nodes; subsequent edits along that path reuse the now-expanded
    /// subtree.
    ///
    /// Returns the previous block at this position. The returned
    /// `BlockId` matches what `sample(x, y, z)` would have returned
    /// just before the call.
    ///
    /// v0.24.23 propagation: replaces the previous "rebuild from
    /// scratch on every edit" pattern. Per-edit cost drops from ~1ms
    /// (full chunk rebuild) to a few microseconds (worst case ~40
    /// node allocations + 5 reads). Both `set_block` and `from_chunk`
    /// remain useful: `from_chunk` for fresh chunk-load, `set_block`
    /// for incremental edits during gameplay.
    ///
    /// **Cache-deferment fix**: this method also resolves the
    /// "collapsed-leaf invalidation poisoning" issue noted in earlier
    /// docs. Previously, a fully-collapsed homogeneous chunk that got
    /// edited stamped its single root leaf with the new tick, which
    /// invalidated cache validity for the whole chunk's volume. Now
    /// the split-on-write expands the affected octant down to the
    /// edited cell, so `region_stable_since` queries on unaffected
    /// sibling regions correctly report stable.
    pub fn set_block(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        block: BlockId,
        tick: u32,
    ) -> BlockId {
        debug_assert!((0..CHUNK_SIZE).contains(&x));
        debug_assert!((0..CHUNK_SIZE).contains(&y));
        debug_assert!((0..CHUNK_SIZE).contains(&z));

        // Phase 1: descend from root, expanding any collapsed leaf
        // encountered along the path. Stop at depth 0.
        let mut node_idx: u32 = 0;
        let mut current_depth = OCTREE_DEPTH;
        let mut origin_x: i32 = 0;
        let mut origin_y: i32 = 0;
        let mut origin_z: i32 = 0;

        while current_depth > 0 {
            let node = self.nodes[node_idx as usize];
            if node.is_leaf() {
                let existing = node.leaf_block();
                // Optimization: if the leaf already holds the target
                // block, no edit is needed. Still bump the path tick
                // so consumers see the "I touched this" semantics.
                if existing == block {
                    self.mark_path_modified(x, y, z, tick);
                    return existing;
                }
                // Split: allocate 8 children at `current_depth - 1`,
                // each a leaf carrying the OLD block (so the
                // unedited siblings continue to represent homogeneous
                // regions of the original content). Patch this node
                // to be internal pointing at the new children.
                let new_depth = current_depth - 1;
                let first_child = self.nodes.len() as u32;
                for _ in 0..8 {
                    self.nodes.push(OctreeNode::leaf(new_depth, existing));
                }
                let mut children = [0u32; 8];
                for i in 0..8 {
                    children[i] = first_child + i as u32;
                }
                self.nodes[node_idx as usize] =
                    OctreeNode::internal(current_depth, children);
            }
            // Internal node now. Pick the octant containing (x,y,z).
            let half = 1i32 << (current_depth - 1);
            let bx = if x - origin_x >= half { 1 } else { 0 };
            let by = if y - origin_y >= half { 1 } else { 0 };
            let bz = if z - origin_z >= half { 1 } else { 0 };
            let octant = (bx | (by << 1) | (bz << 2)) as usize;
            if bx == 1 { origin_x += half; }
            if by == 1 { origin_y += half; }
            if bz == 1 { origin_z += half; }
            node_idx = self.nodes[node_idx as usize].children[octant];
            current_depth -= 1;
        }

        // Phase 2: at depth 0. Replace the leaf.
        let old_block = self.nodes[node_idx as usize].leaf_block();
        self.nodes[node_idx as usize] = OctreeNode::leaf(0, block);

        // Phase 3: stamp the path with the new tick. Walks from root
        // through the modified path; each ancestor's tick advances
        // to `tick` (taking max with existing so we never go
        // backward). Sibling subtrees keep their pre-edit ticks.
        self.mark_path_modified(x, y, z, tick);

        old_block
    }

    /// Incremental single-block edit that does the same split-on-write
    /// descent as [`set_block`] but **does not** stamp
    /// `last_modified_tick` anywhere. The octree's *data* becomes
    /// current, but every node's tick is left untouched, so
    /// cache-deferment consumers (`region_stable_since`) still report
    /// the region as stable and the GPU mesher leaves the existing
    /// (coarse-LOD) geometry frozen — no re-mesh fires.
    ///
    /// This is the variant the batch apply path uses when the remesh
    /// gate has decided an edit is invisible at the chunk's current LOD
    /// (e.g. grass/flower growth on a far chunk): we want the data right
    /// for whenever the chunk later resolves to a finer LOD, without
    /// paying a GPU re-mesh now. Returns the previous block at (x, y, z),
    /// matching what `sample(x, y, z)` would have returned just before.
    pub fn edit_block_unstamped(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        block: BlockId,
    ) -> BlockId {
        debug_assert!((0..CHUNK_SIZE).contains(&x));
        debug_assert!((0..CHUNK_SIZE).contains(&y));
        debug_assert!((0..CHUNK_SIZE).contains(&z));

        // Descend from root, expanding any collapsed leaf encountered
        // along the path. Stop at depth 0. (Identical traversal to
        // `set_block` Phases 1-2; only the Phase-3 tick stamp is omitted.)
        let mut node_idx: u32 = 0;
        let mut current_depth = OCTREE_DEPTH;
        let mut origin_x: i32 = 0;
        let mut origin_y: i32 = 0;
        let mut origin_z: i32 = 0;

        while current_depth > 0 {
            let node = self.nodes[node_idx as usize];
            if node.is_leaf() {
                let existing = node.leaf_block();
                // Already the target block: subtree is correct, nothing
                // to split or set, and (by contract) no tick to stamp.
                if existing == block {
                    return existing;
                }
                // Split: 8 children at `current_depth - 1`, each a leaf
                // carrying the OLD block, then make this node internal.
                let new_depth = current_depth - 1;
                let first_child = self.nodes.len() as u32;
                for _ in 0..8 {
                    self.nodes.push(OctreeNode::leaf(new_depth, existing));
                }
                let mut children = [0u32; 8];
                for i in 0..8 {
                    children[i] = first_child + i as u32;
                }
                self.nodes[node_idx as usize] =
                    OctreeNode::internal(current_depth, children);
            }
            let half = 1i32 << (current_depth - 1);
            let bx = if x - origin_x >= half { 1 } else { 0 };
            let by = if y - origin_y >= half { 1 } else { 0 };
            let bz = if z - origin_z >= half { 1 } else { 0 };
            let octant = (bx | (by << 1) | (bz << 2)) as usize;
            if bx == 1 { origin_x += half; }
            if by == 1 { origin_y += half; }
            if bz == 1 { origin_z += half; }
            node_idx = self.nodes[node_idx as usize].children[octant];
            current_depth -= 1;
        }

        let old_block = self.nodes[node_idx as usize].leaf_block();
        self.nodes[node_idx as usize] = OctreeNode::leaf(0, block);
        old_block
    }

    /// Reverse-propagate a tick stamp from a single edited cell up
    /// to the root. Walks from root to the cell at (x, y, z), bumping
    /// each visited node's `last_modified_tick` to the given tick if
    /// it isn't already at least that high. Caller-side hook for
    /// `set_block` and direct external "I touched here" callers.
    pub fn mark_path_modified(&mut self, x: i32, y: i32, z: i32, tick: u32) {
        debug_assert!((0..CHUNK_SIZE).contains(&x));
        debug_assert!((0..CHUNK_SIZE).contains(&y));
        debug_assert!((0..CHUNK_SIZE).contains(&z));

        // Same descent as `sample` but mutating tick on each visited
        // node. Record the path so we touch ancestors only, never
        // the same node twice.
        let mut node_idx: u32 = 0;
        let mut current_depth = OCTREE_DEPTH;
        let mut origin_x: i32 = 0;
        let mut origin_y: i32 = 0;
        let mut origin_z: i32 = 0;

        loop {
            // Always bump the visited node's tick — this is the
            // upward propagation. Even if the node is a leaf, we
            // stamp it (consumers that look at just-this-leaf
            // know).
            if self.nodes[node_idx as usize].last_modified_tick < tick {
                self.nodes[node_idx as usize].last_modified_tick = tick;
            }

            let node = self.nodes[node_idx as usize];
            if node.is_leaf() {
                return;
            }
            let half = 1i32 << (current_depth - 1);
            let bx = if x - origin_x >= half { 1 } else { 0 };
            let by = if y - origin_y >= half { 1 } else { 0 };
            let bz = if z - origin_z >= half { 1 } else { 0 };
            let octant = bx | (by << 1) | (bz << 2);
            if bx == 1 { origin_x += half; }
            if by == 1 { origin_y += half; }
            if bz == 1 { origin_z += half; }
            current_depth -= 1;
            node_idx = node.children[octant as usize];
        }
    }

    /// Reverse-propagate a tick stamp across all leaves within an
    /// axis-aligned region (closed-open: min inclusive, max
    /// exclusive). Cheaper than calling `mark_path_modified` per
    /// leaf because subtrees that lie entirely inside the region
    /// only need their root stamped — the consumer's stability
    /// check short-circuits at that level anyway.
    pub fn mark_region_modified(
        &mut self,
        min: (i32, i32, i32),
        max: (i32, i32, i32),
        tick: u32,
    ) {
        // Conservative: stamp every node whose cube overlaps the
        // region. Recursive descent; we stamp the root once and
        // recurse into octants that intersect.
        if min.0 >= max.0 || min.1 >= max.1 || min.2 >= max.2 {
            return; // empty region
        }
        self.stamp_recursive(
            0,
            OCTREE_DEPTH,
            (0, 0, 0),
            min,
            max,
            tick,
        );
    }

    fn stamp_recursive(
        &mut self,
        node_idx: u32,
        depth: u32,
        origin: (i32, i32, i32),
        rmin: (i32, i32, i32),
        rmax: (i32, i32, i32),
        tick: u32,
    ) {
        let size = 1i32 << depth;
        let node_max = (origin.0 + size, origin.1 + size, origin.2 + size);

        // If this node's cube doesn't intersect the region, skip.
        if rmax.0 <= origin.0 || rmin.0 >= node_max.0
            || rmax.1 <= origin.1 || rmin.1 >= node_max.1
            || rmax.2 <= origin.2 || rmin.2 >= node_max.2 {
            return;
        }

        if self.nodes[node_idx as usize].last_modified_tick < tick {
            self.nodes[node_idx as usize].last_modified_tick = tick;
        }

        // If this node is fully contained in the region, the stamp
        // above is sufficient — consumers will see it at this level
        // and skip the subtree. No need to recurse.
        if rmin.0 <= origin.0 && rmin.1 <= origin.1 && rmin.2 <= origin.2
            && rmax.0 >= node_max.0 && rmax.1 >= node_max.1 && rmax.2 >= node_max.2 {
            return;
        }

        // Leaf at the boundary: nothing more to do (stamp already
        // applied).
        let node = self.nodes[node_idx as usize];
        if node.is_leaf() {
            return;
        }

        // Recurse into each octant.
        let half = size >> 1;
        for octant in 0..8 {
            let dx = (octant & 1) * half;
            let dy = ((octant >> 1) & 1) * half;
            let dz = ((octant >> 2) & 1) * half;
            let child_origin = (origin.0 + dx, origin.1 + dy, origin.2 + dz);
            self.stamp_recursive(
                node.children[octant as usize],
                depth - 1,
                child_origin,
                rmin, rmax,
                tick,
            );
        }
    }

    /// Cache-deferment query: was any leaf within the region
    /// modified AT or AFTER `since_tick`?
    ///
    /// Consumers maintain a per-region "I last read this at tick T"
    /// timestamp. Each frame (or each item invocation, or each
    /// physics tick) they call `region_stable_since(region, T)`. If
    /// the answer is `true`, they reuse cached output and skip the
    /// entire downstream pipeline (mesh extraction, raycast, item
    /// region read, etc.). If `false`, they re-run their work
    /// against the live data and update their cached `T = current
    /// tick`.
    ///
    /// O(min(K, N)) where K = number of nodes intersecting the
    /// region — short-circuits the moment it finds an ancestor with
    /// `last_modified_tick >= since_tick` (i.e., proof of dirty),
    /// AND short-circuits descent into ancestors whose tick is
    /// strictly less than `since_tick` (proof the subtree is
    /// stable). In the typical case (region untouched for many
    /// ticks) this is one root check.
    pub fn region_stable_since(
        &self,
        min: (i32, i32, i32),
        max: (i32, i32, i32),
        since_tick: u32,
    ) -> bool {
        if min.0 >= max.0 || min.1 >= max.1 || min.2 >= max.2 {
            return true; // empty region is trivially stable
        }
        !self.region_dirty_recursive(
            0,
            OCTREE_DEPTH,
            (0, 0, 0),
            min,
            max,
            since_tick,
        )
    }

    fn region_dirty_recursive(
        &self,
        node_idx: u32,
        depth: u32,
        origin: (i32, i32, i32),
        rmin: (i32, i32, i32),
        rmax: (i32, i32, i32),
        since_tick: u32,
    ) -> bool {
        let size = 1i32 << depth;
        let node_max = (origin.0 + size, origin.1 + size, origin.2 + size);

        // No intersection: this subtree contributes no dirty.
        if rmax.0 <= origin.0 || rmin.0 >= node_max.0
            || rmax.1 <= origin.1 || rmin.1 >= node_max.1
            || rmax.2 <= origin.2 || rmin.2 >= node_max.2 {
            return false;
        }

        let node = self.nodes[node_idx as usize];

        // Subtree is stable: every descendant carries a stale tick.
        if node.last_modified_tick < since_tick {
            return false;
        }

        // Subtree has been touched. If the region fully contains
        // this node, the dirty stamp counts; report dirty.
        if rmin.0 <= origin.0 && rmin.1 <= origin.1 && rmin.2 <= origin.2
            && rmax.0 >= node_max.0 && rmax.1 >= node_max.1 && rmax.2 >= node_max.2 {
            return true;
        }

        // Leaf at the boundary: the stamp counts only if its cell
        // actually overlaps the region (which it does, given we
        // passed the intersection check above).
        if node.is_leaf() {
            return true;
        }

        // Internal node, partial overlap: descend to find which
        // child's subtree actually contains the dirt.
        let half = size >> 1;
        for octant in 0..8 {
            let dx = (octant & 1) * half;
            let dy = ((octant >> 1) & 1) * half;
            let dz = ((octant >> 2) & 1) * half;
            let child_origin = (origin.0 + dx, origin.1 + dy, origin.2 + dz);
            if self.region_dirty_recursive(
                node.children[octant as usize],
                depth - 1,
                child_origin,
                rmin, rmax,
                since_tick,
            ) {
                return true;
            }
        }
        false
    }

    /// Wire-format size in bytes. For uploading to a GPU SSBO.
    pub fn byte_size(&self) -> usize {
        self.nodes.len() * OCTREE_NODE_BYTES
    }
}

/// Recursive builder. Pushes nodes representing the subtree rooted
/// at (origin_x, origin_y, origin_z) with edge length 2^depth_remaining.
/// Returns the array index of the root node of this subtree.
fn build_recursive(
    chunk: &Chunk,
    ox: i32, oy: i32, oz: i32,
    depth_remaining: u32,
    nodes: &mut Vec<OctreeNode>,
) -> u32 {
    if depth_remaining == 0 {
        // Single-block leaf. depth field = 0.
        let block = chunk.get(ox, oy, oz);
        nodes.push(OctreeNode::leaf(0, block));
        return (nodes.len() - 1) as u32;
    }

    // Recurse into 8 octants. Each child's subtree is appended to
    // `nodes`. The child's root index is what build_recursive
    // returns.
    let half = 1i32 << (depth_remaining - 1);
    let mut child_idxs = [0u32; 8];
    let pre_len = nodes.len();
    for octant in 0..8u32 {
        let dx = (octant & 1) as i32 * half;
        let dy = ((octant >> 1) & 1) as i32 * half;
        let dz = ((octant >> 2) & 1) as i32 * half;
        child_idxs[octant as usize] = build_recursive(
            chunk,
            ox + dx, oy + dy, oz + dz,
            depth_remaining - 1,
            nodes,
        );
    }

    // Check: are all 8 children leaves carrying the same block id?
    // If so, collapse to a single higher-depth leaf.
    let mut all_same_leaf = true;
    let first_block = {
        let n = nodes[child_idxs[0] as usize];
        if !n.is_leaf() { all_same_leaf = false; }
        n.leaf_block()
    };
    if all_same_leaf {
        for &ci in &child_idxs[1..] {
            let n = nodes[ci as usize];
            if !n.is_leaf() || n.leaf_block() != first_block {
                all_same_leaf = false;
                break;
            }
        }
    }

    if all_same_leaf {
        // Discard the 8 children we just pushed, replace with a
        // single coarser leaf. Since the children are the LAST 8
        // entries in nodes (we just pushed them sequentially and
        // they're all depth-0 leaves with no internal descendants
        // — true because if any child was internal, it would
        // have pushed descendants and we wouldn't be in the
        // all_same_leaf branch), truncating back to pre_len is
        // safe.
        //
        // Wait: not quite. The children's INTERNAL children would
        // have been pushed too. But we just established
        // `all_same_leaf` requires every child to be a depth-0
        // leaf, which means the recursion bottomed out without
        // pushing any internal nodes. So truncating to pre_len
        // really does only discard the 8 leaves we just emitted.
        nodes.truncate(pre_len);
        nodes.push(OctreeNode::leaf(depth_remaining, first_block));
        return (nodes.len() - 1) as u32;
    }

    // Non-collapsing case: emit the internal node referring to its
    // children. The internal node's own array slot is allocated
    // AFTER its children — so the array is in (children-first,
    // parent-last) order. The root ends up being the last node
    // pushed, which `from_chunk` then swaps to index 0.
    nodes.push(OctreeNode::internal(depth_remaining, child_idxs));
    (nodes.len() - 1) as u32
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::blocks::BlockId;
    use crate::world::chunk::{Chunk, ChunkCoord};

    fn empty_chunk() -> Chunk {
        Chunk::from_blocks(
            ChunkCoord::new(0, 0, 0),
            vec![BlockId::Air; CHUNK_VOLUME],
        )
    }

    #[test]
    fn node_is_exactly_40_bytes() {
        assert_eq!(std::mem::size_of::<OctreeNode>(), 40);
        assert_eq!(OCTREE_NODE_BYTES, 40);
    }

    #[test]
    fn empty_chunk_collapses_to_one_node() {
        let chunk = empty_chunk();
        let oct = OctreeArray::from_chunk(&chunk);
        assert_eq!(oct.nodes.len(), 1, "all-air chunk must collapse");
        assert!(oct.nodes[0].is_leaf());
        assert_eq!(oct.nodes[0].leaf_block(), BlockId::Air);
        assert_eq!(oct.nodes[0].depth(), OCTREE_DEPTH);
    }

    #[test]
    fn solid_chunk_collapses_to_one_node() {
        let mut chunk = empty_chunk();
        for i in 0..CHUNK_VOLUME {
            chunk.blocks[i] = BlockId::Stone;
        }
        let oct = OctreeArray::from_chunk(&chunk);
        assert_eq!(oct.nodes.len(), 1);
        assert_eq!(oct.nodes[0].leaf_block(), BlockId::Stone);
    }

    #[test]
    fn single_cell_difference_doesnt_collapse() {
        // Place a stone cube at (0,0,0); rest is air. Expect a
        // partial expansion: the root cannot collapse because its
        // 8 octants differ. One of those octants then differs
        // internally, expanding further, etc.
        let mut chunk = empty_chunk();
        chunk.set(0, 0, 0, BlockId::Stone);
        let oct = OctreeArray::from_chunk(&chunk);
        // Each depth level contributes at most one non-collapsed
        // internal node + 7 air leaves + 1 recurse. So we get:
        //   depth 5: 1 internal, 7 air leaves + 1 child
        //   depth 4: 1 internal, 7 air leaves + 1 child
        //   ... depth 1: 1 internal, 7 air leaves + 1 child
        //   depth 0: 1 stone leaf
        // = 5 internals + 5*7 air leaves + 1 stone leaf = 5 + 35
        //   + 1 = 41 nodes.
        assert_eq!(oct.nodes.len(), 41,
            "single-cell stone in air should expand to 41 nodes");
        // Sanity: the sample API should return what we set.
        assert_eq!(oct.sample(0, 0, 0), BlockId::Stone);
        assert_eq!(oct.sample(1, 0, 0), BlockId::Air);
        assert_eq!(oct.sample(15, 15, 15), BlockId::Air);
    }

    #[test]
    fn round_trip_empty() {
        let chunk = empty_chunk();
        let oct = OctreeArray::from_chunk(&chunk);
        let restored = oct.to_chunk();
        assert_eq!(restored.blocks, chunk.blocks);
    }

    #[test]
    fn round_trip_random_chunk() {
        // Deterministic pseudo-random fill — pick blocks based on a
        // small hash of (x, y, z). Doesn't represent realistic
        // terrain but exercises the full octree with many leaves.
        use crate::world::blocks::BlockId::*;
        let palette = [Air, Stone, Dirt, Sand, Water];
        let mut chunk = empty_chunk();
        let mut idx = 0;
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let h = ((x as u32).wrapping_mul(73856093)
                          ^ (y as u32).wrapping_mul(19349663)
                          ^ (z as u32).wrapping_mul(83492791)) as usize;
                    chunk.blocks[idx] = palette[h % palette.len()];
                    idx += 1;
                }
            }
        }

        let oct = OctreeArray::from_chunk(&chunk);
        let restored = oct.to_chunk();
        assert_eq!(restored.blocks, chunk.blocks,
            "round-trip must produce identical blocks");
    }

    #[test]
    fn round_trip_split_chunk() {
        // Half stone, half air at the y=16 midplane. Should
        // collapse the two homogeneous halves to coarser-than-leaf
        // levels.
        let mut chunk = empty_chunk();
        for y in 0..16 {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    chunk.set(x, y, z, BlockId::Stone);
                }
            }
        }
        let oct = OctreeArray::from_chunk(&chunk);
        // The stone half is a 32×16×32 slab, which is 2 separate
        // 16³ octants of the root. Each 16³ octant should collapse
        // to one leaf (depth 4). The air half collapses similarly.
        // Total: 1 root + 8 leaves at depth 4 = 9 nodes max.
        // Actually the root's octants split y=16, so 4 of them are
        // fully stone (lower-y) and 4 are fully air (upper-y) —
        // each collapses to one depth-4 leaf. Root + 8 children =
        // 9 nodes.
        assert!(oct.nodes.len() <= 9,
            "split chunk should collapse to ≤9 nodes, got {}",
            oct.nodes.len());

        // Verify sample correctness.
        assert_eq!(oct.sample(15, 0, 15), BlockId::Stone);
        assert_eq!(oct.sample(15, 15, 15), BlockId::Stone);
        assert_eq!(oct.sample(15, 16, 15), BlockId::Air);
        assert_eq!(oct.sample(15, 31, 15), BlockId::Air);

        // And round-trip.
        let restored = oct.to_chunk();
        assert_eq!(restored.blocks, chunk.blocks);
    }

    #[test]
    fn sample_corners() {
        let mut chunk = empty_chunk();
        chunk.set(0, 0, 0, BlockId::Stone);
        chunk.set(31, 0, 0, BlockId::Dirt);
        chunk.set(0, 31, 0, BlockId::Sand);
        chunk.set(0, 0, 31, BlockId::Water);
        chunk.set(31, 31, 31, BlockId::Cobblestone);
        let oct = OctreeArray::from_chunk(&chunk);
        assert_eq!(oct.sample(0, 0, 0), BlockId::Stone);
        assert_eq!(oct.sample(31, 0, 0), BlockId::Dirt);
        assert_eq!(oct.sample(0, 31, 0), BlockId::Sand);
        assert_eq!(oct.sample(0, 0, 31), BlockId::Water);
        assert_eq!(oct.sample(31, 31, 31), BlockId::Cobblestone);
        assert_eq!(oct.sample(15, 15, 15), BlockId::Air);
    }

    #[test]
    fn byte_size_matches_node_count() {
        let chunk = empty_chunk();
        let oct = OctreeArray::from_chunk(&chunk);
        assert_eq!(oct.byte_size(), oct.nodes.len() * OCTREE_NODE_BYTES);
    }

    // -------- Cache-deferment / reverse-propagation tests --------

    #[test]
    fn fresh_octree_is_stable_at_tick_one() {
        // No edits yet → every node has tick 0 → any consumer at
        // tick >= 1 sees the entire chunk as stable.
        let mut chunk = empty_chunk();
        chunk.set(5, 6, 7, BlockId::Stone);
        let oct = OctreeArray::from_chunk(&chunk);
        assert!(oct.region_stable_since((0, 0, 0), (32, 32, 32), 1));
    }

    #[test]
    fn single_edit_marks_only_its_path() {
        let mut chunk = empty_chunk();
        chunk.set(5, 6, 7, BlockId::Stone);
        let mut oct = OctreeArray::from_chunk(&chunk);

        oct.mark_path_modified(5, 6, 7, 100);

        // The leaf at (5,6,7) is now stale w.r.t. anyone asking
        // "anything new since tick 0".
        assert!(!oct.region_stable_since((5, 6, 7), (6, 7, 8), 1));
        // But a tiny region NOT containing (5,6,7) is still stable.
        assert!(oct.region_stable_since((20, 20, 20), (21, 21, 21), 1));
        // And someone who already read at tick 100 or later sees
        // no change.
        assert!(oct.region_stable_since((5, 6, 7), (6, 7, 8), 101));
    }

    #[test]
    fn region_mark_stamps_subtree_only_once() {
        // mark_region_modified should stamp full-cover subtrees at
        // the highest enclosing node, then stop. We test that by
        // marking the entire chunk (covers root completely) and
        // checking that only the root's tick changed.
        let chunk = empty_chunk();
        let mut oct = OctreeArray::from_chunk(&chunk);

        // Original: single root leaf at tick 0.
        assert_eq!(oct.nodes.len(), 1);
        assert_eq!(oct.nodes[0].last_modified_tick, 0);

        oct.mark_region_modified((0, 0, 0), (32, 32, 32), 42);

        // Single node, now stamped tick 42.
        assert_eq!(oct.nodes[0].last_modified_tick, 42);
        // Consumer at tick <= 42 sees dirty.
        assert!(!oct.region_stable_since((0, 0, 0), (32, 32, 32), 42));
        assert!(!oct.region_stable_since((0, 0, 0), (32, 32, 32), 1));
        // Consumer at tick >= 43 sees stable.
        assert!(oct.region_stable_since((0, 0, 0), (32, 32, 32), 43));
    }

    #[test]
    fn ticks_dont_regress_under_lower_tick_marks() {
        // If a leaf was marked at tick 50, then we ask to mark at
        // tick 40, the tick should stay at 50 (only monotonic
        // increases).
        let chunk = empty_chunk();
        let mut oct = OctreeArray::from_chunk(&chunk);
        oct.mark_path_modified(0, 0, 0, 50);
        oct.mark_path_modified(0, 0, 0, 40);
        assert_eq!(oct.nodes[0].last_modified_tick, 50);
    }

    #[test]
    fn small_dirty_within_large_stable_subtree() {
        // A chunk that collapsed to one stone node (depth 5). Mark
        // a corner edit. Confirm a region NOT containing that
        // corner reports stable; a region containing it reports
        // dirty.
        let mut chunk = empty_chunk();
        for i in 0..CHUNK_VOLUME {
            chunk.blocks[i] = BlockId::Stone;
        }
        let mut oct = OctreeArray::from_chunk(&chunk);
        assert_eq!(oct.nodes.len(), 1);

        // Naive mark_path_modified on a single-node tree just
        // stamps the root. Verify and check stability semantics.
        oct.mark_path_modified(0, 0, 0, 7);
        // Whole chunk is "potentially dirty" at tick 7 because the
        // sole-leaf root carries the stamp. This is the
        // conservative answer for a fully-collapsed chunk: the
        // octree doesn't know which sub-cell was edited.
        assert!(!oct.region_stable_since((0, 0, 0), (1, 1, 1), 1));
        assert!(!oct.region_stable_since((30, 30, 30), (32, 32, 32), 1));
        // (Edit-aware sub-cell tracking inside collapsed leaves is
        // possible — expand-on-write the leaf into 8 children
        // before stamping — but that defeats the collapse. The
        // pragmatic trade is: edits in homogeneous regions
        // pessimize cache invalidation. Real terrain isn't fully
        // homogeneous, so this rarely matters in practice.)
        //
        // v0.24.23 follow-up: `set_block` resolves this by splitting
        // the collapsed leaf on first edit. See
        // `set_block_expands_collapsed_leaf` below.
    }

    /// v0.24.23: incremental edit on a fully-collapsed (all-air)
    /// octree expands the path down to the edited cell. After the
    /// edit, the chunk has many more nodes than 1 — proof that
    /// expansion happened.
    #[test]
    fn set_block_expands_collapsed_leaf() {
        let mut chunk = empty_chunk();
        let mut oct = OctreeArray::from_chunk(&chunk);
        assert_eq!(oct.nodes.len(), 1, "fully-collapsed chunk starts with 1 node");

        let old = oct.set_block(5, 5, 5, BlockId::Stone, 10);
        assert_eq!(old, BlockId::Air);
        assert_eq!(oct.sample(5, 5, 5), BlockId::Stone);
        // Expanded: at least 5 levels of expansion, each adding 8
        // children. Should be well above 1 node now.
        assert!(oct.nodes.len() > 30,
            "expected expansion to add many nodes, got {}", oct.nodes.len());

        // Update the dense chunk and verify round-trip equivalence
        // (octree should now match dense for the edited cell).
        chunk.set(5, 5, 5, BlockId::Stone);
        for y in 0..32 {
            for z in 0..32 {
                for x in 0..32 {
                    assert_eq!(oct.sample(x, y, z), chunk.get(x, y, z),
                        "mismatch at ({},{},{})", x, y, z);
                }
            }
        }
    }

    /// v0.24.23: after `set_block` expands a previously-collapsed
    /// chunk, `region_stable_since` queries on regions FAR from the
    /// edit correctly report stable. The collapsed-leaf invalidation
    /// poisoning issue is resolved.
    #[test]
    fn set_block_resolves_collapsed_leaf_poisoning() {
        let chunk = empty_chunk();
        let mut oct = OctreeArray::from_chunk(&chunk);
        // Capture an early-tick snapshot before the edit.
        let snapshot_tick = 5;

        oct.set_block(5, 5, 5, BlockId::Stone, 10);

        // Region around the edit: dirty since snapshot.
        assert!(!oct.region_stable_since((0, 0, 0), (8, 8, 8), snapshot_tick + 1),
            "region containing the edit must be dirty");
        // Region far from the edit (other end of chunk): STABLE
        // since snapshot — proves the poisoning is resolved.
        assert!(oct.region_stable_since((24, 24, 24), (32, 32, 32), snapshot_tick + 1),
            "region far from edit must still be stable; expansion isolates the edit");
    }

    /// v0.24.23: setting a block to its existing value is a no-op
    /// (no expansion, no node growth). Still bumps the tick on the
    /// path so consumers see the "I touched here" semantics.
    #[test]
    fn set_block_to_same_value_is_no_op_for_structure() {
        let chunk = empty_chunk();
        let mut oct = OctreeArray::from_chunk(&chunk);
        let nodes_before = oct.nodes.len();

        let old = oct.set_block(5, 5, 5, BlockId::Air, 10);
        assert_eq!(old, BlockId::Air);
        assert_eq!(oct.nodes.len(), nodes_before,
            "setting to the same block should not expand the tree");
        // Path tick still bumped to 10. A stable-since query that's
        // looking back from BEFORE-or-AT the edit tick must see it as
        // dirty. Convention used elsewhere in this file:
        // `snapshot_tick + 1` where snapshot was taken *before* the
        // edit. Here snapshot was tick 0 (fresh octree); 10 >= 1 so
        // dirty. (The previous value `11` was a typo — 10 < 11, so
        // the region would correctly read as stable since 11.)
        assert!(!oct.region_stable_since((4, 4, 4), (8, 8, 8), 1),
            "even a no-op edit bumps the path tick");
    }

    /// v0.24.23: returned `old_block` matches what `sample()` would
    /// have returned just before the call. Useful for caller logic
    /// (item code that wants to know what got overwritten).
    #[test]
    fn set_block_returns_previous_block() {
        let mut chunk = empty_chunk();
        chunk.set(10, 10, 10, BlockId::Stone);
        let mut oct = OctreeArray::from_chunk(&chunk);

        let old = oct.set_block(10, 10, 10, BlockId::Dirt, 5);
        assert_eq!(old, BlockId::Stone);
        assert_eq!(oct.sample(10, 10, 10), BlockId::Dirt);
    }

    // ---- incremental-edit parity (lever #1) ----

    // Build a varied dense chunk: a stone floor, a dirt band, and a few
    // scattered blocks — enough that from_chunk produces a real mix of
    // collapsed and split subtrees rather than one uniform leaf.
    fn varied_blocks() -> Vec<BlockId> {
        let mut b = vec![BlockId::Air; CHUNK_VOLUME];
        let idx = |x: i32, y: i32, z: i32| {
            (x + z * CHUNK_SIZE + y * CHUNK_SIZE * CHUNK_SIZE) as usize
        };
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                b[idx(x, 0, z)] = BlockId::Stone; // floor
                b[idx(x, 1, z)] = BlockId::Dirt; // dirt band
            }
        }
        // A handful of scattered blocks at irregular spots.
        b[idx(3, 5, 7)] = BlockId::Stone;
        b[idx(31, 31, 31)] = BlockId::Dirt;
        b[idx(0, 0, 0)] = BlockId::Stone;
        b[idx(17, 9, 24)] = BlockId::Dirt;
        b[idx(8, 16, 8)] = BlockId::Stone;
        b
    }

    // The core guarantee: an octree built incrementally with set_block
    // (from empty) samples IDENTICALLY to one built with from_chunk for
    // every one of the 32,768 cells. Node layouts may differ (split-only
    // incremental edits carry extra nodes), but the queryable content is
    // bit-identical — which is all any consumer (mesher, physics, raycast)
    // observes.
    #[test]
    fn set_block_sampling_matches_from_chunk() {
        let blocks = varied_blocks();
        let chunk = Chunk::from_blocks(ChunkCoord::new(0, 0, 0), blocks.clone());
        let reference = OctreeArray::from_chunk(&chunk);

        let mut inc = OctreeArray::empty(ChunkCoord::new(0, 0, 0));
        let idx = |x: i32, y: i32, z: i32| {
            (x + z * CHUNK_SIZE + y * CHUNK_SIZE * CHUNK_SIZE) as usize
        };
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let b = blocks[idx(x, y, z)];
                    if b != BlockId::Air {
                        inc.set_block(x, y, z, b, 1);
                    }
                }
            }
        }

        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    assert_eq!(
                        inc.sample(x, y, z),
                        reference.sample(x, y, z),
                        "mismatch at ({x},{y},{z})"
                    );
                }
            }
        }
    }

    // The unstamped variant produces the same data as set_block — only the
    // ticks differ. Verify sampling parity and that NO node carries the
    // edit tick (so region_stable_since stays "stable").
    #[test]
    fn edit_block_unstamped_matches_data_but_not_ticks() {
        let mut stamped = OctreeArray::empty(ChunkCoord::new(0, 0, 0));
        let mut quiet = OctreeArray::empty(ChunkCoord::new(0, 0, 0));
        let cells = [
            (1, 2, 3, BlockId::Stone),
            (1, 2, 3, BlockId::Dirt), // overwrite same cell
            (30, 0, 0, BlockId::Stone),
            (15, 15, 15, BlockId::Dirt),
        ];
        for &(x, y, z, b) in &cells {
            stamped.set_block(x, y, z, b, 9);
            quiet.edit_block_unstamped(x, y, z, b);
        }
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    assert_eq!(stamped.sample(x, y, z), quiet.sample(x, y, z));
                }
            }
        }
        // quiet octree: no node should carry tick 9.
        assert!(quiet.nodes.iter().all(|n| n.last_modified_tick == 0));
        // stamped octree: the root must carry the edit tick.
        assert_eq!(stamped.nodes[0].last_modified_tick, 9);
    }

    // A single incremental edit on a from_chunk-built octree matches the
    // dense chunk with that one cell changed — the actual gameplay path
    // (plant growth edits one block on an existing chunk's octree).
    #[test]
    fn single_incremental_edit_on_built_octree() {
        let blocks = varied_blocks();
        let chunk = Chunk::from_blocks(ChunkCoord::new(0, 0, 0), blocks.clone());
        let mut oct = OctreeArray::from_chunk(&chunk);

        // Grow a block where there was air.
        oct.set_block(12, 20, 5, BlockId::Stone, 7);

        let idx = |x: i32, y: i32, z: i32| {
            (x + z * CHUNK_SIZE + y * CHUNK_SIZE * CHUNK_SIZE) as usize
        };
        let mut expected = blocks.clone();
        expected[idx(12, 20, 5)] = BlockId::Stone;
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    assert_eq!(oct.sample(x, y, z), expected[idx(x, y, z)]);
                }
            }
        }
    }

    // Repeating an identical edit is a structural no-op: the second
    // set_block to the same value must not grow the node table.
    #[test]
    fn repeated_identical_edit_does_not_grow() {
        let mut oct = OctreeArray::empty(ChunkCoord::new(0, 0, 0));
        oct.set_block(4, 4, 4, BlockId::Stone, 1);
        let n = oct.node_count();
        oct.set_block(4, 4, 4, BlockId::Stone, 2);
        assert_eq!(oct.node_count(), n, "no-op edit must not allocate nodes");
        assert_eq!(oct.sample(4, 4, 4), BlockId::Stone);
    }
}
