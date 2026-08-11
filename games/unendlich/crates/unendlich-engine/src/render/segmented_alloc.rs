//! Segmented span allocator — the growable half of the r-vulkanmod-cf
//! finding, isolated and pure so it's fully testable without Vulkan.
//!
//! `mesh_buffer_alloc::SpanAllocator`'s own doc says it plainly: *"the
//! underlying GPU buffer is fixed-size and we can't grow it mid-frame."*
//! True for a single `VkBuffer` — you can't resize one. But you CAN add
//! a second `VkBuffer` alongside it. That's what VulkanMod's `AreaBuffer`
//! does: a segmented allocator (their doc: "doubly-linked list of
//! segments, tracking both used and free segments") that grows by
//! adding segments, never by resizing one.
//!
//! This module is that pattern, built on top of the EXISTING
//! `SpanAllocator` (unmodified — this composes it, doesn't touch it, so
//! `octree_buffer.rs`'s use of the same type is untouched) rather than
//! replacing it. Each segment IS a `SpanAllocator`; this just manages
//! the list of them and decides when to grow.
//!
//! What this does NOT do: bind the resulting multi-buffer layout into
//! actual Vulkan draw calls. That's real GPU-binding surgery
//! (`chunk_indirect.rs`/`chunk_pipeline.rs` currently assume ONE vertex
//! buffer + ONE index buffer for the whole draw) that needs hardware to
//! verify — see the integration doc alongside this file for the plan
//! and why it's staged separately, same discipline as the
//! `mesh_dirty_integration_sketch.md` precedent.

use super::mesh_buffer_alloc::{align_up, AllocError, SpanAllocator};

/// A span within a specific segment. Unlike `mesh_buffer_alloc::Span`,
/// this carries WHICH segment (buffer) the span lives in — the caller
/// needs this to know which `VkBuffer` to bind/copy into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegSpan {
    pub segment: usize,
    pub offset: u64,
    pub size: u64,
}

/// Errors from the segmented allocator. Distinct from `AllocError` (the
/// per-segment error) because "every segment is full AND we're at the
/// segment cap" is a different condition from "this one segment is full"
/// — the latter just means try the next segment / grow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegAllocError {
    /// Every existing segment lacks room AND `max_segments` is already
    /// reached — the only real "no more memory" case. Mirrors
    /// `AllocError::Exhausted`'s fields for the same diagnostics.
    Exhausted { wanted: u64, segment_count: usize, max_segments: usize, total_capacity: u64 },
    /// `segment` in a `SegSpan` doesn't exist (stale handle from before
    /// a... there's no shrink path today, so this really only means a
    /// corrupted/foreign handle, but kept for the same reason
    /// `mesh_buffer_pool::MeshSlot` checks versions: defense against
    /// misuse, not an expected runtime path).
    InvalidSegment { segment: usize },
    /// Forwarded from the segment's own `SpanAllocator::free`.
    InvalidFree { segment: usize, offset: u64 },
}

impl std::fmt::Display for SegAllocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SegAllocError::Exhausted { wanted, segment_count, max_segments, total_capacity } => write!(
                f,
                "Segmented allocator exhausted: wanted {} bytes, {} segment(s) \
                 (cap {}), {} total bytes capacity",
                wanted, segment_count, max_segments, total_capacity,
            ),
            SegAllocError::InvalidSegment { segment } => {
                write!(f, "Invalid segment index {}", segment)
            }
            SegAllocError::InvalidFree { segment, offset } => write!(
                f, "Invalid free at segment {} offset {} (not allocated, or double-free)",
                segment, offset,
            ),
        }
    }
}
impl std::error::Error for SegAllocError {}

/// A segmented, growable span allocator: a `Vec<SpanAllocator>` that
/// adds a new segment when every existing one is full, instead of
/// failing outright — up to `max_segments`, which is the real ceiling
/// (prevents a pathological chunk stream from growing memory
/// unboundedly; the renderer's `mesh_pool_env.rs` env-var default
/// should size this generously but finitely, same spirit as
/// `UNENDLICH_OCTREE_POOL_MB`'s clamp).
#[derive(Debug)]
pub struct SegmentedSpanAllocator {
    segments: Vec<SpanAllocator>,
    /// New segments are at least this big. A single allocation larger
    /// than this still gets a segment sized to fit it (see `alloc`) —
    /// growth never fails to fit an allocation just because it exceeds
    /// the "typical" growth increment.
    segment_capacity: u64,
    max_segments: usize,
}

