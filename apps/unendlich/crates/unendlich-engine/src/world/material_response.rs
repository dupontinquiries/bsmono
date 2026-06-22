//! Per-block-id material response matrices.
//!
//! Each block type carries a `MaterialResponse` describing how its
//! visual properties (color, surface bumps) shift in response to local
//! environmental conditions (temperature, humidity, sky exposure,
//! slope, weathering). The mesher samples the environment once per
//! visible face, looks up the response for that block, and applies
//! the matrix to produce the final tint and bump amplitude.
//!
//! Why per-block-id matrices instead of a single global function:
//!   - Different materials respond differently. Grass becomes more
//!     vivid green in humid climates; stone becomes mossy. They share
//!     the SAME environment input (high humidity) but respond in
//!     different visual directions.
//!   - The math is the same shape for every block — a base color plus
//!     additive shifts scaled by environmental signals. That uniform
//!     shape lets the mesher run one fixed code path for all blocks,
//!     just with different coefficients.
//!   - Adding a new climate axis (e.g. wind erosion, fire damage)
//!     becomes one new field on `MaterialResponse` and one new sample
//!     at the call site — no per-block branching.
//!
//! Greedy-meshing interaction:
//!   - If a material's `position_variance > 0.0`, adjacent blocks of
//!     the same type CANNOT be greedy-merged into one quad — they'd
//!     each have a different per-block tint and the merge would lose
//!     that variation. The mesher checks `merges_at(wy)` before
//!     merging.
//!   - If `position_variance == 0.0`, greedy merging proceeds normally.
//!     Used for deep stone, ore, bedrock — blocks that don't need
//!     visual variation and where merge cost is high.
//!   - Depth-aware: the SAME BlockId can have variance at the surface
//!     (visible to player, varied) and zero variance deep underground
//!     (invisible until mined, can merge for performance). See
//!     `for_block(block, world_y)`.
//!
//! Performance:
//!   - One `MaterialResponse` lookup per visible face. Currently a
//!     match statement; could be a static array indexed by BlockId
//!     enum if profiling shows the match is hot.
//!   - `tint(env)` is 5 multiply-add operations per channel — 15 muladd
//!     total. Negligible compared to the noise math already in place.
//!   - `bump_amplitude(env)` is 3 muladd. Even cheaper.

use crate::world::blocks::BlockId;

/// Local environmental sample at a block position. Computed once per
/// visible face and consumed by `MaterialResponse`. Values are
/// normalised so each is roughly in [0, 1].
#[derive(Debug, Clone, Copy)]
pub struct EnvironmentSample {
    /// Air temperature, [0=cold, 0.5=temperate, 1=hot].
    pub temperature: f32,
    /// Air humidity, [0=arid, 1=saturated].
    pub humidity: f32,
    /// Sky exposure, [0=fully buried, 1=open sky overhead].
    /// Computed cheaply: count of consecutive air blocks above this
    /// block in the chunk, capped at SKY_PROBE_HEIGHT and normalised.
    pub sky_exposure: f32,
    /// Local terrain slope, [0=flat, 1=steep cliff].
    /// Magnitude of (max - min) of surface_y at four neighbors,
    /// normalised by a typical-slope constant.
    pub slope: f32,
    /// Weathering, [0=fresh, 1=fully weathered]. Already used by the
    /// existing bump system; included here so MaterialResponse can
    /// modulate color too (weathered grass is more brown, weathered
    /// stone is more mossy).
    pub weathering: f32,
}

impl EnvironmentSample {
    /// A sample with all-neutral values. Used as fallback when full
    /// environmental sampling isn't available (e.g. in tests, or
    /// when the chunk doesn't have neighbors yet for slope calc).
    pub const fn neutral() -> Self {
        Self {
            temperature: 0.5,
            humidity: 0.5,
            sky_exposure: 1.0,
            slope: 0.0,
            weathering: 0.5,
        }
    }
}

/// Per-channel color shift, added to base when signal is at maximum,
/// scaled linearly by signal value. RGB in [-1, 1] effective.
pub type ColorShift = [f32; 3];

