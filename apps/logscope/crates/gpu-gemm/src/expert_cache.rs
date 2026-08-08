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

    /// As [`pick_repin`], but a set of PREDICTED-hot experts gets a score bonus.
    ///
    /// The coupling predictor (72% accurate) cannot be used to speculate into ring slots on a
    /// capacity-bound model — measured: with ring == working set there is no spare slot, and displacing to
    /// make one costs more fills than it saves. But it can steer WHICH experts are pinned, which is free:
    /// the pin set has to hold something, and pinning what is about to be hot beats pinning what was hot.
    ///
    /// The bonus is deliberately modest — one notch of heat — so a confident observation still outranks a
    /// prediction. A predictor that could evict a genuinely hot expert on a wrong guess would be worse
    /// than not using it.
    pub fn pick_repin_hinted(&self, n_expert: u32, predicted: &[u32]) -> Option<(u32, u32)> {
        if self.pinned.is_empty() { return None; }
        let clk = self.clock as u32;
        let base = |e: u32| lfru_score(*self.heat.get(&e).unwrap_or(&0), *self.last_access.get(&e).unwrap_or(&0), clk);
        let score = |e: u32| base(e) + if predicted.contains(&e) { 1u64 << 8 } else { 0 };
        let cold = *self.pinned.iter().min_by_key(|&&e| score(e))?;
        let hot = (0..n_expert).filter(|e| !self.is_pinned(*e)).max_by_key(|&e| score(e))?;
        let (cs, hs) = (score(cold), score(hot));
        if hs <= cs + (cs >> 2) + (4u64 << 8) { return None; }
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
    fn a_prediction_can_break_a_tie_but_not_outrank_real_heat() {
        // Two unpinned candidates, 6 and 7, equally warm. The predictor favours 7 → 7 should be admitted.
        let mut s = LayerSlots::new(&[0, 1], 3);
        for _ in 0..10 { s.access(6); s.access(7); }
        let (_, admit) = s.pick_repin_hinted(8, &[7]).expect("a repin is warranted");
        assert_eq!(admit, 7, "the predicted expert wins between equals");

        // But a prediction must NOT displace a genuinely hotter expert: 6 is far hotter, 5 is merely
        // predicted, so 6 still wins.
        let mut s2 = LayerSlots::new(&[0, 1], 3);
        for _ in 0..30 { s2.access(6); }
        s2.access(5);
        let (_, admit2) = s2.pick_repin_hinted(8, &[5]).expect("a repin is warranted");
        assert_eq!(admit2, 6, "observed heat outranks a guess");
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

/// Running tally of what the expert cache actually delivered.
///
/// Scout streams 6.33 GiB of expert weights per token and runs at 0.92 tok/s against a 3.3 tok/s
/// upload-bound ceiling — a 3.6× gap that is prefetch, overlap and slot scheduling rather than kernel
/// throughput. **And none of it is visible.** No log line reports whether an expert was already
/// resident when it was needed, so the one number that would say whether the gap is schedulable is
/// the one nobody has.
///
/// The bind-group cache reports 26% hits on Scout against ~100% on a resident model, which says the
/// streaming path churns its bindings — but that is a proxy for the thing, not the thing.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct AccessTally {
    pub pinned: u64,
    /// Present in a ring slot when asked for — the prefetcher won.
    pub resident: u64,
    /// Not present: a stall while the expert uploads.
    pub missed: u64,
}

impl AccessTally {
    pub fn record(&mut self, a: Access) {
        match a {
            Access::Pinned => self.pinned += 1,
            Access::Hit(_) => self.resident += 1,
            _ => self.missed += 1,
        }
    }

    pub fn total(&self) -> u64 { self.pinned + self.resident + self.missed }

    /// Fraction of accesses that did NOT stall, 0..1.
    ///
    /// Pinned and resident both count: from the token's point of view the difference is where the
    /// bytes came from, not whether it waited.
    pub fn hit_rate(&self) -> f64 {
        if self.total() == 0 { return 0.0; }
        (self.pinned + self.resident) as f64 / self.total() as f64
    }

    /// A line worth printing, or None when nothing streamed.
    ///
    /// Silent on a fully resident model, because a hit rate of 1.0 that is structurally guaranteed is
    /// not information — it is a line that trains people to skip the region it prints in.
    pub fn report(&self, gib_per_token: f64, upload_gib_s: f64) -> Option<String> {
        if self.missed == 0 { return None; }
        let stall_gib = gib_per_token * (self.missed as f64 / self.total().max(1) as f64);
        Some(format!(
            "expert cache — {} pinned, {} resident, {} MISSED ({:.0}% hit). At {:.1} GiB/token and \
{upload_gib_s:.0} GiB/s the misses alone cost ~{:.0} ms/token; a perfect prefetcher would remove all \
of it and nothing else here can",
            self.pinned, self.resident, self.missed, self.hit_rate() * 100.0,
            gib_per_token, stall_gib / upload_gib_s * 1000.0))
    }
}

#[cfg(test)]
mod tally_tests {
    use super::*;

    #[test]
    fn pinned_and_resident_both_count_as_HITS() {
        // From the token's point of view the difference is where the bytes came from, not whether it
        // waited — and waiting is what costs the 3.6x.
        let mut t = AccessTally::default();
        t.record(Access::Pinned);
        t.record(Access::Hit(0));
        assert_eq!(t.hit_rate(), 1.0);
    }

    #[test]
    fn a_fully_resident_model_reports_NOTHING() {
        // A hit rate of 1.0 that is structurally guaranteed is not information. Printing it every run
        // trains people to skip the region it appears in, which is where the interesting line will be.
        let mut t = AccessTally::default();
        for _ in 0..48 { t.record(Access::Pinned); }
        assert!(t.report(6.33, 21.0).is_none());
    }

    #[test]
    fn the_report_names_the_COST_of_the_misses_not_just_the_rate() {
        // "19% hit" is a number. "the misses cost ~250 ms/token" is a decision — it says whether
        // prefetch work is worth doing before anyone does it.
        let mut t = AccessTally::default();
        t.record(Access::Pinned);
        for _ in 0..15 { t.record(Access::Miss(0)); }
        let r = t.report(6.33, 21.0).expect("something missed");
        assert!(r.contains("MISSED"));
        assert!(r.contains("ms/token"), "the cost, not just the rate: {r}");
    }

    #[test]
    fn an_empty_tally_does_not_divide_by_zero() {
        let t = AccessTally::default();
        assert_eq!(t.hit_rate(), 0.0);
        assert!(t.report(6.33, 21.0).is_none());
    }
}

/// Can this layer take the GPU-routed path even though the model streams?
///
/// # The condition is not what the guard assumes
///
/// `moe_ffn_gpu_routed` is gated on `m.stream.is_none()`, and the reason is real: the routed path
/// selects experts on the device, so the CPU never learns which ones — and a streaming model needs
/// the CPU to know, in order to fetch them.
///
/// **But that is a property of the LAYER, not the MODEL.** If every expert for this layer is already
/// pinned, any selection the device makes is a cache hit, so there is nothing to fetch and nothing
/// for the CPU to learn. The layer can route on-device while the model as a whole still streams.
///
/// # Why this matters more than it looks
///
/// Scout's decode is 99.9% CPU-wait from one blocking readback per layer. Every layer that qualifies
/// removes one readback. The condition is per-layer, so the benefit is proportional rather than
/// all-or-nothing — a model with half its layers fully pinned loses half its stalls.
///
/// # And it is what makes IQ1_S experts worth having
///
/// Scout pins 6 of 16 experts at q4, so no layer qualifies today. At IQ1_S the expert bytes are 2.88x
/// smaller, so the same budget holds ~17 of 16 — every layer qualifies. **That is the chain: narrower
/// experts, more pinned, whole layers resident, on-device routing, no readback.** The memory saving
/// is not the point; it is the means.
pub fn layer_can_route(pinned: &[u32], n_experts: u32) -> bool {
    // Every expert, not merely enough of them. A selection the device makes is unknown to the CPU, so
    // a single unpinned expert means a possible miss the CPU cannot service — and a miss it cannot
    // service is worse than the stall this avoids.
    n_experts > 0 && (0..n_experts).all(|e| pinned.contains(&e))
}

/// How many of a model's layers could route, given a per-layer pin set.
pub fn routable_layers(per_layer_pinned: &[Vec<u32>], n_experts: u32) -> usize {
    per_layer_pinned.iter().filter(|p| layer_can_route(p, n_experts)).count()
}

/// The share of per-layer readbacks this removes, 0..1 — which is the share of the stall removed,
/// since the stalls are one per layer and equal in cost.
pub fn stall_reduction(per_layer_pinned: &[Vec<u32>], n_experts: u32) -> f32 {
    if per_layer_pinned.is_empty() { return 0.0 }
    routable_layers(per_layer_pinned, n_experts) as f32 / per_layer_pinned.len() as f32
}

#[cfg(test)]
mod route_condition_tests {
    use super::*;

    #[test]
    fn a_FULLY_PINNED_layer_can_route_while_the_model_streams() {
        // The insight: stream.is_none() is a property of the MODEL and the real constraint is a
        // property of the LAYER. If nothing can miss, the CPU has nothing to fetch.
        assert!(layer_can_route(&(0..16).collect::<Vec<_>>(), 16));
    }

    #[test]
    fn ONE_unpinned_expert_disqualifies_the_layer() {
        // Not "enough of them". A selection the device makes is unknown to the CPU, so a single
        // unpinned expert means a possible miss the CPU cannot service — worse than the stall this
        // avoids.
        let mut p: Vec<u32> = (0..16).collect();
        p.retain(|e| *e != 7);
        assert!(!layer_can_route(&p, 16));
    }

    #[test]
    fn SCOUT_TODAY_qualifies_for_nothing() {
        // 6 of 16 pinned at q4. This is the honest baseline: the condition changes nothing until the
        // experts get narrower.
        let layers: Vec<Vec<u32>> = (0..48).map(|_| (0..6).collect()).collect();
        assert_eq!(routable_layers(&layers, 16), 0);
        assert_eq!(stall_reduction(&layers, 16), 0.0);
    }

    #[test]
    fn IQ1_S_EXPERTS_would_qualify_every_layer() {
        // 2.88x smaller experts in the same budget: 6 of 16 becomes all 16. That is the chain —
        // narrower experts, more pinned, whole layers resident, on-device routing, no readback. The
        // memory saving is the means, not the point.
        let layers: Vec<Vec<u32>> = (0..48).map(|_| (0..16).collect()).collect();
        assert_eq!(stall_reduction(&layers, 16), 1.0);
    }

    #[test]
    fn the_benefit_is_PROPORTIONAL_not_all_or_nothing() {
        // Half the layers fully pinned removes half the stalls, which makes a partial pin budget
        // worth having rather than a failure.
        let layers: Vec<Vec<u32>> = (0..48).map(|i| {
            if i % 2 == 0 { (0..16).collect() } else { (0..6).collect() }
        }).collect();
        assert!((stall_reduction(&layers, 16) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_model_with_NO_experts_does_not_claim_to_route() {
        // A dense model has no MoE path to take, and returning true would be a division by an idea.
        assert!(!layer_can_route(&[], 0));
    }
}

/// # CORRECTION: this is redundant with `layer_can_route`, and the arithmetic below is still right
///
/// The 62x figure holds and the conclusion drawn from it did not. Dense MoE removes the readback by
/// computing every expert — **and it requires every expert resident, because you cannot compute what
/// you would have to stream.**
///
/// That is the same precondition `layer_can_route` needs. And when it holds, `layer_can_route`
/// already removes the readback for FREE: the readback exists so the CPU can FETCH a cold expert,
/// and `MOE_MATVEC_GLU_Q4` reads the expert index from `selw` on the GPU. With everything pinned
/// there is nothing to fetch, so nothing to read back — at 1x the compute rather than 16x.
///
/// **Dense MoE pays 16x compute to buy something already available at 1x.** The two are not
/// alternatives to weigh; one strictly dominates.
///
/// Kept rather than deleted because the model is correct and the crossover arithmetic is reusable —
/// if a case ever appears where the readback survives full residency, this is the calculation. What
/// is deleted is the belief that Scout is that case.
///
/// # SUPERSEDED — read this before implementing dense-MoE
///
/// **The v2 decode path already routes on the GPU with zero readbacks**, and it was built before
/// this analysis. `moe_ffn_routed` chooses it when:
///
/// ```text
/// t == 1  &&  (m.stream.is_none() || layer_resident)  &&  !m.cfg.weight_before
/// ```
///
/// So the readback that dense-MoE removes by computing all sixteen experts is ALREADY removed, at
/// 1x the compute instead of 16x, by a top-k kernel plus expert-indexed matvecs over the stacked
/// weights. Computing everything is strictly worse than routing correctly on the GPU.
///
/// **What actually blocks Scout is the two other conditions**, not the readback itself:
///
/// * `m.stream.is_none() || layer_resident` — Scout streams, and pins 6 of 16 experts, so no layer
///   qualifies. IQ1_S residency is what changes this.
/// * `!m.cfg.weight_before` — Scout has weight_before, which `MOE_PRESCALE` addresses and which is
///   still unvalidated.
///
/// The arithmetic below is kept because the CROSSOVER reasoning is sound and reusable — a 16x
/// compute multiplier landing on 0.1% of a step really is favourable. It is simply not the cheapest
/// way to reach that outcome here, and I proposed it without checking what the routed path already
/// did.
///
/// Is it faster to compute EVERY expert than to ask which one?
///
/// # The trade
///
/// `moe_ffn_v1` downloads router logits so the CPU can fetch the chosen experts. That readback is a
/// DEPENDENCY — nothing can proceed without knowing which expert — so it cannot be batched away.
///
/// **But it can be removed.** Compute all `n_experts` and mask by the routing weight: the router's
/// output never leaves the GPU, so there is nothing to wait for. The cost is `n_experts / top_k`
/// times the expert compute.
///
/// # Why that is worth considering here and nowhere else
///
/// Normally 16x the compute is absurd. When 99.9% of the step is spent WAITING, the compute being
/// multiplied is 0.1% of the time:
///
/// ```text
/// stalled 99.9%   compute 0.1% -> 1.6%    62x faster
/// stalled 99.0%   compute 1.0% -> 16%      6x faster
/// stalled 91.0%   compute 9.0% -> 144%   0.7x — a LOSS
/// ```
///
/// The crossover is where `compute * multiplier` reaches 1. Above roughly 94% stall it wins; below,
/// it loses badly. **That is a measurement, not a preference**, which makes it exactly the kind of
/// decision the autotuner already makes for kernels.
///
/// # The precondition
///
/// Every expert must be RESIDENT. You cannot compute what you would have to stream, and streaming
/// them all is the readback's cost paid sixteen times. So this needs the same thing
/// `layer_can_route` needs, and for the same reason: narrow enough experts that a whole layer fits.
pub fn dense_moe_worthwhile(sync_share: f32, n_experts: u32, top_k: u32) -> Result<f32, String> {
    if top_k == 0 || n_experts == 0 { return Err("a model with no experts has nothing to densify".into()) }
    let mult = n_experts as f32 / top_k as f32;
    let compute = (1.0 - sync_share).max(1e-6);
    let after = compute * mult;
    if after < 1.0 {
        Ok(1.0 / after)
    } else {
        Err(format!(
            "computing all {n_experts} experts costs {mult:.0}x the compute, and compute is already \
{:.0}% of the step — it would become {:.0}%. Only worth it when the stall dominates, which is a \
measurement rather than a preference", compute * 100.0, after * 100.0))
    }
}

/// The stall share above which computing everything wins.
pub fn dense_moe_crossover(n_experts: u32, top_k: u32) -> f32 {
    if n_experts == 0 || top_k == 0 { return 1.0 }
    1.0 - (top_k as f32 / n_experts as f32)
}

#[cfg(test)]
mod dense_moe_tests {
    use super::*;

    #[test]
    fn dense_moe_is_DOMINATED_by_layer_can_route_wherever_it_applies() {
        // Both need every expert resident. When that holds, layer_can_route removes the same
        // readback at 1x compute instead of 16x — so the favourable arithmetic below describes a
        // trade nobody should take.
        let all_pinned: Vec<u32> = (0..16).collect();
        assert!(layer_can_route(&all_pinned, 16), "the cheaper path already applies");
        assert!(dense_moe_worthwhile(0.999, 16, 1).is_ok(), "and the expensive one also would");
    }

    #[test]
    fn at_SCOUTS_measured_stall_computing_everything_is_dramatically_faster() {
        // 99.9% waiting means the 16x multiplier lands on 0.1% of the step.
        let s = dense_moe_worthwhile(0.999, 16, 1).unwrap();
        assert!(s > 50.0, "got {s}x");
    }

    #[test]
    fn at_a_MODERATE_stall_it_is_a_LOSS_and_says_so() {
        // 91% was turn two of a real conversation. The same change that wins 62x at turn three loses
        // at turn two, which is why this must be measured per-run rather than decided once.
        let e = dense_moe_worthwhile(0.91, 16, 1).unwrap_err();
        assert!(e.contains("Only worth it when the stall dominates"));
    }

    #[test]
    fn the_CROSSOVER_is_where_the_multiplier_eats_the_saving() {
        // 16 experts at top-1: compute must be under 1/16, so the stall must exceed 15/16.
        let c = dense_moe_crossover(16, 1);
        assert!((c - 0.9375).abs() < 1e-4, "got {c}");
        assert!(dense_moe_worthwhile(c + 0.01, 16, 1).is_ok());
        assert!(dense_moe_worthwhile(c - 0.01, 16, 1).is_err());
    }

    #[test]
    fn a_WIDER_top_k_makes_it_cheaper_and_the_crossover_lower() {
        // Top-2 of 16 is an 8x multiplier, so it wins at a lower stall — the trade improves as the
        // router already touches more experts.
        assert!(dense_moe_crossover(16, 2) < dense_moe_crossover(16, 1));
    }

    #[test]
    fn the_module_says_v2_ALREADY_removes_this_readback() {
        // Guards against someone implementing dense-MoE from the arithmetic without reading why it
        // is not the cheapest route. The v2 path removes the same readback at 1x compute.
        let src = include_str!("expert_cache.rs");
        assert!(src.contains("SUPERSEDED"));
        assert!(src.contains("already removed, at\n/// 1x the compute instead of 16x")
             || src.contains("1x the compute instead of 16x"));
    }

    #[test]
    fn a_DENSE_model_is_refused_rather_than_given_a_ratio() {
        assert!(dense_moe_worthwhile(0.99, 0, 0).is_err());
    }
}

/// Choosing between routed and dense MoE at runtime, with hysteresis.
///
/// # Why this cannot be a constant
///
/// The crossover for Scout is 93.75% stall, and a real conversation measured **74%, 91%, then 112%**
/// across three turns. So the right answer changes WITHIN a session: dense loses at turn one and wins
/// by 60x at turn three, because the stall compounds with context length.
///
/// A fixed choice is wrong for two thirds of that conversation.
///
/// # Why it needs a dwell
///
/// A measurement near the crossover flips the decision, and each flip costs a mode change. Two
/// options within noise of each other alternating is the flapping `dwell` was written for — and here
/// the switch is not free, because dense mode requires every expert resident and routed mode does
/// not, so a flip can force a residency change.
///
/// # top_k is ONE for Scout, and it matters
///
/// vLLM's Llama-4 post states one expert activated per token. A search summary claimed two; the
/// difference moves the crossover from 93.75% to 87.5%, which is the gap between "wins at turn
/// three only" and "wins at turn two as well". Primary source over summary.
pub struct MoeModeChoice {
    dense: bool,
    dwell: std::time::Instant,
    min_dwell: std::time::Duration,
    switches: u32,
}

impl MoeModeChoice {
    pub fn new() -> Self {
        MoeModeChoice {
            dense: false,
            dwell: std::time::Instant::now(),
            // Longer than dwell's default: a mode change here may force experts to become resident,
            // which is far more expensive than a kernel swap.
            min_dwell: std::time::Duration::from_secs(120),
            switches: 0,
        }
    }

    pub fn dense(&self) -> bool { self.dense }
    pub fn switches(&self) -> u32 { self.switches }

    /// Consider switching, given the measured stall and whether every expert is resident.
    ///
    /// Returns the reason when it declines, because "it stayed routed" is not informative and "it
    /// stayed routed because only 6 of 16 experts are resident" is.
    pub fn consider(&mut self, sync_share: f32, all_resident: bool, n_experts: u32, top_k: u32)
        -> Result<bool, String>
    {
        // The hard precondition, checked before anything else: dense mode computes every expert, and
        // computing one that is not resident means streaming it — which is the readback's cost paid
        // n_experts times, the opposite of the intent.
        if !all_resident {
            if self.dense { self.dense = false; }   // residency was lost; fall back immediately
            return Err(format!("dense MoE needs every expert resident and they are not. This is the \
same precondition as layer_can_route, and it is what narrower experts buy"));
        }
        let want = dense_moe_worthwhile(sync_share, n_experts, top_k).is_ok();
        if want == self.dense { return Ok(self.dense) }
        let waited = self.dwell.elapsed();
        if waited < self.min_dwell {
            return Err(format!("{} is {:.0}% better right now, but the last change was {}s ago — a \
measurement near the {:.0}% crossover flips this, and each flip may force a residency change",
                if want { "dense" } else { "routed" },
                (sync_share * 100.0), waited.as_secs(),
                dense_moe_crossover(n_experts, top_k) * 100.0));
        }
        self.dense = want;
        self.dwell = std::time::Instant::now();
        self.switches += 1;
        Ok(self.dense)
    }
}

impl Default for MoeModeChoice { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod mode_choice_tests {
    use super::*;

    fn ready() -> MoeModeChoice {
        let mut m = MoeModeChoice::new();
        m.dwell = std::time::Instant::now() - std::time::Duration::from_secs(300);
        m
    }

    #[test]
    fn a_REAL_CONVERSATION_flips_the_right_answer_mid_session() {
        // 74%, 91%, 112% across three turns. Dense loses at turn one and wins at turn three, so a
        // fixed choice is wrong for two thirds of it.
        assert!(dense_moe_worthwhile(0.74, 16, 1).is_err(), "turn one: routed");
        assert!(dense_moe_worthwhile(0.91, 16, 1).is_err(), "turn two: still routed");
        assert!(dense_moe_worthwhile(0.999, 16, 1).is_ok(), "turn three: dense, by 60x");
    }

    #[test]
    fn NON_RESIDENT_experts_refuse_dense_mode_outright() {
        // Computing an expert that is not resident means streaming it — the readback's cost paid
        // sixteen times, which is the opposite of the intent.
        let e = ready().consider(0.999, false, 16, 1).unwrap_err();
        assert!(e.contains("needs every expert resident"));
        assert!(e.contains("what narrower experts buy"));
    }

    #[test]
    fn losing_residency_falls_back_IMMEDIATELY_without_waiting_for_the_dwell() {
        // The dwell protects against flapping on a marginal measurement. Losing residency is not a
        // marginal measurement, and holding dense mode without the experts would be catastrophic.
        let mut m = ready();
        assert!(m.consider(0.999, true, 16, 1).unwrap(), "dense");
        let _ = m.consider(0.999, false, 16, 1);
        assert!(!m.dense(), "fell back at once");
    }

    #[test]
    fn a_measurement_near_the_CROSSOVER_does_not_flap() {
        // 93.75% for Scout, and readings either side of it arrive turn to turn.
        let mut m = MoeModeChoice::new();
        let e = m.consider(0.999, true, 16, 1).unwrap_err();
        assert!(e.contains("crossover"));
        assert!(!m.dense(), "held by the dwell");
    }

    #[test]
    fn TOP_K_moves_the_crossover_and_the_source_matters() {
        // vLLM's Llama-4 post says ONE expert per token; a search summary said two. The difference is
        // 93.75% against 87.5% — the gap between winning at turn three only and winning at turn two
        // as well.
        assert!((dense_moe_crossover(16, 1) - 0.9375).abs() < 1e-4);
        assert!((dense_moe_crossover(16, 2) - 0.875).abs() < 1e-4);
    }

    #[test]
    fn a_DECLINE_says_why_rather_than_just_staying_put() {
        // "It stayed routed" is not informative. "It stayed routed because only 6 of 16 experts are
        // resident" is.
        assert!(MoeModeChoice::new().consider(0.999, false, 16, 1).is_err());
    }
}
