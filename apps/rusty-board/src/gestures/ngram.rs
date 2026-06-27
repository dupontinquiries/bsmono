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
        if inner.bigrams.len() > MAX_BIGRAMS {
            evict_lowest(&mut inner.bigrams, now);
        }
        if inner.trigrams.len() > MAX_TRIGRAMS {
            evict_lowest(&mut inner.trigrams, now);
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
    pub fn score(
        &self,
        prev_prev: Option<&str>,
        prev: Option<&str>,
        word: &str,
    ) -> f64 {
        let word = word.to_lowercase();
        let prev = prev.map(|s| s.to_lowercase());
        let prev_prev = prev_prev.map(|s| s.to_lowercase());
        let inner = self.inner.read();
        let now = now_secs();

        // Try trigram first.
        if let (Some(pp), Some(p)) = (&prev_prev, &prev) {
            let key = trigram_key(pp, p, &word);
            if let Some(entry) = inner.trigrams.get(&key) {
                let eff = effective_count(entry, now);
                // Normalize: total trigrams starting with (pp, p).
                let prefix = format!("{}\x1f{}\x1f", pp, p);
                let total: f64 = inner
                    .trigrams
                    .iter()
                    .filter(|(k, _)| k.starts_with(&prefix))
                    .map(|(_, e)| effective_count(e, now))
                    .sum();
                if total > 0.0 {
                    let p_score = eff / total;
                    return (MAX_BOOST * p_score).min(MAX_BOOST);
                }
            }
        }

        // Backoff: bigram.
        if let Some(p) = &prev {
            let key = bigram_key(p, &word);
            if let Some(entry) = inner.bigrams.get(&key) {
                let eff = effective_count(entry, now);
                let prefix = format!("{}\x1f", p);
                let total: f64 = inner
                    .bigrams
                    .iter()
                    .filter(|(k, _)| k.starts_with(&prefix))
                    .map(|(_, e)| effective_count(e, now))
                    .sum();
                if total > 0.0 {
                    let p_score = eff / total;
                    // Bigram backoff weights less than trigram by a
                    // small factor (less context → less confidence).
                    return (MAX_BOOST * p_score * 0.75).min(MAX_BOOST);
                }
            }
        }

        0.0
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
        core_kit::time::now_secs() as i64
    }

fn effective_count(entry: &CountedEntry, now: i64) -> f64 {
    let age = (now - entry.last_seen).max(0) as f64;
    let half_lives = age / DECAY_HALF_LIFE_SECS;
    (entry.count as f64) * 0.5_f64.powf(half_lives)
}

fn evict_lowest(entries: &mut HashMap<String, CountedEntry>, now: i64) {
    let Some((victim, _)) = entries
        .iter()
        .map(|(k, v)| (k.clone(), effective_count(v, now)))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    else {
        return;
    };
    entries.remove(&victim);
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
        for i in 0..MAX_BIGRAMS + 50 {
            m.record(None, Some(&format!("p{}", i)), &format!("n{}", i));
        }
        // Should be at or below cap.
        assert!(m.bigram_count() <= MAX_BIGRAMS);
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
