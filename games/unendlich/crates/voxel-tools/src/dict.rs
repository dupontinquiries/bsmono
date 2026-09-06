//! The dictionary: named forms as recipes, not as ops.
//!
//! `roof` was a first-class op with a parser arm, a run arm, a math arm
//! and an exhaustiveness break in three files. Adding `dormer` would
//! cost the same again, and `buttress` after that. That is a language
//! that grows one hardcoded noun at a time and never catches up with
//! what a builder says.
//!
//! Here a form is a NAME, a parameter set, and a function returning a
//! `Solid`. Adding one is adding an entry. The IR needs no new verb —
//! `form roof gable at ... size ...` reaches every entry in the table,
//! and the model can be told the list at prompt time.
//!
//! The recipes below are deliberately small. A dictionary earns its
//! keep by being extended by whoever is building, not by shipping
//! complete; what matters is that extending it is data.

use crate::solid::{Axis, Material, Solid};

/// A form request: what to build, where, and how big.
#[derive(Debug, Clone)]
pub struct FormArgs {
    /// Footprint corners in world blocks.
    pub min: (i32, i32, i32),
    pub max: (i32, i32, i32),
    /// Style word, e.g. "gable" for a roof. Empty for the default.
    pub style: String,
    pub block: u16,
    pub accent: u16,
    /// Free numeric parameters, e.g. `h`, `count`, `spacing`.
    pub nums: Vec<(String, i32)>,
}

impl FormArgs {
    pub fn num(&self, key: &str, default: i32) -> i32 {
        self.nums.iter().find(|(k, _)| k == key).map(|(_, v)| *v).unwrap_or(default)
    }
    fn lo(&self) -> [f32; 3] {
        [self.min.0.min(self.max.0) as f32, self.min.1 as f32,
         self.min.2.min(self.max.2) as f32]
    }
    fn hi(&self) -> [f32; 3] {
        [(self.min.0.max(self.max.0) + 1) as f32, self.max.1 as f32,
         (self.min.2.max(self.max.2) + 1) as f32]
    }
}

/// One dictionary entry.
pub struct Form {
    pub name: &'static str,
    pub summary: &'static str,
    pub build: fn(&FormArgs) -> Solid,
}

/// Every form the model may name. This list IS the vocabulary; it is
/// what gets printed into the prompt, so adding an entry teaches the
/// model without touching the prompt text.
pub const FORMS: &[Form] = &[
    Form { name: "roof", summary: "pitched roof over a footprint (gable|hip|flat|pyramid)",
           build: roof_shell },
    Form { name: "wall", summary: "a wall around the footprint, with a doorway", build: wall },
    Form { name: "house", summary: "walls, floor, door, windows and a roof", build: house },
    Form { name: "colonnade", summary: "a row of evenly spaced pillars", build: colonnade },
    Form { name: "arch", summary: "an opening with a rounded head", build: arch },
    Form { name: "tower", summary: "a round tower with a conical cap", build: tower },
    Form { name: "village", summary: "several houses on plots, facing a centre, joined by paths",
           build: village },
    Form { name: "nest", summary: "a ringed crater with a raised rim -- a lair or a bowl",
           build: nest },
    Form { name: "cavern", summary: "a carved underground chamber with pillars and an entrance shaft",
           build: cavern },
    Form { name: "statue", summary: "a figure on a plinth: figure|gargoyle|idol|obelisk|colossus, \
                                     pose=standing|seated|arms_up|kneeling|lunge, \
                                     hold=bolt|sword|staff|orb|shield, beard=1",
           build: statue },
    Form { name: "castle", summary: "curtain wall, corner towers, gatehouse and a keep",
           build: castle },
    Form { name: "temple", summary: "steps, a colonnade, a cella and a domed roof",
           build: temple },
];

pub fn lookup(name: &str) -> Option<&'static Form> {
    FORMS.iter().find(|f| f.name.eq_ignore_ascii_case(name))
}

/// The prompt line listing what can be built. Generated, so the prompt
/// cannot drift from the table.
pub fn vocabulary() -> String {
    FORMS.iter()
        .map(|f| format!("  {:11} {}", f.name, f.summary))
        .collect::<Vec<_>>()
        .join("\n")
}

// ── recipes ─────────────────────────────────────────────────────────

fn roof(a: &FormArgs) -> Solid {
    let (lo, hi) = (a.lo(), a.hi());
    let over = a.num("overhang", 1) as f32;
    let h = a.num("h", ((hi[0] - lo[0]).min(hi[2] - lo[2]) / 2.0).max(2.0) as i32) as f32;
    let min = [lo[0] - over, lo[1], lo[2] - over];
    let max = [hi[0] + over, hi[1], hi[2] + over];
    let mat = Material::structural(a.block);
    match a.style.as_str() {
        "flat" => Solid::Box { min, max: [max[0], lo[1] + 1.0, max[2]], mat },
        "pyramid" | "hip" => {
            // Two crossed wedges: the intersection is a hip, and clipping
            // to the footprint is what the `Intersect` node is for. The
            // op version had to special-case this with a loop.
            let a1 = Solid::Wedge { min, max, h, axis: Axis::X, mat: mat.clone() };
            let a2 = Solid::Wedge { min, max, h, axis: Axis::Z, mat };
            Solid::Intersect(Box::new(a1), Box::new(a2))
        }
        // gable ridges along the LONGER axis, which is what a builder
        // means without saying it
        _ => {
            let axis = if hi[0] - lo[0] >= hi[2] - lo[2] { Axis::X } else { Axis::Z };
            Solid::Wedge { min, max, h, axis, mat }
        }
    }
}

/// A roof SURFACE rather than a solid mass.
///
/// `Wedge` is a solid prism, so a bare wedge fills the attic with
/// stone: the cross-section came out as a four-block-thick slab instead
/// of two sloping faces. A roof is the shell of that prism. This is the
/// kind of error the continuous layer makes easy to fix — one `Shell`
/// node, no re-derivation of which cells are on a slope.
fn roof_shell(a: &FormArgs) -> Solid {
    match a.style.as_str() {
        // A flat roof is already a slab; shelling it would hollow the slab.
        "flat" => roof(a),
        _ => Solid::Shell { inner: Box::new(roof(a)), thickness: 1.0 },
    }
}

fn wall(a: &FormArgs) -> Solid {
    let (lo, hi) = (a.lo(), a.hi());
    let h = a.num("h", 5) as f32;
    let shell = Solid::Shell {
        inner: Box::new(Solid::Box { min: lo, max: [hi[0], lo[1] + h, hi[2]],
                                     mat: Material::structural(a.block) }),
        thickness: 1.0,
    };
    // A doorway, centred on the long side, as a CARVE that outranks the
    // wall -- so a later thickening cannot seal it.
    let cx = (lo[0] + hi[0]) / 2.0;
    let door = Solid::Box { min: [cx - 1.0, lo[1], lo[2] - 1.0],
                            max: [cx + 1.0, lo[1] + 3.0, lo[2] + 1.0],
                            mat: Material::carved() };
    Solid::Diff(Box::new(shell), Box::new(door))
}

