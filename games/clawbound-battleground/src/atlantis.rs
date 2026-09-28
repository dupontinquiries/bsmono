//! atlantis — the Drowned Halls: 3 a side, on the floor of the sea.
//!
//! One lane and one jungle, both indoors. The lane is a long colonnaded
//! hall from base to base, bending a little west at the middle; the jungle
//! is one great domed rotunda to the east, **the Drowned Library**, reached
//! from the lane by a passage on each half and from each base by a back
//! passage. West of the lane, a side gallery on each half gives a flank and
//! a heal spring. Each base is a domed rotunda of its own.
//!
//! ## How the ground works
//!
//! As on the pass and the sky, the walkable floor is a union of capsules
//! and discs (`floor()`), all at y = 0. Outside it `terrain_height` rises
//! sheer into the halls' walls ([`WALL_H`] high, [`WALL_THICK`] thick),
//! and beyond the walls it falls away to the sea floor ([`SEABED_Y`]),
//! dunes and rock where kelp grows and fish go by -- seen over the walls,
//! through the glass. There is no water to wade: the halls are dry, and the
//! sea is everywhere else (the renderer's `sea_map`: caustics on every
//! floor, the underwater grade, the sea overhead in the sky).
//!
//! ## Symmetry
//!
//! Mirrored in z like the other maps: every authored point is for the blue
//! (z < 0) half or on z = 0. `tests::the_map_is_fair` holds it to that.
//! Only `Lane::Mid` has minions and towers here (`map::lanes`); `Top` and
//! `Bot` answer with routes for cameras (Top: the galleries' side; Bot:
//! through the Library).

use glam::Vec3;
use std::sync::OnceLock;

use crate::blockers::Shape;
use crate::consts::{Lane, Team};
use crate::floor::FloorPlan;

/// Players per team.
pub const TEAM_SIZE: usize = 3;

pub const ARENA_RADIUS: f32 = 125.0;
/// The rendered square: the sea floor runs on past the halls.
pub const GROUND_HALF: f32 = 250.0;

/// Base (nexus) distance from the centre, along z.
pub const BASE_Z: f32 = 92.0;
/// The lane hall's half-width.
pub const LANE_R: f32 = 8.0;
/// The Drowned Library: centre x, and radius (centre z = 0).
pub const LIBRARY_X: f32 = 48.0;
pub const LIBRARY_R: f32 = 28.0;
/// Passages' half-width.
pub const PASSAGE_R: f32 = 4.5;
/// A base's rotunda.
pub const BASE_R: f32 = 22.0;
/// The side galleries west of the lane: centre (blue half) and radius.
pub const GALLERY: (f32, f32, f32) = (-20.0, -46.0, 8.0);

/// The walls: height, how sheer (the rise from floor to top), thickness.
pub const WALL_H: f32 = 11.0;
pub const WALL_RISE: f32 = 1.2;
pub const WALL_THICK: f32 = 5.0;
/// The sea floor outside the halls.
pub const SEABED_Y: f32 = -8.0;

pub fn base_pos(team: Team) -> Vec3 {
    Vec3::new(0.0, 0.0, -team.push_z() * BASE_Z)
}
pub fn fountain(team: Team) -> Vec3 {
    Vec3::new(0.0, 0.0, -team.push_z() * (BASE_Z + 11.0))
}
pub fn titan_pos(team: Team) -> Vec3 {
    Vec3::new(0.0, 0.0, -team.push_z() * (BASE_Z - 11.0))
}

fn at(x: f32, z: f32) -> Vec3 {
    Vec3::new(x, 0.0, z)
}

fn mirror_z(pts: &[(f32, f32)]) -> Vec<Vec3> {
    let mut v: Vec<Vec3> = pts.iter().map(|&(x, z)| at(x, z)).collect();
    let last_on_axis = pts.last().is_some_and(|p| p.1 == 0.0);
    let back: Vec<Vec3> = pts.iter().rev().skip(last_on_axis as usize).map(|&(x, z)| at(x, -z)).collect();
    v.extend(back);
    v
}

/// The lane hall's centre-line, blue half (to the axis).
const HALL: [(f32, f32); 5] = [(0.0, -80.0), (0.0, -58.0), (-5.0, -36.0), (-8.0, -14.0), (-8.0, 0.0)];

