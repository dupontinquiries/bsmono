//! In-memory chunk representation.
//!
//! Chunks are 32×32×32 cubes of blocks, addressed in 3D by `ChunkCoord`.
//! There is no fixed world height — chunks stack vertically as needed,
//! and a chunk far above any terrain is simply absent (or, if loaded
//! transiently, all-air). A chunk far below terrain is all-stone.
//!
//! Heightmaps and biome samples are *not* stored on the chunk. Either is
//! cheap to compute on demand from the noise field — surface y via a few
//! density-bisection samples per (x, z) column, biome via a single sample
//! at any (x, y, z). Storing them per-chunk would duplicate data across
//! every vertically-stacked chunk in a column, for no benefit.

use crate::constants::{CHUNK_SIZE, CHUNK_VOLUME};
use crate::world::blocks::BlockId;

/// 3D chunk coordinate. World position of the chunk's min corner is
/// `(x * CHUNK_SIZE, y * CHUNK_SIZE, z * CHUNK_SIZE)`.
///
/// `i32` for each axis gives a nominal range of ±2.1 billion chunks per axis,
/// which at 32 blocks/chunk is ±66 billion blocks. "Effectively unbounded"
/// for any plausible world. We can revisit if anyone ever needs more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkCoord {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl ChunkCoord {
    pub const fn new(x: i32, y: i32, z: i32) -> Self { Self { x, y, z } }

    /// Squared Euclidean distance in chunk-coord space. Useful as a priority
    /// metric when streaming chunks around the player.
    pub fn distance_sq(self, other: ChunkCoord) -> i32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dz = self.z - other.z;
        dx * dx + dy * dy + dz * dz
    }

    /// Convert a world-space block position into the chunk that contains it.
    pub fn from_block_pos(world_x: i32, world_y: i32, world_z: i32) -> Self {
        Self {
            x: world_x.div_euclid(CHUNK_SIZE),
            y: world_y.div_euclid(CHUNK_SIZE),
            z: world_z.div_euclid(CHUNK_SIZE),
        }
    }

    /// World position of this chunk's min corner.
    pub fn block_origin(self) -> (i32, i32, i32) {
        (self.x * CHUNK_SIZE, self.y * CHUNK_SIZE, self.z * CHUNK_SIZE)
    }
}

/// 32×32×32 cube of blocks.
///
/// Storage is a flat `Vec<BlockId>` indexed `x + z*CHUNK_SIZE + y*CHUNK_SIZE²`.
/// 32 KB per chunk. Palette compression is a future optimization (typical
/// chunks have 1-8 unique block types and could fit in 4 bits/block easily).
///
/// Hybrid procedural/explicit model: chunks are *born* fully procedural —
/// every block in `blocks` came from the world generator at chunk-load time,
/// derivable from `(coord, seed, generator_version)`. When `set()` is called
/// on a block (player edit, simulation change, etc.), that block's index
/// goes into `explicit_indices`. Storage only needs to persist the explicit
/// blocks; everything else can be regenerated.
///
/// This means most pristine chunks have an empty `explicit_indices` set
/// (zero overhead), while edited chunks scale storage with edit count, not
/// chunk volume.
#[derive(Clone, Debug)]
pub struct Chunk {
    pub coord: ChunkCoord,
    /// Block array, length = CHUNK_VOLUME = 32³ = 32,768.
    /// Indexed as `x + z*CHUNK_SIZE + y*CHUNK_SIZE*CHUNK_SIZE`.
    pub blocks: Vec<BlockId>,
    /// Has this chunk been edited since last save? Edited chunks need
    /// persistence; pristine chunks are regenerable from `(coord, seed)`.
    pub dirty: bool,
    /// Monotonic version counter. Bumped on any block edit. Lets consumers
    /// (mesher, save, network) detect "did this chunk change since I last
    /// looked" without per-block diffing.
    pub version: u64,
    /// Sparse set of block indices that have been *explicitly* set since
    /// chunk creation. Distinguishes player edits / simulation changes from
    /// the original procedural state. Empty for pristine chunks (zero
    /// overhead). Persistence layer only saves these indices.
    ///
    /// Type: BTreeSet<u16> — indices range 0..32768 fits in u16. BTreeSet
    /// gives sorted iteration (helpful for delta encoding) and O(log n)
    /// insertion.
    pub explicit_indices: std::collections::BTreeSet<u16>,

    /// Sparse set of block indices for plants and other sub-voxel hosts.
    /// Maintained on `set()` and `from_blocks()` so consumers (mesher's
    /// decoration pass, lifecycle tick handler, future plant simulation)
    /// can iterate plant blocks directly instead of scanning all 32k cells.
    ///
    /// This is the "inverted index by category" pattern — pay a small cost
    /// at write time, save the linear scan at every read. For pristine
    /// chunks with few plants the index is small (typically 50-200
    /// entries on a forested surface chunk; 0 deep underground).
    pub plant_indices: std::collections::BTreeSet<u16>,

    /// Sparse set of block indices for wood blocks that are tree-root
    /// markers (carry `BlockInstanceState::FLAG_TREE_ROOT`). One entry
    /// per tree visible in this chunk. Mirrors the `plant_indices`
    /// inverted-index pattern: the lifecycle handler's tree-root pass
    /// iterates this directly instead of scanning all wood blocks
    /// looking for the flag.
    ///
    /// Maintained automatically by `set()` — a wood→non-wood transition
    /// removes the entry; the lifecycle handler must call
    /// `set_tree_root_flag()` to add/remove via the instance_state path
    /// since the flag itself isn't visible to `set()`.
    pub tree_root_indices: std::collections::BTreeSet<u16>,

    /// Sparse set of block indices for ALL wood blocks in this chunk
    /// (OakLog, BirchLog), regardless of whether they're tree-root
    /// markers. Populated once at chunk load (worldgen scan) and
    /// maintained incrementally by `set()` on every wood↔non-wood
    /// transition.
    ///
    /// **Why this exists**: the POI build needs to enumerate wood
    /// blocks per chunk. Before this index, the build did a full 32K
    /// linear scan of `blocks` per chunk per refresh. That cost
    /// dominated tick_sim once render distance grew (1300 chunks ×
    /// 32K = 41M block reads per refresh wave). With this index, the
    /// build iterates 0-200 entries per chunk — and an empty index
    /// short-circuits the whole pass, restoring the v0.9.x perf
    /// envelope for the 70%+ of chunks that have no wood.
    ///
    /// **Relationship to `tree_root_indices`**: tree_root_indices is a
    /// strict subset (the marker-bearing wood blocks). All entries in
    /// tree_root_indices MUST also appear in wood_indices.
    pub wood_indices: std::collections::BTreeSet<u16>,

