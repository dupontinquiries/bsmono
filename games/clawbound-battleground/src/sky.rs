//! sky — Starwell Glacier: glacier islands adrift above a sea of cloud,
//! in the spirit world, under a night that does not end. 15 a side.
//!
//! Two wide lanes (west and east -- `Lane::Top` and `Lane::Bot`) run stone
//! roads over the islands from base to base. Between them, the jungle is a
//! scatter of glacier islands joined by causeways and, across the open
//! chasms, by **cloud ferries** (`ferry`). At the centre, on an island of
//! its own, is the **Moonwell** (`moonwell`): shallow healing water with the
//! Moonwyrm coiled in it. Two long star-bridges on the z = 0 line reach it
//! from the lanes; the ferries are the quick way.
//!
//! ## How the ground works
//!
//! As on the pass, the walkable floor is a union of capsules and discs
//! (`floor()`), and outside it the sim will not walk. What is outside is
//! not cliff but SKY: `terrain_height` rises a little into an icy rim at the
//! floor's edge, then drops away to far below the cloud sea. Everything
//! walkable is at y = 0 (`floor_height`).
//!
//! ## Symmetry
//!
//! Mirrored in z like the other maps: every authored point is for the blue
//! (z < 0) half or on z = 0. `tests::the_map_is_fair` holds it to that.
//! The Mid lane does not exist for minions or towers here (`map::lanes`);
//! `Lane::Mid`'s polyline is the sky route over the Moonwell, for cameras.

use glam::Vec3;
use std::sync::OnceLock;

use crate::blockers::Shape;
use crate::consts::{Lane, Team};
use crate::floor::FloorPlan;

/// Players per team.
pub const TEAM_SIZE: usize = 15;

/// Twice the Pass's area: its 218 u radius times sqrt 2.
pub const ARENA_RADIUS: f32 = 310.0;
/// The rendered square reaches well past the rim: sky and cloud all round.
pub const GROUND_HALF: f32 = 400.0;

/// Base (nexus) distance from the centre, along z.
pub const BASE_Z: f32 = 252.0;

/// The lanes' half-width: half again the pass's side lanes (10.5).
pub const LANE_R: f32 = 16.0;
/// Causeways between islands.
pub const CAUSEWAY_R: f32 = 5.5;
/// The two star-bridges to the Moonwell: narrow and long.
pub const BRIDGE_R: f32 = 4.0;

/// The Moonwell island and its pool.
pub const WELL_ISLAND_R: f32 = 50.0;
pub const POOL_R: f32 = 22.0;

pub fn base_pos(team: Team) -> Vec3 {
    Vec3::new(0.0, 0.0, -team.push_z() * BASE_Z)
}
pub fn fountain(team: Team) -> Vec3 {
    Vec3::new(0.0, 0.0, -team.push_z() * (BASE_Z + 13.0))
}
pub fn titan_pos(team: Team) -> Vec3 {
    Vec3::new(0.0, 0.0, -team.push_z() * (BASE_Z - 16.0))
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

/// The west lane (Top), blue half. Bot is its mirror in x.
const WEST: [(f32, f32); 7] = [(0.0, -222.0), (-58.0, -214.0), (-128.0, -182.0), (-182.0, -128.0), (-208.0, -64.0), (-214.0, -20.0), (-214.0, 0.0)];

/// Lane centre-lines, blue base mouth to red. `Mid` is the sky route over
/// the Moonwell: not walkable end to end (it crosses open sky), and used
/// only by cameras and the intro.
pub fn lane_polyline(lane: Lane) -> Vec<Vec3> {
    match lane {
        Lane::Top => mirror_z(&WEST),
        Lane::Bot => mirror_z(&WEST.map(|(x, z)| (-x, z))),
        Lane::Mid => mirror_z(&[(0.0, -222.0), (0.0, -150.0), (0.0, -60.0), (0.0, 0.0)]),
    }
}

/// Towers by distance along the lane from the team's end.
pub fn tower_pos(lane: Lane, team: Team, inner: bool) -> Vec3 {
    let mut pts = lane_polyline(lane);
    if team == Team::Red {
        pts.reverse();
    }
    crate::pass::along(&pts, if inner { 60.0 } else { 175.0 })
}

/// A circular island (blue half; mirrored unless on z = 0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Island {
    pub x: f32,
    pub z: f32,
    pub r: f32,
    pub name: &'static str,
}

