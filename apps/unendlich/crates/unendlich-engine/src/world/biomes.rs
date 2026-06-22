//! Biome definitions, climate-based selection, and biome data.

use std::convert::TryFrom;
use crate::world::blocks::BlockId;

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Biome {
    Plains = 0,
    TemperateForest = 1,
    TemperateHighlands = 2,
    BirchForest = 3,
    MapleForest = 4,
    CloudForest = 5,
    MistyMountains = 6,
    AlpineHighlands = 7,
    Desert = 8,
    RedSandstoneCanyon = 9,
    BloomingCanyon = 10,
    Ocean = 11,
    Beach = 12,
    Swamp = 13,
    Tundra = 14,
    Skylands = 15,
    IceCave = 16,
    FungalCave = 17,
    ThermalCave = 18,
    // --- new surface biomes ---
    Savanna = 19,       // warm, dry grassland with sparse trees
    Jungle = 20,        // hot + very wet, dense canopy
    Taiga = 21,         // cool conifer forest
    // --- new cave biomes ---
    CrystalCave = 22,   // crystal/amethyst-lined dry caverns
    LushCave = 23,      // moss/vine/mushroom overgrown caverns
    // --- r336-biomes: new surface biomes (append-only; see MANIFEST re: id rebase) ---
    Mesa = 24,            // hot, very dry, eroded red-rock badlands / plateau
    Mangrove = 25,        // hot + very wet + low-lying coastal/tidal swamp forest
    VolcanicWastes = 26,  // extreme hot + arid + high erosion: basalt/ash flats
    FrozenOcean = 27,     // very cold + low-lying + wet: ice-capped sea
    HighlandSteppe = 28,  // r457: warm-temperate, dry, moderately eroded — raised treeless grassland
}

impl Biome {
    pub fn height_modifier(self) -> f64 {
        match self {
            Biome::MistyMountains => 50.0,
            Biome::AlpineHighlands => 35.0,
            Biome::TemperateHighlands => 25.0,
            Biome::Skylands => 80.0,
            Biome::Ocean => -30.0,
            Biome::Beach => -5.0,
            Biome::RedSandstoneCanyon => 15.0,
            Biome::BloomingCanyon => 12.0,
            Biome::TemperateForest | Biome::MapleForest | Biome::BirchForest => 5.0,
            Biome::CloudForest => 18.0,
            Biome::Swamp => -8.0,
            // r336-biomes
            Biome::Mesa => 15.0,           // raised, layered plateau
            Biome::VolcanicWastes => 8.0,  // low rugged basalt flats
            Biome::FrozenOcean => -30.0,   // sea level, like Ocean
            Biome::Mangrove => -7.0,       // low-lying tidal flat
            Biome::HighlandSteppe => 22.0, // r457: raised dry grassland plateau
            _ => 0.0,
        }
    }

    pub fn erosion_factor(self) -> f64 {
        match self {
            Biome::MistyMountains | Biome::Skylands => 1.5,
            Biome::AlpineHighlands | Biome::TemperateHighlands => 1.2,
            Biome::Plains | Biome::Beach => 0.4,
            _ => 1.0,
        }
    }

    pub fn is_cave_biome(self) -> bool {
        matches!(self, Biome::IceCave | Biome::FungalCave | Biome::ThermalCave | Biome::CrystalCave | Biome::LushCave)
    }

    pub fn is_canyon(self) -> bool {
        matches!(self, Biome::RedSandstoneCanyon | Biome::BloomingCanyon)
    }

    pub fn is_forest(self) -> bool {
        matches!(self,
            Biome::TemperateForest | Biome::BirchForest
            | Biome::MapleForest | Biome::CloudForest
            | Biome::Jungle | Biome::Taiga
            | Biome::Mangrove) // r336-biomes
    }

    pub fn cave_threshold(self) -> f64 {
        if self.is_cave_biome() { 0.5 } else { 0.6 }
    }

