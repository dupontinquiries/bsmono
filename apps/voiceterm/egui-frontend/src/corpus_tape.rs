//! corpus_tape — the recording policy from `corpus_recorder.rs`.
//!
//! A bounded start→stop tape taken during reading-mode, saved beside the
//! reference paragraph. It is what fills the corpus that
//! [`crate::acoustic_cache`] later learns from, which sets the standard: a bad
//! recording here becomes a bad fingerprint there, and a bad fingerprint
//! promotes wrong text.
//!
//! ## Distinct from the continuous pipeline, on purpose
//!
//! The original is explicit that this is *"distinct from the continuous voice
//! pipeline in voice.rs"*. That separation is why the rules can be strict — a
//! tape has a beginning, an end, and a paragraph it is supposed to match, none
//! of which a live stream has.
//!
//! ## The cap stops accumulating but does NOT stop the recording
//!
//! The comment is precise: *"Hit the cap. Stop accumulating; the stop_recording
//! call will write what we have. Don't mark inactive here — the caller will
//! explicitly stop."*
//!
//! That is the right shape and easy to get wrong. Flipping to inactive inside
//! the audio callback would end a session the UI still believes is running, so
//! the stop button would do nothing and the user would be left holding a
//! recording that already finished without saying so.
//!
//! ## The sample rate is learned, not assumed
//!
//! Taken from the first non-empty batch, because the device decides it. A
//! hardcoded 48k would write a WAV header that disagrees with its own samples —
//! playable, wrong pitch, and silently wrong in the corpus.

/// Sixty seconds at 48 kHz. The original's reasoning: *"long enough for a
/// 50-word paragraph at 250 wpm (~12s) with lots of headroom"*.
pub const MAX_SAMPLES: usize = 48_000 * 60;

/// Bytes per sample on disk — i16 PCM.
pub const BYTES_PER_SAMPLE: usize = 2;

/// What a push of audio did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Push {
    /// Samples appended.
    Took,
    /// At the cap. The tape keeps its data and stays ACTIVE — see the docs.
    Full,
    /// Not recording; the callback costs nothing.
    Inactive,
}

/// The tape's state. Deliberately small: the audio callback touches it.
#[derive(Clone, Debug, Default)]
pub struct Tape {
    active: bool,
    samples: Vec<f32>,
    /// Learned from the first non-empty batch; zero until then.
    sample_rate: u32,
    reference: String,
}

impl Tape {
    pub fn is_active(&self) -> bool { self.active }
    pub fn len(&self) -> usize { self.samples.len() }
    pub fn is_empty(&self) -> bool { self.samples.is_empty() }
    pub fn sample_rate(&self) -> u32 { self.sample_rate }
    pub fn reference(&self) -> &str { &self.reference }
    pub fn samples(&self) -> &[f32] { &self.samples }

    /// Begin a tape. Clears whatever the last one left.
    ///
    /// Clearing on START rather than on stop is what makes an interrupted
    /// session harmless: a crash between stop and write leaves data that the
    /// next start discards, rather than data the next tape appends to.
    pub fn start(&mut self, reference: &str) {
        self.active = true;
        self.samples.clear();
        self.sample_rate = 0;
        self.reference = reference.to_string();
    }

    /// Push a batch from the audio callback.
    pub fn push(&mut self, samples: &[f32], rate: u32) -> Push {
        if !self.active { return Push::Inactive; }
        if self.sample_rate == 0 && rate > 0 && !samples.is_empty() {
            self.sample_rate = rate;
        }
        if self.samples.len() + samples.len() > MAX_SAMPLES {
            // Stop accumulating, stay ACTIVE. Marking inactive here would end
            // a session the UI still believes is running.
            return Push::Full;
        }
        self.samples.extend_from_slice(samples);
        Push::Took
    }

    /// End the tape and hand back what should be written, if anything.
    pub fn stop(&mut self) -> Option<Recording> {
        self.active = false;
        if self.samples.is_empty() || self.sample_rate == 0 { return None; }
        Some(Recording {
            samples: std::mem::take(&mut self.samples),
            sample_rate: self.sample_rate,
            reference: std::mem::take(&mut self.reference),
        })
    }

