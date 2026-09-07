//! Assets first, then passes that add or cut.
//!
//! WHAT WAS MISSING
//!
//! A 1B asked to "carve a great schism" built a statue, and the fix I
//! reached for was teaching it the word `canyon`. That was treating a
//! symptom. The real gap: nothing in this crate can say a build is a
//! SEQUENCE OF PASSES over one region, some adding material and some
//! taking it away. Ops run in order and each writes blocks. `Diff`
//! exists but only inside one form's CSG; at the plan level the only
//! way to remove anything is `block=air`, which is an accident of the
//! block system rather than a concept.
//!
//! So there was no way to express the most ordinary thing a builder
//! does: block out the mass, cut the openings, add the trim, cut the
//! weathering. Every build was one shot.
//!
//! HOW A BUILDER ACTUALLY WORKS
//!
//! Watch a build timelapse and two things dominate, neither of which
//! this pipeline had:
//!
//!   ASSETS FIRST. A window is designed once, then appears forty
//!   times. A pillar, an arch, a merlon, a roof section. That is how a
//!   thousand-block build gets coherent detail -- not by detailing a
//!   thousand blocks, but by detailing one window well and repeating
//!   it. The repetition is what makes it read as architecture rather
//!   than as noise.
//!
//!   MANY PASSES, ALTERNATING POLARITY. Nobody builds a castle wall
//!   and stops. They build the mass, cut the crenellations, add the
//!   walkway, cut the arrow slits, weather the corners. Each pass is
//!   over the SAME REGION and each is either additive or subtractive.
//!
//! THE MODEL
//!
//! A `Program` is assets and passes. An `Asset` is a named cell set
//! built once in scratch space. A `Pass` has a POLARITY -- `Add` or
//! `Cut` -- a source of geometry, and a scope. Running a program
//! accumulates a cell set: `Add` unions, `Cut` subtracts, and the
//! result is one change list the existing pipeline consumes unchanged.
//!
//! Polarity is the thing that was missing, and having it means a model
//! can be ASKED which it wants. "Carve" and "build" stop being two
//! vocabularies that a small model has to pick between correctly on
//! its first and only turn.

use crate::sketch::{self, Sketch};
use crate::weather::Change;
use std::collections::{BTreeMap, BTreeSet};

/// Does this pass put material in or take it out?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polarity { Add, Cut }

impl Polarity {
    pub fn parse(s: &str) -> Option<Polarity> {
        match s.to_lowercase().as_str() {
            "add" | "build" | "place" | "raise" | "grow" => Some(Polarity::Add),
            "cut" | "carve" | "remove" | "subtract" | "dig" | "hollow" | "erase" => Some(Polarity::Cut),
            _ => None,
        }
    }
    pub fn word(self) -> &'static str { match self { Polarity::Add => "add", Polarity::Cut => "cut" } }
}

/// A piece designed once and used many times.
#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub name: String,
    /// Cells relative to the asset's own origin, with their blocks.
    pub cells: Vec<((i32, i32, i32), u16)>,
    /// Its extent, for placing it.
    pub size: (i32, i32, i32),
}

impl Asset {
    /// Build an asset from a sketch, in its own space.
    pub fn from_sketch(name: &str, s: &Sketch) -> (Asset, Vec<String>) {
        let (placed, notes) = sketch::resolve(s, (0, 0, 0));
        let mut cells: BTreeMap<(i32, i32, i32), u16> = BTreeMap::new();
        for p in &placed {
            let block = crate::plan::resolve_block(&p.material).unwrap_or(1);
            // shapes with an op are rasterised here too: an asset is
            // cells, not IR, because it will be stamped rather than run
            for c in raster(p) { cells.insert(c, block); }
        }
        let lo = cells.keys().fold((i32::MAX, i32::MAX, i32::MAX),
            |a, c| (a.0.min(c.0), a.1.min(c.1), a.2.min(c.2)));
        let hi = cells.keys().fold((i32::MIN, i32::MIN, i32::MIN),
            |a, c| (a.0.max(c.0), a.1.max(c.1), a.2.max(c.2)));
        let size = if cells.is_empty() { (0, 0, 0) } else { (hi.0 - lo.0 + 1, hi.1 - lo.1 + 1, hi.2 - lo.2 + 1) };
        // normalised to its own corner, so a stamp is a simple offset
        let cells = cells.into_iter()
            .map(|(c, b)| (((c.0 - lo.0), (c.1 - lo.1), (c.2 - lo.2)), b))
            .collect();
        (Asset { name: name.to_string(), cells, size }, notes)
    }

