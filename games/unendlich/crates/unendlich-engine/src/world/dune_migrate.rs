//! `dune_migrate` — sand-dune crest migration distance.
//!
//! Worker-03 / WORLD pure core, round 25. How far a dune crest advances in one
//! step: driven by wind, requiring sand supply to move, and scaling with time.
//! No wind means no migration; thin sand supply migrates slowly. Pairs with
//! `vegetation_scatter` (R12) / `beach_band` (R18). Pure and deterministic.
//!
//! Self-contained for `rustc --test`.
#![allow(dead_code)]

const WIND_K: f32 = 1.0; // wind at which the wind term is ~half
const SAND_K: f32 = 1.0; // sand supply at which the sand term is ~half
const SPEED: f32 = 1.0; // migration constant
const MAX_DIST: f32 = 1000.0; // per-step cap

/// Crest migration distance for `wind` over a `sand` supply, across `dt`.
/// Zero without wind; rises with wind and dt; throttled by low sand; capped.
pub fn migrate(wind: f32, sand: f32, dt: f32) -> f32 {
    let w = wind.max(0.0);
    let s = sand.max(0.0);
    let t = dt.max(0.0);
    let wind_term = w / (w + WIND_K); // 0 at calm, →1
    let sand_term = s / (s + SAND_K); // 0 at no sand, →1
    let dist = SPEED * wind_term * sand_term * t;
    if dist < 0.0 {
        0.0
    } else if dist > MAX_DIST {
        MAX_DIST
    } else {
        dist
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_wind_no_migration() {
        assert_eq!(migrate(0.0, 10.0, 1.0), 0.0);
    }

    #[test]
    fn more_wind_migrates_farther() {
        assert!(migrate(10.0, 10.0, 1.0) > migrate(1.0, 10.0, 1.0));
    }

    #[test]
    fn low_sand_is_slow() {
        assert!(migrate(5.0, 0.1, 1.0) < migrate(5.0, 10.0, 1.0));
    }

    #[test]
    fn dt_scales() {
        let a = migrate(5.0, 5.0, 1.0);
        let b = migrate(5.0, 5.0, 2.0);
        assert!(b > a);
        assert!((b - 2.0 * a).abs() < 1e-3); // linear in dt below the cap
    }

    #[test]
    fn bounded() {
        for wi in 0..20 {
            for si in 0..20 {
                let v = migrate(wi as f32, si as f32, 50.0);
                assert!(v >= 0.0 && v <= MAX_DIST);
            }
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(migrate(4.0, 6.0, 1.0), migrate(4.0, 6.0, 1.0));
    }
}
