//! anchorpool.rs — the engine's warm-resume KV pool, tiered and value-ranked.
//!
//! The engine warm-resumes a turn by seeding a cached branch tip's KV and prefilling only the
//! divergent suffix (see `kvtree::pick_anchor`). The pool of tips used to be a flat `Vec` with
//! `remove(0)` FIFO eviction: the OLDEST entry was dropped regardless of how valuable it was, so a
//! frequently-reused compaction prefix or the shared ancestor of a fan-out got evicted the moment a
//! few siblings pushed in after it — then re-prefilled from scratch on the next hit.
//!
//! This pool replaces that with the two ideas the standalone `PrefixStore` already proved (and the
//! colibrì memory-hierarchy pattern): **value-per-byte ranking** and **demote-before-drop**. Each
//! entry is a branch tip — its token lineage plus the resume state (`pos`, `logits`, `rng`,
//! `prefill_len`) and the KV itself at one of three tiers:
//!
//!   Hot  — the f32 `Session`, seeded directly (bit-exact, fastest).
//!   Warm — int8 KV (`demote_cache`, ~4× smaller), promoted back to f32 on a hit. Same tolerance
//!          tier as the int8-resident decode path the kernel gate already validates.
//!   Cold — the int8 bytes evicted off the GPU allocator into host RAM (nearly free on UMA), re-
//!          uploaded on a hit. Lets a deep fan-out tree keep many tips without exhausting the VMA
//!          ceiling that a pool of full f32 caches would hit.
//!
//! When the pool exceeds its byte budget it demotes the lowest-value Hot entry to Warm, then Warm to
//! Cold, and only DROPS an entry (losing its KV, forcing a future re-prefill) when nothing else frees
//! enough. Value is `(hits+1)·len·fanout`: reuse count, prefix length, and how many distinct lineages
//! descend from the tip (the shared ancestor of a fan is worth keeping over an incidental leaf),
//! recency breaking ties. Selection stays `kvtree::pick_anchor{,_pinned}` on the token lineages — the
//! tier is an implementation detail the caller never sees; it always gets a seedable f32 `Session`.
//!
//! Invariant (colibrì's rule, applied to KV): the tier only ever decides speed/memory, never the
//! output. Hot is bit-exact; Warm/Cold ride the int8-KV tolerance the decode gate already checks.

use gpu_kernels::{Gpu, Model, QKvCache, Session};

/// One cached branch tip. `toks` is its full token lineage (prompt+generated), `prefill_len` the
/// leading rows that were batch-prefilled (the only region ever re-seeded — see the engine's reuse
/// path). `pos`/`logits`/`rng` restore the exact resume state a Hot `Session` carried.
struct Anchor {
    toks: Vec<u32>,
    prefill_len: usize,
    pos: usize,
    logits: Vec<f32>,
    rng: u64,
    hits: u32,
    last: u64,
    fanout: u32, // distinct child lineages seen descending from this tip (≥1)
    tier: Tier,
}

enum Tier {
    Hot(Session),                    // f32 KV, seed directly
    Warm(QKvCache),                  // int8 KV resident on the GPU, promote on hit
    Cold { kq: Vec<Vec<u8>>, ks: Vec<Vec<u8>>, vq: Vec<Vec<u8>>, vs: Vec<Vec<u8>> }, // int8 bytes in host RAM
}

pub struct AnchorPool {
    entries: Vec<Anchor>,
    budget_bytes: usize,
    clock: u64,
    n_layers: usize,
    kv_dim: usize,
    // knobs surfaced for logging / A-B
    pub demoted: u64,
    pub cooled: u64,
    pub dropped: u64,
}

impl AnchorPool {
    /// `budget_bytes` caps the pool's GPU-resident KV. `cap_entries` is a soft ceiling on how many
    /// tips to track at all (cold entries are cheap but not free — their host bytes still cost RAM).
    pub fn new(budget_bytes: usize, n_layers: usize, kv_dim: usize) -> Self {
        Self { entries: Vec::new(), budget_bytes, clock: 0, n_layers, kv_dim,
               demoted: 0, cooled: 0, dropped: 0 }
    }

