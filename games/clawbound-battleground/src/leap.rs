//! leap — a cat's jump is aimed at a PLACE.
//!
//! `docs/WARRIORS_MECHANICS.md`: *"Leap is not a jump. A cat's leap is aimed
//! at a place — a branch, a rock — and the interesting part is the gather
//! before it and the balance check after. Wants a target, not just a phase."*
//!
//! So [`plan`] looks along the aim for something to land ON — a rock, a
//! boulder cluster, a fallen log, a pedestal — and only falls back to open
//! ground when there is nothing. A cat that lands on a perch stands on it
//! (`GodUnit::perch`), and a leap begun FROM a perch, or during the landing
//! of the last one, skips most of the gather and all of the cooldown. That
//! chain is the "bounding from rock to rock" and it is deliberately the only
//! way to leap back-to-back: across open ground a leap is a committed move
//! with a cooldown, along an outcrop it is how you travel.
//!
//! ## What is sim and what is not
//!
//! Phases, positions and the perch are simulation: they decide where a cat
//! is and whether a boulder stops it. The wiggle, the stretch and the wobble
//! are `puppet`'s, read through [`LeapState::view`].

//! ## Scaled to a cat, not to a player capsule
//!
//! The first version of this capped a landing at 2.4 u and called anything
//! taller a wall. An ancestor stands 1.1 u at the shoulder. A house cat
//! clears five to six times its own height from a standstill, runs fence
//! tops, and turns over in the air to land on its feet — so the cap was a
//! person's jump in a cat's body. Now:
//!
//! * [`LEAP_UP`] is 5.5 u — five shoulder heights — and is measured from
//!   where the cat is STANDING, so a rock is a step toward a colossus head.
//! * What can be landed on is [`Perch`], a property of the prop. A ruin wall
//!   is a destination. A pine is not, at any height.
//! * A [`Perch::Narrow`] top is walked at nearly full speed. What it costs is
//!   footing: a solid hit knocks a cat off it ([`KNOCK_OFF_FRAC`]).
//! * Any fall is survived. A cat that leaves a perch without meaning to
//!   starts the fall on its side or its back ([`Fall::tumble`]) and is
//!   upright again inside [`RIGHTING_SECS`], front half first.

use glam::Vec3;

use crate::blockers::{Blockers, Perch};
use crate::puppet::LeapView;

/// Shortest and longest ground distance a leap covers. About four body
/// lengths: a cat's long jump, not its record.
pub const LEAP_MIN: f32 = 1.5;
pub const LEAP_RANGE: f32 = 9.0;
/// How far UP a leap reaches, from the cat's own feet.
pub const LEAP_UP: f32 = 5.5;
/// Things that cannot be stood on are sailed over if they top out below
/// this above the feet; above it they are walls.
pub const CLEAR_OVER: f32 = 2.2;
/// A solid hit of this fraction of max HP takes a cat's footing on a
/// narrow perch.
pub const KNOCK_OFF_FRAC: f32 = 0.10;
/// Too small to put four paws on.
pub const PERCH_MIN_RADIUS: f32 = 0.5;
pub const GRAVITY: f32 = 30.0;
/// The air-righting reflex: fully upright this long into any fall. A real
/// cat needs about a third of a second, which is roughly half a metre.
pub const RIGHTING_SECS: f32 = 0.30;
/// How far off the aim line a perch can be and still be "what I meant".
pub const AIM_SLACK: f32 = 1.3;
pub const GATHER_SECS: f32 = 0.18;
/// From a perch, or buffered during a landing: already coiled.
pub const CHAIN_GATHER_SECS: f32 = 0.06;
pub const LAND_SECS: f32 = 0.20;
pub const LEAP_SPEED: f32 = 16.0;
pub const LEAP_COOLDOWN: f32 = 1.6;
/// Rise above the higher of the two ends, plus a share of the distance.
const APEX_BASE: f32 = 0.7;
const APEX_PER_UNIT: f32 = 0.11;
/// Speed along a narrow top, as a fraction of normal. Barely a cost — they
/// are good at this — but a wall is not somewhere to sprint.
pub const NARROW_SPEED: f32 = 0.85;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Gather,
    Flight,
    Land,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeapState {
    pub phase: Phase,
    /// Seconds into the current phase.
    pub t: f32,
    pub from: Vec3,
    pub to: Vec3,
    pub gather: f32,
    pub flight: f32,
    pub apex: f32,
    /// Landing on something: its top, which is also `to.y`.
    pub perch: Option<f32>,
    pub narrow: bool,
    /// How hard the landing is, `0..=1`, for the body to absorb.
    pub impact: f32,
}

