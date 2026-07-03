//! Basic entity scaffold (v0.24.58-entity-scaffold).
//!
//! Defines the data shape for non-grid-aligned things that exist in
//! the world: critters, dropped items, projectiles. This is the
//! "entity substrate" referenced in DESIGN_NOTES.md L1286-1304 —
//! currently UNWIRED (no spawning, no rendering, no ticking from
//! real code) but the API surface is defined so future RCs can
//! layer on top without retrofitting type signatures.
//!
//! ## Design decisions captured here
//!
//! - **Entities are NOT chunk-scoped at the data model level.** The
//!   docs note (L165-168) "entity spawning queries WorldModel
//!   fields, not biome enums." That implies a flat global store,
//!   not per-chunk Vecs. Spatial indexing for queries can layer on
//!   top later without changing the storage shape.
//! - **Position is f32 continuous space.** Block coords are i32;
//!   entities live between cells. At f32 precision (~24-bit
//!   mantissa) the sub-millimeter resolution holds out to world
//!   coords of ~16M blocks, which we won't reach this decade.
//! - **No SoA yet.** Vec<Entity> is fine until entity count exceeds
//!   ~1000 simultaneously. Switching to SoA (separate position,
//!   velocity, kind arrays) is a localized change inside EntityStore.
//! - **Tick handler is defined as a free function**, not bound to
//!   a particular registry registration. The integration point
//!   (whoever ends up wiring this in) decides storage lifecycle
//!   and locking.
//!
//! ## What's intentionally NOT here
//!
//! - **No rendering.** Entities can't currently be drawn — that's a
//!   separate pipeline (instanced draw, billboards, or per-entity
//!   meshes). The chunk-mesh pipeline assumes grid-aligned voxels.
//! - **No collision with voxel grid.** `advance_entities` integrates
//!   in free space. Real physics needs to consult the octree.
//! - **No serialization.** EntityStore isn't persisted across
//!   sessions. Saved-world support is a separate increment.
//! - **No multiplayer authority.** Per DESIGN_NOTES.md L309-354,
//!   entity state should eventually be server-authoritative. Today
//!   there's only one local copy.
//! - **No spatial query indexing.** `nearby` is a linear scan;
//!   acceptable while entity counts stay small.

use std::sync::atomic::{AtomicU64, Ordering};

/// Unique entity identifier. Allocated monotonically by EntityStore.
/// `u64` doesn't wrap in any practical scenario (would need ~10¹⁹
/// spawns).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntityId(pub u64);

/// What kind of entity. Open for extension; the variants chosen here
/// give the next few RCs concrete targets to wire against.
///
/// Future variants will follow the docs' design guidance — entity types
/// keyed by behavior class, not by biome or visual appearance. Visual
/// differentiation happens at render time via the per-kind data here
/// and per-entity state in `Entity`.
///
/// **Choice of enum-with-data over external table:** Rust idiomatic
/// pattern for variant-specific state. The Copy bound is preserved
/// because BlockId is Copy and u32 is Copy. Renderer / physics code
/// matches on the variant naturally. The downside (less friendly to
/// SoA layouts) is filed for whenever we cross ~1000-entity threshold
/// and need to revisit storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityKind {
    /// A small wandering creature. No AI, no rendering, no physics
    /// integration with the voxel grid in this RC. Exists to validate
    /// the scaffold compiles end-to-end.
    Critter,
    /// A block visible in mid-air after the player broke its source.
    /// Carries the BlockId so the renderer knows what color to paint
    /// it and pickup logic knows what to add to inventory. Despawns
    /// after a fixed age (TBD — probably ~600 ticks = 5 minutes at
    /// 2 Hz) so the world doesn't fill with orphan items.
    DroppedItem { item: crate::world::blocks::BlockId },
    /// A short-lived particle: tossed pebble, ember, leaf. Lifetime
    /// is determined by `Entity.age_ticks`; the spawn site decides
    /// the despawn rule. Renderer typically draws particles as
    /// billboards rather than meshes.
    Particle,
}

/// One entity's full state. Mob, projectile, item-drop — all share
/// this shape today. As behaviors diverge we'll likely add per-kind
/// state slots (a Critter might track its target, a Projectile its
/// originator), but keep the common case in the base struct.
#[derive(Debug, Clone, Copy)]
pub struct Entity {
    pub id: EntityId,
    pub kind: EntityKind,
    /// World position in continuous space, blocks units. The block at
    /// integer (X, Y, Z) occupies the cube [X, X+1) × [Y, Y+1) × [Z, Z+1).
    pub position: [f32; 3],
    /// Velocity in blocks/second.
    pub velocity: [f32; 3],
    /// Ticks since spawn. `u32` lasts about 68 years at 2 Hz, which is
    /// the entity-substrate equivalent of "ought to be enough for
    /// anyone." Useful for despawn timers, age-keyed behavior changes,
    /// debug output.
    pub age_ticks: u32,
}

impl Entity {
    pub fn new(id: EntityId, kind: EntityKind, position: [f32; 3]) -> Self {
        Self {
            id,
            kind,
            position,
            velocity: [0.0, 0.0, 0.0],
            age_ticks: 0,
        }
    }

    /// Bounding box for this entity in world space. Half-extents are
    /// per-kind so physics and renderer can branch on the variant
    /// without duplicating the dimension table.
    ///
    /// Numbers chosen to be reasonable defaults; future RCs may
    /// promote these to per-instance data (a baby critter is smaller
    /// than an adult). For now per-kind is enough granularity.
    pub fn aabb(&self) -> Aabb {
        let half = match self.kind {
            EntityKind::Critter => [0.40, 0.50, 0.40],
            // Dropped items are visually small floating cubes —
            // ¼-block cube centered on the entity position.
            EntityKind::DroppedItem { .. } => [0.125, 0.125, 0.125],
            // Particles are visually 1/16-block specks.
            EntityKind::Particle => [0.0625, 0.0625, 0.0625],
        };
        Aabb::from_center_half_extents(self.position, half)
    }
}

/// Axis-aligned bounding box in world space. Foundational primitive
/// for entity-vs-voxel collision, entity-vs-entity overlap tests, and
/// renderer culling (does this entity's AABB intersect the view
/// frustum?).
///
/// Convention: `min` is INCLUSIVE, `max` is INCLUSIVE. So a unit-block
/// AABB at the origin is `min=[0,0,0], max=[1,1,1]` and covers exactly
/// that 1-block volume. (Voxel cell semantics differ — the cell at
/// block coord X occupies [X, X+1) — so when bridging, see
/// `Aabb::block_range`.)
///
/// Operations are exact-AABB; no bullet-style margin / penetration
/// depth tracking. Add that in a physics-specific layer if needed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    pub fn new(min: [f32; 3], max: [f32; 3]) -> Self {
        Self { min, max }
    }

    /// Construct from a center point and per-axis half-extents.
    /// Useful for entity AABBs since we store center positions and
    /// know dimensions per-kind.
    pub fn from_center_half_extents(center: [f32; 3], half: [f32; 3]) -> Self {
        Self {
            min: [center[0] - half[0], center[1] - half[1], center[2] - half[2]],
            max: [center[0] + half[0], center[1] + half[1], center[2] + half[2]],
        }
    }

    /// Standard SAT: AABBs overlap iff they overlap on every axis.
    /// Edge contact (`a.max == b.min`) counts as intersection — this
    /// matches the "inclusive" convention. If you need strict
    /// non-touching overlap, check `intersects && !touches_edge`.
    pub fn intersects(&self, other: &Aabb) -> bool {
        !(self.max[0] < other.min[0]
            || self.min[0] > other.max[0]
            || self.max[1] < other.min[1]
            || self.min[1] > other.max[1]
            || self.max[2] < other.min[2]
            || self.min[2] > other.max[2])
    }

    pub fn contains_point(&self, p: [f32; 3]) -> bool {
        p[0] >= self.min[0]
            && p[0] <= self.max[0]
            && p[1] >= self.min[1]
            && p[1] <= self.max[1]
            && p[2] >= self.min[2]
            && p[2] <= self.max[2]
    }

    /// The integer range of voxel cells that this AABB potentially
    /// overlaps. Returns (min_inclusive, max_inclusive) block coords.
    ///
    /// Use this to drive entity-vs-voxel collision: iterate the cells
    /// in this range, for each that's solid, do per-cell intersection
    /// and resolve. Cell at block coord X covers [X, X+1).
    pub fn block_range(&self) -> ([i32; 3], [i32; 3]) {
        let min = [
            self.min[0].floor() as i32,
            self.min[1].floor() as i32,
            self.min[2].floor() as i32,
        ];
        // For max, a position exactly at integer X is INSIDE cell
        // X-1, not X. But floats often land at .9999... so floor()
        // is safer. (max-eps).floor() == max.floor()-1 in the
        // boundary case, which is what we want.
        let max = [
            self.max[0].floor() as i32,
            self.max[1].floor() as i32,
            self.max[2].floor() as i32,
        ];
        (min, max)
    }

    pub fn center(&self) -> [f32; 3] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }

    pub fn extents(&self) -> [f32; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }
}

/// Owner of all entities for one world. The integration story:
///
/// 1. Some authoritative struct (ChunkManager or a new WorldEntities)
///    owns an `Arc<RwLock<EntityStore>>`.
/// 2. The tick driver borrows it during each tick and calls
///    `advance_entities(&mut store, ctx.tick_dt)` from inside a
///    registered TickRegistry handler.
/// 3. Renderer reads the store each frame to emit draws for visible
///    entities.
/// 4. Edit handlers (block-break, projectile-fire) call
///    `store.spawn(...)` to add new entities.
///
/// None of those callers exist yet — this is pure scaffold. The point
/// is that all four integration points have a single stable type to
/// reference.
pub struct EntityStore {
    next_id: AtomicU64,
    entities: Vec<Entity>,
}

