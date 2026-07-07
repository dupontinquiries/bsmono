//! `sim::env_batch` — run the per-chunk water/environment step across worker threads.
//!
//! [`env_frame::tile_step`](crate::sim::env_frame::tile_step) is the unified
//! per-chunk step: it adds **rainfall**, flows **water** downhill, applies **drain
//! physics** (infiltration + evaporation), **erodes** terrain, and diffuses soil
//! **moisture** (the field plants read) — all as a pure function of one chunk's
//! `w*h` tile. Tiles are independent, so stepping a whole set of them is a
//! **Class-P** parallel map (disjoint owned outputs) over the engine's rayon lane
//! ([`util::task_lanes::map_pure`](crate::util::task_lanes)). One batch call threads
//! water flow, drainage, rainfall, moisture/plant-water, and hydraulic erosion
//! together, and because `map_pure` preserves order the result is identical to
//! stepping each tile serially.

use crate::sim::env_frame::tile_step;
use crate::util::task_lanes::{map_pure, map_pure_threshold};

/// One chunk's environment state: row-major `w*h` height / water / moisture arrays.
/// `heights` is mutated by erosion, so it round-trips through the step.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvTile {
    pub heights: Vec<f32>,
    pub water: Vec<f32>,
    pub moisture: Vec<f32>,
    pub w: usize,
    pub h: usize,
}

impl EnvTile {
    /// Step this tile once (rain → flow → drain → erode → moisture), in place.
    pub fn step(&mut self, rain: f32, talus: f32) {
        tile_step(&mut self.heights, &mut self.water, &mut self.moisture, self.w, self.h, rain, talus);
    }

    /// Owned step for the parallel map: clone → step → return (disjoint outputs).
    fn stepped(&self, rain: f32, talus: f32) -> EnvTile {
        let mut t = self.clone();
        t.step(rain, talus);
        t
    }
}

/// Step every tile once in parallel across the worker pool. Order-preserving, so the
/// output equals stepping each tile serially.
pub fn step_tiles(tiles: &[EnvTile], rain: f32, talus: f32) -> Vec<EnvTile> {
    map_pure(tiles, |t| t.stepped(rain, talus))
}

/// As [`step_tiles`], but run serially below `min_parallel` tiles.
pub fn step_tiles_threshold(
    tiles: &[EnvTile],
    min_parallel: usize,
    rain: f32,
    talus: f32,
) -> Vec<EnvTile> {
    map_pure_threshold(tiles, min_parallel, |t| t.stepped(rain, talus))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(seed: f32) -> EnvTile {
        let (w, h) = (4, 4);
        let mut heights = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                heights[y * w + x] = (x as f32 + y as f32) * 0.5 + seed; // a slope
            }
        }
        EnvTile { heights, water: vec![0.0; w * h], moisture: vec![0.0; w * h], w, h }
    }

    #[test]
    fn parallel_equals_serial_and_preserves_order() {
        let tiles: Vec<EnvTile> = (0..16).map(|i| tile(i as f32)).collect();
        let par = step_tiles(&tiles, 0.2, 0.5);
        let ser: Vec<EnvTile> = tiles.iter().map(|t| t.stepped(0.2, 0.5)).collect();
        assert_eq!(par, ser, "Class-P batch equals the serial fold, in order");
    }

    #[test]
    fn rainfall_adds_water() {
        let tiles = vec![tile(0.0)];
        let dry = step_tiles(&tiles, 0.0, 0.0);
        let wet = step_tiles(&tiles, 0.5, 0.0);
        let dry_total: f32 = dry[0].water.iter().sum();
        let wet_total: f32 = wet[0].water.iter().sum();
        assert!(wet_total > dry_total, "rain raises standing water ({wet_total} > {dry_total})");
    }

    #[test]
    fn threshold_matches_full_parallel() {
        let tiles: Vec<EnvTile> = (0..20).map(|i| tile(i as f32 * 0.1)).collect();
        assert_eq!(step_tiles_threshold(&tiles, 4, 0.1, 0.3), step_tiles(&tiles, 0.1, 0.3));
    }

    #[test]
    fn empty_and_no_mutation() {
        assert!(step_tiles(&[], 0.1, 0.1).is_empty());
        let tiles: Vec<EnvTile> = (0..4).map(|i| tile(i as f32)).collect();
        let snapshot = tiles.clone();
        let _ = step_tiles(&tiles, 0.3, 0.4);
        assert_eq!(tiles, snapshot, "the parallel map reads inputs, never mutates them");
    }
}
