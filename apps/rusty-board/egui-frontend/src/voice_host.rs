//! voice_host — the capture + streaming-partial driver for the egui keyboard.
//!
//! What the egui build had: the dictation STATE MACHINE (`dictation.rs`), the mic-state UI,
//! and the engine mounted by `#[path]` from the GTK app (`voice::engine`: recorder, whisper
//! transcriber, service). What it did not have was anything connecting them — the toolbar's
//! "dictate" started a state machine that no audio ever reached ("No capture backend attached
//! in this build", in the frame loop). Voice typing worked on the GTK build and was a mock
//! here. This is the connection, and it is the GTK streaming worker
//! (`ui/window.rs::streaming_partial_worker`) re-shaped for a frame loop:
//!
//! - **no timer thread**: `poll` is called every frame while dictating; it spawns ONE
//!   inference at a time when the interval has elapsed and reads back the last result;
//! - **bounded window**: the GTK worker transcribed the whole ring every interval, which is
//!   what made partials fall behind on the FP5. Here the window is `dictation::partial_window`
//!   (12 s), converted to the mic's rate before the snapshot is sliced;
//! - **`Pass::Partial`** parameters (encoder sized to the window, no fallback re-decodes);
//! - **the corpus**: on release the last window's audio is kept, so a committed utterance can
//!   be `remember`ed against the text the user accepted.
//!
//! Everything whisper-shaped stays in the engine; this file owns no inference parameters.

use std::ops::Range;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use crate::voice::engine::recorder::{resample_to_16k, snapshot_ring};
use crate::voice::engine::service::VoiceService;
use crate::voice::engine::transcriber::{Pass, Timing};

/// Under this much audio a partial is noise, not speech (the GTK worker's threshold).
const MIN_AUDIO_MS: u32 = 400;

/// What one partial inference sends back: the text and the 16 kHz audio it was made from.
struct PartialResult {
    text: String,
    audio: Vec<f32>,
}

pub struct VoiceHost {
    service: VoiceService,
    inflight: Option<Receiver<anyhow::Result<PartialResult>>>,
    last_spawn: Instant,
    interval: Duration,
    /// The 16 kHz audio of the most recent completed partial — what `stop` hands back for
    /// the corpus. Never a partial's TEXT: only the caller knows what was committed.
    last_audio: Vec<f32>,
}

impl VoiceHost {
    /// Load the engine (this loads the whisper model; slow, once). `Err` when no model is
    /// found — the caller shows that in the mic status line and dictation stays a preview.
    pub fn new(cfg: &crate::config::Config) -> anyhow::Result<VoiceHost> {
        let service = VoiceService::new(cfg.voice.clone())?;
        let interval = Duration::from_millis(cfg.voice.streaming_interval_ms.max(200) as u64);
        Ok(VoiceHost { service, inflight: None, last_spawn: Instant::now(), interval, last_audio: Vec::new() })
    }

    pub fn start(&mut self) -> anyhow::Result<()> {
        self.inflight = None;
        self.last_audio.clear();
        self.last_spawn = Instant::now() - self.interval; // the first partial is due immediately
        self.service.start_recording()
    }

    /// Mic RMS-ish level for the preview meter.
    pub fn level(&self) -> f32 {
        self.service.current_level()
    }

    /// How much audio the ring holds, in 16 kHz samples — what `DictationService::captured`
    /// wants so the next window can be bounded.
    pub fn captured_samples(&self) -> usize {
        let (ring, rate) = self.service.audio_snapshot_handle();
        let n = ring.lock().len();
        if rate == 0 { 0 } else { (n as u64 * 16_000 / rate as u64) as usize }
    }

    /// Call every frame while dictating. Returns a finished partial when one has arrived.
    /// `window` is in 16 kHz samples (from `DictationService::next_window`).
    pub fn poll(&mut self, window: Range<usize>) -> Option<String> {
        // 1. a result from the inference in flight?
        let mut out = None;
        if let Some(rx) = &self.inflight {
            match rx.try_recv() {
                Ok(Ok(r)) => {
                    let text = crate::voice::engine::hallucination::strip_trailing_artifacts(&r.text);
                    self.last_audio = r.audio;
                    self.inflight = None;
                    if !text.is_empty() {
                        out = Some(text);
                    }
                }
                Ok(Err(e)) => {
                    tracing::debug!("partial inference error (ignored): {e}");
                    self.inflight = None;
                }
                Err(mpsc::TryRecvError::Empty) => return None, // still running: one at a time
                Err(mpsc::TryRecvError::Disconnected) => self.inflight = None,
            }
        }
        // 2. spawn the next one when due
        if self.last_spawn.elapsed() >= self.interval {
            self.spawn_partial(window);
        }
        out
    }

    fn spawn_partial(&mut self, window: Range<usize>) {
        let Some(transcriber) = self.service.transcriber() else { return };
        let (ring, rate) = self.service.audio_snapshot_handle();
        if rate == 0 {
            return;
        }
        let samples = snapshot_ring(&ring);
        // The window is in 16 kHz samples; the ring is at the mic's rate.
        let keep = (window.len() as u64 * rate as u64 / 16_000) as usize;
        let samples: Vec<f32> = if samples.len() > keep { samples[samples.len() - keep..].to_vec() } else { samples };
        let duration_ms = (samples.len() as u64 * 1000 / rate as u64) as u32;
        self.last_spawn = Instant::now();
        if duration_ms < MIN_AUDIO_MS {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("voice-partial".into()).spawn(move || {
            let audio = resample_to_16k(&samples, rate);
            let result = transcriber
                .transcribe_with(&audio, Pass::Partial)
                .map(|text| PartialResult { text, audio });
            let _ = tx.send(result);
        });
        match spawned {
            Ok(_) => self.inflight = Some(rx),
            Err(e) => tracing::warn!("voice-partial thread: {e}"),
        }
    }

    /// Stop capturing without a final pass (PartialsOnly: the last partial is the result).
    /// Returns the audio the last partial was made from, for `remember`.
    pub fn stop(&mut self) -> Vec<f32> {
        self.service.cancel_recording();
        self.inflight = None;
        std::mem::take(&mut self.last_audio)
    }

    /// The user accepted `text` for `audio`: put it in the corpus.
    pub fn remember(&self, audio: &[f32], text: &str) {
        if let Some(t) = self.service.transcriber() {
            t.remember(audio, text);
        }
    }

    pub fn last_timing(&self) -> Option<Timing> {
        self.service.transcriber().and_then(|t| t.last_timing())
    }

    pub fn corpus_len(&self) -> usize {
        self.service.transcriber().map(|t| t.corpus_len()).unwrap_or(0)
    }
}
