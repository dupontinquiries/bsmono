//! Per-chunk overlay fields for derived world state.
//!
//! Some world state — light, moisture, temperature, wind, pressure — can be
//! recomputed from the block grid + neighbors + simulation rules. There's no
//! point storing it persistently: it's derivable. This module is the home
//! for those *recomputable overlays*.
//!
//! Architectural distinction from `Chunk.explicit_indices`:
//!
//!   * `Chunk.blocks` + `Chunk.explicit_indices` = **persistent state**.
//!     Blocks must be saved because they came from worldgen + edits and
//!     can't be re-derived from anything cheaper. The explicit-index list
//!     tracks which blocks have been edited so storage knows what to save.
//!
//!   * `WorldFields` (this module) = **derived state**.
//!     Light, wind, moisture, etc. are all computable from blocks + ticks.
//!     Never saved. Never networked directly (clients re-derive from blocks).
//!     Cheap to throw away on chunk eviction; cheap to recompute on load.
//!
//! For multiplayer, this distinction is critical: the "ground truth" each
//! player sees is the block grid + their explicit-edit overlay. Derived
//! fields are local computations that converge toward consistency over
//! time as edits propagate. Diff trees only need to merge block edits —
//! the field state regenerates on its own.
//!
//! Resolution: each field is an 8×8×8 grid per chunk = 512 samples per
//! 32³ chunk. That's 1/64 block resolution. Plenty for slowly-varying
//! continuous fields (wind, moisture). Light may want full 32³ later;
//! the framework supports per-field resolution.
//!
//! Performance: the framework is designed for lazy evaluation. Fields
//! derive on first read, cache until invalidated. Greedy multipass
//! re-derivation (parallel field updates per chunk per tick) is the
//! intended scale-out path; this MVP is single-threaded.

use crate::world::chunk::{Chunk, ChunkCoord};
use std::collections::HashMap;
use std::sync::Arc;

/// Resolution of a field per chunk axis. 8 = 1/4 block resolution
/// (each sample covers a 4×4×4 block region). Lower than blocks for cost.
pub const FIELD_RESOLUTION: usize = 8;
pub const FIELD_VOLUME: usize = FIELD_RESOLUTION * FIELD_RESOLUTION * FIELD_RESOLUTION;

/// A single per-chunk scalar field. Samples laid out X-major (so
/// `samples[x + y * R + z * R * R]`).
#[derive(Debug, Clone)]
pub struct ScalarFieldGrid {
    pub samples: Vec<f32>,
}

impl ScalarFieldGrid {
    pub fn zeros() -> Self {
        Self { samples: vec![0.0; FIELD_VOLUME] }
    }

    pub fn filled(value: f32) -> Self {
        Self { samples: vec![value; FIELD_VOLUME] }
    }

    /// Build a stack-allocated snapshot. Used by tick handlers to
    /// pull a chunk's field data once per chunk per tick (under a
    /// brief lock), then sample it cheaply per-plant without holding
    /// any lock or paying string-hash + DashMap costs per sample.
    ///
    /// Cost is one memcpy of `FIELD_VOLUME * 4` bytes (≈2KB for
    /// FIELD_RESOLUTION=8) — much cheaper than dozens of locked
    /// `field_at` calls per chunk.
    #[inline]
    pub fn snapshot(&self) -> ChunkFieldSnapshot {
        let mut out = [0.0f32; FIELD_VOLUME];
        // copy_from_slice with the matching length is the fastest path.
        // The Vec is sized to FIELD_VOLUME at construction time.
        let n = self.samples.len().min(FIELD_VOLUME);
        out[..n].copy_from_slice(&self.samples[..n]);
        ChunkFieldSnapshot { samples: out }
    }

    /// Index a sample by 0..FIELD_RESOLUTION coords.
    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> f32 {
        self.samples[x + y * FIELD_RESOLUTION + z * FIELD_RESOLUTION * FIELD_RESOLUTION]
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, value: f32) {
        self.samples[x + y * FIELD_RESOLUTION + z * FIELD_RESOLUTION * FIELD_RESOLUTION] = value;
    }

