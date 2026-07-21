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

    /// Fold another branch's observations into this map (histogram add). This is how sibling fan-out
    /// branches POOL their lexical maps: N branches exploring a related sub-problem collectively warm one
    /// map, so a cold branch inherits every other branch's token→expert discoveries instead of warming from
    /// scratch. Only ever call with a RELATED branch's map (see `branch_affinity`) — pooling a divergent
    /// branch pollutes the prediction.
    pub fn merge_from(&mut self, other: &ExpertMap) {
        for (&k, m) in &other.lexical { let e = self.lexical.entry(k).or_default(); for (&x, &c) in m { *e.entry(x).or_insert(0) += c; } }
        for (&k, m) in &other.marginal { let e = self.marginal.entry(k).or_default(); for (&x, &c) in m { *e.entry(x).or_insert(0) += c; } }
        for (&k, m) in &other.coupling { let e = self.coupling.entry(k).or_default(); for (&x, &c) in m { *e.entry(x).or_insert(0) += c; } }
    }

    /// The distinct tokens this map has observed (its vocabulary) — the signal a relation-search compares
    /// across branches to decide whether two fan-outs are exploring related work.
    pub fn vocabulary(&self) -> std::collections::HashSet<u32> {
        self.lexical.keys().map(|(sig, _)| sig.tok).collect()
    }
}

/// Relation-search primitive: how related are two fan-out branches, by the Jaccard overlap of their token
/// vocabularies? High = they reuse the same words (type names, keywords, APIs) → their expert-routing maps
/// are correlated and worth pooling. Low = divergent sub-problems → pooling would pollute. This is the
/// cheap, deterministic core of "small models searching for relations across fan-outs"; a small model can
/// supply a semantic version, but lexical overlap is the buildable-now signal and needs no model.
pub fn branch_affinity(a: &std::collections::HashSet<u32>, b: &std::collections::HashSet<u32>) -> f64 {
    if a.is_empty() && b.is_empty() { return 1.0; }
    let inter = a.intersection(b).count();
    let uni = a.union(b).count();
    if uni == 0 { 0.0 } else { inter as f64 / uni as f64 }
}

/// A pool of per-branch expert maps that share discoveries across RELATED fan-out branches. Each branch
/// warms its own map from its own decode; when predicting, a branch also consults the pooled map built from
/// every sibling whose `branch_affinity` clears a threshold — so a cold branch rides its relatives' warm-up.
/// This is the fan-out cache accelerator: instead of N branches each warming independently, related ones
/// collectively fill a shared map, and the prefetch hit-rate climbs immediately.
#[derive(Default)]
pub struct FanoutPool {
    branches: Vec<BranchEntry>,
    threshold: f64,
}

struct BranchEntry {
    id: u64,
    vocab: std::collections::HashSet<u32>,
    map: ExpertMap,
}

impl FanoutPool {
    /// `threshold` is the minimum branch_affinity to pool two branches (e.g. 0.2 — a fifth of vocabulary
    /// shared). Higher = more conservative (only very related branches pool); lower = more sharing, more
    /// pollution risk.
    pub fn new(threshold: f64) -> Self { FanoutPool { branches: Vec::new(), threshold } }

    /// Register (or replace) a branch's map + vocabulary. Called as each fan-out branch accumulates routing.
    pub fn upsert(&mut self, id: u64, map: ExpertMap) {
        let vocab = map.vocabulary();
        if let Some(b) = self.branches.iter_mut().find(|b| b.id == id) { b.vocab = vocab; b.map = map; }
        else { self.branches.push(BranchEntry { id, vocab, map }); }
    }

    /// Build a pooled map for branch `id`: its own map merged with every sibling whose affinity clears the
    /// threshold. The result is what that branch predicts from — its own warm-up plus its relatives'. A
    /// divergent sibling (low affinity) is left out, so its routing can't pollute this branch's prefetch.
    pub fn pooled_for(&self, id: u64) -> ExpertMap {
        let mut out = ExpertMap::new();
        let Some(me) = self.branches.iter().find(|b| b.id == id) else { return out };
        out.merge_from(&me.map);
        for other in &self.branches {
            if other.id == id { continue; }
            if branch_affinity(&me.vocab, &other.vocab) >= self.threshold {
                out.merge_from(&other.map);
            }
        }
        out
    }

    /// How many siblings would pool with `id` at the current threshold (diagnostics / a UI "N related
    /// branches feeding this one's cache" readout).
    pub fn related_count(&self, id: u64) -> usize {
        let Some(me) = self.branches.iter().find(|b| b.id == id) else { return 0 };
        self.branches.iter().filter(|o| o.id != id && branch_affinity(&me.vocab, &o.vocab) >= self.threshold).count()
    }

    pub fn len(&self) -> usize { self.branches.len() }
    pub fn is_empty(&self) -> bool { self.branches.is_empty() }
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

    #[test]
    fn branch_affinity_measures_vocabulary_overlap() {
        use std::collections::HashSet;
        let a: HashSet<u32> = [1, 2, 3, 4].into_iter().collect();
        let b: HashSet<u32> = [3, 4, 5, 6].into_iter().collect(); // 2 shared / 6 union
        let c: HashSet<u32> = [90, 91].into_iter().collect();     // disjoint
        assert!((branch_affinity(&a, &b) - 2.0 / 6.0).abs() < 1e-9);
        assert_eq!(branch_affinity(&a, &c), 0.0);
    }

