//! liveliness — every pose carries a little physics. A creature is never purely
//! rigid: idling sways, walking has secondary jiggle on the limbs, resting goes
//! slack, casting tenses then recoils, and dying hands fully over to the ragdoll.
//! This is the blend schedule — how *limp* the body is per state (0 = the
//! animation drives the pose, 1 = the [`crate::sim::ragdoll`] owns it) — plus the
//! breathing swell that rides on top of every living state. The renderer lerps
//! the driven pose toward the ragdoll result by `limpness`, and scales the torso
//! by `breath`. Pure.

use std::f32::consts::TAU;

/// What the body is doing this tick. Steady states (Idle/Walking/Resting) ignore
/// progress; the transient ones (Casting/Dying) read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionState {
    Idle,
    Walking,
    Resting,
    Casting,
    Dying,
}

/// Faint baseline limpness while idling (a little sway).
pub const IDLE_LIMP: f32 = 0.08;
/// Secondary motion on the limbs while walking.
pub const WALK_LIMP: f32 = 0.15;
/// Slack, settled body at rest.
pub const REST_LIMP: f32 = 0.25;

/// How limp the body is in `state` at `progress` in `[0, 1]`. 0 = the animation
/// drives the pose rigidly; 1 = physics owns it entirely.
pub fn limpness(state: MotionState, progress: f32) -> f32 {
    match state {
        MotionState::Idle => IDLE_LIMP,
        MotionState::Walking => WALK_LIMP,
        MotionState::Resting => REST_LIMP,
        MotionState::Casting => cast_curve(progress),
        MotionState::Dying => progress.clamp(0.0, 1.0),
    }
}

/// The casting limpness arc: a tense, rigid wind-up (limpness dips *below* idle),
/// a recoil spike as the ability releases, then a decay back toward idle.
pub fn cast_curve(progress: f32) -> f32 {
    let p = progress.clamp(0.0, 1.0);
    if p < 0.45 {
        // anticipation: tense up (more rigid than idle)
        0.06 - 0.04 * (p / 0.45)
    } else if p < 0.6 {
        // release: the body whips — a recoil spike
        let t = (p - 0.45) / 0.15;
        0.02 + 0.43 * t
    } else {
        // recovery: settle back to the idle baseline
        let t = (p - 0.6) / 0.4;
        0.45 - (0.45 - IDLE_LIMP) * t
    }
}

/// Breathing swell: a multiplier around 1.0 that oscillates by ±`depth` at `rate`
/// Hz. Apply to the torso half-extents (e.g. via [`crate::sim::deform`]) so the
/// chest rises and falls. `t` is seconds.
pub fn breath(t: f32, rate: f32, depth: f32) -> f32 {
    1.0 + depth * (TAU * rate * t).sin()
}

/// Breathing rate (Hz) and depth for a state: resting breaths are slow and deep,
/// walking is quick and shallow (exertion), casting holds the breath.
pub fn breath_params(state: MotionState) -> (f32, f32) {
    match state {
        MotionState::Resting => (0.15, 0.06),
        MotionState::Idle => (0.25, 0.04),
        MotionState::Walking => (0.50, 0.03),
        MotionState::Casting => (0.20, 0.015),
        MotionState::Dying => (0.10, 0.02),
    }
}

/// Breathing swell for a state at time `t`.
pub fn breath_for(state: MotionState, t: f32) -> f32 {
    let (rate, depth) = breath_params(state);
    breath(t, rate, depth)
}

/// Blend a driven pose value toward the physics value by `limpness` (0 = driven,
/// 1 = physics).
pub fn blend(driven: f32, physics: f32, limpness: f32) -> f32 {
    let w = limpness.clamp(0.0, 1.0);
    driven * (1.0 - w) + physics * w
}

/// Per-axis [`blend`].
pub fn blend3(driven: [f32; 3], physics: [f32; 3], limpness: f32) -> [f32; 3] {
    [
        blend(driven[0], physics[0], limpness),
        blend(driven[1], physics[1], limpness),
        blend(driven[2], physics[2], limpness),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dying_ramps_from_pose_to_full_ragdoll() {
        assert_eq!(limpness(MotionState::Dying, 0.0), 0.0);
        assert_eq!(limpness(MotionState::Dying, 1.0), 1.0);
        assert!(limpness(MotionState::Dying, 0.5) > limpness(MotionState::Dying, 0.2));
    }

    #[test]
    fn living_states_are_ordered_slack() {
        // resting is slacker than walking, which carries more than idle, all far
        // below full ragdoll.
        let idle = limpness(MotionState::Idle, 0.0);
        let walk = limpness(MotionState::Walking, 0.0);
        let rest = limpness(MotionState::Resting, 0.0);
        assert!(idle < walk && walk < rest, "idle {idle} < walk {walk} < rest {rest}");
        assert!(rest < limpness(MotionState::Dying, 1.0));
    }

    #[test]
    fn casting_tenses_then_recoils() {
        let tense = cast_curve(0.2); // wind-up
        let peak = cast_curve(0.6); // release
        let end = cast_curve(1.0); // recovered
        assert!(tense < IDLE_LIMP, "wind-up is more rigid than idle: {tense}");
        assert!(peak > REST_LIMP, "release recoils past resting slack: {peak}");
        assert!((end - IDLE_LIMP).abs() < 0.02, "recovers to idle baseline: {end}");
    }

    #[test]
    fn breath_oscillates_around_one_within_depth() {
        let depth = 0.06;
        assert!((breath(0.0, 0.2, depth) - 1.0).abs() < 1e-5, "starts at neutral");
        // quarter period → peak swell
        let peak = breath(1.0 / (4.0 * 0.2), 0.2, depth);
        assert!((peak - (1.0 + depth)).abs() < 1e-4, "peaks at 1+depth: {peak}");
        for i in 0..200 {
            let v = breath(i as f32 * 0.05, 0.2, depth);
            assert!(v >= 1.0 - depth - 1e-4 && v <= 1.0 + depth + 1e-4, "stays within band");
        }
    }

    #[test]
    fn resting_breathes_deeper_than_walking() {
        let (_, rest_depth) = breath_params(MotionState::Resting);
        let (_, walk_depth) = breath_params(MotionState::Walking);
        assert!(rest_depth > walk_depth, "rest deeper than walk");
    }

    #[test]
    fn blend_spans_driven_to_physics() {
        assert_eq!(blend(10.0, 20.0, 0.0), 10.0);
        assert_eq!(blend(10.0, 20.0, 1.0), 20.0);
        assert_eq!(blend(10.0, 20.0, 0.5), 15.0);
        assert_eq!(blend3([0.0; 3], [2.0, 4.0, 6.0], 0.5), [1.0, 2.0, 3.0]);
    }
}
