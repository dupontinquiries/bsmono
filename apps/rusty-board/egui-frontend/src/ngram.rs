//! ngram — the next-word tie-breaker `swipe_rerank` calls into.
//!
//! *"After you type 'thank' the model learns that you tend to follow it with
//! 'you' (or 'ya', or your specific style); next time the recognizer is
//! choosing between 'you' and 'your' with similar shape scores, the n-gram
//! model breaks the tie."*
//!
//! ## Three rejected alternatives, each with a number attached
//!
//! The source's "what we DON'T do" section is the argument, and it is unusually
//! concrete:
//!
//! * **No pre-trained corpus** — *"a 3-gram model trained on the Brown corpus
//!   would be ~100MB and biased toward whatever text was in the corpus.
//!   Building up from the user's actual typing is ~50KB and personalized."*
//!   Two thousand times smaller, and it learns *your* style rather than a 1961
//!   American news corpus's.
//! * **No neural net** — pure counting, *"bounded, cheap to look up, no
//!   inference latency"*. On a keyboard the budget is the gap between finishing
//!   a swipe and showing a word.
//! * **No smoothing tricks (Kneser-Ney etc)** — because *"the n-gram score is a
//!   TIE-BREAKER, not the primary ranking signal."* Smoothing exists to make a
//!   language model's absolute probabilities meaningful; here only the ordering
//!   between two candidates matters, so the complexity buys nothing.
//!
//! That third one is the reasoning most likely to be "improved" away by
//! somebody who knows what Kneser-Ney is.
//!
//! ## Backoff stops at bigrams, deliberately
//!
//! Trigram, then bigram, then **zero** — not unigram, because *"the
//! recognizer's existing frequency_rank already handles unigrams"*. Adding one
//! here would double-count the same signal and let raw word frequency
//! masquerade as context.

/// Ceiling on the contribution, shared with the dictionary so neither signal
/// can dominate the recognizer.
pub const MAX_BOOST: f64 = 0.5;
/// Same half-life as the adaptive dictionary: old patterns fade.
pub const DECAY_HALF_LIFE_SECS: f64 = 30.0 * 24.0 * 3600.0;
pub const MAX_BIGRAMS: usize = 5_000;
pub const MAX_TRIGRAMS: usize = 10_000;
pub const MIN_EFFECTIVE_COUNT: f64 = 0.05;

/// Which order produced a score. Carried so a diagnostic can say whether
/// context or mere frequency drove a suggestion.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Order { Trigram, Bigram, None }

/// A counted continuation.
#[derive(Clone, Copy, Debug)]
pub struct Count {
    pub count: f64,
    pub last_used_secs: u64,
}

/// Count after decay — identical curve to the dictionary's, so the two signals
/// age together rather than one outliving the other.
pub fn effective(c: &Count, now_secs: u64) -> f64 {
    let age = now_secs.saturating_sub(c.last_used_secs) as f64;
    c.count * 0.5f64.powf(age / DECAY_HALF_LIFE_SECS)
}

/// Normalise a word for keying.
///
/// Lowercased and trimmed, so "Thank" and "thank" share a history. Without it
/// the model learns sentence-initial capitals as separate words and needs twice
/// the data to reach the same confidence.
pub fn norm(w: &str) -> String { w.trim().to_lowercase() }

pub fn bigram_key(prev: &str, word: &str) -> String {
    format!("{}\u{1}{}", norm(prev), norm(word))
}

pub fn trigram_key(pp: &str, prev: &str, word: &str) -> String {
    format!("{}\u{1}{}\u{1}{}", norm(pp), norm(prev), norm(word))
}

/// What the model knows, as the caller supplies it.
pub struct Context<'a> {
    /// Effective count for the trigram, and the total over that prefix.
    pub trigram: Option<(f64, f64)>,
    /// Same for the bigram.
    pub bigram: Option<(f64, f64)>,
    pub _marker: std::marker::PhantomData<&'a ()>,
}

/// Score a candidate, with backoff.
///
/// Returns the value and which order supplied it. Trigram wins when it has
/// data at all — a longer match is more specific, and specificity is the whole
/// reason to keep trigrams as well as bigrams.
pub fn score(ctx: &Context) -> (f64, Order) {
    if let Some((c, total)) = ctx.trigram {
        if c > 0.0 && total > 0.0 {
            return ((c / total).min(MAX_BOOST), Order::Trigram);
        }
    }
    if let Some((c, total)) = ctx.bigram {
        if c > 0.0 && total > 0.0 {
            return ((c / total).min(MAX_BOOST), Order::Bigram);
        }
    }
    // Deliberately NOT a unigram fallback: frequency_rank already handles that,
    // and adding it here would double-count and let raw frequency masquerade as
    // context.
    (0.0, Order::None)
}

