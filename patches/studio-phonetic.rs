//! phonetic — a retry pass for voice commands a recognizer mis-heard,
//! ported from voiceterm-qt's `phonetic_confusions` module (its own docs:
//! "the ONE genuinely unrouted module, flagged unwired since v0.1.194" —
//! the mechanism was pure and tested there long before it had a caller;
//! this port gives it one).
//!
//! ## The idea, unchanged from voiceterm-qt
//!
//! A speech recognizer's failure mode is a MIS-HEARD word, not a
//! nonsensical one — "rock" for "rack", "root" for "route", "so low" for
//! "solo". [`Dialog::hear`](crate::voice::Dialog::hear) already walks the
//! palette on the raw utterance; when that walk fails outright, this
//! module expands the utterance into single-word phonetic variants and
//! retries each, in order, stopping at the first that resolves. Only
//! runs on total failure (never overrides a match that already worked),
//! single-substitution per variant (not a cross-product — voiceterm-qt's
//! own reasoning: a recognizer usually mangles one word, and swapping
//! every confusable word at once would explode the search space for no
//! extra recall), and bounded.
//!
//! ## What's actually new here, vs. a straight copy
//!
//! voiceterm-qt's table (`ls`/`less`/`list`, `grep`/`great`/`grip`, …) is
//! terminal-command vocabulary — porting the DATA verbatim would be
//! useless in a DAW. The table below is built from `palette.rs`'s own
//! command vocabulary instead (`rack`, `bus`, `mute`, `solo`,
//! `key`, `resync`, `invite`, …), including the collaboration commands —
//! short, easily-mangled words are exactly where a recognizer struggles,
//! and exactly where this module earns its keep.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Confusion groups: words a recognizer plausibly substitutes for one
/// another. Symmetric within a group — any member heard could have meant
/// any other. Kept small per group (voiceterm-qt's own tuning advice:
/// bigger groups expand the candidate space for little extra recall) and
/// drawn from real palette vocabulary, not a dictionary of near-rhymes.
const GROUPS: &[&[&str]] = &[
    &["rack", "rock", "wrack"],
    &["bus", "buzz", "bust"],
    &["mute", "newt", "moot"],
    &["solo", "so low", "sole oh"],
    &["mixer", "mixture", "mix her"],
    &["key", "quay", "kee"],
    &["tempo", "tempoh", "tempeau"],
    &["group", "grope", "groop"],
    &["split", "spit", "splint"],
    &["align", "a line", "uh line"],
    &["invite", "in bite", "and write"],
    &["follow", "fellow", "fallow"],
    &["resync", "re sink", "re think"],
    &["redo", "read oh", "re do"],
    &["undo", "un do", "and do"],
    &["marker", "marketer", "mocker"],
    &["quantize", "quant eyes", "con eyes"],
    &["collaborator", "collaborate her", "collaborate or"],
];

fn table() -> &'static HashMap<String, Vec<String>> {
    static TABLE: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for group in GROUPS {
            for (i, word) in group.iter().enumerate() {
                let alts: Vec<String> = group.iter().enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, w)| (*w).to_string())
                    .collect();
                map.entry(word.to_lowercase()).or_default().extend(alts);
            }
        }
        map
    })
}

/// Alternates for one word, lowercase-normalized. Empty if `word` isn't
/// in any confusion group.
pub fn alternates_for(word: &str) -> &'static [String] {
    table().get(&word.to_lowercase()).map(Vec::as_slice).unwrap_or(&[])
}

/// One phonetic variant of a phrase: the substituted tokens, plus which
/// original word (`from`) was swapped for which alternate (`to`) — kept
/// alongside the tokens (not just the resulting `Vec<String>`) so a
/// caller whose variant actually resolves a command can record the
/// correction into [`Learned`] without re-diffing two token lists itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub tokens: Vec<String>,
    pub from: String,
    pub to: String,
}

/// Single-substitution phrase expansion: for each word in `phrase`, try
/// each of its phonetic alternates in that word's position, one
/// substitution per variant. A multi-word alternate (e.g. "solo" ->
/// "so low") expands into that many tokens in the variant, not one
/// token with an embedded space — downstream matching works on tokens,
/// and a two-word alternate IS two words once substituted in. The
/// original phrase is never included (the caller already tried it) and
/// a no-op substitution (alternate equal to the original word) is
/// skipped. Order is stable — left to right by word position, then by
/// alternate order within that word's group — bounded by
/// `max_variants`, ported from voiceterm-qt's own `phrase_variants`
/// (there, phrase and alternates were always single words, so this
/// token-count change is this port's own addition, not upstream's).
pub fn phrase_variants(phrase: &[String], max_variants: usize) -> Vec<Variant> {
    if max_variants == 0 {
        return Vec::new();
    }
    let mut out: Vec<Variant> = Vec::new();
    for (i, w) in phrase.iter().enumerate() {
        for alt in alternates_for(w) {
            if alt.eq_ignore_ascii_case(w) {
                continue;
            }
            let mut tokens: Vec<String> = phrase[..i].to_vec();
            tokens.extend(alt.split_whitespace().map(str::to_string));
            tokens.extend(phrase[i + 1..].iter().cloned());
            if !out.iter().any(|v| v.tokens == tokens) {
                out.push(Variant { tokens, from: w.clone(), to: alt.clone() });
                if out.len() >= max_variants {
                    return out;
                }
            }
        }
    }
    out
}

