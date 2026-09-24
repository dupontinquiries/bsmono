//! `sim::garment` — a cape, cloak or scarf: a `sim::cloth` sheet hung from an
//! attach line that moves with a body.
//!
//! The body says where the garment's top edge is each frame (`GarmentAnchor`:
//! its left and right top corners in world space, e.g. from
//! `hero_forge::build_rigged`); the top row of the sheet is pinned along that
//! line, the rest falls, swings and blows, pushed out of the body's capsules.
//!
//! A TELEPORT (respawn, a portal, a big correction) moves the attach line
//! further in one frame than any body can move; dragging the sheet across that
//! distance would stretch it into a streak, so the garment is re-hung at rest
//! at the new place instead.
//!
//! Pure: no clock, no GPU. The mesh is double-sided (both windings), since a
//! cape is seen from both sides.

use crate::sim::cloth::{Capsule, Cloth, ClothParams};
use crate::world::entity::EntityVertex;

/// Where a garment hangs this frame, in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GarmentAnchor {
    /// The top edge's two ends.
    pub left: [f32; 3],
    pub right: [f32; 3],
    /// How far it hangs, in blocks.
    pub length: f32,
    pub color: [f32; 3],
}

/// Farther than this (blocks) in one frame is a teleport, not motion.
pub const TELEPORT: f32 = 2.5;
/// Particles across the top edge.
const COLS: usize = 7;
/// At most this many rows, however long the garment.
const MAX_ROWS: usize = 20;

#[derive(Clone, Debug)]
pub struct Garment {
    pub cloth: Cloth,
    width: f32,
    length: f32,
    last_left: [f32; 3],
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn len(a: [f32; 3]) -> f32 { (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt() }
fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] { [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t] }

impl Garment {
    /// Hung at rest from `a`: straight down from the attach line.
    pub fn new(a: &GarmentAnchor) -> Garment {
        let width = len(sub(a.right, a.left)).max(0.02);
        // Columns spaced for the width, rows for the length (cells near-square).
        let spacing = width / (COLS - 1) as f32;
        let rows = ((a.length / spacing).ceil() as usize + 1).clamp(3, MAX_ROWS);
        let spacing_v = a.length / (rows - 1) as f32;
        let mut pos = Vec::with_capacity(COLS * rows);
        for r in 0..rows {
            for c in 0..COLS {
                let top = lerp(a.left, a.right, c as f32 / (COLS - 1) as f32);
                pos.push([top[0], top[1] - r as f32 * spacing_v, top[2]]);
            }
        }
        let mut cloth = Cloth::from_positions(COLS, rows, pos, ClothParams::default());
        for c in 0..COLS {
            let i = cloth.index(c, 0);
            cloth.pin(i);
        }
        Garment { cloth, width, length: a.length, last_left: a.left }
    }

    /// Move the top edge to `a` and advance `dt` in `wind` (world air velocity,
    /// blocks/s), outside `colliders`. A teleport, or a garment whose size
    /// changed (a different body), is re-hung at rest.
    pub fn drive(&mut self, a: &GarmentAnchor, colliders: &[Capsule], wind: [f32; 3], dt: f32) {
        let resized = (len(sub(a.right, a.left)) - self.width).abs() > 0.2 * self.width || (a.length - self.length).abs() > 0.2 * self.length;
        if len(sub(a.left, self.last_left)) > TELEPORT || resized {
            *self = Garment::new(a);
        }
        for c in 0..COLS {
            let i = self.cloth.index(c, 0);
            self.cloth.set_pin(i, lerp(a.left, a.right, c as f32 / (COLS - 1) as f32));
        }
        self.cloth.step(dt, wind, colliders);
        self.last_left = a.left;
    }

