//! Biome registry: parses Minecraft-format biome JSON files and provides
//! best-fit lookup against the world's temperature/humidity fields.
//!
//! Why JSON: it lets us add or tune biomes without rebuilding. Designers
//! drop a file in `assets/biomes/<namespace>/<id>.json` and it's loaded
//! at startup. Future-proofs us toward AI/storytelling biome generation
//! (system writes a JSON, registry picks it up next reload).
//!
//! Schema deliberately mirrors Minecraft 1.18+ biome JSON so existing
//! Minecraft biome files (datapacks, modpacks) can be dropped in. Most
//! fields are stored as opaque `serde_json::Value` because we don't have
//! the subsystems to consume them yet (entity spawning, feature placement,
//! particles, audio). When we build those subsystems, the data is already
//! parsed; they just plug into the registry.
//!
//! Best-fit lookup: each biome has `(temperature, downfall)` coordinates.
//! At a world position, we sample WorldModel temperature/humidity and find
//! the closest biome by Euclidean distance in that 2D space. This replaces
//! the old hardcoded `determine_biome` decision tree.

use crate::world::biomes::Biome;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Default Minecraft temperature is in [0.0, 2.0]. Our WorldModel
/// returns humidity in [0.0, 1.0]. Downfall in JSON is also [0.0, 1.0].
/// We normalise temperature to [0, 1] for the best-fit metric.
pub const TEMPERATURE_NORMALISATION: f64 = 2.0;

/// Parsed biome record. Fields that the engine doesn't consume yet are
/// kept as `serde_json::Value` so they can be re-emitted or queried by
/// future subsystems.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiomeDef {
    /// Stable identifier — namespaced like `relics:rusted_expanse` or
    /// `unendlich:plains`.
    #[serde(rename = "_id")]
    pub id: String,

    /// Optional designer comment, ignored by code.
    #[serde(rename = "_comment", default)]
    pub comment: String,

    /// Optional legacy `Biome` enum variant name. When present, lookups
    /// can return this for callers that want the legacy enum surface.
    #[serde(rename = "_legacy_enum", default)]
    pub legacy_enum: Option<String>,

    /// Surface block ID string (e.g. "limestone", "minecraft:grass").
    /// Used by the chunk generator to decorate the topmost solid block in
    /// each column. Resolved through `BlockId::from_id_str`; unknown
    /// values fall back to `BlockId::Stone`.
    ///
    /// Optional: if absent, surface decoration falls back to the legacy
    /// per-`Biome` enum mapping.
    #[serde(rename = "surface_block", default)]
    pub surface_block: Option<String>,

    /// Subsurface block ID string. Used for the 1-3 blocks beneath the
    /// surface. Optional with same fallback as `surface_block`.
    #[serde(rename = "subsurface_block", default)]
    pub subsurface_block: Option<String>,

    /// Whether this biome has precipitation when conditions allow.
    #[serde(default)]
    pub has_precipitation: bool,

    /// Temperature in Minecraft's range [0.0, 2.0]. Used for best-fit lookup.
    pub temperature: f64,

    /// Downfall (≈ humidity) in [0.0, 1.0]. Used for best-fit lookup.
    pub downfall: f64,

    /// Optional temperature modifier (Minecraft has e.g. "frozen", "none").
    #[serde(default)]
    pub temperature_modifier: Option<String>,

    /// Visual/audio effects. Stored opaque for now; consumers parse what
    /// they need.
    #[serde(default)]
    pub effects: Value,

    /// Mob spawners — opaque, used by future entity system.
    #[serde(default)]
    pub spawners: Value,

    /// Spawn cost overrides — opaque.
    #[serde(default)]
    pub spawn_costs: Value,

    /// Carvers (caves, ravines) — opaque, used by future world-gen passes.
    #[serde(default)]
    pub carvers: Value,

    /// Features per generation step — opaque, used by future feature placement.
    /// Minecraft's biome JSON has 10 steps; we accept any length.
    #[serde(default)]
    pub features: Value,

    /// `type` field, kept for round-tripping. We assert it's "minecraft:biome".
    #[serde(rename = "type", default)]
    pub kind: String,
}