impl EntityStore {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(0),
            entities: Vec::new(),
        }
    }

    /// Add an entity. Returns its ID, which is stable for the
    /// lifetime of the store.
    ///
    /// `&mut self` because we mutate the Vec. The AtomicU64 isn't
    /// strictly needed (Vec push requires &mut anyway), but using
    /// atomic ordering keeps the future "shared store, lockless
    /// ID allocation" path clean.
    pub fn spawn(&mut self, kind: EntityKind, position: [f32; 3]) -> EntityId {
        let id = EntityId(self.next_id.fetch_add(1, Ordering::Relaxed));
        self.entities.push(Entity::new(id, kind, position));
        id
    }

    /// Remove an entity by ID. Returns true if it existed.
    ///
    /// O(n) — linear scan to find the index. Future spatial-index work
    /// would also accelerate this. For the entity counts we expect in
    /// the near term (dozens, not thousands) it's fine.
    pub fn despawn(&mut self, id: EntityId) -> bool {
        if let Some(idx) = self.entities.iter().position(|e| e.id == id) {
            self.entities.swap_remove(idx);
            true
        } else {
            false
        }
    }

    pub fn len(&self) -> usize { self.entities.len() }
    pub fn is_empty(&self) -> bool { self.entities.is_empty() }

    pub fn iter(&self) -> impl Iterator<Item = &Entity> {
        self.entities.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Entity> {
        self.entities.iter_mut()
    }

    /// Entities within `radius` (in block units) of `pos`. Linear scan.
    /// Used by AI behaviors (find target), interaction code (pick up
    /// the nearest item), and proximity-based effects.
    pub fn nearby(&self, pos: [f32; 3], radius: f32) -> Vec<&Entity> {
        let r2 = radius * radius;
        self.entities
            .iter()
            .filter(|e| {
                let dx = e.position[0] - pos[0];
                let dy = e.position[1] - pos[1];
                let dz = e.position[2] - pos[2];
                dx * dx + dy * dy + dz * dz <= r2
            })
            .collect()
    }

    /// Look up an entity by ID. O(n) linear scan; same caveat as
    /// `despawn` about spatial-index work.
    pub fn get(&self, id: EntityId) -> Option<&Entity> {
        self.entities.iter().find(|e| e.id == id)
    }

    pub fn get_mut(&mut self, id: EntityId) -> Option<&mut Entity> {
        self.entities.iter_mut().find(|e| e.id == id)
    }

    /// Despawn all entities whose `age_ticks` exceeds the per-kind
    /// max. Returns the number despawned.
    ///
    /// Per-kind limits:
    /// - DroppedItem: 600 ticks (~5 minutes at 2 Hz). Matches the
    ///   classic "items vanish after 5 min" convention.
    /// - Particle: 60 ticks (~30 sec). Particles are short-lived
    ///   by definition.
    /// - Critter: never. Critters live until killed.
    ///
    /// Called from the tick loop after `advance_entities`. Future
    /// extensions: spawn-anchored despawn (item near origin
    /// despawns slower), kind-specific decay shapes.
    pub fn despawn_aged_above(&mut self) -> usize {
        let before = self.entities.len();
        self.entities.retain(|e| match e.kind {
            EntityKind::Critter => true,
            EntityKind::DroppedItem { .. } => e.age_ticks < 600,
            EntityKind::Particle => e.age_ticks < 60,
        });
        before - self.entities.len()
    }
}

impl Default for EntityStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Frozen view of an entity's state at a moment in time, intended for
/// the renderer.
///
/// The point of this type is **lock decoupling**. The future
/// `Arc<RwLock<EntityStore>>` design means the renderer would need to
/// hold a read lock for the entire frame to iterate the store. With
/// a snapshot, the renderer briefly write-locks once per frame to
/// capture, then reads from the snapshot without lock contention.
///
/// Currently nothing populates these snapshots automatically. When
/// renderer wiring lands, the pattern is:
///
/// ```ignore
/// let snapshot = {
///     let store = world_entities.read().unwrap();
///     EntityRenderSnapshot::capture(&store)
/// };
/// for item in snapshot.items() {
///     emit_entity_draw(item);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct EntityRenderSnapshot {
    items: Vec<EntityRenderItem>,
}

/// One renderable entity within a snapshot. The fields are exactly
/// what the future entity renderer needs: where to draw, what to
/// draw, and what AABB to test against the view frustum.
#[derive(Debug, Clone, Copy)]
pub struct EntityRenderItem {
    pub id: EntityId,
    pub kind: EntityKind,
    pub position: [f32; 3],
    pub aabb: Aabb,
}

impl EntityRenderSnapshot {
    pub fn capture(store: &EntityStore) -> Self {
        Self {
            items: store
                .iter()
                .map(|e| EntityRenderItem {
                    id: e.id,
                    kind: e.kind,
                    position: e.position,
                    aabb: e.aabb(),
                })
                .collect(),
        }
    }

    pub fn items(&self) -> &[EntityRenderItem] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Advance all entities by one tick.
///
/// Today: integrate velocity into position, age, apply gravity. NO
/// voxel-grid collision — entities fall forever. NO despawn rules —
/// entities accumulate. Real physics integration is the next layer up
/// once the data flow is validated.
///
/// `dt` is in seconds (typically 0.5 at the default 2 Hz tick rate).
///
/// This function is NOT wired into TickRegistry. Integration looks
/// like:
///
/// ```ignore
/// let store = Arc::new(RwLock::new(EntityStore::new()));
/// let s = Arc::clone(&store);
/// registry.register("entity_update", Arc::new(move |ctx| {
///     let mut store = s.write().unwrap();
///     advance_entities(&mut store, ctx.tick_dt);
/// }));
/// ```
pub fn advance_entities(store: &mut EntityStore, dt: f32) {
    const GRAVITY: f32 = -9.8;
    for e in store.iter_mut() {
        e.age_ticks = e.age_ticks.saturating_add(1);
        // Semi-implicit Euler: update velocity first, then position
        // using the new velocity. Slightly more stable than fully
        // explicit Euler for falling objects under gravity. Standard
        // game-physics pattern.
        e.velocity[1] += GRAVITY * dt;
        e.position[0] += e.velocity[0] * dt;
        e.position[1] += e.velocity[1] * dt;
        e.position[2] += e.velocity[2] * dt;
    }
}

/// Shared, thread-safe wrapper around an `EntityStore`. The concrete
/// "where do entities live" answer the scaffold RC left open.
///
/// `WorldEntities` is `Clone` because it holds `Arc<RwLock<...>>`
/// internally — cloning shares the same underlying store with refcount
/// bump only. Tick handlers, edit hooks, and renderer can all hold
/// their own clones and synchronize via the inner RwLock.
///
/// **Lock granularity:** one global lock, single writer at a time.
/// Acceptable while entity counts and tick frequencies are small.
/// Sharded storage or per-kind locks are deferred until the lock
/// shows up in profiling.
///
/// **Tick integration:** `advance_one_tick` does the standard
/// "advance + cull aged" sequence atomically. Future tick handlers
/// may want to interleave (advance, run AI, cull) — at that point
/// expose the inner methods individually instead of bundling.
#[derive(Clone)]
pub struct WorldEntities {
    inner: std::sync::Arc<std::sync::RwLock<EntityStore>>,
}

impl WorldEntities {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::RwLock::new(EntityStore::new())),
        }
    }

    /// Spawn an entity. Acquires the write lock briefly.
    pub fn spawn_entity(&self, kind: EntityKind, position: [f32; 3]) -> EntityId {
        self.inner.write().unwrap().spawn(kind, position)
    }

    /// Remove an entity. Returns true if it existed.
    pub fn despawn_entity(&self, id: EntityId) -> bool {
        self.inner.write().unwrap().despawn(id)
    }

    /// Capture the current state for rendering. Brief read lock.
    /// Renderer then uses the snapshot independent of the store.
    pub fn snapshot(&self) -> EntityRenderSnapshot {
        let store = self.inner.read().unwrap();
        EntityRenderSnapshot::capture(&store)
    }

    /// Standard tick step: advance physics, then cull aged-out
    /// entities. Designed to be the body of a TickRegistry handler.
    pub fn advance_one_tick(&self, dt: f32) {
        let mut store = self.inner.write().unwrap();
        advance_entities(&mut store, dt);
        store.despawn_aged_above();
    }

    /// Entity count for diagnostic / log output. Brief read lock.
    pub fn len(&self) -> usize {
        self.inner.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.read().unwrap().is_empty()
    }
}

impl Default for WorldEntities {
    fn default() -> Self {
        Self::new()
    }
}

/// Abstraction over "something that can spawn entities."
///
/// Block-break code, projectile-fire code, etc., shouldn't need to
/// know whether they're spawning into a real `WorldEntities` or a
/// test mock or a network-relayed channel. They take
/// `&dyn EntitySpawnSink` and call `.spawn(...)`.
///
/// The trait deliberately mirrors `WorldEntities::spawn_entity` so
/// the production implementation is trivial.
///
/// **Why a trait, not a closure:** captures are awkward across
/// async boundaries and trait objects let us swap implementations
/// (testing, future networked spawn) without changing call sites.
pub trait EntitySpawnSink: Send + Sync {
    fn spawn(&self, kind: EntityKind, position: [f32; 3]) -> EntityId;
}

impl EntitySpawnSink for WorldEntities {
    fn spawn(&self, kind: EntityKind, position: [f32; 3]) -> EntityId {
        self.spawn_entity(kind, position)
    }
}

/// Trait for "is this voxel cell occupied by something solid?"
///
/// Entity physics doesn't care about the underlying storage (Chunk,
/// Octree, mocked test array) — it just wants to ask whether motion
/// is blocked. Implementations adapt the storage type to this query
/// shape.
///
/// **Performance contract:** implementations should be O(1) or close
/// to it. The collision resolver may call `is_solid` for many cells
/// in a tight loop. The standard implementation will be a closure
/// over an Octree sampler — sub-100ns per call. If your implementation
/// is much slower than that, collision will be the bottleneck.
///
/// **Coordinate convention:** `(x, y, z)` are block coordinates in
/// world space (i32). The cell at integer X covers [X, X+1) on the
/// x axis. Out-of-loaded-range cells should return `false` (not
/// blocking) — entities pass through unloaded chunks as if they're
/// air. That's the safe default since we don't have streaming
/// collision queries yet.
pub trait VoxelSolidQuery {
    fn is_solid(&self, x: i32, y: i32, z: i32) -> bool;
}

/// Find a displacement that, when added to `center`, removes any
/// overlap between the entity's AABB and the solid voxels in the
/// world. Returns `None` if there's no overlap to resolve.
///
/// Algorithm: per-direction maximum penetration.
///
/// For each of the 6 push directions (±X, ±Y, ±Z), compute the
/// MAXIMUM penetration across all overlapping solid cells. Pushing
/// the entity that far in that direction resolves ALL overlapping
/// cells simultaneously on that direction. Then pick whichever
/// direction has the smallest such max — that's the shortest correction
/// that genuinely removes all overlap.
///
/// **Why max-per-direction and not min-per-cell:** an entity embedded
/// in multiple floor cells has small per-cell X-penetration but large
/// per-cell Y-penetration. The naive "smallest per-cell" picks an X
/// push that resolves ONE cell but leaves the others overlapping.
/// Max-per-direction guarantees a push that clears every cell on
/// the chosen axis.
///
/// **Single application.** If the entity is deeply embedded such that
/// no single direction works (e.g., fully surrounded by solid), this
/// returns the smallest push but residual overlap may remain. Caller
/// re-checks and re-resolves. Practically: small per-step penetrations
/// (a few cm) at 2 Hz tick rate always resolve in one call.
///
/// **No swept-volume / corner snagging logic.** Fine for slow entities
/// (mobs, dropped items). For high-speed projectiles a swept-AABB
/// algorithm is needed; deferred until projectiles ship.
pub fn resolve_intersection<Q: VoxelSolidQuery>(
    center: [f32; 3],
    half_extents: [f32; 3],
    voxels: &Q,
) -> Option<[f32; 3]> {
    let aabb = Aabb::from_center_half_extents(center, half_extents);
    let (lo, hi) = aabb.block_range();

    // Per-direction maximum penetration depth needed to clear all
    // overlapping solid cells. Indices: 0=+X, 1=-X, 2=+Y, 3=-Y, 4=+Z, 5=-Z.
    let mut max_pen = [0.0f32; 6];
    let mut found = false;

    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                if !voxels.is_solid(x, y, z) {
                    continue;
                }
                // Cell occupies [x, x+1] × [y, y+1] × [z, z+1].
                // Strict overlap check — if a "cell in range" actually
                // touches but doesn't overlap our AABB, skip it.
                let cell_min = [x as f32, y as f32, z as f32];
                let cell_max = [(x + 1) as f32, (y + 1) as f32, (z + 1) as f32];
                if cell_max[0] <= aabb.min[0] || cell_min[0] >= aabb.max[0] {
                    continue;
                }
                if cell_max[1] <= aabb.min[1] || cell_min[1] >= aabb.max[1] {
                    continue;
                }
                if cell_max[2] <= aabb.min[2] || cell_min[2] >= aabb.max[2] {
                    continue;
                }
                found = true;

                let p_xp = cell_max[0] - aabb.min[0];
                let p_xn = aabb.max[0] - cell_min[0];
                let p_yp = cell_max[1] - aabb.min[1];
                let p_yn = aabb.max[1] - cell_min[1];
                let p_zp = cell_max[2] - aabb.min[2];
                let p_zn = aabb.max[2] - cell_min[2];

                if p_xp > max_pen[0] { max_pen[0] = p_xp; }
                if p_xn > max_pen[1] { max_pen[1] = p_xn; }
                if p_yp > max_pen[2] { max_pen[2] = p_yp; }
                if p_yn > max_pen[3] { max_pen[3] = p_yn; }
                if p_zp > max_pen[4] { max_pen[4] = p_zp; }
                if p_zn > max_pen[5] { max_pen[5] = p_zn; }
            }
        }
    }

    if !found {
        return None;
    }

    // Pick the smallest "global" penetration across directions.
    // (axis_index, sign) tuples, mirror of max_pen[].
    let dirs: [(usize, f32); 6] = [
        (0, 1.0),
        (0, -1.0),
        (1, 1.0),
        (1, -1.0),
        (2, 1.0),
        (2, -1.0),
    ];
    let mut best_pen = f32::INFINITY;
    let mut best_axis: usize = 0;
    let mut best_sign: f32 = 0.0;
    for (i, &(axis, sign)) in dirs.iter().enumerate() {
        if max_pen[i] > 0.0 && max_pen[i] < best_pen {
            best_pen = max_pen[i];
            best_axis = axis;
            best_sign = sign;
        }
    }

    if !best_pen.is_finite() {
        return None;
    }

    let mut out = center;
    out[best_axis] += best_sign * best_pen;
    Some(out)
}

