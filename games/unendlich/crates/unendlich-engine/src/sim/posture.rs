//! Posture — how a living entity *leans into the terrain* so it climbs and
//! descends hills like a body rather than a sliding billboard. From a few ground
//! samples around the feet we derive a target pitch (nose up/down on slopes) and
//! roll (banking across side-slopes); the entity then eases its current
//! orientation toward that target at a bounded *rotational correction speed*, so
//! it tilts smoothly instead of snapping — the "basic ragdoll" feel.
//!
//! Pure + deterministic. Angles in radians; small-angle slopes are exact via
//! atan. This is the orientation model; the bin samples real ground heights
//! (front/back for pitch, left/right for roll) and feeds them in.

/// An entity's body orientation. `yaw` is heading (handled by steering); `pitch`
/// and `roll` are the terrain lean.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Posture {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
}

impl Posture {
    pub fn new(yaw: f32) -> Posture {
        Posture { yaw, pitch: 0.0, roll: 0.0 }
    }
}

/// Target pitch & roll (radians) from ground heights sampled around the body.
/// `front`/`back` are surface heights ahead of and behind the feet along the
/// heading; `left`/`right` are to the sides. `span` is the distance between the
/// opposing samples (body length / width). Pitch is positive nose-UP (climbing);
/// roll is positive when the left side is higher.
pub fn terrain_pitch_roll(front: f32, back: f32, left: f32, right: f32, span: f32) -> (f32, f32) {
    let span = span.max(1e-3);
    let pitch = ((front - back) / span).atan();
    let roll = ((left - right) / span).atan();
    (pitch, roll)
}

/// Ease `current` angle toward `target` by at most `max_step` (the rotational
/// correction speed × dt). Lands exactly when within reach; never overshoots.
/// Used for pitch and roll (small, non-wrapping angles).
pub fn correct(current: f32, target: f32, max_step: f32) -> f32 {
    if max_step <= 0.0 {
        return current;
    }
    let diff = target - current;
    if diff.abs() <= max_step {
        target
    } else {
        current + max_step * diff.signum()
    }
}

impl Posture {
    /// Ease pitch and roll toward the terrain target at `correction_speed`
    /// (rad/s) over `dt`. Yaw is left to the steering layer.
    pub fn settle_to_terrain(&mut self, target_pitch: f32, target_roll: f32, correction_speed: f32, dt: f32) {
        let step = correction_speed * dt;
        self.pitch = correct(self.pitch, target_pitch, step);
        self.roll = correct(self.roll, target_roll, step);
    }
}

/// Tuning for [`terrain_lean`] — turns raw ground-height differences into a
/// *stable* lean instead of letting terrain noise (and cliff edges) throw the
/// body around. Defaults give a calm, upright-biased animal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeanLimits {
    /// Height differences (per opposing pair) are clamped to ±this many blocks
    /// before becoming an angle. A cliff edge 30 blocks down then reads as a
    /// steep-but-bounded slope, not a somersault. Blocks.
    pub max_delta: f32,
    /// Below this angle (radians) the lean is snapped to 0 — a deadzone that
    /// kills jitter from sub-block surface noise (the "gated deltas").
    pub deadzone: f32,
    /// Final lean is clamped to ±this (radians) so nothing tips past a believable
    /// stance even on a wall.
    pub max_angle: f32,
    /// Fraction of the (clamped) slope the body actually commits to. <1 biases
    /// toward upright — the animal leans *into* the hill a bit, it doesn't lie
    /// flat along it. This is the "uprightedness for stability".
    pub commit: f32,
}

impl Default for LeanLimits {
    fn default() -> LeanLimits {
        LeanLimits { max_delta: 1.5, deadzone: 0.06, max_angle: 0.6, commit: 0.6 }
    }
}

/// One axis of [`terrain_lean`]: clamp the height delta, convert to an angle,
/// bias toward upright by `commit`, deadzone tiny leans to 0, clamp to max.
fn lean_axis(delta: f32, span: f32, lim: &LeanLimits) -> f32 {
    let d = delta.clamp(-lim.max_delta, lim.max_delta);
    let a = (d / span.max(1e-3)).atan() * lim.commit;
    let a = if a.abs() < lim.deadzone { 0.0 } else { a };
    a.clamp(-lim.max_angle, lim.max_angle)
}

/// Like [`terrain_pitch_roll`] but *stable*: clamps the per-pair height delta
/// (so a cliff doesn't flip the body), deadzones tiny slopes (no jitter), biases
/// toward upright (`commit`), and clamps the final lean. Use this for living
/// bodies; the raw version is for diagnostics. Pitch +up, roll +left-high.
pub fn terrain_lean(front: f32, back: f32, left: f32, right: f32, span: f32, lim: &LeanLimits) -> (f32, f32) {
    (lean_axis(front - back, span, lim), lean_axis(left - right, span, lim))
}

/// Size-aware terrain lean. A larger creature's legs absorb the same absolute
/// terrain step, so it should tilt *less* for it: a 1-block lip is a stumble for a
/// rabbit but nothing to a dragon. `size` is the body scale (1.0 = baseline); the
/// height deltas are divided by it before the angle is taken, so a size-3 dragon
/// barely leans on a 1-block step yet still leans on a genuine slope. `size <= 1`
/// behaves exactly like [`terrain_lean`].
pub fn terrain_lean_sized(
    front: f32,
    back: f32,
    left: f32,
    right: f32,
    span: f32,
    lim: &LeanLimits,
    size: f32,
) -> (f32, f32) {
    let s = size.max(1.0);
    (
        lean_axis((front - back) / s, span, lim),
        lean_axis((left - right) / s, span, lim),
    )
}