    /// Tree-placement parameters per biome. Returns None if the biome
    /// shouldn't have trees at all (oceans, deserts, beaches, caves).
    /// Climate-driven density and species variation comes from this
    /// profile — without it, every biome had identical oak trees at
    /// identical density.
    ///
    /// Note: humidity and temperature ALSO modulate this at runtime
    /// (see `tree_density_factor` in cpu_generator). The biome profile
    /// gives the SHAPE of the tree (trunk height, canopy radius, species);
    /// climate gates whether trees appear at all (e.g. dry cold biomes
    /// might have a forest biome label but few actual trees).
    pub fn tree_profile(self) -> Option<TreeProfile> {
        match self {
            // Forests: dense, varied species.
            Biome::TemperateForest => Some(TreeProfile {
                base_density: 0.04,        // dense — about 1 in 25 columns
                trunk_min_height: 5,
                trunk_max_height: 9,
                canopy_radius: 3,
                trunk_block: BlockId::OakLog,
                leaves_block: BlockId::OakLeaves,
            }),
            Biome::BirchForest => Some(TreeProfile {
                base_density: 0.035,       // dense, slightly sparser than temp.
                trunk_min_height: 6,        // birch is taller, slimmer
                trunk_max_height: 11,
                canopy_radius: 2,           // narrower canopy
                trunk_block: BlockId::BirchLog,
                leaves_block: BlockId::BirchLeaves,
            }),
            Biome::MapleForest => Some(TreeProfile {
                base_density: 0.04,
                trunk_min_height: 5,
                trunk_max_height: 8,
                canopy_radius: 3,
                trunk_block: BlockId::OakLog,   // No MapleLog yet — reuse oak.
                leaves_block: BlockId::OakLeaves, // visual variation via biome lighting later.
                                                  // TODO: add MapleLog/MapleLeaves blocks.
            }),
            Biome::CloudForest => Some(TreeProfile {
                base_density: 0.06,        // dense canopy — wettest forest
                trunk_min_height: 7,        // tall — high humidity grows them up
                trunk_max_height: 13,
                canopy_radius: 4,           // very wide canopy
                trunk_block: BlockId::OakLog,
                leaves_block: BlockId::OakLeaves,
            }),

            // Highlands: sparse, shorter trees (cold + windy).
            Biome::TemperateHighlands => Some(TreeProfile {
                base_density: 0.012,       // sparse — most columns no tree
                trunk_min_height: 3,
                trunk_max_height: 5,
                canopy_radius: 2,
                trunk_block: BlockId::OakLog,
                leaves_block: BlockId::OakLeaves,
            }),
            Biome::AlpineHighlands => Some(TreeProfile {
                base_density: 0.008,       // very sparse — high altitude / cold
                trunk_min_height: 3,
                trunk_max_height: 6,
                canopy_radius: 1,           // narrow conifer-like crown
                trunk_block: BlockId::BirchLog,    // birch as cold-climate substitute
                leaves_block: BlockId::BirchLeaves,
            }),

            // Plains: occasional trees.
            Biome::Plains => Some(TreeProfile {
                base_density: 0.005,       // very sparse — open grassland
                trunk_min_height: 4,
                trunk_max_height: 7,
                canopy_radius: 3,
                trunk_block: BlockId::OakLog,
                leaves_block: BlockId::OakLeaves,
            }),

            // Swamp: short, wide-crowned trees in waterlogged ground.
            Biome::Swamp => Some(TreeProfile {
                base_density: 0.025,
                trunk_min_height: 4,
                trunk_max_height: 6,
                canopy_radius: 3,
                trunk_block: BlockId::OakLog,
                leaves_block: BlockId::OakLeaves,
            }),

            // No-tree biomes.
            // Savanna: sparse, scattered flat-topped trees over open grass.
            Biome::Savanna => Some(TreeProfile {
                base_density: 0.006,
                trunk_min_height: 4,
                trunk_max_height: 7,
                canopy_radius: 3,
                trunk_block: BlockId::OakLog,
                leaves_block: BlockId::OakLeaves,
            }),
            // Jungle: very dense, tall, broad canopy — the wettest, hottest forest.
            Biome::Jungle => Some(TreeProfile {
                base_density: 0.07,
                trunk_min_height: 7,
                trunk_max_height: 14,
                canopy_radius: 3,
                trunk_block: BlockId::OakLog,
                leaves_block: BlockId::OakLeaves,
            }),
            // Taiga: tall, narrow conifers at moderate density.
            Biome::Taiga => Some(TreeProfile {
                base_density: 0.03,
                trunk_min_height: 7,
                trunk_max_height: 13,
                canopy_radius: 2,
                trunk_block: BlockId::OakLog,
                leaves_block: BlockId::OakLeaves,
            }),
            // r336-biomes: Mangrove — short, wide, dense canopy in waterlogged
            // tidal ground (reuses oak geometry until Mangrove blocks exist).
            Biome::Mangrove => Some(TreeProfile {
                base_density: 0.05,
                trunk_min_height: 4,
                trunk_max_height: 7,
                canopy_radius: 3,
                trunk_block: BlockId::OakLog,
                leaves_block: BlockId::OakLeaves,
            }),
            Biome::Desert
            | Biome::Beach
            | Biome::Ocean
            | Biome::Tundra            // tundra has no trees (too cold)
            | Biome::RedSandstoneCanyon
            | Biome::BloomingCanyon    // canyons too dry / too sheer
            | Biome::MistyMountains    // peaks above tree line
            | Biome::Skylands          // floating sky biome — no soil
            | Biome::Mesa              // r336-biomes: arid badlands — no trees
            | Biome::VolcanicWastes    // r336-biomes: scorched basalt — no trees
            | Biome::FrozenOcean       // r336-biomes: open ice sheet — no trees
            | Biome::IceCave
            | Biome::FungalCave
            | Biome::ThermalCave
            | Biome::CrystalCave       // caves: foliage comes from cave gen, not surface trees
            | Biome::LushCave
            | Biome::HighlandSteppe => None, // r457: treeless dry steppe
        }
    }
}