/// Convenience: resolve any overlap between this entity's AABB and
/// solid voxels, updating `entity.position` in place. Returns true
/// if a correction was applied.
pub fn entity_resolve_intersection<Q: VoxelSolidQuery>(
    entity: &mut Entity,
    voxels: &Q,
) -> bool {
    let half = entity.aabb().extents();
    let half = [half[0] * 0.5, half[1] * 0.5, half[2] * 0.5];
    if let Some(new_center) = resolve_intersection(entity.position, half, voxels) {
        entity.position = new_center;
        true
    } else {
        false
    }
}

// ============================================================
// Spawn rules (v0.24.61-entity-spawn-rules)
// ============================================================
//
// `docs/DESIGN_NOTES.md` L165-168:
//   "Mob/entity model: 'as material/growth processes, not surface vs
//    underground.' Slime molds spawning where moisture > X and food
//    > Y, insects on chemical/temperature trails. Not biome-locked
//    spawn tables. Plan: entity spawning queries WorldModel fields,
//    not biome enums."
//
// This is the substrate for that plan. SpawnRule expresses "this
// kind of entity wants to appear under these field conditions." The
// runtime evaluator queries the field_reader closure (the same one
// TickContext carries) and emits spawn positions where all conditions
// hold.
//
// Field name strings match the canonical names in
// `crate::world::fields` (`"moisture"`, `"wind"`, etc.). Future
// fields (food, temperature, chemical-trail) follow the same shape.

/// One field-range condition. A spawn rule with multiple conditions
/// is satisfied only when ALL conditions hold (AND semantics).
///
/// Both bounds are inclusive. A condition `min=0.3 max=0.9` means
/// "field value in [0.3, 0.9]." Setting `min=f32::MIN` or
/// `max=f32::MAX` gives a one-sided constraint.
#[derive(Debug, Clone, Copy)]
pub struct SpawnCondition {
    pub field_name: &'static str,
    pub min_value: f32,
    pub max_value: f32,
}

impl SpawnCondition {
    pub fn at_least(field_name: &'static str, min_value: f32) -> Self {
        Self { field_name, min_value, max_value: f32::MAX }
    }
    pub fn at_most(field_name: &'static str, max_value: f32) -> Self {
        Self { field_name, min_value: f32::MIN, max_value }
    }
    pub fn between(field_name: &'static str, min_value: f32, max_value: f32) -> Self {
        Self { field_name, min_value, max_value }
    }

    /// Test whether a sampled field value satisfies this condition.
    /// `None` (field not loaded / not registered) always fails — we
    /// prefer "no spawn" over "spawn with uncertain conditions."
    pub fn check(&self, field_value: Option<f32>) -> bool {
        match field_value {
            Some(v) => v >= self.min_value && v <= self.max_value,
            None => false,
        }
    }
}

/// A spawn rule: "this kind of entity wants to appear at this base
/// density wherever the conditions hold."
///
/// `base_density` is the fraction of candidate positions that pass
/// the density gate before condition checks. So `0.005` means "5
/// out of every 1000 candidates have a shot at spawning before
/// condition filtering further trims." Tune per kind.
///
/// Conditions are stored as `Vec` (not array) so a rule can have any
/// number. Empty conditions = "spawn whenever density gate passes,
/// regardless of fields" — useful for testing.
#[derive(Debug, Clone)]
pub struct SpawnRule {
    pub kind: EntityKind,
    pub conditions: Vec<SpawnCondition>,
    pub base_density: f32,
}

impl SpawnRule {
    pub fn new(kind: EntityKind, base_density: f32) -> Self {
        Self {
            kind,
            conditions: Vec::new(),
            base_density,
        }
    }

    pub fn with_condition(mut self, condition: SpawnCondition) -> Self {
        self.conditions.push(condition);
        self
    }
}

/// Deterministic hash for spawn-density gating. Same `(position,
/// rule_idx, seed)` always produces the same float in [0, 1).
///
/// Caller uses this as "should I spawn here?" gate before the more
/// expensive field-reader probes for condition checks. The hash is
/// salted by rule_idx so different rules don't lock-step at the
/// same positions.
fn spawn_hash(pos: [i32; 3], rule_idx: usize, seed: u64) -> f32 {
    let mut h: u64 = (pos[0] as u64).wrapping_mul(0xa376_2dab_5ce3_43e9)
        ^ (pos[1] as u64).wrapping_mul(0x5851_f42d_4c95_7f2d)
        ^ (pos[2] as u64).wrapping_mul(0xc1b3_e99c_5f78_2d11)
        ^ (rule_idx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ seed.wrapping_mul(0x14057B7E_F767_814F);
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^= h >> 33;
    (h & 0xFFFFFFFF) as f32 / (u32::MAX as f32)
}

/// Evaluate spawn rules against candidate positions.
///
/// Args:
/// - `rules`: rules to apply. Each candidate is tested against each
///   rule independently (a single position could spawn multiple
///   entities if multiple rules pass — usually unwanted; rules should
///   be tuned so conditions partition cleanly).
/// - `field_reader`: same signature as `TickContext.field_reader`.
///   Implementations look up the named field at the world position
///   and return its value, or `None` if not available.
/// - `candidate_positions`: world positions to test. Typically the
///   spawner generates these from a hash-perturbed grid over a chunk
///   or via surface_y lookups on a sparse sample of columns.
/// - `seed`: world seed (or any stable per-world value) so the same
///   world produces the same spawn pattern across runs.
///
/// Returns the `(kind, position)` pairs that the caller should
/// spawn. Caller is responsible for actually invoking `spawn(...)`
/// on a sink and for converting the i32 cell position to f32 entity
/// position (typically `[x + 0.5, y + 0.5, z + 0.5]` for cell-
/// centered).
pub fn evaluate_spawn_rules(
    rules: &[SpawnRule],
    field_reader: &dyn Fn(&str, i32, i32, i32) -> Option<f32>,
    candidate_positions: &[[i32; 3]],
    seed: u64,
) -> Vec<(EntityKind, [i32; 3])> {
    let mut out = Vec::new();
    for &pos in candidate_positions.iter() {
        for (rule_idx, rule) in rules.iter().enumerate() {
            // Density gate first — cheap, fails most candidates,
            // skips the more expensive field samples below.
            let h = spawn_hash(pos, rule_idx, seed);
            if h >= rule.base_density {
                continue;
            }
            // Every condition must pass.
            let mut all_pass = true;
            for cond in rule.conditions.iter() {
                let val = field_reader(cond.field_name, pos[0], pos[1], pos[2]);
                if !cond.check(val) {
                    all_pass = false;
                    break;
                }
            }
            if all_pass {
                out.push((rule.kind, pos));
            }
        }
    }
    out
}

/// Reference / starter set of spawn rules. The exact density and
/// condition values are placeholders pending playtest tuning —
/// they're picked to produce *some* visible spawning in typical
/// biomes, not to model real ecology.
///
/// Future RCs will replace these with playtest-tuned values and
/// likely move them out of code into a data file (JSON or RON) so
/// designers can iterate without rebuilds.
pub fn default_spawn_rules() -> Vec<SpawnRule> {
    vec![
        // Critters favor moderately humid terrain — present in
        // forests and plains, absent from deserts and oceans. The
        // moisture field range mirrors the band where moss and
        // grass already thrive.
        SpawnRule::new(EntityKind::Critter, 0.005)
            .with_condition(SpawnCondition::between("moisture", 0.30, 0.90)),
    ]
}

// ============================================================
// Render bake (v0.24.62-entity-shaders)
// ============================================================
//
// The shape that bridges the entity layer to the GPU pipeline. The
// renderer takes an `EntityRenderSnapshot` and calls
// `snapshot_to_vertices` to produce a flat vertex+index buffer pair,
// then uploads to the GPU and draws.
//
// The matching shaders live at:
//   crates/unendlich-engine/src/shaders/entity.vert
//   crates/unendlich-engine/src/shaders/entity.frag
//
// Vertex format is `EntityVertex` — repr(C) so byte layout matches
// the shader inputs:
//   layout(location = 0) in vec3 in_position;
//   layout(location = 1) in vec3 in_color;
//
// **Per-frame bake, not instanced.** Each entity becomes 8 vertices
// and 36 indices in the batched buffer. At ~50 entities (typical
// expected count) that's 400 vertices + 1800 indices = ~5 KB total —
// well within "rebuild every frame" budget. Switching to instanced
// rendering with per-instance offset is a 5-minute change if profiling
// ever shows this bake as a bottleneck.

/// Vertex format for entity rendering. Matches the layout declared in
/// `shaders/entity.vert`:
///
/// ```glsl
/// layout(location = 0) in vec3 in_position;
/// layout(location = 1) in vec3 in_color;
/// layout(location = 2) in float in_alpha;
/// ```
///
/// `repr(C)` ensures field order and no padding insertion. Total size
/// 28 bytes (7× f32). r535: `alpha` was added for real per-vertex
/// transparency (the player's own avatar fades as it obscures the camera);
/// it defaults to 1.0 (fully opaque) everywhere else.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityVertex {
    pub position: [f32; 3],
    pub color: [f32; 3],
    pub alpha: f32,
    /// r599: wind/disturber sway weight `0..1` (0 = rigid, 1 = free). Drives the
    /// same bending model as foliage — a steady downwind lean plus the mob/dragon
    /// disturber wake from `sim::secondary_motion` — applied per-part in
    /// `hero_forge::dress()` to capes, scarves, and trailing cloth. `entity.vert`
    /// reads this to displace; solid body parts default to 0 (no motion).
    pub sway: f32,
}

impl EntityVertex {
    /// Opaque vertex (alpha = 1.0) — the common case.
    #[inline]
    pub fn opaque(position: [f32; 3], color: [f32; 3]) -> Self {
        Self { position, color, alpha: 1.0, sway: 0.0 }
    }