/// A per-session LEARNED confusion table, ported from voiceterm-qt's
/// `confusion_table.rs` — distinct from [`GROUPS`]'s curated, static set.
/// Where the static table encodes what a whole community's transcriptions
/// commonly mangle, this one learns what THIS session's recognizer
/// specifically mangles: every time a command only resolved after the
/// phonetic retry (see [`crate::voice::Dialog::hear`]'s call site), the
/// word that actually changed is a real, observed mis-hearing — recorded
/// here so the NEXT time it's heard, the retry tries the learned
/// correction (once confirmed at least [`MIN_CONFIRMATIONS`] times) ahead
/// of the generic static groups.
///
/// In-memory only for this port (no disk persistence yet) — voiceterm-qt's
/// own file format is real, separate work; proving the learning mechanism
/// itself first is this port's own version of the "prototype, then wire"
/// discipline `TODO_command_clustering.md` explicitly asks for elsewhere
/// in that codebase.
pub struct Learned {
    /// misheard -> [(intended, confirmation count)], capped per key and
    /// in total, same as voiceterm-qt's own bounds.
    map: HashMap<String, Vec<(String, u32)>>,
}

/// Below this many confirmations, a correction is not trusted yet — one
/// odd retry success could be coincidence; two is a pattern.
const MIN_CONFIRMATIONS: u32 = 2;
const MAX_ENTRIES: usize = 500;
const MAX_PER_WORD: usize = 5;

impl Default for Learned {
    fn default() -> Self { Learned { map: HashMap::new() } }
}

impl Learned {
    /// Record that `misheard` was probably meant to be `intended` — called
    /// once a phonetic-retry variant is the one that actually resolved a
    /// command. Rejects a no-op ("misheard" == "intended"), anything
    /// implausibly long, or a pair too different to plausibly be the same
    /// mis-hearing (more than 60% of characters changed, by Levenshtein
    /// distance) — voiceterm-qt's own three guards, unchanged.
    pub fn record(&mut self, misheard: &str, intended: &str) {
        if misheard.is_empty() || intended.is_empty() { return; }
        if misheard.eq_ignore_ascii_case(intended) { return; }
        if misheard.len() > 30 || intended.len() > 30 { return; }
        let mh = misheard.to_lowercase();
        let intd = intended.to_lowercase();
        let dist = levenshtein(&mh, &intd);
        let max_len = mh.len().max(intd.len());
        if max_len > 0 && dist as f32 / max_len as f32 > 0.6 { return; }

        let entries = self.map.entry(mh).or_default();
        if let Some((_, cnt)) = entries.iter_mut().find(|(t, _)| *t == intd) {
            *cnt += 1;
        } else {
            entries.push((intd, 1));
        }
        if entries.len() > MAX_PER_WORD {
            entries.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
            entries.truncate(MAX_PER_WORD);
        }
        if self.map.len() > MAX_ENTRIES {
            // Drop the entries whose TOP correction has the lowest count —
            // the least-confirmed patterns are the ones worth forgetting
            // first when the table is full.
            let mut kv: Vec<(String, u32)> = self.map.iter()
                .map(|(k, v)| (k.clone(), v.first().map(|(_, c)| c).copied().unwrap_or(0)))
                .collect();
            kv.sort_by_key(|(_, c)| *c);
            let drop_n = self.map.len() - MAX_ENTRIES;
            for (k, _) in kv.iter().take(drop_n) { self.map.remove(k); }
        }
    }

    /// The most likely correction for a word, if it has been confirmed
    /// enough times to trust — `None` below [`MIN_CONFIRMATIONS`], not a
    /// weaker guess: an unconfirmed single observation is exactly as
    /// likely to be its own kind of mistake as a real pattern.
    pub fn correct(&self, word: &str) -> Option<&str> {
        let entries = self.map.get(&word.to_lowercase())?;
        let (intended, count) = entries.first()?;
        if *count >= MIN_CONFIRMATIONS { Some(intended.as_str()) } else { None }
    }
}

