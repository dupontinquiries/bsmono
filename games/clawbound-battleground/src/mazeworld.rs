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
use crate::consts::Lane;
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
    /// An Archimedes screw at the head of a boulder track (`machine`): it
    /// winds the ball up out of the floor. Drawn turning by `scene`.
    Screw { at: Vec3 },
}

pub fn item_at(it: &Item) -> Vec3 {
    match *it {
        Item::Statue { at, .. } | Item::Brazier { at } | Item::Pillar { at, .. } | Item::Screw { at } => at,
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
        Item::Screw { at } => Item::Screw { at: f(at) },
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
            // (and off the boulders' tracks: a stub beside one pinned a bot
            // against its end, alone in the thorns, for 24 s -- seed 5)
            let track_d = |p: Vec3| crate::machine::pairs().iter().flatten().map(|t| t.dist(p, 0.0)).fold(f32::MAX, f32::min);
            if [a, b, c].iter().all(|&p| lane_d(p) > 5.0 && mouth_d(p) > 6.0 && spring_d(p) > 6.0 && track_d(p) > crate::machine::BOULDER_R + 5.0) {
                both(&mut out, Item::Stub { a, b });
            }
        }
    }
    // a screw beside every track's hopper, up in the hills or on the wall
    // top, standing on the ground there (placed on each pair's blue track
    // and mirrored)
    for pair in crate::machine::pairs() {
        let tr = &pair[0];
        let d = tr.dir_at(0.0);
        let side = Vec3::new(-d.z, 0.0, d.x);
        let h = tr.hopper() + side * (crate::machine::BOULDER_R + 2.6);
        both(&mut out, Item::Screw { at: Vec3::new(h.x, mz::terrain_height(h.x, h.z), h.z) });
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
                let at = c + out_corner * 9.0;
                // (not where a boulder comes down its track)
                if crate::machine::pairs().iter().flatten().all(|t| t.dist(at, 0.0) > crate::machine::BOULDER_R + 2.0) {
                    both(&mut out, Item::Brazier { at });
                }
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

/// A gear wheel lying flat on a wall top: its centre (at the wall's top),
/// radius, and which way it turns (+1 / -1: neighbours turn opposite, so a
/// train reads as meshed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gear {
    pub at: Vec3,
    pub r: f32,
    pub turn: f32,
}

/// How far a wheel's teeth stand out past its rim (u). Two wheels whose
/// centres are `r1 + r2 + TOOTH` apart mesh: each one's teeth reach into the
/// other's.
pub const TOOTH: f32 = 1.0;