    /// Like [`opaque`], with an explicit wind-sway weight (see [`sway`](Self::sway)).
    #[inline]
    pub fn opaque_sway(position: [f32; 3], color: [f32; 3], sway: f32) -> Self {
        Self { position, color, alpha: 1.0, sway }
    }
}

/// Default per-kind color when no per-instance data overrides. The
/// renderer is expected to call this for entities whose kind doesn't
/// have its own color source (e.g., `DroppedItem` should sample the
/// CPU mesher's `block_color` instead of using the generic golden
/// fallback here).
///
/// Values picked to read distinctly against grass-green surroundings.
pub fn default_color_for_kind(kind: EntityKind) -> [f32; 3] {
    match kind {
        EntityKind::Critter => [0.55, 0.35, 0.30],         // earthy brown
        EntityKind::DroppedItem { .. } => [0.80, 0.65, 0.30], // generic golden
        EntityKind::Particle => [0.95, 0.95, 0.85],        // off-white
    }
}

/// Convert an entity render snapshot into a flat vertex + index buffer
/// suitable for direct GPU upload. Each entity becomes a cube — 8
/// vertices + 36 indices.
///
/// `color_fn` decides each entity's color. Callers that want
/// block-id-aware coloring for `DroppedItem` should look up the block
/// color from the CPU mesher and return it; callers that want a
/// uniform per-kind default can just pass `default_color_for_kind`.
///
/// Winding follows the existing chunk mesh shader conventions exactly
/// (CCW from outside, per-face indices borrowed from
/// chunk_mesh_octree.comp). Vulkan defaults to CCW front-facing, so no
/// pipeline rasterization-state change is needed to share a render
/// pass with the chunk pipeline.
pub fn snapshot_to_vertices(
    snap: &EntityRenderSnapshot,
    color_fn: impl Fn(EntityKind) -> [f32; 3],
) -> (Vec<EntityVertex>, Vec<u32>) {
    let mut verts = Vec::with_capacity(snap.len() * 8);
    let mut indices = Vec::with_capacity(snap.len() * 36);

    // Cube face triangles, indexing into the 8 corner vertices.
    // Vertex order from the loop below: iter (x, y, z) with x as
    // outermost, z as innermost gives this layout:
    //   index 0 = c000 (x=min, y=min, z=min)
    //   index 1 = c001 (x=min, y=min, z=max)
    //   index 2 = c010
    //   index 3 = c011
    //   index 4 = c100
    //   index 5 = c101
    //   index 6 = c110
    //   index 7 = c111
    //
    // CCW from outside, faces in (+X, -X, +Y, -Y, +Z, -Z) order.
    // These windings match the conventions in chunk_mesh_octree.comp.
    const FACE_INDICES: [u32; 36] = [
        // +X: c100, c110, c111, c101
        4, 6, 7,  4, 7, 5,
        // -X: c000, c001, c011, c010
        0, 1, 3,  0, 3, 2,
        // +Y: c010, c011, c111, c110
        2, 3, 7,  2, 7, 6,
        // -Y: c000, c100, c101, c001
        0, 4, 5,  0, 5, 1,
        // +Z: c001, c101, c111, c011
        1, 5, 7,  1, 7, 3,
        // -Z: c000, c010, c110, c100
        0, 2, 6,  0, 6, 4,
    ];

    for item in snap.items() {
        let base = verts.len() as u32;
        let color = color_fn(item.kind);
        let aabb = item.aabb;

        // 8 corners of the AABB. Iteration order matches the FACE_INDICES
        // layout above — outer x, mid y, inner z.
        let xs = [aabb.min[0], aabb.max[0]];
        let ys = [aabb.min[1], aabb.max[1]];
        let zs = [aabb.min[2], aabb.max[2]];
        for &x in xs.iter() {
            for &y in ys.iter() {
                for &z in zs.iter() {
                    verts.push(EntityVertex {
                        position: [x, y, z],
                        color,
                        alpha: 1.0,
                        sway: 0.0,
                    });
                }
            }
        }

        for &i in FACE_INDICES.iter() {
            indices.push(base + i);
        }
    }

    (verts, indices)
}

/// Body-base colour for a creature's torso box.
const CREATURE_BODY_COLOR: [f32; 3] = [0.50, 0.45, 0.40];

/// Distinct colour per anatomy part so a multi-part creature reads as a body
/// with eyes, jaw, wings, etc. rather than a uniform blob.
pub fn part_color(part: crate::creature::Part) -> [f32; 3] {
    use crate::creature::Part::*;
    match part {
        Eye => [0.08, 0.08, 0.10],
        Ear => [0.34, 0.29, 0.30],
        Jaw => [0.80, 0.75, 0.62],
        Wing => [0.62, 0.66, 0.72],
        Foot | StickyFoot => [0.30, 0.26, 0.22],
        Hand => [0.42, 0.32, 0.26],
        Fin => [0.40, 0.52, 0.62],
        Tail => [0.46, 0.41, 0.36],
    }
}

/// Emit one axis-aligned box (8 corners + 36 indices) into the buffers, using
/// the same corner order + winding as [`snapshot_to_vertices`].
fn push_box(
    verts: &mut Vec<EntityVertex>,
    indices: &mut Vec<u32>,
    center: [f32; 3],
    half: [f32; 3],
    color: [f32; 3],
) {
    let base = verts.len() as u32;
    let xs = [center[0] - half[0], center[0] + half[0]];
    let ys = [center[1] - half[1], center[1] + half[1]];
    let zs = [center[2] - half[2], center[2] + half[2]];
    for &x in &xs {
        for &y in &ys {
            for &z in &zs {
                verts.push(EntityVertex { position: [x, y, z], color, alpha: 1.0, sway: 0.0 });
            }
        }
    }
    const FACE_INDICES: [u32; 36] = [
        4, 6, 7, 4, 7, 5, 0, 1, 3, 0, 3, 2, 2, 3, 7, 2, 7, 6, 0, 4, 5, 0, 5, 1, 1, 5, 7, 1, 7, 3,
        0, 2, 6, 0, 6, 4,
    ];
    for &i in &FACE_INDICES {
        indices.push(base + i);
    }
}

/// Like [`push_box`] (body-local + yaw) but also applies `pitch` (nose up/down) and `roll`
/// (bank) so a creature leans into terrain. Each corner is rotated in the body's
/// local frame — roll about forward (X), then pitch about right (Z), then yaw
/// about up (Y) — before translating to `world_origin`. With pitch=roll=0 it is
/// identical to a yaw-only rotation.
#[allow(clippy::too_many_arguments)]
fn push_box_oriented(
    verts: &mut Vec<EntityVertex>,
    indices: &mut Vec<u32>,
    world_origin: [f32; 3],
    local_center: [f32; 3],
    half: [f32; 3],
    color: [f32; 3],
    yaw: f32,
    pitch: f32,
    roll: f32,
) {
    let base = verts.len() as u32;
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sr, cr) = roll.sin_cos();
    let xs = [local_center[0] - half[0], local_center[0] + half[0]];
    let ys = [local_center[1] - half[1], local_center[1] + half[1]];
    let zs = [local_center[2] - half[2], local_center[2] + half[2]];
    for &x in &xs {
        for &y in &ys {
            for &z in &zs {
                let (y1, z1) = (y * cr - z * sr, y * sr + z * cr); // roll about +X
                let (x2, y2) = (x * cp - y1 * sp, x * sp + y1 * cp); // pitch about +Z
                let z2 = z1;
                let rx = x2 * cy - z2 * sy; // yaw about +Y
                let rz = x2 * sy + z2 * cy;
                verts.push(EntityVertex {
                    position: [world_origin[0] + rx, world_origin[1] + y2, world_origin[2] + rz],
                    color,
                    alpha: 1.0,
                    sway: 0.0,
                });
            }
        }
    }
    const FACE_INDICES: [u32; 36] = [
        4, 6, 7, 4, 7, 5, 0, 1, 3, 0, 3, 2, 2, 3, 7, 2, 7, 6, 0, 4, 5, 0, 5, 1, 1, 5, 7, 1, 7, 3,
        0, 2, 6, 0, 6, 4,
    ];
    for &i in &FACE_INDICES {
        indices.push(base + i);
    }
}

/// Bake a `Creature` into renderable geometry: a torso box at the body AABB plus
/// one box per *intact* anatomy part at its body-local offset (X forward, Y up,
/// Z right — parts are placed relative to `body.position`). Destroyed parts are
/// omitted, so tearing off a wing visibly removes it. Pure; feeds the same entity
/// pipeline as [`snapshot_to_vertices`].
pub fn creature_to_vertices(creature: &crate::creature::Creature) -> (Vec<EntityVertex>, Vec<u32>) {
    creature_to_vertices_lod(creature, 0.0)
}

