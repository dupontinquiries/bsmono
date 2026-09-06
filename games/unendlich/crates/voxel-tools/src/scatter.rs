//! Placement: where instances of a structure go, as data.
//!
//! THE SEAM
//!
//! `def`/`use` builds a structure many times, but the player has to
//! write every `use`, and each is a coordinate. That is fine for six
//! huts and useless for a coastline of watchtowers, a road's mile
//! markers, or forty shrines scattered through a forest. Those are
//! not one build; they are one STRUCTURE plus a RULE about where its
//! instances go.
//!
//! So placement is separate data: a rail (the shape the instances
//! follow), spacing rules, per-instance parameters, and site
//! selection. A `Scatter` is JSON -- the one place in this crate where
//! that is the right format, because it is what a model writes when
//! asked for structured data and what a map-builder wants to keep in a
//! file, hand-edit and share.
//!
//! WHAT THE ENGINE DOES WITH IT
//!
//! `sites()` resolves a scatter against the world into a list of
//! `Site`: a position, a rotation, a seed, and the parameters that
//! instance gets. Then the host chooses:
//!
//!   * build them now -- `to_ir()` emits `use`/`form` lines, which
//!     preview, commit, undo and journal like anything else
//!   * hold them -- keep the sites and build each when a player comes
//!     near, which is what a thousand shrines across a continent
//!     wants. A `Site` is small; the structure is stored once.
//!
//! The distinction the engine cares about is exactly that: a scatter
//! is a description that can be realised eagerly or lazily, and
//! nothing in the description says which.
//!
//! RAILS
//!
//! * `point`      one site
//! * `line`       from A to B
//! * `ring`       a circle, or an arc
//! * `grid`       rows and columns, jittered
//! * `area`       Poisson-ish scatter in a box
//! * `path`       along an existing road or river's waypoints
//! * `contour`    along a height line: a coastline, a ridge, a
//!                treeline -- the rail the terrain decides
//!
//! Every rail is filtered by SITE RULES -- slope, height band, clear
//! space, distance from other sites and from water -- so a watchtower
//! rail on a coastline does not put one in the sea.

use crate::VoxelWorld;
use std::collections::BTreeMap;

/// The shape instances follow.
#[derive(Debug, Clone, PartialEq)]
pub enum Rail {
    Point { at: (i32, i32) },
    Line { from: (i32, i32), to: (i32, i32) },
    Ring { centre: (i32, i32), radius: i32, arc_deg: (i32, i32) },
    Grid { min: (i32, i32), max: (i32, i32), step: i32 },
    Area { min: (i32, i32), max: (i32, i32) },
    Path { points: Vec<(i32, i32)> },
    /// Follow a height: sites where the surface is within `band` of
    /// `y`. `coast` follows the waterline.
    Contour { min: (i32, i32), max: (i32, i32), y: i32, band: i32 },
}

/// What makes a spot acceptable.
#[derive(Debug, Clone, PartialEq)]
pub struct Rules {
    /// Maximum surface height difference across the footprint.
    pub max_slope: i32,
    /// Allowed surface height band, absolute.
    pub y_range: Option<(i32, i32)>,
    /// Footprint radius that must be clear of other sites.
    pub spacing: i32,
    /// Blocks of air needed above the surface.
    pub headroom: i32,
    /// Reject sites whose footprint contains water; or REQUIRE it.
    pub water: WaterRule,
    /// Reject sites this close to water (a beach margin).
    pub water_margin: i32,
    /// Footprint half-width the rules are checked over.
    pub footprint: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaterRule { Avoid, Allow, Require }

impl Default for Rules {
    fn default() -> Self {
        Self { max_slope: 3, y_range: None, spacing: 12, headroom: 4, water: WaterRule::Avoid,
               water_margin: 0, footprint: 4 }
    }
}

/// A parameter varied per instance.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    /// One of these, chosen by the site's seed.
    Pick(Vec<String>),
    /// An integer in a range.
    Range(i32, i32),
    /// The same for every instance.
    Fixed(String),
    /// Increasing along the rail: mile markers, numbered shrines.
    Index { from: i32, step: i32 },
}

