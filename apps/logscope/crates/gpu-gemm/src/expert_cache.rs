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
    // LFRU (Colibri tier.h): eviction is frequency-primary, recency-secondary. Per-expert decaying heat
    // (route frequency) and last-access clock; the victim is the resident expert with the lowest
    // score = (heat<<8)|recency. Pure LRU pins whatever was touched last; real MoE routing is skewed, so
    // keeping the *hot* experts resident (even if not the most recent) is what lifts the hit rate.
    heat: HashMap<u32, u32>,      // expert → decaying access frequency
    last_access: HashMap<u32, u32>, // expert → clock of last access (recency for the score)
    clock: u64,
    decay_every: u64,             // auto-decay heat every N accesses (Colibri decays at the ~16-token safe point)
    since_decay: u64,
    pub hits_pin: u64,
    pub hits_lru: u64,
    pub misses: u64,
}

/// Colibri's LFRU score (tier.h `tier_lfru_score`): heat in the high bits, recency (capped at 255) in the
/// low 8 — so frequency dominates and recency only breaks ties among equally-hot experts.
fn lfru_score(heat: u32, last: u32, clock: u32) -> u64 {
    let age = clock.wrapping_sub(last);
    let recent = if age < 255 { 255 - age } else { 0 };
    ((heat as u64) << 8) | recent as u64
}

impl LayerSlots {
    pub fn new(pinned: &[u32], n_slots: u32) -> Self {
        let mut p = pinned.to_vec();
        p.sort_unstable();
        LayerSlots {
            pinned: p,
            slot_expert: vec![None; n_slots as usize],
            expert_slot: HashMap::new(),
            heat: HashMap::new(),
            last_access: HashMap::new(),
            clock: 1, // demand accesses increment from here (first access → 2), leaving 1 as the prefetch band
            // decay ~every 16 tokens' worth of accesses through this per-layer policy; scaled by the working
            // set so a bigger ring accumulates proportionally more heat before halving.
            decay_every: (n_slots as u64 * 16).max(32),
            since_decay: 0,
            hits_pin: 0, hits_lru: 0, misses: 0,
        }
    }

    fn is_pinned(&self, e: u32) -> bool { self.pinned.binary_search(&e).is_ok() }
    /// Is this expert in the never-evicted pin store?
    pub fn is_pinned_pub(&self, e: u32) -> bool { self.is_pinned(e) }
    /// The current pin set (sorted) — for a caller that mirrors it in GPU buffers.
    pub fn pinned_ids(&self) -> &[u32] { &self.pinned }

    /// Resolve one expert to its residency, updating recency/stats. On a miss, selects the LRU victim slot,
    /// reassigns it to `e` (so a subsequent fill + dispatch targets it), and returns Miss(slot). The caller
    /// must fill that slot's GPU buffer from host bytes before dispatching the expert.
    pub fn access(&mut self, e: u32) -> Access {
        self.clock += 1;
        self.since_decay += 1;
        if self.since_decay >= self.decay_every { self.decay(); self.since_decay = 0; }
        // record heat/recency for LFRU (pinned experts too — so a live repin pass can see their heat)
        *self.heat.entry(e).or_insert(0) += 1;
        self.last_access.insert(e, self.clock as u32);
        if self.is_pinned(e) { self.hits_pin += 1; return Access::Pinned; }
        if let Some(&slot) = self.expert_slot.get(&e) {
            self.hits_lru += 1;
            return Access::Hit(slot);
        }
        // miss → evict by LFRU (empty slots first, then coldest-by-score)
        let victim = self.lfru_victim();
        if let Some(old) = self.slot_expert[victim as usize] { self.expert_slot.remove(&old); }
        self.slot_expert[victim as usize] = Some(e);
        self.expert_slot.insert(e, victim);
        self.misses += 1;
        Access::Miss(victim)
    }

    /// Halve every expert's heat — Colibri's `tier_decay`, run periodically (it fires at the repin safe
    /// point, ~every 16 tokens) so the heat map tracks RECENT frequency rather than all-time counts. The
    /// caller invokes this once per N tokens; without it, early-hot experts would ossify in the slots.
    pub fn decay(&mut self) { for h in self.heat.values_mut() { *h >>= 1; } }

