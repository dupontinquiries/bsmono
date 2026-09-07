//! Parts and relations, without coordinates.
//!
//! WHY THIS EXISTS
//!
//! "A water sphere" produced a house. The recognizer had no word for
//! it and correctly said nothing, so the model answered -- and the
//! model's prompt is a MENU: twelve form names and an instruction to
//! choose. A small model asked for something not on the menu orders
//! the nearest thing that is.
//!
//! That is the whole problem, and it is not a prompt bug. Every build
//! looks prebuilt because every build IS prebuilt: `form house` calls
//! a function that draws a house, and the model's only expressive act
//! is picking which function. Nothing the model can say composes.
//!
//! FOUR PASSES, SPLIT WHERE THE MODEL IS GOOD
//!
//! A model is good at saying what a thing is made of and how the
//! pieces relate. It is bad at coordinates -- that finding is a year
//! of this crate's history, from towers-for-statues onward. So the
//! passes split on exactly that line:
//!
//!   1. INTERPRET  what IS this thing? Its parts, materials, what
//!                 makes it recognisable. The model answers in prose.
//!   2. COMPOSE    the parts as SHAPES with RELATIONS -- "the shell
//!                 surrounds the core", "the ring is level with the
//!                 middle" -- and no numbers that are positions.
//!   3. RESOLVE    relations to coordinates. CODE, not the model.
//!                 Deterministic, checkable, and where every previous
//!                 failure lived.
//!   4. EMIT       primitive ops the planner already runs.
//!
//! This module is passes 2 and 3: the language a model writes in, and
//! the solver that grounds it.
//!
//! WHAT A MODEL WRITES
//!
//! ```text
//! part shell sphere r=8 material=water hollow
//! part core sphere r=3 material=ice
//! part ring torus r=11 thickness=1 material=glass
//! rel core inside shell
//! rel ring around shell
//! rel shell above ground by=6
//! ```
//!
//! No coordinate appears. `r=8` is a size, not a position; `by=6` is
//! a distance, not a place. Every position comes out of the solver.

use std::collections::BTreeMap;

/// A primitive shape. Deliberately few: these compose, and a longer
/// list is a menu again.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Sphere { r: f32 },
    Box { w: f32, h: f32, d: f32 },
    Cylinder { r: f32, h: f32 },
    /// A ring: a doughnut standing in the horizontal plane.
    Torus { r: f32, thickness: f32 },
    /// A tapering shaft: a spire, a trunk, a horn.
    Cone { r: f32, h: f32 },
    /// A rising spiral: a staircase, a vine, a helix of blocks.
    Helix { r: f32, h: f32, turns: f32, thickness: f32 },
    /// A flat slab, for floors, discs and lily pads.
    Slab { w: f32, d: f32, thickness: f32 },
    /// A straight run between two other parts, resolved by the solver.
    Link { thickness: f32 },
}

impl Shape {
    /// The half-extent in each axis: how much room the part needs.
    pub fn extent(&self) -> [f32; 3] {
        match self {
            Shape::Sphere { r } => [*r, *r, *r],
            Shape::Box { w, h, d } => [w / 2.0, h / 2.0, d / 2.0],
            Shape::Cylinder { r, h } => [*r, h / 2.0, *r],
            Shape::Torus { r, thickness } => [r + thickness, *thickness, r + thickness],
            Shape::Cone { r, h } => [*r, h / 2.0, *r],
            Shape::Helix { r, h, thickness, .. } => [r + thickness, h / 2.0, r + thickness],
            Shape::Slab { w, d, thickness } => [w / 2.0, thickness / 2.0, d / 2.0],
            Shape::Link { thickness } => [*thickness, *thickness, *thickness],
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            Shape::Sphere { .. } => "sphere", Shape::Box { .. } => "box",
            Shape::Cylinder { .. } => "cylinder", Shape::Torus { .. } => "torus",
            Shape::Cone { .. } => "cone", Shape::Helix { .. } => "helix",
            Shape::Slab { .. } => "slab", Shape::Link { .. } => "link",
        }
    }
}

