//! Steno word expansion.
//!
//! The steno engine accumulates *phonemes* (from pair/solo strokes).
//! This module turns a phoneme sequence into ranked English word
//! candidates so the user types far fewer strokes than letters.
//!
//! # Why a custom dictionary instead of CMU
//!
//! The CMU Pronouncing Dictionary uses ARPAbet with ~39 phonemes.
//! Our steno layout only emits 20 (consonants t/n/s/l/r/d/k/m/p/b/ng
//! + vowels uh/a/e/o/i/u). Many ARPAbet phonemes (/f v θ ð ʃ ʒ h tʃ
//! dʒ j w g z/) have no key, so a direct CMU import would produce
//! thousands of words the user cannot actually stroke. For v1 we
//! hand-curate a focused high-frequency list using only reachable
//! phonemes, guaranteeing every entry is typeable. A future round
//! can import a filtered CMU subset once the phoneme inventory grows
//! (the "rare onset shift" gesture is filed in the design notes).
//!
//! # Schwa collapse / vowel fuzziness
//!
//! The design collapses unstressed English vowels to a single schwa
//! (`uh`). So "computer" might be stroked /k uh m p uh t uh/ even
//! though the citation form has distinct vowels. The matcher does
//! two passes:
//!   1. Exact phoneme match (unambiguous — best).
//!   2. Vowel-fuzzy — consonant skeleton must match exactly, any
//!      vowel matches any vowel.
//! Pass-1 candidates always outrank pass-2; within a pass, higher
//! word frequency (lower rank number) wins. Same principle as a
//! pinyin IME: approximate input, dictionary disambiguates.

/// A dictionary entry: phoneme spelling, surface word, frequency
/// rank (lower = more common; used for tie-breaking).
struct Entry {
    phonemes: &'static [&'static str],
    word: &'static str,
    rank: u32,
}

/// A ranked expansion candidate returned to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub word: String,
    /// True if from an exact phoneme match (vs vowel-fuzzy).
    pub exact: bool,
}

/// True if a phoneme is a vowel in our inventory.
fn is_vowel(p: &str) -> bool {
    matches!(p, "uh" | "a" | "e" | "o" | "i" | "u")
}

