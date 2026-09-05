//! Detailing: the passes that make a build look made rather than
//! generated.
//!
//! These are the finishing tricks the corpus keeps naming — "break up"
//! 20 times, "detailing" 14, "protruding"/"inset" 6, "scattered around
//! the foot" 5 — and none of them is a shape. They are all
//! probabilistic transforms over blocks that already exist, which is
//! why they live here beside `weather` rather than in the dictionary.
//!
//! What they have in common is that a flat, uniform surface is the
//! signature of generated work. Every pass here attacks that:
//!
//! * `speckle` breaks a single-material face into a mix.
//! * `relief` pushes and pulls wall cells so the surface has depth.
//! * `trim` accents the edges, which is where the eye goes.
//! * `feather` replaces a hard material boundary with a probabilistic
//!   ramp — the thing that makes two regions look like they met rather
//!   than like one was pasted over the other.
//! * `ground_blend` scatters the structure's material around its foot,
//!   so it looks settled rather than stamped down.
//!
//! All are seeded and deterministic, and all return
//! [`weather::Change`], so they share `apply_changes`, preview and
//! single-unit undo with everything else.

use crate::weather::Change;
use crate::VoxelWorld;

fn hash3(x: i32, y: i32, z: i32, seed: u64) -> u64 {
    let mut h = seed
        ^ (x as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (z as i64 as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 31;
    h = h.wrapping_mul(0x7FB5_D329_728E_A185);
    h ^ (h >> 27)
}

fn roll(x: i32, y: i32, z: i32, seed: u64, salt: u64) -> f32 {
    (hash3(x, y, z, seed ^ salt) % 10_000) as f32 / 10_000.0
}

fn norm(min: (i32, i32, i32), max: (i32, i32, i32))
    -> ((i32, i32, i32), (i32, i32, i32)) {
    ((min.0.min(max.0), min.1.min(max.1), min.2.min(max.2)),
     (min.0.max(max.0), min.1.max(max.1), min.2.max(max.2)))
}

/// Is this cell visible from outside? Only surfaces need detailing;
/// changing buried blocks is invisible work that still costs an edit
/// and still has to be undone.
fn exposed<W: VoxelWorld>(w: &W, x: i32, y: i32, z: i32) -> bool {
    [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)]
        .iter()
        .any(|(dx, dy, dz)| w.get(x + dx, y + dy, z + dz) == 0)
}

/// Break a uniform surface into a mix.
///
/// "Break up the dead space" is the corpus's own phrase and it is the
/// most-repeated detailing note in it. A wall of one block reads as a
/// fill command; the same wall with a tenth of its faces swapped for
/// two related blocks reads as masonry.
///
/// `amount` is the fraction of EXPOSED matching cells replaced.
/// Interior cells are left alone: nobody sees them.
pub fn speckle<W: VoxelWorld>(w: &W, min: (i32, i32, i32), max: (i32, i32, i32),
                              target: u16, variants: &[u16], amount: f32, seed: u64)
    -> Vec<Change> {
    if variants.is_empty() { return Vec::new(); }
    let (lo, hi) = norm(min, max);
    let mut out = Vec::new();
    for x in lo.0..=hi.0 {
        for y in lo.1..=hi.1 {
            for z in lo.2..=hi.2 {
                let b = w.get(x, y, z);
                if b == 0 || (target != 0 && b != target) { continue; }
                if !exposed(w, x, y, z) { continue; }
                if roll(x, y, z, seed, 11) >= amount { continue; }
                let pick = (hash3(x, y, z, seed ^ 12) % variants.len() as u64) as usize;
                if variants[pick] != b {
                    out.push(Change { at: (x, y, z), to: variants[pick] });
                }
            }
        }
    }
    out
}

/// Give a flat wall depth by pushing cells out.
///
/// "A bit more complex detailing to the walls" — a face that is exactly
/// planar reads as a fill. This adds a block in front of a fraction of
/// the exposed faces, so the surface has relief a shadow can catch.
///
/// Pushing OUT rather than carving in, deliberately: carving into a
/// wall can open a hole through a one-block shell, and a structure you
/// can see daylight through is worse than a flat one.
pub fn relief<W: VoxelWorld>(w: &W, min: (i32, i32, i32), max: (i32, i32, i32),
                             amount: f32, seed: u64) -> Vec<Change> {
    let (lo, hi) = norm(min, max);
    let mut out = Vec::new();
    for x in lo.0..=hi.0 {
        for y in lo.1..=hi.1 {
            for z in lo.2..=hi.2 {
                let b = w.get(x, y, z);
                if b == 0 { continue; }
                if roll(x, y, z, seed, 21) >= amount { continue; }
                // Only sideways: pushing up makes lumps on a roof and
                // pushing down leaves blocks hanging under a floor.
                for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    if w.get(x + dx, y, z + dz) == 0 {
                        out.push(Change { at: (x + dx, y, z + dz), to: b });
                        break;
                    }
                }
            }
        }
    }
    out
}

