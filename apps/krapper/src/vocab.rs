//! Vocabulary handling and the rap-suggestion engine.
//!
//! The new shape: phoneme-first via [`Dict`](crate::dict::Dict), with the old
//! character-based heuristics surviving only as fallbacks for words the dict
//! doesn't know about. Every public op records which path it took so the debug
//! panel can surface it.
//!
//! The original kMelodizer / kLyricFormatter approach lives unchanged in
//! `examples/learning_string_matching.rs` for posterity and reference.

use rand::Rng;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::dict::Dict;

// --- vocab loading ----------------------------------------------------------

/// One loaded artist vocabulary.
#[derive(Debug, Clone)]
pub struct Vocab {
    pub name: String,
    /// Lower-cased, punctuation-stripped words.
    pub words: HashSet<String>,
    /// Words grouped by syllable count. Counts use the dict where known,
    /// fall back to the vowel-cluster heuristic otherwise.
    pub by_syllables: BTreeMap<u8, Vec<String>>,
    /// Words indexed by their phoneme stress pattern. Only populated for words
    /// the dict knows — these are the words usable for stress-matched blurbs.
    pub by_stress: HashMap<Vec<u8>, Vec<String>>,
    /// How many words in `words` had phoneme entries available at load time.
    /// Surfaced in the debug panel so the user can see how much of the vocab
    /// is operating on phonemes vs heuristics.
    pub dict_coverage: usize,
    /// File path this vocab lives at on disk. `None` for in-memory-only vocabs
    /// (used during initial embedded loading before the file is written).
    pub path: Option<std::path::PathBuf>,
}

impl Vocab {
    pub fn load_from_str(name: &str, raw: &str, dict: &Dict) -> Self {
        let mut v = Self {
            name: name.to_string(),
            words: HashSet::new(),
            by_syllables: BTreeMap::new(),
            by_stress: HashMap::new(),
            dict_coverage: 0,
            path: None,
        };
        v.absorb(raw, dict);
        v
    }

    /// Build a vocab whose canonical home is `path`. Loads the file's contents
    /// if present; otherwise creates an empty vocab pointing at that path so
    /// the first save lands in the right place.
    pub fn load_from_file<P: AsRef<std::path::Path>>(
        name: &str,
        path: P,
        dict: &Dict,
    ) -> std::io::Result<Self> {
        let path_buf = path.as_ref().to_path_buf();
        let raw = std::fs::read_to_string(&path_buf).unwrap_or_default();
        let mut v = Self::load_from_str(name, &raw, dict);
        v.path = Some(path_buf);
        Ok(v)
    }

    /// Merge another batch of raw lyric text into this vocab and rebuild
    /// indexes. Existing words are kept; new sanitized words are added.
    pub fn extend_with_text(&mut self, raw: &str, dict: &Dict) {
        self.absorb(raw, dict);
    }

    /// Internal: parse `raw` into words, merge into `self`, rebuild indexes.
    fn absorb(&mut self, raw: &str, dict: &Dict) {
        for tok in raw.split(|c: char| c.is_whitespace() || c == ',') {
            let clean = sanitize(tok);
            if !clean.is_empty() {
                self.words.insert(clean);
            }
        }
        // Rebuild indexes from scratch — simpler and correct than incremental.
        self.by_syllables.clear();
        self.by_stress.clear();
        self.dict_coverage = 0;
        for w in &self.words {
            let count = match dict.syllables(w) {
                Some(n) => {
                    self.dict_coverage += 1;
                    n
                }
                None => syllable_count_heuristic(w),
            };
            self.by_syllables.entry(count).or_default().push(w.clone());
            if let Some(stress) = dict.stress_pattern(w) {
                self.by_stress.entry(stress).or_default().push(w.clone());
            }
        }
        for v in self.by_syllables.values_mut() {
            v.sort();
        }
        for v in self.by_stress.values_mut() {
            v.sort();
        }
    }

