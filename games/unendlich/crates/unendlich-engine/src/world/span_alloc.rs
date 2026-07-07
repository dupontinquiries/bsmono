//! `span_alloc` — best-fit byte-span allocator with free-coalescing.
//!
//! Worker-03 / WORLD-PERF pure core, round 4. This is the pure core behind the
//! GPU octree pool that fragments in the perf logs (2.6 MB free but the largest
//! contiguous span only ~72 KB, so a fresh octree upload fails even though the
//! bytes exist). It allocates byte spans out of a fixed arena, **coalesces
//! adjacent free spans on release**, and answers `largest_free_span` exactly.
//!
//! Bugs here become GPU crashes, so the invariants are property-tested hard:
//! the free list is always sorted, disjoint, fully coalesced (no two free spans
//! touch), and within the arena.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// A contiguous byte span `[offset, offset + size)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub offset: u64,
    pub size: u64,
}

impl Span {
    pub fn end(&self) -> u64 {
        self.offset + self.size
    }
}

/// Best-fit allocator over a fixed arena. Free spans are kept sorted by offset,
/// disjoint, and coalesced.
#[derive(Clone, Debug)]
pub struct SpanAllocator {
    capacity: u64,
    free: Vec<Span>,
}

impl SpanAllocator {
    pub fn new(capacity: u64) -> Self {
        let free = if capacity > 0 {
            vec![Span { offset: 0, size: capacity }]
        } else {
            Vec::new()
        };
        SpanAllocator { capacity, free }
    }

    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// Total free bytes across all free spans.
    pub fn total_free(&self) -> u64 {
        self.free.iter().map(|s| s.size).sum()
    }

    /// Size of the largest single contiguous free span (0 if none).
    pub fn largest_free_span(&self) -> u64 {
        self.free.iter().map(|s| s.size).max().unwrap_or(0)
    }

    /// Whether an allocation of `size` could currently succeed.
    pub fn can_alloc(&self, size: u64) -> bool {
        size > 0 && self.free.iter().any(|s| s.size >= size)
    }

    /// Allocate `size` bytes best-fit (smallest free span that fits), carving
    /// from the front of that span. Returns the allocated span, or `None` if no
    /// single free span is large enough (i.e. iff `!can_alloc(size)`).
    pub fn alloc(&mut self, size: u64) -> Option<Span> {
        if size == 0 {
            return None;
        }
        let mut best: Option<usize> = None;
        for (i, f) in self.free.iter().enumerate() {
            if f.size >= size {
                match best {
                    None => best = Some(i),
                    Some(bi) => {
                        if f.size < self.free[bi].size {
                            best = Some(i);
                        }
                    }
                }
            }
        }
        let i = best?;
        let f = self.free[i];
        let out = Span { offset: f.offset, size };
        if f.size == size {
            self.free.remove(i);
        } else {
            self.free[i] = Span { offset: f.offset + size, size: f.size - size };
        }
        Some(out)
    }

