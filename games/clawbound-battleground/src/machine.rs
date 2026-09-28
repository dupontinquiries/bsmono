//! machine — the Quarry's clockwork: the stones it moves, and when.
//!
//! The labyrinth is a machine, the works of the Geartooth clan of
//! engineer-cats (gear trains on the wall tops, stone gates in their slots).
//! It was built to move the mountain: quarried stone is lifted from the
//! cisterns in the hills (or a hopper on a wall top) by Archimedes screws,
//! rolls down a trestle chute, drops off its lip into the labyrinth and
//! rolls on through the rooms.
//!
//! Each turn of the labyrinth (`maze::Wake::turn`) the machine works one
//! pair of TRACKS -- a track and its mirror. The rooms a track runs through
//! become LOCKS for the turn ([`lock_rooms`]): their other gates seal
//! (`maze::Wake::closed`, which also holds the track's own passages open,
//! [`held_open`]). The screws wind for [`ROLL_WARN`] s, and at [`ROLL_AT`]
//! into the turn each stone is let go down its chute at [`SPEED`]. On the
//! floor it runs down anything in its way ([`HIT_SHARE`] of its health,
//! thrown aside [`KNOCK`], once a run), and as it passes, the lock's PADS
//! -- stone trapdoors either side of the track ([`pads`]) -- drop open
//! under it, and a cat standing on one falls to its death ([`PadPhase`]).
//! That is why a cat stays out of a sealed room.
//!
//! Everything is a function of the match clock and the match's seed (like
//! the gates): every client sees the same stone and the same open pad.
//! What it does is the sim's ([`Game::machine_tick`]). Bots leave a lock
//! (`maze::bot_goal`) and step clear of a ball about to reach them
//! ([`dodge`]). The machine knows its own: a Sentinel (the same clan built
//! them) is not thrown by a stone and braces across a pad.
//!
//! Headless. `mazeworld` builds the chutes, hoppers, screws and pads' meshes;
//! `scene` draws the stones, the pads and the gear trains turning (faster
//! while the machine works: [`drive`]).

use glam::Vec3;
use std::sync::OnceLock;

use crate::maze::{self as mz, Room};
use crate::sim::{Event, Game};

/// The ball's radius.
pub const BOULDER_R: f32 = 2.2;
/// How fast it rolls (along its path, chute and fall included), u/s.
pub const SPEED: f32 = 15.0;
/// When in a turn it is let go, and how long the screws wind before.
pub const ROLL_AT: f32 = 10.0;
pub const ROLL_WARN: f32 = 4.0;
/// Its fall off the chute's lip.
pub const GRAVITY: f32 = 30.0;
/// What it does to a cat it rolls over: a share of its health, thrown aside
/// this far, and slowed. (0.35, from 0.18: playtested, a stone "wasn't
/// doing much" -- under a fifth of a bar was shrugged off.)
pub const HIT_SHARE: f32 = 0.35;
pub const KNOCK: f32 = 4.5;
pub const HIT_SLOW: f32 = 0.4;
/// How soon (s) a ball must be due at a bot before it steps off the track.
pub const REACT: f32 = 1.5;
/// The pads: square trapdoors `2 * PAD_HALF` across, every `PAD_STEP` along
/// a track, centred `PAD_OFF` to either side of it.
pub const PAD_HALF: f32 = 2.5;
pub const PAD_STEP: f32 = 6.0;
pub const PAD_OFF: f32 = 8.0;
/// A pad drops as the ball passes it: swinging open over PAD_FALL (a cat on
/// it falls from PAD_BITE in), open for PAD_OPEN, closing over PAD_RISE.
pub const PAD_FALL: f32 = 0.4;
pub const PAD_BITE: f32 = 0.15;
pub const PAD_OPEN: f32 = 4.0;
pub const PAD_RISE: f32 = 1.2;
/// Where a bot steps to, clear of a track and of its pads.
pub const DODGE_OUT: f32 = PAD_OFF + PAD_HALF + 2.0;

/// A track: the stone's whole path -- the hopper, the chute's lip, the fall
/// off it, then along the floor through rooms and passages.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub pts: Vec<Vec3>,
    /// How far along `pts` it lands on the floor.
    pub floor_from: f32,
    /// The passages it runs through (held open while it runs).
    pub passages: Vec<usize>,
    /// The rooms its floor part runs through: the locks.
    pub rooms: Vec<Room>,
}

impl Track {
    pub fn len(&self) -> f32 {
        self.pts.windows(2).map(|w| (w[1] - w[0]).length()).sum()
    }
    /// The point `s` along it (the path's own height: 0 on the floor).
    pub fn at(&self, s: f32) -> Vec3 {
        crate::pass::along(&self.pts, s.clamp(0.0, self.len()))
    }
    /// Where the stone waits to be wound up, and where it leaves the chute.
    pub fn hopper(&self) -> Vec3 {
        self.pts[0]
    }
    pub fn lip(&self) -> Vec3 {
        self.pts[1]
    }
    /// Flat distance from `p` to the track's floor part (from `s0` on).
    pub fn dist(&self, p: Vec3, s0: f32) -> f32 {
        let s0 = s0.max(self.floor_from - 1e-3);
        let mut best = f32::MAX;
        let mut acc = 0.0;
        for w in self.pts.windows(2) {
            let l = (w[1] - w[0]).length();
            if acc + l >= s0 {
                let a = if s0 > acc { w[0] + (w[1] - w[0]) * ((s0 - acc) / l.max(1e-6)) } else { w[0] };
                let (fa, fb, fp) = (Vec3::new(a.x, 0.0, a.z), Vec3::new(w[1].x, 0.0, w[1].z), Vec3::new(p.x, 0.0, p.z));
                best = best.min(crate::abilities::point_segment_dist(fp, fa, fb));
            }
            acc += l;
        }
        best
    }
    /// The flat way it runs at `s`.
    pub fn dir_at(&self, s: f32) -> Vec3 {
        let (a, b) = (self.at(s), self.at(s + 1.0));
        let c = if Vec3::new(b.x - a.x, 0.0, b.z - a.z).length() > 1e-3 { b - a } else { a - self.at(s - 1.0) };
        Vec3::new(c.x, 0.0, c.z).normalize_or_zero()
    }
}

fn mirror(p: Vec3) -> Vec3 {
    Vec3::new(p.x, p.y, -p.z)
}

