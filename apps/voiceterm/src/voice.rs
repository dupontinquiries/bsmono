// src/voice.rs
// Voice input via whisper-rs + cpal.
// Compile with `cargo build --features voice` to enable.
// Without the feature, falls back to system tools (whisper-cpp, nerd-dictation).
//
// Model location: ~/.config/web2qml/models/
//   ggml-tiny.en.bin       — fast, less accurate (~75MB fp16, ~31MB Q5_1)
//   ggml-tiny.en-q5_1.bin  — same as above quantized
//   ggml-base.en.bin       — slower, more accurate (~142MB fp16, ~57MB Q5_1)
//   ggml-base.en-q5_1.bin  — same as above quantized
// Download via setup-voice.sh or directly from
// https://huggingface.co/ggerganov/whisper.cpp/tree/main
//
// When BOTH tiny and base are present, transcription dispatches to BOTH
// in parallel. tiny finishes first; we wait up to 1s for base to catch
// up. If base returns within that window, use base's better result; if
// not, use tiny's already-available result.

use std::path::PathBuf;
#[cfg(feature = "voice")]
use std::sync::{Mutex, OnceLock, Arc};
#[cfg(feature = "voice")]
use std::collections::VecDeque;

/// Continuous ring buffer of the last N samples (always 16kHz mono).
/// When user starts recording, we already have ~500ms of pre-roll captured.
#[cfg(feature = "voice")]
static RING_BUFFER: OnceLock<Arc<Mutex<RingBuf>>> = OnceLock::new();

/// Set to true to cancel an in-progress recording. Polled by the VAD loop.
/// Reset to false when a new capture begins.
static CANCEL_FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
static FINISH_FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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

/// v0.1.225: gate for per-chunk VAD trace. Returns true when
/// `WEB2QML_VAD_DEBUG` is set to "1" or "true". Cached so the
/// env-var lookup happens once per process. Used by the VAD
/// loop to emit a line per evaluation showing rms/zcr/decision.
///
/// Diagnostic-only — leave off in normal operation.
fn vad_debug_enabled() -> bool {
    use std::sync::OnceLock;
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        match std::env::var("WEB2QML_VAD_DEBUG") {
            Ok(v) if v == "1" || v.eq_ignore_ascii_case("true") => {
                crate::clog!("vad-debug", "enabled — per-chunk VAD trace will spam the log");
                true
            }
            _ => false,
        }
    })
}

#[cfg(feature = "voice")]
#[allow(dead_code)] // future preroll capture work
const PREROLL_MS: usize = 700;  // capture this much audio BEFORE the button press
#[cfg(feature = "voice")]
const MAX_RECORD_S: usize = 6;
#[cfg(feature = "voice")]
const TARGET_RATE: u32 = 16000;

#[cfg(feature = "voice")]
struct RingBuf {
    /// Most recent samples, capped at MAX_RECORD_S * TARGET_RATE
    samples: VecDeque<f32>,
    /// Source sample rate (capture native, we resample on extraction)
    source_rate: u32,
    /// Source channel count (we average to mono)
    source_channels: usize,
    /// Are we actively capturing into this buffer?
    capturing: bool,
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
    fn drain_into_mono(&mut self) -> Vec<f32> {
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

/// Two whisper contexts loaded in parallel when both model files are
/// present. We dispatch each transcription request to BOTH simultaneously
/// in worker threads, then choose the result via the policy in
/// `transcribe_with_dual_model`:
///
///   - If only one is loaded, use that.
///   - If both: wait for tiny first (always faster). When tiny returns,
///     give base up to 1s more. If base returns within that window, use
///     base's result (more accurate). Otherwise use tiny's and let base
///     finish in the background.
///
/// Memory cost when both loaded: ~220MB for fp16, ~88MB for quantized.
/// Acceptable on desktop, expensive on mobile — mobile usually only has
/// tiny installed via setup-voice.sh, so only tiny gets loaded.
#[cfg(feature = "voice")]
static TINY_CTX: OnceLock<Mutex<Option<Arc<whisper_rs::WhisperContext>>>> = OnceLock::new();
#[cfg(feature = "voice")]
static BASE_CTX: OnceLock<Mutex<Option<Arc<whisper_rs::WhisperContext>>>> = OnceLock::new();
/// Optional third tier for systems fast enough to run a Large model in
/// the grace window. ggml-large-v3-turbo is the practical candidate
/// (~6x faster than vanilla large-v3 thanks to reduced decoder layers).
/// Loaded only if a matching file exists in the model dir; absent on
/// most systems including all phones.
///
/// The dispatcher tries HighAccuracy first when picking results, but
/// it falls back to Base or Tiny if HighAccuracy hasn't returned within
/// the grace window. So loading it on a slow system doesn't block —
/// it just gets skipped at decision time. The wasted inference cost
/// is the price of the optionality.
#[cfg(feature = "voice")]
static LARGE_CTX: OnceLock<Mutex<Option<Arc<whisper_rs::WhisperContext>>>> = OnceLock::new();

// ─── Per-tier WhisperState caches ───
//
// whisper-rs 0.16's WhisperState owns its data (no borrow on
// WhisperContext at the API level), so we can cache it across calls.
// This avoids the ~210MB allocation churn (kv_self + kv_cross +
// compute_buffer_encode + compute_buffer_decode) that happens on
// every state creation — visible in the user's logs as a wall of
// `whisper_init_state:` lines on every transcribe.
//
// Pattern: Mutex<Option<WhisperState>>. On call:
//   1. Take it out of the Option (.take())
//   2. If None (first call), create a new one
//   3. Run inference
//   4. Put it back (.replace(state))
//
// The Mutex serializes calls per tier. Foreground + bg-listen are
// already serialized at a higher level (bg-listen skips while
// foreground is recording), so contention is rare.
//
// Memory cost: keeping state alive between calls keeps the ~210MB
// of compute buffers resident. That's RAM we're spending to save
// 100-300ms per call. Worth it on any machine with > 1GB free.
//
// State is reset implicitly by each .full() call — whisper-rs
// clears segments and KV state at the start of inference.
#[cfg(feature = "voice")]
static TINY_STATE: OnceLock<Mutex<Option<whisper_rs::WhisperState>>> = OnceLock::new();
#[cfg(feature = "voice")]
static BASE_STATE: OnceLock<Mutex<Option<whisper_rs::WhisperState>>> = OnceLock::new();
#[cfg(feature = "voice")]
static LARGE_STATE: OnceLock<Mutex<Option<whisper_rs::WhisperState>>> = OnceLock::new();

/// Dedicated WhisperState for streaming preview (v0.1.116).
///
/// The streaming preview runs Tiny on growing audio prefixes every
/// ~1500ms during recording. If it shared the foreground TINY_STATE,
/// it would lock-contend with voice_listen_dual at end-of-utterance.
/// Allocating a separate state lets preview run concurrently with
/// foreground inference using the SAME underlying WhisperContext
/// (which is Arc-shared and thread-safe).
///
/// Memory cost: ~210MB extra. Only allocated lazily on first use,
/// so users who never preview (no recording) don't pay.
#[cfg(feature = "voice")]
static PREVIEW_STATE: OnceLock<Mutex<Option<whisper_rs::WhisperState>>> = OnceLock::new();

// ── Streaming preview (v0.1.108) ──
//
// While the user is recording (capturing == true), a worker thread
// polls the ring buffer every 250ms and runs the dumb-DB phonetic
// match against current audio. The best match (if any) is written
// to PARTIAL_PREVIEW so QML can surface it as a grey "preview"
// string under the listening indicator.
//
// This is the "streaming preview" capability without requiring a
// real whisper-streaming API. The dumb-DB only knows about utterances
// the user has voiced before — but that's also the most likely
// next utterance. New utterances show empty preview until the real
// transcription lands. That's fine; preview is opportunistic, not
// authoritative.
//
// Cost: 250ms × ~10ms phonetic match = 4% CPU on a single core
// during recording. Negligible.
//
// Threading: ONE preview thread is spawned on first begin_capture
// (via PREVIEW_THREAD OnceLock). It loops forever, checking
// CAPTURE_PHASE atomically. On capturing=true it does work; on
// false it sleeps. This avoids respawning a thread per recording.
#[cfg(feature = "voice")]
pub static PARTIAL_PREVIEW: Mutex<String> = Mutex::new(String::new());

/// Structured preview words (v0.1.117): per-word entries carrying
/// speaker tag and arrival time. The live multicolor view reads
/// from this — each word renders in the speaker's registered color
/// with a fade-in animation timed from `arrived_at_ms`.
///
/// Why two preview surfaces (string AND words)?
/// - Backwards compat: existing QML reads PARTIAL_PREVIEW as a
///   single-line string; that path still works for users with no
///   speakers registered.
/// - New richer view: PARTIAL_PREVIEW_WORDS is populated by the
///   whisper streaming tier, tagged with whoever speakers::identify()
///   matched at the time of inference.
///
/// Both surfaces are updated by the same preview worker. The string
/// is the joined text (for the legacy renderer); the words list is
/// the structured form (for the multicolor renderer).
#[cfg(feature = "voice")]
pub struct PreviewWord {
    pub text: String,
    /// Registered speaker name (or "" when none identified yet).
    pub speaker: String,
    /// Hex color from the speaker registry (or "" for default).
    pub color: String,
    /// Milliseconds since UNIX epoch when this word was first
    /// emitted (the moment it appeared in the preview). UI uses
    /// this to anchor fade-in animations.
    pub arrived_at_ms: u64,
    /// v0.1.128: milliseconds since UNIX epoch when the AUDIO that
    /// produced this word STARTED being recorded. This is the
    /// chronological anchor used for ordering across speakers — so
    /// Alice's slow-transcribing utterance still sits before Bob's
    /// faster-transcribing reply, even if Bob's finalize came back
    /// first. Zero for words from the cumulative streaming tier
    /// (no per-utterance start time tracked there); those keep
    /// arrival-order behavior.
    pub audio_start_ms: u64,
    /// v0.1.131: speaker was muted (allowed=false) at the time of
    /// transcription. UI renders these dimmer with strikethrough
    /// or italic so the user sees background chatter without it
    /// driving commands. The audio still went through whisper but
    /// the intent dispatch path ignores muted utterances entirely.
    pub muted: bool,
}

#[cfg(feature = "voice")]
pub static PARTIAL_PREVIEW_WORDS: Mutex<Vec<PreviewWord>> = Mutex::new(Vec::new());

#[cfg(feature = "voice")]
static PREVIEW_THREAD_STARTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Read the current partial preview text. QML polls this via
/// backend.voice_partial_preview() every ~150-250ms while the
/// listening UI is visible. Returns empty string when nothing is
/// matched yet (first ~250ms of capture, or unrecognized utterance).
#[cfg(feature = "voice")]
pub fn get_partial_preview() -> String {
    PARTIAL_PREVIEW.lock().map(|g| g.clone()).unwrap_or_default()
}

#[cfg(not(feature = "voice"))]
pub fn get_partial_preview() -> String { String::new() }

/// v0.1.713: snapshot the most recent mic audio for the acoustic-handshake
/// LISTEN side, resampled to `target_rate` (the modem's 8 kHz). Returns mono
/// f32 samples. The foreground RING_BUFFER must be capturing (the handshake UI
/// arms it like a voice recording). Empty when not capturing / no audio.
/// RUNTIME-UNTESTED in-sandbox (no audio device); the resample math is simple.
#[cfg(feature = "voice")]
pub fn capture_modem_samples(target_rate: u32) -> Vec<f32> {
    let (mono, src_rate) = match RING_BUFFER.get() {
        Some(arc) => match arc.lock() {
            Ok(g) => (g.snapshot_mono(), g.source_rate),
            Err(_) => return Vec::new(),
        },
        None => return Vec::new(),
    };
    if mono.is_empty() || src_rate == 0 { return Vec::new(); }
    if src_rate == target_rate { return mono; }
    // linear resample src_rate → target_rate
    let ratio = src_rate as f64 / target_rate as f64;
    let out_len = (mono.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let s = i as f64 * ratio;
        let idx = s as usize;
        if idx >= mono.len() { break; }
        let frac = (s - idx as f64) as f32;
        let next = (idx + 1).min(mono.len() - 1);
        out.push(mono[idx] * (1.0 - frac) + mono[next] * frac);
    }
    out
}

#[cfg(not(feature = "voice"))]
pub fn capture_modem_samples(_target_rate: u32) -> Vec<f32> { Vec::new() }

/// v0.1.714: compute a speaker embedding from raw audio samples — the
/// voiceprint used in the peer handshake's identity step. samples → MFCC
/// (phonetic) → embedding (speaker_embed). Empty when audio is too short.
#[cfg(feature = "voice")]
pub fn embed_samples(samples: &[f32]) -> Vec<f32> {
    let mfcc = crate::phonetic::compute_mfcc(samples);
    crate::peer_handshake::embed_mfcc(&mfcc)
}

#[cfg(not(feature = "voice"))]
pub fn embed_samples(_samples: &[f32]) -> Vec<f32> { Vec::new() }

/// Read the structured preview as a serialized string for QML.
/// Format: each word on its own line, fields tab-separated:
///   text\tspeaker\tcolor\tarrived_ms\taudio_start_ms\tmuted
/// QML currently consumes 4 fields; 5th (audio_start_ms) and 6th
/// (muted "1"/"0") are harmless additions for existing parsers
/// and used by the new muted-background channel renderer.
#[cfg(feature = "voice")]
pub fn get_partial_preview_words() -> String {
    let words = match PARTIAL_PREVIEW_WORDS.lock() {
        Ok(g) => g,
        Err(_) => return String::new(),
    };
    let mut out = String::with_capacity(words.len() * 40);
    for (i, w) in words.iter().enumerate() {
        if i > 0 { out.push('\n'); }
        out.push_str(&w.text);
        out.push('\t');
        out.push_str(&w.speaker);
        out.push('\t');
        out.push_str(&w.color);
        out.push('\t');
        out.push_str(&w.arrived_at_ms.to_string());
        out.push('\t');
        out.push_str(&w.audio_start_ms.to_string());
        out.push('\t');
        out.push_str(if w.muted { "1" } else { "0" });
    }
    out
}

#[cfg(not(feature = "voice"))]
pub fn get_partial_preview_words() -> String { String::new() }

/// Replace the structured preview with a new transcript. v0.1.117.
///
/// `full_text` is the whole transcript whisper returned this pass
/// (whisper-streaming gives the FULL re-transcription each time,
/// not delta-only). We diff against the existing word list and
/// preserve `arrived_at_ms` for words that haven't changed —
/// otherwise every poll would reset the fade-in animation on
/// every word.
///
/// Diff strategy: prefix match. Tokenize on whitespace. Words that
/// match the start of the existing list keep their timestamps; new
/// words get fresh timestamps; words removed from the end drop out.
/// Mid-sequence changes (whisper hallucinated different words in
/// the middle) cause re-arrival of everything from divergence point.
#[cfg(feature = "voice")]
pub fn update_partial_preview_words(full_text: &str, speaker: &str, color: &str) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let new_tokens: Vec<&str> = full_text
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .collect();
    if new_tokens.is_empty() {
        if let Ok(mut g) = PARTIAL_PREVIEW_WORDS.lock() {
            g.clear();
        }
        return;
    }

    if let Ok(mut g) = PARTIAL_PREVIEW_WORDS.lock() {
        // Find divergence point: longest common prefix of words
        // where text matches AND speaker matches. We require speaker
        // match too because the SAME text from a different speaker
        // should re-animate (visual cue of speaker change).
        let mut common = 0usize;
        for (i, tok) in new_tokens.iter().enumerate() {
            match g.get(i) {
                Some(w) if w.text == *tok && w.speaker == speaker => {
                    common = i + 1;
                }
                _ => break,
            }
        }
        // Truncate beyond divergence; append new with fresh timestamps.
        g.truncate(common);
        for tok in new_tokens.iter().skip(common) {
            g.push(PreviewWord {
                text: tok.to_string(),
                speaker: speaker.to_string(),
                color: color.to_string(),
                arrived_at_ms: now_ms,
                // Cumulative streaming tier doesn't track per-
                // utterance audio start; ordering for these falls
                // through to arrival order (they're added in
                // arrival order anyway). 0 means "no chronological
                // anchor"; the insertion logic treats 0 as
                // unordered = append.
                audio_start_ms: 0,
                muted: false,
            });
        }
    }
}

/// Replace this speaker's words with the given finalized transcript,
/// inserted in chronological order based on `audio_start_ms`. v0.1.128.
///
/// Strategy:
///   - Find the contiguous block of words for this speaker in the
///     existing list. (All this speaker's words sit together at a
///     single insertion point — set by the previous call OR by
///     chronological insert.)
///   - Diff prefix-match the existing block against new tokens:
///     words that match keep their arrived_at_ms (no re-fade).
///     Words past divergence get fresh timestamps.
///   - If the speaker has NO existing block, find chronological
///     insertion point and splice the whole thing in.
///
/// Why diff-based: when whisper updates its transcript pass-by-pass
/// (e.g. "show fil" → "show files" → "show files now"), the old
/// strategy would fade every word in fresh on every update —
/// visually distracting. Keeping matching prefix means only NEW
/// or CHANGED tokens animate.
///
/// Words with `audio_start_ms == 0` (cumulative streaming tier)
/// are NOT used as ordering targets. This preserves arrival-order
/// behavior for those.
#[cfg(feature = "voice")]
pub fn replace_speaker_words(
    full_text: &str,
    speaker: &str,
    color: &str,
    audio_start_ms: u64,
    muted: bool,
) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let new_tokens: Vec<String> = full_text
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    if let Ok(mut g) = PARTIAL_PREVIEW_WORDS.lock() {
        // Find the contiguous span of THIS speaker's existing words.
        // Assumes they sit together — true by construction (each
        // replace_speaker_words drops all of theirs and re-splices
        // at one site).
        let speaker_first = g.iter().position(|w| w.speaker == speaker);
        let speaker_last = g.iter().rposition(|w| w.speaker == speaker);

        // Diff: compare existing tokens against new tokens. Keep
        // matching prefix; replace only the divergent tail.
        let (insert_at, kept_words): (usize, Vec<PreviewWord>) = match (speaker_first, speaker_last) {
            (Some(first), Some(last)) => {
                // Existing block exists. Walk the prefix.
                let mut kept: Vec<PreviewWord> = Vec::new();
                for (i, tok) in new_tokens.iter().enumerate() {
                    let idx = first + i;
                    if idx > last { break; }
                    let existing = &g[idx];
                    if existing.text == *tok {
                        // Match — preserve original arrived_at_ms so
                        // no re-fade.
                        kept.push(PreviewWord {
                            text: existing.text.clone(),
                            speaker: existing.speaker.clone(),
                            color: existing.color.clone(),
                            arrived_at_ms: existing.arrived_at_ms,
                            audio_start_ms: existing.audio_start_ms,
                            muted: existing.muted,
                        });
                    } else {
                        break;
                    }
                }
                // Drop the entire existing block. We'll splice
                // kept + fresh tail back in.
                g.drain(first..=last);
                (first, kept)
            }
            _ => {
                // No existing block. Find chronological insertion
                // point as a function of audio_start_ms.
                let insert_at = if audio_start_ms == 0 {
                    g.len()
                } else {
                    g.iter()
                        .position(|w| w.audio_start_ms > 0 && w.audio_start_ms > audio_start_ms)
                        .unwrap_or(g.len())
                };
                (insert_at, Vec::new())
            }
        };

        // Build full word list: kept prefix + fresh tail.
        let kept_len = kept_words.len();
        let mut combined: Vec<PreviewWord> = kept_words;
        for tok in new_tokens.iter().skip(kept_len) {
            combined.push(PreviewWord {
                text: tok.clone(),
                speaker: speaker.to_string(),
                color: color.to_string(),
                arrived_at_ms: now_ms,
                audio_start_ms,
                muted,
            });
        }

        // Splice in.
        g.splice(insert_at..insert_at, combined);
    }
}

