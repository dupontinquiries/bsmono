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

/// The map's scale. Every authored point below is multiplied by it: the pass
/// was laid out at 1.0 for 15 a side and grew by this much so thirty cats
/// have room to spread. One factor, so the shape is exactly the tested one,
/// only bigger. Lane WIDTHS grew by more than this (see `floor`).
pub const S: f32 = 1.3;

/// Playable radius and rendered-terrain half-extent on this map
/// (`consts::arena_radius`, `consts::ground_half`).
pub const ARENA_RADIUS: f32 = 218.0;
pub const GROUND_HALF: f32 = 300.0;

/// The two peaks' centres (x, z) and their height above the plateau.
pub const PEAKS: [(f32, f32); 2] = [(-60.0 * S, 0.0), (60.0 * S, 0.0)];
pub const PEAK_HEIGHT: f32 = 110.0;
pub const PEAK_WIDTH: f32 = 34.0 * S;

/// Base (nexus) distance from the centre, along z.
pub const BASE_Z: f32 = 138.0 * S;

pub fn base_pos(team: Team) -> Vec3 {
    Vec3::new(0.0, 0.0, -team.push_z() * BASE_Z)
}
pub fn fountain(team: Team) -> Vec3 {
    Vec3::new(0.0, 0.0, -team.push_z() * (BASE_Z + 9.0 * S))
}
pub fn titan_pos(team: Team) -> Vec3 {
    Vec3::new(0.0, 0.0, -team.push_z() * (BASE_Z - 12.0 * S))
}

/// An authored (blue-half) point in world space.
fn at(x: f32, z: f32) -> Vec3 {
    Vec3::new(x * S, 0.0, z * S)
}

fn mirror_z(pts: &[(f32, f32)]) -> Vec<Vec3> {
    // blue half as given, then the red half reflected, meeting on z = 0
    let mut v: Vec<Vec3> = pts.iter().map(|&(x, z)| at(x, z)).collect();
    let last_on_axis = pts.last().is_some_and(|p| p.1 == 0.0);
    let back: Vec<Vec3> = pts.iter().rev().skip(last_on_axis as usize).map(|&(x, z)| at(x, -z)).collect();
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
    let s = S * match (lane, inner) {
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
pub const LEDGES: [((f32, f32), (f32, f32)); 11] = [
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
pub const SLOTS: [((f32, f32), (f32, f32)); 8] = [
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
            s.extend(lane_shapes(lane));
        }
        for team in [Team::Blue, Team::Red] {
            let b = base_pos(team);
            s.push(Shape::Circle { x: b.x, z: b.z, r: 24.0 * S });
            let f = fountain(team);
            s.push(Shape::Circle { x: f.x, z: f.z, r: 12.0 * S });
        }
        for (x, z, r) in clearings() {
            s.push(Shape::Circle { x, z, r });
        }
        for &((ax, az), (bx, bz)) in &LEDGES {
            s.push(seg(at(ax, az), at(bx, bz), LEDGE_R));
            s.push(seg(at(ax, -az), at(bx, -bz), LEDGE_R));
        }
        for &((ax, az), (bx, bz)) in &SLOTS {
            s.push(seg(at(ax, az), at(bx, bz), SLOT_R));
            s.push(seg(at(ax, -az), at(bx, -bz), SLOT_R));
        }
        for c in camps() {
            s.push(Shape::Circle { x: c.at.x, z: c.at.z, r: c.r });
        }
        FloorPlan::new(s)
    })
}

/// Lane half-widths: wider than the scale alone gives (asked for: room for
/// thirty cats to spread), then pinched to `CHOKE_R` at the chokes.
pub const MID_R: f32 = 12.0;
pub const SIDE_R: f32 = 10.5;
pub const LEDGE_R: f32 = 4.0;
pub const SLOT_R: f32 = 5.4;
/// Half-width of a choke's narrows: a 7.6 u gap, room for a few cats
/// abreast and not a wave.
pub const CHOKE_R: f32 = 3.8;
/// Where each lane pinches, as fractions of its length from the blue end.
/// Mirrored pairs, so both teams meet each choke at the same distance. Just
/// beyond each outer tower, toward the centre -- and never at a ledge's
/// mouth: a ledge joining at 0.36 on Top widened that choke to 12.3 u
/// (`the_chokes_pinch_...` caught it).
pub const CHOKES: [(Lane, f32); 6] = [(Lane::Mid, 0.32), (Lane::Mid, 0.68), (Lane::Top, 0.3), (Lane::Top, 0.7), (Lane::Bot, 0.3), (Lane::Bot, 0.7)];
/// Length of the tapered approach either side of a choke's narrowest point.
const TAPER: f32 = 14.0;

