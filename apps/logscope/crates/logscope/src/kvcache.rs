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

use std::collections::{HashMap, VecDeque};

pub type Block = u32; // physical block id
pub type SeqId = u64;

/// Blocks required to hold `tokens` tokens.
pub fn blocks_for(tokens: usize, block_size: usize) -> usize {
    let bs = block_size.max(1);
    (tokens + bs - 1) / bs
}

/// Chained content hash of a FULL block: folds the previous block's hash with this block's token ids, so two
/// sequences that share a leading prefix produce identical per-block hashes (and diverge the moment their
/// tokens differ). Only full blocks are cacheable — a partial trailing block isn't hashed until it fills.
/// FNV-1a-style; collisions are astronomically unlikely and the engine still verifies token identity.
pub fn block_hash(prev: u64, tokens: &[u32]) -> u64 {
    let mut h = prev ^ 0x9E37_79B9_7F4A_7C15;
    for &t in tokens {
        h = (h ^ t as u64).wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

#[derive(Debug, PartialEq, Eq)]
pub struct Oom; // out of physical blocks

/// A paged KV-cache allocator over a fixed pool of physical blocks, with content-addressed PREFIX CACHING:
/// a full block computed for one sequence is retained after that sequence frees, keyed by its prefix hash,
/// so a later sequence with the same leading prefix reuses it instead of re-allocating and re-prefilling.
pub struct Paged {
    block_size: usize,
    free: Vec<Block>,
    refcount: Vec<u16>,
    tables: HashMap<SeqId, Vec<Block>>,
    lengths: HashMap<SeqId, usize>,
    // ── automatic prefix cache ──
    cached: HashMap<u64, Block>,      // prefix-hash → physical block holding that exact prefix content
    hash_of: HashMap<Block, u64>,     // reverse map, to drop a block from `cached` when it's reclaimed
    retained: VecDeque<Block>,        // cached blocks with refcount 0 — reclaimable (evicted by value)
    reuse: Vec<u32>,                  // per-block: times this cached block has been reused (LFU signal)
    depth: Vec<u32>,                  // per-block: prefix position when cached (0 = root; shallow = shared by more)
    last_tick: Vec<u64>,              // per-block: logical clock of last touch (LRU tiebreak)
    tick: u64,                        // monotonic logical clock
    retain_cap: usize,                // max blocks to keep retained; the tuner adjusts it from the hit rate
    win_lookups: u64,                 // lookups since the last tune (windowed, so tuning tracks CURRENT load)
    win_hits: u64,
    stats: PrefixStats,
}

/// Observability for the prefix cache — the signal the tuner (and you) read to know if it's paying off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrefixStats {
    pub lookups: u64,        // prompt blocks checked against the cache
    pub hits: u64,           // of those, found + reused
    pub tokens_saved: u64,   // prefill tokens skipped by reuse (hits × block_size)
    pub evictions: u64,      // cached blocks dropped under pressure
}

impl PrefixStats {
    /// Fraction of looked-up prefix blocks that hit (0 if nothing looked up yet).
    pub fn hit_rate(&self) -> f32 {
        if self.lookups == 0 { 0.0 } else { self.hits as f32 / self.lookups as f32 }
    }
}

impl Paged {
    pub fn new(num_blocks: usize, block_size: usize) -> Self {
        Self {
            block_size: block_size.max(1),
            free: (0..num_blocks as Block).rev().collect(), // pop() hands out 0,1,2,…
            refcount: vec![0; num_blocks],
            tables: HashMap::new(),
            lengths: HashMap::new(),
            cached: HashMap::new(),
            hash_of: HashMap::new(),
            retained: VecDeque::new(),
            reuse: vec![0; num_blocks],
            depth: vec![0; num_blocks],
            last_tick: vec![0; num_blocks],
            tick: 0,
            retain_cap: num_blocks, // start permissive; the tuner narrows it if the cache isn't paying off
            win_lookups: 0,
            win_hits: 0,
            stats: PrefixStats::default(),
        }
    }

    pub fn block_size(&self) -> usize { self.block_size }
    pub fn total_blocks(&self) -> usize { self.refcount.len() }
    pub fn free_blocks(&self) -> usize { self.free.len() }
    /// Blocks that could still back a new admission: the free pool PLUS the reclaimable cached blocks.
    pub fn reclaimable_blocks(&self) -> usize { self.free.len() + self.retained.len() }
    pub fn used_blocks(&self) -> usize { self.total_blocks() - self.free_blocks() }
    pub fn retained_blocks(&self) -> usize { self.retained.len() }
    pub fn block_table(&self, seq: SeqId) -> Option<&[Block]> { self.tables.get(&seq).map(|v| v.as_slice()) }
    pub fn seq_len(&self, seq: SeqId) -> Option<usize> { self.lengths.get(&seq).copied() }
    pub fn stats(&self) -> &PrefixStats { &self.stats }
    /// Could a fresh sequence of `n_tokens` be admitted right now? Counts reclaimable cached blocks, since
    /// [`alloc`](Self::alloc) will evict them under pressure.
    pub fn can_admit(&self, n_tokens: usize) -> bool { self.reclaimable_blocks() >= blocks_for(n_tokens, self.block_size) }

    /// Evict the LEAST-VALUABLE retained (cached, refcount-0) block and return it to use. Value = reuse count
    /// then recency (LFU with an LRU tiebreak): a system prompt reused many times survives; a one-off prefix
    /// goes first. This is the whole point — blind LRU would drop a hot prefix just because it wasn't the most
    /// recent, tanking the hit rate.
    fn reclaim_one(&mut self) -> Option<Block> {
        if self.retained.is_empty() { return None; }
        let mut best_i = 0usize;
        let mut best_key = (u32::MAX, u32::MAX, u64::MAX);
        for (i, &b) in self.retained.iter().enumerate() {
            // evict min: fewest reuses first, then DEEPEST prefix position (u32::MAX - depth: a root block
            // shared by every sequence sorts last), then least-recently used. Keeps the hot shared root.
            let key = (self.reuse[b as usize], u32::MAX - self.depth[b as usize], self.last_tick[b as usize]);
            if key < best_key { best_key = key; best_i = i; }
        }
        let b = self.retained.remove(best_i)?;
        if let Some(h) = self.hash_of.remove(&b) { self.cached.remove(&h); }
        self.stats.evictions += 1;
        Some(b)
    }

    fn alloc(&mut self) -> Option<Block> {
        let b = self.free.pop().or_else(|| self.reclaim_one())?;
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

    /// Release a sequence; blocks whose refcount hits zero return to the pool — but a CACHED block is kept
    /// *retained* (reclaimable, still hashed) so a later matching prefix can reuse it, rather than dropped.
    pub fn free(&mut self, seq: SeqId) {
        if let Some(table) = self.tables.remove(&seq) {
            self.lengths.remove(&seq);
            for &b in &table {
                let rc = &mut self.refcount[b as usize];
                *rc = rc.saturating_sub(1);
                if *rc == 0 {
                    if self.hash_of.contains_key(&b) {
                        self.retained.push_back(b); // keep it around for prefix reuse
                    } else {
                        self.free.push(b);
                    }
                }
            }
            self.enforce_retain_cap();
        }
    }

    /// Admit a sequence but REUSE cached blocks for any leading prefix already in the cache. `block_hashes`
    /// are the chained [`block_hash`]es of the prompt's FULL blocks, in order (compute with `block_hash`,
    /// folding each block's tokens). Leading blocks found in the cache are shared (no alloc, no prefill);
    /// the first miss ends the reuse run and the rest are allocated fresh and registered for future reuse.
    /// Returns the number of PREFILL TOKENS SAVED (cached prefix length) so the caller prefills only the tail.
    pub fn admit_cached(&mut self, seq: SeqId, block_hashes: &[u64], n_tokens: usize) -> Result<usize, Oom> {
        let bs = self.block_size;
        let total_blocks = blocks_for(n_tokens, bs);
        // how many leading full blocks hit the cache (consecutively)?
        let mut hit = 0usize;
        while hit < block_hashes.len() && hit < total_blocks {
            self.stats.lookups += 1;
            self.win_lookups += 1;
            if self.cached.contains_key(&block_hashes[hit]) { self.stats.hits += 1; self.win_hits += 1; hit += 1; } else { break; }
        }
        // the tail we must allocate ourselves
        let tail = total_blocks - hit;
        if self.reclaimable_blocks() < tail { return Err(Oom); }

        self.tick += 1;
        let mut table = Vec::with_capacity(total_blocks);
        // 1) share the cached prefix blocks (revive from retained if they were idle)
        for &h in &block_hashes[..hit] {
            let b = self.cached[&h];
            if self.refcount[b as usize] == 0 {
                if let Some(pos) = self.retained.iter().position(|&x| x == b) { self.retained.remove(pos); }
            }
            self.refcount[b as usize] += 1;
            self.reuse[b as usize] = self.reuse[b as usize].saturating_add(1);
            self.last_tick[b as usize] = self.tick;
            table.push(b);
        }
        self.stats.tokens_saved += (hit * bs) as u64;
        // 2) allocate the divergent tail, registering any block whose content hash we know (extends the chain)
        for i in hit..total_blocks {
            let b = self.alloc().ok_or(Oom)?;
            self.last_tick[b as usize] = self.tick;
            if let Some(&h) = block_hashes.get(i) {
                // this full block's content is known → cache it for future prefixes, recording its prefix
                // DEPTH (position i) so eviction can favour the shared root over deep, request-specific blocks
                self.cached.insert(h, b);
                self.hash_of.insert(b, h);
                self.depth[b as usize] = i as u32;
            }
            table.push(b);
        }
        self.tables.insert(seq, table);
        self.lengths.insert(seq, n_tokens);
        Ok(hit * bs)
    }

    /// Keep the retained (reclaimable-cached) pool within `retain_cap` by evicting least-valuable blocks —
    /// so a low hit-rate cache can't hoard the whole pool as idle-but-reserved.
    fn enforce_retain_cap(&mut self) {
        while self.retained.len() > self.retain_cap {
            let Some(b) = self.reclaim_one() else { break };
            self.free.push(b); // reclaimed but not needed right now → back to the free pool
        }
    }

    /// DYNAMIC TUNING: resize the retention budget from the observed hit rate. A cache that's paying off
    /// (high hit rate) earns more room to hold prefixes; one that isn't gives blocks back so admissions see a
    /// healthier free pool. Called periodically by the engine (e.g. every N admissions). `floor`/`ceil` bound
    /// it as fractions of the pool. Returns the new cap.
    pub fn tune_retention(&mut self, floor_frac: f32, ceil_frac: f32) -> usize {
        let total = self.total_blocks();
        let floor = ((total as f32 * floor_frac) as usize).min(total);
        let ceil = ((total as f32 * ceil_frac) as usize).min(total).max(floor);
        // WINDOWED hit rate — the rate since the last tune, so the budget tracks the CURRENT workload rather
        // than a sticky lifetime average (a chat that goes cold should release retention promptly).
        let hr = if self.win_lookups == 0 { self.stats.hit_rate() } else { self.win_hits as f32 / self.win_lookups as f32 };
        self.retain_cap = floor + ((ceil - floor) as f32 * hr) as usize;
        self.win_lookups = 0;
        self.win_hits = 0;
        self.enforce_retain_cap();
        self.retain_cap
    }

    /// Hit rate over the window since the last [`tune_retention`] — the live "is the cache paying off" gauge.
    pub fn recent_hit_rate(&self) -> f32 {
        if self.win_lookups == 0 { 0.0 } else { self.win_hits as f32 / self.win_lookups as f32 }
    }
}

/// A KV pool tenant key: `(model, device_id)` — identical to `deploy::Slot::key()`. Same model on two
/// devices is two tenants (different physical KV), so they never share blocks.
pub type Tenant = (String, u32);

/// Multi-tenant KV cache for a multi-model, multi-device deployment. Holds one paged [`Paged`] pool per
/// tenant, each sized to its device's KV budget and carrying its OWN prefix cache — so a system prompt cached
/// for the target model can't be mis-served to the draft model, and one device filling up can't evict
/// another's blocks. The engine routes every admit/append/free to the pool for the slot running that request.
#[derive(Default)]
pub struct CacheManager {
    pools: HashMap<Tenant, Paged>,
}

impl CacheManager {
    pub fn new() -> Self { Self::default() }

    /// Register a tenant's pool (one per enabled deployment slot). `num_blocks` is sized from that device's
    /// free KV budget ÷ per-block bytes for that model. Replaces any existing pool for the tenant.
    pub fn add_pool(&mut self, tenant: Tenant, num_blocks: usize, block_size: usize) {
        self.pools.insert(tenant, Paged::new(num_blocks, block_size));
    }

    pub fn pool(&self, tenant: &Tenant) -> Option<&Paged> { self.pools.get(tenant) }
    pub fn pool_mut(&mut self, tenant: &Tenant) -> Option<&mut Paged> { self.pools.get_mut(tenant) }
    pub fn remove_pool(&mut self, tenant: &Tenant) -> bool { self.pools.remove(tenant).is_some() }
    pub fn tenant_count(&self) -> usize { self.pools.len() }

    pub fn total_used_blocks(&self) -> usize { self.pools.values().map(|p| p.used_blocks()).sum() }
    pub fn total_free_blocks(&self) -> usize { self.pools.values().map(|p| p.free_blocks()).sum() }

    /// Prefix-cache stats summed across every tenant — the deployment-wide "is caching paying off" gauge.
    pub fn aggregate_stats(&self) -> PrefixStats {
        let mut s = PrefixStats::default();
        for p in self.pools.values() {
            let st = p.stats();
            s.lookups += st.lookups;
            s.hits += st.hits;
            s.tokens_saved += st.tokens_saved;
            s.evictions += st.evictions;
        }
        s
    }

    /// Retune retention on every tenant (call periodically); each adapts to its own hit rate independently.
    pub fn tune_all(&mut self, floor_frac: f32, ceil_frac: f32) {
        for p in self.pools.values_mut() { p.tune_retention(floor_frac, ceil_frac); }
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

    // chained prefix hashes for a list of full token-blocks
    fn hashes(blocks: &[&[u32]]) -> Vec<u64> {
        let mut h = 0u64;
        blocks.iter().map(|b| { h = block_hash(h, b); h }).collect()
    }

    #[test]
    fn shared_prefix_reuses_blocks_and_skips_prefill() {
        let mut p = Paged::new(16, 4);
        let sys = hashes(&[&[1, 2, 3, 4], &[5, 6, 7, 8]]); // a 2-block shared system prompt
        // first request: cache is cold, nothing saved, but it registers the prefix blocks
        let saved_a = p.admit_cached(1, &sys, 12).unwrap(); // 12 tokens = 3 blocks
        assert_eq!(saved_a, 0);
        let prefix_blocks: Vec<_> = p.block_table(1).unwrap()[..2].to_vec();
        p.free(1); // done — its cached prefix blocks are retained, not dropped
        assert_eq!(p.retained_blocks(), 2);

        // second request with the SAME prefix: reuses both cached blocks
        let free_before = p.free_blocks();
        let saved_b = p.admit_cached(2, &sys, 12).unwrap();
        assert_eq!(saved_b, 8, "2 cached blocks × 4 tokens of prefill skipped");
        assert_eq!(&p.block_table(2).unwrap()[..2], &prefix_blocks[..], "B shares A's physical prefix blocks");
        // memory: B only allocated its 1-block tail, not 3
        assert_eq!(p.free_blocks(), free_before - 1);
        assert_eq!(p.stats().tokens_saved, 8);
        assert!(p.stats().hit_rate() > 0.0);
    }

    #[test]
    fn eviction_keeps_the_hot_prefix_over_one_offs() {
        // small pool so we force reclaim; one prefix gets reused (hot), others are one-offs (cold)
        let mut p = Paged::new(4, 4);
        let hot = hashes(&[&[9, 9, 9, 9]]);
        // build reuse on `hot`: admit+free it a few times so its reuse count climbs
        for s in 0..3 {
            p.admit_cached(s, &hot, 4).unwrap();
            p.free(s);
        }
        let hot_block = p.cached[&hot[0]];
        assert!(p.reuse[hot_block as usize] >= 2, "hot block accrued reuse");
        // now churn several DISTINCT cold one-off prefixes through the tiny pool, forcing reclaims
        for s in 10..20u64 {
            let cold = hashes(&[&[s as u32, 1, 2, 3]]);
            p.admit_cached(s, &cold, 4).unwrap();
            p.free(s);
        }
        // value-aware eviction should have preserved the hot prefix and dropped cold ones
        assert!(p.cached.contains_key(&hot[0]), "the frequently-reused prefix survived eviction");
        assert!(p.stats().evictions > 0, "cold one-offs were evicted under pressure");
    }

    #[test]
    fn tune_retention_tracks_hit_rate() {
        let mut p = Paged::new(100, 4);
        // simulate a HIGH hit rate
        p.stats.lookups = 100;
        p.stats.hits = 90;
        let cap_hi = p.tune_retention(0.1, 0.8);
        // simulate a LOW hit rate
        p.stats.hits = 5;
        let cap_lo = p.tune_retention(0.1, 0.8);
        assert!(cap_hi > cap_lo, "a paying-off cache earns more retention room ({cap_hi} vs {cap_lo})");
        assert!(cap_lo >= 10, "floor respected");   // 0.1 × 100
        assert!(cap_hi <= 80, "ceil respected");    // 0.8 × 100
    }

    #[test]
    fn eviction_favours_the_shared_root_over_deep_blocks() {
        // a 3-block prompt: block 0 is the shared system-prompt ROOT, blocks 1-2 are request-specific.
        let mut p = Paged::new(3, 4);
        let h = hashes(&[&[1, 1, 1, 1], &[2, 2, 2, 2], &[3, 3, 3, 3]]);
        p.admit_cached(1, &h, 12).unwrap(); // caches all three, depths 0,1,2
        let root = p.cached[&h[0]];
        p.free(1); // all three retained, equal reuse (0)
        assert_eq!(p.retained_blocks(), 3);
        // force TWO reclaims by admitting a fresh 2-block prompt with no cache hits
        let cold = hashes(&[&[9, 9, 9, 9], &[8, 8, 8, 8]]);
        p.admit_cached(2, &cold, 8).unwrap();
        // with equal reuse, the DEEP blocks (depth 2 then 1) are evicted first; the root survives
        assert!(p.cached.contains_key(&h[0]), "the shared root (depth 0) is evicted last");
        assert!(!p.cached.contains_key(&h[2]), "the deepest, least-shared block went first");
        assert_eq!(p.reuse[root as usize], 0);
    }

    #[test]
    fn cache_manager_isolates_tenants() {
        let mut mgr = CacheManager::new();
        // target model on device 0 (big pool), draft model on device 1 (small pool)
        mgr.add_pool(("target".into(), 0), 16, 4);
        mgr.add_pool(("draft".into(), 1), 4, 4);
        assert_eq!(mgr.tenant_count(), 2);

        // admit into the target pool; the draft pool is untouched
        mgr.pool_mut(&("target".into(), 0)).unwrap().admit(1, 24).unwrap(); // 6 blocks
        assert_eq!(mgr.pool(&("target".into(), 0)).unwrap().used_blocks(), 6);
        assert_eq!(mgr.pool(&("draft".into(), 1)).unwrap().used_blocks(), 0, "draft pool isolated");
        assert_eq!(mgr.total_used_blocks(), 6);

        // a prefix cached for the target is invisible to the draft (different physical KV)
        let sys = hashes(&[&[1, 2, 3, 4]]);
        mgr.pool_mut(&("target".into(), 0)).unwrap().admit_cached(2, &sys, 4).unwrap();
        mgr.pool_mut(&("target".into(), 0)).unwrap().free(2);
        let saved_draft = mgr.pool_mut(&("draft".into(), 1)).unwrap().admit_cached(9, &sys, 4).unwrap();
        assert_eq!(saved_draft, 0, "draft can't reuse the target's cached blocks");

        // one device filling up can't starve another tenant
        assert!(mgr.pool_mut(&("draft".into(), 1)).unwrap().admit(5, 100).is_err(), "draft pool OOMs on its own budget");
        assert!(mgr.pool(&("target".into(), 0)).unwrap().free_blocks() > 0, "target pool unaffected");
    }

    #[test]
    fn cache_manager_aggregates_stats() {
        let mut mgr = CacheManager::new();
        mgr.add_pool(("a".into(), 0), 8, 4);
        mgr.add_pool(("b".into(), 0), 8, 4);
        let sys = hashes(&[&[7, 7, 7, 7]]);
        for t in [&("a".to_string(), 0u32), &("b".to_string(), 0u32)] {
            let p = mgr.pool_mut(t).unwrap();
            p.admit_cached(1, &sys, 4).unwrap(); // populate
            p.free(1);
            p.admit_cached(2, &sys, 4).unwrap(); // reuse → a hit + tokens saved
        }
        let agg = mgr.aggregate_stats();
        assert_eq!(agg.hits, 2, "one hit in each tenant");
        assert_eq!(agg.tokens_saved, 8, "4 tokens saved per tenant");
    }
}