/// How uneven the ground under the feet is: the spread (max − min) of the four
/// foot samples, in blocks. 0 = a flat slab; large = a broken cliff edge. Feeds
/// the stumble model (rough ground + speed → trip).
pub fn terrain_roughness(front: f32, back: f32, left: f32, right: f32) -> f32 {
    let hi = front.max(back).max(left).max(right);
    let lo = front.min(back).min(left).min(right);
    hi - lo
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_ground_is_level() {
        let (p, r) = terrain_pitch_roll(40.0, 40.0, 40.0, 40.0, 2.0);
        assert_eq!((p, r), (0.0, 0.0));
    }

    #[test]
    fn lean_clamps_a_cliff_edge() {
        // A 30-block drop in front (a cliff) must NOT tip the body over: clamped.
        let lim = LeanLimits::default();
        let (cliff, _) = terrain_lean(10.0, 40.0, 40.0, 40.0, 2.0, &lim);
        assert!(cliff.abs() <= lim.max_angle + 1e-6, "cliff lean is clamped, got {cliff}");
        // And it's no steeper than a modest real slope of the clamp size.
        let (slope, _) = terrain_lean(40.0 - lim.max_delta, 40.0, 40.0, 40.0, 2.0, &lim);
        assert!((cliff - slope).abs() < 1e-6, "beyond the clamp, more drop adds no more lean");
    }

    #[test]
    fn lean_deadzones_noise_but_passes_real_slopes() {
        let lim = LeanLimits::default();
        // A few-cm wobble → dead flat (no jitter).
        let (tiny, _) = terrain_lean(40.05, 40.0, 40.0, 40.0, 2.0, &lim);
        assert_eq!(tiny, 0.0, "sub-block noise is deadzoned to level");
        // A real 1-block-over-2 slope still leans.
        let (real, _) = terrain_lean(41.0, 40.0, 40.0, 40.0, 2.0, &lim);
        assert!(real > 0.0, "a real slope still tilts");
    }

    #[test]
    fn commit_biases_toward_upright() {
        // With commit<1 the body leans LESS than the bare slope angle.
        let mut lim = LeanLimits::default();
        lim.deadzone = 0.0;
        lim.commit = 0.5;
        lim.max_delta = 10.0;
        let (p, _) = terrain_lean(42.0, 40.0, 40.0, 40.0, 2.0, &lim);
        let bare = ((42.0_f32 - 40.0) / 2.0).atan();
        assert!(p < bare && p > 0.0, "leans into the slope but stays more upright");
        assert!((p - bare * 0.5).abs() < 1e-5);
    }

    #[test]
    fn roughness_is_flat_zero_and_grows_with_spread() {
        assert_eq!(terrain_roughness(40.0, 40.0, 40.0, 40.0), 0.0);
        assert_eq!(terrain_roughness(40.0, 43.0, 41.0, 40.0), 3.0);
    }

    #[test]
    fn climbing_pitches_nose_up_descending_nose_down() {
        // Front higher than back → climbing → nose up (positive pitch).
        let (up, _) = terrain_pitch_roll(42.0, 40.0, 40.0, 40.0, 2.0);
        assert!(up > 0.0, "climbing tilts nose up");
        // Front lower than back → descending → nose down.
        let (down, _) = terrain_pitch_roll(40.0, 42.0, 40.0, 40.0, 2.0);
        assert!(down < 0.0, "descending tilts nose down");
        // A 2-up-over-2-across slope is a 45° pitch.
        let (p45, _) = terrain_pitch_roll(42.0, 40.0, 40.0, 40.0, 2.0);
        assert!((p45 - std::f32::consts::FRAC_PI_4).abs() < 1e-5);
    }

    #[test]
    fn side_slope_banks_roll() {
        let (_, left_high) = terrain_pitch_roll(40.0, 40.0, 41.0, 40.0, 2.0);
        assert!(left_high > 0.0, "left side higher → positive roll");
        let (_, right_high) = terrain_pitch_roll(40.0, 40.0, 40.0, 41.0, 2.0);
        assert!(right_high < 0.0, "right side higher → negative roll");
    }

    #[test]
    fn correction_eases_toward_target_without_overshoot() {
        // Far target → steps by exactly max_step.
        assert!((correct(0.0, 1.0, 0.1) - 0.1).abs() < 1e-6);
        // Within reach → lands.
        assert!((correct(0.0, 0.05, 0.1) - 0.05).abs() < 1e-6);
        // Negative direction.
        assert!(correct(0.0, -1.0, 0.1) < 0.0);
        // Non-positive step holds.
        assert_eq!(correct(0.3, 1.0, 0.0), 0.3);
    }

    #[test]
    fn posture_settles_onto_a_slope_over_time() {
        let mut p = Posture::new(0.0);
        let (tp, tr) = terrain_pitch_roll(41.0, 40.0, 40.0, 40.5, 2.0);
        for _ in 0..200 {
            p.settle_to_terrain(tp, tr, 3.0, 0.05); // 3 rad/s correction
        }
        assert!((p.pitch - tp).abs() < 1e-3 && (p.roll - tr).abs() < 1e-3, "leans to match the slope");
    }
}