/// The passage between rooms `a` and `b` whose ends are `off` (dx or dz)
/// from their centres' line.
fn passage(a: Room, b: Room, off: f32) -> usize {
    mz::passages()
        .iter()
        .position(|p| {
            let rooms = (p.a == a && p.b == b) || (p.a == b && p.b == a);
            let o = if a.0 == b.0 { p.p0.x - mz::room_pos(p.a).x } else { p.p0.z - mz::room_pos(p.a).z };
            rooms && (o - off).abs() < 0.5
        })
        .unwrap_or_else(|| panic!("no passage {a:?}-{b:?} at {off}"))
}

/// A stone's path whose floor run goes from `start` to `end`: back from
/// `start` to the wall behind it (the chute's lip), and on back to the
/// hopper -- up in the hills if they rise within 40 u, else on the deepest
/// wall top within 12 u -- then off the lip, falling, onto the floor.
fn build(start: Vec3, end: Vec3, passages: Vec<usize>) -> Track {
    let f = mz::floor();
    let dir = Vec3::new(end.x - start.x, 0.0, end.z - start.z).normalize_or_zero();
    let mut lip = start;
    for _ in 0..200 {
        if f.sdf(lip.x, lip.z) >= 0.5 {
            break;
        }
        lip -= dir * 0.25;
    }
    let lip_y = mz::terrain_height(lip.x, lip.z).max(mz::WALL_H) + 1.0;
    let hills = (1..=160).map(|k| lip - dir * (k as f32 * 0.25)).find(|q| mz::terrain_height(q.x, q.z) >= mz::WALL_H + 11.0);
    let hopper = hills.unwrap_or_else(|| {
        (1..=48).map(|k| lip - dir * (k as f32 * 0.25)).max_by(|a, b| f.sdf(a.x, a.z).total_cmp(&f.sdf(b.x, b.z))).unwrap()
    });
    let hopper_y = mz::terrain_height(hopper.x, hopper.z) + 2.5;
    let mut pts = vec![Vec3::new(hopper.x, hopper_y, hopper.z), Vec3::new(lip.x, lip_y, lip.z)];
    let tf = (2.0 * lip_y / GRAVITY).sqrt();
    for i in 1..=8 {
        let t = tf * i as f32 / 8.0;
        let q = lip + dir * (SPEED * t);
        pts.push(Vec3::new(q.x, (lip_y - 0.5 * GRAVITY * t * t).max(0.0), q.z));
    }
    let floor_from: f32 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    pts.push(Vec3::new(end.x, 0.0, end.z));
    let mut t = Track { pts, floor_from, passages, rooms: Vec::new() };
    let (a, n) = (t.floor_from, (t.len() - t.floor_from) as usize);
    for k in 0..=n {
        let p = t.at(a + k as f32);
        if let Some(sec) = mz::section_at(p.x, p.z) {
            if !t.rooms.contains(&sec.room()) {
                t.rooms.push(sec.room());
            }
        }
    }
    t
}

/// A straight run across a row, west to east, `dz` off its centre line,
/// through the two passages at that `dz`: its stones come down from the
/// western hills.
fn across(row: i32, dz: f32) -> Track {
    let z = row as f32 * mz::ROW_Z + dz;
    build(Vec3::new(mz::COLS[0] - 22.0, 0.0, z), Vec3::new(mz::COLS[2] + 22.0, 0.0, z), vec![passage((0, row), (1, row), dz), passage((1, row), (2, row), dz)])
}

/// The run up the middle column from the blue end into the Heart, `dx` off
/// its centre line: its stones drop from a hopper on the wall by the base.
fn up_the_middle(dx: f32) -> Track {
    build(Vec3::new(dx, 0.0, -2.0 * mz::ROW_Z - 16.0), Vec3::new(dx, 0.0, -8.0), vec![passage((1, -2), (1, -1), dx), passage((1, -1), (1, 0), dx)])
}

fn mirrored(t: &Track) -> Track {
    Track {
        pts: t.pts.iter().map(|&p| mirror(p)).collect(),
        floor_from: t.floor_from,
        passages: t.passages.iter().map(|&g| mz::mirror_passage(g)).collect(),
        rooms: t.rooms.iter().map(|&(c, r)| (c, -r)).collect(),
    }
}

/// The track pairs: across the Heart's row, across the pits' rows, and up
/// the middle into the Heart from both ends. Each pair is a track and its
/// mirror; they run together.
pub fn pairs() -> &'static [[Track; 2]] {
    static P: OnceLock<Vec<[Track; 2]>> = OnceLock::new();
    P.get_or_init(|| {
        crate::map::with(crate::map::MapId::Maze, || {
            let heart = across(0, -9.0);
            let pits = across(-1, -9.0);
            let middle = up_the_middle(-14.0);
            [heart, pits, middle].into_iter().map(|t| {
                let m = mirrored(&t);
                [t, m]
            }).collect()
        })
    })
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The track pair the machine runs in turn `k` (none before the first
/// turn). Never the same pair two turns running.
pub fn pair_for(seed: u64, k: u32) -> Option<usize> {
    if k == 0 {
        return None;
    }
    let n = pairs().len() as u64;
    let pick = |k: u32| (splitmix(seed ^ 0xB0_1DE2 ^ (k as u64).wrapping_mul(0x9E37_79B9)) % n) as usize;
    let p = pick(k);
    if k > 1 && p == pair_for(seed, k - 1).unwrap_or(usize::MAX) {
        Some((p + 1 + (splitmix(seed ^ k as u64) % (n - 1)) as usize) % n as usize)
    } else {
        Some(p)
    }
}

/// The passages the machine holds open in turn `k` (its tracks').
pub fn held_open(seed: u64, k: u32) -> Vec<usize> {
    pair_for(seed, k).map(|p| pairs()[p].iter().flat_map(|t| t.passages.iter().copied()).collect()).unwrap_or_default()
}

/// The locks in turn `k`: the rooms its tracks run through. Their other
/// gates seal for the turn (`maze::Wake::closed`).
pub fn lock_rooms(seed: u64, k: u32) -> Vec<Room> {
    pair_for(seed, k).map(|p| pairs()[p].iter().flat_map(|t| t.rooms.iter().copied()).collect()).unwrap_or_default()
}

/// How long into a turn its locks are dangerous: until the last pad has
/// closed again.
pub fn lock_ends(p: usize) -> f32 {
    let last = pads(p).iter().map(|d| d.s).fold(0.0f32, f32::max);
    ROLL_AT + last / SPEED + PAD_FALL + PAD_OPEN + PAD_RISE
}

/// Is `(x, z)` in a lock the machine is working at `t`?
pub fn locked(seed: u64, t: f32, x: f32, z: f32) -> bool {
    let (k, into) = mz::Wake::turn(t);
    let Some(p) = pair_for(seed, k) else { return false };
    if into > lock_ends(p) {
        return false;
    }
    mz::section_at(x, z).is_some_and(|s| pairs()[p].iter().any(|t| t.rooms.contains(&s.room())))
}

/// A boulder's doing at some moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ball {
    /// Which pair and which of its two tracks, and the run's id (unique per
    /// turn and track: a cat is hit once a run).
    pub pair: usize,
    pub side: usize,
    pub run: u32,
    /// The ball's centre.
    pub pos: Vec3,
    /// Unit, flat: the way it is rolling.
    pub dir: Vec3,
    /// How far along the track (the scene turns it by this).
    pub s: f32,
    /// `0..1` while the screw winds it up out of the hopper; 1 rolling;
    /// `1..0` as it sinks away at the end.
    pub up: f32,
    pub rolling: bool,
    /// Down on the floor (it hits nothing on the chute or in the air).
    pub on_floor: bool,
}