/// Where a leap would go.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    pub to: Vec3,
    pub perch: Option<f32>,
    pub narrow: bool,
}

/// Airborne without a leap: walked off an edge, dragged off one, or knocked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fall {
    pub vy: f32,
    pub t: f32,
    /// Roll at the moment of leaving, radians. 0 for a deliberate hop down;
    /// up to a full half-turn for a cat knocked off a wall on its back.
    pub tumble: f32,
    pub from_y: f32,
}

impl Fall {
    pub fn new(from_y: f32, tumble: f32) -> Fall {
        Fall { vy: 0.0, t: 0.0, tumble, from_y }
    }

    /// Roll remaining at time `t`. Smoothstep to zero over RIGHTING_SECS,
    /// whatever it started at: the reflex does not take longer for a worse
    /// start, the cat just turns faster.
    pub fn roll(&self) -> f32 {
        let k = (self.t / RIGHTING_SECS).clamp(0.0, 1.0);
        self.tumble * (1.0 - k * k * (3.0 - 2.0 * k))
    }

    /// How far the front half leads the hind half, radians. The reflex is
    /// two turns, not one: head and forelegs come round first and the
    /// hindquarters follow. Peaks mid-righting, zero at both ends.
    pub fn twist(&self) -> f32 {
        let k = (self.t / RIGHTING_SECS).clamp(0.0, 1.0);
        self.tumble * 0.45 * (std::f32::consts::PI * k).sin()
    }

    pub fn tick(&mut self, y: f32, dt: f32) -> f32 {
        self.t += dt;
        self.vy -= GRAVITY * dt;
        y + self.vy * dt
    }

    pub fn view(&self) -> LeapView {
        LeapView::Fall { roll: self.roll(), twist: self.twist(), speed: -self.vy }
    }
}

/// Choose a landing for a leap from `pos` along `dir` (xz; need not be unit).
///
/// The aim is marched outward. The FIRST standable, reachable thing near the
/// line wins — "I pressed leap looking at the wall" means the wall, and the
/// rock behind it is the next leap. The landing is the point of its spine
/// nearest the line, so a wall is landed on where it was aimed at, not at
/// its midpoint. Anything unstandable and too tall to sail over ends the
/// march: no landing beyond it, no perch behind it.
pub fn plan(pos: Vec3, dir: Vec3, standing_on: Option<f32>, blockers: &Blockers) -> Option<Plan> {
    let d = Vec3::new(dir.x, 0.0, dir.z);
    let len = d.length();
    if !(len > 1e-4) || !pos.is_finite() {
        return None;
    }
    let d = d / len;
    let body = crate::consts::GOD_RADIUS;
    let feet = pos.y.max(0.0);
    let underfoot = |b: &crate::blockers::Blocker| {
        let (ax, az, r) = b.shape.nearest_axis(pos.x, pos.z);
        standing_on.is_some() && (b.top - feet).abs() < 0.1 && (pos.x - ax).hypot(pos.z - az) < r + 0.2
    };

    let mut wall = LEAP_RANGE;
    let mut s = 0.25;
    'march: while s <= LEAP_RANGE {
        let (x, z) = (pos.x + d.x * s, pos.z + d.z * s);
        for (b, perch) in blockers.with_perch() {
            if !b.top.is_finite() || underfoot(b) {
                continue;
            }
            let (ax, az, r) = b.shape.nearest_axis(x, z);
            let off = (x - ax).hypot(z - az);
            let rise = b.top - feet;
            let landable = perch != Perch::None && rise <= LEAP_UP && r >= PERCH_MIN_RADIUS;
            if landable {
                if s >= LEAP_MIN && off < r * 0.5 + AIM_SLACK * 0.5 {
                    return Some(Plan { to: Vec3::new(ax, b.top, az), perch: Some(b.top), narrow: perch == Perch::Narrow });
                }
            } else if rise > CLEAR_OVER && off < r + body {
                wall = s - 0.5;
                break 'march;
            }
        }
        s += 0.25;
    }

    // Open ground: as far as the wall allows, pulled in until the landing is clear.
    let mut reach = wall;
    while reach >= LEAP_MIN {
        let (x, z) = (pos.x + d.x * reach, pos.z + d.z * reach);
        if !blockers.blocked(x, z, body) {
            // open ground lands on the ground there -- the pass's shelf is
            // not at 0 (`map::ground_y`; 0 on the jungle, unchanged)
            return Some(Plan { to: Vec3::new(x, crate::map::ground_y(x, z), z), perch: None, narrow: false });
        }
        reach -= 0.5;
    }
    None
}

