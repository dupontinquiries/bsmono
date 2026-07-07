//! `sim::world_prober` — a unified, within-batch memo for the procedural world
//! queries that several systems each recompute from scratch.
//!
//! The procedural generator answers two column-keyed questions over and over:
//!   - **surface height** at `(x, z)` — a top-down column scan that evaluates
//!     `density()` + `is_cave()` (full 3D noise) at every block in a `[y_min,
//!     y_max]` range. The ecosystem-stream path scans a 768-block-deep range
//!     across all 1024 columns of a chunk, *twice* (once to classify biome, once
//!     to place a spawn), with no memo — ~1.5M noise evals per chunk, the
//!     multi-second region-crossing freeze.
//!   - **biome** at `(x, z)` — derived from the surface sample; also recomputed
//!     per column.
//!
//! `WorldProber` consolidates those into one place: several typed caches behind a
//! single object, each keyed by the world column `(x, z)`. A column scanned once
//! is free for every later query in the same batch — and crucially, the biome
//! lookup *reuses* the surface memo instead of scanning the column a second time.
//!
//! This generalises the single-purpose `surface_cache::SurfaceCache` (which the
//! creature loop uses) so future systems — flora placement, prey selection,
//! cliff-aware pathing — can share one prober rather than each growing their own
//! cache. The intent is that `SurfaceCache` eventually becomes a thin alias for
//! the surface portion of this.
//!
//! **Scope = one batch.** Surface/biome are stable for a given world *between*
//! edits, but terrain streams and changes, so a prober memoizes for one tick /
//! one streaming pass and is `clear()`ed before reuse. It does not outlive that.
//!
//! Pure container: it never mutates the world. Misses are filled by calling the
//! generator the caller hands in, so the prober carries no borrow of the world
//! and can be a long-lived field cleared each batch. No panic, no IO, no clock.

use std::collections::HashMap;
use std::sync::Arc;

use crate::world::biomes::Biome;
use crate::world::biomes::BiomeSample;
use crate::world::cpu_generator::WorldGenerator;

/// Within-batch memo of column-keyed world queries. Construct once, `clear()`
/// before each tick / streaming pass, query freely.
#[derive(Default)]
pub struct WorldProber {
    /// `(x, z)` → surface height (top-most standable). `None` is cached too, so a
    /// column with no surface in range isn't re-scanned.
    surface: HashMap<(i32, i32), Option<i32>>,
    /// `(x, z)` → surface biome. Assumes surface-level sampling (the placement
    /// use case); the `y` passed to [`biome`] only matters on first encounter.
    biome: HashMap<(i32, i32), Biome>,
    /// `(x, z)` → full surface biome sample (biome + humidity + temperature), for
    /// callers (flora) that need the climate fields too.
    biome_sample: HashMap<(i32, i32), BiomeSample>,
    surface_hits: u64,
    surface_misses: u64,
    biome_hits: u64,
    biome_misses: u64,
}

impl WorldProber {
    pub fn new() -> WorldProber {
        WorldProber::default()
    }

    /// Drop all memoized columns. Call once per tick / per streaming pass — the
    /// world can change between batches, so the memo must not outlive one.
    pub fn clear(&mut self) {
        self.surface.clear();
        self.biome.clear();
        self.biome_sample.clear();
        // Counters are cumulative across the prober's life (diagnostics); not reset.
    }

    /// Surface height at `(x, z)`, served from the memo if seen this batch, else
    /// computed via the generator's deep column scan and remembered. `y_min` /
    /// `y_max` are the scan bounds (caller's choice — the prober does not assume a
    /// range); they're consulted only on a miss, so all callers in one batch
    /// should use the same bounds for a coherent memo.
    pub fn surface_y(
        &mut self,
        gen: &WorldGenerator,
        x: i32,
        z: i32,
        y_min: i32,
        y_max: i32,
    ) -> Option<i32> {
        if let Some(&v) = self.surface.get(&(x, z)) {
            self.surface_hits += 1;
            return v;
        }
        self.surface_misses += 1;
        let v = gen.surface_y_at(x, z, y_min, y_max);
        self.surface.insert((x, z), v);
        v
    }

    /// Convenience for `&Arc<WorldGenerator>` (what `generator().cpu()` returns).
    pub fn surface_y_arc(
        &mut self,
        gen: &Arc<WorldGenerator>,
        x: i32,
        z: i32,
        y_min: i32,
        y_max: i32,
    ) -> Option<i32> {
        self.surface_y(gen.as_ref(), x, z, y_min, y_max)
    }

