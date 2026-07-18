//! data.rs — the Foldworks faction book, loaded from the SAME files the
//! Python game reads: `rts_data.json` plus any `expansions/*.json` packs'
//! `rts_factions` (merged after, exactly like `expansions.apply_rts`).
//!
//! Field names mirror rts.py's spec dicts one-to-one so the two engines can
//! never drift on vocabulary. Unknown fields are preserved-by-ignoring
//! (serde default), matching Python's dict tolerance.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

fn default_mult() -> f64 {
    1.0
}

#[derive(Debug, Clone, Deserialize)]
pub struct StructureSpec {
    pub name: String,
    #[serde(default)]
    pub cost: i64,
    pub hp: f64,
    #[serde(default)]
    pub r: f64,
    #[serde(default)]
    pub build: f64,
    #[serde(default)]
    pub makes: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub researches: Vec<String>,
    #[serde(default)]
    pub tier: i32,
    /// research gate: the upgrade key that must be finished first (rts.py's
    /// `requires_research`)
    #[serde(default)]
    pub requires_research: Option<String>,
    #[serde(default)]
    pub supply: i64,
    #[serde(default)]
    pub detector: bool,
    /// data-driven tower profile — any structure with tower_dmg > 0 fires
    #[serde(default)]
    pub tower_dmg: f64,
    #[serde(default)]
    pub tower_rng: Option<f64>,
    #[serde(default)]
    pub tower_cool: Option<f64>,
}

impl StructureSpec {
    /// rts.py: the classic Watch Spire keeps 22/240/0.8 as defaults; other
    /// structures fire only if they declare tower_dmg.
    pub fn tower_profile(&self, kind: &str) -> Option<(f64, f64, f64)> {
        let dmg = if self.tower_dmg > 0.0 {
            self.tower_dmg
        } else if kind == "spire" {
            22.0
        } else {
            return None;
        };
        Some((
            dmg,
            self.tower_rng.unwrap_or(240.0),
            self.tower_cool.unwrap_or(0.8),
        ))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct UnitSpec {
    pub name: String,
    pub cost: i64,
    pub hp: f64,
    pub dmg: f64,
    pub spd: f64,
    pub rng: f64,
    #[serde(default)]
    pub r: f64,
    #[serde(default)]
    pub worker: bool,
    #[serde(default)]
    pub build_t: f64,
    #[serde(default)]
    pub tier: i32,
    #[serde(default)]
    pub requires_research: Option<String>,
    /// The battler card whose folded model this unit wears on the field.
    #[serde(default)]
    pub card: Option<String>,
    /// rts.py `_supply_free`: `spec.get("supply", 1)`
    #[serde(default = "default_unit_supply")]
    pub supply: i64,
    #[serde(default)]
    pub cloak: bool,
}

fn default_unit_supply() -> i64 {
    1
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Applies {
    Group(String),      // "all" | "army" | "workers"
    Kinds(Vec<String>), // explicit unit kinds
}

#[derive(Debug, Clone, Deserialize)]
pub struct Upgrade {
    pub name: String,
    pub cost: i64,
    #[serde(default)]
    pub time: f64,
    pub stat: String, // maxhp | dmg | spd | rng | harvest
    #[serde(default = "default_mult")]
    pub mult: f64,
    pub applies: Applies,
}

impl Upgrade {
    pub fn hits(&self, kind: &str, worker: bool) -> bool {
        match &self.applies {
            Applies::Group(g) => match g.as_str() {
                "all" => true,
                "army" => !worker,
                "workers" => worker,
                _ => false,
            },
            Applies::Kinds(ks) => ks.iter().any(|k| k == kind),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Faction {
    pub name: String,
    #[serde(default)]
    pub blurb: String,
    #[serde(default = "default_mult")]
    pub econ: f64,
    #[serde(default)]
    pub mechanic: Option<String>,
    pub structures: BTreeMap<String, StructureSpec>,
    pub units: BTreeMap<String, UnitSpec>,
    #[serde(default)]
    pub upgrades: BTreeMap<String, Upgrade>,
}

#[derive(Debug, Deserialize)]
struct RtsData {
    factions: BTreeMap<String, Faction>,
}

#[derive(Debug, Deserialize)]
struct Pack {
    #[serde(default)]
    rts_factions: BTreeMap<String, Faction>,
}

/// The faction book: rts_data.json overlaid with pack races, in file order —
/// the merge order `expansions.apply_rts` uses.
pub fn load_factbook(paperbound_dir: &Path) -> Result<BTreeMap<String, Faction>, String> {
    let core = std::fs::read_to_string(paperbound_dir.join("rts_data.json"))
        .map_err(|e| format!("rts_data.json: {e}"))?;
    let core: RtsData =
        serde_json::from_str(&core).map_err(|e| format!("rts_data.json: {e}"))?;
    let mut book = core.factions;
    let expdir = paperbound_dir.join("expansions");
    if let Ok(rd) = std::fs::read_dir(&expdir) {
        let mut paths: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
            .collect();
        paths.sort();
        for p in paths {
            let src = std::fs::read_to_string(&p)
                .map_err(|e| format!("{}: {e}", p.display()))?;
            let pack: Pack =
                serde_json::from_str(&src).map_err(|e| format!("{}: {e}", p.display()))?;
            for (k, f) in pack.rts_factions {
                book.insert(k, f);
            }
        }
    }
    Ok(book)
}