/// How a material visually responds to local environmental conditions.
///
/// All shifts are expressed relative to `base_rgb`. The final color
/// is computed as:
///   color = base_rgb
///         + humid_shift     × env.humidity
///         + temp_warm_shift × max(0, env.temperature - 0.5) × 2
///         + temp_cold_shift × max(0, 0.5 - env.temperature) × 2
///         + sky_dim_shift   × (1 - env.sky_exposure)
///         + weathering_shift × env.weathering
///         + position_jitter × position_variance
///
/// The 2× on temperature shifts is so that signals are normalised to
/// [0, 1] over their relevant half-range (cold or hot).
#[derive(Debug, Clone, Copy)]
pub struct MaterialResponse {
    /// Base RGB color of the material in [0, 1]. Replaces the old
    /// `block_color()` palette entry for this material.
    pub base_rgb: [f32; 3],

    /// Color shift applied at maximum humidity. Grass: greener.
    /// Stone: darker (water stains). Sand: less saturated (damp).
    pub humid_shift: ColorShift,

    /// Color shift applied at maximum hot temperature (env.temperature
    /// > 0.5). Grass: yellower (sun-bleached). Sand: more orange.
    pub temp_warm_shift: ColorShift,

    /// Color shift applied at maximum cold temperature (env.temperature
    /// < 0.5). Grass: blue-tinged (frost). Stone: pale.
    pub temp_cold_shift: ColorShift,

    /// Color shift in deep shadow (low sky exposure). Used to give
    /// underhangs and caves a cooler tint than open surfaces. Helps
    /// disambiguate "this is a ceiling" from "this is a wall" via
    /// color cue alone.
    pub sky_dim_shift: ColorShift,

    /// Color shift at full weathering. Grass: browner. Stone: greener
    /// (moss). Sand: greyer (dust).
    pub weathering_shift: ColorShift,

    /// Per-block position-noise amplitude. 0 = no per-block variation
    /// (this material's faces can be greedy-merged). >0 = each block
    /// gets its own random tint within this amplitude (no greedy
    /// merge).
    pub position_variance: f32,

    /// Base bump amplitude in blocks. Replaces the old softness-driven
    /// path. 0 = no surface bumps (smooth like stone, glass).
    pub bump_amplitude: f32,

    /// Extra bump multiplier when on a slope. >1 amplifies bumps on
    /// steep faces (loose material on slopes is rougher than on flat
    /// ground). 1.0 = no slope effect.
    pub slope_bump_boost: f32,

    /// Bump multiplier as weathering rises. Fresh materials have
    /// little roughness; weathered materials are bumpier.
    pub weathering_bump_scale: f32,
}

impl MaterialResponse {
    /// Apply this response to an environmental sample to produce a
    /// per-block color multiplier. The returned RGB is the absolute
    /// color (already including base_rgb), in [0, 1] after clamping.
    ///
    /// Caller is responsible for multiplying by the face light factor.
    #[inline]
    pub fn tint(&self, env: EnvironmentSample) -> [f32; 3] {
        // Decompose temperature into warm-strength and cold-strength,
        // each in [0, 1] over the relevant half-range.
        let warm = ((env.temperature - 0.5) * 2.0).max(0.0);
        let cold = ((0.5 - env.temperature) * 2.0).max(0.0);
        let dim = 1.0 - env.sky_exposure;

        let mut out = self.base_rgb;
        for i in 0..3 {
            out[i] = (out[i]
                + self.humid_shift[i] * env.humidity
                + self.temp_warm_shift[i] * warm
                + self.temp_cold_shift[i] * cold
                + self.sky_dim_shift[i] * dim
                + self.weathering_shift[i] * env.weathering)
                .clamp(0.0, 1.0);
        }
        out
    }

    /// Effective bump amplitude given the environment.
    #[inline]
    pub fn bump_amplitude(&self, env: EnvironmentSample) -> f32 {
        let slope_factor = 1.0 + (self.slope_bump_boost - 1.0) * env.slope;
        let weathering_factor = self.weathering_bump_scale * env.weathering;
        // weathering scales bumps; slope amplifies further.
        self.bump_amplitude * weathering_factor * slope_factor
    }

    /// Whether faces of this material at this depth can be greedy-
    /// merged. False if `position_variance > 0` (per-block tint
    /// requires per-block quads).
    #[inline]
    pub fn merges(&self) -> bool {
        self.position_variance == 0.0
    }
}