    /// Cached structural points-of-interest for this chunk. Lazy-built
    /// by the lifecycle handler's POI pass and consulted by the
    /// stickiness / growth / shrink passes instead of re-flood-filling
    /// every tick.
    ///
    /// **Refresh policy** (transitional): rebuilt when None OR when
    /// `tick_id - cache.computed_at_tick >= POI_REFRESH_INTERVAL_TICKS`.
    /// The 10-tick cadence is a placeholder until structural dirty
    /// marking lands. The end-state policy is "rebuild iff
    /// `chunk.structural_dirty`," which means a chunk where nothing
    /// changes never rebuilds — paying the cost once across the
    /// chunk's lifetime instead of every N ticks. The 10-tick floor
    /// gives us bounded staleness in the meantime.
    ///
    /// See `should_refresh_poi()` in plant_lifecycle.rs — that's the
    /// single seam where the dirty-bit check will replace the cadence
    /// when the time comes.
    ///
    /// Not persisted: rebuilt on chunk load, idempotent.
    pub poi_cache: Option<ChunkPoiCache>,

    /// Sparse per-block instance state. Stores mutable per-instance data
    /// (lifecycle age, decay damage, erosion level, etc.) for blocks that
    /// have *non-default* state. Empty for pristine blocks — most blocks
    /// have no entry here, paying zero memory.
    ///
    /// Why sparse: the alternative (dense `Vec<BlockInstanceState>` of
    /// length CHUNK_VOLUME) doubles chunk memory even when 99% of blocks
    /// are unmodified. A BTreeMap costs O(log n) lookup but is empty for
    /// pristine chunks. For lifecycle ticks iterating `plant_indices`
    /// (the inverted index), we lookup state once per plant — fast even
    /// at thousands of entries.
    pub instance_state: std::collections::BTreeMap<u16, BlockInstanceState>,

    /// Sparse per-block in-progress visual transitions. Empty for any
    /// block not currently animating; populated when a tick handler
    /// starts a transition (e.g. `lifecycle_handler` starting a
    /// Mature→Drying color fade). The mesher reads these to interpolate
    /// visual properties at frame rate; the handler commits + removes
    /// them when the animation duration completes.
    ///
    /// Same sparse pattern as `instance_state`. NOT persisted to disk —
    /// transient by nature (3 seconds at default tick rate).
    pub transitions: std::collections::BTreeMap<u16, BlockTransition>,

    /// v0.24.70-structural-dirty: marks the chunk as having structural
    /// changes since its POI cache was last built. Cleared when a fresh
    /// `poi_cache` is installed via the InstallPoi op. Set whenever a
    /// wood transition happens in `set()` (tree topology changed → POI
    /// cache stale).
    ///
    /// **The end-state of the placeholder cadence in
    /// `POI_REFRESH_INTERVAL_TICKS`.** The doc comment on `poi_cache`
    /// above describes the eventual policy ("rebuild iff
    /// chunk.structural_dirty"). This field is the seam.
    ///
    /// Initialized to `true` for fresh chunks (empty + from_blocks)
    /// so the first lifecycle pass over the chunk builds the cache.
    /// After that pass, the cache is installed and dirty is cleared.
    /// Subsequent rebuilds only happen if a wood block changes.
    ///
    /// Phase A (this RC): set on wood adds/removes only. Future
    /// phases may also dirty on anchorage changes (dirt/stone edits
    /// adjacent to wood), but anchorage is currently tracked by the
    /// Pass 0b tree-root stickiness path so per-tick rebuilds aren't
    /// needed for anchorage transitions.
    ///
    /// Not persisted: rebuilt on chunk load (the cache is None until
    /// the first lifecycle pass, and structural_dirty=true at load
    /// ensures the rebuild fires).
    pub structural_dirty: bool,

    /// v0.24.72: cached "is this chunk entirely air?" flag.
    ///
    /// Computed once in `empty()` / `from_blocks()` and used by the
    /// mesher as a fast-path: an all-air chunk produces an empty mesh
    /// without iterating the 6 axis × 32 slice × 32×32 u/v greedy
    /// meshing loop (which would do ~196K block lookups to produce
    /// nothing).
    ///
    /// **Semantics**: this is a CONSERVATIVE flag. Once it goes
    /// false (a non-air block placed via `set()`), it never returns
    /// to true even if a player digs the chunk back to fully empty.
    /// Recovery to `true` would require an `is_all_air()` re-scan
    /// (~32µs) on every air placement, which we don't pay. The
    /// trade-off: streaming-loaded sky chunks benefit (the dominant
    /// case); long-edited chunks that once held a non-air block
    /// never re-benefit even after being cleared.
    ///
    /// False negatives (cached false when the chunk is actually
    /// all-air) produce a correctness-neutral CPU regression (full
    /// mesher runs, produces an empty result), not a visual bug.
    /// False positives (cached true when the chunk has a non-air
    /// block) WOULD be a visual bug (the block wouldn't render),
    /// which is why we invalidate to false on any non-air `set()`.
    ///
    /// Not persisted: recomputed in from_blocks() at chunk load.
    pub cached_all_air: bool,
    /// r702: true iff EVERY block is a full opaque cube (`is_opaque_occluder`).
    /// A fully-opaque chunk whose 6 face-neighbours are also fully opaque is
    /// completely occluded and can skip meshing. Conservatively false after edits
    /// (never recovered to true without a rescan), so it can only ever cause a
    /// missed skip — never a wrongly-skipped visible chunk. Recomputed at load.
    pub cached_all_opaque: bool,
}

/// Per-block-instance mutable state. Compact (currently 2 bytes) and
/// optional — only blocks that need state get an entry in
/// `Chunk::instance_state`. Pristine blocks pay nothing.
///
/// Field meanings depend on the block type. For plants:
///   - `age_ticks`: how many ticks the plant has aged (drives lifecycle)
///   - `flags` bit 0: "withered" (color shift, growth halted)
///   - `flags` bit 1: "decaying" (leaf-decay state, destined to disappear)
/// For loose/wearable terrain (future erosion):
///   - `age_ticks`: weathering exposure
///   - `flags`: surface-style variants
///
/// We don't enforce a per-block-type interpretation here — the consumer
/// (lifecycle handler, decay handler) knows what to do with these fields
/// for its block type. Future work might split this into per-type structs
/// stored in a typed map; for now one compact struct keeps things simple.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlockInstanceState {
    /// Saturating tick counter. 0 = freshly placed; 255 = ancient.
    pub age_ticks: u8,
    /// Per-block bitfield. Bits are interpreted by the consumer.
    pub flags: u8,
}