/// A lane's floor: capsules along its centre-line, wide, narrowing into
/// each choke. A pinch has to be built this way round: the floor is a
/// UNION, so a narrow capsule over a wide one changes nothing. The wide
/// runs stop a full lane-width short of the choke (their round ends then
/// reach exactly its centre, at zero offset only) and short capsules of
/// shrinking radius fill the gap -- a funnel with curved walls.
pub fn lane_shapes(lane: Lane) -> Vec<Shape> {
    let pts = lane_polyline(lane);
    let r_wide = if lane == Lane::Mid { MID_R } else { SIDE_R };
    let len: f32 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    let chokes: Vec<f32> = CHOKES.iter().filter(|c| c.0 == lane).map(|c| c.1 * len).collect();
    // width at distance s along the lane
    let width = |s: f32| {
        chokes.iter().fold(r_wide, |w, &c| {
            let d = ((s - c).abs() / TAPER).clamp(0.0, 1.0);
            let t = d * d * (3.0 - 2.0 * d);
            w.min(CHOKE_R + (r_wide - CHOKE_R) * t)
        })
    };
    let seg = |a: Vec3, b: Vec3, r: f32| Shape::Segment { x0: a.x, z0: a.z, x1: b.x, z1: b.z, r };
    let mut out = Vec::new();
    // breakpoints: wide spans between chokes, fine steps through each funnel
    let mut marks = vec![0.0f32, len];
    // the polyline's own corners: a span between two breakpoints is ONE
    // straight capsule, and without these it cut across the lane's bends
    // (the first version pinched Top near its start and broke symmetry)
    let mut acc = 0.0;
    for w in pts.windows(2) {
        acc += (w[1] - w[0]).length();
        marks.push(acc);
    }
    for &c in &chokes {
        let (lo, hi) = (c - TAPER - r_wide, c + TAPER + r_wide);
        // an exact division of the funnel, so a choke and its mirror are
        // sampled at mirrored points (stepping 1.5 from `lo` was not, and
        // the two halves' walls differed by centimetres)
        let n = ((hi - lo) / 1.5).ceil().max(1.0) as usize;
        for k in 0..=n {
            marks.push((lo + (hi - lo) * k as f32 / n as f32).clamp(0.0, len));
        }
    }
    marks.sort_by(|a, b| a.partial_cmp(b).unwrap());
    marks.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
    for w in marks.windows(2) {
        let (s0, s1) = (w[0], w[1]);
        let in_funnel = chokes.iter().any(|&c| s1 > c - TAPER - r_wide && s0 < c + TAPER + r_wide);
        // in a funnel, a piece is as narrow as its narrower end AND must
        // not let its round cap bulge past the next piece's width
        let r = if in_funnel { width(s0).min(width(s1)) } else { r_wide };
        let (a, b) = (along(&pts, s0), along(&pts, s1));
        if in_funnel {
            out.push(seg(a, b, r));
        } else {
            // a wide span that ends at a funnel is pulled back by its radius
            // so its cap does not flood the narrows
            let near = |s: f32| chokes.iter().any(|&c| (s - (c - TAPER - r_wide)).abs() < 1e-3 || (s - (c + TAPER + r_wide)).abs() < 1e-3);
            let dir = (b - a).normalize_or_zero();
            let a2 = if near(s0) { a + dir * (r_wide - width(s0)).max(0.0) } else { a };
            let b2 = if near(s1) { b - dir * (r_wide - width(s1)).max(0.0) } else { b };
            out.push(seg(a2, b2, r));
        }
    }
    out
}

/// A shallow cave over a stretch of side path: the roof spans the ledge
/// from `a` to `b` and rests on the walls either side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cave {
    pub a: Vec3,
    pub b: Vec3,
    /// Half-width of the floor under the roof.
    pub half: f32,
    /// Height of the roof's underside.
    pub height: f32,
}

