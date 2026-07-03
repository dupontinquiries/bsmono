//! specdecode.rs — speculative decoding: the pure verify/accept core that turns a draft model's guesses
//! into committed tokens, plus throughput accounting and an adaptive speculation length.
//!
//! Speculative decoding raises tokens/sec by letting a small DRAFT model propose `k` tokens that the big
//! TARGET model verifies in ONE batched forward pass: every accepted guess is a token the target didn't
//! have to generate serially, so a single target pass can emit up to `k+1` tokens instead of one. The two
//! model forward passes are the GPU seam (the engine runs the draft and the batched target verify); the
//! accept rule, the bookkeeping, and the adaptive `k` are pure and unit-tested here.
#![allow(dead_code)]

/// Greedy speculative-decoding verification — lossless for greedy/argmax decoding.
///
/// * `draft`  — the `k` tokens the draft model proposed.
/// * `target` — the target model's own argmax token at each of the `k+1` verification positions, where
///   `target[i]` is what the target emits given the true prefix plus `draft[..i]`.
///
/// Accepts the longest matching prefix of `draft`, then appends exactly one target token: the correction
/// at the first mismatch, or the bonus token `target[k]` when every draft token matched. The result is
/// always `1..=k+1` tokens and is exactly what the target would have produced serially — `>1` is the
/// speedup over batch-1 decode.
pub fn verify_greedy(draft: &[u32], target: &[u32]) -> Vec<u32> {
    assert_eq!(target.len(), draft.len() + 1, "target needs one argmax per draft position plus the bonus");
    let mut out = Vec::with_capacity(target.len());
    let mut i = 0;
    while i < draft.len() && draft[i] == target[i] { out.push(draft[i]); i += 1; }
    out.push(target[i]); // correction at the first mismatch, or the bonus token if i == draft.len()
    out
}

/// One verification position for *sampled* speculative decoding: the draft token and the draft (`q`) and
/// target (`p`) distributions at that position, over the same vocab indexing (each summing ~1).
pub struct DraftPos<'a> { pub token: u32, pub q: &'a [f32], pub p: &'a [f32] }

/// Lossless sampled speculative verification (the Leviathan/Chen rule) — the temperature>0 counterpart of
/// [`verify_greedy`]. For each position accept the draft token with probability `min(1, p(x)/q(x))`; on the
/// first rejection emit a token resampled from the normalized residual `(p - q)₊` and stop. If every draft
/// token is accepted, emit a bonus token sampled from `bonus` (the target distribution at position k+1).
/// The committed tokens are distributed exactly as if sampled from the target directly. `rand` supplies the
/// uniform draws (injected for determinism): one per accept test, plus one per (re)sample.
pub fn verify_sampled(positions: &[DraftPos], bonus: &[f32], rand: &mut dyn FnMut() -> f32) -> Vec<u32> {
    let mut out = Vec::new();
    for pos in positions {
        let x = pos.token as usize;
        let qx = pos.q.get(x).copied().unwrap_or(0.0);
        let px = pos.p.get(x).copied().unwrap_or(0.0);
        let accept = if qx <= 0.0 { 1.0 } else { (px / qx).min(1.0) };
        if rand() < accept {
            out.push(pos.token);
        } else {
            out.push(sample_residual(pos.p, pos.q, rand())); // resample from (p−q)₊
            return out;
        }
    }
    out.push(sample_categorical(bonus, rand())); // all accepted → bonus token from p_{k+1}
    out
}

/// Inverse-CDF categorical sample from an unnormalized weight slice, using a uniform `u` in [0,1).
fn sample_categorical(dist: &[f32], u: f32) -> u32 {
    let sum: f32 = dist.iter().map(|w| w.max(0.0)).sum();
    if sum <= 0.0 { return 0; }
    let target = u * sum;
    let mut acc = 0.0;
    for (i, &w) in dist.iter().enumerate() { acc += w.max(0.0); if acc >= target { return i as u32; } }
    (dist.len().saturating_sub(1)) as u32
}

