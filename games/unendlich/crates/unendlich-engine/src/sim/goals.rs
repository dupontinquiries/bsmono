//! Goals — utility-based priority arbitration. Given the appraisal [`Signals`]
//! and a creature's [`Disposition`], score each candidate goal and pick the
//! highest. Pure, integer/fixed-point, deterministic.
//!
//! This is where temperament becomes behavior: feed the *same* signals to two
//! creatures and a bold, territorial one scores `Defend` highest while a wary
//! one scores `Evade` (adapt / work around) or `Flee` (when actually hurt).
//!
//! `Signals` are the output of appraisal (a later brick) — perceived threat,
//! food opportunity, novelty, hunger, and current health. Until appraisal
//! exists this module is exercised directly; the arbitration logic is the part
//! worth pinning down first because it defines what every other cognitive piece
//! is feeding *into*.

use crate::sim::fixed::Fixed;
use crate::sim::personality::Disposition;

/// The behaviors a creature can pursue. The array order in [`Goal::ORDER`] is
/// also the deterministic tie-break priority (earlier wins an exact tie):
/// survival, then territory, then needs, then idle drift.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Goal {
    /// Panic-retreat: low health.
    Flee,
    /// Confront a threat in territory (aggression + territoriality).
    Defend,
    /// Keep away from / route around a threat without fighting (caution).
    Evade,
    /// Pursue food: raid a village, hunt livestock (greed × hunger).
    Raid,
    /// Approach something novel (curiosity).
    Investigate,
    /// Default territorial behavior: cycle known landmarks.
    Patrol,
    /// Lowest-priority aimless drift.
    #[default]
    Wander,
}

impl Goal {
    /// All goals in tie-break priority order.
    pub const ORDER: [Goal; 7] = [
        Goal::Flee,
        Goal::Defend,
        Goal::Evade,
        Goal::Raid,
        Goal::Investigate,
        Goal::Patrol,
        Goal::Wander,
    ];
}

/// The situation a creature is appraising, every field a [`Fixed`] in `[0, 1]`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Signals {
    /// Strength of a threat within the creature's territory.
    pub threat: Fixed,
    /// Strength of a perceived or recalled food opportunity.
    pub food: Fixed,
    /// Novelty of something unexplored.
    pub novelty: Fixed,
    /// Hunger / need pressure driving raids.
    pub hunger: Fixed,
    /// Health fraction (`1` = full); flee pressure rises as it falls.
    pub health_frac: Fixed,
}

/// Baseline pull of patrolling for a territorial creature when nothing else is
/// salient; scaled by `territoriality`.
const PATROL_BASE: Fixed = Fixed::from_frac(15, 100);
/// Floor activity when even patrol is weak.
const WANDER_BASE: Fixed = Fixed::from_frac(5, 100);
/// Stickiness added to the currently-active goal so near-ties don't dither.
const HYSTERESIS: Fixed = Fixed::from_frac(8, 100);
const HALF: Fixed = Fixed::from_frac(1, 2);

#[inline]
fn avg(a: Fixed, b: Fixed) -> Fixed {
    (a + b) * HALF
}

/// Utility of pursuing `goal` given the situation and temperament. All terms
/// are products of `[0,1]` values (plus small constant baselines), so scores
/// stay in `[0,1]` and never overflow.
pub fn score(goal: Goal, s: &Signals, d: &Disposition) -> Fixed {
    match goal {
        // Survival panic: only meaningful as health drops, sharpened by caution.
        Goal::Flee => (Fixed::ONE - s.health_frac) * d.caution,
        // Stand and fight: a threat met by aggression + territoriality.
        Goal::Defend => s.threat * avg(d.aggression, d.territoriality),
        // Adapt: keep distance from a threat rather than fight it.
        Goal::Evade => s.threat * d.caution,
        // Feed: opportunity × appetite × pressure.
        Goal::Raid => (s.food * d.greed) * s.hunger,
        // Explore the unknown.
        Goal::Investigate => s.novelty * d.curiosity,
        // Hold the range.
        Goal::Patrol => PATROL_BASE * d.territoriality,
        // Drift.
        Goal::Wander => WANDER_BASE,
    }
}