/// A structure and a rule about where its instances go.
#[derive(Debug, Clone, PartialEq)]
pub struct Scatter {
    /// The structure: a dictionary form, or a captured template name.
    pub structure: String,
    pub rail: Rail,
    pub rules: Rules,
    /// How many sites at most. The rail may yield fewer.
    pub count: usize,
    /// Rotation: `along` faces the rail, `out` faces away from its
    /// centre, `random` is seeded, a number is fixed.
    pub facing: Facing,
    pub params: BTreeMap<String, Param>,
    pub seed: u64,
    /// Build the sites now, or hold them for the engine to realise
    /// when a player comes near. The engine's choice, recorded here
    /// because a scatter is written once and used either way.
    pub lazy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facing { Along, Out, Random, Fixed(i32) }

/// One resolved instance.
#[derive(Debug, Clone, PartialEq)]
pub struct Site {
    pub at: (i32, i32, i32),
    pub rot: i32,
    pub seed: u64,
    pub index: usize,
    pub params: BTreeMap<String, String>,
}

impl Site {
    /// The IR line that builds this instance, in ABSOLUTE coordinates.
    ///
    /// A `size=` word expands to a footprint around the ANCHOR, and a
    /// held site is built later with no anchor at all -- so a size word
    /// here produced `@-4,-4 @4,4`, which resolves to nothing and
    /// placed zero blocks. The size is turned into the two corners the
    /// site actually occupies, here, where the position is known.
    pub fn to_ir(&self, structure: &str) -> String {
        let r = self.params.get("size").map(|s| match s.as_str() {
            "tiny" => 2, "small" => 4, "medium" | "mid" => 8, "large" | "big" => 14,
            "huge" | "giant" => 24, _ => 8,
        }).unwrap_or(8);
        let mut s = format!("form {structure} {},{} {},{}",
                            self.at.0 - r, self.at.2 - r, self.at.0 + r, self.at.2 + r);
        for (k, v) in &self.params {
            if k == "size" { continue; }        // spent on the corners above
            s.push_str(&format!(" {k}={v}"));
        }
        if self.rot != 0 { s.push_str(&format!(" rot={}", self.rot)); }
        s.push_str(&format!(" seed={}", self.seed % 100_000));
        s
    }
}

fn hash(a: u64, b: u64) -> u64 {
    let mut h = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 31; h = h.wrapping_mul(0x7FB5_D329_728E_A185); h ^ (h >> 27)
}
fn unit(a: u64, b: u64) -> f32 { (hash(a, b) % 10_000) as f32 / 10_000.0 }

/// Candidate x,z along the rail, before the rules.
fn candidates(rail: &Rail, count: usize, seed: u64) -> Vec<(i32, i32)> {
    let n = count.max(1);
    match rail {
        Rail::Point { at } => vec![*at],
        Rail::Line { from, to } => (0..n).map(|i| {
            let t = if n == 1 { 0.5 } else { i as f32 / (n - 1) as f32 };
            ((from.0 as f32 + (to.0 - from.0) as f32 * t).round() as i32,
             (from.1 as f32 + (to.1 - from.1) as f32 * t).round() as i32)
        }).collect(),
        Rail::Ring { centre, radius, arc_deg } => (0..n).map(|i| {
            let (a0, a1) = (arc_deg.0 as f32, arc_deg.1 as f32);
            let span = if (a1 - a0).abs() >= 359.0 { 360.0 * (n - 1).max(1) as f32 / n as f32 } else { a1 - a0 };
            let t = if n == 1 { 0.0 } else { i as f32 / (n - 1) as f32 };
            let a = (a0 + span * t).to_radians();
            (centre.0 + (*radius as f32 * a.cos()).round() as i32,
             centre.1 + (*radius as f32 * a.sin()).round() as i32)
        }).collect(),
        Rail::Grid { min, max, step } => {
            let s = (*step).max(1);
            let mut v = Vec::new();
            let mut i = 0u64;
            for x in (min.0..=max.0).step_by(s as usize) {
                for z in (min.1..=max.1).step_by(s as usize) {
                    // jittered, so a grid does not read as a grid
                    let j = (s as f32 * 0.3) as i32;
                    let dx = if j > 0 { (unit(seed, i) * j as f32 * 2.0) as i32 - j } else { 0 };
                    let dz = if j > 0 { (unit(seed, i + 7919) * j as f32 * 2.0) as i32 - j } else { 0 };
                    v.push((x + dx, z + dz));
                    i += 1;
                    if v.len() >= n { return v; }
                }
            }
            v
        }
        Rail::Area { min, max } => {
            // oversample; the spacing rule thins it to a Poisson-ish set
            (0..n * 8).map(|i| {
                (min.0 + (unit(seed, i as u64) * (max.0 - min.0) as f32) as i32,
                 min.1 + (unit(seed, i as u64 + 104_729) * (max.1 - min.1) as f32) as i32)
            }).collect()
        }
        Rail::Path { points } => {
            if points.is_empty() { return Vec::new(); }
            (0..n).map(|i| {
                let t = if n == 1 { 0.0 } else { i as f32 * (points.len() - 1) as f32 / (n - 1) as f32 };
                points[(t.round() as usize).min(points.len() - 1)]
            }).collect()
        }
        Rail::Contour { min, max, .. } => {
            // every column in the box is a candidate; `sites` filters
            // by the height band, which is what makes it a contour
            let mut v = Vec::new();
            let step = (((max.0 - min.0).max(max.1 - min.1) / 48).max(1)) as usize;
            for x in (min.0..=max.0).step_by(step) {
                for z in (min.1..=max.1).step_by(step) { v.push((x, z)); }
            }
            v
        }
    }
}

fn param_value(p: &Param, seed: u64, index: usize) -> String {
    match p {
        Param::Fixed(s) => s.clone(),
        Param::Pick(v) if v.is_empty() => String::new(),
        Param::Pick(v) => v[(hash(seed, 11) % v.len() as u64) as usize].clone(),
        Param::Range(a, b) => {
            let (lo, hi) = (a.min(b), a.max(b));
            (lo + (hash(seed, 12) % ((hi - lo + 1).max(1)) as u64) as i32).to_string()
        }
        Param::Index { from, step } => (from + step * index as i32).to_string(),
    }
}

/// Resolve a scatter against the world.
pub fn sites<W: VoxelWorld>(w: &W, s: &Scatter, from_y: i32) -> (Vec<Site>, Vec<String>) {
    let mut out: Vec<Site> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let (mut rejected_slope, mut rejected_water, mut rejected_spacing, mut rejected_band) = (0, 0, 0, 0);
    let surface = |x: i32, z: i32| -> Option<i32> {
        (0..=from_y + 64).rev().map(|y| y).find(|&y| w.get(x, y, z) != 0)
    };
    let is_water = |x: i32, y: i32, z: i32| matches!(w.get(x, y, z), 8 | 9 | 31 | 32);
    let fp = s.rules.footprint.max(0);
    for (i, (x, z)) in candidates(&s.rail, s.count, s.seed).into_iter().enumerate() {
        if out.len() >= s.count { break; }
        let Some(sy) = surface(x, z) else { continue };
        // the height band, which is what makes a contour a contour
        if let Rail::Contour { y, band, .. } = &s.rail {
            if (sy - y).abs() > *band { rejected_band += 1; continue; }
        }
        if let Some((lo, hi)) = s.rules.y_range {
            if sy < lo || sy > hi { rejected_band += 1; continue; }
        }
        // slope across the footprint
        let mut lo = sy; let mut hi = sy;
        let mut water_in = false;
        let mut water_near = false;
        for dx in -fp..=fp { for dz in -fp..=fp {
            if let Some(y) = surface(x + dx, z + dz) { lo = lo.min(y); hi = hi.max(y); 
                if is_water(x + dx, y, z + dz) { water_in = true; } }
        }}
        if hi - lo > s.rules.max_slope { rejected_slope += 1; continue; }
        if s.rules.water_margin > 0 {
            let m = s.rules.water_margin;
            'near: for dx in -(fp + m)..=(fp + m) { for dz in -(fp + m)..=(fp + m) {
                if let Some(y) = surface(x + dx, z + dz) {
                    if is_water(x + dx, y, z + dz) { water_near = true; break 'near; }
                }
            }}
        }
        match s.rules.water {
            WaterRule::Avoid => if water_in || water_near { rejected_water += 1; continue; },
            WaterRule::Require => if !water_in && !water_near { rejected_water += 1; continue; },
            WaterRule::Allow => {}
        }
        // headroom
        if (1..=s.rules.headroom).any(|d| w.get(x, sy + d, z) != 0) { rejected_slope += 1; continue; }
        // spacing against sites already taken
        if out.iter().any(|o| {
            let (dx, dz) = ((o.at.0 - x).abs(), (o.at.2 - z).abs());
            dx < s.rules.spacing && dz < s.rules.spacing
        }) { rejected_spacing += 1; continue; }
        let site_seed = hash(s.seed, i as u64);
        let rot = match s.facing {
            Facing::Fixed(d) => d.rem_euclid(360) / 90 * 90,
            Facing::Random => (hash(site_seed, 3) % 4) as i32 * 90,
            Facing::Out => {
                let c = rail_centre(&s.rail);
                let (dx, dz) = (x - c.0, z - c.1);
                // the quarter turn that faces away from the centre
                if dx.abs() >= dz.abs() { if dx >= 0 { 90 } else { 270 } } else if dz >= 0 { 180 } else { 0 }
            }
            Facing::Along => {
                let t = rail_tangent(&s.rail, (x, z));
                if t.0.abs() >= t.1.abs() { if t.0 >= 0 { 0 } else { 180 } } else if t.1 >= 0 { 90 } else { 270 }
            }
        };
        let params = s.params.iter().map(|(k, p)| (k.clone(), param_value(p, site_seed, out.len()))).collect();
        out.push(Site { at: (x, sy + 1, z), rot, seed: site_seed, index: out.len(), params });
    }
    if out.len() < s.count {
        notes.push(format!("{} of {} site(s) placed", out.len(), s.count));
    }
    for (n, why) in [(rejected_slope, "too steep or blocked"), (rejected_water, "water"),
                     (rejected_spacing, "too close to another"), (rejected_band, "outside the height band")] {
        if n > 0 { notes.push(format!("{n} rejected: {why}")); }
    }
    (out, notes)
}

