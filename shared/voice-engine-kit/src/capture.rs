//! capture — the mic ring buffer, cpal stream setup, and the
//! begin/cancel/finish surface a popup calls.
//!
//! Lifted from voiceterm/src/voice.rs. Two direct calls into
//! voiceterm-specific features were replaced with hooks this crate
//! defines and a consuming app installs -- see capture_hooks.rs and
//! deferral_hooks.rs for exactly what and why:
//!
//!   * the cpal audio callback's three `corpus_recorder::push_samples`
//!     calls (F32/I16/U16 sample-format branches) -> capture_hooks::tap_audio
//!   * begin_capture's `ensure_preview_thread`/`clear_partial_preview`
//!     calls -> capture_hooks::capture_started
//!   * listen_with_audio's `log_summary::VOICE_ACTIVE` before/after pair
//!     -> deferral_hooks::transcribe_starting/transcribe_ending
//!
//! Everything else is unchanged from voiceterm's own.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock, Arc};
use crate::model::{ModelTier, model_path};
#[cfg(feature = "voice")]
use crate::model::ensure_models_loaded;
use crate::transcribe::listen_external;
#[cfg(feature = "voice")]
use crate::transcribe::record_and_transcribe_inner;
#[cfg(feature = "voice")]
use crate::transcribe::transcribe_with_dual_model;


#[cfg(feature = "voice")]
pub(crate) static RING_BUFFER: OnceLock<Arc<Mutex<RingBuf>>> = OnceLock::new();

/// Set to true to cancel an in-progress recording. Polled by the VAD loop.
/// Reset to false when a new capture begins.
pub(crate) static CANCEL_FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// v0.1.368 (PA2 prerequisite): minimum silence_ms across all
/// registered speakers' pending buffers. Updated by the voice
/// loop on each tick after silence accounting completes.
///
/// Why MIN (not max, mean, or per-speaker): the reading-overlay
/// use case is "is the user talking right now." Conservative
/// answer: if ANYONE has just-recent voice, treat as "active
/// speech happening" and don't speed reading up. So we take the
/// minimum across speakers — the freshest voice anywhere.
///
/// Special values: 0 means active speech right now. u32::MAX
/// means no speaker buffers active (long quiet idle). Caller
/// (main.rs) decodes both.
///
/// Polled on the QML pty timer from main.rs, which emits the
/// `voice_silence_tick` signal when the value crosses
/// meaningful thresholds. Tier-2 phoneme classifier work (PA4+)
/// will read the same atomic but at higher frequency.
pub static MIN_SILENCE_MS: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(u32::MAX);

/// Set to true to force the VAD loop to stop NOW and proceed to transcription.
/// Unlike cancel, this preserves the audio captured so far.
pub(crate) static FINISH_FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Cancel any in-progress voice recording. Safe to call from any thread.
/// Effect: the recording loop sees the flag at its next poll (≤80ms latency)
/// and returns an error, dropping the audio without transcription.
pub fn cancel_recording() {
    CANCEL_FLAG.store(true, std::sync::atomic::Ordering::Relaxed);
    eprintln!("Voice: cancel requested");
}

/// Force-finish any in-progress recording. Unlike cancel, this keeps the audio
/// and proceeds to transcription. Use when the user signals "I'm done speaking".
pub fn finish_recording() {
    FINISH_FLAG.store(true, std::sync::atomic::Ordering::Relaxed);
    eprintln!("Voice: finish requested");
}


#[cfg(feature = "voice")]
#[allow(dead_code)] // future preroll capture work
const PREROLL_MS: usize = 700;  // capture this much audio BEFORE the button press
#[cfg(feature = "voice")]
const MAX_RECORD_S: usize = 6;
#[cfg(feature = "voice")]
pub(crate) const TARGET_RATE: u32 = 16000;

#[cfg(feature = "voice")]
pub(crate) struct RingBuf {
    /// Most recent samples, capped at MAX_RECORD_S * TARGET_RATE
    pub(crate) samples: VecDeque<f32>,
    /// Source sample rate (capture native, we resample on extraction)
    pub(crate) source_rate: u32,
    /// Source channel count (we average to mono)
    pub(crate) source_channels: usize,
    /// Are we actively capturing into this buffer?
    pub(crate) capturing: bool,
}