    pub fn duration_s(&self) -> f32 {
        if self.sample_rate == 0 { return 0.0; }
        self.samples.len() as f32 / self.sample_rate as f32
    }
}

/// A finished tape, ready for the writer thread.
#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub reference: String,
}

impl Recording {
    pub fn duration_s(&self) -> f32 {
        self.samples.len() as f32 / self.sample_rate.max(1) as f32
    }

    /// Total WAV size: 44-byte canonical header plus i16 data.
    pub fn wav_bytes(&self) -> usize { 44 + self.samples.len() * BYTES_PER_SAMPLE }
}

/// Convert a float sample to i16 PCM.
///
/// Clamped BEFORE scaling. A sample above 1.0 — which a hot mic produces
/// routinely — would otherwise wrap through the cast and turn a loud passage
/// into a burst of opposite-sign noise: audible as a click, and poison for a
/// fingerprint taken from it.
pub fn to_i16(s: f32) -> i16 {
    if s.is_nan() { return 0; }
    (s.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// A filename component from a paragraph reference or speaker id.
///
/// The corpus layout is `corpus/<speaker_id>/<unix_ms>.{wav,txt}`, so a
/// speaker id reaches the filesystem directly. Anything that could traverse or
/// break a path is replaced rather than escaped — an id is a label, and a label
/// that needs escaping is a label that will be un-escaped wrong somewhere.
pub fn sanitize_id(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() { "unknown".to_string() } else { trimmed.to_string() }
}

/// Is this tape worth keeping as a corpus entry?
///
/// Two rejections, both cheap and both saving a bad fingerprint later: a tape
/// with no reference text has nothing to be transcribed AGAINST, and a tape
/// under a fifth of a second is a button press rather than speech.
pub const MIN_KEEP_S: f32 = 0.2;

pub fn worth_keeping(r: &Recording) -> bool {
    !r.reference.trim().is_empty() && r.duration_s() >= MIN_KEEP_S
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(n: usize) -> Vec<f32> { vec![0.1; n] }

    #[test]
    fn an_inactive_tape_costs_nothing() {
        // The audio callback runs whether or not anyone is recording; it must
        // not allocate or copy when nobody is.
        let mut t = Tape::default();
        assert_eq!(t.push(&batch(1024), 48_000), Push::Inactive);
        assert!(t.is_empty());
    }

    #[test]
    fn the_sample_rate_is_learned_from_the_device() {
        // Hardcoding 48k would write a header that disagrees with its own
        // samples: playable, wrong pitch, silently wrong in the corpus.
        let mut t = Tape::default();
        t.start("the quick brown fox");
        assert_eq!(t.sample_rate(), 0, "unknown until audio arrives");
        t.push(&batch(64), 44_100);
        assert_eq!(t.sample_rate(), 44_100);
        t.push(&batch(64), 48_000);
        assert_eq!(t.sample_rate(), 44_100, "the first rate wins; a device does not change mid-tape");
    }

    #[test]
    fn an_empty_batch_does_not_set_the_rate() {
        // A callback can fire with nothing; taking the rate from it would
        // record a rate for a device that has not spoken yet.
        let mut t = Tape::default();
        t.start("x");
        t.push(&[], 44_100);
        assert_eq!(t.sample_rate(), 0);
    }

    #[test]
    fn the_cap_stops_accumulating_but_the_tape_stays_active() {
        // "Don't mark inactive here — the caller will explicitly stop."
        // Flipping active inside the audio callback would end a session the UI
        // still believes is running: the stop button does nothing, and the user
        // is holding a recording that already finished without saying so.
        let mut t = Tape::default();
        t.start("long paragraph");
        t.push(&batch(MAX_SAMPLES), 48_000);
        assert_eq!(t.push(&batch(1024), 48_000), Push::Full);
        assert!(t.is_active(), "the cap ended the session");
        assert_eq!(t.len(), MAX_SAMPLES, "and kept what it had");
    }

    #[test]
    fn what_was_captured_before_the_cap_is_still_written() {
        let mut t = Tape::default();
        t.start("p");
        t.push(&batch(MAX_SAMPLES), 48_000);
        t.push(&batch(4096), 48_000);
        let r = t.stop().expect("a full tape is still a tape");
        assert_eq!(r.samples.len(), MAX_SAMPLES);
    }

    #[test]
    fn the_cap_is_a_minute_of_headroom_over_a_read_paragraph() {
        // "Long enough for a 50-word paragraph at 250 wpm (~12s) with lots of
        // headroom."
        let secs = MAX_SAMPLES as f32 / 48_000.0;
        assert!((secs - 60.0).abs() < 0.01);
        assert!(secs > 12.0 * 4.0, "not much headroom over a read paragraph");
    }

    #[test]
    fn starting_clears_what_the_last_tape_left() {
        // Clearing on START rather than stop is what makes an interrupted
        // session harmless: a crash between stop and write leaves data the next
        // start discards, rather than data the next tape appends to.
        let mut t = Tape::default();
        t.start("first");
        t.push(&batch(1000), 48_000);
        t.start("second");
        assert!(t.is_empty());
        assert_eq!(t.reference(), "second");
        assert_eq!(t.sample_rate(), 0, "and the rate is re-learned");
    }

    #[test]
    fn stopping_an_empty_tape_writes_nothing() {
        // A start-stop with no audio is a mis-click, not a corpus entry.
        let mut t = Tape::default();
        t.start("p");
        assert_eq!(t.stop(), None);
        assert!(!t.is_active());
    }

    #[test]
    fn stopping_without_a_known_rate_writes_nothing() {
        // Samples with no rate cannot be given a correct header, and a wrong
        // header is worse than a missing file.
        let mut t = Tape::default();
        t.start("p");
        t.push(&batch(500), 0);
        assert_eq!(t.stop(), None);
    }

    #[test]
    fn a_hot_mic_clips_rather_than_wrapping() {
        // A sample above 1.0 is routine on a hot mic; an unclamped cast wraps
        // and turns a loud passage into opposite-sign noise — a click, and
        // poison for any fingerprint taken from it.
        assert_eq!(to_i16(2.0), 32767);
        assert_eq!(to_i16(-2.0), -32767);
        assert_eq!(to_i16(0.0), 0);
        assert_eq!(to_i16(f32::NAN), 0, "a NaN sample is silence, not a wrap");
        assert!(to_i16(1.0) > 32000 && to_i16(-1.0) < -32000);
    }

    #[test]
    fn a_speaker_id_cannot_escape_the_corpus_directory() {
        // The layout is corpus/<speaker_id>/..., so the id reaches the
        // filesystem directly.
        assert_eq!(sanitize_id("../../etc/passwd"), "etc_passwd");
        assert_eq!(sanitize_id("kd"), "kd");
        assert_eq!(sanitize_id("a b/c"), "a_b_c");
        assert_eq!(sanitize_id(""), "unknown");
        assert_eq!(sanitize_id("///"), "unknown", "an id of only separators is not a directory");
    }

    #[test]
    fn a_tape_with_no_reference_text_is_not_kept() {
        // There is nothing to transcribe it AGAINST, so it can never become a
        // trustworthy fingerprint.
        let r = Recording { samples: vec![0.1; 48_000], sample_rate: 48_000, reference: "  ".into() };
        assert!(!worth_keeping(&r));
    }

    #[test]
    fn a_button_press_is_not_speech() {
        let r = Recording { samples: vec![0.1; 100], sample_rate: 48_000, reference: "hello".into() };
        assert!(r.duration_s() < MIN_KEEP_S);
        assert!(!worth_keeping(&r));
    }

    #[test]
    fn a_real_tape_is_kept_and_sized_correctly() {
        let r = Recording { samples: vec![0.1; 48_000], sample_rate: 48_000, reference: "hello".into() };
        assert!(worth_keeping(&r));
        assert!((r.duration_s() - 1.0).abs() < 1e-6);
        assert_eq!(r.wav_bytes(), 44 + 48_000 * 2);
    }
}