    /// Return a previously-allocated span to the pool, coalescing with any
    /// adjacent free spans. Precondition: `span` is currently allocated (does
    /// not overlap an existing free span). Zero-size frees are ignored.
    pub fn free(&mut self, span: Span) {
        if span.size == 0 {
            return;
        }
        // Insertion point: first free span starting at or after `span.offset`.
        let mut i = 0;
        while i < self.free.len() && self.free[i].offset < span.offset {
            i += 1;
        }
        self.free.insert(i, span);

        // Coalesce with the right neighbor, then the left neighbor.
        if i + 1 < self.free.len() {
            let cur = self.free[i];
            let nxt = self.free[i + 1];
            if cur.end() == nxt.offset {
                self.free[i] = Span { offset: cur.offset, size: cur.size + nxt.size };
                self.free.remove(i + 1);
            }
        }
        if i > 0 {
            let prev = self.free[i - 1];
            let cur = self.free[i];
            if prev.end() == cur.offset {
                self.free[i - 1] = Span { offset: prev.offset, size: prev.size + cur.size };
                self.free.remove(i);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invariant: free list sorted, disjoint, fully coalesced, within capacity.
    fn check_invariants(a: &SpanAllocator) {
        let mut prev_end = 0u64;
        for (k, s) in a.free.iter().enumerate() {
            assert!(s.size > 0, "zero-size free span");
            assert!(s.end() <= a.capacity(), "free span exceeds capacity");
            if k > 0 {
                assert!(s.offset > prev_end, "free spans not sorted/disjoint/coalesced");
            }
            prev_end = s.end();
        }
    }

    fn overlaps(a: Span, b: Span) -> bool {
        a.offset < b.end() && b.offset < a.end()
    }

    #[test]
    fn empty_arena_full_free() {
        let a = SpanAllocator::new(1024);
        assert_eq!(a.total_free(), 1024);
        assert_eq!(a.largest_free_span(), 1024);
        check_invariants(&a);
    }

    #[test]
    fn alloc_fails_iff_nothing_fits() {
        let mut a = SpanAllocator::new(100);
        assert!(a.alloc(0).is_none()); // zero rejected
        assert!(a.alloc(101).is_none()); // too big
        let s = a.alloc(100).unwrap(); // exact fit
        assert_eq!(s, Span { offset: 0, size: 100 });
        assert!(a.alloc(1).is_none()); // arena empty now
        assert_eq!(a.total_free(), 0);
        assert_eq!(a.largest_free_span(), 0);
    }

    #[test]
    fn returned_spans_never_overlap() {
        let mut a = SpanAllocator::new(1000);
        let mut live: Vec<Span> = Vec::new();
        for sz in [100, 50, 200, 33, 17] {
            if let Some(s) = a.alloc(sz) {
                for &o in &live {
                    assert!(!overlaps(s, o), "allocated spans overlap");
                }
                assert!(s.end() <= 1000);
                live.push(s);
            }
        }
    }

    #[test]
    fn free_coalesces_adjacent() {
        let mut a = SpanAllocator::new(300);
        let s0 = a.alloc(100).unwrap(); // [0,100)
        let s1 = a.alloc(100).unwrap(); // [100,200)
        let s2 = a.alloc(100).unwrap(); // [200,300)
        assert_eq!(a.total_free(), 0);
        // Free the outer two then the middle: everything must coalesce to one.
        a.free(s0);
        a.free(s2);
        check_invariants(&a);
        assert_eq!(a.free.len(), 2); // [0,100) and [200,300), gap in the middle
        a.free(s1); // fills the gap → single span
        check_invariants(&a);
        assert_eq!(a.free.len(), 1);
        assert_eq!(a.free[0], Span { offset: 0, size: 300 });
    }

    #[test]
    fn full_free_restores_empty_arena() {
        let mut a = SpanAllocator::new(512);
        let mut live = Vec::new();
        while let Some(s) = a.alloc(64) {
            live.push(s);
        }
        assert_eq!(a.total_free(), 0);
        for s in live {
            a.free(s);
        }
        check_invariants(&a);
        assert_eq!(a.total_free(), 512);
        assert_eq!(a.largest_free_span(), 512);
        assert_eq!(a.free.len(), 1);
    }

    #[test]
    fn largest_free_span_is_exact() {
        let mut a = SpanAllocator::new(1000);
        let s0 = a.alloc(100).unwrap();
        let _s1 = a.alloc(100).unwrap();
        let s2 = a.alloc(100).unwrap();
        a.free(s0); // hole [0,100)
        a.free(s2); // hole [200,300) coalesces with the tail [300,1000) → [200,1000)=800
        // brute-force max
        let brute = a.free.iter().map(|s| s.size).max().unwrap_or(0);
        assert_eq!(a.largest_free_span(), brute);
        assert_eq!(a.largest_free_span(), 800);
    }

    #[test]
    fn prop_random_sequence_preserves_accounting_and_disjointness() {
        // Deterministic LCG (no rng). Mixed alloc/free; check total-free
        // accounting and non-overlap of live allocations at every step.
        let cap = 4096u64;
        let mut a = SpanAllocator::new(cap);
        let mut live: Vec<Span> = Vec::new();
        let mut allocated_bytes = 0u64;
        let mut state: u64 = 0x1234_5678;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            state >> 33
        };
        for _ in 0..4000 {
            let do_alloc = (next() & 1) == 0 || live.is_empty();
            if do_alloc {
                let sz = (next() % 300) + 1; // 1..=300
                if let Some(s) = a.alloc(sz) {
                    // non-overlap vs all live
                    for &o in &live {
                        assert!(!overlaps(s, o));
                    }
                    assert!(s.end() <= cap);
                    assert_eq!(s.size, sz);
                    allocated_bytes += s.size;
                    live.push(s);
                } else {
                    // alloc failed ⇒ no single free span fits
                    assert!(!a.can_alloc(sz));
                }
            } else {
                let idx = (next() as usize) % live.len();
                let s = live.swap_remove(idx);
                allocated_bytes -= s.size;
                a.free(s);
            }
            // Accounting + invariants hold at every step.
            assert_eq!(a.total_free(), cap - allocated_bytes);
            check_invariants(&a);
        }
        // Drain everything → arena restored.
        for s in live.drain(..) {
            a.free(s);
        }
        assert_eq!(a.total_free(), cap);
        assert_eq!(a.largest_free_span(), cap);
    }
}
