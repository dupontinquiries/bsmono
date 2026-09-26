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

/// Share of 20 ms windows with speech-level energy (above -45 dBFS RMS).
/// Pure.
///
/// A lecture's pauses, a video's silent stretches, music beds at the noise
/// floor: whisper runs its full 30 s encoder on them anyway and then invents
/// "Thank you." for the silence. Gating first costs one pass over the
/// samples (microseconds) against seconds of encoder.
pub fn speech_fraction(pcm: &[f32]) -> f32 {
    const WIN: usize = (SAMPLE_RATE as usize) / 50; // 20 ms
    const FLOOR: f32 = 0.005_6; // -45 dBFS
    let wins = pcm.len() / WIN;
    if wins == 0 {
        return 0.0;
    }
    let loud = pcm
        .chunks_exact(WIN)
        .filter(|w| (w.iter().map(|s| s * s).sum::<f32>() / WIN as f32).sqrt() > FLOOR)
        .count();
    loud as f32 / wins as f32
}

/// Below this share of speech-level windows a chunk is not worth whisper.
pub const MIN_SPEECH_FRACTION: f32 = 0.03;

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

    #[test]
    fn silence_and_noise_floor_are_not_speech() {
        let rate = SAMPLE_RATE as usize;
        assert_eq!(speech_fraction(&vec![0.0; rate * 30]), 0.0);
        let hiss: Vec<f32> = (0..rate * 30).map(|i| ((i * 7919 % 101) as f32 / 101.0 - 0.5) * 0.004).collect();
        assert!(speech_fraction(&hiss) < MIN_SPEECH_FRACTION, "hiss at ~-55 dBFS");
        // One second of talk-level tone in 30 s of silence: 3.3%, kept.
        let mut one_sec = vec![0.0f32; rate * 30];
        for (i, s) in one_sec.iter_mut().take(rate).enumerate() {
            *s = (i as f32 * 0.05).sin() * 0.1;
        }
        assert!(speech_fraction(&one_sec) >= MIN_SPEECH_FRACTION);
        assert_eq!(speech_fraction(&[]), 0.0);
    }
}

/// The language to give whisper: the caller's, else "auto" for a multilingual
/// model and "en" for an English-only one (`*.en` models). Detection on an
/// English-only model is meaningless -- its language probabilities are flat:
/// on a Smite video (Fedora Asahi, ggml-base.en-q5_1) every chunk logged
/// "auto-detected language: nl (p = 0.010000)", 1/99, i.e. chance -- and it is
/// still work done per chunk.
pub fn effective_language(requested: Option<&str>, multilingual: bool) -> &str {
    match requested {
        Some(l) if !l.is_empty() => l,
        _ if multilingual => "auto",
        _ => "en",
    }
}

#[cfg(test)]
mod language_tests {
    use super::*;

    #[test]
    fn an_english_only_model_is_not_asked_to_detect_a_language() {
        assert_eq!(effective_language(None, false), "en");
        assert_eq!(effective_language(None, true), "auto");
        assert_eq!(effective_language(Some(""), false), "en");
        assert_eq!(effective_language(Some("de"), true), "de");
        assert_eq!(effective_language(Some("de"), false), "de", "an explicit choice wins");
    }
}