impl BiomeDef {
    /// Normalised (temperature, downfall) coords in [0,1]² for best-fit metric.
    /// Temperature is squashed by the normalisation constant; downfall is
    /// assumed to already be in [0,1].
    pub fn climate_coord(&self) -> (f64, f64) {
        ((self.temperature / TEMPERATURE_NORMALISATION).clamp(0.0, 1.0),
         self.downfall.clamp(0.0, 1.0))
    }

    /// Try to map this biome to a legacy `Biome` enum variant. Returns
    /// `Biome::Plains` if no mapping is declared.
    ///
    /// The legacy enum is what existing surface-block / subsurface-block
    /// code consults. New surface-block selection will eventually be
    /// JSON-driven (per-biome `surface_block` field), but until then we
    /// translate.
    pub fn to_legacy(&self) -> Biome {
        match self.legacy_enum.as_deref() {
            Some("Plains") => Biome::Plains,
            Some("Desert") => Biome::Desert,
            Some("TemperateForest") => Biome::TemperateForest,
            Some("BirchForest") => Biome::BirchForest,
            Some("MapleForest") => Biome::MapleForest,
            Some("CloudForest") => Biome::CloudForest,
            Some("Tundra") => Biome::Tundra,
            Some("AlpineHighlands") => Biome::AlpineHighlands,
            Some("TemperateHighlands") => Biome::TemperateHighlands,
            Some("MistyMountains") => Biome::MistyMountains,
            Some("Skylands") => Biome::Skylands,
            Some("Swamp") => Biome::Swamp,
            Some("Beach") => Biome::Beach,
            Some("Ocean") => Biome::Ocean,
            Some("RedSandstoneCanyon") => Biome::RedSandstoneCanyon,
            Some("BloomingCanyon") => Biome::BloomingCanyon,
            Some("IceCave") => Biome::IceCave,
            Some("FungalCave") => Biome::FungalCave,
            Some("ThermalCave") => Biome::ThermalCave,
            // r336-biomes
            Some("Mesa") => Biome::Mesa,
            Some("Mangrove") => Biome::Mangrove,
            Some("VolcanicWastes") => Biome::VolcanicWastes,
            Some("FrozenOcean") => Biome::FrozenOcean,
            // No declared mapping. Fall back based on climate.
            _ => climate_to_legacy(self.temperature, self.downfall),
        }
    }

    /// Resolve `surface_block` to a `BlockId`. Returns `None` if the field
    /// is absent (caller should use legacy enum mapping). Returns
    /// `Some(fallback)` (`BlockId::Stone`) if the field is present but
    /// names an unknown block — also logs a warning so designers can
    /// catch typos.
    pub fn resolve_surface_block(&self) -> Option<crate::world::blocks::BlockId> {
        let s = self.surface_block.as_ref()?;
        match crate::world::blocks::BlockId::from_id_str(s) {
            Some(b) => Some(b),
            None => {
                log::warn!(
                    "Biome '{}' references unknown surface_block '{}' — falling back to Stone",
                    self.id, s,
                );
                Some(crate::world::blocks::BlockId::Stone)
            }
        }
    }

    /// Resolve `subsurface_block` to a `BlockId`. See `resolve_surface_block`.
    pub fn resolve_subsurface_block(&self) -> Option<crate::world::blocks::BlockId> {
        let s = self.subsurface_block.as_ref()?;
        match crate::world::blocks::BlockId::from_id_str(s) {
            Some(b) => Some(b),
            None => {
                log::warn!(
                    "Biome '{}' references unknown subsurface_block '{}' — falling back to Dirt",
                    self.id, s,
                );
                Some(crate::world::blocks::BlockId::Dirt)
            }
        }
    }
}

/// Heuristic legacy mapping for biomes that don't declare `_legacy_enum`.
/// Used so that relics/arenas biomes still produce a sensible surface
/// block via the legacy enum. Crude on purpose — the proper fix is a JSON
/// `surface_block` field on each biome, which we'll add later.
fn climate_to_legacy(temp: f64, downfall: f64) -> Biome {
    // Cold zones
    if temp < 0.2 { return Biome::Tundra; }
    if temp < 0.4 && downfall > 0.4 { return Biome::AlpineHighlands; }
    // Hot dry zones
    if temp > 1.4 && downfall < 0.2 { return Biome::Desert; }
    if temp > 1.0 && downfall < 0.3 { return Biome::Desert; } // savanna -> desert proxy
    // Hot wet
    if temp > 0.9 && downfall > 0.7 { return Biome::TemperateForest; } // jungle -> forest proxy
    // Wet/swampy
    if downfall > 0.8 { return Biome::Swamp; }
    // Temperate forest band
    if downfall > 0.5 { return Biome::TemperateForest; }
    // Default
    Biome::Plains
}

