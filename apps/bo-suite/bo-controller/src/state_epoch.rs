#![allow(dead_code)]
// src/state_epoch.rs
//
// A tiny synchronization primitive: each logical "piece of state" gets
// an atomic counter that ratchets up every time the state changes.
// Managers that compute things based on that state can tag their
// computation with the epoch they observed. When they go to commit,
// they check whether the epoch has moved on — if it has, their work is
// based on stale assumptions and they can decide to bail.
//
// This is NOT full optimistic concurrency control. It's a "is my view
// stale?" check, useful for catching the kinds of races our system
// actually produces:
//
//   * Shadow stream saved vol=30, then the user manually set vol=50.
//     When the shadow times out we shouldn't restore vol=30 — that
//     would undo the user's deliberate adjustment.
//
//   * Anticipator computed predictions at epoch 100. By epoch 105 the
//     user switched devices. Pre-staging based on the old prediction
//     is wasted bandwidth and could even mislead the user.
//
//   * Device probe started at epoch 50. By the time the probe replied
//     we're at epoch 70 because the device list mutated. The probe
//     result describes a world that no longer exists.
//
// Cheap: a single AtomicU64 per state with Acquire/Release semantics.
// Tens of nanoseconds per read on x86. Shared across threads via Arc.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Atomic ratcheting counter. Starts at 0; every `bump()` increments by
/// one. Readers call `current()` to capture, then later `is_stale(captured)`
/// to check whether the state has moved on.
#[derive(Debug, Default)]
pub struct StateEpoch {
    counter: AtomicU64,
}

impl StateEpoch {
    pub fn new() -> Self { Self::default() }

    /// Read the current epoch. Use this when starting a piece of work
    /// that depends on the state this epoch tracks; pass the result
    /// alongside the work, then call `is_stale` before committing.
    pub fn current(&self) -> u64 {
        self.counter.load(Ordering::Acquire)
    }

    /// Advance the epoch. Returns the new value. Called by whatever
    /// manager owns the underlying state right after committing a
    /// change.
    pub fn bump(&self) -> u64 {
        // Add 1 with AcqRel: any writes that preceded this bump are
        // visible to threads that subsequently read this epoch with
        // Acquire and see the new value.
        self.counter.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Has the epoch advanced since `observed` was captured? True
    /// means "your view is stale, reconsider before acting".
    pub fn is_stale(&self, observed: u64) -> bool {
        self.current() != observed
    }
}

/// Convenience alias for the cross-manager shared form. The Arc lets
/// multiple managers hold references; bumps from any of them are
/// visible to all readers.
pub type SharedEpoch = Arc<StateEpoch>;

/// Convenience: create a fresh shared epoch.
pub fn shared() -> SharedEpoch { Arc::new(StateEpoch::new()) }

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn fresh_epoch_starts_at_zero() {
        let e = StateEpoch::new();
        assert_eq!(e.current(), 0);
    }

    #[test]
    fn bump_advances_monotonically() {
        let e = StateEpoch::new();
        assert_eq!(e.bump(), 1);
        assert_eq!(e.bump(), 2);
        assert_eq!(e.bump(), 3);
        assert_eq!(e.current(), 3);
    }

    #[test]
    fn captured_epoch_is_not_stale_immediately() {
        let e = StateEpoch::new();
        let captured = e.current();
        assert!(!e.is_stale(captured));
    }

    #[test]
    fn captured_epoch_becomes_stale_after_bump() {
        let e = StateEpoch::new();
        let captured = e.current();
        e.bump();
        assert!(e.is_stale(captured));
    }

    #[test]
    fn shared_epoch_visible_across_threads() {
        let e = shared();
        let e_writer = Arc::clone(&e);
        let writer = thread::spawn(move || {
            for _ in 0..100 { e_writer.bump(); }
        });
        writer.join().unwrap();
        // From this thread, the writer's 100 bumps are visible.
        assert_eq!(e.current(), 100);
    }

    #[test]
    fn cross_thread_staleness_detection() {
        // Reader captures epoch, writer bumps, reader checks → stale.
        let e = shared();
        let captured = e.current();
        let e_writer = Arc::clone(&e);
        thread::spawn(move || { e_writer.bump(); }).join().unwrap();
        assert!(e.is_stale(captured));
    }

    #[test]
    fn no_false_staleness_on_unchanged_state() {
        // Across 1000 reads with no bumps, all are fresh.
        let e = shared();
        let captured = e.current();
        for _ in 0..1000 {
            assert!(!e.is_stale(captured));
        }
    }

    #[test]
    fn bump_count_matches_writer_total() {
        // Several writer threads each bump 100 times; total should match.
        let e = shared();
        let mut handles = Vec::new();
        for _ in 0..4 {
            let e2 = Arc::clone(&e);
            handles.push(thread::spawn(move || {
                for _ in 0..100 { e2.bump(); }
            }));
        }
        for h in handles { h.join().unwrap(); }
        assert_eq!(e.current(), 400);
    }
}
