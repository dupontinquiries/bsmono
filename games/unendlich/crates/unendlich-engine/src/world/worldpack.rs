//! worldpack.rs — data-driven world packs for unendlich.
//!
//! A world pack customises how a world GENERATES — it never ships a map.
//! That's a hard policy, not a missing feature: unendlich worlds are infinite,
//! deterministic functions of a seed, and everything multiplayer (rollback,
//! region digests, `world_authority` command sourcing) leans on every peer
//! being able to regenerate identical terrain from `(seed, ruleset)`. A fixed
//! voxel map would break that contract — including for server sub-lobbies. So
//! a pack is *parameters and procedural directives* only, and [`validate`]
//! actively REJECTS documents that smuggle in voxel/heightmap payloads.
//!
//! What a pack MAY carry:
//!   • `biomes`      — extra biome definitions in the existing
//!                     `assets/biomes/…` JSON format (the registry the engine
//!                     already hot-loads; see `biome_loader.rs`, whose header
//!                     literally anticipates "AI/storytelling biome
//!                     generation (system writes a JSON…)").
//!   • `climate`     — global scalar offsets: temperature/humidity deltas and
//!                     a sea-level shift, applied on top of the field model.
//!   • `field_mods`  — procedural FIELD BRUSHES (`field_brush.rs` vocabulary:
//!                     radius/peak/falloff) aimed at *procedural anchors* —
//!                     the world origin, the spawn column, or a seeded scatter
//!                     ring — never absolute hand-placed voxels. Same brush
//!                     math as elemental items; a pack is "weather-scale
//!                     elements applied at genesis".
//!
//! The same document type is what the LLM terrain endpoint
//! (`terrain_llm.rs`) accepts at runtime, so "a designer wrote a pack" and
//! "a model proposed a terrain change" go through one schema, one validator,
//! one policy.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Fields a brush may perturb. Mirrors the scalar fields the interaction
/// layer already exposes to `field_brush` writes.
pub const FIELDS: &[&str] = &["temperature", "humidity", "pressure", "water_depth"];

/// Procedural anchor for a field mod — the *whole point* of the format.
/// Nothing here can encode "this exact voxel": positions are derived from the
/// world seed at generation time, so every peer lands the same brushes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "at", rename_all = "snake_case")]
pub enum Anchor {
    /// The world origin column.
    Origin,
    /// The (seed-derived) spawn column.
    Spawn,
    /// `count` brushes scattered on a ring `r_min..r_max` around the origin,
    /// positions drawn from `(world_seed, salt)` — deterministic everywhere.
    ScatterRing { r_min: f32, r_max: f32, count: u32, salt: u32 },
}

/// One procedural field perturbation, in `field_brush` vocabulary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldMod {
    /// Which scalar field (see [`FIELDS`]).
    pub field: String,
    #[serde(flatten)]
    pub anchor: Anchor,
    /// Brush radius in cells.
    pub radius: f32,
    /// Signed peak delta (positive injects, negative drains).
    pub peak: f32,
    /// "constant" | "linear" | "smooth" (default).
    #[serde(default = "default_falloff")]
    pub falloff: String,
}

fn default_falloff() -> String { "smooth".into() }

/// Global climate offsets — small numbers, big consequences.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Climate {
    #[serde(default)]
    pub temperature_offset: f32,
    #[serde(default)]
    pub humidity_offset: f32,
    #[serde(default)]
    pub sea_level_delta: i32,
}

/// The pack document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorldPack {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub blurb: String,
    /// Inline biome definitions, forwarded verbatim to the biome registry
    /// (Minecraft-format JSON, keyed by namespaced id). Kept as opaque values
    /// exactly like `biome_loader` stores unconsumed fields.
    #[serde(default)]
    pub biomes: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub climate: Climate,
    #[serde(default)]
    pub field_mods: Vec<FieldMod>,
}

/// Keys whose presence marks a smuggled fixed-map payload. Checked
/// recursively; hitting one fails validation with the policy message.
const FORBIDDEN_KEYS: &[&str] = &[
    "voxels", "blocks", "heightmap", "chunks", "tiles", "map", "schematic",
];