/// Accent the edges.
///
/// The eye goes to boundaries, so a different block along them does
/// more for a build than anything applied to the middle of a face. No
/// coordinates needed: a cell is an edge if a horizontal neighbour is
/// empty or a different material.
pub fn trim<W: VoxelWorld>(w: &W, min: (i32, i32, i32), max: (i32, i32, i32),
                           accent: u16, amount: f32, seed: u64) -> Vec<Change> {
    let (lo, hi) = norm(min, max);
    let mut out = Vec::new();
    for x in lo.0..=hi.0 {
        for y in lo.1..=hi.1 {
            for z in lo.2..=hi.2 {
                let b = w.get(x, y, z);
                if b == 0 || b == accent { continue; }
                // COUNT like neighbours rather than asking "is any
                // neighbour different". On a one-block-thick wall every
                // cell has air in front and behind, so the any-test
                // called the entire wall an edge and repainted 90% of
                // it. What makes a cell an edge is being connected to
                // FEWER of its own kind than a cell in the middle of a
                // face: an interior face cell keeps four, a perimeter
                // cell three, a corner two.
                let like = [(1, 0, 0), (-1, 0, 0), (0, 1, 0),
                            (0, -1, 0), (0, 0, 1), (0, 0, -1)]
                    .iter()
                    .filter(|(dx, dy, dz)| w.get(x + dx, y + dy, z + dz) == b)
                    .count();
                if like <= 3 && roll(x, y, z, seed, 31) < amount {
                    out.push(Change { at: (x, y, z), to: accent });
                }
            }
        }
    }
    out
}

/// Feather a hard boundary between two materials into a ramp.
///
/// THE one you asked for by name. Where a build meets terrain, or one
/// biome meets another, a clean line is the giveaway. Feathering
/// replaces it with a probability gradient: near the boundary almost
/// every cell converts, and the chance falls to nothing over `width`.
/// The result interleaves, so the two regions look like they met.
///
/// `axis`: 0 = x, 2 = z, 1 = y (a vertical fade, for a wall that should
/// grow out of the ground).
pub fn feather<W: VoxelWorld>(w: &W, min: (i32, i32, i32), max: (i32, i32, i32),
                              axis: usize, from_coord: i32, width: i32,
                              into: u16, only: Option<u16>, seed: u64) -> Vec<Change> {
    let (lo, hi) = norm(min, max);
    let width = width.max(1);
    let mut out = Vec::new();
    for x in lo.0..=hi.0 {
        for y in lo.1..=hi.1 {
            for z in lo.2..=hi.2 {
                let b = w.get(x, y, z);
                if b == 0 || b == into { continue; }
                if let Some(t) = only { if b != t { continue; } }
                let c = [x, y, z][axis.min(2)];
                let d = (c - from_coord).abs();
                if d > width { continue; }
                // Linear falloff: 1.0 at the boundary, 0.0 at the edge
                // of the band. A step function here would just move the
                // hard line, which is the whole thing being fixed.
                let p = 1.0 - (d as f32 / width as f32);
                if roll(x, y, z, seed, 41) < p {
                    out.push(Change { at: (x, y, z), to: into });
                }
            }
        }
    }
    out
}

