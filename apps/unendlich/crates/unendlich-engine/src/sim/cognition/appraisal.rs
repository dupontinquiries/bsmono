//! Appraisal — turns what a creature *perceives* plus what it *remembers* into
//! the objective [`Signals`] the goals layer scores.
//!
//! Separation of concerns: appraisal computes how threatening / foody / novel
//! the situation **objectively** is (using geometry + memory); the goals layer
//! ([`crate::sim::goals`]) applies the individual's **temperament**. So
//! `Disposition` is deliberately absent here — folding it in would double-count
//! with the goal scores.
//!
//! Tick ordering matters: run `appraise` **before** updating memory with this
//! tick's sightings, so "novelty" reflects whether the creature *already knew*
//! a thing. The intended cognition loop is:
//! `perceive → appraise(vs prior memory) → memory.observe(…) → memory.decay() →
//! goals::choose`.
//!
//! Server-side (not rolled back), integer/`Fixed`, deterministic, testable.

use crate::sim::cognition::memory::{CellPos, LandmarkTag, Memory};
use crate::sim::fixed::Fixed;
use crate::sim::goals::Signals;

/// One currently-perceived, already-classified feature (perception/tagging is
/// upstream — see the GPU vision brick).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sighting {
    pub pos: CellPos,
    pub tag: LandmarkTag,
    pub strength: u16,
}

/// The creature's own internal state that feeds the signals directly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SelfState {
    /// Health fraction `[0,1]` (1 = full).
    pub health_frac: Fixed,
    /// Hunger / need pressure `[0,1]`.
    pub hunger: Fixed,
}

// ── tunables ──────────────────────────────────────────────────────────────
/// Strength value at which a feature's contribution saturates to 1.
const STRENGTH_NORM: i64 = 64;
/// A threat felt out to ~48 blocks from the territory anchor.
const THREAT_RANGE_SQ: i64 = 48 * 48;
/// Food matters out to ~64 blocks from the observer.
const FOOD_RANGE_SQ: i64 = 64 * 64;
/// A sighting within ~8 blocks of a same-tag memory counts as recognized.
const RECOGNITION_DIST_SQ: i64 = 64;

#[inline]
fn dist_sq(a: CellPos, b: CellPos) -> i64 {
    let dx = (a.0 - b.0) as i64;
    let dy = (a.1 - b.1) as i64;
    let dz = (a.2 - b.2) as i64;
    dx * dx + dy * dy + dz * dz
}

#[inline]
fn fmax(a: Fixed, b: Fixed) -> Fixed {
    if a > b {
        a
    } else {
        b
    }
}

/// Feature strength mapped into `[0,1]`.
#[inline]
fn norm_strength(s: u16) -> Fixed {
    Fixed::from_frac((s as i64).min(STRENGTH_NORM), STRENGTH_NORM)
}

/// Proximity weight in `[0,1]`: 1 at zero distance, 0 at/beyond `range_sq`.
#[inline]
fn prox(dist_sq: i64, range_sq: i64) -> Fixed {
    if dist_sq >= range_sq {
        Fixed::ZERO
    } else {
        Fixed::from_frac(range_sq - dist_sq, range_sq)
    }
}