    /// Sample at a chunk-local block position in [0, 32). Uses nearest-
    /// neighbor lookup at field resolution; trilinear interpolation is
    /// future work for fields that care about smoothness.
    pub fn sample_at_block(&self, x: i32, y: i32, z: i32) -> f32 {
        let scale = FIELD_RESOLUTION as i32 / 32;
        let scale = scale.max(1);
        // Map block coord to field-grid coord.
        let fx = ((x * FIELD_RESOLUTION as i32) / 32).clamp(0, FIELD_RESOLUTION as i32 - 1);
        let fy = ((y * FIELD_RESOLUTION as i32) / 32).clamp(0, FIELD_RESOLUTION as i32 - 1);
        let fz = ((z * FIELD_RESOLUTION as i32) / 32).clamp(0, FIELD_RESOLUTION as i32 - 1);
        let _ = scale;
        self.get(fx as usize, fy as usize, fz as usize)
    }
}

/// Stack-allocated copy of a single chunk's field data. Returned by
/// `ScalarFieldGrid::snapshot()`. Provides the same `sample_at_block`
/// API as the live grid but holds no lock and is freely copyable.
///
/// The point: tick handlers that loop over many plants in one chunk
/// pull a single snapshot at the top of the chunk loop and sample it
/// per-plant with no locking overhead. Replaces what used to be
/// hundreds of `field_at` calls (each a lock + DashMap + string hash)
/// with one snapshot-build + N cache-hot array reads.
#[derive(Debug, Clone, Copy)]
pub struct ChunkFieldSnapshot {
    samples: [f32; FIELD_VOLUME],
}

impl ChunkFieldSnapshot {
    /// Sample at a chunk-local block position in [0, CHUNK_SIZE). Mirrors
    /// `ScalarFieldGrid::sample_at_block` exactly.
    #[inline]
    pub fn sample_at_block(&self, x: i32, y: i32, z: i32) -> f32 {
        let fx = ((x * FIELD_RESOLUTION as i32) / 32).clamp(0, FIELD_RESOLUTION as i32 - 1);
        let fy = ((y * FIELD_RESOLUTION as i32) / 32).clamp(0, FIELD_RESOLUTION as i32 - 1);
        let fz = ((z * FIELD_RESOLUTION as i32) / 32).clamp(0, FIELD_RESOLUTION as i32 - 1);
        self.samples[fx as usize
            + fy as usize * FIELD_RESOLUTION
            + fz as usize * FIELD_RESOLUTION * FIELD_RESOLUTION]
    }
}

/// A registered field's derive function. Takes the chunk and an output
/// grid to populate. May read neighbors via the chunk_lookup callback.
///
/// Pure derive functions are easier to parallelize. For fields that need
/// neighbor information (e.g. light propagating between chunks), the
/// chunk_lookup gives bounded read access.
pub type DeriveFn = Arc<dyn Fn(&Chunk, &mut ScalarFieldGrid) + Send + Sync>;

/// Predicate that decides whether a block change at the given local coord
/// invalidates this field. Default: every block change invalidates the
/// whole field (conservative, correct, cheap for fields that recompute
/// fast).
pub type InvalidationFn = Arc<dyn Fn(i32, i32, i32) -> bool + Send + Sync>;

/// Definition of a registered field — its name, derive function, and
/// invalidation rule.
pub struct FieldDef {
    pub name: &'static str,
    pub derive: DeriveFn,
    pub invalidates_on_edit: InvalidationFn,
}

/// Registry of all fields the engine knows about. Built once at startup
/// (typically) and shared across the world.
pub struct FieldRegistry {
    defs: Vec<FieldDef>,
    // Index by name for O(1) lookup.
    by_name: HashMap<&'static str, usize>,
}