    /// Triangles for the entity pipeline, both windings. Faces tilted toward
    /// the sky are lit a little more than ones facing down, so folds read.
    pub fn mesh(&self, color: [f32; 3]) -> (Vec<EntityVertex>, Vec<u32>) {
        let mut v = Vec::new();
        let mut idx = Vec::new();
        for t in self.cloth.triangles() {
            let (p0, p1, p2) = (self.cloth.pos[t[0]], self.cloth.pos[t[1]], self.cloth.pos[t[2]]);
            let e1 = sub(p1, p0);
            let e2 = sub(p2, p0);
            let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
            let up = (n[1] / len(n).max(1e-9)).abs();
            let shade = 0.72 + 0.28 * up;
            let c = [color[0] * shade, color[1] * shade, color[2] * shade];
            let base = v.len() as u32;
            for p in [p0, p1, p2] {
                v.push(EntityVertex { position: p, color: c, alpha: 1.0 });
            }
            idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 1]);
        }
        (v, idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cape on a body facing +x: shoulders at y 1.5, the back at x = -0.15.
    fn anchor(at: [f32; 3]) -> GarmentAnchor {
        GarmentAnchor { left: [at[0] - 0.15, at[1] + 1.5, at[2] - 0.25], right: [at[0] - 0.15, at[1] + 1.5, at[2] + 0.25], length: 1.0, color: [0.7, 0.5, 0.2] }
    }
    fn torso(at: [f32; 3]) -> Capsule { Capsule { a: [at[0], at[1] + 0.7, at[2]], b: [at[0], at[1] + 1.4, at[2]], radius: 0.14 } }

    #[test]
    fn it_hangs_from_the_attach_line_down_the_back() {
        let at = [3.0, 10.0, -2.0];
        let mut g = Garment::new(&anchor(at));
        for _ in 0..240 { g.drive(&anchor(at), &[torso(at)], [0.0; 3], 1.0 / 60.0); }
        let a = anchor(at);
        for c in 0..COLS {
            let p = g.cloth.pos[g.cloth.index(c, 0)];
            let want = lerp(a.left, a.right, c as f32 / (COLS - 1) as f32);
            assert!(len(sub(p, want)) < 1e-6, "top edge on the attach line");
        }
        let hem = g.cloth.pos[g.cloth.index(COLS / 2, g.cloth.rows - 1)];
        assert!(hem[1] < a.left[1] - 0.85 * a.length, "hangs its length: hem y {:.2}", hem[1]);
        assert!(hem[0] <= a.left[0] + 0.01, "stays behind the back, not through the body (x {:.3})", hem[0]);
    }

    #[test]
    fn running_it_trails_behind_and_wind_blows_it() {
        let mut g = Garment::new(&anchor([0.0; 3]));
        // Run along +x at 5 blocks/s for 2 s.
        for i in 0..120 {
            let at = [i as f32 * 5.0 / 60.0, 0.0, 0.0];
            g.drive(&anchor(at), &[torso(at)], [0.0; 3], 1.0 / 60.0);
        }
        let at = [119.0 * 5.0 / 60.0, 0.0, 0.0];
        let hem = g.cloth.pos[g.cloth.index(COLS / 2, g.cloth.rows - 1)];
        assert!(hem[0] < anchor(at).left[0] - 0.2, "trails the runner: hem {:.2} vs shoulders {:.2}", hem[0], anchor(at).left[0]);
        // Standing in a HEADWIND (air moving toward -x at a body facing +x), the
        // cape is blown back off the body. (Wind along the attach line -- z here
        // -- meets the fabric edge-on, and the wind model rightly gives that no
        // push: a first version of this test asked for one.)
        let mut w = Garment::new(&anchor([0.0; 3]));
        for _ in 0..240 { w.drive(&anchor([0.0; 3]), &[torso([0.0; 3])], [-8.0, 0.0, 0.0], 1.0 / 60.0); }
        let hem = w.cloth.pos[w.cloth.index(COLS / 2, w.cloth.rows - 1)];
        let back = anchor([0.0; 3]).left[0];
        assert!(hem[0] < back - 0.2, "blown back off the body: hem x {:.2}, shoulders {:.2}", hem[0], back);
    }

    #[test]
    fn a_teleport_rehangs_it_instead_of_stretching_it_across_the_map() {
        let mut g = Garment::new(&anchor([0.0; 3]));
        for _ in 0..60 { g.drive(&anchor([0.0; 3]), &[], [0.0; 3], 1.0 / 60.0); }
        let far = [400.0, 30.0, -90.0];
        g.drive(&anchor(far), &[], [0.0; 3], 1.0 / 60.0);
        assert!(g.cloth.max_stretch() < 0.05, "stretch {:.3}", g.cloth.max_stretch());
        assert!(g.cloth.pos.iter().all(|p| len(sub(*p, far)) < 3.0), "all of it came along");
    }

    #[test]
    fn the_mesh_is_double_sided_and_indexes_what_it_emits() {
        let g = Garment::new(&anchor([0.0; 3]));
        let (v, i) = g.mesh([1.0, 0.0, 0.0]);
        assert_eq!(i.len(), g.cloth.triangles().len() * 6, "each triangle twice, both windings");
        assert!(i.iter().all(|&k| (k as usize) < v.len()));
        assert!(v.iter().all(|x| x.position.iter().all(|c| c.is_finite())));
    }
}
