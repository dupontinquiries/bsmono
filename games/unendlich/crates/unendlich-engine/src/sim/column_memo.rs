//! `sim::column_memo` — a generic within-tick memo for repeated per-`(x, z)`
//! lookups, the generalization of [`crate::sim::surface_cache::SurfaceCache`].
//!
//! Fauna hammer the same expensive *column* query many times per tick: a cluster
//! of roamers each sample the biome / climate / moisture under their feet and a
//! few cells ahead, and `biome_at_3d` (and friends) are noise evaluations. The
//! surface-height memo already collapses `surface_y_at`; this does the same for
//! *any* `Copy` value keyed by a column, so the per-roamer biome lookup the
//! VISION doc flagged stops being recomputed once per roamer per frame.
//!
//! Pure container — the expensive evaluation is supplied by the caller via
//! [`ColumnMemo::get_or`], so it is trivially testable and carries no dependency
//! on the world generator. **Clear it at the top of each tick:** terrain and
//! climate can change between ticks (edits, streaming, season), so the memo must
//! not outlive one tick.

use std::collections::HashMap;

/// Within-tick memo of a `Copy` value keyed by a `(x, z)` column.
#[derive(Default)]
pub struct ColumnMemo<V: Copy> {
    map: HashMap<(i32, i32), V>,
    /// Served-from-memo count (diagnostics).
    pub hits: u64,
    /// Memo-miss (eval ran) count (diagnostics).
    pub misses: u64,
}

impl<V: Copy> ColumnMemo<V> {
    pub fn new() -> ColumnMemo<V> {
        ColumnMemo { map: HashMap::new(), hits: 0, misses: 0 }
    }

    /// Drop all memoized columns. Call once per tick.
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Distinct columns held this tick.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Value at `(x, z)`, served from the memo if seen this tick, else computed
    /// via `eval` (the real, expensive lookup) and remembered.
    pub fn get_or<F: FnOnce() -> V>(&mut self, x: i32, z: i32, eval: F) -> V {
        if let Some(v) = self.map.get(&(x, z)) {
            self.hits += 1;
            return *v;
        }
        self.misses += 1;
        let v = eval();
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
    fn repeat_query_is_a_hit_and_does_not_reeval() {
        let mut m: ColumnMemo<i32> = ColumnMemo::new();
        let evals = Cell::new(0);
        let probe = |m: &mut ColumnMemo<i32>, x, z| {
            m.get_or(x, z, || {
                evals.set(evals.get() + 1);
                7
            })
        };
        assert_eq!(probe(&mut m, 4, 4), 7);
        assert_eq!(probe(&mut m, 4, 4), 7); // hit
        assert_eq!(probe(&mut m, 4, 4), 7); // hit
        assert_eq!(evals.get(), 1, "the expensive eval ran exactly once");
        assert_eq!(m.hits, 2);
        assert_eq!(m.misses, 1);
    }

    #[test]
    fn distinct_columns_each_eval_once() {
        let mut m: ColumnMemo<u8> = ColumnMemo::new();
        let evals = Cell::new(0u32);
        let probe = |m: &mut ColumnMemo<u8>, x, z| {
            m.get_or(x, z, || {
                evals.set(evals.get() + 1);
                3u8
            })
        };
        probe(&mut m, 0, 0);
        probe(&mut m, 1, 0);
        probe(&mut m, 0, 1);
        assert_eq!(evals.get(), 3);
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn clear_resets_for_next_tick() {
        let mut m: ColumnMemo<i32> = ColumnMemo::new();
        m.get_or(2, 2, || 9);
        assert_eq!(m.len(), 1);
        m.clear();
        assert!(m.is_empty(), "memo must not outlive a tick");
        let evals = Cell::new(0);
        m.get_or(2, 2, || {
            evals.set(1);
            9
        });
        assert_eq!(evals.get(), 1, "after clear the same column re-evaluates");
    }

    #[test]
    fn hit_rate_reports_savings() {
        let mut m: ColumnMemo<i32> = ColumnMemo::new();
        assert_eq!(m.hit_rate(), 0.0);
        m.get_or(0, 0, || 1); // miss
        m.get_or(0, 0, || 1); // hit
        m.get_or(0, 0, || 1); // hit
        assert!((m.hit_rate() - (2.0 / 3.0)).abs() < 1e-6);
    }

    #[test]
    fn works_for_an_enum_like_value() {
        // a biome-ish Copy payload
        #[derive(Clone, Copy, PartialEq, Debug)]
        enum FakeBiome { Forest, Desert }
        let mut m: ColumnMemo<FakeBiome> = ColumnMemo::new();
        let got = m.get_or(5, 5, || FakeBiome::Forest);
        assert_eq!(got, FakeBiome::Forest);
        assert_eq!(m.get_or(5, 5, || FakeBiome::Desert), FakeBiome::Forest, "memo wins on a hit");
    }
}
