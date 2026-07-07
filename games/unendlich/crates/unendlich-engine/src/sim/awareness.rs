//! Awareness — an information-seeking drive that steers a creature's *facing*
//! (separate from where it moves). Getting cornered is fine; staring at the wall
//! you're cornered against is not. A creature low on information — boxed in, or
//! just idle and unsure — should turn toward where it can *see* the most and
//! knows the least, so it keeps building a picture of its surroundings (and, when
//! a predator drives it into a corner, at least turns to face the open ground the
//! threat will come across). This is the "survey / get your bearings" behaviour.
//!
//! Pure + testable: the world enters only through a `surface_at` height sampler
//! (the eyes). No knowledge map is stored here yet — `familiarity` is supplied by
//! the caller, so a memory layer can plug in later.

use std::f32::consts::{PI, TAU};

/// Smallest absolute angle between two headings (handles wrap-around).
pub fn angle_between(a: f32, b: f32) -> f32 {
    let mut d = (a - b) % TAU;
    if d > PI {
        d -= TAU;
    } else if d < -PI {
        d += TAU;
    }
    d.abs()
}

/// Rotate heading `current` toward `desired` by at most `max_step` radians,
/// taking the shorter way around the circle. A creature with a finite turn rate
/// (degrees-per-second × dt) *sweeps* to a new facing instead of snapping — no
/// instant 180° flips. When the target is within `max_step` it lands exactly on
/// `desired`. `max_step <= 0` holds the current heading.
pub fn turn_toward(current: f32, desired: f32, max_step: f32) -> f32 {
    if max_step <= 0.0 {
        return current;
    }
    // Shortest signed angular difference in (-PI, PI].
    let mut diff = (desired - current) % TAU;
    if diff > PI {
        diff -= TAU;
    } else if diff < -PI {
        diff += TAU;
    }
    if diff.abs() <= max_step {
        desired
    } else {
        current + max_step * diff.signum()
    }
}

/// How much information facing a direction yields, in `[0,1]`: how open the view
/// is (clear sightline vs the creature's max view range) plus a novelty bonus for
/// ground it knows least (`familiarity` in `[0,1]`, 1 = already well known).
/// Openness dominates — you learn most by being able to see far.
pub fn look_value(clear_dist: f32, max_view: f32, familiarity: f32) -> f32 {
    let openness = if max_view <= 0.0 { 0.0 } else { (clear_dist / max_view).clamp(0.0, 1.0) };
    let novelty = 1.0 - familiarity.clamp(0.0, 1.0);
    openness * 0.7 + novelty * 0.3
}

/// March along `heading` from `from` until the heightfield rises more than
/// `step_up` above `base_y` (a wall cuts off the view) or runs into a void, or
/// `max_view` is reached; return how far the view stays clear. This is the eyes
/// measuring openness — a long clear line is a direction worth facing.
pub fn clear_sightline(
    from: (f32, f32),
    heading: f32,
    max_view: f32,
    base_y: i32,
    step_up: i32,
    surface_at: impl Fn(f32, f32) -> Option<i32>,
) -> f32 {
    let (cx, cz) = (heading.cos(), heading.sin());
    let mut d = 1.0;
    while d <= max_view {
        match surface_at(from.0 + cx * d, from.1 + cz * d) {
            Some(sy) if sy - base_y > step_up => return d - 1.0, // a wall blocks the view here
            None => return d - 1.0,                              // edge of the known world
            _ => {}
        }
        d += 1.0;
    }
    max_view
}

/// A direction the creature could turn to face, with what its eyes/memory report
/// about it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LookCandidate {
    pub heading: f32,
    pub clear_dist: f32,
    /// `[0,1]`, how well this direction is already known (1 = fully).
    pub familiarity: f32,
}

/// Pick the facing that maximizes information among `candidates`, given the
/// creature's `max_view`. A small turn-cost bias breaks near-ties toward the
/// current heading so it doesn't twitch. Returns `None` if there are no
/// candidates.
pub fn best_facing(current: f32, candidates: &[LookCandidate], max_view: f32) -> Option<f32> {
    candidates
        .iter()
        .map(|c| {
            let info = look_value(c.clear_dist, max_view, c.familiarity);
            let turn_cost = 0.05 * (angle_between(current, c.heading) / PI);
            (c.heading, info - turn_cost)
        })
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(h, _)| h)
}

