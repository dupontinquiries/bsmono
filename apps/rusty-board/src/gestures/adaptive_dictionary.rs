//! Per-user adaptive dictionary.
//!
//! Tracks which words THIS user actually swipes, and surfaces that
//! signal to the recognizer so user-recent words get a scoring
//! bonus over generic-corpus frequencies.
//!
//! ## Why
//!
//! The shape-based recognizer uses a frequency map baked at build
//! time — common-English word frequencies from a generic corpus.
//! That's a fine starting point but it's wrong in important ways:
//!
//! - **Proper names**: "Sue" doesn't show up in the corpus often
//!   enough to outrank "use" but if the user has typed "Sue" 20
//!   times this week, it should.
//! - **Domain vocab**: a code-heavy user types "fn", "impl", "pub"
//!   constantly; a chat-heavy user types "lol", "tbh", "imo". The
//!   generic corpus doesn't reflect this.
//! - **Recent context drift**: people use bursts of vocabulary
//!   tied to whatever they're working on. The recognizer should
//!   weight the user's RECENT activity heavier than their
//!   long-term average.
//!
//! ## Design
//!
//! Each commit increments a counter for that word. The dictionary
//! file is a sorted-by-count list of `(word, count, last_seen_ts)`.
//! Persistence is via the existing `util::io::atomic_write` helper
//! so we never leave a corrupt file behind on crash.
//!
//! The recognizer-facing API is `boost(word)`: returns a small
//! score adjustment in `[0.0, MAX_BOOST]` based on how recently
//! and how often the user typed this word. Combined with the
//! recognizer's existing frequency_rank to refine ranking when
//! the top candidates are otherwise close.
//!
//! ## Decay
//!
//! Counts decay over time so a one-off use of "antidisestablishmentarianism"
//! doesn't bias the recognizer forever. Specifically: every load
//! applies an exponential decay based on `(now - last_seen_ts)`,
//! with a half-life of ~30 days. Stuff you typed yesterday counts
//! 1.0×; stuff from a month ago counts 0.5×; stuff from six months
//! ago counts ~0.06×.
//!
//! ## Storage location
//!
//! `$XDG_DATA_HOME/rusty-board/adaptive_dictionary.toml` (or
//! `~/.local/share/...` if XDG_DATA_HOME isn't set). TOML over JSON
//! because Rust + serde + TOML is what the rest of the project uses
//! and the read/write performance difference for ~1000 entries is
//! negligible.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Maximum score bonus a single word can earn. Recognizer scores
/// are lower-is-better and typical inter-candidate differences are
/// ~0.1 to ~2.0. A bonus of 0.5 is "enough to flip a close call,
/// not enough to override a clearly-better candidate."
const MAX_BOOST: f64 = 0.5;

/// Decay half-life in seconds: 30 days. After 30 days of disuse a
/// word's count is halved; after 60 days it's quartered.
const DECAY_HALF_LIFE_SECS: f64 = 30.0 * 24.0 * 3600.0;

/// Cap total entries to bound storage size + load time. When
/// the cap is hit, the lowest-effective-count entry is evicted.
/// 1500 covers a typical user's active vocabulary comfortably.
const MAX_ENTRIES: usize = 1500;

/// Minimum effective count below which we don't bother surfacing
/// the entry at all (drop it on next save). One-off rare words
/// shouldn't persist forever.
const MIN_EFFECTIVE_COUNT: f64 = 0.05;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    count: u32,
    /// Unix epoch seconds.
    last_seen: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DiskRepr {
    #[serde(default)]
    entries: HashMap<String, Entry>,
}

/// The adaptive dictionary itself. Cheap to clone (entries are
/// behind an `Arc<RwLock>` so multiple call sites share state).
#[derive(Clone)]
pub struct AdaptiveDictionary {
    inner: std::sync::Arc<parking_lot::RwLock<Inner>>,
}

struct Inner {
    entries: HashMap<String, Entry>,
    /// Total effective count across all entries — used to
    /// normalize `boost` so adding more entries doesn't change
    /// the boost magnitude for any single entry.
    total_effective: f64,
    /// True when there are unsaved changes. `flush_if_dirty`
    /// writes only when this is set.
    dirty: bool,
}