impl SegmentedSpanAllocator {
    /// `initial_capacity`: size of the first segment (created eagerly,
    /// same as today's single-buffer pool — no behavior change for the
    /// common case where one segment is enough).
    /// `segment_capacity`: size of each segment added on growth.
    /// `max_segments`: hard cap on segment count (the real exhaustion
    /// bound — total capacity is at most `max_segments * segment_capacity`
    /// plus slack for oversized single allocations, see `alloc`).
    pub fn new(initial_capacity: u64, segment_capacity: u64, max_segments: usize) -> Self {
        let max_segments = max_segments.max(1);
        Self {
            segments: vec![SpanAllocator::new(initial_capacity)],
            segment_capacity,
            max_segments,
        }
    }

    pub fn segment_count(&self) -> usize { self.segments.len() }
    pub fn max_segments(&self) -> usize { self.max_segments }

    /// Per-segment capacities, in creation order. The renderer needs
    /// this to know how big a `VkBuffer` to create for each segment
    /// index (segment 0 already exists at construction; later indices
    /// are created lazily as `alloc` grows).
    pub fn segment_capacities(&self) -> Vec<u64> {
        self.segments.iter().map(|s| s.capacity()).collect()
    }

    pub fn total_capacity(&self) -> u64 {
        self.segments.iter().map(|s| s.capacity()).sum()
    }

    pub fn total_allocated(&self) -> u64 {
        self.segments.iter().map(|s| s.allocated_bytes()).sum()
    }

    pub fn total_free(&self) -> u64 {
        self.total_capacity() - self.total_allocated()
    }

    /// Allocate `size` bytes. Tries existing segments in order
    /// (first-fit across segments; best-fit within a segment, since
    /// that's `SpanAllocator`'s own strategy — unchanged). If none fit
    /// and `max_segments` isn't reached, adds a new segment sized to
    /// `max(segment_capacity, size)` — so one oversized mesh (e.g. the
    /// documented 7MB outlier) always gets a segment that fits it in
    /// one shot, never a partial/retry loop.
    pub fn alloc(&mut self, size: u64) -> Result<SegSpan, SegAllocError> {
        for (i, seg) in self.segments.iter_mut().enumerate() {
            if let Ok(span) = seg.alloc(size) {
                return Ok(SegSpan { segment: i, offset: span.offset, size: span.size });
            }
        }
        if self.segments.len() >= self.max_segments {
            return Err(SegAllocError::Exhausted {
                wanted: size,
                segment_count: self.segments.len(),
                max_segments: self.max_segments,
                total_capacity: self.total_capacity(),
            });
        }
        let new_cap = self.segment_capacity.max(align_up(size));
        let mut new_seg = SpanAllocator::new(new_cap);
        // Guaranteed to succeed: the segment was sized to fit `size`
        // and starts empty. An Err here would mean align_up/capacity
        // rounding disagree with SpanAllocator's own alignment, which
        // would be a bug in this function, not a real exhaustion case —
        // propagate rather than panic so it surfaces as a normal error
        // instead of crashing the renderer.
        let span = new_seg.alloc(size).map_err(|_| SegAllocError::Exhausted {
            wanted: size,
            segment_count: self.segments.len(),
            max_segments: self.max_segments,
            total_capacity: self.total_capacity(),
        })?;
        let idx = self.segments.len();
        self.segments.push(new_seg);
        Ok(SegSpan { segment: idx, offset: span.offset, size: span.size })
    }

