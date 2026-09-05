//! Views: build a solid from 2D elevations, then check it against them.
//!
//! THE IDEA
//!
//! A model is bad at emitting 3D coordinates and comparatively good at
//! drawing a picture. So let it draw. A front elevation and a side
//! elevation, each a small ASCII grid, extruded along their view axis
//! and intersected, give a solid — which is how plan-and-elevation has
//! worked on paper for centuries.
//!
//! ```text
//!   front (looking along -Z)      side (looking along -X)
//!     ...#...                       ...#...
//!     ..###..                       ..###..
//!     .#####.                       .#####.
//!     #######                       #######
//!     .#####.                       .#####.
//!
//!   intersect the two extrusions -> an octahedral spire
//! ```
//!
//! This is the earlier ASCII-art idea turned the right way round. As an
//! OUTPUT format it would be a lossy 2D projection of a 3D problem. As
//! an INPUT it is the opposite: a lossless statement of a silhouette,
//! in the one notation a language model reliably produces.
//!
//! AND THE HONEST LIMIT
//!
//! Two views do not determine a solid. The intersection of extrusions
//! is the LARGEST shape consistent with both — the visual hull — and
//! for anything with a concavity hidden from both cameras it is too
//! large. An L-shaped floor plan extruded from a front and side view
//! comes back a full rectangle.
//!
//! Which is why the projection runs BOTH WAYS. `project` takes the
//! finished solid back to a mask, and `check_views` diffs it against
//! what was asked for. When the hull over-fills, you are told where,
//! rather than discovering it by looking at the build.

use crate::solid::{cell_centre, Material, Solid};

/// A 2D occupancy grid. Row 0 is the TOP row, as drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    cells: Vec<bool>,
}

impl Mask {
    pub fn new(w: usize, h: usize) -> Self {
        Self { w, h, cells: vec![false; w * h] }
    }

    /// Parse an ASCII block. Any of `#*XO+` is solid; anything else is
    /// empty. Ragged lines are padded, because a model will not keep
    /// its rows the same length and rejecting the drawing over that
    /// would waste the one thing it is good at.
    pub fn parse(art: &str) -> Self {
        let lines: Vec<&str> = art.lines()
            .map(|l| l.trim_end())
            .skip_while(|l| l.is_empty())
            .collect();
        let lines: Vec<&str> = {
            let mut v = lines;
            while v.last().is_some_and(|l| l.is_empty()) { v.pop(); }
            v
        };
        let h = lines.len();
        let w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        let mut m = Mask::new(w, h);
        for (row, line) in lines.iter().enumerate() {
            for (col, c) in line.chars().enumerate() {
                if matches!(c, '#' | '*' | 'X' | 'x' | 'O' | 'o' | '+') {
                    m.set(col, row, true);
                }
            }
        }
        m
    }

    pub fn get(&self, x: usize, y: usize) -> bool {
        if x >= self.w || y >= self.h { return false; }
        self.cells[y * self.w + x]
    }

    pub fn set(&mut self, x: usize, y: usize, v: bool) {
        if x < self.w && y < self.h { self.cells[y * self.w + x] = v; }
    }

    pub fn count(&self) -> usize { self.cells.iter().filter(|c| **c).count() }

    pub fn is_empty(&self) -> bool { self.count() == 0 }

    /// Render back to ASCII, for a diff a person can read.
    pub fn to_art(&self) -> String {
        (0..self.h).map(|y| {
            (0..self.w).map(|x| if self.get(x, y) { '#' } else { '.' }).collect::<String>()
        }).collect::<Vec<_>>().join("\n")
    }

    /// Cells in `self` that `other` lacks, and vice versa, as one
    /// picture: `#` both, `+` only here, `-` only there.
    pub fn diff_art(&self, other: &Mask) -> String {
        let w = self.w.max(other.w);
        let h = self.h.max(other.h);
        (0..h).map(|y| {
            (0..w).map(|x| match (self.get(x, y), other.get(x, y)) {
                (true, true) => '#',
                (true, false) => '+',
                (false, true) => '-',
                _ => '.',
            }).collect::<String>()
        }).collect::<Vec<_>>().join("\n")
    }
}

/// Which way a view looks. The mask's x is the first world axis named,
/// the mask's y is world Y counting DOWN from the top of the drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Looking along Z: mask x -> world X, mask y -> world Y.
    Front,
    /// Looking along X: mask x -> world Z, mask y -> world Y.
    Side,
    /// Looking down: mask x -> world X, mask y -> world Z. A floor plan.
    Top,
}

