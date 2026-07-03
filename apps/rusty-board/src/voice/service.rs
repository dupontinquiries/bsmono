//! `VoiceService` — owns a recorder + transcriber, exposes start/stop/state.

use crate::config::VoiceConfig;
use crate::voice::recorder::{resample_to_16k, AudioRecorder};
use crate::voice::transcriber::{find_default_model, Transcriber};
use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceServiceState {
    Idle,
    Recording,
    Transcribing,
}

/// Auto-shutdown timeout: after this many seconds with no voice
/// activity (state stays Idle), release the transcriber + recorder
/// resources to free RAM and stop any audio-backend wakeups.
/// 5 minutes per user request. Settable via env for testing.
pub const VOICE_AUTO_SHUTDOWN_SECS: u64 = 300;

pub struct VoiceService {
    cfg: VoiceConfig,
    /// Whisper transcriber. Held in Option so we can release the
    /// model from memory on auto-shutdown without invalidating any
    /// Arc<VoiceService> handles held elsewhere. Re-loaded lazily
    /// on the next start_recording call.
    transcriber: Arc<Mutex<Option<Arc<Transcriber>>>>,
    /// Path to the whisper model — kept so we can re-load after
    /// release.
    model_path: PathBuf,
    /// Audio recorder. Always-allocated; cheap to keep around
    /// (the cpal::Stream is None until start() is called).
    recorder: Arc<Mutex<AudioRecorder>>,
    state: Arc<Mutex<VoiceServiceState>>,
    /// Last time the service did real work (started recording,
    /// finished a transcribe, or was explicitly poked). Used by
    /// the auto-shutdown timer to decide when to release the
    /// transcriber. Stored as epoch seconds in an atomic so the
    /// shutdown checker can read it without locking.
    last_activity: Arc<AtomicU64>,
}

impl VoiceService {
    pub fn new(cfg: VoiceConfig) -> Result<Self> {
        let model_path = cfg
            .model_path
            .clone()
            .or_else(find_default_model)
            .ok_or_else(|| {
                anyhow!(
                    "No Whisper model configured and none found in standard \
                     locations. Run `rusty-board download-model` or set \
                     `voice.model_path` in config.toml."
                )
            })?;
        Self::new_with_model(cfg, model_path)
    }

    pub fn new_with_model(cfg: VoiceConfig, model_path: PathBuf) -> Result<Self> {
        // Eagerly load the transcriber on construction so the first
        // voice tap doesn't pay the 1-2s model-load cost. The auto-
        // shutdown timer will drop it later if idle.
        let transcriber = Transcriber::new(&model_path, &cfg.language, cfg.threads)?;
        let recorder = AudioRecorder::new(30); // 30 seconds of buffer

        Ok(Self {
            cfg,
            transcriber: Arc::new(Mutex::new(Some(Arc::new(transcriber)))),
            model_path,
            recorder: Arc::new(Mutex::new(recorder)),
            state: Arc::new(Mutex::new(VoiceServiceState::Idle)),
            last_activity: Arc::new(AtomicU64::new(now_secs())),
        })
    }

    pub fn state(&self) -> VoiceServiceState {
        *self.state.lock()
    }

    /// Time in epoch seconds when the service was last active.
    /// Used by the auto-shutdown timer.
    pub fn last_activity_secs(&self) -> u64 {
        self.last_activity.load(Ordering::Relaxed)
    }

    /// Seconds elapsed since the last activity.
    pub fn idle_seconds(&self) -> u64 {
        now_secs().saturating_sub(self.last_activity_secs())
    }

    /// True if the transcriber is currently loaded in RAM.
    pub fn is_transcriber_loaded(&self) -> bool {
        self.transcriber.lock().is_some()
    }

    /// Mark the service as just-used. Called from start_recording,
    /// transcribe completion, and any other path that should reset
    /// the auto-shutdown clock.
    fn poke_activity(&self) {
        self.last_activity.store(now_secs(), Ordering::Relaxed);
    }

    /// Release the transcriber from RAM. Safe to call any time;
    /// becomes a no-op if state isn't Idle (we won't drop the
    /// model out from under an in-progress transcription).
    ///
    /// After release, the next start_recording call will re-load
    /// from disk — costing a 1-2 second model-load delay but saving
    /// the 50-200MB of RAM the model uses while idle.
    pub fn release_resources(&self) -> bool {
        let state = self.state.lock();
        if *state != VoiceServiceState::Idle {
            return false;
        }
        let mut t = self.transcriber.lock();
        if t.is_some() {
            *t = None;
            info!("voice: transcriber released (idle timeout)");
            true
        } else {
            false
        }
    }

