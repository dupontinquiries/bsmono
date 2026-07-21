//! expert_cache.rs — the POLICY core of MoE expert streaming: which expert lives in which resident slot,
//! what to evict, what to prefetch. Pure + fully testable (no GPU, no I/O). The GPU buffer copy and the
//! host/SSD read are a separate MECHANISM layer that needs hardware; this decides WHAT to copy, not HOW.
//!
//! Structure mirrors Colibri's tiered cache (validated on GLM): a per-layer set of resident expert slots,
//! split into a PIN store (the hottest experts, never evicted — from plan_moe_streaming.pinned_experts) and
//! an LRU ring (rotating slots for streamed cold experts). An access resolves to one of:
//!   * Pinned      — resident in the hot store, submit immediately (no load).
//!   * Hit(slot)   — resident in an LRU slot, submit immediately; its recency is bumped.
//!   * Miss(slot)  — not resident; the returned slot (LRU victim) must be FILLED from host bytes first.
//! The GPU/disk OVERLAP that makes streaming pay: `resolve_token` splits a token's routed set into the
//! already-resident experts (dispatch now) and the misses (fill their slots, then dispatch) — so the
//! resident matmuls run while the misses' bytes are still arriving. The predictor (expert_predict) feeds
//! `prefetch` so a miss is often already in flight before the router even asks for it.

use std::collections::HashMap;

/// How an expert access resolved against the resident set for one layer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Access {
    /// In the pin store — always resident, never streamed.
    Pinned,
    /// In an LRU slot — resident; `slot` is its index (recency bumped).
    Hit(u32),
    /// Not resident; `slot` is the LRU victim to fill from host bytes before dispatch.
    Miss(u32),
}

impl Access {
    pub fn is_resident(&self) -> bool { matches!(self, Access::Pinned | Access::Hit(_)) }
}

/// Per-layer residency: a pin store (fixed hot set) + an LRU ring of `n_slots` streamed-expert slots. One
/// of these per MoE layer (experts route independently per layer). Cheap: a slot→expert map + expert→slot
/// index + a recency clock.
pub struct LayerSlots {
    pinned: Vec<u32>,             // expert ids kept permanently resident (sorted, small)
    slot_expert: Vec<Option<u32>>, // slot index → expert currently in it (None = empty)
    expert_slot: HashMap<u32, u32>, // expert → slot (LRU tier only)
    last_used: Vec<u64>,          // slot → recency clock (for LRU victim selection)
    clock: u64,
    pub hits_pin: u64,
    pub hits_lru: u64,
    pub misses: u64,
}

impl LayerSlots {
    pub fn new(pinned: &[u32], n_slots: u32) -> Self {
        let mut p = pinned.to_vec();
        p.sort_unstable();
        LayerSlots {
            pinned: p,
            slot_expert: vec![None; n_slots as usize],
            expert_slot: HashMap::new(),
            last_used: vec![0; n_slots as usize],
            clock: 1, // demand accesses increment from here (first access → 2), leaving 1 as the prefetch band
            hits_pin: 0, hits_lru: 0, misses: 0,
        }
    }

    fn is_pinned(&self, e: u32) -> bool { self.pinned.binary_search(&e).is_ok() }

    /// Resolve one expert to its residency, updating recency/stats. On a miss, selects the LRU victim slot,
    /// reassigns it to `e` (so a subsequent fill + dispatch targets it), and returns Miss(slot). The caller
    /// must fill that slot's GPU buffer from host bytes before dispatching the expert.
    pub fn access(&mut self, e: u32) -> Access {
        self.clock += 1;
        if self.is_pinned(e) { self.hits_pin += 1; return Access::Pinned; }
        if let Some(&slot) = self.expert_slot.get(&e) {
            self.last_used[slot as usize] = self.clock;
            self.hits_lru += 1;
            return Access::Hit(slot);
        }
        // miss → evict the LRU slot (empty slots are victims first: last_used 0)
        let victim = self.lru_victim();
        if let Some(old) = self.slot_expert[victim as usize] { self.expert_slot.remove(&old); }
        self.slot_expert[victim as usize] = Some(e);
        self.expert_slot.insert(e, victim);
        self.last_used[victim as usize] = self.clock;
        self.misses += 1;
        Access::Miss(victim)
    }

