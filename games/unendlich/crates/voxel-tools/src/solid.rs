//! The form layer: solids before blocks.
//!
//! WHY THIS EXISTS
//!
//! `roof` was added as a first-class op because the corpus said "roof"
//! 29 times. That was the right observation and the wrong conclusion.
//! `roof gable` is not a primitive — it is a *wedge prism, sized to a
//! footprint, hollowed, with its eaves extended*. Baking that into an
//! op freezes one answer, and then the same argument demands `arch`,
//! `buttress`, `chimney`, `dormer`, `balcony`, `colonnade`, forever. A
//! vocabulary that grows one hardcoded op per noun is not a vocabulary.
//!
//! So: two layers.
//!
//! * **This layer** is continuous and resolution-independent. Solids are
//!   primitives combined with union / difference / intersection and
//!   moved by transforms. Nothing here knows what a block is.
//! * **The block layer** rasterizes a solid into voxels, and that is
//!   where conflicts get resolved — one cell claimed by two solids picks
//!   a winner by an explicit policy rather than by whichever op ran last.
//!
//! Named forms like `roof` become DICTIONARY ENTRIES: recipes written in
//! terms of solids (see `dict.rs`). Adding `dormer` is then data, not a
//! new op, a new parser arm, a new match in three files, and a new
//! exhaustiveness break.
//!
//! WHAT THIS BUYS FOR LITTLE DETAILS
//!
//! Details are the reason for the split, not a bonus. In a
//! one-op-per-shape world, a window is `fill` calls the model has to
//! place by hand. Here:
//!
//! * A window is a `Diff` of a small box, ARRAYED along a wall face.
//!   Five windows evenly spaced is one node, not five ops.
//! * Trim is a surface rule applied after rasterizing — the cells whose
//!   neighbours cross a material boundary. It needs no coordinates at
//!   all.
//! * A door that must not be filled in by a later solid is a material
//!   with higher precedence, not an ordering constraint the model must
//!   remember.
//!
//! Coordinates are f32 because a wedge's slope, a cylinder's rim and an
//! arch's curve do not land on integers, and rounding each primitive
//! independently is what produces the one-block seams and gaps that make
//! generated builds look wrong.

use std::collections::BTreeMap;

/// What a solid is made of, and who wins a contested cell.
///
/// `rank` is the whole conflict story: when two solids claim a voxel,
/// the higher rank wins. A carved doorway (rank 100, air) beats the wall
/// it is cut from (rank 10), regardless of which was declared first —
/// so "add a door" cannot be undone by a later "make the walls thicker".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Material {
    pub block: u16,
    pub rank: i32,
}

impl Material {
    pub fn new(block: u16, rank: i32) -> Self { Self { block, rank } }
    /// Structure: walls, floors, roofs. The default.
    pub fn structural(block: u16) -> Self { Self { block, rank: 10 } }
    /// Trim and accents: sit on top of structure.
    pub fn accent(block: u16) -> Self { Self { block, rank: 50 } }
    /// Carved space: openings that must survive later structure.
    pub fn carved() -> Self { Self { block: 0, rank: 100 } }
}

/// An axis, for mirroring and arraying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis { X, Y, Z }

/// A continuous solid. Sample it anywhere; it does not know about grids.
#[derive(Debug, Clone)]
pub enum Solid {
    /// Axis-aligned box, inclusive of `min`, exclusive of `max`.
    Box { min: [f32; 3], max: [f32; 3], mat: Material },
    /// Vertical cylinder from `base`, radius `r`, height `h`.
    Cylinder { base: [f32; 3], r: f32, h: f32, mat: Material },
    Sphere { centre: [f32; 3], r: f32, mat: Material },
    /// A prism whose cross-section is a triangle: the shape of a gable
    /// roof, a ramp, or a wedge buttress. Rises from `min` to a ridge at
    /// height `h`, ridged along `axis`.
    Wedge { min: [f32; 3], max: [f32; 3], h: f32, axis: Axis, mat: Material },
    /// Everything in `a` and `b`.
    Union(Vec<Solid>),
    /// `a` with `b` removed. The classic doorway.
    Diff(Box<Solid>, Box<Solid>),
    /// Only where both overlap — a roof clipped to its footprint.
    Intersect(Box<Solid>, Box<Solid>),
    /// A shell: the solid minus an inset copy of itself.
    Shell { inner: Box<Solid>, thickness: f32 },
    Translate { by: [f32; 3], inner: Box<Solid> },
    /// Reflect across a plane. "Pillars either side of" is one node.
    Mirror { axis: Axis, about: f32, inner: Box<Solid>, keep_original: bool },
    /// `count` copies, `step` apart. Five evenly spaced windows are one
    /// node rather than five ops the model has to place by hand.
    Array { count: u32, step: [f32; 3], inner: Box<Solid> },
    /// Nothing. The identity for a recipe that declines to add anything.
    Empty,
}