// ── r336-biomes ──────────────────────────────────────────────────────────────
// Characteristic surface block + a terrain palette per biome, focused on the
// new biomes (existing biomes keep sensible defaults; the JSON loader's
// `surface_block` field still overrides at runtime where present). The palette
// is the `[f32;4]` that `biome_blend::BiomeParams.palette` blends across seams
// (RGB tint + a 4th "wetness/specular" channel). Pure & deterministic.
impl Biome {
    /// The default characteristic surface block for this biome. The runtime
    /// JSON biome defs (`biome_loader`) may override this; it is the built-in
    /// fallback the worldgen surface pass uses when no JSON override exists.
    /// Canonical max surface-flora density `[0,1]` (single source of truth for both
    /// `flora_placement::biome_density_base` and `vegetation_scatter::biome_base`, which
    /// can't share via a module dep without a cycle). Oceans + caves are 0 (no surface
    /// flora) — this is what gates grass/trees out of the ocean. r336-biomes: exhaustive
    /// so a new biome must declare its density.
    pub fn flora_density(self) -> f32 {
        match self {
            Biome::Jungle => 0.90,
            Biome::TemperateForest | Biome::BirchForest | Biome::MapleForest | Biome::CloudForest => 0.80,
            Biome::Mangrove | Biome::Swamp => 0.75,
            Biome::Taiga => 0.65,
            Biome::Savanna => 0.55,
            Biome::Plains | Biome::BloomingCanyon => 0.50,
            Biome::Beach | Biome::Skylands => 0.30,
            Biome::TemperateHighlands | Biome::AlpineHighlands | Biome::MistyMountains => 0.25,
            Biome::HighlandSteppe => 0.30, // r457: sparse dry grassland
            Biome::Mesa | Biome::RedSandstoneCanyon => 0.12,
            Biome::Tundra => 0.10,
            Biome::Desert => 0.08,
            // No surface flora: open water + all caves + barren volcanic flats.
            Biome::Ocean | Biome::FrozenOcean | Biome::VolcanicWastes
            | Biome::IceCave | Biome::FungalCave | Biome::ThermalCave
            | Biome::CrystalCave | Biome::LushCave => 0.0,
        }
    }

    pub fn surface_block(self) -> BlockId {
        match self {
            // r336-biomes additions.
            Biome::Mesa => BlockId::RedSandstone,
            Biome::VolcanicWastes => BlockId::Basalt,
            Biome::FrozenOcean => BlockId::Ice,
            Biome::Mangrove => BlockId::Clay,
            // Established characteristic surfaces.
            Biome::Desert | Biome::Beach => BlockId::Sand,
            Biome::RedSandstoneCanyon => BlockId::RedSandstone,
            Biome::Ocean => BlockId::Sand,
            Biome::Tundra => BlockId::SnowBlock,
            Biome::Swamp => BlockId::Moss,
            b if b.is_cave_biome() => BlockId::Stone,
            // Default grassy surface for the rest.
            _ => BlockId::Grass,
        }
    }