/// A mask swept along its view axis between two world coordinates.
#[derive(Debug, Clone)]
pub struct Extrusion {
    pub mask: Mask,
    pub view: View,
    /// World position of the mask's (0,0) corner.
    pub origin: [i32; 3],
    /// How far to sweep along the view axis, in blocks.
    pub depth: i32,
    pub mat: Material,
}

impl Extrusion {
    /// Does this extrusion contain a world cell?
    pub fn contains(&self, x: i32, y: i32, z: i32) -> bool {
        let (o, m) = (self.origin, &self.mask);
        let (mx, my, along) = match self.view {
            View::Front => (x - o[0], o[1] - y, z - o[2]),
            View::Side => (z - o[2], o[1] - y, x - o[0]),
            // A plan is drawn top-down: mask y increases southward.
            View::Top => (x - o[0], z - o[2], y - o[1]),
        };
        if along < 0 || along >= self.depth { return false; }
        if mx < 0 || my < 0 { return false; }
        m.get(mx as usize, my as usize)
    }

    pub fn bounds(&self) -> ([i32; 3], [i32; 3]) {
        let (o, m) = (self.origin, &self.mask);
        let (w, h) = (m.w as i32, m.h as i32);
        match self.view {
            View::Front => ([o[0], o[1] - h + 1, o[2]],
                            [o[0] + w - 1, o[1], o[2] + self.depth - 1]),
            View::Side => ([o[0], o[1] - h + 1, o[2]],
                           [o[0] + self.depth - 1, o[1], o[2] + w - 1]),
            View::Top => ([o[0], o[1], o[2]],
                          [o[0] + w - 1, o[1] + self.depth - 1, o[2] + h - 1]),
        }
    }
}

/// Intersect several extrusions: the visual hull of the views.
///
/// Returns the cells solid in EVERY view. With one view this is a plain
/// extrusion; with two it is the classic plan-and-elevation
/// construction; with three it is as tight as silhouettes can make it.
pub fn hull(views: &[Extrusion]) -> Vec<(i32, i32, i32)> {
    let Some(first) = views.first() else { return Vec::new() };
    let (mut lo, mut hi) = first.bounds();
    for v in &views[1..] {
        let (a, b) = v.bounds();
        for i in 0..3 { lo[i] = lo[i].max(a[i]); hi[i] = hi[i].min(b[i]); }
    }
    let mut out = Vec::new();
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                if views.iter().all(|v| v.contains(x, y, z)) {
                    out.push((x, y, z));
                }
            }
        }
    }
    out
}

/// Project a set of cells back to a mask, as seen from `view`.
///
/// This is the validation half. A silhouette that went in must come
/// back out; where it does not, the hull over-filled or the extrusion
/// missed, and `check_views` says which cells.
pub fn project(cells: &[(i32, i32, i32)], view: View, origin: [i32; 3],
                w: usize, h: usize) -> Mask {
    let mut m = Mask::new(w, h);
    for &(x, y, z) in cells {
        let (mx, my) = match view {
            View::Front => (x - origin[0], origin[1] - y),
            View::Side => (z - origin[2], origin[1] - y),
            View::Top => (x - origin[0], z - origin[2]),
        };
        if mx >= 0 && my >= 0 { m.set(mx as usize, my as usize, true); }
    }
    m
}

/// The result of checking a build against the views that made it.
#[derive(Debug, Clone)]
pub struct ViewCheck {
    pub view: View,
    /// Cells the drawing asked for that the solid does not have.
    pub missing: usize,
    /// Cells the solid has that the drawing did not ask for. This is the
    /// visual-hull over-fill, and it is the number that matters.
    pub extra: usize,
    /// A readable picture: `#` agreed, `+` asked-for and absent,
    /// `-` present and unasked.
    pub diff: String,
}

impl ViewCheck {
    pub fn ok(&self) -> bool { self.missing == 0 && self.extra == 0 }
}

