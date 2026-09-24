//! pass — Twinfang Pass: a mountain map, 15 a side.
//!
//! The clans' journey through the mountains: two great peaks with a gorge
//! between them, bare rock, thin cold streams and little waterfalls off the
//! cliffs, a few hard-won clearings with grass and scrub, and weather that
//! is mostly grey. Everything here is geometry and data; `scene` and
//! `render` draw it, `map` switches to it.
//!
//! ## How the ground works
//!
//! The sim is flat (y = 0) and 2D, as on the jungle map. So this map is a
//! CANYON: the walkable floor is carved out of solid rock (`floor()`, a
//! union of capsules and discs) and everything else is cliff. The cliffs
//! rise within a few units of the floor's edge -- sharp, as asked -- to a
//! broken high plateau, and the two peaks tower out of that. Cliffs are
//! collision because `Blockers` checks the floor plan; the twin peaks are
//! scenery you walk AROUND and THROUGH (two slots cut across each massif).
//!
//! ## Symmetry
//!
//! Like the jungle map, mirrored in z: Top is x < 0 for both teams, and
//! every authored point is given for the blue (z < 0) half or on z = 0 and
//! reflected. `tests::the_map_is_fair` holds it to that.

use glam::Vec3;
use std::sync::OnceLock;

use crate::blockers::Shape;
use crate::consts::{Lane, Team};
use crate::floor::FloorPlan;

/// Players per team on this map.
pub const TEAM_SIZE: usize = 15;

/// The two peaks' centres (x, z) and their height above the plateau.
pub const PEAKS: [(f32, f32); 2] = [(-60.0, 0.0), (60.0, 0.0)];
pub const PEAK_HEIGHT: f32 = 96.0;
pub const PEAK_WIDTH: f32 = 34.0;

/// Base (nexus) distance from the centre, along z.
pub const BASE_Z: f32 = 138.0;

pub fn base_pos(team: Team) -> Vec3 {
    Vec3::new(0.0, 0.0, -team.push_z() * BASE_Z)
}
pub fn fountain(team: Team) -> Vec3 {
    Vec3::new(0.0, 0.0, -team.push_z() * (BASE_Z + 9.0))
}
pub fn titan_pos(team: Team) -> Vec3 {
    Vec3::new(0.0, 0.0, -team.push_z() * (BASE_Z - 12.0))
}

fn mirror_z(pts: &[(f32, f32)]) -> Vec<Vec3> {
    // blue half as given, then the red half reflected, meeting on z = 0
    let mut v: Vec<Vec3> = pts.iter().map(|&(x, z)| Vec3::new(x, 0.0, z)).collect();
    let last_on_axis = pts.last().is_some_and(|p| p.1 == 0.0);
    let back: Vec<Vec3> = pts.iter().rev().skip(last_on_axis as usize).map(|&(x, z)| Vec3::new(x, 0.0, -z)).collect();
    v.extend(back);
    v
}

/// Lane centre-lines, from the blue base's mouth to the red one's.
pub fn lane_polyline(lane: Lane) -> Vec<Vec3> {
    match lane {
        // Through the gorge between the peaks, a slight dogleg either side
        // of the centre so the whole lane is not one sightline.
        Lane::Mid => mirror_z(&[(0.0, -124.0), (0.0, -92.0), (5.0, -58.0), (-4.0, -28.0), (0.0, 0.0)]),
        // Round the outside of the west peak.
        Lane::Top => mirror_z(&[(0.0, -124.0), (-38.0, -118.0), (-92.0, -96.0), (-122.0, -58.0), (-132.0, -20.0), (-133.0, 0.0)]),
        Lane::Bot => mirror_z(&[(0.0, -124.0), (38.0, -118.0), (92.0, -96.0), (122.0, -58.0), (132.0, -20.0), (133.0, 0.0)]),
    }
}