impl Solid {
    /// The material at a point, or None if the point is outside.
    ///
    /// Where solids overlap the highest `rank` wins, and ties go to the
    /// one declared later — so a recipe reads top to bottom like a list
    /// of refinements, but an explicit rank always beats declaration
    /// order.
    pub fn sample(&self, p: [f32; 3]) -> Option<&Material> {
        match self {
            Solid::Empty => None,
            Solid::Box { min, max, mat } => {
                let inside = (0..3).all(|i| p[i] >= min[i] && p[i] < max[i]);
                inside.then_some(mat)
            }
            Solid::Cylinder { base, r, h, mat } => {
                if p[1] < base[1] || p[1] >= base[1] + h { return None; }
                let (dx, dz) = (p[0] - base[0], p[2] - base[2]);
                (dx * dx + dz * dz <= r * r).then_some(mat)
            }
            Solid::Sphere { centre, r, mat } => {
                let d: f32 = (0..3).map(|i| (p[i] - centre[i]).powi(2)).sum();
                (d <= r * r).then_some(mat)
            }
            Solid::Wedge { min, max, h, axis, mat } => {
                if p[0] < min[0] || p[0] >= max[0] { return None; }
                if p[2] < min[2] || p[2] >= max[2] { return None; }
                if p[1] < min[1] { return None; }
                // Across the ridge, height falls linearly to the eaves.
                let (span_lo, span_hi, across) = match axis {
                    Axis::X => (min[2], max[2], p[2]),   // ridge runs along x
                    _ => (min[0], max[0], p[0]),
                };
                let half = (span_hi - span_lo) / 2.0;
                if half <= 0.0 { return None; }
                let centre = (span_lo + span_hi) / 2.0;
                let t = 1.0 - ((across - centre).abs() / half);
                (p[1] < min[1] + h * t).then_some(mat)
            }
            Solid::Union(parts) => {
                let mut best: Option<&Material> = None;
                for s in parts {
                    if let Some(m) = s.sample(p) {
                        if best.map(|b| m.rank >= b.rank).unwrap_or(true) { best = Some(m); }
                    }
                }
                best
            }
            Solid::Diff(a, b) => {
                let am = a.sample(p)?;
                match b.sample(p) {
                    // A cut only wins if it outranks what it cuts. This is
                    // what stops a low-priority scoop from eating a door
                    // frame that was declared as an accent.
                    Some(bm) if bm.rank >= am.rank => None,
                    _ => Some(am),
                }
            }
            Solid::Intersect(a, b) => {
                let am = a.sample(p)?;
                let bm = b.sample(p)?;
                Some(if bm.rank > am.rank { bm } else { am })
            }
            Solid::Shell { inner, thickness } => {
                let m = inner.sample(p)?;
                // On the shell if inside, but not inside the inset copy.
                // Probing along each axis is cheap and exact enough at
                // voxel scale; a true inset would need per-primitive
                // offsets and buys nothing here.
                let t = *thickness;
                let deep = [[t, 0.0, 0.0], [-t, 0.0, 0.0], [0.0, t, 0.0],
                            [0.0, -t, 0.0], [0.0, 0.0, t], [0.0, 0.0, -t]]
                    .iter()
                    .all(|d| inner.sample([p[0] + d[0], p[1] + d[1], p[2] + d[2]]).is_some());
                (!deep).then_some(m)
            }
            Solid::Translate { by, inner } =>
                inner.sample([p[0] - by[0], p[1] - by[1], p[2] - by[2]]),
            Solid::Mirror { axis, about, inner, keep_original } => {
                if *keep_original {
                    if let Some(m) = inner.sample(p) { return Some(m); }
                }
                let mut q = p;
                let i = *axis as usize;
                q[i] = 2.0 * about - p[i];
                inner.sample(q)
            }
            Solid::Array { count, step, inner } => {
                let mut best: Option<&Material> = None;
                for k in 0..*count {
                    let k = k as f32;
                    let q = [p[0] - step[0] * k, p[1] - step[1] * k, p[2] - step[2] * k];
                    if let Some(m) = inner.sample(q) {
                        if best.map(|b| m.rank >= b.rank).unwrap_or(true) { best = Some(m); }
                    }
                }
                best
            }
        }
    }