/// Emit the voxel dragon model (`dragon_forge`) as one small cube per surface
/// cell, scaled to fill the creature's body-box height, positioned at the body
/// and rotated by yaw. Forge model space is Z-forward / Y-up; the engine is
/// X-forward / Y-up / Z-right, so model `z`->forward(X), `x`->right(Z).
pub fn forge_to_vertices(
    creature: &crate::creature::Creature,
    model: &crate::sim::dragon_forge::ForgeModel,
) -> (Vec<EntityVertex>, Vec<u32>) {
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    let p = creature.body.position;
    let half_y = creature.body.half_extents[1].max(0.1);
    let half_z = creature.body.half_extents[2].max(0.1);
    // Match the forge model's OVERALL size (bounding-box diagonal) to the old
    // dragon's visual envelope for this genome, so the new dragons span the same
    // size range as the old ones (no tiny, no gigantic) while keeping the forge's
    // own proportions. The old visual box measured ~[9, 3.6, 8]*s, where the
    // bulk scalar s = half_z/4 (half_z = 4*s).
    let s = (half_z / 4.0).max(0.1);
    let target_diag =
        ((9.0 * s).powi(2) + (3.6 * s).powi(2) + (8.0 * s).powi(2)).sqrt();
    let fd = ((model.max[0] - model.min[0]).powi(2)
        + (model.max[1] - model.min[1]).powi(2)
        + (model.max[2] - model.min[2]).powi(2))
        .sqrt()
        .max(0.1);
    let scale = target_diag / fd;
    let yaw = creature.yaw;
    // animate the rig (legs from gait, wings/jaw/sway from the idle clock); rest
    // pose returns the boxes unchanged, so a standing dragon is identical to before.
    // r117: wing pose from the body's vertical motion — the brain's pose was never
    // threaded here, so airborne dragons held the default Spread (gentle idle beat
    // only). Climbing/hovering => Beating (powered flap); fast descent => Flared
    // (landing brake); grounded/slow => Spread, unchanged from before.
    let vy = creature.body.velocity[1];
    let wing_pose = if vy > 0.5 || (vy.abs() <= 0.5 && creature.body.velocity[0].hypot(creature.body.velocity[2]) > 3.0) {
        crate::sim::dragon_forge::WingPose::Beating
    } else if vy < -2.0 {
        crate::sim::dragon_forge::WingPose::Flared
    } else {
        crate::sim::dragon_forge::WingPose::Spread
    };
    let anim = crate::sim::dragon_forge::ForgeAnim {
        gait_phase: creature.gait_phase,
        anim_time: creature.anim_time,
        head_yaw: creature.gaze_yaw,
        head_pitch: creature.gaze_pitch,
        wing_pose,
    };
    // r599: apply the wing-membrane flex (sim::dragon_wing_flex, stepped live in
    // the bin's creature tick) — empty offsets (no forge/never ticked) reproduce
    // `skin`'s output exactly, so this is a strict superset of the old behaviour.
    let boxes = crate::sim::dragon_forge::skin_with_flex(
        model, &anim, &creature.wing_offset_l, &creature.wing_offset_r,
    );
    let lean_pitch = creature.pitch;
    let lean_roll = creature.roll + creature.torso_bend;
    for b in &boxes {
        // model axes -> engine local: model z -> forward(X), y -> up, x -> right(Z)
        let lx = b.center[2] * scale; // forward
        let ly = (b.center[1] - model.min[1]) * scale - half_y; // up, feet at box bottom
        let lz = b.center[0] * scale; // right
        let half = [b.half[2] * scale, b.half[1] * scale, b.half[0] * scale];
        let color = if b.glow {
            model.eye_color
        } else if b.mouth {
            model.mouth_color
        } else {
            // Weather the body: a box low on the creature reads as belly (downward
            // normal ~ -1, full grime); high boxes are the back (only a little).
            let ny = (ly / half_y).clamp(-1.0, 1.0);
            crate::sim::creature_grime::apply_grime_color(model.body_color, ny, &creature.grime)
        };
        push_box_oriented(&mut verts, &mut indices, p, [lx, ly, lz], half, color, yaw, lean_pitch, lean_roll);
    }
    // r555: WING MEMBRANE — the forge leaves the membrane out of the voxels (it's
    // render-only); now that the entity path carries per-vertex alpha we fill each
    // wing's footprint with a translucent membrane sheet so the wings read as skin
    // rather than sparse bones (docs/TODO.md #6). One triangle fan per side across
    // the combined span of that side's wing bones; alpha from wing_opacity.
    {
        use crate::sim::dragon_forge::{J_WING_L, J_WING_R};
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = lean_pitch.sin_cos();
        let (sr, cr) = lean_roll.sin_cos();
        for joint in [J_WING_L, J_WING_R] {
            // combined bounding box of this side's wing bones (model space)
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            let mut any = false;
            for b in &boxes {
                if b.joint == joint && !b.glow {
                    any = true;
                    for k in 0..3 {
                        lo[k] = lo[k].min(b.center[k] - b.half[k]);
                        hi[k] = hi[k].max(b.center[k] + b.half[k]);
                    }
                }
            }
            if !any {
                continue;
            }
            let center = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
            let half = [(hi[0] - lo[0]) * 0.5, (hi[1] - lo[1]) * 0.5, (hi[2] - lo[2]) * 0.5];
            // span axis 0 = model X (wingspan); chord picks the larger of Y/Z (front-back).
            let mem = crate::wing_membrane::membrane_from_box(center, half, 0, 14, 0.55);
            let base = verts.len() as u32;
            for (vi, v) in mem.positions.iter().enumerate() {
                // model -> engine local (same mapping as the boxes above)
                let lx = v[2] * scale;
                let ly = (v[1] - model.min[1]) * scale - half_y;
                let lz = v[0] * scale;
                // rotate (roll about X, pitch about Z, yaw about Y) + world origin
                let (y1, z1) = (ly * cr - lz * sr, ly * sr + lz * cr);
                let (x2, y2) = (lx * cp - y1 * sp, lx * sp + y1 * cp);
                let (rx, rz) = (x2 * cy - z1 * sy, x2 * sy + z1 * cy);
                verts.push(EntityVertex {
                    position: [p[0] + rx, p[1] + y2, p[2] + rz],
                    color: model.body_color,
                    alpha: mem.alpha.get(vi).copied().unwrap_or(0.55),
                    sway: 0.0,
                });
            }
            for &idx in &mem.indices {
                indices.push(base + idx);
            }
        }
    }
    (verts, indices)
}


/// Like [`creature_to_vertices`] but drops anatomy parts whose largest half-extent
/// is below `min_part_half` — a cheap per-creature level of detail. The torso and
/// any feet are always kept (silhouette and gait stay legible); only fine detail
/// (eyes, snout, horns, ridges) is culled. Pass a distance-scaled `min_part_half`
/// so far-off or very large creatures — a dragon carries the most parts — shed
/// detail they're too far to show, trimming entity vertices. `0.0` = full detail.
pub fn creature_to_vertices_lod(creature: &crate::creature::Creature, min_part_half: f32) -> (Vec<EntityVertex>, Vec<u32>) {
    // A genome dragon renders as its voxel forge model, not the box anatomy.
    if let Some(forge) = creature.forge.as_ref() {
        return forge_to_vertices(creature, forge);
    }
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    let p = creature.body.position;
    let (yaw, pitch, roll) = (creature.yaw, creature.pitch, creature.roll);
    // Torso rotates with the body (its corners orbit the centre), so it faces the
    // heading and leans with the terrain instead of sitting axis-aligned.
    let tint = creature.tint;
    let tinted = |c: [f32; 3]| [c[0] * tint[0], c[1] * tint[1], c[2] * tint[2]];

    // r484: drive the full pose rig (pose.rs) instead of the old foot-only inline
    // swing — legs swing with a proper cadence, wings flap while airborne, the
    // body bobs with the gait, and a damaged leg limps automatically. Current
    // flight state comes from the body's velocity (a grounded flyer doesn't flap);
    // leg-health / fly-power come from the graded anatomy so injuries read.
    // gait_phase = 0 + grounded + still leaves every part at rest, so a standing
    // creature is byte-identical to before.
    let (body_bob, animated) = if let Some(anat) = creature.anatomy.as_ref() {
        let vel = creature.body.velocity;
        let hspeed = (vel[0] * vel[0] + vel[2] * vel[2]).sqrt();
        let flying = vel[1] > 0.5 || (vel[1].abs() <= 0.5 && hspeed > 3.0);
        let (leg_health, fly_power) = creature
            .graded_capabilities()
            .map(|g| (g.move_speed, g.fly_power))
            .unwrap_or((1.0, 1.0));
        let loco = crate::pose::Locomotion {
            speed: hspeed,
            leg_health,
            flying,
            fly_power: if flying { fly_power.max(0.3) } else { 0.0 },
        };
        let n_legs = anat
            .parts
            .iter()
            .filter(|pp| matches!(pp.part, crate::creature::Part::Foot | crate::creature::Part::StickyFoot))
            .count();
        let pose = crate::pose::pose_for(creature.gait_phase, n_legs, &loco);
        let animated = crate::pose::animate(anat, creature.gait_phase, &loco);
        (pose.body_bob, Some((anat, animated)))
    } else {
        (0.0, None)
    };

    // Torso rotates with the body (its corners orbit the centre), so it faces the
    // heading and leans with the terrain instead of sitting axis-aligned; it also
    // rides the gait bob so the whole creature rises and falls together.
    push_box_oriented(&mut verts, &mut indices, p, [0.0, body_bob, 0.0], creature.body.half_extents, tinted(CREATURE_BODY_COLOR), yaw, pitch, roll + creature.torso_bend);

    if let Some((anat, animated)) = animated {
        for (part, ap) in anat.parts.iter().zip(animated.iter()) {
            if part.is_destroyed() {
                continue;
            }
            let keep_always = matches!(part.part, crate::creature::Part::Foot | crate::creature::Part::StickyFoot);
            let biggest = part.half_extents[0].max(part.half_extents[1]).max(part.half_extents[2]);
            if !keep_always && biggest < min_part_half {
                continue; // LOD: too small to matter at this distance
            }
            // ap.offset already carries the leg swing / wing flap / body bob.
            push_box_oriented(&mut verts, &mut indices, p, ap.offset, part.half_extents, tinted(part_color(part.part)), yaw, pitch, roll);
        }
    }
    (verts, indices)
}

/// Render half-extents + colour for a sim mob kind: predators larger and red,
/// prey small and tan, flyers bluish, the rest a neutral green-grey.
pub fn mob_appearance(kind: crate::sim::mob::MobKind) -> ([f32; 3], [f32; 3]) {
    use crate::sim::mob::MobKind::*;
    match kind {
        Seeker => ([0.50, 0.60, 0.50], [0.62, 0.20, 0.18]),
        Grazer => ([0.32, 0.40, 0.32], [0.62, 0.55, 0.40]),
        Flyer => ([0.45, 0.30, 0.45], [0.48, 0.55, 0.70]),
        Walker => ([0.40, 0.45, 0.40], [0.45, 0.50, 0.42]),
    }
}

/// Bake the deterministic sim's mobs ([`crate::sim::mob::Mob`]) into renderable
/// boxes — one per mob, sized and coloured by kind — so the ambient spawner can
/// feed the same entity pass as creatures and dropped items. Pure; positions
/// come from the fixed-point body converted to f32.
pub fn mobs_to_vertices(mobs: &[crate::sim::mob::Mob]) -> (Vec<EntityVertex>, Vec<u32>) {
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    for m in mobs {
        let (half, color) = mob_appearance(m.kind);
        let p = m.body.pos;
        push_box(&mut verts, &mut indices, [p.x.to_f32(), p.y.to_f32(), p.z.to_f32()], half, color);
    }
    (verts, indices)
}

