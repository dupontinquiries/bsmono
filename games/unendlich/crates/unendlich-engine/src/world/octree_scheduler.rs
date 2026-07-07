//! Cache-defer-aware work queue.
//!
//! v0.24.23. CPU implementation of the scheduling concept documented
//! in `architecture-v1-addendum.md §A4`. The queue holds pending
//! work items each carrying a dependency region + the tick at which
//! it was last computed; consumers process items in deadline order,
//! skipping items whose dependency region is stable since their
//! computed tick.
//!
//! ## The shape of an item
//!
//! Each [`WorkItem`] carries:
//!
//! - **What it does.** A `kind` tag plus an opaque `payload` u64 the
//!   item's executor unpacks. The queue itself doesn't dispatch;
//!   callers pop items and dispatch based on the kind.
//! - **What it depends on.** An AABB in chunk-local coords. The
//!   queue checks this region against the octree's
//!   `last_modified_tick` field via `region_stable_since`.
//! - **When it was last computed.** A tick value the consumer set
//!   when it last produced output for this item. The defer check
//!   asks "anything in my region changed since then?"
//! - **A deadline.** Tick by which the item must be processed (or
//!   its output is stale). The scheduler orders by deadline.
//! - **A defer cost.** How much it "hurts" to defer this item one
//!   tick. Higher cost → prefer to do now. Used as a tie-breaker
//!   among items with similar deadlines.
//!
//! ## What the queue does each cycle
//!
//! Caller invokes [`DeferQueue::drain_ready`] which:
//!
//! 1. Sorts items by deadline ascending (closest-deadline first).
//! 2. For each item: ask the octree
//!    `region_stable_since(item.region, item.last_computed_tick + 1)`.
//! 3. **Stable** → mark item as skipped (caller's output is still
//!    valid; nothing to do). Item stays in queue with its deadline
//!    pushed forward by the configured "skip lifetime" so it doesn't
//!    re-evaluate every tick.
//! 4. **Dirty** → emit the item for processing. Caller dispatches,
//!    produces fresh output, then calls
//!    [`DeferQueue::mark_processed`] which updates the item's
//!    `last_computed_tick` to the current tick.
//!
//! The "skip lifetime" is the key tunable: if dependencies usually
//! stay stable for many ticks, set it high (e.g., 8) so the queue
//! doesn't waste cycles re-checking. If dependencies churn quickly,
//! set it low.
//!
//! ## What this is NOT
//!
//! This is the primitive. The engine wires the queue into specific
//! pipelines (mesher refresh, raycast cache, item region read pool)
//! in subsequent ships. v0.24.23 lands the type + the defer logic +
//! tests. Real integration is filed for v0.24.25.

use crate::world::octree::OctreeArray;

/// Identifies what kind of work this item represents. Specific kinds
/// added as integrations land — for v0.24.23 we have a placeholder
/// catalog covering the obvious cases. Kinds are dense u32 so the
/// queue can be sharded by kind cheaply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum WorkKind {
    /// Mesh extraction for a chunk. Payload = chunk index.
    MeshChunk = 0,
    /// Cached raycast result. Payload = caller-supplied raycast id.
    Raycast = 1,
    /// Item region-read snapshot. Payload = caller-supplied read id.
    RegionRead = 2,
    /// Custom / future. Payload semantics defined by caller.
    Custom = 0xFFFF,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkItem {
    pub kind: WorkKind,
    pub payload: u64,
    /// Region in chunk-local coords the work depends on. The defer
    /// check asks the octree "anything in this region changed
    /// since `last_computed_tick`?"
    pub region_min: (i32, i32, i32),
    pub region_max: (i32, i32, i32),
    pub last_computed_tick: u32,
    pub deadline_tick: u32,
    /// Higher = more painful to defer. Used as a tie-break among
    /// items with the same deadline.
    pub defer_cost: u32,
}

