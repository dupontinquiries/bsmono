//! `sim::mount_stamina` — a mount's stamina, draining as it sprints and
//! recovering as it rests.
//!
//! [`stamina_step`] advances stamina by one tick: while `sprinting` it loses
//! `drain · dt`, otherwise it recovers `regen · dt`, clamped to `[0, max]`. So a
//! galloping mount tires and a resting one recovers, and the change scales with
//! the elapsed time `dt`. Stamina never drops below 0 or rises above its cap.
//!
//! Pure and deterministic: one linear step and a clamp — no clock, no
//! randomness, no cross-imports.

fn nonneg(x: f32) -> f32 {
    if x > 0.0 {
        x
    } else {
        0.0
    }
}

/// Advance mount stamina one tick.
///
/// `sprinting` ⇒ `cur - drain·dt`, else `cur + regen·dt`, clamped to
/// `[0, max(0,max)]`. `dt`, `drain`, `regen` are floored at 0.
pub fn stamina_step(
    cur: f32,
    sprinting: bool,
    dt: f32,
    drain: f32,
    regen: f32,
    max: f32,
) -> f32 {
    let d = nonneg(dt);
    let next = if sprinting {
        cur - nonneg(drain) * d
    } else {
        cur + nonneg(regen) * d
    };
    let hi = if max > 0.0 { max } else { 0.0 };
    if next < 0.0 {
        0.0
    } else if next > hi {
        hi
    } else {
        next
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
    fn sprinting_drains() {
        assert_eq!(stamina_step(50.0, true, 1.0, 10.0, 5.0, 100.0), 40.0);
    }

    #[test]
    fn resting_regens() {
        assert_eq!(stamina_step(50.0, false, 1.0, 10.0, 5.0, 100.0), 55.0);
    }

    #[test]
    fn never_below_zero() {
        // over-drain clamps at 0
        assert_eq!(stamina_step(5.0, true, 10.0, 10.0, 5.0, 100.0), 0.0);
    }

    #[test]
    fn never_above_max() {
        // over-regen clamps at max
        assert_eq!(stamina_step(98.0, false, 10.0, 10.0, 5.0, 100.0), 100.0);
    }

    #[test]
    fn dt_scales_change() {
        let small = stamina_step(50.0, true, 0.5, 10.0, 5.0, 100.0);
        let big = stamina_step(50.0, true, 2.0, 10.0, 5.0, 100.0);
        assert!(big < small, "more dt should drain more: {} vs {}", big, small);
        // and bounded everywhere
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let cur = (lcg(&mut seed) % 20_001) as f32 / 100.0;
            let sprint = lcg(&mut seed) & 1 == 0;
            let dt = (lcg(&mut seed) % 10_001) as f32 / 100.0;
            let drn = (lcg(&mut seed) % 10_001) as f32 / 100.0;
            let rgn = (lcg(&mut seed) % 10_001) as f32 / 100.0;
            let mx = (lcg(&mut seed) % 20_001) as f32 / 100.0;
            let s = stamina_step(cur, sprint, dt, drn, rgn, mx);
            assert!(s >= 0.0 && s <= mx.max(0.0));
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let cur = (lcg(&mut seed) % 10_001) as f32 / 100.0;
            let sprint = lcg(&mut seed) & 1 == 0;
            let dt = (lcg(&mut seed) % 10_001) as f32 / 100.0;
            let drn = (lcg(&mut seed) % 10_001) as f32 / 100.0;
            let rgn = (lcg(&mut seed) % 10_001) as f32 / 100.0;
            let mx = (lcg(&mut seed) % 10_001) as f32 / 100.0;
            assert_eq!(
                stamina_step(cur, sprint, dt, drn, rgn, mx),
                stamina_step(cur, sprint, dt, drn, rgn, mx)
            );
        }
    }
}
