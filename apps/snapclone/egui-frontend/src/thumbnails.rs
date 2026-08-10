//! thumbnails — `ThumbnailService.cpp`, ported.
//!
//! A bounded cache of decoded thumbnails, keyed by path **and size**, with the
//! decode itself left to the shell.
//!
//! ## The cache is keyed by size, and that is not an optimisation
//!
//! `cacheKey(path, size)` means one photo at 64px and the same photo at 256px
//! are separate entries. Keying by path alone would serve a list row a
//! full-screen bitmap or a detail view a blurry 64px square, and both look like
//! rendering bugs rather than cache bugs.
//!
//! ## FIFO, and the C++ says it means it
//!
//! The comment is unusually direct: *"we don't re-promote on hit, so this is
//! FIFO-with-LRU-flavor not strict LRU"*. That is the right trade for a
//! thumbnail grid — a user scrolls forward, so insertion order approximates
//! recency closely enough, and promoting on every hit means touching the order
//! structure on the hot path for a benefit nobody scrolling a photo list will
//! notice.
//!
//! Porting it as FIFO rather than "improving" it to LRU keeps that decision
//! where it was made. An LRU here would be a different cache with different
//! behaviour under a backwards scroll, and swapping one for the other inside a
//! port is not a port.
//!
//! ## Coalescing is the part that actually matters
//!
//! A grid scrolling past fifty rows asks for the same thumbnail several times
//! before the first decode finishes. Without coalescing that is fifty decodes
//! of one file — which is why a photo list stutters exactly when you scroll it
//! fastest.

use std::collections::{HashMap, VecDeque};

/// Cache capacity, from the original.
pub const MAX_ENTRIES: usize = 256;

/// How many decodes may run at once.
///
/// Decoding is CPU-bound, so more threads than cores adds contention rather
/// than throughput — and on a phone it also adds heat, which throttles the
/// cores you already had.
pub const MAX_ACTIVE: usize = 4;

/// A cache key: one photo at two sizes is two entries.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Key {
    pub path: String,
    pub size: u32,
}

impl Key {
    pub fn new(path: &str, size: u32) -> Key { Key { path: path.into(), size } }
}

/// What a request resolved to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Request {
    /// Already decoded; the caller can draw this frame.
    Hit,
    /// Queued. A decode will run.
    Queued,
    /// Already in flight from an earlier request — nothing new was started.
    Coalesced,
    /// Too many decodes running; the caller should ask again later.
    Deferred,
}

/// Decoded thumbnail bytes, however the shell represents them.
pub type Thumb = Vec<u8>;

#[derive(Default)]
pub struct ThumbnailCache {
    entries: HashMap<Key, Thumb>,
    /// Insertion order for eviction. Not touched on hit — see the module docs.
    order: VecDeque<Key>,
    in_flight: Vec<Key>,
    capacity: usize,
    max_active: usize,
    hits: u64,
    misses: u64,
}

impl ThumbnailCache {
    pub fn new() -> ThumbnailCache {
        ThumbnailCache { capacity: MAX_ENTRIES, max_active: MAX_ACTIVE, ..Default::default() }
    }

