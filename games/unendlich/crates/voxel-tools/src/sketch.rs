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

use std::collections::{BTreeMap, BTreeSet};

/// A primitive shape. Deliberately few: these compose, and a longer
/// list is a menu again.
/// The cross-section a sweep carries. Few on purpose: the SHAPE of a
/// section matters far less than the path and the radii, and a long
/// list of sections is a menu inside a generalisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section { Circle, Square, Diamond, Cross, Hollow }

impl Section {
    pub fn parse(s: &str) -> Option<Section> {
        Some(match s.to_lowercase().as_str() {
            "circle" | "round" | "circular" => Section::Circle,
            "square" | "box" | "rect" => Section::Square,
            "diamond" | "rhombus" => Section::Diamond,
            "cross" | "plus" | "x" => Section::Cross,
            "hollow" | "tube" | "pipe" | "ring" => Section::Hollow,
            _ => return None,
        })
    }
    /// Is this offset inside the section at radius `r`?
    fn holds(self, dx: f32, dy: f32, r: f32) -> bool {
        let d = (dx * dx + dy * dy).sqrt();
        match self {
            Section::Circle => d <= r,
            Section::Square => dx.abs() <= r && dy.abs() <= r,
            Section::Diamond => dx.abs() + dy.abs() <= r,
            Section::Cross => (dx.abs() <= r * 0.4 && dy.abs() <= r)
                           || (dy.abs() <= r * 0.4 && dx.abs() <= r),
            Section::Hollow => d <= r && d >= (r - 1.5).max(0.0),
        }
    }
}

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
    /// A sphere swept along a segment: the fundamental ORGANIC form.
    /// A limb, a neck, a finger, a tail segment. `dir` is where it
    /// points, in degrees about Y, and `pitch` how far it rises.
    /// Tapers from `r` at the base to `r2` at the tip -- a limb of
    /// uniform thickness reads as plumbing.
    Capsule { r: f32, r2: f32, len: f32, dir: f32, pitch: f32 },
    /// A SECTION SWEPT ALONG A PATH: the most general shape here, and
    /// the one the others are special cases of.
    ///
    /// `profile` revolves a silhouette around a FIXED VERTICAL AXIS, so
    /// it makes a vase, a chess piece, a tower -- and nothing that
    /// bends. A sweep takes a path through space and carries a
    /// cross-section along it, its radius varying as it goes.
    ///
    ///   profile  = a sweep along a straight vertical path
    ///   spine    = a sweep with two radii and a circular section
    ///   capsule  = a sweep along one segment
    ///   torus    = a sweep along a circle
    ///   helix    = a sweep along a helix
    ///
    /// And it adds what none of them can do: a horn that curves, a
    /// river that meanders, a branch, a claw, a hook, a corridor that
    /// turns a corner, a boat hull.
    ///
    /// WHY THIS IS THE ANSWER TO "how do you represent any object".
    /// Not because it can express everything -- it cannot -- but
    /// because it is the most general thing a MODEL CAN WRITE. A model
    /// cannot write `f(x,y,z) < 0`; it writes a list of numbers, and a
    /// path with radii along it is a list of numbers.
    Sweep { path: Vec<(f32, f32, f32)>, radii: Vec<f32>, section: Section },
    /// A SILHOUETTE SPUN AROUND A VERTICAL AXIS: radius at each height,
    /// from the base up. The single most general shape there is, and
    /// the one that was missing.
    ///
    /// A breadth test over 43 ordinary requests found 27 with no
    /// scaffolding at all -- a mushroom, an hourglass, a chess piece, a
    /// lantern, a pagoda, a vase. Every one of them is a profile: the
    /// mushroom narrow then flaring, the hourglass wide-pinch-wide, the
    /// chess piece a stack of discs. Adding a form for each is a bigger
    /// menu; adding the profile is the generalisation.
    ///
    /// And it is what a small model CAN write. Not code, not a body
    /// plan -- a list of numbers.
    Profile { radii: Vec<f32>, h: f32, square: bool },
    /// A chain of capsules following a curve: a spine, a neck, a tail.
    /// This is what makes a neck a neck rather than a stack of balls --
    /// the whole reason the reference dragon reads as an animal.
    Spine { r: f32, r2: f32, len: f32, dir: f32, arch: f32, segments: usize },
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
            // A capsule's extent is its ACTUAL reach in each axis given
            // where it points, not `len` in all three. Treating a
            // vertical limb as if it were also `len` wide sent a pair
            // of arms nineteen blocks out from the body: the offset for
            // `pair` is half-width plus half-width, and the half-width
            // was wrong by the whole length.
            Shape::Capsule { r, r2, len, dir, pitch } => {
                let rr = r.max(*r2);
                let (a, p) = (dir.to_radians(), pitch.to_radians());
                [(len * a.cos() * p.cos()).abs() / 2.0 + rr,
                 (len * p.sin()).abs() / 2.0 + rr,
                 (len * a.sin() * p.cos()).abs() / 2.0 + rr]
            }
            Shape::Sweep { path, radii, .. } => {
                let m = radii.iter().cloned().fold(1.0f32, f32::max);
                let f = |g: fn(&(f32, f32, f32)) -> f32| {
                    let lo = path.iter().map(g).fold(f32::MAX, f32::min);
                    let hi = path.iter().map(g).fold(f32::MIN, f32::max);
                    (hi - lo) / 2.0 + m
                };
                [f(|p| p.0), f(|p| p.1), f(|p| p.2)]
            }
            Shape::Profile { radii, h, .. } => {
                let m = radii.iter().cloned().fold(1.0f32, f32::max);
                [m, h / 2.0, m]
            }
            Shape::Spine { r, r2, len, dir, arch, .. } => {
                let rr = r.max(*r2);
                let a = dir.to_radians();
                [(len * a.cos()).abs() / 2.0 + rr,
                 (len * arch).abs() / 2.0 + rr,
                 (len * a.sin()).abs() / 2.0 + rr]
            }
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            Shape::Sphere { .. } => "sphere", Shape::Box { .. } => "box",
            Shape::Cylinder { .. } => "cylinder", Shape::Torus { .. } => "torus",
            Shape::Cone { .. } => "cone", Shape::Helix { .. } => "helix",
            Shape::Slab { .. } => "slab", Shape::Link { .. } => "link",
            Shape::Capsule { .. } => "capsule", Shape::Spine { .. } => "spine",
            Shape::Profile { .. } => "profile", Shape::Sweep { .. } => "sweep",
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
    /// The part's ORIGIN sits on `b`'s surface and it swings from
    /// there. For a limb this is the joint: an arm hangs FROM a
    /// shoulder, it is not centred beside a torso.
    ///
    /// `pair` offsets by half-extents, which for a limb that swings
    /// means the offset includes its whole reach -- a pair of arms
    /// ended up nineteen blocks out from a body eleven wide. A limb
    /// needs a joint, not a bounding box.
    /// `contra` is CONTRAPPOSTO: how much the second of a pair differs
    /// from the first. 0 is an exact mirror, which is what a pair
    /// always was and is the stiffness itself -- the reference
    /// critique's first complaint about its own statue was "the pose
    /// is too symmetrical". A real standing figure bears its weight on
    /// one leg; that hip rides high, the shoulders counter-tilt, and
    /// nothing about it mirrors.
    Joint { at: f32, out: f32, pair: bool, contra: f32 },
    /// Put it at an OFFSET from the anchor, in blocks. The one
    /// relation that is a position, and it exists because a solver
    /// upstream has already decided where things go: a room layout
    /// computes its own coordinates, and `brief::to_program` had no
    /// way to say them -- so it emitted every room at the anchor and a
    /// twelve-room plan carved one pile of overlapping voids in the
    /// middle. The layout's whole output was being discarded.
    ///
    /// Not for a model to write. The prompts do not mention it, and
    /// the note in `compose.rs` about coordinates still stands: this
    /// is for code that has solved the positions.
    Offset { dx: f32, dy: f32, dz: f32 },
    /// Stretch to touch the ground below wherever it ends up. The
    /// part's `len` becomes the distance from its joint to the floor,
    /// so a leg reaches, a pier founds, a column meets its footing.
    Reaches,
    /// A MIRRORED PAIR either side of `b`: one left, one right, the
    /// second a mirror image of the first.
    ///
    /// Without this a model wrote `rel legL below torso` and `rel legR
    /// below torso` and got both legs in the SAME PLACE -- there was
    /// no way to say "one on each side", so a statue had one fused leg
    /// and a slab of arm. Symmetry is the commonest thing in anything
    /// built or grown, and it had no word.
    PairAcross { by: f32 },
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
    /// Names the caller promises exist elsewhere -- parts from an
    /// earlier pass. Set by `program`, empty for a lone sketch.
    pub assume_known: BTreeSet<String>,
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
    /// The copies in `extra` are MIRROR images, not translations: a
    /// right arm reaching forward is a left arm reaching forward, not
    /// a left arm reaching backward.
    pub mirror_extra: bool,
    /// How far the mirrored copy departs from a true mirror. `None` is
    /// an exact mirror; a value tilts the copy's own heading too, so a
    /// contrapposto leg does not merely sit higher, it angles.
    pub lean_extra: Option<f32>,
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
                    "sweep" | "swept" | "along" | "extrude" => {
                        let nums = |v: &str| -> Vec<f32> {
                            v.split(',').filter_map(|n| n.trim().parse().ok()).collect()
                        };
                        let path: Vec<(f32, f32, f32)> = t.iter()
                            .find_map(|x| x.strip_prefix("path="))
                            .map(|v| v.split(';').filter_map(|p| {
                                let c = nums(p);
                                match c.len() { 3 => Some((c[0], c[1], c[2])),
                                                2 => Some((c[0], c[1], 0.0)), _ => None }
                            }).collect())
                            .unwrap_or_default();
                        if path.len() < 2 {
                            errs.push(format!("sweep needs a path of at least two points, as \
                                               path=0,0,0;4,8,0: {line:?}"));
                            continue;
                        }
                        let radii: Vec<f32> = t.iter().find_map(|x| x.strip_prefix("r="))
                            .map(|v| nums(v).into_iter().filter(|r| *r >= 0.0 && *r < 400.0).collect())
                            .unwrap_or_else(|| vec![2.0]);
                        if radii.is_empty() { errs.push(format!("sweep needs r=: {line:?}")); continue; }
                        Shape::Sweep {
                            path,
                            radii,
                            section: t.iter().find_map(|x| x.strip_prefix("section="))
                                .and_then(Section::parse).unwrap_or(Section::Circle),
                        }
                    }
                    "profile" | "revolve" | "lathe" | "silhouette" => {
                        // A PIECEWISE RADIUS IS ALSO A LIST. `r=at
                        // 0:2, 0.35:7, 1:1` says WHERE each value
                        // falls; sampling it makes the evenly spaced
                        // list every shape here already takes, so the
                        // higher rung costs the geometry nothing.
                        let radii: Vec<f32> = if let Some(p) =
                            crate::capability::parse_piecewise(&line.replace(' ', " ")) {
                            if line.contains("at ") || line.contains(':') { p.sample(11) }
                            else { Vec::new() }
                        } else { Vec::new() };
                        let radii: Vec<f32> = if !radii.is_empty() { radii } else {
                            t.iter()
                            .find_map(|x| x.strip_prefix("r=").or_else(|| x.strip_prefix("radii=")))
                            .map(|v| v.split(',').filter_map(|n| n.trim().parse().ok())
                                 .filter(|x: &f32| *x >= 0.0 && *x < 600.0).collect())
                            .unwrap_or_default() };
                        if radii.len() < 2 {
                            errs.push(format!("profile needs at least two radii, as r=2,6,3: {line:?}"));
                            continue;
                        }
                        Shape::Profile { radii, h: kv(&t, "h").unwrap_or(16.0).clamp(2.0, 512.0),
                                         square: t.contains(&"square") }
                    }
                    "capsule" | "limb" | "arm" | "leg" | "bone" => {
                        let r = kv(&t, "r").unwrap_or(1.5);
                        Shape::Capsule { r, r2: kv(&t, "r2").unwrap_or(r * 0.7),
                                         len: kv(&t, "len").or(kv(&t, "h")).unwrap_or(8.0),
                                         dir: kv(&t, "dir").unwrap_or(0.0),
                                         pitch: kv(&t, "pitch").unwrap_or(-90.0) }
                    }
                    "spine" | "neck" | "tail" | "curve" => {
                        let r = kv(&t, "r").unwrap_or(2.0);
                        Shape::Spine { r, r2: kv(&t, "r2").unwrap_or(r * 0.4),
                                       len: kv(&t, "len").or(kv(&t, "h")).unwrap_or(16.0),
                                       dir: kv(&t, "dir").unwrap_or(0.0),
                                       arch: kv(&t, "arch").unwrap_or(0.3),
                                       segments: kv(&t, "segments").unwrap_or(8.0).clamp(2.0, 64.0) as usize }
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
                    "at" | "offset" => Rel::Offset {
                        dx: kv(&t, "dx").unwrap_or(0.0),
                        dy: kv(&t, "dy").unwrap_or(0.0),
                        dz: kv(&t, "dz").unwrap_or(0.0) },
                    "reaches" | "reach" | "touches" | "down_to" => Rel::Reaches,
                    "joint" | "from" | "hangs" | "attach" => Rel::Joint {
                        at: kv(&t, "at").unwrap_or(0.5),        // 0 base, 1 top of the object
                        out: kv(&t, "out").unwrap_or(0.8),      // how far toward its edge
                        pair: t.contains(&"pair") || t.contains(&"both"),
                        contra: kv(&t, "contra").unwrap_or(0.0).clamp(-1.0, 1.0),
                    },
                    "pair" | "either_side" | "eitherside" | "both_sides" =>
                        Rel::PairAcross { by: kv(&t, "by").unwrap_or(0.0) },
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
            if n != "ground" && !names.contains(&n.as_str()) && !s.assume_known.contains(n) {
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
    resolve_with(s, anchor, &BTreeMap::new())
}

/// Resolve with parts from EARLIER passes available as relation
/// targets. They position things and place nothing.
pub fn resolve_with(s: &Sketch, anchor: (i32, i32, i32),
                    prior: &BTreeMap<String, (Shape, (i32, i32, i32))>) -> (Vec<Placed>, Vec<String>) {
    resolve_grounded(s, anchor, prior, None)
}

/// Resolve with a ground height available, so a part can be told to
/// REACH it rather than being given a length.
///
/// A dragon's leg was `len=34` -- a number guessed to look right, and
/// wrong the moment the body moved or the terrain changed. A leg's
/// length is not a design decision, it is the distance from the hip to
/// the floor. Same for a column to a ceiling or a pier to a riverbed.
pub fn resolve_grounded(s: &Sketch, anchor: (i32, i32, i32),
                        prior: &BTreeMap<String, (Shape, (i32, i32, i32))>,
                        ground_at: Option<&dyn Fn(i32, i32) -> Option<i32>>)
    -> (Vec<Placed>, Vec<String>) {
    let mut notes = Vec::new();
    let mut pos: BTreeMap<String, (f32, f32, f32)> = BTreeMap::new();
    let mut extra: BTreeMap<String, Vec<(f32, f32, f32)>> = BTreeMap::new();
    let mut spans: BTreeMap<String, ((f32, f32, f32), (f32, f32, f32))> = BTreeMap::new();
    let mut mirrored: BTreeSet<String> = BTreeSet::new();
    let mut leaned: BTreeSet<String> = BTreeSet::new();
    let by_name: BTreeMap<&str, &Part> = s.parts.iter().map(|p| (p.name.as_str(), p)).collect();
    // prior parts seed the position map, so a relation naming one resolves
    for (n, (_, at)) in prior {
        if !by_name.contains_key(n.as_str()) {
            pos.insert(n.clone(), (at.0 as f32, at.1 as f32, at.2 as f32));
        }
    }
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
            let bext = by_name.get(b.as_str()).map(|p| p.shape.extent())
                .or_else(|| prior.get(b.as_str()).map(|(sh, _)| sh.extent()))
                .unwrap_or([0.0; 3]);
            let aext = pa.shape.extent();
            let placed = match rel {
                Rel::Inside | Rel::Around | Rel::LevelWith => bp,
                Rel::Offset { dx, dy, dz } => (bp.0 + dx, bp.1 + dy, bp.2 + dz),
                // `reaches` sets a LENGTH, not a position: applied
                // after everything is placed, in a second sweep
                Rel::Reaches => continue,
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
                Rel::Joint { at, out, pair, contra } => {
                    // The joint: up the object by `at`, out toward its
                    // side by `out`. The limb's own reach is not part of
                    // the offset -- it swings from here. Since a shape
                    // is centred on its position, the limb's centre goes
                    // half its own reach along its heading FROM the
                    // joint, so its base lands on the joint.
                    let y = bp.1 - bext[1] + bext[1] * 2.0 * at.clamp(0.0, 1.0) + reach(&pa.shape).1 * 0.5;
                    let x = bext[0] * out.clamp(-2.0, 2.0);
                    if *pair {
                        // CONTRAPPOSTO. The second of the pair is
                        // raised and drawn in, and the pair is no
                        // longer a mirror: one hip rides high, one
                        // shoulder drops. A figure whose halves match
                        // exactly reads as a diagram of a figure.
                        let lift = contra * bext[1] * 0.18;
                        let draw = contra * bext[0] * 0.22;
                        extra.insert(a.clone(), vec![(bp.0 + x - draw, y + lift, bp.2 + draw * 0.5)]);
                        mirrored.insert(a.clone());
                        if contra.abs() > 0.01 { leaned.insert(a.clone()); }
                    }
                    (bp.0 - x, y, bp.2)
                }
                Rel::PairAcross { by } => {
                    // one each side, offset by the two half-widths plus
                    // the gap; the copy is the mirror
                    let off = bext[0] + aext[0] + by;
                    let left = (bp.0 - off, bp.1, bp.2);
                    extra.insert(a.clone(), vec![(bp.0 + off, bp.1, bp.2)]);
                    mirrored.insert(a.clone());
                    left
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
    // STRETCH TO REACH. Done after positions, because a part must know
    // where it ended up before it can know how far it has to go.
    let mut stretched: BTreeMap<String, Shape> = BTreeMap::new();
    let reach_list: Vec<String> = s.rels.iter()
        .filter(|(_, r, _)| matches!(r, Rel::Reaches)).map(|(a, _, _)| a.clone()).collect();
    for a in &reach_list {
        let rel = Rel::Reaches;
        let (a, rel, _) = (a, &rel, &String::new());
        let _ = rel;
        let Some(pa) = by_name.get(a.as_str()) else { continue };
        let Some(c) = pos.get(a) else { continue };
        let floor = match ground_at {
            Some(f) => f(c.0.round() as i32, c.2.round() as i32).map(|y| y as f32),
            None => Some(anchor.1 as f32),
        };
        // NO GROUND IS NOT A REASON TO FLOAT. Observed: a gorilla
        // statue built four blocks in the air, because the run had no
        // world under it, `reaches` found no floor and gave up -- and
        // giving up leaves the figure wherever the proportions happened
        // to put it. The anchor is where the player is standing and
        // pointing, so it is the floor of last resort. A statue on the
        // wrong ground is a statue; one hanging in the air is a bug.
        let floor = match floor {
            Some(f) => f,
            None => {
                notes.push(format!("'{a}' found no ground; standing it on the anchor instead"));
                anchor.1 as f32
            }
        };
        // The joint is the part's TOP -- it hangs from there. A capsule
        // is centred on its position, so lengthening it grows both ways
        // from a centre computed for the OLD length, and the foot ended
        // up seventeen blocks above the floor it was told to touch. The
        // centre moves too, keeping the top on the joint.
        let top = c.1 + reach(&pa.shape).1.abs() * 0.5;
        let want = (top - floor).max(1.0);
        stretched.insert(a.clone(), set_len(&pa.shape, want));
        let new_centre = top - want * 0.5;
        let drop = new_centre - c.1;
        pos.insert(a.clone(), (c.0, new_centre, c.2));
        if let Some(v) = extra.get_mut(a) { for e in v.iter_mut() { e.1 += drop; } }
        notes.push(format!("'{a}' reaches the ground: length {:.0}", want));
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
            shape: shrunk.get(&p.name).cloned()
                .or_else(|| stretched.get(&p.name).cloned())
                .unwrap_or_else(|| p.shape.clone()),
            material: p.material.clone(),
            hollow: p.hollow,
            at: (c.0.round() as i32, c.1.round() as i32, c.2.round() as i32),
            extra: extra.get(&p.name).map(|v| v.iter()
                .map(|c| (c.0.round() as i32, c.1.round() as i32, c.2.round() as i32)).collect())
                .unwrap_or_default(),
            mirror_extra: mirrored.contains(&p.name),
            lean_extra: leaned.contains(&p.name).then(|| {
                s.rels.iter().find_map(|(a, r, _)| match r {
                    Rel::Joint { contra, .. } if *a == p.name => Some(*contra), _ => None })
                    .unwrap_or(0.0)
            }),
            span: spans.get(&p.name).map(|(a, b)| (
                (a.0.round() as i32, a.1.round() as i32, a.2.round() as i32),
                (b.0.round() as i32, b.1.round() as i32, b.2.round() as i32))),
        }
    }).collect();
    (out, notes)
}

/// A shape's mirror image about the x axis: its heading reflected.
fn flip_x(s: &Shape) -> Shape {
    match s {
        Shape::Capsule { r, r2, len, dir, pitch } =>
            Shape::Capsule { r: *r, r2: *r2, len: *len, dir: 180.0 - dir, pitch: *pitch },
        Shape::Spine { r, r2, len, dir, arch, segments } =>
            Shape::Spine { r: *r, r2: *r2, len: *len, dir: 180.0 - dir, arch: *arch, segments: *segments },
        other => other.clone(),
    }
}

/// Where a shape's far end lies relative to its centre: the vector
/// from the middle to the tip. A limb attaches by its base, so the
/// solver moves it half this from the joint.
fn reach(s: &Shape) -> (f32, f32, f32) {
    match s {
        Shape::Capsule { len, dir, pitch, .. } => {
            let (a, p) = (dir.to_radians(), pitch.to_radians());
            (len * a.cos() * p.cos(), len * p.sin(), len * a.sin() * p.cos())
        }
        Shape::Spine { len, dir, .. } => {
            let a = dir.to_radians();
            (len * a.cos(), 0.0, len * a.sin())
        }
        _ => (0.0, 0.0, 0.0),
    }
}

/// A shape with a new length, keeping everything else.
fn set_len(s: &Shape, l: f32) -> Shape {
    match s {
        Shape::Profile { radii, square, .. } =>
            Shape::Profile { radii: radii.clone(), h: l, square: *square },
        Shape::Capsule { r, r2, dir, pitch, .. } =>
            Shape::Capsule { r: *r, r2: *r2, len: l, dir: *dir, pitch: *pitch },
        Shape::Spine { r, r2, dir, arch, segments, .. } =>
            Shape::Spine { r: *r, r2: *r2, len: l, dir: *dir, arch: *arch, segments: *segments },
        Shape::Cylinder { r, .. } => Shape::Cylinder { r: *r, h: l },
        other => other.clone(),
    }
}

/// A shape's heading nudged by `deg`, for the leaning half of a pair.
fn tilt(s: &Shape, deg: f32) -> Shape {
    match s {
        Shape::Capsule { r, r2, len, dir, pitch } =>
            Shape::Capsule { r: *r, r2: *r2, len: *len, dir: *dir, pitch: pitch + deg },
        Shape::Spine { r, r2, len, dir, arch, segments } =>
            Shape::Spine { r: *r, r2: *r2, len: *len, dir: dir + deg, arch: *arch, segments: *segments },
        other => other.clone(),
    }
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
        Shape::Sweep { path, radii, section } => Shape::Sweep {
            path: path.iter().map(|p| (p.0 * f, p.1 * f, p.2 * f)).collect(),
            radii: radii.iter().map(|r| r * f).collect(), section: *section },
        Shape::Profile { radii, h, square } =>
            Shape::Profile { radii: radii.iter().map(|r| r * f).collect(), h: h * f, square: *square },
        Shape::Capsule { r, r2, len, dir, pitch } =>
            Shape::Capsule { r: r * f, r2: r2 * f, len: len * f, dir: *dir, pitch: *pitch },
        Shape::Spine { r, r2, len, dir, arch, segments } =>
            Shape::Spine { r: r * f, r2: r2 * f, len: len * f, dir: *dir, arch: *arch, segments: *segments },
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
    out.extend(cells_at(p, p.at, block));
    for at in p.extra.iter() {
        if p.mirror_extra {
            // reflect the shape's own heading about the x axis, so a
            // pair faces outward from the body rather than both the
            // same way
            let mut m = p.clone();
            m.shape = flip_x(&p.shape);
            // and tilt its heading, so the copy is posed rather than
            // reflected: a raised hip carries an angled thigh with it
            if let Some(c) = p.lean_extra { m.shape = tilt(&m.shape, c * 12.0); }
            out.extend(cells_at(&m, *at, block));
        } else {
            out.extend(cells_at(p, *at, block));
        }
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
        Shape::Sweep { path, radii, section } => {
            // Walk the path, carrying the section. Radii are keyed to
            // the whole run and interpolated between, so three numbers
            // describe a smooth taper over a twelve-point path.
            let n = path.len();
            let total: f32 = path.windows(2)
                .map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)
                          + (w[1].2 - w[0].2).powi(2)).sqrt()).sum::<f32>().max(1.0);
            let steps = (total * 2.0).ceil() as i32;
            let radius_at = |t: f32| -> f32 {
                if radii.len() == 1 { return radii[0]; }
                let pos = t * (radii.len() - 1) as f32;
                let i = (pos.floor() as usize).min(radii.len() - 2);
                radii[i] + (radii[i + 1] - radii[i]) * (pos - i as f32)
            };
            for k in 0..=steps {
                let t = k as f32 / steps as f32;
                // where on the path
                let seg = t * (n - 1) as f32;
                let i = (seg.floor() as usize).min(n - 2);
                let f = seg - i as f32;
                let (a, b) = (path[i], path[i + 1]);
                let p = (at.0 + a.0 + (b.0 - a.0) * f,
                         at.1 + a.1 + (b.1 - a.1) * f,
                         at.2 + a.2 + (b.2 - a.2) * f);
                // the section lies ACROSS the path: a sweep whose
                // section stayed axis-aligned would flatten wherever
                // the path turned
                let (dx, dy, dz) = (b.0 - a.0, b.1 - a.1, b.2 - a.2);
                let len = (dx * dx + dy * dy + dz * dz).sqrt().max(0.001);
                let (ux, uy, uz) = (dx / len, dy / len, dz / len);
                // two vectors perpendicular to the heading
                let (rx, ry, rz) = if uy.abs() < 0.9 { (0.0, 1.0, 0.0) } else { (1.0, 0.0, 0.0) };
                let (e1x, e1y, e1z) = (uy * rz - uz * ry, uz * rx - ux * rz, ux * ry - uy * rx);
                let m1 = (e1x * e1x + e1y * e1y + e1z * e1z).sqrt().max(0.001);
                let (e1x, e1y, e1z) = (e1x / m1, e1y / m1, e1z / m1);
                let (e2x, e2y, e2z) = (uy * e1z - uz * e1y, uz * e1x - ux * e1z, ux * e1y - uy * e1x);
                let r = radius_at(t);
                if r < 0.4 { continue; }
                let ri = r.ceil() as i32;
                for a1 in -ri..=ri { for a2 in -ri..=ri {
                    if !section.holds(a1 as f32, a2 as f32, r) { continue; }
                    push(p.0 + e1x * a1 as f32 + e2x * a2 as f32,
                         p.1 + e1y * a1 as f32 + e2y * a2 as f32,
                         p.2 + e1z * a1 as f32 + e2z * a2 as f32);
                }}
            }
        }
        Shape::Profile { radii, h, square } => {
            // Radii are keyed to the profile's own height and
            // INTERPOLATED between, so five numbers describe a smooth
            // vase rather than five discs. A model gives the shape, not
            // the resolution.
            let hi = h.max(2.0);
            let n = radii.len().max(2);
            let base = at.1 - hi / 2.0;
            for k in 0..=(hi as i32) {
                let f = k as f32 / hi;
                let pos = f * (n - 1) as f32;
                let i = (pos.floor() as usize).min(n - 2);
                let t = pos - i as f32;
                let rr = radii[i] + (radii[i + 1] - radii[i]) * t;
                if rr < 0.4 { continue; }
                let ri = rr.ceil() as i32;
                for dx in -ri..=ri { for dz in -ri..=ri {
                    let inside = if *square { dx.abs() as f32 <= rr && dz.abs() as f32 <= rr }
                                 else { ((dx * dx + dz * dz) as f32).sqrt() <= rr };
                    if inside { push(at.0 + dx as f32, base + k as f32, at.2 + dz as f32); }
                }}
            }
        }
        Shape::Capsule { r, r2, len, dir, pitch } => {
            let a = dir.to_radians();
            let p = pitch.to_radians();
            let (dx, dy, dz) = (a.cos() * p.cos(), p.sin(), a.sin() * p.cos());
            // CENTRED on `at`, like every other shape. Drawing from the
            // origin outward while `extent` treated the origin as the
            // centre put every capsule half a length from where the
            // solver placed it -- which is why a statue's legs floated
            // detached below its body.
            let steps = (len.max(1.0) * 2.0) as i32;
            let (bx, by, bz) = (at.0 - dx * len * 0.5, at.1 - dy * len * 0.5, at.2 - dz * len * 0.5);
            for i in 0..=steps {
                let f = i as f32 / steps as f32;
                let rr = r + (r2 - r) * f;                     // the taper
                let (cx, cy, cz) = (bx + dx * len * f, by + dy * len * f, bz + dz * len * f);
                let ri = rr.ceil() as i32;
                for ox in -ri..=ri { for oy in -ri..=ri { for oz in -ri..=ri {
                    if ((ox * ox + oy * oy + oz * oz) as f32).sqrt() <= rr {
                        push(cx + ox as f32, cy + oy as f32, cz + oz as f32);
                    }
                }}}
            }
        }
        Shape::Spine { r, r2, len, dir, arch, segments } => {
            // A curve, not a line: each segment steps along the heading
            // and the whole run arches. A neck built as a straight
            // capsule is a pipe; the arch is the animal.
            let a = dir.to_radians();
            let (ux, uz) = (a.cos(), a.sin());
            let n = (*segments).max(2);
            let fine = n * 6;
            let (bx, bz) = (at.0 - ux * len * 0.5, at.2 - uz * len * 0.5);
            for i in 0..=fine {
                let f = i as f32 / fine as f32;
                let rr = r + (r2 - r) * f;
                let lift = arch * len * (f * std::f32::consts::PI).sin();
                let (cx, cy, cz) = (bx + ux * len * f, at.1 + lift - arch * len * 0.5,
                                    bz + uz * len * f);
                let ri = rr.ceil() as i32;
                for ox in -ri..=ri { for oy in -ri..=ri { for oz in -ri..=ri {
                    if ((ox * ox + oy * oy + oz * oz) as f32).sqrt() <= rr {
                        push(cx + ox as f32, cy + oy as f32, cz + oz as f32);
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

/// Fill the crevice where two parts meet, so a limb reads as GROWING
/// from a body rather than glued to it.
///
/// Ported in spirit from `cad-kernel`'s SDF `blend`, which is the
/// polynomial smooth minimum: `h = clamp(0.5 + 0.5*(db - da)/k, 0, 1)`
/// then `db + (da - db)*h - k*h*(1 - h)`.
///
/// That needs distance fields, and this crate resolves to cell sets.
/// The discrete equivalent of a smooth union is the same idea by
/// another route: a cell close to BOTH surfaces is inside the blend
/// even when it is inside neither shape. So the fillet is every empty
/// cell within `k` of both parts, which fills exactly the crevice the
/// smooth-min would have rounded.
///
/// This is why figures looked stiff and assembled. Every part was
/// hard-unioned, so an arm met a torso at a seam you could see, and no
/// amount of correct proportion or pose fixes a seam.
pub fn fillet(a: &[(i32, i32, i32)], b: &[(i32, i32, i32)], k: i32) -> Vec<(i32, i32, i32)> {
    let k = k.clamp(0, 6);
    if k == 0 || a.is_empty() || b.is_empty() { return Vec::new(); }
    let sa: BTreeSet<(i32, i32, i32)> = a.iter().copied().collect();
    let sb: BTreeSet<(i32, i32, i32)> = b.iter().copied().collect();
    // Only look near where the two actually come close: the whole of
    // both bounding boxes is most of the figure, and a fillet applied
    // everywhere is just a fatter figure.
    let bounds = |s: &BTreeSet<(i32, i32, i32)>| s.iter().fold(
        ((i32::MAX, i32::MAX, i32::MAX), (i32::MIN, i32::MIN, i32::MIN)),
        |(l, h), c| ((l.0.min(c.0), l.1.min(c.1), l.2.min(c.2)),
                     (h.0.max(c.0), h.1.max(c.1), h.2.max(c.2))));
    let (alo, ahi) = bounds(&sa);
    let (blo, bhi) = bounds(&sb);
    let lo = (alo.0.max(blo.0) - k, alo.1.max(blo.1) - k, alo.2.max(blo.2) - k);
    let hi = (ahi.0.min(bhi.0) + k, ahi.1.min(bhi.1) + k, ahi.2.min(bhi.2) + k);
    if lo.0 > hi.0 || lo.1 > hi.1 || lo.2 > hi.2 { return Vec::new(); }
    let near = |s: &BTreeSet<(i32, i32, i32)>, c: (i32, i32, i32)| {
        for dx in -k..=k { for dy in -k..=k { for dz in -k..=k {
            if dx * dx + dy * dy + dz * dz > k * k { continue; }
            if s.contains(&(c.0 + dx, c.1 + dy, c.2 + dz)) { return true; }
        }}}
        false
    };
    let mut out = Vec::new();
    for x in lo.0..=hi.0 { for y in lo.1..=hi.1 { for z in lo.2..=hi.2 {
        let c = (x, y, z);
        if sa.contains(&c) || sb.contains(&c) { continue; }
        if near(&sa, c) && near(&sb, c) { out.push(c); }
    }}}
    out
}

/// Which parts `to_ir` could not express, so the caller knows to
/// rasterise them.
pub fn needs_cells(placed: &[Placed]) -> Vec<&Placed> {
    placed.iter().filter(|p| matches!(p.shape,
        Shape::Torus { .. } | Shape::Cone { .. } | Shape::Helix { .. } | Shape::Link { .. }
        | Shape::Capsule { .. } | Shape::Spine { .. } | Shape::Profile { .. }
        | Shape::Sweep { .. })).collect()
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

    #[test]
    fn a_leg_reaches_the_ground_rather_than_guessing_a_length() {
        // A dragon's leg was `len=34`: a number guessed to look right,
        // and wrong the moment the body moved or the terrain changed.
        let src = "part body capsule r=6 r2=5 len=30 dir=0 pitch=0 material=stone\n\
                   part leg capsule r=3 r2=2 len=4 dir=0 pitch=-90 material=stone\n\
                   rel body above ground by=40\n\
                   rel leg joint body at=0.2 out=0.8 pair\n\
                   rel leg reaches ground\n";
        let (sk, errs) = parse(src);
        assert!(errs.is_empty(), "{errs:?}");
        // a flat floor at y=64
        let floor = |_x: i32, _z: i32| Some(64);
        let (placed, notes) = resolve_grounded(&sk, (0, 64, 0), &BTreeMap::new(), Some(&floor));
        assert!(notes.iter().any(|n| n.contains("'leg' reaches the ground")), "{notes:?}");
        let leg = placed.iter().find(|p| p.name == "leg").unwrap();
        // it grew from 4 to whatever the drop is
        match leg.shape { Shape::Capsule { len, .. } => assert!(len > 20.0, "stretched to {len}"),
                          ref o => panic!("{o:?}") }
        // and its foot lands ON the floor, not above or through it
        let cells = cells(leg, 1);
        let low = cells.iter().map(|c| c.1).min().unwrap();
        assert!((low - 64).abs() <= 3, "the foot is on the floor: {low}");
        // both of the pair, since `reaches` applies to the shape
        assert_eq!(leg.extra.len(), 1);
        let xs: BTreeSet<i32> = cells.iter().map(|c| c.0).collect();
        assert!(xs.iter().any(|x| *x < -2) && xs.iter().any(|x| *x > 2), "one each side");
    }

    #[test]
    fn reaching_for_a_floor_that_is_not_there_says_so() {
        let (sk, _) = parse("part leg capsule r=2 len=4 pitch=-90 material=stone\n\
                             rel leg reaches ground\n");
        let none = |_x: i32, _z: i32| None;
        let (placed, notes) = resolve_grounded(&sk, (0, 64, 0), &BTreeMap::new(), Some(&none));
        // NO GROUND IS NOT A REASON TO FLOAT. This used to give up,
        // and giving up left the figure wherever the proportions put it
        // -- a gorilla statue four blocks in the air. The anchor is
        // where the player is standing and pointing, so it is the floor
        // of last resort.
        assert!(notes.iter().any(|n| n.contains("standing it on the anchor")), "{notes:?}");
        let leg = placed.iter().find(|p| p.name == "leg").expect("a leg");
        let cells = cells(leg, 1);
        let lowest = cells.iter().map(|c| c.1).min().unwrap();
        assert!((lowest - 64).abs() <= 2, "it stands on the anchor, not above it: {lowest}");
    }

    #[test]
    fn a_profile_is_the_shape_that_covers_the_things_with_no_form() {
        // A breadth test over 43 ordinary requests found 27 with no
        // scaffolding at all. A mushroom, an hourglass, a chess piece,
        // a lantern, a pagoda: every one is a profile.
        let cases = [
            ("mushroom",  "part m profile r=1.5,1.2,1,7,5,0.5 h=20 material=stone"),
            ("hourglass", "part g profile r=7,4,1,1,4,7 h=24 material=glass"),
            ("chess",     "part p profile r=6,5,2.5,2,4,3,2.5,4 h=30 material=quartz"),
            ("vase",      "part v profile r=3,6,7,5,3,4 h=18 material=sandstone"),
            ("silo",      "part s profile r=8,8,8,8,7,4 h=40 material=stone square"),
        ];
        for (name, line) in cases {
            let (sk, errs) = parse(&format!("{line}\n"));
            assert!(errs.is_empty(), "{name}: {errs:?}");
            let (placed, _) = resolve(&sk, (0, 64, 0));
            let cells = cells(&placed[0], 1);
            assert!(!cells.is_empty(), "{name} built nothing");
            // it is the height asked for
            let h = cells.iter().map(|c| c.1).max().unwrap() - cells.iter().map(|c| c.1).min().unwrap();
            let want: i32 = line.split("h=").nth(1).unwrap().split_whitespace().next().unwrap().parse().unwrap();
            assert!((h - want).abs() <= 2, "{name}: {h} tall, asked {want}");
        }
        // AND THE SILHOUETTE IS THE ONE DESCRIBED. A mushroom is narrow
        // low down and wide at the top; an hourglass is the opposite of
        // itself in the middle.
        let width_at = |line: &str, frac: f32| -> i32 {
            let (sk, _) = parse(&format!("{line}\n"));
            let (placed, _) = resolve(&sk, (0, 64, 0));
            let c = cells(&placed[0], 1);
            let (lo, hi) = (c.iter().map(|x| x.1).min().unwrap(), c.iter().map(|x| x.1).max().unwrap());
            let y = lo + ((hi - lo) as f32 * frac) as i32;
            let at: Vec<i32> = c.iter().filter(|x| x.1 == y).map(|x| x.0).collect();
            if at.is_empty() { 0 } else { at.iter().max().unwrap() - at.iter().min().unwrap() }
        };
        let m = cases[0].1;
        assert!(width_at(m, 0.8) > width_at(m, 0.3) * 2, "a mushroom flares: {} vs {}",
                width_at(m, 0.8), width_at(m, 0.3));
        let g = cases[1].1;
        assert!(width_at(g, 0.5) < width_at(g, 0.1) / 2, "an hourglass pinches: {} vs {}",
                width_at(g, 0.5), width_at(g, 0.1));
        // square makes a tower, not a silo
        let round = cells(&resolve(&parse("part s profile r=8,8 h=10 material=stone\n").0,
                                  (0, 64, 0)).0[0], 1).len();
        let sq = cells(&resolve(&parse("part s profile r=8,8 h=10 material=stone square\n").0,
                                (0, 64, 0)).0[0], 1).len();
        assert!(sq > round, "a square profile fills its corners: {sq} vs {round}");
    }

    #[test]
    fn a_profile_needs_enough_radii_to_be_a_shape() {
        let (_, errs) = parse("part x profile h=10 material=stone\n");
        assert!(errs.iter().any(|e| e.contains("at least two radii")), "{errs:?}");
        let (_, e2) = parse("part x profile r=5 h=10 material=stone\n");
        assert!(e2.iter().any(|e| e.contains("at least two radii")), "{e2:?}");
        // and a zero radius is a gap, not an error: an hourglass may
        // pinch to nothing
        let (sk, e3) = parse("part x profile r=6,0,6 h=12 material=stone\n");
        assert!(e3.is_empty(), "{e3:?}");
        let (placed, _) = resolve(&sk, (0, 64, 0));
        assert!(!cells(&placed[0], 1).is_empty());
    }

    #[test]
    fn a_fillet_fills_the_crevice_where_two_parts_meet() {
        // Every part was hard-unioned, so an arm met a torso at a seam
        // you could see -- and no amount of correct proportion or pose
        // fixes a seam. This is `cad-kernel`'s SDF blend by another
        // route: a cell close to BOTH surfaces is inside the smooth
        // union even when it is inside neither shape.
        let (sk, _) = parse("part torso capsule r=5 r2=5 len=20 pitch=90 material=stone\n\
                             part arm capsule r=2 r2=2 len=12 dir=0 pitch=-30 material=stone\n\
                             rel torso above ground by=10\n\
                             rel arm joint torso at=0.8 out=1.0\n");
        let (placed, _) = resolve(&sk, (0, 64, 0));
        let t: Vec<(i32, i32, i32)> = cells(&placed[0], 1).into_iter().map(|c| (c.0, c.1, c.2)).collect();
        let a: Vec<(i32, i32, i32)> = cells(&placed[1], 1).into_iter().map(|c| (c.0, c.1, c.2)).collect();
        let f = fillet(&t, &a, 3);
        assert!(!f.is_empty(), "there is a crevice to fill");
        // it fills BETWEEN them, not inside either
        let ts: BTreeSet<_> = t.iter().copied().collect();
        let as_: BTreeSet<_> = a.iter().copied().collect();
        for c in &f {
            assert!(!ts.contains(c) && !as_.contains(c), "{c:?} was already solid");
        }
        // and it is a modest amount: a fillet is a smoothing, not a
        // second body
        assert!(f.len() < t.len() / 2, "{} fillet cells against {} torso", f.len(), t.len());
        // a bigger k fills more
        assert!(fillet(&t, &a, 4).len() > f.len());
        assert!(fillet(&t, &a, 0).is_empty(), "k=0 is a hard union, as before");
    }

    #[test]
    fn parts_that_are_nowhere_near_each_other_get_no_fillet() {
        // a fillet applied everywhere is just a fatter figure
        let far_a = vec![(0, 64, 0), (0, 65, 0)];
        let far_b = vec![(60, 64, 0), (60, 65, 0)];
        assert!(fillet(&far_a, &far_b, 4).is_empty());
        // and empty input is not a crash
        assert!(fillet(&[], &far_b, 3).is_empty());
    }

    #[test]
    fn contrapposto_breaks_the_mirror_that_made_every_figure_stiff() {
        // The reference critique's first complaint about its own statue
        // was "the pose is too symmetrical". `pair` mirrored EXACTLY,
        // so every figure this crate made scored a perfect zero on the
        // asymmetry measure and nothing noticed.
        let src = |c: &str| format!(
            "part torso capsule r=5 r2=4 len=20 pitch=90 material=stone\n\
             part leg capsule r=2.5 r2=2 len=22 dir=190 pitch=-84 material=stone\n\
             rel torso above ground by=22\n\
             rel leg joint torso at=0.05 out=0.55 pair{c}\n\
             rel leg reaches ground\n");
        let cells_of = |c: &str| {
            let (sk, errs) = parse(&src(c));
            assert!(errs.is_empty(), "{errs:?}");
            let (placed, _) = resolve(&sk, (0, 64, 0));
            let mut set = BTreeSet::new();
            for p in &placed { for x in cells(p, 1) { set.insert((x.0, x.1, x.2)); } }
            set
        };
        let mirror = cells_of("");
        let posed = cells_of(" contra=0.6");
        let asym = |s: &BTreeSet<(i32, i32, i32)>| crate::bench::measure(s).asymmetry;
        assert!(asym(&mirror) < 0.02, "a plain pair is an exact mirror: {}", asym(&mirror));
        assert!(asym(&posed) > 0.05, "contrapposto is not: {}", asym(&posed));
        // and the bench agrees it is an improvement
        let a = crate::bench::Attempt { label: "mirror".into(), source: String::new(),
                                        cells: mirror, notes: vec![] };
        let b = crate::bench::Attempt { label: "posed".into(), source: String::new(),
                                        cells: posed, notes: vec![] };
        assert_eq!(crate::bench::compare(&a, &b).better, Some(true),
                   "{}", crate::bench::compare(&a, &b).summary());
    }

    #[test]
    fn contrapposto_of_zero_is_exactly_what_a_pair_always_was() {
        // nothing that did not ask for a lean changes
        let base = "part t capsule r=4 r2=4 len=16 pitch=90 material=stone\n\
                    part arm capsule r=2 r2=1.5 len=10 dir=200 pitch=-50 material=stone\n\
                    rel t above ground by=10\n\
                    rel arm joint t at=0.8 out=1.0 pair";
        let of = |s: &str| {
            let (sk, _) = parse(&format!("{s}\n"));
            let (placed, _) = resolve(&sk, (0, 64, 0));
            placed.iter().flat_map(|p| cells(p, 1)).collect::<Vec<_>>()
        };
        assert_eq!(of(base), of(&format!("{base} contra=0")));
        assert_ne!(of(base), of(&format!("{base} contra=0.5")));
    }

    #[test]
    fn a_sweep_is_the_shape_the_others_are_special_cases_of() {
        // `profile` revolves around a FIXED VERTICAL AXIS, so it makes
        // a vase, a tower, and nothing that bends
        let cases = [
            ("a bending horn", "part h sweep path=0,0,0;2,7,1;7,12,4;14,13,9 r=3,2,1,0.5"),
            ("a corridor turning a corner", "part c sweep path=0,0,0;20,0,0;20,0,18 r=3 section=square"),
            ("a hook", "part k sweep path=0,0,0;0,12,0;3,16,0;6,13,0 r=1.5"),
            ("a pipe", "part p sweep path=0,0,0;0,14,0 r=4 section=hollow"),
            ("a straight taper, which is what profile does",
             "part t sweep path=0,0,0;0,20,0 r=6,4,2"),
        ];
        for (what, line) in cases {
            let (sk, errs) = parse(&format!("{line}\n"));
            assert!(errs.is_empty(), "{what}: {errs:?}");
            let (placed, _) = resolve(&sk, (0, 64, 0));
            let c = cells(&placed[0], 1);
            assert!(!c.is_empty(), "{what} built nothing");
            assert!(c.len() > 40, "{what}: only {} cells", c.len());
        }
        // A BENT PATH REALLY BENDS: the far end is nowhere near the
        // near end in more than one axis, which no profile can do
        let (sk, _) = parse("part h sweep path=0,0,0;2,7,1;7,12,4;14,13,9 r=3,2,1,0.5\n");
        let (placed, _) = resolve(&sk, (0, 64, 0));
        let c = cells(&placed[0], 1);
        let spread = |g: fn(&(i32, i32, i32, u16)) -> i32| {
            c.iter().map(g).max().unwrap() - c.iter().map(g).min().unwrap() };
        assert!(spread(|p| p.0) > 8 && spread(|p| p.1) > 8 && spread(|p| p.2) > 6,
                "it runs in all three axes: {} {} {}",
                spread(|p| p.0), spread(|p| p.1), spread(|p| p.2));
        // and it TAPERS: the far end is thinner than the near end
        // measured against the shape's OWN extent: a shape is centred
        // on its position, so absolute y values are not where a guess
        // puts them
        let (ylo, yhi) = (c.iter().map(|p| p.1).min().unwrap(), c.iter().map(|p| p.1).max().unwrap());
        let (q1, q3) = (ylo + (yhi - ylo) / 4, yhi - (yhi - ylo) / 4);
        let near: Vec<&(i32, i32, i32, u16)> = c.iter().filter(|p| p.1 < q1).collect();
        let far: Vec<&(i32, i32, i32, u16)> = c.iter().filter(|p| p.1 > q3).collect();
        assert!(!near.is_empty() && !far.is_empty(), "{ylo}..{yhi}");
        // COUNT, not extent. The path itself moves in z as it rises,
        // so a z-extent measures where the path went and not how thick
        // the section is. Cells per band is proportional to section
        // area, which is what tapering means.
        assert!(near.len() > far.len() * 2,
                "thick to thin: {} cells near, {} far", near.len(), far.len());
    }

    #[test]
    fn the_section_lies_across_the_path_and_not_along_an_axis() {
        // a sweep whose section stayed axis-aligned would FLATTEN
        // wherever the path turned, which is the whole difficulty
        let (sk, _) = parse("part d sweep path=0,0,0;14,14,0 r=4 section=circle\n");
        let (placed, _) = resolve(&sk, (0, 64, 0));
        let c = cells(&placed[0], 1);
        // measure the run's thickness ACROSS its diagonal heading: it
        // should be about 2r everywhere, not collapsed
        let mid: Vec<&(i32, i32, i32, u16)> = c.iter().filter(|p| (p.0 - 7).abs() < 2).collect();
        assert!(!mid.is_empty());
        let thick = mid.iter().map(|p| p.2).max().unwrap() - mid.iter().map(|p| p.2).min().unwrap();
        assert!(thick >= 6, "the section stayed round on a diagonal: {thick}");
    }

    #[test]
    fn every_section_makes_a_different_solid() {
        let of = |sec: &str| {
            let (sk, errs) = parse(&format!("part s sweep path=0,0,0;0,16,0 r=5 section={sec}\n"));
            assert!(errs.is_empty(), "{sec}: {errs:?}");
            let (placed, _) = resolve(&sk, (0, 64, 0));
            cells(&placed[0], 1).len()
        };
        let (circle, square, diamond, cross, hollow) =
            (of("circle"), of("square"), of("diamond"), of("cross"), of("hollow"));
        assert!(square > circle, "a square fills its corners: {square} vs {circle}");
        assert!(diamond < circle, "a diamond is inside the circle: {diamond} vs {circle}");
        // A CROSS IS NOT SMALLER THAN A DIAMOND, which I asserted
        // without doing the arithmetic: two arms of 0.8r by 2r overlap
        // in the middle, about 2.6r squared, where a diamond is 2r
        // squared. It is smaller than the SQUARE, which is the true
        // statement.
        assert!(cross < square, "a cross is mostly gap next to a square: {cross} vs {square}");
        assert!(hollow < circle && hollow > 0, "a tube is a rind: {hollow}");
    }

    #[test]
    fn a_sweep_needs_a_path_and_says_so() {
        let (_, e) = parse("part x sweep r=3\n");
        assert!(e.iter().any(|m| m.contains("at least two points")), "{e:?}");
        let (_, e2) = parse("part x sweep path=0,0,0 r=3\n");
        assert!(e2.iter().any(|m| m.contains("at least two points")), "{e2:?}");
        // a two-dimensional point is read as flat, not refused
        let (sk, e3) = parse("part x sweep path=0,0;0,12 r=2\n");
        assert!(e3.is_empty(), "{e3:?}");
        let (placed, _) = resolve(&sk, (0, 64, 0));
        assert!(!cells(&placed[0], 1).is_empty());
    }

    #[test]
    fn a_profile_takes_a_piecewise_radius_and_the_geometry_does_not_notice() {
        // saying WHERE each value falls is the rung above a bare list,
        // and sampling it makes the evenly spaced list every shape here
        // already takes
        let (sk, errs) = parse("part v profile r=at 0:2, 0.35:8, 1:1 h=24 material=stone\n");
        assert!(errs.is_empty(), "{errs:?}");
        let (placed, _) = resolve(&sk, (0, 64, 0));
        let c = cells(&placed[0], 1);
        assert!(!c.is_empty());
        // THE SWELLING IS WHERE IT WAS ASKED FOR: about a third up, not
        // halfway, which an evenly spaced list of three could not say
        let (lo, hi) = (c.iter().map(|p| p.1).min().unwrap(), c.iter().map(|p| p.1).max().unwrap());
        let width_at = |f: f32| {
            let y = lo + ((hi - lo) as f32 * f) as i32;
            let at: Vec<i32> = c.iter().filter(|p| p.1 == y).map(|p| p.0).collect();
            if at.is_empty() { 0 } else { at.iter().max().unwrap() - at.iter().min().unwrap() }
        };
        assert!(width_at(0.35) > width_at(0.7), "widest low down: {} vs {}",
                width_at(0.35), width_at(0.7));
        assert!(width_at(0.35) > width_at(0.05), "and not at the very base");
        // a plain list still works exactly as before
        let (plain, e2) = parse("part v profile r=2,8,1 h=24 material=stone\n");
        assert!(e2.is_empty(), "{e2:?}");
        assert!(!cells(&resolve(&plain, (0, 64, 0)).0[0], 1).is_empty());
    }
}
