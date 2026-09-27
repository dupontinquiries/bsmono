//! machine — the Thornmaze's clockwork: the boulders it rolls, and when.
//!
//! The labyrinth is a machine (an old clan of engineer-cats built it: gear
//! trains on the wall tops, Archimedes screws at the heads of its tracks).
//! Each turn of the labyrinth (`maze::Wake::turn`) the machine picks one
//! pair of TRACKS -- a track and its mirror -- holds their gates open for
//! the turn ([`held_open`], read by `maze::Wake::closed`), winds its screws
//! for [`ROLL_WARN`] seconds, and at [`ROLL_AT`] into the turn lets a stone
//! ball roll down each track at [`SPEED`]. At the far end it drops back
//! under the floor.
//!
//! A boulder's place is a function of the match clock and the match's seed
//! ([`boulders_at`]), like the gates: every client sees the same boulder in
//! the same place. What it hits is the sim's ([`Game::machine_tick`]): a cat
//! it rolls over takes [`HIT_SHARE`] of its health, is thrown aside
//! [`KNOCK`] units and slowed -- once a run. Bots step off a track a
//! boulder is winding up for or rolling down ([`dodge`]).
//!
//! Headless. `mazeworld` places the screws; `scene` draws the boulders, the
//! screws and the gear trains turning (faster while the machine works:
//! [`drive`]).

use glam::Vec3;
use std::sync::OnceLock;

use crate::maze::{self as mz, Room};
use crate::sim::{Event, Game};

/// The ball's radius.
pub const BOULDER_R: f32 = 2.2;
/// How fast it rolls, u/s.
pub const SPEED: f32 = 15.0;
/// When in a turn it is let go, and how long the screws wind before.
pub const ROLL_AT: f32 = 10.0;
pub const ROLL_WARN: f32 = 4.0;
/// What it does to a cat it rolls over: a share of its health, thrown aside
/// this far, and slowed.
pub const HIT_SHARE: f32 = 0.18;
pub const KNOCK: f32 = 4.5;
/// How soon (s) a ball must be due at a bot before it steps off the track.
pub const REACT: f32 = 1.5;
pub const HIT_SLOW: f32 = 0.4;

/// A track: a polyline on the floor, through rooms and passages.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub pts: Vec<Vec3>,
    /// The passages it runs through (held open while it runs).
    pub passages: Vec<usize>,
}

