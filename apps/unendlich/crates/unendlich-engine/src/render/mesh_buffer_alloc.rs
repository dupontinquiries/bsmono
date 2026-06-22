//! Free-list span allocator.
//!
//! Allocates variable-sized spans within a fixed-size buffer. Used by
//! `mesh_buffer_pool` to back vertex and index buffers in the renderer:
//! one big GPU buffer per axis, sliced into variable-size sub-regions
//! whose sizes match each chunk's actual mesh footprint.
//!
//! # Why this exists
//!
//! v0.18.2 introduced fixed-size slots in the mesh pool. v0.18.3 raised
//! those caps to fit ~70% of real chunks. The remaining 20-30% (densest
//! weathered terrain) still overflow. The fundamental problem: real chunk
//! mesh sizes range from 21 KB to 7 MB — a 333× ratio. Any fixed slot size
//! either wastes memory on small chunks or drops large ones. A free-list
//! allocator gives each chunk exactly the bytes it needs.
//!
//! # Allocation strategy
//!
//! **Best-fit with offset-keyed free-list**, the textbook design.
//!
//! - Free spans tracked in `BTreeMap<u64 offset, u64 size>`. Sorted by
//!   offset (the natural BTreeMap key order) to make coalescing on free
//!   straightforward — neighbors in offset are neighbors in the map.
//! - Allocation: linear scan over free spans, pick the smallest one that
//!   fits the request. O(n) where n = free span count.
//! - Free: insert the span, then check both immediate neighbors in offset
//!   and merge if they're contiguous (i.e., `prev.offset + prev.size ==
//!   our.offset` or `our.offset + our.size == next.offset`).
//!
//! # Alignment
//!
//! All allocations are rounded up to ALIGNMENT bytes (256 by default).
//! 256 is `vk::PhysicalDeviceLimits::min_storage_buffer_offset_alignment`
//! on Apple Silicon and a safe upper bound for most consumer GPUs. The
//! free-list internally stores aligned offsets/sizes only — no
//! sub-alignment fragments ever exist.
//!
//! # Exhaustion
//!
//! If no free span ≥ the requested aligned size exists, allocation fails
//! with `AllocError::Exhausted`. Callers handle by dropping the mesh and
//! logging; the underlying GPU buffer is fixed-size and we can't grow it
//! mid-frame.

use std::collections::BTreeMap;

/// All allocations and free-list spans are multiples of this size in bytes.
/// 256 is the minimum-storage-buffer-offset-alignment on Apple Silicon
/// (`vk::PhysicalDeviceLimits::min_storage_buffer_offset_alignment`) and
/// covers all consumer GPUs we target.
pub const ALIGNMENT: u64 = 256;

#[inline]
pub fn align_up(n: u64) -> u64 {
    (n + ALIGNMENT - 1) & !(ALIGNMENT - 1)
}

/// Errors from allocator operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllocError {
    /// Couldn't satisfy alloc — no free span large enough.
    Exhausted { wanted: u64, free_total: u64, free_largest: u64 },
    /// Caller passed an offset that wasn't allocated, or freed twice.
    InvalidFree { offset: u64 },
}

impl std::fmt::Display for AllocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AllocError::Exhausted { wanted, free_total, free_largest } => write!(
                f,
                "Span allocator exhausted: wanted {} bytes, {} free total, largest free span {}",
                wanted, free_total, free_largest,
            ),
            AllocError::InvalidFree { offset } => write!(
                f, "Invalid free at offset {} (not allocated, or double-free)", offset,
            ),
        }
    }
}
impl std::error::Error for AllocError {}

/// One allocated span. Caller frees by passing `offset` back to `free()`.
/// `size` is the aligned size, may be larger than what was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub offset: u64,
    pub size: u64,
}

