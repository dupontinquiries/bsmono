//! dsa.rs — DeepSeek Sparse Attention (DSA) lightning indexer, pure CPU reference.
//!
//! The context-lengthening lever beyond MLA's KV shrink: instead of attending over ALL past keys (O(n²)),
//! a small separate "indexer" attention scores every key and the main attention keeps only the top
//! `index_topk`. Above the threshold this makes attention sub-quadratic (bounded key count), while at or
//! below it the indexer is a NO-OP (keeps all keys → dense, exact). That no-op property is the correctness
//! anchor — a sparse selection that includes all keys must produce the SAME attention as dense, so the
//! reference below is validated against dense attention at seq ≤ index_topk (mirrors Colibri's DSA_FORCE=1
//! "selecting all must equal the dense output" test, glm.c:2612).
//!
//! Indexer score for key t (Colibri glm.c:2647): a ReLU'd, per-head-weighted dot product —
//!   score[t] = (1/√nh) · Σ_h w[h] · ReLU( (q_idx[h] · k_idx[t]) / √hd )
//! where q_idx = ix_wq·q_latent (nh heads × hd, rope'd), k_idx = ix_wk·x (layernorm'd, rope'd, cached),
//! and w = ix_wp·x (nh per-head weights). Selection keeps the top `min(nk, index_topk)` by score.

/// Indexer dims.
#[derive(Clone, Copy)]
pub struct DsaDims { pub index_nh: usize, pub index_hd: usize, pub index_topk: usize }

/// Score all `nk` cached keys for one query (returns one score per key). `q_idx` is [nh·hd] (rope'd),
/// `k_idx` is [nk, nh? no — hd shared across heads per Colibri: k is [nk, hd]], `w` is [nh].
/// Colibri's k_idx is a single hd-wide vector per position (shared across the nh indexer heads); each head
/// dots its own q slice against that shared key. This mirrors that.
pub fn indexer_scores(q_idx: &[f32], k_idx: &[f32], w: &[f32], nk: usize, dm: &DsaDims) -> Vec<f32> {
    let (nh, hd) = (dm.index_nh, dm.index_hd);
    let wsc = 1.0 / (nh as f32).sqrt();
    let rs = 1.0 / (hd as f32).sqrt();
    let mut scores = vec![0f32; nk];
    for (t, score) in scores.iter_mut().enumerate() {
        let kt = &k_idx[t * hd..(t + 1) * hd];
        let mut a = 0f32;
        for h in 0..nh {
            let qh = &q_idx[h * hd..(h + 1) * hd];
            let mut d0 = 0f32;
            for i in 0..hd { d0 += qh[i] * kt[i]; }
            d0 *= rs;
            if d0 > 0.0 { a += w[h] * d0; } // ReLU on the per-head score, then weight
        }
        *score = a * wsc;
    }
    scores
}

/// Select the indices of the top `keep = min(nk, index_topk)` keys by score, returned in ASCENDING position
/// order (so the downstream masked attention walks them causally). Matches Colibri's partial-select +
/// position-order scan: threshold = the keep-th largest score, take scores > thr then == thr up to keep.
pub fn select_topk(scores: &[f32], dm: &DsaDims) -> Vec<usize> {
    let nk = scores.len();
    let keep = nk.min(dm.index_topk);
    if keep >= nk {
        // no-op: all keys selected (the dense-equivalent case)
        return (0..nk).collect();
    }
    // threshold = keep-th largest score
    let mut sorted = scores.to_vec();
    sorted.sort_by(|a, b| b.total_cmp(a));
    let thr = sorted[keep - 1];
    let mut sel = Vec::with_capacity(keep);
    for (t, &s) in scores.iter().enumerate() { if sel.len() < keep && s > thr { sel.push(t); } }
    for (t, &s) in scores.iter().enumerate() { if sel.len() < keep && s == thr { sel.push(t); } }
    sel.sort_unstable();
    sel
}