impl Track {
    pub fn len(&self) -> f32 {
        self.pts.windows(2).map(|w| (w[1] - w[0]).length()).sum()
    }
    /// The point `s` along it.
    pub fn at(&self, s: f32) -> Vec3 {
        crate::pass::along(&self.pts, s.clamp(0.0, self.len()))
    }
    /// Flat distance from `p` to the track (all of it, or from `s0` on).
    pub fn dist(&self, p: Vec3, s0: f32) -> f32 {
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

/// A straight run across a row, west to east, `dz` off its centre line,
/// through the two passages at that `dz`.
fn across(row: i32, dz: f32) -> Track {
    let z = row as f32 * mz::ROW_Z + dz;
    let pts = vec![Vec3::new(mz::COLS[0] - 22.0, 0.0, z), Vec3::new(mz::COLS[2] + 22.0, 0.0, z)];
    Track { pts, passages: vec![passage((0, row), (1, row), dz), passage((1, row), (2, row), dz)] }
}

/// The run up the middle column from the blue end into the Heart, `dx` off
/// its centre line.
fn up_the_middle(dx: f32) -> Track {
    let pts = vec![Vec3::new(dx, 0.0, -2.0 * mz::ROW_Z - 16.0), Vec3::new(dx, 0.0, -8.0)];
    Track { pts, passages: vec![passage((1, -2), (1, -1), dx), passage((1, -1), (1, 0), dx)] }
}

fn mirrored(t: &Track) -> Track {
    Track { pts: t.pts.iter().map(|&p| mirror(p)).collect(), passages: t.passages.iter().map(|&g| mz::mirror_passage(g)).collect() }
}

/// The track pairs: across the Heart's row, across the pits' rows, and up
/// the middle into the Heart from both ends. Each pair is a track and its
/// mirror; they run together.
pub fn pairs() -> &'static [[Track; 2]] {
    static P: OnceLock<Vec<[Track; 2]>> = OnceLock::new();
    P.get_or_init(|| {
        let heart = across(0, -9.0);
        let pits = across(-1, -9.0);
        let middle = up_the_middle(-14.0);
        [heart, pits, middle].into_iter().map(|t| {
            let m = mirrored(&t);
            [t, m]
        }).collect()
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

/// A boulder's doing at some moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ball {
    /// Which pair and which of its two tracks, and the run's id (unique per
    /// turn and track: a cat is hit once a run).
    pub pair: usize,
    pub side: usize,
    pub run: u32,
    pub pos: Vec3,
    /// Unit, flat: the way it is rolling.
    pub dir: Vec3,
    /// How far along the track (the scene turns it by this).
    pub s: f32,
    /// `0..1` while the screws wind it up out of the floor; 1 rolling;
    /// `1..0` as it drops away at the end.
    pub up: f32,
    pub rolling: bool,
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
        let dir = {
            let a = tr.at(s);
            let b = tr.at((s + 1.0).min(len));
            let c = if (b - a).length() > 1e-3 { b - a } else { a - tr.at((s - 1.0).max(0.0)) };
            Vec3::new(c.x, 0.0, c.z).normalize_or_zero()
        };
        let (up, rolling) = if run_t < -ROLL_WARN {
            continue;
        } else if run_t < 0.0 {
            ((run_t + ROLL_WARN) / ROLL_WARN, false)
        } else if s < len {
            (1.0, true)
        } else {
            let gone = (run_t - len / SPEED) / 1.0;
            if gone >= 1.0 {
                continue;
            }
            (1.0 - gone, false)
        };
        let pos = tr.at(s);
        out.push(Ball { pair: p, side, run: k * 2 + side as u32 + 1, pos: Vec3::new(pos.x, BOULDER_R * up - BOULDER_R * (1.0 - up), pos.z), dir, s, up, rolling });
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

impl Game {
    /// Once a tick, on the Thornmaze: the boulders roll over whatever cats
    /// are in their way. (Their place is the clock's; this is only what they
    /// hit.)
    pub fn machine_tick(&mut self, dt: f32) {
        if crate::map::active() != crate::map::MapId::Maze {
            return;
        }
        let t = self.t;
        let now = boulders_at(self.wake.seed, t);
        // a rumble as each is let go
        for b in &now {
            if b.rolling && b.s <= SPEED * dt + 1e-3 {
                self.events.push(Event::BoulderRoll { pos: b.pos });
            }
        }
        for b in now.iter().filter(|b| b.rolling) {
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
    }
}

/// Where a bot should step to get off a track a boulder is winding up for
/// or rolling down toward it (None: it is not in the way). Off to the side
/// it is already on, clear of the ball, on the floor.
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
        // the nearest point on the track (to tell which side, and how far
        // along it the cat stands)
        let (mut best, mut s_me) = ((f32::MAX, tr.pts[0]), 0.0);
        let n = tr.len() as usize;
        for k in 0..=n {
            let p = tr.at(k as f32);
            let d = Vec3::new(p.x - me.x, 0.0, p.z - me.z).length();
            if d < best.0 {
                best = (d, p);
                s_me = k as f32;
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
        let from_line = best.1;
        let side = Vec3::new(-b.dir.z, 0.0, b.dir.x);
        let sgn = if Vec3::new(me.x - from_line.x, 0.0, me.z - from_line.z).dot(side) >= 0.0 { 1.0 } else { -1.0 };
        let f = mz::floor();
        for s in [sgn, -sgn] {
            let to = from_line + side * s * (BOULDER_R + 4.0);
            if f.fits(to.x, to.z, 0.8) {
                return Some(Vec3::new(to.x, 0.0, to.z));
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

    /// Every track lies on the floor with the ball's room either side, runs
    /// through the passages it names (and they are gates, never a lane's),
    /// and a track's mirror is its pair.
    #[test]
    fn the_tracks_lie_on_the_floor_through_their_passages() {
        within(|| {
            let f = mz::floor();
            assert_eq!(pairs().len(), 3);
            for pair in pairs() {
                for tr in pair {
                    assert!(tr.len() > 100.0, "a short track: {}", tr.len());
                    let n = tr.len() as usize;
                    for k in 0..=n {
                        let p = tr.at(k as f32);
                        assert!(f.fits(p.x, p.z, BOULDER_R + 0.2), "the ball does not fit at {p}");
                    }
                    for &g in &tr.passages {
                        let ps = mz::passages()[g];
                        assert!(ps.lane.is_none(), "a track through a lane's passage");
                        assert!(tr.dist(ps.mid(), 0.0) < 0.5, "the track misses passage {g}");
                    }
                }
                let m = mirrored(&pair[0]);
                assert_eq!(m.pts, pair[1].pts);
            }
        });
    }

    /// The machine runs one pair a turn, never the same twice running,
    /// every pair in time; it holds that pair's gates open for the turn
    /// (`Wake::closed` says so too); nothing runs before the first turn.
    #[test]
    fn one_pair_a_turn_with_its_gates_held_open() {
        let w = mz::Wake::new(&mut Rng::new(3));
        assert_eq!(pair_for(w.seed, 0), None);
        let mut seen = std::collections::HashSet::new();
        for k in 1..60 {
            let p = pair_for(w.seed, k).unwrap();
            seen.insert(p);
            if k > 1 {
                assert_ne!(Some(p), pair_for(w.seed, k - 1), "turn {k} repeats the pair");
            }
            let shut = w.closed(k);
            for g in held_open(w.seed, k) {
                assert!(!shut[g], "turn {k}: the machine's gate {g} is shut");
            }
        }
        assert_eq!(seen.len(), pairs().len(), "a pair never ran");
    }

    /// A boulder winds up out of the floor, rolls the whole track at SPEED
    /// from ROLL_AT, drops away at the end, and is gone before the next turn.
    #[test]
    fn a_boulder_rolls_its_track_on_time() {
        let w = mz::Wake::new(&mut Rng::new(3));
        let t0 = mz::FIRST_TURN + mz::TURN; // turn 2
        let p = pair_for(w.seed, 2).unwrap();
        let len = pairs()[p][0].len();
        assert!(boulders_at(w.seed, t0 + ROLL_AT - ROLL_WARN - 0.5).is_empty(), "out before its warning");
        let winding = boulders_at(w.seed, t0 + ROLL_AT - 1.0);
        assert_eq!(winding.len(), 2);
        assert!(winding.iter().all(|b| !b.rolling && b.up > 0.5 && b.up < 1.0));
        let mid = boulders_at(w.seed, t0 + ROLL_AT + 3.0);
        assert!(mid.iter().all(|b| b.rolling && (b.s - 3.0 * SPEED).abs() < 1e-3));
        // the two roll mirrored
        assert!((mid[0].pos - mirror(mid[1].pos)).length() < 1e-3);
        let after = t0 + ROLL_AT + len / SPEED + 1.2;
        assert!(after < t0 + mz::TURN - mz::GATE_WARN, "a run overlaps the next turn's gates");
        assert!(boulders_at(w.seed, after).is_empty(), "still there after its run");
    }

    fn scene_with_ball(offset: f32) -> (Game, Ball) {
        scene_with_ball_for(0, offset)
    }

    fn scene_with_ball_for(god: u32, offset: f32) -> (Game, Ball) {
        let mut g = Game::new_teams(&[god], &[1]);
        g.wake = mz::Wake::new(&mut Rng::new(3));
        let t = mz::FIRST_TURN + mz::TURN + ROLL_AT + 2.0;
        g.t = t;
        let b = boulders_at(g.wake.seed, t)[0];
        let side = Vec3::new(-b.dir.z, 0.0, b.dir.x);
        g.gods[0].pos = Vec3::new(b.pos.x, 0.0, b.pos.z) + side * offset;
        g.gods[1].pos = Vec3::new(0.0, 0.0, 400.0);
        (g, b)
    }

    /// A cat the boulder rolls over takes HIT_SHARE of its health and is
    /// thrown aside KNOCK units, once a run; a cat beside the track is
    /// untouched.
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

    /// A bot in a boulder's way is sent off the track, to the side, clear
    /// of the ball and on the floor; one beside it, or behind it, is not.
    #[test]
    fn a_bot_steps_off_the_track() {
        within(|| {
            let (mut g, b) = scene_with_ball(0.0);
            let side = Vec3::new(-b.dir.z, 0.0, b.dir.x);
            // ahead of the ball, on its line
            let ahead = b.pos + b.dir * 12.0;
            g.gods[0].pos = Vec3::new(ahead.x, 0.0, ahead.z) + side * 0.5;
            let to = dodge(&g, 0).expect("a dodge");
            let tr = &pairs()[b.pair][b.side];
            assert!(tr.dist(to, 0.0) >= BOULDER_R + 3.0, "still on the track at {to}");
            assert!(mz::floor().fits(to.x, to.z, 0.8));
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
}