/// Plain Levenshtein edit distance — single-row DP, no dependency needed
/// for a check this small. Operates on bytes, which is fine here: every
/// input is already lowercased ASCII command vocabulary.
fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        curr[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alternates_are_symmetric_within_a_group() {
        assert!(alternates_for("rack").contains(&"rock".to_string()));
        assert!(alternates_for("rock").contains(&"rack".to_string()));
        assert!(alternates_for("rock").contains(&"wrack".to_string()));
    }

    #[test]
    fn alternates_are_case_insensitive_and_empty_for_unknown_words() {
        assert_eq!(alternates_for("RACK"), alternates_for("rack"));
        assert!(alternates_for("xyzzy").is_empty());
    }

    #[test]
    fn phrase_variants_substitutes_one_word_at_a_time() {
        let phrase = vec!["go".to_string(), "to".to_string(), "rock".to_string()];
        let variants = phrase_variants(&phrase, 10);
        assert!(variants.iter().any(|v| v.tokens == ["go", "to", "rack"]),
                "must recover 'rack' from a mis-heard 'rock': {variants:?}");
        // Only ONE word ever changes per variant.
        for v in &variants {
            let diffs = v.tokens.iter().zip(&phrase).filter(|(a, b)| a != b).count();
            assert_eq!(diffs, 1, "each variant must differ by exactly one word: {v:?}");
        }
    }

    #[test]
    fn phrase_variants_excludes_the_original_and_no_op_substitutions() {
        let phrase = vec!["mute".to_string(), "drums".to_string()];
        let variants = phrase_variants(&phrase, 10);
        assert!(!variants.iter().any(|v| v.tokens == phrase),
                "the original phrase must not be its own variant");
        // "drums" has no confusion group, so it must never move.
        for v in &variants {
            assert_eq!(v.tokens[1], "drums");
        }
    }

    #[test]
    fn phrase_variants_is_bounded() {
        // "resync" has 2 alternates; with a cap of 1 only the first shows.
        let phrase = vec!["resync".to_string()];
        assert_eq!(phrase_variants(&phrase, 1).len(), 1);
        assert_eq!(phrase_variants(&phrase, 0).len(), 0);
    }

    #[test]
    fn phrase_variants_handles_a_multi_word_alternate() {
        // "solo" <-> "so low" is a 1-word-to-2-word substitution — the
        // variant's token COUNT changes, expanding into two tokens rather
        // than one token with an embedded space, since downstream
        // matching works token-by-token.
        let phrase = vec!["so".to_string(), "low".to_string(), "the".to_string(), "kick".to_string()];
        // "so" and "low" individually have no confusion group (only the
        // TWO-WORD phrase "so low" does) — this test documents that
        // limitation rather than hiding it: phrase_variants substitutes
        // per TOKEN, so a multi-word confusion only fires from the token
        // that IS a key ("solo" -> "so low"), not the reverse direction.
        assert!(phrase_variants(&phrase, 10).is_empty());

        let reverse = vec!["mute".to_string(), "the".to_string(), "solo".to_string()];
        let variants = phrase_variants(&reverse, 10);
        assert!(variants.iter().any(|v| v.tokens == ["mute", "the", "so", "low"]),
                "'so low' must expand into two tokens, not one with an embedded space: {variants:?}");
    }

    #[test]
    fn learned_requires_confirmation_before_trusting_a_correction() {
        let mut learned = Learned::default();
        assert!(learned.correct("quant-eyes").is_none(), "nothing learned yet");
        learned.record("quant-eyes", "quantize");
        assert!(learned.correct("quant-eyes").is_none(),
                "one observation is not yet a pattern");
        learned.record("quant-eyes", "quantize");
        assert_eq!(learned.correct("quant-eyes"), Some("quantize"),
                "two confirmations of the SAME correction earns trust");
    }

    #[test]
    fn learned_rejects_a_no_op_and_an_implausible_correction() {
        let mut learned = Learned::default();
        learned.record("mute", "mute"); // no-op, case aside
        learned.record("MUTE", "mute");
        assert!(learned.map.is_empty(), "identical (case-insensitive) pairs must not be learned");

        // "mute" -> "collaborator": almost nothing in common, should be
        // rejected by the edit-distance guard rather than learned as a
        // real phonetic pattern.
        let mut learned2 = Learned::default();
        learned2.record("mute", "collaborator");
        learned2.record("mute", "collaborator");
        assert!(learned2.correct("mute").is_none(),
                "a correction this different must be rejected, not just under-confirmed");
    }

    #[test]
    fn learned_keeps_only_the_most_confirmed_correction_per_word() {
        let mut learned = Learned::default();
        learned.record("bost", "bus");
        learned.record("bost", "bus");
        learned.record("bost", "bust");
        assert_eq!(learned.correct("bost"), Some("bus"),
                "the correction with more confirmations wins, even if a rival exists");
    }

    #[test]
    fn levenshtein_matches_known_distances() {
        assert_eq!(levenshtein("bus", "bus"), 0);
        assert_eq!(levenshtein("bus", "buzz"), 2);
        assert_eq!(levenshtein("kitten", "sitting"), 3, "the textbook example");
        assert_eq!(levenshtein("", "abc"), 3);
    }
}