/// Outcome of a queue cycle for one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyKind {
    /// Item is dirty — caller must run it.
    Dirty,
    /// Item is stable — caller skipped it. Output still valid.
    Skipped,
}

#[derive(Debug, Clone, Copy)]
pub struct Ready {
    pub item: WorkItem,
    pub kind: ReadyKind,
}

/// Defer-aware queue. Holds work items + drains them each cycle in
/// deadline order, applying the cache-defer skip.
#[derive(Debug, Default)]
pub struct DeferQueue {
    items: Vec<WorkItem>,
    /// Number of ticks to push a skipped item's deadline forward
    /// after the queue confirms it's stable. Bigger = fewer
    /// re-checks of stable items, smaller = faster reaction when
    /// stability flips.
    skip_lifetime: u32,
}

impl DeferQueue {
    pub fn new(skip_lifetime: u32) -> Self {
        Self { items: Vec::new(), skip_lifetime }
    }

    pub fn push(&mut self, item: WorkItem) {
        self.items.push(item);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Drain the queue against the octree's current state. Returns
    /// every item that's either Dirty (needs processing) or Skipped
    /// (still valid; caller should know nothing changed).
    ///
    /// `current_tick` is the world tick the caller is processing.
    /// Items with `deadline_tick < current_tick` are emitted as
    /// Dirty unconditionally (their deadline already passed; if the
    /// caller is still processing them, they need fresh output).
    ///
    /// Items that stay stable have their deadline pushed forward by
    /// `skip_lifetime` to avoid re-checking the same stable region
    /// every tick.
    pub fn drain_ready(
        &mut self,
        octree: &OctreeArray,
        current_tick: u32,
    ) -> Vec<Ready> {
        // Sort by (deadline, -defer_cost) so closest-deadline +
        // highest-cost-to-defer items come first.
        self.items.sort_by(|a, b| {
            a.deadline_tick
                .cmp(&b.deadline_tick)
                .then(b.defer_cost.cmp(&a.defer_cost))
        });

        let mut out: Vec<Ready> = Vec::with_capacity(self.items.len());

        // We mutate items (updating their deadlines on skip), so
        // walk by index and modify in place.
        for item in self.items.iter_mut() {
            // Cache-defer check.
            let stable = octree.region_stable_since(
                item.region_min,
                item.region_max,
                item.last_computed_tick + 1,
            );

            if stable && item.deadline_tick > current_tick {
                // Skip — push deadline forward; output still valid.
                item.deadline_tick = item.deadline_tick
                    .saturating_add(0)  // already past current_tick
                    .max(current_tick + 1)
                    .saturating_add(0);
                // Use skip_lifetime as the offset relative to
                // current_tick so all skipped items get equal
                // breathing room.
                let new_deadline = current_tick.saturating_add(self.skip_lifetime);
                if new_deadline > item.deadline_tick {
                    item.deadline_tick = new_deadline;
                }
                out.push(Ready { item: *item, kind: ReadyKind::Skipped });
            } else {
                // Dirty (or deadline past). Caller must run it.
                out.push(Ready { item: *item, kind: ReadyKind::Dirty });
            }
        }

        out
    }

    /// Mark an item as processed: update its `last_computed_tick`
    /// to the current tick. Caller passes the same payload they
    /// got from `drain_ready` so the queue can find the entry.
    pub fn mark_processed(
        &mut self,
        kind: WorkKind,
        payload: u64,
        current_tick: u32,
    ) -> bool {
        for item in self.items.iter_mut() {
            if item.kind == kind && item.payload == payload {
                item.last_computed_tick = current_tick;
                return true;
            }
        }
        false
    }

    /// Remove an item from the queue (e.g., when a chunk unloads
    /// and its mesh-refresh entry is no longer relevant).
    pub fn remove(&mut self, kind: WorkKind, payload: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|it| !(it.kind == kind && it.payload == payload));
        self.items.len() < before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::CHUNK_VOLUME;
    use crate::world::blocks::BlockId;
    use crate::world::chunk::{Chunk, ChunkCoord};

    fn make_octree_with_some_structure() -> OctreeArray {
        let mut chunk = Chunk::from_blocks(
            ChunkCoord::new(0, 0, 0),
            vec![BlockId::Air; CHUNK_VOLUME],
        );
        // Sprinkle a few blocks to force tree decomposition.
        chunk.set(2, 2, 2, BlockId::Stone);
        chunk.set(20, 20, 20, BlockId::Dirt);
        OctreeArray::from_chunk(&chunk)
    }

    #[test]
    fn empty_queue_drains_empty() {
        let oct = make_octree_with_some_structure();
        let mut q = DeferQueue::new(4);
        let out = q.drain_ready(&oct, 100);
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn stable_item_is_skipped() {
        let oct = make_octree_with_some_structure();
        let mut q = DeferQueue::new(4);
        q.push(WorkItem {
            kind: WorkKind::MeshChunk,
            payload: 1,
            region_min: (10, 10, 10),
            region_max: (15, 15, 15),
            last_computed_tick: 50,
            deadline_tick: 60,
            defer_cost: 1,
        });
        let out = q.drain_ready(&oct, 55);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, ReadyKind::Skipped);
        // Deadline pushed forward.
        assert!(q.items[0].deadline_tick >= 55 + 4);
    }

    #[test]
    fn dirty_region_emits_dirty() {
        let mut oct = make_octree_with_some_structure();
        let mut q = DeferQueue::new(4);
        q.push(WorkItem {
            kind: WorkKind::MeshChunk,
            payload: 1,
            region_min: (0, 0, 0),
            region_max: (10, 10, 10),
            last_computed_tick: 50,
            deadline_tick: 60,
            defer_cost: 1,
        });
        // Edit inside the dependency region after the item's tick.
        oct.mark_path_modified(2, 2, 2, 55);
        let out = q.drain_ready(&oct, 56);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, ReadyKind::Dirty);
    }

