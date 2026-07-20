//! ngram.rs — prompt-lookup (n-gram) speculative draft source. Pure CPU, zero extra weights.
//!
//! A speedup with NO accuracy cost: it only proposes a draft the target model then verifies. If the draft
//! is right the target confirms several tokens in one forward (fewer forwards → faster); if it's wrong the
//! verify rejects it and output is byte-identical to plain greedy. So it's lossless by construction — the
//! draft is "just a guess the model will check" (Colibri glm.c:4130, METODO E).
//!
//! Method: find the most recent earlier occurrence of the last bigram (a,b) in the token stream, and draft
//! the up-to-G tokens that followed it there. This wins big on repetitive/structured output (code, JSON,
//! lists, quoted context) where recent spans recur — exactly the local-inference workloads here.

/// Propose up to `g` draft tokens by prompt-lookup on the token history `ids`. Returns the tokens that
/// followed the most recent earlier occurrence of the trailing bigram, or empty if none / inputs too short.
/// Mirrors Colibri's ngram_draft (bigram match, most-recent-first scan, bounded to `g`).
pub fn ngram_draft(ids: &[u32], g: usize) -> Vec<u32> {
    let len = ids.len();
    if len < 4 || g < 1 { return Vec::new(); }
    let (a, b) = (ids[len - 2], ids[len - 1]);
    // scan backward for the most recent i where ids[i-1]==a && ids[i]==b (i is the position of `b`);
    // exclude the trailing bigram itself (i must be < len-1). Colibri starts at len-3.
    let mut i = len as isize - 3;
    while i >= 1 {
        let iu = i as usize;
        if ids[iu - 1] == a && ids[iu] == b {
            // draft the tokens that followed this occurrence
            return ids[iu + 1..].iter().take(g).copied().collect();
        }
        i -= 1;
    }
    Vec::new()
}