impl FieldRegistry {
    pub fn new() -> Self {
        Self { defs: Vec::new(), by_name: HashMap::new() }
    }

    pub fn register(&mut self, def: FieldDef) {
        let idx = self.defs.len();
        self.by_name.insert(def.name, idx);
        self.defs.push(def);
    }

    pub fn len(&self) -> usize { self.defs.len() }
    pub fn is_empty(&self) -> bool { self.defs.is_empty() }

    pub fn iter(&self) -> impl Iterator<Item = (usize, &FieldDef)> {
        self.defs.iter().enumerate()
    }

    pub fn get(&self, name: &str) -> Option<(usize, &FieldDef)> {
        self.by_name.get(name).map(|&i| (i, &self.defs[i]))
    }

    /// Build a registry with all built-in fields, using a real `WorldModel`
    /// for fields that need it (currently moisture). Use this in production;
    /// `Default::default()` uses stubs and is for tests only.
    pub fn with_world_model(model: std::sync::Arc<crate::world::world_model::WorldModel>) -> Self {
        let mut r = Self::new();
        r.register(wind::field_def());
        r.register(moisture::field_def(model));
        r
    }
}

impl Default for FieldRegistry {
    fn default() -> Self {
        let mut r = Self::new();
        // Stub fields for tests / contexts without a WorldModel. Production
        // code should use `FieldRegistry::with_world_model(...)` for the
        // real moisture derivation.
        r.register(wind::field_def());
        r.register(moisture_stub::field_def());
        r
    }
}

/// Stub moisture field for tests / contexts without a WorldModel.
mod moisture_stub {
    use super::*;
    pub fn field_def() -> FieldDef {
        FieldDef {
            name: super::moisture::FIELD_NAME,
            derive: Arc::new(|_chunk: &Chunk, grid: &mut ScalarFieldGrid| {
                for s in &mut grid.samples { *s = 0.5; }
            }),
            invalidates_on_edit: Arc::new(|_, _, _| false),
        }
    }
}

/// Per-chunk field cache. Holds one grid per registered field, plus a
/// validity bitset (one bit per field) so we know which fields are
/// stale and need re-deriving.
pub struct ChunkFields {
    pub coord: ChunkCoord,
    grids: Vec<ScalarFieldGrid>,
    valid: Vec<bool>,
}

impl ChunkFields {
    pub fn new(coord: ChunkCoord, num_fields: usize) -> Self {
        Self {
            coord,
            grids: (0..num_fields).map(|_| ScalarFieldGrid::zeros()).collect(),
            valid: vec![false; num_fields],
        }
    }

    /// Get a field grid, deriving it lazily if stale.
    pub fn get_or_derive(
        &mut self,
        field_idx: usize,
        registry: &FieldRegistry,
        chunk: &Chunk,
    ) -> &ScalarFieldGrid {
        if !self.valid[field_idx] {
            let def = &registry.defs[field_idx];
            (def.derive)(chunk, &mut self.grids[field_idx]);
            self.valid[field_idx] = true;
        }
        &self.grids[field_idx]
    }

    /// Mark a field stale. Called when a block edit invalidates it.
    pub fn invalidate(&mut self, field_idx: usize) {
        if field_idx < self.valid.len() {
            self.valid[field_idx] = false;
        }
    }

    /// Mark all fields stale that care about an edit at (x,y,z).
    /// Called by the chunk manager on `set_block`.
    pub fn invalidate_for_edit(&mut self, registry: &FieldRegistry, x: i32, y: i32, z: i32) {
        for (i, def) in registry.iter() {
            if (def.invalidates_on_edit)(x, y, z) {
                self.valid[i] = false;
            }
        }
    }
}

// =============================================================================
// Built-in field stubs — real implementations land in their own modules.
// =============================================================================