/// The gear trains: wheels meshed tooth to tooth across the wall tops. A
/// train starts on the deepest free block of wall and grows wheel by wheel
/// (each placed so its teeth reach into its parent's, turning the other
/// way) while the wall holds them; none overlaps any but its own
/// neighbours. Found on the blue half and mirrored, turns reversed.
/// Visible from the play camera, which looks down over the walls.
pub fn gears() -> Vec<Gear> {
    crate::map::with(crate::map::MapId::Maze, || {
        let f = mz::floor();
        // a wheel of radius r fits at p: its teeth over wall, off the far
        // side, and clear of its own mirror
        let fits = |p: Vec3, r: f32| {
            f.sdf(p.x, p.z) >= r + TOOTH + 0.8 && !mz::beyond(p.x, p.z) && p.z <= 0.01 && (p.z.abs() < 0.5 || p.z.abs() * 2.0 > r * 2.0 + TOOTH + 1.5)
        };
        let clear = |out: &[Gear], p: Vec3, r: f32, parent: Option<usize>| {
            out.iter().enumerate().all(|(i, g)| {
                let q = Vec3::new(g.at.x, 0.0, g.at.z);
                let m = Vec3::new(q.x, 0.0, -q.z);
                let need = if Some(i) == parent { g.r + r + TOOTH - 0.01 } else { g.r + r + TOOTH * 2.0 + 4.0 };
                q.distance(p) >= need && m.distance(p) > g.r + r + TOOTH * 2.0 + 1.0
            })
        };
        let (hx, hz) = (mz::COLS[2] + mz::ROOM_HX + 8.0, mz::ROWS as f32 * mz::ROW_Z + mz::ROOM_HZ + 8.0);
        let mut cand = Vec::new();
        let mut z = -hz;
        while z <= 0.01 {
            let mut x = -hx;
            while x <= hx {
                let d = f.sdf(x, z);
                if d > 6.0 {
                    cand.push((d, Vec3::new(x, 0.0, z)));
                }
                x += 2.0;
            }
            z += 2.0;
        }
        cand.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.x.total_cmp(&b.1.x)).then(a.1.z.total_cmp(&b.1.z)));
        let mut out: Vec<Gear> = Vec::new();
        let mut trains = 0;
        for (d, p) in cand {
            let r = (d - TOOTH - 0.8).min(6.0);
            if r < 4.5 || !fits(p, r) || !clear(&out, p, r, None) {
                continue;
            }
            let turn = if trains % 2 == 0 { 1.0 } else { -1.0 };
            trains += 1;
            let start = out.len();
            out.push(Gear { at: Vec3::new(p.x, WALL_H + 0.9, p.z), r, turn });
            // grow the train: from each wheel in it, the first spot round
            // it (from its parent's far side) where a wheel fits
            let mut k = start;
            while k < out.len() && out.len() - start < 4 {
                let g = out[k];
                'grow: for step in 0..16 {
                    let a = step as f32 / 16.0 * std::f32::consts::TAU;
                    for r2 in [5.0f32, 4.0, 3.0] {
                        let dir = Vec3::new(a.cos(), 0.0, a.sin());
                        let c = Vec3::new(g.at.x, 0.0, g.at.z) + dir * (g.r + r2 + TOOTH);
                        if fits(c, r2) && clear(&out, c, r2, Some(k)) {
                            out.push(Gear { at: Vec3::new(c.x, WALL_H + 0.9, c.z), r: r2, turn: -g.turn });
                            break 'grow;
                        }
                    }
                }
                k += 1;
            }
        }
        let mirrored: Vec<Gear> = out.iter().filter(|g| g.at.z.abs() > 0.5).map(|g| Gear { at: Vec3::new(g.at.x, g.at.y, -g.at.z), r: g.r, turn: -g.turn }).collect();
        out.extend(mirrored);
        out
    })
}

/// A gear wheel, lying flat, radius `r` to its rim: a thick wooden rim,
/// six spokes and a bronze hub, and square cogs standing TOOTH out all
/// round, one every ~1.4 u of rim (so two wheels that mesh interleave).
/// Centred on its axle, y from 0 (so it sits on the wall top).
pub fn gear_mesh(r: f32) -> VoxelMesh {
    const V: f32 = 0.25;
    use std::f32::consts::{PI, TAU};
    let n = ((r + TOOTH) / V).ceil() as i32 + 1;
    let mut g = Grid::new((n * 2 + 1) as usize, 6, (n * 2 + 1) as usize);
    let wood = [0.46, 0.30, 0.15, 1.0];
    let dark = [0.30, 0.19, 0.10, 1.0];
    let bronze = [0.70, 0.49, 0.21, 1.0];
    let (rv, tv) = (r / V, TOOTH / V);
    let teeth = ((TAU * r / 1.4) as i32).max(10);
    for x in -n..=n {
        for z in -n..=n {
            let d = ((x * x + z * z) as f32).sqrt();
            let a = (z as f32).atan2(x as f32);
            let (cx, cz) = (x + n, z + n);
            if d <= rv && d >= rv - 3.5 {
                g.filled_box(cx, 0, cz, cx, 2, cz, if d >= rv - 0.8 { dark } else { wood });
            } else if d < rv - 3.5 {
                let spoke = (0..6).any(|k| {
                    let s = k as f32 / 6.0 * TAU;
                    ((a - s + PI).rem_euclid(TAU) - PI).abs() * d < 1.4
                });
                if d < 3.0 {
                    g.filled_box(cx, 0, cz, cx, 4, cz, bronze);
                } else if spoke {
                    g.filled_box(cx, 0, cz, cx, 1, cz, wood);
                }
            } else if d <= rv + tv {
                let k = ((a + PI) / TAU * teeth as f32).fract();
                if k < 0.45 {
                    g.filled_box(cx, 0, cz, cx, 2, cz, wood);
                }
            }
        }
    }
    g.mesh(V, true)
}

