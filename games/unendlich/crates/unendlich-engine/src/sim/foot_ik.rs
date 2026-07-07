//! foot_ik — per-leg foot-lock inverse kinematics. A two-bone leg
//! (hip → knee → foot) solved analytically (law of cosines) so the foot plants
//! exactly on a contact target while the body translates over it. That is what
//! kills the foot-skate/float noted in `docs/body_physics.md`: hold the world
//! contact `target` fixed and re-solve each frame as the `hip` moves — the foot
//! stays locked to the ground while the hip glides. If the target is out of reach
//! the leg extends straight toward it (no snap, no NaN). Pure, std-only,
//! stateless, deterministic.

/// Result of one leg solve.
#[derive(Clone, Copy, Debug)]
pub struct LegSolve {
    /// World position of the knee joint.
    pub knee: [f32; 3],
    /// World position of the foot (equals `target` when reachable).
    pub foot: [f32; 3],
    /// Angle at the hip between the thigh and the hip→target line (radians).
    pub hip_angle: f32,
    /// Interior angle at the knee (radians); near `PI` when the leg is straight.
    pub knee_angle: f32,
    /// True if the target was within reach (then `foot == target`).
    pub reachable: bool,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

/// Solve a hip→knee→foot leg of bone lengths `thigh` and `shin` to plant the foot
/// at `target`. The knee bends in the plane spanned by the hip→target line and the
/// `pole` hint (the direction the knee should point — typically the body's
/// forward). Beyond reach, the leg fully extends toward the target
/// (`reachable == false`, foot short of target). Never returns NaN/Inf for finite
/// inputs.
pub fn solve_leg(hip: [f32; 3], thigh: f32, shin: f32, target: [f32; 3], pole: [f32; 3]) -> LegSolve {
    let l1 = thigh.abs().max(1e-4);
    let l2 = shin.abs().max(1e-4);
    let axis = sub(target, hip);
    let d_raw = length(axis);
    // Unit vector hip→target; guard the degenerate zero-length case.
    let forward = if d_raw > 1e-5 { scale(axis, 1.0 / d_raw) } else { [0.0, -1.0, 0.0] };
    let reach = l1 + l2;
    let reachable = d_raw <= reach;
    // Distance used for the cosine solve, clamped to the valid triangle range so
    // acos never sees an out-of-range argument.
    let dmin = (l1 - l2).abs() + 1e-4;
    let d = d_raw.clamp(dmin, reach);

    // Knee bend direction: the part of `pole` perpendicular to `forward`.
    let pdot = dot(pole, forward);
    let perp_raw = sub(pole, scale(forward, pdot));
    let perp_len = length(perp_raw);
    let perp = if perp_len > 1e-5 {
        scale(perp_raw, 1.0 / perp_len)
    } else {
        // `pole` parallel to `forward` — choose any stable perpendicular.
        let alt = if forward[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
        let a2 = sub(alt, scale(forward, dot(alt, forward)));
        let a2_len = length(a2).max(1e-5);
        scale(a2, 1.0 / a2_len)
    };

    // Law of cosines at the hip.
    let cos_hip = ((l1 * l1 + d * d - l2 * l2) / (2.0 * l1 * d)).clamp(-1.0, 1.0);
    let sin_hip = (1.0 - cos_hip * cos_hip).max(0.0).sqrt();
    let hip_angle = cos_hip.acos();
    let knee = add(hip, add(scale(forward, l1 * cos_hip), scale(perp, l1 * sin_hip)));

    let foot = if reachable { target } else { add(hip, scale(forward, reach)) };

    // Interior angle at the knee.
    let cos_knee = ((l1 * l1 + l2 * l2 - d * d) / (2.0 * l1 * l2)).clamp(-1.0, 1.0);
    let knee_angle = cos_knee.acos();

    LegSolve { knee, foot, hip_angle, knee_angle, reachable }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }
    fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
        length(sub(a, b))
    }
    fn finite(v: [f32; 3]) -> bool {
        v[0].is_finite() && v[1].is_finite() && v[2].is_finite()
    }

