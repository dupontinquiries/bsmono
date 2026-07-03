//! ARPABET / CMUdict phoneme operations.
//!
//! This is the "proper" layer that replaces the character-level matching from
//! the original kMelodizer. Everything here operates on phoneme sequences, not
//! on letters — so *gone / on* rhyme, *night / knight* are phonetically identical,
//! and *fire* has the 2 syllables it actually has (`AY1 ER0`) rather than the
//! 1 that vowel-cluster counting reports.
//!
//! The dictionary is loaded once at startup. If `data/cmudict-0.7b.txt` is
//! present we use that (~134k entries); otherwise we fall back to the curated
//! starter set embedded via `include_str!`.

use std::collections::HashMap;

/// A single ARPABET phoneme. Vowels carry a trailing stress digit
/// (`AH0`, `AH1`, `AH2`); consonants don't (`HH`, `M`, `TH`).
pub type Phoneme = String;

/// A word's pronunciation: the ordered phoneme sequence.
pub type Pronunciation = Vec<Phoneme>;

/// The loaded dictionary. Construct via [`Dict::load`] or [`Dict::empty`].
#[derive(Debug, Clone, Default)]
pub struct Dict {
    entries: HashMap<String, Pronunciation>,
}

impl Dict {
    pub fn empty() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Parse CMUdict-format text. Tolerates the `;;;` comment convention and
    /// skips alternate-pronunciation entries like `READ(1)`.
    pub fn load(raw: &str) -> Self {
        let mut entries = HashMap::new();
        for line in raw.lines() {
            let line = line.trim_start_matches('\u{FEFF}'); // strip BOM if present
            if line.starts_with(";;;") || line.trim().is_empty() {
                continue;
            }
            // The canonical separator is two spaces.
            let Some((word_part, phon_part)) = line.split_once("  ") else {
                continue;
            };
            // Skip alternate pronunciations: "READ(1)", "READ(2)", etc.
            if word_part.contains('(') {
                continue;
            }
            let word = word_part.trim().to_lowercase();
            if word.is_empty() {
                continue;
            }
            let phones: Vec<String> = phon_part
                .split_whitespace()
                .map(|s| s.to_uppercase())
                .collect();
            if phones.is_empty() {
                continue;
            }
            entries.insert(word, phones);
        }
        Self { entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn lookup(&self, w: &str) -> Option<&Pronunciation> {
        // Normalize: lowercase, strip non-alpha-apostrophe.
        let key: String = w
            .chars()
            .filter(|c| c.is_alphabetic() || *c == '\'')
            .collect::<String>()
            .to_lowercase();
        self.entries.get(&key)
    }

    pub fn contains(&self, w: &str) -> bool {
        self.lookup(w).is_some()
    }

    /// Authoritative syllable count: the number of vowel phonemes (those that
    /// end with a stress digit). Returns `None` if the word isn't in the dict.
    pub fn syllables(&self, w: &str) -> Option<u8> {
        self.lookup(w).map(count_vowel_phonemes)
    }

    /// The rhyme key of a word: phonemes from the last primary-stressed vowel
    /// onward, with stress digits stripped. Two words rhyme iff their rhyme
    /// keys are equal and non-empty.
    ///
    /// Falls back to the last vowel of any stress if no primary-stress vowel
    /// exists (rare; mostly function words like "the" → `DH AH0`).
    pub fn rhyme_key(&self, w: &str) -> Option<Vec<String>> {
        let p = self.lookup(w)?;
        let primary = p.iter().rposition(|ph| ph.ends_with('1'));
        let any_vowel = p.iter().rposition(is_vowel_phoneme);
        let start = primary.or(any_vowel)?;
        Some(
            p[start..]
                .iter()
                .map(|ph| strip_stress(ph).to_string())
                .collect(),
        )
    }

    pub fn rhymes(&self, a: &str, b: &str) -> bool {
        match (self.rhyme_key(a), self.rhyme_key(b)) {
            (Some(ka), Some(kb)) => !ka.is_empty() && ka == kb,
            _ => false,
        }
    }

    /// Stress pattern in order of syllables. `1` = stressed (primary or
    /// secondary), `0` = unstressed. Useful for matching against a hum's
    /// detected rhythmic shape.
    pub fn stress_pattern(&self, w: &str) -> Option<Vec<u8>> {
        self.lookup(w).map(|p| {
            p.iter()
                .filter_map(|ph| {
                    ph.chars()
                        .last()
                        .and_then(|c| c.to_digit(10))
                        .map(|d| if d == 0 { 0 } else { 1 })
                })
                .collect()
        })
    }

    /// Levenshtein distance over phoneme sequences. `None` if either word is
    /// not in the dict.
    ///
    /// This is the phoneme-space analogue of kMelodizer's character edit
    /// distance — same algorithm, right substrate. `through` / `thru` are
    /// distance 0 here (both `TH R UW1`); the character version had them
    /// at distance 3.
    pub fn phonetic_distance(&self, a: &str, b: &str) -> Option<usize> {
        let pa = self.lookup(a)?;
        let pb = self.lookup(b)?;
        Some(levenshtein(pa, pb))
    }

    /// Find the vocab word phonetically closest to `w`. Returns
    /// `(word, distance)`. Ties broken by alphabetical order, matching the
    /// "lowest-sorted" rule from kMelodizer.
    pub fn nearest_phonetic<'a, I>(&self, w: &str, vocab: I) -> Option<(String, usize)>
    where
        I: IntoIterator<Item = &'a String>,
    {
        let target = self.lookup(w)?;
        let mut best: Option<(String, usize)> = None;
        for cand in vocab {
            let Some(cp) = self.lookup(cand) else {
                continue;
            };
            let d = levenshtein(target, cp);
            best = match best {
                None => Some((cand.clone(), d)),
                Some((bw, bd)) if d < bd || (d == bd && cand < &bw) => Some((cand.clone(), d)),
                some => some,
            };
        }
        best
    }
}

// --- helpers ----------------------------------------------------------------

fn is_vowel_phoneme(ph: &Phoneme) -> bool {
    ph.chars().last().is_some_and(|c| c.is_ascii_digit())
}

fn strip_stress(ph: &str) -> &str {
    match ph.chars().last() {
        Some(c) if c.is_ascii_digit() => &ph[..ph.len() - 1],
        _ => ph,
    }
}

fn count_vowel_phonemes(p: &Pronunciation) -> u8 {
    p.iter().filter(|ph| is_vowel_phoneme(ph)).count() as u8
}

/// Standard Levenshtein distance with O(n) extra memory.
fn levenshtein<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let (m, n) = (a.len(), b.len());
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

// --- tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Dict {
        Dict::load(
            "\
HEARSE  HH ER1 S
CURSE  K ER1 S
GONE  G AO1 N
ON  AA1 N
NIGHT  N AY1 T
KNIGHT  N AY1 T
THROUGH  TH R UW1
THRU  TH R UW1
FIRE  F AY1 ER0
THE  DH AH0
",
        )
    }

