//! `sim::flora_spread` (r450) — decide which neighbouring cells a flora block seeds in a tick.
//!
//! Pure glue over two existing cores: [`plant_propagation`] (downwind dispersal + germination
//! viability) evaluated against [`plant_growth_env`] [`Conditions`]/[`Tolerance`]. The CALLER owns
//! the live-world part — checking that a target cell is empty over suitable ground, and placing the
//! block — so this module stays deterministic and unit-testable.
//!
//! Tolerances for the common ground flora are provided as named helpers so the wiring reads clearly:
//! hardy ground cover (grass/fern), warmth-loving flowers, and cool-damp mushrooms.

use crate::sim::plant_growth_env::{Conditions, Tolerance};
use crate::sim::plant_propagation::{disperse, germination_chance, viable, DispersalMode};

const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

/// Hardy ground cover (grass, fern): wide temperature band, some drought resistance.
pub fn ground_cover_tolerance() -> Tolerance {
    Tolerance { temp_lo: -5.0, temp_hi: 40.0, temp_opt: 18.0, drought_resist01: 0.4 }
}

/// Flowers: narrower, warmth-loving, less drought-tolerant than grass.
pub fn flower_tolerance() -> Tolerance {
    Tolerance { temp_lo: 2.0, temp_hi: 36.0, temp_opt: 22.0, drought_resist01: 0.2 }
}

/// Mushrooms: cool, damp, shade-loving — low optimum temperature, no drought resistance.
pub fn mushroom_tolerance() -> Tolerance {
    Tolerance { temp_lo: 0.0, temp_hi: 30.0, temp_opt: 12.0, drought_resist01: 0.0 }
}

/// Integer cells `[x, z]` that a parent at `parent_xy` seeds this tick.
///
/// Disperses `n` seeds downwind ([`DispersalMode::Wind`]), snaps them to integer cells, drops the
/// parent's own cell and any duplicates, and keeps only the cells whose per-seed draw germinates
/// under the local `cond` versus `tol`. Deterministic in `seed`: same inputs ⇒ same cells.
pub fn spread_targets(
    parent_xy: [f32; 2],
    tol: &Tolerance,
    cond: &Conditions,
    wind_xy: [f32; 2],
    seed: u64,
    n: usize,
) -> Vec<[i32; 2]> {
    let chance = germination_chance(cond, tol);
    let pcx = parent_xy[0].floor() as i32;
    let pcz = parent_xy[1].floor() as i32;
    let raw = disperse(parent_xy, DispersalMode::Wind, wind_xy, seed, n);
    let mut out: Vec<[i32; 2]> = Vec::with_capacity(n);
    for (i, p) in raw.iter().enumerate() {
        let cx = p[0].floor() as i32;
        let cz = p[1].floor() as i32;
        if cx == pcx && cz == pcz {
            continue; // never seed onto itself
        }
        if out.contains(&[cx, cz]) {
            continue; // dedup
        }
        if viable(seed ^ (i as u64).wrapping_mul(GOLDEN), chance) {
            out.push([cx, cz]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ideal() -> Conditions {
        Conditions { moisture01: 0.9, light01: 1.0, temp_c: 18.0, season01: 0.5 }
    }
    fn frozen() -> Conditions {
        Conditions { moisture01: 0.9, light01: 1.0, temp_c: -40.0, season01: 0.5 }
    }

    #[test]
    fn never_seeds_its_own_cell() {
        let t = ground_cover_tolerance();
        for seed in 0..200u64 {
            let cells = spread_targets([10.3, 20.7], &t, &ideal(), [0.0, 0.0], seed, 8);
            assert!(cells.iter().all(|c| *c != [10, 20]));
        }
    }

    #[test]
    fn frozen_conditions_block_germination() {
        let t = ground_cover_tolerance();
        let mut total = 0;
        for seed in 0..500u64 {
            total += spread_targets([0.5, 0.5], &t, &frozen(), [1.0, 0.0], seed, 6).len();
        }
        assert_eq!(total, 0);
    }

    #[test]
    fn ideal_conditions_do_germinate() {
        let t = ground_cover_tolerance();
        let mut total = 0;
        for seed in 0..200u64 {
            total += spread_targets([0.5, 0.5], &t, &ideal(), [2.0, 0.0], seed, 6).len();
        }
        assert!(total > 100, "got {total}");
    }

    #[test]
    fn deterministic_in_seed() {
        let t = flower_tolerance();
        let c = Conditions { moisture01: 0.7, light01: 0.9, temp_c: 22.0, season01: 0.5 };
        let a = spread_targets([4.2, 9.9], &t, &c, [1.5, -0.5], 12345, 8);
        let b = spread_targets([4.2, 9.9], &t, &c, [1.5, -0.5], 12345, 8);
        assert_eq!(a, b);
    }

    #[test]
    fn no_duplicate_cells() {
        let t = ground_cover_tolerance();
        for seed in 0..300u64 {
            let cells = spread_targets([0.5, 0.5], &t, &ideal(), [0.2, 0.2], seed, 10);
            let mut sorted = cells.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), cells.len());
        }
    }

    #[test]
    fn strong_wind_biases_downwind() {
        let t = ground_cover_tolerance();
        let (mut sx, mut n) = (0i64, 0i64);
        for seed in 0..2000u64 {
            for c in spread_targets([0.5, 0.5], &t, &ideal(), [8.0, 0.0], seed, 4) {
                sx += c[0] as i64;
                n += 1;
            }
        }
        assert!(n > 0);
        assert!(sx as f64 / n as f64 > 1.0);
    }

    #[test]
    fn mushroom_wants_damp_cool() {
        let mush = mushroom_tolerance();
        let damp_cool = Conditions { moisture01: 0.95, light01: 0.6, temp_c: 12.0, season01: 0.5 };
        let hot_dry = Conditions { moisture01: 0.05, light01: 1.0, temp_c: 45.0, season01: 0.5 };
        assert!(germination_chance(&damp_cool, &mush) > 0.4);
        assert!(germination_chance(&hot_dry, &mush) < 0.05);
    }
}