/// Clear the partial preview. Called at begin_capture() so leftovers
/// from previous recordings don't flash on screen.
#[cfg(feature = "voice")]
pub fn clear_partial_preview() {
    if let Ok(mut g) = PARTIAL_PREVIEW.lock() {
        g.clear();
    }
    if let Ok(mut g) = PARTIAL_PREVIEW_WORDS.lock() {
        g.clear();
    }
}

/// Start the preview worker thread (idempotent). Called from
/// begin_capture so the thread comes up at first record but stays
/// dormant when not recording.
///
/// v0.1.116: TWO-TIER preview. The fast tier runs every 250ms doing
/// a dumb-DB phonetic match (catches known utterances in <10ms; gives
/// users immediate visual feedback). The slow tier runs every 1500ms
/// doing REAL whisper transcription on the growing buffer (catches
/// novel utterances with actual text). Whisper output supersedes
/// dumb-DB output when available.
#[cfg(feature = "voice")]
fn ensure_preview_thread() {
    use std::sync::atomic::Ordering;
    if PREVIEW_THREAD_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        crate::clog!("voice-preview", "worker started (two-tier: phonetic 250ms + whisper 1500ms + separator + per-speaker routing)");
        let mut last_whisper = std::time::Instant::now() - std::time::Duration::from_secs(10);
        let mut last_whisper_len = 0usize;
        // v0.1.119: per-chunk speaker separation.
        // v0.1.120: per-speaker audio routing. Each registered speaker
        // gets their own accumulator buffer. As chunks classify, audio
        // routes to the matching speaker's buffer. When a speaker goes
        // silent for ~800ms, we submit their accumulated audio to the
        // whisper_pool with their name as a tag. Pool result comes back
        // tagged; we merge into the multicolor view.
        //
        // Reset on each new recording (when capturing flips false→true).
        let mut separator = crate::separation::Separator::new();
        let mut was_capturing = false;
        let mut last_chunk_offset = 0usize;
        // Per-speaker accumulators. Keyed by speaker name.
        // Value: (audio_buffer, ms_since_last_speech_chunk, audio_start_ms)
        // where audio_start_ms is the wall-clock instant the first
        // chunk for this utterance was recorded. Threaded to the
        // pool job so the preview consumer can insert words in
        // chronological order across speakers. Submit + clear when
        // ms_since_last_speech >= 800ms AND buffer is non-empty.
        let mut spk_buffers: std::collections::HashMap<String, (Vec<f32>, u64, u64)> =
            std::collections::HashMap::new();
        // Monotonic counter for whisper_pool job generation. Bumped on
        // each new recording so stale results from prior utterance
        // can be filtered (though we don't filter today — pool results
        // are best-effort current).
        let mut current_generation: u64 = 0;
        // Speaker silence threshold (ms) before we submit their
        // accumulated buffer for finalized transcription.
        const PER_SPEAKER_SILENCE_MS: u64 = 800;
        // Minimum audio length (samples at 16kHz) before we bother
        // submitting. 8000 samples = 500ms — anything shorter is
        // probably too brief for useful transcription.
        const MIN_SUBMIT_SAMPLES: usize = 8000;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(
                phonetic_interval_ms()));
            // Read capturing state via the ring buffer's flag.
            let (capturing, snap, src_rate) = match RING_BUFFER.get() {
                Some(ring) => match ring.lock() {
                    Ok(g) => {
                        if !g.capturing {
                            (false, Vec::new(), TARGET_RATE)
                        } else if g.samples.len() < 4000 {
                            // Too short — phonetic match is unreliable below
                            // ~150ms. Wait for more audio.
                            (true, Vec::new(), TARGET_RATE)
                        } else {
                            (true, g.snapshot_mono(), g.source_rate)
                        }
                    }
                    Err(_) => (false, Vec::new(), TARGET_RATE),
                },
                None => (false, Vec::new(), TARGET_RATE),
            };
            if !capturing {
                // Not recording — clear preview if it has stale content,
                // and reset whisper cadence + separator state.
                if let Ok(mut g) = PARTIAL_PREVIEW.lock() {
                    if !g.is_empty() { g.clear(); }
                }
                last_whisper_len = 0;
                last_whisper = std::time::Instant::now() - std::time::Duration::from_secs(10);
                if was_capturing {
                    // Recording just ended — reset separator for the
                    // next utterance and clear active-speaker UI.
                    // Also discard any partial per-speaker buffers
                    // (they'll be redundant: the foreground voice
                    // path is finalizing through voice_listen_dual
                    // which uses the cumulative audio).
                    separator = crate::separation::Separator::new();
                    last_chunk_offset = 0;
                    spk_buffers.clear();
                    was_capturing = false;
                }
                continue;
            }
            if !was_capturing {
                // Recording just started — fresh separator + buffers.
                separator = crate::separation::Separator::new();
                last_chunk_offset = 0;
                spk_buffers.clear();
                current_generation = current_generation.wrapping_add(1);
                was_capturing = true;
            }
            if snap.is_empty() {
                continue;
            }
            // Resample to 16kHz (shared between phonetic, whisper, separator).
            let audio_16k = if src_rate == TARGET_RATE {
                snap
            } else {
                resample(&snap, src_rate, TARGET_RATE)
            };

            // ── Per-chunk speaker separation (v0.1.119) ──
            // Take new audio since last chunk classified. If we have
            // at least 8000 new samples (~500ms at 16kHz), classify
            // that window. Otherwise skip — the next tick will
            // accumulate more.
            //
            // This gives us per-chunk speaker tracking with ~500ms
            // resolution. Cheap: TonalProfile computation + registry
            // scan. The Separator's internal hysteresis (3 consecutive
            // matching decisions) means active_speaker only changes
            // after ~1.5s of consistent signal — prevents flicker
            // from per-chunk pitch noise.
            // v0.1.122 perf: skip per-chunk separator + per-speaker
            // routing entirely when registry is empty. The Separator
            // would always return Unknown anyway (nobody to match
            // against) and the per-speaker buffer routing has nowhere
            // to route to. Saves ~3-5ms per 250ms tick + heap churn.
            //
            // The cumulative whisper streaming below still runs —
            // it doesn't depend on the registry.
            //
            // v0.1.288: tightened from `!is_empty()` (count > 0) to
            // `count > 1`. With a single registered speaker, there's
            // no disambiguation work to do — the per-speaker
            // streaming whisper pass duplicates what the foreground
            // voice_listen_dual produces as the authoritative
            // transcript. Mobile CPU is constrained; user reported
            // "whisper is working on mobile but it is getting choked
            // up somehow." This skip should help.
            let multi_user_active = crate::speakers::count() > 1;
            let new_len = audio_16k.len();
            if multi_user_active && new_len >= last_chunk_offset + 8000 {
                let chunk_start = last_chunk_offset;
                let chunk_end = chunk_start + 8000;
                let chunk = &audio_16k[chunk_start..chunk_end];
                let prev_active: Option<String> = separator.active_speaker().map(|s| s.to_string());
                let decision = separator.classify_chunk(chunk);
                let new_active: Option<String> = separator.active_speaker().map(|s| s.to_string());
                // v0.1.120: throttled logging. Log per-chunk decisions
                // only when active speaker CHANGES, or every 8 chunks
                // (~4s) as a heartbeat to confirm classification is
                // still running. Avoids the per-tick spam reported
                // in v0.1.119.
                use std::sync::atomic::{AtomicU32, Ordering};
                static CHUNK_LOG_COUNT: AtomicU32 = AtomicU32::new(0);
                let n = CHUNK_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
                if prev_active != new_active || n % 8 == 0 {
                    crate::clog!("voice-preview", "chunk {}-{}: {:?} active={:?}",
                        chunk_start, chunk_end, decision, new_active);
                }
                last_chunk_offset = chunk_end;
                // Surface the active speaker so QML circles pulse
                // immediately (instead of waiting for whisper).
                match separator.active_speaker() {
                    Some(name) => {
                        if crate::speakers::is_allowed(name) {
                            crate::speakers::note_active(name);
                        } else {
                            crate::speakers::clear_active();
                        }
                    }
                    None => {
                        // No confirmed active yet — leave whatever
                        // was previously active (don't clear during
                        // a single utterance just because hysteresis
                        // is still warming up).
                    }
                }

                // ── Per-speaker audio routing (v0.1.120) ──
                //
                // Route this chunk into the active speaker's
                // accumulator if any. On Silence, increment the
                // silence counter for ALL speakers that have a
                // buffered utterance pending. When silence exceeds
                // PER_SPEAKER_SILENCE_MS, finalize that speaker's
                // utterance: submit to whisper_pool with the
                // speaker name as tag.
                //
                // This is a heuristic, not true source separation.
                // Audio in this chunk is the AVERAGED mix of
                // everyone present; we attribute it to the
                // dominant speaker (per Separator) and that's it.
                // When two people talk simultaneously, the
                // dominant one captures both — accepted limit.
                //
                // The cumulative whisper streaming continues to
                // run in parallel below. Per-speaker finalized
                // results arrive later (after speaker silence
                // detected) and REPLACE the cumulative-attributed
                // words for that speaker via update_partial_preview_words.
                let chunk_duration_ms = 500u64; // 8000 samples @ 16kHz
                match &decision {
                    crate::separation::Decision::Speaker { name, .. } => {
                        // v0.1.133: deep mute skips the speaker
                        // entirely. Audio dropped at the routing
                        // stage — no buffer entry, no whisper job,
                        // no transcript. Different from regular
                        // (background) mute which still transcribes
                        // for display dim. Long-press the speaker
                        // circle toggles between background mute
                        // and deep mute (when allowed=false).
                        //
                        // We still tick the silence counter for
                        // OTHER speakers' pending buffers below so
                        // their utterances finalize on schedule —
                        // the deep-muted speaker simply gets no
                        // buffer slot and is treated like noise.
                        let is_deep = crate::speakers::is_deep_muted(name);
                        if !is_deep {
                            // v0.1.131: route muted speakers' audio to the
                            // pipeline too. Previously their chunks were
                            // dropped on the floor (the is_allowed gate
                            // skipped the .entry insert). The user asked
                            // for a "background channel" — see them
                            // talking without their commands driving the
                            // app. Audio goes to the same per-speaker
                            // buffer; at finalize time we tag the job with
                            // muted=is_allowed(name)==false so the result
                            // renders dim and the dispatch path ignores it.
                            let now_ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0);
                            let entry = spk_buffers
                                .entry(name.clone())
                                .or_insert_with(|| (Vec::with_capacity(48000), 0u64, now_ms));
                            entry.0.extend_from_slice(chunk);
                            entry.1 = 0; // reset silence counter
                            // audio_start_ms (entry.2) intentionally
                            // NOT updated on subsequent chunks — it
                            // anchors the START of the utterance.
                        }

                        // For all OTHER speakers with pending buffers,
                        // their silence counter still ticks (the chunk
                        // wasn't theirs). Also ticks when this speaker
                        // is deep-muted — the deep-muted speaker's
                        // chunk isn't anyone else's, so it's effectively
                        // silence from their perspective.
                        for (other, (_buf, silence_ms, _start)) in spk_buffers.iter_mut() {
                            if other != name {
                                *silence_ms += chunk_duration_ms;
                            }
                        }
                    }
                    crate::separation::Decision::Silence => {
                        // True silence — increment silence counter
                        // for everyone with a pending buffer.
                        for (_name, (_buf, silence_ms, _start)) in spk_buffers.iter_mut() {
                            *silence_ms += chunk_duration_ms;
                        }
                    }
                    crate::separation::Decision::Unknown { .. } => {
                        // Audio detected but no registered speaker
                        // matched. Don't route to any buffer; treat
                        // like silence for the purposes of triggering
                        // finalization (so a registered speaker who
                        // stops while an unknown person continues
                        // still gets their utterance finalized).
                        for (_name, (_buf, silence_ms, _start)) in spk_buffers.iter_mut() {
                            *silence_ms += chunk_duration_ms;
                        }
                    }
                }

                // Check for finalization candidates: speakers whose
                // silence counter exceeded threshold AND buffer big
                // enough to bother transcribing.
                let mut to_finalize: Vec<String> = Vec::new();
                for (name, (buf, silence_ms, _start)) in spk_buffers.iter() {
                    if *silence_ms >= PER_SPEAKER_SILENCE_MS && buf.len() >= MIN_SUBMIT_SAMPLES {
                        to_finalize.push(name.clone());
                    }
                }
                // v0.1.368 (PA2 prereq): update the public MIN_SILENCE_MS
                // atomic so reading-overlay and future tier-2 classifier
                // can observe silence dynamics. Min across active buffers;
                // u32::MAX when no buffer is active. Cheap — one atomic
                // store per voice tick.
                {
                    let mut min_silence: u32 = u32::MAX;
                    for (_n, (_b, s, _st)) in spk_buffers.iter() {
                        // spk_buffers stores silence as u64 (loop-tick
                        // counter accumulated since v0.1.371's calibration
                        // work); MIN_SILENCE_MS is AtomicU32 to match the
                        // bucket comparisons in main.rs. Saturating cast
                        // is safe — silence values stay well under 2^32
                        // ms (about 49 days; we reset on speaker activity).
                        let s_u32 = (*s).min(u32::MAX as u64) as u32;
                        if s_u32 < min_silence {
                            min_silence = s_u32;
                        }
                    }
                    MIN_SILENCE_MS.store(min_silence,
                        std::sync::atomic::Ordering::Relaxed);
                }
                for name in to_finalize {
                    if let Some((buf, _, audio_start_ms)) = spk_buffers.remove(&name) {
                        // v0.1.131: check current allowed status at
                        // finalize time. The Job carries this through
                        // to the result so the preview renders muted
                        // utterances in the background channel.
                        let is_muted = !crate::speakers::is_allowed(&name);
                        let job = crate::whisper_pool::Job {
                            audio: buf,
                            speaker: name.clone(),
                            generation: current_generation,
                            submitted_at: std::time::Instant::now(),
                            audio_start_ms,
                            muted: is_muted,
                        };
                        match crate::whisper_pool::submit(job) {
                            Ok(()) => {
                                crate::clog!("voice-preview", "submitted per-speaker job for {:?}",
                                    name);
                            }
                            Err(e) => {
                                crate::clog!("voice-preview", "pool submit failed for {:?}: {}",
                                    name, e);
                            }
                        }
                    }
                }
            }

            // ── Drain whisper_pool results (v0.1.120) ──
            //
            // Per-speaker finalized transcripts arrive here. Replace
            // any cumulative-attributed words for that speaker with
            // the finalized version (more accurate — whisper had
            // the whole utterance, not just the buffer state at
            // a streaming-pass moment).
            //
            // Currently merge strategy: full replace per speaker.
            // We strip existing words tagged with this speaker
            // from PARTIAL_PREVIEW_WORDS, then append the finalized
            // tokens. New arrival_at_ms so fade-in animates the
            // replacement.
            let results = crate::whisper_pool::drain_results();
            for r in results {
                if r.generation != current_generation {
                    // Stale — utterance ended, this is from prior recording.
                    crate::clog!("voice-preview", "dropping stale pool result gen={} cur={}",
                        r.generation, current_generation);
                    continue;
                }
                if r.text.trim().is_empty() {
                    continue;
                }
                let color = crate::speakers::color_for(&r.speaker);
                crate::clog!("voice-preview", "pool result speaker={:?} text={:?} ({}ms)",
                    r.speaker,
                    r.text.chars().take(60).collect::<String>(),
                    r.took_ms);

                // v0.1.137: roll cadence stats for the identified speaker.
                // We have word timings from whisper if set_token_timestamps
                // was enabled. Compute syllables_per_sec (proxy:
                // word_count * 1.4 / utterance_duration) and mean inter-word
                // pause in ms. Only update for non-muted, non-empty
                // speakers — otherwise we'd poison the EMA with placeholder
                // data or with speech that didn't really come from this
                // person.
                //
                // Pure data collection. Nothing reads these values for
                // matching yet. Inspectable via speaker_cadence qt_method
                // + Settings debug toggle.
                if !r.muted && !r.speaker.is_empty() {
                    if let Some(ref words) = r.word_timings {
                        if words.len() >= 2 {
                            // Total duration: first word's start to last word's end.
                            let dur_ms = words.last().map(|w| w.t1_ms).unwrap_or(0)
                                .saturating_sub(words.first().map(|w| w.t0_ms).unwrap_or(0));
                            if dur_ms > 200 {  // ignore degenerate utterances
                                let word_count = words.len() as f32;
                                let syl_per_sec = (word_count * 1.4) / (dur_ms as f32 / 1000.0);
                                // Mean inter-word pause: gaps between consecutive
                                // words (w[i+1].t0 - w[i].t1). Negative gaps clamp
                                // to 0 (segment-level timing can produce overlap).
                                let mut pause_sum: u32 = 0;
                                let mut pause_count: u32 = 0;
                                for w in words.windows(2) {
                                    let gap = w[1].t0_ms.saturating_sub(w[0].t1_ms);
                                    if gap < 2000 {  // cap pathological gaps
                                        pause_sum += gap;
                                        pause_count += 1;
                                    }
                                }
                                let mean_pause = if pause_count > 0 {
                                    pause_sum as f32 / pause_count as f32
                                } else {
                                    0.0
                                };
                                crate::speakers::update_cadence(&r.speaker, syl_per_sec, mean_pause);
                            }
                        }
                    }
                }

                // Replace this speaker's words in PARTIAL_PREVIEW_WORDS
                // with the finalized transcript. Other speakers'
                // words are left alone. v0.1.128: insertion is now
                // chronological based on audio_start_ms — the moment
                // this speaker's utterance started being recorded —
                // so out-of-order finalize doesn't scramble display
                // order.
                replace_speaker_words(&r.text, &r.speaker, &color, r.audio_start_ms, r.muted);
            }

            // ── Tier 1: dumb-DB phonetic match (250ms cadence) ──
            // Always run. Cheap (~10ms). If we already have a fresh
            // whisper preview, the phonetic match doesn't overwrite —
            // it only fills the slot when whisper hasn't produced
            // anything yet.
            let hint = crate::audio_profile::pitch_hint();
            let matches = crate::phonetic::top_matches_with_hint(&audio_16k, 1, hint);
            if let Some((label, score)) = matches.first() {
                if *score >= 0.55 && !label.is_empty() {
                    if let Ok(mut g) = PARTIAL_PREVIEW.lock() {
                        // Only overwrite if currently empty OR what's
                        // there is itself a phonetic match (prefix "≈ ").
                        // This way a whisper transcript "stays" once
                        // it lands, even if phonetic keeps cycling.
                        let starts_with_approx = g.starts_with("≈ ");
                        if g.is_empty() || starts_with_approx {
                            let new_val = format!("≈ {}", label);
                            if g.as_str() != new_val.as_str() {
                                *g = new_val;
                            }
                        }
                    }
                }
            }

            // ── Tier 2: real whisper streaming (1500ms cadence) ──
            // Only runs when:
            //   - At least 1500ms since last whisper call (or first call)
            //   - Audio buffer has grown by at least 8000 samples
            //     (~500ms) since last whisper. Re-running on
            //     unchanged audio just produces the same transcript
            //     with extra latency.
            //   - PREVIEW_STATE lock available (try_lock; skip if busy)
            //
            // The transcript replaces PARTIAL_PREVIEW unconditionally
            // (whisper beats phonetic when both available).
            let now = std::time::Instant::now();
            let buf_grew = audio_16k.len() >= last_whisper_len + 8000;
            if buf_grew && now.duration_since(last_whisper).as_millis() as u64
                >= whisper_interval_ms()
            {
                // Capture audio length up-front so we can record it
                // even if the whisper call fails or skips.
                let attempted_len = audio_16k.len();
                if let Some(tiny_arc) = TINY_CTX.get()
                    .and_then(|m| m.lock().ok().and_then(|g| g.clone()))
                {
                    // Try to grab a state for preview. Lazy-init.
                    let state_cell = PREVIEW_STATE.get_or_init(|| Mutex::new(None));
                    if let Ok(mut sguard) = state_cell.try_lock() {
                        let mut state = match sguard.take() {
                            Some(s) => s,
                            None => {
                                // Lazy init — only on first call.
                                match tiny_arc.create_state() {
                                    Ok(s) => s,
                                    Err(e) => {
                                        crate::clog!("voice-preview", "state init failed: {:?}", e);
                                        last_whisper = now;
                                        continue;
                                    }
                                }
                            }
                        };
                        // Run a fast whisper pass.
                        let mut params = whisper_rs::FullParams::new(
                            whisper_rs::SamplingStrategy::Greedy { best_of: 1 }
                        );
                        params.set_n_threads(2);  // fewer threads — share with foreground
                        params.set_print_progress(false);
                        params.set_print_realtime(false);
                        params.set_print_timestamps(false);
                        params.set_no_context(true);
                        params.set_single_segment(true);
                        params.set_suppress_nst(true);
                        // Language from voice_lang config (defaults "en")
                        params.set_language(crate::voice_lang::whisper_lang_code());
                        let start = std::time::Instant::now();
                        let result = state.full(params, &audio_16k);
                        let took_ms = start.elapsed().as_millis();
                        // Always return the state to the cache, even on err.
                        *sguard = Some(state);
                        match result {
                            Ok(_) => {
                                let n_segs = sguard.as_ref()
                                    .map(|s| s.full_n_segments())
                                    .unwrap_or(0);
                                let text: String = (0..n_segs)
                                    .filter_map(|i| sguard.as_ref()
                                        .and_then(|s| s.get_segment(i)))
                                    .filter_map(|seg| seg.to_str().ok().map(|s| s.to_string()))
                                    .collect::<Vec<_>>()
                                    .join("");
                                let text = text.trim();
                                if !text.is_empty() {
                                    if let Ok(mut g) = PARTIAL_PREVIEW.lock() {
                                        let new_val = text.to_string();
                                        if g.as_str() != new_val.as_str() {
                                            *g = new_val;
                                        }
                                    }
                                    // v0.1.117: also update the
                                    // structured multicolor preview.
                                    // v0.1.119: prefer the separator's
                                    // active_speaker (per-chunk tracked
                                    // with hysteresis) over a fresh
                                    // identify() on the cumulative
                                    // buffer.
                                    //
                                    // v0.1.122 perf: skip speaker
                                    // tagging entirely when registry
                                    // is empty (the common solo-user
                                    // case). Saves a TonalProfile build
                                    // per whisper streaming pass.
                                    let (sp_name, sp_color) = if crate::speakers::is_empty() {
                                        (String::new(), String::new())
                                    } else {
                                        match separator.active_speaker() {
                                            Some(name) => {
                                                let color = crate::speakers::color_for(name);
                                                (name.to_string(), color)
                                            }
                                            None => {
                                                let profile = crate::phonetic::TonalProfile::from_audio(&audio_16k);
                                                match crate::speakers::identify(&profile) {
                                                    Some((name, _sim)) => {
                                                        let color = crate::speakers::color_for(&name);
                                                        (name, color)
                                                    }
                                                    None => (String::new(), String::new()),
                                                }
                                            }
                                        }
                                    };
                                    update_partial_preview_words(text, &sp_name, &sp_color);
                                    crate::clog!("voice-preview", "whisper streaming: {:?} ({}ms) speaker={:?}",
                                        text.chars().take(60).collect::<String>(),
                                        took_ms,
                                        if sp_name.is_empty() { "<unknown>" } else { sp_name.as_str() });
                                }
                                last_whisper_len = attempted_len;
                            }
                            Err(e) => {
                                crate::clog!("voice-preview", "whisper streaming err: {:?}", e);
                            }
                        }
                        last_whisper = now;
                    } else {
                        // Preview state busy (other thread holding it?).
                        // Skip this cycle.
                    }
                }
                // If TINY_CTX not loaded, fall back to phonetic-only —
                // already done above.
            }
        }
    });
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModelTier { Tiny, Base, HighAccuracy }