/// The jungle islands of the blue half. The glades sit between the lanes
/// and the Moonwell; the terrace is below the base; the shoals lie outside
/// the lanes and are reached only by cloud.
pub const ISLANDS: [Island; 5] = [
    Island { x: -104.0, z: -96.0, r: 27.0, name: "the West Glade" },
    Island { x: 104.0, z: -96.0, r: 27.0, name: "the East Glade" },
    Island { x: 0.0, z: -150.0, r: 25.0, name: "the Frost Terrace" },
    Island { x: -245.0, z: -135.0, r: 18.0, name: "the West Shoal" },
    Island { x: 245.0, z: -135.0, r: 18.0, name: "the East Shoal" },
];

/// Walkable causeways (blue half, mirrored): base to terrace, terrace to
/// the glades, the glades out to their lanes.
pub const CAUSEWAYS: [((f32, f32), (f32, f32)); 5] = [
    ((0.0, -222.0), (0.0, -150.0)),
    ((0.0, -150.0), (-104.0, -96.0)),
    ((0.0, -150.0), (104.0, -96.0)),
    ((-104.0, -96.0), (-190.0, -110.0)),
    ((104.0, -96.0), (190.0, -110.0)),
];

/// The star-bridges, on z = 0, lane to Moonwell.
pub const BRIDGES: [((f32, f32), (f32, f32)); 2] = [((-214.0, 0.0), (-WELL_ISLAND_R + 2.0, 0.0)), ((214.0, 0.0), (WELL_ISLAND_R - 2.0, 0.0))];

/// Cloud-ferry routes (blue half, mirrored): pier to pier across a chasm.
/// Piers are points on the floor's edge; the cloud docks just off each
/// (`ferry`).
pub const FERRIES: [((f32, f32), (f32, f32)); 3] = [
    // glade to Moonwell: the quick way to the centre
    ((-84.0, -78.0), (-36.0, -35.0)),
    ((84.0, -78.0), (36.0, -35.0)),
    // the west lane's outer edge out to its shoal
    ((-197.0, -130.0), (-228.0, -133.0)),
];

/// A linking way (causeway or star-bridge) as walked: a polyline of
/// centre points, and the half-width at each.
#[derive(Debug, Clone, PartialEq)]
pub struct Way {
    pub pts: Vec<Vec3>,
    pub r: Vec<f32>,
}

/// Points along a way.
const WAY_STEPS: usize = 14;

/// A causeway from `a` to `b`, `r` wide, as it is walked: not a ruler line
/// but a gentle bend (one way or the other by `k`), a second smaller
/// wander, and a widening near the middle -- a landing where it is wide
/// enough to fight -- narrowing again toward each end.
fn causeway_way(a: Vec3, b: Vec3, r: f32, k: usize) -> Way {
    let d = b - a;
    let len = d.length();
    let side = Vec3::new(-d.z, 0.0, d.x) / len.max(1e-3);
    let bend = if k % 2 == 0 { 0.09 } else { -0.08 } * len;
    let wander = (0.03 + 0.01 * (k % 3) as f32) * len;
    let (mut pts, mut rr) = (Vec::new(), Vec::new());
    for i in 0..=WAY_STEPS {
        let t = i as f32 / WAY_STEPS as f32;
        let s = (std::f32::consts::PI * t).sin();
        pts.push(a + d * t + side * (bend * s + wander * (2.0 * std::f32::consts::PI * t + k as f32).sin() * s));
        rr.push(r * (0.85 + 0.45 * s.powi(4)));
    }
    Way { pts, r: rr }
}

/// A star-bridge: it stays on z = 0 (the line between the halves, so
/// neither team's approach is shorter) and varies in width instead, swelling
/// into three small landings along its span.
fn bridge_way(a: Vec3, b: Vec3, r: f32) -> Way {
    let (mut pts, mut rr) = (Vec::new(), Vec::new());
    for i in 0..=WAY_STEPS {
        let t = i as f32 / WAY_STEPS as f32;
        pts.push(a.lerp(b, t));
        rr.push(r * (0.9 + 0.55 * (3.0 * std::f32::consts::PI * t).sin().abs().powi(3)));
    }
    Way { pts, r: rr }
}

/// Every linking way: the causeways of both halves (the red half the blue
/// half mirrored, so the two are the same walk), then the two star-bridges.
pub fn ways() -> Vec<Way> {
    static W: OnceLock<Vec<Way>> = OnceLock::new();
    W.get_or_init(ways_built).clone()
}