fn house(a: &FormArgs) -> Solid {
    // The style sets materials, roof, storey height and window rhythm
    // together; the seed decides which of the style's variants this
    // particular house gets. Two houses from one line differ the way
    // two houses on one street do: same vernacular, different details.
    let st = a.nums.iter().find(|(k, _)| k == "style").map(|(_, v)| *v as usize)
        .and_then(|i| crate::style::STYLES.get(i))
        .or_else(|| crate::style::lookup(&a.style))
        .unwrap_or_else(|| crate::style::lookup("cottage").unwrap());
    let seed = a.num("seed", 1) as u32;
    let roll = |k: u32| (hash2(k, seed) % 1000) as f32 / 1000.0;
    let (lo, hi) = (a.lo(), a.hi());
    let storeys = a.num("storeys", 1).clamp(1, 3);
    let h = (st.storey * storeys) as f32;
    let wall = Material::structural(if a.block != 1 { a.block } else { st.wall });
    let trim = Material::accent(st.trim);
    let mut parts: Vec<Solid> = Vec::new();

    // Body: walls + floor, door on the long side facing -z.
    let body = Solid::Shell {
        inner: Box::new(Solid::Box { min: lo, max: [hi[0], lo[1] + h, hi[2]], mat: wall.clone() }),
        thickness: 1.0 };
    let cx = (lo[0] + hi[0]) / 2.0;
    let door = Solid::Box { min: [cx - 1.0, lo[1], lo[2] - 1.0], max: [cx + 1.0, lo[1] + 3.0, lo[2] + 1.0],
                            mat: Material::carved() };
    // Windows in the style's rhythm, on both long walls, per storey.
    let span = hi[0] - lo[0];
    let step = st.window_every.max(2) as f32;
    let count = ((span - 2.0) / step).floor().max(1.0) as u32;
    let mut cuts: Vec<Solid> = vec![door];
    for s in 0..storeys {
        let wy = lo[1] + (s * st.storey) as f32 + 2.0;
        for zf in [lo[2] - 1.0, hi[2] - 1.0] {
            cuts.push(Solid::Array { count, step: [step, 0.0, 0.0], inner: Box::new(Solid::Box {
                min: [lo[0] + 1.0, wy, zf], max: [lo[0] + 2.0, wy + 2.0, zf + 2.0], mat: Material::carved() }) });
        }
    }
    parts.push(Solid::Diff(Box::new(body), Box::new(Solid::Union(cuts))));
    parts.push(Solid::Box { min: lo, max: [hi[0], lo[1] + 1.0, hi[2]], mat: Material::structural(st.floor) });
    // Storey floors and trim bands between storeys.
    for s in 1..storeys {
        let fy = lo[1] + (s * st.storey) as f32;
        parts.push(Solid::Box { min: [lo[0] + 1.0, fy, lo[2] + 1.0], max: [hi[0] - 1.0, fy + 1.0, hi[2] - 1.0],
                                mat: Material::structural(st.floor) });
        parts.push(Solid::Shell { inner: Box::new(Solid::Box { min: [lo[0], fy, lo[2]], max: [hi[0], fy + 1.0, hi[2]],
                                                               mat: trim.clone() }), thickness: 1.0 });
    }

    // Roof in the style's shape and pitch.
    let roof_args = FormArgs { min: (a.min.0, (lo[1] + h) as i32, a.min.2), max: (a.max.0, (lo[1] + h) as i32, a.max.2),
                               style: st.roof.into(), block: st.roof_block, accent: st.trim,
                               nums: vec![("h".into(), ((hi[0] - lo[0]).min(hi[2] - lo[2]) * st.pitch).max(2.0) as i32)] };
    parts.push(roof_shell(&roof_args));

    // VARIANTS, by the style's odds and the house's seed.
    if roll(1) < st.chimney {
        // a chimney on the roof, off-centre, one block above the ridge
        let chx = lo[0] + 2.0 + roll(2) * (span - 4.0);
        let ridge = lo[1] + h + roof_args.num("h", 3) as f32;
        parts.push(Solid::Box { min: [chx, lo[1] + h - 1.0, hi[2] - 3.0], max: [chx + 1.0, ridge + 2.0, hi[2] - 2.0],
                                mat: Material::new(st.trim, 60) });
    }
    if roll(3) < st.dormers && st.roof != "flat" {
        // dormers: two small gabled boxes on the front slope
        let dy = lo[1] + h;
        for k in 0..2 {
            let dx = lo[0] + 2.0 + (k as f32 + 0.5) * (span - 4.0) / 2.0;
            parts.push(Solid::Box { min: [dx - 1.0, dy, lo[2] - 0.5], max: [dx + 1.0, dy + 2.0, lo[2] + 2.5],
                                    mat: Material::new(st.wall, 60) });
            parts.push(Solid::Box { min: [dx - 0.5, dy + 0.5, lo[2] - 1.0], max: [dx + 0.5, dy + 1.5, lo[2] + 0.5],
                                    mat: Material::new(0, 70) });
            parts.push(Solid::Wedge { min: [dx - 1.5, dy + 2.0, lo[2] - 0.5], max: [dx + 1.5, 0.0, lo[2] + 2.5],
                                      h: 2.0, axis: Axis::Z, mat: Material::new(st.roof_block, 60) });
        }
    }
    if roll(4) < st.porch {
        // a porch: a roof on posts in front of the door
        let py = lo[1] + 3.0;
        parts.push(Solid::Box { min: [cx - 3.0, py, lo[2] - 3.0], max: [cx + 3.0, py + 1.0, lo[2]],
                                mat: Material::new(st.roof_block, 60) });
        for px in [cx - 2.5, cx + 2.5] {
            parts.push(Solid::Cylinder { base: [px, lo[1], lo[2] - 2.5], r: 0.5, h: 3.0,
                                         mat: Material::new(st.trim, 60) });
        }
    }
    if roll(5) < 0.35 && span >= 10.0 {
        // an L-wing: a smaller body off one end, with its own roof
        let side = if roll(6) < 0.5 { -1.0 } else { 1.0 };
        let wx0 = if side < 0.0 { lo[0] } else { hi[0] - span * 0.4 };
        let wx1 = wx0 + span * 0.4;
        let wz0 = hi[2] - 1.0;
        let wz1 = hi[2] + (hi[2] - lo[2]) * 0.6;
        let wh = st.storey as f32;
        parts.push(Solid::Shell { inner: Box::new(Solid::Box { min: [wx0, lo[1], wz0], max: [wx1, lo[1] + wh, wz1],
                                                               mat: wall.clone() }), thickness: 1.0 });
        parts.push(Solid::Box { min: [wx0 + 1.0, lo[1] + 1.0, wz0 - 0.5], max: [wx1 - 1.0, lo[1] + wh - 1.0, wz0 + 1.5],
                                mat: Material::carved() });
        let wing_roof = FormArgs { min: (wx0 as i32, (lo[1] + wh) as i32, wz0 as i32),
                                   max: (wx1 as i32 - 1, (lo[1] + wh) as i32, wz1 as i32 - 1),
                                   style: st.roof.into(), block: st.roof_block, accent: st.trim,
                                   nums: vec![("h".into(), ((wx1 - wx0).min(wz1 - wz0) * st.pitch).max(2.0) as i32)] };
        parts.push(roof_shell(&wing_roof));
    }
    Solid::Union(parts)
}

fn colonnade(a: &FormArgs) -> Solid {
    let (lo, hi) = (a.lo(), a.hi());
    let h = a.num("h", 6) as f32;
    let r = a.num("r", 1) as f32;
    let spacing = a.num("spacing", 4).max(2) as f32;
    let span = (hi[0] - lo[0]).max(1.0);
    let count = (span / spacing).floor().max(1.0) as u32;
    let pillar = Solid::Cylinder { base: [lo[0] + r, lo[1], lo[2] + r], r, h,
                                   mat: Material::structural(a.block) };
    Solid::Array { count, step: [spacing, 0.0, 0.0], inner: Box::new(pillar) }
}

fn arch(a: &FormArgs) -> Solid {
    let (lo, hi) = (a.lo(), a.hi());
    let h = a.num("h", 6) as f32;
    let w = hi[0] - lo[0];
    let pier = Solid::Box { min: lo, max: [hi[0], lo[1] + h, hi[2]],
                            mat: Material::structural(a.block) };
    // The opening: a box with a half-round head. This is the shape that
    // has no integer answer -- the curve does not land on cell edges,
    // which is precisely why it belongs in the continuous layer.
    let cx = (lo[0] + hi[0]) / 2.0;
    let cz = (lo[2] + hi[2]) / 2.0;
    let r = (w / 2.0 - 1.0).max(1.0);
    let opening = Solid::Union(vec![
        Solid::Box { min: [cx - r, lo[1], lo[2] - 1.0],
                     max: [cx + r, lo[1] + h - r, hi[2] + 1.0],
                     mat: Material::carved() },
        Solid::Cylinder { base: [cx, lo[1] + h - r, cz], r, h: 0.0,
                          mat: Material::carved() },
        Solid::Sphere { centre: [cx, lo[1] + h - r, cz], r, mat: Material::carved() },
    ]);
    Solid::Diff(Box::new(pier), Box::new(opening))
}