impl AdaptiveDictionary {
    /// Load from the default path, applying decay to all entries.
    /// Missing file → empty dictionary. Corrupt file → empty
    /// dictionary + log warning (we don't crash the keyboard
    /// because of a malformed adaptive dict).
    pub fn load() -> Self {
        let path = default_path();
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Self::empty();
            }
            Err(e) => {
                tracing::warn!(
                    "adaptive-dict: read failed at {}: {}",
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
                    "adaptive-dict: parse failed (treating as empty): {}",
                    e
                );
                return Self::empty();
            }
        };
        // Apply decay and prune.
        let now = now_secs();
        let mut entries: HashMap<String, Entry> = HashMap::new();
        for (word, entry) in parsed.entries {
            if effective_count(&entry, now) >= MIN_EFFECTIVE_COUNT {
                entries.insert(word, entry);
            }
        }
        let total_effective = entries
            .values()
            .map(|e| effective_count(e, now))
            .sum::<f64>();
        Self {
            inner: std::sync::Arc::new(parking_lot::RwLock::new(Inner {
                entries,
                total_effective,
                dirty: false,
            })),
        }
    }

    /// In-memory empty dictionary. Useful for testing and as the
    /// fallback when the disk file is unreadable/corrupt.
    pub fn empty() -> Self {
        Self {
            inner: std::sync::Arc::new(parking_lot::RwLock::new(Inner {
                entries: HashMap::new(),
                total_effective: 0.0,
                dirty: false,
            })),
        }
    }

    /// Note that the user committed this word. Increments count
    /// and updates last_seen.
    pub fn record_commit(&self, word: &str) {
        // Normalize: dictionary is case-insensitive; "Sue" and
        // "sue" share an entry. The recognizer always returns
        // lowercase candidates anyway.
        let key = word.to_lowercase();
        if key.is_empty() {
            return;
        }
        // Don't pollute the dictionary with whitespace-only or
        // pure-punctuation "commits".
        if !key.chars().any(|c| c.is_alphabetic()) {
            return;
        }
        let now = now_secs();
        let mut inner = self.inner.write();
        let entry = inner.entries.entry(key).or_insert(Entry {
            count: 0,
            last_seen: now,
        });
        entry.count = entry.count.saturating_add(1);
        entry.last_seen = now;
        inner.dirty = true;
        // Recompute total_effective lazily on the next boost()
        // call. Setting it to NaN is a marker; we check for NaN
        // on read and recompute if so. Cheaper than recomputing
        // on every commit.
        inner.total_effective = f64::NAN;
        // Evict if over the cap.
        if inner.entries.len() > MAX_ENTRIES {
            evict_lowest(&mut inner.entries, now);
        }
    }

    /// Score bonus to subtract from the recognizer's score for
    /// `word` (lower is better in the recognizer's scoring
    /// convention, so a positive boost makes the word more
    /// attractive). Range: `[0.0, MAX_BOOST]`.
    ///
    /// Words with no entry get 0.0. Words with effective count
    /// far above the mean get the full MAX_BOOST. Cheap: a single
    /// HashMap lookup + a log/exp.
    pub fn boost(&self, word: &str) -> f64 {
        let key = word.to_lowercase();
        let mut inner = self.inner.write();
        // Lazy total_effective recompute (NaN marker after a commit).
        if !inner.total_effective.is_finite() {
            let now = now_secs();
            inner.total_effective = inner
                .entries
                .values()
                .map(|e| effective_count(e, now))
                .sum();
        }
        let Some(entry) = inner.entries.get(&key) else {
            return 0.0;
        };
        let now = now_secs();
        let eff = effective_count(entry, now);
        // Normalize by mean effective count: a word that's at the
        // mean gets a small boost; a word that's many σ above
        // mean gets close to MAX_BOOST.
        if inner.total_effective < 1.0 {
            // Tiny dictionary — first few entries shouldn't get
            // a huge boost just because they're first.
            return (eff / 10.0).min(MAX_BOOST);
        }
        let n = inner.entries.len() as f64;
        let mean = inner.total_effective / n;
        // Boost = MAX_BOOST * tanh((eff - mean) / mean), so words
        // at the mean get 0, words 2σ above get ~0.4 of MAX_BOOST,
        // words 5σ above get ~0.46.
        let z = if mean > 0.0 { (eff - mean) / mean } else { 0.0 };
        let bonus = MAX_BOOST * z.tanh().max(0.0);
        bonus.min(MAX_BOOST)
    }

    /// Save changes to disk if dirty. No-op if there are no
    /// pending changes. Atomic: writes to a temp file and renames.
    pub fn flush_if_dirty(&self) -> std::io::Result<()> {
        let (dirty, entries) = {
            let inner = self.inner.read();
            (inner.dirty, inner.entries.clone())
        };
        if !dirty {
            return Ok(());
        }
        let path = default_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let repr = DiskRepr { entries };
        let toml = toml::to_string_pretty(&repr).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e)
        })?;
        crate::util::io::atomic_write(&path, toml.as_bytes())?;
        self.inner.write().dirty = false;
        Ok(())
    }

    /// Diagnostics: count of entries in the dictionary.
    pub fn len(&self) -> usize {
        self.inner.read().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Diagnostics: top N entries by effective count, for surfacing
    /// in the settings panel ("your most-used swipe words").
    pub fn top_n(&self, n: usize) -> Vec<(String, f64)> {
        let inner = self.inner.read();
        let now = now_secs();
        let mut all: Vec<(String, f64)> = inner
            .entries
            .iter()
            .map(|(w, e)| (w.clone(), effective_count(e, now)))
            .collect();
        all.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        all.into_iter().take(n).collect()
    }

    /// Wipe the dictionary. Used by the settings panel's "forget my
    /// vocabulary" action. Marks dirty so the next flush writes an
    /// empty file (rather than leaving the old data on disk).
    pub fn forget_all(&self) {
        let mut inner = self.inner.write();
        inner.entries.clear();
        inner.total_effective = 0.0;
        inner.dirty = true;
    }
}

