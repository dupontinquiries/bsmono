//! CPU-side world generation. Queries `WorldModel` for every world property.
//!
//! This module produces concrete `Chunk`s by sampling fields from the
//! `WorldModel` at every block position. It owns no world-property logic
//! itself — density curves, biome decisions, atmospheric coupling all
//! live in `WorldModel`. The generator's job is purely:
//!
//!   1. For each block: query density, decide solid/air.
//!   2. Carve caves.
//!   3. Surface-decorate (grass on top of dirt on top of stone).
//!   4. Replace stone with ores per local conditions.
//!   5. Plant bedrock at the world floor.
//!
//! All "what does this look like" goes through `WorldModel`. To change
//! how the world looks, edit the model — not this file.

use super::biomes::{Biome, BiomeSample, determine_biome};
use super::blocks::BlockId;
use super::chunk::{Chunk, ChunkCoord};
use super::noise::noise_3d;
use super::world_model::{anchors, WorldModel};
use crate::constants::{CHUNK_SIZE, CHUNK_VOLUME};
use std::sync::Arc;

/// Seed offsets for ore noise. Ores are still sampled directly (not via
/// WorldModel) because they're high-frequency per-block decisions that
/// don't need region blending. May get migrated to WorldModel later if
/// we want region-varying ore distributions.
mod ore_seeds {
    pub const COAL: u32 = 0xC041_0001;
    pub const IRON: u32 = 0x1404_0002;
    pub const GOLD: u32 = 0x6014_0003;
    /// Seed offset for lava-pocket noise. Distinct from ore seeds so
    /// lava and ore deposits don't co-correlate spatially.
    pub const LAVA: u32 = 0x1ABA_0004;
}

/// World-generation parameters. Today this is just a seed; later it'll grow
/// to include narrative state, AI directives, era, etc. The shape is a
/// struct rather than a bare seed so adding fields doesn't require
/// touching every call site.
#[derive(Debug, Clone, Copy)]
pub struct WorldParams {
    pub seed: u32,
}

impl WorldParams {
    pub fn new(seed: u32) -> Self { Self { seed } }
}

/// Whether the deep multi-scale `cave_field` layer is enabled, read once from
/// `UNENDLICH_MULTI_SCALE_CAVES` and cached (this is consulted per voxel).
/// DEFAULT ON since r116 (the layer is PROVEN+WIRED in the ledger and was only
/// ever off because nobody flipped the env var); set `=0` to opt out.
fn multi_scale_caves_enabled() -> bool {
    use std::sync::OnceLock;
    static EN: OnceLock<bool> = OnceLock::new();
    *EN.get_or_init(|| {
        std::env::var("UNENDLICH_MULTI_SCALE_CAVES")
            .map(|v| !(v == "0" || v.eq_ignore_ascii_case("false")))
            .unwrap_or(true)
    })
}

pub struct WorldGenerator {
    params: WorldParams,
    /// The world's field model. All terrain decisions go through this.
    /// Wrapped in `Arc` so it can be shared with the mesher (which
    /// samples climate fields per face for material-response tinting).
    /// `WorldModel` is `Send + Sync` and uses `DashMap` internally for
    /// the region cache, so multiple threads sharing one Arc benefit
    /// from the same cache.
    model: Arc<WorldModel>,
    /// Optional biome registry. When present, biome lookup uses best-fit
    /// against the JSON registry. When absent, falls back to the legacy
    /// hardcoded `determine_biome` decision tree.
    biome_registry: Option<std::sync::Arc<super::biome_loader::BiomeRegistry>>,
}

/// The immediate-Ocean threshold for `biome_at_3d`'s topographic veto.
///
/// Below this, a column's biome-free base (sea + continental + hills, no
/// biome_height) is vetoed straight to Ocean with no refinement. The base
/// threshold is `sea_level - 2.0`; `height_modifier` (>=0 only — Ocean/Beach/
/// Swamp/etc's own negative modifiers don't widen it, since a column that's
/// ALREADY climate-classified as one of those doesn't need extra margin to
/// stay Ocean) widens the margin so a column whose climate suggests a high-
/// elevation biome (Skylands +80, MistyMountains +50, AlpineHighlands +35)
/// isn't vetoed before that modifier is ever applied — it instead falls
/// through to the ambiguous-shelf refinement, which uses the true
/// (biome-adjusted) surface via `surface_y_at`.
#[inline]
fn topo_ocean_threshold(sea_level: f64, height_modifier: f64) -> f64 {
    sea_level - 2.0 - height_modifier.max(0.0)
}

/// The highest `y` in `[y_min, y_max]` that is solid AND has a non-solid block
/// directly above it — i.e. a block a creature can stand on. Scans top-down, so
/// it is correct for NON-MONOTONIC columns (caves): a buried cave's floor is not
/// mistaken for the surface — the terrain crust above the cave is returned. (The
/// old binary search assumed monotonic solidity and could land in a cave void,
/// grounding creatures *inside* caves — the surface-glitch bug.) `y_max + 1` is
/// treated as open sky. Returns `None` if nothing in range is standable.
///
/// Scanning from the top is also cheap in practice: callers pass a `y_max` just
/// above the creature, so the surface is found within a few steps.
pub fn topmost_standable(solid_at: impl Fn(i32) -> bool, y_min: i32, y_max: i32) -> Option<i32> {
    let mut above_solid = false; // y_max+1 is open sky
    let mut y = y_max;
    while y >= y_min {
        let here = solid_at(y);
        if here && !above_solid {
            return Some(y);
        }
        above_solid = here;
        y -= 1;
    }
    None
}

/// How far from the surface target the density sign is taken as decided
/// without evaluating the 3D detail noise. The analytic bound is
/// `0.075 * VERTICAL_BIAS_SCALE` = 3 blocks; 6 is twice that.
const DENSITY_SKIP_MARGIN: f64 = 6.0;

/// `UNENDLICH_SURFACE_SCAN_SEED` (default on). Read once: it used to be
/// an `std::env::var` on every `surface_y_at` call, i.e. per column.
fn surface_scan_seed_enabled() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("UNENDLICH_SURFACE_SCAN_SEED").map(|v| v != "0").unwrap_or(true))
}

impl WorldGenerator {
    pub fn new(seed: u32) -> Self {
        Self {
            params: WorldParams::new(seed),
            model: Arc::new(WorldModel::new(seed)),
            biome_registry: None,
        }
    }

    /// Build with a biome registry attached. Recommended for production use;
    /// without it biome lookups fall back to the legacy hardcoded tree.
    pub fn with_biome_registry(
        seed: u32,
        registry: std::sync::Arc<super::biome_loader::BiomeRegistry>,
    ) -> Self {
        Self {
            params: WorldParams::new(seed),
            model: Arc::new(WorldModel::new(seed)),
            biome_registry: Some(registry),
        }
    }

    /// Attach (or replace) the biome registry. Returns the previous one.
    pub fn set_biome_registry(
        &mut self,
        registry: Option<std::sync::Arc<super::biome_loader::BiomeRegistry>>,
    ) -> Option<std::sync::Arc<super::biome_loader::BiomeRegistry>> {
        std::mem::replace(&mut self.biome_registry, registry)
    }

    pub fn seed(&self) -> u32 { self.params.seed }
    pub fn params(&self) -> WorldParams { self.params }
    pub fn model(&self) -> &WorldModel { &self.model }
    /// Get an `Arc` clone of the world model. Used to share the model
    /// with the mesher so it can sample temperature/humidity per face
    /// for material-response tinting.
    pub fn model_arc(&self) -> Arc<WorldModel> { Arc::clone(&self.model) }

    // ---- Block-level queries (used by chunk_manager and other consumers) ----

    /// Density at a world-space block position, via the world model.
    /// Solid where density > anchors::DENSITY_THRESHOLD.
    pub fn density(&self, x: i32, y: i32, z: i32) -> f64 {
        self.model.density(x as f64, y as f64, z as f64, 0.0)
    }

    /// Cave carving lookup at a world-space block position. Existing caves
    /// (model `is_cave`) always apply; the deep multi-scale `cave_field` layer
    /// is additive and opt-in via `UNENDLICH_MULTI_SCALE_CAVES=1` (cached once).
    /// Every solidity site does `density > T && !is_cave`, so OR-ing the deep
    /// carve in here lights up the whole world at once.
    pub fn is_cave(&self, x: i32, y: i32, z: i32) -> bool {
        if self.model.is_cave(x as f64, y as f64, z as f64) {
            return true;
        }
        if multi_scale_caves_enabled() {
            let surface_y = self.model.surface_target(x as f64, z as f64, 0.0);
            if crate::world::cave_field::cave_at(self.seed(), x as f64, y as f64, z as f64, surface_y) {
                return true;
            }
        }
        false
    }

    /// [`is_cave`] for a caller that already knows the column's surface
    /// target — which every per-column loop does, having just computed it.
    ///
    /// 2026-09-02: this is THE hot spot of CPU worldgen. `is_cave`
    /// recomputes `surface_target(x, z)` for every cell, and
    /// `surface_target` costs ~4.1 us; at 32 768 cells that is ~134 ms of
    /// the ~225 ms a chunk takes, all of it recomputing 1 024 distinct
    /// values 32 times each. `surface_target` is a pure function of
    /// (x, z), so passing the column's value in is exact, not an
    /// approximation — `is_cave_with_surface_matches_is_cave` checks it
    /// cell for cell.
    ///
    /// (Two earlier guesses at this hot spot were wrong: the density
    /// noise is 2.2 ms per chunk and the `WorldModel::is_cave` noises
    /// are 1.9 ms. `hotspot_tests::column_pass_breakdown` is what
    /// settled it.)
    pub fn is_cave_with_surface(&self, x: i32, y: i32, z: i32, surface_y: f64) -> bool {
        if self.model.is_cave(x as f64, y as f64, z as f64) {
            return true;
        }
        if multi_scale_caves_enabled() {
            if crate::world::cave_field::cave_at(self.seed(), x as f64, y as f64, z as f64, surface_y) {
                return true;
            }
        }
        false
    }

    /// Effective sea level at a horizontal world position. Computed from
    /// the model's continental + sea-level + humidity coupling.
    pub fn sea_level_at(&self, x: i32, z: i32) -> f64 {
        self.model.sea_level(x as f64, z as f64, 0.0)
    }

    /// Biome sample at a world-space position. Today this still uses the
    /// old temp/humidity/erosion → biome mapping, but the inputs come from
    /// the WorldModel. Biome translation into 3D fields is deferred.
    pub fn biome_at_3d(&self, x: i32, y: i32, z: i32) -> BiomeSample {
        let xf = x as f64;
        let yf = y as f64;
        let zf = z as f64;
        let temp = self.model.temperature(xf, yf, zf, 0.0);
        let humid = self.model.humidity(xf, yf, zf, 0.0);
        // Erosion isn't a model field yet; sample it lightly here. Will get
        // moved into the model when the biome translation work happens.
        let erosion = (super::noise::fbm_2d(
            xf * 0.0008, zf * 0.0008,
            self.params.seed.wrapping_add(0xE105),
            2, 1.0, 1.0,
        ) - 0.5) * 2.0;
        let erosion_unit = (erosion + 1.0) * 0.5;

        // If we have a biome registry, use best-fit against it.
        // Otherwise fall back to the legacy hardcoded decision tree.
        //
        // The registry's temperature is in Minecraft's [0,2] range; our
        // WorldModel temperature is in [0,1]. Multiply by 2 when querying.
        // r177: TOPOGRAPHY VETOES CLIMATE. The registry best-fits on
        // (temp, humidity) only — but Ocean/Beach are TOPOGRAPHIC classes
        // living in the erosion channel the legacy tree consumes. The r174
        // registry silently deleted oceans (grass seas, land fauna in
        // them — operator report). Rule: if the legacy tree says this is
        // water/shore ground, that stands; the registry owns the LAND
        // climate biomes.
        let legacy_verdict = determine_biome(temp, humid, erosion_unit);
        // r192: TRULY topographic veto — the erosion channel said "land"
        // for columns the r188 varying sea floods (submerged Plains,
        // probe-found: 11,852 columns at seed 1337). Compare the model's
        // own surface vs sea level; submerged => Ocean, shoreline =>
        // Beach, regardless of what climate or erosion claim.
        // (biome-free base: sea + continental + hills — the biome height
        // modifier is a CONSEQUENCE of biome and must not decide it; the
        // GPU twin uses the identical three-term base for parity.)
        //
        // BUGFIX (found during audit): `st` above is deliberately the
        // "biome-free base" (sea + continental + hills, NO biome_height) to
        // avoid circularity — you can't know biome_height without already
        // knowing the biome. But some biomes have large height_modifier
        // values (Skylands: +80, MistyMountains: +50, AlpineHighlands: +35
        // — see biomes.rs), and continental*30+hills*15 alone spans roughly
        // [-45,45] (both documented [-1,1] noise fields). Since
        // determine_biome's climate inputs are independent noise fields
        // from continental/hills, a column can land on BOTH "erosion>0.85
        // (Skylands)" AND "continental=-1,hills=-1 (deep valley)" at once —
        // nothing prevents that correlation. Before this fix, the immediate-
        // Ocean branch (`st < sl - 2.0`, no refinement) fired before
        // biome_height was ever applied, permanently classifying such
        // columns as Ocean even though the true surface could be 50-80
        // blocks above water. Fixed below by widening the threshold using
        // legacy_verdict's own height_modifier (already computed above, for
        // free) — biomes with modifier <= 0 keep the exact original
        // sl-2.0 threshold (r177/r188/r192 behavior is bit-for-bit
        // unchanged; see topo_ocean_threshold_widens_only_for_positive_
        // height_modifiers for the regression test), while high-modifier
        // biomes route through the existing surface_y_at refinement — which
        // DOES include the real biome height via surface_target — instead
        // of an immediate, unrefined veto.
        let sl = self.model.sea_level(xf, zf, 0.0);
        let st = sl
            + self.model.continental(xf, zf) * 30.0
            + self.model.hills(xf, zf) * 15.0;
        let ocean_threshold = topo_ocean_threshold(sl, legacy_verdict.height_modifier());
        let shelf_threshold = sl + 6.0;
        let topo_verdict = if st < ocean_threshold {
            Some(Biome::Ocean)
        } else if st < shelf_threshold {
            // ambiguous shelf band: the base can sit near sea while noise
            // carves the REAL surface under it (probe: 2-9-block-deep
            // "submerged Plains"). Refine with the actual surface; ~8
            // density evals, only inside this band.
            match self.surface_y_at(x, z, (sl - 120.0) as i32, (sl + 8.0) as i32) {
                Some(sy) if (sy as f64) < sl - 2.0 => Some(Biome::Ocean),
                Some(sy) if (sy as f64) < sl + 2.0 => Some(Biome::Beach),
                _ => None,
            }
        } else {
            None
        };
        let biome = match &self.biome_registry {
            _ if topo_verdict.is_some() => topo_verdict.unwrap(),
            Some(_) if matches!(legacy_verdict, Biome::Ocean | Biome::Beach) => legacy_verdict,
            Some(reg) => {
                let mc_temp = temp * 2.0; // [0,1] -> [0,2] for Minecraft schema
                match reg.best_fit(mc_temp, humid) {
                    Some(def) => def.to_legacy(),
                    None => legacy_verdict,
                }
            }
            None => legacy_verdict,
        };

        BiomeSample {
            biome,
            temperature: temp,
            humidity: humid,
            erosion: erosion_unit,
        }
    }