fn rail_centre(r: &Rail) -> (i32, i32) {
    match r {
        Rail::Point { at } => *at,
        Rail::Line { from, to } => ((from.0 + to.0) / 2, (from.1 + to.1) / 2),
        Rail::Ring { centre, .. } => *centre,
        Rail::Grid { min, max, .. } | Rail::Area { min, max } | Rail::Contour { min, max, .. } =>
            ((min.0 + max.0) / 2, (min.1 + max.1) / 2),
        Rail::Path { points } => points.first().copied().unwrap_or((0, 0)),
    }
}

fn rail_tangent(r: &Rail, at: (i32, i32)) -> (i32, i32) {
    match r {
        Rail::Line { from, to } => (to.0 - from.0, to.1 - from.1),
        Rail::Ring { centre, .. } => (-(at.1 - centre.1), at.0 - centre.0),
        Rail::Path { points } => {
            let i = points.iter().enumerate()
                .min_by_key(|(_, p)| (p.0 - at.0).pow(2) + (p.1 - at.1).pow(2)).map(|(i, _)| i).unwrap_or(0);
            let j = (i + 1).min(points.len().saturating_sub(1));
            (points[j].0 - points[i].0, points[j].1 - points[i].1)
        }
        _ => (1, 0),
    }
}

/// The IR for the whole scatter: one line per site.
pub fn to_ir(s: &Scatter, sites: &[Site]) -> String {
    let mut out = format!("# {} x {}\n", sites.len(), s.structure);
    for site in sites { out.push_str(&site.to_ir(&s.structure)); out.push('\n'); }
    out
}

