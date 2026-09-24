//! steer — a bot's last look at the ground before it commits to a direction.
//!
//! The brains in `ai` decide WHERE to go and emit a straight-line direction
//! toward it. Until rough ground existed that was nearly fine: a blocker in
//! the way was resolved by push-out, and the bot slid round it (or stalled in
//! a concave corner — TODO.md). With `going`, a straight line can also run a
//! bot through a bramble bed that a two-metre sidestep avoids, which a human
//! would never do and which makes bots slow in exactly the jungle they are
//! supposed to contest.
//!
//! ## What this is and is not
//!
//! Local, not global: a fan of candidate headings around the wanted one,
//! each walked a few metres out and scored by progress per unit time. It is
//! not a path planner and cannot see a detour longer than its reach — a wide
//! thicket is crossed, a small one is skirted, which is the right behaviour
//! for a sidestep and the wrong tool for a maze. A planner over the going
//! field is the next step if bots need one; the scoring below is the cost it
//! would use.
//!
//! ## Visible, not a black box
//!
//! [`fan`] returns every candidate with its score so the choice can be
//! printed (`examples/simdbg`) and tested; [`steer`] is just its argmax.

use glam::Vec3;

use crate::blockers::Blockers;
use crate::going::Carriage;

/// Candidate headings, radians off the wanted direction. Zero first so that
/// on a tie the bot goes where it was told.
///
/// The fan goes all the way round. It stopped at +-90 deg at first, and
/// `examples/botwalk` found a bot pinned for three minutes in the inner
/// corner of a ruin wall's L: every forward heading was blocked, so steer
/// passed the doomed one through and push-out held it in the corner. A
/// heading behind has negative progress, so it never wins while anything
/// forward is walkable -- it is only the way OUT.
pub const FAN: [f32; 15] = [0.0, 0.35, -0.35, 0.7, -0.7, 1.1, -1.1, 1.57, -1.57, 2.0, -2.0, 2.5, -2.5, 3.0, -3.0];
/// How far out each heading is walked, world units.
pub const REACH: f32 = 4.0;
const PROBES: [f32; 3] = [1.2, 2.5, REACH];
/// Weight on continuing the previous heading, so a choice between two nearly
/// equal sides does not flip every tick.
const KEEP: f32 = 0.12;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub angle: f32,
    pub dir: Vec3,
    /// How far along this heading is walkable before a blocker, `0..=REACH`.
    pub reach: f32,
    /// Harmonic-mean going over that walkable stretch.
    pub going: f32,
    /// `None`: blocked at the first probe, not considered.
    pub score: Option<f32>,
}

fn rotate(v: Vec3, a: f32) -> Vec3 {
    let (s, c) = a.sin_cos();
    Vec3::new(v.x * c - v.z * s, 0.0, v.x * s + v.z * c)
}

/// Every candidate heading, scored. See the module header.
pub fn fan(world: &Blockers, pos: Vec3, want: Vec3, prev: Vec3, radius: f32, who: Carriage) -> Vec<Candidate> {
    let w = Vec3::new(want.x, 0.0, want.z).normalize_or_zero();
    let prev = Vec3::new(prev.x, 0.0, prev.z).normalize_or_zero();
    FAN.iter()
        .map(|&angle| {
            let dir = rotate(w, angle);
            let mut reach = 0.0;
            for &d in &PROBES {
                let p = pos + dir * d;
                if world.blocked(p.x, p.z, radius) {
                    break;
                }
                reach = d;
            }
            if reach <= 0.0 {
                return Candidate { angle, dir, reach, going: 0.0, score: None };
            }
            let going = world.going().along(pos, pos + dir * reach, who);
            // Progress toward the goal per unit time over the walkable
            // stretch, discounted for a stretch that ends at a wall. Behind
            // (cos < 0) the discount is inverted: backing out, the LONGER
            // clear stretch is the better way out, not the worse one.
            let c = angle.cos();
            let progress = if c >= 0.0 { c * going * (reach / REACH) } else { c * (1.0 - 0.5 * reach / REACH) };
            let score = progress + KEEP * dir.dot(prev) + if angle == 0.0 { 1e-3 } else { 0.0 };
            Candidate { angle, dir, reach, going, score: Some(score) }
        })
        .collect()
}

