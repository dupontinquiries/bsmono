//! Steering — how a creature *turns its body* while wandering, made to look
//! alive rather than robotic. Two ideas the raw turn-rate cap doesn't give you:
//!
//! 1. A **detent** (deadzone): heading errors below a threshold produce no turn
//!    at all, so a creature stops micro-vacillating clockwise/counter-clockwise
//!    when its desired heading jitters by a hair each tick.
//! 2. A **nonlinear turn map**: the turn rate eases in with the size of the
//!    error (small errors → gentle correction, big errors → near the full rate),
//!    so motion curves instead of stepping linearly toward the target.
//!
//! Each creature carries its own [`TurnProfile`] ("stored in the brain"), so a
//! herd doesn't all turn identically. This is meant for **wandering** — goal
//! pathing and fleeing should still turn crisply (use the plain rate cap there),
//! which is why this is a separate call.
//!
//! Pure + deterministic, trig-free except the caller's headings.

use std::f32::consts::{PI, TAU};

/// A creature's idle-turning character.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TurnProfile {
    /// Detent half-width in radians: errors within this are ignored (held).
    pub deadzone: f32,
    /// Nonlinear response exponent (>1 eases in; 1 is linear).
    pub ease: f32,
}

impl TurnProfile {
    pub fn new(deadzone: f32, ease: f32) -> TurnProfile {
        TurnProfile { deadzone: deadzone.max(0.0), ease: ease.max(0.1) }
    }

    /// A reasonable, slightly-varied profile from a seed: deadzone ~0.05..0.18
    /// rad (~3..10°), ease ~1.3..2.4.
    pub fn from_seed(seed: u64) -> TurnProfile {
        let mut x = seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(0xD1B5_4A32_D192_ED03);
        let mut next01 = || {
            x ^= x >> 30;
            x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x ^= x >> 27;
            ((x >> 33) as f32) / ((1u64 << 31) as f32)
        };
        let deadzone = 0.05 + next01() * 0.13;
        let ease = 1.3 + next01() * 1.1;
        TurnProfile::new(deadzone, ease)
    }
}

/// Shortest signed angular difference `desired - current`, in `(-PI, PI]`.
fn shortest(desired: f32, current: f32) -> f32 {
    let mut d = (desired - current) % TAU;
    if d > PI {
        d -= TAU;
    } else if d < -PI {
        d += TAU;
    }
    d
}

/// Turn `current` heading toward `desired` for one tick of wandering, applying
/// the profile's detent and nonlinear response. `max_step` is the hard cap for
/// this tick (turn_rate × dt). Within the deadzone it HOLDS (returns `current`,
/// no vacillation); otherwise it eases toward `desired` the short way around,
/// never overshooting.
pub fn wander_turn(current: f32, desired: f32, max_step: f32, profile: TurnProfile) -> f32 {
    if max_step <= 0.0 {
        return current;
    }
    let diff = shortest(desired, current);
    let err = diff.abs();
    if err <= profile.deadzone {
        return current; // detent: ignore tiny corrections
    }
    // Map the error past the deadzone into [0,1], ease it, scale the rate cap.
    // A small floor keeps it from crawling forever just outside the detent: once
    // it crosses in, the deadzone holds it (capped at err, so never overshoots).
    let span = (PI - profile.deadzone).max(1e-3);
    let t = ((err - profile.deadzone) / span).clamp(0.0, 1.0);
    const MIN_TURN_FRAC: f32 = 0.08;
    let frac = MIN_TURN_FRAC + (1.0 - MIN_TURN_FRAC) * t.powf(profile.ease);
    let amount = (max_step * frac).min(err); // no overshoot
    current + amount * diff.signum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ang(a: f32, b: f32) -> f32 {
        shortest(a, b).abs()
    }

    #[test]
    fn within_the_detent_it_holds_no_vacillation() {
        let p = TurnProfile::new(0.1, 2.0);
        // Desired jitters +/- a hair around current each "tick" → no movement.
        assert_eq!(wander_turn(1.0, 1.0 + 0.05, 0.5, p), 1.0);
        assert_eq!(wander_turn(1.0, 1.0 - 0.05, 0.5, p), 1.0);
        // A real correction (beyond the detent) does move.
        assert!(wander_turn(1.0, 1.0 + 0.4, 0.5, p) > 1.0);
    }

    #[test]
    fn the_response_is_nonlinear_small_errors_turn_gentler_than_linear() {
        let p = TurnProfile::new(0.05, 2.0);
        let max_step = 0.3;
        // A modest error past the deadzone: eased turn is less than the linear
        // cap would give (because ease > 1 and t < 1).
        let err = 0.5;
        let got = wander_turn(0.0, err, max_step, p) - 0.0;
        assert!(got < max_step, "eased response stays under the rate cap for a mid error");
        // A large error approaches the full rate cap.
        let big = wander_turn(0.0, PI - 0.01, max_step, p);
        assert!(big > max_step * 0.95, "near-opposite error uses ~full rate");
    }

    #[test]
    fn it_takes_the_short_way_and_never_overshoots() {
        let p = TurnProfile::new(0.02, 1.5);
        // Target slightly behind on the negative side → turn negative.
        assert!(wander_turn(0.0, -PI * 0.8, 0.5, p) < 0.0);
        // Tiny error just past the deadzone → step is clamped to the error (lands,
        // doesn't overshoot).
        let r = wander_turn(0.0, 0.03, 1.0, TurnProfile::new(0.02, 1.0));
        assert!(r > 0.0 && r <= 0.03 + 1e-6, "no overshoot past the target");
    }

    #[test]
    fn wandering_settles_near_the_target_within_a_few_ticks() {
        let p = TurnProfile::new(0.08, 2.0);
        let desired = 2.0;
        let mut h = 0.0;
        for _ in 0..200 {
            h = wander_turn(h, desired, 0.15, p);
        }
        assert!(ang(h, desired) <= p.deadzone + 1e-3, "comes to rest within the detent of target");
    }

    #[test]
    fn profiles_vary_by_seed_but_stay_in_sane_bounds() {
        let mut distinct = std::collections::HashSet::new();
        for s in 0..200u64 {
            let p = TurnProfile::from_seed(s);
            assert!(p.deadzone >= 0.05 && p.deadzone <= 0.18);
            assert!(p.ease >= 1.3 && p.ease <= 2.4);
            distinct.insert((p.deadzone.to_bits(), p.ease.to_bits()));
        }
        assert!(distinct.len() > 150, "seeds produce a spread of profiles, not clones");
    }
}
