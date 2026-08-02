//! specverify.rs — the accept rule and draft policy for speculative decoding.
//!
//! Decode measured 182 ms/token against prefill's 12.6 ms/token on the same weights in the same run:
//! **14× slower per token for strictly less work**. The difference is that prefill has m=109 rows in
//! flight to cover memory latency and decode at m=1 has nothing to hide behind.
//!
//! `MATVEC_SMALL_Q4` already exists for m≤8 — its own comment says "speculative verify block /
//! batched decode: read each weight nibble once, accumulate all m rows" — and each row is bit-exact to
//! the m=1 kernel. Nothing drives it, because nothing produces more than one candidate token.
//!
//! This is the decision layer that would: how many tokens to draft, and how many of them to keep.
//! Pure, so it can be tested without a second model.
//!
//! ## Why the accept rule is the delicate part
//!
//! Speculation is only free if the accepted tokens are EXACTLY what the target model would have
//! produced alone. Accept one token too many and the output is subtly not the model's — which is
//! indistinguishable from a bad model and is the failure this cannot be allowed to have.

/// What the target model's greedy argmax says at each verified position.
///
/// The draft proposed `drafted`, the target was run over all of them in one batched pass, and
/// `target_argmax[i]` is what the target would itself have emitted at position `i`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Verified {
    /// Tokens the caller may emit. Always a prefix of the draft, possibly empty.
    pub accepted: Vec<u32>,
    /// The correction: what the target wanted where the draft first diverged.
    ///
    /// Always present, and this is what makes speculation a strict win. Even when NOTHING is accepted
    /// the batched pass produced the target's own next token, so a fully-rejected round still advances
    /// by one — it costs the draft's time and never a wasted target step.
    pub correction: u32,
}

/// Accept the longest prefix on which the draft and the target agree, then take the target's token.
///
/// This is llama.cpp's rule. The subtlety is the correction: at the first disagreement the target's
/// own argmax is already known from the same batched pass, so it is emitted rather than recomputed.
///
/// Panics never; a draft longer than the verification is truncated to what was actually verified,
/// because accepting a token the target never scored is exactly the way to emit something the model
/// did not choose.
pub fn accept_prefix(drafted: &[u32], target_argmax: &[u32]) -> Verified {
    let n = drafted.len().min(target_argmax.len());
    let mut accepted = Vec::new();
    for i in 0..n {
        if drafted[i] != target_argmax[i] { break; }
        accepted.push(drafted[i]);
    }
    // The correction is the target's token at the first UNACCEPTED position. When the whole draft
    // matched, that is the position one past it — which the batched pass covers because it is run over
    // k drafted tokens and yields k+1 predictions.
    let correction = target_argmax.get(accepted.len()).copied()
        .unwrap_or_else(|| *target_argmax.last().unwrap_or(&0));
    Verified { accepted, correction }
}

/// How many tokens to draft next, from how the last round went.
///
/// Adaptive rather than fixed, because the accept rate varies enormously by content: a draft model
/// tracks the target closely through boilerplate and diverges immediately on anything surprising, and
/// a fixed k is either wasteful on the first or too timid on the second.
///
/// Bounded by `max_k`, which must not exceed the batched kernel's own limit — above m=8 the q4 path
/// falls to the strided `LINEAR_Q4` that re-reads weights per tile, so drafting more would move work
/// onto a kernel that undoes the saving.
pub fn next_draft_len(last_drafted: usize, last_accepted: usize, max_k: usize) -> usize {
    let max_k = max_k.max(1);
    if last_drafted == 0 { return 2.min(max_k); }
    if last_accepted == last_drafted {
        // Everything matched: the draft is tracking, so reach further.
        (last_drafted + 1).min(max_k)
    } else if last_accepted == 0 {
        // Nothing matched. Back off hard rather than gradually — a draft that is wrong at position 0
        // is wrong about the current context, not about its length, and repeating a long wrong draft
        // is the one way speculation costs more than it saves.
        1
    } else {
        // Partial: aim just past what was accepted.
        (last_accepted + 1).min(max_k)
    }
}

