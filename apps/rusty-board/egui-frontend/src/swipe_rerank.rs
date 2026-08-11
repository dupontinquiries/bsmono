//! swipe_rerank — personalising swipe candidates, from `ai_swipe` and
//! `adaptive_dictionary`.
//!
//! [`crate::cardinal_gesture`] decides *which gesture* a path was. This is the
//! next question: when a swipe produces several candidate words, which one gets
//! committed.
//!
//! ## Score is a COST — lower wins
//!
//! The original applies personalisation by **subtracting**:
//!
//! ```text
//! c.score -= dict.boost(&c.word);
//! c.score -= ngram.score(prev_prev, prev, &c.word);
//! ```
//!
//! Get that sign backwards and personalisation actively demotes the words the
//! user types most — the feature would make the keyboard worse the longer they
//! used it, which is close to the worst failure mode a learning system has.
//!
//! ## The boost is bounded, and that bound is the safety property
//!
//! `MAX_BOOST = 0.5`. Without a ceiling a word committed a thousand times would
//! win against a candidate the recognizer is certain about — the user's history
//! would override what their finger actually did, and a common word would start
//! appearing in place of the one they swiped.
//!
//! ## History decays, so the keyboard can follow a change of subject
//!
//! A 30-day half-life. Frequency without decay means a project you finished
//! last year still shapes your typing today, and words you have stopped using
//! never release their hold on the candidate list.
//!
//! ## Off means genuinely off
//!
//! *"When the master toggle is off, they're cheap no-ops — no allocations, no
//! map lookups, no I/O."* On a phone keyboard the work happens on every commit,
//! so a disabled feature that still allocates is a battery cost for something
//! the user declined.

/// Ceiling on the per-word boost.
pub const MAX_BOOST: f64 = 0.5;
/// Frequency half-life.
pub const DECAY_HALF_LIFE_SECS: f64 = 30.0 * 24.0 * 3600.0;
/// Words remembered before eviction.
pub const MAX_ENTRIES: usize = 1500;
/// Below this an entry has decayed to nothing and is dropped.
pub const MIN_EFFECTIVE_COUNT: f64 = 0.05;

/// A candidate from the recognizer.
///
/// `score` is a **cost**: lower is better. Named in the docs because the
/// subtraction below is meaningless — and actively harmful — under the other
/// convention.
#[derive(Clone, PartialEq, Debug)]
pub struct WordCandidate {
    pub word: String,
    pub score: f64,
}

/// One learned word.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub count: f64,
    pub last_used_secs: u64,
}

/// Count after decay.
///
/// Halves every `DECAY_HALF_LIFE_SECS`. A word not typed for two months carries
/// a quarter of its weight, which is what lets the keyboard follow a change of
/// subject rather than being permanently shaped by one.
pub fn effective_count(e: &Entry, now_secs: u64) -> f64 {
    let age = now_secs.saturating_sub(e.last_used_secs) as f64;
    e.count * 0.5f64.powf(age / DECAY_HALF_LIFE_SECS)
}

/// The personalised boost for one word.
///
/// Relative frequency against the whole vocabulary, capped at `MAX_BOOST`.
/// Relative rather than absolute so a heavy user does not accumulate unbounded
/// influence simply by typing more.
pub fn boost(effective: f64, total_effective: f64) -> f64 {
    if total_effective <= 0.0 || effective <= 0.0 { return 0.0; }
    (effective / total_effective).min(MAX_BOOST)
}