/// Curated dictionary. Phoneme spellings use ONLY the 20 reachable
/// phonemes. Ranks are rough frequency order (hand-set). Small for
/// v1 — the point is to prove the expansion loop end-to-end. Some
/// entries approximate sounds we lack a key for (e.g. "dog" uses a
/// final `g` spelled with our `k`-adjacent `ng`? no — we just omit
/// truly unspellable words). Words here are chosen to be both
/// high-frequency AND fully spellable in the reachable set.
#[rustfmt::skip]
static DICTIONARY: &[Entry] = &[
    // Short function words
    Entry { phonemes: &["i", "t"],            word: "it",    rank: 1 },
    Entry { phonemes: &["a", "t"],            word: "at",    rank: 2 },
    Entry { phonemes: &["o", "n"],            word: "on",    rank: 3 },
    Entry { phonemes: &["a", "n"],            word: "an",    rank: 4 },
    Entry { phonemes: &["uh", "p"],           word: "up",    rank: 5 },
    Entry { phonemes: &["n", "o"],            word: "no",    rank: 6 },
    Entry { phonemes: &["s", "o"],            word: "so",    rank: 7 },
    Entry { phonemes: &["m", "i"],            word: "me",    rank: 8 },
    Entry { phonemes: &["b", "i"],            word: "be",    rank: 9 },
    Entry { phonemes: &["d", "u"],            word: "do",    rank: 10 },
    Entry { phonemes: &["t", "u"],            word: "to",    rank: 11 },
    Entry { phonemes: &["m", "a", "i"],       word: "my",    rank: 13 },
    // CVC content words
    Entry { phonemes: &["k", "a", "t"],       word: "cat",   rank: 20 },
    Entry { phonemes: &["t", "o", "p"],       word: "top",   rank: 22 },
    Entry { phonemes: &["s", "t", "o", "p"],  word: "stop",  rank: 23 },
    Entry { phonemes: &["m", "a", "n"],       word: "man",   rank: 24 },
    Entry { phonemes: &["m", "a", "p"],       word: "map",   rank: 25 },
    Entry { phonemes: &["p", "e", "n"],       word: "pen",   rank: 26 },
    Entry { phonemes: &["t", "e", "n"],       word: "ten",   rank: 27 },
    Entry { phonemes: &["r", "u", "n"],       word: "run",   rank: 28 },
    Entry { phonemes: &["s", "u", "n"],       word: "sun",   rank: 29 },
    Entry { phonemes: &["b", "e", "d"],       word: "bed",   rank: 30 },
    Entry { phonemes: &["r", "e", "d"],       word: "red",   rank: 31 },
    Entry { phonemes: &["l", "e", "t"],       word: "let",   rank: 32 },
    Entry { phonemes: &["s", "e", "t"],       word: "set",   rank: 33 },
    Entry { phonemes: &["n", "o", "t"],       word: "not",   rank: 34 },
    Entry { phonemes: &["l", "o", "t"],       word: "lot",   rank: 35 },
    Entry { phonemes: &["p", "u", "t"],       word: "put",   rank: 36 },
    Entry { phonemes: &["k", "a", "n"],       word: "can",   rank: 37 },
    Entry { phonemes: &["m", "a", "d"],       word: "mad",   rank: 38 },
    Entry { phonemes: &["b", "a", "d"],       word: "bad",   rank: 39 },
    Entry { phonemes: &["s", "i", "t"],       word: "sit",   rank: 40 },
    Entry { phonemes: &["b", "i", "t"],       word: "bit",   rank: 41 },
    Entry { phonemes: &["k", "i", "t"],       word: "kit",   rank: 42 },
    Entry { phonemes: &["l", "i", "p"],       word: "lip",   rank: 43 },
    Entry { phonemes: &["t", "i", "p"],       word: "tip",   rank: 44 },
    Entry { phonemes: &["t", "a", "p"],       word: "tap",   rank: 45 },
    Entry { phonemes: &["n", "a", "p"],       word: "nap",   rank: 46 },
    Entry { phonemes: &["k", "i", "ng"],      word: "king",  rank: 47 },
    Entry { phonemes: &["r", "i", "ng"],      word: "ring",  rank: 48 },
    Entry { phonemes: &["s", "i", "ng"],      word: "sing",  rank: 49 },
    Entry { phonemes: &["l", "o", "ng"],      word: "long",  rank: 50 },
    Entry { phonemes: &["l", "a", "n", "d"],  word: "land",  rank: 52 },
    Entry { phonemes: &["s", "a", "n", "d"],  word: "sand",  rank: 53 },
    Entry { phonemes: &["b", "a", "n", "d"],  word: "band",  rank: 54 },
    Entry { phonemes: &["t", "e", "s", "t"],  word: "test",  rank: 55 },
    Entry { phonemes: &["b", "e", "s", "t"],  word: "best",  rank: 56 },
    Entry { phonemes: &["r", "e", "s", "t"],  word: "rest",  rank: 57 },
    Entry { phonemes: &["l", "a", "s", "t"],  word: "last",  rank: 58 },
    Entry { phonemes: &["m", "u", "s", "t"],  word: "must",  rank: 59 },
    Entry { phonemes: &["l", "i", "s", "t"],  word: "list",  rank: 60 },
    Entry { phonemes: &["p", "l", "a", "n"],  word: "plan",  rank: 61 },
    Entry { phonemes: &["s", "t", "a", "n", "d"], word: "stand", rank: 62 },
    Entry { phonemes: &["m", "a", "t", "uh", "r"], word: "matter", rank: 63 },
    Entry { phonemes: &["b", "e", "t", "uh", "r"], word: "better", rank: 64 },
    Entry { phonemes: &["l", "e", "t", "uh", "r"], word: "letter", rank: 65 },
];