/// Resolve the model file path for the given tier. Prefers the quantized
/// variant if it exists (faster, same accuracy), else falls back to fp16.
/// Returns None if no file exists for this tier.
fn model_file(tier: ModelTier) -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var("HOME").unwrap_or("/tmp".into()))
        .join(".config/web2qml/models");
    // v0.1.652: language-aware model selection. The .en models are
    // English-ONLY — they ignore a non-"en" language request and
    // transcribe other languages phonetically as English (e.g. Spanish
    // "abre terminal" → "abreit terminal"). So when the configured
    // language isn't English, prefer the MULTILINGUAL variants (no .en
    // suffix); only fall back to .en with a warning, since it can't
    // actually transcribe the language.
    let non_english = crate::voice_lang::current() != "en";
    let candidates: &[&str] = match (tier, non_english) {
        (ModelTier::Tiny, false) => &["ggml-tiny.en-q5_1.bin", "ggml-tiny.en.bin"],
        (ModelTier::Tiny, true)  => &["ggml-tiny-q5_1.bin", "ggml-tiny.bin", "ggml-tiny.en-q5_1.bin", "ggml-tiny.en.bin"],
        (ModelTier::Base, false) => &["ggml-base.en-q5_1.bin", "ggml-base.en.bin"],
        (ModelTier::Base, true)  => &["ggml-base-q5_1.bin", "ggml-base.bin", "ggml-base.en-q5_1.bin", "ggml-base.en.bin"],
        // Large-v3-turbo is multilingual already (no .en variant).
        (ModelTier::HighAccuracy, _) => &[
            "ggml-large-v3-turbo-q5_0.bin",
            "ggml-large-v3-turbo.bin",
        ],
    };
    for name in candidates {
        let p = dir.join(name);
        if p.exists() {
            // warn if a non-English language ends up on an English-only model
            if non_english && name.contains(".en") {
                crate::clog!("whisper",
                    "language is '{}' but only an English-only model ({}) is available — \
                     it will transcribe {} phonetically as English. Install a multilingual \
                     model (e.g. ggml-base-q5_1.bin, no .en) for correct transcription.",
                    crate::voice_lang::current(), name, crate::voice_lang::current());
            }
            return Some(p);
        }
    }
    None
}

/// Honor WEB2QML_FORCE_MODEL=tiny|tiny.en|base|base.en. Returns Some(tier)
/// when the env var is set, None when no override is in effect.
fn forced_tier() -> Option<ModelTier> {
    let v = std::env::var("WEB2QML_FORCE_MODEL").ok()?;
    match v.as_str() {
        "tiny" | "tiny.en" => Some(ModelTier::Tiny),
        "base" | "base.en" => Some(ModelTier::Base),
        "large" | "large-v3-turbo" | "turbo" | "high" | "high-accuracy" =>
            Some(ModelTier::HighAccuracy),
        _ => None,
    }
}

/// Back-compat for system_info.rs / other call sites that just want SOMETHING
/// to point at. Returns the lowest tier that exists, falling back to base.
pub fn model_path() -> PathBuf {
    if let Some(t) = forced_tier() {
        if let Some(p) = model_file(t) { return p; }
    }
    if let Some(p) = model_file(ModelTier::Base) { return p; }
    if let Some(p) = model_file(ModelTier::Tiny) { return p; }
    // No file — return a guess so callers can format an error message.
    PathBuf::from(std::env::var("HOME").unwrap_or("/tmp".into()))
        .join(".config/web2qml/models/ggml-base.en.bin")
}

/// Check if voice input is available
pub fn is_available() -> bool {
    if cfg!(feature = "voice") && model_path().exists() {
        return true;
    }
    for tool in &["whisper-cpp", "whisper-cli", "nerd-dictation"] {
        if std::process::Command::new("which").arg(tool).output()
            .map(|o| o.status.success()).unwrap_or(false) {
            return true;
        }
    }
    false
}

/// Pre-warm everything: load model, start cpal stream in background.
/// Call this at app startup so first voice press is instant.
pub fn prewarm() {
    #[cfg(feature = "voice")]
    {
        // Load whisper model in background thread (~800ms)
        std::thread::spawn(|| {
            let _ = ensure_whisper_loaded();
        });
        // Start the persistent cpal stream
        std::thread::spawn(|| {
            if let Err(e) = ensure_capture_running() {
                eprintln!("Voice prewarm: capture init failed: {}", e);
            }
        });
        // Start the background mic listener — opportunistic transcription
        // for fingerprint-DB enrichment. Gated on env var so users who
        // don't want it can opt out.
        if std::env::var("WEB2QML_NO_BACKGROUND_MIC").is_err() {
            start_background_listener();
        } else {
            crate::clog!("bg-listen", "disabled via WEB2QML_NO_BACKGROUND_MIC");
        }
        // Start the wake-word listener. Different mode from bg-listen:
        // burst-detection rather than slow-poll. Opt-out via env so
        // users who don't want continuous mic processing can disable.
        if std::env::var("WEB2QML_NO_WAKE_WORD").is_err() {
            start_wake_word_listener();
        } else {
            crate::clog!("wake-listen", "disabled via WEB2QML_NO_WAKE_WORD");
        }
    }
}

/// Queue of background-transcribed utterances for main.rs to consume.
/// Each entry is (recognized_text, mono_16khz_audio). The audio is kept
/// so main.rs can store a fingerprint once it has matched the text to
/// a known intent or history command.
///
/// Bounded: we cap at 32 pending entries. If main.rs isn't draining fast
/// enough, oldest get dropped. Prevents unbounded memory growth on a
/// system where shell history is empty (so nothing matches).
#[cfg(feature = "voice")]
static BG_TRANSCRIPTIONS: OnceLock<Mutex<VecDeque<(String, Vec<f32>)>>> = OnceLock::new();

/// Pull the next background transcription if available. Returns None when
/// the queue is empty. Called by main.rs on a periodic timer.
#[cfg(feature = "voice")]
pub fn pop_bg_transcription() -> Option<(String, Vec<f32>)> {
    let q = BG_TRANSCRIPTIONS.get()?;
    let mut g = q.lock().ok()?;
    g.pop_front()
}

#[cfg(not(feature = "voice"))]
pub fn pop_bg_transcription() -> Option<(String, Vec<f32>)> { None }

/// Start the background listening loop. Idempotent — calling multiple
/// times spawns only the first listener. Runs forever in a worker thread.
///
/// The loop:
///   1. Sleep 60-180 seconds (random within range to avoid synchronized
///      spikes if multiple instances run; jitter prevents predictable
///      battery-drain patterns).
///   2. If WEB2QML_NO_BACKGROUND_MIC was set, return immediately.
///   3. If foreground voice is currently capturing, skip this round —
///      we don't want to compete for the model context.
///   4. Check battery: if < 30% AND not charging, skip. We don't want
///      to drain a critically-low battery for opportunistic learning.
///   5. Check rolling RMS on bg buffer: if too low, no recent speech,
///      skip. Otherwise we'd waste a whisper inference on silence.
///   6. Snapshot the bg buffer, resample to 16kHz mono, transcribe.
///   7. Validate the result (length sane, no obvious repetition
///      hallucination, no junk artifacts).
///   8. Push to BG_TRANSCRIPTIONS for main to process.
///
/// Designed to never starve the user. Runs at low priority via OS
/// scheduling defaults — Rust threads inherit the parent thread's
/// priority and we don't elevate.
#[cfg(feature = "voice")]
fn start_background_listener() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return; // already running
    }
    BG_TRANSCRIPTIONS.get_or_init(|| Mutex::new(VecDeque::new()));
    crate::clog!("bg-listen", "starting background fingerprint enrichment loop");

    std::thread::spawn(|| {
        // Stagger the first sleep so we don't fire on launch (mic isn't
        // calibrated, user hasn't spoken yet, system is busy with init).
        std::thread::sleep(std::time::Duration::from_secs(120));

        let mut sample_count: u32 = 0;
        const MAX_SAMPLES_PER_SESSION: u32 = 50;

        loop {
            // Random jitter 60-180s. Without rand crate we use a cheap
            // pseudo-random source: nanoseconds of system time mod range.
            let jitter_secs = {
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos() as u64).unwrap_or(0);
                60 + (nanos % 121)
            };
            std::thread::sleep(std::time::Duration::from_secs(jitter_secs));

            if sample_count >= MAX_SAMPLES_PER_SESSION {
                crate::clog!("bg-listen", "hit per-session cap ({}), pausing for 1h",
                    MAX_SAMPLES_PER_SESSION);
                std::thread::sleep(std::time::Duration::from_secs(3600));
                sample_count = 0;
                continue;
            }

            // Don't compete with foreground voice. The CANCEL_FLAG is
            // set when foreground recording is in progress. The capturing
            // bool on the foreground ring is a more direct check.
            let foreground_active = RING_BUFFER.get()
                .and_then(|r| r.lock().ok().map(|g| g.capturing))
                .unwrap_or(false);
            if foreground_active {
                crate::clog!("bg-listen", "skipping — foreground voice is recording");
                continue;
            }

            // Battery check.
            if !battery_ok_for_background() {
                crate::clog!("bg-listen", "skipping — battery low and not charging");
                continue;
            }

            // Speech-presence check via the rolling RMS the bg buffer
            // maintains. Threshold ~0.005 covers normal speech with
            // some headroom for quiet speakers; below that is silence
            // or background hum.
            let bg_arc = match BG_RING_BUFFER.get() {
                Some(b) => b.clone(),
                None => continue,
            };
            let (rms, audio_src, src_rate) = {
                let g = match bg_arc.lock() {
                    Ok(g) => g,
                    Err(_) => continue,
                };
                let r = g.rolling_rms();
                let snap = g.snapshot_mono();
                let sr = g.source_rate;
                drop(g);
                if r < 0.005 {
                    // Silence — don't bother whisper. But DO use this
                    // sample to characterize the user's noise floor.
                    // Resample to 16k for consistency with the rest of
                    // the pipeline (audio_profile RMS calc is rate-
                    // independent, but a future spectral analysis isn't).
                    let snap16k = if sr == TARGET_RATE { snap }
                        else { resample(&snap, sr, TARGET_RATE) };
                    if !snap16k.is_empty() {
                        crate::audio_profile::record_noise_sample(&snap16k);
                    }
                    crate::clog!("bg-listen", "skipping — rms {:.4} below threshold (recorded as noise)", r);
                    continue;
                }
                (r, snap, sr)
            };

            // Resample to 16kHz mono. Same logic as foreground path.
            let mut audio_16k = if src_rate == TARGET_RATE {
                audio_src
            } else {
                resample(&audio_src, src_rate, TARGET_RATE)
            };
            if audio_16k.len() < TARGET_RATE as usize {
                // Less than 1s of audio — not worth running.
                continue;
            }
            // Pre-process: high-pass to kill rumble + DC offset.
            // Background captures are especially prone to picking up
            // HVAC / fans because they're always-on; this filter
            // makes a meaningful difference in transcription quality.
            high_pass_80hz(&mut audio_16k);
            // v0.1.783: optional gentle gain for quiet captures (opt-in via
            // WEB2QML_VOICE_GAIN=1). Conservative — never clips, never
            // attenuates, capped boost. Logged when it does anything.
            if std::env::var("WEB2QML_VOICE_GAIN").map(|v| v == "1").unwrap_or(false) {
                let g = normalize_gain(&mut audio_16k);
                if g != 1.0 { crate::clog!("voice", "gain x{:.2}", g); }
            }
            // Drone-pattern check: if the audio is just a sustained
            // tone (fan, AC hum, bike traffic outside), skip it
            // entirely. Saves a whisper inference and prevents
            // garbage fingerprints from polluting the DB.
            if looks_like_drone(&audio_16k) {
                continue;
            }
            // Password-mode suppression. If terminal is at a sudo
            // prompt right now, the user might voice their password
            // (or anything else they said quietly while focused on
            // typing). We DON'T transcribe, fingerprint, or store
            // any of it. Whole capture is dropped on the floor.
            if crate::security::should_suppress_capture() {
                crate::clog!("bg-listen", "skipping — password mode active");
                continue;
            }

            // Transcribe via whichever model is loaded. Use the same
            // dispatcher the foreground uses — it picks the best
            // available tier.
            let text = match transcribe_with_dual_model(&audio_16k) {
                Ok(t) => t,
                Err(e) => {
                    crate::clog!("bg-listen", "transcription failed: {}", e);
                    continue;
                }
            };
            let cleaned = strip_whisper_artifacts(&text);
            if !is_sane_transcription(&cleaned) {
                crate::clog!("bg-listen", "discarding likely-hallucination: {:?}", cleaned);
                continue;
            }

            sample_count += 1;
            crate::clog!("bg-listen", "sample #{} rms={:.4} text={:?}",
                sample_count, rms, cleaned);

            // Queue for main.rs to match against history + store fingerprint.
            if let Some(q) = BG_TRANSCRIPTIONS.get() {
                if let Ok(mut g) = q.lock() {
                    g.push_back((cleaned, audio_16k));
                    while g.len() > 32 { g.pop_front(); }
                }
            }
        }
    });
}

/// Returns true if it's safe to do background work (mic + whisper) on
/// the current battery state. Logic:
///   - On AC / charging / unknown: always OK
///   - On battery: OK only if capacity > 30%
///
/// Reads /sys/class/power_supply. Returns true if we can't read (e.g.
/// non-Linux, no battery — desktop systems): no battery means infinite
/// power.
#[cfg(feature = "voice")]
fn battery_ok_for_background() -> bool {
    let supplies_dir = "/sys/class/power_supply";
    let entries = match std::fs::read_dir(supplies_dir) {
        Ok(e) => e,
        Err(_) => return true, // no /sys = desktop or non-Linux, OK
    };
    let mut on_battery = false;
    let mut capacity_pct: Option<u32> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("BAT") {
            // Read capacity
            if let Ok(c) = std::fs::read_to_string(entry.path().join("capacity")) {
                if let Ok(n) = c.trim().parse::<u32>() {
                    capacity_pct = Some(n);
                }
            }
            // Read status — Discharging means on battery
            if let Ok(s) = std::fs::read_to_string(entry.path().join("status")) {
                if s.trim() == "Discharging" {
                    on_battery = true;
                }
            }
        }
    }
    if !on_battery { return true; }
    capacity_pct.map(|p| p > 30).unwrap_or(true)
}

/// Heuristic check that a transcription is plausible (not a whisper
/// hallucination). Rules:
///   - Length 2..=120 chars (too short = no speech; too long = wall of
///     noise that whisper invented from breath/static)
///   - Not entirely repeating one word (whisper sometimes loops)
///   - Doesn't contain known hallucination patterns ("thank you for
///     watching", "subscribe to my channel", etc. — whisper trained on
///     YouTube text and emits these on near-silence)
///
/// v0.1.209: now `pub` so the bg-drain path in main.rs can use it as
/// a pre-filter before fingerprint storage. Previously private.
#[cfg(feature = "voice")]
pub fn is_sane_transcription(text: &str) -> bool {
    let t = text.trim();
    let len = t.chars().count();
    if len < 2 || len > 120 { return false; }
    // Repetition check: if the same word appears >50% of the time, it's
    // probably a hallucination loop.
    let words: Vec<&str> = t.split_whitespace().collect();
    if words.len() >= 4 {
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for w in &words { *counts.entry(*w).or_insert(0) += 1; }
        let max_count = counts.values().max().copied().unwrap_or(0);
        if max_count * 2 > words.len() { return false; }
    }
    // Known hallucination phrases. These are common enough across
    // whisper outputs that filtering them isn't going to lose much.
    let lower = t.to_lowercase();
    let bad_phrases = [
        "thank you for watching", "thanks for watching",
        "subscribe to", "like and subscribe",
        "see you next time", "see you in the next video",
        "don't forget to subscribe",
        "[music]", "[silence]", "[blank_audio]",
    ];
    for p in &bad_phrases {
        if lower.contains(p) { return false; }
    }
    true
}

