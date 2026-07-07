//! buildscript.rs — a JSON instruction pipeline for the map forge. `builder.rs`'s `CustomMap` is the
//! in-memory editor model; this is a stable, documented **external** format so tooling — including an LLM
//! given the schema below — can author or edit a map without touching the editor UI at all: describe a
//! sequence of operations (place an asset, scatter a cluster, clear an area, set the spawn/theme/size) as
//! JSON, and `apply()` turns it into a real `CustomMap` the game can load and play.
//!
//! Design choices, on purpose:
//!   - **Names, not IDs.** `"kind": "Barrel"`, `"theme": "Volcanic"` — an LLM (or a human) can read and write
//!     this without a lookup table. `builder.rs`'s on-disk `.map` format uses numeric IDs for compactness;
//!     this format optimizes for being *authored*, not stored.
//!   - **Ops are a sequence, not a snapshot.** A document describes how to *build* the map (in order), which
//!     is both more natural to generate incrementally and easier to review as a diff-able script.
//!   - **`export()` round-trips.** Given an existing `CustomMap`, produce the `BuildDoc` that reconstructs
//!     it — so a tool can read the current map, propose a change, and hand back a full script rather than an
//!     ad-hoc patch.
//!   - **Unknown/bad ops don't abort the whole script.** `apply()` collects per-op errors and keeps going,
//!     since an LLM-authored script with one typo'd asset name shouldn't lose every other op in the file.
//!
//! Example document:
//! ```json
//! {
//!   "name": "Ashfall Outpost",
//!   "theme": "Volcanic",
//!   "size": 160,
//!   "spawn": {"x": 160.0, "z": 160.0},
//!   "ops": [
//!     {"op": "place", "kind": "Crate", "x": 140.0, "z": 150.0, "rot": 0.0, "scale": 1.0},
//!     {"op": "scatter", "kind": "Rock", "cx": 180.0, "cz": 170.0, "count": 6, "radius": 5.0, "seed": 7},
//!     {"op": "clear_area", "x": 160.0, "z": 160.0, "radius": 12.0},
//!     {"op": "set_spawn", "x": 158.0, "z": 158.0}
//!   ]
//! }
//! ```

use crate::builder::{AssetKind, CustomMap, Placed};
use crate::level::{MapTheme, Rng};
use serde::{Deserialize, Serialize};

/// One instruction. `#[serde(tag = "op")]` means the JSON discriminant is a field named `"op"` with a value
/// matching the variant name in snake_case (serde's default rename for this pattern needs an explicit
/// rename, spelled out below so the wire format stays predictable regardless of serde's defaults).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BuildOp {
    /// Place one asset at (x,z). `rot` radians, `scale` uniform, `param` the asset's tunable (man-cannon
    /// launch strength; ignored for everything else). All fields but kind/x/z are optional and default
    /// sensibly, so a minimal op is just `{"op":"place","kind":"Tree","x":10.0,"z":5.0}`.
    Place {
        kind: String,
        x: f32,
        z: f32,
        #[serde(default)]
        rot: f32,
        #[serde(default = "one")]
        scale: f32,
        #[serde(default)]
        param: Option<f32>,
    },
    /// A small natural cluster of `count` copies of `kind` scattered within `radius` of (cx,cz), deterministic
    /// in `seed` — the scripted equivalent of the editor's scatter brush.
    Scatter {
        kind: String,
        cx: f32,
        cz: f32,
        #[serde(default = "six")]
        count: u32,
        #[serde(default = "four")]
        radius: f32,
        #[serde(default)]
        seed: u32,
    },
    /// Remove every placed asset within `radius` of (x,z). Useful for carving a landing zone clear before
    /// scattering more detail around it.
    ClearArea { x: f32, z: f32, radius: f32 },
    /// Remove every placed asset of `kind` map-wide (e.g. thin out an over-scattered pass).
    ClearKind { kind: String },
    /// Remove everything placed so far — a hard reset mid-script, rarely needed but cheap to support.
    ClearAll,
    SetSpawn { x: f32, z: f32 },
    SetTheme { theme: String },
    SetSize { size: i32 },
}