    #[test]
    fn reachable_target_is_planted_and_bones_keep_length() {
        let hip = [0.0, 2.0, 0.0];
        let target = [0.4, 0.6, 0.2]; // distance ~1.47 from hip, within reach 2.0
        let s = solve_leg(hip, 1.0, 1.0, target, [0.0, 0.0, 1.0]);
        assert!(s.reachable, "should be reachable");
        assert!(dist(s.foot, target) < 1e-3, "foot planted on target");
        assert!((dist(hip, s.knee) - 1.0).abs() < 1e-3, "thigh length preserved");
        assert!((dist(s.knee, s.foot) - 1.0).abs() < 1e-3, "shin length preserved");
    }

    #[test]
    fn out_of_reach_extends_straight_without_nan() {
        let hip = [0.0, 5.0, 0.0];
        let target = [10.0, 5.0, 0.0]; // far beyond reach 2.0
        let s = solve_leg(hip, 1.0, 1.0, target, [0.0, 0.0, 1.0]);
        assert!(!s.reachable, "beyond reach");
        assert!(finite(s.knee) && finite(s.foot), "no NaN");
        // foot extends to the reach distance toward the target
        assert!((dist(hip, s.foot) - 2.0).abs() < 1e-3, "fully extended to reach");
        assert!(s.knee_angle > 3.0, "knee nearly straight (~PI): {}", s.knee_angle);
    }

    #[test]
    fn small_target_move_gives_small_joint_move_continuity() {
        let hip = [0.0, 2.0, 0.0];
        let pole = [0.0, 0.0, 1.0];
        let t1 = [0.5, 0.2, 0.2];
        let t2 = [0.51, 0.2, 0.2]; // nudge by 0.01
        let a = solve_leg(hip, 1.0, 1.0, t1, pole);
        let b = solve_leg(hip, 1.0, 1.0, t2, pole);
        assert!(dist(a.foot, b.foot) < 0.05, "foot moves a little: {}", dist(a.foot, b.foot));
        assert!(dist(a.knee, b.knee) < 0.2, "knee moves a little: {}", dist(a.knee, b.knee));
    }

    #[test]
    fn foot_locks_while_the_hip_translates() {
        // Same world target, hip slides sideways frame to frame: foot stays put.
        let pole = [0.0, 0.0, 1.0];
        let target = [0.4, 0.0, 0.1];
        let s0 = solve_leg([0.0, 1.8, 0.0], 1.0, 1.0, target, pole);
        let s1 = solve_leg([0.2, 1.8, 0.0], 1.0, 1.0, target, pole);
        assert!(dist(s0.foot, s1.foot) < 1e-3, "foot stays locked to the contact");
    }

    #[test]
    fn prop_solver_stays_finite_for_any_input() {
        let mut seed = 0x0BAD_F00Du32;
        for _ in 0..6000 {
            let hip = [lcg(&mut seed) * 20.0 - 10.0, lcg(&mut seed) * 20.0 - 10.0, lcg(&mut seed) * 20.0 - 10.0];
            let target = [lcg(&mut seed) * 20.0 - 10.0, lcg(&mut seed) * 20.0 - 10.0, lcg(&mut seed) * 20.0 - 10.0];
            let pole = [lcg(&mut seed) * 2.0 - 1.0, lcg(&mut seed) * 2.0 - 1.0, lcg(&mut seed) * 2.0 - 1.0];
            let thigh = lcg(&mut seed) * 3.0;
            let shin = lcg(&mut seed) * 3.0;
            let s = solve_leg(hip, thigh, shin, target, pole);
            assert!(finite(s.knee) && finite(s.foot), "positions finite");
            assert!(s.hip_angle.is_finite() && s.knee_angle.is_finite(), "angles finite");
        }
    }
}