// v0.1.209: stub for builds without the voice feature. Same shape,
// trivially "sane" since this path won't actually run when voice
// is disabled (the bg-drain loop exits on first None pop).
#[cfg(not(feature = "voice"))]
pub fn is_sane_transcription(_text: &str) -> bool {
    true
}

// ════════════════════════════════════════════════════════════════════
// WAKE-WORD DETECTION
// ════════════════════════════════════════════════════════════════════
//
// Continuous "Yo Big K" detection over the bg ring buffer. When a
// wake fires, push an event for QML to consume. QML's voice popup
// then opens automatically as if the user had pressed the voice
// button, with the captured wake-word audio's pitch already passed
// to the matching pipeline.
//
// This is a SEPARATE worker from start_background_listener. The bg
// listener does slow-poll sampling for fingerprint enrichment; this
// one does fast burst detection for hands-free triggering. They share
// the BG_RING_BUFFER but run on independent schedules.
//
// Detection pipeline:
//   1. Watch rolling RMS. Wait for it to RISE above noise_floor*1.8
//      and stay high for >= 250ms (real speech, not a transient).
//   2. Continue watching until RMS FALLS below noise_floor*1.4 for
//      >= 350ms (utterance ended) OR we hit 4s max (cap on wake
//      duration — "Yo Big K" is ~700ms, no real wake should exceed
//      4s).
//   3. Snapshot the bg buffer covering the speech region.
//   4. Resample to 16kHz mono.
//   5. Quick MFCC match against any "__wake_word__" fingerprints in
//      the phonetic DB. If no decent match, skip whisper entirely
//      (cheap rejection).
//   6. If MFCC is plausible, run whisper on the snapshot.
//   7. Check if recognized text contains the wake phrase (fuzzy).
//   8. Fire wake event with the audio for downstream pitch capture.
//
// Cost budget per detection cycle:
//   - RMS polling: trivial, every 50ms
//   - MFCC + match: 5-30ms (already what we compute for foreground)
//   - Whisper: 200-1500ms depending on tier and burst length
//
// Total per false-positive (RMS rose but no wake): ~30ms (skipped
// whisper). Total per true positive (wake detected): ~400-2000ms,
// but this is the path the user actively wants.

/// Wake event queue. Each entry is `(audio, remainder)`:
///   - audio: 16kHz mono of the wake utterance
///   - remainder: text AFTER the wake phrase in the same utterance,
///     empty if the user only said the wake phrase. v0.1.121+: when
///     the user says "keyword, open terminal" we extract "open
///     terminal" as the remainder so QML can act on it directly
///     instead of starting a fresh recording.
///
/// Main pulls and acts: if remainder is non-empty, process as a
/// recognized utterance immediately (intent parse → execute). If
/// remainder is empty (user just said the wake phrase), open the
/// voice popup to listen for the next command.
#[cfg(feature = "voice")]
static WAKE_EVENTS: OnceLock<Mutex<VecDeque<(Vec<f32>, String)>>> = OnceLock::new();

/// AI-question event queue. Each entry is the question text (the post-
/// "ask ai" remainder). Main pulls and dispatches an LLM query, then
/// surfaces the response in a dedicated UI popup with code blocks the
/// user can press-and-hold to paste.
#[cfg(feature = "voice")]
static ASK_AI_EVENTS: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();

/// Pop the next wake event. Returns `(audio, remainder)` where remainder
/// is the post-keyword text (empty if user only said the wake phrase).
#[cfg(feature = "voice")]
pub fn pop_wake_event() -> Option<(Vec<f32>, String)> {
    let q = WAKE_EVENTS.get()?;
    let mut g = q.lock().ok()?;
    let ev = g.pop_front();
    // v0.1.750: clear the signal bit once the queue is drained so the cheap
    // peek goes quiet again. (raise on enqueue means we never miss one.)
    if g.is_empty() {
        crate::event_signal::clear(crate::event_signal::Channel::Wake);
    }
    ev
}

#[cfg(not(feature = "voice"))]
pub fn pop_wake_event() -> Option<(Vec<f32>, String)> { None }

/// Pop the next AI-question event. Returns the question text that
/// followed the "ask ai" buzzword, or None if no AI question has
/// fired since the last pop.
#[cfg(feature = "voice")]
pub fn pop_ask_ai_event() -> Option<String> {
    let q = ASK_AI_EVENTS.get()?;
    let mut g = q.lock().ok()?;
    let ev = g.pop_front();
    if g.is_empty() {
        crate::event_signal::clear(crate::event_signal::Channel::AskAi);
    }
    ev
}

#[cfg(not(feature = "voice"))]
pub fn pop_ask_ai_event() -> Option<String> { None }

/// Allow main to suppress wake detection during foreground voice
/// (we don't want a wake to fire while the user is already in the
/// voice popup mid-utterance). Foreground voice flow sets this true
/// at capture-start and false at capture-end.
#[cfg(feature = "voice")]
static WAKE_SUPPRESSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Suppress / unsuppress wake detection. Public so main can gate
/// wake firing during foreground voice activity. Idempotent.
pub fn set_wake_suppressed(suppressed: bool) {
    #[cfg(feature = "voice")]
    {
        WAKE_SUPPRESSED.store(suppressed, std::sync::atomic::Ordering::Relaxed);
    }
    #[cfg(not(feature = "voice"))]
    {
        let _ = suppressed;
    }
}

/// v0.1.202: query current suppression state. Returns false when
/// voice feature is disabled (no wake to suppress). Used by the
/// quick-settings popup to display the current mute state on the
/// voice toggle.
pub fn is_wake_suppressed() -> bool {
    #[cfg(feature = "voice")]
    {
        WAKE_SUPPRESSED.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(feature = "voice"))]
    {
        false
    }
}

/// v0.1.459: app-foreground state. QML pushes this from the
/// ApplicationWindow's active/visibility changes (screen-off or
/// app-backgrounded). When true, idle subsystems back off to save
/// power: the wake-listener polls 4x slower (200ms vs 50ms) since
/// nobody's watching the screen and the extra wake latency is
/// imperceptible when the phone's in a pocket. Other subsystems
/// (summary worker, etc.) can consult this too. Cheap to read;
/// checked once per loop iteration.
static APP_BACKGROUNDED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Set by QML when the app goes to background / screen turns off
/// (true) or returns to foreground (false). Idempotent.
pub fn set_app_backgrounded(bg: bool) {
    APP_BACKGROUNDED.store(bg, std::sync::atomic::Ordering::Relaxed);
}

/// True when the app is backgrounded / screen is off. Used to
/// pick the slow poll cadence in the wake loop and to gate other
/// idle work.
pub fn is_app_backgrounded() -> bool {
    APP_BACKGROUNDED.load(std::sync::atomic::Ordering::Relaxed)
}
//
// Lifetime counters that QML can poll to give the user feedback on
// whether the wake-word listener is actually working. Without these
// the listener is invisible — there's no way for a user to tell if
// their enrollment took effect or if the mic is being picked up.
//
// All atomics so reads/writes from the listener thread don't need
// to lock anything.
#[cfg(feature = "voice")]
static WAKE_BURSTS_DETECTED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "voice")]
static WAKE_FIRED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Most recent peak RMS observed by the wake listener. Lets QML render
/// a tiny "are you receiving anything?" indicator. We multiply by 1000
/// and store as u32 so the atomic stays simple — units are "RMS × 1000".
#[cfg(feature = "voice")]
static WAKE_LAST_RMS_X1000: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// How many bursts the wake listener has detected since startup.
/// (A burst = audio rising above the speaking threshold for >= 250ms.
/// Most bursts are real speech; not all become wake fires — that
/// requires the audio to also classify as a known wake phrase.)
pub fn wake_bursts_detected() -> u32 {
    #[cfg(feature = "voice")]
    {
        return WAKE_BURSTS_DETECTED.load(std::sync::atomic::Ordering::Relaxed);
    }
    #[cfg(not(feature = "voice"))]
    { 0 }
}

/// How many times the wake word has actually fired since startup.
pub fn wake_fired_count() -> u32 {
    #[cfg(feature = "voice")]
    {
        return WAKE_FIRED.load(std::sync::atomic::Ordering::Relaxed);
    }
    #[cfg(not(feature = "voice"))]
    { 0 }
}

/// Most recent peak audio RMS the wake listener observed. 0..~50 typical
/// (noise floor ~3-8, normal speech 20-40, loud speech 50+). Useful
/// for a "your mic is working" indicator in the wake-word settings.
pub fn wake_last_rms() -> f32 {
    #[cfg(feature = "voice")]
    {
        return WAKE_LAST_RMS_X1000.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0;
    }
    #[cfg(not(feature = "voice"))]
    { 0.0 }
}

/// Wake-word phrases. Match against the WHOLE recognized text in
/// lowercase form. We accept several variants because whisper's
/// transcription of "Yo Big K" is unstable across mics — common
/// variants include "yo big k", "your big k", "yo big kay", "you
/// big k", etc. Liberal matching is OK at this layer because
/// false positives still get gated by the MFCC similarity check
/// upstream — only audio close to the registered wake-word
/// fingerprint reaches whisper at all.
///
/// User-configurable in future via a settings file; hardcoded for
/// v0.1.72 first release.
#[cfg(feature = "voice")]
const WAKE_PHRASES: &[&str] = &[
    "yo big k", "yo bigk", "yo, big k", "yo big kay", "yo big-k",
    "your big k", "you big k", "your big kay",
    "hey big k", "hey big kay", "hey, big k",
    "ok big k", "okay big k",
];

/// User's enrolled wake phrase — what whisper transcribed when they
/// recorded the wake word. We use this as the AUTHORITATIVE text
/// match instead of the hardcoded WAKE_PHRASES list, because the
/// user may have enrolled "hey fairphone" or "computer" or anything,
/// and the hardcoded list would never accept it.
///
/// The hardcoded WAKE_PHRASES list remains as a fallback so users
/// who haven't enrolled (or whose enrollment was erased) can still
/// use the default wake.
///
/// Stored as a single string (not a list) — when the user re-records,
/// the new phrase replaces the old. Loaded from
/// ~/.config/web2qml/wake_phrase.txt at startup.
#[cfg(feature = "voice")]
static REGISTERED_WAKE_PHRASE: OnceLock<Mutex<String>> = OnceLock::new();

/// Set / update the user's enrolled wake phrase. Called after
/// successful wake-word registration. Empty string clears it
/// (revert to hardcoded fallback list).
pub fn set_registered_wake_phrase(phrase: &str) {
    #[cfg(feature = "voice")]
    {
        let lock = REGISTERED_WAKE_PHRASE.get_or_init(|| Mutex::new(String::new()));
        if let Ok(mut g) = lock.lock() {
            *g = phrase.to_lowercase();
        }
    }
    // Silence unused-variable warning when voice feature is off.
    #[cfg(not(feature = "voice"))]
    {
        let _ = phrase;
    }
}

/// Read the currently registered wake phrase. Empty when none enrolled.
pub fn registered_wake_phrase() -> String {
    #[cfg(feature = "voice")]
    {
        if let Some(lock) = REGISTERED_WAKE_PHRASE.get() {
            if let Ok(g) = lock.lock() {
                return g.clone();
            }
        }
    }
    String::new()
}

/// Load the registered wake phrase from disk at startup. Idempotent.
/// Looks for ~/.config/web2qml/wake_phrase.txt — single line, lowercase.
pub fn load_registered_wake_phrase_from_disk() {
    #[cfg(feature = "voice")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        if home.is_empty() { return; }
        let path = std::path::Path::new(&home).join(".config/web2qml/wake_phrase.txt");
        if let Ok(content) = std::fs::read_to_string(&path) {
            let trimmed = content.trim().to_lowercase();
            if !trimmed.is_empty() {
                set_registered_wake_phrase(&trimmed);
                crate::clog!("wake-word", "loaded registered phrase: {:?}", trimmed);
            }
        }
    }
}

/// AI-question wake phrases. When matched, the post-phrase remainder is
/// sent to the LLM as a free-form question; the response surfaces in a
/// dedicated popup with tappable code blocks.
///
/// Unlike the standard wake (which has a fixed pitch profile + acoustic
/// prefilter via __wake_word__ fingerprints), the AI wake is text-only.
/// We accept slightly more variation because the typical use case is
/// "ask AI how many miles to the moon" — a question, not a command —
/// and false-positives just produce a useless LLM response that the
/// user can dismiss.
#[cfg(feature = "voice")]
const AI_WAKE_PHRASES: &[&str] = &[
    "ask ai", "ask a i", "ask the ai", "ask the a i",
    "ai question", "a i question",
    "hey ai", "hey a i",
    "computer", "computer question",  // sci-fi homage; users seem to like
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(feature = "voice")]
enum WakeCategory { Standard, AskAi }

/// Returns Some(category) if `text` contains a wake phrase. The
/// matching is case-insensitive and tolerates extra punctuation.
#[cfg(feature = "voice")]
fn classify_wake_phrase(text: &str) -> Option<WakeCategory> {
    let lower = text.to_lowercase();
    // Strip non-alphanumeric (except spaces) so commas / punctuation
    // don't break a contains() match. "yo, big k!" -> "yo big k".
    let cleaned: String = lower.chars()
        .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
        .collect();
    // Collapse whitespace runs.
    let mut compact = String::with_capacity(cleaned.len());
    let mut prev_space = false;
    for c in cleaned.chars() {
        if c.is_whitespace() {
            if !prev_space { compact.push(' '); }
            prev_space = true;
        } else {
            compact.push(c);
            prev_space = false;
        }
    }
    let final_text = compact.trim();
    // FIRST: check the user's enrolled wake phrase if any. Liberal
    // matching so whisper transcription noise doesn't kill the match.
    // We use a contains() check + a simple word-overlap fuzzy match
    // so "hey fairphone" registered as "hey fair phone" still matches.
    let enrolled = registered_wake_phrase();
    if !enrolled.is_empty() {
        if phrase_match(final_text, &enrolled) {
            return Some(WakeCategory::Standard);
        }
    }
    // SECOND: hardcoded fallback list (covers default "yo big k"
    // when user hasn't enrolled).
    for phrase in WAKE_PHRASES {
        if final_text.contains(phrase) { return Some(WakeCategory::Standard); }
    }
    for phrase in AI_WAKE_PHRASES {
        if final_text.contains(phrase) { return Some(WakeCategory::AskAi); }
    }
    None
}

/// Fuzzy phrase match: returns true if `text` plausibly contains
/// `phrase`. Strict `contains()` first; if that fails, fall back to
/// word-overlap (>=70% of phrase's content words appear in text).
/// This handles whisper transcription drift like "hey fair phone"
/// matching enrolled "hey fairphone" and vice versa.
#[cfg(feature = "voice")]
fn phrase_match(text: &str, phrase: &str) -> bool {
    if text.contains(phrase) { return true; }
    let phrase_words: Vec<&str> = phrase.split_whitespace()
        .filter(|w| w.len() > 1)  // skip "a", "i" etc.
        .collect();
    if phrase_words.is_empty() { return false; }
    // Squash both to no-spaces and check substring — handles "fair phone"
    // vs "fairphone" specifically.
    let squashed_text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let squashed_phrase: String = phrase.chars().filter(|c| !c.is_whitespace()).collect();
    if squashed_text.contains(&squashed_phrase) { return true; }
    // Word-overlap: count phrase words that appear in text.
    let text_words: std::collections::HashSet<&str> = text.split_whitespace().collect();
    let mut hits = 0;
    for pw in &phrase_words {
        if text_words.contains(pw) { hits += 1; }
    }
    let ratio = hits as f32 / phrase_words.len() as f32;
    ratio >= 0.70
}