/// The passage from the lane into the Library (blue half): lane end, hall end.
pub const PASSAGE: ((f32, f32), (f32, f32)) = ((-2.0, -40.0), (LIBRARY_X - 20.0, -16.0));
/// The back passage from the base rotunda to the Library (blue half).
pub const BACK: ((f32, f32), (f32, f32)) = ((14.0, -84.0), (LIBRARY_X - 8.0, -22.0));
/// The gallery's neck to the lane (blue half).
pub const NECK: ((f32, f32), (f32, f32)) = ((-6.0, -46.0), (GALLERY.0 + 4.0, GALLERY.1));

/// Lane centre-lines, blue base mouth to red.
pub fn lane_polyline(lane: Lane) -> Vec<Vec3> {
    match lane {
        Lane::Mid => mirror_z(&HALL),
        // camera routes only
        Lane::Top => mirror_z(&[(0.0, -80.0), (-6.0, -56.0), (GALLERY.0, GALLERY.1), (-8.0, -20.0), (-8.0, 0.0)]),
        Lane::Bot => mirror_z(&[(0.0, -80.0), (BACK.0 .0, BACK.0 .1), (BACK.1 .0, BACK.1 .1), (LIBRARY_X, 0.0)]),
    }
}

/// Towers by distance along the hall from the team's end: the inner one at
/// the base's door, the outer one short of the middle.
pub fn tower_pos(lane: Lane, team: Team, inner: bool) -> Vec3 {
    let mut pts = lane_polyline(lane);
    if team == Team::Red {
        pts.reverse();
    }
    crate::pass::along(&pts, if inner { 14.0 } else { 44.0 })
}

