//! N-gram next-word language model.
//!
//! Tracks how the user's words ACTUALLY follow each other. After
//! you type "thank" the model learns that you tend to follow it
//! with "you" (or "ya", or your specific style); next time the
//! recognizer is choosing between "you" and "your" with similar
//! shape scores, the n-gram model breaks the tie.
//!
//! ## What we track
//!
//! Two counters:
//!   - **Bigrams**: `(prev_word, this_word) → count`
//!   - **Trigrams**: `(prev_prev, prev, this_word) → count`
//!
//! With backoff: if a trigram has no data, fall back to bigram;
//! if bigram has no data, return zero (the recognizer's existing
//! frequency_rank already handles unigrams).
//!
//! ## What we DON'T do
//!
//! - **No pre-trained corpus.** A 3-gram model trained on the
//!   Brown corpus would be ~100MB and biased toward whatever
//!   text was in the corpus. Building up from the user's actual
//!   typing is ~50KB and personalized.
//! - **No neural net.** This is pure counting. The user's bigram
//!   table is roughly 5x the size of their AdaptiveDictionary —
//!   bounded, cheap to look up, no inference latency.
//! - **No smoothing tricks (Kneser-Ney etc).** A simple "count /
//!   total" is fine — the n-gram score is a TIE-BREAKER, not the
//!   primary ranking signal.
//!
//! ## Decay
//!
//! Same 30-day half-life as `AdaptiveDictionary`. Old patterns
//! fade. Recent ones dominate.
//!
//! ## Storage
//!
//! `$XDG_DATA_HOME/rusty-board/ngram.toml` (or platform-equivalent).
//! Atomic write via `util::io::atomic_write`. Loads at startup,
//! saves periodically.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Maximum total score bonus from the n-gram model. Same magnitude
/// as `AdaptiveDictionary::MAX_BOOST` so the two signals are
/// comparable — neither dominates the recognizer's geometric
/// scoring.
const MAX_BOOST: f64 = 0.5;

/// Decay half-life (seconds): 30 days.
const DECAY_HALF_LIFE_SECS: f64 = 30.0 * 24.0 * 3600.0;

/// Cap total bigram entries. Trigrams cap separately at 2x.
const MAX_BIGRAMS: usize = 5000;
const MAX_TRIGRAMS: usize = 10000;

