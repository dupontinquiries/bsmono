//! The ingest path: English prose -> IR -> validated plan -> batched
//! writes -> mesher.
//!
//! WHY AN IR AT ALL
//!
//! The obvious design is prose -> tool calls -> world. It fails in
//! four specific ways, and each one is a reason for this module:
//!
//!  1. **Partial failure leaves rubble.** A model emits forty calls;
//! ```text
//!     the twelfth is out of bounds. Without a plan you have twelve
//!     blocks of a building and no idea what the other twenty-eight
//!     were going to be. A plan is validated whole, then committed.
//!  2. **You cannot preview a stream of calls.** Users want "show me
//!     what that would do" before it touches their world. A plan can
//!     be dry-run against a shadow copy; a stream of side effects
//!     cannot.
//!  3. **Undo needs a unit.** "Undo the tower" means reversing one
//!     intent, not forty edits the user never saw.
//!  4. **Animation and meshing need ORDER and GROUPING.** Writing
//!     blocks in the order a model happened to emit them remeshes
//!     random chunks repeatedly. Grouping by chunk per animation
//!     tick turns N remeshes into one per chunk per tick, which is
//!     the difference between a build that animates and a build that
//!     stalls the frame.
//! ```
//!
//! WHY A LINE FORMAT AND NOT JSON
//!
//! The IR's text form is one op per line. Small local models emit
//! well-formed lines far more reliably than nested JSON, a truncated
//! line loses one op rather than the whole document, and the result
//! is diffable, hand-editable and cacheable. Native tool calling
//! still works -- `Op` is the same type either way -- but the line
//! format is what survives a 512-token reply budget.
//!
//! ```text
//!     # tower on the hill
//!     terraform 20,-10 34,4 smooth block=stone
//!     path 0,6 27,-3 w=3 block=cobblestone
//!     cylinder 27,-3 r=4 h=12 block=stone hollow
//!     tree -12,-8 oak h=8 lean=1,0
//!     scatter -20,-20 20,20 block=oak_leaves clusters=6 per=10 spread=3 seed=7
//! ```
//!
//! Coordinates are X,Z pairs; Y comes from the ground unless a `y=`
//! is given, because guessing Y is the single most common way a model
//! produces a floating structure.

use crate::{palettes, CallResult, VoxelTools, VoxelWorld};

/// Default footprint half-size when a model gives fewer coordinates
/// than an op wants.
pub const ASSUMED_HALF_SPAN: i32 = 6;

/// A size a model gave that cannot work, replaced by one that can.
///
/// `r=0` and `h=-3` are not requests to build nothing; they are a model
/// fumbling a number. Substituting the default and saying so builds the
/// thing that was asked for. Rejecting the whole plan over it -- which
/// is what "sphere radius must be positive" did to both a 1B and an 8B
/// -- builds nothing and teaches the player that the feature is broken.
fn clamp_size(v: i32, default: i32, notes: &mut Vec<String>, what: &str) -> i32 {
    if v > 0 { return v; }
    notes.push(format!("{what} was {v}; used {default}"));
    default
}

/// The x,z of the focus anchor, or the origin when there is none.
fn anchor_xz(anchor: Option<(i32, i32, i32)>) -> (i32, i32) {
    anchor.map(|a| (a.0, a.2)).unwrap_or((0, 0))
}

/// Resolve the two corners a footprint op wants, from however many the
/// model actually supplied.
///
/// BE PERMISSIVE HERE. Both a 1B and an 8B failed the same way: verbs
/// with too few coordinates, rejected wholesale. "terraform needs two
/// x,z pairs" is technically true and completely useless -- the player
/// asked for flat ground where they are looking, and that is
/// reconstructible. A near-miss should build something and SAY what was
/// assumed; only a request with no recoverable meaning should fail.
///
/// Returns the corners and a note when anything was inferred.
fn footprint(t: &[&str], anchor: Option<(i32, i32, i32)>)
    -> Option<((i32, i32), (i32, i32), Option<String>)> {
    let h = ASSUMED_HALF_SPAN;
    // A token that is PRESENT but unresolvable is not the same as an
    // absent one. `@5,5` with no focus anchor has no meaning to recover
    // -- inferring a footprint there would place the build somewhere the
    // player never indicated. Missing is inferable; broken is an error.
    for i in [1usize, 2] {
        if let Some(s) = t.get(i) {
            if s.starts_with('@') && anchor.is_none() { return None; }
        }
    }
    let get = |i: usize| t.get(i).and_then(|s| pair_at(s, anchor).ok().flatten());
    Some(match (get(1), get(2)) {
        (Some(a), Some(b)) => (a, b, None),
        // One corner: centre a footprint on it.
        (Some(a), None) => ((a.0 - h, a.1 - h), (a.0 + h, a.1 + h),
                            Some(format!("one corner given; used {}x{} around it", h * 2, h * 2))),
        // None at all: the player is looking somewhere. That is the spot.
        _ => {
            let a = anchor_xz(anchor);
            ((a.0 - h, a.1 - h), (a.0 + h, a.1 + h),
             Some(format!("no coordinates; used {}x{} at the focus", h * 2, h * 2)))
        }
    })
}

/// Expand `def NAME` … `end` / `use NAME @x,z [rot=N] [mirror]` into
/// plain lines. Unknown `use` names are left as-is, so the line loop
/// reports them as an unknown op with a line number.
pub fn expand_defs(src: &str) -> String {
    use std::collections::BTreeMap;
    let mut defs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut out = String::new();
    let mut cur: Option<(String, Vec<String>)> = None;
    for line in src.lines() {
        let t = line.trim();
        if let Some((name, body)) = cur.as_mut() {
            if t.eq_ignore_ascii_case("end") {
                defs.insert(name.to_lowercase(), std::mem::take(body));
                cur = None;
            } else { body.push(line.to_string()); }
            // a def contributes no lines of its own; keep numbering by
            // emitting a comment so line numbers in errors still match
            out.push_str("# def\n");
            continue;
        }
        let mut toks = t.split_whitespace();
        match toks.next().map(|s| s.to_lowercase()).as_deref() {
            Some("def") => {
                let name = toks.next().unwrap_or("").to_string();
                cur = Some((name, Vec::new()));
                out.push_str("# def\n");
            }
            Some("use") => {
                let name = toks.next().unwrap_or("").to_lowercase();
                let Some(body) = defs.get(&name) else { out.push_str(line); out.push('\n'); continue };
                let rest: Vec<&str> = toks.collect();
                let (mut dx, mut dz) = (0i32, 0i32);
                for r in &rest {
                    if let Some(c) = r.strip_prefix('@') {
                        if let Some((a, b)) = c.split_once(',') {
                            dx = a.parse().unwrap_or(0); dz = b.parse().unwrap_or(0);
                        }
                    }
                }
                let rot = rest.iter().find_map(|r| r.strip_prefix("rot="))
                    .and_then(|v| v.parse::<i32>().ok()).unwrap_or(0).rem_euclid(360) / 90;
                let mirror = rest.iter().any(|r| *r == "mirror" || *r == "flip");
                for bl in body {
                    out.push_str(&shift_line(bl, dx, dz, rot as u8, mirror));
                    out.push('\n');
                }
            }
            _ => { out.push_str(line); out.push('\n'); }
        }
    }
    out
}

/// Rewrite every `@x,z` and `@x,y,z` token in a line: rotate about the
/// def's origin by quarter turns, mirror across x, then offset.
fn shift_line(line: &str, dx: i32, dz: i32, turns: u8, mirror: bool) -> String {
    line.split(' ').map(|tok| {
        let Some(body) = tok.strip_prefix('@') else { return tok.to_string() };
        let parts: Vec<&str> = body.split(',').collect();
        let nums: Vec<Option<i32>> = parts.iter().map(|p| p.parse().ok()).collect();
        match (nums.len(), nums.as_slice()) {
            (2, [Some(x), Some(z)]) => {
                let (x, z) = rot_xz(*x, *z, turns, mirror);
                format!("@{},{}", x + dx, z + dz)
            }
            (3, [Some(x), Some(y), Some(z)]) => {
                let (x, z) = rot_xz(*x, *z, turns, mirror);
                format!("@{},{},{}", x + dx, y, z + dz)
            }
            _ => tok.to_string(),
        }
    }).collect::<Vec<_>>().join(" ")
}

fn rot_xz(x: i32, z: i32, turns: u8, mirror: bool) -> (i32, i32) {
    let (x, z) = if mirror { (-x, z) } else { (x, z) };
    match turns % 4 { 0 => (x, z), 1 => (-z, x), 2 => (-x, -z), _ => (z, -x) }
}

/// How far above the focus anchor to begin scanning for the ground.
/// Enough to clear the player's own head and a step up, small enough
/// that a cave roof a few blocks above is not mistaken for terrain.
pub const GROUND_SEARCH_MARGIN: i32 = 6;

/// One planned operation. Mirrors the tool set, but as data.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Terraform { min: (i32, i32), max: (i32, i32), delta: i32, smooth: bool, block: u16 },
    Path { from: (i32, i32), to: (i32, i32), width: i32, block: u16 },
    Enclose { min: (i32, i32), max: (i32, i32), height: i32, block: u16 },
    /// A pitched roof over a footprint. 29 mentions in the build corpus
    /// and no word for it in the IR: a model asked for a house made
    /// walls and left the sky open.
    Roof { min: (i32, i32), max: (i32, i32), y: Option<i32>, style: String,
           block: u16, overhang: i32 },
    /// Reflect a region. "either side of" is how builders describe
    /// symmetry, and authoring both halves by hand gets them different.
    Mirror { min: (i32, i32, i32), max: (i32, i32, i32), axis: String, about: i32 },
    /// A span at constant height with piers -- not a path, which follows
    /// the ground and would dig a trench across a ravine.
    Bridge { from: (i32, i32), to: (i32, i32), y: Option<i32>, width: i32,
             block: u16, supports: bool },
    /// A staircase rising one block per step.
    Stairs { from: (i32, i32), to: (i32, i32), y: Option<i32>, width: i32, block: u16 },
    /// A storey floor inside a footprint.
    Floor { min: (i32, i32), max: (i32, i32), y: Option<i32>, block: u16 },
    /// Build from ASCII elevations. The model draws; the tool
    /// reconstructs and reports how faithful the result is.
    Views { views: Vec<(String, String)>, at: (i32, i32), y: Option<i32>,
            depth: i32, block: u16 },
    /// Greebles: evolve a decorator FSM for a region's surface against
    /// a phrase, then apply the winner. Or run a stated genome.
    Greeble { min: (i32, i32, i32), max: (i32, i32, i32), phrase: String,
              gens: u32, seed: i32, genome: Option<String> },
    /// A terrain brush: raise/lower/smooth/flatten/roughen with falloff.
    Brush { op: String, amount: i32, at: (i32, i32), radius: i32, falloff: String, seed: i32 },
    /// A painted layer baked into blocks: a forest, a meadow, a desert.
    Layer { packet: String, min: (i32, i32), max: (i32, i32), density: f32,
            coverage: String, seed: i32 },
    /// A bounded flood: fill a basin with water to a level. A build op,
    /// not a world override -- water placed by a plan within a radius,
    /// which is what a river's pool is.
    Lake { at: (i32, i32), radius: i32, level: i32 },
    /// A road from A to B as a state machine: it cuts, fills, bridges,
    /// tunnels and switchbacks as the ground demands.
    Road { from: (i32, i32), to: (i32, i32), width: i32, block: u16 },
    /// A river run as a state machine from a source.
    River { at: (i32, i32), dir_deg: i32, max_len: i32, seed: i32 },
    /// Apply a mood over a region: palette restyle, weathering, clutter,
    /// and an atmosphere request for the engine.
    Mood { name: String, min: (i32, i32), max: (i32, i32), y: Option<i32> },
    /// Ask the engine to spawn creatures. This crate has none; the
    /// request carries kind, disposition, count and an optional name.
    Populate { kind: String, count: u32, min: (i32, i32), max: (i32, i32),
               name: Option<String> },
    /// Stamp a learned template. The vocabulary the player taught it.
    Place { name: String, at: (i32, i32), y: Option<i32>, turns: u8,
            mirror: bool, vary: f32, seed: i32, palette: Option<String> },
    /// A change to the world itself, PROPOSED by the model. It is only
    /// applied if the player's own words corroborate it; see
    /// `intent::corroborate`. Carries the verb rather than an effect so
    /// the gate runs against prose the parser cannot see.
    World { verb: String, arg: Option<i32>, at: (i32, i32), radius: i32 },
    /// A detailing pass over an existing region: speckle, relief, trim,
    /// feather or ground blend. Like `weather`, a transform rather than
    /// a shape.
    Detail { kind: String, min: (i32, i32, i32), max: (i32, i32, i32),
             block: u16, amount: f32, seed: i32 },
    /// Weather an existing region: decay, overgrowth, rubble.
    /// A transform over blocks that already exist, not a shape — which
    /// is why it is an op and not a dictionary form.
    Weather { min: (i32, i32, i32), max: (i32, i32, i32), preset: String, seed: i32 },
    /// ONE verb reaching the whole form dictionary.
    ///
    /// `roof`, `wall`, `house`, `colonnade`, `arch`, `tower` and
    /// everything added later are entries in `dict::FORMS`, not ops.
    /// Adding `dormer` is a table row: no parser arm, no run arm, no
    /// math arm, no exhaustiveness break in three files.
    Form { name: String, min: (i32, i32), max: (i32, i32), y: Option<i32>,
           style: String, block: u16, accent: u16, nums: Vec<(String, i32)> },
    Fill { min: (i32, i32, i32), max: (i32, i32, i32), block: u16, hollow: bool },
    Cylinder { at: (i32, i32), y: Option<i32>, radius: i32, height: i32, block: u16, hollow: bool },
    Sphere { at: (i32, i32), y: Option<i32>, radius: i32, block: u16, hollow: bool },
    Tree { at: (i32, i32), species: String, height: i32, lean: (i32, i32) },
    /// A living tree: bred toward a phrase, placed at its phenotype,
    /// and registered with the engine so it keeps growing.
    Plant { at: (i32, i32), species: String, phrase: String, seed: i32, mature: bool },
    Scatter { min: (i32, i32), max: (i32, i32), block: u16, clusters: usize,
              per: usize, spread: i32, seed: u64 },
    Restyle { min: (i32, i32, i32), max: (i32, i32, i32), from: String, to: String },
}