    /// Conservative integer bounds. Used to size the rasterization loop;
    /// generous is fine, wrong is not.
    pub fn bounds(&self) -> Option<([i32; 3], [i32; 3])> {
        fn merge(a: Option<([i32; 3], [i32; 3])>, b: Option<([i32; 3], [i32; 3])>)
            -> Option<([i32; 3], [i32; 3])> {
            match (a, b) {
                (Some((a0, a1)), Some((b0, b1))) => Some((
                    [a0[0].min(b0[0]), a0[1].min(b0[1]), a0[2].min(b0[2])],
                    [a1[0].max(b1[0]), a1[1].max(b1[1]), a1[2].max(b1[2])])),
                (x, None) => x,
                (None, y) => y,
            }
        }
        let f = |lo: [f32; 3], hi: [f32; 3]| Some((
            [lo[0].floor() as i32, lo[1].floor() as i32, lo[2].floor() as i32],
            [hi[0].ceil() as i32, hi[1].ceil() as i32, hi[2].ceil() as i32]));
        match self {
            Solid::Empty => None,
            Solid::Box { min, max, .. } => f(*min, *max),
            Solid::Cylinder { base, r, h, .. } =>
                f([base[0] - r, base[1], base[2] - r], [base[0] + r, base[1] + h, base[2] + r]),
            Solid::Sphere { centre, r, .. } =>
                f([centre[0] - r, centre[1] - r, centre[2] - r],
                  [centre[0] + r, centre[1] + r, centre[2] + r]),
            Solid::Wedge { min, max, h, .. } =>
                f(*min, [max[0], min[1] + h, max[2]]),
            Solid::Union(parts) => parts.iter().fold(None, |acc, s| merge(acc, s.bounds())),
            // A difference cannot be larger than what it subtracts from.
            Solid::Diff(a, _) => a.bounds(),
            Solid::Intersect(a, b) => match (a.bounds(), b.bounds()) {
                (Some((a0, a1)), Some((b0, b1))) => Some((
                    [a0[0].max(b0[0]), a0[1].max(b0[1]), a0[2].max(b0[2])],
                    [a1[0].min(b1[0]), a1[1].min(b1[1]), a1[2].min(b1[2])])),
                _ => None,
            },
            Solid::Shell { inner, .. } => inner.bounds(),
            Solid::Translate { by, inner } => inner.bounds().map(|(lo, hi)| (
                [lo[0] + by[0] as i32, lo[1] + by[1] as i32, lo[2] + by[2] as i32],
                [hi[0] + by[0] as i32, hi[1] + by[1] as i32, hi[2] + by[2] as i32])),
            Solid::Mirror { axis, about, inner, .. } => {
                let b = inner.bounds()?;
                let i = *axis as usize;
                let (mut lo, mut hi) = (b.0, b.1);
                let (a, c) = (2.0 * about - b.0[i] as f32, 2.0 * about - b.1[i] as f32);
                lo[i] = lo[i].min(a.floor() as i32).min(c.floor() as i32);
                hi[i] = hi[i].max(a.ceil() as i32).max(c.ceil() as i32);
                Some((lo, hi))
            }
            Solid::Array { count, step, inner } => {
                let (lo, hi) = inner.bounds()?;
                let k = (*count).saturating_sub(1) as f32;
                let d = [step[0] * k, step[1] * k, step[2] * k];
                Some(([lo[0].min(lo[0] + d[0] as i32), lo[1].min(lo[1] + d[1] as i32),
                       lo[2].min(lo[2] + d[2] as i32)],
                      [hi[0].max(hi[0] + d[0] as i32), hi[1].max(hi[1] + d[1] as i32),
                       hi[2].max(hi[2] + d[2] as i32)]))
            }
        }
    }
}

/// Where a voxel's sample is taken. Cell CENTRES, not corners: a box
/// from 0.0 to 3.0 should be three blocks, not four, and sampling
/// corners is how a wall ends up one thicker than asked for.
pub fn cell_centre(x: i32, y: i32, z: i32) -> [f32; 3] {
    [x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5]
}

/// Rasterize a solid into blocks. Air (block 0) from a carve is kept in
/// the output so the caller can apply it as a removal.
pub fn rasterize(s: &Solid) -> BTreeMap<(i32, i32, i32), u16> {
    let mut out = BTreeMap::new();
    let Some((lo, hi)) = s.bounds() else { return out };
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                if let Some(m) = s.sample(cell_centre(x, y, z)) {
                    out.insert((x, y, z), m.block);
                }
            }
        }
    }
    out
}

