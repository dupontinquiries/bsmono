//! `lake_fill` — water level in a basin, capped at the spill height.
//!
//! Worker-03 / WORLD pure core, round 19. A basin fills with inflow from its
//! floor upward, but can't rise past the spill point (where it would overflow
//! into the next basin). Dry basins sit at the floor. Pairs with
//! `flow_route` / `flow_accumulation` (which feed inflow) and `sea_level` (R3).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const FILL_RATE: f32 = 1.0; // inflow units → height units

/// Standing water level for a basin with `basin_floor`, `spill` height, and
/// accumulated `inflow`. Rises from the floor with inflow, capped at `spill`.
pub fn water_level(basin_floor: f32, spill: f32, inflow: f32) -> f32 {
    let cap = spill.max(basin_floor); // spill can't be below the floor
    let inf = inflow.max(0.0);
    let level = basin_floor + inf * FILL_RATE;
    level.min(cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rises_with_inflow() {
        assert!(water_level(0.0, 100.0, 10.0) > water_level(0.0, 100.0, 2.0));
    }

    #[test]
    fn capped_at_spill() {
        let lvl = water_level(0.0, 5.0, 1000.0);
        assert!((lvl - 5.0).abs() < 1e-6);
    }

    #[test]
    fn floor_when_dry() {
        assert_eq!(water_level(12.0, 30.0, 0.0), 12.0);
    }

    #[test]
    fn monotone_in_inflow() {
        let mut prev = -1.0f32;
        for i in 0..200 {
            let lvl = water_level(3.0, 50.0, i as f32);
            assert!(lvl >= prev - 1e-6);
            prev = lvl;
        }
    }

    #[test]
    fn bounded_between_floor_and_spill() {
        for i in 0..300 {
            let lvl = water_level(10.0, 40.0, i as f32);
            assert!(lvl >= 10.0 && lvl <= 40.0);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(water_level(2.0, 20.0, 7.0), water_level(2.0, 20.0, 7.0));
    }
}