/// Look up word candidates for an accumulated phoneme sequence.
/// Returns up to `max` candidates, exact matches first (by
/// frequency), then vowel-fuzzy matches (by frequency). Empty input
/// returns no candidates.
///
/// "Exact" means every phoneme equals the entry's phoneme at the
/// same position. "Vowel-fuzzy" means the sequences are the same
/// length and every consonant matches exactly while vowels are
/// treated as interchangeable — this is what makes schwa-collapsed
/// input ("stand" stroked with a schwa) still resolve.
pub fn lookup(phonemes: &[String], max: usize) -> Vec<Candidate> {
    if phonemes.is_empty() {
        return Vec::new();
    }

    let mut exact: Vec<(&Entry, ())> = Vec::new();
    let mut fuzzy: Vec<&Entry> = Vec::new();

    for entry in DICTIONARY {
        if entry.phonemes.len() != phonemes.len() {
            continue;
        }
        let mut is_exact = true;
        let mut is_fuzzy = true;
        for (got, want) in phonemes.iter().zip(entry.phonemes.iter()) {
            if got.as_str() == *want {
                continue; // matches both exact and fuzzy
            }
            is_exact = false;
            // Fuzzy still OK if BOTH positions are vowels.
            if !(is_vowel(got) && is_vowel(want)) {
                is_fuzzy = false;
                break;
            }
        }
        if is_exact {
            exact.push((entry, ()));
        } else if is_fuzzy {
            fuzzy.push(entry);
        }
    }

    // Sort each bucket by frequency rank (ascending = more common
    // first).
    exact.sort_by_key(|(e, _)| e.rank);
    fuzzy.sort_by_key(|e| e.rank);

    let mut out: Vec<Candidate> = Vec::with_capacity(max);
    for (e, _) in exact {
        if out.len() >= max {
            return out;
        }
        out.push(Candidate {
            word: e.word.to_string(),
            exact: true,
        });
    }
    for e in fuzzy {
        if out.len() >= max {
            break;
        }
        // Don't duplicate a word already added as exact.
        if out.iter().any(|c| c.word == e.word) {
            continue;
        }
        out.push(Candidate {
            word: e.word.to_string(),
            exact: false,
        });
    }
    out
}

/// True if any dictionary entry STARTS WITH the given phoneme
/// prefix (exact consonants, fuzzy vowels). Used by the engine to
/// decide whether to keep accumulating (a longer word is still
/// reachable) vs. commit now. Empty prefix is always a viable
/// prefix.
pub fn has_prefix(phonemes: &[String]) -> bool {
    if phonemes.is_empty() {
        return true;
    }
    'entry: for entry in DICTIONARY {
        if entry.phonemes.len() < phonemes.len() {
            continue;
        }
        for (got, want) in phonemes.iter().zip(entry.phonemes.iter()) {
            if got.as_str() == *want {
                continue;
            }
            if is_vowel(got) && is_vowel(want) {
                continue; // fuzzy vowel match
            }
            continue 'entry; // mismatch — try next entry
        }
        return true; // this entry has our sequence as a prefix
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ph(seq: &[&str]) -> Vec<String> {
        seq.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_input_no_candidates() {
        assert!(lookup(&[], 5).is_empty());
    }

    #[test]
    fn exact_match_ranks_first() {
        let c = lookup(&ph(&["k", "a", "t"]), 5);
        assert!(!c.is_empty());
        assert_eq!(c[0].word, "cat");
        assert!(c[0].exact);
    }

    #[test]
    fn vowel_fuzzy_resolves_schwa() {
        // "stand" stroked with a schwa instead of /a/: /s t uh n d/.
        // Consonant skeleton s-t-_-n-d matches "stand"; the vowel
        // differs (uh vs a) but both are vowels → fuzzy match.
        let c = lookup(&ph(&["s", "t", "uh", "n", "d"]), 5);
        assert!(c.iter().any(|x| x.word == "stand" && !x.exact),
            "expected fuzzy 'stand', got {:?}", c);
    }

    #[test]
    fn exact_outranks_fuzzy() {
        // /m a n/ exact-matches "man". /m i n/ would fuzzy-match
        // "man" too. With exact input, "man" should be exact and
        // first.
        let c = lookup(&ph(&["m", "a", "n"]), 5);
        assert_eq!(c[0].word, "man");
        assert!(c[0].exact);
    }

    #[test]
    fn length_mismatch_excluded() {
        // A 2-phoneme input shouldn't match a 3-phoneme entry.
        let c = lookup(&ph(&["k", "a"]), 5);
        assert!(!c.iter().any(|x| x.word == "cat"));
    }

    #[test]
    fn has_prefix_partial_word() {
        // /s t/ is a prefix of "stop" (s-t-o-p).
        assert!(has_prefix(&ph(&["s", "t"])));
        // /s t o/ still a prefix.
        assert!(has_prefix(&ph(&["s", "t", "o"])));
    }

    #[test]
    fn has_prefix_rejects_unreachable() {
        // No dictionary word starts /n g/ (ng is coda-only here).
        assert!(!has_prefix(&ph(&["ng", "ng"])));
    }

    #[test]
    fn max_caps_results() {
        let c = lookup(&ph(&["t", "e", "n"]), 1);
        assert!(c.len() <= 1);
    }
}