/// Choose the highest-utility goal, adding [`HYSTERESIS`] to the `current` goal
/// so the creature commits rather than flickering between near-tied options.
/// Exact ties resolve to the earlier goal in [`Goal::ORDER`].
pub fn choose(s: &Signals, d: &Disposition, current: Goal) -> Goal {
    let mut best = Goal::Wander;
    let mut best_score = Fixed(i64::MIN);
    for g in Goal::ORDER {
        let mut sc = score(g, s, d);
        if g == current {
            sc = sc + HYSTERESIS;
        }
        if sc > best_score {
            best_score = sc;
            best = g;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disp(a: i64, t: i64, c: i64, cu: i64, g: i64) -> Disposition {
        let f = |x: i64| Fixed::from_frac(x, 100);
        Disposition { aggression: f(a), territoriality: f(t), caution: f(c), curiosity: f(cu), greed: f(g) }
    }
    fn sig(threat: i64, food: i64, novelty: i64, hunger: i64, health: i64) -> Signals {
        let f = |x: i64| Fixed::from_frac(x, 100);
        Signals { threat: f(threat), food: f(food), novelty: f(novelty), hunger: f(hunger), health_frac: f(health) }
    }

    #[test]
    fn temperament_splits_the_same_threat() {
        let bold = disp(90, 90, 10, 50, 50);
        let timid = disp(10, 20, 90, 50, 50);
        let threat = sig(80, 0, 0, 0, 100); // threatened, full health
        assert_eq!(choose(&threat, &bold, Goal::Wander), Goal::Defend);
        assert_eq!(choose(&threat, &timid, Goal::Wander), Goal::Evade);
    }

    #[test]
    fn hurt_timid_flees_bold_still_defends() {
        let bold = disp(90, 90, 10, 50, 50);
        let timid = disp(10, 20, 90, 50, 50);
        let hurt = sig(80, 0, 0, 0, 15);
        assert_eq!(choose(&hurt, &bold, Goal::Wander), Goal::Defend);
        assert_eq!(choose(&hurt, &timid, Goal::Wander), Goal::Flee);
    }

    #[test]
    fn raiding_is_gated_by_hunger() {
        let greedy = disp(30, 30, 30, 40, 95);
        assert_eq!(choose(&sig(0, 100, 0, 100, 100), &greedy, Goal::Wander), Goal::Raid);
        assert_ne!(choose(&sig(0, 100, 0, 10, 100), &greedy, Goal::Wander), Goal::Raid);
    }

    #[test]
    fn novelty_and_quiet_world() {
        let curious = disp(30, 30, 30, 95, 30);
        assert_eq!(choose(&sig(0, 0, 90, 0, 100), &curious, Goal::Wander), Goal::Investigate);
        let territorial = disp(90, 90, 10, 50, 50);
        assert_eq!(choose(&sig(0, 0, 0, 0, 100), &territorial, Goal::Wander), Goal::Patrol);
    }

    #[test]
    fn hysteresis_prevents_dither() {
        let neutral = disp(50, 50, 50, 50, 50);
        // Defend, Evade, Investigate all score 0.15 here; current goal should hold.
        let tie = sig(30, 0, 30, 0, 100);
        assert_eq!(choose(&tie, &neutral, Goal::Investigate), Goal::Investigate);
        assert_eq!(choose(&tie, &neutral, Goal::Evade), Goal::Evade);
    }

    #[test]
    fn is_deterministic() {
        let d = disp(40, 60, 30, 50, 70);
        let s = sig(50, 40, 20, 60, 80);
        assert_eq!(choose(&s, &d, Goal::Patrol), choose(&s, &d, Goal::Patrol));
    }
}