#[derive(Debug, Clone)]
pub struct BuildPlan {
    pub name: String,
    pub ops: Vec<Op>,
    /// The focus anchor this plan was parsed against, when there was
    /// one. Carving ops need it: `find_surface` at a door's own x,z
    /// returns the top of the wall that was just built, so a doorway
    /// with no explicit `y` would be cut in the sky above the tower.
    /// The anchor is the ground the player is looking at, which is
    /// what "at the door" means.
    pub anchor: Option<(i32, i32, i32)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Severity { Error, Warning }

#[derive(Debug, Clone)]
pub struct Issue {
    pub severity: Severity,
    /// 1-based op index, or 0 for a whole-plan issue.
    pub op: usize,
    pub message: String,
}

/// Resolve a block name to an id. Accepts a palette role
/// (`oak.primary`) or a bare material name, so a model can write
/// what it means instead of memorising numbers.
pub fn resolve_block(name: &str) -> Option<u16> {
    let n = name.trim().to_lowercase();
    if let Some((pal, role)) = n.split_once('.') {
        let pals = palettes();
        let p = pals.get(pal)?;
        return Some(match role {
            "primary" => p.primary,
            "secondary" => p.secondary,
            "accent" => p.accent,
            "foliage" => p.foliage,
            _ => return None,
        });
    }
    // Full name table, generated against unendlich's BlockId enum in
    // world/blocks.rs. Aliases follow what a person actually types
    // into a chat box -- "cobble", "planks", "bricks" -- because the
    // paintbrush is driven by prose, not by the enum's spelling.
    Some(match n.as_str() {
        "air" | "nothing" | "empty" => 0,
        "stone" => 1,
        "grass" | "grass_block" => 2,
        "dirt" => 3,
        "cobblestone" | "cobble" => 4,
        "bedrock" => 7,
        "water" => 9,
        "lava" => 11,
        "sand" => 12,
        "gravel" => 13,
        "gold_ore" => 14,
        "iron_ore" => 15,
        "coal_ore" => 16,
        "oak_log" | "log" | "oak_wood" | "wood" => 17,
        "oak_leaves" | "leaves" | "foliage" => 18,
        "birch_log" | "birch" => 19,
        "birch_leaves" => 20,
        "cactus" => 21,
        "clay" => 22,
        "moss" => 23,
        "red_sandstone" => 24,
        "flower" | "flowers" => 25,
        "tall_grass" => 26,
        "fern" => 27,
        "dead_bush" => 28,
        "vine" | "vines" => 29,
        "dripstone" => 30,
        "ice" => 31,
        "snow" => 32,
        "mushroom" => 33,
        "cobweb" => 34,
        "lichen" => 35,
        "limestone" => 36,
        "marble" => 37,
        "basalt" => 38,
        "tuff" => 39,
        "granite" => 40,
        "slate" => 41,
        "calcite" => 42,
        "andesite" => 43,
        "schist" => 44,
        "bone" => 45,
        "amber" => 46,
        "crystal" => 47,
        "sandstone" => 48,
        "obsidian" => 49,
        "copper_ore" => 50,
        "quartz" => 51,
        "diamond_ore" => 56,
        "oak_planks" | "planks" | "plank" => 57,
        "stone_bricks" | "stone_brick" | "bricks_stone" => 58,
        "bricks" | "brick" => 59,
        "glass" => 61,
        "iron_block" | "iron" => 62,
        "gold_block" | "gold" => 63,
        "coal_block" => 64,
        "bookshelf" | "books" => 65,
        "pumpkin" => 66,
        "snow_block" => 80,
        "mossy_cobblestone" | "mossy_cobble" | "mossy" => 81,
        "polished_stone" | "polished" => 82,
        "smooth_sandstone" => 83,
        "terracotta" => 84,
        "chiseled_stone_bricks" | "chiseled" => 85,
        "dark_oak_planks" | "dark_oak" => 86,
        "polished_basalt" => 87,
        "carrot" | "carrots" => 88,
        "potato" | "potatoes" => 89,
        "cabbage" => 90,
        "sprout" | "sprouts" => 91,
        "selenite" => 92,
        "gypsum" => 93,
        "dolomite" => 94,
        "sulfur" => 95,
        "magma" => 96,
        "glow_crystal" | "glowstone" => 97,
        _ => return n.parse().ok(),      // numeric ids still accepted
    })
}

fn pair(s: &str) -> Option<(i32, i32)> {
    let (a, b) = s.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// A coordinate pair that may be focus-relative: `@5,-3` means five
/// east and three north of the anchor the player is looking at. This
/// is the IR half of the placement engine -- the model reasons in
/// "here" and the tool supplies the world coordinates. Returns None if
/// a relative pair is used with no anchor available.
fn pair_at(s: &str, anchor: Option<(i32, i32, i32)>) -> Result<Option<(i32, i32)>, &'static str> {
    if let Some(rest) = s.strip_prefix('@') {
        let Some(a) = anchor else { return Err("relative coordinate used but no focus anchor") };
        Ok(pair(rest).map(|(dx, dz)| (a.0 + dx, a.2 + dz)))
    } else {
        Ok(pair(s))
    }
}

fn triple(s: &str) -> Option<(i32, i32, i32)> {
    let mut it = s.split(',');
    let (a, b, c) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() { return None; }
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?, c.trim().parse().ok()?))
}

/// 3D coordinate, `@dx,dy,dz` fully relative to the anchor (y included,
/// so "from one below the anchor to fifteen above" is @0,-1,0 @0,15,0).
fn triple_at(s: &str, anchor: Option<(i32, i32, i32)>) -> Result<Option<(i32, i32, i32)>, &'static str> {
    if let Some(rest) = s.strip_prefix('@') {
        let Some(a) = anchor else { return Err("relative coordinate used but no focus anchor") };
        Ok(triple(rest).map(|(dx, dy, dz)| (a.0 + dx, a.1 + dy, a.2 + dz)))
    } else {
        Ok(triple(s))
    }
}

fn kv(tokens: &[&str], key: &str) -> Option<String> {
    tokens.iter().find_map(|t| t.strip_prefix(&format!("{key}="))).map(|v| v.to_string())
}

