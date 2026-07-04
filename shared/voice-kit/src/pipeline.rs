//! pipeline — the voice-input pipeline shared by every voice-mode surface: keyboard dictation
//! (rusty-board), voiceterm's voice pane, logscope voice commands.
//!
//! The app owns the mic (a `mic_kit::FallbackChain`) and pushes the audio it reads into a
//! [`VoicePipeline`]; on utterance end it `flush`es, which transcribes the accumulated audio via
//! a [`Transcriber`] (whisper on-device) and runs the result through the voice filter at the
//! level the user chose in settings. Keeping transcription behind a trait means this whole
//! pipeline is testable without a model, and the filter flags come straight from settings-kit's
//! `VoiceFilter` (no coupling — the app passes booleans).

use crate::hallucination::filter_transcript;

/// On-device speech-to-text. The real impl wraps whisper; tests use a stub.
pub trait Transcriber {
    /// Transcribe a buffer of mono samples into text (`None` if nothing was recognised).
    fn transcribe(&mut self, audio: &[f32]) -> Option<String>;
}

/// A fast, learned recogniser: returns candidate texts with acoustic scores (0..1), or empty if
/// it has nothing confident. On-device this is voiceterm's fastpath (learned command index +
/// acoustic match) — cheap, and after learning your vocabulary, often better than cold whisper.
pub trait Recognizer {
    fn recognize(&mut self, audio: &[f32]) -> Vec<(String, f32)>;
}

/// Fastpath-first transcription. Runs the fast [`Recognizer`], biases its candidates with the
/// learned [`ContextModel`](crate::context::ContextModel), and accepts a confident learned match
/// **without running whisper**; only on a miss does it fall back to the slow [`Transcriber`].
/// `learn` folds confirmed results back into the context so the fastpath keeps improving. This
/// is why the pipeline must not be whisper-only.
pub struct FastpathTranscriber<R: Recognizer, S: Transcriber> {
    pub fast: R,
    pub slow: S,
    pub context: crate::context::ContextModel,
    /// Minimum blended (acoustic + learned-prior) score to accept the fastpath and skip whisper.
    pub accept: f32,
}

impl<R: Recognizer, S: Transcriber> FastpathTranscriber<R, S> {
    pub fn new(fast: R, slow: S) -> Self {
        FastpathTranscriber { fast, slow, context: crate::context::ContextModel::new(), accept: 0.6 }
    }
    /// Learn a confirmed utterance (call when the user accepts the result).
    pub fn learn(&mut self, confirmed: &str) {
        self.context.record(confirmed);
    }
}

impl<R: Recognizer, S: Transcriber> Transcriber for FastpathTranscriber<R, S> {
    fn transcribe(&mut self, audio: &[f32]) -> Option<String> {
        let cands = self.fast.recognize(audio);
        if !cands.is_empty() {
            let ranked = self.context.rank(cands);
            if let Some((text, score)) = ranked.first() {
                if *score >= self.accept {
                    return Some(text.clone()); // fastpath win — no whisper
                }
            }
        }
        self.slow.transcribe(audio) // fall back to whisper
    }
}

/// Accumulates mic audio for one utterance, then transcribes + filters it.
pub struct VoicePipeline {
    buf: Vec<f32>,
    min_samples: usize,
    strip: bool,
    drop_hallucinations: bool,
    require_speech: bool,
}

impl VoicePipeline {
    /// `min_samples` = how much audio makes a flush worthwhile (below it, `ready` is false).
    pub fn new(min_samples: usize) -> VoicePipeline {
        VoicePipeline { buf: Vec::new(), min_samples, strip: true, drop_hallucinations: true, require_speech: false }
    }

    /// Set the filter strength from a settings `VoiceFilter` level's booleans
    /// (`strips_artifacts` / `filters_hallucinations` / `requires_speech_like`).
    pub fn set_filter(&mut self, strip: bool, drop_hallucinations: bool, require_speech: bool) {
        self.strip = strip;
        self.drop_hallucinations = drop_hallucinations;
        self.require_speech = require_speech;
    }

    /// Accumulate a chunk of captured audio (typically a `FallbackChain::read` frame).
    pub fn push(&mut self, audio: &[f32]) {
        self.buf.extend_from_slice(audio);
    }

    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    /// Enough audio buffered to be worth transcribing?
    pub fn ready(&self) -> bool {
        self.buf.len() >= self.min_samples
    }

