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
pub const FAN: [f32; 9] = [0.0, 0.35, -0.35, 0.7, -0.7, 1.1, -1.1, 1.57, -1.57];
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
            // stretch, discounted for a stretch that ends at a wall.
            let progress = angle.cos() * going * (reach / REACH);
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