impl BlockInstanceState {
    pub const FLAG_WITHERED: u8 = 1 << 0;
    pub const FLAG_DECAYING: u8 = 1 << 1;
    /// Marks a wood block (OakLog/BirchLog) as a tree's root marker —
    /// the single anchor point identifying "this is one tree." Set by
    /// the lifecycle handler's tree-root-discovery pass when a tree is
    /// first noticed; maintained by a stickiness pass that may walk
    /// the marker by 1 block per fire when the underlying anchorage
    /// erodes. Tree growth and degrowth (Phase B) operate on the
    /// graph rooted at this marker.
    pub const FLAG_TREE_ROOT: u8 = 1 << 2;
    /// Set by the root-stickiness pass when a tree's root marker can't
    /// find any face-adjacent wood that touches solid anchorage. The
    /// tree is no longer attached to the world. Phase B will use this
    /// to drive accelerated bottom-up degrowth (or, eventually, real
    /// fall physics via the cluster-physics layer).
    pub const FLAG_TREE_UNANCHORED: u8 = 1 << 3;

    /// Saturating-add to age_ticks. Returns the new age.
    pub fn age(&mut self, dt_ticks: u8) -> u8 {
        self.age_ticks = self.age_ticks.saturating_add(dt_ticks);
        self.age_ticks
    }

    pub fn set_flag(&mut self, flag: u8, on: bool) {
        if on { self.flags |= flag; }
        else  { self.flags &= !flag; }
    }

    pub fn has_flag(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
}

/// In-progress visual transition for a single block. Stored sparsely
/// in `Chunk::transitions` — only blocks currently animating get an
/// entry. The animation runs over a fixed wall-clock duration; the
/// mesher interpolates visual properties (currently only color
/// modulation for plants) between `from_stage` and `to_stage` based
/// on elapsed real time. When the duration completes, the tick handler
/// commits the new stage to `instance_state` and removes the entry.
///
/// Why store `started_tick` rather than mutate `progress` per tick:
///   - The mesher runs at frame rate, the tick driver runs at 2 Hz.
///     A frame-rate-smooth animation can't be driven by a tick-rate
///     state field — would step in chunks. Storing the start tick
///     lets the mesher interpolate continuously.
///   - Distance-band scheduling (chunks beyond render distance tick
///     less often) doesn't perturb the visible animation, because
///     the mesher only animates render-distance chunks anyway. And
///     because the start tick is a deterministic identifier, two
///     clients see identical animations.
///
/// **Not persisted**: transitions are transient (3-second animations).
/// On save/reload, a mid-transition plant just snaps to whatever was
/// in `instance_state` at save time. The handler will eventually fire
/// the transition again from scratch under the new conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockTransition {
    /// The stage value before the transition started. Stored as raw
    /// u8 (not the typed enum) so this struct stays generic across
    /// different transition kinds — e.g. future leaf-decay age
    /// crossovers could reuse this same struct shape.
    pub from_stage: u8,
    /// The stage value at completion.
    pub to_stage: u8,
    /// The TickId at which the transition started. The mesher computes
    /// progress = (current_real_time - started_tick * tick_period)
    /// / animation_duration_real_seconds, clamped to [0, 1].
    pub started_tick: u64,
}

/// Default animation duration in TICKS. At 2 Hz tick rate (default
/// period 0.5s), 6 ticks = 3 real seconds. The handler commits the
/// transition when `current_tick - started_tick >= this value`.
///
/// The mesher uses the same constant (converted to real seconds via
/// the tick period) to compute progress.
///
/// **Tick speed multiplier interaction**: at 1000× tick speed,
/// 6 ticks completes in milliseconds of real time. That's intended:
/// fast-forward should fast-forward animations too. The animation
/// is keyed off TickId, not wall clock, so it scales with simulation
/// speed naturally.
pub const TRANSITION_ANIM_TICKS: u64 = 6;

/// How often the POI cache is rebuilt while we don't yet have
/// structural dirty marking. At default 0.5s tick rate, 10 ticks =
/// 5 seconds of worst-case staleness. When dirty marking lands this
/// constant becomes obsolete — the cache will refresh exactly when a
/// structural change touches the chunk (or a face-neighbor for
/// cluster-spanning concerns), and an idle chunk will never refresh.
pub const POI_REFRESH_INTERVAL_TICKS: u64 = 10;

/// One tree's structural summary as captured by the POI builder.
/// Holds the canonical anchor candidate (lowest-coordinate wood block
/// touching `is_tree_anchorage()` ground), the cluster size, and the
/// world position of the cluster's bottom-most member. The lifecycle
/// handler's stickiness pass uses these to decide whether to walk a
/// tree-root marker.
///
/// Endpoints aren't stored here yet — that's Phase B work. When they
/// land they'll go in this struct so the per-endpoint shrink/growth
/// passes can iterate them without re-walking the cluster.
#[derive(Debug, Clone)]
pub struct TreePoi {
    /// World coordinates of the canonical anchor candidate: the
    /// lowest-y member of the wood cluster that touches an anchorage
    /// block on at least one face. None if the cluster has NO member
    /// touching anchorage (the tree is unanchored).
    pub anchor_candidate: Option<[i32; 3]>,
    /// World coordinates of the cluster's bottom-most member
    /// (lexicographically smallest in canonical (y, z, x) order).
    /// Used by Phase B's bottom-up shrink to find where to start
    /// removing wood when a tree is unanchored.
    pub cluster_bottom: [i32; 3],
    /// Total wood blocks in this tree's cluster. Bounded by
    /// `TREE_CLUSTER_MAX` from the cluster module.
    pub cluster_size: usize,
    /// Cluster diameter (Chebyshev). Used by growth to enforce
    /// `TREE_DIAMETER_CAP`.
    pub diameter: i32,
}

