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
pub fn next_draft_len(last_drafted: usize, last_accepted: usize, max_k: usize, cost: DraftCost) -> usize {
    let max_k = max_k.max(1);
    if last_drafted == 0 { return 2.min(max_k); }
    if last_accepted == last_drafted {
        // Everything matched: the draft is tracking, so reach further.
        (last_drafted + 1).min(max_k)
    } else if last_accepted == 0 {
        // Nothing matched. Back off hard rather than gradually — a draft that is wrong at position 0
        // is wrong about the current context, not about its length, and repeating a long wrong draft
        // is the one way speculation costs more than it saves.
        //
        // Except when the draft was free. A rejected prompt-lookup proposal cost nothing but a few
        // wider rows in a verify that had to run regardless, so collapsing to k=1 gives up the next
        // round's upside to save an expense that was never incurred. Prompt-lookup misses constantly
        // on prose and hits in runs on code; a hard reset makes it miss the start of every run.
        if cost == DraftCost::Free { (last_drafted.saturating_sub(1)).max(2).min(max_k) } else { 1 }
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
        assert_eq!(next_draft_len(4, 4, 8, DraftCost::Model { step_ratio: 0.25 }), 5, "tracking: reach further");
        assert_eq!(next_draft_len(4, 2, 8, DraftCost::Model { step_ratio: 0.25 }), 3, "partial: aim just past what landed");
        assert_eq!(next_draft_len(4, 0, 8, DraftCost::Model { step_ratio: 0.25 }), 1, "total miss: back off hard, not gradually");
    }

    #[test]
    fn a_FREE_drafter_does_not_collapse_on_a_miss() {
        // The hard reset exists to stop wasting draft-MODEL decode steps. A rejected prompt-lookup
        // proposal wasted nothing — the verify had to run anyway and the extra rows are free — so
        // collapsing to 1 gives up the next round for an expense that was never paid. Prompt-lookup
        // hits in runs on code and misses on prose; resetting hard makes it miss the start of every run.
        assert_eq!(next_draft_len(4, 0, 8, DraftCost::Model { step_ratio: 0.25 }), 1);
        assert_eq!(next_draft_len(4, 0, 8, DraftCost::Free), 3, "step down, do not collapse");
        assert_eq!(next_draft_len(2, 0, 8, DraftCost::Free), 2, "and never below a floor of 2");
    }

    #[test]
    fn the_draft_cap_is_one_BELOW_the_kernel_row_limit() {
        // A draft of k verifies k+1 rows: the drafted tokens plus the free correction. Passing
        // KERNEL_MAX_ROWS straight through as max_k puts the verify on the strided path.
        assert_eq!(max_draft_for_kernel(8), 7);
        assert_eq!(max_draft_for_kernel(1), 1);
        let k = max_draft_for_kernel(crate::batchplan::KERNEL_MAX_ROWS);
        assert!(k + 1 <= crate::batchplan::KERNEL_MAX_ROWS, "k={k} verifies {} rows", k + 1);
    }

    #[test]
    fn the_draft_never_exceeds_the_BATCHED_KERNEL_limit() {
        // Above m=8 the q4 path falls to the strided LINEAR_Q4 that re-reads weights per tile, so
        // drafting further moves the work onto a kernel that undoes the saving.
        for accepted in 0..12 {
            assert!(next_draft_len(12, accepted, 8, DraftCost::Model { step_ratio: 0.25 }) <= 8, "accepted {accepted}");
            assert!(next_draft_len(12, accepted, 8, DraftCost::Free) <= 8, "free, accepted {accepted}");
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

/// Whether speculation helps, given what a run is actually bound by.
///
/// # The non-obvious part
///
/// Speculation is usually described as free when it works: the target verifies k drafted tokens in
/// ONE batched pass, so the weights are read once for k tokens instead of k times. `verify_forward`
/// does exactly that — `block_prefill_ext` over all rows — so on a **bandwidth-bound** decode the
/// verification is nearly free and the win is close to the acceptance rate.
///
/// **On a submit-bound decode it is the opposite.** Each draft step is a full decode on the draft
/// model, with its own dispatches and its own pass boundaries. Drafting k tokens adds k draft
/// decodes' worth of submits to buy one batched verify. If submits are what costs, speculation
/// makes the run SLOWER while every acceptance-rate number looks healthy.
///
/// Scout's measured shape — 71% no-frame, 913 submits per token — is the second case. That is worth
/// knowing before loading a draft model and concluding the feature does not work.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Bound { Bandwidth, Submits, Latency, Unknown }

/// What a draft token costs, relative to a target decode step.
///
/// The whole policy below used to assume a draft MODEL, and every decision it makes comes out
/// differently for the model-free drafter this engine ships and recommends. A prompt-lookup draft
/// is a table probe on the CPU: it adds no dispatches, no submits and no GPU work, so k drafted
/// tokens cost the same as zero.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DraftCost {
    /// Prompt-lookup or any other table drafter. One verify pass per round and nothing else.
    Free,
    /// A second checkpoint. `step_ratio` is a draft decode step over a target decode step.
    Model { step_ratio: f64 },
}

/// Largest draft length that keeps the verify batch on the batched matvec.
///
/// A draft of `k` verifies `k + 1` rows — the drafted tokens plus the correction the batched pass
/// yields free. So the cap is one BELOW the kernel's row limit, and passing `KERNEL_MAX_ROWS`
/// straight through as `max_k` puts the verify on the strided path that re-reads weights per tile,
/// which is the exact cost speculation was meant to avoid.
pub fn max_draft_for_kernel(kernel_max_rows: usize) -> usize {
    kernel_max_rows.saturating_sub(1).max(1)
}

/// Classify from a run's own numbers.
/// `roofline_frac` is achieved bandwidth over the device's spec figure, which `roofline.rs` already
/// computes. Without it, high sync time reads as memory pressure — but sync is also where a driver
/// with slow completion callbacks spends its life, and those two want opposite fixes. A measured run
/// at 3 submits/token, 93% sync and 9% of roofline was classified `Bandwidth` and told that
/// "verification reads the weights ONCE for k tokens", on a device the engine's own roofline line
/// was simultaneously calling "mostly idle".
pub fn what_binds(submits_per_token: f64, sync_share: f64, roofline_frac: Option<f64>) -> Bound {
    // THRESHOLD FROM THE MEASUREMENTS, not from a round number. The dense 1B runs about 4 submits
    // per token; Scout runs 913 and, on a longer conversation, 13321. That is two orders of
    // magnitude of separation, so anything above 100 is unambiguous — my first guess was 1000, which
    // put Scout's own 913 in the "unclear" bucket and made the classifier useless for the exact case
    // it was written for.
    if submits_per_token > 100.0 { return Bound::Submits }
    // High sync with few submits is waiting on SOMETHING, and the roofline says which. Under a
    // fifth of spec bandwidth the memory system is idle and the wait is the driver's; near it, the
    // wait is memory.
    if sync_share > 0.5 {
        return match roofline_frac {
            Some(f) if f < 0.2 => Bound::Latency,
            Some(_) => Bound::Bandwidth,
            None => Bound::Unknown,
        };
    }
    Bound::Unknown
}

/// Expected speedup, given what binds and what the drafter costs.
///
/// A round emits `1 + accept * k` tokens. What it COSTS depends entirely on the drafter: a free one
/// pays a single verify pass, the same submits and dispatches a plain decode step would have spent
/// on one token. A draft model pays `k` of its own decode steps on top.
pub fn expected_speedup(bound: Bound, accept_rate: f64, k: usize, cost: DraftCost) -> f64 {
    let tokens = 1.0 + accept_rate.clamp(0.0, 1.0) * k as f64;
    let rounds_cost = match cost {
        DraftCost::Free => 1.0,
        DraftCost::Model { step_ratio } => 1.0 + step_ratio.max(0.0) * k as f64,
    };
    match bound {
        // Both are per-round-fixed costs that a wider verify does not multiply: the weights are read
        // once either way, and the submits are the same count either way.
        Bound::Bandwidth | Bound::Latency => tokens / rounds_cost,
        // Draft steps are whole extra decodes here, each with its own submits.
        Bound::Submits => tokens / rounds_cost,
        Bound::Unknown => 1.0,
    }
}

/// What speculation is worth here.
pub fn speculation_outlook(bound: Bound, accept_rate: f64, k: usize, cost: DraftCost) -> String {
    let x = expected_speedup(bound, accept_rate, k, cost);
    let free = cost == DraftCost::Free;
    match bound {
        Bound::Bandwidth => format!(
            "bandwidth-bound: verification reads the weights ONCE for {k} tokens, so at a {:.0}% \
acceptance rate the win is close to {x:.1}x. This is the case speculation was designed for",
            accept_rate * 100.0),
        Bound::Latency => format!(
            "latency-bound: the wait is the driver's completion callback, not memory — the roofline \
says the hardware is idle. A verify over {k}+1 rows costs the SAME submits as a one-token decode, so \
at a {:.0}% acceptance rate the win is close to {x:.1}x for the same reason as the bandwidth case, by \
a different mechanism. Widening the verify is free here; the ceiling is the batched matvec's row limit",
            accept_rate * 100.0),
        Bound::Submits if free => format!(
            "submit-bound, but the drafter is FREE: a table probe adds no dispatches and no submits, \
so the round still costs one verify. Expect about {x:.1}x. The warning below applies to a draft \
MODEL and not to this drafter — declining to speculate here would be leaving the win on the table"),
        Bound::Submits => format!(
            "submit-bound: each of the {k} draft steps is a full decode on the draft model with its \
own dispatches, and they buy ONE batched verify. Speculation adds submits to a run that is already \
losing to them — expect it to be SLOWER while the acceptance rate looks healthy. Fix the submit \
path first, or use the model-free drafter, which costs no submits at all"),
        Bound::Unknown =>
            "neither submits nor sync dominates clearly, so speculation's effect is not predictable \
from these numbers — measure it rather than reasoning about it".into(),
    }
}

#[cfg(test)]
mod outlook_tests {
    use super::*;

    #[test]
    fn SCOUT_s_measured_shape_is_submit_bound_and_speculation_would_HURT() {
        // 913 submits per token, 71% no-frame. Loading a draft model here and concluding the feature
        // does not work would be the wrong lesson from a real slowdown.
        assert_eq!(what_binds(913.0, 1.12, None), Bound::Submits);
        let s = speculation_outlook(Bound::Submits, 0.7, 4, DraftCost::Model { step_ratio: 0.25 });
        assert!(s.contains("SLOWER while the acceptance rate looks healthy"));
        assert!(s.contains("Fix the submit path first"));
    }

    #[test]
    fn a_submit_bound_run_with_a_FREE_drafter_is_not_told_to_expect_a_slowdown() {
        // Same device, different drafter, opposite advice. Telling the operator to expect a slowdown
        // when the recommended drafter costs no submits at all is how a real win gets declined.
        let s = speculation_outlook(Bound::Submits, 0.7, 4, DraftCost::Free);
        assert!(!s.contains("expect it to be SLOWER"), "{s}");
        assert!(s.contains("FREE"), "{s}");
        assert!(expected_speedup(Bound::Submits, 0.7, 4, DraftCost::Free)
              > expected_speedup(Bound::Submits, 0.7, 4, DraftCost::Model { step_ratio: 0.25 }));
    }

    #[test]
    fn high_sync_at_NINE_PERCENT_of_roofline_is_latency_not_bandwidth() {
        // The measured Asahi run: 3 submits/token, 93% sync, 9% of spec bandwidth. The two-argument
        // classifier called this Bandwidth and told the operator that verification "reads the weights
        // ONCE for k tokens" — on a device its own roofline line was calling mostly idle.
        assert_eq!(what_binds(3.0, 0.926, Some(0.09)), Bound::Latency);
        assert_eq!(what_binds(3.0, 0.926, Some(0.75)), Bound::Bandwidth);
        assert_eq!(what_binds(3.0, 0.926, None), Bound::Unknown, "no roofline, no verdict");
        let s = speculation_outlook(Bound::Latency, 0.6, 4, DraftCost::Free);
        assert!(s.contains("3.4x"), "{s}");
        assert!(s.contains("driver's completion callback"), "{s}");
    }

    #[test]
    fn a_BANDWIDTH_bound_run_gets_the_win_speculation_is_designed_for() {
        // Weights read once for k tokens, so the verify is nearly free and the win tracks acceptance.
        assert_eq!(what_binds(4.0, 0.74, Some(0.62)), Bound::Bandwidth);
        // 1 + 0.75*4 = 4.0. My first assertion said 3.0x, computed in my head rather than from the
        // formula in the function — the same error as the 97%-versus-98% one two commits ago.
        assert!(speculation_outlook(Bound::Bandwidth, 0.75, 4, DraftCost::Free).contains("4.0x"));
    }

    #[test]
    fn an_UNCLEAR_run_is_told_to_MEASURE_rather_than_given_a_number() {
        // A prediction from numbers that do not support one is worse than no prediction, because it
        // gets acted on.
        assert_eq!(what_binds(50.0, 0.3, Some(0.4)), Bound::Unknown);
        assert!(speculation_outlook(Bound::Unknown, 0.7, 4, DraftCost::Free).contains("measure it rather than"));
    }

    #[test]
    fn the_submit_verdict_does_not_depend_on_the_ACCEPTANCE_RATE() {
        // The trap this exists to name: acceptance can be excellent and the run still slower,
        // because the cost is in the draft's submits rather than in wasted tokens.
        for r in [0.3, 0.9] {
            assert!(speculation_outlook(Bound::Submits, r, 4, DraftCost::Model { step_ratio: 0.25 })
                .contains("SLOWER"));
        }
    }
}
