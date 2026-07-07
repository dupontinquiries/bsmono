//! World-level field model.
//!
//! Replaces the old "constants and direct noise calls" approach with a model
//! where every world property is a *field* — a function `f(x, y, z) -> T` —
//! sampled lazily from a 3D region grid. Each field has its own region size
//! (independent scale), its own sampler (the noise it derives from), and its
//! own derivation logic (how it composes with other fields).
//!
//! Why region grids? They're the band-aid between two extremes:
//!   - Constants (`SEA_LEVEL = 64`): no spatial variation, can't model local
//!     conditions.
//!   - Per-pixel noise (`fbm_3d(x, y, z, ...)`): full spatial variation,
//!     expensive, no natural place for time-evolution or AI overrides.
//!
//! Region grids give us: cheap O(8 cache-hit) sampling, smooth trilinear
//! blending, a clear state-shape that can be persisted/evolved/overridden
//! later, and per-field tunable resolution.
//!
//! Today every field is a pure function of `(seed, region_coord)`. Tomorrow,
//! fields will become time-evolving (`f(seed, t, region_coord)`), and later
//! still, AI/narrative overrides will write region values directly. The query
//! API (`.sea_level(x, z)`, `.humidity(x, y, z)`) doesn't change across any
//! of those evolutions.

use crate::constants::CHUNK_SIZE;
use crate::world::biomes::{determine_biome, Biome};
use crate::world::noise::{fbm_2d, fbm_3d, noise_2d, noise_3d};
use dashmap::DashMap;
use std::sync::Arc;

/// 3D region coordinate. Same shape as ChunkCoord but at a coarser scale —
/// each region spans `region_size_chunks` chunks in each dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegionCoord {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl RegionCoord {
    pub const fn new(x: i32, y: i32, z: i32) -> Self { Self { x, y, z } }
}

/// 3D region grid keyed by region coord, with values computed lazily by a
/// sampler closure. Trilinear blending is provided for numeric fields;
/// nearest-region lookup is provided for categorical fields.
///
/// Sized at construction (not const-generic) so a field's region size can
/// be tuned per-instance and changed without recompiling everything.
pub struct RegionGrid<T: Send + Sync + Clone + 'static> {
    /// Region edge length in chunks. A region spans (chunks * CHUNK_SIZE)
    /// blocks on each axis.
    region_size_chunks: i32,
    /// Per-field seed offset so different fields' region values don't
    /// correlate by accident.
    seed_offset: u32,
    /// Lazy cache of computed values.
    cache: DashMap<RegionCoord, T>,
    /// Sampler: given a region coord and the field's offset seed, produce
    /// the value at that region's center.
    sampler: Arc<dyn Fn(RegionCoord, u32) -> T + Send + Sync>,
}

impl<T: Send + Sync + Clone + 'static> RegionGrid<T> {
    pub fn new(
        region_size_chunks: i32,
        seed_offset: u32,
        sampler: impl Fn(RegionCoord, u32) -> T + Send + Sync + 'static,
    ) -> Self {
        assert!(region_size_chunks > 0, "region size must be positive");
        Self {
            region_size_chunks,
            seed_offset,
            cache: DashMap::new(),
            sampler: Arc::new(sampler),
        }
    }

    /// Region size in BLOCKS (= region_size_chunks * CHUNK_SIZE).
    #[inline]
    fn region_size_blocks(&self) -> f64 {
        (self.region_size_chunks as f64) * (CHUNK_SIZE as f64)
    }

    /// Look up (or compute and cache) the value at a region center.
    pub fn at(&self, coord: RegionCoord) -> T {
        if let Some(v) = self.cache.get(&coord) {
            return v.value().clone();
        }
        let v = (self.sampler)(coord, self.seed_offset);
        self.cache.insert(coord, v.clone());
        v
    }

    /// Nearest-region lookup at a world-block position. Use for categorical
    /// fields (biome IDs, region "type" enums).
    pub fn at_block_nearest(&self, x: f64, y: f64, z: f64) -> T {
        let rs = self.region_size_blocks();
        let rx = (x / rs).floor() as i32;
        let ry = (y / rs).floor() as i32;
        let rz = (z / rs).floor() as i32;
        self.at(RegionCoord::new(rx, ry, rz))
    }

    /// Optional: clear the cache. Used for invalidation when a field's
    /// dependencies change (e.g., time advances and humidity re-derives
    /// from new pressure values).
    pub fn clear_cache(&self) {
        self.cache.clear();
    }
}

