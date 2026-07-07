//! `snow_accumulate` — snow depth change over a time step.
//!
//! Worker-03 / WORLD pure core, round 24. Below freezing, falling precipitation
//! adds to the snow pack; above freezing, the pack melts at a rate set by how
//! far above freezing it is. Depth never goes negative, and a cold, dry cell
//! holds steady. Pairs with `snow_line` (R19) and surface material selection.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const FREEZE: f32 = 0.0; // freezing temperature
const MELT_RATE: f32 = 0.5; // melt per degree above freezing per unit time

/// New snow depth from current `depth`, ambient `temp`, `precip` rate, over `dt`.
/// Accumulates below freezing, melts above, never negative.
pub fn depth_step(depth: f32, temp: f32, precip: f32, dt: f32) -> f32 {
    let d0 = depth.max(0.0);
    let p = precip.max(0.0);
    let step = dt.max(0.0);

    let new = if temp < FREEZE {
        d0 + p * step
    } else if temp > FREEZE {
        d0 - MELT_RATE * (temp - FREEZE) * step
    } else {
        d0
    };

    if new < 0.0 {
        0.0
    } else {
        new
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_with_precip_grows() {
        assert!(depth_step(2.0, -5.0, 1.0, 1.0) > 2.0);
    }

    #[test]
    fn warm_melts() {
        assert!(depth_step(5.0, 5.0, 0.0, 1.0) < 5.0);
    }

    #[test]
    fn never_negative() {
        // Lots of melt on a thin pack clamps at 0, not below.
        assert_eq!(depth_step(0.1, 50.0, 0.0, 10.0), 0.0);
    }

    #[test]
    fn cold_no_precip_is_steady() {
        assert_eq!(depth_step(3.0, -10.0, 0.0, 1.0), 3.0);
    }

    #[test]
    fn bounded() {
        for di in 0..20 {
            for ti in -10..10 {
                let d0 = di as f32;
                let v = depth_step(d0, ti as f32, 1.0, 1.0);
                assert!(v >= 0.0);
                // a single cold step adds at most precip·dt
                assert!(v <= d0 + 1.0 + 1e-6);
            }
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(depth_step(2.0, -3.0, 0.5, 1.0), depth_step(2.0, -3.0, 0.5, 1.0));
    }
}