/// Default storage path. XDG_DATA_HOME-aware; falls back to
/// ~/.local/share on Unix and the platform-specific data dir
/// elsewhere.
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
    data_dir.join("rusty-board").join("adaptive_dictionary.toml")
}

fn now_secs() -> i64 {
        core_kit::time::now_secs() as i64
    }

/// Effective count with exponential decay applied. Half-life
/// is `DECAY_HALF_LIFE_SECS`.
fn effective_count(entry: &Entry, now: i64) -> f64 {
    let age = (now - entry.last_seen).max(0) as f64;
    let half_lives = age / DECAY_HALF_LIFE_SECS;
    (entry.count as f64) * 0.5_f64.powf(half_lives)
}

/// Evict the entry with the lowest effective count. Mutates in
/// place. Called when we exceed `MAX_ENTRIES`.
fn evict_lowest(entries: &mut HashMap<String, Entry>, now: i64) {
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
    fn empty_dict_has_no_boost() {
        let d = AdaptiveDictionary::empty();
        assert_eq!(d.boost("anything"), 0.0);
        assert_eq!(d.len(), 0);
        assert!(d.is_empty());
    }

    #[test]
    fn record_commit_increases_count() {
        let d = AdaptiveDictionary::empty();
        d.record_commit("hello");
        assert_eq!(d.len(), 1);
        d.record_commit("hello");
        d.record_commit("hello");
        // Same word, still 1 entry.
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn case_insensitive() {
        let d = AdaptiveDictionary::empty();
        d.record_commit("Hello");
        d.record_commit("hello");
        d.record_commit("HELLO");
        // All three should share one entry.
        assert_eq!(d.len(), 1);
        // Boost is the same regardless of input casing.
        let b1 = d.boost("Hello");
        let b2 = d.boost("hello");
        let b3 = d.boost("HELLO");
        assert_eq!(b1, b2);
        assert_eq!(b2, b3);
    }

    #[test]
    fn whitespace_and_punctuation_not_recorded() {
        let d = AdaptiveDictionary::empty();
        d.record_commit("");
        d.record_commit("   ");
        d.record_commit("!!!");
        d.record_commit("123");
        assert_eq!(d.len(), 0);
        // But a word with mixed punct IS recorded.
        d.record_commit("it's");
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn boost_increases_with_frequency() {
        let d = AdaptiveDictionary::empty();
        // Establish a baseline of varied words.
        for w in ["one", "two", "three", "four", "five"] {
            d.record_commit(w);
        }
        let baseline_boost = d.boost("one");
        // Commit "hello" many times.
        for _ in 0..20 {
            d.record_commit("hello");
        }
        let hello_boost = d.boost("hello");
        assert!(
            hello_boost > baseline_boost,
            "frequently-typed word should get larger boost ({} vs {})",
            hello_boost,
            baseline_boost
        );
        assert!(hello_boost <= MAX_BOOST);
    }

    #[test]
    fn unknown_word_no_boost() {
        let d = AdaptiveDictionary::empty();
        d.record_commit("hello");
        assert_eq!(d.boost("goodbye"), 0.0);
    }

    #[test]
    fn forget_all_clears() {
        let d = AdaptiveDictionary::empty();
        d.record_commit("a");
        d.record_commit("b");
        d.record_commit("c");
        assert_eq!(d.len(), 3);
        d.forget_all();
        assert_eq!(d.len(), 0);
        assert_eq!(d.boost("a"), 0.0);
    }

    #[test]
    fn top_n_sorted_by_effective_count() {
        let d = AdaptiveDictionary::empty();
        d.record_commit("rare");
        for _ in 0..5 {
            d.record_commit("medium");
        }
        for _ in 0..20 {
            d.record_commit("common");
        }
        let top = d.top_n(3);
        assert_eq!(top.len(), 3);
        assert_eq!(top[0].0, "common");
        assert_eq!(top[1].0, "medium");
        assert_eq!(top[2].0, "rare");
        assert!(top[0].1 > top[1].1);
        assert!(top[1].1 > top[2].1);
    }

    #[test]
    fn eviction_at_cap_drops_lowest() {
        let d = AdaptiveDictionary::empty();
        // Heavy entry first so it has a clear effective-count lead.
        for _ in 0..100 {
            d.record_commit("kept");
        }
        // Fill up to cap with low-count entries.
        for i in 0..MAX_ENTRIES + 10 {
            d.record_commit(&format!("filler{}", i));
        }
        // Dictionary should be at or below cap.
        assert!(d.len() <= MAX_ENTRIES + 1); // +1 for "kept"
        // And "kept" should still be there (it has higher count
        // than the fillers).
        assert!(d.boost("kept") > 0.0);
    }

    #[test]
    fn boost_is_bounded_by_max() {
        let d = AdaptiveDictionary::empty();
        // Type one word a million times. Boost must still be ≤ MAX.
        for _ in 0..10000 {
            d.record_commit("dominant");
        }
        assert!(d.boost("dominant") <= MAX_BOOST + 1e-9);
    }

    #[test]
    fn decay_reduces_effective_count() {
        // Construct an Entry with a last_seen 60 days ago.
        let two_months_ago = now_secs() - (60 * 24 * 3600);
        let entry = Entry {
            count: 100,
            last_seen: two_months_ago,
        };
        let eff = effective_count(&entry, now_secs());
        // 60 days = 2 half-lives → 0.25× decay
        assert!(eff > 23.0 && eff < 27.0, "expected ~25, got {}", eff);
    }

    #[test]
    fn fresh_entries_dont_decay() {
        let entry = Entry {
            count: 50,
            last_seen: now_secs(),
        };
        let eff = effective_count(&entry, now_secs());
        assert!((eff - 50.0).abs() < 0.01);
    }

    /// "Sue" vs "use" disambiguation: if user typed "Sue" 10 times
    /// recently and "use" 5 times recently, "Sue" should get a
    /// larger boost. The recognizer's scoring would otherwise
    /// favor "use" (a slightly more common generic-corpus word).
    #[test]
    fn sue_vs_use_disambiguation_simulation() {
        let d = AdaptiveDictionary::empty();
        for _ in 0..10 {
            d.record_commit("sue");
        }
        for _ in 0..5 {
            d.record_commit("use");
        }
        assert!(d.boost("sue") > d.boost("use"));
    }
}