fn lerp2(a: (f32, f32), b: (f32, f32), t: f32) -> Vec3 {
    at(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// The walkable floor.
pub fn floor() -> &'static FloorPlan {
    static F: OnceLock<FloorPlan> = OnceLock::new();
    F.get_or_init(|| FloorPlan::new(shapes()))
}

fn shapes() -> Vec<Shape> {
    let mut s = Vec::new();
    let seg = |a: Vec3, b: Vec3, r: f32| Shape::Segment { x0: a.x, z0: a.z, x1: b.x, z1: b.z, r };
    let pts = lane_polyline(Lane::Mid);
    for w in pts.windows(2) {
        s.push(seg(w[0], w[1], LANE_R));
    }
    for team in [Team::Blue, Team::Red] {
        let b = base_pos(team);
        s.push(Shape::Circle { x: b.x, z: b.z, r: BASE_R });
        let f = fountain(team);
        s.push(Shape::Circle { x: f.x, z: f.z, r: 11.0 });
    }
    s.push(Shape::Circle { x: LIBRARY_X, z: 0.0, r: LIBRARY_R });
    for zs in [1.0f32, -1.0] {
        for (a, b, r) in [(PASSAGE.0, PASSAGE.1, PASSAGE_R), (BACK.0, BACK.1, PASSAGE_R), (NECK.0, NECK.1, 4.0)] {
            s.push(seg(at(a.0, a.1 * zs), at(b.0, b.1 * zs), r));
        }
        s.push(Shape::Circle { x: GALLERY.0, z: GALLERY.1 * zs, r: GALLERY.2 });
    }
    s
}

/// The walkable floor's height: all at 0.
pub fn floor_height(_x: f32, _z: f32) -> f32 {
    0.0
}

/// The two beasts' grounds (`bosses`): the Library's two ends.
pub fn pit_positions() -> [Vec3; 2] {
    [at(LIBRARY_X + 4.0, -13.0), at(LIBRARY_X + 4.0, 13.0)]
}

/// Heal springs: in each gallery, midway down each back passage, and at the
/// Library's east wall.
pub fn heal_tree_positions() -> [Vec3; 6] {
    let mid = lerp2(BACK.0, BACK.1, 0.45);
    [
        at(GALLERY.0 - 2.0, GALLERY.1),
        at(GALLERY.0 - 2.0, -GALLERY.1),
        mid,
        Vec3::new(mid.x, 0.0, -mid.z),
        at(LIBRARY_X + 18.0, -6.0),
        at(LIBRARY_X + 18.0, 6.0),
    ]
}

/// No water to wade in the halls.
pub fn river_dist(_x: f32, _z: f32) -> f32 {
    1.0e4
}

fn hash2(x: f32, z: f32) -> f32 {
    let v = (x * 127.1 + z * 311.7).sin() * 43758.547;
    v - v.floor()
}

fn vnoise(x: f32, z: f32) -> f32 {
    let (xi, zi) = (x.floor(), z.floor());
    let (xf, zf) = (x - xi, z - zi);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (u, v) = (s(xf), s(zf));
    let a = hash2(xi, zi);
    let b = hash2(xi + 1.0, zi);
    let c = hash2(xi, zi + 1.0);
    let d = hash2(xi + 1.0, zi + 1.0);
    a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v
}

/// Terrain height: the floor (flat, a hair of wear), the walls rising sheer
/// round it, and beyond them the sea floor. `|z|` in the noise so the two
/// halves match exactly.
pub fn terrain_height(x: f32, z: f32) -> f32 {
    let d = floor().sdf(x, z);
    let za = z.abs();
    if d <= 0.0 {
        return 0.03 * (vnoise(x * 0.5, za * 0.5) - 0.5);
    }
    if d < WALL_RISE {
        let t = d / WALL_RISE;
        return WALL_H * (1.0 - (1.0 - t).powi(3));
    }
    if d < WALL_RISE + WALL_THICK {
        // the wall's top: a coping, a little broken here and there
        let broken = (vnoise(x * 0.35 + 7.0, za * 0.35) - 0.72).max(0.0) * 6.0;
        return WALL_H + 0.4 - broken;
    }
    // down the outer face to the sea floor, then dunes and rock
    let e = d - WALL_RISE - WALL_THICK;
    let t = (e / 3.0).clamp(0.0, 1.0);
    let face = WALL_H + (SEABED_Y - WALL_H) * t * t * (3.0 - 2.0 * t);
    let dunes = 1.6 * (vnoise(x * 0.06, za * 0.06) - 0.5) + 0.6 * (vnoise(x * 0.23 + 3.0, za * 0.23) - 0.5);
    let rocks = (vnoise(x * 0.11 + 11.0, za * 0.11 + 5.0) - 0.62).max(0.0) * 14.0;
    face.max(SEABED_Y + dunes + rocks) * (1.0 - t) + (SEABED_Y + dunes + rocks) * t
}

/// Is (x, z) out on the sea floor (past the walls)?
pub fn on_seabed(x: f32, z: f32) -> bool {
    floor().sdf(x, z) > WALL_RISE + WALL_THICK + 3.0
}

/// Colonnade columns along both walls of the lane hall (blue half, mirrored
/// by the caller): points just inside the wall, every `step` along it.
pub fn colonnade(step: f32) -> Vec<Vec3> {
    let pts = lane_polyline(Lane::Mid);
    let mut out = Vec::new();
    let total: f32 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    let mut s = step * 0.5;
    while s < total * 0.5 {
        let p = crate::pass::along(&pts, s);
        let q = crate::pass::along(&pts, s + 0.5);
        let dir = (q - p).normalize_or(Vec3::Z);
        let side = Vec3::new(-dir.z, 0.0, dir.x);
        for sgn in [-1.0f32, 1.0] {
            let c = p + side * sgn * (LANE_R - 1.6);
            // not where a passage or the gallery's neck opens the wall, and
            // not in a base's rotunda
            if floor().sdf(c.x + side.x * sgn * 3.0, c.z + side.z * sgn * 3.0) > 0.5 && c.z > -BASE_Z + BASE_R + 1.0 {
                out.push(c);
            }
        }
        s += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_map_is_fair() {
        crate::map::with(crate::map::MapId::Atlantis, || {
            let f = floor();
            // mirrored in z: floor, terrain, bases, towers, pits, springs
            for k in 0..4000 {
                let x = (hash2(k as f32, 1.0) - 0.5) * 2.0 * ARENA_RADIUS;
                let z = (hash2(k as f32, 2.0) - 0.5) * 2.0 * ARENA_RADIUS;
                assert!((f.sdf(x, z) - f.sdf(x, -z)).abs() < 1e-3, "floor at {x},{z}");
                assert!((terrain_height(x, z) - terrain_height(x, -z)).abs() < 1e-3, "terrain at {x},{z}");
            }
            let mirror = |p: Vec3| Vec3::new(p.x, p.y, -p.z);
            assert!((base_pos(Team::Blue) - mirror(base_pos(Team::Red))).length() < 1e-4);
            for inner in [false, true] {
                assert!((tower_pos(Lane::Mid, Team::Blue, inner) - mirror(tower_pos(Lane::Mid, Team::Red, inner))).length() < 1e-3);
            }
            let p = pit_positions();
            assert!((p[0] - mirror(p[1])).length() < 1e-4);
        });
    }

    /// Everything a match needs stands on the floor with room round it:
    /// bases, fountains, Elders, towers, pits, springs; and the lane runs
    /// unbroken from base to base.
    #[test]
    fn everything_stands_on_the_floor_and_the_lane_runs_through() {
        crate::map::with(crate::map::MapId::Atlantis, || {
            let f = floor();
            for team in [Team::Blue, Team::Red] {
                for p in [base_pos(team), fountain(team), titan_pos(team), tower_pos(Lane::Mid, team, true), tower_pos(Lane::Mid, team, false)] {
                    assert!(f.fits(p.x, p.z, 2.0), "{p} is not on the floor");
                }
            }
            for p in pit_positions().into_iter().chain(heal_tree_positions()) {
                assert!(f.fits(p.x, p.z, 2.0), "{p} is not on the floor");
            }
            let pts = lane_polyline(Lane::Mid);
            for w in pts.windows(2) {
                for k in 0..=20 {
                    let p = w[0] + (w[1] - w[0]) * (k as f32 / 20.0);
                    assert!(f.fits(p.x, p.z, LANE_R - 1.0), "the lane is pinched at {p}");
                }
            }
            // the Library is reachable by the passages (floor all along)
            for (a, b) in [(PASSAGE.0, PASSAGE.1), (BACK.0, BACK.1)] {
                for k in 0..=20 {
                    let p = lerp2(a, b, k as f32 / 20.0);
                    assert!(f.fits(p.x, p.z, 1.5), "passage pinched at {p}");
                }
            }
        });
    }

    /// Closed spaces: the floor is ringed by walls a cat's leap cannot top
    /// (every floor edge rises to WALL_H within a couple of units), and the
    /// sea floor lies beyond them, well below the halls.
    #[test]
    fn the_halls_are_walled_and_the_sea_lies_beyond() {
        crate::map::with(crate::map::MapId::Atlantis, || {
            let f = floor();
            let mut edges = 0;
            for k in 0..3000 {
                let x = (hash2(k as f32, 7.0) - 0.5) * 2.0 * ARENA_RADIUS;
                let z = (hash2(k as f32, 9.0) - 0.5) * 2.0 * ARENA_RADIUS;
                let d = f.sdf(x, z);
                if d <= 0.0 {
                    assert!(terrain_height(x, z).abs() < 0.05, "the floor is flat at {x},{z}");
                } else if (WALL_RISE + 0.2..WALL_RISE + WALL_THICK).contains(&d) {
                    assert!(terrain_height(x, z) > WALL_H - 1.5, "a wall at {x},{z}: {}", terrain_height(x, z));
                    edges += 1;
                } else if d > WALL_RISE + WALL_THICK + 4.0 {
                    assert!(terrain_height(x, z) < 2.0, "the sea floor at {x},{z}: {}", terrain_height(x, z));
                }
            }
            assert!(edges > 50, "the scan found {edges} wall points");
            assert!(WALL_H > crate::leap::LEAP_UP + crate::leap::CLEAR_OVER, "a leap could top the walls");
        });
    }

    #[test]
    fn the_colonnade_lines_the_hall_walls() {
        crate::map::with(crate::map::MapId::Atlantis, || {
            let c = colonnade(9.0);
            assert!((10..60).contains(&c.len()), "{} columns", c.len());
            for p in c {
                let d = floor().sdf(p.x, p.z);
                assert!((-2.5..-0.5).contains(&d), "a column in the aisle or the wall at {p}: {d}");
            }
        });
    }
}