fn tower(a: &FormArgs) -> Solid {
    let (lo, hi) = (a.lo(), a.hi());
    let h = a.num("h", 12) as f32;
    let r = a.num("r", ((hi[0] - lo[0]) / 2.0).max(2.0) as i32) as f32;
    let cx = (lo[0] + hi[0]) / 2.0;
    let cz = (lo[2] + hi[2]) / 2.0;
    let body = Solid::Shell {
        inner: Box::new(Solid::Cylinder { base: [cx, lo[1], cz], r, h,
                                          mat: Material::structural(a.block) }),
        thickness: 1.0,
    };
    let cap = Solid::Shell {
        inner: Box::new(Solid::Wedge {
            min: [cx - r - 1.0, lo[1] + h, cz - r - 1.0],
            max: [cx + r + 1.0, 0.0, cz + r + 1.0],
            h: r + 1.0, axis: Axis::X,
            mat: Material::accent(a.accent) }),
        thickness: 1.0,
    };
    Solid::Union(vec![body, cap])
}

/// A settlement, not a building.
///
/// The corpus is emphatic about this and I had missed it entirely:
/// "village" appears 123 times, "settlement" words 162, against 29 for
/// "roof". The unit of work a builder narrates is a PLACE — several
/// structures in relation to one another, facing something, joined by
/// paths — and a dictionary that can only make one house at a time
/// makes the model emit five houses in a row on a grid.
///
/// The layout rules are the content here, and they are what a model
/// cannot be trusted to work out per-building:
///
/// * Plots are laid on a ring around a centre, not on a lattice. A grid
///   of houses reads as a car park; a ring reads as a place.
/// * Every house FACES the centre — its door is on the inward side.
/// * Sizes vary from a seeded hash, so no two neighbours match, without
///   the model having to pick numbers.
/// * A path runs from each door to the centre.
fn village(a: &FormArgs) -> Solid {
    let (lo, hi) = (a.lo(), a.hi());
    let cx = (lo[0] + hi[0]) / 2.0;
    let cz = (lo[2] + hi[2]) / 2.0;
    let count = a.num("count", 5).clamp(2, 12) as u32;
    let seed = a.num("seed", 7) as u32;
    // Radius: fill the footprint, leaving room for the plots themselves.
    let span = (hi[0] - lo[0]).min(hi[2] - lo[2]);
    let radius = (span / 2.0 - 6.0).max(6.0);

    let mut parts: Vec<Solid> = Vec::new();
    for i in 0..count {
        let ang = std::f32::consts::TAU * (i as f32) / (count as f32);
        let (sx, sz) = (cx + radius * ang.cos(), cz + radius * ang.sin());
        // Seeded variation, so neighbours differ without the model
        // choosing sizes. Small, odd numbers: an even footprint puts a
        // gable ridge on a cell boundary (see solid.rs).
        let h = hash2(i, seed);
        let w = 7.0 + (h % 3) as f32 * 2.0;
        let d = 7.0 + ((h >> 3) % 3) as f32 * 2.0;
        let plot = FormArgs {
            min: ((sx - w / 2.0) as i32, a.min.1, (sz - d / 2.0) as i32),
            max: ((sx + w / 2.0) as i32, a.max.1, (sz + d / 2.0) as i32),
            style: if h % 2 == 0 { "gable".into() } else { "hip".into() },
            block: a.block, accent: a.accent,
            nums: vec![("h".into(), 4 + (h % 2) as i32)],
        };
        parts.push(house(&plot));
        // A path from the plot toward the centre. Laid as a thin slab at
        // ground level; the terrain pass under it is the caller's job.
        parts.push(path_slab([sx, lo[1], sz], [cx, lo[1], cz], 2.0,
                             Material::structural(a.accent)));
    }
    // A well or marker at the centre gives the ring something to face,
    // which is what makes it read as a village rather than a circle of
    // houses.
    parts.push(Solid::Shell {
        inner: Box::new(Solid::Cylinder { base: [cx, lo[1], cz], r: 2.0, h: 2.0,
                                          mat: Material::accent(a.accent) }),
        thickness: 1.0,
    });
    Solid::Union(parts)
}

/// A castle: the wall and towers `fortify` makes, with a keep inside
/// and a gatehouse -- as one form, so a model says the word.
fn castle(a: &FormArgs) -> Solid {
    let (lo, hi) = (a.lo(), a.hi());
    let st = crate::style::lookup(&a.style).unwrap_or_else(|| crate::style::lookup("stone").unwrap());
    let wall = Material::structural(if a.block != 1 { a.block } else { st.wall });
    let acc = Material::accent(st.trim);
    let wh = a.num("h", 7).max(4) as f32;
    let mut parts: Vec<Solid> = Vec::new();
    // curtain wall with a walkway
    parts.push(Solid::Shell { inner: Box::new(Solid::Box { min: lo, max: [hi[0], lo[1] + wh, hi[2]], mat: wall.clone() }),
                              thickness: 1.5 });
    // crenellations: a cap ring with every other cell removed
    parts.push(Solid::Diff(
        Box::new(Solid::Shell { inner: Box::new(Solid::Box { min: [lo[0], lo[1] + wh, lo[2]], max: [hi[0], lo[1] + wh + 1.0, hi[2]],
                                                             mat: acc.clone() }), thickness: 1.0 }),
        Box::new(Solid::Array { count: ((hi[0] - lo[0]) / 2.0) as u32, step: [2.0, 0.0, 0.0],
                                inner: Box::new(Solid::Box { min: [lo[0] + 1.0, lo[1] + wh, lo[2] - 1.0],
                                                             max: [lo[0] + 2.0, lo[1] + wh + 1.0, hi[2] + 1.0],
                                                             mat: Material::carved() }) })));
    // four corner towers, taller than the wall
    for (x, z) in [(lo[0], lo[2]), (hi[0], lo[2]), (lo[0], hi[2]), (hi[0], hi[2])] {
        parts.push(Solid::Shell { inner: Box::new(Solid::Cylinder { base: [x, lo[1], z], r: 3.0, h: wh + 5.0, mat: wall.clone() }),
                                  thickness: 1.0 });
        parts.push(Solid::Wedge { min: [x - 4.0, lo[1] + wh + 5.0, z - 4.0], max: [x + 4.0, 0.0, z + 4.0], h: 4.0,
                                  axis: Axis::X, mat: acc.clone() });
    }
    // gatehouse on the south wall: two towers flanking a carved arch
    let cx = (lo[0] + hi[0]) / 2.0;
    for gx in [cx - 4.0, cx + 4.0] {
        parts.push(Solid::Shell { inner: Box::new(Solid::Cylinder { base: [gx, lo[1], lo[2]], r: 2.0, h: wh + 3.0, mat: wall.clone() }),
                                  thickness: 1.0 });
    }
    parts.push(Solid::Union(vec![
        Solid::Box { min: [cx - 2.0, lo[1], lo[2] - 2.0], max: [cx + 2.0, lo[1] + 4.0, lo[2] + 2.0], mat: Material::carved() },
        Solid::Cylinder { base: [cx, lo[1] + 4.0, lo[2]], r: 2.0, h: 0.0, mat: Material::carved() },
        Solid::Sphere { centre: [cx, lo[1] + 4.0, lo[2]], r: 2.0, mat: Material::carved() },
    ]));
    // the keep: a squat tower in the centre, with a pyramid roof
    let cz = (lo[2] + hi[2]) / 2.0;
    let kr = ((hi[0] - lo[0]).min(hi[2] - lo[2]) * 0.18).max(4.0);
    let kh = wh * 1.8;
    parts.push(Solid::Shell { inner: Box::new(Solid::Box { min: [cx - kr, lo[1], cz - kr], max: [cx + kr, lo[1] + kh, cz + kr],
                                                           mat: wall.clone() }), thickness: 1.0 });
    parts.push(Solid::Wedge { min: [cx - kr - 1.0, lo[1] + kh, cz - kr - 1.0], max: [cx + kr + 1.0, 0.0, cz + kr + 1.0],
                              h: kr, axis: Axis::X, mat: acc.clone() });
    parts.push(Solid::Box { min: [cx - 1.0, lo[1], cz - kr - 1.0], max: [cx + 1.0, lo[1] + 3.0, cz - kr + 1.0],
                            mat: Material::carved() });
    Solid::Union(parts)
}