pub const NO_FIXED_MAPS_POLICY: &str =
    "world packs are parametric by design: unendlich terrain must regenerate \
     identically from (seed, pack) on every peer — fixed voxel/height data is \
     rejected, including for server sub-lobbies";

/// Validate a pack. Returns problems (empty = valid). Per-item, not fail-fast,
/// so an LLM-authored pack with one bad mod still reports everything at once.
pub fn validate(pack: &WorldPack) -> Vec<String> {
    let mut probs = Vec::new();
    if pack.id.is_empty() {
        probs.push("pack.id is empty".into());
    }
    for (i, m) in pack.field_mods.iter().enumerate() {
        if !FIELDS.contains(&m.field.as_str()) {
            probs.push(format!("field_mods[{i}].field '{}' not in {:?}", m.field, FIELDS));
        }
        if !(0.5..=64.0).contains(&m.radius) {
            probs.push(format!("field_mods[{i}].radius {} outside 0.5..=64", m.radius));
        }
        if !m.peak.is_finite() || m.peak.abs() > 1000.0 {
            probs.push(format!("field_mods[{i}].peak {} unreasonable", m.peak));
        }
        if !["constant", "linear", "smooth"].contains(&m.falloff.as_str()) {
            probs.push(format!("field_mods[{i}].falloff '{}' unknown", m.falloff));
        }
        if let Anchor::ScatterRing { r_min, r_max, count, .. } = m.anchor {
            if r_min < 0.0 || r_max <= r_min {
                probs.push(format!("field_mods[{i}]: ring {r_min}..{r_max} degenerate"));
            }
            if count == 0 || count > 4096 {
                probs.push(format!("field_mods[{i}]: ring count {count} outside 1..=4096"));
            }
        }
    }
    if !(-64..=64).contains(&pack.climate.sea_level_delta) {
        probs.push(format!("climate.sea_level_delta {} outside -64..=64",
                           pack.climate.sea_level_delta));
    }
    // policy: no fixed-map payloads anywhere in the document
    let as_value = serde_json::to_value(pack).unwrap_or(serde_json::Value::Null);
    scan_forbidden(&as_value, &mut probs, "");
    for (bid, bdef) in &pack.biomes {
        scan_forbidden(bdef, &mut probs, &format!("biomes.{bid}"));
        if !bid.contains(':') {
            probs.push(format!("biomes.{bid}: use a namespaced id like 'mypack:{bid}'"));
        }
    }
    probs
}

fn scan_forbidden(v: &serde_json::Value, probs: &mut Vec<String>, path: &str) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, vv) in map {
                if FORBIDDEN_KEYS.contains(&k.as_str()) {
                    probs.push(format!("{path}.{k}: {NO_FIXED_MAPS_POLICY}"));
                }
                scan_forbidden(vv, probs, &format!("{path}.{k}"));
            }
        }
        serde_json::Value::Array(a) => {
            for (i, vv) in a.iter().enumerate() {
                scan_forbidden(vv, probs, &format!("{path}[{i}]"));
            }
        }
        _ => {}
    }
}

/// Resolve a pack's field mods into concrete `(field, x, z, radius, peak,
/// falloff)` brush placements for THIS world seed. Deterministic: every peer
/// with the same `(seed, pack)` computes the same placements — the property
/// that lets packs exist at all.
pub fn resolve_brushes(
    pack: &WorldPack,
    world_seed: u32,
    spawn_xz: (f32, f32),
) -> Vec<(String, f32, f32, f32, f32, String)> {
    let mut out = Vec::new();
    for m in &pack.field_mods {
        match m.anchor {
            Anchor::Origin => {
                out.push((m.field.clone(), 0.0, 0.0, m.radius, m.peak, m.falloff.clone()));
            }
            Anchor::Spawn => {
                out.push((m.field.clone(), spawn_xz.0, spawn_xz.1, m.radius, m.peak,
                          m.falloff.clone()));
            }
            Anchor::ScatterRing { r_min, r_max, count, salt } => {
                // SplitMix64-style scramble of (seed, salt, i) → angle + radius.
                for i in 0..count {
                    let mut h = (world_seed as u64)
                        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                        ^ ((salt as u64) << 32 | i as u64);
                    h ^= h >> 30; h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
                    h ^= h >> 27; h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
                    h ^= h >> 31;
                    let a = (h & 0xFFFF) as f32 / 65535.0 * std::f32::consts::TAU;
                    let t = ((h >> 16) & 0xFFFF) as f32 / 65535.0;
                    let r = r_min + (r_max - r_min) * t;
                    out.push((m.field.clone(), r * a.cos(), r * a.sin(),
                              m.radius, m.peak, m.falloff.clone()));
                }
            }
        }
    }
    out
}