/// An Archimedes screw, standing: a wooden core, a helical blade round it
/// climbing three turns, bronze bands top and bottom. ~7 u tall; its axis
/// through the grid's middle, feet at y = 0.
pub fn screw_mesh() -> VoxelMesh {
    const V: f32 = 0.2;
    let (r_core, r_blade) = (0.45 / V, 1.5 / V);
    let n = (r_blade.ceil() as i32) + 1;
    let h = (7.0 / V) as i32;
    let mut g = Grid::new((n * 2 + 1) as usize, (h + 2) as usize, (n * 2 + 1) as usize);
    let wood = [0.45, 0.30, 0.16, 1.0];
    let blade = [0.55, 0.38, 0.20, 1.0];
    let bronze = [0.68, 0.48, 0.22, 1.0];
    for y in 0..h {
        let turn = y as f32 / h as f32 * 3.0 * std::f32::consts::TAU;
        for x in -n..=n {
            for z in -n..=n {
                let d = ((x * x + z * z) as f32).sqrt();
                let a = (z as f32).atan2(x as f32);
                let (cx, cz) = (x + n, z + n);
                if d <= r_core {
                    g.set(cx, y, cz, if y < 3 || y > h - 4 { bronze } else { wood });
                } else if d <= r_blade {
                    let off = ((a - turn).rem_euclid(std::f32::consts::TAU)) / std::f32::consts::TAU;
                    if off < 0.08 {
                        g.set(cx, y, cz, blade);
                    }
                }
            }
        }
    }
    g.mesh(V, true)
}