/// Per-chunk cached structural summary. Built by the lifecycle
/// handler's POI pass; consumed by the stickiness, growth, and (in
/// Phase B) shrink passes. The cache is per-chunk but its content
/// can reference world coordinates outside the chunk — a tree
/// rooted in chunk A but with branches in chunk B has its TreePoi
/// stored in A (where the anchor lives), and the summary's positions
/// are world coordinates that may be anywhere reachable from A.
#[derive(Debug, Clone)]
pub struct ChunkPoiCache {
    /// TickId at which this cache was last computed. Refresh policy
    /// uses `tick_id - computed_at_tick >= POI_REFRESH_INTERVAL_TICKS`
    /// (transitional) — when dirty marking lands this becomes a flag
    /// check instead.
    pub computed_at_tick: crate::world::ticks::TickId,
    /// One entry per tree whose anchor wood block lives in THIS
    /// chunk. Trees whose anchor is in another chunk appear in that
    /// other chunk's cache, even if their canopy is in this one.
    /// (Avoids double-listing when a tree spans chunks.)
    pub tree_pois: Vec<TreePoi>,
}

impl ChunkPoiCache {
    /// An empty cache stamped at the given tick. Used when a chunk
    /// has no wood blocks — the refresh "succeeded" with zero results.
    pub fn empty(at_tick: crate::world::ticks::TickId) -> Self {
        Self { computed_at_tick: at_tick, tree_pois: Vec::new() }
    }
}

impl Chunk {
    pub fn empty(coord: ChunkCoord) -> Self {
        Self {
            coord,
            blocks: vec![BlockId::Air; CHUNK_VOLUME],
            dirty: false,
            version: 0,
            explicit_indices: std::collections::BTreeSet::new(),
            plant_indices: std::collections::BTreeSet::new(),
            tree_root_indices: std::collections::BTreeSet::new(),
            wood_indices: std::collections::BTreeSet::new(),
            poi_cache: None,
            instance_state: std::collections::BTreeMap::new(),
            transitions: std::collections::BTreeMap::new(),
            structural_dirty: true,
            // v0.24.72: empty chunk = all air by definition.
            cached_all_air: true,
            // r702: all-air is the opposite of all-opaque.
            cached_all_opaque: false,
        }
    }

    /// Construct from a fully-populated block array. Used by generators.
    /// Marks all blocks as procedural (no explicit overrides yet).
    /// Scans for plant blocks to seed the plant_indices index, AND for
    /// wood blocks to seed wood_indices. Both indices are then
    /// maintained incrementally by `set()` — the scan happens exactly
    /// once per chunk lifetime.
    pub fn from_blocks(coord: ChunkCoord, blocks: Vec<BlockId>) -> Self {
        debug_assert_eq!(blocks.len(), CHUNK_VOLUME, "block array must be CHUNK_VOLUME");
        // One-time scan to populate the plant_indices and wood_indices
        // inverted indices. After this point, both are maintained
        // incrementally on `set()`. Single pass over the block array.
        let mut plant_indices = std::collections::BTreeSet::new();
        let mut wood_indices = std::collections::BTreeSet::new();
        // v0.24.72: compute cached_all_air in the same single pass —
        // free since we're already walking blocks.
        let mut all_air = true;
        let mut all_opaque = true; // r702
        for (i, b) in blocks.iter().enumerate() {
            if b.has_subvoxels() {
                plant_indices.insert(i as u16);
            }
            if matches!(b, BlockId::OakLog | BlockId::BirchLog) {
                wood_indices.insert(i as u16);
            }
            if !b.is_air() {
                all_air = false;
            }
            if !b.is_opaque_occluder() {
                all_opaque = false;
            }
        }
        Self {
            coord, blocks,
            dirty: false, version: 0,
            explicit_indices: std::collections::BTreeSet::new(),
            plant_indices,
            // Empty: tree-root markers are added by the lifecycle
            // handler's discovery pass on first encounter, not by
            // worldgen. A freshly-generated chunk full of trees has
            // no markers; the first lifecycle pass over those chunks
            // populates them.
            tree_root_indices: std::collections::BTreeSet::new(),
            wood_indices,
            poi_cache: None,
            instance_state: std::collections::BTreeMap::new(),
            transitions: std::collections::BTreeMap::new(),
            // v0.24.70: fresh chunk → dirty, so first lifecycle pass
            // builds the POI cache. Cleared on InstallPoi.
            structural_dirty: true,
            // v0.24.72: computed in the scan above. Air-only chunks
            // (sky, deep underground) get the mesher fast-path.
            cached_all_air: all_air,
            // r702: computed in the same scan — feeds the fully-buried skip.
            cached_all_opaque: all_opaque,
        }
    }

    /// Local-coordinate flat index. Caller is responsible for bounds.
    #[inline]
    pub fn block_index(x: i32, y: i32, z: i32) -> usize {
        let cs = CHUNK_SIZE as usize;
        x as usize + z as usize * cs + y as usize * cs * cs
    }

    /// Read a block at chunk-local coordinates. Out-of-bounds returns Air.
    pub fn get(&self, x: i32, y: i32, z: i32) -> BlockId {
        if !in_bounds(x, y, z) { return BlockId::Air; }
        self.blocks[Self::block_index(x, y, z)]
    }

    /// Recompute `cached_all_air` from the current block contents.
    ///
    /// Use after direct mutation of `self.blocks` (tests, batch edits)
    /// that bypassed `Chunk::set()`. `set()` already invalidates the
    /// cache when a non-air block is placed; this method is for paths
    /// that touch the blocks vec directly and need the cache to be
    /// consistent before the next mesher run.
    ///
    /// RC42.1: introduced because RC37's `mesh_chunk_greedy` fast-path
    /// trusted the cache; tests that did `chunk.blocks[idx] = Stone`
    /// got empty meshes back. The proper fix is to call this after
    /// direct mutation, OR migrate to `set()` which handles the cache
    /// automatically.
    pub fn recompute_cached_all_air(&mut self) {
        // r702: refresh both cached flags in one pass (both are O(CHUNK_VOLUME)).
        let mut all_air = true;
        let mut all_opaque = true;
        for b in self.blocks.iter() {
            if !b.is_air() {
                all_air = false;
            }
            if !b.is_opaque_occluder() {
                all_opaque = false;
            }
        }
        self.cached_all_air = all_air;
        self.cached_all_opaque = all_opaque;
    }

    /// Was this block explicitly set (player edit, simulation, etc.) or is
    /// it still the procedural value from worldgen? The persistence layer
    /// uses this to decide what to save.
    pub fn is_explicit(&self, x: i32, y: i32, z: i32) -> bool {
        if !in_bounds(x, y, z) { return false; }
        let idx = Self::block_index(x, y, z) as u16;
        self.explicit_indices.contains(&idx)
    }