fn ways_built() -> Vec<Way> {
    let mut v = Vec::new();
    for zs in [1.0f32, -1.0] {
        for (k, &((ax, az), (bx, bz))) in CAUSEWAYS.iter().enumerate() {
            v.push(causeway_way(at(ax, az * zs), at(bx, bz * zs), CAUSEWAY_R, k));
        }
    }
    // the mirror of a bend is a bend the other way: mirror the points, not
    // the recipe
    let n = CAUSEWAYS.len();
    for k in 0..n {
        let blue = v[k].clone();
        v[n + k] = Way { pts: blue.pts.iter().map(|p| Vec3::new(p.x, p.y, -p.z)).collect(), r: blue.r };
    }
    for &((ax, az), (bx, bz)) in &BRIDGES {
        v.push(bridge_way(at(ax, az), at(bx, bz), BRIDGE_R));
    }
    v
}

fn way_dist(w: &Way, p: Vec3) -> f32 {
    w.pts.windows(2).map(|s| seg_dist(p, s[0], s[1])).fold(f32::MAX, f32::min)
}

/// All ferry routes, both halves.
pub fn ferry_routes() -> Vec<(Vec3, Vec3)> {
    let mut v = Vec::new();
    for zs in [1.0f32, -1.0] {
        for &((ax, az), (bx, bz)) in &FERRIES {
            v.push((at(ax, az * zs), at(bx, bz * zs)));
        }
    }
    v
}

/// Every island, both halves, plus the Moonwell island.
pub fn islands() -> Vec<Island> {
    let mut v: Vec<Island> = Vec::new();
    for i in ISLANDS {
        v.push(i);
        v.push(Island { z: -i.z, ..i });
    }
    v.push(Island { x: 0.0, z: 0.0, r: WELL_ISLAND_R, name: "the Moonwell" });
    v
}

/// The Aerie: the upper layer, [`UPPER_Y`] above the islands, over the
/// inner chasms between the Moonwell, the glades and the terrace. Small on
/// purpose -- three islets a half -- and every islet joined to both others
/// by a sky-bridge and served by its own lift, so there are always three
/// ways up and a fight can come from any side: a lookout, not a fortress.
/// Nothing of it lies over the lower floor (the two layers never touch in
/// plan: `tests::the_aerie_floats_clear_of_the_islands`).
pub const UPPER_Y: f32 = 20.0;
/// The Aerie's islets (blue half, mirrored): x, z, radius.
// (The side islets sat at (+-45, -80) first, in the chasm the Moonwyrm's
// loop dives through; `wyrmflight`'s test keeps the loop wide of them.)
pub const AERIE: [(f32, f32, f32); 3] = [(0.0, -100.0, 13.0), (-21.0, -72.0, 10.0), (21.0, -72.0, 10.0)];
/// Its sky-bridges, islet to islet: every pair.
pub const AERIE_BRIDGES: [(usize, usize); 3] = [(0, 1), (0, 2), (1, 2)];
pub const AERIE_BRIDGE_R: f32 = 3.0;
/// The lifts (blue half, mirrored): a pad on the islands below and one on
/// an islet of the Aerie above (`lift`).
pub const LIFTS: [((f32, f32), (f32, f32)); 3] = [((0.0, -134.0), (0.0, -106.0)), ((-92.0, -104.0), (-26.0, -76.0)), ((92.0, -104.0), (26.0, -76.0))];

/// The Aerie's walkable shapes, both halves, at plan level (their height is
/// `floor_height`).
fn aerie_shapes() -> Vec<Shape> {
    let mut s = Vec::new();
    let isle = |k: usize| at(AERIE[k].0, AERIE[k].1);
    for zs in [1.0f32, -1.0] {
        for (x, z, r) in AERIE {
            s.push(Shape::Circle { x, z: z * zs, r });
        }
        for (k, &(a, b)) in AERIE_BRIDGES.iter().enumerate() {
            // a gentle bend, like the causeways below: laid for the blue
            // half and mirrored point for point, so both halves are one walk
            let w = causeway_way(isle(a), isle(b), AERIE_BRIDGE_R, k + 1);
            let w = Way { pts: w.pts.iter().map(|p| Vec3::new(p.x, p.y, p.z * zs)).collect(), r: w.r };
            for i in 0..w.pts.len() - 1 {
                s.push(Shape::Segment { x0: w.pts[i].x, z0: w.pts[i].z, x1: w.pts[i + 1].x, z1: w.pts[i + 1].z, r: (w.r[i] + w.r[i + 1]) * 0.5 });
            }
        }
    }
    s
}