pub mod wind {
    //! Wind field — air motion vector at sub-block resolution.
    //!
    //! STUB. The real wind solver should:
    //! 1. Compute horizontal pressure gradients from biome temperature
    //!    (warm air rises → low pressure aloft, high pressure at surface).
    //! 2. Add altitude-driven flow (mountain valleys channel wind).
    //! 3. Treat solid blocks as obstacles (wind flows around).
    //! 4. Settle to steady state per region per tick (one Jacobi sweep).
    //!
    //! For multiplayer, wind is a perfect derived field — clients re-derive
    //! it locally from blocks + biomes + a synced "weather seed" + game time.
    //! No replication needed.
    //!
    //! Today this is a stub returning a constant scalar (will become a 3D
    //! vector once we have proper field types).

    use super::*;

    pub const FIELD_NAME: &str = "wind";

    pub fn field_def() -> FieldDef {
        FieldDef {
            name: FIELD_NAME,
            derive: Arc::new(|chunk: &Chunk, grid: &mut ScalarFieldGrid| {
                // Sample the analytical wind magnitude function at the
                // chunk's center for each field grid cell. Magnitude
                // varies slowly in space (regional gusts), so per-chunk
                // sampling is enough resolution. Direction (which DOES
                // change with time) is sampled directly by consumers via
                // `crate::wind::wind_at(...)`, NOT through this field.
                let (origin_x, origin_y, origin_z) = chunk.coord.block_origin();
                let stride = (crate::constants::CHUNK_SIZE as f32) / FIELD_RESOLUTION as f32;
                let half = stride * 0.5;
                for fz in 0..FIELD_RESOLUTION {
                    let sz = origin_z as f64 + fz as f64 * stride as f64 + half as f64;
                    for fy in 0..FIELD_RESOLUTION {
                        let sy = origin_y as f64 + fy as f64 * stride as f64 + half as f64;
                        for fx in 0..FIELD_RESOLUTION {
                            let sx = origin_x as f64 + fx as f64 * stride as f64 + half as f64;
                            // world_time=0 — magnitude isn't time-varying
                            // in the current model.
                            let (_, _, m) = crate::wind::wind_at(sx, sy, sz, 0.0);
                            let idx = fx + fy * FIELD_RESOLUTION + fz * FIELD_RESOLUTION * FIELD_RESOLUTION;
                            grid.samples[idx] = m;
                        }
                    }
                }
            }),
            invalidates_on_edit: Arc::new(|_x, _y, _z| false),
        }
    }
}

pub mod moisture {
    //! Moisture field — how wet is the air/ground at this point?
    //!
    //! Drives plant growth rate, fire spread, mob spawning, sound
    //! propagation, water evaporation. Two contributions:
    //!
    //! - **Biome humidity baseline.** Sampled from the world model at
    //!   the chunk's center. Tropical chunks start near 0.8; deserts
    //!   start near 0.1.
    //!
    //! - **Water/lava proximity boost.** Each water cell in the chunk
    //!   contributes a falloff to nearby field samples. Lava DECREASES
    //!   nearby moisture (heat dries the air). Boost is bounded so a
    //!   chunk full of water doesn't drown the field at 1.0 everywhere
    //!   instantly — the biome ceiling caps unboosted dry chunks.
    //!
    //! Not yet handled (future turns):
    //! - Cross-chunk diffusion (water in chunk N+1 doesn't bleed into N's
    //!   moisture). Needs the multipass framework.
    //! - Time-based evaporation / weather events.
    //! - Plant extraction lowering local moisture as plants grow.
    //!
    //! Resolution: 8³ field samples per 32³ chunk. Each sample covers a
    //! 4-block region; we sample its center for biome lookup.

    use super::*;
    use crate::world::blocks::BlockId;
    use crate::world::world_model::WorldModel;

    pub const FIELD_NAME: &str = "moisture";