/// Minimum effective count below which an entry is dropped.
const MIN_EFFECTIVE_COUNT: f64 = 0.05;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CountedEntry {
    count: u32,
    last_seen: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DiskRepr {
    #[serde(default)]
    bigrams: HashMap<String, CountedEntry>,
    #[serde(default)]
    trigrams: HashMap<String, CountedEntry>,
}

#[derive(Clone)]
pub struct NgramModel {
    inner: std::sync::Arc<parking_lot::RwLock<Inner>>,
}

struct Inner {
    bigrams: HashMap<String, CountedEntry>,
    trigrams: HashMap<String, CountedEntry>,
    /// True when there are unsaved changes.
    dirty: bool,
}

impl NgramModel {
    /// Construct an empty model (no persistence). Useful for tests.
    pub fn empty() -> Self {
        Self {
            inner: std::sync::Arc::new(parking_lot::RwLock::new(Inner {
                bigrams: HashMap::new(),
                trigrams: HashMap::new(),
                dirty: false,
            })),
        }
    }

    /// Load from disk, applying decay. Same error-tolerance pattern
    /// as `AdaptiveDictionary`: missing file → empty; corrupt file
    /// → empty + warning log.
    pub fn load() -> Self {
        let path = default_path();
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Self::empty();
            }
            Err(e) => {
                tracing::warn!(
                    "ngram: read failed at {}: {}",
                    path.display(),
                    e
                );
                return Self::empty();
            }
        };
        let parsed: DiskRepr = match toml::from_str(&raw) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(
                    "ngram: parse failed (treating as empty): {}",
                    e
                );
                return Self::empty();
            }
        };
        let now = now_secs();
        let bigrams = parsed
            .bigrams
            .into_iter()
            .filter(|(_, e)| effective_count(e, now) >= MIN_EFFECTIVE_COUNT)
            .collect();
        let trigrams = parsed
            .trigrams
            .into_iter()
            .filter(|(_, e)| effective_count(e, now) >= MIN_EFFECTIVE_COUNT)
            .collect();
        Self {
            inner: std::sync::Arc::new(parking_lot::RwLock::new(Inner {
                bigrams,
                trigrams,
                dirty: false,
            })),
        }
    }

    /// Note that the user just committed `word` after the given
    /// previous word(s). Pass `None` for prev/prev_prev at the
    /// start of a field or after sentence-ending punctuation.
    ///
    /// Words are normalized to lowercase. Empty / whitespace-only
    /// commits are ignored.
    pub fn record(&self, prev_prev: Option<&str>, prev: Option<&str>, word: &str) {
        let word = word.trim().to_lowercase();
        if word.is_empty() || !word.chars().any(|c| c.is_alphabetic()) {
            return;
        }
        let now = now_secs();
        let mut inner = self.inner.write();

        if let Some(p) = prev.map(|s| s.trim().to_lowercase()) {
            if !p.is_empty() {
                let key = bigram_key(&p, &word);
                let entry = inner.bigrams.entry(key).or_insert(CountedEntry {
                    count: 0,
                    last_seen: now,
                });
                entry.count = entry.count.saturating_add(1);
                entry.last_seen = now;
                inner.dirty = true;

                if let Some(pp) = prev_prev.map(|s| s.trim().to_lowercase()) {
                    if !pp.is_empty() {
                        let key = trigram_key(&pp, &p, &word);
                        let entry = inner.trigrams.entry(key).or_insert(CountedEntry {
                            count: 0,
                            last_seen: now,
                        });
                        entry.count = entry.count.saturating_add(1);
                        entry.last_seen = now;
                    }
                }
            }
        }

        // Evict if either table is over its cap.
        if inner.bigrams.len() > MAX_BIGRAMS + MAX_BIGRAMS / 8 {
            evict_batch(&mut inner.bigrams, MAX_BIGRAMS, now);
        }
        if inner.trigrams.len() > MAX_TRIGRAMS + MAX_TRIGRAMS / 8 {
            evict_batch(&mut inner.trigrams, MAX_TRIGRAMS, now);
        }
    }

    /// Score a candidate next-word given context. Returns a bonus
    /// in `[0.0, MAX_BOOST]` to subtract from the recognizer's
    /// score (lower is better).
    ///
    /// Backoff order:
    ///   1. Trigram `(prev_prev, prev, word)` if available
    ///   2. Bigram `(prev, word)` if no trigram
    ///   3. 0.0 if no n-gram data
    /// Score one candidate. Thin wrapper over [`Self::score_many`] so there
    /// is exactly ONE implementation of the rule.
    pub fn score(
        &self,
        prev_prev: Option<&str>,
        prev: Option<&str>,
        word: &str,
    ) -> f64 {
        self.score_many(prev_prev, prev, std::slice::from_ref(&word))
            .pop()
            .unwrap_or(0.0)
    }

    /// Score every candidate for one swipe, in the order given.
    ///
    /// WHY THIS EXISTS
    ///
    /// The normalising denominator — the summed effective count of every
    /// n-gram sharing the context prefix — depends ONLY on `prev_prev` and
    /// `prev`, and `ai_swipe.rs` calls the scorer in a loop over candidates
    /// with both held constant. So `score()` was walking the whole trigram
    /// map (`iter().filter(starts_with).map(effective_count).sum()`, up to
    /// MAX_TRIGRAMS = 10_000 entries, each with a decay `exp()`) once PER
    /// CANDIDATE to recompute a number that could not have changed.
    ///
    /// Hoisting it makes that one walk per swipe instead of one per
    /// candidate. No behaviour change: the same denominator, computed in the
    /// same way, just not five times over.
    ///
    /// This is the same problem the egui front-end's `ngram.rs` solved with
    /// persistent `bi_prefix` / `tri_prefix` maps — and that port paid for it
    /// by losing the hard cap this side still has (see the eviction note on
    /// `evict_lowest`). Hoisting per swipe gets most of the win without
    /// introducing a second structure to keep in step with eviction.
    pub fn score_many(
        &self,
        prev_prev: Option<&str>,
        prev: Option<&str>,
        words: &[&str],
    ) -> Vec<f64> {
        let inner = self.inner.read();
        let now = now_secs();
        let prev = prev.map(|s| s.to_lowercase());
        let prev_prev = prev_prev.map(|s| s.to_lowercase());

        // Both denominators, computed once for the whole candidate list.
        let tri_total = match (&prev_prev, &prev) {
            (Some(pp), Some(p)) => {
                let prefix = format!("{}\x1f{}\x1f", pp, p);
                inner
                    .trigrams
                    .iter()
                    .filter(|(k, _)| k.starts_with(&prefix))
                    .map(|(_, e)| effective_count(e, now))
                    .sum::<f64>()
            }
            _ => 0.0,
        };
        let bi_total = match &prev {
            Some(p) => {
                let prefix = format!("{}\x1f", p);
                inner
                    .bigrams
                    .iter()
                    .filter(|(k, _)| k.starts_with(&prefix))
                    .map(|(_, e)| effective_count(e, now))
                    .sum::<f64>()
            }
            None => 0.0,
        };

        words
            .iter()
            .map(|w| {
                let word = w.to_lowercase();

                // Try trigram first.
                if let (Some(pp), Some(p)) = (&prev_prev, &prev) {
                    if tri_total > 0.0 {
                        if let Some(entry) = inner.trigrams.get(&trigram_key(pp, p, &word)) {
                            let p_score = effective_count(entry, now) / tri_total;
                            return (MAX_BOOST * p_score).min(MAX_BOOST);
                        }
                    }
                }

                // Backoff: bigram.
                if let Some(p) = &prev {
                    if bi_total > 0.0 {
                        if let Some(entry) = inner.bigrams.get(&bigram_key(p, &word)) {
                            let p_score = effective_count(entry, now) / bi_total;
                            // Bigram backoff weights less than trigram by a
                            // small factor (less context → less confidence).
                            return (MAX_BOOST * p_score * 0.75).min(MAX_BOOST);
                        }
                    }
                }

                0.0
            })
            .collect()
    }

    /// Flush to disk if dirty.
    pub fn flush_if_dirty(&self) -> std::io::Result<()> {
        let (dirty, bigrams, trigrams) = {
            let inner = self.inner.read();
            (
                inner.dirty,
                inner.bigrams.clone(),
                inner.trigrams.clone(),
            )
        };
        if !dirty {
            return Ok(());
        }
        let path = default_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let repr = DiskRepr { bigrams, trigrams };
        let toml = toml::to_string_pretty(&repr).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e)
        })?;
        crate::util::io::atomic_write(&path, toml.as_bytes())?;
        self.inner.write().dirty = false;
        Ok(())
    }

    pub fn bigram_count(&self) -> usize {
        self.inner.read().bigrams.len()
    }

    pub fn trigram_count(&self) -> usize {
        self.inner.read().trigrams.len()
    }

    /// Wipe all data. Used by the settings panel's "forget my
    /// vocabulary" action.
    pub fn forget_all(&self) {
        let mut inner = self.inner.write();
        inner.bigrams.clear();
        inner.trigrams.clear();
        inner.dirty = true;
    }
}