    /// Serialise the vocab's word list to its `path` (atomic write via tmp +
    /// rename). Returns an error if `path` is unset or the write fails.
    pub fn save_to_disk(&self) -> std::io::Result<()> {
        let path = self
            .path
            .as_ref()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "no path set"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut words: Vec<&String> = self.words.iter().collect();
        words.sort();
        let body = words
            .iter()
            .map(|w| w.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let tmp = path.with_extension("txt.tmp");
        std::fs::write(&tmp, body.as_bytes())?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Strip the kinds of junk `kLyricFormatter` cleaned out (parens, commas,
/// section labels) and lower-case.
fn sanitize(raw: &str) -> String {
    let s: String = raw
        .chars()
        .filter(|c| c.is_alphabetic() || *c == '\'')
        .collect::<String>()
        .to_lowercase();
    const SKIP: &[&str] = &["chorus", "verse", "intro", "outro", "bridge"];
    if SKIP.iter().any(|k| s.contains(k)) {
        return String::new();
    }
    s
}

// --- syllable counting fallback --------------------------------------------

/// Vowel-cluster heuristic, used **only** when the dict doesn't know the word.
/// Misses *fire* (1 vs 2), among others. The dict version is authoritative.
pub fn syllable_count_heuristic(word: &str) -> u8 {
    let word = word
        .trim_matches(|c: char| !c.is_alphabetic())
        .to_lowercase();
    if word.is_empty() {
        return 0;
    }
    let bytes = word.as_bytes();
    let is_vowel = |i: usize, c: u8| -> bool {
        matches!(c, b'a' | b'e' | b'i' | b'o' | b'u') || (c == b'y' && i > 0)
    };

    let mut count: u8 = 0;
    let mut prev_vowel = false;
    for (i, &c) in bytes.iter().enumerate() {
        let v = is_vowel(i, c);
        if v && !prev_vowel {
            count = count.saturating_add(1);
        }
        prev_vowel = v;
    }
    if bytes.last() == Some(&b'e') && count > 1 {
        count -= 1;
    }
    count.max(1)
}

/// Authoritative when possible: dict first, heuristic if not.
pub fn syllable_count(dict: &Dict, word: &str) -> u8 {
    dict.syllables(word).unwrap_or_else(|| syllable_count_heuristic(word))
}


// --- blurbs: stress-matched phrase fragments --------------------------------
//
// This is the replacement for the old "Suggest lines" word-soup generator. The
// difference matters:
//
//   * `suggest_lines` returned random word combinations summing to a syllable
//     count. The output looked like rap but had no rhythmic structure. It was
//     a Mad Libs prompt.
//
//   * `suggest_blurbs` returns short phrase fragments (2-4 words, ~2-6
//     syllables) whose concatenated *stress patterns* match a target rhythm.
//     The output is rhythmically usable: the writer can pick a blurb and know
//     it'll scan against the bar.
//
// What it doesn't try to do: produce grammatical full lines. That's a
// language-model problem, not a rule-engine one. Blurbs are raw material for
// a writer, not finished output.

/// A short stress-matched phrase fragment.
#[derive(Debug, Clone)]
pub struct Blurb {
    /// `(word, syllable_count)` in order.
    pub words: Vec<(String, u8)>,
    /// Concatenated phoneme stress pattern across all the words.
    pub stress: Vec<u8>,
    /// True if a rhyme seed was supplied AND the closing word rhymed phonetically.
    pub rhymes_seed: bool,
}

impl Blurb {
    pub fn as_text(&self) -> String {
        self.words
            .iter()
            .map(|(w, _)| w.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn syllables(&self) -> usize {
        self.stress.len()
    }
}

/// Produce up to `n` distinct blurbs whose concatenated stress patterns match
/// `target_stress`. If `rhyme_seed` is non-empty, the closing word of each
/// blurb must rhyme with it phonetically (via the dict).
///
/// Strategy: random sampling with a per-attempt budget. We pick a closing word
/// (rhyme-constrained if asked), check it matches the tail of `target_stress`,
/// then greedily fill the prefix with vocab words whose stress patterns slot
/// in. The closing-word-first pass is important — rhymes are scarce; if we
/// can't satisfy the rhyme constraint there's no point building a body.
pub fn suggest_blurbs(
    vocab: &Vocab,
    dict: &Dict,
    target_stress: &[u8],
    rhyme_seed: &str,
    n: usize,
) -> Vec<Blurb> {
    if target_stress.is_empty() || vocab.by_stress.is_empty() {
        return vec![];
    }
    let seed = rhyme_seed.trim();
    let mut rng = rand::thread_rng();
    let mut out: Vec<Blurb> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // Pre-compute candidate closing words. If a seed is given, this is the
    // dict-rhyme pool intersected with the vocab. If not, it's the whole
    // by_stress pool — any word is a valid closer.
    let closer_pool: Vec<&String> = if seed.is_empty() {
        vocab.by_stress.values().flatten().collect()
    } else {
        vocab
            .words
            .iter()
            .filter(|w| dict.rhymes(w, seed))
            .collect()
    };
    if closer_pool.is_empty() {
        return vec![];
    }

    let max_attempts = n * 50;
    for _ in 0..max_attempts {
        if out.len() >= n {
            break;
        }

        // Pick a closing word at random.
        let closer = closer_pool[rng.gen_range(0..closer_pool.len())].clone();
        let Some(closer_stress) = dict.stress_pattern(&closer) else {
            continue;
        };
        if closer_stress.is_empty() || closer_stress.len() > target_stress.len() {
            continue;
        }
        // It must match the *tail* of the target stress pattern.
        let tail_start = target_stress.len() - closer_stress.len();
        if target_stress[tail_start..] != closer_stress[..] {
            continue;
        }

        // Fill the prefix [0..tail_start] with vocab words whose stress
        // patterns slot in.
        let prefix = &target_stress[..tail_start];
        let Some(filler_words) = fill_prefix(vocab, prefix, &mut rng) else {
            continue;
        };

        let mut words: Vec<(String, u8)> = filler_words
            .into_iter()
            .map(|(w, s)| (w, s.len() as u8))
            .collect();
        words.push((closer.clone(), closer_stress.len() as u8));

        let text = words
            .iter()
            .map(|(w, _)| w.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        if seen.insert(text) {
            out.push(Blurb {
                words,
                stress: target_stress.to_vec(),
                rhymes_seed: !seed.is_empty(),
            });
        }
    }

    out
}

/// Greedily pick vocab words whose stress patterns concatenate to exactly
/// match `prefix`. Returns `(word, stress_slice)` tuples. Backs off a few
/// times if a greedy pick paints the search into a corner.
fn fill_prefix(
    vocab: &Vocab,
    prefix: &[u8],
    rng: &mut impl Rng,
) -> Option<Vec<(String, Vec<u8>)>> {
    if prefix.is_empty() {
        return Some(vec![]);
    }
    // Up to a handful of random restarts.
    for _ in 0..6 {
        if let Some(filled) = try_fill_prefix(vocab, prefix, rng) {
            return Some(filled);
        }
    }
    None
}

fn try_fill_prefix(
    vocab: &Vocab,
    prefix: &[u8],
    rng: &mut impl Rng,
) -> Option<Vec<(String, Vec<u8>)>> {
    let mut result: Vec<(String, Vec<u8>)> = Vec::new();
    let mut idx = 0usize;
    let mut safety = 8;
    while idx < prefix.len() && safety > 0 {
        safety -= 1;
        // Collect stress patterns whose length fits the remaining prefix AND
        // which match position-by-position.
        let remaining = &prefix[idx..];
        let candidates: Vec<&Vec<u8>> = vocab
            .by_stress
            .keys()
            .filter(|p| !p.is_empty() && p.len() <= remaining.len() && p.as_slice() == &remaining[..p.len()])
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let pat = candidates[rng.gen_range(0..candidates.len())];
        let pool = &vocab.by_stress[pat];
        let word = pool[rng.gen_range(0..pool.len())].clone();
        result.push((word, pat.clone()));
        idx += pat.len();
    }
    if idx == prefix.len() {
        Some(result)
    } else {
        None
    }
}
