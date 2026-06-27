//! hallucination.rs — whisper hallucination filtering.
//!
//! Whisper.cpp emits plausible short phrases on near-silent / noise-only audio
//! (its statistical fallback for "no words here, emit the highest-probability
//! sentence-shaped tokens"): "Thank you.", "Thanks for watching.", "[BLANK_AUDIO]",
//! and it loops repeated phrases ("the the the world", "run cargo test, run cargo
//! test, …"). Both rusty-board (`voice/hallucination.rs`) and voiceterm/web2qml
//! (`voice.rs`) filter these, with complementary coverage. This module is the
//! union:
//!   * [`is_hallucination`] — drop a short partial that matches the canonical list,
//!   * [`strip_trailing_artifacts`] — remove `[BLANK_AUDIO]`-style tokens off the end,
//!   * [`trim_hallucination_loop`] — collapse a repeated-shingle loop,
//!   * [`looks_like_speech`] — a gate: length, single-word repetition, bad phrases.
//! Pure, zero-dependency.

/// True if `text` is a known short whisper hallucination to suppress from a
/// streaming partial. Conservative: the canonical list is matched exactly (after
/// case/trailing-punctuation normalization), not as a prefix — "thank you very
/// much for the help" is NOT filtered. Empty / whitespace / pure-punctuation is.
pub fn is_hallucination(text: &str) -> bool {
    let n = normalize(text);
    n.is_empty() || HALLUCINATIONS.contains(&n.as_str())
}

/// Strip trailing whisper artifacts (`[BLANK_AUDIO]`, `(silence)`, `[music]`, …)
/// off the END of a transcript, preserving the leading content verbatim
/// (case-insensitive match). Artifacts in the MIDDLE are left in place:
/// `"hello [BLANK_AUDIO] world"` is unchanged.
pub fn strip_trailing_artifacts(text: &str) -> String {
    let mut s = text.trim_end().to_string();
    loop {
        let t = s.trim_end();
        let mut hit = None;
        for art in ARTIFACTS {
            if t.len() >= art.len() {
                let start = t.len() - art.len();
                if t.is_char_boundary(start) && t[start..].eq_ignore_ascii_case(art) {
                    hit = Some(start);
                    break;
                }
            }
        }
        match hit {
            Some(i) => s = t[..i].trim_end().to_string(),
            None => break,
        }
    }
    s
}

/// Trim a whisper hallucination loop: if the same 1–4 word shingle repeats 3+
/// times consecutively, keep one copy and drop everything after it. Conservative
/// — returns the input unchanged if no clear loop is found (2 repeats can be
/// rhetorical: "you can do this, do that").
pub fn trim_hallucination_loop(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return String::new();
    }
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.len() < 6 {
        return s.to_string();
    }
    for window in 1..=4 {
        if window * 3 > words.len() {
            break;
        }
        for start in 0..=(words.len() - window * 3) {
            let first = &words[start..start + window];
            let mut runs = 1;
            let mut pos = start + window;
            while pos + window <= words.len() && &words[pos..pos + window] == first {
                runs += 1;
                pos += window;
            }
            if runs >= 3 {
                let kept = words[..start + window].join(" ");
                return kept
                    .trim_end_matches(|c: char| c == ',' || c == '.' || c == ' ')
                    .to_string();
            }
        }
    }
    s.to_string()
}