/// Bake a procedural [`CreatureBlueprint`](crate::creature_design::CreatureBlueprint)
/// into renderable geometry — one box per blueprint box at its centre offset.
/// Pure; feeds the same entity pipeline as the other bakers. Used by the
/// candidate contact-sheet renderer for design iteration.
pub fn blueprint_to_vertices(
    bp: &crate::creature_design::CreatureBlueprint,
) -> (Vec<EntityVertex>, Vec<u32>) {
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    for b in &bp.boxes {
        push_box(&mut verts, &mut indices, b.offset, b.half, b.color);
    }
    (verts, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creature_yaw_rotates_part_offsets_around_y() {
        use crate::creature::{Anatomy, Creature, PartState};
        use crate::creature::Part;
        use crate::physics::PhysicsBody;
        let centroid_of_last_box = |v: &[EntityVertex]| {
            let s = v.len() - 8;
            let mut acc = [0.0f32; 3];
            for q in &v[s..] {
                for i in 0..3 {
                    acc[i] += q.position[i];
                }
            }
            [acc[0] / 8.0, acc[1] / 8.0, acc[2] / 8.0]
        };
        let mut c = Creature::new(PhysicsBody::new_player([0.0, 0.0, 0.0]));
        c.anatomy = Some(Anatomy::new().with(PartState::new(Part::Jaw, 10, [1.0, 0.0, 0.0], [0.1, 0.1, 0.1])));
        // yaw 0: the front part sits at +X.
        let (v0, _) = creature_to_vertices(&c);
        let c0 = centroid_of_last_box(&v0);
        assert!(c0[0] > 0.5 && c0[2].abs() < 0.1, "yaw 0 -> front at +X (got {c0:?})");
        // yaw +PI/2: +X rotates to +Z.
        c.yaw = std::f32::consts::FRAC_PI_2;
        let (v1, _) = creature_to_vertices(&c);
        let c1 = centroid_of_last_box(&v1);
        assert!(c1[2] > 0.5 && c1[0].abs() < 0.1, "yaw PI/2 -> front at +Z (got {c1:?})");
    }

    #[test]
    fn gait_swings_diagonal_feet_in_opposition() {
        use crate::creature::{Anatomy, Creature, Part, PartState};
        use crate::physics::PhysicsBody;
        let centroid = |v: &[EntityVertex], box_idx: usize| {
            let s = box_idx * 8;
            let mut acc = [0.0f32; 3];
            for q in &v[s..s + 8] {
                for i in 0..3 {
                    acc[i] += q.position[i];
                }
            }
            [acc[0] / 8.0, acc[1] / 8.0, acc[2] / 8.0]
        };
        let mut c = Creature::new(PhysicsBody::new_player([0.0, 0.0, 0.0]));
        // Two feet on opposite diagonals: front-left (+x,+z) and front-right (+x,-z).
        c.anatomy = Some(
            Anatomy::new()
                .with(PartState::new(Part::Foot, 10, [0.4, -0.5, 0.4], [0.1, 0.3, 0.1]))
                .with(PartState::new(Part::Foot, 10, [0.4, -0.5, -0.4], [0.1, 0.3, 0.1])),
        );
        // At rest (phase 0) the feet sit at their authored x.
        let (v0, _) = creature_to_vertices(&c);
        assert!((centroid(&v0, 1)[0] - 0.4).abs() < 1e-4, "foot at rest x");
        // Mid-stride, the two diagonal feet step in opposite directions.
        c.gait_phase = std::f32::consts::FRAC_PI_2;
        let (v1, _) = creature_to_vertices(&c);
        let fl = centroid(&v1, 1); // (+x,+z): diagonal -> phase 0 -> sin=+1 -> forward
        let fr = centroid(&v1, 2); // (+x,-z): off-diagonal -> phase PI -> sin=-1 -> back
        assert!(fl[0] > 0.4 && fr[0] < 0.4, "diagonal feet swing in opposition (fl {fl:?} fr {fr:?})");
    }

    #[test]
    fn the_torso_itself_rotates_with_yaw() {
        use crate::creature::Creature;
        use crate::physics::PhysicsBody;
        let mut body = PhysicsBody::new_player([0.0, 0.0, 0.0]);
        body.half_extents = [0.5, 0.5, 2.0]; // long along Z
        let c = Creature::new(body);
        let x_range = |v: &[EntityVertex]| {
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for q in &v[0..8] {
                lo = lo.min(q.position[0]);
                hi = hi.max(q.position[0]);
            }
            hi - lo
        };
        // At rest the torso is narrow in X (0.5 half -> 1.0 span).
        let (v0, _) = creature_to_vertices(&Creature { yaw: 0.0, ..c.clone() });
        assert!((x_range(&v0) - 1.0).abs() < 1e-4, "rest torso narrow in X");
        // Turned 90°, the long Z axis now lies along X (2.0 half -> 4.0 span) — the
        // body genuinely rotated, not just the limbs.
        let (v1, _) = creature_to_vertices(&Creature { yaw: std::f32::consts::FRAC_PI_2, ..c });
        assert!((x_range(&v1) - 4.0).abs() < 1e-3, "turned torso wide in X (got {})", x_range(&v1));
    }

    #[test]
    fn a_blueprint_bakes_one_box_each() {
        let cands = crate::creature_design::generate_candidates(42, 5);
        for bp in &cands {
            let (v, i) = blueprint_to_vertices(bp);
            assert_eq!(v.len(), bp.boxes.len() * 8);
            assert_eq!(i.len(), bp.boxes.len() * 36);
            assert!(i.iter().all(|&ix| (ix as usize) < v.len()));
        }
    }

    #[test]
    fn mobs_bake_a_box_each_and_predator_prey_differ() {
        use crate::sim::fixed::{Fixed, Vec3};
        use crate::sim::mob::{Mob, MobKind};
        let mut seeker = Mob::new_walker(
            Vec3::new(Fixed::from_int(3), Fixed::from_int(6), Fixed::from_int(5)),
            20,
        );
        seeker.kind = MobKind::Seeker;
        let grazer = Mob::new_walker(Vec3 { x: Fixed::ZERO, y: Fixed::from_int(6), z: Fixed::ZERO }, 12);
        let mobs = [seeker, grazer];
        let (v, i) = mobs_to_vertices(&mobs);
        assert_eq!(v.len(), 2 * 8, "one 8-corner box per mob");
        assert_eq!(i.len(), 2 * 36);
        assert!(i.iter().all(|&ix| (ix as usize) < v.len()));

        // Predator and prey are visually distinct: different colour, predator bulkier.
        let (sh, sc) = mob_appearance(MobKind::Seeker);
        let (gh, gc) = mob_appearance(MobKind::Grazer);
        assert_ne!(sc, gc, "predator and prey coloured differently");
        assert!(sh[0] > gh[0], "predator is bulkier");

        // The seeker's box is centred near its fixed-point position (~3,6,5).
        let cx = v[0..8].iter().map(|vt| vt.position[0]).sum::<f32>() / 8.0;
        assert!((cx - 3.0).abs() < 1.0, "seeker box near x=3, got {cx}");
    }

    #[test]
    fn creature_geometry_is_a_box_per_body_and_intact_part() {
        // This validates the anatomy-box mesher (still used by every non-forge
        // creature); strip the forge model so we exercise that path.
        let mut dragon = crate::spawn::spawn(crate::item::CreatureKind::Dragon, [0.0, 0.0, 0.0]);
        dragon.forge = None;
        let parts = dragon.anatomy.as_ref().unwrap().parts.len(); // 8
        let (verts, indices) = creature_to_vertices(&dragon);
        let boxes = 1 + parts; // body + parts
        assert_eq!(verts.len(), boxes * 8, "8 corners per box");
        assert_eq!(indices.len(), boxes * 36, "36 indices per box");
        // Indices stay in bounds of the vertex buffer.
        assert!(indices.iter().all(|&i| (i as usize) < verts.len()));
    }

    #[test]
    fn a_dragon_is_visibly_bigger_and_wider_than_a_lauscher() {
        let extent = |c: &crate::creature::Creature| {
            let (v, _) = creature_to_vertices(c);
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            for vert in &v {
                for k in 0..3 {
                    lo[k] = lo[k].min(vert.position[k]);
                    hi[k] = hi[k].max(vert.position[k]);
                }
            }
            [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]]
        };
        let mut dd = crate::spawn::spawn(crate::item::CreatureKind::Dragon, [0.0; 3]);
        dd.forge = None; // compare the anatomy silhouettes
        let d = extent(&dd);
        let l = extent(&crate::spawn::spawn(crate::item::CreatureKind::Lauscher, [0.0; 3]));
        // Wingspan (Z) and overall size dwarf the little listener.
        assert!(d[2] > l[2] * 3.0, "dragon wingspan {} vs lauscher {}", d[2], l[2]);
        assert!(d[0] > l[0] && d[1] > l[1]);
    }

    #[test]
    fn tearing_off_a_part_removes_its_box() {
        let mut dragon = crate::spawn::spawn(crate::item::CreatureKind::Dragon, [0.0; 3]);
        dragon.forge = None; // anatomy-box path: parts are individually rendered
        let (_, before) = creature_to_vertices(&dragon);
        // Destroy one wing.
        if let Some(anat) = dragon.anatomy.as_mut() {
            if let Some(w) = anat.parts.iter_mut().find(|p| p.part == crate::creature::Part::Wing) {
                w.hp = 0;
            }
        }
        let (_, after) = creature_to_vertices(&dragon);
        assert_eq!(before.len() - after.len(), 36, "one fewer box (36 indices) after losing a wing");
    }

    #[test]
    fn spawn_assigns_unique_ids() {
        let mut store = EntityStore::new();
        let a = store.spawn(EntityKind::Critter, [0.0, 0.0, 0.0]);
        let b = store.spawn(EntityKind::Critter, [1.0, 0.0, 0.0]);
        assert_ne!(a, b);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn despawn_removes_entity() {
        let mut store = EntityStore::new();
        let id = store.spawn(EntityKind::Critter, [0.0, 0.0, 0.0]);
        assert_eq!(store.len(), 1);
        assert!(store.despawn(id));
        assert_eq!(store.len(), 0);
        // Idempotent: re-despawning a vanished entity returns false.
        assert!(!store.despawn(id));
    }

    #[test]
    fn nearby_filters_by_distance() {
        let mut store = EntityStore::new();
        store.spawn(EntityKind::Critter, [0.0, 0.0, 0.0]);
        store.spawn(EntityKind::Critter, [10.0, 0.0, 0.0]);
        store.spawn(EntityKind::Critter, [100.0, 0.0, 0.0]);
        let near = store.nearby([0.0, 0.0, 0.0], 15.0);
        assert_eq!(near.len(), 2);  // the [0,0,0] and the [10,0,0]
    }

    #[test]
    fn advance_applies_gravity_and_ages() {
        let mut store = EntityStore::new();
        let id = store.spawn(EntityKind::Critter, [0.0, 10.0, 0.0]);
        advance_entities(&mut store, 0.5);
        let e = store.get(id).unwrap();
        assert_eq!(e.age_ticks, 1);
        // velocity[1] should now be negative — gravity applied to a
        // starting velocity of 0.
        assert!(e.velocity[1] < 0.0);
        // position[1] should have dropped: semi-implicit Euler does
        // v_new = 0 + (-9.8)*0.5 = -4.9, then p_new = 10 + (-4.9)*0.5 = 7.55.
        assert!(e.position[1] < 10.0);
        assert!(e.position[1] > 5.0);  // sanity bounds
    }

    #[test]
    fn get_and_get_mut_round_trip() {
        let mut store = EntityStore::new();
        let id = store.spawn(EntityKind::Critter, [0.0, 0.0, 0.0]);
        assert!(store.get(id).is_some());
        store.get_mut(id).unwrap().position = [5.0, 5.0, 5.0];
        assert_eq!(store.get(id).unwrap().position, [5.0, 5.0, 5.0]);
    }

    #[test]
    fn default_is_empty() {
        let store = EntityStore::default();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    // v0.24.59-entity-expansion tests below.

    #[test]
    fn aabb_intersects_basic_cases() {
        let a = Aabb::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let b = Aabb::new([0.5, 0.5, 0.5], [1.5, 1.5, 1.5]);
        let c = Aabb::new([2.0, 2.0, 2.0], [3.0, 3.0, 3.0]);
        assert!(a.intersects(&b));
        assert!(b.intersects(&a));
        assert!(!a.intersects(&c));
        // Touching edge: inclusive convention says yes.
        let edge = Aabb::new([1.0, 0.0, 0.0], [2.0, 1.0, 1.0]);
        assert!(a.intersects(&edge));
    }

    #[test]
    fn aabb_contains_point_endpoints_inclusive() {
        let a = Aabb::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        assert!(a.contains_point([0.5, 0.5, 0.5]));
        assert!(a.contains_point([0.0, 0.0, 0.0]));
        assert!(a.contains_point([1.0, 1.0, 1.0]));
        assert!(!a.contains_point([1.1, 0.5, 0.5]));
    }

    #[test]
    fn aabb_block_range_typical() {
        let a = Aabb::new([0.2, 0.2, 0.2], [2.8, 1.5, 3.0]);
        let (lo, hi) = a.block_range();
        assert_eq!(lo, [0, 0, 0]);
        // max=2.8 → floor=2; max=1.5 → floor=1; max=3.0 → floor=3.
        // (Caller decides whether to include the boundary cell.)
        assert_eq!(hi, [2, 1, 3]);
    }

    #[test]
    fn entity_aabb_uses_kind_dimensions() {
        let critter = Entity::new(EntityId(0), EntityKind::Critter, [0.0, 0.0, 0.0]);
        let item = Entity::new(
            EntityId(1),
            EntityKind::DroppedItem { item: crate::world::blocks::BlockId::Stone },
            [0.0, 0.0, 0.0],
        );
        // Critter is wider in X/Z and taller in Y than dropped item.
        let cb = critter.aabb();
        let ib = item.aabb();
        assert!(cb.extents()[0] > ib.extents()[0]);
        assert!(cb.extents()[1] > ib.extents()[1]);
    }

    #[test]
    fn despawn_aged_above_respects_kind_limits() {
        let mut store = EntityStore::new();
        let critter = store.spawn(EntityKind::Critter, [0.0, 0.0, 0.0]);
        let item = store.spawn(
            EntityKind::DroppedItem { item: crate::world::blocks::BlockId::Stone },
            [0.0, 0.0, 0.0],
        );
        let particle = store.spawn(EntityKind::Particle, [0.0, 0.0, 0.0]);
        // Age everyone past every limit.
        for e in store.iter_mut() {
            e.age_ticks = 1000;
        }
        let removed = store.despawn_aged_above();
        // Item (>600) and particle (>60) should die; critter persists.
        assert_eq!(removed, 2);
        assert!(store.get(critter).is_some());
        assert!(store.get(item).is_none());
        assert!(store.get(particle).is_none());
    }

    #[test]
    fn despawn_aged_above_preserves_young_entities() {
        let mut store = EntityStore::new();
        let item = store.spawn(
            EntityKind::DroppedItem { item: crate::world::blocks::BlockId::Stone },
            [0.0, 0.0, 0.0],
        );
        // Item is young: should NOT be despawned.
        store.get_mut(item).unwrap().age_ticks = 100;
        let removed = store.despawn_aged_above();
        assert_eq!(removed, 0);
        assert!(store.get(item).is_some());
    }

    #[test]
    fn render_snapshot_captures_all_entities() {
        let mut store = EntityStore::new();
        store.spawn(EntityKind::Critter, [1.0, 2.0, 3.0]);
        store.spawn(
            EntityKind::DroppedItem { item: crate::world::blocks::BlockId::Stone },
            [5.0, 6.0, 7.0],
        );
        let snap = EntityRenderSnapshot::capture(&store);
        assert_eq!(snap.len(), 2);
        // Snapshot is decoupled — mutating store doesn't affect it.
        store.spawn(EntityKind::Particle, [0.0, 0.0, 0.0]);
        assert_eq!(snap.len(), 2);
        // Each snapshot item carries the entity's own AABB.
        let item0 = &snap.items()[0];
        assert!((item0.aabb.center()[0] - item0.position[0]).abs() < 1e-5);
    }

    #[test]
    fn dropped_item_kind_carries_block_id() {
        // The variant data is what makes items distinguishable.
        let k1 = EntityKind::DroppedItem { item: crate::world::blocks::BlockId::Stone };
        let k2 = EntityKind::DroppedItem { item: crate::world::blocks::BlockId::Dirt };
        assert_ne!(k1, k2);
    }

    #[test]
    fn world_entities_shares_store_across_clones() {
        let a = WorldEntities::new();
        let b = a.clone();
        let id = a.spawn_entity(EntityKind::Critter, [1.0, 2.0, 3.0]);
        // The clone sees the same underlying store.
        assert_eq!(b.len(), 1);
        assert!(b.despawn_entity(id));
        assert_eq!(a.len(), 0);
    }

    #[test]
    fn world_entities_advance_culls_aged_items() {
        let entities = WorldEntities::new();
        // Spawn an item, age it past despawn threshold, then advance.
        let id = entities.spawn_entity(
            EntityKind::DroppedItem { item: crate::world::blocks::BlockId::Stone },
            [0.0, 0.0, 0.0],
        );
        {
            let mut store = entities.inner.write().unwrap();
            store.get_mut(id).unwrap().age_ticks = 700;
        }
        // advance bumps age by 1 then culls.
        entities.advance_one_tick(0.5);
        assert_eq!(entities.len(), 0);
    }

    #[test]
    fn world_entities_snapshot_isolated_from_store() {
        let entities = WorldEntities::new();
        entities.spawn_entity(EntityKind::Critter, [0.0, 0.0, 0.0]);
        let snap = entities.snapshot();
        assert_eq!(snap.len(), 1);
        // Adding more entities after snapshot doesn't change the snapshot.
        entities.spawn_entity(EntityKind::Critter, [10.0, 0.0, 0.0]);
        assert_eq!(snap.len(), 1);
        assert_eq!(entities.len(), 2);
    }

    #[test]
    fn entity_spawn_sink_trait_object_works() {
        // The whole point: callers don't depend on the concrete type.
        let entities = WorldEntities::new();
        let sink: &dyn EntitySpawnSink = &entities;
        let id = sink.spawn(EntityKind::Critter, [1.0, 1.0, 1.0]);
        assert!(entities.snapshot().items().iter().any(|i| i.id == id));
    }

    // ---- Collision tests (v0.24.60-entity-collision) ----

    /// Test helper: HashSet-backed VoxelSolidQuery. Cells in the set
    /// are solid; everything else is air.
    struct MockVoxels {
        solid: std::collections::HashSet<(i32, i32, i32)>,
    }

    impl MockVoxels {
        fn new() -> Self {
            Self { solid: std::collections::HashSet::new() }
        }
        fn set_solid(&mut self, x: i32, y: i32, z: i32) {
            self.solid.insert((x, y, z));
        }
    }

    impl VoxelSolidQuery for MockVoxels {
        fn is_solid(&self, x: i32, y: i32, z: i32) -> bool {
            self.solid.contains(&(x, y, z))
        }
    }

    #[test]
    fn resolve_no_overlap_returns_none() {
        let voxels = MockVoxels::new();  // all air
        assert!(resolve_intersection([0.5, 0.5, 0.5], [0.4, 0.4, 0.4], &voxels).is_none());
    }

    #[test]
    fn resolve_above_floor_no_overlap() {
        // Solid floor at y=0, entity floats above.
        let mut voxels = MockVoxels::new();
        voxels.set_solid(0, 0, 0);
        voxels.set_solid(1, 0, 0);
        voxels.set_solid(0, 0, 1);
        voxels.set_solid(1, 0, 1);
        // Entity at y=2.5, half-extent 0.4 — well above floor.
        assert!(resolve_intersection([0.5, 2.5, 0.5], [0.4, 0.4, 0.4], &voxels).is_none());
    }

    #[test]
    fn resolve_inside_block_pushes_to_smallest_axis() {
        let mut voxels = MockVoxels::new();
        voxels.set_solid(0, 0, 0);  // Solid at origin cell [0,1]^3
        // Entity slightly above the cell floor — small Y penetration.
        // Centered at (0.5, 0.2, 0.5) with half-extents (0.4, 0.1, 0.4):
        //   AABB Y = [0.1, 0.3]. Cell Y = [0, 1]. Overlap on Y.
        //   AABB X = [0.1, 0.9]. Cell X = [0, 1]. Overlap on X.
        //   Penetration on +Y (push up): 1.0 - 0.1 = 0.9
        //   Penetration on -Y (push down): 0.3 - 0.0 = 0.3
        //   Penetration on +X: 1.0 - 0.1 = 0.9
        //   Penetration on -X: 0.9 - 0.0 = 0.9
        //   Smallest is -Y (push down by 0.3).
        let resolved = resolve_intersection([0.5, 0.2, 0.5], [0.4, 0.1, 0.4], &voxels);
        assert!(resolved.is_some());
        let r = resolved.unwrap();
        // Y should have decreased (push down).
        assert!(r[1] < 0.2);
    }

    #[test]
    fn resolve_on_floor_pushes_up() {
        // Floor at y=0,1,2 (a single layer). Entity center slightly
        // sunk into the floor.
        let mut voxels = MockVoxels::new();
        for x in -1..=1 {
            for z in -1..=1 {
                voxels.set_solid(x, 0, z);
            }
        }
        // Entity center at y=0.9 with half-extents (0.3, 0.5, 0.3):
        //   AABB Y = [0.4, 1.4].
        //   Cell at y=0 has Y range [0, 1] → overlap [0.4, 1.0].
        //   Penetration +Y (push up): 1.0 - 0.4 = 0.6
        //   Penetration -Y (push down): 1.4 - 0.0 = 1.4
        //   So smallest is +Y by 0.6.
        let resolved = resolve_intersection([0.0, 0.9, 0.0], [0.3, 0.5, 0.3], &voxels);
        assert!(resolved.is_some());
        let r = resolved.unwrap();
        // Y should increase to ~1.5 (entity now standing on floor).
        assert!(r[1] > 0.9);
        // The other axes shouldn't have moved.
        assert!((r[0] - 0.0).abs() < 1e-5);
        assert!((r[2] - 0.0).abs() < 1e-5);
    }

    #[test]
    fn entity_resolve_intersection_updates_position() {
        let mut voxels = MockVoxels::new();
        voxels.set_solid(0, 0, 0);
        let mut e = Entity::new(EntityId(0), EntityKind::Critter, [0.5, 0.5, 0.5]);
        // Critter half-extents (0.4, 0.5, 0.4) — definitely overlaps
        // a solid block at cell (0,0,0).
        let changed = entity_resolve_intersection(&mut e, &voxels);
        assert!(changed);
        // Position should have moved.
        assert!(e.position != [0.5, 0.5, 0.5]);
    }

    // ---- Spawn-rule tests (v0.24.61-entity-spawn-rules) ----

    #[test]
    fn spawn_condition_check_handles_missing_field() {
        let cond = SpawnCondition::between("moisture", 0.3, 0.9);
        assert!(!cond.check(None));        // no data = fail safely
        assert!(!cond.check(Some(0.1)));   // below range
        assert!(cond.check(Some(0.5)));    // inside range
        assert!(!cond.check(Some(0.95)));  // above range
        // Boundary is inclusive.
        assert!(cond.check(Some(0.3)));
        assert!(cond.check(Some(0.9)));
    }

    #[test]
    fn spawn_condition_one_sided_constructors() {
        let at_least = SpawnCondition::at_least("moisture", 0.5);
        assert!(at_least.check(Some(0.5)));
        assert!(at_least.check(Some(0.9)));
        assert!(at_least.check(Some(1e6)));
        assert!(!at_least.check(Some(0.4)));

        let at_most = SpawnCondition::at_most("moisture", 0.5);
        assert!(at_most.check(Some(0.5)));
        assert!(at_most.check(Some(-1e6)));
        assert!(!at_most.check(Some(0.6)));
    }

    #[test]
    fn evaluate_spawn_rules_filters_by_density_gate() {
        // Rule with density 0 → nothing spawns even when conditions
        // are trivially true.
        let rules = vec![SpawnRule::new(EntityKind::Critter, 0.0)];
        let field: Box<dyn Fn(&str, i32, i32, i32) -> Option<f32>> =
            Box::new(|_, _, _, _| Some(1.0));
        let positions: Vec<[i32; 3]> = (0..100).map(|i| [i, 64, 0]).collect();
        let out = evaluate_spawn_rules(&rules, &field, &positions, 0xDEADBEEF);
        assert!(out.is_empty());
    }

    #[test]
    fn evaluate_spawn_rules_filters_by_condition() {
        // Rule with density 1.0 (everything passes gate) but
        // moisture condition that nothing satisfies.
        let rules = vec![SpawnRule::new(EntityKind::Critter, 1.0)
            .with_condition(SpawnCondition::between("moisture", 0.5, 0.5))];
        // Field reader returns 0.0 — outside the [0.5, 0.5] window.
        let field: Box<dyn Fn(&str, i32, i32, i32) -> Option<f32>> =
            Box::new(|_, _, _, _| Some(0.0));
        let positions = vec![[0, 64, 0], [1, 64, 0], [2, 64, 0]];
        let out = evaluate_spawn_rules(&rules, &field, &positions, 0xDEADBEEF);
        assert!(out.is_empty());
    }

    #[test]
    fn evaluate_spawn_rules_passes_when_all_conditions_met() {
        let rules = vec![SpawnRule::new(EntityKind::Critter, 1.0)  // density 100%
            .with_condition(SpawnCondition::at_least("moisture", 0.5))];
        let field: Box<dyn Fn(&str, i32, i32, i32) -> Option<f32>> =
            Box::new(|_, _, _, _| Some(0.8));  // field always passes
        let positions = vec![[0, 64, 0], [1, 64, 0], [2, 64, 0]];
        let out = evaluate_spawn_rules(&rules, &field, &positions, 0xDEADBEEF);
        // Density gate at 1.0 always passes the threshold (hash returns
        // values in [0, 1) and 1.0 is strictly greater than any of them).
        assert_eq!(out.len(), 3);
        // Each entry is the Critter at one of the candidate positions.
        for (kind, _pos) in out.iter() {
            assert_eq!(*kind, EntityKind::Critter);
        }
    }

    #[test]
    fn evaluate_spawn_rules_deterministic_for_same_seed() {
        let rules = vec![SpawnRule::new(EntityKind::Critter, 0.1)];
        let field: Box<dyn Fn(&str, i32, i32, i32) -> Option<f32>> =
            Box::new(|_, _, _, _| Some(1.0));
        let positions: Vec<[i32; 3]> = (0..500).map(|i| [i, 64, 0]).collect();
        let a = evaluate_spawn_rules(&rules, &field, &positions, 12345);
        let b = evaluate_spawn_rules(&rules, &field, &positions, 12345);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.0, y.0);
            assert_eq!(x.1, y.1);
        }
    }

    #[test]
    fn evaluate_spawn_rules_different_for_different_seeds() {
        // With density 0.1 and 500 candidates, expect ~50 spawns.
        // Different seeds should select different positions; the COUNT
        // may match by coincidence but the position SET should differ.
        let rules = vec![SpawnRule::new(EntityKind::Critter, 0.1)];
        let field: Box<dyn Fn(&str, i32, i32, i32) -> Option<f32>> =
            Box::new(|_, _, _, _| Some(1.0));
        let positions: Vec<[i32; 3]> = (0..500).map(|i| [i, 64, 0]).collect();
        let a = evaluate_spawn_rules(&rules, &field, &positions, 1);
        let b = evaluate_spawn_rules(&rules, &field, &positions, 2);
        // Both should produce SOME spawns.
        assert!(!a.is_empty());
        assert!(!b.is_empty());
        // The sets should differ — convert to sorted positions and compare.
        let mut a_pos: Vec<[i32; 3]> = a.iter().map(|x| x.1).collect();
        let mut b_pos: Vec<[i32; 3]> = b.iter().map(|x| x.1).collect();
        a_pos.sort();
        b_pos.sort();
        assert_ne!(a_pos, b_pos);
    }

    #[test]
    fn default_spawn_rules_has_critter_with_moisture_condition() {
        // Smoke test: the default set isn't empty, and the first rule
        // has the moisture condition the docs (L165-168) reference.
        let rules = default_spawn_rules();
        assert!(!rules.is_empty());
        let critter = rules.iter().find(|r| r.kind == EntityKind::Critter).unwrap();
        assert!(critter.conditions.iter().any(|c| c.field_name == "moisture"));
    }

    #[test]
    fn evaluate_spawn_rules_handles_missing_field() {
        // Field reader returns None for everything. Conditions should
        // fail safely; no spawns.
        let rules = vec![SpawnRule::new(EntityKind::Critter, 1.0)
            .with_condition(SpawnCondition::at_least("moisture", 0.5))];
        let field: Box<dyn Fn(&str, i32, i32, i32) -> Option<f32>> =
            Box::new(|_, _, _, _| None);
        let positions = vec![[0, 64, 0], [1, 64, 0]];
        let out = evaluate_spawn_rules(&rules, &field, &positions, 1);
        assert!(out.is_empty());
    }

    // ---- Render-bake tests (v0.24.62-entity-shaders) ----

    #[test]
    fn entity_vertex_size_matches_shader_layout() {
        // EntityVertex is 8 floats = 32 bytes, repr(C), no padding: position[3] @0,
        // color[3] @12, alpha @24, sway @28 (r599). NOTE: this assertion was stale
        // (asserted 24 after r535's `alpha` made it 28) until r599 fixed it while
        // adding `sway`; entity_pipeline.rs's manual field-by-field byte-packer is
        // the actual upload path and is not affected by Rust struct padding either
        // way, but keeping this in sync is cheap insurance.
        assert_eq!(std::mem::size_of::<EntityVertex>(), 32);
        assert_eq!(std::mem::align_of::<EntityVertex>(), 4);
    }

    #[test]
    fn default_color_for_kind_distinct_per_kind() {
        let c1 = default_color_for_kind(EntityKind::Critter);
        let c2 = default_color_for_kind(EntityKind::DroppedItem {
            item: crate::world::blocks::BlockId::Stone,
        });
        let c3 = default_color_for_kind(EntityKind::Particle);
        assert_ne!(c1, c2);
        assert_ne!(c2, c3);
        assert_ne!(c1, c3);
    }

    #[test]
    fn snapshot_to_vertices_produces_8_verts_36_indices_per_entity() {
        let mut store = EntityStore::new();
        store.spawn(EntityKind::Critter, [0.0, 0.0, 0.0]);
        store.spawn(EntityKind::Critter, [10.0, 0.0, 0.0]);
        let snap = EntityRenderSnapshot::capture(&store);
        let (verts, indices) = snapshot_to_vertices(&snap, default_color_for_kind);
        assert_eq!(verts.len(), 16);    // 2 entities × 8 vertices
        assert_eq!(indices.len(), 72);  // 2 entities × 36 indices
    }

    #[test]
    fn snapshot_to_vertices_empty_snapshot_returns_empty() {
        let store = EntityStore::new();
        let snap = EntityRenderSnapshot::capture(&store);
        let (verts, indices) = snapshot_to_vertices(&snap, default_color_for_kind);
        assert!(verts.is_empty());
        assert!(indices.is_empty());
    }

    #[test]
    fn snapshot_to_vertices_corners_match_aabb() {
        let mut store = EntityStore::new();
        store.spawn(EntityKind::Critter, [0.0, 1.0, 0.0]);
        let snap = EntityRenderSnapshot::capture(&store);
        let aabb = snap.items()[0].aabb;
        let (verts, _) = snapshot_to_vertices(&snap, default_color_for_kind);
        // Find the min-corner vertex and verify it's at AABB min.
        let has_min_corner = verts
            .iter()
            .any(|v| v.position == [aabb.min[0], aabb.min[1], aabb.min[2]]);
        let has_max_corner = verts
            .iter()
            .any(|v| v.position == [aabb.max[0], aabb.max[1], aabb.max[2]]);
        assert!(has_min_corner);
        assert!(has_max_corner);
    }

    #[test]
    fn snapshot_to_vertices_indices_in_bounds() {
        // Sanity: every index must refer to a valid vertex.
        let mut store = EntityStore::new();
        for i in 0..5 {
            store.spawn(EntityKind::Critter, [i as f32, 0.0, 0.0]);
        }
        let snap = EntityRenderSnapshot::capture(&store);
        let (verts, indices) = snapshot_to_vertices(&snap, default_color_for_kind);
        for &i in indices.iter() {
            assert!((i as usize) < verts.len(), "index {} out of bounds (verts.len() = {})", i, verts.len());
        }
    }

    #[test]
    fn snapshot_to_vertices_color_fn_applied() {
        let mut store = EntityStore::new();
        store.spawn(EntityKind::Critter, [0.0, 0.0, 0.0]);
        let snap = EntityRenderSnapshot::capture(&store);
        let custom_color = [0.5, 0.6, 0.7];
        let (verts, _) = snapshot_to_vertices(&snap, |_| custom_color);
        for v in verts.iter() {
            assert_eq!(v.color, custom_color);
        }
    }
}

