//! maze — the Quarry: 5 a side in a great walled labyrinth of SECTIONS
//! (rooms) joined by gates, where the labyrinth turns -- gates rise and
//! sink -- and sections wake and turn on whoever walks them alone.
//!
//! ## The ground
//!
//! Fifteen rooms in three columns and five rows ([`Room`]): courtyards and
//! halls some sixty units across, walled from each other by a thick hedge
//! of stone, mirrored across z = 0 like every other map. The Heart (a round
//! court) is in the middle, the beasts' pits north and south of it, the
//! springs in the side rooms and by the bases. Each base opens by a door
//! into the three rooms of its end row.
//!
//! Rooms are joined by short passages ([`passages`]): two between each pair
//! of neighbouring rooms. The two lanes run down the west (`Top`) and east
//! (`Bot`) columns, room to room through passages of their own that are
//! always open. Every other passage is a GATE.
//!
//! ## The labyrinth turns ([`Wake::closed`])
//!
//! Every [`TURN`] seconds the labyrinth turns: a fresh pattern of gates
//! shuts (a slab of stone rises in each) and the rest open. A gate and its
//! mirror always move together, the pattern never cuts a room off, and a
//! gate about to move glows for [`GATE_WARN`] seconds first. The patterns
//! come from the match's own seed, so the ways between the sections are
//! different every turn and every match -- and the same on every client:
//! a gate's state is a function of the match clock.
//!
//! A shut gate is a wall in `Blockers` (`push_gate`: every collision,
//! steering probe and push-out sees it) and a shape the floor router routes
//! round (`floor::route_with`). The sim sets the gates from the clock each
//! tick (`maze_tick`).
//!
//! ## The waking ([`Wake::mood`])
//!
//! From [`FIRST_WAKE`], every [`CYCLE`] seconds two new CLASSES of section
//! stir for [`WARN`] seconds and then turn hostile for [`HOSTILE`]; a class
//! is a column and a band, so a section and its mirror always wake together.
//! In a hostile section the thorns close on a cat by how many of its friends
//! are with it ([`thorn_share`]): alone, a heavy bite and a slow; with one
//! friend, lighter; with two or more, nothing. And anywhere on this map, a
//! cat with two friends close takes a tenth less from every blow
//! ([`PACK_WARD`]).
//!
//! Headless: `mazeworld` dresses it and places the gates' collision,
//! `render` colours it, `scene` draws the gates and the thorns, `ai` routes
//! round shut gates and walks bots out of waking sections they are alone in.

use glam::Vec3;
use std::sync::OnceLock;

use crate::blockers::Shape;
use crate::consts::{Lane, Team};
use crate::floor::FloorPlan;
use crate::sim::{Event, Game, Rng};

/// Players per team.
pub const TEAM_SIZE: usize = 5;
/// The rooms' columns (x of their centres), and the rows' spacing along z
/// (rows -2..=2).
pub const COLS: [f32; 3] = [-80.0, 0.0, 80.0];
pub const ROWS: i32 = 2;
pub const ROW_Z: f32 = 62.0;
/// A room's half-width (x) and half-depth (z): a stadium 60 by 42.
pub const ROOM_HX: f32 = 30.0;
pub const ROOM_HZ: f32 = 21.0;
/// The Heart: a round court.
pub const HEART_R: f32 = 27.0;
/// A passage's half-width.
pub const PASSAGE_R: f32 = 4.5;

pub const ARENA_RADIUS: f32 = 220.0;
pub const GROUND_HALF: f32 = 290.0;

/// A base's centre from the middle (along z), and its rotunda.
pub const BASE_Z: f32 = 180.0;
pub const BASE_R: f32 = 20.0;

/// The walls: height (more than a leap tops), the rise, and the rock beyond
/// the labyrinth's outer wall.
pub const WALL_H: f32 = 9.5;
pub const WALL_RISE: f32 = 1.0;
pub const RIM_H: f32 = 22.0;

/// The labyrinth turning: when first, how often, the warning, how long a
/// slab takes to rise or sink, and the share of gates a turn shuts.
pub const FIRST_TURN: f32 = 45.0;
pub const TURN: f32 = 40.0;
pub const GATE_WARN: f32 = 5.0;
pub const GATE_MOVE: f32 = 2.5;
pub const SHUT_SHARE: f32 = 0.45;

/// The waking: when it starts, how often, how long the warning, how long
/// it bites.
pub const FIRST_WAKE: f32 = 75.0;
pub const CYCLE: f32 = 50.0;
pub const WARN: f32 = 8.0;
pub const HOSTILE: f32 = 36.0;
/// Classes that wake each cycle.
pub const PER_CYCLE: usize = 2;
/// Friends this close (flat) count as with you.
pub const PACK_R: f32 = 10.0;
/// With two friends close, the share of each blow you take.
pub const PACK_WARD: f32 = 0.9;
/// Thorn pulses this far apart.
pub const BITE_EVERY: f32 = 0.5;
/// Slow on a cat alone in the thorns.
pub const THORN_SLOW: f32 = 0.25;

/// A room: its column (0..3) and row (-2..=2; negative is the blue half).
pub type Room = (usize, i32);

pub fn rooms() -> impl Iterator<Item = Room> {
    (-ROWS..=ROWS).flat_map(|r| (0..3).map(move |c| (c, r)))
}

pub fn room_pos(rm: Room) -> Vec3 {
    Vec3::new(COLS[rm.0], 0.0, rm.1 as f32 * ROW_Z)
}

pub const HEART: Room = (1, 0);
/// The beasts' pits (blue half; mirrored).
const PIT: Room = (1, -1);

pub fn base_pos(team: Team) -> Vec3 {
    Vec3::new(0.0, 0.0, -team.push_z() * BASE_Z)
}
pub fn fountain(team: Team) -> Vec3 {
    Vec3::new(0.0, 0.0, -team.push_z() * (BASE_Z + 11.0))
}
pub fn titan_pos(team: Team) -> Vec3 {
    Vec3::new(0.0, 0.0, -team.push_z() * (BASE_Z - 11.0))
}

fn mirror_room(rm: Room) -> Room {
    (rm.0, -rm.1)
}

fn mirror(p: Vec3) -> Vec3 {
    Vec3::new(p.x, p.y, -p.z)
}

/// A passage between two rooms: its ends (inside each room), and whether it
/// is a lane's (always open) or a gate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Passage {
    pub a: Room,
    pub b: Room,
    pub p0: Vec3,
    pub p1: Vec3,
    pub lane: Option<Lane>,
}

impl Passage {
    pub fn mid(&self) -> Vec3 {
        (self.p0 + self.p1) * 0.5
    }
    /// Along the passage (unit, flat).
    pub fn dir(&self) -> Vec3 {
        Vec3::new(self.p1.x - self.p0.x, 0.0, self.p1.z - self.p0.z).normalize_or_zero()
    }
}

/// The lanes' passages going north from each row (blue half): `(row, dx)`
/// off the column's centre. Top zig-zags down the west column, Bot down the
/// east, mirrored in x.
fn lane_steps(lane: Lane) -> [(i32, f32); 2] {
    match lane {
        Lane::Top => [(-2, -14.0), (-1, 14.0)],
        _ => [(-2, 14.0), (-1, -14.0)],
    }
}

fn lane_col(lane: Lane) -> usize {
    match lane {
        Lane::Top => 0,
        Lane::Mid => 1,
        Lane::Bot => 2,
    }
}

