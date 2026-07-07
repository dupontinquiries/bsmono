//! GPU chunk storage pool.
//!
//! This is the v0.18.0 increment of the GPU-everything path: chunk block
//! data lives persistently in GPU memory, indexed by a slot handle. The
//! follow-on GPU mesher (v0.18.1) reads from this pool directly without
//! a CPU round-trip.
//!
//! # What the pool is
//!
//! A fixed-capacity array of slots, each large enough to hold one chunk
//! (`CHUNK_VOLUME` bytes packed into u32 words, same layout the existing
//! `chunk_generation.comp` writes). The pool owns the storage; callers
//! receive `PoolSlot` handles. Allocation is O(1) (free-list pop), free
//! is O(1) (push to free-list).
//!
//! # What the pool is NOT (yet, in v0.18.0)
//!
//! - Not the source of truth for block data. The CPU `Chunk` struct in
//!   `chunk_manager` remains canonical. The pool is a GPU mirror that
//!   downstream GPU passes can read from.
//! - Not the input to the GPU generator yet. v0.18.0 ships the pool as
//!   a separable component; the generator is rewired in a follow-up
//!   sub-step and validated on hardware before the production path
//!   flips.
//! - Not a sparse paging system. Slots are full chunks; a single u32
//!   word represents one slot, and the slot is fully resident or absent.
//!
//! # Backend abstraction
//!
//! The pool surface is a trait so the same orchestration logic
//! (`ChunkManager::ensure_pool_slot` etc.) can drive either the real
//! Vulkan-backed pool or a CPU-shadow stub. The CPU stub exists to
//! keep the unit tests independent of Vulkan availability — every
//! sandbox / CI runner can exercise the slot lifecycle without a GPU.
//!
//! The Vulkan backend lives behind `#[cfg(feature = "gpu")]` in
//! `gpu_chunk_pool_vk.rs`.

use crate::constants::CHUNK_VOLUME;
use crate::world::blocks::BlockId;
use crate::world::chunk::ChunkCoord;
use std::collections::VecDeque;

/// One slot's packed-word footprint. CHUNK_VOLUME bytes packed 4-per-u32,
/// matching the layout `chunk_generation.comp` writes. Keeping this as a
/// `const` rather than a runtime value makes it trivial to validate at
/// compile time that the pool size and the shader's per-chunk stride agree.
pub const SLOT_WORDS: usize = CHUNK_VOLUME / 4;

/// Bytes per slot.
pub const SLOT_BYTES: usize = SLOT_WORDS * std::mem::size_of::<u32>();

/// Opaque handle to a pool slot. The contained index is the slot's
/// position within the pool's storage array. `version` is the chunk
/// version this slot's contents correspond to — callers compare it
/// against their `Chunk::version` to know whether the slot is stale.
///
/// `PoolSlot` is `Copy` and lives wherever the chunk lives (typically
/// alongside the `Arc<Chunk>` in `ChunkManager`). It is NOT itself a
/// resource that needs to be released — releasing is done explicitly
/// via `ChunkPool::free` when the chunk is evicted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolSlot {
    pub index: u32,
    pub version: u64,
}

/// Errors from pool operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolError {
    /// Pool is at capacity; no free slots available.
    Exhausted { capacity: u32 },
    /// Caller passed a `PoolSlot` that this pool does not recognize
    /// (out-of-range index, or freed-and-not-realloced slot).
    InvalidSlot { index: u32 },
    /// Backend-specific failure (Vulkan error, mostly).
    Backend(String),
}

impl std::fmt::Display for PoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PoolError::Exhausted { capacity } => {
                write!(f, "GPU chunk pool exhausted (capacity {})", capacity)
            }
            PoolError::InvalidSlot { index } => {
                write!(f, "Invalid pool slot index {}", index)
            }
            PoolError::Backend(s) => write!(f, "GPU chunk pool backend error: {}", s),
        }
    }
}
impl std::error::Error for PoolError {}