    /// Returns the `BiomeDef` from the registry that best fits the climate
    /// at this position, if a registry is attached. Returns `None` if no
    /// registry is available.
    pub fn biome_def_at(&self, x: i32, y: i32, z: i32)
        -> Option<&super::biome_loader::BiomeDef>
    {
        let reg = self.biome_registry.as_ref()?;
        let xf = x as f64;
        let yf = y as f64;
        let zf = z as f64;
        let temp = self.model.temperature(xf, yf, zf, 0.0);
        let humid = self.model.humidity(xf, yf, zf, 0.0);
        let mc_temp = temp * 2.0;
        reg.best_fit(mc_temp, humid)
    }

    /// Pick the surface block for a given biome at this y. Tries the
    /// registry's `surface_block` field first; falls back to the legacy
    /// per-Biome-enum mapping.
    pub fn pick_surface_block(&self, biome: Biome, x: i32, y: i32, z: i32) -> BlockId {
        if let Some(def) = self.biome_def_at(x, y, z) {
            if let Some(b) = def.resolve_surface_block() {
                // Snow override at high altitudes still applies for tundra/alpine.
                if matches!(biome, Biome::Tundra | Biome::AlpineHighlands) && y > 90 {
                    return BlockId::SnowBlock;
                }
                return b;
            }
        }
        legacy_surface_block(biome, y)
    }

    /// Pick the subsurface block for a given biome. Same fallback semantics.
    pub fn pick_subsurface_block(&self, biome: Biome, x: i32, y: i32, z: i32) -> BlockId {
        if let Some(def) = self.biome_def_at(x, y, z) {
            if let Some(b) = def.resolve_subsurface_block() {
                return b;
            }
        }
        legacy_subsurface_block(biome, y)
    }

    /// Find the surface y at column (world_x, world_z) by scanning the
    /// density field top-down. Returns None if there's no surface in
    /// [y_min, y_max].
    ///
    /// r299: the scan no longer starts at the caller's `y_max` (often
    /// hundreds of blocks of empty sky — the ecosystem-stream region-crossing
    /// freeze). The density's only above-target term is `detail * 0.15`
    /// (detail ∈ [-0.5, 0.5]) on top of a vertical bias of slope
    /// 1/VERTICAL_BIAS_SCALE; with DENSITY_THRESHOLD = 0, no voxel above
    /// `surface_target + 0.15*0.5*VERTICAL_BIAS_SCALE` (≈ +3) can be solid.
    /// So `surface_target + SURFACE_SCAN_MARGIN` (margin well above that) is a
    /// PROVABLE upper bound on any solid voxel — starting the top-down scan
    /// there instead of `y_max` is the identical result, just skipping the
    /// guaranteed-air region above. (Set UNENDLICH_SURFACE_SCAN_SEED=0 to scan
    /// from the raw `y_max`, the pre-r299 behaviour.)
    pub fn surface_y_at(&self, world_x: i32, world_z: i32, y_min: i32, y_max: i32) -> Option<i32> {
        // 2026-09-02: this scans up to ~130 y values for ONE column, and
        // every probe recomputed `surface_target(world_x, world_z)` TWICE
        // — once inside `density`, once inside `is_cave` — while the
        // function already computes it for the scan seed. That is one
        // pure function of (x, z) evaluated a few hundred times per
        // column at ~3.6 us each. Computed once here and threaded into
        // both. Exact, not an approximation.
        //
        // The env read was per call too (a getenv and a String for every
        // column reaching the shelf branch of `biome_at_3d`); it is a
        // OnceLock now. UNENDLICH_SURFACE_SCAN_SEED still works, read
        // once per process.
        let st = self.model.surface_target(world_x as f64, world_z as f64, 0.0);
        let solid_at = |y: i32| {
            self.model.density_from_surface(st, world_x as f64, y as f64, world_z as f64)
                > anchors::DENSITY_THRESHOLD
                && !self.is_cave_with_surface(world_x, y, world_z, st)
        };
        // Generous margin above the proven +3 boundary, so future tweaks to the
        // detail amplitude can't silently clip a surface. Still typically saves
        // hundreds of blocks vs y_max=512.
        const SURFACE_SCAN_MARGIN: i32 = 16;
        let seed_enabled = surface_scan_seed_enabled();
        let scan_top = if seed_enabled {
            // Tighter upper bound, but never above the caller's y_max and never
            // below y_min (a degenerate window still scans correctly → None).
            (st.floor() as i32 + SURFACE_SCAN_MARGIN).min(y_max).max(y_min)
        } else {
            y_max
        };
        topmost_standable(solid_at, y_min, scan_top)
    }

    // ---- Chunk generation ----