#[cfg(feature = "voice")]
impl RingBuf {
    fn new() -> Self { Self { samples: VecDeque::new(), source_rate: 48000, source_channels: 1, capturing: false } }
    fn capacity(&self) -> usize {
        // Hold up to MAX_RECORD_S worth at the source rate
        self.source_rate as usize * MAX_RECORD_S * self.source_channels.max(1)
    }
    fn push(&mut self, sample: f32) {
        if !self.capturing { return; }
        self.samples.push_back(sample);
        let cap = self.capacity();
        while self.samples.len() > cap {
            self.samples.pop_front();
        }
    }
    pub(crate) fn drain_into_mono(&mut self) -> Vec<f32> {
        let raw: Vec<f32> = self.samples.drain(..).collect();
        if self.source_channels <= 1 {
            return raw;
        }
        let ch = self.source_channels;
        raw.chunks_exact(ch).map(|c| c.iter().sum::<f32>() / ch as f32).collect()
    }

    /// Non-destructive read of current samples, mono-mixed.
    /// v0.1.108: used by the streaming preview worker to peek at
    /// audio mid-recording without disturbing the buffer state.
    /// Caller resamples to 16kHz separately.
    fn snapshot_mono(&self) -> Vec<f32> {
        let raw: Vec<f32> = self.samples.iter().copied().collect();
        if self.source_channels <= 1 {
            return raw;
        }
        let ch = self.source_channels;
        raw.chunks_exact(ch).map(|c| c.iter().sum::<f32>() / ch as f32).collect()
    }
}

/// Background ring buffer — always-on, smaller capacity (3s) than the
/// foreground RingBuf. The cpal callback pushes every sample to BOTH
/// buffers; foreground gates on `capturing`, background never gates.
///
/// Used by the background mic polling loop (see start_background_listener)
/// to opportunistically capture speech for fingerprint-DB enrichment
/// without interfering with foreground voice flow.
///
/// Memory cost: ~280KB at 48kHz × 2ch × 3s × 4 bytes/f32. Cheap.
#[cfg(feature = "voice")]
struct BgRingBuf {
    samples: VecDeque<f32>,
    source_rate: u32,
    source_channels: usize,
    /// Max seconds to retain. Smaller than foreground because background
    /// only needs the most recent few seconds for its periodic snapshot.
    capacity_secs: f32,
    /// Rolling RMS over a small window — used for VAD-like speech
    /// detection without needing to drain. Updated incrementally on
    /// push so checking it is O(1).
    recent_rms_sum_sq: f32,
    recent_rms_count: usize,
    recent_rms_window: usize,
    /// v0.1.124: snapshot of mean squared from the last completed
    /// window. rolling_rms() returns sqrt(this). Decoupled from
    /// the accumulator so the reading remains stable for the
    /// duration of the next window's fill.
    last_mean_sq: f32,
}

#[cfg(feature = "voice")]
impl BgRingBuf {
    fn new() -> Self {
        Self {
            samples: VecDeque::new(),
            source_rate: 48000,
            source_channels: 1,
            capacity_secs: 3.0,
            recent_rms_sum_sq: 0.0,
            recent_rms_count: 0,
            recent_rms_window: 4800, // 100ms at 48kHz
            last_mean_sq: 0.0,
        }
    }
    fn capacity(&self) -> usize {
        (self.source_rate as f32 * self.capacity_secs) as usize
            * self.source_channels.max(1)
    }
    fn push(&mut self, sample: f32) {
        // Track rolling RMS — used by background loop to gate transcription
        // on actual speech presence without scanning the buffer.
        //
        // v0.1.124 bug fix: the old code applied a 0.95 decay multiplier
        // on EVERY push once the window was full. That meant each
        // sample's contribution decayed exponentially within
        // milliseconds — the rolling RMS reading was orders of
        // magnitude lower than actual signal RMS (heartbeat log
        // showed rms=0.0000 even during clear speech, killing wake
        // word detection).
        //
        // Now: simple sliding mean of recent squared samples. We
        // accumulate sum_sq during a window; once the window fills,
        // snapshot the result as `last_mean_sq` and reset for the
        // next window. rolling_rms() returns sqrt(last_mean_sq).
        // Trailing samples between windows contribute to the next
        // reading, ~100ms latency — fine for VAD-style gating.
        self.recent_rms_sum_sq += sample * sample;
        self.recent_rms_count += 1;
        if self.recent_rms_count >= self.recent_rms_window {
            // Window full — finalize and snapshot. Reset for next.
            self.last_mean_sq = self.recent_rms_sum_sq
                / self.recent_rms_count as f32;
            self.recent_rms_sum_sq = 0.0;
            self.recent_rms_count = 0;
        }
        self.samples.push_back(sample);
        let cap = self.capacity();
        while self.samples.len() > cap {
            self.samples.pop_front();
        }
    }
    fn rolling_rms(&self) -> f32 {
        self.last_mean_sq.sqrt()
    }
    fn snapshot_mono(&self) -> Vec<f32> {
        let raw: Vec<f32> = self.samples.iter().copied().collect();
        if self.source_channels <= 1 {
            return raw;
        }
        let ch = self.source_channels;
        raw.chunks_exact(ch).map(|c| c.iter().sum::<f32>() / ch as f32).collect()
    }
}