/// Cells on a material boundary: solid cells with at least one
/// horizontal neighbour that is empty or a different material.
///
/// This is how trim, edging and weathering work WITHOUT coordinates.
/// The model says "trim the edges in dark oak"; nothing has to know
/// where the edges are.
pub fn boundary_cells(cells: &BTreeMap<(i32, i32, i32), u16>) -> Vec<(i32, i32, i32)> {
    cells.iter()
        .filter(|((x, y, z), b)| {
            if **b == 0 { return false; }
            [(1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1)].iter().any(|(dx, dy, dz)| {
                match cells.get(&(x + dx, y + dy, z + dz)) {
                    None => true,
                    Some(n) => n != *b,
                }
            })
        })
        .map(|(k, _)| *k)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stone() -> Material { Material::structural(1) }

    #[test]
    fn a_box_is_the_size_you_asked_for() {
        // Sampling corners instead of centres makes every wall one
        // thicker than requested; this is the test that pins it.
        let s = Solid::Box { min: [0.0, 0.0, 0.0], max: [3.0, 1.0, 3.0], mat: stone() };
        let cells = rasterize(&s);
        assert_eq!(cells.len(), 9, "3x1x3 = 9 cells, not 16");
        assert!(cells.contains_key(&(0, 0, 0)) && cells.contains_key(&(2, 0, 2)));
        assert!(!cells.contains_key(&(3, 0, 0)), "max is exclusive");
    }

    #[test]
    fn a_wedge_is_a_gable_roof() {
        // The shape `roof gable` hardcoded. As a solid it is one node,
        // and it can be intersected, mirrored and arrayed like anything
        // else -- which the op could not.
        let s = Solid::Wedge { min: [0.0, 10.0, 0.0], max: [8.0, 0.0, 6.0],
                               h: 4.0, axis: Axis::X, mat: stone() };
        let cells = rasterize(&s);
        // tallest along the ridge line (z = 3), lowest at the eaves
        let ridge = cells.keys().filter(|(_, _, z)| *z == 3).map(|(_, y, _)| *y).max().unwrap();
        let eave = cells.keys().filter(|(_, _, z)| *z == 0).map(|(_, y, _)| *y).max().unwrap();
        assert!(ridge > eave, "ridge {ridge} must be above the eaves {eave}");
        // An EVEN span puts the true ridge line on a cell boundary, so no
        // cell sits exactly on it and the peak is one lower than h would
        // suggest: 12, not 13. This is the quantization the two-layer
        // split exists to make explicit — an op that computed integer
        // heights directly would have to pick a side silently and would
        // produce a lopsided roof.
        assert_eq!(ridge, 12, "even span: the ridge falls between cells");
        let both = cells.keys().filter(|(_, y, z)| *y == 12 && (*z == 2 || *z == 3)).count();
        assert_eq!(both, 16, "both cells flanking the ridge reach the same height");
    }

    #[test]
    fn an_odd_span_puts_a_cell_on_the_ridge() {
        // The companion case: an odd footprint has a true centre cell, so
        // the peak is a single line of blocks. Builders care about this —
        // it is the difference between a crisp ridge and a flat top.
        let s = Solid::Wedge { min: [0.0, 0.0, 0.0], max: [8.0, 0.0, 7.0],
                               h: 4.0, axis: Axis::X, mat: stone() };
        let cells = rasterize(&s);
        let top = cells.values().count();
        assert!(top > 0);
        let peak = cells.keys().map(|(_, y, _)| *y).max().unwrap();
        let peak_cells: Vec<i32> = cells.keys()
            .filter(|(_, y, _)| *y == peak).map(|(_, _, z)| *z).collect();
        let distinct: std::collections::BTreeSet<i32> = peak_cells.into_iter().collect();
        assert_eq!(distinct.len(), 1, "odd span: a single ridge line at z={distinct:?}");
    }

    #[test]
    fn a_carve_outranks_the_wall_it_cuts() {
        // The detail problem stated plainly: a door must survive the
        // wall, whatever order they were declared in.
        let wall = Solid::Box { min: [0.0, 0.0, 0.0], max: [5.0, 5.0, 1.0], mat: stone() };
        let door = Solid::Box { min: [2.0, 0.0, 0.0], max: [3.0, 3.0, 1.0],
                                mat: Material::carved() };
        let s = Solid::Diff(Box::new(wall), Box::new(door));
        let cells = rasterize(&s);
        assert!(!cells.contains_key(&(2, 1, 0)), "the doorway is open");
        assert!(cells.contains_key(&(1, 1, 0)), "the wall beside it stands");
        assert!(cells.contains_key(&(2, 4, 0)), "and the lintel above it");
    }

    #[test]
    fn a_low_rank_cut_cannot_eat_a_high_rank_detail() {
        // Ordering-independence is the point: a later scoop must not
        // remove an accent that outranks it.
        let trim = Solid::Box { min: [0.0, 0.0, 0.0], max: [4.0, 1.0, 1.0],
                                mat: Material::accent(58) };
        let scoop = Solid::Box { min: [1.0, 0.0, 0.0], max: [2.0, 1.0, 1.0],
                                 mat: Material::structural(0) };
        let s = Solid::Diff(Box::new(trim), Box::new(scoop));
        let cells = rasterize(&s);
        assert!(cells.contains_key(&(1, 0, 0)), "accent survives a structural cut");
    }

    #[test]
    fn five_windows_are_one_node() {
        // In the op world this is five fills the model has to space by
        // hand, and it gets the arithmetic wrong. Here it is an Array.
        let wall = Solid::Box { min: [0.0, 0.0, 0.0], max: [20.0, 5.0, 1.0], mat: stone() };
        let window = Solid::Box { min: [1.0, 2.0, 0.0], max: [2.0, 4.0, 1.0],
                                  mat: Material::carved() };
        let windows = Solid::Array { count: 5, step: [4.0, 0.0, 0.0], inner: Box::new(window) };
        let s = Solid::Diff(Box::new(wall), Box::new(windows));
        let cells = rasterize(&s);
        for k in 0..5 {
            let x = 1 + 4 * k;
            assert!(!cells.contains_key(&(x, 2, 0)), "window {k} at x={x} is open");
            assert!(cells.contains_key(&(x + 2, 2, 0)), "wall between windows stands");
        }
    }

    #[test]
    fn mirror_makes_a_pair_from_one_description() {
        let pillar = Solid::Cylinder { base: [3.0, 0.0, 0.0], r: 1.0, h: 5.0, mat: stone() };
        let pair = Solid::Mirror { axis: Axis::X, about: 0.0,
                                   inner: Box::new(pillar), keep_original: true };
        let cells = rasterize(&pair);
        assert!(cells.keys().any(|(x, _, _)| *x >= 2), "original stands");
        assert!(cells.keys().any(|(x, _, _)| *x <= -2), "and its reflection");
    }

    #[test]
    fn a_shell_is_hollow_but_closed() {
        let solid = Solid::Box { min: [0.0, 0.0, 0.0], max: [7.0, 7.0, 7.0], mat: stone() };
        let s = Solid::Shell { inner: Box::new(solid), thickness: 1.0 };
        let cells = rasterize(&s);
        assert!(cells.contains_key(&(0, 3, 3)), "wall present");
        assert!(cells.contains_key(&(3, 0, 3)), "floor present");
        assert!(cells.contains_key(&(3, 6, 3)), "roof present");
        assert!(!cells.contains_key(&(3, 3, 3)), "and hollow inside");
    }

    #[test]
    fn boundary_cells_find_trim_without_being_told_where() {
        // "trim the edges" with no coordinates: the shape knows.
        let s = Solid::Box { min: [0.0, 0.0, 0.0], max: [5.0, 1.0, 5.0], mat: stone() };
        let cells = rasterize(&s);
        let edge = boundary_cells(&cells);
        assert_eq!(edge.len(), 16, "a 5x5 slab has 16 perimeter cells");
        assert!(!edge.contains(&(2, 0, 2)), "the middle is not an edge");
        assert!(edge.contains(&(0, 0, 0)));
    }

    #[test]
    fn bounds_cover_everything_that_samples_inside() {
        // A bounds that is too small silently truncates a build, which
        // looks like a modelling bug and is not one.
        let s = Solid::Mirror {
            axis: Axis::X, about: 0.0, keep_original: true,
            inner: Box::new(Solid::Array {
                count: 3, step: [5.0, 0.0, 0.0],
                inner: Box::new(Solid::Box { min: [10.0, 0.0, 0.0], max: [11.0, 2.0, 1.0],
                                             mat: stone() }) }) };
        let (lo, hi) = s.bounds().unwrap();
        let cells = rasterize(&s);
        assert!(!cells.is_empty());
        for (x, y, z) in cells.keys() {
            assert!(*x >= lo[0] && *x <= hi[0] && *y >= lo[1] && *y <= hi[1]
                    && *z >= lo[2] && *z <= hi[2],
                    "cell ({x},{y},{z}) outside bounds {lo:?}..{hi:?}");
        }
    }
}