/// Towers, placed by distance walked along the lane from the team's end.
pub fn tower_pos(lane: Lane, team: Team, inner: bool) -> Vec3 {
    let mut pts = lane_polyline(lane);
    if team == Team::Red {
        pts.reverse();
    }
    let s = match (lane, inner) {
        (Lane::Mid, true) => 30.0,
        (Lane::Mid, false) => 62.0,
        (_, true) => 44.0,
        (_, false) => 104.0,
    };
    along(&pts, s)
}

/// The point `s` units along a polyline (clamped to its end).
pub fn along(pts: &[Vec3], mut s: f32) -> Vec3 {
    for w in pts.windows(2) {
        let l = (w[1] - w[0]).length();
        if s <= l {
            return w[0] + (w[1] - w[0]) * (s / l.max(1e-6));
        }
        s -= l;
    }
    *pts.last().unwrap_or(&Vec3::ZERO)
}

/// Clearings on the blue half: (x, z, radius). Reflected for red. The
/// high ones are notches in the massifs' shoulders; the low ones sit
/// between the lanes and the rim.
const CLEARINGS: [(f32, f32, f32); 9] = [
    (-52.0, -62.0, 10.0), // the west shoulder: a waterfall pool
    (52.0, -62.0, 10.0),  // east shoulder
    (-28.0, -84.0, 8.0),
    (28.0, -84.0, 8.0),
    (-110.0, -120.0, 9.0), // behind the side lanes, near the rim
    (110.0, -120.0, 9.0),
    (-150.0, -40.0, 9.0),
    (150.0, -40.0, 9.0),
    (-26.0, -38.0, 7.0), // off the gorge, under the peaks
];
/// Clearings ON the z = 0 line, not reflected: the boss grounds in the
/// slots through the massifs, and the saddle.
const CENTRE_CLEARINGS: [(f32, f32, f32); 2] = [(-96.0, 0.0, 12.5), (96.0, 0.0, 12.5)];

/// Narrow ledges joining clearings to lanes (blue half), radius 3.2.
const LEDGES: [((f32, f32), (f32, f32)); 11] = [
    ((-52.0, -62.0), (-28.0, -84.0)),
    ((-28.0, -84.0), (0.0, -92.0)),
    ((52.0, -62.0), (28.0, -84.0)),
    ((28.0, -84.0), (0.0, -92.0)),
    ((-52.0, -62.0), (-92.0, -96.0)),
    ((52.0, -62.0), (92.0, -96.0)),
    ((-110.0, -120.0), (-92.0, -96.0)),
    ((110.0, -120.0), (92.0, -96.0)),
    ((-150.0, -40.0), (-122.0, -58.0)),
    ((150.0, -40.0), (122.0, -58.0)),
    ((-26.0, -38.0), (-4.0, -28.0)),
];
/// Slots cut clean across each massif, mid to side, either side of the
/// peak: the ways "through the mountains". Blue half; mirrored.
///
/// Each bends round the peak's lower flank (~33 u from the summit): a
/// straight cut from the gorge to the boss ground passed 7 u from it, and
/// the two mirrored cuts would have sliced the mountain into a fin.
const SLOTS: [((f32, f32), (f32, f32)); 8] = [
    ((-2.0, -24.0), (-38.0, -40.0)),
    ((-38.0, -40.0), (-80.0, -34.0)),
    ((-80.0, -34.0), (-96.0, 0.0)),
    ((2.0, -24.0), (38.0, -40.0)),
    ((38.0, -40.0), (80.0, -34.0)),
    ((80.0, -34.0), (96.0, 0.0)),
    ((-96.0, 0.0), (-132.0, -20.0)),
    ((96.0, 0.0), (132.0, -20.0)),
];