/// The Aerie's floor alone.
pub fn upper_floor() -> &'static FloorPlan {
    static F: OnceLock<FloorPlan> = OnceLock::new();
    F.get_or_init(|| FloorPlan::new(aerie_shapes()))
}

/// The islands' floor alone (everything below the Aerie).
pub fn lower_floor() -> &'static FloorPlan {
    static F: OnceLock<FloorPlan> = OnceLock::new();
    F.get_or_init(|| FloorPlan::new(lower_shapes()))
}

/// Every lift, both halves: (pad below, pad above).
pub fn lifts() -> Vec<(Vec3, Vec3)> {
    let mut v = Vec::new();
    for zs in [1.0f32, -1.0] {
        for &((ax, az), (bx, bz)) in &LIFTS {
            v.push((at(ax, az * zs), Vec3::new(bx, UPPER_Y, bz * zs)));
        }
    }
    v
}

/// The walkable floor: the islands and the Aerie together (they never
/// touch in plan, so walking cannot get from one to the other; the lifts
/// do that, and a leap down).
pub fn floor() -> &'static FloorPlan {
    static F: OnceLock<FloorPlan> = OnceLock::new();
    F.get_or_init(|| {
        let mut s = lower_shapes();
        s.extend(aerie_shapes());
        FloorPlan::new(s)
    })
}

fn lower_shapes() -> Vec<Shape> {
    {
        let mut s = Vec::new();
        let seg = |a: Vec3, b: Vec3, r: f32| Shape::Segment { x0: a.x, z0: a.z, x1: b.x, z1: b.z, r };
        for lane in [Lane::Top, Lane::Bot] {
            let pts = lane_polyline(lane);
            for w in pts.windows(2) {
                s.push(seg(w[0], w[1], LANE_R));
            }
        }
        for team in [Team::Blue, Team::Red] {
            let b = base_pos(team);
            s.push(Shape::Circle { x: b.x, z: b.z, r: 36.0 });
            let f = fountain(team);
            s.push(Shape::Circle { x: f.x, z: f.z, r: 18.0 });
        }
        for i in islands() {
            s.push(Shape::Circle { x: i.x, z: i.z, r: i.r });
        }
        // the linking ways, bending and swelling (`ways`): a segment per
        // step at the mean of its ends' widths, and a disc at each joint so
        // the edge runs smooth
        for w in ways() {
            for i in 0..w.pts.len() - 1 {
                s.push(seg(w.pts[i], w.pts[i + 1], (w.r[i] + w.r[i + 1]) * 0.5));
            }
            for (p, r) in w.pts.iter().zip(&w.r) {
                s.push(Shape::Circle { x: p.x, z: p.z, r: *r });
            }
        }
        s
    }
}

/// Is (x, z) the Aerie's (nearer its floor than the islands')?
pub fn on_aerie(x: f32, z: f32) -> bool {
    upper_floor().sdf(x, z) < lower_floor().sdf(x, z)
}

/// The walkable floor's height: the islands at 0, the Aerie at `UPPER_Y`.
pub fn floor_height(x: f32, z: f32) -> f32 {
    if on_aerie(x, z) {
        UPPER_Y
    } else {
        0.0
    }
}

/// The two wild beasts' grounds (`bosses`): on the west glade of the blue
/// half and the east glade of the red -- each team's own flank, turned
/// half a circle from the other's (the Moonwyrm is the one in the middle).
pub fn pit_positions() -> [Vec3; 2] {
    [at(-98.0, -90.0), at(98.0, 90.0)]
}

/// The Moonwell's centre.
pub fn well_pos() -> Vec3 {
    Vec3::ZERO
}

/// Heal springs: at the back of each glade, both halves, and one on each
/// shoal -- a reason to ride the cloud out there.
pub fn heal_tree_positions() -> [Vec3; 6] {
    [at(-116.0, -110.0), at(116.0, -110.0), at(-116.0, 110.0), at(116.0, 110.0), at(-249.0, -139.0), at(249.0, 139.0)]
}

/// Distance to water: the Moonwell is the only water on the map.
pub fn river_dist(x: f32, z: f32) -> f32 {
    (x.hypot(z) - POOL_R).max(0.0)
}

