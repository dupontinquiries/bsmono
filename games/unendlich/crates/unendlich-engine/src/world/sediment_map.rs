//! `sediment_map` — the bookkeeping for flow erosion (operator: "sediment as a per-chunk map table
//! to deposit"). Erosion doesn't just delete bed; it lifts material into a *suspended load* that the
//! current carries and drops again where it slows. This sparse per-column table tracks that load so
//! the system conserves mass: every block eroded is carried as sediment and later deposited, instead
//! of vanishing. Pure + tested; the water sim wires erosion → `erode`, flow transport → `advect`,
//! and slow water → `deposit_excess` (which returns whole blocks to place as bed).

use std::collections::HashMap;

/// Suspended sediment per world column, keyed by (x,z). Units are "blocks of material".
#[derive(Debug, Default, Clone)]
pub struct SedimentMap {
    load: HashMap<(i32, i32), f32>,
    /// running total deposited as solid blocks (for mass-balance assertions/telemetry).
    pub deposited_blocks: u64,
    /// running total eroded from the bed (blocks of material lifted).
    pub eroded_total: f64,
}

impl SedimentMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn suspended(&self, x: i32, z: i32) -> f32 {
        self.load.get(&(x, z)).copied().unwrap_or(0.0)
    }

    /// Total material currently suspended (mass still in transit).
    pub fn total_suspended(&self) -> f64 {
        self.load.values().map(|&v| v as f64).sum()
    }

    /// Lift `amount` (>0) of bed at (x,z) into suspension. Caller is responsible for removing the
    /// bed voxel(s); this only books the material so it can be deposited later.
    pub fn erode(&mut self, x: i32, z: i32, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        *self.load.entry((x, z)).or_insert(0.0) += amount;
        self.eroded_total += amount as f64;
    }

    /// Move a fraction `frac` (0..1) of the suspended load at `from` to `to`, following the current.
    /// Self-move is a no-op. Models advection of sediment downstream.
    pub fn advect(&mut self, from: (i32, i32), to: (i32, i32), frac: f32) {
        if from == to {
            return;
        }
        let f = frac.clamp(0.0, 1.0);
        let cur = self.suspended(from.0, from.1);
        if cur <= 0.0 || f <= 0.0 {
            return;
        }
        let moved = cur * f;
        *self.load.entry(from).or_insert(0.0) -= moved;
        *self.load.entry(to).or_insert(0.0) += moved;
        if self.suspended(from.0, from.1) <= 1e-6 {
            self.load.remove(&from);
        }
    }

    /// Where the current can't hold all the suspended material (`capacity` = how much this column's
    /// flow can keep aloft), drop the excess. Returns the number of WHOLE blocks to deposit as bed
    /// at (x,z); fractional remainder stays suspended. Conserves mass (deposited + remainder = old
    /// load).
    pub fn deposit_excess(&mut self, x: i32, z: i32, capacity: f32) -> u32 {
        let cur = self.suspended(x, z);
        let cap = capacity.max(0.0);
        if cur <= cap {
            return 0;
        }
        let excess = cur - cap;
        let blocks = excess.floor() as u32;
        if blocks == 0 {
            return 0;
        }
        let new_load = cur - blocks as f32;
        if new_load <= 1e-6 {
            self.load.remove(&(x, z));
        } else {
            self.load.insert((x, z), new_load);
        }
        self.deposited_blocks += blocks as u64;
        blocks
    }

    /// Mass-balance check: every eroded unit is either still suspended or deposited as a block.
    /// Returns the imbalance (should be ~0).
    pub fn imbalance(&self) -> f64 {
        self.eroded_total - (self.total_suspended() + self.deposited_blocks as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erode_then_suspended() {
        let mut s = SedimentMap::new();
        s.erode(5, 7, 2.5);
        assert_eq!(s.suspended(5, 7), 2.5);
        assert_eq!(s.suspended(0, 0), 0.0);
    }

    #[test]
    fn advect_moves_load_downstream() {
        let mut s = SedimentMap::new();
        s.erode(0, 0, 4.0);
        s.advect((0, 0), (1, 0), 0.5);
        assert_eq!(s.suspended(0, 0), 2.0);
        assert_eq!(s.suspended(1, 0), 2.0);
    }

    #[test]
    fn deposit_drops_whole_blocks_keeps_remainder() {
        let mut s = SedimentMap::new();
        s.erode(3, 3, 2.7);
        let placed = s.deposit_excess(3, 3, 0.0); // flow holds nothing → drop all whole blocks
        assert_eq!(placed, 2);
        assert!((s.suspended(3, 3) - 0.7).abs() < 1e-5);
    }

    #[test]
    fn deposit_below_capacity_keeps_everything() {
        let mut s = SedimentMap::new();
        s.erode(1, 1, 0.9);
        assert_eq!(s.deposit_excess(1, 1, 2.0), 0);
        assert_eq!(s.suspended(1, 1), 0.9);
    }

    #[test]
    fn mass_is_conserved_through_a_cycle() {
        let mut s = SedimentMap::new();
        s.erode(0, 0, 5.3);
        s.advect((0, 0), (1, 0), 0.5);
        s.advect((1, 0), (2, 0), 1.0);
        s.deposit_excess(2, 0, 0.5);
        // everything eroded is either still suspended or deposited
        assert!(s.imbalance().abs() < 1e-4, "imbalance {}", s.imbalance());
    }
}
