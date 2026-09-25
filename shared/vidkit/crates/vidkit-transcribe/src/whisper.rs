//! Whisper, in process, through whisper-rs 0.16.
//!
//! In process rather than `whisper-cli` per chunk because the model load is
//! the expensive part: `base.en` is ~150 MB to map and initialise, and a
//! lecture is a hundred-odd chunks. One context, one state, reused.
//!
//! The model is found through models-kit (via voice-kit's resolver), so a
//! machine that has a model for voiceterm has one for vidkit.

use std::path::{Path, PathBuf};
use std::time::Duration;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

use crate::{clean_text, Cue, CueSource};

/// Above this, whisper itself thinks the segment is not speech. Its own
/// default threshold is 0.6; segments above it in a lecture are coughs,
/// music stings and the text it invents for silence.
pub const NO_SPEECH_MAX: f32 = 0.6;

/// Turn one whisper segment into a media-time cue, or drop it. Pure: the
/// units (centiseconds, chunk-relative) are the part that goes wrong.
pub fn segment_cue(offset: Duration, t0_cs: i64, t1_cs: i64, text: &str, no_speech: f32) -> Option<Cue> {
    if no_speech > NO_SPEECH_MAX {
        return None;
    }
    let text = clean_text(text)?;
    let cs = |v: i64| Duration::from_millis(v.max(0) as u64 * 10);
    let start = offset + cs(t0_cs);
    let end = (offset + cs(t1_cs)).max(start);
    Some(Cue { start, end, text })
}

pub struct WhisperSource {
    // The state holds an Arc of the context's internals, but keeping the
    // context too documents the ownership and keeps create_state available.
    _ctx: WhisperContext,
    state: WhisperState,
    model: PathBuf,
    threads: i32,
    language: Option<String>,
    multilingual: bool,
}


impl WhisperSource {
    /// Load `model`. `threads`: whisper scales well to the big cores and
    /// badly past them; on a phone, 4.
    pub fn load(model: &Path, threads: i32, language: Option<String>) -> Result<Self, String> {
        let ctx = WhisperContext::new_with_params(model, WhisperContextParameters::default())
            .map_err(|e| format!("{}: {e:?}", model.display()))?;
        let state = ctx.create_state().map_err(|e| format!("whisper state: {e:?}"))?;
        let multilingual = ctx.is_multilingual();
        Ok(Self { _ctx: ctx, state, model: model.to_path_buf(), threads: threads.max(1), language, multilingual })
    }

    /// Find a model in the shared store (`~/models/whisper`, or the legacy
    /// places voiceterm used) and load it. `VIDKIT_WHISPER_MODEL` names a
    /// file outright.
    pub fn from_store(threads: i32) -> Result<Self, String> {
        let env = std::env::var("VIDKIT_WHISPER_MODEL").ok();
        let path = voice_kit::model::find_whisper_model(env.as_deref()).ok_or_else(|| {
            "no whisper model: put a ggml model (e.g. ggml-base.en.bin) in ~/models/whisper".to_string()
        })?;
        Self::load(&path, threads, None)
    }
}

impl CueSource for WhisperSource {
    fn transcribe(&mut self, pcm: &[f32], offset: Duration) -> Result<Vec<Cue>, String> {
        // whisper.cpp segfaults on an empty buffer; whisper-rs checks, but the
        // error it returns reads like a model failure. Empty audio is "no cues".
        if pcm.is_empty() {
            return Ok(Vec::new());
        }
        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        p.set_n_threads(self.threads);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_timestamps(false);
        p.set_print_special(false);
        // Each chunk stands alone. With context carried over, a hallucinated
        // phrase in one chunk primes the next to repeat it -- the loop
        // voice-kit's filter exists for, fed by the previous chunk.
        p.set_no_context(true);
        p.set_suppress_blank(true);
        p.set_language(Some(crate::effective_language(self.language.as_deref(), self.multilingual)));
        self.state.full(p, pcm).map_err(|e| format!("whisper: {e:?}"))?;
        let n = self.state.full_n_segments();
        let mut out = Vec::new();
        for i in 0..n {
            let Some(seg) = self.state.get_segment(i) else { continue };
            let Ok(text) = seg.to_str_lossy() else { continue };
            if let Some(c) = segment_cue(offset, seg.start_timestamp(), seg.end_timestamp(), &text, seg.no_speech_probability()) {
                out.push(c);
            }
        }
        Ok(out)
    }

    fn name(&self) -> String {
        format!("whisper:{}", self.model.file_name().unwrap_or_default().to_string_lossy())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centiseconds_are_offset_into_media_time() {
        let c = segment_cue(Duration::from_secs(60), 150, 420, " The Laplace transform", 0.1).unwrap();
        assert_eq!(c.start, Duration::from_millis(61_500));
        assert_eq!(c.end, Duration::from_millis(64_200));
        assert_eq!(c.text, "The Laplace transform");
    }

    #[test]
    fn non_speech_and_fillers_are_dropped() {
        assert!(segment_cue(Duration::ZERO, 0, 100, "real words here", 0.9).is_none());
        assert!(segment_cue(Duration::ZERO, 0, 100, "[BLANK_AUDIO]", 0.0).is_none());
    }

    #[test]
    fn inverted_or_negative_stamps_are_clamped() {
        let c = segment_cue(Duration::from_secs(1), -5, -10, "x y z", 0.0).unwrap();
        assert_eq!((c.start, c.end), (Duration::from_secs(1), Duration::from_secs(1)));
    }}