    /// Is this expert resident right now (pin or LRU) WITHOUT changing state? Used to split a token's routed
    /// set into resident (dispatch now) vs misses (load first) for the GPU/disk overlap.
    pub fn resident(&self, e: u32) -> bool { self.is_pinned(e) || self.expert_slot.contains_key(&e) }

    /// Prefetch: bring `e` resident if it isn't, WITHOUT counting a demand miss (it's speculative). Returns
    /// Some(slot) if a slot was assigned to fill (the mechanism should start the copy), None if already
    /// resident. An unused prefetch is the first non-empty LRU victim (so a wrong prediction self-corrects),
    /// while a real access to it afterwards bumps it to the live clock and makes it hot. Recency band:
    /// empty slot = 0; prefetched-but-unused = 1; demand-used = the incrementing clock (≥2 after the first
    /// access). So victims are chosen empty → unused-prefetch → coldest demand slot, exactly as wanted.
    pub fn prefetch(&mut self, e: u32) -> Option<u32> {
        if self.resident(e) { return None; }
        let victim = self.lru_victim();
        if let Some(old) = self.slot_expert[victim as usize] { self.expert_slot.remove(&old); }
        self.slot_expert[victim as usize] = Some(e);
        self.expert_slot.insert(e, victim);
        self.last_used[victim as usize] = 1; // low band: above empty, below any demand access
        Some(victim)
    }

    fn lru_victim(&self) -> u32 {
        // the slot with the smallest last_used (empty slots have 0 → chosen first)
        let mut best = 0u32; let mut best_t = u64::MAX;
        for (i, &t) in self.last_used.iter().enumerate() {
            if t < best_t { best_t = t; best = i as u32; }
        }
        best
    }

    pub fn n_slots(&self) -> u32 { self.slot_expert.len() as u32 }
    pub fn n_pinned(&self) -> u32 { self.pinned.len() as u32 }
}

/// A token's routed experts split for the GPU/disk overlap: the ones already resident (dispatch their
/// matmuls immediately) and the ones that missed (fill their slots from host bytes, then dispatch). The
/// resident matmuls run while the misses' bytes are still arriving — the whole point of streaming.
pub struct TokenPlan {
    pub resident: Vec<(u32, Access)>, // (expert, Pinned|Hit) — submit now
    pub misses: Vec<(u32, u32)>,      // (expert, slot to fill) — load then submit
}

impl LayerSlots {
    /// Resolve a whole token's routed set at once, returning the overlap split. Resident experts keep their
    /// slots; misses reserve LRU victims. Order preserved for deterministic dispatch.
    pub fn resolve_token(&mut self, routed: &[u32]) -> TokenPlan {
        let mut resident = Vec::new();
        let mut misses = Vec::new();
        for &e in routed {
            match self.access(e) {
                a @ (Access::Pinned | Access::Hit(_)) => resident.push((e, a)),
                Access::Miss(slot) => misses.push((e, slot)),
            }
        }
        TokenPlan { resident, misses }
    }

    /// Hit rate so far (pin + LRU hits over all accesses) — the number that decides whether streaming keeps
    /// up. Colibri's GLM ran ~70% one-ahead; the predictor + pooling aim to push the *effective* rate higher.
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits_pin + self.hits_lru + self.misses;
        if total == 0 { return 1.0; }
        (self.hits_pin + self.hits_lru) as f64 / total as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_experts_always_resident() {
        let mut s = LayerSlots::new(&[0, 1, 2], 2);
        assert_eq!(s.access(1), Access::Pinned);
        assert_eq!(s.access(2), Access::Pinned);
        assert!(s.resident(0) && s.resident(1) && s.resident(2));
        // pinned never consume LRU slots
        assert_eq!(s.misses, 0);
    }

    #[test]
    fn miss_then_hit() {
        let mut s = LayerSlots::new(&[], 2);
        let a = s.access(5);
        assert!(matches!(a, Access::Miss(_)));
        // second access to 5 is a hit in the same slot
        match (a, s.access(5)) {
            (Access::Miss(m), Access::Hit(h)) => assert_eq!(m, h),
            other => panic!("expected miss then hit in same slot: {other:?}"),
        }
        assert_eq!(s.hits_lru, 1);
        assert_eq!(s.misses, 1);
    }

