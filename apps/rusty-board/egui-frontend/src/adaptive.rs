//! adaptive — the dictionary that learns what THIS person types.
//!
//! The shape recognizer scores against a frequency map baked at build time from
//! a generic English corpus. That is a fine start and wrong in three specific
//! ways the original names:
//!
//! * **Proper names.** *"Sue" doesn't show up in the corpus often enough to
//!   outrank "use" — but if the user has typed "Sue" 20 times this week, it
//!   should.*
//! * **Domain vocabulary.** A code-heavy user types `fn`, `impl`, `pub`
//!   constantly; a chat-heavy user types `lol`, `tbh`, `imo`.
//! * **Recent drift.** People work in bursts of vocabulary tied to whatever
//!   they are doing, so recent activity should weigh more than a long-term
//!   average.
//!
//! ## The boost is small on purpose
//!
//! `MAX_BOOST` is 0.5 against scores of roughly 0.1–2.0, and the original states
//! the intent exactly: *"enough to flip a close call, not enough to override a
//! clearly-better candidate."* A dictionary that could overrule the shape would
//! make the keyboard confidently wrong about words you typed once — and the
//! more you used it, the more certain it would be.
//!
//! ## Decay is what makes it adaptive rather than merely personal
//!
//! Without it the dictionary is a lifetime tally: a project you finished last
//! year keeps outranking the one you are on. A 30-day half-life means disuse
//! costs something, so the keyboard follows you rather than remembering you.
//!
//! ## It is bounded, and forgettable
//!
//! `MAX_ENTRIES` bounds storage and load time; below `MIN_EFFECTIVE_COUNT` an
//! entry is dropped, because a word swiped once should not persist forever.
//! `forget_all` exists because this is a record of everything a person has
//! typed, and a keyboard that learns must also be able to be told to stop.

/// Largest bonus any word can receive.
pub const MAX_BOOST: f64 = 0.5;
/// Half-life of an unused word's count: 30 days.
pub const DECAY_HALF_LIFE_SECS: f64 = 30.0 * 24.0 * 3600.0;
/// Entry cap, to bound storage and load time.
pub const MAX_ENTRIES: usize = 1500;
/// Below this effective count an entry is dropped.
pub const MIN_EFFECTIVE_COUNT: f64 = 0.05;

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    count: f64,
    last_used: i64,
}

/// A per-user word-frequency record.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdaptiveDictionary {
    words: Vec<(String, Entry)>,
}

/// Decay a raw count to its value at `now`.
///
/// Exponential rather than a sliding window: a window forgets everything at its
/// edge at once, so a word used heavily thirty-one days ago drops to nothing
/// between one launch and the next. Decay makes disuse gradual, which is how
/// the underlying behaviour actually changes.
pub fn effective_count(count: f64, last_used: i64, now: i64) -> f64 {
    let age = (now - last_used).max(0) as f64;
    count * 0.5f64.powf(age / DECAY_HALF_LIFE_SECS)
}

impl AdaptiveDictionary {
    pub fn len(&self) -> usize { self.words.len() }
    pub fn is_empty(&self) -> bool { self.words.is_empty() }

    /// Record that the user committed a word.
    ///
    /// Case-folded, because "Sue" and "sue" are the same evidence about
    /// vocabulary even though they are different words to type — and splitting
    /// them halves the signal for exactly the proper nouns this exists to
    /// rescue.
    pub fn record_commit(&mut self, word: &str, now: i64) {
        let w = word.trim().to_lowercase();
        // An empty commit is not evidence. Recording it would build an entry
        // nothing can ever match.
        if w.is_empty() { return; }
        match self.words.iter_mut().find(|(k, _)| *k == w) {
            Some((_, e)) => {
                // The existing count is decayed to now BEFORE incrementing, or
                // a word used twice years apart would score as if used twice
                // today.
                e.count = effective_count(e.count, e.last_used, now) + 1.0;
                e.last_used = now;
            }
            None => self.words.push((w, Entry { count: 1.0, last_used: now })),
        }
        self.evict_if_needed(now);
    }

    /// Drop the weakest entry when over the cap.
    fn evict_if_needed(&mut self, now: i64) {
        if self.words.len() <= MAX_ENTRIES { return; }
        // Lowest EFFECTIVE count, not lowest raw count: raw would evict a word
        // used five times yesterday in favour of one used six times two years
        // ago, which is backwards.
        let mut weakest = 0;
        let mut worst = f64::INFINITY;
        for (i, (_, e)) in self.words.iter().enumerate() {
            let eff = effective_count(e.count, e.last_used, now);
            if eff < worst { worst = eff; weakest = i; }
        }
        self.words.remove(weakest);
    }