/// Every passage, both halves: between rows in each column (two each, at
/// dx = +-14), and between columns in each row (two each, at dz = +-9).
/// Ordered, and the order is the gates' ids.
pub fn passages() -> &'static [Passage] {
    static P: OnceLock<Vec<Passage>> = OnceLock::new();
    P.get_or_init(|| {
        let mut v = Vec::new();
        let lane_of = |c: usize, r: i32, dx: f32| -> Option<Lane> {
            for lane in [Lane::Top, Lane::Bot] {
                if lane_col(lane) != c {
                    continue;
                }
                for (row, ldx) in lane_steps(lane) {
                    // blue half as written; the red half is its mirror
                    // (the passage from row r up to r + 1 mirrors the one
                    // from -r - 1 up to -r)
                    if (r == row || r == -row - 1) && ldx == dx {
                        return Some(lane);
                    }
                }
            }
            None
        };
        for c in 0..3 {
            for r in -ROWS..ROWS {
                for dx in [-14.0f32, 14.0] {
                    let (a, b) = ((c, r), (c, r + 1));
                    let p0 = room_pos(a) + Vec3::new(dx, 0.0, 12.0);
                    let p1 = room_pos(b) + Vec3::new(dx, 0.0, -12.0);
                    v.push(Passage { a, b, p0, p1, lane: lane_of(c, r, dx) });
                }
            }
        }
        for r in -ROWS..=ROWS {
            for c in 0..2 {
                for dz in [-9.0f32, 9.0] {
                    let (a, b) = ((c, r), (c + 1, r));
                    let p0 = room_pos(a) + Vec3::new(18.0, 0.0, dz);
                    let p1 = room_pos(b) + Vec3::new(-18.0, 0.0, dz);
                    v.push(Passage { a, b, p0, p1, lane: None });
                }
            }
        }
        v
    })
}

/// Gate `g`'s mirror (itself on the axis).
pub fn mirror_passage(g: usize) -> usize {
    let p = passages()[g];
    let (m0, m1) = (mirror(p.p0), mirror(p.p1));
    passages().iter().position(|q| (q.p0.distance(m0) < 1e-3 && q.p1.distance(m1) < 1e-3) || (q.p0.distance(m1) < 1e-3 && q.p1.distance(m0) < 1e-3)).expect("every passage has a mirror")
}

/// Where a base's door opens: its rim, toward the room it leads to.
fn door(team: Team, to: Room) -> Vec3 {
    let b = base_pos(team);
    let t = room_pos(if team == Team::Blue { to } else { mirror_room(to) });
    b + (t - b).normalize_or_zero() * (BASE_R - 6.0)
}

/// Lane centre-lines, blue base to red: through the door, then room centre
/// to lane passage to room centre, up the lane's column.
pub fn lane_polyline(lane: Lane) -> Vec<Vec3> {
    let c = lane_col(lane);
    let mut half = vec![door(Team::Blue, (c, -ROWS)), room_pos((c, -2))];
    if lane == Lane::Mid {
        // a camera route only: straight up the middle
        half.push(room_pos((c, -1)));
    } else {
        // room centre to the passage's mouth, down the passage, out of its
        // far mouth to the next room's centre (a diagonal from centre to
        // the passage's middle cut the wall's corner)
        for (row, dx) in lane_steps(lane) {
            half.push(room_pos((c, row)) + Vec3::new(dx, 0.0, 12.0));
            half.push(room_pos((c, row + 1)) + Vec3::new(dx, 0.0, -12.0));
            half.push(room_pos((c, row + 1)));
        }
    }
    let axis = *half.last().unwrap();
    let mut v = half.clone();
    if axis.z.abs() > 1e-3 {
        v.push(room_pos((c, 0)));
    }
    v.extend(half.iter().rev().filter(|p| p.z.abs() > 1e-3).map(|&p| mirror(p)));
    v
}

/// Towers: the inner in the base's door, the outer in the lane's second room.
pub fn tower_pos(lane: Lane, team: Team, inner: bool) -> Vec3 {
    let mut pts = lane_polyline(lane);
    if team == Team::Red {
        pts.reverse();
    }
    crate::pass::along(&pts, if inner { 30.0 } else { 140.0 })
}

/// The walkable floor, and where in it each passage's shape is.
struct Plan {
    floor: FloorPlan,
    /// `passage_shape[g]`: passage `g`'s index in `floor.shapes`.
    passage_shape: Vec<usize>,
}

fn plan() -> &'static Plan {
    static P: OnceLock<Plan> = OnceLock::new();
    P.get_or_init(|| {
        let mut s = Vec::new();
        for rm in rooms() {
            let p = room_pos(rm);
            if rm == HEART {
                s.push(Shape::Circle { x: p.x, z: p.z, r: HEART_R });
            } else {
                let h = ROOM_HX - ROOM_HZ;
                s.push(Shape::Segment { x0: p.x - h, z0: p.z, x1: p.x + h, z1: p.z, r: ROOM_HZ });
            }
        }
        let mut passage_shape = Vec::new();
        for g in passages() {
            passage_shape.push(s.len());
            s.push(Shape::Segment { x0: g.p0.x, z0: g.p0.z, x1: g.p1.x, z1: g.p1.z, r: PASSAGE_R });
        }
        for team in [Team::Blue, Team::Red] {
            let b = base_pos(team);
            s.push(Shape::Circle { x: b.x, z: b.z, r: BASE_R });
            let f = fountain(team);
            s.push(Shape::Circle { x: f.x, z: f.z, r: 11.0 });
            for c in 0..3 {
                let to = room_pos(if team == Team::Blue { (c, -ROWS) } else { (c, ROWS) });
                s.push(Shape::Segment { x0: b.x, z0: b.z, x1: to.x, z1: to.z, r: 5.0 });
            }
        }
        let n = s.len();
        let floor = FloorPlan::new(s);
        assert_eq!(floor.shapes.len(), n, "a floor shape was dropped: the passage indices would be off");
        Plan { floor, passage_shape }
    })
}

/// The walkable floor (every passage included, open or shut: a shut gate
/// is a wall in `Blockers`, not a hole in the floor).
pub fn floor() -> &'static FloorPlan {
    &plan().floor
}

/// Passage `g`'s shape in [`floor`].
pub fn passage_shape(g: usize) -> usize {
    plan().passage_shape[g]
}

/// The passage a floor shape is, if it is one.
pub fn shape_passage(shape: usize) -> Option<usize> {
    plan().passage_shape.iter().position(|&s| s == shape)
}

/// The walkable floor's height: all at 0.
pub fn floor_height(_x: f32, _z: f32) -> f32 {
    0.0
}

/// The two beasts' grounds: the rooms north and south of the Heart.
pub fn pit_positions() -> [Vec3; 2] {
    [room_pos(PIT), room_pos(mirror_room(PIT))]
}

/// The springs: at the far wall of the lanes' middle rooms, and in the
/// middle rooms by the bases.
pub fn heal_tree_positions() -> [Vec3; 6] {
    let w = room_pos((0, -1)) + Vec3::new(-22.0, 0.0, 0.0);
    let e = room_pos((2, -1)) + Vec3::new(22.0, 0.0, 0.0);
    let m = room_pos((1, -2));
    [w, mirror(w), e, mirror(e), m, mirror(m)]
}

/// No water in the maze.
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

/// The labyrinth's outer wall: half-width and half-depth.
fn outer() -> (f32, f32) {
    (COLS[2] + ROOM_HX + 14.0, BASE_Z + 24.0)
}

/// Is (x, z) past the labyrinth's outer wall (the rock round it)?
pub fn beyond(x: f32, z: f32) -> bool {
    let (hx, hz) = outer();
    x.abs() > hx || z.abs() > hz
}

/// Terrain height: the floor (flat), the walls rising sheer round it with a
/// ragged thorn-hedge crown, and past the outer wall, rock rising higher.
/// `|z|` in the noise so the halves match exactly.
pub fn terrain_height(x: f32, z: f32) -> f32 {
    let d = floor().sdf(x, z);
    let za = z.abs();
    if d <= 0.0 {
        return 0.03 * (vnoise(x * 0.5, za * 0.5) - 0.5);
    }
    let crown = 0.9 * vnoise(x * 0.45, za * 0.45) + 0.5 * vnoise(x * 1.3 + 4.0, za * 1.3);
    let wall = if d < WALL_RISE {
        let t = d / WALL_RISE;
        WALL_H * (1.0 - (1.0 - t).powi(3))
    } else {
        WALL_H + crown * ((d - WALL_RISE) / 1.5).clamp(0.0, 1.0)
    };
    if beyond(x, z) {
        let (hx, hz) = outer();
        let out = (x.abs() - hx).max(za - hz).max(0.0);
        let k = (out / 25.0).clamp(0.0, 1.0);
        return wall.max(WALL_H + RIM_H * k + 6.0 * (vnoise(x * 0.05, za * 0.05) - 0.5) * k);
    }
    wall
}