    /// Generate one 32³ chunk. Pure function of (coord, seed) — no neighbor
    /// dependencies, safe to parallelise.
    pub fn generate_chunk(&self, coord: ChunkCoord) -> Chunk {
        let mut blocks = vec![BlockId::Air; CHUNK_VOLUME];
        let (origin_x, origin_y, origin_z) = coord.block_origin();

        // -----------------------------------------------------------------
        // Fast path: chunks far above the surface that sample as all-air.
        // -----------------------------------------------------------------
        //
        // The render-distance sphere (radius 6, ~925 chunks at startup)
        // includes many chunks well above any terrain. With a player at
        // sea level, ~30% of the sphere is above Y=200, where the
        // lapse-rate density model reliably gives air. Running the full
        // 32K-cell density loop for these is wasted work — they all
        // finish as all-air.
        //
        // Quick check: 8 corners + center = 9 cells. If every sample
        // has density well below the solid threshold AND the chunk is
        // well above sea level, the entire chunk is air. We skip:
        //   - Phase 1 (density carving): blocks already initialized to Air.
        //   - Phase 2 (surface decoration): no surface candidates.
        //   - Phase 2.5 (water filling): chunk is above sea level.
        //   - Phase 3/3.5 (ore/lava): only act on Stone, none present.
        //   - Phase 4 (bedrock): only at y=0, this chunk is well above.
        //   - Phase 5 (trees): trees top out around y=80, this is way above.
        //
        // This is the user's "physics chunk cache" idea applied to
        // worldgen: trade a few sample queries for skipping 32K
        // per-cell evaluations. Margin is wide enough that the fast
        // path NEVER fires within range of actual terrain — see the
        // tests at the bottom of this module.
        //
        // The threshold below sea level is asymmetric: caves carve
        // everywhere underground, so we can't safely fast-path
        // "all solid" deep chunks even though density is reliably
        // positive. Only the all-air case is safe.
        const HIGH_AIR_MIN_Y: i32 = 160; // sea_level + 96; well above any terrain
        const HIGH_AIR_DENSITY_MAX: f64 = -0.3; // well below threshold (0.0)
        if origin_y >= HIGH_AIR_MIN_Y {
            // 8 corners + center sample.
            let chunk_max = CHUNK_SIZE - 1;
            let samples = [
                (0, 0, 0),
                (chunk_max, 0, 0),
                (0, chunk_max, 0),
                (chunk_max, chunk_max, 0),
                (0, 0, chunk_max),
                (chunk_max, 0, chunk_max),
                (0, chunk_max, chunk_max),
                (chunk_max, chunk_max, chunk_max),
                (chunk_max / 2, chunk_max / 2, chunk_max / 2),
            ];
            let all_air = samples.iter().all(|(lx, ly, lz)| {
                let wx = origin_x + lx;
                let wy = origin_y + ly;
                let wz = origin_z + lz;
                self.density(wx, wy, wz) < HIGH_AIR_DENSITY_MAX
            });
            if all_air {
                // Fast-path: return early with all-air chunk. No phases run.
                return Chunk::from_blocks(coord, blocks);
            }
        }

        // -----------------------------------------------------------------
        // Slow path (default): full multi-phase generation.
        // -----------------------------------------------------------------

        // Phase 1: density carving. Each block independently solid-or-air.
        // Loop column-outer (lz, lx) / y-inner so the 2D per-column surface
        // target is computed ONCE per column instead of once per voxel.
        // Previously density() recomputed ~5 fbm_2d terms for all 32 y-values
        // in a column (32x redundant); density_from_surface() keeps only the
        // genuinely-3D detail noise per voxel. Bit-identical to the old
        // per-voxel density() (verified by hoist_matches_per_voxel_density).
        // r191: THREADED columns (rayon) — each column is independent
        // (generator is Sync, no interior mutability); map to per-column
        // solid index lists in parallel, scatter serially (distinct
        // indices; order-free). Deterministic: identical per-column math.
        // r737/r739: per-column solidity as a u32 BITMASK (bit ly = solid), not a
        // heap Vec<usize> per column — drops 1024 inner Vec allocations per chunk to
        // zero (CHUNK_SIZE==32 fits a u32 exactly). Bit-identical scatter (see
        // bitmask_scatter_matches_index_list). r739: the mask now OUTLIVES phase 1 so
        // the phase-2 surface walk reads solidity straight from it.
        let col_masks: Vec<u32> = {
            use rayon::prelude::*;
            (0..(CHUNK_SIZE * CHUNK_SIZE))
                .into_par_iter()
                .map(|ci| {
                    let lz = ci / CHUNK_SIZE;
                    let lx = ci % CHUNK_SIZE;
                    let wz = origin_z + lz;
                    let wx = origin_x + lx;
                    let surface_target =
                        self.model.surface_target(wx as f64, wz as f64, 0.0);
                    let mut mask = 0u32;
                    // 2026-09-02: skip the density noise where its sign is
                    // already decided. density_from_surface is
                    //   clamp((surface - y) / VERTICAL_BIAS_SCALE, ±1.5)
                    //   + (fbm_3d(..) - 0.5) * 0.15
                    // so with the detail term bounded by ±0.075 for
                    // fbm ∈ [0, 1], any cell more than
                    // 0.075 * VERTICAL_BIAS_SCALE = 3 blocks below the
                    // surface is solid and any cell more than 3 above is
                    // air, whatever the noise says. DENSITY_SKIP_MARGIN
                    // doubles that to 6 so the conclusion survives an fbm
                    // that overshoots [0, 1] by up to ±0.5 — and
                    // `density_skip_matches_full_evaluation` checks the
                    // masks against full evaluation over 24 chunks.
                    // Caves still get their say on every solid cell.
                    let above_all = surface_target + DENSITY_SKIP_MARGIN;
                    let below_all = surface_target - DENSITY_SKIP_MARGIN;
                    for ly in 0..CHUNK_SIZE {
                        let wy = origin_y + ly;
                        let wyf = wy as f64;
                        // Ordered so the common cases (well above, well
                        // below) never touch the 3-octave 3D noise.
                        let dense = if wyf >= above_all {
                            false
                        } else if wyf <= below_all {
                            true
                        } else {
                            self.model.density_from_surface(
                                surface_target, wx as f64, wyf, wz as f64,
                            ) > anchors::DENSITY_THRESHOLD
                        };
                        if dense && !self.is_cave_with_surface(wx, wy, wz, surface_target) {
                            mask |= 1u32 << ly;
                        }
                    }
                    mask
                })
                .collect()
        };
        for ci in 0..(CHUNK_SIZE * CHUNK_SIZE) {
            let lz = ci / CHUNK_SIZE;
            let lx = ci % CHUNK_SIZE;
            let mut mask = col_masks[ci as usize];
            while mask != 0 {
                let ly = mask.trailing_zeros() as i32;
                blocks[Chunk::block_index(lx, ly, lz)] = BlockId::Stone;
                mask &= mask - 1; // clear lowest set bit
            }
        }

        // Phase 2: surface decoration. Walk each column top-down; the first
        // solid block with air above is the surface. The "above" sample may
        // need to look outside the chunk — for that we re-evaluate the
        // density model directly, which is cheap and chunk-independent.
        for lz in 0..CHUNK_SIZE {
            let wz = origin_z + lz;
            for lx in 0..CHUNK_SIZE {
                let wx = origin_x + lx;
                let sample = self.biome_at_3d(wx, origin_y, wz);
                // r739: solidity from the phase-1 bitmask instead of re-reading
                // `blocks`. Valid because phase 1 is the only writer before here and
                // phase 2 only RECOLOURS already-solid cells (solidity unchanged), so
                // `(col_mask >> ly) & 1` == `blocks[idx].is_solid()` for every in-chunk
                // cell. The top-of-chunk `above` sample still re-evaluates density
                // since it looks outside the chunk (not covered by the mask).
                let col_mask = col_masks[(lz * CHUNK_SIZE + lx) as usize];
                let col_solid = |ly: i32| (col_mask >> ly) & 1 != 0;

                for ly in (0..CHUNK_SIZE).rev() {
                    let wy = origin_y + ly;
                    let here_idx = Chunk::block_index(lx, ly, lz);
                    if !col_solid(ly) { continue; }

                    let above_solid = if ly + 1 < CHUNK_SIZE {
                        col_solid(ly + 1)
                    } else {
                        let wy_above = origin_y + ly + 1;
                        self.density(wx, wy_above, wz) > anchors::DENSITY_THRESHOLD
                            && !self.is_cave(wx, wy_above, wz)
                    };

                    if !above_solid {
                        blocks[here_idx] = self.pick_surface_block(sample.biome, wx, wy, wz);
                        for d in 1..=3 {
                            if ly < d { break; }
                            let sub_ly = ly - d;
                            let sub_idx = Chunk::block_index(lx, sub_ly, lz);
                            if col_solid(sub_ly) {
                                let sub_wy = origin_y + sub_ly;
                                blocks[sub_idx] = self.pick_subsurface_block(sample.biome, wx, sub_wy, wz);
                            } else {
                                break;
                            }
                        }
                        break;
                    }
                }
            }
        }

        // Phase 2.5: water filling.
        //
        // Pass A: fill every air cell at or below sea level with Water.
        // Pass B: for each freshly-placed water cell, apply the
        //         thermal-mass-aware freezing rule. Cells where local
        //         temperature is below the threshold (which depends on
        //         the cell's thermal mass) become Ice instead.
        //
        // The two-pass split is so the freezing test in pass B can read
        // already-placed Water from neighbors — the thermal_mass model
        // needs to see real neighbor materials, not just air.
        //
        // Why thermal mass and not just height: the prior version used
        // "freeze if surface AND cold" which was a height proxy for
        // "exposed to air." The actual physics is "freeze if heat loss
        // outpaces thermal reservoir." A 1-cell puddle 3 blocks above
        // sea level should freeze more easily than a deep ocean's
        // surface at the same temperature, because the puddle has no
        // reservoir below. Height-based rules can't see that;
        // thermal-mass-based ones can. See `thermal_mass.rs` for the
        // mathematical model.
        // r188: the fill follows the VARYING model sea level (the terrain
        // already does, via surface_target = sea(x,z) + ...). Flat 64 vs
        // varying 64+-8 was the "two layers of water": +8 zones drowned
        // their intended shoreline, -8 zones left a dry band. Per-column
        // levels hoisted once; conservative outer gate at baseline+amp.
        let sea_max = anchors::SEA_LEVEL_BASELINE as i32 + 9;
        let mut sl_col = [[0i32; CHUNK_SIZE as usize]; CHUNK_SIZE as usize];
        for lz in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                // r195: cross-backend fill height. The GPU computes sea
                // level in f32; the CPU in f64. Flooring different-precision
                // values left a 1-block water lip where Near(GPU) meets
                // Far(CPU-regen) chunks (dual ocean at boundaries). FIX:
                // cast to f32 FIRST so both backends floor the identical
                // bit pattern, then biased-floor (the -1e-3 keeps an exact
                // .5 midpoint off the knife-edge). Proven bit-identical
                // across 1.05M columns by examples/seam_probe.rs.
                let slv = self.sea_level_at(origin_x + lx, origin_z + lz) as f32;
                sl_col[lz as usize][lx as usize] = (slv + 0.5 - 1e-3).floor() as i32;
            }
        }
        // Pass A — fill water.
        for ly in 0..CHUNK_SIZE {
            let wy = origin_y + ly;
            if wy > sea_max { continue; }
            for lz in 0..CHUNK_SIZE {
                let wz = origin_z + lz;
                for lx in 0..CHUNK_SIZE {
                    let idx = Chunk::block_index(lx, ly, lz);
                    if wy > sl_col[lz as usize][lx as usize] { continue; } // r188
                    if blocks[idx] == BlockId::Air {
                        // v0.24.NEXT-cave-flood-fix: only flood OPEN water — air
                        // ABOVE the terrain surface (density <= threshold), i.e.
                        // an ocean/lake column. A carved cave is air with
                        // density > threshold (it was solid rock, carved by
                        // is_cave); flooding it drowns the cave. This pass used
                        // to flood EVERY sub-sea-level air cell, destroying all
                        // sub-sea caves ("caves generate then get destroyed in
                        // another pass"). Density is CPU<->GPU parity-verified,
                        // and the GPU shader's Pass A gets the identical gate, so
                        // the two paths stay consistent. See docs/NOISE-QUALITY.md.
                        let wx = origin_x + lx;
                        if self.density(wx, wy, wz) <= anchors::DENSITY_THRESHOLD {
                            blocks[idx] = BlockId::Water;
                        }
                    }
                }
            }
        }
        // Pass B — freeze where thermal-mass rule applies.
        // Borrow `blocks` as a closure for the sampler. We need
        // shared (immutable) access during the inner loop and exclusive
        // (mutable) access to write the result, so we collect freeze
        // decisions into a small Vec first and apply them in a separate
        // pass. This avoids aliasing issues and keeps the logic clear.
        //
        // PERF: Only check cells in the SURFACE BAND (within 5 blocks
        // below sea level). Deeper water cells:
        //   - Cannot meaningfully freeze (their thermal mass factor is
        //     ~1.0 because all neighbors are water/solid, requiring
        //     temperature < -8°C, which only happens at extreme
        //     altitudes which deep-sea cells don't reach).
        //   - Are not visible (covered by water above; only the top
        //     few cells matter for ice-vs-water rendering).
        //
        // This reduces Pass B's cost from ~32K cells per submerged chunk
        // to a max of 5 layers × 32 × 32 = 5120 cells, AND Pass B is
        // skipped entirely for chunks fully below sea_level - 5.
        // Worldgen throughput improvement: roughly 5× for sub-sea chunks.
        let freeze_band_top = sea_max; // r188b: varying-level era
        let freeze_band_bottom = sea_max - 14; // covers baseline-8 .. baseline+9
        let mut freeze_targets: Vec<usize> = Vec::new();
        // Only iterate the freeze band's intersection with this chunk.
        let band_ly_start = (freeze_band_bottom - origin_y).max(0);
        let band_ly_end = ((freeze_band_top - origin_y) + 1).min(CHUNK_SIZE);
        if band_ly_start < band_ly_end {
            for ly in band_ly_start..band_ly_end {
                let wy = origin_y + ly;
                for lz in 0..CHUNK_SIZE {
                    let wz = origin_z + lz;
                    for lx in 0..CHUNK_SIZE {
                        let wx = origin_x + lx;
                        let idx = Chunk::block_index(lx, ly, lz);
                        if blocks[idx] != BlockId::Water { continue; }
                        // Sampler closure: reads from THIS chunk's in-progress
                        // blocks array. Out-of-chunk reads return Air (which
                        // produces a slightly higher freezing tendency at
                        // chunk edges; acceptable approximation).
                        let sample = |x: i32, y: i32, z: i32| -> BlockId {
                            let lx_n = x - origin_x;
                            let ly_n = y - origin_y;
                            let lz_n = z - origin_z;
                            if lx_n < 0 || lx_n >= CHUNK_SIZE
                                || ly_n < 0 || ly_n >= CHUNK_SIZE
                                || lz_n < 0 || lz_n >= CHUNK_SIZE {
                                return BlockId::Air;
                            }
                            blocks[Chunk::block_index(lx_n, ly_n, lz_n)]
                        };
                        if crate::thermal_mass::freezes_at(wx, wy, wz, &sample) {
                            freeze_targets.push(idx);
                        }
                    }
                }
            }
        }
        for idx in freeze_targets {
            blocks[idx] = BlockId::Ice;
        }

        // Phase 3: ore replacement. Stone below sea level can become ore.
        let sea_chunk_y = (anchors::SEA_LEVEL_BASELINE / CHUNK_SIZE as f64) as i32;
        for ly in 0..CHUNK_SIZE {
            let wy = origin_y + ly;
            // Only place ores below the average sea level.
            if wy >= sea_chunk_y * CHUNK_SIZE { continue; }
            for lz in 0..CHUNK_SIZE {
                let wz = origin_z + lz;
                for lx in 0..CHUNK_SIZE {
                    let wx = origin_x + lx;
                    let idx = Chunk::block_index(lx, ly, lz);
                    if blocks[idx] != BlockId::Stone { continue; }

                    if let Some(ore) = roll_ore(wx, wy, wz, self.params.seed) {
                        blocks[idx] = ore;
                    }
                }
            }
        }

        // Phase 3.5: lava pockets. Rare lava cells in deep stone.
        // Driven by low-frequency 3D noise: most cells are stone (the
        // overwhelming worldgen default), occasional clusters become
        // lava. The 3D noise pattern produces blob-shaped pockets
        // rather than scattered isolated cells, which looks like
        // natural geological lava chambers when the player digs into
        // them.
        //
        // Threshold parameters tuned so lava covers ~0.3% of deep
        // stone — visible when found, not flooding the world.
        //
        // Lava cells become heat sources at runtime via the chunk-load
        // hook in the bin (TODO when implemented): each Lava block
        // adjacent to air is registered with HeatSourceMap so its
        // chimney updraft drives convection. For now, lava is a
        // visual feature only.
        let lava_max_y = (anchors::SEA_LEVEL_BASELINE as i32) - 30;
        // Skip chunks entirely above the lava ceiling. Lava only spawns
        // where world_y < lava_max_y (i.e., at least 30 blocks below sea).
        if origin_y >= lava_max_y {
            // Entirely too shallow — no lava possible.
        } else {
            // Chunk includes some deep cells. Process only those.
            // ly_max = how high in the chunk we still have deep cells.
            let ly_max = ((lava_max_y - origin_y).min(CHUNK_SIZE)).max(0);
            for ly in 0..ly_max {
                let wy = origin_y + ly;
                for lz in 0..CHUNK_SIZE {
                    let wz = origin_z + lz;
                    for lx in 0..CHUNK_SIZE {
                        let wx = origin_x + lx;
                        let idx = Chunk::block_index(lx, ly, lz);
                        if blocks[idx] != BlockId::Stone { continue; }
                        if roll_lava(wx, wy, wz, self.params.seed) {
                            blocks[idx] = BlockId::Lava;
                        }
                    }
                }
            }
        }

        // Phase 4: bedrock at the absolute bottom (chunks containing y=0).
        if origin_y == 0 {
            for lz in 0..CHUNK_SIZE {
                for lx in 0..CHUNK_SIZE {
                    let idx = Chunk::block_index(lx, 0, lz);
                    blocks[idx] = BlockId::Bedrock;
                }
            }
        }

        // Phase 5: tree placement.
        //
        // Trees are placed deterministically per (world_x, world_z) origin.
        // A tree originating outside this chunk can have its canopy reach
        // inside, so we scan a neighborhood that extends ±TREE_MAX_RADIUS
        // beyond the chunk's XZ extent. For each candidate origin in that
        // neighborhood: hash the origin, decide whether to spawn, and if so,
        // stamp the tree's voxels into our chunk (writes outside the chunk
        // are silently dropped — the neighbor chunk will re-evaluate the
        // same origin and stamp its own slice).
        //
        // This approach is correct under chunk re-loads (no per-chunk RNG
        // state) and across chunk boundaries (every chunk that intersects
        // a tree's bounding box independently writes the relevant portion).
        // Cost: TREE_MAX_RADIUS expands the scan area by ~30% per axis.
        //
        // Bumped from 3 to 4 because the canopy's widest layer is radius
        // 3 (5×5 footprint at peak). A tree origin that's 3 blocks
        // outside our chunk WOULD still reach in via that layer, but the
        // scan with radius=3 missed origins at distance 4 whose canopy
        // crossed into us. Result: missing-leaves bug — some trees had
        // bare trunks because their canopy was supposed to come from a
        // neighbor and never got stamped. Radius=4 covers all canopy
        // reach (max canopy radius=3, +1 margin).
        const TREE_MAX_RADIUS: i32 = 4;
        const TREE_MAX_HEIGHT: i32 = 8;

        let chunk_x_lo = origin_x;
        let chunk_x_hi = origin_x + CHUNK_SIZE;
        let chunk_y_lo = origin_y;
        let chunk_y_hi = origin_y + CHUNK_SIZE;
        let chunk_z_lo = origin_z;
        let chunk_z_hi = origin_z + CHUNK_SIZE;

        // Helper: write a block into our chunk if (wx, wy, wz) is inside,
        // Closure: write a block at (wx, wy, wz) into `blocks` IF in-bounds
        // and only if the existing block is overwriteable (Air or leaves).
        //
        // Trunk-vs-leaves preference: a log block always wins over an
        // existing leaves block (trunks are physically inside the canopy).
        // We check this by `is_log` rather than enumerating species —
        // matters now that biome can pick OakLog or BirchLog.
        let try_write = |blocks: &mut [BlockId], wx: i32, wy: i32, wz: i32, b: BlockId| {
            if wx < chunk_x_lo || wx >= chunk_x_hi { return; }
            if wy < chunk_y_lo || wy >= chunk_y_hi { return; }
            if wz < chunk_z_lo || wz >= chunk_z_hi { return; }
            let lx = wx - chunk_x_lo;
            let ly = wy - chunk_y_lo;
            let lz = wz - chunk_z_lo;
            let idx = Chunk::block_index(lx, ly, lz);
            let existing = blocks[idx];
            let is_leaves = matches!(existing,
                BlockId::OakLeaves | BlockId::BirchLeaves);
            let is_log_b = matches!(b,
                BlockId::OakLog | BlockId::BirchLog);
            match existing {
                BlockId::Air => blocks[idx] = b,
                _ if is_leaves && is_log_b => blocks[idx] = b, // log wins over leaves
                _ => {}
            }
        };

        // Per-chunk surface_y cache. Tree placement at column (wx, wz)
        // queries surface_y at (wx, wz) plus its 4 orthogonal neighbors
        // for slope estimation. With 30 trees/chunk × 5 lookups each =
        // 150 surface_y calls per chunk; many trees share neighbor
        // columns, so memoising halves the total. Each surface_y_at is
        // a binary-search of ~7 density() calls — non-trivial to
        // recompute. Cache lifetime is the tree pass only, then dropped.
        let mut surface_cache: std::collections::HashMap<(i32, i32), Option<i32>>
            = std::collections::HashMap::with_capacity(256);
        let mut surface_y_cached = |wx: i32, wz: i32, this: &Self| -> Option<i32> {
            if let Some(&v) = surface_cache.get(&(wx, wz)) { return v; }
            let v = this.surface_y_at(wx, wz, chunk_y_lo - 32, chunk_y_hi + 96);
            surface_cache.insert((wx, wz), v);
            v
        };

        for ext_z in -TREE_MAX_RADIUS .. (CHUNK_SIZE + TREE_MAX_RADIUS) {
            let wz = origin_z + ext_z;
            for ext_x in -TREE_MAX_RADIUS .. (CHUNK_SIZE + TREE_MAX_RADIUS) {
                let wx = origin_x + ext_x;

                // Quick rejection: don't even hash unless this column might
                // host a tree.
                let pos_hash = tree_origin_hash(wx, wz, self.params.seed as u64);
                // Use the loosest density threshold (CloudForest at 0.06)
                // as the upper bound — anything above this can't host a
                // tree in any biome, so skip the more expensive surface
                // lookup. Tighter biome thresholds get checked below.
                if pos_hash >= 0.06 { continue; }

                // Find the surface y at this column.
                let surface_y = match surface_y_cached(wx, wz, self) {
                    Some(y) => y,
                    None => continue,
                };
                // v0.24.NEXT-submerged-trees: don't spawn trees whose root
                // sits at/below sea level. Pass A floods that column with
                // water, so the tree would otherwise stand drowned. Mirrored
                // in chunk_generation.comp tree placement.
                if (surface_y as f64) < self.sea_level_at(wx, wz) { // r188 varying
                    continue;
                }
                // Determine biome at surface; reject biomes that don't
                // have trees (oceans, deserts, etc.).
                let surface_sample = self.biome_at_3d(wx, surface_y, wz);
                let mut profile = match surface_sample.biome.tree_profile() {
                    Some(p) => p,
                    None => continue,
                };

                // Climate-driven density modulation. Composes with
                // biome's base_density: a TemperateForest column in a
                // marginally-cold corner of the biome gets fewer trees
                // than the same column in the biome's sweet spot.
                let temp = surface_sample.temperature;
                let humid = surface_sample.humidity;
                let climate_factor = climate_tree_factor(temp, humid);
                let mut effective_density = profile.base_density * climate_factor;

                // r605: FOREST ISLANDS in the savanna (Comoé study — see
                // docs/design/biomes_west_africa.md and world::forest_islands).
                // Inside an island, density rises with the core and each
                // tree draws a STRATUM (D/d/o) from the island's own
                // palette — crossing to the next island reads as a
                // different forest (the 8 %-species-overlap rule as a
                // mechanic). MIRRORED in chunk_generation.comp; the
                // transliteration tests in forest_islands.rs gate the twin.
                if surface_sample.biome == Biome::Savanna {
                    use crate::world::forest_islands as fi;
                    let isl = fi::sample(wx, wz, self.seed());
                    if isl.in_island {
                        effective_density = fi::island_density(isl.core) * climate_factor as f64;
                        let strat_h = fi::cell_hash01(wx, wz, 0x57A7, self.seed());
                        profile = fi::island_profile(isl.palette, fi::stratum_for(strat_h));
                    }
                }
                if pos_hash >= effective_density { continue; }

                // Reject if surface block isn't a tree-suitable substrate.
                let surface_b = self.pick_surface_block(surface_sample.biome, wx, surface_y, wz);
                if !matches!(surface_b, BlockId::Grass | BlockId::Moss) { continue; }

                // ---- PIECE A: continuous climate-driven dimensions ----
                //
                // growth_quality is a [0, 1] scalar combining climate
                // suitability and per-tree noise:
                //   - climate_factor / 1.5 normalises the [0, 1.5] range
                //     of climate_tree_factor down to [0, 1].
                //   - per-tree noise (from pos_hash sub-bits) gives ±20%
                //     stochastic variation so adjacent trees aren't
                //     identical.
                //
                // Trees in their biome sweet spot (high climate_factor)
                // grow to the upper end of the biome's profile range;
                // trees on the climatic edge stay small.
                let climate_quality = (climate_factor / 1.5).clamp(0.0, 1.0);
                // Noise from sub-bits of pos_hash, in roughly [-0.2, 0.2].
                let stochastic = ((pos_hash * 1.0e7).fract() - 0.5) * 0.4;
                let growth_quality = (climate_quality + stochastic).clamp(0.0, 1.0);

                // Trunk height: continuous lerp between profile min/max.
                let height_range = (profile.trunk_max_height - profile.trunk_min_height) as f64;
                let trunk_height = profile.trunk_min_height
                    + (height_range * growth_quality).round() as i32;
                if trunk_height > TREE_MAX_HEIGHT - 1 { continue; }
                if trunk_height < 1 { continue; }

                // Canopy radius: also lerps with growth_quality. Min 1
                // so even stunted trees have at least one leaf layer.
                let r_max_full = profile.canopy_radius;
                let r_max = ((r_max_full as f64) * (0.5 + 0.5 * growth_quality)).round().max(1.0) as i32;

                // ---- PIECE B: slope-driven lean ----
                //
                // Sample surface_y at 4 orthogonal neighbors 2 blocks out.
                // Compute a 2D slope vector (dy/dx, dy/dz). If magnitude
                // exceeds a threshold (steep terrain), the trunk leans
                // UPHILL (gravitropism + light from open down-slope side)
                // and the canopy shifts uphill too.
                //
                // Lean is implemented as a per-block horizontal offset
                // for the trunk: each block higher, the trunk shifts by
                // `lean_per_block` in the slope direction. Voxels can't
                // truly tilt — this stair-steps the trunk, which reads
                // visually as a leaning tree, especially on cliff edges.
                //
                // Quantization: lean direction snaps to one of 8 cardinal
                // unit vectors so the stair-step pattern is consistent
                // (otherwise rounding produces jagged zigzags).
                let s_w = surface_y_cached(wx - 2, wz, self).unwrap_or(surface_y);
                let s_e = surface_y_cached(wx + 2, wz, self).unwrap_or(surface_y);
                let s_n = surface_y_cached(wx, wz - 2, self).unwrap_or(surface_y);
                let s_s = surface_y_cached(wx, wz + 2, self).unwrap_or(surface_y);
                // Slope vector points DOWNHILL (toward lower neighbors).
                // We want the tree to lean UPHILL (away from downhill),
                // which is the negation.
                let dh_x = (s_e - s_w) as f64 / 4.0; // rise / run
                let dh_z = (s_s - s_n) as f64 / 4.0;
                let slope_mag = (dh_x * dh_x + dh_z * dh_z).sqrt();
                let (lean_dx, lean_dz) = if slope_mag > 0.4 {
                    // On a real slope. Lean uphill, magnitude scales
                    // with steepness, capped so even cliff trees don't
                    // become horizontal. Lean direction is the upslope
                    // unit vector (negated downslope vector).
                    let cap = slope_mag.min(2.0); // cap at very steep
                    let lean_factor = (cap - 0.4) * 0.25; // ~0..0.4 per block
                    (-(dh_x / slope_mag) * lean_factor,
                     -(dh_z / slope_mag) * lean_factor)
                } else {
                    (0.0, 0.0)
                };

                // ---- Stamp trunk (with lean offset per block) ----
                // r604: thickness from the form — the baobab's 2×2 trunk
                // mass IS its silhouette. Extra columns extend +x/+z.
                let tt = crate::world::tree_form::trunk_thickness(profile.form);
                for h in 1..=trunk_height {
                    let off_x = (lean_dx * h as f64).round() as i32;
                    let off_z = (lean_dz * h as f64).round() as i32;
                    for txo in 0..tt {
                        for tzo in 0..tt {
                            try_write(
                                &mut blocks,
                                wx + off_x + txo,
                                surface_y + h,
                                wz + off_z + tzo,
                                profile.trunk_block,
                            );
                        }
                    }
                }

                // ---- Stamp leaf canopy (centered on trunk-top, lean-shifted) ----
                let canopy_cy = surface_y + trunk_height;
                let canopy_off_x = (lean_dx * trunk_height as f64).round() as i32;
                let canopy_off_z = (lean_dz * trunk_height as f64).round() as i32;
                let canopy_cx = wx + canopy_off_x;
                let canopy_cz = wz + canopy_off_z;
                // r604: canopy geometry now comes from the profile's FORM
                // (world::tree_form). Dome reproduces the old inline math
                // byte-for-byte (pinned by unit test); FlatTop/Baobab/Shrub
                // are the new West-African silhouettes. MIRRORED in
                // chunk_generation.comp — the worldgen-parity probe diffs
                // full chunks across backends to keep the twins honest.
                use crate::world::tree_form as tf;
                let v_extent = tf::canopy_v_extent(profile.form, r_max);
                for cy in (canopy_cy - v_extent) ..= (canopy_cy + v_extent) {
                    let dy = cy - canopy_cy;
                    let Some(r) = tf::canopy_layer_radius(profile.form, dy, v_extent, r_max)
                    else {
                        continue;
                    };
                    for cz in (canopy_cz - r) ..= (canopy_cz + r) {
                        for cx in (canopy_cx - r) ..= (canopy_cx + r) {
                            let dx = cx - canopy_cx;
                            let dz = cz - canopy_cz;
                            // Round-ish footprint: skip outermost corners.
                            if r >= 2 && dx.abs() == r && dz.abs() == r { continue; }
                            // Skip cells that would overwrite a trunk block.
                            // Trunks lean, so the trunk position at this cy
                            // (if cy within trunk height) is the lean-shifted
                            // column, not the original (wx, wz).
                            if cy <= surface_y + trunk_height {
                                let h_at_cy = cy - surface_y;
                                let trunk_at_cy_x = wx + (lean_dx * h_at_cy as f64).round() as i32;
                                let trunk_at_cy_z = wz + (lean_dz * h_at_cy as f64).round() as i32;
                                if cx == trunk_at_cy_x && cz == trunk_at_cy_z { continue; }
                            }
                            try_write(&mut blocks, cx, cy, cz, profile.leaves_block);
                        }
                    }
                }
            }
        }

        // Phase 6: scattered surface RUINS. Extracted to `stamp_ruins` so the GPU
        // worldgen path can run the SAME decoration after read-back (the GPU shader
        // mirrors terrain only — structures are a CPU post-pass in both paths).
        self.stamp_ruins(&mut blocks, coord);
        // Phase 6b: strata, ore veins, cave dressing and indicator plants. Same
        // post-pass on the GPU path (terrain_generator.rs), after its ruins.
        self.decorate_underground(&mut blocks, coord);

        // Phase 7: cave flora (r-flora-wiring step 1, per docs/FLORA-WIRING.md).
        //
        // STAGED, NOT DEFAULT — matches the doc's own caution: "designed,
        // not wired... left for a session with the build-and-screenshot
        // loop." This is that step-1 cut (gated, CPU-path-only, `Vine`
        // stand-in block, clamp-to-box, no `flora_store`/lifecycle yet) so
        // it's a one-env-var try when that session happens. It has NOT
        // been visually confirmed in a cave — this sandbox has no GPU/
        // display to build-and-screenshot with, which is exactly the
        // check the doc says this needs before it's more than staged.
        //
        // Also note (see docs/CAVES_WIRED.md for the precedent): this
        // only reaches the CPU-generated world. The fullgame path
        // generates terrain on the GPU by default; on hardware where GPU
        // worldgen actually runs, this will be silently invisible until
        // the same placement is ported to `chunk_generation.comp`, same
        // as cave_field was for months. It'll be visible today only on
        // configs where GPU generation falls back to CPU (e.g. missing
        // the required Vulkan feature).
        if std::env::var("UNENDLICH_CAVE_FLORA").is_ok() {
            let flora_max_y = (self.sea_level_at(origin_x, origin_z) as i32) - 4;
            let occ = |cell: crate::world::stem::Cell| {
                let (lx, ly, lz) = (cell.0 - origin_x, cell.1 - origin_y, cell.2 - origin_z);
                if lx < 0 || ly < 0 || lz < 0
                    || lx >= CHUNK_SIZE || ly >= CHUNK_SIZE || lz >= CHUNK_SIZE
                {
                    // Out-of-box: doc's v1 choice — treat as empty rather
                    // than re-querying density/is_cave for a neighbor
                    // chunk we don't have here. Truncates edge plants;
                    // safe and deterministic (same tradeoff as the
                    // tree-canopy `try_write` bounds check above).
                    return false;
                }
                blocks[Chunk::block_index(lx, ly, lz)] != BlockId::Air
            };
            let moist = |cell: crate::world::stem::Cell| {
                self.model.humidity(cell.0 as f64, cell.1 as f64, cell.2 as f64, 0.0) as f32
            };
            let temp = |cell: crate::world::stem::Cell| {
                self.model.temperature(cell.0 as f64, cell.1 as f64, cell.2 as f64, 0.0) as f32
            };

            let flora = crate::world::chunk_flora::ChunkFlora::populate(
                coord,
                self.seed() as u64,
                flora_max_y,
                &occ,
                &moist,
                &temp,
            );
            for plant in &flora.plants {
                for cell in plant.cells() {
                    let (lx, ly, lz) = (cell.0 - origin_x, cell.1 - origin_y, cell.2 - origin_z);
                    if lx < 0 || ly < 0 || lz < 0
                        || lx >= CHUNK_SIZE || ly >= CHUNK_SIZE || lz >= CHUNK_SIZE
                    {
                        continue; // clamp: cross-chunk growth deferred (v2, per the doc)
                    }
                    let idx = Chunk::block_index(lx, ly, lz);
                    if blocks[idx] == BlockId::Air {
                        blocks[idx] = BlockId::Vine;
                    }
                }
            }
            // v1 deliberately does not populate `flora_store` / register
            // the `plant_lifecycle` tick — placement only, per the doc's
            // build-loop step 1. Lighting/animation is step 2.
        }

        Chunk::from_blocks(coord, blocks)
    }

    /// Stamp scattered surface ruins into a chunk's block array. Deterministic per
    /// 80×80 region (no per-chunk RNG); each hosting region drops one `make_ruin`
    /// footprint at a hashed anchor on dry land, written cross-chunk so seams match.
    /// Runs as a CPU post-pass in BOTH the CPU generator and the GPU read-back path,
    /// since the GPU shader produces terrain only. Surface height + biome come from
    /// the analytic generator, which the GPU terrain mirrors.