    /// Terrain palette `[r, g, b, w]` for this biome — `w` is a
    /// wetness/specular hint. Values in `[0,1]`. Feeds
    /// `biome_blend::BiomeParams.palette`.
    pub fn palette(self) -> [f32; 4] {
        match self {
            // r336-biomes additions.
            Biome::Mesa => [0.78, 0.40, 0.22, 0.05],            // red-orange rock
            Biome::VolcanicWastes => [0.16, 0.13, 0.13, 0.10],  // dark basalt/ash
            Biome::FrozenOcean => [0.74, 0.85, 0.92, 0.85],     // pale ice over water
            Biome::Mangrove => [0.30, 0.42, 0.24, 0.70],        // wet muddy green
            // A representative spread for established biomes.
            Biome::Desert => [0.86, 0.78, 0.52, 0.02],
            Biome::Tundra => [0.86, 0.90, 0.95, 0.30],
            Biome::Ocean => [0.12, 0.30, 0.55, 0.95],
            Biome::Swamp => [0.28, 0.36, 0.26, 0.80],
            b if b.is_forest() => [0.22, 0.50, 0.24, 0.30],
            _ => [0.34, 0.55, 0.30, 0.25], // grassy default
        }
    }
}

/// Tree placement parameters for a biome. Used by worldgen during the
/// vegetation pass.
///
/// `base_density` is then ALSO modulated by runtime climate at
/// generation time — humid biomes get a bonus, dry get a penalty,
/// hot AND dry suppresses trees entirely. See `cpu_generator::
/// climate_tree_factor`.
#[derive(Debug, Clone, Copy)]
pub struct TreeProfile {
    /// Base probability per column that a tree will spawn. Multiplied
    /// by climate factor at runtime.
    pub base_density: f64,
    /// Minimum trunk height in blocks.
    pub trunk_min_height: i32,
    /// Maximum trunk height in blocks (inclusive). Hash picks within range.
    pub trunk_max_height: i32,
    /// Canopy half-extent in blocks. The tree's leaves form a sphere-ish
    /// silhouette of this radius (slightly narrower at top/bottom).
    pub canopy_radius: i32,
    /// Block used for the trunk column.
    pub trunk_block: BlockId,
    /// Block used for the canopy.
    pub leaves_block: BlockId,
}

impl TryFrom<u32> for Biome {
    type Error = u32;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Biome::Plains),
            1 => Ok(Biome::TemperateForest),
            2 => Ok(Biome::TemperateHighlands),
            3 => Ok(Biome::BirchForest),
            4 => Ok(Biome::MapleForest),
            5 => Ok(Biome::CloudForest),
            6 => Ok(Biome::MistyMountains),
            7 => Ok(Biome::AlpineHighlands),
            8 => Ok(Biome::Desert),
            9 => Ok(Biome::RedSandstoneCanyon),
            10 => Ok(Biome::BloomingCanyon),
            11 => Ok(Biome::Ocean),
            12 => Ok(Biome::Beach),
            13 => Ok(Biome::Swamp),
            14 => Ok(Biome::Tundra),
            15 => Ok(Biome::Skylands),
            16 => Ok(Biome::IceCave),
            17 => Ok(Biome::FungalCave),
            18 => Ok(Biome::ThermalCave),
            19 => Ok(Biome::Savanna),
            20 => Ok(Biome::Jungle),
            21 => Ok(Biome::Taiga),
            22 => Ok(Biome::CrystalCave),
            23 => Ok(Biome::LushCave),
            24 => Ok(Biome::Mesa),
            25 => Ok(Biome::Mangrove),
            26 => Ok(Biome::VolcanicWastes),
            27 => Ok(Biome::FrozenOcean),
            other => Err(other),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BiomeSample {
    pub biome: Biome,
    pub temperature: f64,
    pub humidity: f64,
    pub erosion: f64,
}

impl BiomeSample {
    pub fn from_climate(temperature: f64, humidity: f64, erosion: f64) -> Self {
        let biome = determine_biome(temperature, humidity, erosion);
        Self { biome, temperature, humidity, erosion }
    }
}