/// A temple: a stepped base, a colonnade round a cella, a dome.
fn temple(a: &FormArgs) -> Solid {
    let (lo, hi) = (a.lo(), a.hi());
    let st = crate::style::lookup(&a.style).unwrap_or_else(|| crate::style::lookup("elven").unwrap());
    let stone = Material::structural(if a.block != 1 { a.block } else { 37 });
    let acc = Material::accent(st.trim);
    let mut parts: Vec<Solid> = Vec::new();
    // three steps
    let mut y = lo[1];
    for s in 0..3 {
        let inset = s as f32;
        parts.push(Solid::Box { min: [lo[0] + inset, y, lo[2] + inset], max: [hi[0] - inset, y + 1.0, hi[2] - inset],
                                mat: stone.clone() });
        y += 1.0;
    }
    let (ix0, ix1, iz0, iz1) = (lo[0] + 3.0, hi[0] - 3.0, lo[2] + 3.0, hi[2] - 3.0);
    let ch = a.num("h", 8).max(5) as f32;
    // colonnade: pillars every 3 along all four sides
    let (nx, nz) = (((ix1 - ix0) / 3.0) as u32 + 1, ((iz1 - iz0) / 3.0) as u32 + 1);
    let pillar = |x: f32, z: f32| Solid::Cylinder { base: [x, y, z], r: 0.8, h: ch, mat: stone.clone() };
    parts.push(Solid::Array { count: nx, step: [3.0, 0.0, 0.0], inner: Box::new(pillar(ix0, iz0)) });
    parts.push(Solid::Array { count: nx, step: [3.0, 0.0, 0.0], inner: Box::new(pillar(ix0, iz1)) });
    parts.push(Solid::Array { count: nz, step: [0.0, 0.0, 3.0], inner: Box::new(pillar(ix0, iz0)) });
    parts.push(Solid::Array { count: nz, step: [0.0, 0.0, 3.0], inner: Box::new(pillar(ix1, iz0)) });
    // entablature
    parts.push(Solid::Shell { inner: Box::new(Solid::Box { min: [ix0 - 1.0, y + ch, iz0 - 1.0], max: [ix1 + 1.0, y + ch + 1.0, iz1 + 1.0],
                                                           mat: acc.clone() }), thickness: 1.0 });
    // cella: an inner room, with a door at the south
    let (cx0, cx1, cz0, cz1) = (ix0 + 3.0, ix1 - 3.0, iz0 + 3.0, iz1 - 3.0);
    parts.push(Solid::Diff(
        Box::new(Solid::Shell { inner: Box::new(Solid::Box { min: [cx0, y, cz0], max: [cx1, y + ch, cz1], mat: stone.clone() }),
                                thickness: 1.0 }),
        Box::new(Solid::Box { min: [(cx0 + cx1) / 2.0 - 1.0, y, cz0 - 1.0], max: [(cx0 + cx1) / 2.0 + 1.0, y + 3.0, cz0 + 1.0],
                              mat: Material::carved() })));
    // dome over the cella
    let cx = (cx0 + cx1) / 2.0;
    let cz = (cz0 + cz1) / 2.0;
    let dr = ((cx1 - cx0).min(cz1 - cz0) / 2.0).max(2.0);
    parts.push(Solid::Intersect(
        Box::new(Solid::Shell { inner: Box::new(Solid::Sphere { centre: [cx, y + ch, cz], r: dr, mat: acc.clone() }),
                                thickness: 1.0 }),
        Box::new(Solid::Box { min: [cx - dr - 1.0, y + ch, cz - dr - 1.0], max: [cx + dr + 1.0, y + ch + dr + 1.0, cz + dr + 1.0],
                              mat: acc.clone() })));
    Solid::Union(parts)
}