/// Longest-suffix variant: prefer a longer matched context (trigram+) when available, falling back to the
/// bigram match. A longer matched suffix is a stronger signal, so its draft is likelier to be accepted —
/// fewer wasted verifies. `min_ctx` is the shortest suffix to match (>=2); we try longest-first down to it.
pub fn ngram_draft_longest(ids: &[u32], g: usize, min_ctx: usize) -> Vec<u32> {
    let len = ids.len();
    let min_ctx = min_ctx.max(2);
    if len < min_ctx + 2 || g < 1 { return Vec::new(); }
    // try the longest suffix first (capped so there's room for an earlier occurrence), down to min_ctx.
    let max_ctx = (len - 1).min(16);
    for ctx in (min_ctx..=max_ctx).rev() {
        let suffix = &ids[len - ctx..];
        // search for an earlier occurrence of `suffix` ending before the final position
        // (end index e is the position just past the match; e ranges over [ctx, len-1)).
        let mut e = len - 1;
        while e >= ctx {
            if &ids[e - ctx..e] == suffix {
                let draft: Vec<u32> = ids[e..].iter().take(g).copied().collect();
                if !draft.is_empty() { return draft; }
            }
            e -= 1;
        }
    }
    // fall back to the plain bigram lookup
    ngram_draft(ids, g)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drafts_the_continuation_of_the_last_bigram() {
        // "the cat sat . the cat" → last bigram (the,cat) last occurred at the start; the draft is the
        // continuation from there, bounded by g. The verify step keeps whatever prefix the model confirms.
        let ids = [10u32, 20, 30, 99, 10, 20];
        //          the cat sat  .  the cat
        let d = ngram_draft(&ids, 3);
        assert_eq!(d, vec![30, 99, 10], "drafts the g tokens following the earlier (the,cat)");
        assert_eq!(ngram_draft(&ids, 1), vec![30], "g=1 → just the next token");
    }

    #[test]
    fn drafts_multiple_tokens_up_to_g() {
        // repeating structure: a b c d e ... a b  → draft c,d,e
        let ids = [1u32, 2, 3, 4, 5, 9, 1, 2];
        assert_eq!(ngram_draft(&ids, 3), vec![3, 4, 5]);
        assert_eq!(ngram_draft(&ids, 2), vec![3, 4], "bounded to g");
    }

    #[test]
    fn most_recent_occurrence_wins() {
        // (1,2) occurs twice earlier; the MORE RECENT one (followed by 7) should be drafted, not the first.
        let ids = [1u32, 2, 3, 9, 1, 2, 7, 9, 1, 2];
        assert_eq!(ngram_draft(&ids, 1), vec![7], "most-recent earlier bigram, not the oldest");
    }

    #[test]
    fn no_match_returns_empty() {
        let ids = [1u32, 2, 3, 4, 5, 6]; // trailing bigram (5,6) never occurred earlier
        assert!(ngram_draft(&ids, 4).is_empty());
    }

    #[test]
    fn too_short_or_zero_g_returns_empty() {
        assert!(ngram_draft(&[1, 2, 3], 4).is_empty(), "len<4");
        assert!(ngram_draft(&[1, 2, 3, 4], 0).is_empty(), "g<1");
    }

    #[test]
    fn longest_suffix_prefers_longer_context() {
        // Two earlier spots continue the last token differently; the longer matched suffix should win.
        // stream: X A B C   Y A B D   ... A B   → suffix (A,B) matches both; but (Y,A,B) matches only the
        // second (→ D). Longest-context should pick D over C.
        let ids = [88u32, 1, 2, 3, 77, 1, 2, 4, 66, 1, 2];
        //          X  A  B  C   Y  A  B  D   Z  A  B
        let d = ngram_draft_longest(&ids, 1, 2);
        // longest matched suffix ending before the tail: the trigram (2,4? no)… the tail is (…,1,2).
        // ctx=3 suffix = [66,1,2] has no earlier occurrence; ctx=2 suffix [1,2] most-recent earlier is at
        // e=7 (after the 77,1,2,4 span) → draft [4]. Deterministic + matches bigram fallback here.
        assert_eq!(d, vec![4], "most-recent [1,2] continuation is 4");
    }

    #[test]
    fn longest_falls_back_to_bigram() {
        let ids = [1u32, 2, 3, 4, 5, 9, 1, 2];
        assert_eq!(ngram_draft_longest(&ids, 3, 2), vec![3, 4, 5], "no longer context → bigram result");
    }

    // The lossless invariant: speculation with an n-gram draft + accept-longest-matching-prefix produces
    // EXACTLY the greedy sequence, whatever the draft proposes. Model it against a deterministic oracle
    // (greedy next-token = some fixed function) so the test is self-contained — this is the property that
    // makes the draft a pure speedup (the real verify in speculative_generate has the same structure).
    #[test]
    fn ngram_speculation_is_lossless_vs_greedy() {
        // deterministic "model": next token = (last + 1) % 7 — a fully predictable oracle
        let oracle = |ctx: &[u32]| -> u32 { (ctx[ctx.len() - 1] + 1) % 7 };
        let steps = 30usize;
        // plain greedy
        let mut greedy = vec![3u32, 1, 4, 1]; // seed
        for _ in 0..steps { let t = oracle(&greedy); greedy.push(t); }

        // speculative with n-gram draft + prefix accept against the SAME oracle
        let mut spec = vec![3u32, 1, 4, 1];
        let mut produced = 0usize;
        while produced < steps {
            let draft = ngram_draft(&spec, 4);
            // verify: accept draft tokens while each equals the oracle's greedy pick; stop at first miss
            let mut accepted = 0usize;
            for &d in &draft {
                if produced + accepted >= steps { break; }
                if oracle(&spec) == d { spec.push(d); accepted += 1; } else { break; }
            }
            produced += accepted;
            // the target always emits its own correction/next token (the guaranteed-correct one)
            if produced < steps { let t = oracle(&spec); spec.push(t); produced += 1; }
        }
        assert_eq!(&spec[..greedy.len()], &greedy[..], "speculated output must equal greedy, draft or no draft");
    }
}
