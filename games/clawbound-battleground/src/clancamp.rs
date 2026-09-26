//! clancamp -- a clan's camp at each home base.
//!
//! Behind each team's fountain the clan lives: a hollow ringed by bramble
//! walls with the front left open to the lanes, and in it the
//!
//! * **leader's rock** -- a great flat-topped stone to speak from, a den
//!   in the hollow beneath it (a cat can leap up onto it);
//! * **medicine den** -- a cleft in a rock screened with ferns, a little
//!   pool, bundles of herbs drying outside;
//! * **warriors' den** -- the largest bramble dome;
//! * **apprentices' den** -- a smaller one;
//! * **nursery** -- the densest thicket, lined with moss, at the back where
//!   it is safest;
//! * **elders' den** -- under a fallen log, brambles grown over it;
//! * **fresh-kill pile** by the middle.
//!
//! Layout is data ([`layout`], tested without a GPU): each piece is placed
//! on a ring behind the base in the team's own frame (forward = toward the
//! enemy), then nudged round the ring until it sits on floor, clear of the
//! fountain, the Elder's ground, the lanes and every other piece; a piece
//! that finds no room is left out rather than put somewhere wrong. Both
//! teams' camps are the same camp, mirrored. On the sky map everything is
//! frosted.
//!
//! Scenery with collision (dens and rocks are solid, bramble walls hide
//! and block), no rules of its own.

use glam::Vec3;

use crate::blockers::{Blocker, Blockers, Cover, Shape};
use crate::consts::Team;
use crate::voxel::{Grid, VoxelMesh};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    LeaderRock,
    MedicineDen,
    WarriorsDen,
    ApprenticesDen,
    Nursery,
    EldersDen,
    FreshKill,
    Bramble,
}

