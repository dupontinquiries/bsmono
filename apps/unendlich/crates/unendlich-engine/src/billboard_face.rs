//! billboard_face.rs — pure yaw to face the camera (particles / impostors).
//!
//! Given the horizontal vector from a billboard to the camera, returns the yaw
//! (radians) that orients the billboard to face it. Pure, deterministic,
//! panic-free (R1/R11/R21). Convention: +X → 0, +Z → +π/2 (`atan2(z, x)`).

/// Facing yaw for `to_cam = [x, z]`. Range `(-π, π]`. A zero vector returns `0`.
pub fn face_yaw(to_cam: [f32; 2]) -> f32 {
    // atan2(0,0) is defined as 0 in Rust, so the zero vector is safe.
    to_cam[1].atan2(to_cam[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn plus_x_is_zero() {
        assert!(face_yaw([1.0, 0.0]).abs() < 1e-6);
        assert!(face_yaw([5.0, 0.0]).abs() < 1e-6);
    }

    #[test]
    fn plus_z_is_half_pi() {
        assert!((face_yaw([0.0, 1.0]) - PI / 2.0).abs() < 1e-6);
    }

    #[test]
    fn zero_vector_safe() {
        let y = face_yaw([0.0, 0.0]);
        assert!(y.is_finite());
        assert_eq!(y, 0.0);
    }

    #[test]
    fn range_minus_pi_to_pi() {
        let mut seed = 0x1827_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..5000 {
            let y = face_yaw([lcg() * 2.0 - 1.0, lcg() * 2.0 - 1.0]);
            assert!(y > -PI - 1e-6 && y <= PI + 1e-6, "yaw out of range: {}", y);
        }
    }

    #[test]
    fn continuous_away_from_branch_cut() {
        // sweep the direction angle, avoiding the -X wrap; small step => small change
        let step = 0.005f32;
        let mut theta = -PI + 0.2;
        let mut prev = face_yaw([theta.cos(), theta.sin()]);
        while theta <= PI - 0.2 {
            let cur = face_yaw([theta.cos(), theta.sin()]);
            assert!((cur - prev).abs() <= step * 2.0 + 1e-4, "discontinuity at {}", theta);
            prev = cur;
            theta += step;
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(face_yaw([0.3, -0.7]), face_yaw([0.3, -0.7]));
    }
}