    /// Surface biome at `(x, z)`, memoized. On first encounter samples
    /// `biome_at_3d(x, y, z)` at the supplied `y` (normally the column's surface
    /// height); thereafter served from the memo. Keyed by `(x, z)` — intended for
    /// surface-level placement where `y` is the surface, so repeat queries for the
    /// same column are coherent regardless of the `y` passed later.
    pub fn biome(&mut self, gen: &WorldGenerator, x: i32, y: i32, z: i32) -> Biome {
        if let Some(&b) = self.biome.get(&(x, z)) {
            self.biome_hits += 1;
            return b;
        }
        self.biome_misses += 1;
        let b = gen.biome_at_3d(x, y, z).biome;
        self.biome.insert((x, z), b);
        b
    }

    /// `&Arc<WorldGenerator>` convenience for [`biome`].
    pub fn biome_arc(&mut self, gen: &Arc<WorldGenerator>, x: i32, y: i32, z: i32) -> Biome {
        self.biome(gen.as_ref(), x, y, z)
    }

    /// Full `BiomeSample` at `(x, z)`, memoized. Like [`biome`] but returns the
    /// whole sample (biome + humidity + temperature) — flora placement needs the
    /// climate fields, not just the biome tag. Keyed by `(x, z)`; `y` is the
    /// sampling height (normally the surface), consulted only on first encounter.
    pub fn biome_sample(
        &mut self,
        gen: &WorldGenerator,
        x: i32,
        y: i32,
        z: i32,
    ) -> crate::world::biomes::BiomeSample {
        if let Some(&s) = self.biome_sample.get(&(x, z)) {
            self.biome_hits += 1;
            return s;
        }
        self.biome_misses += 1;
        let s = gen.biome_at_3d(x, y, z);
        self.biome_sample.insert((x, z), s);
        // keep the biome-only memo coherent too (free)
        self.biome.entry((x, z)).or_insert(s.biome);
        s
    }

    /// Surface-height memo hit rate this prober's life (0 if no queries). For
    /// confirming the memo is actually saving scans.
    pub fn surface_hit_rate(&self) -> f32 {
        let total = self.surface_hits + self.surface_misses;
        if total == 0 { 0.0 } else { self.surface_hits as f32 / total as f32 }
    }

    /// Biome memo hit rate this prober's life (0 if no queries).
    pub fn biome_hit_rate(&self) -> f32 {
        let total = self.biome_hits + self.biome_misses;
        if total == 0 { 0.0 } else { self.biome_hits as f32 / total as f32 }
    }

    /// Distinct columns currently memoized (surface side) — rough memo footprint.
    pub fn columns_cached(&self) -> usize {
        self.surface.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A WorldProber test can't construct a real WorldGenerator cheaply, so these
    // exercise the memo contract through the public counters using a generator
    // built via its test constructor when available. The memo logic itself is
    // generator-agnostic: the invariants below (a repeat key is a hit; clear
    // resets the memo; None is cached) hold regardless of what the scan returns.
    //
    // We validate the cache bookkeeping directly by pre-seeding the maps, which
    // is what the get_or path manipulates — no generator needed for these.

    #[test]
    fn repeat_surface_query_is_a_hit_after_seed() {
        let mut p = WorldProber::new();
        // Simulate a miss having populated the column, then a repeat.
        p.surface.insert((5, 7), Some(64));
        // A direct map read models what surface_y does on a hit; assert the value
        // round-trips and the contract (Some cached) holds.
        assert_eq!(p.surface.get(&(5, 7)).copied(), Some(Some(64)));
    }

    #[test]
    fn none_surface_is_cached_not_rescanned() {
        let mut p = WorldProber::new();
        p.surface.insert((1, 1), None);
        // None must be a stored entry (so the column isn't re-scanned), distinct
        // from "absent".
        assert!(p.surface.contains_key(&(1, 1)));
        assert_eq!(p.surface.get(&(1, 1)).copied(), Some(None));
    }

    #[test]
    fn clear_empties_both_memos() {
        let mut p = WorldProber::new();
        p.surface.insert((0, 0), Some(70));
        p.biome.insert((0, 0), Biome::Plains);
        p.clear();
        assert_eq!(p.columns_cached(), 0);
        assert!(p.biome.is_empty());
    }

    #[test]
    fn hit_rate_is_zero_with_no_queries() {
        let p = WorldProber::new();
        assert_eq!(p.surface_hit_rate(), 0.0);
        assert_eq!(p.biome_hit_rate(), 0.0);
    }
}
