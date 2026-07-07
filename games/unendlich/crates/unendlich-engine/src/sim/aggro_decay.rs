//! `sim::aggro_decay` — a mob's aggression toward a target cools off over time,
//! and faster the farther the target gets.
//!
//! [`decay`] reduces an `aggro` value by an amount that grows with elapsed time
//! `dt` and with `dist` to the target, clamped so the result stays in
//! `[0, aggro]`. A target that runs away loses the mob's interest sooner; a `dt`
//! of `0` leaves aggro unchanged; given enough time aggro reaches `0` and the
//! mob disengages.
//!
//! Pure and deterministic: one linear decay term and a clamp — no clock, no
//! randomness, no cross-imports.

/// Base aggro lost per second (at zero distance).
pub const BASE_RATE: f32 = 0.5;
/// How much distance accelerates the decay.
pub const DIST_WEIGHT: f32 = 0.1;

fn nonneg(x: f32) -> f32 {
    if x > 0.0 {
        x
    } else {
        0.0
    }
}

/// Decay `aggro` over `dt` seconds at distance `dist`. Result is clamped to
/// `[0, aggro]` (with `aggro` floored at `0`).
pub fn decay(aggro: f32, dt: f32, dist: f32) -> f32 {
    let a = nonneg(aggro);
    let drop = BASE_RATE * nonneg(dt) * (1.0 + nonneg(dist) * DIST_WEIGHT);
    let new = a - drop;
    if new < 0.0 {
        0.0
    } else if new > a {
        a
    } else {
        new
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn decreases_over_time() {
        let a = 1.0;
        let after = decay(a, 0.5, 0.0);
        assert!(after < a && after >= 0.0);
    }

    #[test]
    fn farther_decays_faster() {
        let near = decay(1.0, 0.2, 0.0);
        let far = decay(1.0, 0.2, 20.0);
        assert!(far < near, "far {} should be below near {}", far, near);
    }

    #[test]
    fn reaches_zero() {
        // a long time, or a great distance, drives aggro to 0
        assert_eq!(decay(1.0, 100.0, 0.0), 0.0);
        assert_eq!(decay(1.0, 5.0, 1000.0), 0.0);
    }

    #[test]
    fn bounded_in_zero_to_aggro() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..4000 {
            let a = (lcg(&mut seed) % 100_001) as f32 / 1000.0; // [0,100]
            let dt = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
            let dist = (lcg(&mut seed) % 100_001) as f32 / 100.0;
            let r = decay(a, dt, dist);
            assert!(r >= 0.0 && r <= a + 1e-5, "{} not in [0,{}]", r, a);
        }
    }

    #[test]
    fn dt_zero_unchanged() {
        let mut seed = 0x0001_u32;
        for _ in 0..1000 {
            let a = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
            let dist = (lcg(&mut seed) % 100_001) as f32 / 100.0;
            assert_eq!(decay(a, 0.0, dist), a);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let a = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
            let dt = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
            let dist = (lcg(&mut seed) % 100_001) as f32 / 100.0;
            assert_eq!(decay(a, dt, dist), decay(a, dt, dist));
        }
    }
}