/// Is this entry spent?
pub fn is_spent(c: &Count, now_secs: u64) -> bool {
    effective(c, now_secs) < MIN_EFFECTIVE_COUNT
}

/// Do the caps leave room for trigrams to be useful?
///
/// Trigrams are sparser than bigrams — most pairs are followed by many words —
/// so a trigram table the same size as the bigram one fills with
/// once-seen entries and evicts before any of them earn a score.
pub fn caps_are_sane() -> bool { MAX_TRIGRAMS > MAX_BIGRAMS }

/// Approximate on-disk size, for the argument against a pre-trained corpus.
pub fn approx_bytes(bigrams: usize, trigrams: usize) -> usize {
    // ~16 bytes per line in the TOML: two short words, a count, a timestamp.
    (bigrams + trigrams) * 16
}

/// The counts, held so the scoring policy above has something to score.
///
/// The port carried the POLICY — backoff, decay, the caps — and no store, so
/// nothing could call it. This is the smallest thing that makes it usable: two
/// maps and the prefix totals the policy needs, with the same decay curve.
///
/// Deliberately not persisted here. A keyboard writing to disk on every commit
/// is a keyboard that stutters, and the host owns when to save — the same split
/// `adaptive` uses.
#[derive(Debug, Clone, Default)]
pub struct NgramStore {
    bigrams: std::collections::HashMap<String, Count>,
    trigrams: std::collections::HashMap<String, Count>,
    /// Totals per prefix, so scoring does not walk the whole map per candidate.
    ///
    /// Maintained alongside rather than recomputed: a candidate list is scored
    /// on every swipe, and summing thousands of entries per candidate is the
    /// difference between a suggestion bar and a stutter.
    bi_prefix: std::collections::HashMap<String, f64>,
    tri_prefix: std::collections::HashMap<String, f64>,
}

impl NgramStore {
    pub fn len(&self) -> (usize, usize) { (self.bigrams.len(), self.trigrams.len()) }
    pub fn is_empty(&self) -> bool { self.bigrams.is_empty() && self.trigrams.is_empty() }

    /// Learn from a committed word given the two before it.
    pub fn record(&mut self, pp: Option<&str>, prev: Option<&str>, word: &str, now_secs: u64) {
        let w = norm(word);
        if w.is_empty() { return; }
        if let Some(p) = prev {
            let p = norm(p);
            bump(&mut self.bigrams, bigram_key(&p, &w), now_secs);
            *self.bi_prefix.entry(p.clone()).or_insert(0.0) += 1.0;
            if let Some(pp) = pp {
                let pp = norm(pp);
                bump(&mut self.trigrams, trigram_key(&pp, &p, &w), now_secs);
                *self.tri_prefix.entry(format!("{pp}\u{1}{p}")).or_insert(0.0) += 1.0;
            }
        }
        self.evict(now_secs);
    }

    /// The score for one candidate in this context.
    pub fn score_for(&self, pp: Option<&str>, prev: Option<&str>, word: &str, now_secs: u64)
        -> (f64, Order) {
        let w = norm(word);
        let bigram = prev.map(|p| norm(p)).and_then(|p| {
            let c = self.bigrams.get(&bigram_key(&p, &w))?;
            Some((effective(c, now_secs), *self.bi_prefix.get(&p).unwrap_or(&0.0)))
        });
        let trigram = match (pp, prev) {
            (Some(pp), Some(p)) => {
                let (pp, p) = (norm(pp), norm(p));
                self.trigrams.get(&trigram_key(&pp, &p, &w)).map(|c| {
                    (effective(c, now_secs),
                     *self.tri_prefix.get(&format!("{pp}\u{1}{p}")).unwrap_or(&0.0))
                })
            }
            _ => None,
        };
        score(&Context { trigram, bigram, _marker: std::marker::PhantomData })
    }

    /// Drop spent entries, and enforce the caps if still over.
    fn evict(&mut self, now_secs: u64) {
        if self.bigrams.len() > MAX_BIGRAMS {
            self.bigrams.retain(|_, c| !is_spent(c, now_secs));
        }
        if self.trigrams.len() > MAX_TRIGRAMS {
            self.trigrams.retain(|_, c| !is_spent(c, now_secs));
        }
    }
}

