//! expert_predict.rs — predict a MoE layer's routed expert set BEFORE the router runs, so an SSD/streaming
//! backend can prefetch those experts off disk in time. Pure + testable; no GPU, no model, no I/O.
//!
//! Why this exists: for a model whose experts don't fit in RAM (GLM-5.2 at ~223 GB, the big Qwens, Llama-4
//! Maverick), decode streams each layer's selected experts off SSD on demand. A blocking miss stalls the
//! whole token. If you can PREDICT the selected set a few layers ahead — or before the router runs at all —
//! you issue the disk readahead early and the bytes are warm when the matmul needs them. Colibri's own
//! measurement put GLM's one-layer-ahead coupling at ~71.6% recall; this module adds a stronger,
//! CONTEXT-CONDITIONED predictor on top of that baseline.
//!
//! The idea (the "lexical map"): routing is not random — it correlates with the TOKEN and its recent
//! context. The same token/n-gram tends to select overlapping experts each time it appears. So we key a
//! per-layer expert histogram on a small context signature (the current token, optionally the previous
//! token) and, when that signature recurs, predict the experts it selected before. The KV *tree* supplies
//! the token lineage this keys on for free — a long-horizon task revisits its own vocabulary constantly
//! (type names, API calls, keywords), so the map's hit rate climbs as the task goes.
//!
//! Three predictors compose, strongest-first, filling a fixed budget of B experts per layer:
//!   1. LEXICAL — experts this context-signature chose before (the new signal; needs a warm map).
//!   2. COUPLING — experts co-activated with the PREVIOUS layer's selected set (Colibri's one-ahead).
//!   3. MARGINAL — the layer's globally-hottest experts (the always-available floor).
//! A prediction is B experts; recall = fraction of the actually-routed set that lands in the B. Budget is
//! the streaming backend's prefetch width (how many experts it can pull ahead without thrashing the cache).

use std::collections::HashMap;

/// A compact context signature to condition routing on. Keeping it small keeps the map dense (more hits)
/// and cheap. `tok` is the current token id; `prev` is the previous token (0 = none), which sharpens the
/// prediction where a single token is ambiguous but a bigram isn't.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CtxSig {
    pub tok: u32,
    pub prev: u32,
}

impl CtxSig {
    pub fn unigram(tok: u32) -> Self { CtxSig { tok, prev: 0 } }
    pub fn bigram(prev: u32, tok: u32) -> Self { CtxSig { tok, prev } }
}

/// Per-(signature, layer) expert histogram — the lexical map. Also holds the marginal (per-layer global
/// heat) and the one-layer-ahead coupling table, so all three predictors share one trained object.
#[derive(Default)]
pub struct ExpertMap {
    /// (signature, layer) → expert → count. The context-conditioned prior.
    lexical: HashMap<(CtxSig, i32), HashMap<u32, u32>>,
    /// layer → expert → count. Marginal heat (the floor).
    marginal: HashMap<i32, HashMap<u32, u32>>,
    /// (layer, expert_at_L) → expert_at_L+1 → count. One-layer-ahead coupling.
    coupling: HashMap<(i32, u32), HashMap<u32, u32>>,
}

impl ExpertMap {
    pub fn new() -> Self { Self::default() }

    /// Record one layer's routed set for a token with the given context signature. `prev_layer_experts` is
    /// the set routed at layer-1 for the SAME token (empty for layer 0), feeding the coupling table.
    pub fn observe(&mut self, sig: CtxSig, layer: i32, experts: &[u32], prev_layer_experts: &[u32]) {
        let lex = self.lexical.entry((sig, layer)).or_default();
        for &e in experts { *lex.entry(e).or_insert(0) += 1; }
        let marg = self.marginal.entry(layer).or_default();
        for &e in experts { *marg.entry(e).or_insert(0) += 1; }
        if !prev_layer_experts.is_empty() {
            for &x in prev_layer_experts {
                let row = self.coupling.entry((layer - 1, x)).or_default();
                for &y in experts { *row.entry(y).or_insert(0) += 1; }
            }
        }
    }

    /// Predict the top-`budget` experts for `layer` given the context signature and the previous layer's
    /// selected set. Blends the three predictors: lexical counts (weighted highest since they're the
    /// context-specific signal), coupling counts, and marginal heat as backfill. Returns exactly up to
    /// `budget` expert ids, highest-scored first.
    pub fn predict(&self, sig: CtxSig, layer: i32, prev_layer_experts: &[u32], budget: usize) -> Vec<u32> {
        let mut score: HashMap<u32, f64> = HashMap::new();
        // 1) lexical — the context-conditioned prior (weight 3.0: most specific).
        if let Some(m) = self.lexical.get(&(sig, layer)) {
            let total: u32 = m.values().sum();
            if total > 0 { for (&e, &c) in m { *score.entry(e).or_insert(0.0) += 3.0 * c as f64 / total as f64; } }
        }
        // 2) coupling — co-activation with the previous layer's set (weight 2.0).
        for &x in prev_layer_experts {
            if let Some(m) = self.coupling.get(&(layer - 1, x)) {
                let total: u32 = m.values().sum();
                if total > 0 { for (&e, &c) in m { *score.entry(e).or_insert(0.0) += 2.0 * c as f64 / total as f64; } }
            }
        }
        // 3) marginal — global heat backfill (weight 1.0), so a cold signature still yields a full budget.
        if let Some(m) = self.marginal.get(&layer) {
            let total: u32 = m.values().sum();
            if total > 0 { for (&e, &c) in m { *score.entry(e).or_insert(0.0) += 1.0 * c as f64 / total as f64; } }
        }
        top_scored(&score, budget)
    }