/// Backend-agnostic interface to a chunk storage pool.
///
/// Lifecycle of a slot:
///   1. `alloc(coord, version)` — caller asks for a slot for a given
///      chunk. Returns a `PoolSlot` handle.
///   2. `upload(slot, blocks)` — caller writes block data into the
///      slot. May be skipped if the next stage (e.g. GPU generator)
///      is going to write into the slot directly via the underlying
///      buffer binding.
///   3. ... downstream GPU passes (mesher, lighting) read from the
///      slot via the backend-specific buffer binding ...
///   4. `free(slot)` — caller releases the slot when the chunk evicts.
///
/// The trait deliberately does NOT expose the underlying buffer
/// binding — that is backend-specific (Vulkan descriptor set, OpenGL
/// SSBO binding, etc.) and lives in the concrete impl.
pub trait ChunkPool: Send + Sync {
    /// Total slot capacity.
    fn capacity(&self) -> u32;

    /// Number of currently-allocated slots.
    fn len(&self) -> u32;

    fn is_empty(&self) -> bool { self.len() == 0 }

    /// Allocate a slot for the given chunk. The `version` is recorded
    /// in the returned `PoolSlot` and is used by callers to detect
    /// stale uploads.
    fn alloc(&mut self, coord: ChunkCoord, version: u64) -> Result<PoolSlot, PoolError>;

    /// Upload block data into a slot. The slot must have been
    /// returned by a prior `alloc` and not yet freed.
    fn upload(&mut self, slot: PoolSlot, blocks: &[BlockId]) -> Result<(), PoolError>;

    /// Release a slot back to the pool's free list.
    fn free(&mut self, slot: PoolSlot) -> Result<(), PoolError>;

    /// Read block data back out of a slot. This is provided primarily
    /// for testing and for the eventual round-trip path where chunks
    /// are GPU-generated and the CPU needs to mirror the result. In
    /// the steady-state v0.18.x design, this is rarely called on the
    /// hot path — most CPU-side block reads come from the canonical
    /// `Chunk::blocks` Vec.
    fn read(&self, slot: PoolSlot) -> Result<Vec<BlockId>, PoolError>;

    /// Inspect what coord is currently held in a slot. Used by
    /// debugging / verification code; not on any hot path.
    fn coord_of(&self, slot: PoolSlot) -> Result<ChunkCoord, PoolError>;
}

/// Internal slot record. Lives in the pool's slots array indexed by
/// `PoolSlot::index`. `Some` means allocated, `None` means free.
#[derive(Debug, Clone)]
struct SlotRecord {
    coord: ChunkCoord,
    version: u64,
}

/// CPU-shadow implementation of `ChunkPool`. Keeps slot data in a
/// `Vec<Vec<BlockId>>` and exists primarily so the slot lifecycle
/// logic (alloc, upload, free, read) can be unit-tested without
/// needing a Vulkan device.
///
/// The Vulkan backend reuses the same allocator structure and slot
/// ID semantics, so any logic exercised against `CpuShadowPool` is
/// directly transferable.
pub struct CpuShadowPool {
    capacity: u32,
    slots: Vec<Option<SlotRecord>>,
    /// Block storage, one Vec per slot. None when the slot is free
    /// or when the slot has been allocated but not yet uploaded.
    storage: Vec<Option<Vec<BlockId>>>,
    free_list: VecDeque<u32>,
    allocated: u32,
}

impl CpuShadowPool {
    pub fn new(capacity: u32) -> Self {
        let cap = capacity as usize;
        let mut free_list = VecDeque::with_capacity(cap);
        for i in 0..capacity {
            free_list.push_back(i);
        }
        Self {
            capacity,
            slots: vec![None; cap],
            storage: vec![None; cap],
            free_list,
            allocated: 0,
        }
    }

