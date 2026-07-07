//! voice.rs — the speech-to-text BASE: mic capture → whisper → `voice_ingest::Pass`.
//!
//! This is the upstream seam `voice_ingest` documents ("mic capture, the whisper FFI"). It wires the
//! ported `whisper_pool` (a worker pool of whisper states — the efficient-batching primitive) to a `cpal`
//! microphone stream, and hands the transcriptions to the existing downstream pipeline as `Pass`es
//! (→ `voice_ingest::Transcriber` → `voicefan` → `reaction`).
//!
//! Shape:
//!   1. `init(model)` loads a whisper GGML model and spins up the worker pool (once).
//!   2. `VoiceCapture::start()` opens the default mic and streams mono audio into a buffer. The app owns
//!      the returned `VoiceCapture` (a cpal `Stream` is `!Send`, so it lives on the UI thread), and calls
//!      `poll()` each frame — that runs an energy VAD, cuts utterances on silence, resamples to 16 kHz, and
//!      submits them to the pool.
//!   3. `poll_passes()` drains finished transcriptions and maps them to `voice_ingest::Pass`.
//!
//! Everything is behind the `voice` feature (whisper-rs + cpal). Without it, the stubs below compile so
//! the default build is unaffected. Built with `--features voice-vulkan` on the Asahi M2 for GPU whisper.

use crate::voice_ingest::Pass;

#[cfg(feature = "voice")]
pub use imp::*;
#[cfg(not(feature = "voice"))]
pub use stub::*;

#[cfg(feature = "voice")]
mod imp {
    use super::*;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Instant;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    /// Target sample rate whisper expects.
    const WHISPER_HZ: u32 = 16_000;

    static INITED: OnceLock<bool> = OnceLock::new();