#[cfg(feature = "voice")]
static BG_RING_BUFFER: OnceLock<Arc<Mutex<BgRingBuf>>> = OnceLock::new();


#[cfg(feature = "voice")]
pub(crate) fn ensure_capture_running() -> Result<(), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.get().is_some() { return Ok(()); }

    let host = cpal::default_host();

    // Device enumeration diagnostics. Useful especially on phone platforms
    // (postmarketOS) where default_input_device() may return None due to
    // PipeWire/PulseAudio/portal-permission interactions. Logging available
    // hosts and devices on every startup tells us what cpal can see.
    crate::clog!("audio", "host: {:?}", host.id());
    match host.input_devices() {
        Ok(devs) => {
            let names: Vec<String> = devs
                .filter_map(|d| d.name().ok())
                .collect();
            if names.is_empty() {
                crate::clog!("audio", "WARNING: no input devices enumerated by host");
            } else {
                crate::clog!("audio", "{} input device(s): {:?}", names.len(), names);
            }
        }
        Err(e) => crate::clog!("audio", "input_devices() error: {}", e),
    }

    let device_opt = match host.default_input_device() {
        Some(d) => {
            if let Ok(name) = d.name() {
                crate::clog!("audio", "default input device: {}", name);
            }
            Some(d)
        }
        None => {
            crate::clog!("audio", "default_input_device() returned None — will try enumerated devices");
            None
        }
    };

    // v0.1.136: defensive device selection. The original path bailed
    // out on the first failure — single `?` on default_input_config()
    // meant a brief pipewire-pulse hiccup or a broken default device
    // (loopback monitor / dummy sink on some Linux setups) would
    // permanently disable voice for the session. The new path:
    //
    //   - Builds an ORDERED list of candidates (default first, then
    //     enumerated devices). De-duped by name.
    //   - Tries up to 3 attempts spaced 250ms apart. Covers the
    //     post-launch window where pipewire-pulse isn't fully ready.
    //   - For each device, tries its default config; if that fails,
    //     scans supported_input_configs for a reasonable one
    //     (1-8 channels, 8-48 kHz).
    //   - Each failure is logged with the device label and the
    //     cpal error, so the next bug report includes a real trail.
    //
    // Behavior on the working device (almost everyone) is unchanged
    // because the default config path is tried FIRST and succeeds.
    let mut candidates: Vec<(cpal::Device, String)> = Vec::new();
    if let Some(d) = device_opt {
        let label = d.name().unwrap_or_else(|_| "default".to_string());
        candidates.push((d, label));
    }
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            let name = d.name().unwrap_or_else(|_| "unknown".to_string());
            if candidates.iter().any(|(_, n)| n == &name) { continue; }
            candidates.push((d, name));
        }
    }
    if candidates.is_empty() {
        crate::clog!("audio", "FATAL: no input devices available (host enumerated 0).");
        crate::clog!("audio", "  On Linux phones (postmarketOS, Mobian), this usually means");
        crate::clog!("audio", "  either no permission for mic access, or PipeWire's PA-compat");
        crate::clog!("audio", "  layer isn't exposing the device. Check:");
        crate::clog!("audio", "    - Is pipewire-pulse running?  systemctl --user status pipewire-pulse");
        crate::clog!("audio", "    - Does pactl list short sources show a mic?");
        crate::clog!("audio", "    - xdg-desktop-portal mic permission for this app");
        return Err("No input device".into());
    }

    let mut chosen: Option<(cpal::Device, cpal::SupportedStreamConfig, String)> = None;
    'outer: for attempt in 0..3 {
        if attempt > 0 {
            crate::clog!("audio", "retry attempt {} after 250ms delay", attempt + 1);
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        for (dev, label) in &candidates {
            match dev.default_input_config() {
                Ok(cfg) => {
                    crate::clog!("audio", "using device {:?} default config: {} Hz, {} ch, {:?}",
                        label, cfg.sample_rate().0, cfg.channels(), cfg.sample_format());
                    chosen = Some((dev.clone(), cfg, label.clone()));
                    break 'outer;
                }
                Err(e) => {
                    crate::clog!("audio", "device {:?} default_input_config() failed: {}", label, e);
                    match dev.supported_input_configs() {
                        Ok(supported) => {
                            for supp_range in supported {
                                let ch = supp_range.channels();
                                let max_sr = supp_range.max_sample_rate().0;
                                if ch == 0 || ch > 8 { continue; }
                                if max_sr < 8000 { continue; }
                                let pick_sr = max_sr.min(48000).max(supp_range.min_sample_rate().0);
                                let cfg = supp_range.with_sample_rate(cpal::SampleRate(pick_sr));
                                crate::clog!("audio", "device {:?} fallback config: {} Hz, {} ch, {:?}",
                                    label, pick_sr, ch, cfg.sample_format());
                                chosen = Some((dev.clone(), cfg, label.clone()));
                                break 'outer;
                            }
                            crate::clog!("audio", "device {:?} had no acceptable supported configs", label);
                        }
                        Err(e2) => {
                            crate::clog!("audio", "device {:?} supported_input_configs() also failed: {}", label, e2);
                        }
                    }
                }
            }
        }
    }
    let (device, config, dev_label) = match chosen {
        Some(t) => t,
        None => {
            crate::clog!("audio", "FATAL: could not get a working config from any device after 3 attempts.");
            return Err("No working input config".into());
        }
    };
    crate::clog!("audio", "selected device: {:?}", dev_label);

    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let format = config.sample_format();

    eprintln!("Voice: starting persistent stream {}Hz, {}ch, {:?}", sample_rate, channels, format);

    // Init ring buffer
    let ring = RING_BUFFER.get_or_init(|| Arc::new(Mutex::new(RingBuf::new())));
    {
        let mut g = ring.lock().unwrap();
        g.source_rate = sample_rate;
        g.source_channels = channels;
        g.samples.clear();
    }
    // Init background ring buffer — always-on snapshot for the magic
    // fingerprint-DB enrichment loop. Same source format as main ring.
    let bg_ring = BG_RING_BUFFER.get_or_init(|| Arc::new(Mutex::new(BgRingBuf::new())));
    {
        let mut g = bg_ring.lock().unwrap();
        g.source_rate = sample_rate;
        g.source_channels = channels;
        // Window matches sample rate (100ms of samples)
        g.recent_rms_window = (sample_rate as usize / 10) * channels.max(1);
        g.samples.clear();
    }

    let ring_clone = ring.clone();
    let bg_ring_clone = bg_ring.clone();
    let err_fn = |err| eprintln!("Voice: stream error: {}", err);

    // Spawn the stream on its own thread - cpal Stream isn't Send so it must
    // live inside the thread that creates it.
    std::thread::spawn(move || {
        let stream_result = match format {
            cpal::SampleFormat::F32 => {
                let r = ring_clone.clone();
                let bg = bg_ring_clone.clone();
                let sr = config.sample_rate().0;
                device.build_input_stream(
                    &config.into(),
                    move |data: &[f32], _: &_| {
                        if let Ok(mut g) = r.lock() {
                            for &s in data { g.push(s); }
                        }
                        if let Ok(mut g) = bg.lock() {
                            for &s in data { g.push(s); }
                        }
                        // v0.1.192 (ship 2.3): tap into corpus recorder.
                        // Inactive-state check is a single atomic load;
                        // when not recording the cost is nil. Active
                        // path appends samples under a short-held mutex.
                        crate::capture_hooks::tap_audio(data, sr);
                    },
                    err_fn, None
                )
            }
            cpal::SampleFormat::I16 => {
                let r = ring_clone.clone();
                let bg = bg_ring_clone.clone();
                let sr = config.sample_rate().0;
                device.build_input_stream(
                    &config.into(),
                    move |data: &[i16], _: &_| {
                        // v0.1.192 (ship 2.3): convert once, push to all
                        // three sinks. tmp buffer typically 256-1024
                        // samples; allocation cost is small per callback.
                        let mut tmp: Vec<f32> = Vec::with_capacity(data.len());
                        for &s in data { tmp.push(s as f32 / 32768.0); }
                        if let Ok(mut g) = r.lock() {
                            for &s in &tmp { g.push(s); }
                        }
                        if let Ok(mut g) = bg.lock() {
                            for &s in &tmp { g.push(s); }
                        }
                        crate::capture_hooks::tap_audio(&tmp, sr);
                    },
                    err_fn, None
                )
            }
            cpal::SampleFormat::U16 => {
                let r = ring_clone.clone();
                let bg = bg_ring_clone.clone();
                let sr = config.sample_rate().0;
                device.build_input_stream(
                    &config.into(),
                    move |data: &[u16], _: &_| {
                        let mut tmp: Vec<f32> = Vec::with_capacity(data.len());
                        for &s in data { tmp.push((s as f32 - 32768.0) / 32768.0); }
                        if let Ok(mut g) = r.lock() {
                            for &s in &tmp { g.push(s); }
                        }
                        if let Ok(mut g) = bg.lock() {
                            for &s in &tmp { g.push(s); }
                        }
                        crate::capture_hooks::tap_audio(&tmp, sr);
                    },
                    err_fn, None
                )
            }
            _ => {
                crate::clog!("audio", "FATAL: sample format {:?} not handled.", format);
                crate::clog!("audio", "  Currently F32, I16, U16 are supported.");
                crate::clog!("audio", "  The device exposed {:?}, which is unusual on PipeWire/PulseAudio.", format);
                crate::clog!("audio", "  Voice will be unavailable this session.");
                return;
            }
        };

        match stream_result {
            Ok(stream) => {
                if let Err(e) = stream.play() {
                    crate::clog!("audio", "FATAL: stream.play() failed on {:?}: {}", dev_label, e);
                    crate::clog!("audio", "  The stream was built successfully but couldn't start.");
                    crate::clog!("audio", "  This often means another process has exclusive access");
                    crate::clog!("audio", "  to the device, or the device disappeared between probe");
                    crate::clog!("audio", "  and play (USB unplug, suspend, etc).");
                    return;
                }
                eprintln!("Voice: persistent stream playing on {:?}", dev_label);
                let _ = STARTED.set(());
                // Park forever — the stream lives as long as this thread does
                loop { std::thread::park(); }
            }
            Err(e) => {
                crate::clog!("audio", "FATAL: build_input_stream() failed on {:?}: {}", dev_label, e);
                crate::clog!("audio", "  The device probed OK but couldn't allocate a stream.");
                crate::clog!("audio", "  Possible causes: PipeWire buffer-size mismatch, sample-rate");
                crate::clog!("audio", "  the kernel driver doesn't actually support despite advertising,");
                crate::clog!("audio", "  or device claimed by another exclusive client.");
            }
        }
    });

    Ok(())
}