/// Scatter a structure's material around its foot.
///
/// A building whose walls meet flat ground on an exact line looks
/// stamped down. Real ones have spill: fallen stones, a worn apron,
/// grass growing over the base. This drops loose blocks in a ring
/// outside the footprint, thinning with distance.
pub fn ground_blend<W: VoxelWorld>(w: &W, min: (i32, i32), max: (i32, i32),
                                   ground_y: i32, block: u16, reach: i32,
                                   amount: f32, seed: u64) -> Vec<Change> {
    let (x0, x1) = (min.0.min(max.0), min.0.max(max.0));
    let (z0, z1) = (min.1.min(max.1), min.1.max(max.1));
    let reach = reach.max(1);
    let mut out = Vec::new();
    for x in (x0 - reach)..=(x1 + reach) {
        for z in (z0 - reach)..=(z1 + reach) {
            let inside = x >= x0 && x <= x1 && z >= z0 && z <= z1;
            if inside { continue; }
            // Distance outside the footprint, in the Chebyshev sense:
            // a ring, not a circle, because the footprint is a box.
            let dx = if x < x0 { x0 - x } else if x > x1 { x - x1 } else { 0 };
            let dz = if z < z0 { z0 - z } else if z > z1 { z - z1 } else { 0 };
            let d = dx.max(dz);
            if d > reach { continue; }
            let p = amount * (1.0 - (d - 1) as f32 / reach as f32);
            if roll(x, ground_y, z, seed, 51) >= p { continue; }
            // Only onto open ground: spill lands on the surface, never
            // inside something that is already there.
            if w.get(x, ground_y + 1, z) == 0 && w.get(x, ground_y, z) != 0 {
                out.push(Change { at: (x, ground_y + 1, z), to: block });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct W { cells: BTreeMap<(i32, i32, i32), u16>, ground: i32 }
    impl W {
        fn flat() -> Self { W { cells: BTreeMap::new(), ground: 64 } }
        /// A solid 9x9 stone wall block, 6 high, on flat ground.
        fn wall() -> Self {
            let mut w = W::flat();
            for x in 0..9 { for y in 65..71 { for z in 0..9 {
                if x == 0 || x == 8 || z == 0 || z == 8 { w.cells.insert((x, y, z), 1); }
            }}}
            w
        }
    }
    impl VoxelWorld for W {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.cells.get(&(x, y, z)) { return b; }
            if y <= self.ground { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool {
            self.cells.insert((x, y, z), b); true
        }
    }

    #[test]
    fn speckle_only_touches_faces_anyone_can_see() {
        // Detailing a buried block is invisible work that still costs an
        // edit and still has to be undone.
        let mut w = W::flat();
        for x in 0..5 { for y in 65..70 { for z in 0..5 { w.set(x, y, z, 1); } } }
        let ch = speckle(&w, (0, 65, 0), (4, 69, 4), 1, &[4, 58], 1.0, 7);
        assert!(!ch.is_empty());
        for c in &ch {
            assert!(exposed(&w, c.at.0, c.at.1, c.at.2),
                    "{:?} is buried and should not have been touched", c.at);
        }
        assert!(!ch.iter().any(|c| c.at == (2, 67, 2)), "the core is untouched");
    }

    #[test]
    fn speckle_produces_a_mix_not_a_repaint() {
        let w = W::wall();
        let ch = speckle(&w, (0, 65, 0), (8, 70, 8), 1, &[4, 58], 0.3, 3);
        let kinds: std::collections::BTreeSet<u16> = ch.iter().map(|c| c.to).collect();
        assert!(kinds.len() >= 2, "a mix, not one substitute: {kinds:?}");
        // and a minority: 0.3 means most of the wall stays as it was
        let exposed_cells = (0..9).flat_map(|x| (65..71).flat_map(move |y| (0..9)
            .map(move |z| (x, y, z))))
            .filter(|&(x, y, z)| w.get(x, y, z) == 1 && exposed(&w, x, y, z))
            .count();
        assert!(ch.len() * 2 < exposed_cells,
                "{} of {exposed_cells} changed: too much", ch.len());
    }

    #[test]
    fn relief_pushes_out_and_never_opens_a_hole() {
        // Carving into a one-block shell would let daylight through,
        // which is worse than a flat wall.
        let w = W::wall();
        let ch = relief(&w, (0, 65, 0), (8, 70, 8), 0.4, 5);
        assert!(!ch.is_empty());
        for c in &ch {
            assert_ne!(c.to, 0, "relief never removes");
            assert_eq!(w.get(c.at.0, c.at.1, c.at.2), 0,
                       "it fills air, so nothing existing is disturbed");
        }
    }

    #[test]
    fn trim_lands_on_edges_and_tops_not_in_the_middle_of_a_face() {
        let mut w = W::flat();
        // a 7x7 slab: only its perimeter is an edge
        for x in 0..7 { for z in 0..7 { w.set(x, 65, z, 1); } }
        let ch = trim(&w, (0, 65, 0), (6, 65, 6), 58, 1.0, 9);
        // The perimeter of the slab has fewer like neighbours than its
        // middle, so trim lands there and NOT in the centre.
        for x in [0, 6] { for z in [0, 6] {
            assert!(ch.iter().any(|c| c.at == (x, 65, z)), "corner ({x},65,{z}) trimmed");
        }}
        assert!(!ch.iter().any(|c| c.at == (3, 65, 3)),
                "the middle of the face is not an edge");
        assert!(ch.len() < 7 * 7, "trim is a minority of the surface, not a repaint");
    }

    #[test]
    fn trim_on_a_thin_wall_stays_a_minority() {
        // The regression: a one-block-thick wall has air on both faces,
        // so an any-different test made every cell an "edge" and trim at
        // 0.9 repainted the whole thing.
        let mut w = W::flat();
        for x in 0..20 { for y in 65..72 { w.set(x, y, 5, 1); } }
        let ch = trim(&w, (0, 65, 5), (19, 71, 5), 58, 0.9, 4);
        let total = 20 * 7;
        assert!(ch.len() * 3 < total,
                "{} of {total} repainted -- trim should follow edges, not cover the face",
                ch.len());
        // the border of the wall IS trimmed
        assert!(ch.iter().any(|c| c.at.1 == 71), "the top course is an edge");
    }

    #[test]
    fn feather_is_a_ramp_not_a_moved_line() {
        // The whole point: a step function would just relocate the hard
        // edge. Density must FALL with distance from the boundary.
        let mut w = W::flat();
        for x in 0..40 { for z in 0..40 { w.set(x, 65, z, 1); } }
        let ch = feather(&w, (0, 65, 0), (39, 65, 39), 0, 0, 12, 12, Some(1), 4);
        let band = |a: i32, b: i32| ch.iter()
            .filter(|c| c.at.0 >= a && c.at.0 < b).count();
        let (near, mid, far) = (band(0, 4), band(4, 8), band(8, 12));
        assert!(near > mid && mid > far,
                "density must fall with distance: {near} > {mid} > {far}");
        assert_eq!(band(13, 40), 0, "and stop at the band edge");
    }

    #[test]
    fn feather_can_be_restricted_to_one_material() {
        let mut w = W::flat();
        for x in 0..10 { w.set(x, 65, 0, 1); w.set(x, 65, 1, 58); }
        let ch = feather(&w, (0, 65, 0), (9, 65, 1), 0, 0, 9, 12, Some(1), 2);
        for c in &ch {
            assert_eq!(w.get(c.at.0, c.at.1, c.at.2), 1,
                       "only the named material converts; the accent row is left alone");
        }
    }

    #[test]
    fn ground_blend_spills_outside_and_thins_with_distance() {
        let w = W::flat();
        let ch = ground_blend(&w, (10, 10), (20, 20), 64, 4, 4, 0.9, 6);
        assert!(!ch.is_empty());
        for c in &ch {
            let inside = c.at.0 >= 10 && c.at.0 <= 20 && c.at.2 >= 10 && c.at.2 <= 20;
            assert!(!inside, "spill goes OUTSIDE the footprint, not over it");
            assert_eq!(c.at.1, 65, "and sits on the ground surface");
        }
        let ring = |d: i32| ch.iter().filter(|c| {
            let dx = (10 - c.at.0).max(c.at.0 - 20).max(0);
            let dz = (10 - c.at.2).max(c.at.2 - 20).max(0);
            dx.max(dz) == d
        }).count();
        assert!(ring(1) > ring(4), "thins outward: {} at d=1 vs {} at d=4", ring(1), ring(4));
    }

    #[test]
    fn every_pass_is_deterministic_from_its_seed() {
        // Preview and commit must agree, the same reason scatter and
        // weather are seeded.
        let w = W::wall();
        let a = speckle(&w, (0, 65, 0), (8, 70, 8), 1, &[4, 58], 0.3, 3);
        assert_eq!(a, speckle(&w, (0, 65, 0), (8, 70, 8), 1, &[4, 58], 0.3, 3));
        assert_ne!(a, speckle(&w, (0, 65, 0), (8, 70, 8), 1, &[4, 58], 0.3, 4));
        let t = trim(&w, (0, 65, 0), (8, 70, 8), 58, 0.5, 1);
        assert_eq!(t, trim(&w, (0, 65, 0), (8, 70, 8), 58, 0.5, 1));
    }

    #[test]
    fn zero_amount_changes_nothing_anywhere() {
        let w = W::wall();
        assert!(speckle(&w, (0, 65, 0), (8, 70, 8), 1, &[4], 0.0, 1).is_empty());
        assert!(relief(&w, (0, 65, 0), (8, 70, 8), 0.0, 1).is_empty());
        assert!(trim(&w, (0, 65, 0), (8, 70, 8), 58, 0.0, 1).is_empty());
        assert!(ground_blend(&w, (0, 0), (8, 8), 64, 4, 3, 0.0, 1).is_empty());
    }
}