fn kvi(tokens: &[&str], key: &str, default: i32) -> i32 {
    kv(tokens, key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Parse the line IR. Unparseable lines become issues rather than
/// aborting: one bad line should cost one op, not the whole build.
pub fn parse_plan(src: &str) -> (BuildPlan, Vec<Issue>) {
    parse_plan_at(src, None)
}

/// Parse with a focus anchor, so `@dx,dz` coordinates resolve. The
/// anchor comes from `focus::Focus::anchor` (x, z).
pub fn parse_plan_at(src: &str, anchor: Option<(i32, i32, i32)>) -> (BuildPlan, Vec<Issue>) {
    let mut ops = Vec::new();
    let mut issues = Vec::new();
    let mut name = String::from("build");
    // Peekable, because `views` is the one construct that spans lines:
    // it consumes its own drawings up to `end`.
    // SUBSTRUCTURES BY ID. `def NAME` ... `end` captures a block of
    // lines; `use NAME @x,z [rot=90|180|270] [mirror]` re-emits them
    // with every @-relative coordinate offset and rotated. A village is
    // six houses: with this the model writes one house and six `use`
    // lines -- a sixth of the output tokens, which on a 1B is the
    // expensive part, and six IDENTICAL houses rather than six
    // approximations. Expanded here, before the line loop, so every
    // other op sees plain lines and needs to know nothing about it.
    let src_owned = expand_defs(src);
    let src: &str = &src_owned;
    let mut lines_iter = src.lines().enumerate().peekable();
    while let Some((lineno, raw)) = lines_iter.next() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            if let Some(c) = raw.trim().strip_prefix("# ") {
                if lineno == 0 { name = c.trim().to_string(); }
            }
            continue;
        }
        let t: Vec<&str> = line.split_whitespace().collect();
        let idx = ops.len() + 1;
        // Resolve token N as a possibly-relative pair; a bad relative
        // pair records an issue and yields None so the op is skipped
        // rather than silently placed at the wrong spot.
        let rel_err = std::cell::Cell::new(false);
        // Inferences recorded as WARNINGS, not errors: the op still runs,
        // and the preview tells the player what was filled in, so a wrong
        // guess is visible before /commit rather than after. Collected
        // into a local and drained at the end of the line, because a
        // closure capturing `issues` mutably would conflict with `bad`.
        let mut notes: Vec<String> = Vec::new();
        macro_rules! assumed { ($w:expr) => { notes.push($w.to_string()) } }
        let rel = |t: &[&str], i: usize, anchor: Option<(i32, i32, i32)>| -> Option<(i32, i32)> {
            match t.get(i).map(|s| pair_at(s, anchor)) {
                Some(Ok(v)) => v,
                Some(Err(_)) => { rel_err.set(true); None }
                None => None,
            }
        };
        // EVERY parse error carries its line number. `recovery::salvage`
        // identifies bad lines by parsing "line N" out of the message,
        // so an error without one is invisible to it -- the salvage
        // finds nothing to drop, concludes the plan is clean, and the
        // whole thing is rejected on the errors anyway. That is exactly
        // what happened to a 1B whose fifth line was a malformed
        // restyle: four good lines thrown away with it.
        let mut bad = |m: String| issues.push(Issue {
            severity: Severity::Error, op: idx,
            message: format!("line {}: {m}", lineno + 1) });
        let blk = |d: &str| resolve_block(&kv(&t, "block").unwrap_or_else(|| d.to_string()));
        let hollow = t.contains(&"hollow");

        match t[0] {
            "terraform" => {
                let Some((a, b, note)) = footprint(&t, anchor) else {
                    rel_err.set(true);
                    continue;
                };
                if let Some(n) = note { assumed!(&n); }
                ops.push(Op::Terraform {
                    min: a, max: b, delta: kvi(&t, "delta", 0),
                    // A terraform with no delta and no `smooth` would do
                    // nothing at all; smoothing is what "flatten this"
                    // means and is the safe reading.
                    smooth: t.contains(&"smooth") || kvi(&t, "delta", 0) == 0,
                    block: blk("stone").unwrap_or(1) });
            }
            "path" => {
                let Some((a, b, note)) = footprint(&t, anchor) else {
                    rel_err.set(true);
                    continue;
                };
                if let Some(n) = note { assumed!(&n); }
                ops.push(Op::Path {
                    from: a, to: b, width: kvi(&t, "w", 3).max(1),
                    block: blk("cobblestone").unwrap_or(4) });
            }
            "enclose" => {
                let Some((a, b, note)) = footprint(&t, anchor) else {
                    rel_err.set(true);
                    continue;
                };
                if let Some(n) = note { assumed!(&n); }
                ops.push(Op::Enclose {
                    min: a, max: b, height: kvi(&t, "h", 3).max(1),
                    block: blk("oak_planks").unwrap_or(57) });
            }
            // `views` opens a multi-line block: each `front:`/`side:`/
            // `top:` line is followed by its drawing, ended by `end`.
            // Parsed here rather than as a normal op because it is the
            // one construct that spans lines.
            "views" => {
                let at = rel(&t, 1, anchor).unwrap_or_else(|| anchor_xz(anchor));
                let depth = kvi(&t, "depth", 8).max(1);
                let mut collected: Vec<(String, String)> = Vec::new();
                let mut current: Option<(String, Vec<String>)> = None;
                while let Some((_, next)) = lines_iter.next() {
                    let raw = next.trim_end();
                    let low = raw.trim().to_lowercase();
                    if low == "end" { break; }
                    if let Some(name) = low.strip_suffix(':') {
                        if let Some((n, art)) = current.take() {
                            collected.push((n, art.join("\n")));
                        }
                        current = Some((name.to_string(), Vec::new()));
                    } else if let Some((_, art)) = current.as_mut() {
                        art.push(raw.to_string());
                    }
                }
                if let Some((n, art)) = current.take() {
                    collected.push((n, art.join("\n")));
                }
                if collected.is_empty() {
                    bad("views needs at least one of front:/side:/top:".into());
                } else {
                    ops.push(Op::Views {
                        views: collected, at,
                        y: kv(&t, "y").and_then(|v| v.parse().ok()),
                        depth, block: blk("stone").unwrap_or(1) });
                }
            }
            // `form <name> A B [style] [k=v ...]` -- the dictionary verb.
            "form" | "build" => {
                let Some(name) = t.get(1).map(|s| s.to_string()) else {
                    bad("form needs a name, e.g. `form house @-6,-6 @6,6`".into());
                    continue;
                };
                if crate::dict::lookup(&name).is_none() {
                    bad(format!("unknown form '{name}'; known: {}",
                        crate::dict::FORMS.iter().map(|f| f.name)
                            .collect::<Vec<_>>().join(", ")));
                    continue;
                }
                // Coordinates start at token 2 for this verb, so shift.
                let shifted: Vec<&str> = std::iter::once("form")
                    .chain(t.iter().skip(2).copied()).collect();
                let Some((a, b, note)) = footprint(&shifted, anchor) else {
                    rel_err.set(true); continue;
                };
                if let Some(n) = note { assumed!(&n); }
                let nums: Vec<(String, i32)> = t.iter().skip(2)
                    .filter_map(|s| s.split_once('='))
                    .filter_map(|(k, v)| {
                        // pose words for statues: a number the recipe reads
                        let n: Option<i32> = match (k, v) {
                            ("pose", "standing") => Some(0), ("pose", "seated") => Some(1),
                            ("pose", "arms_up") | ("pose", "raised") => Some(2),
                            ("pose", "kneeling") | ("pose", "crouched") => Some(3),
                            _ => v.parse().ok(),
                        };
                        n.map(|n| (k.to_string(), n))
                    })
                    .collect();
                ops.push(Op::Form {
                    name, min: a, max: b,
                    y: kv(&t, "y").and_then(|v| v.parse().ok()),
                    style: t.iter().skip(2)
                        .find(|s| !s.contains('=') && !s.contains(',') && !s.starts_with('@'))
                        .unwrap_or(&"").to_string(),
                    block: blk("stone").unwrap_or(1),
                    accent: blk("dark_oak_planks").unwrap_or(86),
                    nums });
            }
            "roof" => {
                let Some((a, b, note)) = footprint(&t, anchor) else {
                    rel_err.set(true); continue;
                };
                if let Some(n) = note { assumed!(&n); }
                ops.push(Op::Roof {
                    min: a, max: b, y: kv(&t, "y").and_then(|v| v.parse().ok()),
                    style: t.iter().find(|s| ["gable","hip","flat","pyramid"].contains(s))
                            .unwrap_or(&"gable").to_string(),
                    block: blk("oak_planks").unwrap_or(57),
                    overhang: kvi(&t, "overhang", 1).clamp(0, 4) });
            }
            "greeble" | "decorate" => {
                let a = t.get(1).and_then(|s| triple_at(s, anchor).ok().flatten());
                let b = t.get(2).and_then(|s| triple_at(s, anchor).ok().flatten());
                let (Some(mn), Some(mx)) = (a, b) else {
                    bad("greeble needs two x,y,z corners".into()); continue;
                };
                let phrase: Vec<&str> = t.iter().skip(3)
                    .filter(|s| !s.contains('=')).copied().collect();
                ops.push(Op::Greeble {
                    min: mn, max: mx, phrase: phrase.join(" "),
                    gens: kvi(&t, "gens", 12).clamp(1, 60) as u32,
                    seed: kvi(&t, "seed", 1),
                    genome: t.iter().find_map(|s| s.strip_prefix("genome=")).map(|s| s.to_string()),
                });
            }
            "brush" => {
                let Some(op) = t.get(1).map(|s| s.to_lowercase()) else {
                    bad("brush needs an op: raise, lower, smooth, flatten, roughen".into());
                    continue;
                };
                if !["raise", "lower", "smooth", "flatten", "roughen"].contains(&op.as_str()) {
                    bad(format!("unknown brush op '{op}'")); continue;
                }
                let at = rel(&t, 2, anchor).unwrap_or_else(|| anchor_xz(anchor));
                ops.push(Op::Brush {
                    op, at,
                    amount: kvi(&t, "by", kvi(&t, "to", 10)),
                    radius: kvi(&t, "r", 30).clamp(2, 400),
                    falloff: t.iter().skip(2)
                        .find(|s| crate::brush::Falloff::parse(s).is_some())
                        .unwrap_or(&"smooth").to_string(),
                    seed: kvi(&t, "seed", 1),
                });
            }
            "layer" | "paint" => {
                let Some(pk) = t.get(1).map(|s| s.to_lowercase()) else {
                    bad("layer needs a packet name".into()); continue;
                };
                if crate::layer::packet(&pk).is_none() {
                    bad(format!("unknown packet '{pk}'; known: {}",
                        crate::layer::PACKETS.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")));
                    continue;
                }
                let shifted: Vec<&str> = std::iter::once("layer")
                    .chain(t.iter().skip(2).copied()).collect();
                let Some((a, b, note)) = footprint(&shifted, anchor) else {
                    rel_err.set(true); continue;
                };
                if let Some(n) = note { assumed!(&n); }
                ops.push(Op::Layer {
                    packet: pk, min: a, max: b,
                    density: kv(&t, "density").and_then(|v| v.parse().ok())
                        .unwrap_or(0.6f32).clamp(0.0, 1.0),
                    coverage: t.iter().skip(2)
                        .find(|s| ["full", "radial", "gentle"].contains(s)
                              || s.starts_with("above=") || s.starts_with("below="))
                        .unwrap_or(&"full").to_string(),
                    seed: kvi(&t, "seed", 1),
                });
            }
            "lake" | "pond" => {
                let at = rel(&t, 1, anchor).unwrap_or_else(|| anchor_xz(anchor));
                let Some(level) = kv(&t, "level").and_then(|v| v.parse().ok()) else {
                    bad("lake needs level=Y".into()); continue;
                };
                ops.push(Op::Lake { at, radius: kvi(&t, "r", 8).clamp(2, 80), level });
            }
            "road" | "highway" => {
                let Some((a, b, note)) = footprint(&t, anchor) else {
                    rel_err.set(true); continue;
                };
                if let Some(n) = note { assumed!(&n); }
                ops.push(Op::Road { from: a, to: b, width: kvi(&t, "w", 3).clamp(1, 7),
                                    block: blk("cobblestone").unwrap_or(4) });
            }
            "river" | "stream" => {
                let at = rel(&t, 1, anchor).unwrap_or_else(|| anchor_xz(anchor));
                ops.push(Op::River {
                    at,
                    dir_deg: kvi(&t, "dir", 0),
                    max_len: kvi(&t, "len", 200).clamp(10, 2000),
                    seed: kvi(&t, "seed", 1),
                });
            }
            "mood" | "atmosphere" => {
                let Some(name) = t.get(1).map(|s| s.to_lowercase()) else {
                    bad("mood needs a name".into()); continue;
                };
                if crate::mood::lookup(&name).is_none() {
                    bad(format!("unknown mood '{name}'; known: {}",
                        crate::mood::MOODS.iter().map(|m| m.name).collect::<Vec<_>>().join(", ")));
                    continue;
                }
                let shifted: Vec<&str> = std::iter::once("mood")
                    .chain(t.iter().skip(2).copied()).collect();
                let Some((a, b, note)) = footprint(&shifted, anchor) else {
                    rel_err.set(true); continue;
                };
                if let Some(n) = note { assumed!(&n); }
                ops.push(Op::Mood { name, min: a, max: b,
                                    y: kv(&t, "y").and_then(|v| v.parse().ok()) });
            }
            "populate" | "spawn" => {
                // The kind is PLAIN WORDS and may be several: "horrifying
                // creatures", "fire dragon". It runs until the first token
                // that looks like a coordinate or a k=v. Taking only one
                // word made "creatures" a coordinate and dropped the
                // adjective that carried the disposition.
                let is_arg = |s: &str| s.starts_with('@') || s.contains(',') || s.contains('=');
                let kind_words: Vec<&str> = t.iter().skip(1).take_while(|s| !is_arg(s))
                    .copied().collect();
                if kind_words.is_empty() {
                    bad("populate needs a creature kind".into()); continue;
                }
                let kind = kind_words.join(" ");
                let shifted: Vec<&str> = std::iter::once("populate")
                    .chain(t.iter().skip(1 + kind_words.len()).copied()).collect();
                let Some((a, b, note)) = footprint(&shifted, anchor) else {
                    rel_err.set(true); continue;
                };
                if let Some(n) = note { assumed!(&n); }
                ops.push(Op::Populate {
                    kind, min: a, max: b,
                    count: kvi(&t, "count", 3).clamp(1, 32) as u32,
                    name: t.iter().skip(2).find_map(|s| s.strip_prefix("name="))
                        .map(|s| s.to_string()),
                });
            }
            "place" | "stamp" => {
                let Some(name) = t.get(1).map(|s| s.to_string()) else {
                    bad("place needs a template name".into());
                    continue;
                };
                let at = rel(&t, 2, anchor).unwrap_or_else(|| anchor_xz(anchor));
                ops.push(Op::Place {
                    name, at,
                    y: kv(&t, "y").and_then(|v| v.parse().ok()),
                    // Degrees are what a person says; quarter turns are
                    // what the code wants.
                    turns: ((kvi(&t, "rot", 0).rem_euclid(360)) / 90) as u8,
                    mirror: t.contains(&"mirror") || t.contains(&"flip"),
                    vary: kv(&t, "vary").and_then(|v| v.parse().ok())
                        .unwrap_or(0.0f32).clamp(0.0, 0.9),
                    seed: kvi(&t, "seed", 1),
                    palette: t.iter().skip(2)
                        .find_map(|s| s.strip_prefix("palette=")).map(|s| s.to_string()),
                });
            }
            "world" => {
                let verb = t.get(1).map(|s| s.to_lowercase()).unwrap_or_default();
                if crate::intent::WorldIntent::parse(&verb).is_none() {
                    bad(format!("unknown world verb '{verb}'"));
                    continue;
                }
                ops.push(Op::World {
                    verb,
                    arg: t.get(2).and_then(|s| s.parse().ok()),
                    at: rel(&t, 3, anchor).unwrap_or_else(|| anchor_xz(anchor)),
                    radius: kvi(&t, "r", 32).clamp(1, 128),
                });
            }
            "detail" | "finish" => {
                let kind = t.get(1).map(|s| s.to_lowercase()).unwrap_or_default();
                const KINDS: &[&str] = &["speckle", "relief", "trim", "feather", "blend"];
                if !KINDS.contains(&kind.as_str()) {
                    bad(format!("detail needs one of: {}", KINDS.join(", ")));
                    continue;
                }
                let a = t.get(2).and_then(|s| triple_at(s, anchor).ok().flatten());
                let b = t.get(3).and_then(|s| triple_at(s, anchor).ok().flatten());
                match (a, b) {
                    (Some(mn), Some(mx)) => ops.push(Op::Detail {
                        kind, min: mn, max: mx,
                        block: t.iter().skip(2)
                            .find_map(|s| s.strip_prefix("block="))
                            .and_then(blk)
                            .unwrap_or(4),
                        amount: kv(&t, "amount").and_then(|v| v.parse().ok())
                            .unwrap_or(0.25f32).clamp(0.0, 1.0),
                        seed: kvi(&t, "seed", 1) }),
                    _ => bad("detail needs two x,y,z corners".into()),
                }
            }
            "weather" | "ruin" | "age" => {
                let a = t.get(1).and_then(|s| triple_at(s, anchor).ok().flatten());
                let b = t.get(2).and_then(|s| triple_at(s, anchor).ok().flatten());
                match (a, b) {
                    (Some(mn), Some(mx)) => ops.push(Op::Weather {
                        min: mn, max: mx,
                        preset: t.iter().find(|s| ["worn","abandoned","ancient","burnt"]
                                                  .contains(s))
                                 .unwrap_or(&"abandoned").to_string(),
                        seed: kvi(&t, "seed", 1) }),
                    _ => bad("weather needs two x,y,z corners".into()),
                }
            }
            "mirror" => {
                let a = t.get(1).and_then(|s| triple_at(s, anchor).ok().flatten());
                let b = t.get(2).and_then(|s| triple_at(s, anchor).ok().flatten());
                match (a, b) {
                    (Some(mn), Some(mx)) => ops.push(Op::Mirror {
                        min: mn, max: mx,
                        axis: if t.contains(&"z") { "z".into() } else { "x".into() },
                        about: kvi(&t, "about", if t.contains(&"z") { mn.2 } else { mn.0 }) }),
                    _ => bad("mirror needs two x,y,z corners".into()),
                }
            }
            "bridge" | "walkway" => {
                let Some((a, b, note)) = footprint(&t, anchor) else {
                    rel_err.set(true); continue;
                };
                if let Some(n) = note { assumed!(&n); }
                ops.push(Op::Bridge {
                    from: a, to: b, y: kv(&t, "y").and_then(|v| v.parse().ok()),
                    width: kvi(&t, "w", 3).max(1),
                    block: blk("oak_planks").unwrap_or(57),
                    supports: !t.contains(&"nosupports") });
            }
            "stairs" | "staircase" => {
                let Some((a, b, note)) = footprint(&t, anchor) else {
                    rel_err.set(true); continue;
                };
                if let Some(n) = note { assumed!(&n); }
                ops.push(Op::Stairs {
                    from: a, to: b, y: kv(&t, "y").and_then(|v| v.parse().ok()),
                    width: kvi(&t, "w", 2).max(1),
                    block: blk("cobblestone").unwrap_or(4) });
            }
            "floor" | "storey" | "story" => {
                let Some((a, b, note)) = footprint(&t, anchor) else {
                    rel_err.set(true); continue;
                };
                if let Some(n) = note { assumed!(&n); }
                ops.push(Op::Floor {
                    min: a, max: b, y: kv(&t, "y").and_then(|v| v.parse().ok()),
                    block: blk("oak_planks").unwrap_or(57) });
            }
            "plant" | "grow" => {
                // `plant @x,z oak tall ancient [seed=N] [seedling]`
                let at = rel(&t, 1, anchor).unwrap_or_else(|| anchor_xz(anchor));
                let words: Vec<&str> = t.iter().skip(1)
                    .filter(|s| !s.contains(',') && !s.contains('=') && !s.starts_with('@')
                            && **s != "seedling").copied().collect();
                let species = ["oak", "pine", "spruce", "birch", "jungle", "cactus"].iter()
                    .find(|sp| words.contains(sp)).unwrap_or(&"oak").to_string();
                ops.push(Op::Plant { at, species, phrase: words.join(" "),
                                     seed: kvi(&t, "seed", 1), mature: !t.contains(&"seedling") });
            }
            "cylinder" | "tower" => match rel(&t, 1, anchor) {
                Some(a) => ops.push(Op::Cylinder {
                    at: a, y: kv(&t, "y").and_then(|v| v.parse().ok()),
                    // Clamp, do not reject. A model that writes r=0 meant
                    // a tower; "radius must be positive" is a true
                    // statement that builds nothing.
                    radius: clamp_size(kvi(&t, "r", 4), 4, &mut notes, "radius"),
                    height: clamp_size(kvi(&t, "h", 10), 10, &mut notes, "height"),
                    block: blk("stone").unwrap_or(1), hollow }),
                // No coordinate at all means HERE. The player is looking
                // at a spot and asked for a tower; refusing over a
                // missing `@0,0` is pedantry, and both a 1B and an 8B
                // hit exactly this.
                _ if t.get(1).is_some_and(|s| s.starts_with('@')) && anchor.is_none() => {
                    rel_err.set(true);
                }
                _ => { ops.push(Op::Cylinder {
                        at: anchor_xz(anchor), y: kv(&t, "y").and_then(|v| v.parse().ok()),
                        radius: clamp_size(kvi(&t, "r", 4), 4, &mut notes, "radius"),
                        height: clamp_size(kvi(&t, "h", 10), 10, &mut notes, "height"),
                        block: blk("stone").unwrap_or(1), hollow });
                       assumed!("cylinder placed at the focus"); }
            },
            "sphere" | "dome" => match rel(&t, 1, anchor) {
                Some(a) => ops.push(Op::Sphere {
                    at: a, y: kv(&t, "y").and_then(|v| v.parse().ok()),
                    radius: clamp_size(kvi(&t, "r", 5), 5, &mut notes, "radius"),
                    block: blk("stone").unwrap_or(1), hollow }),
                _ if t.get(1).is_some_and(|s| s.starts_with('@')) && anchor.is_none() => {
                    rel_err.set(true);
                }
                _ => { ops.push(Op::Sphere {
                        at: anchor_xz(anchor), y: kv(&t, "y").and_then(|v| v.parse().ok()),
                        radius: clamp_size(kvi(&t, "r", 5), 5, &mut notes, "radius"),
                        block: blk("stone").unwrap_or(1), hollow });
                       assumed!("sphere placed at the focus"); }
            },
            "tree" => match rel(&t, 1, anchor) {
                Some(a) => ops.push(Op::Tree {
                    at: a,
                    species: t.get(2).filter(|s| !s.contains('=')).unwrap_or(&"oak").to_string(),
                    height: kvi(&t, "h", 8),
                    lean: kv(&t, "lean").and_then(|v| pair(&v)).unwrap_or((0, 0)) }),
                _ if t.get(1).is_some_and(|s| s.starts_with('@')) && anchor.is_none() => {
                    rel_err.set(true);
                }
                _ => { ops.push(Op::Tree {
                        at: anchor_xz(anchor),
                        species: t.get(1).filter(|s| !s.contains('=') && !s.contains(','))
                                  .unwrap_or(&"oak").to_string(),
                        height: kvi(&t, "h", 8), lean: (0, 0) });
                       assumed!("tree placed at the focus"); }
            },
            "scatter" => match (rel(&t, 1, anchor), rel(&t, 2, anchor)) {
                (Some(a), Some(b)) => ops.push(Op::Scatter {
                    min: a, max: b, block: blk("flower").unwrap_or(25),
                    clusters: kvi(&t, "clusters", 5).max(0) as usize,
                    per: kvi(&t, "per", 8).max(0) as usize,
                    spread: kvi(&t, "spread", 3),
                    seed: kv(&t, "seed").and_then(|v| v.parse().ok()).unwrap_or(1) }),
                _ => {
                    let Some((a, b, note)) = footprint(&t, anchor) else {
                        rel_err.set(true);
                        continue;
                    };
                    if let Some(n) = note { assumed!(&n); }
                    ops.push(Op::Scatter {
                        min: a, max: b, block: blk("flower").unwrap_or(25),
                        clusters: kvi(&t, "clusters", 4).max(1) as usize,
                        per: kvi(&t, "per", 6).max(1) as usize,
                        spread: kvi(&t, "spread", 3).max(1),
                        seed: kvi(&t, "seed", 1) as u64 });
                }
            },
            "restyle" => {
                let a = t.get(1).map(|s| triple_at(s, anchor));
                let b = t.get(2).map(|s| triple_at(s, anchor));
                match (a, b, t.get(3), t.get(4)) {
                    (Some(Ok(Some(mn))), Some(Ok(Some(mx))), Some(from), Some(to)) =>
                        ops.push(Op::Restyle { min: mn, max: mx,
                                               from: from.to_string(), to: to.to_string() }),
                    (Some(Err(_)), _, _, _) | (_, Some(Err(_)), _, _) => rel_err.set(true),
                    _ => bad("restyle needs two x,y,z triples and two palette names".into()),
                }
            }
            "fill" | "box" => {
                let a = t.get(1).map(|s| triple_at(s, anchor));
                let b = t.get(2).map(|s| triple_at(s, anchor));
                match (a, b) {
                    (Some(Ok(Some(mn))), Some(Ok(Some(mx)))) => ops.push(Op::Fill {
                        min: mn, max: mx, block: blk("stone").unwrap_or(1), hollow }),
                    (Some(Err(_)), _) | (_, Some(Err(_))) => rel_err.set(true),
                    _ => bad("fill needs two x,y,z triples".into()),
                }
            }
            // "line N:" PREFIX, not a trailing parenthetical: salvage
            // parses the first "line " it finds, and a suffix form put
            // the number after the text where it was never looked for.
            other => issues.push(Issue {
                severity: Severity::Error, op: idx,
                message: format!("line {}: unknown op '{other}'", lineno + 1) }),
        }
        for n in notes.drain(..) {
            issues.push(Issue { severity: Severity::Warning, op: ops.len().max(1),
                message: format!("line {}: {n}", lineno + 1) });
        }
        if rel_err.get() {
            issues.push(Issue { severity: Severity::Error, op: idx,
                message: format!("line {}: @relative coordinate but no focus anchor \
                                  (look at something first)", lineno + 1) });
        }
    }
    (BuildPlan { name, ops, anchor }, issues)
}

/// Static checks that need no world access.
pub fn validate(plan: &BuildPlan) -> Vec<Issue> {
    let mut out = Vec::new();
    if plan.ops.is_empty() {
        out.push(Issue { severity: Severity::Error, op: 0, message: "plan has no operations".into() });
    }
    for (i, op) in plan.ops.iter().enumerate() {
        let n = i + 1;
        // Plain pushes rather than closures: two closures capturing
        // the same Vec mutably will not compile, and a helper reads
        // better than fighting the borrow checker over ergonomics.
        macro_rules! err  { ($m:expr) => { out.push(Issue { severity: Severity::Error,   op: n, message: $m.into() }) } }
        macro_rules! warn { ($m:expr) => { out.push(Issue { severity: Severity::Warning, op: n, message: $m.into() }) } }
        match op {
            Op::Cylinder { radius, height, .. } => {
                // Clamped at parse time, so this can only fire on a plan
                // built in code. A model writing r=0 gets a small tower
                // and a note, not a rejection.
                if *radius <= 0 || *height <= 0 { err!("cylinder radius and height must be positive"); }
                if *radius > 64 { warn!("radius over 64 is very large"); }
            }
            Op::Sphere { radius, .. } => {
                if *radius <= 0 { err!("sphere radius must be positive"); }
            }
            Op::Tree { height, species, .. } => {
                if *height < 3 { warn!("tree shorter than 3 will not look like a tree"); }
                if !["oak", "birch", "pine", "spruce", "cactus"].contains(&species.as_str()) {
                    warn!("unknown species; will fall back to oak");
                }
            }
            Op::Path { from, to, width, .. } => {
                let d = (to.0 - from.0).abs().max((to.1 - from.1).abs());
                if d == 0 { warn!("path start and end are the same point"); }
                if *width <= 0 { err!("path width must be positive"); }
            }
            Op::Restyle { from, to, .. } => {
                let p = palettes();
                if !p.contains_key(from.as_str()) { err!("unknown source palette"); }
                if !p.contains_key(to.as_str()) { err!("unknown target palette"); }
            }
            _ => {}
        }
    }
    // Ordering: terraform after something has been built on that
    // ground will bury or behead it. Cheap to check, expensive to
    // discover by eye.
    let first_build = plan.ops.iter().position(|o| !matches!(o, Op::Terraform { .. }));
    if let Some(fb) = first_build {
        for (i, op) in plan.ops.iter().enumerate().skip(fb) {
            if matches!(op, Op::Terraform { .. }) {
                out.push(Issue { severity: Severity::Warning, op: i + 1,
                    message: "terraform after a build op; ground changes may bury or float it".into() });
            }
        }
    }
    out
}

/// One animation step: writes that belong to the same chunk, so the
/// mesher rebuilds each affected chunk once per tick instead of once
/// per block.
#[derive(Debug, Clone)]
pub struct Batch {
    pub tick: u32,
    pub chunk: (i32, i32, i32),
    pub blocks: Vec<(i32, i32, i32, u16)>,
}

/// Result of running a plan.
#[derive(Debug, Default)]
pub struct PlanRun {
    /// Dependents withdrawn after a structural failure. Empty when
    /// nothing failed or nothing depended on what did.
    pub withdrawn: Vec<crate::recovery::Withdrawal>,
    /// Living plants this plan asks the engine to register. The crate
    /// placed their current form; the engine owns them from here.
    pub plants: Vec<crate::flora::PlantRequest>,
    pub applied: usize,
    pub failed: usize,
    pub blocks_changed: usize,
    pub log: Vec<String>,
    /// Chunk-grouped animation batches, in commit order.
    pub batches: Vec<Batch>,
}

/// Execute a plan. `chunk_size` should match the engine's (32 in
/// unendlich). `dry_run` applies everything and then reverses it, so
/// the caller gets the full predicted effect with the world unchanged
/// -- this is what powers "show me what that would do".
pub fn run_plan<W: VoxelWorld>(
    tools: &mut VoxelTools<W>,
    plan: &BuildPlan,
    from_y: i32,
    chunk_size: i32,
    dry_run: bool,
) -> PlanRun {
    run_plan_with(tools, plan, from_y, chunk_size, dry_run, None)
}

/// As `run_plan`, with the template library so `place` can resolve.
pub fn run_plan_with<W: VoxelWorld>(
    tools: &mut VoxelTools<W>,
    plan: &BuildPlan,
    from_y: i32,
    chunk_size: i32,
    dry_run: bool,
    library: Option<&crate::template::Library>,
) -> PlanRun {
    // GROUND SEARCH CEILING.
    //
    // `find_surface` scans DOWN from here, so a fixed value is wrong
    // in both directions and silently so:
    //
    //   * on a mountain above the ceiling, every op lands at the
    //     ceiling — a tower built 50 blocks BELOW the player;
    //   * in a cave, the scan starts above the roof and finds the
    //     outdoor surface, so the build appears 100 blocks overhead,
    //     through the ceiling, out of sight.
    //
    // Neither errors. You just look at empty ground and conclude the
    // feature is broken. Anchoring the search just above the focus —
    // the block the player is actually looking at — makes "here" mean
    // here at any altitude, indoors or out. The caller's `from_y` is
    // only a fallback for plans parsed without an anchor.
    let from_y = match plan.anchor {
        Some(a) => a.1 + GROUND_SEARCH_MARGIN,
        None => from_y,
    };
    let mut run = PlanRun { applied: 0, failed: 0, blocks_changed: 0,
                            log: Vec::new(), batches: Vec::new(), withdrawn: Vec::new(),
                            plants: Vec::new() };
    let mut outcomes: Vec<(bool, String, Option<u64>)> = Vec::new();
    let first_edit = tools.edits.len();

    for (i, op) in plan.ops.iter().enumerate() {
        let r: CallResult = match op {
            Op::Terraform { min, max, delta, smooth, block } =>
                tools.terraform(*min, *max, *delta, *smooth, *block, from_y),
            Op::Path { from, to, width, block } =>
                tools.connect_path(*from, *to, *width, *block, from_y),
            Op::Enclose { min, max, height, block } =>
                tools.enclose(*min, *max, *height, *block, from_y),
            Op::Fill { min, max, block, hollow } =>
                tools.fill_box(*min, *max, *block, *hollow),
            Op::Cylinder { at, y, radius, height, block, hollow } => {
                let base_y = y.unwrap_or_else(|| tools.find_surface(at.0, at.1, from_y)
                                                    .map(|s| s + 1).unwrap_or(from_y));
                tools.cylinder((at.0, base_y, at.1), *radius, *height, *block, *hollow)
            }
            Op::Roof { min, max, y, style, block, overhang } => {
                // Sits on top of the walls: the highest surface under the
                // footprint, not the ground, or a roof lands in the floor.
                let base = y.unwrap_or_else(|| {
                    let mut top = i32::MIN;
                    for x in [min.0, max.0, (min.0 + max.0) / 2] {
                        for z in [min.1, max.1, (min.1 + max.1) / 2] {
                            if let Some(s) = tools.find_surface(x, z, from_y) { top = top.max(s); }
                        }
                    }
                    if top == i32::MIN { from_y } else { top + 1 }
                });
                tools.roof((min.0, base, min.1), *max, style, *block, *overhang)
            }
            Op::Views { views, at, y, depth, block } => {
                use crate::views::{check_views, Extrusion, Mask, View};
                let base = y.unwrap_or_else(|| {
                    tools.find_surface(at.0, at.1, from_y).map(|s| s + 1).unwrap_or(from_y)
                });
                let mut ex = Vec::new();
                for (name, art) in views {
                    let mask = Mask::parse(art);
                    if mask.is_empty() { continue; }
                    let (view, origin) = match name.as_str() {
                        "side" => (View::Side, [at.0, base + mask.h as i32 - 1, at.1]),
                        "top" | "plan" => (View::Top, [at.0, base, at.1]),
                        _ => (View::Front, [at.0, base + mask.h as i32 - 1, at.1]),
                    };
                    ex.push(Extrusion { mask, view, origin, depth: *depth,
                                        mat: crate::solid::Material::structural(*block) });
                }
                if ex.is_empty() {
                    CallResult::err("no readable drawings in the views block".to_string())
                } else {
                    let (cells, checks) = check_views(&ex);
                    let mut res = tools.stamp_cells(&cells, *block);
                    // Report fidelity: a hull can only be as tight as the
                    // silhouettes allow, and the player should hear that
                    // from the tool rather than from the finished build.
                    let off: Vec<String> = checks.iter().filter(|c| !c.ok())
                        .map(|c| format!("{:?} +{} -{}", c.view, c.extra, c.missing))
                        .collect();
                    if !off.is_empty() {
                        // A hull is only as tight as its silhouettes. Say
                        // so here rather than letting it be discovered by
                        // looking at the finished build.
                        res.summary = format!(
                            "{} | views not exact: {} (add a third view to tighten)",
                            res.summary, off.join(", "));
                    }
                    res
                }
            }
            Op::Form { name, min, max, y, style, block, accent, nums } => {
                let base = y.unwrap_or_else(|| {
                    tools.find_surface(min.0, min.1, from_y).map(|s| s + 1).unwrap_or(from_y)
                });
                let args = crate::dict::FormArgs {
                    min: (min.0, base, min.1), max: (max.0, base, max.1),
                    style: style.clone(), block: *block, accent: *accent,
                    nums: nums.clone() };
                match crate::dict::lookup(name) {
                    Some(f) => tools.stamp(&(f.build)(&args)),
                    None => CallResult::err(format!("unknown form '{name}'")),
                }
            }
            Op::Greeble { min, max, phrase, gens, seed, genome } => (|| {
                use crate::gene;
                let f = gene::faces(tools.world, *min, *max);
                if f.is_empty() {
                    return CallResult { ok: false, summary: "greeble: no exposed surface in that region".into(),
                                        ..Default::default() };
                }
                let (changes, note) = match genome {
                    Some(g) => match gene::Genome::parse(&g.replace(';', "\n")) {
                        Ok(gn) => { let c = gene::run(&gn, &f, *seed as u64);
                                    let m = gene::measure(&f, &c);
                                    (c, format!("stated genome: coverage {:.0}%, noise {:.2}",
                                                m.coverage * 100.0, m.noise)) }
                        Err(e) => return CallResult { ok: false, summary: format!("greeble: bad genome: {e}"),
                                                      ..Default::default() },
                    },
                    None => {
                        let fit = gene::Fitness::from_phrase(phrase);
                        let e = gene::evolve(&f, &fit, *seed as u64, 24, *gens as usize);
                        let m = e.metrics;
                        (e.changes, format!(
                            "evolved '{}' over {} faces in {gens} generations: score {:.2}, \
                             coverage {:.0}%, edges {:.0}%, depth {:.0}%, noise {:.2}, symmetry {:.2}\n{}",
                            if phrase.is_empty() { "default" } else { phrase }, f.len(), e.score,
                            m.coverage * 100.0, m.edges * 100.0, m.depth * 100.0, m.noise, m.symmetry,
                            e.genome.to_text().trim()))
                    }
                };
                let mut r = tools.apply_changes(&changes);
                if r.ok { r.summary = format!("greeble: {note}"); }
                r
            })(),
            Op::Brush { op, amount, at, radius, falloff, seed } => {
                use crate::brush::{self, Brush, BrushOp, Falloff};
                let bop = match op.as_str() {
                    "raise" => BrushOp::Raise(*amount),
                    "lower" => BrushOp::Lower(*amount),
                    "smooth" => BrushOp::Smooth((*amount as f32 / 10.0).clamp(0.1, 1.0)),
                    "flatten" => BrushOp::Flatten(*amount),
                    _ => BrushOp::Roughen(*amount),
                };
                let b = Brush { centre: *at, radius: *radius,
                                falloff: Falloff::parse(falloff).unwrap_or(Falloff::Smooth),
                                op: bop, seed: *seed as u64 };
                let plan = brush::plan(tools.world, &b, from_y, 2);
                let summary = plan.summary();
                let mut r = tools.apply_changes_large(&plan.changes);
                // Keep a refusal VISIBLE. The first version overwrote the
                // summary unconditionally, so a brush refused for size
                // reported its full planned count as if placed -- and the
                // mesa that was not there looked like a rendering bug.
                if r.ok { r.summary = format!("brush {op}: {summary}"); }
                else { r.summary = format!("brush {op} REFUSED: {}", r.summary); }
                r
            }
            Op::Layer { packet, min, max, density, coverage, seed } => {
                use crate::layer::{self, Coverage, Layer};
                let cov = match coverage.as_str() {
                    "radial" => Coverage::Radial,
                    "gentle" => Coverage::Gentle(1.0),
                    s if s.starts_with("above=") =>
                        Coverage::Above(s[6..].parse().unwrap_or(from_y)),
                    s if s.starts_with("below=") =>
                        Coverage::Below(s[6..].parse().unwrap_or(from_y)),
                    _ => Coverage::Full,
                };
                let l = Layer { name: packet.clone(), min: *min, max: *max,
                                packet: layer::packet(packet).expect("parser checked"),
                                density: *density, coverage: cov, seed: *seed as u64 };
                let baked = layer::bake(tools.world, &l, from_y);
                // The bake is IR: run it as a nested plan so every tree
                // goes through the same tree op as a hand-written one.
                let (sub, _) = parse_plan_at(&baked.ir, None);
                let sr = run_plan_with(tools, &sub, from_y, chunk_size, dry_run, library);
                CallResult {
                    ok: sr.failed == 0, blocks_changed: sr.blocks_changed,
                    summary: format!("layer {}: {} trees, {} cover, {} rocks, {} repainted \
                                      -> {} blocks", packet, baked.trees, baked.cover,
                                     baked.rocks, baked.repainted, sr.blocks_changed),
                    ..Default::default()
                }
            }
            Op::Lake { at, radius, level } => {
                let eff = crate::godmode::flood(tools.world, *at, *radius, *level, 9);
                let mut r = tools.apply_changes_large(&eff.blocks);
                if r.ok { r.summary = format!("lake: {}", eff.note); }
                r
            }
            Op::Road { from, to, width, block: _ } => {
                use crate::feature::{Reader, Road};
                let r = Reader::new(tools.world, from_y);
                let mut road = Road::new(*from, *to, *width);
                let ir = road.run(&r);
                let desc = road.describe();
                let (sub, _) = parse_plan_at(&ir, None);
                let sr = run_plan_with(tools, &sub, from_y, chunk_size, dry_run, library);
                CallResult {
                    ok: sr.failed == 0 && road.done(), blocks_changed: sr.blocks_changed,
                    summary: format!("{desc} -> {} blocks", sr.blocks_changed),
                    ..Default::default()
                }
            }
            Op::River { at, dir_deg, max_len, seed } => {
                use crate::feature::{Reader, River};
                let r = Reader::new(tools.world, from_y);
                let mut river = River::new(at.0, at.1, (*dir_deg as f32).to_radians(), *max_len);
                river.seed = *seed as u64;
                let ir = river.run(&r);
                let desc = river.describe();
                let (sub, _) = parse_plan_at(&ir, None);
                let sr = run_plan_with(tools, &sub, from_y, chunk_size, dry_run, library);
                CallResult {
                    ok: sr.failed == 0, blocks_changed: sr.blocks_changed,
                    summary: format!("{desc} -> {} blocks", sr.blocks_changed),
                    ..Default::default()
                }
            }
            Op::Mood { name, min, max, y } => {
                let Some(m) = crate::mood::lookup(name) else {
                    // Unreachable: the parser rejects unknown moods.
                    return PlanRun { log: vec![format!("unknown mood '{name}'")],
                                     ..Default::default() };
                };
                let base = y.unwrap_or_else(|| {
                    tools.find_surface(min.0, min.1, from_y).unwrap_or(from_y)
                });
                let lo = (min.0.min(max.0), base - 4, min.1.min(max.1));
                let hi = (min.0.max(max.0), base + 24, min.1.max(max.1));
                let mut n = 0usize;
                // Restyle toward the mood's palette, from every palette
                // we know: the build may have been made in any of them.
                for from in crate::palettes().keys() {
                    if *from == m.palette { continue; }
                    n += tools.restyle(lo, hi, from, m.palette).blocks_changed;
                }
                if let Some(w) = m.weather {
                    let cfg = crate::weather::Weather::preset(w);
                    let ch = crate::weather::plan(tools.world, lo, hi, &cfg, 23, 29, 4);
                    n += tools.apply_changes(&ch).blocks_changed;
                }
                for (block, density) in m.scatter {
                    let per = ((hi.0 - lo.0) as f32 * (hi.2 - lo.2) as f32 * density * 0.05)
                        .max(1.0) as usize;
                    n += tools.scatter_clustered((lo.0, lo.2), (hi.0, hi.2), from_y, *block,
                                                 (per / 4).max(1), 4, 3, 7).blocks_changed;
                }
                let at = m.atmosphere;
                let mut r = CallResult { ok: true, blocks_changed: n, ..Default::default() };
                r.summary = format!(
                    "mood {}: {n} blocks | engine: fog {:.1} light {:.1}{}{}",
                    m.name, at.fog, at.light,
                    at.time.map(|t| format!(" time {t:.2}")).unwrap_or_default(),
                    if at.warmth != 0.0 { format!(" warmth {:+.2}", at.warmth) } else { String::new() });
                r
            }
            Op::Populate { kind, count, min, max, name } => {
                // Nothing to write: the request is reported for the
                // engine, which owns the roster. The summary carries every
                // field so the game can act on it from the run log.
                let disp = crate::mood::Disposition::infer(kind);
                let mut r = CallResult { ok: true, ..Default::default() };
                r.summary = format!(
                    "engine: spawn {count} x '{kind}' ({}) in {:?}..{:?}{}",
                    disp.word(), min, max,
                    name.as_ref().map(|n| format!(" named {n}")).unwrap_or_default());
                r
            }
            Op::Place { name, at, y, turns, mirror, vary, seed, palette } => {
                match library.and_then(|l| l.get(name)) {
                    None => CallResult::err(format!(
                        "no template '{name}'{}",
                        match library {
                            Some(l) if !l.is_empty() =>
                                format!("; known: {}", l.names().join(", ")),
                            _ => " (capture one first)".into(),
                        })),
                    Some(t) => {
                        let base = y.unwrap_or_else(|| {
                            tools.find_surface(at.0, at.1, from_y).map(|s| s + 1)
                                .unwrap_or(from_y)
                        });
                        let stamp = crate::template::Stamp {
                            turns: *turns, mirror: *mirror,
                            vary: *vary, seed: *seed as u64 };
                        let pal = palette.clone().unwrap_or_else(|| t.palette.clone());
                        let changes = crate::template::place(
                            t, (at.0, base, at.1), &stamp, &pal);
                        tools.apply_changes(&changes)
                    }
                }
            }
            Op::World { verb, arg, at, radius } => {
                use crate::godmode;
                use crate::intent::WorldIntent as I;
                // By the time an op reaches here the gate has already
                // passed it; an ungated one would have been removed.
                let eff = match I::parse(verb) {
                    Some(I::Hotter) => godmode::temperature(*at, *radius, 0.2),
                    Some(I::Colder) => godmode::temperature(*at, *radius, -0.2),
                    Some(I::Wetter) => godmode::humidity(*at, *radius, 0.2),
                    Some(I::Drier) => godmode::humidity(*at, *radius, -0.2),
                    Some(I::Flood) => {
                        let level = arg.unwrap_or_else(|| {
                            tools.find_surface(at.0, at.1, from_y).unwrap_or(from_y) + 2
                        });
                        godmode::flood(tools.world, *at, *radius, level, 9)
                    }
                    Some(I::Freeze) =>
                        godmode::freeze(tools.world, *at, *radius, from_y, 9, 31, 32),
                    // Unreachable in practice: the parser rejects an
                    // unknown verb. An empty effect keeps this an
                    // expression rather than an early return out of the
                    // whole plan run.
                    None => godmode::Effect {
                        note: format!("unknown world verb '{verb}'"), ..Default::default() },
                };
                // Blocks are applied; FIELD requests are not ours to
                // apply and are reported for the game to act on.
                let mut r = tools.apply_changes(&eff.blocks);
                let fields: Vec<String> = eff.fields.iter().map(|f| f.describe()).collect();
                r.summary = if fields.is_empty() {
                    format!("{}: {}", verb, eff.note)
                } else {
                    format!("{}: {} | engine: {}", verb, eff.note, fields.join("; "))
                };
                r
            }
            Op::Detail { kind, min, max, block, amount, seed } => {
                use crate::detail;
                let s = *seed as u64;
                let changes = match kind.as_str() {
                    // Variants chosen to be RELATED to what is there:
                    // a mix of cobble, mossy cobble and andesite reads as
                    // masonry, while a mix of anything reads as noise.
                    "speckle" => detail::speckle(tools.world, *min, *max, 0,
                                                 &[4, 81, 43], *amount, s),
                    "relief" => detail::relief(tools.world, *min, *max, *amount, s),
                    "trim" => detail::trim(tools.world, *min, *max, *block, *amount, s),
                    // Width is the region's own span on the feather axis,
                    // so the ramp always covers the band asked for.
                    "feather" => detail::feather(tools.world, *min, *max, 0, min.0,
                                                 (max.0 - min.0).abs().max(1),
                                                 *block, None, s),
                    _ => detail::ground_blend(tools.world, (min.0, min.2), (max.0, max.2),
                                              min.1, *block, 4, *amount, s),
                };
                tools.apply_changes(&changes)
            }
            Op::Weather { min, max, preset, seed } => {
                let cfg = crate::weather::Weather {
                    seed: *seed as u64, ..crate::weather::Weather::preset(preset) };
                let changes = crate::weather::plan(
                    tools.world, *min, *max, &cfg,
                    /* moss */ 23, /* vine */ 29, /* rubble */ 4);
                tools.apply_changes(&changes)
            }
            Op::Mirror { min, max, axis, about } => tools.mirror(*min, *max, axis, *about),
            Op::Bridge { from, to, y, width, block, supports } => {
                let base = y.unwrap_or_else(|| {
                    tools.find_surface(from.0, from.1, from_y).map(|s| s + 1).unwrap_or(from_y)
                });
                tools.bridge((from.0, base, from.1), *to, *width, *block, *supports)
            }
            Op::Stairs { from, to, y, width, block } => {
                let base = y.unwrap_or_else(|| {
                    tools.find_surface(from.0, from.1, from_y).map(|s| s + 1).unwrap_or(from_y)
                });
                tools.stairs((from.0, base, from.1), *to, *width, *block)
            }
            Op::Floor { min, max, y, block } => {
                let base = y.unwrap_or_else(|| {
                    tools.find_surface(min.0, min.1, from_y).map(|s| s + 1).unwrap_or(from_y)
                });
                tools.floor((min.0, base, min.1), *max, *block)
            }
            Op::Sphere { at, y, radius, block, hollow } => {
                let cy = y.unwrap_or_else(|| {
                    if *block == 0 {
                        // Carving: a sphere of air with no explicit y is a
                        // doorway or a tunnel mouth. Measure from the
                        // ANCHOR, not from find_surface at the door's own
                        // column -- by the time a door is cut, that column
                        // is the wall, and its "surface" is the parapet ten
                        // blocks up. Two bugs in one line, both only
                        // visible by running a two-turn session.
                        let ground = plan.anchor.map(|a| a.1)
                            .unwrap_or_else(|| tools.find_surface(at.0, at.1, from_y)
                                                    .unwrap_or(from_y));
                        ground + radius / 2
                    } else {
                        tools.find_surface(at.0, at.1, from_y)
                            .map(|s| s + radius + 1).unwrap_or(from_y)
                    }
                });
                tools.sphere((at.0, cy, at.1), *radius, *block, *hollow)
            }
            Op::Tree { at, species, height, lean } => {
                let sy = tools.find_surface(at.0, at.1, from_y).map(|s| s + 1).unwrap_or(from_y);
                tools.place_tree((at.0, sy, at.1), species, *height, *lean)
            }
            Op::Plant { at, species, phrase, seed, mature } => {
                use crate::flora;
                let sy = tools.find_surface(at.0, at.1, from_y).map(|s| s + 1).unwrap_or(from_y);
                // Breed toward the words using the engine's own cross,
                // from wild stock seeded by the position -- so "tall
                // oaks" here are the tall end of THIS world's oaks.
                let bred = flora::breed_toward(phrase, *seed as u64 ^ ((at.0 as u64) << 32) ^ at.1 as u64,
                                               24, 30);
                let mut r = if *mature {
                    tools.place_tree_genome((at.0, sy, at.1), species, &bred.genome, *seed as u64)
                } else {
                    // a seedling: one sapling block; the lifecycle grows it
                    tools.set_block(at.0, sy, at.1, 6)
                };
                let req = flora::PlantRequest {
                    at: (at.0, sy, at.1),
                    species_block: match species.as_str() { "birch" => 19, "cactus" => 21, _ => 17 },
                    genome: bred.genome.as_array(),
                    growth: if *mature { 1.0 } else { 0.02 },
                };
                run.plants.push(req);
                if r.ok {
                    r.summary = format!("{} (bred {} gen(s) toward '{}'; registered with the engine)",
                                        r.summary, bred.generations, phrase);
                }
                r
            }
            Op::Scatter { min, max, block, clusters, per, spread, seed } =>
                tools.scatter_clustered(*min, *max, from_y, *block, *clusters, *per, *spread, *seed),
            Op::Restyle { min, max, from, to } =>
                tools.restyle(*min, *max, from, to),
        };
        if r.ok {
            run.applied += 1;
            run.blocks_changed += r.blocks_changed;
        } else {
            run.failed += 1;
        }
        outcomes.push((r.ok, r.summary.clone(), r.edit_id));
        run.log.push(format!("{}. {}", i + 1, r.summary));
    }

    // ROLLBACK DEPENDENTS. A wall refused for size used to leave the
    // roof that came after it floating on nothing, because the run
    // simply continued. Now a structural failure withdraws every later
    // op that stood on structure, and says so.
    let ok: Vec<bool> = outcomes.iter().map(|o| o.0).collect();
    let why: Vec<String> = outcomes.iter().map(|o| o.1.clone()).collect();
    let withdrawn = crate::recovery::plan_rollback(&plan.ops, &ok, &why);
    for w in &withdrawn {
        for idx in &w.rolled_back {
            if let Some((_, _, Some(id))) = outcomes.get(idx - 1) {
                let u = tools.undo_edit(*id);
                run.blocks_changed = run.blocks_changed.saturating_sub(u.blocks_changed);
            }
        }
        run.log.push(w.message());
    }
    run.withdrawn = withdrawn;

    // Group everything this plan wrote into per-chunk batches, in the
    // order the ops ran. The tick is the op index, so an animation can
    // reveal the build in the order it was authored.
    for (op_i, edit) in tools.edits[first_edit..].iter().enumerate() {
        let mut by_chunk: std::collections::BTreeMap<(i32, i32, i32), Vec<(i32, i32, i32, u16)>> =
            Default::default();
        for &(x, y, z, _prev) in &edit.before {
            let c = (x.div_euclid(chunk_size), y.div_euclid(chunk_size), z.div_euclid(chunk_size));
            // the NEW block is what the mesher needs; read it back
            by_chunk.entry(c).or_default().push((x, y, z, 0));
        }
        for (chunk, blocks) in by_chunk {
            run.batches.push(Batch { tick: op_i as u32, chunk, blocks });
        }
    }

    if dry_run {
        // Reverse in creation order, newest first, so the world is
        // byte-identical afterwards.
        let ids: Vec<u64> = tools.edits[first_edit..].iter().map(|e| e.id).collect();
        for id in ids.into_iter().rev() {
            tools.undo_edit(id);
        }
    }
    run
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Flat { cells: BTreeMap<(i32, i32, i32), u16> }
    impl Flat { fn new() -> Self { Self { cells: BTreeMap::new() } } }
    impl VoxelWorld for Flat {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.cells.get(&(x, y, z)) { return b; }
            if y <= 64 { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool {
            self.cells.insert((x, y, z), b); true
        }
    }

    const SAMPLE: &str = "\
# hilltop camp
terraform 20,-10 34,4 smooth block=stone
path 0,6 27,-3 w=3 block=cobblestone
cylinder 27,-3 r=4 h=12 block=stone hollow
tree -12,-8 oak h=8 lean=1,0
scatter -20,-20 20,20 block=oak_leaves clusters=6 per=10 spread=3 seed=7
";

    #[test]
    fn parses_the_line_ir() {
        let (plan, issues) = parse_plan(SAMPLE);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(plan.ops.len(), 5);
        assert_eq!(plan.name, "hilltop camp");
        match &plan.ops[2] {
            Op::Cylinder { radius, height, hollow, .. } => {
                assert_eq!((*radius, *height, *hollow), (4, 12, true));
            }
            other => panic!("expected cylinder, got {other:?}"),
        }
    }

    #[test]
    fn one_bad_line_costs_one_op_not_the_plan() {
        let (plan, issues) = parse_plan("tree 0,0 oak h=6\nfrobnicate the thing\ntree 5,5 birch h=7\n");
        assert_eq!(plan.ops.len(), 2, "good lines must survive");
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("frobnicate"));
    }

    #[test]
    fn block_names_and_palette_roles_resolve() {
        assert_eq!(resolve_block("cobblestone"), Some(4));
        assert_eq!(resolve_block("oak.primary"), Some(57));
        assert_eq!(resolve_block("volcanic.secondary"), Some(49));
        assert_eq!(resolve_block("58"), Some(58));
        assert_eq!(resolve_block("nonsense"), None);
    }

    #[test]
    fn validation_flags_terraform_after_build() {
        let (plan, _) = parse_plan("tree 0,0 oak h=6\nterraform 0,0 10,10 delta=3\n");
        let issues = validate(&plan);
        assert!(issues.iter().any(|i| i.message.contains("terraform after a build")),
                "{issues:?}");
    }

    #[test]
    fn validation_catches_impossible_geometry() {
        // r=0 from a LINE is clamped at parse time now, so build the bad
        // plan directly: validate() is still the backstop for a plan
        // constructed in code.
        let plan = BuildPlan {
            name: "t".into(),
            ops: vec![Op::Cylinder { at: (0, 0), y: None, radius: 0, height: 5,
                                     block: 1, hollow: false }],
            anchor: None,
        };
        let issues = validate(&plan);
        assert!(issues.iter().any(|i| i.severity == Severity::Error), "{issues:?}");
        // and from a line, it is repaired rather than refused
        let (p2, i2) = parse_plan("cylinder 0,0 r=0 h=5\n");
        assert!(validate(&p2).iter().all(|i| i.severity != Severity::Error));
        assert!(i2.iter().any(|i| i.message.contains("radius was 0")), "{i2:?}");
    }

    #[test]
    fn dry_run_leaves_the_world_untouched() {
        let mut w = Flat::new();
        let before: Vec<u16> = (60..80).map(|y| w.get(27, y, -3)).collect();
        let (plan, _) = parse_plan(SAMPLE);
        let run = {
            let mut t = VoxelTools::new(&mut w);
            run_plan(&mut t, &plan, 200, 32, true)
        };
        assert!(run.blocks_changed > 100, "dry run should still report the effect");
        let after: Vec<u16> = (60..80).map(|y| w.get(27, y, -3)).collect();
        assert_eq!(before, after, "dry run must not modify the world");
    }

    #[test]
    fn commit_actually_builds() {
        let mut w = Flat::new();
        let (plan, _) = parse_plan(SAMPLE);
        let run = {
            let mut t = VoxelTools::new(&mut w);
            run_plan(&mut t, &plan, 200, 32, false)
        };
        assert_eq!(run.failed, 0, "{:?}", run.log);
        // the tower is `hollow`, so its centre is air by design and
        // the wall sits at radius 3-4 from the axis
        let shell = (2..=4).any(|dx| w.get(27 + dx, 66, -3) != 0);
        assert!(shell, "the tower wall should exist after a real run");
        assert_eq!(w.get(27, 66, -3), 0, "and its interior should be hollow");
    }

    #[test]
    fn batches_are_grouped_by_chunk_for_the_mesher() {
        let mut w = Flat::new();
        let (plan, _) = parse_plan("cylinder 0,0 r=3 h=40 block=stone\n");
        let run = {
            let mut t = VoxelTools::new(&mut w);
            run_plan(&mut t, &plan, 200, 32, false)
        };
        // a 40-tall tower from y=65 crosses a chunk boundary at y=96
        let chunks: std::collections::BTreeSet<_> =
            run.batches.iter().map(|b| b.chunk).collect();
        assert!(chunks.len() >= 2, "tall build should span chunks, got {chunks:?}");
        // and every batch belongs to exactly one chunk
        for b in &run.batches {
            for &(x, y, z, _) in &b.blocks {
                assert_eq!((x.div_euclid(32), y.div_euclid(32), z.div_euclid(32)), b.chunk);
            }
        }
    }

    #[test]
    fn y_is_taken_from_the_ground_unless_given() {
        let mut w = Flat::new();
        let (plan, _) = parse_plan("cylinder 0,0 r=2 h=4 block=stone\n");
        {
            let mut t = VoxelTools::new(&mut w);
            run_plan(&mut t, &plan, 200, 32, false);
        }
        assert_eq!(w.get(0, 65, 0), 1, "should start just above the surface");
        assert_eq!(w.get(0, 70, 0), 0, "and not float higher up");
    }

    #[test]
    fn relative_coords_resolve_against_a_focus_anchor() {
        let (plan, issues) = parse_plan_at("tower @0,0 r=4 h=12\npath @-5,0 @20,3 w=3\n",
                                           Some((100, 65, -40)));
        assert!(issues.is_empty(), "{issues:?}");
        match &plan.ops[0] {
            Op::Cylinder { at, .. } => assert_eq!(*at, (100, -40)),
            o => panic!("{o:?}"),
        }
        match &plan.ops[1] {
            Op::Path { from, to, .. } => {
                assert_eq!(*from, (95, -40));
                assert_eq!(*to, (120, -37));
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn relative_coords_without_an_anchor_are_an_error_not_a_guess() {
        let (plan, issues) = parse_plan_at("tower @0,0 r=4 h=12\n", None);
        assert!(plan.ops.is_empty(), "the op must not be placed somewhere arbitrary");
        assert!(issues.iter().any(|i| i.message.contains("focus anchor")), "{issues:?}");
    }

    #[test]
    fn absolute_and_relative_can_mix() {
        let (plan, issues) = parse_plan_at("path 0,0 @10,10 w=2\n", Some((50, 65, 50)));
        assert!(issues.is_empty(), "{issues:?}");
        match &plan.ops[0] {
            Op::Path { from, to, .. } => { assert_eq!(*from, (0, 0)); assert_eq!(*to, (60, 60)); }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn carving_a_doorway_cuts_the_wall_not_the_sky() {
        let mut w = Flat::new();
        let (tower, _) = parse_plan_at("cylinder @0,0 r=3 h=10 block=stone_bricks hollow\n",
                                        Some((0, 65, 0)));
        { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &tower, 200, 32, false); }
        assert_ne!(w.get(3, 66, 0), 0, "wall is there before carving");
        let (door, _) = parse_plan_at("sphere @3,0 r=1 block=air\n", Some((0, 65, 0)));
        { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &door, 200, 32, false); }
        assert_eq!(w.get(3, 66, 0), 0, "the doorway must be cut through the wall");
    }

    #[test]
    fn a_solid_dome_still_sits_above_the_surface() {
        let mut w = Flat::new();
        let (p, _) = parse_plan_at("sphere @0,0 r=3 block=stone\n", Some((0, 65, 0)));
        { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &p, 200, 32, false); }
        assert_ne!(w.get(0, 68, 0), 0, "centred at surface + r + 1");
    }

    #[test]
    fn a_build_lands_at_the_anchor_on_a_mountain_and_in_a_cave() {
        // The two worlds where a fixed search ceiling silently misplaced
        // everything: terrain above it, and a cave under it.
        struct Layered { ground: i32, ceiling: Option<i32>, cells: BTreeMap<(i32,i32,i32), u16> }
        impl VoxelWorld for Layered {
            fn get(&self, x: i32, y: i32, z: i32) -> u16 {
                if let Some(&b) = self.cells.get(&(x,y,z)) { return b; }
                if y <= self.ground { return 1; }
                if let Some(c) = self.ceiling { if y >= c && y <= c + 3 { return 1; } }
                0
            }
            fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool {
                self.cells.insert((x,y,z), b); true
            }
            fn y_bounds(&self) -> (i32, i32) { (-64, 512) }
        }
        for (ground, ceiling, label) in [
            (64, None, "surface"),
            (250, None, "mountain above the old ceiling"),
            (20, Some(120), "cave under a roof"),
        ] {
            let mut w = Layered { ground, ceiling, cells: BTreeMap::new() };
            let anchor = (0, ground + 1, 0);
            let (plan, _) = parse_plan_at("cylinder @0,0 r=1 h=3 block=stone_bricks\n",
                                          Some(anchor));
            { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false); }
            assert_eq!(w.get(0, anchor.1, 0), 58,
                       "{label}: base must sit at the anchor, not at a fixed altitude");
            assert_eq!(w.get(0, anchor.1 + 2, 0), 58, "{label}: and rise from there");
        }
    }

    #[test]
    fn without_an_anchor_the_callers_ceiling_is_still_used() {
        let mut w = Flat::new();
        let (plan, _) = parse_plan("cylinder 0,0 r=1 h=2 block=stone_bricks\n");
        assert!(plan.anchor.is_none());
        { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false); }
        assert_eq!(w.get(0, 65, 0), 58, "flat world, fallback path unchanged");
    }

    #[test]
    fn the_exact_lines_two_local_models_produced_now_build() {
        // From the field: a 1B and an 8B both had every op rejected.
        // "terraform needs two x,z pairs", "sphere radius must be
        // positive", "cylinder radius and height must be positive".
        // Each is a recoverable near-miss, and refusing them made the
        // feature look broken rather than imprecise.
        let cases: &[&str] = &[
            "terraform",                       // no coordinates at all
            "terraform @0,0",                  // one corner
            "sphere @0,0 r=0",                 // zero radius
            "cylinder @0,0 r=0 h=0",           // both zero
            "cylinder",                        // bare verb
            "path @0,0",                       // one end
            "enclose @2,2 h=-1",               // negative height
            "tree",                            // bare
            "scatter",                         // bare
        ];
        for line in cases {
            let (plan, issues) = parse_plan_at(&format!("{line}\n"), Some((10, 65, 20)));
            let errs: Vec<_> = issues.iter()
                .filter(|i| i.severity == Severity::Error).collect();
            assert!(errs.is_empty(), "{line:?} still rejected: {errs:?}");
            assert_eq!(plan.ops.len(), 1, "{line:?} produced no op");
            // and it must actually place blocks
            // Terraform on already-flat ground legitimately changes
            // nothing; what matters is that it PARSED and ran without
            // error, not that it moved dirt.
            let mut w = Flat::new();
            let run = { let mut t = VoxelTools::new(&mut w);
                        run_plan(&mut t, &plan, 200, 32, false) };
            assert_eq!(run.failed, 0, "{line:?} parsed but the op failed");
            if !line.starts_with("terraform") {
                assert!(run.blocks_changed > 0, "{line:?} parsed but built nothing");
            }
        }
    }

    #[test]
    fn an_inference_is_a_warning_the_player_can_see() {
        let (_, issues) = parse_plan_at("cylinder r=0\n", Some((10, 65, 20)));
        let warns: Vec<&str> = issues.iter()
            .filter(|i| i.severity == Severity::Warning)
            .map(|i| i.message.as_str()).collect();
        assert!(warns.iter().any(|m| m.contains("at the focus")), "{warns:?}");
        assert!(warns.iter().any(|m| m.contains("radius was 0")), "{warns:?}");
    }

    #[test]
    fn a_bare_verb_lands_at_the_focus_not_the_origin() {
        let (plan, _) = parse_plan_at("cylinder\n", Some((100, 65, -40)));
        match &plan.ops[0] {
            Op::Cylinder { at, .. } => assert_eq!(*at, (100, -40)),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn genuinely_meaningless_input_is_still_an_error() {
        // permissive is not the same as credulous
        let (plan, issues) = parse_plan_at("frobnicate the widget\n", Some((0, 65, 0)));
        assert!(plan.ops.is_empty());
        assert!(issues.iter().any(|i| i.severity == Severity::Error));
    }

    #[test]
    fn a_house_now_gets_a_roof() {
        // The gap the corpus named: "roof" said 29 times, no op for it.
        // Walls plus a roof must actually close the top.
        let mut w = Flat::new();
        let src = "enclose @-4,-4 @4,4 h=5 block=oak_planks\n\
                   roof @-4,-4 @4,4 gable block=dark_oak_planks\n";
        let (plan, issues) = parse_plan_at(src, Some((0, 65, 0)));
        assert!(issues.iter().all(|i| i.severity != Severity::Error), "{issues:?}");
        let run = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false) };
        assert!(run.blocks_changed > 50);
        // Walls occupy y=65..69, so the roof starts at 70. A GABLE has
        // two sloping faces and a ridge: along z=0 (the ridge line) only
        // the ridge itself is filled, which is what a gable looks like —
        // my first assertion checked a point that is correctly empty.
        assert_ne!(w.get(0, 75, 0), 0, "ridge above the walls");
        assert_ne!(w.get(0, 70, -5), 0, "eaves start at wall top, overhanging the wall line");
        assert_eq!(w.get(0, 72, 0), 0, "the space under the ridge is open, not solid");
    }

    #[test]
    fn mirror_reflects_and_does_not_smear_itself() {
        let mut w = Flat::new();
        // a lopsided pillar at x=3
        let (p1, _) = parse_plan_at("cylinder @3,0 r=1 h=6 block=stone_bricks\n",
                                    Some((0, 65, 0)));
        { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &p1, 200, 32, false); }
        assert_ne!(w.get(3, 67, 0), 0);
        assert_eq!(w.get(-3, 67, 0), 0, "nothing on the far side yet");
        // mirror across x=0 -> "either side of"
        let (p2, issues) = parse_plan_at("mirror @1,0,-3 @6,8,3 x about=0\n",
                                          Some((0, 65, 0)));
        assert!(issues.iter().all(|i| i.severity != Severity::Error), "{issues:?}");
        { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &p2, 200, 32, false); }
        assert_ne!(w.get(-3, 67, 0), 0, "reflected pillar exists");
        assert_ne!(w.get(3, 67, 0), 0, "and the original survives");
    }

    #[test]
    fn a_bridge_holds_its_height_where_a_path_would_dive() {
        // Flat world with a trench: a path follows the ground into it,
        // a bridge spans it. This is why they are different ops.
        struct Trench(BTreeMap<(i32,i32,i32), u16>);
        impl VoxelWorld for Trench {
            fn get(&self, x: i32, y: i32, z: i32) -> u16 {
                if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
                let floor = if (4..=10).contains(&x) { 50 } else { 64 };
                if y <= floor { 1 } else { 0 }
            }
            fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool {
                self.0.insert((x,y,z), b); true
            }
        }
        let mut w = Trench(BTreeMap::new());
        let (plan, issues) = parse_plan_at(
            "bridge @0,0 @14,0 w=3 block=oak_planks\n", Some((0, 65, 0)));
        assert!(issues.iter().all(|i| i.severity != Severity::Error), "{issues:?}");
        { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false); }
        // deck stays at the starting height right across the trench
        assert_ne!(w.get(7, 65, 0), 0, "deck spans the gap at height");
        assert_ne!(w.get(6, 60, 0), 0, "and a pier reaches down into it");
    }

    #[test]
    fn stairs_rise_one_per_step() {
        let mut w = Flat::new();
        let (plan, _) = parse_plan_at("stairs @0,0 @6,0 w=2 block=cobblestone\n",
                                      Some((0, 65, 0)));
        { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false); }
        assert_ne!(w.get(0, 65, 0), 0, "first tread at the base");
        assert_ne!(w.get(6, 71, 0), 0, "sixth tread six blocks up");
    }

    #[test]
    fn a_floor_makes_a_second_storey() {
        let mut w = Flat::new();
        let (plan, _) = parse_plan_at("floor @-3,-3 @3,3 y=70 block=oak_planks\n",
                                      Some((0, 65, 0)));
        { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false); }
        assert_ne!(w.get(0, 70, 0), 0);
        assert_eq!(w.get(0, 69, 0), 0, "nothing below it");
    }

    #[test]
    fn a_multi_word_creature_kind_keeps_its_adjective() {
        // "horrifying creatures": the adjective is the disposition, and
        // the first version dropped it by taking one word as the kind.
        let (plan, issues) = parse_plan_at(
            "populate horrifying creatures @-8,-8 @8,8 count=6 name=Alex\n", Some((0, 65, 0)));
        assert!(issues.is_empty(), "{issues:?}");
        match &plan.ops[0] {
            Op::Populate { kind, count, name, .. } => {
                assert_eq!(kind, "horrifying creatures");
                assert_eq!(*count, 6);
                assert_eq!(name.as_deref(), Some("Alex"));
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn the_exact_reply_that_was_rejected_now_salvages() {
        // From the field: a 1B whose fifth line was a malformed restyle.
        // Four good lines were thrown away with it because the error
        // carried no line number and `salvage` could not see it.
        let ir = "# tower\n\
                  terraform @-5,-5 @5,5 smooth block=stone\n\
                  cylinder @0,0 r=3 h=11 block=stone hollow\n\
                  roof @-3,-3 @3,3 hip block=dark_oak_planks\n\
                  restyle stone ruined\n\
                  tree @8,4 oak h=7\n";
        let (_, issues) = parse_plan_at(ir, Some((0, 65, 0)));
        let errs: Vec<&Issue> = issues.iter()
            .filter(|i| i.severity == Severity::Error).collect();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.starts_with("line 5:"),
                "the error must name its line or salvage cannot use it: {}", errs[0].message);

        let s = crate::recovery::salvage(ir, &issues);
        assert_eq!(s.bad.len(), 1, "one line dropped, not the plan");
        assert!(s.bad[0].1.contains("restyle"));
        assert!(!s.is_total_loss());
        let (plan, clean) = parse_plan_at(&s.good, Some((0, 65, 0)));
        assert!(clean.iter().all(|i| i.severity != Severity::Error));
        assert_eq!(plan.ops.len(), 4, "the four good ops survive");
    }

    #[test]
    fn every_parse_error_names_its_line() {
        // The property, not the instance: salvage depends on it for all
        // of them, so one bare message reintroduces the bug.
        let ir = "cylinder\nfrobnicate\nrestyle stone ruined\nmirror @0,0,0\n                  form nosuchform @0,0 @4,4\nmood nosuchmood @0,0 @4,4\n";
        let (_, issues) = parse_plan_at(ir, Some((0, 65, 0)));
        let errs: Vec<&Issue> = issues.iter()
            .filter(|i| i.severity == Severity::Error).collect();
        assert!(errs.len() >= 4, "several distinct failures: {errs:?}");
        for e in errs {
            assert!(e.message.starts_with("line "), "bare error message: {}", e.message);
        }
    }

    #[test]
    fn a_plant_is_placed_at_its_phenotype_and_handed_to_the_engine() {
        let mut w = Flat::new();
        let ir = "plant @0,0 oak tall ancient seed=3\nplant @20,0 oak small sparse seed=3\n";
        let (plan, issues) = parse_plan_at(ir, Some((0, 65, 0)));
        assert!(issues.iter().all(|i| i.severity != Severity::Error), "{issues:?}");
        let run = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false) };
        assert_eq!(run.plants.len(), 2, "both registered with the engine");
        assert!(run.plants[0].genome[crate::flora::MAX_HEIGHT] > 0.75, "tall: {:?}", run.plants[0].genome);
        assert!(run.plants[1].genome[crate::flora::MAX_HEIGHT] < 0.35, "small: {:?}", run.plants[1].genome);
        assert!(run.plants.iter().all(|p| p.growth >= 0.99), "placed mature");
        // and the placed forms differ the way the genomes do
        let top = |x: i32| (60..120).rev().find(|&y| w.get(x, y, 0) != 0).unwrap();
        assert!(top(0) - top(20) >= 6, "the tall one is taller: {} vs {}", top(0), top(20));
    }

    #[test]
    fn a_seedling_is_one_block_and_a_growth_of_near_zero() {
        let mut w = Flat::new();
        let (plan, _) = parse_plan_at("plant @0,0 birch fruit seedling\n", Some((0, 65, 0)));
        let run = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false) };
        assert_eq!(run.blocks_changed, 1);
        assert!(run.plants[0].growth < 0.1);
        assert!(run.plants[0].genome[crate::flora::FRUITFULNESS] > 0.8, "fruit: {:?}", run.plants[0].genome);
    }

    #[test]
    fn a_def_used_six_times_is_six_identical_houses() {
        let ir = "def hut\n\
                  enclose @-3,-3 @3,3 h=4 block=oak_planks\n\
                  roof @-3,-3 @3,3 gable\n\
                  sphere @3,0 r=1 block=air\n\
                  end\n\
                  use hut @0,0\n\
                  use hut @20,0 rot=90\n\
                  use hut @40,0 rot=180\n\
                  use hut @0,20 rot=270\n\
                  use hut @20,20 mirror\n\
                  use hut @40,20\n";
        let (plan, issues) = parse_plan_at(ir, Some((0, 65, 0)));
        assert!(issues.iter().all(|i| i.severity != Severity::Error), "{issues:?}");
        assert_eq!(plan.ops.len(), 18, "3 ops x 6 uses");
        // the door rotates with the hut: at rot=90 the @3,0 door is at @0,3
        let doors: Vec<(i32, i32)> = plan.ops.iter().filter_map(|o| match o {
            Op::Sphere { at, .. } => Some(*at), _ => None }).collect();
        assert!(doors.contains(&(3, 0)), "unrotated door: {doors:?}");
        assert!(doors.contains(&(20, 3)), "rot=90 door faces +z: {doors:?}");
        assert!(doors.contains(&(37, 0)), "rot=180 door faces -x: {doors:?}");
        assert!(doors.contains(&(17, 20)), "mirrored door faces -x: {doors:?}");
    }

    #[test]
    fn an_unknown_use_is_a_normal_unknown_op_with_its_line() {
        let (_, issues) = parse_plan_at("use nothing @0,0\n", Some((0, 65, 0)));
        let e = issues.iter().find(|i| i.severity == Severity::Error).expect("an error");
        assert!(e.message.starts_with("line 1:"), "{}", e.message);
    }

    #[test]
    fn line_numbers_survive_a_def_block() {
        // Salvage keys on line numbers; a def must not shift them.
        let ir = "def a\ncylinder @0,0 r=2\nend\nuse a @0,0\nfrobnicate\n";
        let (_, issues) = parse_plan_at(ir, Some((0, 65, 0)));
        let e = issues.iter().find(|i| i.severity == Severity::Error).unwrap();
        assert!(e.message.starts_with("line 5:"), "the bad line is line 5: {}", e.message);
    }
}
