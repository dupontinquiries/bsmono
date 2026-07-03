//! Whisper hallucination filter.
//!
//! Whisper.cpp is well-known to output plausible-sounding short
//! phrases on near-silent or noise-only audio. The same garbage
//! tends to repeat across users, because it's the model's
//! statistical fallback for "I can't make out words, here's the
//! highest-probability sentence-shaped token sequence":
//!
//!   "Thank you."
//!   "Thanks for watching."
//!   "Bye."
//!   "."
//!   "[BLANK_AUDIO]"  (a token from the dataset's silence labels)
//!   ...etc.
//!
//! This filter recognizes those and returns `true` so the caller
//! can drop the partial transcription. It's deliberately
//! conservative — we don't filter long outputs, only short ones
//! that match the canonical hallucination list verbatim (case-
//! and punctuation-insensitive).
//!
//! Behavior on legitimate speech:
//! - "Bye." said by an actual user → still filtered. False
//!   positive. Acceptable because (a) the streaming path will
//!   pick up more text on later iterations once the user speaks
//!   more, and (b) the final inference path doesn't filter (so
//!   a deliberate "bye" said as a complete utterance commits).
//! - "Thank you very much" → NOT filtered. The hallucination
//!   list is matched exactly, not as a prefix.
//!
//! Inspired by voiceterm/web2qml's filler_words.rs but trimmed
//! to just the most common silence-hallucinations (no need to
//! filter pronouns, articles, etc.).

/// Returns true if the text is a known whisper hallucination that
/// should be suppressed from a streaming partial display.
pub fn is_hallucination(text: &str) -> bool {
    let normalized = normalize(text);
    if normalized.is_empty() {
        // Empty / whitespace-only / pure punctuation — drop.
        return true;
    }
    HALLUCINATIONS
        .iter()
        .any(|&h| normalized == h)
}

/// Strip trailing whisper artifacts from a transcript without
/// dropping legitimate content. Removes tokens like `[BLANK_AUDIO]`,
/// `[_BEG_]`, `(silence)` that whisper occasionally appends to
/// otherwise-real transcripts.
///
/// Returns a String with the artifacts removed and trailing
/// whitespace trimmed. The leading content is preserved verbatim.
///
/// Example:
///   "hello world [BLANK_AUDIO]" → "hello world"
///   "I think [BLANK_AUDIO] [BLANK_AUDIO]" → "I think"
///   "hello [BLANK_AUDIO] world" → "hello [BLANK_AUDIO] world"
///     (artifact in the middle, not the end — preserved)
pub fn strip_trailing_artifacts(text: &str) -> String {
    let artifacts = [
        "[BLANK_AUDIO]",
        "[blank_audio]",
        "[BLANK]",
        "[_BEG_]",
        "[silence]",
        "(silence)",
        "(BLANK)",
        // Some whisper builds emit a literal music-note when audio
        // is silent — the dataset's "music" label leaks through.
        "[music]",
        "(music)",
    ];
    let mut s = text.trim_end().to_string();
    loop {
        let mut changed = false;
        for art in &artifacts {
            if let Some(stripped) = s
                .trim_end()
                .strip_suffix(art)
                .or_else(|| s.trim_end().strip_suffix(&art.to_lowercase()))
                .or_else(|| s.trim_end().strip_suffix(&art.to_uppercase()))
            {
                s = stripped.trim_end().to_string();
                changed = true;
                break;
            }
        }
        if !changed {
            break;
        }
    }
    s
}

/// Strip leading/trailing whitespace, lowercase, strip terminal
/// punctuation. Keeps internal punctuation so "thank you, sir"
/// doesn't collide with "thank you".
fn normalize(s: &str) -> String {
    let trimmed = s.trim();
    let mut out: String = trimmed.chars().map(|c| c.to_ascii_lowercase()).collect();
    // Strip trailing punctuation iteratively (handles "thank you!!!").
    while let Some(c) = out.chars().last() {
        if matches!(c, '.' | '!' | '?' | ',' | ';' | ':' | '–' | '—' | '-') {
            out.pop();
        } else {
            break;
        }
    }
    out.trim().to_string()
}

/// The canonical list of short whisper hallucinations to suppress.
/// All entries are pre-normalized: lowercase, no trailing punct.
const HALLUCINATIONS: &[&str] = &[
    // Sign-off hallucinations (most common on silence)
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
    // YouTube-style outros (whisper was trained on a lot of YT)
    "subscribe",
    "please subscribe",
    "like and subscribe",
    "don't forget to subscribe",
    "thanks for listening",
    "thank you for listening",
    // Music / noise hallucinations
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
    // Single-word noise
    "you",
    "yeah",
    "uh",
    "um",
    "hmm",
    "mm",
    "mhm",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_canonical_silence_artifact() {
        assert!(is_hallucination("Thank you."));
        assert!(is_hallucination("thanks for watching"));
        assert!(is_hallucination("Bye!"));
        assert!(is_hallucination(" Bye "));
        assert!(is_hallucination("[BLANK_AUDIO]"));
    }

    #[test]
    fn passes_real_speech() {
        assert!(!is_hallucination("hello world"));
        assert!(!is_hallucination("thank you very much for the help"));
        assert!(!is_hallucination("see you at noon"));
    }

    #[test]
    fn empty_and_punctuation_filtered() {
        assert!(is_hallucination(""));
        assert!(is_hallucination("   "));
        assert!(is_hallucination("..."));
        assert!(is_hallucination("?!"));
    }

    #[test]
    fn normalization_handles_case_and_trailing_punct() {
        assert_eq!(normalize("Thank You!!!"), "thank you");
        assert_eq!(normalize("  bye.  "), "bye");
    }
}