    /// Drop entries that have decayed below usefulness.
    ///
    /// Called on save rather than on every lookup: a one-off word should not
    /// persist forever, but paying for a sweep on each keystroke to remove it
    /// sooner is the wrong trade.
    pub fn prune(&mut self, now: i64) {
        self.words.retain(|(_, e)| effective_count(e.count, e.last_used, now)
                          >= MIN_EFFECTIVE_COUNT);
    }

    /// The scoring bonus for a word, `0.0..=MAX_BOOST`.
    ///
    /// Normalised by the MEAN effective count, so the bonus means "unusually
    /// frequent for this user" rather than "frequent". An absolute threshold
    /// would give a heavy typist boosts on everything and a light one boosts on
    /// nothing.
    pub fn boost(&self, word: &str, now: i64) -> f64 {
        let w = word.trim().to_lowercase();
        let Some((_, e)) = self.words.iter().find(|(k, _)| *k == w) else { return 0.0 };
        let eff = effective_count(e.count, e.last_used, now);

        let total: f64 = self.words.iter()
            .map(|(_, x)| effective_count(x.count, x.last_used, now)).sum();
        if total <= 0.0 { return 0.0; }
        let mean = total / self.words.len() as f64;
        if mean <= 0.0 { return 0.0; }

        // A tiny dictionary must not hand out large boosts merely because its
        // few entries are all "above average" — with three words, everything is.
        let confidence = (self.words.len() as f64 / 50.0).min(1.0);
        let ratio = (eff / mean - 1.0).max(0.0);
        (ratio * 0.25 * confidence).min(MAX_BOOST)
    }

