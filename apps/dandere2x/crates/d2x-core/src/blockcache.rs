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

/// Coarse content key: the cell reduced to a `G`×`G` grid of per-channel
/// means, quantised. Cells that differ only by codec noise or a fraction of
/// a code value land in the same bucket; the exact hash never does.
///
/// This is the lever that matters. An exact-byte cache measures 0.1% hits on
/// real footage no matter how much is fed into it (harvesting tripled the
/// supply and moved hits from 34 to 41 in 120 frames), because two cells of
/// live-action video are almost never byte-identical. Bucketing coarsely and
/// then *verifying* against the same gate every other reference in this
/// pipeline uses turns the cache from an exact-repeat detector into an
/// ordinary approximate reference.
pub fn coarse_key(padded: &Frame, x: u32, y: u32, cell: u32, quant: u32) -> u64 {
    // Two per axis: 12 quantised numbers. A 4x4 grid (48 numbers) was tried
    // first and measured no better than exact matching — hashing quantised
    // values is still an exact match in a coarser space, and in 48
    // dimensions the chance that every one lands in the same bin (bin
    // boundaries included) is negligible. Fewer dimensions is the only way
    // a hash bucket collides at all; a real fix is an LSH or a small ANN
    // index over a learned descriptor, not a finer/coarser quantiser.
    const G: u32 = 2;
    const K: u64 = 0x9E37_79B9_7F4A_7C15;
    let q = quant.max(1);
    let mut h: u64 = 0x51ED_2701_A5B3_C7D9 ^ ((cell as u64) << 32) ^ (q as u64);
    for gy in 0..G {
        for gx in 0..G {
            let (x0, x1) = (x + gx * cell / G, x + (gx + 1) * cell / G);
            let (y0, y1) = (y + gy * cell / G, y + (gy + 1) * cell / G);
            let mut sum = [0u64; 3];
            for yy in y0..y1 {
                for px in padded.row(yy, x0, x1).chunks_exact(3) {
                    sum[0] += px[0] as u64;
                    sum[1] += px[1] as u64;
                    sum[2] += px[2] as u64;
                }
            }
            let n = ((x1 - x0) * (y1 - y0)).max(1) as u64;
            for c in 0..3 {
                h = (h ^ ((sum[c] / n) / q as u64)).wrapping_mul(K).rotate_left(23);
            }
        }
    }
    h ^ (h >> 32)
}

/// One cached upscale plus the low-res block it came from, which is what a
/// gated probe verifies against.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Upscaled `block·scale` square, rgb24. Empty in a dry cache.
    pub up: Arc<Vec<u8>>,
    /// The low-res `block` square this was produced from, rgb24.
    pub lo: Arc<Vec<u8>>,
}

#[derive(Debug)]
pub struct BlockCache {
    map: HashMap<u64, Arc<Vec<u8>>>,
    /// Charged bytes per key (differs from the payload for a dry cache).
    cost: HashMap<u64, u64>,
    /// Gated index: coarse bucket -> candidate entries, newest last.
    buckets: HashMap<u64, Vec<(u64, Entry)>>,
    order: VecDeque<u64>,
    bytes: u64,
    pub budget: u64,
    pub hits: u64,
    pub misses: u64,
    pub inserts: u64,
    /// Keys only, no pixels: entries are charged their real byte cost but
    /// store nothing. Lets `analyze` (which never upscales) measure the hit
    /// rate and the eviction behaviour a real run would have. A merge that
    /// is handed a placeholder fails its length check, so a dry cache can
    /// never leak into output.
    pub dry: bool,
}

impl BlockCache {
    pub fn new(budget: u64) -> Self {
        BlockCache { map: HashMap::new(), cost: HashMap::new(), buckets: HashMap::new(), order: VecDeque::new(), bytes: 0, budget, hits: 0, misses: 0, inserts: 0, dry: false }
    }

    /// A cache that records keys and charges `bytes` per entry but stores no
    /// pixels. See [`BlockCache::dry`].
    pub fn new_dry(budget: u64) -> Self {
        BlockCache { dry: true, ..BlockCache::new(budget) }
    }

    /// Insert a key that costs `bytes` without the pixels (dry only).
    pub fn insert_sized(&mut self, key: u64, bytes: u64) {
        debug_assert!(self.dry, "insert_sized on a real cache would serve empty blocks");
        self.insert_costed(key, Arc::new(Vec::new()), bytes);
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
        let n = block.len() as u64;
        self.insert_costed(key, block, n);
    }

    fn insert_costed(&mut self, key: u64, block: Arc<Vec<u8>>, bytes: u64) {
        if self.budget == 0 || self.map.contains_key(&key) {
            return;
        }
        let n = bytes + 32;
        while self.bytes + n > self.budget {
            let Some(old) = self.order.pop_front() else { break };
            if self.map.remove(&old).is_some() {
                self.bytes -= self.cost.remove(&old).unwrap_or(0);
            }
        }
        if n > self.budget {
            return;
        }
        self.bytes += n;
        self.order.push_back(key);
        self.cost.insert(key, n);
        self.map.insert(key, block);
        self.inserts += 1;
    }