/// Pure function for biome selection — kept identical to the GPU shader logic
/// so CPU and GPU paths produce matching results.
pub fn determine_biome(temperature: f64, humidity: f64, erosion: f64) -> Biome {
    if erosion > 0.85 {
        return Biome::Skylands;
    }

    // r336-biomes: extreme hot + arid + highly eroded (but below the Skylands
    // cutoff) → scorched basalt flats. Narrow + rare.
    if temperature > 0.9 && humidity < 0.12 && erosion > 0.72 {
        return Biome::VolcanicWastes;
    }

    if temperature < 0.25 {
        // r336-biomes: very cold, low-lying, wet → ice-capped sea.
        if temperature < 0.18 && erosion < 0.25 && humidity > 0.55 {
            return Biome::FrozenOcean;
        }
        if erosion > 0.6 { return Biome::IceCave; }
        return Biome::Tundra;
    }

    // Cool conifer forest, between the frozen tundra and the temperate woods.
    if temperature < 0.42 && humidity > 0.45 && erosion < 0.55 {
        return Biome::Taiga;
    }

    if temperature > 0.75 {
        // r336-biomes: hot + very dry + eroded (below canyon erosion) → red-rock
        // badlands/plateau. Sits between Desert (flatter) and the canyons (more eroded).
        if humidity < 0.22 && erosion > 0.4 && erosion <= 0.65 {
            return Biome::Mesa;
        }
        if erosion > 0.65 {
            return if humidity > 0.5 { Biome::BloomingCanyon } else { Biome::RedSandstoneCanyon };
        }
        // Hot + very wet = dense jungle.
        if humidity > 0.7 { return Biome::Jungle; }
        if humidity < 0.3 { return Biome::Desert; }
    }

    if erosion > 0.7 && temperature > 0.3 && temperature < 0.7 {
        return Biome::MistyMountains;
    }

    if erosion > 0.55 && erosion <= 0.7 {
        // Eroded ground hides caverns: dry warm rock → crystal caves; humid
        // temperate rock → lush, overgrown caves. Otherwise it's highlands.
        if temperature > 0.45 && humidity < 0.32 { return Biome::CrystalCave; }
        if temperature >= 0.4 && temperature < 0.7 && humidity > 0.6 { return Biome::LushCave; }
        return if temperature < 0.45 { Biome::AlpineHighlands } else { Biome::TemperateHighlands };
    }

    // r457: elevated dry grassland — warm-temperate, dry, moderately eroded. Raised treeless steppe
    // (height_modifier lifts it above the plains). Niche is disjoint from Savanna (erosion < 0.4).
    if temperature >= 0.45 && temperature <= 0.7 && humidity < 0.30 && erosion >= 0.40 && erosion <= 0.55
    {
        return Biome::HighlandSteppe;
    }

    if humidity > 0.6 {
        // r336-biomes: warm/hot + very wet + low-lying (little erosion) → tidal
        // mangrove swamp forest. Checked before the cloud/swamp split.
        if temperature > 0.6 && humidity > 0.75 && erosion < 0.3 {
            return Biome::Mangrove;
        }
        if temperature < 0.4 { return Biome::BirchForest; }
        if temperature > 0.6 {
            return if erosion > 0.5 { Biome::CloudForest } else { Biome::Swamp };
        }
        return if erosion < 0.3 { Biome::MapleForest } else { Biome::TemperateForest };
    }

    // Warm, semi-dry grassland with scattered trees.
    if temperature > 0.55 && humidity < 0.48 && erosion < 0.4 {
        return Biome::Savanna;
    }

    if humidity < 0.25 && erosion < 0.3 {
        return Biome::Beach;
    }

    Biome::Plains
}

// ── r336-biomes: tests ───────────────────────────────────────────────────────
#[cfg(test)]
mod r336_biomes_tests {
    #[test]
    fn highland_steppe_selected_in_its_niche() {
        // warm-temperate, dry, moderately eroded → raised treeless steppe (r457).
        assert_eq!(super::determine_biome(0.6, 0.2, 0.48), super::Biome::HighlandSteppe);
        // disjoint from Savanna (erosion < 0.4) and from highlands (erosion > 0.55).
        assert_ne!(super::determine_biome(0.6, 0.2, 0.30), super::Biome::HighlandSteppe);
        assert_ne!(super::determine_biome(0.6, 0.2, 0.60), super::Biome::HighlandSteppe);
        // elevated terrain + treeless.
        assert_eq!(super::Biome::HighlandSteppe.height_modifier(), 22.0);
        assert!(super::Biome::HighlandSteppe.tree_profile().is_none());
    }

    use super::*;
    use std::convert::TryFrom;