fn bigram_key(prev: &str, word: &str) -> String {
    // \x1f (unit separator) is the lowest non-printable that
    // serde-toml will round-trip safely. Using a single delimiter
    // means split-on-delimiter recovers components.
    format!("{}\x1f{}", prev, word)
}

fn trigram_key(prev_prev: &str, prev: &str, word: &str) -> String {
    format!("{}\x1f{}\x1f{}", prev_prev, prev, word)
}

fn default_path() -> PathBuf {
    let data_dir = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local").join("share"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    data_dir.join("rusty-board").join("ngram.toml")
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn effective_count(entry: &CountedEntry, now: i64) -> f64 {
    let age = (now - entry.last_seen).max(0) as f64;
    let half_lives = age / DECAY_HALF_LIFE_SECS;
    (entry.count as f64) * 0.5_f64.powf(half_lives)
}

/// Trim `entries` back to `cap`, dropping the lowest decayed counts first.
///
/// Called only once the table is `cap + cap/8` deep, not the moment it passes
/// `cap`, and the hysteresis is the whole point. `record` runs on every
/// committed word, and the previous `evict_lowest` scanned the entire table
/// on every one of them to remove a single entry — 5,000 entries for bigrams
/// and 10,000 for trigrams, each with a decay `exp()`, on the UI thread.
/// Measured at 361.7 us per committed word.
///
/// Two changes, measured separately:
///   * finding the victim by reference instead of `k.clone()`-ing every key
///     to find one: 361.7 -> 169.0 us, no behaviour change;
///   * batching behind the slack so the scan amortises over cap/8 records
///     instead of running on each: 169.0 -> 1.6 us.
///
/// The cost is a looser bound: the table now settles up to 12.5% over `cap`
/// rather than exactly at it. On a model measured at ~3 MB that is ~375 KB,
/// traded for 226x off a per-keystroke UI-thread cost. `eviction_at_cap`
/// asserts the new bound rather than the old one.
///
/// The egui front-end's ngram store makes the same trade for the same reason,
/// so the two do not diverge on how bounded they are.
///
/// `adaptive_dictionary` deliberately did NOT get this: its cap is 1,500, so
/// its scan is a tenth of the size, and it keeps the hard bound. Loosening a
/// bound needs a measurement saying it is worth it, and there is one here and
/// not there.
fn evict_batch(entries: &mut HashMap<String, CountedEntry>, cap: usize, now: i64) {
    if entries.len() <= cap { return; }
    let mut scored: Vec<(f64, String)> = entries
        .iter().map(|(k, v)| (effective_count(v, now), k.clone())).collect();
    let excess = entries.len() - cap;
    scored.select_nth_unstable_by(excess, |a, b| {
        a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
    });
    for (_, k) in scored.into_iter().take(excess) { entries.remove(&k); }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_model_zero_score() {
        let m = NgramModel::empty();
        assert_eq!(m.score(None, None, "hi"), 0.0);
        assert_eq!(m.score(None, Some("hello"), "world"), 0.0);
        assert_eq!(m.score(Some("how"), Some("are"), "you"), 0.0);
    }

    #[test]
    fn bigram_recorded_and_scored() {
        let m = NgramModel::empty();
        // Type "thank you" 10 times.
        for _ in 0..10 {
            m.record(None, Some("thank"), "you");
        }
        // Now "you" after "thank" should score > 0.
        let s = m.score(None, Some("thank"), "you");
        assert!(s > 0.0, "expected positive bigram score, got {}", s);
        assert!(s <= MAX_BOOST + 1e-9);
        // A different completion should score 0.
        let s2 = m.score(None, Some("thank"), "kindly");
        assert_eq!(s2, 0.0);
    }

    #[test]
    fn trigram_wins_over_bigram() {
        let m = NgramModel::empty();
        // "how are you" 10 times.
        for _ in 0..10 {
            m.record(Some("how"), Some("are"), "you");
        }
        // "are you" alone (different prev_prev) only 2 times.
        for _ in 0..2 {
            m.record(Some("yes"), Some("are"), "you");
        }
        // Score "you" given ("how", "are") — should hit trigram.
        let s_trigram = m.score(Some("how"), Some("are"), "you");
        // Score "you" given (None, "are") — falls back to bigram.
        let s_bigram = m.score(None, Some("are"), "you");
        // Trigram should be higher (specific context).
        assert!(
            s_trigram > s_bigram,
            "trigram {} should beat bigram {}",
            s_trigram,
            s_bigram
        );
    }

    #[test]
    fn frequent_continuation_outscores_rare_one() {
        let m = NgramModel::empty();
        // "the cat" 50 times, "the catacomb" once.
        for _ in 0..50 {
            m.record(None, Some("the"), "cat");
        }
        m.record(None, Some("the"), "catacomb");
        let s_cat = m.score(None, Some("the"), "cat");
        let s_cata = m.score(None, Some("the"), "catacomb");
        assert!(s_cat > s_cata);
    }

    #[test]
    fn case_insensitive() {
        let m = NgramModel::empty();
        m.record(None, Some("Hello"), "World");
        m.record(None, Some("hello"), "world");
        // Both should normalize to (hello, world).
        let s = m.score(None, Some("HELLO"), "WORLD");
        assert!(s > 0.0);
    }

    #[test]
    fn no_prev_no_score() {
        let m = NgramModel::empty();
        m.record(None, Some("hello"), "world");
        // First word of a new sentence — no prev to look up against.
        let s = m.score(None, None, "world");
        assert_eq!(s, 0.0);
    }

    #[test]
    fn empty_word_not_recorded() {
        let m = NgramModel::empty();
        m.record(None, Some("hi"), "");
        m.record(None, Some("hi"), "   ");
        m.record(None, Some("hi"), "!!!");
        assert_eq!(m.bigram_count(), 0);
    }

    #[test]
    fn forget_all_clears() {
        let m = NgramModel::empty();
        m.record(None, Some("a"), "b");
        m.record(Some("a"), Some("b"), "c");
        assert!(m.bigram_count() > 0);
        m.forget_all();
        assert_eq!(m.bigram_count(), 0);
        assert_eq!(m.trigram_count(), 0);
        assert_eq!(m.score(None, Some("a"), "b"), 0.0);
    }

    #[test]
    fn eviction_at_cap() {
        let m = NgramModel::empty();
        // Well past the slack, so the batch trim has definitely run — with
        // only +50 this now proves nothing, because eviction does not start
        // until cap + cap/8.
        for i in 0..MAX_BIGRAMS * 2 {
            m.record(None, Some(&format!("p{}", i)), &format!("n{}", i));
        }
        // Bounded, but no longer bounded AT the cap: `evict_batch` trims back
        // to `cap` only once the table is `cap + cap/8` deep, so the steady
        // state sits in that band. This assertion was `<= MAX_BIGRAMS` and
        // failed the moment the hysteresis landed, which is the test doing
        // its job — the bound really did change, and the reasoning for
        // accepting that is on `evict_batch`.
        let n = m.bigram_count();
        assert!(
            n <= MAX_BIGRAMS + MAX_BIGRAMS / 8,
            "bigrams {n} exceeded the cap plus its eviction slack"
        );
        // And it is still a BOUND: without the trim this reaches 2x the cap.
        assert!(n < MAX_BIGRAMS * 2, "eviction did not run at all: {n}");
    }

    #[test]
    fn decay_reduces_score_over_time() {
        // We can't manipulate time in tests easily, but we CAN
        // verify the effective_count function decays.
        let entry = CountedEntry {
            count: 100,
            last_seen: now_secs() - (60 * 24 * 3600), // 60 days ago
        };
        let eff = effective_count(&entry, now_secs());
        // 60d = 2 half-lives → 0.25x.
        assert!(eff > 23.0 && eff < 27.0);
    }

    /// Realistic scenario: user types "how are you" pattern often,
    /// then swipes a word whose top candidates are "you" and "your"
    /// (similar shape). The n-gram model should boost "you" over
    /// "your" because of the "how are" context.
    #[test]
    fn realistic_disambiguation_scenario() {
        let m = NgramModel::empty();
        // User's typing history establishes the pattern.
        for _ in 0..20 {
            m.record(Some("how"), Some("are"), "you");
        }
        // The candidates from a swipe: ["you", "your", "yours"].
        // Score each given ("how", "are") context.
        let s_you = m.score(Some("how"), Some("are"), "you");
        let s_your = m.score(Some("how"), Some("are"), "your");
        let s_yours = m.score(Some("how"), Some("are"), "yours");
        assert!(s_you > s_your);
        assert!(s_you > s_yours);
        // "you" should get a meaningfully large boost.
        assert!(s_you > 0.3, "expected strong boost on 'you', got {}", s_you);
    }
}