/// Best-fit free-list allocator.
///
/// Tracks allocations and free spans separately to detect double-frees
/// and report stats. Owns no actual memory — caller is responsible for
/// the underlying buffer; the allocator just hands out offsets.
#[derive(Debug)]
pub struct SpanAllocator {
    capacity: u64,
    /// Free spans, keyed by offset. Sorted by offset for coalescing.
    free: BTreeMap<u64, u64>,
    /// Allocated spans, keyed by offset. Used to validate `free()` calls
    /// and report stats. Value = size of that allocation.
    allocated: BTreeMap<u64, u64>,
    /// Running total of allocated bytes (sum of `allocated` values).
    /// Maintained incrementally to avoid a full map walk for stats.
    allocated_total: u64,
}

impl SpanAllocator {
    /// Create an allocator over a buffer of `capacity` bytes. The capacity
    /// is rounded down to ALIGNMENT — the tail bytes are unusable.
    pub fn new(capacity: u64) -> Self {
        let capacity = capacity & !(ALIGNMENT - 1);
        let mut free = BTreeMap::new();
        if capacity > 0 {
            free.insert(0, capacity);
        }
        Self {
            capacity,
            free,
            allocated: BTreeMap::new(),
            allocated_total: 0,
        }
    }

    pub fn capacity(&self) -> u64 { self.capacity }
    pub fn allocated_bytes(&self) -> u64 { self.allocated_total }
    pub fn free_bytes(&self) -> u64 { self.capacity - self.allocated_total }
    pub fn allocation_count(&self) -> usize { self.allocated.len() }
    pub fn free_span_count(&self) -> usize { self.free.len() }
    /// Largest contiguous free span, in bytes. Useful for diagnosing
    /// fragmentation: if `free_bytes()` is large but `largest_free_span()`
    /// is small, the allocator is fragmented.
    pub fn largest_free_span(&self) -> u64 {
        self.free.values().copied().max().unwrap_or(0)
    }

    /// Allocate a span of at least `size` bytes. Returns `AllocError`
    /// if no free span fits. The returned `Span.size` is the aligned
    /// size, which may be larger than `size`.
    pub fn alloc(&mut self, size: u64) -> Result<Span, AllocError> {
        let aligned = align_up(size);
        if aligned == 0 {
            // Zero-size allocations: return a zero-size span at offset 0.
            // The caller probably shouldn't be doing this, but a degenerate
            // result is safer than panicking.
            return Ok(Span { offset: 0, size: 0 });
        }

        // Best-fit: scan free spans, pick the smallest one ≥ aligned.
        // For ~1000-span free-lists this is fast enough; if profiling
        // shows it matters, add a size-keyed secondary index.
        let mut best: Option<(u64, u64)> = None; // (offset, size)
        for (&off, &sz) in self.free.iter() {
            if sz >= aligned {
                match best {
                    None => best = Some((off, sz)),
                    Some((_, best_sz)) if sz < best_sz => best = Some((off, sz)),
                    _ => {}
                }
            }
        }

        let (off, sz) = best.ok_or_else(|| AllocError::Exhausted {
            wanted: aligned,
            free_total: self.free_bytes(),
            free_largest: self.largest_free_span(),
        })?;

        // Pop the free span; replace with the leftover (if any) at the
        // end. Allocations grow from the start of the span; the tail
        // becomes a new free span if non-empty.
        self.free.remove(&off);
        let leftover = sz - aligned;
        if leftover > 0 {
            self.free.insert(off + aligned, leftover);
        }

        self.allocated.insert(off, aligned);
        self.allocated_total += aligned;
        Ok(Span { offset: off, size: aligned })
    }

    /// Free a previously-allocated span by its offset. Coalesces with
    /// neighboring free spans. Returns `InvalidFree` if `offset` doesn't
    /// match any current allocation (e.g. double-free or never-allocated).
    pub fn free(&mut self, offset: u64) -> Result<(), AllocError> {
        let size = self.allocated.remove(&offset)
            .ok_or(AllocError::InvalidFree { offset })?;
        self.allocated_total -= size;

        let mut new_offset = offset;
        let mut new_size = size;

        // Coalesce with previous free span if contiguous.
        if let Some((&prev_off, &prev_size)) =
            self.free.range(..new_offset).next_back()
        {
            if prev_off + prev_size == new_offset {
                self.free.remove(&prev_off);
                new_offset = prev_off;
                new_size += prev_size;
            }
        }

        // Coalesce with next free span if contiguous.
        if let Some((&next_off, &next_size)) = self.free.range(new_offset..).next() {
            if new_offset + new_size == next_off {
                self.free.remove(&next_off);
                new_size += next_size;
            }
        }

        self.free.insert(new_offset, new_size);
        Ok(())
    }