    /// Preemptively load the transcriber WITHOUT starting a
    /// recording. Called by the readiness-FSM coordinator when it
    /// predicts voice will be used soon (Warm state), so the first
    /// Voice tap doesn't pay the 1-2s cold-load cost. Idempotent —
    /// no-op if already loaded. Does NOT poke the activity clock
    /// (we don't want pre-warming to keep the model alive forever;
    /// the auto-shutdown timer should still release it if the
    /// predicted use never materializes).
    pub fn prewarm_transcriber(&self) -> bool {
        if self.is_transcriber_loaded() {
            return false;
        }
        match self.ensure_transcriber_loaded() {
            Ok(_) => {
                info!("voice: transcriber pre-warmed (readiness FSM)");
                true
            }
            Err(e) => {
                tracing::warn!("voice: pre-warm failed: {}", e);
                false
            }
        }
    }

    /// Ensure the transcriber is loaded. Reloads from `model_path`
    /// if it was released. Called by start_recording so users who
    /// returned after the timeout don't see anything obviously
    /// broken — just a brief delay on the first tap.
    fn ensure_transcriber_loaded(&self) -> Result<Arc<Transcriber>> {
        {
            let t = self.transcriber.lock();
            if let Some(existing) = t.as_ref() {
                return Ok(existing.clone());
            }
        }
        info!(
            "voice: transcriber not loaded — reloading from {}",
            self.model_path.display()
        );
        let fresh = Transcriber::new(&self.model_path, &self.cfg.language, self.cfg.threads)?;
        let fresh_arc = Arc::new(fresh);
        *self.transcriber.lock() = Some(fresh_arc.clone());
        Ok(fresh_arc)
    }

    pub fn start_recording(&self) -> Result<()> {
        let state = self.state.lock();
        if *state != VoiceServiceState::Idle {
            return Err(anyhow!("voice service already active: {:?}", *state));
        }
        drop(state);
        // Reload the transcriber if it was dropped by the auto-
        // shutdown timer. Costs ~1-2s on first re-use after a
        // long idle but most users won't notice — the recording
        // starts immediately, and inference only runs at stop.
        self.ensure_transcriber_loaded()?;
        // Now safe to enter the Recording state.
        let mut state = self.state.lock();
        self.recorder.lock().start()?;
        *state = VoiceServiceState::Recording;
        self.poke_activity();
        info!("Voice recording started");
        Ok(())
    }

    /// Stop recording and run a final transcription. Returns the transcribed text.
    pub fn stop_recording(&self) -> Result<String> {
        let mut state = self.state.lock();
        if *state != VoiceServiceState::Recording {
            warn!("stop_recording called while not recording (state={:?})", *state);
            return Ok(String::new());
        }

        let mut recorder = self.recorder.lock();
        recorder.stop();
        let raw = recorder.drain();
        let input_rate = recorder.input_sample_rate();
        drop(recorder);

        let duration_ms = (raw.len() as f64 / input_rate as f64 * 1000.0) as u32;
        if duration_ms < self.cfg.min_audio_ms {
            *state = VoiceServiceState::Idle;
            self.poke_activity();
            info!("Audio too short ({}ms), skipping transcription", duration_ms);
            return Ok(String::new());
        }

        *state = VoiceServiceState::Transcribing;
        drop(state);

        let resampled = resample_to_16k(&raw, input_rate);
        // Take a strong reference to the transcriber inside the
        // mutex; if it was released between start_recording and
        // here (shouldn't happen but defensive), re-load.
        let transcriber = self.ensure_transcriber_loaded()?;
        let text = transcriber.transcribe(&resampled)?;
        info!(
            "Transcribed {} samples ({}ms): {:?}",
            resampled.len(),
            duration_ms,
            text
        );

        *self.state.lock() = VoiceServiceState::Idle;
        self.poke_activity();
        Ok(text)
    }

    pub fn cancel_recording(&self) {
        let mut state = self.state.lock();
        if *state == VoiceServiceState::Idle {
            return;
        }
        let mut recorder = self.recorder.lock();
        recorder.stop();
        let _ = recorder.drain();
        *state = VoiceServiceState::Idle;
        info!("Voice recording cancelled");
    }