/// The heading to take, with the wanted vector's length kept (the brains use
/// a shorter vector to mean "ease in"). Returns `want` unchanged when it is
/// zero or when nothing in the fan is walkable — push-out still gets the last
/// word on collisions.
pub fn steer(world: &Blockers, pos: Vec3, want: Vec3, prev: Vec3, radius: f32, who: Carriage) -> Vec3 {
    let len = Vec3::new(want.x, 0.0, want.z).length();
    if len < 1e-4 || !pos.is_finite() || !want.is_finite() {
        return want;
    }
    let best = fan(world, pos, want, prev, radius, who)
        .into_iter()
        .filter_map(|c| c.score.map(|s| (s, c.dir)))
        .fold(None, |b: Option<(f32, Vec3)>, c| if b.map_or(true, |b| c.0 > b.0) { Some(c) } else { b });
    match best {
        Some((_, d)) => d * len,
        None => want,
    }
}

/// Per-bot steering memory: the detour a plan produced, while it lasts.
/// Kept on `Game` (`steer_mem`) so it is deterministic sim state that a
/// replay reproduces, and public so a debug view can draw the path.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Memory {
    /// Waypoints still to reach, nearest first.
    pub path: Vec<Vec3>,
    /// Sim time the path was planned; it is dropped `PATH_TTL` later.
    pub since: f32,
    /// How many plans this bot has needed, for tuning and tests.
    pub plans: u32,
    /// Floor-route waypoint (`floor::next_waypoint`), cached: routing every
    /// bot every tick doubled a 15v15's sim cost. Refreshed every
    /// `ROUTE_EVERY` s or when reached.
    pub route_wp: Option<glam::Vec3>,
    pub route_t: f32,
}

/// Seconds between floor-route refreshes for one bot.
pub const ROUTE_EVERY: f32 = 0.35;

/// A planned detour is followed at most this long before the direct fan is
/// trusted again -- the goal moves, and a stale path is its own trap.
pub const PATH_TTL: f32 = 4.0;
/// Planner grid pitch and reach, world units.
pub const PLAN_CELL: f32 = 1.0;
pub const PLAN_RADIUS: f32 = 16.0;
/// Plans the whole match may make in one sim tick. Measured (examples/
/// planbench, unoptimized crate code on x86): median 150 us, p95 630 us,
/// worst 3 ms per plan. Two a tick bounds a burst of stuck bots to a few ms
/// on a slow core instead of nine plans stacking on one frame; the rest
/// fall back to the fan for a tick and plan on the next.
pub const PLANS_PER_TICK: u32 = 2;

/// Is the direct fan good enough, or does this need a plan? Good enough
/// means a heading within ~70 deg of the goal that is walkable at least half
/// the probe reach.
fn fan_is_enough(f: &[Candidate]) -> bool {
    f.iter().any(|c| c.score.is_some() && c.angle.cos() > 0.34 && c.reach >= REACH * 0.6)
}

