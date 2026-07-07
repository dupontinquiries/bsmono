//! impostor_card.rs — pure billboard impostor selection.
//!
//! Picks which pre-captured angle to use for a billboard impostor given the view
//! yaw, exploiting left/right symmetry: captures span only the half-circle
//! `[0, π]`, and the back half is covered by horizontally flipping a front
//! capture — so `n_angles` captures cover the full circle. Also builds the
//! camera-facing quad basis. Pairs with `silhouette_lod`. Pure, deterministic,
//! panic-free (R1/R11/R21).

use std::f32::consts::PI;

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn normalize(v: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-6 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        fallback
    }
}

/// Choose the nearest captured angle for `view_yaw` (radians, any value) given
/// `n_angles` captures spanning `[0, π]`. Returns `(index, flip)` where `flip`
/// means the capture must be mirrored horizontally (the view is in the back
/// half). `index < n_angles` always; `n_angles == 0` yields `(0, false)`.
pub fn card(view_yaw: f32, n_angles: u32) -> (u32, bool) {
    if n_angles == 0 {
        return (0, false);
    }
    let two_pi = 2.0 * PI;
    // normalize yaw into [0, 2π)
    let mut y = view_yaw % two_pi;
    if y < 0.0 {
        y += two_pi;
    }
    // back half -> flip and reflect into [0, π]
    let (flip, y_eff) = if y > PI { (true, two_pi - y) } else { (false, y) };

    let idx = if n_angles == 1 {
        0
    } else {
        let step = PI / (n_angles - 1) as f32; // captures at 0, step, ..., π
        let k = (y_eff / step).round();
        let max = (n_angles - 1) as f32;
        let clamped = if k < 0.0 { 0.0 } else if k > max { max } else { k };
        clamped as u32
    };
    (idx, flip)
}

/// Camera-facing quad basis at `pos` viewed from `cam`: returns `(right, up)`,
/// both unit length and mutually perpendicular, spanning the plane facing the
/// camera. Falls back to a stable basis when the camera is directly above.
pub fn billboard_basis(cam: [f32; 3], pos: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let forward = normalize([cam[0] - pos[0], cam[1] - pos[1], cam[2] - pos[2]], [0.0, 0.0, 1.0]);
    let world_up = [0.0, 1.0, 0.0];
    let right = normalize(cross(world_up, forward), [1.0, 0.0, 0.0]);
    let up = normalize(cross(forward, right), [0.0, 1.0, 0.0]);
    (right, up)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn len(v: [f32; 3]) -> f32 {
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
    }
    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    #[test]
    fn nearest_angle_chosen() {
        // 5 captures at 0, π/4, π/2, 3π/4, π
        assert_eq!(card(0.0, 5), (0, false));
        assert_eq!(card(PI / 2.0, 5), (2, false));
        assert_eq!(card(PI, 5), (4, false));
        // slightly past π/2 still rounds to capture 2
        assert_eq!(card(PI / 2.0 + 0.05, 5).0, 2);
    }

    #[test]
    fn wraps_across_two_pi() {
        for &y in &[0.3f32, 1.1, 2.0, 3.0] {
            assert_eq!(card(y, 8), card(y + 2.0 * PI, 8));
            assert_eq!(card(y, 8), card(y - 2.0 * PI, 8));
        }
    }

    #[test]
    fn flip_halves_the_captures() {
        // a back-half view reuses the same index as its mirror, with flip set
        for &y in &[0.4f32, 0.9, 1.5, 2.5] {
            let front = card(y, 8);
            let back = card(2.0 * PI - y, 8);
            assert_eq!(front.0, back.0, "mirror should share the capture index");
            assert!(!front.1 && back.1, "back half should flip, front shouldn't");
        }
    }

    #[test]
    fn index_always_in_range() {
        let mut y = -10.0f32;
        while y < 10.0 {
            for &n in &[1u32, 2, 5, 8, 16] {
                let (idx, _) = card(y, n);
                assert!(idx < n, "idx {} >= n {} at yaw {}", idx, n, y);
            }
            y += 0.137;
        }
    }

    #[test]
    fn basis_unit_and_perpendicular() {
        let cases = [
            ([5.0f32, 0.0, 0.0], [0.0f32, 0.0, 0.0]),
            ([0.0, 3.0, 4.0], [0.0, 0.0, 0.0]),
            ([0.0, 10.0, 0.0], [0.0, 0.0, 0.0]), // camera directly above (fallback)
            ([-2.0, 1.0, 7.0], [1.0, 1.0, 1.0]),
        ];
        for (cam, pos) in cases {
            let (r, u) = billboard_basis(cam, pos);
            assert!((len(r) - 1.0).abs() < 1e-4, "right not unit: {}", len(r));
            assert!((len(u) - 1.0).abs() < 1e-4, "up not unit: {}", len(u));
            assert!(dot(r, u).abs() < 1e-4, "right·up not ~0: {}", dot(r, u));
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(card(1.234, 12), card(1.234, 12));
        assert_eq!(billboard_basis([1.0, 2.0, 3.0], [0.0, 0.0, 0.0]), billboard_basis([1.0, 2.0, 3.0], [0.0, 0.0, 0.0]));
    }
}
