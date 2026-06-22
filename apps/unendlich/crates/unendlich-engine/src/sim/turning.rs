//! turning — smooth, momentum-eased yaw for mobs.
//!
//! [`crate::sim::awareness::turn_toward`] steps at a constant capped rate — fine
//! for snappy alignment, but turn-*state* changes (idle→pursue, pursue→flee) look
//! mechanical because the angular velocity jumps. [`ease_yaw`] runs a near-
//! critically-damped angular spring instead, so a turn accelerates out of rest,
//! carries momentum through the middle, and decelerates into the target without
//! the constant-velocity snap. Pure and deterministic; the bin holds a `vel`
//! alongside each mob's yaw and calls this each tick.

/// Wrap an angle to `(-π, π]`.
fn wrap_pi(a: f32) -> f32 {
    let twopi = std::f32::consts::TAU;
    let mut a = a % twopi;
    if a > std::f32::consts::PI {
        a -= twopi;
    } else if a <= -std::f32::consts::PI {
        a += twopi;
    }
    a
}

/// Default angular spring constants (rad/s² per rad, and damping). Tuned near
/// critical so a turn eases in and out without ringing.
pub const TURN_STIFFNESS: f32 = 40.0;
pub const TURN_DAMPING: f32 = 12.0;

/// One step of a damped angular spring easing `yaw` toward `target` (taking the
/// shortest way around the circle), carrying angular velocity `vel`. Returns the
/// new `(yaw, vel)`. An optional `max_rate` clamps the angular speed so even a
/// half-turn can't whip around faster than a mob's neck/body allows; pass
/// `f32::INFINITY` for no cap.
pub fn ease_yaw(
    yaw: f32,
    target: f32,
    vel: f32,
    dt: f32,
    stiffness: f32,
    damping: f32,
    max_rate: f32,
) -> (f32, f32) {
    let err = wrap_pi(target - yaw);
    let accel = stiffness * err - damping * vel;
    let mut v = vel + accel * dt;
    if v.abs() > max_rate {
        v = max_rate * v.signum();
    }
    let y = wrap_pi(yaw + v * dt);
    (y, v)
}

/// Convenience: ease with the default constants and no rate cap.
pub fn ease_yaw_default(yaw: f32, target: f32, vel: f32, dt: f32) -> (f32, f32) {
    ease_yaw(yaw, target, vel, dt, TURN_STIFFNESS, TURN_DAMPING, f32::INFINITY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn run_to_rest(mut yaw: f32, target: f32, max_rate: f32) -> (f32, usize) {
        let mut vel = 0.0;
        for i in 0..2000 {
            let (y, v) = ease_yaw(yaw, target, vel, 1.0 / 120.0, TURN_STIFFNESS, TURN_DAMPING, max_rate);
            yaw = y;
            vel = v;
            if wrap_pi(target - yaw).abs() < 1e-3 && vel.abs() < 1e-2 {
                return (yaw, i);
            }
        }
        (yaw, usize::MAX)
    }

    #[test]
    fn eases_to_the_target() {
        let (yaw, n) = run_to_rest(0.0, 1.0, f32::INFINITY);
        assert!(wrap_pi(1.0 - yaw).abs() < 1e-2, "should reach target, got {yaw}");
        assert!(n < usize::MAX, "should settle");
    }

    #[test]
    fn turn_accelerates_from_rest() {
        // First step from rest is small (eased in), not an instant max-rate jump.
        let (_y, v1) = ease_yaw(0.0, 1.5, 0.0, 1.0 / 120.0, TURN_STIFFNESS, TURN_DAMPING, f32::INFINITY);
        // A few steps later it's moving faster (momentum built).
        let mut yaw = 0.0;
        let mut vel = 0.0;
        for _ in 0..15 {
            let (y, v) = ease_yaw(yaw, 1.5, vel, 1.0 / 120.0, TURN_STIFFNESS, TURN_DAMPING, f32::INFINITY);
            yaw = y;
            vel = v;
        }
        assert!(vel.abs() > v1.abs(), "should accelerate out of rest: {} -> {}", v1, vel);
    }

    #[test]
    fn takes_the_short_way_around_the_wrap() {
        // From ~+π to ~-π should cross through π, not unwind all the way back.
        let start = 3.0;
        let target = -3.0; // shortest path is +0.28 through π, not -6.0
        let mut yaw = start;
        let mut vel = 0.0;
        let mut max_excursion: f32 = 0.0;
        for _ in 0..400 {
            let (y, v) = ease_yaw(yaw, target, vel, 1.0 / 120.0, TURN_STIFFNESS, TURN_DAMPING, f32::INFINITY);
            yaw = y;
            vel = v;
            max_excursion = max_excursion.max(yaw.abs());
        }
        assert!(wrap_pi(target - yaw).abs() < 1e-2, "should reach target");
        // It should not have swung down toward 0 (the long way).
        assert!(max_excursion > 3.0, "should go over the top, not unwind: {max_excursion}");
    }

    #[test]
    fn rate_cap_limits_angular_speed() {
        let cap = 1.0; // rad/s
        let mut yaw = 0.0;
        let mut vel = 0.0;
        let mut peak: f32 = 0.0;
        for _ in 0..200 {
            let (y, v) = ease_yaw(yaw, PI, vel, 1.0 / 120.0, TURN_STIFFNESS, TURN_DAMPING, cap);
            yaw = y;
            vel = v;
            peak = peak.max(v.abs());
        }
        assert!(peak <= cap + 1e-3, "angular speed must respect the cap, peaked {peak}");
    }

    #[test]
    fn larger_turns_reach_higher_mid_speed() {
        let mut small_peak: f32 = 0.0;
        let mut big_peak: f32 = 0.0;
        for (target, peak) in [(0.3_f32, &mut small_peak), (2.5_f32, &mut big_peak)] {
            let mut yaw = 0.0;
            let mut vel = 0.0;
            for _ in 0..200 {
                let (y, v) = ease_yaw(yaw, target, vel, 1.0 / 120.0, TURN_STIFFNESS, TURN_DAMPING, f32::INFINITY);
                yaw = y;
                vel = v;
                *peak = peak.max(v.abs());
            }
        }
        assert!(big_peak > small_peak, "a bigger turn should build more speed");
    }
}
