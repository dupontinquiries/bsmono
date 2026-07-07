//! `aquifer` — groundwater table height by rainfall and drainage.
//!
//! Worker-03 / WORLD pure core, round 23. The water table sits at the bedrock
//! floor plus a rise driven by net recharge: rainfall raises it, drainage lowers
//! it, and it can't fall below bedrock or rise past a saturation cap. Feeds
//! spring/seep placement and `moisture_spread` (R17). Pairs with `lake_fill`
//! (R19, surface water) as the subsurface analogue.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const DRAIN_FACTOR: f32 = 1.0; // how strongly drainage offsets rain
const RISE_SCALE: f32 = 1.0; // net recharge → height units
const MAX_RISE: f32 = 64.0; // saturation cap above bedrock

/// Water-table height for given `rain`, `drainage`, and `bedrock` floor. Rises
/// with rain, falls with drainage, never below bedrock, capped at
/// `bedrock + MAX_RISE`.
pub fn water_table(rain: f32, drainage: f32, bedrock: f32) -> f32 {
    let r = rain.max(0.0);
    let d = drainage.max(0.0);
    let net = (r - d * DRAIN_FACTOR).max(0.0); // can't drain below the floor
    let rise = (net * RISE_SCALE).min(MAX_RISE);
    bedrock + rise
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn more_rain_raises_table() {
        assert!(water_table(20.0, 2.0, 30.0) > water_table(5.0, 2.0, 30.0));
    }

    #[test]
    fn more_drainage_lowers_table() {
        assert!(water_table(20.0, 2.0, 30.0) > water_table(20.0, 10.0, 30.0));
    }

    #[test]
    fn never_below_bedrock() {
        for d in 0..50 {
            let t = water_table(1.0, d as f32, 40.0);
            assert!(t >= 40.0);
        }
    }

    #[test]
    fn bounded_above() {
        let t = water_table(1e6, 0.0, 30.0);
        assert!((t - (30.0 + MAX_RISE)).abs() < 1e-3);
    }

    #[test]
    fn monotone() {
        // non-decreasing in rain
        let mut prev = -1.0f32;
        for r in 0..100 {
            let t = water_table(r as f32, 5.0, 10.0);
            assert!(t >= prev - 1e-6);
            prev = t;
        }
        // non-increasing in drainage
        let mut prevd = f32::INFINITY;
        for d in 0..100 {
            let t = water_table(50.0, d as f32, 10.0);
            assert!(t <= prevd + 1e-6);
            prevd = t;
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(water_table(12.0, 3.0, 20.0), water_table(12.0, 3.0, 20.0));
    }
}
