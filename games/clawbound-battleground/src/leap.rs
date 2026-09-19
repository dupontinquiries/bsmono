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

use glam::Vec3;

use crate::blockers::Blockers;
use crate::puppet::LeapView;

/// Shortest and longest ground distance a leap covers.
pub const LEAP_MIN: f32 = 2.0;
pub const LEAP_RANGE: f32 = 7.5;
/// Tallest thing a cat lands on. A rock is 1.4, a boulder cluster 2.0, a
/// fallen log 0.9; a ruin pillar (4.0) and any tree are out.
pub const PERCH_MAX_TOP: f32 = 2.4;
/// Too small to stand on.
pub const PERCH_MIN_RADIUS: f32 = 0.5;
/// How far off the aim line a perch can be and still be "what I meant".
pub const AIM_SLACK: f32 = 1.3;
pub const GATHER_SECS: f32 = 0.18;
/// From a perch, or buffered during a landing: already coiled.
pub const CHAIN_GATHER_SECS: f32 = 0.06;
pub const LAND_SECS: f32 = 0.20;
pub const LEAP_SPEED: f32 = 15.0;
pub const LEAP_COOLDOWN: f32 = 1.6;
/// Rise above the higher of the two ends, plus a share of the distance.
const APEX_BASE: f32 = 0.7;
const APEX_PER_UNIT: f32 = 0.11;
/// How fast a cat drops when it walks off a perch.
pub const STEP_OFF_FALL: f32 = 11.0;

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
}

/// Where a leap would go.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    pub to: Vec3,
    pub perch: Option<f32>,
}