/// Appraise the situation into [`Signals`]. `observer` is the creature's cell;
/// `sightings` are what it perceives this tick; `memory` is its *prior* memory.
pub fn appraise(
    observer: CellPos,
    memory: &Memory,
    sightings: &[Sighting],
    self_state: SelfState,
) -> Signals {
    // Threats are weighed by nearness to the territory anchor (the nest), so an
    // intrusion in the home range alarms more than a distant one.
    let anchor = memory.nest().map(|n| n.pos).unwrap_or(observer);
    let mut threat = Fixed::ZERO;
    for s in sightings {
        if s.tag == LandmarkTag::ThreatStructure {
            threat = fmax(threat, norm_strength(s.strength) * prox(dist_sq(anchor, s.pos), THREAT_RANGE_SQ));
        }
    }
    if let Some(t) = memory.strongest(LandmarkTag::ThreatStructure) {
        threat = fmax(threat, t.salience * prox(dist_sq(anchor, t.pos), THREAT_RANGE_SQ));
    }

    // Food from sight OR memory — recalling a known food source drives revisits
    // even when nothing is currently visible.
    let mut food = Fixed::ZERO;
    for s in sightings {
        if s.tag == LandmarkTag::FoodSource {
            food = fmax(food, norm_strength(s.strength) * prox(dist_sq(observer, s.pos), FOOD_RANGE_SQ));
        }
    }
    if let Some(f) = memory.recall_nearest(LandmarkTag::FoodSource, observer) {
        food = fmax(food, f.salience * prox(dist_sq(observer, f.pos), FOOD_RANGE_SQ));
    }

    // Novelty: a sighting with no same-tag memory nearby is unrecognized.
    let mut novelty = Fixed::ZERO;
    for s in sightings {
        let recognized = memory
            .iter()
            .any(|m| m.tag == s.tag && dist_sq(m.pos, s.pos) <= RECOGNITION_DIST_SQ);
        if !recognized {
            novelty = fmax(novelty, norm_strength(s.strength));
        }
    }

    Signals {
        threat,
        food,
        novelty,
        hunger: self_state.hunger,
        health_frac: self_state.health_frac,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::goals::{choose, Goal};
    use crate::sim::personality::Disposition;

    fn st(health: i64, hunger: i64) -> SelfState {
        SelfState { health_frac: Fixed::from_frac(health, 100), hunger: Fixed::from_frac(hunger, 100) }
    }
    fn mem_with(landmarks: &[(CellPos, LandmarkTag, u16)]) -> Memory {
        let mut m = Memory::new(16);
        for &(p, t, s) in landmarks {
            m.observe(p, t, s, 0);
        }
        m
    }

    #[test]
    fn threat_is_weighed_by_nearness_to_nest() {
        let mem = mem_with(&[((0, 0, 0), LandmarkTag::Nest, 1)]);
        let near = appraise((0, 0, 0), &mem, &[Sighting { pos: (10, 0, 0), tag: LandmarkTag::ThreatStructure, strength: 64 }], st(100, 0));
        let far = appraise((0, 0, 0), &mem, &[Sighting { pos: (300, 0, 0), tag: LandmarkTag::ThreatStructure, strength: 64 }], st(100, 0));
        assert!(near.threat > far.threat);
        assert!(near.threat > Fixed::from_frac(1, 2));
        assert_eq!(far.threat, Fixed::ZERO);
    }

    #[test]
    fn food_comes_from_sight_or_memory() {
        // visible food
        let seen = appraise((0, 0, 0), &Memory::new(8), &[Sighting { pos: (20, 0, 0), tag: LandmarkTag::FoodSource, strength: 64 }], st(100, 100));
        assert!(seen.food > Fixed::from_frac(3, 10));
        // nothing visible, but a remembered food source nearby → still drives food
        let mem = mem_with(&[((20, 0, 0), LandmarkTag::FoodSource, 50)]);
        let remembered = appraise((0, 0, 0), &mem, &[], st(100, 100));
        assert!(remembered.food > Fixed::from_frac(2, 10));
        // nothing at all
        let none = appraise((0, 0, 0), &Memory::new(8), &[], st(100, 100));
        assert_eq!(none.food, Fixed::ZERO);
    }

    #[test]
    fn novelty_is_the_unrecognized() {
        let mem = mem_with(&[((100, 0, 0), LandmarkTag::Feature, 1)]);
        let novel = appraise((0, 0, 0), &mem, &[Sighting { pos: (500, 0, 0), tag: LandmarkTag::Feature, strength: 50 }], st(100, 0));
        let known = appraise((0, 0, 0), &mem, &[Sighting { pos: (102, 0, 0), tag: LandmarkTag::Feature, strength: 50 }], st(100, 0));
        assert!(novel.novelty > Fixed::from_frac(1, 2));
        assert_eq!(known.novelty, Fixed::ZERO);
    }

    #[test]
    fn self_state_passes_through() {
        let s = appraise((0, 0, 0), &Memory::new(8), &[], st(30, 70));
        assert_eq!(s.health_frac, Fixed::from_frac(30, 100));
        assert_eq!(s.hunger, Fixed::from_frac(70, 100));
    }

    #[test]
    fn end_to_end_threat_splits_by_temperament() {
        let f = |x: i64| Fixed::from_frac(x, 100);
        let bold = Disposition { aggression: f(90), territoriality: f(90), caution: f(10), curiosity: f(50), greed: f(50) };
        let timid = Disposition { aggression: f(10), territoriality: f(20), caution: f(90), curiosity: f(50), greed: f(50) };
        let mem = mem_with(&[((0, 0, 0), LandmarkTag::Nest, 1)]);
        let sig = appraise((0, 0, 0), &mem, &[Sighting { pos: (10, 0, 0), tag: LandmarkTag::ThreatStructure, strength: 64 }], st(100, 0));
        assert_eq!(choose(&sig, &bold, Goal::Wander), Goal::Defend);
        assert_eq!(choose(&sig, &timid, Goal::Wander), Goal::Evade);
    }
}