    /// Does the lexical map have an entry for this signature+layer? (A streaming backend can use the
    /// presence of a warm lexical hit to decide whether to trust an early/aggressive prefetch.)
    pub fn has_lexical(&self, sig: CtxSig, layer: i32) -> bool { self.lexical.contains_key(&(sig, layer)) }

    pub fn is_empty(&self) -> bool { self.marginal.is_empty() }
}

/// Top-`k` keys of a score map, highest first (deterministic tie-break by expert id for reproducibility).
fn top_scored(m: &HashMap<u32, f64>, k: usize) -> Vec<u32> {
    let mut v: Vec<(f64, u32)> = m.iter().map(|(&e, &s)| (s, e)).collect();
    v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
    v.truncate(k);
    v.into_iter().map(|(_, e)| e).collect()
}

/// Recall of an actually-routed set under a prediction: fraction of `actual` covered by `predicted`.
pub fn recall(predicted: &[u32], actual: &[u32]) -> f64 {
    if actual.is_empty() { return 1.0; }
    let pset: std::collections::HashSet<u32> = predicted.iter().copied().collect();
    let hit = actual.iter().filter(|e| pset.contains(e)).count();
    hit as f64 / actual.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_map_predicts_a_recurring_token() {
        let mut m = ExpertMap::new();
        // token 42 at layer 0 always routes to experts {3,7}; train it a few times
        for _ in 0..5 { m.observe(CtxSig::unigram(42), 0, &[3, 7], &[]); }
        // and some noise from other tokens so marginal isn't dominated by 3,7
        for t in 100..120 { m.observe(CtxSig::unigram(t), 0, &[t % 8], &[]); }
        let pred = m.predict(CtxSig::unigram(42), 0, &[], 4);
        // the lexical signal should surface 3 and 7 at the top
        assert!(pred.contains(&3) && pred.contains(&7), "recurring token's experts predicted: {pred:?}");
    }

    #[test]
    fn cold_signature_falls_back_to_marginal_and_still_fills_budget() {
        let mut m = ExpertMap::new();
        for _ in 0..10 { m.observe(CtxSig::unigram(1), 0, &[5, 6], &[]); } // 5,6 are globally hot
        // an unseen token: no lexical entry, must still return a full budget from marginal
        let pred = m.predict(CtxSig::unigram(999), 0, &[], 2);
        assert_eq!(pred.len(), 2);
        assert!(pred.contains(&5) && pred.contains(&6));
    }

    #[test]
    fn coupling_contributes_when_prev_layer_known() {
        let mut m = ExpertMap::new();
        // whenever L0 picks expert 1, L1 picks expert 9 (strong coupling)
        for _ in 0..8 {
            m.observe(CtxSig::unigram(1), 0, &[1], &[]);
            m.observe(CtxSig::unigram(1), 1, &[9], &[1]);
        }
        // predict L1 for a DIFFERENT token but with prev-layer set {1}: coupling should surface 9
        let pred = m.predict(CtxSig::unigram(777), 1, &[1], 3);
        assert!(pred.contains(&9), "coupling surfaces the co-activated expert: {pred:?}");
    }

    #[test]
    fn recall_math() {
        assert!((recall(&[1, 2, 3], &[2, 3]) - 1.0).abs() < 1e-9);
        assert!((recall(&[1, 2], &[2, 3]) - 0.5).abs() < 1e-9);
        assert!((recall(&[], &[]) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn bigram_sharpens_over_unigram() {
        let mut m = ExpertMap::new();
        // token 5 after token 1 → experts {2}; token 5 after token 8 → experts {4}
        for _ in 0..6 { m.observe(CtxSig::bigram(1, 5), 0, &[2], &[]); }
        for _ in 0..6 { m.observe(CtxSig::bigram(8, 5), 0, &[4], &[]); }
        let p_after1 = m.predict(CtxSig::bigram(1, 5), 0, &[], 1);
        let p_after8 = m.predict(CtxSig::bigram(8, 5), 0, &[], 1);
        assert_eq!(p_after1, vec![2]);
        assert_eq!(p_after8, vec![4], "the bigram context disambiguates the same token");
    }

    #[test]
    fn has_lexical_reports_warmth() {
        let mut m = ExpertMap::new();
        assert!(!m.has_lexical(CtxSig::unigram(3), 0));
        m.observe(CtxSig::unigram(3), 0, &[1], &[]);
        assert!(m.has_lexical(CtxSig::unigram(3), 0));
    }
}
