//! `render::octree_pin` — span pin ledger for lock-free GPU dispatch waits.
//!
//! r-t1-pool-pin: the T1 hitch root cause (docs/device_logs/TRIAGE_r462.md)
//! is the octree mesher holding the shared `octree_pool` mutex across its
//! fence wait. The lock exists so main-thread eviction can't reuse a span
//! the GPU is mid-read — but on Asahi a dispatch's submit-to-signal wall
//! time is 47–889 ms, so every render/main-thread pool touch (evict_chunks,
//! world reset, coord snapshots, uploads) can stall for that long. The
//! in-code claim "Lock duration: ~1 ms per batch" predates that measurement.
//!
//! This ledger replaces lock-duration with *span pinning*: the mesher pins
//! the spans its dispatch will read (batch chunks + resident neighbors)
//! while holding the lock, then RELEASES the lock for the fence wait. Frees
//! of pinned spans are deferred and executed at unpin. The GPU-visible
//! guarantee is unchanged — a pinned span's bytes are never freed,
//! reallocated, or overwritten while pinned — but the mutex hold shrinks
//! from "whole GPU round-trip" to "upload + bookkeeping".
//!
//! The ledger is deliberately dependency-free (no ash, no pool types):
//! it keys spans by their buffer byte offset (unique among live spans) and
//! is generic over the deferred payload, so it unit-tests headlessly and
//! the Vulkan shell stays thin.

use std::collections::HashMap;

/// Refcounted pin ledger over spans keyed by byte offset.
///
/// `S` is the caller's slot payload, returned from [`request_free`] /
/// [`unpin`] when a deferred free becomes executable.
#[derive(Debug)]
pub struct PinLedger<S> {
    /// offset → pin refcount. Absent = unpinned.
    pins: HashMap<u64, u32>,
    /// offset → slot whose free was requested while pinned.
    /// At most one deferred free per offset can exist: a deferred span is
    /// out of the allocator's reach, so its offset can't be re-issued (and
    /// therefore can't be re-freed) until the deferred free executes.
    deferred: HashMap<u64, S>,
}

impl<S> Default for PinLedger<S> {
    fn default() -> Self {
        Self { pins: HashMap::new(), deferred: HashMap::new() }
    }
}