/// Start capturing into the ring buffer (but don't block).
/// Returns immediately. Pre-roll from buffer is included.
pub fn begin_capture() {
    #[cfg(feature = "voice")]
    {
        // Clear any previous cancel/finish state
        CANCEL_FLAG.store(false, std::sync::atomic::Ordering::Relaxed);
        FINISH_FLAG.store(false, std::sync::atomic::Ordering::Relaxed);
        // v0.1.108: streaming preview. Start the dumb-DB poller
        // (idempotent) and wipe any stale preview from a prior
        // session.
        crate::capture_hooks::capture_started();
        let _ = ensure_capture_running();
        if let Some(ring) = RING_BUFFER.get() {
            if let Ok(mut g) = ring.lock() {
                // Drop pre-roll older than ~80ms so the start chime (which plays
                // immediately before this call) doesn't bleed into the recording.
                // Without this, whisper hallucinated leading "and ..." on every
                // utterance because it interpreted the chime tail as a phoneme.
                // We keep a small amount of pre-roll (~80ms) so users don't lose
                // the initial syllable of fast speakers.
                let preroll_samples = (g.source_rate as usize / 1000) * 80
                    * g.source_channels as usize;
                if g.samples.len() > preroll_samples {
                    let drop = g.samples.len() - preroll_samples;
                    g.samples.drain(..drop);
                }
                g.capturing = true;
            }
        }
    }
}

