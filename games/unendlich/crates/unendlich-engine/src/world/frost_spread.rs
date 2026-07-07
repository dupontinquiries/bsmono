//! `frost_spread` — frost coverage by temperature and humidity.
//!
//! Worker-03 / WORLD-FX pure core, round 21. Frost forms on cold, humid
//! surfaces: coverage rises as temperature falls and as humidity rises, and is
//! zero when warm or bone-dry. Pairs with `snow_line` (R19) and surface material
//! selection.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Frost coverage in [0,1] from `temp` and `humidity` (both ≈[0,1]). Max when
/// cold and humid; zero when warm; low when dry.
pub fn frost(temp: f32, humidity: f32) -> f32 {
    let t = clamp01(temp);
    let h = clamp01(humidity);
    let cold = 1.0 - t; // colder ⇒ more
    clamp01(cold * h)
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_and_humid_is_max() {
        assert!((frost(0.0, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn warm_is_zero() {
        for hi in 0..=10 {
            assert_eq!(frost(1.0, hi as f32 / 10.0), 0.0);
        }
    }

    #[test]
    fn dry_is_low() {
        assert_eq!(frost(0.0, 0.0), 0.0);
        assert!(frost(0.1, 0.1) < 0.2);
    }

    #[test]
    fn bounded() {
        for ti in 0..=10 {
            for hi in 0..=10 {
                let v = frost(ti as f32 / 10.0, hi as f32 / 10.0);
                assert!(v >= 0.0 && v <= 1.0);
            }
        }
    }

    #[test]
    fn monotone_in_cold() {
        // Colder (lower temp) ⇒ more frost at fixed humidity.
        assert!(frost(0.1, 0.8) > frost(0.9, 0.8));
    }

    #[test]
    fn deterministic() {
        assert_eq!(frost(0.3, 0.6), frost(0.3, 0.6));
    }
}