// --- the labyrinth turning ---------------------------------------------------

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The rooms reachable from the blue base through the passages `open`
/// allows. The bases join their end rows by their doors (the red base
/// joins its row once any of it is reached).
fn reachable(open: &dyn Fn(usize) -> bool) -> std::collections::HashSet<Room> {
    let mut seen: std::collections::HashSet<Room> = (0..3).map(|c| (c, -ROWS)).collect();
    let mut q: std::collections::VecDeque<Room> = seen.iter().copied().collect();
    while let Some(rm) = q.pop_front() {
        let mut next: Vec<Room> = passages().iter().enumerate().filter(|(g, p)| open(*g) && (p.a == rm || p.b == rm)).map(|(_, p)| if p.a == rm { p.b } else { p.a }).collect();
        if rm.1 == ROWS {
            next.extend((0..3).map(|c| (c, ROWS)));
        }
        for n in next {
            if seen.insert(n) {
                q.push_back(n);
            }
        }
    }
    seen
}

// --- sections and the waking ------------------------------------------------

/// A section: a column (0 west, 1 centre, 2 east) and a band (0 the Heart's
/// row, 1 the halls, 2 the depths by the bases) on a side (-1 blue, 0 the
/// middle row, +1 red) -- one room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Section {
    pub col: u8,
    pub band: u8,
    pub side: i8,
}

impl Section {
    /// The class: what wakes together (a section and its mirror).
    pub fn class(self) -> u8 {
        self.col * 3 + self.band
    }
    pub fn name(self) -> String {
        class_name(self.class())
    }
    pub fn room(self) -> Room {
        (self.col as usize, self.band as i32 * self.side as i32)
    }
}

pub const CLASSES: u8 = 9;

pub fn class_name(class: u8) -> String {
    let col = ["WEST", "MIDDLE", "EAST"][(class / 3) as usize % 3];
    let band = ["HEART", "HALLS", "DEPTHS"][(class % 3) as usize];
    format!("THE {col} {band}")
}

/// The section a point is in: the room it is in or nearest to (a passage
/// belongs to the room it is nearer). None: a base, or past the walls.
pub fn section_at(x: f32, z: f32) -> Option<Section> {
    if z.abs() > ROWS as f32 * ROW_Z + ROOM_HZ + 6.0 || beyond(x, z) {
        return None;
    }
    let col = if x < -40.0 { 0 } else if x > 40.0 { 2 } else { 1 };
    let row = ((z / ROW_Z).round() as i32).clamp(-ROWS, ROWS);
    Some(Section { col, band: row.unsigned_abs() as u8, side: row.signum() as i8 })
}

/// How a section is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mood {
    Calm,
    /// Waking: `0..1` through the warning.
    Stirring(f32),
    /// Biting: `0..1` through it.
    Hostile(f32),
}

/// The match's labyrinth: the waking order (every class once, shuffled at
/// the start, then round again, two a cycle) and the seed its turning
/// patterns are drawn from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wake {
    pub order: [u8; CLASSES as usize],
    pub seed: u64,
}

impl Default for Wake {
    fn default() -> Self {
        Wake { order: std::array::from_fn(|k| k as u8), seed: 0 }
    }
}