    /// Write a block at chunk-local coordinates. Out-of-bounds is a no-op.
    /// Marks the chunk dirty, bumps version, tags this index as explicit,
    /// and updates the plant_indices inverted index.
    pub fn set(&mut self, x: i32, y: i32, z: i32, block: BlockId) {
        if !in_bounds(x, y, z) { return; }
        let idx = Self::block_index(x, y, z);
        let old = self.blocks[idx];
        if old != block {
            self.blocks[idx] = block;
            self.dirty = true;
            self.version += 1;

            // v0.24.72: invalidate the all-air cache when a non-air
            // block is placed. We don't recover the cache to true on
            // removal (would require an O(CHUNK_VOLUME) re-scan per
            // call). The flag is permitted to be conservatively
            // false; the mesher just runs full work and produces a
            // (possibly empty) result. False positives — claiming
            // all-air when there's a non-air block — WOULD cause the
            // mesher to wrongly skip a chunk with content, so we
            // strictly invalidate here.
            if !block.is_air() {
                self.cached_all_air = false;
            }
            // r702: invalidate the all-opaque cache when a non-opaque block
            // (air, glass, plant, fluid) is placed. Conservatively left false
            // otherwise (no recover-to-true rescan) — the buried skip then just
            // meshes the chunk normally. A false NEGATIVE here is safe (missed
            // skip); a false positive would punch a hole, so we only clear.
            if !block.is_opaque_occluder() {
                self.cached_all_opaque = false;
            }

            // Maintain the plant inverted index: if the block-type
            // category changed, update membership.
            let was_plant = old.has_subvoxels();
            let is_plant = block.has_subvoxels();
            if was_plant && !is_plant {
                self.plant_indices.remove(&(idx as u16));
            } else if !was_plant && is_plant {
                self.plant_indices.insert(idx as u16);
            }

            // Maintain the tree-root inverted index. When a wood block
            // is replaced by anything else (Air, leaves, the other wood
            // species, anything), it can no longer be a tree-root
            // marker. We don't add to the index here — adds go through
            // `set_tree_root_marker()` which knows the flag rules.
            //
            // Why check is_wood specifically: leaves and plants don't
            // qualify as tree-root anchors, only the log/wood types.
            let was_wood = matches!(old, BlockId::OakLog | BlockId::BirchLog);
            let is_wood  = matches!(block, BlockId::OakLog | BlockId::BirchLog);

            // Maintain wood_indices in lockstep with the actual block
            // type. This is the cheap, complete-coverage index used by
            // POI build to enumerate wood positions per chunk without
            // a 32K linear scan.
            if was_wood && !is_wood {
                self.wood_indices.remove(&(idx as u16));
                // v0.24.70: wood removal changes tree topology → POI
                // cache stale.
                self.structural_dirty = true;
            } else if !was_wood && is_wood {
                self.wood_indices.insert(idx as u16);
                // v0.24.70: wood addition changes tree topology → POI
                // cache stale.
                self.structural_dirty = true;
            }
            // Note: a species swap (oak → birch) keeps the entry in
            // wood_indices unchanged (both are wood). But the cluster
            // identity changes (species is part of the cluster rule),
            // so we still need a POI rebuild. Catch this case too.
            if was_wood && is_wood && old != block {
                self.structural_dirty = true;
            }
            // Note: a species swap (oak → birch) keeps the entry in
            // wood_indices unchanged (both are wood); we still drop the
            // tree_root marker below because the cluster identity
            // changed.

            if was_wood && !is_wood {
                if self.tree_root_indices.remove(&(idx as u16)) {
                    // Also clear the flag bit in instance_state so a
                    // future block placed at this index doesn't
                    // inherit a stale TREE_ROOT marker.
                    if let Some(state) = self.instance_state.get_mut(&(idx as u16)) {
                        state.set_flag(BlockInstanceState::FLAG_TREE_ROOT, false);
                        state.set_flag(BlockInstanceState::FLAG_TREE_UNANCHORED, false);
                        if state.flags == 0 && state.age_ticks == 0 {
                            self.instance_state.remove(&(idx as u16));
                        }
                    }
                }
            } else if was_wood && is_wood && old != block {
                // Species change (oak → birch or vice-versa): the old
                // tree's root marker is no longer valid for the new
                // species. Drop it; the new species will get a fresh
                // marker on next discovery.
                self.tree_root_indices.remove(&(idx as u16));
                if let Some(state) = self.instance_state.get_mut(&(idx as u16)) {
                    state.set_flag(BlockInstanceState::FLAG_TREE_ROOT, false);
                    state.set_flag(BlockInstanceState::FLAG_TREE_UNANCHORED, false);
                }
            }
        }
        // Always mark explicit on `set` — even if the block "happens to
        // match" the procedural value, the player meant to set it. (A future
        // tool that toggles "revert to procedural" would clear this.)
        self.explicit_indices.insert(idx as u16);
    }

    /// Set or clear the tree-root marker on a wood block. Adds/removes
    /// the index from `tree_root_indices` and toggles
    /// `FLAG_TREE_ROOT` in `instance_state`. Caller must guarantee the
    /// block at this index is currently OakLog or BirchLog.
    ///
    /// If `on` is true and the block isn't wood, this is a no-op (with
    /// a debug-build assertion).
    pub fn set_tree_root_marker(&mut self, x: i32, y: i32, z: i32, on: bool) {
        if !in_bounds(x, y, z) { return; }
        let idx = Self::block_index(x, y, z) as u16;
        let here = self.blocks[idx as usize];
        if on {
            debug_assert!(
                matches!(here, BlockId::OakLog | BlockId::BirchLog),
                "set_tree_root_marker(true) on non-wood block {:?}", here,
            );
            if !matches!(here, BlockId::OakLog | BlockId::BirchLog) { return; }
            self.tree_root_indices.insert(idx);
            let state = self.instance_state.entry(idx).or_default();
            state.set_flag(BlockInstanceState::FLAG_TREE_ROOT, true);
            self.dirty = true;
            self.version += 1;
        } else {
            self.tree_root_indices.remove(&idx);
            if let Some(state) = self.instance_state.get_mut(&idx) {
                state.set_flag(BlockInstanceState::FLAG_TREE_ROOT, false);
                state.set_flag(BlockInstanceState::FLAG_TREE_UNANCHORED, false);
                if state.flags == 0 && state.age_ticks == 0 {
                    self.instance_state.remove(&idx);
                }
            }
            self.dirty = true;
            self.version += 1;
        }
    }