    /// Validate that a `PoolSlot` corresponds to a currently-allocated
    /// slot whose recorded version matches the handle. Returns the
    /// slot index as `usize` for indexing into `slots`/`storage`.
    fn validate(&self, slot: PoolSlot) -> Result<usize, PoolError> {
        let idx = slot.index as usize;
        if idx >= self.slots.len() {
            return Err(PoolError::InvalidSlot { index: slot.index });
        }
        match &self.slots[idx] {
            Some(rec) if rec.version == slot.version => Ok(idx),
            _ => Err(PoolError::InvalidSlot { index: slot.index }),
        }
    }
}

impl ChunkPool for CpuShadowPool {
    fn capacity(&self) -> u32 { self.capacity }
    fn len(&self) -> u32 { self.allocated }

    fn alloc(&mut self, coord: ChunkCoord, version: u64) -> Result<PoolSlot, PoolError> {
        let idx = self.free_list.pop_front()
            .ok_or(PoolError::Exhausted { capacity: self.capacity })?;
        let i = idx as usize;
        debug_assert!(self.slots[i].is_none(),
            "free-list invariant violation: slot {} on free-list but allocated", idx);
        self.slots[i] = Some(SlotRecord { coord, version });
        self.storage[i] = None; // explicit: contents are undefined until uploaded
        self.allocated += 1;
        Ok(PoolSlot { index: idx, version })
    }

    fn upload(&mut self, slot: PoolSlot, blocks: &[BlockId]) -> Result<(), PoolError> {
        if blocks.len() != CHUNK_VOLUME {
            return Err(PoolError::Backend(format!(
                "upload size {} != CHUNK_VOLUME {}", blocks.len(), CHUNK_VOLUME
            )));
        }
        let i = self.validate(slot)?;
        self.storage[i] = Some(blocks.to_vec());
        Ok(())
    }

    fn free(&mut self, slot: PoolSlot) -> Result<(), PoolError> {
        let i = self.validate(slot)?;
        self.slots[i] = None;
        self.storage[i] = None;
        self.free_list.push_back(slot.index);
        self.allocated -= 1;
        Ok(())
    }

    fn read(&self, slot: PoolSlot) -> Result<Vec<BlockId>, PoolError> {
        let i = self.validate(slot)?;
        match &self.storage[i] {
            Some(v) => Ok(v.clone()),
            None => Err(PoolError::Backend(format!(
                "slot {} is allocated but contents have not been uploaded", slot.index
            ))),
        }
    }