fn one() -> f32 {
    1.0
}
fn six() -> u32 {
    6
}
fn four() -> f32 {
    4.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildDoc {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_size")]
    pub size: i32,
    #[serde(default)]
    pub spawn: Option<Spawn>,
    #[serde(default)]
    pub ops: Vec<BuildOp>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Spawn {
    pub x: f32,
    pub z: f32,
}
fn default_name() -> String {
    "Untitled".to_string()
}
fn default_theme() -> String {
    "Urban".to_string()
}
fn default_size() -> i32 {
    160
}

/// Parse a JSON string into a `BuildDoc`. Returns the serde error message on malformed JSON — passed through
/// as-is since it already names the line/column, which is exactly what an LLM re-prompted with the error
/// needs to fix its own output.
pub fn from_json(s: &str) -> Result<BuildDoc, String> {
    serde_json::from_str(s).map_err(|e| e.to_string())
}

pub fn to_json(doc: &BuildDoc) -> String {
    serde_json::to_string_pretty(doc).unwrap_or_default()
}

/// Look up an `AssetKind` by its name (case-insensitive, matching `AssetKind::name()`).
fn asset_by_name(name: &str) -> Option<AssetKind> {
    AssetKind::ALL.into_iter().find(|k| k.name().eq_ignore_ascii_case(name))
}

/// Look up a `MapTheme` by its Debug name (case-insensitive) — e.g. `"volcanic"` → `MapTheme::Volcanic`.
fn theme_by_name(name: &str) -> Option<MapTheme> {
    MapTheme::ALL.into_iter().find(|t| format!("{t:?}").eq_ignore_ascii_case(name))
}

/// Build a fresh `CustomMap` from a document: honors name/theme/size/spawn, then applies `ops` in order.
/// Returns the map plus a list of human-readable per-op problems (unknown asset/theme names, etc.) — the
/// map is still usable even if some ops failed, since one bad line shouldn't sink the whole script.
pub fn apply(doc: &BuildDoc) -> (CustomMap, Vec<String>) {
    let theme = theme_by_name(&doc.theme).unwrap_or(MapTheme::Urban);
    let mut map = CustomMap::new(&doc.name, theme, doc.size);
    if let Some(s) = doc.spawn {
        map.spawn = (s.x, s.z);
    }
    let mut errors = Vec::new();
    apply_ops(&mut map, &doc.ops, &mut errors);
    (map, errors)
}

/// Apply a sequence of ops to an *existing* map (incremental edits, e.g. an LLM iterating on a map a human
/// started in the editor). Errors are appended to `errors` rather than returned, so a caller can batch
/// several `apply_ops` calls and report everything at the end.
pub fn apply_ops(map: &mut CustomMap, ops: &[BuildOp], errors: &mut Vec<String>) {
    let mut rng = Rng::new(0x5EED_1234);
    for (i, op) in ops.iter().enumerate() {
        if let Err(e) = apply_one(map, op, &mut rng) {
            errors.push(format!("op {i} ({}): {e}", op_name(op)));
        }
    }
}

fn op_name(op: &BuildOp) -> &'static str {
    match op {
        BuildOp::Place { .. } => "place",
        BuildOp::Scatter { .. } => "scatter",
        BuildOp::ClearArea { .. } => "clear_area",
        BuildOp::ClearKind { .. } => "clear_kind",
        BuildOp::ClearAll => "clear_all",
        BuildOp::SetSpawn { .. } => "set_spawn",
        BuildOp::SetTheme { .. } => "set_theme",
        BuildOp::SetSize { .. } => "set_size",
    }
}

fn apply_one(map: &mut CustomMap, op: &BuildOp, rng: &mut Rng) -> Result<(), String> {
    match op {
        BuildOp::Place { kind, x, z, rot, scale, param } => {
            let kind = asset_by_name(kind).ok_or_else(|| format!("unknown asset kind '{kind}'"))?;
            let param = param.unwrap_or_else(|| kind.default_param());
            map.placed.push(Placed { kind, x: *x, z: *z, rot: *rot, scale: scale.clamp(0.3, 4.0), param });
            Ok(())
        }
        BuildOp::Scatter { kind, cx, cz, count, radius, seed } => {
            let kind = asset_by_name(kind).ok_or_else(|| format!("unknown asset kind '{kind}'"))?;
            let mut r = Rng::new((*seed as u64) ^ 0xA5C4_77E1);
            for _ in 0..*count {
                let a = r.f32() * std::f32::consts::TAU;
                let d = r.f32().sqrt() * radius; // sqrt for uniform area density, not clustered at centre
                map.placed.push(Placed {
                    kind,
                    x: cx + a.cos() * d,
                    z: cz + a.sin() * d,
                    rot: r.f32() * std::f32::consts::TAU,
                    scale: 0.8 + r.f32() * 0.4,
                    param: kind.default_param(),
                });
            }
            let _ = rng; // reserved for future ops that want the shared stream instead of a local one
            Ok(())
        }
        BuildOp::ClearArea { x, z, radius } => {
            let r2 = radius * radius;
            map.placed.retain(|p| (p.x - x).powi(2) + (p.z - z).powi(2) > r2);
            Ok(())
        }
        BuildOp::ClearKind { kind } => {
            let kind = asset_by_name(kind).ok_or_else(|| format!("unknown asset kind '{kind}'"))?;
            map.placed.retain(|p| p.kind != kind);
            Ok(())
        }
        BuildOp::ClearAll => {
            map.placed.clear();
            Ok(())
        }
        BuildOp::SetSpawn { x, z } => {
            map.spawn = (*x, *z);
            Ok(())
        }
        BuildOp::SetTheme { theme } => {
            map.theme = theme_by_name(theme).ok_or_else(|| format!("unknown theme '{theme}'"))?;
            Ok(())
        }
        BuildOp::SetSize { size } => {
            map.size = (*size).clamp(64, 240);
            Ok(())
        }
    }
}

/// Dump an existing map as a `BuildDoc` that reconstructs it — one `place` op per placed asset, in order.
/// Round-trips through `apply()`: `apply(&export(&map)).0` reproduces `map` (modulo float rounding in the
/// text form, which is why this returns the doc directly rather than going through JSON and back).
pub fn export(map: &CustomMap) -> BuildDoc {
    let ops = map
        .placed
        .iter()
        .map(|p| BuildOp::Place { kind: p.kind.name().to_string(), x: p.x, z: p.z, rot: p.rot, scale: p.scale, param: Some(p.param) })
        .collect();
    BuildDoc { name: map.name.clone(), theme: format!("{:?}", map.theme), size: map.size, spawn: Some(Spawn { x: map.spawn.0, z: map.spawn.1 }), ops }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_document_builds_a_map() {
        let json = r#"{
            "name": "Test Outpost",
            "theme": "Volcanic",
            "size": 128,
            "spawn": {"x": 100.0, "z": 100.0},
            "ops": [
                {"op": "place", "kind": "Crate", "x": 10.0, "z": 20.0},
                {"op": "scatter", "kind": "Rock", "cx": 50.0, "cz": 50.0, "count": 5, "radius": 6.0, "seed": 3}
            ]
        }"#;
        let doc = from_json(json).expect("valid JSON should parse");
        let (map, errors) = apply(&doc);
        assert!(errors.is_empty(), "a well-formed script shouldn't report errors: {errors:?}");
        assert_eq!(map.name, "Test Outpost");
        assert_eq!(map.theme, MapTheme::Volcanic);
        assert_eq!(map.size, 128);
        assert_eq!(map.spawn, (100.0, 100.0));
        assert_eq!(map.placed.len(), 6, "1 place + 5 scattered rocks");
        assert_eq!(map.placed[0].kind, AssetKind::Crate);
        assert!(map.placed[1..].iter().all(|p| p.kind == AssetKind::Rock));
    }

    #[test]
    fn scatter_respects_radius() {
        let json = r#"{"ops":[{"op":"scatter","kind":"Bush","cx":0.0,"cz":0.0,"count":40,"radius":5.0,"seed":1}]}"#;
        let doc = from_json(json).unwrap();
        let (map, errors) = apply(&doc);
        assert!(errors.is_empty());
        for p in &map.placed {
            let d = (p.x * p.x + p.z * p.z).sqrt();
            assert!(d <= 5.01, "scattered asset landed outside its radius: d={d}");
        }
    }

    #[test]
    fn clear_area_only_removes_within_radius() {
        let mut map = CustomMap::new("t", MapTheme::Urban, 128);
        map.placed.push(Placed { kind: AssetKind::Crate, x: 0.0, z: 0.0, rot: 0.0, scale: 1.0, param: 0.0 });
        map.placed.push(Placed { kind: AssetKind::Crate, x: 50.0, z: 50.0, rot: 0.0, scale: 1.0, param: 0.0 });
        let mut errors = Vec::new();
        apply_ops(&mut map, &[BuildOp::ClearArea { x: 0.0, z: 0.0, radius: 5.0 }], &mut errors);
        assert!(errors.is_empty());
        assert_eq!(map.placed.len(), 1, "only the in-radius asset should be removed");
        assert!((map.placed[0].x - 50.0).abs() < 1e-6);
    }

    #[test]
    fn unknown_asset_name_reports_an_error_but_doesnt_abort_the_script() {
        let json = r#"{"ops":[
            {"op":"place","kind":"NotARealThing","x":0.0,"z":0.0},
            {"op":"place","kind":"Crate","x":1.0,"z":1.0}
        ]}"#;
        let doc = from_json(json).unwrap();
        let (map, errors) = apply(&doc);
        assert_eq!(errors.len(), 1, "the bad op should report exactly one error");
        assert!(errors[0].contains("NotARealThing"));
        assert_eq!(map.placed.len(), 1, "the good op after it should still have applied");
    }

    #[test]
    fn export_round_trips_through_apply() {
        let mut original = CustomMap::new("Roundtrip", MapTheme::Meadow, 176);
        original.spawn = (12.0, 34.0);
        original.placed.push(Placed { kind: AssetKind::Pine, x: 5.0, z: 6.0, rot: 1.2, scale: 1.4, param: 0.0 });
        original.placed.push(Placed { kind: AssetKind::ManCannon, x: 7.0, z: 8.0, rot: 0.0, scale: 1.0, param: 12.0 });
        let doc = export(&original);
        // the doc itself should be valid JSON (this is the format a tool would actually see)
        let json = to_json(&doc);
        let parsed = from_json(&json).expect("exported doc should round-trip through JSON text");
        let (rebuilt, errors) = apply(&parsed);
        assert!(errors.is_empty());
        assert_eq!(rebuilt.name, original.name);
        assert_eq!(rebuilt.theme, original.theme);
        assert_eq!(rebuilt.size, original.size);
        assert_eq!(rebuilt.spawn, original.spawn);
        assert_eq!(rebuilt.placed.len(), original.placed.len());
        for (a, b) in rebuilt.placed.iter().zip(original.placed.iter()) {
            assert_eq!(a.kind, b.kind);
            assert!((a.x - b.x).abs() < 1e-4 && (a.z - b.z).abs() < 1e-4);
            assert!((a.param - b.param).abs() < 1e-4);
        }
    }

    #[test]
    fn malformed_json_reports_a_useful_error_instead_of_panicking() {
        let err = from_json("{ this is not json").unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn set_theme_and_set_size_and_clear_all_work() {
        let mut map = CustomMap::new("t", MapTheme::Urban, 128);
        map.placed.push(Placed { kind: AssetKind::Tree, x: 0.0, z: 0.0, rot: 0.0, scale: 1.0, param: 0.0 });
        let mut errors = Vec::new();
        apply_ops(
            &mut map,
            &[BuildOp::SetTheme { theme: "Ice".to_string() }, BuildOp::SetSize { size: 999 }, BuildOp::ClearAll],
            &mut errors,
        );
        assert!(errors.is_empty());
        assert_eq!(map.theme, MapTheme::Ice);
        assert_eq!(map.size, 240, "size should clamp to the same 64..240 range the editor uses");
        assert!(map.placed.is_empty());
    }
}