/// Record and transcribe ~5 seconds of audio. Returns text or error message.
pub fn listen() -> String {
    let (text, _audio) = listen_with_audio();
    text
}

/// v0.1.281 (voice speed): warm up whisper state allocations
/// in the background so the first real user utterance doesn't
/// pay cold-start latency. Spawned from main thread at app
/// startup once a model is loaded.
///
/// What this does:
///   1. Allocates ~1 second of silence as fake audio
///   2. Runs transcribe_with_dual_model on it
///   3. Discards the result
///
/// Side effects: the per-tier state (TINY_STATE, BASE_STATE,
/// LARGE_STATE) gets initialized to its ~210MB allocation,
/// CPU caches warm up for the matmul kernels, and the
/// initial_prompt gets tokenized once. First REAL utterance
/// after warmup completes feels noticeably snappier.
///
/// Cost: blocks the calling thread for ~500-1000ms (Tiny on
/// 1s silence is fast). Caller should spawn from a background
/// thread.
///
/// Idempotent — calling twice is wasted CPU but not harmful.
#[cfg(feature = "voice")]
pub fn warmup() {
    use std::time::Instant;
    let t_start = Instant::now();
    // 1 second of silence at 16kHz.
    let silence = vec![0.0_f32; 16_000];
    match transcribe_with_dual_model(&silence) {
        Ok(_) => {
            crate::clog!("whisper", "warmup done in {:.2}s",
                t_start.elapsed().as_secs_f32());
        }
        Err(e) => {
            crate::clog!("whisper", "warmup failed: {} (will lazy-init on first listen)", e);
        }
    }
}

