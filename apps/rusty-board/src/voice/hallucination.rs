//! Whisper hallucination filter — a thin shim over `voice-kit` (TODO.md's
//! "voice-kit consumers" item).
//!
//! This used to be a full standalone implementation. Confirmed before
//! repointing: this file's `HALLUCINATIONS` list, `is_hallucination`, and
//! `strip_trailing_artifacts` were behaviorally equivalent to voice-kit's
//! copies — voice-kit's own module doc says it's exactly "the union" of
//! this file and voiceterm's equivalent, so this restores the single source
//! of truth that union was supposed to produce. One real (and safe)
//! difference: voice-kit's artifact-strip is fully case-insensitive
//! (`eq_ignore_ascii_case`) where this file's only matched exact/all-lower/
//! all-upper case variants — a strict improvement, since an artifact token
//! is extraneous debris by definition and matching more of it can't drop
//! real speech.
//!
//! Callers go through `crate::voice::hallucination::` (see `mod.rs`), so
//! keeping the same function names/signatures here means nothing outside
//! this file needs to change.

/// Returns true if the text is a known whisper hallucination that should be
/// suppressed from a streaming partial display. See voice-kit's own doc for
/// the canonical list and normalization rules.
pub fn is_hallucination(text: &str) -> bool {
    voice_kit::hallucination::is_hallucination(text)
}

/// Strip trailing whisper artifacts (`[BLANK_AUDIO]`, `(silence)`, `[music]`,
/// …) off the end of a transcript, preserving leading content verbatim.
/// Artifacts in the middle are left in place.
pub fn strip_trailing_artifacts(text: &str) -> String {
    voice_kit::hallucination::strip_trailing_artifacts(text)
}

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
    fn strips_trailing_artifacts_but_preserves_middle_ones() {
        assert_eq!(strip_trailing_artifacts("hello world [BLANK_AUDIO]"), "hello world");
        assert_eq!(strip_trailing_artifacts("I think [BLANK_AUDIO] [BLANK_AUDIO]"), "I think");
        assert_eq!(
            strip_trailing_artifacts("hello [BLANK_AUDIO] world"),
            "hello [BLANK_AUDIO] world",
            "artifact in the middle, not the end — preserved"
        );
    }
}