/// Whether the creature's current view is cramped enough that it should stop and
/// survey — true when what it can currently see is a small fraction of its range
/// (e.g. nose against a wall).
pub fn should_survey(current_clear_dist: f32, max_view: f32) -> bool {
    max_view > 0.0 && current_clear_dist < 0.25 * max_view
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_toward_sweeps_then_lands_taking_the_short_way() {
        // Steps toward a far target by exactly max_step, no overshoot.
        assert!((turn_toward(0.0, PI / 2.0, 0.1) - 0.1).abs() < 1e-6);
        // Lands exactly when within reach.
        assert!((turn_toward(0.0, 0.05, 0.1) - 0.05).abs() < 1e-6);
        // A target nearly behind: turning the SHORT way is negative here.
        assert!(turn_toward(0.0, PI * 0.9, 0.1) > 0.0, "positive short way to +0.9PI");
        assert!(turn_toward(0.0, -PI * 0.9, 0.1) < 0.0, "negative short way to -0.9PI");
        // Across the wrap (3.0 -> -3.0 is short via +PI, ~0.28 rad away).
        let r = turn_toward(3.0, -3.0, 0.1);
        assert!(angle_between(r, -3.0) < angle_between(3.0, -3.0), "moved closer to target");
        assert!(r > 3.0, "took the short way across +PI, not the long way back");
        // Non-positive step holds.
        assert_eq!(turn_toward(1.2, 2.0, 0.0), 1.2);
    }

    #[test]
    fn turn_toward_reaches_a_reversal_only_over_several_steps() {
        // A 180° flip at a finite rate takes many ticks — never instant.
        let desired = PI;
        let mut h = 0.0;
        let max_step = 0.2;
        let mut steps = 0;
        while angle_between(h, desired) > 1e-3 && steps < 100 {
            h = turn_toward(h, desired, max_step);
            steps += 1;
        }
        assert!(steps >= (PI / max_step) as i32 - 1, "a half-turn can't happen in one step");
        assert!(angle_between(h, desired) < 1e-3, "but it does get there");
    }

    #[test]
    fn open_and_novel_directions_are_worth_more() {
        let max = 32.0;
        // Open beats cramped at equal familiarity.
        assert!(look_value(30.0, max, 0.5) > look_value(4.0, max, 0.5));
        // Novel beats familiar at equal openness.
        assert!(look_value(20.0, max, 0.0) > look_value(20.0, max, 1.0));
    }

    #[test]
    fn a_wall_cuts_the_sightline_short_open_ground_runs_to_max() {
        let base = 64;
        // Wall (rises >1) starting at distance 5 along +X.
        let walled = |x: f32, _z: f32| Some(if x >= 5.0 { 80 } else { 64 });
        let s = clear_sightline((0.0, 0.0), 0.0, 32.0, base, 1, walled);
        assert!(s >= 3.0 && s <= 5.0, "sightline stops near the wall (got {s})");
        // Flat ground: the view runs out to max_view.
        let flat = |_x: f32, _z: f32| Some(64);
        assert_eq!(clear_sightline((0.0, 0.0), 0.0, 32.0, base, 1, flat), 32.0);
    }

    #[test]
    fn cornered_creature_turns_to_face_the_open_direction() {
        // Facing +X (heading 0) is walled (short view); the open way is behind
        // (heading PI, long view). best_facing should turn it around.
        let max = 32.0;
        let candidates = [
            LookCandidate { heading: 0.0, clear_dist: 2.0, familiarity: 0.5 },
            LookCandidate { heading: PI, clear_dist: 30.0, familiarity: 0.5 },
            LookCandidate { heading: PI / 2.0, clear_dist: 6.0, familiarity: 0.5 },
        ];
        assert_eq!(best_facing(0.0, &candidates, max), Some(PI), "turns toward the open view");
        // It knows its view is cramped and should survey.
        assert!(should_survey(2.0, max));
        assert!(!should_survey(30.0, max));
    }

    #[test]
    fn near_ties_break_toward_the_current_heading() {
        // Two equally-informative open directions; the one nearer the current
        // facing wins (no needless spinning).
        let max = 32.0;
        let candidates = [
            LookCandidate { heading: 0.3, clear_dist: 28.0, familiarity: 0.5 },
            LookCandidate { heading: 3.0, clear_dist: 28.0, familiarity: 0.5 },
        ];
        assert_eq!(best_facing(0.0, &candidates, max), Some(0.3));
    }
}
