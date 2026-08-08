//! Balance/content definitions for Echelon.
//!
//! Everything the simulation knows about units comes from RON files parsed
//! into these types. All numbers are integers (milli-units where fractional
//! precision is needed) so the files contain no floats and the load is
//! bit-reproducible everywhere. Rosters and tech requirements were
//! reconstructed from the Scion Custom Races mod data; damage numbers are
//! placeholder balance (see BALANCE notes in README).

use serde::Deserialize;
use std::collections::BTreeMap;

pub const TICKS_PER_SEC: i32 = 22; // 22.4 in spirit; integer for determinism

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Worker,
    Unit,
    Structure,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mover {
    Ground,
    Air,
    None,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Attr {
    Light,
    Armored,
    Biological,
    Mechanical,
    Massive,
    Psionic,
    Structure,
}

impl Attr {
    pub fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct WeaponDef {
    pub dmg: i32,
    #[serde(default)]
    pub bonus: i32,
    #[serde(default)]
    pub bonus_vs: Option<Attr>,
    /// range in milli-tiles (e.g. 5000 = 5 tiles)
    pub range_m: i32,
    /// cooldown in ticks
    pub cd: i32,
    #[serde(default = "yes")]
    pub ground: bool,
    #[serde(default)]
    pub air: bool,
    /// splash radius in milli-tiles, 0 = single target
    #[serde(default)]
    pub splash_m: i32,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize, Clone, Debug)]
pub struct SpawnerDef {
    /// stream name, e.g. "Worm" / "Grub" / "Wasp"
    pub kind: String,
    pub max: i32,
    pub regen_s: i32,
}

#[derive(Deserialize, Clone, Debug)]
pub struct UnitDef {
    pub name: String,
    pub kind: Kind,
    #[serde(default)]
    pub mv: Option<Mover>,
    pub hp: i32,
    #[serde(default)]
    pub sh: i32,
    #[serde(default)]
    pub armor: i32,
    #[serde(default)]
    pub energy: i32,
    #[serde(default)]
    pub attrs: Vec<Attr>,
    /// (alloy, flux)
    pub cost: (i32, i32),
    /// supply cost in half-units (Raptor = 3 => 1.5 supply)
    #[serde(default)]
    pub supply2: i32,
    /// supply provided in half-units
    #[serde(default)]
    pub provides2: i32,
    pub build_s: i32,
    /// milli-tiles per second; 0 => kind default
    #[serde(default)]
    pub speed_m: i32,
    /// sight in milli-tiles; 0 => 9000
    #[serde(default)]
    pub sight_m: i32,
    #[serde(default)]
    pub wpn: Option<WeaponDef>,
    /// tech requirements: structure names, or "@uplinkN" (N uploaders linked
    /// to the training facility — the Genetron configuration system)
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub trains: Vec<String>,
    #[serde(default)]
    pub builds: Vec<String>,
    /// parallel production slots (Keiron Citadel materializes 3 at once)
    #[serde(default)]
    pub parallel: i32,
    #[serde(default)]
    pub spawner: Option<SpawnerDef>,
    /// spawner stream this unit consumes when trained ("Worm"/"Grub"/"Wasp")
    #[serde(default)]
    pub consumes: Option<String>,
    #[serde(default)]
    pub townhall: bool,
    /// must be placed on a flux geyser; enables flux harvesting
    #[serde(default)]
    pub gas: bool,
    /// can Link to a production facility (Genetron Uploader)
    #[serde(default)]
    pub uploader: bool,
    /// two units hatch per training (Xayid Raptor)
    #[serde(default)]
    pub twin: bool,
    /// vertical thickness in levels; 0 = auto (units 1, structures 2)
    #[serde(default)]
    pub depth: i32,
    /// flyers: air layer index 0..2 the unit's base occupies (capitals
    /// with depth 2 fill fly_layer .. fly_layer+1)
    #[serde(default)]
    pub fly_layer: i32,
    /// ability ids from assets/data/abilities.ron
    #[serde(default)]
    pub abil: Vec<String>,
    /// reveals cloaked enemies within sight for its owner
    #[serde(default)]
    pub detector: bool,
    /// structure footprint side in tiles; 0 => 3 for structures
    #[serde(default)]
    pub footprint: i32,
}

impl UnitDef {
    pub fn mover(&self) -> Mover {
        match self.mv {
            Some(m) => m,
            None => match self.kind {
                Kind::Structure => Mover::None,
                _ => Mover::Ground,
            },
        }
    }
    pub fn attr_mask(&self) -> u16 {
        let mut m = 0u16;
        for a in &self.attrs {
            m |= a.bit();
        }
        if self.kind == Kind::Structure {
            m |= Attr::Structure.bit();
        }
        m
    }
    pub fn speed_milli(&self) -> i32 {
        if self.speed_m != 0 {
            self.speed_m
        } else {
            match self.kind {
                Kind::Worker => 2800,
                _ => 2800,
            }
        }
    }
    pub fn sight_milli(&self) -> i32 {
        if self.sight_m != 0 {
            self.sight_m
        } else {
            9000
        }
    }
    pub fn fp(&self) -> i32 {
        if self.kind != Kind::Structure {
            0
        } else if self.footprint != 0 {
            self.footprint
        } else {
            3
        }
    }
    pub fn build_ticks(&self) -> i32 {
        self.build_s * TICKS_PER_SEC
    }
    pub fn depth_eff(&self) -> u8 {
        if self.depth > 0 {
            self.depth as u8
        } else if self.kind == Kind::Structure {
            2
        } else {
            1
        }
    }
}

/// How a race erects structures.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum BuildStyle {
    /// Worker must stay on-site (Genetron)
    Attend,
    /// Worker starts it and is freed; it self-completes (Keiron shaping)
    Auto,
    /// Worker is consumed into the structure (Xayid mutation)
    Consume,
}