    fn hot_bytes(&self, len: usize) -> usize { 2 * len * self.kv_dim * 4 * self.n_layers }
    // int8: kv_dim bytes/row + one f32 scale/row, K and V, per layer.
    fn q8_bytes(&self, len: usize) -> usize { 2 * self.n_layers * (len * self.kv_dim + len * 4) }
    fn gpu_bytes(&self, a: &Anchor) -> usize {
        match a.tier {
            Tier::Hot(_) => self.hot_bytes(a.toks.len().min(a.pos.max(1))),
            Tier::Warm(_) => self.q8_bytes(a.pos.max(1)),
            Tier::Cold { .. } => 0, // off the GPU allocator entirely
        }
    }
    /// GPU-resident KV bytes across the pool (what the budget bounds).
    pub fn usage(&self) -> usize { self.entries.iter().map(|a| self.gpu_bytes(a)).sum() }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    // Higher = keep. (hits+1)·len·fanout, recency breaks ties.
    fn score(&self, a: &Anchor) -> (i64, u64) {
        ((a.hits as i64 + 1) * a.pos.max(1) as i64 * a.fanout.max(1) as i64, a.last)
    }

    /// The lightweight view `kvtree::pick_anchor` needs: (tokens, prefill_len, pos) per entry.
    fn meta(&self) -> Vec<(Vec<u32>, usize, usize)> {
        self.entries.iter().map(|a| (a.toks.clone(), a.prefill_len, a.pos)).collect()
    }

    /// Select the entry sharing the longest usable prefix with `prompt` (same thresholds as the
    /// single-anchor path), promote it to a seedable f32 `Session`, and return it with the reuse
    /// length. The chosen entry is REMOVED from the pool (the caller owns it for the turn, then
    /// re-inserts its extended tip); a bump to hits/recency is recorded on the returned anchor's
    /// lineage via `pick`'s bookkeeping. `None` → nothing clears the thresholds → full prefill.
    pub fn pick(&mut self, gpu: &Gpu, model: &Model, prompt: &[u32], min_prefix: usize, max_suffix: usize)
        -> Option<(usize, usize, Session)> {
        let meta = self.meta();
        let (idx, l) = crate::kvtree::pick_anchor(&meta, prompt, min_prefix, max_suffix)?;
        // Record a hit + count this prompt as a (possibly new) descending lineage.
        self.clock += 1;
        let now = self.clock;
        {
            let a = &mut self.entries[idx];
            a.hits += 1; a.last = now; a.fanout = a.fanout.saturating_add(1);
        }
        // Materialize the chosen tier to a Hot Session for seeding.
        let a = self.entries.remove(idx);
        let sess = self.to_session(gpu, model, a);
        Some((idx, l, sess))
    }

    /// Turn any tier back into a seedable f32 `Session` (Warm/Cold promote through `promote_cache`).
    fn to_session(&self, gpu: &Gpu, model: &Model, a: Anchor) -> Session {
        match a.tier {
            Tier::Hot(s) => s,
            Tier::Warm(q) => self.session_from_q8(gpu, model, &q, a.pos, &a.logits, a.rng),
            Tier::Cold { kq, ks, vq, vs } => {
                let q = self.upload_cold(gpu, kq, ks, vq, vs);
                self.session_from_q8(gpu, model, &q, a.pos, &a.logits, a.rng)
            }
        }
    }

    fn session_from_q8(&self, gpu: &Gpu, _model: &Model, q: &QKvCache, pos: usize, logits: &[f32], rng: u64) -> Session {
        let cache = gpu.promote_cache(q, pos, self.kv_dim);
        Session { cache, pos, logits: logits.to_vec(), generated: Vec::new(), rng, done: false }
    }