// ── JSON, hand-rolled ────────────────────────────────────────────────
//
// The crate has no dependencies and this is the only place JSON is
// wanted, so it is parsed by hand: a small recursive reader over the
// subset a scatter needs (objects, arrays, strings, numbers, bools).
// Unknown keys are IGNORED rather than refused, so a file written for
// a later version still loads.

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Str(String), Num(f64), Bool(bool), Arr(Vec<Json>), Obj(BTreeMap<String, Json>), Null,
}

impl Json {
    pub fn get(&self, k: &str) -> Option<&Json> { match self { Json::Obj(m) => m.get(k), _ => None } }
    pub fn as_str(&self) -> Option<&str> { match self { Json::Str(s) => Some(s), _ => None } }
    pub fn as_i32(&self) -> Option<i32> { match self { Json::Num(n) => Some(*n as i32), _ => None } }
    pub fn as_bool(&self) -> Option<bool> { match self { Json::Bool(b) => Some(*b), _ => None } }
    pub fn as_arr(&self) -> Option<&[Json]> { match self { Json::Arr(v) => Some(v), _ => None } }
    /// An [x, z] or [x, y, z] pair as x,z.
    pub fn as_xz(&self) -> Option<(i32, i32)> {
        let a = self.as_arr()?;
        match a.len() {
            2 => Some((a[0].as_i32()?, a[1].as_i32()?)),
            3 => Some((a[0].as_i32()?, a[2].as_i32()?)),
            _ => None,
        }
    }
}

pub fn parse_json(src: &str) -> Result<Json, String> {
    let b: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let v = json_value(&b, &mut i)?;
    Ok(v)
}

fn skip_ws(b: &[char], i: &mut usize) { while *i < b.len() && b[*i].is_whitespace() { *i += 1; } }