/// Is (x, z) in the Moonwell's water?
pub fn in_pool(x: f32, z: f32) -> bool {
    x.hypot(z) < POOL_R
}

/// Depth of the pool's water above its bed at (x, z): shallow, a paw deep
/// at the rim and a leg deep at the middle.
pub const POOL_DEPTH: f32 = 0.6;

/// Marble statues (blue half, mirrored): (x, z, facing yaw). Along the
/// lanes at the towers' feet and in a ring round the well.
pub fn statues() -> Vec<(Vec3, f32)> {
    let mut v = Vec::new();
    for lane in [Lane::Top, Lane::Bot] {
        let pts = lane_polyline(lane);
        let len: f32 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        for f in [0.18f32, 0.36, 0.5, 0.64, 0.82] {
            let p = crate::pass::along(&pts, f * len);
            let q = crate::pass::along(&pts, f * len + 1.0);
            let dir = (q - p).normalize_or_zero();
            let side = Vec3::new(-dir.z, 0.0, dir.x);
            for s in [1.0f32, -1.0] {
                let at = p + side * s * (LANE_R - 2.5);
                // not in the mouth of a star-bridge or a causeway
                if bridge_dist(at.x, at.z) < BRIDGE_R + 6.0 || causeway_dist(at.x, at.z) < CAUSEWAY_R + 6.0 {
                    continue;
                }
                // facing the road: yaw 0 is +z, as the statue mesh faces
                v.push((at, (-side.x * s).atan2(-side.z * s)));
            }
        }
    }
    // the ring round the well: eight, facing the water
    for k in 0..8 {
        let a = k as f32 / 8.0 * std::f32::consts::TAU + std::f32::consts::FRAC_PI_8;
        let p = at(a.cos() * (POOL_R + 9.0), a.sin() * (POOL_R + 9.0));
        v.push((p, (-p.x).atan2(-p.z)));
    }
    v
}

fn seg_dist(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t - p).length()
}

/// Distance to the nearest star-bridge's centre-line.
pub fn bridge_dist(x: f32, z: f32) -> f32 {
    let p = at(x, z);
    ways().iter().skip(2 * CAUSEWAYS.len()).map(|w| way_dist(w, p)).fold(f32::MAX, f32::min)
}

/// Distance to the nearest causeway's centre-line (both halves), as it
/// bends.
pub fn causeway_dist(x: f32, z: f32) -> f32 {
    let p = at(x, z);
    ways().iter().take(2 * CAUSEWAYS.len()).map(|w| way_dist(w, p)).fold(f32::MAX, f32::min)
}

fn hash2(x: f32, z: f32) -> f32 {
    let v = (x * 127.1 + z * 311.7).sin() * 43758.547;
    v - v.floor()
}

/// Far below the islands, the cloud deck (drawn by the sky shader: there is
/// no ground under Starwell Glacier, only mist, floating stone and cloud).
pub const DECK_Y: f32 = -260.0;
/// Where an island ends: just past the outside of its icy lip. Beyond it
/// is open sky (the terrain mesh's top has no triangles there).
pub const EDGE: f32 = RIM_W + 0.6;

/// Open sky: past every island's edge, with nothing below but cloud.
pub fn in_void(x: f32, z: f32) -> bool {
    floor().sdf(x, z) > EDGE
}

/// How far below its floor an island's UNDERSIDE hangs at (x, z), for a
/// point over the island (inside its edge). The underside is its own
/// surface (`render::build_terrain` hangs it under the top): from the lip
/// it curls straight down and back in under the island, then keeps
/// deepening toward the middle -- an inverted mountain, a keel of rock and
/// ice, so each island reads as floating, not as a mesa standing on
/// something. Narrow causeways hang shallow (~30 u); the Moonwell's island
/// hangs ~80 u. Ragged with drips of ice.
pub fn underside(x: f32, z: f32) -> f32 {
    let inside = (EDGE - floor().sdf(x, z)).max(0.0);
    let v = crate::pass::vnoise;
    let keel = 3.6 * inside.sqrt() + 1.1 * inside * (0.8 + 0.4 * v(x * 0.02 + 7.1, z.abs() * 0.02));
    let drips = 4.0 * v(x * 0.35 + 2.3, z.abs() * 0.35 - 1.1).powi(3) * (inside / 5.0).min(1.0);
    (keel + drips).min(120.0)
}
/// The icy lip round every island: it rises this high just past the edge.
// (2.2 until the ground below was taken away: from the game's own camera,
// 8.5 u behind a cat, a lip that high was a wall that hid the drop)
pub const RIM_H: f32 = 0.8;
/// ...over this width, then the island's underside falls away.
pub const RIM_W: f32 = 3.0;