#[cfg(test)]
mod forge_render_tests {
    use super::*;

    #[test]
    fn spawned_dragon_renders_as_forge_voxels() {
        let d = crate::spawn::spawn(crate::item::CreatureKind::Dragon, [0.0, 20.0, 0.0]);
        assert!(d.forge.is_some(), "a spawned dragon carries the forge model");
        let (v, i) = creature_to_vertices(&d);
        let boxes = d.forge.as_ref().unwrap().boxes.len();
        assert_eq!(v.len(), boxes * 8, "one 8-corner cube per merged box");
        assert_eq!(i.len(), boxes * 36);
        assert!(boxes > 30, "the forge dragon has a real shell, got {boxes}");
        assert!(i.iter().all(|&ix| (ix as usize) < v.len()));
    }

    #[test]
    fn animation_changes_the_mesh_but_keeps_count() {
        let mut d = crate::spawn::spawn(crate::item::CreatureKind::Dragon, [0.0, 20.0, 0.0]);
        let (rest, _) = creature_to_vertices(&d);
        // walking + idle clock should move some vertices
        d.gait_phase = 1.1;
        d.anim_time = 0.8;
        let (moving, _) = creature_to_vertices(&d);
        assert_eq!(moving.len(), rest.len(), "animation must not change the primitive count");
        let any_moved = rest.iter().zip(moving.iter()).any(|(a, b)| a.position != b.position);
        assert!(any_moved, "gait + idle clock should visibly animate the voxel dragon");
    }