    /// The cells this asset occupies when stamped at `at`, turned
    /// `turns` quarter-turns about Y.
    pub fn stamp(&self, at: (i32, i32, i32), turns: u8) -> Vec<((i32, i32, i32), u16)> {
        let (w, _, d) = self.size;
        self.cells.iter().map(|((x, y, z), b)| {
            let (rx, rz) = match turns % 4 {
                0 => (*x, *z),
                1 => (d - 1 - *z, *x),
                2 => (w - 1 - *x, d - 1 - *z),
                _ => (*z, w - 1 - *x),
            };
            ((at.0 + rx, at.1 + y, at.2 + rz), *b)
        }).collect()
    }
}

/// Every cell a placed part occupies, whatever its shape.
fn raster(p: &sketch::Placed) -> Vec<(i32, i32, i32)> {
    // the shapes with no IR op know how to draw themselves
    let from_cells = sketch::cells(p, 1);
    if !from_cells.is_empty() {
        return from_cells.into_iter().map(|(x, y, z, _)| (x, y, z)).collect();
    }
    // the rest are simple enough to draw here rather than round-trip
    // through the planner: an asset is built in scratch space where
    // there is no world to run a plan against
    let mut out = Vec::new();
    let at = p.at;
    match &p.shape {
        sketch::Shape::Sphere { r } => {
            let ri = r.ceil() as i32;
            for dx in -ri..=ri { for dy in -ri..=ri { for dz in -ri..=ri {
                let d = ((dx * dx + dy * dy + dz * dz) as f32).sqrt();
                if d <= *r && (!p.hollow || d >= r - 1.5) {
                    out.push((at.0 + dx, at.1 + dy, at.2 + dz));
                }
            }}}
        }
        sketch::Shape::Box { w, h, d } => {
            let (hw, hh, hd) = ((w / 2.0) as i32, (h / 2.0) as i32, (d / 2.0) as i32);
            for x in -hw..=hw { for y in -hh..=hh { for z in -hd..=hd {
                let edge = x.abs() == hw || y.abs() == hh || z.abs() == hd;
                if !p.hollow || edge { out.push((at.0 + x, at.1 + y, at.2 + z)); }
            }}}
        }
        sketch::Shape::Cylinder { r, h } => {
            let (ri, hi) = (r.ceil() as i32, h.max(1.0) as i32);
            for dx in -ri..=ri { for dz in -ri..=ri {
                let d = ((dx * dx + dz * dz) as f32).sqrt();
                if d > *r { continue; }
                if p.hollow && d < r - 1.0 { continue; }
                for k in 0..hi { out.push((at.0 + dx, at.1 - hi / 2 + k, at.2 + dz)); }
            }}
        }
        sketch::Shape::Slab { w, d, thickness } => {
            let (hw, hd, t) = ((w / 2.0) as i32, (d / 2.0) as i32, thickness.max(1.0) as i32);
            for x in -hw..=hw { for z in -hd..=hd { for k in 0..t {
                out.push((at.0 + x, at.1 + k, at.2 + z));
            }}}
        }
        _ => {}
    }
    out
}

/// Where a pass's geometry comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// Parts and relations, resolved in place.
    Sketch(String),
    /// An asset, stamped at each of these places.
    Stamp { asset: String, at: Vec<((i32, i32, i32), u8)> },
    /// An asset repeated on a rule: along a line, around a ring, on a
    /// grid. The repetition a builder actually uses.
    Repeat { asset: String, rule: Repeat },
}

