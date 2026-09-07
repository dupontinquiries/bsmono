//! Content-keyed cache of upscaled blocks.
//!
//! The reference ring recalls a block that reappears *at the same position*.
//! This catches the rest: a cell (block plus bleed ring) whose exact bytes
//! were upscaled before — anywhere in the frame, any time in the job — is
//! served from memory. Static overlays, letterbox, black, repeated UI, an
//! animation cycle, a logo that moves: all copies. Exact match only, so a
//! hit is the same pixels the network would produce up to the influence of
//! the neighbouring cells in the residual image, which already varies
//! frame to frame.
//!
//! FIFO eviction under a byte budget. Repeats are bursty and cheap to
//! re-learn, so LRU bookkeeping is not worth its cost per frame.

use crate::frame::Frame;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// 64-bit hash of a cell's bytes (a multiply-rotate mix over 8-byte lanes;
/// speed matters more than adversarial resistance here).
pub fn hash_cell(padded: &Frame, x: u32, y: u32, cell: u32) -> u64 {
    const K: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut h: u64 = 0x2545_F491_4F6C_DD1D ^ (cell as u64);
    for j in 0..cell {
        let row = padded.row(y + j, x, x + cell);
        let mut chunks = row.chunks_exact(8);
        for c in &mut chunks {
            let v = u64::from_le_bytes(c.try_into().unwrap());
            h = (h ^ v).wrapping_mul(K).rotate_left(29);
        }
        let mut tail = [0u8; 8];
        let r = chunks.remainder();
        tail[..r.len()].copy_from_slice(r);
        h = (h ^ u64::from_le_bytes(tail) ^ (r.len() as u64) << 56).wrapping_mul(K).rotate_left(29);
    }
    h ^ (h >> 32)
}

#[derive(Debug)]
pub struct BlockCache {
    map: HashMap<u64, Arc<Vec<u8>>>,
    order: VecDeque<u64>,
    bytes: u64,
    pub budget: u64,
    pub hits: u64,
    pub misses: u64,
    pub inserts: u64,
}

impl BlockCache {
    pub fn new(budget: u64) -> Self {
        BlockCache { map: HashMap::new(), order: VecDeque::new(), bytes: 0, budget, hits: 0, misses: 0, inserts: 0 }
    }

    pub fn enabled(&self) -> bool {
        self.budget > 0
    }

    pub fn get(&mut self, key: u64) -> Option<Arc<Vec<u8>>> {
        let r = self.map.get(&key).cloned();
        if r.is_some() {
            self.hits += 1;
        } else {
            self.misses += 1;
        }
        r
    }

    pub fn insert(&mut self, key: u64, block: Arc<Vec<u8>>) {
        if self.budget == 0 || self.map.contains_key(&key) {
            return;
        }
        let n = block.len() as u64 + 32;
        while self.bytes + n > self.budget {
            let Some(old) = self.order.pop_front() else { break };
            if let Some(b) = self.map.remove(&old) {
                self.bytes -= b.len() as u64 + 32;
            }
        }
        if n > self.budget {
            return;
        }
        self.bytes += n;
        self.order.push_back(key);
        self.map.insert(key, block);
        self.inserts += 1;
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_distinguishes_and_repeats() {
        let mut f = Frame::new(40, 40);
        for (i, b) in f.data.iter_mut().enumerate() {
            *b = ((i * 31) % 251) as u8;
        }
        let a = hash_cell(&f, 0, 0, 18);
        assert_eq!(a, hash_cell(&f, 0, 0, 18));
        assert_ne!(a, hash_cell(&f, 1, 0, 18));
        let mut g = f.clone();
        g.set_px(17, 17, [1, 2, 3]);
        assert_ne!(a, hash_cell(&g, 0, 0, 18), "last pixel of the cell must matter");
    }

    #[test]
    fn evicts_fifo_under_budget() {
        let mut c = BlockCache::new(3 * (100 + 32));
        for k in 0..5u64 {
            c.insert(k, Arc::new(vec![0u8; 100]));
        }
        assert_eq!(c.len(), 3);
        assert!(c.get(0).is_none() && c.get(1).is_none());
        assert!(c.get(2).is_some() && c.get(4).is_some());
        assert_eq!((c.hits, c.misses, c.inserts), (2, 2, 5));
        let mut off = BlockCache::new(0);
        off.insert(1, Arc::new(vec![0; 10]));
        assert!(off.is_empty() && !off.enabled());
    }
}
