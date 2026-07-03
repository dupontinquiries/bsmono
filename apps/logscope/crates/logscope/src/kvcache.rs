//! kvcache.rs — paged KV-cache block allocation (PagedAttention-style): the pure bookkeeping that lets the
//! engine pack many sequences into a fixed KV budget without reserving `max_len` each.
//!
//! Physical KV storage is divided into fixed-size **blocks** (`block_size` tokens of K/V each). Every
//! sequence keeps a **block table** mapping its logical positions to physical blocks, and grows it one
//! block at a time only when it crosses a block boundary — so memory tracks actual length, not the worst
//! case, and uniform blocks mean no external fragmentation. Blocks are **ref-counted**: [`fork`](Paged::fork)
//! shares a parent's blocks with a child (prefix sharing — e.g. the many fanout lanes off one prompt), and
//! a write into a still-shared partial block triggers **copy-on-write** so the siblings stay isolated.
//!
//! This composes under `batching`: admit a request only when [`can_admit`](Paged::can_admit); `append` as
//! it decodes; `free` on completion. The K/V tensor storage and the paged-attention kernel are the GPU
//! seam (this module hands out block ids and says when to copy); the allocation logic is here and tested.
#![allow(dead_code)]

use std::collections::HashMap;

pub type Block = u32; // physical block id
pub type SeqId = u64;

/// Blocks required to hold `tokens` tokens.
pub fn blocks_for(tokens: usize, block_size: usize) -> usize {
    let bs = block_size.max(1);
    (tokens + bs - 1) / bs
}

#[derive(Debug, PartialEq, Eq)]
pub struct Oom; // out of physical blocks

/// A paged KV-cache allocator over a fixed pool of physical blocks.
pub struct Paged {
    block_size: usize,
    free: Vec<Block>,
    refcount: Vec<u16>,
    tables: HashMap<SeqId, Vec<Block>>,
    lengths: HashMap<SeqId, usize>,
}

impl Paged {
    pub fn new(num_blocks: usize, block_size: usize) -> Self {
        Self {
            block_size: block_size.max(1),
            free: (0..num_blocks as Block).rev().collect(), // pop() hands out 0,1,2,…
            refcount: vec![0; num_blocks],
            tables: HashMap::new(),
            lengths: HashMap::new(),
        }
    }

    pub fn block_size(&self) -> usize { self.block_size }
    pub fn total_blocks(&self) -> usize { self.refcount.len() }
    pub fn free_blocks(&self) -> usize { self.free.len() }
    pub fn used_blocks(&self) -> usize { self.total_blocks() - self.free_blocks() }
    pub fn block_table(&self, seq: SeqId) -> Option<&[Block]> { self.tables.get(&seq).map(|v| v.as_slice()) }
    pub fn seq_len(&self, seq: SeqId) -> Option<usize> { self.lengths.get(&seq).copied() }
    /// Could a fresh sequence of `n_tokens` be admitted right now?
    pub fn can_admit(&self, n_tokens: usize) -> bool { self.free.len() >= blocks_for(n_tokens, self.block_size) }

    fn alloc(&mut self) -> Option<Block> {
        let b = self.free.pop()?;
        self.refcount[b as usize] = 1;
        Some(b)
    }

    /// Admit a new sequence and allocate blocks for its `n_prompt_tokens` prefill.
    pub fn admit(&mut self, seq: SeqId, n_prompt_tokens: usize) -> Result<(), Oom> {
        let need = blocks_for(n_prompt_tokens, self.block_size);
        if self.free.len() < need { return Err(Oom); }
        let table = (0..need).map(|_| self.alloc().unwrap()).collect();
        self.tables.insert(seq, table);
        self.lengths.insert(seq, n_prompt_tokens);
        Ok(())
    }

    /// Extend `seq` by `n` tokens, allocating blocks as boundaries are crossed and copying-on-write the
    /// current partial block if it is still shared. Atomic: if the blocks aren't available, nothing changes.
    pub fn append(&mut self, seq: SeqId, n: usize) -> Result<(), Oom> {
        let Some(&len) = self.lengths.get(&seq) else { return Err(Oom) };
        let bs = self.block_size;
        let table = self.tables.get(&seq).unwrap();
        let growth = blocks_for(len + n, bs) - blocks_for(len, bs);
        // a write into the existing partial last block needs COW if that block is still shared
        let cow = if len % bs != 0 {
            let last = *table.last().unwrap();
            usize::from(self.refcount[last as usize] > 1)
        } else { 0 };
        if self.free.len() < growth + cow { return Err(Oom); }

        let mut table = self.tables.get(&seq).unwrap().clone();
        if cow == 1 {
            let li = table.len() - 1;
            let old = table[li];
            let nb = self.alloc().unwrap(); // engine copies old→nb
            self.refcount[old as usize] -= 1;
            table[li] = nb;
        }
        for _ in 0..growth { let b = self.alloc().unwrap(); table.push(b); }
        self.tables.insert(seq, table);
        self.lengths.insert(seq, len + n);
        Ok(())
    }