#[derive(Deserialize, Clone, Debug)]
pub struct RaceDef {
    pub name: String,
    pub build_style: BuildStyle,
    /// innate life regen, milli-hp per second (Xayid & Keiron)
    #[serde(default)]
    pub life_regen_m: i32,
    pub units: Vec<UnitDef>,
}

pub type DefId = u16;

#[derive(Clone)]
pub struct DataPack {
    pub races: Vec<RaceDef>,
    /// flattened defs: (race index, def)
    pub defs: Vec<(usize, UnitDef)>,
    pub by_name: BTreeMap<String, DefId>,
    pub abilities: Vec<AbilityDef>,
    pub abil_by_id: BTreeMap<String, u16>,
    /// hash of the raw source bytes — part of the lockstep handshake
    pub hash: u64,
}

impl DataPack {
    pub fn from_sources(named_sources: &[(&str, &str)]) -> Result<DataPack, String> {
        DataPack::from_sources_with_abilities(named_sources, "[]")
    }

    pub fn from_sources_with_abilities(
        named_sources: &[(&str, &str)],
        abilities_src: &str,
    ) -> Result<DataPack, String> {
        let mut h = Fnv::new();
        let mut races = Vec::new();
        for (name, src) in named_sources {
            h.str(name);
            h.str(src);
            let r: RaceDef = ron::from_str(src)
                .map_err(|e| format!("parse {}: {}", name, e))?;
            races.push(r);
        }
        let mut defs = Vec::new();
        let mut by_name = BTreeMap::new();
        for (ri, r) in races.iter().enumerate() {
            for u in &r.units {
                if by_name.contains_key(&u.name) {
                    return Err(format!("duplicate unit name {}", u.name));
                }
                by_name.insert(u.name.clone(), defs.len() as DefId);
                defs.push((ri, u.clone()));
            }
        }
        // referential integrity
        for (ri, u) in &defs {
            for list in [&u.requires, &u.trains, &u.builds] {
                for n in list {
                    if n.starts_with('@') {
                        continue;
                    }
                    if !by_name.contains_key(n) {
                        return Err(format!("{}: unknown reference {}", u.name, n));
                    }
                }
            }
            if let Some(c) = &u.consumes {
                let ok = races[*ri].units.iter().any(|x| match &x.spawner {
                    Some(s) => &s.kind == c,
                    None => false,
                });
                if !ok {
                    return Err(format!("{}: no spawner stream {}", u.name, c));
                }
            }
        }
        h.str(abilities_src);
        let abilities: Vec<AbilityDef> = ron::from_str(abilities_src)
            .map_err(|e| format!("parse abilities: {}", e))?;
        let mut abil_by_id = BTreeMap::new();
        for (i, a) in abilities.iter().enumerate() {
            if abil_by_id.insert(a.id.clone(), i as u16).is_some() {
                return Err(format!("duplicate ability id {}", a.id));
            }
            for op in &a.ops {
                if let Op::Summon { unit, .. } = op {
                    if !by_name.contains_key(unit) {
                        return Err(format!("ability {}: unknown summon {}", a.id, unit));
                    }
                }
            }
        }
        for (_, u) in &defs {
            for aid in &u.abil {
                if !abil_by_id.contains_key(aid) {
                    return Err(format!("{}: unknown ability {}", u.name, aid));
                }
            }
        }
        Ok(DataPack {
            races,
            defs,
            by_name,
            abilities,
            abil_by_id,
            hash: h.fin(),
        })
    }