/// Every boulder at match time `t`.
pub fn boulders_at(seed: u64, t: f32) -> Vec<Ball> {
    let (k, into) = mz::Wake::turn(t);
    let Some(p) = pair_for(seed, k) else { return Vec::new() };
    let mut out = Vec::new();
    for (side, tr) in pairs()[p].iter().enumerate() {
        let len = tr.len();
        let run_t = into - ROLL_AT;
        let s = (run_t * SPEED).clamp(0.0, len);
        let dir = tr.dir_at(s);
        let run = k * 2 + side as u32 + 1;
        let ball = |pos: Vec3, up: f32, rolling: bool| Ball { pair: p, side, run, pos, dir, s, up, rolling, on_floor: s >= tr.floor_from };
        if run_t < -ROLL_WARN {
            continue;
        } else if run_t < 0.0 {
            // wound up out of the hopper by its screw
            let up = (run_t + ROLL_WARN) / ROLL_WARN;
            out.push(ball(tr.hopper() + Vec3::Y * (BOULDER_R - (1.0 - up) * (2.0 * BOULDER_R + 0.5)), up, false));
        } else if s < len {
            out.push(ball(tr.at(s) + Vec3::Y * BOULDER_R, 1.0, true));
        } else {
            let gone = run_t - len / SPEED;
            if gone >= 1.0 {
                continue;
            }
            let up = 1.0 - gone;
            out.push(ball(tr.at(len) + Vec3::Y * (BOULDER_R * up - BOULDER_R * (1.0 - up)), up, false));
        }
    }
    out
}

/// How hard the machine is working at `t`, `0..1`: the gears idle at a
/// crawl, and spin up while the gates move and while its boulders wind and
/// roll.
pub fn drive(seed: u64, t: f32) -> f32 {
    let (k, into) = mz::Wake::turn(t);
    let gates = if k > 0 && into < mz::GATE_MOVE + 0.5 { 1.0 } else { 0.0 };
    let balls = boulders_at(seed, t).iter().map(|b| if b.rolling { 1.0 } else { 0.7 * b.up }).fold(0.0f32, f32::max);
    0.15 + 0.85 * f32::max(gates, balls)
}

/// A pad: a stone trapdoor beside a track, in one of its locks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pad {
    /// Its centre, on the floor; squares are axis-aligned (every track is).
    pub at: Vec3,
    /// Where along its track the ball passes it (it drops then).
    pub s: f32,
    /// Which track of the pair.
    pub side: usize,
    /// Unit, flat: from the track out to the pad (it hinges on its far edge).
    pub out: Vec3,
}

impl Pad {
    pub fn covers(&self, p: Vec3, margin: f32) -> bool {
        (p.x - self.at.x).abs() < PAD_HALF + margin && (p.z - self.at.z).abs() < PAD_HALF + margin
    }
}

/// Where a pad is in its drop at some moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PadPhase {
    Shut,
    /// The machine is working its lock and the ball has not passed yet:
    /// `0..1` to the drop.
    Armed(f32),
    /// Swinging open, `0..1`.
    Falling(f32),
    Open,
    /// Swinging shut, `0..1`.
    Rising(f32),
}

impl PadPhase {
    /// Does a cat standing on it fall?
    pub fn deadly(self) -> bool {
        match self {
            PadPhase::Falling(f) => f * PAD_FALL >= PAD_BITE,
            PadPhase::Open => true,
            _ => false,
        }
    }
    /// How far open, `0..1` (the scene swings it by this).
    pub fn open(self) -> f32 {
        match self {
            PadPhase::Shut | PadPhase::Armed(_) => 0.0,
            PadPhase::Falling(f) => f,
            PadPhase::Open => 1.0,
            PadPhase::Rising(f) => 1.0 - f,
        }
    }
}

/// The pads of pair `p` (both tracks'). Along each track's floor run, every
/// PAD_STEP, PAD_OFF to either side -- wherever a whole pad fits in a room
/// (not a passage), clear of the lanes, passage mouths, springs, towers,
/// the other dressing, and its own mirror. Found on the pair's first track
/// and mirrored.
pub fn pads(p: usize) -> &'static [Pad] {
    static P: OnceLock<Vec<Vec<Pad>>> = OnceLock::new();
    &P.get_or_init(|| {
        crate::map::with(crate::map::MapId::Maze, || {
            // pair by pair, each clear of the pads already laid (the middle
            // run crosses the Heart's row)
            let mut all: Vec<Vec<Pad>> = Vec::new();
            for p in 0..pairs().len() {
                let laid: Vec<Pad> = all.iter().flatten().copied().collect();
                all.push(pads_of(p, &laid));
            }
            all
        })
    })[p]
}

