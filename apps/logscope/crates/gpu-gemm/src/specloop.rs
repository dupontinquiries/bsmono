//! specloop.rs — sequences draft → verify → emit, so a decode loop integrates with two calls.
//!
//! `lookupdraft` proposes tokens, `specverify` decides which survive, and `MATVEC_SMALL_Q4` runs the
//! batched pass. Sequencing them at the call site means the adaptive draft length, the accept rule and
//! the context bookkeeping all live inside a generation loop that is already the hardest code in the
//! engine to change safely.
//!
//! So the sequencing lives here and the loop calls two methods.
//!
//! ## The shape
//!
//! ```text
//! let d = spec.propose(&context);        // tokens to draft, possibly empty
//! // run the target over context + d, one batched pass, yielding d.len()+1 argmaxes
//! let out = spec.settle(&d, &argmaxes);  // tokens to emit
//! ```
//!
//! When `propose` returns empty the caller decodes one token normally — speculation is an
//! optimisation, never a requirement, and a loop that cannot fall back to plain decoding has made a
//! performance feature into a correctness dependency.
//!
//! ## What it does not do
//!
//! It does not run the model. Keeping inference out means the whole policy is testable against
//! hand-written argmaxes, which is how the accept rule can be checked at all — the one part that must
//! never be wrong, because accepting a token the target did not choose emits something the model would
//! not have said.

pub struct SpecLoop {
    /// Upper bound on a draft. Must not exceed the batched kernel's own limit.
    max_k: usize,
    last_drafted: usize,
    last_accepted: usize,
    /// Running totals, for deciding whether speculation is paying at all.
    pub drafted_total: usize,
    pub accepted_total: usize,
}

impl SpecLoop {
    /// `max_k` is clamped to 8: above that the q4 path leaves `MATVEC_SMALL_Q4` for the strided
    /// `LINEAR_Q4`, which re-reads weights per tile and undoes the saving speculation exists to get.
    pub fn new(max_k: usize) -> SpecLoop {
        SpecLoop { max_k: max_k.clamp(1, 8), last_drafted: 0, last_accepted: 0,
                   drafted_total: 0, accepted_total: 0 }
    }

    /// Tokens to draft this step. Empty means decode one token normally.
    pub fn propose(&mut self, context: &[u32]) -> Vec<u32> {
        let k = crate::specverify::next_draft_len(self.last_drafted, self.last_accepted, self.max_k);
        let d = crate::lookupdraft::draft(context, k);
        self.last_drafted = d.len();
        // NOT counted here. A draft that is proposed and never settled cost nothing and verified
        // nothing, so counting it would understate the accept rate by exactly the drafts the caller
        // chose to discard. The count belongs where the verification happens.
        d
    }

    /// Given the target's argmax at each drafted position (plus one past the end), what to emit.
    ///
    /// Returns the accepted prefix followed by the target's own correction — so this ALWAYS advances
    /// by at least one token, even when the draft was entirely wrong. A caller can therefore treat it
    /// as an unconditional replacement for a single decode step.
    pub fn settle(&mut self, drafted: &[u32], target_argmax: &[u32]) -> Vec<u32> {
        let v = crate::specverify::accept_prefix(drafted, target_argmax);
        self.last_accepted = v.accepted.len();
        self.drafted_total += drafted.len();
        self.accepted_total += v.accepted.len();
        let mut out = v.accepted;
        out.push(v.correction);
        out
    }

    /// Fraction of drafted tokens that survived. `None` before anything has been drafted.
    ///
    /// Worth surfacing rather than keeping internal: a rate near zero means lookup is finding nothing
    /// useful in this conversation, and the honest response is to stop drafting rather than to keep
    /// paying for it — even though the cost is small, a feature that never helps should say so.
    pub fn accept_rate(&self) -> Option<f64> {
        if self.drafted_total == 0 { return None; }
        Some(self.accepted_total as f64 / self.drafted_total as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matching_context_drafts_and_accepts() {
        let mut s = SpecLoop::new(4);
        let ctx = vec![1, 2, 3, 4, 5, 6, 9, 1, 2, 3];
        let d = s.propose(&ctx);
        assert_eq!(d, vec![4, 5], "the first draft is 2 tokens");
        // The target agrees with both, and wants 6 next.
        let out = s.settle(&d, &[4, 5, 6]);
        assert_eq!(out, vec![4, 5, 6], "two accepted plus the target's own next");
        assert_eq!(s.accept_rate(), Some(1.0));
    }

    #[test]
    fn settle_ALWAYS_advances_even_when_the_draft_is_entirely_wrong() {
        // This is what lets a caller use it as an unconditional replacement for one decode step. A
        // version that could return nothing would need the loop to handle two cases, and the second
        // case would be the untested one.
        let mut s = SpecLoop::new(4);
        let out = s.settle(&[99, 98], &[10, 11]);
        assert_eq!(out, vec![10], "nothing accepted, but the target's token is still emitted");
        // Counted at SETTLE, not at propose: a draft that is proposed and never verified cost
        // nothing, so counting it would understate the rate by exactly the drafts a caller discarded.
        assert_eq!(s.accept_rate(), Some(0.0), "two drafted, none accepted");
    }

    #[test]
    fn an_empty_draft_still_settles_to_one_token() {
        // propose() returning empty means "decode normally", but a caller that settles anyway must not
        // get an empty result — the fallback path and the speculative path should differ in speed, not
        // in whether they produce output.
        let mut s = SpecLoop::new(4);
        let out = s.settle(&[], &[42]);
        assert_eq!(out, vec![42]);
    }

    #[test]
    fn max_k_is_CLAMPED_to_the_batched_kernel_limit() {
        // Above 8 the q4 path leaves MATVEC_SMALL_Q4 for the strided LINEAR_Q4, which re-reads weights
        // per tile — drafting further moves the work onto a kernel that undoes the saving.
        let mut s = SpecLoop::new(64);
        let ctx: Vec<u32> = (0..40).chain(0..8).collect();
        for _ in 0..20 { let d = s.propose(&ctx); s.settle(&d, &d.iter().copied()
            .chain(std::iter::once(0)).collect::<Vec<_>>()); }
        assert!(s.last_drafted <= 8, "drafted {}", s.last_drafted);
    }

    #[test]
    fn a_run_of_misses_COLLAPSES_the_draft_length() {
        // A drafter that is wrong about the context is not fixed by drafting less of the same wrong
        // thing gradually — it should back off at once and re-probe cheaply.
        let mut s = SpecLoop::new(8);
        let ctx = vec![1, 2, 3, 7, 8, 9, 4, 1, 2, 3];
        for _ in 0..3 {
            let d = s.propose(&ctx);
            s.settle(&d, &vec![77u32; d.len() + 1]);   // target disagrees with everything
        }
        assert!(s.last_drafted <= 1, "backed off to {}", s.last_drafted);
    }

    #[test]
    fn the_accept_rate_is_NONE_before_anything_is_drafted() {
        // Zero drafted is not a zero rate. Reporting 0% would say lookup is failing when it has not
        // been asked — the same distinction as an unmeasured bandwidth versus a measured zero.
        assert_eq!(SpecLoop::new(4).accept_rate(), None);
    }
}