/// The walkable floor.
pub fn floor() -> &'static FloorPlan {
    static F: OnceLock<FloorPlan> = OnceLock::new();
    F.get_or_init(|| {
        let mut s = Vec::new();
        let seg = |a: Vec3, b: Vec3, r: f32| Shape::Segment { x0: a.x, z0: a.z, x1: b.x, z1: b.z, r };
        for lane in crate::consts::LANES {
            let pts = lane_polyline(lane);
            let r = if lane == Lane::Mid { 8.5 } else { 7.5 };
            for w in pts.windows(2) {
                s.push(seg(w[0], w[1], r));
            }
        }
        for team in [Team::Blue, Team::Red] {
            let b = base_pos(team);
            s.push(Shape::Circle { x: b.x, z: b.z, r: 24.0 });
            let f = fountain(team);
            s.push(Shape::Circle { x: f.x, z: f.z, r: 12.0 });
        }
        for &(x, z, r) in &CLEARINGS {
            s.push(Shape::Circle { x, z, r });
            s.push(Shape::Circle { x, z: -z, r });
        }
        for &(x, z, r) in &CENTRE_CLEARINGS {
            s.push(Shape::Circle { x, z, r });
        }
        for &((ax, az), (bx, bz)) in &LEDGES {
            s.push(seg(Vec3::new(ax, 0.0, az), Vec3::new(bx, 0.0, bz), 3.2));
            s.push(seg(Vec3::new(ax, 0.0, -az), Vec3::new(bx, 0.0, -bz), 3.2));
        }
        for &((ax, az), (bx, bz)) in &SLOTS {
            s.push(seg(Vec3::new(ax, 0.0, az), Vec3::new(bx, 0.0, bz), 4.2));
            s.push(seg(Vec3::new(ax, 0.0, -az), Vec3::new(bx, 0.0, -bz), 4.2));
        }
        FloorPlan::new(s)
    })
}

/// The clearings, both halves: (x, z, radius).
pub fn clearings() -> Vec<(f32, f32, f32)> {
    let mut v: Vec<_> = CLEARINGS.iter().flat_map(|&(x, z, r)| [(x, z, r), (x, -z, r)]).collect();
    v.extend(CENTRE_CLEARINGS);
    v
}

/// Boss grounds: the two slot clearings in the massifs.
pub fn pit_positions() -> [Vec3; 2] {
    [Vec3::new(CENTRE_CLEARINGS[0].0, 0.0, 0.0), Vec3::new(CENTRE_CLEARINGS[1].0, 0.0, 0.0)]
}

/// Heal trees -- here, sheltered springs: in the six mid-height clearings.
pub fn heal_tree_positions() -> [Vec3; 6] {
    let c = |x: f32, z: f32| Vec3::new(x, 0.0, z);
    // z-mirrored like everything else: two on the far west ledge, one per half
    // Beside the pools, not in them: the streams run through the clearings'
    // centres, and the first render had a tree standing in the water.
    [c(-57.0, -65.0), c(57.0, -65.0), c(-57.0, 65.0), c(57.0, 65.0), c(-145.0, -36.0), c(-145.0, 36.0)]
}

/// A little waterfall: water leaves the cliff top at `lip` (on the rock,
/// just outside the floor) and falls `height` into a pool at `foot`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fall {
    pub lip: Vec3,
    pub foot: Vec3,
    pub height: f32,
}

/// Cold streams: each runs from a waterfall's foot across the floor. Blue
/// half, mirrored. Shallow: they cost a little speed, like the jungle's
/// rivers, and are narrow enough to step over in a stride or two.
const STREAMS: [&[(f32, f32)]; 4] = [
    &[(-56.0, -49.0), (-54.0, -55.0), (-52.0, -62.0), (-40.0, -73.0), (-28.0, -84.0), (-12.0, -89.0)],
    &[(56.0, -49.0), (54.0, -55.0), (52.0, -62.0), (40.0, -73.0), (28.0, -84.0), (12.0, -89.0)],
    &[(-160.0, -47.0), (-155.0, -43.0), (-150.0, -40.0), (-136.0, -49.0), (-124.0, -55.0)],
    &[(118.0, -129.0), (114.0, -125.0), (110.0, -120.0), (100.0, -107.0), (92.0, -96.0)],
];
pub const STREAM_HALF_WIDTH: f32 = 1.4;
pub const STREAM_DEPTH: f32 = 0.55;

