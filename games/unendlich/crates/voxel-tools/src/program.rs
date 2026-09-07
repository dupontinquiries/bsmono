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

    /// Stamped and scaled: each cell's offset multiplied, then the
    /// gaps filled so a scaled-up asset is solid rather than a lattice
    /// of its original cells.
    pub fn stamp_scaled(&self, at: (i32, i32, i32), turns: u8, scale: f32)
        -> Vec<((i32, i32, i32), u16)> {
        let base = self.stamp((0, 0, 0), turns);
        let s = scale.max(0.05);
        let mut out: BTreeMap<(i32, i32, i32), u16> = BTreeMap::new();
        let step = if s > 1.0 { s.ceil() as i32 } else { 1 };
        for ((x, y, z), b) in base {
            let (bx, by, bz) = ((x as f32 * s) as i32, (y as f32 * s) as i32, (z as f32 * s) as i32);
            for dx in 0..step { for dy in 0..step { for dz in 0..step {
                out.insert((at.0 + bx + dx, at.1 + by + dy, at.2 + bz + dz), b);
            }}}
        }
        out.into_iter().collect()
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
    ///
    /// `vary` is an expression per copy, evaluated with `i`, `n` and
    /// `t` bound: `scale=1 + t*0.6` makes each copy larger than the
    /// last. That is the one thing generating code would genuinely buy,
    /// and it costs an expression evaluator rather than an interpreter.
    Repeat { asset: String, rule: Repeat, vary: Vec<(String, String)>, site: Site },
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
    /// One copy at every point of a frame -- the armature's own joints.
    OnFrame { frame: String },
    /// Along a PATH through a frame's points in order, every `step`
    /// blocks. A wall following a ridge, a road through a valley: the
    /// thing `link` could not do because it had only two ends.
    Along { frame: String, step: i32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pass {
    pub name: String,
    pub polarity: Polarity,
    pub source: Source,
}

/// A named set of points: an ARMATURE.
///
/// The reference dragon's skeleton is thirteen named joints, and
/// everything -- limbs, wings, the rig -- binds to them. This program
/// language had no such thing: a part could relate to another PART,
/// but there was no way to lay out a structure's bones once and hang
/// several passes on them. So a dragon skeleton came out as a single
/// arched spine with everything else guessed relative to it.
///
/// A frame is points with names. Parts attach by `rel X at JOINT`, and
/// repeats walk a frame with `repeat asset frame NAME`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Frame {
    pub name: String,
    pub points: Vec<(String, (i32, i32, i32))>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Program {
    pub frames: Vec<Frame>,
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
/// Run without a world: positions are taken literally and nothing can
/// consult the ground. Fine for an asset or a test, wrong for a build.
pub fn run(prog: &Program, anchor: (i32, i32, i32)) -> Run {
    run_in(&Void, prog, anchor)
}

/// A world where nothing exists, for `run`.
struct Void;
impl crate::VoxelWorld for Void {
    fn get(&self, _: i32, _: i32, _: i32) -> u16 { 0 }
    fn set(&mut self, _: i32, _: i32, _: i32, _: u16) -> bool { false }
}

/// Run against the WORLD, so repeats can sit on the ground and refuse
/// the places that do not suit them.
///
/// Without this a town was a floating regular lattice: every hut at
/// the same y whatever the terrain did, and evenly spaced whether or
/// not there was anywhere to stand. Both of those read as generated,
/// and both are fixed by letting a repeat ask the ground a question.
pub fn run_in<W: crate::VoxelWorld>(w: &W, prog: &Program, anchor: (i32, i32, i32)) -> Run {
    let mut r = Run { notes: prog.notes.clone(), ..Default::default() };
    let assets: BTreeMap<&str, &Asset> = prog.assets.iter().map(|a| (a.name.as_str(), a)).collect();
    let mut solid: BTreeMap<(i32, i32, i32), u16> = BTreeMap::new();
    let mut known: BTreeMap<String, (sketch::Shape, (i32, i32, i32))> = BTreeMap::new();
    let mut cut: BTreeSet<(i32, i32, i32)> = BTreeSet::new();
    for pass in &prog.passes {
        let cells: Vec<((i32, i32, i32), u16)> = match &pass.source {
            Source::Sketch(src) => {
                let (mut sk, _) = sketch::parse(src);
                let (_, errs) = sketch::parse(src);
                let known_names: BTreeSet<String> = known.keys().cloned()
                    .chain(prog.frames.iter().flat_map(|f| f.points.iter().map(|(n, _)| n.clone())))
                    .collect();
                r.errors.extend(errs.into_iter()
                    .filter(|e| !known_names.iter().any(|k| e.contains(&format!("'{k}'"))))
                    .map(|e| format!("{}: {e}", pass.name)));
                // PARTS FROM EARLIER PASSES ARE VISIBLE. A builder says
                // "the ribs hang off the spine" where the spine was the
                // last pass's work; without this each pass resolved in
                // isolation and every cross-pass relation silently
                // anchored at the origin. Earlier parts are carried as
                // ghosts: they position things and place nothing.
                // a frame's points are relation targets too: `rel skull
                // at head` puts a part on a named joint
                let mut with_frames = known.clone();
                for f in &prog.frames {
                    for (n, pt) in &f.points {
                        with_frames.entry(n.clone())
                            .or_insert((sketch::Shape::Sphere { r: 0.0 },
                                        (anchor.0 + pt.0, anchor.1 + pt.1, anchor.2 + pt.2)));
                    }
                }
                sk.assume_known = with_frames.keys().cloned().collect();
                let (placed, notes) = sketch::resolve_with(&sk, anchor, &with_frames);
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
            Source::Repeat { asset, rule, vary, site } => {
                let site = *site;
                let Some(a) = assets.get(asset.as_str()) else {
                    r.errors.push(format!("{}: no asset '{asset}'", pass.name)); continue;
                };
                let spots = places_in(w, rule, a, site, anchor, &prog.frames);
                let n = spots.len();
                let mut out = Vec::new();
                for (i, (p, t)) in spots.into_iter().enumerate() {
                    let vars = crate::expr::repeat_vars(i, n);
                    // per-copy variation: scale, and an extra lift
                    let get = |k: &str, d: f32| -> f32 {
                        match vary.iter().find(|(n, _)| n == k) {
                            Some((_, e)) => match crate::expr::eval(e, &vars) {
                                Ok(v) => v,
                                Err(_) => d,
                            },
                            None => d,
                        }
                    };
                    for (name, e) in vary {
                        if let Err(msg) = crate::expr::eval(e, &vars) {
                            r.errors.push(format!("{}: {name}={e:?}: {msg}", pass.name));
                        }
                    }
                    let scale = get("scale", 1.0).clamp(0.1, 8.0);
                    let lift = get("lift", 0.0).round() as i32;
                    let spin = ((get("rot", 0.0) as i32).rem_euclid(360) / 90) as u8;
                    let at = (anchor.0 + p.0, anchor.1 + p.1 + lift, anchor.2 + p.2);
                    if (scale - 1.0).abs() < 0.05 {
                        out.extend(a.stamp(at, (t + spin) % 4));
                    } else {
                        out.extend(a.stamp_scaled(at, (t + spin) % 4, scale));
                    }
                }
                out
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

/// What a repeat asks of the ground before it places anything.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Site {
    /// Drop each copy onto the surface rather than using the given y.
    pub on_ground: bool,
    /// Reject a spot whose surface varies more than this across the
    /// asset's footprint.
    pub max_slope: Option<i32>,
    /// Move each copy by up to this much, seeded: a town is not a grid.
    pub jitter: i32,
    pub seed: u64,
}

fn places_in<W: crate::VoxelWorld>(w: &W, rule: &Repeat, a: &Asset, site: Site,
                                   anchor: (i32, i32, i32), frames: &[Frame])
    -> Vec<((i32, i32, i32), u8)> {
    let raw = places_with(rule, a, frames);
    if !site.on_ground && site.max_slope.is_none() && site.jitter == 0 { return raw; }
    let surface = |x: i32, z: i32| (0..=anchor.1 + 160).rev()
        .find(|&y| w.get(x, y, z) != 0);
    let mut out = Vec::new();
    // The footprint an asset ACTUALLY occupies. `stamp` writes cells at
    // `at + offset` for offset in 0..size, so an asset spans [at,
    // at+size) -- it is not centred on its position. Sampling
    // symmetrically around `at` tested a box offset from the building
    // by half its width, and a hut was allowed to straddle a cliff
    // edge its own front door hung over.
    let (fw, fd) = (a.size.0.max(1), a.size.2.max(1));
    for (i, (p, t)) in raw.into_iter().enumerate() {
        // JITTER first, so the slope test judges where it will land
        let mut p = p;
        if site.jitter > 0 {
            let h = |k: u64| {
                let mut v = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ site.seed ^ k;
                v ^= v >> 31; v = v.wrapping_mul(0x7FB5_D329_728E_A185); (v >> 33) as i32
            };
            let j = site.jitter;
            p.0 += h(1) % (j * 2 + 1) - j;
            p.2 += h(2) % (j * 2 + 1) - j;
        }
        let (wx, wz) = (anchor.0 + p.0, anchor.2 + p.2);
        if let Some(max) = site.max_slope {
            let mut lo = i32::MAX; let mut hi = i32::MIN;
            let mut missing = false;
            for dx in [0, fw / 2, fw - 1] { for dz in [0, fd / 2, fd - 1] {
                match surface(wx + dx, wz + dz) { Some(y) => { lo = lo.min(y); hi = hi.max(y); }
                                                  None => missing = true }
            }}
            if missing || hi - lo > max { continue; }
        }
        if site.on_ground {
            // the LOWEST point under the footprint, so a building rests
            // on the ground rather than having one corner buried
            match (0..fw).step_by(fw.max(1) as usize / 2 + 1)
                .flat_map(|dx| (0..fd).step_by(fd.max(1) as usize / 2 + 1).map(move |dz| (dx, dz)))
                .filter_map(|(dx, dz)| surface(wx + dx, wz + dz)).min() {
                // the asset's own base sits on the surface, so `at` is
                // the surface plus one, in the anchor's frame
                Some(y) => p.1 = y + 1 - anchor.1,
                None => continue,
            }
        }
        out.push((p, t));
    }
    out
}

fn places_with(rule: &Repeat, a: &Asset, frames: &[Frame]) -> Vec<((i32, i32, i32), u8)> {
    match rule {
        Repeat::OnFrame { frame } => frames.iter().find(|f| &f.name == frame)
            .map(|f| f.points.iter().map(|(_, p)| (*p, 0u8)).collect()).unwrap_or_default(),
        Repeat::Along { frame, step } => {
            let Some(f) = frames.iter().find(|f| &f.name == frame) else { return Vec::new() };
            let step = (*step).max(1) as f32;
            let mut out = Vec::new();
            for seg in f.points.windows(2) {
                let (a0, b0) = (seg[0].1, seg[1].1);
                let (dx, dy, dz) = ((b0.0 - a0.0) as f32, (b0.1 - a0.1) as f32, (b0.2 - a0.2) as f32);
                let len = (dx * dx + dy * dy + dz * dz).sqrt();
                let n = (len / step).floor().max(1.0) as usize;
                for i in 0..n {
                    let t = i as f32 / n as f32;
                    // each copy turned to face along the segment, so a
                    // wall follows its path rather than all facing north
                    let turns = (((dz.atan2(dx).to_degrees() / 90.0).round() as i32).rem_euclid(4)) as u8;
                    out.push((((a0.0 as f32 + dx * t) as i32, (a0.1 as f32 + dy * t) as i32,
                               (a0.2 as f32 + dz * t) as i32), turns));
                }
            }
            if let Some((_, last)) = f.points.last() { out.push((*last, 0)); }
            out
        }
        other => places(other, a),
    }
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
        // handled by places_with, which has the frames; unreachable here
        Repeat::OnFrame { .. } | Repeat::Along { .. } => Vec::new(),
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
            Some("frame") => {
                let Some(name) = w.get(1) else { errs.push("frame needs a name".into()); continue };
                let body = take_body(&mut lines);
                let mut points = Vec::new();
                for l in body.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
                    let t: Vec<&str> = l.split_whitespace().collect();
                    match (t.first(), t.get(1).and_then(|s| triple(s))) {
                        (Some(n), Some(p)) => points.push((n.to_lowercase(), p)),
                        _ => errs.push(format!("frame {name}: '{l}' is not `name x,y,z`")),
                    }
                }
                if points.is_empty() { errs.push(format!("frame {name} has no points")); continue; }
                p.frames.push(Frame { name: name.to_lowercase(), points });
            }
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
    // Per-copy expressions, and a HARD ERROR on anything unrecognised.
    // `repeat t ring count=8 grow=1.1` was accepted and silently
    // ignored -- it built the same eight identical towers and reported
    // success. A key that does nothing must never look like a key that
    // works.
    const KNOWN: &[&str] = &["from", "to", "step", "centre", "center", "radius", "r", "count",
                             "min", "max", "at", "face_out", "scale", "lift", "rot",
                             "on_ground", "max_slope", "jitter", "seed"];
    // a frame name is the third token of `repeat X frame NAME`, not a
    // setting, so it is skipped rather than refused
    let bare_ok = matches!(t.get(2).map(|v| v.to_lowercase()).as_deref(),
                           Some("frame") | Some("joints") | Some("along") | Some("path"));
    let mut vary = Vec::new();
    for s in t.iter().skip(if bare_ok { 4 } else { 3 }) {
        let Some((k, v)) = s.split_once('=') else {
                if ["face_out", "on_ground"].contains(s) { continue; }
            return Err(format!("'{s}' is not a setting; write key=value"));
        };
        if !KNOWN.contains(&k) {
            return Err(format!("'{k}' is not a repeat setting; known: {}", KNOWN.join(", ")));
        }
        if ["scale", "lift", "rot"].contains(&k) { vary.push((k.to_string(), v.to_string())); }
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
        Some("frame") | Some("joints") => Repeat::OnFrame {
            frame: t.get(3).ok_or("frame needs a frame name")?.to_lowercase() },
        Some("along") | Some("path") => Repeat::Along {
            frame: t.get(3).ok_or("along needs a frame name")?.to_lowercase(),
            step: num("step", 4) },
        Some("stack") => Repeat::Stack {
            at: kv("at").and_then(triple).unwrap_or((0, 0, 0)),
            count: num("count", 4).clamp(1, 128) as usize, step: num("step", 5) },
        other => return Err(format!("unknown repeat rule '{}'; known: line, ring, grid, stack",
                                    other.unwrap_or("(none)"))),
    };
    let site = Site {
        on_ground: t.contains(&"on_ground") || kv("on_ground").is_some(),
        max_slope: kv("max_slope").and_then(|v| v.parse().ok()),
        jitter: num("jitter", 0).clamp(0, 64),
        seed: num("seed", 1) as u64,
    };
    Ok(Source::Repeat { asset, rule, vary, site })
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

    use crate::VoxelWorld;

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

    #[test]
    fn each_copy_can_differ_from_the_last_which_is_what_code_would_have_bought() {
        let (prog, errs) = parse(
            "asset t\n  part s cylinder r=2 h=8 material=stone\nend\n\
             pass ring add\n  repeat t ring radius=24 count=8 scale=1+t*1.5\nend\n");
        assert!(errs.is_empty(), "{errs:?}");
        let r = run(&prog, (0, 64, 0));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        // the copies are NOT identical: the last is far bigger than the first
        let by_x: BTreeMap<i32, usize> = r.changes.iter()
            .fold(BTreeMap::new(), |mut m, c| { *m.entry(c.at.0 / 8).or_insert(0) += 1; m });
        let (small, big) = (by_x.values().min().unwrap(), by_x.values().max().unwrap());
        assert!(*big > small * 2, "the copies vary in size: {small} to {big}");
    }

    #[test]
    fn a_setting_that_does_nothing_is_an_error_not_a_shrug() {
        // `grow=1.1` was ACCEPTED and IGNORED: eight identical towers,
        // reported as success. That is worse than a parse failure,
        // because nothing tells the model its instruction was dropped.
        let (_, errs) = parse(
            "asset t\n  part s cylinder r=2 h=8 material=stone\nend\n\
             pass a add\n  repeat t ring radius=20 count=8 grow=1.1\nend\n");
        assert!(errs.iter().any(|e| e.contains("'grow' is not a repeat setting")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("known: from, to, step")), "it lists what works: {errs:?}");
    }

    #[test]
    fn a_bad_expression_is_reported_per_copy_and_the_build_survives() {
        let (prog, _) = parse(
            "asset t\n  part s cylinder r=2 h=6 material=stone\nend\n\
             pass a add\n  repeat t line from=-20,0,0 to=20,0,0 step=10 scale=nosuch*2\nend\n");
        let r = run(&prog, (0, 64, 0));
        assert!(r.errors.iter().any(|e| e.contains("not a known value")), "{:?}", r.errors);
        // and it still built, at the default scale: a bad expression
        // costs its own effect, not the whole pass
        assert!(!r.changes.is_empty(), "the build survived a bad expression");
    }

    /// A hillside with a cliff in it: flat west, a slope, then a wall.
    struct Land;
    impl crate::VoxelWorld for Land {
        fn get(&self, x: i32, y: i32, _z: i32) -> u16 {
            let h = if x < 0 { 64 } else if x < 40 { 64 + x / 4 } else { 130 };
            if y <= h { 1 } else { 0 }
        }
        fn set(&mut self, _: i32, _: i32, _: i32, _: u16) -> bool { false }
    }

    #[test]
    fn a_repeat_can_sit_on_the_ground_instead_of_floating_at_one_height() {
        let (prog, errs) = parse(
            "asset hut\n  part w box w=3 h=3 d=3 material=oak_planks\nend\n\
             pass town add\n  repeat hut line from=-30,0,0 to=30,0,0 step=10 on_ground\nend\n");
        assert!(errs.is_empty(), "{errs:?}");
        let r = run_in(&Land, &prog, (0, 64, 0));
        // the huts follow the hillside: several distinct heights
        let ys: BTreeSet<i32> = r.changes.iter().map(|c| c.at.1).collect();
        assert!(ys.len() > 6, "the town follows the ground: {} heights", ys.len());
        // and none of them floats
        for c in r.changes.iter().filter(|c| c.to != 0) {
            let ground = (0..200).rev().find(|&y| Land.get(c.at.0, y, c.at.2) != 0).unwrap();
            assert!(c.at.1 > ground - 1 && c.at.1 < ground + 12, "{:?} is not near its ground {ground}", c.at);
        }
        // without on_ground it is a flat lattice, which is what it was
        let (flat, _) = parse(
            "asset hut\n  part w box w=3 h=3 d=3 material=oak_planks\nend\n\
             pass town add\n  repeat hut line from=-30,0,0 to=30,0,0 step=10\nend\n");
        let f = run_in(&Land, &flat, (0, 64, 0));
        let fys: BTreeSet<i32> = f.changes.iter().map(|c| c.at.1).collect();
        assert!(fys.len() <= 4, "all at one height: {} -- the old behaviour", fys.len());
    }

    #[test]
    fn a_repeat_refuses_the_places_that_do_not_suit_it() {
        let (prog, _) = parse(
            "asset hut\n  part w box w=5 h=3 d=5 material=oak_planks\nend\n\
             pass town add\n  repeat hut line from=-40,0,0 to=60,0,0 step=4 on_ground max_slope=2\nend\n");
        let r = run_in(&Land, &prog, (0, 64, 0));
        assert!(!r.changes.is_empty(), "it did build where the ground suits it");
        // The direct property: FEWER PLACEMENTS survive with the rule
        // than without it, and the ones dropped are the ones whose own
        // footprint spans the cliff. Counting placements is the honest
        // measure -- inferring buildings back out of a cell list merges
        // neighbours and measures the grouping heuristic instead.
        let (loose, _) = parse(
            "asset hut\n  part w box w=5 h=3 d=5 material=oak_planks\nend\n\
             pass town add\n  repeat hut line from=-40,0,0 to=60,0,0 step=4 on_ground\nend\n");
        let l = run_in(&Land, &loose, (0, 64, 0));
        assert!(r.changes.len() < l.changes.len(),
                "the rule rejected some: {} cells with it, {} without", r.changes.len(), l.changes.len());
        // and every surviving hut sits within a few blocks of its own
        // ground, which is what "suits it" means
        for c in r.changes.iter().filter(|c| c.to != 0) {
            let g = (0..200).rev().find(|&y| Land.get(c.at.0, y, c.at.2) != 0).unwrap();
            assert!((c.at.1 - g).abs() < 8, "{:?} sits on ground {g}", c.at);
        }
    }

    #[test]
    fn jitter_makes_a_town_rather_than_a_lattice() {
        let (prog, _) = parse(
            "asset hut\n  part w box w=3 h=3 d=3 material=oak_planks\nend\n\
             pass town add\n  repeat hut grid min=-30,0,-30 max=30,0,30 step=12 jitter=4 seed=7\nend\n");
        let r = run_in(&Land, &prog, (0, 64, 0));
        // the x positions are not all multiples of the step
        let xs: BTreeSet<i32> = r.changes.iter().map(|c| c.at.0).collect();
        assert!(xs.iter().any(|x| x.rem_euclid(12) != 0), "not a lattice: {:?}",
                xs.iter().take(8).collect::<Vec<_>>());
        // and it is deterministic
        let again = run_in(&Land, &prog, (0, 64, 0));
        assert_eq!(r.changes, again.changes, "same seed, same town");
    }

    #[test]
    fn a_frame_is_an_armature_things_hang_on() {
        let (prog, errs) = parse(
            "frame bones\n\
               hips 0,20,-20\n\
               spine 0,26,0\n\
               neck 0,30,16\n\
               head 0,34,28\n\
             end\n\
             asset rib\n  part r box w=1 h=8 d=1 material=stone\nend\n\
             pass skull add\n\
               part dome sphere r=6 material=stone hollow\n\
               rel dome on head\n\
             end\n\
             pass ribs add\n  repeat rib along bones step=5\nend\n");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(prog.frames.len(), 1);
        assert_eq!(prog.frames[0].points.len(), 4);
        let r = run(&prog, (0, 0, 0));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        // The skull is at the HEAD joint, not the origin. WIDTH tells
        // it from the ribs: a sphere spreads in x, a rib is one block.
        // Height does not -- the armature rises toward the head, so the
        // ribs near the neck are legitimately as high as the skull.
        let wide: Vec<(i32,i32,i32)> = r.changes.iter().filter(|c| c.at.0.abs() > 2).map(|c| c.at).collect();
        assert!(!wide.is_empty(), "the skull sits somewhere");
        assert!(wide.iter().all(|c| c.2 > 20), "and it is at the head end: {:?}", &wide[..3.min(wide.len())]);
        // the ribs walk the whole armature, tail to head
        let zs: BTreeSet<i32> = r.changes.iter().map(|c| c.at.2).collect();
        assert!(*zs.iter().min().unwrap() < -15 && *zs.iter().max().unwrap() > 25,
                "ribs run the length of it: {:?}..{:?}", zs.iter().min(), zs.iter().max());
        // and they rise with the spine rather than sitting at one height
        let bases: BTreeSet<i32> = r.changes.iter().map(|c| c.at.1).collect();
        assert!(bases.len() > 10, "the armature has a profile: {} heights", bases.len());
    }

    #[test]
    fn one_copy_per_joint_is_different_from_walking_the_path() {
        let src = |rule: &str| format!(
            "frame f\n  a 0,0,0\n  b 0,0,30\n  c 0,0,60\nend\n\
             asset m\n  part x box w=1 h=1 d=1 material=stone\nend\n\
             pass p add\n  repeat m {rule}\nend\n");
        let (joints, _) = parse(&src("frame f"));
        let (along, _) = parse(&src("along f step=5"));
        let j = run(&joints, (0, 0, 0));
        let a = run(&along, (0, 0, 0));
        assert_eq!(j.changes.len(), 3, "one per joint");
        assert!(a.changes.len() > 10, "the path is sampled: {}", a.changes.len());
    }

    #[test]
    fn a_frame_that_is_missing_or_malformed_says_so() {
        let (_, e1) = parse("frame f\n  notapoint\nend\n");
        assert!(e1.iter().any(|e| e.contains("is not `name x,y,z`")), "{e1:?}");
        let (_, e2) = parse("frame f\nend\n");
        assert!(e2.iter().any(|e| e.contains("has no points")), "{e2:?}");
        // a repeat naming a frame that does not exist builds nothing
        // rather than crashing
        let (p, _) = parse("asset m\n  part x box w=1 h=1 d=1 material=stone\nend\n\
                            pass p add\n  repeat m frame nosuch\nend\n");
        let r = run(&p, (0, 0, 0));
        assert!(r.changes.is_empty());
    }
}