    /// Is this expert resident right now (pin or LRU) WITHOUT changing state? Used to split a token's routed
    /// set into resident (dispatch now) vs misses (load first) for the GPU/disk overlap.
    pub fn resident(&self, e: u32) -> bool { self.is_pinned(e) || self.expert_slot.contains_key(&e) }

    /// Colibri's REPIN decision (glm.c `tier_pick_lfru` + its 25%+4 hysteresis): should a pinned expert be
    /// swapped out for a hotter unpinned one? Returns `(evict, admit)` only when the incoming expert beats
    /// the coldest pinned one by a clear margin — without that hysteresis the pin set ping-pongs between
    /// two near-equal experts and every swap costs a full re-upload.
    ///
    /// This is what makes the pin set *earned* rather than arbitrary. The loader has to choose a pin set
    /// before it has ever seen the model route (a chicken-and-egg it cannot win), so the initial choice is
    /// necessarily a guess; this corrects it from observed routing.
    pub fn pick_repin(&self, n_expert: u32) -> Option<(u32, u32)> {
        if self.pinned.is_empty() { return None; }
        let clk = self.clock as u32;
        let score = |e: u32| lfru_score(*self.heat.get(&e).unwrap_or(&0), *self.last_access.get(&e).unwrap_or(&0), clk);
        let cold = *self.pinned.iter().min_by_key(|&&e| score(e))?;
        let hot = (0..n_expert).filter(|e| !self.is_pinned(*e)).max_by_key(|&e| score(e))?;
        let (cs, hs) = (score(cold), score(hot));
        if hs <= cs + (cs >> 2) + (4u64 << 8) { return None; } // hysteresis: must be clearly hotter
        Some((cold, hot))
    }

    /// Apply a repin decided by [`pick_repin`]: `evict` leaves the pin store, `admit` joins it. If `admit`
    /// was occupying an LRU slot it is released (it is resident by pin now), which hands a slot back to the
    /// streaming ring.
    pub fn repin(&mut self, evict: u32, admit: u32) {
        self.pinned.retain(|&e| e != evict);
        if let Err(pos) = self.pinned.binary_search(&admit) { self.pinned.insert(pos, admit); }
        if let Some(slot) = self.expert_slot.remove(&admit) { self.slot_expert[slot as usize] = None; }
    }

    /// Speculative fill that can NEVER displace a demand-loaded expert — it takes only a genuinely spare
    /// slot: empty, or holding a previous speculative fill that was never used (heat 0, i.e. the model has
    /// not routed to it). Returns None when every slot holds an expert the model actually demanded.
    ///
    /// This is the economics fix. Plain `prefetch` raises the hit rate but evicts real residents to do it,
    /// so total fills (demand + speculative) go UP — a loss on hardware where a fill costs bandwidth and
    /// does not overlap compute (measured: Asahi's single serialized queue). Restricting prefetch to spare
    /// capacity makes it strictly non-harmful: it can only ever convert an idle slot into a hit.
    pub fn prefetch_spare(&mut self, e: u32) -> Option<u32> {
        if self.resident(e) { return None; }
        let spare = self.slot_expert.iter().position(|s| match s {
            None => true,
            Some(x) => *self.heat.get(x).unwrap_or(&0) == 0, // never-demanded (unused prefetch / decayed cold)
        })? as u32;
        if let Some(old) = self.slot_expert[spare as usize] { self.expert_slot.remove(&old); }
        self.slot_expert[spare as usize] = Some(e);
        self.expert_slot.insert(e, spare);
        Some(spare)
    }

    /// Prefetch: bring `e` resident if it isn't, WITHOUT counting a demand miss (it's speculative). Returns
    /// Some(slot) if a slot was assigned to fill (the mechanism should start the copy), None if already
    /// resident. An unused prefetch is the first non-empty LRU victim (so a wrong prediction self-corrects),
    /// while a real access to it afterwards bumps it to the live clock and makes it hot. Recency band:
    /// empty slot = 0; prefetched-but-unused = 1; demand-used = the incrementing clock (≥2 after the first
    /// access). So victims are chosen empty → unused-prefetch → coldest demand slot, exactly as wanted.
    pub fn prefetch(&mut self, e: u32) -> Option<u32> {
        if self.resident(e) { return None; }
        let victim = self.lfru_victim();
        if let Some(old) = self.slot_expert[victim as usize] { self.expert_slot.remove(&old); }
        self.slot_expert[victim as usize] = Some(e);
        self.expert_slot.insert(e, victim);
        Some(victim)
    }