/// Look up the material response for a block at a given world depth.
///
/// Depth-awareness lets the SAME BlockId have different responses at
/// the surface (where the player sees it) versus deep underground
/// (where it's hidden behind other stone). Surface stone gets visual
/// variation; deep stone gets greedy-merged for performance.
///
/// The rule: hard non-decorative blocks switch to a "deep" variant
/// at world_y < SEA_LEVEL - 16 (rough cave depth). Soft surface
/// blocks (grass, sand, dirt) keep their varied response everywhere
/// since they're never deep.
pub fn for_block(block: BlockId, world_y: i32) -> MaterialResponse {
    use BlockId::*;
    let sea_level = crate::constants::SEA_LEVEL as i32;
    let is_deep = world_y < sea_level - 16;

    match block {
        // ---- Soft surface blocks: varied everywhere ----
        Grass => MaterialResponse {
            base_rgb: [0.35, 0.70, 0.30],
            // Wet grass is deeper, more saturated green.
            humid_shift: [-0.05, 0.10, -0.05],
            // Hot+sunny grass is yellower (savanna look).
            temp_warm_shift: [0.15, 0.05, -0.10],
            // Cold grass takes on a pale, frost-bluish cast.
            temp_cold_shift: [-0.05, -0.05, 0.08],
            // Shaded grass is darker and slightly cooler.
            sky_dim_shift: [-0.10, -0.05, 0.02],
            // Weathered grass becomes more brown/dry.
            weathering_shift: [0.05, -0.10, -0.05],
            position_variance: 0.10,
            bump_amplitude: 0.30,
            slope_bump_boost: 1.0,
            weathering_bump_scale: 1.0,
        },

        Sand => MaterialResponse {
            base_rgb: [0.92, 0.86, 0.65],
            // Wet sand is darker/duller.
            humid_shift: [-0.20, -0.18, -0.10],
            // Hot dry sand is more orange.
            temp_warm_shift: [0.05, -0.05, -0.15],
            // Cold sand reads as pale, slightly grey.
            temp_cold_shift: [-0.05, 0.0, 0.05],
            sky_dim_shift: [-0.15, -0.12, -0.05],
            weathering_shift: [-0.05, -0.05, -0.05],
            position_variance: 0.08,
            // Sand is loose — slope makes ridges/dunes more visible.
            bump_amplitude: 0.30,
            slope_bump_boost: 1.5,
            weathering_bump_scale: 1.0,
        },

        Dirt => MaterialResponse {
            base_rgb: [0.55, 0.40, 0.25],
            humid_shift: [-0.15, -0.10, -0.05],
            temp_warm_shift: [0.08, 0.02, -0.05],
            temp_cold_shift: [-0.05, -0.03, 0.0],
            sky_dim_shift: [-0.12, -0.10, -0.05],
            weathering_shift: [-0.05, -0.05, -0.03],
            position_variance: 0.10,
            bump_amplitude: 0.30,
            slope_bump_boost: 1.3,
            weathering_bump_scale: 1.0,
        },

        Gravel => MaterialResponse {
            base_rgb: [0.65, 0.62, 0.58],
            humid_shift: [-0.12, -0.10, -0.10],
            temp_warm_shift: [0.05, 0.03, -0.02],
            temp_cold_shift: [-0.03, -0.03, 0.02],
            sky_dim_shift: [-0.10, -0.08, -0.05],
            weathering_shift: [-0.05, -0.05, -0.05],
            position_variance: 0.08,
            bump_amplitude: 0.30,
            slope_bump_boost: 1.4,
            weathering_bump_scale: 1.0,
        },

        Snow | SnowBlock => MaterialResponse {
            base_rgb: [0.95, 0.95, 1.00],
            // Wet snow becomes dull, slightly grey.
            humid_shift: [-0.05, -0.05, -0.05],
            // Warm snow is melting — slightly grey.
            temp_warm_shift: [-0.10, -0.10, -0.10],
            // Cold snow is bluer.
            temp_cold_shift: [-0.05, -0.02, 0.05],
            sky_dim_shift: [-0.10, -0.08, -0.02],
            // Old snow is grey/dirty.
            weathering_shift: [-0.10, -0.10, -0.08],
            position_variance: 0.05,
            bump_amplitude: if matches!(block, Snow) { 0.30 } else { 0.20 },
            slope_bump_boost: 1.2,
            weathering_bump_scale: 1.0,
        },

        Moss => MaterialResponse {
            base_rgb: [0.30, 0.55, 0.30],
            humid_shift: [-0.05, 0.10, -0.05],
            temp_warm_shift: [0.05, -0.05, -0.05],
            temp_cold_shift: [-0.05, -0.05, 0.0],
            sky_dim_shift: [-0.05, 0.02, 0.0], // moss likes shade — barely dimmer
            weathering_shift: [-0.05, 0.05, -0.02],
            position_variance: 0.10,
            bump_amplitude: 0.20,
            slope_bump_boost: 1.0,
            weathering_bump_scale: 1.0,
        },

        Clay => MaterialResponse {
            base_rgb: [0.65, 0.65, 0.75],
            humid_shift: [-0.10, -0.10, -0.05],
            temp_warm_shift: [0.05, 0.02, -0.03],
            temp_cold_shift: [-0.02, -0.02, 0.05],
            sky_dim_shift: [-0.08, -0.08, -0.05],
            weathering_shift: [-0.05, -0.05, -0.05],
            position_variance: 0.07,
            bump_amplitude: 0.20,
            slope_bump_boost: 1.0,
            weathering_bump_scale: 0.8,
        },

        // ---- Foliage: leaves vary a lot with climate ----
        OakLeaves => MaterialResponse {
            base_rgb: [0.18, 0.55, 0.20],
            // Wet leaves are darker, deeper green.
            humid_shift: [-0.05, 0.10, -0.02],
            // Hot dry leaves: yellow autumn-ish shift.
            temp_warm_shift: [0.18, 0.05, -0.08],
            // Cold leaves: dim, hint of red (autumn cold).
            temp_cold_shift: [0.10, -0.10, 0.0],
            sky_dim_shift: [-0.05, -0.05, 0.0],
            weathering_shift: [0.10, -0.10, -0.05],
            position_variance: 0.15, // leaves are very varied
            bump_amplitude: 0.0, // foliage doesn't bump
            slope_bump_boost: 1.0,
            weathering_bump_scale: 0.0,
        },

        BirchLeaves => MaterialResponse {
            base_rgb: [0.40, 0.65, 0.35],
            humid_shift: [-0.05, 0.05, -0.02],
            temp_warm_shift: [0.10, 0.0, -0.10],
            temp_cold_shift: [0.05, -0.05, 0.0],
            sky_dim_shift: [-0.05, -0.05, 0.0],
            weathering_shift: [0.05, -0.10, -0.05],
            position_variance: 0.15,
            bump_amplitude: 0.0,
            slope_bump_boost: 1.0,
            weathering_bump_scale: 0.0,
        },

        // ---- Hard surface blocks: varied at surface, plain when deep ----
        Stone => {
            if is_deep {
                MaterialResponse {
                    base_rgb: [0.55, 0.55, 0.55],
                    humid_shift: [0.0; 3],
                    temp_warm_shift: [0.0; 3],
                    temp_cold_shift: [0.0; 3],
                    sky_dim_shift: [0.0; 3],
                    weathering_shift: [0.0; 3],
                    position_variance: 0.0, // CAN merge — invisible in caves
                    bump_amplitude: 0.0,
                    slope_bump_boost: 1.0,
                    weathering_bump_scale: 0.0,
                }
            } else {
                MaterialResponse {
                    base_rgb: [0.55, 0.55, 0.55],
                    // Wet stone is darker.
                    humid_shift: [-0.10, -0.08, -0.05],
                    temp_warm_shift: [0.05, 0.02, -0.02],
                    temp_cold_shift: [-0.02, -0.02, 0.03],
                    sky_dim_shift: [-0.10, -0.08, -0.03],
                    // Weathered stone gets mossy.
                    weathering_shift: [-0.08, 0.05, -0.05],
                    position_variance: 0.06,
                    bump_amplitude: 0.0, // stone doesn't bump (sharp edges)
                    slope_bump_boost: 1.0,
                    weathering_bump_scale: 0.0,
                }
            }
        }

        Cobblestone => MaterialResponse {
            base_rgb: [0.50, 0.50, 0.50],
            humid_shift: [-0.08, -0.05, -0.05],
            temp_warm_shift: [0.03, 0.02, -0.02],
            temp_cold_shift: [-0.02, -0.02, 0.03],
            sky_dim_shift: [-0.08, -0.08, -0.05],
            // Cobblestone gets very mossy when weathered.
            weathering_shift: [-0.10, 0.10, -0.05],
            position_variance: 0.08,
            bump_amplitude: 0.0,
            slope_bump_boost: 1.0,
            weathering_bump_scale: 0.0,
        },

        // ---- Deep / decorative: no environmental response, mergeable ----
        Bedrock | CoalOre | IronOre | GoldOre | DiamondOre
        | Limestone | Marble | Basalt | Tuff | Granite
        | Slate | Calcite | Andesite | Schist | Bone | Amber | Crystal
        | OakLog | BirchLog
        | RedSandstone | Cactus | Dripstone | Ice
        | Flower | TallGrass | Fern | DeadBush | Vine | Mushroom
        | Cobweb | Lichen
        | Water | Lava
        | Sandstone | Obsidian | CopperOre | Quartz
        | OakPlanks | StoneBricks | Bricks | Glass
        | IronBlock | GoldBlock | CoalBlock | Bookshelf | Pumpkin
        | MossyCobblestone | PolishedStone | SmoothSandstone | Terracotta
        | ChiseledStoneBricks | DarkOakPlanks | PolishedBasalt
        | Air => {
            // Default: pull base color from existing palette, no
            // environmental shifts, allow greedy merge. Some blocks
            // (Water, Lava) will get specialised treatment later when
            // we do fluids properly; for now they pass through.
            let base = legacy_block_color(block).unwrap_or([0.5, 0.5, 0.5]);
            MaterialResponse {
                base_rgb: base,
                humid_shift: [0.0; 3],
                temp_warm_shift: [0.0; 3],
                temp_cold_shift: [0.0; 3],
                sky_dim_shift: [0.0; 3],
                weathering_shift: [0.0; 3],
                position_variance: 0.0,
                bump_amplitude: 0.0,
                slope_bump_boost: 1.0,
                weathering_bump_scale: 0.0,
            }
        }
    }
}