/// How an asset repeats. Positions are relative to the pass anchor.
#[derive(Debug, Clone, PartialEq)]
pub enum Repeat {
    /// Every `step` blocks from `from` to `to`.
    Line { from: (i32, i32, i32), to: (i32, i32, i32), step: i32 },
    /// `count` copies on a circle, each turned to face outward.
    Ring { centre: (i32, i32, i32), radius: i32, count: usize, face_out: bool },
    /// A lattice.
    Grid { min: (i32, i32, i32), max: (i32, i32, i32), step: i32 },
    /// Up a column.
    Stack { at: (i32, i32, i32), count: usize, step: i32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pass {
    pub name: String,
    pub polarity: Polarity,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Program {
    pub assets: Vec<Asset>,
    pub passes: Vec<Pass>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Run {
    /// The final cell set: what to write.
    pub changes: Vec<Change>,
    /// Per pass: name, polarity, and how many cells it moved.
    pub log: Vec<(String, Polarity, usize)>,
    pub notes: Vec<String>,
    pub errors: Vec<String>,
}

impl Run {
    pub fn summary(&self) -> String {
        let parts: Vec<String> = self.log.iter()
            .map(|(n, p, c)| format!("{n} ({} {c})", p.word())).collect();
        format!("{} pass(es), {} cells: {}", self.log.len(), self.changes.len(), parts.join(" -> "))
    }
}

/// Run a program: accumulate a cell set, pass by pass.
///
/// `Add` unions; `Cut` subtracts. A cut of something that was never
/// added still writes air -- cutting into the WORLD is how a canyon or
/// a doorway works, and the run reports how many cells a cut removed
/// from its own accumulation versus from the world, because the
/// difference is the difference between shaping a build and digging.
pub fn run(prog: &Program, anchor: (i32, i32, i32)) -> Run {
    let mut r = Run { notes: prog.notes.clone(), ..Default::default() };
    let assets: BTreeMap<&str, &Asset> = prog.assets.iter().map(|a| (a.name.as_str(), a)).collect();
    let mut solid: BTreeMap<(i32, i32, i32), u16> = BTreeMap::new();
    let mut known: BTreeMap<String, (sketch::Shape, (i32, i32, i32))> = BTreeMap::new();
    let mut cut: BTreeSet<(i32, i32, i32)> = BTreeSet::new();
    for pass in &prog.passes {
        let cells: Vec<((i32, i32, i32), u16)> = match &pass.source {
            Source::Sketch(src) => {
                let (mut sk, _) = sketch::parse(src);
                sk.assume_known = known.keys().cloned().collect();
                let (_, errs) = sketch::parse(src);
                let known_names: BTreeSet<String> = known.keys().cloned().collect();
                r.errors.extend(errs.into_iter()
                    .filter(|e| !known_names.iter().any(|k| e.contains(&format!("'{k}'"))))
                    .map(|e| format!("{}: {e}", pass.name)));
                // PARTS FROM EARLIER PASSES ARE VISIBLE. A builder says
                // "the ribs hang off the spine" where the spine was the
                // last pass's work; without this each pass resolved in
                // isolation and every cross-pass relation silently
                // anchored at the origin. Earlier parts are carried as
                // ghosts: they position things and place nothing.
                let (placed, notes) = sketch::resolve_with(&sk, anchor, &known);
                r.notes.extend(notes.into_iter().map(|n| format!("{}: {n}", pass.name)));
                for pl in &placed { known.insert(pl.name.clone(), (pl.shape.clone(), pl.at)); }
                placed.iter().flat_map(|p| {
                    let b = crate::plan::resolve_block(&p.material).unwrap_or(1);
                    raster(p).into_iter().map(move |c| (c, b))
                }).collect()
            }
            Source::Stamp { asset, at } => {
                let Some(a) = assets.get(asset.as_str()) else {
                    r.errors.push(format!("{}: no asset '{asset}'", pass.name)); continue;
                };
                at.iter().flat_map(|(p, turns)| a.stamp(
                    (anchor.0 + p.0, anchor.1 + p.1, anchor.2 + p.2), *turns)).collect()
            }
            Source::Repeat { asset, rule } => {
                let Some(a) = assets.get(asset.as_str()) else {
                    r.errors.push(format!("{}: no asset '{asset}'", pass.name)); continue;
                };
                places(rule, a).into_iter()
                    .flat_map(|(p, t)| a.stamp((anchor.0 + p.0, anchor.1 + p.1, anchor.2 + p.2), t))
                    .collect()
            }
        };
        let mut moved = 0usize;
        match pass.polarity {
            Polarity::Add => {
                for (c, b) in cells {
                    // an Add un-cuts: a later pass may restore what an
                    // earlier one removed, which is how a builder fixes
                    // an over-cut without starting again
                    cut.remove(&c);
                    if solid.insert(c, b).is_none() { moved += 1; }
                }
            }
            Polarity::Cut => {
                for (c, _) in cells {
                    if solid.remove(&c).is_some() { moved += 1; }
                    // recorded either way: a cut into untouched world is
                    // still a cut, and that is what digging is
                    cut.insert(c);
                    moved += usize::from(!solid.contains_key(&c));
                }
            }
        }
        r.log.push((pass.name.clone(), pass.polarity, moved));
    }
    // the final write: everything solid, plus air where a cut reached
    // ground the build never occupied
    r.changes = solid.iter().map(|(at, to)| Change { at: *at, to: *to })
        .chain(cut.iter().filter(|c| !solid.contains_key(c)).map(|at| Change { at: *at, to: 0 }))
        .collect();
    r
}

fn places(rule: &Repeat, _a: &Asset) -> Vec<((i32, i32, i32), u8)> {
    match rule {
        Repeat::Line { from, to, step } => {
            let step = (*step).max(1);
            let (dx, dy, dz) = (to.0 - from.0, to.1 - from.1, to.2 - from.2);
            let len = ((dx * dx + dz * dz) as f32).sqrt().max(1.0);
            let n = ((len / step as f32).floor() as usize).max(1);
            (0..=n).map(|i| {
                let f = i as f32 / n as f32;
                (((from.0 as f32 + dx as f32 * f) as i32,
                  (from.1 as f32 + dy as f32 * f) as i32,
                  (from.2 as f32 + dz as f32 * f) as i32), 0u8)
            }).collect()
        }
        Repeat::Ring { centre, radius, count, face_out } => {
            (0..*count).map(|i| {
                let th = i as f32 / *count as f32 * std::f32::consts::TAU;
                let p = (centre.0 + (*radius as f32 * th.cos()) as i32, centre.1,
                         centre.2 + (*radius as f32 * th.sin()) as i32);
                // each copy turned to face away from the centre, so a
                // ring of windows looks out rather than all facing north
                let t = if *face_out { (((th / std::f32::consts::FRAC_PI_2).round() as i32).rem_euclid(4)) as u8 }
                        else { 0 };
                (p, t)
            }).collect()
        }
        Repeat::Grid { min, max, step } => {
            let step = (*step).max(1);
            let mut v = Vec::new();
            for x in (min.0..=max.0).step_by(step as usize) {
                for z in (min.2..=max.2).step_by(step as usize) { v.push(((x, min.1, z), 0u8)); }
            }
            v
        }
        Repeat::Stack { at, count, step } =>
            (0..*count).map(|i| ((at.0, at.1 + i as i32 * (*step).max(1), at.2), 0u8)).collect(),
    }
}

// ── the program language ─────────────────────────────────────────────
//
// What a model writes for the new pass 2. Blocks, because assets and
// passes both have bodies:
//
//   asset window
//     part frame box w=3 h=4 d=1 material=stone
//   end
//   pass mass add
//     part keep box w=20 h=16 d=20 material=stone hollow
//     rel keep on ground
//   end
//   pass windows cut
//     repeat window ring centre=0,8,0 radius=10 count=8 face_out
//   end

pub fn parse(src: &str) -> (Program, Vec<String>) {
    let mut p = Program::default();
    let mut errs = Vec::new();
    let mut lines = src.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') { continue; }
        let w: Vec<&str> = t.split_whitespace().collect();
        match w.first().map(|v| v.to_lowercase()).as_deref() {
            Some("asset") => {
                let Some(name) = w.get(1) else { errs.push("asset needs a name".into()); continue };
                let body = take_body(&mut lines);
                let (sk, e) = sketch::parse(&body);
                errs.extend(e.into_iter().map(|x| format!("asset {name}: {x}")));
                let (a, notes) = Asset::from_sketch(name, &sk);
                if a.cells.is_empty() { errs.push(format!("asset {name} is empty")); continue; }
                p.notes.extend(notes.into_iter().map(|n| format!("asset {name}: {n}")));
                p.assets.push(a);
            }
            Some("pass") => {
                // POLARITY IS THE LAST WORD, and the name is everything
                // between. `pass hollow the ribcage cut` is what a model
                // writes; taking w[2] as the polarity read "the" and
                // threw the pass away -- three of five passes lost in
                // the first real test.
                if w.len() < 3 { errs.push(format!("pass needs a name and add|cut: {t:?}")); continue; }
                let pol = w[w.len() - 1];
                let Some(polarity) = Polarity::parse(pol) else {
                    errs.push(format!("pass '{}': '{pol}' is not add or cut -- put add or cut LAST",
                                      w[1..w.len() - 1].join(" ")));
                    continue;
                };
                let name = w[1..w.len() - 1].join(" ");
                let name = &name.as_str();
                let body = take_body(&mut lines);
                // a body of `repeat`/`stamp` lines is asset work; anything
                // else is a sketch
                let first = body.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#'));
                let source = match first.map(|l| l.split_whitespace().next().unwrap_or("")) {
                    Some("repeat") | Some("stamp") => match parse_asset_use(&body) {
                        Ok(s) => s,
                        Err(e) => { errs.push(format!("pass '{name}': {e}")); continue; }
                    },
                    _ => Source::Sketch(body),
                };
                p.passes.push(Pass { name: name.to_string(), polarity, source });
            }
            _ => p.notes.push(t.to_string()),
        }
    }
    // a pass naming an asset that does not exist is worth saying loudly
    let names: Vec<&str> = p.assets.iter().map(|a| a.name.as_str()).collect();
    for pass in &p.passes {
        let want = match &pass.source {
            Source::Stamp { asset, .. } | Source::Repeat { asset, .. } => Some(asset.as_str()),
            _ => None,
        };
        if let Some(a) = want {
            if !names.contains(&a) { errs.push(format!("pass '{}' uses asset '{a}', which is not defined", pass.name)); }
        }
    }
    (p, errs)
}

fn take_body<'a>(lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>) -> String {
    let mut body = String::new();
    for line in lines.by_ref() {
        if line.trim().eq_ignore_ascii_case("end") { break; }
        body.push_str(line.trim());
        body.push('\n');
    }
    body
}

fn triple(s: &str) -> Option<(i32, i32, i32)> {
    let p: Vec<i32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    match p.len() { 3 => Some((p[0], p[1], p[2])), 2 => Some((p[0], 0, p[1])), _ => None }
}

fn parse_asset_use(body: &str) -> Result<Source, String> {
    let line = body.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#'))
        .ok_or("empty pass body")?;
    let t: Vec<&str> = line.split_whitespace().collect();
    let asset = t.get(1).ok_or("repeat needs an asset name")?.to_string();
    let kv = |k: &str| t.iter().find_map(|s| s.strip_prefix(&format!("{k}=")));
    let num = |k: &str, d: i32| kv(k).and_then(|v| v.parse().ok()).unwrap_or(d);
    if t.first().map(|v| v.to_lowercase()) == Some("stamp".into()) {
        let at: Vec<((i32, i32, i32), u8)> = body.lines().map(str::trim)
            .filter(|l| l.to_lowercase().starts_with("stamp "))
            .filter_map(|l| {
                let p: Vec<&str> = l.split_whitespace().collect();
                let at = triple(p.get(2)?)?;
                let turns = p.iter().find_map(|s| s.strip_prefix("rot="))
                    .and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
                Some((at, ((turns.rem_euclid(360)) / 90) as u8))
            }).collect();
        if at.is_empty() { return Err("stamp needs at least one position".into()); }
        return Ok(Source::Stamp { asset, at });
    }
    let rule = match t.get(2).map(|v| v.to_lowercase()).as_deref() {
        Some("line") => Repeat::Line {
            from: kv("from").and_then(triple).ok_or("line needs from=x,y,z")?,
            to: kv("to").and_then(triple).ok_or("line needs to=x,y,z")?,
            step: num("step", 6) },
        Some("ring") => Repeat::Ring {
            centre: kv("centre").or_else(|| kv("center")).and_then(triple).unwrap_or((0, 0, 0)),
            radius: num("radius", 10), count: num("count", 8).clamp(1, 256) as usize,
            face_out: t.contains(&"face_out") },
        Some("grid") => Repeat::Grid {
            min: kv("min").and_then(triple).ok_or("grid needs min=x,y,z")?,
            max: kv("max").and_then(triple).ok_or("grid needs max=x,y,z")?,
            step: num("step", 8) },
        Some("stack") => Repeat::Stack {
            at: kv("at").and_then(triple).unwrap_or((0, 0, 0)),
            count: num("count", 4).clamp(1, 128) as usize, step: num("step", 5) },
        other => return Err(format!("unknown repeat rule '{}'; known: line, ring, grid, stack",
                                    other.unwrap_or("(none)"))),
    };
    Ok(Source::Repeat { asset, rule })
}

/// The prompt for the pass that decides POLARITY and ASSETS -- the
/// thing that had no pass at all, which is why "carve a schism" came
/// back as a statue.
pub fn plan_prompt(request: &str, reading: &crate::compose::Reading) -> String {
    format!("\
The player asked for: {request}

You already decided what it is:

{}
Now plan it as a builder does: ASSETS first, then PASSES.

An ASSET is a piece you design once and repeat -- a window, a pillar, a merlon, an arch.
Repetition is what makes a big build read as architecture instead of noise. Design the
piece well once; place it forty times.

A PASS is one sweep over the area that either ADDS material or CUTS it away. Real building
alternates: block out the mass, cut the openings, add the trim, cut the weathering. Say
which each pass is.

  asset NAME
    <parts and relations>
  end

  pass NAME add|cut
    <parts and relations>       (geometry made here)
  end

  pass NAME add|cut
    repeat ASSET ring centre=0,8,0 radius=10 count=8 face_out
    repeat ASSET line from=-20,0,0 to=20,0,0 step=6
    repeat ASSET grid min=-20,0,-20 max=20,0,20 step=10
    repeat ASSET stack at=0,0,0 count=5 step=6
    stamp ASSET 4,0,-8 rot=90
  end

THE FIRST QUESTION FOR EVERY PASS: is this pass putting material in, or taking it out?
A canyon is one cut pass. A castle is a mass pass, then cut passes for the gate and the
crenellations, then add passes for the walkway and the towers. Something asked to be
'carved', 'dug' or 'hollowed' is mostly cut passes; do not build a shape that looks like
the thing you were asked to remove.

EXAMPLE
  player: carve a great schism into the ground
pass rift cut
  part gash box w=140 h=30 d=18 material=stone
  rel gash below ground by=0
end

EXAMPLE
  player: a castle keep with arrow slits
asset slit
  part hole box w=1 h=3 d=4 material=stone
end
pass mass add
  part keep box w=24 h=20 d=24 material=stone hollow
  rel keep on ground
end
pass slits cut
  repeat slit ring centre=0,10,0 radius=12 count=12 face_out
end

Write the assets and passes only.
", reading.brief())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_pass_removes_what_an_add_pass_put_there() {
        let (p, errs) = parse(
            "pass mass add\n\
               part block box w=10 h=10 d=10 material=stone\n\
               rel block on ground\n\
             end\n\
             pass hole cut\n\
               part bore cylinder r=2 h=12 material=stone\n\
               rel bore on ground\n\
             end\n");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(p.passes.len(), 2);
        assert_eq!(p.passes[0].polarity, Polarity::Add);
        assert_eq!(p.passes[1].polarity, Polarity::Cut);
        let r = run(&p, (0, 64, 0));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        // the block is there, minus a bore through it
        let solid: BTreeSet<(i32, i32, i32)> = r.changes.iter().filter(|c| c.to != 0).map(|c| c.at).collect();
        let air: BTreeSet<(i32, i32, i32)> = r.changes.iter().filter(|c| c.to == 0).map(|c| c.at).collect();
        assert!(solid.len() > 400, "a 10-cube of stone: {}", solid.len());
        assert!(!solid.contains(&(0, 68, 0)), "the bore went through the middle");
        assert!(solid.contains(&(4, 68, 4)), "and the corner is untouched");
        // the cut is logged as a cut
        assert_eq!(r.log[1].1, Polarity::Cut);
        assert!(r.log[1].2 > 20, "it removed something: {}", r.log[1].2);
        assert!(r.summary().contains("mass (add") && r.summary().contains("hole (cut"), "{}", r.summary());
        let _ = air;
    }

    #[test]
    fn a_cut_into_untouched_ground_still_digs() {
        // the schism case: one cut pass, nothing added first
        let (p, errs) = parse(
            "pass rift cut\n\
               part gash box w=60 h=20 d=10 material=stone\n\
               rel gash below ground by=0\n\
             end\n");
        assert!(errs.is_empty(), "{errs:?}");
        let r = run(&p, (0, 80, 0));
        let air: Vec<&Change> = r.changes.iter().filter(|c| c.to == 0).collect();
        assert!(air.len() > 5000, "a schism of {} cells", air.len());
        assert!(r.changes.iter().all(|c| c.to == 0), "a pure cut writes only air");
        // and it is below the anchor, where the ground is
        assert!(air.iter().all(|c| c.at.1 <= 80), "it digs down");
    }

    #[test]
    fn an_asset_is_built_once_and_repeated() {
        let (p, errs) = parse(
            "asset merlon\n\
               part tooth box w=2 h=3 d=2 material=stone\n\
             end\n\
             pass wall add\n\
               part w box w=30 h=8 d=2 material=stone\n\
               rel w on ground\n\
             end\n\
             pass teeth add\n\
               repeat merlon line from=-14,4,0 to=14,4,0 step=4\n\
             end\n");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(p.assets.len(), 1);
        assert!(p.assets[0].cells.len() >= 18, "the merlon has body: {}", p.assets[0].cells.len());
        let r = run(&p, (0, 64, 0));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        // the merlons are spread along the wall top, not all at once
        let xs: BTreeSet<i32> = r.changes.iter().filter(|c| c.at.1 > 70).map(|c| c.at.0).collect();
        assert!(xs.len() > 12, "spread along the top: {} distinct x", xs.len());
        assert!(r.log[1].2 > 50, "the repeat placed real cells: {}", r.log[1].2);
    }

    #[test]
    fn a_ring_of_assets_faces_outward() {
        let (p, _) = parse(
            "asset slit\n\
               part hole box w=1 h=3 d=5 material=stone\n\
             end\n\
             pass slits cut\n\
               repeat slit ring centre=0,6,0 radius=10 count=8 face_out\n\
             end\n");
        let r = run(&p, (0, 64, 0));
        let at: BTreeSet<(i32, i32)> = r.changes.iter().map(|c| (c.at.0, c.at.2)).collect();
        // eight clusters on a circle
        assert!(at.len() > 20, "{} cells", at.len());
        let far = at.iter().map(|(x, z)| (((x * x + z * z) as f32).sqrt()) as i32).max().unwrap();
        assert!((8..=16).contains(&far), "on a circle of about radius 10: {far}");
    }

    #[test]
    fn an_add_after_a_cut_restores_what_was_removed() {
        // a builder over-cuts and fixes it rather than starting again
        let (p, _) = parse(
            "pass mass add\n\
               part b box w=8 h=8 d=8 material=stone\n\
               rel b on ground\n\
             end\n\
             pass overcut cut\n\
               part c box w=12 h=12 d=12 material=stone\n\
               rel c on ground\n\
             end\n\
             pass fix add\n\
               part d box w=6 h=6 d=6 material=stone\n\
               rel d on ground\n\
             end\n");
        let r = run(&p, (0, 64, 0));
        let solid: BTreeSet<(i32, i32, i32)> = r.changes.iter().filter(|c| c.to != 0).map(|c| c.at).collect();
        assert!(!solid.is_empty(), "the fix pass put material back");
        // and nothing is BOTH solid and air in the output
        let air: BTreeSet<(i32, i32, i32)> = r.changes.iter().filter(|c| c.to == 0).map(|c| c.at).collect();
        assert!(solid.is_disjoint(&air), "a cell is either solid or air, never both");
        assert_eq!(r.log.len(), 3);
    }

    #[test]
    fn polarity_words_map_the_way_a_player_would_say_them() {
        for w in ["add", "build", "place", "raise", "grow"] { assert_eq!(Polarity::parse(w), Some(Polarity::Add), "{w}"); }
        for w in ["cut", "carve", "remove", "dig", "hollow", "erase"] { assert_eq!(Polarity::parse(w), Some(Polarity::Cut), "{w}"); }
        assert_eq!(Polarity::parse("frobnicate"), None);
    }

    #[test]
    fn a_program_that_names_a_missing_asset_says_so() {
        let (_, errs) = parse("pass x add\n  repeat nosuch ring radius=5 count=4\nend\n");
        assert!(errs.iter().any(|e| e.contains("'nosuch', which is not defined")), "{errs:?}");
        let (_, e2) = parse("pass x sideways\n  part a box w=2 material=stone\nend\n");
        assert!(e2.iter().any(|e| e.contains("not add or cut")), "{e2:?}");
    }

    #[test]
    fn the_prompt_asks_polarity_first_and_its_examples_run() {
        let reading = crate::compose::parse_reading("IT IS: a rift in the ground\nAVOID: building a wall\n");
        let p = plan_prompt("carve a great schism into the ground", &reading);
        assert!(p.contains("is this pass putting material in, or taking it out?"), "{p}");
        assert!(p.contains("ASSETS first"), "assets are named first");
        // the sentence wraps, so compare on whitespace-normalised text
        let flat = p.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains("do not build a shape that looks like the thing you were asked to remove"),
                "the anti-statue instruction is present");
        // the examples parse and run
        for ex in p.split("EXAMPLE").skip(1) {
            let body: String = ex.lines().skip(1)
                .skip_while(|l| l.trim_start().starts_with("player:"))
                .take_while(|l| !l.trim().is_empty() && !l.starts_with("Write"))
                .map(|l| format!("{l}\n")).collect();
            let (prog, errs) = parse(&body);
            assert!(errs.is_empty(), "{body}\n{errs:?}");
            assert!(!prog.passes.is_empty(), "{body}");
            let r = run(&prog, (0, 64, 0));
            assert!(r.errors.is_empty(), "{body}\n{:?}", r.errors);
            assert!(!r.changes.is_empty(), "{body} produced nothing");
        }
        // the schism example is a CUT, which is the entire point
        let (schism, _) = parse("pass rift cut\n  part gash box w=140 h=30 d=18 material=stone\n  rel gash below ground by=0\nend\n");
        assert_eq!(schism.passes[0].polarity, Polarity::Cut);
    }
}