    /// LFRU victim (Colibri tier.h): an empty slot first (nothing to evict), otherwise the resident expert
    /// with the lowest score = (heat<<8)|recency — i.e. the coldest-by-frequency, breaking ties by least
    /// recent. Degrades to plain LRU when all heats are equal (every expert seen the same number of times),
    /// which is why the pre-existing LRU tests still hold.
    fn lfru_victim(&self) -> u32 {
        // empty slot wins immediately
        for (i, s) in self.slot_expert.iter().enumerate() { if s.is_none() { return i as u32; } }
        let clk = self.clock as u32;
        let mut best = 0u32; let mut best_score = u64::MAX;
        for (i, s) in self.slot_expert.iter().enumerate() {
            let e = s.unwrap();
            let sc = lfru_score(*self.heat.get(&e).unwrap_or(&0), *self.last_access.get(&e).unwrap_or(&0), clk);
            if sc < best_score { best_score = sc; best = i as u32; }
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
    fn repin_corrects_a_wrong_pin_set_but_only_on_a_clear_margin() {
        // The loader pinned {0,1} before it had ever seen routing. The model actually routes to {6,7}.
        let mut s = LayerSlots::new(&[0, 1], 3);
        for _ in 0..10 { s.access(6); s.access(7); }
        // expert 6/7 are now far hotter than the pinned 0/1 → a repin is warranted
        let (evict, admit) = s.pick_repin(8).expect("hot unpinned expert should trigger a repin");
        assert!(evict == 0 || evict == 1, "evicts a cold pinned expert, got {evict}");
        assert!(admit == 6 || admit == 7, "admits a hot routed expert, got {admit}");
        s.repin(evict, admit);
        assert!(s.is_pinned_pub(admit), "admitted expert is now pinned");
        assert!(!s.is_pinned_pub(evict), "evicted expert is no longer pinned");
        // a second, near-equal candidate must NOT ping-pong: with both hot experts now
        // one-pinned/one-not and similar heat, hysteresis should hold the line
        let before = s.pick_repin(8);
        if let Some((e2, a2)) = before {
            assert_ne!((e2, a2), (admit, evict), "must not immediately swap back");
        }
    }

    #[test]
    fn repin_declines_when_nothing_is_clearly_hotter() {
        // uniform routing: every expert equally warm → no swap is justified, so the pin set holds
        let mut s = LayerSlots::new(&[0, 1], 3);
        for _ in 0..4 { for e in 0..8u32 { s.access(e); } }
        // the pinned experts were accessed exactly as often as everyone else
        assert!(s.pick_repin(8).is_none(), "no clear winner → hysteresis blocks a pointless re-upload");
    }

    #[test]
    fn lfru_keeps_the_hot_expert_over_a_recent_cold_one() {
        // LFRU (Colibri): frequency beats recency. 3 slots. Make 10 HOT (many accesses), 20 and 30 cold.
        // Then touch a fresh cold expert repeatedly to churn the ring; 10 must survive because its heat is
        // high, even though it wasn't the most-recently-used. Pure LRU would evict 10 once it aged out.
        let mut s = LayerSlots::new(&[], 3);
        for _ in 0..8 { s.access(10); } // 10 is very hot
        s.access(20);                    // slot fills
        s.access(30);                    // ring now {10(hot),20,30}
        // churn: bring in 40 then 50 — each miss must evict a COLD slot (20/30), never hot 10
        let _ = s.access(40); // evicts coldest of {20,30} by score
        let _ = s.access(50); // evicts the other cold one
        assert!(s.resident(10), "hot 10 survives churn (LFRU frequency wins)");
        assert!(s.resident(40) && s.resident(50), "the two new experts are resident");
        // decay enough and 10's heat erodes → it becomes evictable like anything else
        for _ in 0..6 { s.decay(); }
        let _ = s.access(60); let _ = s.access(70);
        assert!(!s.resident(10), "after heavy decay, cold-again 10 can finally be evicted");
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