/// Parse + validate JSON source in one step. Returns the pack alongside its
/// problems; callers decide whether problems are fatal (the endpoint treats
/// any as a rejection, a local load may warn-and-continue on soft ones).
pub fn parse(src: &str) -> Result<(WorldPack, Vec<String>), String> {
    let pack: WorldPack =
        serde_json::from_str(src).map_err(|e| format!("not a world pack: {e}"))?;
    let probs = validate(&pack);
    Ok((pack, probs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> WorldPack {
        serde_json::from_value(serde_json::json!({
            "id": "ashen_reach",
            "name": "Ashen Reach",
            "blurb": "A scorched, dried-out overworld with oasis pockets.",
            "climate": {"temperature_offset": 0.35, "humidity_offset": -0.2,
                        "sea_level_delta": -6},
            "biomes": {
                "ashen_reach:cinder_flats": {"_id": "ashen_reach:cinder_flats",
                    "temperature": 1.8, "downfall": 0.05,
                    "surface_block": "basalt"}
            },
            "field_mods": [
                {"field": "humidity", "at": "spawn", "radius": 24.0, "peak": 3.0},
                {"field": "temperature", "at": "scatter_ring",
                 "r_min": 200.0, "r_max": 900.0, "count": 12, "salt": 7,
                 "radius": 16.0, "peak": 5.0, "falloff": "linear"}
            ]
        })).unwrap()
    }

    #[test]
    fn sample_pack_validates() {
        assert_eq!(validate(&sample()), Vec::<String>::new());
    }

    #[test]
    fn fixed_map_payloads_are_rejected() {
        let mut p = sample();
        p.biomes.insert(
            "ashen_reach:evil".into(),
            serde_json::json!({"_id": "ashen_reach:evil",
                               "heightmap": [[1, 2], [3, 4]]}),
        );
        let probs = validate(&p);
        assert!(probs.iter().any(|s| s.contains("parametric by design")), "{probs:?}");
    }

    #[test]
    fn bad_mods_report_individually() {
        let (_p, probs) = parse(r#"{"id":"x","field_mods":[
            {"field":"lava","at":"origin","radius":900.0,"peak":1.0},
            {"field":"humidity","at":"origin","radius":4.0,"peak":1.0}
        ]}"#).unwrap();
        assert_eq!(probs.len(), 2, "{probs:?}");   // bad field + bad radius; second mod clean
    }

    #[test]
    fn resolve_is_deterministic_and_seed_sensitive() {
        let p = sample();
        let a = resolve_brushes(&p, 1337, (10.0, 20.0));
        let b = resolve_brushes(&p, 1337, (10.0, 20.0));
        assert_eq!(a, b, "same (seed, pack) must resolve identically");
        assert_eq!(a.len(), 1 + 12);
        let c = resolve_brushes(&p, 1338, (10.0, 20.0));
        assert_ne!(a, c, "different seed should move the scatter ring");
        // spawn anchor followed the spawn column
        assert_eq!((a[0].1, a[0].2), (10.0, 20.0));
    }

    #[test]
    fn json_round_trips() {
        let p = sample();
        let s = serde_json::to_string(&p).unwrap();
        let (p2, probs) = parse(&s).unwrap();
        assert!(probs.is_empty());
        assert_eq!(p2.field_mods, p.field_mods);
        assert_eq!(p2.climate, p.climate);
    }
}