    pub fn load_dir(dir: &std::path::Path, files: &[&str]) -> Result<DataPack, String> {
        let mut owned = Vec::new();
        for f in files {
            let p = dir.join(f);
            let s = std::fs::read_to_string(&p)
                .map_err(|e| format!("read {}: {}", p.display(), e))?;
            owned.push(((*f).to_string(), s));
        }
        let refs: Vec<(&str, &str)> =
            owned.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let abil_path = dir.parent().unwrap().join("abilities.ron");
        let abil = std::fs::read_to_string(&abil_path).unwrap_or_else(|_| "[]".into());
        DataPack::from_sources_with_abilities(&refs, &abil)
    }

    pub fn id(&self, name: &str) -> DefId {
        *self
            .by_name
            .get(name)
            .unwrap_or_else(|| panic!("unknown def {}", name))
    }
    pub fn def(&self, id: DefId) -> &UnitDef {
        &self.defs[id as usize].1
    }
    pub fn race_of(&self, id: DefId) -> usize {
        self.defs[id as usize].0
    }
    pub fn race_index(&self, name: &str) -> Option<usize> {
        self.races.iter().position(|r| r.name == name)
    }
}

// ---------------- abilities: paperbound-style effect templating ----------------
// An ability is a trigger kind plus an ordered op list. Ops are the whole
// effect vocabulary; new abilities are data, not code.

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum AbilKind {
    /// fires by itself on a cadence when its condition holds
    Auto,
    /// player-cast via Cmd::Cast
    Active,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum AbilTarget {
    SelfT,
    Ally,
    Enemy,
    Point,
}

#[derive(Deserialize, Clone, Debug)]
pub enum Op {
    Heal(i32),
    ShieldGive(i32),
    EnergyGive(i32),
    SelfDamage(i32),
    KillSelf,
    /// r_m 0 = primary target only
    AreaDamage { r_m: i32, dmg: i32, #[serde(default)] bonus: i32, #[serde(default)] vs: Option<Attr> },
    ChainDamage { dmg: i32, jumps: i32, r_m: i32 },
    Dot { dps: i32, dur_s: i32 },
    Stun { r_m: i32, dur_s: i32 },
    /// pct 1000 = x1.0; r_m 0 = target/self, else allies around the point
    SpeedMul { pct: i32, dur_s: i32, #[serde(default)] r_m: i32 },
    AtkSpeedMul { pct: i32, dur_s: i32, #[serde(default)] r_m: i32 },
    DmgTakenMul { pct: i32, dur_s: i32, #[serde(default)] r_m: i32 },
    RegenHp { hps: i32, dur_s: i32 },
    Cloak { dur_s: i32 },
    Blink { max_m: i32 },
    Summon { unit: String, count: i32, life_s: i32 },
}

#[derive(Deserialize, Clone, Debug)]
pub struct AbilityDef {
    pub id: String,
    pub name: String,
    pub kind: AbilKind,
    pub target: AbilTarget,
    #[serde(default)]
    pub energy: i32,
    #[serde(default)]
    pub cd_s: i32,
    #[serde(default)]
    pub range_m: i32,
    /// Auto ally filter: only allies with this attribute qualify
    #[serde(default)]
    pub ally_attr: Option<Attr>,
    /// Auto self trigger: fire when own hp below this percent (0 = ignore)
    #[serde(default)]
    pub self_hp_below: i32,
    pub ops: Vec<Op>,
}

/// A playable map: two aligned character layers plus metadata.
/// height: '.'=low  '^'=high  '/'=ramp  '#'=void/cliff-rock
/// things: '.'=none 'a'=alloy 'f'=flux geyser '1'/'2'=start locations
#[derive(Deserialize, Clone, Debug)]
pub struct MapDef {
    pub name: String,
    pub tileset: String,
    pub height: Vec<String>,
    pub things: Vec<String>,
    #[serde(default)]
    pub ramps: Vec<String>,
    #[serde(default)]
    pub bridges: Vec<String>,
    #[serde(default = "d_amount")]
    pub alloy_amount: i32,
    #[serde(default = "d_amount")]
    pub flux_amount: i32,
}
fn d_amount() -> i32 {
    20000
}

#[derive(Deserialize, Clone, Copy, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Renderer palette + light rig for one biome. Pure client-side data —
/// never part of the sim pack hash.
#[derive(Deserialize, Clone, Debug)]
pub struct TilesetDef {
    pub name: String,
    pub low: Rgb,
    pub low_var: Rgb,
    pub high: Rgb,
    pub high_var: Rgb,
    pub ramp: Rgb,
    pub bridge: Rgb,
    pub cliff: Rgb,
    pub rock: Rgb,
    /// 0-100: albedo grime/noise strength
    pub grit: i32,
    /// milli-normalized sun direction (x right, y down, z up)
    pub sun_dir: (i32, i32, i32),
    pub sun: Rgb,
    pub ambient: Rgb,
    pub rim: Rgb,
    /// Blinn exponent (low = broad, worn highlight) and strength percent
    pub spec_pow: i32,
    pub spec: i32,
    pub alloy_glow: Rgb,
    pub flux_glow: Rgb,
}

impl TilesetDef {
    pub fn load(dir: &std::path::Path, name: &str) -> Result<TilesetDef, String> {
        let p = dir.join(format!("{}.ron", name));
        let s = std::fs::read_to_string(&p)
            .map_err(|e| format!("read {}: {}", p.display(), e))?;
        ron::from_str(&s).map_err(|e| format!("parse {}: {}", p.display(), e))
    }
}

impl MapDef {
    pub fn load(dir: &std::path::Path, name: &str) -> Result<MapDef, String> {
        let p = dir.join(format!("{}.ron", name));
        let s = std::fs::read_to_string(&p)
            .map_err(|e| format!("read {}: {}", p.display(), e))?;
        ron::from_str(&s).map_err(|e| format!("parse {}: {}", p.display(), e))
    }
}

/// FNV-1a 64 — the deterministic hash used for the pack and for sim state.
pub struct Fnv(pub u64);
impl Fnv {
    pub fn new() -> Fnv {
        Fnv(0xcbf29ce484222325)
    }
    pub fn u8(&mut self, b: u8) {
        self.0 ^= b as u64;
        self.0 = self.0.wrapping_mul(0x100000001b3);
    }
    pub fn u64(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.u8(b);
        }
    }
    pub fn i64(&mut self, v: i64) {
        self.u64(v as u64);
    }
    pub fn u32(&mut self, v: u32) {
        self.u64(v as u64);
    }
    pub fn i32(&mut self, v: i32) {
        self.u64(v as u32 as u64);
    }
    pub fn str(&mut self, s: &str) {
        for b in s.as_bytes() {
            self.u8(*b);
        }
        self.u8(0xff);
    }
    pub fn fin(&self) -> u64 {
        self.0
    }
}