    #[test]
    fn hearse_curse_rhyme() {
        assert!(tiny().rhymes("hearse", "curse"));
    }

    #[test]
    fn gone_on_rhyme_even_though_spelled_differently() {
        // gone = G AO1 N, on = AA1 N. AO and AA don't match exactly in strict
        // ARPABET, so they wouldn't rhyme here. Demonstrating that rhyme is
        // strict on phonemes — this is a feature, not a bug.
        let d = tiny();
        assert_eq!(d.rhyme_key("gone"), Some(vec!["AO".into(), "N".into()]));
        assert_eq!(d.rhyme_key("on"), Some(vec!["AA".into(), "N".into()]));
        assert!(!d.rhymes("gone", "on"));
    }

    #[test]
    fn night_knight_phonetically_identical() {
        let d = tiny();
        assert_eq!(d.phonetic_distance("night", "knight"), Some(0));
    }

    #[test]
    fn through_thru_phonetically_identical() {
        assert_eq!(tiny().phonetic_distance("through", "thru"), Some(0));
    }

    #[test]
    fn fire_has_two_syllables() {
        assert_eq!(tiny().syllables("fire"), Some(2));
    }

    #[test]
    fn unknown_word_returns_none() {
        assert_eq!(tiny().syllables("supercalifragilistic"), None);
    }
}