#[cfg(not(feature = "voice"))]
pub fn warmup() {}

/// Like listen() but also returns the captured 16kHz mono audio for downstream
/// processing (MFCC fingerprinting, etc).
pub fn listen_with_audio() -> (String, Vec<f32>) {
    // Tell a consuming app to defer other work -- voice is about to
    // take CPU. RAII-style: cleared on every return path via the
    // closure below. (Was a direct crate::log_summary::VOICE_ACTIVE
    // store in voiceterm -- real app state this crate has no
    // business naming; see deferral_hooks.rs.)
    crate::deferral_hooks::transcribe_starting();
    let result = (|| {
        #[cfg(feature = "voice")]
        {
            match record_and_transcribe_inner() {
                Ok((text, audio)) => return (text, audio),
                Err(e) => {
                    // v0.1.225: user-cancellation is NOT a failure to
                    // recover from. Falling back to listen_external
                    // on a cancel made the "X" button trigger a
                    // SECOND voice attempt that often surfaced
                    // "arecord/whisper not in PATH" on systems
                    // without those tools — confusing because the
                    // user just asked to STOP. Cancel returns empty
                    // text and the QML popup closes cleanly.
                    if e.contains("cancelled") {
                        eprintln!("Voice (native): cancelled — no fallback");
                        return (String::new(), Vec::new());
                    }
                    eprintln!("Voice (native): {}, falling back to external tool", e);
                }
            }
        }
        (listen_external(), Vec::new())
    })();
    // Clear the deferral -- consuming app's worker can resume.
    crate::deferral_hooks::transcribe_ending();
    result
}