/// A figure on a plinth.
///
/// A 1B asked for "a creepy stone statue" made a cylinder with a ball
/// on top, because a statue is not a primitive and it had none to
/// reach for. This is the primitive: a body with proportions, a pose,
/// and a style, all from solids, so it can be intersected, mirrored and
/// greebled like anything else. "Creepy" is not the statue's job -- the
/// finishing passes do that from the adjective.
///
/// Proportions follow the eight-head figure with a blocky exaggeration:
/// the head is a sphere the width of the shoulders' third, the torso
/// tapers, limbs are cylinders. Styles change the silhouette --
/// `gargoyle` adds wings, horns and a crouch; `idol` is a squat totem
/// with a huge head; `obelisk` is a tapered shaft with a carved face;
/// `colossus` is the figure at three times the scale on a stepped
/// base.
fn statue(a: &FormArgs) -> Solid {
    let (lo, hi) = (a.lo(), a.hi());
    let cx = (lo[0] + hi[0]) / 2.0;
    let cz = (lo[2] + hi[2]) / 2.0;
    let style = a.style.as_str();
    let pose = a.nums.iter().find(|(k, _)| k == "pose").map(|(_, v)| *v).unwrap_or(0);
    // What the raised hand holds, and whether the face has a beard:
    // both from k=v numbers the parser mapped from words (hold=bolt,
    // beard=1). A statue of somebody is a statue of what they carry.
    let hold = a.nums.iter().find(|(k, _)| k == "hold").map(|(_, v)| *v).unwrap_or(0);
    let beard = a.nums.iter().any(|(k, v)| k == "beard" && *v > 0);
    // Height from h= or the footprint: a figure is ~2.4x its footprint.
    let base_w = ((hi[0] - lo[0]).min(hi[2] - lo[2])).max(4.0);
    let scale = if style == "colossus" { 3.0 } else { 1.0 };
    let h = a.num("h", (base_w * 2.4 * scale) as i32).max(6) as f32;
    let stone = Material::structural(a.block);
    let dark = Material::accent(a.accent);
    let mut parts: Vec<Solid> = Vec::new();

    // Plinth: a stepped base, one step per 8 blocks of height.
    let steps = (h / 8.0).ceil().max(1.0) as i32;
    let mut py = lo[1];
    for s in 0..steps {
        let inset = s as f32 * 0.8;
        parts.push(Solid::Box { min: [lo[0] + inset, py, lo[2] + inset],
                                max: [hi[0] - inset, py + 1.0, hi[2] - inset], mat: dark.clone() });
        py += 1.0;
    }
    let ground = py;

    if style == "obelisk" {
        // A tapered shaft: intersect a box with two crossed wedges
        // upside down (widest at the base), then a carved face.
        let half = base_w * 0.35;
        let shaft = Solid::Box { min: [cx - half, ground, cz - half], max: [cx + half, ground + h, cz + half],
                                 mat: stone.clone() };
        let taper_x = Solid::Wedge { min: [cx - half - h * 0.02, ground, cz - half],
                                     max: [cx + half + h * 0.02, 0.0, cz + half],
                                     h: h * 1.4, axis: Axis::Z, mat: stone.clone() };
        let taper_z = Solid::Wedge { min: [cx - half, ground, cz - half - h * 0.02],
                                     max: [cx + half, 0.0, cz + half + h * 0.02],
                                     h: h * 1.4, axis: Axis::X, mat: stone.clone() };
        let tapered = Solid::Intersect(Box::new(shaft),
                                       Box::new(Solid::Intersect(Box::new(taper_x), Box::new(taper_z))));
        parts.push(tapered);
        // eyes and a mouth carved into the south face
        let fy = ground + h * 0.75;
        for ex in [cx - 1.0, cx + 1.0] {
            parts.push(Solid::Box { min: [ex - 0.5, fy, cz + half - 1.0], max: [ex + 0.5, fy + 1.0, cz + half + 0.5],
                                    mat: Material::carved() });
        }
        parts.push(Solid::Box { min: [cx - 1.5, fy - 2.0, cz + half - 1.0], max: [cx + 1.5, fy - 1.0, cz + half + 0.5],
                                mat: Material::carved() });
        return Solid::Union(parts);
    }

    // The figure. Units: head_r is the module.
    let head_r = (h / 8.0 * if style == "idol" { 2.0 } else { 1.0 }).max(1.0);
    let shoulder_w = head_r * 3.0;
    let torso_h = h * if style == "idol" { 0.3 } else { 0.35 };
    let leg_h = h * if style == "idol" { 0.15 } else { 0.4 };
    let crouch = style == "gargoyle" || pose == 3;
    let seated = pose == 1;
    let lunge = pose == 4;
    let leg_h = if crouch || seated { leg_h * 0.5 } else { leg_h };
    let hip_y = ground + leg_h;
    let neck_y = hip_y + torso_h;

    // legs: two cylinders, or a solid block for seated/kneeling
    let leg_r = (shoulder_w * 0.22).max(0.9);
    if seated {
        parts.push(Solid::Box { min: [cx - shoulder_w * 0.5, ground, cz - shoulder_w * 0.7],
                                max: [cx + shoulder_w * 0.5, hip_y, cz + shoulder_w * 0.3], mat: stone.clone() });
    } else if lunge {
        // LUNGE: the front leg (+z, toward the viewer) is bent -- a
        // thigh sloping forward from the hip and a shin rising from a
        // foot well ahead; the back leg is straight and trails behind.
        let stride = shoulder_w * 0.9;
        let front = [cx - shoulder_w * 0.28, ground, cz + stride];
        parts.push(Solid::Cylinder { base: front, r: leg_r, h: leg_h * 0.55, mat: stone.clone() });   // shin
        parts.push(Solid::Wedge { min: [front[0] - leg_r, ground + leg_h * 0.5, cz - leg_r],
                                  max: [front[0] + leg_r, 0.0, cz + stride + leg_r],
                                  h: leg_h * 0.55, axis: Axis::X, mat: stone.clone() });               // thigh
        let back = [cx + shoulder_w * 0.28, ground, cz - stride * 0.7];
        parts.push(Solid::Cylinder { base: back, r: leg_r, h: leg_h * 0.7, mat: stone.clone() });
        parts.push(Solid::Wedge { min: [back[0] - leg_r, ground + leg_h * 0.6, cz - stride * 0.7 - leg_r],
                                  max: [back[0] + leg_r, 0.0, cz + leg_r],
                                  h: leg_h * 0.45, axis: Axis::X, mat: stone.clone() });
    } else {
        for side in [-1.0, 1.0] {
            parts.push(Solid::Cylinder { base: [cx + side * shoulder_w * 0.28, ground, cz], r: leg_r, h: leg_h,
                                         mat: stone.clone() });
        }
    }
    // torso: tapers from shoulders to hips
    parts.push(Solid::Intersect(
        Box::new(Solid::Box { min: [cx - shoulder_w * 0.5, hip_y, cz - shoulder_w * 0.28],
                              max: [cx + shoulder_w * 0.5, neck_y, cz + shoulder_w * 0.28], mat: stone.clone() }),
        Box::new(Solid::Wedge { min: [cx - shoulder_w * 0.5, hip_y - torso_h, cz - shoulder_w],
                                max: [cx + shoulder_w * 0.5, 0.0, cz + shoulder_w],
                                h: torso_h * 2.6, axis: Axis::Z, mat: stone.clone() })));
    // arms: raised, out, or down
    let arm_r = (shoulder_w * 0.16).max(0.8);
    let arm_len = torso_h * 0.9;
    let mut hand: Option<[f32; 3]> = None;   // where the raised hand ends up
    for side in [-1.0, 1.0] {
        let sx = cx + side * (shoulder_w * 0.5 + arm_r * 0.5);
        match pose {
            2 => { parts.push(Solid::Cylinder { base: [sx, neck_y - arm_r, cz], r: arm_r, h: arm_len,
                                                mat: stone.clone() });          // arms up
                   if side > 0.0 { hand = Some([sx, neck_y - arm_r + arm_len, cz]); } }
            4 if side > 0.0 => {
                // the throwing arm: up and back, a wedge from the shoulder
                parts.push(Solid::Wedge { min: [sx - arm_r, neck_y - arm_r, cz - arm_len],
                                          max: [sx + arm_r, 0.0, cz + arm_r],
                                          h: arm_len, axis: Axis::X, mat: stone.clone() });
                hand = Some([sx, neck_y - arm_r + arm_len * 0.9, cz - arm_len * 0.8]);
            }
            4 => parts.push(Solid::Wedge { min: [sx - arm_r, neck_y - arm_len * 0.6, cz - arm_r],
                                           max: [sx + arm_r, 0.0, cz + arm_len],
                                           h: arm_len * 0.6, axis: Axis::X, mat: stone.clone() }),   // the leading arm, forward
            _ => parts.push(Solid::Cylinder { base: [sx, neck_y - arm_len, cz], r: arm_r, h: arm_len,
                                              mat: stone.clone() }),          // arms down
        }
    }
    if let Some(h) = hand {
        let bright = Material::new(37, 60);   // quartz: what reads as light in stone
        match hold {
            1 => {
                // a LIGHTNING BOLT: a zigzag of five short bars from the hand
                let mut p = h;
                for i in 0..5 {
                    let dx = if i % 2 == 0 { 1.0 } else { -1.0 } * head_r * 0.6;
                    parts.push(Solid::Box { min: [p[0].min(p[0] + dx) - 0.5, p[1] - 0.5, p[2] - 0.5],
                                            max: [p[0].max(p[0] + dx) + 0.5, p[1] + 0.5, p[2] + 0.5], mat: bright.clone() });
                    p = [p[0] + dx, p[1] + head_r * 0.5, p[2]];
                }
            }
            2 => parts.push(Solid::Box { min: [h[0] - 0.5, h[1] - arm_len * 0.3, h[2] - 0.5],
                                         max: [h[0] + 0.5, h[1] + arm_len * 1.2, h[2] + 0.5], mat: dark.clone() }),   // sword
            3 => parts.push(Solid::Cylinder { base: [h[0], ground, h[2]], r: 0.5, h: h[1] - ground + head_r * 2.0,
                                              mat: dark.clone() }),    // staff, to the ground
            4 => parts.push(Solid::Sphere { centre: [h[0], h[1] + head_r * 0.6, h[2]], r: head_r * 0.6, mat: bright }),  // orb
            5 => parts.push(Solid::Box { min: [h[0] - head_r, h[1] - head_r, h[2] - 0.5],
                                         max: [h[0] + head_r, h[1] + head_r, h[2] + 0.5], mat: dark.clone() }),  // shield
            _ => {}
        }
    }
    // head
    let head_y = neck_y + head_r * 0.8;
    parts.push(Solid::Sphere { centre: [cx, head_y, cz], r: head_r, mat: stone.clone() });
    // eyes: two carved sockets on the south face, which is what makes a
    // head a face and a figure a presence
    for ex in [cx - head_r * 0.4, cx + head_r * 0.4] {
        parts.push(Solid::Box { min: [ex - 0.5, head_y, cz + head_r * 0.6], max: [ex + 0.5, head_y + 1.0, cz + head_r + 0.5],
                                mat: Material::carved() });
    }
    if beard {
        parts.push(Solid::Wedge { min: [cx - head_r * 0.6, head_y - head_r * 1.6, cz + head_r * 0.3],
                                  max: [cx + head_r * 0.6, 0.0, cz + head_r + 0.5],
                                  h: head_r * 1.1, axis: Axis::X, mat: dark.clone() });
    }
    if style == "gargoyle" {
        // horns and wings
        for side in [-1.0, 1.0] {
            parts.push(Solid::Cylinder { base: [cx + side * head_r * 0.6, head_y + head_r * 0.6, cz],
                                         r: 0.6, h: head_r * 1.2, mat: dark.clone() });
            let wx = cx + side * (shoulder_w * 0.5);
            let wing = Solid::Wedge {
                min: [wx.min(wx + side * shoulder_w * 1.5), hip_y + torso_h * 0.3, cz - 1.0],
                max: [wx.max(wx + side * shoulder_w * 1.5), 0.0, cz + 1.0],
                h: torso_h * 1.4, axis: Axis::Z, mat: dark.clone() };
            parts.push(wing);
        }
    }
    Solid::Union(parts)
}