#[derive(Debug)]
pub enum BiomeLoadError {
    Io(String),
    ParseError { path: PathBuf, message: String },
    EmptyRegistry(String),
}

impl std::fmt::Display for BiomeLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BiomeLoadError::Io(s) => write!(f, "I/O error: {}", s),
            BiomeLoadError::ParseError { path, message } =>
                write!(f, "Parse error in {}: {}", path.display(), message),
            BiomeLoadError::EmptyRegistry(s) =>
                write!(f, "Biome registry empty: {}", s),
        }
    }
}
impl std::error::Error for BiomeLoadError {}

/// In-memory biome registry with best-fit lookup.
///
/// Built once at startup from JSON files, then read-only. (Hot reload —
/// allowing JSON edits to take effect at runtime — is a future addition;
/// the architecture supports it because re-loading just produces a new
/// `BiomeRegistry`.)
pub struct BiomeRegistry {
    biomes: Vec<BiomeDef>,
    by_id: HashMap<String, usize>,
}

impl BiomeRegistry {
    /// Build an empty registry. Mostly for tests.
    pub fn empty() -> Self {
        Self { biomes: Vec::new(), by_id: HashMap::new() }
    }

    /// Add a biome. Last-write-wins on duplicate IDs (logs a warning).
    pub fn insert(&mut self, biome: BiomeDef) {
        if let Some(&existing) = self.by_id.get(&biome.id) {
            log::warn!(
                "Biome ID '{}' duplicated — replacing previous entry",
                biome.id
            );
            self.biomes[existing] = biome;
        } else {
            let idx = self.biomes.len();
            self.by_id.insert(biome.id.clone(), idx);
            self.biomes.push(biome);
        }
    }

    /// Total number of loaded biomes.
    pub fn len(&self) -> usize { self.biomes.len() }
    pub fn is_empty(&self) -> bool { self.biomes.is_empty() }

    /// Iterate all biomes.
    pub fn iter(&self) -> impl Iterator<Item = &BiomeDef> {
        self.biomes.iter()
    }

    /// Look up by stable ID, e.g. "relics:rusted_expanse".
    pub fn get_by_id(&self, id: &str) -> Option<&BiomeDef> {
        self.by_id.get(id).map(|&i| &self.biomes[i])
    }

    /// Best-fit lookup. Given world temperature in [0,2] and humidity
    /// (≈downfall) in [0,1], finds the closest biome by Euclidean distance
    /// in normalised climate space.
    ///
    /// Themed biomes (relics/arenas — those without `_legacy_enum`) are
    /// "pushed away" by `THEMED_PENALTY` so they only win in the climate
    /// regions where no legacy biome is close. This keeps the broad world
    /// covered by familiar biomes (Plains, Forest, Tundra, Desert) while
    /// themed pockets still appear at the extremes of climate space — exactly
    /// what you'd want from a "themed biomes are special, not default" model.
    ///
    /// Tune `THEMED_PENALTY` to taste:
    ///   - 0.0 = no bias, themed biomes win on raw distance (original behavior)
    ///   - 0.05 = light bias, themed appear in moderate-extreme zones
    ///   - 0.20 = strong bias, themed only in very extreme corners
    ///
    /// Returns `None` only if the registry is empty.
    pub fn best_fit(&self, temperature: f64, humidity: f64) -> Option<&BiomeDef> {
        const THEMED_PENALTY: f64 = 0.10;
        let target_t = (temperature / TEMPERATURE_NORMALISATION).clamp(0.0, 1.0);
        let target_h = humidity.clamp(0.0, 1.0);

        let mut best: Option<&BiomeDef> = None;
        let mut best_dist_sq = f64::INFINITY;
        for b in &self.biomes {
            let (bt, bh) = b.climate_coord();
            let dt = bt - target_t;
            let dh = bh - target_h;
            let mut d2 = dt * dt + dh * dh;
            // Themed biomes (no `_legacy_enum`) get a distance penalty.
            if b.legacy_enum.is_none() {
                d2 += THEMED_PENALTY * THEMED_PENALTY;
            }
            if d2 < best_dist_sq {
                best_dist_sq = d2;
                best = Some(b);
            }
        }
        best
    }