/// Sample from the normalized residual `(p − q)₊`. Falls back to `p` if the residual is degenerate.
fn sample_residual(p: &[f32], q: &[f32], u: f32) -> u32 {
    let n = p.len().max(q.len());
    let resid = |i: usize| (p.get(i).copied().unwrap_or(0.0) - q.get(i).copied().unwrap_or(0.0)).max(0.0);
    let sum: f32 = (0..n).map(resid).sum();
    if sum <= 0.0 { return sample_categorical(p, u); }
    let target = u * sum;
    let mut acc = 0.0;
    for i in 0..n { acc += resid(i); if acc >= target { return i as u32; } }
    (n.saturating_sub(1)) as u32
}

/// Running throughput counters for a speculative-decoding session.
#[derive(Clone, Debug, Default)]
pub struct SpecStats {
    pub forward_passes: u64, // target forward passes (one per verified draft block)
    pub committed: u64,      // tokens actually emitted
    pub proposed: u64,       // draft tokens proposed
    pub accepted: u64,       // draft tokens accepted (committed minus the one target token per pass)
}
impl SpecStats {
    /// Record one verification step from the proposed count and the length `verify_greedy` returned.
    pub fn record(&mut self, proposed: usize, committed_len: usize) {
        self.forward_passes += 1;
        self.proposed += proposed as u64;
        self.committed += committed_len as u64;
        self.accepted += committed_len.saturating_sub(1) as u64; // committed = accepted_draft + 1 target token
    }
    /// Fraction of proposed draft tokens accepted (0..=1). High ⇒ the draft tracks the target well.
    pub fn acceptance_rate(&self) -> f32 {
        if self.proposed == 0 { 0.0 } else { self.accepted as f32 / self.proposed as f32 }
    }
    /// Tokens committed per target forward pass — the throughput multiplier vs. serial decode (which is 1.0).
    pub fn tokens_per_pass(&self) -> f32 {
        if self.forward_passes == 0 { 0.0 } else { self.committed as f32 / self.forward_passes as f32 }
    }
}

/// Adapts the speculation length `k` from recent acceptance: propose more when the draft is hitting,
/// fewer when it's missing (a long draft that gets rejected wastes target compute).
#[derive(Clone, Copy, Debug)]
pub struct DraftPolicy { pub min: usize, pub max: usize, pub grow_above: f32, pub shrink_below: f32 }
impl Default for DraftPolicy {
    fn default() -> Self { Self { min: 1, max: 8, grow_above: 0.75, shrink_below: 0.40 } }
}
impl DraftPolicy {
    pub fn next_len(&self, current: usize, accepted: usize, proposed: usize) -> usize {
        let rate = if proposed == 0 { 0.0 } else { accepted as f32 / proposed as f32 };
        let next = if rate >= self.grow_above { current + 1 }
                   else if rate <= self.shrink_below { current.saturating_sub(1) }
                   else { current };
        next.clamp(self.min.max(1), self.max.max(self.min.max(1)))
    }
}