    pub fn with_limits(capacity: usize, max_active: usize) -> ThumbnailCache {
        ThumbnailCache { capacity: capacity.max(1), max_active: max_active.max(1),
                         ..Default::default() }
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn active(&self) -> usize { self.in_flight.len() }
    pub fn hits(&self) -> u64 { self.hits }
    pub fn misses(&self) -> u64 { self.misses }

    pub fn get(&mut self, key: &Key) -> Option<&Thumb> {
        match self.entries.get(key) {
            Some(t) => { self.hits += 1; Some(t) }
            None => { self.misses += 1; None }
        }
    }

    /// Ask for a thumbnail.
    ///
    /// The single entry point, so a caller cannot forget the coalescing check —
    /// which is the failure that turns one file into fifty decodes.
    pub fn request(&mut self, key: &Key) -> Request {
        if self.entries.contains_key(key) { self.hits += 1; return Request::Hit; }
        self.misses += 1;
        if self.in_flight.contains(key) { return Request::Coalesced; }
        if self.in_flight.len() >= self.max_active { return Request::Deferred; }
        self.in_flight.push(key.clone());
        Request::Queued
    }

    /// A decode finished.
    ///
    /// `None` means it failed — the in-flight slot is still released, or a
    /// corrupt file would block a decode slot for the life of the process and
    /// the grid would slow down permanently after meeting one bad photo.
    pub fn complete(&mut self, key: &Key, thumb: Option<Thumb>) {
        self.in_flight.retain(|k| k != key);
        let Some(t) = thumb else { return };
        if self.entries.insert(key.clone(), t).is_none() {
            self.order.push_back(key.clone());
        }
        while self.order.len() > self.capacity {
            if let Some(gone) = self.order.pop_front() { self.entries.remove(&gone); }
        }
    }

    /// Drop everything, as `clearCache` does.
    ///
    /// In-flight decodes are NOT cancelled — they cannot be, they are already
    /// running — but their results are no longer wanted. Clearing the in-flight
    /// list lets a later request re-queue rather than coalescing onto a job
    /// whose result will be discarded.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.in_flight.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thumb(n: u8) -> Thumb { vec![n; 4] }

    #[test]
    fn one_photo_at_two_sizes_is_two_entries() {
        // Keying by path alone would serve a list row a full-screen bitmap or a
        // detail view a blurry 64px square — both look like rendering bugs.
        let mut c = ThumbnailCache::new();
        let small = Key::new("/photos/a.jpg", 64);
        let large = Key::new("/photos/a.jpg", 256);
        c.complete(&small, Some(thumb(1)));
        assert_eq!(c.request(&small), Request::Hit);
        assert_eq!(c.request(&large), Request::Queued, "a different size is a miss");
    }

    #[test]
    fn a_repeat_request_while_decoding_is_coalesced() {
        // A grid scrolling past fifty rows asks for the same thumbnail several
        // times before the first decode finishes; without this that is fifty
        // decodes of one file.
        let mut c = ThumbnailCache::new();
        let k = Key::new("/photos/a.jpg", 64);
        assert_eq!(c.request(&k), Request::Queued);
        assert_eq!(c.request(&k), Request::Coalesced);
        assert_eq!(c.request(&k), Request::Coalesced);
        assert_eq!(c.active(), 1, "one decode, not three");
    }

    #[test]
    fn decodes_are_bounded_and_the_caller_is_told_to_retry() {
        // More threads than cores adds contention rather than throughput, and
        // on a phone adds heat, which throttles the cores you already had.
        let mut c = ThumbnailCache::with_limits(64, 2);
        assert_eq!(c.request(&Key::new("a", 64)), Request::Queued);
        assert_eq!(c.request(&Key::new("b", 64)), Request::Queued);
        assert_eq!(c.request(&Key::new("c", 64)), Request::Deferred);
        assert_eq!(c.active(), 2);
    }

    #[test]
    fn finishing_one_decode_frees_a_slot() {
        let mut c = ThumbnailCache::with_limits(64, 1);
        let a = Key::new("a", 64);
        c.request(&a);
        assert_eq!(c.request(&Key::new("b", 64)), Request::Deferred);
        c.complete(&a, Some(thumb(1)));
        assert_eq!(c.request(&Key::new("b", 64)), Request::Queued);
    }

    #[test]
    fn a_failed_decode_still_frees_its_slot() {
        // A corrupt file would otherwise hold a decode slot for the life of the
        // process, and the grid would run permanently slower after meeting one
        // bad photo.
        let mut c = ThumbnailCache::with_limits(64, 1);
        let bad = Key::new("/photos/corrupt.jpg", 64);
        c.request(&bad);
        c.complete(&bad, None);
        assert_eq!(c.active(), 0);
        assert!(c.is_empty(), "and nothing was cached");
        assert_eq!(c.request(&Key::new("good", 64)), Request::Queued);
    }

    #[test]
    fn a_failed_decode_can_be_retried() {
        // Not poisoned: a file may have been mid-copy the first time.
        let mut c = ThumbnailCache::new();
        let k = Key::new("a", 64);
        c.request(&k);
        c.complete(&k, None);
        assert_eq!(c.request(&k), Request::Queued, "retryable");
    }

    #[test]
    fn the_cache_evicts_in_insertion_order() {
        // FIFO, as the C++ says it means: "we don't re-promote on hit, so this
        // is FIFO-with-LRU-flavor not strict LRU". Ported as-is rather than
        // "improved" to LRU, which would be a different cache.
        let mut c = ThumbnailCache::with_limits(3, 4);
        for i in 0..3u8 {
            let k = Key::new(&format!("{i}"), 64);
            c.complete(&k, Some(thumb(i)));
        }
        // Hit the oldest — under strict LRU this would promote it.
        assert_eq!(c.request(&Key::new("0", 64)), Request::Hit);
        c.complete(&Key::new("3", 64), Some(thumb(3)));
        assert_eq!(c.request(&Key::new("0", 64)), Request::Queued,
                   "the oldest went despite the hit — FIFO, not LRU");
        assert_eq!(c.request(&Key::new("1", 64)), Request::Hit);
    }

    #[test]
    fn the_cache_never_exceeds_its_capacity() {
        let mut c = ThumbnailCache::with_limits(10, 4);
        for i in 0..50u32 {
            c.complete(&Key::new(&format!("{i}"), 64), Some(thumb(0)));
        }
        assert_eq!(c.len(), 10);
    }

    #[test]
    fn re_completing_a_key_does_not_grow_the_order_twice() {
        // A second decode of the same key — from a retry after a clear — would
        // otherwise leave a stale order entry that evicts a live one later.
        let mut c = ThumbnailCache::with_limits(2, 4);
        let a = Key::new("a", 64);
        c.complete(&a, Some(thumb(1)));
        c.complete(&a, Some(thumb(2)));
        c.complete(&Key::new("b", 64), Some(thumb(3)));
        assert_eq!(c.request(&a), Request::Hit, "a survived: it is only one entry");
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn clearing_lets_a_request_requeue_rather_than_coalescing_onto_a_dead_job() {
        // In-flight decodes cannot be cancelled, but their results are no
        // longer wanted; coalescing onto one would wait for a result that gets
        // discarded.
        let mut c = ThumbnailCache::new();
        let k = Key::new("a", 64);
        c.request(&k);
        c.clear();
        assert_eq!(c.request(&k), Request::Queued, "not Coalesced");
    }

    #[test]
    fn a_late_completion_after_a_clear_is_harmless() {
        // The decode was already running when the cache was cleared.
        let mut c = ThumbnailCache::new();
        let k = Key::new("a", 64);
        c.request(&k);
        c.clear();
        c.complete(&k, Some(thumb(1)));
        assert_eq!(c.len(), 1, "it lands rather than corrupting anything");
    }

    #[test]
    fn hit_and_miss_counts_track_what_a_diagnostics_panel_would_show() {
        let mut c = ThumbnailCache::new();
        let k = Key::new("a", 64);
        c.request(&k);
        c.complete(&k, Some(thumb(1)));
        c.request(&k);
        assert_eq!(c.hits(), 1);
        assert_eq!(c.misses(), 1);
    }

    #[test]
    fn zero_limits_are_clamped_rather_than_deadlocking() {
        // A capacity of zero evicts everything instantly and a max_active of
        // zero defers forever — a grid that never loads with no error.
        let mut c = ThumbnailCache::with_limits(0, 0);
        assert_eq!(c.request(&Key::new("a", 64)), Request::Queued);
        c.complete(&Key::new("a", 64), Some(thumb(1)));
        assert_eq!(c.len(), 1);
    }
}