    const NEW: [(Biome, u32, &str); 4] = [
        (Biome::Mesa, 24, "Mesa"),
        (Biome::Mangrove, 25, "Mangrove"),
        (Biome::VolcanicWastes, 26, "VolcanicWastes"),
        (Biome::FrozenOcean, 27, "FrozenOcean"),
    ];

    #[test]
    fn discriminants_are_append_only_and_unique() {
        // Existing ids must be unchanged (no renumber — wire/save formats depend
        // on these). Spot-check the anchors of the existing range.
        assert_eq!(Biome::Plains as u32, 0);
        assert_eq!(Biome::Skylands as u32, 15);
        assert_eq!(Biome::LushCave as u32, 23);
        // New ids append after 23, contiguous + unique.
        let mut ids: Vec<u32> = NEW.iter().map(|&(b, _, _)| b as u32).collect();
        assert_eq!(ids, vec![24, 25, 26, 27]);
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 4, "new ids must be unique");
    }

    #[test]
    fn tryfrom_roundtrips_new_and_rejects_unknown() {
        for &(b, id, _) in &NEW {
            assert_eq!(Biome::try_from(id), Ok(b), "id {id} must map to {b:?}");
            assert_eq!(b as u32, id, "{b:?} must encode to {id}");
        }
        assert!(Biome::try_from(28u32).is_err(), "unassigned id must error");
    }

    #[test]
    fn each_new_biome_has_a_climate_cell() {
        // determine_biome must produce each new biome for its documented climate.
        assert_eq!(determine_biome(0.95, 0.08, 0.78), Biome::VolcanicWastes); // hot+arid+eroded
        assert_eq!(determine_biome(0.10, 0.70, 0.10), Biome::FrozenOcean);    // very cold+wet+low
        assert_eq!(determine_biome(0.85, 0.15, 0.50), Biome::Mesa);           // hot+dry+mid-eroded
        assert_eq!(determine_biome(0.70, 0.85, 0.15), Biome::Mangrove);       // hot+wet+low
    }

    #[test]
    fn each_new_biome_has_surface_block_and_palette() {
        assert_eq!(Biome::Mesa.surface_block(), BlockId::RedSandstone);
        assert_eq!(Biome::VolcanicWastes.surface_block(), BlockId::Basalt);
        assert_eq!(Biome::FrozenOcean.surface_block(), BlockId::Ice);
        assert_eq!(Biome::Mangrove.surface_block(), BlockId::Clay);
        for &(b, _, _) in &NEW {
            let p = b.palette();
            assert!(p.iter().all(|c| (0.0..=1.0).contains(c)), "{b:?} palette out of [0,1]: {p:?}");
        }
    }

    #[test]
    fn palettes_blend_within_range() {
        // Blending two biome palettes stays a convex combination (per-channel
        // within the two inputs) — feeds biome_blend at seams.
        use crate::world::biome_blend::{blend, BiomeParams};
        let a = BiomeParams { height_scale: 1.0, roughness: 0.2, temp: 0.9, palette: Biome::Mesa.palette() };
        let b = BiomeParams { height_scale: 0.0, roughness: 0.1, temp: 0.1, palette: Biome::FrozenOcean.palette() };
        let r = blend(&[0.4, 0.6], &[a, b]);
        for k in 0..4 {
            let (lo, hi) = (a.palette[k].min(b.palette[k]), a.palette[k].max(b.palette[k]));
            assert!(r.palette[k] >= lo - 1e-4 && r.palette[k] <= hi + 1e-4, "channel {k} out of convex range");
        }
    }

    #[test]
    fn new_surface_biomes_have_no_trees_except_mangrove() {
        assert!(Biome::Mesa.tree_profile().is_none());
        assert!(Biome::VolcanicWastes.tree_profile().is_none());
        assert!(Biome::FrozenOcean.tree_profile().is_none());
        let m = Biome::Mangrove.tree_profile().expect("mangrove has trees");
        assert!(m.base_density > 0.0 && m.trunk_max_height >= m.trunk_min_height);
        assert!(Biome::Mangrove.is_forest());
    }

    #[test]
    fn deterministic() {
        assert_eq!(determine_biome(0.85, 0.15, 0.50), determine_biome(0.85, 0.15, 0.50));
        assert_eq!(Biome::Mesa.palette(), Biome::Mesa.palette());
    }
}