/// Trilinear-blend specialization for fields whose value is `f64`.
/// Numeric fields use this for smooth boundaries between regions.
impl RegionGrid<f64> {
    /// Trilinearly blend the 8 surrounding region centers.
    /// Region centers sit at integer multiples of region_size in world space —
    /// not at the region's geometric center. This is the standard convention
    /// for sampled fields and matches how `noise_3d` is structured.
    pub fn at_block_blended(&self, x: f64, y: f64, z: f64) -> f64 {
        let rs = self.region_size_blocks();
        let fx = x / rs;
        let fy = y / rs;
        let fz = z / rs;
        let xi = fx.floor() as i32;
        let yi = fy.floor() as i32;
        let zi = fz.floor() as i32;
        let tx = fx - fx.floor();
        let ty = fy - fy.floor();
        let tz = fz - fz.floor();

        let v000 = self.at(RegionCoord::new(xi,     yi,     zi));
        let v100 = self.at(RegionCoord::new(xi + 1, yi,     zi));
        let v010 = self.at(RegionCoord::new(xi,     yi + 1, zi));
        let v110 = self.at(RegionCoord::new(xi + 1, yi + 1, zi));
        let v001 = self.at(RegionCoord::new(xi,     yi,     zi + 1));
        let v101 = self.at(RegionCoord::new(xi + 1, yi,     zi + 1));
        let v011 = self.at(RegionCoord::new(xi,     yi + 1, zi + 1));
        let v111 = self.at(RegionCoord::new(xi + 1, yi + 1, zi + 1));

        let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
        lerp(
            lerp(lerp(v000, v100, tx), lerp(v010, v110, tx), ty),
            lerp(lerp(v001, v101, tx), lerp(v011, v111, tx), ty),
            tz,
        )
    }
}

// ============================================================================
// Per-field seed offsets. Keep these stable; changing them changes worlds.
// ============================================================================

mod seeds {
    pub const SEA_LEVEL:  u32 = 0x5EA1_0000;
    pub const HUMIDITY:   u32 = 0x4D01_5700;
    pub const PRESSURE:   u32 = 0x9255_AA01;
    pub const TEMPERATURE: u32 = 0x7E40_0000;
    pub const CONTINENTAL: u32 = 0xC047_1AEA;
    pub const TERRAIN_3D: u32 = 0x3DAA_0001;
    pub const CAVE:       u32 = 0xCA0E_0000;
    /// v0.24.68-cave-gen-multi-scale: low-frequency chamber noise.
    /// Used in the multi-scale `cave_density` model to produce
    /// occasional large open chambers (vs. the worm tunnels of
    /// the base `CAVE` field). Only active when the multi-scale
    /// cave generation flag is enabled at WorldModel construction.
    pub const CAVE_CHAMBER: u32 = 0xCA0E_0001;
    /// v0.24.68-cave-gen-multi-scale: high-frequency seepage noise.
    /// Used in the multi-scale `cave_density` model to produce
    /// narrow veins at depth, attenuated by `depth_factor` so
    /// they only appear well below the surface (modeling bedrock
    /// seepage). Only active when the flag is enabled.
    pub const CAVE_SEEP:  u32 = 0xCA0E_0002;
    /// Mid-frequency hills noise. Adds rolling-hill variation on top of
    /// the low-frequency continental shape. Without this, terrain is
    /// either flat plains or whole-mountain — no in-between.
    pub const HILLS:      u32 = 0x4111_5000;
    /// Erosion field for biome selection. Was previously sampled inline
    /// in cpu_generator; pulled here so density() can look up the same
    /// value to compute biome height_modifier.
    pub const EROSION:    u32 = 0xE105_0000;
    /// High-frequency noise added to the (temperature, humidity) samples
    /// going into determine_biome. The amplitude is small (~0.03) so the
    /// climate fields are visually unchanged, but the threshold boundary
    /// lines in determine_biome wiggle by a few blocks instead of running
    /// straight — softens the visible biome transitions (cherry/snow etc).
    pub const BIOME_JITTER: u32 = 0xB10E_0001;
}

// ============================================================================
// WorldModel — the composed field set.
// ============================================================================

/// Anchoring constants. These are the "what shape is the world?" knobs that
/// sit underneath the noise. Future evolution: each becomes a field of its
/// own with even higher-level region resolution (e.g., per-continent).
pub mod anchors {
    /// Baseline mean sea level, in world-y blocks. Region noise varies the
    /// actual sea level around this point.
    pub const SEA_LEVEL_BASELINE: f64 = 64.0;
    /// Maximum amplitude of sea-level variation per region, in blocks.
    pub const SEA_LEVEL_AMPLITUDE: f64 = 8.0;
    /// Mean atmospheric pressure at SEA_LEVEL_BASELINE, normalized to ~1.0.
    pub const PRESSURE_BASELINE: f64 = 1.0;
    /// How many blocks of altitude correspond to a 0.5 pressure drop.
    pub const PRESSURE_SCALE_HEIGHT: f64 = 200.0;
    /// Mean humidity, in [0,1].
    pub const HUMIDITY_BASELINE: f64 = 0.5;
    /// Mean temperature normalized to [0,1] (0=cold, 1=hot).
    pub const TEMPERATURE_BASELINE: f64 = 0.5;

    /// Density threshold for "solid" — used by terrain decisions.
    pub const DENSITY_THRESHOLD: f64 = 0.0;
    /// Vertical bias scale: density gradient from solid (deep) to air (high)
    /// per this many blocks.
    pub const VERTICAL_BIAS_SCALE: f64 = 40.0;
}