impl Cave {
    /// Is (x, z) under the roof?
    pub fn covers(&self, x: f32, z: f32) -> bool {
        let p = Vec3::new(x, 0.0, z);
        let ab = self.b - self.a;
        let t = (p - self.a).dot(ab) / ab.length_squared().max(1e-6);
        (0.0..=1.0).contains(&t) && (self.a + ab * t - p).length() <= self.half + 0.5
    }
    pub fn mid(&self) -> Vec3 {
        (self.a + self.b) * 0.5
    }
}

/// The caves: the middle 60% of the two long shoulder-to-side-lane ledges
/// (LEDGES 4 and 5), mirrored -- four in all.
pub fn caves() -> Vec<Cave> {
    let mut v = Vec::new();
    for i in [4usize, 5] {
        let ((ax, az), (bx, bz)) = LEDGES[i];
        for zs in [1.0f32, -1.0] {
            let (a, b) = (at(ax, az * zs), at(bx, bz * zs));
            // 6.2: at a mouth the lip (1.4), its noise (0.9) and a stalactite (1.4)
            // still leave 2.5 u -- at 5.5 the deepest point was 1.8, under a cat
            v.push(Cave { a: a.lerp(b, 0.2), b: a.lerp(b, 0.8), half: LEDGE_R, height: 6.2 });
        }
    }
    v
}

/// The side paths -- ledges and slots, both halves -- as (a, b, half-width).
pub fn side_paths() -> Vec<(Vec3, Vec3, f32)> {
    let mut v = Vec::new();
    for zs in [1.0f32, -1.0] {
        for &((ax, az), (bx, bz)) in &LEDGES {
            v.push((at(ax, az * zs), at(bx, bz * zs), LEDGE_R));
        }
        for &((ax, az), (bx, bz)) in &SLOTS {
            v.push((at(ax, az * zs), at(bx, bz * zs), SLOT_R));
        }
    }
    v
}

/// Distance from (x, z) to the nearest side path's centre-line.
pub fn side_path_dist(x: f32, z: f32) -> f32 {
    let p = Vec3::new(x, 0.0, z);
    side_paths()
        .iter()
        .map(|&(a, b, _)| {
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            (a + ab * t - p).length()
        })
        .fold(f32::MAX, f32::min)
}

/// A pocket of floor off a side path, kept clear for a jungle camp (vole
/// monsters, when the sim has them): nothing is placed in it but its burrow
/// mound at the back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camp {
    pub at: Vec3,
    pub r: f32,
    /// Unit vector from the path into the pocket (where the burrow is).
    pub back: Vec3,
}

/// Camp pockets: one off each of four ledges per half, pushed out to the
/// side of the path so the path runs past the camp's mouth, not through it.
/// Blue half authored, mirrored.
pub fn camps() -> Vec<Camp> {
    // (ledge index, along 0..1, side +1/-1)
    const SPOTS: [(usize, f32, f32); 4] = [(0, 0.5, 1.0), (2, 0.5, -1.0), (6, 0.45, -1.0), (7, 0.45, 1.0)];
    let mut v = Vec::new();
    for zs in [1.0f32, -1.0] {
        for &(i, f, side) in &SPOTS {
            let ((ax, az), (bx, bz)) = LEDGES[i];
            let (a, b) = (at(ax, az * zs), at(bx, bz * zs));
            let dir = (b - a).normalize_or_zero();
            let across = Vec3::new(-dir.z, 0.0, dir.x) * side * zs;
            let r = 7.5;
            v.push(Camp { at: a.lerp(b, f) + across * (LEDGE_R + r - 2.5), r, back: across });
        }
    }
    v
}

/// A place where the rock is cut down: a sightline gap (a low sill where a
/// thin wall stands between a side path and a lane) or a diorama niche (a
/// shelf cut into a wall at knee height). Rock only -- the floor plan does
/// not change, so nothing becomes walkable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cap {
    pub a: Vec3,
    pub b: Vec3,
    pub r: f32,
    /// Height the rock is cut down to.
    pub h: f32,
    pub niche: bool,
}

