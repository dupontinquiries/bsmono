//! `sim::movement_tune` — player jump + camera smoothing.
//!
//! Jump launch speed for a target apex (plus a slight boost and apex float), and
//! frame-rate-independent smoothing for the camera (position + look angle).
//! Pure math, no IO, no clock — `dt` is a parameter. Deterministic, panic-free.

use std::f32::consts::PI;

/// Launch velocity to reach `target_height` under gravity `g` (g>0, downward):
/// v = sqrt(2·g·h). Non-positive inputs → 0.
pub fn jump_launch_speed(target_height: f32, g: f32) -> f32 {
    if target_height <= 0.0 || g <= 0.0 {
        0.0
    } else {
        (2.0 * g * target_height).sqrt()
    }
}

/// `jump_launch_speed` with a slight boost (up to +15% at boost01=1) for a
/// livelier takeoff. Monotone in `boost01`, always ≥ the base speed.
pub fn jump_launch_speed_boosted(target_height: f32, g: f32, boost01: f32) -> f32 {
    let b = boost01.clamp(0.0, 1.0);
    jump_launch_speed(target_height, g) * (1.0 + 0.15 * b)
}

/// Reduced gravity near the apex (small |vy|) so the jump floats a touch. Returns
/// the gravity to apply: in [g·(1−float01), g]; equals `g` away from the apex.
pub fn apex_gravity(g: f32, vy: f32, float01: f32) -> f32 {
    let f = float01.clamp(0.0, 1.0);
    const APEX_BAND: f32 = 3.0; // m/s window around the apex
    let t = (vy.abs() / APEX_BAND).clamp(0.0, 1.0); // 0 at apex, 1 away
    g * (1.0 - f * (1.0 - t))
}

fn decay(half_life: f32, dt: f32) -> f32 {
    if half_life <= 0.0 {
        0.0 // snap to target
    } else {
        0.5f32.powf(dt.max(0.0) / half_life)
    }
}

/// Critically-damped smoothing toward `target`. `half_life` = seconds to close
/// half the gap; frame-rate independent, never overshoots, dt=0 → unchanged.
pub fn smooth_damp(current: f32, target: f32, half_life: f32, dt: f32) -> f32 {
    let k = decay(half_life, dt);
    target + (current - target) * k
}

/// Per-component [`smooth_damp`].
pub fn smooth_damp3(current: [f32; 3], target: [f32; 3], half_life: f32, dt: f32) -> [f32; 3] {
    let k = decay(half_life, dt);
    [
        target[0] + (current[0] - target[0]) * k,
        target[1] + (current[1] - target[1]) * k,
        target[2] + (current[2] - target[2]) * k,
    ]
}

fn wrap_pi(a: f32) -> f32 {
    let mut x = (a + PI).rem_euclid(2.0 * PI) - PI;
    if x <= -PI {
        x += 2.0 * PI;
    }
    x
}

/// Smooth an angle toward a target along the shortest arc (no 359°→0° snap).
pub fn smooth_angle(current: f32, target: f32, half_life: f32, dt: f32) -> f32 {
    let diff = wrap_pi(target - current);
    let k = decay(half_life, dt);
    wrap_pi(current + diff * (1.0 - k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn apex_height_matches_target() {
        let g = 20.0;
        for h in [1.0, 2.5, 4.0] {
            let v = jump_launch_speed(h, g);
            let apex = v * v / (2.0 * g); // h = v²/2g
            assert!((apex - h).abs() < 1e-3, "apex {apex} != target {h}");
        }
        assert_eq!(jump_launch_speed(-1.0, g), 0.0);
        assert_eq!(jump_launch_speed(1.0, 0.0), 0.0);
    }

    #[test]
    fn boost_monotone_and_at_least_base() {
        let (h, g) = (2.0, 20.0);
        let base = jump_launch_speed(h, g);
        assert!(jump_launch_speed_boosted(h, g, 0.0) >= base - 1e-6);
        assert!(jump_launch_speed_boosted(h, g, 1.0) > jump_launch_speed_boosted(h, g, 0.5));
        assert!(jump_launch_speed_boosted(h, g, 0.5) > base);
    }

    #[test]
    fn apex_gravity_bounds_and_offapex_full() {
        let g = 20.0;
        let at_apex = apex_gravity(g, 0.0, 1.0);
        assert!((at_apex - 0.0).abs() < 1e-4, "full float at apex => ~0 gravity");
        assert!((apex_gravity(g, 10.0, 1.0) - g).abs() < 1e-4, "full gravity away from apex");
        for vy in [-5.0, -1.0, 0.0, 1.0, 5.0] {
            let ge = apex_gravity(g, vy, 0.5);
            assert!(ge >= g * 0.5 - 1e-4 && ge <= g + 1e-4);
        }
    }

    #[test]
    fn smooth_damp_converges_no_overshoot_framerate_independent() {
        // converges toward target, no overshoot
        let mut x = 0.0;
        for _ in 0..200 {
            x = smooth_damp(x, 10.0, 0.2, 1.0 / 60.0);
            assert!(x <= 10.0 + 1e-4, "no overshoot");
        }
        assert!((x - 10.0).abs() < 0.1, "converges");
        // dt = 0 leaves it unchanged
        assert_eq!(smooth_damp(3.0, 9.0, 0.2, 0.0), 3.0);
        // frame-rate independence: one 2·dt step == two dt steps
        let single = smooth_damp(0.0, 1.0, 0.25, 0.2);
        let twice = smooth_damp(smooth_damp(0.0, 1.0, 0.25, 0.1), 1.0, 0.25, 0.1);
        assert!((single - twice).abs() < 1e-5, "frame-rate independent");
    }

    #[test]
    fn smooth_angle_short_arc() {
        // 170° -> -170° should go the short way (through 180°), ending near -170°
        let cur = 170f32.to_radians();
        let tgt = (-170f32).to_radians();
        let next = smooth_angle(cur, tgt, 0.1, 1.0 / 60.0);
        // moved toward +180 side, i.e. angle increased (wrapped), not swung down through 0
        let step = wrap_pi(next - cur);
        assert!(step > 0.0, "took the short +arc across the seam");
        assert!(step.abs() < 0.5, "small step");
    }

    #[test]
    fn smooth_damp3_and_determinism() {
        let a = smooth_damp3([0.0; 3], [1.0, 2.0, 3.0], 0.2, 0.1);
        let b = smooth_damp3([0.0; 3], [1.0, 2.0, 3.0], 0.2, 0.1);
        assert_eq!(a, b);
        assert!(a.iter().zip([1.0, 2.0, 3.0]).all(|(v, t)| *v >= 0.0 && *v <= t));
        let _ = FRAC_PI_2;
    }
}
