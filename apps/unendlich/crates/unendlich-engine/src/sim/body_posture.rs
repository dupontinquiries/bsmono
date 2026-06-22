//! Body posture — tilt lerping and torso warping.
//!
//! A creature's pitch/roll should ease toward the slope its feet stand on
//! rather than snapping, and its torso should warp (bend) into turns and across
//! slopes. This core computes the *target* posture from the four foot heights
//! and turn rate, and a frame-rate-independent smoothing toward it.
//!
//! Pure and deterministic.

/// Pitch (nose up/down), roll (bank), and a torso bend (lateral S-curve of the
/// spine, signed: + bends toward +Z / right).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Posture {
    pub pitch: f32,
    pub roll: f32,
    pub torso_bend: f32,
}

/// Exponential (critically-damped-feeling) smoothing of `cur` toward `target`.
/// `rate` is the responsiveness per second; `dt` the frame time in seconds.
/// `dt <= 0` returns `cur` unchanged; large `rate*dt` snaps to `target`.
pub fn settle(cur: Posture, target: Posture, rate: f32, dt: f32) -> Posture {
    if dt <= 0.0 || rate <= 0.0 {
        return cur;
    }
    let k = 1.0 - (-rate * dt).exp(); // 0..1
    Posture {
        pitch: cur.pitch + (target.pitch - cur.pitch) * k,
        roll: cur.roll + (target.roll - cur.roll) * k,
        torso_bend: cur.torso_bend + (target.torso_bend - cur.torso_bend) * k,
    }
}

/// Target posture from the stance.
///
/// * `foot_h` — ground heights at the four feet: `[FL, FR, RL, RR]`.
/// * `track_width` — left↔right foot spacing (for roll).
/// * `wheelbase` — front↔rear foot spacing (for pitch).
/// * `turn_rate` — yaw rate (rad/s); the torso warps into the turn.
///
/// Pitch tips the nose down when the front feet sit lower than the rear; roll
/// banks toward the lower side; torso_bend curves the spine into the turn and
/// adds a little with cross-slope.
pub fn target_from_feet(foot_h: [f32; 4], track_width: f32, wheelbase: f32, turn_rate: f32) -> Posture {
    let front = 0.5 * (foot_h[0] + foot_h[1]);
    let rear = 0.5 * (foot_h[2] + foot_h[3]);
    let left = 0.5 * (foot_h[0] + foot_h[2]);
    let right = 0.5 * (foot_h[1] + foot_h[3]);

    let wb = wheelbase.max(1e-3);
    let tw = track_width.max(1e-3);

    // Nose up when the front is higher than the rear.
    let pitch = (front - rear).atan2(wb);
    // Bank toward the lower side (roll right-positive when right is lower).
    let roll = (left - right).atan2(tw);

    // Torso bends into the turn (clamped) plus a touch from the cross-slope.
    let cross = (left - right) / tw;
    let torso_bend = (turn_rate * 0.15 + cross * 0.25).clamp(-0.6, 0.6);

    Posture { pitch, roll, torso_bend }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_ground_is_level() {
        let p = target_from_feet([4.0; 4], 2.0, 3.0, 0.0);
        assert!(p.pitch.abs() < 1e-6 && p.roll.abs() < 1e-6 && p.torso_bend.abs() < 1e-6);
    }

    #[test]
    fn front_higher_pitches_nose_up() {
        let p = target_from_feet([5.0, 5.0, 4.0, 4.0], 2.0, 3.0, 0.0);
        assert!(p.pitch > 0.0, "front higher than rear → nose up");
    }

    #[test]
    fn lower_right_banks_right() {
        // Right feet lower than left → roll positive (toward +Z/right).
        let p = target_from_feet([5.0, 4.0, 5.0, 4.0], 2.0, 3.0, 0.0);
        assert!(p.roll > 0.0);
    }

    #[test]
    fn turning_warps_the_torso() {
        let straight = target_from_feet([4.0; 4], 2.0, 3.0, 0.0);
        let turning = target_from_feet([4.0; 4], 2.0, 3.0, 2.0);
        assert!(turning.torso_bend.abs() > straight.torso_bend.abs());
        assert!(turning.torso_bend.abs() <= 0.6);
    }

    #[test]
    fn settle_eases_toward_target_and_clamps_dt() {
        let cur = Posture::default();
        let target = Posture { pitch: 1.0, roll: -0.5, torso_bend: 0.2 };
        let once = settle(cur, target, 8.0, 0.1);
        assert!(once.pitch > 0.0 && once.pitch < 1.0, "moves partway");
        assert_eq!(settle(cur, target, 8.0, 0.0), cur, "dt=0 is a no-op");
        // Many steps converge.
        let mut s = cur;
        for _ in 0..200 {
            s = settle(s, target, 8.0, 0.05);
        }
        assert!((s.pitch - 1.0).abs() < 1e-3 && (s.roll + 0.5).abs() < 1e-3);
    }

    #[test]
    fn deterministic() {
        let a = target_from_feet([5.0, 4.5, 4.0, 4.2], 2.1, 3.3, 1.1);
        let b = target_from_feet([5.0, 4.5, 4.0, 4.2], 2.1, 3.3, 1.1);
        assert_eq!(a, b);
    }
}
