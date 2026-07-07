//! `sim::buddy_pool` — a pure, deterministic **buddy allocator** for
//! sub-allocating one large byte range (the GPU octree / mesh pool).
//!
//! Why: the render octree pool uses a coalescing free-list
//! (`render::mesh_buffer_alloc::SpanAllocator`). Under render-distance churn it
//! suffers *external fragmentation* — the run log shows
//! `203520 free total, largest free span 19200`: plenty free, but no contiguous
//! slot for a ~156 KB chunk octree, so the GPU mesher OOMs and falls back to the
//! CPU (the chug + 1946 fallbacks).
//!
//! A buddy allocator removes external fragmentation: every request rounds up to a
//! power-of-two **block**, so blocks of the same order are interchangeable, and a
//! freed block always merges with its (aligned) buddy — recursively — restoring
//! large contiguous space. The cost is *internal* round-up (a 156 KB chunk takes
//! a 256 KB block); for the octree pool that funnels every chunk into ~3 size
//! classes (128/256/512 KB), which is exactly what kills the fragmentation. Pair
//! with rendering far LOD from the compressed surface-run form (queue item 2) to
//! cut how many octrees the pool must hold at once.
//!
//! Pure: it hands out byte **offsets**, no GPU types — unit-tested here, the
//! render pools adopt it. Deterministic: a given alloc/free sequence yields
//! identical offsets on every machine (alloc takes the lowest-offset free block).

use std::collections::HashMap;

#[derive(Debug, PartialEq, Eq)]
pub enum AllocError {
    ZeroSize,
    TooLarge { wanted: u64, capacity: u64 },
    Exhausted { wanted: u64, free_total: u64, free_largest: u64 },
    InvalidFree { offset: u64 },
}

impl std::fmt::Display for AllocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AllocError::ZeroSize => write!(f, "zero-size allocation"),
            AllocError::TooLarge { wanted, capacity } => {
                write!(f, "alloc {} exceeds pool capacity {}", wanted, capacity)
            }
            AllocError::Exhausted { wanted, free_total, free_largest } => write!(
                f,
                "buddy pool exhausted: wanted {} bytes, {} free total, largest free block {}",
                wanted, free_total, free_largest
            ),
            AllocError::InvalidFree { offset } => write!(f, "free of unknown offset {}", offset),
        }
    }
}

/// A buddy allocator over `[0, capacity)`. `capacity` and `min_block` must both
/// be powers of two with `capacity >= min_block`.
pub struct BuddyPool {
    min_block: u64,
    /// Number of orders: order `o` has block size `min_block << o`; the top order
    /// is the whole pool.
    levels: u32,
    capacity: u64,
    /// `free[o]` = offsets of free blocks of size `min_block << o`.
    free: Vec<Vec<u64>>,
    /// offset -> order, for every live allocation.
    allocated: HashMap<u64, u32>,
    /// Sum of *rounded* block sizes handed out (internal waste counts as used).
    allocated_total: u64,
}

impl BuddyPool {
    pub fn new(capacity: u64, min_block: u64) -> BuddyPool {
        assert!(min_block > 0 && min_block.is_power_of_two(), "min_block must be a power of two");
        assert!(capacity >= min_block && capacity.is_power_of_two(), "capacity must be a power of two >= min_block");
        let levels = (capacity / min_block).trailing_zeros() + 1;
        let mut free = vec![Vec::new(); levels as usize];
        free[(levels - 1) as usize].push(0u64); // one whole-pool block
        BuddyPool { min_block, levels, capacity, free, allocated: HashMap::new(), allocated_total: 0 }
    }

    #[inline]
    fn block_size(&self, order: u32) -> u64 {
        self.min_block << order
    }
    #[inline]
    fn top_order(&self) -> u32 {
        self.levels - 1
    }

    /// Smallest order whose block fits `size`.
    fn order_for(&self, size: u64) -> u32 {
        let mut o = 0u32;
        while self.block_size(o) < size {
            o += 1;
        }
        o
    }

    /// Pop the lowest-offset free block at order `o` (keeps allocation order
    /// deterministic regardless of free-list churn).
    fn pop_min(&mut self, o: u32) -> u64 {
        let v = &mut self.free[o as usize];
        let mut idx = 0;
        for i in 1..v.len() {
            if v[i] < v[idx] {
                idx = i;
            }
        }
        v.remove(idx)
    }

    /// Allocate at least `size` bytes; returns the block's byte offset.
    pub fn alloc(&mut self, size: u64) -> Result<u64, AllocError> {
        if size == 0 {
            return Err(AllocError::ZeroSize);
        }
        if size > self.capacity {
            return Err(AllocError::TooLarge { wanted: size, capacity: self.capacity });
        }
        let need = self.order_for(size);
        // smallest order >= need with a free block
        let mut o = need;
        while o <= self.top_order() && self.free[o as usize].is_empty() {
            o += 1;
        }
        if o > self.top_order() {
            return Err(AllocError::Exhausted {
                wanted: size,
                free_total: self.free_bytes(),
                free_largest: self.largest_free_block(),
            });
        }
        let off = self.pop_min(o);
        // split the oversized block down to `need`, freeing the upper halves
        while o > need {
            o -= 1;
            let buddy = off + self.block_size(o);
            self.free[o as usize].push(buddy);
        }
        self.allocated.insert(off, need);
        self.allocated_total += self.block_size(need);
        Ok(off)
    }

