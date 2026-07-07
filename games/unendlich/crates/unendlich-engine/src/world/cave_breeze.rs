//! `cave_breeze` — airflow speed through a cave passage.
//!
//! Worker-03 / WORLD-FX pure core, round 23. By continuity, a fixed pressure
//! difference drives faster air through a narrower passage (Venturi): speed
//! rises with pressure and falls with cross-section, is zero without a pressure
//! difference, and saturates at a cap. Drives ambient wind/particle motion in
//! caves. Pairs with `cave_room` (R17) / `dust_motes` (R14).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const MIN_AREA: f32 = 0.1; // floor so a pinch doesn't divide by zero
const K: f32 = 1.0; // speed constant
const MAX_SPEED: f32 = 20.0; // saturation cap

/// Breeze speed through a passage of `cross_section` driven by `pressure_diff`.
/// Faster when narrow and/or high-pressure; zero without pressure; capped.
pub fn breeze(cross_section: f32, pressure_diff: f32) -> f32 {
    let a = cross_section.max(MIN_AREA);
    let p = pressure_diff.max(0.0);
    let speed = K * p / a;
    if speed > MAX_SPEED {
        MAX_SPEED
    } else {
        speed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_with_pressure_is_fast() {
        let narrow = breeze(0.5, 3.0);
        let wide = breeze(8.0, 3.0);
        assert!(narrow > wide);
    }

    #[test]
    fn wide_is_slow() {
        assert!(breeze(20.0, 1.0) < breeze(2.0, 1.0));
    }

    #[test]
    fn no_pressure_is_zero() {
        for ai in 1..20 {
            assert_eq!(breeze(ai as f32, 0.0), 0.0);
        }
    }

    #[test]
    fn bounded() {
        for ai in 0..20 {
            for pi in 0..40 {
                let v = breeze(ai as f32 * 0.5, pi as f32);
                assert!(v >= 0.0 && v <= MAX_SPEED);
            }
        }
    }

    #[test]
    fn monotone() {
        // rises with pressure (below cap), falls with cross-section
        assert!(breeze(4.0, 5.0) > breeze(4.0, 1.0));
        assert!(breeze(2.0, 2.0) > breeze(6.0, 2.0));
    }

    #[test]
    fn deterministic() {
        assert_eq!(breeze(3.0, 2.0), breeze(3.0, 2.0));
    }
}