/// Choose a landing for a leap from `pos` along `dir` (xz; need not be unit).
///
/// A perch wins over open ground whenever one is near the aim line, because
/// "I pressed leap while looking at a rock" means the rock. Among perches the
/// NEAREST along the line wins, not the best-centred: the far one is the
/// next leap.
pub fn plan(pos: Vec3, dir: Vec3, standing_on: Option<f32>, blockers: &Blockers) -> Option<Plan> {
    let d = Vec3::new(dir.x, 0.0, dir.z);
    let len = d.length();
    if !(len > 1e-4) || !pos.is_finite() {
        return None;
    }
    let d = d / len;

    // Anything too tall to land on is a WALL: a leap neither lands beyond it
    // nor picks a perch behind it. Without this a cat aimed at a pillar
    // planned a landing on its far side and was shoved round it in mid-air.
    let body = crate::consts::GOD_RADIUS;
    let mut wall = f32::INFINITY;
    let mut s = 0.5;
    while s <= LEAP_RANGE && wall.is_infinite() {
        let (x, z) = (pos.x + d.x * s, pos.z + d.z * s);
        for b in blockers.all() {
            if b.top > PERCH_MAX_TOP {
                let (ax, az, r) = b.shape.nearest_axis(x, z);
                if (x - ax).hypot(z - az) < r + body {
                    wall = s;
                    break;
                }
            }
        }
        s += 0.5;
    }

    let mut best: Option<(f32, Plan)> = None;
    for b in blockers.all() {
        if !(b.top > 0.3 && b.top <= PERCH_MAX_TOP) {
            continue;
        }
        let (ax, az, r) = b.shape.nearest_axis(pos.x + d.x * LEAP_RANGE * 0.5, pos.z + d.z * LEAP_RANGE * 0.5);
        if r < PERCH_MIN_RADIUS {
            continue;
        }
        let rel = Vec3::new(ax - pos.x, 0.0, az - pos.z);
        let along = rel.dot(d);
        let off = (rel - d * along).length();
        if !(LEAP_MIN..=LEAP_RANGE.min(wall)).contains(&along) || off > AIM_SLACK + r * 0.5 {
            continue;
        }
        // Not the thing we are already standing on.
        if standing_on.is_some() && rel.length() < r {
            continue;
        }
        if best.map_or(true, |(a, _)| along < a) {
            best = Some((along, Plan { to: Vec3::new(ax, b.top, az), perch: Some(b.top) }));
        }
    }
    if let Some((_, p)) = best {
        return Some(p);
    }

    // Open ground: full range, pulled in until the landing is clear.
    let mut reach = LEAP_RANGE.min(wall - 0.5);
    while reach >= LEAP_MIN {
        let (x, z) = (pos.x + d.x * reach, pos.z + d.z * reach);
        if !blockers.blocked(x, z, crate::consts::GOD_RADIUS) {
            return Some(Plan { to: Vec3::new(x, 0.0, z), perch: None });
        }
        reach -= 0.5;
    }
    None
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
            flight: (run / LEAP_SPEED).clamp(0.28, 0.6),
            apex: APEX_BASE + APEX_PER_UNIT * run,
            perch: plan.perch,
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
            Phase::Land => LeapView::Land { t: (self.t / LAND_SECS).clamp(0.0, 1.0), perched: self.perch.is_some() },
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
        b.push_prop(PropKind::Rock, 5.0, 0.0, 0.0, 1.0); // top 1.4, 5 u ahead
        b.push_prop(PropKind::BoulderCluster, 10.5, 0.4, 0.0, 1.0); // top 2.0, the next hop
        b.push_prop(PropKind::RuinPillar, 0.0, 5.0, 0.0, 1.0); // top 4.0: not a perch
        b.push_prop(PropKind::Fern, 0.0, -5.0, 0.0, 1.0); // cover only
        b
    }

    #[test]
    fn a_leap_toward_a_rock_lands_on_the_rock() {
        let p = plan(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.1), None, &world()).unwrap();
        assert_eq!(p.perch, Some(1.4));
        assert!((p.to - Vec3::new(5.0, 1.4, 0.0)).length() < 1e-3, "aimed near it, landed ON it: {}", p.to);
    }

    /// Both directions matter: a pillar must not be a perch, and neither
    /// must a fern — one is too tall and the other is not there.
    #[test]
    fn pillars_and_ferns_are_not_perches() {
        let at_pillar = plan(Vec3::ZERO, Vec3::Z, None, &world()).unwrap();
        assert_eq!(at_pillar.perch, None);
        assert!(at_pillar.to.z < 5.0 - 0.8, "and the ground landing stops short of it: {}", at_pillar.to);
        let at_fern = plan(Vec3::ZERO, -Vec3::Z, None, &world()).unwrap();
        assert_eq!(at_fern.perch, None);
        assert!((at_fern.to.z + LEAP_RANGE).abs() < 1e-3, "ferns do not shorten a leap");
    }

    #[test]
    fn from_a_perch_the_next_rock_is_chosen_not_the_one_underfoot() {
        let on_rock = Vec3::new(5.0, 1.4, 0.0);
        let p = plan(on_rock, Vec3::X, Some(1.4), &world()).unwrap();
        assert_eq!(p.perch, Some(2.0));
        assert!((p.to.x - 10.5).abs() < 1e-3);
    }

    #[test]
    fn the_arc_starts_and_ends_where_it_says_and_clears_both_ends() {
        let plan = Plan { to: Vec3::new(6.0, 1.4, 0.0), perch: Some(1.4) };
        let l = LeapState::begin(Vec3::ZERO, plan, false);
        assert!((l.arc(0.0) - l.from).length() < 1e-5 && (l.arc(1.0) - l.to).length() < 1e-5);
        let top = (0..=20).map(|i| l.arc(i as f32 / 20.0).y).fold(0.0f32, f32::max);
        assert!((2.0..3.5).contains(&top), "clears the rock without going into orbit: {top}");
        assert!(l.climb(0.0) > 0.3 && l.climb(1.0) < -0.2, "up then down");
    }

    #[test]
    fn phases_run_in_order_and_a_chain_gathers_faster() {
        let plan = Plan { to: Vec3::new(6.0, 0.0, 0.0), perch: None };
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
        let chained = LeapState::begin(Vec3::ZERO, plan, true);
        assert!(chained.gather < l.gather * 0.5);
    }

    #[test]
    fn nonsense_aims_plan_nothing() {
        assert_eq!(plan(Vec3::ZERO, Vec3::ZERO, None, &world()), None);
        assert_eq!(plan(Vec3::ZERO, Vec3::Y, None, &world()), None, "straight up has no xz direction");
        assert_eq!(plan(Vec3::new(f32::NAN, 0.0, 0.0), Vec3::X, None, &world()), None);
    }
}
