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

/// Standard gravity, mirrored from [`crate::turn_g_force`] so this module stays
/// std-only and independently `rustc --test`-able (no cross-module import). Keep
/// in sync with `turn_g_force`'s divisor if it ever changes.
const G_MPS2: f32 = 9.81;

/// Maximum yaw rate (rad/s) that keeps a moving body's turn *radius* believable,
/// as a **function of forward speed** — the missing speed coupling for the
/// [`ease_yaw`] `max_rate` cap.
///
/// It's the inverse of [`crate::turn_g_force::g_force`]: a body turning at rate
/// `ω` while moving at `speed` carves radius `R = speed/ω`, pulling centripetal
/// load `g = speed²/R / 9.81 = ω·speed / 9.81`. Holding that at or below
/// `max_g` gives `ω ≤ max_g·9.81 / speed`. The result is then clamped to a
/// physical `neck_cap` (how fast the neck/body can *possibly* yaw).
///
/// Feel this produces, for free, from one number (`max_g`):
/// - **Slow / standing** (`speed → 0`): the g-limit relaxes to infinity, so the
///   cap is `neck_cap` — a creature pivots snappily in place.
/// - **Fast** (large `speed`): the g-limit bites, the cap shrinks, and the turn
///   radius `speed/ω` widens — a galloping body can't corner on a dime.
///
/// Pure and deterministic. Pass the result as `ease_yaw`'s `max_rate`.
pub fn yaw_rate_cap(speed: f32, max_g: f32, neck_cap: f32) -> f32 {
    let v = speed.abs().max(1e-3); // avoid div-by-zero; below this, neck_cap wins anyway
    let g_limited = max_g.max(0.0) * G_MPS2 / v;
    g_limited.min(neck_cap.max(0.0))
}

/// The turn radius (world units) implied by moving at `speed` and yawing at
/// `rate` — `R = speed / rate`. Handy for debug HUDs and for asserting the
/// [`yaw_rate_cap`] behaviour. Returns `f32::INFINITY` for a zero rate (straight
/// line).
pub fn turn_radius(speed: f32, rate: f32) -> f32 {
    let w = rate.abs();
    if w < 1e-6 { f32::INFINITY } else { speed.abs() / w }
}

/// One eased-yaw step whose angular-speed cap is derived from `speed` via
/// [`yaw_rate_cap`], so the same call gives a mob (or a mounted player) both
/// momentum-eased turning *and* a speed-dependent minimum turn radius. Uses the
/// default spring constants. Returns the new `(yaw, vel)`.
pub fn ease_yaw_g_limited(
    yaw: f32,
    target: f32,
    vel: f32,
    speed: f32,
    dt: f32,
    max_g: f32,
    neck_cap: f32,
) -> (f32, f32) {
    let cap = yaw_rate_cap(speed, max_g, neck_cap);
    ease_yaw(yaw, target, vel, dt, TURN_STIFFNESS, TURN_DAMPING, cap)
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

    // ── turn-radius cap (yaw_rate_cap / ease_yaw_g_limited) ──────────────────

    #[test]
    fn slow_body_pivots_at_neck_cap() {
        // Near standstill, the g-limit relaxes away and the neck cap governs.
        let neck = 3.0;
        assert!((yaw_rate_cap(0.0, 1.5, neck) - neck).abs() < 1e-6);
        assert!((yaw_rate_cap(0.05, 1.5, neck) - neck).abs() < 1e-6);
    }

    #[test]
    fn faster_means_gentler_cap_in_the_g_regime() {
        // Once the g-limit binds (below neck_cap), a higher speed lowers the cap.
        let (max_g, neck) = (0.8_f32, 100.0_f32);
        let slow = yaw_rate_cap(6.0, max_g, neck);
        let fast = yaw_rate_cap(18.0, max_g, neck);
        assert!(slow < neck && fast < neck, "both should be g-limited, not neck-limited");
        assert!(fast < slow, "faster ⇒ smaller angular cap: {slow} -> {fast}");
    }

    #[test]
    fn cap_matches_the_g_force_inverse() {
        // yaw_rate_cap is the inverse of turn_g_force: at the cap, the actual
        // centripetal g equals max_g (when g-limited, not neck-limited).
        let (speed, max_g, neck) = (12.0_f32, 0.6_f32, 100.0_f32);
        let w = yaw_rate_cap(speed, max_g, neck);
        let r = turn_radius(speed, w);
        let g = (speed * speed / r) / G_MPS2; // == turn_g_force::g_force(speed, r)
        assert!((g - max_g).abs() < 1e-3, "recovered g {g} should equal max_g {max_g}");
    }

    #[test]
    fn radius_widens_with_speed_at_a_fixed_cap() {
        // Straight radius law R = v/ω: at a common (neck) cap, faster ⇒ wider.
        let cap = 2.0;
        assert!(turn_radius(20.0, cap) > turn_radius(5.0, cap));
    }

    #[test]
    fn cap_is_nonneg_and_deterministic() {
        for &(v, g, n) in &[(0.0f32, 1.0f32, 3.0f32), (10.0, 0.5, 4.0), (30.0, 2.0, 8.0)] {
            let a = yaw_rate_cap(v, g, n);
            assert!(a >= 0.0 && a.is_finite());
            assert_eq!(a.to_bits(), yaw_rate_cap(v, g, n).to_bits());
        }
    }

    #[test]
    fn g_limited_ease_still_reaches_target_and_respects_the_cap() {
        // Compose radius cap + eased spring: it converges, and never exceeds the
        // speed-derived cap.
        let (speed, max_g, neck) = (10.0_f32, 0.7_f32, 100.0_f32);
        let cap = yaw_rate_cap(speed, max_g, neck);
        let (mut yaw, mut vel) = (0.0_f32, 0.0_f32);
        let mut peak = 0.0_f32;
        for _ in 0..2000 {
            let (y, v) = ease_yaw_g_limited(yaw, 2.0, vel, speed, 1.0 / 120.0, max_g, neck);
            yaw = y; vel = v;
            peak = peak.max(v.abs());
        }
        assert!(wrap_pi(2.0 - yaw).abs() < 1e-2, "should reach target, got {yaw}");
        assert!(peak <= cap + 1e-3, "angular speed must respect the radius cap: {peak} > {cap}");
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