/// As [`plan`], with moving platforms too (the sky map's clouds, at where
/// each WILL be when a leap from here lands -- the caller predicts that):
/// marching out along the aim, a cloud's deck met before anything else
/// that can be landed on is the landing, at the point of the aim nearest
/// the deck's middle (not its near rim). Returns the plan and which
/// platform it lands on.
pub fn plan_onto(pos: Vec3, dir: Vec3, standing_on: Option<f32>, blockers: &Blockers, platforms: &[crate::ferry::Platform]) -> Option<(Plan, Option<usize>)> {
    let fixed = plan(pos, dir, standing_on, blockers);
    let d = Vec3::new(dir.x, 0.0, dir.z);
    if platforms.is_empty() || !(d.length() > 1e-4) || !pos.is_finite() {
        return fixed.map(|p| (p, None));
    }
    let d = d.normalize();
    // how far along the aim the fixed plan lands (a perch or open ground)
    let fixed_s = fixed.map_or(f32::MAX, |p| Vec3::new(p.to.x - pos.x, 0.0, p.to.z - pos.z).length());
    let mut best: Option<(f32, f32, usize)> = None; // (off-centre, s, platform)
    let mut s = LEAP_MIN;
    while s <= LEAP_RANGE {
        let (x, z) = (pos.x + d.x * s, pos.z + d.z * s);
        for (k, p) in platforms.iter().enumerate() {
            let off = (x - p.at.x).hypot(z - p.at.z);
            if off <= p.r - 1.2 && best.map_or(true, |b| off < b.0) {
                best = Some((off, s, k));
            }
        }
        s += 0.25;
    }
    match best {
        // a cloud on the aim beats open ground (from a pier, open ground
        // is the pier's own edge: the leap would stop short of the cloud);
        // only a perch nearer along the aim beats the cloud
        Some((_, bs, k)) if fixed.map_or(true, |p| p.perch.is_none() || bs <= fixed_s + 0.5) => {
            let (x, z) = (pos.x + d.x * bs, pos.z + d.z * bs);
            Some((Plan { to: Vec3::new(x, platforms[k].at.y, z), perch: None, narrow: false }, Some(k)))
        }
        _ => fixed.map(|p| (p, None)),
    }
}

/// Seconds from pressing leap to landing, for a leap of `run` (ground
/// distance) that is `chained` or not: what a platform's position must be
/// predicted for (mirrors [`LeapState::begin`]).
pub fn time_to_land(run: f32, chained: bool) -> f32 {
    let gather = if chained { CHAIN_GATHER_SECS } else { GATHER_SECS };
    gather + (run / LEAP_SPEED).clamp(0.28, 0.75)
}