    /// Set or clear the unanchored flag on a tree root marker. Caller
    /// must have already established the marker via
    /// `set_tree_root_marker(.., true)`. No-op on non-marker blocks.
    pub fn set_tree_unanchored(&mut self, x: i32, y: i32, z: i32, on: bool) {
        if !in_bounds(x, y, z) { return; }
        let idx = Self::block_index(x, y, z) as u16;
        if !self.tree_root_indices.contains(&idx) { return; }
        let state = self.instance_state.entry(idx).or_default();
        state.set_flag(BlockInstanceState::FLAG_TREE_UNANCHORED, on);
        self.dirty = true;
        self.version += 1;
    }

    /// Convert a flat block index back to chunk-local (x, y, z).
    #[inline]
    pub fn index_to_xyz(idx: u16) -> (i32, i32, i32) {
        let cs = CHUNK_SIZE as usize;
        let i = idx as usize;
        let x = (i % cs) as i32;
        let z = ((i / cs) % cs) as i32;
        let y = (i / (cs * cs)) as i32;
        (x, y, z)
    }

    /// How many explicit (non-procedural) blocks does this chunk hold?
    /// Useful for storage decisions and diagnostics.
    pub fn explicit_count(&self) -> usize {
        self.explicit_indices.len()
    }

    /// Read the instance state for a block at chunk-local coords. Returns
    /// `None` if the block has no entry (i.e. default state). Most blocks
    /// have no entry — the sparse map saves memory.
    pub fn get_instance_state(&self, x: i32, y: i32, z: i32) -> Option<BlockInstanceState> {
        if !in_bounds(x, y, z) { return None; }
        let idx = Self::block_index(x, y, z) as u16;
        self.instance_state.get(&idx).copied()
    }

    /// Read the instance state, or return default if no entry. Convenient
    /// for tick handlers that want "default if pristine".
    pub fn get_instance_state_or_default(&self, x: i32, y: i32, z: i32) -> BlockInstanceState {
        self.get_instance_state(x, y, z).unwrap_or_default()
    }

    /// Write the instance state for a block. If `state` is the default
    /// (all-zero), the entry is REMOVED rather than stored — keeping the
    /// sparse map sparse for blocks that revert to pristine state.
    pub fn set_instance_state(&mut self, x: i32, y: i32, z: i32, state: BlockInstanceState) {
        if !in_bounds(x, y, z) { return; }
        let idx = Self::block_index(x, y, z) as u16;
        if state == BlockInstanceState::default() {
            self.instance_state.remove(&idx);
        } else {
            self.instance_state.insert(idx, state);
        }
        self.dirty = true;
        self.version += 1;
    }

    /// Same as set_instance_state but takes a flat block index. Used by
    /// tick handlers iterating over the inverted plant index.
    pub fn set_instance_state_by_index(&mut self, idx: u16, state: BlockInstanceState) {
        if state == BlockInstanceState::default() {
            self.instance_state.remove(&idx);
        } else {
            self.instance_state.insert(idx, state);
        }
        self.dirty = true;
        self.version += 1;
    }

    /// Convenience: advance a block's age by `dt_ticks` saturating, set
    /// flags optionally, and store back. Returns the new state.
    pub fn age_block(&mut self, idx: u16, dt_ticks: u8) -> BlockInstanceState {
        let mut s = self.instance_state.get(&idx).copied().unwrap_or_default();
        s.age(dt_ticks);
        self.set_instance_state_by_index(idx, s);
        s
    }

    /// Start a visual transition for the block at `idx`. If a transition
    /// is already in progress for this block, it gets replaced — the
    /// caller is responsible for not stomping on a desired animation.
    /// (The lifecycle handler avoids this by checking `is_transitioning`
    /// before calling.)
    ///
    /// `started_tick` should be the current `TickId.raw()`. The mesher
    /// uses it to compute interpolation progress at frame rate.
    pub fn start_transition(&mut self, idx: u16, transition: BlockTransition) {
        self.transitions.insert(idx, transition);
        self.dirty = true;
        self.version += 1;
    }

    /// Remove a transition entry (e.g. on commit). No-op if absent.
    /// Caller is typically the tick handler, which has just applied
    /// the to_stage to the block's `instance_state`.
    pub fn finish_transition(&mut self, idx: u16) {
        if self.transitions.remove(&idx).is_some() {
            self.dirty = true;
            self.version += 1;
        }
    }

    /// Whether a transition is currently in progress for the given
    /// block index. Cheap — single map lookup.
    pub fn is_transitioning(&self, idx: u16) -> bool {
        self.transitions.contains_key(&idx)
    }

    /// Returns true if every block in this chunk is air. Used to decide
    /// whether the chunk produces any geometry at all.
    pub fn is_all_air(&self) -> bool {
        self.blocks.iter().all(|b| b.is_air())
    }

    /// Clone for the copy-on-write block-edit path. Marks the result dirty
    /// Bump version only, without changing block content. Used by the
    /// chunk manager to invalidate neighbor chunks' meshes after a
    /// boundary-edit in this chunk: the neighbors didn't change their
    /// blocks, but their boundary FACES depend on this chunk's blocks,
    /// so their meshes are now stale.
    pub fn bump_version_only(&mut self) {
        self.version += 1;
    }

    /// Clones this chunk for a Copy-on-Write edit. Sets dirty=true
    /// and bumps version, so the COW clone is distinguishable from the
    /// original by version.
    pub fn clone_for_edit(&self) -> Self {
        let mut c = self.clone();
        c.dirty = true;
        c.version += 1;
        c
    }
}