/// A* on a local grid round `pos` toward `pos + want * PLAN_RADIUS`: cells a
/// unit of `radius` does not fit are walls, a step costs its length over the
/// going there. The subgoal is the reachable cell nearest the ideal one, so
/// a goal behind a long wall still yields the best partial detour. Returns
/// waypoints (cell centres, thinned to direction changes), nearest first.
pub fn plan(world: &Blockers, pos: Vec3, want: Vec3, radius: f32, who: Carriage) -> Vec<Vec3> {
    let n = (PLAN_RADIUS * 2.0 / PLAN_CELL) as i32 + 1;
    let half = n / 2;
    let at = |ix: i32, iz: i32| Vec3::new(pos.x + (ix - half) as f32 * PLAN_CELL, 0.0, pos.z + (iz - half) as f32 * PLAN_CELL);
    let idx = |ix: i32, iz: i32| (iz * n + ix) as usize;
    let w = Vec3::new(want.x, 0.0, want.z).normalize_or_zero();
    let ideal = pos + w * PLAN_RADIUS * 0.95;
    // Lazily evaluated walls: most cells are never touched.
    let mut wall: Vec<Option<bool>> = vec![None; (n * n) as usize];
    let mut is_wall = |ix: i32, iz: i32, wall: &mut Vec<Option<bool>>| -> bool {
        let k = idx(ix, iz);
        if let Some(b) = wall[k] {
            return b;
        }
        let p = at(ix, iz);
        let b = world.blocked(p.x, p.z, radius);
        wall[k] = Some(b);
        b
    };
    let mut g = vec![f32::INFINITY; (n * n) as usize];
    let mut from = vec![u32::MAX; (n * n) as usize];
    let mut open = std::collections::BinaryHeap::new();
    #[derive(PartialEq)]
    struct Node(f32, i32, i32);
    impl Eq for Node {}
    impl PartialOrd for Node {
        fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(o))
        }
    }
    impl Ord for Node {
        fn cmp(&self, o: &Self) -> std::cmp::Ordering {
            o.0.partial_cmp(&self.0).unwrap_or(std::cmp::Ordering::Equal)
        }
    }
    let h = |p: Vec3| Vec3::new(p.x - ideal.x, 0.0, p.z - ideal.z).length();
    g[idx(half, half)] = 0.0;
    open.push(Node(h(pos), half, half));
    let (mut best, mut best_h) = ((half, half), h(pos));
    let mut expanded = 0;
    while let Some(Node(_, x, z)) = open.pop() {
        expanded += 1;
        if expanded > 2500 {
            break;
        }
        let here = at(x, z);
        let hh = h(here);
        if hh < best_h {
            best_h = hh;
            best = (x, z);
        }
        if hh < PLAN_CELL {
            break;
        }
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let (nx, nz) = (x + dx, z + dz);
            if nx < 0 || nz < 0 || nx >= n || nz >= n || is_wall(nx, nz, &mut wall) {
                continue;
            }
            // No corner cutting past a wall.
            if dx != 0 && dz != 0 && (is_wall(x + dx, z, &mut wall) || is_wall(x, z + dz, &mut wall)) {
                continue;
            }
            let np = at(nx, nz);
            let step = if dx != 0 && dz != 0 { std::f32::consts::SQRT_2 } else { 1.0 } * PLAN_CELL;
            let cost = g[idx(x, z)] + step / world.going_at(np.x, np.z, who);
            if cost < g[idx(nx, nz)] {
                g[idx(nx, nz)] = cost;
                from[idx(nx, nz)] = idx(x, z) as u32;
                open.push(Node(cost + h(np), nx, nz));
            }
        }
    }
    // Walk back from the best cell reached.
    let mut cells = Vec::new();
    let mut k = idx(best.0, best.1);
    while k != idx(half, half) && from[k] != u32::MAX {
        cells.push(at(k as i32 % n, k as i32 / n));
        k = from[k] as usize;
    }
    cells.reverse();
    // Thin to the points where the direction changes: fewer waypoints to
    // chase, and a path that reads as a route when drawn.
    let mut out: Vec<Vec3> = Vec::new();
    for (i, p) in cells.iter().enumerate() {
        let last = i + 1 == cells.len();
        let turn = i > 0 && !last && {
            let a = cells[i] - cells[i - 1];
            let b = cells[i + 1] - cells[i];
            (a - b).length() > 1e-3
        };
        if turn || last {
            out.push(*p);
        }
    }
    out
}