impl LeapState {
    pub fn begin(from: Vec3, plan: Plan, chained: bool) -> LeapState {
        let run = Vec3::new(plan.to.x - from.x, 0.0, plan.to.z - from.z).length();
        LeapState {
            phase: Phase::Gather,
            t: 0.0,
            from,
            to: plan.to,
            gather: if chained { CHAIN_GATHER_SECS } else { GATHER_SECS },
            // A steep leap is slower than its ground distance suggests.
            flight: ((run + (plan.to.y - from.y).abs() * 0.6) / LEAP_SPEED).clamp(0.28, 0.75),
            apex: APEX_BASE + APEX_PER_UNIT * run,
            perch: plan.perch,
            narrow: plan.narrow,
            impact: 0.6,
        }
    }

    /// A landing with no leap before it: the end of a fall. `fell` is the
    /// height dropped; it sets how deep the crouch is, and nothing else —
    /// a cat is not hurt by any drop this map has.
    pub fn landing(at: Vec3, perch: Option<(f32, Perch)>, fell: f32) -> LeapState {
        LeapState {
            phase: Phase::Land,
            t: 0.0,
            from: at,
            to: at,
            gather: GATHER_SECS,
            flight: 0.3,
            apex: 0.0,
            perch: perch.map(|p| p.0),
            narrow: matches!(perch, Some((_, Perch::Narrow))),
            impact: (0.35 + fell / 8.0).clamp(0.35, 1.0),
        }
    }

    /// Position along the arc at flight fraction `a`.
    pub fn arc(&self, a: f32) -> Vec3 {
        let a = a.clamp(0.0, 1.0);
        let mut p = self.from.lerp(self.to, a);
        p.y += self.apex * 4.0 * a * (1.0 - a);
        p
    }

    /// Vertical over horizontal speed at flight fraction `a`.
    pub fn climb(&self, a: f32) -> f32 {
        let run = Vec3::new(self.to.x - self.from.x, 0.0, self.to.z - self.from.z).length().max(0.5);
        ((self.to.y - self.from.y) + self.apex * 4.0 * (1.0 - 2.0 * a.clamp(0.0, 1.0))) / run
    }

    /// Advance. Returns the position the cat should be at, and `true` on the
    /// tick the leap finishes.
    pub fn tick(&mut self, dt: f32) -> (Vec3, bool) {
        self.t += dt.max(0.0);
        match self.phase {
            Phase::Gather => {
                if self.t >= self.gather {
                    self.phase = Phase::Flight;
                    self.t = 0.0;
                }
                (self.from, false)
            }
            Phase::Flight => {
                let a = self.t / self.flight;
                if a >= 1.0 {
                    self.phase = Phase::Land;
                    self.t = 0.0;
                    return (self.to, false);
                }
                (self.arc(a), false)
            }
            Phase::Land => (self.to, self.t >= LAND_SECS),
        }
    }

    pub fn view(&self) -> LeapView {
        match self.phase {
            Phase::Gather => LeapView::Gather { t: (self.t / self.gather).clamp(0.0, 1.0) },
            Phase::Flight => {
                let a = (self.t / self.flight).clamp(0.0, 1.0);
                LeapView::Flight { t: a, climb: self.climb(a) }
            }
            Phase::Land => LeapView::Land { t: (self.t / LAND_SECS).clamp(0.0, 1.0), perched: self.narrow, impact: self.impact },
        }
    }