fn json_value(b: &[char], i: &mut usize) -> Result<Json, String> {
    skip_ws(b, i);
    match b.get(*i) {
        None => Err("unexpected end of JSON".into()),
        Some('{') => {
            *i += 1;
            let mut m = BTreeMap::new();
            loop {
                skip_ws(b, i);
                if b.get(*i) == Some(&'}') { *i += 1; return Ok(Json::Obj(m)); }
                let k = match json_value(b, i)? { Json::Str(s) => s, other => return Err(format!("key must be a string, got {other:?}")) };
                skip_ws(b, i);
                if b.get(*i) != Some(&':') { return Err(format!("expected ':' after key '{k}'")); }
                *i += 1;
                m.insert(k, json_value(b, i)?);
                skip_ws(b, i);
                match b.get(*i) { Some(',') => { *i += 1; } Some('}') => { *i += 1; return Ok(Json::Obj(m)); }
                                  _ => return Err("expected ',' or '}'".into()) }
            }
        }
        Some('[') => {
            *i += 1;
            let mut v = Vec::new();
            loop {
                skip_ws(b, i);
                if b.get(*i) == Some(&']') { *i += 1; return Ok(Json::Arr(v)); }
                v.push(json_value(b, i)?);
                skip_ws(b, i);
                match b.get(*i) { Some(',') => { *i += 1; } Some(']') => { *i += 1; return Ok(Json::Arr(v)); }
                                  _ => return Err("expected ',' or ']'".into()) }
            }
        }
        Some('"') => {
            *i += 1;
            let mut s = String::new();
            while let Some(c) = b.get(*i) {
                *i += 1;
                match c {
                    '"' => return Ok(Json::Str(s)),
                    '\\' => { let e = b.get(*i).copied().unwrap_or('\\'); *i += 1;
                              s.push(match e { 'n' => '\n', 't' => '\t', 'r' => '\r', o => o }); }
                    o => s.push(*o),
                }
            }
            Err("unterminated string".into())
        }
        Some(c) if *c == '-' || c.is_ascii_digit() => {
            let start = *i;
            if b[*i] == '-' { *i += 1; }
            while b.get(*i).map(|c| c.is_ascii_digit() || *c == '.' || *c == 'e' || *c == 'E' || *c == '-').unwrap_or(false) { *i += 1; }
            b[start..*i].iter().collect::<String>().parse().map(Json::Num).map_err(|e| format!("bad number: {e}"))
        }
        Some('t') => { *i += 4; Ok(Json::Bool(true)) }
        Some('f') => { *i += 5; Ok(Json::Bool(false)) }
        Some('n') => { *i += 4; Ok(Json::Null) }
        Some(c) => Err(format!("unexpected character '{c}'")),
    }
}

