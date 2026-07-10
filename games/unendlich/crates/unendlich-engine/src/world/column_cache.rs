//! `world::column_cache` — a small LRU cache of per-column generation *profiles*,
//! so a vertical stack of chunks reuses the shared column work instead of redoing
//! it for every Y level.
//!
//! Terrain generation is mostly a function of the horizontal `(x, z)` column:
//! surface height, biome, climate, the density-noise seed for that column. A chunk
//! at `(cx, cy, cz)` needs the same column profiles as its neighbours directly above
//! and below — but a naive generator recomputes them for every `cy`. Caching the
//! partial "column profile" and reusing it across the vertical stack (and across a
//! revisited region) removes that redundant front-end work, feeding the mesher and
//! GPU faster.
//!
//! This is the cache itself — generic over the profile type `P`, pure and
//! deterministic, LRU-bounded so it can't grow without limit. The generator calls
//! [`ColumnCache::get_or_compute`] with the expensive column build as the closure;
//! on a hit the closure never runs.

use std::collections::HashMap;

/// An LRU cache keyed by column coordinate `(x, z)` → profile `P`.
pub struct ColumnCache<P> {
    map: HashMap<(i32, i32), Entry<P>>,
    capacity: usize,
    clock: u64,
    hits: u64,
    misses: u64,
}

struct Entry<P> {
    profile: P,
    last_used: u64,
}

impl<P: Clone> ColumnCache<P> {
    /// Create a cache holding at most `capacity` column profiles (min 1).
    pub fn new(capacity: usize) -> ColumnCache<P> {
        ColumnCache {
            map: HashMap::new(),
            capacity: capacity.max(1),
            clock: 0,
            hits: 0,
            misses: 0,
        }
    }

    /// Return the profile for `col`, computing and caching it on a miss. On a hit
    /// the profile is returned (cloned) and `compute` is **not** called. Touching an
    /// entry makes it most-recently-used; a miss that overflows capacity evicts the
    /// least-recently-used column.
    pub fn get_or_compute(&mut self, col: (i32, i32), compute: impl FnOnce() -> P) -> P {
        self.clock += 1;
        let now = self.clock;
        if let Some(e) = self.map.get_mut(&col) {
            e.last_used = now;
            self.hits += 1;
            return e.profile.clone();
        }
        // Miss.
        self.misses += 1;
        let profile = compute();
        if self.map.len() >= self.capacity && !self.map.contains_key(&col) {
            self.evict_lru();
        }
        self.map.insert(col, Entry { profile: profile.clone(), last_used: now });
        profile
    }

    /// Peek without computing or affecting recency — for tests/metrics.
    pub fn contains(&self, col: (i32, i32)) -> bool {
        self.map.contains_key(&col)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Cache hit rate `0..1` over the cache's lifetime — a metric worth logging to
    /// confirm the vertical-stack reuse is landing.
    pub fn hit_rate(&self) -> f32 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f32 / total as f32
        }
    }

    pub fn hits(&self) -> u64 {
        self.hits
    }
    pub fn misses(&self) -> u64 {
        self.misses
    }

    fn evict_lru(&mut self) {
        if let Some((&victim, _)) = self.map.iter().min_by_key(|(_, e)| e.last_used) {
            self.map.remove(&victim);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn miss_computes_hit_reuses() {
        let mut c: ColumnCache<u32> = ColumnCache::new(8);
        let calls = Cell::new(0);

        let p1 = c.get_or_compute((0, 0), || {
            calls.set(calls.get() + 1);
            42
        });
        assert_eq!(p1, 42);
        // Second access to the same column is a hit — closure must not run again.
        let p2 = c.get_or_compute((0, 0), || {
            calls.set(calls.get() + 1);
            999
        });
        assert_eq!(p2, 42, "cached value returned, not recomputed");
        assert_eq!(calls.get(), 1, "compute ran exactly once");
        assert_eq!(c.hits(), 1);
        assert_eq!(c.misses(), 1);
    }

    #[test]
    fn vertical_stack_reuses_the_column() {
        // Simulate generating a stack of 8 chunks over one column: one compute, 7 hits.
        let mut c: ColumnCache<i32> = ColumnCache::new(16);
        let computes = Cell::new(0);
        for _cy in 0..8 {
            c.get_or_compute((5, 9), || {
                computes.set(computes.get() + 1);
                123
            });
        }
        assert_eq!(computes.get(), 1, "the whole stack shares one column compute");
        assert!((c.hit_rate() - 7.0 / 8.0).abs() < 1e-6);
    }

    #[test]
    fn lru_evicts_least_recently_used() {
        let mut c: ColumnCache<i32> = ColumnCache::new(2);
        c.get_or_compute((0, 0), || 0); // [A]
        c.get_or_compute((1, 1), || 1); // [A, B]
        // Touch A so B becomes the LRU.
        c.get_or_compute((0, 0), || 0);
        // Insert C → evicts B (the least recently used), keeps A and C.
        c.get_or_compute((2, 2), || 2);
        assert!(c.contains((0, 0)), "A was touched, stays");
        assert!(c.contains((2, 2)), "C just inserted");
        assert!(!c.contains((1, 1)), "B evicted as LRU");
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn recomputes_after_eviction() {
        let mut c: ColumnCache<i32> = ColumnCache::new(1);
        let computes = Cell::new(0);
        let mut go = |col| {
            c.get_or_compute(col, || {
                computes.set(computes.get() + 1);
                7
            })
        };
        go((0, 0)); // compute
        go((1, 1)); // compute (evicts 0,0)
        go((0, 0)); // recompute — was evicted
        assert_eq!(computes.get(), 3);
    }

    #[test]
    fn capacity_is_never_exceeded() {
        let mut c: ColumnCache<i32> = ColumnCache::new(4);
        for i in 0..100 {
            c.get_or_compute((i, i), || i);
            assert!(c.len() <= 4, "cache stays bounded");
        }
    }

    #[test]
    fn empty_hit_rate_is_zero() {
        let c: ColumnCache<i32> = ColumnCache::new(4);
        assert_eq!(c.hit_rate(), 0.0);
        assert!(c.is_empty());
    }
}