/// Strip the AI-wake phrase prefix from text, returning the remainder
/// (the actual question). E.g. "ask ai how many miles to the moon"
/// → "how many miles to the moon".
///
/// If no prefix matches OR the remainder is empty, returns None.
/// Empty remainder means user just said "ask ai" without a question
/// — we don't fire a useless LLM call.
#[cfg(feature = "voice")]
fn strip_ai_wake_prefix(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let cleaned: String = lower.chars()
        .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
        .collect();
    let compact: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    for phrase in AI_WAKE_PHRASES {
        if let Some(idx) = compact.find(phrase) {
            let after = &compact[idx + phrase.len()..];
            let trimmed = after.trim();
            if !trimmed.is_empty() && trimmed.len() >= 3 {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Extract the post-wake-keyword remainder from a transcript. v0.1.121.
///
/// E.g. "hey claude open terminal" with enrolled phrase "hey claude"
/// returns "open terminal". When the user says only the wake phrase
/// ("hey claude") returns None — caller opens the popup for the
/// next utterance instead of trying to execute an empty command.
///
/// Tries the enrolled phrase first, then the hardcoded WAKE_PHRASES
/// list. Returns the first non-empty remainder found.
///
/// We use the same cleaning logic as classify_wake_phrase so the
/// trimming behaves consistently with what triggered the wake.
#[cfg(feature = "voice")]
fn strip_standard_wake_prefix(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let cleaned: String = lower.chars()
        .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
        .collect();
    let compact: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");

    // First: try the user's enrolled wake phrase.
    let enrolled = registered_wake_phrase();
    if !enrolled.is_empty() {
        if let Some(remainder) = strip_phrase(&compact, &enrolled) {
            return Some(remainder);
        }
    }

    // Second: hardcoded WAKE_PHRASES list.
    for phrase in WAKE_PHRASES {
        if let Some(remainder) = strip_phrase(&compact, phrase) {
            return Some(remainder);
        }
    }

    None
}

/// Helper: find `phrase` in `text` (using the same fuzzy logic as
/// phrase_match where possible), return everything AFTER it trimmed,
/// or None if there's nothing meaningful after.
///
/// Returns None when:
/// - phrase not found
/// - remainder is empty or whitespace
/// - remainder is too short to be a meaningful command (< 2 chars)
#[cfg(feature = "voice")]
fn strip_phrase(text: &str, phrase: &str) -> Option<String> {
    // Try exact substring first.
    if let Some(idx) = text.find(phrase) {
        let after = &text[idx + phrase.len()..];
        let trimmed = after.trim();
        if trimmed.len() >= 2 {
            return Some(trimmed.to_string());
        } else if trimmed.is_empty() {
            // Caller distinguishes "no trailing command" (None) from
            // "found, no remainder" — here we signal "no remainder".
            return None;
        }
    }
    // Fallback: squashed (whitespace-removed) match for "fair phone"
    // vs "fairphone" cases. Compute squashed indexes; map back to the
    // original text by walking character offsets.
    let squashed_text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let squashed_phrase: String = phrase.chars().filter(|c| !c.is_whitespace()).collect();
    if let Some(sq_idx) = squashed_text.find(&squashed_phrase) {
        let sq_end = sq_idx + squashed_phrase.len();
        // Walk text, counting non-whitespace chars, until we reach sq_end.
        let mut non_ws_seen = 0usize;
        let mut end_byte: Option<usize> = None;
        for (i, c) in text.char_indices() {
            if non_ws_seen >= sq_end {
                end_byte = Some(i);
                break;
            }
            if !c.is_whitespace() { non_ws_seen += 1; }
        }
        let after_idx = end_byte.unwrap_or(text.len());
        let after = &text[after_idx..];
        let trimmed = after.trim();
        if trimmed.len() >= 2 {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Backward-compat shim: returns true if text contains any wake phrase.
/// Older code paths used this single boolean; new paths should prefer
/// classify_wake_phrase to distinguish categories.
#[cfg(feature = "voice")]
#[allow(dead_code)] // older boolean API; new paths use classify_wake_phrase
fn matches_wake_phrase(text: &str) -> bool {
    classify_wake_phrase(text).is_some()
}

/// Check whether the bg DB has any registered wake-word fingerprints.
/// If not, the wake-word listener doesn't need to fire whisper at all
/// (no reference to compare against, and a "yo big k" phrase from a
/// non-registered speaker would still trigger but we'd have no
/// speaker-pitch to attach to it).
///
/// Implementation: peek into phonetic db looking for the sentinel
/// label "__wake_word__". This is the label voice_register_wake_word
/// stores under.
#[cfg(feature = "voice")]
fn has_registered_wake_word() -> bool {
    crate::phonetic::has_label("__wake_word__")
}

/// MFCC similarity score between captured audio and the registered
/// __wake_word__ fingerprints. Returns 0.0 if no fingerprints exist
/// or audio is too short. Higher = more similar (we use 1 - normalized
/// DTW distance internally; 1.0 is identical).
///
/// Used as the cheap prefilter BEFORE whisper. Threshold: ~0.55
/// is the empirical line we'll start at (tunable). Below that, the
/// audio doesn't acoustically resemble the registered wake — skip
/// whisper.
#[cfg(feature = "voice")]
fn wake_word_acoustic_score(audio_16k_mono: &[f32]) -> f32 {
    let matches = crate::phonetic::top_matches(audio_16k_mono, 5);
    let best_score = matches.iter()
        .filter(|(label, _)| label == "__wake_word__")
        .map(|(_, s)| *s)
        .fold(f32::INFINITY, f32::min);
    if best_score == f32::INFINITY {
        return 0.0;
    }
    // top_matches returns DISTANCE (lower = better). Convert to
    // similarity in 0..1 via a soft inverse. The exact constant
    // (4.0) is tuned to put typical good matches around 0.55-0.85.
    // Values much above won't happen; values below mean "not similar".
    let sim = 1.0 / (1.0 + best_score / 4.0);
    sim.clamp(0.0, 1.0)
}

/// Start the wake-word listening loop. Idempotent — only spawns one
/// thread. The thread runs forever (until process exit), polling the
/// bg ring buffer for speech bursts.
#[cfg(feature = "voice")]
fn start_wake_word_listener() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    WAKE_EVENTS.get_or_init(|| Mutex::new(VecDeque::new()));
    crate::clog!("wake-listen", "starting wake-word detection loop");
    std::thread::spawn(|| {
        // Initial pause: let the bg ring buffer fill with audio AND
        // give whisper time to load. Without this, the first wake
        // attempt would race the whisper-load and fail.
        std::thread::sleep(std::time::Duration::from_secs(8));

        // State machine for burst detection.
        // Idle: waiting for RMS to rise.
        // Speaking: RMS has been high; tracking duration.
        let mut speaking_start: Option<std::time::Instant> = None;
        let mut speech_high_since: Option<std::time::Instant> = None;
        let mut speech_low_since: Option<std::time::Instant> = None;

        // Tunable thresholds. Scaled relative to dynamic noise floor
        // so the listener adapts to the user's environment. The
        // floor is updated by audio_profile from bg-listen samples.
        const POLL_MS: u64 = 50;
        const RISE_HOLD_MS: u128 = 250;     // RMS must stay high this long
        const FALL_HOLD_MS: u128 = 350;     // RMS must stay low this long
        const MAX_BURST_MS: u128 = 4000;    // cap utterance length

        // Per-session counters (logging only).
        let mut bursts_detected: u32 = 0;
        let mut wakes_fired: u32 = 0;

        loop {
            // v0.1.459: back off the poll cadence when the app is
            // backgrounded / screen off. Foreground: 50ms (snappy
            // wake). Background: 200ms — 4x fewer wakeups for this
            // thread, and the ~150ms extra worst-case wake latency
            // is irrelevant when the phone's in a pocket. Read once
            // per iteration (cheap atomic).
            let poll_ms = if is_app_backgrounded() { 200 } else { POLL_MS };
            std::thread::sleep(std::time::Duration::from_millis(poll_ms));

            // Cheap gates first.
            if WAKE_SUPPRESSED.load(Ordering::Relaxed) {
                speaking_start = None;
                speech_high_since = None;
                speech_low_since = None;
                continue;
            }
            // Don't fire wake while foreground voice is recording.
            // The capturing flag on the foreground ring is the
            // canonical signal.
            let foreground_active = RING_BUFFER.get()
                .and_then(|r| r.lock().ok().map(|g| g.capturing))
                .unwrap_or(false);
            if foreground_active {
                speaking_start = None;
                speech_high_since = None;
                speech_low_since = None;
                continue;
            }

            // Skip detection only if NEITHER feature is configured
            // — i.e. no standard wake registered AND no point listening
            // for AI wakes. Currently AI wakes are always available
            // (no registration needed), so the listener always runs.
            // Kept the gate as a future hook in case we add an AI-wake
            // disable.
            //
            // Even without a standard-wake fingerprint, we still want
            // to listen for "ask ai <question>" — that path doesn't
            // need acoustic enrollment.

            // Battery: same conservative gate as bg-listen.
            if !battery_ok_for_background() {
                speaking_start = None;
                speech_high_since = None;
                speech_low_since = None;
                continue;
            }

            // Read current rolling RMS + adaptive thresholds.
            let bg_arc = match BG_RING_BUFFER.get() {
                Some(b) => b.clone(),
                None => continue,
            };
            let rms = match bg_arc.lock() {
                Ok(g) => g.rolling_rms(),
                Err(_) => continue,
            };
            // Track peak RMS over recent polls for QML diagnostic.
            // Stored as RMS × 1000 so we can use a u32 atomic. We
            // overwrite on each poll rather than tracking peak — the
            // user wants "is the mic active" which is "did this go
            // above noise floor recently", and the rolling_rms is
            // already a 100ms-windowed average so it's stable.
            WAKE_LAST_RMS_X1000.store((rms * 1000.0) as u32,
                std::sync::atomic::Ordering::Relaxed);
            // v0.1.123: tightened sensitivity. Was noise*1.8 (high)
            // and noise*1.4 (low). For quiet speakers whose speech
            // RMS sits close to noise floor, 1.8x was too high — the
            // bg listener would miss the wake entirely. Cut to 1.4 /
            // 1.15 so the wake fires more readily; the acoustic
            // prefilter + whisper transcription still gates false
            // positives downstream. Worst case: a few more bursts get
            // through to whisper (cheap; ~250ms) and get rejected
            // there.
            //
            // Hardcoded mins kept at the same levels to avoid trigger
            // storms in very quiet rooms.
            let noise = crate::audio_profile::noise_floor();
            let high_threshold = (noise * 1.4).max(0.005);
            let low_threshold = (noise * 1.15).max(0.003);

            // Periodic heartbeat so the user can confirm the wake
            // listener is alive AND see what its current threshold is.
            // Without this, "wake word doesn't work" is undiagnosable
            // from the log alone — there's no signal the listener is
            // running unless a burst happens to fire.
            //
            // Log once every ~6s (one tick every 50ms * 120) = at most
            // 10 lines per minute. Includes current RMS, threshold,
            // and registered phrase so user can compare against
            // observed audio level.
            use std::sync::atomic::{AtomicU32, Ordering as AOrder};
            static HEARTBEAT_COUNT: AtomicU32 = AtomicU32::new(0);
            let hb = HEARTBEAT_COUNT.fetch_add(1, AOrder::Relaxed);
            if hb % 120 == 0 {
                let enrolled = registered_wake_phrase();
                crate::clog!("wake-listen", "alive: rms={:.4} high_thr={:.4} low_thr={:.4} enrolled={:?}",
                    rms, high_threshold, low_threshold,
                    if enrolled.is_empty() { "(none)" } else { enrolled.as_str() });
            }

            let now = std::time::Instant::now();

            match speaking_start {
                None => {
                    // Idle. Watch for a sustained RMS rise.
                    if rms > high_threshold {
                        match speech_high_since {
                            Some(t) if now.duration_since(t).as_millis() >= RISE_HOLD_MS => {
                                // Confirmed speech start.
                                speaking_start = Some(t);
                                speech_low_since = None;
                                crate::clog!("wake-listen", "burst start, rms={:.4} thr={:.4}",
                                    rms, high_threshold);
                            }
                            Some(_) => {} // still accumulating
                            None => speech_high_since = Some(now),
                        }
                    } else {
                        // RMS dropped before we confirmed — abort.
                        speech_high_since = None;
                    }
                }
                Some(burst_start) => {
                    let elapsed_ms = now.duration_since(burst_start).as_millis();
                    let timed_out = elapsed_ms >= MAX_BURST_MS;
                    let confirmed_low = if rms < low_threshold {
                        match speech_low_since {
                            None => { speech_low_since = Some(now); false }
                            Some(t) => now.duration_since(t).as_millis() >= FALL_HOLD_MS,
                        }
                    } else {
                        speech_low_since = None;
                        false
                    };
                    if confirmed_low || timed_out {
                        // End of burst. Snapshot and process.
                        bursts_detected += 1;
                        WAKE_BURSTS_DETECTED.store(bursts_detected,
                            std::sync::atomic::Ordering::Relaxed);
                        crate::clog!("wake-listen", "burst end after {}ms ({} bursts so far)",
                            elapsed_ms, bursts_detected);
                        let (snap, src_rate) = match bg_arc.lock() {
                            Ok(g) => (g.snapshot_mono(), g.source_rate),
                            Err(_) => {
                                speaking_start = None;
                                speech_high_since = None;
                                speech_low_since = None;
                                continue;
                            }
                        };
                        let snap16k = if src_rate == TARGET_RATE { snap }
                            else { resample(&snap, src_rate, TARGET_RATE) };
                        // Reset state BEFORE running expensive checks
                        // so we're ready to detect the next burst even
                        // if processing this one takes a moment.
                        speaking_start = None;
                        speech_high_since = None;
                        speech_low_since = None;
                        if snap16k.len() < TARGET_RATE as usize / 2 {
                            // < 0.5s — too short for a wake phrase.
                            continue;
                        }
                        // Acoustic prefilter for STANDARD wake only.
                        // The AI-question wake doesn't have an acoustic
                        // reference fingerprint, so it can't benefit
                        // from this fast-path rejection. We compute
                        // acoustic anyway because we still want to skip
                        // whisper on bursts that ARE close to the
                        // standard wake but didn't quite match — those
                        // are the user trying to say it again, and
                        // running whisper is the right thing.
                        //
                        // Strategy: if standard-wake-registered AND
                        // acoustic is very LOW (< 0.4), skip whisper
                        // entirely. Otherwise run whisper and let
                        // classify_wake_phrase decide.
                        let standard_registered = has_registered_wake_word();
                        let acoustic = if standard_registered {
                            wake_word_acoustic_score(&snap16k)
                        } else { 0.0 };
                        // v0.1.632: decision extracted to the pure, tested
                        // crate::wake_gate (same thresholds: 0.40 match,
                        // 0.005 noise-floor, 1s short/long split). Keeping
                        // the per-branch clog messages here for dogfood
                        // visibility. See wake_gate.rs for the rationale +
                        // the journal evidence behind 0.005.
                        // v0.1.676: fix the longstanding custom-wake bug —
                        // the old gate vetoed a long burst whenever the
                        // acoustic fingerprint scored ~0, so a genuine
                        // enrolled phrase ("hey apple") never reached Whisper
                        // (→ the text classifier that actually knows the
                        // custom phrase). wake_gate_v2 only skips a long burst
                        // when SNR confirms it's ambient (RMS doesn't rise
                        // over the noise floor), so real utterances always get
                        // transcribed regardless of the acoustic fingerprint.
                        let burst_rms = {
                            let n = snap16k.len();
                            if n > 0 {
                                let sum_sq: f32 = snap16k.iter().map(|s| s * s).sum();
                                (sum_sq / n as f32).sqrt()
                            } else { 0.0 }
                        };
                        let noise_floor = crate::audio_profile::noise_floor();
                        match crate::wake_gate::wake_gate_v2(
                            standard_registered, acoustic, snap16k.len(), TARGET_RATE,
                            burst_rms, noise_floor,
                        ) {
                            crate::wake_gate::WakeGate::Skip(
                                crate::wake_gate::SkipReason::ShortLowAcoustic) => {
                                crate::clog!("wake-listen", "short burst rejected by acoustic match (sim={:.3})",
                                    acoustic);
                                continue;
                            }
                            crate::wake_gate::WakeGate::Skip(
                                crate::wake_gate::SkipReason::NoiseFloor) => {
                                crate::clog!("wake-listen", "long burst rejected — acoustic={:.3} (noise, not a wake-or-AI phrase)",
                                    acoustic);
                                continue;
                            }
                            crate::wake_gate::WakeGate::RunWhisper => {
                                if standard_registered && acoustic < 0.40 {
                                    crate::clog!("wake-listen", "long burst with low acoustic ({:.3}) — running whisper anyway for ai-wake check",
                                        acoustic);
                                } else {
                                    crate::clog!("wake-listen", "burst will be transcribed (acoustic={:.3}, registered={})",
                                        acoustic, standard_registered);
                                }
                            }
                        }
                        // Whisper to verify. Mark VOICE_ACTIVE so
                        // log_summary worker doesn't compete with us
                        // for CPU during inference.
                        crate::log_summary::VOICE_ACTIVE.store(true,
                            std::sync::atomic::Ordering::Relaxed);
                        let text = match transcribe_with_dual_model(&snap16k) {
                            Ok(t) => {
                                crate::log_summary::VOICE_ACTIVE.store(false,
                                    std::sync::atomic::Ordering::Relaxed);
                                t
                            }
                            Err(e) => {
                                crate::log_summary::VOICE_ACTIVE.store(false,
                                    std::sync::atomic::Ordering::Relaxed);
                                crate::clog!("wake-listen", "whisper failed: {}", e);
                                continue;
                            }
                        };
                        let cleaned = strip_whisper_artifacts(&text);
                        crate::clog!("wake-listen", "whisper said: {:?}", cleaned);
                        // Classification first
                        let mut category = classify_wake_phrase(&cleaned);
                        // ACOUSTIC OVERRIDE: if the audio is very
                        // close to the enrolled wake-word fingerprint
                        // (sim > 0.75), trust it even if the text
                        // classifier didn't match. This handles cases
                        // where whisper mistranscribes the user's
                        // own voice into something unrecognized but
                        // the audio shape is unmistakable.
                        if category.is_none() && standard_registered && acoustic > 0.75 {
                            crate::clog!("wake-listen", "text classify failed but acoustic={:.3} — trusting acoustic, firing standard wake",
                                acoustic);
                            category = Some(WakeCategory::Standard);
                        }
                        match category {
                            Some(WakeCategory::Standard) => {
                                wakes_fired += 1;
                                WAKE_FIRED.store(wakes_fired,
                                    std::sync::atomic::Ordering::Relaxed);
                                // v0.1.121: extract trailing command if any.
                                // "{keyword}, open terminal" → remainder
                                // "open terminal", which the QML side
                                // will treat as an immediate recognized
                                // utterance (no fresh recording needed).
                                let remainder = strip_standard_wake_prefix(&cleaned)
                                    .unwrap_or_default();
                                if !remainder.is_empty() {
                                    crate::clog!("wake-listen", "*** WAKE FIRED (standard) *** ({} total) — inline command: {:?}",
                                        wakes_fired, remainder);
                                } else {
                                    crate::clog!("wake-listen", "*** WAKE FIRED (standard) *** ({} total)",
                                        wakes_fired);
                                }
                                // Update wake-word profile from fresh audio.
                                crate::audio_profile::record_wake_word(&snap16k);
                                if let Some(q) = WAKE_EVENTS.get() {
                                    if let Ok(mut g) = q.lock() {
                                        g.push_back((snap16k, remainder));
                                        while g.len() > 4 { g.pop_front(); }
                                    }
                                }
                                // v0.1.750: cheap signal so the QML poll can
                                // skip the mutex lock entirely when idle.
                                crate::event_signal::raise(crate::event_signal::Channel::Wake);
                            }
                            Some(WakeCategory::AskAi) => {
                                let question = match strip_ai_wake_prefix(&cleaned) {
                                    Some(q) => q,
                                    None => {
                                        crate::clog!("wake-listen", "AI wake matched but no question text after prefix");
                                        continue;
                                    }
                                };
                                wakes_fired += 1;
                                WAKE_FIRED.store(wakes_fired,
                                    std::sync::atomic::Ordering::Relaxed);
                                crate::clog!("wake-listen", "*** ASK-AI FIRED *** question={:?}", question);
                                if let Some(q) = ASK_AI_EVENTS.get() {
                                    if let Ok(mut g) = q.lock() {
                                        g.push_back(question);
                                        while g.len() > 4 { g.pop_front(); }
                                    }
                                }
                                crate::event_signal::raise(crate::event_signal::Channel::AskAi);
                            }
                            None => {
                                // Acoustically close but didn't say
                                // any wake phrase. Don't fire.
                                continue;
                            }
                        }
                    }
                }
            }
        }
    });
}

#[cfg(feature = "voice")]
fn ensure_models_loaded() -> Result<(), String> {
    // Tier loading policy (changed in v0.1.66 after multi-tier proved too
    // expensive on typical CPU desktops):
    //   - WEB2QML_FORCE_MODEL=tiny|base|turbo: load that tier ONLY
    //   - WEB2QML_PARALLEL_MODELS=1: load every tier whose file exists
    //     and run the parallel dispatcher
    //   - default: load Base if available, else Tiny. Single tier, no
    //     parallel dispatch. This is robust on midrange hardware where
    //     three concurrent whisper contexts can blow the CPU budget
    //     (turning a 0.5s tiny inference into a 4-12s tiny inference
    //     because all three tiers contend for the same cores).
    //
    // The fallback path (when neither env var is set):
    //   Base is preferred over Tiny because it's the better quality/
    //   speed point on a single-tier setup. If Base file is missing,
    //   fall back to Tiny.
    let force = forced_tier();
    let parallel_opt_in = std::env::var("WEB2QML_PARALLEL_MODELS")
        .map(|v| v == "1" || v == "true" || v == "yes")
        .unwrap_or(false);

    let (load_tiny, load_base, load_large) = if let Some(t) = force {
        (t == ModelTier::Tiny, t == ModelTier::Base, t == ModelTier::HighAccuracy)
    } else if parallel_opt_in {
        // User explicitly asked for parallel — load every tier whose
        // file exists. The dispatcher will fan out.
        crate::clog!("whisper", "WEB2QML_PARALLEL_MODELS=1 — loading all available tiers");
        (true, true, true)
    } else {
        // Default: single tier. Prefer Base, fall back to Tiny.
        // We probe file presence first to decide which one to load.
        let base_present = model_file(ModelTier::Base).is_some();
        if base_present {
            (false, true, false)
        } else {
            (true, false, false)
        }
    };

    if load_tiny { load_tier(ModelTier::Tiny)?; }
    if load_base { load_tier(ModelTier::Base)?; }
    if load_large { load_tier(ModelTier::HighAccuracy)?; }

    // v0.1.116: bring up the whisper worker pool. Uses the Tiny
    // context (smallest, fastest — good for streaming jobs). 1 worker
    // by default; future setting could let the user bump to 2 for
    // multi-speaker concurrency at the cost of extra ~210MB.
    //
    // Reading WEB2QML_WHISPER_WORKERS lets the user opt in to 2
    // workers without rebuilding. Capped at 4 by the pool.
    // v0.1.124: prefer Tiny for the pool (fast) but fall back to
    // Base if user didn't install Tiny. Previously the pool init
    // was gated strictly on TINY_CTX, leaving the pool uninitialized
    // for Base-only users — every per-speaker submit failed with
    // "pool not initialized".
    let pool_ctx = TINY_CTX.get()
        .and_then(|m| m.lock().ok().and_then(|g| g.clone()))
        .or_else(|| BASE_CTX.get()
            .and_then(|m| m.lock().ok().and_then(|g| g.clone())));
    if let Some(ctx) = pool_ctx {
        let n = std::env::var("WEB2QML_WHISPER_WORKERS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1);
        crate::whisper_pool::init_with_tiny_ctx(ctx, n);
    }

    // Sanity check: at least one must be loaded for transcription to work.
    let any_loaded = TINY_CTX.get().map(|m| m.lock().map(|g| g.is_some()).unwrap_or(false)).unwrap_or(false)
        || BASE_CTX.get().map(|m| m.lock().map(|g| g.is_some()).unwrap_or(false)).unwrap_or(false)
        || LARGE_CTX.get().map(|m| m.lock().map(|g| g.is_some()).unwrap_or(false)).unwrap_or(false);
    if !any_loaded {
        return Err(format!(
            "No whisper model present. Run setup-voice.sh to download (default: tiny+base quantized).\n\
             Looked in {}/ggml-{{tiny,base}}.en{{,-q5_1}}.bin and ggml-large-v3-turbo*.bin",
            PathBuf::from(std::env::var("HOME").unwrap_or("/tmp".into()))
                .join(".config/web2qml/models").display()
        ));
    }
    Ok(())
}

/// Load one tier's model context if its file exists. No-op if already loaded.
/// Missing file is NOT an error — the dispatcher handles partial setups.
/// Returns Err only if the file exists but loading failed.
#[cfg(feature = "voice")]
fn load_tier(tier: ModelTier) -> Result<(), String> {
    let cell = match tier {
        ModelTier::Tiny => &TINY_CTX,
        ModelTier::Base => &BASE_CTX,
        ModelTier::HighAccuracy => &LARGE_CTX,
    };
    let lock = cell.get_or_init(|| Mutex::new(None));
    let mut g = lock.lock().map_err(|e| format!("CTX lock: {}", e))?;
    if g.is_some() { return Ok(()); }
    let path = match model_file(tier) {
        Some(p) => p,
        None => {
            // No file for this tier — silently skip.
            return Ok(());
        }
    };
    crate::clog!("whisper", "loading {:?} from {}", tier, path.display());
    let start = std::time::Instant::now();
    // v0.1.283: was `path.to_str().unwrap()`. Would panic on a
    // non-UTF-8 path (extremely unlikely — would mean user's $HOME
    // or model dir has non-UTF-8 bytes — but defensive matters
    // because panic on startup is fatal whereas error-return
    // surfaces a clean message).
    let path_str = path.to_str()
        .ok_or_else(|| format!("Whisper init {:?}: model path is not UTF-8 ({})",
            tier, path.display()))?;
    let ctx = whisper_rs::WhisperContext::new_with_params(
        path_str,
        whisper_rs::WhisperContextParameters::default()
    ).map_err(|e| format!("Whisper init {:?}: {:?}", tier, e))?;
    *g = Some(Arc::new(ctx));
    crate::clog!("whisper", "{:?} ready in {:?}", tier, start.elapsed());
    Ok(())
}

/// Back-compat shim. Old call sites used this name. Routes to the new dual-
/// model loader.
#[cfg(feature = "voice")]
fn ensure_whisper_loaded() -> Result<(), String> {
    ensure_models_loaded()
}

/// v0.1.856: list the names of all available capture (input) devices, for the
/// multi-mic → pane binding UI. Stable id used by MicBindings is the device
/// name. Empty vec if the host enumerates nothing.
#[cfg(feature = "voice")]
pub fn input_device_names() -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devs) => devs.filter_map(|d| d.name().ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Spawn a persistent cpal input stream that fills RING_BUFFER continuously.
/// Stream runs on its own thread for the lifetime of the app.
#[cfg(feature = "voice")]
fn ensure_capture_running() -> Result<(), String> {
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
                        crate::corpus_recorder::push_samples(data, sr);
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
                        crate::corpus_recorder::push_samples(&tmp, sr);
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
                        crate::corpus_recorder::push_samples(&tmp, sr);
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
        ensure_preview_thread();
        clear_partial_preview();
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
    // Tell log_summary to defer — voice is about to take CPU.
    // RAII-style: we'll clear it on every return path via the
    // closure below.
    crate::log_summary::VOICE_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
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
    // Clear the deferral flag — summary worker can resume.
    crate::log_summary::VOICE_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    result
}

#[cfg(feature = "voice")]
#[allow(dead_code)] // public API kept for future use; current code uses _inner directly
fn record_and_transcribe_with_audio() -> Result<(String, Vec<f32>), String> {
    record_and_transcribe_inner()
}

#[cfg(feature = "voice")]
#[allow(dead_code)] // public API kept for future use; current code uses _inner directly
fn record_and_transcribe() -> Result<String, String> {
    record_and_transcribe_inner().map(|(text, _)| text)
}

#[cfg(feature = "voice")]
fn record_and_transcribe_inner() -> Result<(String, Vec<f32>), String> {
    use std::time::{Duration, Instant};

    // Make sure stream is running and capture is enabled
    ensure_capture_running()?;
    begin_capture();

    // VAD-based recording with ADAPTIVE thresholds.
    //
    // Stop criteria:
    //   - Hard cap: 12 seconds (was 7s — too short for spelling URLs)
    //   - Min record: 1.2s before any early-stop is allowed
    //   - Early stop: after min_record AND we've heard speech, IF we observe
    //                 silence_to_stop ms of low-energy audio.
    //
    // Adaptive calibration: the first 0.5s of detected speech sets the
    // "speech baseline" (average RMS during active speech). The silence
    // threshold then becomes baseline * 0.35, with a floor at 0.005.
    // This adapts to quiet speakers, loud speakers, distant mics, close mics.
    //
    // Silence tolerance: 1100ms. Longer than before because:
    //   - Spelled letters ("ay ... el ... ex") have 400-700ms pauses
    //   - Thinking pauses ("open... uh... github") need to not cut off
    //   - DTW-aware: extra silence before whisper costs ~50ms; cutting off mid-word costs reparse.
    let start = Instant::now();
    let max_record = Duration::from_secs_f32(12.0);
    // v0.1.122: one-shot fast-finalize for confirm-listen flow (yes/no).
    // QML sets the FAST_FINALIZE flag right before voice_listen_dual,
    // we read+clear here and tighten the VAD timings for this single
    // recording.
    let fast = FAST_FINALIZE.swap(false, std::sync::atomic::Ordering::SeqCst);
    let min_record = if fast {
        Duration::from_millis(600)
    } else {
        Duration::from_secs_f32(1.2)
    };
    // VAD tail silence: how long after the last speech sample we wait
    // before declaring the utterance over. v0.1.91 reduced from 1100ms
    // to 825ms (25% trim per user request) — most of the perceived-
    // latency cost between "I stopped talking" and "transcription
    // begins" comes from this silence wait. 825ms is still well above
    // a typical inter-word pause (200-400ms) but pulls the latency
    // budget tighter. If users report transcripts being cut mid-word,
    // dial back toward 950ms.
    //
    // v0.1.122: when fast-finalize is armed (confirm-listen yes/no
    // path), tail drops to 450ms. Short replies don't have
    // mid-utterance pauses.
    let silence_to_stop = if fast {
        Duration::from_millis(450)
    } else {
        Duration::from_millis(825)
    };
    if fast {
        crate::clog!("voice", "fast-finalize armed: min_record=600ms silence_to_stop=450ms");
    }

    // Initial threshold — used until we've calibrated.
    //
    // Adaptive: scales the hardcoded baseline by the user's measured
    // noise floor when one is available (audio_profile populates this
    // from bg-listen silence samples). The formula uses noise_floor *
    // 1.8 with a hard floor of 0.005 (so very-quiet rooms don't get
    // a near-zero threshold that would trigger on every breath).
    //
    // In a noisy environment (cafe, fan, crowd) the noise floor is
    // higher → threshold is higher → spurious speech detection drops.
    // In a quiet office the threshold stays near 0.005-0.008 like
    // before, no regression.
    let noise_floor = crate::audio_profile::noise_floor();
    let initial_speech_rms: f32 = if noise_floor > 0.0 {
        (noise_floor * 1.8).max(0.005).min(0.025)
    } else {
        0.008  // unchanged baseline when no noise data yet
    };
    eprintln!("Voice VAD: initial speech_rms={:.4} (noise_floor={:.4})",
        initial_speech_rms, noise_floor);
    let mut speech_rms = initial_speech_rms;
    let mut last_speech: Option<Instant> = None;
    let mut had_speech = false;

    // Calibration state: collect peak RMS during first ~0.6s of detected speech
    // to set adaptive threshold.
    let mut speech_rms_samples: Vec<f32> = Vec::with_capacity(16);
    let mut calibrated = false;
    let mut first_speech_at: Option<Instant> = None;

    loop {
        std::thread::sleep(Duration::from_millis(80));
        // User-cancelled: stop right away, drop the audio
        if CANCEL_FLAG.load(std::sync::atomic::Ordering::Relaxed) {
            CANCEL_FLAG.store(false, std::sync::atomic::Ordering::Relaxed);
            // Mark capture done so the buffer drains cleanly
            if let Some(ring) = RING_BUFFER.get() {
                if let Ok(mut g) = ring.lock() {
                    g.capturing = false;
                    let _ = g.drain_into_mono();  // discard
                }
            }
            return Err("cancelled by user".into());
        }
        // User force-finished (tapped "done"): stop now, KEEP audio
        if FINISH_FLAG.load(std::sync::atomic::Ordering::Relaxed) {
            FINISH_FLAG.store(false, std::sync::atomic::Ordering::Relaxed);
            eprintln!("Voice: user-finished at {:.1}s", start.elapsed().as_secs_f32());
            break;
        }
        let elapsed = start.elapsed();
        if elapsed >= max_record { break; }

        // Get last 100ms of audio for VAD
        let ring = RING_BUFFER.get().ok_or("Ring buffer not initialized")?;
        let (rms, zcr, src_rate, src_ch) = {
            let g = ring.lock().map_err(|e| format!("Ring lock: {}", e))?;
            let window_samples = (g.source_rate as usize / 10) * g.source_channels.max(1);
            let take = window_samples.min(g.samples.len());
            let start_idx = g.samples.len().saturating_sub(take);
            let mut sum_sq = 0.0f64;
            let mut n = 0u64;
            // v0.1.130: zero-crossing count over the same 100ms window.
            // Used as a coarse voicing gate alongside RMS. Speech ZCR
            // (per 100ms) typically falls 50-300 at 44.1kHz × 1ch;
            // sustained drums/bass lines have very low ZCR (long
            // periodic crossings); cymbals/static have very high
            // (thousands). Music sits somewhere depending on
            // material. This is a coarse filter — not a substitute
            // for a proper frequency-domain voicing detector — but
            // it knocks out the easy cases (subway rumble, kick
            // drums, sustained hum) with one extra comparison per
            // sample. No FFT, no allocation.
            //
            // v0.1.225 bugfix: when the source is stereo, the
            // interleaved buffer is L0 R0 L1 R1 L2 R2... Treating
            // this as a continuous mono signal for ZCR is wrong —
            // independent L/R signals create spurious "crossings"
            // at every channel boundary. On the user's MacBook
            // (44.1k stereo) this inflated ZCR from speech-band
            // (~50-300 per 100ms) to 2100-3000, blowing past the
            // 30..800 voicing gate and rejecting every voice
            // chunk as "non-voice noise."
            //
            // Fix: stride through one channel only (the left, by
            // convention). RMS sum is also restricted to that
            // channel so the energy estimate stays consistent
            // with the crossings analyzed.
            //
            // Previous sample's sign for crossing detection. Starts
            // at 0 so a leading positive sample doesn't false-count.
            let mut prev: f64 = 0.0;
            let mut crossings: u64 = 0;
            let stride = g.source_channels.max(1);
            let mut i = start_idx;
            while i < g.samples.len() {
                let s = g.samples[i] as f64;
                sum_sq += s * s; n += 1;
                if (s >= 0.0) != (prev >= 0.0) && n > 1 {
                    crossings += 1;
                }
                prev = s;
                i += stride;
            }
            let rms = if n > 0 { (sum_sq / n as f64).sqrt() as f32 } else { 0.0 };
            // ZCR normalized to crossings per 100ms (the window length).
            // At source_rate 44.1k, window is 4410 samples per channel
            // = 100ms regardless of channel count, since we now stride
            // by channels. n counts only the channel we sampled.
            let zcr = if n > 0 {
                (crossings as f32 / n as f32) * (g.source_rate as f32 / 10.0)
            } else { 0.0 };
            (rms, zcr, g.source_rate, g.source_channels)
        };
        let _ = (src_rate, src_ch);

        // v0.1.130: voicing gate. RMS passes the energy bar but ZCR
        // tells us whether that energy looks speech-shaped. Acceptable
        // speech band: 30 < ZCR < 800 crossings/sec at 44.1kHz mono.
        // The wide tolerance is intentional — strict ZCR rejects
        // legitimate speech that happens to have unusual phonetic
        // makeup (e.g. lots of fricatives push ZCR up, lots of
        // vowels push it down). We're filtering OBVIOUSLY non-speech.
        //
        // Below 30: sustained low-frequency hum (subway rumble, AC
        // noise, kick drums in adjacent room).
        // Above 800: high-frequency static, cymbals, electrical
        // interference.
        // Both bands could COINCIDE with speech in mixed scenes; we
        // accept that false negative rate as the cost of cleaner
        // VAD in noisy environments.
        let voicing_ok = zcr >= 30.0 && zcr <= 800.0;
        let is_speech = rms > speech_rms && voicing_ok;
        // v0.1.225: optional per-chunk VAD trace. Set
        // WEB2QML_VAD_DEBUG=1 to emit one line per VAD evaluation
        // showing the raw rms, zcr, gate thresholds, and decision.
        // Lets the user (or future-Claude reading a transcript)
        // see WHY chunks are being rejected, and tune the
        // thresholds against real captured audio rather than
        // synthetic test cases.
        //
        // Caveat: spams the log heavily — only enable for
        // diagnostic sessions, not normal use.
        if vad_debug_enabled() {
            crate::clog!("vad-debug", "rms={:.4} (thr={:.4}) zcr={:.0}/100ms (band 30..800) ch={} → {}",
                rms, speech_rms, zcr, src_ch,
                if is_speech { "ACCEPT" }
                else if !voicing_ok { "reject (zcr out of band)" }
                else { "reject (below rms threshold)" });
        }
        // Surface rejected-but-loud chunks once per VAD session so
        // we can see WHY a noisy environment isn't triggering speech.
        // Throttle to first occurrence to avoid log spam.
        if rms > speech_rms && !voicing_ok && first_speech_at.is_none() {
            eprintln!("Voice VAD: rejected loud chunk (rms={:.4} zcr={:.0}/s) — non-voice noise",
                rms, zcr);
        }
        if is_speech {
            last_speech = Some(Instant::now());
            had_speech = true;
            if first_speech_at.is_none() { first_speech_at = Some(Instant::now()); }

            // Calibration: collect samples for the first 600ms of speech, then
            // compute adaptive threshold.
            if !calibrated {
                speech_rms_samples.push(rms);
                if let Some(fsa) = first_speech_at {
                    if fsa.elapsed() >= Duration::from_millis(600) && speech_rms_samples.len() >= 4 {
                        // Use median of collected speech RMS as baseline (robust to outliers)
                        let mut sorted = speech_rms_samples.clone();
                        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                        let median = sorted[sorted.len() / 2];
                        // Threshold = 35% of median observed speech, floor at 0.005,
                        // cap at 0.022. The cap was 0.030 prior to v0.1.79 but real-
                        // world testing showed it was high enough to swallow quiet
                        // confirmation utterances ("yes", "no") spoken at lower volume
                        // than the original calibrating burst. 0.022 still rejects
                        // most non-speech ambient while letting natural-volume
                        // confirmations through.
                        let new_thresh = (median * 0.35).max(0.005).min(0.022);
                        eprintln!("Voice VAD: calibrated speech_rms = {:.4} (was {:.4}, median speech RMS {:.4})",
                            new_thresh, speech_rms, median);
                        speech_rms = new_thresh;
                        calibrated = true;
                    }
                }
            }
        }

        // Allow early stop only after min_record AND we've heard speech
        if elapsed >= min_record && had_speech {
            if let Some(t) = last_speech {
                if t.elapsed() >= silence_to_stop {
                    eprintln!("Voice: VAD stop at {:.1}s ({}ms silence, threshold {:.4})",
                        elapsed.as_secs_f32(), t.elapsed().as_millis(), speech_rms);
                    break;
                }
            }
        }
    }

    // Stop capturing and drain
    let ring = RING_BUFFER.get().ok_or("Ring buffer not initialized")?;
    let (raw_mono, source_rate) = {
        let mut g = ring.lock().map_err(|e| format!("Ring lock: {}", e))?;
        g.capturing = false;
        (g.drain_into_mono(), g.source_rate)
    };

    // Resample to 16kHz
    let mut audio = if source_rate == TARGET_RATE { raw_mono.clone() } else { resample(&raw_mono, source_rate, TARGET_RATE) };
    eprintln!("Voice: captured {} mono samples → {} 16kHz samples ({:.1}s)",
        raw_mono.len(), audio.len(), audio.len() as f32 / TARGET_RATE as f32);

    if audio.is_empty() {
        return Err("No audio captured".into());
    }

    // High-pass to kill rumble + DC. Significantly improves whisper
    // transcription on rooms with HVAC, traffic, fans, or 50/60Hz
    // mains buzz. Cheap (~1 mult per sample). Applied to a copy
    // because we ALSO want the unfiltered audio for the phonetic
    // fingerprint store later (filtering changes fingerprint
    // characteristics, and we want fingerprints to match raw mic
    // input not filtered audio).
    high_pass_80hz(&mut audio);
    // v0.1.783: optional gentle gain (opt-in WEB2QML_VOICE_GAIN=1). Same
    // conservative normalization as the background path.
    if std::env::var("WEB2QML_VOICE_GAIN").map(|v| v == "1").unwrap_or(false) {
        let g = normalize_gain(&mut audio);
        if g != 1.0 { crate::clog!("voice", "gain x{:.2}", g); }
    }
    if audio.len() < 8000 {
        return Err(format!("Too little audio: {} samples", audio.len()));
    }

    let max_amp = audio.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
    if max_amp < 0.001 {
        return Err(format!("Audio silent (max amp {:.6})", max_amp));
    }

    // Pad to whisper-friendly minimum 1s if needed
    let mut audio_padded = audio;
    while audio_padded.len() < TARGET_RATE as usize {
        audio_padded.push(0.0);
    }

    // v0.1.289 (Tier1-2): acoustic-index fast path. Before
    // dispatching to Whisper, query the acoustic_index for a
    // nearest-neighbor match. If one exists below the
    // threshold AND we're in PROMOTE mode (env var
    // WEB2QML_TIER1_PROMOTE=1), return the cached transcript
    // and SKIP Whisper entirely. Otherwise we run Whisper
    // anyway and log the (distance, cached_transcript,
    // whisper_transcript) tuple for tuning.
    //
    // Default behavior (SHADOW mode): no behavioral change.
    // Whisper always runs. Log lines accumulate so the user
    // can read journals to see where distances land relative
    // to actual Whisper output. Once the user observes
    // agreement at acceptable distances, they flip
    // WEB2QML_TIER1_PROMOTE=1 to enable the fast path.
    //
    // PROMOTE mode (env var set): skip Whisper when a tight
    // match exists. Currently uses 0.10 as the same-speaker
    // threshold (very conservative; tighter than the 0.15
    // value baked into find_best_by_fingerprint's
    // speaker-preferred short-circuit). Cross-speaker hits
    // get 0.20.
    let tier1_promote = crate::acoustic_index::promote_enabled();
    let current_speaker = crate::speakers::current_active_id();
    let prefer_speaker = if current_speaker.is_empty() { None } else { Some(current_speaker.as_str()) };
    let tier1_query = crate::acoustic_index::fingerprint(&audio_padded);
    let tier1_hit = if tier1_query.is_empty() {
        None
    } else {
        crate::acoustic_index::find_best_by_fingerprint(&tier1_query, prefer_speaker)
    };

    // Decide if Tier1 hit is good enough to SKIP Whisper.
    // v0.1.304: thresholds now runtime-tunable via
    // acoustic_index::set_promote_thresholds. Defaults 0.10
    // same-speaker / 0.20 cross-speaker (preserves v0.1.289
    // behavior); user can set via "set threshold to X" voice
    // intent based on the histogram suggestion.
    let (thresh_same, thresh_cross) = crate::acoustic_index::promote_thresholds();
    let tier1_skip_whisper = tier1_promote && tier1_hit.as_ref().map_or(false, |h| {
        let same_speaker = !current_speaker.is_empty() && h.speaker_id == current_speaker;
        let threshold = if same_speaker { thresh_same } else { thresh_cross };
        h.distance < threshold && h.count >= 2
    });

    if tier1_skip_whisper {
        let h = tier1_hit.as_ref().unwrap();
        crate::clog!("tier1", "PROMOTE hit: {:?} (distance={:.3}, count={}, speaker={:?}) — skipping Whisper",
            h.transcript, h.distance, h.count, h.speaker_id);
        crate::acoustic_index::record_hit(&h.speaker_id, h.unix_ms);
        return Ok((h.transcript.clone(), audio_padded));
    }

    // Transcribe (models already loaded via prewarm). Dual-model dispatch
    // sends audio to BOTH tiny and base in parallel and picks the result
    // per the policy in transcribe_with_dual_model.
    ensure_models_loaded()?;
    let trans_start = Instant::now();
    let raw_cleaned = transcribe_with_dual_model(&audio_padded)?;
    // Trim repetition loops. Whisper occasionally emits "command, run
    // cargo test, run cargo test, run cargo test, ..." when audio
    // includes silence after a confident phrase — its decoder enters a
    // fixation loop. Detect the loop and truncate. The result is just
    // the unique prefix, no padding.
    let cleaned = trim_hallucination_loop(&raw_cleaned);
    let xc_secs = trans_start.elapsed().as_secs_f32();
    eprintln!("Voice: transcribed in {:.3}s: {:?}", xc_secs, cleaned);

    // v0.1.289 (Tier1-2): shadow-mode log + feedback loop.
    // - Log the Tier1 hit vs Whisper agreement (so the user
    //   can tune the threshold based on real data).
    // - Add the (audio, whisper_transcript) pair to the
    //   acoustic_index so future utterances can match it.
    if let Some(h) = tier1_hit.as_ref() {
        let matched = h.transcript.trim().eq_ignore_ascii_case(cleaned.trim());
        let agreement = if matched { "MATCH" } else { "DIVERGE" };
        crate::clog!("tier1", "shadow: distance={:.3} cached={:?} whisper={:?} → {} (speaker={:?}, count={})",
            h.distance, h.transcript, cleaned, agreement, h.speaker_id, h.count);
        // v0.1.300 (Tier1-3 tuning): record into histogram so
        // the user can read aggregated stats later via
        // "tier 1 histogram" voice intent.
        crate::acoustic_index::record_shadow_observation(h.distance, matched);
    } else {
        crate::clog!("tier1", "no hit (index empty or fingerprint failed); whisper produced {:?}", cleaned);
    }

    // Feedback: store this (fingerprint, transcript) so next
    // time the same utterance shape arrives we have an
    // exemplar. Skipped when transcript is suspiciously short
    // or empty — those aren't reliable cache seeds.
    if !tier1_query.is_empty() && cleaned.len() >= 2 && cleaned.len() <= 200 {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64).unwrap_or(0);
        // Speaker id: same as the find_best query. If unknown,
        // tag with "_unknown" so the entry doesn't pollute any
        // real speaker's pool.
        let spk_id = if current_speaker.is_empty() {
            "_unknown".to_string()
        } else {
            current_speaker.clone()
        };
        crate::acoustic_index::add_exemplar(crate::acoustic_index::Exemplar {
            unix_ms: now_ms,
            speaker_id: spk_id,
            transcript: cleaned.clone(),
            mfcc_mean: tier1_query.clone(),
            count: 1,
            last_used_ms: now_ms,
        });
    }

    Ok((cleaned, audio_padded))
}

/// Trim a whisper hallucination-loop. Detection: if the same N-token
/// substring appears 3+ times in the output, trim everything after
/// the second occurrence.
///
/// Examples:
///   "run cargo test, run cargo test, run cargo test" → "run cargo test"
///   "the the the the world" → "the world" (the leading "the" loops trimmed)
///
/// Conservative: if we can't find a clear loop, return the input as-is.
/// The loop must repeat at least 3 times to qualify (2 might be a
/// natural rhetorical structure: "you can do this, do that").
fn trim_hallucination_loop(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() { return String::new(); }
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.len() < 6 { return s.to_string(); }
    // Try shingle sizes 1..=4. For each size, count how many
    // consecutive identical shingles appear. If >= 3, truncate
    // after the FIRST shingle (so "X X X" becomes "X").
    for window in 1..=4 {
        if window * 3 > words.len() { break; }
        // Scan for runs of identical shingles starting from the
        // beginning. Practically, hallucinations loop early in the
        // output so we don't bother scanning later positions.
        for start in 0..(words.len() - window * 3) {
            let first_shingle = &words[start..start + window];
            let mut runs = 1;
            let mut pos = start + window;
            while pos + window <= words.len()
                && &words[pos..pos + window] == first_shingle {
                runs += 1;
                pos += window;
            }
            if runs >= 3 {
                // Trim. Keep words[..start + window], discard rest.
                let kept: Vec<&str> = words[..start + window].iter().copied().collect();
                let trimmed = kept.join(" ");
                crate::clog!("whisper", "trimmed hallucination loop ({}x repetition of '{}'): {:?} -> {:?}",
                    runs, first_shingle.join(" "), s, trimmed);
                // Also strip any trailing comma / punctuation we kept.
                return trimmed.trim_end_matches(|c: char|
                    c == ',' || c == '.' || c == ' ').to_string();
            }
        }
    }
    s.to_string()
}

/// Run transcription using whichever models are loaded (tiny, base, or both)
/// and return the chosen text result.
///
/// Single-model case: dispatch to that model, await result.
///
/// Dual-model case: dispatch to BOTH in worker threads. Tiny finishes
/// first (always faster). When tiny returns we wait up to BASE_GRACE_MS
/// for base. If base returns in that window, use base's better result;
/// else use tiny.
///
/// We do NOT cancel base when we abandon it — let it finish in the
/// background and warm caches for the next call. That's why each context
/// is wrapped in Arc — both threads need to hold a reference and the
/// owner of the data outlives both.
#[cfg(feature = "voice")]
/// When set true (via voice::set_force_tiny_tier from QML), the next
/// transcription call uses ONLY the Tiny model regardless of which
/// other models are loaded. The flag is one-shot — cleared on use —
/// so QML doesn't have to remember to unset it.
///
/// Use case: yes/no confirmation popups. After the user has
/// transcribed e.g. "open browser tab" and we surface 3 candidate
/// files, the FOLLOW-UP utterance is almost certainly going to be
/// short (yes/no/cancel/the candidate name). Tiny is fast enough
/// to do this 200-400ms instead of Base's 1000ms+, so the popup
/// closes feeling responsive.
///
/// We don't use this for normal voice queries because Tiny's accuracy
/// drops on longer phrases. The hint is opt-in, set ONLY by QML
/// paths that know the next utterance will be short.
#[cfg(feature = "voice")]
pub static FORCE_TINY_TIER: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(feature = "voice")]
pub fn set_force_tiny_tier(v: bool) {
    FORCE_TINY_TIER.store(v, std::sync::atomic::Ordering::SeqCst);
}

/// One-shot "fast finalize" flag (v0.1.122). When QML knows the
/// next utterance will be short (yes/no/cancel/single-word
/// confirmation), it sets this to true. record_and_transcribe_inner
/// reads + clears it on entry and adjusts:
///   - silence_to_stop: 825ms → 450ms
///   - min_record: 1200ms → 600ms
///
/// Result: confirm-listen flow shaves ~600-750ms off the perceived
/// "I said yes, why is it still waiting" delay. Single-word
/// utterances finish ~1.2s instead of ~1.9s on a typical recording.
///
/// Trade-off: if the user actually says a long sentence during what
/// QML expected to be a confirmation (rare), VAD might cut off
/// prematurely. The same flag is one-shot — bigger commands hit the
/// normal voice path with the standard timings.
#[cfg(feature = "voice")]
pub static FAST_FINALIZE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(feature = "voice")]
pub fn set_fast_finalize(v: bool) {
    FAST_FINALIZE.store(v, std::sync::atomic::Ordering::SeqCst);
}

/// v0.1.131: configurable preview cadences. Defaults match the
/// hardcoded values from v0.1.116 (250ms phonetic, 1500ms whisper).
/// QML reads/writes these via voice_set_phonetic_interval_ms /
/// voice_set_whisper_interval_ms so users can back off the cadence
/// on slower devices.
///
/// Bounds enforced at setter time: phonetic 100..=2000ms, whisper
/// 500..=5000ms. Phonetic floor at 100ms because anything faster
/// burns CPU without UI benefit (QML polls at ~150ms anyway). Whisper
/// ceiling at 5s because anything slower defeats the purpose of
/// streaming preview (just wait for final).
///
/// Read each loop iteration so changes take effect on the next tick
/// without needing a thread restart.
#[cfg(feature = "voice")]
pub static PHONETIC_INTERVAL_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(250);

#[cfg(feature = "voice")]
pub static WHISPER_INTERVAL_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1500);

#[cfg(feature = "voice")]
pub fn set_phonetic_interval_ms(v: u64) {
    let clamped = v.clamp(100, 2000);
    PHONETIC_INTERVAL_MS.store(clamped, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(feature = "voice")]
pub fn set_whisper_interval_ms(v: u64) {
    let clamped = v.clamp(500, 5000);
    WHISPER_INTERVAL_MS.store(clamped, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(feature = "voice")]
pub fn phonetic_interval_ms() -> u64 {
    PHONETIC_INTERVAL_MS.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(feature = "voice")]
pub fn whisper_interval_ms() -> u64 {
    WHISPER_INTERVAL_MS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Initialize cadences from env vars (called from main at startup).
/// WEB2QML_PHONETIC_MS and WEB2QML_WHISPER_MS override defaults.
#[cfg(feature = "voice")]
pub fn init_preview_cadence_from_env() {
    if let Ok(s) = std::env::var("WEB2QML_PHONETIC_MS") {
        if let Ok(n) = s.parse::<u64>() {
            set_phonetic_interval_ms(n);
            crate::clog!("voice-preview", "phonetic interval set to {}ms (env)",
                phonetic_interval_ms());
        }
    }
    if let Ok(s) = std::env::var("WEB2QML_WHISPER_MS") {
        if let Ok(n) = s.parse::<u64>() {
            set_whisper_interval_ms(n);
            crate::clog!("voice-preview", "whisper interval set to {}ms (env)",
                whisper_interval_ms());
        }
    }
}

// v0.1.171 build fix: gate the dual-model dispatcher under the voice
// feature, same as run_one and the CTX statics it depends on. Without
// this gate, building without voice fails to compile because the
// function body references FORCE_TINY_TIER, TINY_CTX/BASE_CTX/LARGE_CTX,
// whisper_rs, and run_one — all of which are voice-feature-only.
#[cfg(feature = "voice")]
fn transcribe_with_dual_model(audio: &[f32]) -> Result<String, String> {
    use std::sync::mpsc;
    use std::time::{Instant, Duration};

    // Force-tiny check: when QML armed this for a yes/no confirmation,
    // bypass the dispatch loop entirely and just call Tiny. One-shot:
    // we clear the flag here so subsequent normal queries get the full
    // priority cascade.
    let force_tiny = FORCE_TINY_TIER.swap(false, std::sync::atomic::Ordering::SeqCst);
    if force_tiny {
        if let Some(tiny_arc) = TINY_CTX.get()
            .and_then(|m| m.lock().ok().and_then(|g| g.clone()))
        {
            crate::clog!("voice", "force-tiny tier (yes/no context)");
            return run_one(&tiny_arc, audio, ModelTier::Tiny);
        }
        crate::clog!("voice", "force-tiny requested but Tiny not loaded — falling through");
    }

    // v0.1.280 (voice speed): adaptive grace window. Short
    // utterances (< 2s of audio) are commands like "scroll
    // down", "open settings", "undo" — Tiny is reliable on
    // these and waiting up to 1000ms for Base to overtake is
    // mostly wasted latency. Longer utterances are sentences
    // (dictation, URLs spelled out, longer commands) where
    // Base's higher accuracy matters and the grace is worth
    // it.
    //
    // Audio is 16kHz mono f32. 2 seconds = 32000 samples.
    //
    // Tuning:
    //   ≤ 2s : 300ms  — barely-noticeable grace, but still
    //                   gives Base a fighting chance if it's
    //                   already in flight on a fast machine.
    //   2-5s : 700ms  — moderate (mixed shorter sentences).
    //   > 5s : 1000ms — original behavior for dictation-like.
    let grace_ms: u64 = if audio.len() <= 32_000 {
        300
    } else if audio.len() <= 80_000 {
        700
    } else {
        1000
    };

    // Collect all loaded contexts in priority order: HighAccuracy beats
    // Base which beats Tiny. We dispatch to all and pick the highest-
    // priority result available within the grace window.
    let pairs: Vec<(ModelTier, Arc<whisper_rs::WhisperContext>)> = [
        (ModelTier::HighAccuracy, &LARGE_CTX),
        (ModelTier::Base, &BASE_CTX),
        (ModelTier::Tiny, &TINY_CTX),
    ].iter().filter_map(|(tier, cell)| {
        cell.get()
            .and_then(|m| m.lock().ok().and_then(|g| g.clone()))
            .map(|arc| (*tier, arc))
    }).collect();

    if pairs.is_empty() {
        return Err("No model loaded".into());
    }

    // Single-tier fast path — no thread fan-out.
    if pairs.len() == 1 {
        let (tier, ctx) = &pairs[0];
        return run_one(ctx, audio, *tier);
    }

    // Multi-tier: dispatch every loaded tier in parallel, collect by
    // priority. Tier priority is the index in `pairs` — index 0 wins.
    let (tx, rx) = mpsc::channel::<(usize, ModelTier, Result<String, String>)>();
    let t_start = Instant::now();
    for (idx, (tier, ctx)) in pairs.iter().enumerate() {
        let tx = tx.clone();
        let ctx = ctx.clone();
        let audio_v = audio.to_vec();
        let tier = *tier;
        std::thread::spawn(move || {
            let r = run_one(&ctx, &audio_v, tier);
            let _ = tx.send((idx, tier, r));
        });
    }
    drop(tx); // Original tx dropped; channel closes when all spawned tx clones finish.

    // Result collection. We track the best-priority successful result
    // seen so far. Strategy:
    //   1. Wait for the FIRST result to arrive (no timeout).
    //   2. Note which tier landed and its priority index.
    //   3. If it's the best tier (idx 0), return immediately — nothing
    //      better is coming.
    //   4. Otherwise wait up to GRACE_MS for any higher-priority tier
    //      to land. If one does, use it. If not, return the best we have.
    let mut best: Option<(usize, String)> = None;
    let first = match rx.recv() {
        Ok(r) => r,
        Err(_) => return Err("All models failed (channel closed)".into()),
    };
    crate::clog!("whisper", "{:?} returned at +{:.3}s",
        first.1, t_start.elapsed().as_secs_f32());
    match first.2 {
        Ok(text) => best = Some((first.0, text)),
        Err(e) => crate::clog!("whisper", "{:?} failed: {}", first.1, e),
    }
    // If the first result is the BEST tier (idx 0), return immediately.
    // No grace needed because nothing better is in flight.
    if let Some((0, text)) = &best {
        crate::clog!("whisper", "using {:?} (best tier, arrived first)", pairs[0].0);
        return Ok(text.clone());
    }

    // Grace loop: keep collecting results, upgrade `best` when a higher-
    // priority result arrives. Stop when we hit GRACE_MS from t_start
    // OR when we've heard from all tiers OR when we have idx 0.
    let deadline = t_start + Duration::from_millis(grace_ms);
    loop {
        let now = Instant::now();
        if now >= deadline { break; }
        let timeout = deadline - now;
        match rx.recv_timeout(timeout) {
            Ok((idx, tier, r)) => {
                crate::clog!("whisper", "{:?} returned at +{:.3}s",
                    tier, t_start.elapsed().as_secs_f32());
                match r {
                    Ok(text) => {
                        let cur_idx = best.as_ref().map(|(i, _)| *i).unwrap_or(usize::MAX);
                        if idx < cur_idx {
                            best = Some((idx, text));
                            if idx == 0 {
                                // Top tier landed in the grace window — done.
                                crate::clog!("whisper", "using {:?} (top tier, within grace)", tier);
                                return Ok(best.unwrap().1);
                            }
                        }
                    }
                    Err(e) => crate::clog!("whisper", "{:?} failed: {}", tier, e),
                }
            }
            Err(_) => break, // Timeout or channel closed.
        }
    }

    match best {
        Some((idx, text)) => {
            crate::clog!("whisper", "using {:?} (grace window expired)", pairs[idx].0);
            Ok(text)
        }
        None => Err("All models failed".into()),
    }
}

/// Run one transcription on one whisper context. Builds params, creates
/// state, runs, collects segments, strips artifacts. Same as before but
/// factored out so dual dispatch can call it from worker threads.
#[cfg(feature = "voice")]
fn run_one(ctx: &whisper_rs::WhisperContext, audio: &[f32], tier: ModelTier) -> Result<String, String> {
    let mut params = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(4);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    // v0.1.110: configurable language. Default "en". For non-English
    // users, set WEB2QML_VOICE_LANG (e.g. "es", "fr", "ja"). Only
    // takes effect if a multilingual model is loaded — tiny.en /
    // base.en are English-only and will ignore non-"en" requests.
    //
    // We pass the configured code straight to whisper. Whisper falls
    // back to auto-detect if the code is unrecognized.
    let lang_code = crate::voice_lang::whisper_lang_code();
    params.set_language(lang_code);
    params.set_no_context(true);
    params.set_single_segment(false);
    params.set_suppress_nst(true);
    // Initial prompt: heavily English-biased. For non-English users
    // this prompt actually HARMS recognition by anchoring whisper on
    // English. Only set the prompt when we're in English mode.
    if crate::voice_lang::current() == "en" {
        params.set_initial_prompt(
            "open github, navigate to apple.com, search for rust async, scroll down, \
             go back, switch to terminal, run git status, alx.sh, news.ycombinator.com, \
             lore.kernel.org, sourcehut, web2qml, ollama, tinyllama, helix editor, \
             show history, show menu, find on page, copy URL, edit URL, A B C D E F, \
             cd tilde slash code, cd tilde slash documents slash work, \
             cd dot dot slash src, cd slash etc slash hosts, \
             go to tilde slash projects, navigate to tilde slash downloads, \
             change directory to slash usr slash local slash bin, \
             control C, control Z, escape colon W Q, save and quit, \
             exit helix, quit vim, kill the running process, \
             open first error, open next error, open next warning, \
             show second commit, show third one, run cargo build, run cargo test"
        );
    }

    // Take the cached state for this tier (or None on first call).
    // We hold the Mutex across the inference to serialize within the
    // tier — concurrent foreground+bg calls on the same tier would
    // race state internals otherwise. Different tiers run independent
    // inferences in parallel because they have separate state caches.
    let cache = match tier {
        ModelTier::Tiny         => &TINY_STATE,
        ModelTier::Base         => &BASE_STATE,
        ModelTier::HighAccuracy => &LARGE_STATE,
    };
    let cell = cache.get_or_init(|| Mutex::new(None));
    let mut guard = cell.lock().map_err(|e| format!("state lock {:?}: {:?}", tier, e))?;

    // Lazy-init the state on first call; reuse on subsequent calls.
    // This is the speedup vs creating a fresh state every time —
    // saves the ~210MB allocation visible in whisper_init_state logs.
    let mut state = match guard.take() {
        Some(s) => s,
        None => ctx.create_state().map_err(|e| format!("State {:?}: {:?}", tier, e))?,
    };

    let result = state.full(params, audio).map_err(|e| format!("Transcribe {:?}: {:?}", tier, e));

    // Always put the state back, even on error — we want to reuse
    // it even after a failed inference (the state's buffers are
    // still valid; whisper-rs resets them on next .full() call).
    let n = state.full_n_segments();
    let text: String = (0..n)
        .filter_map(|i| state.get_segment(i))
        .filter_map(|seg| seg.to_str().ok().map(|s| s.to_string()))
        .collect::<Vec<_>>()
        .join(" ");
    *guard = Some(state);
    drop(guard);

    result?;
    Ok(strip_whisper_artifacts(text.trim()))
}

// v0.1.171: removed a `#[cfg(not(feature = "voice"))] pub fn
// begin_capture() {}` stub here. The full version of begin_capture
// (line ~2752) is unconditionally defined; its body is gated with
// inner `#[cfg(feature = "voice")]`, so under no-voice it compiles
// to an empty function naturally. The stub here was a duplicate
// definition triggering E0428 on no-voice builds.


/// Remove non-speech tokens that whisper sometimes emits even with suppress_nst
fn strip_whisper_artifacts(s: &str) -> String {
    // Trim trailing/leading commas and stray punctuation that whisper
    // routinely adds. Examples: ", DNF, search python" → "DNF search
    // python", "test," → "test", ", home page," → "home page". Trailing
    // commas are particularly dangerous because they get appended to
    // shell command arguments when the recognized text is sent through.
    //
    // We do NOT strip mid-string commas — those CAN be intentional in
    // sentences like "list X, Y, Z". The pipe-chain parser handles the
    // ", ... | ..." case separately.
    let mut out = s.to_string();
    let trim_chars: &[char] = &[',', '.', '!', '?', ';', ':', ' ', '\t', '\n', '\r'];
    out = out.trim_matches(trim_chars).to_string();
    // After trim, also collapse the very-common whisper pattern ", word"
    // → "word" at the start (whisper sometimes encodes a quiet breath
    // as a leading comma + space + the actual word). The trim above
    // already handles plain ", " but if there's whitespace between the
    // comma and the next word that survived, normalize it here.
    while out.starts_with(", ") || out.starts_with(",  ") {
        out = out[2..].trim_start().to_string();
    }
    let s = out.as_str();
    let mut out = s.to_string();
    let artifacts = [
        "[BLANK_AUDIO]", "[NO_SPEECH]", "(silence)", "[silence]",
        "[Music]", "[music]", "[MUSIC]", "(music)",
        "(speaking in foreign language)", "[speaking in foreign language]",
        "(coughing)", "(laughing)", "(applause)", "[Applause]",
        "(typing)", "(door closing)", ". . .", "...",
    ];
    for a in &artifacts {
        out = out.replace(a, "");
    }
    let out = out.trim();

    // Whisper hallucinates leading discourse markers when the audio starts with
    // non-speech sound (chime tail, room noise, breath). These are NOT real
    // speech — strip them. Only strips at the start, only when followed by space.
    // Conservative: capitalised "And " stays (could be a real sentence start),
    // we only strip lowercase variants which whisper emits for hallucinations.
    let leading_hallucinations = [
        "and ", "And ", "so ", "So ", "uh ", "Uh ", "um ", "Um ",
        "okay ", "Okay ", "you ", "You ",
    ];
    let mut cleaned = out.to_string();
    // Strip up to 2 leading hallucination tokens (sometimes whisper emits "And so ...")
    for _ in 0..2 {
        let mut stripped = false;
        for h in &leading_hallucinations {
            if let Some(rest) = cleaned.strip_prefix(h) {
                eprintln!("Voice: stripping whisper hallucination prefix {:?}", h);
                cleaned = rest.to_string();
                stripped = true;
                break;
            }
        }
        if !stripped { break; }
    }
    // Path-token translation. Whisper transcribes paths as English words
    // ("tilde slash code slash kwin") because that's how people speak them.
    // Translate these into actual path syntax (~/Code/kwin) so all downstream
    // parsers see clean text. This is the right layer for this transform —
    // doing it here means every consumer (cd matcher, cmd-index, entity
    // resolver) gets clean input without each one re-implementing the same
    // English-to-path mapping. It also positions us for a custom voice model
    // later: we can train a model that natively outputs ~/Code instead of
    // "tilde slash code", and at that point this pass becomes a no-op.
    let cleaned = translate_path_tokens(&cleaned);
    // Normalize stray spaces around dots in numeric sequences. Whisper
    // routinely transcribes "one ninety two dot one sixty dot two dot one
    // seventy" as "192 .160.2.170" — a number, a space, then ".160.2.170"
    // glued together. We want IP addresses, version numbers, and similar
    // numeric-dot sequences to come out as "192.160.2.170".
    //
    // Rule: collapse "<digit><spaces>.<digit>" → "<digit>.<digit>" and
    // "<digit>.<spaces><digit>" → "<digit>.<digit>". Only when both sides
    // are digits — preserves prose like "I have 5 . The next one..." (which
    // a user wouldn't say anyway, but the constraint keeps us safe).
    let cleaned = normalize_numeric_dots(&cleaned);

    cleaned.trim().to_string()
}

fn normalize_numeric_dots(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        // Look for: digit, then whitespace, then '.', then digit → emit "X.Y"
        if c.is_ascii_digit() && i + 1 < bytes.len() {
            // Find the end of any whitespace gap
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') { j += 1; }
            if j < bytes.len() && bytes[j] == b'.' && j + 1 < bytes.len()
                && bytes[j+1].is_ascii_digit() && j > i + 1 {
                // Stray-space-before-dot pattern. Collapse the gap.
                out.push(c as char);
                out.push('.');
                i = j + 1;
                continue;
            }
        }
        // Also: digit, then '.', then whitespace, then digit → "X.Y"
        if c.is_ascii_digit() && i + 2 < bytes.len() && bytes[i+1] == b'.'
            && (bytes[i+2] == b' ' || bytes[i+2] == b'\t') {
            let mut j = i + 2;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') { j += 1; }
            if j < bytes.len() && bytes[j].is_ascii_digit() {
                out.push(c as char);
                out.push('.');
                i = j;
                continue;
            }
        }
        out.push(c as char);
        i += 1;
    }
    out
}

/// Translate sequences of path-tokens in spoken-English form into actual
/// path syntax. "tilde slash code slash kwin" → "~/Code/kwin".
/// "dot dot slash src" → "../src". Only triggers within sequences of
/// path-tokens — surrounding text (verbs, articles, command names) passes
/// through untouched.
///
/// Recognized tokens (lowercase compared, original case preserved for
/// pass-through tokens):
///   tilde       → ~
///   slash       → /
///   backslash   → \
///   dot dot     → ..  (matched as a 2-token sequence)
///   dot         → .
///   dash, hyphen→ -
///   underscore  → _
///   space       → ' '  (rare but supported)
///
/// Any non-path-token word inside a "run" of path-tokens is treated as a
/// path component and concatenated. We use capitalize_known_dir() to pick
/// the right capitalization for common directory names (Code, Documents,
/// Downloads, Desktop) which voice users say lowercase but the filesystem
/// conventionally has capitalized.
///
/// A "run" is started by encountering tilde/slash/dot/dotdot at any
/// position. The run ends when we encounter whitespace AFTER a non-path-
/// token word (so "cd tilde slash code slash kwin go to bed" → run is
/// "tilde slash code slash kwin", "go to bed" stays as separate words).
fn translate_path_tokens(input: &str) -> String {
    let words: Vec<&str> = input.split_whitespace().collect();
    if words.is_empty() { return input.to_string(); }

    // Strip trailing sentence-end punctuation that whisper appends
    // ("kwin." → "kwin"). We do this by trimming each word's trailing
    // ASCII period/comma/!/? before matching. This isn't lossy — if a
    // user actually says "dot" they spell it out, and that's a separate
    // token. The only false-positive would be a path that legitimately
    // ends with "." which is rare and recoverable.
    fn trim_punct(w: &str) -> &str {
        w.trim_end_matches(|c: char| c == '.' || c == ',' || c == '!' || c == '?')
    }

    let mut out_parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let raw = trim_punct(words[i]);
        let lc = raw.to_lowercase();
        // Is this word the start of a path-token run? Whisper has phonetic
        // confusions: "tilda" and "tilly" for "tilde" are common. We accept
        // these as the start of a path run too.
        let is_path_start = matches!(lc.as_str(),
            "tilde" | "tilda" | "tilly" | "slash" | "backslash" | "dot");
        if !is_path_start {
            out_parts.push(words[i].to_string());
            i += 1;
            continue;
        }
        // Build the path. Continue as long as the next token is a path-
        // token OR an alphanumeric word that's CONTIGUOUS in path-shape
        // (no path token between component words ends the run).
        let mut path = String::new();
        let mut last_was_separator = false;  // whether the last token wrote a separator (slash/tilde/dot)
        while i < words.len() {
            let raw = trim_punct(words[i]);
            let lc = raw.to_lowercase();
            match lc.as_str() {
                "tilde" | "tilda" | "tilly" => { path.push('~'); last_was_separator = true; }
                "slash" => { path.push('/'); last_was_separator = true; }
                "backslash" => { path.push('\\'); last_was_separator = true; }
                "dot" => {
                    if i + 1 < words.len() && trim_punct(words[i+1]).to_lowercase() == "dot" {
                        path.push_str("..");
                        i += 1;
                    } else {
                        path.push('.');
                    }
                    last_was_separator = true;
                }
                "dash" | "hyphen" => { path.push('-'); last_was_separator = false; }
                "underscore" => { path.push('_'); last_was_separator = false; }
                _ => {
                    // Plain word. Three possibilities:
                    //  1. last_was_separator=true: clearly a path component
                    //     (e.g. after "slash"). Consume.
                    //  2. last_was_separator=false but NEXT word is a path-
                    //     separator (slash/dot/tilde): the path run is
                    //     continuing and this word is a fragment whisper
                    //     split off the previous component (e.g. "kde" got
                    //     transcribed as "keg E" → next is "slash kwin").
                    //     Glue this fragment to the previous component
                    //     directly (no space) and continue.
                    //  3. last_was_separator=false and next word is also
                    //     non-separator: this is post-path English. Stop
                    //     the run BEFORE consuming this word.
                    if last_was_separator {
                        path.push_str(&capitalize_known_dir(&lc));
                        last_was_separator = false;
                    } else {
                        let next_is_sep = i + 1 < words.len() && {
                            let n = trim_punct(words[i+1]).to_lowercase();
                            n == "slash" || n == "tilde" || n == "tilda" || n == "tilly"
                                || n == "dot" || n == "backslash"
                        };
                        if next_is_sep {
                            // Glue fragment to the existing path component.
                            // Don't capitalize (it's a fragment, not a known-dir name).
                            path.push_str(&lc);
                        } else {
                            break;
                        }
                    }
                }
            }
            i += 1;
        }
        out_parts.push(path);
    }

    out_parts.join(" ")
}

/// Voice users say directory names lowercased ("code", "documents", "desktop"),
/// but the filesystem usually has them capitalized. Map common ones.
/// For unknown words, return as-is (lowercased) — losing capitalization is
/// less bad than guessing wrong.
fn capitalize_known_dir(w: &str) -> String {
    match w {
        "code" => "Code".into(),
        "documents" => "Documents".into(),
        "downloads" => "Downloads".into(),
        "desktop" => "Desktop".into(),
        "pictures" => "Pictures".into(),
        "videos" => "Videos".into(),
        "music" => "Music".into(),
        "public" => "Public".into(),
        "templates" => "Templates".into(),
        _ => w.to_string(),
    }
}

/// Linear resampling — good enough for speech
#[cfg(feature = "voice")]
fn resample(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate { return input.to_vec(); }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = (input.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let src_idx = src_pos as usize;
        if src_idx >= input.len() { break; }
        let frac = src_pos - src_idx as f64;
        let next_idx = (src_idx + 1).min(input.len() - 1);
        let s = input[src_idx] as f64 * (1.0 - frac) + input[next_idx] as f64 * frac;
        out.push(s as f32);
    }
    out
}

/// Single-pole high-pass filter to attenuate sub-80Hz content. Real
/// speech has fundamentals from ~85Hz (deep male voice) up to ~255Hz
/// (children); harmonics carry intelligibility upward. Sub-80Hz energy
/// is almost always non-speech: AC mains hum (50/60Hz + harmonics),
/// HVAC rumble, traffic, electrical buzz from poor cables, USB noise.
///
/// The filter:
///     y[n] = α * (y[n-1] + x[n] - x[n-1])
///
/// where α = exp(-2π·fc / fs) for cutoff fc and sample rate fs.
/// At fc=80Hz, fs=16000Hz, α ≈ 0.969. This is a very light touch —
/// barely audible on speech, big effect on rumble.
///
/// Applied to the 16kHz resampled signal just before whisper.
/// Cheap: 2 mults + 2 adds per sample, ~32k ops on a 6s capture.
pub fn high_pass_80hz(input: &mut [f32]) {
    if input.len() < 2 { return; }
    // Coefficient for ~80Hz cutoff at 16kHz.
    // α = exp(-2π·80/16000) = exp(-0.0314) ≈ 0.96904
    const ALPHA: f32 = 0.969;
    let mut prev_x = input[0];
    let mut prev_y = 0.0f32;
    for s in input.iter_mut() {
        let x = *s;
        let y = ALPHA * (prev_y + x - prev_x);
        *s = y;
        prev_x = x;
        prev_y = y;
    }
}

/// v0.1.783: gentle pre-whisper gain for quiet captures (soft speech, distant
/// mic, low input gain). Whisper is fairly level-robust but not level-INVARIANT,
/// and very low amplitude loses detail. This scales toward a target RMS but is
/// deliberately conservative so it can't hurt:
///   - never ATTENUATES (gain ≥ 1) — loud audio is left alone;
///   - a hard PEAK CEILING clamps the gain so the signal can never clip;
///   - MAX_GAIN caps boost so a near-silent / noise-only buffer isn't blown up
///     into amplified hiss (which whisper would hallucinate words from);
///   - pure silence (peak below a floor) is skipped entirely.
/// Returns the gain applied (1.0 = untouched) so the caller can log it. Pure +
/// cheap (one accumulating pass, one scale pass). Opt-in via WEB2QML_VOICE_GAIN.
pub fn normalize_gain(input: &mut [f32]) -> f32 {
    if input.is_empty() { return 1.0; }
    let mut peak = 0.0f32;
    let mut sumsq = 0.0f64;
    for &s in input.iter() {
        let a = s.abs();
        if a > peak { peak = a; }
        sumsq += (s as f64) * (s as f64);
    }
    if peak < 1e-4 { return 1.0; }                 // silence — don't amplify the floor
    let rms = (sumsq / input.len() as f64).sqrt() as f32;
    const TARGET_RMS: f32 = 0.10;                  // comfortable speech level
    const PEAK_CEIL: f32 = 0.95;                   // clip guard
    const MAX_GAIN: f32 = 4.0;                     // bound noise amplification
    if rms >= TARGET_RMS { return 1.0; }           // already loud enough
    let gain_rms = TARGET_RMS / rms.max(1e-6);
    let gain_peak = PEAK_CEIL / peak;              // gain that would just reach the ceiling
    let gain = gain_rms.min(gain_peak).min(MAX_GAIN).max(1.0);
    if (gain - 1.0).abs() < 1e-3 { return 1.0; }   // nothing meaningful to do
    for s in input.iter_mut() { *s *= gain; }
    gain
}


/// variation lasting > 3 seconds. Returns `true` if the signal
/// looks like a drone (we should reject the capture as non-speech).
///
/// Approach (cheap, no FFT): compute zero-crossing rate (ZCR) per
/// 200ms window. Real speech has highly variable ZCR — vowels are
/// low ZCR, consonants high. Drones have nearly-constant ZCR (the
/// dominant frequency doesn't change). If 80%+ of windows have ZCR
/// within ±15% of the median, it's a drone.
///
/// Also requires duration ≥ 3s (per user spec: "for more than three
/// seconds are probably not voice"). Shorter captures don't trigger
/// this check — we'd risk false-rejecting genuine short utterances.
///
/// Conservative thresholds chosen to avoid false-rejecting real
/// speech with sustained vowels (e.g. someone saying "ahhhhh"). The
/// 80% windows criterion + ZCR variance band combined make this
/// genuinely picky about what counts as a drone.
pub fn looks_like_drone(audio_16k: &[f32]) -> bool {
    let sr = 16000;
    let min_len = (sr as f32 * 3.0) as usize;
    if audio_16k.len() < min_len { return false; }
    // Compute ZCR per 200ms window.
    let win_len = sr / 5;  // 200ms
    let mut zcrs: Vec<f32> = Vec::with_capacity(audio_16k.len() / win_len + 1);
    let mut idx = 0;
    while idx + win_len <= audio_16k.len() {
        let win = &audio_16k[idx..idx + win_len];
        // Skip near-silent windows — they'd skew the analysis.
        let energy: f32 = win.iter().map(|s| s * s).sum::<f32>() / win.len() as f32;
        if energy < 1e-6 {
            idx += win_len;
            continue;
        }
        let mut crossings = 0u32;
        for w in win.windows(2) {
            if (w[0] >= 0.0) != (w[1] >= 0.0) {
                crossings += 1;
            }
        }
        let zcr = crossings as f32 / win.len() as f32;
        zcrs.push(zcr);
        idx += win_len;
    }
    // Need at least 15 windows (3 seconds of non-silent audio).
    if zcrs.len() < 15 { return false; }
    // Sort to find the median.
    let mut sorted = zcrs.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];
    if median < 0.001 { return false; }  // pure DC — already filtered
    // Count how many windows fall within ±15% of the median.
    let lo = median * 0.85;
    let hi = median * 1.15;
    let in_band = zcrs.iter().filter(|&&z| z >= lo && z <= hi).count();
    let ratio = in_band as f32 / zcrs.len() as f32;
    let is_drone = ratio >= 0.80;
    if is_drone {
        crate::clog!("drone-filter", "reject: {}/{} windows ({:.0}%) within ±15% of median ZCR {:.4}",
            in_band, zcrs.len(), ratio * 100.0, median);
    }
    is_drone
}

/// Fallback: use system tools
fn listen_external() -> String {
    // whisper-cpp / whisper-cli with arecord
    let model = model_path();
    let model_str = model.to_string_lossy();
    let cmd = format!(
        "arecord -d 5 -f S16_LE -r 16000 -c 1 /tmp/web2qml_voice.wav 2>/dev/null && \
         (which whisper-cli >/dev/null && whisper-cli -m '{}' -f /tmp/web2qml_voice.wav --no-prints 2>/dev/null | tail -1 || \
          which whisper-cpp >/dev/null && whisper-cpp -m '{}' -f /tmp/web2qml_voice.wav --no-prints 2>/dev/null | tail -1)",
        model_str, model_str
    );

    if let Ok(output) = std::process::Command::new("sh").arg("-c").arg(&cmd).output() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !text.is_empty() { return text; }
    }

    // nerd-dictation fallback
    if let Ok(output) = std::process::Command::new("sh")
        .arg("-c")
        .arg("which nerd-dictation >/dev/null && timeout 5 nerd-dictation begin --output STDOUT 2>/dev/null")
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !text.is_empty() { return text; }
        }
    }

    let model_path = model_path();
    if !model_path.exists() {
        format!("[Voice tools not available. Build with --features voice and download model to {}]", model_path.display())
    } else {
        "[Voice failed: arecord/whisper not in PATH]".into()
    }
}
