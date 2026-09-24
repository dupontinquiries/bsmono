//! Timed transcripts for section watching.
//!
//! `vidkit-relevance` already turns cues into chapters and answers "where did
//! they discuss X". Nothing produced cues. This crate is the producing side,
//! split so that everything with a decision in it runs without a model, a
//! process or a thread:
//!
//! - [`CueSource`] is the seam. Whisper is one implementation (feature
//!   `whisper`); a sidecar `.vtt`/`.srt` is another ([`subtitles`]), and it is
//!   the cheaper one whenever it exists — yt-dlp can fetch YouTube's captions
//!   for free, where whisper costs minutes of CPU on a phone.
//! - [`coverage::Coverage`] decides which chunk to transcribe next. Playhead
//!   first, because the chunk you are watching is the one you want subtitles
//!   and chapter context for; then forward; then back to fill the start.
//! - [`transcript::Transcript`] merges chunk results. Chunks are decoded with
//!   a post-roll so a sentence crossing a boundary is heard whole, and each
//!   cue is OWNED by the chunk its start falls in, so the overlap never yields
//!   the sentence twice.
//!
//! The shell (the app) owns the loop: it asks `Coverage` for a chunk, decodes
//! [`SAMPLE_RATE`] mono f32 for `chunk.decode_span()`, calls the source, and
//! hands the result to `Transcript::accept`. Every step of that loop is
//! visible to the UI and can be paused or redirected between chunks.

pub mod coverage;
pub mod subtitles;
pub mod transcript;
#[cfg(feature = "whisper")]
pub mod whisper;

use std::time::Duration;

pub use vidkit_relevance::Cue;

/// What whisper consumes: 16 kHz mono float. The decoder is asked for exactly
/// this, so no resampling happens in Rust.
pub const SAMPLE_RATE: u32 = 16_000;

/// Something that turns audio into timed text.
///
/// `offset` is the media time of `pcm[0]`. Returned cues are in MEDIA time,
/// not chunk-relative: the offset arithmetic lives in one place, the source,
/// rather than in every caller.
pub trait CueSource: Send {
    fn transcribe(&mut self, pcm: &[f32], offset: Duration) -> Result<Vec<Cue>, String>;
    /// For the UI and the cache key: `whisper:ggml-base.en.bin`, `subtitles:x.vtt`.
    fn name(&self) -> String;
    /// The whole transcript, if this source has it without listening -- a
    /// sidecar file does. The shell asks first and skips decoding entirely:
    /// an hour of audio decoded to be ignored is exactly the waste to avoid.
    fn complete(&mut self) -> Option<Vec<Cue>> {
        None
    }
}

/// Clean one segment of whisper output, or drop it.
///
/// Whisper fills silence with "Thank you." and `[BLANK_AUDIO]`, and in a
/// lecture those land in every pause. Left in, they become chapter keywords
/// ("thank" is distinctive in a window that is otherwise silent). The filters
/// are voice-kit's, shared with voiceterm and rusty-board, not a fourth copy.
pub fn clean_text(raw: &str) -> Option<String> {
    let t = voice_kit::hallucination::strip_trailing_artifacts(raw.trim());
    let t = voice_kit::hallucination::trim_hallucination_loop(&t);
    let t = t.trim();
    if t.is_empty() || voice_kit::hallucination::is_hallucination(t) {
        return None;
    }
    // Bracketed-only segments are sound annotations, not speech.
    if (t.starts_with('[') && t.ends_with(']')) || (t.starts_with('(') && t.ends_with(')')) {
        return None;
    }
    Some(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_fillers_and_annotations_are_dropped() {
        assert_eq!(clean_text("[BLANK_AUDIO]"), None);
        assert_eq!(clean_text("  Thank you.  "), None);
        assert_eq!(clean_text("(applause)"), None);
        assert_eq!(clean_text(""), None);
    }

    #[test]
    fn speech_survives_with_its_trailing_artifact_removed() {
        assert_eq!(
            clean_text(" The eigenvalues are real [BLANK_AUDIO]").as_deref(),
            Some("The eigenvalues are real")
        );
        // Long thanks is speech, not the canned filler.
        assert!(clean_text("thank you very much for the question about entropy").is_some());
    }
}