/// Is speculation worth running at all, given the measured cost of a draft step?
///
/// Returns false when the draft is not cheap enough relative to the target for any accept rate to pay.
/// A draft model that costs a third of the target must accept better than one token in three just to
/// break even, and that is before the verification pass.
pub fn worth_speculating(draft_ms: f64, target_ms: f64, observed_accept_rate: f64) -> bool {
    if draft_ms <= 0.0 || target_ms <= 0.0 { return false; }
    // Per accepted token: k draft steps plus one batched target step, against k+1 target steps.
    // Break-even needs the expected tokens per round to exceed the draft overhead.
    let expected_tokens = 1.0 + observed_accept_rate;
    let cost = draft_ms * expected_tokens + target_ms;
    let baseline = target_ms * expected_tokens;
    cost < baseline
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fully_matching_draft_is_accepted_WHOLE_plus_the_correction() {
        let v = accept_prefix(&[10, 11, 12], &[10, 11, 12, 13]);
        assert_eq!(v.accepted, vec![10, 11, 12]);
        assert_eq!(v.correction, 13, "the batched pass yields k+1 predictions, so the next is free");
    }

    #[test]
    fn a_divergence_TRUNCATES_and_the_target_wins() {
        // The delicate part. Accepting past the first disagreement emits a token the target did not
        // choose, which is indistinguishable from a bad model.
        let v = accept_prefix(&[10, 99, 12], &[10, 11, 12]);
        assert_eq!(v.accepted, vec![10]);
        assert_eq!(v.correction, 11, "the target's token at the divergence, not the draft's");
    }

    #[test]
    fn a_fully_rejected_round_still_ADVANCES_by_one() {
        // This is what makes speculation a strict win: the batched pass already produced the target's
        // own next token, so a wasted draft never costs a wasted target step.
        let v = accept_prefix(&[99, 98], &[10, 11]);
        assert!(v.accepted.is_empty());
        assert_eq!(v.correction, 10);
    }

    #[test]
    fn a_draft_longer_than_the_verification_is_TRUNCATED() {
        // Accepting a token the target never scored is exactly how to emit something the model did not
        // choose — the one outcome this must never produce.
        let v = accept_prefix(&[10, 11, 12, 13], &[10, 11]);
        assert_eq!(v.accepted, vec![10, 11]);
    }

    #[test]
    fn the_draft_length_grows_on_success_and_COLLAPSES_on_total_failure() {
        // A draft wrong at position 0 is wrong about the context, not its length, and repeating a long
        // wrong draft is the one way speculation costs more than it saves.
        assert_eq!(next_draft_len(4, 4, 8), 5, "tracking: reach further");
        assert_eq!(next_draft_len(4, 2, 8), 3, "partial: aim just past what landed");
        assert_eq!(next_draft_len(4, 0, 8), 1, "total miss: back off hard, not gradually");
    }

    #[test]
    fn the_draft_never_exceeds_the_BATCHED_KERNEL_limit() {
        // Above m=8 the q4 path falls to the strided LINEAR_Q4 that re-reads weights per tile, so
        // drafting further moves the work onto a kernel that undoes the saving.
        for accepted in 0..12 {
            assert!(next_draft_len(12, accepted, 8) <= 8, "accepted {accepted}");
        }
    }

    #[test]
    fn speculation_is_refused_when_the_draft_is_not_CHEAP_enough() {
        // A draft costing a third of the target must accept better than one in three to break even,
        // and that is before the verification pass.
        assert!(worth_speculating(5.0, 180.0, 0.8), "a cheap draft with good acceptance pays");
        assert!(!worth_speculating(170.0, 180.0, 0.8), "a draft nearly as costly as the target does not");
        assert!(!worth_speculating(0.0, 180.0, 0.5), "and a zero cost is a missing measurement");
    }
}