    pub fn free(&mut self, span: SegSpan) -> Result<(), SegAllocError> {
        let seg = self.segments.get_mut(span.segment)
            .ok_or(SegAllocError::InvalidSegment { segment: span.segment })?;
        seg.free(span.offset).map_err(|_| SegAllocError::InvalidFree {
            segment: span.segment,
            offset: span.offset,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_segment_common_case_unchanged() {
        // Small pool, small allocations: never grows, matches today's
        // single-buffer behavior exactly.
        let mut a = SegmentedSpanAllocator::new(1024 * 1024, 1024 * 1024, 4);
        let s1 = a.alloc(1000).unwrap();
        let s2 = a.alloc(2000).unwrap();
        assert_eq!(s1.segment, 0);
        assert_eq!(s2.segment, 0);
        assert_eq!(a.segment_count(), 1);
    }

    #[test]
    fn grows_to_a_second_segment_when_first_is_full() {
        let cap = 4096u64;
        let mut a = SegmentedSpanAllocator::new(cap, cap, 4);
        // Fill segment 0 exactly (align_up(4096) with ALIGNMENT=256, one
        // alloc of the whole capacity).
        let first = a.alloc(cap).unwrap();
        assert_eq!(first.segment, 0);
        assert_eq!(a.segment_count(), 1);

        // Segment 0 has nothing left; this must land in a NEW segment,
        // not fail.
        let second = a.alloc(100).unwrap();
        assert_eq!(second.segment, 1, "should have grown to a second segment");
        assert_eq!(a.segment_count(), 2);
    }

    #[test]
    fn oversized_single_allocation_gets_a_right_sized_segment() {
        // Growth increment is small; one allocation far exceeds it — the
        // new segment must still fit it in one shot (documented 7MB
        // outlier vs a smaller default growth chunk).
        let mut a = SegmentedSpanAllocator::new(1024, 1024, 4);
        let huge = 10 * 1024 * 1024u64; // 10 MiB, way bigger than the 1024 growth size
        let s = a.alloc(huge).unwrap();
        assert_eq!(s.segment, 1);
        assert!(a.segment_capacities()[1] >= huge, "new segment must be sized to fit the oversized alloc");
    }

    #[test]
    fn max_segments_is_a_real_ceiling() {
        let cap = 1024u64;
        let mut a = SegmentedSpanAllocator::new(cap, cap, 2); // only 2 segments ever allowed
        a.alloc(cap).unwrap(); // fills segment 0
        a.alloc(cap).unwrap(); // fills segment 1 (growth #1, at the cap)
        let err = a.alloc(100);
        assert!(matches!(err, Err(SegAllocError::Exhausted { segment_count: 2, max_segments: 2, .. })));
    }

    #[test]
    fn freeing_reclaims_space_without_growing_again() {
        let cap = 4096u64;
        let mut a = SegmentedSpanAllocator::new(cap, cap, 4);
        let s1 = a.alloc(cap).unwrap(); // fills segment 0
        assert_eq!(a.segment_count(), 1);
        a.free(s1).unwrap();
        // Same-size alloc should reuse segment 0's now-free space, not grow.
        let s2 = a.alloc(cap).unwrap();
        assert_eq!(s2.segment, 0);
        assert_eq!(a.segment_count(), 1, "freeing should let the same segment absorb a re-alloc, no growth needed");
    }

    #[test]
    fn free_wrong_segment_index_errors_cleanly() {
        let mut a = SegmentedSpanAllocator::new(1024, 1024, 2);
        let err = a.free(SegSpan { segment: 5, offset: 0, size: 256 });
        assert!(matches!(err, Err(SegAllocError::InvalidSegment { segment: 5 })));
    }

    #[test]
    fn double_free_in_a_valid_segment_errors_cleanly() {
        let mut a = SegmentedSpanAllocator::new(4096, 4096, 2);
        let s = a.alloc(1000).unwrap();
        a.free(s).unwrap();
        let err = a.free(s);
        assert!(matches!(err, Err(SegAllocError::InvalidFree { .. })));
    }

    #[test]
    fn totals_roll_up_across_segments() {
        let cap = 4096u64;
        let mut a = SegmentedSpanAllocator::new(cap, cap, 4);
        a.alloc(cap).unwrap();      // segment 0 full
        a.alloc(1000).unwrap();     // grows to segment 1
        assert_eq!(a.segment_count(), 2);
        assert_eq!(a.total_capacity(), cap + cap); // both segments same size here
        // total_allocated should be cap (seg0, aligned) + align_up(1000) (seg1)
        assert_eq!(a.total_allocated(), cap + align_up(1000));
        assert_eq!(a.total_free(), a.total_capacity() - a.total_allocated());
    }

    #[test]
    fn realistic_streaming_scenario_matches_documented_working_set() {
        // Models the actual numbers from mesh_buffer_pool.rs's own docs:
        // ~925 chunks at RD=6, sizes 21KB..7MB. A higher render distance
        // (this pool's whole reason for existing) pushes past one
        // segment's capacity — confirm it grows smoothly rather than
        // dropping meshes.
        let segment_cap = 512 * 1024 * 1024u64; // 512 MiB per segment
        let mut a = SegmentedSpanAllocator::new(segment_cap, segment_cap, 16);
        let mut spans = Vec::new();
        let mut total_requested = 0u64;
        // Simulate ~1500 chunks (a higher-RD working set) with sizes
        // spanning the documented 21KB..7MB range, deterministic pseudo-
        // random sizing (no external RNG dependency needed for this).
        for i in 0..1500u64 {
            let size = 21_000 + (i.wrapping_mul(48271) % (7 * 1024 * 1024 - 21_000));
            total_requested += align_up(size);
            spans.push(a.alloc(size).expect("streaming alloc must not fail within max_segments"));
        }
        assert!(a.segment_count() > 1, "a realistic higher-RD working set should need more than one segment");
        assert!(a.segment_count() <= 16, "must respect max_segments");
        assert_eq!(a.total_allocated(), total_requested);
    }
}