    #[test]
    fn airborne_velocity_changes_the_wing_pose() {
        // r117: forge_to_vertices derives WingPose from body velocity — a climbing
        // dragon must mesh differently (Beating) from a grounded one (Spread), and
        // a fast-descending one differently again (Flared).
        let mut d = crate::spawn::spawn(crate::item::CreatureKind::Dragon, [0.0, 40.0, 0.0]);
        let (grounded, _) = creature_to_vertices(&d);
        d.body.velocity = [0.0, 3.0, 0.0]; // climbing
        let (climbing, _) = creature_to_vertices(&d);
        assert_eq!(grounded.len(), climbing.len(), "pose must not change primitive count");
        assert!(
            grounded.iter().zip(climbing.iter()).any(|(a, b)| a.position != b.position),
            "climbing should switch wings to Beating and move the mesh"
        );
        d.body.velocity = [0.0, -4.0, 0.0]; // fast descent
        let (descending, _) = creature_to_vertices(&d);
        assert!(
            climbing.iter().zip(descending.iter()).any(|(a, b)| a.position != b.position),
            "fast descent should switch wings to Flared (differs from Beating)"
        );
    }

    #[test]
    fn gaze_turns_the_head() {
        let mut d = crate::spawn::spawn(crate::item::CreatureKind::Dragon, [0.0, 20.0, 0.0]);
        let (ahead, _) = creature_to_vertices(&d);
        d.gaze_yaw = 0.8; // look hard to one side
        let (looking, _) = creature_to_vertices(&d);
        assert_eq!(ahead.len(), looking.len());
        let moved = ahead.iter().zip(looking.iter()).any(|(a, b)| a.position != b.position);
        assert!(moved, "setting gaze_yaw should rotate the head/neck voxels");
    }
}
