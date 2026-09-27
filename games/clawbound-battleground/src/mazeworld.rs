//! mazeworld — what stands in the Thornmaze's rooms, its gates, and where
//! its thorns grow.
//!
//! `maze` is the ground; this is the dressing, for `scene` to draw and
//! `Blockers` to collide against. Placements are data ([`placements`],
//! tested without a GPU); [`build`] turns them into meshes and collision.
//!
//! * the Heart: a seated stone cat on a plinth, the labyrinth's centre;
//! * the lane rooms by the bases: colonnades, two rows of pillars to fight
//!   round; the lane rooms in the middle: broken wall stubs for cover;
//! * braziers round the beasts' pits, and on the corners of each lane's
//!   turns, so a lane reads as a lane from inside;
//! * the GATES: every passage gets a slab of stone across its middle
//!   ([`gate_slab`]) -- in `Blockers` as a gate (`push_gate`, in the
//!   passages' order, so gate `g`'s wall is passage `g`'s; a lane's never
//!   shuts), drawn by `scene` at the height the clock says.
//!
//! And [`thorn_spots`]: where the thorns rise along the walls when a
//! section wakes, by the section's class.

use glam::Vec3;

use crate::blockers::{Blocker, Blockers, Cover, Shape};
use crate::consts::{Lane, Team};
use crate::maze::{self as mz, Room, PASSAGE_R, WALL_H};
use crate::scene::{Piece, V_MED};
use crate::voxel::{Grid, VoxelMesh};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Item {
    /// The seated cat at the Heart, facing `yaw`.
    Statue { at: Vec3, yaw: f32 },
    Brazier { at: Vec3 },
    Pillar { at: Vec3, variant: i32 },
    /// A broken stretch of wall from `a` to `b`: cover.
    Stub { a: Vec3, b: Vec3 },
}

pub fn item_at(it: &Item) -> Vec3 {
    match *it {
        Item::Statue { at, .. } | Item::Brazier { at } | Item::Pillar { at, .. } => at,
        Item::Stub { a, b } => (a + b) * 0.5,
    }
}

fn flip(it: Item) -> Item {
    let f = |p: Vec3| Vec3::new(p.x, p.y, -p.z);
    match it {
        Item::Statue { at, yaw } => Item::Statue { at: f(at), yaw: std::f32::consts::PI - yaw },
        Item::Brazier { at } => Item::Brazier { at: f(at) },
        Item::Pillar { at, variant } => Item::Pillar { at: f(at), variant },
        Item::Stub { a, b } => Item::Stub { a: f(a), b: f(b) },
    }
}

/// Everything placed in the Thornmaze. Computed AS that map whatever is
/// active.
pub fn placements() -> Vec<Item> {
    crate::map::with(crate::map::MapId::Maze, placements_here)
}

