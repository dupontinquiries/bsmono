//! lookupdraft.rs — draft tokens from the context itself, with no second model.
//!
//! `specverify` gives the accept rule and `MATVEC_SMALL_Q4` gives the batched kernel; what neither
//! supplies is candidate tokens. The obvious source is a smaller draft model, which means loading two
//! checkpoints and having them share a tokenizer — a real barrier, and the reason speculation stays
//! unimplemented in most single-model engines.
//!
//! llama.cpp's `lookup` decoding takes the other route: **find the current suffix earlier in the
//! context and copy what followed it**. No second model, no extra memory, and on the work this engine
//! is actually used for — editing code, answering about a pasted file, rewriting a passage — the
//! output repeats the input constantly, so the hit rate is high exactly where it matters.
//!
//! ## Why this is worth more here than elsewhere
//!
//! Decode measured 182 ms/token against prefill's 12.6 on the same weights: 14× slower per token for
//! strictly less work, because m=1 has nothing in flight to cover memory latency. A draft of 4 makes
//! it m=5. Colibri does not speculate at all, so this shape comes from llama.cpp alone.
//!
//! ## The cost of a wrong guess is one batched pass
//!
//! `specverify::accept_prefix` keeps only the matching prefix and emits the target's own token at the
//! divergence, so a rejected draft costs the lookup (microseconds) and never a wasted target step.
//! That asymmetry is what makes a cheap, often-wrong drafter worth having.

/// Longest suffix length to try matching. Longer matches are rarer and much more likely to continue
/// correctly, so the search runs long-to-short and takes the first hit.
pub const MAX_NGRAM: usize = 8;
/// Shorter than this matches noise. Two tokens recur constantly and predict nothing.
pub const MIN_NGRAM: usize = 3;

/// Propose up to `k` tokens by finding the recent suffix earlier in `context`.
///
/// Searches for the longest suffix of `context` that also occurs earlier, and returns what followed
/// that earlier occurrence. Returns an empty vec when nothing matches, which the caller should treat
/// as "decode normally this step" rather than as an error.
///
/// ## Searching backwards
///
/// The scan runs from the most recent earlier occurrence to the oldest. A phrase repeated many times
/// is most likely to continue the way it did MOST RECENTLY — in an edit-this-code exchange the latest
/// occurrence is the version being revised, and the oldest is the one already superseded.
pub fn draft(context: &[u32], k: usize) -> Vec<u32> {
    if k == 0 || context.len() < MIN_NGRAM + 1 { return Vec::new(); }
    let max_n = MAX_NGRAM.min(context.len() - 1);
    for n in (MIN_NGRAM..=max_n).rev() {
        let suffix = &context[context.len() - n..];
        // Candidate positions where this n-gram occurred earlier, most recent first. The search space
        // excludes the suffix itself, so a match is always a genuine repeat.
        let limit = context.len() - n;
        for start in (0..limit).rev() {
            if &context[start..start + n] != suffix { continue; }
            let after = start + n;
            if after >= context.len() { continue; }
            let take = k.min(context.len() - after);
            if take == 0 { continue; }
            return context[after..after + take].to_vec();
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeated_phrase_drafts_its_own_continuation() {
        // The case this exists for: the model is rewriting something it was shown.
        let ctx = vec![1, 2, 3, 4, 5, 6, 9, 9, 1, 2, 3];
        assert_eq!(draft(&ctx, 3), vec![4, 5, 6]);
    }

    #[test]
    fn nothing_matching_drafts_NOTHING_rather_than_guessing() {
        // An empty draft means "decode normally this step". Returning a plausible-looking guess would
        // spend a batched pass to reject it, every step, on exactly the content where lookup cannot
        // help.
        let ctx = vec![1, 2, 3, 4, 5, 6, 7, 8];
        assert!(draft(&ctx, 4).is_empty());
    }

    #[test]
    fn the_MOST_RECENT_occurrence_wins() {
        // A phrase repeated several times is most likely to continue the way it did last. In an
        // edit-this-code exchange the latest occurrence is the version being revised and the earliest
        // is already superseded — drafting from the old one is confidently wrong.
        let ctx = vec![1, 2, 3, 40, 41, 9, 1, 2, 3, 50, 51, 9, 1, 2, 3];
        assert_eq!(draft(&ctx, 2), vec![50, 51], "not the older 40,41");
    }

    #[test]
    fn a_LONGER_match_is_preferred_over_a_shorter_one() {
        // Long-to-short: a 5-token match predicts far better than a 3-token one, and taking the first
        // short hit would discard that.
        //            0  1  2  3  4  5   6  7  8  9 10 11 12 13 14
        let ctx = vec![7, 8, 1, 2, 3, 99, 5, 6, 1, 2, 3, 77, 7, 8, 1, 2, 3];
        // The 5-gram [6,1,2,3] is not present again; the 3-gram [1,2,3] occurs at 2, 8 and 14.
        // The longest suffix that repeats is [7,8,1,2,3] at index 0, which was followed by 99.
        assert_eq!(draft(&ctx, 1), vec![99], "the 5-token match, not the most recent 3-token one");
    }

    #[test]
    fn a_match_at_the_very_end_yields_no_continuation() {
        // The earlier occurrence must have something AFTER it, or there is nothing to copy. Returning
        // the suffix itself would draft the tokens already emitted.
        let ctx = vec![1, 2, 3, 1, 2, 3];
        // [1,2,3] occurs at 0 followed by [1,2,3] — a legitimate draft.
        assert_eq!(draft(&ctx, 3), vec![1, 2, 3]);
    }

    #[test]
    fn a_short_context_drafts_nothing() {
        // Below MIN_NGRAM+1 there is not enough to match against, and a 2-token match predicts noise.
        assert!(draft(&[1, 2], 4).is_empty());
        assert!(draft(&[], 4).is_empty());
    }

    #[test]
    fn k_bounds_the_draft_and_the_context_bounds_it_further() {
        let ctx = vec![1, 2, 3, 4, 5, 6, 7, 8, 1, 2, 3];
        assert_eq!(draft(&ctx, 2).len(), 2, "k caps it");
        // Everything after the earlier occurrence is available — 8 tokens here, not 5: my first
        // expectation stopped at the second occurrence of the pattern, but there is no reason to.
        // Drafting past it is a guess like any other, and a wrong one costs the verify pass it was
        // going to spend anyway.
        assert_eq!(draft(&ctx, 99), vec![4, 5, 6, 7, 8, 1, 2, 3]);
    }

    #[test]
    fn a_zero_k_drafts_nothing() {
        assert!(draft(&[1, 2, 3, 4, 1, 2, 3], 0).is_empty());
    }
}