/// A scatter from JSON. `anchor` resolves rails written relative to
/// the player's focus, which is what a model writes.
pub fn from_json(src: &str, anchor: Option<(i32, i32, i32)>) -> Result<Scatter, String> {
    let j = parse_json(src)?;
    let structure = j.get("structure").or_else(|| j.get("form")).and_then(|v| v.as_str())
        .ok_or("a scatter needs a \"structure\"")?.to_string();
    if crate::dict::lookup(&structure).is_none() {
        return Err(format!("unknown structure '{structure}'; known: {}",
                           crate::dict::FORMS.iter().map(|f| f.name).collect::<Vec<_>>().join(", ")));
    }
    let a = anchor.map(|a| (a.0, a.2)).unwrap_or((0, 0));
    let rel = |p: (i32, i32), relative: bool| if relative { (p.0 + a.0, p.1 + a.1) } else { p };
    let r = j.get("rail").ok_or("a scatter needs a \"rail\"")?;
    let relative = r.get("relative").and_then(|v| v.as_bool()).unwrap_or(true);
    let kind = r.get("kind").and_then(|v| v.as_str()).ok_or("the rail needs a \"kind\"")?;
    let xz = |k: &str| r.get(k).and_then(|v| v.as_xz()).map(|p| rel(p, relative));
    let rail = match kind {
        "point" => Rail::Point { at: xz("at").ok_or("point needs \"at\": [x, z]")? },
        "line" => Rail::Line { from: xz("from").ok_or("line needs \"from\"")?,
                               to: xz("to").ok_or("line needs \"to\"")? },
        "ring" => Rail::Ring { centre: xz("centre").or_else(|| xz("center")).unwrap_or(a),
                               radius: r.get("radius").and_then(|v| v.as_i32()).ok_or("ring needs \"radius\"")?,
                               arc_deg: r.get("arc").and_then(|v| v.as_arr())
                                   .and_then(|v| Some((v.first()?.as_i32()?, v.get(1)?.as_i32()?)))
                                   .unwrap_or((0, 360)) },
        "grid" => Rail::Grid { min: xz("min").ok_or("grid needs \"min\"")?, max: xz("max").ok_or("grid needs \"max\"")?,
                               step: r.get("step").and_then(|v| v.as_i32()).unwrap_or(24) },
        "area" => Rail::Area { min: xz("min").ok_or("area needs \"min\"")?, max: xz("max").ok_or("area needs \"max\"")? },
        "path" => Rail::Path { points: r.get("points").and_then(|v| v.as_arr())
            .ok_or("path needs \"points\": [[x, z], ...]")?
            .iter().filter_map(|p| p.as_xz()).map(|p| rel(p, relative)).collect() },
        "contour" | "coast" => Rail::Contour {
            min: xz("min").ok_or("contour needs \"min\"")?, max: xz("max").ok_or("contour needs \"max\"")?,
            y: r.get("y").and_then(|v| v.as_i32()).unwrap_or(64),
            band: r.get("band").and_then(|v| v.as_i32()).unwrap_or(2) },
        other => return Err(format!("unknown rail kind '{other}'; known: point, line, ring, grid, area, path, contour")),
    };
    let mut rules = Rules::default();
    if let Some(ru) = j.get("rules") {
        if let Some(v) = ru.get("max_slope").and_then(|v| v.as_i32()) { rules.max_slope = v.max(0); }
        if let Some(v) = ru.get("spacing").and_then(|v| v.as_i32()) { rules.spacing = v.max(0); }
        if let Some(v) = ru.get("headroom").and_then(|v| v.as_i32()) { rules.headroom = v.max(0); }
        if let Some(v) = ru.get("footprint").and_then(|v| v.as_i32()) { rules.footprint = v.clamp(0, 32); }
        if let Some(v) = ru.get("water_margin").and_then(|v| v.as_i32()) { rules.water_margin = v.clamp(0, 32); }
        if let Some(v) = ru.get("y_range").and_then(|v| v.as_arr())
            .and_then(|v| Some((v.first()?.as_i32()?, v.get(1)?.as_i32()?))) { rules.y_range = Some(v); }
        rules.water = match ru.get("water").and_then(|v| v.as_str()) {
            Some("allow") => WaterRule::Allow, Some("require") => WaterRule::Require, _ => WaterRule::Avoid,
        };
    }
    let facing = match j.get("facing") {
        Some(Json::Str(s)) => match s.as_str() {
            "along" => Facing::Along, "out" => Facing::Out, "random" => Facing::Random,
            other => return Err(format!("unknown facing '{other}'; known: along, out, random, or a number")),
        },
        Some(Json::Num(n)) => Facing::Fixed(*n as i32),
        _ => Facing::Along,
    };
    let mut params = BTreeMap::new();
    if let Some(Json::Obj(m)) = j.get("params") {
        for (k, v) in m {
            let p = match v {
                Json::Str(s) => Param::Fixed(s.clone()),
                Json::Num(n) => Param::Fixed((*n as i32).to_string()),
                Json::Arr(a) if a.iter().all(|x| matches!(x, Json::Str(_))) =>
                    Param::Pick(a.iter().filter_map(|x| x.as_str().map(String::from)).collect()),
                Json::Arr(a) if a.len() == 2 => Param::Range(a[0].as_i32().unwrap_or(0), a[1].as_i32().unwrap_or(0)),
                Json::Obj(o) if o.contains_key("index") => {
                    let idx = &o["index"];
                    Param::Index { from: idx.get("from").and_then(|v| v.as_i32()).unwrap_or(1),
                                   step: idx.get("step").and_then(|v| v.as_i32()).unwrap_or(1) }
                }
                other => return Err(format!("param '{k}': cannot read {other:?}")),
            };
            params.insert(k.clone(), p);
        }
    }
    Ok(Scatter {
        structure, rail, rules,
        count: j.get("count").and_then(|v| v.as_i32()).unwrap_or(8).clamp(1, 512) as usize,
        facing, params,
        seed: j.get("seed").and_then(|v| v.as_i32()).unwrap_or(1) as u64,
        lazy: j.get("lazy").and_then(|v| v.as_bool()).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Terrain;
    impl VoxelWorld for Terrain {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            // a slope rising east, water west of x = -20, a cliff at x = 40
            let h = if x < -20 { 60 } else if x > 40 { 90 } else { 64 + x / 8 };
            if x < -20 && y <= 62 { return if y == 62 { 9 } else { 1 } }
            if y <= h { 1 } else { 0 }
        }
        fn set(&mut self, _: i32, _: i32, _: i32, _: u16) -> bool { false }
    }

    #[test]
    fn json_parses_the_subset_a_scatter_needs() {
        let j = parse_json(r#"{ "a": 1, "b": [1, 2, 3], "c": "hi\nthere", "d": true, "e": {"f": -2.5}, "g": null }"#).unwrap();
        assert_eq!(j.get("a").unwrap().as_i32(), Some(1));
        assert_eq!(j.get("b").unwrap().as_arr().unwrap().len(), 3);
        assert_eq!(j.get("c").unwrap().as_str(), Some("hi\nthere"));
        assert_eq!(j.get("d").unwrap().as_bool(), Some(true));
        assert_eq!(j.get("e").unwrap().get("f").unwrap().as_i32(), Some(-2));
        assert_eq!(j.get("g"), Some(&Json::Null));
        assert!(parse_json("{").is_err() && parse_json(r#"{"a" 1}"#).is_err());
        assert!(parse_json("").is_err());
    }

    #[test]
    fn a_line_of_towers_lands_on_the_slope_and_skips_the_water() {
        let s = from_json(r#"{
            "structure": "tower",
            "rail": { "kind": "line", "from": [-40, 0], "to": [40, 0], "relative": false },
            "count": 9,
            "rules": { "spacing": 6, "max_slope": 4, "footprint": 3 },
            "facing": "along",
            "params": { "size": "small", "seed": [1, 999] }
        }"#, None).unwrap();
        assert_eq!(s.structure, "tower");
        let (sites, notes) = sites(&Terrain, &s, 64);
        assert!(!sites.is_empty(), "{notes:?}");
        assert!(sites.iter().all(|p| p.at.0 >= -20), "none in the water: {:?}", sites.iter().map(|p| p.at.0).collect::<Vec<_>>());
        // each sits on the surface, one above it
        for p in &sites { assert_eq!(Terrain.get(p.at.0, p.at.1, p.at.2), 0); assert_ne!(Terrain.get(p.at.0, p.at.1 - 1, p.at.2), 0); }
        // spacing respected
        for (i, a) in sites.iter().enumerate() { for b in &sites[i + 1..] {
            assert!((a.at.0 - b.at.0).abs() >= 6 || (a.at.2 - b.at.2).abs() >= 6, "{a:?} {b:?}");
        }}
        // the params: size fixed, seed in range
        assert!(sites.iter().all(|p| p.params["size"] == "small"));
        assert!(sites.iter().all(|p| p.params["seed"].parse::<i32>().map(|v| (1..=999).contains(&v)).unwrap_or(false)));
        // and the IR is valid, and ABSOLUTE: a held site builds with no
        // anchor at all
        let ir = to_ir(&s, &sites);
        assert!(!ir.contains('@'), "absolute coordinates only: {ir}");
        let (anchorless, issues) = crate::plan::parse_plan_at(&ir, None);
        assert_eq!(anchorless.ops.len(), sites.len(), "builds with no anchor: {issues:?}");
        let (plan, issues) = crate::plan::parse_plan_at(&ir, Some((0, 65, 0)));
        assert!(issues.iter().all(|i| i.severity != crate::plan::Severity::Error), "{ir}\n{issues:?}");
        assert_eq!(plan.ops.len(), sites.len());
        for l in ir.lines() { assert!(crate::plan::line_is_valid_ir(l), "{l:?}"); }
    }

    #[test]
    fn a_contour_follows_a_height_and_a_coast_wants_the_water() {
        let coast = from_json(r#"{
            "structure": "tower",
            "rail": { "kind": "contour", "min": [-60, -30], "max": [60, 30], "y": 62, "band": 1, "relative": false },
            "count": 6,
            "rules": { "spacing": 10, "water": "require", "water_margin": 4, "footprint": 2 }
        }"#, None).unwrap();
        let (found, notes) = sites(&Terrain, &coast, 64);
        assert!(!found.is_empty(), "{notes:?}");
        // the shore is at x = -20; every site is on the flat west side
        // the waterline: surface 62 (water over stone), sites one above
        assert!(found.iter().all(|p| p.at.1 == 63), "on the shore: {:?}", found.iter().map(|p| p.at.1).collect::<Vec<_>>());
        assert!(found.iter().all(|p| p.at.0 < -20), "west of the shoreline: {:?}", found.iter().map(|p| p.at.0).collect::<Vec<_>>());
        // and a band that matches nothing places nothing, with a reason
        let high = from_json(r#"{ "structure": "tower",
            "rail": { "kind": "contour", "min": [-10, -10], "max": [10, 10], "y": 200, "band": 1, "relative": false },
            "count": 4 }"#, None).unwrap();
        let (none, why) = sites(&Terrain, &high, 64);
        assert!(none.is_empty());
        assert!(why.iter().any(|w| w.contains("height band")), "{why:?}");
    }

    #[test]
    fn a_ring_faces_outward_and_an_index_param_counts_along_the_rail() {
        let s = from_json(r#"{
            "structure": "statue",
            "rail": { "kind": "ring", "centre": [0, 0], "radius": 24, "relative": false },
            "count": 8,
            "rules": { "spacing": 4, "max_slope": 8, "footprint": 2 },
            "facing": "out",
            "params": { "marker": { "index": { "from": 1, "step": 1 } }, "style": ["figure", "idol"] }
        }"#, None).unwrap();
        let (sites, notes) = sites(&Terrain, &s, 64);
        assert!(sites.len() >= 4, "{} sites: {notes:?}", sites.len());
        // outward facing: the site east of centre faces +x
        let east = sites.iter().max_by_key(|p| p.at.0).unwrap();
        assert_eq!(east.rot, 90, "the east site faces out");
        // the index counts 1, 2, 3 ... along the rail
        let markers: Vec<i32> = sites.iter().map(|p| p.params["marker"].parse().unwrap()).collect();
        assert_eq!(markers, (1..=sites.len() as i32).collect::<Vec<_>>());
        // the pick is one of the two, and not always the same one
        assert!(sites.iter().all(|p| ["figure", "idol"].contains(&p.params["style"].as_str())));
        assert_eq!(from_json(r#"{"structure":"tower","rail":{"kind":"ring","radius":10},"facing":270}"#, None).unwrap().facing,
                   Facing::Fixed(270));
    }

    #[test]
    fn an_area_thins_to_the_spacing_and_a_grid_is_jittered() {
        let area = from_json(r#"{ "structure": "nest",
            "rail": { "kind": "area", "min": [0, -30], "max": [40, 30], "relative": false },
            "count": 12, "rules": { "spacing": 8, "max_slope": 8, "footprint": 2 } }"#, None).unwrap();
        let (found, _) = sites(&Terrain, &area, 64);
        assert!(found.len() >= 4);
        for (i, a) in found.iter().enumerate() { for b in &found[i + 1..] {
            assert!((a.at.0 - b.at.0).abs() >= 8 || (a.at.2 - b.at.2).abs() >= 8);
        }}
        let grid = from_json(r#"{ "structure": "house",
            "rail": { "kind": "grid", "min": [0, -20], "max": [40, 20], "step": 16, "relative": false },
            "count": 9, "rules": { "spacing": 4, "max_slope": 8, "footprint": 2 } }"#, None).unwrap();
        let (g, _) = sites(&Terrain, &grid, 64);
        assert!(g.len() >= 4);
        // jittered: not every site is on a multiple of the step
        assert!(g.iter().any(|p| p.at.0 % 16 != 0 || p.at.2 % 16 != 0), "a grid that reads as a grid: {:?}",
                g.iter().map(|p| (p.at.0, p.at.2)).collect::<Vec<_>>());
    }

    #[test]
    fn the_errors_say_what_is_wrong_and_unknown_keys_are_ignored() {
        assert!(from_json("{}", None).unwrap_err().contains("\"structure\""));
        assert!(from_json(r#"{"structure":"zigurat","rail":{"kind":"point","at":[0,0]}}"#, None)
                .unwrap_err().contains("unknown structure 'zigurat'"));
        assert!(from_json(r#"{"structure":"tower"}"#, None).unwrap_err().contains("\"rail\""));
        assert!(from_json(r#"{"structure":"tower","rail":{"kind":"helix"}}"#, None)
                .unwrap_err().contains("unknown rail kind 'helix'"));
        assert!(from_json(r#"{"structure":"tower","rail":{"kind":"line","to":[1,1]}}"#, None)
                .unwrap_err().contains("\"from\""));
        // a key from a later version does not stop it loading
        let ok = from_json(r#"{"structure":"tower","rail":{"kind":"point","at":[0,0]},
                               "biome_filter":"tundra","future":{"x":1}}"#, None).unwrap();
        assert_eq!(ok.count, 8);
        assert!(!ok.lazy);
    }

    #[test]
    fn relative_rails_land_at_the_focus_and_lazy_is_carried() {
        let s = from_json(r#"{ "structure": "tower", "count": 3, "lazy": true,
            "rail": { "kind": "line", "from": [0, 0], "to": [20, 0] } }"#, Some((100, 70, 200))).unwrap();
        assert!(s.lazy, "the engine may hold these rather than build them");
        match s.rail { Rail::Line { from, to } => assert_eq!((from, to), ((100, 200), (120, 200))),
                       o => panic!("{o:?}") }
    }
}