fn pads_of(p: usize, laid: &[Pad]) -> Vec<Pad> {
    let f = mz::floor();
    let pair = &pairs()[p];
    let tr = &pair[0];
    let items: Vec<Vec3> = crate::mazeworld::placements_without_tablets()
        .into_iter()
        .filter(|i| !matches!(i, crate::mazeworld::Item::Screw { .. }))
        .flat_map(|i| match i {
            crate::mazeworld::Item::Stub { a, b } => vec![a, b, (a + b) * 0.5],
            other => vec![crate::mazeworld::item_at(&other)],
        })
        .collect();
    let lanes: Vec<Vec<Vec3>> = [crate::consts::Lane::Top, crate::consts::Lane::Bot].map(mz::lane_polyline).into();
    let lane_d = |c: Vec3| lanes.iter().flat_map(|l| l.windows(2).map(|w| crate::abilities::point_segment_dist(c, w[0], w[1]))).fold(f32::MAX, f32::min);
    let mut out = Vec::new();
    let mut s = tr.floor_from + 4.0;
    while s < tr.len() - 4.0 {
        let c0 = tr.at(s);
        let dir = tr.dir_at(s);
        let side = Vec3::new(-dir.z, 0.0, dir.x);
        for lat in [1.0f32, -1.0] {
            let c = Vec3::new(c0.x, 0.0, c0.z) + side * (lat * PAD_OFF);
            let corners = [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0), (0.0, 0.0)].map(|(a, b)| Vec3::new(c.x + a * PAD_HALF, 0.0, c.z + b * PAD_HALF));
            let ok = corners.iter().all(|q| f.fits(q.x, q.z, 0.6) && f.shape_at(q.x, q.z).and_then(mz::shape_passage).is_none())
                && lane_d(c) > PAD_HALF + 3.5
                && mz::passages().iter().all(|g| g.p0.distance(c) > PAD_HALF + 3.0 && g.p1.distance(c) > PAD_HALF + 3.0)
                && mz::heal_tree_positions().iter().all(|h| h.distance(c) > PAD_HALF + 3.5)
                && [crate::consts::Lane::Top, crate::consts::Lane::Bot].iter().all(|&l| [crate::consts::Team::Blue, crate::consts::Team::Red].iter().all(|&tm| [false, true].iter().all(|&inner| mz::tower_pos(l, tm, inner).distance(c) > PAD_HALF + 5.0)))
                && items.iter().all(|q| Vec3::new(q.x, 0.0, q.z).distance(c) > PAD_HALF + 2.0)
                && c.z.abs() > PAD_HALF + 0.5
                && pair[1].dist(c, 0.0) > PAD_HALF + BOULDER_R + 1.0
                && laid.iter().all(|d| (d.at.x - c.x).abs() >= PAD_HALF * 2.0 + 0.5 || (d.at.z - c.z).abs() >= PAD_HALF * 2.0 + 0.5);
            if ok {
                out.push(Pad { at: c, s, side: 0, out: side * lat });
            }
        }
        s += PAD_STEP;
    }
    let mirrored: Vec<Pad> = out.iter().map(|d| Pad { at: mirror(d.at), s: d.s, side: 1, out: mirror(d.out) }).collect();
    out.extend(mirrored);
    out
}

/// Where pad `d` of pair `p` is at match time `t`.
pub fn pad_phase(seed: u64, t: f32, p: usize, d: &Pad) -> PadPhase {
    let (k, into) = mz::Wake::turn(t);
    if pair_for(seed, k) != Some(p) {
        return PadPhase::Shut;
    }
    let trig = ROLL_AT + d.s / SPEED;
    let a = into - trig;
    if a < 0.0 {
        PadPhase::Armed((into / trig).clamp(0.0, 1.0))
    } else if a < PAD_FALL {
        PadPhase::Falling(a / PAD_FALL)
    } else if a < PAD_FALL + PAD_OPEN {
        PadPhase::Open
    } else if a < PAD_FALL + PAD_OPEN + PAD_RISE {
        PadPhase::Rising((a - PAD_FALL - PAD_OPEN) / PAD_RISE)
    } else {
        PadPhase::Shut
    }
}

impl Game {
    /// Once a tick, on the Quarry: the boulders roll over whatever cats
    /// are in their way, and the pads they pass drop whatever cats stand on
    /// them. (Their places are the clock's; this is only what they do.)
    pub fn machine_tick(&mut self, dt: f32) {
        if crate::map::active() != crate::map::MapId::Maze {
            return;
        }
        let t = self.t;
        let seed = self.wake.seed;
        let now = boulders_at(seed, t);
        for b in &now {
            // a rumble as each is let go, a thud as it lands
            if b.rolling && b.s <= SPEED * dt + 1e-3 {
                self.events.push(Event::BoulderRoll { pos: b.pos });
            }
            let ff = pairs()[b.pair][b.side].floor_from;
            if b.rolling && b.s >= ff && b.s - SPEED * dt < ff {
                self.events.push(Event::BoulderLand { pos: b.pos });
            }
        }
        for b in now.iter().filter(|b| b.rolling && b.on_floor) {
            for i in 0..self.gods.len() {
                let g = &self.gods[i];
                if g.respawn_t > 0.0 || g.boulder_run == b.run || g.leap.is_some() {
                    continue;
                }
                let off = Vec3::new(g.pos.x - b.pos.x, 0.0, g.pos.z - b.pos.z);
                if off.length() > BOULDER_R + crate::consts::GOD_RADIUS + 0.3 {
                    continue;
                }
                // thrown to the side it stood on
                let side = Vec3::new(-b.dir.z, 0.0, b.dir.x);
                let sgn = if off.dot(side) >= 0.0 { 1.0 } else { -1.0 };
                let to = g.pos + side * sgn * KNOCK + b.dir * 1.0;
                let to = self.blockers.resolve(Vec3::new(to.x, crate::map::ground_y(to.x, to.z), to.z), crate::consts::GOD_RADIUS);
                let dmg = g.max_hp * HIT_SHARE;
                // the machine knows its own: the same clan built the
                // Sentinels, and a ball rolls over one without throwing her
                // (she still takes the blow)
                let kin = self.def(i).role == crate::gods::Role::Sentinel;
                let g = &mut self.gods[i];
                g.boulder_run = b.run;
                if !kin {
                    g.pos = to;
                    g.slow = g.slow.max(HIT_SLOW);
                    g.slow_t = g.slow_t.max(1.0);
                    g.casting = None;
                }
                // the machine's own: no one's kill, no armour turns it
                self.deal_damage(crate::sim::TargetRef::God(i), dmg, true, None, 1.0, 0.0);
                let at = self.gods[i].pos;
                self.events.push(Event::BoulderHit { pos: at });
            }
        }
        // the pads
        let (k, _) = mz::Wake::turn(t);
        let Some(p) = pair_for(seed, k) else { return };
        for d in pads(p) {
            let ph = pad_phase(seed, t, p, d);
            if matches!(ph, PadPhase::Falling(_)) && !matches!(pad_phase(seed, t - dt, p, d), PadPhase::Falling(_)) {
                self.events.push(Event::PadDrop { pos: d.at });
            }
            if !ph.deadly() {
                continue;
            }
            for i in 0..self.gods.len() {
                let g = &self.gods[i];
                if g.respawn_t > 0.0 || g.hp <= 0.0 || g.leap.is_some() || !d.covers(g.pos, 0.0) {
                    continue;
                }
                // a Sentinel is too broad to fall: she braces across it
                if self.def(i).role == crate::gods::Role::Sentinel {
                    continue;
                }
                let at = g.pos;
                let all = g.max_hp * 10.0;
                self.deal_damage(crate::sim::TargetRef::God(i), all, true, None, 1.0, 0.0);
                self.events.push(Event::CatFell { pos: at });
            }
        }
    }
}