fn placements_here() -> Vec<Item> {
    let mut out = Vec::new();
    let both = |out: &mut Vec<Item>, it: Item| {
        let on_axis = item_at(&it).z.abs() < 0.5;
        out.push(it);
        if !on_axis {
            out.push(flip(it));
        }
    };
    let at = |rm: Room, dx: f32, dz: f32| mz::room_pos(rm) + Vec3::new(dx, 0.0, dz);
    // the Heart's cat, facing the blue side (its twin would stand on it)
    out.push(Item::Statue { at: Vec3::ZERO, yaw: std::f32::consts::PI });
    // colonnades in the lane rooms by the bases: two rows of pillars along
    // the room's long sides, clear of the lane's line through it
    // (a pillar the lane would walk into is left out)
    let lane_d = |p: Vec3| {
        [Lane::Top, Lane::Bot].iter().map(|l| mz::lane_polyline(*l).windows(2).map(|w| crate::abilities::point_segment_dist(p, w[0], w[1])).fold(f32::MAX, f32::min)).fold(f32::MAX, f32::min)
    };
    for c in [0usize, 2] {
        for dx in [-20.0f32, -8.0, 8.0, 20.0] {
            for dz in [-13.0f32, 13.0] {
                let p = at((c, -2), dx, dz);
                if lane_d(p) > 4.5 {
                    both(&mut out, Item::Pillar { at: p, variant: ((dx + dz) as i32).rem_euclid(4) });
                }
            }
        }
    }
    // wall stubs in the lanes' middle rooms and the heart row's side rooms:
    // cover, set in the corners
    // (in the room's long sides and round ends, between the passages'
    // mouths -- the corners are all near one -- and clear of the lanes)
    let mouth_d = |p: Vec3| mz::passages().iter().flat_map(|g| [g.p0, g.p1]).map(|m| m.distance(p)).fold(f32::MAX, f32::min);
    for rm in [(0usize, -1i32), (2, -1), (0, 0), (2, 0)] {
        let spots: [(f32, f32, bool); 6] = [(-4.0, -14.0, true), (4.0, -14.0, true), (-4.0, 14.0, true), (4.0, 14.0, true), (-24.0, 0.0, false), (24.0, 0.0, false)];
        for (dx, dz, along_x) in spots {
            // the heart row's rooms are their own mirror: their north
            // spots come as the mirrors of the south ones
            if rm.1 == 0 && dz > 0.0 {
                continue;
            }
            let c = at(rm, dx, dz);
            let h = if along_x { Vec3::new(4.0, 0.0, 0.0) } else { Vec3::new(0.0, 0.0, 4.0) };
            let (a, b) = (c - h, c + h);
            let spring_d = |p: Vec3| mz::heal_tree_positions().iter().map(|s| s.distance(p)).fold(f32::MAX, f32::min);
            if [a, b, c].iter().all(|&p| lane_d(p) > 5.0 && mouth_d(p) > 6.0 && spring_d(p) > 6.0) {
                both(&mut out, Item::Stub { a, b });
            }
        }
    }
    // braziers round the pit (blue; mirrored), on its diagonals
    let pit = mz::pit_positions()[0];
    for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        both(&mut out, Item::Brazier { at: pit + Vec3::new(sx * 22.0, 0.0, sz * 14.0) });
    }
    // and on the outer corner of each lane's turns (blue half)
    for lane in [Lane::Top, Lane::Bot] {
        let pts = mz::lane_polyline(lane);
        for w in pts.windows(3) {
            let (a, c, e) = (w[0], w[1], w[2]);
            if c.z > -0.5 {
                continue;
            }
            let (u, v) = ((a - c).normalize_or_zero(), (e - c).normalize_or_zero());
            if u.dot(v) > -0.9 {
                let out_corner = -(u + v).normalize_or_zero();
                both(&mut out, Item::Brazier { at: c + out_corner * 9.0 });
            }
        }
    }
    out
}

/// A gate's slab: a wall of stone across passage `g`'s middle, into the
/// walls either side. Its collision, as `Blockers` holds it.
pub fn gate_slab(g: usize) -> Blocker {
    let p = mz::passages()[g];
    let (m, d) = (p.mid(), p.dir());
    let side = Vec3::new(-d.z, 0.0, d.x) * (PASSAGE_R + 1.5);
    let (a, b) = (m - side, m + side);
    Blocker { shape: Shape::Segment { x0: a.x, z0: a.z, x1: b.x, z1: b.z, r: 1.1 }, top: WALL_H, cover: Cover::NONE }
}

/// The slab's mesh: a dressed stone door, `(PASSAGE_R + 1.5) * 2` wide and
/// WALL_H tall, built across x (the scene turns it across its passage),
/// feet at y = 0; a band of thorns over its top.
pub fn gate_mesh() -> VoxelMesh {
    const V: f32 = 0.5;
    let w = (((PASSAGE_R + 1.5) * 2.0) / V) as i32;
    let h = (WALL_H / V) as i32;
    let mut g = Grid::new(w as usize + 2, h as usize + 2, 6);
    let stone = [0.40, 0.38, 0.35, 1.0];
    let dark = [0.24, 0.22, 0.20, 1.0];
    let thorn = [0.10, 0.12, 0.07, 1.0];
    g.filled_box(1, 0, 1, w, h - 1, 4, stone);
    for y in (3..h).step_by(4) {
        g.filled_box(1, y, 1, w, y, 4, dark);
    }
    for x in (2..w).step_by(3) {
        g.filled_box(x, h - 3 - (x % 2), 0, x, h - 2, 5, thorn);
    }
    g.mesh(V, true)
}

/// A clump of thorns: dark stems bending out and up from one root, spiked,
/// with a few red berries. ~1.8 u tall.
pub fn thorn_mesh(variant: i32) -> VoxelMesh {
    let mut g = Grid::new(20, 18, 20);
    let stem = [0.10, 0.09, 0.07, 1.0];
    let dark = [0.05, 0.07, 0.04, 1.0];
    let berry = [0.62, 0.07, 0.10, 1.0];
    let n = 6 + variant.rem_euclid(3);
    for k in 0..n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU + variant as f32 * 0.7;
        let (dx, dz) = (a.cos(), a.sin());
        let lean = 0.35 + 0.08 * (k * 5 + variant).rem_euclid(4) as f32;
        let top = 12 + (k * 3 + variant).rem_euclid(5);
        for y in 0..top {
            let r = y as f32 * lean;
            let (x, z) = ((10.0 + dx * r).round() as i32, (10.0 + dz * r).round() as i32);
            g.set(x, y, z, if y % 3 == 0 { dark } else { stem });
            if y % 3 == 2 {
                g.set(x + (dz.signum() as i32), y + 1, z - (dx.signum() as i32), stem);
            }
            if y + 1 == top && (k + variant).rem_euclid(3) == 0 {
                g.set(x, y + 1, z, berry);
            }
        }
    }
    g.mesh(0.12, true)
}