    #[test]
    fn lru_evicts_the_least_recently_used() {
        let mut s = LayerSlots::new(&[], 2);
        s.access(10); // slot A
        s.access(20); // slot B
        s.access(10); // touch 10 → 20 is now LRU
        let a = s.access(30); // miss → must evict 20
        assert!(matches!(a, Access::Miss(_)));
        assert!(s.resident(10), "recently-used 10 kept");
        assert!(!s.resident(20), "LRU 20 evicted");
        assert!(s.resident(30), "new 30 resident");
    }

    #[test]
    fn resolve_token_splits_resident_from_misses() {
        // pin {0}, 2 slots. token routes {0, 7, 8}: 0 pinned, 7+8 miss (fill 2 slots)
        let mut s = LayerSlots::new(&[0], 2);
        let plan = s.resolve_token(&[0, 7, 8]);
        assert_eq!(plan.resident.len(), 1); // expert 0 (pinned) dispatches now
        assert_eq!(plan.misses.len(), 2);   // 7, 8 fill slots then dispatch
        // next token routes {0, 7}: now 0 pinned + 7 resident → both dispatch now, no miss
        let plan2 = s.resolve_token(&[0, 7]);
        assert_eq!(plan2.resident.len(), 2);
        assert_eq!(plan2.misses.len(), 0);
    }

    #[test]
    fn prefetch_makes_a_future_access_a_hit() {
        let mut s = LayerSlots::new(&[], 3);
        assert!(s.prefetch(42).is_some(), "42 not resident → prefetch assigns a slot");
        assert!(s.prefetch(42).is_none(), "already resident → no-op");
        // a demand access to the prefetched expert is now a HIT, not a miss
        assert!(matches!(s.access(42), Access::Hit(_)));
        assert_eq!(s.misses, 0, "prefetch turned the would-be miss into a hit");
    }

    #[test]
    fn unused_prefetch_is_evicted_before_used_slots() {
        // 2 slots. Demand-load 1 (hot), prefetch 2 (speculative, unused). A new miss should evict the
        // unused prefetch (2), not the demanded expert (1).
        let mut s = LayerSlots::new(&[], 2);
        s.access(1);          // demand → hot
        s.prefetch(2);        // speculative, lower recency
        let a = s.access(3);  // miss → evict the unused prefetch 2
        assert!(matches!(a, Access::Miss(_)));
        assert!(s.resident(1), "demanded expert kept");
        assert!(!s.resident(2), "unused prefetch evicted first (wrong prediction self-corrects)");
    }

    #[test]
    fn hit_rate_tracks_streaming_health() {
        let mut s = LayerSlots::new(&[0], 4);
        for _ in 0..10 { s.access(0); }   // all pin hits
        s.access(99);                      // one miss
        assert!(s.hit_rate() > 0.9, "mostly pinned → high hit rate: {}", s.hit_rate());
    }

    #[test]
    fn predictor_driven_prefetch_lifts_hit_rate_end_to_end() {
        // The whole streaming loop in miniature: a token-correlated routing stream, an expert map that
        // learns it, and a slot cache that prefetches the map's predictions before each demand access.
        // Prefetching should convert would-be misses into hits vs a cold cache with no prefetch.
        use crate::expert_predict::{ExpertMap, CtxSig};
        // 8 experts, 3 stream slots (tight — forces eviction), no pins. Token t routes to expert t%8 and
        // (t*3+1)%8 at layer 0 — deterministic, learnable.
        let route = |t: u32| -> Vec<u32> { vec![t % 8, (t * 3 + 1) % 8] };
        let stream: Vec<u32> = (0..200u32).map(|i| (i * 7 + 3) % 12).collect(); // recurring 12-token vocab

        // train the predictor on the first half
        let mut map = ExpertMap::new();
        for &t in &stream[..100] {
            for &e in &route(t) { map.observe(CtxSig::unigram(t), 0, &[e], &[]); }
        }

        // eval WITHOUT prefetch (demand only) vs WITH predictor prefetch, on the second half
        let mut cold = LayerSlots::new(&[], 3);
        let mut warm = LayerSlots::new(&[], 3);
        for &t in &stream[100..] {
            // warm: prefetch the predicted experts for this token before the demand accesses
            for e in map.predict(CtxSig::unigram(t), 0, &[], 3) { warm.prefetch(e); }
            for &e in &route(t) { cold.access(e); warm.access(e); }
        }
        assert!(warm.hit_rate() > cold.hit_rate(),
            "predictor prefetch lifts hit rate: cold {:.2} → warm {:.2}", cold.hit_rate(), warm.hit_rate());
    }
}
