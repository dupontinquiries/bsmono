//! held_item_pose.rs — pure held-item transform by item class.
//!
//! Maps an item class (and a scale) to a first-/third-person hand pose: an
//! offset from the hand anchor plus yaw/pitch. Distinct classes get distinct
//! poses; unknown classes fall back to a generic pose. Pure, deterministic,
//! panic-free (R1/R11/R21).

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Pose {
    pub offset: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

/// Number of known item classes (0..NUM_CLASSES). Out-of-range classes use the
/// fallback pose.
pub const NUM_CLASSES: u8 = 6;

fn base_pose(class: u8) -> ([f32; 3], f32, f32) {
    match class {
        0 => ([0.30, -0.20, 0.45], 0.10, -0.25), // sword
        1 => ([0.28, -0.18, 0.40], 0.05, -0.15), // tool/pick
        2 => ([0.20, -0.10, 0.55], 0.30, 0.00),  // bow
        3 => ([0.15, -0.05, 0.30], 0.00, -0.40), // food
        4 => ([0.35, -0.25, 0.32], -0.20, 0.00), // shield
        5 => ([0.25, -0.15, 0.50], 0.15, -0.10), // rod/staff
        _ => ([0.24, -0.14, 0.38], 0.00, -0.20), // fallback (generic)
    }
}

/// Hand pose for `class` at the given `scale` (scales the offset; angles are
/// scale-independent). Angles are bounded to `[-π, π]`.
pub fn held_pose(class: u8, scale: f32) -> Pose {
    let (off, yaw, pitch) = base_pose(class);
    Pose {
        offset: [off[0] * scale, off[1] * scale, off[2] * scale],
        yaw,
        pitch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn distinct_classes_distinct_poses() {
        let poses: Vec<Pose> = (0..NUM_CLASSES).map(|c| held_pose(c, 1.0)).collect();
        for i in 0..poses.len() {
            for j in (i + 1)..poses.len() {
                assert!(poses[i] != poses[j], "classes {} and {} share a pose", i, j);
            }
        }
    }

    #[test]
    fn scale_scales_offset() {
        for c in 0..NUM_CLASSES {
            let a = held_pose(c, 1.0);
            let b = held_pose(c, 2.0);
            for k in 0..3 {
                assert!((b.offset[k] - 2.0 * a.offset[k]).abs() < 1e-6, "offset not scaled");
            }
            // angles unaffected by scale
            assert_eq!(a.yaw, b.yaw);
            assert_eq!(a.pitch, b.pitch);
        }
    }

    #[test]
    fn angles_bounded() {
        for c in 0..=255u8 {
            let p = held_pose(c, 1.0);
            assert!(p.yaw.abs() <= PI && p.pitch.abs() <= PI, "angle out of bounds for class {}", c);
        }
    }

    #[test]
    fn class_fallback_for_unknown() {
        let fb = held_pose(NUM_CLASSES, 1.0);
        assert_eq!(held_pose(200, 1.0), fb, "all unknown classes share fallback");
        assert_eq!(held_pose(99, 1.0), fb);
        // fallback differs from every known class
        for c in 0..NUM_CLASSES {
            assert!(held_pose(c, 1.0) != fb, "known class {} collides with fallback", c);
        }
    }

    #[test]
    fn stable_across_calls() {
        assert_eq!(held_pose(2, 1.5), held_pose(2, 1.5));
    }

    #[test]
    fn deterministic() {
        assert_eq!(held_pose(0, 1.0), held_pose(0, 1.0));
    }
}