/// Fallback color table for blocks that don't have specialised material
/// responses. Mirrors `mesher::block_color()` for these block types.
fn legacy_block_color(block: BlockId) -> Option<[f32; 3]> {
    use BlockId::*;
    Some(match block {
        Air => return None,
        Water => [0.20, 0.40, 0.85],
        Bedrock => [0.20, 0.20, 0.20],
        RedSandstone => [0.78, 0.45, 0.25],
        OakLog => [0.45, 0.30, 0.18],
        BirchLog => [0.85, 0.85, 0.80],
        CoalOre => [0.30, 0.30, 0.30],
        IronOre => [0.70, 0.60, 0.50],
        GoldOre => [0.85, 0.75, 0.30],
        DiamondOre => [0.45, 0.85, 0.95],
        Ice => [0.65, 0.85, 0.95],
        Cactus => [0.20, 0.55, 0.20],
        Dripstone => [0.65, 0.50, 0.50],
        Flower => [0.95, 0.30, 0.40],
        TallGrass => [0.40, 0.70, 0.30],
        Fern => [0.30, 0.60, 0.30],
        DeadBush => [0.55, 0.40, 0.20],
        Vine => [0.30, 0.55, 0.25],
        Mushroom => [0.85, 0.30, 0.30],
        Cobweb => [0.85, 0.85, 0.85],
        Lichen => [0.55, 0.65, 0.55],
        Limestone => [0.85, 0.83, 0.78],
        Marble => [0.92, 0.92, 0.89],
        Basalt => [0.18, 0.17, 0.20],
        Tuff => [0.55, 0.52, 0.50],
        Granite => [0.72, 0.55, 0.48],
        Slate => [0.30, 0.32, 0.38],
        Calcite => [0.94, 0.94, 0.96],
        Andesite => [0.50, 0.50, 0.52],
        Schist => [0.38, 0.36, 0.40],
        Bone => [0.93, 0.90, 0.78],
        Amber => [0.85, 0.55, 0.18],
        Crystal => [0.78, 0.65, 0.95],
        Lava => [0.95, 0.45, 0.10],
        Sandstone => [0.86, 0.78, 0.58],
        Obsidian => [0.10, 0.08, 0.15],
        CopperOre => [0.72, 0.50, 0.38],
        Quartz => [0.93, 0.91, 0.88],
        OakPlanks => [0.72, 0.56, 0.34],
        StoneBricks => [0.55, 0.55, 0.55],
        Bricks => [0.62, 0.32, 0.26],
        Glass => [0.82, 0.90, 0.93],
        IronBlock => [0.82, 0.82, 0.84],
        GoldBlock => [0.92, 0.80, 0.30],
        CoalBlock => [0.12, 0.12, 0.13],
        Bookshelf => [0.60, 0.46, 0.30],
        Pumpkin => [0.85, 0.50, 0.15],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tint at neutral environment should be exactly the base color.
    /// (No shifts contribute when humidity=0.5, temperature=0.5,
    /// sky=1, weathering=0.5 for shifts that don't activate.)
    /// Actually weathering=0.5 DOES contribute half the weathering_shift,
    /// so a true "no shift" baseline needs weathering=0.
    #[test]
    fn neutral_environment_with_zero_weathering_returns_base() {
        let mut env = EnvironmentSample::neutral();
        env.weathering = 0.0;
        env.humidity = 0.0;
        // At humidity=0, temperature=0.5 (neither warm nor cold),
        // sky=1 (no dim), weathering=0, no shift fires. Base color
        // should be exactly preserved.
        let r = for_block(BlockId::Grass, 70);
        let tint = r.tint(env);
        assert_eq!(tint, r.base_rgb);
    }

    /// Humid environment shifts grass color (humid_shift activates).
    #[test]
    fn humid_grass_shifts_toward_green() {
        let r = for_block(BlockId::Grass, 70);
        let mut env = EnvironmentSample::neutral();
        env.weathering = 0.0;
        env.humidity = 1.0;
        let dry_tint = {
            let mut e = env;
            e.humidity = 0.0;
            r.tint(e)
        };
        let humid_tint = r.tint(env);
        // humid shift is [-0.05, +0.10, -0.05] — green channel up.
        assert!(humid_tint[1] > dry_tint[1],
            "humid grass should be greener: dry G={}, humid G={}",
            dry_tint[1], humid_tint[1]);
        assert!(humid_tint[0] < dry_tint[0],
            "humid grass should be less red: dry R={}, humid R={}",
            dry_tint[0], humid_tint[0]);
    }

    /// Hot grass shifts toward yellow.
    #[test]
    fn hot_grass_shifts_toward_yellow() {
        let r = for_block(BlockId::Grass, 70);
        let cool = {
            let mut e = EnvironmentSample::neutral();
            e.weathering = 0.0; e.humidity = 0.0; e.temperature = 0.5;
            r.tint(e)
        };
        let hot = {
            let mut e = EnvironmentSample::neutral();
            e.weathering = 0.0; e.humidity = 0.0; e.temperature = 1.0;
            r.tint(e)
        };
        // temp_warm is [+0.15, +0.05, -0.10] — R up, B down.
        assert!(hot[0] > cool[0], "hot grass should be redder/yellower");
        assert!(hot[2] < cool[2], "hot grass should be less blue");
    }

    /// Stone deep underground keeps `merges() == true` (mergeable).
    #[test]
    fn deep_stone_merges() {
        let r = for_block(BlockId::Stone, -32);
        assert!(r.merges(), "deep stone should be greedy-mergeable");
        assert_eq!(r.position_variance, 0.0);
    }

    /// Stone at the surface does NOT merge (has variance).
    #[test]
    fn surface_stone_does_not_merge() {
        let r = for_block(BlockId::Stone, 60);
        assert!(!r.merges(), "surface stone should NOT be greedy-mergeable");
        assert!(r.position_variance > 0.0);
    }

    /// Grass always has variance regardless of depth (it's never deep).
    #[test]
    fn grass_always_has_variance() {
        let r_high = for_block(BlockId::Grass, 80);
        let r_low = for_block(BlockId::Grass, 30); // doesn't normally happen but test anyway
        assert!(r_high.position_variance > 0.0);
        assert!(r_low.position_variance > 0.0);
    }

    /// Bump amplitude scales with weathering.
    #[test]
    fn bumps_scale_with_weathering() {
        let r = for_block(BlockId::Grass, 70);
        let fresh = {
            let mut e = EnvironmentSample::neutral();
            e.weathering = 0.0;
            r.bump_amplitude(e)
        };
        let weathered = {
            let mut e = EnvironmentSample::neutral();
            e.weathering = 1.0;
            r.bump_amplitude(e)
        };
        assert!(weathered > fresh,
            "weathered grass should bump more: fresh={}, weathered={}",
            fresh, weathered);
        assert_eq!(fresh, 0.0, "zero weathering should give zero bumps");
    }

    /// Sand on a slope bumps more than sand on flat (loose material
    /// on slopes is rougher).
    #[test]
    fn slope_amplifies_sand_bumps() {
        let r = for_block(BlockId::Sand, 64);
        let flat = {
            let mut e = EnvironmentSample::neutral();
            e.weathering = 1.0;
            e.slope = 0.0;
            r.bump_amplitude(e)
        };
        let slope = {
            let mut e = EnvironmentSample::neutral();
            e.weathering = 1.0;
            e.slope = 1.0;
            r.bump_amplitude(e)
        };
        assert!(slope > flat,
            "sand on slope should bump more: flat={}, slope={}", flat, slope);
    }

    /// Stone bumps stay zero regardless of weathering or slope.
    #[test]
    fn stone_never_bumps() {
        let r = for_block(BlockId::Stone, 60);
        let mut env = EnvironmentSample::neutral();
        env.weathering = 1.0;
        env.slope = 1.0;
        assert_eq!(r.bump_amplitude(env), 0.0);
    }

    /// Cold environment shifts grass toward bluer/dimmer.
    #[test]
    fn cold_grass_shifts_toward_pale_blue() {
        let r = for_block(BlockId::Grass, 70);
        let mild = {
            let mut e = EnvironmentSample::neutral();
            e.weathering = 0.0; e.humidity = 0.0; e.temperature = 0.5;
            r.tint(e)
        };
        let cold = {
            let mut e = EnvironmentSample::neutral();
            e.weathering = 0.0; e.humidity = 0.0; e.temperature = 0.0;
            r.tint(e)
        };
        // temp_cold shift is [-0.05, -0.05, +0.08] — B up.
        assert!(cold[2] > mild[2], "cold grass should be bluer");
    }

    /// Shaded blocks (low sky exposure) get sky_dim_shift applied.
    #[test]
    fn shadowed_blocks_dimmer_than_lit() {
        let r = for_block(BlockId::Grass, 70);
        let lit = {
            let mut e = EnvironmentSample::neutral();
            e.weathering = 0.0; e.humidity = 0.0; e.sky_exposure = 1.0;
            r.tint(e)
        };
        let shadowed = {
            let mut e = EnvironmentSample::neutral();
            e.weathering = 0.0; e.humidity = 0.0; e.sky_exposure = 0.0;
            r.tint(e)
        };
        // Shadow should reduce overall brightness.
        let lit_avg = (lit[0] + lit[1] + lit[2]) / 3.0;
        let shadowed_avg = (shadowed[0] + shadowed[1] + shadowed[2]) / 3.0;
        assert!(shadowed_avg < lit_avg,
            "shadowed grass should be dimmer: lit avg={}, shadowed avg={}",
            lit_avg, shadowed_avg);
    }

    /// Foliage blocks have zero bump amplitude (canopies are smooth).
    #[test]
    fn foliage_does_not_bump() {
        for block in [BlockId::OakLeaves, BlockId::BirchLeaves] {
            let r = for_block(block, 70);
            assert_eq!(r.bump_amplitude, 0.0, "{:?} bump base should be 0", block);
            assert_eq!(r.weathering_bump_scale, 0.0,
                "{:?} weathering bump scale should be 0", block);
        }
    }
}
