//! `sim::water_batch` — run the water simulation across worker threads.
//!
//! Each chunk's surface water is a `w*h` tile that
//! [`water_physics::simulate`](crate::sim::water_physics::simulate) steps. Tiles
//! are independent (a tile's step reads/writes only its own arrays), so stepping
//! many of them is a **Class-P** parallel map — disjoint owned outputs — over the
//! engine's rayon lane ([`util::task_lanes::map_pure`](crate::util::task_lanes)).
//! That spawns the work across the worker pool without any shared mutable state,
//! and because `map_pure` preserves input order the result is identical to
//! stepping every tile serially: deterministic regardless of how the threads
//! schedule.
//!
//! The same shape applies to the other per-chunk subsystems (fluid field,
//! placement, terrain sampling): gather independent tiles, map them pure.

use crate::sim::water_physics::{simulate, WaterParams};
use crate::util::task_lanes::{map_pure, map_pure_threshold};

/// One chunk's water state: row-major `w*h` height / water / moisture arrays.
#[derive(Clone, Debug, PartialEq)]
pub struct WaterTile {
    pub heights: Vec<f32>,
    pub water: Vec<f32>,
    pub moisture: Vec<f32>,
    pub w: usize,
    pub h: usize,
}

impl WaterTile {
    /// Step this tile `n` shallow-water iterations in place (size mismatches are
    /// a no-op inside `simulate`).
    pub fn step(&mut self, p: &WaterParams, n: u32) {
        simulate(&self.heights, &mut self.water, &mut self.moisture, self.w, self.h, p, n);
    }

    /// Owned step used by the parallel map: clone → step → return (so each thread
    /// works on its own copy and outputs are disjoint).
    fn stepped(&self, p: &WaterParams, n: u32) -> WaterTile {
        let mut t = self.clone();
        t.step(p, n);
        t
    }
}

/// Step every tile `n` iterations in parallel across the worker pool. Order is
/// preserved, so the output equals stepping each tile serially.
pub fn step_tiles(tiles: &[WaterTile], p: &WaterParams, n: u32) -> Vec<WaterTile> {
    map_pure(tiles, |t| t.stepped(p, n))
}

/// As [`step_tiles`], but run serially below `min_parallel` tiles — a handful of
/// chunks per frame isn't worth a fork.
pub fn step_tiles_threshold(
    tiles: &[WaterTile],
    min_parallel: usize,
    p: &WaterParams,
    n: u32,
) -> Vec<WaterTile> {
    map_pure_threshold(tiles, min_parallel, |t| t.stepped(p, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(seed: f32) -> WaterTile {
        // 4x4 bowl: low centre, water piled on one side.
        let w = 4;
        let h = 4;
        let mut heights = vec![0.0f32; w * h];
        let mut water = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                heights[y * w + x] = ((x as f32 - 1.5).abs() + (y as f32 - 1.5).abs()) + seed;
            }
        }
        water[0] = 4.0; // a pile in one corner
        WaterTile { heights, water, moisture: vec![0.0; w * h], w, h }
    }

    fn total_water(t: &WaterTile) -> f32 {
        t.water.iter().sum()
    }

    #[test]
    fn parallel_equals_serial_and_preserves_order() {
        let tiles: Vec<WaterTile> = (0..16).map(|i| tile(i as f32)).collect();
        let p = WaterParams::default();
        let par = step_tiles(&tiles, &p, 3);
        let ser: Vec<WaterTile> = tiles.iter().map(|t| t.stepped(&p, 3)).collect();
        assert_eq!(par, ser, "Class-P map equals the serial fold, in order");
    }

    #[test]
    fn threshold_matches_full_parallel() {
        let tiles: Vec<WaterTile> = (0..20).map(|i| tile(i as f32 * 0.1)).collect();
        let p = WaterParams::default();
        assert_eq!(step_tiles_threshold(&tiles, 4, &p, 2), step_tiles(&tiles, &p, 2));
        // below the threshold it still produces the right answer
        let few = &tiles[..2];
        assert_eq!(
            step_tiles_threshold(few, 4, &p, 2),
            few.iter().map(|t| t.stepped(&p, 2)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn mass_only_decreases_per_tile() {
        let tiles: Vec<WaterTile> = (0..8).map(|i| tile(i as f32)).collect();
        let before: Vec<f32> = tiles.iter().map(total_water).collect();
        let after = step_tiles(&tiles, &WaterParams::default(), 5);
        for (b, t) in before.iter().zip(after.iter()) {
            let a = total_water(t);
            assert!(a <= *b + 1e-3, "water never created: {a} <= {b}");
            assert!(a >= 0.0, "water never negative");
        }
    }

    #[test]
    fn empty_batch_is_empty() {
        assert!(step_tiles(&[], &WaterParams::default(), 1).is_empty());
    }

    #[test]
    fn inputs_are_not_mutated() {
        let tiles: Vec<WaterTile> = (0..4).map(|i| tile(i as f32)).collect();
        let snapshot = tiles.clone();
        let _ = step_tiles(&tiles, &WaterParams::default(), 3);
        assert_eq!(tiles, snapshot, "the parallel map reads inputs, never mutates them");
    }
}