    /// Feet height for collision: airborne cats clear what is below them.
    pub fn feet_y(&self, pos_y: f32) -> f32 {
        if self.phase == Phase::Flight { pos_y } else { f32::NEG_INFINITY }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockers::PropKind;

    fn world() -> Blockers {
        let mut b = Blockers::new(100.0, 6.0);
        b.push_prop(PropKind::Rock, 5.0, 0.0, 0.0, 1.0); // Flat, 1.4
        b.push_prop(PropKind::ColossusHead, 12.0, 0.0, 0.0, 1.0); // Flat, 6.0: out of reach from the floor
        b.push_prop(PropKind::RuinPillar, 0.0, 5.0, 0.0, 1.0); // Narrow, 4.0
        b.push_prop(PropKind::Pine, -5.0, 0.0, 0.0, 1.0); // blocks to 3.0, stands nobody
        b.push_prop(PropKind::Fern, 0.0, -5.0, 0.0, 1.0); // cover only
        // a wall running along x at z = -12, seen side-on from the origin
        b.push_segment(PropKind::RuinWall, -8.0, -12.0, 8.0, -12.0, 0.0, 1.0);
        b
    }

    #[test]
    fn a_leap_toward_a_rock_lands_on_the_rock() {
        let p = plan(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.1), None, &world()).unwrap();
        assert_eq!((p.perch, p.narrow), (Some(1.4), false));
        assert!((p.to - Vec3::new(5.0, 1.4, 0.0)).length() < 1e-3, "aimed near it, landed ON it: {}", p.to);
    }

    /// The correction this module was rewritten for. A 4 u pillar is a
    /// destination for an animal that jumps five times its height.
    #[test]
    fn a_cat_leaps_to_the_top_of_a_pillar_four_times_its_height() {
        let p = plan(Vec3::ZERO, Vec3::Z, None, &world()).unwrap();
        assert_eq!((p.perch, p.narrow), (Some(4.0), true), "the pillar cap, and it is a narrow one");
        let l = LeapState::begin(Vec3::ZERO, p, false);
        let peak = (0..=30).map(|i| l.arc(i as f32 / 30.0).y).fold(0.0f32, f32::max);
        assert!(peak > 4.0 && peak < 6.5, "rises past the cap and comes down onto it: {peak}");
        assert!(l.climb(0.0) > 0.9, "and it goes UP: climb {}", l.climb(0.0));
    }

    /// Standable is not "short". A pine blocks to 3 u, under the reach, and
    /// must still stop a leap rather than catch it; a fern must do neither.
    #[test]
    fn a_tree_is_a_wall_and_a_fern_is_nothing() {
        let at_pine = plan(Vec3::ZERO, -Vec3::X, None, &world()).unwrap();
        assert_eq!(at_pine.perch, None, "nobody lands on a pine");
        assert!(at_pine.to.x > -5.0 + 0.75, "and the landing stops short of the trunk: {}", at_pine.to);
        let at_fern = plan(Vec3::ZERO, Vec3::new(0.6, 0.0, -1.0), None, &world()).unwrap();
        assert_eq!(at_fern.perch, None);
    }

    /// A wall is landed on WHERE IT WAS AIMED AT. The first planner took the
    /// spine point nearest the ray's midpoint, which for a long wall seen
    /// at an angle is somewhere else entirely.
    #[test]
    fn a_wall_is_landed_on_where_the_aim_crosses_it() {
        let from = Vec3::new(3.0, 0.0, -5.0);
        let p = plan(from, Vec3::new(0.4, 0.0, -1.0), None, &world()).unwrap();
        assert_eq!((p.perch, p.narrow), (Some(3.0), true));
        assert!((p.to.z + 12.0).abs() < 1e-3, "on the ridge: {}", p.to);
        // The ray x = 3 + 0.4 t, z = -5 - t meets z = -12 at x = 5.8; the
        // march stops a little early, where the ridge first comes in reach.
        assert!((5.0..=5.9).contains(&p.to.x), "where the aim meets the wall, not its middle (0) or its end (8): x = {}", p.to.x);
    }

    /// Reach is from the feet. Both directions: out of reach from the
    /// floor, in reach from the rock.
    #[test]
    fn a_rock_is_a_step_toward_something_higher() {
        let w = world();
        let from_floor = plan(Vec3::new(6.0, 0.0, 0.0), Vec3::X, None, &w);
        assert!(from_floor.map_or(true, |p| p.perch.is_none()), "6.0 u is past 5.5 from the ground: {from_floor:?}");
        let from_rock = plan(Vec3::new(5.0, 1.4, 0.0), Vec3::X, Some(1.4), &w).unwrap();
        assert_eq!(from_rock.perch, Some(6.0), "4.6 u up from the rock: reachable, and not the rock underfoot");
    }