/// Engine-facing driver: ask [`draft_len`](Self::draft_len) for how many tokens to propose, run the draft
/// model, run the batched target verify, then [`commit`](Self::commit) the result — which records stats
/// and adapts `k`. Deterministic; the model passes around it are the seam.
pub struct SpecDecoder { k: usize, pub policy: DraftPolicy, pub stats: SpecStats }
impl SpecDecoder {
    pub fn new(policy: DraftPolicy) -> Self { Self { k: policy.min.max(1), policy, stats: SpecStats::default() } }
    /// How many tokens the draft model should propose next.
    pub fn draft_len(&self) -> usize { self.k }
    /// Verify one draft block against the target's argmaxes, record throughput, adapt `k`, return the
    /// committed tokens (accepted draft prefix + one target token).
    pub fn commit(&mut self, draft: &[u32], target_argmax: &[u32]) -> Vec<u32> {
        let committed = verify_greedy(draft, target_argmax);
        self.stats.record(draft.len(), committed.len());
        self.k = self.policy.next_len(self.k, committed.len() - 1, draft.len());
        committed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_full_draft_plus_bonus_token() {
        // every guess right ⇒ k+1 tokens from one target pass (the max speedup)
        assert_eq!(verify_greedy(&[5, 6, 7], &[5, 6, 7, 8]), vec![5, 6, 7, 8]);
    }

    #[test]
    fn stops_at_first_mismatch_with_correction() {
        // draft diverges at index 1 → keep the 5, replace with the target's 6, stop
        assert_eq!(verify_greedy(&[5, 9, 7], &[5, 6, 7, 8]), vec![5, 6]);
        // mismatch at index 0 → just the target's first token
        assert_eq!(verify_greedy(&[1, 2], &[5, 6, 7]), vec![5]);
    }

    #[test]
    fn empty_draft_degrades_to_one_token() {
        assert_eq!(verify_greedy(&[], &[5]), vec![5]); // == ordinary batch-1 decode
    }

    #[test]
    fn sampled_accepts_when_target_likes_the_draft_token() {
        // p(x)/q(x) = 0.9/0.5 = 1.8 → accept for any draw; then bonus from p_{k+1}=[0,1] → token 1
        let pos = [DraftPos { token: 0, q: &[0.5, 0.5], p: &[0.9, 0.1] }];
        let mut draws = [0.99f32, 0.5].into_iter();
        let out = verify_sampled(&pos, &[0.0, 1.0], &mut || draws.next().unwrap());
        assert_eq!(out, vec![0, 1]);
    }

    #[test]
    fn sampled_rejects_and_resamples_from_residual() {
        // draft picked token 1 but target dislikes it: ratio = 0.1/0.9 ≈ 0.11; draw 0.5 > ratio → reject.
        // residual (p−q)₊ = (0.8, 0) → resample must yield token 0.
        let pos = [DraftPos { token: 1, q: &[0.1, 0.9], p: &[0.9, 0.1] }];
        let mut draws = [0.5f32, 0.5].into_iter();
        let out = verify_sampled(&pos, &[1.0, 0.0], &mut || draws.next().unwrap());
        assert_eq!(out, vec![0], "rejection stops the block and emits the corrected token");
    }

    #[test]
    fn categorical_inverse_cdf_picks_the_right_bucket() {
        let d = [0.2f32, 0.3, 0.5];
        assert_eq!(sample_categorical(&d, 0.0), 0);
        assert_eq!(sample_categorical(&d, 0.25), 1); // 0.2 < 0.25 ≤ 0.5
        assert_eq!(sample_categorical(&d, 0.60), 2); // 0.5 < 0.60 ≤ 1.0
    }

    #[test]
    fn stats_track_acceptance_and_throughput() {
        let mut s = SpecStats::default();
        s.record(3, 4); // proposed 3, all accepted + bonus ⇒ committed 4
        s.record(3, 2); // proposed 3, accepted 1 + correction ⇒ committed 2
        assert_eq!(s.accepted, 4);          // 3 + 1
        assert_eq!(s.proposed, 6);
        assert!((s.acceptance_rate() - 4.0 / 6.0).abs() < 1e-6);
        assert!((s.tokens_per_pass() - 3.0).abs() < 1e-6); // 6 committed / 2 passes
    }

    #[test]
    fn policy_grows_when_hitting_and_shrinks_when_missing() {
        let p = DraftPolicy::default();
        assert_eq!(p.next_len(4, 4, 4), 5); // 100% accepted → grow
        assert_eq!(p.next_len(4, 1, 4), 3); // 25% accepted → shrink
        assert_eq!(p.next_len(4, 2, 4), 4); // 50% → hold
        assert_eq!(p.next_len(8, 8, 8), 8); // already at max → clamp
        assert_eq!(p.next_len(1, 0, 4), 1); // already at min → clamp
    }

    #[test]
    fn decoder_drives_and_adapts() {
        let mut d = SpecDecoder::new(DraftPolicy { min: 1, max: 6, grow_above: 0.75, shrink_below: 0.4 });
        d.k = 3;
        let committed = d.commit(&[10, 11, 12], &[10, 11, 12, 13]); // perfect block
        assert_eq!(committed, vec![10, 11, 12, 13]);
        assert_eq!(d.draft_len(), 4, "all accepted → propose one more next time");
        assert!((d.stats.tokens_per_pass() - 4.0).abs() < 1e-6);
    }
}