    #[test]
    fn mark_processed_updates_tick() {
        let oct = make_octree_with_some_structure();
        let mut q = DeferQueue::new(4);
        q.push(WorkItem {
            kind: WorkKind::Raycast,
            payload: 42,
            region_min: (0, 0, 0),
            region_max: (16, 16, 16),
            last_computed_tick: 0,
            deadline_tick: 100,
            defer_cost: 5,
        });
        assert!(q.mark_processed(WorkKind::Raycast, 42, 75));
        assert_eq!(q.items[0].last_computed_tick, 75);
        // Subsequent drain should see this item as stable against
        // anything before tick 76.
        let out = q.drain_ready(&oct, 80);
        assert_eq!(out[0].kind, ReadyKind::Skipped);
    }

    #[test]
    fn remove_drops_item() {
        let oct = make_octree_with_some_structure();
        let mut q = DeferQueue::new(4);
        q.push(WorkItem {
            kind: WorkKind::RegionRead,
            payload: 7,
            region_min: (0, 0, 0),
            region_max: (8, 8, 8),
            last_computed_tick: 0,
            deadline_tick: 100,
            defer_cost: 1,
        });
        assert!(q.remove(WorkKind::RegionRead, 7));
        assert!(q.is_empty());
        let out = q.drain_ready(&oct, 50);
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn deadline_past_emits_dirty_even_if_stable() {
        let oct = make_octree_with_some_structure();
        let mut q = DeferQueue::new(4);
        q.push(WorkItem {
            kind: WorkKind::MeshChunk,
            payload: 1,
            region_min: (0, 0, 0),
            region_max: (8, 8, 8),
            last_computed_tick: 50,
            deadline_tick: 50, // already past at current=60
            defer_cost: 1,
        });
        let out = q.drain_ready(&oct, 60);
        assert_eq!(out[0].kind, ReadyKind::Dirty,
            "deadline-past items must process regardless of stability");
    }
}