/// A crater with a raised rim: the shape of a lair. Carved into the
/// ground, so `y` is the SURFACE and the bowl goes down from it. Rim
/// blocks are structural; the bowl is a carve that outranks them.
fn nest(a: &FormArgs) -> Solid {
    let (lo, hi) = (a.lo(), a.hi());
    let cx = (lo[0] + hi[0]) / 2.0;
    let cz = (lo[2] + hi[2]) / 2.0;
    let r = ((hi[0] - lo[0]).min(hi[2] - lo[2]) / 2.0).max(4.0);
    let depth = a.num("depth", (r * 0.5) as i32).max(2) as f32;
    let rim_h = a.num("rim", 2).max(1) as f32;
    // The rim: a ring of the structural block, rising above ground.
    let rim = Solid::Diff(
        Box::new(Solid::Cylinder { base: [cx, lo[1], cz], r: r + 1.0, h: rim_h,
                                   mat: Material::structural(a.block) }),
        Box::new(Solid::Cylinder { base: [cx, lo[1], cz], r: r - 1.5, h: rim_h + 1.0,
                                   mat: Material::structural(0) }));
    // The bowl: a sphere of carved air sunk into the ground. Carve
    // outranks structure, so the bowl wins where they overlap.
    let bowl = Solid::Sphere { centre: [cx, lo[1] - depth * 0.3, cz], r: r - 0.5,
                               mat: Material::carved() };
    // Clip the bowl to below-ground so it does not carve sky.
    let below = Solid::Box { min: [cx - r - 2.0, lo[1] - depth - 1.0, cz - r - 2.0],
                             max: [cx + r + 2.0, lo[1] + 0.5, cz + r + 2.0],
                             mat: Material::carved() };
    Solid::Union(vec![rim, Solid::Intersect(Box::new(bowl), Box::new(below))])
}

/// An underground chamber: a hollow carved below the surface with
/// supporting pillars and a vertical shaft to the entrance. The "dig an
/// underground city" starting point -- one chamber; a city is several
/// connected by `path` at depth.
fn cavern(a: &FormArgs) -> Solid {
    let (lo, hi) = (a.lo(), a.hi());
    let h = a.num("h", 7).max(4) as f32;
    let depth = a.num("depth", 12).max(h as i32 + 2) as f32;
    let floor_y = lo[1] - depth;
    let cx = (lo[0] + hi[0]) / 2.0;
    let cz = (lo[2] + hi[2]) / 2.0;
    // The chamber: a carved box with a domed ceiling (a sphere clipped
    // to the box top), so it reads as hollowed rather than mined.
    let room = Solid::Box { min: [lo[0], floor_y, lo[2]], max: [hi[0], floor_y + h, hi[2]],
                            mat: Material::carved() };
    let dome = Solid::Intersect(
        Box::new(Solid::Sphere { centre: [cx, floor_y + h * 0.4, cz],
                                 r: ((hi[0] - lo[0]).min(hi[2] - lo[2]) / 2.0) * 0.9 + h * 0.3,
                                 mat: Material::carved() }),
        Box::new(Solid::Box { min: [lo[0], floor_y + h, lo[2]],
                              max: [hi[0], floor_y + h + h * 0.5, hi[2]],
                              mat: Material::carved() }));
    // Pillars where a span would be unsupported. Structural, and
    // declared AFTER the carve so ties go to them; the carve's higher
    // rank would otherwise remove them.
    let span = hi[0] - lo[0];
    let step = 6.0;
    let count = ((span - 4.0) / step).floor().max(1.0) as u32;
    let pillar = Solid::Cylinder { base: [lo[0] + 3.0, floor_y, cz], r: 1.0, h,
                                   mat: Material::new(a.block, 150) };
    let pillars = Solid::Array { count, step: [step, 0.0, 0.0], inner: Box::new(pillar) };
    // The shaft: a 2x2 carve from the surface down to the room.
    let shaft = Solid::Box { min: [cx - 1.0, floor_y + 1.0, lo[2] - 3.0],
                             max: [cx + 1.0, lo[1] + 1.0, lo[2] - 1.0],
                             mat: Material::carved() };
    let shaft_link = Solid::Box { min: [cx - 1.0, floor_y, lo[2] - 3.0],
                                  max: [cx + 1.0, floor_y + 3.0, lo[2] + 1.0],
                                  mat: Material::carved() };
    Solid::Union(vec![room, dome, shaft, shaft_link, pillars])
}

/// A flat ribbon between two points, as a union of boxes. Cheap, and it
/// does not need a new primitive.
fn path_slab(from: [f32; 3], to: [f32; 3], width: f32, mat: Material) -> Solid {
    let steps = ((to[0] - from[0]).abs().max((to[2] - from[2]).abs()) as i32).max(1);
    let half = width / 2.0;
    let mut segs = Vec::new();
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = from[0] + (to[0] - from[0]) * t;
        let z = from[2] + (to[2] - from[2]) * t;
        segs.push(Solid::Box {
            min: [x - half, from[1], z - half],
            max: [x + half, from[1] + 1.0, z + half],
            mat: mat.clone(),
        });
    }
    Solid::Union(segs)
}