/// Terrain height: level stone and snow on the floor, a pool bed in the
/// well, an ice lip at the edge, then the drop into the sky.
pub fn terrain_height(x: f32, z: f32) -> f32 {
    let d = floor().sdf(x, z);
    let base = floor_height(x, z);
    if d <= 0.0 {
        if base > 0.0 {
            return base + 0.1 * (crate::pass::vnoise(x * 0.35, z.abs() * 0.35) - 0.5);
        }
        let r = x.hypot(z);
        if r < POOL_R + 1.5 {
            let t = 1.0 - ((r - POOL_R + 1.5).max(0.0) / 3.0).clamp(0.0, 1.0);
            let bowl = 1.0 - (r / POOL_R).min(1.0).powi(2);
            return -POOL_DEPTH * (0.35 + 0.65 * bowl) * t;
        }
        return 0.1 * (crate::pass::vnoise(x * 0.35, z.abs() * 0.35) - 0.5);
    }
    if d < RIM_W {
        // the lip: up and over, a little broken
        let t = d / RIM_W;
        let n = 0.6 + 0.8 * hash2((x * 0.5).floor(), (z.abs() * 0.5).floor());
        return base + RIM_H * n * (t * std::f32::consts::PI).sin();
    }
    // past the lip, open sky: nothing is drawn there (`in_void`); the
    // height only says "far down" (the Moonwyrm's chasm dives read it)
    void_depth(d)
}

fn void_depth(d: f32) -> f32 {
    let t = ((d - RIM_W) / 3.0).clamp(0.0, 1.0);
    -40.0 * t * t * (3.0 - 2.0 * t)
}