pub fn streams() -> &'static [Vec<Vec3>] {
    static S: OnceLock<Vec<Vec<Vec3>>> = OnceLock::new();
    S.get_or_init(|| {
        let mut v = Vec::new();
        for s in STREAMS {
            v.push(s.iter().map(|&(x, z)| Vec3::new(x, 0.0, z)).collect::<Vec<_>>());
            v.push(s.iter().map(|&(x, z)| Vec3::new(x, 0.0, -z)).collect());
        }
        v
    })
}

/// A fall at the head of every stream: its first point is on the rock, the
/// second is the pool. The lip is where the stream leaves the cliff.
pub fn falls() -> Vec<Fall> {
    streams()
        .iter()
        .map(|s| {
            let (lip, foot) = (s[0], s[1]);
            Fall { lip, foot, height: cliff_top(lip.x, lip.z).min(14.0) }
        })
        .collect()
}

pub fn river_dist(x: f32, z: f32) -> f32 {
    let p = Vec3::new(x, 0.0, z);
    let mut best = f32::MAX;
    for s in streams() {
        // from the pool on: the lip-to-pool stretch is the waterfall
        for w in s[1..].windows(2) {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            best = best.min((a + ab * t - p).length());
        }
    }
    best
}

fn hash2(x: f32, z: f32) -> f32 {
    let v = (x * 127.1 + z * 311.7).sin() * 43758.547;
    v - v.floor()
}
/// Value noise, smooth, 0..1.
pub fn vnoise(x: f32, z: f32) -> f32 {
    let (ix, iz) = (x.floor(), z.floor());
    let (fx, fz) = (x - ix, z - iz);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let a = hash2(ix, iz);
    let b = hash2(ix + 1.0, iz);
    let c = hash2(ix, iz + 1.0);
    let d = hash2(ix + 1.0, iz + 1.0);
    let (u, v) = (s(fx), s(fz));
    a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v
}

/// Height of the rock at (x, z), ignoring the floor: the broken plateau and
/// the two peaks. Ridged noise, so the high ground is crags, not domes.
pub fn cliff_top(x: f32, z: f32) -> f32 {
    let ridge = |x: f32, z: f32| 1.0 - (vnoise(x, z) * 2.0 - 1.0).abs();
    let plateau = 9.0 + 7.0 * ridge(x * 0.045, z * 0.045) + 3.0 * vnoise(x * 0.16 + 5.0, z * 0.16);
    let mut h = plateau;
    for &(px, pz) in &PEAKS {
        let (dx, dz) = (x - px, z - pz);
        let d = dx.hypot(dz);
        // A fang, not a dome: a concave cone (steepest at the summit), its
        // radius wobbling with angle so the skyline has spurs and gullies.
        // The first version was a gaussian, and from above it read as two
        // snowballs.
        let a = dz.atan2(dx);
        let reach = PEAK_WIDTH * 2.1 * (1.0 + 0.22 * (a * 3.0 + px * 0.1).sin() + 0.12 * (a * 7.0).sin());
        let t = (1.0 - d / reach).clamp(0.0, 1.0);
        let peak = PEAK_HEIGHT * t.powf(1.7);
        // crags on the flanks, sharpest high up
        let crag = ridge(x * 0.09 + px, z * 0.09) * 12.0 * t;
        h = h.max(plateau + peak + crag);
    }
    // The world does not end at the arena: mountains climb away on every
    // side to the mesh edge.
    let r = x.hypot(z);
    let out = ((r - crate::consts::ARENA_RADIUS) / (crate::consts::GROUND_HALF - crate::consts::ARENA_RADIUS)).clamp(0.0, 1.0);
    h + out * out * 75.0 * (0.7 + 0.6 * ridge(x * 0.03 + 9.0, z * 0.03))
}