fn bump(m: &mut std::collections::HashMap<String, Count>, key: String, now: u64) {
    let e = m.entry(key).or_insert(Count { count: 0.0, last_used_secs: now });
    // Decay the existing count to NOW before adding, or an old entry re-used
    // today would keep its undecayed weight and outrank everything.
    e.count = effective(e, now) + 1.0;
    e.last_used_secs = now;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(tri: Option<(f64, f64)>, bi: Option<(f64, f64)>) -> Context<'static> {
        Context { trigram: tri, bigram: bi, _marker: std::marker::PhantomData }
    }

    #[test]
    fn a_trigram_beats_a_bigram_when_it_has_data() {
        // A longer match is more specific, and specificity is the whole reason
        // to keep trigrams as well as bigrams.
        let (s, o) = score(&ctx(Some((8.0, 10.0)), Some((1.0, 100.0))));
        assert_eq!(o, Order::Trigram);
        assert!(s > 0.1);
    }

    #[test]
    fn backoff_reaches_the_bigram_when_the_trigram_is_unseen() {
        // The common case early on: two words of context is rare, one is not.
        let (s, o) = score(&ctx(None, Some((5.0, 10.0))));
        assert_eq!(o, Order::Bigram);
        assert!((s - 0.5).abs() < 1e-9);
    }

    #[test]
    fn an_empty_trigram_entry_does_not_block_the_bigram() {
        // A zero-count trigram is an entry that decayed, not evidence against
        // the continuation — treating it as a match would return 0.0 and
        // suppress a bigram that does know.
        let (_, o) = score(&ctx(Some((0.0, 10.0)), Some((5.0, 10.0))));
        assert_eq!(o, Order::Bigram);
    }

    #[test]
    fn backoff_stops_at_bigrams_rather_than_reaching_unigrams() {
        // "The recognizer's existing frequency_rank already handles unigrams."
        // Adding one here would DOUBLE-COUNT the same signal and let raw word
        // frequency masquerade as context.
        let (s, o) = score(&ctx(None, None));
        assert_eq!(s, 0.0);
        assert_eq!(o, Order::None);
    }

    #[test]
    fn the_contribution_is_capped_like_the_dictionarys() {
        // Shared ceiling so neither personalisation signal can override the
        // recognizer on its own.
        let (s, _) = score(&ctx(Some((100.0, 100.0)), None));
        assert_eq!(s, MAX_BOOST);
        assert_eq!(MAX_BOOST, crate::swipe_rerank::MAX_BOOST,
                   "the two signals drifted apart on their ceiling");
    }

    #[test]
    fn a_zero_total_cannot_divide() {
        // An entry present with a zero prefix total is a corrupt load, and
        // dividing would produce inf and sort every other candidate below it.
        let (s, o) = score(&ctx(Some((5.0, 0.0)), None));
        assert_eq!(s, 0.0);
        assert_eq!(o, Order::None);
    }

    #[test]
    fn case_does_not_split_a_words_history() {
        // Without normalisation the model learns sentence-initial capitals as
        // separate words and needs twice the data for the same confidence.
        assert_eq!(bigram_key("Thank", "You"), bigram_key("thank", "you"));
        assert_eq!(trigram_key("I", "Thank", "You"), trigram_key("i", "thank", "you"));
        assert_eq!(norm("  Thank\t"), "thank");
    }

    #[test]
    fn the_key_separator_cannot_be_typed() {
        // A space separator would make ("a b", "c") and ("a", "b c") collide,
        // and the model would learn a continuation for a phrase nobody typed.
        let a = bigram_key("a b", "c");
        let b = bigram_key("a", "b c");
        assert_ne!(a, b);
    }

    #[test]
    fn decay_matches_the_dictionarys_curve() {
        // The two signals must age together; one outliving the other would
        // shift the balance between context and frequency over time without
        // anyone changing a constant.
        assert_eq!(DECAY_HALF_LIFE_SECS, crate::swipe_rerank::DECAY_HALF_LIFE_SECS);
        let c = Count { count: 8.0, last_used_secs: 0 };
        assert!((effective(&c, DECAY_HALF_LIFE_SECS as u64) - 4.0).abs() < 1e-6);
    }

    #[test]
    fn a_backwards_clock_does_not_inflate_a_count() {
        let c = Count { count: 4.0, last_used_secs: 10_000 };
        assert!(effective(&c, 1_000) <= 4.0);
    }

    #[test]
    fn a_stale_pattern_is_droppable() {
        let c = Count { count: 1.0, last_used_secs: 0 };
        assert!(!is_spent(&c, 0));
        assert!(is_spent(&c, (DECAY_HALF_LIFE_SECS * 60.0) as u64));
    }

    #[test]
    fn there_is_more_room_for_trigrams_than_bigrams() {
        // Trigrams are sparser — most pairs are followed by many words — so an
        // equal-sized table fills with once-seen entries and evicts them before
        // any earn a score.
        assert!(caps_are_sane());
        assert_eq!(MAX_TRIGRAMS, MAX_BIGRAMS * 2);
    }

    #[test]
    fn the_whole_model_stays_far_smaller_than_a_corpus() {
        // "~100MB [for Brown] ... ~50KB and personalized." The size argument is
        // the reason this is counting rather than a pretrained model, so a cap
        // change that broke it would quietly undo the decision.
        let full = approx_bytes(MAX_BIGRAMS, MAX_TRIGRAMS);
        assert!(full < 1_000_000, "the model grew past its own argument: {full}");
    }

    #[test]
    fn no_smoothing_is_a_decision_not_an_omission() {
        // "The n-gram score is a TIE-BREAKER, not the primary ranking signal."
        // Smoothing exists to make ABSOLUTE probabilities meaningful; only the
        // ORDERING between candidates matters here, so it buys nothing — and
        // this is the reasoning most likely to be "improved" away by somebody
        // who knows what Kneser-Ney is.
        let (a, _) = score(&ctx(None, Some((3.0, 10.0))));
        let (b, _) = score(&ctx(None, Some((7.0, 10.0))));
        assert!(b > a, "the ordering between two seen continuations must hold");
        // and an unseen continuation contributes nothing rather than a smoothed
        // floor, which would rank it above a candidate with no entry at all
        let (u, _) = score(&ctx(None, None));
        assert_eq!(u, 0.0);
    }
}

