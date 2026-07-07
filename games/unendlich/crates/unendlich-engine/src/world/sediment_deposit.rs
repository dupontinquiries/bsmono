//! `sediment_deposit` — drop carried sediment into a cell.
//!
//! Worker-03 / WORLD pure core, round 19. Flowing water carries sediment up to a
//! capacity; this decides how much settles here and how much continues. Over
//! capacity, the excess drops immediately; within capacity, gentle slopes settle
//! a larger fraction (slow water drops its load). Mass is conserved:
//! `deposited + remaining == carried`. Pairs with `hydraulic_erosion` (R10).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const SETTLE_MAX: f32 = 0.5; // max settle fraction on flat ground
const SLOPE_FACTOR: f32 = 2.0; // how fast settling falls off with slope

/// Deposit from `carried` sediment given local `slope` and flow `capacity`.
/// Returns `(deposited, remaining)`, conserving mass.
pub fn deposit(carried: f32, slope: f32, capacity: f32) -> (f32, f32) {
    let c = carried.max(0.0);
    let cap = capacity.max(0.0);
    let s = slope.max(0.0);

    if c > cap {
        // Over capacity: shed the excess now, carry the capacity onward.
        let dep = c - cap;
        return (dep, cap);
    }
    // Within capacity: settle a slope-dependent fraction (more on the flats).
    let settle = clamp01(SETTLE_MAX / (1.0 + s * SLOPE_FACTOR));
    let dep = c * settle;
    (dep, c - dep)
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_slope_deposits_more() {
        let (flat, _) = deposit(10.0, 0.0, 100.0);
        let (steep, _) = deposit(10.0, 5.0, 100.0);
        assert!(flat > steep);
    }

    #[test]
    fn over_capacity_drops_excess() {
        let (dep, rem) = deposit(30.0, 1.0, 20.0);
        assert!((dep - 10.0).abs() < 1e-5); // excess 30-20
        assert!((rem - 20.0).abs() < 1e-5); // carry the capacity
    }

    #[test]
    fn mass_conserved() {
        for &(c, s, cap) in &[
            (30.0f32, 1.0, 20.0),
            (5.0, 0.0, 100.0),
            (12.0, 3.0, 12.0),
            (0.0, 2.0, 5.0),
        ] {
            let (dep, rem) = deposit(c, s, cap);
            assert!((dep + rem - c).abs() < 1e-4, "mass not conserved");
        }
    }

    #[test]
    fn bounded() {
        for ci in 0..20 {
            for si in 0..20 {
                let c = ci as f32;
                let (dep, rem) = deposit(c, si as f32 * 0.5, 10.0);
                assert!(dep >= 0.0 && dep <= c + 1e-4);
                assert!(rem >= 0.0 && rem <= c + 1e-4);
            }
        }
    }

    #[test]
    fn flat_edge_deposits_the_most() {
        // Flat ground within capacity settles the SETTLE_MAX fraction.
        let (dep, _) = deposit(10.0, 0.0, 100.0);
        assert!((dep - 10.0 * SETTLE_MAX).abs() < 1e-5);
    }

    #[test]
    fn deterministic() {
        assert_eq!(deposit(8.0, 1.5, 20.0), deposit(8.0, 1.5, 20.0));
    }
}