/// Where thorns rise when a section wakes: along the foot of every room's
/// walls (and its passages'), every few units, by the section's class.
/// `(class, position, yaw)`.
pub fn thorn_spots() -> Vec<(u8, Vec3, f32)> {
    crate::map::with(crate::map::MapId::Maze, || {
        let f = mz::floor();
        let mut out = Vec::new();
        let (hx, hz) = (mz::COLS[2] + mz::ROOM_HX + 4.0, mz::ROWS as f32 * mz::ROW_Z + mz::ROOM_HZ + 4.0);
        // a fine sweep for the band at the walls' foot, then one clump per
        // 3 u cell (a coarse sweep mostly missed a band a unit wide)
        let step = 0.7;
        let mut taken = std::collections::HashSet::new();
        let mut z = -hz;
        while z <= hz {
            let mut x = -hx;
            while x <= hx {
                let d = f.sdf(x, z);
                if (-1.8..=-0.8).contains(&d) && taken.insert(((x / 3.0).floor() as i32, (z / 3.0).floor() as i32)) {
                    if let Some(s) = mz::section_at(x, z) {
                        out.push((s.class(), Vec3::new(x, 0.0, z), (x * 0.37 + z * 0.61).sin() * 3.0));
                    }
                }
                x += step;
            }
            z += step;
        }
        out
    })
}

/// Meshes and collision for everything in [`placements`], and the gates'
/// walls (gate `g` is passage `g`).
pub fn build() -> (Vec<Piece>, Blockers) {
    let mut blockers = Blockers::new(crate::consts::arena_radius() + 20.0, 6.0);
    blockers.set_floor(mz::floor());
    for g in 0..mz::passages().len() {
        let id = blockers.push_gate(gate_slab(g));
        assert_eq!(id, g, "gate ids are the passages'");
    }
    let mut pieces = Vec::new();
    let pillars: Vec<VoxelMesh> = (0..4).map(|v| crate::props::ruin_pillar(1.0, v)).collect();
    let brazier = crate::props::brazier();
    for it in placements() {
        match it {
            Item::Statue { at, yaw } => {
                let q = (((yaw / std::f32::consts::FRAC_PI_2).round() as i32).rem_euclid(4)) as u8;
                blockers.push(Blocker { shape: Shape::Circle { x: at.x, z: at.z, r: 3.2 }, top: 8.0, cover: Cover::NONE });
                pieces.push(Piece { mesh: crate::props::statue_cat(0.5, 0), pos: at, quarter: q, vsize: V_MED, sway: 0.0 });
            }
            Item::Brazier { at } => {
                blockers.push(Blocker { shape: Shape::Circle { x: at.x, z: at.z, r: 0.9 }, top: 3.0, cover: Cover::NONE });
                pieces.push(Piece { mesh: brazier.clone(), pos: at, quarter: 0, vsize: 0.35, sway: 0.0 });
            }
            Item::Pillar { at, variant } => {
                blockers.push(Blocker { shape: Shape::Circle { x: at.x, z: at.z, r: 1.3 }, top: 6.0, cover: Cover::NONE });
                pieces.push(Piece { mesh: pillars[variant as usize % 4].clone(), pos: at, quarter: (variant % 4) as u8, vsize: V_MED, sway: 0.0 });
            }
            Item::Stub { a, b } => {
                blockers.push(Blocker { shape: Shape::Segment { x0: a.x, z0: a.z, x1: b.x, z1: b.z, r: 0.8 }, top: 3.2, cover: Cover::NONE });
                pieces.push(Piece { mesh: stub_mesh(a, b), pos: (a + b) * 0.5, quarter: 0, vsize: 0.4, sway: 0.0 });
            }
        }
    }
    (pieces, blockers)
}

