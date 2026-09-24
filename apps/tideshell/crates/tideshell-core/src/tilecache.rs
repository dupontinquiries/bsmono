//! What a software drawer should keep, and what it should just redraw.
//!
//! The instinct to cache rasterised tiles is right only for a narrow class
//! of them, and getting the class wrong makes things slower:
//!
//! * **A solid span is not worth caching.** Regenerating it is `memset`,
//!   which is the fastest thing the machine does; reading it back from DRAM
//!   is a cache miss per line.
//! * **Nor is a gradient.** It is a sequential add per pixel with perfect
//!   locality. Reading the same pixels back costs more than computing them,
//!   which inverts the usual assumption about decompression.
//! * **Glyphs are already cached** by the text layer's own atlas. A second
//!   cache underneath it duplicates the memory and gives two eviction
//!   policies something to disagree about.
//!
//! That leaves irregular content that is expensive to produce and reused
//! across frames, which is the only thing [`TileCache`] admits.
//!
//! ## The budget is the last-level cache, not the screen
//!
//! Sizing a cache from the framebuffer's pixel count is the obvious move and
//! the wrong one: the point of caching is to avoid DRAM, so a cache that
//! does not fit in the LLC is a slower way to reach the same memory. The
//! budget here is a fraction of the LLC, and on a machine where that is
//! unknown it stays deliberately small — a cache that is too small loses
//! some hits, a cache that is too large loses all of them.

/// How a tile's pixels are produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    /// One colour. Regenerate: it is a memset.
    Solid,
    /// A linear ramp. Regenerate: sequential, no misses.
    Gradient,
    /// Text, icons, photographs, anything irregular. Worth keeping.
    Complex,
}

impl Fill {
    /// Is keeping the pixels ever worth it?
    pub fn cacheable(self) -> bool {
        matches!(self, Fill::Complex)
    }
}

/// A cache budget in bytes.
///
/// `llc_bytes` is the last-level cache if it is known. The fraction is well
/// under 1 because the tile cache is not the only thing that wants to live
/// there — the framebuffer being written streams through it too.
pub fn budget_bytes(llc_bytes: Option<usize>) -> usize {
    match llc_bytes {
        Some(l) if l > 0 => (l / 4).max(64 * 1024),
        // Unknown, or a nonsense report of zero: small on purpose. Too small
        // loses some hits; too large loses all of them, because the cache
        // stops fitting in the LLC it exists to stay inside, and adds
        // eviction work on top.
        _ => 512 * 1024,
    }
}

/// One cached tile's bookkeeping. The pixels live in the drawer; this is the
/// policy's view of them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entry {
    pub bytes: usize,
    /// Rough cost to produce again, in arbitrary units the caller keeps
    /// consistent (microseconds work well).
    pub regen_cost: u32,
    pub hits: u32,
    pub last_used: u64,
}

impl Entry {
    /// What this entry is worth per byte it occupies.
    ///
    /// Cost times use, divided by size: a small expensive tile used often
    /// beats a large cheap one used once, which is what a plain LRU gets
    /// wrong when entries differ wildly in size.
    pub fn density(&self) -> f32 {
        (self.regen_cost as f32 * (1 + self.hits) as f32) / self.bytes.max(1) as f32
    }
}

#[derive(Debug, Default)]
pub struct TileCache {
    pub budget: usize,
    entries: Vec<(u64, Entry)>,
    used: usize,
}

impl TileCache {
    pub fn new(budget: usize) -> TileCache {
        TileCache { budget, entries: Vec::new(), used: 0 }
    }

