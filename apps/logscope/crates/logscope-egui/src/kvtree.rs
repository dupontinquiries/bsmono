//! kvtree.rs — the pure decision at the heart of the KV-cache *prefix tree*.
//!
//! The engine already warm-resumes from a SINGLE anchor: it keeps the last turn's (tokens, KV cache) and,
//! when the next prompt shares a long prefix with it, seeds that prefix's KV and prefills only the
//! divergent suffix instead of the whole prompt. That's perfect for a LINEAR chat — each turn extends the
//! last — but a branching conversation has many tips. Switching to a sibling branch, or fanning N
//! `:continue-all` continuations off one leaf, shares its prefix with a DIFFERENT lineage than "the last
//! turn", so the single anchor misses and the engine re-prefills from scratch every time.
//!
//! Generalize the single anchor to a small POOL of (tokens, KV) entries — one per recent branch tip — and
//! the only new decision is: given a new prompt, which cached entry shares the longest *usable* prefix? In
//! particular, the first continuation off a leaf prefills its lineage and caches it; the next N−1 reuse
//! that cache → **1 prefill + N decodes** instead of N prefills. This module is that decision, pure and
//! tested; the engine side just does the GPU buffer copy the existing reuse path already does.

/// Length of the shared leading run of two token sequences.
pub fn common_prefix(a: &[u32], b: &[u32]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

/// Among cached entries — each `(tokens, batch_prefilled_len, filled_pos)` — pick the one sharing the
/// LONGEST usable prefix with `prompt`, subject to the same reuse thresholds the engine uses:
///   * the shared prefix must be ≥ `min_prefix` (else seeding isn't worth a buffer copy), and
///   * the divergent suffix (`prompt.len() − l`) must be ≤ `max_suffix` (the suffix is prefilled with the
///     batched kernel, but reusing a long prefix while prefilling a huge suffix can cost more than just
///     prefilling the whole prompt fresh — so bound it).
///
/// `l` is additionally clamped to the entry's batch-prefilled rows and filled positions (only those KV
/// rows are valid to seed) and to `prompt.len() − 1` (leave ≥1 token to actually start decode). Returns
/// `(entry_index, reuse_len)`, choosing the longest match and, on a tie, the MOST RECENT entry (highest
/// index) since its KV is warmest. `None` when nothing clears the thresholds → caller does a full prefill.
pub fn pick_anchor(
    entries: &[(Vec<u32>, usize, usize)],
    prompt: &[u32],
    min_prefix: usize,
    max_suffix: usize,
) -> Option<(usize, usize)> {
    if prompt.len() < 2 { return None; }
    let mut best: Option<(usize, usize)> = None; // (index, l)
    for (i, (toks, aprefill, pos)) in entries.iter().enumerate() {
        let l = common_prefix(toks, prompt)
            .min(*aprefill)
            .min(*pos)
            .min(prompt.len() - 1);
        if l < min_prefix || prompt.len() - l > max_suffix { continue; }
        // `>=` so an equally-long, more-recent entry wins the tie (warmer cache).
        if best.map_or(true, |(_, bl)| l >= bl) { best = Some((i, l)); }
    }
    best
}

/// Tokens a reuse avoids re-prefilling — i.e. the shared-prefix length actually seeded. The decode then
/// only processes `prompt_len − reuse_len` new tokens, so this is the direct prefill speedup of a hit.
pub fn prefill_savings(prompt_len: usize, reuse_len: usize) -> usize { reuse_len.min(prompt_len) }

/// Like [`pick_anchor`], but `pinned[i]` entries win ties over unpinned ones. A compaction checkpoint
/// (a short, stable, deliberately-cached prefix) is pinned; when its prefix ties a longer-lived neighbour's
/// of equal length, the pinned one is reused — so every path descending from the checkpoint shares its KV
/// instead of being shadowed by an incidentally-equal sibling. Longest prefix still wins outright; among
/// equal lengths, pinned beats unpinned, then most-recent (highest index) breaks the remaining tie.
pub fn pick_anchor_pinned(
    entries: &[(Vec<u32>, usize, usize)],
    prompt: &[u32],
    min_prefix: usize,
    max_suffix: usize,
    pinned: &[bool],
) -> Option<(usize, usize)> {
    if prompt.len() < 2 { return None; }
    let mut best: Option<(usize, usize, bool)> = None; // (index, l, pinned)
    for (i, (toks, aprefill, pos)) in entries.iter().enumerate() {
        let l = common_prefix(toks, prompt).min(*aprefill).min(*pos).min(prompt.len() - 1);
        if l < min_prefix || prompt.len() - l > max_suffix { continue; }
        let pin = pinned.get(i).copied().unwrap_or(false);
        let better = match best {
            None => true,
            Some((_, bl, bp)) => {
                if l != bl { l > bl }       // longer prefix wins
                else if pin != bp { pin }   // tie on length → pinned wins
                else { true }               // tie on length+pin → later (more recent) wins
            }
        };
        if better { best = Some((i, l, pin)); }
    }
    best.map(|(i, l, _)| (i, l))
}

/// Which pool entry to evict when the anchor pool is full: the OLDEST UNPINNED entry (lowest index that is
/// not pinned), so pinned checkpoints survive eviction and stay hot. `None` when every entry is pinned —
/// the caller must grow the pool or force-drop, rather than silently dropping a checkpoint.
pub fn evict_index(len: usize, pinned: &[bool]) -> Option<usize> {
    (0..len).find(|&i| !pinned.get(i).copied().unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build an entry whose batch_prefilled_len and filled_pos both cover the whole token vec (the common
    // case: the entry was fully prefilled then decoded).
    fn entry(toks: &[u32]) -> (Vec<u32>, usize, usize) { (toks.to_vec(), toks.len(), toks.len()) }

    #[test]
    fn common_prefix_counts_shared_run() {
        assert_eq!(common_prefix(&[1, 2, 3, 4], &[1, 2, 9, 4]), 2);
        assert_eq!(common_prefix(&[1, 2], &[1, 2, 3]), 2);
        assert_eq!(common_prefix(&[], &[1]), 0);
    }

    #[test]
    fn picks_the_longest_matching_branch() {
        // shared prefix P (0..20), then sibling suffixes A and B
        let p: Vec<u32> = (0..20).collect();
        let mut pa = p.clone(); pa.extend([100, 101, 102]);          // lineage A = P + A
        let mut pb = p.clone(); pb.extend([200, 201, 202]);          // lineage B = P + B
        let pool = [entry(&pa), entry(&pb)];

        // continuing lineage A (prompt = P+A+new) reuses entry 0 for the FULL P+A, not just P
        let mut prompt_a = pa.clone(); prompt_a.extend([103, 104]);
        let (idx, l) = pick_anchor(&pool, &prompt_a, 16, 256).expect("A reuses");
        assert_eq!(idx, 0);
        assert_eq!(l, pa.len(), "reuses all of lineage A's prefix");

        // continuing lineage B reuses entry 1
        let mut prompt_b = pb.clone(); prompt_b.extend([203]);
        assert_eq!(pick_anchor(&pool, &prompt_b, 16, 256).unwrap().0, 1);

        // a prompt that only shares the common ancestor P with both → reuses P (l == 20) off one of them
        let mut prompt_c = p.clone(); prompt_c.extend([900, 901]);
        let (_, lc) = pick_anchor(&pool, &prompt_c, 16, 256).expect("ancestor reuse");
        assert_eq!(lc, 20, "falls back to the shared ancestor prefix");
    }

    #[test]
    fn the_fanout_case_one_prefill_then_reuse() {
        // :continue-all fans N continuations off ONE leaf L. After the first prefills L and caches L+gen0,
        // the next continuation's prompt is L again — it must still reuse L's KV via the L+gen0 entry.
        let leaf: Vec<u32> = (0..30).collect();
        let mut l_gen0 = leaf.clone(); l_gen0.extend([500, 501, 502, 503]); // L + first continuation
        let pool = [entry(&l_gen0)];
        // second continuation: prompt is the leaf lineage L
        let (idx, l) = pick_anchor(&pool, &leaf, 16, 256).expect("second continuation reuses L");
        assert_eq!(idx, 0);
        // when the prompt is exactly a cached prefix, all but the last token is seeded (≥1 must remain to
        // start decode) — so the prefill is paid once and every later continuation seeds the rest.
        assert_eq!(l, leaf.len() - 1, "reuses the leaf prefix; only 1 token is re-prefilled");
    }

    #[test]
    fn respects_thresholds() {
        let p: Vec<u32> = (0..20).collect();
        let pool = [entry(&p)];
        // tiny shared prefix → below min_prefix → no reuse
        let short = vec![0u32, 1, 999, 998, 997];
        assert!(pick_anchor(&pool, &short, 16, 256).is_none(), "prefix < min → full prefill");
        // long divergent suffix → above max_suffix → no reuse even though prefix is long
        let mut long_suffix = p.clone();
        long_suffix.extend((0..400).map(|j| 1000 + j));
        assert!(pick_anchor(&pool, &long_suffix, 16, 256).is_none(), "suffix > max → full prefill");
    }

    #[test]
    fn empty_pool_and_trivial_prompt() {
        assert_eq!(pick_anchor(&[], &[1, 2, 3], 16, 256), None);
        let p: Vec<u32> = (0..20).collect();
        assert_eq!(pick_anchor(&[entry(&p)], &[0], 16, 256), None, "1-token prompt can't reuse");
    }

    #[test]
    fn tie_prefers_most_recent_entry() {
        // two entries with the SAME prefix length vs the prompt → the later (warmer) one wins
        let p: Vec<u32> = (0..20).collect();
        let mut a = p.clone(); a.push(100);
        let mut b = p.clone(); b.push(200);
        let mut prompt = p.clone(); prompt.extend([900, 901]); // shares only P (l=20) with both
        let pool = [entry(&a), entry(&b)];
        assert_eq!(pick_anchor(&pool, &prompt, 16, 256).unwrap().0, 1, "ties go to the most recent");
    }

    #[test]
    fn prefill_savings_is_the_reused_prefix() {
        assert_eq!(prefill_savings(100, 40), 40);
        assert_eq!(prefill_savings(30, 50), 30, "can't reuse more than the prompt has");
    }

    #[test]
    fn pinned_checkpoint_wins_an_equal_length_tie() {
        // shared ancestor P (len 20); entry 0 is a PINNED compaction checkpoint, entry 1 an ordinary tip.
        let p: Vec<u32> = (0..20).collect();
        let mut chk = p.clone(); chk.push(100);   // checkpoint lineage
        let mut tip = p.clone(); tip.push(200);   // ordinary newer tip
        let mut prompt = p.clone(); prompt.extend([900, 901]); // shares only P (l=20) with both
        let pool = [entry(&chk), entry(&tip)];
        // unpinned: most-recent (entry 1) wins the tie
        assert_eq!(pick_anchor_pinned(&pool, &prompt, 16, 256, &[false, false]).unwrap().0, 1);
        // pin the checkpoint (entry 0): now it wins the equal-length tie → descendants reuse it
        assert_eq!(pick_anchor_pinned(&pool, &prompt, 16, 256, &[true, false]).unwrap().0, 0);
        // but a strictly longer match still wins regardless of pinning
        let mut prompt_b = tip.clone(); prompt_b.push(201); // shares all of `tip` (l=21) — longer than P
        assert_eq!(pick_anchor_pinned(&pool, &prompt_b, 16, 256, &[true, false]).unwrap().0, 1, "length beats pin");
    }

    #[test]
    fn eviction_skips_pinned_entries() {
        assert_eq!(evict_index(3, &[true, false, false]), Some(1), "oldest UNPINNED entry is dropped");
        assert_eq!(evict_index(2, &[true, true]), None, "all pinned → nothing to evict");
        assert_eq!(evict_index(3, &[]), Some(0), "no pin info → oldest");
    }
}