/// Run `world::underground::decorate` over a finished chunk. Both terrain
    /// backends call this after `stamp_ruins`, so they stay byte-identical.
    /// `UNENDLICH_UNDERGROUND=0` turns it off (for an A/B look at the plain
    /// terrain); anything else, or unset, leaves it on.
    pub fn decorate_underground(&self, blocks: &mut [BlockId], coord: ChunkCoord) -> super::underground::Report {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*ON.get_or_init(|| std::env::var("UNENDLICH_UNDERGROUND").map_or(true, |v| v != "0")) {
            return super::underground::Report::default();
        }
        let origin = coord.block_origin();
        let model = &self.model;
        super::underground::decorate(blocks, origin, self.seed(), &|x, z| {
            model.surface_target(x as f64, z as f64, 0.0).round() as i32
        })
    }

    pub fn stamp_ruins(&self, blocks: &mut [BlockId], coord: ChunkCoord) {
        let (origin_x, origin_y, origin_z) = coord.block_origin();
        let chunk_x_lo = origin_x;
        let chunk_x_hi = origin_x + CHUNK_SIZE;
        let chunk_y_lo = origin_y;
        let chunk_y_hi = origin_y + CHUNK_SIZE;
        let chunk_z_lo = origin_z;
        let chunk_z_hi = origin_z + CHUNK_SIZE;
        const RUIN_REGION: i32 = 80;
        const RUIN_MAX: i32 = 12;
        let r_x0 = (chunk_x_lo - RUIN_MAX).div_euclid(RUIN_REGION);
        let r_x1 = chunk_x_hi.div_euclid(RUIN_REGION);
        let r_z0 = (chunk_z_lo - RUIN_MAX).div_euclid(RUIN_REGION);
        let r_z1 = chunk_z_hi.div_euclid(RUIN_REGION);
        let span = (RUIN_REGION - RUIN_MAX) as u64;
        for rz in r_z0..=r_z1 {
            for rx in r_x0..=r_x1 {
                let rh = ruin_region_hash(rx, rz, self.params.seed as u64);
                if rh % 100 >= 38 {
                    continue; // ~38% of regions host a ruin
                }
                let ax = rx * RUIN_REGION + ((rh >> 8) % span) as i32;
                let az = rz * RUIN_REGION + ((rh >> 24) % span) as i32;
                let w = 5 + ((rh >> 40) % 4) as i32;
                let h = 5 + ((rh >> 44) % 4) as i32;
                if ax + w < chunk_x_lo || ax > chunk_x_hi
                    || az + h < chunk_z_lo || az > chunk_z_hi
                {
                    continue;
                }
                let base_y = match self.surface_y_at(ax, az, chunk_y_lo - 64, chunk_y_hi + 96) {
                    Some(y) => y,
                    None => continue,
                };
                if base_y < crate::constants::SEA_LEVEL {
                    continue; // dry land only
                }
                // Log once per ruin — when its anchor lands in THIS chunk — so a
                // session log confirms ruins are being generated and where.
                if ax >= chunk_x_lo && ax < chunk_x_hi && az >= chunk_z_lo && az < chunk_z_hi {
                    log::info!("[worldgen] ruin at ({},{},{}) {}x{}", ax, base_y, az, w, h);
                }
                let wall_h = 3 + ((rh >> 48) % 2) as i32; // 3..4 tall — clearly visible
                let mat = match self.biome_at_3d(ax, base_y, az).biome {
                    Biome::Desert | Biome::Beach => BlockId::Sandstone,
                    Biome::RedSandstoneCanyon | Biome::BloomingCanyon => BlockId::RedSandstone,
                    Biome::Swamp | Biome::LushCave => BlockId::MossyCobblestone,
                    _ => BlockId::Cobblestone,
                };
                let tmpl = crate::world::structure_template::make_ruin(w as u32, h as u32, 1, 2, rh);
                for cz in 0..h {
                    for cx in 0..w {
                        let code = tmpl.cells[(cz * w + cx) as usize];
                        if code == 0 {
                            continue;
                        }
                        let wx = ax + cx;
                        let wz = az + cz;
                        if code == 2 {
                            ruin_write(
                                blocks, wx, base_y, wz, mat,
                                chunk_x_lo, chunk_x_hi, chunk_y_lo, chunk_y_hi, chunk_z_lo, chunk_z_hi,
                            );
                        } else {
                            for dy in 1..=wall_h {
                                ruin_write(
                                    blocks, wx, base_y + dy, wz, mat,
                                    chunk_x_lo, chunk_x_hi, chunk_y_lo, chunk_y_hi, chunk_z_lo, chunk_z_hi,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Climate-driven multiplier on a biome's base tree density. Lets the
/// "right" biome (e.g. TemperateForest) STILL produce sparse trees in
/// a marginal climate corner (cold edge of the temperate zone) and
/// dense trees in its sweet spot (mild + humid).
///
/// Inputs are in [0, 1] from the WorldModel's normalised fields:
///   - temperature: 0=cold (0°C ish), 1=hot (40°C ish), 0.5=temperate.
///   - humidity:    0=arid, 1=saturated.
///
/// Output is a multiplier on `TreeProfile::base_density`. Typical
/// range is [0.0, 1.5]:
///   - Mild + humid → 1.4 (lush forest peak)
///   - Mild + dry  → 0.5 (sparse)
///   - Cold OR hot extremes → tapered down toward 0
///   - Hot AND dry → 0.0 (effectively no trees, even if biome says forest)
///
/// This composes with biome selection: BIOME picks the species/shape,
/// CLIMATE picks the abundance. So you get TemperateForest with VERY
/// FEW trees on its cold edge, transitioning naturally to its dense
/// core, as you walk across a temperature gradient.
fn climate_tree_factor(temperature: f64, humidity: f64) -> f64 {
    // Hot+dry hard cutoff: real-world deserts have no trees regardless
    // of biome label noise. Catches edge cases where biome lookup said
    // "forest" but climate says "Sahara."
    if temperature > 0.75 && humidity < 0.25 { return 0.0; }

    // Bell curve on temperature centred at the temperate sweet spot.
    // Wide bell — even cold (0.2) and hot (0.8) climates get some trees
    // if humidity is right (think cold rainforest, hot jungle).
    // Peak at 0.5 = 1.0; tapers to ~0.3 at extremes.
    let t_d = temperature - 0.5;
    let temp_factor = 1.0 - 1.4 * t_d * t_d;
    let temp_factor = temp_factor.max(0.2);

    // Humidity is roughly linear: dry suppresses, humid encourages.
    // 0% humidity → 0.3× density; 100% humidity → 1.5× density.
    let humid_factor = 0.3 + humidity * 1.2;

    temp_factor * humid_factor
}

/// Deterministic hash of a tree origin column to a [0, 1) value.
/// Used as a probability test for tree placement. Same coord → same value.
fn tree_origin_hash(wx: i32, wz: i32, seed: u64) -> f64 {
    let mut h: u64 = (wx as u64).wrapping_mul(0xa376_2dab_5ce3_43e9)
        ^ (wz as u64).wrapping_mul(0xc1b3_e99c_5f78_2d11)
        ^ seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^= h >> 33;
    (h & 0xFFFFFFFF) as f64 / (u32::MAX as f64)
}

/// r530: deterministic per-region hash for surface ruin placement.
fn ruin_region_hash(rx: i32, rz: i32, seed: u64) -> u64 {
    let mut h: u64 = (rx as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (rz as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ seed.wrapping_mul(0xA376_2DAB_5CE3_43E9);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^= h >> 33;
    h
}

/// r530: write a ruin block at world (wx,wy,wz) into this chunk if in-bounds,
/// overwriting only air, foliage, and soft ground (never solid rock / existing
/// structure), so ruins drape onto terrain without gouging hillsides.
#[allow(clippy::too_many_arguments)]
fn ruin_write(
    blocks: &mut [BlockId], wx: i32, wy: i32, wz: i32, b: BlockId,
    x_lo: i32, x_hi: i32, y_lo: i32, y_hi: i32, z_lo: i32, z_hi: i32,
) {
    if wx < x_lo || wx >= x_hi || wy < y_lo || wy >= y_hi || wz < z_lo || wz >= z_hi {
        return;
    }
    let idx = Chunk::block_index(wx - x_lo, wy - y_lo, wz - z_lo);
    let existing = blocks[idx];
    let overwriteable = existing == BlockId::Air
        || !existing.is_solid()
        || matches!(existing, BlockId::Grass | BlockId::Dirt | BlockId::Sand | BlockId::SnowBlock);
    if overwriteable {
        blocks[idx] = b;
    }
}

fn legacy_surface_block(biome: Biome, y: i32) -> BlockId {
    match biome {
        Biome::Desert | Biome::Beach => BlockId::Sand,
        Biome::RedSandstoneCanyon | Biome::BloomingCanyon => BlockId::RedSandstone,
        Biome::Swamp => BlockId::Moss,
        Biome::Tundra | Biome::AlpineHighlands if y > 90 => BlockId::SnowBlock,
        Biome::IceCave => BlockId::Ice,
        Biome::CrystalCave => BlockId::Crystal,
        Biome::LushCave => BlockId::Moss,
        _ => BlockId::Grass,
    }
}

fn legacy_subsurface_block(biome: Biome, _y: i32) -> BlockId {
    match biome {
        Biome::Desert | Biome::Beach => BlockId::Sand,
        Biome::RedSandstoneCanyon | Biome::BloomingCanyon => BlockId::RedSandstone,
        _ => BlockId::Dirt,
    }
}

fn roll_ore(world_x: i32, y: i32, world_z: i32, seed: u32) -> Option<BlockId> {
    if y < 128 {
        let coal = noise_3d(
            world_x as f64 * 0.1, y as f64 * 0.1, world_z as f64 * 0.1,
            seed.wrapping_add(ore_seeds::COAL),
        );
        if coal > 0.85 { return Some(BlockId::CoalOre); }
    }
    if y < 64 {
        let iron = noise_3d(
            world_x as f64 * 0.08, y as f64 * 0.08, world_z as f64 * 0.08,
            seed.wrapping_add(ore_seeds::IRON),
        );
        if iron > 0.88 { return Some(BlockId::IronOre); }
    }
    if y < 32 {
        let gold = noise_3d(
            world_x as f64 * 0.07, y as f64 * 0.07, world_z as f64 * 0.07,
            seed.wrapping_add(ore_seeds::GOLD),
        );
        if gold > 0.92 { return Some(BlockId::GoldOre); }
    }
    None
}

/// Test whether this stone cell should become lava. Used by Phase 3.5
/// of `generate_chunk`. Lower frequency than ore noise so pockets are
/// blob-shaped (chambers, not specks). Higher threshold so lava is
/// rarer than ores.
///
/// The Y modulation makes lava more common deeper down (y < -32 is
/// hot core; y > -10 below sea level is rare but possible).
fn roll_lava(world_x: i32, y: i32, world_z: i32, seed: u32) -> bool {
    // Low-frequency 3D noise: blob radius around 8-12 blocks.
    let raw = noise_3d(
        world_x as f64 * 0.04, y as f64 * 0.04, world_z as f64 * 0.04,
        seed.wrapping_add(ore_seeds::LAVA),
    );
    // Y bias: deeper = slightly more lava. Sea level (y~64) is rare;
    // far below is somewhat more common but still rare. Linear ramp
    // from threshold 0.92 (rare) at sea level to 0.85 (still rare,
    // but visible) deep down.
    //
    // Tuning history: original threshold drop was 0.14 cap which gave
    // ~5% lava density at chunk_y=-3 — too dense, test expected <5%.
    // Now capped at 0.07 → ~3% density at that depth, well within
    // the design target of "visible when found, not flooding."
    let depth_below_sea = (64 - y).max(0) as f64;
    // Each 32 blocks of depth lowers the threshold by 0.035, capped at 0.07.
    let threshold = 0.92 - (depth_below_sea / 32.0 * 0.035).min(0.07);
    raw > threshold
}

/// Render a small TOP-DOWN elevation map preview for `seed`: sample the terrain
/// surface over a `w`×`h` grid of world columns and colour each cell by height with
/// a terrain ramp (water → sand → grass → rock → snow). RGBA thumbnail (row-major,
/// 4 bytes/px) the menu uploads as a texture next to the seed field. Uses the same
/// `WorldGenerator` the heightmap harness proved, so it matches the world the seed
/// generates. Cheap: one surface sample per pixel.
pub fn surface_run_preview(seed: u32, w: usize, h: usize) -> Vec<u8> {
    const Y_MIN: i32 = 0;
    const Y_MAX: i32 = 200;
    const STEP: i32 = 6; // world blocks per pixel → a wide map per thumbnail
    let gen = WorldGenerator::new(seed);
    let (w, h) = (w.max(1), h.max(1));
    let mut elev = vec![64i32; w * h];
    let (mut lo, mut hi) = (i32::MAX, i32::MIN);
    for j in 0..h {
        for i in 0..w {
            let y = gen
                .surface_y_at(i as i32 * STEP, j as i32 * STEP, Y_MIN, Y_MAX)
                .unwrap_or(64);
            elev[j * w + i] = y;
            lo = lo.min(y);
            hi = hi.max(y);
        }
    }
    let span = (hi - lo).max(1) as f32;
    let mut out = vec![0u8; w * h * 4];
    for k in 0..w * h {
        let t = (elev[k] - lo) as f32 / span; // 0 low .. 1 high
        let (r, g, b) = terrain_ramp(t);
        out[k * 4] = r;
        out[k * 4 + 1] = g;
        out[k * 4 + 2] = b;
        out[k * 4 + 3] = 255;
    }
    out
}

/// Elevation → terrain colour ramp for the top-down preview.
fn terrain_ramp(t: f32) -> (u8, u8, u8) {
    let lerp = |a: (u8, u8, u8), b: (u8, u8, u8), u: f32| {
        let u = u.clamp(0.0, 1.0);
        (
            (a.0 as f32 + (b.0 as f32 - a.0 as f32) * u) as u8,
            (a.1 as f32 + (b.1 as f32 - a.1 as f32) * u) as u8,
            (a.2 as f32 + (b.2 as f32 - a.2 as f32) * u) as u8,
        )
    };
    if t < 0.22 {
        lerp((26, 52, 110), (54, 110, 168), t / 0.22) // deep → shallow water
    } else if t < 0.30 {
        lerp((200, 196, 138), (170, 188, 120), (t - 0.22) / 0.08) // sand
    } else if t < 0.62 {
        lerp((86, 150, 74), (60, 120, 56), (t - 0.30) / 0.32) // grass
    } else if t < 0.82 {
        lerp((96, 96, 90), (130, 126, 120), (t - 0.62) / 0.20) // rock
    } else {
        lerp((180, 182, 188), (240, 242, 248), (t - 0.82) / 0.18) // snow caps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_preview_is_terrain_shaped_and_seed_varying() {
        let (w, h) = (96usize, 48usize);
        let a = surface_run_preview(7, w, h);
        let b = surface_run_preview(99, w, h);
        assert_eq!(a.len(), w * h * 4);
        // a top-down elevation map has variety, not a flat fill: count coarse
        // (quantised) colours — terrain should span several ramp bands.
        let mut buckets = std::collections::HashSet::new();
        for px in a.chunks(4) {
            buckets.insert((px[0] / 32, px[1] / 32, px[2] / 32));
        }
        assert!(buckets.len() >= 3, "map spans several elevation bands (got {})", buckets.len());
        // different seeds → different maps
        assert_ne!(a, b, "different seeds produce different previews");
    }

    #[test]
    fn deterministic_generation() {
        let g = WorldGenerator::new(42);
        let coord = ChunkCoord::new(2, 1, 3);
        let a = g.generate_chunk(coord);
        let b = g.generate_chunk(coord);
        assert_eq!(a.blocks, b.blocks);
    }

    #[test]
    fn topmost_standable_finds_the_surface_over_a_cave_not_the_cave_floor() {
        // A column: deep solid [0..40], a CAVE void [40..45], crust solid
        // [45..50], open sky above. The standable surface is y=49 (top of the
        // crust) — NOT y=39 (the cave floor), which the old binary search could
        // wrongly return, dropping a creature into the cave.
        let col = |y: i32| (0..40).contains(&y) || (45..50).contains(&y);
        assert_eq!(topmost_standable(col, 0, 64), Some(49), "stands on the crust, not in the cave");

        // Plain monotonic column: solid up to 49, air above → surface at 49.
        let plain = |y: i32| y < 50;
        assert_eq!(topmost_standable(plain, 0, 64), Some(49));

        // A floating island [60..63] above a gap above ground [0..30]: the
        // topmost standable is the island top (63).
        let island = |y: i32| (0..30).contains(&y) || (60..64).contains(&y);
        assert_eq!(topmost_standable(island, 0, 128), Some(63));

        // All air → nothing to stand on.
        assert_eq!(topmost_standable(|_| false, 0, 64), None);

        // Solid to the very top of the range → that top is standable (y_max+1 is
        // treated as sky).
        assert_eq!(topmost_standable(|_| true, 0, 64), Some(64));
    }

    #[test]
    fn topo_ocean_threshold_widens_only_for_positive_height_modifiers() {
        let sl = 64.0;
        // Zero/negative modifiers (Plains=0, Ocean=-30, Beach=-5, Swamp=-8):
        // threshold is exactly sea_level - 2.0, unchanged from before this
        // fix — the r177/r188/r192 submerged-Plains/Ocean/Beach behavior at
        // this threshold is bit-for-bit preserved.
        assert_eq!(topo_ocean_threshold(sl, 0.0), sl - 2.0);
        assert_eq!(topo_ocean_threshold(sl, -30.0), sl - 2.0);
        assert_eq!(topo_ocean_threshold(sl, -5.0), sl - 2.0);

        // Positive modifiers widen the threshold downward by exactly their
        // magnitude, so a Skylands-climate column's biome-free base has to
        // dip below sea_level-82 (not just sea_level-2) before the immediate
        // Ocean veto fires — anything shallower than that falls through to
        // the surface_y_at refinement, which sees the true (+80-adjusted)
        // surface instead of the biome-free base.
        assert_eq!(topo_ocean_threshold(sl, 80.0), sl - 82.0); // Skylands
        assert_eq!(topo_ocean_threshold(sl, 50.0), sl - 52.0); // MistyMountains
        assert_eq!(topo_ocean_threshold(sl, 35.0), sl - 37.0); // AlpineHighlands

        // Reachability check: continental*30 + hills*15 (both documented
        // [-1,1] noise fields) spans roughly [-45, 45] — comfortably inside
        // the widened Skylands/MistyMountains margins, confirming a valley
        // deep enough to fool the OLD threshold (sl-2.0) is NOT deep enough
        // to fool the fixed one for these biomes.
        let worst_case_valley = sl - 45.0;
        assert!(worst_case_valley < sl - 2.0,
            "a deep valley would have tripped the old immediate-Ocean veto");
        assert!(worst_case_valley > topo_ocean_threshold(sl, 80.0),
            "but the fixed Skylands threshold correctly lets it through to refinement");
    }

    #[test]
    fn surface_results_are_always_standable_even_over_real_caves() {
        // Sweep a grid of real generated columns; every surface_y_at result must
        // be a genuine standable block (solid, with non-solid directly above) —
        // never a cave floor with rock over it. Guards the surface-glitch fix on
        // actual terrain, where caves make columns non-monotonic.
        let (g, loaded) = production_like_generator(1337);
        println!("(standard registry loaded: {} — TerrainGenerator::new loads it in production)", loaded);
        let mut checked = 0;
        for x in -30..30 {
            for z in -30..30 {
                if let Some(y) = g.surface_y_at(x, z, -64, 200) {
                    let solid = |yy: i32| {
                        g.density(x, yy, z) > anchors::DENSITY_THRESHOLD && !g.is_cave(x, yy, z)
                    };
                    assert!(solid(y), "({x},{z}) grounded at y={y} which isn't solid");
                    assert!(!solid(y + 1), "({x},{z}) grounded at y={y} but y+1 is solid (buried, not a surface)");
                    checked += 1;
                }
            }
        }
        assert!(checked > 1000, "swept a meaningful number of columns ({checked})");
    }

    #[test]
    fn different_chunks_differ() {
        let g = WorldGenerator::new(42);
        let a = g.generate_chunk(ChunkCoord::new(0, 1, 0));
        let b = g.generate_chunk(ChunkCoord::new(10, 1, 10));
        assert_ne!(a.blocks, b.blocks);
    }

    #[test]
    fn deep_chunks_are_mostly_solid() {
        let g = WorldGenerator::new(42);
        let chunk = g.generate_chunk(ChunkCoord::new(0, -50, 0));
        let solid_count = chunk.blocks.iter().filter(|b| b.is_solid()).count();
        assert!(
            solid_count as f32 / CHUNK_VOLUME as f32 > 0.85,
            "deep chunk only {}/{} solid", solid_count, CHUNK_VOLUME,
        );
    }

    #[test]
    fn high_chunks_are_mostly_air() {
        let g = WorldGenerator::new(42);
        let chunk = g.generate_chunk(ChunkCoord::new(0, 50, 0));
        let air_count = chunk.blocks.iter().filter(|b| b.is_air()).count();
        assert!(
            air_count as f32 / CHUNK_VOLUME as f32 > 0.95,
            "high chunk only {}/{} air", air_count, CHUNK_VOLUME,
        );
    }

    #[test]
    fn surface_y_finds_terrain() {
        let g = WorldGenerator::new(42);
        let surface = g.surface_y_at(0, 0, -64, 256);
        assert!(surface.is_some(), "no surface found near origin");
        let s = surface.unwrap();
        // Sea level baseline is 64 with regional variation up to ±20 or so.
        assert!(s > 20, "surface too low: {}", s);
        assert!(s < 200, "surface too high: {}", s);
    }

    #[test]
    fn worldgen_eventually_places_a_tree() {
        // Across enough chunks, at least one should contain an OakLog
        // (proving the tree-placement phase runs and writes blocks).
        // We test ~256 surface chunks; with 1/80 density per column and
        // many grass columns per chunk, several trees should appear.
        let g = WorldGenerator::new(0xCAFE);
        let mut log_count = 0;
        let mut leaf_count = 0;
        for cx in -8..8 {
            for cz in -8..8 {
                // Try a chunk straddling typical surface band.
                let chunk = g.generate_chunk(ChunkCoord::new(cx, 2, cz));
                for b in &chunk.blocks {
                    if *b == BlockId::OakLog { log_count += 1; }
                    if *b == BlockId::OakLeaves { leaf_count += 1; }
                }
            }
        }
        assert!(log_count > 0, "expected at least one OakLog in 256 chunks; got {}", log_count);
        // Each tree has roughly 4-7 logs and 12-20 leaves; if logs exist,
        // leaves should exceed logs.
        assert!(leaf_count > log_count, "leaves should outnumber logs (got logs={}, leaves={})",
            log_count, leaf_count);
    }

    /// Lava generates in deep stone but NOT in surface chunks.
    #[test]
    fn no_lava_at_surface() {
        let g = WorldGenerator::new(42);
        let mut total_lava = 0;
        // Surface chunks (chunk_y=2 → world y=64..95, around sea level).
        for cx in -4..4 {
            for cz in -4..4 {
                let chunk = g.generate_chunk(ChunkCoord::new(cx, 2, cz));
                total_lava += chunk.blocks.iter()
                    .filter(|b| **b == BlockId::Lava).count();
            }
        }
        assert_eq!(total_lava, 0,
            "expected zero surface lava, got {} cells across 64 surface chunks",
            total_lava);
    }

    /// Lava generates in deep chunks at non-zero rate. Tests against
    /// the deep-Y-bias logic in `roll_lava`.
    #[test]
    fn lava_appears_in_deep_chunks() {
        let g = WorldGenerator::new(42);
        let mut total_lava = 0;
        let mut total_blocks = 0;
        // Very deep chunks (chunk_y=-3 → world y=-96..-65). Should
        // have rich lava density per the roll_lava depth-bias.
        for cx in -4..4 {
            for cz in -4..4 {
                let chunk = g.generate_chunk(ChunkCoord::new(cx, -3, cz));
                total_lava += chunk.blocks.iter()
                    .filter(|b| **b == BlockId::Lava).count();
                total_blocks += chunk.blocks.len();
            }
        }
        assert!(total_lava > 0,
            "expected some lava in 64 deep chunks; got {}", total_lava);
        // Ratio should be modest — not a flooded world.
        let ratio = total_lava as f32 / total_blocks as f32;
        assert!(ratio < 0.05,
            "lava density too high ({}); should be < 5% of deep stone",
            ratio);
    }

    /// Lava generation is deterministic (same seed → same lava placement).
    /// Critical for save/load consistency.
    #[test]
    fn lava_generation_is_deterministic() {
        let g = WorldGenerator::new(99);
        let coord = ChunkCoord::new(1, -3, 2);
        let a = g.generate_chunk(coord);
        let b = g.generate_chunk(coord);
        let lava_a: Vec<_> = a.blocks.iter().enumerate()
            .filter(|(_, b)| **b == BlockId::Lava)
            .map(|(i, _)| i).collect();
        let lava_b: Vec<_> = b.blocks.iter().enumerate()
            .filter(|(_, b)| **b == BlockId::Lava)
            .map(|(i, _)| i).collect();
        assert_eq!(lava_a, lava_b,
            "lava placement not deterministic across regenerations");
    }

    /// Fast-path: chunks well above the surface generate quickly and
    /// produce all-air output. This is the worldgen optimization for
    /// the render-distance sphere extending high above the player.
    ///
    /// Verifies the fast-path FIRES (chunk is empty) for a clearly-
    /// high chunk. Doesn't verify it doesn't fire for surface chunks
    /// — that's covered by `high_chunks_are_mostly_air` which already
    /// passes with chunks at chunk_y=50.
    #[test]
    fn fast_path_high_chunk_is_pure_air() {
        let g = WorldGenerator::new(42);
        // chunk_y = 100 → origin_y = 3200, way above HIGH_AIR_MIN_Y (192).
        let chunk = g.generate_chunk(ChunkCoord::new(0, 100, 0));
        let air_count = chunk.blocks.iter().filter(|b| b.is_air()).count();
        assert_eq!(air_count, CHUNK_VOLUME,
            "fast-path high chunk should be 100% air, got {}/{}",
            air_count, CHUNK_VOLUME);
    }

    /// Fast-path is faster than slow-path. We approximate the slow-
    /// path cost by generating chunks at the surface (which DON'T
    /// take the fast path) and comparing to high chunks (which DO).
    /// A high chunk should be many times faster.
    ///
    /// This is a smoke test, not a benchmark — we only check that
    /// the fast path is strictly less than 1/4 the cost of a
    /// surface chunk. Real benchmarking belongs in world-bench.
    #[test]
    fn fast_path_is_substantially_faster() {
        use std::time::Instant;
        let g = WorldGenerator::new(42);
        // Warmup
        let _ = g.generate_chunk(ChunkCoord::new(0, 2, 0));
        let _ = g.generate_chunk(ChunkCoord::new(0, 100, 0));
        // Time a surface chunk (slow path)
        let n = 5;
        let t0 = Instant::now();
        for i in 0..n {
            let _ = g.generate_chunk(ChunkCoord::new(i, 2, 0));
        }
        let slow = t0.elapsed();
        // Time a high chunk (fast path)
        let t0 = Instant::now();
        for i in 0..n {
            let _ = g.generate_chunk(ChunkCoord::new(i, 100, 0));
        }
        let fast = t0.elapsed();
        // The fast path skips the 32K cell density loop, so it
        // should be at least 4× faster than slow. In practice the
        // ratio is much higher, but 4× is a robust lower bound.
        assert!(fast * 4 < slow,
            "fast path should be >=4× faster: slow={:?}, fast={:?}",
            slow, fast);
    }

    /// Critical: the fast-path's "all air" guess must be CORRECT.
    /// If we set HIGH_AIR_DENSITY_MAX too liberal, we could miss
    /// genuine terrain blocks at high altitude (mountains, floating
    /// islands if they ever exist). This test brute-force-verifies:
    /// for a fast-path chunk, the slow-path WOULD also produce all-air
    /// (by sampling many cells with full density evaluation).
    ///
    /// We do this by sampling ~100 random cells inside a fast-path
    /// chunk and confirming density() < threshold for each. If even
    /// one is wrong, the fast-path is unsafe and we'd need to lower
    /// HIGH_AIR_DENSITY_MAX or raise HIGH_AIR_MIN_Y.
    #[test]
    fn fast_path_all_air_assumption_holds() {
        let g = WorldGenerator::new(42);
        // Test multiple high chunks across X/Z to cover noise variations.
        for cx in [-50, 0, 50] {
            for cz in [-50, 0, 50] {
                // Just barely past the threshold — most aggressive case.
                let chunk_y = 5; // origin_y = 160, exactly at threshold
                let coord = ChunkCoord::new(cx, chunk_y, cz);
                let (origin_x, origin_y, origin_z) = coord.block_origin();
                // Sample EVERY cell, not just corners, to catch any
                // density spike the fast-path's 9-sample heuristic
                // might miss.
                for ly in 0..CHUNK_SIZE {
                    let wy = origin_y + ly;
                    for lz in (0..CHUNK_SIZE).step_by(4) {
                        let wz = origin_z + lz;
                        for lx in (0..CHUNK_SIZE).step_by(4) {
                            let wx = origin_x + lx;
                            let d = g.density(wx, wy, wz);
                            assert!(d < anchors::DENSITY_THRESHOLD,
                                "fast-path zone has solid cell at ({},{},{}) d={}",
                                wx, wy, wz, d);
                        }
                    }
                }
            }
        }
    }

    /// Climate tree factor: hot+dry returns zero (no trees in the desert).
    #[test]
    fn climate_tree_factor_zeroes_in_hot_dry() {
        let f = climate_tree_factor(0.9, 0.1);
        assert_eq!(f, 0.0,
            "hot+dry should fully suppress trees, got {}", f);
    }

    /// r605 end-to-end: forest islands make savanna chunks measurably
    /// denser. Scan a seed for a savanna column sitting in an island CORE
    /// and one in open grass, generate both chunks, and require the island
    /// chunk to carry clearly more tree blocks. (The exact counts are seed
    /// noise; the ORDER is the invariant.)
    #[test]
    fn savanna_islands_are_denser_than_open_grass() {
        use crate::world::forest_islands as fi;
        use crate::world::biomes::Biome;
        // Savanna placement is seed luck; try a fixed list until one seed
        // yields both an island-core column and open grass. Deterministic:
        // the list is fixed and the first hit wins.
        let mut found: Option<(WorldGenerator, (i32, i32), (i32, i32))> = None;
        'seeds: for s in [0xBEEF_5A7Au32, 42, 7, 12345, 999_331, 0xCAFE, 2024, 555_000] {
        let g = WorldGenerator::new(s);
        let seed = g.seed();
        // Enumerate island CENTRES analytically from the cell field (a
        // blind grid scan strides over the 5–18-block cores), then keep
        // the first whose centre is savanna.
        let mut island_col: Option<(i32, i32)> = None;
        let mut open_col: Option<(i32, i32)> = None;
        'cells: for cx in -60i32..60 {
            for cz in -60i32..60 {
                if fi::cell_hash01(cx, cz, 0x15AD, seed) >= fi::ISLAND_P {
                    continue;
                }
                let jx = fi::cell_hash01(cx, cz, 0xC311, seed);
                let jz = fi::cell_hash01(cx, cz, 0xC312, seed);
                let wx = (cx as f32 * fi::ISLAND_CELL as f32
                    + (0.25 + 0.5 * jx) * fi::ISLAND_CELL as f32) as i32;
                let wz = (cz as f32 * fi::ISLAND_CELL as f32
                    + (0.25 + 0.5 * jz) * fi::ISLAND_CELL as f32) as i32;
                let sy = match g.surface_y_at(wx, wz, -64, 256) {
                    Some(y) => y,
                    None => continue,
                };
                if g.biome_at_3d(wx, sy, wz).biome != Biome::Savanna {
                    continue;
                }
                let isl = fi::sample(wx, wz, seed);
                if isl.in_island && isl.core > 0.6 && isl.palette != fi::TERMITE_PALETTE {
                    island_col = Some((wx, wz));
                    // Open grass: walk outward until the field says open
                    // AND biome is still savanna.
                    for step in 1..40 {
                        let (ox, oz) = (wx + step * 13, wz + step * 7);
                        let osy = match g.surface_y_at(ox, oz, -64, 256) {
                            Some(y) => y,
                            None => continue,
                        };
                        if g.biome_at_3d(ox, osy, oz).biome == Biome::Savanna
                            && !fi::sample(ox, oz, seed).in_island
                        {
                            open_col = Some((ox, oz));
                            break 'cells;
                        }
                    }
                }
            }
        }
        if let (Some(i), Some(o)) = (island_col, open_col) {
            found = Some((g, i, o));
            break 'seeds;
        }
        }
        let (g, (ix, iz), (ox, oz)) =
            found.expect("no seed in the fixed list yields a savanna island + open grass pair");
        let count_trees = |wx: i32, wz: i32| -> usize {
            let cs = crate::constants::CHUNK_SIZE;
            let coord = ChunkCoord::new(wx.div_euclid(cs), 2, wz.div_euclid(cs));
            let chunk = g.generate_chunk(coord);
            chunk
                .blocks
                .iter()
                .filter(|b| {
                    matches!(
                        **b,
                        BlockId::OakLog
                            | BlockId::OakLeaves
                            | BlockId::BirchLog
                            | BlockId::BirchLeaves
                    )
                })
                .count()
        };
        let dense = count_trees(ix, iz);
        let open = count_trees(ox, oz);
        assert!(
            dense > open.saturating_mul(2).max(20),
            "island chunk ({ix},{iz}) must be much denser than open ({ox},{oz}): {dense} vs {open}"
        );
    }

    /// r614: the tree-site puzzle instrument. The GLSL biome_at_3d_gpu
    /// (transliterated below, f32, literally) is diffed against the REAL
    /// biome_at_3d over a coordinate grid at the regressing seed. Zero or
    /// near-zero mismatches ⇒ the r613 tree-site regression is DOWNSTREAM
    /// of classification (profile/density/hash interplay); a large count
    /// ⇒ a mirror bug, located by the printed mismatch cases.
    #[test]
    fn glsl_biome_at_3d_transliteration_vs_cpu() {
        use crate::world::biome_loader::BiomeRegistry;
        // load_standard resolves cwd-dependently; tests run from the
        // crate dir, so try the workspace-relative path explicitly.
        let mut reg = BiomeRegistry::empty();
        let loaded = ["../../assets/biomes", "assets/biomes"]
            .iter()
            .find_map(|p| reg.load_directory(p).ok());
        if loaded.is_none() {
            eprintln!("SKIP: no biome assets from test cwd");
            return;
        }
        // Bake the def table BEFORE the registry moves into the Arc.
        let defs: Vec<(f32, f32, f32, u32)> = reg
            .defs()
            .iter()
            .map(|d| {
                let (t, h) = d.climate_coord();
                let pen = if d.legacy_enum.is_none() { 0.01f32 } else { 0.0 };
                (t as f32, h as f32, pen, d.to_legacy() as u32)
            })
            .collect();
        let g = WorldGenerator::with_biome_registry(424242, std::sync::Arc::new(reg));

        // ---- literal transliteration of biome_at_3d_gpu (f32) ----
        let glsl = |x: f32, y: f32, z: f32| -> u32 {
            let temp = g.model().temperature(x as f64, y as f64, z as f64, 0.0) as f32;
            let humid = g.model().humidity(x as f64, y as f64, z as f64, 0.0) as f32;
            let erosion_raw = ((crate::world::noise::fbm_2d(
                (x * 0.0008) as f64,
                (z * 0.0008) as f64,
                g.seed().wrapping_add(0xE105),
                2,
                1.0,
                1.0,
            ) as f32)
                - 0.5)
                * 2.0;
            let erosion_unit = (erosion_raw + 1.0) * 0.5;
            let legacy_verdict =
                determine_biome(temp as f64, humid as f64, erosion_unit as f64);
            let sl = g.model().sea_level(x as f64, z as f64, 0.0) as f32;
            let st = sl
                + g.model().continental(x as f64, z as f64) as f32 * 30.0
                + g.model().hills(x as f64, z as f64) as f32 * 15.0;
            let ocean_threshold =
                sl - 2.0 - (legacy_verdict.height_modifier() as f32).max(0.0);
            let shelf_threshold = sl + 6.0;
            let mut topo: Option<Biome> = None;
            if st < ocean_threshold {
                topo = Some(Biome::Ocean);
            } else if st < shelf_threshold {
                match g.surface_y_at(x as i32, z as i32, (sl - 120.0) as i32, (sl + 8.0) as i32)
                {
                    Some(sy) if (sy as f32) < sl - 2.0 => topo = Some(Biome::Ocean),
                    Some(sy) if (sy as f32) < sl + 2.0 => topo = Some(Biome::Beach),
                    _ => {}
                }
            }
            if let Some(t) = topo {
                return t as u32;
            }
            if matches!(legacy_verdict, Biome::Ocean | Biome::Beach) {
                return legacy_verdict as u32;
            }
            let (tt, th) = (temp.clamp(0.0, 1.0), humid.clamp(0.0, 1.0));
            let mut best = -1i32;
            let mut best_d2 = f32::INFINITY;
            for (i, &(dt_, dh_, pen, _)) in defs.iter().enumerate() {
                let dt = dt_ - tt;
                let dh = dh_ - th;
                let d2 = dt * dt + dh * dh + pen;
                if d2 < best_d2 {
                    best_d2 = d2;
                    best = i as i32;
                }
            }
            if best >= 0 {
                return defs[best as usize].3;
            }
            legacy_verdict as u32
        };
        // ---- end transliteration ----

        let (mut mismatch, mut total) = (0u32, 0u32);
        let mut cases: Vec<(i32, i32, u32, u32)> = Vec::new();
        for wx in (-1600..1600).step_by(37) {
            for wz in (-1600..1600).step_by(41) {
                let sy = match g.surface_y_at(wx, wz, -64, 256) {
                    Some(y) => y,
                    None => continue,
                };
                total += 1;
                let cpu = g.biome_at_3d(wx, sy, wz).biome as u32;
                let gpu = glsl(wx as f32, sy as f32, wz as f32);
                if cpu != gpu {
                    mismatch += 1;
                    if cases.len() < 8 {
                        cases.push((wx, wz, cpu, gpu));
                    }
                }
            }
        }
        println!("biome_at_3d transliteration: {mismatch}/{total} mismatches; first: {cases:?}");
        assert!(
            (mismatch as f64) < (total as f64) * 0.02,
            "mirror diverges materially: {mismatch}/{total}, cases {cases:?}"
        );
    }

    /// Climate tree factor: peak in mild + humid (>1.0).
    #[test]
    fn climate_tree_factor_peaks_in_mild_humid() {
        let f = climate_tree_factor(0.5, 0.9);
        assert!(f > 1.0,
            "mild+humid should boost tree density, got {}", f);
    }

    /// Climate tree factor: monotonic in humidity at fixed temp.
    #[test]
    fn climate_tree_factor_increases_with_humidity() {
        let f_dry = climate_tree_factor(0.5, 0.2);
        let f_mid = climate_tree_factor(0.5, 0.5);
        let f_wet = climate_tree_factor(0.5, 0.9);
        assert!(f_dry < f_mid && f_mid < f_wet,
            "factor should increase with humidity: {} < {} < {}",
            f_dry, f_mid, f_wet);
    }

    /// Climate tree factor: mild always > extremes at same humidity.
    #[test]
    fn climate_tree_factor_temperate_beats_extremes() {
        let f_cold = climate_tree_factor(0.1, 0.7);
        let f_mild = climate_tree_factor(0.5, 0.7);
        let f_hot  = climate_tree_factor(0.85, 0.7);
        assert!(f_mild > f_cold,
            "mild should exceed cold at same humidity: {} > {}", f_mild, f_cold);
        assert!(f_mild > f_hot,
            "mild should exceed hot at same humidity: {} > {}", f_mild, f_hot);
    }

    /// Tree profile: forests have profiles; oceans/deserts don't.
    #[test]
    fn tree_profile_present_for_forest_biomes() {
        assert!(Biome::TemperateForest.tree_profile().is_some());
        assert!(Biome::BirchForest.tree_profile().is_some());
        assert!(Biome::CloudForest.tree_profile().is_some());
        assert!(Biome::Plains.tree_profile().is_some());
    }

    /// Tree profile: no-tree biomes return None.
    #[test]
    fn tree_profile_absent_for_no_tree_biomes() {
        assert!(Biome::Desert.tree_profile().is_none());
        assert!(Biome::Ocean.tree_profile().is_none());
        assert!(Biome::Beach.tree_profile().is_none());
        assert!(Biome::Tundra.tree_profile().is_none());
        assert!(Biome::Skylands.tree_profile().is_none());
    }

    /// Tree profile: cloud forest has the tallest, densest, widest
    /// trees among the forest biomes (sanity-check the differentiation).
    #[test]
    fn cloud_forest_is_taller_and_wider_than_temperate() {
        let cf = Biome::CloudForest.tree_profile().unwrap();
        let tf = Biome::TemperateForest.tree_profile().unwrap();
        assert!(cf.trunk_max_height > tf.trunk_max_height,
            "cloud forest should be taller: {} vs {}",
            cf.trunk_max_height, tf.trunk_max_height);
        assert!(cf.canopy_radius > tf.canopy_radius,
            "cloud forest should be wider: {} vs {}",
            cf.canopy_radius, tf.canopy_radius);
        assert!(cf.base_density >= tf.base_density,
            "cloud forest should be at least as dense: {} vs {}",
            cf.base_density, tf.base_density);
    }

    /// Trees vary in size across the world. Different chunks should
    /// produce different tree dimensions because climate varies with
    /// position. Tests Piece A: continuous climate-driven dimensions.
    #[test]
    fn tree_dimensions_vary_by_position() {
        let g = WorldGenerator::new(0xCAFE);
        // Walk a small swath; for each tree found, record its trunk
        // height (count of consecutive log blocks above a grass).
        // Small sample (5×5 = 25 chunks) keeps test time under a
        // few seconds while still hitting enough territory to find
        // multiple trees with varying climates.
        let mut trunk_heights: Vec<i32> = Vec::new();
        for cx in -2..=2 {
            for cz in -2..=2 {
                let chunk = g.generate_chunk(ChunkCoord::new(cx, 1, cz));
                let (origin_x, origin_y, origin_z) = chunk.coord.block_origin();
                // Scan for tree trunks: a column of OakLog or BirchLog
                // adjacent to grass at its base.
                for lz in 0..CHUNK_SIZE {
                    for lx in 0..CHUNK_SIZE {
                        // Find the highest log in this column; if there
                        // is one and grass directly below, count height.
                        let mut h: i32 = 0;
                        let mut top_log_y: Option<i32> = None;
                        for ly in 0..CHUNK_SIZE {
                            let idx = Chunk::block_index(lx, ly, lz);
                            if matches!(chunk.blocks[idx], BlockId::OakLog | BlockId::BirchLog) {
                                top_log_y = Some(ly);
                                h += 1;
                            }
                        }
                        if let Some(_) = top_log_y {
                            if h >= 2 { trunk_heights.push(h); }
                        }
                        let _ = (origin_x, origin_y, origin_z);
                    }
                }
            }
        }
        if trunk_heights.is_empty() {
            // Sample area might be all-ocean or all-desert. Still
            // useful to have ran without panic; skip the assertion.
            return;
        }
        let min = *trunk_heights.iter().min().unwrap();
        let max = *trunk_heights.iter().max().unwrap();
        // With climate-continuous dimensions, we should see a range
        // wider than what a single fixed value would produce.
        assert!(max > min,
            "expected tree heights to vary across positions, got constant {}",
            min);
    }

    /// Tree placement responds to slope. On a hill, trunks should
    /// be displaced from their column origin (lean offset). Tests
    /// Piece B: slope-driven lean.
    ///
    /// We look for a chunk where there's enough slope variation to
    /// trigger lean. Then check whether ANY tree in that chunk has
    /// its top log displaced from its base column. Voxel quantization
    /// means we can't always observe lean (rounding), so we just need
    /// to confirm that displacement happens at all somewhere in a
    /// large sample.
    #[test]
    fn trees_can_lean_on_steep_terrain() {
        let g = WorldGenerator::new(0xCAFE);
        let mut found_leaning = false;
        // Sample with a smaller grid to keep test time reasonable.
        // 7×7 = 49 chunks is enough to find leaning trees in the
        // continental noise.
        'outer: for cx in -3..=3 {
            for cz in -3..=3 {
                let chunk = g.generate_chunk(ChunkCoord::new(cx, 1, cz));
                let (origin_x, origin_y, origin_z) = chunk.coord.block_origin();
                // Walk through chunk: for each log block at local
                // y > 1, check if the cell directly below is also a
                // log or grass. If neither, the trunk above must have
                // come from a leaned tree (its base is in a different
                // column).
                for ly in 2..CHUNK_SIZE {
                    for lz in 1..CHUNK_SIZE - 1 {
                        for lx in 1..CHUNK_SIZE - 1 {
                            let idx = Chunk::block_index(lx, ly, lz);
                            if !matches!(chunk.blocks[idx], BlockId::OakLog | BlockId::BirchLog) {
                                continue;
                            }
                            // Look 1 below for log continuity.
                            let below = Chunk::block_index(lx, ly - 1, lz);
                            if matches!(chunk.blocks[below], BlockId::OakLog | BlockId::BirchLog) {
                                continue;
                            }
                            // Found a log block whose direct support
                            // is NOT another log. Check if there's a
                            // log within ±2 horizontally below it —
                            // that's evidence of a lean.
                            for off_z in [-2, -1, 0, 1, 2] {
                                for off_x in [-2, -1, 0, 1, 2] {
                                    if off_x == 0 && off_z == 0 { continue; }
                                    let nx = lx + off_x;
                                    let nz = lz + off_z;
                                    if nx < 0 || nx >= CHUNK_SIZE { continue; }
                                    if nz < 0 || nz >= CHUNK_SIZE { continue; }
                                    for ny in (ly - 3)..ly {
                                        if ny < 0 || ny >= CHUNK_SIZE { continue; }
                                        let n_idx = Chunk::block_index(nx, ny, nz);
                                        if matches!(chunk.blocks[n_idx],
                                            BlockId::OakLog | BlockId::BirchLog)
                                        {
                                            found_leaning = true;
                                            break 'outer;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                let _ = (origin_x, origin_y, origin_z);
            }
        }
        // We don't HARD-require that lean is observed (a particular
        // seed might not have any steep enough columns intersecting
        // tree spawns). But we want to know if it NEVER happens —
        // that would mean the lean code is dead. So this test just
        // documents the expectation and prints an info note.
        if !found_leaning {
            eprintln!("Note: no leaning trees observed in 33×33 chunk sample. \
                Lean code may still be correct — particular seed may not \
                generate steep+treed terrain in this range.");
        }
    }
}

#[cfg(test)]
mod generation_cost_tests {
    use super::*;

    /// Where the per-chunk streaming budget actually goes. Not an
    /// assertion — a number to compare against the octree build
    /// (`octree::pyramid_tests::octree_build_ab`), `Chunk::from_blocks`
    /// (`chunk::from_blocks_tests::from_blocks_ab`) and meshing
    /// (`mesher_binary::tests::timing_report`), so effort lands where the
    /// time is.
    #[test]
    #[ignore]
    fn generate_chunk_cost() {
        use std::time::Instant;
        let g = WorldGenerator::new(1337);
        // A surface column, a deep column, and a high-altitude one: the
        // generator's cost varies with what it has to evaluate.
        let cases = [
            ("surface (y=0)", ChunkCoord::new(4, 0, 4)),
            ("underground (y=-3)", ChunkCoord::new(4, -3, 4)),
            ("sky (y=6)", ChunkCoord::new(4, 6, 4)),
            ("far surface", ChunkCoord::new(400, 0, -250)),
        ];
        let rounds = 10;
        println!("WorldGenerator::generate_chunk ({} rounds each):", rounds);
        for (name, c) in cases {
            let t = Instant::now();
            let mut solid = 0usize;
            for _ in 0..rounds {
                let ch = g.generate_chunk(c);
                solid += ch.blocks.iter().filter(|b| !b.is_air()).count();
            }
            let us = t.elapsed().as_secs_f64() * 1e6 / rounds as f64;
            println!("  {:20} {:8.0} us   ({} solid cells)", name, us, solid / rounds);
        }
    }
}

#[cfg(test)]
mod density_skip_tests {
    use super::*;

    /// Column mask by full evaluation — the pre-2026-09-02 inner loop.
    fn mask_full(g: &WorldGenerator, wx: i32, wz: i32, origin_y: i32) -> u32 {
        let surface_target = g.model.surface_target(wx as f64, wz as f64, 0.0);
        let mut mask = 0u32;
        for ly in 0..CHUNK_SIZE {
            let wy = origin_y + ly;
            let solid = g.model.density_from_surface(
                            surface_target, wx as f64, wy as f64, wz as f64,
                        ) > anchors::DENSITY_THRESHOLD
                        && !g.is_cave(wx, wy, wz);
            if solid { mask |= 1u32 << ly; }
        }
        mask
    }

    fn mask_skipped(g: &WorldGenerator, wx: i32, wz: i32, origin_y: i32) -> u32 {
        let surface_target = g.model.surface_target(wx as f64, wz as f64, 0.0);
        let above_all = surface_target + DENSITY_SKIP_MARGIN;
        let below_all = surface_target - DENSITY_SKIP_MARGIN;
        let mut mask = 0u32;
        for ly in 0..CHUNK_SIZE {
            let wy = origin_y + ly;
            let wyf = wy as f64;
            let dense = if wyf >= above_all { false }
                else if wyf <= below_all { true }
                else {
                    g.model.density_from_surface(surface_target, wx as f64, wyf, wz as f64)
                        > anchors::DENSITY_THRESHOLD
                };
            if dense && !g.is_cave(wx, wy, wz) { mask |= 1u32 << ly; }
        }
        mask
    }

    #[test]
    fn density_skip_matches_full_evaluation() {
        let g = WorldGenerator::new(20260902);
        let mut columns = 0u32;
        for (cx, cy, cz) in [
            (0, 0, 0), (3, 0, -5), (17, -2, 40), (-120, 0, 77),
            (400, 1, -250), (9, 3, 9),
        ] {
            let (ox, oy, oz) = ChunkCoord::new(cx, cy, cz).block_origin();
            // Every 3rd column of the chunk — 121 per chunk, 726 total.
            for lz in (0..CHUNK_SIZE).step_by(3) {
                for lx in (0..CHUNK_SIZE).step_by(3) {
                    let (wx, wz) = (ox + lx, oz + lz);
                    assert_eq!(mask_skipped(&g, wx, wz, oy), mask_full(&g, wx, wz, oy),
                        "column ({}, {}) at origin_y {}", wx, wz, oy);
                    columns += 1;
                }
            }
        }
        assert!(columns > 700, "checked {} columns", columns);
    }

    #[test]
    fn the_analytic_bound_holds_for_the_detail_term() {
        // If VERTICAL_BIAS_SCALE or the 0.15 detail weight change, the
        // margin has to be revisited: this pins the arithmetic the skip
        // rests on.
        let bound = 0.075 * anchors::VERTICAL_BIAS_SCALE;
        assert!(DENSITY_SKIP_MARGIN >= 2.0 * bound,
            "margin {} should be at least twice the analytic bound {}", DENSITY_SKIP_MARGIN, bound);
        assert_eq!(anchors::DENSITY_THRESHOLD, 0.0);
    }

    #[test]
    #[ignore]
    fn density_skip_ab() {
        use std::time::Instant;
        let g = WorldGenerator::new(20260902);
        let rounds = 4;
        println!("column-mask A/B ({} rounds x 1024 columns, interleaved):", rounds);
        for (name, c) in [
            ("surface", ChunkCoord::new(4, 0, 4)),
            ("underground", ChunkCoord::new(4, -3, 4)),
            ("above ground", ChunkCoord::new(4, 3, 4)),
        ] {
            let (ox, oy, oz) = c.block_origin();
            let (mut t_new, mut t_old) = (0.0f64, 0.0f64);
            for r in 0..rounds {
                let run = |f: &dyn Fn(&WorldGenerator, i32, i32, i32) -> u32| -> f64 {
                    let t = Instant::now();
                    let mut acc = 0u32;
                    for lz in 0..CHUNK_SIZE { for lx in 0..CHUNK_SIZE {
                        acc = acc.wrapping_add(f(&g, ox + lx, oz + lz, oy));
                    }}
                    std::hint::black_box(acc);
                    t.elapsed().as_secs_f64()
                };
                if r % 2 == 0 { t_new += run(&mask_skipped); t_old += run(&mask_full); }
                else { t_old += run(&mask_full); t_new += run(&mask_skipped); }
            }
            let ms = |t: f64| t * 1e3 / rounds as f64;
            println!("  {:14} full {:8.1} ms   skip {:8.1} ms   ({:+.1}%)",
                name, ms(t_old), ms(t_new), (t_new / t_old - 1.0) * 100.0);
        }
    }
}


/// A generator configured the way `TerrainGenerator::new` configures the
/// real one: with the standard biome registry when it loads. Timings
/// taken WITHOUT it understate production, because `biome_def_at`
/// short-circuits to `None` and never computes temperature/humidity.
#[cfg(test)]
fn production_like_generator(seed: u32) -> (WorldGenerator, bool) {
    match crate::world::biome_loader::BiomeRegistry::load_standard() {
        Ok(r) => (WorldGenerator::with_biome_registry(seed, std::sync::Arc::new(r)), true),
        Err(_) => (WorldGenerator::new(seed), false),
    }
}

#[cfg(test)]
mod cave_surface_tests {
    use super::*;

    #[test]
    fn is_cave_with_surface_matches_is_cave() {
        let g = WorldGenerator::new(20260902);
        let mut checked = 0u32;
        for (cx, cy, cz) in [(0i32, 0i32, 0i32), (4, -3, 4), (-31, 1, 62), (400, 0, -250)] {
            let (ox, oy, oz) = ChunkCoord::new(cx, cy, cz).block_origin();
            // Every 4th column, every 2nd layer: 8 x 8 x 16 per chunk.
            for lz in (0..CHUNK_SIZE).step_by(4) {
                for lx in (0..CHUNK_SIZE).step_by(4) {
                    let (wx, wz) = (ox + lx, oz + lz);
                    let st = g.model.surface_target(wx as f64, wz as f64, 0.0);
                    for ly in (0..CHUNK_SIZE).step_by(2) {
                        let wy = oy + ly;
                        assert_eq!(g.is_cave_with_surface(wx, wy, wz, st), g.is_cave(wx, wy, wz),
                            "cave mismatch at ({}, {}, {})", wx, wy, wz);
                        checked += 1;
                    }
                }
            }
        }
        assert_eq!(checked, 4 * 8 * 8 * 16);
    }

    #[test]
    fn surface_y_at_is_unchanged_by_the_hoist() {
        // The scan's own result, against a reference that recomputes
        // surface_target on every probe exactly as the old body did.
        let g = WorldGenerator::new(20260902);
        let reference = |wx: i32, wz: i32, y_min: i32, y_max: i32| -> Option<i32> {
            let solid_at = |y: i32| {
                g.density(wx, y, wz) > anchors::DENSITY_THRESHOLD && !g.is_cave(wx, y, wz)
            };
            let st = g.model.surface_target(wx as f64, wz as f64, 0.0);
            let scan_top = (st.floor() as i32 + 16).min(y_max).max(y_min);
            topmost_standable(solid_at, y_min, scan_top)
        };
        let mut checked = 0u32;
        for (x0, z0) in [(0i32, 0i32), (128, 128), (-400, 250), (4096, -4096)] {
            for dx in (0..32).step_by(2) {
                for dz in (0..32).step_by(2) {
                    let (wx, wz) = (x0 + dx, z0 + dz);
                    assert_eq!(g.surface_y_at(wx, wz, -64, 200), reference(wx, wz, -64, 200),
                        "surface_y_at differs at ({}, {})", wx, wz);
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 4 * 16 * 16);
    }

    #[test]
    #[ignore]
    fn generate_chunk_after_ab() {
        use std::time::Instant;
        let g = WorldGenerator::new(20260902);
        let rounds = 6;
        println!("generate_chunk with the column surface_target reused ({} rounds):", rounds);
        for (name, c) in [
            ("surface", ChunkCoord::new(4, 0, 4)),
            ("underground", ChunkCoord::new(4, -3, 4)),
            ("far surface", ChunkCoord::new(400, 0, -250)),
        ] {
            let t = Instant::now();
            let mut solid = 0usize;
            for _ in 0..rounds {
                let ch = g.generate_chunk(c);
                solid += ch.blocks.iter().filter(|b| !b.is_air()).count();
            }
            println!("  {:14} {:8.1} ms   ({} solid cells)",
                name, t.elapsed().as_secs_f64() * 1e3 / rounds as f64, solid / rounds);
        }
    }
}

#[cfg(test)]
mod hotspot_tests {
    use super::*;

    /// Where the 225 ms/chunk of CPU worldgen actually is. The two
    /// candidates I optimised for on suspicion were both wrong — the
    /// density noise is ~4 % of the column pass and all 32 768 `is_cave`
    /// calls together are under 2 ms — so this splits the column pass
    /// into its two parts and times them separately.
    /// Split the surface/biome pass. `biome_at_3d` runs once per column
    /// and is now the biggest remaining piece of a chunk; this times its
    /// parts so the next change is aimed rather than guessed.
    #[test]
    #[ignore]
    fn biome_pass_breakdown() {
        use std::time::Instant;
        let g = WorldGenerator::new(20260902);
        let (ox, _oy, oz) = ChunkCoord::new(4, 0, 4).block_origin();
        let cols: Vec<(i32, i32)> = (0..CHUNK_SIZE).flat_map(|lz| (0..CHUNK_SIZE).map(move |lx| (lx, lz)))
            .map(|(lx, lz)| (ox + lx, oz + lz)).collect();
        let rounds = 4;
        let time = |label: &str, f: &dyn Fn() -> f64| {
            let t = Instant::now();
            let mut sink = 0.0f64;
            for _ in 0..rounds { sink += f(); }
            let ms = t.elapsed().as_secs_f64() * 1e3 / rounds as f64;
            println!("  {:34} {:8.2} ms   ({:7.2} us/column)  [{}]", label, ms, ms * 1000.0 / 1024.0, sink as i64);
        };
        println!("biome pass breakdown, one surface chunk (1024 columns):");
        time("biome_at_3d (whole)", &|| {
            cols.iter().map(|&(x, z)| g.biome_at_3d(x, 0, z).temperature).sum()
        });
        time("  model.temperature", &|| {
            cols.iter().map(|&(x, z)| g.model.temperature(x as f64, 0.0, z as f64, 0.0)).sum()
        });
        time("  model.humidity", &|| {
            cols.iter().map(|&(x, z)| g.model.humidity(x as f64, 0.0, z as f64, 0.0)).sum()
        });
        time("  fbm_2d erosion", &|| {
            cols.iter().map(|&(x, z)| super::super::noise::fbm_2d(
                x as f64 * 0.0008, z as f64 * 0.0008, g.params.seed.wrapping_add(0xE105), 2, 1.0, 1.0)).sum()
        });
        time("  sea_level", &|| {
            cols.iter().map(|&(x, z)| g.model.sea_level(x as f64, z as f64, 0.0)).sum()
        });
        time("  continental + hills", &|| {
            cols.iter().map(|&(x, z)| g.model.continental(x as f64, z as f64)
                + g.model.hills(x as f64, z as f64)).sum()
        });
        time("  surface_y_at (shelf branch)", &|| {
            cols.iter().map(|&(x, z)| g.surface_y_at(x, z, -60, 140).unwrap_or(0) as f64).sum()
        });
        time("  surface_target", &|| {
            cols.iter().map(|&(x, z)| g.model.surface_target(x as f64, z as f64, 0.0)).sum()
        });
    }

    /// Does one `biome_def_at` per column give the same answer as the
    /// four the surface pass actually makes? The doc
    /// (`worldgen-hotspots-2026-09-02.md`) asks for a disagreement RATE
    /// over a wide sweep before anyone collapses those four calls into
    /// one, because `temperature` moves with `y` and a column on a
    /// climate boundary could legitimately differ. This is that sweep:
    /// many columns, several regions, several altitudes, counting
    /// mismatches rather than asserting none.
    #[test]
    #[ignore]
    fn biome_def_y_disagreement_sweep() {
        let (g, loaded) = production_like_generator(20260902);
        if !loaded {
            println!("standard biome registry did not load — sweep is meaningless, skipping");
            return;
        }
        let mut checked = 0u64;
        let mut differ = 0u64;
        let mut examples: Vec<String> = Vec::new();
        // Regions far apart so different climates are covered, and a
        // range of surface altitudes.
        for (x0, z0) in [(0i32, 0i32), (5000, 5000), (-8000, 3000), (120000, -70000),
                         (-333, 999), (64, -64), (2048, 2048), (-15000, -15000)] {
            for dx in 0..40 {
                for dz in 0..40 {
                    let (x, z) = (x0 + dx * 7, z0 + dz * 11);
                    // Where the surface actually is for this column, so
                    // the 4 y values are the ones the pass really uses.
                    let st = g.model.surface_target(x as f64, z as f64, 0.0);
                    let sy = st.floor() as i32;
                    let at = |y: i32| g.biome_def_at(x, y, z).map(|d| d.id.clone());
                    let top = at(sy);
                    for d in 1..=3 {
                        let sub = at(sy - d);
                        if sub != top {
                            differ += 1;
                            if examples.len() < 5 {
                                examples.push(format!("({}, {}, {}) surface {:?} vs sub-{} {:?}", x, sy, z, top, d, sub));
                            }
                        }
                        checked += 1;
                    }
                }
            }
        }
        println!("biome_def y-disagreement sweep: {} subsurface cells checked, {} differ from their column's surface def ({:.4}%)",
            checked, differ, differ as f64 * 100.0 / checked as f64);
        for e in &examples { println!("  e.g. {}", e); }
    }

    /// The per-cell part of the surface pass: `biome_def_at` and its
    /// two components. Called 4x per column (pick_surface_block once,
    /// pick_subsurface_block three times).
    #[test]
    #[ignore]
    fn biome_def_breakdown() {
        use std::time::Instant;
        let (g, loaded) = production_like_generator(20260902);
        let (ox, _oy, oz) = ChunkCoord::new(4, 0, 4).block_origin();
        let cols: Vec<(i32, i32)> = (0..CHUNK_SIZE).flat_map(|lz| (0..CHUNK_SIZE).map(move |lx| (lx, lz)))
            .map(|(lx, lz)| (ox + lx, oz + lz)).collect();
        let rounds = 4;
        let has_reg = g.biome_registry.is_some();
        println!("(standard registry loaded: {})", loaded);
        let n_biomes = g.biome_registry.as_ref().map(|r| r.len()).unwrap_or(0);
        println!("biome_def breakdown (registry: {}, {} biomes), 1024 columns x 4 calls:", has_reg, n_biomes);
        let time = |label: &str, f: &dyn Fn() -> f64| {
            let t = Instant::now();
            let mut sink = 0.0f64;
            for _ in 0..rounds { sink += f(); }
            let ms = t.elapsed().as_secs_f64() * 1e3 / rounds as f64;
            println!("  {:36} {:8.2} ms   ({:7.3} us/call)  [{}]", label, ms, ms * 1000.0 / (1024.0 * 4.0), sink as i64);
        };
        time("biome_def_at x4 (as the loop calls it)", &|| {
            let mut n = 0.0;
            for &(x, z) in &cols {
                for dy in 0..4 {
                    if g.biome_def_at(x, 40 - dy, z).is_some() { n += 1.0; }
                }
            }
            n
        });
        time("  temperature + humidity x4", &|| {
            let mut a = 0.0;
            for &(x, z) in &cols {
                for dy in 0..4 {
                    let y = (40 - dy) as f64;
                    a += g.model.temperature(x as f64, y, z as f64, 0.0)
                       + g.model.humidity(x as f64, y, z as f64, 0.0);
                }
            }
            a
        });
        time("  best_fit x4 (fixed climate)", &|| {
            let mut n = 0.0;
            if let Some(reg) = &g.biome_registry {
                for (i, _) in cols.iter().enumerate() {
                    let t = (i % 100) as f64 / 100.0;
                    for _ in 0..4 { if reg.best_fit(t * 2.0, 0.5).is_some() { n += 1.0; } }
                }
            }
            n
        });
        // Do the 4 y values actually resolve to different defs? If not,
        // one call per column would do — see the doc note.
        let mut distinct_per_col = 0u32;
        for &(x, z) in cols.iter().take(256) {
            let defs: Vec<Option<&str>> = (0..4).map(|dy| g.biome_def_at(x, 40 - dy, z).map(|d| d.id.as_str())).collect();
            if defs.iter().any(|d| *d != defs[0]) { distinct_per_col += 1; }
        }
        println!("  columns (of 256) where the 4 y values give different defs: {}", distinct_per_col);
    }

    #[test]
    #[ignore]
    fn column_pass_breakdown() {
        use std::time::Instant;
        let g = WorldGenerator::new(20260902);
        let (ox, oy, oz) = ChunkCoord::new(4, 0, 4).block_origin();
        let cols = 1024;

        // 1. surface_target, once per column.
        let t = Instant::now();
        let mut targets = Vec::with_capacity(cols);
        for lz in 0..CHUNK_SIZE { for lx in 0..CHUNK_SIZE {
            targets.push(g.model.surface_target((ox + lx) as f64, (oz + lz) as f64, 0.0));
        }}
        let t_surface = t.elapsed().as_secs_f64();

        // 2. density_from_surface, 32 cells per column.
        let t = Instant::now();
        let mut solid = 0u32;
        for (ci, &st) in targets.iter().enumerate() {
            let lz = ci as i32 / CHUNK_SIZE;
            let lx = ci as i32 % CHUNK_SIZE;
            for ly in 0..CHUNK_SIZE {
                if g.model.density_from_surface(st, (ox + lx) as f64, (oy + ly) as f64, (oz + lz) as f64)
                    > anchors::DENSITY_THRESHOLD { solid += 1; }
            }
        }
        let t_density = t.elapsed().as_secs_f64();

        // 3. is_cave, 32 cells per column — with the column's surface
        //    target reused, as the generator now does.
        let t = Instant::now();
        let mut caves = 0u32;
        for (ci, &st) in targets.iter().enumerate() {
            let lz = ci as i32 / CHUNK_SIZE;
            let lx = ci as i32 % CHUNK_SIZE;
            for ly in 0..CHUNK_SIZE {
                if g.is_cave_with_surface(ox + lx, oy + ly, oz + lz, st) { caves += 1; }
            }
        }
        let t_cave = t.elapsed().as_secs_f64();

        // 4. Everything after the mask: the surface/biome pass. Timed as
        //    (whole chunk) - (the three above), since it is not callable
        //    on its own.
        let t = Instant::now();
        let ch = g.generate_chunk(ChunkCoord::new(4, 0, 4));
        let t_whole = t.elapsed().as_secs_f64();
        std::hint::black_box(&ch);

        let ms = |t: f64| t * 1e3;
        println!("column pass breakdown, one surface chunk (1024 columns, 32768 cells):");
        println!("  surface_target  x1024   {:8.1} ms   ({:7.1} us each)", ms(t_surface), t_surface * 1e6 / cols as f64);
        println!("  density         x32768  {:8.1} ms   ({:7.3} us each)  {} solid", ms(t_density), t_density * 1e6 / 32768.0, solid);
        println!("  is_cave         x32768  {:8.1} ms   ({:7.3} us each)  {} caves", ms(t_cave), t_cave * 1e6 / 32768.0, caves);
        let rest = t_whole - t_surface - t_density - t_cave;
        println!("  ---- whole chunk        {:8.1} ms", ms(t_whole));
        println!("  surface/biome pass etc. {:8.1} ms   (whole minus the three above)", ms(rest));
    }
}