/// Apply personalisation and re-sort.
///
/// Subtracts, because score is a cost. Stable within equal scores so a tie
/// keeps the recognizer's own ordering — the recognizer knows something about
/// the path that the frequency model does not.
pub fn apply_to_candidates(
    candidates: &mut Vec<WordCandidate>,
    enabled: bool,
    dict_boost: impl Fn(&str) -> f64,
    ngram_score: impl Fn(&str) -> f64,
) {
    // Cheap no-op when off: no allocation, no lookup. On a phone keyboard this
    // runs on every commit.
    if !enabled || candidates.is_empty() { return; }

    for c in candidates.iter_mut() {
        c.score -= dict_boost(&c.word);
        c.score -= ngram_score(&c.word);
    }
    candidates.sort_by(|a, b| {
        a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// The word that would be committed.
pub fn winner(candidates: &[WordCandidate]) -> Option<&str> {
    candidates.first().map(|c| c.word.as_str())
}

/// Should this entry be dropped?
pub fn is_spent(e: &Entry, now_secs: u64) -> bool {
    effective_count(e, now_secs) < MIN_EFFECTIVE_COUNT
}

/// Which entry to evict when full.
///
/// Lowest effective count, so decay decides rather than insertion order. An
/// LRU here would evict a word used steadily for a year in favour of one typed
/// twice yesterday.
pub fn evict_candidate<'a>(entries: &'a [(String, Entry)], now_secs: u64)
    -> Option<&'a String> {
    entries.iter()
        .min_by(|a, b| effective_count(&a.1, now_secs)
            .partial_cmp(&effective_count(&b.1, now_secs))
            .unwrap_or(std::cmp::Ordering::Equal))
        .map(|(w, _)| w)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(w: &str, s: f64) -> WordCandidate { WordCandidate { word: w.into(), score: s } }

    #[test]
    fn a_familiar_word_wins_over_a_slightly_better_raw_match() {
        // The feature's whole purpose: the recognizer cannot know which of two
        // plausible words this person actually uses.
        let mut c = vec![cand("hello", 1.00), cand("hallo", 0.95)];
        apply_to_candidates(&mut c, true,
            |w| if w == "hello" { 0.3 } else { 0.0 }, |_| 0.0);
        assert_eq!(winner(&c), Some("hello"));
    }

    #[test]
    fn the_boost_is_subtracted_because_score_is_a_cost() {
        // Getting this sign backwards demotes the words the user types MOST —
        // the keyboard would get worse the longer they used it, which is close
        // to the worst failure a learning system has.
        let mut c = vec![cand("word", 1.0)];
        apply_to_candidates(&mut c, true, |_| 0.4, |_| 0.0);
        assert!(c[0].score < 1.0, "personalisation raised the cost: {}", c[0].score);
    }

    #[test]
    fn the_boost_cannot_override_a_confident_recognition() {
        // Without the ceiling, a word committed a thousand times would beat a
        // candidate the recognizer is certain about — the user's HISTORY would
        // override what their FINGER did.
        let mut c = vec![cand("certain", 0.1), cand("frequent", 1.0)];
        apply_to_candidates(&mut c, true,
            |w| if w == "frequent" { MAX_BOOST } else { 0.0 }, |_| 0.0);
        assert_eq!(winner(&c), Some("certain"), "history overrode the recognizer");
    }

    #[test]
    fn the_boost_is_capped_however_dominant_a_word_becomes() {
        // Relative frequency alone would approach 1.0 for a single-word
        // vocabulary.
        assert_eq!(boost(999.0, 1000.0), MAX_BOOST);
        assert!(boost(1.0, 1000.0) < MAX_BOOST);
    }

    #[test]
    fn boost_is_relative_so_a_heavy_user_gains_no_extra_influence() {
        // Absolute counts would mean somebody who types more accumulates
        // unbounded pull on every candidate list.
        assert!((boost(10.0, 100.0) - boost(100.0, 1000.0)).abs() < 1e-9);
    }

    #[test]
    fn an_unknown_word_gets_no_boost_rather_than_a_penalty() {
        // A word not yet learned must not be pushed DOWN, or the keyboard would
        // resist every new word the user tries.
        assert_eq!(boost(0.0, 100.0), 0.0);
        let mut c = vec![cand("new", 1.0), cand("old", 1.1)];
        apply_to_candidates(&mut c, true, |w| if w == "old" { 0.0 } else { 0.0 }, |_| 0.0);
        assert_eq!(winner(&c), Some("new"));
    }

    #[test]
    fn history_halves_every_thirty_days() {
        // Frequency without decay means a project finished last year still
        // shapes today's typing.
        let e = Entry { count: 8.0, last_used_secs: 0 };
        let month = DECAY_HALF_LIFE_SECS as u64;
        assert!((effective_count(&e, month) - 4.0).abs() < 1e-6);
        assert!((effective_count(&e, month * 2) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn a_word_used_just_now_keeps_its_full_weight() {
        let e = Entry { count: 5.0, last_used_secs: 1_000 };
        assert!((effective_count(&e, 1_000) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_inflate_a_count() {
        // saturating_sub: an NTP correction would otherwise give a NEGATIVE age
        // and a boost above the cap.
        let e = Entry { count: 5.0, last_used_secs: 10_000 };
        assert!(effective_count(&e, 1_000) <= 5.0);
    }

    #[test]
    fn a_long_unused_word_is_spent_and_droppable() {
        // Words the user stopped using must release their hold on the
        // candidate list, or the dictionary only ever grows.
        let e = Entry { count: 1.0, last_used_secs: 0 };
        assert!(!is_spent(&e, 0));
        let ten_years = (DECAY_HALF_LIFE_SECS * 60.0) as u64;
        assert!(is_spent(&e, ten_years));
    }

    #[test]
    fn eviction_follows_decay_rather_than_insertion_order() {
        // An LRU would evict a word used steadily for a year in favour of one
        // typed twice yesterday.
        let now = DECAY_HALF_LIFE_SECS as u64 * 2;
        let entries = vec![
            ("steady".to_string(), Entry { count: 50.0, last_used_secs: now - 100 }),
            ("fad".to_string(), Entry { count: 2.0, last_used_secs: now - 10 }),
        ];
        assert_eq!(evict_candidate(&entries, now).map(|s| s.as_str()), Some("fad"));
    }

    #[test]
    fn turning_it_off_changes_nothing_at_all() {
        // "Cheap no-ops — no allocations, no map lookups, no I/O." On a phone
        // keyboard this runs on every commit, so a disabled feature that still
        // works is a battery cost for something the user declined.
        let original = vec![cand("b", 1.0), cand("a", 2.0)];
        let mut c = original.clone();
        apply_to_candidates(&mut c, false, |_| 0.5, |_| 0.5);
        assert_eq!(c, original, "a disabled feature reordered the candidates");
    }

    #[test]
    fn an_empty_candidate_list_is_left_alone() {
        let mut c: Vec<WordCandidate> = Vec::new();
        apply_to_candidates(&mut c, true, |_| 0.5, |_| 0.5);
        assert!(c.is_empty());
        assert_eq!(winner(&c), None);
    }

    #[test]
    fn a_tie_keeps_the_recognizers_own_ordering() {
        // The recognizer knows something about the PATH that the frequency
        // model does not, so it should break its own ties.
        let mut c = vec![cand("first", 1.0), cand("second", 1.0)];
        apply_to_candidates(&mut c, true, |_| 0.0, |_| 0.0);
        assert_eq!(winner(&c), Some("first"));
    }

    #[test]
    fn context_and_frequency_both_apply() {
        // The n-gram knows what usually FOLLOWS the previous words; the
        // dictionary knows what this user types at all. Either alone misses
        // half the signal.
        let mut c = vec![cand("bank", 1.0), cand("tank", 1.0)];
        apply_to_candidates(&mut c, true,
            |_| 0.0,
            |w| if w == "tank" { 0.4 } else { 0.0 });
        assert_eq!(winner(&c), Some("tank"));
    }

    #[test]
    fn the_entry_cap_is_a_phone_sized_number() {
        // 1500 words is a working vocabulary, not a corpus. Unbounded growth
        // on a keyboard process is memory the compositor eventually reclaims by
        // killing it.
        assert_eq!(MAX_ENTRIES, 1500);
    }
}
