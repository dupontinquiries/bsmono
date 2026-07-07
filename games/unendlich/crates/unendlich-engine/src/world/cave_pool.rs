//! `cave_pool` — standing-water depth in a cave-floor depression.
//!
//! Worker-03 / WORLD pure core, round 21. A dip in the cave floor collects
//! inflow into a pool; the pool can hold up to the depression depth, and never
//! rises above the rim (where it would spill out). No inflow → dry. Pairs with
//! `cave_room` (R17) / `lake_fill` (R19, the surface analogue).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const FILL: f32 = 1.0; // inflow units → depth units

/// Pool depth for a depression `floor_dip` deep with a `rim` height and
/// accumulated `inflow`. Holds up to `min(floor_dip, rim)`; dry without inflow.
pub fn pool_depth(floor_dip: f32, inflow: f32, rim: f32) -> f32 {
    let cap = floor_dip.max(0.0).min(rim.max(0.0));
    let inf = inflow.max(0.0);
    (inf * FILL).min(cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deeper_dip_holds_more() {
        // With ample inflow, a deeper dip pools deeper (both under the rim).
        assert!(pool_depth(5.0, 1000.0, 100.0) > pool_depth(2.0, 1000.0, 100.0));
    }

    #[test]
    fn capped_at_rim() {
        // Dip is deep but the rim is low → pool capped at the rim.
        let d = pool_depth(50.0, 1000.0, 10.0);
        assert!((d - 10.0).abs() < 1e-6);
    }

    #[test]
    fn dry_when_no_inflow() {
        assert_eq!(pool_depth(8.0, 0.0, 20.0), 0.0);
    }

    #[test]
    fn monotone_in_inflow() {
        let mut prev = -1.0f32;
        for i in 0..100 {
            let d = pool_depth(20.0, i as f32, 50.0);
            assert!(d >= prev - 1e-6);
            prev = d;
        }
    }

    #[test]
    fn bounded() {
        for i in 0..200 {
            let d = pool_depth(12.0, i as f32, 30.0);
            assert!(d >= 0.0 && d <= 12.0);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(pool_depth(6.0, 3.0, 20.0), pool_depth(6.0, 3.0, 20.0));
    }
}