#[inline]
fn in_bounds(x: i32, y: i32, z: i32) -> bool {
    x >= 0 && x < CHUNK_SIZE
        && y >= 0 && y < CHUNK_SIZE
        && z >= 0 && z < CHUNK_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coord_from_block_pos_handles_negatives() {
        // Block at world (-1, -1, -1) belongs to chunk (-1, -1, -1).
        assert_eq!(
            ChunkCoord::from_block_pos(-1, -1, -1),
            ChunkCoord::new(-1, -1, -1),
        );
        // Block at (0, 0, 0) is in chunk (0, 0, 0).
        assert_eq!(
            ChunkCoord::from_block_pos(0, 0, 0),
            ChunkCoord::new(0, 0, 0),
        );
        // Block at (CHUNK_SIZE, 0, 0) is in chunk (1, 0, 0).
        assert_eq!(
            ChunkCoord::from_block_pos(CHUNK_SIZE, 0, 0),
            ChunkCoord::new(1, 0, 0),
        );
    }

    #[test]
    fn block_origin_is_chunk_min_corner() {
        let c = ChunkCoord::new(2, -1, 3);
        assert_eq!(c.block_origin(), (64, -32, 96));
    }

    #[test]
    fn empty_chunk_is_all_air() {
        let c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        assert!(c.is_all_air());
        assert_eq!(c.blocks.len(), CHUNK_VOLUME);
    }

    #[test]
    fn set_increments_version_only_on_change() {
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        assert_eq!(c.version, 0);
        c.set(0, 0, 0, BlockId::Stone);
        assert_eq!(c.version, 1);
        c.set(0, 0, 0, BlockId::Stone); // no change
        assert_eq!(c.version, 1);
        c.set(0, 0, 0, BlockId::Dirt);
        assert_eq!(c.version, 2);
    }

    #[test]
    fn plant_index_tracks_subvoxel_blocks() {
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        assert!(c.plant_indices.is_empty());
        // Adding a non-plant block: index stays empty.
        c.set(0, 0, 0, BlockId::Stone);
        assert!(c.plant_indices.is_empty());
        // Adding a plant block: index gains an entry.
        c.set(1, 0, 0, BlockId::Grass);
        assert_eq!(c.plant_indices.len(), 1);
        // Adding more plant blocks: index grows.
        c.set(2, 0, 0, BlockId::Fern);
        c.set(3, 0, 0, BlockId::Flower);
        assert_eq!(c.plant_indices.len(), 3);
        // Removing a plant block (replace with non-plant): index shrinks.
        c.set(2, 0, 0, BlockId::Stone);
        assert_eq!(c.plant_indices.len(), 2);
        // Replacing one plant with another plant: index size stays.
        c.set(1, 0, 0, BlockId::TallGrass);
        assert_eq!(c.plant_indices.len(), 2);
    }

    #[test]
    fn from_blocks_seeds_plant_index() {
        let mut blocks = vec![BlockId::Air; CHUNK_VOLUME];
        blocks[Chunk::block_index(0, 0, 0)] = BlockId::Grass;
        blocks[Chunk::block_index(5, 5, 5)] = BlockId::Fern;
        blocks[Chunk::block_index(10, 10, 10)] = BlockId::Stone;
        let c = Chunk::from_blocks(ChunkCoord::new(0, 0, 0), blocks);
        assert_eq!(c.plant_indices.len(), 2);
    }

    #[test]
    fn index_to_xyz_round_trips_with_block_index() {
        for x in 0..32 {
            for y in 0..32 {
                for z in 0..32 {
                    let idx = Chunk::block_index(x, y, z) as u16;
                    let (rx, ry, rz) = Chunk::index_to_xyz(idx);
                    assert_eq!((rx, ry, rz), (x, y, z),
                        "round-trip failed at ({}, {}, {})", x, y, z);
                }
            }
        }
    }

    #[test]
    fn instance_state_default_returns_none() {
        let c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        assert!(c.get_instance_state(5, 5, 5).is_none());
        assert_eq!(c.get_instance_state_or_default(5, 5, 5),
            BlockInstanceState::default());
    }

    #[test]
    fn instance_state_round_trips() {
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let s = BlockInstanceState { age_ticks: 42, flags: 0b0000_0011 };
        c.set_instance_state(7, 8, 9, s);
        assert_eq!(c.get_instance_state(7, 8, 9), Some(s));
        // Other coords still default.
        assert!(c.get_instance_state(0, 0, 0).is_none());
    }

    #[test]
    fn instance_state_default_writes_remove_entry() {
        // Setting a block's instance state to default should REMOVE it
        // from the sparse map (keeping the map sparse).
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let s = BlockInstanceState { age_ticks: 10, flags: 0 };
        c.set_instance_state(1, 1, 1, s);
        assert_eq!(c.instance_state.len(), 1);
        c.set_instance_state(1, 1, 1, BlockInstanceState::default());
        assert_eq!(c.instance_state.len(), 0);
    }

    #[test]
    fn age_block_increments_saturating() {
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let idx = Chunk::block_index(2, 2, 2) as u16;
        let s = c.age_block(idx, 5);
        assert_eq!(s.age_ticks, 5);
        let s = c.age_block(idx, 10);
        assert_eq!(s.age_ticks, 15);
        // Saturating add: never overflows.
        let s = c.age_block(idx, 250);
        assert_eq!(s.age_ticks, 255);
        let s = c.age_block(idx, 10);
        assert_eq!(s.age_ticks, 255);
    }

    #[test]
    fn instance_state_flags_round_trip() {
        let mut s = BlockInstanceState::default();
        s.set_flag(BlockInstanceState::FLAG_WITHERED, true);
        assert!(s.has_flag(BlockInstanceState::FLAG_WITHERED));
        assert!(!s.has_flag(BlockInstanceState::FLAG_DECAYING));
        s.set_flag(BlockInstanceState::FLAG_DECAYING, true);
        assert!(s.has_flag(BlockInstanceState::FLAG_WITHERED));
        assert!(s.has_flag(BlockInstanceState::FLAG_DECAYING));
        s.set_flag(BlockInstanceState::FLAG_WITHERED, false);
        assert!(!s.has_flag(BlockInstanceState::FLAG_WITHERED));
        assert!(s.has_flag(BlockInstanceState::FLAG_DECAYING));
    }

    #[test]
    fn empty_chunk_has_no_transitions() {
        let c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        assert!(c.transitions.is_empty());
        assert!(!c.is_transitioning(0));
    }

    #[test]
    fn start_and_finish_transition_round_trip() {
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let idx = Chunk::block_index(5, 5, 5) as u16;
        let t = BlockTransition {
            from_stage: 2,  // Mature
            to_stage: 3,    // Drying
            started_tick: 100,
        };
        c.start_transition(idx, t);
        assert!(c.is_transitioning(idx));
        assert_eq!(c.transitions.get(&idx).copied(), Some(t));

        c.finish_transition(idx);
        assert!(!c.is_transitioning(idx));
        assert!(c.transitions.is_empty());
    }

    #[test]
    fn start_transition_replaces_existing() {
        // Stomping on an in-progress transition is allowed (the LIFO
        // pattern — newest transition wins). Caller is responsible for
        // checking is_transitioning if it doesn't want to stomp.
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let idx = 42u16;
        let t1 = BlockTransition { from_stage: 1, to_stage: 2, started_tick: 50 };
        let t2 = BlockTransition { from_stage: 2, to_stage: 3, started_tick: 100 };
        c.start_transition(idx, t1);
        c.start_transition(idx, t2);
        assert_eq!(c.transitions.get(&idx).copied(), Some(t2));
    }

    #[test]
    fn finish_transition_is_noop_when_absent() {
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let v_before = c.version;
        c.finish_transition(99);
        // Version should NOT bump on a no-op finish.
        assert_eq!(c.version, v_before);
    }

    #[test]
    fn transition_marks_chunk_dirty_and_versions() {
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let v0 = c.version;
        let t = BlockTransition { from_stage: 0, to_stage: 1, started_tick: 0 };
        c.start_transition(7, t);
        assert!(c.dirty);
        assert!(c.version > v0);

        let v1 = c.version;
        c.finish_transition(7);
        assert!(c.version > v1, "finish should bump version when entry existed");
    }

    #[test]
    fn transitions_are_independent_per_block() {
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        c.start_transition(1, BlockTransition { from_stage: 0, to_stage: 1, started_tick: 10 });
        c.start_transition(2, BlockTransition { from_stage: 1, to_stage: 2, started_tick: 20 });
        c.start_transition(3, BlockTransition { from_stage: 2, to_stage: 3, started_tick: 30 });
        assert_eq!(c.transitions.len(), 3);
        c.finish_transition(2);
        assert_eq!(c.transitions.len(), 2);
        assert!(c.is_transitioning(1));
        assert!(!c.is_transitioning(2));
        assert!(c.is_transitioning(3));
    }

    // ===== v0.24.70-structural-dirty tests =====

    #[test]
    fn structural_dirty_starts_true_on_fresh_chunks() {
        let c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        assert!(c.structural_dirty,
            "fresh empty chunk should be dirty so first POI pass builds the cache");

        let c2 = Chunk::from_blocks(
            ChunkCoord::new(0, 0, 0),
            vec![BlockId::Air; CHUNK_VOLUME],
        );
        assert!(c2.structural_dirty,
            "fresh from_blocks chunk should be dirty so first POI pass builds the cache");
    }

    #[test]
    fn structural_dirty_set_on_wood_add() {
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        // Pretend the first POI pass has run and cleared dirty.
        c.structural_dirty = false;
        // Adding wood should re-dirty.
        c.set(1, 1, 1, BlockId::OakLog);
        assert!(c.structural_dirty,
            "adding wood should re-dirty the chunk for POI rebuild");
    }

    #[test]
    fn structural_dirty_set_on_wood_remove() {
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        c.set(2, 2, 2, BlockId::OakLog);
        // Pretend the POI pass has run and cleared dirty.
        c.structural_dirty = false;
        // Removing the wood should re-dirty.
        c.set(2, 2, 2, BlockId::Air);
        assert!(c.structural_dirty,
            "removing wood should re-dirty the chunk for POI rebuild");
    }

    #[test]
    fn structural_dirty_set_on_wood_species_swap() {
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        c.set(3, 3, 3, BlockId::OakLog);
        c.structural_dirty = false;
        // Species swap stays in wood_indices but cluster identity
        // changes — POI cache is stale.
        c.set(3, 3, 3, BlockId::BirchLog);
        assert!(c.structural_dirty,
            "wood species swap should re-dirty for cluster-identity reasons");
    }

    #[test]
    fn structural_dirty_NOT_set_on_unrelated_edits() {
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        c.structural_dirty = false;
        // Non-wood edits should NOT dirty: stone, dirt, grass, plants
        // don't affect the POI cache (which is wood-cluster-only).
        c.set(1, 1, 1, BlockId::Stone);
        c.set(2, 2, 2, BlockId::Dirt);
        c.set(3, 3, 3, BlockId::Grass);
        assert!(!c.structural_dirty,
            "non-wood edits should not dirty the POI cache");
    }

    #[test]
    fn structural_dirty_NOT_set_on_noop_set() {
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        c.set(1, 1, 1, BlockId::OakLog);
        c.structural_dirty = false;
        // Same block set again — no actual change.
        c.set(1, 1, 1, BlockId::OakLog);
        assert!(!c.structural_dirty,
            "no-op set on existing wood should not dirty");
    }

    // ===== v0.24.72 cached_all_air tests =====

    #[test]
    fn cached_all_air_true_for_empty_chunk() {
        let c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        assert!(c.cached_all_air, "empty() chunk is all air by definition");
    }

    #[test]
    fn cached_all_air_true_for_all_air_from_blocks() {
        let c = Chunk::from_blocks(
            ChunkCoord::new(0, 0, 0),
            vec![BlockId::Air; CHUNK_VOLUME],
        );
        assert!(c.cached_all_air,
            "from_blocks with all-air vector → cached_all_air=true");
    }

    #[test]
    fn cached_all_air_false_for_non_air_from_blocks() {
        let mut blocks = vec![BlockId::Air; CHUNK_VOLUME];
        blocks[Chunk::block_index(5, 5, 5)] = BlockId::Stone;
        let c = Chunk::from_blocks(ChunkCoord::new(0, 0, 0), blocks);
        assert!(!c.cached_all_air,
            "from_blocks with one stone block → cached_all_air=false");
    }

    #[test]
    fn cached_all_air_invalidated_on_non_air_set() {
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        assert!(c.cached_all_air);
        c.set(1, 1, 1, BlockId::Stone);
        assert!(!c.cached_all_air,
            "placing a stone block should invalidate cached_all_air");
    }

    #[test]
    fn cached_all_air_NOT_recovered_on_block_removal() {
        // Document the conservative semantics: once invalidated, the
        // flag stays false even if the chunk becomes all-air again.
        // This is a deliberate trade-off (no per-set re-scan cost).
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        c.set(1, 1, 1, BlockId::Stone);
        assert!(!c.cached_all_air);
        // Remove the block; chunk is genuinely all-air now.
        c.set(1, 1, 1, BlockId::Air);
        assert!(c.is_all_air(), "the chunk genuinely IS all-air after removal");
        assert!(!c.cached_all_air,
            "but the cached flag is conservatively stale (false-negative is OK)");
    }

    #[test]
    fn cached_all_air_unaffected_by_noop_air_set() {
        let c0 = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let mut c = c0.clone();
        // Setting Air over Air at a position is a no-op (block didn't
        // change). The cached_all_air flag should remain true.
        c.set(0, 0, 0, BlockId::Air);
        assert!(c.cached_all_air,
            "no-op set (air over air) should not invalidate cached_all_air");
    }
}
