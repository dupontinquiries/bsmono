//! A within-tick memo of surface heights. `surface_y_at` is a top-down column
//! scan (tens of voxel samples per call); within one tick a cluster of roamers
//! each probes several *nearby* columns — posture samples four around the feet,
//! terrain steering fans out a handful, foraging adds more — so the same `(x,z)`
//! column gets scanned many times. Memoizing per tick collapses that to one scan
//! per distinct column. Clear it at the top of each tick.
//!
//! Pure container (the expensive scan is supplied by the caller via `get_or`),
//! so it's testable: a repeated query is a hit and never re-runs the closure.

use std::collections::HashMap;

/// Caches `surface_y_at(x, z)` results (including `None`) for the current tick.
#[derive(Default)]
pub struct SurfaceCache {
    map: HashMap<(i32, i32), Option<i32>>,
    /// Served-from-cache count (diagnostics).
    pub hits: u64,
    /// Cache-miss (scan ran) count (diagnostics).
    pub misses: u64,
}

impl SurfaceCache {
    pub fn new() -> SurfaceCache {
        SurfaceCache { map: HashMap::new(), hits: 0, misses: 0 }
    }

    /// Drop all memoized columns. Call once per tick — the world is mutated between ticks (erosion,
    /// water, builder dragons, plant growth …), and grounding may become terrain-aware, so the memo
    /// is intentionally kept to a single tick rather than persisted.
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Surface height at `(x, z)`, served from the memo if seen this tick, else computed via `scan`
    /// (the real column scan) and remembered for the rest of the tick.
    pub fn get_or<F: FnOnce() -> Option<i32>>(&mut self, x: i32, z: i32, scan: F) -> Option<i32> {
        if let Some(v) = self.map.get(&(x, z)) {
            self.hits += 1;
            return *v;
        }
        self.misses += 1;
        let v = scan();
        self.map.insert((x, z), v);
        v
    }

    /// Fraction of queries served from the memo this tick (0 if none yet).
    pub fn hit_rate(&self) -> f32 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f32 / total as f32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn repeat_query_is_a_hit_and_does_not_rescan() {
        let mut c = SurfaceCache::new();
        let scans = Cell::new(0);
        let mut probe = |c: &mut SurfaceCache, x, z| {
            c.get_or(x, z, || {
                scans.set(scans.get() + 1);
                Some(72)
            })
        };
        assert_eq!(probe(&mut c, 4, 4), Some(72));
        assert_eq!(probe(&mut c, 4, 4), Some(72)); // same column again
        assert_eq!(probe(&mut c, 4, 4), Some(72));
        assert_eq!(scans.get(), 1, "the column was scanned exactly once");
        assert_eq!(c.hits, 2);
        assert_eq!(c.misses, 1);
    }

    #[test]
    fn distinct_columns_each_scan_once() {
        let mut c = SurfaceCache::new();
        let scans = Cell::new(0);
        for (x, z) in [(0, 0), (1, 0), (0, 1), (0, 0), (1, 0)] {
            c.get_or(x, z, || {
                scans.set(scans.get() + 1);
                Some(64)
            });
        }
        assert_eq!(scans.get(), 3, "three distinct columns → three scans");
    }

    #[test]
    fn none_is_cached_too() {
        let mut c = SurfaceCache::new();
        let scans = Cell::new(0);
        let mut probe = |c: &mut SurfaceCache| {
            c.get_or(9, 9, || {
                scans.set(scans.get() + 1);
                None
            })
        };
        assert_eq!(probe(&mut c), None);
        assert_eq!(probe(&mut c), None);
        assert_eq!(scans.get(), 1, "a None result is memoized, not re-scanned");
    }

    #[test]
    fn clear_forces_a_rescan() {
        let mut c = SurfaceCache::new();
        let scans = Cell::new(0);
        c.get_or(0, 0, || {
            scans.set(scans.get() + 1);
            Some(1)
        });
        c.clear();
        c.get_or(0, 0, || {
            scans.set(scans.get() + 1);
            Some(1)
        });
        assert_eq!(scans.get(), 2, "after clear, the column is scanned again");
    }
}
