//! reverse — which token most likely PRECEDED what you have: search-based
//! inversion of a causal model.
//!
//! A causal LM defines `P(next | past)` and nothing else; it cannot sample
//! `P(prev | future)` directly. But Bayes runs the search for free: for a
//! candidate predecessor `x` of a known suffix `S` after a prefix `P`,
//!
//! ```text
//! score(x) = log P(S | P, x) = Σ_i log P(S_i | P, x, S_..i)
//! ```
//!
//! and `verify_forward` produces every one of those per-position logits in a
//! SINGLE forward over `[x] ++ S` — the same batched pass speculative
//! verification uses. Ranking candidates by `score` is reverse token
//! generation as inference-time search: exact with the whole vocabulary as
//! candidates, cheap with a shortlist.
//!
//! The strange-ideas surface this unlocks (c11 ask, deliberately left open):
//! walking a generation BACKWARD from an anchor to see where the model
//! believes it came from; recovering a lost prompt tail from its reply;
//! detecting that a decode has returned near an earlier state (score a
//! return-step against the remembered continuation); palindromic decoding
//! games. What the sandbox proves is narrower and stated exactly: the score
//! is what the formula says, per test, against an independent recomputation.
//! Whether the search finds anything INTERESTING is the on-device
//! experiment.

use crate::{Gpu, Model};

/// Log-softmax of one vocab row at index `tok`, computed CPU-side from raw
/// logits. Public so tests recompute scores independently of `reverse_step`.
pub fn logprob_at(row: &[f32], tok: u32) -> f32 {
    let m = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let lse = m + row.iter().map(|&v| (v - m).exp()).sum::<f32>().ln();
    row[tok as usize] - lse
}

/// Score each candidate predecessor of `suffix` after `prefix`; returns
/// `(candidate, log P(suffix | prefix, candidate))` sorted best-first.
///
/// One `verify_forward` per candidate over `[cand] ++ suffix`, all at the
/// same positions — each candidate's pass OVERWRITES the previous one's KV
/// rows above the prefix, so the prefix is prefilled once (chunked; the
/// c11-012 lesson) and the cache is reused without copies. Deterministic:
/// ties keep candidate order.
pub fn reverse_step(
    gpu: &Gpu,
    m: &Model,
    prefix: &[u32],
    candidates: &[u32],
    suffix: &[u32],
    cache_cap: usize,
) -> Vec<(u32, f32)> {
    let cache = gpu.new_cache(m.n_layers, cache_cap, m.kv_dim());
    if !prefix.is_empty() {
        let _ = gpu.prefill_chunked(m, &cache, prefix, 64);
    }
    let mut scored: Vec<(u32, f32)> = Vec::with_capacity(candidates.len());
    for &cand in candidates {
        let mut seq = Vec::with_capacity(1 + suffix.len());
        seq.push(cand);
        seq.extend_from_slice(suffix);
        // all rows: row i predicts the token at position prefix+i+1, i.e.
        // row 0 predicts suffix[0], row i predicts suffix[i] for i < len.
        let all = gpu.verify_forward(m, &cache, &seq, prefix.len());
        let mut score = 0.0f32;
        for (i, &s) in suffix.iter().enumerate() {
            let row = &all[i * m.vocab..(i + 1) * m.vocab];
            score += logprob_at(row, s);
        }
        scored.push((cand, score));
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
}

/// Walk backward `n_back` steps from `suffix`: each step asks `candidates_for`
/// for a shortlist (full vocab is exact and slow; a shortlist is the
/// experiment's tuning knob), takes the best predecessor, and prepends it —
/// the growing sequence is re-scored whole each step, so later steps are
/// conditioned on the reconstruction so far.
pub fn reverse_generate(
    gpu: &Gpu,
    m: &Model,
    suffix: &[u32],
    n_back: usize,
    cache_cap: usize,
    mut candidates_for: impl FnMut(&[u32]) -> Vec<u32>,
) -> Vec<u32> {
    let mut seq: Vec<u32> = suffix.to_vec();
    for _ in 0..n_back {
        let cands = candidates_for(&seq);
        if cands.is_empty() {
            break;
        }
        let ranked = reverse_step(gpu, m, &[], &cands, &seq, cache_cap);
        let Some(&(best, _)) = ranked.first() else { break };
        seq.insert(0, best);
    }
    seq
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(gpu: &Gpu) -> Option<Model> {
        let bytes = crate::testfix::read("/tmp/tiny-qwen2.gguf")?;
        gpu.load_gguf_opt(&bytes, false).ok()
    }

    /// The contract, against an independent recomputation: reverse_step's
    /// score for each candidate equals the sum of per-position logprobs
    /// computed from verify_forward's rows by THIS TEST, not by the code
    /// under test — same formula, separate arithmetic path.
    #[test]
    fn scores_are_the_forward_likelihood_of_the_suffix() {
        let gpu = Gpu::new();
        let m = match model(&gpu) { Some(m) => m, None => return };
        let prefix: Vec<u32> = (0..8u32).map(|i| 1 + (i * 5) % 90).collect();
        let suffix: Vec<u32> = vec![17, 42, 3];
        let cands: Vec<u32> = vec![7, 21, 55, 80];
        let ranked = reverse_step(&gpu, &m, &prefix, &cands, &suffix, 64);
        assert_eq!(ranked.len(), cands.len());
        for &(cand, score) in &ranked {
            let cache = gpu.new_cache(m.n_layers, 64, m.kv_dim());
            let _ = gpu.prefill_chunked(&m, &cache, &prefix, 4);
            let mut seq = vec![cand];
            seq.extend_from_slice(&suffix);
            let all = gpu.verify_forward(&m, &cache, &seq, prefix.len());
            let mut want = 0.0f32;
            for (i, &s) in suffix.iter().enumerate() {
                want += logprob_at(&all[i * m.vocab..(i + 1) * m.vocab], s);
            }
            assert!((score - want).abs() < 1e-3,
                "cand {cand}: reverse_step said {score}, independent recompute {want}");
        }
        // best-first ordering holds
        for w in ranked.windows(2) {
            assert!(w[0].1 >= w[1].1);
        }
    }

    /// Determinism: the experiment is only steerable if the same question
    /// gives the same answer.
    #[test]
    fn reverse_is_deterministic() {
        let gpu = Gpu::new();
        let m = match model(&gpu) { Some(m) => m, None => return };
        let suffix = vec![5u32, 9, 12];
        let cands = vec![3u32, 14, 60];
        let a = reverse_step(&gpu, &m, &[], &cands, &suffix, 64);
        let b = reverse_step(&gpu, &m, &[], &cands, &suffix, 64);
        assert_eq!(a, b);
    }

    /// reverse_generate grows leftward and conditions each step on the
    /// reconstruction so far: two steps from a 2-token suffix give a 4-token
    /// sequence ending in the original suffix, deterministically.
    #[test]
    fn walking_back_prepends_and_preserves_the_suffix() {
        let gpu = Gpu::new();
        let m = match model(&gpu) { Some(m) => m, None => return };
        let suffix = vec![11u32, 29];
        let shortlist = vec![2u32, 8, 33, 71];
        let out = reverse_generate(&gpu, &m, &suffix, 2, 64, |_| shortlist.clone());
        assert_eq!(out.len(), 4);
        assert_eq!(&out[2..], &suffix[..], "the anchor must survive the walk");
        let again = reverse_generate(&gpu, &m, &suffix, 2, 64, |_| shortlist.clone());
        assert_eq!(out, again);
    }
}
