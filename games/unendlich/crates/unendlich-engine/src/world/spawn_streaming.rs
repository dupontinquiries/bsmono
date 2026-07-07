//! Streaming spawn window — the bookkeeping for populating creatures as the
//! player moves through the world and releasing them once they fall behind.
//!
//! It tracks which chunk columns are currently *populated* and, each update,
//! reports a [`StreamDelta`]: chunks that just left the radius (despawn the
//! creatures tagged to them) and chunks to populate now. New chunks are queued
//! and released a `budget` at a time so entering a fresh area doesn't spawn a
//! few hundred creatures in one frame — the spike that the per-frame entity bake
//! can't absorb. The streamer holds no creatures itself; it's pure chunk
//! bookkeeping, so the engine wiring (run [`super::ecosystem::populate_region`]
//! per populated chunk through the [`super::mob_persistence::SpawnLedger`],
//! register the results tagged by chunk, drop them on despawn) stays thin.
//!
//! Square (Chebyshev) range to match how chunks load on a grid. Deterministic
//! and fully unit-tested.

use std::collections::{HashSet, VecDeque};

/// A chunk column identity `(chunk_x, chunk_z)`.
pub type ChunkXZ = (i32, i32);

/// What changed this update.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct StreamDelta {
    /// Chunks to populate now (released from the pending queue, up to budget).
    pub to_populate: Vec<ChunkXZ>,
    /// Chunks that left the radius — despawn the creatures tagged to them.
    pub to_despawn: Vec<ChunkXZ>,
}

impl StreamDelta {
    pub fn is_empty(&self) -> bool {
        self.to_populate.is_empty() && self.to_despawn.is_empty()
    }
}

/// Tracks the populated window around the player.
#[derive(Debug, Clone)]
pub struct SpawnStreamer {
    radius: i32,
    active: HashSet<ChunkXZ>,
    pending: VecDeque<ChunkXZ>,
    queued: HashSet<ChunkXZ>, // mirror of `pending` for O(1) membership
}

impl SpawnStreamer {
    /// `radius` is in chunks (Chebyshev): a `radius`-of-2 window is 5×5 = 25
    /// chunks.
    pub fn new(radius: i32) -> Self {
        SpawnStreamer {
            radius: radius.max(0),
            active: HashSet::new(),
            pending: VecDeque::new(),
            queued: HashSet::new(),
        }
    }

    fn in_range(&self, center: ChunkXZ) -> HashSet<ChunkXZ> {
        let mut s = HashSet::new();
        for dz in -self.radius..=self.radius {
            for dx in -self.radius..=self.radius {
                s.insert((center.0 + dx, center.1 + dz));
            }
        }
        s
    }

    /// Recompute the window around `center` and release up to `budget` pending
    /// chunks for population. Despawns (chunks that left the window) are reported
    /// immediately and in full; populations are throttled by `budget`.
    ///
    /// Idempotent once settled: when every in-range chunk is active and nothing
    /// is pending, repeated calls with the same `center` return an empty delta.
    pub fn update(&mut self, center: ChunkXZ, budget: usize) -> StreamDelta {
        let range = self.in_range(center);

        // Chunks that left the window: despawn and forget them (active or queued).
        let mut to_despawn = Vec::new();
        for &c in self.active.iter() {
            if !range.contains(&c) {
                to_despawn.push(c);
            }
        }
        for c in &to_despawn {
            self.active.remove(c);
        }
        // Drop any queued chunk no longer in range (don't bother populating it).
        if !self.queued.is_empty() {
            self.pending.retain(|c| range.contains(c));
            self.queued.retain(|c| range.contains(c));
        }

        // Newly in range (not active, not already queued) -> enqueue.
        for &c in range.iter() {
            if !self.active.contains(&c) && !self.queued.contains(&c) {
                self.pending.push_back(c);
                self.queued.insert(c);
            }
        }

        // Release up to `budget` from the front of the queue into active.
        let mut to_populate = Vec::new();
        while to_populate.len() < budget {
            match self.pending.pop_front() {
                Some(c) => {
                    self.queued.remove(&c);
                    // Still in range (it always is here, but stay defensive).
                    if range.contains(&c) {
                        self.active.insert(c);
                        to_populate.push(c);
                    }
                }
                None => break,
            }
        }

        StreamDelta { to_populate, to_despawn }
    }

    /// Chunks awaiting population (not yet released by the budget).
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Currently populated chunks.
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    pub fn is_active(&self, c: ChunkXZ) -> bool {
        self.active.contains(&c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_update_budgets_the_initial_fill() {
        let mut s = SpawnStreamer::new(1); // 3x3 = 9 chunks
        let d = s.update((0, 0), 3);
        assert_eq!(d.to_populate.len(), 3, "only the budget is released");
        assert!(d.to_despawn.is_empty());
        assert_eq!(s.active_count(), 3);
        assert_eq!(s.pending_count(), 6, "the rest wait");
    }

    #[test]
    fn the_queue_drains_over_subsequent_updates() {
        let mut s = SpawnStreamer::new(1);
        s.update((0, 0), 3);
        let d = s.update((0, 0), 100); // generous budget finishes the fill
        assert_eq!(d.to_populate.len(), 6);
        assert_eq!(s.active_count(), 9);
        assert_eq!(s.pending_count(), 0);
        // Settled: another update with the same center does nothing.
        assert!(s.update((0, 0), 100).is_empty());
    }

    #[test]
    fn moving_despawns_the_trailing_edge_and_queues_the_leading_one() {
        let mut s = SpawnStreamer::new(1);
        s.update((0, 0), 100); // fully populate the 3x3 around origin
        assert_eq!(s.active_count(), 9);
        // Step east by one chunk: the x=-1 column (3 chunks) leaves, the x=+2
        // column (3 chunks) enters.
        let d = s.update((1, 0), 100);
        assert_eq!(d.to_despawn.len(), 3, "trailing column despawns");
        assert!(d.to_despawn.iter().all(|c| c.0 == -1));
        assert_eq!(d.to_populate.len(), 3, "leading column populates");
        assert!(d.to_populate.iter().all(|c| c.0 == 2));
        assert_eq!(s.active_count(), 9, "window size is constant");
    }

    #[test]
    fn a_chunk_that_leaves_before_it_is_populated_is_dropped() {
        let mut s = SpawnStreamer::new(1);
        let first = s.update((0, 0), 1); // populate 1 near origin, queue 8
        assert_eq!(first.to_populate.len(), 1);
        assert_eq!(s.pending_count(), 8);
        // Teleport far away before the queue drains.
        let d = s.update((100, 100), 100);
        // Everything populated now is around the new center...
        assert!(d.to_populate.iter().all(|c| (c.0 - 100).abs() <= 1 && (c.1 - 100).abs() <= 1));
        // ...the one previously-active (near-origin) chunk despawns...
        assert_eq!(d.to_despawn.len(), 1);
        let dc = d.to_despawn[0];
        assert!(dc.0.abs() <= 1 && dc.1.abs() <= 1, "despawned the old near-origin chunk");
        // ...and the 8 stale pending chunks were discarded, not populated.
        assert_eq!(s.active_count(), 9);
        assert_eq!(s.pending_count(), 0);
        assert!(s.is_active((100, 100)) && !s.is_active((0, 0)) && !s.is_active((-1, -1)));
    }

    #[test]
    fn radius_zero_is_just_the_player_chunk() {
        let mut s = SpawnStreamer::new(0);
        let d = s.update((5, -3), 10);
        assert_eq!(d.to_populate, vec![(5, -3)]);
        assert_eq!(s.active_count(), 1);
    }
}