    /// Current input level in [0.0, 1.0] from the audio device's
    /// peak meter. Returns 0.0 when not recording or when no audio
    /// has been received yet. Cheap — just an atomic load.
    pub fn current_level(&self) -> f32 {
        self.recorder.lock().current_level()
    }

    /// Get a clone of the Arc<Transcriber> for use in worker threads
    /// that want to run inference on snapshots they take from the
    /// audio ring directly. Returns None if the transcriber has
    /// been released (auto-shutdown timeout) and the caller didn't
    /// explicitly ensure it was loaded. Most callers should go
    /// through `start_recording` first which auto-reloads.
    pub fn transcriber(&self) -> Option<Arc<Transcriber>> {
        self.transcriber.lock().clone()
    }

    /// Snapshot-only handles for worker threads. Returns:
    ///   - the audio ring buffer (Arc<Mutex<HeapRb<f32>>>) for
    ///     non-destructive snapshots
    ///   - the device's native sample rate (u32) for resampling
    ///
    /// These are both Send unlike the full AudioRecorder (which
    /// holds a cpal::Stream that's pinned to its creation thread).
    /// Worker threads use this pair to snapshot + resample + run
    /// inference without ever touching the recorder itself.
    ///
    /// Returns the sample rate of the LAST start_recording call;
    /// returns 16000 (the assumed default) if recording has never
    /// been started.
    pub fn audio_snapshot_handle(&self) -> (crate::voice::recorder::AudioRing, u32) {
        let rec = self.recorder.lock();
        (rec.ring(), rec.input_sample_rate())
    }

    /// Asynchronous stop: drain the recorder synchronously (cheap),
    /// then spawn a worker thread that runs whisper inference and
    /// posts the result through the returned Receiver. The caller
    /// polls the receiver on its own schedule (e.g. from a glib
    /// timeout) without blocking the GTK main loop.
    ///
    /// Returns `Ok(Some(rx))` when inference was started, the rx
    /// will receive the transcribed string (possibly empty if the
    /// audio was too short or silence).
    ///
    /// Returns `Ok(None)` when there was nothing to transcribe (no
    /// recording was active, or the buffer was too short by the
    /// min_audio_ms threshold).
    ///
    /// Returns `Err` if the recorder couldn't be stopped or the
    /// worker couldn't be spawned.
    pub fn stop_recording_async(
        &self,
    ) -> Result<Option<std::sync::mpsc::Receiver<Result<String>>>> {
        let mut state = self.state.lock();
        if *state != VoiceServiceState::Recording {
            tracing::warn!(
                "stop_recording_async called while not recording (state={:?})",
                *state
            );
            return Ok(None);
        }

        // Drain on the main thread — this is fast (a few KB memcpy)
        // and avoids sending the recorder across threads.
        let mut recorder = self.recorder.lock();
        recorder.stop();
        let raw = recorder.drain();
        let input_rate = recorder.input_sample_rate();
        drop(recorder);

        let duration_ms = (raw.len() as f64 / input_rate as f64 * 1000.0) as u32;
        if duration_ms < self.cfg.min_audio_ms {
            *state = VoiceServiceState::Idle;
            info!(
                "Audio too short ({}ms < {}ms), skipping transcription",
                duration_ms, self.cfg.min_audio_ms
            );
            return Ok(None);
        }

        *state = VoiceServiceState::Transcribing;
        drop(state);

        // Worker thread does resampling + inference. Ensure the
        // transcriber is loaded BEFORE spawning the thread (re-
        // loads from disk if the auto-shutdown timer dropped it).
        let transcriber = self.ensure_transcriber_loaded()?;
        let (tx, rx) = std::sync::mpsc::channel::<Result<String>>();
        let state_for_worker = self.state.clone();
        let activity_for_worker = self.last_activity.clone();
        std::thread::Builder::new()
            .name("voice-inference".to_string())
            .spawn(move || {
                let resampled = resample_to_16k(&raw, input_rate);
                let result = transcriber.transcribe(&resampled);
                *state_for_worker.lock() = VoiceServiceState::Idle;
                activity_for_worker.store(now_secs(), Ordering::Relaxed);
                let _ = tx.send(result);
            })
            .context("spawn voice-inference thread")?;

        Ok(Some(rx))
    }
}

/// Current epoch seconds. Falls back to 0 on clock skew (system
/// time before UNIX epoch — shouldn't happen in practice).
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
