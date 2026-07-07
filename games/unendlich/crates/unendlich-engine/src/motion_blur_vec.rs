//! motion_blur_vec.rs — pure screen-space velocity for motion blur.
//!
//! Per-pixel/object velocity = `(cur - prev) * scale`, clamped per component so a
//! single fast-moving object can't smear across the whole screen. Pure,
//! deterministic, panic-free (R1/R11/R21).

/// Max per-component velocity (clip-space units) to avoid runaway smearing.
pub const MAX_VEL: f32 = 1.0;

/// Screen-space velocity from previous and current clip-space positions, scaled
/// by `scale` and clamped to `±MAX_VEL` per component.
pub fn velocity(prev_clip: [f32; 2], cur_clip: [f32; 2], scale: f32) -> [f32; 2] {
    let vx = (cur_clip[0] - prev_clip[0]) * scale;
    let vy = (cur_clip[1] - prev_clip[1]) * scale;
    [vx.max(-MAX_VEL).min(MAX_VEL), vy.max(-MAX_VEL).min(MAX_VEL)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_is_zero() {
        assert_eq!(velocity([0.3, 0.4], [0.3, 0.4], 1.0), [0.0, 0.0]);
    }

    #[test]
    fn moving_is_nonzero() {
        let v = velocity([0.0, 0.0], [0.1, -0.2], 1.0);
        assert!(v[0] != 0.0 && v[1] != 0.0);
    }

    #[test]
    fn scale_scales() {
        // small displacement so the clamp doesn't engage
        let a = velocity([0.0, 0.0], [0.1, 0.05], 1.0);
        let b = velocity([0.0, 0.0], [0.1, 0.05], 2.0);
        assert!((b[0] - 2.0 * a[0]).abs() < 1e-6 && (b[1] - 2.0 * a[1]).abs() < 1e-6);
    }

    #[test]
    fn bounded_clamp() {
        let v = velocity([0.0, 0.0], [100.0, -100.0], 5.0);
        assert!(v[0].abs() <= MAX_VEL && v[1].abs() <= MAX_VEL, "velocity not clamped: {:?}", v);
        assert_eq!(v, [MAX_VEL, -MAX_VEL]);
    }

    #[test]
    fn symmetric_under_reversal() {
        // reversing motion negates the velocity (within the clamp band)
        let a = velocity([0.1, 0.2], [0.3, 0.05], 1.0);
        let b = velocity([0.3, 0.05], [0.1, 0.2], 1.0);
        assert!((a[0] + b[0]).abs() < 1e-6 && (a[1] + b[1]).abs() < 1e-6);
    }

    #[test]
    fn deterministic() {
        assert_eq!(velocity([0.1, 0.2], [0.3, 0.4], 1.5), velocity([0.1, 0.2], [0.3, 0.4], 1.5));
    }
}