/// Whether the indexer is a no-op at this length (all keys kept → dense-exact).
pub fn is_noop(nk: usize, dm: &DsaDims) -> bool { nk <= dm.index_topk }

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded(n: usize, seed: u64) -> Vec<f32> {
        let mut s = seed;
        (0..n).map(|_| { s = s.wrapping_mul(6364136223846793005).wrapping_add(1); ((s >> 33) as f32 / (1u64 << 31) as f32) - 1.0 }).collect()
    }

    #[test]
    fn indexer_is_noop_below_threshold() {
        // At nk <= index_topk the selection is ALL keys in order — the dense-equivalent case that anchors
        // correctness (a downstream attention masked to these keys == unmasked dense attention).
        let dm = DsaDims { index_nh: 2, index_hd: 4, index_topk: 8 };
        let nk = 5;
        let q = seeded(dm.index_nh * dm.index_hd, 1);
        let k = seeded(nk * dm.index_hd, 2);
        let w = seeded(dm.index_nh, 3);
        let scores = indexer_scores(&q, &k, &w, nk, &dm);
        let sel = select_topk(&scores, &dm);
        assert!(is_noop(nk, &dm));
        assert_eq!(sel, (0..nk).collect::<Vec<_>>(), "below threshold: all keys, in order");
    }

    #[test]
    fn indexer_selects_bounded_topk_above_threshold() {
        // Above the threshold the count is bounded to index_topk (the sub-quadratic property) and the
        // selection is exactly the highest-scoring keys, returned in ascending position order.
        let dm = DsaDims { index_nh: 2, index_hd: 4, index_topk: 3 };
        let nk = 10;
        let q = seeded(dm.index_nh * dm.index_hd, 11);
        let k = seeded(nk * dm.index_hd, 12);
        let w = seeded(dm.index_nh, 13);
        let scores = indexer_scores(&q, &k, &w, nk, &dm);
        let sel = select_topk(&scores, &dm);
        assert_eq!(sel.len(), 3, "bounded to index_topk");
        assert!(sel.windows(2).all(|w| w[0] < w[1]), "ascending position order");
        // every selected key's score must be >= every unselected key's score
        let selmin = sel.iter().map(|&t| scores[t]).fold(f32::INFINITY, f32::min);
        let unselmax = (0..nk).filter(|t| !sel.contains(t)).map(|t| scores[t]).fold(f32::NEG_INFINITY, f32::max);
        assert!(selmin >= unselmax - 1e-6, "selected are the top-scoring keys");
    }

    // Simple single-head attention over a subset of key positions `sel` (ascending). Returns the [dv]
    // context. When sel = all positions, this is dense attention.
    fn attend_over(q: &[f32], k: &[f32], v: &[f32], sel: &[usize], dk: usize, dv: usize) -> Vec<f32> {
        let scale = 1.0 / (dk as f32).sqrt();
        let mut scores: Vec<f32> = sel.iter().map(|&t| {
            let kt = &k[t * dk..(t + 1) * dk];
            q.iter().zip(kt).map(|(a, b)| a * b).sum::<f32>() * scale
        }).collect();
        let mx = scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let mut denom = 0f32;
        for s in scores.iter_mut() { *s = (*s - mx).exp(); denom += *s; }
        let mut out = vec![0f32; dv];
        for (idx, &t) in sel.iter().enumerate() {
            let a = scores[idx] / denom;
            let vt = &v[t * dv..(t + 1) * dv];
            for i in 0..dv { out[i] += a * vt[i]; }
        }
        out
    }

    #[test]
    fn dsa_selection_masked_attention_equals_dense_below_threshold() {
        // THE correctness anchor (Colibri DSA_FORCE=1): below index_topk the indexer keeps all keys, so
        // attention masked to the DSA selection must EXACTLY equal full dense attention. This is what lets
        // DSA be enabled unconditionally — it can never change short-context output.
        let dm = DsaDims { index_nh: 2, index_hd: 4, index_topk: 16 };
        let nk = 6; // <= index_topk → no-op
        let q_idx = seeded(dm.index_nh * dm.index_hd, 21);
        let k_idx = seeded(nk * dm.index_hd, 22);
        let w = seeded(dm.index_nh, 23);
        let sel = select_topk(&indexer_scores(&q_idx, &k_idx, &w, nk, &dm), &dm);
        // main attention with arbitrary q/k/v
        let (dk, dv) = (5, 3);
        let q = seeded(dk, 31); let k = seeded(nk * dk, 32); let v = seeded(nk * dv, 33);
        let dense = attend_over(&q, &k, &v, &(0..nk).collect::<Vec<_>>(), dk, dv);
        let sparse = attend_over(&q, &k, &v, &sel, dk, dv);
        let err = dense.iter().zip(&sparse).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(err == 0.0, "below threshold, DSA-masked attention must be bit-identical to dense, err={err}");
    }

    #[test]
    fn relu_gate_zeros_negative_head_scores() {
        // The ReLU on each head's score means a head contributes only when its q·k is positive — a key
        // whose every head dots negative gets score 0 (Colibri: `if(d0>0) a+=w*d0`).
        let dm = DsaDims { index_nh: 1, index_hd: 2, index_topk: 4 };
        let q = vec![1.0, 1.0];
        let k = vec![-1.0, -1.0]; // q·k = -2 < 0 → ReLU → 0
        let w = vec![1.0];
        let scores = indexer_scores(&q, &k, &w, 1, &dm);
        assert_eq!(scores[0], 0.0, "negative head score is gated to zero");
    }
}