/// Width over which a cliff rises from the floor to its top. Small: sharp.
pub const CLIFF_RISE: f32 = 2.6;

/// Terrain height: the floor (with stream beds cut into it), then cliffs.
pub fn terrain_height(x: f32, z: f32) -> f32 {
    let d = floor().sdf(x, z);
    if d <= 0.0 {
        // the floor: flat stone, beds cut for the streams
        let rd = river_dist(x, z);
        let bed = if rd < STREAM_HALF_WIDTH + 1.0 {
            let t = 1.0 - (rd / (STREAM_HALF_WIDTH + 1.0)).clamp(0.0, 1.0);
            -STREAM_DEPTH * t * t * (3.0 - 2.0 * t)
        } else {
            0.0
        };
        // |z| so the two halves' floors match to the millimetre
        return bed + 0.12 * (vnoise(x * 0.4, z.abs() * 0.4) - 0.5);
    }
    let t = (d / CLIFF_RISE).clamp(0.0, 1.0);
    // a steep face with a lip, not a slope: most of the rise in the first half
    let s = 1.0 - (1.0 - t).powi(3);
    cliff_top(x, z) * s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::LANES;

    #[test]
    fn every_lane_is_walkable_end_to_end_by_a_big_cat() {
        let f = floor();
        for lane in LANES {
            let pts = lane_polyline(lane);
            let total: f32 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
            let mut s = 0.0;
            while s <= total {
                let p = along(&pts, s);
                assert!(f.fits(p.x, p.z, 1.2), "{lane:?} pinches at {p} (sdf {})", f.sdf(p.x, p.z));
                s += 1.0;
            }
        }
    }

    #[test]
    fn the_map_is_fair() {
        let f = floor();
        let mut k = 0;
        for x in (-160..=160).step_by(7) {
            for z in (-160..=160).step_by(7) {
                let (x, z) = (x as f32 + 0.3, z as f32 + 0.7);
                assert!((f.sdf(x, z) - f.sdf(x, -z)).abs() < 1e-3, "floor not mirrored at ({x},{z})");
                assert!((terrain_height(x, z) - terrain_height(x, -z)).abs() < 2e-2 || f.sdf(x, z) > 0.0, "ground at ({x},{z})");
                k += 1;
            }
        }
        assert!(k > 2000);
        for lane in LANES {
            for inner in [false, true] {
                let (b, r) = (tower_pos(lane, Team::Blue, inner), tower_pos(lane, Team::Red, inner));
                assert!((b.x - r.x).abs() < 1e-3 && (b.z + r.z).abs() < 1e-3, "{lane:?} towers not mirrored: {b} {r}");
            }
        }
    }

    #[test]
    fn towers_stand_on_the_floor_on_their_own_half() {
        for lane in LANES {
            for inner in [false, true] {
                let p = tower_pos(lane, Team::Blue, inner);
                assert!(floor().fits(p.x, p.z, 2.0), "{lane:?} tower in the rock at {p}");
                assert!(p.z < -20.0, "{lane:?} blue tower on the wrong half: {p}");
            }
        }
    }

    /// Everything a match needs to stand on is floor: bases, fountains,
    /// titans, boss grounds (with room for the pit), springs, stream beds.
    #[test]
    fn everything_stands_on_the_floor() {
        let f = floor();
        for t in [Team::Blue, Team::Red] {
            for p in [base_pos(t), fountain(t), titan_pos(t)] {
                assert!(f.fits(p.x, p.z, 3.0), "{p}");
            }
        }
        for p in pit_positions() {
            assert!(f.fits(p.x, p.z, crate::bosses::PIT_RADIUS * 0.9), "boss ground too tight at {p}: sdf {}", f.sdf(p.x, p.z));
        }
        for p in heal_tree_positions() {
            assert!(f.fits(p.x, p.z, 2.5), "{p}");
            assert!(river_dist(p.x, p.z) > STREAM_HALF_WIDTH + 2.0, "spring tree standing in a stream at {p}");
        }
        for s in streams() {
            assert!(f.sdf(s[0].x, s[0].z) > 0.0, "a fall's lip must be on the rock: {}", s[0]);
            for p in &s[1..] {
                assert!(f.fits(p.x, p.z, 1.0), "stream point off the floor: {p}");
            }
        }
    }

    /// Every clearing is reachable from every other along the floor: a
    /// flood fill on a 2 u grid from the blue base reaches them all.
    #[test]
    fn the_floor_is_one_connected_piece() {
        let f = floor();
        let (cell, n) = (2.0f32, 180i32);
        let at = |i: i32, j: i32| ((i - n / 2) as f32 * cell, (j - n / 2) as f32 * cell);
        let mut seen = vec![false; (n * n) as usize];
        let b = base_pos(Team::Blue);
        let start = (((b.x / cell) as i32 + n / 2), ((b.z / cell) as i32 + n / 2));
        let mut q = vec![start];
        seen[(start.1 * n + start.0) as usize] = true;
        while let Some((i, j)) = q.pop() {
            for (di, dj) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (a, c) = (i + di, j + dj);
                if a < 0 || c < 0 || a >= n || c >= n || seen[(c * n + a) as usize] {
                    continue;
                }
                let (x, z) = at(a, c);
                if f.fits(x, z, 0.6) {
                    seen[(c * n + a) as usize] = true;
                    q.push((a, c));
                }
            }
        }
        let reach = |x: f32, z: f32| {
            let (i, j) = (((x / cell).round() as i32 + n / 2), ((z / cell).round() as i32 + n / 2));
            (-1..=1).any(|di| (-1..=1).any(|dj| seen[((j + dj) * n + (i + di)) as usize]))
        };
        for (x, z, _) in clearings() {
            assert!(reach(x, z), "clearing at ({x},{z}) is cut off");
        }
        let r = base_pos(Team::Red);
        assert!(reach(r.x, r.z), "the red base is cut off");
    }

    /// The routing graph reaches every clearing from the blue base, and no
    /// leg of any route passes through rock.
    #[test]
    fn every_clearing_is_routable_without_crossing_rock() {
        let f = floor();
        let from = base_pos(Team::Blue);
        let mut goals: Vec<Vec3> = clearings().iter().map(|&(x, z, _)| Vec3::new(x, 0.0, z)).collect();
        goals.push(base_pos(Team::Red));
        for lane in LANES {
            for inner in [false, true] {
                goals.push(tower_pos(lane, Team::Red, inner));
            }
        }
        for g in goals {
            let r = f.route(from, g);
            let mut prev = from;
            for p in &r {
                for k in 0..=40 {
                    let q = prev.lerp(*p, k as f32 / 40.0);
                    assert!(f.sdf(q.x, q.z) < 0.0, "route to {g} crosses rock at {q} (leg {prev} -> {p})");
                }
                prev = *p;
            }
            assert_eq!(*r.last().unwrap(), g);
        }
    }

    #[test]
    fn the_peaks_tower_and_the_cliffs_are_sharp() {
        for &(px, pz) in &PEAKS {
            assert!(terrain_height(px, pz) > 90.0, "peak {}", terrain_height(px, pz));
        }
        // across the mid gorge's west wall: floor, then most of a cliff
        // within CLIFF_RISE
        let f = floor();
        let mut x = 0.0;
        while f.sdf(x, -60.0) < 0.0 {
            x -= 0.25;
        }
        let top = terrain_height(x - CLIFF_RISE, -60.0);
        assert!(top > 9.0, "cliff top only {top} u");
        assert!(terrain_height(x + 0.5, -60.0).abs() < 0.3, "floor at the foot");
    }

    #[test]
    fn the_falls_fall() {
        let fs = falls();
        assert_eq!(fs.len(), 8);
        for f in fs {
            assert!(f.height > 4.0, "a fall of {} u is a trickle", f.height);
            assert!((f.lip - f.foot).length() < 12.0);
        }
    }
}
