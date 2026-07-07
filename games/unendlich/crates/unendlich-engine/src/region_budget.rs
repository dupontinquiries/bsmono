//! region_budget (r206) — a per-region work-budget tracker. The operator
//! asked for "a budgeting tracker for regions": this aggregates per-chunk
//! costs (mesh time, generation time, chunk/vertex counts, bytes) into
//! 4x4x4-chunk regions so you can see WHICH regions are expensive, not
//! just a global total. Pairs with the r204 profiler (which tracks costs
//! by named zone over time); this tracks them by SPACE.
//!
//! Use: each frame, `charge()` a region for the work it did; periodically
//! `report()` the hottest regions (greppable) or query `budget_used()`
//! for a single region to drive adaptive throttling. Pure, std-only,
//! tested.

use std::collections::HashMap;

/// A region's accumulated work this window.
#[derive(Default, Clone, Copy, Debug)]
pub struct RegionCost {
    /// Wall-time spent meshing chunks in this region (ms).
    pub mesh_ms: f64,
    /// Wall-time spent generating chunks in this region (ms).
    pub gen_ms: f64,
    /// Chunks currently resident from this region.
    pub chunks: u32,
    /// Approx GPU/CPU bytes attributed to this region.
    pub bytes: u64,
}

impl RegionCost {
    /// A single scalar "weight" for ranking regions: time dominates, with
    /// a small bytes term so a heavy-but-fast region still ranks.
    pub fn weight(&self) -> f64 {
        self.mesh_ms + self.gen_ms + (self.bytes as f64) / 1.0e7
    }
}

/// Region size in chunks (edge). Matches the 4x4x4 grass/work grouping.
pub const REGION_CHUNKS: i32 = 4;

/// Map a chunk coord to its region coord (floor-divide by REGION_CHUNKS).
#[inline]
pub fn region_of_chunk(cx: i32, cy: i32, cz: i32) -> (i32, i32, i32) {
    (
        cx.div_euclid(REGION_CHUNKS),
        cy.div_euclid(REGION_CHUNKS),
        cz.div_euclid(REGION_CHUNKS),
    )
}

/// The tracker. Cheap HashMap keyed by region coord.
#[derive(Default)]
pub struct RegionBudget {
    regions: HashMap<(i32, i32, i32), RegionCost>,
}

impl RegionBudget {
    pub fn new() -> Self {
        RegionBudget { regions: HashMap::new() }
    }

    /// Add mesh time to the region containing a chunk.
    pub fn charge_mesh(&mut self, cx: i32, cy: i32, cz: i32, ms: f64) {
        self.regions.entry(region_of_chunk(cx, cy, cz)).or_default().mesh_ms += ms;
    }

    /// Add generation time to the region containing a chunk.
    pub fn charge_gen(&mut self, cx: i32, cy: i32, cz: i32, ms: f64) {
        self.regions.entry(region_of_chunk(cx, cy, cz)).or_default().gen_ms += ms;
    }

    /// Record a chunk's residency + byte footprint (call on upload).
    pub fn charge_resident(&mut self, cx: i32, cy: i32, cz: i32, bytes: u64) {
        let e = self.regions.entry(region_of_chunk(cx, cy, cz)).or_default();
        e.chunks += 1;
        e.bytes += bytes;
    }

    /// Total accumulated weight across all regions.
    pub fn total_weight(&self) -> f64 {
        self.regions.values().map(|c| c.weight()).sum()
    }

    /// Cost for one region (zero if untracked).
    pub fn region_cost(&self, region: (i32, i32, i32)) -> RegionCost {
        self.regions.get(&region).copied().unwrap_or_default()
    }

    /// The `n` heaviest regions, descending by weight. For "which part of
    /// the world is eating the frame" answers.
    pub fn hottest(&self, n: usize) -> Vec<((i32, i32, i32), RegionCost)> {
        let mut v: Vec<_> = self.regions.iter().map(|(k, c)| (*k, *c)).collect();
        v.sort_by(|a, b| b.1.weight().partial_cmp(&a.1.weight()).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(n);
        v
    }

    /// One greppable line per hot region. Format:
    /// `region=(x,y,z) mesh=Xms gen=Xms chunks=N bytes=N w=X`.
    pub fn report(&self, top_n: usize) -> String {
        let mut out = format!(
            "region_budget: {} regions, total_weight={:.1}\n",
            self.regions.len(),
            self.total_weight()
        );
        for (coord, c) in self.hottest(top_n) {
            out.push_str(&format!(
                "  region=({},{},{}) mesh={:.1}ms gen={:.1}ms chunks={} bytes={} w={:.1}\n",
                coord.0, coord.1, coord.2, c.mesh_ms, c.gen_ms, c.chunks, c.bytes, c.weight()
            ));
        }
        out
    }

    /// Reset the window (call after reporting, e.g. once a second).
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    pub fn region_count(&self) -> usize {
        self.regions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_map_to_regions() {
        // 4-chunk regions: chunks 0..3 -> region 0, 4..7 -> region 1.
        assert_eq!(region_of_chunk(0, 0, 0), (0, 0, 0));
        assert_eq!(region_of_chunk(3, 3, 3), (0, 0, 0));
        assert_eq!(region_of_chunk(4, 0, 0), (1, 0, 0));
        // negatives floor correctly (-1 -> region -1, not 0).
        assert_eq!(region_of_chunk(-1, 0, 0), (-1, 0, 0));
        assert_eq!(region_of_chunk(-4, 0, 0), (-1, 0, 0));
        assert_eq!(region_of_chunk(-5, 0, 0), (-2, 0, 0));
    }

    #[test]
    fn charges_accumulate_per_region() {
        let mut b = RegionBudget::new();
        b.charge_mesh(0, 0, 0, 5.0);
        b.charge_mesh(1, 1, 1, 3.0); // same region (0,0,0)
        b.charge_gen(0, 0, 0, 2.0);
        let c = b.region_cost((0, 0, 0));
        assert!((c.mesh_ms - 8.0).abs() < 1e-9);
        assert!((c.gen_ms - 2.0).abs() < 1e-9);
        assert_eq!(b.region_count(), 1);
    }

    #[test]
    fn hottest_ranks_by_weight() {
        let mut b = RegionBudget::new();
        b.charge_mesh(0, 0, 0, 1.0); // region 0: light
        b.charge_mesh(4, 0, 0, 50.0); // region 1: heavy
        b.charge_mesh(8, 0, 0, 10.0); // region 2: medium
        let hot = b.hottest(2);
        assert_eq!(hot.len(), 2);
        assert_eq!(hot[0].0, (1, 0, 0), "heaviest region first");
        assert_eq!(hot[1].0, (2, 0, 0), "then medium");
    }

    #[test]
    fn report_and_clear() {
        let mut b = RegionBudget::new();
        b.charge_resident(0, 0, 0, 1024);
        let r = b.report(4);
        assert!(r.contains("region=(0,0,0)"));
        assert!(r.contains("chunks=1"));
        b.clear();
        assert_eq!(b.region_count(), 0);
    }
}