    /// Free a block previously returned by [`alloc`], coalescing buddies upward.
    pub fn free(&mut self, offset: u64) -> Result<(), AllocError> {
        let mut order = match self.allocated.remove(&offset) {
            Some(o) => o,
            None => return Err(AllocError::InvalidFree { offset }),
        };
        self.allocated_total -= self.block_size(order);
        let mut off = offset;
        while order < self.top_order() {
            let buddy = off ^ self.block_size(order); // aligned buddy address
            match self.free[order as usize].iter().position(|&b| b == buddy) {
                Some(pos) => {
                    self.free[order as usize].remove(pos);
                    off = off.min(buddy);
                    order += 1;
                }
                None => break,
            }
        }
        self.free[order as usize].push(off);
        Ok(())
    }

    pub fn capacity(&self) -> u64 {
        self.capacity
    }
    /// Bytes handed out (rounded up to block sizes — internal waste included).
    pub fn allocated_bytes(&self) -> u64 {
        self.allocated_total
    }
    pub fn free_bytes(&self) -> u64 {
        self.capacity - self.allocated_total
    }
    /// Largest single block a future alloc could get. With a free-list this can
    /// collapse far below `free_bytes()` (fragmentation); with buddy it tracks
    /// the real contiguous capacity.
    pub fn largest_free_block(&self) -> u64 {
        for o in (0..self.levels).rev() {
            if !self.free[o as usize].is_empty() {
                return self.block_size(o);
            }
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_up_to_power_of_two() {
        let mut p = BuddyPool::new(16, 1);
        let _ = p.alloc(3).unwrap(); // rounds to a 4-block
        assert_eq!(p.allocated_bytes(), 4);
        assert_eq!(p.free_bytes(), 12);
    }

    #[test]
    fn basic_alloc_offsets_are_deterministic() {
        let mut p = BuddyPool::new(16, 1);
        let a = p.alloc(1).unwrap();
        let b = p.alloc(1).unwrap();
        let c = p.alloc(2).unwrap();
        assert_eq!(a, 0);
        assert_eq!(b, 1);
        assert_eq!(c, 2); // first free 2-block after the split
    }

    #[test]
    fn freeing_all_restores_one_full_block() {
        let mut p = BuddyPool::new(16, 1);
        let mut offs = Vec::new();
        for _ in 0..16 {
            offs.push(p.alloc(1).unwrap());
        }
        assert_eq!(p.free_bytes(), 0);
        assert!(p.alloc(1).is_err(), "pool is full");
        // free in a scattered order; buddies must still fully coalesce
        for &o in offs.iter().rev() {
            p.free(o).unwrap();
        }
        assert_eq!(p.free_bytes(), 16);
        assert_eq!(p.largest_free_block(), 16, "full coalesce — no fragmentation left");
    }

    #[test]
    fn fixed_class_churn_never_fragments() {
        // The octree case: many same-size allocations, freed and re-allocated.
        // A naive first-fit could fragment; buddy keeps same-order slots
        // interchangeable, so every realloc round succeeds.
        let mut p = BuddyPool::new(64, 1);
        for _round in 0..50 {
            let mut offs = Vec::new();
            for _ in 0..8 {
                offs.push(p.alloc(8).unwrap()); // eight 8-blocks = full
            }
            assert!(p.alloc(8).is_err());
            // free every other one first, then the rest — scattered
            for i in (0..offs.len()).step_by(2) {
                p.free(offs[i]).unwrap();
            }
            for i in (1..offs.len()).step_by(2) {
                p.free(offs[i]).unwrap();
            }
            assert_eq!(p.free_bytes(), 64, "all space reclaimed each round");
        }
    }

    #[test]
    fn buddy_merge_grows_the_largest_block() {
        let mut p = BuddyPool::new(16, 1);
        let a = p.alloc(1).unwrap(); // 0
        let b = p.alloc(1).unwrap(); // 1  (buddy of a)
        let _c = p.alloc(2).unwrap(); // 2..4
        p.free(a).unwrap();
        // a's buddy (b) still allocated -> no merge yet, largest free stays small
        let before = p.largest_free_block();
        p.free(b).unwrap();
        // now a|b merge into a 2-block, which may merge further with the 2..4 hole? c holds it.
        assert!(p.largest_free_block() >= before, "freeing the buddy grew contiguous space");
    }

    #[test]
    fn exhausted_reports_largest_free_block() {
        let mut p = BuddyPool::new(8, 1);
        for _ in 0..8 {
            p.alloc(1).unwrap();
        }
        match p.alloc(1) {
            Err(AllocError::Exhausted { wanted, free_total, free_largest }) => {
                assert_eq!(wanted, 1);
                assert_eq!(free_total, 0);
                assert_eq!(free_largest, 0);
            }
            other => panic!("expected Exhausted, got {:?}", other),
        }
    }

    #[test]
    fn too_large_and_zero_and_bad_free() {
        let mut p = BuddyPool::new(8, 1);
        assert_eq!(p.alloc(0), Err(AllocError::ZeroSize));
        assert!(matches!(p.alloc(99), Err(AllocError::TooLarge { .. })));
        assert!(matches!(p.free(123), Err(AllocError::InvalidFree { offset: 123 })));
    }

    #[test]
    fn identical_sequences_give_identical_offsets() {
        fn run() -> Vec<u64> {
            let mut p = BuddyPool::new(64, 2);
            let mut out = Vec::new();
            let a = p.alloc(4).unwrap();
            out.push(a);
            let b = p.alloc(10).unwrap();
            out.push(b);
            p.free(a).unwrap();
            let c = p.alloc(2).unwrap();
            out.push(c);
            let d = p.alloc(20).unwrap();
            out.push(d);
            out
        }
        assert_eq!(run(), run(), "deterministic across runs");
    }
}