/// The boulder: a stone ball with two bronze bands round it at right
/// angles, so its roll reads. Centred on its middle.
pub fn boulder_mesh() -> VoxelMesh {
    const V: f32 = 0.25;
    let r = crate::machine::BOULDER_R / V;
    let n = r.ceil() as i32 + 1;
    let mut g = Grid::new((n * 2 + 1) as usize, (n * 2 + 1) as usize, (n * 2 + 1) as usize);
    let bronze = [0.66, 0.46, 0.20, 1.0];
    for x in -n..=n {
        for y in -n..=n {
            for z in -n..=n {
                let d = ((x * x + y * y + z * z) as f32).sqrt();
                if d <= r {
                    let k = ((x * 7 + y * 13 + z * 5) as u32).wrapping_mul(2654435761) >> 28;
                    let stone = [0.44 + k as f32 * 0.008, 0.42 + k as f32 * 0.007, 0.38 + k as f32 * 0.006, 1.0];
                    let band = d > r - 1.5 && (x.abs() <= 0 || y.abs() <= 0);
                    g.set(x + n, y + n, z + n, if band { bronze } else { stone });
                }
            }
        }
    }
    g.mesh(V, false)
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
    // the chutes, from each hopper down to its lip, on their trestles
    for pair in crate::machine::pairs() {
        for tr in pair {
            let (mesh, pos) = chute_mesh(tr);
            pieces.push(Piece { mesh, pos, quarter: 0, vsize: CHUTE_V, sway: 0.0 });
        }
    }
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
            Item::Screw { .. } => {
                // drawn by the scene, turning; up on the hills, off the
                // floor: nothing to collide with
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
const CHUTE_V: f32 = 0.4;

/// A track's chute: a wooden trough on trestles from its hopper (a bin
/// with bronze bands, up in the hills or on a wall top) down to its lip,
/// the legs standing on the ground beneath. Built in world units round its
/// middle; returns the mesh and where its origin goes (feet at the lowest
/// ground under it).
pub fn chute_mesh(tr: &crate::machine::Track) -> (VoxelMesh, Vec3) {
    const V: f32 = CHUTE_V;
    let (h, l) = (tr.hopper(), tr.lip());
    let r = crate::machine::BOULDER_R;
    let dir = Vec3::new(l.x - h.x, 0.0, l.z - h.z).normalize_or_zero();
    let side = Vec3::new(-dir.z, 0.0, dir.x);
    let n = ((l - h).length() / 0.15) as i32;
    let ground = |p: Vec3| mz::terrain_height(p.x, p.z);
    let base = (0..=n).map(|k| ground(h + (l - h) * (k as f32 / n as f32))).fold(f32::MAX, f32::min) - 0.4;
    let mid = (h + l) * 0.5;
    let half = (l - h).abs() * 0.5 + Vec3::splat(r + 3.0);
    let (nx, nz) = ((half.x * 2.0 / V) as i32 + 4, (half.z * 2.0 / V) as i32 + 4);
    let ny = ((h.y.max(l.y) + 4.0 - base) / V) as i32 + 2;
    let mut g = Grid::new(nx as usize, ny as usize, nz as usize);
    let cell = |p: Vec3| (((p.x - mid.x) / V).round() as i32 + nx / 2, ((p.y - base) / V).round() as i32, ((p.z - mid.z) / V).round() as i32 + nz / 2);
    let wood = [0.44, 0.29, 0.15, 1.0];
    let dark = [0.28, 0.18, 0.09, 1.0];
    let bronze = [0.68, 0.47, 0.21, 1.0];
    let mut put = |p: Vec3, c: [f32; 4]| {
        let (x, y, z) = cell(p);
        if x >= 0 && y >= 0 && z >= 0 && x < nx && y < ny && z < nz {
            g.set(x, y, z, c);
        }
    };
    // the trough: a floor under the ball's path and a rail either side
    for k in 0..=n {
        let c = h + (l - h) * (k as f32 / n as f32);
        let mut w = -(r + 0.4);
        while w <= r + 0.4 {
            put(c + side * w - Vec3::Y * 0.3, if k % 12 < 1 { dark } else { wood });
            w += V * 0.9;
        }
        for sgn in [-1.0f32, 1.0] {
            let mut y = -0.3;
            while y <= 1.2 {
                put(c + side * (sgn * (r + 0.5)) + Vec3::Y * y, wood);
                y += V * 0.9;
            }
        }
        // the trestles: a pair of legs every ~4 u, down to the ground, and a
        // brace across between them
        if k % 27 == 0 {
            for sgn in [-1.0f32, 1.0] {
                let foot = c + side * (sgn * (r + 0.5));
                let gy = ground(foot);
                let mut y = c.y - 0.6;
                while y >= gy - 0.2 {
                    put(Vec3::new(foot.x, y, foot.z), dark);
                    y -= V * 0.9;
                }
            }
            let by = (c.y - 0.6 + ground(c)) * 0.5;
            let mut w = -(r + 0.5);
            while w <= r + 0.5 {
                put(Vec3::new(c.x, by, c.z) + side * w, dark);
                w += V * 0.9;
            }
        }
    }
    // the hopper: an open bin round where the stone comes up, banded
    let gy = ground(h);
    let mut y = gy;
    while y <= h.y + 1.6 {
        let band = ((y - gy) / V) as i32 % 5 == 0;
        let mut a = -(r + 1.2);
        while a <= r + 1.2 {
            for sgn in [-1.0f32, 1.0] {
                put(Vec3::new(h.x, y, h.z) + dir * a + side * (sgn * (r + 1.2)), if band { bronze } else { dark });
                put(Vec3::new(h.x, y, h.z) + side * a + dir * (sgn * (r + 1.2)), if band { bronze } else { dark });
            }
            a += V * 0.9;
        }
        y += V * 0.9;
    }
    (g.mesh(V, true), Vec3::new(mid.x, base, mid.z))
}

/// A lock's pad: a square stone trapdoor, flush with the floor, a bronze
/// rim round it (the scene lights the rim while the lock is armed). Centred,
/// y from 0.
pub fn pad_mesh() -> VoxelMesh {
    const V: f32 = 0.25;
    let n = (crate::machine::PAD_HALF * 2.0 / V) as i32;
    let mut g = Grid::new(n as usize, 2, n as usize);
    for x in 0..n {
        for z in 0..n {
            let rim = x == 0 || z == 0 || x == n - 1 || z == n - 1;
            let k = ((x * 7 + z * 13) as u32).wrapping_mul(2654435761) >> 29;
            let stone = [0.46 + k as f32 * 0.01, 0.44 + k as f32 * 0.01, 0.40 + k as f32 * 0.008, 1.0];
            g.set(x, 0, z, if rim { [0.62, 0.44, 0.20, 1.0] } else { stone });
            if !rim && (x == n / 2 || z == n / 2) {
                g.set(x, 0, z, [0.30, 0.28, 0.26, 1.0]);
            }
        }
    }
    g.mesh(V, true)
}

/// The hole under an open pad: a square of dark, darkest in the middle, laid
/// just over the floor. Centred, y from 0.
pub fn pit_mesh() -> VoxelMesh {
    const V: f32 = 0.25;
    let n = (crate::machine::PAD_HALF * 2.0 / V) as i32;
    let mut g = Grid::new(n as usize, 1, n as usize);
    for x in 0..n {
        for z in 0..n {
            let e = x.min(z).min(n - 1 - x).min(n - 1 - z) as f32 / (n as f32 * 0.5);
            let v = 0.10 * (1.0 - e) + 0.01;
            g.set(x, 0, z, [v, v * 0.9, v * 0.8, 1.0]);
        }
    }
    g.mesh(V, true)
}

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
        assert_eq!(count(&|i| matches!(i, Item::Screw { .. })), 6, "one at every track's head");
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
                    // up by a hopper, on the ground there, off the floor
                    Item::Screw { at } => {
                        assert!(crate::machine::pairs().iter().flatten().any(|t| Vec3::new(t.hopper().x, 0.0, t.hopper().z).distance(Vec3::new(at.x, 0.0, at.z)) < crate::machine::BOULDER_R + 3.0), "{it:?} by no hopper");
                        assert!(f.sdf(at.x, at.z) > 1.0 && (at.y - mz::terrain_height(at.x, at.z)).abs() < 1e-3, "{it:?} not standing on the wall or the hills");
                        vec![]
                    }
                    _ => vec![item_at(it)],
                };
                for p in pts {
                    assert!(f.fits(p.x, p.z, 0.5), "{it:?} is off the floor at {p}");
                    for lane in [Lane::Top, Lane::Bot] {
                        let d = lane.polyline().windows(2).map(|w| crate::abilities::point_segment_dist(p, w[0], w[1])).fold(f32::MAX, f32::min);
                        assert!(d > 3.0, "{it:?} stands in the {lane:?} lane ({d})");
                        for team in [crate::consts::Team::Blue, crate::consts::Team::Red] {
                            for inner in [false, true] {
                                assert!(p.distance(crate::consts::tower_pos(lane, team, inner)) > 5.0, "{it:?} crowds a tower");
                            }
                        }
                    }
                    for s in mz::heal_tree_positions() {
                        assert!(p.distance(s) > 4.0, "{it:?} is on a spring");
                    }
                    // the boulders' way is clear (a screw stands at a
                    // track's head, beside it, by design)
                    // and a stub stands well off one: the bots' dodge room
                    // (one 5 u off pinned a bot against its end: seed 5)
                    if !matches!(it, Item::Screw { .. }) {
                        let d = crate::machine::pairs().iter().flatten().map(|t| t.dist(p, 0.0)).fold(f32::MAX, f32::min);
                        let need = if matches!(it, Item::Stub { .. }) { crate::machine::BOULDER_R + 5.0 } else { crate::machine::BOULDER_R + 1.5 };
                        assert!(d > need, "{it:?} stands on a boulder track ({d})");
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

    /// The gear trains: on the walls (never over the floor, never past the
    /// outer wall), meshed in trains, mirrored with the turn reversed, and
    /// enough of them to read as machinery from anywhere.
    #[test]
    fn gears_crown_the_walls() {
        let gs = gears();
        // (122 at writing, trains of up to four; each is one culled draw of
        // MAX_DRAWS 4096, so the ceiling is about clutter, not cost)
        assert!((40..=140).contains(&gs.len()), "{} gears", gs.len());
        crate::map::with(crate::map::MapId::Maze, || {
            for g in &gs {
                let d = mz::floor().sdf(g.at.x, g.at.z);
                assert!(d >= g.r + 1.0, "a gear of {} over the floor's edge at {} (wall depth {d})", g.r, g.at);
                assert!(!mz::beyond(g.at.x, g.at.z));
                assert!(gs.iter().any(|m| (m.at - Vec3::new(g.at.x, g.at.y, -g.at.z)).length() < 1e-3 && (m.turn == -g.turn || g.at.z.abs() < 0.5)), "{g:?} has no mirror");
            }
            // any two wheels either mesh (teeth into teeth, turning opposite
            // ways) or stand clear of each other's teeth
            let mut meshed = 0;
            for (i, a) in gs.iter().enumerate() {
                for b in gs.iter().skip(i + 1) {
                    let d = a.at.distance(b.at);
                    if d < a.r + b.r + TOOTH + 0.05 {
                        assert!(d > a.r + b.r + TOOTH - 0.05, "gears overlap at {} and {}", a.at, b.at);
                        assert_eq!(a.turn, -b.turn, "meshed wheels at {} and {} turn the same way", a.at, b.at);
                        meshed += 1;
                    } else {
                        assert!(d > a.r + b.r + TOOTH * 2.0, "wheels at {} and {} foul each other's teeth", a.at, b.at);
                    }
                }
            }
            assert!(meshed * 3 >= gs.len(), "{meshed} meshings among {} wheels: not trains", gs.len());
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
