//! v0.1.704: word error rate (WER) scoring for the recorded-voice corpus
//! harness. The user's corpus pairs each .opus recording with a reference
//! .txt; the harness transcribes the audio with whisper and scores the result
//! against the reference. WER = (substitutions + insertions + deletions) /
//! reference_words, via word-level Levenshtein. PURE + testable.
//!
//! Normalization matters: real transcripts have punctuation, case, and odd
//! separators (the corpus uses "⁄", "·", "↓", emoji). We lowercase, strip
//! punctuation/symbols to spaces, and collapse whitespace so scoring reflects
//! recognition quality, not formatting noise.

/// Normalize a transcript for comparison: lowercase, non-alphanumeric → space,
/// collapse runs of whitespace. Returns the word vector.
pub fn normalize_words(s: &str) -> Vec<String> {
    let mut cur = String::new();
    let mut out = Vec::new();
    for ch in s.chars() {
        // v0.1.706: count only ASCII alphanumerics as word characters. Real
        // reference transcripts carry mojibake/symbol artifacts (the corpus
        // has "â" where a "→" was double-encoded upstream, plus ·↓ separators
        // and emoji). Treating "â" as a word inflated WER by one guaranteed
        // error on every affected reference, since whisper never emits it from
        // clean audio. Non-ASCII letters in genuine words (café, naïve) are
        // rare in this command/dictation corpus and degrade gracefully (the
        // accented letter is dropped, leaving "caf"/"naive"-ish) rather than
        // adding a phantom token. ASCII-only is the right tradeoff here.
        if ch.is_ascii_alphanumeric() {
            cur.push(ch.to_ascii_lowercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() { out.push(cur); }
    out
}

/// Word-level edit distance (Levenshtein) between two token slices.
pub fn word_edit_distance(a: &[String], b: &[String]) -> usize {
    let (n, m) = (a.len(), b.len());
    if n == 0 { return m; }
    if m == 0 { return n; }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0usize; m + 1];
    for i in 1..=n {
        cur[0] = i;
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1)        // deletion
                .min(cur[j - 1] + 1)      // insertion
                .min(prev[j - 1] + cost); // substitution / match
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m]
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WerResult {
    pub ref_words: usize,
    pub edits: usize,
    /// edits / ref_words, clamped to [0, 1.0+] (can exceed 1 with many inserts)
    pub wer: f32,
}

/// Score a hypothesis transcript against the reference.
pub fn score(reference: &str, hypothesis: &str) -> WerResult {
    let r = normalize_words(reference);
    let h = normalize_words(hypothesis);
    let edits = word_edit_distance(&r, &h);
    let ref_words = r.len();
    let wer = if ref_words == 0 {
        if h.is_empty() { 0.0 } else { 1.0 }
    } else {
        edits as f32 / ref_words as f32
    };
    WerResult { ref_words, edits, wer }
}

/// Aggregate WER across many (ref, hyp) pairs — pooled (total edits / total
/// ref words), the standard way to report corpus WER (not the mean of per-
/// utterance WERs, which over-weights short clips).
pub fn corpus_wer(pairs: &[(String, String)]) -> WerResult {
    let mut total_edits = 0usize;
    let mut total_ref = 0usize;
    for (r, h) in pairs {
        let res = score(r, h);
        total_edits += res.edits;
        total_ref += res.ref_words;
    }
    let wer = if total_ref == 0 { 0.0 } else { total_edits as f32 / total_ref as f32 };
    WerResult { ref_words: total_ref, edits: total_edits, wer }
}

/// v0.1.705: a single alignment step between reference and hypothesis.
#[derive(Debug, Clone, PartialEq)]
pub enum EditOp {
    Match(String),
    Sub(String, String), // (reference word, hypothesis word)
    Ins(String),         // word in hyp not in ref
    Del(String),         // word in ref not in hyp
}

/// Align `ref` against `hyp` at the word level (Levenshtein backtrace).
/// Used for confusion mining — which words the recognizer actually swaps.
/// Corpus clips are short, so the full O(n·m) matrix is fine.
pub fn align(reference: &str, hypothesis: &str) -> Vec<EditOp> {
    let r = normalize_words(reference);
    let h = normalize_words(hypothesis);
    let (n, m) = (r.len(), h.len());
    // dp[i][j] = edit distance between r[..i] and h[..j]
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in 0..=n { dp[i][0] = i; }
    for j in 0..=m { dp[0][j] = j; }
    for i in 1..=n {
        for j in 1..=m {
            let cost = if r[i - 1] == h[j - 1] { 0 } else { 1 };
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }
    // backtrace from (n, m)
    let mut ops = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 {
            let cost = if r[i - 1] == h[j - 1] { 0 } else { 1 };
            if dp[i][j] == dp[i - 1][j - 1] + cost {
                ops.push(if cost == 0 { EditOp::Match(r[i - 1].clone()) }
                         else { EditOp::Sub(r[i - 1].clone(), h[j - 1].clone()) });
                i -= 1; j -= 1; continue;
            }
        }
        if i > 0 && dp[i][j] == dp[i - 1][j] + 1 {
            ops.push(EditOp::Del(r[i - 1].clone())); i -= 1; continue;
        }
        // remaining: insertion
        ops.push(EditOp::Ins(h[j - 1].clone())); j -= 1;
    }
    ops.reverse();
    ops
}

/// Just the substitution pairs (ref→hyp) from an alignment — the raw material
/// for confusion mining. Feeds confusion_table to surface systematic mishears.
pub fn substitutions(reference: &str, hypothesis: &str) -> Vec<(String, String)> {
    align(reference, hypothesis).into_iter().filter_map(|op| match op {
        EditOp::Sub(a, b) => Some((a, b)),
        _ => None,
    }).collect()
}

/// v0.1.705 (#1): per-speaker WER from labeled (speaker, ref, hyp) triples,
/// each speaker's pairs pooled. Sorted by speaker for stable reports.
pub fn group_wer(labeled: &[(String, String, String)]) -> Vec<(String, WerResult)> {
    use std::collections::BTreeMap;
    let mut by: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (spk, r, h) in labeled {
        by.entry(spk.clone()).or_default().push((r.clone(), h.clone()));
    }
    by.into_iter().map(|(spk, pairs)| (spk, corpus_wer(&pairs))).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerDelta {
    pub speaker: String,
    pub baseline_wer: f32,
    pub current_wer: f32,
    pub regressed: bool,
}

/// v0.1.705 (#1): diff current per-speaker WER against a saved baseline.
/// A speaker regressed if current exceeds baseline by more than `tolerance`.
/// New speakers (not in baseline) are reported with baseline_wer = -1.
pub fn diff_baseline(baseline: &[(String, f32)], current: &[(String, WerResult)],
                     tolerance: f32) -> Vec<SpeakerDelta> {
    current.iter().map(|(spk, res)| {
        let base = baseline.iter().find(|(s, _)| s == spk).map(|(_, w)| *w);
        let regressed = match base {
            Some(b) => res.wer > b + tolerance,
            None => false, // unseen speaker isn't a regression
        };
        SpeakerDelta {
            speaker: spk.clone(),
            baseline_wer: base.unwrap_or(-1.0),
            current_wer: res.wer,
            regressed,
        }
    }).collect()
}

/// v0.1.705 (#3): is a phrase unreliable to TEST with synthesized TTS? True
/// when the synthesized-audio WER is much worse than the real-recording WER —
/// i.e. TTS mispronounces it, so a failure there is a TTS artifact, not a real
/// recognizer regression. Such phrases should be excluded from the TTS harness
/// (the user's "words to not test TTS with if they sound off").
pub fn tts_unreliable(real_wer: f32, synth_wer: f32, margin: f32) -> bool {
    synth_wer - real_wer > margin
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_punct_and_symbols() {
        // the corpus uses odd separators; they shouldn't count as words
        let w = normalize_words("Go back ⁄ Quick settings · Tap!");
        assert_eq!(w, vec!["go", "back", "quick", "settings", "tap"]);
    }

    #[test]
    fn normalize_drops_mojibake_artifacts() {
        // real corpus bug: "→" double-encoded to "â". It must NOT count as a
        // word, or every affected reference gets a phantom error vs whisper.
        let w = normalize_words("welcome â getting started");
        assert_eq!(w, vec!["welcome", "getting", "started"]);
        // emoji + bullets likewise contribute no words
        assert_eq!(normalize_words("3️⃣ New Decepticons ·"), vec!["3", "new", "decepticons"]);
    }

    #[test]
    fn identical_is_zero() {
        let r = score("get the highlights", "Get the highlights.");
        assert_eq!(r.edits, 0);
        assert_eq!(r.wer, 0.0);
    }

    #[test]
    fn one_substitution() {
        // "the" → "a": 1 edit of 3 words
        let r = score("get the highlights", "get a highlights");
        assert_eq!(r.edits, 1);
        assert!((r.wer - 1.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn insertion_and_deletion() {
        // ref 3 words, hyp drops one + adds one
        let ins = score("amazon search terms", "amazon search the terms");
        assert_eq!(ins.edits, 1);
        let del = score("amazon search terms", "amazon terms");
        assert_eq!(del.edits, 1);
    }

    #[test]
    fn empty_hypothesis_is_full_wer() {
        let r = score("three words here", "");
        assert_eq!(r.edits, 3);
        assert_eq!(r.wer, 1.0);
    }

    #[test]
    fn empty_reference_guards() {
        assert_eq!(score("", "").wer, 0.0);
        assert_eq!(score("", "spurious").wer, 1.0);
    }

    #[test]
    fn corpus_wer_is_pooled() {
        // two utterances: 1 edit / 3 words, and 0 edits / 2 words
        // pooled = 1 / 5 = 0.2 (NOT the mean of 0.333 and 0.0 = 0.167)
        let pairs = vec![
            ("get the highlights".to_string(), "get a highlights".to_string()),
            ("amazon terms".to_string(), "amazon terms".to_string()),
        ];
        let r = corpus_wer(&pairs);
        assert_eq!(r.ref_words, 5);
        assert_eq!(r.edits, 1);
        assert!((r.wer - 0.2).abs() < 0.001);
    }

    #[test]
    fn case_and_punctuation_ignored() {
        let r = score("Curl the URL", "curl the url");
        assert_eq!(r.edits, 0);
    }

    #[test]
    fn align_classifies_ops() {
        // ref: a b c d   hyp: a x c   → match a, sub b→x, match c, del d
        let ops = align("a b c d", "a x c");
        assert_eq!(ops, vec![
            EditOp::Match("a".into()),
            EditOp::Sub("b".into(), "x".into()),
            EditOp::Match("c".into()),
            EditOp::Del("d".into()),
        ]);
    }

    #[test]
    fn align_insertion() {
        // ref: a c   hyp: a b c → insertion of b
        let ops = align("a c", "a b c");
        assert!(ops.contains(&EditOp::Ins("b".into())));
        assert!(ops.contains(&EditOp::Match("a".into())));
        assert!(ops.contains(&EditOp::Match("c".into())));
    }

    #[test]
    fn substitutions_extracted() {
        let subs = substitutions("connect to prod", "connect to dev");
        assert_eq!(subs, vec![("prod".to_string(), "dev".to_string())]);
        // no subs when identical
        assert!(substitutions("get the highlights", "get the highlights").is_empty());
    }

    #[test]
    fn group_wer_per_speaker_pooled() {
        let labeled = vec![
            ("alice".to_string(), "get the highlights".to_string(), "get a highlights".to_string()),
            ("alice".to_string(), "amazon terms".to_string(), "amazon terms".to_string()),
            ("bob".to_string(), "open the editor".to_string(), "open the editor".to_string()),
        ];
        let g = group_wer(&labeled);
        assert_eq!(g.len(), 2);
        // alice: 1 edit / 5 words = 0.2
        let alice = g.iter().find(|(s, _)| s == "alice").unwrap();
        assert!((alice.1.wer - 0.2).abs() < 0.001);
        // bob: perfect
        let bob = g.iter().find(|(s, _)| s == "bob").unwrap();
        assert_eq!(bob.1.wer, 0.0);
    }

    #[test]
    fn diff_baseline_flags_regression() {
        let current = vec![
            ("alice".to_string(), WerResult { ref_words: 10, edits: 4, wer: 0.40 }),
            ("bob".to_string(), WerResult { ref_words: 10, edits: 1, wer: 0.10 }),
            ("carol".to_string(), WerResult { ref_words: 5, edits: 0, wer: 0.0 }), // new
        ];
        let baseline = vec![("alice".to_string(), 0.20), ("bob".to_string(), 0.12)];
        let deltas = diff_baseline(&baseline, &current, 0.05);
        let alice = deltas.iter().find(|d| d.speaker == "alice").unwrap();
        assert!(alice.regressed, "0.40 > 0.20 + 0.05");
        let bob = deltas.iter().find(|d| d.speaker == "bob").unwrap();
        assert!(!bob.regressed, "0.10 improved vs 0.12");
        let carol = deltas.iter().find(|d| d.speaker == "carol").unwrap();
        assert!(!carol.regressed && carol.baseline_wer < 0.0, "new speaker, no baseline");
    }

    #[test]
    fn tts_unreliable_flags_divergence() {
        // real recording transcribes fine (0.05) but TTS makes it 0.50 → bad
        // phrase to test TTS with.
        assert!(tts_unreliable(0.05, 0.50, 0.25));
        // both similar → fine
        assert!(!tts_unreliable(0.10, 0.15, 0.25));
    }
}
