//! strafe_face — keep sideways motion from reading as a moonwalk. Given the
//! direction a creature is moving and the direction it wants to face, this returns
//! the body yaw to use plus a lateral lean (roll) banked into the strafe, like a
//! skater leaning through a sideways push. When facing already matches movement
//! there is no lean and the body just faces that way. OPTIONAL task module — the
//! coordinator may prune it if it overlaps `head_steer`/`turning`. Pure, std-only,
//! deterministic.

use std::f32::consts::PI;

/// Wrap an angle to `(-PI, PI]`.
fn wrap_pi(a: f32) -> f32 {
    let two_pi = 2.0 * PI;
    let mut x = a % two_pi;
    if x > PI {
        x -= two_pi;
    }
    if x < -PI {
        x += two_pi;
    }
    x
}

/// Tuning for the strafe solver.
#[derive(Clone, Copy, Debug)]
pub struct StrafeParams {
    /// Maximum lateral lean (radians).
    pub max_lean: f32,
    /// Fraction of the move/face mismatch the body yaw commits toward movement
    /// (0 = face stays put, 1 = body fully turns to the move direction).
    pub commit: f32,
}

impl Default for StrafeParams {
    fn default() -> StrafeParams {
        StrafeParams { max_lean: 0.35, commit: 0.25 }
    }
}

/// Returns `(body_yaw, lean_roll)` in radians. `move_yaw` / `face_yaw` are world
/// headings; `speed01` in `[0, 1]` scales the lean. The body turns a fraction
/// `commit` from the desired facing toward the movement (so a hard strafe shows a
/// little shoulder), and `lean_roll` banks into the sideways component. When
/// `face_yaw == move_yaw` the result is that heading with zero lean.
pub fn solve(move_yaw: f32, face_yaw: f32, speed01: f32, p: &StrafeParams) -> (f32, f32) {
    let rel = wrap_pi(move_yaw - face_yaw);
    let s = speed01.clamp(0.0, 1.0);
    let commit = p.commit.clamp(0.0, 1.0);
    let body_yaw = wrap_pi(face_yaw + rel * commit);
    let lean = (rel.sin() * s * p.max_lean).clamp(-p.max_lean, p.max_lean);
    (body_yaw, lean)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }

    #[test]
    fn facing_the_way_you_move_means_no_lean() {
        let p = StrafeParams::default();
        let (body, lean) = solve(1.2, 1.2, 1.0, &p);
        assert!((lean).abs() < 1e-6, "no sideways component, no lean");
        assert!((wrap_pi(body - 1.2)).abs() < 1e-6, "body faces the shared heading");
    }

    #[test]
    fn a_strafe_leans_into_the_motion_and_is_signed() {
        let p = StrafeParams::default();
        // moving 90° to the left of facing
        let (_, lean_left) = solve(PI / 2.0, 0.0, 1.0, &p);
        let (_, lean_right) = solve(-PI / 2.0, 0.0, 1.0, &p);
        assert!(lean_left.abs() > 0.0, "a strafe leans");
        assert!((lean_left + lean_right).abs() < 1e-6, "left/right lean is mirrored");
    }

    #[test]
    fn slower_strafe_leans_less() {
        let p = StrafeParams::default();
        let (_, fast) = solve(PI / 2.0, 0.0, 1.0, &p);
        let (_, slow) = solve(PI / 2.0, 0.0, 0.2, &p);
        assert!(slow.abs() < fast.abs(), "lean scales with speed");
    }

    #[test]
    fn prop_lean_bounded_and_finite() {
        let p = StrafeParams::default();
        let mut seed = 0x5151_2727u32;
        for _ in 0..5000 {
            let mv = lcg(&mut seed) * 12.0 - 6.0;
            let fc = lcg(&mut seed) * 12.0 - 6.0;
            let sp = lcg(&mut seed) * 2.0 - 0.5;
            let (body, lean) = solve(mv, fc, sp, &p);
            assert!(body.is_finite() && lean.is_finite(), "finite outputs");
            assert!(lean.abs() <= p.max_lean + 1e-6, "lean bounded: {lean}");
            assert!(body > -PI - 1e-4 && body <= PI + 1e-4, "body yaw wrapped: {body}");
        }
    }
}