    /// Check that internal invariants hold. Used by tests; cheap to call.
    /// Returns Err with a message if anything is inconsistent.
    #[cfg(test)]
    pub fn check_invariants(&self) -> Result<(), String> {
        // 1. No two free spans overlap or are adjacent (otherwise they'd
        //    have been coalesced).
        let mut prev_end: Option<u64> = None;
        for (&off, &sz) in self.free.iter() {
            if let Some(pe) = prev_end {
                if pe == off {
                    return Err(format!(
                        "uncoalesced adjacent free spans ending/starting at {}", off));
                }
                if pe > off {
                    return Err(format!("overlapping free spans near offset {}", off));
                }
            }
            prev_end = Some(off + sz);
        }

        // 2. Free + allocated must cover exactly capacity.
        let free_total: u64 = self.free.values().sum();
        if free_total + self.allocated_total != self.capacity {
            return Err(format!(
                "totals don't match capacity: free {} + alloc {} != cap {}",
                free_total, self.allocated_total, self.capacity,
            ));
        }

        // 3. allocated_total cache matches.
        let actual_alloc: u64 = self.allocated.values().sum();
        if actual_alloc != self.allocated_total {
            return Err(format!(
                "allocated_total cache wrong: cached {}, actual {}",
                self.allocated_total, actual_alloc,
            ));
        }

        // 4. No allocated span overlaps a free span.
        for (&aoff, &asz) in self.allocated.iter() {
            for (&foff, &fsz) in self.free.iter() {
                if aoff < foff + fsz && foff < aoff + asz {
                    return Err(format!(
                        "alloc [{},{}) overlaps free [{},{})",
                        aoff, aoff + asz, foff, foff + fsz));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_up_works() {
        assert_eq!(align_up(0), 0);
        assert_eq!(align_up(1), 256);
        assert_eq!(align_up(255), 256);
        assert_eq!(align_up(256), 256);
        assert_eq!(align_up(257), 512);
        assert_eq!(align_up(1024), 1024);
    }

    #[test]
    fn fresh_allocator_is_one_big_free_span() {
        let a = SpanAllocator::new(1024 * 1024);
        assert_eq!(a.capacity(), 1024 * 1024);
        assert_eq!(a.free_bytes(), 1024 * 1024);
        assert_eq!(a.allocated_bytes(), 0);
        assert_eq!(a.free_span_count(), 1);
        assert_eq!(a.largest_free_span(), 1024 * 1024);
        a.check_invariants().unwrap();
    }

    #[test]
    fn capacity_rounds_down_to_alignment() {
        let a = SpanAllocator::new(1000); // not aligned
        assert_eq!(a.capacity(), 1000 & !(ALIGNMENT - 1)); // = 768
        a.check_invariants().unwrap();
    }

    #[test]
    fn alloc_and_free_round_trip() {
        let mut a = SpanAllocator::new(8192);
        let s = a.alloc(1000).unwrap();
        assert_eq!(s.offset, 0);
        assert_eq!(s.size, 1024); // rounded up to alignment
        assert_eq!(a.allocated_bytes(), 1024);
        assert_eq!(a.free_bytes(), 8192 - 1024);
        a.check_invariants().unwrap();
        a.free(s.offset).unwrap();
        assert_eq!(a.allocated_bytes(), 0);
        assert_eq!(a.free_bytes(), 8192);
        // Coalesced back to one free span.
        assert_eq!(a.free_span_count(), 1);
        a.check_invariants().unwrap();
    }

    #[test]
    fn many_allocs_then_free_in_reverse_coalesces() {
        let mut a = SpanAllocator::new(1024 * 1024);
        let mut spans = Vec::new();
        for i in 0..16 {
            let s = a.alloc(1000 + i * 100).unwrap();
            spans.push(s);
        }
        a.check_invariants().unwrap();
        // Free in reverse: each free should coalesce with the previous
        // (next-in-offset) free span, ending with a single free span.
        for s in spans.into_iter().rev() {
            a.free(s.offset).unwrap();
            a.check_invariants().unwrap();
        }
        assert_eq!(a.free_span_count(), 1);
        assert_eq!(a.free_bytes(), a.capacity());
    }

    #[test]
    fn many_allocs_then_free_in_order_coalesces() {
        let mut a = SpanAllocator::new(1024 * 1024);
        let mut spans = Vec::new();
        for i in 0..16 {
            spans.push(a.alloc(1000 + i * 100).unwrap());
        }
        // Free in alloc order — each free coalesces with the previous
        // (lower-offset) free span via the "previous neighbor" check.
        for s in spans {
            a.free(s.offset).unwrap();
            a.check_invariants().unwrap();
        }
        assert_eq!(a.free_span_count(), 1);
    }

    #[test]
    fn free_in_middle_does_not_coalesce_when_neighbors_used() {
        let mut a = SpanAllocator::new(8192);
        let s0 = a.alloc(1024).unwrap();
        let s1 = a.alloc(1024).unwrap();
        let s2 = a.alloc(1024).unwrap();
        // Free middle: leaves a hole; neighbors still allocated, no coalesce.
        a.free(s1.offset).unwrap();
        assert_eq!(a.free_span_count(), 2); // hole + tail
        a.check_invariants().unwrap();
        let _ = (s0, s2);
    }

    #[test]
    fn fragmented_then_coalesce() {
        let mut a = SpanAllocator::new(4096);
        let s0 = a.alloc(1024).unwrap();
        let s1 = a.alloc(1024).unwrap();
        let s2 = a.alloc(1024).unwrap();
        // Free outside in: s0, s2, then s1 — that last free should
        // coalesce with both neighbors into one span.
        a.free(s0.offset).unwrap();
        a.free(s2.offset).unwrap();
        a.check_invariants().unwrap();
        assert_eq!(a.free_span_count(), 2); // beginning, end
        a.free(s1.offset).unwrap();
        a.check_invariants().unwrap();
        assert_eq!(a.free_span_count(), 1); // fully coalesced
    }

    #[test]
    fn exhaustion_returns_err() {
        let mut a = SpanAllocator::new(4096);
        let _s = a.alloc(3000).unwrap();
        match a.alloc(2000) {
            Err(AllocError::Exhausted { wanted: 2048, free_total: 1024, free_largest: 1024 }) => {}
            other => panic!("expected exhaustion, got {:?}", other),
        }
    }

    #[test]
    fn exhaustion_fragmented_reports_largest() {
        let mut a = SpanAllocator::new(4096);
        // Alloc 4 × 1024, then free every other → 2 free spans of 1024 each
        // but no contiguous 2048.
        let s0 = a.alloc(1024).unwrap();
        let s1 = a.alloc(1024).unwrap();
        let s2 = a.alloc(1024).unwrap();
        let s3 = a.alloc(1024).unwrap();
        a.free(s0.offset).unwrap();
        a.free(s2.offset).unwrap();
        a.check_invariants().unwrap();
        // 2048 free total, but largest contiguous = 1024.
        match a.alloc(2000) {
            Err(AllocError::Exhausted { free_total: 2048, free_largest: 1024, .. }) => {}
            other => panic!("expected fragmented exhaustion, got {:?}", other),
        }
        let _ = (s1, s3);
    }

    #[test]
    fn invalid_free_rejected() {
        let mut a = SpanAllocator::new(4096);
        match a.free(99) {
            Err(AllocError::InvalidFree { offset: 99 }) => {}
            other => panic!("expected InvalidFree, got {:?}", other),
        }
    }

    #[test]
    fn double_free_rejected() {
        let mut a = SpanAllocator::new(4096);
        let s = a.alloc(1024).unwrap();
        a.free(s.offset).unwrap();
        match a.free(s.offset) {
            Err(AllocError::InvalidFree { .. }) => {}
            other => panic!("expected InvalidFree on double-free, got {:?}", other),
        }
    }

    #[test]
    fn best_fit_picks_smallest_fitting_span() {
        // Build three NON-CONTIGUOUS free spans of distinct sizes:
        // we need allocs separating them so they don't coalesce on free.
        // Layout: [s0:1024 | s1:8192 | s2:1024 | s3:2048 | s4:1024 | s5:4096 | tail]
        //         total used = 17408, tail = ...
        let mut a = SpanAllocator::new(32768);
        let s0 = a.alloc(1024).unwrap();  // separator
        let s1 = a.alloc(8192).unwrap();  // big hole candidate
        let s2 = a.alloc(1024).unwrap();  // separator
        let s3 = a.alloc(2048).unwrap();  // small hole candidate
        let s4 = a.alloc(1024).unwrap();  // separator
        let s5 = a.alloc(4096).unwrap();  // medium hole candidate
        // Free the holes; separators stay allocated to prevent coalescing.
        a.free(s1.offset).unwrap();
        a.free(s3.offset).unwrap();
        a.free(s5.offset).unwrap();
        // Free spans: (1024, 8192), (10240, 2048), (13312, 4096), and tail.
        // Best-fit for 1500 (→ 1536): smallest fitting is the 2048 at
        // offset 10240. NOT the 8192, NOT the 4096, NOT the tail.
        let s = a.alloc(1500).unwrap();
        assert_eq!(s.size, 1536);
        assert_eq!(s.offset, 10240, "best-fit should pick the 2048 hole");
        let _ = (s0, s2, s4);
        a.check_invariants().unwrap();
    }

    #[test]
    fn stress_alloc_free_no_leak() {
        let mut a = SpanAllocator::new(1024 * 1024);
        let mut live = Vec::new();
        // Mix allocs and frees, varied sizes.
        for round in 0..200u64 {
            let size: u64 = 100 + (round * 37) % 5000;
            if let Ok(s) = a.alloc(size) {
                live.push(s);
            }
            // Every few rounds, free a random live span.
            if round % 3 == 0 && !live.is_empty() {
                let i = (round as usize) % live.len();
                let s = live.swap_remove(i);
                a.free(s.offset).unwrap();
            }
            a.check_invariants().unwrap();
        }
        // Drain the rest.
        for s in live {
            a.free(s.offset).unwrap();
        }
        assert_eq!(a.allocated_bytes(), 0);
        assert_eq!(a.free_bytes(), a.capacity());
        assert_eq!(a.free_span_count(), 1);
    }

    #[test]
    fn realistic_workload_no_unbounded_fragmentation() {
        // Simulate a chunk-stream workload: 256 simultaneously-live
        // allocations of varied sizes (matching observed mesh sizes
        // from playtest), with steady churn.
        let mut a = SpanAllocator::new(512 * 1024 * 1024); // 512 MB
        let sizes = [
            21_000, 50_000, 188_000, 500_000, 1_200_000, 2_300_000, 4_000_000, 6_000_000,
        ];
        let mut live: Vec<Span> = Vec::new();
        let mut rng = 0xC0FFEEu32;
        let next = |rng: &mut u32| {
            *rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            *rng
        };

        for _ in 0..2000 {
            // Maybe alloc, maybe free.
            if live.len() < 256 && (next(&mut rng) & 1 == 1 || live.is_empty()) {
                let size = sizes[(next(&mut rng) as usize) % sizes.len()];
                if let Ok(s) = a.alloc(size as u64) {
                    live.push(s);
                }
            } else if !live.is_empty() {
                let i = (next(&mut rng) as usize) % live.len();
                let s = live.swap_remove(i);
                a.free(s.offset).unwrap();
            }
        }
        a.check_invariants().unwrap();
        // Sanity: largest free span should still be a meaningful fraction
        // of free total (at least 1/10), not pathologically fragmented.
        let frag_ratio = a.largest_free_span() as f64 / a.free_bytes().max(1) as f64;
        assert!(frag_ratio > 0.1,
            "fragmentation too high: largest={} free_total={} ratio={:.3}",
            a.largest_free_span(), a.free_bytes(), frag_ratio);
    }
}