    #[test]
    fn the_arc_starts_and_ends_where_it_says() {
        let plan = Plan { to: Vec3::new(6.0, 1.4, 0.0), perch: Some(1.4), narrow: false };
        let l = LeapState::begin(Vec3::ZERO, plan, false);
        assert!((l.arc(0.0) - l.from).length() < 1e-5 && (l.arc(1.0) - l.to).length() < 1e-5);
        assert!(l.climb(0.0) > 0.3 && l.climb(1.0) < -0.2, "up then down");
    }

    #[test]
    fn phases_run_in_order_and_a_chain_gathers_faster() {
        let plan = Plan { to: Vec3::new(6.0, 0.0, 0.0), perch: None, narrow: false };
        let mut seen = Vec::new();
        let mut l = LeapState::begin(Vec3::ZERO, plan, false);
        let mut secs = 0.0;
        loop {
            if seen.last() != Some(&l.phase) {
                seen.push(l.phase);
            }
            let (_, done) = l.tick(1.0 / 60.0);
            secs += 1.0 / 60.0;
            if done {
                break;
            }
            assert!(secs < 3.0, "a leap must end");
        }
        assert_eq!(seen, vec![Phase::Gather, Phase::Flight, Phase::Land]);
        assert!((0.7..1.0).contains(&secs), "gather + 6 u flight + land: {secs}");
        assert!(LeapState::begin(Vec3::ZERO, plan, true).gather < l.gather * 0.5);
    }

    /// The righting reflex. Upright inside RIGHTING_SECS from ANY start,
    /// never overshooting, front half leading, and done long before the
    /// ground arrives from a wall.
    #[test]
    fn a_falling_cat_is_upright_before_it_lands() {
        for tumble in [0.0, 1.3, -2.4, std::f32::consts::PI] {
            let mut f = Fall::new(3.0, tumble);
            let (mut y, mut upright_at, mut peak_twist) = (3.0f32, None, 0.0f32);
            let mut last = tumble.abs();
            while y > 0.0 {
                y = f.tick(y, 1.0 / 120.0);
                assert!(f.roll().abs() <= last + 1e-5, "righting never rolls the wrong way");
                assert!(f.roll() * tumble >= -1e-5, "and never overshoots past upright");
                last = f.roll().abs();
                peak_twist = peak_twist.max(f.twist().abs());
                if upright_at.is_none() && f.roll().abs() < 0.02 {
                    upright_at = Some(f.t);
                }
                assert!(f.t < 2.0);
            }
            let landed = f.t;
            let up = upright_at.expect("it never got upright");
            assert!(up <= RIGHTING_SECS + 0.01, "tumble {tumble}: upright at {up}s");
            assert!(up < landed, "tumble {tumble}: upright at {up}s but landed at {landed}s -- on its back");
            assert!((0.40..0.50).contains(&landed), "3 u at g = 30 is sqrt(2h/g) = 0.447 s, got {landed}");
            if tumble != 0.0 {
                assert!(peak_twist > tumble.abs() * 0.3, "front half leads the hind: peak twist {peak_twist}");
                assert!(f.twist().abs() < 0.02, "and the two halves agree again by the ground");
            } else {
                assert_eq!(peak_twist, 0.0, "a deliberate hop down does not twist");
            }
        }
    }

    #[test]
    fn nonsense_aims_plan_nothing() {
        assert_eq!(plan(Vec3::ZERO, Vec3::ZERO, None, &world()), None);
        assert_eq!(plan(Vec3::ZERO, Vec3::Y, None, &world()), None, "straight up has no xz direction");
        assert_eq!(plan(Vec3::new(f32::NAN, 0.0, 0.0), Vec3::X, None, &world()), None);
    }
}