    /// Discard accumulated audio without transcribing (e.g. cancelled utterance).
    pub fn cancel(&mut self) {
        self.buf.clear();
    }

    /// Transcribe the accumulated audio and filter it; clears the buffer. `None` if there was
    /// nothing to transcribe or the result was filtered out entirely.
    pub fn flush(&mut self, transcriber: &mut dyn Transcriber) -> Option<String> {
        if self.buf.is_empty() {
            return None;
        }
        let raw = transcriber.transcribe(&self.buf);
        self.buf.clear();
        let raw = raw?;
        filter_transcript(&raw, self.strip, self.drop_hallucinations, self.require_speech)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Stub(&'static str);
    impl Transcriber for Stub {
        fn transcribe(&mut self, _audio: &[f32]) -> Option<String> {
            Some(self.0.to_string())
        }
    }
    struct Deaf;
    impl Transcriber for Deaf {
        fn transcribe(&mut self, _audio: &[f32]) -> Option<String> {
            None
        }
    }

    #[test]
    fn accumulates_and_reports_ready() {
        let mut p = VoicePipeline::new(4);
        assert!(!p.ready());
        p.push(&[0.1, 0.1]);
        assert!(!p.ready());
        p.push(&[0.1, 0.1]);
        assert!(p.ready());
        assert_eq!(p.buffered(), 4);
    }

    #[test]
    fn flush_transcribes_and_filters() {
        let mut p = VoicePipeline::new(1);
        p.set_filter(true, true, false);
        p.push(&[0.2; 8]);
        // stub returns text with trailing whitespace → filter trims it
        let out = p.flush(&mut Stub("open settings  "));
        assert_eq!(out.as_deref(), Some("open settings"));
        // buffer cleared after flush
        assert_eq!(p.buffered(), 0);
    }

    #[test]
    fn empty_or_deaf_flush_is_none() {
        let mut p = VoicePipeline::new(1);
        assert!(p.flush(&mut Stub("x")).is_none(), "no audio → none");
        p.push(&[0.1; 4]);
        assert!(p.flush(&mut Deaf).is_none(), "transcriber returned nothing → none");
    }

    #[test]
    fn cancel_discards_without_transcribing() {
        let mut p = VoicePipeline::new(1);
        p.push(&[0.5; 10]);
        p.cancel();
        assert_eq!(p.buffered(), 0);
        assert!(p.flush(&mut Stub("late")).is_none());
    }

    struct FastStub(Vec<(String, f32)>);
    impl Recognizer for FastStub {
        fn recognize(&mut self, _audio: &[f32]) -> Vec<(String, f32)> {
            self.0.clone()
        }
    }
    struct Whisper(std::rc::Rc<std::cell::Cell<u32>>);
    impl Transcriber for Whisper {
        fn transcribe(&mut self, _audio: &[f32]) -> Option<String> {
            self.0.set(self.0.get() + 1); // count how often whisper actually ran
            Some("whisper result".into())
        }
    }

    #[test]
    fn fastpath_wins_after_learning_and_skips_whisper() {
        let ran = std::rc::Rc::new(std::cell::Cell::new(0u32));
        let mut ft = FastpathTranscriber::new(
            FastStub(vec![("git status".into(), 0.7)]),
            Whisper(ran.clone()),
        );
        // learn the phrase so the prior lifts it over `accept`
        for _ in 0..4 {
            ft.learn("git status");
        }
        assert_eq!(ft.transcribe(&[0.1; 8]).as_deref(), Some("git status"));
        assert_eq!(ran.get(), 0, "whisper was skipped on the fastpath");
    }

    #[test]
    fn falls_back_to_whisper_on_weak_or_empty_fastpath() {
        let ran = std::rc::Rc::new(std::cell::Cell::new(0u32));
        // fast recogniser has nothing → whisper runs
        let mut ft = FastpathTranscriber::new(FastStub(vec![]), Whisper(ran.clone()));
        assert_eq!(ft.transcribe(&[0.1; 8]).as_deref(), Some("whisper result"));
        assert_eq!(ran.get(), 1);
        // fast candidate but unlearned + low score → still whisper
        let mut ft2 = FastpathTranscriber::new(FastStub(vec![("unknown cmd".into(), 0.2)]), Whisper(ran.clone()));
        assert_eq!(ft2.transcribe(&[0.1; 8]).as_deref(), Some("whisper result"));
        assert_eq!(ran.get(), 2);
    }
}