impl Cap {
    fn dist(&self, x: f32, z: f32) -> f32 {
        let p = Vec3::new(x, 0.0, z);
        let ab = self.b - self.a;
        let t = ((p - self.a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
        (self.a + ab * t - p).length()
    }
}

/// Sightline gaps: at the middle of each side path, if a lane runs within
/// 18 u beside it with only rock between, a sill 1.3 u high and 3.5 u
/// either side of the line across. Niches: every ~38 u along the side
/// paths, on alternating walls, clear of camps, gaps and caves.
pub fn caps() -> &'static [Cap] {
    static C: OnceLock<Vec<Cap>> = OnceLock::new();
    C.get_or_init(|| {
        let f = floor();
        let mut out: Vec<Cap> = Vec::new();
        let lanes: Vec<Vec<Vec3>> = crate::consts::LANES.iter().map(|&l| lane_polyline(l)).collect();
        let near_lane = |p: Vec3| {
            let mut best = (f32::MAX, Vec3::ZERO);
            for pts in &lanes {
                for w in pts.windows(2) {
                    let ab = w[1] - w[0];
                    let t = ((p - w[0]).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                    let q = w[0] + ab * t;
                    let d = (q - p).length();
                    if d < best.0 {
                        best = (d, q);
                    }
                }
            }
            best
        };
        let caves = caves();
        let camp_list = camps();
        let clear = |p: Vec3, out: &[Cap]| {
            camp_list.iter().all(|c| (c.at - p).length() > c.r + 6.0)
                && caves.iter().all(|c| !c.covers(p.x, p.z) && (c.a - p).length() > 6.0 && (c.b - p).length() > 6.0)
                && out.iter().all(|c| (c.a.lerp(c.b, 0.5) - p).length() > 12.0)
        };
        let mut k = 0;
        for (a, b, half) in side_paths() {
            if a.z > 0.5 || b.z > 0.5 {
                continue; // blue half; mirrored below
            }
            let len = (b - a).length();
            let dir = (b - a).normalize_or_zero();
            let across = Vec3::new(-dir.z, 0.0, dir.x);
            // A sightline gap: look sideways from the path, both ways, at a
            // few points along it. A thin wall -- 3 to 16 u of rock -- with
            // floor beyond (a lane, a clearing, another path) is cut to a
            // sill for a stretch. (First version looked only toward the
            // nearest LANE, which on this layout is always through the
            // open junction where the path meets it: zero gaps.)
            'gap: for fr in [0.35f32, 0.5, 0.65, 0.25, 0.75] {
                for side in [1.0f32, -1.0] {
                    let m = a.lerp(b, fr);
                    let from = m + across * side * (half + 0.3);
                    let mut t = 0.0;
                    let mut left_floor = false;
                    let mut to = None;
                    while t < 18.0 {
                        let p = from + across * side * t;
                        let d = f.sdf(p.x, p.z);
                        if d > 0.0 {
                            left_floor = true;
                        } else if left_floor {
                            to = Some(p);
                            break;
                        }
                        t += 0.5;
                    }
                    let Some(to) = to else { continue };
                    let thick = (to - from).length();
                    let mid = from.lerp(to, 0.5);
                    if (3.0..=16.0).contains(&thick) && clear(mid, &out) {
                        out.push(Cap { a: from, b: to, r: 3.5, h: 1.3, niche: false });
                        break 'gap;
                    }
                }
            }
            // niches along it
            let mut s = 14.0;
            while s < len - 10.0 {
                let side = if k % 2 == 0 { 1.0 } else { -1.0 };
                let p = a + dir * s + across * side * (half + 1.4);
                if clear(p, &out) && f.sdf(p.x, p.z) > 0.4 && (0.3..3.2).contains(&f.sdf(p.x, p.z)) {
                    out.push(Cap { a: p, b: p + across * side * 1.6, r: 1.9, h: 1.1, niche: true });
                }
                k += 1;
                s += 38.0;
            }
        }
        let mirrored: Vec<Cap> = out.iter().map(|c| Cap { a: Vec3::new(c.a.x, 0.0, -c.a.z), b: Vec3::new(c.b.x, 0.0, -c.b.z), ..*c }).collect();
        out.extend(mirrored);
        out
    })
}

/// The climb: the Top lane -- the solo lane round the outside of the west
/// peak -- rises onto a shelf this high around the far side of the peak.
pub const SHELF_H: f32 = 9.0;
/// Where along Top (fractions of its length) the ramp up starts and tops
/// out, and the ramp down starts and ends. Between the chokes (0.3, 0.7),
/// clear of the stream that joins at ~0.28.
pub const SHELF: [f32; 4] = [0.38, 0.46, 0.54, 0.62];
/// How far past the lane's edge the shelf's height carries before it
/// fades to the surrounding floor (a slot joining the shelf ramps up
/// over this distance).
const SHELF_FADE: f32 = 14.0;

/// Height of the floor at (x, z): 0 except on and around the Top lane's
/// shelf. The sim's ground (`map::ground_y`), the terrain's floor, and
/// where scenery on the shelf stands.
pub fn floor_height(x: f32, z: f32) -> f32 {
    // cached polyline and its length
    static TOP: OnceLock<(Vec<Vec3>, f32)> = OnceLock::new();
    let (pts, len) = TOP.get_or_init(|| {
        let p = lane_polyline(Lane::Top);
        let l = p.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        (p, l)
    });
    // cheap reject: the shelf is on the far west
    if x > -120.0 {
        return 0.0;
    }
    let p = Vec3::new(x, 0.0, z);
    let (mut best_d, mut best_s, mut acc) = (f32::MAX, 0.0, 0.0);
    for w in pts.windows(2) {
        let ab = w[1] - w[0];
        let l = ab.length();
        let t = ((p - w[0]).dot(ab) / (l * l).max(1e-6)).clamp(0.0, 1.0);
        let d = (w[0] + ab * t - p).length();
        if d < best_d {
            best_d = d;
            best_s = acc + t * l;
        }
        acc += l;
    }
    let f = best_s / len;
    let sm = |a: f32, b: f32, x: f32| {
        let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let along = sm(SHELF[0], SHELF[1], f) * (1.0 - sm(SHELF[2], SHELF[3], f));
    let off = (best_d - SIDE_R).max(0.0);
    let across = 1.0 - sm(0.0, SHELF_FADE, off);
    let h = SHELF_H * along * across;
    if h <= 0.0 {
        return 0.0;
    }
    // Water does not run uphill: the stream that joins Top near the foot of
    // the up-ramp keeps its bed at 0, and the shelf blends up away from it
    // (the test found its lower reach inside the shelf's sideways fade).
    let rd = river_dist(x, z);
    h * sm(STREAM_HALF_WIDTH + 1.0, STREAM_HALF_WIDTH + 12.0, rd)
}

/// The narrowest point of every choke, and the lane's direction there.
pub fn choke_points() -> Vec<(Vec3, Vec3)> {
    CHOKES
        .iter()
        .map(|&(lane, f)| {
            let pts = lane_polyline(lane);
            let len: f32 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
            let p = along(&pts, f * len);
            let q = along(&pts, f * len + 1.0);
            (p, (q - p).normalize_or_zero())
        })
        .collect()
}

/// The clearings, both halves: (x, z, radius).
pub fn clearings() -> Vec<(f32, f32, f32)> {
    let mut v: Vec<_> = CLEARINGS.iter().flat_map(|&(x, z, r)| [(x * S, z * S, r * S), (x * S, -z * S, r * S)]).collect();
    v.extend(CENTRE_CLEARINGS.iter().map(|&(x, z, r)| (x * S, z * S, r * S)));
    v
}

/// Boss grounds: the two slot clearings in the massifs.
pub fn pit_positions() -> [Vec3; 2] {
    [at(CENTRE_CLEARINGS[0].0, 0.0), at(CENTRE_CLEARINGS[1].0, 0.0)]
}

/// Heal trees -- here, sheltered springs: in the six mid-height clearings.
pub fn heal_tree_positions() -> [Vec3; 6] {
    let c = at;
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
            v.push(s.iter().map(|&(x, z)| at(x, z)).collect::<Vec<_>>());
            v.push(s.iter().map(|&(x, z)| at(x, -z)).collect());
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
    let out = ((r - ARENA_RADIUS) / (GROUND_HALF - ARENA_RADIUS)).clamp(0.0, 1.0);
    h + out * out * 75.0 * (0.7 + 0.6 * ridge(x * 0.03 + 9.0, z * 0.03))
}

/// Width over which a cliff rises from the floor to its top. Small: sharp.
pub const CLIFF_RISE: f32 = 2.6;

/// Terrain height: the floor (with stream beds cut into it), then cliffs.
pub fn terrain_height(x: f32, z: f32) -> f32 {
    floor_height(x, z) + terrain_height_flat(x, z)
}

/// The terrain as if the floor were all at 0: the floor's surface detail
/// and stream beds, then the cliffs.
fn terrain_height_flat(x: f32, z: f32) -> f32 {
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
    let h = cliff_top(x, z) * s;
    // gaps and niches: the rock cut down, with a short soft edge
    let mut out = h;
    for c in caps() {
        let e = c.dist(x, z) - c.r;
        if e < 1.2 {
            let k = (e / 1.2).clamp(0.0, 1.0);
            let k = k * k * (3.0 - 2.0 * k);
            out = out.min(c.h + (h - c.h) * k);
        }
    }
    out
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

    /// Width of the floor across the lane at `p` (direction `dir`).
    fn span(p: Vec3, dir: Vec3) -> f32 {
        let side = Vec3::new(-dir.z, 0.0, dir.x);
        let f = floor();
        let edge = |sgn: f32| {
            let mut d = 0.0;
            while f.sdf(p.x + side.x * d * sgn, p.z + side.z * d * sgn) < 0.0 && d < 80.0 {
                d += 0.1;
            }
            d
        };
        edge(1.0) + edge(-1.0)
    }

    /// Every choke is a real narrows -- about 2 * CHOKE_R across -- with the
    /// full lane either side of it, and a big cat still fits through.
    #[test]
    fn the_chokes_pinch_and_the_lanes_are_wide_between_them() {
        let pts = choke_points();
        assert_eq!(pts.len(), CHOKES.len());
        for (p, dir) in pts {
            let w = span(p, dir);
            assert!((w - 2.0 * CHOKE_R).abs() < 1.0, "choke at {p} is {w:.1} u across, want ~{}", 2.0 * CHOKE_R);
            assert!(floor().fits(p.x, p.z, 1.2), "a big cat cannot get through the choke at {p}");
            // 25 u either way along the lane it is wide again
            for k in [-25.0f32, 25.0] {
                let q = p + dir * k;
                let wq = span(q, dir);
                assert!(wq > 2.0 * SIDE_R - 2.5, "not wide {k} u from the choke at {p}: {wq:.1}");
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
        let (cell, n) = (2.0f32, 240i32);
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

    /// The climb: Top rises to SHELF_H at its middle and nowhere else is
    /// raised; the ramps are walkable; streams and clearings stay at 0; the
    /// shelf is mirrored like the rest of the map.
    #[test]
    fn the_top_lane_climbs_onto_a_shelf_and_nothing_else_rises() {
        let top = lane_polyline(Lane::Top);
        let len: f32 = top.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        let h = |f: f32| {
            let p = along(&top, f * len);
            floor_height(p.x, p.z)
        };
        assert!((h(0.5) - SHELF_H).abs() < 1e-3, "crest {}", h(0.5));
        assert!(h(0.2) < 1e-3 && h(0.8) < 1e-3, "the ends stay down");
        // walkable ramps: slope along the lane at most ~0.35 (about 19 deg)
        let mut s = 0.0;
        while s < len - 1.0 {
            let (a, b) = (along(&top, s), along(&top, s + 1.0));
            let dh = (floor_height(b.x, b.z) - floor_height(a.x, a.z)).abs();
            assert!(dh <= 0.35, "too steep at {a}: {dh} per unit");
            s += 1.0;
        }
        for lane in [Lane::Mid, Lane::Bot] {
            for p in lane_polyline(lane) {
                assert_eq!(floor_height(p.x, p.z), 0.0, "{lane:?} raised at {p}");
            }
        }
        for s in streams() {
            for p in s {
                assert!(floor_height(p.x, p.z) < 1e-3, "a stream on the shelf at {p}");
            }
        }
        for (x, z, _) in clearings() {
            if !crate::bosses::in_pit(x, z) {
                assert!(floor_height(x, z) < 1e-3, "a clearing raised at ({x},{z})");
            }
        }
        for x in (-230..-120).step_by(3) {
            for z in (-80..80).step_by(3) {
                let (x, z) = (x as f32 + 0.4, z as f32 + 0.3);
                assert!((floor_height(x, z) - floor_height(x, -z)).abs() < 1e-3, "shelf not mirrored at ({x},{z})");
            }
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
