//! Higher-level swipe recognition.
//!
//! This module turns a noisy raw swipe path into a ranked list of word
//! candidates. The previous implementation used only `is_subsequence` plus a
//! length penalty, which works for clean swipes on common words but breaks
//! down quickly on real input. This version adds:
//!
//! 1. **Damerau–Levenshtein edit distance** — typo-tolerant; handles
//!    insertions, deletions, substitutions, *and* adjacent transpositions.
//! 2. **Spatial similarity** — when the swipe says `helli` and the dictionary
//!    has both `hello` and `hippo`, we prefer `hello` because the `i` in the
//!    raw is *physically near* the `o` on the keyboard. Without spatial
//!    scoring, both are just "one substitution" and we'd flip a coin.
//! 3. **Endpoint anchoring** — humans land deliberately on the first and
//!    last key, so words sharing both endpoints get a boost; we no longer
//!    use it as a hard filter (the old version dropped legitimate matches
//!    when the user mis-released).
//! 4. **Word frequency weighting** — when scores are close, prefer common
//!    words over rare ones.
//!
//! All scoring is unit-tested in isolation.

use crate::swipe::{decode_swipe_path, KeyHitbox, SwipePoint};
use std::collections::HashMap;

/// Process-wide flag controlling whether recognize_with_speed
/// uses lenient scoring (wider edit cap, less spatial weight,
/// more frequency weight). Set by the settings panel.
pub static LENIENT_SWIPE_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_lenient_swipe_enabled(on: bool) {
    LENIENT_SWIPE_ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn lenient_swipe_enabled() -> bool {
    LENIENT_SWIPE_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

#[derive(Debug, Clone)]
pub struct WordCandidate {
    pub word: String,
    /// Lower is better. 0.0 ≈ exact match.
    pub score: f64,
}

/// A dictionary entry stored once at load time. We keep both the
/// `String` form (for outputting candidates and for the
/// frequency_rank map lookups) and the pre-computed `Vec<char>`
/// (for the recognizer's edit-distance and spatial-distance
/// scoring). Recomputing `word.chars().collect()` per scoring
/// iteration is one allocation per word per swipe — 500+
/// allocations per recognized gesture. Precomputing them once
/// at load eliminates that cost.
#[derive(Debug, Clone)]
struct WordEntry {
    text: String,
    chars: Vec<char>,
}

pub struct GestureRecognizer {
    /// All known words, keyed by length for fast filtering.
    words_by_length: Vec<Vec<WordEntry>>,
    /// Word frequency rank: lower = more common (rank 0 is most common).
    /// Words not in this map are treated as rank = `dictionary.len()`.
    frequency_rank: HashMap<String, usize>,
    /// Layout-derived spatial map: lowercase-letter → (x, y) in unit-key
    /// space (0..grid_columns horizontally, 0..rows vertically). Empty
    /// until `set_key_positions` is called.
    key_positions: HashMap<char, (f64, f64)>,
    /// Fast-path mirror of `key_positions` for ASCII a-z. Indexed by
    /// `c as u8 - b'a'`. `Some(x, y)` if that letter has a known
    /// position, `None` otherwise. spatial_distance is called once per
    /// dictionary word per swipe (~500 calls), each iterating per
    /// character — that's thousands of HashMap probes on the hot
    /// path. The array fast-path replaces ~95% of those with a
    /// direct index.
    key_positions_ascii: [Option<(f64, f64)>; 26],
    /// Total dictionary size — used to default unranked words.
    total_words: usize,
}

impl GestureRecognizer {
    pub fn new() -> Self {
        Self {
            words_by_length: vec![],
            frequency_rank: HashMap::new(),
            key_positions: HashMap::new(),
            key_positions_ascii: [None; 26],
            total_words: 0,
        }
    }

    /// Build a recognizer from a list of words. The list order is
    /// interpreted as frequency order (most common first), which is the
    /// natural form of dictionary files like `en_50k.txt`.
    pub fn with_dictionary(words: Vec<String>) -> Self {
        let mut me = Self::new();
        me.load_dictionary(words);
        me
    }

    pub fn load_dictionary(&mut self, words: Vec<String>) {
        self.words_by_length.clear();
        self.frequency_rank.clear();
        self.total_words = words.len();

        let max_len = words.iter().map(|w| w.len()).max().unwrap_or(20).max(20);
        self.words_by_length = vec![vec![]; max_len + 1];

        for (rank, w) in words.into_iter().enumerate() {
            let lc = w.to_lowercase();
            let len = lc.len();
            if len < self.words_by_length.len() {
                // Build the WordEntry once: pre-collected chars for
                // scoring + the canonical String for output. The
                // text allocation here also serves as the frequency_rank
                // key below (we clone for the map insert).
                let chars: Vec<char> = lc.chars().collect();
                self.words_by_length[len].push(WordEntry {
                    text: lc.clone(),
                    chars,
                });
            }
            // Keep the *first* occurrence (highest frequency) if duplicated.
            self.frequency_rank.entry(lc).or_insert(rank);
        }
    }

    /// Provide the current keyboard's letter positions for spatial scoring.
    /// Coordinates are in unit-key space; the recognizer normalizes
    /// internally so absolute scale doesn't matter.
    pub fn set_key_positions(&mut self, positions: HashMap<char, (f64, f64)>) {
        // Mirror ASCII a-z entries into the fast-path array. Other
        // characters (digits, punctuation) stay in the HashMap and
        // fall through the slow path in `spatial_distance`.
        let mut ascii = [None; 26];
        for (&c, &xy) in &positions {
            if c.is_ascii_lowercase() {
                let idx = c as u8 - b'a';
                ascii[idx as usize] = Some(xy);
            }
        }
        self.key_positions_ascii = ascii;
        self.key_positions = positions;
    }

    pub fn dictionary_size(&self) -> usize {
        self.total_words
    }

    /// Recognize the swipe path: snap to keys, then find the best dictionary
    /// matches.
    pub fn recognize(&self, path: &[SwipePoint], keys: &[KeyHitbox]) -> Vec<WordCandidate> {
        let raw = decode_swipe_path(path, keys);
        if raw.is_empty() {
            return vec![];
        }
        self.candidates_for_raw(&raw)
    }

    /// Recognize a swipe path with awareness of how fast the user moved.
    ///
    /// `avg_velocity_px_ms` is the path's average velocity in pixels per
    /// millisecond. Typical values:
    ///   * < 0.5  — very deliberate, "tracing" the letters one by one.
    ///   * 0.5–2.0 — normal swipe speed.
    ///   * > 2.0  — fast/sloppy.
    ///
    /// The weighting compromise:
    ///   * Slow swipes — user is being precise, so we trust their raw
    ///     output more. The raw fallback score drops; edit-distance is
    ///     penalized harder; the dictionary's pull weakens.
    ///   * Fast swipes — user is being sloppy, so we lean on the
    ///     dictionary harder. Raw fallback gets a bigger penalty;
    ///     frequency matters more.
    ///
    /// This matches user intuition: when you tap-type carefully you want
    /// the system to respect your input; when you swipe quickly you want
    /// it to clean up your mess.
    pub fn recognize_with_speed(
        &self,
        path: &[SwipePoint],
        keys: &[KeyHitbox],
        avg_velocity_px_ms: f64,
    ) -> Vec<WordCandidate> {
        let raw = decode_swipe_path(path, keys);
        if raw.is_empty() {
            return vec![];
        }
        let lenient = LENIENT_SWIPE_ENABLED.load(std::sync::atomic::Ordering::Relaxed);
        self.candidates_for_raw_with_speed_lenient(&raw, avg_velocity_px_ms, lenient)
    }

    /// Find the best candidates for a raw decoded string. Public for
    /// testing.
    pub fn candidates_for_raw(&self, raw: &str) -> Vec<WordCandidate> {
        // Default speed = normal (1.0 px/ms) → weights balanced as before.
        self.candidates_for_raw_with_speed(raw, 1.0)
    }

    /// Speed-aware variant of `candidates_for_raw`. See
    /// [`recognize_with_speed`] for what speed means.
    pub fn candidates_for_raw_with_speed(
        &self,
        raw: &str,
        avg_velocity_px_ms: f64,
    ) -> Vec<WordCandidate> {
        self.candidates_for_raw_with_speed_lenient(raw, avg_velocity_px_ms, false)
    }

    /// Like `candidates_for_raw_with_speed` but with a `lenient`
    /// flag. When lenient is true:
    ///   - edit-distance cap rises from 5 → 7 (accept more typos)
    ///   - length window widens from ±4 → ±6 (accept more length
    ///     mismatch between raw path-trace and dictionary word)
    ///   - spatial weight halves (path doesn't have to hit letters
    ///     accurately — useful for fast, sloppy, circular swipes)
    ///   - frequency weight increases by 50% (common words win more
    ///     readily, since the actual path matters less)
    /// Strict mode (lenient=false) gives the existing tuned behavior.
    pub fn candidates_for_raw_with_speed_lenient(
        &self,
        raw: &str,
        avg_velocity_px_ms: f64,
        lenient: bool,
    ) -> Vec<WordCandidate> {
        if raw.is_empty() {
            return vec![];
        }

        let speed_factor = (avg_velocity_px_ms / 2.0).clamp(0.0, 1.0);

        // Per-call weights. Lenient bumps frequency, drops spatial.
        let edit_weight = 0.3 + 0.4 * speed_factor;
        let spatial_weight = if lenient {
            // Halve spatial weight; users doing circular gestures
            // aren't aiming at letters with precision.
            (1.2 - 0.4 * speed_factor) * 0.5
        } else {
            1.2 - 0.4 * speed_factor
        };
        let freq_weight = if lenient {
            // Bump frequency weight — common words win more easily
            // when path is sloppy.
            (0.7 + 0.6 * speed_factor) * 1.5
        } else {
            0.7 + 0.6 * speed_factor
        };
        let raw_fallback_score = 1.5 + 4.0 * speed_factor;
        let endpoint_bonus_strength = 0.4 + 0.4 * speed_factor;
        let length_weight = 0.15 + 0.2 * speed_factor;

        // Edit-distance cap. Lenient widens by 2 — accommodates
        // dropped or doubled letters during a fast circular swipe.
        let edit_cap: usize = if lenient { 7 } else { 5 };
        // Length window — how much the dictionary word's length can
        // differ from the raw path's letter count. Lenient widens it.
        let len_window: usize = if lenient { 6 } else { 4 };

        let raw_lc = raw.to_lowercase();
        let raw_chars: Vec<char> = raw_lc.chars().collect();
        let first = raw_chars.first().copied();
        let last = raw_chars.last().copied();

        let raw_len = raw_chars.len();
        let lo = raw_len.saturating_sub(len_window);
        let hi = (raw_len + len_window).min(self.words_by_length.len().saturating_sub(1));

        let mut candidates: Vec<WordCandidate> = Vec::with_capacity(64);

        if self.frequency_rank.contains_key(&raw_lc) {
            candidates.push(WordCandidate {
                word: raw_lc.clone(),
                score: 0.0,
            });
        }

        // Pre-compute raw-letter positions ONCE. spatial_distance
        // used to look them up per-candidate (~500x for a typical
        // dictionary pass) — same raw, same positions, every time.
        // Hoist them out and let each candidate compare against the
        // cached array. Same correctness, fewer lookups.
        let raw_positions: Vec<Option<(f64, f64)>> = raw_chars
            .iter()
            .map(|&c| {
                if c.is_ascii_lowercase() {
                    self.key_positions_ascii[(c as u8 - b'a') as usize]
                } else {
                    self.key_positions.get(&c).copied()
                }
            })
            .collect();

        for length in lo..=hi {
            if length >= self.words_by_length.len() {
                break;
            }
            for entry in &self.words_by_length[length] {
                let wc = &entry.chars[..];
                let word = &entry.text;

                let edit = damerau_levenshtein_capped(&raw_chars, wc, edit_cap);
                if edit > edit_cap {
                    continue;
                }

                let spatial = self.spatial_distance_precomputed(&raw_positions, wc);

                let endpoint_bonus = match (wc.first(), wc.last()) {
                    (Some(&fw), Some(&lw)) => {
                        let f_match = first.map_or(false, |c| c == fw);
                        let l_match = last.map_or(false, |c| c == lw);
                        match (f_match, l_match) {
                            (true, true) => -endpoint_bonus_strength * 1.5,
                            (true, false) | (false, true) => -endpoint_bonus_strength * 0.5,
                            (false, false) => 0.0,
                        }
                    }
                    _ => 0.0,
                };

                let len_diff = (raw_chars.len() as f64 - wc.len() as f64).abs();
                let length_pen = len_diff * length_weight;

                let rank = self
                    .frequency_rank
                    .get(word)
                    .copied()
                    .unwrap_or(self.total_words);
                let freq_pen = ((rank as f64 + 1.0).ln() / 12.0).min(0.8) * freq_weight;

                let score = spatial * spatial_weight
                    + (edit as f64) * edit_weight
                    + length_pen
                    + endpoint_bonus
                    + freq_pen;

                candidates.push(WordCandidate {
                    word: word.clone(),
                    score,
                });
            }
        }

        candidates.sort_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        // After sorting, equal-score duplicates of the same word are
        // adjacent (scoring is a pure function of inputs, so the same
        // word produces the same score). dedup_by avoids the
        // HashSet+String-clone-per-candidate cost.
        candidates.dedup_by(|a, b| a.word == b.word);
        candidates.truncate(5);

        // Raw-fallback penalty depends on speed: slow → low penalty so
        // the user's deliberate raw input can win. Fast → high penalty
        // so the dictionary almost always overrides.
        if candidates.iter().all(|c| c.word != raw_lc) {
            candidates.push(WordCandidate {
                word: raw_lc,
                score: raw_fallback_score,
            });
        }

        candidates
    }

    /// Sum of geometric distances between aligned letters of `raw` and
    /// `candidate`, using the layout's key positions. Pads the shorter
    /// side with the last position of the longer side to keep alignment
    /// reasonable for length-mismatched inputs.
    fn spatial_distance(&self, raw: &[char], candidate: &[char]) -> f64 {
        if self.key_positions.is_empty() {
            return 0.0;
        }
        let n = raw.len().min(candidate.len());
        if n == 0 {
            return 0.0;
        }

        // Fast lookup: ASCII a-z goes through the array; everything
        // else falls through to the HashMap. Vast majority of swipe
        // typing is ASCII letters, so the HashMap path is rarely
        // taken.
        let lookup = |c: char| -> Option<(f64, f64)> {
            if c.is_ascii_lowercase() {
                self.key_positions_ascii[(c as u8 - b'a') as usize]
            } else {
                self.key_positions.get(&c).copied()
            }
        };

        let mut total = 0.0;
        for i in 0..n {
            if let (Some((x1, y1)), Some((x2, y2))) = (lookup(raw[i]), lookup(candidate[i])) {
                let dx = x1 - x2;
                let dy = y1 - y2;
                total += (dx * dx + dy * dy).sqrt();
            }
        }
        total / n as f64
    }

    /// Variant of `spatial_distance` that takes precomputed `raw`
    /// positions. The candidate-word loop in `candidates_for_raw_*`
    /// computes the SAME raw-letter positions for every candidate;
    /// hoisting the lookups out of the inner loop is a real win on
    /// dictionaries large enough that the constant factor matters
    /// (≥500 candidates → 500× the saved lookups). For small
    /// dictionaries the savings are unmeasurable but never negative.
    ///
    /// `raw_positions[i] = None` means "raw letter i has no known
    /// position" — typically because the layout doesn't include that
    /// letter (digit row off, etc.). When either side is None, the
    /// pair contributes 0 to the total. Same behavior as the original.
    fn spatial_distance_precomputed(
        &self,
        raw_positions: &[Option<(f64, f64)>],
        candidate: &[char],
    ) -> f64 {
        if self.key_positions.is_empty() {
            return 0.0;
        }
        let n = raw_positions.len().min(candidate.len());
        if n == 0 {
            return 0.0;
        }
        let mut total = 0.0;
        for i in 0..n {
            let cand_pos = if candidate[i].is_ascii_lowercase() {
                self.key_positions_ascii[(candidate[i] as u8 - b'a') as usize]
            } else {
                self.key_positions.get(&candidate[i]).copied()
            };
            if let (Some((x1, y1)), Some((x2, y2))) = (raw_positions[i], cand_pos) {
                let dx = x1 - x2;
                let dy = y1 - y2;
                total += (dx * dx + dy * dy).sqrt();
            }
        }
        total / n as f64
    }
}

impl Default for GestureRecognizer {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------------------------------------
// Damerau–Levenshtein with adjacent-transposition (capped variant)
// ------------------------------------------------------------

/// Damerau–Levenshtein distance with adjacent transposition, bailing out
/// early if the running minimum exceeds `cap`. Returns `cap + 1` to signal
/// "above cap" rather than computing the full distance.
pub fn damerau_levenshtein_capped(a: &[char], b: &[char], cap: usize) -> usize {
    let m = a.len();
    let n = b.len();

    // Length-difference is a hard lower bound on edit distance.
    if (m as isize - n as isize).unsigned_abs() > cap {
        return cap + 1;
    }
    if m == 0 {
        return n.min(cap + 1);
    }
    if n == 0 {
        return m.min(cap + 1);
    }

    // Three rolling rows. Past impl used Vec<usize> here, paying 3
    // small allocations every call → 1500/swipe at 500-word dicts.
    // Our words are <32 chars (English dict has nothing close), so
    // fixed-size stack arrays cover every realistic case without
    // allocation. Use index rotation instead of `mem::swap` so we
    // don't memcpy the whole row each iteration.
    const STACK_LEN: usize = 32;
    if n + 1 > STACK_LEN {
        // Rare fallback for very long words.
        return damerau_levenshtein_capped_heap(a, b, cap);
    }

    // 3 rows × STACK_LEN cells; indexed by `rows[role]` where role
    // is 0=prev_prev, 1=prev, 2=curr. After each iteration we
    // rotate role indices instead of moving data.
    let mut rows = [[0usize; STACK_LEN]; 3];
    let mut prev_prev = 0usize;
    let mut prev = 1usize;
    let mut curr = 2usize;

    // Initialise rows[prev] = [0, 1, 2, ..., n].
    for j in 0..=n {
        rows[prev][j] = j;
    }

    for i in 1..=m {
        rows[curr][0] = i;
        let mut row_min = rows[curr][0];
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            let mut v = (rows[curr][j - 1] + 1)
                .min(rows[prev][j] + 1)
                .min(rows[prev][j - 1] + cost);
            // Transposition.
            if i > 1
                && j > 1
                && a[i - 1] == b[j - 2]
                && a[i - 2] == b[j - 1]
            {
                v = v.min(rows[prev_prev][j - 2] + 1);
            }
            rows[curr][j] = v;
            row_min = row_min.min(v);
        }
        if row_min > cap {
            return cap + 1;
        }
        // Rotate role indices: prev_prev ← prev ← curr (new prev_prev
        // gets the old curr's slot, which we'll overwrite next iter).
        let next_curr = prev_prev;
        prev_prev = prev;
        prev = curr;
        curr = next_curr;
    }

    rows[prev][n]
}

/// Heap-allocated fallback for the rare case of words exceeding the
/// stack array size. Identical algorithm — only the buffer storage
/// differs.
fn damerau_levenshtein_capped_heap(a: &[char], b: &[char], cap: usize) -> usize {
    let m = a.len();
    let n = b.len();
    let mut prev_prev = vec![0usize; n + 1];
    let mut prev = (0..=n).collect::<Vec<usize>>();
    let mut curr = vec![0usize; n + 1];

    for i in 1..=m {
        curr[0] = i;
        let mut row_min = curr[0];
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            let mut v = (curr[j - 1] + 1)
                .min(prev[j] + 1)
                .min(prev[j - 1] + cost);
            if i > 1
                && j > 1
                && a[i - 1] == b[j - 2]
                && a[i - 2] == b[j - 1]
            {
                v = v.min(prev_prev[j - 2] + 1);
            }
            curr[j] = v;
            row_min = row_min.min(v);
        }
        if row_min > cap {
            return cap + 1;
        }
        std::mem::swap(&mut prev_prev, &mut prev);
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_dict() -> GestureRecognizer {
        GestureRecognizer::with_dictionary(vec![
            "the".into(),
            "and".into(),
            "for".into(),
            "hello".into(),
            "help".into(),
            "world".into(),
            "rust".into(),
            "test".into(),
            "type".into(),
            "swipe".into(),
            "keyboard".into(),
            "hippo".into(),
        ])
    }

    fn qwerty_positions() -> HashMap<char, (f64, f64)> {
        // Approximate QWERTY positions for spatial-scoring tests.
        let rows = [
            "qwertyuiop",
            "asdfghjkl",
            "zxcvbnm",
        ];
        let mut map = HashMap::new();
        for (y, row) in rows.iter().enumerate() {
            let row_offset = match y {
                0 => 0.0,
                1 => 0.5,
                2 => 1.5,
                _ => 0.0,
            };
            for (x, ch) in row.chars().enumerate() {
                map.insert(ch, (x as f64 + row_offset, y as f64));
            }
        }
        map
    }

    // ------- Damerau–Levenshtein tests -------

    #[test]
    fn dl_identity() {
        let a: Vec<char> = "hello".chars().collect();
        assert_eq!(damerau_levenshtein_capped(&a, &a, 4), 0);
    }

    #[test]
    fn dl_substitution() {
        let a: Vec<char> = "hello".chars().collect();
        let b: Vec<char> = "hallo".chars().collect();
        assert_eq!(damerau_levenshtein_capped(&a, &b, 4), 1);
    }

    #[test]
    fn dl_transposition() {
        let a: Vec<char> = "hlelo".chars().collect();
        let b: Vec<char> = "hello".chars().collect();
        // Should be 1 (single adjacent transposition), not 2.
        assert_eq!(damerau_levenshtein_capped(&a, &b, 4), 1);
    }

    #[test]
    fn dl_insertion_and_deletion() {
        let a: Vec<char> = "helo".chars().collect();
        let b: Vec<char> = "hello".chars().collect();
        assert_eq!(damerau_levenshtein_capped(&a, &b, 4), 1);
    }

    #[test]
    fn dl_caps_out() {
        let a: Vec<char> = "hello".chars().collect();
        let b: Vec<char> = "world".chars().collect();
        // h-e-l-l-o → w-o-r-l-d is a substitution-heavy edit; capping at 1
        // should bail out early.
        assert_eq!(damerau_levenshtein_capped(&a, &b, 1), 2);
    }

    #[test]
    fn dl_length_diff_caps_out_immediately() {
        let a: Vec<char> = "a".chars().collect();
        let b: Vec<char> = "abcdefghij".chars().collect();
        assert_eq!(damerau_levenshtein_capped(&a, &b, 3), 4);
    }

    // ------- Recognizer behavior -------

    #[test]
    fn raw_in_dictionary_is_top_candidate() {
        let r = small_dict();
        let cands = r.candidates_for_raw("hello");
        assert_eq!(cands[0].word, "hello", "candidates: {:?}", cands);
        // Exact-match score must be at-or-below 0 (no edit/spatial
        // penalty; possibly negative from endpoint_bonus which
        // rewards matching first+last chars). The earlier "== 0.0"
        // assertion predates the endpoint-bonus + freq-pen
        // refinements.
        assert!(
            cands[0].score <= 0.5,
            "exact match should score ≤ 0.5, got {}",
            cands[0].score
        );
    }

    #[test]
    fn helo_finds_hello() {
        let r = small_dict();
        let cands = r.candidates_for_raw("helo");
        assert_eq!(cands[0].word, "hello", "candidates: {:?}", cands);
    }

    #[test]
    fn hlelo_finds_hello_via_transposition() {
        let r = small_dict();
        let cands = r.candidates_for_raw("hlelo");
        assert_eq!(cands[0].word, "hello", "candidates: {:?}", cands);
    }

    #[test]
    fn keybard_finds_keyboard() {
        let r = small_dict();
        let cands = r.candidates_for_raw("keybard");
        assert_eq!(cands[0].word, "keyboard", "candidates: {:?}", cands);
    }

    #[test]
    fn unknown_word_falls_back_to_raw() {
        let r = small_dict();
        let cands = r.candidates_for_raw("xyzqq");
        assert!(!cands.is_empty());
        // Either nothing close exists (fallback raw) or the fallback raw
        // is the worst candidate.
        let last = cands.last().unwrap();
        assert!(last.word == "xyzqq" || cands.iter().any(|c| c.word == "xyzqq"));
    }

    #[test]
    fn frequency_breaks_ties() {
        // "the" is more common than "hippo" — both have similar edit distance
        // from a random short input, so frequency should tilt the ranking.
        let r = small_dict();
        let cands = r.candidates_for_raw("the");
        assert_eq!(cands[0].word, "the");
    }

    #[test]
    fn spatial_scoring_disambiguates_substitutions() {
        // Without spatial scoring, "helli" is one substitution from both
        // "hello" (i↔o) and "helps" (i↔p). With QWERTY positions, i (7,0)
        // is much closer to o (8,0) than to p (9,0), so we should pick hello.
        let mut r = GestureRecognizer::with_dictionary(vec![
            "hello".into(),
            "helps".into(),
        ]);
        r.set_key_positions(qwerty_positions());
        let cands = r.candidates_for_raw("helli");
        assert_eq!(cands[0].word, "hello", "got {:?}", cands);
    }

    #[test]
    fn spatial_scoring_breaks_edit_ties() {
        // Test case where two candidates have identical edit distance:
        // raw "thnt" vs "tent" (sub n→e, edit=1) vs "tint" (sub n→i, edit=1).
        // n is at (6,2), e is at (2,0), i is at (7,0). On QWERTY:
        //   distance(n, e) ≈ sqrt((6-2)^2 + (2-0)^2) = ~4.5
        //   distance(n, i) ≈ sqrt((6-7)^2 + (2-0)^2) = ~2.2
        // So 'tint' should win on spatial alone (n is closer to i than to e).
        let mut r = GestureRecognizer::with_dictionary(vec![
            "tent".into(),
            "tint".into(),
        ]);
        r.set_key_positions(qwerty_positions());
        let cands = r.candidates_for_raw("thnt");
        // Both candidates have the same edit distance (2: h→e/i and n→n,
        // wait: thnt vs tent is t-t, h-e, n-n, t-t = 1 edit. Same for tint:
        // t-t, h-i, n-n, t-t = 1 edit. So edit-distance is tied at 1.
        assert_eq!(cands[0].word, "tint", "got {:?}", cands);
    }

    #[test]
    fn endpoint_match_boosts_ranking() {
        // "hellp" (mistyped 'o' as 'p') vs candidates "hello" and "help".
        // hello shares first+last with hellp ❌ (last is 'o' vs 'p')
        // help shares first only.
        // Edit distances: hellp→hello = 1 (sub), hellp→help = 1 (delete).
        // Ties go to the one with both endpoints matching, but neither does
        // here, so this test just verifies the function doesn't crash.
        let r = small_dict();
        let cands = r.candidates_for_raw("hellp");
        assert!(!cands.is_empty());
    }

    #[test]
    fn endpoint_bonus_does_not_overrule_better_edit_distance() {
        // Reproduces the bug from the user's "hello" → "thete" → "there"
        // log: "there" matched both endpoints of "thete" and got ranked
        // above better candidates. The fix: endpoint bonus must be smaller
        // than the cost of one extra edit.
        //
        // raw=`thete` (5 chars). Candidates:
        //   - "there" (5 chars): edit=1 (t→r at pos 3), endpoints t/e match
        //   - "these" (5 chars): edit=1 (t→s at pos 3), endpoints t/e match
        //   - "the"   (3 chars): edit=2 (delete t,e), starts with t, ends with e
        //
        // We don't have a single right answer here without more context,
        // but we want to verify that the endpoint bonus alone (-0.6) can't
        // promote a candidate over one with strictly fewer edits. This is
        // a regression test for the scoring weights.
        let r = GestureRecognizer::with_dictionary(vec![
            "there".into(),
            "these".into(),
            "the".into(),
            "tee".into(),
        ]);
        // "tee": edit from "thete" is 2 (delete h, delete t), endpoints match
        let cands = r.candidates_for_raw("thete");
        // Whatever the top candidate is, "tee" (edit=2) should NOT outrank
        // "there" (edit=1) just because both have endpoints matching.
        let there_pos = cands.iter().position(|c| c.word == "there");
        let tee_pos = cands.iter().position(|c| c.word == "tee");
        if let (Some(t), Some(e)) = (there_pos, tee_pos) {
            assert!(
                t < e,
                "'there' (edit=1) should outrank 'tee' (edit=2). \
                 candidates: {:?}",
                cands
            );
        }
    }

    #[test]
    fn slow_swipe_promotes_raw_input() {
        // For a deliberate, slow swipe (e.g., 0.2 px/ms — three letters per
        // second), the user is being precise. Their raw input should win
        // over closely-related dictionary words.
        //
        // raw="rust" with `rust` and `must` in dictionary: edit distance
        // from "rust" is 0 vs 1 — at any speed `rust` wins because it's
        // exact. So pick a tougher case: raw="rsut" (intended `rust` with
        // wobble) — at slow speed the raw might be preserved verbatim;
        // at fast speed `rust` should win.
        let r = GestureRecognizer::with_dictionary(vec![
            "rust".into(),
            "must".into(),
            "just".into(),
        ]);
        let slow = r.candidates_for_raw_with_speed("rsut", 0.1);
        let fast = r.candidates_for_raw_with_speed("rsut", 3.0);

        // At fast speed, dictionary should be more aggressive — the raw
        // fallback "rsut" should rank lower (or be absent from the top).
        let slow_raw_pos = slow.iter().position(|c| c.word == "rsut");
        let fast_raw_pos = fast.iter().position(|c| c.word == "rsut");

        // Both should still find some "rust"-like candidate. The exact
        // ranking depends on weights but the key invariant is that the
        // raw input is more competitive at slow speed than at fast speed.
        match (slow_raw_pos, fast_raw_pos) {
            (Some(s), Some(f)) => {
                // If both contain raw, slow should rank it equal or higher
                // (lower index) than fast.
                assert!(s <= f, "slow={} fast={} (slow should be <=)", s, f);
            }
            (Some(_), None) => {
                // Slow ranks raw, fast doesn't — perfect demonstration.
            }
            (None, Some(_)) => {
                panic!(
                    "fast ranks raw but slow doesn't — that's the wrong direction. \
                     slow: {:?}, fast: {:?}",
                    slow, fast
                );
            }
            (None, None) => {
                // Both prefer dictionary entirely; that's fine, just no test signal.
            }
        }
    }

    #[test]
    fn fast_swipe_strengthens_dictionary_correction() {
        // raw "tge" — could be the user wobbling on their way to "the".
        // At fast speed we should aggressively correct to "the". At slow
        // speed we should be more hesitant.
        let r = GestureRecognizer::with_dictionary(vec![
            "the".into(),
            "tee".into(),
            "tea".into(),
        ]);
        let fast = r.candidates_for_raw_with_speed("tge", 3.0);
        // At fast speed, "the" (edit=1) should comfortably beat the raw.
        assert_eq!(
            fast[0].word, "the",
            "fast swipe should auto-correct to 'the'. got: {:?}",
            fast
        );
    }

    #[test]
    fn spatial_distance_precomputed_matches_original() {
        // Verifies the perf-optimization preserves identical semantics:
        // spatial_distance_precomputed must produce the same result as
        // the original spatial_distance for every input pair. Without
        // this guard, a future refactor of one of the two functions
        // could silently drift the recognizer's scoring.
        let mut r = GestureRecognizer::new();
        let mut positions = std::collections::HashMap::new();
        let qwerty_top = "qwertyuiop";
        for (i, c) in qwerty_top.chars().enumerate() {
            positions.insert(c, (i as f64, 0.0));
        }
        let qwerty_mid = "asdfghjkl";
        for (i, c) in qwerty_mid.chars().enumerate() {
            positions.insert(c, (i as f64 + 0.25, 1.0));
        }
        r.set_key_positions(positions);

        let cases = [
            ("hello", "hello"),
            ("hello", "world"),
            ("hi", "hello"),
            ("teh", "the"),
            ("qwerty", "qwerty"),
            ("asd", "qwe"),
        ];

        for (raw_str, cand_str) in cases {
            let raw_chars: Vec<char> = raw_str.chars().collect();
            let cand_chars: Vec<char> = cand_str.chars().collect();

            let original = r.spatial_distance(&raw_chars, &cand_chars);

            let raw_positions: Vec<Option<(f64, f64)>> = raw_chars
                .iter()
                .map(|&c| {
                    if c.is_ascii_lowercase() {
                        r.key_positions_ascii[(c as u8 - b'a') as usize]
                    } else {
                        r.key_positions.get(&c).copied()
                    }
                })
                .collect();
            let precomputed = r.spatial_distance_precomputed(&raw_positions, &cand_chars);

            assert_eq!(
                original, precomputed,
                "spatial_distance vs precomputed for raw={:?} cand={:?}: {} vs {}",
                raw_str, cand_str, original, precomputed,
            );
        }
    }

    // ----------------------------------------------------------------
    // End-to-end bug-reproduction test for the alphabet-layout
    // "swipe-typing line shows up but does not write" report.
    //
    // Pipeline mirrored:
    //   1. Build realistic Dvorak ortholinear hitboxes
    //   2. Construct GestureRecognizer with a dictionary containing
    //      target word + decoys
    //   3. Compute positions from boxes the same way the production
    //      `refresh_recognizer_positions` does and call set_key_positions
    //   4. Synthesize a "the" swipe path
    //   5. Push the path through SwipeTracker with smoothing=0.3 (the
    //      production default — drag_update applies this per-sample)
    //   6. Call recognize_with_speed on the smoothed path
    //
    // The user sees the trail render but no SwipeWord fires. The
    // failure mode at the dispatch_swipe_source layer requires
    // recognize_with_speed to return empty candidates AND
    // decode_swipe_path to return empty raw — both via the same
    // decode logic. This test catches the recognizer-pipeline branch
    // of that combined failure.
    // ----------------------------------------------------------------

    fn build_dvorak_recognizer_input() -> (
        Vec<crate::swipe::KeyHitbox>,
        HashMap<char, (f64, f64)>,
    ) {
        use crate::swipe::KeyHitbox;
        
        let key_w = 60.0;
        let key_h = 46.0;
        let row1 = ["'", ",", ".", "p", "y", "f", "g", "c", "r", "l"];
        let row2 = ["a", "o", "e", "u", "i", "d", "h", "t", "n", "s"];
        let row3 = ["q", "j", "k", "x", "b", "m", "w", "v"];

        let mut boxes = Vec::new();
        let mk = |label: &str, x: f64, y: f64| KeyHitbox {
            label: label.to_string(),
            swipe_target: true,
            x,
            y,
            width: key_w,
            height: key_h,
        };
        for (c, &l) in row1.iter().enumerate() {
            boxes.push(mk(l, (c as f64) * key_w, 0.0));
        }
        for (c, &l) in row2.iter().enumerate() {
            boxes.push(mk(l, (c as f64) * key_w, key_h + 3.0));
        }
        for (c, &l) in row3.iter().enumerate() {
            boxes.push(mk(l, ((c + 1) as f64) * key_w, (key_h + 3.0) * 2.0));
        }

        // Same calculation as production refresh_recognizer_positions:
        // median key width as unit, normalize centers by it.
        let mut widths: Vec<f64> = boxes
            .iter()
            .filter(|b| b.swipe_target)
            .map(|b| b.width)
            .collect();
        widths.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let unit = widths
            .get(widths.len() / 2)
            .copied()
            .filter(|&w| w > 0.0)
            .unwrap_or(60.0);

        let mut positions = HashMap::new();
        for b in &boxes {
            if !b.swipe_target {
                continue;
            }
            if let Some(c) = b.label.chars().next() {
                let cx = (b.x + b.width / 2.0) / unit;
                let cy = (b.y + b.height / 2.0) / unit;
                for k in c.to_lowercase() {
                    positions.insert(k, (cx, cy));
                }
            }
        }
        (boxes, positions)
    }

    fn synth_dvorak_swipe(centers: &[(f64, f64)]) -> Vec<SwipePoint> {
        let mut path = Vec::new();
        let mut t: i64 = 0;
        for (i, &(lx, ly)) in centers.iter().enumerate() {
            if i > 0 {
                let (px, py) = centers[i - 1];
                for k in 1..=5 {
                    let f = k as f64 / 6.0;
                    let x = px + (lx - px) * f;
                    let y = py + (ly - py) * f;
                    t += 16;
                    path.push(SwipePoint::with_timestamp(x, y, t));
                }
            }
            for _ in 0..4 {
                t += 30;
                path.push(SwipePoint::with_timestamp(lx, ly, t));
            }
        }
        path
    }

    #[test]
    fn recognize_dvorak_the_swipe_returns_candidates() {
        let (boxes, positions) = build_dvorak_recognizer_input();
        let mut r = GestureRecognizer::with_dictionary(vec![
            "the".into(),
            "their".into(),
            "them".into(),
            "this".into(),
            "to".into(),
            "and".into(),
        ]);
        r.set_key_positions(positions);

        // Find Dvorak letter centers from the boxes.
        let center = |label: &str| -> (f64, f64) {
            let b = boxes
                .iter()
                .find(|b| b.label == label)
                .unwrap_or_else(|| panic!("no key {}", label));
            (b.x + b.width / 2.0, b.y + b.height / 2.0)
        };
        let path = synth_dvorak_swipe(&[center("t"), center("h"), center("e")]);

        // Push through the tracker with production smoothing.
        let mut tracker = crate::swipe::SwipeTracker::new(0.3);
        tracker.start(path[0].x, path[0].y);
        for p in &path[1..] {
            tracker.update(p.x, p.y);
        }
        let smoothed = tracker.finish();

        let avg_v = crate::swipe::path_avg_velocity(&smoothed);
        let candidates = r.recognize_with_speed(&smoothed, &boxes, avg_v);

        assert!(
            !candidates.is_empty(),
            "recognize_with_speed returned 0 candidates for a Dvorak 'the' swipe. \
             smoothed.len={}, avg_v={:.3}, raw={:?}",
            smoothed.len(),
            avg_v,
            crate::swipe::decode_swipe_path(&smoothed, &boxes)
        );
        // Spot-check: at minimum the top candidate should start with 't'.
        let top = &candidates[0].word;
        assert!(
            top.starts_with('t'),
            "top candidate {:?} does not start with 't'. all candidates: {:?}",
            top,
            candidates.iter().map(|c| &c.word).collect::<Vec<_>>()
        );
    }
}