    /// Insert (or refresh) a branch tip built this turn. `sess` carries its Hot KV + resume state.
    /// Enforces the byte budget afterwards (demote → cool → drop, lowest value first).
    pub fn insert(&mut self, gpu: &Gpu, toks: Vec<u32>, prefill_len: usize, sess: Session, cap_entries: usize) {
        self.clock += 1;
        let now = self.clock;
        let (pos, rng) = (sess.pos, sess.rng);
        let logits = sess.logits.clone();
        // A tip that extends an existing lineage refreshes it in place (keeps hits/fanout).
        if let Some(e) = self.entries.iter_mut().find(|e| e.toks == toks) {
            e.tier = Tier::Hot(sess); e.prefill_len = prefill_len;
            e.pos = pos; e.logits = logits; e.rng = rng; e.last = now;
        } else {
            self.entries.push(Anchor {
                toks, prefill_len, pos, logits, rng, hits: 0, last: now, fanout: 1, tier: Tier::Hot(sess),
            });
        }
        self.enforce(gpu, cap_entries);
    }

    /// Pop the MOST RECENT tip as a Hot session for `continue_last` (the auto-continue path resumes
    /// the last reply). Returns its (toks, session, prefill_len). `None` when the pool is empty.
    pub fn take_last(&mut self, gpu: &Gpu, model: &Model) -> Option<(Vec<u32>, Session, usize)> {
        let idx = self.entries.iter().enumerate().max_by_key(|(_, a)| a.last).map(|(i, _)| i)?;
        let a = self.entries.remove(idx);
        let (toks, pf) = (a.toks.clone(), a.prefill_len);
        let sess = self.to_session(gpu, model, a);
        Some((toks, sess, pf))
    }

    /// Bring GPU usage under budget and the entry count under `cap_entries`: demote lowest-value Hot
    /// → Warm, then Warm → Cold, and only DROP (lose the KV) when nothing else frees enough.
    pub fn enforce(&mut self, gpu: &Gpu, cap_entries: usize) {
        // 1) demote Hot → Warm (int8, still on GPU)
        while self.usage() > self.budget_bytes {
            let pick = self.entries.iter().enumerate()
                .filter(|(_, a)| matches!(a.tier, Tier::Hot(_)))
                .min_by_key(|(_, a)| self.score(a)).map(|(i, _)| i);
            match pick {
                Some(i) => {
                    let pos = self.entries[i].pos.max(1);
                    if let Tier::Hot(s) = &self.entries[i].tier {
                        let q = gpu.demote_cache(&s.cache, pos, self.kv_dim);
                        self.entries[i].tier = Tier::Warm(q); self.demoted += 1;
                    }
                }
                None => break,
            }
        }
        // 2) still over → cool Warm → Cold (int8 bytes to host RAM, off the GPU allocator)
        while self.usage() > self.budget_bytes {
            let pick = self.entries.iter().enumerate()
                .filter(|(_, a)| matches!(a.tier, Tier::Warm(_)))
                .min_by_key(|(_, a)| self.score(a)).map(|(i, _)| i);
            match pick {
                Some(i) => {
                    let pos = self.entries[i].pos.max(1);
                    if let Tier::Warm(q) = &self.entries[i].tier {
                        let cold = self.download_warm(gpu, q, pos);
                        self.entries[i].tier = cold; self.cooled += 1;
                    }
                }
                None => break,
            }
        }
        // 3) still over budget, or too many entries → drop the lowest-value entry outright
        while (self.usage() > self.budget_bytes || self.entries.len() > cap_entries.max(1))
            && !self.entries.is_empty()
        {
            let i = self.entries.iter().enumerate().min_by_key(|(_, a)| self.score(a)).map(|(i, _)| i).unwrap();
            self.entries.remove(i); self.dropped += 1;
        }
    }

    /// Warm (GPU int8) → Cold (host int8 bytes). Downloads the packed rows + scales per layer.
    /// `demote_cache` sizes each tensor to exactly `pos` rows, so a whole-tensor download is the
    /// entry's live KV with no slicing.
    fn download_warm(&self, gpu: &Gpu, q: &QKvCache, _pos: usize) -> Tier {
        let dl = |t: &gpu_kernels::Tensor| -> Vec<u8> {
            let words = gpu.download_u32(t);
            let mut b = Vec::with_capacity(words.len() * 4);
            for w in words { b.extend_from_slice(&w.to_le_bytes()); }
            b
        };
        let (kq, ks, vq, vs) = (
            q.kq.iter().map(|t| dl(t)).collect(),
            q.ks.iter().map(|t| dl(t)).collect(),
            q.vq.iter().map(|t| dl(t)).collect(),
            q.vs.iter().map(|t| dl(t)).collect(),
        );
        Tier::Cold { kq, ks, vq, vs }
    }