    /// Path to the whisper GGML model. Order: LOGSCOPE_WHISPER_MODEL, then the first `ggml-*.bin` in
    /// `~/models` (where you drop models), then `$XDG_DATA_HOME/logscope/whisper/ggml-base.en.bin`.
    pub fn model_path() -> std::path::PathBuf {
        if let Ok(p) = std::env::var("LOGSCOPE_WHISPER_MODEL") { return p.into(); }
        let home = std::env::var("HOME").unwrap_or_default();
        let models = std::path::PathBuf::from(&home).join("models");
        if let Ok(rd) = std::fs::read_dir(&models) {
            let mut cands: Vec<std::path::PathBuf> = rd.flatten().map(|e| e.path())
                .filter(|p| p.file_name().and_then(|n| n.to_str())
                    .map(|n| n.starts_with("ggml-") && n.ends_with(".bin")).unwrap_or(false))
                .collect();
            // Quality-ranked, not alphabetical: with fast-path tiers (tiny/base) also dropped in ~/models
            // for latency racing, a plain alpha sort would wrongly promote e.g. ggml-base.en.bin ('b' <
            // 'l') to primary over ggml-large-v3-turbo.bin. Rank by tier so the best model is always
            // primary; tier_model_path() finds the tiny/base ones separately for the fast-path race.
            cands.sort_by_key(|p| (model_rank(p.file_name().and_then(|n| n.to_str()).unwrap_or("")), p.clone()));
            if let Some(p) = cands.into_iter().next() { return p; }
        }
        let base = std::env::var("XDG_DATA_HOME").map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from(&home).join(".local/share"));
        base.join("logscope/whisper/ggml-base.en.bin")
    }

    /// Quality rank for a ggml model filename — lower is better. Drives the primary-model pick above.
    fn model_rank(name: &str) -> i32 {
        let n = name.to_lowercase();
        if n.contains("large") || n.contains("turbo") { 0 }
        else if n.contains("medium") { 1 }
        else if n.contains("small") { 2 }
        else if n.contains("base") { 3 }
        else if n.contains("tiny") { 4 }
        else { 5 }
    }

    /// True when a mic and a model file are both present (cheap pre-check for the UI).
    pub fn is_available() -> bool {
        model_path().exists() && cpal::default_host().default_input_device().is_some()
    }

    /// Load the whisper model and start the worker pool. Idempotent; safe to call on first voice use.
    /// Worker count via LOGSCOPE_STT_WORKERS (default 1 — bump for multi-speaker fan-out).
    pub fn init(model: &std::path::Path) -> Result<(), String> {
        if *INITED.get_or_init(|| false) { return Ok(()); }
        let path_str = model.to_str().ok_or_else(|| format!("whisper model path not UTF-8: {}", model.display()))?;
        let t0 = Instant::now();
        let ctx = Arc::new(whisper_rs::WhisperContext::new_with_params(
            path_str,
            whisper_rs::WhisperContextParameters::default(),
        ).map_err(|e| format!("whisper init ({}): {:?}", model.display(), e))?);
        let workers = std::env::var("LOGSCOPE_STT_WORKERS").ok().and_then(|s| s.parse().ok()).unwrap_or(1usize).clamp(1, 4);
        crate::whisper_pool::init_with_tiny_ctx(ctx.clone(), workers);
        crate::voice_confusion::load_from_disk();
        crate::voice_confusion::seed_universal_confusions();
        // Fold the primary model into the fast-path race as the HighAccuracy tier (shares the Arc — no
        // second decode). Then probe for Tiny/Base models to race for latency on short commands; fully
        // optional — if absent, loaded_tiers().len() stays 1 and callers fall back to the plain path.
        crate::fastpath::load_tier_ctx(crate::fastpath::Tier::HighAccuracy, ctx);
        if let Some(p) = tier_model_path("tiny") { let _ = crate::fastpath::load_tier(crate::fastpath::Tier::Tiny, &p); }
        if let Some(p) = tier_model_path("base") { let _ = crate::fastpath::load_tier(crate::fastpath::Tier::Base, &p); }
        let _ = INITED.set(true);
        crate::clog!("voice", "whisper ready ({} workers, {} fast-path tiers) in {:?}",
            workers, crate::fastpath::loaded_tiers().len(), t0.elapsed());
        Ok(())
    }

    /// Find a `ggml-<tag>*.bin` model in `~/models` for the fast-path Tiny/Base tiers (e.g. tag="tiny"
    /// matches ggml-tiny.en.bin / ggml-tiny-q5_1.bin). Override with LOGSCOPE_WHISPER_<TAG> (e.g.
    /// LOGSCOPE_WHISPER_TINY). Absent by default — dropping a file in is all that's needed to enable it.
    fn tier_model_path(tag: &str) -> Option<std::path::PathBuf> {
        if let Ok(p) = std::env::var(format!("LOGSCOPE_WHISPER_{}", tag.to_uppercase())) { return Some(p.into()); }
        let home = std::env::var("HOME").ok()?;
        let dir = std::path::PathBuf::from(&home).join("models");
        let mut cands: Vec<std::path::PathBuf> = std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str())
                .map(|n| { let n = n.to_lowercase(); n.starts_with(&format!("ggml-{tag}")) && n.ends_with(".bin") })
                .unwrap_or(false))
            .collect();
        cands.sort();
        cands.into_iter().next()
    }

    /// Submit a finished 16 kHz mono utterance for transcription (the audio seam — also usable for
    /// tests/replay). Uses the fast-path tier race when more than one tier is loaded (lower latency on
    /// short commands); otherwise the plain single-tier pool — today's behavior when no extra models exist.
    pub fn submit_utterance(samples_16k: Vec<f32>, speaker: String) {
        if samples_16k.is_empty() { return; }
        if crate::fastpath::loaded_tiers().len() > 1 {
            crate::fastpath::submit(samples_16k, speaker);
            return;
        }
        let _ = crate::whisper_pool::submit(crate::whisper_pool::Job {
            audio: samples_16k,
            speaker,
            generation: 0,
            submitted_at: Instant::now(),
            audio_start_ms: 0,
            muted: false,
        });
    }

    /// Drain finished transcriptions (from whichever backend produced them) and map to `voice_ingest::Pass`.
    /// A result is a whole finalized utterance, so `stable = true` (the streaming/partial layer can lower
    /// this later).
    pub fn poll_passes() -> Vec<Pass> {
        let mut out: Vec<Pass> = crate::whisper_pool::drain_results().into_iter().filter_map(|r| {
            let text = r.text.trim().to_string();
            crate::clog!("voice", "heard {text:?}"); // every result, so you can see exactly what whisper returned
            if !is_speech(&text) { return None; } // drop empties + whisper's silence hallucinations
            let t_ms = if r.audio_start_ms != 0 { r.audio_start_ms } else { now_ms() };
            Some(Pass { speaker: r.speaker.clone(), name: r.speaker, full_text: text, stable: true, t_ms })
        }).collect();
        out.extend(crate::fastpath::drain_results().into_iter().filter_map(|(speaker, text, t_ms, is_final)| {
            let text = text.trim().to_string();
            crate::clog!("voice", "heard {text:?} (fast-path)");
            if !is_speech(&text) { return None; }
            // is_final=false is a volatile fast-tier preview (updates the display, doesn't finalize);
            // is_final=true commits the utterance. That's exactly Pass.stable's contract.
            Some(Pass { speaker: speaker.clone(), name: speaker, full_text: text, stable: is_final, t_ms })
        }));
        out
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }

    /// Reject empty results, whisper's silence artifacts, repetition loops, and its frequent no-audio
    /// hallucinations (YouTube-transcript boilerplate) so a silent/wrong mic can't fire garbage. Ported from
    /// voiceterm's is_sane_transcription — the repetition + known-phrase checks are what stop invisible dictation.
    fn is_speech(text: &str) -> bool {
        let t = text.trim();
        let len = t.chars().count();
        if len < 2 || len > 200 { return false; } // too short, or a runaway wall of repeated text
        if (t.starts_with('[') && t.ends_with(']')) || (t.starts_with('(') && t.ends_with(')')) { return false; }
        if !t.chars().any(|c| c.is_alphanumeric()) { return false; }
        // repetition loop: if one token is >50% of all tokens, it's almost certainly a hallucination
        let words: Vec<&str> = t.split_whitespace().collect();
        if words.len() >= 4 {
            let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
            for w in &words { *counts.entry(*w).or_insert(0) += 1; }
            if counts.values().copied().max().unwrap_or(0) * 2 > words.len() { return false; }
        }
        let low = t.to_lowercase();
        const BAD: &[&str] = &["thank you for watching", "thanks for watching", "subscribe to",
            "like and subscribe", "see you next time", "see you in the next video", "don't forget to subscribe",
            "[music]", "[silence]", "[blank_audio]"];
        !BAD.iter().any(|p| low.contains(p))
    }

    /// A live microphone capture. The app owns one (the cpal `Stream` is `!Send`), calls `poll()` each frame.
    pub struct VoiceCapture {
        _stream: cpal::Stream,          // held to keep the stream alive
        buf: Arc<Mutex<Vec<f32>>>,      // mono samples at `src_rate`, filled by the audio callback
        src_rate: u32,
        speaker: String,
        // energy-VAD state
        in_speech: bool,
        silence_run: usize,             // consecutive below-threshold samples (at src_rate)
        speech: Vec<f32>,               // accumulating current utterance at src_rate
        last_meter: Instant,            // throttle for the input-level log
        // rolling-RMS VAD (ported from voiceterm): energy averaged over ~100ms, snapshot per window. Robust
        // vs instantaneous peak, which triggers on transient clicks. rms=sqrt(last_mean_sq).
        rms_sum_sq: f32,
        rms_count: usize,
        rms_window: usize,
        last_rms: f32,
        // preroll (ported from voiceterm): a small ring buffer of the most recent raw samples, kept
        // continuously regardless of speech state. RMS is a lagging measure (averaged over ~100ms), so by
        // the time it crosses threshold the first syllable has often already happened — seeding the
        // utterance with the ring buffer on speech onset recovers it instead of clipping it.
        preroll: std::collections::VecDeque<f32>,
        preroll_cap: usize,
    }

    impl VoiceCapture {
        /// Open a mic and start streaming. Tries LOGSCOPE_MIC matches, then the OS default, then every other
        /// input device, using the first that actually opens (robust to silent/broken default devices).
        pub fn start(speaker: impl Into<String>) -> Result<Self, String> {
            let want = std::env::var("LOGSCOPE_MIC").ok().filter(|s| !s.trim().is_empty());
            Self::start_with_mic(speaker, want.as_deref())
        }

        /// Like `start`, but `mic` (if given) OVERRIDES LOGSCOPE_MIC as the name-substring to prefer — used
        /// by multi-speaker setups (LOGSCOPE_SPEAKERS) to pin each capture to its own device independent of
        /// the single global env var. `mic = None` behaves exactly like plain `start`.
        pub fn start_with_mic(speaker: impl Into<String>, mic: Option<&str>) -> Result<Self, String> {
            let host = cpal::default_host();
            let want = mic.map(|s| s.to_lowercase());
            // Ordered candidate list (ported from voiceterm): the wanted substring matches first, then the
            // OS default, then the rest (deduped by name). On PipeWire/Pulse the default input is often a
            // silent monitor/dummy sink — trying each until a stream opens lets you land on the real mic.
            let enumerated: Vec<cpal::Device> = host.input_devices().map(|it| it.collect()).unwrap_or_default();
            let names: Vec<String> = enumerated.iter().map(|d| d.name().unwrap_or_else(|_| "?".into())).collect();
            crate::clog!("voice", "input devices: {names:?}");
            let (mut cands, mut rest): (Vec<(cpal::Device, String)>, Vec<(cpal::Device, String)>) = (Vec::new(), Vec::new());
            for (d, n) in enumerated.into_iter().zip(names.iter().cloned()) {
                if want.as_ref().is_some_and(|w| n.to_lowercase().contains(w)) { cands.push((d, n)); } else { rest.push((d, n)); }
            }
            if let Some(d) = host.default_input_device() {
                let n = d.name().unwrap_or_else(|_| "default".into());
                if !cands.iter().any(|(_, cn)| *cn == n) { cands.push((d, n)); }
            }
            for (d, n) in rest { if !cands.iter().any(|(_, cn)| *cn == n) { cands.push((d, n)); } }
            if cands.is_empty() { return Err("no input device (check mic permissions / pipewire-pulse)".into()); }

            let buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
            let mut errors = Vec::new();
            for (dev, name) in cands {
                match open_stream(&dev, buf.clone()) {
                    Ok((stream, src_rate, chans, fmt)) => {
                        stream.play().map_err(|e| format!("stream play: {e}"))?;
                        crate::clog!("voice", "using mic '{name}' — {src_rate} Hz · {chans} ch · {fmt:?} · VAD thresh {} (LOGSCOPE_STT_VAD)",
                            std::env::var("LOGSCOPE_STT_VAD").unwrap_or_else(|_| "0.012".into()));
                        return Ok(Self { _stream: stream, buf, src_rate, speaker: speaker.into(),
                            in_speech: false, silence_run: 0, speech: Vec::new(), last_meter: Instant::now(),
                            rms_sum_sq: 0.0, rms_count: 0, rms_window: (src_rate / 10).max(160) as usize, last_rms: 0.0,
                            preroll: std::collections::VecDeque::with_capacity((src_rate as usize / 10 * 3).max(480)),
                            preroll_cap: (src_rate as usize * 3 / 10).max(480) }); // ~300ms
                    }
                    Err(e) => { crate::clog!("voice", "mic '{name}' unusable: {e}"); errors.push(format!("{name}: {e}")); }
                }
            }
            Err(format!("no usable mic. tried: {}", errors.join("; ")))
        }

        /// Drain captured audio, run the VAD, and submit any completed utterance to the pool. Call once
        /// per UI frame. Returns true if an utterance was submitted this call.
        pub fn poll(&mut self) -> bool {
            let chunk = { let mut b = self.buf.lock().unwrap(); std::mem::take(&mut *b) };
            if chunk.is_empty() { return false; }
            // rolling-RMS energy vs threshold + endpoint after ~600 ms of silence.
            let thresh = std::env::var("LOGSCOPE_STT_VAD").ok().and_then(|s| s.parse().ok()).unwrap_or(0.012f32);
            // Endpoint after a pause. Configurable (voiceterm's PER_SPEAKER_SILENCE_MS concept — default
            // 600ms; a slower or more thoughtful speaker may want this higher via LOGSCOPE_STT_SILENCE_MS).
            let silence_ms = std::env::var("LOGSCOPE_STT_SILENCE_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(600u32);
            let end_silence = (self.src_rate as f32 * silence_ms as f32 / 1000.0) as usize;
            let mut submitted = false;
            for &s in &chunk {
                // update the rolling RMS (sliding mean of squared samples over one window, then snapshot)
                self.rms_sum_sq += s * s;
                self.rms_count += 1;
                if self.rms_count >= self.rms_window {
                    self.last_rms = (self.rms_sum_sq / self.rms_count as f32).sqrt();
                    self.rms_sum_sq = 0.0; self.rms_count = 0;
                }
                let loud = self.last_rms > thresh;
                if loud && !self.in_speech {
                    // seed with the preroll (NOT yet containing `s`) so the syllable that made RMS cross
                    // threshold isn't clipped — RMS is a ~100ms-lagging measure, so speech has usually
                    // already begun by the time it trips.
                    self.in_speech = true;
                    self.speech.clear();
                    self.speech.extend(self.preroll.iter().copied());
                }
                // feed the preroll ring buffer AFTER any onset-seeding above, regardless of speech state
                self.preroll.push_back(s);
                while self.preroll.len() > self.preroll_cap { self.preroll.pop_front(); }
                if loud {
                    self.silence_run = 0;
                    self.speech.push(s);
                } else if self.in_speech {
                    self.speech.push(s);       // keep trailing silence inside the utterance
                    self.silence_run += 1;
                    if self.silence_run >= end_silence {
                        let dur = self.speech.len() as f32 / self.src_rate as f32;
                        let spk = self.speech.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
                        let utt = resample(&self.speech, self.src_rate, WHISPER_HZ);
                        if utt.len() > WHISPER_HZ as usize / 4 { // ignore <0.25 s blips
                            crate::clog!("voice", "utterance captured — {dur:.1}s, peak {spk:.3} → whisper");
                            submit_utterance(utt, self.speaker.clone());
                            submitted = true;
                        }
                        self.in_speech = false; self.silence_run = 0; self.speech.clear();
                    }
                }
            }
            // Throttled level meter (~1/s) showing the rolling RMS vs the threshold, so you can see audio
            // arriving and tune LOGSCOPE_STT_VAD. Flat near 0 while speaking → wrong mic (set LOGSCOPE_MIC).
            if self.last_meter.elapsed().as_secs_f32() > 1.0 {
                self.last_meter = Instant::now();
                let bars = (self.last_rms / thresh.max(1e-4) * 8.0).min(24.0) as usize;
                crate::clog!("voice", "mic rms {:.4} [{}{}] speech>{:.3}{}", self.last_rms,
                    "#".repeat(bars), "·".repeat(24 - bars.min(24)), thresh,
                    if self.in_speech { " (capturing…)" } else { "" });
            }
            submitted
        }
    }

    /// Try to open an input stream on `dev`. Returns (stream, sample_rate, channels, format); the audio
    /// callback downmixes to mono and appends to `buf`. Any failure is returned so `start` can try the next
    /// candidate device instead of giving up.
    fn open_stream(dev: &cpal::Device, buf: Arc<Mutex<Vec<f32>>>)
        -> Result<(cpal::Stream, u32, usize, cpal::SampleFormat), String>
    {
        let cfg = dev.default_input_config().map_err(|e| format!("config: {e}"))?;
        let (src_rate, channels, fmt) = (cfg.sample_rate().0, cfg.channels() as usize, cfg.sample_format());
        let bc = buf;
        let err_cb = |e| crate::clog!("voice", "stream error: {e}");
        let stream = match fmt {
            cpal::SampleFormat::F32 => dev.build_input_stream(&cfg.config(),
                move |data: &[f32], _: &_| push_mono(&bc, data, channels, |s| s), err_cb, None),
            cpal::SampleFormat::I16 => dev.build_input_stream(&cfg.config(),
                move |data: &[i16], _: &_| push_mono(&bc, data, channels, |s| s as f32 / 32768.0), err_cb, None),
            cpal::SampleFormat::U16 => dev.build_input_stream(&cfg.config(),
                move |data: &[u16], _: &_| push_mono(&bc, data, channels, |s| (s as f32 - 32768.0) / 32768.0), err_cb, None),
            other => return Err(format!("unsupported sample format: {other:?}")),
        }.map_err(|e| format!("build stream: {e}"))?;
        Ok((stream, src_rate, channels, fmt))
    }

    /// Downmix interleaved multi-channel `data` to mono and append to `buf`, converting each sample via `f`.
    fn push_mono<T: Copy>(buf: &Arc<Mutex<Vec<f32>>>, data: &[T], channels: usize, f: impl Fn(T) -> f32) {
        if channels == 0 { return; }
        let mut b = buf.lock().unwrap();
        b.reserve(data.len() / channels);
        for frame in data.chunks_exact(channels) {
            let mut acc = 0.0f32;
            for &s in frame { acc += f(s); }
            b.push(acc / channels as f32);
        }
    }

    /// Linear-interpolation resample mono `src` from `from` Hz to `to` Hz. Adequate for STT input; a
    /// polyphase/sinc filter is a later refinement if quality demands.
    fn resample(src: &[f32], from: u32, to: u32) -> Vec<f32> {
        if from == to || src.is_empty() { return src.to_vec(); }
        let ratio = to as f64 / from as f64;
        let out_len = ((src.len() as f64) * ratio).round() as usize;
        let mut out = Vec::with_capacity(out_len);
        for i in 0..out_len {
            let pos = i as f64 / ratio;
            let i0 = pos.floor() as usize;
            let frac = (pos - i0 as f64) as f32;
            let a = src[i0.min(src.len() - 1)];
            let b = src[(i0 + 1).min(src.len() - 1)];
            out.push(a + (b - a) * frac);
        }
        out
    }
}

#[cfg(not(feature = "voice"))]
mod stub {
    use super::*;
    pub fn model_path() -> std::path::PathBuf { std::path::PathBuf::new() }
    pub fn is_available() -> bool { false }
    pub fn init(_model: &std::path::Path) -> Result<(), String> { Err("built without the `voice` feature".into()) }
    pub fn submit_utterance(_samples_16k: Vec<f32>, _speaker: String) {}
    pub fn poll_passes() -> Vec<Pass> { Vec::new() }
    /// Placeholder so the app can hold `Option<VoiceCapture>` regardless of feature. `start()` errors.
    pub struct VoiceCapture(());
    impl VoiceCapture {
        pub fn start(_speaker: impl Into<String>) -> Result<Self, String> { Err("built without the `voice` feature".into()) }
        pub fn start_with_mic(_speaker: impl Into<String>, _mic: Option<&str>) -> Result<Self, String> { Err("built without the `voice` feature".into()) }
        pub fn poll(&mut self) -> bool { false }
    }
}