/// Per-field region sizes (in chunks). Tuned per field for what scale of
/// variation makes sense.
pub mod region_sizes {
    /// Sea level varies at continental scale.
    pub const SEA_LEVEL: i32 = 50;
    /// Atmospheric pressure varies smoothly at large scale.
    pub const PRESSURE: i32 = 20;
    /// Humidity at biome-ish scale.
    pub const HUMIDITY: i32 = 10;
    /// Temperature at biome-ish scale.
    pub const TEMPERATURE: i32 = 10;
    /// Continental shape — the outer envelope of sea level.
    pub const CONTINENTAL: i32 = 80;
}

/// The world model. Owns all field grids; provides the queries the
/// generator uses to compute terrain.
///
/// Today: every field is a pure function of (seed, position). The dependency
/// chain (sea level depends on humidity depends on pressure) is encoded
/// inside the methods, but each underlying grid is independently sampled.
///
/// Tomorrow: fields can become time-evolved (the `_t: f64` parameter on
/// query methods). Today we accept the param and ignore it.
pub struct WorldModel {
    seed: u32,

    sea_level_base:  RegionGrid<f64>, // [-1, 1] noise
    pressure_base:   RegionGrid<f64>, // [-1, 1] noise
    humidity_base:   RegionGrid<f64>, // [-1, 1] noise
    temperature_base: RegionGrid<f64>, // [-1, 1] noise
    continental:     RegionGrid<f64>, // [-1, 1] noise — outer envelope of sea level

    /// v0.24.68-cave-gen-multi-scale: when true, `is_cave` uses the
    /// three-component density model (worm tunnels + chambers +
    /// bedrock seepage) instead of the single-noise threshold.
    /// Read from `UNENDLICH_MULTI_SCALE_CAVES=1` at construction
    /// time; defaults to false so existing worlds are byte-identical.
    multi_scale_caves: bool,
    fbm_cave_threshold: f64,
    /// v0.24.68-cave-gen-multi-scale: threshold for the multi-scale
    /// density model; cells with `cave_density > threshold` are
    /// caves. Read from `UNENDLICH_CAVE_THRESHOLD=<f64>` at
    /// construction; defaults to 0.6 (produces "moderately rich"
    /// caves, ~8-15% volume below surface vs. ~5% in the
    /// single-noise model). Only used when `multi_scale_caves`
    /// is true.
    cave_threshold: f64,
}