    /// Per-water-block contribution to moisture, applied with falloff.
    const WATER_BOOST: f32 = 0.4;
    /// Per-lava-block penalty (lava dries the air).
    const LAVA_PENALTY: f32 = 0.3;
    /// Distance in blocks beyond which a fluid cell stops affecting a sample.
    const FLUID_INFLUENCE_RADIUS: f32 = 6.0;

    /// Build a moisture field def backed by a real `WorldModel`. The
    /// closure captures an `Arc<WorldModel>` so multiple chunks can derive
    /// in parallel without contention (model is Send+Sync, read-only).
    pub fn field_def(model: Arc<WorldModel>) -> FieldDef {
        FieldDef {
            name: FIELD_NAME,
            derive: Arc::new(move |chunk: &Chunk, grid: &mut ScalarFieldGrid| {
                let (origin_x, origin_y, origin_z) = chunk.coord.block_origin();
                let cx = (origin_x + crate::constants::CHUNK_SIZE / 2) as f64;
                let cy = (origin_y + crate::constants::CHUNK_SIZE / 2) as f64;
                let cz = (origin_z + crate::constants::CHUNK_SIZE / 2) as f64;
                // Biome baseline once per chunk — humidity varies slowly,
                // sampling per-cell would be overkill.
                let baseline = model.humidity(cx, cy, cz, 0.0) as f32;

                // Initialize the field to baseline.
                for s in &mut grid.samples {
                    *s = baseline;
                }

                // Pass over chunk blocks looking for water/lava influence.
                // To avoid scanning all 32k blocks for every sample, we
                // build a list of fluid sources first, then for each field
                // sample we accumulate contributions from sources within
                // FLUID_INFLUENCE_RADIUS.
                let mut sources: Vec<(f32, f32, f32, f32)> = Vec::new(); // (wx, wy, wz, signed_strength)
                for ly in 0..crate::constants::CHUNK_SIZE {
                    for lz in 0..crate::constants::CHUNK_SIZE {
                        for lx in 0..crate::constants::CHUNK_SIZE {
                            let b = chunk.get(lx, ly, lz);
                            let strength = match b {
                                BlockId::Water => WATER_BOOST,
                                BlockId::Lava => -LAVA_PENALTY,
                                _ => continue,
                            };
                            sources.push((
                                (origin_x + lx) as f32,
                                (origin_y + ly) as f32,
                                (origin_z + lz) as f32,
                                strength,
                            ));
                        }
                    }
                }

                if sources.is_empty() { return; }

                // Apply contributions per field sample. Each sample sits
                // at a 4-block stride (32 / 8 = 4); we use the sample's
                // CENTER world position so falloff is symmetric around it.
                let stride = (crate::constants::CHUNK_SIZE as f32) / FIELD_RESOLUTION as f32;
                let half = stride * 0.5;
                let r2 = FLUID_INFLUENCE_RADIUS * FLUID_INFLUENCE_RADIUS;
                for fz in 0..FIELD_RESOLUTION {
                    let sz = origin_z as f32 + fz as f32 * stride + half;
                    for fy in 0..FIELD_RESOLUTION {
                        let sy = origin_y as f32 + fy as f32 * stride + half;
                        for fx in 0..FIELD_RESOLUTION {
                            let sx = origin_x as f32 + fx as f32 * stride + half;
                            let mut acc = 0.0_f32;
                            for &(wx, wy, wz, str_) in &sources {
                                let dx = sx - wx;
                                let dy = sy - wy;
                                let dz = sz - wz;
                                let d2 = dx*dx + dy*dy + dz*dz;
                                if d2 > r2 { continue; }
                                // Linear falloff: 1 at distance 0, 0 at radius.
                                let d = d2.sqrt();
                                let f = 1.0 - d / FLUID_INFLUENCE_RADIUS;
                                acc += str_ * f;
                            }
                            let idx = fx + fy * FIELD_RESOLUTION + fz * FIELD_RESOLUTION * FIELD_RESOLUTION;
                            grid.samples[idx] = (grid.samples[idx] + acc).clamp(0.0, 1.0);
                        }
                    }
                }
            }),
            // Moisture is invalidated when water/lava is placed or removed.
            // For other blocks (stone edits, etc.) the field stays valid.
            // We can't filter by block-id here without seeing the new block,
            // so we conservatively invalidate on any edit. A future turn
            // can plumb the new BlockId into the predicate for tighter
            // invalidation.
            invalidates_on_edit: Arc::new(|_x, _y, _z| true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_default_has_known_fields() {
        let r = FieldRegistry::default();
        assert!(r.get(wind::FIELD_NAME).is_some());
        assert!(r.get(moisture::FIELD_NAME).is_some());
    }

    #[test]
    fn lazy_derive_runs_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        let mut r = FieldRegistry::new();
        r.register(FieldDef {
            name: "test",
            derive: Arc::new(move |_chunk, grid| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                grid.samples[0] = 42.0;
            }),
            invalidates_on_edit: Arc::new(|_,_,_| false),
        });

        let chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let mut fields = ChunkFields::new(chunk.coord, r.len());
        // First read derives.
        let _ = fields.get_or_derive(0, &r, &chunk);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        // Subsequent reads use cache.
        let _ = fields.get_or_derive(0, &r, &chunk);
        let _ = fields.get_or_derive(0, &r, &chunk);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        // After invalidation, next read re-derives.
        fields.invalidate(0);
        let _ = fields.get_or_derive(0, &r, &chunk);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn invalidation_predicate_only_marks_relevant_fields() {
        let mut r = FieldRegistry::new();
        // Field A: invalidated by edits at y < 10.
        r.register(FieldDef {
            name: "low_only",
            derive: Arc::new(|_, _| {}),
            invalidates_on_edit: Arc::new(|_, y, _| y < 10),
        });
        // Field B: invalidated by edits at y >= 10.
        r.register(FieldDef {
            name: "high_only",
            derive: Arc::new(|_, _| {}),
            invalidates_on_edit: Arc::new(|_, y, _| y >= 10),
        });

        let mut fields = ChunkFields::new(ChunkCoord::new(0, 0, 0), 2);
        fields.valid = vec![true, true];
        fields.invalidate_for_edit(&r, 5, 5, 5);
        assert!(!fields.valid[0]);
        assert!(fields.valid[1]);

        fields.valid = vec![true, true];
        fields.invalidate_for_edit(&r, 5, 25, 5);
        assert!(fields.valid[0]);
        assert!(!fields.valid[1]);
    }

    #[test]
    fn moisture_field_boosts_near_water() {
        // A chunk with a water cell should have higher moisture near it
        // than a chunk without one.
        use crate::world::blocks::BlockId;
        use crate::world::world_model::WorldModel;

        let model = std::sync::Arc::new(WorldModel::new(0xCAFE));
        let registry = FieldRegistry::with_world_model(model);
        let (idx, _def) = registry.get(moisture::FIELD_NAME).unwrap();

        // Dry chunk: no fluids. Pure baseline.
        let dry_chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let mut dry_fields = ChunkFields::new(dry_chunk.coord, registry.len());
        let dry_grid = dry_fields.get_or_derive(idx, &registry, &dry_chunk).clone();

        // Wet chunk: a water block right at the center.
        let mut wet_chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        wet_chunk.set(16, 16, 16, BlockId::Water);
        let mut wet_fields = ChunkFields::new(wet_chunk.coord, registry.len());
        let wet_grid = wet_fields.get_or_derive(idx, &registry, &wet_chunk).clone();

        // Sample the field at the chunk's center (field-grid (4,4,4) ≈
        // block (16,16,16)). Wet should be measurably higher than dry.
        let dry_v = dry_grid.get(4, 4, 4);
        let wet_v = wet_grid.get(4, 4, 4);
        assert!(wet_v > dry_v + 0.05,
            "moisture field should boost near water: dry={}, wet={}", dry_v, wet_v);
    }
}