/// How one part sits relative to another. No coordinates: a relation
/// is a constraint the solver satisfies.
#[derive(Debug, Clone, PartialEq)]
pub enum Rel {
    /// Centres coincide; `a` fits within `b`.
    Inside,
    /// Concentric, `a` larger: a ring around a sphere.
    Around,
    /// `a` sits on top of `b`, touching.
    On,
    /// `a` is above `b` with a gap.
    Above { by: f32 },
    Below { by: f32 },
    /// Beside, in the given compass direction.
    Beside { dir: Dir, by: f32 },
    /// Centres share a height.
    LevelWith,
    /// `a` spans from `b` to a third part, named in the relation.
    Between { and: String },
    /// Evenly spaced copies of `a` on a circle about `b`.
    RingOf { count: usize, radius: f32 },
    /// Copies of `a` stacked up `b`.
    StackOn { count: usize, gap: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir { North, South, East, West }

impl Dir {
    fn unit(self) -> (f32, f32) {
        match self { Dir::North => (0.0, -1.0), Dir::South => (0.0, 1.0),
                     Dir::East => (1.0, 0.0), Dir::West => (-1.0, 0.0) }
    }
    fn parse(s: &str) -> Option<Dir> {
        match s { "north" | "n" => Some(Dir::North), "south" | "s" => Some(Dir::South),
                  "east" | "e" => Some(Dir::East), "west" | "w" => Some(Dir::West), _ => None }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub name: String,
    pub shape: Shape,
    pub material: String,
    pub hollow: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sketch {
    pub parts: Vec<Part>,
    /// (subject, relation, object). "ground" is a valid object.
    pub rels: Vec<(String, Rel, String)>,
    pub notes: Vec<String>,
}

/// A part with a place: what pass 3 produces.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub name: String,
    pub shape: Shape,
    pub material: String,
    pub hollow: bool,
    /// Centre, in world coordinates.
    pub at: (i32, i32, i32),
    /// Copies, for RingOf and StackOn.
    pub extra: Vec<(i32, i32, i32)>,
    /// For a `link`: the two ends it spans. A link has no size of its
    /// own -- its whole shape is where it goes -- so without this it
    /// resolved to a point and drew nothing.
    pub span: Option<((i32, i32, i32), (i32, i32, i32))>,
}

// ── parsing ──────────────────────────────────────────────────────────

fn kv(t: &[&str], k: &str) -> Option<f32> {
    t.iter().find_map(|s| s.strip_prefix(&format!("{k}="))).and_then(|v| v.parse().ok())
}

/// Parse the sketch language. Unknown lines are collected as notes
/// rather than refused: a model that adds a comment should not lose
/// its whole sketch.
pub fn parse(src: &str) -> (Sketch, Vec<String>) {
    let mut s = Sketch::default();
    let mut errs = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let t: Vec<&str> = line.split_whitespace().collect();
        match t.first().map(|v| v.to_lowercase()) {
            Some(v) if v == "part" => {
                let (Some(name), Some(kind)) = (t.get(1), t.get(2)) else {
                    errs.push(format!("part needs a name and a shape: {line:?}")); continue;
                };
                let shape = match kind.to_lowercase().as_str() {
                    "sphere" | "ball" | "orb" => Shape::Sphere { r: kv(&t, "r").unwrap_or(5.0) },
                    "box" | "cube" | "block" => {
                        let w = kv(&t, "w").or(kv(&t, "size")).unwrap_or(6.0);
                        Shape::Box { w, h: kv(&t, "h").unwrap_or(w), d: kv(&t, "d").unwrap_or(w) }
                    }
                    "cylinder" | "column" | "pillar" | "shaft" =>
                        Shape::Cylinder { r: kv(&t, "r").unwrap_or(2.0), h: kv(&t, "h").unwrap_or(8.0) },
                    "torus" | "ring" | "hoop" =>
                        Shape::Torus { r: kv(&t, "r").unwrap_or(8.0),
                                       thickness: kv(&t, "thickness").or(kv(&t, "t")).unwrap_or(1.0) },
                    "cone" | "spire" | "horn" =>
                        Shape::Cone { r: kv(&t, "r").unwrap_or(4.0), h: kv(&t, "h").unwrap_or(10.0) },
                    "helix" | "spiral" =>
                        Shape::Helix { r: kv(&t, "r").unwrap_or(5.0), h: kv(&t, "h").unwrap_or(20.0),
                                       turns: kv(&t, "turns").unwrap_or(3.0),
                                       thickness: kv(&t, "thickness").or(kv(&t, "t")).unwrap_or(1.5) },
                    "slab" | "disc" | "platform" | "floor" => {
                        let w = kv(&t, "w").or(kv(&t, "size")).unwrap_or(10.0);
                        Shape::Slab { w, d: kv(&t, "d").unwrap_or(w),
                                      thickness: kv(&t, "thickness").or(kv(&t, "t")).unwrap_or(1.0) }
                    }
                    "link" | "beam" | "bridge" =>
                        Shape::Link { thickness: kv(&t, "thickness").or(kv(&t, "t")).unwrap_or(1.0) },
                    other => { errs.push(format!("unknown shape '{other}'; known: sphere, box, cylinder, \
                                                  torus, cone, helix, slab, link")); continue; }
                };
                s.parts.push(Part {
                    name: name.to_lowercase(),
                    shape,
                    material: t.iter().find_map(|x| x.strip_prefix("material="))
                        .or_else(|| t.iter().find_map(|x| x.strip_prefix("block=")))
                        .unwrap_or("stone").to_string(),
                    hollow: t.iter().any(|x| *x == "hollow"),
                });
            }
            Some(v) if v == "rel" => {
                let (Some(a), Some(kind)) = (t.get(1), t.get(2)) else {
                    errs.push(format!("rel needs a subject and a relation: {line:?}")); continue;
                };
                let obj = t.get(3).copied().unwrap_or("ground").to_lowercase();
                let by = kv(&t, "by").unwrap_or(2.0);
                let rel = match kind.to_lowercase().as_str() {
                    "inside" | "in" | "within" => Rel::Inside,
                    "around" | "surrounding" | "encircling" => Rel::Around,
                    "on" | "atop" => Rel::On,
                    "above" | "over" => Rel::Above { by },
                    "below" | "under" | "beneath" => Rel::Below { by },
                    "level" | "level_with" | "levelwith" => Rel::LevelWith,
                    "beside" | "next_to" | "nextto" => Rel::Beside {
                        dir: t.iter().find_map(|x| x.strip_prefix("dir=")).and_then(Dir::parse)
                            .unwrap_or(Dir::East), by },
                    "between" => match t.get(4) {
                        Some(third) => Rel::Between { and: third.to_lowercase() },
                        None => { errs.push(format!("between needs two objects: {line:?}")); continue; }
                    },
                    "ring_of" | "ringof" | "around_in_a_ring" => Rel::RingOf {
                        count: kv(&t, "count").unwrap_or(6.0).clamp(2.0, 64.0) as usize,
                        radius: kv(&t, "radius").or(kv(&t, "r")).unwrap_or(10.0) },
                    "stack" | "stack_on" | "stackon" => Rel::StackOn {
                        count: kv(&t, "count").unwrap_or(3.0).clamp(2.0, 64.0) as usize,
                        gap: kv(&t, "gap").unwrap_or(0.0) },
                    other => { errs.push(format!("unknown relation '{other}'")); continue; }
                };
                s.rels.push((a.to_lowercase(), rel, obj));
            }
            _ => s.notes.push(line.to_string()),
        }
    }
    // a relation naming a part that does not exist is an error worth
    // saying, not a silent no-op
    let names: Vec<&str> = s.parts.iter().map(|p| p.name.as_str()).collect();
    for (a, r, b) in &s.rels {
        for n in [a, b] {
            if n != "ground" && !names.contains(&n.as_str()) {
                errs.push(format!("relation names '{n}', which is not a part"));
            }
        }
        if let Rel::Between { and } = r {
            if and != "ground" && !names.contains(&and.as_str()) {
                errs.push(format!("between names '{and}', which is not a part"));
            }
        }
    }
    (s, errs)
}

// ── the solver: pass 3 ───────────────────────────────────────────────

/// Resolve a sketch to placed parts about an anchor.
///
/// This is the pass that is CODE. Relations are satisfied in dependency
/// order -- a part whose position depends on another waits for it --
/// and anything still unplaced after that is anchored, so a sketch with
/// a cycle or a missing relation still builds something rather than
/// nothing.
pub fn resolve(s: &Sketch, anchor: (i32, i32, i32)) -> (Vec<Placed>, Vec<String>) {
    let mut notes = Vec::new();
    let mut pos: BTreeMap<String, (f32, f32, f32)> = BTreeMap::new();
    let mut extra: BTreeMap<String, Vec<(f32, f32, f32)>> = BTreeMap::new();
    let mut spans: BTreeMap<String, ((f32, f32, f32), (f32, f32, f32))> = BTreeMap::new();
    let by_name: BTreeMap<&str, &Part> = s.parts.iter().map(|p| (p.name.as_str(), p)).collect();
    let ground = anchor.1 as f32;
    // A part with no relation at all sits at the anchor, on the ground.
    let unrelated: Vec<&Part> = s.parts.iter()
        .filter(|p| !s.rels.iter().any(|(a, _, _)| *a == p.name)).collect();
    for p in &unrelated {
        pos.insert(p.name.clone(), (anchor.0 as f32, ground + p.shape.extent()[1], anchor.2 as f32));
    }
    // Then relations, repeatedly, until nothing more can be placed.
    // Ten rounds is more than any sensible sketch needs and terminates
    // a cycle without a graph.
    for _ in 0..10 {
        let mut progress = false;
        for (a, rel, b) in &s.rels {
            if pos.contains_key(a) { continue; }
            let Some(pa) = by_name.get(a.as_str()) else { continue };
            let base = if b == "ground" { Some((anchor.0 as f32, ground, anchor.2 as f32)) }
                       else { pos.get(b).copied() };
            let Some(bp) = base else { continue };
            let bext = by_name.get(b.as_str()).map(|p| p.shape.extent()).unwrap_or([0.0; 3]);
            let aext = pa.shape.extent();
            let placed = match rel {
                Rel::Inside | Rel::Around | Rel::LevelWith => bp,
                Rel::On => (bp.0, bp.1 + bext[1] + aext[1], bp.2),
                Rel::Above { by } => (bp.0, bp.1 + bext[1] + aext[1] + by, bp.2),
                Rel::Below { by } => (bp.0, bp.1 - bext[1] - aext[1] - by, bp.2),
                Rel::Beside { dir, by } => {
                    let (dx, dz) = dir.unit();
                    (bp.0 + dx * (bext[0] + aext[0] + by), bp.1, bp.2 + dz * (bext[2] + aext[2] + by))
                }
                Rel::Between { and } => {
                    let other = if and == "ground" { Some((anchor.0 as f32, ground, anchor.2 as f32)) }
                                else { pos.get(and).copied() };
                    let Some(o) = other else { continue };
                    spans.insert(a.clone(), (bp, o));
                    ((bp.0 + o.0) / 2.0, (bp.1 + o.1) / 2.0, (bp.2 + o.2) / 2.0)
                }
                Rel::RingOf { count, radius } => {
                    let mut copies = Vec::new();
                    for i in 0..*count {
                        let th = i as f32 / *count as f32 * std::f32::consts::TAU;
                        copies.push((bp.0 + radius * th.cos(), bp.1 + aext[1], bp.2 + radius * th.sin()));
                    }
                    let first = copies.remove(0);
                    extra.insert(a.clone(), copies);
                    first
                }
                Rel::StackOn { count, gap } => {
                    let step = aext[1] * 2.0 + gap;
                    let bottom = (bp.0, bp.1 + bext[1] + aext[1], bp.2);
                    let copies: Vec<(f32, f32, f32)> = (1..*count)
                        .map(|i| (bottom.0, bottom.1 + step * i as f32, bottom.2)).collect();
                    extra.insert(a.clone(), copies);
                    bottom
                }
            };
            pos.insert(a.clone(), placed);
            progress = true;
        }
        if !progress { break; }
    }
    // Anything still unplaced: a cycle, or a relation to something that
    // never resolved. Anchor it and say so -- half a sketch built is
    // better than a refusal, and the note tells the next pass what to
    // fix.
    for p in &s.parts {
        if matches!(p.shape, Shape::Link { .. }) && !spans.contains_key(&p.name) {
            notes.push(format!("'{}' is a link with no `between`: it spans nothing and draws nothing",
                               p.name));
        }
        if !pos.contains_key(&p.name) {
            notes.push(format!("'{}' could not be placed from its relations; anchored", p.name));
            pos.insert(p.name.clone(), (anchor.0 as f32, ground + p.shape.extent()[1], anchor.2 as f32));
        }
    }
    // A part INSIDE another must actually fit: shrink it if it does not.
    let mut shrunk: BTreeMap<String, Shape> = BTreeMap::new();
    for (a, rel, b) in &s.rels {
        if !matches!(rel, Rel::Inside) { continue; }
        let (Some(pa), Some(pb)) = (by_name.get(a.as_str()), by_name.get(b.as_str())) else { continue };
        let (ae, be) = (pa.shape.extent(), pb.shape.extent());
        if ae[0] < be[0] && ae[1] < be[1] { continue; }
        let f = (be[0].min(be[1]).min(be[2]) * 0.6 / ae[0].max(ae[1]).max(ae[2]).max(0.1)).clamp(0.1, 1.0);
        notes.push(format!("'{a}' was too big to sit inside '{b}'; scaled to {:.0}%", f * 100.0));
        shrunk.insert(a.clone(), scale(&pa.shape, f));
    }
    let out = s.parts.iter().map(|p| {
        let c = pos[&p.name];
        Placed {
            name: p.name.clone(),
            shape: shrunk.get(&p.name).cloned().unwrap_or_else(|| p.shape.clone()),
            material: p.material.clone(),
            hollow: p.hollow,
            at: (c.0.round() as i32, c.1.round() as i32, c.2.round() as i32),
            extra: extra.get(&p.name).map(|v| v.iter()
                .map(|c| (c.0.round() as i32, c.1.round() as i32, c.2.round() as i32)).collect())
                .unwrap_or_default(),
            span: spans.get(&p.name).map(|(a, b)| (
                (a.0.round() as i32, a.1.round() as i32, a.2.round() as i32),
                (b.0.round() as i32, b.1.round() as i32, b.2.round() as i32))),
        }
    }).collect();
    (out, notes)
}

fn scale(s: &Shape, f: f32) -> Shape {
    match s {
        Shape::Sphere { r } => Shape::Sphere { r: r * f },
        Shape::Box { w, h, d } => Shape::Box { w: w * f, h: h * f, d: d * f },
        Shape::Cylinder { r, h } => Shape::Cylinder { r: r * f, h: h * f },
        Shape::Torus { r, thickness } => Shape::Torus { r: r * f, thickness: thickness.max(1.0) },
        Shape::Cone { r, h } => Shape::Cone { r: r * f, h: h * f },
        Shape::Helix { r, h, turns, thickness } =>
            Shape::Helix { r: r * f, h: h * f, turns: *turns, thickness: thickness.max(1.0) },
        Shape::Slab { w, d, thickness } => Shape::Slab { w: w * f, d: d * f, thickness: thickness.max(1.0) },
        Shape::Link { thickness } => Shape::Link { thickness: *thickness },
    }
}

// ── pass 4: emit ─────────────────────────────────────────────────────

/// The IR for a placed part. Shapes the planner has an op for become
/// that op; the rest are drawn from cells directly by `cells`.
pub fn to_ir(placed: &[Placed]) -> String {
    let mut out = String::new();
    for p in placed {
        for at in std::iter::once(&p.at).chain(p.extra.iter()) {
            let line = match &p.shape {
                Shape::Sphere { r } => format!("sphere {},{} r={} block={}{}",
                    at.0, at.2, r.round().max(1.0) as i32, p.material, if p.hollow { " hollow" } else { "" }),
                Shape::Cylinder { r, h } => format!("cylinder {},{} r={} h={} block={}{}",
                    at.0, at.2, r.round().max(1.0) as i32, h.round().max(1.0) as i32, p.material,
                    if p.hollow { " hollow" } else { "" }),
                Shape::Box { w, h, d } => format!("fill {},{},{} {},{},{} block={}",
                    at.0 - (w / 2.0) as i32, at.1 - (h / 2.0) as i32, at.2 - (d / 2.0) as i32,
                    at.0 + (w / 2.0) as i32, at.1 + (h / 2.0) as i32, at.2 + (d / 2.0) as i32, p.material),
                Shape::Slab { w, d, thickness } => format!("fill {},{},{} {},{},{} block={}",
                    at.0 - (w / 2.0) as i32, at.1, at.2 - (d / 2.0) as i32,
                    at.0 + (w / 2.0) as i32, at.1 + thickness.max(1.0) as i32 - 1, at.2 + (d / 2.0) as i32,
                    p.material),
                // torus, cone, helix and link have no op: they are
                // rasterised by `cells` and emitted as a stamp
                _ => continue,
            };
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// The cells a placed part occupies, for the shapes with no IR op.
/// Sphere, box, cylinder and slab go through the planner's own ops;
/// these are the ones only this module knows how to draw.
pub fn cells(p: &Placed, block: u16) -> Vec<(i32, i32, i32, u16)> {
    // EVERY copy, not just the first. `ring_of` and `stack` put their
    // copies in `extra`, and drawing only `at` meant a stack of five
    // rings rasterised as one ring -- the IR path handles `extra`
    // correctly, so only the shapes without an op were affected, which
    // is exactly the shapes this function exists for.
    let mut out = Vec::new();
    for at in std::iter::once(&p.at).chain(p.extra.iter()) {
        out.extend(cells_at(p, *at, block));
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn cells_at(p: &Placed, at_i: (i32, i32, i32), block: u16) -> Vec<(i32, i32, i32, u16)> {
    let mut out = Vec::new();
    let at = (at_i.0 as f32, at_i.1 as f32, at_i.2 as f32);
    let mut push = |x: f32, y: f32, z: f32| out.push((x.round() as i32, y.round() as i32, z.round() as i32, block));
    match &p.shape {
        Shape::Torus { r, thickness } => {
            let t = thickness.max(1.0);
            let steps = ((r * std::f32::consts::TAU) as usize).clamp(16, 720);
            for i in 0..steps {
                let th = i as f32 / steps as f32 * std::f32::consts::TAU;
                let (cx, cz) = (at.0 + r * th.cos(), at.2 + r * th.sin());
                let ti = t.ceil() as i32;
                for dx in -ti..=ti { for dy in -ti..=ti { for dz in -ti..=ti {
                    if ((dx * dx + dy * dy + dz * dz) as f32).sqrt() <= t {
                        push(cx + dx as f32, at.1 + dy as f32, cz + dz as f32);
                    }
                }}}
            }
        }
        Shape::Cone { r, h } => {
            let hi = h.max(1.0);
            for k in 0..=hi as i32 {
                let f = 1.0 - k as f32 / hi;
                let rr = (r * f).max(0.0);
                let ri = rr.ceil() as i32;
                for dx in -ri..=ri { for dz in -ri..=ri {
                    if ((dx * dx + dz * dz) as f32).sqrt() <= rr {
                        push(at.0 + dx as f32, at.1 - hi / 2.0 + k as f32, at.2 + dz as f32);
                    }
                }}
            }
        }
        Shape::Helix { r, h, turns, thickness } => {
            let steps = ((h.max(1.0) * turns.max(0.25)) as usize * 4).clamp(16, 4000);
            let t = thickness.max(1.0);
            for i in 0..=steps {
                let f = i as f32 / steps as f32;
                let th = f * turns * std::f32::consts::TAU;
                let (cx, cy, cz) = (at.0 + r * th.cos(), at.1 - h / 2.0 + h * f, at.2 + r * th.sin());
                let ti = t.ceil() as i32;
                for dx in -ti..=ti { for dy in -ti..=ti { for dz in -ti..=ti {
                    if ((dx * dx + dy * dy + dz * dz) as f32).sqrt() <= t {
                        push(cx + dx as f32, cy + dy as f32, cz + dz as f32);
                    }
                }}}
            }
        }
        Shape::Link { thickness } => {
            // A link with no ends is a link to nowhere: the sketch said
            // to span something and never said what, so nothing is
            // drawn and the resolver's note says why.
            let Some((a, b)) = p.span else { return Vec::new() };
            let (dx, dy, dz) = ((b.0 - a.0) as f32, (b.1 - a.1) as f32, (b.2 - a.2) as f32);
            let len = (dx * dx + dy * dy + dz * dz).sqrt().max(1.0);
            let t = thickness.max(1.0);
            let ti = t.ceil() as i32;
            for i in 0..=(len.ceil() as i32) {
                let f = i as f32 / len;
                let (cx, cy, cz) = (a.0 as f32 + dx * f, a.1 as f32 + dy * f, a.2 as f32 + dz * f);
                for ox in -ti..=ti { for oy in -ti..=ti { for oz in -ti..=ti {
                    if ((ox * ox + oy * oy + oz * oz) as f32).sqrt() <= t {
                        push(cx + ox as f32, cy + oy as f32, cz + oz as f32);
                    }
                }}}
            }
        }
        _ => {}
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Which parts `to_ir` could not express, so the caller knows to
/// rasterise them.
pub fn needs_cells(placed: &[Placed]) -> Vec<&Placed> {
    placed.iter().filter(|p| matches!(p.shape,
        Shape::Torus { .. } | Shape::Cone { .. } | Shape::Helix { .. } | Shape::Link { .. })).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_water_sphere_is_a_water_sphere() {
        // The failing case: this produced a HOUSE, because the model's
        // only vocabulary was twelve form names.
        let (s, errs) = parse("part shell sphere r=8 material=water hollow\nrel shell above ground by=6\n");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(s.parts.len(), 1);
        assert_eq!(s.parts[0].material, "water");
        assert!(s.parts[0].hollow);
        let (placed, notes) = resolve(&s, (0, 64, 0));
        assert!(notes.is_empty(), "{notes:?}");
        // 6 above the ground, plus its own radius: floating
        assert_eq!(placed[0].at, (0, 78, 0));
        let ir = to_ir(&placed);
        assert!(ir.contains("sphere 0,0 r=8 block=water hollow"), "{ir}");
        assert!(crate::plan::line_is_valid_ir(ir.trim()), "{ir}");
    }

    #[test]
    fn relations_resolve_in_dependency_order_whatever_the_line_order() {
        // the ring depends on the shell, which depends on the ground,
        // and they are written backwards
        let src = "part ring torus r=12 thickness=1 material=glass\n\
                   part core sphere r=3 material=ice\n\
                   part shell sphere r=8 material=water hollow\n\
                   rel ring around shell\n\
                   rel core inside shell\n\
                   rel shell above ground by=10\n";
        let (s, errs) = parse(src);
        assert!(errs.is_empty(), "{errs:?}");
        let (placed, notes) = resolve(&s, (0, 64, 0));
        assert!(notes.is_empty(), "{notes:?}");
        let get = |n: &str| placed.iter().find(|p| p.name == n).unwrap();
        // all three concentric, floating together
        assert_eq!(get("shell").at, (0, 82, 0));
        assert_eq!(get("core").at, get("shell").at, "inside means concentric");
        assert_eq!(get("ring").at, get("shell").at, "around means concentric");
    }

    #[test]
    fn a_thing_too_big_to_be_inside_is_scaled_not_refused() {
        let (s, _) = parse("part shell sphere r=4 material=glass\n\
                            part core sphere r=20 material=stone\n\
                            rel core inside shell\n");
        let (placed, notes) = resolve(&s, (0, 64, 0));
        assert!(notes.iter().any(|n| n.contains("too big to sit inside")), "{notes:?}");
        let core = placed.iter().find(|p| p.name == "core").unwrap();
        match core.shape { Shape::Sphere { r } => assert!(r < 4.0, "shrunk to {r}"), ref o => panic!("{o:?}") }
    }

    #[test]
    fn a_ring_of_and_a_stack_make_copies_the_solver_places() {
        let (s, _) = parse("part hub cylinder r=2 h=10 material=stone\n\
                            part post cylinder r=1 h=6 material=oak_planks\n\
                            rel post ring_of hub count=6 radius=14\n");
        let (placed, _) = resolve(&s, (0, 64, 0));
        let post = placed.iter().find(|p| p.name == "post").unwrap();
        assert_eq!(post.extra.len(), 5, "six in all, one is the part itself");
        // spread on a circle of the given radius
        let all: Vec<(i32, i32, i32)> = std::iter::once(post.at).chain(post.extra.iter().copied()).collect();
        for p in &all {
            let d = (((p.0 * p.0 + p.2 * p.2) as f32).sqrt() - 14.0).abs();
            assert!(d < 1.5, "{p:?} is not on the circle");
        }
        // and a stack
        let (s2, _) = parse("part base slab w=8 d=8 material=stone\n\
                             part tier slab w=6 d=6 material=stone\n\
                             rel tier stack base count=4 gap=2\n");
        let (p2, _) = resolve(&s2, (0, 64, 0));
        let tier = p2.iter().find(|p| p.name == "tier").unwrap();
        assert_eq!(tier.extra.len(), 3);
        let ys: Vec<i32> = std::iter::once(tier.at.1).chain(tier.extra.iter().map(|c| c.1)).collect();
        assert!(ys.windows(2).all(|w| w[1] > w[0]), "each tier above the last: {ys:?}");
    }

    #[test]
    fn shapes_with_no_op_are_rasterised_and_the_caller_is_told() {
        let (s, _) = parse("part ring torus r=10 thickness=2 material=glass\n\
                            part spire cone r=4 h=16 material=quartz\n\
                            part stair helix r=6 h=24 turns=3 thickness=1.5 material=stone\n");
        let (placed, _) = resolve(&s, (0, 64, 0));
        assert_eq!(needs_cells(&placed).len(), 3, "none of these has an IR op");
        assert!(to_ir(&placed).trim().is_empty(), "so none is emitted as a line");
        for p in &placed {
            let c = cells(p, 1);
            assert!(!c.is_empty(), "{} rasterises to nothing", p.name);
            // and it is roughly the size asked for
            let w = c.iter().map(|c| c.0).max().unwrap() - c.iter().map(|c| c.0).min().unwrap();
            let e = p.shape.extent();
            assert!((w as f32 - e[0] * 2.0).abs() < 6.0, "{}: {w} wide, expected ~{}", p.name, e[0] * 2.0);
        }
        // the helix rises: its cells span its height
        let helix = placed.iter().find(|p| p.name == "stair").unwrap();
        let c = cells(helix, 1);
        let h = c.iter().map(|c| c.1).max().unwrap() - c.iter().map(|c| c.1).min().unwrap();
        assert!(h >= 20, "a 24-tall helix spans {h}");
    }

    #[test]
    fn a_broken_sketch_says_what_is_wrong_and_still_builds_what_it_can() {
        let (s, errs) = parse("part a sphere r=4 material=stone\n\
                               rel a above nonesuch\n\
                               part b frobnicate r=2\n\
                               rel a sideways b\n\
                               this line is prose\n");
        assert!(errs.iter().any(|e| e.contains("nonesuch")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("unknown shape 'frobnicate'")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("unknown relation 'sideways'")), "{errs:?}");
        assert_eq!(s.notes, vec!["this line is prose"], "prose is kept, not fatal");
        // the one good part still resolves
        let (placed, notes) = resolve(&s, (0, 64, 0));
        assert_eq!(placed.len(), 1);
        assert!(notes.iter().any(|n| n.contains("could not be placed")), "{notes:?}");
    }

    #[test]
    fn every_shape_the_parser_knows_resolves_and_produces_something() {
        for line in ["part x sphere r=5 material=water", "part x box w=6 h=8 d=6 material=stone",
                     "part x cylinder r=3 h=12 material=stone", "part x torus r=9 thickness=2 material=glass",
                     "part x cone r=5 h=14 material=quartz", "part x helix r=6 h=20 turns=2 material=stone",
                     "part x slab w=12 d=12 material=stone"] {
            let (s, errs) = parse(line);
            assert!(errs.is_empty(), "{line}: {errs:?}");
            let (placed, _) = resolve(&s, (0, 64, 0));
            assert_eq!(placed.len(), 1, "{line}");
            let ir = to_ir(&placed);
            let raster = cells(&placed[0], 1);
            assert!(!ir.trim().is_empty() || !raster.is_empty(),
                    "{line} produces neither an op nor cells");
            for l in ir.lines() {
                assert!(crate::plan::line_is_valid_ir(l), "{line} -> {l:?}");
            }
        }
    }

    #[test]
    fn a_link_spans_what_it_is_between_and_says_so_when_it_is_between_nothing() {
        let (sk, _) = parse("part a cylinder r=2 h=10 material=stone\n\
                             part b cylinder r=2 h=10 material=stone\n\
                             part span link thickness=1 material=oak_planks\n\
                             rel b beside a dir=east by=30\n\
                             rel span between a b\n");
        let (placed, notes) = resolve(&sk, (0, 64, 0));
        assert!(notes.is_empty(), "{notes:?}");
        let span = placed.iter().find(|p| p.name == "span").unwrap();
        let (from, to) = span.span.expect("its two ends");
        assert!((to.0 - from.0).abs() > 20, "it reaches across: {from:?} to {to:?}");
        let c = cells(span, 1);
        assert!(c.len() > 60, "a span of {} cells", c.len());
        let w = c.iter().map(|c| c.0).max().unwrap() - c.iter().map(|c| c.0).min().unwrap();
        assert!(w > 20, "the cells run the whole way: {w}");
        // and a link with no `between`
        let (lone, _) = parse("part span link thickness=1 material=oak_planks\n");
        let (p2, n2) = resolve(&lone, (0, 64, 0));
        assert!(n2.iter().any(|n| n.contains("spans nothing")), "{n2:?}");
        assert!(cells(&p2[0], 1).is_empty());
    }
}