/// A broken stretch of wall from `a` to `b` (world), centred on their
/// middle: a low course of stone, ragged along its top. Feet at y = 0,
/// origin at the middle of `a`..`b`.
fn stub_mesh(a: Vec3, b: Vec3) -> VoxelMesh {
    const V: f32 = 0.4;
    let mid = (a + b) * 0.5;
    let half = Vec3::new((a.x - mid.x).abs().max((b.x - mid.x).abs()), 0.0, (a.z - mid.z).abs().max((b.z - mid.z).abs()));
    let (nx, nz) = ((half.x * 2.0 / V) as i32 + 6, (half.z * 2.0 / V) as i32 + 6);
    let mut g = Grid::new(nx as usize, 10, nz as usize);
    let to = |p: Vec3| (((p.x - mid.x) / V).round() as i32 + nx / 2, ((p.z - mid.z) / V).round() as i32 + nz / 2);
    let (a2, b2) = (to(a), to(b));
    let n = ((b2.0 - a2.0).abs().max((b2.1 - a2.1).abs())).max(1);
    for k in 0..=n {
        let x = a2.0 + (b2.0 - a2.0) * k / n;
        let z = a2.1 + (b2.1 - a2.1) * k / n;
        let top = 5 + ((k * 7) % 4);
        g.filled_box(x - 1, 0, z - 1, x + 1, top, z + 1, [0.38, 0.36, 0.33, 1.0]);
    }
    g.mesh(V, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dressing is mirrored, stands on the floor, and keeps the lanes,
    /// the towers, the springs and the passages' mouths clear.
    #[test]
    fn the_dressing_is_fair_and_keeps_the_way_clear() {
        let items = placements();
        let count = |f: &dyn Fn(&Item) -> bool| items.iter().filter(|i| f(i)).count();
        assert_eq!(count(&|i| matches!(i, Item::Statue { .. })), 1, "the Heart's cat");
        let pillars = count(&|i| matches!(i, Item::Pillar { .. }));
        assert!((16..=32).contains(&pillars), "{pillars} pillars: two colonnades a side, less what the lanes cross");
        assert!(count(&|i| matches!(i, Item::Stub { .. })) >= 8);
        let braziers = count(&|i| matches!(i, Item::Brazier { .. }));
        assert!((8 + 4..=8 + 16).contains(&braziers), "braziers {braziers}");
        for it in &items {
            let p = item_at(it);
            if p.z.abs() > 0.5 {
                assert!(items.iter().any(|o| (item_at(o) - Vec3::new(p.x, p.y, -p.z)).length() < 1e-3), "{it:?} has no twin");
            }
        }
        crate::map::with(crate::map::MapId::Maze, || {
            let f = mz::floor();
            for it in &items {
                let pts: Vec<Vec3> = match *it {
                    Item::Stub { a, b } => vec![a, b, (a + b) * 0.5],
                    _ => vec![item_at(it)],
                };
                for p in pts {
                    assert!(f.fits(p.x, p.z, 0.5), "{it:?} is off the floor at {p}");
                    for lane in [Lane::Top, Lane::Bot] {
                        let d = lane.polyline().windows(2).map(|w| crate::abilities::point_segment_dist(p, w[0], w[1])).fold(f32::MAX, f32::min);
                        assert!(d > 3.0, "{it:?} stands in the {lane:?} lane ({d})");
                        for team in [Team::Blue, Team::Red] {
                            for inner in [false, true] {
                                assert!(p.distance(crate::consts::tower_pos(lane, team, inner)) > 5.0, "{it:?} crowds a tower");
                            }
                        }
                    }
                    for s in mz::heal_tree_positions() {
                        assert!(p.distance(s) > 4.0, "{it:?} is on a spring");
                    }
                    for g in mz::passages() {
                        for end in [g.p0, g.p1] {
                            assert!(p.distance(end) > 4.0 || matches!(it, Item::Statue { .. }), "{it:?} at a passage's mouth {end}");
                        }
                    }
                }
            }
        });
    }

    /// A gate's slab lies across its passage and into the walls either
    /// side: nothing gets round it while it is shut.
    #[test]
    fn a_slab_closes_its_passage() {
        crate::map::with(crate::map::MapId::Maze, || {
            let f = mz::floor();
            for g in 0..mz::passages().len() {
                let Shape::Segment { x0, z0, x1, z1, r } = gate_slab(g).shape else { panic!() };
                for end in [Vec3::new(x0, 0.0, z0), Vec3::new(x1, 0.0, z1)] {
                    assert!(f.sdf(end.x, end.z) > -r, "gate {g}'s slab stops short of the wall at {end}");
                }
            }
        });
    }

    /// Thorns line every section's walls (so every waking shows), at the
    /// walls' foot, never out in a room's middle.
    #[test]
    fn thorns_line_every_section() {
        let spots = thorn_spots();
        assert!(spots.len() > 300, "{} thorn spots", spots.len());
        for class in 0..mz::CLASSES {
            let n = spots.iter().filter(|s| s.0 == class).count();
            assert!(n >= 20, "class {class} ({}) has {n} thorns", mz::class_name(class));
        }
        crate::map::with(crate::map::MapId::Maze, || {
            for (_, p, _) in &spots {
                let d = mz::floor().sdf(p.x, p.z);
                assert!((-1.9..=-0.7).contains(&d), "a thorn {d} from the wall at {p}");
            }
        });
    }
}