/// [`terrain_height`] of the islands alone, as if the Aerie were not
/// there: what lies under the Aerie's bridges (the Moonwyrm dives beneath
/// them). The islands' floor is at 0, so only the lip and the pool matter.
pub fn islands_height(x: f32, z: f32) -> f32 {
    let d = lower_floor().sdf(x, z);
    if d <= 0.0 {
        let r = x.hypot(z);
        if r < POOL_R + 1.5 {
            return terrain_height(x, z);
        }
        return 0.1 * (crate::pass::vnoise(x * 0.35, z.abs() * 0.35) - 0.5);
    }
    if d < RIM_W {
        let t = d / RIM_W;
        let n = 0.6 + 0.8 * hash2((x * 0.5).floor(), (z.abs() * 0.5).floor());
        return RIM_H * n * (t * std::f32::consts::PI).sin();
    }
    void_depth(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_map_is_fair() {
        let f = floor();
        for x in (-300..=300).step_by(7) {
            for z in (7..=300).step_by(7) {
                let (x, z) = (x as f32, z as f32);
                assert!((f.sdf(x, z) - f.sdf(x, -z)).abs() < 1e-3, "floor at ({x},{z})");
                assert!((terrain_height(x, z) - terrain_height(x, -z)).abs() < 1e-3, "terrain at ({x},{z})");
            }
        }
        for lane in [Lane::Top, Lane::Bot] {
            for inner in [false, true] {
                let (b, r) = (tower_pos(lane, Team::Blue, inner), tower_pos(lane, Team::Red, inner));
                assert!((b.x - r.x).abs() < 1e-3 && (b.z + r.z).abs() < 1e-3, "{lane:?} {inner}: {b} {r}");
            }
        }
    }

    #[test]
    fn it_is_twice_the_pass() {
        let ratio = (ARENA_RADIUS / crate::pass::ARENA_RADIUS).powi(2);
        assert!((1.9..2.1).contains(&ratio), "area ratio {ratio}");
        assert!(LANE_R > crate::pass::MID_R && LANE_R > crate::pass::SIDE_R, "wider lanes than any before");
    }

    #[test]
    fn lanes_bases_towers_and_islands_are_on_the_floor() {
        let f = floor();
        for lane in [Lane::Top, Lane::Bot] {
            for p in lane_polyline(lane) {
                assert!(f.fits(p.x, p.z, LANE_R - 1.0), "{lane:?} at {p}");
            }
            for team in [Team::Blue, Team::Red] {
                for inner in [false, true] {
                    let t = tower_pos(lane, team, inner);
                    assert!(f.fits(t.x, t.z, 4.0), "tower {lane:?} {team:?} {inner} at {t}");
                    assert!(t.length() < ARENA_RADIUS - 5.0);
                }
            }
        }
        for team in [Team::Blue, Team::Red] {
            for p in [base_pos(team), fountain(team), titan_pos(team)] {
                assert!(f.fits(p.x, p.z, 3.0), "{p}");
                assert!(p.length() < ARENA_RADIUS - 2.0);
            }
        }
        for i in islands() {
            assert!(f.fits(i.x, i.z, i.r * 0.9), "{}", i.name);
            assert!(i.x.hypot(i.z) + i.r < ARENA_RADIUS - 2.0, "{} reaches past the arena's edge", i.name);
        }
        for p in heal_tree_positions() {
            assert!(f.fits(p.x, p.z, 2.0), "spring at {p}");
        }
        for p in pit_positions() {
            assert!(f.fits(p.x, p.z, 6.0), "pit at {p}");
        }
        for (p, _) in statues() {
            assert!(f.fits(p.x, p.z, 1.0), "statue at {p}");
            assert!(bridge_dist(p.x, p.z) > BRIDGE_R + 4.0 && causeway_dist(p.x, p.z) > CAUSEWAY_R + 4.0, "a statue in a way onto the lane at {p}");
        }
    }

    /// Everything a bot or a minion must reach is joined by walkable floor:
    /// both bases, both lanes end to end, the glades, the terrace and the
    /// Moonwell (by its star-bridges). The shoals are the exception: cloud
    /// only.
    #[test]
    fn the_floor_joins_what_must_be_walked() {
        let f = floor();
        // connected through the floor's shape graph (`route` answers
        // `[to]` for an unreachable goal, so it cannot say)
        let reach = |a: Vec3, b: Vec3| {
            let (s, g) = (f.shape_at(a.x, a.z).unwrap(), f.shape_at(b.x, b.z).unwrap());
            let links = f.links();
            let mut seen = vec![false; links.len()];
            let mut stack = vec![s];
            while let Some(u) = stack.pop() {
                if u == g {
                    return true;
                }
                if std::mem::replace(&mut seen[u], true) {
                    continue;
                }
                stack.extend(links[u].iter().map(|l| l.to));
            }
            false
        };
        let blue = base_pos(Team::Blue);
        for (to, what) in [
            (base_pos(Team::Red), "the red base"),
            (at(-104.0, -96.0), "the west glade"),
            (at(0.0, -150.0), "the terrace"),
            (Vec3::ZERO, "the Moonwell"),
            (at(-214.0, 0.0), "the west lane's middle"),
        ] {
            assert!(reach(blue, to), "blue base to {what}");
        }
        assert!(!reach(blue, at(-245.0, -135.0)), "the west shoal is cloud-only");
    }

    /// The ferries cross real chasms: each pier is on the floor's edge and
    /// the middle of the route is open sky.
    #[test]
    fn every_ferry_spans_a_chasm_between_two_piers() {
        let f = floor();
        for (a, b) in ferry_routes() {
            for p in [a, b] {
                let d = f.sdf(p.x, p.z);
                assert!(d.abs() < 4.0, "pier {p} is near the edge (sdf {d})");
            }
            let mid = a.lerp(b, 0.5);
            assert!(f.sdf(mid.x, mid.z) > 6.0, "the middle of {a}-{b} is open sky: {}", f.sdf(mid.x, mid.z));
            assert!(in_void(mid.x, mid.z), "and nothing below it but cloud");
            assert!(terrain_height(mid.x, mid.z) < -12.0);
        }
    }

    /// The linking ways are not ruler lines: causeways bend and swell, the
    /// halves mirror each other exactly, and the star-bridges stay on the
    /// line between the halves (fair) but swell into landings.
    #[test]
    fn the_ways_bend_and_swell_and_stay_fair() {
        let v = ways();
        let n = CAUSEWAYS.len();
        assert_eq!(v.len(), 2 * n + BRIDGES.len());
        for (k, w) in v.iter().take(2 * n).enumerate() {
            let (a, b) = (w.pts[0], *w.pts.last().unwrap());
            let off = w.pts.iter().map(|p| seg_dist(*p, a, b)).fold(0.0f32, f32::max);
            assert!(off > 4.0, "causeway {k} bends: {off}");
            let (lo, hi) = w.r.iter().fold((f32::MAX, f32::MIN), |(l, h), &r| (l.min(r), h.max(r)));
            assert!(hi > lo * 1.3, "causeway {k} swells: {lo}..{hi}");
        }
        for k in 0..n {
            for (p, q) in v[k].pts.iter().zip(&v[n + k].pts) {
                assert!((p.x - q.x).abs() < 1e-4 && (p.z + q.z).abs() < 1e-4, "mirrored");
            }
        }
        for w in v.iter().skip(2 * n) {
            assert!(w.pts.iter().all(|p| p.z.abs() < 1e-4), "a bridge stays on z = 0");
            let (lo, hi) = w.r.iter().fold((f32::MAX, f32::MIN), |(l, h), &r| (l.min(r), h.max(r)));
            assert!(hi > lo * 1.3, "and swells: {lo}..{hi}");
        }
        // the floor follows them: the middle of a bent causeway is floor,
        // the old straight line there is not always
        let w = &v[1];
        let mid = w.pts[WAY_STEPS / 2];
        assert!(floor().sdf(mid.x, mid.z) < -2.0);
    }

    /// The Aerie floats: none of it over or touching the islands in plan
    /// (so no one walks up into it), it is small, its islets are all joined
    /// to each other, both halves are the same, and every lift runs from
    /// floor below to floor above.
    #[test]
    fn the_aerie_floats_clear_of_the_islands() {
        let (up, low) = (upper_floor(), lower_floor());
        let (mut up_n, mut low_n) = (0usize, 0usize);
        let mut x = -320.0;
        while x < 320.0 {
            let mut z = -320.0;
            while z < 320.0 {
                if up.sdf(x, z) < 0.0 {
                    up_n += 1;
                    assert!(low.sdf(x, z) > 4.0, "the Aerie at ({x}, {z}) comes within 4 u of the islands in plan");
                    assert!((floor_height(x, z) - UPPER_Y).abs() < 1e-3);
                    assert!(up.sdf(x, -z) < 0.5, "mirrored");
                }
                if low.sdf(x, z) < 0.0 {
                    low_n += 1;
                    assert_eq!(floor_height(x, z), 0.0);
                }
                z += 2.0;
            }
            x += 2.0;
        }
        assert!(up_n > 0 && up_n * 12 < low_n, "small: {up_n} cells up, {low_n} below");
        // every islet reaches both others over its sky-bridges
        let links = up.links();
        for zs in [1.0f32, -1.0] {
            let ids: Vec<usize> = AERIE.iter().map(|&(x, z, _)| up.shape_at(x, z * zs).unwrap()).collect();
            for &a in &ids {
                let mut seen = vec![false; links.len()];
                let mut stack = vec![a];
                while let Some(u) = stack.pop() {
                    if !std::mem::replace(&mut seen[u], true) {
                        stack.extend(links[u].iter().map(|l| l.to));
                    }
                }
                assert!(ids.iter().all(|&b| seen[b]), "islets joined");
            }
        }
        // lifts: a pad on the islands, a pad on the Aerie, one per islet
        let ls = lifts();
        assert_eq!(ls.len(), 2 * AERIE.len());
        for (lo, hi) in &ls {
            assert!(low.sdf(lo.x, lo.z) < -2.5 && lo.y == 0.0, "pad below at {lo} on the islands");
            assert!(up.sdf(hi.x, hi.z) < -2.5 && hi.y == UPPER_Y, "pad above at {hi} on the Aerie");
        }
        for &(x, z, _) in &AERIE {
            assert!(ls.iter().any(|(_, hi)| Vec3::new(hi.x - x, 0.0, hi.z - z).length() < 13.0), "a lift up to the islet at ({x}, {z})");
        }
    }

    #[test]
    fn the_pool_is_shallow_and_the_sky_is_deep() {
        assert!(terrain_height(0.0, 0.0) < -0.4 && terrain_height(0.0, 0.0) > -1.0);
        assert!(in_pool(3.0, 4.0) && !in_pool(POOL_R + 1.0, 0.0));
        // between the terrace and the Moonwell: open sky down to the clouds
        assert!(lower_floor().sdf(0.0, -95.0) > 20.0);
        // (and at (0, -120), between the terrace and the Aerie above it)
        assert!(in_void(0.0, -120.0) && terrain_height(0.0, -120.0) < -12.0);
    }
}