    /// Add an entry to the gated index under `coarse`. Bounded per bucket so
    /// a probe's cost stays flat however skewed the content is; the newest
    /// candidates win, which matches how repeats arrive (bursty).
    pub fn insert_gated(&mut self, coarse: u64, exact: u64, e: Entry, max_per_bucket: usize) {
        if self.budget == 0 {
            return;
        }
        let v = self.buckets.entry(coarse).or_default();
        if v.iter().any(|(k, _)| *k == exact) {
            return;
        }
        v.push((exact, e));
        if v.len() > max_per_bucket {
            v.remove(0);
        }
        self.inserts += 1;
    }

    /// Best candidate in `coarse`'s bucket whose low-res block is within
    /// `limit` SSE of the block at `(bx, by)` of `next`, using the caller's
    /// comparison (so the gate is the pipeline's, not the cache's).
    pub fn get_gated(&mut self, coarse: u64, block: u32, mut sse: impl FnMut(&Frame) -> Option<u64>) -> Option<(u64, Entry)> {
        let Some(v) = self.buckets.get(&coarse) else {
            self.misses += 1;
            return None;
        };
        let mut best: Option<(u64, u64, Entry)> = None;
        for (k, e) in v.iter() {
            let Ok(f) = Frame::from_rgb(block, block, (*e.lo).clone()) else { continue };
            if let Some(s) = sse(&f) {
                if best.as_ref().map_or(true, |b| s < b.0) {
                    best = Some((s, *k, e.clone()));
                }
            }
        }
        match best {
            Some((_, k, e)) => {
                self.hits += 1;
                Some((k, e))
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    /// Candidates currently indexed for gated probes.
    pub fn gated_len(&self) -> usize {
        self.buckets.values().map(|v| v.len()).sum()
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

    /// A dry cache evicts exactly like a real one of the same budget, and
    /// its entries are empty.
    #[test]
    fn dry_cache_matches_real_eviction() {
        let (b, budget) = (100u64, 3 * (100 + 32));
        let mut real = BlockCache::new(budget);
        let mut dry = BlockCache::new_dry(budget);
        for k in 0..5u64 {
            real.insert(k, Arc::new(vec![7u8; b as usize]));
            dry.insert_sized(k, b);
        }
        assert_eq!((real.len(), real.bytes()), (dry.len(), dry.bytes()));
        for k in 0..5u64 {
            assert_eq!(real.get(k).is_some(), dry.get(k).is_some(), "key {k}");
        }
        assert_eq!((real.hits, real.misses), (dry.hits, dry.misses));
        assert!(dry.get(4).unwrap().is_empty());
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

// MEASURED, so the next person does not repeat it.
//
// Subtile harvesting (feeding the cache from every redraw and region redraw
// instead of only from packed residual cells) works exactly as designed: on
// Lone Ranger AV1, 120 frames, it took inserts from 14,567 to 54,390 — 3.7x
// the supply, at no extra network cost, and the harvested blocks have better
// context than residual ones.
//
// It bought nothing. Hits went 34 -> 41 out of ~14,600 probes (0.2%).
//
// Gated probing (coarse bucket + verify under the frame's own gate) was the
// obvious fix and also bought nothing: 36 hits at every quantiser from 8 to
// 48, with a 4x4 signature and again with 2x2. Three clips, same answer.
//
// The reason is the probe set, not the supply and not the matcher. A cell is
// only probed if it is still Missing after the stationary test, the motion
// search, the reference ring, the plate and the anchor. Those already catch
// everything that repeats *near* where it was; what survives to the cache is
// content that is genuinely new to this shot, and genuinely new content does
// not have a twin sitting in a cache of the last few seconds. The cases the
// cache was built for — letterbox, a static HUD, an unchanged background —
// never reach it, because they are Stationary, which is cheaper.
//
// What would change the answer, in the order worth trying:
//   1. Make the cache outlive the shot. A film cuts back to the same set
//      dozens of times; this cache holds ~4 seconds at 512 MB and 1080p and
//      the FIFO throws shot A away before shot A returns. A small on-disk
//      index keyed by a shot descriptor is the change that fits how film is
//      actually cut.
//   2. Probe with motion, not identity. `RecalledMoved` beat the cache by two
//      orders of magnitude (1238 blocks/frame vs 0.3) because it searches a
//      *position*. A cache that could be searched the same way — an ANN index
//      over block descriptors — is the difference between "the same pixels
//      appeared before" and "these pixels appeared before, somewhere".
//   3. Give up on live action and keep it for animation, where exact repeats
//      are the norm. That is what the cache was originally for.