/// [`steer`], with a memory: when the direct fan cannot make progress (a
/// concave corner, a long wall across the way), plan a local detour and
/// follow it for up to [`PATH_TTL`], re-planning if it is exhausted early.
///
/// `may_plan` is the caller's budget: when false, a failing fan falls back
/// to the stateless [`steer`] this tick and the plan waits for a tick with
/// budget (see `PLANS_PER_TICK`).
pub fn steer_with(world: &Blockers, pos: Vec3, want: Vec3, prev: Vec3, radius: f32, who: Carriage, mem: &mut Memory, now: f32, may_plan: bool) -> Vec3 {
    let len = Vec3::new(want.x, 0.0, want.z).length();
    if len < 1e-4 || !pos.is_finite() || !want.is_finite() {
        mem.path.clear();
        return want;
    }
    // Following a detour: drop reached waypoints, expire a stale path.
    while mem.path.first().is_some_and(|p| Vec3::new(p.x - pos.x, 0.0, p.z - pos.z).length() < 0.8) {
        mem.path.remove(0);
    }
    if !(now - mem.since <= PATH_TTL) {
        mem.path.clear();
    }
    if mem.path.is_empty() {
        let f = fan(world, pos, want, prev, radius, who);
        if fan_is_enough(&f) || !may_plan {
            return steer(world, pos, want, prev, radius, who);
        }
        mem.path = plan(world, pos, want, radius, who);
        mem.since = now;
        mem.plans += 1;
        if mem.path.is_empty() {
            return steer(world, pos, want, prev, radius, who);
        }
    }
    let to = mem.path[0];
    let d = Vec3::new(to.x - pos.x, 0.0, to.z - pos.z).normalize_or_zero();
    // Still steered locally toward the waypoint, so a moving obstacle (a
    // cat, a log) is skirted on the way.
    steer(world, pos, d * len, prev, radius, who)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockers::{Blocker, Cover, Shape};
    use crate::going::{Ground, Rough};

    const RUN: Carriage = Carriage { stalking: false };
    const R: f32 = 0.55;

    fn world() -> Blockers {
        Blockers::new(60.0, 4.0)
    }
    fn angle_of(v: Vec3) -> f32 {
        v.z.atan2(v.x)
    }

    #[test]
    fn open_ground_goes_where_it_was_told() {
        let w = world();
        for want in [Vec3::X, Vec3::new(0.3, 0.0, -0.8), Vec3::new(-2.0, 0.0, 1.0)] {
            let got = steer(&w, Vec3::ZERO, want, Vec3::ZERO, R, RUN);
            assert!((got - want).length() < 1e-4, "{want} -> {got}");
        }
    }

    #[test]
    fn a_small_thicket_dead_ahead_is_skirted() {
        let mut w = world();
        w.push_rough(Rough { shape: Shape::Circle { x: 2.5, z: 0.0, r: 1.2 }, mul: 0.5, edge: 0.5, ground: Ground::Thicket });
        let got = steer(&w, Vec3::ZERO, Vec3::X, Vec3::ZERO, R, RUN);
        let a = angle_of(got).abs();
        assert!(a > 0.2 && a < 0.8, "went round, not away: {a} rad");
        assert!((got.length() - 1.0).abs() < 1e-4, "length kept");
    }

    /// A thicket wider than the fan can see round is crossed: turning 90°
    /// makes no progress at all, and progress is what is being bought.
    #[test]
    fn a_wide_thicket_is_crossed() {
        let mut w = world();
        w.push_rough(Rough { shape: Shape::Segment { x0: 3.0, z0: -30.0, x1: 3.0, z1: 30.0, r: 2.0 }, mul: 0.6, edge: 0.3, ground: Ground::Thicket });
        let got = steer(&w, Vec3::ZERO, Vec3::X, Vec3::ZERO, R, RUN);
        assert!(angle_of(got).abs() < 1e-3, "{}", angle_of(got));
    }

    #[test]
    fn a_wall_ahead_turns_the_bot_and_the_choice_holds() {
        let mut w = world();
        w.push(Blocker { shape: Shape::Circle { x: 2.0, z: 0.0, r: 1.0 }, top: 3.0, cover: Cover::NONE });
        let first = steer(&w, Vec3::ZERO, Vec3::X, Vec3::ZERO, R, RUN);
        assert!(angle_of(first).abs() > 0.3, "turned: {}", angle_of(first));
        // Next tick, with that heading as the previous one: same side. A
        // symmetric post is exactly where a flip-flopping bot would jitter.
        let second = steer(&w, Vec3::new(0.05, 0.0, 0.0), Vec3::X, first, R, RUN);
        assert_eq!(angle_of(first).signum(), angle_of(second).signum());
    }

    /// The case `botwalk` found: the inner corner of an L-shaped wall (the
    /// ruin wall's own spines), with the goal straight through the long arm.
    /// Driven the way the sim drives a unit -- steer, move, push out -- the
    /// bot must get round the wall, not sit in the corner.
    #[test]
    fn a_bot_backs_out_of_a_concave_corner_and_gets_round() {
        let mut w = world();
        // Long arm along x at z = 1.5, short arm down from its east end.
        w.push(Blocker { shape: Shape::Segment { x0: -10.0, z0: 1.5, x1: 0.0, z1: 1.5, r: 0.9 }, top: 3.0, cover: Cover::NONE });
        w.push(Blocker { shape: Shape::Segment { x0: 0.0, z0: -0.3, x1: 0.0, z1: -6.3, r: 0.9 }, top: 3.0, cover: Cover::NONE });
        let goal = Vec3::new(1.0, 0.0, 30.0);
        let mut pos = Vec3::new(-1.4, 0.0, -0.1); // in the corner
        let mut prev = Vec3::ZERO;
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        let mut mem = Memory::default();
        while t < 12.0 && pos.z < 3.0 {
            let want = (goal - pos).normalize_or_zero();
            let d = steer_with(&w, pos, want, prev, R, RUN, &mut mem, t, true);
            prev = d;
            pos += d.normalize_or_zero() * 6.0 * dt;
            pos = w.resolve(pos, R);
            t += dt;
        }
        assert!(pos.z >= 3.0, "still behind the wall after {t:.1} s, at {pos}");
        assert!(t < 8.0, "got round, but slowly: {t:.1} s");
        assert!(mem.plans >= 1 && mem.plans < 10, "planned, and did not thrash: {} plans", mem.plans);
    }

    /// No budget, no plan: the corner is left to the fan for that tick.
    #[test]
    fn without_budget_it_does_not_plan() {
        let mut w = world();
        w.push(Blocker { shape: Shape::Segment { x0: -10.0, z0: 1.5, x1: 0.0, z1: 1.5, r: 0.9 }, top: 3.0, cover: Cover::NONE });
        w.push(Blocker { shape: Shape::Segment { x0: 0.0, z0: -0.3, x1: 0.0, z1: -6.3, r: 0.9 }, top: 3.0, cover: Cover::NONE });
        let mut mem = Memory::default();
        let _ = steer_with(&w, Vec3::new(-1.4, 0.0, -0.1), Vec3::Z, Vec3::ZERO, R, RUN, &mut mem, 0.0, false);
        assert_eq!(mem.plans, 0);
        assert!(mem.path.is_empty());
        let _ = steer_with(&w, Vec3::new(-1.4, 0.0, -0.1), Vec3::Z, Vec3::ZERO, R, RUN, &mut mem, 0.016, true);
        assert_eq!(mem.plans, 1, "and plans once it has budget");
    }

    /// Open ground never plans: the planner is for when the fan fails, and
    /// a bot that planned every tick would cost 2500 cell expansions a tick.
    #[test]
    fn open_ground_never_plans() {
        let w = world();
        let mut mem = Memory::default();
        let mut pos = Vec3::ZERO;
        for k in 0..300 {
            let d = steer_with(&w, pos, Vec3::X, Vec3::X, R, RUN, &mut mem, k as f32 / 60.0, true);
            pos += d * 0.1;
        }
        assert_eq!(mem.plans, 0);
    }

    /// The plan goes round a wall across the way, not through it, and ends
    /// on the goal side.
    #[test]
    fn a_plan_goes_round_a_wall() {
        let mut w = world();
        w.push(Blocker { shape: Shape::Segment { x0: 3.0, z0: -6.0, x1: 3.0, z1: 6.0, r: 0.9 }, top: 3.0, cover: Cover::NONE });
        let p = plan(&w, Vec3::ZERO, Vec3::X, R, RUN);
        assert!(!p.is_empty());
        for q in &p {
            assert!(!w.blocked(q.x, q.z, R), "waypoint in the wall at {q}");
        }
        assert!(p.last().unwrap().x > 4.0, "ends past the wall: {:?}", p.last());
        assert!(p.iter().any(|q| q.z.abs() > 6.5), "went round an end: {p:?}");
    }

    #[test]
    fn boxed_in_leaves_the_request_alone() {
        let mut w = world();
        for k in 0..16 {
            let a = k as f32 / 16.0 * std::f32::consts::TAU;
            w.push(Blocker { shape: Shape::Circle { x: a.cos() * 1.1, z: a.sin() * 1.1, r: 0.5 }, top: 3.0, cover: Cover::NONE });
        }
        assert_eq!(steer(&w, Vec3::ZERO, Vec3::X, Vec3::ZERO, R, RUN), Vec3::X);
    }

    #[test]
    fn zero_and_garbage_pass_through() {
        let w = world();
        assert_eq!(steer(&w, Vec3::ZERO, Vec3::ZERO, Vec3::X, R, RUN), Vec3::ZERO);
        let nan = Vec3::new(f32::NAN, 0.0, 0.0);
        assert!(steer(&w, nan, Vec3::X, Vec3::ZERO, R, RUN) == Vec3::X);
    }

    #[test]
    fn the_fan_is_visible() {
        let mut w = world();
        w.push(Blocker { shape: Shape::Circle { x: 1.2, z: 0.0, r: 0.6 }, top: 3.0, cover: Cover::NONE });
        let f = fan(&w, Vec3::ZERO, Vec3::X, Vec3::ZERO, R, RUN);
        assert_eq!(f.len(), FAN.len());
        assert!(f[0].score.is_none(), "straight ahead is blocked at the first probe");
        assert!(f.iter().filter(|c| c.score.is_some()).count() >= 4);
    }
}