impl Wake {
    pub fn new(rng: &mut Rng) -> Wake {
        let mut order: [u8; CLASSES as usize] = std::array::from_fn(|k| k as u8);
        for i in (1..order.len()).rev() {
            let j = (rng.next_u() % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        Wake { order, seed: rng.next_u() }
    }

    /// The cycle at match time `t`, and how far into it (None before the
    /// first).
    pub fn cycle(t: f32) -> Option<(u32, f32)> {
        (t >= FIRST_WAKE).then(|| (((t - FIRST_WAKE) / CYCLE) as u32, (t - FIRST_WAKE) % CYCLE))
    }

    /// The classes that wake in cycle `n`.
    pub fn classes(&self, n: u32) -> [u8; PER_CYCLE] {
        std::array::from_fn(|k| self.order[((n as usize) * PER_CYCLE + k) % CLASSES as usize])
    }

    /// How section `s` is doing at match time `t`.
    pub fn mood(&self, s: Section, t: f32) -> Mood {
        self.class_mood(s.class(), t)
    }

    /// How every section of `class` is doing at `t`.
    pub fn class_mood(&self, class: u8, t: f32) -> Mood {
        let Some((n, into)) = Wake::cycle(t) else { return Mood::Calm };
        if !self.classes(n).contains(&class) {
            return Mood::Calm;
        }
        if into < WARN {
            Mood::Stirring(into / WARN)
        } else if into < WARN + HOSTILE {
            Mood::Hostile((into - WARN) / HOSTILE)
        } else {
            Mood::Calm
        }
    }

    /// How the ground at (x, z) is doing.
    pub fn mood_at(&self, x: f32, z: f32, t: f32) -> Mood {
        section_at(x, z).map_or(Mood::Calm, |s| self.mood(s, t))
    }

    /// The turn at match time `t` (0 before the first) and how far into it.
    pub fn turn(t: f32) -> (u32, f32) {
        if t < FIRST_TURN {
            (0, t + TURN)
        } else {
            (((t - FIRST_TURN) / TURN) as u32 + 1, (t - FIRST_TURN) % TURN)
        }
    }

    /// Which gates are shut in turn `k`: a gate and its mirror together,
    /// about [`SHUT_SHARE`] of them, never a lane's, and never so many that
    /// a room is cut off (gates are opened again, in order, until every
    /// room can be reached).
    pub fn closed(&self, k: u32) -> Vec<bool> {
        let ps = passages();
        let mut shut = vec![false; ps.len()];
        for g in 0..ps.len() {
            let m = mirror_passage(g);
            if ps[g].lane.is_some() || m < g {
                continue;
            }
            let h = splitmix(self.seed ^ (k as u64).wrapping_mul(0x1000_0000_01B3) ^ ((g as u64) << 40));
            let u = (h >> 11) as f64 / (1u64 << 53) as f64;
            if u < SHUT_SHARE as f64 {
                shut[g] = true;
                shut[m] = true;
            }
        }
        // the machine's locks seal for the turn: every gate of a room its
        // tracks run through (`machine::lock_rooms`), but a lane's...
        let locks = crate::machine::lock_rooms(self.seed, k);
        for (g, p) in ps.iter().enumerate() {
            if p.lane.is_none() && (locks.contains(&p.a) || locks.contains(&p.b)) {
                shut[g] = true;
            }
        }
        // ...and its tracks' own, which it holds open (`machine::held_open`)
        for g in crate::machine::held_open(self.seed, k) {
            shut[g] = false;
        }
        let all = rooms().count();
        let mut g = 0;
        while reachable(&|i| !shut[i]).len() < all && g < ps.len() {
            if shut[g] {
                shut[g] = false;
                shut[mirror_passage(g)] = false;
            }
            g += 1;
        }
        shut
    }

    /// How far up gate `g`'s slab is at `t` (0 sunk, 1 shut), and whether
    /// it is about to move (the warning), from the patterns of this turn,
    /// the one before, and the next.
    pub fn gate_state(t: f32, g: usize, now: &[bool], before: &[bool], next: &[bool]) -> (f32, bool) {
        let (k, into) = Wake::turn(t);
        let (a, b) = (before[g] as i32 as f32, now[g] as i32 as f32);
        let h = if into < GATE_MOVE { a + (b - a) * (into / GATE_MOVE) } else { b };
        // by the time left to the next turn (before the first, `into` is
        // not a time into anything: every gate that would move glowed from
        // the match's first second)
        let next_at = FIRST_TURN + k as f32 * TURN;
        let warn = next_at - t < GATE_WARN && next[g] != now[g];
        (h, warn)
    }

    /// Every gate's slab height and warning at `t`.
    pub fn gates_at(&self, t: f32) -> Vec<(f32, bool)> {
        let (k, _) = Wake::turn(t);
        let now = self.closed(k);
        let before = if k == 0 { now.clone() } else { self.closed(k - 1) };
        let next = self.closed(k + 1);
        (0..now.len()).map(|g| Wake::gate_state(t, g, &now, &before, &next)).collect()
    }
}

// --- sight -------------------------------------------------------------------

/// How far a cat sees down the Quarry's halls.
pub const SIGHT: f32 = 55.0;

/// Can a cat at `a` see one at `b` in the Quarry? Within [`SIGHT`], along a
/// line that stays on the floor (no wall between) and crosses no shut
/// gate. Close enough to touch, always.
pub fn sight(game: &Game, a: Vec3, b: Vec3) -> bool {
    let (fa, fb) = (Vec3::new(a.x, 0.0, a.z), Vec3::new(b.x, 0.0, b.z));
    let len = fa.distance(fb);
    if len < 3.0 {
        return true;
    }
    if len > SIGHT {
        return false;
    }
    let f = floor();
    let n = (len / 1.0).ceil() as usize;
    for k in 1..n {
        let p = fa + (fb - fa) * (k as f32 / n as f32);
        if f.sdf(p.x, p.z) > 0.3 {
            return false;
        }
    }
    // a shut gate's slab across the line
    for (g, p) in passages().iter().enumerate() {
        if game.gate_open(g) {
            continue;
        }
        let (m, d) = (p.mid(), p.dir());
        let side = Vec3::new(-d.z, 0.0, d.x) * (PASSAGE_R + 1.5);
        if segments_cross(fa, fb, m - side, m + side) {
            return false;
        }
    }
    true
}

fn segments_cross(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> bool {
    let cross = |o: Vec3, p: Vec3, q: Vec3| (p.x - o.x) * (q.z - o.z) - (p.z - o.z) * (q.x - o.x);
    let (d1, d2) = (cross(c, d, a), cross(c, d, b));
    let (d3, d4) = (cross(a, b, c), cross(a, b, d));
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

/// Which cats `team` can see in the Quarry: its own; any enemy one of its
/// living cats has [`sight`] of; any in reach of its standing towers or at
/// its base. Minions give no sight (as on the sky map).
pub fn seen_by(game: &Game, team: crate::consts::Team) -> Vec<bool> {
    (0..game.gods.len())
        .map(|j| {
            let e = &game.gods[j];
            if e.team == team || e.respawn_t > 0.0 {
                return true;
            }
            let by_cat = game.gods.iter().any(|v| v.team == team && v.respawn_t <= 0.0 && sight(game, v.pos, e.pos));
            let by_tower = game.towers.iter().any(|t| t.team == team && t.hp > 0.0 && t.pos.distance(e.pos) < crate::consts::TOWER_RANGE + 2.0);
            let at_base = team.base_pos().distance(e.pos) < 40.0;
            by_cat || by_tower || at_base
        })
        .collect()
}

/// A slab this far up blocks.
pub const GATE_BLOCKS: f32 = 0.5;

/// What share of its health a second the thorns take from a cat with
/// `pack` friends close: alone, a heavy bite; with one friend, lighter;
/// with two or more, none.
pub fn thorn_share(pack: usize) -> f32 {
    match pack {
        0 => 0.05,
        1 => 0.02,
        _ => 0.0,
    }
}

impl Game {
    /// Friends of cat `i` within [`PACK_R`] (alive, flat distance).
    pub fn pack(&self, i: usize) -> usize {
        let g = &self.gods[i];
        self.gods
            .iter()
            .enumerate()
            .filter(|(k, o)| *k != i && o.team == g.team && o.respawn_t <= 0.0 && Vec3::new(o.pos.x - g.pos.x, 0.0, o.pos.z - g.pos.z).length() <= PACK_R)
            .count()
    }

    /// Is gate `g` open now (as the match's walls have it)? A game with no
    /// gates in its Blockers (a bare headless one) has every gate open.
    pub fn gate_open(&self, g: usize) -> bool {
        self.blockers.gate_count() <= g || self.blockers.gate_open(g)
    }

    /// May the floor router go through floor shape `s` now? Not through a
    /// shut gate's passage (on the Quarry; every shape elsewhere).
    pub fn route_open(&self, s: usize) -> bool {
        if crate::map::active() != crate::map::MapId::Maze {
            return true;
        }
        shape_passage(s).is_none_or(|g| self.gate_open(g))
    }

    /// Where a route from `pos` should start. Inside a shut gate's passage,
    /// the route began in the passage's own shape and left by whichever end
    /// was shorter -- through the slab, and the cat walked into it (a bot
    /// caught by a gate shutting on it stood against it for ten seconds).
    /// So from there it starts at the mouth on the cat's own side.
    pub fn route_from(&self, pos: Vec3) -> Vec3 {
        if crate::map::active() != crate::map::MapId::Maze {
            return pos;
        }
        let Some(g) = floor().shape_at(pos.x, pos.z).and_then(shape_passage) else { return pos };
        if self.gate_open(g) {
            return pos;
        }
        let p = passages()[g];
        if p.p0.distance(pos) < p.p1.distance(pos) { p.p0 } else { p.p1 }
    }

    /// Once a tick, on the Quarry only: the gates set from the clock, the
    /// pack's ward, the thorns, and the announcements.
    pub fn maze_tick(&mut self, dt: f32) {
        if crate::map::active() != crate::map::MapId::Maze {
            return;
        }
        let t = self.t;
        // the gates (their walls are in the shared Blockers)
        for (g, (h, _)) in self.wake.gates_at(t).into_iter().enumerate() {
            self.blockers.set_gate(g, h < GATE_BLOCKS);
        }
        if t >= FIRST_TURN && Wake::turn(t).0 != Wake::turn(t - dt).0 {
            self.events.push(Event::Announce { text: "THE LABYRINTH TURNS".into() });
            self.events.push(Event::LabyrinthTurns);
        }
        // announce a waking as it stirs, and again as it bites
        if let (Some((n, into)), prev) = (Wake::cycle(t), Wake::cycle(t - dt)) {
            let names = self.wake.classes(n).map(class_name).join(" AND ");
            let crossed = |at: f32| into >= at && prev.map_or(true, |(pn, pi)| pn != n || pi < at);
            if crossed(0.0) {
                self.events.push(Event::Announce { text: format!("THE MAZE STIRS: {names}") });
            }
            if crossed(WARN) {
                self.events.push(Event::Announce { text: format!("THE THORNS CLOSE: {names}") });
            }
        }
        for i in 0..self.gods.len() {
            if self.gods[i].respawn_t > 0.0 {
                self.gods[i].thorn_t = 0.0;
                continue;
            }
            let pack = self.pack(i);
            if pack >= 2 {
                self.gods[i].ward *= PACK_WARD;
            }
            let p = self.gods[i].pos;
            if !matches!(self.wake.mood_at(p.x, p.z, t), Mood::Hostile(_)) {
                self.gods[i].thorn_t = 0.0;
                continue;
            }
            let share = thorn_share(pack);
            if share <= 0.0 {
                self.gods[i].thorn_t = 0.0;
                continue;
            }
            if pack == 0 {
                let g = &mut self.gods[i];
                g.slow = g.slow.max(THORN_SLOW);
                g.slow_t = g.slow_t.max(0.25);
            }
            self.gods[i].thorn_t += dt;
            while self.gods[i].thorn_t >= BITE_EVERY {
                self.gods[i].thorn_t -= BITE_EVERY;
                let d = self.gods[i].max_hp * share * BITE_EVERY;
                // the maze's own: no one's kill, and no armour turns it
                self.deal_damage(crate::sim::TargetRef::God(i), d, true, None, 1.0, 0.0);
                self.events.push(Event::ThornBite { pos: p, alone: pack == 0 });
                if self.gods[i].respawn_t > 0.0 {
                    break;
                }
            }
        }
    }
}

/// What the HUD tells cat `i` about the ground it stands on (text, and a
/// colour: red alone in the thorns, amber one short, green in a pack,
/// amber while it stirs). None on calm ground or another map.
pub fn banner(game: &Game, i: usize) -> Option<(String, [u8; 3])> {
    if crate::map::active() != crate::map::MapId::Maze || game.gods[i].respawn_t > 0.0 {
        return None;
    }
    let p = game.gods[i].pos;
    let pack = game.pack(i);
    if crate::machine::locked(game.wake.seed, game.t, p.x, p.z) {
        return Some(("A SEALED LOCK -- THE STONES ARE COMING, THE FLOOR WILL DROP".to_string(), [235, 120, 50]));
    }
    match game.wake.mood_at(p.x, p.z, game.t) {
        Mood::Calm => None,
        Mood::Stirring(f) => Some((format!("THIS GROUND IS WAKING -- {:.0}s", (1.0 - f) * WARN), [240, 180, 60])),
        Mood::Hostile(_) => Some(match pack {
            0 => ("ALONE IN THE THORNS -- FIND YOUR PACK".to_string(), [235, 70, 60]),
            1 => ("THE THORNS -- ONE FRIEND IS NOT ENOUGH".to_string(), [240, 150, 60]),
            _ => ("THE PACK HOLDS THE THORNS BACK".to_string(), [110, 220, 120]),
        }),
    }
}

/// Where a bot should go instead of its lane, on the Quarry (None: carry
/// on). A cat in a pack (two friends close) goes where it likes. Otherwise:
///
/// * in a section that is waking or biting: out of it, to the nearest room
///   that is calm or holds a friend -- nearest by the open passages, room to
///   room, not as the crow flies (through a wall);
/// * on calm ground whose lane runs on (the next ~20 u) into waking ground:
///   hold, at the edge. Without this a bot stepped out, found calm ground,
///   took up its lane again, walked back in and was sent out again --
///   hovering at the edge inside the thorns (measured: more time alone in
///   them than with no rule at all).
pub fn bot_goal(game: &Game, gi: usize) -> Option<Vec3> {
    if crate::map::active() != crate::map::MapId::Maze {
        return None;
    }
    let me = &game.gods[gi];
    let t = game.t;
    // a boulder coming first: step off its track, pack or no pack
    if let Some(p) = crate::machine::dodge(game, gi) {
        return Some(p);
    }
    // a lock the machine is working: out, pack or no pack (the pads do not
    // care how many stand on them)
    let locked = |p: Vec3| crate::machine::locked(game.wake.seed, t, p.x, p.z);
    if game.pack(gi) >= 2 && !locked(me.pos) {
        return None;
    }
    let waking = |p: Vec3| !matches!(game.wake.mood_at(p.x, p.z, t), Mood::Calm) || locked(p);
    if !waking(me.pos) {
        let brain = me.ai.as_ref()?;
        let path = crate::consts::lane_path(brain.lane, me.team);
        let ahead = path.iter().skip(brain.wp).take(4).any(|p| waking(*p));
        return ahead.then_some(me.pos);
    }
    let here = section_at(me.pos.x, me.pos.z)?;
    let calm = |rm: Room| {
        let p = room_pos(rm);
        matches!(game.wake.mood_at(p.x, p.z, t + 2.0), Mood::Calm) && !crate::machine::locked(game.wake.seed, t, p.x, p.z)
    };
    // A room with a standing enemy tower in it is no refuge and no way
    // through: a bot sent past one stood at the tower's edge, stepping in
    // for the refuge and backing out for the tower, alone in the thorns for
    // 20 s (seed 5).
    let towered = |rm: Room| game.towers.iter().any(|tw| tw.team != me.team && tw.hp > 0.0 && section_at(tw.pos.x, tw.pos.z).is_some_and(|s| s.room() == rm));
    let friend_in = |rm: Room| {
        game.gods.iter().enumerate().any(|(k, o)| k != gi && o.team == me.team && o.respawn_t <= 0.0 && section_at(o.pos.x, o.pos.z).is_some_and(|s| s.room() == rm))
    };
    let start = here.room();
    let mut seen = std::collections::HashSet::from([start]);
    let mut q = std::collections::VecDeque::from([start]);
    while let Some(rm) = q.pop_front() {
        if rm != start && (calm(rm) || (friend_in(rm) && !locked(room_pos(rm)))) {
            return Some(room_pos(rm));
        }
        for (g, p) in passages().iter().enumerate() {
            if !(p.a == rm || p.b == rm) || !game.gate_open(g) {
                continue;
            }
            let n = if p.a == rm { p.b } else { p.a };
            // (nor a passage an enemy tower covers: the same standoff, one
            // room on -- Twofang, seed 9, 15 s)
            let covered = game.towers.iter().any(|tw| tw.team != me.team && tw.hp > 0.0 && [p.p0, p.mid(), p.p1].iter().any(|q| tw.pos.distance(*q) < crate::consts::TOWER_RANGE + 2.0));
            if !towered(n) && !covered && seen.insert(n) {
                q.push_back(n);
            }
        }
    }
    Some(me.team.fountain())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn within(f: impl FnOnce()) {
        crate::map::with(crate::map::MapId::Maze, f)
    }

    /// Mirrored across z = 0: floor, terrain, bases, towers, pits, springs,
    /// the passages, and the sections (a section and its mirror are one
    /// class).
    #[test]
    fn the_labyrinth_is_fair() {
        within(|| {
            let f = floor();
            for k in 0..4000 {
                let x = (hash2(k as f32, 1.0) - 0.5) * 2.0 * ARENA_RADIUS;
                let z = (hash2(k as f32, 2.0) - 0.5) * 2.0 * ARENA_RADIUS;
                assert!((f.sdf(x, z) - f.sdf(x, -z)).abs() < 1e-3, "floor at {x},{z}");
                assert!((terrain_height(x, z) - terrain_height(x, -z)).abs() < 1e-3, "terrain at {x},{z}");
                assert_eq!(section_at(x, z).map(|s| s.class()), section_at(x, -z).map(|s| s.class()), "section at {x},{z}");
            }
            for g in 0..passages().len() {
                let m = mirror_passage(g);
                assert_eq!(passages()[g].lane, passages()[m].lane, "passage {g} and its mirror {m}");
            }
            for lane in [Lane::Top, Lane::Bot] {
                for inner in [false, true] {
                    assert!((tower_pos(lane, Team::Blue, inner) - mirror(tower_pos(lane, Team::Red, inner))).length() < 1e-3);
                }
            }
            let p = pit_positions();
            assert!((p[0] - mirror(p[1])).length() < 1e-4);
        });
    }

    /// Everything a match needs stands on the floor with room round it, and
    /// both lanes run unbroken from base to base -- through their own
    /// passages, never a gate.
    #[test]
    fn everything_stands_on_the_floor_and_the_lanes_run_through() {
        within(|| {
            let f = floor();
            for team in [Team::Blue, Team::Red] {
                for p in [base_pos(team), fountain(team), titan_pos(team)] {
                    assert!(f.fits(p.x, p.z, 2.0), "{p} is not on the floor");
                }
                for lane in [Lane::Top, Lane::Bot] {
                    for inner in [false, true] {
                        let p = tower_pos(lane, team, inner);
                        assert!(f.fits(p.x, p.z, 2.0), "tower {p} is not on the floor");
                    }
                }
            }
            for p in pit_positions().into_iter().chain(heal_tree_positions()) {
                assert!(f.fits(p.x, p.z, 3.0), "{p} is not on the floor");
            }
            let lane_passages = passages().iter().filter(|p| p.lane.is_some()).count();
            assert_eq!(lane_passages, 8, "two lanes, four passages each");
            for lane in [Lane::Top, Lane::Bot] {
                let pts = lane_polyline(lane);
                let mut through = 0;
                for w in pts.windows(2) {
                    for k in 0..=40 {
                        let p = w[0] + (w[1] - w[0]) * (k as f32 / 40.0);
                        assert!(f.fits(p.x, p.z, PASSAGE_R - 1.0), "{lane:?} pinched at {p}");
                        // between rooms (out of every room's own shape), it
                        // is in one of its own passages, never a gate's
                        let in_room = rooms().any(|rm| {
                            let q = room_pos(rm);
                            if rm == HEART { q.distance(p) < HEART_R - 0.5 } else { (p.x - q.x).abs() < ROOM_HX - ROOM_HZ + (ROOM_HZ - 0.5) && (p.z - q.z).abs() < ROOM_HZ - 0.5 }
                        });
                        // (the doors from the bases are corridors, not
                        // passages: only between the rows of rooms)
                        if !in_room && p.z.abs() < ROWS as f32 * ROW_Z + ROOM_HZ {
                            let own = passages().iter().any(|g| g.lane == Some(lane) && crate::abilities::point_segment_dist(p, g.p0, g.p1) < PASSAGE_R);
                            assert!(own, "{lane:?} runs between rooms outside its own passages at {p}");
                            through += 1;
                        }
                    }
                }
                assert!(through > 8, "{lane:?}: the scan found {through} points between rooms");
            }
        });
    }

    /// Sections, not a maze: fifteen rooms, each open enough to fight in
    /// (a disc of 12 u fits in every one), walls between them thick enough
    /// to be walls, and the walls cannot be leapt.
    #[test]
    fn it_is_rooms_with_thick_walls() {
        within(|| {
            assert!(WALL_H > crate::leap::LEAP_UP + crate::leap::CLEAR_OVER, "a leap could top the walls");
            let f = floor();
            assert_eq!(rooms().count(), 15);
            for rm in rooms() {
                let p = room_pos(rm);
                assert!(f.fits(p.x, p.z, 12.0), "room {rm:?} is cramped");
                assert_eq!(section_at(p.x, p.z).map(|s| s.room()), Some(rm));
            }
            // walls between spaces: any two floor shapes that do not touch
            // are that far apart through a wall, and it is at least 8 u of
            // wall (a thinner one reads as a screen, not a labyrinth). (A
            // scan along lines measured tangent slivers where a passage
            // meets a room's edge at a right angle: not walls.)
            let axis = |sh: &Shape| match *sh {
                Shape::Circle { x, z, r } => (Vec3::new(x, 0.0, z), Vec3::new(x, 0.0, z), r),
                Shape::Segment { x0, z0, x1, z1, r } => (Vec3::new(x0, 0.0, z0), Vec3::new(x1, 0.0, z1), r),
            };
            let mut thinnest = (f32::MAX, Vec3::ZERO);
            let mut pairs = 0;
            for (i, a) in f.shapes.iter().enumerate() {
                let (a0, a1, ra) = axis(a);
                for b in f.shapes.iter().skip(i + 1) {
                    let (b0, b1, rb) = axis(b);
                    let (d, at) = (0..=40)
                        .map(|k| {
                            let p = a0.lerp(a1, k as f32 / 40.0);
                            (crate::abilities::point_segment_dist(p, b0, b1) - ra - rb, p)
                        })
                        .fold((f32::MAX, Vec3::ZERO), |m, x| if x.0 < m.0 { x } else { m });
                    if d > 0.0 {
                        pairs += 1;
                        if d < thinnest.0 {
                            thinnest = (d, at);
                        }
                    }
                }
            }
            assert!(pairs > 100, "only {pairs} pairs of shapes apart");
            assert!(thinnest.0 >= 8.0, "a wall {} u thick near {}", thinnest.0, thinnest.1);
            let mut high = 0;
            for k in 0..4000 {
                let x = (hash2(k as f32, 7.0) - 0.5) * 2.0 * ARENA_RADIUS;
                let z = (hash2(k as f32, 9.0) - 0.5) * 2.0 * ARENA_RADIUS;
                let d = f.sdf(x, z);
                if d <= 0.0 {
                    assert!(terrain_height(x, z).abs() < 0.05, "the floor is flat at {x},{z}");
                } else if d > WALL_RISE + 0.1 {
                    assert!(terrain_height(x, z) >= WALL_H - 0.01, "a low wall at {x},{z}");
                    high += 1;
                }
            }
            assert!(high > 500, "the scan found {high} wall points");
        });
    }

    /// The labyrinth turns: in every turn some gates are shut and every room
    /// can still be reached; lanes' passages never shut; a gate and its
    /// mirror move together; and from turn to turn the pattern changes.
    #[test]
    fn the_labyrinth_turns_and_never_cuts_a_room_off() {
        let w = Wake::new(&mut Rng::new(7));
        let ps = passages();
        let gates = ps.iter().filter(|p| p.lane.is_none()).count();
        assert!(gates >= 30, "{gates} gates");
        let mut changed = 0usize;
        let mut prev = w.closed(0);
        for k in 0..40 {
            let s = w.closed(k);
            let shut = s.iter().filter(|x| **x).count();
            // (up to 0.8: the machine's locks seal on top of the random share --
            // 26 of 36 in turn 4 of seed 7 -- and connectivity still holds)
            assert!(shut as f32 >= gates as f32 * 0.15 && shut as f32 <= gates as f32 * 0.8, "turn {k}: {shut} of {gates} shut");
            assert_eq!(reachable(&|i| !s[i]).len(), 15, "turn {k} cuts a room off");
            for g in 0..ps.len() {
                assert!(!(ps[g].lane.is_some() && s[g]), "turn {k} shuts lane passage {g}");
                assert_eq!(s[g], s[mirror_passage(g)], "turn {k}: gate {g} without its mirror");
            }
            if k > 0 {
                changed += s.iter().zip(&prev).filter(|(a, b)| a != b).count();
            }
            prev = s;
        }
        let per_turn = changed as f32 / 39.0;
        assert!(per_turn >= gates as f32 * 0.2, "only {per_turn:.1} gates move a turn");
        let other = Wake::new(&mut Rng::new(8));
        assert!((0..10).any(|k| other.closed(k) != w.closed(k)), "two matches, one labyrinth");
    }

    /// A slab rises over GATE_MOVE and blocks from half way; it warns for
    /// GATE_WARN before a turn that moves it, and not for one that does not.
    #[test]
    fn a_gate_moves_after_warning() {
        let w = Wake::new(&mut Rng::new(7));
        let (a, b) = (w.closed(1), w.closed(2));
        let g = (0..a.len()).find(|&g| !a[g] && b[g]).expect("a gate that shuts at the second turn");
        let t2 = FIRST_TURN + TURN;
        let at = |t: f32| w.gates_at(t)[g];
        assert_eq!(at(t2 - GATE_WARN - 1.0), (0.0, false));
        assert!(at(t2 - 1.0).1, "no warning a second before");
        let (h, _) = at(t2 + GATE_MOVE * 0.25);
        assert!(h > 0.1 && h < GATE_BLOCKS, "a quarter of the way: {h}");
        assert_eq!(at(t2 + GATE_MOVE + 0.1).0, 1.0);
        let still = (0..a.len()).find(|&g| a[g] == b[g] && passages()[g].lane.is_none()).unwrap();
        assert!(!w.gates_at(t2 - 1.0)[still].1);
        // before the first turn: no warning until its last GATE_WARN
        // seconds, and then only for the gates that first turn moves
        assert!(w.gates_at(10.0).iter().all(|x| !x.1), "gates warn at 10 s");
        let first = w.gates_at(FIRST_TURN - 1.0);
        let (z, o) = (w.closed(0), w.closed(1));
        assert!((0..z.len()).all(|g| first[g].1 == (z[g] != o[g])), "the first turn's warnings");
        assert!(first.iter().any(|x| x.1), "the first turn moves nothing");
    }

    /// The waking: nothing before the first; each cycle wakes PER_CYCLE
    /// classes, never one from the cycle before; it stirs before it bites,
    /// and quiets before the next cycle; the bases never wake; and over a
    /// round, every class has its turn.
    #[test]
    fn the_waking_moves_round_the_labyrinth_fairly() {
        let w = Wake::new(&mut Rng::new(11));
        let s = Section { col: 1, band: 1, side: -1 };
        assert_eq!(w.mood(s, FIRST_WAKE - 1.0), Mood::Calm);
        let mut woke = std::collections::HashSet::new();
        for n in 0..20u32 {
            let c = w.classes(n);
            assert!(c[0] != c[1]);
            if n > 0 {
                let p = w.classes(n - 1);
                assert!(c.iter().all(|x| !p.contains(x)), "cycle {n} repeats the one before");
            }
            woke.extend(c);
            let t0 = FIRST_WAKE + n as f32 * CYCLE;
            for &class in &c {
                assert!(matches!(w.class_mood(class, t0 + 1.0), Mood::Stirring(_)));
                assert!(matches!(w.class_mood(class, t0 + WARN + 1.0), Mood::Hostile(_)));
                assert_eq!(w.class_mood(class, t0 + WARN + HOSTILE + 0.5), Mood::Calm);
            }
        }
        assert_eq!(woke.len(), CLASSES as usize, "a class never woke in 20 cycles");
        within(|| {
            for team in [Team::Blue, Team::Red] {
                let b = base_pos(team);
                assert!(section_at(b.x, b.z).is_none(), "a base is in a section");
            }
        });
    }

    fn scene(pack: &[f32]) -> Game {
        // cat 0 in the middle of a room that is hostile at t; its friends at
        // the offsets given; one enemy far away
        let mut blue = vec![0u32];
        blue.extend(pack.iter().map(|_| 1u32));
        let mut g = Game::new_teams(&blue, &[2]);
        g.wake = Wake::default();
        let class = g.wake.classes(0)[0];
        let rm = rooms().find(|&rm| {
            let p = room_pos(rm);
            section_at(p.x, p.z).is_some_and(|s| s.class() == class)
        });
        let at = room_pos(rm.unwrap());
        g.gods[0].pos = at;
        for (k, dx) in pack.iter().enumerate() {
            g.gods[k + 1].pos = at + Vec3::new(*dx, 0.0, 0.0);
        }
        let e = g.gods.len() - 1;
        g.gods[e].pos = Vec3::new(0.0, 0.0, 400.0);
        g.t = FIRST_WAKE + WARN + 1.0;
        g
    }

    fn bitten(pack: &[f32]) -> f32 {
        let mut out = 0.0;
        within(|| {
            let mut g = scene(pack);
            let hp = g.gods[0].hp;
            for _ in 0..120 {
                g.t += 1.0 / 60.0;
                g.maze_tick(1.0 / 60.0);
            }
            out = (hp - g.gods[0].hp) / g.gods[0].max_hp;
        });
        out
    }

    /// The thorns take by the pack: alone, about a twentieth of the cat's
    /// health a second; with one friend close, about a fiftieth; with two
    /// close, nothing; and a friend out of reach does not count.
    #[test]
    fn the_thorns_spare_a_pack() {
        let alone = bitten(&[]);
        let one = bitten(&[3.0]);
        let two = bitten(&[3.0, -3.0]);
        let far = bitten(&[PACK_R + 3.0, -(PACK_R + 3.0)]);
        assert!((alone - 0.10).abs() < 0.015, "alone, two seconds: {alone}");
        assert!((one - 0.04).abs() < 0.01, "with one friend: {one}");
        assert_eq!(two, 0.0, "with two friends");
        assert!((far - alone).abs() < 0.01, "friends out of reach: {far} vs {alone}");
    }

    /// Sight in the Quarry: across a room, yes; through a wall, no; down an
    /// open passage, yes; through the same passage with its gate shut, no;
    /// past SIGHT, no. And the team's view follows: an enemy behind a wall
    /// is hidden unless a tower or another cat sees it.
    #[test]
    fn walls_and_shut_gates_hide() {
        within(|| {
            let mut g = scene(&[]);
            let a = room_pos((0, 0));
            assert!(sight(&g, a + Vec3::new(-10.0, 0.0, 0.0), a + Vec3::new(10.0, 0.0, 5.0)), "across a room");
            // (0,0) to (0,-1): a wall between the rooms' middles unless a
            // passage lines up
            // (one far from every tower: towers see through walls)
            let gp = (0..passages().len())
                .find(|&k| passages()[k].lane.is_none() && g.towers.iter().all(|t| t.pos.distance(passages()[k].mid()) > crate::consts::TOWER_RANGE + 30.0))
                .unwrap();
            let p = passages()[gp];
            let (x0, x1) = (p.p0 - p.dir() * 6.0, p.p1 + p.dir() * 6.0);
            assert!(sight(&g, x0, x1), "down an open passage");
            // a line parallel to it, well off to the side, through the wall
            let off = Vec3::new(-p.dir().z, 0.0, p.dir().x) * 14.0;
            assert!(!sight(&g, x0 + off, x1 + off), "through a wall");
            let mut b = crate::blockers::Blockers::new(300.0, 6.0);
            for k in 0..passages().len() {
                b.push_gate(crate::mazeworld::gate_slab(k));
            }
            b.set_gate(gp, false);
            g.blockers = std::sync::Arc::new(b);
            assert!(!sight(&g, x0, x1), "through a shut gate");
            // range, across one open room (no wall to do the hiding)
            let x = Vec3::X * (SIGHT * 0.5);
            assert!(sight(&g, a - x * 0.98, a + x * 0.98), "just within SIGHT");
            assert!(!sight(&g, a - x * 1.02, a + x * 1.02), "past SIGHT");
            // the team's view: the enemy (last cat) put behind a wall from
            // cat 0, far from any tower
            let e = g.gods.len() - 1;
            g.gods[0].pos = x0 + off;
            g.gods[e].pos = x1 + off;
            let blue = g.gods[0].team;
            let seen = seen_by(&g, blue);
            let towers = g.towers.iter().filter(|t| t.team == blue && t.pos.distance(x1 + off) < crate::consts::TOWER_RANGE + 2.0).count();
            assert!(!seen[e], "seen through the wall (towers near: {towers}, at {})", x1 + off);
            g.gods[e].pos = x0 + p.dir() * 3.0;
            assert!(seen_by(&g, blue)[e]);
        });
    }

    /// A bot alone in a biting room is never sent to refuge in a room with
    /// a standing enemy tower, nor through one: over many wakings, both
    /// sides, every biting room.
    #[test]
    fn the_escape_does_not_run_under_an_enemy_tower() {
        within(|| {
            let mut tried = 0;
            for seed in 0..24u64 {
                for who in [0usize, 1] {
                    let mut g = Game::new_teams(&[0], &[2]);
                    g.wake = Wake::new(&mut Rng::new(seed));
                    g.t = FIRST_WAKE + WARN + 1.0;
                    let class = g.wake.classes(0)[0];
                    let other = 1 - who;
                    g.gods[other].pos = Vec3::new(0.0, 0.0, 400.0);
                    g.gods[who].ai = Some(crate::sim::AiBrain::new(Lane::Top, 0.9));
                    let team = g.gods[who].team;
                    for rm in rooms() {
                        let at = room_pos(rm);
                        if !section_at(at.x, at.z).is_some_and(|s| s.class() == class) {
                            continue;
                        }
                        g.gods[who].pos = at;
                        let Some(goal) = bot_goal(&g, who) else { continue };
                        if goal.distance(team.fountain()) < 1.0 {
                            continue;
                        }
                        let groom = section_at(goal.x, goal.z).map(|s| s.room());
                        let towered = g.towers.iter().any(|tw| tw.team != team && tw.hp > 0.0 && section_at(tw.pos.x, tw.pos.z).map(|s| s.room()) == groom);
                        assert!(!towered, "seed {seed}: sent from {rm:?} to {groom:?}, under an enemy tower");
                        tried += 1;
                    }
                }
            }
            assert!(tried > 40, "only {tried} escapes");
        });
    }

    /// Nothing bites on calm ground, or before the first waking.
    #[test]
    fn calm_ground_does_not_bite() {
        within(|| {
            let mut g = scene(&[]);
            g.t = FIRST_WAKE - 5.0;
            let hp = g.gods[0].hp;
            for _ in 0..120 {
                g.t += 1.0 / 60.0;
                g.maze_tick(1.0 / 60.0);
            }
            assert_eq!(g.gods[0].hp, hp);
        });
    }

    /// A pack is harder to hurt anywhere on the map: with two friends close,
    /// a cat takes PACK_WARD of a blow; alone, all of it.
    #[test]
    fn a_pack_takes_less() {
        within(|| {
            let taken = |pack: &[f32]| {
                let mut g = scene(pack);
                g.t = 10.0; // no waking: only the ward
                g.sentinel_tick(1.0 / 60.0);
                g.maze_tick(1.0 / 60.0);
                let hp = g.gods[0].hp;
                g.deal_damage(crate::sim::TargetRef::God(0), 100.0, false, None, 0.0, 0.0);
                hp - g.gods[0].hp
            };
            let (alone, pack) = (taken(&[]), taken(&[3.0, -3.0]));
            assert!((pack / alone - PACK_WARD).abs() < 1e-3, "pack {pack}, alone {alone}");
        });
    }

    /// The HUD says what the ground and the pack mean, by the mechanism.
    #[test]
    fn the_banner_tells_a_cat_where_it_stands() {
        within(|| {
            let red = banner(&scene(&[]), 0).unwrap();
            let amber = banner(&scene(&[3.0]), 0).unwrap();
            let green = banner(&scene(&[3.0, -3.0]), 0).unwrap();
            assert!(red.0.contains("ALONE") && red.1[0] > 200 && red.1[1] < 100);
            assert!(amber.0.contains("ONE FRIEND"));
            assert!(green.0.contains("PACK") && green.1[1] > 200);
            let mut g = scene(&[]);
            g.t = FIRST_WAKE + 2.0;
            assert!(banner(&g, 0).unwrap().0.contains("WAKING"));
            g.t = 10.0;
            assert!(banner(&g, 0).is_none());
        });
    }

    /// The sim sets the walls from the clock: after a tick, exactly the
    /// gates the pattern has up (half way or more) are shut in the match's
    /// Blockers, and every other gate is open.
    #[test]
    fn the_sim_sets_the_gates_from_the_clock() {
        within(|| {
            let mut b = crate::blockers::Blockers::new(ARENA_RADIUS + 20.0, 6.0);
            b.set_floor(floor());
            for g in 0..passages().len() {
                b.push_gate(crate::mazeworld::gate_slab(g));
            }
            let mut game = Game::new_teams(&[0], &[1]);
            game.blockers = std::sync::Arc::new(b);
            let mut checked = 0;
            for t in [10.0, FIRST_TURN + 0.5, FIRST_TURN + GATE_MOVE + 1.0, FIRST_TURN + TURN * 3.0 + 1.9] {
                game.t = t;
                game.maze_tick(1.0 / 60.0);
                let want = game.wake.gates_at(t);
                let shut = (0..passages().len()).filter(|&g| !game.blockers.gate_open(g)).count();
                for (g, (h, _)) in want.iter().enumerate() {
                    assert_eq!(game.blockers.gate_open(g), *h < GATE_BLOCKS, "t {t}: gate {g} at {h}");
                }
                checked += shut;
            }
            assert!(checked > 10, "hardly a gate shut across the times checked ({checked})");
        });
    }

    /// A route from inside a shut gate's passage starts at the mouth on
    /// the cat's side, so it cannot lead through the slab; from an open
    /// passage, or anywhere else, from where the cat is.
    #[test]
    fn a_route_never_leads_through_a_shut_gate() {
        within(|| {
            let mut b = crate::blockers::Blockers::new(ARENA_RADIUS + 20.0, 6.0);
            b.set_floor(floor());
            for g in 0..passages().len() {
                b.push_gate(crate::mazeworld::gate_slab(g));
            }
            let mut game = Game::new_teams(&[0], &[1]);
            game.blockers = std::sync::Arc::new(b);
            let g = passages().iter().position(|p| p.lane.is_none()).unwrap();
            let p = passages()[g];
            let here = p.p0.lerp(p.p1, 0.35); // the p0 side of the slab
            let far = room_pos(p.b);
            game.blockers.set_gate(g, false);
            assert_eq!(game.route_from(here), p.p0);
            let wp = floor().next_waypoint_with(game.route_from(here), far, 0.5, &|s| game.route_open(s));
            let wp = wp.unwrap_or(far);
            assert!(wp.distance(p.p1) > 3.0 && wp.distance(far) > 3.0, "routed through the shut gate to {wp}");
            game.blockers.set_gate(g, true);
            assert_eq!(game.route_from(here), here, "open: from where it stands");
        });
    }

    /// A bot alone in waking ground is sent to a neighbouring room that is
    /// calm; with two friends it is left be; on calm ground, to its lane.
    #[test]
    fn a_lone_bot_is_sent_out_of_waking_ground() {
        within(|| {
            let g = scene(&[]);
            let goal = bot_goal(&g, 0).expect("a goal out of the thorns");
            assert!(matches!(g.wake.mood_at(goal.x, goal.z, g.t), Mood::Calm), "sent to {goal}, which is not calm");
            let here = section_at(g.gods[0].pos.x, g.gods[0].pos.z).unwrap().room();
            let there = section_at(goal.x, goal.z).unwrap().room();
            assert!(passages().iter().any(|p| (p.a == here && p.b == there) || (p.b == here && p.a == there)), "sent from {here:?} to {there:?}, not a neighbour");
            let g = scene(&[3.0, -3.0]);
            assert!(bot_goal(&g, 0).is_none(), "a pack is left be");
            let mut g = scene(&[]);
            g.t = 10.0;
            assert!(bot_goal(&g, 0).is_none(), "calm ground");
        });
    }

    /// A lone bot is sent out only through open passages: with the gates to
    /// the calm room next door shut, it is not sent there.
    #[test]
    fn a_lone_bot_is_not_sent_through_a_shut_gate() {
        within(|| {
            let mut g = scene(&[]);
            let mut b = crate::blockers::Blockers::new(ARENA_RADIUS + 20.0, 6.0);
            b.set_floor(floor());
            for k in 0..passages().len() {
                b.push_gate(crate::mazeworld::gate_slab(k));
            }
            g.blockers = std::sync::Arc::new(b);
            let here = section_at(g.gods[0].pos.x, g.gods[0].pos.z).unwrap().room();
            let open_goal = bot_goal(&g, 0).unwrap();
            let there = section_at(open_goal.x, open_goal.z).unwrap().room();
            // shut every gate between here and there
            let mut shut = 0;
            for (k, p) in passages().iter().enumerate() {
                if p.lane.is_none() && ((p.a == here && p.b == there) || (p.b == here && p.a == there)) {
                    g.blockers.set_gate(k, false);
                    shut += 1;
                }
            }
            assert!(shut > 0, "no gate between {here:?} and {there:?} to shut");
            let goal = bot_goal(&g, 0).unwrap();
            let now = section_at(goal.x, goal.z).map(|s| s.room());
            assert_ne!(now, Some(there), "sent to {there:?} through a shut gate");
        });
    }

    /// On calm ground, a lone bot whose lane runs on into waking ground
    /// holds where it is; with nothing waking ahead, it walks its lane.
    #[test]
    fn a_lone_bot_holds_at_the_edge_of_waking_ground() {
        within(|| {
            let mut g = Game::new_teams(&[0, 1], &[2]);
            g.wake = Wake::default();
            g.gods[0].ai = Some(crate::sim::AiBrain::new(Lane::Top, 0.9));
            g.gods[1].pos = Vec3::new(0.0, 0.0, 400.0);
            let path = crate::consts::lane_path(Lane::Top, Team::Blue);
            // a waypoint on calm ground whose next few are in a section
            // that wakes in the first cycle
            // (both of the cycle's classes: a spot in the second is not calm)
            let classes = g.wake.classes(0);
            let wakes = |p: &Vec3| section_at(p.x, p.z).is_some_and(|s| classes.contains(&s.class()));
            let k = (0..path.len() - 4).find(|&k| !wakes(&path[k]) && path[k + 1..k + 4].iter().any(wakes));
            let Some(k) = k else {
                // the class never meets the Top lane: nothing to hold for
                assert!(!path.iter().any(wakes));
                return;
            };
            g.gods[0].pos = path[k];
            g.gods[0].ai.as_mut().unwrap().wp = k;
            g.t = FIRST_WAKE + 1.0;
            assert_eq!(bot_goal(&g, 0), Some(path[k]), "should hold at the edge");
            g.t = 10.0;
            assert_eq!(bot_goal(&g, 0), None, "nothing waking: walk on");
        });
    }
}