    pub fn used_bytes(&self) -> usize {
        self.used
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn contains(&self, key: u64) -> bool {
        self.entries.iter().any(|(k, _)| *k == key)
    }

    /// Note a hit, so the entry's density reflects it.
    pub fn touch(&mut self, key: u64, now: u64) -> bool {
        if let Some((_, e)) = self.entries.iter_mut().find(|(k, _)| *k == key) {
            e.hits = e.hits.saturating_add(1);
            e.last_used = now;
            return true;
        }
        false
    }

    /// Offer a tile. Returns the keys evicted to make room, or `None` if the
    /// tile was not admitted at all.
    pub fn insert(&mut self, key: u64, fill: Fill, e: Entry) -> Option<Vec<u64>> {
        if !fill.cacheable() || e.bytes == 0 {
            return None;
        }
        // One tile may not take more than a quarter of the cache, or a
        // single big surface flushes everything that was working.
        if e.bytes * 4 > self.budget {
            return None;
        }
        let mut evicted = Vec::new();
        while self.used + e.bytes > self.budget {
            // Drop the least value per byte. Ties go to the older entry.
            let Some(i) = (0..self.entries.len()).min_by(|a, b| {
                let (ea, eb) = (&self.entries[*a].1, &self.entries[*b].1);
                ea.density().partial_cmp(&eb.density()).unwrap().then(ea.last_used.cmp(&eb.last_used))
            }) else {
                break;
            };
            let (k, victim) = self.entries.remove(i);
            self.used -= victim.bytes;
            evicted.push(k);
        }
        if self.used + e.bytes > self.budget {
            return None;
        }
        self.used += e.bytes;
        self.entries.push((key, e));
        Some(evicted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(bytes: usize, cost: u32) -> Entry {
        Entry { bytes, regen_cost: cost, hits: 0, last_used: 0 }
    }

    #[test]
    fn only_irregular_content_is_kept() {
        let mut c = TileCache::new(1 << 20);
        assert!(c.insert(1, Fill::Solid, e(4096, 100)).is_none(), "a memset is cheaper than a DRAM read");
        assert!(c.insert(2, Fill::Gradient, e(4096, 100)).is_none(), "so is a sequential add");
        assert!(c.insert(3, Fill::Complex, e(4096, 100)).is_some());
        assert_eq!(c.len(), 1);
        assert_eq!(c.used_bytes(), 4096);
    }

    #[test]
    fn the_budget_follows_the_cache_hierarchy_not_the_screen() {
        // A 4 MB LLC gets 1 MB; an unknown machine gets a small, safe cache
        // rather than a large one that cannot possibly be resident.
        assert_eq!(budget_bytes(Some(4 << 20)), 1 << 20);
        assert_eq!(budget_bytes(None), 512 * 1024);
        assert!(budget_bytes(Some(1024)) >= 64 * 1024, "a floor, so the cache is not pointless");
    }

    #[test]
    fn value_per_byte_decides_not_recency_alone() {
        // Every tile here respects the quarter-of-the-budget rule, which is
        // the other half of the policy: a test that violated it would be
        // measuring the refusal rather than the eviction order.
        let mut c = TileCache::new(10_000);
        // Small, expensive, well used: the one that must survive.
        c.insert(1, Fill::Complex, Entry { bytes: 1_000, regen_cost: 500, hits: 20, last_used: 0 }).unwrap();
        // Cheap and unused, even though it arrived later.
        c.insert(2, Fill::Complex, Entry { bytes: 2_000, regen_cost: 10, hits: 0, last_used: 100 }).unwrap();
        for k in 3..6u64 {
            c.insert(k, Fill::Complex, Entry { bytes: 2_000, regen_cost: 200, hits: 3, last_used: k }).unwrap();
        }
        let evicted = c.insert(9, Fill::Complex, Entry { bytes: 2_000, regen_cost: 300, hits: 5, last_used: 200 }).unwrap();
        assert_eq!(evicted, vec![2], "the cheap unused one goes, not the old valuable one");
        assert!(c.contains(1) && c.contains(9));
    }

    #[test]
    fn one_tile_cannot_flush_the_whole_cache() {
        let mut c = TileCache::new(10_000);
        for k in 0..4 {
            c.insert(k, Fill::Complex, e(2_000, 100)).unwrap();
        }
        // A tile larger than a quarter of the budget is refused outright,
        // rather than evicting everything that was working to hold it.
        assert!(c.insert(99, Fill::Complex, e(6_000, 1_000)).is_none());
        assert_eq!(c.len(), 4, "the working set survived");
    }

    #[test]
    fn the_budget_is_never_exceeded() {
        let mut c = TileCache::new(20_000);
        for k in 0..50u64 {
            let _ = c.insert(k, Fill::Complex, Entry { bytes: 1_000 + (k as usize % 5) * 500, regen_cost: 50 + k as u32, hits: (k % 3) as u32, last_used: k });
            assert!(c.used_bytes() <= c.budget, "over budget at {k}: {}", c.used_bytes());
        }
        assert!(!c.is_empty());
    }

    #[test]
    fn a_hit_raises_an_entrys_worth() {
        let mut c = TileCache::new(10_000);
        c.insert(1, Fill::Complex, e(1_000, 100)).unwrap();
        assert!(c.touch(1, 5));
        assert!(!c.touch(404, 5), "a miss says so");
        // Two entries identical but for use: the used one must outlive the
        // other, which is the whole reason hits are in the density.
        c.insert(2, Fill::Complex, e(1_000, 100)).unwrap();
        for _ in 0..10 {
            c.touch(1, 6);
        }
        for k in 3..10u64 {
            c.insert(k, Fill::Complex, e(1_000, 150)).unwrap();
        }
        let evicted = c.insert(20, Fill::Complex, e(2_000, 150)).unwrap();
        assert!(evicted.contains(&2), "the untouched one goes first: {evicted:?}");
        assert!(!evicted.contains(&1), "the touched one stays: {evicted:?}");
    }
}