    fn coord_of(&self, slot: PoolSlot) -> Result<ChunkCoord, PoolError> {
        let i = self.validate(slot)?;
        // safe: validate() guarantees Some
        Ok(self.slots[i].as_ref().unwrap().coord)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cc(x: i32, y: i32, z: i32) -> ChunkCoord { ChunkCoord { x, y, z } }

    fn dummy_blocks() -> Vec<BlockId> {
        let mut v = vec![BlockId::Air; CHUNK_VOLUME];
        // a few non-air entries to make read-back distinguishable from default
        v[0] = BlockId::Stone;
        v[CHUNK_VOLUME - 1] = BlockId::Dirt;
        v
    }

    #[test]
    fn slot_words_matches_chunk_volume() {
        assert_eq!(SLOT_WORDS * 4, CHUNK_VOLUME);
        assert_eq!(SLOT_BYTES, CHUNK_VOLUME); // 1 byte per block, packed 4 per u32
    }

    #[test]
    fn alloc_and_free_cycle() {
        let mut pool = CpuShadowPool::new(4);
        assert_eq!(pool.capacity(), 4);
        assert_eq!(pool.len(), 0);

        let s = pool.alloc(cc(0, 0, 0), 1).unwrap();
        assert_eq!(pool.len(), 1);
        assert_eq!(pool.coord_of(s).unwrap(), cc(0, 0, 0));

        pool.free(s).unwrap();
        assert_eq!(pool.len(), 0);

        // After free, the slot handle should be invalid.
        assert!(matches!(pool.coord_of(s), Err(PoolError::InvalidSlot { .. })));
    }

    #[test]
    fn upload_and_read_roundtrip() {
        let mut pool = CpuShadowPool::new(2);
        let s = pool.alloc(cc(1, 2, 3), 7).unwrap();
        let data = dummy_blocks();
        pool.upload(s, &data).unwrap();
        let got = pool.read(s).unwrap();
        assert_eq!(got, data);
    }

    #[test]
    fn read_before_upload_errors() {
        let mut pool = CpuShadowPool::new(1);
        let s = pool.alloc(cc(0, 0, 0), 0).unwrap();
        assert!(pool.read(s).is_err());
    }

    #[test]
    fn upload_wrong_size_errors() {
        let mut pool = CpuShadowPool::new(1);
        let s = pool.alloc(cc(0, 0, 0), 0).unwrap();
        assert!(pool.upload(s, &[BlockId::Air; 100]).is_err());
    }

    #[test]
    fn exhaustion_returns_error() {
        let mut pool = CpuShadowPool::new(2);
        let _a = pool.alloc(cc(0, 0, 0), 0).unwrap();
        let _b = pool.alloc(cc(1, 0, 0), 0).unwrap();
        match pool.alloc(cc(2, 0, 0), 0) {
            Err(PoolError::Exhausted { capacity: 2 }) => {}
            other => panic!("expected Exhausted(2), got {:?}", other),
        }
    }

    #[test]
    fn version_mismatch_invalidates_handle() {
        // If a slot is freed and re-allocated, the OLD handle must
        // not work (even though the index is the same). This is what
        // protects mesher / downstream from racing against eviction.
        let mut pool = CpuShadowPool::new(1);
        let old = pool.alloc(cc(0, 0, 0), 1).unwrap();
        pool.free(old).unwrap();
        let new = pool.alloc(cc(5, 5, 5), 2).unwrap();
        // The recycled index is the same...
        assert_eq!(old.index, new.index);
        // ...but the version differs, so the old handle is dead.
        assert!(matches!(pool.coord_of(old), Err(PoolError::InvalidSlot { .. })));
        // The new handle works.
        assert_eq!(pool.coord_of(new).unwrap(), cc(5, 5, 5));
    }

    #[test]
    fn free_list_recycles_in_order() {
        // Sanity: free-list is FIFO so we don't accidentally hot-spot
        // one slot. Important for the GPU backend where a freshly-
        // freed slot's memory may still be in flight.
        let mut pool = CpuShadowPool::new(3);
        let a = pool.alloc(cc(0, 0, 0), 0).unwrap();
        let b = pool.alloc(cc(1, 0, 0), 0).unwrap();
        let c = pool.alloc(cc(2, 0, 0), 0).unwrap();
        assert_eq!([a.index, b.index, c.index], [0, 1, 2]);
        pool.free(a).unwrap();
        pool.free(b).unwrap();
        let d = pool.alloc(cc(3, 0, 0), 0).unwrap();
        assert_eq!(d.index, 0); // recycled the first-freed
        let e = pool.alloc(cc(4, 0, 0), 0).unwrap();
        assert_eq!(e.index, 1); // then the second-freed
        let _ = c;
    }

    #[test]
    fn invalid_index_handle_is_rejected() {
        let pool = CpuShadowPool::new(2);
        let bogus = PoolSlot { index: 99, version: 0 };
        assert!(matches!(pool.coord_of(bogus), Err(PoolError::InvalidSlot { .. })));
    }

    #[test]
    fn many_alloc_free_cycles_no_leak() {
        // Stress: alloc and free repeatedly, ensure capacity is held.
        let mut pool = CpuShadowPool::new(8);
        for i in 0..1000 {
            let s = pool.alloc(cc(i, 0, 0), i as u64).unwrap();
            pool.free(s).unwrap();
        }
        assert_eq!(pool.len(), 0);
        // Should still be able to fully fill.
        let mut handles = Vec::new();
        for i in 0..8 {
            handles.push(pool.alloc(cc(i, 0, 0), 0).unwrap());
        }
        assert_eq!(pool.len(), 8);
        assert!(pool.alloc(cc(99, 0, 0), 0).is_err());
        for h in handles { pool.free(h).unwrap(); }
        assert_eq!(pool.len(), 0);
    }
}