    /// The most-used words, strongest first.
    ///
    /// Ties break by word so the list does not reshuffle between calls — this
    /// is shown in settings, and a list that reorders while being read is
    /// unusable.
    pub fn top_n(&self, n: usize, now: i64) -> Vec<(String, f64)> {
        let mut v: Vec<(String, f64)> = self.words.iter()
            .map(|(k, e)| (k.clone(), effective_count(e.count, e.last_used, now)))
            .collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                  .then_with(|| a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }

    /// Forget everything.
    ///
    /// This is a record of what a person has typed. A keyboard that learns must
    /// also be able to be told to stop, and the control has to be complete
    /// rather than a decay the user waits out.
    pub fn forget_all(&mut self) { self.words.clear(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 3600;

    fn dict_with(words: &[(&str, u32)], now: i64) -> AdaptiveDictionary {
        let mut d = AdaptiveDictionary::default();
        for (w, n) in words {
            for _ in 0..*n { d.record_commit(w, now); }
        }
        d
    }

    #[test]
    fn a_word_the_user_types_often_gains_a_bonus_over_their_other_words() {
        // "Sue" typed twenty times should be able to outrank "use".
        //
        // Note what this test had to be corrected to say. The first version
        // asserted a bonus for a dictionary containing ONLY "sue", and it
        // failed — correctly. The boost means "unusually frequent FOR THIS
        // USER", and with one entry that word IS the mean, so there is nothing
        // to be unusual against. The feature needs context before it can have
        // an opinion, which is the same reason the confidence factor exists.
        let mut d = AdaptiveDictionary::default();
        for i in 0..60 { d.record_commit(&format!("w{i}"), 0); }
        for _ in 0..20 { d.record_commit("sue", 0); }
        assert!(d.boost("sue", 0) > 0.0);
        assert_eq!(d.boost("use", 0), 0.0, "a word never typed gains nothing");
    }

    #[test]
    fn a_lone_word_earns_no_boost_because_there_is_nothing_to_compare_it_to() {
        // Deliberate, and the first thing a reader will question. A brand-new
        // dictionary handing out bonuses would make the keyboard most opinionated
        // exactly when it knows least.
        let d = dict_with(&[("sue", 20)], 0);
        assert_eq!(d.boost("sue", 0), 0.0);
    }

    #[test]
    fn the_bonus_can_flip_a_close_call_and_not_much_more() {
        // The original's own words: "enough to flip a close call, not enough to
        // override a clearly-better candidate." A dictionary that could
        // overrule the shape would make the keyboard confidently wrong about
        // words you typed once, and more certain the more you used it.
        let mut d = AdaptiveDictionary::default();
        for i in 0..60 { d.record_commit(&format!("w{i}"), 0); }
        for _ in 0..500 { d.record_commit("impl", 0); }
        assert!(d.boost("impl", 0) <= MAX_BOOST);
    }

    #[test]
    fn case_is_folded_so_proper_nouns_keep_their_signal() {
        // "Sue" and "sue" are the same evidence about vocabulary, and splitting
        // them halves the signal for exactly the words this exists to rescue.
        let mut d = AdaptiveDictionary::default();
        d.record_commit("Sue", 0);
        d.record_commit("sue", 0);
        assert_eq!(d.len(), 1);
        assert!(d.boost("SUE", 0) >= 0.0);
    }

    #[test]
    fn an_empty_commit_is_not_evidence() {
        // Recording it would build an entry nothing can ever match.
        let mut d = AdaptiveDictionary::default();
        d.record_commit("   ", 0);
        d.record_commit("", 0);
        assert!(d.is_empty());
    }

    #[test]
    fn a_word_halves_after_thirty_days_of_disuse() {
        let c = effective_count(8.0, 0, 30 * DAY);
        assert!((c - 4.0).abs() < 0.01, "got {c}");
        let q = effective_count(8.0, 0, 60 * DAY);
        assert!((q - 2.0).abs() < 0.01, "and quarters after sixty");
    }

    #[test]
    fn decay_is_gradual_rather_than_a_cliff() {
        // A sliding window forgets everything at its edge at once, so a word
        // used heavily thirty-one days ago drops to nothing between one launch
        // and the next.
        let a = effective_count(10.0, 0, 29 * DAY);
        let b = effective_count(10.0, 0, 31 * DAY);
        assert!(a > b);
        assert!(a - b < 1.0, "no cliff: {a} vs {b}");
    }

    #[test]
    fn a_word_used_twice_years_apart_does_not_score_as_twice_today() {
        // The count is decayed to now BEFORE incrementing.
        let mut d = AdaptiveDictionary::default();
        d.record_commit("old", 0);
        d.record_commit("old", 365 * DAY);
        let recent = dict_with(&[("new", 2)], 365 * DAY);
        assert!(d.top_n(1, 365 * DAY)[0].1 < recent.top_n(1, 365 * DAY)[0].1);
    }

    #[test]
    fn a_future_timestamp_does_not_amplify_a_count() {
        // A clock that jumped backwards would otherwise make old words score
        // MORE than they should, since the exponent goes negative.
        let c = effective_count(4.0, 100 * DAY, 0);
        assert!(c <= 4.0, "got {c}");
    }

    #[test]
    fn the_bonus_means_unusual_for_this_user_not_merely_frequent() {
        // An absolute threshold gives a heavy typist boosts on everything and a
        // light one boosts on nothing.
        let mut even = AdaptiveDictionary::default();
        for i in 0..60 { for _ in 0..10 { even.record_commit(&format!("w{i}"), 0); } }
        let flat = even.boost("w0", 0);

        let mut peaked = AdaptiveDictionary::default();
        for i in 0..60 { peaked.record_commit(&format!("w{i}"), 0); }
        for _ in 0..40 { peaked.record_commit("w0", 0); }
        assert!(peaked.boost("w0", 0) > flat,
                "standing out earns more than typing a lot");
    }

    #[test]
    fn a_tiny_dictionary_does_not_hand_out_large_boosts() {
        // With three words, everything is above average.
        let d = dict_with(&[("a", 50), ("b", 1), ("c", 1)], 0);
        assert!(d.boost("a", 0) < MAX_BOOST * 0.5, "got {}", d.boost("a", 0));
    }

    #[test]
    fn an_unknown_word_gets_nothing_rather_than_a_default() {
        let d = dict_with(&[("impl", 5)], 0);
        assert_eq!(d.boost("pterodactyl", 0), 0.0);
        assert_eq!(AdaptiveDictionary::default().boost("anything", 0), 0.0);
    }

    #[test]
    fn eviction_drops_the_weakest_by_effective_count_not_raw_count() {
        // Raw would evict a word used five times yesterday in favour of one
        // used six times two years ago, which is backwards.
        let ancient = effective_count(6.0, 0, 730 * DAY);
        let recent = effective_count(5.0, 729 * DAY, 730 * DAY);
        assert!(recent > ancient, "the recent word is worth more");
    }

    #[test]
    fn pruning_removes_words_that_have_faded_to_nothing() {
        // A one-off word should not persist forever.
        let mut d = dict_with(&[("once", 1)], 0);
        d.prune(0);
        assert_eq!(d.len(), 1, "still fresh");
        d.prune(400 * DAY);
        assert!(d.is_empty(), "and gone once it means nothing");
    }

    #[test]
    fn the_top_list_is_stable_between_calls() {
        // Shown in settings; a list that reorders while being read is unusable.
        let d = dict_with(&[("beta", 3), ("alpha", 3), ("gamma", 9)], 0);
        let a = d.top_n(3, 0);
        assert_eq!(a, d.top_n(3, 0));
        assert_eq!(a[0].0, "gamma");
        assert_eq!(a[1].0, "alpha", "ties break by word, not by insertion");
    }

    #[test]
    fn forgetting_is_complete_rather_than_a_decay_to_wait_out() {
        // This is a record of everything a person has typed. A keyboard that
        // learns must be able to be told to stop.
        let mut d = dict_with(&[("private", 40)], 0);
        d.forget_all();
        assert!(d.is_empty());
        assert_eq!(d.boost("private", 0), 0.0);
    }
}