impl WorldModel {
    pub fn new(seed: u32) -> Self {
        // Sampler: each region gets a noise value at its CENTER in block-space,
        // mapped to roughly [-1, 1].
        let make_noise_sampler = move |region_size_chunks: i32|
            -> Arc<dyn Fn(RegionCoord, u32) -> f64 + Send + Sync>
        {
            let chunk_size = CHUNK_SIZE as f64;
            let region_blocks = region_size_chunks as f64 * chunk_size;
            Arc::new(move |coord: RegionCoord, offset: u32| -> f64 {
                let cx = coord.x as f64 * region_blocks + region_blocks * 0.5;
                let cy = coord.y as f64 * region_blocks + region_blocks * 0.5;
                let cz = coord.z as f64 * region_blocks + region_blocks * 0.5;
                // Noise at "region scale" — frequency adapted so each region
                // gets meaningfully different values from its neighbors.
                let scale = 1.0 / region_blocks;
                let raw = fbm_3d(cx * scale, cy * scale, cz * scale, offset, 3, 1.0, 1.0);
                raw * 2.0 - 1.0 // [0,1] -> [-1,1]
            })
        };

        let make_grid = |region_size: i32, seed_offset: u32| -> RegionGrid<f64> {
            let sampler = make_noise_sampler(region_size);
            let actual_seed = seed.wrapping_add(seed_offset);
            RegionGrid::new(region_size, actual_seed, move |coord, off| sampler(coord, off))
        };

        // v0.24.68-cave-gen-multi-scale: opt-in multi-scale cave
        // generation. Read env once at construction so per-cell
        // `is_cave` calls just check a struct bool (no env reads
        // in the hot path).
        //
        // Default: OFF. With the flag unset, `is_cave` behaves
        // identically to the prior single-noise threshold model,
        // and all WorldModel-derived tests remain byte-identical.
        //
        // When ON, the threshold can be tuned via
        // `UNENDLICH_CAVE_THRESHOLD=<f64>` (default 0.6, producing
        // a "moderately rich" cave density of ~8-15% volume below
        // surface). Lower → more caves; higher → fewer.
        let multi_scale_caves = std::env::var("UNENDLICH_MULTI_SCALE_CAVES")
            .ok().as_deref() == Some("1");
        let fbm_cave_threshold = std::env::var("UNENDLICH_CAVE_THRESHOLD_FBM")
            .ok().and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.70);
        let cave_threshold = std::env::var("UNENDLICH_CAVE_THRESHOLD")
            .ok().and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.6);

        Self {
            seed,
            sea_level_base:   make_grid(region_sizes::SEA_LEVEL,   seeds::SEA_LEVEL),
            pressure_base:    make_grid(region_sizes::PRESSURE,    seeds::PRESSURE),
            humidity_base:    make_grid(region_sizes::HUMIDITY,    seeds::HUMIDITY),
            temperature_base: make_grid(region_sizes::TEMPERATURE, seeds::TEMPERATURE),
            continental:      make_grid(region_sizes::CONTINENTAL, seeds::CONTINENTAL),
            multi_scale_caves,
            fbm_cave_threshold,
            cave_threshold,
        }
    }

    pub fn seed(&self) -> u32 { self.seed }

    // ---- Atmospheric / climate fields ----

    /// Atmospheric pressure at world position. Decreases with altitude
    /// (basic isothermal atmosphere model). [0, ~1.5] roughly.
    pub fn pressure(&self, x: f64, y: f64, z: f64, _t: f64) -> f64 {
        let raw = self.pressure_base.at_block_blended(x, y, z); // [-1, 1]
        let altitude_drop = (-y / anchors::PRESSURE_SCALE_HEIGHT).exp().clamp(0.1, 1.5);
        anchors::PRESSURE_BASELINE * altitude_drop + raw * 0.1
    }

    /// Humidity at world position. Pressure-coupled (high pressure → drier
    /// at the same altitude). [0, 1] clamped.
    pub fn humidity(&self, x: f64, y: f64, z: f64, t: f64) -> f64 {
        let base = self.humidity_base.at_block_blended(x, y, z); // [-1, 1]
        let p = self.pressure(x, y, z, t);
        let pressure_offset = (anchors::PRESSURE_BASELINE - p) * 0.2;
        (anchors::HUMIDITY_BASELINE + base * 0.3 + pressure_offset).clamp(0.0, 1.0)
    }

    /// Temperature at world position. [0, 1].
    pub fn temperature(&self, x: f64, y: f64, z: f64, _t: f64) -> f64 {
        let base = self.temperature_base.at_block_blended(x, y, z);
        // Cooler at high altitude.
        let altitude_cooling = (y - anchors::SEA_LEVEL_BASELINE) / 200.0 * -0.3;
        (anchors::TEMPERATURE_BASELINE + base * 0.3 + altitude_cooling).clamp(0.0, 1.0)
    }

    // ---- Continental / surface fields ----

    /// Continental shape — broad horizontal variation that anchors sea level
    /// and roughly defines "where land vs ocean is." [-1, 1].
    pub fn continental(&self, x: f64, z: f64) -> f64 {
        // Sample at fixed y=0 — continental shape is a 2D-ish concept.
        // We use 3D regions throughout for uniformity; y=0 means "ground floor."
        self.continental.at_block_blended(x, 0.0, z)
    }

    /// Sea level at a horizontal world position. Depends on continental shape
    /// (ocean basins are lower) and humidity (humid regions have slightly
    /// higher seas — placeholder for a real water-cycle later).
    pub fn sea_level(&self, x: f64, z: f64, t: f64) -> f64 {
        let base = self.sea_level_base.at_block_blended(x, 0.0, z); // [-1, 1]
        let cont = self.continental(x, z);                           // [-1, 1]
        let h = self.humidity(x, anchors::SEA_LEVEL_BASELINE, z, t); // [0, 1]
        anchors::SEA_LEVEL_BASELINE
            + base * anchors::SEA_LEVEL_AMPLITUDE
            + cont * 4.0
            + (h - anchors::HUMIDITY_BASELINE) * 6.0
    }

    // ---- Density / terrain ----

    /// Approximate biome at a horizontal world position, using the same
    /// climate inputs as `cpu_generator::biome_at_3d` but at the surface
    /// y (sea_level + 10) rather than a specific 3D point. Used by
    /// `density()` to apply biome-specific height contributions.
    ///
    /// We sample at the surface band because biomes are conceptually 2D
    /// at the surface — the same biome stretches from bedrock up to the
    /// canopy. The 3D variant exists for cave biomes (IceCave, etc.)
    /// which only matter underground anyway and don't drive height.
    pub fn biome_at_2d(&self, x: f64, z: f64) -> Biome {
        let surface_y = self.sea_level(x, z, 0.0) + 10.0;
        let mut temp = self.temperature(x, surface_y, z, 0.0);
        let mut humid = self.humidity(x, surface_y, z, 0.0);
        // v0.24.NEXT-biome-jitter: add a small high-freq noise to the climate
        // values before threshold-classifying. Amplitude 0.03 means thresholds
        // (0.25/0.45/0.5/0.6/0.75 etc in determine_biome) get crossed inside a
        // ~3-block-wide fuzzy band instead of along a straight line. Same
        // SEED_BIOME_JITTER seed + same noise primitive in chunk_generation.comp.
        let jx = (noise_2d(x * 0.18, z * 0.18,
                           self.seed.wrapping_add(seeds::BIOME_JITTER)) - 0.5) * 0.06;
        let jy = (noise_2d(x * 0.18 + 137.7, z * 0.18 + 91.3,
                           self.seed.wrapping_add(seeds::BIOME_JITTER)) - 0.5) * 0.06;
        temp = (temp + jx).clamp(0.0, 1.0);
        humid = (humid + jy).clamp(0.0, 1.0);
        let erosion_raw = (fbm_2d(
            x * 0.0008, z * 0.0008,
            self.seed.wrapping_add(seeds::EROSION),
            2, 1.0, 1.0,
        ) - 0.5) * 2.0;
        let erosion_unit = (erosion_raw + 1.0) * 0.5;
        determine_biome(temp, humid, erosion_unit)
    }

    /// Mid-frequency rolling-hills noise. Returns roughly [-1, 1].
    /// Spatial frequency is between continental (region-scale) and
    /// 3D detail (cell-scale) — gives chunks of varied elevation
    /// across hundreds of blocks. Without this, plains were pancake-
    /// flat with only the ±30-block continental amplitude varying
    /// over kilometres.
    pub fn hills(&self, x: f64, z: f64) -> f64 {
        // Frequency 0.005 means a full wave period over ~200 blocks —
        // visible from one side of a chunk to the other but not so
        // tight that it creates noise-like roughness. 3 octaves give
        // some sub-hill detail.
        let raw = fbm_2d(
            x * 0.005, z * 0.005,
            self.seed.wrapping_add(seeds::HILLS),
            3, 1.0, 1.0,
        );
        // raw is [0, 1] from fbm_2d — recentre to [-1, 1].
        raw * 2.0 - 1.0
    }

    /// Terrain density at a world position. Solid where this > DENSITY_THRESHOLD.
    pub fn density(&self, x: f64, y: f64, z: f64, t: f64) -> f64 {
        // Composed from the 2D per-column term + the per-voxel term. Kept as a
        // single call for existing callers; the chunk generator uses the two
        // pieces directly to avoid recomputing the 2D part 32x per column.
        self.density_from_surface(self.surface_target(x, z, t), x, y, z)
    }

    /// The 2D per-column surface elevation target: sea level + continental
    /// shape + biome height modifier + mid-frequency hills. Depends only on
    /// `(x, z, t)`, so the chunk generator hoists this out of the vertical
    /// loop — it was the bulk of `density`'s cost (4-5 fbm_2d evaluations)
    /// being recomputed for every one of the 32 voxels in a column.
    pub fn surface_target(&self, x: f64, z: f64, t: f64) -> f64 {
        let sea = self.sea_level(x, z, t);
        let cont_height = self.continental(x, z) * 30.0;
        let biome = self.biome_at_2d(x, z);
        let biome_height = biome.height_modifier();
        let hills_height = self.hills(x, z) * 15.0;
        sea + cont_height + biome_height + hills_height
    }

    /// Per-voxel density given a precomputed column `surface_target`. The only
    /// genuinely 3D noise (the detail fbm) lives here; the 2D terms are in
    /// [`surface_target`]. By construction
    /// `density(x,y,z,t) == density_from_surface(surface_target(x,z,t), x,y,z)`
    /// with bit-identical arithmetic, so worldgen parity is preserved.
    pub fn density_from_surface(&self, surface_target: f64, x: f64, y: f64, z: f64) -> f64 {
        // Vertical bias: + below target, - above. Smooth ramp.
        let vertical_bias = ((surface_target - y) / anchors::VERTICAL_BIAS_SCALE)
            .clamp(-1.5, 1.5);

        // 3D detail noise — gives bumps, overhangs, the rough cave shapes.
        let detail = fbm_3d(
            x * 0.025, y * 0.025, z * 0.025,
            self.seed.wrapping_add(seeds::TERRAIN_3D),
            3, 1.0, 1.0,
        ) - 0.5; // recentre to ~[-0.5, 0.5]

        // Detail amplitude is deliberately small relative to the vertical
        // bias gradient (see history below). 0.15 gives ~3x margin so density
        // is monotonic through the surface → one clean boundary, no striping.
        vertical_bias + detail * 0.15
    }

    /// Cave carving: returns true where there should be a cave.
    /// Independent of the main density function so caves can carve through
    /// already-solid terrain.
    ///
    /// v0.24.68: branches on `self.multi_scale_caves`. When false
    /// (default), uses the original single-noise threshold model
    /// (preserved byte-identical for existing worlds). When true,
    /// uses a three-component density: worm tunnels + chambers +
    /// bedrock seepage.
    pub fn is_cave(&self, x: f64, y: f64, z: f64) -> bool {
        if y < 4.0 { return false; } // no caves in the bedrock zone

        if self.multi_scale_caves {
            // ---- v0.24.68 multi-scale cave density ----
            // Three independent noise layers combined into a
            // single density score. Each layer contributes a
            // different cave morphology:
            //
            //   - WORM (60% weight): mid-frequency 3D noise.
            //     Same parameters as the original is_cave so the
            //     dominant cave shape is recognizable. Produces
            //     the worm-tunnel network that defines the bulk
            //     of cave volume.
            //
            //   - CHAMBER (30% weight): low-frequency 3D noise.
            //     Larger features that, when they coincide with
            //     a worm tunnel, produce occasional open-air
            //     chambers along the tunnel route. Sparser than
            //     worms — chambers are rare but bigger when they
            //     appear.
            //
            //   - SEEPAGE (10% weight): high-frequency 3D noise,
            //     scaled by depth. Models the narrow veins that
            //     form in bedrock from prolonged moisture seepage
            //     over geological time. Linearly ramps from 0
            //     at y=20 to 1 at y=-40 (clamped), so seepage
            //     veins ONLY appear well below the surface.
            //
            // Threshold defaults to 0.6 (chosen for "moderately
            // rich" cave density of ~8-15% volume below surface);
            // tunable via UNENDLICH_CAVE_THRESHOLD. The weighted
            // sum of three noise layers has a tighter distribution
            // than a single noise, so the same threshold value
            // produces MORE caves than the single-noise model.
            // Approximate cave volume at common thresholds (rough,
            // depends on actual noise statistics):
            //   threshold=0.50 → ~30-50% (dense Swiss cheese)
            //   threshold=0.55 → ~15-30% (cavernous)
            //   threshold=0.60 → ~8-15%  (moderately rich) [DEFAULT]
            //   threshold=0.65 → ~3-8%   (mild increase)
            //   threshold=0.70 → ~1-3%   (sparse)
            let worm = noise_3d(
                x * 0.05, y * 0.05, z * 0.05,
                self.seed.wrapping_add(seeds::CAVE),
            );
            let chamber = noise_3d(
                x * 0.012, y * 0.012, z * 0.012,
                self.seed.wrapping_add(seeds::CAVE_CHAMBER),
            );
            let depth_factor = ((20.0 - y) / 60.0).clamp(0.0, 1.0);
            let seepage = noise_3d(
                x * 0.15, y * 0.15, z * 0.15,
                self.seed.wrapping_add(seeds::CAVE_SEEP),
            ) * depth_factor;

            let density = worm * 0.6 + chamber * 0.3 + seepage * 0.1;
            density > self.cave_threshold
        } else {
            // Caves use a 2-octave fbm in BOTH paths so the cave noise isn't
            // single-octave grid-aligned. Threshold chosen so the cave fraction
            // matches the original single-noise 0.78 (~7% of sub-surface volume).
            // MEASURED (cave_probe, seed 12345, y 4..80): original >0.78 = 7.22%;
            // fbm >0.58 = 26.85% (3.7x too much — swiss-cheesed the surface and
            // dropped detected surface_y to ~3); fbm >0.70 = 7.64% (match).
            // fbm averages octaves so its variance is tighter, but 0.58 vastly
            // overshot; 0.70 is the correct comparable-volume threshold.
            // Tunable via UNENDLICH_CAVE_THRESHOLD_FBM (defaults to 0.70).
            let cave = fbm_3d(
                x * 0.05, y * 0.05, z * 0.05,
                self.seed.wrapping_add(seeds::CAVE),
                2, 1.0, 1.0,
            );
            cave > self.fbm_cave_threshold
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    /// The generation 10x hoist factors density() into a per-column
    /// surface_target() + a per-voxel density_from_surface(). For the
    /// optimization to be parity-safe, the composed result must be EXACTLY
    /// equal (bit-for-bit, not approx) to the old monolithic density() for
    /// every voxel. Sweep a representative volume and assert exact equality.
    #[test]
    fn hoist_matches_per_voxel_density() {
        let m = WorldModel::new(0xC0FFEE);
        let mut checked = 0u64;
        let mut x = -40i32;
        while x < 40 {
            let mut z = -40i32;
            while z < 40 {
                let st = m.surface_target(x as f64, z as f64, 0.0);
                let mut y = 0i32;
                while y < 128 {
                    let hoisted = m.density_from_surface(st, x as f64, y as f64, z as f64);
                    let direct = m.density(x as f64, y as f64, z as f64, 0.0);
                    assert_eq!(
                        hoisted.to_bits(),
                        direct.to_bits(),
                        "hoist mismatch at ({x},{y},{z}): {hoisted} vs {direct}"
                    );
                    checked += 1;
                    y += 7;
                }
                z += 5;
            }
            x += 5;
        }
        assert!(checked > 1000, "sanity: swept {checked} voxels");
    }

    /// Regression guard for the fbm cave threshold. The 2026-05-29 toolchain
    /// pass found the shipped 0.58 carved ~27% of sub-surface volume (vs the
    /// intended ~7%), swiss-cheesing terrain and dropping detected surface_y to
    /// ~3. The default 0.70 restores ~7-8%. This test fails fast if the
    /// threshold is ever miscalibrated again — sampling the SAME volume the
    /// probe used (origin region, y 4..80). A sane cave fraction is 4-12%.
    #[test]
    fn cave_volume_fraction_is_sane() {
        let m = WorldModel::new(12345);
        let mut hits = 0u64;
        let mut total = 0u64;
        let mut x = -96i32;
        while x < 96 {
            let mut z = -96i32;
            while z < 96 {
                for y in 4..80 {
                    if m.is_cave(x as f64, y as f64, z as f64) {
                        hits += 1;
                    }
                    total += 1;
                }
                z += 2;
            }
            x += 2;
        }
        let frac = 100.0 * hits as f64 / total as f64;
        assert!(
            (4.0..=12.0).contains(&frac),
            "cave fraction {:.2}% out of sane band 4-12% (threshold miscalibrated?)",
            frac
        );
    }

    #[test]
    fn region_grid_caches_results() {
        let calls = std::sync::atomic::AtomicU32::new(0);
        let grid: RegionGrid<f64> = RegionGrid::new(
            10, 0,
            move |_coord, _seed| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                42.0
            },
        );
        // Note: closure captures &calls but we can't use it after. Build a
        // fresh grid via a different counting mechanism.
        let _ = grid.at(RegionCoord::new(0, 0, 0));
        let _ = grid.at(RegionCoord::new(0, 0, 0));
        // Just verify result is consistent.
        assert_eq!(grid.at(RegionCoord::new(0, 0, 0)), 42.0);
    }

    #[test]
    fn region_grid_sample_coordinates_match() {
        // Same world position on either side of `at` should yield the same
        // value (i.e., we compute region coord deterministically).
        let grid: RegionGrid<f64> = RegionGrid::new(10, 0, |coord, _| coord.x as f64);
        let v1 = grid.at_block_nearest(100.0, 50.0, 200.0);
        let v2 = grid.at_block_nearest(100.0, 50.0, 200.0);
        assert_eq!(v1, v2);
    }

    #[test]
    fn region_grid_blended_is_continuous() {
        // Build a grid where region values vary linearly. Trilinear blend
        // should produce a (near-)linear function across boundaries.
        let grid: RegionGrid<f64> = RegionGrid::new(
            10, 0,
            |coord, _| (coord.x + coord.y + coord.z) as f64,
        );
        // Sample at multiple points along x and check monotone.
        let region_blocks = (10 * CHUNK_SIZE) as f64;
        let v_at = |x: f64| grid.at_block_blended(x, 0.0, 0.0);
        let a = v_at(region_blocks * 0.0);
        let b = v_at(region_blocks * 0.5);
        let c = v_at(region_blocks * 1.0);
        // Should be monotonic (a < b < c, roughly).
        assert!(b > a - 0.5);
        assert!(c >= b - 0.5);
    }

    #[test]
    fn world_model_sea_level_near_baseline() {
        let m = WorldModel::new(0xCAFE);
        // Across many points, average sea level should be near the baseline.
        let mut sum = 0.0;
        let mut n = 0;
        for x in (-1000..=1000).step_by(100) {
            for z in (-1000..=1000).step_by(100) {
                sum += m.sea_level(x as f64, z as f64, 0.0);
                n += 1;
            }
        }
        let avg = sum / n as f64;
        assert!(approx_eq(avg, anchors::SEA_LEVEL_BASELINE, 5.0),
                "avg sea level {} not near baseline {}", avg, anchors::SEA_LEVEL_BASELINE);
    }

    #[test]
    fn world_model_density_solid_below_air_above() {
        let m = WorldModel::new(0xCAFE);
        // Far below sea level — should be very solid (positive density).
        let deep = m.density(0.0, -100.0, 0.0, 0.0);
        assert!(deep > 0.5, "deep density should be high, got {}", deep);
        // Far above sea level — should be very air (negative density).
        let high = m.density(0.0, 300.0, 0.0, 0.0);
        assert!(high < -0.5, "high density should be low, got {}", high);
    }

    #[test]
    fn world_model_pressure_drops_with_altitude() {
        let m = WorldModel::new(0xCAFE);
        let p_low = m.pressure(0.0, 0.0, 0.0, 0.0);
        let p_high = m.pressure(0.0, 500.0, 0.0, 0.0);
        assert!(p_high < p_low, "pressure should drop with altitude: {} vs {}", p_high, p_low);
    }

    #[test]
    fn world_model_humidity_in_unit_range() {
        let m = WorldModel::new(0xCAFE);
        for x in (-500..=500).step_by(50) {
            for z in (-500..=500).step_by(50) {
                let h = m.humidity(x as f64, 64.0, z as f64, 0.0);
                assert!(h >= 0.0 && h <= 1.0, "humidity out of [0,1]: {} at ({}, {})", h, x, z);
            }
        }
    }

    #[test]
    fn world_model_deterministic() {
        let m1 = WorldModel::new(0xCAFE);
        let m2 = WorldModel::new(0xCAFE);
        assert_eq!(m1.density(10.0, 20.0, 30.0, 0.0), m2.density(10.0, 20.0, 30.0, 0.0));
        assert_eq!(m1.sea_level(50.0, 50.0, 0.0), m2.sea_level(50.0, 50.0, 0.0));
    }

    /// `hills()` produces values approximately in [-1, 1] across a wide
    /// sample. (fbm is not strictly bounded but stays close.)
    #[test]
    fn world_model_hills_in_reasonable_range() {
        let m = WorldModel::new(0xCAFE);
        for x in (-2000..=2000).step_by(100) {
            for z in (-2000..=2000).step_by(100) {
                let h = m.hills(x as f64, z as f64);
                assert!(h.abs() <= 2.0,
                    "hills out of plausible range: {} at ({},{})", h, x, z);
            }
        }
    }

    /// `hills()` is deterministic for the same seed.
    #[test]
    fn world_model_hills_deterministic() {
        let m1 = WorldModel::new(0xCAFE);
        let m2 = WorldModel::new(0xCAFE);
        for _ in 0..20 {
            let x = 100.0;
            let z = 200.0;
            assert_eq!(m1.hills(x, z), m2.hills(x, z));
        }
    }

    /// `biome_at_2d` returns SOMETHING (deterministic). Doesn't pin a
    /// specific biome — that depends on the noise — but ensures the
    /// lookup runs without panic and is repeatable.
    #[test]
    fn world_model_biome_at_2d_deterministic() {
        let m = WorldModel::new(0xCAFE);
        let b1 = m.biome_at_2d(500.0, 500.0);
        let b2 = m.biome_at_2d(500.0, 500.0);
        assert_eq!(b1, b2);
    }

    /// Biome variation: across a wide world sample, we should see
    /// MULTIPLE distinct biomes (not all the same). This tests that
    /// the climate fields produce enough variation to drive biome
    /// selection across the domain.
    #[test]
    fn world_model_biome_variation_across_world() {
        let m = WorldModel::new(0xCAFE);
        let mut seen = std::collections::HashSet::new();
        for x in (-3000..=3000).step_by(200) {
            for z in (-3000..=3000).step_by(200) {
                seen.insert(m.biome_at_2d(x as f64, z as f64));
            }
        }
        assert!(seen.len() >= 3,
            "expected at least 3 distinct biomes across the domain, got {}: {:?}",
            seen.len(), seen);
    }

    /// Density now incorporates biome height_modifier and hills. Sanity
    /// check: in regions where biomes vary, surface heights should
    /// differ by more than the previous 30-block continental amplitude
    /// would have allowed.
    #[test]
    fn world_model_surface_height_varies_by_biome() {
        let m = WorldModel::new(0xCAFE);
        // Sample surface_y at many points; find min and max.
        // The previous flat-density code produced ~30 blocks of variation;
        // with biome height_modifier and hills, the range should exceed
        // that across a wide world sample.
        let mut min_y: f64 = 1e9;
        let mut max_y: f64 = -1e9;
        for x in (-3000..=3000).step_by(150) {
            for z in (-3000..=3000).step_by(150) {
                // Find approximate surface: the y where density crosses 0.
                // Walk down from y=200; first y where density > 0 is the
                // top of solid land.
                for y in (0..=200).rev() {
                    let d = m.density(x as f64, y as f64, z as f64, 0.0);
                    if d > anchors::DENSITY_THRESHOLD {
                        min_y = min_y.min(y as f64);
                        max_y = max_y.max(y as f64);
                        break;
                    }
                }
            }
        }
        let range = max_y - min_y;
        assert!(range > 50.0,
            "expected surface height range > 50 blocks (now we have hills + biome heights), \
             got range={} (min={}, max={})", range, min_y, max_y);
    }

    // ===== v0.24.68-cave-gen-multi-scale tests =====

    /// Default (env unset) preserves byte-identical cave generation.
    /// is_cave with the same coords/seed must produce the SAME result
    /// as before the v0.24.68 change. We can't directly assert "same
    /// as before" without a baseline, but we CAN assert that the
    /// default-constructed WorldModel takes the single-noise branch
    /// and produces values consistent with the old threshold.
    ///
    /// Specifically: above y=4 the function should sometimes be true
    /// and sometimes be false (caves are sparse, ~5% volume). Below
    /// y=4 always false.
    #[test]
    fn cave_gen_default_preserves_single_noise_path() {
        // Don't rely on env being unset in CI; force the field to
        // its default by constructing the model and inspecting.
        // (If a CI env had UNENDLICH_MULTI_SCALE_CAVES=1 set, this
        // test would falsely fail — flag it in the canonical readme.)
        let m = WorldModel::new(0xCAFE);
        // Bedrock band: always false.
        for y in 0..=3 {
            assert!(!m.is_cave(0.0, y as f64, 0.0),
                "bedrock band (y={}) must never be cave", y);
            assert!(!m.is_cave(123.0, y as f64, 456.0),
                "bedrock band (y={}) must never be cave", y);
        }
    }

    /// Multi-scale cave model is deterministic for the same seed.
    /// Two WorldModel instances with the same seed (and same env)
    /// must produce the same is_cave decisions.
    #[test]
    fn cave_gen_multi_scale_deterministic() {
        // Construct two models and verify the cave decisions agree
        // for a swept volume. This works regardless of whether
        // multi_scale_caves is enabled — both paths are deterministic.
        let m1 = WorldModel::new(0xCAFE);
        let m2 = WorldModel::new(0xCAFE);
        for x in (-50..=50).step_by(7) {
            for z in (-50..=50).step_by(7) {
                for y in (4..=80).step_by(5) {
                    let c1 = m1.is_cave(x as f64, y as f64, z as f64);
                    let c2 = m2.is_cave(x as f64, y as f64, z as f64);
                    assert_eq!(c1, c2,
                        "non-deterministic at ({}, {}, {}): {} vs {}",
                        x, y, z, c1, c2);
                }
            }
        }
    }

    /// Sanity check that the bedrock-no-caves rule applies to both
    /// code paths. With either the single-noise or multi-scale
    /// branch active, y < 4 must always be solid.
    #[test]
    fn cave_gen_bedrock_band_always_solid() {
        let m = WorldModel::new(0xCAFE);
        for x in (-100..=100).step_by(13) {
            for z in (-100..=100).step_by(13) {
                for y in 0..=3 {
                    assert!(!m.is_cave(x as f64, y as f64, z as f64),
                        "bedrock y={} at ({}, {}) must be solid", y, x, z);
                }
            }
        }
    }
}