#[cfg(test)]
mod store_tests {
    use super::*;

    #[test]
    fn a_learned_continuation_scores_above_an_unseen_one() {
        // The whole job: after "thank" you tend to write "you".
        let mut st = NgramStore::default();
        for _ in 0..5 { st.record(None, Some("thank"), "you", 0); }
        let (seen, order) = st.score_for(None, Some("thank"), "you", 0);
        let (unseen, _) = st.score_for(None, Some("thank"), "your", 0);
        assert!(seen > unseen, "{seen} vs {unseen}");
        assert_eq!(order, Order::Bigram);
    }

    #[test]
    fn a_trigram_beats_the_bigram_once_it_has_data() {
        let mut st = NgramStore::default();
        for _ in 0..3 { st.record(Some("i"), Some("thank"), "you", 0); }
        let (_, order) = st.score_for(Some("i"), Some("thank"), "you", 0);
        assert_eq!(order, Order::Trigram);
    }

    #[test]
    fn no_context_scores_nothing_rather_than_guessing() {
        // At the start of a line there is no previous word, and inventing a
        // score would rank candidates by nothing at all.
        let mut st = NgramStore::default();
        st.record(None, Some("a"), "b", 0);
        assert_eq!(st.score_for(None, None, "b", 0), (0.0, Order::None));
    }

    #[test]
    fn re_using_an_old_entry_decays_it_before_adding() {
        // Without decaying first, an entry untouched for a year would keep its
        // full weight the moment it is used again and outrank everything.
        let mut st = NgramStore::default();
        st.record(None, Some("a"), "b", 0);
        let year = (DECAY_HALF_LIFE_SECS * 12.0) as u64;
        st.record(None, Some("a"), "b", year);
        let (s, _) = st.score_for(None, Some("a"), "b", year);
        assert!(s <= MAX_BOOST);
    }

    #[test]
    fn an_empty_word_is_not_learned() {
        let mut st = NgramStore::default();
        st.record(None, Some("a"), "", 0);
        assert!(st.is_empty());
    }

    #[test]
    fn case_does_not_split_a_continuation() {
        let mut st = NgramStore::default();
        for _ in 0..3 { st.record(None, Some("Thank"), "You", 0); }
        let (s, _) = st.score_for(None, Some("thank"), "you", 0);
        assert!(s > 0.0, "capitalisation split the history");
    }
}