    /// Load all `.json` files under a directory tree. Recursively walks
    /// subdirectories. Files that fail to parse log a warning and are skipped.
    pub fn load_directory<P: AsRef<Path>>(&mut self, root: P) -> Result<usize, BiomeLoadError> {
        let root = root.as_ref();
        if !root.exists() {
            return Err(BiomeLoadError::Io(format!("path does not exist: {}", root.display())));
        }
        let mut count = 0;
        let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(e) => {
                    log::warn!("Cannot read dir {}: {}", dir.display(), e);
                    continue;
                }
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().map_or(false, |x| x == "json") {
                    match Self::load_file(&path) {
                        Ok(b) => { self.insert(b); count += 1; }
                        Err(e) => log::warn!("{}", e),
                    }
                }
            }
        }
        Ok(count)
    }

    /// Load a single JSON file.
    pub fn load_file<P: AsRef<Path>>(path: P) -> Result<BiomeDef, BiomeLoadError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|e| BiomeLoadError::Io(format!("{}: {}", path.display(), e)))?;
        let biome: BiomeDef = serde_json::from_str(&text)
            .map_err(|e| BiomeLoadError::ParseError {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;
        Ok(biome)
    }

    /// Convenience: build a registry from the standard `assets/biomes/` path
    /// relative to the current working directory.
    pub fn load_standard() -> Result<Self, BiomeLoadError> {
        let mut reg = Self::empty();
        // r174: the operator runs from crates/ (their log shows the music
        // path resolved via ../..). Try CWD first, then the workspace root,
        // then next to the executable — same spirit as the music loader.
        let candidates = ["assets/biomes", "../../assets/biomes", "../assets/biomes"];
        let dir = candidates
            .iter()
            .find(|p| std::path::Path::new(p).is_dir())
            .copied()
            .unwrap_or("assets/biomes");
        let n = reg.load_directory(dir)?;
        if n == 0 {
            return Err(BiomeLoadError::EmptyRegistry(
                "no biome JSON files found under assets/biomes".into()
            ));
        }
        log::info!("Loaded {} biomes from assets/biomes", n);
        Ok(reg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_biome(id: &str, t: f64, d: f64) -> BiomeDef {
        BiomeDef {
            id: id.into(),
            comment: String::new(),
            legacy_enum: None,
            surface_block: None,
            subsurface_block: None,
            has_precipitation: false,
            temperature: t,
            downfall: d,
            temperature_modifier: None,
            effects: Value::Null,
            spawners: Value::Null,
            spawn_costs: Value::Null,
            carvers: Value::Null,
            features: Value::Null,
            kind: "minecraft:biome".into(),
        }
    }

    #[test]
    fn empty_registry_returns_none() {
        let r = BiomeRegistry::empty();
        assert!(r.best_fit(0.5, 0.5).is_none());
    }

    // r336-biomes: the new biomes round-trip through the loader's legacy_enum map.
    #[test]
    fn r336_new_biomes_legacy_enum_roundtrip() {
        for (name, want) in [
            ("Mesa", Biome::Mesa),
            ("Mangrove", Biome::Mangrove),
            ("VolcanicWastes", Biome::VolcanicWastes),
            ("FrozenOcean", Biome::FrozenOcean),
        ] {
            let mut d = make_biome(name, 1.0, 0.2);
            d.legacy_enum = Some(name.to_string());
            assert_eq!(d.to_legacy(), want, "legacy_enum {name} must map to {want:?}");
        }
    }

    #[test]
    fn best_fit_picks_closest_in_climate_space() {
        let mut r = BiomeRegistry::empty();
        r.insert(make_biome("hot_dry",  1.6, 0.0)); // desert
        r.insert(make_biome("cold_wet", 0.1, 0.8)); // tundra-ish
        r.insert(make_biome("temperate", 0.7, 0.5)); // forest

        // Query in the desert region.
        let b = r.best_fit(1.4, 0.05).unwrap();
        assert_eq!(b.id, "hot_dry");

        // Query for tundra.
        let b = r.best_fit(0.15, 0.7).unwrap();
        assert_eq!(b.id, "cold_wet");

        // Middle ground.
        let b = r.best_fit(0.6, 0.5).unwrap();
        assert_eq!(b.id, "temperate");
    }

    #[test]
    fn parses_minimal_biome_json() {
        let json = r#"{
            "type": "minecraft:biome",
            "_id": "test:foo",
            "temperature": 0.7,
            "downfall": 0.4
        }"#;
        let b: BiomeDef = serde_json::from_str(json).unwrap();
        assert_eq!(b.id, "test:foo");
        assert!((b.temperature - 0.7).abs() < 1e-9);
    }

    #[test]
    fn parses_full_biome_json_with_extras() {
        let json = r#"{
            "type": "minecraft:biome",
            "_id": "test:bar",
            "_comment": "test biome",
            "_legacy_enum": "Plains",
            "has_precipitation": true,
            "temperature": 0.5,
            "downfall": 0.6,
            "effects": { "sky_color": 12345 },
            "spawners": { "monster": [] },
            "features": [["x"], []]
        }"#;
        let b: BiomeDef = serde_json::from_str(json).unwrap();
        assert_eq!(b.id, "test:bar");
        assert_eq!(b.legacy_enum.as_deref(), Some("Plains"));
        assert!(b.effects.is_object());
        assert!(b.features.is_array());
        assert_eq!(b.to_legacy(), Biome::Plains);
    }

    #[test]
    fn climate_to_legacy_extremes() {
        // Cold = Tundra
        assert_eq!(climate_to_legacy(0.05, 0.5), Biome::Tundra);
        // Hot dry = Desert
        assert_eq!(climate_to_legacy(1.5, 0.1), Biome::Desert);
        // Wet = Swamp
        assert_eq!(climate_to_legacy(0.7, 0.9), Biome::Swamp);
    }

    #[test]
    fn registry_loads_real_biome_files() {
        // This test only runs if assets/biomes is present in the cwd.
        if !Path::new("assets/biomes").exists() { return; }
        let reg = BiomeRegistry::load_standard()
            .expect("load_standard should succeed when biome dir exists");
        assert!(reg.len() > 0, "expected at least one biome to load");

        // Sanity: best_fit picks something at any climate query.
        for t in [0.1, 0.5, 0.9, 1.5] {
            for h in [0.1, 0.5, 0.9] {
                let b = reg.best_fit(t, h);
                assert!(b.is_some(), "no biome returned for ({}, {})", t, h);
            }
        }
    }

    #[test]
    fn surface_block_resolves_known_block() {
        let json = r#"{
            "type": "minecraft:biome",
            "_id": "test:rocky",
            "temperature": 0.5,
            "downfall": 0.5,
            "surface_block": "limestone",
            "subsurface_block": "marble"
        }"#;
        let b: BiomeDef = serde_json::from_str(json).unwrap();
        assert_eq!(b.resolve_surface_block(), Some(crate::world::blocks::BlockId::Limestone));
        assert_eq!(b.resolve_subsurface_block(), Some(crate::world::blocks::BlockId::Marble));
    }

    #[test]
    fn surface_block_unknown_falls_back_to_stone() {
        let json = r#"{
            "type": "minecraft:biome",
            "_id": "test:weird",
            "temperature": 0.5,
            "downfall": 0.5,
            "surface_block": "vex_milk_pool"
        }"#;
        let b: BiomeDef = serde_json::from_str(json).unwrap();
        // Unknown block should fall back to Stone, not None.
        assert_eq!(b.resolve_surface_block(), Some(crate::world::blocks::BlockId::Stone));
    }

    #[test]
    fn surface_block_absent_returns_none() {
        let json = r#"{
            "type": "minecraft:biome",
            "_id": "test:bare",
            "temperature": 0.5,
            "downfall": 0.5
        }"#;
        let b: BiomeDef = serde_json::from_str(json).unwrap();
        // Absent field => None, signalling "use legacy mapping".
        assert_eq!(b.resolve_surface_block(), None);
        assert_eq!(b.resolve_subsurface_block(), None);
    }
}