impl Kind {
    /// Footprint radius on the ground (u).
    pub fn radius(self) -> f32 {
        match self {
            Kind::LeaderRock => 4.5,
            Kind::MedicineDen => 3.6,
            Kind::WarriorsDen => 3.8,
            Kind::ApprenticesDen => 2.9,
            Kind::Nursery => 3.3,
            Kind::EldersDen => 3.4,
            Kind::FreshKill => 1.0,
            Kind::Bramble => 1.6,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Piece {
    pub team: Team,
    pub kind: Kind,
    pub at: Vec3,
    /// Facing (the den's mouth, the rock's face), radians, 0 = +z.
    pub yaw: f32,
}

/// Where each piece wants to be, in the base's frame: angle round the
/// fountain measured from straight back (0) toward the team's right, and
/// distance from the fountain.
const PLAN: [(Kind, f32, f32); 7] = [
    (Kind::LeaderRock, 0.0, 17.0),
    (Kind::MedicineDen, -1.15, 16.0),
    (Kind::WarriorsDen, 1.15, 16.0),
    (Kind::Nursery, -0.55, 21.0),
    (Kind::ApprenticesDen, 1.75, 18.0),
    (Kind::EldersDen, 0.55, 21.0),
    (Kind::FreshKill, 0.9, 10.0),
];

/// The bramble wall: an arc behind the camp, this far from the fountain,
/// from this angle to its mirror, a stretch every this many radians.
const WALL_R: f32 = 27.0;
const WALL_SPAN: f32 = 2.2;
const WALL_STEP: f32 = 0.11;

/// Is (x, z) floor on the active map, with `margin` to spare?
fn on_floor(p: Vec3, margin: f32) -> bool {
    match crate::map::floor() {
        Some(f) => f.sdf(p.x, p.z) < -margin,
        None => {
            // the jungle: inside the arena, out of the water
            p.length() < crate::consts::ARENA_RADIUS - margin && crate::consts::water_depth(p.x, p.z) < 0.05
        }
    }
}

/// Clear of what a match needs: the fountain, the Elder's ground and its
/// pedestal, the lanes, the base's gate.
fn clear_of_play(team: Team, p: Vec3, r: f32) -> bool {
    let home = team.titan_pos();
    let near_zone = (0..12).any(|k| {
        let a = k as f32 / 12.0 * std::f32::consts::TAU;
        crate::systems::titan_zone(home, team, p + Vec3::new(a.cos(), 0.0, a.sin()) * r)
    }) || crate::systems::titan_zone(home, team, p);
    !near_zone
        && team.fountain().distance(p) > 8.0 + r
        && crate::consts::lane_clearance(p.x, p.z) > r + 3.0
        && (team.base_pos() + Vec3::Z * team.push_z() * 12.0).distance(p) > 6.0 + r
}

/// The camps of both teams on the active map.
pub fn layout() -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for team in [Team::Blue, Team::Red] {
        let pz = team.push_z();
        let back = Vec3::new(0.0, 0.0, -pz);
        let right = Vec3::new(pz, 0.0, 0.0);
        let centre = team.fountain();
        let placed_before = out.len();
        let fits = |out: &[Piece], p: Vec3, r: f32| {
            on_floor(p, r) && clear_of_play(team, p, r) && out[placed_before..].iter().all(|q| q.at.distance(p) > q.kind.radius() + r + 1.2)
        };
        for &(kind, ang, dist) in &PLAN {
            let r = kind.radius();
            // nudge round the ring, then in and out, until it fits
            let mut spot = None;
            'find: for dd in [0.0f32, -3.0, 3.0, -6.0, 6.0] {
                for k in 0..10 {
                    let da = (k as f32 * 0.12) * if k % 2 == 0 { 1.0 } else { -1.0 };
                    let a = ang + da;
                    let p = centre + (back * a.cos() + right * a.sin()) * (dist + dd);
                    if fits(&out, p, r) {
                        spot = Some(p);
                        break 'find;
                    }
                }
            }
            if let Some(p) = spot {
                // facing the middle of the camp
                let to = centre - p;
                out.push(Piece { team, kind, at: p, yaw: to.x.atan2(to.z) });
            }
        }
        let mut a = -WALL_SPAN;
        while a <= WALL_SPAN + 1e-3 {
            let p = centre + (back * a.cos() + right * a.sin()) * WALL_R;
            if fits(&out, p, Kind::Bramble.radius() * 0.6) {
                let tangent = right * a.cos() - back * a.sin();
                out.push(Piece { team, kind: Kind::Bramble, at: p, yaw: tangent.x.atan2(tangent.z) });
            }
            a += WALL_STEP * 2.0;
        }
    }
    out
}

// --- meshes -----------------------------------------------------------------

/// Voxel size of the camp's pieces.
pub const VOX: f32 = 0.25;

fn h(x: f32, y: f32, z: f32, s: f32) -> f32 {
    let v = (x * 12.9898 + y * 78.233 + z * 37.719 + s * 4.1414).sin() * 43758.547;
    v - v.floor()
}

const BRAMBLE: [f32; 4] = [0.20, 0.24, 0.13, 1.0];
const THORN: [f32; 4] = [0.36, 0.26, 0.16, 1.0];
const BERRY: [f32; 4] = [0.45, 0.12, 0.30, 1.0];
const STONE: [f32; 4] = [0.46, 0.45, 0.44, 1.0];
const STONE_D: [f32; 4] = [0.32, 0.31, 0.31, 1.0];
const DARK: [f32; 4] = [0.05, 0.05, 0.06, 1.0];
const MOSS: [f32; 4] = [0.25, 0.40, 0.20, 1.0];
const WOOD: [f32; 4] = [0.36, 0.26, 0.17, 1.0];
const FERN: [f32; 4] = [0.22, 0.46, 0.22, 1.0];
const WATER: [f32; 4] = [0.35, 0.55, 0.70, 1.0];

/// A thicket: a dome of woven stems, lumpy, thorned, a few berries.
fn thicket(g: &mut Grid, cx: f32, cz: f32, rx: f32, ry: f32, rz: f32, seed: f32) {
    g.ellipsoid(cx, 0.0, cz, rx, ry, rz, BRAMBLE);
    for k in 0..40 {
        let a = h(k as f32, 0.0, 0.0, seed) * std::f32::consts::TAU;
        let e = h(k as f32, 1.0, 0.0, seed) * 1.3;
        let (x, y, z) = (cx + a.cos() * rx * e.cos() * 0.95, ry * e.sin() * 0.95, cz + a.sin() * rz * e.cos() * 0.95);
        let s = 1.2 + h(k as f32, 2.0, 0.0, seed) * 1.6;
        g.ellipsoid(x, y, z, s, s * 0.8, s, if k % 7 == 0 { THORN } else { BRAMBLE });
        if k % 9 == 0 {
            g.set(x as i32, (y + s) as i32, z as i32, BERRY);
        }
    }
}

/// A den: a thicket with its mouth (a dark hollow) toward +z.
fn den(size: f32, seed: f32, moss: bool) -> VoxelMesh {
    let k = 1.0 / VOX;
    let (rx, ry, rz) = (size * k, size * 0.72 * k, size * 0.9 * k);
    let n = (size * 2.0 * k + 6.0) as usize;
    let mut g = Grid::new(n, (ry + 4.0) as usize, n);
    let c = n as f32 * 0.5;
    thicket(&mut g, c, c, rx, ry, rz, seed);
    // the mouth: a hollow into the dome, its floor dark
    g.carve_sphere(c, ry * 0.35, c + rz * 0.75, size * 0.42 * k);
    g.carve_sphere(c, ry * 0.3, c + rz * 0.3, size * 0.5 * k);
    g.ellipsoid(c, 0.0, c + rz * 0.4, size * 0.4 * k, 0.8, size * 0.45 * k, if moss { MOSS } else { DARK });
    g.mesh(VOX, true)
}

/// The leader's rock: stacked slabs to a flat top, a den under the overhang.
fn leader_rock(seed: f32) -> VoxelMesh {
    let k = 1.0 / VOX;
    let n = (10.0 * k) as usize;
    let mut g = Grid::new(n, (6.5 * k) as usize, n);
    let c = n as f32 * 0.5;
    for (i, (r, y0, y1)) in [(4.4f32, 0.0f32, 2.0f32), (3.9, 2.0, 3.6), (3.4, 3.6, 5.0)].iter().enumerate() {
        let off = (i as f32 - 1.0) * 0.4 * k;
        g.cyl_y(c + off, c - off * 0.5, r * k, (y0 * k) as i32, (y1 * k) as i32, if i == 1 { STONE_D } else { STONE });
    }
    // lumps on the sides, a lichen stain
    for q in 0..14 {
        let a = h(q as f32, 3.0, 0.0, seed) * std::f32::consts::TAU;
        let y = 0.5 + h(q as f32, 4.0, 0.0, seed) * 3.5;
        g.ellipsoid(c + a.cos() * 3.9 * k, y * k, c + a.sin() * 3.9 * k, 0.9 * k, 0.7 * k, 0.9 * k, if q % 4 == 0 { MOSS } else { STONE });
    }
    // the den beneath: a dark hollow under the front
    g.carve_sphere(c, 0.8 * k, c + 3.6 * k, 1.6 * k);
    g.ellipsoid(c, 0.0, c + 3.0 * k, 1.3 * k, 0.6, 1.2 * k, DARK);
    g.mesh(VOX, true)
}

/// The medicine den: a cleft in a boulder, screened with ferns, a little
/// pool beside it and herb bundles drying on a stick.
fn medicine_den(seed: f32) -> VoxelMesh {
    let k = 1.0 / VOX;
    let n = (8.0 * k) as usize;
    let mut g = Grid::new(n, (4.5 * k) as usize, n);
    let c = n as f32 * 0.5;
    g.ellipsoid(c - 0.6 * k, 0.0, c - 0.6 * k, 3.0 * k, 3.6 * k, 2.6 * k, STONE);
    // the cleft
    g.carve_sphere(c - 0.4 * k, 1.1 * k, c + 1.4 * k, 1.3 * k);
    g.ellipsoid(c - 0.4 * k, 0.0, c + 0.8 * k, 1.0 * k, 0.6, 1.2 * k, DARK);
    // ferns either side of the mouth
    for s in [-1.0f32, 1.0] {
        for f in 0..5 {
            let (x, z) = (c + s * (1.6 + f as f32 * 0.15) * k, c + (1.8 + h(f as f32, s, 0.0, seed)) * k);
            g.ellipsoid(x, (0.5 + f as f32 * 0.2) * k, z, 0.9 * k, 0.35 * k, 0.5 * k, FERN);
        }
    }
    // the pool
    g.ellipsoid(c + 2.5 * k, 0.0, c + 2.2 * k, 1.1 * k, 0.4, 0.9 * k, WATER);
    // herbs drying: a stick with bundles
    g.cyl_y(c + 2.6 * k, c - 1.2 * k, 0.12 * k, 0, (1.6 * k) as i32, WOOD);
    for (i, col) in [[0.55, 0.62, 0.30, 1.0], [0.70, 0.60, 0.25, 1.0], [0.40, 0.55, 0.40, 1.0]].iter().enumerate() {
        g.ellipsoid(c + 2.6 * k, (1.0 + i as f32 * 0.2) * k, c + (-1.2 + (i as f32 - 1.0) * 0.4) * k, 0.25 * k, 0.4 * k, 0.25 * k, *col);
    }
    g.mesh(VOX, true)
}

/// The elders' den: a fallen log, bark and a broken branch, brambles grown
/// over its far side, a mossy hollow under it facing +z.
fn elders_den(seed: f32) -> VoxelMesh {
    let k = 1.0 / VOX;
    let n = (8.0 * k) as usize;
    let mut g = Grid::new(n, (4.0 * k) as usize, n);
    let c = n as f32 * 0.5;
    for i in 0..(6.6 * k) as i32 {
        let x = c - 3.3 * k + i as f32;
        g.ellipsoid(x, 1.0 * k, c - 0.6 * k, 0.6, 1.0 * k, 1.0 * k, WOOD);
    }
    thicket(&mut g, c, c - 1.6 * k, 3.2 * k, 2.2 * k, 1.6 * k, seed);
    g.carve_sphere(c, 0.5 * k, c + 0.9 * k, 1.2 * k);
    g.ellipsoid(c, 0.0, c + 0.6 * k, 1.1 * k, 0.6, 1.0 * k, MOSS);
    g.mesh(VOX, true)
}

/// The fresh-kill pile: a heap of fur and feather, small.
fn fresh_kill(seed: f32) -> VoxelMesh {
    let k = 1.0 / VOX;
    let n = (2.4 * k) as usize;
    let mut g = Grid::new(n, n, n);
    let c = n as f32 * 0.5;
    for q in 0..7 {
        let a = h(q as f32, 5.0, 0.0, seed) * std::f32::consts::TAU;
        let r = h(q as f32, 6.0, 0.0, seed) * 0.6 * k;
        let col = [[0.45, 0.36, 0.28, 1.0], [0.52, 0.50, 0.46, 1.0], [0.35, 0.30, 0.24, 1.0]][q % 3];
        g.ellipsoid(c + a.cos() * r, (0.2 + q as f32 * 0.08) * k, c + a.sin() * r, 0.5 * k, 0.3 * k, 0.3 * k, col);
    }
    g.mesh(VOX, true)
}

/// A stretch of bramble wall, along x.
fn bramble(seed: f32) -> VoxelMesh {
    let k = 1.0 / VOX;
    let (nx, nz) = ((4.2 * k) as usize, (2.2 * k) as usize);
    let mut g = Grid::new(nx, (2.6 * k) as usize, nz);
    thicket(&mut g, nx as f32 * 0.5, nz as f32 * 0.5, 2.0 * k, 2.0 * k, 0.9 * k, seed);
    g.mesh(VOX, true)
}

/// Frost: the tops of everything whitened (the sky map).
fn frost(mut m: VoxelMesh) -> VoxelMesh {
    let top = m.inst.iter().map(|i| i.off_ao[1]).fold(0.0f32, f32::max).max(0.1);
    for i in m.inst.iter_mut() {
        let t = (i.off_ao[1] / top).clamp(0.0, 1.0);
        let snow = h(i.off_ao[0], i.off_ao[1], i.off_ao[2], 3.0) < 0.35 + 0.5 * t;
        if snow {
            i.color = [0.88, 0.91, 0.97, 1.0];
        }
    }
    m
}

pub fn mesh(kind: Kind, variant: i32) -> VoxelMesh {
    let s = variant as f32 * 1.37;
    let m = match kind {
        Kind::LeaderRock => leader_rock(s),
        Kind::MedicineDen => medicine_den(s),
        Kind::WarriorsDen => den(3.4, s, false),
        Kind::ApprenticesDen => den(2.6, s, false),
        Kind::Nursery => den(3.0, s, true),
        Kind::EldersDen => elders_den(s),
        Kind::FreshKill => fresh_kill(s),
        Kind::Bramble => bramble(s),
    };
    if crate::map::active() == crate::map::MapId::Sky {
        frost(m)
    } else {
        m
    }
}

/// Turn a mesh made facing +z to face `yaw` (0 = +z), about its origin.
fn turned(mut m: VoxelMesh, yaw: f32) -> VoxelMesh {
    let (s, c) = yaw.sin_cos();
    for i in m.inst.iter_mut() {
        let (x, z) = (i.off_ao[0], i.off_ao[2]);
        i.off_ao[0] = x * c + z * s;
        i.off_ao[2] = -x * s + z * c;
    }
    m
}

/// Add both camps to a world being built: meshes to `pieces`, collision to
/// `blockers` (dens and rocks solid, bramble walls block and hide).
pub(crate) fn build(pieces: &mut Vec<crate::scene::Piece>, blockers: &mut Blockers) {
    for (i, p) in layout().into_iter().enumerate() {
        let r = p.kind.radius();
        // not into a tree or a rock the map already put here
        if blockers.blocked(p.at.x, p.at.z, r * 0.6) {
            continue;
        }
        let (top, cover) = match p.kind {
            Kind::LeaderRock => (5.0, Cover::NONE),
            Kind::FreshKill => (0.0, Cover::NONE),
            Kind::Bramble => (2.4, Cover { x: p.at.x, z: p.at.z, radius: r, amount: 0.7 }),
            _ => (3.0, Cover::NONE),
        };
        if top > 0.0 {
            blockers.push(Blocker { shape: Shape::Circle { x: p.at.x, z: p.at.z, r: r * 0.8 }, top, cover });
        }
        let y = crate::map::ground_y(p.at.x, p.at.z);
        pieces.push(crate::scene::Piece { mesh: turned(mesh(p.kind, i as i32 % 5), p.yaw), pos: Vec3::new(p.at.x, y, p.at.z), quarter: 0, vsize: VOX, sway: 0.0 });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_map_has_two_whole_camps_clear_of_play() {
        for m in [crate::map::MapId::Jungle, crate::map::MapId::Pass, crate::map::MapId::Sky] {
            crate::map::with(m, || {
                let v = layout();
                for team in [Team::Blue, Team::Red] {
                    let mine: Vec<&Piece> = v.iter().filter(|p| p.team == team).collect();
                    for kind in [Kind::LeaderRock, Kind::MedicineDen, Kind::WarriorsDen, Kind::Nursery] {
                        assert!(mine.iter().any(|p| p.kind == kind), "{m:?} {team:?}: a {kind:?}");
                    }
                    let dens = mine.iter().filter(|p| matches!(p.kind, Kind::WarriorsDen | Kind::ApprenticesDen | Kind::Nursery | Kind::EldersDen)).count();
                    assert!(dens >= 3, "{m:?} {team:?}: dens {dens}");
                    let wall = mine.iter().filter(|p| p.kind == Kind::Bramble).count();
                    assert!(wall >= 6, "{m:?} {team:?}: a bramble wall ({wall} stretches)");
                    for p in &mine {
                        assert!(on_floor(p.at, p.kind.radius() * 0.5), "{m:?}: {p:?} on floor");
                        assert!(clear_of_play(team, p.at, p.kind.radius() * 0.5), "{m:?}: {p:?} clear of play");
                        // behind the base, not out in the lanes
                        assert!((p.at.z - team.base_pos().z) * team.push_z() < 12.0, "{m:?}: {p:?} behind the base");
                    }
                }
                // the same camp, mirrored
                let n = |t: Team| v.iter().filter(|p| p.team == t).count();
                assert_eq!(n(Team::Blue), n(Team::Red), "{m:?}: fair");
            });
        }
    }

    #[test]
    fn the_pieces_have_meshes() {
        for kind in [Kind::LeaderRock, Kind::MedicineDen, Kind::WarriorsDen, Kind::ApprenticesDen, Kind::Nursery, Kind::EldersDen, Kind::FreshKill, Kind::Bramble] {
            let m = mesh(kind, 1);
            assert!(m.inst.len() > 30, "{kind:?}: {}", m.inst.len());
        }
    }
}