    #[test]
    fn cold_branch_inherits_a_related_sibling_warmup() {
        // Branch 1 (warm): token 42 → experts {3,7} at layer 0, and shares vocabulary with branch 2.
        let mut warm = ExpertMap::new();
        for _ in 0..5 { warm.observe(CtxSig::unigram(42), 0, &[3, 7], &[]); }
        for t in [10, 11, 12] { warm.observe(CtxSig::unigram(t), 0, &[1], &[]); } // shared vocab
        // Branch 2 (cold on token 42) but overlapping vocabulary {10,11,12}.
        let mut cold = ExpertMap::new();
        for t in [10, 11, 12] { cold.observe(CtxSig::unigram(t), 0, &[1], &[]); }
        let mut pool = FanoutPool::new(0.2);
        pool.upsert(1, warm);
        pool.upsert(2, cold);
        assert_eq!(pool.related_count(2), 1, "branch 2 sees branch 1 as related");
        // pooled map for the cold branch now predicts token 42's experts it never saw itself
        let pooled = pool.pooled_for(2);
        let pred = pooled.predict(CtxSig::unigram(42), 0, &[], 4);
        assert!(pred.contains(&3) && pred.contains(&7), "cold branch inherits sibling's warm-up: {pred:?}");
    }

    #[test]
    fn divergent_branch_is_not_pooled_no_pollution() {
        // Branch A: token 42 → {3,7}, vocab {42, 10,11}. Branch B: totally different vocab {900,901,902},
        // routing token 42 → {50,51} (a DIFFERENT approach). They must NOT pool.
        let mut a = ExpertMap::new();
        for _ in 0..5 { a.observe(CtxSig::unigram(42), 0, &[3, 7], &[]); }
        for t in [10, 11] { a.observe(CtxSig::unigram(t), 0, &[1], &[]); }
        let mut b = ExpertMap::new();
        for _ in 0..5 { b.observe(CtxSig::unigram(42), 0, &[50, 51], &[]); }
        for t in [900, 901, 902] { b.observe(CtxSig::unigram(t), 0, &[2], &[]); }
        let mut pool = FanoutPool::new(0.2);
        pool.upsert(1, a);
        pool.upsert(2, b);
        assert_eq!(pool.related_count(1), 0, "disjoint vocab → not related");
        // A's pooled prediction for token 42 must stay {3,7}-led, NOT polluted by B's {50,51}
        let pred = pool.pooled_for(1).predict(CtxSig::unigram(42), 0, &[], 2);
        assert!(pred.contains(&3) && pred.contains(&7), "divergent branch didn't pollute: {pred:?}");
        assert!(!pred.contains(&50) && !pred.contains(&51));
    }

    #[test]
    fn merge_is_additive() {
        let mut a = ExpertMap::new();
        a.observe(CtxSig::unigram(1), 0, &[5], &[]);
        let mut b = ExpertMap::new();
        for _ in 0..3 { b.observe(CtxSig::unigram(1), 0, &[9], &[]); }
        a.merge_from(&b);
        // after merge, token 1 at layer 0 has seen 5 once and 9 three times → 9 leads
        let pred = a.predict(CtxSig::unigram(1), 0, &[], 1);
        assert_eq!(pred, vec![9]);
    }

    #[test]
    fn pooling_lifts_a_thin_branch_recall_quantified() {
        // The payoff, measured: a task's shared vocabulary of 30 tokens each route to a fixed expert set.
        // 4 sibling branches each observe only a THIN slice (8 tokens) of that vocabulary — individually
        // cold on the other 22. Pooled across the (all related) siblings, each branch can predict the whole
        // vocabulary. Recall for a target branch on the FULL vocab should jump when pooled.
        let mut pref = std::collections::HashMap::new();
        for t in 0..30u32 { pref.insert(t, vec![(t % 8), (t % 8) + 8, (t % 8) + 16]); }
        let mk = |toks: &[u32]| { let mut m = ExpertMap::new(); for &t in toks { for _ in 0..3 { m.observe(CtxSig::unigram(t), 0, &pref[&t], &[]); } } m };
        let mut pool = FanoutPool::new(0.1);
        // 4 branches, overlapping thin slices covering the vocab between them
        pool.upsert(1, mk(&[0,1,2,3,4,5,6,7]));
        pool.upsert(2, mk(&[6,7,8,9,10,11,12,13]));
        pool.upsert(3, mk(&[12,13,14,15,16,17,18,19]));
        pool.upsert(4, mk(&[18,19,20,21,22,23,24,25]));
        // target = branch 1, alone vs pooled — recall over the full 0..30 vocab (budget covers a set)
        let alone = mk(&[0,1,2,3,4,5,6,7]);
        let pooled = pool.pooled_for(1);
        let score = |m: &ExpertMap| -> f64 {
            let mut hit = 0.0; let mut n = 0.0;
            for t in 0..30u32 { let pred = m.predict(CtxSig::unigram(t), 0, &[], 4); hit += recall(&pred, &pref[&t]); n += 1.0; }
            hit / n
        };
        let (r_alone, r_pooled) = (score(&alone), score(&pooled));
        // pooling related siblings materially lifts recall over the full vocab (measured ~+16pp here; the
        // real lift grows with how much of the task's vocabulary the siblings collectively cover).
        assert!(r_pooled > r_alone + 0.1, "pooling related siblings lifts recall: alone {r_alone:.2} → pooled {r_pooled:.2}");
    }
}