    /// Cold (host bytes) → Warm (GPU int8) for promotion on a hit.
    fn upload_cold(&self, gpu: &Gpu, kq: Vec<Vec<u8>>, ks: Vec<Vec<u8>>, vq: Vec<Vec<u8>>, vs: Vec<Vec<u8>>) -> QKvCache {
        let up = |b: &[u8]| -> gpu_kernels::Tensor {
            let words: Vec<u32> = b.chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            gpu.upload_u32(&words)
        };
        QKvCache {
            kq: kq.iter().map(|b| up(b)).collect(),
            ks: ks.iter().map(|b| up(b)).collect(),
            vq: vq.iter().map(|b| up(b)).collect(),
            vs: vs.iter().map(|b| up(b)).collect(),
        }
    }

    /// One-line tier census for the log (Hot/Warm/Cold counts + budget headroom).
    pub fn census(&self) -> String {
        let (mut h, mut w, mut c) = (0, 0, 0);
        for a in &self.entries {
            match a.tier { Tier::Hot(_) => h += 1, Tier::Warm(_) => w += 1, Tier::Cold { .. } => c += 1 }
        }
        format!("{h} hot / {w} warm / {c} cold · {:.1}/{:.1} MB · {} demoted {} cooled {} dropped",
            self.usage() as f32 / 1e6, self.budget_bytes as f32 / 1e6, self.demoted, self.cooled, self.dropped)
    }

    // ── ranking, exposed for unit tests (no GPU needed) ──────────────────────────────────────────
    #[cfg(test)]
    fn push_bare(&mut self, toks: Vec<u32>, pos: usize, hits: u32, fanout: u32) {
        self.clock += 1;
        self.entries.push(Anchor { toks, prefill_len: pos, pos, logits: vec![], rng: 1,
            hits, last: self.clock, fanout, tier: Tier::Cold { kq: vec![], ks: vec![], vq: vec![], vs: vec![] } });
    }
    #[cfg(test)]
    fn lowest_value_idx(&self) -> Option<usize> {
        self.entries.iter().enumerate().min_by_key(|(_, a)| self.score(a)).map(|(i, _)| i)
    }
    #[cfg(test)]
    fn keys(&self) -> Vec<Vec<u32>> { self.entries.iter().map(|a| a.toks.clone()).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> AnchorPool { AnchorPool::new(1 << 30, 2, 8) }

    #[test]
    fn value_ranks_by_hits_len_and_fanout() {
        let mut p = pool();
        p.push_bare(vec![1; 100], 100, 5, 1);   // long, hot
        p.push_bare(vec![2; 100], 100, 0, 1);   // long, cold  ← lowest value
        p.push_bare(vec![3; 100], 100, 0, 4);   // long, cold but shared ancestor (fanout 4)
        // the unused, unshared long entry is the eviction victim, not the hot or fanned one
        assert_eq!(p.lowest_value_idx(), Some(1));
    }

    #[test]
    fn fanout_beats_a_longer_but_incidental_leaf() {
        let mut p = pool();
        p.push_bare(vec![1; 200], 200, 0, 1);   // longer leaf, seen once, no descendants
        p.push_bare(vec![2; 120], 120, 1, 6);   // shorter, but a fan's shared ancestor
        // (hits+1)·len·fanout: leaf = 1·200·1 = 200; ancestor = 2·120·6 = 1440 → keep the ancestor
        assert_eq!(p.lowest_value_idx(), Some(0));
    }

    #[test]
    fn recency_breaks_ties() {
        let mut p = pool();
        p.push_bare(vec![1; 50], 50, 0, 1);
        p.push_bare(vec![2; 50], 50, 0, 1); // identical value, but newer → the older is the victim
        assert_eq!(p.lowest_value_idx(), Some(0));
    }

    #[test]
    fn census_counts_tiers() {
        let mut p = pool();
        p.push_bare(vec![1; 10], 10, 0, 1); // Cold in the test harness
        assert!(p.census().contains("cold"));
        assert_eq!(p.keys().len(), 1);
    }
}