/// Heuristic gate: does `text` look like a real utterance rather than a
/// hallucination? Rejects too-short / too-long, high single-word repetition, and
/// known bad-phrase substrings. Lifted from voiceterm's `is_sane_transcription`.
pub fn looks_like_speech(text: &str) -> bool {
    let t = text.trim();
    let len = t.chars().count();
    if len < 2 || len > 120 {
        return false;
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    if words.len() >= 4 {
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for w in &words {
            *counts.entry(*w).or_insert(0) += 1;
        }
        if counts.values().copied().max().unwrap_or(0) * 2 > words.len() {
            return false; // a single word is >50% of the output — loop
        }
    }
    let lower = t.to_lowercase();
    !BAD_PHRASES.iter().any(|p| lower.contains(p))
}

/// Lowercase, trim, and strip trailing punctuation (keeping internal punctuation,
/// so "thank you, sir" doesn't collide with "thank you").
fn normalize(s: &str) -> String {
    let mut out: String = s.trim().chars().map(|c| c.to_ascii_lowercase()).collect();
    while matches!(
        out.chars().last(),
        Some('.' | '!' | '?' | ',' | ';' | ':' | '–' | '—' | '-')
    ) {
        out.pop();
    }
    out.trim().to_string()
}

/// Trailing artifact tokens whisper appends to otherwise-real transcripts.
const ARTIFACTS: &[&str] = &[
    "[BLANK_AUDIO]",
    "[BLANK]",
    "[_BEG_]",
    "[silence]",
    "(silence)",
    "(BLANK)",
    "[music]",
    "(music)",
];

/// Canonical short hallucinations (pre-normalized: lowercase, no trailing punct).
/// Merged from rusty-board's list and voiceterm's phrases.
const HALLUCINATIONS: &[&str] = &[
    "thank you",
    "thanks",
    "thanks for watching",
    "thank you for watching",
    "thanks for watching this video",
    "thank you so much",
    "thank you very much",
    "bye",
    "bye bye",
    "goodbye",
    "okay bye",
    "okay",
    "ok",
    "see you next time",
    "see you later",
    "see you soon",
    "see you in the next video",
    "see ya",
    "subscribe",
    "please subscribe",
    "like and subscribe",
    "don't forget to subscribe",
    "thanks for listening",
    "thank you for listening",
    "music",
    "[music]",
    "applause",
    "[applause]",
    "laughter",
    "[laughter]",
    "silence",
    "[silence]",
    "blank_audio",
    "[blank_audio]",
    "you",
    "yeah",
    "uh",
    "um",
    "hmm",
    "mm",
    "mhm",
];

/// Substrings that mark a hallucination anywhere in the text (voiceterm's set).
const BAD_PHRASES: &[&str] = &[
    "thank you for watching",
    "thanks for watching",
    "subscribe to",
    "like and subscribe",
    "see you next time",
    "see you in the next video",
    "don't forget to subscribe",
    "[music]",
    "[silence]",
    "[blank_audio]",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_canonical_silence_artifacts() {
        for s in ["Thank you.", "thanks for watching", "Bye!", " Bye ", "[BLANK_AUDIO]", "uh"] {
            assert!(is_hallucination(s), "should filter {s:?}");
        }
    }

    #[test]
    fn passes_real_speech() {
        for s in ["hello world", "thank you very much for the help", "see you at noon"] {
            assert!(!is_hallucination(s), "should pass {s:?}");
        }
    }

    #[test]
    fn empty_and_punctuation_filtered() {
        for s in ["", "   ", "...", "?!"] {
            assert!(is_hallucination(s));
        }
    }

    #[test]
    fn strips_only_trailing_artifacts() {
        assert_eq!(strip_trailing_artifacts("hello world [BLANK_AUDIO]"), "hello world");
        assert_eq!(strip_trailing_artifacts("I think [BLANK_AUDIO] [blank_audio]"), "I think");
        assert_eq!(
            strip_trailing_artifacts("hello [BLANK_AUDIO] world"),
            "hello [BLANK_AUDIO] world",
            "middle artifact preserved"
        );
    }

    #[test]
    fn trims_repeated_shingle_loops() {
        // single-word loop → one copy
        assert_eq!(trim_hallucination_loop("hello hello hello hello hello world"), "hello");
        // 3-word shingle repeated 3× → one copy
        assert_eq!(
            trim_hallucination_loop("open the door open the door open the door now"),
            "open the door"
        );
        // loop whose shingle carries a comma → comma stripped from the kept copy
        assert_eq!(trim_hallucination_loop("stop now, stop now, stop now, end here"), "stop now");
        // exact-fill: shingle repeating exactly 3x and filling the whole string —
        // the off-by-one case; only caught with an inclusive loop bound.
        assert_eq!(trim_hallucination_loop("go left go left go left"), "go left");
        // no clear loop → unchanged
        assert_eq!(
            trim_hallucination_loop("the quick brown fox jumps over"),
            "the quick brown fox jumps over"
        );
        // under the 6-word floor → unchanged (conservative; 2 repeats can be rhetorical)
        assert_eq!(trim_hallucination_loop("the the the the world"), "the the the the world");
    }

    #[test]
    fn speech_gate_rejects_loops_and_bad_phrases() {
        assert!(looks_like_speech("open the settings panel"));
        assert!(!looks_like_speech("a")); // too short
        assert!(!looks_like_speech("word word word word")); // >50% repetition
        assert!(!looks_like_speech("please subscribe to my channel")); // bad phrase
    }
}