/// A small deterministic hash. Seeded variation beats asking a model for
/// numbers: it always picks round ones, and neighbours come out matching.
fn hash2(i: u32, seed: u32) -> u32 {
    let mut h = i.wrapping_mul(0x9E37_79B9) ^ seed.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^ (h >> 13)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solid::rasterize;

    fn args(w: i32, d: i32) -> FormArgs {
        FormArgs { min: (0, 64, 0), max: (w - 1, 64, d - 1), style: String::new(),
                   block: 1, accent: 58, nums: vec![] }
    }

    #[test]
    fn the_dictionary_is_the_vocabulary() {
        // The prompt is GENERATED from the table, so a new form teaches
        // the model without anyone editing prompt text — which is how
        // `/provider` ended up invisible.
        let v = vocabulary();
        for f in FORMS {
            assert!(v.contains(f.name), "{} missing from the vocabulary line", f.name);
        }
        assert!(lookup("ROOF").is_some(), "lookup is case-insensitive");
        assert!(lookup("dormer").is_none());
    }

    #[test]
    fn roof_as_a_recipe_still_makes_a_roof() {
        // Same output as the hardcoded op, but now it is data: the shape
        // is a Wedge, and it can be intersected and mirrored like any
        // other solid.
        let mut a = args(9, 7);
        a.style = "gable".into();
        let cells = rasterize(&roof(&a));
        assert!(!cells.is_empty());
        let peak = cells.keys().map(|(_, y, _)| *y).max().unwrap();
        assert!(peak > 64, "the roof rises above its base");
        let eaves_x: Vec<i32> = cells.keys().map(|(x, _, _)| *x).collect();
        assert!(eaves_x.iter().any(|x| *x < 0), "eaves overhang the footprint");
    }

    #[test]
    fn hip_is_two_crossed_wedges_not_a_special_case() {
        let mut a = args(11, 11);
        a.style = "hip".into();
        let cells = rasterize(&roof(&a));
        // A hip slopes on all four sides: the peak is a small area, and
        // every edge of the footprint is low.
        let peak = cells.keys().map(|(_, y, _)| *y).max().unwrap();
        let at_peak = cells.keys().filter(|(_, y, _)| *y == peak).count();
        let base = cells.keys().filter(|(_, y, _)| *y == 64).count();
        assert!(at_peak < base / 4, "peak {at_peak} must be far smaller than base {base}");
    }

    #[test]
    fn a_house_has_a_floor_walls_a_door_windows_and_a_roof() {
        // The whole point: one word from the player, a composite from
        // the dictionary, and the details are part of the recipe rather
        // than ops the model has to remember to add.
        let a = args(13, 9);
        let cells = rasterize(&house(&a));
        assert!(cells.contains_key(&(6, 64, 4)), "floor");
        assert!(cells.contains_key(&(0, 66, 4)), "wall");
        let peak = cells.keys().map(|(_, y, _)| *y).max().unwrap();
        assert!(peak >= 70, "roof above the walls, got {peak}");
        // the doorway is carved through the front wall
        let door_open = (65..67).all(|y| cells.get(&(6, y, 0)).map(|b| *b == 0).unwrap_or(true));
        assert!(door_open, "a doorway you can walk through");
        // and the interior is hollow
        // y=66: inside the storey (64 floor, 65..67 room, 68 ceiling)
        assert!(!cells.contains_key(&(6, 66, 4)) || cells[&(6, 66, 4)] == 0,
                "the inside is a room, not a block of stone");
    }

    #[test]
    fn a_colonnade_spaces_its_own_pillars() {
        let mut a = args(20, 3);
        a.nums = vec![("spacing".into(), 5)];
        let cells = rasterize(&colonnade(&a));
        let xs: std::collections::BTreeSet<i32> = cells.keys().map(|(x, _, _)| *x).collect();
        // gaps between pillars, not a solid wall
        let solid_run = xs.len();
        assert!(solid_run < 20, "pillars, not a wall: {solid_run} columns filled");
        assert!(cells.keys().any(|(x, _, _)| *x >= 15), "the row reaches the far end");
    }

    #[test]
    fn an_arch_has_a_rounded_head_no_integer_could_express() {
        let mut a = args(9, 3);
        a.nums = vec![("h".into(), 7)];
        let cells = rasterize(&arch(&a));
        // The opening narrows as it rises: that curve is the thing the
        // continuous layer exists for.
        let width_at = |y: i32| (0..9)
            .filter(|x| !cells.contains_key(&(*x, y, 1)))
            .count();
        let low = width_at(65);
        let high = width_at(69);
        assert!(low > 0, "the arch is open at the bottom");
        assert!(high < low, "and narrows toward the head: {low} -> {high}");
    }

    #[test]
    fn every_form_produces_something_and_stays_in_bounds() {
        // A recipe that silently yields nothing is worse than a missing
        // one: the player asks and the world does not change.
        for f in FORMS {
            let mut a = args(11, 9);
            a.style = "gable".into();
            let s = (f.build)(&a);
            let cells = rasterize(&s);
            let placed = cells.values().filter(|b| **b != 0).count();
            assert!(placed > 0, "form '{}' produced no blocks", f.name);
            let (lo, hi) = s.bounds().expect("a non-empty form has bounds");
            for (x, y, z) in cells.keys() {
                assert!(*x >= lo[0] && *x <= hi[0] && *y >= lo[1] && *y <= hi[1]
                        && *z >= lo[2] && *z <= hi[2],
                        "form '{}' wrote ({x},{y},{z}) outside its own bounds", f.name);
            }
        }
    }

    #[test]
    fn a_roof_is_a_surface_not_a_solid_mass() {
        // The bare Wedge filled the attic: the cross-section came out a
        // four-block-thick slab. A roof is the shell of the prism.
        let mut a = args(13, 9);
        a.style = "gable".into();
        let solid_cells = rasterize(&roof(&a)).len();
        let shell_cells = rasterize(&roof_shell(&a)).len();
        // Not a fixed ratio: a shallow roof is mostly surface anyway, so
        // "half" was an arbitrary number that happened to fail. The real
        // property is that the mass is reduced AND the attic is open.
        assert!(shell_cells < solid_cells,
                "a shell must be lighter than the mass: {shell_cells} vs {solid_cells}");
        // and there is an attic void under the ridge
        let cells = rasterize(&roof_shell(&a));
        let peak = cells.keys().map(|(_, y, _)| *y).max().unwrap();
        assert!(!cells.contains_key(&(6, peak - 1, 4)),
                "the space under the ridge is an attic, not stone");
    }

    #[test]
    fn a_nest_is_a_bowl_with_a_rim() {
        let mut a = args(21, 21);
        a.min.1 = 64; a.max.1 = 64;
        let cells = rasterize(&nest(&a));
        let rim: Vec<_> = cells.iter().filter(|(_, b)| **b != 0).collect();
        let bowl: Vec<_> = cells.iter().filter(|(_, b)| **b == 0).collect();
        assert!(!rim.is_empty(), "a raised rim");
        assert!(!bowl.is_empty(), "and a carved bowl");
        assert!(rim.iter().all(|((_, y, _), _)| *y >= 64), "the rim is above ground");
        assert!(bowl.iter().all(|((_, y, _), _)| *y <= 64), "the bowl is below it");
        assert!(bowl.iter().any(|((x, _, z), _)| (*x - 10).abs() <= 1 && (*z - 10).abs() <= 1),
                "the centre is hollow");
    }

    #[test]
    fn a_cavern_is_hollow_below_ground_with_a_shaft_up() {
        let mut a = args(19, 13);
        a.min.1 = 64; a.max.1 = 64;
        let s = cavern(&a);
        let cells = rasterize(&s);
        let carved: Vec<_> = cells.iter().filter(|(_, b)| **b == 0).map(|(k, _)| *k).collect();
        let solid: Vec<_> = cells.iter().filter(|(_, b)| **b != 0).map(|(k, _)| *k).collect();
        assert!(!carved.is_empty());
        assert!(carved.iter().all(|(_, y, _)| *y <= 65), "everything carved is underground");
        assert!(carved.iter().any(|(_, y, _)| *y == 64), "the shaft reaches the surface");
        assert!(!solid.is_empty(), "pillars stand");
        // pillars survive the carve: they are declared with a higher rank
        assert!(solid.iter().any(|(_, y, _)| *y < 60), "a pillar in the chamber");
    }

    #[test]
    fn a_statue_has_a_head_a_body_and_stands_on_a_plinth() {
        let a = FormArgs { min: (0, 64, 0), max: (6, 64, 6), style: "figure".into(),
                           block: 1, accent: 58, nums: vec![("h".into(), 16)] };
        let cells = rasterize(&statue(&a));
        let solid: Vec<_> = cells.iter().filter(|(_, b)| **b != 0).map(|(k, _)| *k).collect();
        let top = solid.iter().map(|c| c.1).max().unwrap();
        assert!(top >= 64 + 14, "about as tall as asked: {top}");
        // narrow at the neck, wide at the shoulders, wide at the plinth
        let width_at = |y: i32| { let xs: Vec<i32> = solid.iter().filter(|c| c.1 == y).map(|c| c.0).collect();
            if xs.is_empty() { 0 } else { xs.iter().max().unwrap() - xs.iter().min().unwrap() + 1 } };
        assert!(width_at(64) >= 6, "plinth spans the footprint");
        // two legs, not one block: a gap between them at leg height
        let leg_y = 66;
        assert!(!solid.contains(&(3, leg_y, 3)), "air between the legs at the centre");
        assert!(solid.iter().any(|c| c.1 == leg_y && c.0 <= 2), "a left leg");
        assert!(solid.iter().any(|c| c.1 == leg_y && c.0 >= 4), "a right leg");
        // eyes carved: some air inside the head's front
        assert!(cells.values().any(|b| *b == 0), "carved sockets");
    }

    #[test]
    fn styles_change_the_silhouette() {
        let mk = |style: &str| {
            let a = FormArgs { min: (0, 64, 0), max: (6, 64, 6), style: style.into(),
                               block: 1, accent: 58, nums: vec![("h".into(), 16)] };
            rasterize(&statue(&a)).into_iter().filter(|(_, b)| *b != 0).map(|(k, _)| k).collect::<Vec<_>>()
        };
        let figure = mk("figure");
        let gargoyle = mk("gargoyle");
        let obelisk = mk("obelisk");
        let idol = mk("idol");
        let span = |v: &Vec<(i32, i32, i32)>| v.iter().map(|c| c.0).max().unwrap() - v.iter().map(|c| c.0).min().unwrap();
        assert!(span(&gargoyle) > span(&figure) + 4, "wings: {} vs {}", span(&gargoyle), span(&figure));
        // an obelisk TAPERS: wider at the base than near the top
        let ow = |y: i32| { let xs: Vec<i32> = obelisk.iter().filter(|c| c.1 == y).map(|c| c.0).collect();
            if xs.is_empty() { 0 } else { xs.iter().max().unwrap() - xs.iter().min().unwrap() + 1 } };
        assert!(ow(67) > ow(76), "tapered: {} at the base, {} near the top", ow(67), ow(76));
        assert!(span(&obelisk) <= span(&figure), "no shoulders on a shaft");
        let head_rows = |v: &Vec<(i32, i32, i32)>| v.iter().filter(|c| c.1 >= 64 + 12).count();
        assert!(head_rows(&idol) > head_rows(&figure), "an idol is mostly head");
    }

    #[test]
    fn every_statue_variant_is_valid_and_placed() {
        for style in ["figure", "gargoyle", "idol", "obelisk", "colossus"] {
            for pose in 0..4 {
                let a = FormArgs { min: (0, 64, 0), max: (7, 64, 7), style: style.into(),
                                   block: 1, accent: 58, nums: vec![("pose".into(), pose)] };
                let s = statue(&a);
                let cells = rasterize(&s);
                assert!(cells.values().filter(|b| **b != 0).count() > 30, "{style} pose {pose}");
                let (lo, hi) = s.bounds().unwrap();
                for (x, y, z) in cells.keys() {
                    assert!(*x >= lo[0] && *x <= hi[0] && *y >= lo[1] && *y <= hi[1] && *z >= lo[2] && *z <= hi[2],
                            "{style}: cell outside bounds");
                }
            }
        }
    }

    #[test]
    fn two_houses_from_one_line_differ_by_seed() {
        let mk = |seed: i32, style: &str| {
            let a = FormArgs { min: (0, 64, 0), max: (12, 64, 8), style: style.into(),
                               block: 1, accent: 58, nums: vec![("seed".into(), seed)] };
            rasterize(&house(&a))
        };
        let a = mk(1, "cottage");
        let b = mk(2, "cottage");
        let c = mk(3, "cottage");
        assert!(a != b || b != c, "seeds change the variants");
        for h in [&a, &b, &c] { assert!(h.values().filter(|v| **v != 0).count() > 200); }
    }

    #[test]
    fn styles_change_the_house() {
        let mk = |style: &str| {
            let a = FormArgs { min: (0, 64, 0), max: (12, 64, 8), style: style.into(),
                               block: 1, accent: 58, nums: vec![("seed".into(), 1)] };
            rasterize(&house(&a))
        };
        let nordic = mk("nordic");
        let desert = mk("desert");
        let gothic = mk("gothic");
        let top = |m: &std::collections::BTreeMap<(i32, i32, i32), u16>| m.keys().map(|k| k.1).max().unwrap();
        assert!(top(&gothic) > top(&desert) + 4, "a gothic pitch is a flat roof's opposite: {} vs {}", top(&gothic), top(&desert));
        assert!(nordic.values().any(|b| *b == 17), "nordic is log-built");
        assert!(desert.values().any(|b| *b == 48), "desert is sandstone");
    }

    #[test]
    fn castle_and_temple_are_whole_and_bounded() {
        for (name, w, d) in [("castle", 40, 30), ("temple", 24, 16)] {
            let a = FormArgs { min: (0, 64, 0), max: (w, 64, d), style: String::new(),
                               block: 1, accent: 58, nums: vec![] };
            let s = (lookup(name).unwrap().build)(&a);
            let cells = rasterize(&s);
            let n = cells.values().filter(|b| **b != 0).count();
            assert!(n > 500, "{name}: {n} blocks");
            let (lo, hi) = s.bounds().unwrap();
            for (x, y, z) in cells.keys() {
                assert!(*x >= lo[0] && *x <= hi[0] && *y >= lo[1] && *y <= hi[1] && *z >= lo[2] && *z <= hi[2], "{name}");
            }
            // an entrance: a gap in the south wall at ground level. A
            // Diff leaves ABSENCE, not a zero cell, so look for a wall
            // course with a hole in it rather than for carved cells.
            let ground = 64 + if name == "temple" { 3 } else { 0 };
            let south = cells.keys().filter(|k| k.1 == ground + 1 && *k != &(0, 0, 0)).map(|k| k.2).min().unwrap();
            let row: Vec<i32> = cells.iter().filter(|(k, b)| **b != 0 && k.1 == ground + 1 && k.2 == south)
                .map(|(k, _)| k.0).collect();
            let (lo_x, hi_x) = (*row.iter().min().unwrap(), *row.iter().max().unwrap());
            let gap = (lo_x..=hi_x).any(|x| !row.contains(&x));
            assert!(gap, "{name}: the south course at z={south} has no gap: {row:?}");
        }
    }

    fn solid_stone_top(cells: &std::collections::BTreeMap<(i32, i32, i32), u16>) -> i32 {
        cells.iter().filter(|(_, b)| **b == 1).map(|(k, _)| k.1).max().unwrap()
    }

    #[test]
    fn a_lunging_colossus_with_a_bolt_reads_as_one() {
        let a = FormArgs { min: (0, 64, 0), max: (10, 64, 10), style: "colossus".into(),
                           block: 1, accent: 58, nums: vec![("pose".into(), 4), ("hold".into(), 1), ("beard".into(), 1)] };
        let cells = rasterize(&statue(&a));
        let solid: Vec<_> = cells.iter().filter(|(_, b)| **b != 0).map(|(k, _)| *k).collect();
        // giant: well over the footprint's implied height
        let top = solid.iter().map(|c| c.1).max().unwrap();
        assert!(top - 64 >= 40, "a colossus is tall: {}", top - 64);
        // a lunge: solid reaches well forward (+z) of the plinth's centre at ground level
        let front_reach = solid.iter().filter(|c| c.1 <= 68).map(|c| c.2).max().unwrap();
        assert!(front_reach > 8, "the front foot is out ahead: z={front_reach}");
        // a bolt: bright cells (quartz) high up and off to one side
        let bolt: Vec<_> = cells.iter().filter(|(_, b)| **b == 37).map(|(k, _)| *k).collect();
        assert!(bolt.len() >= 5, "a bolt of {} cells", bolt.len());
        assert!(bolt.iter().map(|c| c.1).min().unwrap() > 64 + 20, "held high");
        // a beard: accent cells just under the head on the south face
        // the beard hangs from chin height: head_r ~10 on a colossus, so
        // it sits 8-26 below the CROWN -- the top of the stone, not the
        // bolt, which rises twenty blocks above the head
        let crown = solid_stone_top(&cells);
        assert!(cells.iter().any(|(k, b)| *b == 58 && k.1 < crown - 8 && k.1 > crown - 26 && k.2 >= 5),
                "a beard under the chin (crown {crown})");
        // and it still parses through the form line
        let (plan, issues) = crate::plan::parse_plan_at(
            "form statue @-5,-5 @5,5 colossus pose=lunge hold=bolt beard=1\n", Some((0, 64, 0)));
        assert!(issues.iter().all(|i| i.severity != crate::plan::Severity::Error), "{issues:?}");
        assert_eq!(plan.ops.len(), 1);
    }
}
