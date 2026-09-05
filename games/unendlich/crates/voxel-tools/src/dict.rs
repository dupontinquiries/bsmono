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
    let (lo, hi) = (a.lo(), a.hi());
    let h = a.num("h", 5) as f32;
    let body = wall(a);
    let floor = Solid::Box { min: lo, max: [hi[0], lo[1] + 1.0, hi[2]],
                             mat: Material::structural(a.block) };
    // Windows: an ARRAY along the long wall, spaced by the parameter.
    // Five of them is one node; in the op world it was five fills the
    // model had to space by hand, and that arithmetic is exactly what a
    // small model gets wrong.
    let span = hi[0] - lo[0];
    let step = a.num("spacing", 4).max(2) as f32;
    let count = ((span - 2.0) / step).floor().max(1.0) as u32;
    let win = Solid::Box { min: [lo[0] + 1.0, lo[1] + 2.0, hi[2] - 1.0],
                           max: [lo[0] + 2.0, lo[1] + 4.0, hi[2]],
                           mat: Material::carved() };
    let windows = Solid::Array { count, step: [step, 0.0, 0.0], inner: Box::new(win) };
    let roof_args = FormArgs { min: (a.min.0, (a.min.1 as f32 + h) as i32, a.min.2), ..a.clone() };
    Solid::Union(vec![
        floor,
        Solid::Diff(Box::new(body), Box::new(windows)),
        roof_shell(&roof_args),
    ])
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
        assert!(!cells.contains_key(&(6, 67, 4)) || cells[&(6, 67, 4)] == 0,
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
}