impl<S> PinLedger<S> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pin a span. Reentrant: pinning the same offset twice (e.g. a chunk
    /// that is both a batch member and another member's neighbor, or two
    /// concurrent batches sharing a neighbor) increments a refcount and
    /// requires a matching number of unpins.
    pub fn pin(&mut self, offset: u64) {
        *self.pins.entry(offset).or_insert(0) += 1;
    }

    /// True if the span at `offset` currently has any pins.
    pub fn is_pinned(&self, offset: u64) -> bool {
        self.pins.contains_key(&offset)
    }

    /// A free was requested for the span at `offset`.
    ///
    /// Returns `Some(slot)` when the span is unpinned — the caller must
    /// free it now. Returns `None` when the span is pinned — the free is
    /// deferred and will be handed back by a later [`unpin`].
    #[must_use = "Some(slot) means the caller must perform the free now"]
    pub fn request_free(&mut self, offset: u64, slot: S) -> Option<S> {
        if self.is_pinned(offset) {
            let prev = self.deferred.insert(offset, slot);
            debug_assert!(
                prev.is_none(),
                "double deferred-free for offset {offset}: a deferred span can't \
                 be re-allocated (it was never returned to the allocator), so a \
                 second free request for the same offset is a caller bug"
            );
            None
        } else {
            Some(slot)
        }
    }

    /// Drop one pin from the span at `offset`.
    ///
    /// Returns `Some(slot)` when this was the last pin AND a free had been
    /// deferred for the span — the caller must free it now. Unpinning an
    /// offset that was never pinned is a caller bug (debug-asserted,
    /// ignored in release).
    #[must_use = "Some(slot) means the caller must perform the deferred free now"]
    pub fn unpin(&mut self, offset: u64) -> Option<S> {
        match self.pins.get_mut(&offset) {
            Some(rc) if *rc > 1 => {
                *rc -= 1;
                None
            }
            Some(_) => {
                self.pins.remove(&offset);
                self.deferred.remove(&offset)
            }
            None => {
                debug_assert!(false, "unpin of never-pinned offset {offset}");
                None
            }
        }
    }

    /// Unpin a whole batch of offsets, collecting every deferred free that
    /// became executable. Convenience over per-offset [`unpin`] so the
    /// caller re-takes its pool lock once.
    pub fn unpin_many(&mut self, offsets: &[u64]) -> Vec<S> {
        let mut out = Vec::new();
        for &o in offsets {
            if let Some(s) = self.unpin(o) {
                out.push(s);
            }
        }
        out
    }

    /// Number of distinct pinned spans (telemetry).
    pub fn pinned_count(&self) -> usize {
        self.pins.len()
    }

    /// Number of frees currently deferred (telemetry — a persistently
    /// nonzero value between batches means a leaked pin).
    pub fn deferred_count(&self) -> usize {
        self.deferred.len()
    }

    /// Drop all pins and hand back every deferred slot. For pool teardown,
    /// where the caller has externally guaranteed no GPU work is in flight.
    pub fn clear(&mut self) -> Vec<S> {
        self.pins.clear();
        self.deferred.drain().map(|(_, s)| s).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_of_unpinned_is_immediate() {
        let mut l: PinLedger<&'static str> = PinLedger::new();
        assert_eq!(l.request_free(100, "a"), Some("a"));
        assert_eq!(l.deferred_count(), 0);
    }

    #[test]
    fn free_of_pinned_defers_until_unpin() {
        let mut l: PinLedger<&'static str> = PinLedger::new();
        l.pin(100);
        assert_eq!(l.request_free(100, "a"), None);
        assert_eq!(l.deferred_count(), 1);
        assert_eq!(l.unpin(100), Some("a"));
        assert_eq!(l.deferred_count(), 0);
        assert!(!l.is_pinned(100));
    }

    #[test]
    fn refcount_defers_across_multiple_pins() {
        let mut l: PinLedger<u32> = PinLedger::new();
        l.pin(64); // batch member
        l.pin(64); // also someone's neighbor
        assert_eq!(l.request_free(64, 7), None);
        assert_eq!(l.unpin(64), None); // one pin remains
        assert!(l.is_pinned(64));
        assert_eq!(l.unpin(64), Some(7)); // last pin releases the free
    }

    #[test]
    fn unpin_without_deferred_free_returns_none() {
        let mut l: PinLedger<u32> = PinLedger::new();
        l.pin(0);
        assert_eq!(l.unpin(0), None);
        assert!(!l.is_pinned(0));
    }

    #[test]
    fn unpin_many_collects_all_executable_frees() {
        let mut l: PinLedger<u32> = PinLedger::new();
        for o in [10u64, 20, 30] {
            l.pin(o);
        }
        assert_eq!(l.request_free(10, 1), None);
        assert_eq!(l.request_free(30, 3), None);
        // 20 has no deferred free; 10 and 30 do.
        let mut freed = l.unpin_many(&[10, 20, 30]);
        freed.sort_unstable();
        assert_eq!(freed, vec![1, 3]);
        assert_eq!(l.pinned_count(), 0);
        assert_eq!(l.deferred_count(), 0);
    }

    #[test]
    fn distinct_offsets_are_independent() {
        let mut l: PinLedger<u32> = PinLedger::new();
        l.pin(100);
        assert_eq!(l.request_free(200, 2), Some(2)); // 200 unpinned
        assert_eq!(l.request_free(100, 1), None); // 100 pinned
        assert_eq!(l.unpin(100), Some(1));
    }

    #[test]
    fn clear_returns_all_deferred() {
        let mut l: PinLedger<u32> = PinLedger::new();
        l.pin(1);
        l.pin(2);
        assert_eq!(l.request_free(1, 11), None);
        assert_eq!(l.request_free(2, 22), None);
        let mut all = l.clear();
        all.sort_unstable();
        assert_eq!(all, vec![11, 22]);
        assert_eq!(l.pinned_count(), 0);
        assert_eq!(l.deferred_count(), 0);
    }

    #[test]
    fn interleaved_batches_sharing_a_neighbor() {
        // Batch A pins {10, 20}; batch B pins {20, 30} while A is in
        // flight. A's unpin must not release 20's deferred free while B
        // still reads it.
        let mut l: PinLedger<u32> = PinLedger::new();
        l.pin(10);
        l.pin(20); // A
        l.pin(20);
        l.pin(30); // B
        assert_eq!(l.request_free(20, 2), None);
        assert_eq!(l.unpin_many(&[10, 20]), Vec::<u32>::new()); // A done; 20 held by B
        assert!(l.is_pinned(20));
        assert_eq!(l.unpin_many(&[20, 30]), vec![2]); // B done; 20 freed
    }
}