/// Where a bot should step to get off a track a boulder is about to reach
/// it on (None: it is not in the way, or not yet). Clear of the ball and of
/// the pads beside it, on the floor; to the side it is already on if it can.
pub fn dodge(game: &Game, gi: usize) -> Option<Vec3> {
    if crate::map::active() != crate::map::MapId::Maze {
        return None;
    }
    let me = game.gods[gi].pos;
    for b in boulders_at(game.wake.seed, game.t) {
        if b.up <= 0.0 || (!b.rolling && b.s > 0.0) {
            continue; // gone, or dropping away at the end
        }
        let tr = &pairs()[b.pair][b.side];
        if tr.dist(me, b.s) >= BOULDER_R + 2.5 {
            continue; // not on its line
        }
        // the nearest point on the track's floor run (to tell which side,
        // and how far along it the cat stands)
        let (mut best, mut s_me) = ((f32::MAX, tr.at(tr.floor_from)), tr.floor_from);
        let n = (tr.len() - tr.floor_from) as usize;
        for k in 0..=n {
            let s = tr.floor_from + k as f32;
            let p = tr.at(s);
            let d = Vec3::new(p.x - me.x, 0.0, p.z - me.z).length();
            if d < best.0 {
                best = (d, p);
                s_me = s;
            }
        }
        // only when the ball is nearly on it: a bot that stepped aside for
        // the whole wind-up and the whole run (anywhere along a 100 u
        // track) left its pack and its path, and walked into the thorns
        // (seed 5: 17.5 s alone in them against 7.4 s without dodging)
        let wind_left = if b.rolling { 0.0 } else { (1.0 - b.up) * ROLL_WARN };
        let reach = wind_left + (s_me - b.s) / SPEED;
        if s_me < b.s - BOULDER_R || reach > REACT {
            continue;
        }
        let from_line = Vec3::new(best.1.x, 0.0, best.1.z);
        let dir = tr.dir_at(s_me);
        let side = Vec3::new(-dir.z, 0.0, dir.x);
        let sgn = if Vec3::new(me.x - from_line.x, 0.0, me.z - from_line.z).dot(side) >= 0.0 { 1.0 } else { -1.0 };
        let f = mz::floor();
        let on_pad = |q: Vec3| pads(b.pair).iter().any(|d| d.covers(q, 1.0));
        for off in [sgn * DODGE_OUT, -sgn * DODGE_OUT, sgn * (BOULDER_R + 4.0), -sgn * (BOULDER_R + 4.0)] {
            let to = from_line + side * off;
            if f.fits(to.x, to.z, 0.8) && !on_pad(to) {
                return Some(to);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::Rng;

    fn within(f: impl FnOnce()) {
        crate::map::with(crate::map::MapId::Maze, f)
    }

    /// Every track: a hopper high up (in the hills for the rows; on a wall
    /// top for the middle), a chute that clears the ground under it down to
    /// a lip at the wall's edge, a fall onto the floor inside a room, and a
    /// floor run the ball fits along, through the passages it names (gates,
    /// never a lane's); its rooms are its locks; its mirror is its pair.
    #[test]
    fn the_tracks_come_down_from_the_hills_through_their_passages() {
        within(|| {
            let f = mz::floor();
            assert_eq!(pairs().len(), 3);
            for (i, pair) in pairs().iter().enumerate() {
                for tr in pair {
                    let (h, l) = (tr.hopper(), tr.lip());
                    let hill = mz::terrain_height(h.x, h.z);
                    if i < 2 {
                        assert!(hill >= mz::WALL_H + 11.0, "pair {i}: the hopper is not in the hills ({hill})");
                    } else {
                        assert!(f.sdf(h.x, h.z) > 4.0 && hill >= mz::WALL_H - 0.5, "pair {i}: the hopper is not on a wall top");
                    }
                    assert!(h.y > l.y, "the chute runs uphill");
                    assert!(f.sdf(l.x, l.z) >= 0.5 && f.sdf(l.x, l.z) < 1.0, "the lip is not at the wall's edge ({})", f.sdf(l.x, l.z));
                    for k in 0..=40 {
                        let c = h + (l - h) * (k as f32 / 40.0);
                        assert!(c.y > mz::terrain_height(c.x, c.z) + 0.3, "the chute runs into the ground at {c}");
                    }
                    let land = tr.at(tr.floor_from);
                    assert!(land.y.abs() < 1e-3 && f.fits(land.x, land.z, BOULDER_R), "it lands at {land}");
                    assert!(tr.len() - tr.floor_from > 100.0, "a short run: {}", tr.len() - tr.floor_from);
                    let n = (tr.len() - tr.floor_from) as usize;
                    for k in 0..=n {
                        let p = tr.at(tr.floor_from + k as f32);
                        assert!(p.y.abs() < 1e-3 && f.fits(p.x, p.z, BOULDER_R + 0.2), "the ball does not fit at {p}");
                    }
                    for &g in &tr.passages {
                        let ps = mz::passages()[g];
                        assert!(ps.lane.is_none(), "a track through a lane's passage");
                        assert!(tr.dist(ps.mid(), 0.0) < 0.5, "the track misses passage {g}");
                        assert!(tr.rooms.contains(&ps.a) && tr.rooms.contains(&ps.b), "passage {g}'s rooms are not locks");
                    }
                    assert_eq!(tr.rooms.len(), 3, "{:?}", tr.rooms);
                }
                let m = mirrored(&pair[0]);
                assert_eq!(m.pts, pair[1].pts);
            }
        });
    }

    /// The machine runs one pair a turn, never the same twice running,
    /// every pair in time; it holds that pair's gates open for the turn and
    /// seals its locks' others (but a lane's) -- all but the few the
    /// labyrinth must reopen so no room is cut off.
    #[test]
    fn one_pair_a_turn_its_gates_open_and_its_locks_sealed() {
        let w = mz::Wake::new(&mut Rng::new(3));
        assert_eq!(pair_for(w.seed, 0), None);
        let mut seen = std::collections::HashSet::new();
        let (mut sealed, mut lock_gates) = (0, 0);
        for k in 1..60 {
            let p = pair_for(w.seed, k).unwrap();
            seen.insert(p);
            if k > 1 {
                assert_ne!(Some(p), pair_for(w.seed, k - 1), "turn {k} repeats the pair");
            }
            let shut = w.closed(k);
            let held = held_open(w.seed, k);
            for g in &held {
                assert!(!shut[*g], "turn {k}: the machine's gate {g} is shut");
            }
            let locks = lock_rooms(w.seed, k);
            for (g, ps) in mz::passages().iter().enumerate() {
                if ps.lane.is_none() && !held.contains(&g) && (locks.contains(&ps.a) || locks.contains(&ps.b)) {
                    lock_gates += 1;
                    sealed += shut[g] as usize;
                }
            }
        }
        assert_eq!(seen.len(), pairs().len(), "a pair never ran");
        assert!(lock_gates > 50 && sealed * 10 >= lock_gates * 7, "{sealed} of {lock_gates} lock gates sealed");
    }

    /// A stone winds up out of its hopper, comes down the chute at SPEED
    /// from ROLL_AT, lands, rolls the floor, sinks at the end, and is gone
    /// -- and every pad has closed again -- before the next turn's gates
    /// warn.
    #[test]
    fn a_boulder_comes_down_on_time() {
        within(|| {
            let w = mz::Wake::new(&mut Rng::new(3));
            let t0 = mz::FIRST_TURN + mz::TURN; // turn 2
            let p = pair_for(w.seed, 2).unwrap();
            let tr = &pairs()[p][0];
            let len = tr.len();
            assert!(boulders_at(w.seed, t0 + ROLL_AT - ROLL_WARN - 0.5).is_empty(), "out before its warning");
            let winding = boulders_at(w.seed, t0 + ROLL_AT - 1.0);
            assert_eq!(winding.len(), 2);
            assert!(winding.iter().all(|b| !b.rolling && !b.on_floor && b.up > 0.5 && b.up < 1.0 && (b.pos.x - pairs()[p][b.side].hopper().x).abs() < 1e-3));
            let chute = boulders_at(w.seed, t0 + ROLL_AT + 0.5);
            assert!(chute.iter().all(|b| b.rolling && !b.on_floor && b.pos.y > mz::WALL_H), "not up on the chute");
            let t_floor = tr.floor_from / SPEED;
            let mid = boulders_at(w.seed, t0 + ROLL_AT + t_floor + 2.0);
            assert!(mid.iter().all(|b| b.rolling && b.on_floor && (b.pos.y - BOULDER_R).abs() < 1e-3));
            assert!((mid[0].pos - mirror(mid[1].pos)).length() < 1e-3, "the two roll mirrored");
            let after = t0 + ROLL_AT + len / SPEED + 1.2;
            assert!(boulders_at(w.seed, after).is_empty(), "still there after its run");
            for q in 0..pairs().len() {
                let len = pairs()[q][0].len();
                assert!(ROLL_AT + len / SPEED + 1.2 < mz::TURN - mz::GATE_WARN, "pair {q}'s run overlaps the next turn's gates");
                assert!(lock_ends(q) < mz::TURN - mz::GATE_WARN, "pair {q}'s pads are still open at the next turn: {}", lock_ends(q));
            }
        });
    }

    fn scene_with_ball(offset: f32) -> (Game, Ball) {
        scene_with_ball_for(0, offset)
    }

    /// A cat `offset` to the side of a ball rolling on the floor.
    fn scene_with_ball_for(god: u32, offset: f32) -> (Game, Ball) {
        let mut g = Game::new_teams(&[god], &[1]);
        g.wake = mz::Wake::new(&mut Rng::new(3));
        let p = pair_for(g.wake.seed, 2).unwrap();
        let t = mz::FIRST_TURN + mz::TURN + ROLL_AT + pairs()[p][0].floor_from / SPEED + 2.0;
        g.t = t;
        let b = boulders_at(g.wake.seed, t)[0];
        assert!(b.on_floor);
        let side = Vec3::new(-b.dir.z, 0.0, b.dir.x);
        g.gods[0].pos = Vec3::new(b.pos.x, 0.0, b.pos.z) + side * offset;
        g.gods[1].pos = Vec3::new(0.0, 0.0, 400.0);
        (g, b)
    }

    /// A cat the boulder rolls over takes HIT_SHARE of its health and is
    /// thrown aside KNOCK units, once a run; a cat beside the track is
    /// untouched; and nothing is hit from the chute.
    #[test]
    fn a_boulder_hits_once_and_throws_aside() {
        within(|| {
            let (mut g, b) = scene_with_ball(0.5);
            let (hp, at) = (g.gods[0].hp, g.gods[0].pos);
            g.machine_tick(1.0 / 60.0);
            let lost = (hp - g.gods[0].hp) / g.gods[0].max_hp;
            assert!((lost - HIT_SHARE).abs() < 0.01, "lost {lost}");
            let side = Vec3::new(-b.dir.z, 0.0, b.dir.x);
            let moved = (g.gods[0].pos - at).dot(side);
            assert!(moved > KNOCK * 0.8, "thrown {moved} aside");
            // again, in its way, same run: no second hit
            g.gods[0].pos = at;
            let hp2 = g.gods[0].hp;
            g.machine_tick(1.0 / 60.0);
            assert_eq!(g.gods[0].hp, hp2, "hit twice in one run");
            let (mut g, _) = scene_with_ball(BOULDER_R + 3.0);
            let hp = g.gods[0].hp;
            g.machine_tick(1.0 / 60.0);
            assert_eq!(g.gods[0].hp, hp, "hit from beside the track");
            // under the chute, the ball overhead: nothing
            let (mut g, _) = scene_with_ball(0.0);
            g.t = mz::FIRST_TURN + mz::TURN + ROLL_AT + 0.5;
            let b = boulders_at(g.wake.seed, g.t)[0];
            g.gods[0].pos = Vec3::new(b.pos.x, 0.0, b.pos.z);
            let hp = g.gods[0].hp;
            g.machine_tick(1.0 / 60.0);
            assert_eq!(g.gods[0].hp, hp, "hit by a ball up on its chute");
        });
    }

    /// The machine knows its own: a Sentinel (the same clan built them)
    /// takes the blow but is neither thrown nor slowed nor stopped casting.
    #[test]
    fn a_boulder_does_not_throw_a_sentinel() {
        within(|| {
            let id = crate::gods::ROSTER.iter().find(|d| d.role == crate::gods::Role::Sentinel).unwrap().id;
            let (mut g, _) = scene_with_ball_for(id, 0.5);
            let (hp, at) = (g.gods[0].hp, g.gods[0].pos);
            g.machine_tick(1.0 / 60.0);
            let lost = (hp - g.gods[0].hp) / g.gods[0].max_hp;
            assert!((lost - HIT_SHARE).abs() < 0.01, "lost {lost}");
            assert!((g.gods[0].pos - at).length() < 1e-3, "a Sentinel thrown to {}", g.gods[0].pos);
            assert!(g.gods[0].slow_t <= 0.0, "a Sentinel slowed");
        });
    }

    /// A bot in a boulder's way is sent off the track, clear of it and of
    /// its pads, on the floor; one beside it, or behind it, is not; and not
    /// until the ball is nearly on it.
    #[test]
    fn a_bot_steps_off_the_track() {
        within(|| {
            let (mut g, b) = scene_with_ball(0.0);
            let side = Vec3::new(-b.dir.z, 0.0, b.dir.x);
            let tr = &pairs()[b.pair][b.side];
            let mut dodged = 0;
            for ahead_by in [8.0f32, 12.0, 16.0, 20.0] {
                let ahead = b.pos + b.dir * ahead_by;
                g.gods[0].pos = Vec3::new(ahead.x, 0.0, ahead.z) + side * 0.5;
                if let Some(to) = dodge(&g, 0) {
                    assert!(tr.dist(to, 0.0) >= BOULDER_R + 3.0, "still on the track at {to}");
                    assert!(mz::floor().fits(to.x, to.z, 0.8));
                    assert!(pads(b.pair).iter().all(|d| !d.covers(to, 0.5)), "sent onto a pad at {to}");
                    dodged += 1;
                }
            }
            assert!(dodged >= 2, "only {dodged} dodges");
            let ahead = b.pos + b.dir * 12.0;
            g.gods[0].pos = Vec3::new(ahead.x, 0.0, ahead.z) + side * (BOULDER_R + 5.0);
            assert!(dodge(&g, 0).is_none(), "beside the track");
            let behind = b.pos - b.dir * 10.0;
            g.gods[0].pos = Vec3::new(behind.x, 0.0, behind.z);
            assert!(dodge(&g, 0).is_none(), "behind the ball");
            // on its line but far down it: not yet (REACT) -- and once the
            // ball is nearly there, yes (so the spot is one it CAN step
            // aside from: a passage with wall both sides says None either
            // way, and proves nothing)
            let mut tried = 0;
            for k in 0..12 {
                let far_s = b.s + SPEED * (REACT + 1.0) + k as f32 * 4.0;
                if far_s >= tr.len() {
                    break;
                }
                let far = tr.at(far_s);
                g.gods[0].pos = Vec3::new(far.x, 0.0, far.z);
                let early = dodge(&g, 0);
                let t0 = g.t;
                g.t += (far_s - b.s) / SPEED - 0.5;
                let due = dodge(&g, 0);
                g.t = t0;
                if due.is_some() {
                    assert!(early.is_none(), "stepped aside {:.1} s early", (far_s - b.s) / SPEED);
                    tried += 1;
                }
            }
            assert!(tried >= 2, "only {tried} far spots with room to dodge");
        });
    }

    /// Wherever along a track a bot is when the ball is due, it is never
    /// sent onto a pad -- the narrow places, where there is no room past
    /// the pads and the near step would land on one, included.
    #[test]
    fn a_dodge_never_lands_on_a_pad() {
        within(|| {
            let (mut g, _) = scene_with_ball(0.0);
            let seed = g.wake.seed;
            let (mut sent, mut near) = (0, 0);
            // a turn for each pair
            let turns: Vec<u32> = (0..pairs().len()).map(|q| (1..40).find(|&k| pair_for(seed, k) == Some(q)).unwrap()).collect();
            for (p, &k) in turns.iter().enumerate() {
            let t0 = mz::FIRST_TURN + (k - 1) as f32 * mz::TURN;
            for side in 0..2 {
                let tr = &pairs()[p][side];
                let mut s = tr.floor_from + 6.0;
                while s < tr.len() - 2.0 {
                    let at = tr.at(s);
                    for lat in [-1.5f32, 0.0, 1.5] {
                        let dir = tr.dir_at(s);
                        g.gods[0].pos = Vec3::new(at.x, 0.0, at.z) + Vec3::new(-dir.z, 0.0, dir.x) * lat;
                        g.t = t0 + ROLL_AT + (s - 8.0) / SPEED;
                        if let Some(to) = dodge(&g, 0) {
                            sent += 1;
                            assert!(pads(p).iter().all(|d| !d.covers(to, 0.5)), "sent onto a pad at {to} from s {s}");
                            near += (tr.dist(to, 0.0) < DODGE_OUT - 1.0) as usize;
                        }
                    }
                    s += 2.0;
                }
            }
            }
            assert!(sent > 150, "only {sent} dodges");
            eprintln!("{sent} dodges, {near} to the near step");
        });
    }

    /// Every pair has pads down both sides of its runs, mirrored, whole
    /// on room floor (never a passage), clear of the lanes and of the ball.
    #[test]
    fn the_locks_have_pads() {
        within(|| {
            let f = mz::floor();
            for p in 0..pairs().len() {
                let ds = pads(p);
                let n0 = ds.iter().filter(|d| d.side == 0).count();
                assert!((8..=60).contains(&n0), "pair {p}: {n0} pads a side");
                assert_eq!(ds.len(), n0 * 2);
                for d in ds {
                    assert!(ds.iter().any(|m| (m.at - mirror(d.at)).length() < 1e-3), "{d:?} has no mirror");
                    for (a, b) in [(-1.0f32, -1.0f32), (1.0, 1.0), (-1.0, 1.0), (1.0, -1.0)] {
                        let q = d.at + Vec3::new(a, 0.0, b) * PAD_HALF;
                        assert!(f.fits(q.x, q.z, 0.5), "{d:?} off the floor at {q}");
                        assert!(f.shape_at(q.x, q.z).and_then(mz::shape_passage).is_none(), "{d:?} in a passage");
                    }
                    for tr in &pairs()[p] {
                        assert!(tr.dist(d.at, 0.0) > PAD_HALF + BOULDER_R - 0.1, "{d:?} under the ball");
                    }
                    for lane in [crate::consts::Lane::Top, crate::consts::Lane::Bot] {
                        let l = mz::lane_polyline(lane);
                        let dl = l.windows(2).map(|w| crate::abilities::point_segment_dist(d.at, w[0], w[1])).fold(f32::MAX, f32::min);
                        assert!(dl > PAD_HALF + 3.0, "{d:?} in the {lane:?} lane");
                    }
                }
            }
            // no two pads overlap, of any pairs
            let all: Vec<Pad> = (0..pairs().len()).flat_map(|p| pads(p).iter().copied()).collect();
            for (i, a) in all.iter().enumerate() {
                for b in all.iter().skip(i + 1) {
                    assert!((a.at.x - b.at.x).abs() >= PAD_HALF * 2.0 - 1e-3 || (a.at.z - b.at.z).abs() >= PAD_HALF * 2.0 - 1e-3, "pads overlap at {} and {}", a.at, b.at);
                }
            }
        });
    }

    /// A pad is armed from the turn's start, drops as the ball passes it,
    /// kills a cat standing on it once it is open (not the moment it
    /// starts to swing, nor before, nor after it has closed), spares a cat
    /// beside it -- and a Sentinel, who braces across it.
    #[test]
    fn a_pad_drops_as_the_stone_passes_and_the_cat_on_it_falls() {
        within(|| {
            let mut g = Game::new_teams(&[0], &[1]);
            g.wake = mz::Wake::new(&mut Rng::new(3));
            let p = pair_for(g.wake.seed, 2).unwrap();
            let d = pads(p)[pads(p).len() / 4];
            let t0 = mz::FIRST_TURN + mz::TURN;
            let trig = t0 + ROLL_AT + d.s / SPEED;
            assert!(matches!(pad_phase(g.wake.seed, t0 + 1.0, p, &d), PadPhase::Armed(_)));
            assert!(matches!(pad_phase(g.wake.seed, trig + 0.05, p, &d), PadPhase::Falling(_)));
            assert!(!pad_phase(g.wake.seed, trig + 0.05, p, &d).deadly());
            assert!(pad_phase(g.wake.seed, trig + 1.0, p, &d).deadly());
            assert_eq!(pad_phase(g.wake.seed, trig + PAD_FALL + PAD_OPEN + PAD_RISE + 0.1, p, &d), PadPhase::Shut);
            assert_eq!(pad_phase(g.wake.seed, t0 + mz::TURN + trig - t0, p, &d), PadPhase::Shut, "a pad drops in a turn its pair does not run");
            g.gods[1].pos = Vec3::new(0.0, 0.0, 400.0);
            let fall = |g: &mut Game, at: Vec3, t: f32| {
                g.gods[0].pos = at;
                g.gods[0].hp = g.gods[0].max_hp;
                g.gods[0].respawn_t = 0.0;
                g.t = t;
                g.events.clear();
                g.machine_tick(1.0 / 60.0);
                g.gods[0].hp <= 0.0 || g.gods[0].respawn_t > 0.0
            };
            assert!(!fall(&mut g, d.at, trig - 0.5), "fell before the stone passed");
            assert!(fall(&mut g, d.at, trig + 1.0), "stood on an open pad");
            assert!(g.events.iter().any(|e| matches!(e, Event::CatFell { .. })));
            let mut g = Game::new_teams(&[0], &[1]);
            g.wake = mz::Wake::new(&mut Rng::new(3));
            g.gods[1].pos = Vec3::new(0.0, 0.0, 400.0);
            assert!(!fall(&mut g, d.at + d.out * (PAD_HALF + 1.0), trig + 1.0), "fell from beside the pad");
            assert!(!fall(&mut g, d.at, trig + PAD_FALL + PAD_OPEN + PAD_RISE + 0.2), "fell after it closed");
            let id = crate::gods::ROSTER.iter().find(|d| d.role == crate::gods::Role::Sentinel).unwrap().id;
            let mut g = Game::new_teams(&[id], &[1]);
            g.wake = mz::Wake::new(&mut Rng::new(3));
            g.gods[1].pos = Vec3::new(0.0, 0.0, 400.0);
            assert!(!fall(&mut g, d.at, trig + 1.0), "a Sentinel fell");
        });
    }

    /// A lock is a lock from its turn's start until its last pad has shut,
    /// and only its own rooms.
    #[test]
    fn the_locks_are_the_tracks_rooms_while_the_machine_works() {
        within(|| {
            let w = mz::Wake::new(&mut Rng::new(3));
            let p = pair_for(w.seed, 2).unwrap();
            let t0 = mz::FIRST_TURN + mz::TURN;
            let rm = pairs()[p][0].rooms[1];
            let at = mz::room_pos(rm);
            assert!(locked(w.seed, t0 + 0.5, at.x, at.z));
            assert!(locked(w.seed, t0 + lock_ends(p) - 0.2, at.x, at.z));
            assert!(!locked(w.seed, t0 + lock_ends(p) + 0.2, at.x, at.z));
            let other = mz::rooms().find(|r| pairs()[p].iter().all(|t| !t.rooms.contains(r))).unwrap();
            let o = mz::room_pos(other);
            assert!(!locked(w.seed, t0 + 5.0, o.x, o.z));
        });
    }
}