/// Build from views and check the result against them.
///
/// The honest report. Two silhouettes cannot determine a solid, so this
/// says how far off the reconstruction is instead of pretending.
pub fn check_views(views: &[Extrusion]) -> (Vec<(i32, i32, i32)>, Vec<ViewCheck>) {
    let cells = hull(views);
    let checks = views.iter().map(|v| {
        let got = project(&cells, v.view, v.origin, v.mask.w, v.mask.h);
        let mut missing = 0;
        let mut extra = 0;
        for y in 0..v.mask.h {
            for x in 0..v.mask.w {
                match (v.mask.get(x, y), got.get(x, y)) {
                    (true, false) => missing += 1,
                    (false, true) => extra += 1,
                    _ => {}
                }
            }
        }
        ViewCheck { view: v.view, missing, extra, diff: v.mask.diff_art(&got) }
    }).collect();
    (cells, checks)
}

/// Project a rasterized SOLID to a mask, so a form from the dictionary
/// can be checked against a drawing, or shown as one.
pub fn project_solid(s: &Solid, view: View) -> Option<(Mask, [i32; 3])> {
    let (lo, hi) = s.bounds()?;
    let cells: Vec<(i32, i32, i32)> = (lo[0]..=hi[0])
        .flat_map(|x| (lo[1]..=hi[1]).flat_map(move |y| (lo[2]..=hi[2]).map(move |z| (x, y, z))))
        .filter(|&(x, y, z)| s.sample(cell_centre(x, y, z)).is_some_and(|m| m.block != 0))
        .collect();
    let (w, h, origin) = match view {
        View::Front => ((hi[0] - lo[0] + 1) as usize, (hi[1] - lo[1] + 1) as usize,
                        [lo[0], hi[1], lo[2]]),
        View::Side => ((hi[2] - lo[2] + 1) as usize, (hi[1] - lo[1] + 1) as usize,
                       [lo[0], hi[1], lo[2]]),
        View::Top => ((hi[0] - lo[0] + 1) as usize, (hi[2] - lo[2] + 1) as usize,
                      [lo[0], lo[1], lo[2]]),
    };
    Some((project(&cells, view, origin, w, h), origin))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ex(art: &str, view: View, origin: [i32; 3], depth: i32) -> Extrusion {
        Extrusion { mask: Mask::parse(art), view, origin, depth,
                    mat: Material::structural(1) }
    }

    #[test]
    fn ascii_parses_with_ragged_rows() {
        // A model will not keep its rows the same length; refusing the
        // drawing over that throws away the thing it is good at.
        let m = Mask::parse("\n  .#.\n  ###\n  #\n");
        assert_eq!(m.h, 3);
        assert_eq!(m.w, 5, "widest row wins");
        assert!(m.get(3, 0), "leading spaces are columns, not trimmed away");
        assert_eq!(m.count(), 5);
    }

    #[test]
    fn one_view_extrudes() {
        let e = ex("###\n#.#\n###", View::Front, [0, 10, 0], 4);
        let cells = hull(&[e]);
        assert_eq!(cells.len(), 8 * 4, "8 solid cells swept 4 deep");
        assert!(cells.contains(&(0, 10, 0)), "top-left of the drawing is at origin y");
        assert!(!cells.contains(&(1, 9, 0)), "the hole in the middle stays a hole");
    }

    #[test]
    fn two_views_intersect_into_a_spire() {
        // The plan-and-elevation construction: a diamond from the front
        // and a diamond from the side give an octahedron, which neither
        // view alone describes.
        let art = ".#.\n###\n.#.";
        let f = ex(art, View::Front, [0, 10, 0], 3);
        let s = ex(art, View::Side, [0, 10, 0], 3);
        let cells = hull(&[f, s]);
        assert!(cells.contains(&(1, 9, 1)), "the centre is solid");
        assert!(!cells.contains(&(0, 10, 0)), "the corner is cut by the other view");
        assert!(cells.len() < 9 * 3, "tighter than a single extrusion");
    }

    #[test]
    fn the_round_trip_confirms_a_shape_the_views_do_determine() {
        let art = "###\n###\n###";
        let f = ex(art, View::Front, [0, 10, 0], 3);
        let s = ex(art, View::Side, [0, 10, 0], 3);
        let (cells, checks) = check_views(&[f, s]);
        assert_eq!(cells.len(), 27, "a solid cube");
        for c in &checks {
            assert!(c.ok(), "{:?} disagreed:\n{}", c.view, c.diff);
        }
    }

    #[test]
    fn a_concavity_hidden_from_both_cameras_over_fills() {
        // THE honest limit, demonstrated the way it actually bites.
        //
        // Take a real L-shaped prism. Derive its front and side
        // silhouettes -- exactly what a model would draw from looking at
        // it. Reconstruct from those two views alone. The notch is
        // invisible in both silhouettes, so the hull fills it: you get a
        // full rectangle back.
        //
        // Adding the PLAN fixes it, which is why three views are worth
        // asking for and why `check_views` reports the gap instead of
        // letting you find it by looking at the build.
        let intended: Vec<(i32, i32, i32)> = (0..3).flat_map(|x| (0..3).flat_map(move |z| {
            // L: drop the far corner
            if x >= 1 && z >= 1 { None } else { Some((x, 0, z)) }
        })).collect();
        assert_eq!(intended.len(), 5, "an L, not a square");

        let front = project(&intended, View::Front, [0, 0, 0], 3, 1);
        let side = project(&intended, View::Side, [0, 0, 0], 3, 1);
        let cells = hull(&[
            Extrusion { mask: front, view: View::Front, origin: [0, 0, 0], depth: 3,
                        mat: Material::structural(1) },
            Extrusion { mask: side, view: View::Side, origin: [0, 0, 0], depth: 3,
                        mat: Material::structural(1) },
        ]);
        assert_eq!(cells.len(), 9, "two views reconstruct the full square, not the L");

        // With the plan as a third view, the notch survives.
        let plan = project(&intended, View::Top, [0, 0, 0], 3, 3);
        let front2 = project(&intended, View::Front, [0, 0, 0], 3, 1);
        let tight = hull(&[
            Extrusion { mask: plan.clone(), view: View::Top, origin: [0, 0, 0], depth: 1,
                        mat: Material::structural(1) },
            Extrusion { mask: front2, view: View::Front, origin: [0, 0, 0], depth: 3,
                        mat: Material::structural(1) },
        ]);
        assert_eq!(tight.len(), 5, "the plan restores the notch");
    }

    #[test]
    fn check_views_reports_where_a_drawing_was_not_honoured() {
        // Ask for something the hull cannot produce and read the diff.
        // A hollow ring seen from the front, swept, and checked from the
        // TOP against a plan that wanted a cross: the two disagree, and
        // the report shows exactly which cells.
        let ring = ex("###\n#.#\n###", View::Front, [0, 2, 0], 3);
        let cross_plan = Mask::parse(".#.\n###\n.#.");
        let cells = hull(&[ring]);
        let got = project(&cells, View::Top, [0, 0, 0], 3, 3);
        let diff = cross_plan.diff_art(&got);
        assert!(diff.contains('-'), "cells present but never asked for:\n{diff}");
        assert!(got.count() > cross_plan.count(), "the sweep is fuller than the plan");
    }

    #[test]
    fn three_views_are_tighter_than_two() {
        let diamond = ".#.\n###\n.#.";
        let two = hull(&[ex(diamond, View::Front, [0, 10, 0], 3),
                         ex(diamond, View::Side, [0, 10, 0], 3)]);
        let three = hull(&[ex(diamond, View::Front, [0, 10, 0], 3),
                           ex(diamond, View::Side, [0, 10, 0], 3),
                           ex(diamond, View::Top, [0, 8, 0], 3)]);
        assert!(three.len() <= two.len(), "a third silhouette can only remove cells");
        assert!(!three.is_empty());
    }

    #[test]
    fn a_dictionary_form_can_be_drawn_as_an_elevation() {
        // The loop closes: build a form, project it, and the picture is
        // something a person can check and a model can be shown.
        use crate::dict::{FormArgs, lookup};
        let a = FormArgs { min: (0, 0, 0), max: (10, 0, 8), style: "gable".into(),
                           block: 1, accent: 58, nums: vec![] };
        let solid = (lookup("roof").unwrap().build)(&a);
        let (mask, _) = project_solid(&solid, View::Side).expect("bounds");
        let art = mask.to_art();
        // A gable seen from the side is a triangle: the top row must be
        // narrower than the bottom.
        let rows: Vec<usize> = art.lines().map(|l| l.matches('#').count()).collect();
        let top = rows.iter().find(|n| **n > 0).copied().unwrap();
        let bottom = rows.iter().rev().find(|n| **n > 0).copied().unwrap();
        assert!(top < bottom, "a gable narrows toward the ridge:\n{art}");
    }

    #[test]
    fn an_empty_drawing_yields_nothing_and_does_not_panic() {
        let e = ex("...\n...", View::Front, [0, 0, 0], 3);
        assert!(e.mask.is_empty());
        assert!(hull(&[e]).is_empty());
        assert!(hull(&[]).is_empty());
    }
}