    /// Share `parent`'s current blocks with a new `child` (prefix sharing, no new blocks). Subsequent
    /// writes copy-on-write so the two never corrupt each other.
    pub fn fork(&mut self, child: SeqId, parent: SeqId) -> Result<(), Oom> {
        let table = self.tables.get(&parent).ok_or(Oom)?.clone();
        let len = self.lengths[&parent];
        for &b in &table { self.refcount[b as usize] += 1; }
        self.tables.insert(child, table);
        self.lengths.insert(child, len);
        Ok(())
    }

    /// Release a sequence; blocks whose refcount hits zero return to the free pool.
    pub fn free(&mut self, seq: SeqId) {
        if let Some(table) = self.tables.remove(&seq) {
            self.lengths.remove(&seq);
            for &b in &table {
                let rc = &mut self.refcount[b as usize];
                *rc = rc.saturating_sub(1);
                if *rc == 0 { self.free.push(b); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_for_is_ceil_div() {
        assert_eq!(blocks_for(0, 4), 0);
        assert_eq!(blocks_for(1, 4), 1);
        assert_eq!(blocks_for(4, 4), 1);
        assert_eq!(blocks_for(5, 4), 2);
    }

    #[test]
    fn admit_allocates_prefill_blocks_and_can_oom() {
        let mut p = Paged::new(3, 4); // 3 blocks × 4 tokens
        assert!(p.admit(1, 6).is_ok()); // ceil(6/4)=2 blocks
        assert_eq!(p.used_blocks(), 2);
        assert_eq!(p.block_table(1).unwrap().len(), 2);
        assert_eq!(p.admit(2, 9), Err(Oom)); // needs 3, only 1 free
        assert!(p.admit(3, 4).is_ok());      // 1 block fits
        assert_eq!(p.free_blocks(), 0);
    }

    #[test]
    fn append_only_grows_on_block_boundaries() {
        let mut p = Paged::new(8, 4);
        p.admit(1, 1).unwrap();          // 1 block, len 1
        let before = p.free_blocks();
        p.append(1, 2).unwrap();         // len 1→3, still in block 0
        assert_eq!(p.free_blocks(), before, "no new block within a block");
        p.append(1, 2).unwrap();         // len 3→5, crosses into block 1
        assert_eq!(p.free_blocks(), before - 1);
        assert_eq!(p.block_table(1).unwrap().len(), 2);
    }

    #[test]
    fn free_returns_blocks_to_the_pool() {
        let mut p = Paged::new(4, 4);
        p.admit(1, 8).unwrap(); // 2 blocks
        assert_eq!(p.free_blocks(), 2);
        p.free(1);
        assert_eq!(p.free_blocks(), 4);
        assert!(p.block_table(1).is_none());
    }

    #[test]
    fn fork_shares_blocks_without_allocating() {
        let mut p = Paged::new(8, 4);
        p.admit(1, 6).unwrap();          // 2 blocks
        let free_after_admit = p.free_blocks();
        p.fork(2, 1).unwrap();
        assert_eq!(p.free_blocks(), free_after_admit, "sharing allocates nothing");
        assert_eq!(p.block_table(1), p.block_table(2), "child sees the same physical blocks");
        assert_eq!(p.seq_len(2), Some(6));
    }

    #[test]
    fn append_after_fork_copies_on_write() {
        let mut p = Paged::new(8, 4);
        p.admit(1, 2).unwrap();          // 1 partial block, len 2
        p.fork(2, 1).unwrap();           // child shares it (refcount 2)
        let free_before = p.free_blocks();
        p.append(1, 1).unwrap();         // parent writes into the shared partial block → COW
        assert_eq!(p.free_blocks(), free_before - 1, "COW consumed one block");
        assert_ne!(p.block_table(1).unwrap()[0], p.block_table(2).unwrap()[0], "parent diverged onto its own copy");
        assert_eq!(p.seq_len(1), Some(3));
        assert_eq!(p.seq_len(2), Some(2), "child is untouched");
        // freeing the child returns its (now unshared) original block
        let f = p.free_blocks();
        p.free(2);
        assert_eq!(p.free_blocks(), f + 1);
    }

    #[test]
    fn append_is_atomic_under_oom() {
        let mut p = Paged::new(2, 4);
        p.admit(1, 4).unwrap();          // 1 block exactly full, 1 free
        // ask for 8 more tokens → needs 2 new blocks, only 1 free → Oom, no mutation
        assert_eq!(p.append(1, 8), Err(Oom));
        assert_eq!(p.seq_len(1), Some(4));
        assert_eq!(p.block_table(1).unwrap().len(), 1);
        assert_eq!(p.free_blocks(), 1);
    }
}
